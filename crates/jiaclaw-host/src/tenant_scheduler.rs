// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Backend control protocol for gateway-authorized scheduled execution.

use super::{
    jobs::{dispatch_issued_ms, GatewayDispatchReceipt},
    scheduler, with_sessions, AppError, AppState,
};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    if !state.agent.config().scheduler.gateway_driven {
        return Err(AppError::NotFound);
    }
    let mut authorization = headers.get_all(header::AUTHORIZATION).iter();
    if authorization.next().is_none_or(|value| {
        value.to_str().is_err() || !value.to_str().unwrap_or_default().starts_with("Bearer ")
    }) || authorization.next().is_some()
        || headers.contains_key("x-api-token")
    {
        return Err(AppError::Unauthorized);
    }
    scheduler::authorize(state, headers)
}

pub(super) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    authorize(&state, &headers)?;
    let permit = state
        .tenant_control_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::ServiceUnavailable)?;
    let ready = state.scheduler_health.load(Ordering::Acquire) == 1;
    let due = with_sessions(&state, move |store| {
        let _permit = permit;
        store.gateway_job_due(scheduler::now_ms())
    })
    .await?;
    Ok(Json(
        json!({"protocol":1,"mode":"gateway","backend_id":state.agent.config().name,"ready":ready,"due":ready && due,"max_run_seconds":120}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DispatchRequest {
    request_id: String,
}

pub(super) async fn operation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Result<Json<GatewayDispatchReceipt>, AppError> {
    authorize(&state, &headers)?;
    dispatch_issued_ms(&request_id)
        .map_err(|_| AppError::BadRequest("canonical UUIDv7 operation ID required".into()))?;
    let permit = state
        .tenant_control_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::ServiceUnavailable)?;
    let receipt = with_sessions(&state, move |store| {
        let _permit = permit;
        store.gateway_dispatch(&request_id)
    })
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(receipt))
}

pub(super) async fn dispatch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DispatchRequest>,
) -> Result<Json<GatewayDispatchReceipt>, AppError> {
    authorize(&state, &headers)?;
    scheduler::accepting(&state)?;
    dispatch_issued_ms(&request.request_id)
        .map_err(|_| AppError::BadRequest("canonical UUIDv7 dispatch ID required".into()))?;
    let permit = state
        .tenant_dispatch_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::JobConflict("scheduled dispatch already executing".into()))?;
    // A disconnected gateway cannot cancel the worker or release its only
    // execution slot before completion. The durable association precedes work.
    tokio::spawn(async move {
        let _permit = permit;
        let id = request.request_id;
        let claim_id = id.clone();
        let health = state.scheduler_health.clone();
        let dispatch = scheduler::jobs_db(&state, move |store| {
            if health.load(Ordering::Acquire) != 1 {
                anyhow::bail!("scheduler is stopping");
            }
            store.claim_gateway_dispatch(&claim_id, scheduler::now_ms())
        })
        .await?;
        if let Some(run) = dispatch.claimed {
            if scheduler::execute(state.clone(), run).await.is_err() {
                state.scheduler_health.store(2, Ordering::Release);
                let _ =
                    with_sessions(&state, |store| store.recover_jobs(scheduler::now_ms())).await;
                return Err(AppError::ServiceUnavailable);
            }
        } else if dispatch
            .receipt
            .run
            .as_ref()
            .is_some_and(|run| run.status == "running")
        {
            return Err(AppError::JobConflict(
                "dispatch is unresolved; use the read-only operation endpoint".into(),
            ));
        }
        let receipt = with_sessions(&state, move |store| store.gateway_dispatch(&id))
            .await?
            .ok_or_else(|| AppError::Internal("durable dispatch receipt is missing".into()))?;
        if receipt
            .run
            .as_ref()
            .is_some_and(|run| run.status == "running")
        {
            return Err(AppError::ServiceUnavailable);
        }
        Ok(Json(receipt))
    })
    .await
    .map_err(|_| AppError::ServiceUnavailable)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{jobs::JobSpec, schedule::ScheduleSpec, store::SessionStore};
    use std::{
        path::Path as FilePath,
        sync::{Arc, Mutex},
        time::Duration,
    };

    fn fixture() -> (std::path::PathBuf, AppState) {
        let workspace =
            std::env::temp_dir().join(format!("jiaclaw-tenant-dispatch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut state = crate::tests::test_state_for_workspace(workspace.clone());
        let mut config = state.agent.config().clone();
        config.scheduler.enabled = true;
        config.scheduler.gateway_driven = true;
        state.agent = Arc::new(jiaclaw::JiaClawAgent::new(config).unwrap());
        state.api_token = Some("tenant-backend-only".into());
        state.persist_enabled = true;
        state.sessions = Arc::new(Mutex::new(
            SessionStore::open(FilePath::new(":memory:")).unwrap(),
        ));
        state.scheduler_health.store(1, Ordering::Release);
        (workspace, state)
    }
    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer tenant-backend-only".parse().unwrap(),
        );
        headers
    }
    fn job() -> JobSpec {
        JobSpec {
            name: "isolated schedule".into(),
            prompt: "Call datetime_now and report the current time.".into(),
            schedule: ScheduleSpec::Interval { seconds: 60 },
            enabled_tools: vec!["datetime_now".into()],
            timeout_secs: 10,
            delivery: None,
        }
    }

    #[tokio::test]
    async fn dispatch_authorization_and_control_reads_are_separate_from_execution() {
        let (workspace, state) = fixture();
        assert!(matches!(
            status(State(state.clone()), HeaderMap::new()).await,
            Err(AppError::Unauthorized)
        ));
        let execution = state
            .tenant_dispatch_permit
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        let response = status(State(state.clone()), headers()).await.unwrap().0;
        assert_eq!(response["protocol"], 1);
        assert_eq!(response["mode"], "gateway");
        assert_eq!(response["ready"], true);
        assert_eq!(response["max_run_seconds"], 120);
        assert_eq!(
            scheduler::status(State(state.clone()), headers())
                .await
                .unwrap()
                .0["max_concurrent_runs"],
            1
        );
        let control = state
            .tenant_control_permits
            .clone()
            .acquire_many_owned(4)
            .await
            .unwrap();
        assert!(matches!(
            status(State(state.clone()), headers()).await,
            Err(AppError::ServiceUnavailable)
        ));
        drop(control);
        let mut ambiguous = headers();
        ambiguous.append(
            header::AUTHORIZATION,
            "Bearer tenant-backend-only".parse().unwrap(),
        );
        assert!(matches!(
            status(State(state.clone()), ambiguous).await,
            Err(AppError::Unauthorized)
        ));
        drop(execution);
        drop(state);
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn disconnect_keeps_worker_permit_and_finishes_the_bound_run_without_replay() {
        let (workspace, state) = fixture();
        let job = state
            .sessions
            .lock()
            .unwrap()
            .create_job(job(), scheduler::now_ms() - 60_000)
            .unwrap();
        let turn = crate::session_turn_lock(&state, &job.session_id).await;
        let request_id = uuid::Uuid::now_v7().to_string();
        let sent_id = request_id.clone();
        let sent_state = state.clone();
        let caller = tokio::spawn(async move {
            dispatch(
                State(sent_state),
                headers(),
                Json(DispatchRequest {
                    request_id: sent_id,
                }),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state
                    .sessions
                    .lock()
                    .unwrap()
                    .gateway_dispatch(&request_id)
                    .unwrap()
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert_eq!(state.tenant_dispatch_permit.available_permits(), 0);
        let running = operation(State(state.clone()), headers(), Path(request_id.clone()))
            .await
            .unwrap()
            .0;
        assert_eq!(running.run.as_ref().unwrap().status, "running");
        assert!(matches!(
            dispatch(
                State(state.clone()),
                headers(),
                Json(DispatchRequest {
                    request_id: uuid::Uuid::now_v7().to_string()
                })
            )
            .await,
            Err(AppError::JobConflict(_))
        ));
        drop(turn);
        tokio::time::timeout(Duration::from_secs(5), async {
            while state.tenant_dispatch_permit.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let complete = operation(State(state.clone()), headers(), Path(request_id.clone()))
            .await
            .unwrap()
            .0;
        assert_eq!(complete.run.as_ref().unwrap().status, "completed");
        let repeated = dispatch(
            State(state.clone()),
            headers(),
            Json(DispatchRequest { request_id }),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(complete, repeated);
        assert_eq!(
            state
                .sessions
                .lock()
                .unwrap()
                .list_job_runs(&job.id, 5, 0)
                .unwrap()
                .len(),
            1
        );
        drop(state);
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[tokio::test]
    async fn gateway_mode_never_runs_autonomously_and_standalone_assertion_is_rejected() {
        let (workspace, state) = fixture();
        let scheduler = scheduler::start(state.clone()).await.unwrap().unwrap();
        let job = state
            .sessions
            .lock()
            .unwrap()
            .create_job(job(), scheduler::now_ms() - 60_000)
            .unwrap();
        tokio::time::sleep(Duration::from_millis(650)).await;
        assert!(state
            .sessions
            .lock()
            .unwrap()
            .list_job_runs(&job.id, 5, 0)
            .unwrap()
            .is_empty());
        assert_eq!(
            status(State(state.clone()), headers()).await.unwrap().0["due"],
            true
        );
        scheduler.shutdown(&state, Duration::from_secs(1)).await;
        let mut standalone = state.clone();
        let mut config = standalone.agent.config().clone();
        config.scheduler.gateway_driven = false;
        standalone.agent = Arc::new(jiaclaw::JiaClawAgent::new(config).unwrap());
        let mut asserted = headers();
        asserted.insert("x-jiaclaw-gateway-scheduler", "1".parse().unwrap());
        assert!(matches!(
            scheduler::authorize(&standalone, &asserted),
            Err(AppError::BadRequest(_))
        ));
        drop(standalone);
        drop(state);
        std::fs::remove_dir_all(workspace).unwrap();
    }
}
