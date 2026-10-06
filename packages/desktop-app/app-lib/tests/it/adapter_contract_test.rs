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
    create_node, get_children, get_node, move_node, update_collection_node,
    update_database_settings_node, update_person_node, update_play_node, update_query_node,
    update_skill_node, update_task_node, CreateNodeInput, InsertPositionInput,
};
use nodespace_app_lib::types::{
    CollectionNodeUpdate, DatabaseSettingsNodeUpdate, PersonNodeUpdate, PlayNodeUpdate, Priority,
    QueryNodeUpdate, SkillNodeUpdate, TaskNodeUpdate, TaskStatus,
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
            priority: Some(Some(Priority::High)),
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
    let version = updated["version"]
        .as_i64()
        .expect("version must be a number");

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
    assert!(
        cleared.get("email").is_none(),
        "cleared email must be absent"
    );
    assert_eq!(cleared["lastName"], json!("Lovelace"));
}

/// Mirrors `adapter-contract.e2e.ts`'s "create → typed query update → read
/// back carries typed fields".
#[tokio::test]
async fn query_typed_update_matches_the_http_adapter_contract() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(
        state.clone(),
        CreateNodeInput {
            id: id.clone(),
            node_type: "query".to_string(),
            content: "Open tasks".to_string(),
            parent_id: None,
            insert_position: None,
            properties: json!({
                "target_type": "task",
                "filters": [],
                "generated_by": "user",
                "view_config": { "lastView": "table" },
            }),
        },
    )
    .await
    .expect("create query failed");

    let update: QueryNodeUpdate = serde_json::from_value(json!({
        "filters": [
            { "type": "property", "operator": "equals", "property": "status", "value": "open" }
        ],
        "viewConfig": { "lastView": "kanban", "kanban": { "groupBy": "status" } },
    }))
    .unwrap();
    let updated = update_query_node(state.clone(), id.clone(), 1, update)
        .await
        .expect("update_query_node (set) failed");
    assert_eq!(updated["targetType"], json!("task"));
    assert_eq!(updated["filters"][0]["property"], json!("status"));
    assert_eq!(updated["viewConfig"]["kanban"]["groupBy"], json!("status"));
    assert_eq!(updated["properties"], json!({}));
    let version = updated["version"]
        .as_i64()
        .expect("version must be a number");

    let cleared = update_query_node(
        state.clone(),
        id.clone(),
        version,
        serde_json::from_value(json!({ "viewConfig": null })).unwrap(),
    )
    .await
    .expect("update_query_node (clear) failed");
    assert!(
        cleared.get("viewConfig").is_none(),
        "cleared viewConfig must be absent"
    );

    let reread = get_node(state.clone(), id.clone())
        .await
        .expect("get_node failed")
        .expect("query must exist");
    assert_eq!(reread["filters"][0]["value"], json!("open"));
}

/// Mirrors `adapter-contract.e2e.ts`'s "create → typed play update → read
/// back carries typed fields".
#[tokio::test]
async fn play_typed_update_matches_the_http_adapter_contract() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(
        state.clone(),
        CreateNodeInput {
            id: id.clone(),
            node_type: "play".to_string(),
            content: "Greet new tasks".to_string(),
            parent_id: None,
            insert_position: None,
            properties: json!({ "rules": [] }),
        },
    )
    .await
    .expect("create play failed");

    let update: PlayNodeUpdate = serde_json::from_value(json!({
        "rules": [{
            "name": "greet",
            "description": "Test rule",
            "trigger": {
                "type": "graph_event",
                "on": "node_created",
                "select": { "target_type": "task" }
            },
            "conditions": [{ "expr": "node.content == 'hello'", "description": "Test condition" }],
            "actions": []
        }],
        "description": "Greets new tasks",
    }))
    .unwrap();
    let updated = update_play_node(state.clone(), id.clone(), 1, update)
        .await
        .expect("update_play_node (set) failed");
    assert_eq!(updated["rules"][0]["name"], json!("greet"));
    assert_eq!(
        updated["rules"][0]["trigger"]["select"],
        json!({ "target_type": "task" })
    );
    assert_eq!(updated["description"], json!("Greets new tasks"));
    assert_eq!(updated["properties"], json!({}));
    let version = updated["version"]
        .as_i64()
        .expect("version must be a number");

    let cleared = update_play_node(
        state.clone(),
        id.clone(),
        version,
        serde_json::from_value(json!({ "description": null })).unwrap(),
    )
    .await
    .expect("update_play_node (clear) failed");
    assert!(
        cleared.get("description").is_none(),
        "cleared description must be absent"
    );

    let reread = get_node(state.clone(), id.clone())
        .await
        .expect("get_node failed")
        .expect("play must exist");
    assert_eq!(reread["rules"][0]["name"], json!("greet"));
}

/// Mirrors `adapter-contract.e2e.ts`'s "create → typed collection update →
/// read back carries the typed description".
#[tokio::test]
async fn collection_typed_update_matches_the_http_adapter_contract() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(
        state.clone(),
        CreateNodeInput {
            id: id.clone(),
            node_type: "collection".to_string(),
            content: "contract-clients".to_string(),
            parent_id: None,
            insert_position: None,
            properties: json!({}),
        },
    )
    .await
    .expect("create collection failed");

    let updated = update_collection_node(
        state.clone(),
        id.clone(),
        1,
        CollectionNodeUpdate {
            description: Some(Some("Accounts we bill".to_string())),
        },
    )
    .await
    .expect("update_collection_node (set) failed");
    assert_eq!(updated["description"], json!("Accounts we bill"));
    assert_eq!(updated["content"], json!("contract-clients"));
    assert_eq!(updated["properties"], json!({}));
    let version = updated["version"]
        .as_i64()
        .expect("version must be a number");

    let cleared = update_collection_node(
        state.clone(),
        id.clone(),
        version,
        CollectionNodeUpdate {
            description: Some(None),
        },
    )
    .await
    .expect("update_collection_node (clear) failed");
    assert!(
        cleared.get("description").is_none(),
        "cleared description must be absent"
    );
}

/// Mirrors `adapter-contract.e2e.ts`'s "create → typed skill update → read
/// back carries typed fields".
#[tokio::test]
async fn skill_typed_update_matches_the_http_adapter_contract() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(
        state.clone(),
        CreateNodeInput {
            id: id.clone(),
            node_type: "skill".to_string(),
            content: "Contract Skill".to_string(),
            parent_id: None,
            insert_position: None,
            properties: json!({
                "use_for": "Update a record",
                "tool_whitelist": ["update_node"],
                "not_for": "Delete records",
            }),
        },
    )
    .await
    .expect("create skill failed");

    let updated = update_skill_node(
        state.clone(),
        id.clone(),
        1,
        SkillNodeUpdate {
            tool_whitelist: Some(vec!["update_node".to_string(), "get_node".to_string()]),
            max_iterations: Some(Some(4)),
            ..Default::default()
        },
    )
    .await
    .expect("update_skill_node (set) failed");
    assert_eq!(updated["useFor"], json!("Update a record"));
    assert_eq!(updated["notFor"], json!("Delete records"));
    assert_eq!(updated["toolWhitelist"], json!(["update_node", "get_node"]));
    assert_eq!(updated["maxIterations"], json!(4));
    assert_eq!(updated["properties"], json!({}));
    let version = updated["version"]
        .as_i64()
        .expect("version must be a number");

    let cleared = update_skill_node(
        state.clone(),
        id.clone(),
        version,
        SkillNodeUpdate {
            not_for: Some(None),
            max_iterations: Some(None),
            ..Default::default()
        },
    )
    .await
    .expect("update_skill_node (clear) failed");
    assert!(
        cleared.get("notFor").is_none(),
        "cleared not_for must be absent"
    );
    // A cleared number reads as the schema's default.
    assert_eq!(cleared["maxIterations"], json!(2));
    assert_eq!(cleared["toolWhitelist"], json!(["update_node", "get_node"]));
}

/// Mirrors `adapter-contract.e2e.ts`'s "typed database-settings update → read
/// back carries the typed list". The list is cleared again before the daemon
/// goes away, so nothing later opens a database that requires an extension.
#[tokio::test]
async fn database_settings_typed_update_matches_the_http_adapter_contract() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = "database-settings-singleton".to_string();
    let settings = get_node(state.clone(), id.clone())
        .await
        .expect("get_node failed")
        .expect("the settings singleton must exist");
    assert_eq!(settings["requiredExtensions"], json!([]));
    assert_eq!(settings["captureEnabled"], json!(false));
    assert_eq!(settings["captureContent"], json!("metadata_only"));
    assert_eq!(settings["providers"], json!([]));
    let version = settings["version"]
        .as_i64()
        .expect("version must be a number");

    let updated = update_database_settings_node(
        state.clone(),
        id.clone(),
        version,
        DatabaseSettingsNodeUpdate {
            required_extensions: Some(Some(vec!["contract-fixture".to_string()])),
            capture_enabled: Some(Some(true)),
            providers: Some(Some(vec![nodespace_types::ProviderConfig {
                id: "0b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d".to_string(),
                name: "Local".to_string(),
                base_url: "http://127.0.0.1:1/v1".to_string(),
                api_key: String::new(),
                model: "m".to_string(),
                routing_ok: Default::default(),
            }])),
            ..Default::default()
        },
    )
    .await
    .expect("update_database_settings_node (set) failed");
    assert_eq!(updated["requiredExtensions"], json!(["contract-fixture"]));
    assert_eq!(updated["captureEnabled"], json!(true));
    assert_eq!(
        updated["providers"][0]["base_url"],
        json!("http://127.0.0.1:1/v1")
    );
    assert_eq!(updated["properties"], json!({}));
    let version = updated["version"]
        .as_i64()
        .expect("version must be a number");

    let cleared = update_database_settings_node(
        state.clone(),
        id.clone(),
        version,
        DatabaseSettingsNodeUpdate {
            required_extensions: Some(None),
            capture_enabled: Some(None),
            providers: Some(None),
            ..Default::default()
        },
    )
    .await
    .expect("update_database_settings_node (clear) failed");
    assert_eq!(cleared["requiredExtensions"], json!([]));
    assert_eq!(cleared["captureEnabled"], json!(false));
    assert_eq!(cleared["providers"], json!([]));
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
