//! The `database-settings` field `required_extensions` (ADR-083 §2) against a
//! real store: its default, where it is stored, how it survives a retype to a
//! subtype, and what it accepts.

use nodespace_core::db::required_extensions::read_required_extensions;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::NodeUpdate;
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::NodeService;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

const SETTINGS_ID: &str = "database-settings-singleton";

async fn service() -> (Arc<NodeService>, PathBuf, TempDir) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(path.clone()).await.unwrap());
    let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
    (node_service, path, dir)
}

async fn set_required(
    node_service: &NodeService,
    value: serde_json::Value,
) -> Result<(), nodespace_core::services::NodeServiceError> {
    let settings = node_service.get_node(SETTINGS_ID).await?.unwrap();
    node_service
        .update_node(
            &settings.id,
            settings.version,
            NodeUpdate::new().with_properties(json!({ "required_extensions": value })),
        )
        .await
        .map(|_| ())
}

#[tokio::test]
async fn the_seeded_singleton_requires_nothing_in_its_base_bucket() {
    let (node_service, path, _dir) = service().await;

    let settings = node_service.get_node(SETTINGS_ID).await.unwrap().unwrap();

    assert_eq!(
        settings.properties["database-settings"]["required_extensions"],
        json!([]),
        "got {}",
        settings.properties
    );
    assert!(read_required_extensions(&path).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_list_survives_a_retype_to_a_subtype() {
    let (node_service, path, _dir) = service().await;
    handle_create_schema(
        &node_service,
        json!({
            "name": "Fixture Settings",
            "extends": "database-settings",
            "fields": [
                { "name": "fixture_flag", "type": "boolean", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .unwrap();
    set_required(&node_service, json!(["fixture"]))
        .await
        .unwrap();

    let settings = node_service.get_node(SETTINGS_ID).await.unwrap().unwrap();
    let retyped = node_service
        .update_node(
            &settings.id,
            settings.version,
            NodeUpdate {
                node_type: Some("fixture-settings".to_string()),
                properties: Some(json!({ "fixture_flag": true })),
                ..NodeUpdate::new()
            },
        )
        .await
        .unwrap();

    assert_eq!(retyped.node_type, "fixture-settings");
    let stored = node_service.get_node(SETTINGS_ID).await.unwrap().unwrap();
    assert_eq!(
        stored.properties["database-settings"]["required_extensions"],
        json!(["fixture"]),
        "got {}",
        stored.properties
    );
    assert_eq!(
        read_required_extensions(&path).await.unwrap(),
        vec!["fixture".to_string()]
    );
}

#[tokio::test]
async fn anything_but_a_list_of_strings_is_rejected() {
    let (node_service, path, _dir) = service().await;

    for bad in [json!(["fixture", 1]), json!("fixture"), json!(7)] {
        assert!(
            set_required(&node_service, bad.clone()).await.is_err(),
            "{bad} must be rejected"
        );
    }
    assert!(read_required_extensions(&path).await.unwrap().is_empty());

    set_required(&node_service, json!(["fixture"]))
        .await
        .unwrap();
    assert_eq!(
        read_required_extensions(&path).await.unwrap(),
        vec!["fixture".to_string()]
    );
}
