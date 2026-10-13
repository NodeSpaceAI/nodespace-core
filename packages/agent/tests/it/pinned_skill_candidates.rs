//! A pinned skill as the routing candidate a turn is given (ADR-090 §5),
//! against the real seeded skills, with no embedding model: a pin chooses the
//! skill, so nothing is ranked.

use std::sync::Arc;

use nodespace_agent::local_agent::routing;
use nodespace_agent::local_agent::tools::pinned_skill_candidates;
use nodespace_agent::skill_pipeline::{
    link_seeded_skills, seed_skill_nodes, seed_tool_nodes, PLAY_AUTHORING_SKILL_ID,
    SCHEMA_CHANGING_SKILL_ID, SKILL_SEEDS,
};
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
use serde_json::json;
use tempfile::TempDir;

/// A database seeded with the built-in skills and tools and their schema
/// links, as the daemon seeds one.
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
    link_seeded_skills(&node_service)
        .await
        .expect("linking must succeed");

    (node_service, temp_dir)
}

/// The play-authoring skill, pinned, is a candidate with its procedure, its
/// tools and the `play` schema it links to, and it is eligible with no score.
#[tokio::test]
async fn the_pinned_play_authoring_skill_carries_its_instructions_and_schema() {
    let (service, _dir) = seeded_service().await;

    let candidates = pinned_skill_candidates(&service, &[PLAY_AUTHORING_SKILL_ID.to_string()])
        .await
        .expect("the pinned skill must load");

    assert_eq!(candidates.len(), 1);
    let authoring = &candidates[0];
    assert_eq!(authoring.id, PLAY_AUTHORING_SKILL_ID);
    assert_eq!(authoring.name, "Play Authoring");
    assert!(authoring.pinned);
    assert_eq!(authoring.score, 0.0);
    assert!(authoring.tools.contains(&"update_play".to_string()));
    assert!(!authoring.instructions.trim().is_empty());
    assert!(authoring.schemas_linked);

    // A pin does not clear a destructive skill's bar, so a data-removing tool
    // on this skill's whitelist would unbind every play edit chat.
    assert!(!routing::skill_is_destructive(authoring));
    assert!(routing::clears_score_gate(authoring));
    assert_eq!(
        routing::offered_types(&candidates),
        Some(vec!["play".to_string()])
    );
    let block = routing::render_candidates_for_prompt(&candidates).expect("a candidate block");
    assert!(block.contains("Play Authoring"), "{block}");
    assert!(block.contains(authoring.instructions.trim()), "{block}");
}

/// Only the skills among the pinned ids come back, in the order pinned: a
/// pinned node of another type, and an id that names nothing, are not skills.
#[tokio::test]
async fn only_the_skills_among_the_pinned_ids_are_candidates() {
    let (service, _dir) = seeded_service().await;
    let note_id = service
        .create_node(Node::new(
            "text".to_string(),
            "A note".to_string(),
            json!({}),
        ))
        .await
        .expect("the note must create");
    let other = SKILL_SEEDS
        .iter()
        .find(|seed| seed.id != PLAY_AUTHORING_SKILL_ID)
        .expect("another built-in skill");

    let candidates = pinned_skill_candidates(
        &service,
        &[
            note_id,
            other.id.to_string(),
            "no-such-node".to_string(),
            PLAY_AUTHORING_SKILL_ID.to_string(),
        ],
    )
    .await
    .expect("the pinned skills must load");

    assert_eq!(
        candidates.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        [other.id, PLAY_AUTHORING_SKILL_ID]
    );
    // A skill that links to no schema carries none: there is no query to
    // guess a relevant type from.
    assert!(!candidates[0].schemas_linked);
    assert_eq!(candidates[0].schema_metadata, json!([]));
}

/// The type skill, pinned, reaches the turn as a candidate with its
/// procedure and the tool that changes a type, and clears the gate with no
/// score. Nothing it lists removes user data, so a pin is enough: a write
/// that would is the product's to confirm, not the skill's.
#[tokio::test]
async fn the_pinned_type_skill_is_a_candidate_that_clears_the_gate() {
    let (service, _dir) = seeded_service().await;

    let candidates = pinned_skill_candidates(&service, &[SCHEMA_CHANGING_SKILL_ID.to_string()])
        .await
        .expect("the pinned skill must load");

    assert_eq!(candidates.len(), 1);
    let skill = &candidates[0];
    assert_eq!(skill.name, "Understanding and Changing a Type");
    assert!(skill.pinned);
    assert!(skill.tools.contains(&"get_node".to_string()));
    assert!(skill.tools.contains(&"update_schema".to_string()));
    assert!(!routing::skill_is_destructive(skill));
    assert!(routing::clears_score_gate(skill));

    // What the model reads: the procedure, with the shared rules it includes
    // resolved, and the field lookup the issue asks for.
    let block = routing::render_candidates_for_prompt(&candidates).expect("a candidate block");
    assert!(block.contains("SAY WHICH CASE THE TYPE IS IN"), "{block}");
    assert!(block.contains("field_references"), "{block}");
    assert!(
        !block.contains("<!-- include"),
        "an include marker was left unresolved: {block}"
    );
    assert!(block.contains("add_field_values"), "{block}");
}

/// A chat that pins nothing reads nothing.
#[tokio::test]
async fn no_pinned_ids_yield_no_candidates() {
    let (service, _dir) = seeded_service().await;

    let candidates = pinned_skill_candidates(&service, &[])
        .await
        .expect("an empty read must succeed");

    assert!(candidates.is_empty());
}
