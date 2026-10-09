// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn canceled_stream_waiter_keeps_admission_and_commits_only_after_done() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().canonicalize().unwrap().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                let size: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + size {
                    assert!(headers.contains("accept: text/event-stream"));
                    assert_eq!(
                        serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + size]).unwrap()
                            ["stream"],
                        true
                    );
                    break;
                }
            }
            assert!(bytes.len() < 8192);
        }
        let prefix = format!(
            "data: {}\n\n",
            json!({"id":"fixture","created":1,"object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{"role":"assistant","content":"preview"},"finish_reason":null}]})
        );
        let suffix = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({"id":"fixture","created":1,"object":"chat.completion.chunk","model":"fixture","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
            json!({"id":"fixture","created":1,"object":"chat.completion.chunk","model":"fixture","choices":[],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}})
        );
        let remote = Uuid::new_v4();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nx-brokerrouter-request-id: {remote}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",prefix.len()+suffix.len()).as_bytes()).await.unwrap();
        socket.write_all(prefix.as_bytes()).await.unwrap();
        released.await.unwrap();
        socket.write_all(suffix.as_bytes()).await.unwrap();
    });
    let mut config = AgentConfig {
        workspace_path: workspace,
        ..AgentConfig::default()
    };
    config.provider.provider_type = "brokerrouter".into();
    config.provider.base_url = endpoint;
    config.provider.api_key = Some("fixture-key".into());
    config.model_calls.enabled = true;
    let ledger = ModelCalls::open(&config).await.unwrap().unwrap();
    let (progress, mut events) = ChatProgress::channel().unwrap();
    let prepared = ledger
        .prepare_stream(
            "fixture",
            &[WireMessage::text("user", "fixture".into())],
            0.0,
            10,
            &[],
        )
        .unwrap();
    let service = ledger.clone();
    let waiter = tokio::spawn(async move {
        service
            .complete_stream(
                prepared,
                Uuid::new_v4().to_string(),
                ModelPurpose::Chat,
                None,
                0,
                progress,
            )
            .await
    });
    let event = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .unwrap()
        .unwrap();
    let ChatProgressEvent::ModelStarted {
        operation_id,
        remote_id,
        ..
    } = event
    else {
        panic!("identity first")
    };
    let state = ledger.status().await.unwrap();
    assert_eq!(state["pending"]["id"], operation_id);
    assert_eq!(state["pending"]["remote_id"], remote_id);
    assert_eq!(state["pending"]["streaming"], true);
    assert_eq!(state["retained_receipts"], 0);
    assert!(matches!(
        events.next().await,
        Some(ChatProgressEvent::Preview { .. })
    ));
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    drop(events);
    assert!(
        ledger.admission.try_acquire().is_err(),
        "real worker, not waiter, owns admission"
    );
    assert_eq!(ledger.status().await.unwrap()["retained_receipts"], 0);
    release.send(()).unwrap();
    server.await.unwrap();
    ledger.settle(Duration::from_secs(5)).await.unwrap();
    assert!(ledger.status().await.unwrap()["pending"].is_null());
    assert_eq!(
        ledger.result(operation_id).await.unwrap()["choices"][0]["message"]["content"],
        "preview"
    );
}

#[tokio::test]
async fn canceled_waiter_retains_exclusive_store_until_receipt_is_committed() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().canonicalize().unwrap().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (headers_sent, headers_received) = tokio::sync::oneshot::channel();
    let (release, body_released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let count = socket.read(&mut chunk).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                let length: usize = header
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
            assert!(bytes.len() < 8192);
        }
        let receipt = json!({"model":"fixture","choices":[{"index":0,"message":{"role":"assistant","content":"committed after cancellation"},"finish_reason":"stop"}]}).to_string();
        let remote = Uuid::new_v4();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nx-brokerrouter-request-id: {remote}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", receipt.len()).as_bytes()).await.unwrap();
        headers_sent.send(()).unwrap();
        body_released.await.unwrap();
        socket.write_all(receipt.as_bytes()).await.unwrap();
    });
    let mut config = AgentConfig {
        workspace_path: workspace,
        ..AgentConfig::default()
    };
    config.provider.provider_type = "brokerrouter".into();
    config.provider.base_url = endpoint;
    config.provider.api_key = Some("disposable-fixture-key".into());
    config.model_calls.enabled = true;
    let ledger = ModelCalls::open(&config).await.unwrap().unwrap();
    let prepared = ledger
        .prepare(
            "fixture",
            &[WireMessage::text("user", "fixture".into())],
            0.0,
            10,
            &[],
        )
        .unwrap();
    let service = Arc::clone(&ledger);
    let waiter = tokio::spawn(async move {
        service
            .complete(
                prepared,
                Uuid::new_v4().to_string(),
                ModelPurpose::Chat,
                None,
                0,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), headers_received)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = ledger.status().await.unwrap();
            if status["pending"]["remote_id"].is_string() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    drop(ledger);
    assert!(
        ModelCalls::open(&config).await.is_err(),
        "worker must retain the exclusive store after the caller and owner are dropped"
    );
    release.send(()).unwrap();
    server.await.unwrap();
    let reopened = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(Some(store)) = ModelCalls::open(&config).await {
                break store;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let status = reopened.status().await.unwrap();
    assert!(status["pending"].is_null());
    assert_eq!(status["recent"][0]["state"], "completed");
    assert_eq!(status["retained_receipts"], 1);
    let id = status["recent"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        reopened.result(id).await.unwrap()["choices"][0]["message"]["content"],
        "committed after cancellation"
    );
}
