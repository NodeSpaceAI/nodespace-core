//! End-to-end tests for `extends` subtype identity (ADR-078).
//!
//! Exercises the whole path against a real store: creating an instance of an
//! extending schema, where its fields physically land, what a base-scoped
//! query returns, and what each scope projects to.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::schema::{handle_create_schema, handle_update_schema};
use nodespace_core::services::NodeService;
use nodespace_types::NodeQuery;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn test_service() -> (Arc<NodeService>, TempDir) {
    let temp_dir = TempDir::new().expect("tempdir creation failed");
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(
        SqliteStore::new(db_path)
            .await
            .expect("SqliteStore init failed"),
    );
    let node_service = Arc::new(
        NodeService::new(&mut store)
            .await
            .expect("NodeService init failed"),
    );
    (node_service, temp_dir)
}

/// `ticket` (status) <- `bug` (severity). The minimal two-level chain every
/// test below builds on.
async fn seed_ticket_and_bug(svc: &Arc<NodeService>) {
    handle_create_schema(
        svc,
        json!({
            "name": "Ticket",
            "fields": [
                { "name": "status", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("ticket schema creation failed");

    handle_create_schema(
        svc,
        json!({
            "name": "Bug",
            "extends": "ticket",
            "fields": [
                { "name": "severity", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("bug schema creation failed");
}

async fn create_instance(
    svc: &Arc<NodeService>,
    node_type: &str,
    properties: serde_json::Value,
) -> String {
    let node = Node::new(node_type.to_string(), format!("a {node_type}"), properties);
    svc.create_node(node)
        .await
        .unwrap_or_else(|e| panic!("creating a {node_type} node failed: {e:?}"))
}

#[tokio::test]
async fn instance_of_an_extending_schema_carries_its_own_node_type() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;

    let id = create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;
    let node = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // The central commitment of ADR-078: node_type tells the truth about what
    // the node is, rather than reporting the base type.
    assert_eq!(node.node_type, "bug");
}

#[tokio::test]
async fn inherited_and_own_fields_land_in_their_declaring_schemas_buckets() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;

    let id = create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;
    let node = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // Provenance is what makes scope projection possible: an inherited field
    // must be stored under the schema that declares it, not the node's type.
    assert_eq!(
        node.properties["ticket"]["status"], "open",
        "an inherited field belongs in its declaring ancestor's bucket, got {:?}",
        node.properties
    );
    assert_eq!(
        node.properties["bug"]["severity"], "high",
        "an own field belongs in the node's own bucket, got {:?}",
        node.properties
    );
}

#[tokio::test]
async fn unextended_type_keeps_its_single_bucket() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;

    let id = create_instance(&svc, "ticket", json!({ "status": "open" })).await;
    let node = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // Regression guard: the unextended path must be byte-identical to before.
    assert_eq!(node.properties["ticket"]["status"], "open");
    assert!(
        node.properties.get("bug").is_none(),
        "an unextended node gains no extra buckets, got {:?}",
        node.properties
    );
}

#[tokio::test]
async fn querying_a_base_type_returns_its_subtypes_instances() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;

    create_instance(&svc, "ticket", json!({ "status": "open" })).await;
    create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    let results = svc
        .query_nodes_simple(NodeQuery {
            node_type: Some("ticket".to_string()),
            ..Default::default()
        })
        .await
        .expect("query failed");

    let types: Vec<&str> = results.iter().map(|n| n.node_type.as_str()).collect();
    assert!(
        types.contains(&"ticket") && types.contains(&"bug"),
        "a base-type query should match its subtypes too, got {types:?}"
    );
}

#[tokio::test]
async fn querying_a_leaf_type_returns_only_that_type() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;

    create_instance(&svc, "ticket", json!({ "status": "open" })).await;
    create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    let results = svc
        .query_nodes_simple(NodeQuery {
            node_type: Some("bug".to_string()),
            ..Default::default()
        })
        .await
        .expect("query failed");

    let types: Vec<&str> = results.iter().map(|n| n.node_type.as_str()).collect();
    assert!(
        types.iter().all(|t| *t == "bug"),
        "expansion is additive, not lossy — a leaf query stays exact, got {types:?}"
    );
    assert_eq!(types.len(), 1, "exactly the one bug instance");
}

#[tokio::test]
async fn transitive_subtypes_are_matched_through_the_whole_chain() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    handle_create_schema(
        &svc,
        json!({
            "name": "Regression",
            "extends": "bug",
            "fields": [
                { "name": "found_in", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("regression schema creation failed");

    create_instance(&svc, "ticket", json!({ "status": "open" })).await;
    create_instance(
        &svc,
        "regression",
        json!({ "status": "open", "found_in": "1.2" }),
    )
    .await;

    let results = svc
        .query_nodes_simple(NodeQuery {
            node_type: Some("ticket".to_string()),
            ..Default::default()
        })
        .await
        .expect("query failed");

    let types: Vec<&str> = results.iter().map(|n| n.node_type.as_str()).collect();
    assert!(
        types.contains(&"regression"),
        "the closure must be transitive, not one level deep, got {types:?}"
    );
}

#[tokio::test]
async fn own_scope_projection_shows_inherited_and_own_fields() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    let id = create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    let node = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");
    let chain = svc
        .resolve_type_chain("bug")
        .await
        .expect("chain resolution failed");
    let scopes: Vec<&str> = chain.iter().map(String::as_str).collect();

    let flat = nodespace_types::flatten_namespaced_properties_at_scope(&node.properties, &scopes);

    assert_eq!(
        flat["status"], "open",
        "inherited field visible at own scope"
    );
    assert_eq!(flat["severity"], "high", "own field visible at own scope");
}

#[tokio::test]
async fn base_scope_projection_hides_the_subtypes_own_fields() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    let id = create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    let node = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // Reading the same node at the BASE type's scope — what a
    // `node_type: "ticket"` query projects its results to.
    let flat =
        nodespace_types::flatten_namespaced_properties_at_scope(&node.properties, &["ticket"]);

    assert_eq!(flat["status"], "open", "the base's own field is visible");
    assert!(
        flat.get("severity").is_none(),
        "a subtype's own field must be ABSENT at base scope, not merely unresolved — got {flat:?}"
    );
}

#[tokio::test]
async fn required_inherited_field_is_validated_on_the_subtype() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [{
                "name": "status",
                "type": "string",
                "protection": "user",
                "indexed": false,
                "required": true
            }]
        }),
    )
    .await
    .expect("ticket schema creation failed");
    handle_create_schema(
        &svc,
        json!({ "name": "Bug", "extends": "ticket", "fields": [] }),
    )
    .await
    .expect("bug schema creation failed");

    // Omitting the inherited required field must fail. Reading only the node's
    // own bucket would report it missing too — but reading only the SUBTYPE's
    // own field list would never check it at all, which is the regression this
    // guards.
    let node = Node::new("bug".to_string(), "a bug".to_string(), json!({}));
    let result = svc.create_node(node).await;

    assert!(
        result.is_err(),
        "a missing inherited required field should be rejected"
    );
}

#[tokio::test]
async fn inherited_field_default_is_applied_and_correctly_bucketed() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [{
                "name": "status",
                "type": "string",
                "protection": "user",
                "indexed": false,
                "default": "open"
            }]
        }),
    )
    .await
    .expect("ticket schema creation failed");
    handle_create_schema(
        &svc,
        json!({
            "name": "Bug",
            "extends": "ticket",
            "fields": [
                { "name": "severity", "type": "string", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .expect("bug schema creation failed");

    let id = create_instance(&svc, "bug", json!({ "severity": "high" })).await;
    let node = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // The default is applied into the node's own bucket first, then re-bucketed
    // into its declaring ancestor's — so this asserts the ordering inside the
    // write path, not just that a default appeared somewhere.
    assert_eq!(
        node.properties["ticket"]["status"], "open",
        "an inherited field's default must land in the declaring schema's bucket, got {:?}",
        node.properties
    );
}

#[tokio::test]
async fn parent_enum_value_added_later_validates_on_an_existing_subtype() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [{
                "name": "state",
                "type": "enum",
                "protection": "user",
                "indexed": false,
                "extensible": true,
                "coreValues": [{ "value": "open", "label": "Open" }]
            }]
        }),
    )
    .await
    .expect("ticket schema creation failed");
    handle_create_schema(
        &svc,
        json!({ "name": "Bug", "extends": "ticket", "fields": [] }),
    )
    .await
    .expect("bug schema creation failed");

    // Before the value exists, it fails enum validation.
    let before = svc
        .create_node(Node::new(
            "bug".to_string(),
            "early".to_string(),
            json!({ "state": "blocked" }),
        ))
        .await;
    assert!(
        before.is_err(),
        "a value absent from the inherited enum should be rejected"
    );

    handle_update_schema(
        &svc,
        json!({
            "schema_id": "ticket",
            "add_field_values": [{
                "field": "state",
                "values": [{ "value": "blocked", "label": "Blocked" }]
            }]
        }),
    )
    .await
    .expect("add_field_values on the parent should succeed");

    // Parents are read live, so the subtype accepts it with no write of its own.
    let after = svc
        .create_node(Node::new(
            "bug".to_string(),
            "later".to_string(),
            json!({ "state": "blocked" }),
        ))
        .await;
    assert!(
        after.is_ok(),
        "a value appended to the parent should validate on the subtype with no rewrite: {after:?}"
    );
}

#[tokio::test]
async fn base_scoped_query_results_project_to_the_base_scope() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    let nodes = svc
        .query_nodes_simple(NodeQuery {
            node_type: Some("ticket".to_string()),
            ..Default::default()
        })
        .await
        .expect("query failed");
    let projected = svc
        .project_nodes_to_scope(nodes, Some("ticket"))
        .await
        .expect("projection failed");

    let bug = projected
        .iter()
        .find(|n| n.node_type == "bug")
        .expect("the bug instance should be in a ticket-scoped result set");

    assert!(
        bug.properties.get("ticket").is_some(),
        "the base's bucket survives projection, got {:?}",
        bug.properties
    );
    assert!(
        bug.properties.get("bug").is_none(),
        "the subtype's own bucket must be dropped at base scope, got {:?}",
        bug.properties
    );
}

#[tokio::test]
async fn leaf_scoped_query_results_keep_every_bucket() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    let nodes = svc
        .query_nodes_simple(NodeQuery {
            node_type: Some("bug".to_string()),
            ..Default::default()
        })
        .await
        .expect("query failed");
    let projected = svc
        .project_nodes_to_scope(nodes, Some("bug"))
        .await
        .expect("projection failed");

    let bug = projected.first().expect("one bug instance");
    assert!(
        bug.properties.get("ticket").is_some() && bug.properties.get("bug").is_some(),
        "at its own scope a node keeps its whole chain, got {:?}",
        bug.properties
    );
}

#[tokio::test]
async fn projection_leaves_unextended_results_untouched() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    let id = create_instance(&svc, "ticket", json!({ "status": "open" })).await;

    let before = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");
    let projected = svc
        .project_nodes_to_scope(vec![before.clone()], Some("ticket"))
        .await
        .expect("projection failed");

    assert_eq!(
        projected[0].properties, before.properties,
        "projecting an exact-type match must be the identity"
    );
}

#[tokio::test]
async fn query_without_a_type_filter_is_not_projected() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    let id = create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    let node = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");
    let projected = svc
        .project_nodes_to_scope(vec![node.clone()], None)
        .await
        .expect("projection failed");

    assert_eq!(
        projected[0].properties, node.properties,
        "with no queried type there is no scope to project to"
    );
}

#[tokio::test]
async fn an_unprojected_read_round_trips_without_losing_fields() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    let id = create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    // The reason projection is not applied inside query_nodes_simple: an
    // internal caller reads, mutates and writes a node back. If that read were
    // projected, the write would silently drop every field outside the scope.
    let node = svc
        .query_nodes_simple(NodeQuery {
            node_type: Some("ticket".to_string()),
            ..Default::default()
        })
        .await
        .expect("query failed")
        .into_iter()
        .find(|n| n.node_type == "bug")
        .expect("the bug instance should be found");

    // Write the node's own properties back verbatim, exactly as a
    // read-modify-write caller would.
    svc.update_node_unchecked(
        &node.id,
        nodespace_core::models::NodeUpdate {
            content: Some("edited".to_string()),
            properties: Some(node.properties.clone()),
            ..Default::default()
        },
    )
    .await
    .expect("update failed");

    let after = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    assert_eq!(
        after.properties["bug"]["severity"], "high",
        "a read-modify-write through an internal query must not drop out-of-scope \
         fields, got {:?}",
        after.properties
    );
    assert_eq!(after.properties["ticket"]["status"], "open");
    assert_eq!(after.content, "edited");
}

// ============================================================================
// Write-path regressions found in review of PR #2739
// ============================================================================

#[tokio::test]
async fn updating_an_inherited_field_writes_to_its_declaring_bucket() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    let id = create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    let node = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // A flat update naming an INHERITED field, which is how every real caller
    // writes one (`--property status=done`, NodeUpdate.properties).
    svc.update_node_unchecked(
        &id,
        nodespace_core::models::NodeUpdate {
            properties: Some(json!({ "status": "done" })),
            ..Default::default()
        },
    )
    .await
    .expect("update failed");

    let after = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // The field must live in exactly one bucket. Without re-bucketing on this
    // path the new value lands in `bug` while the stale one stays in `ticket`,
    // and the readers disagree about which wins.
    assert_eq!(
        after.properties["ticket"]["status"], "done",
        "an inherited field updates in its declaring bucket, got {:?}",
        after.properties
    );
    assert!(
        after.properties["bug"].get("status").is_none(),
        "the field must not be duplicated into the node's own bucket, got {:?}",
        after.properties
    );
    assert_eq!(
        after.properties["bug"]["severity"], "high",
        "the node's own field is untouched"
    );
    assert_eq!(node.node_type, "bug");
}

#[tokio::test]
async fn a_type_change_preserves_an_inherited_fields_existing_value() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [{
                "name": "status",
                "type": "string",
                "protection": "user",
                "indexed": false,
                "default": "open"
            }]
        }),
    )
    .await
    .expect("ticket schema creation failed");
    handle_create_schema(
        &svc,
        json!({ "name": "Bug", "extends": "ticket", "fields": [] }),
    )
    .await
    .expect("bug schema creation failed");

    // A ticket whose status the user has already moved off the default.
    let id = create_instance(&svc, "ticket", json!({ "status": "done" })).await;

    svc.update_node_unchecked(
        &id,
        nodespace_core::models::NodeUpdate {
            node_type: Some("bug".to_string()),
            ..Default::default()
        },
    )
    .await
    .expect("type change failed");

    let after = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // Defaulting must judge "missing" across every bucket. Checking only the
    // node's own bucket reads the inherited value as absent, applies the
    // default, and re-bucketing then overwrites the real value with it.
    assert_eq!(
        after.properties["ticket"]["status"], "done",
        "a type change must not overwrite an existing inherited value with the \
         schema default, got {:?}",
        after.properties
    );
}

#[tokio::test]
async fn a_dormant_bucket_does_not_satisfy_a_required_inherited_field() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [{
                "name": "status",
                "type": "string",
                "protection": "user",
                "indexed": false,
                "required": true
            }]
        }),
    )
    .await
    .expect("ticket schema creation failed");
    handle_create_schema(
        &svc,
        json!({ "name": "Bug", "extends": "ticket", "fields": [] }),
    )
    .await
    .expect("bug schema creation failed");

    // A create whose only copy of the required inherited field sits in a
    // bucket outside the type's chain. `normalize_flat_properties_to_namespace`
    // leaves an explicit sibling namespace alone (it only moves *flat* keys),
    // so this reaches validation with `status` present but unreachable — the
    // shape a previous node_type change leaves behind.
    let node = Node::new(
        "bug".to_string(),
        "a bug".to_string(),
        json!({ "bug": {}, "dormant": { "status": "stale" } }),
    );
    let result = svc.create_node(node).await;

    assert!(
        result.is_err(),
        "a value in a bucket outside the type's chain must not satisfy a \
         required field"
    );
}

#[tokio::test]
async fn the_wire_flattener_keeps_inherited_fields() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    let id = create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    let node = svc
        .get_node(&id)
        .await
        .expect("get_node failed")
        .expect("node should exist");

    // The wire conversion has no store access and flattens one bucket, so the
    // service layer collapses the chain into the node's own bucket first.
    // Without that, every inherited field is dropped on every typed read.
    let collapsed = svc
        .collapse_chain_for_wire(vec![node])
        .await
        .expect("collapse failed")
        .remove(0);
    let wire = nodespace_types::node_to_typed_value(collapsed).expect("conversion failed");

    assert_eq!(
        wire["properties"]["status"], "open",
        "an inherited field must survive the wire flattener, got {:?}",
        wire["properties"]
    );
    assert_eq!(wire["properties"]["severity"], "high");
}

#[tokio::test]
async fn a_projected_node_flattens_to_exactly_its_scope() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;
    create_instance(&svc, "bug", json!({ "status": "open", "severity": "high" })).await;

    // Project at base scope first, as the daemon's read RPCs do, then flatten.
    // The two compose: projection drops out-of-scope buckets, so flattening
    // everything that remains yields exactly the scope's fields.
    let nodes = svc
        .query_nodes_simple(NodeQuery {
            node_type: Some("ticket".to_string()),
            ..Default::default()
        })
        .await
        .expect("query failed");
    let projected = svc
        .project_nodes_to_scope(nodes, Some("ticket"))
        .await
        .expect("projection failed");
    let bug = projected
        .into_iter()
        .find(|n| n.node_type == "bug")
        .expect("the bug should be in a ticket-scoped result");

    let collapsed = svc
        .collapse_chain_for_wire(vec![bug])
        .await
        .expect("collapse failed")
        .remove(0);
    let wire = nodespace_types::node_to_typed_value(collapsed).expect("conversion failed");

    assert_eq!(wire["properties"]["status"], "open");
    assert!(
        wire["properties"].get("severity").is_none(),
        "projection + flattening must still hide the subtype's own field, got {:?}",
        wire["properties"]
    );
}

// ============================================================================
// maps_to — enum-value extension and scope-relative resolution (ADR-078)
// ============================================================================

/// `ticket.state` is an extensible enum; `bug` extends it and adds `backlog`
/// mapping to `open`.
async fn seed_maps_to_chain(svc: &Arc<NodeService>) {
    handle_create_schema(
        svc,
        json!({
            "name": "Ticket",
            "fields": [{
                "name": "state",
                "type": "enum",
                "protection": "user",
                "indexed": false,
                "extensible": true,
                "coreValues": [
                    { "value": "open", "label": "Open" },
                    { "value": "done", "label": "Done" }
                ]
            }]
        }),
    )
    .await
    .expect("ticket schema creation failed");

    handle_create_schema(
        svc,
        json!({ "name": "Bug", "extends": "ticket", "fields": [] }),
    )
    .await
    .expect("bug schema creation failed");

    handle_update_schema(
        svc,
        json!({
            "schema_id": "bug",
            "add_field_values": [{
                "field": "state",
                "values": [{ "value": "backlog", "label": "Backlog", "mapsTo": "open" }]
            }]
        }),
    )
    .await
    .expect("extending the inherited enum should succeed");
}

#[tokio::test]
async fn adding_to_an_inherited_field_without_maps_to_is_rejected() {
    let (svc, _tmp) = test_service().await;
    seed_maps_to_chain(&svc).await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "bug",
            "add_field_values": [{
                "field": "state",
                "values": [{ "value": "triage", "label": "Triage" }]
            }]
        }),
    )
    .await;

    let err = result.expect_err("a value with no mapsTo on an inherited field must be rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("mapsTo"),
        "the error should name the missing key so the caller can fix it: {msg}"
    );
}

#[tokio::test]
async fn a_maps_to_naming_a_nonexistent_value_is_rejected() {
    let (svc, _tmp) = test_service().await;
    seed_maps_to_chain(&svc).await;

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "bug",
            "add_field_values": [{
                "field": "state",
                "values": [{ "value": "triage", "label": "Triage", "mapsTo": "nonexistent" }]
            }]
        }),
    )
    .await;

    let err = result.expect_err("mapsTo must name a value the field already has");
    assert!(
        format!("{err:?}").contains("nonexistent"),
        "the error should name the bad target: {err:?}"
    );
}

#[tokio::test]
async fn a_schemas_own_field_needs_no_maps_to() {
    let (svc, _tmp) = test_service().await;
    seed_maps_to_chain(&svc).await;

    // `severity` is declared by `bug` itself, so it has no ancestor scope
    // whose meaning needs preserving — mapsTo is neither required nor
    // meaningful. Regression guard for the non-inherited path.
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "bug",
            "add_fields": [{
                "name": "severity",
                "type": "enum",
                "protection": "user",
                "indexed": false,
                "extensible": true,
                "coreValues": [{ "value": "low", "label": "Low" }]
            }]
        }),
    )
    .await
    .expect("adding an own field should succeed");

    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "bug",
            "add_field_values": [{
                "field": "severity",
                "values": [{ "value": "high", "label": "High" }]
            }]
        }),
    )
    .await;

    assert!(
        result.is_ok(),
        "a value added to the schema's OWN field needs no mapsTo: {result:?}"
    );
}

#[tokio::test]
async fn a_base_scoped_query_matches_an_extended_value_through_maps_to() {
    let (svc, _tmp) = test_service().await;
    seed_maps_to_chain(&svc).await;

    create_instance(&svc, "ticket", json!({ "state": "open" })).await;
    create_instance(&svc, "bug", json!({ "state": "backlog" })).await;

    // The criterion's own case: a filter authored against the BASE type, using
    // the base's vocabulary, must match a subtype instance storing the
    // extended value that maps to it.
    let filter = nodespace_core::models::NodeFilter {
        node_type: Some("ticket".to_string()),
        property_filters: Some(vec![nodespace_core::models::PropertyFilter::new(
            "$.state".to_string(),
            nodespace_core::models::FilterOperator::Equals,
            json!("open"),
        )
        .expect("filter construction failed")]),
        ..Default::default()
    };

    let results = svc.query_nodes(filter).await.expect("query failed");
    let types: Vec<&str> = results.iter().map(|n| n.node_type.as_str()).collect();

    assert!(
        types.contains(&"bug"),
        "a ticket-scoped `state == open` filter should match a bug storing \
         `backlog`, which maps to open — got {types:?}"
    );
    assert!(
        types.contains(&"ticket"),
        "and must still match the plain ticket — got {types:?}"
    );
}

#[tokio::test]
async fn a_native_scoped_query_sees_the_raw_extended_value() {
    let (svc, _tmp) = test_service().await;
    seed_maps_to_chain(&svc).await;
    create_instance(&svc, "bug", json!({ "state": "backlog" })).await;

    // Reading at the node's own scope is already native — nothing to resolve,
    // so the stored value is what a bug-scoped filter compares against.
    let raw = nodespace_core::models::NodeFilter {
        node_type: Some("bug".to_string()),
        property_filters: Some(vec![nodespace_core::models::PropertyFilter::new(
            "$.state".to_string(),
            nodespace_core::models::FilterOperator::Equals,
            json!("backlog"),
        )
        .expect("filter construction failed")]),
        ..Default::default()
    };
    assert_eq!(
        svc.query_nodes(raw).await.expect("query failed").len(),
        1,
        "a bug-scoped filter matches the raw stored value"
    );

    // And the base-scope value must NOT match at the subtype's own scope: the
    // resolution is scope-relative, not a global rewrite.
    let resolved = nodespace_core::models::NodeFilter {
        node_type: Some("bug".to_string()),
        property_filters: Some(vec![nodespace_core::models::PropertyFilter::new(
            "$.state".to_string(),
            nodespace_core::models::FilterOperator::Equals,
            json!("open"),
        )
        .expect("filter construction failed")]),
        ..Default::default()
    };
    assert_eq!(
        svc.query_nodes(resolved).await.expect("query failed").len(),
        0,
        "at its own scope the node reads `backlog`, not the value it maps to"
    );
}

#[tokio::test]
async fn extending_a_non_extensible_inherited_field_is_rejected() {
    let (svc, _tmp) = test_service().await;
    handle_create_schema(
        &svc,
        json!({
            "name": "Ticket",
            "fields": [{
                "name": "kind",
                "type": "enum",
                "protection": "user",
                "indexed": false,
                "coreValues": [{ "value": "a", "label": "A" }]
            }]
        }),
    )
    .await
    .expect("ticket schema creation failed");
    handle_create_schema(
        &svc,
        json!({ "name": "Bug", "extends": "ticket", "fields": [] }),
    )
    .await
    .expect("bug schema creation failed");

    // The extensible gate applies through inheritance exactly as it does
    // directly — extending a closed vocabulary is no more legal via a subtype.
    let result = handle_update_schema(
        &svc,
        json!({
            "schema_id": "bug",
            "add_field_values": [{
                "field": "kind",
                "values": [{ "value": "b", "label": "B", "mapsTo": "a" }]
            }]
        }),
    )
    .await;

    assert!(
        result.is_err(),
        "a non-extensible inherited field must not be extendable"
    );
}

#[tokio::test]
async fn fields_on_an_extending_schema_are_stored_bare() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;

    // ADR-063 requires a namespace prefix when extending a type you don't own.
    // A schema's own fields are exempt, and an extending schema's own fields
    // are its own — they live in its own bucket and cannot collide with the
    // base type's future core fields.
    let schema = svc
        .get_schema_node("bug")
        .await
        .expect("schema lookup failed")
        .expect("bug schema should exist");

    assert!(
        schema.fields.iter().any(|f| f.name == "severity"),
        "an extending schema's own field is stored bare, with no prefix — got {:?}",
        schema.fields.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_second_extends_declaration_is_structurally_impossible() {
    let (svc, _tmp) = test_service().await;
    create_instance_schemas(&svc).await;

    // Single-parent is enforced by shape, not by a runtime check: `extends` is
    // one scalar key, so a second parent is unexpressible in the request. The
    // criterion asks for rejection; this pins the mechanism that makes
    // rejection unnecessary — and would fail if `extends` ever became a list.
    let result = handle_create_schema(
        &svc,
        json!({
            "name": "Multi",
            "extends": ["alpha", "beta"],
            "fields": []
        }),
    )
    .await;

    assert!(
        result.is_err(),
        "an array of parents must not deserialize into the scalar extends key"
    );

    // And re-targeting replaces rather than appends, so a schema never
    // accumulates two edges.
    handle_create_schema(
        &svc,
        json!({ "name": "Child", "extends": "alpha", "fields": [] }),
    )
    .await
    .expect("child extends alpha should succeed");
    handle_update_schema(&svc, json!({ "schema_id": "child", "extends": "beta" }))
        .await
        .expect("re-target should succeed");

    let schema = svc
        .get_schema_node("child")
        .await
        .expect("schema lookup failed")
        .expect("child should exist");
    let extends_edges = schema
        .relationships
        .iter()
        .filter(|r| r.name == "extends")
        .count();
    assert_eq!(
        extends_edges, 1,
        "re-targeting must replace the edge, never accumulate a second"
    );
}

/// Two unrelated base schemas, for the single-parent test.
async fn create_instance_schemas(svc: &Arc<NodeService>) {
    for name in ["Alpha", "Beta"] {
        handle_create_schema(
            svc,
            json!({
                "name": name,
                "fields": [
                    { "name": "note", "type": "string", "protection": "user", "indexed": false }
                ]
            }),
        )
        .await
        .unwrap_or_else(|e| panic!("{name} schema creation failed: {e:?}"));
    }
}

#[tokio::test]
async fn a_title_search_scoped_to_a_base_type_finds_subtype_instances() {
    let (svc, _tmp) = test_service().await;
    seed_ticket_and_bug(&svc).await;

    // Titles are computed from content here, so content is what the stem
    // search matches against.
    svc.create_node(Node::new(
        "bug".to_string(),
        "kubernetes deployment failure".to_string(),
        json!({ "status": "open", "severity": "high" }),
    ))
    .await
    .expect("bug creation failed");

    // `query_nodes` falls back to title-stem matching when the exact
    // substring search finds nothing. That fallback path takes node_type
    // separately from the main query, so it needs the same subtype expansion
    // or a base-scoped title search silently narrows to exact matches.
    let results = svc
        .query_nodes_simple(NodeQuery {
            node_type: Some("ticket".to_string()),
            title_contains: Some("deployments".to_string()),
            ..Default::default()
        })
        .await
        .expect("query failed");

    assert!(
        results.iter().any(|n| n.node_type == "bug"),
        "a base-scoped title search must reach subtype instances through the \
         stem fallback, got {:?}",
        results
            .iter()
            .map(|n| (&n.node_type, &n.content))
            .collect::<Vec<_>>()
    );
}
