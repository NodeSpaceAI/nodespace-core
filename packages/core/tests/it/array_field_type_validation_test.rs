//! A field declared `array` must hold a JSON array, whatever its `itemType`,
//! and where the `itemType` is `number`, `boolean`, `date`, `datetime`,
//! `enum` or `array` every element must be of that type. Before this check
//! only an array of objects was shape-checked: an `array` field with any
//! other `itemType`, or none, stored an object, a string or a number as given.
//!
//! An element of an array with no `itemType`, or with `itemType: "text"`, is
//! not type-checked, the same as a `text` field's value. Arrays of objects
//! are covered by `object_field_type_validation_test.rs`.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    ops::node_ops,
    schema::{handle_create_schema, handle_update_schema},
    services::{InsertPositionOwned, NodeService},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);
    Ok((node_service, temp_dir))
}

async fn seed_listing_schema(svc: &Arc<NodeService>) -> Result<()> {
    let array = |name: &str, item_type: Option<&str>| {
        let mut field = json!({
            "name": name, "type": "array", "protection": "user", "indexed": false
        });
        if let Some(item_type) = item_type {
            field["itemType"] = json!(item_type);
        }
        field
    };
    handle_create_schema(
        svc,
        json!({
            "name": "Listing",
            "fields": [
                array("amenities", None),
                array("tags", Some("text")),
                array("scores", Some("number")),
                array("flags", Some("boolean")),
                array("open_days", Some("date")),
                array("visits", Some("datetime")),
                array("grid", Some("array")),
                {
                    "name": "tiers", "type": "array", "itemType": "enum",
                    "protection": "user", "indexed": false,
                    "coreValues": [
                        { "value": "gold", "label": "Gold" },
                        { "value": "silver", "label": "Silver" }
                    ]
                },
                {
                    "name": "stats", "type": "object", "protection": "user", "indexed": false,
                    "fields": [array("samples", Some("number"))]
                },
                { "name": "note", "type": "text", "protection": "user", "indexed": false },
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("listing schema: {e}"))?;
    Ok(())
}

async fn create_listing(svc: &Arc<NodeService>, properties: Value) -> Result<String> {
    let output = node_ops::create_node(
        svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "listing".to_string(),
            content: "Harbour flat".to_string(),
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

async fn update_listing(svc: &Arc<NodeService>, id: &str, properties: Value) -> Result<()> {
    node_ops::update_node(
        svc,
        node_ops::UpdateNodeInput {
            node_id: id.to_string(),
            version: None,
            node_type: None,
            content: None,
            properties: Some(properties),
            add_to_collections: vec![],
            add_to_collection_ids: vec![],
            remove_from_collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await?;
    Ok(())
}

async fn listing_props(svc: &Arc<NodeService>, id: &str) -> Result<Value> {
    let node = node_ops::get_node(
        svc,
        node_ops::GetNodeInput {
            node_id: id.to_string(),
        },
    )
    .await?;
    Ok(node["properties"].clone())
}

/// Assert creation with `properties` fails with an error holding every one of
/// `expected` (the field, its declared type, and what was received).
async fn assert_create_rejected(svc: &Arc<NodeService>, properties: Value, expected: &[&str]) {
    let err = create_listing(svc, properties.clone())
        .await
        .expect_err(&format!("{properties} must be rejected"));
    let msg = err.to_string();
    for part in expected {
        assert!(
            msg.contains(part),
            "rejection of {properties} must contain {part:?}, got: {msg}"
        );
    }
}

#[tokio::test]
async fn array_field_rejects_every_non_array_shape() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    // With no item type, with a text item type and with a scalar one: the
    // value must be an array in every case.
    for field in ["amenities", "tags", "scores"] {
        for (value, received) in [
            (json!({ "a": 1 }), "object"),
            (json!("x"), "the string 'x'"),
            (json!(5), "number"),
            (json!(true), "boolean"),
        ] {
            assert_create_rejected(
                &svc,
                json!({ field: value }),
                &[field, "declared as type 'array'", received],
            )
            .await;
        }
    }
    Ok(())
}

#[tokio::test]
async fn array_field_accepts_an_array_an_empty_array_and_null() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    let id = create_listing(
        &svc,
        json!({ "amenities": ["pool", "gym"], "tags": [], "scores": null }),
    )
    .await?;

    let props = listing_props(&svc, &id).await?;
    assert_eq!(props["amenities"], json!(["pool", "gym"]));
    assert_eq!(props["tags"], json!([]));
    Ok(())
}

#[tokio::test]
async fn untyped_and_text_arrays_do_not_check_their_items() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    let mixed = json!(["pool", 3, true, { "k": "v" }]);
    let id = create_listing(
        &svc,
        json!({ "amenities": mixed.clone(), "tags": mixed.clone() }),
    )
    .await?;

    let props = listing_props(&svc, &id).await?;
    assert_eq!(props["amenities"], mixed);
    assert_eq!(props["tags"], mixed);
    Ok(())
}

#[tokio::test]
async fn number_array_rejects_a_non_number_item() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    for (item, received) in [
        (json!("7"), "the string '7'"),
        (json!(true), "boolean"),
        (json!(null), "null"),
        (json!([1]), "array"),
        (json!({ "n": 1 }), "object"),
    ] {
        assert_create_rejected(
            &svc,
            json!({ "scores": [1, 2.5, item] }),
            &["scores", "item type 'number'", "item 2", received],
        )
        .await;
    }
    Ok(())
}

#[tokio::test]
async fn boolean_array_rejects_a_non_boolean_item() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    for (item, received) in [(json!("true"), "the string 'true'"), (json!(1), "number")] {
        assert_create_rejected(
            &svc,
            json!({ "flags": [item, false] }),
            &["flags", "item type 'boolean'", "item 0", received],
        )
        .await;
    }
    Ok(())
}

#[tokio::test]
async fn date_array_rejects_a_non_iso_item() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    for (item, received) in [
        (json!("next spring"), "the string 'next spring'"),
        (json!(20261005), "number"),
    ] {
        assert_create_rejected(
            &svc,
            json!({ "open_days": ["2026-10-05", item] }),
            &[
                "open_days",
                "item type 'date'",
                "YYYY-MM-DD",
                "item 1",
                received,
            ],
        )
        .await;
    }
    Ok(())
}

#[tokio::test]
async fn datetime_array_rejects_an_item_that_is_not_a_date_time() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    // A bare date satisfies `date` but not `datetime`.
    for (item, received) in [
        (json!("2026-10-05"), "the string '2026-10-05'"),
        (json!(false), "boolean"),
    ] {
        assert_create_rejected(
            &svc,
            json!({ "visits": ["2026-10-05T09:00:00Z", item] }),
            &[
                "visits",
                "item type 'datetime'",
                "RFC 3339",
                "item 1",
                received,
            ],
        )
        .await;
    }
    Ok(())
}

#[tokio::test]
async fn typed_arrays_accept_items_of_their_type() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    let properties = json!({
        "scores": [1, 2.5, -3],
        "flags": [true, false],
        "open_days": ["2026-10-05", "2026-10-06T08:00:00Z"],
        "visits": ["2026-10-05T09:00:00Z"]
    });
    let id = create_listing(&svc, properties.clone()).await?;

    let props = listing_props(&svc, &id).await?;
    for (field, value) in properties.as_object().expect("an object") {
        assert_eq!(&props[field], value, "{field} must be stored as given");
    }
    Ok(())
}

#[tokio::test]
async fn array_fields_are_validated_on_update() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;
    let id = create_listing(&svc, json!({ "amenities": ["pool"], "scores": [1] })).await?;

    let err = update_listing(&svc, &id, json!({ "amenities": { "a": 1 } }))
        .await
        .expect_err("an object must not replace an array");
    assert!(
        err.to_string().contains("declared as type 'array'"),
        "got: {err}"
    );

    let err = update_listing(&svc, &id, json!({ "scores": [1, "two"] }))
        .await
        .expect_err("a string item must not enter a number array");
    assert!(err.to_string().contains("item 1"), "got: {err}");

    let props = listing_props(&svc, &id).await?;
    assert_eq!(props["amenities"], json!(["pool"]));
    assert_eq!(props["scores"], json!([1]));
    Ok(())
}

#[tokio::test]
async fn enum_array_rejects_an_item_outside_the_declared_values() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    for (item, received) in [
        (json!("bronze"), "the string 'bronze'"),
        (json!(""), "the string ''"),
        (json!(1), "number"),
    ] {
        assert_create_rejected(
            &svc,
            json!({ "tiers": ["gold", item] }),
            &[
                "tiers",
                "item type 'enum'",
                "item 1",
                received,
                "Gold (gold), Silver (silver)",
            ],
        )
        .await;
    }

    let id = create_listing(&svc, json!({ "tiers": ["gold", "silver"] })).await?;
    assert_eq!(
        listing_props(&svc, &id).await?["tiers"],
        json!(["gold", "silver"])
    );
    Ok(())
}

#[tokio::test]
async fn array_of_arrays_rejects_a_non_array_item() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    for (item, received) in [
        (json!("x"), "the string 'x'"),
        (json!({ "a": 1 }), "object"),
    ] {
        assert_create_rejected(
            &svc,
            json!({ "grid": [[1, 2], item] }),
            &["grid", "item type 'array'", "item 1", received],
        )
        .await;
    }

    let id = create_listing(&svc, json!({ "grid": [[1, 2], []] })).await?;
    assert_eq!(listing_props(&svc, &id).await?["grid"], json!([[1, 2], []]));
    Ok(())
}

#[tokio::test]
async fn an_array_nested_in_an_object_is_checked_and_named_by_its_path() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;

    assert_create_rejected(
        &svc,
        json!({ "stats": { "samples": "many" } }),
        &["in 'stats'", "samples", "declared as type 'array'"],
    )
    .await;
    assert_create_rejected(
        &svc,
        json!({ "stats": { "samples": [1, "two"] } }),
        &["in 'stats'", "samples", "item 1", "the string 'two'"],
    )
    .await;

    create_listing(&svc, json!({ "stats": { "samples": [1, 2] } })).await?;
    Ok(())
}

/// Re-declaring a field judges the values nodes already hold by the same
/// rule a write gets, so a schema change can't strand a node.
#[tokio::test]
async fn redeclaring_a_field_is_refused_while_nodes_hold_values_the_array_rule_rejects(
) -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_listing_schema(&svc).await?;
    create_listing(&svc, json!({ "note": "by the sea", "amenities": ["pool"] })).await?;

    let redeclare = |name: &'static str, item_type: Option<&'static str>| {
        let mut field = json!({
            "name": name, "type": "array", "protection": "user", "indexed": false
        });
        if let Some(item_type) = item_type {
            field["itemType"] = json!(item_type);
        }
        json!({ "schema_id": "listing", "remove_fields": [name], "add_fields": [field] })
    };

    let err = handle_update_schema(&svc, redeclare("note", None))
        .await
        .expect_err("a stored string must block re-declaring its field as an array");
    assert!(
        err.to_string().contains("declared as type 'array'"),
        "got: {err}"
    );

    let err = handle_update_schema(&svc, redeclare("amenities", Some("number")))
        .await
        .expect_err("a stored string item must block a number item type");
    assert!(err.to_string().contains("item 0"), "got: {err}");

    // A stored value the new declaration accepts doesn't block it.
    handle_update_schema(&svc, redeclare("amenities", Some("text")))
        .await
        .map_err(|e| anyhow::anyhow!("re-declaring amenities as a text array: {e}"))?;
    Ok(())
}
