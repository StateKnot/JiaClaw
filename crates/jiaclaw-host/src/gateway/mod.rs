// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Authentication boundary for independently isolated `JiaClaw` backends.
pub(crate) mod cli;
mod config;
mod discord;
mod discord_store;
mod feishu;
mod feishu_store;
mod keys;
mod proxy;
mod registry;
mod scheduler;
mod slack;
mod slack_store;
mod telegram;
mod telegram_store;
mod turns;
mod wecom;
mod wecom_store;

use anyhow::{bail, Context, Result};
use axum::{routing::get, Router};
use config::Config;
use fs2::FileExt;
use registry::Registry;
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    future::IntoFuture,
    io::Read,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Semaphore;

struct Backend {
    url: reqwest::Url,
    token: axum::http::HeaderValue,
    permit: Arc<Semaphore>,
    control: Arc<Semaphore>,
    streams: Arc<Semaphore>,
}
struct State {
    registry: Registry,
    admission: scheduler::Stop,
    backends: HashMap<String, Backend>,
    client: reqwest::Client,
    permits: Arc<Semaphore>,
    timeout: Duration,
    control: Arc<Semaphore>,
    streams: Arc<Semaphore>,
    scheduled_jobs: bool,
    tracked_turns: bool,
    reserved_turns: AtomicUsize,
    telegram: Option<Arc<telegram::Runtime>>,
    slack: Option<Arc<slack::Runtime>>,
    discord: Option<Arc<discord::Runtime>>,
    feishu: Option<Arc<feishu::Runtime>>,
    wecom: Option<Arc<wecom::Runtime>>,
}

/// Every runtime registry clone retains this lease, including actual blocking DB
/// work after its async waiter disappears. Last-owner drop explicitly unlocks.
struct ProcessLock(File);
impl Drop for ProcessLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn private_file(path: &std::path::Path) -> Result<File> {
    if let Ok(metadata) = path.symlink_metadata() {
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "gateway lock must be a regular file"
        );
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).context("open gateway process lock")
}

/// The process lock spans both channel queue checks and the registry hold clear.
fn channel_review_guard(
    config: &Config,
    registry: &Registry,
    user: uuid::Uuid,
) -> Result<Option<File>> {
    if !registry
        .list_telegram_bindings()?
        .iter()
        .any(|b| b.user_id == user)
        && !registry
            .list_slack_bindings()?
            .iter()
            .any(|b| b.user_id == user)
        && !registry
            .list_discord_bindings()?
            .iter()
            .any(|b| b.user_id == user)
        && !registry
            .list_feishu_bindings()?
            .iter()
            .any(|b| b.user_id == user)
        && !registry
            .list_wecom_bindings()?
            .iter()
            .any(|b| b.user_id == user)
    {
        return Ok(None);
    }
    let guard = private_file(&config.registry_path.with_extension("gateway.lock"))?;
    guard
        .try_lock_exclusive()
        .context("stop gateway before reviewing channel queues")?;
    registry.recover_writes()?;
    telegram::review_pending(config, registry, user)?;
    slack::review_pending(config, registry, user)?;
    discord::review_pending(config, registry, user)?;
    feishu::review_pending(config, registry, user)?;
    wecom::review_pending(config, registry, user)?;
    Ok(Some(guard))
}
fn review_queue(store: &crate::store::SessionStore) -> Result<()> {
    for offset in (0..=10_000).step_by(100) {
        let events = store.list_channel_events(100, offset)?;
        anyhow::ensure!(
            events.iter().all(|e| e.status != "processing"
                && (e.status != "needs_review" || e.reviewed_ms.is_some())),
            "review or cancel unresolved channel events before clearing the hold"
        );
        if events.len() < 100 {
            break;
        }
        anyhow::ensure!(
            offset < 10_000,
            "channel event inspection capacity exceeded"
        );
    }
    for offset in (0..=10_000).step_by(100) {
        let deliveries = store.list_channel_deliveries(None, 100, offset)?;
        anyhow::ensure!(
            deliveries.iter().all(|d| !matches!(
                d.state.as_str(),
                "submitting" | "unknown" | "permanent_failed" | "expired"
            )),
            "resolve or cancel unresolved channel deliveries before clearing the hold"
        );
        if deliveries.len() < 100 {
            break;
        }
        anyhow::ensure!(
            offset < 10_000,
            "channel delivery inspection capacity exceeded"
        );
    }
    Ok(())
}

pub(super) async fn serve(config: Config) -> Result<()> {
    config.validate()?;
    let registry = Registry::open(&config.registry_path)?;
    let lock_path = config.registry_path.with_extension("gateway.lock");
    let process_lock = private_file(&lock_path)?;
    process_lock
        .try_lock_exclusive()
        .context("another gateway is using this registry")?;
    let registry = registry.with_process_lock(Arc::new(ProcessLock(process_lock)));
    registry.recover_writes()?;
    // Reconstruct uncertain execution reservations before accepting new work.
    // An operator must check backend idleness, clear holds, then restart to reclaim.
    let reserved: HashSet<String> = if config.tracked_turns {
        registry
            .list()?
            .into_iter()
            .filter(|u| u.hold.is_some())
            .map(|u| u.backend_id)
            .collect()
    } else {
        registry.reserved_http_backends()?.into_iter().collect()
    };
    let timeout = Duration::from_secs(config.request_timeout_seconds);
    let client = reqwest::Client::builder()
        .no_proxy()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .pool_max_idle_per_host(1)
        .build()?;
    let mut backends = HashMap::new();
    let mut unique_tokens = HashSet::new();
    for backend in &config.backends {
        let metadata = backend
            .token_file
            .symlink_metadata()
            .context("read backend token metadata")?;
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 4097,
            "backend token must be a bounded regular file"
        );
        let mut secret = String::new();
        File::open(&backend.token_file)?
            .take(4098)
            .read_to_string(&mut secret)
            .context("read backend token")?;
        let secret = secret.trim_end_matches(['\r', '\n']);
        anyhow::ensure!(
            (32..=4096).contains(&secret.len()) && secret.bytes().all(|b| b.is_ascii_graphic()),
            "backend token must contain 32..4096 visible ASCII bytes"
        );
        anyhow::ensure!(
            unique_tokens.insert(secret.to_owned()),
            "backend tokens must be distinct"
        );
        let mut token = axum::http::HeaderValue::from_str(&format!("Bearer {secret}"))?;
        token.set_sensitive(true);
        let url = reqwest::Url::parse(&backend.url)?;
        let check = client
            .get(url.join("health")?)
            .timeout(Duration::from_secs(10))
            .header(axum::http::header::AUTHORIZATION, token.clone())
            .send()
            .await
            .context("backend health check failed")?;
        if !check.status().is_success() {
            bail!("backend health check rejected");
        }
        let health =
            tokio::time::timeout(Duration::from_secs(10), proxy::read_response(check, 4096))
                .await
                .context("backend health check timed out")?
                .map_err(|()| anyhow::anyhow!("invalid backend health response"))?;
        let health: serde_json::Value =
            serde_json::from_slice(&health).context("invalid backend health JSON")?;
        anyhow::ensure!(
            health.get("agent_name").and_then(serde_json::Value::as_str) == Some(&backend.id),
            "backend identity does not match configured backend ID"
        );
        if config.scheduled_jobs {
            let response = client
                .get(url.join("internal/scheduler/status")?)
                .header(axum::http::header::AUTHORIZATION, token.clone())
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .context("backend scheduler handshake failed")?;
            anyhow::ensure!(
                response.status() == reqwest::StatusCode::OK,
                "backend scheduler mode unavailable"
            );
            anyhow::ensure!(
                response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.split(';').next())
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json")),
                "backend scheduler handshake must be JSON"
            );
            let bytes =
                tokio::time::timeout(Duration::from_secs(5), proxy::read_response(response, 4096))
                    .await
                    .context("scheduler handshake timed out")?
                    .map_err(|()| anyhow::anyhow!("invalid scheduler handshake"))?;
            scheduler::parse_status(&bytes, &backend.id)
                .map_err(|()| anyhow::anyhow!("backend scheduler contract mismatch"))?;
        }
        if config.tracked_turns {
            turns::check_backend(&client, &url, &token, &backend.id).await?;
        }
        backends.insert(
            backend.id.clone(),
            Backend {
                url,
                token,
                permit: Arc::new(Semaphore::new(usize::from(!reserved.contains(&backend.id)))),
                control: Arc::new(Semaphore::new(2)),
                streams: Arc::new(tokio::sync::Semaphore::new(2)),
            },
        );
    }
    let telegram =
        telegram::configure(&config, &registry, &client, &backends, &unique_tokens).await?;
    let slack = slack::configure(&config, &registry, &client, &backends, &unique_tokens).await?;
    let discord =
        discord::configure(&config, &registry, &client, &backends, &unique_tokens).await?;
    let feishu = feishu::configure(&config, &registry, &client, &backends, &unique_tokens).await?;
    let wecom = wecom::configure(&config, &registry, &client, &backends, &unique_tokens).await?;
    drop(unique_tokens);
    let capacity = config.max_in_flight;
    let reserved_count = reserved.len().min(capacity);
    let state = Arc::new(State {
        registry,
        admission: scheduler::Stop::new(),
        backends,
        client,
        permits: Arc::new(Semaphore::new(capacity - reserved_count)),
        timeout,
        control: Arc::new(Semaphore::new(8)),
        streams: Arc::new(tokio::sync::Semaphore::new(4)),
        scheduled_jobs: config.scheduled_jobs,
        tracked_turns: config.tracked_turns,
        reserved_turns: AtomicUsize::new(reserved_count),
        telegram,
        slack,
        discord,
        feishu,
        wecom,
    });
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .context("bind gateway")?;
    let app = Router::new()
        .route("/", get(index))
        .route("/ui/app.js", get(crate::ui::javascript))
        .route("/ui/turn-stream.js", get(crate::ui::turn_stream))
        .route("/ui/app.css", get(crate::ui::stylesheet))
        .route(
            "/health",
            get(|| async { axum::Json(serde_json::json!({"status":"ok"})) }),
        )
        .route(
            "/hooks/telegram/:binding_id",
            axum::routing::post(telegram::ingress),
        )
        .route(
            "/hooks/slack/:binding_id",
            axum::routing::post(slack::ingress),
        )
        .route(
            "/hooks/discord/:binding_id",
            axum::routing::post(discord::ingress),
        )
        .route(
            "/hooks/feishu/:binding_id",
            axum::routing::post(feishu::ingress),
        )
        .route(
            "/hooks/wecom/:binding_id",
            get(wecom::ingress).post(wecom::ingress),
        )
        .fallback(proxy::handle)
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            reject_after_close,
        ))
        .with_state(Arc::clone(&state));
    let scheduled = scheduler::start(Arc::clone(&state));
    let stop_scheduled = scheduled.stopper();
    let telegram = telegram::start(Arc::clone(&state));
    let stop_telegram = telegram.stopper();
    let slack = slack::start(Arc::clone(&state));
    let stop_slack = slack.stopper();
    let discord = discord::start(Arc::clone(&state));
    let stop_discord = discord.stopper();
    let feishu = feishu::start(Arc::clone(&state));
    let stop_feishu = feishu.stopper();
    let wecom = wecom::start(Arc::clone(&state));
    let stop_wecom = wecom.stopper();
    tracing::info!("isolated user gateway listening");
    let (signal, stopped) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = stopped.await;
        })
        .into_future();
    tokio::pin!(server);
    let finished = tokio::select! {
        result = &mut server => Some(result),
        () = shutdown() => None,
    };
    // One deadline starts at the signal, before any mutex or transport drain.
    // A slow downstream socket cannot postpone or reset a worker's grace.
    let deadline = tokio::time::Instant::now() + timeout + Duration::from_secs(5);
    state.admission.close();
    for stop in [
        stop_scheduled,
        stop_telegram,
        stop_slack,
        stop_discord,
        stop_feishu,
        stop_wecom,
    ] {
        stop.close();
    }
    let _ = signal.send(());
    tracing::info!("gateway closing admission; shared shutdown grace started");
    let remaining = || deadline.saturating_duration_since(tokio::time::Instant::now());
    let drain = async {
        let workers = async {
            tokio::join!(
                scheduled.shutdown(remaining()),
                telegram.shutdown(remaining()),
                slack.shutdown(remaining()),
                discord.shutdown(remaining()),
                feishu.shutdown(remaining()),
                wecom.shutdown(remaining()),
            )
        };
        let http = async {
            match finished {
                Some(result) => result,
                None => (&mut server).await,
            }
        };
        let (http, workers) = tokio::join!(http, workers);
        http?;
        if workers != (true, true, true, true, true, true) {
            tracing::warn!("gateway worker grace exhausted; original holds retained");
        }
        // Blocking control operations and admitted dispatch retain their actual
        // permits. Unknown reservations are not treated as successful execution.
        while state.permits.available_permits() + state.reserved_turns.load(Ordering::Acquire)
            < capacity
            || state.control.available_permits() != 8
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if workers == (true, true, true, true, true, true) {
            tracing::info!("gateway HTTP and worker drain completed");
        }
        Ok::<_, anyhow::Error>(())
    };
    match tokio::time::timeout_at(deadline, drain).await {
        Ok(result) => result?,
        Err(_) => tracing::warn!("gateway shutdown grace exhausted; original admissions and unknown holds require reconciliation; no replay"),
    }
    Ok(())
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;

    #[test]
    fn closing_admission_does_not_wait_for_entered_database_work_or_admit_a_later_write() {
        let root = std::env::temp_dir().join(format!("jiaclaw-close-{}", uuid::Uuid::new_v4()));
        let registry = Registry::open(&root.join("registry.db")).unwrap();
        let gate = scheduler::Stop::new();
        let (entered, ready) = std::sync::mpsc::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let first_gate = gate.clone();
        let first_registry = registry.clone();
        let first = std::thread::spawn(move || {
            first_gate.admit(|| {
                entered.send(()).unwrap();
                waiting.recv_timeout(Duration::from_secs(5)).unwrap();
                first_registry.add_user("already-entered").unwrap()
            })
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        // This must return before the actual DB owner is released below.
        gate.close();
        let later_gate = gate.clone();
        let later_registry = registry.clone();
        let later = std::thread::spawn(move || {
            later_gate.admit(|| later_registry.add_user("late").unwrap())
        });
        release.send(()).unwrap();
        assert!(first.join().unwrap().is_some());
        assert!(later.join().unwrap().is_none());
        assert_eq!(registry.list().unwrap().len(), 1);
        assert_eq!(registry.list().unwrap()[0].backend_id, "already-entered");
        drop(registry);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn cancelled_database_waiter_keeps_process_lease_until_actual_owner_settles() {
        let root = std::env::temp_dir().join(format!("jiaclaw-lease-{}", uuid::Uuid::new_v4()));
        let registry = Registry::open(&root.join("registry.db")).unwrap();
        let lock_path = root.join("process.lock");
        let file = private_file(&lock_path).unwrap();
        file.try_lock_exclusive().unwrap();
        let registry = registry.with_process_lock(Arc::new(ProcessLock(file)));
        let contender = private_file(&lock_path).unwrap();
        let actual_owner = registry.clone();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let (settled, completion) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                entered.send(()).unwrap();
                waiting.recv_timeout(Duration::from_secs(5)).unwrap();
                actual_owner.add_user("settled").unwrap();
                drop(actual_owner);
                settled.send(()).unwrap();
            })
            .await
            .unwrap();
        });
        ready.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        drop(registry);
        assert!(contender.try_lock_exclusive().is_err());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), completion)
            .await
            .unwrap()
            .unwrap();
        contender.try_lock_exclusive().unwrap();
        assert_eq!(
            Registry::open(&root.join("registry.db"))
                .unwrap()
                .list()
                .unwrap()
                .len(),
            1
        );
        FileExt::unlock(&contender).unwrap();
        drop(contender);
        std::fs::remove_dir_all(root).unwrap();
    }
}

async fn reject_after_close(
    axum::extract::State(state): axum::extract::State<Arc<State>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if state.admission.is_stopped() {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            [
                ("cache-control", "no-store"),
                ("x-content-type-options", "nosniff"),
            ],
            axum::Json(serde_json::json!({"error":"gateway shutting down"})),
        )
            .into_response();
    }
    next.run(request).await
}
async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
async fn index() -> axum::response::Response {
    use axum::response::IntoResponse;
    let body = include_str!("../../ui/index.html").replace(
        "未设置鉴权的本机服务可直接连接",
        "必须使用管理员签发的个人 API Key 连接",
    );
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    headers.insert("content-security-policy", axum::http::HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"));
    headers.insert(
        "x-content-type-options",
        axum::http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "referrer-policy",
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    response
}
