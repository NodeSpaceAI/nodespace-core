//! Context paths on a schema, the skills derived from a node, the version of
//! a context read and a saved query run with context (ADR-094 §2, §4, §7).
//!
//! The workflow here is built from types created through `create_schema`: a
//! desk takes requests, and a request moves between queues by its `state`.
//! Nothing in the read knows those types.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeUpdate, SkillFields, SKILL_ATTACHED_TO};
use nodespace_core::ops::node_context_ops::{
    read_node_context, read_node_contexts, NodeContext, NodeContextInput, SkillQueries,
};
use nodespace_core::ops::query_ops::{run_saved_query_nodes, RunSavedQueryInput};
use nodespace_core::ops::OpsError;
use nodespace_core::schema::{handle_create_schema, handle_update_schema};
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let mut store = Arc::new(SqliteStore::new(temp_dir.path().join("test.db")).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

/// `request` (a `state`, a `due` date) and `desk`, which takes requests.
async fn desk_schemas(service: &Arc<NodeService>) {
    for definition in [
        json!({ "name": "Request", "fields": [
            { "name": "state", "type": "text" },
            { "name": "due", "type": "date" }
        ] }),
        json!({
            "name": "Desk",
            "fields": [],
            "relationships": [{
                "name": "requests", "targetType": "request", "direction": "out",
                "cardinality": "many", "reverseName": "desk", "reverseCardinality": "one"
            }]
        }),
    ] {
        handle_create_schema(service, definition)
            .await
            .unwrap_or_else(|e| panic!("creating a schema failed: {e}"));
    }
}

async fn update_schema(service: &Arc<NodeService>, params: Value) -> Result<Value, String> {
    handle_update_schema(service, params)
        .await
        .map_err(|e| e.to_string())
}

async fn create(service: &NodeService, node_type: &str, content: &str, props: Value) -> String {
    service
        .create_node(Node::new(node_type.to_string(), content.to_string(), props))
        .await
        .unwrap_or_else(|e| panic!("creating the {node_type} '{content}' failed: {e}"))
}

async fn create_child(service: &NodeService, parent: &str, node_type: &str, content: &str) -> String {
    let id = create(service, node_type, content, json!({})).await;
    service
        .create_relationship(parent, "has_child", &id, json!({}))
        .await
        .unwrap_or_else(|e| panic!("placing '{content}' under {parent} failed: {e}"));
    id
}

/// A skill named `name` whose procedure is the one line `step`: the skill's
/// id and the id of that line.
async fn create_skill(service: &NodeService, name: &str, step: &str) -> (String, String) {
    let node = SkillFields::new("When this applies.", &["get_node"], 2).into_node(name);
    let id = service.create_node(node).await.expect("the skill");
    let line = create_child(service, &id, "text", step).await;
    (id, line)
}

async fn attach(service: &NodeService, skill: &str, node: &str) {
    service
        .create_relationship(skill, SKILL_ATTACHED_TO, node, json!({}))
        .await
        .unwrap_or_else(|e| panic!("attaching {skill} to {node} failed: {e}"));
}

async fn update(service: &NodeService, id: &str, update: NodeUpdate) {
    let version = service.get_node(id).await.unwrap().unwrap().version;
    service
        .update_node(id, version, update)
        .await
        .unwrap_or_else(|e| panic!("updating {id} failed: {e}"));
}

async fn set(service: &NodeService, id: &str, properties: Value) {
    update(service, id, NodeUpdate::new().with_properties(properties)).await;
}

/// A saved query over requests in `state`.
async fn state_queue(service: &NodeService, title: &str, state: &str) -> String {
    create(
        service,
        "query",
        title,
        json!({ "target_type": "request", "filters": [{
            "type": "property", "operator": "equals", "property": "state", "value": state
        }] }),
    )
    .await
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

fn path_names(context: &NodeContext) -> Vec<String> {
    context
        .paths
        .iter()
        .map(|reached| reached.path.to_string())
        .collect()
}

fn skill_names(context: &NodeContext) -> Vec<&str> {
    context
        .attached
        .skills
        .iter()
        .map(|attached| attached.skill.name.as_str())
        .collect()
}

async fn declared_paths(service: &NodeService, schema_id: &str) -> Vec<String> {
    service
        .get_schema_node(schema_id)
        .await
        .unwrap()
        .unwrap()
        .context_paths
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// A schema's context paths are added and removed through `update_schema`,
/// each checked against the schemas when it is saved.
#[tokio::test]
async fn a_schema_declares_context_paths_checked_when_saved() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    desk_schemas(&service).await;

    // The list-of-hops form and the dotted form name the same path.
    let added = update_schema(
        &service,
        json!({ "schema_id": "request", "add_context_paths": [["desk"], "desk.requests"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    assert_eq!(added["contextPathsAdded"], 2);
    assert_eq!(
        declared_paths(&service, "request").await,
        ["desk", "desk.requests"]
    );

    // A name the type does not declare is refused, and the message names the
    // path and what the type does declare.
    let refused = update_schema(
        &service,
        json!({ "schema_id": "request", "add_context_paths": ["desk.sponsor"] }),
    )
    .await
    .unwrap_err();
    assert!(
        refused.contains("Context path 'desk.sponsor'")
            && refused.contains("'sponsor' is not declared"),
        "{refused}"
    );
    // A name after a relationship to any type cannot be checked.
    let unchecked = update_schema(
        &service,
        json!({ "schema_id": "request", "add_context_paths": ["child_of.desk"] }),
    )
    .await
    .unwrap_err();
    assert!(
        unchecked.contains("Context path 'child_of.desk'"),
        "{unchecked}"
    );
    for (params, expected) in [
        (
            json!({ "schema_id": "request", "add_context_paths": ["desk"] }),
            "already declares the context path 'desk'",
        ),
        (
            json!({ "schema_id": "request", "add_context_paths": ["child_of", "child_of"] }),
            "listed twice",
        ),
        (
            json!({ "schema_id": "request", "add_context_paths": "desk" }),
            "takes a list of paths",
        ),
        (
            json!({ "schema_id": "request", "add_context_paths": ["desk."] }),
            "empty hop",
        ),
        (
            json!({ "schema_id": "request", "remove_context_paths": ["child_of"] }),
            "declares no context path 'child_of'",
        ),
    ] {
        let refused = update_schema(&service, params).await.unwrap_err();
        assert!(refused.contains(expected), "{expected}: {refused}");
    }
    // A refused call changes nothing.
    assert_eq!(
        declared_paths(&service, "request").await,
        ["desk", "desk.requests"]
    );

    let removed = update_schema(
        &service,
        json!({ "schema_id": "request", "remove_context_paths": ["desk.requests"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    assert_eq!(removed["contextPathsRemoved"], 1);
    assert_eq!(declared_paths(&service, "request").await, ["desk"]);

    // A change that touches nothing else leaves the paths as they are.
    update_schema(
        &service,
        json!({ "schema_id": "request", "add_fields": [{ "name": "note", "type": "text" }] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    assert_eq!(declared_paths(&service, "request").await, ["desk"]);

    // Removing the last one stores none.
    update_schema(
        &service,
        json!({ "schema_id": "request", "remove_context_paths": ["desk"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    assert!(declared_paths(&service, "request").await.is_empty());
    let row = service.get_node("request").await?.unwrap();
    assert!(row.properties.get("contextPaths").is_none(), "{row:?}");

    // A generic node update is not a way around the check.
    let version = row.version;
    let bypass = service
        .update_node(
            "request",
            version,
            NodeUpdate::new().with_properties(json!({ "contextPaths": [["sponsor"]] })),
        )
        .await
        .expect_err("context paths are written through update_schema");
    assert!(bypass.to_string().contains("update_schema"), "{bypass}");
    Ok(())
}

/// A subtype's context paths are its ancestors' and then its own, and a
/// context read of a subtype's node follows them all.
#[tokio::test]
async fn a_subtype_follows_its_ancestors_paths_and_its_own() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    desk_schemas(&service).await;
    handle_create_schema(
        &service,
        json!({
            "name": "Complaint",
            "extends": "request",
            "fields": [],
            "relationships": [{
                "name": "about", "targetType": "desk", "direction": "out",
                "cardinality": "one", "reverseName": "complaints", "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("complaint schema: {e}"))?;

    update_schema(
        &service,
        json!({ "schema_id": "complaint", "add_context_paths": ["about"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    update_schema(
        &service,
        json!({ "schema_id": "request", "add_context_paths": ["desk"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;

    let in_force: Vec<(String, String)> = service
        .resolve_context_paths("complaint")
        .await?
        .into_iter()
        .map(|(path, schema)| (path.to_string(), schema))
        .collect();
    assert_eq!(
        in_force,
        [
            ("desk".to_string(), "request".to_string()),
            ("about".to_string(), "complaint".to_string())
        ]
    );
    // The subtype's own list holds only what it declares.
    assert_eq!(declared_paths(&service, "complaint").await, ["about"]);

    // An inherited path is neither declared again nor removed on the subtype.
    let again = update_schema(
        &service,
        json!({ "schema_id": "complaint", "add_context_paths": ["desk"] }),
    )
    .await
    .unwrap_err();
    assert!(
        again.contains("already in force") && again.contains("'request'"),
        "{again}"
    );
    let elsewhere = update_schema(
        &service,
        json!({ "schema_id": "complaint", "remove_context_paths": ["desk"] }),
    )
    .await
    .unwrap_err();
    assert!(
        elsewhere.contains("declared by schema 'request'") && elsewhere.contains("remove it there"),
        "{elsewhere}"
    );

    let front = create(&service, "desk", "Front desk", json!({})).await;
    let back = create(&service, "desk", "Back office", json!({})).await;
    let complaint = create(&service, "complaint", "Too slow", json!({})).await;
    service
        .create_relationship(&front, "requests", &complaint, json!({}))
        .await?;
    service
        .create_relationship(&complaint, "about", &back, json!({}))
        .await?;

    let context = read(&service, &complaint, &[]).await;
    assert_eq!(path_names(&context), ["desk", "about"]);
    assert_eq!(context.paths[0].nodes[0].node.id, front);
    assert_eq!(context.paths[1].nodes[0].node.id, back);

    // A plain request follows its own type's path alone.
    let request = create(&service, "request", "New badge", json!({})).await;
    assert_eq!(path_names(&read(&service, &request, &[]).await), ["desk"]);
    Ok(())
}

/// Context paths are added to and removed from a core schema like any other,
/// and what a user declared there is still there after the database is
/// opened again and its core schemas seeded again (ADR-072).
#[tokio::test]
async fn a_core_schemas_context_paths_survive_a_restart() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    {
        let mut store = Arc::new(SqliteStore::new(db_path.clone()).await?);
        let service = Arc::new(NodeService::new(&mut store).await?);
        assert!(declared_paths(&service, "task").await.is_empty());
        update_schema(
            &service,
            json!({ "schema_id": "task", "add_context_paths": ["project", "blocked_by"] }),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        update_schema(
            &service,
            json!({ "schema_id": "task", "remove_context_paths": ["blocked_by"] }),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        assert_eq!(declared_paths(&service, "task").await, ["project"]);
    }

    // Opening the database seeds the core schemas again.
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    assert_eq!(declared_paths(&service, "task").await, ["project"]);
    assert!(service.get_schema_node("task").await?.unwrap().is_core);

    let project = create(&service, "project", "Apollo", json!({})).await;
    let task = create(&service, "task", "Write the spec", json!({})).await;
    service
        .create_relationship(&project, "tasks", &task, json!({}))
        .await?;
    let context = read(&service, &task, &[]).await;
    assert_eq!(path_names(&context), ["project"]);
    assert_eq!(context.paths[0].nodes[0].node.id, project);
    Ok(())
}

/// A read with no paths follows the type's context paths; paths given are
/// followed in addition, and one that is already a context path is followed
/// once.
#[tokio::test]
async fn a_context_read_follows_the_types_paths_and_those_asked_for() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    desk_schemas(&service).await;
    let desk = create(&service, "desk", "Front desk", json!({})).await;
    let request = create(&service, "request", "New badge", json!({})).await;
    let other = create(&service, "request", "New desk", json!({})).await;
    for node in [&request, &other] {
        service
            .create_relationship(&desk, "requests", node, json!({}))
            .await?;
    }

    // With nothing declared, a read follows what it is given.
    assert!(read(&service, &request, &[]).await.paths.is_empty());

    update_schema(
        &service,
        json!({ "schema_id": "request", "add_context_paths": ["desk"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;

    let context = read(&service, &request, &[]).await;
    assert_eq!(path_names(&context), ["desk"]);
    assert_eq!(context.paths[0].nodes[0].node.id, desk);

    let context = read(&service, &request, &["desk.requests", "desk"]).await;
    assert_eq!(path_names(&context), ["desk", "desk.requests"]);
    assert_eq!(context.paths[1].nodes.len(), 2);

    // The desk's type declares none, so a desk is read on its own.
    assert!(read(&service, &desk, &[]).await.paths.is_empty());
    Ok(())
}

/// A context path whose relationship has since been removed fails the read
/// with the schema to repair, not with an empty result.
#[tokio::test]
async fn a_context_path_that_no_longer_resolves_names_its_schema() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    desk_schemas(&service).await;
    let request = create(&service, "request", "New badge", json!({})).await;
    update_schema(
        &service,
        json!({ "schema_id": "request", "add_context_paths": ["desk"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    update_schema(
        &service,
        json!({ "schema_id": "desk", "remove_relationships": ["requests"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;

    let error = read_node_context(
        &service,
        NodeContextInput {
            node_id: request.clone(),
            paths: Vec::new(),
        },
    )
    .await
    .expect_err("the declared path no longer resolves");
    let OpsError::InvalidParams(message) = &error else {
        panic!("expected InvalidParams, got {error:?}");
    };
    assert!(
        message.contains("context path 'desk'")
            && message.contains("schema 'request'")
            && message.contains("remove_context_paths"),
        "{message}"
    );

    update_schema(
        &service,
        json!({ "schema_id": "request", "remove_context_paths": ["desk"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;
    assert!(read(&service, &request, &[]).await.paths.is_empty());
    Ok(())
}

/// A node carries the skills of every saved query it currently matches: it
/// moves from one queue's procedure to another's when a field changes, and
/// the skill a queue hands over is reached through that queue.
#[tokio::test]
async fn a_node_carries_the_skills_of_the_queries_it_matches() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    desk_schemas(&service).await;
    let request = create(&service, "request", "New badge", json!({ "state": "new" })).await;

    let new_queue = state_queue(&service, "New requests", "new").await;
    let review_queue = state_queue(&service, "Awaiting review", "review").await;
    let (handling, _) = create_skill(&service, "Handling a request", "Confirm, then fulfil.").await;
    let (reviewing, _) = create_skill(&service, "Reviewing", "Check it was fulfilled.").await;
    let (both, _) = create_skill(&service, "Request etiquette", "Be kind.").await;
    attach(&service, &handling, &new_queue).await;
    attach(&service, &reviewing, &review_queue).await;
    attach(&service, &both, &new_queue).await;
    attach(&service, &both, &review_queue).await;

    let sources = |context: &NodeContext| -> Vec<(String, Vec<String>)> {
        context
            .attached
            .skills
            .iter()
            .map(|attached| {
                assert!(attached.attached_to.is_empty());
                (
                    attached.skill.name.clone(),
                    attached
                        .matched_queries
                        .iter()
                        .map(|query| query.title.clone())
                        .collect(),
                )
            })
            .collect()
    };
    let named = |name: &str, queue: &str| (name.to_string(), vec![queue.to_string()]);

    let context = read(&service, &request, &[]).await;
    assert_eq!(
        sources(&context),
        [
            named("Handling a request", "New requests"),
            named("Request etiquette", "New requests")
        ]
    );
    assert_eq!(context.attached.skills[0].matched_queries[0].id, new_queue);
    assert!(context.attached.skills[0]
        .skill
        .instructions
        .contains("Confirm, then fulfil."));

    // Picked up for review: it has left one queue and joined the other.
    set(&service, &request, json!({ "state": "review" })).await;
    assert_eq!(
        sources(&read(&service, &request, &[]).await),
        [
            named("Reviewing", "Awaiting review"),
            named("Request etiquette", "Awaiting review")
        ]
    );

    // In neither queue, it carries neither procedure.
    set(&service, &request, json!({ "state": "closed" })).await;
    assert!(read(&service, &request, &[]).await.attached.skills.is_empty());

    // Back in the first.
    set(&service, &request, json!({ "state": "new" })).await;
    assert_eq!(
        skill_names(&read(&service, &request, &[]).await),
        ["Handling a request", "Request etiquette"]
    );

    // A skill attached to the node as well as to a queue it matches appears
    // once, with both.
    attach(&service, &both, &request).await;
    let context = read(&service, &request, &[]).await;
    assert_eq!(
        skill_names(&context),
        ["Request etiquette", "Handling a request"]
    );
    assert_eq!(context.attached.skills[0].attached_to, [request.clone()]);
    assert_eq!(context.attached.skills[0].matched_queries[0].id, new_queue);

    // An archived node matches no query; an archived query hands nothing
    // over.
    update(
        &service,
        &new_queue,
        NodeUpdate::new().with_lifecycle_status("archived".to_string()),
    )
    .await;
    assert_eq!(
        skill_names(&read(&service, &request, &[]).await),
        ["Request etiquette"]
    );
    Ok(())
}

/// Membership is asked only of the saved queries a skill is attached to, as
/// each is stored: a relative date is read on the day of the read.
#[tokio::test]
async fn membership_is_asked_only_of_queries_with_a_skill_attached() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    desk_schemas(&service).await;

    let with_skill = state_queue(&service, "New requests", "new").await;
    let without = state_queue(&service, "Closed requests", "closed").await;
    let overdue = create(
        &service,
        "query",
        "Overdue",
        json!({ "target_type": "request", "limit": 1, "filters": [{
            "type": "property", "operator": "lt", "property": "due",
            "relative_date": { "anchor": "today" }
        }] }),
    )
    .await;
    // A node of another type with a skill attached is not a query to ask.
    let desk = create(&service, "desk", "Front desk", json!({})).await;

    let (handling, _) = create_skill(&service, "Handling a request", "Confirm, then fulfil.").await;
    let (chasing, _) = create_skill(&service, "Chasing", "Ask what is blocking it.").await;
    attach(&service, &handling, &with_skill).await;
    attach(&service, &handling, &desk).await;
    attach(&service, &chasing, &overdue).await;

    let asked = SkillQueries::load(&service).await?;
    assert_eq!(asked.query_ids(), [with_skill.as_str(), overdue.as_str()]);
    assert!(!asked.query_ids().contains(&without.as_str()));

    let late = create(
        &service,
        "request",
        "Late",
        json!({ "state": "new", "due": "2000-01-01" }),
    )
    .await;
    let also_late = create(
        &service,
        "request",
        "Also late",
        json!({ "state": "closed", "due": "2000-01-02" }),
    )
    .await;
    let early = create(
        &service,
        "request",
        "Early",
        json!({ "state": "new", "due": "2999-01-01" }),
    )
    .await;
    assert_eq!(
        skill_names(&read(&service, &late, &[]).await),
        ["Handling a request", "Chasing"]
    );
    // The stored limit of one decides what a run shows, not what matches.
    assert_eq!(
        skill_names(&read(&service, &also_late, &[]).await),
        ["Chasing"]
    );
    assert_eq!(
        skill_names(&read(&service, &early, &[]).await),
        ["Handling a request"]
    );
    Ok(())
}

/// The version of a read is the same until the node, a node it returned, an
/// applicable skill's content or the set of applicable skills changes, and is
/// unmoved by a change to anything else.
#[tokio::test]
async fn a_reads_version_follows_what_the_read_returns() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    desk_schemas(&service).await;
    update_schema(
        &service,
        json!({ "schema_id": "request", "add_context_paths": ["desk"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;

    let desk = create(&service, "desk", "Front desk", json!({})).await;
    let request = create(&service, "request", "New badge", json!({ "state": "new" })).await;
    let other = create(&service, "request", "New desk", json!({ "state": "new" })).await;
    for node in [&request, &other] {
        service
            .create_relationship(&desk, "requests", node, json!({}))
            .await?;
    }
    let item = create_child(&service, &request, "checkbox", "- [ ] Print it").await;
    let note = create_child(&service, &request, "text", "Some notes").await;
    let queue = state_queue(&service, "New requests", "new").await;
    let (rules, rule_line) = create_skill(&service, "Desk rules", "Answer within a day.").await;
    let (handling, _) = create_skill(&service, "Handling a request", "Confirm, then fulfil.").await;
    attach(&service, &rules, &desk).await;
    attach(&service, &handling, &queue).await;

    let version = |service: Arc<NodeService>, request: String| async move {
        read(&service, &request, &[]).await.version
    };
    let mut current = version(service.clone(), request.clone()).await;
    assert!(!current.is_empty());
    assert_eq!(version(service.clone(), request.clone()).await, current);

    // Nothing the read returns: another request, a note under this one, a
    // skill that applies to nothing here, a query with no skill.
    update(
        &service,
        &other,
        NodeUpdate::new().with_content("New standing desk".to_string()),
    )
    .await;
    update(
        &service,
        &note,
        NodeUpdate::new().with_content("Other notes".to_string()),
    )
    .await;
    create_skill(&service, "Unrelated", "Applies elsewhere.").await;
    state_queue(&service, "Also new", "new").await;
    assert_eq!(version(service.clone(), request.clone()).await, current);

    fn moved(current: &mut String, label: &str, next: String) {
        assert_ne!(next, *current, "{label} must change the version");
        *current = next;
    }

    update(
        &service,
        &request,
        NodeUpdate::new().with_content("New badge, urgently".to_string()),
    )
    .await;
    moved(&mut current, "the node",version(service.clone(), request.clone()).await);

    update(
        &service,
        &item,
        NodeUpdate::new().with_content("- [x] Print it".to_string()),
    )
    .await;
    moved(
        &mut current,
        "a checklist item",
        version(service.clone(), request.clone()).await,
    );

    update(
        &service,
        &desk,
        NodeUpdate::new().with_content("Reception".to_string()),
    )
    .await;
    moved(
        &mut current,
        "a node the read returned",
        version(service.clone(), request.clone()).await,
    );

    // A skill's procedure is its children: the skill node itself is not
    // written.
    update(
        &service,
        &rule_line,
        NodeUpdate::new().with_content("Answer within the hour.".to_string()),
    )
    .await;
    moved(
        &mut current,
        "an attached skill's content",
        version(service.clone(), request.clone()).await,
    );

    let (extra, _) = create_skill(&service, "Badge policy", "Photo required.").await;
    attach(&service, &extra, &request).await;
    moved(
        &mut current,
        "a skill attached",
        version(service.clone(), request.clone()).await,
    );

    // Leaving the queue takes its skill away. The node changed too, so the
    // set alone is shown by a queue gaining a skill below.
    set(&service, &request, json!({ "state": "review" })).await;
    moved(
        &mut current,
        "leaving a queue",
        version(service.clone(), request.clone()).await,
    );
    let review = state_queue(&service, "Awaiting review", "review").await;
    assert_eq!(version(service.clone(), request.clone()).await, current);
    attach(&service, &handling, &review).await;
    moved(
        &mut current,
        "a matched queue gaining a skill",
        version(service.clone(), request.clone()).await,
    );

    // The same read, asked for with the path the type already declares.
    assert_eq!(read(&service, &request, &["desk"]).await.version, current);
    Ok(())
}

/// A saved query run with context returns each item with its governing nodes
/// and applicable skills; a limit applies to the items, and a skill several
/// items share is returned once.
#[tokio::test]
async fn a_query_run_with_context_returns_each_item_whole() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    desk_schemas(&service).await;
    update_schema(
        &service,
        json!({ "schema_id": "request", "add_context_paths": ["desk"] }),
    )
    .await
    .map_err(anyhow::Error::msg)?;

    let front = create(&service, "desk", "Front desk", json!({})).await;
    let back = create(&service, "desk", "Back office", json!({})).await;
    let mut requests = Vec::new();
    for (title, desk) in [("First", &front), ("Second", &front), ("Third", &back)] {
        let request = create(&service, "request", title, json!({ "state": "new" })).await;
        service
            .create_relationship(desk, "requests", &request, json!({}))
            .await?;
        requests.push(request);
    }
    create_child(&service, &requests[0], "checkbox", "- [ ] Print it").await;
    let queue = create(
        &service,
        "query",
        "New requests",
        json!({
            "target_type": "request",
            "filters": [{
                "type": "property", "operator": "equals", "property": "state", "value": "new"
            }],
            "sorting": [{ "field": "created_at", "direction": "asc" }]
        }),
    )
    .await;
    let (handling, _) = create_skill(&service, "Handling a request", "Confirm, then fulfil.").await;
    let (rules, _) = create_skill(&service, "Desk rules", "Answer within a day.").await;
    let (own, _) = create_skill(&service, "Badge policy", "Photo required.").await;
    attach(&service, &handling, &queue).await;
    attach(&service, &rules, &front).await;
    attach(&service, &own, &requests[0]).await;

    let run = |limit: Option<usize>, filters: Value| {
        let service = service.clone();
        async move {
            let run = run_saved_query_nodes(
                &service,
                RunSavedQueryInput {
                    query: "New requests".to_string(),
                    filters: serde_json::from_value(filters).unwrap(),
                    limit,
                    max_rows: None,
                },
            )
            .await
            .unwrap_or_else(|e| panic!("running the queue failed: {e}"));
            read_node_contexts(&service, run.nodes, &run.query_id)
                .await
                .unwrap_or_else(|e| panic!("reading the items failed: {e}"))
        }
    };

    let context = run(None, json!([])).await;
    assert_eq!(
        context
            .items
            .iter()
            .map(|item| item.node.node.id.clone())
            .collect::<Vec<_>>(),
        requests
    );
    // Each item as a context read of it returns it.
    for (item, request) in context.items.iter().zip(&requests) {
        let alone = read(&service, request, &[]).await;
        assert_eq!(item.version, alone.version);
        assert_eq!(path_names(item), ["desk"]);
        assert_eq!(skill_names(item), skill_names(&alone));
    }
    assert_eq!(context.items[0].node.checkboxes.len(), 1);
    assert_eq!(context.items[0].paths[0].nodes[0].node.id, front);
    assert_eq!(
        skill_names(&context.items[0]),
        ["Badge policy", "Handling a request", "Desk rules"]
    );
    assert_eq!(
        skill_names(&context.items[1]),
        ["Handling a request", "Desk rules"]
    );
    assert_eq!(skill_names(&context.items[2]), ["Handling a request"]);

    // Every skill once, the query's own first, where it says it is attached
    // to the query that ran.
    let once: Vec<(&str, &[String])> = context
        .attached
        .skills
        .iter()
        .map(|attached| (attached.skill.name.as_str(), attached.attached_to.as_slice()))
        .collect();
    assert_eq!(
        once,
        [
            ("Handling a request", std::slice::from_ref(&queue)),
            ("Badge policy", &[][..]),
            ("Desk rules", &[][..]),
        ]
    );

    // The limit applies to the items, and only their skills come back.
    let context = run(Some(1), json!([])).await;
    assert_eq!(context.items.len(), 1);
    assert_eq!(context.items[0].node.node.id, requests[0]);
    assert_eq!(context.attached.skills.len(), 3);
    let context = run(
        None,
        json!([{ "property": "title", "operator": "equals", "value": "Third" }]),
    )
    .await;
    assert_eq!(context.items.len(), 1);
    assert_eq!(context.items[0].node.node.id, requests[2]);
    // Narrowed for this run, the item is still read as matching the stored
    // query.
    assert_eq!(
        context.items[0].attached.skills[0].matched_queries[0].id,
        queue
    );
    assert_eq!(context.attached.skills.len(), 1);

    // A run that returns nothing still hands over the query's procedure.
    let context = run(
        None,
        json!([{ "property": "title", "operator": "equals", "value": "Nothing" }]),
    )
    .await;
    assert!(context.items.is_empty());
    assert_eq!(
        context.attached.skills[0].skill.name,
        "Handling a request"
    );
    Ok(())
}
