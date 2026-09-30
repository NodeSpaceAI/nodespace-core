//! Extension keys on the `database-settings` singleton, against a database
//! created by an older build.
//!
//! Core declares no fields on `database-settings`; extensions keep namespaced
//! keys in its bucket and reach them through `NodeService::database_settings`
//! and `NodeService::merge_database_settings`. A database created by an older
//! build still holds the singleton with values in that bucket, and a stored
//! `database-settings` schema node still declaring fields for them:
//! `seed_core_schemas_if_needed` never rewrites a stored core schema.
//!
//! Generic key names are used on purpose so core keeps no extension key names;
//! the mechanics are identical to any declared field.

use anyhow::Result;
use chrono::Utc;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::schema::EnumValue;
use nodespace_core::models::{Node, SchemaField, SchemaNode, SchemaProtectionLevel};
use nodespace_core::services::{NodeService, NodeServiceError};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

/// Id of the seeded singleton instance.
const SINGLETON_ID: &str = "database-settings-singleton";

/// The `database-settings` declaration as an older build stored it.
fn legacy_schema() -> SchemaNode {
    let now = Utc::now();
    let optional_string = |name: &str| SchemaField {
        name: name.to_string(),
        friendly_name: name.to_string(),
        field_type: "string".to_string(),
        protection: SchemaProtectionLevel::Core,
        required: Some(false),
        ..Default::default()
    };
    SchemaNode {
        id: "database-settings".to_string(),
        content: "Database Settings".to_string(),
        version: 1,
        created_at: now,
        modified_at: now,
        is_core: true,
        schema_version: 1,
        fields: vec![
            SchemaField {
                name: "legacy_flag".to_string(),
                friendly_name: "Legacy flag".to_string(),
                field_type: "boolean".to_string(),
                protection: SchemaProtectionLevel::Core,
                required: Some(false),
                default: Some(json!(false)),
                ..Default::default()
            },
            SchemaField {
                name: "legacy_state".to_string(),
                friendly_name: "Legacy state".to_string(),
                field_type: "enum".to_string(),
                protection: SchemaProtectionLevel::Core,
                core_values: Some(vec![EnumValue::new("a", "A"), EnumValue::new("b", "B")]),
                user_values: Some(vec![]),
                indexed: true,
                required: Some(true),
                extensible: Some(false),
                default: Some(json!("a")),
                ..Default::default()
            },
            optional_string("legacy_ref_a"),
            optional_string("legacy_ref_b"),
        ],
        relationships: vec![],
        title_template: None,
        properties_header_summary_template: None,
    }
}

/// The four legacy values, none of them the schema default.
fn legacy_bucket() -> Value {
    json!({
        "legacy_flag": true,
        "legacy_state": "b",
        "legacy_ref_a": "ref-a",
        "legacy_ref_b": "ref-b"
    })
}

/// Lay down a database before any `NodeService` has opened it: an optional
/// stored `database-settings` schema declaration, and the singleton holding
/// `properties`.
async fn create_database(
    schema: Option<SchemaNode>,
    singleton_properties: Value,
) -> Result<(PathBuf, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let store = SqliteStore::new(db_path.clone()).await?;
    if let Some(schema) = schema {
        store.create_node(schema.into_node(), None, None).await?;
    }
    store
        .create_node(
            Node::new_with_id(
                SINGLETON_ID.to_string(),
                "database-settings".to_string(),
                String::new(),
                singleton_properties,
            ),
            None,
            None,
        )
        .await?;
    Ok((db_path, temp_dir))
}

/// A database as the older build left it: the stored schema declaration and
/// the singleton holding the legacy values.
async fn create_legacy_database() -> Result<(PathBuf, TempDir)> {
    create_database(
        Some(legacy_schema()),
        json!({ "database-settings": legacy_bucket() }),
    )
    .await
}

async fn open(db_path: PathBuf) -> Result<NodeService> {
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    Ok(NodeService::new(&mut store).await?)
}

fn as_map(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("expected a JSON object, got {other}"),
    }
}

#[tokio::test]
async fn legacy_database_opens_and_repairs_the_owner_edge() -> Result<()> {
    let (db_path, _temp) = create_legacy_database().await?;

    let service = open(db_path).await?;

    // The legacy singleton has no owner edge; opening repairs it. Assert the
    // edge itself: `get_local_person` falls back to the first person without one.
    let person = service
        .get_local_person()
        .await?
        .expect("the local person resolves");
    assert_eq!(person.node_type, "person");
    let edge = service
        .store()
        .get_relationship_record(&person.id, SINGLETON_ID, "has_role")
        .await?
        .expect("the owner has_role edge is repaired on open");
    assert_eq!(edge.properties["role"], "owner");
    Ok(())
}

#[tokio::test]
async fn legacy_values_are_neither_read_nor_rewritten_by_core() -> Result<()> {
    let (db_path, _temp) = create_legacy_database().await?;
    let service = open(db_path).await?;

    assert_eq!(service.database_settings().await?, as_map(legacy_bucket()));

    // Neither the seed nor the owner-edge repair wrote to the singleton: it is
    // still the one node, at the version it was created at.
    let singletons = service
        .query_nodes_by_type("database-settings", None)
        .await?;
    assert_eq!(singletons.len(), 1);
    assert_eq!(singletons[0].id, SINGLETON_ID);
    assert_eq!(singletons[0].version, 1);
    Ok(())
}

#[tokio::test]
async fn extension_keys_merge_alongside_legacy_values() -> Result<()> {
    let (db_path, _temp) = create_legacy_database().await?;
    let service = open(db_path).await?;

    service
        .merge_database_settings(&[("plugin:example", json!(1))])
        .await?;

    let mut expected = as_map(legacy_bucket());
    expected.insert("plugin:example".to_string(), json!(1));
    assert_eq!(service.database_settings().await?, expected);
    Ok(())
}

#[tokio::test]
async fn stored_declaration_still_validates_legacy_keys() -> Result<()> {
    let (db_path, _temp) = create_legacy_database().await?;
    let service = open(db_path).await?;

    // The stored schema, not core's compiled definition, validates a write to a
    // key it still declares: `legacy_state` admits only `a` and `b`.
    let refused = service
        .merge_database_settings(&[("legacy_state", json!("z"))])
        .await;
    // Both this refusal and core's key guard are `InvalidUpdate`, so the message
    // says which one fired: the stored enum declaration, not the guard.
    match refused {
        Err(NodeServiceError::InvalidUpdate(msg)) => assert!(
            msg.contains("enum field 'legacy_state'"),
            "expected the stored declaration's enum validation, got: {msg}"
        ),
        other => {
            panic!("the stored declaration must refuse an out-of-range legacy_state, got {other:?}")
        }
    }

    // A refused write leaves the stored value as it was.
    assert_eq!(
        service.database_settings().await?["legacy_state"],
        json!("b")
    );

    // A value the stored declaration admits still writes: core's compiled
    // definition declares nothing, so its guard does not stand in the way.
    service
        .merge_database_settings(&[("legacy_state", json!("a"))])
        .await?;
    assert_eq!(
        service.database_settings().await?["legacy_state"],
        json!("a")
    );
    Ok(())
}

#[tokio::test]
async fn stored_schema_node_keeps_its_declared_fields() -> Result<()> {
    let (db_path, _temp) = create_legacy_database().await?;
    let service = open(db_path).await?;

    // Opening does not rewrite the stored core schema.
    let schema = service
        .get_node("database-settings")
        .await?
        .expect("stored database-settings schema node");
    let names: Vec<&str> = schema.properties["fields"]
        .as_array()
        .expect("fields array")
        .iter()
        .map(|field| field["name"].as_str().expect("field name"))
        .collect();
    assert_eq!(
        names,
        [
            "legacy_flag",
            "legacy_state",
            "legacy_ref_a",
            "legacy_ref_b"
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_bucket_that_is_not_an_object_is_an_initialization_error() -> Result<()> {
    let (db_path, _temp) =
        create_database(None, json!({ "database-settings": "not an object" })).await?;
    let service = open(db_path).await?;

    let read = service.database_settings().await.unwrap_err();
    assert!(
        matches!(read, NodeServiceError::InitializationError(_)),
        "expected InitializationError from the read, got {read:?}"
    );
    let merge = service
        .merge_database_settings(&[("plugin:example", json!(1))])
        .await
        .unwrap_err();
    assert!(
        matches!(merge, NodeServiceError::InitializationError(_)),
        "expected InitializationError from the merge, got {merge:?}"
    );
    Ok(())
}
