//! Schema reads through the REAL Tauri command layer: `#[tauri::command]`
//! handler → `GrpcClient` → gRPC over UDS → a real headless `nodespaced` →
//! real SQLite.
//!
//! A schema's relationships and the type it extends are declaration edges in
//! the `relationship` table, not properties of its node. These tests hold the
//! desktop build's read path to returning them: the daemon's store fills the
//! one `SchemaNode`, and the command hands it on unchanged.

use nodespace_app_lib::commands::schemas::{get_all_schemas, get_schema_definition};
use nodespace_app_test_support::{SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT};
use serde_json::json;

#[tokio::test]
async fn get_all_schemas_returns_relationships_and_extends() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let schemas = get_all_schemas(state)
        .await
        .expect("get_all_schemas failed");
    let by_id = |id: &str| {
        schemas
            .iter()
            .find(|s| s.envelope.id == id)
            .unwrap_or_else(|| panic!("the seeded '{id}' schema should be listed"))
    };

    // A core subtype names its parent in `extends`, never as a relationship.
    let native = by_id("ai-chat-native");
    assert_eq!(native.extends.as_deref(), Some("ai-chat"));
    assert!(native.relationships.iter().all(|r| r.name != "extends"));

    // A type with declared relationships lists them.
    let task = by_id("task");
    assert_eq!(task.extends, None);
    assert!(
        task.relationships.iter().any(|r| r.name == "blocks"),
        "task should list its declared relationships, got {:?}",
        task.relationships
            .iter()
            .map(|r| &r.name)
            .collect::<Vec<_>>()
    );

    // What crosses to the frontend: typed top-level fields, nothing smuggled
    // through `properties`, and no always-empty `description`.
    let wire = serde_json::to_value(task).unwrap();
    assert_eq!(wire["nodeType"], json!("schema"));
    assert_eq!(wire["properties"], json!({}));
    assert!(wire["relationships"].is_array());
    assert!(wire.get("description").is_none());
}

#[tokio::test]
async fn get_schema_definition_returns_relationships_and_extends() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let state = harness.client_state();

    let native = get_schema_definition(state.clone(), "ai-chat-native".to_string())
        .await
        .expect("get_schema_definition failed");
    assert_eq!(native.envelope.id, "ai-chat-native");
    assert_eq!(native.extends.as_deref(), Some("ai-chat"));

    let task = get_schema_definition(state.clone(), "task".to_string())
        .await
        .expect("get_schema_definition failed");
    assert!(task.relationships.iter().any(|r| r.name == "blocks"));

    let missing = get_schema_definition(state, "no-such-type".to_string())
        .await
        .expect_err("an unknown schema id is an error");
    assert_eq!(missing.code, "SCHEMA_NOT_FOUND");
}
