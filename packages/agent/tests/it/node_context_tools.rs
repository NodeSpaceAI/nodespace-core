//! The `get_node_context` and `delete_relationship` tools: a node read with
//! what its paths reach and the skills attached along the way, and a skill
//! detached again, through the production `GraphToolExecutor::execute`
//! surface against a real store (ADR-094 §2 and §3), and their place in the
//! tool registry and the seeded skills.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::{GraphToolExecutor, Tool};
use nodespace_agent::skill_pipeline::{seed_tool_nodes, SKILL_SEEDS};
use nodespace_agent::{AgentToolExecutor, ToolResult};
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, SkillFields, SKILL_ATTACHED_TO};
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

async fn make_executor() -> (GraphToolExecutor, Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut store: Arc<SqliteStore> =
        Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, ns, tmp)
}

async fn call(executor: &GraphToolExecutor, tool: &str, args: Value) -> ToolResult {
    executor
        .execute(tool, args)
        .await
        .unwrap_or_else(|e| panic!("{tool} must return a tool result: {e}"))
}

struct Fixture {
    project: String,
    task: String,
    skill: String,
}

/// A project with one task, the task with one checkbox, and a skill attached
/// to the project.
async fn seed(ns: &Arc<NodeService>) -> Fixture {
    let create = |node_type: &str, content: &str| {
        let node = Node::new(node_type.to_string(), content.to_string(), json!({}));
        async move { ns.create_node(node).await.expect("the node is created") }
    };
    let project = create("project", "Apollo").await;
    let task = create("task", "Write the spec").await;
    ns.create_relationship(&project, "tasks", &task, json!({}))
        .await
        .unwrap();
    let checkbox = create("checkbox", "- [ ] Draft it").await;
    ns.create_relationship(&task, "has_child", &checkbox, json!({}))
        .await
        .unwrap();

    let skill = ns
        .create_node(
            SkillFields::new("Project standards.", &["get_node"], 2).into_node("Standards"),
        )
        .await
        .unwrap();
    let step = create("text", "Name things plainly.").await;
    ns.create_relationship(&skill, "has_child", &step, json!({}))
        .await
        .unwrap();
    ns.create_relationship(&skill, SKILL_ATTACHED_TO, &project, json!({}))
        .await
        .unwrap();
    Fixture {
        project,
        task,
        skill,
    }
}

/// The entry of a context read for the path `names`. A task's read starts
/// with the context paths its type ships with, so an entry is found by path.
fn path_entry<'a>(context: &'a Value, names: &[&str]) -> &'a Value {
    context["paths"]
        .as_array()
        .expect("paths")
        .iter()
        .find(|entry| entry["path"] == json!(names))
        .unwrap_or_else(|| panic!("the read followed no path {names:?}: {context}"))
}

/// The paths a context read followed, each as its names.
fn paths_followed(context: &Value) -> Vec<Value> {
    context["paths"]
        .as_array()
        .expect("paths")
        .iter()
        .map(|entry| entry["path"].clone())
        .collect()
}

#[tokio::test]
async fn get_node_context_returns_the_node_what_its_paths_reach_and_the_attached_skills() {
    let (executor, ns, _tmp) = make_executor().await;
    let fixture = seed(&ns).await;

    let result = call(
        &executor,
        "get_node_context",
        json!({ "id": format!("nodespace://{}", fixture.task), "paths": [["project"]] }),
    )
    .await;
    assert!(!result.is_error, "{}", result.result);
    let context = &result.result;

    assert_eq!(context["node"]["title"], "Write the spec");
    let task_version = ns.get_node(&fixture.task).await.unwrap().unwrap().version;
    assert_eq!(context["node"]["node_version"], task_version);
    let checkboxes = context["node"]["checkboxes"].as_array().expect("items");
    assert_eq!(checkboxes.len(), 1);
    assert_eq!(checkboxes[0]["content"], "- [ ] Draft it");

    let to_project = path_entry(context, &["project"]);
    assert_eq!(to_project["count"], 1);
    assert_eq!(
        to_project["nodes"][0]["id"],
        format!("nodespace://{}", fixture.project)
    );

    let skills = context["skills"].as_array().expect("skills");
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["name"], "Standards");
    assert_eq!(
        skills[0]["attached_to"],
        json!([format!("nodespace://{}", fixture.project)])
    );
    assert!(skills[0]["instructions"]
        .as_str()
        .is_some_and(|body| body.contains("Name things plainly.")));

    // The path asked for is one `task` ships with as a context path, so it
    // is followed once, and a read given no path is the same read.
    let shipped = [
        json!(["spec"]),
        json!(["plan"]),
        json!(["decisions"]),
        json!(["spec", "decisions"]),
        json!(["project"]),
    ];
    assert_eq!(paths_followed(context), shipped);
    let unasked = call(&executor, "get_node_context", json!({ "id": fixture.task })).await;
    assert_eq!(paths_followed(&unasked.result), shipped);
    assert_eq!(unasked.result["skills"], context["skills"]);
    assert_eq!(unasked.result["version"], context["version"]);

    // A node of a type with no context paths comes back alone.
    let note = ns
        .create_node(Node::new(
            "text".to_string(),
            "A note".to_string(),
            json!({}),
        ))
        .await
        .unwrap();
    let alone = call(&executor, "get_node_context", json!({ "id": note })).await;
    assert_eq!(alone.result["paths"], json!([]));
    assert_eq!(alone.result["skills"], json!([]));

    // An item is ticked by the id and version the read gave for it, and the
    // version it gave is refused once the item has changed.
    let tick = json!({
        "id": checkboxes[0]["id"],
        "content": "- [x] Draft it",
        "version": checkboxes[0]["node_version"],
    });
    let ticked = call(&executor, "update_node", tick.clone()).await;
    assert!(!ticked.is_error, "{}", ticked.result);
    let mut untick = tick;
    untick["content"] = json!("- [ ] Draft it");
    let stale = executor
        .execute("update_node", untick)
        .await
        .expect_err("a version the item has moved past is refused");
    assert!(
        stale.to_string().contains("has changed since it was read"),
        "{stale}"
    );
}

#[tokio::test]
async fn get_node_context_says_which_path_name_does_not_apply() {
    let (executor, ns, _tmp) = make_executor().await;
    let fixture = seed(&ns).await;

    let result = call(
        &executor,
        "get_node_context",
        json!({ "id": fixture.task, "paths": [["project", "sponsor"]] }),
    )
    .await;
    assert!(result.is_error, "expected a tool error: {}", result.result);
    let message = result.result["error"].as_str().expect("an error message");
    assert!(
        message.contains("path 'project.sponsor'") && message.contains("'sponsor' is not declared"),
        "{message}"
    );
}

#[tokio::test]
async fn delete_relationship_detaches_a_skill_from_the_next_read() {
    let (executor, ns, _tmp) = make_executor().await;
    let fixture = seed(&ns).await;
    let read = || {
        call(
            &executor,
            "get_node_context",
            json!({ "id": fixture.project }),
        )
    };
    assert_eq!(read().await.result["skills"].as_array().unwrap().len(), 1);

    let args = json!({
        "from_id": format!("nodespace://{}", fixture.skill),
        "to_id": fixture.project,
        "relationship_type": "attached_to",
    });
    let detached = call(&executor, "delete_relationship", args.clone()).await;
    assert!(!detached.is_error, "{}", detached.result);
    assert_eq!(detached.result["deleted"], true);
    assert_eq!(
        detached.result["from_id"],
        format!("nodespace://{}", fixture.skill)
    );
    assert_eq!(read().await.result["skills"], json!([]));

    // Removing a link that is already gone changes nothing and succeeds.
    // The result says nothing was removed, so no deletion is reported.
    let again = call(&executor, "delete_relationship", args).await;
    assert!(!again.is_error, "{}", again.result);
    assert_eq!(again.result["deleted"], false);
    assert!(again.result["note"]
        .as_str()
        .is_some_and(|note| note.contains("nothing was removed")));
}

/// A saved query's run hands over the skills attached to the query, once,
/// beside its nodes; a query with none attached has no `skills` key.
#[tokio::test]
async fn run_query_returns_the_skills_attached_to_the_query() {
    let (executor, ns, _tmp) = make_executor().await;
    let fixture = seed(&ns).await;
    let queue = ns
        .create_node(Node::new(
            "query".to_string(),
            "All tasks".to_string(),
            json!({ "target_type": "task", "filters": [] }),
        ))
        .await
        .unwrap();

    let bare = call(&executor, "run_query", json!({ "query": "All tasks" })).await;
    assert_eq!(bare.result["count"], 1);
    assert!(bare.result.get("skills").is_none(), "{}", bare.result);

    ns.create_relationship(&fixture.skill, SKILL_ATTACHED_TO, &queue, json!({}))
        .await
        .unwrap();
    let run = call(&executor, "run_query", json!({ "query": "All tasks" })).await;
    let skills = run.result["skills"].as_array().expect("skills");
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["name"], "Standards");
    assert_eq!(
        skills[0]["attached_to"],
        json!([format!("nodespace://{queue}")])
    );
    assert!(skills[0]["instructions"]
        .as_str()
        .is_some_and(|body| body.contains("Name things plainly.")));
}

/// A schema's context paths are declared through `update_schema`, and a
/// context read then follows them unasked. The read's version is returned
/// with it, and alone when only it is asked for (ADR-094 §2 and §7).
#[tokio::test]
async fn get_node_context_follows_the_types_context_paths_and_returns_a_version() {
    let (executor, ns, _tmp) = make_executor().await;
    let fixture = seed(&ns).await;

    let refused = call(
        &executor,
        "update_schema",
        json!({ "schema_id": "task", "add_context_paths": ["sponsor"] }),
    )
    .await;
    assert!(refused.is_error, "{}", refused.result);
    assert!(
        refused.result["error"]
            .as_str()
            .is_some_and(|message| message.contains("Context path 'sponsor'")),
        "{}",
        refused.result
    );
    // One `task` ships with is already declared.
    let already = call(
        &executor,
        "update_schema",
        json!({ "schema_id": "task", "add_context_paths": ["project"] }),
    )
    .await;
    assert!(already.is_error, "{}", already.result);
    let declared = call(
        &executor,
        "update_schema",
        json!({ "schema_id": "task", "add_context_paths": ["project.tasks"] }),
    )
    .await;
    assert!(!declared.is_error, "{}", declared.result);
    assert_eq!(declared.result["contextPathsAdded"], 1);

    let read = call(&executor, "get_node_context", json!({ "id": fixture.task })).await;
    assert!(!read.is_error, "{}", read.result);
    // The paths it ships with, then the one declared here.
    assert_eq!(paths_followed(&read.result).len(), 6);
    assert_eq!(
        path_entry(&read.result, &["project", "tasks"])["nodes"][0]["id"],
        format!("nodespace://{}", fixture.task)
    );
    assert_eq!(read.result["skills"][0]["name"], "Standards");
    let version = read.result["version"].as_str().expect("a version");
    assert!(!version.is_empty());

    let alone = call(
        &executor,
        "get_node_context",
        json!({ "id": fixture.task, "version_only": true }),
    )
    .await;
    assert_eq!(alone.result, json!({ "version": version }));

    // The project changed, so the task's read has moved on.
    let project = ns.get_node(&fixture.project).await.unwrap().unwrap();
    ns.update_node(
        &fixture.project,
        project.version,
        nodespace_core::models::NodeUpdate::new().with_content("Apollo 2".to_string()),
    )
    .await
    .unwrap();
    let after = call(
        &executor,
        "get_node_context",
        json!({ "id": fixture.task, "version_only": true }),
    )
    .await;
    assert_ne!(after.result["version"], json!(version));
}

/// A run with context returns each item as a context read returns it, with
/// every skill once: the one attached to the query the item matches, and the
/// one attached to a node the item's context paths reach (ADR-094 §4).
#[tokio::test]
async fn run_query_with_context_returns_each_item_with_its_skills() {
    let (executor, ns, _tmp) = make_executor().await;
    let fixture = seed(&ns).await;
    let second = ns
        .create_node(Node::new(
            "task".to_string(),
            "Review the spec".to_string(),
            json!({}),
        ))
        .await
        .unwrap();
    ns.create_relationship(&fixture.project, "tasks", &second, json!({}))
        .await
        .unwrap();
    let queue = ns
        .create_node(Node::new(
            "query".to_string(),
            "All tasks".to_string(),
            json!({
                "target_type": "task",
                "filters": [],
                "sorting": [{ "field": "created_at", "direction": "asc" }]
            }),
        ))
        .await
        .unwrap();
    let procedure = ns
        .create_node(SkillFields::new("How to work a task.", &[], 2).into_node("Implementing"))
        .await
        .unwrap();
    ns.create_relationship(&procedure, SKILL_ATTACHED_TO, &queue, json!({}))
        .await
        .unwrap();

    // `task` ships with the path to its project as a context path.
    let run = call(
        &executor,
        "run_query",
        json!({ "query": "All tasks", "with_context": true }),
    )
    .await;
    assert!(!run.is_error, "{}", run.result);
    assert_eq!(run.result["count"], 2);
    assert!(run.result.get("nodes").is_none(), "{}", run.result);
    assert!(run.result.get("limit_reached").is_none(), "{}", run.result);

    let items = run.result["items"].as_array().expect("items");
    assert_eq!(items[0]["title"], "Write the spec");
    assert_eq!(items[0]["checkboxes"][0]["content"], "- [ ] Draft it");
    assert!(items[0]["node_version"].is_i64(), "{}", items[0]);
    assert_eq!(items[1]["title"], "Review the spec");
    for item in items {
        assert_eq!(
            path_entry(item, &["project"])["nodes"][0]["id"],
            format!("nodespace://{}", fixture.project)
        );
        assert!(item["version"].as_str().is_some_and(|v| !v.is_empty()));
        // An item names its skills and what each was reached through; the
        // procedures themselves are listed once, below.
        assert_eq!(
            item["skills"],
            json!([
                {
                    "id": format!("nodespace://{procedure}"),
                    "attached_to": [],
                    "matched_queries": [
                        { "id": format!("nodespace://{queue}"), "title": "All tasks" }
                    ]
                },
                {
                    "id": format!("nodespace://{}", fixture.skill),
                    "attached_to": [format!("nodespace://{}", fixture.project)]
                }
            ])
        );
    }
    assert_ne!(items[0]["version"], items[1]["version"]);

    let skills = run.result["skills"].as_array().expect("skills");
    let names: Vec<&str> = skills.iter().filter_map(|s| s["name"].as_str()).collect();
    assert_eq!(names, ["Implementing", "Standards"]);
    assert!(skills[1]["instructions"]
        .as_str()
        .is_some_and(|body| body.contains("Name things plainly.")));

    // The limit applies to the items.
    let one = call(
        &executor,
        "run_query",
        json!({ "query": "All tasks", "with_context": true, "limit": 1 }),
    )
    .await;
    assert_eq!(one.result["count"], 1);
    assert!(one.result["limit_reached"].is_string(), "{}", one.result);
}

/// Each tool is in the registry with the command that does the same from a
/// shell, its seeded node records that command, and a seeded skill offers it.
#[test]
fn both_tools_are_registered_seeded_and_offered() {
    for (name, command, is_write) in [
        ("get_node_context", "nodespace node context", false),
        ("run_query", "nodespace query run", false),
        ("delete_relationship", "nodespace relationship delete", true),
    ] {
        let tool = Tool::from_name(name).unwrap_or_else(|| panic!("{name} is in the registry"));
        assert_eq!(tool.cli_command(), Some(command));
        assert_eq!(tool.is_write(), is_write, "{name}");
        assert!(!tool.removes_user_data(), "{name}");
        assert!(!tool.duplicate_is_destructive(), "{name}");

        let seeded = seed_tool_nodes()
            .into_iter()
            .find(|seed| seed.title == name)
            .unwrap_or_else(|| panic!("{name} has a seeded tool node"));
        assert_eq!(seeded.root_properties["cli_command"], command);

        assert!(
            SKILL_SEEDS.iter().any(|skill| skill.tools.contains(&name)),
            "no seeded skill offers {name}, so the built-in agent could never call it"
        );
    }
}
