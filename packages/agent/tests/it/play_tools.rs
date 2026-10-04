//! The play-authoring tools (`get_play`, `update_play`) and the seeded skill
//! that whitelists them (ADR-090 §6), through the production
//! `GraphToolExecutor::execute` surface against a real store.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_agent::skill_pipeline::{link_seeded_skills, seed_skill_nodes, SKILL_SEEDS};
use nodespace_agent::{AgentToolExecutor, ToolResult};
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::models::{Node, PlayFields, PlaySuspensionReason, SKILL_APPLIES_TO};
use nodespace_core::schema::handle_create_schema;
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

/// One rule on `story`: two conditions, the first walking to the story's
/// epic, and one action.
fn rules() -> Value {
    json!([{
        "name": "close epic",
        "description": "Close an epic when one of its stories is done",
        "trigger": {
            "type": "graph_event",
            "on": "property_changed",
            "select": { "target_type": "story" },
            "property_key": "story.state"
        },
        "conditions": [
            { "expr": "node.epic.state != 'done'", "description": "The epic is still open" },
            { "expr": "node.state == 'done'", "description": "The story is done" }
        ],
        "actions": [{
            "action_type": "update_node",
            "description": "Mark the epic done",
            "params": { "node_id": "{trigger.node.epic.id}", "properties": { "state": "done" } }
        }]
    }])
}

/// A play holding [`rules`], over an `epic` and a `story` type.
async fn seed_play(ns: &Arc<NodeService>) -> String {
    for schema in [
        json!({ "name": "Epic", "fields": [{ "name": "state", "type": "text" }] }),
        json!({
            "name": "Story",
            "fields": [{ "name": "state", "type": "text" }],
            "relationships": [{
                "name": "epic", "targetType": "epic", "direction": "out",
                "cardinality": "one", "reverseName": "stories", "reverseCardinality": "many"
            }]
        }),
    ] {
        handle_create_schema(ns, schema)
            .await
            .expect("the test's schema is created");
    }
    ns.create_node(Node::new(
        "play".to_string(),
        "Epic roll-up".to_string(),
        json!({ "description": "Closes epics", "rules": rules() }),
    ))
    .await
    .expect("the play is created")
}

async fn call(executor: &GraphToolExecutor, tool: &str, args: Value) -> ToolResult {
    executor
        .execute(tool, args)
        .await
        .unwrap_or_else(|e| panic!("{tool} must return a tool result: {e}"))
}

async fn stored(ns: &NodeService, play_id: &str) -> (Node, PlayFields) {
    let node = ns
        .get_node(play_id)
        .await
        .unwrap()
        .expect("the play exists");
    let fields = PlayFields::from_node(&node).expect("the stored play decodes");
    (node, fields)
}

fn error_text(result: &ToolResult) -> &str {
    assert!(result.is_error, "expected a tool error: {}", result.result);
    result.result["error"].as_str().expect("an error message")
}

#[tokio::test]
async fn get_play_returns_the_typed_play_and_the_schemas_its_rules_reference() {
    let (executor, ns, _tmp) = make_executor().await;
    let play_id = seed_play(&ns).await;

    let got = call(&executor, "get_play", json!({ "id": play_id })).await;
    assert!(!got.is_error, "{}", got.result);
    let play = &got.result;

    assert_eq!(play["id"], format!("nodespace://{play_id}"));
    assert_eq!(play["title"], "Epic roll-up");
    assert_eq!(play["description"], "Closes epics");
    assert_eq!(play["enabled"], true);
    assert!(play.get("suspended").is_none(), "{play}");
    // The rules come back in the shape `update_play` takes, descriptions
    // included.
    assert_eq!(play["rules"][0]["name"], "close epic");
    assert_eq!(
        play["rules"][0]["conditions"][0],
        json!({ "expr": "node.epic.state != 'done'", "description": "The epic is still open" })
    );
    assert_eq!(
        play["rules"][0]["actions"][0]["description"],
        "Mark the epic done"
    );

    // `story` is the trigger's type; `epic` is reached only through a
    // condition path and a binding. Each carries its fields and relationships.
    let schemas = play["schemas"].as_array().expect("schemas");
    let ids: Vec<&str> = schemas
        .iter()
        .map(|s| s["type_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["story", "epic"]);
    let story = serde_json::to_string(&schemas[0]).unwrap();
    assert!(story.contains("\"state\""), "story's field: {story}");
    assert!(story.contains("\"epic\""), "story's relationship: {story}");
}

#[tokio::test]
async fn get_play_reports_the_engines_suspension() {
    let (executor, ns, _tmp) = make_executor().await;
    let play_id = seed_play(&ns).await;
    ns.record_play_suspension(
        &play_id,
        PlaySuspensionReason::ActionFailed,
        "update_node failed: no such node",
    )
    .await
    .unwrap();

    let got = call(&executor, "get_play", json!({ "id": play_id })).await;
    assert_eq!(got.result["suspended"]["reason"], "action_failed");
    assert_eq!(
        got.result["suspended"]["message"],
        "update_node failed: no such node"
    );
    assert!(got.result["suspended"]["at"].is_string());
}

#[tokio::test]
async fn the_play_tools_refuse_a_node_that_is_not_a_play() {
    let (executor, ns, _tmp) = make_executor().await;
    let text = ns
        .create_node(Node::new(
            "text".to_string(),
            "A note".to_string(),
            json!({}),
        ))
        .await
        .unwrap();

    for (tool, args) in [
        ("get_play", json!({ "id": text })),
        ("update_play", json!({ "id": text, "enabled": false })),
    ] {
        let result = call(&executor, tool, args).await;
        assert!(
            error_text(&result).contains("is a text node, not a play"),
            "{tool}: {}",
            result.result
        );
    }
    let missing = call(&executor, "get_play", json!({ "id": "no-such-play" })).await;
    assert!(error_text(&missing).contains("no node found"));
}

#[tokio::test]
async fn update_play_writes_changed_rules_and_returns_the_updated_play() {
    let (executor, ns, _tmp) = make_executor().await;
    let play_id = seed_play(&ns).await;

    let mut changed = rules();
    changed[0]["conditions"][1] =
        json!({ "expr": "node.state == 'shipped'", "description": "The story has shipped" });
    let updated = call(
        &executor,
        "update_play",
        json!({ "id": format!("nodespace://{play_id}"), "rules": changed, "description": "Closes shipped epics" }),
    )
    .await;

    assert!(!updated.is_error, "{}", updated.result);
    assert_eq!(updated.result["id"], format!("nodespace://{play_id}"));
    assert_eq!(updated.result["description"], "Closes shipped epics");
    assert_eq!(
        updated.result["rules"][0]["conditions"][1]["expr"],
        "node.state == 'shipped'"
    );

    let (_, fields) = stored(&ns, &play_id).await;
    assert_eq!(
        fields.rules[0].conditions[1].expr,
        "node.state == 'shipped'"
    );
    assert_eq!(
        fields.rules[0].conditions[1].description,
        "The story has shipped"
    );
    assert_eq!(fields.description.as_deref(), Some("Closes shipped epics"));
}

/// Each rejection is a tool result the model can repair from: it names the
/// rule, the component and its index, and for a path the name that is wrong.
/// The play is left as it was.
#[tokio::test]
async fn a_rejected_write_names_the_rule_the_component_and_the_field() {
    let (executor, ns, _tmp) = make_executor().await;
    let play_id = seed_play(&ns).await;
    let (before, _) = stored(&ns, &play_id).await;

    // A changed expression that keeps its stored description.
    let mut stale = rules();
    stale[0]["conditions"][1]["expr"] = json!("node.state == 'shipped'");
    // A path through a relationship `story` does not declare.
    let mut broken = rules();
    broken[0]["conditions"][0] =
        json!({ "expr": "node.sprint.state == 'open'", "description": "The sprint is open" });
    // A condition written as a bare expression.
    let mut bare = rules();
    bare[0]["conditions"][1] = json!("node.state == 'done'");
    // An action missing its description.
    let mut undescribed = rules();
    undescribed[0]["actions"][0]
        .as_object_mut()
        .unwrap()
        .remove("description");

    for (what, rules, expected) in [
        (
            "a stale description",
            stale,
            vec![
                "rule `close epic`, condition 2",
                "its expression changed and its description didn't",
            ],
        ),
        (
            "a broken path",
            broken,
            vec!["rule[0].condition[0]", "node.sprint.state", "'sprint'"],
        ),
        (
            "a bare condition",
            bare,
            vec!["rule[0] ('close epic')", "conditions[1]", "expr"],
        ),
        (
            "a missing description",
            undescribed,
            vec!["rule[0] ('close epic')", "actions[0]", "description"],
        ),
    ] {
        let result = call(
            &executor,
            "update_play",
            json!({ "id": play_id, "rules": rules }),
        )
        .await;
        let message = error_text(&result);
        assert!(
            message.starts_with("The play was not changed: "),
            "{what}: {message}"
        );
        for part in expected {
            assert!(
                message.contains(part),
                "{what} should name {part:?}: {message}"
            );
        }
        let (after, _) = stored(&ns, &play_id).await;
        assert_eq!(after.version, before.version, "{what} must write nothing");
    }
}

#[tokio::test]
async fn update_play_sets_the_switch_and_leaves_the_rules() {
    let (executor, ns, _tmp) = make_executor().await;
    let play_id = seed_play(&ns).await;

    let off = call(
        &executor,
        "update_play",
        json!({ "id": play_id, "enabled": false }),
    )
    .await;
    assert!(!off.is_error, "{}", off.result);
    assert_eq!(off.result["enabled"], false);
    let (_, fields) = stored(&ns, &play_id).await;
    assert!(!fields.enabled);
    assert_eq!(
        serde_json::to_value(&fields.rules).unwrap()[0]["name"],
        "close epic"
    );

    let on = call(
        &executor,
        "update_play",
        json!({ "id": play_id, "enabled": true }),
    )
    .await;
    assert_eq!(on.result["enabled"], true);
    assert!(stored(&ns, &play_id).await.1.enabled);
}

/// The tool writes the typed play update and nothing else: a call naming the
/// lifecycle or a suspension field is refused whole, even when it also names
/// something the update does take.
#[tokio::test]
async fn update_play_never_writes_the_lifecycle_or_the_suspension() {
    let (executor, ns, _tmp) = make_executor().await;
    let play_id = seed_play(&ns).await;
    let (before, _) = stored(&ns, &play_id).await;

    for field in [
        "lifecycle_status",
        "lifecycleStatus",
        "suspended_reason",
        "suspended_message",
        "suspended_at",
        "properties",
    ] {
        let result = call(
            &executor,
            "update_play",
            json!({ "id": play_id, "enabled": false, field: "archived" }),
        )
        .await;
        let message = error_text(&result);
        assert!(message.contains(field), "{field}: {message}");

        let (after, fields) = stored(&ns, &play_id).await;
        assert_eq!(after.version, before.version, "{field} must write nothing");
        assert_eq!(after.lifecycle_status, before.lifecycle_status);
        assert!(
            fields.enabled,
            "{field}: the refused call's switch is not written"
        );
        assert!(fields.suspended_at.is_none());
    }

    let empty = call(&executor, "update_play", json!({ "id": play_id })).await;
    assert!(error_text(&empty).starts_with("Nothing to change"));
}

/// The skill is seeded under its fixed id and linked `applies_to → play`,
/// and the link is made once however many times the database is opened.
#[tokio::test]
async fn the_play_authoring_skill_is_seeded_and_linked_to_the_play_schema() {
    let (_executor, ns, _tmp) = make_executor().await;
    let seed = SKILL_SEEDS
        .iter()
        .find(|seed| seed.title == "Play Authoring")
        .expect("Play Authoring is a built-in skill");
    assert_eq!(seed.id, "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c0c");
    assert_eq!(seed.applies_to, ["play"]);
    for tool in ["get_play", "update_play"] {
        assert!(seed.tools.contains(&tool), "the skill whitelists {tool}");
    }

    for _open in 0..2 {
        let groups: Vec<_> = seed_skill_nodes()
            .iter()
            .map(|tmpl| prepare_nodes_from_template(tmpl).expect("the seed parses"))
            .collect();
        ns.seed_nodes_from_templates(groups).await.unwrap();
        link_seeded_skills(&ns).await.unwrap();
    }

    let node = ns
        .get_node(seed.id)
        .await
        .unwrap()
        .expect("the skill is seeded");
    assert_eq!(node.node_type, "skill");
    assert_eq!(node.content, "Play Authoring");

    let links = ns
        .store()
        .get_edge_targets_by_source(&[seed.id.to_string()], SKILL_APPLIES_TO)
        .await
        .unwrap();
    assert_eq!(links.get(seed.id), Some(&vec!["play".to_string()]));

    // No other built-in links to a schema.
    let others: Vec<String> = SKILL_SEEDS
        .iter()
        .filter(|s| s.id != seed.id)
        .map(|s| s.id.to_string())
        .collect();
    let other_links = ns
        .store()
        .get_edge_targets_by_source(&others, SKILL_APPLIES_TO)
        .await
        .unwrap();
    assert!(other_links.is_empty(), "{other_links:?}");
}
