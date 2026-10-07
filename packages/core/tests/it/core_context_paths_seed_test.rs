//! The context paths a core schema ships with are a seeded aspect (ADR-072,
//! ADR-094 §2 and §8): they follow what ships until the user changes them,
//! and a shipped change to paths the user changed is put to them, never
//! applied on its own.
//!
//! "A later build ships other paths" is `reconcile_core_context_paths` called
//! with a changed list: what the next database open does under that build.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::core_schemas::get_core_schemas;
use nodespace_core::models::{Node, NodeUpdate, SchemaNode, SeedAspect};
use nodespace_core::ops::node_context_ops::{read_node_context, NodeContextInput};
use nodespace_core::ops::path_ops::resolve_path;
use nodespace_core::schema::handle_update_schema;
use nodespace_core::services::node_service::context_paths_version;
use nodespace_core::services::NodeService;
use nodespace_types::RelationshipPath;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;

const TASK_PATHS: [&str; 5] = ["spec", "plan", "decisions", "spec.decisions", "project"];

async fn open(db_path: &Path) -> Result<Arc<NodeService>> {
    let mut store = Arc::new(SqliteStore::new(db_path.to_path_buf()).await?);
    Ok(Arc::new(NodeService::new(&mut store).await?))
}

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let service = open(&temp_dir.path().join("test.db")).await?;
    Ok((service, temp_dir))
}

fn paths(dotted: &[&str]) -> Vec<RelationshipPath> {
    dotted.iter().map(|path| path.parse().unwrap()).collect()
}

/// The core schemas as a build that ships `task_paths` on `task` has them.
fn shipping(task_paths: &[&str]) -> Vec<SchemaNode> {
    let mut schemas = get_core_schemas();
    let task = schemas
        .iter_mut()
        .find(|schema| schema.envelope.id == "task")
        .unwrap();
    task.context_paths = paths(task_paths);
    schemas
}

fn task_of(schemas: &[SchemaNode]) -> &SchemaNode {
    schemas
        .iter()
        .find(|schema| schema.envelope.id == "task")
        .unwrap()
}

async fn update_schema(service: &Arc<NodeService>, params: Value) -> Result<()> {
    handle_update_schema(service, params)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(())
}

/// The context paths `task` holds in this database, dotted.
async fn stored_paths(service: &NodeService) -> Result<Vec<String>> {
    Ok(service
        .get_schema_node("task")
        .await?
        .expect("task is seeded")
        .context_paths
        .iter()
        .map(ToString::to_string)
        .collect())
}

/// The `_seed` bookkeeping on the `task` schema node.
async fn seed_block(service: &NodeService) -> Result<Value> {
    let node = service.get_node("task").await?.expect("task is seeded");
    Ok(node.properties.get("_seed").cloned().unwrap_or(Value::Null))
}

async fn pending(service: &NodeService) -> Result<Option<String>> {
    Ok(service
        .get_pending_seed_update("task", SeedAspect::ContextPaths)
        .await?
        .map(|update| update.shipped_version))
}

async fn create(service: &NodeService, node_type: &str, content: &str) -> Result<String> {
    Ok(service
        .create_node(Node::new(
            node_type.to_string(),
            content.to_string(),
            json!({}),
        ))
        .await?)
}

/// Every context path a core schema ships with resolves from its type
/// through the seeded schemas, and `task` ships the five that reach what
/// working on a task needs.
#[tokio::test]
async fn every_seeded_context_path_resolves() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let mut seeded = 0;
    for schema in get_core_schemas() {
        let id = &schema.envelope.id;
        for path in &schema.context_paths {
            assert!(!path.is_empty(), "{id} ships an empty context path");
            resolve_path(&service, Some(id), path)
                .await
                .unwrap_or_else(|e| panic!("{id}'s context path '{path}' does not resolve: {e}"));
            seeded += 1;
        }
        // What the database holds is what ships.
        let stored = service.get_schema_node(id).await?.expect("seeded");
        assert_eq!(stored.context_paths, schema.context_paths, "{id}");
    }
    assert!(seeded >= TASK_PATHS.len());

    assert_eq!(
        task_of(&get_core_schemas()).context_paths,
        paths(&TASK_PATHS)
    );
    assert_eq!(stored_paths(&service).await?, TASK_PATHS);
    Ok(())
}

/// A context read of a task, given no paths, reaches its spec, its plan, the
/// decisions linked from it and from its spec, and its project.
#[tokio::test]
async fn a_task_read_reaches_its_spec_plan_decisions_and_project() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let task = create(&service, "task", "Write the importer").await?;
    let spec = create(&service, "spec", "Importing").await?;
    let plan = create(&service, "plan", "Importer plan").await?;
    let project = create(&service, "project", "Apollo").await?;
    let task_decision = create(&service, "decision", "Stream the file").await?;
    let spec_decision = create(&service, "decision", "CSV only").await?;
    for (from, name, to) in [
        (&spec, "tasks", &task),
        (&plan, "tasks", &task),
        (&project, "tasks", &task),
        (&task, "decisions", &task_decision),
        (&spec, "decisions", &spec_decision),
    ] {
        service
            .create_relationship(from, name, to, json!({}))
            .await?;
    }

    let context = read_node_context(
        &service,
        NodeContextInput {
            node_id: task.clone(),
            paths: Vec::new(),
        },
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    let reached: Vec<(String, Vec<String>)> = context
        .paths
        .iter()
        .map(|reached| {
            (
                reached.path.to_string(),
                reached.nodes.iter().map(|n| n.node.id.clone()).collect(),
            )
        })
        .collect();
    assert_eq!(
        reached,
        [
            ("spec".to_string(), vec![spec]),
            ("plan".to_string(), vec![plan]),
            ("decisions".to_string(), vec![task_decision]),
            ("spec.decisions".to_string(), vec![spec_decision]),
            ("project".to_string(), vec![project]),
        ]
    );
    Ok(())
}

/// A new database holds the shipped paths as current: stamped with their
/// fingerprint, not marked as the user's, nothing pending. Opening it again
/// changes none of that.
#[tokio::test]
async fn a_fresh_database_holds_the_shipped_paths_as_current() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    for _ in 0..2 {
        let service = open(&db_path).await?;
        assert_eq!(stored_paths(&service).await?, TASK_PATHS);
        assert_eq!(
            seed_block(&service).await?,
            json!({ "context_paths_version": context_paths_version(&paths(&TASK_PATHS)) })
        );
        assert!(service.list_pending_seed_updates().await?.is_empty());
    }

    // Every core schema is stamped, one that ships no paths included.
    let service = open(&db_path).await?;
    for schema in get_core_schemas() {
        let node = service.get_node(&schema.envelope.id).await?.unwrap();
        assert_eq!(
            node.properties["_seed"]["context_paths_version"],
            json!(context_paths_version(&schema.context_paths)),
            "{}",
            schema.envelope.id
        );
    }
    Ok(())
}

/// A user's change to `task`'s paths is still there after the database is
/// opened again, and marks the paths as theirs. A schema change that touches
/// no path does not mark them, and neither does an edit to the schema's
/// description.
#[tokio::test]
async fn a_users_change_survives_a_restart_and_marks_the_paths_theirs() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    {
        let service = open(&db_path).await?;

        // Not a path change: a field, and a description.
        update_schema(
            &service,
            json!({
                "schema_id": "task",
                "add_fields": [{ "name": "custom:estimate", "type": "number" }],
                "description": "A unit of work."
            }),
        )
        .await?;
        let line = service.get_children("task").await?.remove(0);
        service
            .update_node(
                &line.id,
                line.version,
                NodeUpdate::new().with_content("A unit of work, done by one person.".to_string()),
            )
            .await?;
        assert_eq!(
            seed_block(&service).await?,
            json!({ "context_paths_version": context_paths_version(&paths(&TASK_PATHS)) }),
            "only a path change marks anything on a schema"
        );

        update_schema(
            &service,
            json!({ "schema_id": "task", "remove_context_paths": ["plan"] }),
        )
        .await?;
        update_schema(
            &service,
            json!({ "schema_id": "task", "add_context_paths": ["blocked_by"] }),
        )
        .await?;
        assert_eq!(seed_block(&service).await?["context_paths_modified"], true);
    }

    let theirs = [
        "spec",
        "decisions",
        "spec.decisions",
        "project",
        "blocked_by",
    ];
    for _ in 0..2 {
        let service = open(&db_path).await?;
        assert_eq!(stored_paths(&service).await?, theirs);
        let seed = seed_block(&service).await?;
        assert_eq!(seed["context_paths_modified"], true);
        // Still the fingerprint of the paths that shipped when they edited.
        assert_eq!(
            seed["context_paths_version"],
            json!(context_paths_version(&paths(&TASK_PATHS)))
        );
        assert!(seed.get("config_modified").is_none(), "{seed}");
        assert!(seed.get("guidance_modified").is_none(), "{seed}");
        // What ships has not changed, so there is nothing to decide.
        assert_eq!(pending(&service).await?, None);
    }
    Ok(())
}

/// Paths nobody changed follow the shipped ones, without asking: a changed
/// list replaces them, and an emptied one removes them.
#[tokio::test]
async fn unedited_paths_follow_the_shipped_ones() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    let later = shipping(&["project", "blocked_by"]);
    service.reconcile_core_context_paths(&later).await?;
    assert_eq!(stored_paths(&service).await?, ["project", "blocked_by"]);
    assert_eq!(
        seed_block(&service).await?,
        json!({ "context_paths_version": context_paths_version(&task_of(&later).context_paths) })
    );
    assert_eq!(pending(&service).await?, None);

    service.reconcile_core_context_paths(&shipping(&[])).await?;
    assert!(stored_paths(&service).await?.is_empty());
    let node = service.get_node("task").await?.unwrap();
    assert!(node.properties.get("contextPaths").is_none());
    assert_eq!(
        node.properties["_seed"]["context_paths_version"],
        json!(context_paths_version(&[]))
    );

    // And back, as this build ships them.
    service
        .reconcile_core_context_paths(&get_core_schemas())
        .await?;
    assert_eq!(stored_paths(&service).await?, TASK_PATHS);
    Ok(())
}

/// A shipped change to paths the user changed is recorded as pending with
/// the shipped paths' fingerprint, and their paths are left as they are.
/// Keeping theirs settles it until what ships changes again.
#[tokio::test]
async fn a_shipped_change_to_edited_paths_is_pending_and_can_be_kept() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    update_schema(
        &service,
        json!({ "schema_id": "task", "remove_context_paths": ["plan"] }),
    )
    .await?;
    let theirs = ["spec", "decisions", "spec.decisions", "project"];

    let second = shipping(&["spec", "project", "blocked_by"]);
    let second_version = context_paths_version(&task_of(&second).context_paths);
    service.reconcile_core_context_paths(&second).await?;
    assert_eq!(stored_paths(&service).await?, theirs);
    assert_eq!(pending(&service).await?, Some(second_version.clone()));

    let listed = service.list_pending_seed_updates().await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].node_id, "task");
    assert_eq!(listed[0].node_type, "schema");
    assert_eq!(listed[0].aspect, SeedAspect::ContextPaths);

    // The shipped paths beside theirs.
    let comparison = service
        .compare_pending_context_paths_update(task_of(&second))
        .await?
        .expect("an update is pending");
    assert_eq!(comparison.shipped, "spec\nproject\nblocked_by");
    assert_eq!(comparison.yours, theirs.join("\n"));
    assert_eq!(comparison.update.shipped_version, second_version);

    assert!(
        service
            .keep_seed_update("task", SeedAspect::ContextPaths)
            .await?
    );
    assert_eq!(pending(&service).await?, None);
    assert_eq!(stored_paths(&service).await?, theirs);
    let seed = seed_block(&service).await?;
    assert_eq!(seed["context_paths_version"], json!(second_version));
    assert_eq!(seed["context_paths_modified"], true);

    // The same build opening the database again asks nothing.
    service.reconcile_core_context_paths(&second).await?;
    assert_eq!(pending(&service).await?, None);
    assert_eq!(stored_paths(&service).await?, theirs);

    // A build that ships other paths again does.
    let third = shipping(&["project"]);
    service.reconcile_core_context_paths(&third).await?;
    assert_eq!(
        pending(&service).await?,
        Some(context_paths_version(&task_of(&third).context_paths))
    );
    assert_eq!(stored_paths(&service).await?, theirs);

    // Nothing is left to keep once it is kept.
    assert!(
        service
            .keep_seed_update("task", SeedAspect::ContextPaths)
            .await?
    );
    assert!(
        !service
            .keep_seed_update("task", SeedAspect::ContextPaths)
            .await?
    );
    Ok(())
}

/// Taking the shipped paths replaces the user's, clears the mark and the
/// pending record, and from then on the paths follow what ships again.
#[tokio::test]
async fn taking_the_shipped_paths_replaces_theirs() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    // Nothing pending: nothing to take, nothing changed.
    let second = shipping(&["spec", "project", "blocked_by"]);
    assert!(!service.take_context_paths_update(task_of(&second)).await?);
    assert_eq!(stored_paths(&service).await?, TASK_PATHS);
    assert!(service
        .compare_pending_context_paths_update(task_of(&second))
        .await?
        .is_none());

    update_schema(
        &service,
        json!({ "schema_id": "task", "add_context_paths": ["blocked_by"] }),
    )
    .await?;
    service.reconcile_core_context_paths(&second).await?;
    assert!(pending(&service).await?.is_some());
    assert_eq!(stored_paths(&service).await?.len(), 6);

    assert!(service.take_context_paths_update(task_of(&second)).await?);
    assert_eq!(
        stored_paths(&service).await?,
        ["spec", "project", "blocked_by"]
    );
    assert_eq!(pending(&service).await?, None);
    assert_eq!(
        seed_block(&service).await?,
        json!({
            "context_paths_version": context_paths_version(&task_of(&second).context_paths),
            "context_paths_modified": false,
        })
    );

    // No longer theirs: the next shipped change is applied, not asked about.
    service
        .reconcile_core_context_paths(&shipping(&["project"]))
        .await?;
    assert_eq!(stored_paths(&service).await?, ["project"]);
    assert_eq!(pending(&service).await?, None);
    Ok(())
}

/// A pending update to edited paths survives a restart under the same
/// build's paths only while that build still ships something else: opening
/// with the paths the user's were kept against clears it.
#[tokio::test]
async fn a_pending_update_is_settled_when_the_shipped_paths_match_again() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    update_schema(
        &service,
        json!({ "schema_id": "task", "remove_context_paths": ["plan"] }),
    )
    .await?;
    service
        .reconcile_core_context_paths(&shipping(&["project"]))
        .await?;
    assert!(pending(&service).await?.is_some());

    // The build whose paths they edited: its fingerprint is the stored one.
    service
        .reconcile_core_context_paths(&get_core_schemas())
        .await?;
    assert_eq!(pending(&service).await?, None);
    assert_eq!(
        stored_paths(&service).await?,
        ["spec", "decisions", "spec.decisions", "project"]
    );
    Ok(())
}

/// A reset puts the paths back to the shipped list whether or not a shipped
/// change is pending, clears the mark so the paths follow what ships again,
/// and clears a pending record.
#[tokio::test]
async fn a_reset_puts_the_paths_back_and_clears_the_mark() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let shipped = get_core_schemas();
    update_schema(
        &service,
        json!({ "schema_id": "task", "remove_context_paths": ["plan"] }),
    )
    .await?;
    assert_eq!(seed_block(&service).await?["context_paths_modified"], true);

    // Nothing pending, and the reset still restores the list.
    assert!(service.reset_context_paths(task_of(&shipped)).await?);
    assert_eq!(stored_paths(&service).await?, TASK_PATHS);
    let seed = seed_block(&service).await?;
    assert_eq!(seed["context_paths_modified"], false);
    assert_eq!(
        seed["context_paths_version"],
        json!(context_paths_version(&task_of(&shipped).context_paths))
    );

    // Edited again, with a shipped change pending: the reset clears both.
    update_schema(
        &service,
        json!({ "schema_id": "task", "remove_context_paths": ["project"] }),
    )
    .await?;
    service
        .reconcile_core_context_paths(&shipping(&["spec"]))
        .await?;
    assert!(pending(&service).await?.is_some());
    assert!(service.reset_context_paths(task_of(&shipped)).await?);
    assert_eq!(pending(&service).await?, None);
    assert_eq!(stored_paths(&service).await?, TASK_PATHS);
    assert_eq!(seed_block(&service).await?["context_paths_modified"], false);

    // Following what ships again: the next shipped change is applied.
    service
        .reconcile_core_context_paths(&shipping(&["spec"]))
        .await?;
    assert_eq!(stored_paths(&service).await?, ["spec"]);
    assert_eq!(pending(&service).await?, None);

    // A type that is not in this database has nothing to reset.
    let mut unknown = task_of(&shipped).clone();
    unknown.envelope.id = "no-such-type".to_string();
    assert!(!service.reset_context_paths(&unknown).await?);
    Ok(())
}
