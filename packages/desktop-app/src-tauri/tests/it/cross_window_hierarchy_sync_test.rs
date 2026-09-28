//! ADR-048 priority flow 7 — cross-window hierarchy sync.
//!
//! No existing Rust or TypeScript test drives two independent clients
//! against one real daemon (the frontend's `multi-tab-reactivity.test.ts`
//! simulates multiple "viewers" with mocked stores inside a single test
//! process, not real separate windows/IPC contexts). Two windows in the real
//! desktop app are two independent `GrpcClient` connections talking to the
//! same `nodespaced` over the same UDS — that's exactly what two
//! `TauriTestApp::connect` calls against the same `SpawnedDaemon` model:
//! two independent command-layer clients, sharing nothing but the daemon.

use nodespace_app_lib::commands::nodes::{create_node, get_children, move_node, CreateNodeInput};
use nodespace_app_test_support::{SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT};
use serde_json::json;

fn text_input(id: &str, content: &str, parent_id: Option<String>) -> CreateNodeInput {
    CreateNodeInput {
        id: id.to_string(),
        node_type: "text".to_string(),
        content: content.to_string(),
        parent_id,
        insert_position: None,
        properties: json!({}),
    }
}

/// Window A creates the hierarchy and moves a node; Window B — a completely
/// independent client, connected separately — observes the converged result
/// via its own `get_children` calls against the same daemon. This is the
/// "does window B ever see what window A wrote" guarantee: no shared
/// in-process state, no shared client, only the daemon in common.
#[tokio::test]
async fn a_second_independent_client_observes_hierarchy_changes_made_by_the_first() {
    let daemon = SpawnedDaemon::spawn();
    let window_a = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let window_b = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;

    let a_state = window_a.client_state();
    let b_state = window_b.client_state();

    let root_id = uuid::Uuid::new_v4().to_string();
    let child_id = uuid::Uuid::new_v4().to_string();

    // Window A creates the hierarchy.
    create_node(a_state.clone(), text_input(&root_id, "root", None))
        .await
        .expect("window A: create root failed");
    create_node(
        a_state.clone(),
        text_input(&child_id, "child", Some(root_id.clone())),
    )
    .await
    .expect("window A: create child failed");

    // Window B — its own connection, never touched by window A's calls —
    // must see the same hierarchy, because both talk to the same daemon.
    let b_children = get_children(b_state.clone(), root_id.clone())
        .await
        .expect("window B: get_children failed");
    assert_eq!(b_children.len(), 1);
    assert_eq!(b_children[0]["id"], json!(child_id));

    // Window B moves the child to a new root; window A must observe it.
    let other_root_id = uuid::Uuid::new_v4().to_string();
    create_node(
        b_state.clone(),
        text_input(&other_root_id, "other root", None),
    )
    .await
    .expect("window B: create other root failed");

    move_node(
        b_state.clone(),
        child_id.clone(),
        1,
        Some(other_root_id.clone()),
        None,
    )
    .await
    .expect("window B: move_node failed");

    let a_sees_old_root_empty = get_children(a_state.clone(), root_id)
        .await
        .expect("window A: get_children(old root) failed");
    assert!(
        a_sees_old_root_empty.is_empty(),
        "window A must observe the child leaving the old root"
    );

    let a_sees_new_root = get_children(a_state.clone(), other_root_id)
        .await
        .expect("window A: get_children(new root) failed");
    assert_eq!(a_sees_new_root.len(), 1);
    assert_eq!(a_sees_new_root[0]["id"], json!(child_id));
}

/// A third client connecting AFTER both writes must converge to the exact
/// same state on its very first read — there is no "catch-up window" a
/// freshly-opened window could observe stale data during, since all clients
/// read the same durable SQLite store through the same daemon.
#[tokio::test]
async fn a_client_connecting_after_writes_immediately_sees_converged_state() {
    let daemon = SpawnedDaemon::spawn();
    let window_a = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let a_state = window_a.client_state();

    let root_id = uuid::Uuid::new_v4().to_string();
    create_node(a_state.clone(), text_input(&root_id, "root", None))
        .await
        .expect("create root failed");
    for i in 0..3 {
        let id = uuid::Uuid::new_v4().to_string();
        create_node(
            a_state.clone(),
            text_input(&id, &format!("child {i}"), Some(root_id.clone())),
        )
        .await
        .unwrap_or_else(|e| panic!("create child {i} failed: {e:?}"));
    }

    // A late-joining window, connected only after all writes completed.
    let window_c = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let c_children = get_children(window_c.client_state(), root_id)
        .await
        .expect("window C: get_children failed");
    assert_eq!(
        c_children.len(),
        3,
        "a newly connected window must immediately see all prior writes"
    );
}
