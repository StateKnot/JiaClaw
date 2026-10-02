// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Enterprise self-built application callbacks, not group webhooks or AI bots.
//! Protocol: https://developer.work.weixin.qq.com/document/path/90238 (callbacks),
//! /90239 (decrypted message fields), /90968 (cryptography), /90195 (member IDs).

use aes::cipher::{
    block_padding::{NoPadding, Pkcs7, RawPadding},
    BlockDecryptMut, KeyIvInit,
};
use anyhow::{anyhow, ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use std::collections::HashMap;
use subtle::ConstantTimeEq;

const MAX_BODY: usize = 128 * 1024;
const MAX_XML: usize = 64 * 1024;
const MAX_CONTENT: usize = 32 * 1024;
const MAX_CIPHER: usize = ((16 + 4 + MAX_XML + 64) / 32 + 1) * 32;
const MAX_ENCODED: usize = MAX_CIPHER.div_ceil(3) * 4;
const MAX_QUERY: usize = 8192;
const INVALID: &str = "invalid WeCom callback";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Inbound {
    Ignored,
    Message {
        event_id: String,
        sender_id: String,
        text: String,
    },
}

pub(super) fn validate_installation(value: &str) -> Result<(&str, u32)> {
    let (corp, agent) = value
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid WeCom installation"))?;
    ensure!(
        value.len() <= 128
            && !corp.is_empty()
            && corp.len() <= 64
            && corp
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
        "invalid WeCom installation"
    );
    let agent = agent_id(agent).ok_or_else(|| anyhow!("invalid WeCom installation"))?;
    Ok((corp, agent))
}

/// Member IDs are case-insensitively unique on WeCom. Policies, destinations and
/// session identities use this canonical lowercase spelling; broadcasts cannot
/// pass the single-member grammar (official member/create, document 90195).
pub(super) fn user_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.bytes().all(|b| {
            b.is_ascii_digit() || b.is_ascii_lowercase() || matches!(b, b'_' | b'-' | b'.' | b'@')
        })
        && value != "@all"
}

fn agent_id(value: &str) -> Option<u32> {
    if value.is_empty()
        || value.len() > 10
        || value.starts_with('0')
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    value.parse::<u32>().ok().filter(|n| *n <= i32::MAX as u32)
}

// Credentials are intentionally neither Debug nor Serialize. Validate them once
// at startup; callback handling is bounded CPU work and performs no I/O.
pub(super) struct Callback {
    corp_id: String,
    agent_id: u32,
    token: String,
    aes_key: [u8; 32],
}

impl Callback {
    pub(super) fn new(installation: &str, token: String, encoding_aes_key: &str) -> Result<Self> {
        let result = (|| {
            let (corp, agent) = validate_installation(installation)?;
            ensure!(
                !token.is_empty()
                    && token.len() <= 32
                    && token.bytes().all(|b| b.is_ascii_alphanumeric()),
                "token"
            );
            ensure!(
                encoding_aes_key.len() == 43
                    && encoding_aes_key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/')),
                "key"
            );
            // EncodingAESKey is a 43-character platform secret, and the official
            // SDK sample has nonzero unused bits in its final Base64 character.
            // Match that key derivation without relaxing ciphertext decoding.
            let key_decoder = base64::engine::GeneralPurpose::new(
                &base64::alphabet::STANDARD,
                base64::engine::GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
            );
            let key = key_decoder.decode(format!("{encoding_aes_key}="))?;
            let aes_key: [u8; 32] = key.try_into().map_err(|_| anyhow!("key"))?;
            Ok(Self {
                corp_id: corp.into(),
                agent_id: agent,
                token,
                aes_key,
            })
        })();
        result.map_err(|_: anyhow::Error| anyhow!("invalid WeCom configuration"))
    }

    pub(super) fn challenge(&self, query: &str, now_secs: u64) -> Result<String> {
        let result = (|| {
            let query = Query::parse(query, true)?;
            let encrypted = query.echo.as_deref().ok_or_else(|| anyhow!("echo"))?;
            ensure!(encrypted.len() <= 2048, "echo size");
            self.verify(&query, encrypted, now_secs)?;
            let clear = self.decrypt(encrypted)?;
            ensure!(!clear.is_empty() && clear.len() <= 1024, "echo size");
            Ok(String::from_utf8(clear)?)
        })();
        result.map_err(|_: anyhow::Error| anyhow!(INVALID))
    }

    pub(super) fn parse_event(&self, query: &str, body: &[u8], now_secs: u64) -> Result<Inbound> {
        self.parse_inner(query, body, now_secs)
            .map_err(|_| anyhow!(INVALID))
    }

    fn parse_inner(&self, query: &str, body: &[u8], now_secs: u64) -> Result<Inbound> {
        ensure!(!body.is_empty() && body.len() <= MAX_BODY, "body size");
        let query = Query::parse(query, false)?;
        let outer = flat_xml(body)?;
        let encrypted = required(&outer, "Encrypt")?;
        self.verify(&query, encrypted, now_secs)?;
        let clear = self.decrypt(encrypted)?;
        let inner = flat_xml(&clear)?;
        // The outer fields are not included in the signature. Match them as
        // redundant routing hints, and independently require the authenticated
        // fields from inside Encrypt. Never substitute outer AgentID for inner.
        ensure!(
            required(&outer, "ToUserName")? == self.corp_id
                && agent_id(required(&outer, "AgentID")?) == Some(self.agent_id)
                && required(&inner, "ToUserName")? == self.corp_id
                && agent_id(required(&inner, "AgentID")?) == Some(self.agent_id),
            "identity"
        );
        if required(&inner, "MsgType")? != "text" {
            return Ok(Inbound::Ignored);
        }
        let sender = required(&inner, "FromUserName")?;
        ensure!(sender.is_ascii(), "sender");
        let sender_id = sender.to_ascii_lowercase();
        ensure!(user_id(&sender_id), "sender");
        let message_id = required(&inner, "MsgId")?;
        ensure!(
            !message_id.is_empty()
                && message_id.len() <= 20
                && message_id.bytes().all(|b| b.is_ascii_digit())
                && !message_id.starts_with('0'),
            "message id"
        );
        let message_id = message_id.parse::<u64>()?;
        let created = required(&inner, "CreateTime")?;
        ensure!(
            !created.is_empty()
                && created.len() <= 20
                && created.bytes().all(|b| b.is_ascii_digit()),
            "creation time"
        );
        let _ = created.parse::<u64>()?;
        let text = required(&inner, "Content")?;
        ensure!(!text.trim().is_empty() && text.len() <= MAX_CONTENT, "text");
        Ok(Inbound::Message {
            event_id: message_id.to_string(),
            sender_id,
            text: text.into(),
        })
    }

    fn verify(&self, query: &Query, encrypted: &str, now_secs: u64) -> Result<()> {
        ensure!(
            !encrypted.is_empty()
                && encrypted.len() <= MAX_ENCODED
                && !query.timestamp.is_empty()
                && query.timestamp.len() <= 20
                && query.timestamp.bytes().all(|b| b.is_ascii_digit())
                && now_secs.abs_diff(query.timestamp.parse::<u64>()?) <= 300
                && !query.nonce.is_empty()
                && query.nonce.len() <= 256
                && query.nonce.bytes().all(|b| b.is_ascii_graphic())
                && query.signature.len() == 40,
            "signature metadata"
        );
        let mut supplied = [0u8; 20];
        for (pair, output) in query
            .signature
            .as_bytes()
            .chunks_exact(2)
            .zip(&mut supplied)
        {
            let high = hex(pair[0]).ok_or_else(|| anyhow!("signature"))?;
            let low = hex(pair[1]).ok_or_else(|| anyhow!("signature"))?;
            *output = (high << 4) | low;
        }
        // SHA-1 is mandated by this legacy wire protocol, not used for new
        // integrity schemes. Sorting is lexicographic over the decoded values.
        let mut pieces = [
            self.token.as_str(),
            query.timestamp.as_str(),
            query.nonce.as_str(),
            encrypted,
        ];
        pieces.sort_unstable();
        let mut context = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
        for piece in pieces {
            context.update(piece.as_bytes());
        }
        ensure!(
            bool::from(context.finish().as_ref().ct_eq(&supplied)),
            "signature"
        );
        Ok(())
    }

    fn decrypt(&self, encrypted: &str) -> Result<Vec<u8>> {
        ensure!(
            !encrypted.is_empty() && encrypted.len() <= MAX_ENCODED,
            "cipher size"
        );
        let mut cipher = STANDARD.decode(encrypted)?;
        // WeCom uses PKCS#7 with a 32-byte padding boundary, unlike the AES
        // primitive's 16-byte block size. Unpad the final 32-byte block strictly.
        ensure!(
            !cipher.is_empty() && cipher.len() <= MAX_CIPHER && cipher.len() % 32 == 0,
            "cipher size"
        );
        let clear =
            cbc::Decryptor::<aes::Aes256>::new_from_slices(&self.aes_key, &self.aes_key[..16])
                .map_err(|_| anyhow!("cipher key"))?
                .decrypt_padded_mut::<NoPadding>(&mut cipher)
                .map_err(|_| anyhow!("cipher blocks"))?;
        let last = clear.len() - 32;
        let unpadded = Pkcs7::raw_unpad(&clear[last..]).map_err(|_| anyhow!("cipher padding"))?;
        let clear = &clear[..last + unpadded.len()];
        ensure!(clear.len() >= 20, "frame size");
        let length = usize::try_from(u32::from_be_bytes(clear[16..20].try_into()?))?;
        let end = 20usize
            .checked_add(length)
            .ok_or_else(|| anyhow!("frame length"))?;
        ensure!(
            length <= MAX_XML && end <= clear.len() && &clear[end..] == self.corp_id.as_bytes(),
            "receive id"
        );
        Ok(clear[20..end].to_vec())
    }
}

struct Query {
    signature: String,
    timestamp: String,
    nonce: String,
    echo: Option<String>,
}
impl Query {
    fn parse(query: &str, challenge: bool) -> Result<Self> {
        ensure!(!query.is_empty() && query.len() <= MAX_QUERY, "query size");
        let mut values = HashMap::new();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').ok_or_else(|| anyhow!("query pair"))?;
            let key = url_decode(key)?;
            ensure!(
                matches!(key.as_str(), "msg_signature" | "timestamp" | "nonce")
                    || (challenge && key == "echostr"),
                "query key"
            );
            ensure!(
                values.insert(key, url_decode(value)?).is_none(),
                "duplicate query"
            );
        }
        let mut get = |name| values.remove(name).ok_or_else(|| anyhow!("missing query"));
        let result = Self {
            signature: get("msg_signature")?,
            timestamp: get("timestamp")?,
            nonce: get("nonce")?,
            echo: if challenge {
                Some(get("echostr")?)
            } else {
                None
            },
        };
        Ok(result)
    }
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
fn url_decode(value: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.bytes();
    while let Some(byte) = input.next() {
        bytes.push(match byte {
            b'%' => {
                let high = input
                    .next()
                    .and_then(hex)
                    .ok_or_else(|| anyhow!("url encoding"))?;
                let low = input
                    .next()
                    .and_then(hex)
                    .ok_or_else(|| anyhow!("url encoding"))?;
                (high << 4) | low
            }
            b'+' => b' ',
            value => value,
        });
    }
    Ok(String::from_utf8(bytes)?)
}

fn required<'a>(values: &'a HashMap<String, String>, name: &str) -> Result<&'a str> {
    values
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("missing field"))
}
fn legal_xml_char(value: char) -> bool {
    matches!(value, '\t' | '\r' | '\n' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}
fn append(field: &mut Option<(String, String)>, value: &str) -> Result<()> {
    ensure!(value.chars().all(legal_xml_char), "XML character");
    if let Some((_, text)) = field {
        ensure!(
            text.len().saturating_add(value.len()) <= MAX_ENCODED,
            "XML field size"
        );
        text.push_str(value);
    } else {
        ensure!(
            value.trim_matches([' ', '\t', '\r', '\n']).is_empty(),
            "XML mixed text"
        );
    }
    Ok(())
}
fn element_name(element: &BytesStart<'_>) -> Result<String> {
    ensure!(element.attributes().next().is_none(), "XML attributes");
    let name = std::str::from_utf8(element.name().as_ref())?.to_string();
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && name.as_bytes()[0].is_ascii_alphabetic(),
        "XML name"
    );
    Ok(name)
}

/// Streaming, UTF-8-only, flat XML reader. No DTD resolver or entity expansion
/// exists here; only XML's five predefined entities and numeric references are
/// admitted. Every element, including an unknown extension field, is unique.
fn flat_xml(bytes: &[u8]) -> Result<HashMap<String, String>> {
    let source = std::str::from_utf8(bytes)?;
    let mut reader = Reader::from_str(source);
    reader.config_mut().check_comments = true;
    let mut values = HashMap::new();
    let mut field: Option<(String, String)> = None;
    let mut started = false;
    let mut ended = false;
    let mut declaration = false;
    let mut events = 0usize;
    loop {
        events += 1;
        ensure!(events <= MAX_BODY, "XML event count");
        match reader.read_event()? {
            Event::Start(element) => {
                let name = element_name(&element)?;
                if !started {
                    ensure!(name == "xml" && !ended, "XML root");
                    started = true;
                } else {
                    ensure!(
                        !ended
                            && field.is_none()
                            && values.len() < 64
                            && !values.contains_key(&name),
                        "XML nesting or duplicate"
                    );
                    field = Some((name, String::new()));
                }
            }
            Event::Empty(element) => {
                let name = element_name(&element)?;
                ensure!(
                    started
                        && !ended
                        && field.is_none()
                        && values.len() < 64
                        && !values.contains_key(&name),
                    "XML empty field"
                );
                values.insert(name, String::new());
            }
            Event::End(_) => {
                if let Some((name, value)) = field.take() {
                    values.insert(name, value);
                } else {
                    ensure!(started && !ended, "XML root end");
                    ended = true;
                }
            }
            Event::Text(text) => append(&mut field, &text.xml10_content()?)?,
            Event::CData(text) => {
                ensure!(field.is_some(), "XML CDATA position");
                append(&mut field, &text.xml10_content()?)?;
            }
            Event::GeneralRef(reference) => {
                ensure!(field.is_some(), "XML reference position");
                if let Some(value) = reference.resolve_char_ref()? {
                    let mut buf = [0; 4];
                    append(&mut field, value.encode_utf8(&mut buf))?;
                } else {
                    let reference = reference.decode()?;
                    let value = match reference.as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => return Err(anyhow!("XML entity")),
                    };
                    append(&mut field, value)?;
                }
            }
            Event::Decl(decl) => {
                ensure!(
                    !started && !declaration && decl.version()?.as_ref() == b"1.0",
                    "XML declaration"
                );
                let contents = std::str::from_utf8(decl.as_ref())?;
                let attributes = BytesStart::from_content(contents, 3);
                for attribute in attributes.attributes() {
                    let attribute = attribute?;
                    let valid = match attribute.key.as_ref() {
                        b"version" => attribute.value.as_ref() == b"1.0",
                        b"encoding" => attribute.value.eq_ignore_ascii_case(b"UTF-8"),
                        b"standalone" => matches!(attribute.value.as_ref(), b"yes" | b"no"),
                        _ => false,
                    };
                    ensure!(valid, "XML declaration attribute");
                }
                declaration = true;
            }
            Event::Eof => {
                ensure!(started && ended && field.is_none(), "XML incomplete");
                return Ok(values);
            }
            Event::DocType(_) | Event::PI(_) | Event::Comment(_) => {
                return Err(anyhow!("XML unsupported construct"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncryptMut;

    // Independent Tencent Java SDK samples, not generated by our implementation.
    // https://open.work.weixin.qq.com/wwopen/downloadfile/java.zip
    // SHA256 4a1644d08db8a2b489e79281925dedb2a4788a082fda7d311b17ee942f03192f
    const KEY: &str = "jWmYm7qr5nMoAUwZRjGtBxmz3KA1tkAj3ykkR6q2B2C";
    const CORP: &str = "wx5823bf96d3bd56c7";
    const INSTALLATION: &str = "wx5823bf96d3bd56c7:218";
    const TOKEN: &str = "QDG6eK";
    const NOW: u64 = 1_409_659_813;
    const OFFICIAL_ECHO: &str =
        "P9nAzCzyDtyTWESHep1vC5X9xho/qYX3Zpb4yKa9SKld1DsH3Iyt3tP3zNdtp+4RPcs8TgAE7OaBO+FZXvnaqQ==";
    const OFFICIAL_POST: &str = "RypEvHKD8QQKFhvQ6QleEB4J58tiPdvo+rtK1I9qca6aM/wvqnLSV5zEPeusUiX5L5X/0lWfrf0QADHHhGd3QczcdCUpj911L3vg3W/sYYvuJTs3TUUkSUXxaccAS0qhxchrRYt66wiSpGLYL42aM6A8dTT+6k4aSknmPj48kzJs8qLjvd4Xgpue06DOdnLxAUHzM6+kDZ+HMZfJYuR+LtwGc2hgf5gsijff0ekUNXZiqATP7PF5mZxZ3Izoun1s4zG4LUMnvw2r+KqCKIw+3IQH03v+BCA9nMELNqbSf6tiWSrXJB3LAVGUcallcrw8V2t9EL4EhzJWrQUax5wLVMNS0+rUPA3k22Ncx4XXZS9o0MBH27Bo6BpNelZpS+/uh9KsNlY6bHCmJU9p8g7m3fVKn28H3KDYA5Pl/T8Z1ptDAVe0lXdQ2YoyyH2uyPIGHBZZIs2pDBS8R07+qN+E7Q==";

    fn callback() -> Callback {
        Callback::new(INSTALLATION, TOKEN.into(), KEY).unwrap()
    }
    fn message() -> String {
        format!("<xml><ToUserName>{CORP}</ToUserName><FromUserName>Member.One@Example</FromUserName><CreateTime>{NOW}</CreateTime><MsgType>text</MsgType><Content><![CDATA[你好 & hello]]></Content><MsgId>4561255354251345929</MsgId><AgentID>218</AgentID></xml>")
    }
    fn outer(encrypted: &str) -> String {
        format!("<xml><ToUserName>{CORP}</ToUserName><AgentID>218</AgentID><Encrypt><![CDATA[{encrypted}]]></Encrypt></xml>")
    }
    fn encoded(value: &str) -> String {
        value
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() {
                    char::from(byte).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect()
    }
    fn query(encrypted: &str, now: u64, echo: bool) -> String {
        let timestamp = now.to_string();
        let nonce = "12345";
        let mut pieces = [TOKEN, timestamp.as_str(), nonce, encrypted];
        pieces.sort_unstable();
        let digest = ring::digest::digest(
            &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
            pieces.join("").as_bytes(),
        );
        let sig: String = digest
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let mut query = format!("msg_signature={sig}&timestamp={timestamp}&nonce={nonce}");
        if echo {
            query.push_str(&format!("&echostr={}", encoded(encrypted)));
        }
        query
    }
    fn framed(plain: &[u8], corp: &str) -> Vec<u8> {
        let mut frame = vec![b'R'; 16];
        frame.extend_from_slice(&u32::try_from(plain.len()).unwrap().to_be_bytes());
        frame.extend_from_slice(plain);
        frame.extend_from_slice(corp.as_bytes());
        let pad = 32 - frame.len() % 32;
        frame.extend(std::iter::repeat_n(u8::try_from(pad).unwrap(), pad));
        frame
    }
    fn encrypt_frame(mut frame: Vec<u8>) -> String {
        let cb = callback();
        let length = frame.len();
        let encrypted =
            cbc::Encryptor::<aes::Aes256>::new_from_slices(&cb.aes_key, &cb.aes_key[..16])
                .unwrap()
                .encrypt_padded_mut::<NoPadding>(&mut frame, length)
                .unwrap();
        STANDARD.encode(encrypted)
    }
    fn encrypted(plain: &str) -> String {
        encrypt_frame(framed(plain.as_bytes(), CORP))
    }
    fn parse(plain: &str) -> Result<Inbound> {
        let encrypted = encrypted(plain);
        callback().parse_event(
            &query(&encrypted, NOW, false),
            outer(&encrypted).as_bytes(),
            NOW,
        )
    }
    fn rejected<T>(result: Result<T>) {
        let err = match result {
            Ok(_) => panic!("expected rejection"),
            Err(err) => err,
        };
        assert_eq!(err.to_string(), INVALID);
        assert_eq!(format!("{err:#}"), INVALID);
    }

    #[test]
    fn identity_and_secret_configuration_are_strict_and_canonical() {
        assert_eq!(validate_installation(INSTALLATION).unwrap(), (CORP, 218));
        assert_eq!(
            validate_installation("wwabc:2147483647").unwrap().1,
            i32::MAX as u32
        );
        for value in [
            "",
            "wwabc",
            ":1",
            "wwabc:",
            "wwabc:0",
            "wwabc:01",
            "wwabc:+1",
            "wwabc:2147483648",
            "wwabc:1:2",
            "ww/a:1",
            "企业:1",
        ] {
            assert!(validate_installation(value).is_err(), "{value}");
        }
        assert!(validate_installation(&format!("{}:1", "x".repeat(65))).is_err());
        for value in ["alice", "alice.smith@example", "1_a-b", "a"] {
            assert!(user_id(value));
        }
        for value in [
            "",
            "Alice",
            "@all",
            "@alice",
            "_alice",
            "-alice",
            "alice|bob",
            "alice/bob",
            "用户",
        ] {
            assert!(!user_id(value), "{value}");
        }
        assert!(user_id(&"x".repeat(64)));
        assert!(!user_id(&"x".repeat(65)));
        for token in ["", "a-b", "has space", &"x".repeat(33)] {
            let err = match Callback::new(INSTALLATION, token.into(), KEY) {
                Ok(_) => panic!("accepted token"),
                Err(err) => err,
            };
            assert_eq!(err.to_string(), "invalid WeCom configuration");
        }
        for key in ["", "abc", &format!("{KEY}="), &"!".repeat(43)] {
            assert!(Callback::new(INSTALLATION, TOKEN.into(), key).is_err());
        }
    }

    #[test]
    fn official_get_and_post_samples_match_independent_known_plaintext() {
        let query=format!("msg_signature=5c45ff5e21c57e6ad56bac8758b79b1d9ac89fd3&timestamp=1409659589&nonce=263014780&echostr={}",encoded(OFFICIAL_ECHO));
        assert_eq!(
            callback().challenge(&query, 1_409_659_589).unwrap(),
            "1616140317555161061"
        );
        let query="msg_signature=477715d11cdb4164915debcba66cb864d751f3e6&timestamp=1409659813&nonce=1372623149";
        assert_eq!(
            callback()
                .parse_event(query, outer(OFFICIAL_POST).as_bytes(), NOW)
                .unwrap(),
            Inbound::Message {
                event_id: "4561255354251345929".into(),
                sender_id: "mycreate".into(),
                text: "hello".into(),
            }
        );
    }

    #[test]
    fn member_case_is_normalized_without_changing_text_or_message_identity() {
        let expected = Inbound::Message {
            event_id: "4561255354251345929".into(),
            sender_id: "member.one@example".into(),
            text: "你好 & hello".into(),
        };
        assert_eq!(parse(&message()).unwrap(), expected);
        assert_eq!(
            parse(&message().replace("Member.One@Example", "MEMBER.ONE@EXAMPLE")).unwrap(),
            expected
        );
        for invalid in ["@all", "alice|bob", "_alice", "中文", "alice/bob"] {
            rejected(parse(&message().replace("Member.One@Example", invalid)));
        }
    }

    #[test]
    fn authenticated_company_and_agent_cannot_be_replaced_by_outer_hints() {
        for plain in [
            message().replace(CORP, "ww_different"),
            message().replace("<AgentID>218</AgentID>", "<AgentID>219</AgentID>"),
            message().replace("<AgentID>218</AgentID>", ""),
            message().replace("<AgentID>218</AgentID>", "<AgentID>0218</AgentID>"),
        ] {
            rejected(parse(&plain));
        }
        let cipher = encrypted(&message());
        for bad_outer in [
            outer(&cipher).replace(CORP, "ww_different"),
            outer(&cipher).replace("<AgentID>218</AgentID>", "<AgentID>219</AgentID>"),
        ] {
            rejected(callback().parse_event(
                &query(&cipher, NOW, false),
                bad_outer.as_bytes(),
                NOW,
            ));
        }
        let foreign = encrypt_frame(framed(message().as_bytes(), "ww_different"));
        rejected(callback().parse_event(
            &query(&foreign, NOW, false),
            outer(&foreign).as_bytes(),
            NOW,
        ));
        let other = Callback::new("ww_different:218", TOKEN.into(), KEY).unwrap();
        rejected(other.challenge(&query(OFFICIAL_ECHO, NOW, true), NOW));
    }

    #[test]
    fn ignored_events_still_require_signature_and_encrypted_identity() {
        for kind in ["image", "event", "voice", "unknown"] {
            let plain = message().replace(
                "<MsgType>text</MsgType>",
                &format!("<MsgType>{kind}</MsgType>"),
            );
            assert_eq!(parse(&plain).unwrap(), Inbound::Ignored);
            rejected(parse(
                &plain.replace("<AgentID>218</AgentID>", "<AgentID>219</AgentID>"),
            ));
            let cipher = encrypted(&plain);
            rejected(callback().parse_event(
                &query(&cipher, NOW, false).replace("msg_signature=", "msg_signature=0"),
                outer(&cipher).as_bytes(),
                NOW,
            ));
        }
    }

    #[test]
    fn signature_checks_freshness_duplicate_query_and_strict_url_decoding() {
        let cipher = encrypted(&message());
        let body = outer(&cipher);
        for time in [NOW - 300, NOW + 300] {
            assert!(callback()
                .parse_event(&query(&cipher, time, false), body.as_bytes(), NOW)
                .is_ok());
        }
        for time in [NOW - 301, NOW + 301, 0, u64::MAX] {
            rejected(callback().parse_event(&query(&cipher, time, false), body.as_bytes(), NOW));
        }
        let good = query(&cipher, NOW, false);
        for suffix in [
            "&nonce=1",
            "&%6eonce=12345",
            "&timestamp=1",
            "&msg_signature=1",
            "&unknown=1",
            "&",
            "&echostr=abc",
        ] {
            rejected(callback().parse_event(&(good.clone() + suffix), body.as_bytes(), NOW));
        }
        for invalid in [
            good.replace("nonce=12345", "nonce=%"),
            good.replace("nonce=12345", "nonce=%FF"),
            good.replace("nonce=12345", "nonce=%GG"),
            good.replace("nonce=12345", "nonce=%00"),
            good.replace("nonce=12345", "nonce=hello+world"),
            good.replace("timestamp=1409659813", "timestamp=+1409659813"),
            "x".repeat(MAX_QUERY + 1),
        ] {
            rejected(callback().parse_event(&invalid, body.as_bytes(), NOW));
        }
        let good = query(OFFICIAL_ECHO, NOW, true);
        assert!(callback().challenge(&good, NOW).is_ok());
        rejected(callback().challenge(&(good + "&echostr=duplicate"), NOW));
        // A decoded '+' must be encoded as %2B in URL form data, not a space.
        rejected(callback().challenge(&query(OFFICIAL_ECHO, NOW, true).replace("%2B", "+"), NOW));
    }

    #[test]
    fn every_pkcs7_padding_length_and_invalid_length_frame_are_checked() {
        for length in 0..32 {
            let text = "x".repeat(length);
            assert_eq!(
                callback().decrypt(&encrypted(&text)).unwrap(),
                text.as_bytes()
            );
        }
        let original = framed(b"hello", CORP);
        for replacement in [0, 33] {
            let mut bad = original.clone();
            *bad.last_mut().unwrap() = replacement;
            assert!(callback().decrypt(&encrypt_frame(bad)).is_err());
        }
        let mut bad = original.clone();
        let index = bad.len() - 2;
        bad[index] ^= 1;
        assert!(callback().decrypt(&encrypt_frame(bad)).is_err());
        let mut bad = original;
        bad[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(callback().decrypt(&encrypt_frame(bad)).is_err());
        for cipher in [
            "",
            "!bad",
            "AAAA",
            &STANDARD.encode([0; 16]),
            &STANDARD.encode([0; 33]),
        ] {
            assert!(callback().decrypt(cipher).is_err());
        }
        let raw = STANDARD.decode(OFFICIAL_POST).unwrap();
        let mut tampered = raw;
        tampered[16] ^= 1;
        let cipher = STANDARD.encode(tampered);
        rejected(callback().parse_event(
            &query(OFFICIAL_POST, NOW, false),
            outer(&cipher).as_bytes(),
            NOW,
        ));
    }

    #[test]
    fn xml_entities_are_literal_once_and_unsupported_constructs_are_rejected() {
        let plain = message().replace(
            "<![CDATA[你好 & hello]]>",
            "a&amp;b&lt;c&gt;&quot;&apos;&#65;&#x4e2d;",
        );
        let Inbound::Message { text, .. } = parse(&plain).unwrap() else {
            panic!("message");
        };
        assert_eq!(text, "a&b<c>\"'A中");
        let plain = message().replace("<![CDATA[你好 & hello]]>", "&amp;lt;<![CDATA[<b>]]>");
        let Inbound::Message { text, .. } = parse(&plain).unwrap() else {
            panic!("message");
        };
        assert_eq!(text, "&lt;<b>");
        assert!(
            parse(&("<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string() + &message())).is_ok()
        );
        for replacement in [
            "&unknown;",
            "&#0;",
            "&#x01;",
            "&#xD800;",
            "&#1114112;",
            "&bad",
            "<Nested>nested</Nested>",
            "<!--comment-->",
            "<?process x?>",
            "\u{0}",
        ] {
            rejected(parse(
                &message().replace("<![CDATA[你好 & hello]]>", replacement),
            ));
        }
        for plain in [
            "<!DOCTYPE xml [<!ENTITY x SYSTEM 'file:///etc/passwd'>]>".to_string() + &message(),
            "<!DOCTYPE xml [<!ENTITY x 'boom'><!ENTITY y '&x;&x;&x;'>]>".to_string() + &message(),
            "<?xml version=\"1.0\" encoding=\"UTF-16\"?>".to_string() + &message(),
            message().replace("<xml>", "<xml xmlns='urn:other'>"),
            message().replace("<Content>", "<Content a='b'>"),
            message().replace("<Content>", "<ns:Content>"),
            message().replace("</xml>", "<AgentID>218</AgentID></xml>"),
            message().replace("</xml>", "<Content>duplicate</Content></xml>"),
            message().replace("</Content>", "</MsgType>"),
            message() + "<xml></xml>",
            message().replace("</xml>", ""),
        ] {
            rejected(parse(&plain));
        }
        let cipher = encrypted(&message());
        for malicious_outer in [
            format!(
                "<!DOCTYPE xml SYSTEM 'https://invalid.example/dtd'>{}",
                outer(&cipher)
            ),
            outer(&cipher).replace("</xml>", "<Encrypt>second</Encrypt></xml>"),
        ] {
            rejected(callback().parse_event(
                &query(&cipher, NOW, false),
                malicious_outer.as_bytes(),
                NOW,
            ));
        }
    }

    #[test]
    fn text_identity_and_size_limits_apply_after_decoding() {
        for (from, to) in [
            ("4561255354251345929", "0"),
            ("4561255354251345929", "01"),
            ("4561255354251345929", "18446744073709551616"),
            ("4561255354251345929", "message-string"),
            (
                "<CreateTime>1409659813</CreateTime>",
                "<CreateTime>-1</CreateTime>",
            ),
            ("<![CDATA[你好 & hello]]>", " "),
        ] {
            rejected(parse(&message().replace(from, to)));
        }
        let largest = message().replace("你好 & hello", &"a".repeat(MAX_CONTENT));
        assert!(parse(&largest).is_ok());
        rejected(parse(
            &message().replace("你好 & hello", &"a".repeat(MAX_CONTENT + 1)),
        ));
        let largest = message().replace("你好 & hello", &"界".repeat(MAX_CONTENT / 3 + 1));
        rejected(parse(&largest));
        let cipher = encrypted(&message());
        rejected(callback().parse_event(
            &query(&cipher, NOW, false),
            &vec![b' '; MAX_BODY + 1],
            NOW,
        ));
        let huge = encrypted(&"x".repeat(MAX_XML + 1));
        assert!(callback().decrypt(&huge).is_err());
        assert!(callback().decrypt(&"A".repeat(MAX_ENCODED + 4)).is_err());
        let echo = encrypted(&"x".repeat(1025));
        rejected(callback().challenge(&query(&echo, NOW, true), NOW));
        let binary = encrypt_frame(framed(b"\xff", CORP));
        rejected(callback().challenge(&query(&binary, NOW, true), NOW));
        let empty = encrypted("");
        rejected(callback().challenge(&query(&empty, NOW, true), NOW));
        let many_fields = (0..65).map(|i| format!("<Extra{i}/>")).collect::<String>();
        rejected(parse(
            &message().replace("</xml>", &format!("{many_fields}</xml>")),
        ));
    }
}
