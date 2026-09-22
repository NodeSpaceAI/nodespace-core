//! Integration tests for table-backed schema relationship declarations.
//!
//! A `SchemaRelationship` declaration is stored as a `relationship` table row
//! between the declaring and target SCHEMA nodes (self-edge when untyped),
//! never as JSON in the schema node's `properties`. These tests cover:
//!
//! - storage shape: declarations land as relationship rows; `properties`
//!   carries no `relationships` key
//! - read-path agreement: every consolidated reader (`get_schema_node`,
//!   `get_all_schemas`, `get_inbound_relationships`, the relationship viewer
//!   aggregation, and `create_relationship`'s validation) sees the same
//!   declaration set — the highest-value regression, since the pre-table JSON
//!   parses could silently drift apart
//! - reserved-name rejection (built-in structural relationship names)
//! - block-by-default protection: removing/retargeting a declaration with live
//!   instance edges, and deleting a schema node with declarations, are refused
//! - instance/declaration scoping: `create_relationship` refuses schema-node
//!   endpoints; declaration edges never leak into instance queries

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::rel_ops,
    schema::{handle_create_schema, handle_update_schema},
    services::NodeService,
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

/// `project`-style schema declaring `widgets → widget` plus a target schema.
async fn create_widget_pair(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "Widget",
            "fields": [{ "name": "label", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("widget schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "Assembly",
            "fields": [{ "name": "title", "type": "string", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "widgets",
                "targetType": "widget",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "assemblies",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("assembly schema: {e}"))?;
    Ok(())
}

async fn make_node(svc: &NodeService, id: &str, node_type: &str) -> Result<()> {
    svc.create_node(Node::new_with_id(
        id.to_string(),
        node_type.to_string(),
        format!("{id} content"),
        json!({}),
    ))
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Storage shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn declarations_are_relationship_rows_not_properties_json() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_widget_pair(&svc).await?;

    // The schema node's stored properties must NOT carry a relationships key.
    let raw = svc
        .get_node("assembly")
        .await?
        .expect("assembly schema node exists");
    assert!(
        raw.properties.get("relationships").is_none(),
        "declarations must not be stored in properties, got: {}",
        raw.properties
    );

    // The declaration lives as a relationship-table row between schema nodes.
    let declarations = svc.store().get_schema_declarations("assembly").await?;
    assert_eq!(declarations.len(), 1);
    let rel = &declarations[0];
    assert_eq!(rel.name, "widgets");
    assert_eq!(rel.target_type.as_deref(), Some("widget"));
    assert_eq!(rel.reverse_name, "assemblies");
    Ok(())
}

#[tokio::test]
async fn untyped_declaration_stores_as_self_edge_and_round_trips() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    handle_create_schema(
        &svc,
        json!({
            "name": "Notebook",
            "fields": [],
            "relationships": [{ "name": "related", "direction": "out", "cardinality": "many", "reverseName": "related_from", "reverseCardinality": "many" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    let schema = svc
        .get_schema_node("notebook")
        .await?
        .expect("notebook schema");
    assert_eq!(schema.relationships.len(), 1);
    assert_eq!(schema.relationships[0].name, "related");
    assert!(
        schema.relationships[0].target_type.is_none(),
        "untyped declaration must round-trip target_type: None"
    );

    // Untyped declarations apply to every target type via inbound resolution.
    let inbound = svc.get_inbound_relationships("task").await?;
    assert!(
        inbound
            .iter()
            .any(|(source, rel)| source == "notebook" && rel.name == "related"),
        "untyped declaration must surface as inbound for any type"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Read-path agreement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn all_read_paths_agree_on_the_declaration_set() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_widget_pair(&svc).await?;

    // Path 1: single-schema fetch.
    let single = svc
        .get_schema_node("assembly")
        .await?
        .expect("assembly schema")
        .relationships;

    // Path 2: all-schemas fetch (drives get_inbound_relationships and the
    // agent's relationship-hop context).
    let from_all = svc
        .get_all_schemas()
        .await?
        .into_iter()
        .find(|s| s.id == "assembly")
        .expect("assembly among all schemas")
        .relationships;

    assert_eq!(
        single, from_all,
        "single-schema and all-schemas reads drifted"
    );
    assert_eq!(single.len(), 1);

    // Path 3: inbound resolution from the target's side.
    let inbound = svc.get_inbound_relationships("widget").await?;
    let (source, inbound_rel) = inbound
        .iter()
        .find(|(source, rel)| source == "assembly" && rel.name == "widgets")
        .expect("assembly.widgets visible inbound from widget");
    assert_eq!(source, "assembly");
    assert_eq!(
        *inbound_rel, single[0],
        "inbound read drifted from outbound"
    );

    // Path 4: the relationship viewer's outbound aggregation.
    make_node(&svc, "a1", "assembly").await?;
    let viewer = rel_ops::get_node_relationships(&svc, "a1").await?;
    let group = viewer
        .groups
        .iter()
        .find(|g| g.relationship_name == "widgets" && g.direction == "out")
        .expect("declared outbound group renders even with zero edges");
    assert_eq!(group.target_type.as_deref(), Some("widget"));

    // Path 5: create_relationship validates against the same declaration set.
    make_node(&svc, "w1", "widget").await?;
    svc.create_relationship("a1", "widgets", "w1", json!({}))
        .await
        .expect("declared relationship accepted on the write path");
    let related = svc.get_related_nodes("a1", "widgets", "out").await?;
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].id, "w1");
    Ok(())
}

#[tokio::test]
async fn declaration_edges_do_not_leak_into_instance_queries() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_widget_pair(&svc).await?;
    make_node(&svc, "a1", "assembly").await?;
    make_node(&svc, "w1", "widget").await?;
    svc.create_relationship("a1", "widgets", "w1", json!({}))
        .await?;

    // The declaration edge (assembly→widget schema nodes) shares
    // relationship_type "widgets" with the instance edge — instance traversal
    // keyed by instance ids must see exactly the instance edge.
    let related = svc.get_related_nodes("a1", "widgets", "out").await?;
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].id, "w1");

    // And the viewer on the instance shows a count of 1, not 2.
    let viewer = rel_ops::get_node_relationships(&svc, "a1").await?;
    let group = viewer
        .groups
        .iter()
        .find(|g| g.relationship_name == "widgets" && g.direction == "out")
        .expect("outbound group");
    assert_eq!(group.count, 1);
    Ok(())
}

// ---------------------------------------------------------------------------
// Reserved names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reserved_builtin_names_are_rejected_at_declaration_time() -> Result<()> {
    let (svc, _t) = create_test_service().await?;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Bad",
            "fields": [],
            "relationships": [{ "name": "has_child", "direction": "out", "cardinality": "many", "reverseName": "parent", "reverseCardinality": "one" }]
        }),
    )
    .await
    .expect_err("a declaration named after a built-in structural relationship must be rejected");
    assert!(
        err.to_string().contains("has_child"),
        "error should name the reserved relationship: {err}"
    );
    // The rejection must not leave a half-created schema behind.
    assert!(svc.get_schema_node("bad").await?.is_none());

    // Same via update_schema on an existing schema.
    handle_create_schema(&svc, json!({ "name": "Good", "fields": [] }))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let err = handle_update_schema(
        &svc,
        json!({
            "schema_id": "good",
            "add_relationships": [{ "name": "mentions", "direction": "out", "cardinality": "many", "reverseName": "mentioned_by", "reverseCardinality": "many" }]
        }),
    )
    .await
    .expect_err("update_schema must reject reserved relationship names");
    assert!(err.to_string().contains("mentions"));
    Ok(())
}

/// A reserved name is rejected as a `reverseName` too, not only as the forward
/// `name`.
///
/// The failure differs from the forward case and is easy to wave off as
/// harmless: a reverse name is never written to `relationship_type` — it is a
/// resolution alias — so it cannot make stored edges ambiguous the way a
/// reserved forward name does. What it does instead is nothing at all.
/// `resolve_relationship_name` answers a built-in spelling from the built-in
/// table before it consults any declaration, so a `reverseName` of `has_child`
/// can never resolve to this relationship; the built-in wins and the author's
/// chosen reverse spelling is silently inert.
///
/// This matters more now that every relationship must carry a reverse name:
/// what used to be a sparsely-populated opt-in namespace now gains an entry per
/// declaration.
#[tokio::test]
async fn reserved_builtin_names_are_rejected_as_reverse_names() -> Result<()> {
    let (svc, _t) = create_test_service().await?;

    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Folder",
            "fields": [],
            "relationships": [{
                "name": "contains",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "has_child",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .expect_err("a reserved reverseName must be rejected");
    assert!(
        err.to_string().contains("reverseName"),
        "the error must say WHICH end is at fault, so the fix is unambiguous: {err}"
    );
    assert!(
        err.to_string().contains("has_child"),
        "error should name the reserved relationship: {err}"
    );
    // The rejection must not leave a half-created schema behind.
    assert!(svc.get_schema_node("folder").await?.is_none());

    // Same via update_schema on an existing schema.
    handle_create_schema(&svc, json!({ "name": "Tray", "fields": [] }))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let err = handle_update_schema(
        &svc,
        json!({
            "schema_id": "tray",
            "add_relationships": [{
                "name": "holds",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "member_of",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .expect_err("update_schema must reject a reserved reverseName");
    assert!(err.to_string().contains("member_of"), "got: {err}");

    // A non-reserved reverse name on the same shape is still accepted — the
    // guard must reject the reserved word, not the direction.
    handle_create_schema(
        &svc,
        json!({
            "name": "Crate",
            "fields": [],
            "relationships": [{
                "name": "contains",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "contained_by",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("a non-reserved reverseName must still be accepted: {e}"))?;

    Ok(())
}

/// A built-in's INVERSE spelling (`child_of`, `has_member`, …) is reserved too,
/// in both declaration positions.
///
/// These names were free to declare while nothing could traverse by them: a
/// resolver that only walked forward never consulted the inverse table, so
/// `reverseName: "child_of"` was merely unused. Now that a reverse name is
/// resolved — from the built-in table, ahead of any declaration — declaring one
/// would be accepted and then permanently shadowed, which is exactly the
/// "accepted as inert" failure the forward guard exists to prevent.
#[tokio::test]
async fn builtin_reverse_spellings_are_reserved_in_both_positions() -> Result<()> {
    let (svc, _t) = create_test_service().await?;

    // As a forward `name`.
    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Shelf",
            "fields": [],
            "relationships": [{
                "name": "child_of",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "shelves",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .expect_err("a built-in's inverse must be rejected as a forward name");
    assert!(err.to_string().contains("child_of"), "got: {err}");
    assert!(svc.get_schema_node("shelf").await?.is_none());

    // As a `reverseName`.
    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Bin",
            "fields": [],
            "relationships": [{
                "name": "holds",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "child_of",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .expect_err("a built-in's inverse must be rejected as a reverseName");
    assert!(err.to_string().contains("child_of"), "got: {err}");
    assert!(svc.get_schema_node("bin").await?.is_none());

    // Every inverse is covered, not just `child_of` — the guard reads the
    // built-in table rather than listing names, so a fifth built-in added later
    // is reserved on both sides without touching this check.
    for reverse in ["has_member", "mentioned_by", "role_of"] {
        let err = handle_create_schema(
            &svc,
            json!({
                "name": format!("Probe{reverse}"),
                "fields": [],
                "relationships": [{
                    "name": "links",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": reverse,
                    "reverseCardinality": "one"
                }]
            }),
        )
        .await
        .expect_err("every built-in inverse must be reserved");
        assert!(err.to_string().contains(reverse), "got: {err}");
    }

    Ok(())
}

/// The reservation and the resolver's ordering are one mechanism, and this ties
/// them together.
///
/// `resolve_relationship_name` answers a built-in inverse from the built-in
/// table BEFORE consulting the node's own schema. That early return is only
/// safe because no schema can declare such a name — otherwise it would shadow a
/// live declaration. Each half is tested on its own; this asserts the join, so
/// that relaxing the reservation cannot silently make the ordering wrong.
#[tokio::test]
async fn a_builtin_inverse_always_resolves_to_the_builtin() -> Result<()> {
    let (svc, _t) = create_test_service().await?;

    // The declaration that would shadow it is refused...
    handle_create_schema(
        &svc,
        json!({
            "name": "Carton",
            "fields": [],
            "relationships": [{
                "name": "child_of",
                "targetType": "carton",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "cartons",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .expect_err("precondition: a built-in inverse cannot be declared");

    // ...so `child_of` on a real node can only mean the built-in: the forward
    // name rewritten, the direction flipped.
    handle_create_schema(&svc, json!({ "name": "Carton", "fields": [] }))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let node = Node::new("carton".to_string(), "a carton".to_string(), json!({}));
    let node_id = node.id.clone();
    svc.create_node(node).await?;

    let resolved = rel_ops::resolve_relationship_name(&svc, &node_id, "carton", "child_of").await?;
    match resolved {
        rel_ops::ResolvedRelName::Reverse {
            forward_name,
            source_type,
        } => {
            assert_eq!(forward_name, "has_child");
            assert_eq!(
                source_type, None,
                "a built-in has no declaring schema, so results must not be narrowed by type"
            );
        }
        other => panic!("expected the built-in inverse, got {other:?}"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Block-by-default protection
// ---------------------------------------------------------------------------

#[tokio::test]
async fn removing_a_declaration_with_live_edges_is_blocked() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_widget_pair(&svc).await?;
    make_node(&svc, "a1", "assembly").await?;
    make_node(&svc, "w1", "widget").await?;
    svc.create_relationship("a1", "widgets", "w1", json!({}))
        .await?;

    let err = handle_update_schema(
        &svc,
        json!({ "schema_id": "assembly", "remove_relationships": ["widgets"] }),
    )
    .await
    .expect_err("removing a declaration with live instance edges must be blocked");
    assert!(
        err.to_string().contains("1 instance edge"),
        "error should name the live edge count: {err}"
    );

    // The declaration must still be intact (block, not partial apply).
    let schema = svc.get_schema_node("assembly").await?.expect("assembly");
    assert_eq!(schema.relationships.len(), 1);

    // After the edge is deleted, removal succeeds.
    svc.delete_relationship("a1", "widgets", "w1").await?;
    handle_update_schema(
        &svc,
        json!({ "schema_id": "assembly", "remove_relationships": ["widgets"] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("removal after clearing edges should succeed: {e}"))?;
    let schema = svc.get_schema_node("assembly").await?.expect("assembly");
    assert!(schema.relationships.is_empty());
    Ok(())
}

#[tokio::test]
async fn retargeting_a_declaration_with_live_edges_is_blocked() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_widget_pair(&svc).await?;
    handle_create_schema(&svc, json!({ "name": "Gear", "fields": [] }))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    make_node(&svc, "a1", "assembly").await?;
    make_node(&svc, "w1", "widget").await?;
    svc.create_relationship("a1", "widgets", "w1", json!({}))
        .await?;

    // remove + re-add under the same name with a different target = retarget.
    let err = handle_update_schema(
        &svc,
        json!({
            "schema_id": "assembly",
            "remove_relationships": ["widgets"],
            "add_relationships": [{
                "name": "widgets",
                "targetType": "gear",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "assemblies",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .expect_err("retargeting a declaration with live instance edges must be blocked");
    assert!(err.to_string().contains("retarget"), "got: {err}");

    // Declaration unchanged.
    let schema = svc.get_schema_node("assembly").await?.expect("assembly");
    assert_eq!(
        schema.relationships[0].target_type.as_deref(),
        Some("widget")
    );
    Ok(())
}

#[tokio::test]
async fn deleting_a_schema_with_declarations_is_blocked_on_both_ends() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_widget_pair(&svc).await?;

    // Declaring side.
    let err = svc
        .store()
        .delete_node("assembly", None)
        .await
        .expect_err("deleting the declaring schema must be blocked");
    assert!(
        err.to_string().contains("schema_has_declarations"),
        "got: {err}"
    );

    // Target side — the declaration edge points AT widget, and FK CASCADE
    // would silently destroy it.
    let err = svc
        .store()
        .delete_node("widget", None)
        .await
        .expect_err("deleting the target schema must be blocked");
    assert!(
        err.to_string().contains("schema_has_declarations"),
        "got: {err}"
    );

    // After the declaration is removed, both delete cleanly.
    handle_update_schema(
        &svc,
        json!({ "schema_id": "assembly", "remove_relationships": ["widgets"] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    svc.store().delete_node("widget", None).await?;
    svc.store().delete_node("assembly", None).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Instance/declaration write scoping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_relationship_refuses_schema_node_endpoints() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_widget_pair(&svc).await?;
    make_node(&svc, "w1", "widget").await?;

    // Schema node as source.
    let err = svc
        .create_relationship("assembly", "widgets", "w1", json!({}))
        .await
        .expect_err("schema node as relationship source must be refused");
    assert!(err.to_string().contains("schema node"), "got: {err}");

    // Schema node as target of an untyped relationship.
    handle_create_schema(
        &svc,
        json!({
            "name": "Board",
            "fields": [],
            "relationships": [{ "name": "pins", "direction": "out", "cardinality": "many", "reverseName": "pinned_on", "reverseCardinality": "many" }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    make_node(&svc, "b1", "board").await?;
    let err = svc
        .create_relationship("b1", "pins", "widget", json!({}))
        .await
        .expect_err("schema node as relationship target must be refused");
    assert!(err.to_string().contains("schema node"), "got: {err}");
    Ok(())
}

#[tokio::test]
async fn delete_relationship_refuses_to_delete_a_declaration_edge() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_widget_pair(&svc).await?;
    make_node(&svc, "a1", "assembly").await?;
    make_node(&svc, "w1", "widget").await?;
    svc.create_relationship("a1", "widgets", "w1", json!({}))
        .await?;

    // Addressing a declaration edge with the instance-edge delete API must be
    // refused — otherwise it would bypass the live-instance-edge protection
    // update_schema enforces and orphan the instance edge just created.
    let err = svc
        .delete_relationship("assembly", "widgets", "widget")
        .await
        .expect_err("deleting a declaration edge via delete_relationship must be refused");
    assert!(err.to_string().contains("declaration"), "got: {err}");

    // The declaration is intact and still validates instance work.
    let schema = svc.get_schema_node("assembly").await?.expect("assembly");
    assert_eq!(schema.relationships.len(), 1);
    Ok(())
}

#[tokio::test]
async fn update_relationship_properties_refuses_to_corrupt_a_declaration_edge() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_widget_pair(&svc).await?;

    // The declaration row's properties hold the authoritative
    // SchemaRelationship; overwriting them with edge-attribute JSON would make
    // the declaration unparseable and silently vanish from every read path.
    let err = svc
        .update_relationship_properties("assembly", "widgets", "widget", json!({"role": "x"}))
        .await
        .expect_err(
            "editing a declaration edge via update_relationship_properties must be refused",
        );
    assert!(err.to_string().contains("declaration"), "got: {err}");

    // Round-trip still intact.
    let schema = svc.get_schema_node("assembly").await?.expect("assembly");
    assert_eq!(schema.relationships.len(), 1);
    assert_eq!(schema.relationships[0].name, "widgets");
    Ok(())
}

/// An unknown or misspelled key inside a relationship entry must be rejected
/// at schema-save time, exactly like the same mistake in `fields[]` or at the
/// top level — not silently dropped, leaving the relationship created as
/// though it had been written correctly.
///
/// Two shapes that were genuinely silent before this fix: an entirely
/// unknown key, and a snake_case typo of a real *optional* field
/// (`target_type` instead of `targetType`) — the misspelled spelling isn't a
/// known field, so it used to vanish and `target_type` stayed silently unset
/// rather than becoming the type the caller intended. (A typo of a
/// *required* field with no correctly spelled counterpart present, e.g.
/// `reverse_name` alone with no `reverseName`, was already a hard error
/// before this fix too — serde reports "missing field `reverseName`"
/// regardless of `deny_unknown_fields`; see
/// `test_schema_relationship_requires_both_reverse_fields` in
/// `nodespace-types` for that floor.)
#[tokio::test]
async fn misspelled_relationship_key_is_rejected_not_silently_dropped() -> Result<()> {
    let (svc, _t) = create_test_service().await?;

    // An entirely bogus key, at create_schema time.
    let err = handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [],
            "relationships": [{
                "name": "billed_to",
                "targetType": "customer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "invoices",
                "reverseCardinality": "many",
                "bogusRel": 1
            }]
        }),
    )
    .await
    .expect_err("an unknown key in a relationship entry must be rejected");
    assert!(
        err.to_string().contains("bogusRel"),
        "error should name the offending key: {err}"
    );
    // The rejection must not leave a half-created schema behind.
    assert!(svc.get_schema_node("invoice").await?.is_none());

    // The realistic case: a snake_case typo of the optional `targetType`,
    // via update_schema's `add_relationships`. Every required field is
    // present and correctly spelled, so pre-fix this silently created the
    // relationship with `target_type` unset instead of the customer type the
    // caller intended.
    handle_create_schema(
        &svc,
        json!({
            "name": "Customer",
            "fields": []
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": []
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let err = handle_update_schema(
        &svc,
        json!({
            "schema_id": "invoice",
            "add_relationships": [{
                "name": "billed_to",
                "target_type": "customer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "invoices",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .expect_err("a snake_case typo of targetType must be rejected, not dropped");
    assert!(
        err.to_string().contains("target_type"),
        "error should name the offending key: {err}"
    );
    // No relationship was silently created off the bad entry.
    let schema = svc.get_schema_node("invoice").await?.expect("invoice");
    assert!(schema.relationships.is_empty());
    Ok(())
}
