// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Channel {
    Telegram,
    Slack,
    Discord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Destination {
    pub channel: Channel,
    pub installation_id: String,
    pub conversation_id: String,
    pub thread_id: Option<String>,
    pub interaction_id: Option<String>,
    pub expires_ms: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EventSpec {
    pub event_id: String,
    pub session_id: String,
    pub sender_id: String,
    pub prompt: String,
    pub enabled_tools: Vec<String>,
    pub timeout_secs: u64,
    pub destination: Destination,
    pub sealed_token: Option<String>,
    pub fingerprint: String,
}
