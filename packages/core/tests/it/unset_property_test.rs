//! Removing a property from a node.
//!
//! A properties patch only ever merges, so `null` stored a null and the key
//! stayed. `NodeUpdate::unset_properties` removes the key itself, through the
//! same validation, version check and events as any other update.

use anyhow::Result;
use nodespace_core::{
    db::{DomainEvent, SqliteStore},
    models::NodeUpdate,
    ops::node_ops,
    schema::handle_create_schema,
    services::{InsertPositionOwned, NodeService, NodeServiceError},
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let mut store = Arc::new(SqliteStore::new(temp_dir.path().join("test.db")).await?);
    Ok((Arc::new(NodeService::new(&mut store).await?), temp_dir))
}

async fn venue(svc: &Arc<NodeService>) -> Result<String> {
    handle_create_schema(
        svc,
        json!({
            "name": "Venue",
            "fields": [
                { "name": "title", "type": "text", "required": true },
                { "name": "capacity", "type": "number" },
                { "name": "notes", "type": "text" }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("venue schema: {e}"))?;
    let created = node_ops::create_node(
        svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "venue".to_string(),
            content: "Berghain".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: json!({ "title": "Berghain", "capacity": 1500, "notes": "club" }),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await?;
    Ok(created.node_id)
}

fn unset(keys: &[&str]) -> NodeUpdate {
    NodeUpdate::new().with_unset_properties(keys.iter().map(|k| k.to_string()).collect())
}

/// The node as the read surface returns it.
async fn read(svc: &Arc<NodeService>, id: &str) -> Result<serde_json::Value> {
    Ok(node_ops::get_node(
        svc,
        node_ops::GetNodeInput {
            node_id: id.to_string(),
        },
    )
    .await?)
}

#[tokio::test]
async fn an_unset_removes_the_key_and_leaves_no_null() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;
    let version = svc.get_node(&id).await?.unwrap().version;

    svc.update_node(&id, version, unset(&["capacity"])).await?;

    let node = read(&svc, &id).await?;
    let props = node["properties"].as_object().unwrap();
    assert!(!props.contains_key("capacity"), "{props:?}");
    assert_eq!(props.get("notes"), Some(&json!("club")), "a sibling stays");
    let stored = svc.get_node(&id).await?.unwrap();
    assert!(
        stored.properties["venue"].get("capacity").is_none(),
        "no null left behind: {}",
        stored.properties
    );
    assert_eq!(stored.version, version + 1);
    Ok(())
}

#[tokio::test]
async fn an_unset_through_the_ops_update_removes_the_key() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;

    node_ops::update_node(
        &svc,
        node_ops::UpdateNodeInput {
            node_id: id.clone(),
            unset_properties: vec!["notes".to_string()],
            ..Default::default()
        },
    )
    .await?;

    let node = read(&svc, &id).await?;
    assert!(node["properties"].get("notes").is_none(), "{node}");
    assert_eq!(node["properties"]["capacity"], json!(1500));
    Ok(())
}

#[tokio::test]
async fn an_unset_of_a_key_the_node_does_not_hold_changes_nothing() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;
    svc.update_node(
        &id,
        svc.get_node(&id).await?.unwrap().version,
        unset(&["capacity"]),
    )
    .await?;
    let before = svc.get_node(&id).await?.unwrap();
    let mut rx = svc.subscribe_to_events();

    let returned = svc
        .update_node(&id, before.version, unset(&["capacity", "never_set"]))
        .await?;

    let after = svc.get_node(&id).await?.unwrap();
    assert_eq!(after.properties, before.properties);
    assert_eq!(after.version, before.version, "no version bump");
    assert_eq!(returned.version, before.version);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(300), rx.recv())
            .await
            .is_err(),
        "no event for a write that changed nothing"
    );
    // A stale version still conflicts, and a refused key is still refused.
    assert!(matches!(
        svc.update_node(&id, before.version + 5, unset(&["never_set"]))
            .await,
        Err(NodeServiceError::VersionConflict { .. })
    ));
    assert!(matches!(
        svc.update_node(&id, before.version, unset(&["_seed"]))
            .await,
        Err(NodeServiceError::InvalidUpdate(_))
    ));
    Ok(())
}

#[tokio::test]
async fn an_unset_is_applied_after_the_patch_and_in_one_write() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;
    let version = svc.get_node(&id).await?.unwrap().version;

    let update = unset(&["capacity"]).with_properties(json!({ "notes": "techno" }));
    svc.update_node(&id, version, update).await?;

    let node = read(&svc, &id).await?;
    assert_eq!(node["properties"]["notes"], json!("techno"));
    assert!(node["properties"].get("capacity").is_none());
    assert_eq!(svc.get_node(&id).await?.unwrap().version, version + 1);
    Ok(())
}

#[tokio::test]
async fn an_unset_is_version_checked() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;
    let version = svc.get_node(&id).await?.unwrap().version;

    let stale = svc
        .update_node(&id, version + 7, unset(&["capacity"]))
        .await;

    assert!(
        matches!(stale, Err(NodeServiceError::VersionConflict { .. })),
        "{stale:?}"
    );
    assert!(read(&svc, &id).await?["properties"]
        .get("capacity")
        .is_some());
    Ok(())
}

#[tokio::test]
async fn an_unset_cannot_remove_a_required_field() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;
    let version = svc.get_node(&id).await?.unwrap().version;

    let refused = svc.update_node(&id, version, unset(&["title"])).await;

    assert!(refused.is_err(), "validation runs on the unset result");
    assert!(read(&svc, &id).await?["properties"].get("title").is_some());
    Ok(())
}

#[tokio::test]
async fn bookkeeping_keys_and_empty_keys_are_refused() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;
    let version = svc.get_node(&id).await?.unwrap().version;

    for key in ["_seed", ""] {
        let refused = svc.update_node(&id, version, unset(&[key])).await;
        assert!(
            matches!(refused, Err(NodeServiceError::InvalidUpdate(_))),
            "{key:?}: {refused:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_unset_emits_a_node_updated_event() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;
    let version = svc.get_node(&id).await?.unwrap().version;
    let mut rx = svc.subscribe_to_events();

    svc.update_node(&id, version, unset(&["capacity"])).await?;

    let envelope = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("an event within a second")?;
    match envelope.event {
        DomainEvent::NodeUpdated {
            node_id,
            changed_properties,
            ..
        } => {
            assert_eq!(node_id, id);
            let change = changed_properties
                .iter()
                .find(|c| c.key.ends_with("capacity"))
                .expect("the removed key is reported as changed");
            assert_eq!(change.old_value, Some(json!(1500)));
            assert_eq!(change.new_value, None, "removed, not nulled");
        }
        other => panic!("expected NodeUpdated, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn an_unset_through_the_unchecked_update_removes_the_key() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;

    svc.update_node_unchecked(&id, unset(&["capacity"])).await?;

    let stored = svc.get_node(&id).await?.unwrap();
    assert!(stored.properties["venue"].get("capacity").is_none());
    assert_eq!(stored.properties["venue"]["notes"], json!("club"));
    Ok(())
}

#[tokio::test]
async fn an_unset_through_a_bulk_update_removes_the_key() -> Result<()> {
    let (svc, _tmp) = service().await?;
    let id = venue(&svc).await?;

    svc.bulk_update(vec![(id.clone(), unset(&["capacity"]))])
        .await?;

    let stored = svc.get_node(&id).await?.unwrap();
    assert!(stored.properties["venue"].get("capacity").is_none());
    assert_eq!(stored.properties["venue"]["notes"], json!("club"));
    Ok(())
}
