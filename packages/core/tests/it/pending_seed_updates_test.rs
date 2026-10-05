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
use nodespace_core::playbook::core_plays::parent_task_completion_rules;
use nodespace_core::services::NodeService;
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

fn skill(description: &str, body: &str) -> NodeTemplate {
    NodeTemplate::skill(
        SKILL_ID,
        "Filing Notes",
        SkillFields {
            description: description.to_string(),
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
        json!({ "description": "The user's description." }),
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
        stored.properties["skill"]["description"],
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
        stored.properties["skill"]["description"],
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
        stored.properties["skill"]["description"],
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
        stored.properties["skill"]["description"],
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
        json!({ "description": "The user's description." }),
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
