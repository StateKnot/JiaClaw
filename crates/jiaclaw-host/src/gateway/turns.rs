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
    protocol: u8,
    gateway_protocol: u8,
    agent_name: String,
    enabled: bool,
    streaming: bool,
    max_active: usize,
    session_prefix: String,
    max_receipt_bytes: usize,
}

pub(super) async fn check_backend(
    client: &reqwest::Client,
    url: &reqwest::Url,
    token: &HeaderValue,
    identity: &str,
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
        let bytes = super::proxy::read_response(response, 4096)
            .await
            .map_err(|()| anyhow::anyhow!("invalid backend HTTP turn handshake"))?;
        let c: Capability = serde_json::from_slice(&bytes)?;
        ensure!(
            c.protocol == 1
                && c.gateway_protocol == 1
                && c.agent_name == identity
                && !c.streaming
                && c.max_active == 1
                && c.session_prefix == "http:"
                && c.max_receipt_bytes == MAX_RECEIPT,
            "backend HTTP turn contract mismatch"
        );
        Ok(c.enabled)
    })
    .await
    .map_err(|_| anyhow::anyhow!("backend HTTP turn handshake timed out"))?
}

enum Route {
    Capabilities,
    Catalog(usize, usize),
    Turn(Uuid),
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
    if uri.path() == "/api/turns/capabilities" && method == Method::GET {
        return Some(Route::Capabilities);
    }
    let tail = uri.path().strip_prefix("/api/turns/")?;
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
        && (1..=2).contains(&s.enabled_tools.len())
        && s.enabled_skills.is_empty()
        && s.enabled_tools
            .iter()
            .all(|t| matches!(t.as_str(), "datetime_now" | "json_query"))
        && (s.enabled_tools.len() == 1 || s.enabled_tools[0] != s.enabled_tools[1])
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
        Route::Capabilities => response(
            Json(
                json!({"protocol":1,"gateway_protocol":1,"enabled":true,"streaming":false,"listing":true,"scope":"gateway","session_prefix":"http:","max_identities":10000,"max_page":50,"enabled_tools":["datetime_now","json_query"]}),
            ),
            request_id,
        ),
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
        Route::Turn(id) | Route::Cancel(id) => {
            let cancel = matches!(route, Route::Cancel(_));
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
                    || request
                        .headers()
                        .get_all(header::ACCEPT)
                        .iter()
                        .any(|v| v.to_str().is_ok_and(|v| v.contains("text/event-stream")))
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
) -> bool {
    let backend = &state.backends[&p.backend_id];
    if !matches!(
        tokio::time::timeout_at(
            deadline,
            check_backend(&state.client, &backend.url, &backend.token, &p.backend_id)
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
    fn routes_exclude_stream_admin_ambiguous_queries_and_noncanonical_ids() {
        let id = Uuid::new_v4();
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
