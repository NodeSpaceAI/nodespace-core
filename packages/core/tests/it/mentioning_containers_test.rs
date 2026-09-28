//! Regression coverage for `get_incoming_mention_containers`.
//!
//! The backend resolves backlinks by walking each mentioning source up to its
//! container (a task/ai-chat node is its own container; everything else walks
//! `has_child` up to its root). This used to be a per-source 2N-query loop;
//! it is now a single recursive CTE. These tests lock the *behavior* (correct
//! containers, dedup, task/ai-chat self-containment) so the query can be
//! rewritten freely as long as results stay identical. The separate query-plan
//! regression check lives next to the query itself — see
//! `src/db/sqlite_store/relationships.rs`.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore, models::Node, services::InsertPosition, services::NodeService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const TEST_CLIENT_ID: &str = "test-client";

async fn create_test_service() -> Result<(NodeService, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = NodeService::new(&mut store).await?;
    Ok((service, temp_dir))
}

async fn create_typed_node(service: &NodeService, node_type: &str) -> Result<Node> {
    let node = Node::new(node_type.to_string(), "test".to_string(), json!({}));
    service
        .with_client(TEST_CLIENT_ID)
        .create_node(node.clone())
        .await?;
    let created = service
        .get_node(&node.id)
        .await?
        .expect("node should exist after create");
    Ok(created)
}

async fn create_node(service: &NodeService) -> Result<Node> {
    create_typed_node(service, "text").await
}

async fn place_under(service: &NodeService, child_id: &str, parent_id: &str) -> Result<()> {
    service
        .with_client(TEST_CLIENT_ID)
        .move_node_unchecked(child_id, Some(parent_id), InsertPosition::End)
        .await?;
    Ok(())
}

/// Multi-level nesting: a grandchild several levels below a root mentions the
/// target. The container returned must be the root, not the grandchild or any
/// intermediate ancestor.
#[tokio::test]
async fn resolves_deeply_nested_source_to_its_root() -> Result<()> {
    let (service, _t) = create_test_service().await?;

    let target = create_node(&service).await?;
    let root = create_node(&service).await?;
    let child = create_node(&service).await?;
    let grandchild = create_node(&service).await?;

    place_under(&service, &child.id, &root.id).await?;
    place_under(&service, &grandchild.id, &child.id).await?;

    service.create_mention(&grandchild.id, &target.id).await?;

    let containers = service.get_mentioning_containers(&target.id).await?;
    let ids: Vec<&str> = containers.iter().map(|c| c.id.as_str()).collect();

    assert_eq!(ids, vec![root.id.as_str()]);
    Ok(())
}

/// A root-level node mentioning the target is its own container (no ancestors
/// to walk to).
#[tokio::test]
async fn root_level_source_is_its_own_container() -> Result<()> {
    let (service, _t) = create_test_service().await?;

    let target = create_node(&service).await?;
    let source_root = create_node(&service).await?;

    service.create_mention(&source_root.id, &target.id).await?;

    let containers = service.get_mentioning_containers(&target.id).await?;
    let ids: Vec<&str> = containers.iter().map(|c| c.id.as_str()).collect();

    assert_eq!(ids, vec![source_root.id.as_str()]);
    Ok(())
}

/// A `task` node is its own container even when nested under a root — the
/// walk must stop at the task, not continue to its root ancestor.
#[tokio::test]
async fn task_source_is_its_own_container_even_when_nested() -> Result<()> {
    let (service, _t) = create_test_service().await?;

    let target = create_node(&service).await?;
    let root = create_node(&service).await?;
    let task = create_typed_node(&service, "task").await?;

    place_under(&service, &task.id, &root.id).await?;

    service.create_mention(&task.id, &target.id).await?;

    let containers = service.get_mentioning_containers(&target.id).await?;
    let ids: Vec<&str> = containers.iter().map(|c| c.id.as_str()).collect();

    assert_eq!(ids, vec![task.id.as_str()]);
    Ok(())
}

/// An `ai-chat` node is likewise its own container — matches the documented
/// behavior in components/mentions-and-references.md ("Task/AI-chat nodes:
/// Treated as their own containers").
#[tokio::test]
async fn ai_chat_source_is_its_own_container_even_when_nested() -> Result<()> {
    let (service, _t) = create_test_service().await?;

    let target = create_node(&service).await?;
    let root = create_node(&service).await?;
    let ai_chat = create_typed_node(&service, "ai-chat").await?;

    place_under(&service, &ai_chat.id, &root.id).await?;

    service.create_mention(&ai_chat.id, &target.id).await?;

    let containers = service.get_mentioning_containers(&target.id).await?;
    let ids: Vec<&str> = containers.iter().map(|c| c.id.as_str()).collect();

    assert_eq!(ids, vec![ai_chat.id.as_str()]);
    Ok(())
}

/// Two children of the same root both mention the target — the container
/// list must dedup to a single entry for that root.
#[tokio::test]
async fn dedupes_multiple_mentions_from_the_same_container() -> Result<()> {
    let (service, _t) = create_test_service().await?;

    let target = create_node(&service).await?;
    let root = create_node(&service).await?;
    let child_a = create_node(&service).await?;
    let child_b = create_node(&service).await?;

    place_under(&service, &child_a.id, &root.id).await?;
    place_under(&service, &child_b.id, &root.id).await?;

    service.create_mention(&child_a.id, &target.id).await?;
    service.create_mention(&child_b.id, &target.id).await?;

    let containers = service.get_mentioning_containers(&target.id).await?;
    let ids: Vec<&str> = containers.iter().map(|c| c.id.as_str()).collect();

    assert_eq!(ids, vec![root.id.as_str()]);
    Ok(())
}

/// Mentions from multiple distinct containers (a root and an unrelated task)
/// both resolve, independently, with no cross-contamination.
#[tokio::test]
async fn resolves_multiple_distinct_containers() -> Result<()> {
    let (service, _t) = create_test_service().await?;

    let target = create_node(&service).await?;

    let root_a = create_node(&service).await?;
    let child_a = create_node(&service).await?;
    place_under(&service, &child_a.id, &root_a.id).await?;

    let task_b = create_typed_node(&service, "task").await?;

    service.create_mention(&child_a.id, &target.id).await?;
    service.create_mention(&task_b.id, &target.id).await?;

    let containers = service.get_mentioning_containers(&target.id).await?;
    let mut ids: Vec<&str> = containers.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();

    let mut expected = vec![root_a.id.as_str(), task_b.id.as_str()];
    expected.sort_unstable();

    assert_eq!(ids, expected);
    Ok(())
}

/// No mentions at all resolves to an empty list, not an error.
#[tokio::test]
async fn returns_empty_when_no_mentions_exist() -> Result<()> {
    let (service, _t) = create_test_service().await?;
    let target = create_node(&service).await?;

    let containers = service.get_mentioning_containers(&target.id).await?;
    assert!(containers.is_empty());
    Ok(())
}

// The query-plan regression check (confirming the recursive CTE in
// `get_incoming_mention_containers` rides `idx_rel_out` rather than
// `idx_rel_type`) lives as a crate-internal unit test next to the query
// itself in `src/db/sqlite_store/relationships.rs`, since `SqliteStore::read`
// is `pub(crate)` and not reachable from this integration test crate.
