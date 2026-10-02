// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Lifecycle regressions using the real channel worker and `SQLite` transitions.

use super::*;
use crate::{channel_store::ChannelEvent, store::SessionStore};
use std::{
    future::{poll_fn, Future},
    path::{Path, PathBuf},
    sync::{atomic::AtomicUsize, Mutex},
    task::Poll,
    time::Duration,
};

const INSTALLATION_ID: &str = "123456789012345678";
const CONVERSATION_ID: &str = "234567890123456789";
const SENDER_ID: &str = "345678901234567890";
const GOOD_KEY: [u8; 32] = [17; 32];

struct Fixture {
    state: AppState,
    workspace: PathBuf,
    model_calls: Arc<AtomicUsize>,
    model_server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let model_calls = Arc::new(AtomicUsize::new(0));
        let observed = model_calls.clone();
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                async {
                    Json(json!({
                        "choices": [{
                            "message": {"role": "assistant", "content": "verified model response"},
                            "finish_reason": "stop"
                        }]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let model_url = format!("http://{}", listener.local_addr().unwrap());
        let model_server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let workspace = std::env::temp_dir().join(format!(
            "jiaclaw-channel-lifecycle-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        let mut state = crate::tests::test_state_for_workspace(workspace.clone());
        let mut config = state.agent.config().clone();
        config.provider.provider_type = "brokerrouter".into();
        config.provider.base_url = model_url;
        config.provider.api_key = Some("lifecycle-fixture-key".into());
        state.agent = Arc::new(jiaclaw::JiaClawAgent::new(config).unwrap());
        state.sessions = Arc::new(Mutex::new(
            SessionStore::open(Path::new(":memory:")).unwrap(),
        ));
        state.persist_enabled = true;
        state.api_token = Some("lifecycle-fixture-owner".into());
        state.channel_runtime = Some(runtime_with_key(&GOOD_KEY));
        Self {
            state,
            workspace,
            model_calls,
            model_server,
        }
    }

    fn admit_and_claim(&self, spec: EventSpec) -> ChannelEvent {
        let mut store = self.state.sessions.lock().unwrap();
        let admitted = store.accept_channel_event(spec, now_ms()).unwrap();
        let event = store.claim_channel_event(now_ms()).unwrap().unwrap();
        assert_eq!(event.id, admitted.id);
        event
    }

    fn assert_unexecuted(&self, event: &ChannelEvent, reason: &str) {
        let store = self.state.sessions.lock().unwrap();
        let saved = store.get_channel_event(&event.id).unwrap().unwrap();
        assert_eq!(saved.status, "needs_review");
        assert!(saved
            .error
            .as_deref()
            .is_some_and(|error| error.contains(reason)));
        assert!(store.get(&event.spec.session_id).unwrap().is_none());
        assert!(store
            .list_channel_deliveries(Some(&event.id), 100, 0)
            .unwrap()
            .is_empty());
        assert_eq!(self.model_calls.load(Ordering::SeqCst), 0);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.model_server.abort();
        let _ = std::fs::remove_dir_all(&self.workspace);
    }
}

fn runtime_with_key(key: &[u8; 32]) -> Arc<ChannelRuntime> {
    Arc::new(ChannelRuntime {
        installations: vec![Installation {
            channel: Channel::Discord,
            policy: ChannelBinding {
                channel: "discord".into(),
                installation_id: INSTALLATION_ID.into(),
                app_id: None,
                allowed_senders: vec![SENDER_ID.into()],
                allowed_conversations: vec![CONVERSATION_ID.into()],
                scheduled_destinations: vec![],
                enabled_tools: vec!["datetime_now".into()],
                timeout_secs: 600,
                local_test_api_base: None,
            },
            credential: String::new(),
            inbound_secret: String::new(),
            api_base: "https://discord.com/api/v10".into(),
            feishu_sender: None,
            feishu_verification_token: None,
            wecom_sender: None,
            wecom_callback: None,
            dingtalk_sender: None,
            dingtalk_callback: None,
        }],
        client: OutboundClient::new().unwrap(),
        cipher: Some(aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, key).unwrap(),
        )),
        health: AtomicU8::new(0),
    })
}

fn spec(rt: &ChannelRuntime, event_id: &str, expires_ms: i64) -> EventSpec {
    let destination = Destination {
        channel: Channel::Discord,
        installation_id: INSTALLATION_ID.into(),
        conversation_id: CONVERSATION_ID.into(),
        thread_id: None,
        interaction_id: Some(event_id.into()),
        expires_ms: Some(expires_ms),
    };
    let sealed = rt
        .seal(
            "private-lifecycle-interaction-token",
            &token_aad(&destination),
        )
        .unwrap();
    EventSpec {
        event_id: event_id.into(),
        session_id: format!("channel:discord:lifecycle:{event_id}"),
        sender_id: SENDER_ID.into(),
        prompt: "Return a short response without calling tools.".into(),
        enabled_tools: vec!["datetime_now".into()],
        timeout_secs: 600,
        destination,
        sealed_token: Some(sealed),
        fingerprint: "f".repeat(64),
    }
}

#[tokio::test]
async fn wrong_state_key_stops_before_model_and_restoring_key_does_not_replay() {
    let mut fixture = Fixture::new().await;
    let original_runtime = fixture.state.channel_runtime.as_ref().unwrap().clone();
    let event = fixture.admit_and_claim(spec(
        &original_runtime,
        "456789012345678901",
        now_ms() + 120_000,
    ));
    fixture.state.channel_runtime = Some(runtime_with_key(&[29; 32]));
    tokio::time::timeout(
        Duration::from_secs(2),
        process_event(fixture.state.clone(), event.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    fixture.assert_unexecuted(&event, "credential unavailable");

    // A repaired encryption key must not implicitly retry a reviewed run.
    fixture.state.channel_runtime = Some(original_runtime);
    {
        let mut store = fixture.state.sessions.lock().unwrap();
        assert_eq!(store.recover_channels(now_ms()).unwrap(), (0, 0));
        assert!(store.claim_channel_event(now_ms()).unwrap().is_none());
    }
    fixture.assert_unexecuted(&event, "credential unavailable");
}

#[tokio::test]
async fn valid_key_and_budget_reach_model_and_atomically_create_reply() {
    // Positive control: the probe really observes this production model path,
    // and all authorization/tool prerequisites in the negative tests are valid.
    let fixture = Fixture::new().await;
    let event = fixture.admit_and_claim(spec(
        fixture.state.channel_runtime.as_ref().unwrap(),
        "456789012345678902",
        now_ms() + 120_000,
    ));
    tokio::time::timeout(
        Duration::from_secs(5),
        process_event(fixture.state.clone(), event.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    let store = fixture.state.sessions.lock().unwrap();
    assert_eq!(
        store.get_channel_event(&event.id).unwrap().unwrap().status,
        "completed"
    );
    assert!(store.get(&event.spec.session_id).unwrap().is_some());
    let replies = store
        .list_channel_deliveries(Some(&event.id), 100, 0)
        .unwrap();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].state, "pending");
    assert_eq!(replies[0].text, "verified model response");
    assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn queue_wait_that_consumes_delivery_reserve_never_starts_model() {
    let fixture = Fixture::new().await;
    let input = spec(
        fixture.state.channel_runtime.as_ref().unwrap(),
        "456789012345678903",
        now_ms() + 9_000,
    );
    // It was valid at admission; its time in the durable queue consumed the
    // execution budget, leaving only the ten-second outbound reserve.
    let event = {
        let mut store = fixture.state.sessions.lock().unwrap();
        store
            .accept_channel_event(input, now_ms() - 60_000)
            .unwrap();
        store.claim_channel_event(now_ms()).unwrap().unwrap()
    };
    tokio::time::timeout(
        Duration::from_secs(2),
        process_event(fixture.state.clone(), event.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    fixture.assert_unexecuted(&event, "interaction expired");
}

#[tokio::test]
async fn session_lock_wait_uses_expiry_budget_instead_of_configured_timeout() {
    let fixture = Fixture::new().await;
    let event = fixture.admit_and_claim(spec(
        fixture.state.channel_runtime.as_ref().unwrap(),
        "456789012345678904",
        now_ms() + 70_000,
    ));
    assert_eq!(event.spec.timeout_secs, 600);
    let guard = crate::session_turn_lock(&fixture.state, &event.spec.session_id).await;
    tokio::time::pause();
    let execution = process_event(fixture.state.clone(), event.clone());
    tokio::pin!(execution);
    // Poll once to reach the real occupied mutex before advancing the clock.
    poll_fn(|cx| {
        assert!(execution.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(2), execution)
        .await
        .unwrap()
        .unwrap();
    // The guard remains held throughout: no model call could be admitted by
    // accidentally releasing it or by waiting for the full 600-second policy.
    fixture.assert_unexecuted(&event, "session deadline expired");
    drop(guard);
}

#[test]
fn execution_budget_is_clamped_to_expiry_and_reserves_delivery_time() {
    let rt = runtime_with_key(&GOOD_KEY);
    let now = 1_000_000;
    let mut input = spec(&rt, "456789012345678905", now + 120_000);
    for (remaining_ms, expected_ms) in [
        (-1, 0),
        (0, 0),
        (9_999, 0),
        (10_000, 0),
        (10_001, 1),
        (30_123, 20_123),
        (70_000, 60_000),
        (900_000, 600_000),
    ] {
        input.destination.expires_ms = Some(now + remaining_ms);
        assert_eq!(
            execution_budget(&input, now),
            Duration::from_millis(expected_ms)
        );
    }
    input.timeout_secs = 1;
    input.destination.expires_ms = Some(now + 900_000);
    assert_eq!(execution_budget(&input, now), Duration::from_secs(1));
    input.destination.expires_ms = Some(i64::MAX);
    assert_eq!(execution_budget(&input, i64::MIN), Duration::from_secs(1));
    input.destination.expires_ms = Some(i64::MIN);
    assert!(execution_budget(&input, i64::MAX).is_zero());
    input.destination.expires_ms = None;
    assert_eq!(execution_budget(&input, now), Duration::from_secs(1));
}

#[tokio::test]
async fn disabled_channels_recover_inflight_work_without_starting_workers() {
    let mut fixture = Fixture::new().await;
    let rt = fixture.state.channel_runtime.as_ref().unwrap().clone();
    let (processing, submitted, waiting, received) = {
        let mut store = fixture.state.sessions.lock().unwrap();
        store
            .accept_channel_event(
                spec(&rt, "456789012345678906", now_ms() + 120_000),
                now_ms(),
            )
            .unwrap();
        let completed = store.claim_channel_event(now_ms()).unwrap().unwrap();
        store
            .complete_channel_event(
                &completed.id,
                None,
                "completed",
                vec!["first reply".into(), "second reply".into()],
                None,
                now_ms(),
            )
            .unwrap();
        let submitted = store.claim_channel_delivery(now_ms()).unwrap().unwrap();
        let waiting = store
            .list_channel_deliveries(Some(&completed.id), 100, 0)
            .unwrap()
            .remove(1);
        store
            .accept_channel_event(
                spec(&rt, "456789012345678907", now_ms() + 120_000),
                now_ms(),
            )
            .unwrap();
        let processing = store.claim_channel_event(now_ms()).unwrap().unwrap();
        let received = store
            .accept_channel_event(
                spec(&rt, "456789012345678908", now_ms() + 120_000),
                now_ms(),
            )
            .unwrap();
        (processing, submitted, waiting, received)
    };
    fixture.state.channel_runtime = None;
    // Repeat startup to prove recovery is stable and never admits queued work
    // when the configuration has no channel runtime.
    for _ in 0..2 {
        assert!(start(fixture.state.clone()).await.unwrap().is_none());
        let store = fixture.state.sessions.lock().unwrap();
        assert_eq!(
            store
                .get_channel_event(&processing.id)
                .unwrap()
                .unwrap()
                .status,
            "needs_review"
        );
        let recovered = store.get_channel_delivery(&submitted.id).unwrap().unwrap();
        assert_eq!(recovered.state, "unknown");
        assert_eq!(recovered.attempts, 1);
        assert!(recovered.sealed_token.is_none());
        assert_eq!(
            store
                .get_channel_delivery(&waiting.id)
                .unwrap()
                .unwrap()
                .state,
            "pending"
        );
        assert_eq!(
            store
                .get_channel_event(&received.id)
                .unwrap()
                .unwrap()
                .status,
            "received"
        );
        assert_eq!(store.len().unwrap(), 0);
        assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
    }
}

async fn cancelled_claim_cannot_follow_recovery(delivery: bool) {
    let fixture = Fixture::new().await;
    let rt = fixture.state.channel_runtime.as_ref().unwrap().clone();
    rt.health.store(1, Ordering::Release);
    let event_id = {
        let mut store = fixture.state.sessions.lock().unwrap();
        let admitted = store
            .accept_channel_event(
                spec(&rt, "456789012345678909", now_ms() + 120_000),
                now_ms(),
            )
            .unwrap();
        if delivery {
            let event = store.claim_channel_event(now_ms()).unwrap().unwrap();
            store
                .complete_channel_event(
                    &event.id,
                    None,
                    "completed",
                    vec!["pending outbound reply".into()],
                    None,
                    now_ms(),
                )
                .unwrap();
        }
        admitted.id
    };

    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let locked_store = fixture.state.sessions.clone();
    let locker = std::thread::spawn(move || {
        let mut store = locked_store.lock().unwrap();
        locked_tx.send(()).unwrap();
        // Dropping the sender on test failure also releases this real mutex.
        if release_rx.recv().unwrap_or(false) {
            assert_eq!(store.recover_channels(now_ms()).unwrap(), (0, 0));
        }
    });
    locked_rx.await.unwrap();
    let claim_state = fixture.state.clone();
    // Until this barrier passes, only with_sessions can add another owner.
    // Thus the closure is dispatched and blocked on the actual store mutex.
    let dispatched_owners = Arc::strong_count(&fixture.state.sessions) + 1;
    let waiter = tokio::spawn(async move {
        if delivery {
            claim_delivery(&claim_state)
                .await
                .map(|result| result.map(|record| record.id))
        } else {
            claim_event(&claim_state)
                .await
                .map(|result| result.map(|record| record.id))
        }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&fixture.state.sessions) < dispatched_owners {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("claim must reach the real blocking store operation");
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    rt.health.store(3, Ordering::Release);
    // Recovery deterministically runs while the original lock is still held;
    // only afterwards can the abandoned claim closure enter the database.
    release_tx.send(true).unwrap();
    locker.join().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&fixture.state.sessions) > 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled claim closure must finish before inspecting recovered state");

    {
        let store = fixture.state.sessions.lock().unwrap();
        let event = store.get_channel_event(&event_id).unwrap().unwrap();
        let replies = store
            .list_channel_deliveries(Some(&event_id), 100, 0)
            .unwrap();
        if delivery {
            assert_eq!(event.status, "completed");
            assert_eq!(replies.len(), 1);
            assert_eq!(replies[0].state, "pending");
            assert_eq!(replies[0].attempts, 0);
            assert!(replies[0].started_ms.is_none());
        } else {
            assert_eq!(event.status, "received");
            assert!(event.started_ms.is_none());
            assert!(replies.is_empty());
        }
        assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
    }
    // Positive control: the untouched row is genuinely eligible once a new,
    // explicitly authorized running worker invokes the same claim helper.
    rt.health.store(1, Ordering::Release);
    if delivery {
        assert!(claim_delivery(&fixture.state).await.unwrap().is_some());
    } else {
        assert!(claim_event(&fixture.state).await.unwrap().is_some());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_event_claim_cannot_create_processing_after_recovery() {
    cancelled_claim_cannot_follow_recovery(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_delivery_claim_cannot_create_submitting_after_recovery() {
    cancelled_claim_cannot_follow_recovery(true).await;
}
