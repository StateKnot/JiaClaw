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
fn allowed(method: &Method, uri: &Uri) -> bool {
    let path = uri.path();
    let basic = match path {
        "/api/chat" | "/api/sessions/import" => method == Method::POST,
        "/api/sessions" => method == Method::GET || method == Method::POST,
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
    if !allowed(request.method(), request.uri()) {
        return error(StatusCode::NOT_FOUND, "route unavailable", request_id);
    }
    let Some(token) = bearer(request.headers()) else {
        return error(StatusCode::UNAUTHORIZED, "invalid API key", request_id);
    };
    let Ok(global_permit) = state.permits.clone().try_acquire_owned() else {
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
    let Some(backend) = state.backends.get(&principal.backend_id) else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend unavailable",
            request_id,
        );
    };
    let Ok(user_permit) = backend.permit.clone().try_acquire_owned() else {
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
    let bytes = read_response(response, MAX_RESPONSE).await?;
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
    let settled = if method == Method::GET {
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
            "/api/jobs",
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
    fn authentication_header_is_unambiguous() {
        let mut headers = HeaderMap::new();
        let (key, _) = super::super::keys::issue(Uuid::new_v4()).unwrap();
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
