//! The core "no event cross-talk" guarantee, exercised against a REAL daemon
//! with two real registered databases and two independently bound
//! `watcher::run` tasks (mirrors `optimistic_echo_race_test.rs`'s "two
//! windows = two independent `GrpcClient`s" pattern, generalized to two
//! independent *databases* too). Two mock `WebviewWindow`s are pinned, via
//! `WindowDatabaseRegistry`, to the two databases; `watcher::forward` now
//! routes through `window_routing::emit_routed` instead of an unconditional
//! `app.emit` broadcast, so a `node:created` event tagged with database A's
//! id must reach ONLY the window pinned to database A.
//!
//! Production today spawns exactly one `watcher::run`, bound to the single
//! shared `GrpcClient`'s "active" database (see `lib.rs`'s `setup()`) — the
//! app has no way to open a second window yet, so nothing wires up a second,
//! independently-bound watcher for it. This test proves the ROUTING layer
//! this change delivers is correct for when that wiring lands (a follow-up),
//! using two `watcher::run` tasks the way two real concurrent windows'
//! watchers eventually would drive them — not a mock of the routing logic,
//! the actual production `watcher::run` + `window_routing::emit_routed` path
//! against a real daemon.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nodespace_app_lib::commands::database::create_database;
use nodespace_app_lib::commands::nodes::{create_node, CreateNodeInput};
use nodespace_app_lib::watcher;
use nodespace_app_lib::window_routing::WindowDatabaseRegistry;
use nodespace_app_test_support::{
    hold_connect_mutex_and_socket_env, SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT,
};
use serde_json::json;
use tauri::{Listener, Manager};
use tokio_util::sync::CancellationToken;

fn text_input(id: &str, content: &str) -> CreateNodeInput {
    CreateNodeInput {
        id: id.to_string(),
        node_type: "text".to_string(),
        content: content.to_string(),
        parent_id: None,
        insert_position: None,
        properties: json!({}),
    }
}

/// Poll `events` for `id` up to `timeout`. Polling (not a one-shot recv)
/// because delivery is asynchronous relative to the RPC that triggered it.
async fn wait_for(events: &Arc<Mutex<Vec<String>>>, id: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if events.lock().unwrap().iter().any(|e| e == id) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn events_from_one_database_never_reach_a_window_pinned_to_another() {
    let daemon = SpawnedDaemon::spawn();

    // Everything below shares ONE mock app/event bus — `window_a`'s — since
    // `WindowDatabaseRegistry` and `emit_routed`'s window lookups are
    // per-app, exactly like the real desktop app's single Tauri process
    // hosting every window. Each simulated window still gets its own
    // independent `GrpcClient` (own `x-ns-client-id`), because THAT is what a
    // real second window would also have.
    let window_a = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state_a = window_a.client_state();
    let window_b_client = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state_b = window_b_client.client_state();

    let db_a = create_database(state_a.clone(), "cross-talk-db-a".to_string(), None)
        .await
        .expect("create database A failed");
    let db_b = create_database(state_b.clone(), "cross-talk-db-b".to_string(), None)
        .await
        .expect("create database B failed");

    state_a.set_active_database(Some(db_a.id.clone())).await;
    state_b.set_active_database(Some(db_b.id.clone())).await;

    window_a.app.manage(WindowDatabaseRegistry::default());
    let win_a = tauri::WebviewWindowBuilder::new(&window_a.app, "win-a", Default::default())
        .build()
        .expect("failed to build mock window a");
    let win_b = tauri::WebviewWindowBuilder::new(&window_a.app, "win-b", Default::default())
        .build()
        .expect("failed to build mock window b");
    let registry = window_a.app.state::<WindowDatabaseRegistry>();
    registry.pin("win-a", &db_a.id);
    registry.pin("win-b", &db_b.id);

    // A THIRD independent client, connected NOW — before the
    // `hold_connect_mutex_and_socket_env` guard below is acquired.
    // `CONNECT_MUTEX` (which that guard holds for the watchers' lifetime) is
    // a plain non-reentrant `tokio::sync::Mutex`; `TauriTestApp::connect`
    // acquires it too, so calling `connect` again while already holding the
    // guard would deadlock this task against itself. This writer makes the
    // genuinely foreign write into database A that window A's watcher (an
    // independent connection) must observe — window A's own connection can't
    // do it, since the ADR-026 C5 extension suppresses a connection's own
    // writes on its own WatchNodes stream.
    let writer = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let writer_state = writer.client_state();
    writer_state
        .set_active_database(Some(db_a.id.clone()))
        .await;

    let handle = window_a.handle();
    let _socket_guard = hold_connect_mutex_and_socket_env(&daemon).await;

    let cancel_a = CancellationToken::new();
    let cancel_b = CancellationToken::new();
    let watcher_a = tokio::spawn(watcher::run(
        handle.clone(),
        (*state_a).clone(),
        cancel_a.child_token(),
    ));
    let watcher_b = tokio::spawn(watcher::run(
        handle.clone(),
        (*state_b).clone(),
        cancel_b.child_token(),
    ));
    // Give both watchers a moment to open their WatchNodes streams before the
    // write below, so the broadcast isn't lost to a stream that hasn't opened.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let received_a: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_b: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let ra = received_a.clone();
    let rb = received_b.clone();
    win_a.listen("node:created", move |event| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            if let Some(id) = v["id"].as_str() {
                ra.lock().unwrap().push(id.to_string());
            }
        }
    });
    win_b.listen("node:created", move |event| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            if let Some(id) = v["id"].as_str() {
                rb.lock().unwrap().push(id.to_string());
            }
        }
    });

    // The third client (connected earlier, before the socket-env guard) now
    // makes the actual foreign write into database A.
    let node_in_a = uuid::Uuid::new_v4().to_string();
    create_node(
        writer_state.clone(),
        text_input(&node_in_a, "lives in db a"),
    )
    .await
    .expect("create_node in db a failed");

    assert!(
        wait_for(&received_a, &node_in_a, Duration::from_secs(10)).await,
        "window A (pinned to db A) must observe db A's own event"
    );
    // Give window B's (wrong-database) listener every opportunity it would
    // need to wrongly receive the event too, if routing were broken.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        received_b.lock().unwrap().is_empty(),
        "window B (pinned to db B) must NEVER see db A's event — no cross-talk"
    );

    cancel_a.cancel();
    cancel_b.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher_a).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher_b).await;
}
