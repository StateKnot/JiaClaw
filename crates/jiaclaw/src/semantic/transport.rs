// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One-attempt, bounded Brokerrouter embeddings and read-only result recovery.

use std::time::Duration;

use jiaclaw_core::{JiaClawError, ProviderConfig, SemanticMemoryConfig};
use reqwest::{header::HeaderMap, Client, Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stateknot_integrations::ProviderEndpoint;
use uuid::Uuid;

const MAX_INPUTS: usize = 8;
const MAX_INPUT_BYTES: usize = 1024;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_ID_HEADER: &str = "x-brokerrouter-request-id";

/// Immutable gateway binding. Secrets and endpoints are deliberately not Debug.
pub(crate) struct Transport {
    client: Client,
    base_url: String,
    virtual_key: String,
    model: String,
    dimensions: usize,
    fingerprint: String,
    credential_hash: String,
}

/// Validated, ordered unit vectors plus the gateway's durable request identity.
#[derive(Clone, Debug)]
pub(crate) struct EmbeddingReceipt {
    pub request_id: String,
    pub vectors: Vec<Vec<f32>>,
}

/// Sanitized failure retaining a known gateway identity for explicit recovery.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct EmbeddingFailure {
    pub request_id: Option<String>,
    pub message: String,
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'static str,
    dimensions: usize,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    model: String,
    data: Vec<EmbeddingData>,
    usage: EmbeddingUsage,
}

#[derive(Deserialize)]
struct EmbeddingData {
    index: usize,
    embedding: Vec<f64>,
}

#[derive(Deserialize)]
struct EmbeddingUsage {
    prompt_tokens: u64,
    total_tokens: u64,
}

impl Transport {
    pub(crate) fn new(
        provider: &ProviderConfig,
        config: &SemanticMemoryConfig,
    ) -> Result<Self, JiaClawError> {
        config.validate()?;
        if !config.enabled || provider.provider_type != "brokerrouter" {
            return Err(configuration(
                "semantic memory requires enabled Brokerrouter configuration",
            ));
        }
        let endpoint = if provider.base_url.starts_with("https://") {
            ProviderEndpoint::https(&provider.base_url)
        } else {
            ProviderEndpoint::loopback_http(&provider.base_url)
        };
        if provider.base_url.len() > 2048 || endpoint.is_err() {
            return Err(configuration("semantic memory requires HTTPS or literal-loopback HTTP without credentials/query/fragment"));
        }
        // URL normalization makes identity and the actual wire endpoint agree.
        let base_url = Url::parse(&provider.base_url)
            .map_err(|_| configuration("invalid semantic memory endpoint"))?
            .as_str()
            .trim_end_matches('/')
            .to_owned();
        let virtual_key = provider.api_key.as_deref().unwrap_or_default();
        if virtual_key.is_empty()
            || virtual_key.len() > 4096
            || virtual_key.bytes().any(|byte| !byte.is_ascii_graphic())
        {
            return Err(configuration("invalid semantic memory API key"));
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(config.timeout_secs.min(10)))
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|_| configuration("semantic memory HTTP client initialization failed"))?;
        let fingerprint = hash(
            &serde_json::to_vec(&serde_json::json!({
                "endpoint": base_url,
                "model": config.model,
                "space_revision": config.space_revision,
                "dimensions": config.dimensions,
                "preprocessing": "symmetric-v1",
                "normalization": "l2-f64-scaled-to-f32-v1"
            }))
            .expect("fixed fingerprint values serialize"),
        );
        Ok(Self {
            client,
            base_url,
            virtual_key: virtual_key.to_owned(),
            model: config.model.clone(),
            dimensions: config.dimensions,
            fingerprint,
            credential_hash: hash(virtual_key.as_bytes()),
        })
    }

    pub(crate) fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }

    pub(crate) fn credential_hash(&self) -> String {
        self.credential_hash.clone()
    }

    /// Hash the exact serialized payload used by `embed`, excluding credentials.
    pub(crate) fn request_hash(&self, input: &[String]) -> String {
        hash(&self.request_body(input))
    }

    fn request_body(&self, input: &[String]) -> Vec<u8> {
        serde_json::to_vec(&EmbeddingRequest {
            model: &self.model,
            input,
            encoding_format: "float",
            dimensions: self.dimensions,
        })
        .expect("fixed embedding request values serialize")
    }

    /// Submit exactly once. The caller persists operation identity before invoking this.
    pub(crate) async fn embed(
        &self,
        operation_id: &str,
        input: &[String],
    ) -> Result<EmbeddingReceipt, EmbeddingFailure> {
        if operation_id.is_empty()
            || operation_id.len() > 200
            || operation_id.bytes().any(|byte| !byte.is_ascii_graphic())
        {
            return Err(failure(None, "invalid embedding operation identity"));
        }
        if !(1..=MAX_INPUTS).contains(&input.len())
            || input
                .iter()
                .any(|text| text.is_empty() || text.len() > MAX_INPUT_BYTES)
        {
            return Err(failure(
                None,
                "embedding inputs require 1..=8 nonempty strings of at most 1024 bytes each",
            ));
        }
        let response = self
            .client
            .post(format!("{}/v1/embeddings", self.base_url))
            .bearer_auth(&self.virtual_key)
            .header("Content-Type", "application/json")
            .header("Idempotency-Key", operation_id)
            .body(self.request_body(input))
            .send()
            .await
            .map_err(|_| {
                failure(
                    None,
                    "embedding request failed or timed out; outcome unknown, not retried",
                )
            })?;
        self.receive(response, None, input.len()).await
    }

    /// Read a retained gateway result using the same immutable credential binding.
    /// No POST or provider-side result check is performed.
    pub(crate) async fn recover(
        &self,
        id: &str,
        expected_count: usize,
    ) -> Result<EmbeddingReceipt, EmbeddingFailure> {
        let request_id = Uuid::parse_str(id)
            .map_err(|_| failure(None, "invalid embedding recovery identity"))?
            .to_string();
        if !(1..=MAX_INPUTS).contains(&expected_count) {
            return Err(failure(
                Some(request_id),
                "invalid embedding recovery count",
            ));
        }
        let response = self
            .client
            .get(format!("{}/v1/requests/{request_id}/result", self.base_url))
            .bearer_auth(&self.virtual_key)
            .send()
            .await
            .map_err(|_| {
                failure(
                    Some(request_id.clone()),
                    "embedding recovery failed or timed out; not retried",
                )
            })?;
        self.receive(response, Some(request_id), expected_count)
            .await
    }

    async fn receive(
        &self,
        mut response: Response,
        expected_id: Option<String>,
        expected_count: usize,
    ) -> Result<EmbeddingReceipt, EmbeddingFailure> {
        let header_id = response_request_id(response.headers());
        let request_id = expected_id
            .clone()
            .or_else(|| header_id.as_ref().ok().and_then(Clone::clone));
        let invalid_header = header_id.is_err();
        let header_id = header_id.ok().flatten();
        if invalid_header
            || expected_id
                .as_ref()
                .zip(header_id.as_ref())
                .is_some_and(|(expected, actual)| expected != actual)
        {
            return Err(failure(
                request_id,
                "invalid embedding gateway request identity",
            ));
        }
        if response.status() != StatusCode::OK {
            return Err(failure(
                request_id,
                &format!(
                    "embedding HTTP status {}; not retried",
                    response.status().as_u16()
                ),
            ));
        }
        let Some(request_id) = request_id else {
            return Err(failure(
                None,
                "embedding response is missing the gateway request identity",
            ));
        };
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(failure(
                Some(request_id),
                "embedding response exceeds 2 MiB",
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            failure(
                Some(request_id.clone()),
                "embedding response interrupted or timed out; not retried",
            )
        })? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(failure(
                    Some(request_id),
                    "embedding response exceeds 2 MiB",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        self.validate_receipt(&bytes, request_id, expected_count)
    }

    fn validate_receipt(
        &self,
        bytes: &[u8],
        request_id: String,
        expected_count: usize,
    ) -> Result<EmbeddingReceipt, EmbeddingFailure> {
        let invalid = || {
            failure(
                Some(request_id.clone()),
                "invalid embedding response contract",
            )
        };
        let response: EmbeddingResponse = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if response.model != self.model
            || response.data.len() != expected_count
            || response.usage.prompt_tokens != response.usage.total_tokens
        {
            return Err(invalid());
        }
        let mut vectors = vec![None; expected_count];
        for datum in response.data {
            if datum.index >= expected_count
                || vectors[datum.index].is_some()
                || datum.embedding.len() != self.dimensions
            {
                return Err(invalid());
            }
            vectors[datum.index] = Some(normalize(&datum.embedding).ok_or_else(invalid)?);
        }
        let vectors = vectors
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(invalid)?;
        Ok(EmbeddingReceipt {
            request_id,
            vectors,
        })
    }
}

fn response_request_id(headers: &HeaderMap) -> Result<Option<String>, ()> {
    let mut values = headers.get_all(REQUEST_ID_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(());
    }
    Uuid::parse_str(value.to_str().map_err(|_| ())?)
        .map(|id| Some(id.to_string()))
        .map_err(|_| ())
}

// Scaling before squaring avoids overflow/underflow for valid finite gateway vectors.
#[allow(clippy::cast_possible_truncation)]
fn normalize(vector: &[f64]) -> Option<Vec<f32>> {
    if vector.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let scale = vector
        .iter()
        .fold(0.0_f64, |largest, value| largest.max(value.abs()));
    if scale == 0.0 {
        return None;
    }
    let norm = vector
        .iter()
        .map(|value| (value / scale).powi(2))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    Some(
        vector
            .iter()
            .map(|value| ((value / scale) / norm) as f32)
            .collect(),
    )
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn configuration(message: &str) -> JiaClawError {
    JiaClawError::Configuration(message.into())
}

fn failure(request_id: Option<String>, message: &str) -> EmbeddingFailure {
    EmbeddingFailure {
        request_id,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    const ID: &str = "10203040-5060-4070-8090-102030405060";

    fn config() -> SemanticMemoryConfig {
        SemanticMemoryConfig {
            enabled: true,
            model: "embed-v1".into(),
            space_revision: "fixture-v1".into(),
            dimensions: 2,
            timeout_secs: 1,
            ..SemanticMemoryConfig::default()
        }
    }

    fn transport(url: &str) -> Transport {
        Transport::new(
            &ProviderConfig {
                base_url: url.into(),
                api_key: Some("private-virtual-key".into()),
                ..ProviderConfig::default()
            },
            &config(),
        )
        .unwrap()
    }

    fn body() -> String {
        serde_json::json!({"model":"embed-v1","data":[{"index":0,"embedding":[3.0,4.0]}],"usage":{"prompt_tokens":2,"total_tokens":2}}).to_string()
    }

    struct Fixture {
        url: String,
        requests: Arc<Mutex<Vec<String>>>,
        task: JoinHandle<()>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn fixture(status: u16, headers: &str, body: String, delay_body: bool) -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let headers = format!(
            "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        );
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let count = socket.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|value| value == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                        let length = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .map_or(0, |value| value.parse::<usize>().unwrap());
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                if socket.write_all(headers.as_bytes()).await.is_err() {
                    continue;
                }
                if delay_body {
                    tokio::time::sleep(Duration::from_millis(1200)).await;
                }
                let _ = socket.write_all(body.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        Fixture {
            url,
            requests,
            task,
        }
    }

    #[tokio::test]
    async fn exact_request_identity_and_valid_unit_vectors() {
        let fixture = fixture(
            200,
            &format!("{REQUEST_ID_HEADER}: {ID}\r\n"),
            body(),
            false,
        )
        .await;
        let client = transport(&fixture.url);
        let input = vec!["你好 memory".to_owned()];
        let receipt = client.embed("operation-immutable", &input).await.unwrap();
        assert_eq!(receipt.request_id, ID);
        assert_eq!(receipt.vectors, vec![vec![0.6, 0.8]]);
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.starts_with("POST /v1/embeddings HTTP/1.1\r\n"));
        assert!(request.contains("authorization: Bearer private-virtual-key\r\n"));
        assert!(request.contains("idempotency-key: operation-immutable\r\n"));
        let body = request.split_once("\r\n\r\n").unwrap().1;
        assert_eq!(client.request_hash(&input), hash(body.as_bytes()));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            serde_json::json!({"model":"embed-v1","input":input,"encoding_format":"float","dimensions":2})
        );
    }

    #[tokio::test]
    async fn recovery_is_get_only_and_does_not_require_an_absent_upstream_header() {
        let fixture = fixture(200, "", body(), false).await;
        let receipt = transport(&fixture.url).recover(ID, 1).await.unwrap();
        assert_eq!(receipt.request_id, ID);
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with(&format!("GET /v1/requests/{ID}/result HTTP/1.1\r\n")));
        assert!(!requests[0].contains("idempotency-key"));
        assert!(requests[0].contains("authorization: Bearer private-virtual-key\r\n"));
    }

    #[tokio::test]
    async fn unknown_failures_preserve_remote_identity_without_retry_or_secret_reflection() {
        for (status, body, delay) in [
            (
                502,
                "private-virtual-key remote provider body".into(),
                false,
            ),
            (200, "malformed private-virtual-key".into(), false),
            (200, body(), true),
            (200, "x".repeat(MAX_RESPONSE_BYTES + 1), false),
        ] {
            let fixture = fixture(
                status,
                &format!("{REQUEST_ID_HEADER}: {ID}\r\n"),
                body,
                delay,
            )
            .await;
            let error = transport(&fixture.url)
                .embed("op", &["text".into()])
                .await
                .unwrap_err();
            assert_eq!(error.request_id.as_deref(), Some(ID));
            assert!(!error.message.contains("private-virtual-key"));
            assert!(!error.message.contains("127.0.0.1"));
            assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn identity_must_be_valid_unique_and_match_recovery() {
        for headers in [
            String::new(),
            format!("{REQUEST_ID_HEADER}: invalid\r\n"),
            format!("{REQUEST_ID_HEADER}: {ID}\r\n{REQUEST_ID_HEADER}: {ID}\r\n"),
        ] {
            let fixture = fixture(200, &headers, body(), false).await;
            assert!(transport(&fixture.url)
                .embed("op", &["text".into()])
                .await
                .is_err());
        }
        let fixture = fixture(
            200,
            &format!("{REQUEST_ID_HEADER}: 11203040-5060-4070-8090-102030405060\r\n"),
            body(),
            false,
        )
        .await;
        let error = transport(&fixture.url).recover(ID, 1).await.unwrap_err();
        assert_eq!(error.request_id.as_deref(), Some(ID));
    }

    #[tokio::test]
    async fn redirects_and_local_invalid_inputs_make_no_extra_requests() {
        let target = fixture(
            200,
            &format!("{REQUEST_ID_HEADER}: {ID}\r\n"),
            body(),
            false,
        )
        .await;
        let fixture = fixture(
            307,
            &format!("Location: {}/v1/embeddings\r\n", target.url),
            String::new(),
            false,
        )
        .await;
        let client = transport(&fixture.url);
        assert!(client.embed("op", &["text".into()]).await.is_err());
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        assert!(target.requests.lock().unwrap().is_empty());
        for input in [
            vec![],
            vec![String::new()],
            vec!["x".repeat(1025)],
            vec!["a".into(); 9],
        ] {
            assert!(client.embed("op", &input).await.is_err());
        }
        for id in ["", "contains space", "new\nline", &"x".repeat(201)] {
            assert!(client.embed(id, &["text".into()]).await.is_err());
        }
        assert!(client.recover("../escape", 1).await.is_err());
        assert!(client.recover(ID, 0).await.is_err());
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn ordered_receipts_reject_invalid_model_usage_indices_and_vectors() {
        let client = transport("http://127.0.0.1:9");
        let mut response = serde_json::json!({"model":"embed-v1","data":[{"index":1,"embedding":[0.0,7.0]},{"index":0,"embedding":[8.0,0.0]}],"usage":{"prompt_tokens":2,"total_tokens":2}});
        let validate = |value: &serde_json::Value| {
            client.validate_receipt(&serde_json::to_vec(value).unwrap(), ID.into(), 2)
        };
        assert_eq!(
            validate(&response).unwrap().vectors,
            vec![vec![1.0, 0.0], vec![0.0, 1.0]]
        );
        for (pointer, value) in [
            ("/model", serde_json::json!("provider-physical-model")),
            ("/usage/total_tokens", serde_json::json!(3)),
            ("/usage/prompt_tokens", serde_json::json!(-1)),
            ("/usage/prompt_tokens", serde_json::json!(1.5)),
            ("/data/0/index", serde_json::json!(0)),
            ("/data/0/index", serde_json::json!(2)),
            ("/data/0/index", serde_json::json!(-1)),
            ("/data/0/embedding", serde_json::json!([0, 0])),
            ("/data/0/embedding", serde_json::json!([1])),
            ("/data", serde_json::json!([])),
        ] {
            let mut invalid = response.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            let error = validate(&invalid).unwrap_err();
            assert_eq!(error.request_id.as_deref(), Some(ID));
        }
        response.as_object_mut().unwrap().remove("usage");
        assert!(validate(&response).is_err());
        assert!(normalize(&[f64::NAN, 1.0]).is_none());
        assert!(normalize(&[f64::INFINITY, 1.0]).is_none());
        assert_eq!(normalize(&[f64::MAX, 0.0]), Some(vec![1.0, 0.0]));
        assert_eq!(normalize(&[f64::MIN_POSITIVE, 0.0]), Some(vec![1.0, 0.0]));
    }

    #[test]
    fn immutable_space_and_credential_identity_are_separate_and_non_sensitive() {
        let provider = ProviderConfig {
            base_url: "https://EXAMPLE.com:443/gateway/".into(),
            api_key: Some("secret-one".into()),
            ..ProviderConfig::default()
        };
        let original = Transport::new(&provider, &config()).unwrap();
        assert_eq!(original.fingerprint().len(), 64);
        assert_eq!(original.credential_hash().len(), 64);
        let mut equivalent = provider.clone();
        equivalent.base_url = "https://example.com/gateway".into();
        assert_eq!(
            original.fingerprint(),
            Transport::new(&equivalent, &config())
                .unwrap()
                .fingerprint()
        );
        equivalent.api_key = Some("secret-two".into());
        let rotated = Transport::new(&equivalent, &config()).unwrap();
        assert_eq!(original.fingerprint(), rotated.fingerprint());
        assert_ne!(original.credential_hash(), rotated.credential_hash());
        let mut revision = config();
        revision.space_revision = "fixture-v2".into();
        assert_ne!(
            original.fingerprint(),
            Transport::new(&provider, &revision).unwrap().fingerprint()
        );
        assert_ne!(
            original.request_hash(&["one".into()]),
            original.request_hash(&["two".into()])
        );
        for endpoint in [
            "http://localhost:80",
            "http://192.168.1.1",
            "https://user:secret@example.com",
            "https://example.com?secret=x",
            "https://example.com#fragment",
        ] {
            equivalent.base_url = endpoint.into();
            assert!(Transport::new(&equivalent, &config()).is_err());
        }
    }
}
