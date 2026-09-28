//! Does `create_node` persist the property values it is handed?
//!
//! The agent-matrix "log a laser cutter ... replacement cost 2400" scenario
//! reports `create_node` succeeding with zero persisted properties, and the
//! reported count is read back off the stored node rather than counted from the
//! request. That leaves two candidate culprits: the storage path silently
//! dropping values, or the model never supplying them.
//!
//! These tests pin the storage half so the question can only be answered one
//! way. Both cases matter, because the failing scenario supplies a value
//! (`replacement cost`) that the schema created one turn earlier does not
//! necessarily declare:
//!
//!   - a value for a field the schema DOES declare must persist, and
//!   - a value for a field the schema does NOT declare must ALSO persist,
//!     rather than being filtered out for being unknown.
//!
//! If the second case dropped the value, "schema omitted the field" and "model
//! omitted the value" would produce an identical zero count and the trace could
//! not distinguish them.

use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::ops::node_ops;
use nodespace_core::services::{InsertPositionOwned, NodeService};
use serde_json::json;
use std::sync::Arc;

async fn test_service() -> Result<Arc<NodeService>, Box<dyn std::error::Error>> {
    let temp_dir = std::env::temp_dir().join(format!("test_create_props_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)?;
    let store = SqliteStore::new(temp_dir.join("test.db")).await?;
    let mut store = Arc::new(store);
    Ok(Arc::new(NodeService::new(&mut store).await?))
}

/// Count the property values a caller could later resolve against — the same
/// question `exec_create_node`'s `property_count` answers, read off the stored
/// node rather than the request.
///
/// Production uses a plain `o.len()`: by that point `flatten_properties_for_api`
/// has already stripped underscore-prefixed internals (`_schema_version`), so no
/// filter is needed there. The filter here is belt-and-braces against that
/// flattening changing, not a mirror of production logic.
fn persisted_count(node_data: &serde_json::Value) -> usize {
    node_data
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|o| o.keys().filter(|k| !k.starts_with('_')).count())
        .unwrap_or(0)
}

/// Create the equipment schema the matrix's scenario 3 is meant to produce,
/// with `fields` naming exactly what the model chose to declare.
async fn seed_schema(
    ns: &Arc<NodeService>,
    fields: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let schema = Node::new_with_id(
        "equipment".to_string(),
        "schema".to_string(),
        "Equipment".to_string(),
        json!({ "fields": fields }),
    );
    ns.store().create_node(schema, None, None).await?;
    Ok(())
}

#[tokio::test]
async fn persists_values_for_declared_schema_fields() -> Result<(), Box<dyn std::error::Error>> {
    let ns = test_service().await?;
    seed_schema(
        &ns,
        json!([
            { "name": "status", "field_type": "text" },
            { "name": "replacement_cost", "field_type": "number" },
        ]),
    )
    .await?;

    let output = node_ops::create_node(
        &ns,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "equipment".to_string(),
            content: "Laser cutter".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: json!({ "status": "checked out", "replacement_cost": 2400 }),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await?;

    // Exact, not `>=`: `equipment` is user-defined with no defaulted fields, so
    // creation adds nothing of its own and the count is deterministic. A looser
    // bound would also pass if storage started injecting extra keys, which is
    // precisely the kind of drift these tests exist to catch.
    assert_eq!(
        persisted_count(&output.node_data),
        2,
        "declared field values must persist, got: {}",
        serde_json::to_string_pretty(&output.node_data)?
    );
    Ok(())
}

/// The scenario-4 shape: the model supplies `replacement_cost` but the schema
/// created a turn earlier never declared it. The value must still be stored —
/// an undeclared key is not a reason to discard what the user said.
#[tokio::test]
async fn persists_values_for_fields_the_schema_never_declared(
) -> Result<(), Box<dyn std::error::Error>> {
    let ns = test_service().await?;
    seed_schema(&ns, json!([{ "name": "status", "field_type": "text" }])).await?;

    let output = node_ops::create_node(
        &ns,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "equipment".to_string(),
            content: "Laser cutter".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            // `replacement_cost` is absent from the schema above.
            properties: json!({ "replacement_cost": 2400 }),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await?;

    assert_eq!(
        persisted_count(&output.node_data),
        1,
        "a value for an undeclared field must still persist, got: {}",
        serde_json::to_string_pretty(&output.node_data)?
    );
    Ok(())
}

/// The control: handed nothing, `create_node` stores nothing and reports zero.
/// This is the reading the failing scenario actually produces, and it is only
/// diagnostic if the two tests above hold — together they establish that a zero
/// count means the arguments were empty, not that storage ate them.
#[tokio::test]
async fn reports_zero_only_when_given_no_properties() -> Result<(), Box<dyn std::error::Error>> {
    let ns = test_service().await?;
    seed_schema(&ns, json!([{ "name": "status", "field_type": "text" }])).await?;

    let output = node_ops::create_node(
        &ns,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "equipment".to_string(),
            content: "Laser cutter".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: json!({}),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await?;

    assert_eq!(
        persisted_count(&output.node_data),
        0,
        "empty arguments should persist nothing, got: {}",
        serde_json::to_string_pretty(&output.node_data)?
    );
    Ok(())
}

/// The other mechanism behind a zero-property `create_node`: the model invents
/// a `node_type` (a display name, a paraphrase) instead of copying a real
/// schema id. Before this test, that call SUCCEEDED — CustomNodeBehavior
/// accepts any type string, so the node was stored as a bare shell and the
/// model was told the write succeeded. It must now be rejected loudly instead,
/// so the model can retry against a real id rather than lose the write.
#[tokio::test]
async fn rejects_node_type_with_no_schema_and_no_core_behavior(
) -> Result<(), Box<dyn std::error::Error>> {
    let ns = test_service().await?;
    // The real id ("equipment") is never seeded — only the invented display
    // name is attempted, matching the scenario 4 trace where the schema was
    // created as "equipment_item" but create_node was called with "Equipment
    // Item Tracker".
    let err = node_ops::create_node(
        &ns,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "Equipment Item Tracker".to_string(),
            content: "Laser cutter".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: json!({ "replacement_cost": 2400 }),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await
    .expect_err("an invented node_type with no schema must be rejected, not silently accepted");

    let msg = err.to_string();
    assert!(
        msg.contains("Equipment Item Tracker"),
        "error should name the offending node_type, got: {msg}"
    );
    Ok(())
}

/// A `node_type` matching a registered core behavior (e.g. "text") must still
/// be accepted even though no schema node exists for it — core types are
/// validated by behavior, not by a schema row.
#[tokio::test]
async fn accepts_core_node_type_with_no_schema_node() -> Result<(), Box<dyn std::error::Error>> {
    let ns = test_service().await?;

    let output = node_ops::create_node(
        &ns,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "text".to_string(),
            content: "A plain note".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: json!({}),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        },
    )
    .await?;

    assert_eq!(output.node_type, "text");
    Ok(())
}
