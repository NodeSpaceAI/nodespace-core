//! The watcher on the client the app really gives it: a channel held until a
//! start attempt has found this app's daemon answering, which then keeps
//! working through a daemon that goes away and comes back.
//!
//! Each test runs a real headless `nodespaced` (the `SpawnedDaemon` fixture of
//! the integration tests) on a socket in a temporary directory, and builds the
//! client with `GrpcClient::lazy_at` and a `StartupHold` exactly as
//! `GrpcClient::connect_lazy_held` does, but on that socket instead of the
//! one named by the environment. That keeps the tests off the process-global
//! socket variable the integration tests have to serialise on.
//!
//! The fixture crate depends on this one, so its `GrpcClient` is a different
//! type from the one these tests use; only its daemon process handling is
//! used here.
//!
//! The watcher reconnects with a backoff capped at `BACKOFF_MAX`, so the
//! tests wait for delivery for as long as that cap allows, derived from the
//! constant, and for no longer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nodespace_app_test_support::{SpawnedDaemon, DAEMON_CONNECT_TIMEOUT};
use serde_json::{json, Value};
use tauri::{Listener, Manager};
use tokio_util::sync::CancellationToken;

use super::{run, BACKOFF_MAX};
use crate::commands::nodes::{create_node, CreateNodeInput};
use crate::daemon_setup::{wait_for_daemon, DaemonStatus};
use crate::services::startup_hold::{StartupHold, STARTUP_HOLD_LIMIT};
use crate::services::{GrpcClient, StartupHoldState};

/// How long a watcher that lost its daemon is given to deliver a write made
/// after the daemon is answering again: one full backoff wait at the cap to
/// notice, one more for an attempt that lands just before the daemon is up,
/// then a connection and a round trip.
fn reconnect_deadline() -> Duration {
    BACKOFF_MAX * 2 + Duration::from_secs(10)
}

/// Long enough for a task that is going to run to have run; no assertion
/// depends on it being long enough, only on it not being too short to let a
/// call that wrongly went out do so.
const SETTLE: Duration = Duration::from_millis(500);

type Seen = Arc<Mutex<Vec<String>>>;

/// Records the id of every `node:created` the app handle emits.
fn record_created(handle: &tauri::AppHandle<tauri::test::MockRuntime>) -> Seen {
    let seen = Seen::default();
    let sink = Arc::clone(&seen);
    handle.listen("node:created", move |event| {
        if let Ok(payload) = serde_json::from_str::<Value>(event.payload()) {
            if let Some(id) = payload["id"].as_str() {
                sink.lock().unwrap().push(id.to_string());
            }
        }
    });
    seen
}

fn has_seen(seen: &Seen, id: &str) -> bool {
    seen.lock().unwrap().iter().any(|seen| seen == id)
}

fn text_node(id: &str) -> CreateNodeInput {
    CreateNodeInput {
        id: id.to_string(),
        node_type: "text".to_string(),
        content: id.to_string(),
        parent_id: None,
        insert_position: None,
        properties: json!({}),
        collection_id: None,
    }
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Starts a daemon on `socket` and waits until it answers.
async fn start_daemon(socket: &Path) -> SpawnedDaemon {
    let daemon = SpawnedDaemon::spawn_with_socket(
        tempfile::tempdir().expect("a home for the daemon"),
        socket.to_path_buf(),
    );
    let status = wait_for_daemon(socket, DAEMON_CONNECT_TIMEOUT).await;
    assert_eq!(status, DaemonStatus::Healthy, "the daemon never answered");
    daemon
}

fn socket_in(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("daemon.sock")
}

/// A watcher on `client`, emitting to `app`.
fn start_watcher(
    app: &tauri::App<tauri::test::MockRuntime>,
    client: &GrpcClient,
    cancel: &CancellationToken,
) -> tokio::task::JoinHandle<anyhow::Result<()>> {
    tokio::spawn(run(
        app.handle().clone(),
        client.clone(),
        cancel.child_token(),
    ))
}

async fn stop_watcher(
    cancel: CancellationToken,
    watcher: tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher).await;
}

/// A write the frontend makes while the channel is held waits on the hold, and
/// once the hold is released the watcher that was waiting on it too delivers
/// that very write.
#[tokio::test]
async fn a_write_made_during_the_startup_hold_reaches_the_watcher_once_released() {
    let socket_dir = tempfile::tempdir().expect("a directory for the socket");
    let socket = socket_in(&socket_dir);
    let _daemon = start_daemon(&socket).await;

    // The watcher's window, and another window of the app whose channel is
    // held by the same latch. The watcher's own window would not see its own
    // writes: the daemon drops the echoes of a window's writes from its
    // stream.
    let hold = StartupHold::holding(STARTUP_HOLD_LIMIT);
    let app = tauri::test::mock_app();
    app.manage(GrpcClient::lazy_at(&socket, hold.clone()));
    let state = app.state::<GrpcClient>();
    let other = tauri::test::mock_app();
    other.manage(GrpcClient::lazy_at(&socket, hold));
    let other_state = other.state::<GrpcClient>();
    let seen = record_created(app.handle());

    let cancel = CancellationToken::new();
    let watcher = start_watcher(&app, &state, &cancel);
    // The watcher asks for its stream first, as it does at app start, and so
    // is first in line on the hold.
    tokio::time::sleep(SETTLE).await;

    let held_id = new_id();
    let write = create_node(other_state.clone(), text_node(&held_id));
    tokio::pin!(write);
    tokio::select! {
        result = &mut write => panic!("a write went out while the channel was held: {result:?}"),
        _ = tokio::time::sleep(SETTLE) => {}
    }
    assert_eq!(other_state.startup_hold(), StartupHoldState::Holding);
    assert!(!has_seen(&seen, &held_id));

    other_state.release_startup_hold();
    write
        .await
        .expect("the held write goes out once the hold is released");

    let deadline = tokio::time::Instant::now() + reconnect_deadline();
    while !has_seen(&seen, &held_id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the write made during the hold never reached the watcher; saw {:?}",
            seen.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    stop_watcher(cancel, watcher).await;
}

/// A daemon that is killed and started again, as the service manager does,
/// does not end the watcher: it reconnects and delivers a write made after the
/// restart.
#[tokio::test]
async fn a_watcher_resumes_after_the_daemon_is_killed_and_restarted() {
    let socket_dir = tempfile::tempdir().expect("a directory for the socket");
    let socket = socket_in(&socket_dir);
    let first = start_daemon(&socket).await;

    let app = tauri::test::mock_app();
    let client = GrpcClient::lazy_at(&socket, StartupHold::holding(STARTUP_HOLD_LIMIT));
    app.manage(client.clone());
    let seen = record_created(app.handle());
    client.release_startup_hold();

    let cancel = CancellationToken::new();
    let watcher = start_watcher(&app, &client, &cancel);

    // The stream is live: a write reaches it. Writes made before the watcher
    // has subscribed are not replayed, so write until one is seen.
    let writer = tauri::test::mock_app();
    writer.manage(GrpcClient::lazy_at(&socket, StartupHold::released()));
    let mut before = Vec::new();
    let deadline = tokio::time::Instant::now() + reconnect_deadline();
    loop {
        let id = new_id();
        create_node(writer.state::<GrpcClient>(), text_node(&id))
            .await
            .expect("a write to the first daemon");
        before.push(id);
        if before.iter().any(|id| has_seen(&seen, id)) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the watcher never delivered a write from the first daemon"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    drop(first);
    let _second = start_daemon(&socket).await;

    // Only writes made after the restart count.
    let writer = tauri::test::mock_app();
    writer.manage(GrpcClient::lazy_at(&socket, StartupHold::released()));
    let mut after = Vec::new();
    let deadline = tokio::time::Instant::now() + reconnect_deadline();
    loop {
        let id = new_id();
        create_node(writer.state::<GrpcClient>(), text_node(&id))
            .await
            .expect("a write to the restarted daemon");
        after.push(id);
        if after.iter().any(|id| has_seen(&seen, id)) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the watcher never delivered a write made after the daemon restarted; saw {:?}",
            seen.lock().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    stop_watcher(cancel, watcher).await;
}
