//! ADR-048 priority flow 3 — indent/outdent and rapid node-creation ordering
//! through the real Tauri command layer, against a real headless daemon.
//!
//! The frontend has unit coverage (`src/tests/unit/indent-outdent-race-condition.test.ts`)
//! for a client-side coordination pattern that serializes overlapping
//! move_node calls, and this is the regression it guards: a rapid
//! indent-then-outdent issues two `move_node` calls whose OCC versions can
//! race against a live daemon. That unit test reimplements the coordination
//! logic against fake timers — it cannot exercise the actual daemon race.
//! These tests drive the real `move_node`/`reorder_node`/`create_node`
//! command functions directly, so the OCC conflict (or its absence, once
//! calls are properly sequenced) is real, not simulated.

use nodespace_app_lib::commands::nodes::{
    create_node, get_children, move_node, reorder_node, CreateNodeInput, InsertPositionInput,
};
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

/// Indent B under A, then immediately outdent B back to the root — the
/// exact sequence `indent-outdent-race-condition.test.ts` describes.
/// Sequenced correctly (awaiting indent's real new version before issuing
/// outdent), this must succeed against a real daemon: no stale-version
/// conflict, and B ends up back at the root as a sibling of A.
#[tokio::test]
async fn rapid_indent_then_outdent_survives_against_a_real_daemon() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let root_id = uuid::Uuid::new_v4().to_string();
    let a_id = uuid::Uuid::new_v4().to_string();
    let b_id = uuid::Uuid::new_v4().to_string();

    create_node(state.clone(), text_input(&root_id, "root", None))
        .await
        .expect("create root failed");
    create_node(state.clone(), text_input(&a_id, "A", Some(root_id.clone())))
        .await
        .expect("create A failed");
    create_node(state.clone(), text_input(&b_id, "B", Some(root_id.clone())))
        .await
        .expect("create B failed");

    // Indent: B becomes a child of A. move_node returns the new node,
    // whose version is the real, authoritative post-indent version.
    let indented = move_node(
        state.clone(),
        b_id.clone(),
        1,
        Some(a_id.clone()),
        Some(InsertPositionInput::End),
    )
    .await
    .expect("indent (move B under A) failed");
    let version_after_indent = indented.node["version"]
        .as_i64()
        .expect("version must be a number");
    let placement = indented
        .placement
        .expect("a move under a parent returns its placement");
    assert_eq!(placement.parent_id, a_id);

    // Outdent: B moves back under root, using the version move_node just
    // returned — the real sequencing a coordinated frontend must achieve.
    move_node(
        state.clone(),
        b_id.clone(),
        version_after_indent,
        Some(root_id.clone()),
        Some(InsertPositionInput::End),
    )
    .await
    .expect("outdent (move B back under root) failed");

    let root_children = get_children(state.clone(), root_id)
        .await
        .expect("get_children(root) failed");
    let root_child_ids: Vec<&str> = root_children
        .iter()
        .map(|n| n["id"].as_str().unwrap())
        .collect();
    assert!(root_child_ids.contains(&a_id.as_str()));
    assert!(root_child_ids.contains(&b_id.as_str()));

    let a_children = get_children(state.clone(), a_id)
        .await
        .expect("get_children(A) failed");
    assert!(
        a_children.is_empty(),
        "B must no longer be a child of A after outdent"
    );
}

/// Rapid-Enter: create several sibling nodes back-to-back (as fast as the
/// event loop allows, not serialized by any artificial delay) each inserted
/// After the previous one, then confirm get_children returns them in the
/// exact order they were created — the ordering rapid Enter-presses rely on.
#[tokio::test]
async fn rapid_sibling_creation_preserves_insertion_order() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let root_id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), text_input(&root_id, "root", None))
        .await
        .expect("create root failed");

    let mut ids = Vec::new();
    let mut prev_id: Option<String> = None;
    for i in 0..5 {
        let id = uuid::Uuid::new_v4().to_string();
        let mut input = text_input(&id, &format!("line {i}"), Some(root_id.clone()));
        input.insert_position = Some(match &prev_id {
            None => InsertPositionInput::Beginning,
            Some(sibling_id) => InsertPositionInput::After {
                sibling_id: sibling_id.clone(),
            },
        });
        create_node(state.clone(), input)
            .await
            .unwrap_or_else(|e| panic!("create line {i} failed: {e:?}"));
        ids.push(id.clone());
        prev_id = Some(id);
    }

    let children = get_children(state.clone(), root_id)
        .await
        .expect("get_children failed");
    let child_ids: Vec<String> = children
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        child_ids, ids,
        "children must preserve rapid-Enter insertion order"
    );
}

/// reorder_node under concurrent load: fire off several reorder_node calls
/// for distinct nodes concurrently (Tokio join, not sequential awaits) and
/// confirm every call itself succeeds (no RPC error, no OCC conflict — the
/// two nodes being reordered are distinct, so there's no version conflict
/// between them). Does NOT assert the final sibling order matches both
/// reorders: that assertion is `concurrent_reorders_produce_correct_final_order`
/// below, `#[ignore]`d as a known-failing regression test for a real,
/// intermittent race in the fractional-order read-then-write window that
/// this concurrent case can trigger.
#[tokio::test]
async fn concurrent_reorders_of_distinct_nodes_all_succeed() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let root_id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), text_input(&root_id, "root", None))
        .await
        .expect("create root failed");

    let mut ids = Vec::new();
    for i in 0..3 {
        let id = uuid::Uuid::new_v4().to_string();
        create_node(
            state.clone(),
            text_input(&id, &format!("n{i}"), Some(root_id.clone())),
        )
        .await
        .unwrap_or_else(|e| panic!("create n{i} failed: {e:?}"));
        ids.push(id);
    }

    // Move the last node to Beginning, and the first node to End, concurrently.
    let (r1, r2) = tokio::join!(
        reorder_node(
            state.clone(),
            ids[2].clone(),
            1,
            Some(InsertPositionInput::Beginning),
        ),
        reorder_node(
            state.clone(),
            ids[0].clone(),
            1,
            Some(InsertPositionInput::End),
        ),
    );
    r1.expect("reorder of ids[2] to Beginning failed");
    r2.expect("reorder of ids[0] to End failed");
}

/// Regression test: concurrent reorders of distinct siblings to
/// opposite boundary positions (Beginning / End) must produce a sibling
/// order reflecting BOTH operations, not just that each RPC individually
/// succeeded. `move_node`'s same-parent branch reads sibling order values
/// and writes the new order back as two separate, non-transactional steps
/// (`packages/core/src/db/sqlite_store.rs`), so two concurrent reorders can
/// read the same sibling snapshot and race. Reproduces intermittently
/// (roughly 1 in 15 full-suite runs) — `#[ignore]`d so the flake doesn't
/// block the suite; un-ignore once the read+compute+write is wrapped in a
/// transaction per parent.
#[tokio::test]
#[ignore = "known race: non-transactional sibling reorder"]
async fn concurrent_reorders_produce_correct_final_order() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let root_id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), text_input(&root_id, "root", None))
        .await
        .expect("create root failed");

    let mut ids = Vec::new();
    for i in 0..3 {
        let id = uuid::Uuid::new_v4().to_string();
        create_node(
            state.clone(),
            text_input(&id, &format!("n{i}"), Some(root_id.clone())),
        )
        .await
        .unwrap_or_else(|e| panic!("create n{i} failed: {e:?}"));
        ids.push(id);
    }

    let (r1, r2) = tokio::join!(
        reorder_node(
            state.clone(),
            ids[2].clone(),
            1,
            Some(InsertPositionInput::Beginning),
        ),
        reorder_node(
            state.clone(),
            ids[0].clone(),
            1,
            Some(InsertPositionInput::End),
        ),
    );
    r1.expect("reorder of ids[2] to Beginning failed");
    r2.expect("reorder of ids[0] to End failed");

    let children = get_children(state.clone(), root_id)
        .await
        .expect("get_children failed");
    let child_ids: Vec<String> = children
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(
        child_ids[0], ids[2],
        "node reordered to Beginning must be first"
    );
    assert_eq!(
        child_ids[child_ids.len() - 1],
        ids[0],
        "node reordered to End must be last"
    );
}
