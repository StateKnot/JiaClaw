// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Gateway-owned cron admission. A worker never retries an uncertain dispatch.
use super::{proxy::read_response, registry::ScheduledUser, State};
use serde::Deserialize;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::Notify,
    task::{JoinHandle, JoinSet},
    time::MissedTickBehavior,
};
use uuid::Uuid;

const RESPONSE_LIMIT: usize = 4096;
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

struct StopState {
    stopped: AtomicBool,
    // Linearize stop against the short durable admission transaction. Once stop()
    // returns, only previously admitted requests may still reach a backend.
    admission: Mutex<()>,
    wake: Notify,
}
#[derive(Clone)]
pub(super) struct Stop(Arc<StopState>);
impl Stop {
    pub(super) fn new() -> Self {
        Self(Arc::new(StopState {
            stopped: AtomicBool::new(false),
            admission: Mutex::new(()),
            wake: Notify::new(),
        }))
    }
    pub(super) fn stop(&self) {
        let _guard = self
            .0
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.0.stopped.store(true, Ordering::Release);
        self.0.wake.notify_waiters();
    }
    pub(super) fn admit<T>(&self, work: impl FnOnce() -> T) -> Option<T> {
        let _guard = self
            .0
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_stopped() {
            None
        } else {
            Some(work())
        }
    }
    pub(super) fn is_stopped(&self) -> bool {
        self.0.stopped.load(Ordering::Acquire)
    }
}
pub(super) struct Scheduler {
    stop: Stop,
    task: Option<JoinHandle<()>>,
}
impl Scheduler {
    pub(super) fn stopper(&self) -> Stop {
        self.stop.clone()
    }
    pub(super) fn stop(&self) {
        self.stop.stop();
    }
    pub(super) async fn shutdown(mut self, grace: Duration) -> bool {
        self.stop();
        let Some(mut task) = self.task.take() else {
            return true;
        };
        // A timeout drops only the join handle, not the task. Admitted workers
        // retain execution permits until receipt persistence or runtime shutdown.
        matches!(tokio::time::timeout(grace, &mut task).await, Ok(Ok(())))
    }
}
impl Drop for Scheduler {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn start(state: Arc<State>) -> Scheduler {
    let stop = Stop::new();
    let task = state
        .scheduled_jobs
        .then(|| tokio::spawn(coordinate(state, stop.clone())));
    Scheduler { stop, task }
}
async fn coordinate(state: Arc<State>, stop: Stop) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut workers = JoinSet::new();
    let mut running = HashSet::new();
    let mut offset = 0_usize;
    loop {
        if stop.is_stopped() {
            break;
        }
        tokio::select! {
            _ = stop.0.wake.notified() => {},
            result = workers.join_next(), if !workers.is_empty() => {
                if let Some(Ok(id)) = result { running.remove(&id); }
                // Panic leaves the backend excluded until process restart. Any
                // admitted hold also survives; do not guess its outcome.
            },
            _ = ticker.tick() => {
                let Ok(control) = state.control.clone().try_acquire_owned() else { continue; };
                let registry = state.registry.clone();
                let users = tokio::task::spawn_blocking(move || {
                    let _control = control;
                    registry.scheduled_users()
                }).await;
                let Ok(Ok(mut users)) = users else { continue; };
                if stop.is_stopped() { break; }
                // Poll order rotates so the bounded control lane does not starve
                // tenants when there are more backends than available permits.
                if !users.is_empty() {
                    let length = users.len();
                    users.rotate_left(offset % length);
                    offset = offset.wrapping_add(1);
                }
                for user in users {
                    if running.len() >= 32 || stop.is_stopped() { break; }
                    if !state.backends.contains_key(&user.backend_id) || !running.insert(user.backend_id.clone()) { continue; }
                    let state = Arc::clone(&state); let stop = stop.clone();
                    workers.spawn(async move {
                        let id = user.backend_id.clone();
                        poll_backend(state, stop, user).await;
                        id
                    });
                }
            }
        }
    }
    // Never abort requests admitted before stop, including on a caller's drain
    // timeout. Runtime death is reconciled by Registry::recover_writes.
    while workers.join_next().await.is_some() {}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    protocol: u8,
    mode: String,
    backend_id: String,
    ready: bool,
    due: bool,
    max_run_seconds: u64,
}
pub(super) fn parse_status(bytes: &[u8], backend_id: &str) -> Result<bool, ()> {
    let status: Status = serde_json::from_slice(bytes).map_err(|_| ())?;
    if status.protocol != 1
        || status.mode != "gateway"
        || status.backend_id != backend_id
        || !status.ready
        || status.max_run_seconds != 120
    {
        return Err(());
    }
    Ok(status.due)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    request_id: String,
    run: serde_json::Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletedRun {
    id: String,
    job_id: String,
    status: String,
}
fn canonical_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn completed_receipt(bytes: &[u8], request_id: Uuid) -> bool {
    let Ok(receipt) = serde_json::from_slice::<Receipt>(bytes) else {
        return false;
    };
    if receipt.request_id != request_id.to_string() {
        return false;
    }
    if receipt.run.is_null() {
        return true;
    }
    let Ok(run) = serde_json::from_value::<CompletedRun>(receipt.run) else {
        return false;
    };
    run.status == "completed" && canonical_id(&run.id) && canonical_id(&run.job_id)
}
fn json_response(response: &reqwest::Response) -> bool {
    response.status() == reqwest::StatusCode::OK
        && response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}
async fn poll_backend(state: Arc<State>, stop: Stop, user: ScheduledUser) {
    let Some(backend) = state.backends.get(&user.backend_id) else {
        return;
    };
    let Ok(global_control) = state.control.clone().try_acquire_owned() else {
        return;
    };
    let Ok(backend_control) = backend.control.clone().try_acquire_owned() else {
        return;
    };
    if stop.is_stopped() {
        return;
    }
    let status = tokio::time::timeout(STATUS_TIMEOUT, async {
        let url = backend
            .url
            .join("internal/scheduler/status")
            .map_err(|_| ())?;
        let response = state
            .client
            .get(url)
            .header(reqwest::header::AUTHORIZATION, backend.token.clone())
            .timeout(STATUS_TIMEOUT)
            .send()
            .await
            .map_err(|_| ())?;
        if !json_response(&response) {
            return Err(());
        }
        let bytes = read_response(response, RESPONSE_LIMIT).await?;
        parse_status(&bytes, &user.backend_id)
    })
    .await;
    drop(backend_control);
    drop(global_control);
    if !matches!(status, Ok(Ok(true))) || stop.is_stopped() {
        return;
    }
    let Ok(global_execution) = state.permits.clone().try_acquire_owned() else {
        return;
    };
    let Ok(backend_execution) = backend.permit.clone().try_acquire_owned() else {
        return;
    };
    let registry = state.registry.clone();
    let owner = user.clone();
    let admission_stop = stop.clone();
    let request_id = Uuid::now_v7();
    // Move permits through blocking SQLite calls so cancellation cannot release
    // execution capacity while a transaction is still admitting/finalizing work.
    let admitted = tokio::task::spawn_blocking(move || {
        let _guard = admission_stop
            .0
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if admission_stop.is_stopped() {
            return None;
        }
        registry
            .admit_scheduled(owner.user_id, &owner.backend_id, request_id)
            .ok()?;
        Some((global_execution, backend_execution))
    })
    .await;
    let Ok(Some((global_execution, backend_execution))) = admitted else {
        return;
    };
    // There is exactly one POST. Even explicit backend errors may follow a
    // durable claim or model submission, and require administrator reconciliation.
    let outcome = tokio::time::timeout(state.timeout, async {
        let url = backend
            .url
            .join("internal/scheduler/dispatch")
            .map_err(|_| ())?;
        let response = state
            .client
            .post(url)
            .header(reqwest::header::AUTHORIZATION, backend.token.clone())
            .timeout(state.timeout)
            .json(&serde_json::json!({"request_id":request_id.to_string()}))
            .send()
            .await
            .map_err(|_| ())?;
        if !json_response(&response) {
            return Err(());
        }
        let bytes = read_response(response, RESPONSE_LIMIT).await?;
        Ok::<_, ()>(completed_receipt(&bytes, request_id))
    })
    .await;
    let known_success = matches!(outcome, Ok(Ok(true)));
    let registry = state.registry.clone();
    let finished = tokio::task::spawn_blocking(move || {
        let _permits = (global_execution, backend_execution);
        registry.finish_write(user.user_id, request_id, known_success)
    })
    .await;
    if !matches!(finished, Ok(Ok(()))) {
        tracing::warn!("scheduled dispatch receipt could not be committed; user remains held");
    }
}

#[cfg(test)]
mod tests {
    use super::super::{registry::Registry, Backend};
    use super::*;
    use axum::{
        extract::State as ExtractState,
        response::{IntoResponse, Response},
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::{collections::HashMap, path::PathBuf, sync::atomic::AtomicUsize};
    use tokio::sync::Semaphore;

    struct Fixture {
        root: PathBuf,
        registry: Registry,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("jiaclaw-gateway-cron-{}", Uuid::new_v4()));
            let registry = Registry::open(&root.join("registry.sqlite3")).unwrap();
            Self { root, registry }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    struct Fake {
        mode: &'static str,
        gets: AtomicUsize,
        posts: AtomicUsize,
        status_gate: Arc<Semaphore>,
        post_gate: Arc<Semaphore>,
    }
    async fn fake_status(
        ExtractState(fake): ExtractState<Arc<Fake>>,
        headers: axum::http::HeaderMap,
    ) -> Response {
        assert_eq!(headers["authorization"], "Bearer test-only-token");
        fake.gets.fetch_add(1, Ordering::SeqCst);
        let permit = fake.status_gate.acquire().await.unwrap();
        permit.forget();
        let mut body = json!({"protocol":1,"mode":"gateway","backend_id":"alice","ready":true,"due":true,"max_run_seconds":120});
        if fake.mode == "bad_status" {
            body["backend_id"] = json!("bob");
        }
        Json(body).into_response()
    }
    async fn fake_dispatch(
        ExtractState(fake): ExtractState<Arc<Fake>>,
        headers: axum::http::HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        assert_eq!(headers["authorization"], "Bearer test-only-token");
        assert_eq!(
            Uuid::parse_str(body["request_id"].as_str().unwrap())
                .unwrap()
                .get_version_num(),
            7
        );
        fake.posts.fetch_add(1, Ordering::SeqCst);
        let permit = fake.post_gate.acquire().await.unwrap();
        permit.forget();
        let mut receipt = json!({"request_id":body["request_id"],"run":{"id":Uuid::new_v4().to_string(),"job_id":Uuid::new_v4().to_string(),"status":"completed"}});
        match fake.mode {
            "wrong_receipt" => receipt["request_id"] = json!(Uuid::now_v7().to_string()),
            "failed" => receipt["run"]["status"] = json!("failed"),
            "idle" => receipt["run"] = Value::Null,
            "missing_run" => {
                receipt.as_object_mut().unwrap().remove("run");
            }
            "oversized" => {
                return Json(json!({"payload":"x".repeat(RESPONSE_LIMIT)})).into_response()
            }
            "non_json" => return "not JSON".into_response(),
            "error" => return axum::http::StatusCode::BAD_GATEWAY.into_response(),
            _ => {}
        }
        Json(receipt).into_response()
    }
    async fn setup(
        fixture: &Fixture,
        mode: &'static str,
    ) -> (Arc<State>, ScheduledUser, Arc<Fake>, JoinHandle<()>) {
        let issued = fixture.registry.add_user("alice").unwrap();
        let user = ScheduledUser {
            user_id: issued.user_id,
            backend_id: "alice".into(),
        };
        let fake = Arc::new(Fake {
            mode,
            gets: AtomicUsize::new(0),
            posts: AtomicUsize::new(0),
            status_gate: Arc::new(Semaphore::new(100)),
            post_gate: Arc::new(Semaphore::new(100)),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/internal/scheduler/status", get(fake_status))
            .route("/internal/scheduler/dispatch", post(fake_dispatch))
            .with_state(fake.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let state = Arc::new(State {
            registry: fixture.registry.clone(),
            backends: HashMap::from([(
                "alice".into(),
                Backend {
                    url: format!("http://{address}/").parse().unwrap(),
                    token: axum::http::HeaderValue::from_static("Bearer test-only-token"),
                    permit: Arc::new(Semaphore::new(1)),
                    control: Arc::new(Semaphore::new(2)),
                },
            )]),
            client: reqwest::Client::builder()
                .no_proxy()
                .retry(reqwest::retry::never())
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            permits: Arc::new(Semaphore::new(1)),
            timeout: Duration::from_secs(5),
            control: Arc::new(Semaphore::new(8)),
            scheduled_jobs: true,
            telegram: None,
        });
        (state, user, fake, server)
    }
    async fn until(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(4), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("condition was not reached");
    }

    #[test]
    fn receipts_and_status_require_exact_protocol_and_explicit_idle() {
        let request = Uuid::now_v7();
        let status = json!({"protocol":1,"mode":"gateway","backend_id":"alice","ready":true,"due":true,"max_run_seconds":120});
        assert_eq!(
            parse_status(&serde_json::to_vec(&status).unwrap(), "alice"),
            Ok(true)
        );
        for (key, value) in [
            ("protocol", json!(2)),
            ("mode", json!("local")),
            ("backend_id", json!("bob")),
            ("ready", json!(false)),
            ("due", json!(1)),
            ("max_run_seconds", json!(121)),
            ("extra", json!(true)),
        ] {
            let mut invalid = status.clone();
            invalid[key] = value;
            assert!(parse_status(&serde_json::to_vec(&invalid).unwrap(), "alice").is_err());
        }
        assert!(completed_receipt(
            &serde_json::to_vec(&json!({"request_id":request.to_string(),"run":null})).unwrap(),
            request
        ));
        for invalid in [
            json!({"request_id":request.to_string()}),
            json!({"request_id":request.to_string(),"run":null,"extra":true}),
            json!({"request_id":request.to_string(),"run":{"id":Uuid::nil().to_string(),"job_id":Uuid::new_v4().to_string(),"status":"completed"}}),
        ] {
            assert!(!completed_receipt(
                &serde_json::to_vec(&invalid).unwrap(),
                request
            ));
        }
    }

    #[tokio::test]
    async fn protocol_errors_retain_hold_and_are_never_retried() {
        for mode in [
            "wrong_receipt",
            "failed",
            "missing_run",
            "oversized",
            "non_json",
            "error",
            "timeout",
        ] {
            let fixture = Fixture::new();
            let (mut state, user, fake, server) = setup(&fixture, mode).await;
            if mode == "timeout" {
                Arc::get_mut(&mut state).unwrap().timeout = Duration::from_millis(100);
                fake.post_gate.forget_permits(100);
            }
            poll_backend(state.clone(), Stop::new(), user.clone()).await;
            assert_eq!(fake.posts.load(Ordering::SeqCst), 1, "{mode}");
            assert_eq!(
                fixture.registry.list().unwrap()[0]
                    .hold
                    .as_ref()
                    .unwrap()
                    .state,
                "needs_review",
                "{mode}"
            );
            let key = fixture.registry.add_key(user.user_id).unwrap();
            fixture.registry.rotate(key.key_id).unwrap();
            poll_backend(state, Stop::new(), user).await;
            assert_eq!(fake.posts.load(Ordering::SeqCst), 1, "no replay for {mode}");
            server.abort();
        }
    }

    #[tokio::test]
    async fn completed_and_idle_receipts_clear_hold_but_bad_status_never_admits() {
        for mode in ["completed", "idle", "bad_status"] {
            let fixture = Fixture::new();
            let (state, user, fake, server) = setup(&fixture, mode).await;
            poll_backend(state, Stop::new(), user).await;
            assert_eq!(
                fake.posts.load(Ordering::SeqCst),
                usize::from(mode != "bad_status")
            );
            assert!(fixture.registry.list().unwrap()[0].hold.is_none());
            server.abort();
        }
    }

    #[tokio::test]
    async fn disable_during_status_rechecks_authority_before_post() {
        let fixture = Fixture::new();
        let (state, user, fake, server) = setup(&fixture, "completed").await;
        fake.status_gate.forget_permits(100);
        let task = tokio::spawn(poll_backend(state, Stop::new(), user.clone()));
        until(|| fake.gets.load(Ordering::SeqCst) == 1).await;
        fixture.registry.set_enabled(user.user_id, false).unwrap();
        fake.status_gate.add_permits(1);
        task.await.unwrap();
        assert_eq!(fake.posts.load(Ordering::SeqCst), 0);
        assert!(fixture.registry.list().unwrap()[0].hold.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn stop_during_status_prevents_new_durable_admission() {
        let fixture = Fixture::new();
        let (state, user, fake, server) = setup(&fixture, "completed").await;
        fake.status_gate.forget_permits(100);
        let stop = Stop::new();
        let task = tokio::spawn(poll_backend(state, stop.clone(), user));
        until(|| fake.gets.load(Ordering::SeqCst) == 1).await;
        stop.stop();
        fake.status_gate.add_permits(1);
        task.await.unwrap();
        assert_eq!(fake.posts.load(Ordering::SeqCst), 0);
        assert!(fixture.registry.list().unwrap()[0].hold.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn control_lane_shares_no_execution_and_shutdown_does_not_abort_admitted_work() {
        let fixture = Fixture::new();
        let (state, user, fake, server) = setup(&fixture, "completed").await;
        let chat = state.backends["alice"]
            .permit
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        poll_backend(state.clone(), Stop::new(), user).await;
        assert_eq!(fake.gets.load(Ordering::SeqCst), 1);
        assert_eq!(fake.posts.load(Ordering::SeqCst), 0);
        drop(chat);
        fake.post_gate.forget_permits(100);
        let scheduler = start(state.clone());
        until(|| fake.posts.load(Ordering::SeqCst) == 1).await;
        assert_eq!(state.permits.available_permits(), 0);
        assert_eq!(state.backends["alice"].permit.available_permits(), 0);
        assert_eq!(state.control.available_permits(), 8);
        assert_eq!(state.backends["alice"].control.available_permits(), 2);
        assert!(!scheduler.shutdown(Duration::from_millis(10)).await);
        assert_eq!(state.permits.available_permits(), 0);
        assert_eq!(
            fixture.registry.list().unwrap()[0]
                .hold
                .as_ref()
                .unwrap()
                .state,
            "in_flight"
        );
        fake.post_gate.add_permits(1);
        until(|| state.permits.available_permits() == 1).await;
        assert!(fixture.registry.list().unwrap()[0].hold.is_none());
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(fake.posts.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
