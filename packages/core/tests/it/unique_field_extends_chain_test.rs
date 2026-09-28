//! `find_duplicate_for` and `detect_unique_field_collisions`, against a
//! `unique`/`unique_case_insensitive` field declared only on an ancestor
//! schema and inherited (not redeclared) by a subtype.
//!
//! Both functions used to resolve a node type's uniqueness-flagged fields
//! via a direct own-schema lookup (`get_schema_node`), never the ADR-078
//! `extends`-chain-merged field set (`NodeService::resolve_field_owners`).
//! An inherited-only unique field was invisible to both, so a real
//! conflicting value on a subtype instance never surfaced a duplicate
//! suggestion and never got journaled as a `UniqueFieldCollision` conflict.
//!
//! Same fix pattern as `workflow_state.rs`, `validation.rs`,
//! `graph_resolver.rs`'s `is_declared_many_relationship`, and
//! `rel_ops.rs`'s `resolve_relationship_name`/`get_node_relationships`.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::conflict::{ConflictKind, ConflictStatus};
use nodespace_core::models::Node;
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::NodeService;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);
    Ok((node_service, temp_dir))
}

/// Base type declares `email` as `unique`/`uniqueCaseInsensitive`; the
/// subtype extends the base without redeclaring the field.
async fn create_base_and_subtype(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "unique_ext_base",
            "fields": [{
                "name": "email",
                "type": "string",
                "protection": "user",
                "indexed": false,
                "unique": true,
                "uniqueCaseInsensitive": true
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("base schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "unique_ext_sub",
            "extends": "unique_ext_base",
            "fields": []
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("subtype schema: {e}"))?;
    Ok(())
}

async fn open_unique_field_collisions(
    service: &NodeService,
    node_id: &str,
) -> Result<Vec<nodespace_core::models::ConflictRecord>> {
    let records = service.conflicts_for_node(node_id).await?;
    Ok(records
        .into_iter()
        .filter(|r| {
            r.kind == ConflictKind::UniqueFieldCollision && r.status == ConflictStatus::Open
        })
        .collect())
}

/// `find_duplicate_for`: a subtype instance with a real conflicting value on
/// the inherited unique field must be surfaced as a duplicate.
#[tokio::test]
async fn find_duplicate_for_sees_inherited_unique_field() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;

    let existing_id = svc
        .create_node(Node::new(
            "unique_ext_sub".to_string(),
            "Existing".to_string(),
            json!({ "unique_ext_sub": { "email": "alice@example.com" } }),
        ))
        .await?;

    let dup = svc
        .find_duplicate_for("unique_ext_sub", "email", "alice@example.com", None)
        .await?;

    assert_eq!(
        dup.map(|n| n.id),
        Some(existing_id),
        "a unique field declared only on the ancestor schema must still be \
         resolved for a subtype and surface a real conflicting value"
    );
    Ok(())
}

/// `find_duplicate_for`: no conflicting value on the inherited unique field
/// must still return `None` (no false positive).
#[tokio::test]
async fn find_duplicate_for_inherited_unique_field_no_conflict() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;

    svc.create_node(Node::new(
        "unique_ext_sub".to_string(),
        "Existing".to_string(),
        json!({ "unique_ext_sub": { "email": "alice@example.com" } }),
    ))
    .await?;

    let dup = svc
        .find_duplicate_for("unique_ext_sub", "email", "bob@example.com", None)
        .await?;

    assert!(dup.is_none());
    Ok(())
}

/// `detect_unique_field_collisions`: two subtype instances sharing the same
/// value on an inherited unique field must be journaled as a
/// `UniqueFieldCollision`, automatically, on the second create.
#[tokio::test]
async fn detect_unique_field_collisions_sees_inherited_unique_field() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;

    let first_id = svc
        .create_node(Node::new(
            "unique_ext_sub".to_string(),
            "First".to_string(),
            json!({ "unique_ext_sub": { "email": "alice@example.com" } }),
        ))
        .await?;

    let second_id = svc
        .create_node(Node::new(
            "unique_ext_sub".to_string(),
            "Second".to_string(),
            json!({ "unique_ext_sub": { "email": "ALICE@example.com" } }),
        ))
        .await?;

    let first_records = open_unique_field_collisions(&svc, &first_id).await?;
    let second_records = open_unique_field_collisions(&svc, &second_id).await?;
    assert_eq!(
        first_records.len(),
        1,
        "an inherited unique field collision must be journaled for the first node"
    );
    assert_eq!(
        second_records.len(),
        1,
        "an inherited unique field collision must be journaled for the second node"
    );
    assert_eq!(first_records[0].id, second_records[0].id);
    assert_eq!(first_records[0].detail["field"], "email");

    Ok(())
}

/// `detect_unique_field_collisions`: two subtype instances with genuinely
/// distinct values on the inherited unique field must never be journaled.
#[tokio::test]
async fn detect_unique_field_collisions_inherited_unique_field_no_conflict() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;

    let first_id = svc
        .create_node(Node::new(
            "unique_ext_sub".to_string(),
            "First".to_string(),
            json!({ "unique_ext_sub": { "email": "alice@example.com" } }),
        ))
        .await?;

    let second_id = svc
        .create_node(Node::new(
            "unique_ext_sub".to_string(),
            "Second".to_string(),
            json!({ "unique_ext_sub": { "email": "bob@example.com" } }),
        ))
        .await?;

    assert!(open_unique_field_collisions(&svc, &first_id)
        .await?
        .is_empty());
    assert!(open_unique_field_collisions(&svc, &second_id)
        .await?
        .is_empty());

    Ok(())
}
