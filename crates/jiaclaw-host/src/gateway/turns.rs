// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Tenant HTTP identities. Only the first admission can send a backend PUT.
use super::{
    registry::{HttpAdmissionError, Principal, Registry, WriteAdmissionError},
    State,
};
use crate::{http_turn_store::Receipt, http_turns::Submission};
use anyhow::{ensure, Result};
use axum::{
    body::to_bytes,
    extract::Request,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::{sync::OwnedSemaphorePermit, time::Instant};
use uuid::Uuid;

const MAX_BODY: usize = 64 * 1024;
const MAX_RECEIPT: usize = crate::http_turn_store::MAX_RESULT_BYTES + 16_384;
const IO_BUDGET: Duration = Duration::from_secs(5);
#[path = "turn_stream.rs"]
mod stream;

fn json_content(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(header::CONTENT_TYPE).iter();
    values
        .next()
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(';')
                .next()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
        })
        && values.next().is_none()
}

#[derive(Deserialize)]
struct Capability {
    #[serde(default)]
    file_download_protocol: Option<u8>,
    feedback_protocol: u8,
    protocol: u8,
    gateway_protocol: u8,
    agent_name: String,
    enabled: bool,
    streaming: bool,
    max_active: usize,
    session_prefix: String,
    max_receipt_bytes: usize,
    stream_suffix: String,
    max_stream_wire_bytes: usize,
    max_preview_round_bytes: usize,
    max_preview_total_bytes: usize,
    #[serde(default)]
    file_read: Option<jiaclaw::TenantFileReadCapability>,
    #[serde(default)]
    file_write: Option<jiaclaw::TenantFileWriteCapability>,
    #[serde(default)]
    skill_read: Option<jiaclaw::TenantSkillReadCapability>,
    #[serde(default)]
    memory_search: Option<jiaclaw::TenantMemorySearchCapability>,
}

pub(super) async fn check_backend(
    client: &reqwest::Client,
    url: &reqwest::Url,
    token: &HeaderValue,
    identity: &str,
    file_read: Option<&jiaclaw::TenantFileReadCapability>,
    file_write: Option<&jiaclaw::TenantFileWriteCapability>,
    skill_read: Option<&jiaclaw::TenantSkillReadCapability>,
    memory_search: Option<&jiaclaw::TenantMemorySearchCapability>,
    file_download: bool,
) -> Result<bool> {
    tokio::time::timeout(IO_BUDGET, async {
        let response = client
            .get(url.join("api/turns/capabilities")?)
            .header(header::AUTHORIZATION, token.clone())
            .header("x-jiaclaw-gateway-turns", "1")
            .timeout(IO_BUDGET)
            .send()
            .await?;
        ensure!(
            response.status() == StatusCode::OK && json_content(response.headers()),
            "backend HTTP turn handshake rejected"
        );
        let bytes = super::proxy::read_response(response, 32 * 1024)
            .await
            .map_err(|()| anyhow::anyhow!("invalid backend HTTP turn handshake"))?;
        let c: Capability = serde_json::from_slice(&bytes)?;
        ensure!(
            c.protocol == 1
                && c.feedback_protocol == u8::from(c.enabled)
                && c.gateway_protocol == 2
                && c.agent_name == identity
                && c.streaming == c.enabled
                && c.max_active == 1
                && c.session_prefix == "http:"
                && c.max_receipt_bytes == MAX_RECEIPT
                && c.stream_suffix == "/stream"
                && c.max_stream_wire_bytes == crate::http_turns::delivery::WIRE_BYTES
                && c.max_preview_round_bytes == crate::http_turns::delivery::ROUND_BYTES
                && c.max_preview_total_bytes == crate::http_turns::delivery::PREVIEW_BYTES,
            "backend HTTP turn contract mismatch"
        );
        ensure!(
            c.file_download_protocol == file_download.then_some(1)
                && c.file_read.as_ref() == file_read
                && c.file_write.as_ref() == file_write
                && c.skill_read.as_ref() == skill_read
                && c.memory_search.as_ref() == memory_search,
            "backend exact tenant permission contract mismatch"
        );
        Ok(c.enabled)
    })
    .await
    .map_err(|_| anyhow::anyhow!("backend HTTP turn handshake timed out"))?
}

enum Route {
    File(usize, String),
    Feedback(Uuid),
    Capabilities,
    Catalog(usize, usize),
    Turn(Uuid),
    Stream(Uuid),
    Cancel(Uuid),
}
fn canonical(value: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(value).ok()?;
    crate::jobs::valid_creation_id(value).then_some(id)
}
fn route(method: &Method, uri: &Uri) -> Option<Route> {
    if uri.scheme().is_some() || uri.authority().is_some() {
        return None;
    }
    if uri.path() == "/api/turns" && method == Method::GET {
        let (mut limit, mut offset) = (20, 0);
        let mut seen = std::collections::HashSet::new();
        if let Some(query) = uri.query() {
            if query.is_empty() || query.len() > 128 {
                return None;
            }
            for item in query.split('&') {
                let (name, value) = item.split_once('=')?;
                if value.is_empty()
                    || !value.bytes().all(|b| b.is_ascii_digit())
                    || !seen.insert(name)
                {
                    return None;
                }
                let value: usize = value.parse().ok()?;
                match name {
                    "limit" if (1..=50).contains(&value) => limit = value,
                    "offset" if value <= 10_000 => offset = value,
                    _ => return None,
                }
            }
        }
        return Some(Route::Catalog(limit, offset));
    }
    if uri.query().is_some() {
        return None;
    }
    if let Some(tail) = uri.path().strip_prefix("/api/turns/files/") {
        let (index, policy) = tail.split_once('/')?;
        return (method == Method::GET)
            .then(|| {
                crate::tenant_file_download::target(index, policy)
                    .map(|n| Route::File(n, policy.to_owned()))
            })
            .flatten();
    }
    if uri.path() == "/api/turns/capabilities" && method == Method::GET {
        return Some(Route::Capabilities);
    }
    let tail = uri.path().strip_prefix("/api/turns/")?;
    if let Some(value) = tail.strip_suffix("/feedback") {
        return (method == Method::PUT)
            .then(|| canonical(value).map(Route::Feedback))
            .flatten();
    }
    if let Some(value) = tail.strip_suffix("/stream") {
        return (method == Method::PUT)
            .then(|| canonical(value).map(Route::Stream))
            .flatten();
    }
    if let Some(value) = tail.strip_suffix("/cancel") {
        return (method == Method::POST)
            .then(|| canonical(value).map(Route::Cancel))
            .flatten();
    }
    if method != Method::GET && method != Method::PUT {
        return None;
    }
    canonical(tail).map(Route::Turn)
}

fn valid_submission(s: &Submission) -> bool {
    s.session_id
        .strip_prefix("http:")
        .and_then(canonical)
        .is_some()
        && !s.prompt.trim().is_empty()
        && s.prompt.len() <= 32 * 1024
        && (1..=6).contains(&s.enabled_tools.len())
        && s.enabled_skills.is_empty()
        && s.enabled_tools.iter().all(|t| {
            matches!(
                t.as_str(),
                "datetime_now"
                    | "json_query"
                    | "file_read"
                    | "file_write"
                    | "skill_read"
                    | "memory_search"
            )
        })
        && s.enabled_tools
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            == s.enabled_tools.len()
        && if s.enabled_tools.iter().any(|t| t == "file_read") {
            s.file_read_policy_sha256.as_ref().is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        } else {
            s.file_read_policy_sha256.is_none()
        }
        && if s.enabled_tools.iter().any(|t| t == "file_write") {
            s.file_write_policy_sha256.as_ref().is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        } else {
            s.file_write_policy_sha256.is_none()
        }
        && if s.enabled_tools.iter().any(|t| t == "skill_read") {
            s.skill_read_policy_sha256.as_ref().is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        } else {
            s.skill_read_policy_sha256.is_none()
        }
        && if s.enabled_tools.iter().any(|t| t == "memory_search") {
            s.memory_search_policy_sha256.as_ref().is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        } else {
            s.memory_search_policy_sha256.is_none()
        }
}

// An abandoned waiter must not free capacity still used by a blocking DB call.
struct Control {
    _global: OwnedSemaphorePermit,
    _backend: Option<OwnedSemaphorePermit>,
}
async fn db<T: Send + 'static>(
    registry: Registry,
    owner: Arc<Control>,
    operation: impl FnOnce(Registry) -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(move || {
        let _owner = owner;
        operation(registry)
    })
    .await?
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    protocol: u8,
    receipt: Receipt,
    active: bool,
}
impl Envelope {
    fn validate(&self, id: Uuid, hash: &str, session: &str) -> Result<()> {
        let r = &self.receipt;
        ensure!(
            self.protocol == 1
                && r.id == id.to_string()
                && r.request_hash == hash
                && r.session_id == session
                && r.session_id
                    .strip_prefix("http:")
                    .and_then(canonical)
                    .is_some()
                && r.context_hash.len() == 64
                && r.context_hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && r.error.as_ref().is_none_or(|s| s.len() <= 128)
                && r.review_note.as_ref().is_none_or(|s| s.len() <= 1024),
            "invalid original HTTP receipt"
        );
        ensure!(
            r.feedback_revision <= 2_147_483_647
                && r.feedback
                    .as_ref()
                    .is_none_or(|f| f.validate(r.feedback_revision)
                        && r.state == "completed"
                        && !r.result_purged),
            "invalid original feedback receipt"
        );
        ensure!(
            match r.state.as_str() {
                "running" =>
                    r.finished_ms.is_none()
                        && !r.session_committed
                        && r.result.is_none()
                        && !r.result_purged
                        && r.reviewed_ms.is_none(),
                "completed" =>
                    r.finished_ms.is_some()
                        && r.session_committed
                        && r.error.is_none()
                        && r.reviewed_ms.is_none(),
                "needs_review" => r.finished_ms.is_some(),
                _ => false,
            } && (!r.result_purged || r.result.is_none())
                && (r.reviewed_ms.is_none()
                    || (r.state == "needs_review" && r.review_note.is_some())),
            "invalid HTTP receipt state"
        );
        Ok(())
    }
    fn known_success(&self) -> bool {
        !self.active
            && self.receipt.state == "completed"
            && self.receipt.session_committed
            && self.receipt.error.is_none()
    }
}

async fn remote(
    state: &State,
    p: &Principal,
    id: Uuid,
    hash: &str,
    session: &str,
    method: Method,
    body: Option<&Submission>,
    cancel: bool,
) -> Result<(StatusCode, Envelope)> {
    let backend = &state.backends[&p.backend_id];
    let path = format!("api/turns/{id}{}", if cancel { "/cancel" } else { "" });
    tokio::time::timeout(IO_BUDGET, async {
        let mut request = state
            .client
            .request(method, backend.url.join(&path)?)
            .header(header::AUTHORIZATION, backend.token.clone())
            .header("x-jiaclaw-gateway-turns", "1")
            .timeout(IO_BUDGET);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        ensure!(
            matches!(status, StatusCode::OK | StatusCode::ACCEPTED)
                && json_content(response.headers()),
            "original receipt unavailable"
        );
        let bytes = super::proxy::read_response(response, MAX_RECEIPT)
            .await
            .map_err(|()| anyhow::anyhow!("invalid original receipt"))?;
        let envelope: Envelope = serde_json::from_slice(&bytes)?;
        envelope.validate(id, hash, session)?;
        Ok((status, envelope))
    })
    .await
    .map_err(|_| anyhow::anyhow!("original receipt timed out"))?
}

fn response(value: impl IntoResponse, request_id: Uuid) -> Response {
    let mut r = value.into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    r.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&request_id.to_string()).expect("UUID header"),
    );
    r
}

async fn remote_feedback(
    state: &State,
    principal: &Principal,
    id: Uuid,
    hash: &str,
    session: &str,
    body: &crate::http_turn_store::FeedbackRequest,
) -> Result<(StatusCode, Option<Envelope>)> {
    let backend = &state.backends[&principal.backend_id];
    tokio::time::timeout(IO_BUDGET, async {
        let response = state
            .client
            .put(backend.url.join(&format!("api/turns/{id}/feedback"))?)
            .header(header::AUTHORIZATION, backend.token.clone())
            .header("x-jiaclaw-gateway-turns", "1")
            .json(body)
            .timeout(IO_BUDGET)
            .send()
            .await?;
        let status = response.status();
        ensure!(
            json_content(response.headers()),
            "invalid feedback response"
        );
        let bytes = super::proxy::read_response(response, MAX_RECEIPT)
            .await
            .map_err(|()| anyhow::anyhow!("invalid feedback response"))?;
        if matches!(
            status,
            StatusCode::BAD_REQUEST | StatusCode::CONFLICT | StatusCode::NOT_FOUND
        ) {
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            ensure!(
                value.get("error").is_some_and(serde_json::Value::is_string),
                "invalid feedback refusal"
            );
            return Ok((status, None));
        }
        ensure!(status == StatusCode::OK, "feedback outcome unknown");
        let envelope: Envelope = serde_json::from_slice(&bytes)?;
        envelope.validate(id, hash, session)?;
        ensure!(
            envelope.known_success()
                && !envelope.receipt.result_purged
                && envelope.receipt.result.as_ref().is_some_and(|result| {
                    result.get("status").and_then(serde_json::Value::as_str) == Some("completed")
                        && result
                            .get("reply")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|reply| reply.len() <= 2 * 1024 * 1024)
                        && result
                            .get("tool_names")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|names| {
                                names.len() <= 2048
                                    && names.iter().all(|name| {
                                        name.as_str().is_some_and(|name| {
                                            matches!(
                                                name,
                                                "datetime_now"
                                                    | "json_query"
                                                    | "file_read"
                                                    | "file_write"
                                                    | "skill_read"
                                                    | "memory_search"
                                            )
                                        })
                                    })
                            })
                }),
            "feedback receipt unavailable"
        );
        let r = &envelope.receipt;
        ensure!(
            r.feedback_revision == body.expected_revision
                || r.feedback_revision == body.expected_revision + 1,
            "feedback revision mismatch"
        );
        ensure!(
            if body.decision == crate::http_turn_store::FeedbackDecision::Clear {
                r.feedback.is_none()
            } else {
                r.feedback
                    .as_ref()
                    .is_some_and(|f| f.decision == body.decision && f.note == body.note)
            },
            "feedback intent mismatch"
        );
        Ok((status, Some(envelope)))
    })
    .await
    .map_err(|_| anyhow::anyhow!("feedback request timed out"))?
}
fn error(status: StatusCode, code: &'static str, id: Uuid) -> Response {
    response((status, Json(json!({"error":code}))), id)
}
fn admission_error(e: &anyhow::Error, id: Uuid) -> Response {
    let (status, code) = match e.downcast_ref::<WriteAdmissionError>() {
        Some(WriteAdmissionError::Unauthorized) => (StatusCode::UNAUTHORIZED, "invalid API key"),
        Some(WriteAdmissionError::ReadOnly) => (StatusCode::FORBIDDEN, "read-only API key"),
        Some(WriteAdmissionError::Held) => (
            StatusCode::CONFLICT,
            "user write needs completion or review",
        ),
        None => match e.downcast_ref::<HttpAdmissionError>() {
            Some(HttpAdmissionError::IdentityConflict) => {
                (StatusCode::CONFLICT, "request identity conflict")
            }
            Some(HttpAdmissionError::Full) => {
                (StatusCode::CONFLICT, "permanent request capacity reached")
            }
            None => (StatusCode::SERVICE_UNAVAILABLE, "registry unavailable"),
        },
    };
    error(status, code, id)
}

pub(super) async fn handle(state: Arc<State>, request: Request) -> Response {
    let request_id = Uuid::new_v4();
    let deadline = Instant::now() + state.timeout;
    let Some(route) = route(request.method(), request.uri()).filter(|_| state.tracked_turns) else {
        return error(StatusCode::NOT_FOUND, "route unavailable", request_id);
    };
    let Some(token) = super::keys::authorization_token(request.headers()).map(str::to_owned) else {
        return error(StatusCode::UNAUTHORIZED, "invalid API key", request_id);
    };
    let Ok(global) = state.control.clone().try_acquire_owned() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "gateway control capacity reached",
            request_id,
        );
    };
    let control = Arc::new(Control {
        _global: global,
        _backend: None,
    });
    let current_token = token.clone();
    let p = match db(state.registry.clone(), control.clone(), move |r| {
        r.authenticate(&token)
    })
    .await
    {
        Ok(Some(p)) => p,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "invalid API key", request_id),
        Err(e) => return admission_error(&e, request_id),
    };
    let write = request.method() != Method::GET;
    if write && p.read_only {
        return error(StatusCode::FORBIDDEN, "read-only API key", request_id);
    }
    let Some(backend) = state.backends.get(&p.backend_id) else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend unavailable",
            request_id,
        );
    };
    let Ok(backend_control) = backend.control.clone().try_acquire_owned() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "user control capacity reached",
            request_id,
        );
    };
    // The authentication closure has completed; its last clone was dropped.
    let mut control = Arc::try_unwrap(control)
        .ok()
        .expect("authentication owner completed");
    control._backend = Some(backend_control);
    let control = Arc::new(control);
    match route {
        Route::File(index, policy) => {
            if !backend.file_download
                || !crate::tenant_file_download::empty_request(request.headers())
                || !backend
                    .file_read
                    .as_ref()
                    .is_some_and(|c| c.policy_sha256 == policy && index < c.paths.len())
            {
                return error(
                    StatusCode::FORBIDDEN,
                    "file download not authorized",
                    request_id,
                );
            }
            let result = tokio::time::timeout(IO_BUDGET, async {
                ensure!(
                    check_backend(
                        &state.client,
                        &backend.url,
                        &backend.token,
                        &p.backend_id,
                        backend.file_read.as_ref(),
                        backend.file_write.as_ref(),
                        backend.skill_read.as_ref(),
                        backend.memory_search.as_ref(),
                        backend.file_download
                    )
                    .await?,
                    "backend file download unavailable"
                );
                let response = state
                    .client
                    .get(
                        backend
                            .url
                            .join(&format!("api/turns/files/{index}/{policy}"))?,
                    )
                    .header(header::AUTHORIZATION, backend.token.clone())
                    .header("x-jiaclaw-gateway-turns", "1")
                    .timeout(IO_BUDGET)
                    .send()
                    .await?;
                ensure!(
                    response.status() == StatusCode::OK,
                    "backend file capture refused"
                );
                let headers = response.headers().clone();
                let bytes = super::proxy::read_response(response, jiaclaw::READ_FILE_MAX_BYTES)
                    .await
                    .map_err(|()| anyhow::anyhow!("file capture body refused"))?;
                ensure!(
                    crate::tenant_file_download::valid(&headers, &bytes, index, &policy),
                    "invalid file capture contract"
                );
                let now = db(state.registry.clone(), control.clone(), move |r| {
                    r.authenticate(&current_token)
                })
                .await?;
                ensure!(
                    now.is_some_and(|q| q.user_id == p.user_id
                        && q.key_id == p.key_id
                        && q.backend_id == p.backend_id
                        && q.read_only == p.read_only),
                    "key revoked during capture"
                );
                Ok::<_, anyhow::Error>(bytes)
            })
            .await;
            match result {
                Ok(Ok(bytes)) => response(
                    crate::tenant_file_download::response(bytes, index, &policy),
                    request_id,
                ),
                _ => error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "file capture unavailable; no download",
                    request_id,
                ),
            }
        }
        Route::Capabilities => {
            let mut value = json!({"protocol":1,"gateway_protocol":2,"enabled":true,"streaming":true,"listing":true,"scope":"gateway","session_prefix":"http:","max_identities":10000,"max_page":50,"enabled_tools":["datetime_now","json_query"],"stream_suffix":"/stream","turn_budget_secs":state.timeout.as_secs(),"max_stream_wire_bytes":crate::http_turns::delivery::WIRE_BYTES,"max_preview_round_bytes":crate::http_turns::delivery::ROUND_BYTES,"max_preview_total_bytes":crate::http_turns::delivery::PREVIEW_BYTES});
            value["feedback_protocol"] = json!(1);
            if backend.file_download {
                value["file_download_protocol"] = json!(1);
            }

            if let Some(permission) = &backend.file_read {
                value["enabled_tools"] = json!(["datetime_now", "json_query", "file_read"]);
                value["file_read"] = json!(permission);
            }
            if let Some(permission) = &backend.file_write {
                value["enabled_tools"]
                    .as_array_mut()
                    .expect("static tool list")
                    .push(json!("file_write"));
                value["file_write"] = json!(permission);
            }
            if let Some(permission) = &backend.skill_read {
                value["enabled_tools"]
                    .as_array_mut()
                    .expect("static tool list")
                    .push(json!("skill_read"));
                value["skill_read"] = json!(permission);
            }
            if let Some(permission) = &backend.memory_search {
                value["enabled_tools"]
                    .as_array_mut()
                    .expect("static tool list")
                    .push(json!("memory_search"));
                value["memory_search"] = json!(permission);
            }
            response(Json(value), request_id)
        }
        Route::Catalog(limit, offset) => {
            let principal = p.clone();
            match db(state.registry.clone(), control, move |r| {
                r.http_turn_catalog(&principal, limit, offset)
            })
            .await
            {
                Ok((requests, has_more)) => response(
                    Json(
                        json!({"protocol":1,"scope":"gateway","requests":requests,"limit":limit,"offset":offset,"has_more":has_more}),
                    ),
                    request_id,
                ),
                Err(e) => admission_error(&e, request_id),
            }
        }
        Route::Feedback(id) => {
            if !json_content(request.headers()) {
                return error(
                    StatusCode::BAD_REQUEST,
                    "JSON feedback required",
                    request_id,
                );
            }
            let bytes =
                match tokio::time::timeout(IO_BUDGET, to_bytes(request.into_body(), 8192)).await {
                    Ok(Ok(v)) => v,
                    Ok(Err(_)) => {
                        return error(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            "feedback body too large",
                            request_id,
                        )
                    }
                    Err(_) => {
                        return error(
                            StatusCode::REQUEST_TIMEOUT,
                            "feedback body timed out",
                            request_id,
                        )
                    }
                };
            let Ok(body) =
                serde_json::from_slice::<crate::http_turn_store::FeedbackRequest>(&bytes)
            else {
                return error(StatusCode::BAD_REQUEST, "invalid feedback", request_id);
            };
            if !body.validate() {
                return error(StatusCode::BAD_REQUEST, "invalid feedback", request_id);
            }
            let principal = p.clone();
            let existing = match db(state.registry.clone(), control.clone(), move |r| {
                r.http_turn_identity(&principal, id, true)
            })
            .await
            {
                Ok(Some(v)) => v,
                Ok(None) => {
                    return error(
                        StatusCode::NOT_FOUND,
                        "original request not found",
                        request_id,
                    )
                }
                Err(e) => return admission_error(&e, request_id),
            };
            match remote_feedback(&state, &p, id, &existing.hash, &existing.session, &body).await {
                Ok((StatusCode::OK, Some(envelope))) => response(Json(envelope), request_id),
                Ok((status, _)) => error(
                    status,
                    "feedback changed or unavailable; read original request",
                    request_id,
                ),
                Err(_) => error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "feedback outcome unknown; read original request before another write",
                    request_id,
                ),
            }
        }
        Route::Turn(id) | Route::Stream(id) | Route::Cancel(id) => {
            let cancel = matches!(route, Route::Cancel(_));
            let streaming = matches!(route, Route::Stream(_));
            let principal = p.clone();
            let existing = match db(state.registry.clone(), control.clone(), move |r| {
                r.http_turn_identity(&principal, id, write)
            })
            .await
            {
                Ok(v) => v,
                Err(e) => return admission_error(&e, request_id),
            };
            let body = if request.method() == Method::PUT {
                if !json_content(request.headers())
                    || (!streaming
                        && request
                            .headers()
                            .get_all(header::ACCEPT)
                            .iter()
                            .any(|v| v.to_str().is_ok_and(|v| v.contains("text/event-stream"))))
                {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "JSON submission required",
                        request_id,
                    );
                }
                let bytes =
                    match tokio::time::timeout(IO_BUDGET, to_bytes(request.into_body(), MAX_BODY))
                        .await
                    {
                        Ok(Ok(v)) => v,
                        Ok(Err(_)) => {
                            return error(
                                StatusCode::PAYLOAD_TOO_LARGE,
                                "request body too large",
                                request_id,
                            )
                        }
                        Err(_) => {
                            return error(
                                StatusCode::REQUEST_TIMEOUT,
                                "request body timed out",
                                request_id,
                            )
                        }
                    };
                let Ok(s) = serde_json::from_slice::<Submission>(&bytes) else {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "invalid HTTP submission",
                        request_id,
                    );
                };
                if !valid_submission(&s) {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "invalid tenant tool or session selection",
                        request_id,
                    );
                }
                Some(s)
            } else {
                None
            };
            if let Some(identity) = existing {
                let hash = identity.hash;
                if body
                    .as_ref()
                    .is_some_and(|s| !s.fingerprint().is_ok_and(|value| value == hash))
                {
                    return error(
                        StatusCode::CONFLICT,
                        "request identity conflict",
                        request_id,
                    );
                }
                // Never PUT an existing identity, including after an operator clears a hold.
                match remote(
                    &state,
                    &p,
                    id,
                    &hash,
                    &identity.session,
                    if cancel { Method::POST } else { Method::GET },
                    None,
                    cancel,
                )
                .await
                {
                    Ok((status, envelope)) => response((status, Json(envelope)), request_id),
                    Err(_) => response(
                        (
                            StatusCode::CONFLICT,
                            Json(
                                json!({"error":"original receipt unavailable; review required","id":id.to_string()}),
                            ),
                        ),
                        request_id,
                    ),
                }
            } else if let Some(body) = body {
                let Ok(global) = state.permits.clone().try_acquire_owned() else {
                    return error(
                        StatusCode::TOO_MANY_REQUESTS,
                        "gateway execution capacity reached",
                        request_id,
                    );
                };
                let Ok(permit) = backend.permit.clone().try_acquire_owned() else {
                    return error(
                        StatusCode::TOO_MANY_REQUESTS,
                        "user request already in progress",
                        request_id,
                    );
                };
                let delivery = if streaming {
                    match stream::Delivery::admit(&state.streams, &backend.streams) {
                        Ok(owner) => Some(owner),
                        Err(_) => {
                            return error(
                                StatusCode::TOO_MANY_REQUESTS,
                                "stream delivery capacity reached",
                                request_id,
                            )
                        }
                    }
                } else {
                    None
                };
                let (sender, waiter) = tokio::sync::oneshot::channel();
                // Detach before durable admission: HTTP disconnect cannot discard an
                // admitted execution owner, a DB operation or its capacity permits.
                tokio::spawn(async move {
                    if !execute(
                        state.clone(),
                        p,
                        control,
                        id,
                        body,
                        request_id,
                        deadline,
                        sender,
                        delivery,
                    )
                    .await
                    {
                        // Observing an unknown outcome is not proof of backend idleness.
                        // Keep both reservations until an operator reconciles and restarts.
                        state
                            .reserved_turns
                            .fetch_add(1, std::sync::atomic::Ordering::Release);
                        global.forget();
                        permit.forget();
                    }
                });
                waiter.await.unwrap_or_else(|_| {
                    error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "admission unavailable",
                        request_id,
                    )
                })
            } else {
                error(
                    StatusCode::NOT_FOUND,
                    "request identity unavailable",
                    request_id,
                )
            }
        }
    }
}

async fn execute(
    state: Arc<State>,
    p: Principal,
    control: Arc<Control>,
    id: Uuid,
    body: Submission,
    request_id: Uuid,
    deadline: Instant,
    sender: tokio::sync::oneshot::Sender<Response>,
    delivery: Option<Arc<stream::Delivery>>,
) -> bool {
    let backend = &state.backends[&p.backend_id];
    // Old identities never enter this new-admission path. Their original body and
    // receipt remain lookup-only even after the operator withdraws a file grant.
    if body.enabled_tools.iter().any(|tool| tool == "file_read")
        && !backend.file_read.as_ref().is_some_and(|permission| {
            body.file_read_policy_sha256.as_deref() == Some(permission.policy_sha256.as_str())
        })
    {
        let _ = sender.send(error(
            StatusCode::BAD_REQUEST,
            "tenant file permission disabled or changed",
            request_id,
        ));
        return true;
    }
    if body.enabled_tools.iter().any(|tool| tool == "file_write")
        && !backend.file_write.as_ref().is_some_and(|permission| {
            body.file_write_policy_sha256.as_deref() == Some(permission.policy_sha256.as_str())
        })
    {
        let _ = sender.send(error(
            StatusCode::BAD_REQUEST,
            "tenant file overwrite permission disabled or changed",
            request_id,
        ));
        return true;
    }
    if body.enabled_tools.iter().any(|tool| tool == "skill_read")
        && !backend.skill_read.as_ref().is_some_and(|permission| {
            body.skill_read_policy_sha256.as_deref() == Some(permission.policy_sha256.as_str())
        })
    {
        let _ = sender.send(error(
            StatusCode::BAD_REQUEST,
            "tenant skill permission disabled or changed",
            request_id,
        ));
        return true;
    }
    if body
        .enabled_tools
        .iter()
        .any(|tool| tool == "memory_search")
        && !backend.memory_search.as_ref().is_some_and(|permission| {
            body.memory_search_policy_sha256.as_deref() == Some(permission.policy_sha256.as_str())
        })
    {
        let _ = sender.send(error(
            StatusCode::BAD_REQUEST,
            "tenant memory permission disabled or changed",
            request_id,
        ));
        return true;
    }
    if !matches!(
        tokio::time::timeout_at(
            deadline,
            check_backend(
                &state.client,
                &backend.url,
                &backend.token,
                &p.backend_id,
                backend.file_read.as_ref(),
                backend.file_write.as_ref(),
                backend.skill_read.as_ref(),
                backend.memory_search.as_ref(),
                backend.file_download
            )
        )
        .await,
        Ok(Ok(true))
    ) {
        let _ = sender.send(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend admission unavailable",
            request_id,
        ));
        return true;
    }
    let Ok(hash) = body.fingerprint() else {
        let _ = sender.send(error(
            StatusCode::BAD_REQUEST,
            "invalid HTTP submission",
            request_id,
        ));
        return true;
    };
    let principal = p.clone();
    let s = body.session_id.clone();
    let h = hash.clone();
    let admission = state.admission.clone();
    // Do not timeout/drop the actual blocking admission owner.
    let admitted = db(state.registry.clone(), control, move |r| {
        admission
            .admit(|| r.admit_http_turn(&principal, id, &s, &h))
            .unwrap_or_else(|| Err(anyhow::anyhow!("gateway shutting down")))
    })
    .await;
    match admitted {
        Err(e) => {
            let _ = sender.send(admission_error(&e, request_id));
            return true;
        }
        Ok(false) => {
            let result = tokio::time::timeout_at(
                deadline,
                remote(
                    &state,
                    &p,
                    id,
                    &hash,
                    &body.session_id,
                    Method::GET,
                    None,
                    false,
                ),
            )
            .await;
            let r = match result {
                Ok(Ok((status, envelope))) => response((status, Json(envelope)), request_id),
                _ => error(
                    StatusCode::CONFLICT,
                    "original receipt unavailable; review required",
                    request_id,
                ),
            };
            let _ = sender.send(r);
            return true;
        }
        Ok(true) => {}
    }
    if let Some(delivery) = delivery {
        return stream::execute(
            state, p, id, body, hash, request_id, deadline, sender, delivery,
        )
        .await;
    }
    let mut success = false;
    let mut idle = Instant::now() >= deadline; // No backend PUT can have been sent yet.
    if Instant::now() < deadline {
        match tokio::time::timeout_at(
            deadline,
            remote(
                &state,
                &p,
                id,
                &hash,
                &body.session_id,
                Method::PUT,
                Some(&body),
                false,
            ),
        )
        .await
        {
            Ok(Ok((status, mut envelope))) => {
                let _ = sender.send(response((status, Json(&envelope)), request_id));
                loop {
                    if !envelope.active && envelope.receipt.state != "running" {
                        idle = true;
                        success = envelope.known_success();
                        break;
                    }
                    if tokio::time::timeout_at(
                        deadline,
                        tokio::time::sleep(Duration::from_millis(100)),
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                    match tokio::time::timeout_at(
                        deadline,
                        remote(
                            &state,
                            &p,
                            id,
                            &hash,
                            &body.session_id,
                            Method::GET,
                            None,
                            false,
                        ),
                    )
                    .await
                    {
                        Ok(Ok((_, next))) => envelope = next,
                        _ => break,
                    }
                }
            }
            _ => {
                let _ = sender.send(error(
                    StatusCode::CONFLICT,
                    "original outcome unknown; review required",
                    request_id,
                ));
            }
        }
    } else {
        idle = true;
        let _ = sender.send(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "admission deadline exceeded; review required",
            request_id,
        ));
    }
    let registry = state.registry.clone();
    if !matches!(
        tokio::task::spawn_blocking(move || registry.finish_write(p.user_id, id, success)).await,
        Ok(Ok(()))
    ) {
        tracing::warn!("tenant HTTP write settlement failed; durable hold retained");
    }
    idle
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn abandoned_control_waiter_keeps_actual_registry_capacity() {
        let root =
            std::env::temp_dir().join(format!("jiaclaw-tenant-http-control-{}", Uuid::new_v4()));
        let registry = Registry::open(&root.join("users.sqlite3")).unwrap();
        let pool = Arc::new(tokio::sync::Semaphore::new(1));
        let owner = Arc::new(Control {
            _global: pool.clone().try_acquire_owned().unwrap(),
            _backend: None,
        });
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let waiter = tokio::spawn(db(registry, owner, move |r| {
            let _ = started_tx.send(());
            release_rx.recv_timeout(Duration::from_secs(5))?;
            r.add_user("alice")?;
            Ok(())
        }));
        started_rx.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert_eq!(pool.available_permits(), 0);
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while pool.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            Registry::open(&root.join("users.sqlite3"))
                .unwrap()
                .list()
                .unwrap()
                .len(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn routes_accept_only_explicit_stream_put_and_exclude_admin_ambiguous_queries() {
        let id = Uuid::new_v4();
        assert!(
            matches!(route(&Method::PUT, &format!("/api/turns/{id}/stream").parse().unwrap()), Some(Route::Stream(found)) if found == id)
        );
        for path in [
            format!("/api/turns/{id}/stream"),
            format!("/api/turns/{id}/review"),
            format!("/api/turns/{id}?backend=alice"),
            "/api/turns?limit=0".into(),
            "/api/turns?limit=1&limit=2".into(),
            "/api/turns?offset=10001".into(),
            format!("/api/turns/{}", id.to_string().to_uppercase()),
        ] {
            assert!(
                route(&Method::GET, &path.parse().unwrap()).is_none(),
                "{path}"
            );
        }
        assert!(matches!(
            route(
                &Method::GET,
                &"/api/turns?limit=50&offset=10000".parse().unwrap()
            ),
            Some(Route::Catalog(50, 10000))
        ));
        assert!(route(&Method::PUT, &format!("/api/turns/{id}").parse().unwrap()).is_some());
    }
}
