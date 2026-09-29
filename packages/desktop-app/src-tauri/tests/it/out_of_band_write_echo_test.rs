//! A window must see its OWN out-of-band writes on its own `WatchNodes`
//! stream: writes that reach the daemon through a Tauri command which the
//! frontend store did not apply itself (the onboarding identity step, a
//! playbook install, ...). The store learns about those only through the
//! watcher → `node:*` event → fetch path, so if the daemon suppresses their
//! echo as "this window's own write" they never reach the UI.
//!
//! The companion check is that echo suppression still holds for the writes it
//! exists for: the store's own optimistic `update_node` must not echo back.
//!
//! Everything here is one window — one `GrpcClient`, one `x-ns-client-id` —
//! driving the production `watcher::run` against a real daemon, since the bug
//! only exists when writer and watcher share an identity.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nodespace_app_lib::commands::methodology::install_methodology;
use nodespace_app_lib::commands::nodes::{
    create_node, create_relationship, delete_relationship, update_node,
    update_relationship_properties, CreateNodeInput,
};
use nodespace_app_lib::commands::onboarding::set_local_identity;
use nodespace_app_lib::types::NodeUpdate;
use nodespace_app_lib::watcher;
use nodespace_app_test_support::{
    hold_connect_mutex_and_socket_env, SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT,
};
use serde_json::{json, Value};
use tauri::{AppHandle, Listener};
use tokio_util::sync::CancellationToken;

/// `(event name, id, node type)` for every `node:*` event the window saw.
type Seen = Arc<Mutex<Vec<(String, String, Option<String>)>>>;

fn record_node_events(handle: &AppHandle<tauri::test::MockRuntime>) -> Seen {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    for name in ["node:created", "node:updated"] {
        let seen = seen.clone();
        handle.listen(name, move |event| {
            if let Ok(v) = serde_json::from_str::<Value>(event.payload()) {
                if let Some(id) = v["id"].as_str() {
                    seen.lock().unwrap().push((
                        name.to_string(),
                        id.to_string(),
                        v["nodeType"].as_str().map(str::to_string),
                    ));
                }
            }
        });
    }
    seen
}

async fn wait_until(
    seen: &Seen,
    what: &str,
    pred: impl Fn(&[(String, String, Option<String>)]) -> bool,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if pred(&seen.lock().unwrap()) {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "timed out waiting for {what}; saw {:?}",
                seen.lock().unwrap()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

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

#[tokio::test]
async fn a_windows_out_of_band_writes_reach_its_own_watcher() {
    let daemon = SpawnedDaemon::spawn();
    let window = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = window.client_state();
    let handle = window.handle();
    let _socket_guard = hold_connect_mutex_and_socket_env(&daemon).await;

    let seen = record_node_events(&handle);
    let cancel_token = CancellationToken::new();
    let watcher_handle = tokio::spawn(watcher::run(
        handle.clone(),
        (*state).clone(),
        cancel_token.child_token(),
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Onboarding identity step: updates the seeded local person node.
    let identity = set_local_identity(
        state.clone(),
        "Ada".to_string(),
        "Lovelace".to_string(),
        "ada@example.com".to_string(),
    )
    .await
    .expect("set_local_identity failed");
    let person_id = identity.node_id.clone();
    wait_until(&seen, "node:updated for the local person node", |s| {
        s.iter()
            .any(|(n, id, _)| n == "node:updated" && *id == person_id)
    })
    .await;

    // Playbook install: creates schemas (the sidebar type list reacts to their
    // `node:created`) and plays.
    let report = install_methodology(state.clone(), "spec-driven".to_string())
        .await
        .expect("install_methodology failed");
    assert!(report.success, "install reported failure: {report:?}");
    for node_type in ["schema", "play"] {
        wait_until(&seen, &format!("node:created for a {node_type}"), |s| {
            s.iter()
                .any(|(n, _, t)| n == "node:created" && t.as_deref() == Some(node_type))
        })
        .await;
    }

    cancel_token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher_handle).await;
}

/// Echo suppression still applies to the store's own optimistic writes. A
/// second window's write, made afterwards, proves the stream was live the
/// whole time, so the absence of our own echo is real and not a dead stream.
#[tokio::test]
async fn a_windows_store_writes_still_do_not_echo_back() {
    let daemon = SpawnedDaemon::spawn();
    let window = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let other = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = window.client_state();
    let handle = window.handle();
    let _socket_guard = hold_connect_mutex_and_socket_env(&daemon).await;

    let seen = record_node_events(&handle);
    let cancel_token = CancellationToken::new();
    let watcher_handle = tokio::spawn(watcher::run(
        handle.clone(),
        (*state).clone(),
        cancel_token.child_token(),
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let own_id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), text_input(&own_id, "v0"))
        .await
        .expect("create_node failed");
    update_node(
        state.clone(),
        own_id.clone(),
        1,
        NodeUpdate {
            content: Some("v1".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("update_node failed");

    let marker_id = uuid::Uuid::new_v4().to_string();
    create_node(other.client_state(), text_input(&marker_id, "marker"))
        .await
        .expect("marker create_node failed");
    wait_until(&seen, "the other window's marker node:created", |s| {
        s.iter()
            .any(|(n, id, _)| n == "node:created" && *id == marker_id)
    })
    .await;

    let own_echoes: Vec<_> = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, id, _)| *id == own_id)
        .cloned()
        .collect();
    assert!(
        own_echoes.is_empty(),
        "the window's own store writes echoed back: {own_echoes:?}"
    );

    cancel_token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher_handle).await;
}

/// `relationship:*` events (name, relationship type) the window saw.
type SeenRels = Arc<Mutex<Vec<(String, String)>>>;

fn record_relationship_events(handle: &AppHandle<tauri::test::MockRuntime>) -> SeenRels {
    let seen: SeenRels = Arc::new(Mutex::new(Vec::new()));
    for name in [
        "relationship:created",
        "relationship:updated",
        "relationship:deleted",
    ] {
        let seen = seen.clone();
        handle.listen(name, move |event| {
            if let Ok(v) = serde_json::from_str::<Value>(event.payload()) {
                if let Some(t) = v["relationshipType"].as_str() {
                    seen.lock().unwrap().push((name.to_string(), t.to_string()));
                }
            }
        });
    }
    seen
}

/// The relationship viewer's typed-edge commands are not applied by the
/// frontend store, so their events must reach this window (the listener, not
/// the store, is the only thing that can react to them).
#[tokio::test]
async fn a_windows_typed_relationship_edits_reach_its_own_watcher() {
    let daemon = SpawnedDaemon::spawn();
    let window = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = window.client_state();
    let handle = window.handle();
    let _socket_guard = hold_connect_mutex_and_socket_env(&daemon).await;

    let seen = record_relationship_events(&handle);
    let cancel_token = CancellationToken::new();
    let watcher_handle = tokio::spawn(watcher::run(
        handle.clone(),
        (*state).clone(),
        cancel_token.child_token(),
    ));
    tokio::time::sleep(Duration::from_millis(300)).await;

    // `blocks` is declared on the core `task` schema.
    let (a, b) = (
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
    );
    for id in [&a, &b] {
        let mut input = text_input(id, "task");
        input.node_type = "task".to_string();
        create_node(state.clone(), input)
            .await
            .expect("create task failed");
    }

    let wait_for = |what: &'static str, name: &'static str| {
        let seen = seen.clone();
        async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            loop {
                if seen
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(n, t)| n == name && t == "blocks")
                {
                    return;
                }
                if tokio::time::Instant::now() >= deadline {
                    panic!(
                        "timed out waiting for {what}; saw {:?}",
                        seen.lock().unwrap()
                    );
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    };

    create_relationship(state.clone(), a.clone(), "blocks".into(), b.clone(), None)
        .await
        .expect("create_relationship failed");
    wait_for("relationship:created for blocks", "relationship:created").await;

    update_relationship_properties(
        state.clone(),
        a.clone(),
        "blocks".into(),
        b.clone(),
        json!({}),
    )
    .await
    .expect("update_relationship_properties failed");
    wait_for("relationship:updated for blocks", "relationship:updated").await;

    delete_relationship(state.clone(), a.clone(), "blocks".into(), b.clone())
        .await
        .expect("delete_relationship failed");
    wait_for("relationship:deleted for blocks", "relationship:deleted").await;

    cancel_token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), watcher_handle).await;
}
