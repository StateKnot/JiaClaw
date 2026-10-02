// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Scheduled delivery authorization exercised through real workers and `SQLite`.

use super::*;
use crate::{
    channel_store::ChannelDelivery,
    channel_types::ScheduledDestination,
    jobs::{Job, JobRun, JobSpec},
    schedule::ScheduleSpec,
    store::SessionStore,
};
use jiaclaw_core::{
    ChatMessage, ChatResponse, MessageRole, RunStatus, ScheduledChannelDestination,
};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Mutex},
    time::Duration,
};

const OWNER_TOKEN: &str = "scheduled-runtime-owner";

struct Fixture {
    state: AppState,
    workspace: PathBuf,
    model_calls: Arc<AtomicUsize>,
    model_entered: Arc<tokio::sync::Notify>,
    model_gate: Arc<tokio::sync::Semaphore>,
    outbound_calls: Arc<AtomicUsize>,
    outbound_body: Arc<Mutex<Option<Value>>>,
    server: tokio::task::JoinHandle<()>,
    destination: ScheduledDestination,
}

impl Fixture {
    async fn new(channel: Channel) -> Self {
        let model_calls = Arc::new(AtomicUsize::new(0));
        let model_entered = Arc::new(tokio::sync::Notify::new());
        let model_gate = Arc::new(tokio::sync::Semaphore::new(1));
        let outbound_calls = Arc::new(AtomicUsize::new(0));
        let outbound_body = Arc::new(Mutex::new(None));
        let models = model_calls.clone();
        let entered = model_entered.clone();
        let gate = model_gate.clone();
        let sends = outbound_calls.clone();
        let recorded = outbound_body.clone();
        let app = axum::Router::new()
            .route(
                "/v1/chat/completions",
                axum::routing::post(move || {
                    models.fetch_add(1, Ordering::SeqCst);
                    let entered = entered.clone();
                    let gate = gate.clone();
                    async move {
                        entered.notify_one();
                        gate.acquire().await.unwrap().forget();
                        Json(json!({"choices":[{"message":{"role":"assistant","content":"scheduled report"},"finish_reason":"stop"}]}))
                    }
                }),
            )
            .fallback(move |Json(body): Json<Value>| {
                sends.fetch_add(1, Ordering::SeqCst);
                *recorded.lock().unwrap() = Some(body.clone());
                async move {
                    if let Some(chat_id) = body.get("chat_id").and_then(Value::as_str) {
                        Json(json!({"ok":true,"result":{"message_id":99,"chat":{"id":chat_id.parse::<i64>().unwrap()}}}))
                    } else {
                        Json(json!({"ok":true,"channel":body["channel"],"ts":"1700000000.000099"}))
                    }
                }
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let workspace = std::env::temp_dir().join(format!(
            "jiaclaw-scheduled-delivery-runtime-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut state = crate::tests::test_state_for_workspace(workspace.clone());
        let mut config = state.agent.config().clone();
        config.provider.provider_type = "brokerrouter".into();
        config.provider.base_url.clone_from(&base);
        config.provider.api_key = Some("scheduled-runtime-model-key".into());
        config.scheduler.enabled = true;
        state.agent = Arc::new(jiaclaw::JiaClawAgent::new(config).unwrap());
        state.sessions = Arc::new(Mutex::new(
            SessionStore::open(Path::new(":memory:")).unwrap(),
        ));
        state.persist_enabled = true;
        state.api_token = Some(OWNER_TOKEN.into());
        state.scheduler_health.store(1, Ordering::Release);
        let destination = target(channel);
        state.channel_runtime = Some(Arc::new(ChannelRuntime {
            installations: vec![Installation {
                channel,
                policy: ChannelBinding {
                    channel: channel_name(channel).into(),
                    installation_id: destination.installation_id.clone(),
                    app_id: (channel == Channel::Slack).then(|| "ATESTAPP".into()),
                    // These intentionally authorize another conversation. A
                    // scheduled destination must neither require nor grant ingress.
                    allowed_senders: vec![sender_id(channel).into()],
                    allowed_conversations: vec![inbound_conversation(channel).into()],
                    enabled_tools: vec!["datetime_now".into()],
                    timeout_secs: 30,
                    local_test_api_base: Some(base.clone()),
                    scheduled_destinations: vec![ScheduledChannelDestination {
                        conversation_id: destination.conversation_id.clone(),
                        thread_id: destination.thread_id.clone(),
                    }],
                },
                credential: match channel {
                    Channel::Telegram => "123456:fixture-token",
                    Channel::Slack => "xoxb-fixture-token",
                    Channel::Discord | Channel::Feishu | Channel::Wecom | Channel::Dingtalk => {
                        unreachable!()
                    }
                }
                .into(),
                inbound_secret: "inbound-fixture-secret".into(),
                api_base: base,
                feishu_sender: None,
                feishu_verification_token: None,
                wecom_sender: None,
                wecom_callback: None,
                dingtalk_sender: None,
                dingtalk_callback: None,
            }],
            client: OutboundClient::new_with_loopback(true).unwrap(),
            cipher: None,
            health: AtomicU8::new(1),
        }));
        Self {
            state,
            workspace,
            model_calls,
            model_entered,
            model_gate,
            outbound_calls,
            outbound_body,
            server,
            destination,
        }
    }

    fn due_job(&self) -> Job {
        self.state
            .sessions
            .lock()
            .unwrap()
            .create_job(job_spec(self.destination.clone()), now_ms() - 60_000)
            .unwrap()
    }

    fn completed_delivery(&self) -> (Job, JobRun, ChannelDelivery) {
        let job = self.due_job();
        let mut store = self.state.sessions.lock().unwrap();
        let run = store.claim_due_jobs(now_ms(), 1).unwrap().remove(0);
        assert_eq!(run.job_id, job.id);
        let response = ChatResponse {
            message: ChatMessage {
                role: MessageRole::Assistant,
                content: "pending scheduled report".into(),
            },
            tool_calls: vec![],
            status: RunStatus::Completed,
            session_id: Some(run.session_id.clone()),
        };
        assert!(store
            .finish_job_run(&run.id, None, "completed", Some(response), None, now_ms())
            .unwrap());
        let delivery = store.claim_channel_delivery(now_ms()).unwrap().unwrap();
        assert_eq!(delivery.job_run_id.as_deref(), Some(run.id.as_str()));
        assert!(delivery.event_id.is_none());
        (job, run, delivery)
    }

    fn policy_mut(&mut self) -> &mut ChannelBinding {
        &mut Arc::get_mut(self.state.channel_runtime.as_mut().unwrap())
            .expect("no worker shares the fixture before policy replacement")
            .installations[0]
            .policy
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.workspace);
    }
}

fn target(channel: Channel) -> ScheduledDestination {
    match channel {
        Channel::Feishu | Channel::Wecom | Channel::Dingtalk => {
            unreachable!("Dedicated process fixtures")
        }
        Channel::Telegram => ScheduledDestination {
            channel,
            installation_id: "123456".into(),
            conversation_id: "-100444".into(),
            thread_id: Some("77".into()),
        },
        Channel::Slack => ScheduledDestination {
            channel,
            installation_id: "TTESTTEAM".into(),
            conversation_id: "CSCHEDULED".into(),
            thread_id: Some("1700000000.000077".into()),
        },
        Channel::Discord => ScheduledDestination {
            channel,
            installation_id: "123456789012345678".into(),
            conversation_id: "234567890123456789".into(),
            thread_id: None,
        },
    }
}

fn sender_id(channel: Channel) -> &'static str {
    if channel == Channel::Telegram {
        "42"
    } else {
        "UTESTUSER"
    }
}

fn inbound_conversation(channel: Channel) -> &'static str {
    if channel == Channel::Telegram {
        "-100999"
    } else {
        "CINBOUND"
    }
}

fn job_spec(destination: ScheduledDestination) -> JobSpec {
    JobSpec {
        name: "authorized scheduled delivery".into(),
        prompt: "Produce a short scheduled report without calling tools.".into(),
        schedule: ScheduleSpec::Interval { seconds: 60 },
        enabled_tools: vec!["datetime_now".into()],
        timeout_secs: 30,
        delivery: Some(destination),
    }
}

fn owner_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {OWNER_TOKEN}").parse().unwrap(),
    );
    headers
}

async fn wait_for_run(state: &AppState, job: &Job) -> JobRun {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let id = job.id.clone();
            let runs = crate::with_sessions(state, move |store| store.list_job_runs(&id, 1, 0))
                .await
                .unwrap();
            if let Some(run) = runs.into_iter().find(|run| run.status != "running") {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("scheduled worker must persist a terminal run")
}

#[tokio::test]
async fn scheduled_targets_require_exact_installation_conversation_and_thread() {
    for channel in [Channel::Telegram, Channel::Slack] {
        let fixture = Fixture::new(channel).await;
        assert!(validate_scheduled_delivery(&fixture.state, &fixture.destination).is_ok());
        let mut wrong_installation = fixture.destination.clone();
        wrong_installation.installation_id = "999999".into();
        let mut inbound_only = fixture.destination.clone();
        inbound_only.conversation_id = inbound_conversation(channel).into();
        let mut wrong_thread = fixture.destination.clone();
        wrong_thread.thread_id = Some(
            if channel == Channel::Telegram {
                "78"
            } else {
                "1700000000.000078"
            }
            .into(),
        );
        let mut whole_conversation = fixture.destination.clone();
        whole_conversation.thread_id = None;
        for denied in [
            wrong_installation,
            inbound_only,
            wrong_thread,
            whole_conversation,
            target(Channel::Discord),
        ] {
            assert!(validate_scheduled_delivery(&fixture.state, &denied).is_err());
            assert!(crate::scheduler::create(
                State(fixture.state.clone()),
                owner_headers(),
                Json(job_spec(denied))
            )
            .await
            .is_err());
        }
        assert!(fixture
            .state
            .sessions
            .lock()
            .unwrap()
            .list_jobs(100, 0)
            .unwrap()
            .is_empty());
        let binding = &fixture
            .state
            .channel_runtime
            .as_ref()
            .unwrap()
            .installations[0];
        let inbound = Destination {
            channel,
            installation_id: fixture.destination.installation_id.clone(),
            conversation_id: fixture.destination.conversation_id.clone(),
            thread_id: fixture.destination.thread_id.clone(),
            interaction_id: None,
            expires_ms: None,
        };
        assert!(matches!(
            event_spec(
                binding,
                "scheduled-is-not-ingress".into(),
                sender_id(channel).into(),
                "hello".into(),
                inbound,
                None
            ),
            Err(AppError::Unauthorized)
        ));
        let (status, _) = crate::scheduler::create(
            State(fixture.state.clone()),
            owner_headers(),
            Json(job_spec(fixture.destination.clone())),
        )
        .await
        .unwrap();
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.outbound_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn independent_scheduled_permission_runs_model_and_delivers_exact_thread() {
    for channel in [Channel::Telegram, Channel::Slack] {
        let fixture = Fixture::new(channel).await;
        let scheduler = crate::scheduler::start(fixture.state.clone())
            .await
            .unwrap()
            .unwrap();
        let job = fixture.due_job();
        let run = wait_for_run(&fixture.state, &job).await;
        scheduler
            .shutdown(&fixture.state, Duration::from_secs(2))
            .await;
        assert_eq!(run.status, "completed");
        assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 1);
        let delivery = fixture
            .state
            .sessions
            .lock()
            .unwrap()
            .claim_channel_delivery(now_ms())
            .unwrap()
            .unwrap();
        assert_eq!(delivery.job_run_id.as_deref(), Some(run.id.as_str()));
        assert!(delivery.event_id.is_none());
        let delivery_id = delivery.id.clone();
        deliver(fixture.state.clone(), delivery).await.unwrap();
        let store = fixture.state.sessions.lock().unwrap();
        assert_eq!(
            store
                .get_channel_delivery(&delivery_id)
                .unwrap()
                .unwrap()
                .state,
            "delivered"
        );
        assert!(store.get(&run.session_id).unwrap().is_some());
        assert!(store.get_job(&job.id).unwrap().unwrap().enabled);
        assert_eq!(fixture.outbound_calls.load(Ordering::SeqCst), 1);
        let body = fixture.outbound_body.lock().unwrap().clone().unwrap();
        if channel == Channel::Telegram {
            assert_eq!(body["chat_id"], fixture.destination.conversation_id);
            assert_eq!(body["message_thread_id"], 77);
        } else {
            assert_eq!(body["channel"], fixture.destination.conversation_id);
            assert_eq!(body["thread_ts"], "1700000000.000077");
        }
    }
}

#[tokio::test]
async fn revoking_scheduled_binding_blocks_existing_outbox_before_http_and_pauses_job() {
    for channel in [Channel::Telegram, Channel::Slack] {
        for changed in ["removed", "installation", "thread", "tools"] {
            let mut fixture = Fixture::new(channel).await;
            let (job, _, delivery) = fixture.completed_delivery();
            match changed {
                "removed" => fixture.policy_mut().scheduled_destinations.clear(),
                "installation" => fixture.policy_mut().installation_id = "999999".into(),
                "thread" => fixture.policy_mut().scheduled_destinations[0].thread_id = None,
                "tools" => fixture.policy_mut().enabled_tools.clear(),
                _ => unreachable!(),
            }
            let delivery_id = delivery.id.clone();
            tokio::time::timeout(
                Duration::from_secs(2),
                deliver(fixture.state.clone(), delivery),
            )
            .await
            .unwrap()
            .unwrap();
            let store = fixture.state.sessions.lock().unwrap();
            assert_eq!(
                store
                    .get_channel_delivery(&delivery_id)
                    .unwrap()
                    .unwrap()
                    .state,
                "permanent_failed",
                "{channel:?}: {changed}"
            );
            assert!(
                !store.get_job(&job.id).unwrap().unwrap().enabled,
                "{channel:?}: {changed}"
            );
            assert_eq!(fixture.outbound_calls.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn unhealthy_or_missing_channels_reject_job_creation_without_model_execution() {
    for health in [0, 2, 3] {
        let fixture = Fixture::new(Channel::Telegram).await;
        fixture
            .state
            .channel_runtime
            .as_ref()
            .unwrap()
            .health
            .store(health, Ordering::Release);
        assert!(validate_scheduled_delivery(&fixture.state, &fixture.destination).is_err());
        assert!(crate::scheduler::create(
            State(fixture.state.clone()),
            owner_headers(),
            Json(job_spec(fixture.destination.clone()))
        )
        .await
        .is_err());
        assert!(fixture
            .state
            .sessions
            .lock()
            .unwrap()
            .list_jobs(100, 0)
            .unwrap()
            .is_empty());
        assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.outbound_calls.load(Ordering::SeqCst), 0);
    }
    let mut fixture = Fixture::new(Channel::Telegram).await;
    fixture.state.channel_runtime = None;
    assert!(crate::scheduler::create(
        State(fixture.state.clone()),
        owner_headers(),
        Json(job_spec(fixture.destination.clone()))
    )
    .await
    .is_err());
    assert!(fixture
        .state
        .sessions
        .lock()
        .unwrap()
        .list_jobs(100, 0)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn unhealthy_channels_pause_due_jobs_without_model_or_outbox() {
    for health in [Some(0), Some(2), Some(3), None] {
        let mut fixture = Fixture::new(Channel::Telegram).await;
        if let Some(health) = health {
            fixture
                .state
                .channel_runtime
                .as_ref()
                .unwrap()
                .health
                .store(health, Ordering::Release);
        } else {
            fixture.state.channel_runtime = None;
        }
        let scheduler = crate::scheduler::start(fixture.state.clone())
            .await
            .unwrap()
            .unwrap();
        let job = fixture.due_job();
        let run = wait_for_run(&fixture.state, &job).await;
        scheduler
            .shutdown(&fixture.state, Duration::from_secs(2))
            .await;
        assert!(matches!(run.status.as_str(), "failed" | "needs_review"));
        let store = fixture.state.sessions.lock().unwrap();
        assert!(!store.get_job(&job.id).unwrap().unwrap().enabled);
        assert!(store.get(&run.session_id).unwrap().is_none());
        assert!(store
            .list_channel_deliveries(None, 100, 0)
            .unwrap()
            .is_empty());
        assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.outbound_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn disabled_scheduler_recovers_running_jobs_and_preserves_existing_outbox() {
    let mut fixture = Fixture::new(Channel::Telegram).await;
    let (_, completed_run, old_delivery) = fixture.completed_delivery();
    let interrupted_job = fixture.due_job();
    let running = fixture
        .state
        .sessions
        .lock()
        .unwrap()
        .claim_due_jobs(now_ms(), 1)
        .unwrap()
        .remove(0);
    assert_eq!(running.job_id, interrupted_job.id);
    let mut config = fixture.state.agent.config().clone();
    config.scheduler.enabled = false;
    fixture.state.agent = Arc::new(jiaclaw::JiaClawAgent::new(config).unwrap());
    fixture.state.scheduler_health.store(0, Ordering::Release);

    assert!(crate::scheduler::start(fixture.state.clone())
        .await
        .unwrap()
        .is_none());

    let store = fixture.state.sessions.lock().unwrap();
    let recovered = store.get_job_run(&running.id).unwrap().unwrap();
    assert_eq!(recovered.status, "interrupted");
    assert!(recovered.finished_ms.is_some());
    assert!(!store.get_job(&interrupted_job.id).unwrap().unwrap().enabled);
    assert!(store.get(&running.session_id).unwrap().is_none());
    assert_eq!(
        store
            .get_job_run(&completed_run.id)
            .unwrap()
            .unwrap()
            .status,
        "completed"
    );
    let deliveries = store.list_channel_deliveries(None, 100, 0).unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].id, old_delivery.id);
    // Channel recovery owns uncertain sends; scheduler recovery must retain
    // this existing record without sending, replaying, or replacing it.
    assert_eq!(deliveries[0].state, old_delivery.state);
    assert_eq!(fixture.state.scheduler_health.load(Ordering::Acquire), 0);
    assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.outbound_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn admitted_run_retains_completed_result_when_channel_stops_or_fails() {
    for health in [3, 2] {
        let fixture = Fixture::new(Channel::Telegram).await;
        fixture.model_gate.try_acquire().unwrap().forget();
        let scheduler = crate::scheduler::start(fixture.state.clone())
            .await
            .unwrap()
            .unwrap();
        let job = fixture.due_job();
        tokio::time::timeout(Duration::from_secs(5), fixture.model_entered.notified())
            .await
            .expect("model request must enter before the channel changes health");
        let runtime = fixture.state.channel_runtime.as_ref().unwrap();
        runtime.health.store(health, Ordering::Release);
        fixture.model_gate.add_permits(1);

        let run = wait_for_run(&fixture.state, &job).await;
        scheduler
            .shutdown(&fixture.state, Duration::from_secs(2))
            .await;
        assert_eq!(run.status, "completed", "channel health {health}");
        let delivery = {
            let mut store = fixture.state.sessions.lock().unwrap();
            assert!(store.get_job(&job.id).unwrap().unwrap().enabled);
            assert!(store.get(&run.session_id).unwrap().is_some());
            let pending = store.list_job_deliveries(&job.id, &run.id, 10, 0).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].state, "pending");
            assert_eq!(pending[0].text, "scheduled report");
            store.claim_channel_delivery(now_ms()).unwrap().unwrap()
        };
        assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.outbound_calls.load(Ordering::SeqCst), 0);
        runtime.health.store(1, Ordering::Release);
        let id = delivery.id.clone();
        deliver(fixture.state.clone(), delivery).await.unwrap();
        assert_eq!(
            fixture
                .state
                .sessions
                .lock()
                .unwrap()
                .get_channel_delivery(&id)
                .unwrap()
                .unwrap()
                .state,
            "delivered"
        );
        assert_eq!(fixture.outbound_calls.load(Ordering::SeqCst), 1);
    }
}
