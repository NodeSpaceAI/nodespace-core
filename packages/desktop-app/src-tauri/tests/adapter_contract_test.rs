//! ADR-048 priority flow 8 — adapter contract parity (decision 5).
//!
//! `src/tests/e2e/adapter-contract.e2e.ts` already proves HttpAdapter and
//! dev-proxy agree on these same operations against a real daemon, and
//! notes explicitly (its own module doc) that "a true Tauri-IPC round-trip
//! ... requires a Rust-side integration test — tracked separately, not
//! something a TypeScript harness without a webview can drive." This file
//! is that Rust-side half: it drives the identical operations — task
//! tri-state clear/set/no-change, and InsertPosition on both create and
//! move — through the REAL TauriAdapter path (the `#[tauri::command]`
//! functions themselves) against a real daemon, and asserts the same
//! outcomes the TS suite asserts for HttpAdapter/dev-proxy. Two suites
//! independently pinning the same documented contract is what makes a
//! divergence between the paths a test failure rather than a silent drift
//! — neither suite can drift without the OTHER one continuing to pass, so
//! a change that breaks the contract on one path and not the other shows up
//! as exactly one of the two suites failing.

use nodespace_app_lib::commands::nodes::{
    create_node, get_children, move_node, update_person_node, update_task_node, CreateNodeInput,
    InsertPositionInput,
};
use nodespace_app_lib::types::{PersonNodeUpdate, TaskNodeUpdate, TaskPriority, TaskStatus};
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

fn task_input(id: &str) -> CreateNodeInput {
    CreateNodeInput {
        id: id.to_string(),
        node_type: "task".to_string(),
        content: "contract task".to_string(),
        parent_id: None,
        insert_position: None,
        properties: json!({}),
    }
}

/// Mirrors `adapter-contract.e2e.ts`'s "create → update task fields with
/// tri-state clear/set/no-change → read back matches".
#[tokio::test]
async fn task_tri_state_update_clear_set_no_change_matches_the_http_adapter_contract() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), task_input(&id))
        .await
        .expect("create task failed");

    // Set: priority -> "high", status -> in_progress.
    let updated = update_task_node(
        state.clone(),
        id.clone(),
        1,
        TaskNodeUpdate {
            priority: Some(Some(TaskPriority::High)),
            status: Some(TaskStatus::InProgress),
            ..Default::default()
        },
    )
    .await
    .expect("update_task_node (set) failed");
    assert_eq!(updated["priority"], json!("high"));
    assert_eq!(updated["status"], json!("in_progress"));
    let version_after_set = updated["version"]
        .as_i64()
        .expect("version must be a number");

    // Clear: priority -> None must round-trip to "no priority", not the
    // literal string "null" or an unset-vs-cleared ambiguity — the exact
    // regression the tri-state encoding exists to prevent.
    let cleared = update_task_node(
        state.clone(),
        id.clone(),
        version_after_set,
        TaskNodeUpdate {
            priority: Some(None),
            ..Default::default()
        },
    )
    .await
    .expect("update_task_node (clear) failed");
    assert!(
        cleared["priority"].is_null(),
        "cleared priority must be null, got: {:?}",
        cleared["priority"]
    );
    // No-change: status must still be in_progress — clearing priority must
    // not have touched a field the update didn't mention.
    assert_eq!(cleared["status"], json!("in_progress"));
}

/// Mirrors `adapter-contract.e2e.ts`'s "create → typed person update → read
/// back carries typed fields and the templated title".
#[tokio::test]
async fn person_typed_update_matches_the_http_adapter_contract() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(
        state.clone(),
        CreateNodeInput {
            id: id.clone(),
            node_type: "person".to_string(),
            content: String::new(),
            parent_id: None,
            insert_position: None,
            properties: json!({ "first_name": "Ada" }),
        },
    )
    .await
    .expect("create person failed");

    let updated = update_person_node(
        state.clone(),
        id.clone(),
        1,
        PersonNodeUpdate {
            last_name: Some(Some("Lovelace".to_string())),
            email: Some(Some("ada@example.com".to_string())),
            ..Default::default()
        },
    )
    .await
    .expect("update_person_node (set) failed");
    assert_eq!(updated["firstName"], json!("Ada"));
    assert_eq!(updated["lastName"], json!("Lovelace"));
    assert_eq!(updated["email"], json!("ada@example.com"));
    assert_eq!(updated["title"], json!("Ada Lovelace"));
    assert_eq!(updated["properties"], json!({}));
    let version = updated["version"].as_i64().expect("version must be a number");

    let cleared = update_person_node(
        state.clone(),
        id.clone(),
        version,
        PersonNodeUpdate {
            email: Some(None),
            ..Default::default()
        },
    )
    .await
    .expect("update_person_node (clear) failed");
    assert!(cleared.get("email").is_none(), "cleared email must be absent");
    assert_eq!(cleared["lastName"], json!("Lovelace"));
}

/// Mirrors `adapter-contract.e2e.ts`'s "createNode honors an explicit
/// InsertPosition the same way move/reorder do".
#[tokio::test]
async fn create_node_insert_position_matches_the_http_adapter_contract() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let parent_id = uuid::Uuid::new_v4().to_string();
    let first_id = uuid::Uuid::new_v4().to_string();
    let second_id = uuid::Uuid::new_v4().to_string();

    create_node(state.clone(), text_input(&parent_id, "parent", None))
        .await
        .expect("create parent failed");
    create_node(
        state.clone(),
        text_input(&first_id, "first", Some(parent_id.clone())),
    )
    .await
    .expect("create first failed");

    let mut second_input = text_input(&second_id, "inserted-before-first", Some(parent_id.clone()));
    second_input.insert_position = Some(InsertPositionInput::Beginning);
    create_node(state.clone(), second_input)
        .await
        .expect("create second (Beginning) failed");

    let children = get_children(state.clone(), parent_id)
        .await
        .expect("get_children failed");
    let child_ids: Vec<String> = children
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(child_ids, vec![second_id, first_id]);
}

/// Mirrors `adapter-contract.e2e.ts`'s "moveNode honors an explicit
/// InsertPosition (regression: dev-proxy previously ignored it entirely)".
#[tokio::test]
async fn move_node_insert_position_matches_the_http_adapter_contract() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let parent_a_id = uuid::Uuid::new_v4().to_string();
    let parent_b_id = uuid::Uuid::new_v4().to_string();
    let staying_id = uuid::Uuid::new_v4().to_string();
    let moving_id = uuid::Uuid::new_v4().to_string();

    create_node(state.clone(), text_input(&parent_a_id, "parent-a", None))
        .await
        .expect("create parent-a failed");
    create_node(state.clone(), text_input(&parent_b_id, "parent-b", None))
        .await
        .expect("create parent-b failed");
    create_node(
        state.clone(),
        text_input(&moving_id, "moving", Some(parent_a_id.clone())),
    )
    .await
    .expect("create moving failed");
    create_node(
        state.clone(),
        text_input(&staying_id, "staying", Some(parent_b_id.clone())),
    )
    .await
    .expect("create staying failed");

    move_node(
        state.clone(),
        moving_id.clone(),
        1,
        Some(parent_b_id.clone()),
        Some(InsertPositionInput::Beginning),
    )
    .await
    .expect("move_node failed");

    let children = get_children(state.clone(), parent_b_id)
        .await
        .expect("get_children failed");
    let child_ids: Vec<String> = children
        .iter()
        .map(|n| n["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(child_ids, vec![moving_id, staying_id]);
}
