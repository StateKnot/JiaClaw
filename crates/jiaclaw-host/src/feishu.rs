// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Feishu enterprise-app callbacks. Protocol reference: larksuite/oapi-sdk-go
//! 99927aa13e271ea9fe03591204aad7bc6a2d869c, event/event.go and
//! event/dispatcher/dispatcher.go. Signatures cover the original HTTP body;
//! encrypted bodies carry their CBC IV in the first 16 decoded bytes.

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use anyhow::{anyhow, ensure, Result};
use axum::http::HeaderMap;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const MAX_BODY_BYTES: usize = 128 * 1024;
const MAX_PLAINTEXT_BYTES: usize = 64 * 1024;
const MAX_PROMPT_BYTES: usize = 32 * 1024;
const MAX_CIPHERTEXT_BYTES: usize = MAX_PLAINTEXT_BYTES + 32;
const MAX_ENCODED_BYTES: usize = MAX_CIPHERTEXT_BYTES.div_ceil(3) * 4;
const INVALID_CALLBACK: &str = "invalid Feishu callback";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Inbound {
    Challenge(String),
    Ignored,
    Message {
        event_id: String,
        sender_id: String,
        conversation_id: String,
        thread_id: Option<String>,
        text: String,
    },
}

pub(super) fn validate_installation(installation: &str) -> Result<(&str, &str)> {
    let (app_id, tenant_key) = installation
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid Feishu installation"))?;
    ensure!(
        installation.len() <= 128 && identifier(app_id, "cli_") && identifier(tenant_key, ""),
        "invalid Feishu installation"
    );
    Ok((app_id, tenant_key))
}

/// All untrusted input failures deliberately have the same public error. No
/// callback payload, token, signature or decryption diagnostic enters logs.
pub(super) fn parse_event(
    headers: &HeaderMap,
    body: &[u8],
    installation: &str,
    encrypt_key: &str,
    verification_token: &str,
    now_secs: u64,
) -> Result<Inbound> {
    parse_inner(
        headers,
        body,
        installation,
        encrypt_key,
        verification_token,
        now_secs,
    )
    .map_err(|_| anyhow!(INVALID_CALLBACK))
}

#[derive(Deserialize)]
struct Envelope {
    encrypt: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    token: Option<String>,
    challenge: Option<String>,
    schema: Option<String>,
    header: Option<Box<RawValue>>,
    event: Option<Box<RawValue>>,
}

#[derive(Deserialize)]
struct EventHeader {
    event_id: String,
    event_type: String,
    app_id: String,
    tenant_key: String,
    token: String,
}

#[derive(Deserialize)]
struct MessageEvent {
    sender: Box<RawValue>,
    message: Box<RawValue>,
}

#[derive(Deserialize)]
struct Sender {
    sender_type: String,
    sender_id: Option<Box<RawValue>>,
    tenant_key: Option<String>,
}

#[derive(Deserialize)]
struct SenderId {
    open_id: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    message_id: String,
    chat_id: String,
    chat_type: String,
    message_type: String,
    root_id: Option<String>,
    content: String,
    // Native thread_id (omt_...) is deliberately ignored. A root message may
    // carry it before root_id exists; replies use om_ root_id/message_id.
}

#[derive(Deserialize)]
struct TextContent {
    text: String,
}

fn parse_inner(
    headers: &HeaderMap,
    body: &[u8],
    installation: &str,
    encrypt_key: &str,
    verification_token: &str,
    now_secs: u64,
) -> Result<Inbound> {
    ensure!(!body.is_empty() && body.len() <= MAX_BODY_BYTES, "body");
    ensure!(
        !encrypt_key.is_empty()
            && encrypt_key.len() <= 1024
            && !verification_token.is_empty()
            && verification_token.len() <= 1024,
        "credentials"
    );
    ensure!(
        headers.len() <= 64
            && headers
                .iter()
                .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
                .sum::<usize>()
                <= 16 * 1024,
        "headers"
    );
    let (app_id, tenant_key) = validate_installation(installation)?;
    let mut envelope: Envelope = decode_object(body)?;
    if let Some(encrypted) = envelope.encrypt.take() {
        ensure!(
            envelope.kind.is_none()
                && envelope.token.is_none()
                && envelope.challenge.is_none()
                && envelope.schema.is_none()
                && envelope.header.is_none()
                && envelope.event.is_none(),
            "mixed envelope"
        );
        envelope = decode_object(&decrypt(&encrypted, encrypt_key)?)?;
        ensure!(envelope.encrypt.is_none(), "nested encryption");
    }

    // The official SDK exempts URL verification from the ordinary event
    // signature. The verification token is still mandatory, including inside
    // encrypted URL-verification requests.
    if envelope.kind.as_deref() == Some("url_verification") {
        let token = envelope.token.ok_or_else(|| anyhow!("token"))?;
        let challenge = envelope.challenge.ok_or_else(|| anyhow!("challenge"))?;
        ensure!(
            secret_matches(&token, verification_token)
                && !challenge.is_empty()
                && challenge.len() <= 1024
                && envelope.header.is_none()
                && envelope.event.is_none()
                && envelope.schema.is_none(),
            "challenge"
        );
        return Ok(Inbound::Challenge(challenge));
    }

    verify_signature(headers, body, encrypt_key, now_secs)?;
    ensure!(envelope.schema.as_deref() == Some("2.0"), "schema");
    let header: EventHeader = decode_object(
        envelope
            .header
            .ok_or_else(|| anyhow!("header"))?
            .get()
            .as_bytes(),
    )?;
    ensure!(
        header.app_id == app_id
            && header.tenant_key == tenant_key
            && secret_matches(&header.token, verification_token)
            && identifier(&header.event_id, "")
            && !header.event_type.is_empty()
            && header.event_type.len() <= 128,
        "event identity"
    );
    if header.event_type != "im.message.receive_v1" {
        return Ok(Inbound::Ignored);
    }
    let event: MessageEvent = decode_object(
        envelope
            .event
            .ok_or_else(|| anyhow!("event"))?
            .get()
            .as_bytes(),
    )?;
    let sender: Sender = decode_object(event.sender.get().as_bytes())?;
    ensure!(
        sender
            .tenant_key
            .as_deref()
            .is_none_or(|value| value == tenant_key),
        "sender tenant"
    );
    if sender.sender_type != "user" {
        return Ok(Inbound::Ignored);
    }
    let sender_id: SenderId = decode_object(
        sender
            .sender_id
            .ok_or_else(|| anyhow!("sender"))?
            .get()
            .as_bytes(),
    )?;
    let sender_id = sender_id.open_id.ok_or_else(|| anyhow!("sender"))?;
    ensure!(identifier(&sender_id, "ou_"), "sender id");
    let mut message: Message = decode_object(event.message.get().as_bytes())?;
    // Treat an explicitly empty optional root like an absent root. Do not trim:
    // whitespace or a native omt_ topic ID is not a valid om_ reply target.
    message.root_id = message.root_id.filter(|root| !root.is_empty());
    ensure!(
        identifier(&message.message_id, "om_")
            && identifier(&message.chat_id, "oc_")
            && message
                .root_id
                .as_deref()
                .is_none_or(|root| identifier(root, "om_")),
        "message identity"
    );
    if message.message_type != "text" {
        return Ok(Inbound::Ignored);
    }
    let thread_id = match message.chat_type.as_str() {
        "p2p" => None,
        "group" => Some(
            message
                .root_id
                .unwrap_or_else(|| message.message_id.clone()),
        ),
        _ => return Ok(Inbound::Ignored),
    };
    let content: TextContent = decode_object(message.content.as_bytes())?;
    ensure!(
        !content.text.trim().is_empty() && content.text.len() <= MAX_PROMPT_BYTES,
        "text"
    );
    Ok(Inbound::Message {
        // The same platform message can arrive under different delivery event
        // IDs. Persistent deduplication must use the message ID.
        event_id: message.message_id,
        sender_id,
        conversation_id: message.chat_id,
        thread_id,
        text: content.text,
    })
}

pub(super) fn decode_object<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ensure!(
        bytes.iter().copied().find(|b| !b.is_ascii_whitespace()) == Some(b'{'),
        "object required"
    );
    // Decode the original bytes at every object boundary; Value would erase duplicates.
    Ok(serde_json::from_slice(bytes)?)
}

fn identifier(value: &str, prefix: &str) -> bool {
    value.len() > prefix.len()
        && value.len() <= 128
        && value.starts_with(prefix)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn secret_matches(supplied: &str, expected: &str) -> bool {
    // Fixed-size digest comparison also keeps token length out of comparison
    // timing; values are bounded before hashing.
    supplied.len() <= 1024 && bool::from(Sha256::digest(supplied).ct_eq(&Sha256::digest(expected)))
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str, max: usize) -> Result<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .ok_or_else(|| anyhow!("missing header"))?
        .to_str()?;
    ensure!(
        values.next().is_none() && !value.is_empty() && value.len() <= max,
        "header shape"
    );
    Ok(value)
}

fn verify_signature(
    headers: &HeaderMap,
    body: &[u8],
    encrypt_key: &str,
    now_secs: u64,
) -> Result<()> {
    let timestamp = single_header(headers, "x-lark-request-timestamp", 20)?;
    let nonce = single_header(headers, "x-lark-request-nonce", 256)?;
    let signature = single_header(headers, "x-lark-signature", 64)?;
    ensure!(
        timestamp.bytes().all(|byte| byte.is_ascii_digit())
            && now_secs.abs_diff(timestamp.parse::<u64>()?) <= 300
            && nonce.bytes().all(|byte| byte.is_ascii_graphic())
            && signature.len() == 64,
        "signature metadata"
    );
    let mut supplied = [0_u8; 32];
    for (pair, output) in signature.as_bytes().chunks_exact(2).zip(&mut supplied) {
        let high = char::from(pair[0])
            .to_digit(16)
            .ok_or_else(|| anyhow!("signature encoding"))?;
        let low = char::from(pair[1])
            .to_digit(16)
            .ok_or_else(|| anyhow!("signature encoding"))?;
        *output = u8::try_from((high << 4) | low)?;
    }
    let mut digest = Sha256::new();
    digest.update(timestamp);
    digest.update(nonce);
    digest.update(encrypt_key);
    digest.update(body);
    ensure!(
        bool::from(digest.finalize().as_slice().ct_eq(&supplied)),
        "signature"
    );
    Ok(())
}

fn decrypt(encoded: &str, encrypt_key: &str) -> Result<Vec<u8>> {
    ensure!(
        !encoded.is_empty() && encoded.len() <= MAX_ENCODED_BYTES,
        "encrypted size"
    );
    let mut bytes = STANDARD.decode(encoded)?;
    ensure!(
        bytes.len() >= 32 && bytes.len() <= MAX_CIPHERTEXT_BYTES && bytes.len() % 16 == 0,
        "ciphertext size"
    );
    let key = Sha256::digest(encrypt_key);
    let (iv, ciphertext) = bytes.split_at_mut(16);
    let plaintext = cbc::Decryptor::<aes::Aes256>::new_from_slices(&key, iv)
        .map_err(|_| anyhow!("cipher key"))?
        .decrypt_padded_mut::<Pkcs7>(ciphertext)
        .map_err(|_| anyhow!("cipher padding"))?;
    ensure!(plaintext.len() <= MAX_PLAINTEXT_BYTES, "plaintext size");
    Ok(plaintext.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncryptMut;
    use axum::http::HeaderValue;
    use serde_json::{json, Value};

    const INSTALLATION: &str = "cli_app123:tenant_123";
    const KEY: &str = "test-encrypt-key";
    const TOKEN: &str = "test-verification-token";
    const NOW: u64 = 1_800_000_000;

    fn message() -> Value {
        json!({
            "schema": "2.0",
            "header": {
                "event_id": "7c7de5ce-ef08-4de5-a5b4-c30d982f8487",
                "event_type": "im.message.receive_v1",
                "app_id": "cli_app123",
                "tenant_key": "tenant_123",
                "token": TOKEN,
                "create_time": "1800000000000"
            },
            "event": {
                "sender": {
                    "sender_type": "user",
                    "sender_id": {"open_id": "ou_person1"},
                    "tenant_key": "tenant_123"
                },
                "message": {
                    "message_id": "om_message1",
                    "chat_id": "oc_chat1",
                    "chat_type": "p2p",
                    "message_type": "text",
                    "content": "{\"text\":\"你好，飞书\"}"
                }
            }
        })
    }

    fn sign(body: &[u8], timestamp: u64) -> HeaderMap {
        let timestamp = timestamp.to_string();
        let nonce = "fixture-nonce";
        let mut hasher = Sha256::new();
        hasher.update(timestamp.as_bytes());
        hasher.update(nonce.as_bytes());
        hasher.update(KEY.as_bytes());
        hasher.update(body);
        let signature = format!("{:x}", hasher.finalize());
        let mut headers = HeaderMap::new();
        headers.insert("x-lark-request-timestamp", timestamp.parse().unwrap());
        headers.insert("x-lark-request-nonce", nonce.parse().unwrap());
        headers.insert("x-lark-signature", signature.parse().unwrap());
        headers
    }

    fn parse(value: &Value) -> Result<Inbound> {
        let body = serde_json::to_vec(value).unwrap();
        parse_event(&sign(&body, NOW), &body, INSTALLATION, KEY, TOKEN, NOW)
    }

    fn encrypt(plaintext: &[u8]) -> String {
        let key = Sha256::digest(KEY);
        let iv = [0x35; 16];
        let mut buffer = vec![0; plaintext.len() + 16];
        buffer[..plaintext.len()].copy_from_slice(plaintext);
        let ciphertext = cbc::Encryptor::<aes::Aes256>::new_from_slices(&key, &iv)
            .unwrap()
            .encrypt_padded_mut::<Pkcs7>(&mut buffer, plaintext.len())
            .unwrap();
        let mut combined = iv.to_vec();
        combined.extend_from_slice(ciphertext);
        STANDARD.encode(combined)
    }

    fn encrypted(value: &Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"encrypt": encrypt(&serde_json::to_vec(value).unwrap())}))
            .unwrap()
    }

    fn rejected(result: Result<Inbound>) {
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), INVALID_CALLBACK);
        assert_eq!(format!("{error:#}"), INVALID_CALLBACK);
    }

    #[test]
    fn installation_is_unambiguous_and_bounded() {
        assert_eq!(
            validate_installation(INSTALLATION).unwrap(),
            ("cli_app123", "tenant_123")
        );
        let longest = format!("cli_a:{}", "x".repeat(122));
        assert_eq!(longest.len(), 128);
        assert!(validate_installation(&longest).is_ok());
        for invalid in [
            "",
            "cli_x",
            "cli_x:",
            ":tenant",
            "cli_:tenant",
            "app:tenant",
            "cli_x:t:extra",
            "cli_x:t/other",
            "cli_x:租户",
            " cli_x:t",
            "cli_x:t ",
        ] {
            assert!(validate_installation(invalid).is_err(), "{invalid}");
        }
        assert!(validate_installation(&(longest + "x")).is_err());
    }

    #[test]
    fn official_aes_sample_and_strict_padding() {
        // Feishu's independent published sample (Python, Java and Node examples):
        // https://open.feishu.cn/document/ukTMukTMukTM/uYDNxYjL2QTM24iN0EjN/event-subscription-configure-/encrypt-key-encryption-configuration-case
        let encoded = "P37w+VZImNgPEO1RBhJ6RtKl7n6zymIbEG1pReEzghk=";
        assert_eq!(decrypt(encoded, "test key").unwrap(), b"hello world");
        let original = STANDARD.decode(encoded).unwrap();
        // Altering the IV changes only the corresponding plaintext byte. The
        // sample's five 0x05 bytes must all match the padding length.
        for final_padding in [0, 4, 17] {
            let mut bad = original.clone();
            bad[15] ^= 5 ^ final_padding;
            assert!(decrypt(&STANDARD.encode(bad), "test key").is_err());
        }
        let mut bad = original;
        bad[14] ^= 1;
        assert!(decrypt(&STANDARD.encode(bad), "test key").is_err());
        for invalid in ["", "!not_base64!", "AAAA", "AAAAAAAAAAAAAAAAAAAAAA=="] {
            assert!(decrypt(invalid, KEY).is_err());
        }
        assert!(decrypt(&STANDARD.encode([0; 33]), KEY).is_err());
        assert!(decrypt(encoded, "wrong key").is_err());
    }

    #[test]
    fn signed_plain_and_encrypted_messages_use_message_id_for_deduplication() {
        let mut event = message();
        let expected = Inbound::Message {
            event_id: "om_message1".into(),
            sender_id: "ou_person1".into(),
            conversation_id: "oc_chat1".into(),
            thread_id: None,
            text: "你好，飞书".into(),
        };
        assert_eq!(parse(&event).unwrap(), expected);
        event["header"]["event_id"] = json!("second-delivery-of-the-same-message");
        assert_eq!(parse(&event).unwrap(), expected);
        let body = encrypted(&event);
        assert_eq!(
            parse_event(&sign(&body, NOW), &body, INSTALLATION, KEY, TOKEN, NOW).unwrap(),
            expected
        );
        // The signature must cover the ciphertext envelope, not its plaintext.
        let plaintext = serde_json::to_vec(&event).unwrap();
        rejected(parse_event(
            &sign(&plaintext, NOW),
            &body,
            INSTALLATION,
            KEY,
            TOKEN,
            NOW,
        ));
    }

    #[test]
    fn challenge_requires_token_even_when_signature_is_optional() {
        let challenge = json!({"type":"url_verification","token":TOKEN,"challenge":"verify-me"});
        for body in [
            serde_json::to_vec(&challenge).unwrap(),
            encrypted(&challenge),
        ] {
            assert_eq!(
                parse_event(&HeaderMap::new(), &body, INSTALLATION, KEY, TOKEN, NOW).unwrap(),
                Inbound::Challenge("verify-me".into())
            );
        }
        for token in [json!("wrong-secret"), Value::Null, json!(5)] {
            let mut wrong = challenge.clone();
            wrong["token"] = token;
            rejected(parse_event(
                &HeaderMap::new(),
                &serde_json::to_vec(&wrong).unwrap(),
                INSTALLATION,
                KEY,
                TOKEN,
                NOW,
            ));
            rejected(parse_event(
                &HeaderMap::new(),
                &encrypted(&wrong),
                INSTALLATION,
                KEY,
                TOKEN,
                NOW,
            ));
        }
        for value in [
            json!(""),
            json!("x".repeat(1025)),
            json!({"text":"challenge"}),
        ] {
            let mut wrong = challenge.clone();
            wrong["challenge"] = value;
            rejected(parse(&wrong));
        }
        let mut mixed = message();
        mixed["type"] = json!("url_verification");
        mixed["token"] = json!(TOKEN);
        mixed["challenge"] = json!("bypass");
        rejected(parse(&mixed));
    }

    #[test]
    fn ordinary_event_requires_fresh_unique_signature_headers() {
        let body = serde_json::to_vec(&message()).unwrap();
        for timestamp in [NOW - 300, NOW + 300] {
            assert!(parse_event(
                &sign(&body, timestamp),
                &body,
                INSTALLATION,
                KEY,
                TOKEN,
                NOW
            )
            .is_ok());
        }
        for timestamp in [NOW - 301, NOW + 301, 0, u64::MAX] {
            rejected(parse_event(
                &sign(&body, timestamp),
                &body,
                INSTALLATION,
                KEY,
                TOKEN,
                NOW,
            ));
        }
        for header in [
            "x-lark-request-timestamp",
            "x-lark-request-nonce",
            "x-lark-signature",
        ] {
            let mut headers = sign(&body, NOW);
            headers.remove(header);
            rejected(parse_event(&headers, &body, INSTALLATION, KEY, TOKEN, NOW));
            let mut headers = sign(&body, NOW);
            headers.append(header, headers[header].clone());
            rejected(parse_event(&headers, &body, INSTALLATION, KEY, TOKEN, NOW));
        }
        for (header, value) in [
            ("x-lark-request-timestamp", "+1800000000".to_string()),
            (
                "x-lark-request-timestamp",
                "18446744073709551616".to_string(),
            ),
            ("x-lark-request-nonce", "x".repeat(257)),
            ("x-lark-request-nonce", " ".to_string()),
            ("x-lark-signature", "g".repeat(64)),
            ("x-lark-signature", "0".repeat(64)),
        ] {
            let mut headers = sign(&body, NOW);
            headers.insert(header, value.parse().unwrap());
            rejected(parse_event(&headers, &body, INSTALLATION, KEY, TOKEN, NOW));
        }
        let mut headers = sign(&body, NOW);
        headers.insert(
            "oversized",
            HeaderValue::from_str(&"x".repeat(16 * 1024)).unwrap(),
        );
        rejected(parse_event(&headers, &body, INSTALLATION, KEY, TOKEN, NOW));
    }

    #[test]
    fn authentication_precedes_ignoring_bots_nontext_and_unknown_events() {
        for kind in ["bot", "image", "unknown"] {
            let mut event = message();
            match kind {
                "bot" => event["event"]["sender"]["sender_type"] = json!("app"),
                "image" => {
                    event["event"]["message"]["message_type"] = json!("image");
                    event["event"]["message"]["content"] = json!("{\"image_key\":\"img_abc\"}");
                }
                _ => {
                    event["header"]["event_type"] = json!("im.chat.member.user.added_v1");
                    event["event"] = json!({"chat_id":"oc_chat1"});
                }
            }
            assert_eq!(parse(&event).unwrap(), Inbound::Ignored);
            for field in ["token", "app_id", "tenant_key"] {
                let mut wrong = event.clone();
                wrong["header"][field] = json!("wrong");
                rejected(parse(&wrong));
            }
            let body = serde_json::to_vec(&event).unwrap();
            rejected(parse_event(
                &HeaderMap::new(),
                &body,
                INSTALLATION,
                KEY,
                TOKEN,
                NOW,
            ));
            if kind != "unknown" {
                event["event"]["sender"]["tenant_key"] = json!("different_tenant");
                rejected(parse(&event));
            }
        }
    }

    #[test]
    fn message_schema_identity_and_text_are_strict() {
        for (pointer, invalid) in [
            ("/schema", json!("1.0")),
            ("/header/event_id", json!("")),
            ("/header/event_id", json!("x".repeat(129))),
            ("/header/event_id", json!("id/another")),
            ("/header/token", Value::Null),
            ("/event/sender/sender_id/open_id", json!("user_123")),
            ("/event/sender/sender_id/open_id", json!("ou_")),
            ("/event/sender/tenant_key", json!("other_tenant")),
            ("/event/message/message_id", json!("omt_not_a_message")),
            ("/event/message/message_id", json!("om_x/../../bad")),
            ("/event/message/chat_id", json!("ou_not_a_chat")),
            (
                "/event/message/content",
                json!({"text":"object, not JSON string"}),
            ),
            ("/event/message/content", json!("{\"text\":7}")),
            ("/event/message/content", json!("{\"text\":\" \"}")),
            ("/event/message/content", json!("{\"image_key\":\"img\"}")),
            ("/event/message/content", json!("not-json")),
        ] {
            let mut event = message();
            *event.pointer_mut(pointer).unwrap() = invalid;
            rejected(parse(&event));
        }
        let mut event = message();
        event["event"]["sender"]
            .as_object_mut()
            .unwrap()
            .remove("tenant_key");
        assert!(parse(&event).is_ok());
    }

    #[test]
    fn group_routing_uses_root_message_and_never_native_thread_id() {
        let mut event = message();
        event["event"]["message"]["chat_type"] = json!("group");
        for native_thread in [None, Some("omt_native_topic")] {
            if let Some(thread) = native_thread {
                event["event"]["message"]["thread_id"] = json!(thread);
            }
            let Inbound::Message { thread_id, .. } = parse(&event).unwrap() else {
                panic!("message");
            };
            assert_eq!(thread_id.as_deref(), Some("om_message1"));
        }
        event["event"]["message"]["root_id"] = json!("");
        let Inbound::Message { thread_id, .. } = parse(&event).unwrap() else {
            panic!("message");
        };
        assert_eq!(thread_id.as_deref(), Some("om_message1"));
        event["event"]["message"]["root_id"] = json!("om_root1");
        let Inbound::Message { thread_id, .. } = parse(&event).unwrap() else {
            panic!("message");
        };
        assert_eq!(thread_id.as_deref(), Some("om_root1"));
        event["event"]["message"]["chat_type"] = json!("p2p");
        let Inbound::Message { thread_id, .. } = parse(&event).unwrap() else {
            panic!("message");
        };
        assert_eq!(thread_id, None);
        event["event"]["message"]["root_id"] = json!("");
        let Inbound::Message { thread_id, .. } = parse(&event).unwrap() else {
            panic!("message");
        };
        assert_eq!(thread_id, None);
        for root in [" ", "omt_topic", "om_x/other"] {
            event["event"]["message"]["root_id"] = json!(root);
            rejected(parse(&event));
        }
    }

    #[test]
    fn tampering_and_wrong_credentials_produce_only_generic_errors() {
        let body = encrypted(&message());
        let headers = sign(&body, NOW);
        rejected(parse_event(
            &headers,
            &body,
            INSTALLATION,
            "wrong-encrypt-key",
            TOKEN,
            NOW,
        ));
        rejected(parse_event(
            &headers,
            &body,
            INSTALLATION,
            KEY,
            "wrong-token",
            NOW,
        ));
        for (key, token) in [("", TOKEN), (KEY, "")] {
            rejected(parse_event(&headers, &body, INSTALLATION, key, token, NOW));
        }
        let mut envelope: Value = serde_json::from_slice(&body).unwrap();
        let mut cipher = STANDARD
            .decode(envelope["encrypt"].as_str().unwrap())
            .unwrap();
        cipher[16] ^= 1;
        envelope["encrypt"] = json!(STANDARD.encode(cipher));
        rejected(parse_event(
            &headers,
            &serde_json::to_vec(&envelope).unwrap(),
            INSTALLATION,
            KEY,
            TOKEN,
            NOW,
        ));
        let body = encrypted(&json!({"encrypt":"nested"}));
        rejected(parse_event(
            &sign(&body, NOW),
            &body,
            INSTALLATION,
            KEY,
            TOKEN,
            NOW,
        ));
        let mut mixed: Value = serde_json::from_slice(&encrypted(&message())).unwrap();
        mixed["type"] = json!("url_verification");
        rejected(parse(&mixed));
    }

    #[test]
    fn raw_decrypted_and_prompt_bounds_are_enforced_independently() {
        let mut event = message();
        for text in [
            "a".repeat(MAX_PROMPT_BYTES),
            "界".repeat(MAX_PROMPT_BYTES / 3),
        ] {
            event["event"]["message"]["content"] = json!(json!({"text":text}).to_string());
            assert!(parse(&event).is_ok());
        }
        event["event"]["message"]["content"] =
            json!(json!({"text":"a".repeat(MAX_PROMPT_BYTES + 1)}).to_string());
        rejected(parse(&event));
        let body = vec![b' '; MAX_BODY_BYTES + 1];
        rejected(parse_event(
            &sign(&body, NOW),
            &body,
            INSTALLATION,
            KEY,
            TOKEN,
            NOW,
        ));
        let mut event = message();
        event["metadata"] = json!("x".repeat(MAX_PLAINTEXT_BYTES));
        let body = encrypted(&event);
        assert!(body.len() < MAX_BODY_BYTES);
        rejected(parse_event(
            &sign(&body, NOW),
            &body,
            INSTALLATION,
            KEY,
            TOKEN,
            NOW,
        ));
        assert_eq!(
            decrypt(&encrypt(&vec![b'a'; MAX_PLAINTEXT_BYTES]), KEY)
                .unwrap()
                .len(),
            MAX_PLAINTEXT_BYTES
        );
        assert!(decrypt(&encrypt(&vec![b'a'; MAX_PLAINTEXT_BYTES + 1]), KEY).is_err());
        assert!(decrypt(&"A".repeat(MAX_ENCODED_BYTES + 4), KEY).is_err());
    }
    #[test]
    fn critical_raw_duplicate_fields_are_rejected_in_plain_and_encrypted_events() {
        let valid = message().to_string();
        for (old, replacement) in [
            (
                "\"schema\":\"2.0\"",
                "\"schema\":\"2.0\",\"schema\":\"2.0\"",
            ),
            (
                "\"app_id\":\"cli_app123\"",
                "\"app_id\":\"cli_other\",\"app_id\":\"cli_app123\"",
            ),
            (
                "\"tenant_key\":\"tenant_123\"",
                "\"tenant_key\":\"other_tenant\",\"tenant_key\":\"tenant_123\"",
            ),
            (
                "\"event_type\":\"im.message.receive_v1\"",
                "\"event_type\":\"im.message.receive_v1\",\"event_type\":\"im.message.receive_v1\"",
            ),
            ("\"sender\":{", "\"sender\":{},\"sender\":{"),
            ("\"message\":{", "\"message\":{},\"message\":{"),
            ("\"sender_id\":{", "\"sender_id\":{},\"sender_id\":{"),
            (
                "\"open_id\":\"ou_person1\"",
                "\"open_id\":\"ou_other\",\"open_id\":\"ou_person1\"",
            ),
            (
                "\"chat_id\":\"oc_chat1\"",
                "\"chat_id\":\"oc_other\",\"chat_id\":\"oc_chat1\"",
            ),
            (
                "\"message_id\":\"om_message1\"",
                "\"message_id\":\"om_other\",\"message_id\":\"om_message1\"",
            ),
            (
                "\"sender_type\":\"user\"",
                "\"sender_type\":\"user\",\"sender_type\":\"user\"",
            ),
        ] {
            assert!(valid.contains(old), "fixture missing {old}");
            let plain = valid.replace(old, replacement).into_bytes();
            let encrypted = serde_json::to_vec(&json!({"encrypt":encrypt(&plain)})).unwrap();
            for body in [plain, encrypted] {
                rejected(parse_event(
                    &sign(&body, NOW),
                    &body,
                    INSTALLATION,
                    KEY,
                    TOKEN,
                    NOW,
                ));
            }
        }
        let mut duplicate_text = message();
        duplicate_text["event"]["message"]["content"] =
            json!("{\"text\":\"attacker\",\"text\":\"expected\"}");
        let plain = duplicate_text.to_string().into_bytes();
        for body in [
            plain.clone(),
            serde_json::to_vec(&json!({"encrypt":encrypt(&plain)})).unwrap(),
        ] {
            rejected(parse_event(
                &sign(&body, NOW),
                &body,
                INSTALLATION,
                KEY,
                TOKEN,
                NOW,
            ));
        }
        let encrypted = encrypt(valid.as_bytes());
        let raw = format!("{{\"encrypt\":{0},\"encrypt\":{0}}}", json!(encrypted));
        rejected(parse_event(
            &sign(raw.as_bytes(), NOW),
            raw.as_bytes(),
            INSTALLATION,
            KEY,
            TOKEN,
            NOW,
        ));
    }

    #[test]
    fn positional_arrays_are_rejected_at_each_authenticated_object_boundary() {
        for (pointer, array) in [
            (
                "/header",
                json!([
                    "event",
                    "im.message.receive_v1",
                    "cli_app123",
                    "tenant_123",
                    TOKEN
                ]),
            ),
            (
                "/event",
                json!([
                    message()["event"]["sender"].clone(),
                    message()["event"]["message"].clone()
                ]),
            ),
            (
                "/event/sender",
                json!(["user", {"open_id":"ou_person1"}, "tenant_123"]),
            ),
            ("/event/sender/sender_id", json!(["ou_person1"])),
            (
                "/event/message",
                json!([
                    "om_message1",
                    "oc_chat1",
                    "p2p",
                    "text",
                    null,
                    "{\"text\":\"hello\"}"
                ]),
            ),
            ("/event/message/content", json!("[\"hello\"]")),
        ] {
            let mut value = message();
            *value.pointer_mut(pointer).unwrap() = array;
            let plain = value.to_string().into_bytes();
            for body in [
                plain.clone(),
                serde_json::to_vec(&json!({"encrypt":encrypt(&plain)})).unwrap(),
            ] {
                rejected(parse_event(
                    &sign(&body, NOW),
                    &body,
                    INSTALLATION,
                    KEY,
                    TOKEN,
                    NOW,
                ));
            }
        }
        let raw = json!([
            null,
            "url_verification",
            TOKEN,
            "challenge",
            null,
            null,
            null
        ])
        .to_string();
        for body in [
            raw.as_bytes().to_vec(),
            serde_json::to_vec(&json!({"encrypt":encrypt(raw.as_bytes())})).unwrap(),
        ] {
            rejected(parse_event(
                &HeaderMap::new(),
                &body,
                INSTALLATION,
                KEY,
                TOKEN,
                NOW,
            ));
        }
    }

    #[test]
    fn challenge_duplicate_secret_or_challenge_cannot_use_last_field_wins() {
        for raw in [
            format!("{{\"type\":\"url_verification\",\"token\":\"wrong\",\"token\":{0},\"challenge\":\"verify\"}}", json!(TOKEN)),
            format!("{{\"type\":\"url_verification\",\"token\":{0},\"challenge\":\"verify\",\"challenge\":\"verify\"}}", json!(TOKEN)),
        ] {
            for body in [raw.as_bytes().to_vec(), serde_json::to_vec(&json!({"encrypt":encrypt(raw.as_bytes())})).unwrap()] {
                rejected(parse_event(&HeaderMap::new(), &body, INSTALLATION, KEY, TOKEN, NOW));
            }
        }
    }
}
