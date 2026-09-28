//! Object-valued properties must round-trip on every type, not just `task`.
//!
//! Storage nests a node's own fields under its type key
//! (`{"venue": {"capacity": 250}}`), and every read path flattens that key back
//! out. A field whose value happens to be a JSON object is indistinguishable by
//! shape from a *sibling namespace* — another type's dormant key left beside the
//! active one — so a normalizer that classified on the value's type had to guess,
//! and guessed "namespace". The field landed outside the type key, where the
//! flattener never looks, and disappeared from output with no error and no null:
//! a documented input shape (`--property address='{"city":"Berlin"}'`) that
//! silently lost data.
//!
//! The classification now keys off the property name's `_` prefix instead, which
//! is unambiguous: `_`-prefixed keys are internal bookkeeping that belongs at the
//! top level, everything else is a field of this type whatever its value.
//!
//! These tests pin the round-trip end to end for a user-defined type, through
//! each of the three read surfaces that flatten independently — `get_node`,
//! `query_nodes`, and `get_related_nodes` — because they reach the flattener by
//! different routes and could drift apart.
//!
//! They also pin the reservation that makes the rule safe. Because `_` now
//! decides classification, a schema that could *declare* a `_`-prefixed field
//! would relocate the same silent loss rather than close it: the write path
//! would store such a field top-level and every read would drop it. Both routes
//! to that state — declaring one at `create_schema`, and renaming an existing
//! field into one — are rejected, and the tests assert the failure, not just
//! that the happy path still works.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::{Node, NodeUpdate},
    ops::{node_ops, rel_ops},
    schema::{handle_create_schema, handle_update_schema},
    services::{InsertPositionOwned, NodeService},
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);
    Ok((node_service, temp_dir))
}

/// A user-defined type with an `object` field — the shape with no built-in
/// equivalent, and the one that used to lose its value.
async fn seed_venue_schema(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "Venue",
            "fields": [
                { "name": "address", "type": "object", "protection": "user", "indexed": false },
                { "name": "capacity", "type": "number", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("venue schema: {e}"))?;
    Ok(())
}

const ADDRESS: fn() -> serde_json::Value =
    || json!({ "city": "Berlin", "street": "Torstrasse 1", "postal": { "code": "10119" } });

async fn create_venue(svc: &Arc<NodeService>) -> Result<String> {
    let output = node_ops::create_node(
        svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "venue".to_string(),
            content: "Berghain".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: json!({ "address": ADDRESS(), "capacity": 1500 }),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await?;
    Ok(output.node_id)
}

fn properties_of(node: &serde_json::Value) -> &serde_json::Value {
    node.get("properties")
        .expect("read output must carry properties")
}

#[tokio::test]
async fn object_property_round_trips_through_get_node() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_venue_schema(&svc).await?;
    let id = create_venue(&svc).await?;

    let node = node_ops::get_node(
        &svc,
        node_ops::GetNodeInput {
            node_id: id.clone(),
        },
    )
    .await?;

    let props = properties_of(&node);
    assert_eq!(
        props.get("address"),
        Some(&ADDRESS()),
        "the object-valued field must survive the read, nested value included"
    );
    assert_eq!(
        props.get("capacity"),
        Some(&json!(1500)),
        "a scalar sibling must be unaffected"
    );
    assert!(
        props.get("venue").is_none(),
        "storage namespacing must not be observable on the read surface"
    );
    Ok(())
}

#[tokio::test]
async fn object_property_round_trips_through_query() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_venue_schema(&svc).await?;
    create_venue(&svc).await?;

    let out = node_ops::query_nodes(
        &svc,
        node_ops::QueryNodesInput {
            node_type: Some("venue".to_string()),
            limit: None,
            offset: None,
            collection_id: None,
            collection: None,
            filters: None,
        },
    )
    .await?;

    assert_eq!(out.count, 1, "the venue must be queryable");
    assert_eq!(
        properties_of(&out.nodes[0]).get("address"),
        Some(&ADDRESS()),
        "query must expose the object-valued field like every other read path"
    );
    Ok(())
}

#[tokio::test]
async fn object_property_round_trips_through_relationship_get() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_venue_schema(&svc).await?;

    handle_create_schema(
        &svc,
        json!({
            "name": "Concert",
            "fields": [{ "name": "billing", "type": "string", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "held_at",
                "targetType": "venue",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "concerts",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("concert schema: {e}"))?;

    let venue_id = create_venue(&svc).await?;
    let concert_id = uuid::Uuid::new_v4().to_string();
    svc.create_node(Node::new_with_id(
        concert_id.clone(),
        "concert".to_string(),
        "Klubnacht".to_string(),
        json!({ "billing": "headline" }),
    ))
    .await?;

    svc.create_relationship(&concert_id, "held_at", &venue_id, json!({}))
        .await?;

    let out = rel_ops::get_related_nodes(
        &svc,
        rel_ops::GetRelatedInput {
            node_id: concert_id,
            relationship_name: "held_at".to_string(),
            direction: "out".to_string(),
        },
    )
    .await?;

    assert_eq!(out.related_nodes.len(), 1, "the venue must be reachable");
    assert_eq!(
        properties_of(&out.related_nodes[0]).get("address"),
        Some(&ADDRESS()),
        "relationship traversal must expose the object-valued field too"
    );
    Ok(())
}

/// An update must not lose the field either — the update path normalizes
/// separately from create, and used to pass no schema at all.
#[tokio::test]
async fn object_property_survives_an_update() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_venue_schema(&svc).await?;
    let id = create_venue(&svc).await?;

    let moved = json!({ "city": "Leipzig", "street": "Karl-Liebknecht-Str 2" });
    node_ops::update_node(
        &svc,
        node_ops::UpdateNodeInput {
            node_id: id.clone(),
            version: None,
            node_type: None,
            content: None,
            properties: Some(json!({ "address": moved })),
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await?;

    let node = node_ops::get_node(&svc, node_ops::GetNodeInput { node_id: id }).await?;
    let props = properties_of(&node);
    assert_eq!(
        props.get("address").and_then(|a| a.get("city")),
        Some(&json!("Leipzig")),
        "the updated object value must be readable back"
    );
    assert_eq!(
        props.get("capacity"),
        Some(&json!(1500)),
        "an untouched sibling must survive the deep merge"
    );
    Ok(())
}

/// `bulk_update` normalizes through its own call site rather than the one
/// `update_node` uses, so it could drift from the single-node path. An empty
/// object is included because `{}` is the one value whose two readings —
/// "an object-valued field" and "an empty namespace" — used to be genuinely
/// indistinguishable.
#[tokio::test]
async fn object_property_survives_a_bulk_update() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_venue_schema(&svc).await?;
    let id = create_venue(&svc).await?;

    svc.bulk_update(vec![(
        id.clone(),
        NodeUpdate {
            node_type: None,
            content: None,
            properties: Some(json!({ "address": { "city": "Hamburg" }, "notes": {} })),
            title: None,
            lifecycle_status: None,
        },
    )])
    .await?;

    let node = node_ops::get_node(&svc, node_ops::GetNodeInput { node_id: id }).await?;
    let props = properties_of(&node);
    assert_eq!(
        props.get("address").and_then(|a| a.get("city")),
        Some(&json!("Hamburg")),
        "bulk_update must namespace an object-valued field like the single-node path"
    );
    assert_eq!(
        props.get("notes"),
        Some(&json!({})),
        "an empty object is a field value, not an empty namespace to be hoisted"
    );
    Ok(())
}

/// A schema may not declare a `_`-prefixed field, because such a field could be
/// written but never read back: the write path leaves it outside the type
/// namespace and the flattener drops it from every read surface.
///
/// Without this gate the data loss this file exists to close would simply move
/// — `create_schema` would accept `_internal_id`, a write would store it
/// top-level, and every read would omit it with no error. Rejected at schema
/// creation rather than warned about, because unlike a name that merely shadows
/// a reserved core property (which stores a field that does work), this one is
/// unreadable by construction.
#[tokio::test]
async fn schema_cannot_declare_an_underscore_prefixed_field() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Widget",
            "fields": [{ "name": "_internal_id", "type": "number" }]
        }),
    )
    .await
    .expect_err("a leading '_' must be rejected, not stored");

    let msg = err.to_string();
    assert!(
        msg.contains("_internal_id"),
        "the error must name the offending field, got: {msg}"
    );
    // The rejection redirects rather than merely refusing: it names the legal
    // form of the same intent, built from the caller's own field name, so the
    // suggestion can be pasted rather than translated.
    assert!(
        msg.contains("custom:internal_id"),
        "the error must suggest the namespaced alternative, got: {msg}"
    );

    // Rejected before any write — a half-created schema would be worse than
    // the silent loss it replaces.
    assert!(
        svc.get_node("widget").await?.is_none(),
        "a rejected create must leave no schema node behind"
    );
    Ok(())
}

/// Renaming an existing field *to* a `_`-prefixed name is the same loss by
/// another route, and a worse one: a rename migrates every instance's property
/// data as it executes, so accepting it would move real stored values into a
/// key no read path can see.
#[tokio::test]
async fn a_field_cannot_be_renamed_to_an_underscore_prefixed_name() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_venue_schema(&svc).await?;
    let id = create_venue(&svc).await?;

    let err = handle_update_schema(
        &svc,
        json!({
            "schema_id": "venue",
            "rename_fields": [{ "from": "capacity", "to": "_capacity" }]
        }),
    )
    .await
    .expect_err("renaming to a reserved prefix must be rejected");

    assert!(
        err.to_string().contains("_capacity"),
        "the error must name the destination, got: {err}"
    );

    // The rejection must land before the migration, not after it: the value is
    // still readable under its original name.
    let node = node_ops::get_node(&svc, node_ops::GetNodeInput { node_id: id }).await?;
    assert_eq!(
        properties_of(&node).get("capacity"),
        Some(&json!(1500)),
        "a rejected rename must not have migrated any instance data"
    );
    Ok(())
}

/// The reservation is on the *stored key*, which is the field name verbatim.
/// A namespaced name stores under `custom:_internal`, which does not begin with
/// `_`, so it survives the flattener and stays legal — the gate must not
/// over-reach into names that round-trip correctly.
#[tokio::test]
async fn a_namespaced_field_may_contain_an_underscore_after_the_prefix() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;

    handle_create_schema(
        &svc,
        json!({
            "name": "Gadget",
            "fields": [{ "name": "custom:_internal", "type": "text" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("custom:_internal must stay legal: {e}"))?;

    let id = uuid::Uuid::new_v4().to_string();
    svc.create_node(Node::new_with_id(
        id.clone(),
        "gadget".to_string(),
        "Sprocket".to_string(),
        json!({ "custom:_internal": "kept" }),
    ))
    .await?;

    let node = node_ops::get_node(&svc, node_ops::GetNodeInput { node_id: id }).await?;
    assert_eq!(
        properties_of(&node).get("custom:_internal"),
        Some(&json!("kept")),
        "a prefixed name is a normal field and must round-trip"
    );
    Ok(())
}

/// `_`-prefixed keys are the one thing that stays outside the type namespace:
/// seed bookkeeping must land at a fixed, type-independent path rather than
/// being swept in with the type's own fields.
#[tokio::test]
async fn underscore_prefixed_keys_stay_out_of_the_type_namespace() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_venue_schema(&svc).await?;

    let id = uuid::Uuid::new_v4().to_string();
    svc.create_node(Node::new_with_id(
        id.clone(),
        "venue".to_string(),
        "Tresor".to_string(),
        json!({
            "address": ADDRESS(),
            "_seed": { "key": "Tresor", "tier": "system" }
        }),
    ))
    .await?;

    let stored = svc.get_node(&id).await?.expect("venue must exist");
    assert_eq!(
        stored.properties["venue"]["address"],
        ADDRESS(),
        "the field belongs inside the type namespace"
    );
    assert_eq!(
        stored.properties["_seed"]["key"], "Tresor",
        "_seed belongs at the top level, not inside the type namespace"
    );
    assert!(
        stored.properties["venue"].get("_seed").is_none(),
        "_seed must not be swept into the type namespace"
    );
    Ok(())
}
