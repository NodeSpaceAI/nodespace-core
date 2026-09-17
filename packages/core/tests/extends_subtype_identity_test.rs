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
    create_instance(&svc, "regression", json!({ "status": "open", "found_in": "1.2" })).await;

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

    assert_eq!(flat["status"], "open", "inherited field visible at own scope");
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
    let flat = nodespace_types::flatten_namespaced_properties_at_scope(&node.properties, &["ticket"]);

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
