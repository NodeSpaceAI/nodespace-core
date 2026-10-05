//! A skill's `attached_to` relationship, the read that follows relationship
//! paths from a node, and the skills returned with a node and with a saved
//! query run (ADR-094 §2 and §3).
//!
//! The same flow is shown twice: on tasks and a project, and on a workflow
//! built from types created through `create_schema`. Nothing in the read
//! knows either set of types.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeUpdate, SkillFields, SKILL_ATTACHED_TO};
use nodespace_core::ops::node_context_ops::{
    attached_skills, read_node_context, NodeContext, NodeContextInput,
};
use nodespace_core::ops::query_ops::{run_saved_query_nodes, RunSavedQueryInput};
use nodespace_core::ops::OpsError;
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::NodeService;
use nodespace_types::RelationshipPath;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let mut store = Arc::new(SqliteStore::new(temp_dir.path().join("test.db")).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

async fn create(service: &NodeService, node_type: &str, content: &str, props: Value) -> String {
    service
        .create_node(Node::new(node_type.to_string(), content.to_string(), props))
        .await
        .unwrap_or_else(|e| panic!("creating the {node_type} '{content}' failed: {e}"))
}

async fn create_child(service: &NodeService, parent: &str, node_type: &str, content: &str) {
    let id = create(service, node_type, content, json!({})).await;
    service
        .create_relationship(parent, "has_child", &id, json!({}))
        .await
        .unwrap_or_else(|e| panic!("placing '{content}' under {parent} failed: {e}"));
}

/// A skill named `name` whose procedure is the one line `step`.
async fn create_skill(service: &NodeService, name: &str, step: &str) -> String {
    let node = SkillFields::new("When this applies.", &["get_node"], 2).into_node(name);
    let id = service.create_node(node).await.expect("the skill");
    create_child(service, &id, "text", step).await;
    id
}

async fn attach(service: &NodeService, skill: &str, node: &str) {
    service
        .create_relationship(skill, SKILL_ATTACHED_TO, node, json!({}))
        .await
        .unwrap_or_else(|e| panic!("attaching {skill} to {node} failed: {e}"));
}

async fn archive(service: &NodeService, id: &str) {
    let version = service.get_node(id).await.unwrap().unwrap().version;
    service
        .update_node(
            id,
            version,
            NodeUpdate::new().with_lifecycle_status("archived".to_string()),
        )
        .await
        .unwrap_or_else(|e| panic!("archiving {id} failed: {e}"));
}

async fn read(service: &NodeService, node_id: &str, paths: &[&str]) -> NodeContext {
    read_node_context(
        service,
        NodeContextInput {
            node_id: node_id.to_string(),
            paths: paths.iter().map(|path| path.parse().unwrap()).collect(),
        },
    )
    .await
    .unwrap_or_else(|e| panic!("reading {node_id} with {paths:?} failed: {e}"))
}

/// The names of the skills a read returned, with the nodes each is attached
/// to.
fn skill_names(context: &NodeContext) -> Vec<(&str, Vec<&str>)> {
    context
        .attached
        .skills
        .iter()
        .map(|attached| {
            (
                attached.skill.name.as_str(),
                attached.attached_to.iter().map(String::as_str).collect(),
            )
        })
        .collect()
}

/// The saved queries a read reached its `skill`-th skill through.
fn matched_queries(context: &NodeContext, skill: usize) -> Vec<&str> {
    context.attached.skills[skill]
        .matched_queries
        .iter()
        .map(|query| query.id.as_str())
        .collect()
}

fn reached(context: &NodeContext, path: usize) -> Vec<&str> {
    context.paths[path]
        .nodes
        .iter()
        .map(|reached| reached.node.id.as_str())
        .collect()
}

async fn run_query(service: &Arc<NodeService>, query: &str) -> (String, Vec<String>) {
    let run = run_saved_query_nodes(
        service,
        RunSavedQueryInput {
            query: query.to_string(),
            filters: Vec::new(),
            limit: None,
            max_rows: None,
        },
    )
    .await
    .unwrap_or_else(|e| panic!("running '{query}' failed: {e}"));
    (
        run.query_id,
        run.nodes.into_iter().map(|node| node.id).collect(),
    )
}

/// A skill attaches to a node of any type: a project, a saved query, a task
/// and a node of a user-defined type. The link reads from both ends.
#[tokio::test]
async fn a_skill_attaches_to_a_node_of_any_type() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    handle_create_schema(&service, json!({ "name": "Desk", "fields": [] }))
        .await
        .map_err(|e| anyhow::anyhow!("desk schema: {e}"))?;

    let skill = create_skill(&service, "Standards", "Name things plainly.").await;
    let targets = [
        create(&service, "project", "Apollo", json!({})).await,
        create(
            &service,
            "query",
            "Ready",
            json!({ "target_type": "task", "filters": [] }),
        )
        .await,
        create(&service, "task", "Write the spec", json!({})).await,
        create(&service, "desk", "Front desk", json!({})).await,
    ];
    for target in &targets {
        attach(&service, &skill, target).await;
        // Read from the target by the reverse name, as the CLI does.
        let from_target = nodespace_core::ops::rel_ops::get_related_nodes(
            &service,
            nodespace_core::ops::rel_ops::GetRelatedInput {
                node_id: target.clone(),
                relationship_name: "attached_skills".to_string(),
                direction: "out".to_string(),
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("attached_skills from {target}: {e}"))?;
        assert_eq!(
            from_target
                .related_nodes
                .iter()
                .filter_map(|node| node["id"].as_str())
                .collect::<Vec<_>>(),
            [skill.as_str()]
        );
    }
    let mut attached: Vec<String> = service
        .get_related_nodes(&skill, SKILL_ATTACHED_TO, "out")
        .await?
        .into_iter()
        .map(|node| node.id)
        .collect();
    attached.sort();
    let mut expected = targets.to_vec();
    expected.sort();
    assert_eq!(attached, expected);
    Ok(())
}

/// Proof on tasks: running the queue returns its procedure, and reading a
/// task with the path to its project returns the project's standards.
#[tokio::test]
async fn the_task_flow_returns_a_queues_procedure_and_a_projects_standards() -> Result<()> {
    let (service, _tmp) = test_service().await?;

    let project = create(&service, "project", "Apollo", json!({})).await;
    let task = create(&service, "task", "Write the spec", json!({})).await;
    service
        .create_relationship(&project, "tasks", &task, json!({}))
        .await?;
    create_child(&service, &task, "checkbox", "- [ ] Draft it").await;
    create_child(&service, &task, "text", "Some notes").await;
    create_child(&service, &project, "checkbox", "- [x] Kick-off held").await;
    let queue = create(
        &service,
        "query",
        "Open tasks",
        json!({ "target_type": "task", "filters": [{
            "type": "property", "operator": "equals", "property": "status", "value": "open"
        }] }),
    )
    .await;

    let standards = create_skill(&service, "Standards", "Name things plainly.").await;
    let procedure = create_skill(&service, "Implementing", "Tick each item as you go.").await;
    attach(&service, &standards, &project).await;
    attach(&service, &procedure, &queue).await;

    // The queue hands over its procedure beside its result.
    let (query_id, nodes) = run_query(&service, "Open tasks").await;
    assert_eq!(nodes, std::slice::from_ref(&task));
    let with_run = attached_skills(&service, std::slice::from_ref(&query_id)).await?;
    assert_eq!(with_run.skills.len(), 1);
    assert_eq!(with_run.skills[0].skill.name, "Implementing");
    assert!(with_run.skills[0]
        .skill
        .instructions
        .contains("Tick each item as you go."));
    assert_eq!(with_run.skills[0].attached_to, std::slice::from_ref(&queue));

    // The task matches the queue, so it carries the queue's procedure; read
    // with the path to its project, it carries the standards too.
    let context = read(&service, &task, &["project"]).await;
    assert_eq!(context.node.node.id, task);
    assert_eq!(reached(&context, 0), [project.as_str()]);
    assert_eq!(
        skill_names(&context),
        [
            ("Implementing", vec![]),
            ("Standards", vec![project.as_str()])
        ]
    );
    assert_eq!(matched_queries(&context, 0), [queue.as_str()]);
    assert!(matched_queries(&context, 1).is_empty());
    assert!(context.attached.skills[1]
        .skill
        .instructions
        .contains("Name things plainly."));
    // A node comes with its direct checkbox children and nothing else of its
    // subtree.
    let checkbox_content =
        |nodes: &[Node]| -> Vec<String> { nodes.iter().map(|node| node.content.clone()).collect() };
    assert_eq!(
        checkbox_content(&context.node.checkboxes),
        ["- [ ] Draft it"]
    );
    assert_eq!(
        checkbox_content(&context.paths[0].nodes[0].checkboxes),
        ["- [x] Kick-off held"]
    );

    // Read without the path, the project's standards are not reached.
    assert_eq!(
        skill_names(&read(&service, &task, &[]).await),
        [("Implementing", vec![])]
    );
    Ok(())
}

/// Proof on a user-defined workflow: the same operations over types created
/// through `create_schema`.
#[tokio::test]
async fn a_user_defined_workflow_gets_the_same_read_and_the_same_skills() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    handle_create_schema(
        &service,
        json!({ "name": "Request", "fields": [{ "name": "state", "type": "text" }] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("request schema: {e}"))?;
    handle_create_schema(
        &service,
        json!({
            "name": "Desk",
            "fields": [],
            "relationships": [{
                "name": "requests", "targetType": "request", "direction": "out",
                "cardinality": "many", "reverseName": "desk", "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("desk schema: {e}"))?;

    let desk = create(&service, "desk", "Front desk", json!({})).await;
    let request = create(&service, "request", "New badge", json!({ "state": "new" })).await;
    service
        .create_relationship(&desk, "requests", &request, json!({}))
        .await?;
    let queue = create(
        &service,
        "query",
        "New requests",
        json!({ "target_type": "request", "filters": [{
            "type": "property", "operator": "equals", "property": "state", "value": "new"
        }] }),
    )
    .await;

    let standards = create_skill(&service, "Desk rules", "Answer within a day.").await;
    let procedure = create_skill(&service, "Handling a request", "Confirm, then fulfil.").await;
    attach(&service, &standards, &desk).await;
    attach(&service, &procedure, &queue).await;

    let (query_id, nodes) = run_query(&service, "New requests").await;
    assert_eq!(nodes, std::slice::from_ref(&request));
    let with_run = attached_skills(&service, &[query_id]).await?;
    assert_eq!(with_run.skills.len(), 1);
    assert_eq!(with_run.skills[0].skill.name, "Handling a request");
    assert!(with_run.skills[0]
        .skill
        .instructions
        .contains("Confirm, then fulfil."));

    // The request matches the queue, so it carries the queue's procedure as
    // well as its desk's rules.
    let context = read(&service, &request, &["desk"]).await;
    assert_eq!(reached(&context, 0), [desk.as_str()]);
    assert_eq!(
        skill_names(&context),
        [
            ("Handling a request", vec![]),
            ("Desk rules", vec![desk.as_str()])
        ]
    );
    assert_eq!(matched_queries(&context, 0), [queue.as_str()]);
    assert!(context.attached.skills[1]
        .skill
        .instructions
        .contains("Answer within a day."));

    // Once the request is no longer new it has left the queue, and the next
    // read of it carries no procedure.
    let version = service.get_node(&request).await?.unwrap().version;
    service
        .update_node(
            &request,
            version,
            NodeUpdate::new().with_properties(json!({ "state": "handled" })),
        )
        .await?;
    assert_eq!(
        skill_names(&read(&service, &request, &["desk"]).await),
        [("Desk rules", vec![desk.as_str()])]
    );
    Ok(())
}

/// Paths take declared names, reverse names and `has_child`, are returned
/// grouped by path, and a hop after a relationship with no declared target
/// type is resolved from the nodes that relationship reached.
#[tokio::test]
async fn paths_are_followed_by_name_and_returned_by_path() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let project = create(&service, "project", "Apollo", json!({})).await;
    let first = create(&service, "task", "First", json!({})).await;
    let second = create(&service, "task", "Second", json!({})).await;
    for task in [&first, &second] {
        service
            .create_relationship(&project, "tasks", task, json!({}))
            .await?;
    }
    let note = create(&service, "text", "A note", json!({})).await;
    service
        .create_relationship(&first, "has_child", &note, json!({}))
        .await?;

    // A declared forward name, and `has_child`.
    let context = read(&service, &project, &["tasks", "tasks.has_child"]).await;
    assert_eq!(context.paths[0].path.to_string(), "tasks");
    assert_eq!(reached(&context, 0), [first.as_str(), second.as_str()]);
    assert_eq!(reached(&context, 1), [note.as_str()]);

    // A reverse name, then on from the node it reached: the sibling tasks.
    let context = read(&service, &first, &["project.tasks"]).await;
    assert_eq!(reached(&context, 0), [first.as_str(), second.as_str()]);

    // `child_of` reaches a node of no declared type, so `tasks` is resolved
    // from the project it turned out to be.
    let under = create(&service, "text", "Under the project", json!({})).await;
    service
        .create_relationship(&project, "has_child", &under, json!({}))
        .await?;
    let context = read(&service, &under, &["child_of.tasks", "child_of*"]).await;
    assert_eq!(reached(&context, 0), [first.as_str(), second.as_str()]);
    assert_eq!(reached(&context, 1), [project.as_str()]);

    // A declared path that reaches nothing is an empty group, not an error.
    let alone = create(&service, "task", "Alone", json!({})).await;
    let context = read(&service, &alone, &["project", "project.tasks"]).await;
    assert!(reached(&context, 0).is_empty());
    assert!(reached(&context, 1).is_empty());
    Ok(())
}

/// Where a hop is followed from nodes of several types, the ones whose type
/// does not declare the name lead nowhere; the name is refused only when none
/// of them declares it. After a hop that reached nothing and declares no
/// type, there is nothing to check a name against, and the path is empty.
#[tokio::test]
async fn a_name_is_checked_against_the_nodes_the_walk_stands_on() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let folder = create(&service, "text", "A folder", json!({})).await;
    let project = create(&service, "project", "Apollo", json!({})).await;
    let note = create(&service, "text", "A note", json!({})).await;
    let task = create(&service, "task", "Write the spec", json!({})).await;
    for child in [&project, &note] {
        service
            .create_relationship(&folder, "has_child", child, json!({}))
            .await?;
    }
    service
        .create_relationship(&project, "tasks", &task, json!({}))
        .await?;

    // A project and a note: only the project declares `tasks`.
    let context = read(&service, &folder, &["has_child.tasks"]).await;
    assert_eq!(reached(&context, 0), [task.as_str()]);

    // From the note, every child is a note: nothing declares `tasks`.
    service
        .create_relationship(
            &note,
            "has_child",
            &create(&service, "text", "Deeper", json!({})).await,
            json!({}),
        )
        .await?;
    let refused = read_node_context(
        &service,
        NodeContextInput {
            node_id: note.clone(),
            paths: vec!["has_child.tasks".parse().unwrap()],
        },
    )
    .await
    .expect_err("no node the walk stands on declares the name");
    assert!(
        refused.to_string().contains("path 'has_child.tasks'"),
        "{refused}"
    );

    // The task has no children, and `has_child` declares no type.
    let context = read(&service, &task, &["has_child.anything"]).await;
    assert!(reached(&context, 0).is_empty());
    Ok(())
}

/// A path that reaches more nodes than one read returns is cut to the first
/// of them and says so.
#[tokio::test]
async fn a_path_that_reaches_too_many_nodes_is_cut_and_says_so() -> Result<()> {
    use nodespace_core::ops::node_context_ops::MAX_NODES_PER_PATH;

    let (service, _tmp) = test_service().await?;
    let project = create(&service, "project", "Apollo", json!({})).await;
    for index in 0..=MAX_NODES_PER_PATH {
        let task = create(&service, "task", &format!("Task {index}"), json!({})).await;
        service
            .create_relationship(&project, "tasks", &task, json!({}))
            .await?;
    }

    let context = read(&service, &project, &["tasks", "has_child"]).await;
    assert_eq!(context.paths[0].nodes.len(), MAX_NODES_PER_PATH);
    assert!(context.paths[0].limit_reached);
    assert!(!context.paths[1].limit_reached);
    Ok(())
}

/// A name the node's type does not declare is refused, and the message names
/// the path and the name.
#[tokio::test]
async fn an_unknown_path_is_refused_with_a_message_naming_it() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let project = create(&service, "project", "Apollo", json!({})).await;
    let task = create(&service, "task", "Write the spec", json!({})).await;
    service
        .create_relationship(&project, "tasks", &task, json!({}))
        .await?;

    for (node, path, name) in [
        (&task, "owner", "owner"),
        (&task, "project.sponsor", "sponsor"),
        // Checked against the declared type even when nothing was reached.
        (&project, "tasks.project.sponsor", "sponsor"),
    ] {
        let error = read_node_context(
            &service,
            NodeContextInput {
                node_id: node.to_string(),
                paths: vec![path.parse::<RelationshipPath>().unwrap()],
            },
        )
        .await
        .expect_err("an undeclared name must be refused");
        let OpsError::InvalidParams(message) = &error else {
            panic!("expected InvalidParams, got {error:?}");
        };
        assert!(
            message.contains(&format!("path '{path}'"))
                && message.contains(&format!("'{name}' is not declared")),
            "{message}"
        );
    }

    let missing = read_node_context(
        &service,
        NodeContextInput {
            node_id: "no-such-node".to_string(),
            paths: Vec::new(),
        },
    )
    .await
    .expect_err("a missing node is not found");
    assert!(matches!(missing, OpsError::NotFound { .. }), "{missing:?}");
    Ok(())
}

/// A skill attached to several returned nodes appears once, with each of
/// them; one attached to the node read is returned with no path at all.
#[tokio::test]
async fn a_skill_attached_to_several_returned_nodes_appears_once() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let project = create(&service, "project", "Apollo", json!({})).await;
    let task = create(&service, "task", "Write the spec", json!({})).await;
    service
        .create_relationship(&project, "tasks", &task, json!({}))
        .await?;

    let shared = create_skill(&service, "Shared", "Applies in both places.").await;
    let own = create_skill(&service, "Own", "Applies to the task.").await;
    attach(&service, &shared, &project).await;
    attach(&service, &shared, &task).await;
    attach(&service, &own, &task).await;

    let context = read(&service, &task, &["project", "project"]).await;
    assert_eq!(
        skill_names(&context),
        [
            ("Shared", vec![task.as_str(), project.as_str()]),
            ("Own", vec![task.as_str()]),
        ]
    );
    assert_eq!(
        skill_names(&read(&service, &task, &[]).await),
        [
            ("Shared", vec![task.as_str()]),
            ("Own", vec![task.as_str()])
        ]
    );
    Ok(())
}

/// Detaching a skill removes it from the next read of that node.
#[tokio::test]
async fn a_detached_skill_is_not_returned_by_the_next_read() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let task = create(&service, "task", "Write the spec", json!({})).await;
    let skill = create_skill(&service, "Standards", "Name things plainly.").await;
    // A node's relationship view lists the link before any skill is attached,
    // so the first one can be attached from the node's side.
    let listed = |service: Arc<NodeService>, task: String| async move {
        nodespace_core::ops::rel_ops::get_node_relationships(&service, &task)
            .await
            .expect("the task's relationships")
            .groups
            .iter()
            .find(|group| group.reverse_name == "attached_skills")
            .map(|group| group.count)
    };
    assert_eq!(listed(service.clone(), task.clone()).await, Some(0));

    attach(&service, &skill, &task).await;
    assert_eq!(read(&service, &task, &[]).await.attached.skills.len(), 1);
    assert_eq!(listed(service.clone(), task.clone()).await, Some(1));

    // The ends the wrong way round remove nothing, and the delete says so.
    let delete = |from: String, to: String| {
        let service = service.clone();
        async move {
            nodespace_core::ops::rel_ops::delete_relationship(
                &service,
                nodespace_core::ops::rel_ops::DeleteRelInput {
                    source_id: from,
                    relationship_name: SKILL_ATTACHED_TO.to_string(),
                    target_id: to,
                },
            )
            .await
        }
    };
    // A task declares no `attached_to`, so the swapped form is refused or a
    // no-op; either way the skill stays attached.
    let swapped = delete(task.clone(), skill.clone()).await;
    assert!(!matches!(swapped, Ok(true)), "{swapped:?}");
    assert_eq!(read(&service, &task, &[]).await.attached.skills.len(), 1);

    assert!(delete(skill.clone(), task.clone())
        .await
        .map_err(|e| anyhow::anyhow!("detach: {e}"))?);
    assert!(!delete(skill.clone(), task.clone())
        .await
        .map_err(|e| anyhow::anyhow!("detach again: {e}"))?);
    assert!(read(&service, &task, &[]).await.attached.skills.is_empty());
    assert_eq!(listed(service.clone(), task.clone()).await, Some(0));
    Ok(())
}

/// An archived skill is not returned, an archived node is not reached, and
/// neither are the skills attached to it.
#[tokio::test]
async fn archived_skills_and_archived_nodes_are_not_returned() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let project = create(&service, "project", "Apollo", json!({})).await;
    let task = create(&service, "task", "Write the spec", json!({})).await;
    service
        .create_relationship(&project, "tasks", &task, json!({}))
        .await?;
    create_child(&service, &task, "checkbox", "- [ ] Kept").await;
    let retired = create(&service, "checkbox", "- [ ] Retired", json!({})).await;
    service
        .create_relationship(&task, "has_child", &retired, json!({}))
        .await?;
    archive(&service, &retired).await;

    let live = create_skill(&service, "Live", "Still applies.").await;
    let old = create_skill(&service, "Old", "No longer applies.").await;
    let standards = create_skill(&service, "Standards", "Name things plainly.").await;
    attach(&service, &live, &task).await;
    attach(&service, &old, &task).await;
    attach(&service, &standards, &project).await;
    archive(&service, &old).await;

    let context = read(&service, &task, &["project"]).await;
    assert_eq!(
        skill_names(&context),
        [
            ("Live", vec![task.as_str()]),
            ("Standards", vec![project.as_str()])
        ]
    );
    assert_eq!(context.node.checkboxes.len(), 1);

    archive(&service, &project).await;
    let context = read(&service, &task, &["project"]).await;
    assert!(reached(&context, 0).is_empty());
    assert_eq!(skill_names(&context), [("Live", vec![task.as_str()])]);

    // Asked for by its id, an archived node is still read, as any read by id
    // is, with the skills attached to it.
    let context = read(&service, &project, &[]).await;
    assert_eq!(context.node.node.id, project);
    assert_eq!(
        skill_names(&context),
        [("Standards", vec![project.as_str()])]
    );
    Ok(())
}
