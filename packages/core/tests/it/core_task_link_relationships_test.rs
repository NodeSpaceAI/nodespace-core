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
//! - each pair resolves the forward and reverse names to DIFFERENT edge sets,
//!   and the reverse name agrees with the forward name read with
//!   `direction: "in"` (the self-referential case, where the declaring and
//!   target type are the same, so the schema appears in its own inbound set)
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
        // A templated type (`person`) is named by its fields, not content.
        if node_type == "person" {
            String::new()
        } else {
            format!("{id} content")
        },
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

/// Each self-referential pair traverses from both ends, and the forward and
/// reverse names resolve to genuinely DIFFERENT edge sets.
///
/// The middle node of a three-node chain is what makes this meaningful. With a
/// single ordered pair, `blocks` and `blocked_by` would both return "the other
/// task" even if the reverse name silently resolved to the forward edge set —
/// the one failure mode that matters here, since both names live on `task` and
/// the usual `source_type` narrowing degenerates to a no-op when the declaring
/// and target type are the same. `mid` sits between two distinct tasks, so
/// resolving the wrong direction surfaces the wrong node and fails loudly.
#[tokio::test]
async fn task_link_pairs_resolve_direction_asymmetrically() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    make_node(&svc, "87f5bbc3-0d4e-5cbf-9f65-6776ee32e35c", "task").await?;
    make_node(&svc, "fa93961d-299d-551a-b363-5b1b783ab365", "task").await?;
    make_node(&svc, "0afefbf8-6574-5d5d-ba9f-025143d26219", "task").await?;

    for (name, reverse_name) in [
        ("blocks", "blocked_by"),
        ("relates_to", "related_from"),
        ("duplicates", "duplicated_by"),
    ] {
        // upstream → mid → downstream, under this relationship name.
        svc.create_relationship(
            "87f5bbc3-0d4e-5cbf-9f65-6776ee32e35c",
            name,
            "fa93961d-299d-551a-b363-5b1b783ab365",
            json!({}),
        )
        .await
        .map_err(|e| anyhow::anyhow!("create upstream {name} mid: {e}"))?;
        svc.create_relationship(
            "fa93961d-299d-551a-b363-5b1b783ab365",
            name,
            "0afefbf8-6574-5d5d-ba9f-025143d26219",
            json!({}),
        )
        .await
        .map_err(|e| anyhow::anyhow!("create mid {name} downstream: {e}"))?;

        // Forward from the middle reaches only what mid points at.
        let forward = rel_ops::get_related_nodes(
            &svc,
            get("fa93961d-299d-551a-b363-5b1b783ab365", name, "out"),
        )
        .await?;
        assert_eq!(forward.count, 1, "{name} should traverse forward from mid");
        assert_eq!(
            forward.related_nodes[0]["id"], "0afefbf8-6574-5d5d-ba9f-025143d26219",
            "{name} from mid must reach downstream, not upstream"
        );

        // The reverse spelling from the same node reaches the OTHER neighbour.
        // This only resolves if self-referential declarations are handled, and
        // only passes if the two names address different edge sets.
        let reverse = rel_ops::get_related_nodes(
            &svc,
            get("fa93961d-299d-551a-b363-5b1b783ab365", reverse_name, "out"),
        )
        .await?;
        assert_eq!(
            reverse.count, 1,
            "{reverse_name} must resolve rather than return a silent zero: {reverse:?}"
        );
        assert_eq!(
            reverse.related_nodes[0]["id"], "87f5bbc3-0d4e-5cbf-9f65-6776ee32e35c",
            "{reverse_name} from mid must reach upstream — if it returns downstream, \
             the reverse name collapsed onto the forward edge set"
        );

        // ...and agree with the same traversal spelled the long way.
        let inbound = rel_ops::get_related_nodes(
            &svc,
            get("fa93961d-299d-551a-b363-5b1b783ab365", name, "in"),
        )
        .await?;
        assert_eq!(
            inbound.related_nodes[0]["id"], reverse.related_nodes[0]["id"],
            "{reverse_name} and `{name} --direction in` must agree"
        );

        // The chain ends see exactly one side each.
        let upstream_reverse = rel_ops::get_related_nodes(
            &svc,
            get("87f5bbc3-0d4e-5cbf-9f65-6776ee32e35c", reverse_name, "out"),
        )
        .await?;
        assert_eq!(
            upstream_reverse.count, 0,
            "nothing points at upstream under {name}"
        );
        let downstream_forward = rel_ops::get_related_nodes(
            &svc,
            get("0afefbf8-6574-5d5d-ba9f-025143d26219", name, "out"),
        )
        .await?;
        assert_eq!(
            downstream_forward.count, 0,
            "downstream points at nothing under {name}"
        );
    }

    Ok(())
}

/// A task's creator is independent of its assignee: different relationship,
/// different person, neither shadowing the other.
#[tokio::test]
async fn creator_resolves_independently_of_assignee() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    make_node(&svc, "ce036e75-4532-5462-b7eb-ba53673bc4bc", "person").await?;
    make_node(&svc, "23b4430f-01b5-5ad2-aa2c-cfb9edae6e2f", "person").await?;
    make_node(&svc, "47b0416e-68db-58e9-805c-db17bfe8856d", "task").await?;

    svc.create_relationship(
        "ce036e75-4532-5462-b7eb-ba53673bc4bc",
        "reported_tasks",
        "47b0416e-68db-58e9-805c-db17bfe8856d",
        json!({}),
    )
    .await
    .map_err(|e| anyhow::anyhow!("reported_tasks: {e}"))?;
    svc.create_relationship(
        "23b4430f-01b5-5ad2-aa2c-cfb9edae6e2f",
        "tasks",
        "47b0416e-68db-58e9-805c-db17bfe8856d",
        json!({}),
    )
    .await
    .map_err(|e| anyhow::anyhow!("tasks: {e}"))?;

    let creator = rel_ops::get_related_nodes(
        &svc,
        get("47b0416e-68db-58e9-805c-db17bfe8856d", "creator", "out"),
    )
    .await?;
    assert_eq!(creator.count, 1, "creator must resolve: {creator:?}");
    assert_eq!(
        creator.related_nodes[0]["id"],
        "ce036e75-4532-5462-b7eb-ba53673bc4bc"
    );

    let assignee = rel_ops::get_related_nodes(
        &svc,
        get("47b0416e-68db-58e9-805c-db17bfe8856d", "assignee", "out"),
    )
    .await?;
    assert_eq!(assignee.count, 1);
    assert_eq!(
        assignee.related_nodes[0]["id"], "23b4430f-01b5-5ad2-aa2c-cfb9edae6e2f",
        "the new declaration must not shadow the existing assignee reverse name"
    );

    // And from the person's end, the reporter's queues stay distinct.
    let reported = rel_ops::get_related_nodes(
        &svc,
        get(
            "ce036e75-4532-5462-b7eb-ba53673bc4bc",
            "reported_tasks",
            "out",
        ),
    )
    .await?;
    assert_eq!(reported.count, 1);
    assert_eq!(
        reported.related_nodes[0]["id"],
        "47b0416e-68db-58e9-805c-db17bfe8856d"
    );
    let reporter_assigned = rel_ops::get_related_nodes(
        &svc,
        get("ce036e75-4532-5462-b7eb-ba53673bc4bc", "tasks", "out"),
    )
    .await?;
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
    make_node(&svc, "f6070d8d-5c7b-5257-bcf3-6e52ee19d27b", "task").await?;
    make_node(&svc, "86eac5ed-73ba-500f-bf77-83a5f12b66cb", "task").await?;

    svc.create_relationship(
        "f6070d8d-5c7b-5257-bcf3-6e52ee19d27b",
        "blocks",
        "86eac5ed-73ba-500f-bf77-83a5f12b66cb",
        json!({}),
    )
    .await
    .map_err(|e| anyhow::anyhow!("a blocks b: {e}"))?;
    svc.create_relationship(
        "86eac5ed-73ba-500f-bf77-83a5f12b66cb",
        "blocks",
        "f6070d8d-5c7b-5257-bcf3-6e52ee19d27b",
        json!({}),
    )
    .await
    .map_err(|e| anyhow::anyhow!("b blocks a — a cycle must be accepted: {e}"))?;

    // Both directions read back on both nodes: each task blocks the other and
    // is blocked by the other.
    for id in [
        "f6070d8d-5c7b-5257-bcf3-6e52ee19d27b",
        "86eac5ed-73ba-500f-bf77-83a5f12b66cb",
    ] {
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
