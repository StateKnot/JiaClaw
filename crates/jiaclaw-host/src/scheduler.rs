// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Authenticated persistent scheduling with explicit uncertain-outcome handling.
use super::jobs::{Job, JobRun, JobSpec};
use super::{
    check_api_auth, prepare_session_chat_messages, session_turn_lock, with_sessions, AppError,
    AppState, SessionRecord,
};
use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use jiaclaw_core::{ChatMessage, ChatRequest, ChatResponse, MessageRole, ModelPurpose, RunStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::Instant,
};

const MAX_CONCURRENCY: usize = 4;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

pub(super) fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

pub(super) fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    if headers.contains_key("x-jiaclaw-gateway-scheduler") {
        let mut values = headers.get_all("x-jiaclaw-gateway-scheduler").iter();
        if values.next().is_none_or(|value| value != "1")
            || values.next().is_some()
            || !state.agent.config().scheduler.gateway_driven
        {
            return Err(AppError::BadRequest(
                "gateway scheduler mode assertion failed".into(),
            ));
        }
    }
    if !state.agent.config().scheduler.enabled {
        return Err(AppError::NotFound);
    }
    if state.api_token.is_none() || !check_api_auth(state, headers) {
        return Err(AppError::Unauthorized);
    }
    if !state.persist_enabled {
        return Err(AppError::Internal(
            "scheduler requires SQLite persistence".into(),
        ));
    }
    Ok(())
}

pub(super) fn accepting(state: &AppState) -> Result<(), AppError> {
    if state.scheduler_health.load(Ordering::Acquire) != 1 {
        return Err(AppError::ServiceUnavailable);
    }
    Ok(())
}

pub(super) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    authorize(&state, &headers)?;
    let status = match state.scheduler_health.load(Ordering::Acquire) {
        1 => "running",
        2 => "failed",
        3 => "stopping",
        _ => "disabled",
    };
    let maximum = if state.agent.config().scheduler.gateway_driven {
        1
    } else {
        MAX_CONCURRENCY
    };
    let mut response = serde_json::json!({"state": status, "max_concurrent_runs": maximum});
    if !state.agent.config().scheduler.gateway_driven {
        response["create_identity_protocol"] = "job-id-v1".into();
    }
    Ok(Json(response))
}

pub(super) async fn jobs_db<T: Send + 'static>(
    state: &AppState,
    operation: impl FnOnce(&mut super::store::SessionStore) -> Result<T> + Send + 'static,
) -> Result<T, AppError> {
    with_sessions(state, move |store| match operation(store) {
        Ok(value) => Ok(Ok(value)),
        Err(error) if error.downcast_ref::<super::jobs::JobConflict>().is_some() => {
            Ok(Err(error.to_string()))
        }
        Err(error) => Err(error),
    })
    .await?
    .map_err(AppError::JobConflict)
}

// Background execution only admits adapters with verified cancellation/resource bounds.
// A request cannot turn an unbounded legacy tool into an unattended capability.
fn validate_tools(state: &AppState, spec: &JobSpec) -> Result<(), AppError> {
    if state.agent.config().scheduler.gateway_driven {
        spec.validate_gateway()
    } else {
        spec.validate()
    }
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    for name in &spec.enabled_tools {
        if state.agent.tools().get(name).is_none() {
            return Err(AppError::BadRequest(format!(
                "unregistered scheduled tool: {name}"
            )));
        }
        if !matches!(
            name.as_str(),
            "datetime_now" | "json_query" | "exec" | "shell_exec"
        ) && !name.starts_with("mcp_")
        {
            return Err(AppError::BadRequest(format!(
                "tool has no verified background cancellation contract: {name}"
            )));
        }
    }
    super::channels::validate_scheduled_job(state, spec)
}

const MAX_GATEWAY_PAGE_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pagination {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    include_deleted: bool,
}
impl Pagination {
    fn validate(&self, gateway: bool) -> Result<usize, AppError> {
        let limit = self.limit.unwrap_or(if gateway { 5 } else { 50 });
        let maximum = if gateway { 5 } else { 100 };
        if !(1..=maximum).contains(&limit) || self.offset > 10_000 {
            return Err(AppError::BadRequest(format!(
                "pagination requires limit 1..{maximum} and offset 0..10000"
            )));
        }
        Ok(limit)
    }
}

fn bounded_single<T: Serialize>(value: T, gateway: bool) -> Result<T, AppError> {
    if gateway
        && serde_json::to_vec(&value).map_or(true, |bytes| bytes.len() > MAX_GATEWAY_PAGE_BYTES)
    {
        return Err(AppError::BadRequest(
            "stored item exceeds the gateway response byte budget".into(),
        ));
    }
    Ok(value)
}

fn page_response<T: Serialize>(
    items: Vec<T>,
    limit: usize,
    offset: usize,
    gateway: bool,
) -> Result<Value, AppError> {
    if !gateway {
        return serde_json::to_value(items)
            .map_err(|_| AppError::Internal("job serialization failed".into()));
    }
    let available = items.len();
    let mut output = Vec::new();
    let mut bytes = 64; // Reserve envelope, separators and bounded numeric cursor.
    for item in items.into_iter().take(limit) {
        let item = serde_json::to_value(item)
            .map_err(|_| AppError::Internal("job serialization failed".into()))?;
        let size = serde_json::to_vec(&item)
            .map_err(|_| AppError::Internal("job serialization failed".into()))?
            .len();
        if bytes + size + 1 > MAX_GATEWAY_PAGE_BYTES {
            if output.is_empty() {
                return Err(AppError::BadRequest(
                    "stored item exceeds the gateway response byte budget; item was not skipped"
                        .into(),
                ));
            }
            break;
        }
        bytes += size + 1;
        output.push(item);
    }
    let next = (output.len() < available).then_some(offset + output.len());
    let result = serde_json::json!({"items":output, "next_offset":next});
    bounded_single(result, true)
}

pub(super) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(spec): Json<JobSpec>,
) -> Result<(StatusCode, Json<Job>), AppError> {
    authorize(&state, &headers)?;
    accepting(&state)?;
    validate_tools(&state, &spec)?;
    let job = jobs_db(&state, move |store| store.create_job(spec, now_ms())).await?;
    Ok((
        StatusCode::CREATED,
        Json(bounded_single(
            job,
            state.agent.config().scheduler.gateway_driven,
        )?),
    ))
}

/// Standalone administrator create-only operation with a durable client identity.
pub(super) async fn create_with_id(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(spec): Json<JobSpec>,
) -> Result<(StatusCode, Json<Job>), AppError> {
    authorize(&state, &headers)?;
    if state.agent.config().scheduler.gateway_driven {
        return Err(AppError::NotFound);
    }
    if !super::jobs::valid_creation_id(&id) {
        return Err(AppError::BadRequest(
            "creation ID must be a canonical UUIDv4".into(),
        ));
    }
    let lookup_id = id.clone();
    let lookup_spec = spec.clone();
    if let Some(job) = jobs_db(&state, move |store| {
        store.get_job_creation(&lookup_id, &lookup_spec)
    })
    .await?
    {
        return Ok((StatusCode::OK, Json(job)));
    }
    accepting(&state)?;
    validate_tools(&state, &spec)?;
    let (job, created) = jobs_db(&state, move |store| {
        store.create_job_with_id(&id, spec, now_ms())
    })
    .await?;
    Ok((
        if created {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(job),
    ))
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Pagination>,
) -> Result<Json<Value>, AppError> {
    authorize(&state, &headers)?;
    let gateway = state.agent.config().scheduler.gateway_driven;
    let limit = page.validate(gateway)?;
    let fetch = limit + usize::from(gateway);
    let offset = page.offset;
    let items = with_sessions(&state, move |store| {
        if page.include_deleted {
            store.list_jobs_including_deleted(fetch, page.offset, true)
        } else {
            store.list_jobs(fetch, page.offset)
        }
    })
    .await?;
    Ok(Json(page_response(items, limit, offset, gateway)?))
}

pub(super) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Job>, AppError> {
    authorize(&state, &headers)?;
    let job = with_sessions(&state, move |store| store.get_job(&id))
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(bounded_single(
        job,
        state.agent.config().scheduler.gateway_driven,
    )?))
}

pub(super) async fn runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(page): Query<Pagination>,
) -> Result<Json<Value>, AppError> {
    authorize(&state, &headers)?;
    let gateway = state.agent.config().scheduler.gateway_driven;
    let limit = page.validate(gateway)?;
    if page.include_deleted {
        return Err(AppError::BadRequest(
            "include_deleted applies only to the job list".into(),
        ));
    }
    let lookup = id.clone();
    if with_sessions(&state, move |store| store.get_job(&lookup))
        .await?
        .is_none()
    {
        return Err(AppError::NotFound);
    }
    let fetch = limit + usize::from(gateway);
    let offset = page.offset;
    let items = with_sessions(&state, move |store| {
        store.list_job_runs(&id, fetch, page.offset)
    })
    .await?;
    Ok(Json(page_response(items, limit, offset, gateway)?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeleteOptions {
    #[serde(default)]
    purge: bool,
}

pub(super) async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(options): Query<DeleteOptions>,
) -> Result<StatusCode, AppError> {
    authorize(&state, &headers)?;
    let found = jobs_db(&state, move |store| {
        if options.purge {
            store.purge_job(&id)
        } else {
            store.delete_job(&id)
        }
    })
    .await?;
    if !found {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn set_enabled(
    state: AppState,
    headers: HeaderMap,
    id: String,
    enabled: bool,
) -> Result<Json<Job>, AppError> {
    authorize(&state, &headers)?;
    if enabled {
        accepting(&state)?;
        let lookup = id.clone();
        let job = with_sessions(&state, move |store| store.get_job(&lookup))
            .await?
            .ok_or(AppError::NotFound)?;
        validate_tools(&state, &job.spec)?;
    }
    let job = jobs_db(&state, move |store| {
        store.set_job_enabled(&id, enabled, now_ms())
    })
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(bounded_single(
        job,
        state.agent.config().scheduler.gateway_driven,
    )?))
}
pub(super) async fn pause(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Job>, AppError> {
    set_enabled(state, headers, id, false).await
}
pub(super) async fn resume(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Job>, AppError> {
    set_enabled(state, headers, id, true).await
}

pub(super) struct Scheduler {
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

impl Scheduler {
    pub(super) async fn shutdown(mut self, state: &AppState, grace: Duration) {
        state.scheduler_health.store(3, Ordering::Release);
        let _ = self.stop.send(true);
        if tokio::time::timeout(grace, &mut self.handle).await.is_err() {
            self.handle.abort();
            let _ = self.handle.await; // Dropping the supervisor aborts its owned JoinSet.
        }
        // A transaction racing cancellation either committed first, or can no longer
        // finish its now-interrupted run. Never publish success after uncertainty.
        if with_sessions(state, |store| store.recover_jobs(now_ms()))
            .await
            .is_err()
        {
            tracing::error!(
                "scheduler shutdown could not persist interruption; startup recovery required"
            );
        }
    }
}

pub(super) async fn start(state: AppState) -> Result<Option<Scheduler>> {
    if state.persist_enabled {
        with_sessions(&state, |store| store.recover_jobs(now_ms()))
            .await
            .map_err(|_| anyhow::anyhow!("scheduler recovery failed"))?;
    }
    if !state.agent.config().scheduler.enabled {
        return Ok(None);
    }
    state.scheduler_health.store(1, Ordering::Release);
    let (stop, mut shutdown) = watch::channel(false);
    if state.agent.config().scheduler.gateway_driven {
        let handle = tokio::spawn(async move {
            let _ = shutdown.changed().await;
            // No autonomous tick in this mode. Existing dispatch owns the permit
            // until execution and the durable receipt have actually settled.
            let _drain = state.tenant_dispatch_permit.acquire().await;
        });
        return Ok(Some(Scheduler { stop, handle }));
    }
    let handle = tokio::spawn(async move {
        let mut active = JoinSet::new();
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut fault = false;
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                result = active.join_next(), if !active.is_empty() => {
                    if !matches!(result, Some(Ok(Ok(())))) {
                        tracing::error!("scheduler worker failed; stop admission and require outcome review");
                        fault = true;
                        break;
                    }
                }
                _ = tick.tick(), if active.len() < MAX_CONCURRENCY => {
                    let capacity = MAX_CONCURRENCY - active.len();
                    let health = state.scheduler_health.clone();
                    let claimed = with_sessions(&state, move |store| {
                        if health.load(Ordering::Acquire) != 1 { return Ok(vec![]); }
                        store.claim_due_jobs(now_ms(), capacity)
                    }).await;
                    match claimed {
                        Ok(runs) => for run in runs {
                            let worker_state = state.clone();
                            active.spawn(async move { execute(worker_state, run).await });
                        },
                        Err(_) => { fault = true; break; }
                    }
                }
            }
        }
        if fault {
            state.scheduler_health.store(2, Ordering::Release);
            active.abort_all();
        }
        while active.join_next().await.is_some() {}
        if fault {
            let _ = with_sessions(&state, |store| store.recover_jobs(now_ms())).await;
            tracing::error!(
                "scheduler stopped after storage/worker failure; repair and restart serve"
            );
        }
    });
    Ok(Some(Scheduler { stop, handle }))
}

fn notice(message: String) -> ChatResponse {
    ChatResponse {
        message: ChatMessage {
            role: MessageRole::Assistant,
            content: message,
        },
        tool_calls: vec![],
        status: RunStatus::RequiresHumanInput,
        session_id: None,
        routing: None,
    }
}

async fn finish(
    state: &AppState,
    run: &JobRun,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    messages: Option<Vec<ChatMessage>>,
    status: &str,
    response: Option<ChatResponse>,
    error: Option<String>,
) -> Result<(), AppError> {
    let id = run.id.clone();
    let session = messages.map(|messages| (run.session_id.clone(), SessionRecord::new(messages)));
    let status = status.to_owned();
    let completed = with_sessions(state, move |store| {
        // The SQLite closure survives cancellation of the awaiting future. Keep
        // the session turn lock until that closure commits or rejects the run.
        let _guard = guard;
        store.finish_job_run(&id, session, &status, response, error, now_ms())
    })
    .await?;
    if !completed {
        return Err(AppError::Internal(
            "scheduled run no longer owns completion".into(),
        ));
    }
    Ok(())
}

pub(super) async fn execute(state: AppState, run: JobRun) -> Result<(), AppError> {
    let deadline = Instant::now() + Duration::from_secs(run.spec.timeout_secs);
    if validate_tools(&state, &run.spec).is_err() {
        return finish(
            &state,
            &run,
            None,
            None,
            "failed",
            None,
            Some("scheduled tool/configuration is no longer authorized".into()),
        )
        .await;
    }
    // Include lock wait, compaction, model/tool turns in the same monotonic deadline.
    let Ok(guard) =
        tokio::time::timeout_at(deadline, session_turn_lock(&state, &run.session_id)).await
    else {
        return finish(
            &state,
            &run,
            None,
            None,
            "interrupted",
            None,
            Some("run deadline expired while waiting for its session".into()),
        )
        .await;
    };
    let incoming = vec![ChatMessage {
        role: MessageRole::User,
        content: run.spec.prompt.clone(),
    }];
    let prepared = tokio::time::timeout_at(
        deadline,
        prepare_session_chat_messages(&state, &run.session_id, incoming, &run.id, "scheduler"),
    )
    .await;
    let mut messages = match prepared {
        Ok(Ok(messages)) => messages,
        _ => return finish(
            &state,
            &run,
            Some(guard),
            None,
            "interrupted",
            None,
            Some(
                "session preparation failed or exceeded the run deadline; inspect before resuming"
                    .into(),
            ),
        )
        .await,
    };
    let request = ChatRequest {
        messages: messages.clone(),
        enabled_tools: run.spec.enabled_tools.clone(),
        enabled_skills: vec![],
        auto_skills: false,
        session_id: Some(run.session_id.clone()),
    };
    let outcome = tokio::time::timeout_at(
        deadline,
        state.agent.chat_for(&request, ModelPurpose::Scheduled),
    )
    .await;
    let (mut response, mut status, mut error) = match outcome {
        Ok(Ok(response)) => {
            let status = if response.status == RunStatus::Completed { "completed" } else { "needs_review" };
            (response, status, None)
        }
        Ok(Err(_)) => (notice("定时任务失败并暂停；模型或工具可能已经执行，请检查结果后手动恢复。".into()), "failed", Some("agent call failed; job paused without automatic replay".into())),
        Err(_) => (notice("定时任务超过执行期限并暂停；已取消等待，但不能据此认定外部操作未发生。请核查后手动恢复。".into()), "interrupted", Some("run deadline exceeded; external outcome may be unknown".into())),
    };
    if response.tool_calls.iter().any(|call| {
        call.result
            .as_ref()
            .is_some_and(|value| value.get("error").is_some())
    }) {
        status = "needs_review";
        response.status = RunStatus::RequiresHumanInput;
        error = Some("one or more tool calls failed; inspect before resuming".into());
    }
    if serde_json::to_vec(&response).map_or(true, |bytes| bytes.len() > MAX_RESPONSE_BYTES) {
        let names = response
            .tool_calls
            .iter()
            .map(|call| call.tool_name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        response = notice(format!("定时任务结果超过 256 KiB，详细内容未保存，任务已暂停。已返回的工具记录：{names}。请检查工作区或外部服务后再恢复；不要直接重放。"));
        status = "needs_review";
        error = Some("response exceeded storage limit; detailed results omitted".into());
    }
    if run.spec.delivery.is_some() && status == "completed" {
        if super::outbound::split_text(&response.message.content).is_err() {
            status = "needs_review";
            response.status = RunStatus::RequiresHumanInput;
            error = Some("scheduled response exceeds outbound bounds; no fragments queued".into());
        } else if !super::channels::scheduled_job_authorized(&state, &run.spec) {
            status = "needs_review";
            response.status = RunStatus::RequiresHumanInput;
            error = Some("scheduled delivery authorization changed; no fragments queued".into());
        }
    }
    response.session_id = Some(run.session_id.clone());
    messages.push(response.message.clone());
    finish(
        &state,
        &run,
        Some(guard),
        Some(messages),
        status,
        Some(response),
        error,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{schedule::ScheduleSpec, store::SessionStore};
    use std::sync::Arc;

    #[test]
    fn gateway_pages_count_encoded_bytes_and_never_skip_unreturned_items() {
        let values = vec![
            serde_json::json!({"value":"\0".repeat(100_000)}),
            serde_json::json!({"value":"\0".repeat(100_000)}),
        ];
        let first = page_response(values.clone(), 5, 0, true).unwrap();
        assert_eq!(first["items"].as_array().unwrap().len(), 1);
        assert_eq!(first["next_offset"], 1);
        assert!(serde_json::to_vec(&first).unwrap().len() <= MAX_GATEWAY_PAGE_BYTES);
        let second = page_response(values[1..].to_vec(), 5, 1, true).unwrap();
        assert_eq!(second["items"].as_array().unwrap().len(), 1);
        assert!(second["next_offset"].is_null());
        let too_large = vec![serde_json::json!({"value":"\0".repeat(200_000)})];
        assert!(page_response(too_large, 5, 0, true).is_err());
        assert!(page_response(values, 50, 0, false).unwrap().is_array());
    }

    #[test]
    fn pagination_preserves_legacy_defaults_and_bounds_gateway_rows() {
        let defaults: Pagination = serde_json::from_str("{}").unwrap();
        assert_eq!(defaults.validate(false).unwrap(), 50);
        assert_eq!(defaults.validate(true).unwrap(), 5);
        let larger: Pagination = serde_json::from_str(r#"{"limit":6}"#).unwrap();
        assert!(larger.validate(true).is_err());
        assert_eq!(larger.validate(false).unwrap(), 6);
        assert!(serde_json::from_str::<Pagination>(r#"{"backend":"other"}"#).is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn job_creation_identity_survives_cancelled_admitted_database_waiter() {
        let workspace =
            std::env::temp_dir().join(format!("jiaclaw-create-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut state = crate::tests::test_state_for_workspace(workspace.clone());
        state.sessions = Arc::new(std::sync::Mutex::new(
            SessionStore::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let id = uuid::Uuid::new_v4().to_string();
        let spec = JobSpec {
            name: "cancelled waiter".into(),
            prompt: "Use the clock".into(),
            schedule: ScheduleSpec::Interval { seconds: 60 },
            enabled_tools: vec!["datetime_now".into()],
            timeout_secs: 120,
            delivery: None,
        };
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (worker_state, worker_id, worker_spec) = (state.clone(), id.clone(), spec.clone());
        let waiter = tokio::spawn(async move {
            jobs_db(&worker_state, move |store| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                store.create_job_with_id(&worker_id, worker_spec, 0)
            })
            .await
        });
        entered_rx.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            state.sessions.try_lock().is_err(),
            "admitted transaction still owns the store after HTTP wait cancellation"
        );
        release_tx.send(()).unwrap();
        let (job, created) = tokio::time::timeout(
            Duration::from_secs(5),
            jobs_db(&state, move |store| {
                store.create_job_with_id(&id, spec, 90_000)
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!created);
        assert_eq!(job.created_ms, 0);
        assert_eq!(job.next_due_ms, 60_000);
        drop(state);
        std::fs::remove_dir_all(workspace).unwrap();
    }

    async fn cancellation_race(recover_before_release: bool) {
        let workspace =
            std::env::temp_dir().join(format!("jiaclaw-scheduler-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut state = crate::tests::test_state_for_workspace(workspace.clone());
        state.persist_enabled = true;
        state.sessions = Arc::new(std::sync::Mutex::new(
            SessionStore::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let original = ChatMessage {
            role: MessageRole::User,
            content: "committed before scheduled work".into(),
        };
        let run = {
            let mut store = state.sessions.lock().unwrap();
            let job = store
                .create_job(
                    JobSpec {
                        name: "cancellation race".into(),
                        prompt: "Report the current time.".into(),
                        schedule: ScheduleSpec::Interval { seconds: 60 },
                        enabled_tools: vec!["datetime_now".into()],
                        timeout_secs: 120,
                        delivery: None,
                    },
                    0,
                )
                .unwrap();
            store
                .insert(
                    job.session_id.clone(),
                    SessionRecord::new(vec![original.clone()]),
                )
                .unwrap();
            store.claim_due_jobs(60_000, 1).unwrap().remove(0)
        };
        let guard = session_turn_lock(&state, &run.session_id).await;

        // Hold the actual store mutex on a separate thread, so the real SQLite
        // completion must queue without blocking the Tokio executor itself.
        let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let locked_store = state.sessions.clone();
        let locker = std::thread::spawn(move || {
            let mut store = locked_store.lock().unwrap();
            locked_tx.send(()).unwrap();
            if release_rx.recv().unwrap_or(false) {
                assert_eq!(store.recover_jobs(60_001).unwrap(), 1);
            }
        });
        locked_rx.await.unwrap();

        let response = ChatResponse {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: "scheduled work committed after its waiter was cancelled".into(),
            },
            tool_calls: vec![],
            status: RunStatus::Completed,
            session_id: Some(run.session_id.clone()),
            routing: None,
        };
        let messages = vec![original.clone(), response.message.clone()];
        let finishing_state = state.clone();
        let finishing_run = run.clone();
        // No other task may clone sessions until this barrier passes. The extra
        // owner is with_sessions' dispatched blocking closure, queued on mutex.
        let dispatched_owners = Arc::strong_count(&state.sessions) + 1;
        let waiter = tokio::spawn(async move {
            finish(
                &finishing_state,
                &finishing_run,
                Some(guard),
                Some(messages),
                "completed",
                Some(response),
                None,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while Arc::strong_count(&state.sessions) < dispatched_owners {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("completion must reach the real blocking store operation");
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());

        // A following HTTP/session turn must not pass its lock and read old
        // history while the cancelled waiter's SQLite transaction can still win.
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                session_turn_lock(&state, &run.session_id),
            )
            .await
            .is_err(),
            "cancellation released the turn lock before storage resolved"
        );

        release_tx.send(recover_before_release).unwrap();
        locker.join().unwrap();
        let session_id = run.session_id.clone();
        let job_id = run.job_id.clone();
        let (history, outcome) = tokio::time::timeout(Duration::from_secs(5), async {
            let _next_turn = session_turn_lock(&state, &session_id).await;
            with_sessions(&state, move |store| {
                Ok((
                    store.get(&session_id)?.unwrap().messages,
                    store.list_job_runs(&job_id, 1, 0)?.remove(0),
                ))
            })
            .await
            .unwrap()
        })
        .await
        .expect("the next session turn must resume once SQLite resolves");

        if recover_before_release {
            assert_eq!(history, vec![original]);
            assert_eq!(outcome.status, "interrupted");
            assert!(outcome.response.is_none());
        } else {
            assert_eq!(history.len(), 2);
            assert_eq!(history[0], original);
            assert_eq!(
                history[1].content,
                "scheduled work committed after its waiter was cancelled"
            );
            assert_eq!(outcome.status, "completed");
            assert_eq!(outcome.response.unwrap().message, history[1]);
        }
        drop(state);
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_completion_waiter_keeps_turn_locked_until_sqlite_commits() {
        cancellation_race(false).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_completion_cannot_overwrite_history_after_recovery_wins() {
        cancellation_race(true).await;
    }
}
