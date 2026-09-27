//! ADR-048 priority flow 1 — node CRUD round-trip through the REAL Tauri
//! command layer (not a mock, not the HTTP/dev-proxy path).
//!
//! This is the production path: `#[tauri::command]` handler → `GrpcClient`
//! → gRPC over UDS → a real headless `nodespaced` → real SQLite. It re-runs
//! the same round-trip `src/tests/e2e/node-roundtrip.e2e.ts` already covers
//! through `HttpAdapter`, but through the adapter that actually ships in the
//! desktop build.

use nodespace_app_lib::commands::nodes::CreateNodeInput;
use nodespace_app_lib::commands::nodes::{create_node, delete_node, get_node, update_node};
use nodespace_app_lib::types::NodeUpdate;
use nodespace_app_test_support::{SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT};
use serde_json::json;

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
async fn creates_a_node_and_reads_it_back() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    let created = create_node(state.clone(), text_input(&id, "hello tauri seam"))
        .await
        .expect("create_node failed");
    assert_eq!(created.id, id);
    assert!(
        created.placement.is_none(),
        "a root node has no parent edge"
    );

    let node = get_node(state.clone(), id.clone())
        .await
        .expect("get_node failed")
        .expect("node should exist");

    assert_eq!(node["id"], json!(id));
    assert_eq!(node["nodeType"], json!("text"));
    assert_eq!(node["content"], json!("hello tauri seam"));
    assert_eq!(node["version"], json!(1));
}

#[tokio::test]
async fn updates_a_node_and_increments_version_each_time() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), text_input(&id, "v1"))
        .await
        .expect("create_node failed");

    let v2 = update_node(
        state.clone(),
        id.clone(),
        1,
        NodeUpdate {
            content: Some("v2".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("update_node (v1 -> v2) failed");
    assert_eq!(v2["version"], json!(2));
    assert_eq!(v2["content"], json!("v2"));

    let v3 = update_node(
        state.clone(),
        id.clone(),
        2,
        NodeUpdate {
            content: Some("v3".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("update_node (v2 -> v3) failed");
    assert_eq!(v3["version"], json!(3));

    let fetched = get_node(state.clone(), id.clone())
        .await
        .expect("get_node failed")
        .expect("node should exist");
    assert_eq!(fetched["content"], json!("v3"));
    assert_eq!(fetched["version"], json!(3));
}

#[tokio::test]
async fn deletes_a_node_and_confirms_it_is_absent() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), text_input(&id, "to delete"))
        .await
        .expect("create_node failed");

    let result = delete_node(state.clone(), id.clone(), 1)
        .await
        .expect("delete_node failed");
    assert!(result.existed);

    let node = get_node(state.clone(), id.clone())
        .await
        .expect("get_node failed");
    assert!(node.is_none(), "deleted node must not be readable");
}

#[tokio::test]
async fn creates_a_parent_child_hierarchy_through_the_command_layer() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let parent_id = uuid::Uuid::new_v4().to_string();
    let child_id = uuid::Uuid::new_v4().to_string();

    create_node(state.clone(), text_input(&parent_id, "parent"))
        .await
        .expect("create parent failed");

    let mut child_input = text_input(&child_id, "child");
    child_input.parent_id = Some(parent_id.clone());
    create_node(state.clone(), child_input)
        .await
        .expect("create child failed");

    let children = nodespace_app_lib::commands::nodes::get_children(state.clone(), parent_id)
        .await
        .expect("get_children failed");
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["id"], json!(child_id));
    assert_eq!(children[0]["content"], json!("child"));
}

#[tokio::test]
async fn persists_node_properties_through_the_round_trip() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let id = uuid::Uuid::new_v4().to_string();
    let mut input = text_input(&id, "with props");
    input.properties = json!({ "text": { "priority": "high", "tags": ["a", "b"], "count": 42 } });

    create_node(state.clone(), input)
        .await
        .expect("create_node failed");

    let node = get_node(state.clone(), id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // "text" isn't one of the typed nodes (task/ai-chat/schema) that get their
    // properties promoted to top-level fields — see `node_to_typed_value` /
    // `flatten_properties_for_api` in nodespace-types. For a generic type,
    // flattened properties land under the top-level `properties` object.
    assert_eq!(node["properties"]["priority"], json!("high"));
    assert_eq!(node["properties"]["tags"], json!(["a", "b"]));
    assert_eq!(node["properties"]["count"], json!(42));
}
