//! Derived attributes read by rules and queries, and a write that names the
//! version it read (ADR-094 §5 and §6).
//!
//! A checkbox's `checked` is computed from its content and never stored. These
//! tests read it where a workflow does: in a Play condition over a node's
//! children, and in a query filter over them.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeUpdate};
use nodespace_core::ops::node_ops::{self, UpdateNodeInput};
use nodespace_core::ops::query_ops::{execute_query, ExecuteQueryInput};
use nodespace_core::ops::OpsError;
use nodespace_core::playbook::PlaybookEngine;
use nodespace_core::services::{NodeService, NodeServiceError};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

async fn create(service: &NodeService, node_type: &str, content: &str) -> Result<String> {
    let properties = if node_type == "task" {
        json!({ "status": "open" })
    } else {
        json!({})
    };
    let node = Node::new(node_type.to_string(), content.to_string(), properties);
    Ok(service.create_node(node).await?)
}

async fn create_child(
    service: &NodeService,
    parent: &str,
    node_type: &str,
    content: &str,
) -> Result<String> {
    let id = create(service, node_type, content).await?;
    service
        .create_relationship(parent, "has_child", &id, json!({}))
        .await?;
    Ok(id)
}

async fn set_content(service: &NodeService, id: &str, content: &str) -> Result<()> {
    let current = service.get_node(id).await?.expect("node should exist");
    service
        .update_node(
            id,
            current.version,
            NodeUpdate::default().with_content(content.to_string()),
        )
        .await?;
    Ok(())
}

async fn set_status(
    service: &NodeService,
    id: &str,
    status: &str,
) -> Result<Node, NodeServiceError> {
    let current = service.get_node(id).await?.expect("node should exist");
    service
        .update_node(
            id,
            current.version,
            NodeUpdate::default().with_properties(json!({ "status": status })),
        )
        .await
}

/// The rule the shipped workflow writes: a task cannot change status while it
/// has an unchecked checkbox child.
fn unchecked_child_rule() -> serde_json::Value {
    json!([{
        "name": "no-unchecked-children",
        "class": "invariant",
        "description": "A task with an unchecked checkbox child cannot change status",
        "trigger": {
            "type": "graph_event",
            "on": "property_changed",
            "select": { "target_type": "task" },
            "property_key": "task.status"
        },
        "conditions": [{
            "expr": "node.has_child.exists(c, c.checked == false)",
            "description": "Some checkbox child is not checked"
        }],
        "actions": [{
            "description": "Refuse the change",
            "action_type": "reject",
            "params": { "message": "an item is not checked" }
        }]
    }])
}

/// Activate the rule on a fresh engine.
fn activate_unchecked_child_rule(service: &Arc<NodeService>) -> PlaybookEngine {
    let engine = PlaybookEngine::new(Arc::clone(service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play = Node::new(
        "play".to_string(),
        "checklist gate".to_string(),
        json!({ "play": { "rules": unchecked_child_rule() } }),
    );
    engine
        .lifecycle()
        .write()
        .unwrap()
        .activate_play(&play)
        .expect("the play must parse and activate");
    engine
}

fn is_rejected(result: &Result<Node, NodeServiceError>) -> bool {
    matches!(result, Err(NodeServiceError::PlayRuleRejected { message, .. }) if message == "an item is not checked")
}

/// `node.has_child.exists(c, c.checked == false)` is true for a node with an
/// unchecked checkbox child and false otherwise, whatever else sits beside
/// the checkboxes.
#[tokio::test]
async fn a_condition_reads_checked_on_checkbox_children() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let _engine = activate_unchecked_child_rule(&service);

    // An unchecked checkbox among other children: the condition holds.
    let gated = create(&service, "task", "gated").await?;
    create_child(&service, &gated, "text", "a note").await?;
    create_child(&service, &gated, "checkbox", "- [x] first").await?;
    create_child(&service, &gated, "checkbox", "- [ ] second").await?;
    create_child(&service, &gated, "text", "another note").await?;
    assert!(
        is_rejected(&set_status(&service, &gated, "done").await),
        "an unchecked checkbox child must hold the condition"
    );

    // Every checkbox checked: it does not.
    let complete = create(&service, "task", "complete").await?;
    create_child(&service, &complete, "text", "a note").await?;
    create_child(&service, &complete, "checkbox", "- [x] first").await?;
    create_child(&service, &complete, "checkbox", "- [X] second").await?;
    set_status(&service, &complete, "done").await?;

    // No checkbox at all, and no children at all: it does not.
    let notes_only = create(&service, "task", "notes only").await?;
    create_child(&service, &notes_only, "text", "- [ ] text, not a checkbox").await?;
    set_status(&service, &notes_only, "done").await?;
    let bare = create(&service, "task", "bare").await?;
    set_status(&service, &bare, "done").await?;

    Ok(())
}

/// Nothing is stored: editing the checkbox's prefix changes what the
/// condition sees on the next evaluation.
#[tokio::test]
async fn the_condition_follows_the_checkbox_content() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let _engine = activate_unchecked_child_rule(&service);

    let task = create(&service, "task", "one item").await?;
    let item = create_child(&service, &task, "checkbox", "- [ ] the item").await?;
    assert!(is_rejected(
        &set_status(&service, &task, "in_progress").await
    ));

    set_content(&service, &item, "- [x] the item").await?;
    set_status(&service, &task, "in_progress").await?;

    set_content(&service, &item, "- [ ] the item").await?;
    assert!(is_rejected(&set_status(&service, &task, "done").await));

    let stored = service.get_node(&item).await?.expect("the checkbox");
    assert_eq!(
        stored
            .properties
            .get("checkbox")
            .and_then(|b| b.get("checked")),
        None,
        "checked is derived, never stored: {}",
        stored.properties
    );
    Ok(())
}

/// The play is accepted when saved through the service, which validates it.
#[tokio::test]
async fn a_play_reading_checked_through_has_child_saves() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());

    let play = Node::new(
        "play".to_string(),
        "checklist gate".to_string(),
        json!({ "rules": unchecked_child_rule() }),
    );
    service.create_node(play).await?;

    // The same condition on the trigger node itself is refused: a task
    // derives no `checked`.
    let mut rules = unchecked_child_rule();
    rules[0]["conditions"][0]["expr"] = json!("node.checked == false");
    let refused = Node::new(
        "play".to_string(),
        "broken gate".to_string(),
        json!({ "rules": rules }),
    );
    let err = service
        .create_node(refused)
        .await
        .expect_err("a task has no derived `checked`");
    assert!(
        err.to_string()
            .contains("derived from the content of a 'checkbox' node"),
        "{err}"
    );
    Ok(())
}

fn ids(output: &nodespace_core::ops::query_ops::ExecuteQueryOutput) -> Vec<String> {
    let mut ids: Vec<String> = output
        .nodes
        .iter()
        .filter_map(|n| n.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    ids.sort();
    ids
}

async fn run(service: &Arc<NodeService>, query: serde_json::Value) -> Result<Vec<String>> {
    let input: ExecuteQueryInput = serde_json::from_value(query)?;
    let output = execute_query(service, input)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(ids(&output))
}

fn tasks_with_an_unchecked_child() -> serde_json::Value {
    json!({
        "target_type": "task",
        "filters": [{
            "type": "related",
            "operator": "equals",
            "path": ["has_child"],
            "filter": {
                "type": "property",
                "operator": "equals",
                "property": "checked",
                "value": false
            }
        }]
    })
}

/// "Nodes with a checkbox child that is not checked" can be written, and
/// follows the content.
#[tokio::test(flavor = "multi_thread")]
async fn a_query_filter_names_checked_on_children() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let open = create(&service, "task", "has an open item").await?;
    create_child(&service, &open, "checkbox", "- [x] first").await?;
    let item = create_child(&service, &open, "checkbox", "- [ ] second").await?;

    let complete = create(&service, "task", "all checked").await?;
    create_child(&service, &complete, "checkbox", "- [x] only").await?;

    // A text child is not a checkbox, whatever its content, and a task child
    // derives nothing: neither is "a checkbox child that is not checked".
    let notes = create(&service, "task", "notes only").await?;
    create_child(&service, &notes, "text", "- [ ] text").await?;
    create_child(&service, &notes, "task", "a subtask").await?;
    create(&service, "task", "no children").await?;

    assert_eq!(
        run(&service, tasks_with_an_unchecked_child()).await?,
        vec![open.clone()]
    );

    set_content(&service, &item, "- [X] second").await?;
    assert!(run(&service, tasks_with_an_unchecked_child())
        .await?
        .is_empty());

    set_content(&service, &item, "- [ ] second").await?;
    assert_eq!(
        run(&service, tasks_with_an_unchecked_child()).await?,
        vec![open]
    );
    Ok(())
}

/// The attribute is named directly on the type that declares it, and under a
/// wildcard it applies to the rows that have it.
#[tokio::test(flavor = "multi_thread")]
async fn a_query_filter_names_checked_on_the_checkbox_itself() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let ticked = create(&service, "checkbox", "- [x] ticked").await?;
    let open = create(&service, "checkbox", "- [ ] open").await?;
    create(&service, "text", "- [x] text that looks ticked").await?;

    let checked = |target: &str, value: bool| {
        json!({
            "target_type": target,
            "filters": [{
                "type": "property",
                "operator": "equals",
                "property": "checked",
                "value": value
            }]
        })
    };
    assert_eq!(
        run(&service, checked("checkbox", true)).await?,
        vec![ticked.clone()]
    );
    assert_eq!(
        run(&service, checked("checkbox", false)).await?,
        vec![open.clone()]
    );
    assert_eq!(run(&service, checked("*", true)).await?, vec![ticked]);
    assert_eq!(run(&service, checked("*", false)).await?, vec![open]);
    // A type that derives no `checked` has no node that matches either value.
    assert!(run(&service, checked("text", true)).await?.is_empty());
    assert!(run(&service, checked("text", false)).await?.is_empty());
    Ok(())
}

/// A derived attribute cannot be written: a write naming one is refused, and
/// the node is left as it was.
#[tokio::test]
async fn a_write_naming_a_derived_attribute_is_refused() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let item = create(&service, "checkbox", "- [ ] the item").await?;

    let err = node_ops::update_node(
        &service,
        UpdateNodeInput {
            node_id: item.clone(),
            version: None,
            node_type: None,
            content: None,
            properties: Some(json!({ "checked": true })),
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await
    .expect_err("checked cannot be written");
    let message = err.to_string();
    assert!(
        message.contains("'checked' is derived from the content of a 'checkbox' node")
            && message.contains("cannot be written"),
        "{message}"
    );

    let created = service
        .create_node(Node::new(
            "checkbox".to_string(),
            "- [ ] another".to_string(),
            json!({ "checked": true }),
        ))
        .await;
    assert!(created.is_err(), "nor can it be set on create");

    let stored = service.get_node(&item).await?.expect("the checkbox");
    assert_eq!(stored.content, "- [ ] the item");
    assert_eq!(stored.version, 1, "a refused write changes nothing");
    Ok(())
}

fn status_update(node_id: &str, version: Option<i64>, status: &str) -> UpdateNodeInput {
    UpdateNodeInput {
        node_id: node_id.to_string(),
        version,
        node_type: None,
        content: None,
        properties: Some(json!({ "status": status })),
        add_to_collections: Vec::new(),
        add_to_collection_ids: Vec::new(),
        remove_from_collection_ids: Vec::new(),
        lifecycle_status: None,
    }
}

/// Two sessions read the same task and both try to start it with the version
/// they read: one write lands and the other is refused, naming the version it
/// gave and the version the task is at now.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_two_concurrent_writes_naming_one_version_only_the_first_lands() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    for round in 0..20 {
        let task = create(&service, "task", &format!("claim {round}")).await?;
        let read = service.get_node(&task).await?.expect("the task").version;

        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let writers: Vec<_> = (0..2)
            .map(|_| {
                let service = Arc::clone(&service);
                let barrier = Arc::clone(&barrier);
                let task = task.clone();
                tokio::spawn(async move {
                    barrier.wait().await;
                    node_ops::update_node(&service, status_update(&task, Some(read), "in_progress"))
                        .await
                })
            })
            .collect();

        let mut landed = 0;
        let mut refused = 0;
        for writer in writers {
            match writer.await? {
                Ok(output) => {
                    landed += 1;
                    assert_eq!(output.version, read + 1);
                }
                Err(OpsError::VersionConflict {
                    node_id,
                    expected,
                    actual,
                    ..
                }) => {
                    refused += 1;
                    assert_eq!(node_id, task);
                    assert_eq!(expected, read, "the version the writer gave");
                    assert_eq!(actual, read + 1, "the version the task is at now");
                }
                Err(other) => panic!("round {round}: unexpected error {other}"),
            }
        }
        assert_eq!((landed, refused), (1, 1), "round {round}");
        let stored = service.get_node(&task).await?.expect("the task");
        assert_eq!(stored.version, read + 1, "exactly one write was applied");
    }
    Ok(())
}

/// A write that names no version applies to whatever is current, as before.
#[tokio::test]
async fn a_write_naming_no_version_applies_to_the_current_node() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let task = create(&service, "task", "unversioned").await?;

    node_ops::update_node(&service, status_update(&task, None, "in_progress"))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let output = node_ops::update_node(&service, status_update(&task, None, "done"))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(output.version, 3);

    // A stale version is refused and leaves the node alone.
    let err = node_ops::update_node(&service, status_update(&task, Some(1), "open"))
        .await
        .expect_err("version 1 is stale");
    assert!(
        matches!(
            err,
            OpsError::VersionConflict {
                expected: 1,
                actual: 3,
                ..
            }
        ),
        "{err}"
    );
    Ok(())
}
