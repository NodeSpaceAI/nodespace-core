//! Integration tests for `RuleClass::Invariant` — synchronous, pre-commit,
//! fail-closed rule execution (ADR-060 §1) and repair-and-log for a node
//! received via sync that violates an invariant this device holds
//! (ADR-060 §7).
//!
//! Structured to mirror `playbook_engine_integration_test.rs`'s harness
//! (real `NodeService` + `SqliteStore`, and for the repair tests a real
//! running `PlaybookEngine`), extended with `set_playbook_lifecycle` — the
//! new wiring the write path needs to look up and dispatch invariant rules.
//!
//! Covers, per the acceptance criteria:
//! - An invariant rule's action executes inside the same transaction as the
//!   triggering write (no polling needed — it is synchronous).
//! - An invariant action failure genuinely rolls back the WHOLE node
//!   creation — the node does not exist afterward, not merely "the action's
//!   effect is missing."
//! - An invariant rule does not re-execute on a device that received the
//!   node via sync (`with_client(SYNC_SERVICE_CLIENT_ID)`).
//! - A device receiving a node that violates an invariant it holds repairs
//!   the node and logs the repair.
//! - Reactive rules are unaffected by any of the above.

use anyhow::Result;
use nodespace_core::db::events::SYNC_SERVICE_CLIENT_ID;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::services::{NodeService, NodeServiceError};
use nodespace_core::PlaybookEngine;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;
use tokio::time::timeout;

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

async fn create_play(
    service: &NodeService,
    name: &str,
    rules: serde_json::Value,
) -> Result<String> {
    let play = Node::new(
        "play".to_string(),
        name.to_string(),
        json!({ "rules": rules }),
    );
    let id = play.id.clone();
    service.create_node(play).await?;
    Ok(id)
}

fn user_field<'a>(node: &'a Node, node_type: &str, field: &str) -> Option<&'a serde_json::Value> {
    node.properties.get(node_type).and_then(|p| p.get(field))
}

async fn wait_until<F, Fut>(mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..80 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

async fn spawn_engine(
    service: &Arc<NodeService>,
) -> (
    Arc<PlaybookEngine>,
    watch::Sender<bool>,
    tokio::task::JoinHandle<Result<()>>,
) {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let engine = Arc::new(PlaybookEngine::new(Arc::clone(service)));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let task = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.start(shutdown_rx).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    (engine, shutdown_tx, task)
}

async fn shutdown_engine(
    shutdown_tx: watch::Sender<bool>,
    task: tokio::task::JoinHandle<Result<()>>,
) {
    let _ = shutdown_tx.send(true);
    let _ = timeout(Duration::from_secs(2), task).await;
}

/// The canonical invariant rule shape: on creation of a node of `node_type`,
/// stamp `approved: true` on the trigger node itself, inside the same
/// transaction.
fn stamp_approved_invariant_rule(node_type: &str) -> serde_json::Value {
    json!([{
        "name": "stamp-approved",
        "class": "invariant",
        "trigger": { "type": "graph_event", "on": "node_created", "node_type": node_type },
        "conditions": ["node.status == 'pending'"],
        "actions": [{
            "action_type": "update_node",
            "params": {
                "node_id": "{trigger.node.id}",
                "properties": { "approved": true }
            }
        }]
    }])
}

// ---------------------------------------------------------------------------
// Slice B: synchronous pre-commit execution
// ---------------------------------------------------------------------------

/// An invariant rule's action executes inside the SAME transaction as the
/// triggering write — synchronously, with no engine loop involved at all
/// (`NodeService::create_node` alone, no `spawn_engine`/polling). If dispatch
/// were async/post-commit like the reactive path, this node would need
/// `wait_until` to observe the stamp; here it must already be present the
/// instant `create_node` returns.
#[tokio::test]
async fn invariant_action_executes_synchronously_in_same_transaction() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_task",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;

    // Directly wire the write path to a lifecycle manager holding this rule —
    // no running engine loop needed, since dispatch happens inline in
    // `create_node`, not through the engine's event subscriber.
    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "stamp-play".to_string(),
        json!({ "rules": stamp_approved_invariant_rule("iv_task") }),
    );
    // Activate directly against the lifecycle manager (no engine loop
    // running to pick up the NodeCreated event reactively) — this test is
    // specifically about the write-path hook, not play installation.
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let triggering = Node::new(
        "iv_task".to_string(),
        "needs approval".to_string(),
        json!({ "status": "pending" }),
    );
    let triggering_id = triggering.id.clone();
    service.create_node(triggering).await?;

    // No wait_until: check immediately.
    let created = service
        .get_node(&triggering_id)
        .await?
        .expect("node must exist");
    assert_eq!(
        user_field(&created, "iv_task", "approved"),
        Some(&json!(true)),
        "invariant action must have already run by the time create_node returned"
    );

    Ok(())
}

/// Adversarial: an invariant action failure must roll back the WHOLE
/// transaction — the triggering node must not exist afterward at all, not
/// merely be missing the action's effect. Forces a genuine runtime failure
/// (not a save-time validation rejection): `add_relationship` to
/// `member_of` targeting the trigger node itself, which is not a
/// `collection` node — `create_relationship_in_tx`'s built-in-type check
/// rejects it. `edge_data.order` is supplied so the rule passes save-time
/// eligibility and the failure is genuinely at execution time.
#[tokio::test]
async fn invariant_action_failure_rolls_back_the_whole_node_creation() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_gated",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "broken-membership-play".to_string(),
        json!({ "rules": [{
            "name": "self-member-of-non-collection",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_gated" },
            "conditions": [],
            "actions": [{
                "action_type": "add_relationship",
                "params": {
                    "source_id": "{trigger.node.id}",
                    "relationship_type": "member_of",
                    "target_id": "{trigger.node.id}",
                    "edge_data": { "order": 1.0 }
                }
            }]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let doomed = Node::new(
        "iv_gated".to_string(),
        "should never exist".to_string(),
        json!({ "status": "pending" }),
    );
    let doomed_id = doomed.id.clone();

    let result = service.create_node(doomed).await;
    assert!(
        result.is_err(),
        "create_node must fail when its invariant rule's action fails"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Invariant rule") && msg.contains("failed"),
        "error should name the invariant-rule failure: {msg}"
    );

    // The real assertion: genuinely absent, not present-but-unlinked.
    let after = service.get_node(&doomed_id).await?;
    assert!(
        after.is_none(),
        "the triggering node must not exist at all after an invariant action failure — \
         got {after:?}"
    );

    Ok(())
}

/// A second, independent invariant rule in the SAME transaction as a
/// successful one still causes a full rollback — proves the fail-closed
/// guarantee holds across multiple rules matching the same trigger, not
/// just a single-rule case.
#[tokio::test]
async fn one_failing_invariant_rule_rolls_back_another_rule_s_successful_effect_too() -> Result<()>
{
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_multi",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    // Two rules matching the SAME trigger: rule[0] succeeds (stamps
    // approved=true), rule[1] fails (bad member_of target). Stable ordering
    // (play_id, rule_index) guarantees rule[0] runs and "succeeds" before
    // rule[1] fails — proving the rollback undoes rule[0]'s already-applied
    // write too, not just prevents rule[1]'s.
    let play_node = Node::new(
        "play".to_string(),
        "multi-rule-play".to_string(),
        json!({ "rules": [
            {
                "name": "stamp-first",
                "class": "invariant",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_multi" },
                "conditions": [],
                "actions": [{
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "approved": true } }
                }]
            },
            {
                "name": "fail-second",
                "class": "invariant",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_multi" },
                "conditions": [],
                "actions": [{
                    "action_type": "add_relationship",
                    "params": {
                        "source_id": "{trigger.node.id}",
                        "relationship_type": "member_of",
                        "target_id": "{trigger.node.id}",
                        "edge_data": { "order": 1.0 }
                    }
                }]
            }
        ] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let doomed = Node::new(
        "iv_multi".to_string(),
        "should never exist".to_string(),
        json!({ "status": "pending" }),
    );
    let doomed_id = doomed.id.clone();
    let result = service.create_node(doomed).await;
    assert!(
        result.is_err(),
        "the second rule's failure must fail the whole create"
    );

    let after = service.get_node(&doomed_id).await?;
    assert!(
        after.is_none(),
        "rule[0]'s already-applied stamp must be rolled back along with the node itself"
    );

    Ok(())
}

/// ADR-060 §1: an invariant rule must NOT re-execute on a device that
/// received the node via sync — the effect arrives WITH the node as ordinary
/// synced data. Uses the exact `with_client(SYNC_SERVICE_CLIENT_ID)` tagging
/// convention `nodespace-sync`'s real apply path uses.
#[tokio::test]
async fn invariant_rule_does_not_re_execute_on_sync_applied_node() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_sync_task",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "sync-stamp-play".to_string(),
        json!({ "rules": stamp_approved_invariant_rule("iv_sync_task") }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let sync_service = service.with_client(SYNC_SERVICE_CLIENT_ID);
    let synced = Node::new(
        "iv_sync_task".to_string(),
        "arrived via sync".to_string(),
        json!({ "status": "pending" }),
    );
    let synced_id = synced.id.clone();
    // Must succeed (sync-apply is not blocked by invariant dispatch at all —
    // dispatch is skipped outright, not "runs and happens to pass").
    sync_service.create_node(synced).await?;

    let after = service
        .get_node(&synced_id)
        .await?
        .expect("sync-applied node must still be created");
    assert_eq!(
        user_field(&after, "iv_sync_task", "approved"),
        None,
        "a sync-applied node must NOT have the invariant's effect applied locally — \
         it must arrive as plain synced data only"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Slice C: repair-and-log (ADR-060 §7)
// ---------------------------------------------------------------------------

/// A device receiving an already-committed node that violates an invariant
/// it holds repairs the node and creates a log node recording the repair.
/// Requires a REAL running engine (unlike the write-path tests above): repair
/// dispatch lives in `handle_event`'s post-commit sync-gate branch.
#[tokio::test]
async fn sync_applied_node_violating_invariant_is_repaired_and_logged() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_repair_task",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;
    create_schema(&service, "playbook_log", json!([])).await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    create_play(
        &service,
        "repair-stamp-play",
        stamp_approved_invariant_rule("iv_repair_task"),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Simulate a node that arrived via sync from an origin device that
    // predates (or had disabled) this invariant rule: matches the trigger
    // and its condition still passes (no `approved` stamp present).
    let sync_service = service.with_client(SYNC_SERVICE_CLIENT_ID);
    let violating = Node::new(
        "iv_repair_task".to_string(),
        "violates the invariant".to_string(),
        json!({ "status": "pending" }),
    );
    let violating_id = violating.id.clone();
    sync_service.create_node(violating).await?;

    let repaired = wait_until(|| {
        let service = Arc::clone(&service);
        let id = violating_id.clone();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, "iv_repair_task", "approved") == Some(&json!(true))
            )
        }
    })
    .await;
    assert!(
        repaired,
        "the violating node must be repaired (approved=true applied)"
    );

    let log_found = wait_until(|| {
        let service = Arc::clone(&service);
        async move {
            match service
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
            {
                Ok(logs) => logs.iter().any(|n| {
                    n.properties
                        .get("playbook_log")
                        .and_then(|p| p.get("kind"))
                        .and_then(|v| v.as_str())
                        == Some("repair")
                }),
                Err(_) => false,
            }
        }
    })
    .await;
    assert!(
        log_found,
        "a repair log node must be created recording what was repaired"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// A node received via sync whose invariant is ALREADY satisfied (the
/// originating device correctly applied it, so it arrives with the effect
/// already present) must not be touched or logged — repair is for
/// violations, not every synced node matching the trigger.
#[tokio::test]
async fn sync_applied_node_already_satisfying_invariant_is_not_touched() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_ok_task",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;
    create_schema(&service, "playbook_log", json!([])).await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;
    create_play(
        &service,
        "already-ok-play",
        stamp_approved_invariant_rule("iv_ok_task"),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Condition is `node.status == 'pending'`; this node has status
    // 'approved-elsewhere' so the condition is false — nothing to repair.
    let sync_service = service.with_client(SYNC_SERVICE_CLIENT_ID);
    let already_fine = Node::new(
        "iv_ok_task".to_string(),
        "already handled by origin".to_string(),
        json!({ "status": "approved-elsewhere" }),
    );
    let id = already_fine.id.clone();
    sync_service.create_node(already_fine).await?;

    // Give the engine a real chance to have processed this (and prove a
    // negative isn't just "too fast to observe").
    tokio::time::sleep(Duration::from_millis(200)).await;

    let node = service.get_node(&id).await?.unwrap();
    assert_eq!(
        user_field(&node, "iv_ok_task", "status"),
        Some(&json!("approved-elsewhere")),
        "a node whose invariant condition already fails must be left exactly as synced"
    );
    let logs = service
        .query_nodes_by_type("playbook_log", Some("active"))
        .await?;
    assert!(
        logs.is_empty(),
        "no repair should be logged when nothing was violated"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Reactive rules unaffected
// ---------------------------------------------------------------------------

/// A reactive rule in the same play/engine as an invariant rule fires
/// exactly as it always has (async, post-commit) — invariant dispatch does
/// not interfere with or substitute for it.
#[tokio::test]
async fn reactive_rule_still_fires_normally_alongside_an_invariant_rule() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_mixed_a",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;
    create_schema(
        &service,
        "iv_mixed_b",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;
    create_play(
        &service,
        "mixed-play",
        json!([
            {
                "name": "invariant-stamp",
                "class": "invariant",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_mixed_a" },
                "conditions": ["node.status == 'pending'"],
                "actions": [{
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "approved": true } }
                }]
            },
            {
                "name": "reactive-close",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_mixed_b" },
                "conditions": ["node.status == 'open'"],
                "actions": [{
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "status": "done" } }
                }]
            }
        ]),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Invariant half: synchronous.
    let a = Node::new(
        "iv_mixed_a".to_string(),
        "a".to_string(),
        json!({ "status": "pending" }),
    );
    let a_id = a.id.clone();
    service.create_node(a).await?;
    let a_after = service.get_node(&a_id).await?.unwrap();
    assert_eq!(
        user_field(&a_after, "iv_mixed_a", "approved"),
        Some(&json!(true))
    );

    // Reactive half: asynchronous, needs polling — unchanged behavior.
    let b = Node::new(
        "iv_mixed_b".to_string(),
        "b".to_string(),
        json!({ "status": "open" }),
    );
    let b_id = b.id.clone();
    service.create_node(b).await?;
    let fired = wait_until(|| {
        let service = Arc::clone(&service);
        let id = b_id.clone();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, "iv_mixed_b", "status").and_then(|v| v.as_str()) == Some("done")
            )
        }
    })
    .await;
    assert!(fired, "the reactive rule must still fire normally");

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Seeded-play protection (ADR-060 §8) — warn on disable, via the real engine
// ---------------------------------------------------------------------------

/// Disabling a seeded play that carries an invariant rule surfaces an
/// explicit warning naming the concrete consequence (a `playbook_log` node),
/// not just a silent lifecycle_status flip.
#[tokio::test]
async fn disabling_a_seeded_invariant_play_logs_a_warning() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_seeded_task",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    create_schema(&service, "playbook_log", json!([])).await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    let default_rules = json!([{
        "name": "seeded-invariant-rule",
        "class": "invariant",
        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_seeded_task" },
        "conditions": [],
        "actions": []
    }]);
    let play = Node::new_with_id(
        "pb-seeded-warn".to_string(),
        "play".to_string(),
        "Seeded Warn Play".to_string(),
        json!({ "rules": default_rules, "_seed": { "default_rules": default_rules } }),
    );
    service.create_node(play).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Disable it.
    let current = service.get_node("pb-seeded-warn").await?.unwrap();
    let update =
        nodespace_core::models::NodeUpdate::default().with_lifecycle_status("archived".to_string());
    service
        .update_node("pb-seeded-warn", current.version, update)
        .await?;

    let warned = wait_until(|| {
        let service = Arc::clone(&service);
        async move {
            match service
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
            {
                Ok(logs) => logs.iter().any(|n| {
                    let content = &n.content;
                    content.contains("seeded-invariant-rule") && content.contains("pb-seeded-warn")
                }),
                Err(_) => false,
            }
        }
    })
    .await;
    assert!(
        warned,
        "disabling a seeded invariant play must log a warning naming the play and rule"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Adversarial: cross-rule chaining prevention (ADR-060 §2, general case)
// ---------------------------------------------------------------------------

/// Save-time validation only catches the STATICALLY DECIDABLE self-chaining
/// case (a rule's own action re-satisfying its own trigger) — see
/// `validation.rs`'s own doc. It cannot see that rule A's `create_node`
/// action produces a node type a DIFFERENT active invariant rule (rule B)
/// triggers on; that general case is prevented structurally at the write
/// path instead (`NodeService::insert_node_in_tx_no_invariant_dispatch`,
/// called by the invariant action executor instead of the
/// dispatch-including `create_node_in_tx`). This test proves that
/// structural prevention actually holds at runtime: two independently valid
/// invariant rules, wired so rule A's action creates exactly the node type
/// rule B triggers on, must NOT have rule B fire as a side effect of rule
/// A's action.
#[tokio::test]
async fn an_invariant_action_creating_a_node_does_not_trigger_another_invariant_rule() -> Result<()>
{
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_chain_source",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    create_schema(
        &service,
        "iv_chain_target",
        json!([
            { "name": "note", "type": "string" },
            { "name": "stamped_by_rule_b", "type": "boolean" }
        ]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());

    // Rule A: on iv_chain_source creation, create an iv_chain_target node.
    // Rule B: on iv_chain_target creation, stamp stamped_by_rule_b=true on it.
    // Individually, each rule is valid (neither self-chains) and would pass
    // save-time eligibility. If dispatch recursed, rule A's own create_node
    // would trigger rule B inline, inside the very same transaction.
    let play_a = Node::new(
        "play".to_string(),
        "chain-rule-a".to_string(),
        json!({ "rules": [{
            "name": "create-target",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_chain_source" },
            "conditions": [],
            "actions": [{
                "action_type": "create_node",
                "params": { "node_type": "iv_chain_target", "content": "created by rule A" }
            }]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_a)
            .expect("play A must parse and activate");
    }

    let play_b = Node::new(
        "play".to_string(),
        "chain-rule-b".to_string(),
        json!({ "rules": [{
            "name": "stamp-target",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_chain_target" },
            "conditions": [],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "stamped_by_rule_b": true }
                }
            }]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_b)
            .expect("play B must parse and activate");
    }

    let source = Node::new(
        "iv_chain_source".to_string(),
        "trigger for the chain".to_string(),
        json!({ "status": "pending" }),
    );
    service.create_node(source).await?;

    // Find the node rule A's action created (derived id — query by type
    // instead of computing the id, to keep this test's assertion independent
    // of the derivation formula's internals).
    let targets = service
        .query_nodes_by_type("iv_chain_target", Some("active"))
        .await?;
    assert_eq!(
        targets.len(),
        1,
        "rule A's create_node action must have produced exactly one iv_chain_target node"
    );
    assert_eq!(
        user_field(&targets[0], "iv_chain_target", "stamped_by_rule_b"),
        None,
        "rule B must NOT have fired as a side effect of rule A's action within the same \
         transaction — invariant rules are non-chaining, depth 1 (ADR-060 §2)"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Regression: an invariant rule must not ALSO be enqueued onto the reactive
// ExecutionQueue for the event that triggered it
// ---------------------------------------------------------------------------

/// Adversarial regression test. Requires a REAL running `PlaybookEngine`
/// (unlike `invariant_action_executes_synchronously_in_same_transaction`,
/// which wires the lifecycle manager directly with no engine loop) — this is
/// specifically about what `PlaybookEngine::handle_event`'s event-subscriber
/// path does with a LOCAL `NodeCreated` event once the synchronous in-tx
/// dispatch has already fully handled it.
///
/// Before the fix, `handle_event`'s local-event branch looked up matching
/// rules with no `RuleClass` filter and enqueued ALL of them — including
/// `RuleClass::Invariant` ones — onto the reactive `ExecutionQueue`.
/// `rule_processor_loop` then re-ran the SAME rule a second time,
/// asynchronously, post-commit, fail-open (no rollback): a genuinely
/// idempotent action (`create_node`/`add_relationship`) would mask this via
/// derived-identity convergence, but `update_node` is not idempotent against
/// itself here — each redundant run bumps `version` and emits a second
/// `NodeUpdated` event. Directly asserts on `version`, which only advances
/// via a genuine write to the row: if the invariant rule's `update_node`
/// action ran the synchronous, correct time PLUS a second, spurious,
/// reactive-queue time, `version` observed some time after `create_node`
/// returns would be higher than immediately after it returns.
#[tokio::test]
async fn invariant_rule_does_not_also_run_via_the_reactive_queue() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_no_double_exec",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;
    create_play(
        &service,
        "no-double-exec-play",
        stamp_approved_invariant_rule("iv_no_double_exec"),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let triggering = Node::new(
        "iv_no_double_exec".to_string(),
        "must only be stamped once".to_string(),
        json!({ "status": "pending" }),
    );
    let triggering_id = triggering.id.clone();
    service.create_node(triggering).await?;

    // Immediately after create_node returns, the synchronous in-tx stamp
    // must already be visible (version 2: 1 for the insert, 1 for the
    // in-tx update_node action).
    let immediately_after = service.get_node(&triggering_id).await?.unwrap();
    assert_eq!(
        user_field(&immediately_after, "iv_no_double_exec", "approved"),
        Some(&json!(true)),
        "the synchronous invariant stamp must already be applied when create_node returns"
    );
    let version_immediately_after = immediately_after.version;

    // Give the reactive engine's ExecutionQueue every chance to have
    // (incorrectly) re-run the same rule if the bug were present — several
    // multiples of the polling interval used elsewhere in this file.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let later = service.get_node(&triggering_id).await?.unwrap();
    assert_eq!(
        later.version, version_immediately_after,
        "the node's version must not advance after the synchronous in-tx stamp — a higher \
         version here means the SAME invariant rule ran a second time via the reactive \
         ExecutionQueue, which must never enqueue RuleClass::Invariant rules at all"
    );
    assert_eq!(
        user_field(&later, "iv_no_double_exec", "approved"),
        Some(&json!(true)),
        "still stamped exactly once"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// The `reject` action type (ADR-060 §2)
// ---------------------------------------------------------------------------
//
// `reject`'s save-time class gate (only usable on `RuleClass::Invariant`
// rules) and its "message" param requirement are covered as unit/integration
// tests of `validate_play` in `playbook::validation`'s own test module, not
// here — these tests are specifically about the EXECUTION-time contract:
// does a reject action, once reached, genuinely veto the write with zero
// partial state, using `create_node`'s already-existing synchronous
// invariant path (the only real caller today; a planned follow-up wires the
// equivalent path for `update_node`). Like the rollback tests above, these activate the
// play directly against the lifecycle manager (bypassing
// `validate_play_rules`) since they are about the write-path hook, not play
// installation.

/// An invariant rule whose sole action is `reject`, gated by `condition` on
/// the trigger node.
fn reject_invariant_rule(node_type: &str, condition: &str, message: &str) -> serde_json::Value {
    json!([{
        "name": "reject-rule",
        "class": "invariant",
        "trigger": { "type": "graph_event", "on": "node_created", "node_type": node_type },
        "conditions": [condition],
        "actions": [{
            "action_type": "reject",
            "params": { "message": message }
        }]
    }])
}

/// The baseline case: a reject action, once its condition is met, prevents
/// the triggering node from ever existing — the same "no partial write"
/// guarantee an ordinary action failure already gets, but reached
/// deliberately (via the rule's own condition) rather than incidentally (a
/// service error).
#[tokio::test]
async fn reject_action_prevents_node_creation_with_no_partial_write() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_reject_basic",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-basic-play".to_string(),
        json!({ "rules": reject_invariant_rule(
            "iv_reject_basic",
            "node.status == 'blocked'",
            "cannot create a blocked iv_reject_basic node",
        ) }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let doomed = Node::new(
        "iv_reject_basic".to_string(),
        "should never exist".to_string(),
        json!({ "status": "blocked" }),
    );
    let doomed_id = doomed.id.clone();

    let result = service.create_node(doomed).await;
    assert!(
        result.is_err(),
        "create_node must fail when its invariant rule rejects the write"
    );

    let after = service.get_node(&doomed_id).await?;
    assert!(
        after.is_none(),
        "the triggering node must not exist at all after a reject — got {after:?}"
    );

    Ok(())
}

/// The error returned is specifically `NodeServiceError::PlayRuleRejected`
/// (modeled on `VersionConflict`), not the generic `InvariantRuleFailed`
/// every other action failure produces, and it carries the rule's own
/// author-supplied message verbatim.
#[tokio::test]
async fn reject_action_error_is_play_rule_rejected_with_the_rule_s_message() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_reject_msg",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-msg-play".to_string(),
        json!({ "rules": reject_invariant_rule(
            "iv_reject_msg",
            "node.status == 'blocked'",
            "custom violation text",
        ) }),
    );
    let play_node_id = play_node.id.clone();
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let doomed = Node::new(
        "iv_reject_msg".to_string(),
        "x".to_string(),
        json!({ "status": "blocked" }),
    );
    let doomed_id = doomed.id.clone();
    let err = service.create_node(doomed).await.unwrap_err();
    match err {
        NodeServiceError::PlayRuleRejected {
            node_id,
            play_id,
            rule_name,
            message,
        } => {
            assert_eq!(message, "custom violation text");
            assert_eq!(
                play_id, play_node_id,
                "play_id must be the play node's own id"
            );
            assert_eq!(
                rule_name, "reject-rule",
                "rule_name carries the rule's author-given name"
            );
            assert_eq!(
                node_id, doomed_id,
                "node_id must be the triggering node's id"
            );
        }
        other => panic!("expected PlayRuleRejected, got {:?}", other),
    }

    Ok(())
}

/// The rule's condition is a genuine gate: when it does not hold, the
/// reject action is never reached at all and creation proceeds normally.
/// Distinguishes "the reject action correctly fires only when its condition
/// matches" from an implementation that rejects unconditionally.
#[tokio::test]
async fn reject_action_condition_not_met_allows_normal_creation() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_reject_gated",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-gated-play".to_string(),
        json!({ "rules": reject_invariant_rule(
            "iv_reject_gated",
            "node.status == 'blocked'",
            "should never fire",
        ) }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let allowed = Node::new(
        "iv_reject_gated".to_string(),
        "fine".to_string(),
        json!({ "status": "open" }),
    );
    let allowed_id = allowed.id.clone();
    service.create_node(allowed).await?;

    let after = service.get_node(&allowed_id).await?;
    assert!(
        after.is_some(),
        "creation must succeed when the reject rule's condition does not hold"
    );

    Ok(())
}

/// Adversarial: `reject` as the FIRST action in a rule, with an augmenting
/// action after it. The augmenting action (updating a separate,
/// already-existing node) must never run at all — proven via that node's
/// `version`, which only advances on a genuine write.
#[tokio::test]
async fn reject_before_augmenting_action_in_the_same_rule_prevents_the_augment() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_reject_order_a",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    // Pre-existing node the (never-reached) augmenting action would target.
    let other = Node::new(
        "iv_reject_order_a".to_string(),
        "bystander".to_string(),
        json!({ "status": "untouched" }),
    );
    let other_id = other.id.clone();
    service.create_node(other).await?;
    let other_version_before = service.get_node(&other_id).await?.unwrap().version;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-before-augment-play".to_string(),
        json!({ "rules": [{
            "name": "reject-then-augment",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_reject_order_a" },
            "conditions": [],
            "actions": [
                {
                    "action_type": "reject",
                    "params": { "message": "vetoed before the augment runs" }
                },
                {
                    "action_type": "update_node",
                    "params": {
                        "node_id": other_id,
                        "properties": { "status": "should never be set" }
                    }
                }
            ]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let doomed = Node::new(
        "iv_reject_order_a".to_string(),
        "trigger".to_string(),
        json!({ "status": "pending" }),
    );
    let doomed_id = doomed.id.clone();
    let result = service.create_node(doomed).await;
    assert!(result.is_err(), "expected the write to be rejected");

    assert!(
        service.get_node(&doomed_id).await?.is_none(),
        "the triggering node must not exist"
    );

    let other_after = service.get_node(&other_id).await?.unwrap();
    assert_eq!(
        other_after.version, other_version_before,
        "the augmenting action after reject must never have run"
    );
    assert_eq!(
        user_field(&other_after, "iv_reject_order_a", "status"),
        Some(&json!("untouched")),
        "the augmenting action's write must not be visible"
    );

    Ok(())
}

/// Adversarial: a play whose rule declares `reject` on a `Reactive` class —
/// invalid per `validate_reject_action_class`, but persisted here via a
/// direct `SqliteStore::create_node` call that bypasses `NodeService`'s
/// `validate_play_rules` save-time gate entirely, simulating a play row that
/// reached the DB before this validation existed (an earlier build) or from
/// a device running different validation rules (ADR-060 is explicitly about
/// multi-device sync). `PlaybookEngine::start()`'s `load_active_plays()`
/// re-validates at load time specifically to catch this: without it, this
/// play would activate unvalidated on every restart and then disable itself
/// entirely (not just the offending rule) the first time its trigger fired,
/// since `execute_reject` always errors when reached.
#[tokio::test]
async fn reject_on_reactive_rule_bypassing_save_time_validation_is_not_activated_at_load(
) -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);

    create_schema(
        &service,
        "iv_reject_bypass",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let invalid_play = Node::new(
        "play".to_string(),
        "invalid-reject-on-reactive".to_string(),
        json!({ "rules": [{
            "name": "reject-on-reactive",
            "class": "reactive",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_reject_bypass" },
            "conditions": [],
            "actions": [{
                "action_type": "reject",
                "params": { "message": "should never reach a caller — this play must not activate" }
            }]
        }] }),
    );
    let invalid_play_id = invalid_play.id.clone();

    // Bypasses NodeService::create_node's validate_play_rules gate entirely —
    // the point of this test.
    store.create_node(invalid_play, None, None).await?;

    let (engine, shutdown_tx, task) = spawn_engine(&service).await;

    let is_active = {
        let lifecycle = engine.lifecycle();
        let lm = lifecycle.read().unwrap();
        lm.get_play(&invalid_play_id).is_some()
    };
    assert!(
        !is_active,
        "a play whose rule fails save-time validation must not be activated at load time, \
         even when it reached the DB by bypassing the normal save path"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Adversarial, the other ordering: an augmenting action that succeeds
/// FIRST, followed by `reject`. Whole-transaction rollback must undo the
/// already-applied augmenting write too, not just prevent the triggering
/// node from existing — proving `reject`'s veto is not merely "stop before
/// doing more" but a genuine transaction-wide abort.
#[tokio::test]
async fn augmenting_action_before_reject_in_the_same_rule_is_rolled_back_too() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_reject_order_b",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let other = Node::new(
        "iv_reject_order_b".to_string(),
        "bystander".to_string(),
        json!({ "status": "untouched" }),
    );
    let other_id = other.id.clone();
    service.create_node(other).await?;
    let other_version_before = service.get_node(&other_id).await?.unwrap().version;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "augment-before-reject-play".to_string(),
        json!({ "rules": [{
            "name": "augment-then-reject",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "iv_reject_order_b" },
            "conditions": [],
            "actions": [
                {
                    "action_type": "update_node",
                    "params": {
                        "node_id": other_id,
                        "properties": { "status": "applied then must be undone" }
                    }
                },
                {
                    "action_type": "reject",
                    "params": { "message": "vetoed after the augment already ran" }
                }
            ]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let doomed = Node::new(
        "iv_reject_order_b".to_string(),
        "trigger".to_string(),
        json!({ "status": "pending" }),
    );
    let doomed_id = doomed.id.clone();
    let result = service.create_node(doomed).await;
    assert!(result.is_err(), "expected the write to be rejected");

    assert!(
        service.get_node(&doomed_id).await?.is_none(),
        "the triggering node must not exist"
    );

    let other_after = service.get_node(&other_id).await?.unwrap();
    assert_eq!(
        other_after.version, other_version_before,
        "the augmenting action's already-applied write must be rolled back \
         when a LATER action in the same rule rejects"
    );
    assert_eq!(
        user_field(&other_after, "iv_reject_order_b", "status"),
        Some(&json!("untouched")),
        "the augmenting action's write must not be durably visible after rollback"
    );

    Ok(())
}
