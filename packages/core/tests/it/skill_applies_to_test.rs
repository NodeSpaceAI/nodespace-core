//! A skill's `applies_to` relationship: the edges from a skill to the schema
//! nodes it is about.
//!
//! The relationship is declared on the `skill` schema with schema nodes as its
//! target, so it is written and read like any other declared relationship,
//! from either end.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, SkillFields, SKILL_APPLIES_TO};
use nodespace_core::ops::rel_ops;
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::NodeService;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

async fn create_skill(service: &NodeService, name: &str) -> Result<String> {
    let node =
        SkillFields::new("Bill a customer for an invoice.", &["create_node"], 2).into_node(name);
    Ok(service.create_node(node).await?)
}

/// The ids `relationship_name` reaches from `node_id`.
async fn related(
    service: &Arc<NodeService>,
    node_id: &str,
    relationship_name: &str,
) -> Vec<String> {
    let output = rel_ops::get_related_nodes(
        service,
        rel_ops::GetRelatedInput {
            node_id: node_id.to_string(),
            relationship_name: relationship_name.to_string(),
            direction: "out".to_string(),
        },
    )
    .await
    .unwrap_or_else(|e| panic!("`{relationship_name}` from {node_id} should resolve: {e:?}"));
    let mut ids: Vec<String> = output
        .related_nodes
        .iter()
        .filter_map(|n| n["id"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    ids
}

/// The edge runs from a skill to a schema node, custom or core, and reads
/// back from both ends: `applies_to` from the skill, `skills` from the schema.
#[tokio::test]
async fn a_skill_links_to_schema_nodes_and_the_edge_reads_from_both_ends() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    handle_create_schema(
        &service,
        json!({ "name": "Invoice", "fields": [{ "name": "amount_due", "type": "number" }] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("invoice schema: {e}"))?;

    let billing = create_skill(&service, "Invoice Billing").await?;
    let closing = create_skill(&service, "Closing Out Work").await?;
    for (skill, schema) in [
        (&billing, "invoice"),
        (&billing, "task"),
        (&closing, "task"),
    ] {
        service
            .create_relationship(skill, SKILL_APPLIES_TO, schema, json!({}))
            .await?;
    }

    assert_eq!(
        related(&service, &billing, "applies_to").await,
        ["invoice", "task"]
    );
    assert_eq!(
        related(&service, "invoice", "skills").await,
        std::slice::from_ref(&billing)
    );
    let mut both = vec![billing, closing];
    both.sort();
    assert_eq!(related(&service, "task", "skills").await, both);
    Ok(())
}

/// The target is a schema node. A link to any other node is refused, so a
/// skill's scope can only ever name types.
#[tokio::test]
async fn a_skill_cannot_apply_to_a_node_that_is_not_a_schema() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let skill = create_skill(&service, "Invoice Billing").await?;
    let task = service
        .create_node(Node::new(
            "task".to_string(),
            "Send the March invoice".to_string(),
            json!({}),
        ))
        .await?;

    let error = service
        .create_relationship(&skill, SKILL_APPLIES_TO, &task, json!({}))
        .await
        .expect_err("a task is not a schema")
        .to_string();
    assert!(
        error.contains("doesn't match expected type 'schema'"),
        "{error}"
    );
    Ok(())
}

/// Declaring `applies_to` on `skill` does not make it a declaration between
/// schemas: a schema a skill links to can still be deleted, and its links go
/// with it.
#[tokio::test]
async fn a_linked_schema_can_still_be_deleted() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    handle_create_schema(
        &service,
        json!({ "name": "Invoice", "fields": [{ "name": "amount_due", "type": "number" }] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("invoice schema: {e}"))?;
    let skill = create_skill(&service, "Invoice Billing").await?;
    service
        .create_relationship(&skill, SKILL_APPLIES_TO, "invoice", json!({}))
        .await?;

    let schema = service.get_node("invoice").await?.expect("schema exists");
    service.delete_node("invoice", schema.version).await?;

    assert!(related(&service, &skill, "applies_to").await.is_empty());
    Ok(())
}
