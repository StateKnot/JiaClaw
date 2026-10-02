// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Channel {
    Telegram,
    Slack,
    Discord,
    Feishu,
    Wecom,
    Dingtalk,
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

/// A durable proactive destination. It cannot carry interaction credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ScheduledDestination {
    pub channel: Channel,
    pub installation_id: String,
    pub conversation_id: String,
    #[serde(default)]
    pub thread_id: Option<String>,
}

impl ScheduledDestination {
    pub(super) fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.channel != Channel::Discord,
            "scheduled delivery supports Telegram, Slack, Feishu, WeCom or DingTalk, not expiring Discord interactions"
        );
        if self.channel == Channel::Feishu {
            super::feishu::validate_installation(&self.installation_id)?;
            return super::outbound::validate_destination(&self.destination());
        }
        if self.channel == Channel::Dingtalk {
            super::dingtalk::validate_installation(&self.installation_id)?;
            return super::outbound::validate_destination(&self.destination());
        }
        if self.channel == Channel::Wecom {
            super::wecom::validate_installation(&self.installation_id)?;
            return super::outbound::validate_destination(&self.destination());
        }
        anyhow::ensure!(
            !self.installation_id.is_empty()
                && self.installation_id.len() <= 128
                && self
                    .installation_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
            "invalid scheduled installation identity"
        );
        super::outbound::validate_destination(&self.destination())
    }

    pub(super) fn destination(&self) -> Destination {
        Destination {
            channel: self.channel,
            installation_id: self.installation_id.clone(),
            conversation_id: self.conversation_id.clone(),
            thread_id: self.thread_id.clone(),
            interaction_id: None,
            expires_ms: None,
        }
    }
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
