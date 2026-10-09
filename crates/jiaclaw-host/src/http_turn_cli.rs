// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Local administrator maintenance. No Agent, provider, MCP or runtime is opened.
use super::store::SessionStore;
use anyhow::{ensure, Result};
use clap::Subcommand;
use serde_json::json;
use std::path::Path;

#[derive(Subcommand)]
pub(super) enum Commands {
    /// List metadata only; never load prompts, results or review notes
    List {
        #[arg(long, default_value = "unresolved", value_parser = ["unresolved", "all", "running", "completed", "needs_review"])]
        state: String,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=50))]
        limit: u16,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..=10000))]
        offset: u32,
    },
    /// Read the original receipt including any retained private result and note
    Get { id: String },
    /// Record abandonment after separately reconciling model receipts and effects
    Review {
        id: String,
        #[arg(long)]
        confirm_abandon: bool,
        #[arg(long)]
        note: String,
    },
    /// Remove only a terminal result; preserve identity, review and session history
    PurgeResult {
        id: String,
        #[arg(long)]
        confirm_purge: bool,
    },
}

pub(super) fn run(database: &Path, action: Commands) -> Result<()> {
    // Reject mistakes before even acquiring the lock or opening the database.
    let writable = match &action {
        Commands::List { .. } => false,
        Commands::Get { id } => {
            ensure!(super::jobs::valid_creation_id(id), "invalid_http_turn_id");
            false
        }
        Commands::Review {
            id,
            confirm_abandon,
            note,
        } => {
            ensure!(super::jobs::valid_creation_id(id), "invalid_http_turn_id");
            ensure!(
                *confirm_abandon && !note.trim().is_empty() && note.len() <= 1024,
                "review requires --confirm-abandon and a nonempty 1..1024 byte --note"
            );
            true
        }
        Commands::PurgeResult { id, confirm_purge } => {
            ensure!(super::jobs::valid_creation_id(id), "invalid_http_turn_id");
            ensure!(*confirm_purge, "purge-result requires --confirm-purge");
            true
        }
    };
    let mut store = SessionStore::open_http_maintenance(database, writable)?;
    let output = match action {
        Commands::List {
            state,
            limit,
            offset,
        } => {
            let (turns, has_more) =
                store.list_http_turns(&state, usize::from(limit), offset as usize)?;
            json!({"protocol":1,"offline":true,"state":state,"limit":limit,"offset":offset,"has_more":has_more,"turns":turns})
        }
        Commands::Get { id } => {
            let receipt = store
                .http_receipt(&id, None)?
                .ok_or(super::http_turn_store::Conflict("http_turn_not_found"))?;
            json!({"protocol":1,"offline":true,"receipt":receipt})
        }
        Commands::Review { id, note, .. } => {
            json!({"protocol":1,"offline":true,"receipt":store.review_http_turn(&id, &note)?})
        }
        Commands::PurgeResult { id, .. } => {
            json!({"protocol":1,"offline":true,"receipt":store.purge_http_result(&id)?})
        }
    };
    // A broken stdout never rolls back a committed review or result purge. Use
    // the original identity to inspect the receipt; do not repeat with a new ID.
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
