//! A shipped change to a seed the user has edited is recorded as pending and
//! put to them, never applied on its own (ADR-094 §8, ADR-072).
//!
//! Each test seeds a template, edits the node the way a user would, then
//! seeds a changed template: what a later release's reconciliation does on
//! the next database open.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::{prepare_nodes_from_template, NodeTemplate, SeedTier};
use nodespace_core::models::{NodeUpdate, SeedAspect, SkillFields};
use nodespace_core::playbook::core_plays::{
    core_play_templates, parent_task_completion_rules, seed_core_plays,
    PARENT_TASK_COMPLETION_PLAY_ID,
};
use nodespace_core::services::{
    CreateNodeParams, InsertPosition, InsertPositionOwned, NodeService,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const SKILL_ID: &str = "0f1d6c4e-8a3b-4c57-9e21-5b7a3d9c1e01";
const PLAY_ID: &str = "0f1d6c4e-8a3b-4c57-9e21-5b7a3d9c1e02";
const QUERY_ID: &str = "0f1d6c4e-8a3b-4c57-9e21-5b7a3d9c1e03";

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

fn skill(use_for: &str, body: &str) -> NodeTemplate {
    NodeTemplate::skill(
        SKILL_ID,
        "Filing Notes",
        SkillFields {
            use_for: use_for.to_string(),
            ..SkillFields::default()
        },
        body,
    )
}

fn play(description: &str) -> NodeTemplate {
    let rules = parent_task_completion_rules();
    NodeTemplate {
        id: PLAY_ID.to_string(),
        title: "Roll a parent task up".to_string(),
        markdown_content: String::new(),
        root_node_type: "play".to_string(),
        root_properties: json!({
            "rules": rules,
            "enabled": true,
            "description": description,
            "_seed": { "default_rules": rules },
        }),
        child_node_type: None,
        tier: SeedTier::System,
    }
}

fn saved_query(limit: u32) -> NodeTemplate {
    NodeTemplate {
        id: QUERY_ID.to_string(),
        title: "Open tasks".to_string(),
        markdown_content: String::new(),
        root_node_type: "query".to_string(),
        root_properties: json!({
            "target_type": "task",
            "filters": [],
            "generated_by": "user",
            "limit": limit,
        }),
        child_node_type: None,
        tier: SeedTier::System,
    }
}

/// What a database open does with a seed table holding `template`.
async fn seed(service: &NodeService, template: &NodeTemplate) -> Result<()> {
    let nodes = prepare_nodes_from_template(template)?;
    service.seed_nodes_from_templates(vec![nodes]).await?;
    Ok(())
}

/// The fingerprint `template` ships for `aspect`.
fn shipped_version(template: &NodeTemplate, aspect: SeedAspect) -> String {
    let nodes = prepare_nodes_from_template(template).unwrap();
    nodes[0].properties["_seed"][aspect.version_key()]
        .as_str()
        .unwrap()
        .to_string()
}

/// Edit the seed's root the way a user does.
async fn edit_root(service: &NodeService, id: &str, properties: serde_json::Value) -> Result<()> {
    let node = service.get_node(id).await?.expect("the seed exists");
    service
        .update_node(
            id,
            node.version,
            NodeUpdate::default().with_properties(properties),
        )
        .await?;
    Ok(())
}

/// Edit the first line of the seed's body the way a user does.
async fn edit_body(service: &NodeService, id: &str, content: &str) -> Result<()> {
    let children = service.get_children(id).await?;
    let first = children.first().expect("the seed has a body");
    service
        .update_node(
            &first.id,
            first.version,
            NodeUpdate::default().with_content(content.to_string()),
        )
        .await?;
    Ok(())
}

async fn body(service: &NodeService, id: &str) -> Result<Vec<String>> {
    Ok(service
        .get_children(id)
        .await?
        .into_iter()
        .map(|child| child.content)
        .collect())
}

/// Write a line of the user's own under `parent`, the way the app, the CLI
/// or an agent creates one: not through an edit of an existing node.
async fn write_line(service: &NodeService, parent: Option<&str>, content: &str) -> Result<String> {
    Ok(service
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: content.to_string(),
            properties: json!({}),
            parent_id: parent.map(ToString::to_string),
            position: InsertPositionOwned::End,
            lifecycle_status: None,
        })
        .await?)
}

/// Move `id` to the end of `new_parent`'s children, the way a user drags it.
async fn move_under(service: &NodeService, id: &str, new_parent: &str) -> Result<()> {
    let node = service.get_node(id).await?.expect("the node exists");
    service
        .move_node(id, node.version, Some(new_parent), InsertPosition::End)
        .await?;
    Ok(())
}

/// The order values on the edges to the seed's lines, smallest first.
async fn line_orders(service: &NodeService) -> Result<Vec<f64>> {
    let (_, edges) = service
        .store()
        .get_subtree_with_relationships(SKILL_ID)
        .await?;
    let mut orders: Vec<f64> = edges
        .iter()
        .filter(|edge| edge.in_node == SKILL_ID)
        .map(|edge| edge.order())
        .collect();
    orders.sort_by(f64::total_cmp);
    Ok(orders)
}

/// The guidance of the seed is pending, and nothing else is.
async fn assert_only_guidance_pending(service: &NodeService, shipped: &NodeTemplate) -> Result<()> {
    let pending = service.list_pending_seed_updates().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].node_id, SKILL_ID);
    assert_eq!(pending[0].aspect, SeedAspect::Guidance);
    assert_eq!(
        pending[0].shipped_version,
        shipped_version(shipped, SeedAspect::Guidance)
    );
    Ok(())
}

#[tokio::test]
async fn an_edited_skill_body_is_kept_and_its_shipped_change_recorded() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    edit_body(&service, SKILL_ID, "The body the user wrote.").await?;

    let v2 = skill("File a note.", "Shipped body, second version.");
    seed(&service, &v2).await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["The body the user wrote."]
    );
    let pending = service.list_pending_seed_updates().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].node_id, SKILL_ID);
    assert_eq!(pending[0].node_type, "skill");
    assert_eq!(pending[0].title, "Filing Notes");
    assert_eq!(pending[0].aspect, SeedAspect::Guidance);
    assert_eq!(
        pending[0].shipped_version,
        shipped_version(&v2, SeedAspect::Guidance)
    );

    // The shipped version beside the user's.
    let comparison = service
        .compare_pending_seed_update(&prepare_nodes_from_template(&v2)?, SeedAspect::Guidance)
        .await?
        .expect("the guidance is pending");
    assert_eq!(comparison.shipped, "Shipped body, second version.");
    assert_eq!(comparison.yours, "The body the user wrote.");
    Ok(())
}

#[tokio::test]
async fn keeping_theirs_stays_settled_until_the_shipped_version_changes_again() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    edit_body(&service, SKILL_ID, "The body the user wrote.").await?;
    let v2 = skill("File a note.", "Shipped body, second version.");
    seed(&service, &v2).await?;

    assert!(
        service
            .keep_seed_update(SKILL_ID, SeedAspect::Guidance)
            .await?
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["The body the user wrote."]
    );

    // The next opens ship the same version: nothing comes back.
    seed(&service, &v2).await?;
    seed(&service, &v2).await?;
    assert!(service.list_pending_seed_updates().await?.is_empty());
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["The body the user wrote."]
    );

    // A newer shipped version is put to the user again.
    let v3 = skill("File a note.", "Shipped body, third version.");
    seed(&service, &v3).await?;
    let pending = service.list_pending_seed_updates().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(
        pending[0].shipped_version,
        shipped_version(&v3, SeedAspect::Guidance)
    );
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["The body the user wrote."]
    );

    // Nothing to keep twice.
    assert!(
        service
            .keep_seed_update(SKILL_ID, SeedAspect::Guidance)
            .await?
    );
    assert!(
        !service
            .keep_seed_update(SKILL_ID, SeedAspect::Guidance)
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn taking_the_shipped_settings_of_a_skill_replaces_only_that_aspect() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(&service, &skill("File a note.", "Shipped body.")).await?;
    edit_root(
        &service,
        SKILL_ID,
        json!({ "use_for": "The user's description." }),
    )
    .await?;
    edit_body(&service, SKILL_ID, "The body the user wrote.").await?;

    // Only the settings change in the next release.
    let v2 = skill("File a note in the right collection.", "Shipped body.");
    seed(&service, &v2).await?;

    let pending = service.list_pending_seed_updates().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].aspect, SeedAspect::Config);
    let stored = service.get_node(SKILL_ID).await?.unwrap();
    assert_eq!(
        stored.properties["skill"]["use_for"],
        "The user's description."
    );

    let group = prepare_nodes_from_template(&v2)?;
    let comparison = service
        .compare_pending_seed_update(&group, SeedAspect::Config)
        .await?
        .expect("the config is pending");
    assert!(
        comparison
            .shipped
            .contains("File a note in the right collection."),
        "{}",
        comparison.shipped
    );
    assert!(
        comparison.yours.contains("The user's description."),
        "{}",
        comparison.yours
    );
    assert!(!comparison.yours.contains("_seed"), "{}", comparison.yours);

    assert!(service.take_seed_update(&group, SeedAspect::Config).await?);

    let stored = service.get_node(SKILL_ID).await?.unwrap();
    assert_eq!(
        stored.properties["skill"]["use_for"],
        "File a note in the right collection."
    );
    assert_eq!(stored.properties["_seed"]["config_modified"], false);
    assert!(service.list_pending_seed_updates().await?.is_empty());
    // The body is the user's still, and still theirs to keep.
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["The body the user wrote."]
    );
    assert_eq!(stored.properties["_seed"]["guidance_modified"], true);

    // Settled: the same release leaves it alone, and the next one replaces
    // the settings without asking, since they are no longer the user's.
    seed(&service, &v2).await?;
    assert!(service.list_pending_seed_updates().await?.is_empty());
    seed(
        &service,
        &skill("File a note, third wording.", "Shipped body."),
    )
    .await?;
    let stored = service.get_node(SKILL_ID).await?.unwrap();
    assert_eq!(
        stored.properties["skill"]["use_for"],
        "File a note, third wording."
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_edited_play_is_kept_and_its_shipped_change_can_be_taken() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(&service, &play("Shipped description, first version.")).await?;
    edit_root(&service, PLAY_ID, json!({ "enabled": false })).await?;

    let v2 = play("Shipped description, second version.");
    seed(&service, &v2).await?;

    let stored = service.get_node(PLAY_ID).await?.unwrap();
    assert_eq!(
        stored.properties["play"]["description"],
        "Shipped description, first version."
    );
    let pending = service.list_pending_seed_updates().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].node_type, "play");
    assert_eq!(pending[0].aspect, SeedAspect::Config);

    let group = prepare_nodes_from_template(&v2)?;
    assert!(service.take_seed_update(&group, SeedAspect::Config).await?);
    let stored = service.get_node(PLAY_ID).await?.unwrap();
    assert_eq!(
        stored.properties["play"]["description"],
        "Shipped description, second version."
    );
    // Taking replaces the user's edit itself: the play they switched off is
    // the shipped play again, and no longer theirs.
    assert_eq!(stored.properties["play"]["enabled"], true);
    assert_eq!(stored.properties["_seed"]["config_modified"], false);
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

/// A shipped change that leaves a play's rules alone does not put a
/// suspended play back in service: the template naming `enabled` is not a
/// write of it while the play is already on.
#[tokio::test]
async fn a_shipped_description_change_leaves_a_suspended_play_suspended() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(&service, &play("Shipped description, first version.")).await?;
    service
        .record_play_suspension(
            PLAY_ID,
            nodespace_core::models::PlaySuspensionReason::ActionFailed,
            "boom",
        )
        .await?;

    seed(&service, &play("Shipped description, second version.")).await?;

    let stored = service.get_node(PLAY_ID).await?.unwrap();
    assert_eq!(
        stored.properties["play"]["description"],
        "Shipped description, second version."
    );
    assert_eq!(
        stored.properties["play"]["suspended_reason"],
        "action_failed"
    );
    Ok(())
}

/// A shipped change to a play's rules reaches a database whose play nobody
/// edited, and the default a reset restores moves with it.
#[tokio::test]
async fn a_shipped_rules_change_replaces_an_unedited_play() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(&service, &play("Shipped description.")).await?;

    let mut v2 = play("Shipped description.");
    let mut rules = parent_task_completion_rules();
    rules[0]["description"] = json!("Mark a task done once its sub-tasks are finished");
    v2.root_properties["rules"] = rules.clone();
    v2.root_properties["_seed"]["default_rules"] = rules.clone();
    seed(&service, &v2).await?;

    let stored = service.get_node(PLAY_ID).await?.unwrap();
    assert_eq!(stored.properties["play"]["rules"], rules);
    assert_eq!(stored.properties["_seed"]["default_rules"], rules);
    assert_eq!(stored.properties["play"]["enabled"], true);
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

/// The roll-up Play as the release before ADR-079 §8 shipped it, under its
/// real id: every child of the parent was asked for a status.
fn roll_up_play_of_the_previous_release() -> NodeTemplate {
    let mut template = core_play_templates()
        .into_iter()
        .find(|play| play.id == PARENT_TASK_COMPLETION_PLAY_ID)
        .expect("the roll-up is a core play");
    let mut rules = parent_task_completion_rules();
    rules[0]["conditions"][0]["expr"] =
        json!("node.child_of.has_child.all(c, c.status == 'done' || c.status == 'cancelled')");
    template.root_properties["rules"] = rules.clone();
    template.root_properties["_seed"]["default_rules"] = rules;
    template
}

/// A database opened by this release after the previous one: the roll-up
/// nobody edited takes the shipped condition, live and as its reset target.
#[tokio::test]
async fn the_changed_roll_up_replaces_one_nobody_edited() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    let previous = roll_up_play_of_the_previous_release();
    seed(&service, &previous).await?;
    let stored = service
        .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
        .await?
        .unwrap();
    assert_eq!(
        stored.properties["play"]["rules"],
        previous.root_properties["rules"]
    );

    seed_core_plays(&service).await?;

    let shipped = parent_task_completion_rules();
    let stored = service
        .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
        .await?
        .unwrap();
    assert_eq!(stored.properties["play"]["rules"], shipped);
    assert_eq!(stored.properties["_seed"]["default_rules"], shipped);
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

/// The same open where the user had edited the roll-up: their Play is kept,
/// and the shipped condition is put to them to take (ADR-072).
#[tokio::test]
async fn the_changed_roll_up_is_put_to_a_user_who_edited_theirs() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    let previous = roll_up_play_of_the_previous_release();
    seed(&service, &previous).await?;
    edit_root(
        &service,
        PARENT_TASK_COMPLETION_PLAY_ID,
        json!({ "description": "A description the user wrote." }),
    )
    .await?;

    seed_core_plays(&service).await?;

    let stored = service
        .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
        .await?
        .unwrap();
    assert_eq!(
        stored.properties["play"]["description"],
        "A description the user wrote."
    );
    assert_eq!(
        stored.properties["play"]["rules"],
        previous.root_properties["rules"]
    );
    let pending = service.list_pending_seed_updates().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].aspect, SeedAspect::Config);

    let shipped = core_play_templates()
        .into_iter()
        .find(|play| play.id == PARENT_TASK_COMPLETION_PLAY_ID)
        .expect("the roll-up is a core play");
    let group = prepare_nodes_from_template(&shipped)?;
    assert!(service.take_seed_update(&group, SeedAspect::Config).await?);
    let stored = service
        .get_node(PARENT_TASK_COMPLETION_PLAY_ID)
        .await?
        .unwrap();
    assert_eq!(
        stored.properties["play"]["rules"],
        parent_task_completion_rules()
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn an_edited_saved_query_is_kept_and_its_shipped_change_recorded() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(&service, &saved_query(10)).await?;
    edit_root(&service, QUERY_ID, json!({ "limit": 3 })).await?;

    let v2 = saved_query(25);
    seed(&service, &v2).await?;

    let stored = service.get_node(QUERY_ID).await?.unwrap();
    assert_eq!(stored.properties["query"]["limit"], 3);
    let pending = service.list_pending_seed_updates().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].node_type, "query");
    assert_eq!(pending[0].title, "Open tasks");
    assert_eq!(pending[0].aspect, SeedAspect::Config);

    assert!(
        service
            .keep_seed_update(QUERY_ID, SeedAspect::Config)
            .await?
    );
    seed(&service, &v2).await?;
    assert!(service.list_pending_seed_updates().await?.is_empty());
    let stored = service.get_node(QUERY_ID).await?.unwrap();
    assert_eq!(stored.properties["query"]["limit"], 3);
    Ok(())
}

#[tokio::test]
async fn a_seed_nobody_edited_is_replaced_without_asking() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;

    seed(
        &service,
        &skill("File a note, reworded.", "Shipped body, second version."),
    )
    .await?;

    let stored = service.get_node(SKILL_ID).await?.unwrap();
    assert_eq!(
        stored.properties["skill"]["use_for"],
        "File a note, reworded."
    );
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped body, second version."]
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_reset_clears_the_pending_state_of_what_it_resets() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    edit_root(
        &service,
        SKILL_ID,
        json!({ "use_for": "The user's description." }),
    )
    .await?;
    edit_body(&service, SKILL_ID, "The body the user wrote.").await?;

    let v2 = skill("File a note, reworded.", "Shipped body, second version.");
    seed(&service, &v2).await?;
    assert_eq!(service.list_pending_seed_updates().await?.len(), 2);

    // Resetting the body leaves the settings pending.
    let group = prepare_nodes_from_template(&v2)?;
    assert_eq!(
        service.reset_seed_node(&group, false, true).await?,
        (false, true)
    );
    let pending = service.list_pending_seed_updates().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].aspect, SeedAspect::Config);
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped body, second version."]
    );

    assert_eq!(
        service.reset_seed_node(&group, true, false).await?,
        (true, false)
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn taking_changes_nothing_when_nothing_is_pending() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    let v1 = skill("File a note.", "Shipped body.");
    seed(&service, &v1).await?;
    edit_body(&service, SKILL_ID, "The body the user wrote.").await?;

    // Edited, but what ships has not changed: there is no choice to make.
    seed(&service, &v1).await?;
    assert!(service.list_pending_seed_updates().await?.is_empty());
    let group = prepare_nodes_from_template(&v1)?;
    assert!(
        !service
            .take_seed_update(&group, SeedAspect::Guidance)
            .await?
    );
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["The body the user wrote."]
    );
    assert!(service
        .compare_pending_seed_update(&group, SeedAspect::Guidance)
        .await?
        .is_none());
    Ok(())
}

/// The pending record is bookkeeping beside the graph: no node carries it,
/// and it goes when its seed does.
#[tokio::test]
async fn pending_state_is_not_node_content() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    edit_body(&service, SKILL_ID, "The body the user wrote.").await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, second version."),
    )
    .await?;
    assert_eq!(service.list_pending_seed_updates().await?.len(), 1);

    let stored = service.get_node(SKILL_ID).await?.unwrap();
    let properties = stored.properties.to_string();
    assert!(!properties.contains("pending"), "{properties}");
    assert!(!properties.contains("shipped"), "{properties}");

    // Read from the table itself: the service's list would hide an orphan row.
    service.delete_node(SKILL_ID, stored.version).await?;
    assert!(service
        .store()
        .list_pending_seed_updates()
        .await?
        .is_empty());
    Ok(())
}

// A seed's body is the user's once anything beneath its root is no longer
// what seeding wrote there, whichever call changed it. Only an `update_node`
// to the content or fields of a line of a seed at the top of its tree marks
// the body as edited, so the tests that follow change it some other way.

#[tokio::test]
async fn a_line_the_user_added_under_a_seed_is_kept_when_its_body_changes() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    let note = write_line(&service, Some(SKILL_ID), "A note of the user's.").await?;
    let beneath = write_line(&service, Some(&note), "Beneath the user's note.").await?;

    let v2 = skill("File a note.", "Shipped body, second version.");
    seed(&service, &v2).await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped body, first version.", "A note of the user's."]
    );
    assert!(service.get_node(&beneath).await?.is_some());
    assert_only_guidance_pending(&service, &v2).await?;
    Ok(())
}

#[tokio::test]
async fn lines_the_user_moved_under_a_seed_are_kept_when_its_body_changes() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    let page = write_line(&service, None, "A page of the user's.").await?;
    let moved = write_line(&service, Some(&page), "A line moved into the skill.").await?;
    let beneath = write_line(&service, Some(&moved), "Beneath the moved line.").await?;
    move_under(&service, &moved, SKILL_ID).await?;

    let v2 = skill("File a note.", "Shipped body, second version.");
    seed(&service, &v2).await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        [
            "Shipped body, first version.",
            "A line moved into the skill."
        ]
    );
    assert!(service.get_node(&beneath).await?.is_some());
    assert_only_guidance_pending(&service, &v2).await?;
    Ok(())
}

#[tokio::test]
async fn lines_the_user_imported_under_a_seed_are_kept_when_its_body_changes() -> Result<()> {
    const IMPORTED: &str = "0f1d6c4e-8a3b-4c57-9e21-5b7a3d9c1e11";
    const IMPORTED_CHILD: &str = "0f1d6c4e-8a3b-4c57-9e21-5b7a3d9c1e12";
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    service
        .bulk_create_hierarchy(vec![
            (
                IMPORTED.to_string(),
                "text".to_string(),
                "An imported line.".to_string(),
                Some(SKILL_ID.to_string()),
                100.0,
                json!({}),
            ),
            (
                IMPORTED_CHILD.to_string(),
                "text".to_string(),
                "Beneath the imported line.".to_string(),
                Some(IMPORTED.to_string()),
                1.0,
                json!({}),
            ),
        ])
        .await?;

    let v2 = skill("File a note.", "Shipped body, second version.");
    seed(&service, &v2).await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped body, first version.", "An imported line."]
    );
    assert!(service.get_node(IMPORTED_CHILD).await?.is_some());
    assert_only_guidance_pending(&service, &v2).await?;
    Ok(())
}

#[tokio::test]
async fn an_edit_to_a_seed_filed_under_a_page_is_kept_when_its_body_changes() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    let page = write_line(&service, None, "A page of the user's.").await?;
    move_under(&service, SKILL_ID, &page).await?;
    edit_body(&service, SKILL_ID, "The body the user wrote.").await?;

    let v2 = skill("File a note.", "Shipped body, second version.");
    seed(&service, &v2).await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["The body the user wrote."]
    );
    assert_only_guidance_pending(&service, &v2).await?;
    Ok(())
}

#[tokio::test]
async fn a_line_the_user_deleted_from_a_seed_stays_deleted_when_its_body_changes() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped line one.\n\nShipped line two."),
    )
    .await?;
    let children = service.get_children(SKILL_ID).await?;
    assert_eq!(children.len(), 2, "{children:?}");
    service
        .delete_node(&children[0].id, children[0].version)
        .await?;

    let v2 = skill("File a note.", "Shipped line one.\n\nShipped line three.");
    seed(&service, &v2).await?;

    assert_eq!(body(&service, SKILL_ID).await?, ["Shipped line two."]);
    assert_only_guidance_pending(&service, &v2).await?;
    Ok(())
}

#[tokio::test]
async fn lines_the_user_reordered_in_a_seed_stay_in_their_order_when_its_body_changes() -> Result<()>
{
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped line one.\n\nShipped line two."),
    )
    .await?;
    let children = service.get_children(SKILL_ID).await?;
    service
        .reorder_node(
            &children[1].id,
            children[1].version,
            InsertPosition::Beginning,
        )
        .await?;

    let v2 = skill("File a note.", "Shipped line one.\n\nShipped line three.");
    seed(&service, &v2).await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped line two.", "Shipped line one."]
    );
    assert_only_guidance_pending(&service, &v2).await?;
    Ok(())
}

/// Archiving changes no content, so the edit marks nothing; the line stays
/// archived.
#[tokio::test]
async fn a_line_the_user_archived_in_a_seed_stays_archived_when_its_body_changes() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped line one.\n\nShipped line two."),
    )
    .await?;
    let children = service.get_children(SKILL_ID).await?;
    let archived = children[0].id.clone();
    service
        .update_node(
            &archived,
            children[0].version,
            NodeUpdate::default().with_lifecycle_status("archived".to_string()),
        )
        .await?;

    let v2 = skill("File a note.", "Shipped line one.\n\nShipped line three.");
    seed(&service, &v2).await?;

    let stored = service
        .get_node(&archived)
        .await?
        .expect("the archived line is kept");
    assert_eq!(stored.lifecycle_status, "archived");
    assert_only_guidance_pending(&service, &v2).await?;
    Ok(())
}

/// A line moved away and back to its place leaves the body as seeding wrote
/// it, though the move bumped its version and timestamps and gave it new
/// order values: only what the body holds, and in what order, is compared.
#[tokio::test]
async fn a_line_moved_away_and_back_leaves_the_body_free_to_follow_what_ships() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped line one.\n\nShipped line two."),
    )
    .await?;
    let children = service.get_children(SKILL_ID).await?;
    let (one, two) = (children[0].id.clone(), children[1].id.clone());
    let orders_as_seeded = line_orders(&service).await?;

    // Line two to the front, then line one in front of it: the order they
    // were seeded in, under new order values.
    for line in [&two, &one] {
        let node = service.get_node(line).await?.expect("the line exists");
        service
            .reorder_node(line, node.version, InsertPosition::Beginning)
            .await?;
    }
    // Line two moved out to a page of the user's, then back to the end.
    let page = write_line(&service, None, "A page of the user's.").await?;
    move_under(&service, &two, &page).await?;
    move_under(&service, &two, SKILL_ID).await?;

    let moved = service.get_node(&two).await?.expect("line two exists");
    assert!(moved.version > children[1].version, "the moves bumped it");
    assert_ne!(line_orders(&service).await?, orders_as_seeded);
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped line one.", "Shipped line two."]
    );

    seed(
        &service,
        &skill("File a note.", "Shipped line one.\n\nShipped line three."),
    )
    .await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped line one.", "Shipped line three."]
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

/// What seeding writes is recorded each time it writes a body, so a body
/// replaced by a shipped change follows the next one on its own too.
#[tokio::test]
async fn a_replaced_body_follows_the_next_shipped_change_on_its_own() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, second version."),
    )
    .await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, third version."),
    )
    .await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped body, third version."]
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

/// A shipped settings change leaves the record of what seeding wrote beneath
/// the root alone, so the next body change still applies on its own.
#[tokio::test]
async fn a_settings_change_leaves_the_body_free_to_follow_what_ships() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    seed(
        &service,
        &skill("File a note, reworded.", "Shipped body, first version."),
    )
    .await?;
    seed(
        &service,
        &skill("File a note, reworded.", "Shipped body, second version."),
    )
    .await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped body, second version."]
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

/// Taking the shipped body discards what the user added, by their choice,
/// and the body follows what ships on its own again afterwards.
#[tokio::test]
async fn taking_the_shipped_body_over_added_lines_lets_later_changes_apply() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    let note = write_line(&service, Some(SKILL_ID), "A note of the user's.").await?;
    let v2 = skill("File a note.", "Shipped body, second version.");
    seed(&service, &v2).await?;
    assert_only_guidance_pending(&service, &v2).await?;

    assert!(
        service
            .take_seed_update(&prepare_nodes_from_template(&v2)?, SeedAspect::Guidance)
            .await?
    );
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped body, second version."]
    );
    assert!(service.get_node(&note).await?.is_none());

    seed(
        &service,
        &skill("File a note.", "Shipped body, third version."),
    )
    .await?;
    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped body, third version."]
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

/// A body with no record of what seeding wrote beneath it cannot be shown to
/// hold nothing of the user's, so it is kept too.
#[tokio::test]
async fn a_body_with_no_record_of_what_seeding_wrote_is_kept_when_it_changes() -> Result<()> {
    let (service, _temp) = create_test_service().await?;
    seed(
        &service,
        &skill("File a note.", "Shipped body, first version."),
    )
    .await?;
    service
        .update_node_unchecked(
            SKILL_ID,
            NodeUpdate::default()
                .with_properties(json!({ "_seed": { "guidance_seeded_fingerprint": null } })),
        )
        .await?;

    let v2 = skill("File a note.", "Shipped body, second version.");
    seed(&service, &v2).await?;

    assert_eq!(
        body(&service, SKILL_ID).await?,
        ["Shipped body, first version."]
    );
    assert_only_guidance_pending(&service, &v2).await?;
    Ok(())
}
