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
use jiaclaw_core::{ChatMessage, ChatRequest, ChatResponse, MessageRole, RunStatus};
use serde::Deserialize;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
    time::Instant,
};

const MAX_CONCURRENCY: usize = 4;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
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

fn accepting(state: &AppState) -> Result<(), AppError> {
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
    Ok(Json(
        serde_json::json!({"state": status, "max_concurrent_runs": MAX_CONCURRENCY}),
    ))
}

async fn jobs_db<T: Send + 'static>(
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
    spec.validate()
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
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pagination {
    #[serde(default = "page_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    include_deleted: bool,
}
fn page_limit() -> usize {
    50
}
impl Pagination {
    fn validate(&self) -> Result<(), AppError> {
        if !(1..=100).contains(&self.limit) || self.offset > 10_000 {
            return Err(AppError::BadRequest(
                "pagination requires limit 1..100 and offset 0..10000".into(),
            ));
        }
        Ok(())
    }
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
    Ok((StatusCode::CREATED, Json(job)))
}

pub(super) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Pagination>,
) -> Result<Json<Vec<Job>>, AppError> {
    authorize(&state, &headers)?;
    page.validate()?;
    Ok(Json(
        with_sessions(&state, move |store| {
            if page.include_deleted {
                store.list_jobs_including_deleted(page.limit, page.offset, true)
            } else {
                store.list_jobs(page.limit, page.offset)
            }
        })
        .await?,
    ))
}

pub(super) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Job>, AppError> {
    authorize(&state, &headers)?;
    Ok(Json(
        with_sessions(&state, move |store| store.get_job(&id))
            .await?
            .ok_or(AppError::NotFound)?,
    ))
}

pub(super) async fn runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(page): Query<Pagination>,
) -> Result<Json<Vec<JobRun>>, AppError> {
    authorize(&state, &headers)?;
    page.validate()?;
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
    Ok(Json(
        with_sessions(&state, move |store| {
            store.list_job_runs(&id, page.limit, page.offset)
        })
        .await?,
    ))
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
    Ok(Json(
        jobs_db(&state, move |store| {
            store.set_job_enabled(&id, enabled, now_ms())
        })
        .await?
        .ok_or(AppError::NotFound)?,
    ))
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
    if !state.agent.config().scheduler.enabled {
        return Ok(None);
    }
    with_sessions(&state, |store| store.recover_jobs(now_ms()))
        .await
        .map_err(|_| anyhow::anyhow!("scheduler recovery failed"))?;
    state.scheduler_health.store(1, Ordering::Release);
    let (stop, mut shutdown) = watch::channel(false);
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

async fn execute(state: AppState, run: JobRun) -> Result<(), AppError> {
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
    let outcome = tokio::time::timeout_at(deadline, state.agent.chat(&request)).await;
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
