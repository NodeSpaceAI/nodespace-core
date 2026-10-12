//! A schema's `fields` have one write path, `update_schema` (with
//! `create_schema` for a new type). A generic node update or create skips the
//! chain, namespace, protection and instance checks those two run, so it is
//! refused, as it is for a schema's structural rules and context paths.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeUpdate};
use nodespace_core::schema::{handle_create_schema, handle_update_schema};
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let mut store = Arc::new(SqliteStore::new(temp_dir.path().join("test.db")).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    handle_create_schema(
        &service,
        json!({ "name": "Ticket", "fields": [{ "name": "state", "type": "text" }] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("creating the schema failed: {e}"))?;
    Ok((service, temp_dir))
}

async fn stored_fields(service: &NodeService) -> Value {
    let row = service.get_node("ticket").await.unwrap().unwrap();
    row.properties["fields"].clone()
}

fn bare_field_patch() -> NodeUpdate {
    NodeUpdate::new().with_properties(json!({
        "fields": [
            { "name": "state", "type": "text" },
            { "name": "priority", "type": "text" }
        ]
    }))
}

fn assert_refused_naming_update_schema(error: &impl std::fmt::Display) {
    let message = error.to_string();
    assert!(
        message.contains("update_schema") && message.contains("fields"),
        "{message}"
    );
}

/// Every generic update path refuses a change to `fields`, names
/// `update_schema`, and leaves the schema as it was.
#[tokio::test]
async fn a_generic_update_cannot_change_a_schemas_fields() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let before = stored_fields(&service).await;
    let version = service.get_node("ticket").await?.unwrap().version;

    let single = service
        .update_node("ticket", version, bare_field_patch())
        .await
        .expect_err("a generic update must not rewrite fields");
    assert_refused_naming_update_schema(&single);

    let unchecked = service
        .update_node_unchecked("ticket", bare_field_patch())
        .await
        .expect_err("the unchecked update must not rewrite fields");
    assert_refused_naming_update_schema(&unchecked);

    let bulk = service
        .bulk_update(vec![("ticket".to_string(), bare_field_patch())])
        .await
        .expect_err("a batch update must not rewrite fields");
    assert_refused_naming_update_schema(&bulk);

    assert_eq!(stored_fields(&service).await, before);
    assert_eq!(
        service.get_node("ticket").await?.unwrap().version,
        version,
        "a refusal writes nothing"
    );
    Ok(())
}

/// A generic update that leaves `fields` as they are still succeeds.
#[tokio::test]
async fn a_generic_update_that_keeps_the_fields_still_succeeds() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let before = stored_fields(&service).await;
    let version = service.get_node("ticket").await?.unwrap().version;

    service
        .update_node(
            "ticket",
            version,
            NodeUpdate::new().with_properties(json!({ "title_template": "{state}" })),
        )
        .await?;
    // The same list sent back is not a change.
    let version = service.get_node("ticket").await?.unwrap().version;
    service
        .update_node(
            "ticket",
            version,
            NodeUpdate::new().with_properties(json!({ "fields": before.clone() })),
        )
        .await?;
    assert_eq!(stored_fields(&service).await, before);
    Ok(())
}

/// A schema node is not created through the generic create with fields on it.
#[tokio::test]
async fn a_schema_is_not_created_with_fields_through_the_generic_create() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    let minted = service
        .create_node(Node::new(
            "schema".to_string(),
            "Sidestep".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "fields": [{ "name": "custom:state", "type": "text" }]
            }),
        ))
        .await
        .expect_err("a schema is not created with fields");
    assert!(minted.to_string().contains("create_schema"), "{minted}");
    assert!(service.get_node("sidestep").await?.is_none());
    Ok(())
}

/// The schema-definition writes are unaffected: `create_schema` stores its
/// fields and `update_schema` adds, renames and removes them.
#[tokio::test]
async fn create_schema_and_update_schema_still_write_fields() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    assert_eq!(stored_fields(&service).await[0]["name"], "state");

    for params in [
        json!({ "schema_id": "ticket", "add_fields": [{ "name": "note", "type": "text" }] }),
        json!({ "schema_id": "ticket", "rename_fields": [{ "from": "note", "to": "remark" }] }),
        json!({ "schema_id": "ticket", "remove_fields": ["remark"] }),
    ] {
        handle_update_schema(&service, params)
            .await
            .map_err(|e| anyhow::anyhow!("update_schema failed: {e}"))?;
    }
    let names: Vec<String> = stored_fields(&service)
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["state"]);
    Ok(())
}
