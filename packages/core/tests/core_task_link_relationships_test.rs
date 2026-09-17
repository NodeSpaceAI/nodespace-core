//! The core task-link relationships, end to end against a seeded database.
//!
//! `task` declares three self-referential links — `blocks`/`blocked_by`,
//! `relates_to`/`related_from`, `duplicates`/`duplicated_by` — and `person`
//! declares `reported_tasks`/`creator` alongside its existing
//! `tasks`/`assignee`. Those declarations are plain `SchemaRelationship`s
//! seeded through the same `set_schema_declarations` path as
//! `project.tasks`, so the unit tests in `core_schemas.rs` already pin their
//! shape. What they cannot show is that the shape survives seeding and is
//! actually traversable, which is the whole point of declaring them.
//!
//! These tests therefore exercise the seeded database rather than the
//! in-memory schema list:
//!
//! - every declaration lands as a relationship-table row on the right schema
//! - each pair traverses from both ends, and the reverse name agrees with the
//!   forward name read with `direction: "in"` (the self-referential case,
//!   where the declaring and target type are the same, so the schema appears
//!   in its own inbound set)
//! - `creator` resolves to a person while `assignee` stays independent of it
//! - a `blocks` cycle is representable, because nothing validates against one

use anyhow::Result;
use nodespace_core::{db::SqliteStore, models::Node, ops::rel_ops, services::NodeService};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

/// A freshly seeded service — `NodeService::new` runs the core-schema seeding,
/// so `task` and `person` arrive with their declarations already written.
async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);
    Ok((node_service, temp_dir))
}

async fn make_node(svc: &NodeService, id: &str, node_type: &str) -> Result<()> {
    svc.create_node(Node::new_with_id(
        id.to_string(),
        node_type.to_string(),
        format!("{id} content"),
        json!({}),
    ))
    .await?;
    Ok(())
}

fn get(node_id: &str, name: &str, direction: &str) -> rel_ops::GetRelatedInput {
    rel_ops::GetRelatedInput {
        node_id: node_id.to_string(),
        relationship_name: name.to_string(),
        direction: direction.to_string(),
    }
}

/// Seeding writes the declarations as relationship rows, not as JSON on the
/// schema node — the storage invariant every declaration reader depends on.
#[tokio::test]
async fn core_link_declarations_are_seeded_as_relationship_rows() -> Result<()> {
    let (svc, _t) = create_test_service().await?;

    let task_declarations = svc.store().get_schema_declarations("task").await?;
    for (name, reverse_name) in [
        ("blocks", "blocked_by"),
        ("relates_to", "related_from"),
        ("duplicates", "duplicated_by"),
    ] {
        let rel = task_declarations
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("seeded task declares {name}, got: {task_declarations:?}"));
        assert_eq!(rel.target_type.as_deref(), Some("task"));
        assert_eq!(rel.reverse_name, reverse_name);
        assert!(rel.required.is_none(), "{name} must be optional");
    }

    // The new person declaration sits alongside the pre-existing one rather
    // than replacing it — both spellings have to keep working.
    let person_declarations = svc.store().get_schema_declarations("person").await?;
    let reported = person_declarations
        .iter()
        .find(|r| r.name == "reported_tasks")
        .expect("seeded person declares reported_tasks");
    assert_eq!(reported.reverse_name, "creator");
    assert!(reported.required.is_none());
    assert!(
        person_declarations.iter().any(|r| r.name == "tasks"),
        "reported_tasks must not displace the existing assignee declaration"
    );

    Ok(())
}

/// Each self-referential pair traverses from both ends, and the reverse name
/// agrees with the forward name read inbound.
#[tokio::test]
async fn task_link_pairs_traverse_from_both_ends() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    make_node(&svc, "t_source", "task").await?;
    make_node(&svc, "t_target", "task").await?;

    for (name, reverse_name) in [
        ("blocks", "blocked_by"),
        ("relates_to", "related_from"),
        ("duplicates", "duplicated_by"),
    ] {
        svc.create_relationship("t_source", name, "t_target", json!({}))
            .await
            .map_err(|e| anyhow::anyhow!("create {name}: {e}"))?;

        let forward = rel_ops::get_related_nodes(&svc, get("t_source", name, "out")).await?;
        assert_eq!(forward.count, 1, "{name} should traverse forward");
        assert_eq!(forward.related_nodes[0]["id"], "t_target");

        // The reverse spelling, from the other end. Both the forward and the
        // reverse name live on `task` here, so this only resolves if
        // self-referential declarations are handled.
        let reverse =
            rel_ops::get_related_nodes(&svc, get("t_target", reverse_name, "out")).await?;
        assert_eq!(
            reverse.count, 1,
            "{reverse_name} must resolve rather than return a silent zero: {reverse:?}"
        );
        assert_eq!(reverse.related_nodes[0]["id"], "t_source");

        // ...and agree with the same traversal spelled the long way.
        let inbound = rel_ops::get_related_nodes(&svc, get("t_target", name, "in")).await?;
        assert_eq!(
            inbound.related_nodes[0]["id"], reverse.related_nodes[0]["id"],
            "{reverse_name} and `{name} --direction in` must agree"
        );
    }

    Ok(())
}

/// A task's creator is independent of its assignee: different relationship,
/// different person, neither shadowing the other.
#[tokio::test]
async fn creator_resolves_independently_of_assignee() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    make_node(&svc, "p_reporter", "person").await?;
    make_node(&svc, "p_assignee", "person").await?;
    make_node(&svc, "t1", "task").await?;

    svc.create_relationship("p_reporter", "reported_tasks", "t1", json!({}))
        .await
        .map_err(|e| anyhow::anyhow!("reported_tasks: {e}"))?;
    svc.create_relationship("p_assignee", "tasks", "t1", json!({}))
        .await
        .map_err(|e| anyhow::anyhow!("tasks: {e}"))?;

    let creator = rel_ops::get_related_nodes(&svc, get("t1", "creator", "out")).await?;
    assert_eq!(creator.count, 1, "creator must resolve: {creator:?}");
    assert_eq!(creator.related_nodes[0]["id"], "p_reporter");

    let assignee = rel_ops::get_related_nodes(&svc, get("t1", "assignee", "out")).await?;
    assert_eq!(assignee.count, 1);
    assert_eq!(
        assignee.related_nodes[0]["id"], "p_assignee",
        "the new declaration must not shadow the existing assignee reverse name"
    );

    // And from the person's end, the reporter's queues stay distinct.
    let reported =
        rel_ops::get_related_nodes(&svc, get("p_reporter", "reported_tasks", "out")).await?;
    assert_eq!(reported.count, 1);
    assert_eq!(reported.related_nodes[0]["id"], "t1");
    let reporter_assigned =
        rel_ops::get_related_nodes(&svc, get("p_reporter", "tasks", "out")).await?;
    assert_eq!(
        reporter_assigned.count, 0,
        "reporting a task must not make the reporter its assignee"
    );

    Ok(())
}

/// `A blocks B blocks A` is representable. Cycle detection is required only for
/// ADR-078's `extends`; a general relationship permits one, and a blocking
/// cycle is real data a user can enter and needs to be able to see.
#[tokio::test]
async fn blocking_cycle_is_representable() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    make_node(&svc, "t_a", "task").await?;
    make_node(&svc, "t_b", "task").await?;

    svc.create_relationship("t_a", "blocks", "t_b", json!({}))
        .await
        .map_err(|e| anyhow::anyhow!("a blocks b: {e}"))?;
    svc.create_relationship("t_b", "blocks", "t_a", json!({}))
        .await
        .map_err(|e| anyhow::anyhow!("b blocks a — a cycle must be accepted: {e}"))?;

    // Both directions read back on both nodes: each task blocks the other and
    // is blocked by the other.
    for id in ["t_a", "t_b"] {
        let blocks = rel_ops::get_related_nodes(&svc, get(id, "blocks", "out")).await?;
        assert_eq!(blocks.count, 1, "{id} blocks the other");
        let blocked_by = rel_ops::get_related_nodes(&svc, get(id, "blocked_by", "out")).await?;
        assert_eq!(blocked_by.count, 1, "{id} is blocked by the other");
        assert_ne!(
            blocks.related_nodes[0]["id"],
            json!(id),
            "the edge should point at the other task"
        );
    }

    Ok(())
}
