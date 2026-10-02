// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Random gateway credentials; only domain-separated verifiers are persisted.
use anyhow::{anyhow, Result};
use axum::http::HeaderMap;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const TOKEN_BYTES: usize = 84;
const DOMAIN: &[u8] = b"JiaClaw/gateway/api-key/v1\0";

/// A newly issued credential. Deliberately has no Debug or Serialize implementation.
pub struct IssuedKey {
    pub user_id: Uuid,
    pub key_id: Uuid,
    pub token: String,
}

pub(super) struct ParsedKey {
    pub key_id: Uuid,
    secret: [u8; 32],
}

impl ParsedKey {
    pub(super) fn verifier(&self, user_id: Uuid) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(DOMAIN);
        digest.update(user_id.as_bytes());
        digest.update(self.key_id.as_bytes());
        digest.update(self.secret);
        digest.finalize().into()
    }
}

pub(super) fn issue(user_id: Uuid) -> Result<(IssuedKey, [u8; 32])> {
    let mut secret = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut secret)
        .map_err(|_| anyhow!("gateway key generation failed"))?;
    let key_id = Uuid::new_v4();
    let parsed = ParsedKey { key_id, secret };
    let verifier = parsed.verifier(user_id);
    Ok((
        IssuedKey {
            user_id,
            key_id,
            token: format!("jc1.{key_id}.{}", URL_SAFE_NO_PAD.encode(secret)),
        },
        verifier,
    ))
}

pub(super) fn parse(token: &str) -> Option<ParsedKey> {
    if token.len() != TOKEN_BYTES || !token.is_ascii() {
        return None;
    }
    let (id, encoded) = token.strip_prefix("jc1.")?.split_once('.')?;
    let key_id = Uuid::parse_str(id).ok()?;
    if id != key_id.hyphenated().to_string() || encoded.len() != 43 {
        return None;
    }
    let mut secret = [0_u8; 32];
    if URL_SAFE_NO_PAD.decode_slice(encoded, &mut secret).ok()? != secret.len() {
        return None;
    }
    Some(ParsedKey { key_id, secret })
}

pub(super) fn matches(candidate: &[u8; 32], stored: &[u8]) -> bool {
    stored.len() == 32 && bool::from(candidate.as_slice().ct_eq(stored))
}

/// Accept only one canonical Bearer credential, without alternate authentication headers.
pub fn authorization_token(headers: &HeaderMap) -> Option<&str> {
    if headers.contains_key("x-api-token") {
        return None;
    }
    let mut authorization = headers.get_all("authorization").iter();
    let header = authorization.next()?;
    if authorization.next().is_some() {
        return None;
    }
    let token = header.to_str().ok()?.strip_prefix("Bearer ")?;
    parse(token)?;
    Some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn credentials_are_canonical_random_and_bound_to_both_identities() {
        let user_id = Uuid::new_v4();
        let (first, verifier) = issue(user_id).unwrap();
        let (second, _) = issue(user_id).unwrap();
        assert_ne!(first.token, second.token);
        assert_eq!(first.token.len(), TOKEN_BYTES);
        let parsed = parse(&first.token).unwrap();
        assert!(matches(&parsed.verifier(user_id), &verifier));
        assert!(!matches(&parsed.verifier(Uuid::new_v4()), &verifier));
        let replaced = first
            .token
            .replace(&first.key_id.to_string(), &second.key_id.to_string());
        assert!(!matches(
            &parse(&replaced).unwrap().verifier(user_id),
            &verifier
        ));
        for invalid in [
            format!(" {}", first.token),
            format!("{}=", first.token),
            first.token.replace("jc1.", "jc2."),
            first.token.replace(
                &first.key_id.to_string(),
                "ABCDEFAB-1234-4234-9234-123456789ABC",
            ),
            first.token.replace('.', ":"),
        ] {
            assert!(parse(&invalid).is_none());
        }
        assert!(!matches(&verifier, &verifier[..31]));
    }

    #[test]
    fn authentication_headers_reject_duplicates_and_ambiguity() {
        let (issued, _) = issue(Uuid::new_v4()).unwrap();
        let mut headers = HeaderMap::new();
        assert!(authorization_token(&headers).is_none());
        let bearer = HeaderValue::from_str(&format!("Bearer {}", issued.token)).unwrap();
        headers.insert("authorization", bearer.clone());
        assert_eq!(authorization_token(&headers), Some(issued.token.as_str()));
        headers.append("authorization", bearer.clone());
        assert!(authorization_token(&headers).is_none());
        headers.insert("authorization", bearer);
        headers.insert("x-api-token", HeaderValue::from_static("ignored"));
        assert!(authorization_token(&headers).is_none());
        headers.remove("x-api-token");
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer malformed"),
        );
        assert!(authorization_token(&headers).is_none());
    }
}
