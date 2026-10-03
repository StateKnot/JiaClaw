// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Deterministic service admission/ownership tests; no provider requests.

use super::*;
use jiaclaw_core::{ProviderConfig, SemanticMemoryConfig};
use std::{process::Command, sync::mpsc};
use tokio::sync::oneshot;

const REMOTE_ID: &str = "10203040-5060-4070-8090-102030405060";
const ISOLATED_TEST: &str =
    "semantic::lifecycle_tests::database_finalization_survives_saturated_file_io";
const CHILD_ENV: &str = "JIACLAW_TEST_SEMANTIC_IO_ISOLATED";

fn service() -> (tempfile::TempDir, Arc<SemanticMemory>) {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let configuration = SemanticMemoryConfig {
        enabled: true,
        model: "local-fixture".into(),
        space_revision: "test-v1".into(),
        dimensions: 2,
        timeout_secs: 1,
        ..SemanticMemoryConfig::default()
    };
    let transport = Transport::new(
        &ProviderConfig {
            base_url: "http://127.0.0.1:9".into(),
            api_key: Some("fixture-only-no-network".into()),
            ..ProviderConfig::default()
        },
        &configuration,
    )
    .unwrap();
    let store = Store::open(&workspace, &configuration.index_path).unwrap();
    let service = SemanticMemory {
        workspace,
        sources: vec!["MEMORY.md".into()],
        dimensions: 2,
        timeout_secs: 1,
        space: digest(format!("{}:{CHUNKER}", transport.fingerprint())),
        credential: transport.credential_hash(),
        transport,
        store: Arc::new(store),
        admission: Arc::new(tokio::sync::Semaphore::new(1)),
        persistence: Arc::new(tokio::sync::Semaphore::new(1)),
    };
    (temporary, Arc::new(service))
}

async fn begin(service: &SemanticMemory) -> String {
    let space = service.space.clone();
    let credential = service.credential.clone();
    service
        .database(move |store| {
            store.begin_operation("query", &space, &credential, &"a".repeat(64), 1)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn cancelled_waiter_keeps_admission_and_store_ownership_until_worker_finishes() {
    let (_temporary, service) = service();
    let workspace = service.workspace.clone();
    let index_path = SemanticMemoryConfig::default().index_path;
    let weak = Arc::downgrade(&service);
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (finished_tx, finished_rx) = oneshot::channel();
    let worker_service = Arc::clone(&service);
    let waiter = tokio::spawn(async move {
        worker_service
            .admitted(move |service| async move {
                let id = begin(&service).await;
                let _ = entered_tx.send(id.clone());
                release_rx
                    .await
                    .map_err(|_| error("fixture release was cancelled"))?;
                service
                    .database(move |store| {
                        store.complete_operation(&id, Some(REMOTE_ID), &[vec![1.0, 0.0]])
                    })
                    .await?;
                let _ = finished_tx.send(());
                Ok(())
            })
            .await
    });
    let operation = tokio::time::timeout(Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(service
        .admitted(|_| async { Ok(()) })
        .await
        .unwrap_err()
        .to_string()
        .contains("busy"));
    drop(service);
    assert!(
        weak.upgrade().is_some(),
        "detached worker must retain its service"
    );
    assert!(Store::open(&workspace, &index_path).is_err());
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), finished_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let reopened = Store::open(&workspace, &index_path).unwrap();
    assert!(reopened.pending().unwrap().is_none());
    assert_eq!(
        reopened.receipt(&operation).unwrap(),
        Some(vec![vec![1.0, 0.0]])
    );
}

// The child runs this exact test alone, so holding the real global eight-slot
// file executor cannot make unrelated parallel Rust tests spuriously busy.
#[test]
fn database_finalization_survives_saturated_file_io() {
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", ISOLATED_TEST, "--test-threads=1", "--nocapture"])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
            "isolated persistence test failed or did not run:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (_temporary, service) = service();
            let (entered_tx, entered_rx) = oneshot::channel();
            let (release_tx, release_rx) = oneshot::channel();
            let worker_service = Arc::clone(&service);
            let worker = tokio::spawn(async move {
                worker_service
                    .admitted(move |service| async move {
                        let id = begin(&service).await;
                        let _ = entered_tx.send(id.clone());
                        release_rx
                            .await
                            .map_err(|_| error("fixture release was cancelled"))?;
                        service
                            .database(move |store| {
                                store.complete_operation(&id, Some(REMOTE_ID), &[vec![1.0, 0.0]])
                            })
                            .await
                    })
                    .await
            });
            let operation = tokio::time::timeout(Duration::from_secs(5), entered_rx)
                .await
                .unwrap()
                .unwrap();
            // On unwind the senders are dropped before runtime destruction,
            // releasing all blocking workers even when an assertion fails.
            let mut releases = Vec::new();
            let mut jobs = Vec::new();
            for _ in 0..8 {
                let (started_tx, started_rx) = oneshot::channel();
                let (unblock_tx, unblock_rx) = mpsc::channel::<()>();
                releases.push(unblock_tx);
                jobs.push(tokio::spawn(memory_io::run_blocking(move || {
                    let _ = started_tx.send(());
                    let _ = unblock_rx.recv();
                    Ok(())
                })));
                tokio::time::timeout(Duration::from_secs(5), started_rx)
                    .await
                    .unwrap()
                    .unwrap();
            }
            let busy = memory_io::run_blocking(|| Ok(())).await.unwrap_err();
            assert!(busy.to_string().contains("capacity busy"));
            release_tx.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), worker)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let receipt = service
                .database(move |store| {
                    assert!(store.pending()?.is_none());
                    store.receipt(&operation)
                })
                .await
                .unwrap();
            assert_eq!(receipt, Some(vec![vec![1.0, 0.0]]));
            assert!(memory_io::run_blocking(|| Ok(())).await.is_err());
            releases.clear();
            for job in jobs {
                job.await.unwrap().unwrap();
            }
            memory_io::run_blocking(|| Ok(())).await.unwrap();
            service.admitted(|_| async { Ok(()) }).await.unwrap();
        });
}
