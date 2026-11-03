//! Evidence for the open points the built-in shape record settles, and the
//! hash's identification of a shape from a database's contents (ADR-104).
//!
//! Nothing here changes what opening a database does: the tests read, and use
//! the same seeding and schema APIs a user or a later release would.

use anyhow::Result;
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::{prepare_nodes_from_template, NodeTemplate, SeedTier};
use nodespace_core::models::{PlayNodeUpdate, SeedAspect, SkillFields};
use nodespace_core::playbook::core_plays::parent_task_completion_rules;
use nodespace_core::services::NodeService;
use nodespace_core::shape_record::embedded_shape_hash;
use nodespace_core::shape_record::identify::hash_of_database;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

const SKILL_ID: &str = "0f1d6c4e-8a3b-4c57-9e21-5b7a3d9c2e01";
const PLAY_ID: &str = "0f1d6c4e-8a3b-4c57-9e21-5b7a3d9c2e02";

async fn service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let mut store = Arc::new(SqliteStore::new(temp_dir.path().join("test.db")).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

fn skill(body: &str) -> NodeTemplate {
    NodeTemplate::skill(
        SKILL_ID,
        "Filing Notes",
        SkillFields {
            use_for: "filing notes".to_string(),
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

async fn seed(service: &NodeService, templates: &[NodeTemplate]) -> Result<()> {
    let groups = templates
        .iter()
        .map(prepare_nodes_from_template)
        .collect::<Result<Vec<_>, _>>()?;
    service.seed_nodes_from_templates(groups).await?;
    Ok(())
}

/// A seed that stops shipping is neither removed nor recorded as pending,
/// whether or not the user edited it: reconciliation visits only what the
/// build ships. The record classifies such a removal `migrate`.
#[tokio::test]
async fn a_seed_that_stops_shipping_is_left_alone() -> Result<()> {
    let (service, _temp) = service().await?;
    seed(
        &service,
        &[skill("First body."), play("First description.")],
    )
    .await?;
    let node = service.get_node(PLAY_ID).await?.unwrap();
    service
        .update_play_node(
            PLAY_ID,
            node.version,
            PlayNodeUpdate {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await?;

    // A later release ships neither seed.
    seed(&service, &[]).await?;

    for id in [SKILL_ID, PLAY_ID] {
        assert!(service.get_node(id).await?.is_some(), "{id} is still there");
    }
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

/// Switching a Play off through the typed Play update counts as an edit to the
/// Play's configuration: a later shipped change to it is put to the user as
/// pending instead of replacing it.
#[tokio::test]
async fn switching_a_play_off_edits_its_configuration() -> Result<()> {
    let (service, _temp) = service().await?;
    seed(&service, &[play("First description.")]).await?;
    let node = service.get_node(PLAY_ID).await?.unwrap();
    service
        .update_play_node(
            PLAY_ID,
            node.version,
            PlayNodeUpdate {
                enabled: Some(false),
                ..Default::default()
            },
        )
        .await?;
    let stored = service.get_node(PLAY_ID).await?.unwrap();
    assert_eq!(stored.properties["_seed"]["config_modified"], true);

    seed(&service, &[play("Second description.")]).await?;

    let stored = service.get_node(PLAY_ID).await?.unwrap();
    assert_eq!(
        stored.properties["play"]["description"],
        "First description."
    );
    assert_eq!(stored.properties["play"]["enabled"], false);
    let pending = service.list_pending_seed_updates().await?;
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(pending[0].aspect, SeedAspect::Config);
    Ok(())
}

/// A Play nobody touched takes the shipped change without asking, so the edit
/// above is what makes the difference.
#[tokio::test]
async fn an_untouched_play_takes_the_shipped_change() -> Result<()> {
    let (service, _temp) = service().await?;
    seed(&service, &[play("First description.")]).await?;
    seed(&service, &[play("Second description.")]).await?;
    let stored = service.get_node(PLAY_ID).await?.unwrap();
    assert_eq!(
        stored.properties["play"]["description"],
        "Second description."
    );
    assert!(service.list_pending_seed_updates().await?.is_empty());
    Ok(())
}

/// A database holding this build's shape hashes to the hash its snapshot
/// records, computed from the database's contents alone: no stored version.
#[tokio::test]
async fn a_databases_contents_hash_to_the_shape_snapshot() -> Result<()> {
    let (service, _temp) = service().await?;
    assert_eq!(
        hash_of_database(service.store()).await?,
        embedded_shape_hash().expect("the embedded snapshot records a hash")
    );
    Ok(())
}

/// What a user may do to a core schema does not move the hash, so the shape is
/// still identified: a namespaced field and a value added to an extensible
/// enum.
#[tokio::test]
async fn a_users_schema_extensions_do_not_change_the_identified_shape() -> Result<()> {
    let (service, _temp) = service().await?;
    let before = hash_of_database(service.store()).await?;
    nodespace_core::schema::handle_update_schema(
        &service,
        json!({
            "schema_id": "task",
            "add_fields": [{ "name": "custom:estimate", "type": "number" }],
            "add_field_values": [{
                "field": "status",
                "values": [{ "value": "parked", "label": "Parked" }]
            }]
        }),
    )
    .await?;
    assert_eq!(hash_of_database(service.store()).await?, before);
    Ok(())
}
