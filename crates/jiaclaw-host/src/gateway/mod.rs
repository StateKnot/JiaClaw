// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT
//! Authentication boundary for independently isolated `JiaClaw` backends.
pub(crate) mod cli;
mod config;
mod keys;
mod proxy;
mod registry;
mod scheduler;

use anyhow::{bail, Context, Result};
use axum::{routing::get, Router};
use config::Config;
use fs2::FileExt;
use registry::Registry;
use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    io::Read,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

struct Backend {
    url: reqwest::Url,
    token: axum::http::HeaderValue,
    permit: Arc<Semaphore>,
    control: Arc<Semaphore>,
}
struct State {
    registry: Registry,
    backends: HashMap<String, Backend>,
    client: reqwest::Client,
    permits: Arc<Semaphore>,
    timeout: Duration,
    control: Arc<Semaphore>,
    scheduled_jobs: bool,
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

pub(super) async fn serve(config: Config) -> Result<()> {
    config.validate()?;
    let registry = Registry::open(&config.registry_path)?;
    let lock_path = config.registry_path.with_extension("gateway.lock");
    let process_lock = private_file(&lock_path)?;
    process_lock
        .try_lock_exclusive()
        .context("another gateway is using this registry")?;
    registry.recover_writes()?;
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
        backends.insert(
            backend.id.clone(),
            Backend {
                url,
                token,
                permit: Arc::new(Semaphore::new(1)),
                control: Arc::new(Semaphore::new(2)),
            },
        );
    }
    drop(unique_tokens);
    let capacity = u32::try_from(config.max_in_flight).context("gateway capacity overflow")?;
    let state = Arc::new(State {
        registry,
        backends,
        client,
        permits: Arc::new(Semaphore::new(config.max_in_flight)),
        timeout,
        control: Arc::new(Semaphore::new(8)),
        scheduled_jobs: config.scheduled_jobs,
    });
    let listener = tokio::net::TcpListener::bind(&config.bind)
        .await
        .context("bind gateway")?;
    let app = Router::new()
        .route("/", get(index))
        .route("/ui/app.js", get(crate::ui::javascript))
        .route("/ui/app.css", get(crate::ui::stylesheet))
        .route(
            "/health",
            get(|| async { axum::Json(serde_json::json!({"status":"ok"})) }),
        )
        .fallback(proxy::handle)
        .with_state(Arc::clone(&state));
    let scheduled = scheduler::start(Arc::clone(&state));
    let stop_scheduled = scheduled.stopper();
    tracing::info!("isolated user gateway listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown().await;
            stop_scheduled.stop();
        })
        .await?;
    let _ = scheduled.shutdown(timeout + Duration::from_secs(5)).await;
    // Detached admitted work retains its permit even after its caller disconnects.
    // On forced shutdown the durable hold remains for startup recovery.
    let _drain = tokio::time::timeout(
        timeout + Duration::from_secs(5),
        state.permits.clone().acquire_many_owned(capacity),
    )
    .await;
    drop(process_lock);
    Ok(())
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
