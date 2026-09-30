//! Integration tests for ADR-060 synchronous invariant-rule dispatch through
//! the bulk write paths: `bulk_create`, `bulk_create_hierarchy`,
//! `bulk_create_hierarchy_trusted`, `bulk_update`, and markdown import.
//!
//! The reactive engine never runs an invariant rule — it assumes synchronous
//! dispatch already did — so a bulk path that skipped dispatch would leave
//! the rule unevaluated for good. Each bulk path runs its whole batch in one
//! transaction, so one row's rejection must fail the batch with nothing of it
//! persisted, and surface the same `PlayRuleRejected` single-node writes do.

use anyhow::Result;
use nodespace_core::db::events::{DomainEvent, REPLICATED_APPLY_CLIENT_ID};
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeFilter, NodeUpdate};
use nodespace_core::services::{NodeService, NodeServiceError};
use nodespace_core::PlaybookEngine;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;
use tokio::time::timeout;

type HierarchyRow = (
    String,
    String,
    String,
    Option<String>,
    f64,
    serde_json::Value,
);

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);
    Ok((service, temp_dir))
}

async fn create_schema(
    service: &NodeService,
    node_type: &str,
    fields: serde_json::Value,
) -> Result<()> {
    let schema = Node::new_with_id(
        node_type.to_string(),
        "schema".to_string(),
        node_type.to_string(),
        json!({
            "isCore": false,
            "schemaVersion": 1,
            "description": format!("{node_type} schema"),
            "fields": fields,
            "relationships": []
        }),
    );
    service.create_node(schema).await?;
    Ok(())
}

/// Wire a `PlaybookEngine`'s lifecycle into `service` and activate `rules`
/// directly, so dispatch sees them without a running engine loop.
fn activate_rules(service: &Arc<NodeService>, rules: serde_json::Value) -> PlaybookEngine {
    let engine = PlaybookEngine::new(Arc::clone(service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play = Node::new(
        "play".to_string(),
        "bulk-invariant-play".to_string(),
        json!({ "rules": rules }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play)
            .expect("play must parse and activate");
    }
    engine
}

fn reject_on_create(node_type: &str, condition: &str, message: &str) -> serde_json::Value {
    json!([{
        "name": "reject-on-create",
        "class": "invariant",
        "trigger": { "type": "graph_event", "on": "node_created", "node_type": node_type },
        "conditions": [condition],
        "actions": [{ "action_type": "reject", "params": { "message": message } }]
    }])
}

fn stamp_approved_on_create(node_type: &str) -> serde_json::Value {
    json!([{
        "name": "stamp-approved",
        "class": "invariant",
        "trigger": { "type": "graph_event", "on": "node_created", "node_type": node_type },
        "conditions": ["node.status == 'pending'"],
        "actions": [{
            "action_type": "update_node",
            "params": { "node_id": "{trigger.node.id}", "properties": { "approved": true } }
        }]
    }])
}

fn row(id: &str, node_type: &str, parent: Option<&str>, props: serde_json::Value) -> HierarchyRow {
    (
        id.to_string(),
        node_type.to_string(),
        format!("{id} content"),
        parent.map(str::to_string),
        1.0,
        props,
    )
}

fn user_field<'a>(node: &'a Node, node_type: &str, field: &str) -> Option<&'a serde_json::Value> {
    node.properties.get(node_type).and_then(|p| p.get(field))
}

fn assert_rejected(err: &NodeServiceError, expected_node: &str) {
    match err {
        NodeServiceError::PlayRuleRejected { node_id, .. } => assert_eq!(
            node_id, expected_node,
            "the rejection must name the violating row"
        ),
        other => panic!("expected PlayRuleRejected, got {other:?}"),
    }
}

async fn assert_absent(service: &NodeService, ids: &[&str]) -> Result<()> {
    for id in ids {
        assert!(
            service.get_node(id).await?.is_none(),
            "{id} must not exist: a rejected row rolls back its whole batch"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// bulk_create_hierarchy
// ---------------------------------------------------------------------------

/// One violating row among valid ones fails the whole hierarchy insert: no
/// row of the batch — before or after the violating one — is persisted.
#[tokio::test]
async fn bulk_create_hierarchy_rejects_the_whole_batch_when_one_row_violates() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "bi_hier",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    let _engine = activate_rules(
        &service,
        reject_on_create("bi_hier", "node.status == 'blocked'", "no blocked rows"),
    );

    let err = service
        .bulk_create_hierarchy(vec![
            row("bi-hier-root", "bi_hier", None, json!({ "status": "open" })),
            row(
                "bi-hier-bad",
                "bi_hier",
                Some("bi-hier-root"),
                json!({ "status": "blocked" }),
            ),
            row(
                "bi-hier-after",
                "bi_hier",
                Some("bi-hier-root"),
                json!({ "status": "open" }),
            ),
        ])
        .await
        .unwrap_err();

    assert_rejected(&err, "bi-hier-bad");
    assert_absent(&service, &["bi-hier-root", "bi-hier-bad", "bi-hier-after"]).await
}

/// The rule's condition is a real gate: a batch that satisfies it commits,
/// and a non-reject invariant action runs for every matching row, inside the
/// insert's transaction — already visible when the call returns.
#[tokio::test]
async fn bulk_create_hierarchy_satisfying_rules_commits_and_runs_their_actions() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "bi_stamp",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;
    let _engine = activate_rules(&service, stamp_approved_on_create("bi_stamp"));

    service
        .bulk_create_hierarchy(vec![
            row(
                "bi-stamp-root",
                "bi_stamp",
                None,
                json!({ "status": "pending" }),
            ),
            row(
                "bi-stamp-child",
                "bi_stamp",
                Some("bi-stamp-root"),
                json!({ "status": "pending" }),
            ),
            row(
                "bi-stamp-other",
                "bi_stamp",
                Some("bi-stamp-root"),
                json!({ "status": "open" }),
            ),
        ])
        .await?;

    for id in ["bi-stamp-root", "bi-stamp-child"] {
        let node = service.get_node(id).await?.expect("row must be created");
        assert_eq!(
            user_field(&node, "bi_stamp", "approved"),
            Some(&json!(true)),
            "{id}: the invariant action must have run before bulk_create_hierarchy returned"
        );
    }
    let other = service
        .get_node("bi-stamp-other")
        .await?
        .expect("row must be created");
    assert_eq!(
        user_field(&other, "bi_stamp", "approved"),
        None,
        "a row failing the condition is created untouched"
    );
    Ok(())
}

/// A rule registered on a base type fires for a subtype row created in bulk
/// (ADR-078), reading the subtype's inherited field.
#[tokio::test]
async fn bulk_create_hierarchy_applies_a_base_type_rule_to_a_subtype_row() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({
            "name": "bi_ticket",
            "fields": [{ "name": "state", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({ "name": "bi_bug", "extends": "bi_ticket", "fields": [] }),
    )
    .await?;
    let play = Node::new(
        "play".to_string(),
        "bi-base-play".to_string(),
        json!({ "rules": reject_on_create("bi_ticket", "node.state == 'done'", "no done tickets") }),
    );
    service.create_node(play).await?;
    // The engine's start-up load activates the play and builds the ancestry
    // a base-type trigger needs to match a subtype.
    let (shutdown_tx, task) = spawn_engine(&service).await;

    let err = service
        .bulk_create_hierarchy(vec![row(
            "bi-bug-done",
            "bi_bug",
            None,
            json!({ "state": "done" }),
        )])
        .await
        .unwrap_err();
    assert_rejected(&err, "bi-bug-done");
    assert_absent(&service, &["bi-bug-done"]).await?;

    service
        .bulk_create_hierarchy(vec![row(
            "bi-bug-open",
            "bi_bug",
            None,
            json!({ "state": "open" }),
        )])
        .await?;
    assert!(service.get_node("bi-bug-open").await?.is_some());

    let _ = shutdown_tx.send(true);
    let _ = timeout(Duration::from_secs(2), task).await;
    Ok(())
}

async fn spawn_engine(
    service: &Arc<NodeService>,
) -> (watch::Sender<bool>, tokio::task::JoinHandle<Result<()>>) {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let engine = Arc::new(PlaybookEngine::new(Arc::clone(service)));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let task = tokio::spawn(async move { engine.start(shutdown_rx).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (shutdown_tx, task)
}

// ---------------------------------------------------------------------------
// bulk_create
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bulk_create_rejects_the_whole_batch_when_one_node_violates() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "bi_flat",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    let _engine = activate_rules(
        &service,
        reject_on_create("bi_flat", "node.status == 'blocked'", "no blocked nodes"),
    );

    let ok = Node::new(
        "bi_flat".to_string(),
        "fine".to_string(),
        json!({ "bi_flat": { "status": "open" } }),
    );
    let bad = Node::new(
        "bi_flat".to_string(),
        "blocked".to_string(),
        json!({ "bi_flat": { "status": "blocked" } }),
    );
    let (ok_id, bad_id) = (ok.id.clone(), bad.id.clone());

    let err = service.bulk_create(vec![ok, bad]).await.unwrap_err();
    assert_rejected(&err, &bad_id);
    assert_absent(&service, &[&ok_id, &bad_id]).await?;

    let fine = Node::new(
        "bi_flat".to_string(),
        "fine again".to_string(),
        json!({ "bi_flat": { "status": "open" } }),
    );
    let fine_id = fine.id.clone();
    service.bulk_create(vec![fine]).await?;
    assert!(service.get_node(&fine_id).await?.is_some());
    Ok(())
}

/// A receiving device never re-runs an invariant rule for a node that
/// arrived via sync (ADR-060 §1) — the same no-op single-node `create_node`
/// has, and the sync catch-up path applies pulled pages through `bulk_create`.
#[tokio::test]
async fn sync_tagged_bulk_create_does_not_run_invariant_rules() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "bi_synced",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    let _engine = activate_rules(
        &service,
        reject_on_create("bi_synced", "node.status == 'blocked'", "no blocked nodes"),
    );

    let synced = Node::new(
        "bi_synced".to_string(),
        "from another device".to_string(),
        json!({ "bi_synced": { "status": "blocked" } }),
    );
    let synced_id = synced.id.clone();
    service
        .with_client(REPLICATED_APPLY_CLIENT_ID)
        .bulk_create(vec![synced])
        .await?;
    assert!(service.get_node(&synced_id).await?.is_some());
    Ok(())
}

/// The sync replay runs `bulk_create`/`bulk_update` inside its own
/// `begin_batch_emit` guard. Their transactions must hand their committed
/// events to that batch — delivered when the guard drops, not before, and
/// not lost. A create stays a create, however many updates follow it.
#[tokio::test]
async fn bulk_writes_inside_a_batch_emit_guard_deliver_their_events_on_drop() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let mut rx = service.subscribe_to_events();

    let node = Node::new("text".to_string(), "batched".to_string(), json!({}));
    let id = node.id.clone();
    {
        let _batch = service.begin_batch_emit();
        service.bulk_create(vec![node]).await?;
        service
            .bulk_update(vec![(
                id.clone(),
                NodeUpdate {
                    content: Some("batched, edited".to_string()),
                    ..Default::default()
                },
            )])
            .await?;
        assert!(
            rx.try_recv().is_err(),
            "events must stay buffered in the batch until its guard drops"
        );
    }

    let mut events = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        events.push(envelope.event);
    }
    assert_eq!(
        events.len(),
        1,
        "the batch keeps one event per node — got {events:?}"
    );
    assert!(
        matches!(&events[0], DomainEvent::NodeCreated { node_id, .. } if node_id == &id),
        "expected the node's NodeCreated, which the update it was followed by must not replace, got {events:?}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// bulk_create_hierarchy_trusted
// ---------------------------------------------------------------------------

/// "Trusted" skips schema validation of the parser's output shape, not the
/// user's product rules: an invariant reject still fails the whole import.
#[tokio::test]
async fn bulk_create_hierarchy_trusted_still_enforces_invariant_rules() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "bi_trusted",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    let _engine = activate_rules(
        &service,
        reject_on_create("bi_trusted", "node.status == 'blocked'", "no blocked rows"),
    );

    let err = service
        .bulk_create_hierarchy_trusted(vec![
            row(
                "bi-trusted-root",
                "bi_trusted",
                None,
                json!({ "status": "open" }),
            ),
            row(
                "bi-trusted-bad",
                "bi_trusted",
                Some("bi-trusted-root"),
                json!({ "status": "blocked" }),
            ),
        ])
        .await
        .unwrap_err();
    assert_rejected(&err, "bi-trusted-bad");
    assert_absent(&service, &["bi-trusted-root", "bi-trusted-bad"]).await?;

    service
        .bulk_create_hierarchy_trusted(vec![row(
            "bi-trusted-ok",
            "bi_trusted",
            None,
            json!({ "status": "open" }),
        )])
        .await?;
    assert!(service.get_node("bi-trusted-ok").await?.is_some());
    Ok(())
}

// ---------------------------------------------------------------------------
// bulk_update
// ---------------------------------------------------------------------------

fn reject_on_status_change(node_type: &str) -> serde_json::Value {
    json!([{
        "name": "reject-blocked-transition",
        "class": "invariant",
        "trigger": {
            "type": "graph_event",
            "on": "property_changed",
            "node_type": node_type,
            "property_key": format!("{node_type}.status")
        },
        "conditions": ["node.status == 'blocked'"],
        "actions": [{ "action_type": "reject", "params": { "message": "cannot block" } }]
    }])
}

fn status_update(status: &str) -> NodeUpdate {
    NodeUpdate {
        properties: Some(json!({ "status": status })),
        ..Default::default()
    }
}

/// A `property_changed` reject on one node fails the whole `bulk_update`:
/// neither node's property or version moves.
#[tokio::test]
async fn bulk_update_rejects_the_whole_batch_when_one_update_violates() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "bi_upd",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    let _engine = activate_rules(&service, reject_on_status_change("bi_upd"));

    let a = Node::new(
        "bi_upd".to_string(),
        "a".to_string(),
        json!({ "status": "open" }),
    );
    let b = Node::new(
        "bi_upd".to_string(),
        "b".to_string(),
        json!({ "status": "open" }),
    );
    let (a_id, b_id) = (a.id.clone(), b.id.clone());
    service.create_node(a).await?;
    service.create_node(b).await?;
    let a_before = service.get_node(&a_id).await?.unwrap();
    let b_before = service.get_node(&b_id).await?.unwrap();

    let err = service
        .bulk_update(vec![
            (a_id.clone(), status_update("in_progress")),
            (b_id.clone(), status_update("blocked")),
        ])
        .await
        .unwrap_err();
    assert_rejected(&err, &b_id);

    for before in [a_before, b_before] {
        let after = service.get_node(&before.id).await?.unwrap();
        assert_eq!(
            after.version, before.version,
            "a rejected batch bumps no version"
        );
        assert_eq!(
            user_field(&after, "bi_upd", "status"),
            Some(&json!("open")),
            "a rejected batch leaves every node's property unchanged"
        );
    }

    service
        .bulk_update(vec![(a_id.clone(), status_update("in_progress"))])
        .await?;
    let a_after = service.get_node(&a_id).await?.unwrap();
    assert_eq!(
        user_field(&a_after, "bi_upd", "status"),
        Some(&json!("in_progress"))
    );
    Ok(())
}

/// A non-reject update invariant runs inside the batch's transaction for
/// the node whose watched property changed — and only that node.
#[tokio::test]
async fn bulk_update_runs_update_invariant_actions_for_changed_properties_only() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "bi_verify",
        json!([
            { "name": "status", "type": "string" },
            { "name": "note", "type": "string" },
            { "name": "verified", "type": "boolean" }
        ]),
    )
    .await?;
    let _engine = activate_rules(
        &service,
        json!([{
            "name": "stamp-verified",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "node_type": "bi_verify",
                "property_key": "bi_verify.status"
            },
            "conditions": [],
            "actions": [{
                "action_type": "update_node",
                "params": { "node_id": "{trigger.node.id}", "properties": { "verified": true } }
            }]
        }]),
    );

    let a = Node::new(
        "bi_verify".to_string(),
        "a".to_string(),
        json!({ "status": "open" }),
    );
    let b = Node::new(
        "bi_verify".to_string(),
        "b".to_string(),
        json!({ "status": "open" }),
    );
    let (a_id, b_id) = (a.id.clone(), b.id.clone());
    service.create_node(a).await?;
    service.create_node(b).await?;

    service
        .bulk_update(vec![
            (a_id.clone(), status_update("done")),
            (
                b_id.clone(),
                NodeUpdate {
                    properties: Some(json!({ "note": "unrelated" })),
                    ..Default::default()
                },
            ),
        ])
        .await?;

    let a_after = service.get_node(&a_id).await?.unwrap();
    let b_after = service.get_node(&b_id).await?.unwrap();
    assert_eq!(
        user_field(&a_after, "bi_verify", "verified"),
        Some(&json!(true))
    );
    assert_eq!(
        user_field(&b_after, "bi_verify", "verified"),
        None,
        "a change to an unwatched property must not match the rule"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Markdown import
// ---------------------------------------------------------------------------

/// Markdown import's child batch goes through `bulk_create_hierarchy`, so an
/// imported node violating an invariant fails the import and none of the
/// batch's nodes — the violating checkbox or its siblings — is persisted.
#[tokio::test]
async fn markdown_import_of_a_violating_node_is_rejected() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let _engine = activate_rules(
        &service,
        reject_on_create(
            "checkbox",
            "node.content.contains('forbidden')",
            "no forbidden items",
        ),
    );

    let result = nodespace_core::markdown::handle_create_nodes_from_markdown(
        &service,
        json!({
            "title": "bi markdown import",
            "markdown_content": "bi-md sibling paragraph\n\n- [ ] bi-md forbidden item",
            "sync_import": true
        }),
    )
    .await;
    assert!(
        result.is_err(),
        "an import containing a rejected node must fail, got {result:?}"
    );

    for node_type in ["checkbox", "text"] {
        let leftovers = service
            .query_nodes(NodeFilter {
                node_type: Some(node_type.to_string()),
                content_contains: Some("bi-md".to_string()),
                ..NodeFilter::new()
            })
            .await?;
        assert!(
            leftovers.is_empty(),
            "no {node_type} node of the rejected batch may persist, got {leftovers:?}"
        );
    }

    let ok = nodespace_core::markdown::handle_create_nodes_from_markdown(
        &service,
        json!({
            "title": "bi markdown import ok",
            "markdown_content": "bi-ok paragraph\n\n- [ ] bi-ok allowed item",
            "sync_import": true
        }),
    )
    .await;
    assert!(ok.is_ok(), "a compliant import must succeed, got {ok:?}");
    Ok(())
}

// ---------------------------------------------------------------------------
// bulk_create_hierarchy_in_tx (schema description subtree)
// ---------------------------------------------------------------------------

/// A schema's description subtree is created through
/// `bulk_create_hierarchy_in_tx` inside the schema's own transaction, so a
/// description node violating an invariant fails the whole schema create.
#[tokio::test]
async fn schema_description_violating_an_invariant_fails_the_schema_create() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let _engine = activate_rules(
        &service,
        reject_on_create(
            "text",
            "node.content.contains('forbidden')",
            "no forbidden text",
        ),
    );

    let rejected = nodespace_core::schema::handle_create_schema(
        &service,
        json!({
            "name": "bi_described",
            "description": "a forbidden description paragraph",
            "fields": [{ "name": "state", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await;
    assert!(rejected.is_err(), "got {rejected:?}");
    assert!(
        service.get_node("bi_described").await?.is_none(),
        "the schema node must roll back with its description"
    );

    nodespace_core::schema::handle_create_schema(
        &service,
        json!({
            "name": "bi_described_ok",
            "description": "an allowed description paragraph",
            "fields": [{ "name": "state", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await?;
    assert!(service.get_node("bi_described_ok").await?.is_some());
    Ok(())
}

// ---------------------------------------------------------------------------
// Batch-guard composition and sync no-op for updates
// ---------------------------------------------------------------------------

/// The sync replay falls back to per-row writes when a batched write fails,
/// under the same `begin_batch_emit` guard. A rejected bulk write inside the
/// guard must contribute no events, and a later successful write in the same
/// guard must still deliver its events when the guard drops.
#[tokio::test]
async fn rejected_bulk_create_inside_a_batch_guard_contributes_no_events() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "bi_guarded",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    let _engine = activate_rules(
        &service,
        reject_on_create("bi_guarded", "node.status == 'blocked'", "no blocked nodes"),
    );
    let mut rx = service.subscribe_to_events();

    let bad = Node::new(
        "bi_guarded".to_string(),
        "blocked".to_string(),
        json!({ "bi_guarded": { "status": "blocked" } }),
    );
    let good = Node::new(
        "bi_guarded".to_string(),
        "fine".to_string(),
        json!({ "bi_guarded": { "status": "open" } }),
    );
    let (bad_id, good_id) = (bad.id.clone(), good.id.clone());
    {
        let _batch = service.begin_batch_emit();
        assert!(service.bulk_create(vec![bad]).await.is_err());
        service.bulk_create(vec![good]).await?;
    }

    let mut created = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let DomainEvent::NodeCreated { node_id, .. } = envelope.event {
            created.push(node_id);
        }
    }
    assert_eq!(
        created,
        vec![good_id],
        "the rejected {bad_id} must emit nothing"
    );
    Ok(())
}

#[tokio::test]
async fn sync_tagged_bulk_update_does_not_run_invariant_rules() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "bi_sync_upd",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    let _engine = activate_rules(&service, reject_on_status_change("bi_sync_upd"));

    let node = Node::new(
        "bi_sync_upd".to_string(),
        "n".to_string(),
        json!({ "status": "open" }),
    );
    let id = node.id.clone();
    service.create_node(node).await?;

    service
        .with_client(REPLICATED_APPLY_CLIENT_ID)
        .bulk_update(vec![(id.clone(), status_update("blocked"))])
        .await?;
    let after = service.get_node(&id).await?.unwrap();
    assert_eq!(
        user_field(&after, "bi_sync_upd", "status"),
        Some(&json!("blocked"))
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// bulk_create collection-name collision journaling
// ---------------------------------------------------------------------------

fn collection(name: &str, lifecycle: &str) -> Node {
    let mut node = Node::new("collection".to_string(), name.to_string(), json!({}));
    node.title = Some(name.to_string());
    node.lifecycle_status = lifecycle.to_string();
    node
}

async fn has_collision(service: &NodeService, id: &str) -> Result<bool> {
    use nodespace_core::models::conflict::{ConflictKind, ConflictStatus};
    Ok(service.conflicts_for_node(id).await?.iter().any(|r| {
        r.kind == ConflictKind::CollectionNameCollision && r.status == ConflictStatus::Open
    }))
}

/// `bulk_create` journals a collection-name collision (ADR-065/068) against a
/// stored collection and against an earlier active row of the same batch —
/// what the same rows created one at a time would journal — and never
/// against an archived row.
#[tokio::test]
async fn bulk_create_journals_collection_name_collisions() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    let stored = collection("Stored Name", "active");
    let stored_id = stored.id.clone();
    service.bulk_create(vec![stored]).await?;

    let vs_stored = collection("stored name", "active");
    let first = collection("Batch Name", "active");
    let second = collection("BATCH NAME", "active");
    let archived = collection("Archived Name", "archived");
    let after_archived = collection("archived name", "active");
    let ids: Vec<String> = [&vs_stored, &first, &second, &archived, &after_archived]
        .iter()
        .map(|n| n.id.clone())
        .collect();
    service
        .bulk_create(vec![vs_stored, first, second, archived, after_archived])
        .await?;

    assert!(
        has_collision(&service, &ids[0]).await?,
        "collides with a stored collection"
    );
    assert!(
        has_collision(&service, &stored_id).await?,
        "both sides are journaled"
    );
    assert!(
        has_collision(&service, &ids[2]).await?,
        "collides with an earlier row"
    );
    assert!(
        has_collision(&service, &ids[1]).await?,
        "the earlier row is journaled too"
    );
    assert!(
        !has_collision(&service, &ids[4]).await?,
        "an archived row frees its name"
    );
    Ok(())
}
