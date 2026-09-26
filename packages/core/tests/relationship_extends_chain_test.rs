//! Regression coverage: `delete_relationship`, `update_relationship_properties`,
//! and `get_relationship_graph` must resolve a relationship declaration
//! through the ADR-078 extends chain, not just the node's own schema.
//!
//! `create_relationship` (via `resolve_declared_relationship`) already walks
//! `resolve_type_chain` to find a relationship declared only on an ancestor
//! schema. These three sibling methods did an own-schema-only lookup instead,
//! so an inherited-but-unredeclared relationship silently lost required-edge
//! protection, edge-field enum validation, and graph visibility on a subtype
//! instance — even though the exact same edge is fully valid and creatable.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore, models::Node, schema::handle_create_schema, services::NodeService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

/// Build a NodeService with an ancestor `gizmo` schema declaring a
/// `required: true`, enum-edge-fielded `assigned_to -> person` relationship,
/// and a `gadget` schema that `extends: gizmo` without redeclaring it. Every
/// edge created against a `gadget` instance therefore exercises the
/// extends-chain lookup, not the node's own (empty) relationship set.
async fn service_with_extending_schemas() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);

    handle_create_schema(
        &node_service,
        json!({
            "name": "Gizmo",
            "fields": [],
            "relationships": [
                {
                    "name": "assigned_to",
                    "targetType": "person",
                    "direction": "out",
                    "cardinality": "many",
                    "required": true,
                    "reverseName": "gizmos",
                    "reverseCardinality": "many",
                    "edgeFields": [
                        {
                            "name": "access",
                            "type": "enum",
                            "coreValues": [
                                { "value": "owner", "label": "Owner" },
                                { "value": "viewer", "label": "Viewer" }
                            ]
                        }
                    ]
                }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("failed to create gizmo schema: {e}"))?;

    handle_create_schema(
        &node_service,
        json!({ "name": "Gadget", "extends": "gizmo", "fields": [] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("failed to create gadget schema: {e}"))?;

    Ok((node_service, temp_dir))
}

async fn make_node(svc: &NodeService, id: &str, node_type: &str, title: &str) -> Result<()> {
    svc.create_node(Node::new_with_id(
        id.to_string(),
        node_type.to_string(),
        title.to_string(),
        json!({}),
    ))
    .await?;
    Ok(())
}

#[tokio::test]
async fn delete_relationship_protects_last_edge_of_an_inherited_required_relationship() -> Result<()>
{
    let (svc, _t) = service_with_extending_schemas().await?;
    make_node(&svc, "gadget-1", "gadget", "Widget").await?;
    make_node(&svc, "person-1", "person", "").await?;

    // `assigned_to` is declared `required: true` on `gizmo`, not redeclared on
    // `gadget`, and the create path already accepts it on a `gadget` instance
    // via the extends chain.
    svc.create_relationship(
        "gadget-1",
        "assigned_to",
        "person-1",
        json!({ "access": "owner" }),
    )
    .await?;

    let err = svc
        .delete_relationship("gadget-1", "assigned_to", "person-1")
        .await;
    assert!(
        err.is_err(),
        "deleting the last edge of an inherited required relationship must be \
         rejected, not silently allowed because the subtype's own schema \
         doesn't redeclare it"
    );
    Ok(())
}

#[tokio::test]
async fn update_relationship_properties_validates_inherited_enum_edge_fields() -> Result<()> {
    let (svc, _t) = service_with_extending_schemas().await?;
    make_node(&svc, "gadget-1", "gadget", "Widget").await?;
    make_node(&svc, "person-1", "person", "").await?;

    svc.create_relationship(
        "gadget-1",
        "assigned_to",
        "person-1",
        json!({ "access": "owner" }),
    )
    .await?;

    let err = svc
        .update_relationship_properties(
            "gadget-1",
            "assigned_to",
            "person-1",
            json!({ "access": "not-a-real-value" }),
        )
        .await;
    assert!(
        err.is_err(),
        "an edge-field enum declared only on an ancestor schema must still be \
         validated on an in-place update from a subtype instance"
    );
    Ok(())
}

#[tokio::test]
async fn get_relationship_graph_includes_inherited_relationships() -> Result<()> {
    let (svc, _t) = service_with_extending_schemas().await?;

    let graph = svc.get_relationship_graph().await?;
    assert!(
        graph.iter().any(|(source, name, target)| source == "gadget"
            && name == "assigned_to"
            && target.as_deref() == Some("person")),
        "graph must surface a relationship declared only on an ancestor schema \
         as belonging to the extending subtype too, got: {graph:?}"
    );
    Ok(())
}
