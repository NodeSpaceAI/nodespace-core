//! Integration tests for the relationship-viewer editing surface.
//!
//! Covers the write paths added for the editable modal:
//! - `update_relationship_properties` (edit an edge's stored `edge_fields` values)
//! - `required` last-edge protection in `delete_relationship`
//!
//! These exercise `NodeService` directly (the layer the daemon RPCs and Tauri
//! commands forward to), building a schema whose typed relationship carries the
//! cardinality/required flags under test.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::{rel_ops, OpsError},
    schema::handle_create_schema,
    services::NodeService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

/// Build a NodeService plus a `gizmo` schema declaring an `assigned_to → person`
/// relationship. Registered through `handle_create_schema` (the real schema
/// path), so the relationship persists exactly where `create_relationship` reads
/// it back. `person` is a seeded core schema, so it satisfies the target-type
/// existence check. `cardinality`/`required` are parameterized per test.
async fn service_with_gizmo_schema(
    cardinality: &str,
    required: bool,
) -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);

    handle_create_schema(
        &node_service,
        json!({
            "name": "Gizmo",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ],
            "relationships": [
                {
                    "name": "assigned_to",
                    "targetType": "person",
                    "direction": "out",
                    "cardinality": cardinality,
                    "required": required,
                    "reverseName": "gizmos",
                    "reverseCardinality": "many"
                }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("failed to create gizmo schema: {e}"))?;

    Ok((node_service, temp_dir))
}

/// Insert a bare instance node of a given type.
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
async fn update_relationship_properties_overwrites_edge_attributes() -> Result<()> {
    let (svc, _t) = service_with_gizmo_schema("many", false).await?;
    make_node(&svc, "gizmo-1", "gizmo", "Ship it").await?;
    make_node(&svc, "person-1", "person", "Alice").await?;

    svc.create_relationship(
        "gizmo-1",
        "assigned_to",
        "person-1",
        json!({ "role": "reviewer" }),
    )
    .await?;

    // Overwrite the edge's stored properties wholesale.
    svc.update_relationship_properties(
        "gizmo-1",
        "assigned_to",
        "person-1",
        json!({ "role": "owner" }),
    )
    .await?;

    let edges = svc
        .get_related_nodes_with_edges("gizmo-1", "assigned_to", "out")
        .await?;
    assert_eq!(
        edges.len(),
        1,
        "still exactly one edge after an in-place update"
    );
    let (_node, props) = &edges[0];
    assert_eq!(
        props.get("role").and_then(|v| v.as_str()),
        Some("owner"),
        "role must reflect the update, not the original value"
    );
    Ok(())
}

#[tokio::test]
async fn update_relationship_properties_on_missing_edge_errors() -> Result<()> {
    let (svc, _t) = service_with_gizmo_schema("many", false).await?;
    make_node(&svc, "gizmo-1", "gizmo", "Ship it").await?;
    make_node(&svc, "person-1", "person", "Alice").await?;

    // No edge was ever created — updating one must be a surfaced error, not a
    // silent no-op that reports success to the UI.
    let err = svc
        .update_relationship_properties(
            "gizmo-1",
            "assigned_to",
            "person-1",
            json!({ "role": "x" }),
        )
        .await;
    assert!(err.is_err(), "updating a nonexistent edge must error");
    Ok(())
}

#[tokio::test]
async fn required_relationship_blocks_deleting_its_last_edge() -> Result<()> {
    let (svc, _t) = service_with_gizmo_schema("many", true).await?;
    make_node(&svc, "gizmo-1", "gizmo", "Ship it").await?;
    make_node(&svc, "person-1", "person", "Alice").await?;
    make_node(&svc, "person-2", "person", "Bob").await?;

    svc.create_relationship("gizmo-1", "assigned_to", "person-1", json!({}))
        .await?;

    // Single edge on a required relationship → deleting it is rejected.
    let last = svc
        .delete_relationship("gizmo-1", "assigned_to", "person-1")
        .await;
    assert!(
        last.is_err(),
        "deleting the last edge of a required relationship must be rejected"
    );

    // Add a second edge; now removing either one is allowed (one remains).
    svc.create_relationship("gizmo-1", "assigned_to", "person-2", json!({}))
        .await?;
    svc.delete_relationship("gizmo-1", "assigned_to", "person-1")
        .await
        .expect("removing a non-last edge of a required relationship is allowed");

    // Back to one edge → protected again.
    let now_last = svc
        .delete_relationship("gizmo-1", "assigned_to", "person-2")
        .await;
    assert!(
        now_last.is_err(),
        "once only one edge remains, a required relationship protects it again"
    );
    Ok(())
}

/// A `cardinality: One` relationship that is ALSO `required: true` must
/// still be reassignable to a different target in one call — the whole
/// point of `cardinality: One`'s replace semantics.
///
/// This is the trap a naive evict-then-insert ordering falls into:
/// `remove_relationship_in_tx`'s required-last-edge guard counts the
/// source's current edges for this relationship type, which under
/// `cardinality: One` is always exactly 1 at eviction time (the
/// replacement has not landed yet) — so evicting first always looks like
/// "this is the last edge" and always gets rejected, even though a
/// replacement is about to be inserted in the very same transaction and
/// the invariant is never actually violated. The fix is ordering (insert
/// the new edge, THEN evict the old one), not a special case — a real
/// deletion with no replacement (`required_relationship_blocks_deleting_its_last_edge`,
/// above) must still correctly stay rejected.
#[tokio::test]
async fn required_cardinality_one_relationship_can_still_be_reassigned() -> Result<()> {
    let (svc, _t) = service_with_gizmo_schema("one", true).await?;
    make_node(&svc, "gizmo-1", "gizmo", "Ship it").await?;
    make_node(&svc, "person-1", "person", "Alice").await?;
    make_node(&svc, "person-2", "person", "Bob").await?;

    svc.create_relationship("gizmo-1", "assigned_to", "person-1", json!({}))
        .await?;

    svc.create_relationship("gizmo-1", "assigned_to", "person-2", json!({}))
        .await
        .expect(
            "reassigning a required, cardinality-one relationship to a different \
             target must succeed, not be rejected as \"deleting the last edge\"",
        );

    let edges = svc
        .get_related_nodes("gizmo-1", "assigned_to", "out")
        .await?;
    assert_eq!(
        edges.len(),
        1,
        "exactly one edge must remain after reassignment"
    );
    assert_eq!(
        edges[0].id, "person-2",
        "the survivor must be the new target"
    );

    // The relationship is still genuinely required and still protected once
    // there is again exactly one edge — the fix must not have accidentally
    // disabled the guard rather than just reordering around it.
    let last = svc
        .delete_relationship("gizmo-1", "assigned_to", "person-2")
        .await;
    assert!(
        last.is_err(),
        "a plain delete (no replacement) of the last edge must still be rejected"
    );

    Ok(())
}

#[tokio::test]
async fn non_required_relationship_allows_deleting_its_last_edge() -> Result<()> {
    let (svc, _t) = service_with_gizmo_schema("many", false).await?;
    make_node(&svc, "gizmo-1", "gizmo", "Ship it").await?;
    make_node(&svc, "person-1", "person", "Alice").await?;

    svc.create_relationship("gizmo-1", "assigned_to", "person-1", json!({}))
        .await?;
    svc.delete_relationship("gizmo-1", "assigned_to", "person-1")
        .await
        .expect("a non-required relationship never blocks removal");

    let edges = svc
        .get_related_nodes_with_edges("gizmo-1", "assigned_to", "out")
        .await?;
    assert!(edges.is_empty(), "the edge is gone after deletion");
    Ok(())
}

#[tokio::test]
async fn delete_required_last_edge_surfaces_a_validation_error_not_internal() -> Result<()> {
    // The rejection is user-actionable ("add another target first"), so the ops
    // layer must classify it as a validation error — which the daemon maps to
    // gRPC INVALID_ARGUMENT — not an opaque Internal/INTERNAL server error.
    let (svc, _t) = service_with_gizmo_schema("many", true).await?;
    make_node(&svc, "gizmo-1", "gizmo", "Ship it").await?;
    make_node(&svc, "person-1", "person", "Alice").await?;
    svc.create_relationship("gizmo-1", "assigned_to", "person-1", json!({}))
        .await?;

    let err = rel_ops::delete_relationship(
        &svc,
        rel_ops::DeleteRelInput {
            source_id: "gizmo-1".to_string(),
            relationship_name: "assigned_to".to_string(),
            target_id: "person-1".to_string(),
        },
    )
    .await
    .expect_err("deleting a required last edge must fail");
    assert!(
        matches!(err, OpsError::ValidationFailed(_)),
        "expected ValidationFailed (→ INVALID_ARGUMENT), got {err:?}"
    );
    Ok(())
}

#[tokio::test]
async fn update_rejects_non_object_properties() -> Result<()> {
    let (svc, _t) = service_with_gizmo_schema("many", false).await?;
    make_node(&svc, "gizmo-1", "gizmo", "Ship it").await?;
    make_node(&svc, "person-1", "person", "Alice").await?;
    svc.create_relationship(
        "gizmo-1",
        "assigned_to",
        "person-1",
        json!({ "role": "reviewer" }),
    )
    .await?;

    // A scalar/array/null would replace the structured edge blob with a shape
    // downstream readers don't expect — reject it.
    for bad in [json!("oops"), json!(42), json!(null), json!(["a"])] {
        let err = svc
            .update_relationship_properties("gizmo-1", "assigned_to", "person-1", bad.clone())
            .await;
        assert!(err.is_err(), "non-object properties {bad} must be rejected");
    }

    // The valid edge is untouched by the rejected updates.
    let edges = svc
        .get_related_nodes_with_edges("gizmo-1", "assigned_to", "out")
        .await?;
    assert_eq!(
        edges[0].1.get("role").and_then(|v| v.as_str()),
        Some("reviewer"),
        "a rejected update must not mutate the edge"
    );
    Ok(())
}
