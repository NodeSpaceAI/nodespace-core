//! `create_schema`'s result must describe what was PERSISTED, not what was
//! REQUESTED.
//!
//! The reported failure: a `create_schema` call returned `is_error=false` with
//! a fully-populated field list for a schema that was never persisted, and the
//! agent — correctly trusting its own tool result — told the user the type had
//! been created. The user sees no error and hits a confusing failure later.
//!
//! The structural cause was that `CreateSchemaOutput` was assembled entirely
//! from the request: `fields` was the caller's own array echoed back, and the
//! id the write transaction actually produced was discarded. A result built
//! that way is identical whether or not the write landed, so it cannot report
//! the difference — and it defeats the caller-side no-op guard in
//! `agent_loop.rs`, which counts `fields` in the result on the premise that a
//! result reports persisted state rather than requested state.
//!
//! These tests pin the premise that guard depends on. `handle_create_schema`
//! now reads the committed schema node back and builds its output from that,
//! so a result that claims fields is evidence those fields are in the
//! database.

use nodespace_core::db::SqliteStore;
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::{NodeService, WriteVerificationFault};
use std::sync::Arc;

async fn test_service() -> (Arc<NodeService>, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let mut store: Arc<SqliteStore> = Arc::new(
        SqliteStore::new(tmp.path().join("nodespace.db"))
            .await
            .expect("open store"),
    );
    let svc = Arc::new(NodeService::new(&mut store).await.expect("node service"));
    (svc, tmp)
}

/// **The test that pins the fix.** A write whose result cannot be confirmed
/// against the database must be an error, never a success carrying a
/// fabricated field list.
///
/// This is the reported bug reproduced exactly: `is_error=false`,
/// `result_field_count=2`, and no such schema afterwards. It is also the only
/// test here that fails when the read-back is reverted — on every path where
/// the write succeeds, an echoed payload and a read-back payload are
/// byte-identical, so nothing else can tell them apart.
///
/// Reaching the branch requires the fault seam: the store is sealed, so there
/// is no way to make a committed row genuinely vanish. See
/// [`NodeService::set_write_verification_fault`].
#[tokio::test]
async fn a_write_that_cannot_be_verified_is_an_error_not_a_populated_success() {
    let (svc, _tmp) = test_service().await;

    svc.set_write_verification_fault(Some(WriteVerificationFault::ReportMissing));

    let result = handle_create_schema(
        &svc,
        serde_json::json!({
            "name": "Event Venue",
            "fields": [
                {"name": "booking_date", "type": "date"},
                {"name": "capacity", "type": "number"}
            ]
        }),
    )
    .await;

    let err = match result {
        Err(e) => e,
        Ok(payload) => panic!(
            "create_schema reported SUCCESS for a schema it could not verify. \
             This is the reported bug: the agent trusts this payload and tells \
             the user the type exists. Payload was: {payload}"
        ),
    };

    let msg = err.to_string();
    assert!(
        msg.contains("event_venue"),
        "the error must name the schema that was not created, got: {msg}"
    );
    assert!(
        msg.contains("NOT created") || msg.contains("not present"),
        "the error must state plainly that nothing was created, so the agent \
         does not report success, got: {msg}"
    );
}

/// A row that committed but reads back unparseable must NOT be reported as
/// "not created".
///
/// The store folds two different facts into one `Ok(None)`: the row is absent,
/// or it is present and `SchemaNode::from_node` failed on it. Claiming the
/// first without checking would be this PR's own bug in miniature — asserting
/// more than the read establishes — and it misleads in the costly direction.
/// An agent told a schema it *did* commit was never created retries, the
/// exists-check reads `Ok(None)` as well, and the retry runs straight into the
/// primary-key collision the exists-check is there to prevent.
#[tokio::test]
async fn a_committed_but_unreadable_schema_is_not_reported_as_never_created() {
    let (svc, _tmp) = test_service().await;

    svc.set_write_verification_fault(Some(WriteVerificationFault::ReportUnparseable));

    let err = handle_create_schema(
        &svc,
        serde_json::json!({
            "name": "Event Venue",
            "fields": [{"name": "capacity", "type": "number"}]
        }),
    )
    .await
    .expect_err("an unverifiable write must not report success");

    let msg = err.to_string();
    assert!(
        msg.contains("event_venue"),
        "the error must name the schema, got: {msg}"
    );
    assert!(
        !msg.contains("NOT created"),
        "the row DID commit — claiming it was not created sends the agent into \
         a retry that collides with it, got: {msg}"
    );
    assert!(
        msg.contains("do not retry") || msg.contains("collide"),
        "the error must steer the agent away from retrying, got: {msg}"
    );
}

/// Every field the result claims must be readable back out of the database
/// under the id the result reports.
///
/// This is the property the agent's no-op guard rests on: a non-zero `fields`
/// length in the result has to mean those fields were persisted. It passes
/// with or without the read-back (the happy path persists correctly either
/// way) — the test above is the one that pins the fix. This one guards the
/// premise itself: if the write path ever starts storing something other than
/// what it reports, this fails.
#[tokio::test]
async fn result_fields_are_all_present_in_the_persisted_schema() {
    let (svc, _tmp) = test_service().await;

    let result = handle_create_schema(
        &svc,
        serde_json::json!({
            "name": "Event Venue",
            "fields": [
                {"name": "booking_date", "type": "date"},
                {"name": "capacity", "type": "number"}
            ]
        }),
    )
    .await
    .expect("create_schema should succeed");

    let reported_id = result
        .get("schemaId")
        .and_then(|v| v.as_str())
        .expect("result carries schemaId");

    let persisted = svc
        .get_schema_node(reported_id)
        .await
        .expect("read back schema")
        .unwrap_or_else(|| {
            panic!("create_schema reported success for '{reported_id}', but no such schema exists")
        });

    let reported_fields: Vec<&str> = result
        .get("fields")
        .and_then(|v| v.as_array())
        .expect("result carries a fields array")
        .iter()
        .map(|f| f.get("name").and_then(|n| n.as_str()).expect("field name"))
        .collect();

    let persisted_fields: Vec<&str> = persisted.fields.iter().map(|f| f.name.as_str()).collect();

    for name in &reported_fields {
        assert!(
            persisted_fields.contains(name),
            "result claims field {name:?} but the persisted schema has only \
             {persisted_fields:?} — the result describes the request, not the database"
        );
    }
    assert_eq!(
        reported_fields, persisted_fields,
        "result's field list must match the persisted schema's exactly"
    );
}

/// The result must not be assembled from the caller's arguments.
///
/// `handle_create_schema` normalizes what it stores — notably
/// `apply_friendly_name_defaults`, which derives a `friendlyName` the caller
/// never sent ("booking_date" → "Booking date"). Both forms happen to agree
/// here, because normalization runs before the write — so this does not by
/// itself distinguish an echo from a read-back. It pins the weaker but still
/// useful property that the reported label is the stored one.
#[tokio::test]
async fn result_field_list_is_not_an_echo_of_the_request() {
    let (svc, _tmp) = test_service().await;

    let result = handle_create_schema(
        &svc,
        serde_json::json!({
            "name": "Event Venue",
            // No friendlyName supplied — the store derives one.
            "fields": [{"name": "booking_date", "type": "date"}]
        }),
    )
    .await
    .expect("create_schema should succeed");

    let field = result
        .get("fields")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .expect("result carries at least one field");

    let friendly = field
        .get("friendlyName")
        .and_then(|v| v.as_str())
        .expect("result field carries friendlyName");

    let persisted = svc
        .get_schema_node("event_venue")
        .await
        .expect("read back schema")
        .expect("schema exists");
    let persisted_friendly = persisted.fields[0].friendly_name.as_str();

    assert_eq!(
        friendly, persisted_friendly,
        "the result's friendlyName must be the stored one, not a value \
         reconstructed from the request"
    );
}

/// A schema that already exists is rejected, and the rejection must not be
/// reachable by a read failure being mistaken for "does not exist".
///
/// The exists-check used to be `if let Ok(Some(..))`, which routed both `Err`
/// and `Ok(None)` into the write path. A duplicate then hit the primary-key
/// constraint and surfaced as an opaque internal error naming neither the real
/// problem nor the repair.
#[tokio::test]
async fn duplicate_schema_is_rejected_with_an_actionable_error() {
    let (svc, _tmp) = test_service().await;

    let args = serde_json::json!({
        "name": "Event Venue",
        "fields": [{"name": "capacity", "type": "number"}]
    });

    handle_create_schema(&svc, args.clone())
        .await
        .expect("first create succeeds");

    let err = handle_create_schema(&svc, args)
        .await
        .expect_err("second create must be rejected");
    let msg = err.to_string();

    assert!(
        msg.contains("already exists"),
        "duplicate rejection must say the schema already exists, got: {msg}"
    );
    assert!(
        msg.contains("create_node"),
        "duplicate rejection must point at create_node as the repair, got: {msg}"
    );
    assert!(
        !msg.contains("UNIQUE constraint") && !msg.contains("PRIMARY KEY"),
        "duplicate must be caught by the exists-check, not left to surface as a \
         raw constraint violation, got: {msg}"
    );
}

/// The two-call sequence from the original report, end to end: two
/// structurally identical `create_schema` calls in the same session must both
/// persist, and each result must describe its own persisted schema.
#[tokio::test]
async fn two_sequential_creates_both_persist_and_both_results_are_accurate() {
    let (svc, _tmp) = test_service().await;

    let first = handle_create_schema(
        &svc,
        serde_json::json!({
            "name": "Company Sold To",
            "fields": [{"name": "signed_date", "type": "date"}]
        }),
    )
    .await
    .expect("first create succeeds");

    let second = handle_create_schema(
        &svc,
        serde_json::json!({
            "name": "Event Venue",
            "fields": [
                {"name": "booking_date", "type": "date"},
                {"name": "capacity", "type": "number"}
            ]
        }),
    )
    .await
    .expect("second create succeeds");

    for (result, expected_id) in [(&first, "company_sold_to"), (&second, "event_venue")] {
        let reported = result
            .get("schemaId")
            .and_then(|v| v.as_str())
            .expect("result carries schemaId");
        assert_eq!(reported, expected_id);
        assert!(
            svc.get_schema_node(expected_id)
                .await
                .expect("read back schema")
                .is_some(),
            "'{expected_id}' was reported created but is absent from the database"
        );
    }
}
