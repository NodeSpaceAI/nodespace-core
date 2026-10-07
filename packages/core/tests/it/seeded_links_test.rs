//! A built-in skill's `attached_to` links are a seeded aspect (ADR-072,
//! ADR-094 §3 and §8): they follow what ships until the user deletes or adds
//! one, a link the user deleted stays deleted across a restart, a shipped
//! change to links the user edited is put to them, and a reset restores them.
//!
//! "A later build ships other links" is `reconcile_seeded_links` called with a
//! changed set: what the next database open does under that build.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, SeedAspect, SkillFields, SKILL_ATTACHED_TO};
use nodespace_core::services::node_service::links_version;
use nodespace_core::services::{NodeService, ShippedLinks};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let mut store = Arc::new(SqliteStore::new(temp_dir.path().join("test.db")).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

async fn create_text(service: &NodeService, content: &str) -> Result<String> {
    Ok(service
        .create_node(Node::new(
            "text".to_string(),
            content.to_string(),
            json!({}),
        ))
        .await?)
}

/// A skill carrying seed bookkeeping, as a built-in one does.
async fn create_seeded_skill(service: &NodeService, name: &str) -> Result<String> {
    let mut node = SkillFields::new("When this applies.", &["get_node"], 2).into_node(name);
    node.properties["_seed"] = json!({ "tier": "system" });
    Ok(service.create_node(node).await?)
}

async fn links_of(service: &NodeService, skill: &str) -> Result<Vec<String>> {
    let mut targets = service.current_links(skill).await?;
    targets.sort();
    Ok(targets)
}

fn sorted(ids: &[&str]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    ids.sort();
    ids
}

async fn seed_block(service: &NodeService, skill: &str) -> Result<Value> {
    let node = service.get_node(skill).await?.expect("the skill");
    Ok(node.properties.get("_seed").cloned().unwrap_or(Value::Null))
}

async fn pending(service: &NodeService, skill: &str) -> Result<Option<String>> {
    Ok(service
        .get_pending_seed_update(skill, SeedAspect::Links)
        .await?
        .map(|update| update.shipped_version))
}

async fn reconcile(service: &NodeService, skill: &str, targets: &[&str]) -> Result<()> {
    service
        .reconcile_seeded_links(&[ShippedLinks {
            skill_id: skill,
            targets,
        }])
        .await?;
    Ok(())
}

/// An untouched database follows a shipped change: the first open attaches
/// the skill, and a build that ships other links replaces them.
#[tokio::test]
async fn an_untouched_skill_follows_what_ships() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let skill = create_seeded_skill(&service, "Procedure").await?;
    let a = create_text(&service, "A").await?;
    let b = create_text(&service, "B").await?;

    reconcile(&service, &skill, &[&a]).await?;
    assert_eq!(links_of(&service, &skill).await?, sorted(&[&a]));
    let seed = seed_block(&service, &skill).await?;
    assert_eq!(seed["links_version"], json!(links_version(&[&a])));
    // Seeding is not the user's edit.
    assert!(seed.get("links_modified").is_none());

    reconcile(&service, &skill, &[&a, &b]).await?;
    assert_eq!(links_of(&service, &skill).await?, sorted(&[&a, &b]));

    reconcile(&service, &skill, &[&b]).await?;
    assert_eq!(links_of(&service, &skill).await?, sorted(&[&b]));
    assert_eq!(pending(&service, &skill).await?, None);
    Ok(())
}

/// A link the user deleted stays deleted across a restart.
#[tokio::test]
async fn a_deleted_link_stays_deleted() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let skill = create_seeded_skill(&service, "Procedure").await?;
    let a = create_text(&service, "A").await?;
    let b = create_text(&service, "B").await?;
    reconcile(&service, &skill, &[&a, &b]).await?;

    service
        .delete_relationship(&skill, SKILL_ATTACHED_TO, &a)
        .await?;
    assert_eq!(seed_block(&service, &skill).await?["links_modified"], true);

    // The restart: the same build opens the database again.
    reconcile(&service, &skill, &[&a, &b]).await?;
    assert_eq!(links_of(&service, &skill).await?, sorted(&[&b]));
    assert_eq!(pending(&service, &skill).await?, None);
    Ok(())
}

/// A link the user added is theirs too, and a deletion through the reverse
/// name is the same edit.
#[tokio::test]
async fn adding_or_deleting_through_either_name_marks_the_links() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let skill = create_seeded_skill(&service, "Procedure").await?;
    let a = create_text(&service, "A").await?;
    let b = create_text(&service, "B").await?;
    reconcile(&service, &skill, &[&a]).await?;

    service
        .create_relationship(&skill, SKILL_ATTACHED_TO, &b, json!({}))
        .await?;
    assert_eq!(seed_block(&service, &skill).await?["links_modified"], true);

    service.reset_links(shipped(&skill, &[&a])).await?;
    assert_eq!(seed_block(&service, &skill).await?["links_modified"], false);

    service
        .delete_relationship(&a, "attached_skills", &skill)
        .await?;
    assert_eq!(seed_block(&service, &skill).await?["links_modified"], true);
    Ok(())
}

fn shipped<'a>(skill: &'a str, targets: &'a [&'a str]) -> ShippedLinks<'a> {
    ShippedLinks {
        skill_id: skill,
        targets,
    }
}

/// A skill a user wrote carries no seed bookkeeping, and attaching it is not
/// an edit to a built-in.
#[tokio::test]
async fn a_users_own_skill_is_not_marked() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let skill = service
        .create_node(SkillFields::new("When this applies.", &["get_node"], 2).into_node("Mine"))
        .await?;
    let a = create_text(&service, "A").await?;
    service
        .create_relationship(&skill, SKILL_ATTACHED_TO, &a, json!({}))
        .await?;
    assert_eq!(seed_block(&service, &skill).await?, Value::Null);
    Ok(())
}

/// A shipped change to links the user deleted is put to them, not applied
/// over their edit; keeping stamps it seen, taking applies it.
#[tokio::test]
async fn a_shipped_change_over_an_edit_is_pending() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let skill = create_seeded_skill(&service, "Procedure").await?;
    let a = create_text(&service, "Ready tasks").await?;
    let b = create_text(&service, "In progress").await?;
    let c = create_text(&service, "Awaiting review").await?;
    reconcile(&service, &skill, &[&a, &b]).await?;
    service
        .delete_relationship(&skill, SKILL_ATTACHED_TO, &a)
        .await?;

    let second = [a.as_str(), b.as_str(), c.as_str()];
    reconcile(&service, &skill, &second).await?;
    assert_eq!(links_of(&service, &skill).await?, sorted(&[&b]));
    assert_eq!(
        pending(&service, &skill).await?,
        Some(links_version(&second))
    );

    let listed = service.list_pending_seed_updates().await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].aspect, SeedAspect::Links);
    assert_eq!(listed[0].node_type, "skill");

    // The shipped links beside theirs.
    let comparison = service
        .compare_pending_links_update(shipped(&skill, &second))
        .await?
        .expect("an update is pending");
    assert!(comparison.shipped.contains(&format!("Ready tasks ({a})")));
    assert!(comparison
        .shipped
        .contains(&format!("Awaiting review ({c})")));
    assert_eq!(comparison.yours, format!("In progress ({b})"));

    // Keeping settles it across restarts, and the user's links stand.
    assert!(service.keep_seed_update(&skill, SeedAspect::Links).await?);
    reconcile(&service, &skill, &second).await?;
    assert_eq!(pending(&service, &skill).await?, None);
    assert_eq!(links_of(&service, &skill).await?, sorted(&[&b]));
    assert_eq!(seed_block(&service, &skill).await?["links_modified"], true);

    // Another shipped change is pending again; taking applies it.
    let third = [c.as_str()];
    reconcile(&service, &skill, &third).await?;
    assert_eq!(
        pending(&service, &skill).await?,
        Some(links_version(&third))
    );
    assert!(service.take_links_update(shipped(&skill, &third)).await?);
    assert_eq!(links_of(&service, &skill).await?, sorted(&[&c]));
    assert_eq!(pending(&service, &skill).await?, None);
    assert_eq!(seed_block(&service, &skill).await?["links_modified"], false);

    // Nothing is left to take.
    assert!(!service.take_links_update(shipped(&skill, &third)).await?);
    Ok(())
}

/// A reset restores a deleted link whether or not a change is pending,
/// removes one the user added, and the links follow what ships again.
#[tokio::test]
async fn a_reset_restores_the_shipped_links() -> Result<()> {
    let (service, _tmp) = test_service().await?;
    let skill = create_seeded_skill(&service, "Procedure").await?;
    let a = create_text(&service, "A").await?;
    let b = create_text(&service, "B").await?;
    let extra = create_text(&service, "Extra").await?;
    let first = [a.as_str(), b.as_str()];
    reconcile(&service, &skill, &first).await?;

    service
        .delete_relationship(&skill, SKILL_ATTACHED_TO, &a)
        .await?;
    service
        .create_relationship(&skill, SKILL_ATTACHED_TO, &extra, json!({}))
        .await?;

    assert!(service.reset_links(shipped(&skill, &first)).await?);
    assert_eq!(links_of(&service, &skill).await?, sorted(&[&a, &b]));
    assert_eq!(seed_block(&service, &skill).await?["links_modified"], false);

    // Following what ships again.
    let second = [b.as_str()];
    reconcile(&service, &skill, &second).await?;
    assert_eq!(links_of(&service, &skill).await?, sorted(&[&b]));
    assert_eq!(pending(&service, &skill).await?, None);

    // A skill that is not in this database has nothing to reset.
    assert!(
        !service
            .reset_links(shipped("no-such-skill", &first))
            .await?
    );
    Ok(())
}
