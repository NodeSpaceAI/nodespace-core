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
    assert_eq!(context["node"]["checkboxes"], json!(["- [ ] Draft it"]));
    assert_eq!(context["paths"][0]["path"], json!(["project"]));
    assert_eq!(context["paths"][0]["count"], 1);
    assert_eq!(
        context["paths"][0]["nodes"][0]["id"],
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

    // With no path the task comes back alone, and it has no skill of its own.
    let alone = call(&executor, "get_node_context", json!({ "id": fixture.task })).await;
    assert_eq!(alone.result["paths"], json!([]));
    assert_eq!(alone.result["skills"], json!([]));
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

/// Each tool is in the registry with the command that does the same from a
/// shell, its seeded node records that command, and a seeded skill offers it.
#[test]
fn both_tools_are_registered_seeded_and_offered() {
    for (name, command, is_write) in [
        ("get_node_context", "nodespace node context", false),
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
