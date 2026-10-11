// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::{shutdown_signal, AgentConfig, PathBuf, Result};
use jiaclaw::ModelProbe;
use std::time::Duration;

pub(super) async fn command(path: PathBuf) -> Result<()> {
    let config = if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        AgentConfig::from_json_file(&path)?
    } else {
        AgentConfig::from_toml_file(&path)?
    };
    eprintln!("Billing confirmed: one no-tools model request, at most 128 output tokens. Private model-call state may be created. No retries; inspect receipts after interruption.");
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut probe = tokio::select! {
        biased;
        () = &mut shutdown => anyhow::bail!("model probe stopped before submission"),
        probe = ModelProbe::open(&config) => probe?,
    };
    let outcome = tokio::select! {
        biased;
        () = &mut shutdown => None,
        receipt = probe.verify() => Some(receipt),
    };
    let Some(receipt) = outcome else {
        eprintln!(
            "Model probe cancellation received; waiting for original receipt owner; no replay."
        );
        probe.settle(Duration::from_secs(5)).await?;
        anyhow::bail!("model probe stopped; inspect original model-calls status/result before another request; no replay");
    };
    let receipt = receipt?;
    println!("{}", serde_json::to_string(&receipt)?);
    anyhow::ensure!(receipt.verified, "model response was persisted but did not match the verification code; inspect the original receipt; no retry");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn model_probe_requires_explicit_config_and_billing_consent() {
        for args in [
            vec!["jiaclaw", "model-probe"],
            vec!["jiaclaw", "model-probe", "--config", "fixture.toml"],
            vec!["jiaclaw", "model-probe", "--confirm-billing"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        let cli = Cli::try_parse_from([
            "jiaclaw",
            "model-probe",
            "--config",
            "fixture.toml",
            "--confirm-billing",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::ModelProbe {
                confirm_billing: true,
                ..
            }
        ));
    }
}
