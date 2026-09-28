//! ADR-048 priority flow 2 — the optimistic-edit-vs-echo race, against the
//! REAL live event path (`watcher::run`, wired up in `lib.rs` at startup —
//! its own module doc previously (and incorrectly) claimed it was inert;
//! confirmed by grep that it is the sole emitter of `node:*` events and is
//! unconditionally spawned).
//!
//! This is the regression class ADR-048 exists to catch: the frontend
//! applies an edit optimistically to local state, and a broadcast for the
//! SAME node arrives over the watch stream out of order — or carrying a
//! value the user has already typed past — and it must not overwrite newer
//! local state. Before the ADR-026 C5 extension (daemon-side same-origin
//! echo suppression), a single window's own write always echoed back to
//! itself, so this scenario was reachable purely from one client's own
//! traffic. Under the C5 extension the daemon never echoes a connection's
//! own writes back to it at all, so the only way this race remains reachable
//! is a genuinely foreign write — a SECOND window/client editing the same
//! node — landing on the first window's `WatchNodes` stream while its own
//! newer write is in flight. This file drives exactly that: two independent
//! `GrpcClient`s (via two `TauriTestApp::connect` calls against the same
//! daemon, each generating its own stable `x-ns-client-id`), so window A's
//! watcher observes window B's write as the late/out-of-order echo. A
//! synchronous mock cannot reproduce this: the race is between two
//! independent clocks (window A's local optimistic write and window B's
//! asynchronous broadcast), and a mock collapses them onto one tick. Here,
//! `watcher::run` is driven for real against a real daemon, emitting real
//! `node:updated` events on a real Tauri event bus (`tauri::test`'s
//! `MockRuntime` — no webview, but a real `Emitter`/`Listener`
//! implementation, not a stand-in).
//!
//! `watcher::run` was made generic over `tauri::Runtime` (previously
//! hardcoded to the real `Wry` runtime, like the rest of the module) SOLELY
//! so this test can drive the actual production function against
//! `MockRuntime` — not a reimplementation of its forwarding logic.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nodespace_app_lib::commands::nodes::{create_node, update_node, CreateNodeInput};
use nodespace_app_lib::types::NodeUpdate;
use nodespace_app_lib::watcher;
use nodespace_app_test_support::{
    hold_connect_mutex_and_socket_env, SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT,
};
use serde_json::json;
use tauri::Listener;
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

/// Wait until at least `count` `node:updated` events carrying `id` have been
/// observed, or panic after `timeout`. Polling rather than a one-shot recv
/// because the watcher's stream delivery is asynchronous relative to the
/// RPC call that triggered it — the whole point of this test.
async fn wait_for_updates(
    events: &Arc<Mutex<Vec<String>>>,
    node_id: &str,
    count: usize,
    timeout: Duration,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let matched = events
            .lock()
            .unwrap()
            .iter()
            .filter(|id| *id == node_id)
            .count();
        if matched >= count {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for {count} node:updated event(s) for {node_id}; saw {matched}"
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The core regression: window A watches a node while window B (a genuinely
/// different `GrpcClient`/`x-ns-client-id`, simulating a second desktop
/// window) makes two real updates through the command layer. Window A's REAL
/// watcher (real gRPC WatchNodes stream, real Tauri event bus) must observe
/// both of window B's writes — and by the time both broadcasts have arrived,
/// the authoritative source (`get_node`) must reflect window B's LATEST
/// write, regardless of any transient reordering. Before the ADR-026 C5
/// extension this scenario was reachable with a single client (its own write
/// echoed back to itself); the daemon now suppresses a connection's own
/// echoes, so a second, independently-connected client is required to
/// reproduce it — see this file's module doc for the full rationale.
#[tokio::test]
async fn newer_local_write_is_not_clobbered_by_a_late_echo_of_an_older_write() {
    let daemon = SpawnedDaemon::spawn();
    // TauriTestApp::connect briefly acquires and releases CONNECT_MUTEX
    // internally (it's safe on its own: each GrpcClient's channel is fixed to
    // whatever socket NODESPACED_SOCKET resolved to at connect() time and is
    // immune to later env changes). The guard acquired below is a SEPARATE,
    // later, longer-held acquisition — for watcher::run, not for either
    // connect() call.
    //
    // Two independent harnesses simulate two desktop windows: each
    // TauriTestApp::connect call produces its own GrpcClient with its own
    // stable x-ns-client-id, so the daemon treats window_b's writes as
    // foreign to window_a's WatchNodes subscription — exactly the same
    // identity split two real windows would have.
    let window_a = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let window_b = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state_a = window_a.client_state();
    let state_b = window_b.client_state();
    let handle_a = window_a.handle();

    // The watcher rides window A's shared GrpcClient channel (fixed to
    // whatever socket NODESPACED_SOCKET resolved to at connect() time). Hold
    // this guard for the watcher's lifetime anyway to serialize this test
    // against every other test in the binary that touches the process-global
    // NODESPACED_SOCKET env var while this test's daemon is the intended
    // target.
    let _socket_guard = hold_connect_mutex_and_socket_env(&daemon).await;

    let cancel_token = CancellationToken::new();
    let watcher_handle = tokio::spawn(watcher::run(
        handle_a.clone(),
        (*state_a).clone(),
        cancel_token.child_token(),
    ));
    // Give window A's watcher a moment to open its WatchNodes stream before
    // window B's first write, so the broadcast isn't lost to a stream that
    // hasn't opened yet.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let received: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let received_clone = received.clone();
    handle_a.listen("node:updated", move |event| {
        // Payload is `{ "id": "...", "nodeType": ... }` (nodeType omitted for
        // updates) — extract just the id for this test's purposes.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            if let Some(id) = v["id"].as_str() {
                received_clone.lock().unwrap().push(id.to_string());
            }
        }
    });

    // Window B creates and edits the node — window A never writes to it, only
    // watches.
    let id = uuid::Uuid::new_v4().to_string();
    create_node(state_b.clone(), text_input(&id, "v0"))
        .await
        .expect("create_node failed");

    // First write: content window A's watcher should see as an earlier
    // broadcast for this node.
    update_node(
        state_b.clone(),
        id.clone(),
        1,
        NodeUpdate {
            content: Some("stale content".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("first update_node failed");

    // Second, newer write from window B — sequenced correctly (using the
    // confirmed version from the first commit).
    update_node(
        state_b.clone(),
        id.clone(),
        2,
        NodeUpdate {
            content: Some("newest content".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("second update_node failed");

    // Wait for both of window B's writes to arrive on window A's real watch
    // stream.
    wait_for_updates(&received, &id, 2, Duration::from_secs(10)).await;

    // The authoritative source must reflect window B's LATEST write — a real
    // out-of-order or late broadcast of "stale content" must never win over it.
    let node = nodespace_app_lib::commands::nodes::get_node(state_a.clone(), id.clone())
        .await
        .expect("get_node failed")
        .expect("node must exist");
    assert_eq!(
        node["content"],
        json!("newest content"),
        "the newer write must not be clobbered by a late broadcast of the older write"
    );
    assert_eq!(node["version"], json!(3));

    cancel_token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher_handle).await;
}

/// Confirms the watcher actually delivers `node:created` for a real create,
/// over the real event bus — the minimal proof that this test file is
/// exercising the live path (`watcher::run`), not a dead one. Uses a second,
/// independent `GrpcClient` (window B) to create the node — since the
/// ADR-026 C5 extension, a window's own writes are suppressed on its own
/// `WatchNodes` stream, so watching window A must observe a genuinely
/// foreign creation for this to prove the live path is wired up.
#[tokio::test]
async fn watcher_delivers_a_real_node_created_event() {
    let daemon = SpawnedDaemon::spawn();
    let window_a = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let window_b = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state_b = window_b.client_state();
    let handle_a = window_a.handle();
    let _socket_guard = hold_connect_mutex_and_socket_env(&daemon).await;

    let cancel_token = CancellationToken::new();
    let watcher_handle = tokio::spawn(watcher::run(
        handle_a.clone(),
        (*window_a.client_state()).clone(),
        cancel_token.child_token(),
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let created: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let created_clone = created.clone();
    handle_a.listen("node:created", move |event| {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(event.payload()) {
            if let Some(id) = v["id"].as_str() {
                created_clone.lock().unwrap().push(id.to_string());
            }
        }
    });

    let id = uuid::Uuid::new_v4().to_string();
    create_node(state_b.clone(), text_input(&id, "hello"))
        .await
        .expect("create_node failed");

    wait_for_updates(&created, &id, 1, Duration::from_secs(10)).await;

    cancel_token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher_handle).await;
}
