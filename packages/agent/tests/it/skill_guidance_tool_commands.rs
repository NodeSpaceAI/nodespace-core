//! What a fetch by name returns and how the skill list is versioned, against
//! the real seeded skills and tools, with no embedding model.
//!
//! A fetch by task ranks with the embedding model, so its tool commands are
//! covered where that model is loaded (`live_skill_guidance_fetch`). A fetch
//! by name reads the same skill through the same code and needs no model, so
//! the rules for which commands come back are proven here: a tool a skill
//! lists or names is returned with its command, for a built-in skill, an
//! edited one and one a user wrote.

use std::sync::Arc;

use nodespace_agent::local_agent::tools::Tool;
use nodespace_agent::skill_pipeline::{seed_skill_nodes, seed_tool_nodes, SKILL_SEEDS};
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::models::{Node, NodeUpdate, SkillFields};
use nodespace_core::ops::skill_ops::{
    get_skill_guidance, list_skill_guidance, GuidanceSkill, GuidanceToolCommand,
};
use nodespace_core::ops::OpsError;
use nodespace_core::services::{CreateNodeParams, InsertPositionOwned, NodeService};
use serde_json::json;
use tempfile::TempDir;

/// A database seeded with the built-in skills and tools, as the daemon seeds
/// one.
async fn seeded_service() -> (NodeService, TempDir) {
    let temp_dir = TempDir::new().expect("tempdir");
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await.expect("store must open"));
    let node_service = NodeService::new(&mut store)
        .await
        .expect("node service must init");

    let groups: Vec<_> = seed_skill_nodes()
        .iter()
        .chain(seed_tool_nodes().iter())
        .map(|t| prepare_nodes_from_template(t).expect("template must parse"))
        .collect();
    node_service
        .seed_nodes_from_templates(groups)
        .await
        .expect("seeding must succeed");

    (node_service, temp_dir)
}

/// A skill a user wrote: its config, and one paragraph per entry of `body`.
async fn user_skill(
    service: &NodeService,
    name: &str,
    description: &str,
    tools: &[&str],
    body: &[&str],
) -> Node {
    let mut node = SkillFields::new(description, tools, 3).into_node(name);
    node.title = Some(name.to_string());
    service
        .create_node(node.clone())
        .await
        .expect("the skill must create");
    for paragraph in body {
        add_paragraph(service, &node.id, paragraph).await;
    }
    service
        .get_node(&node.id)
        .await
        .expect("the skill must read")
        .expect("the skill exists")
}

async fn add_paragraph(service: &NodeService, parent_id: &str, text: &str) -> String {
    service
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: text.to_string(),
            parent_id: Some(parent_id.to_string()),
            position: InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        })
        .await
        .expect("the paragraph must create")
}

async fn fetch(service: &NodeService, name_or_id: &str) -> GuidanceSkill {
    let mut guidance = get_skill_guidance(service, name_or_id)
        .await
        .unwrap_or_else(|e| panic!("fetching {name_or_id:?} must succeed: {e}"));
    assert_eq!(
        guidance.skills.len(),
        1,
        "a fetch by name returns one skill"
    );
    guidance.skills.remove(0)
}

fn tools_of(skill: &GuidanceSkill) -> Vec<&str> {
    skill
        .tool_commands
        .iter()
        .map(|c| c.tool.as_str())
        .collect()
}

fn command_of<'a>(skill: &'a GuidanceSkill, tool: &str) -> Option<&'a GuidanceToolCommand> {
    skill.tool_commands.iter().find(|c| c.tool == tool)
}

async fn version(service: &NodeService) -> String {
    list_skill_guidance(service)
        .await
        .expect("the listing must succeed")
        .version
}

/// Every built-in skill comes back by its exact name and by its id, with its
/// procedure and the command of every tool it lists that has one, each read
/// from that tool's own seeded node.
#[tokio::test]
async fn a_built_in_skill_is_fetched_by_name_and_by_id_with_its_tool_commands() {
    let (service, _temp) = seeded_service().await;

    for seed in SKILL_SEEDS {
        let by_name = fetch(&service, seed.title).await;
        let by_id = fetch(&service, seed.id).await;
        assert_eq!(
            by_name, by_id,
            "{}: a name and an id fetch one skill",
            seed.title
        );

        assert_eq!(by_name.id, seed.id);
        assert_eq!(by_name.name, seed.title);
        assert_eq!(by_name.use_for, seed.use_for);
        assert!(!by_name.instructions.trim().is_empty(), "{}", seed.title);
        assert_eq!(by_name.confidence, None, "a fetch by name ranks nothing");

        for name in seed.tools {
            let tool = Tool::from_name(name).expect("a seeded skill lists registry tools");
            match (tool.cli_command(), command_of(&by_name, name)) {
                (Some(expected), Some(returned)) => {
                    assert_eq!(returned.command, expected, "{}: {name}", seed.title);
                    assert_eq!(returned.node_id, tool.seed_id(), "{}: {name}", seed.title);
                }
                (None, None) => {}
                (expected, returned) => panic!(
                    "{}: {name} should carry {expected:?}, the fetch returned {returned:?}",
                    seed.title
                ),
            }
        }
    }
}

/// A tool with no CLI command is left out, and the fetch still succeeds.
#[tokio::test]
async fn a_tool_with_no_command_is_left_out_and_the_fetch_succeeds() {
    let (service, _temp) = seeded_service().await;
    assert_eq!(Tool::RouteClarify.cli_command(), None);
    let route_clarify = Tool::RouteClarify.name();

    let skill = fetch(&service, "Node Creation").await;

    assert!(
        SKILL_SEEDS
            .iter()
            .any(|s| s.title == "Node Creation" && s.tools.contains(&route_clarify)),
        "Node Creation lists the tool this test is about"
    );
    assert_eq!(command_of(&skill, route_clarify), None);
    assert!(command_of(&skill, "create_node").is_some());
}

/// A skill a user wrote is served the commands of the built-in tools its body
/// names, and of the ones its tool list holds. A tool is named at word
/// boundaries: one named only inside a longer identifier is not returned.
#[tokio::test]
async fn a_user_written_skill_gets_the_commands_of_the_tools_it_names() {
    let (service, _temp) = seeded_service().await;
    user_skill(
        &service,
        "Recording a Decision",
        "Record a decision and link it to what it decides.",
        &["get_node"],
        &[
            "Use `search_nodes` when you know the decision's name.",
            "Link it with create_relationship, then stop.",
            "Never call update_nodes_from_markdown or xdelete_node here.",
        ],
    )
    .await;

    let skill = fetch(&service, "Recording a Decision").await;

    assert_eq!(
        tools_of(&skill),
        ["create_relationship", "get_node", "search_nodes"],
        "the tools it lists and names, in registry order"
    );
    assert_eq!(
        command_of(&skill, "create_relationship").map(|c| c.command.as_str()),
        Some("nodespace relationship create")
    );
    assert_eq!(
        command_of(&skill, "search_nodes").map(|c| c.command.as_str()),
        Some("nodespace query")
    );
    // The body is served as written: it is not rewritten into commands.
    assert!(
        skill.instructions.contains("create_relationship"),
        "{}",
        skill.instructions
    );
}

/// A built-in skill a user added to is served the commands of the tools their
/// addition names, beside the ones the skill already listed.
#[tokio::test]
async fn a_user_edited_built_in_skill_gets_the_commands_of_the_tools_it_names() {
    let (service, _temp) = seeded_service().await;
    let seed = SKILL_SEEDS
        .iter()
        .find(|s| s.title == "Bulk Import")
        .expect("Bulk Import is seeded");
    assert!(!seed.tools.contains(&"get_related_nodes"));

    let before = fetch(&service, seed.title).await;
    assert_eq!(command_of(&before, "get_related_nodes"), None);

    add_paragraph(
        &service,
        seed.id,
        "After an import, check the links with get_related_nodes.",
    )
    .await;

    let after = fetch(&service, seed.title).await;
    assert_eq!(
        command_of(&after, "get_related_nodes").map(|c| c.command.as_str()),
        Some("nodespace relationship get")
    );
    assert!(command_of(&after, "create_nodes_from_markdown").is_some());
}

/// An archived tool is not offered, and its command is not returned.
#[tokio::test]
async fn an_archived_tools_command_is_not_returned() {
    let (service, _temp) = seeded_service().await;
    let tool = service
        .get_node(Tool::GetNode.seed_id())
        .await
        .expect("the tool must read")
        .expect("get_node is seeded");
    service
        .update_node(
            &tool.id,
            tool.version,
            NodeUpdate::new().with_lifecycle_status("archived".to_string()),
        )
        .await
        .expect("the tool must archive");

    let skill = fetch(&service, "Research & Search").await;

    assert_eq!(command_of(&skill, "get_node"), None);
    assert!(command_of(&skill, "search_nodes").is_some());
}

/// A name no skill has fails with a message naming it. So does a name two
/// skills share, which names the ids to choose between.
#[tokio::test]
async fn an_unknown_or_shared_name_fails_with_a_message_naming_it() {
    let (service, _temp) = seeded_service().await;

    let error = get_skill_guidance(&service, "Writing a Spec")
        .await
        .expect_err("no skill has this name");
    assert!(matches!(error, OpsError::NotFound { .. }), "{error:?}");
    assert!(error.to_string().contains("\"Writing a Spec\""), "{error}");

    // A name is matched exactly: another case is another name.
    get_skill_guidance(&service, "node deletion")
        .await
        .expect_err("a name is case-sensitive");

    let first = user_skill(&service, "House Style", "How we write.", &[], &["Plainly."]).await;
    let second = user_skill(&service, "House Style", "How we write.", &[], &["Briefly."]).await;
    let error = get_skill_guidance(&service, "House Style")
        .await
        .expect_err("two skills share the name");
    let message = error.to_string();
    assert!(message.contains("\"House Style\""), "{message}");
    assert!(
        message.contains(&first.id) && message.contains(&second.id),
        "{message}"
    );
    assert_eq!(
        fetch(&service, &second.id).await.instructions.trim(),
        "Briefly."
    );
}

/// The list's version is the same until a skill changes, and changes when one
/// is added, renamed, redescribed, rewritten, archived or removed.
#[tokio::test]
async fn the_list_version_changes_with_the_skills_and_with_nothing_else() {
    let (service, _temp) = seeded_service().await;

    let initial = version(&service).await;
    assert!(!initial.is_empty());
    assert_eq!(version(&service).await, initial, "nothing changed");

    // A node that is no skill, and no part of one.
    service
        .create_node(Node::new(
            "text".to_string(),
            "An unrelated note".to_string(),
            json!({}),
        ))
        .await
        .expect("the note must create");
    assert_eq!(version(&service).await, initial, "an unrelated node");

    let mut seen = vec![initial];
    let mut changed = |next: String, what: &str| {
        assert!(
            !seen.contains(&next),
            "{what} left the version at an earlier one"
        );
        seen.push(next);
    };

    let skill = user_skill(&service, "House Style", "How we write.", &[], &["Plainly."]).await;
    changed(version(&service).await, "adding a skill");

    let paragraph = add_paragraph(&service, &skill.id, "Briefly.").await;
    changed(version(&service).await, "adding to a skill's body");

    let node = service.get_node(&paragraph).await.unwrap().unwrap();
    service
        .update_node(
            &node.id,
            node.version,
            NodeUpdate::new().with_content("In short sentences.".to_string()),
        )
        .await
        .expect("the paragraph must update");
    changed(version(&service).await, "editing a skill's body");

    let node = service.get_node(&skill.id).await.unwrap().unwrap();
    service
        .update_node(
            &node.id,
            node.version,
            NodeUpdate::new().with_content("Writing Style".to_string()),
        )
        .await
        .expect("the skill must rename");
    changed(version(&service).await, "renaming a skill");

    let node = service.get_node(&skill.id).await.unwrap().unwrap();
    let fields = SkillFields::new("How every document here is written.", &[], 3);
    service
        .update_node(
            &node.id,
            node.version,
            NodeUpdate::new().with_properties(fields.properties()),
        )
        .await
        .expect("the description must update");
    changed(version(&service).await, "changing a skill's description");

    let before_archive = seen.last().cloned().unwrap();
    let node = service.get_node(&skill.id).await.unwrap().unwrap();
    service
        .update_node(
            &node.id,
            node.version,
            NodeUpdate::new().with_lifecycle_status("archived".to_string()),
        )
        .await
        .expect("the skill must archive");
    let archived = version(&service).await;
    assert_ne!(archived, before_archive, "archiving a skill");
    assert_eq!(
        archived, seen[0],
        "the list is what it was before the skill"
    );

    let removed = user_skill(&service, "Short Lived", "Soon gone.", &[], &[]).await;
    let with_it = version(&service).await;
    assert_ne!(with_it, archived, "adding a skill");
    service
        .delete_node(&removed.id, removed.version)
        .await
        .expect("the skill must delete");
    assert_eq!(version(&service).await, archived, "removing a skill");
}
