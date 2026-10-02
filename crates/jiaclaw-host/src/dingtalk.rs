// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Enterprise internal application robot HTTP callbacks, not Stream or generic
//! encrypted event subscriptions. Current primary wire contracts:
//! https://open.dingtalk.com/document/development/robot-message-type.md
//! https://open.dingtalk.com/document/development/receive-message.md
//! The platform MAC covers timestamp + newline + Client Secret, NOT the body.
//! HTTPS and trusted ingress are therefore part of the authentication boundary;
//! callback headers must not enter logs, diagnostics, or durable event records.

use anyhow::{anyhow, ensure, Result};
use axum::http::HeaderMap;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

const MAX_BODY: usize = 128 * 1024;
const MAX_CONTENT: usize = 32 * 1024;
const MAX_JSON_DEPTH: usize = 64;
const MAX_SKEW_MS: u64 = 3_600_000;
const INVALID: &str = "invalid DingTalk callback";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Inbound {
    Ignored,
    Message {
        event_id: String,
        sender_id: String,
        text: String,
    },
}

/// Bounded application-supported spelling for configured platform identifiers.
pub(super) fn identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Keep DingTalk UserID case intact: its API documents a 1..64-character unique
/// enterprise ID, not WeCom's case-insensitive contract. This restricted ASCII
/// subset is our supported policy grammar, not a claimed platform-wide limit.
pub(super) fn user_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'@'))
}

pub(super) fn validate_installation(value: &str) -> Result<(&str, &str)> {
    let (robot_code, corp_id) = value
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid DingTalk installation"))?;
    ensure!(
        value.len() <= 128 && identity(robot_code) && identity(corp_id),
        "invalid DingTalk installation"
    );
    Ok((robot_code, corp_id))
}

// No Debug/Serialize: the reusable signature headers and Client Secret are
// credentials. Every input error is mapped to a single non-sensitive string.
pub(super) struct Callback {
    robot_code: String,
    corp_id: String,
    secret: String,
}

impl Callback {
    pub(super) fn new(installation: &str, secret: String) -> Result<Self> {
        let result = (|| {
            let (robot_code, corp_id) = validate_installation(installation)?;
            ensure!(
                !secret.is_empty()
                    && secret.len() <= 1024
                    && secret.trim() == secret
                    && !secret.chars().any(char::is_control),
                "secret"
            );
            Ok(Self {
                robot_code: robot_code.into(),
                corp_id: corp_id.into(),
                secret,
            })
        })();
        result.map_err(|_: anyhow::Error| anyhow!("invalid DingTalk configuration"))
    }

    pub(super) fn parse_event(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        now_ms: i64,
    ) -> Result<Inbound> {
        self.parse_inner(headers, body, now_ms)
            .map_err(|_| anyhow!(INVALID))
    }

    fn parse_inner(&self, headers: &HeaderMap, body: &[u8], now_ms: i64) -> Result<Inbound> {
        ensure!(!body.is_empty() && body.len() <= MAX_BODY, "body");
        self.verify(headers, now_ms)?;
        // serde's ignored fields must also be depth bounded; never deserialize
        // arbitrary deeply nested unknown fields before checking this ceiling.
        ensure!(bounded_json_depth(body), "JSON nesting");
        let event: Envelope = serde_json::from_slice(body)?;
        ensure!(
            event.robot_code == self.robot_code && event.chatbot_corp_id == self.corp_id,
            "installation"
        );
        if let Some(code) = event.error_code {
            // Quota notifications carry no user text. Recognize only the
            // documented code, after binding the sending robot installation.
            ensure!(code == 20001, "platform error");
            return Ok(Inbound::Ignored);
        }
        let conversation = event
            .conversation_type
            .as_deref()
            .ok_or_else(|| anyhow!("conversation type"))?;
        ensure!(matches!(conversation, "1" | "2"), "conversation type");
        let kind = event.msgtype.as_deref().ok_or_else(|| anyhow!("type"))?;
        if conversation != "1" || kind != "text" {
            return Ok(Inbound::Ignored);
        }
        ensure!(
            event.sender_corp_id.as_deref() == Some(&self.corp_id),
            "sender enterprise"
        );
        let sender_id = event.sender_staff_id.ok_or_else(|| anyhow!("sender"))?;
        ensure!(user_id(&sender_id), "sender");
        let event_id = event.msg_id.ok_or_else(|| anyhow!("message id"))?;
        ensure!(opaque_id(&event_id), "message id");
        ensure!(
            event.conversation_id.as_deref().is_some_and(opaque_id),
            "conversation id"
        );
        let text = event.text.ok_or_else(|| anyhow!("text"))?.content;
        ensure!(!text.trim().is_empty() && text.len() <= MAX_CONTENT, "text");
        // Private replies and scheduled notifications share the precise member
        // UserID destination, never a body-supplied webhook or encrypted ID.
        Ok(Inbound::Message {
            event_id,
            sender_id,
            text,
        })
    }

    fn verify(&self, headers: &HeaderMap, now_ms: i64) -> Result<()> {
        ensure!(
            headers.len() <= 64
                && headers
                    .iter()
                    .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
                    .sum::<usize>()
                    <= 16 * 1024,
            "headers"
        );
        let timestamp = unique_header(headers, "timestamp")?;
        ensure!(
            !timestamp.is_empty()
                && timestamp.len() <= 19
                && timestamp.bytes().all(|b| b.is_ascii_digit())
                && (timestamp == "0" || !timestamp.starts_with('0')),
            "timestamp"
        );
        let signed_ms = timestamp.parse::<i64>()?;
        ensure!(
            now_ms >= 0 && now_ms.abs_diff(signed_ms) <= MAX_SKEW_MS,
            "timestamp"
        );
        let signature = unique_header(headers, "sign")?;
        ensure!(signature.len() == 44, "signature");
        let supplied = STANDARD.decode(signature)?;
        ensure!(supplied.len() == 32, "signature");
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes())?;
        mac.update(timestamp.as_bytes());
        mac.update(b"\n");
        mac.update(self.secret.as_bytes());
        mac.verify_slice(&supplied)
            .map_err(|_| anyhow!("signature"))?;
        Ok(())
    }
}

fn unique_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().ok_or_else(|| anyhow!("header"))?;
    ensure!(values.next().is_none(), "duplicate header");
    Ok(value.to_str()?)
}

fn opaque_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

// JSON syntax and UTF-8 validation remain serde_json's responsibility. This
// linear preflight bounds nesting even in fields ignored by the typed parser.
fn bounded_json_depth(body: &[u8]) -> bool {
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for &byte in body {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > MAX_JSON_DEPTH {
                        return false;
                    }
                }
                b']' | b'}' => {
                    let Some(previous) = depth.checked_sub(1) else {
                        return false;
                    };
                    depth = previous;
                }
                _ => {}
            }
        }
    }
    depth == 0 && !quoted
}

// Typed known fields reject duplicates, including repeated keys after null.
// sessionWebhook, senderId, chatbotUserId, user labels and arbitrary metadata
// are intentionally ignored rather than stored or interpreted as authority.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    robot_code: String,
    chatbot_corp_id: String,
    sender_corp_id: Option<String>,
    sender_staff_id: Option<String>,
    conversation_type: Option<String>,
    conversation_id: Option<String>,
    msg_id: Option<String>,
    msgtype: Option<String>,
    text: Option<Text>,
    error_code: Option<u32>,
}

#[derive(Deserialize)]
struct Text {
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::{json, Value};

    const NOW: i64 = 1_577_262_236_757;
    const SECRET: &str = "this is a secret";
    const SIGNATURE: &str = "DJrE6qdyVGCQz9z5r2MDuNcNAhwYnuAkyj13cx169CA=";

    fn callback() -> Callback {
        Callback::new("dingRobot:dingCorp", SECRET.into()).unwrap()
    }
    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("timestamp", HeaderValue::from_static("1577262236757"));
        // Fixed official example inputs, calculated independently with Python
        // hmac/hashlib, not by the implementation under test.
        headers.insert("sign", HeaderValue::from_static(SIGNATURE));
        headers
    }
    fn body() -> Value {
        json!({
            "robotCode": "dingRobot", "chatbotCorpId": "dingCorp",
            "senderCorpId": "dingCorp", "senderStaffId": "Alice_01@example.com",
            "conversationType": "1", "conversationId": "cidAa+/Bb==",
            "msgId": "msgAa+/Bb==", "msgtype": "text",
            "text": { "content": " 你好，原样保留\n" },
            "senderId": "$:LWCP_v1:$sender", "chatbotUserId": "$:LWCP_v1:$bot",
            "sessionWebhook": "https://untrusted.invalid/credential?session=secret"
        })
    }
    fn parse(value: &Value) -> Result<Inbound> {
        callback().parse_event(&headers(), &serde_json::to_vec(value).unwrap(), NOW)
    }
    fn invalid(headers: &HeaderMap, body: &[u8], now: i64) {
        assert_eq!(
            callback()
                .parse_event(headers, body, now)
                .unwrap_err()
                .to_string(),
            INVALID
        );
    }

    #[test]
    fn official_signature_vector_accepts_private_text_and_preserves_member_case() {
        assert_eq!(
            parse(&body()).unwrap(),
            Inbound::Message {
                event_id: "msgAa+/Bb==".into(),
                sender_id: "Alice_01@example.com".into(),
                text: " 你好，原样保留\n".into(),
            }
        );
        // The wire protocol does not MAC the body. Capture this limitation so
        // a future refactor cannot falsely advertise body-integrity guarantees.
        let mut changed = body();
        changed["text"]["content"] = json!("different text with the same signature");
        assert!(parse(&changed).is_ok());
    }

    #[test]
    fn signature_timestamp_and_headers_fail_closed_without_credential_diagnostics() {
        let wire = serde_json::to_vec(&body()).unwrap();
        for now in [NOW - 3_600_001, NOW + 3_600_001, -1, i64::MAX] {
            invalid(&headers(), &wire, now);
        }
        for now in [NOW - 3_600_000, NOW + 3_600_000] {
            assert!(callback().parse_event(&headers(), &wire, now).is_ok());
        }
        for name in ["timestamp", "sign"] {
            let mut missing = headers();
            missing.remove(name);
            invalid(&missing, &wire, NOW);
            let mut duplicate = headers();
            duplicate.append(name, headers()[name].clone());
            invalid(&duplicate, &wire, NOW);
        }
        for value in [
            "",
            "-1",
            "01577262236757",
            "1577262236757.0",
            "1577262236757,1577262236757",
            "9223372036854775808",
        ] {
            let mut bad = headers();
            bad.insert("timestamp", value.parse().unwrap());
            invalid(&bad, &wire, NOW);
        }
        for value in [
            "",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "DJrE6qdyVGCQz9z5r2MDuNcNAhwYnuAkyj13cx169CA%3D",
            "DJrE6qdyVGCQz9z5r2MDuNcNAhwYnuAkyj13cx169CA",
            "DJrE6qdyVGCQz9z5r2MDuNcNAhwYnuAkyj13cx169CB=",
        ] {
            let mut bad = headers();
            bad.insert("sign", value.parse().unwrap());
            invalid(&bad, &wire, NOW);
        }
        let mut oversized = headers();
        oversized.insert("x-unused", "x".repeat(16 * 1024).parse().unwrap());
        invalid(&oversized, &wire, NOW);
        let mut too_many = headers();
        for i in 0..65 {
            too_many.insert(
                format!("x-{i}").parse::<axum::http::HeaderName>().unwrap(),
                HeaderValue::from_static("x"),
            );
        }
        invalid(&too_many, &wire, NOW);
    }

    #[test]
    fn exact_installation_and_enterprise_member_are_required_before_admission() {
        for field in [
            "robotCode",
            "chatbotCorpId",
            "senderCorpId",
            "senderStaffId",
            "conversationId",
            "msgId",
        ] {
            let mut missing = body();
            missing.as_object_mut().unwrap().remove(field);
            assert_eq!(parse(&missing).unwrap_err().to_string(), INVALID, "{field}");
            let mut empty = body();
            empty[field] = json!("");
            assert!(parse(&empty).is_err(), "{field}");
        }
        for field in ["robotCode", "chatbotCorpId", "senderCorpId"] {
            let mut foreign = body();
            foreign[field] = json!("another-installation");
            assert!(parse(&foreign).is_err(), "{field}");
        }
        for sender in [
            "@all",
            "Alice|Bob",
            "Alice,Bob",
            " Alice",
            "Alice ",
            "张三",
            "a\nb",
            "-alice",
        ] {
            let mut bad = body();
            bad["senderStaffId"] = json!(sender);
            assert!(parse(&bad).is_err(), "{sender}");
        }
        let mut long = body();
        long["senderStaffId"] = json!("a".repeat(65));
        assert!(parse(&long).is_err());
        for field in ["msgId", "conversationId"] {
            for value in ["a".repeat(257), "a\nb".into(), " ".into()] {
                let mut bad = body();
                bad[field] = json!(value);
                assert!(parse(&bad).is_err(), "{field}");
            }
        }
    }

    #[test]
    fn authenticated_unsupported_messages_and_quota_notices_do_not_run_agents() {
        let mut group = body();
        group["conversationType"] = json!("2");
        group.as_object_mut().unwrap().remove("senderStaffId");
        assert_eq!(parse(&group).unwrap(), Inbound::Ignored);
        let mut picture = body();
        picture["msgtype"] = json!("picture");
        picture.as_object_mut().unwrap().remove("text");
        assert_eq!(parse(&picture).unwrap(), Inbound::Ignored);
        let quota = json!({"robotCode":"dingRobot", "chatbotCorpId":"dingCorp", "errorCode":20001, "errorMessage":"provider limit"});
        assert_eq!(parse(&quota).unwrap(), Inbound::Ignored);
        let mut unknown_error = quota.clone();
        unknown_error["errorCode"] = json!(500);
        assert!(parse(&unknown_error).is_err());
        let mut foreign = quota;
        foreign["robotCode"] = json!("another-robot");
        assert!(parse(&foreign).is_err());
        for kind in [json!(1), json!("3"), Value::Null] {
            let mut bad = body();
            bad["conversationType"] = kind;
            assert!(parse(&bad).is_err());
        }
    }

    #[test]
    fn duplicate_authority_and_text_fields_reject_even_after_null() {
        let serialized = serde_json::to_string(&body()).unwrap();
        for (name, value) in [
            ("robotCode", "\"dingRobot\""),
            ("chatbotCorpId", "\"dingCorp\""),
            ("senderStaffId", "null"),
            ("msgId", "null"),
            ("conversationType", "null"),
            ("errorCode", "null"),
            ("text", "null"),
        ] {
            let duplicate = format!("{{\"{name}\":{value},{}", &serialized[1..]);
            // errorCode is absent in the base body; give it a second value.
            let duplicate = if name == "errorCode" {
                duplicate.replacen("{", "{\"errorCode\":20001,", 1)
            } else {
                duplicate
            };
            invalid(&headers(), duplicate.as_bytes(), NOW);
        }
        let duplicate_text =
            serialized.replace("\"content\":", "\"content\":\"first\",\"content\":");
        invalid(&headers(), duplicate_text.as_bytes(), NOW);
    }

    #[test]
    fn bounds_cover_utf8_text_unknown_nesting_and_malformed_json() {
        for wire in [
            &b""[..],
            &b"null"[..],
            &b"[]"[..],
            &b"{} trailing"[..],
            &b"{\xff}"[..],
            &b"{\"robotCode\":1}"[..],
        ] {
            invalid(&headers(), wire, NOW);
        }
        invalid(&headers(), &vec![b' '; MAX_BODY + 1], NOW);
        for value in ["".into(), " \n\t".into(), "中".repeat(MAX_CONTENT / 3 + 1)] {
            let mut bad = body();
            bad["text"]["content"] = json!(value);
            assert!(parse(&bad).is_err());
        }
        let mut max = body();
        max["text"]["content"] = json!("x".repeat(MAX_CONTENT));
        assert!(parse(&max).is_ok());
        let mut valid_escapes = body();
        valid_escapes["text"]["content"] = json!("[ { \\\" } ] \\");
        assert!(parse(&valid_escapes).is_ok());
        let serialized = serde_json::to_string(&body()).unwrap();
        let nested = format!(
            "{{\"ignored\":{}0{},{}",
            "[".repeat(MAX_JSON_DEPTH),
            "]".repeat(MAX_JSON_DEPTH),
            &serialized[1..]
        );
        invalid(&headers(), nested.as_bytes(), NOW);
    }

    #[test]
    fn configuration_and_policy_identifiers_are_bounded_and_case_preserving() {
        assert_eq!(
            validate_installation("dingRobot:dingCorp").unwrap(),
            ("dingRobot", "dingCorp")
        );
        assert!(identity("App-Key_1.2"));
        assert!(user_id("Alice_01@example.com"));
        assert!(user_id(&"A".repeat(64)));
        for bad in [
            "",
            "robot",
            ":corp",
            "robot:",
            "robot:corp:extra",
            "robot:corp/evil",
            "机器人:corp",
        ] {
            assert_eq!(
                validate_installation(bad).unwrap_err().to_string(),
                "invalid DingTalk installation"
            );
        }
        assert!(validate_installation(&format!("{}:{}", "a".repeat(64), "b".repeat(64))).is_err());
        for secret in [
            "".into(),
            "secret\n".into(),
            " secret".into(),
            "x".repeat(1025),
        ] {
            let error = Callback::new("dingRobot:dingCorp", secret).err().unwrap();
            assert_eq!(error.to_string(), "invalid DingTalk configuration");
        }
    }
}
