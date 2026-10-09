// Copyright 2026 JiaClaw contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Exercise cancellation at the actual `SQLite` dispatch boundary, without model I/O.
use super::*;

#[derive(Clone, Copy)]
enum Mutation {
    Commit,
    Import,
    Delete,
    RejectedCommit,
}

fn message(content: &str) -> ChatMessage {
    ChatMessage {
        role: MessageRole::User,
        content: content.into(),
    }
}

async fn mutate(state: AppState, mutation: Mutation) -> Result<(), AppError> {
    match mutation {
        Mutation::Commit | Mutation::RejectedCommit => {
            let guard = session_turn_lock(&state, "shared").await;
            commit_session_messages(
                &state,
                guard,
                "shared",
                vec![message("replacement")],
                "cancel-fixture",
                "http",
            )
            .await
        }
        Mutation::Import => import_session_handler(
            State(state),
            HeaderMap::new(),
            Query(ImportSessionQuery {
                id: Some("shared".into()),
                format: Some("json".into()),
                overwrite: true,
            }),
            Bytes::from_static(b"{\"messages\":[{\"role\":\"user\",\"content\":\"replacement\"}]}"),
        )
        .await
        .map(|_| ()),
        Mutation::Delete => {
            delete_session_handler(State(state), HeaderMap::new(), Path("shared".into()))
                .await
                .map(|_| ())
        }
    }
}

async fn dispatched(state: &AppState, owners: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&state.sessions) < owners {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real with_sessions worker must be dispatched");
}

async fn worker_finished(state: &AppState) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&state.sessions) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled waiter must not strand its actual SQLite worker");
}

fn sqlite_state() -> (AppState, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("jiaclaw-session-owner-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let database = root.join("sessions.sqlite3");
    let mut state = crate::tests::test_state_for_workspace(workspace);
    state.persist_enabled = true;
    state.persist_path = Arc::new(database.clone());
    let mut store = SessionStore::open(&database).unwrap();
    store
        .insert(
            "shared".into(),
            SessionRecord::new(vec![message("original")]),
        )
        .unwrap();
    state.sessions = Arc::new(Mutex::new(store));
    (state, root, database)
}

async fn cancelled_store_waiter(mutation: Mutation) {
    let (state, root, database) = sqlite_state();
    if matches!(mutation, Mutation::RejectedCommit) {
        if let SessionStore::Sqlite { conn, .. } = &*state.sessions.lock().unwrap() {
            conn.execute_batch("PRAGMA query_only=ON").unwrap();
        }
    }

    // Occupy the real store mutex on another thread. The asynchronous caller
    // still reaches spawn_blocking, so aborting it cannot remove that work.
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let locked_store = state.sessions.clone();
    let locker = std::thread::spawn(move || {
        let _store = locked_store.lock().unwrap();
        let _ = locked_tx.send(());
        let _ = release_rx.recv();
    });
    locked_rx.await.unwrap();
    let finishing_state = state.clone();
    // The persistence state and with_sessions dispatch each own another Arc.
    let owners = Arc::strong_count(&state.sessions) + 2;
    let waiter = tokio::spawn(mutate(finishing_state, mutation));
    dispatched(&state, owners).await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let owned_until_finished = tokio::time::timeout(
        Duration::from_millis(50),
        session_turn_lock(&state, "shared"),
    )
    .await
    .is_err();

    // Always release the fixture gate before asserting, including regressions.
    release_tx.send(()).unwrap();
    locker.join().unwrap();
    worker_finished(&state).await;
    let guard = tokio::time::timeout(Duration::from_secs(5), session_turn_lock(&state, "shared"))
        .await
        .expect("next turn must resume after successful or rejected storage");
    {
        let mut store = state.sessions.lock().unwrap();
        let history = store.get("shared").unwrap().map(|record| record.messages);
        match mutation {
            Mutation::Commit | Mutation::Import => {
                assert_eq!(history, Some(vec![message("replacement")]));
            }
            Mutation::Delete => assert!(history.is_none()),
            Mutation::RejectedCommit => assert_eq!(history, Some(vec![message("original")])),
        }
        if let SessionStore::Sqlite { conn, .. } = &*store {
            conn.execute_batch("PRAGMA query_only=OFF").unwrap();
        }
        store.remove("shared").unwrap();
        store.flush().unwrap();
    }
    drop(guard);
    drop(state);
    // The later deletion survives restart; there is no late writer to resurrect it.
    assert!(SessionStore::open(&database)
        .unwrap()
        .get("shared")
        .unwrap()
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
    assert!(owned_until_finished, "cancelled waiter released its session turn while dispatched SQLite work could still mutate history");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_chat_commit_keeps_turn_until_sqlite_resolves() {
    cancelled_store_waiter(Mutation::Commit).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_http_import_keeps_turn_until_sqlite_resolves() {
    cancelled_store_waiter(Mutation::Import).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_http_delete_keeps_turn_until_sqlite_resolves() {
    cancelled_store_waiter(Mutation::Delete).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_rejected_commit_releases_turn_without_changing_history() {
    cancelled_store_waiter(Mutation::RejectedCommit).await;
}

#[test]
fn cancelled_worker_queued_on_blocking_pool_keeps_turn_until_storage_finishes() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (state, root, database) = sqlite_state();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let occupied = tokio::task::spawn_blocking(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
        });
        entered_rx.await.unwrap();
        let finishing_state = state.clone();
        let owners = Arc::strong_count(&state.sessions) + 2;
        let waiter = tokio::spawn(mutate(finishing_state, Mutation::Commit));
        dispatched(&state, owners).await;
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(
            state.sessions.try_lock().is_ok(),
            "SQLite worker is queued and has not entered the store"
        );
        let owned_while_queued = tokio::time::timeout(
            Duration::from_millis(50),
            session_turn_lock(&state, "shared"),
        )
        .await
        .is_err();
        release_tx.send(()).unwrap();
        occupied.await.unwrap();
        worker_finished(&state).await;
        let guard = session_turn_lock(&state, "shared").await;
        assert_eq!(
            state
                .sessions
                .lock()
                .unwrap()
                .get("shared")
                .unwrap()
                .unwrap()
                .messages,
            vec![message("replacement")]
        );
        drop(guard);
        drop(state);
        assert_eq!(
            SessionStore::open(&database)
                .unwrap()
                .get("shared")
                .unwrap()
                .unwrap()
                .messages,
            vec![message("replacement")]
        );
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            owned_while_queued,
            "queued work must retain the turn even before it starts"
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_running_sqlite_commit_keeps_turn_while_database_is_busy() {
    let (state, root, database) = sqlite_state();
    let writer = rusqlite::Connection::open(&database).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let finishing_state = state.clone();
    let owners = Arc::strong_count(&state.sessions) + 2;
    let waiter = tokio::spawn(mutate(finishing_state, Mutation::Commit));
    dispatched(&state, owners).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let busy = match state.sessions.try_lock() {
                Err(std::sync::TryLockError::WouldBlock) => true,
                Err(error) => panic!("unexpected store mutex failure: {error}"),
                Ok(_) => false,
            };
            if busy {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual worker must enter SQLite while the other connection owns the writer");
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let owned_while_busy = tokio::time::timeout(
        Duration::from_millis(50),
        session_turn_lock(&state, "shared"),
    )
    .await
    .is_err();
    writer.execute_batch("COMMIT").unwrap();
    drop(writer);
    worker_finished(&state).await;
    let guard = session_turn_lock(&state, "shared").await;
    assert_eq!(
        state
            .sessions
            .lock()
            .unwrap()
            .get("shared")
            .unwrap()
            .unwrap()
            .messages,
        vec![message("replacement")]
    );
    drop(guard);
    drop(state);
    assert_eq!(
        SessionStore::open(&database)
            .unwrap()
            .get("shared")
            .unwrap()
            .unwrap()
            .messages,
        vec![message("replacement")]
    );
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        owned_while_busy,
        "database contention must not release a cancelled turn prematurely"
    );
}
