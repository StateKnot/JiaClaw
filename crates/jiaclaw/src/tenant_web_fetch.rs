// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Complete destination permission for the existing HTTP reader; no network I/O.
use jiaclaw_core::{JiaClawError, TenantWebFetchPolicy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Independent default-off permission protocol. Not a document or DNS identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantWebFetchCapability {
    /// Destination permission protocol, independent of SSE.
    pub protocol: u8,
    /// Complete sorted exact URL strings, including every permitted redirect hop.
    pub urls: Vec<String>,
    /// Explicit delegation of the administrator's private network override.
    pub allow_private: bool,
    /// Original application capture budget, including the overflow probe byte.
    pub max_capture_bytes: usize,
    /// Original retained raw HTTP body budget.
    pub max_body_bytes: usize,
    /// Original total HTTP deadline in seconds.
    pub timeout_seconds: usize,
    /// Original manual redirect budget.
    pub max_redirects: usize,
    /// Original readable body character limit.
    pub max_chars: usize,
    /// SHA256 of the complete ordered permission tuple.
    pub policy_sha256: String,
}
impl TenantWebFetchCapability {
    /// Validate and capture without DNS, URL fetching, or credential lookup.
    /// # Errors
    /// Invalid permission metadata or serialization failure.
    pub fn capture(policy: &TenantWebFetchPolicy) -> Result<Self, JiaClawError> {
        policy.validate()?;
        let mut cap = Self {
            protocol: 1,
            urls: policy.urls.clone(),
            allow_private: policy.allow_private,
            max_capture_bytes: crate::tools::WEB_FETCH_MAX_BODY_BYTES + 1,
            max_body_bytes: crate::tools::WEB_FETCH_MAX_BODY_BYTES,
            timeout_seconds: crate::tools::WEB_FETCH_HTTP_TIMEOUT_SECS as usize,
            max_redirects: crate::tools::WEB_FETCH_MAX_REDIRECTS,
            max_chars: crate::tools::WEB_FETCH_MAX_CHARS,
            policy_sha256: String::new(),
        };
        let input = serde_json::to_vec(&serde_json::json!([
            cap.protocol,
            cap.urls,
            cap.allow_private,
            cap.max_capture_bytes,
            cap.max_body_bytes,
            cap.timeout_seconds,
            cap.max_redirects,
            cap.max_chars
        ]))
        .map_err(|_| {
            JiaClawError::Configuration("tenant web_fetch permission encoding failed".into())
        })?;
        cap.policy_sha256 = format!("{:x}", Sha256::digest(input));
        Ok(cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tool, WebFetchTool};
    use serde_json::json;

    #[test]
    fn tenant_web_fetch_permission_is_exact_canonical_and_default_off() {
        assert!(jiaclaw_core::HttpConfig::default()
            .tenant_web_fetch
            .is_none());
        let urls = vec!["https://example.com/a?x=1".into()];
        let policy = TenantWebFetchPolicy {
            urls: urls.clone(),
            allow_private: false,
        };
        let cap = TenantWebFetchCapability::capture(&policy).unwrap();
        assert_eq!(
            cap.policy_sha256,
            format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&json!([1, urls, false, 1048577, 1048576, 15, 5, 50000]))
                        .unwrap()
                )
            )
        );
        for url in [
            "https://example.com",
            "https://EXAMPLE.com/a",
            "https://user@example.com/a",
            "https://example.com/a#fragment",
            " https://example.com/a",
            "https://example.com/界",
            "https://example.com:080/a",
            "https://example.com:+80/a",
            "https://example.com:65536/a",
            "https://example.com/a\n",
            "file:///private/x",
            "https://example.com\\evil/a",
        ] {
            assert!(
                TenantWebFetchCapability::capture(&TenantWebFetchPolicy {
                    urls: vec![url.into()],
                    allow_private: false
                })
                .is_err(),
                "{url}"
            );
        }
        for urls in [
            vec![],
            vec!["https://example.com/a".into(); 9],
            vec![
                "https://example.com/b".into(),
                "https://example.com/a".into(),
            ],
        ] {
            assert!(TenantWebFetchCapability::capture(&TenantWebFetchPolicy {
                urls,
                allow_private: false
            })
            .is_err());
        }
        assert!(WebFetchTool::restricted(
            &TenantWebFetchPolicy {
                urls: policy.urls,
                allow_private: true
            },
            false
        )
        .is_err());
    }

    #[tokio::test]
    async fn tenant_web_fetch_uses_original_reader_and_checks_every_redirect_before_request() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{
                atomic::{AtomicBool, Ordering},
                Arc, Mutex,
            },
            time::{Duration, Instant},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, done) = (seen.clone(), stop.clone());
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done.load(Ordering::SeqCst) && Instant::now() < deadline {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                let text = String::from_utf8(request).unwrap();
                let path = text.split_whitespace().nth(1).unwrap().to_owned();
                requests.lock().unwrap().push(path.clone());
                let response=match path.as_str() {
                    "/redirect"=>"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    "/blocked"=>"HTTP/1.1 302 Found\r\nLocation: /secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    _=>"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nPUBLIC!",
                };
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let mut urls = vec![
            format!("{base}/redirect"),
            format!("{base}/final"),
            format!("{base}/blocked"),
        ];
        urls.sort();
        let tool = WebFetchTool::restricted(
            &TenantWebFetchPolicy {
                urls,
                allow_private: true,
            },
            true,
        )
        .unwrap();
        let schema = jsonschema::validator_for(&tool.parameters_schema()).unwrap();
        for args in [
            json!({"url":format!("{base}/secret")}),
            json!({"url":format!("{base}/final"),"max_chars":499}),
            json!({"url":format!("{base}/final"),"max_chars":500.0}),
            json!({"url":format!("{base}/final"),"headers":{}}),
        ] {
            assert!(!schema.is_valid(&args) || args["max_chars"].is_f64());
            assert!(tool.execute(args).await.is_err());
        }
        assert!(seen.lock().unwrap().is_empty());
        assert!(tool
            .execute(json!({"url":format!("{base}/redirect"),"max_chars":500}))
            .await
            .unwrap()
            .contains("PUBLIC!"));
        assert!(tool
            .execute(json!({"url":format!("{base}/blocked")}))
            .await
            .is_err());
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        assert_eq!(*seen.lock().unwrap(), ["/redirect", "/final", "/blocked"]);
        assert_eq!(tool.failure_effect(), crate::ToolFailureEffect::Unknown);
    }
}
