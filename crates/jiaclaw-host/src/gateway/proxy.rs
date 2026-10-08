// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
use super::{
    registry::{Principal, WriteAdmissionError},
    State,
};
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State as ExtractState},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use std::{collections::HashSet, sync::Arc, time::Duration};
use uuid::Uuid;
const MAX_REQUEST: usize = 512 * 1024;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;

fn session_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value != "."
        && value != ".."
        && value != "import"
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}
fn canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}
fn job_path(path: &str) -> Option<(&str, &str)> {
    let tail = path.strip_prefix("/api/jobs/")?;
    let (id, action) = tail.split_once('/').unwrap_or((tail, ""));
    (canonical_uuid(id) && matches!(action, "" | "runs" | "pause" | "resume"))
        .then_some((id, action))
}
fn jobs_path(path: &str) -> bool {
    path == "/api/jobs" || path.starts_with("/api/jobs/")
}
fn job_method(method: &Method, path: &str) -> bool {
    match job_path(path) {
        Some((_, "")) => method == Method::GET || method == Method::DELETE,
        Some((_, "runs")) => method == Method::GET,
        Some((_, "pause" | "resume")) => method == Method::POST,
        _ => false,
    }
}
fn decimal(value: &str, min: usize, max: usize) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value
            .parse::<usize>()
            .is_ok_and(|n| (min..=max).contains(&n))
}
fn allowed(method: &Method, uri: &Uri) -> bool {
    let path = uri.path();
    let basic = match path {
        "/api/gateway/capabilities" | "/api/jobs/status" => method == Method::GET,
        "/api/jobs" => method == Method::GET || method == Method::POST,
        "/api/chat" | "/api/sessions/import" => method == Method::POST,
        "/api/sessions" => method == Method::GET || method == Method::POST,
        _ if jobs_path(path) => job_method(method, path),
        _ => path.strip_prefix("/api/sessions/").is_some_and(|tail| {
            if let Some(id) = tail.strip_suffix("/export") {
                method == Method::GET && session_id(id)
            } else {
                session_id(tail) && (method == Method::GET || method == Method::DELETE)
            }
        }),
    };
    if !basic || uri.scheme().is_some() || uri.authority().is_some() {
        return false;
    }
    let Some(query) = uri.query() else {
        return true;
    };
    if query.is_empty() || query.len() > 256 {
        return false;
    }
    if jobs_path(path) {
        if method != Method::GET
            || !(path == "/api/jobs" || job_path(path).is_some_and(|(_, action)| action == "runs"))
        {
            return false;
        }
        let mut seen = HashSet::new();
        return query.split('&').all(|part| {
            let Some((key, value)) = part.split_once('=') else {
                return false;
            };
            seen.insert(key)
                && match key {
                    "limit" => decimal(value, 1, 5),
                    "offset" => decimal(value, 0, 10_000),
                    "include_deleted" => path == "/api/jobs" && matches!(value, "true" | "false"),
                    _ => false,
                }
        });
    }
    let import = path == "/api/sessions/import";
    if !import && !path.ends_with("/export") {
        return false;
    }
    let mut keys = HashSet::new();
    query.split('&').all(|part| {
        let Some((key, value)) = part.split_once('=') else {
            return false;
        };
        keys.insert(key)
            && match key {
                "format" => matches!(value, "json" | "jsonl"),
                "id" => import && session_id(value),
                "overwrite" => import && matches!(value, "true" | "false"),
                _ => false,
            }
    })
}
fn bearer(headers: &HeaderMap) -> Option<String> {
    super::keys::authorization_token(headers).map(str::to_owned)
}

fn error(status: StatusCode, message: &'static str, request_id: Uuid) -> Response {
    let response = (status, axum::Json(serde_json::json!({"error":message}))).into_response();
    finish(response, request_id)
}
fn finish(mut response: Response, request_id: Uuid) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&request_id.to_string()).expect("UUID header"),
    );
    response
}

pub(super) async fn handle(
    ExtractState(state): ExtractState<Arc<State>>,
    request: Request,
) -> Response {
    let request_id = Uuid::new_v4();
    if !allowed(request.method(), request.uri())
        || (jobs_path(request.uri().path()) && !state.scheduled_jobs)
    {
        return error(StatusCode::NOT_FOUND, "route unavailable", request_id);
    }
    let Some(token) = bearer(request.headers()) else {
        return error(StatusCode::UNAUTHORIZED, "invalid API key", request_id);
    };
    let is_control = request.method() == Method::GET
        && (jobs_path(request.uri().path()) || request.uri().path() == "/api/gateway/capabilities");
    let pool = if is_control {
        &state.control
    } else {
        &state.permits
    };
    let Ok(global_permit) = pool.clone().try_acquire_owned() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "gateway capacity reached",
            request_id,
        );
    };
    let registry = state.registry.clone();
    let (global_permit, principal) = match tokio::task::spawn_blocking(move || {
        let result = registry.authenticate(&token);
        (global_permit, result)
    })
    .await
    {
        Ok((permit, Ok(Some(principal)))) => (permit, principal),
        Ok((_, Ok(None))) => return error(StatusCode::UNAUTHORIZED, "invalid API key", request_id),
        _ => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "authentication unavailable",
                request_id,
            )
        }
    };
    // Key permissions are authority from the private registry, never request
    // metadata. Reject before reading a body, taking backend capacity or
    // persisting a hold; denied requests must not reach a tenant backend.
    if principal.read_only && request.method() != Method::GET {
        return error(StatusCode::FORBIDDEN, "read-only API key", request_id);
    }
    let Some(backend) = state.backends.get(&principal.backend_id) else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend unavailable",
            request_id,
        );
    };
    let pool = if is_control {
        &backend.control
    } else {
        &backend.permit
    };
    let Ok(user_permit) = pool.clone().try_acquire_owned() else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "user request already in progress",
            request_id,
        );
    };
    let (parts, body) = request.into_parts();
    let body =
        match tokio::time::timeout(Duration::from_secs(10), to_bytes(body, MAX_REQUEST)).await {
            Ok(Ok(body)) => body,
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
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .unwrap_or("application/json");
    if !matches!(content_type, "application/json" | "application/x-ndjson") {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported request content type",
            request_id,
        );
    }
    if parts.method == Method::GET && !body.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "GET body is unsupported",
            request_id,
        );
    }
    if parts.uri.path() == "/api/gateway/capabilities" {
        return finish(
            axum::Json(serde_json::json!({"scheduled_jobs":state.scheduled_jobs,"read_only":principal.read_only})).into_response(),
            request_id,
        );
    }
    if jobs_path(parts.uri.path()) && parts.method != Method::GET {
        let valid = if parts.uri.path() == "/api/jobs" && parts.method == Method::POST {
            serde_json::from_slice::<crate::jobs::JobSpec>(&body)
                .is_ok_and(|spec| spec.validate_gateway().is_ok())
        } else {
            body.is_empty()
                || serde_json::from_slice::<serde_json::Value>(&body)
                    .is_ok_and(|value| value.as_object().is_some_and(serde_json::Map::is_empty))
        };
        if !valid || content_type != "application/json" {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid or unauthorized scheduled job request",
                request_id,
            );
        }
    }
    if parts.uri.path() == "/api/chat" {
        let parsed = serde_json::from_slice::<crate::ChatHttpBody>(&body);
        let valid = parsed.as_ref().ok().is_some_and(|value| {
            value.request.session_id.as_deref().is_some_and(session_id)
                && !value.stream
                && !value.request.messages.is_empty()
        });
        if !valid
            || content_type != "application/json"
            || parts
                .headers
                .get_all(header::ACCEPT)
                .iter()
                .any(|v| v.to_str().is_ok_and(|s| s.contains("text/event-stream")))
        {
            return error(
                StatusCode::BAD_REQUEST,
                "JSON chat requires an explicit session_id and stream=false",
                request_id,
            );
        }
    }
    if parts.uri.path() == "/api/sessions/import" {
        let json = content_type == "application/json"
            || parts
                .uri
                .query()
                .is_some_and(|q| q.split('&').any(|p| p == "format=json"));
        let valid = std::str::from_utf8(&body).ok().is_some_and(|text| {
            if json {
                crate::parse_session_import_json(text)
                    .is_ok_and(|(id, _)| id.as_deref().is_none_or(session_id))
            } else {
                crate::parse_session_import_jsonl(text).is_ok()
            }
        });
        if !valid {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid session import body or ID",
                request_id,
            );
        }
    }
    let is_write = parts.method != Method::GET;
    let content_type = content_type.to_owned();
    // The task owns admission, backend IO and both permits. Dropping the client
    // future cannot release a durable hold or admit another request prematurely.
    let task = tokio::spawn(async move {
        let _global_permit = global_permit;
        let _user_permit = user_permit;
        if is_write {
            let registry = state.registry.clone();
            let principal = principal.clone();
            match tokio::task::spawn_blocking(move || registry.admit_write(&principal, request_id))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(cause)) => {
                    return match cause.downcast_ref::<WriteAdmissionError>() {
                        Some(WriteAdmissionError::Unauthorized) => {
                            error(StatusCode::UNAUTHORIZED, "invalid API key", request_id)
                        }
                        Some(WriteAdmissionError::ReadOnly) => {
                            error(StatusCode::FORBIDDEN, "read-only API key", request_id)
                        }
                        Some(WriteAdmissionError::Held) => error(
                            StatusCode::CONFLICT,
                            "previous write requires administrator review",
                            request_id,
                        ),
                        None => error(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "write admission unavailable",
                            request_id,
                        ),
                    }
                }
                Err(_) => {
                    return error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "write admission unavailable",
                        request_id,
                    )
                }
            }
        }
        let result = tokio::time::timeout(
            state.timeout,
            forward(
                &state,
                &principal,
                parts.method,
                parts.uri,
                body,
                &content_type,
                request_id,
            ),
        )
        .await;
        let (response, success) = match result {
            Ok(Ok(outcome)) => outcome,
            _ => (
                error(
                    StatusCode::BAD_GATEWAY,
                    "backend result is unknown; administrator review required for writes",
                    request_id,
                ),
                false,
            ),
        };
        if is_write {
            let registry = state.registry.clone();
            if !matches!(
                tokio::task::spawn_blocking(move || registry.finish_write(
                    principal.user_id,
                    request_id,
                    success
                ))
                .await,
                Ok(Ok(()))
            ) {
                return error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "write receipt could not be committed; administrator review required",
                    request_id,
                );
            }
        }
        response
    });
    task.await.unwrap_or_else(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "request interrupted; administrator review required for writes",
            request_id,
        )
    })
}

async fn forward(
    state: &State,
    principal: &Principal,
    method: Method,
    uri: Uri,
    body: axum::body::Bytes,
    content_type: &str,
    request_id: Uuid,
) -> Result<(Response, bool), ()> {
    let backend = state.backends.get(&principal.backend_id).ok_or(())?;
    let mut endpoint = backend.url.clone();
    endpoint.set_path(uri.path());
    endpoint.set_query(uri.query());
    let mut request = state
        .client
        .request(method.clone(), endpoint)
        .header(header::AUTHORIZATION, backend.token.clone())
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT, "application/json")
        .header("x-request-id", request_id.to_string());
    if jobs_path(uri.path()) {
        // A backend restarted in autonomous mode must reject this request before
        // mutating a job, even when its API token remains valid.
        request = request.header("x-jiaclaw-gateway-scheduler", "1");
    }
    let requested_job = if uri.path() == "/api/jobs" && method == Method::POST {
        let spec: crate::jobs::JobSpec = serde_json::from_slice(&body).map_err(|_| ())?;
        Some(serde_json::to_value(spec).map_err(|_| ())?)
    } else {
        None
    };
    let requested_session = match uri.path() {
        "/api/chat" => serde_json::from_slice::<crate::ChatHttpBody>(&body)
            .ok()
            .and_then(|r| r.request.session_id),
        "/api/sessions/import" => {
            // Match the backend's precedence: explicit query ID, then JSON body ID.
            uri.query()
                .and_then(|query| query.split('&').find_map(|part| part.strip_prefix("id=")))
                .map(str::to_owned)
                .or_else(|| {
                    serde_json::from_slice::<crate::ImportSessionJsonBody>(&body)
                        .ok()
                        .and_then(|r| r.id)
                })
        }
        _ => None,
    };
    if !body.is_empty() {
        request = request.body(body);
    }
    let response = request.send().await.map_err(|_| ())?;
    let status = response.status();
    if method == Method::DELETE
        && job_path(uri.path()).is_some_and(|(_, action)| action.is_empty())
        && status == StatusCode::NO_CONTENT
    {
        if !read_response(response, 0).await?.is_empty() {
            return Err(());
        }
        return Ok((
            finish(StatusCode::NO_CONTENT.into_response(), request_id),
            true,
        ));
    }
    let mime = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .unwrap_or("");
    let mime = match mime {
        "application/json" => "application/json",
        "application/x-ndjson" => "application/x-ndjson",
        _ => return Err(()),
    };
    let bytes = read_response(
        response,
        if jobs_path(uri.path()) {
            1024 * 1024
        } else {
            MAX_RESPONSE
        },
    )
    .await?;
    // Never propagate backend errors, redirects, cookies, or private response headers.
    if !status.is_success() {
        return Ok((
            error(
                if status.is_client_error() {
                    status
                } else {
                    StatusCode::BAD_GATEWAY
                },
                "backend rejected request; writes require administrator review",
                request_id,
            ),
            false,
        ));
    }
    if mime == "application/json" {
        serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|_| ())?;
    } else {
        for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
            serde_json::from_slice::<serde_json::Value>(line).map_err(|_| ())?;
        }
    }
    let settled = if jobs_path(uri.path()) {
        validate_job_response(&method, &uri, &bytes, requested_job.as_ref())?
    } else if method == Method::GET {
        true
    } else {
        if mime != "application/json" {
            return Err(());
        }
        known_completion(&method, &uri, &bytes, requested_session.as_deref())?
    };
    let mut response = finish(
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, mime)
            .body(Body::from(bytes))
            .map_err(|_| ())?,
        request_id,
    );
    if !settled {
        response.headers_mut().insert(
            "x-jiaclaw-write-review",
            HeaderValue::from_static("required"),
        );
    }
    Ok((response, settled))
}
fn validate_job(job: &crate::jobs::Job) -> Result<(), ()> {
    if !canonical_uuid(&job.id)
        || job.session_id != format!("job:{}", job.id)
        || job.spec.validate().is_err()
    {
        return Err(());
    }
    Ok(())
}
fn validate_job_response(
    method: &Method,
    uri: &Uri,
    bytes: &[u8],
    requested: Option<&serde_json::Value>,
) -> Result<bool, ()> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    if uri.path() == "/api/jobs/status" && method == Method::GET {
        return matches!(
            value.get("state").and_then(serde_json::Value::as_str),
            Some("running" | "failed" | "stopping" | "disabled")
        )
        .then_some(true)
        .ok_or(());
    }
    if method == Method::GET
        && (uri.path() == "/api/jobs"
            || job_path(uri.path()).is_some_and(|(_, action)| action == "runs"))
    {
        let items = value
            .get("items")
            .and_then(serde_json::Value::as_array)
            .ok_or(())?;
        if items.len() > 5 || value.get("next_offset").is_none() {
            return Err(());
        }
        let offset = uri
            .query()
            .and_then(|q| q.split('&').find_map(|p| p.strip_prefix("offset=")))
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| ())?;
        if !value["next_offset"].is_null()
            && (items.is_empty()
                || value["next_offset"].as_u64() != Some((offset + items.len()) as u64)
                || offset + items.len() > 10_000)
        {
            return Err(());
        }
        for item in items {
            if uri.path() == "/api/jobs" {
                validate_job(&serde_json::from_value(item.clone()).map_err(|_| ())?)?;
            } else {
                let run: crate::jobs::JobRun =
                    serde_json::from_value(item.clone()).map_err(|_| ())?;
                if !canonical_uuid(&run.id)
                    || Some(run.job_id.as_str()) != job_path(uri.path()).map(|(id, _)| id)
                    || run.session_id != format!("job:{}", run.job_id)
                    || run.spec.validate().is_err()
                    || !matches!(
                        run.status.as_str(),
                        "running"
                            | "completed"
                            | "failed"
                            | "interrupted"
                            | "needs_review"
                            | "skipped"
                    )
                {
                    return Err(());
                }
            }
        }
        return Ok(true);
    }
    let job: crate::jobs::Job = serde_json::from_value(value).map_err(|_| ())?;
    validate_job(&job)?;
    if uri.path() == "/api/jobs" && method == Method::POST {
        job.spec.validate_gateway().map_err(|_| ())?;
        return Ok(!job.deleted
            && job.enabled
            && requested == Some(&serde_json::to_value(&job.spec).map_err(|_| ())?));
    }
    let (id, action) = job_path(uri.path()).ok_or(())?;
    if job.id != id {
        return Err(());
    }
    match (method, action) {
        (&Method::GET, "") => Ok(true),
        (&Method::POST, "pause") => Ok(!job.deleted && !job.enabled),
        (&Method::POST, "resume") => {
            Ok(!job.deleted && job.enabled && job.spec.validate_gateway().is_ok())
        }
        _ => Err(()),
    }
}
fn known_completion(
    method: &Method,
    uri: &Uri,
    bytes: &[u8],
    requested_session: Option<&str>,
) -> Result<bool, ()> {
    match uri.path() {
        "/api/chat" => {
            let response: jiaclaw_core::ChatResponse =
                serde_json::from_slice(bytes).map_err(|_| ())?;
            if response.session_id.as_deref() != requested_session {
                return Err(());
            }
            Ok(response.status == jiaclaw_core::RunStatus::Completed
                && response.tool_calls.iter().all(|call| {
                    call.result
                        .as_ref()
                        .is_some_and(|value| value.get("error").is_none())
                }))
        }
        "/api/sessions" => {
            let response: crate::CreateSessionResponse =
                serde_json::from_slice(bytes).map_err(|_| ())?;
            if !session_id(&response.session_id) {
                return Err(());
            }
            Ok(true)
        }
        "/api/sessions/import" => {
            let response: crate::GetSessionResponse =
                serde_json::from_slice(bytes).map_err(|_| ())?;
            if !session_id(&response.id) || requested_session.is_some_and(|id| response.id != id) {
                return Err(());
            }
            Ok(true)
        }
        _ if method == Method::DELETE => {
            let response: crate::DeleteSessionResponse =
                serde_json::from_slice(bytes).map_err(|_| ())?;
            Ok(response.success)
        }
        _ => Err(()),
    }
}
pub(super) async fn read_response(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, ()> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_authentication_retains_capacity_until_blocking_work_finishes() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let root =
                std::env::temp_dir().join(format!("jiaclaw-gateway-cancel-{}", Uuid::new_v4()));
            let registry =
                super::super::registry::Registry::open(&root.join("registry.db")).unwrap();
            let key = registry.add_user("alice").unwrap();
            let state = Arc::new(State {
                registry,
                backends: std::collections::HashMap::new(),
                client: reqwest::Client::new(),
                permits: Arc::new(tokio::sync::Semaphore::new(1)),
                timeout: Duration::from_secs(10),
                control: Arc::new(tokio::sync::Semaphore::new(8)),
                scheduled_jobs: false,
                telegram: None,
                slack: None,
                discord: None,
                feishu: None,
            });
            let (release, receiver) = std::sync::mpsc::channel();
            let (started, ready) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            ready.await.unwrap();
            let request = Request::builder()
                .uri("/api/sessions")
                .header(header::AUTHORIZATION, format!("Bearer {}", key.token))
                .body(Body::empty())
                .unwrap();
            let task = tokio::spawn(handle(ExtractState(Arc::clone(&state)), request));
            tokio::time::timeout(Duration::from_secs(2), async {
                while state.permits.available_permits() != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
            assert_eq!(
                state.permits.available_permits(),
                0,
                "queued authentication must still own capacity"
            );
            release.send(()).unwrap();
            blocker.await.unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while state.permits.available_permits() != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            std::fs::remove_dir_all(root).unwrap();
        });
    }
    #[tokio::test]
    async fn read_only_denial_precedes_body_backend_capacity_and_hold() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root =
            std::env::temp_dir().join(format!("jiaclaw-gateway-read-only-{}", Uuid::new_v4()));
        let registry = super::super::registry::Registry::open(&root.join("registry.db")).unwrap();
        let key = registry.add_user_with_access("alice", true).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().fallback(move || {
                    let observed = observed.clone();
                    async move {
                        observed.fetch_add(1, Ordering::SeqCst);
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                }),
            )
            .await
            .unwrap();
        });
        // An unavailable backend must not mask the permission rejection.
        let permit = Arc::new(tokio::sync::Semaphore::new(1));
        let occupied = permit.clone().acquire_owned().await.unwrap();
        let backend = super::super::Backend {
            url: format!("http://{address}").parse().unwrap(),
            token: HeaderValue::from_static("Bearer private-backend"),
            permit,
            control: Arc::new(tokio::sync::Semaphore::new(2)),
        };
        let state = Arc::new(State {
            registry,
            backends: std::collections::HashMap::from([("alice".into(), backend)]),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            permits: Arc::new(tokio::sync::Semaphore::new(1)),
            timeout: Duration::from_secs(10),
            control: Arc::new(tokio::sync::Semaphore::new(8)),
            scheduled_jobs: true,
            telegram: None,
            slack: None,
            discord: None,
            feishu: None,
        });
        let id = Uuid::new_v4();
        for (method, path) in [
            (Method::POST, "/api/chat".into()),
            (Method::POST, "/api/sessions".into()),
            (Method::POST, "/api/sessions/import".into()),
            (Method::DELETE, "/api/sessions/same".into()),
            (Method::POST, "/api/jobs".into()),
            (Method::POST, format!("/api/jobs/{id}/pause")),
            (Method::POST, format!("/api/jobs/{id}/resume")),
            (Method::DELETE, format!("/api/jobs/{id}")),
        ] {
            let body = Body::from_stream(futures_util::stream::pending::<
                Result<axum::body::Bytes, std::io::Error>,
            >());
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {}", key.token))
                .header("x-jiaclaw-read-only", "false")
                .body(body)
                .unwrap();
            let response = tokio::time::timeout(
                Duration::from_secs(2),
                handle(ExtractState(state.clone()), request),
            )
            .await
            .expect("permission denial must not poll an unfinished body");
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(state.registry.list().unwrap()[0].hold.is_none());
        }
        let request = Request::builder()
            .uri("/api/gateway/capabilities")
            .header(header::AUTHORIZATION, format!("Bearer {}", key.token))
            .body(Body::empty())
            .unwrap();
        let response = handle(ExtractState(state.clone()), request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"scheduled_jobs":true,"read_only":true})
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // Full keys retain their permission, and metadata never needs backend IO.
        let full = state.registry.add_key(key.user_id).unwrap();
        let request = Request::builder()
            .uri("/api/gateway/capabilities")
            .header(header::AUTHORIZATION, format!("Bearer {}", full.token))
            .body(Body::empty())
            .unwrap();
        let response = handle(ExtractState(state.clone()), request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"scheduled_jobs":true,"read_only":false})
        );
        // Permission failures apply only to actual exposed operations. Unknown
        // routes retain the same unavailable result without authentication/body IO.
        let request = Request::builder()
            .method(Method::PUT)
            .uri(format!("/api/jobs/{id}"))
            .header(header::AUTHORIZATION, format!("Bearer {}", key.token))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            handle(ExtractState(state.clone()), request).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(occupied);
        server.abort();
        let _ = server.await;
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn routes_are_canonical_and_explicit() {
        for path in [
            "/api/sessions/a",
            "/api/sessions/a/export?format=json",
            "/api/sessions",
        ] {
            assert!(allowed(&Method::GET, &path.parse().unwrap()));
        }
        assert!(allowed(
            &Method::POST,
            &"/api/sessions/import?id=a&format=json&overwrite=false"
                .parse()
                .unwrap()
        ));
        for path in [
            "/api/jobs?limit=6",
            "/metrics",
            "/api/sessions/%2e%2e",
            "/api/sessions/..",
            "/api/sessions/a%2fb",
            "/api/sessions/a/b",
            "/api/sessions/a?tenant=x",
            "/api/sessions/a/export?format=json&format=json",
            "/api/sessions/a/export?format=%6ason",
            "/api/sessions/a/export?overwrite=true",
            "http://attacker/api/sessions",
        ] {
            assert!(!allowed(&Method::GET, &path.parse().unwrap()), "{path}");
        }
    }
    #[test]
    fn standalone_job_creation_identity_is_never_forwarded_by_the_gateway() {
        let id = "abcdef01-2345-4678-9abc-def012345678";
        for path in [
            "/api/jobs".to_owned(),
            format!("/api/jobs/{id}"),
            format!("/api/jobs/{id}/resume"),
        ] {
            assert!(!allowed(&Method::PUT, &path.parse().unwrap()));
        }
    }
    #[test]
    fn outbox_audit_routes_are_never_forwarded_to_tenant_backends() {
        let id = "abcdef01-2345-4678-9abc-def012345678";
        for path in [
            "/api/channels/deliveries".to_owned(),
            format!("/api/channels/deliveries/{id}"),
            format!("/api/channels/deliveries/{id}/resolve"),
        ] {
            for method in [Method::GET, Method::HEAD, Method::POST, Method::DELETE] {
                assert!(!allowed(&method, &path.parse().unwrap()), "{method} {path}");
            }
        }
    }
    #[test]
    fn tenant_job_routes_only_expose_canonical_bounded_public_operations() {
        let id = "abcdef01-2345-4678-9abc-def012345678".to_owned();
        for (method, path) in [
            (Method::GET, "/api/gateway/capabilities".into()),
            (
                Method::GET,
                "/api/jobs?limit=5&offset=10&include_deleted=true".into(),
            ),
            (Method::POST, "/api/jobs".into()),
            (Method::GET, format!("/api/jobs/{id}/runs?limit=1&offset=0")),
            (Method::POST, format!("/api/jobs/{id}/pause")),
            (Method::POST, format!("/api/jobs/{id}/resume")),
            (Method::DELETE, format!("/api/jobs/{id}")),
        ] {
            assert!(allowed(&method, &path.parse().unwrap()), "{path}");
        }
        for (method, path) in [
            (Method::POST, "/internal/scheduler/dispatch".into()),
            (Method::GET, "/internal/scheduler/status".into()),
            (Method::GET, "/api/jobs?limit=0".into()),
            (Method::GET, "/api/jobs?offset=10001".into()),
            (Method::GET, "/api/jobs?limit=1&limit=2".into()),
            (Method::GET, "/api/jobs?tenant=bob".into()),
            (
                Method::GET,
                format!("/api/jobs/{id}/runs?include_deleted=true"),
            ),
            (Method::DELETE, format!("/api/jobs/{id}?purge=true")),
            (Method::POST, format!("/api/jobs/{id}/pause?anything=1")),
            (Method::GET, format!("/api/jobs/{}/runs", id.to_uppercase())),
            (Method::GET, "/api/jobs/%2e%2e".into()),
            (Method::GET, "/api/sessions/job%3A123".into()),
            (Method::POST, "/api/sessions/import?id=job:123".into()),
        ] {
            assert!(!allowed(&method, &path.parse().unwrap()), "{path}");
        }
    }
    #[test]
    fn job_receipts_bind_target_state_spec_and_pagination() {
        let id = Uuid::new_v4().to_string();
        let spec = serde_json::json!({"name":"fixture","prompt":"Report time","schedule":{"kind":"interval","seconds":60},"enabled_tools":["datetime_now"],"timeout_secs":120});
        let mut job = serde_json::json!({"id":id,"spec":spec,"enabled":true,"deleted":false,"created_ms":1,"next_due_ms":60001,"session_id":format!("job:{id}")});
        let valid = |method: Method,
                     path: &str,
                     value: &serde_json::Value,
                     expected: Option<&serde_json::Value>| {
            validate_job_response(
                &method,
                &path.parse().unwrap(),
                &serde_json::to_vec(value).unwrap(),
                expected,
            )
        };
        assert_eq!(
            valid(Method::POST, "/api/jobs", &job, Some(&spec)),
            Ok(true)
        );
        job["spec"]["prompt"] = serde_json::json!("wrong operation");
        assert_ne!(
            valid(Method::POST, "/api/jobs", &job, Some(&spec)),
            Ok(true)
        );
        job["spec"] = spec;
        let pause = format!("/api/jobs/{id}/pause");
        assert_eq!(valid(Method::POST, &pause, &job, None), Ok(false));
        job["enabled"] = serde_json::json!(false);
        assert_eq!(valid(Method::POST, &pause, &job, None), Ok(true));
        let page = serde_json::json!({"items":[job.clone()],"next_offset":1});
        assert_eq!(valid(Method::GET, "/api/jobs", &page, None), Ok(true));
        assert!(valid(Method::GET, "/api/jobs?offset=5", &page, None).is_err());
        assert!(valid(
            Method::GET,
            "/api/jobs",
            &serde_json::json!({"items":[],"next_offset":1}),
            None
        )
        .is_err());
        job["session_id"] = serde_json::json!("ordinary-chat");
        assert!(valid(Method::POST, &pause, &job, None).is_err());
    }
    #[test]
    fn authentication_header_is_unambiguous() {
        let mut headers = HeaderMap::new();
        let (key, _) = super::super::keys::issue(Uuid::new_v4(), false).unwrap();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", key.token)).unwrap(),
        );
        assert_eq!(bearer(&headers).as_deref(), Some(key.token.as_str()));
        headers.insert("x-api-token", HeaderValue::from_static("token"));
        assert!(bearer(&headers).is_none());
        headers.remove("x-api-token");
        headers.append(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer token"),
        );
        assert!(bearer(&headers).is_none());
    }
}
