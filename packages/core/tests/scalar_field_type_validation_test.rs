//! Fields declared `number`, `boolean`, `date` or `datetime` must hold a value
//! of that type. Before this check they accepted any JSON value, so a
//! `number` field could store `"large"` and a `date` field `"next spring"`,
//! and every downstream consumer (sorting, `gt`/`lt` query filters, the CEL
//! date functions) trusted a type it couldn't rely on.
//!
//! Null stays allowed for a non-required field: it is how a field is cleared.
//!
//! A survey of every production writer of a core-schema scalar field found
//! none storing the wrong JSON type, so the check needed no writer fixes.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    ops::node_ops,
    schema::handle_create_schema,
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

async fn seed_ticket_schema(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "Ticket",
            "fields": [
                { "name": "points", "type": "number", "protection": "user", "indexed": false },
                { "name": "blocked", "type": "boolean", "protection": "user", "indexed": false },
                { "name": "target", "type": "date", "protection": "user", "indexed": false },
                { "name": "reviewed_at", "type": "datetime", "protection": "user", "indexed": false }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("ticket schema: {e}"))?;
    Ok(())
}

async fn create_ticket(svc: &Arc<NodeService>, properties: Value) -> Result<String> {
    let output = node_ops::create_node(
        svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "ticket".to_string(),
            content: "Fix login".to_string(),
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

async fn update_ticket(svc: &Arc<NodeService>, id: &str, properties: Value) -> Result<()> {
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

async fn ticket_props(svc: &Arc<NodeService>, id: &str) -> Result<Value> {
    let node = node_ops::get_node(
        svc,
        node_ops::GetNodeInput {
            node_id: id.to_string(),
        },
    )
    .await?;
    Ok(node["properties"].clone())
}

/// Assert creation with `properties` fails with an error naming `field`, its
/// declared type, and what was received.
async fn assert_create_rejected(
    svc: &Arc<NodeService>,
    properties: Value,
    field: &str,
    declared: &str,
    received: &str,
) {
    let err = create_ticket(svc, properties.clone())
        .await
        .expect_err(&format!("{properties} must be rejected"));
    let msg = err.to_string();
    assert!(
        msg.contains(&format!("Field '{field}'")),
        "error must name the field, got: {msg}"
    );
    assert!(
        msg.contains(&format!("type '{declared}'")),
        "error must name the declared type, got: {msg}"
    );
    assert!(
        msg.contains(received),
        "error must name what was received ({received}), got: {msg}"
    );
}

#[tokio::test]
async fn number_field_rejects_non_numbers() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_ticket_schema(&svc).await?;

    assert_create_rejected(
        &svc,
        json!({ "points": "large" }),
        "points",
        "number",
        "the string 'large'",
    )
    .await;
    // A numeric string is still a string.
    assert_create_rejected(
        &svc,
        json!({ "points": "5" }),
        "points",
        "number",
        "the string '5'",
    )
    .await;
    assert_create_rejected(
        &svc,
        json!({ "points": true }),
        "points",
        "number",
        "a boolean",
    )
    .await;
    Ok(())
}

#[tokio::test]
async fn number_field_accepts_integers_and_floats() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_ticket_schema(&svc).await?;

    let id = create_ticket(&svc, json!({ "points": 5 })).await?;
    assert_eq!(ticket_props(&svc, &id).await?["points"], json!(5));
    let id = create_ticket(&svc, json!({ "points": 0.5 })).await?;
    assert_eq!(ticket_props(&svc, &id).await?["points"], json!(0.5));
    Ok(())
}

#[tokio::test]
async fn boolean_field_rejects_non_booleans() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_ticket_schema(&svc).await?;

    assert_create_rejected(
        &svc,
        json!({ "blocked": "true" }),
        "blocked",
        "boolean",
        "the string 'true'",
    )
    .await;
    assert_create_rejected(
        &svc,
        json!({ "blocked": 1 }),
        "blocked",
        "boolean",
        "a number",
    )
    .await;
    Ok(())
}

#[tokio::test]
async fn boolean_field_accepts_a_boolean() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_ticket_schema(&svc).await?;

    let id = create_ticket(&svc, json!({ "blocked": false })).await?;
    assert_eq!(ticket_props(&svc, &id).await?["blocked"], json!(false));
    Ok(())
}

#[tokio::test]
async fn date_field_rejects_non_iso_values() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_ticket_schema(&svc).await?;

    assert_create_rejected(
        &svc,
        json!({ "target": "next spring" }),
        "target",
        "date",
        "the string 'next spring'",
    )
    .await;
    // A locale-formatted date is not ISO-8601.
    assert_create_rejected(
        &svc,
        json!({ "target": "08/06/2026" }),
        "target",
        "date",
        "the string '08/06/2026'",
    )
    .await;
    // A calendar-invalid date is not a date.
    assert_create_rejected(
        &svc,
        json!({ "target": "2026-02-30" }),
        "target",
        "date",
        "the string '2026-02-30'",
    )
    .await;
    // An epoch timestamp is a number, not a date string.
    assert_create_rejected(
        &svc,
        json!({ "target": 1_780_000_000_000_i64 }),
        "target",
        "date",
        "a number",
    )
    .await;
    Ok(())
}

#[tokio::test]
async fn date_field_accepts_a_date_and_a_date_time() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_ticket_schema(&svc).await?;

    let id = create_ticket(&svc, json!({ "target": "2026-03-01" })).await?;
    assert_eq!(
        ticket_props(&svc, &id).await?["target"],
        json!("2026-03-01")
    );
    let id = create_ticket(&svc, json!({ "target": "2026-03-01T09:30:00Z" })).await?;
    assert_eq!(
        ticket_props(&svc, &id).await?["target"],
        json!("2026-03-01T09:30:00Z")
    );
    let id = create_ticket(&svc, json!({ "target": "2026-03-01T09:30:00+02:00" })).await?;
    assert_eq!(
        ticket_props(&svc, &id).await?["target"],
        json!("2026-03-01T09:30:00+02:00")
    );
    Ok(())
}

#[tokio::test]
async fn datetime_field_requires_an_rfc3339_date_time() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_ticket_schema(&svc).await?;

    // A bare date carries no time of day.
    assert_create_rejected(
        &svc,
        json!({ "reviewed_at": "2026-03-01" }),
        "reviewed_at",
        "datetime",
        "the string '2026-03-01'",
    )
    .await;
    // chrono's Display form is not RFC 3339.
    assert_create_rejected(
        &svc,
        json!({ "reviewed_at": "2026-03-01 09:30:00 UTC" }),
        "reviewed_at",
        "datetime",
        "the string '2026-03-01 09:30:00 UTC'",
    )
    .await;

    let id = create_ticket(&svc, json!({ "reviewed_at": "2026-03-01T09:30:00Z" })).await?;
    assert_eq!(
        ticket_props(&svc, &id).await?["reviewed_at"],
        json!("2026-03-01T09:30:00Z")
    );
    Ok(())
}

#[tokio::test]
async fn scalar_fields_accept_an_explicit_null() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_ticket_schema(&svc).await?;

    create_ticket(
        &svc,
        json!({ "points": null, "blocked": null, "target": null, "reviewed_at": null }),
    )
    .await?;
    Ok(())
}

/// The update path reaches the same check as create, and clearing a field
/// with null still works there.
#[tokio::test]
async fn scalar_fields_are_validated_on_update() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;
    seed_ticket_schema(&svc).await?;
    let id = create_ticket(
        &svc,
        json!({ "points": 3, "blocked": false, "target": "2026-03-01" }),
    )
    .await?;

    for (bad, field) in [
        (json!({ "points": "large" }), "points"),
        (json!({ "blocked": "yes" }), "blocked"),
        (json!({ "target": "next spring" }), "target"),
    ] {
        let err = update_ticket(&svc, &id, bad.clone())
            .await
            .expect_err(&format!("update {bad} must be rejected"));
        assert!(
            err.to_string().contains(&format!("Field '{field}'")),
            "error must name the field, got: {err}"
        );
    }

    update_ticket(
        &svc,
        &id,
        json!({ "points": 8, "blocked": true, "target": "2026-04-01T12:00:00Z" }),
    )
    .await?;
    let props = ticket_props(&svc, &id).await?;
    assert_eq!(props["points"], json!(8));
    assert_eq!(props["blocked"], json!(true));
    assert_eq!(props["target"], json!("2026-04-01T12:00:00Z"));

    update_ticket(&svc, &id, json!({ "points": null })).await?;
    Ok(())
}

/// Core schemas are enforced too, not only user-defined ones:
/// `collection.restrictedToMembers` is declared `boolean`.
#[tokio::test]
async fn core_schema_scalar_fields_are_enforced() -> Result<()> {
    let (svc, _tmp) = create_test_service().await?;

    let err = node_ops::create_node(
        &svc,
        node_ops::CreateNodeInput {
            id: None,
            node_type: "collection".to_string(),
            content: "Private".to_string(),
            parent_id: None,
            position: InsertPositionOwned::End,
            properties: json!({ "restrictedToMembers": "true" }),
            collections: vec![],
            collection_ids: vec![],
            lifecycle_status: None,
        },
    )
    .await
    .expect_err("a string must not satisfy collection.restrictedToMembers (boolean)");
    assert!(
        err.to_string().contains("Field 'restrictedToMembers'"),
        "error must name the field, got: {err}"
    );
    Ok(())
}
