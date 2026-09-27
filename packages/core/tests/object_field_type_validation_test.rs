//! `field_type` is declared but was never enforced except for `enum`: a field
//! declared `object` (or an `array` whose `itemType` is `object`) accepted any
//! JSON shape and stored it as-is, with no error and no warning.
//!
//! This covers the `object` shape: a field declared `object` holds a JSON
//! object, and an `array` field declared `itemType: "object"` holds an array
//! of JSON objects. It does NOT recurse into a declared `object` field's own
//! `fields`/`item_fields` sub-schema; see the comment on the validation itself
//! in `packages/core/src/services/node_service/crud.rs`. Scalar types are
//! covered by `scalar_field_type_validation_test.rs`.
//!
//! A survey of every writer of a declared `object`/`array<object>` field in
//! `core_schemas.rs` (`ai-chat.messages`, `query.filters`, `query.sorting`)
//! found no contradicting writer — every production write already sends a
//! correctly-shaped array of objects — so enabling this check is safe with no
//! accompanying writer fix needed, unlike the `ai-chat.status` precedent.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    ops::node_ops,
    schema::handle_create_schema,
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

async fn seed_widget_schema(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "Widget",
            "fields": [
                { "name": "address", "type": "object", "protection": "user", "indexed": false },
                {
                    "name": "notes",
                    "type": "array",
                    "itemType": "object",
                    "protection": "user",
                    "indexed": false
                }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("widget schema: {e}"))?;
    Ok(())
}

/// Like [`seed_widget_schema`], but `address` is `required` — for pinning the
/// interaction between the pre-existing required-field check and this new
/// object-shape check.
async fn seed_widget_schema_with_required_address(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "Req Widget",
            "fields": [
                {
                    "name": "address",
                    "type": "object",
                    "protection": "user",
                    "indexed": false,
                    "required": true
                }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("req-widget schema: {e}"))?;
    Ok(())
}

async fn create_req_widget(
    svc: &Arc<NodeService>,
    properties: serde_json::Value,
) -> Result<String> {
    let output = node_ops::create_node(
        svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "req_widget".to_string(),
            content: "Sprocket".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties,
            collections: vec![],
            collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await?;
    Ok(output.node_id)
}

async fn create_widget(svc: &Arc<NodeService>, properties: serde_json::Value) -> Result<String> {
    let output = node_ops::create_node(
        svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "widget".to_string(),
            content: "Sprocket".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties,
            collections: vec![],
            collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await?;
    Ok(output.node_id)
}

#[tokio::test]
async fn object_field_rejects_a_string_value() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;

    let err = create_widget(&svc, json!({ "address": "123 Main St" }))
        .await
        .expect_err("a string must not satisfy a field declared type object");

    let msg = err.to_string();
    assert!(
        msg.contains("address"),
        "error must name the field, got: {msg}"
    );
    assert!(
        msg.contains("object"),
        "error must name the declared type, got: {msg}"
    );
    assert!(
        msg.contains("string"),
        "error must say what was received, got: {msg}"
    );
    Ok(())
}

#[tokio::test]
async fn object_field_rejects_a_number_value() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;

    let err = create_widget(&svc, json!({ "address": 42 }))
        .await
        .expect_err("a number must not satisfy a field declared type object");
    let msg = err.to_string();
    assert!(
        msg.contains("address"),
        "error must name the field, got: {msg}"
    );
    assert!(
        msg.contains("object"),
        "error must name the declared type, got: {msg}"
    );
    assert!(
        msg.contains("number"),
        "error must say what was received, got: {msg}"
    );
    Ok(())
}

#[tokio::test]
async fn object_field_accepts_an_object_value() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;

    let id = create_widget(&svc, json!({ "address": { "city": "Berlin" } })).await?;

    let node = node_ops::get_node(&svc, node_ops::GetNodeInput { node_id: id }).await?;
    assert_eq!(
        node.get("properties").and_then(|p| p.get("address")),
        Some(&json!({ "city": "Berlin" })),
        "a correctly-shaped object must be accepted and persisted"
    );
    Ok(())
}

#[tokio::test]
async fn object_field_accepts_an_explicit_null() -> Result<()> {
    // Mirrors the existing enum-field behavior: an explicit `null` on a
    // non-required field is a legal "no value", not a type violation.
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;

    create_widget(&svc, json!({ "address": null })).await?;
    Ok(())
}

#[tokio::test]
async fn array_object_field_rejects_a_non_array_value() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;

    let err = create_widget(&svc, json!({ "notes": "not an array" }))
        .await
        .expect_err("a string must not satisfy a field declared array<object>");

    let msg = err.to_string();
    assert!(
        msg.contains("notes"),
        "error must name the field, got: {msg}"
    );
    assert!(
        msg.contains("array"),
        "error must name the declared type, got: {msg}"
    );
    assert!(
        msg.contains("string"),
        "error must say what was received, got: {msg}"
    );
    Ok(())
}

#[tokio::test]
async fn array_object_field_rejects_a_non_object_item() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;

    let err = create_widget(&svc, json!({ "notes": [{ "text": "ok" }, 5] }))
        .await
        .expect_err("a non-object array element must be rejected");

    let msg = err.to_string();
    assert!(
        msg.contains("notes"),
        "error must name the field, got: {msg}"
    );
    assert!(
        msg.contains('1'),
        "error should identify which item failed (index 1), got: {msg}"
    );
    Ok(())
}

#[tokio::test]
async fn array_object_field_accepts_an_array_of_objects() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;

    let notes = json!([{ "text": "first" }, { "text": "second" }]);
    let id = create_widget(&svc, json!({ "notes": notes.clone() })).await?;

    let node = node_ops::get_node(&svc, node_ops::GetNodeInput { node_id: id }).await?;
    assert_eq!(
        node.get("properties").and_then(|p| p.get("notes")),
        Some(&notes),
        "a correctly-shaped array of objects must be accepted and persisted"
    );
    Ok(())
}

/// The update path is wired through `validate_node_against_schema`, a
/// separate call site from create's (`crud.rs`) — but that function is a thin
/// wrapper that itself calls `validate_node_with_fields`, the same check
/// create uses. This pins that the wiring actually reaches the check on
/// update too, rather than asserting two independent implementations agree.
#[tokio::test]
async fn object_field_rejects_a_bad_value_on_update() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;
    let id = create_widget(&svc, json!({ "address": { "city": "Berlin" } })).await?;

    let err = node_ops::update_node(
        &svc,
        node_ops::UpdateNodeInput {
            node_id: id,
            version: None,
            node_type: None,
            content: None,
            properties: Some(json!({ "address": "not an object anymore" })),
            add_to_collections: vec![],
            add_to_collection_ids: vec![],
            remove_from_collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await
    .expect_err("an update writing a wrongly-typed value must be rejected too");

    assert!(err.to_string().contains("address"));
    Ok(())
}

/// An array item that is itself an array is not an object either — the
/// per-item check must reject it the same way it rejects a number or string
/// item, not treat "not a scalar" as "close enough."
#[tokio::test]
async fn array_object_field_rejects_a_nested_array_item() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;

    let err = create_widget(&svc, json!({ "notes": [{ "text": "ok" }, [1, 2, 3]] }))
        .await
        .expect_err("an array item must be rejected, not treated as object-like");

    let msg = err.to_string();
    assert!(
        msg.contains("notes"),
        "error must name the field, got: {msg}"
    );
    assert!(
        msg.contains('1'),
        "error should identify which item failed (index 1), got: {msg}"
    );
    Ok(())
}

/// An empty array vacuously satisfies "every element is an object" — this
/// pins that the loop over zero items does not itself error, since a naive
/// implementation could plausibly require at least one item.
#[tokio::test]
async fn array_object_field_accepts_an_empty_array() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema(&svc).await?;

    let id = create_widget(&svc, json!({ "notes": [] })).await?;

    let node = node_ops::get_node(&svc, node_ops::GetNodeInput { node_id: id }).await?;
    assert_eq!(
        node.get("properties").and_then(|p| p.get("notes")),
        Some(&json!([])),
        "an empty array must be accepted"
    );
    Ok(())
}

/// A required object field with the key entirely absent must still be
/// rejected by the pre-existing required-field check, which runs before this
/// fix's type check in the same field loop — this pins that the two checks
/// don't interfere with each other's error paths.
#[tokio::test]
async fn required_object_field_missing_entirely_is_rejected_by_the_required_check() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema_with_required_address(&svc).await?;

    let err = create_req_widget(&svc, json!({}))
        .await
        .expect_err("a required field with no value at all must be rejected");

    let msg = err.to_string();
    assert!(
        msg.contains("address") && msg.to_lowercase().contains("required"),
        "must be rejected as a missing required field, got: {msg}"
    );
    Ok(())
}

/// Documents a real, if narrow, gap: the pre-existing `required` check only
/// tests key-absence (`field_value.is_none()`), so an explicit `null` on a
/// required object field is NOT caught by it. This fix's object-shape check
/// then explicitly allows `null` too (mirroring the enum-field precedent, see
/// `object_field_accepts_an_explicit_null` above), so the two checks compound
/// into "required" being satisfiable by writing nothing at all. This test
/// documents today's actual behavior — it is not asserting this is desired,
/// only that it is not silently different from what a reader would expect
/// after reading `object_field_accepts_an_explicit_null`.
#[tokio::test]
async fn required_object_field_set_to_explicit_null_is_currently_accepted() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_widget_schema_with_required_address(&svc).await?;

    create_req_widget(&svc, json!({ "address": null }))
        .await
        .expect(
            "documents today's behavior: null on a required field is not caught by either check",
        );
    Ok(())
}

/// Documents a schema-authoring footgun this fix newly makes reachable:
/// `apply_schema_defaults_with_fields` inserts a declared `default` verbatim,
/// with no check that it matches the field's own declared `field_type` (that
/// check does not exist anywhere in `create_schema`/`update_schema` either).
/// Before this fix, a wrongly-typed default for an `object` field was written
/// and silently accepted. After this fix, the very next node creation that
/// omits the field gets the bad default inserted and then rejected by the new
/// check — bricking creation for the entire type until the schema is fixed.
///
/// None of the three shipped core schemas hit this (their object/array<object>
/// defaults are all correctly typed), so this is not an active bug — but it is
/// a real new failure mode for a future user-defined schema, and worth pinning
/// so a later change to either default-application or schema-creation-time
/// validation makes a deliberate decision about it rather than an accidental
/// one.
#[tokio::test]
async fn a_wrongly_typed_default_bricks_creation_for_omitted_fields() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    handle_create_schema(
        &svc,
        json!({
            "name": "Misconfigured",
            "fields": [{
                "name": "address",
                "type": "object",
                "protection": "user",
                "indexed": false,
                // Wrongly-typed default: declared `object`, but a string.
                "default": "oops"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("misconfigured schema: {e}"))?;

    let err = node_ops::create_node(
        &svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "misconfigured".to_string(),
            content: "x".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            // Omit `address` entirely so the bad default is what fills it in.
            properties: json!({}),
            collections: vec![],
            collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await
    .expect_err(
        "a wrongly-typed default, once applied, is caught by the same check a caller's \
         own value would be — documenting that this now bricks creation rather than \
         silently persisting the bad value",
    );

    assert!(err.to_string().contains("address"));
    Ok(())
}
