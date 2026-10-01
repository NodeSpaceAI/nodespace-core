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
//!   node via sync (`with_client(REPLICATED_APPLY_CLIENT_ID)`).
//! - A device receiving a node that violates an invariant it holds repairs
//!   the node and logs the repair.
//! - Reactive rules are unaffected by any of the above.

use anyhow::Result;
use nodespace_core::db::events::{
    DomainEvent, PLAYBOOK_CHAIN_DEPTH_PROPERTY, PLAYBOOK_WRITE_ID_PROPERTY,
    REPLICATED_APPLY_CLIENT_ID,
};
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{Node, NodeUpdate, Priority, TaskNodeUpdate, TaskStatus};
use nodespace_core::playbook::types::MAX_CHAIN_DEPTH;
use nodespace_core::services::{NodeService, NodeServiceError};
use nodespace_core::PlaybookEngine;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::watch;
use tokio::time::timeout;

/// Captures `tracing` output for the duration of a test, so a diagnostic the
/// engine emits as a `warn!` can be asserted on.
///
/// Play errors are operational telemetry rather than knowledge, so they are
/// logged rather than written into the graph; that makes the subscriber, not
/// a node query, the place to observe them.
struct CapturedLogs {
    buffer: Arc<std::sync::Mutex<Vec<u8>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

impl CapturedLogs {
    fn install() -> Self {
        let buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_writer(move || BufferWriter(writer.clone()))
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        Self {
            buffer,
            _guard: guard,
        }
    }

    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().expect("log buffer poisoned")).into_owned()
    }
}

struct BufferWriter(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

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
/// synced data. Uses the `with_client(REPLICATED_APPLY_CLIENT_ID)` tagging
/// convention that marks a replicated apply.
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

    let sync_service = service.with_client(REPLICATED_APPLY_CLIENT_ID);
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
    let sync_service = service.with_client(REPLICATED_APPLY_CLIENT_ID);
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
    let sync_service = service.with_client(REPLICATED_APPLY_CLIENT_ID);
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
/// explicit warning naming the concrete consequence, not just a silent
/// lifecycle_status flip. The warning goes to tracing (engine diagnostics are
/// not graph nodes), so this captures the subscriber output to assert on it.
#[tokio::test]
async fn disabling_a_seeded_invariant_play_logs_a_warning() -> Result<()> {
    let logs = CapturedLogs::install();
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_seeded_task",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

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

    tokio::time::sleep(Duration::from_millis(100)).await;

    let logged = logs.contents();
    assert!(
        logged.contains("seeded-invariant-rule") && logged.contains("pb-seeded-warn"),
        "disabling a seeded invariant play must log a warning naming the play and \
         rule; captured tracing output was: {logged}"
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

/// A reject rule registered on a subtype reads a field the subtype inherits.
///
/// `iv_sub_bug` extends `iv_sub_ticket` without redeclaring `state`, so a
/// bug's `state` is stored in the ancestor's bucket. A condition at the bug's
/// own scope must still see it: were it absent, the rule would never match and
/// the reject would silently allow the write it exists to veto. The open bug
/// proves the condition is a real gate rather than a blanket rejection.
#[tokio::test]
async fn reject_rule_on_a_subtype_reads_an_inherited_field() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({
            "name": "iv_sub_ticket",
            "fields": [{ "name": "state", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({ "name": "iv_sub_bug", "extends": "iv_sub_ticket", "fields": [] }),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-inherited-play".to_string(),
        json!({ "rules": reject_invariant_rule(
            "iv_sub_bug",
            "node.state == 'done'",
            "cannot create a done bug",
        ) }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let open = Node::new(
        "iv_sub_bug".to_string(),
        "open bug".to_string(),
        json!({ "state": "open" }),
    );
    let open_id = open.id.clone();
    service.create_node(open).await?;
    let stored = service
        .get_node(&open_id)
        .await?
        .expect("an open bug must be created");
    assert_eq!(
        user_field(&stored, "iv_sub_ticket", "state"),
        Some(&json!("open")),
        "precondition: the inherited field lives in the ancestor's bucket, got {}",
        stored.properties
    );

    let done = Node::new(
        "iv_sub_bug".to_string(),
        "done bug".to_string(),
        json!({ "state": "done" }),
    );
    let done_id = done.id.clone();
    let err = service.create_node(done).await.unwrap_err();
    assert!(
        matches!(err, NodeServiceError::PlayRuleRejected { .. }),
        "a done bug must be rejected by the rule, got {err:?}"
    );
    assert!(service.get_node(&done_id).await?.is_none());

    Ok(())
}

/// A `relationship_added` reject rule registered on a subtype reads a field
/// the edge's source inherits (ADR-078): relationship dispatch evaluates at the
/// rule's scope through the same shared execution core as `node_created`.
///
/// `status` lives in `iv_rel_ticket`'s bucket on an `iv_rel_bug`, so an
/// own-bucket read would see nothing and let the done bug's edge through.
#[tokio::test]
async fn relationship_reject_rule_on_a_subtype_reads_an_inherited_field() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({ "name": "iv_rel_target", "fields": [] }),
    )
    .await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({
            "name": "iv_rel_ticket",
            "fields": [{ "name": "status", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({
            "name": "iv_rel_bug",
            "extends": "iv_rel_ticket",
            "fields": [],
            "relationships": [{
                "name": "blocks",
                "targetType": "iv_rel_target",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "blocked_by",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-inherited-relationship-play".to_string(),
        json!({ "rules": [{
            "name": "reject-rule",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "relationship_added",
                "node_type": "iv_rel_bug",
            },
            "conditions": ["node.status == 'done'"],
            "actions": [{
                "action_type": "reject",
                "params": { "message": "a done bug cannot block anything" }
            }]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let target = service
        .create_node(Node::new(
            "iv_rel_target".to_string(),
            "target".to_string(),
            json!({}),
        ))
        .await?;
    let open = service
        .create_node(Node::new(
            "iv_rel_bug".to_string(),
            "open bug".to_string(),
            json!({ "status": "open" }),
        ))
        .await?;
    let done = service
        .create_node(Node::new(
            "iv_rel_bug".to_string(),
            "done bug".to_string(),
            json!({ "status": "done" }),
        ))
        .await?;

    let stored = service
        .get_node(&done)
        .await?
        .expect("the done bug must exist");
    assert_eq!(
        user_field(&stored, "iv_rel_ticket", "status"),
        Some(&json!("done")),
        "precondition: the inherited field lives in the ancestor's bucket, got {}",
        stored.properties
    );

    service
        .create_relationship(&open, "blocks", &target, json!({}))
        .await?;
    let err = service
        .create_relationship(&done, "blocks", &target, json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, NodeServiceError::PlayRuleRejected { .. }),
        "a done bug's edge must be rejected by the rule, got {err:?}"
    );

    let blockers: Vec<String> = service
        .get_related_nodes(&target, "blocks", "in")
        .await?
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(blockers, vec![open], "only the open bug's edge may land");

    Ok(())
}

/// A reject rule registered on a BASE type, fired by a subtype, reads the
/// subtype at the base's scope (ADR-078): an extended value is translated
/// through `maps_to` before the condition sees it.
///
/// `iv_base_bug` adds `backlog`, mapping to `open`, to the `state` it inherits
/// from `iv_base_ticket`. A ticket-scoped rule rejecting `open` must reject a
/// `backlog` bug, and must still allow a `done` one.
#[tokio::test]
async fn reject_rule_on_a_base_type_reads_a_subtype_through_maps_to() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({
            "name": "iv_base_ticket",
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
    .await?;
    nodespace_core::schema::handle_create_schema(
        &service,
        json!({ "name": "iv_base_bug", "extends": "iv_base_ticket", "fields": [] }),
    )
    .await?;
    nodespace_core::schema::handle_update_schema(
        &service,
        json!({
            "schema_id": "iv_base_bug",
            "add_field_values": [{
                "field": "state",
                "values": [{ "value": "backlog", "label": "Backlog", "mapsTo": "open" }]
            }]
        }),
    )
    .await?;
    create_play(
        &service,
        "reject-base-play",
        reject_invariant_rule("iv_base_ticket", "node.state == 'open'", "no open tickets"),
    )
    .await?;
    // The engine's start-up load activates the play and builds the ancestry
    // a base-type trigger needs to match a subtype.
    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    let done = Node::new(
        "iv_base_bug".to_string(),
        "done bug".to_string(),
        json!({ "state": "done" }),
    );
    let done_id = done.id.clone();
    service.create_node(done).await?;
    assert!(service.get_node(&done_id).await?.is_some());

    let backlog = Node::new(
        "iv_base_bug".to_string(),
        "backlog bug".to_string(),
        json!({ "state": "backlog" }),
    );
    let backlog_id = backlog.id.clone();
    let err = service.create_node(backlog).await.unwrap_err();
    assert!(
        matches!(err, NodeServiceError::PlayRuleRejected { .. }),
        "a backlog bug reads as `open` at ticket scope and must be rejected, got {err:?}"
    );
    assert!(service.get_node(&backlog_id).await?.is_none());

    shutdown_engine(shutdown_tx, task).await;
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

// ---------------------------------------------------------------------------
// update_node's synchronous invariant dispatch (ADR-060 §2)
// ---------------------------------------------------------------------------
//
// `create_node`'s dispatch (above) reuses the same `execute_matched_invariant_rules_in_tx`
// core, so reject+augment ordering and rollback atomicity are not
// re-proven here — they hold identically by construction. What's new and
// specific to update_node: the property_changed trigger-key matching
// (exact + wildcard, and that an UNRELATED property change must NOT match
// a property-key-scoped rule), and that update_node's caller — unlike
// create_node's, which has no prior state to diff against — must broadcast
// a real `NodeUpdated` event with accurate `changed_properties` on success
// and NONE at all on rejection.

/// The canonical UPDATE-triggered invariant rule: on a `property_key` change
/// of `node_type`, stamp `verified: true` on the trigger node itself, inside
/// the same transaction as the triggering update. `field` is the BARE
/// property name (e.g. "status") — `compute_property_changes` diffs a
/// node's own top-level namespace object (`{node_type: {field: ...}}`) and
/// reports the change under the namespaced key `"{node_type}.{field}"`, so
/// that is what a play's `property_key` must match against, not the bare
/// field name.
fn stamp_verified_on_update_invariant_rule(node_type: &str, field: &str) -> serde_json::Value {
    let property_key = format!("{node_type}.{field}");
    json!([{
        "name": "stamp-verified-on-update",
        "class": "invariant",
        "trigger": { "type": "graph_event", "on": "property_changed", "node_type": node_type, "property_key": property_key },
        "conditions": [],
        "actions": [{
            "action_type": "update_node",
            "params": {
                "node_id": "{trigger.node.id}",
                "properties": { "verified": true }
            }
        }]
    }])
}

fn properties_update(properties: serde_json::Value) -> NodeUpdate {
    NodeUpdate {
        properties: Some(properties),
        ..Default::default()
    }
}

/// An invariant rule triggered by `property_changed` executes synchronously,
/// inside the SAME transaction as `update_node`'s own write — checked
/// immediately, no `wait_until`/polling, exactly like the `create_node` case.
#[tokio::test]
async fn invariant_update_rule_executes_synchronously_in_same_transaction() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_update_task",
        json!([
            { "name": "status", "type": "string" },
            { "name": "verified", "type": "boolean" }
        ]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "stamp-verified-play".to_string(),
        json!({ "rules": stamp_verified_on_update_invariant_rule("iv_update_task", "status") }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = Node::new(
        "iv_update_task".to_string(),
        "task".to_string(),
        json!({ "status": "open" }),
    );
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    let updated = service
        .update_node(
            &node_id,
            version,
            properties_update(json!({ "status": "in_progress" })),
        )
        .await?;

    // No wait_until: check the RETURN VALUE of update_node itself.
    assert_eq!(
        user_field(&updated, "iv_update_task", "verified"),
        Some(&json!(true)),
        "invariant action must have already run by the time update_node returned"
    );
    assert_eq!(
        user_field(&updated, "iv_update_task", "status"),
        Some(&json!("in_progress")),
        "the triggering update itself must still have applied"
    );

    Ok(())
}

/// Adversarial: a reject firing on an update-triggered invariant rule
/// prevents the triggering update from taking effect at all — the node's
/// property stays at its pre-update value, not a mix of "some fields
/// updated, some not".
#[tokio::test]
async fn invariant_update_rule_reject_prevents_partial_write() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_update_reject",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-on-update-play".to_string(),
        json!({ "rules": [{
            "name": "reject-blocked-transition",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "iv_update_reject", "property_key": "iv_update_reject.status" },
            "conditions": ["node.status == 'blocked'"],
            "actions": [{
                "action_type": "reject",
                "params": { "message": "cannot transition to blocked" }
            }]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = Node::new(
        "iv_update_reject".to_string(),
        "task".to_string(),
        json!({ "status": "open" }),
    );
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let before = service.get_node(&node_id).await?.unwrap();

    let err = service
        .update_node(
            &node_id,
            before.version,
            properties_update(json!({ "status": "blocked" })),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, NodeServiceError::PlayRuleRejected { .. }),
        "expected PlayRuleRejected, got {:?}",
        err
    );

    let after = service.get_node(&node_id).await?.unwrap();
    assert_eq!(
        after.version, before.version,
        "a rejected update must not bump the node's version at all"
    );
    assert_eq!(
        user_field(&after, "iv_update_reject", "status"),
        Some(&json!("open")),
        "a rejected update must leave the property at its pre-update value"
    );

    Ok(())
}

/// No `DomainEvent` is broadcast for a rejected update — the buffered
/// `NodeUpdated` event `update_with_version_check_returning_node_in_tx`
/// emits before dispatch is discarded on rollback, never flushed.
#[tokio::test]
async fn invariant_update_rule_reject_emits_no_domain_event() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_update_no_broadcast",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-no-broadcast-play".to_string(),
        json!({ "rules": [{
            "name": "reject-always",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "iv_update_no_broadcast", "property_key": "iv_update_no_broadcast.status" },
            "conditions": [],
            "actions": [{
                "action_type": "reject",
                "params": { "message": "never allowed" }
            }]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = Node::new(
        "iv_update_no_broadcast".to_string(),
        "task".to_string(),
        json!({ "status": "open" }),
    );
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    // Subscribe AFTER the create (so its own NodeCreated event isn't sitting
    // in the channel) and BEFORE the rejected update.
    let mut rx = service.subscribe_to_events();

    let result = service
        .update_node(
            &node_id,
            version,
            properties_update(json!({ "status": "closed" })),
        )
        .await;
    assert!(result.is_err(), "expected the update to be rejected");

    match rx.try_recv() {
        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
        other => panic!(
            "expected no event to have been broadcast for a rejected update, got {:?}",
            other
        ),
    }

    Ok(())
}

/// Successful path, the mirror of the rejection test above: an invariant
/// rule's augmenting action commits atomically with the triggering update
/// AND a real `NodeUpdated` broadcast for the triggering update itself goes
/// out normally, since that write genuinely succeeded.
#[tokio::test]
async fn invariant_update_rule_augmenting_action_commits_and_broadcasts_normally() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_update_broadcast",
        json!([
            { "name": "status", "type": "string" },
            { "name": "verified", "type": "boolean" }
        ]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "augment-on-update-play".to_string(),
        json!({ "rules": stamp_verified_on_update_invariant_rule("iv_update_broadcast", "status") }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = Node::new(
        "iv_update_broadcast".to_string(),
        "task".to_string(),
        json!({ "status": "open" }),
    );
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    let mut rx = service.subscribe_to_events();

    let updated = service
        .update_node(
            &node_id,
            version,
            properties_update(json!({ "status": "in_progress" })),
        )
        .await?;
    assert_eq!(
        user_field(&updated, "iv_update_broadcast", "verified"),
        Some(&json!(true)),
        "augmenting action must have run"
    );

    let envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out waiting for a broadcast")
        .expect("channel must not have closed");
    match envelope.event {
        DomainEvent::NodeUpdated { node_id: id, .. } => {
            assert_eq!(
                id, node_id,
                "the broadcast must be for the triggering update"
            );
        }
        other => panic!("expected NodeUpdated, got {:?}", other),
    }

    Ok(())
}

/// A `property_changed` invariant rule scoped to a specific `property_key`
/// must not fire when a DIFFERENT property changes — proves
/// `dispatch_invariant_rules_for_update_in_tx` reuses the real exact/wildcard
/// trigger-key matching (`trigger_keys_for_event`), not a blanket "any
/// update to this node_type" match.
#[tokio::test]
async fn invariant_update_rule_scoped_to_one_property_ignores_a_different_property_change(
) -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_update_scoped",
        json!([
            { "name": "status", "type": "string" },
            { "name": "priority", "type": "string" },
            { "name": "verified", "type": "boolean" }
        ]),
    )
    .await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "scoped-property-play".to_string(),
        // Scoped to "status" only.
        json!({ "rules": stamp_verified_on_update_invariant_rule("iv_update_scoped", "status") }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = Node::new(
        "iv_update_scoped".to_string(),
        "task".to_string(),
        json!({ "status": "open", "priority": "low" }),
    );
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    // Change ONLY "priority" — must not match a rule scoped to "status".
    let updated = service
        .update_node(
            &node_id,
            version,
            properties_update(json!({ "priority": "high" })),
        )
        .await?;

    assert_eq!(
        user_field(&updated, "iv_update_scoped", "verified"),
        None,
        "a property-key-scoped invariant rule must not fire for an unrelated property change"
    );
    assert_eq!(
        user_field(&updated, "iv_update_scoped", "priority"),
        Some(&json!("high")),
        "the unrelated update itself must still have applied"
    );

    Ok(())
}

/// `RuleClass::Reactive` rules triggered by `property_changed` remain
/// completely unaffected by the new synchronous wiring: still async,
/// post-commit, requiring the real engine loop (unlike the invariant tests
/// above, which never spawn one).
#[tokio::test]
async fn reactive_update_rule_still_fires_asynchronously_post_commit() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_update_reactive",
        json!([
            { "name": "status", "type": "string" },
            { "name": "notified", "type": "boolean" }
        ]),
    )
    .await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;
    create_play(
        &service,
        "reactive-on-update-play",
        json!([{
            "name": "notify-on-status-change",
            // No "class" -> defaults to reactive.
            "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "iv_update_reactive", "property_key": "iv_update_reactive.status" },
            "conditions": [],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "notified": true }
                }
            }]
        }]),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let node = Node::new(
        "iv_update_reactive".to_string(),
        "task".to_string(),
        json!({ "status": "open" }),
    );
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    let updated = service
        .update_node(
            &node_id,
            version,
            properties_update(json!({ "status": "in_progress" })),
        )
        .await?;
    // Must NOT be synchronous for a reactive rule.
    assert_eq!(
        user_field(&updated, "iv_update_reactive", "notified"),
        None,
        "a reactive rule's effect must not be visible synchronously"
    );

    let fired = wait_until(|| {
        let service = Arc::clone(&service);
        let node_id = node_id.clone();
        async move {
            service
                .get_node(&node_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|n| {
                    user_field(&n, "iv_update_reactive", "notified") == Some(&json!(true))
                })
        }
    })
    .await;
    assert!(fired, "reactive rule must eventually fire post-commit");

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// `update_task_node` wiring (ADR-060 §2, closing the gap the generic
// `update_node` coverage above doesn't reach): `NodeService::update_task_node`
// is a separate write path from `update_node` — it calls
// `SqliteStore::update_task_node_with_version_check_in_tx` directly rather
// than composing `update_node`'s own `_in_tx` pipeline — so it needs its own
// synchronous-dispatch coverage, not just a generic-`update_node` inference.
// Uses the real, seeded built-in "task" schema (no `create_schema` call,
// unlike the generic tests above) since `update_task_node` requires the
// target node's `node_type` to literally be `"task"` (see
// `NodeService::validate_task_status` and `SqliteStore::node_to_task_node`).
// Triggers are scoped to `task.status` — the exact property ADR-060's
// own motivating example turns on ("reject this status change to
// done/in_progress while sub-issues are open") — and augmenting actions
// stamp `priority`, a real built-in task field, rather than an invented one:
// the built-in "task" schema is core-protected (ADR-063), so an action
// targeting it must write a field the schema already declares.
// ---------------------------------------------------------------------------

/// The task-node twin of `stamp_verified_on_update_invariant_rule`: on a
/// `task.status` change, stamps `priority: "high"` on the trigger node
/// itself, inside the same transaction.
fn stamp_priority_on_task_status_update_invariant_rule() -> serde_json::Value {
    json!([{
        "name": "stamp-priority-on-status-update",
        "class": "invariant",
        "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "task", "property_key": "task.status" },
        "conditions": [],
        "actions": [{
            "action_type": "update_node",
            "params": {
                "node_id": "{trigger.node.id}",
                "properties": { "priority": "high" }
            }
        }]
    }])
}

fn new_task_node(content: &str) -> Node {
    Node::new(
        "task".to_string(),
        content.to_string(),
        json!({ "status": "open" }),
    )
}

/// An invariant rule triggered by `update_task_node`'s own `property_changed`
/// write executes synchronously, inside the SAME transaction as the write
/// itself — checked immediately via the RETURN VALUE, no `wait_until`/polling,
/// exactly like `invariant_update_rule_executes_synchronously_in_same_transaction`
/// for the generic path.
#[tokio::test]
async fn invariant_task_update_rule_executes_synchronously_in_same_transaction() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "stamp-priority-on-task-status-play".to_string(),
        json!({ "rules": stamp_priority_on_task_status_update_invariant_rule() }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = new_task_node("Ship the feature");
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    // No wait_until: check the RETURN VALUE of update_task_node itself.
    let updated = service
        .update_task_node(
            &node_id,
            version,
            TaskNodeUpdate::new().with_status(TaskStatus::InProgress),
        )
        .await?;

    assert_eq!(
        updated.priority,
        Some(Priority::High),
        "invariant action must have already run by the time update_task_node returned"
    );
    assert_eq!(
        updated.status,
        TaskStatus::InProgress,
        "the triggering update itself must still have applied"
    );

    Ok(())
}

/// The motivating example from ADR-060 itself: a reject firing on a
/// `task.status` transition to `done` prevents the triggering
/// `update_task_node` call from taking effect at all — the node's status
/// stays at its pre-update value, not a mix of "some fields updated, some
/// not". This is the exact gap this issue closes — before this change, no
/// real daemon call path could ever produce this rejection for a task-node
/// status change.
#[tokio::test]
async fn invariant_task_update_rule_reject_prevents_partial_write() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-task-done-play".to_string(),
        json!({ "rules": [{
            "name": "reject-done-transition",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "task", "property_key": "task.status" },
            "conditions": ["node.status == 'done'"],
            "actions": [{
                "action_type": "reject",
                "params": { "message": "cannot mark done while sub-issues are open" }
            }]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = new_task_node("Ship the feature");
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let before = service.get_node(&node_id).await?.unwrap();

    let err = service
        .update_task_node(
            &node_id,
            before.version,
            TaskNodeUpdate::new().with_status(TaskStatus::Done),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, NodeServiceError::PlayRuleRejected { .. }),
        "expected PlayRuleRejected, got {:?}",
        err
    );

    let after = service.get_node(&node_id).await?.unwrap();
    assert_eq!(
        after.version, before.version,
        "a rejected update must not bump the node's version at all"
    );
    assert_eq!(
        user_field(&after, "task", "status"),
        Some(&json!("open")),
        "a rejected update must leave the property at its pre-update value"
    );

    Ok(())
}

/// No `DomainEvent` is broadcast for a rejected `update_task_node` call — the
/// buffered `NodeUpdated` event `update_task_node_in_tx` emits before
/// dispatch is discarded on rollback, never flushed. Mirrors
/// `invariant_update_rule_reject_emits_no_domain_event` for the generic path.
#[tokio::test]
async fn invariant_task_update_rule_reject_emits_no_domain_event() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "reject-task-no-broadcast-play".to_string(),
        json!({ "rules": [{
            "name": "reject-always",
            "class": "invariant",
            "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "task", "property_key": "task.status" },
            "conditions": [],
            "actions": [{
                "action_type": "reject",
                "params": { "message": "never allowed" }
            }]
        }] }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = new_task_node("Ship the feature");
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    // Subscribe AFTER the create (so its own NodeCreated event isn't sitting
    // in the channel) and BEFORE the rejected update.
    let mut rx = service.subscribe_to_events();

    let result = service
        .update_task_node(
            &node_id,
            version,
            TaskNodeUpdate::new().with_status(TaskStatus::InProgress),
        )
        .await;
    assert!(result.is_err(), "expected the update to be rejected");

    match rx.try_recv() {
        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {}
        other => panic!(
            "expected no event to have been broadcast for a rejected update, got {:?}",
            other
        ),
    }

    Ok(())
}

/// Successful path, the mirror of the rejection test above: an invariant
/// rule's augmenting action commits atomically with the triggering
/// `update_task_node` write AND a real `NodeUpdated` broadcast for the
/// triggering update itself goes out normally, since that write genuinely
/// succeeded.
#[tokio::test]
async fn invariant_task_update_rule_augmenting_action_commits_and_broadcasts_normally() -> Result<()>
{
    let (service, _tmp) = create_test_service().await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "augment-on-task-update-play".to_string(),
        json!({ "rules": stamp_priority_on_task_status_update_invariant_rule() }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = new_task_node("Ship the feature");
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    let mut rx = service.subscribe_to_events();

    let updated = service
        .update_task_node(
            &node_id,
            version,
            TaskNodeUpdate::new().with_status(TaskStatus::InProgress),
        )
        .await?;
    assert_eq!(
        updated.priority,
        Some(Priority::High),
        "augmenting action must have run"
    );

    let envelope = timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("timed out waiting for a broadcast")
        .expect("channel must not have closed");
    match envelope.event {
        DomainEvent::NodeUpdated { node_id: id, .. } => {
            assert_eq!(
                id, node_id,
                "the broadcast must be for the triggering update"
            );
        }
        other => panic!("expected NodeUpdated, got {:?}", other),
    }

    Ok(())
}

/// A `property_changed` invariant rule scoped to `task.status` must not fire
/// when a DIFFERENT task property changes via `update_task_node` — proves
/// `update_task_node_in_tx` reuses the same real exact/wildcard trigger-key
/// matching as the generic path, not a blanket "any update to this node"
/// match. Mirrors
/// `invariant_update_rule_scoped_to_one_property_ignores_a_different_property_change`.
#[tokio::test]
async fn invariant_task_update_rule_scoped_to_one_property_ignores_a_different_property_change(
) -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let engine = PlaybookEngine::new(Arc::clone(&service));
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let play_node = Node::new(
        "play".to_string(),
        "scoped-task-property-play".to_string(),
        // Scoped to "task.status" only.
        json!({ "rules": stamp_priority_on_task_status_update_invariant_rule() }),
    );
    {
        let lifecycle = engine.lifecycle();
        let mut lm = lifecycle.write().unwrap();
        lm.activate_play(&play_node)
            .expect("play must parse and activate");
    }

    let node = new_task_node("Ship the feature");
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    // Change ONLY "due_date" — must not match a rule scoped to "task.status".
    let updated = service
        .update_task_node(
            &node_id,
            version,
            TaskNodeUpdate::new().with_due_date(Some("2026-01-01")),
        )
        .await?;

    assert_eq!(
        updated.priority, None,
        "a property-key-scoped invariant rule must not fire for an unrelated property change"
    );
    assert_eq!(
        updated.due_date.as_deref(),
        Some("2026-01-01"),
        "the unrelated update itself must still have applied"
    );

    Ok(())
}

/// `RuleClass::Reactive` rules triggered by `update_task_node`'s
/// `property_changed` remain completely unaffected by the new synchronous
/// wiring: still async, post-commit, requiring the real engine loop — unlike
/// the invariant tests above, which never spawn one. Mirrors
/// `reactive_update_rule_still_fires_asynchronously_post_commit` for the
/// generic path.
#[tokio::test]
async fn reactive_task_update_rule_still_fires_asynchronously_post_commit() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;
    create_play(
        &service,
        "reactive-on-task-update-play",
        json!([{
            "name": "notify-on-task-status-change",
            // No "class" -> defaults to reactive.
            "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "task", "property_key": "task.status" },
            "conditions": [],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "priority": "high" }
                }
            }]
        }]),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let node = new_task_node("Ship the feature");
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    let updated = service
        .update_task_node(
            &node_id,
            version,
            TaskNodeUpdate::new().with_status(TaskStatus::InProgress),
        )
        .await?;
    // Must NOT be synchronous for a reactive rule.
    assert_eq!(
        updated.priority, None,
        "a reactive rule's effect must not be visible synchronously"
    );

    let fired = wait_until(|| {
        let service = Arc::clone(&service);
        let node_id = node_id.clone();
        async move {
            service
                .get_node(&node_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|n| user_field(&n, "task", "priority") == Some(&json!("high")))
        }
    })
    .await;
    assert!(fired, "reactive rule must eventually fire post-commit");

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Chain-depth provenance (ADR-060 §5)
// ---------------------------------------------------------------------------
//
// A write continues a play chain only when it changed the per-write id every
// play write stamps. A user's edit leaves a play's old stamp in place, so it
// must start a fresh chain instead of continuing from that stale depth.

/// A user's edit to a node a play last stamped at `MAX_CHAIN_DEPTH` fires
/// the node's reactive rule as a fresh chain. Continuing from the stale stamp
/// would read the edit as a cycle, skip the rule and disable the play.
#[tokio::test]
async fn user_edit_to_a_node_stamped_at_max_depth_still_fires_the_reactive_rule() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;
    create_play(
        &service,
        "reactive-on-stamped-task-play",
        json!([{
            "name": "prioritize-on-status-change",
            "trigger": { "type": "graph_event", "on": "property_changed", "node_type": "task", "property_key": "task.status" },
            "conditions": [],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "priority": "high" }
                }
            }]
        }]),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // As a play at the end of a long chain leaves it.
    let node = Node::new(
        "task".to_string(),
        "Stamped by a play".to_string(),
        json!({
            "status": "open",
            (PLAYBOOK_CHAIN_DEPTH_PROPERTY): MAX_CHAIN_DEPTH,
            (PLAYBOOK_WRITE_ID_PROPERTY): "earlier-play-write",
        }),
    );
    let node_id = node.id.clone();
    service.create_node(node).await?;
    let version = service.get_node(&node_id).await?.unwrap().version;

    service
        .update_task_node(
            &node_id,
            version,
            TaskNodeUpdate::new().with_status(TaskStatus::InProgress),
        )
        .await?;

    let fired = wait_until(|| {
        let service = Arc::clone(&service);
        let node_id = node_id.clone();
        async move {
            service
                .get_node(&node_id)
                .await
                .ok()
                .flatten()
                .is_some_and(|n| user_field(&n, "task", "priority") == Some(&json!("high")))
        }
    })
    .await;
    assert!(
        fired,
        "a user's edit must start a fresh chain, not continue from the play's stale stamp"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Sync repair continues the chain a received play write carried, and stops
/// at the limit. A received create carrying only a depth stamp, with no write
/// id, was not written by a play and is repaired as a fresh chain.
#[tokio::test]
async fn sync_repair_continues_only_a_play_written_chain() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_chain_task",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;
    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;
    create_play(
        &service,
        "chain-repair-play",
        stamp_approved_invariant_rule("iv_chain_task"),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let sync_service = service.with_client(REPLICATED_APPLY_CLIENT_ID);

    // A play write at the end of a chain: one more hop would pass the limit.
    let at_limit = Node::new(
        "iv_chain_task".to_string(),
        "play write at the limit".to_string(),
        json!({
            "status": "pending",
            (PLAYBOOK_CHAIN_DEPTH_PROPERTY): MAX_CHAIN_DEPTH,
            (PLAYBOOK_WRITE_ID_PROPERTY): "remote-play-write",
        }),
    );
    let at_limit_id = at_limit.id.clone();
    sync_service.create_node(at_limit).await?;

    // Created after `at_limit`, so once it is repaired the engine has
    // finished with `at_limit` too.
    let depth_only = Node::new(
        "iv_chain_task".to_string(),
        "depth stamp without a write id".to_string(),
        json!({
            "status": "pending",
            (PLAYBOOK_CHAIN_DEPTH_PROPERTY): MAX_CHAIN_DEPTH,
        }),
    );
    let depth_only_id = depth_only.id.clone();
    sync_service.create_node(depth_only).await?;

    let repaired = wait_until(|| {
        let service = Arc::clone(&service);
        let id = depth_only_id.clone();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, "iv_chain_task", "approved") == Some(&json!(true))
            )
        }
    })
    .await;
    assert!(
        repaired,
        "a depth stamp with no write id must not count as a play hop"
    );
    let repaired_node = service.get_node(&depth_only_id).await?.unwrap();
    assert_eq!(
        repaired_node.properties[PLAYBOOK_CHAIN_DEPTH_PROPERTY],
        json!(1),
        "the repair starts a fresh chain"
    );

    let at_limit_node = service.get_node(&at_limit_id).await?.unwrap();
    assert_eq!(
        user_field(&at_limit_node, "iv_chain_task", "approved"),
        None,
        "a repair one hop past the limit of a play-written chain must be skipped"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Repair-and-log for a node UPDATE received via sync (ADR-060 §7)
// ---------------------------------------------------------------------------
//
// A received update can violate a `property_changed` invariant just as a
// received create can violate a `node_created` one: the originating device
// ran an older Play version, had the rule disabled, or predates it. These
// tests drive the real sync tagging (`with_client(REPLICATED_APPLY_CLIENT_ID)`)
// against a running engine, so the repair runs through `handle_event`'s
// sync-originated branch exactly as it does for a real sync apply.
//
// The rules are activated straight into the engine's lifecycle rather than
// saved as Play nodes: the invariant here updates its own trigger node,
// which save-time validation would reject as self-chaining. A received node
// can still arrive in that shape from a device with a different Play, which
// is the case these tests exist for.

/// `property_changed` invariant on `{node_type}.status`: a node whose status
/// is `done` must carry `verified: true`. The condition is written as "the
/// effect is absent", so it stops passing once the repair has applied.
fn verified_when_done_invariant_rule(node_type: &str) -> serde_json::Value {
    json!([{
        "name": "verified-when-done",
        "class": "invariant",
        "trigger": {
            "type": "graph_event",
            "on": "property_changed",
            "node_type": node_type,
            "property_key": format!("{node_type}.status")
        },
        "conditions": ["node.status == 'done' && !has(node.verified)"],
        "actions": [{
            "action_type": "update_node",
            "params": {
                "node_id": "{trigger.node.id}",
                "properties": { "verified": true }
            }
        }]
    }])
}

fn activate_rules_directly(engine: &PlaybookEngine, name: &str, rules: serde_json::Value) {
    let play = Node::new(
        "play".to_string(),
        name.to_string(),
        json!({ "rules": rules }),
    );
    let lifecycle = engine.lifecycle();
    let mut lm = lifecycle.write().unwrap();
    lm.activate_play(&play)
        .expect("play must parse and activate");
}

/// Create `node` on this device as if it had arrived via sync, and return
/// its id and version.
async fn create_via_sync(service: &Arc<NodeService>, node: Node) -> Result<(String, i64)> {
    let id = node.id.clone();
    service
        .with_client(REPLICATED_APPLY_CLIENT_ID)
        .create_node(node)
        .await?;
    let version = service.get_node(&id).await?.unwrap().version;
    Ok((id, version))
}

async fn update_via_sync(
    service: &Arc<NodeService>,
    id: &str,
    properties: serde_json::Value,
) -> Result<Node> {
    let version = service.get_node(id).await?.unwrap().version;
    Ok(service
        .with_client(REPLICATED_APPLY_CLIENT_ID)
        .update_node(id, version, properties_update(properties))
        .await?)
}

async fn setup_verified_when_done(
    node_type: &str,
) -> Result<(
    Arc<NodeService>,
    TempDir,
    watch::Sender<bool>,
    tokio::task::JoinHandle<Result<()>>,
)> {
    let (service, tmp) = create_test_service().await?;
    create_schema(
        &service,
        node_type,
        json!([
            { "name": "status", "type": "string" },
            { "name": "priority", "type": "string" },
            { "name": "verified", "type": "boolean" }
        ]),
    )
    .await?;
    let (engine, shutdown_tx, task) = spawn_engine(&service).await;
    activate_rules_directly(
        &engine,
        "verified-when-done-play",
        verified_when_done_invariant_rule(node_type),
    );
    Ok((service, tmp, shutdown_tx, task))
}

/// Barrier for negative assertions. The engine handles events in order and
/// runs repair inline, so once a violating sentinel written after the events
/// under test has been repaired, every earlier event has been fully handled.
/// Needs the `verified-when-done` rule active for `node_type`.
async fn drain_verified_when_done(service: &Arc<NodeService>, node_type: &str) -> Result<()> {
    let (id, _) = create_via_sync(
        service,
        Node::new(
            node_type.to_string(),
            "sentinel".to_string(),
            json!({ "status": "open" }),
        ),
    )
    .await?;
    update_via_sync(service, &id, json!({ "status": "done" })).await?;
    let drained = wait_until(|| {
        let service = Arc::clone(service);
        let id = id.clone();
        let node_type = node_type.to_string();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, &node_type, "verified") == Some(&json!(true))
            )
        }
    })
    .await;
    assert!(drained, "barrier sentinel was never repaired");
    Ok(())
}

/// A received update that violates a `property_changed` invariant this
/// device holds is repaired, exactly as a received create is.
#[tokio::test]
async fn sync_applied_update_violating_invariant_is_repaired() -> Result<()> {
    let (service, _tmp, shutdown_tx, task) = setup_verified_when_done("iv_su_repair").await?;

    let (id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_repair".to_string(),
            "task".to_string(),
            json!({ "status": "open" }),
        ),
    )
    .await?;
    update_via_sync(&service, &id, json!({ "status": "done" })).await?;

    let repaired = wait_until(|| {
        let service = Arc::clone(&service);
        let id = id.clone();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, "iv_su_repair", "verified") == Some(&json!(true))
            )
        }
    })
    .await;
    assert!(
        repaired,
        "a received update violating a property_changed invariant must be repaired"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Repair is keyed on what the received update changed: an update that
/// doesn't touch the rule's property must not re-check it, even when the
/// node as it stands would fail it.
#[tokio::test]
async fn sync_applied_update_not_touching_the_rule_property_is_not_repaired() -> Result<()> {
    let (service, _tmp, shutdown_tx, task) = setup_verified_when_done("iv_su_other").await?;

    // Arrives already violating (done, not verified). A create does not
    // match a property_changed rule, so nothing repairs it here.
    let (id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_other".to_string(),
            "task".to_string(),
            json!({ "status": "done", "priority": "low" }),
        ),
    )
    .await?;
    let updated = update_via_sync(&service, &id, json!({ "priority": "high" })).await?;

    drain_verified_when_done(&service, "iv_su_other").await?;

    let node = service.get_node(&id).await?.unwrap();
    assert_eq!(
        user_field(&node, "iv_su_other", "verified"),
        None,
        "a received update that changes only an unrelated property must not trigger repair"
    );
    assert_eq!(
        node.version, updated.version,
        "no repair write may follow the received update"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// A received update that touches the rule's property but leaves the
/// invariant satisfied is not repaired.
#[tokio::test]
async fn sync_applied_non_violating_update_is_not_repaired() -> Result<()> {
    let (service, _tmp, shutdown_tx, task) = setup_verified_when_done("iv_su_ok").await?;

    let (id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_ok".to_string(),
            "task".to_string(),
            json!({ "status": "open" }),
        ),
    )
    .await?;
    // Status changes, but not to `done`: the condition fails.
    let first = update_via_sync(&service, &id, json!({ "status": "in_progress" })).await?;
    // Status changes to `done`, and the originating device already applied
    // the effect in the same write.
    let second =
        update_via_sync(&service, &id, json!({ "status": "done", "verified": true })).await?;
    assert!(second.version > first.version);

    drain_verified_when_done(&service, "iv_su_ok").await?;

    let node = service.get_node(&id).await?.unwrap();
    assert_eq!(
        node.version, second.version,
        "an update that leaves the invariant satisfied must not be followed by a repair write"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// A `node_created` invariant is not re-checked by a received update, and
/// the create path still repairs a received create with the update path
/// wired in alongside it.
#[tokio::test]
async fn sync_applied_update_does_not_run_a_node_created_invariant() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_su_create_only",
        json!([
            { "name": "status", "type": "string" },
            { "name": "approved", "type": "boolean" }
        ]),
    )
    .await?;
    let (engine, shutdown_tx, task) = spawn_engine(&service).await;
    activate_rules_directly(
        &engine,
        "create-only-play",
        stamp_approved_invariant_rule("iv_su_create_only"),
    );

    // Condition is `node.status == 'pending'`; created as `open`, so the
    // create itself is compliant.
    let (id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_create_only".to_string(),
            "task".to_string(),
            json!({ "status": "open" }),
        ),
    )
    .await?;
    // Let the engine finish with the create first: create-path repair reads
    // the node as it stands when the event is handled, so an update landing
    // before then would be judged as part of the create. A later violating
    // create being repaired proves the earlier one was handled.
    let (barrier_id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_create_only".to_string(),
            "barrier".to_string(),
            json!({ "status": "pending" }),
        ),
    )
    .await?;
    assert!(
        wait_until(|| {
            let service = Arc::clone(&service);
            let id = barrier_id.clone();
            async move {
                matches!(
                    service.get_node(&id).await,
                    Ok(Some(n)) if user_field(&n, "iv_su_create_only", "approved") == Some(&json!(true))
                )
            }
        })
        .await
    );
    let updated = update_via_sync(&service, &id, json!({ "status": "pending" })).await?;

    // Create path, unchanged: a violating received create is still repaired.
    let (created_id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_create_only".to_string(),
            "task".to_string(),
            json!({ "status": "pending" }),
        ),
    )
    .await?;
    let create_repaired = wait_until(|| {
        let service = Arc::clone(&service);
        let id = created_id.clone();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, "iv_su_create_only", "approved") == Some(&json!(true))
            )
        }
    })
    .await;
    assert!(
        create_repaired,
        "a violating received create must still be repaired"
    );

    let node = service.get_node(&id).await?.unwrap();
    assert_eq!(
        user_field(&node, "iv_su_create_only", "approved"),
        None,
        "a node_created invariant must not be re-checked by a received update"
    );
    assert_eq!(node.version, updated.version);

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Loop safety on one device: the repair write is local-origin, so it never
/// re-enters the sync-only repair branch. Exactly one repair write follows
/// the violating update, stamped one hop past the received write's depth.
#[tokio::test]
async fn sync_update_repair_writes_once_one_hop_past_the_received_depth() -> Result<()> {
    let (service, _tmp, shutdown_tx, task) = setup_verified_when_done("iv_su_once").await?;

    let (id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_once".to_string(),
            "task".to_string(),
            json!({ "status": "open" }),
        ),
    )
    .await?;
    let received = update_via_sync(&service, &id, json!({ "status": "done" })).await?;

    drain_verified_when_done(&service, "iv_su_once").await?;
    let after_repair = service.get_node(&id).await?.unwrap();
    assert_eq!(
        user_field(&after_repair, "iv_su_once", "verified"),
        Some(&json!(true))
    );
    assert_eq!(
        after_repair.version,
        received.version + 1,
        "exactly one repair write must follow the violating update"
    );
    assert_eq!(
        after_repair.properties.get(PLAYBOOK_CHAIN_DEPTH_PROPERTY),
        Some(&json!(1)),
        "the repair write carries its chain depth, one hop past the received write"
    );
    assert!(
        received
            .properties
            .get(PLAYBOOK_WRITE_ID_PROPERTY)
            .is_none(),
        "the received user edit carries no write id"
    );
    assert!(
        after_repair
            .properties
            .get(PLAYBOOK_WRITE_ID_PROPERTY)
            .is_some_and(|id| id.is_string()),
        "the repair write stamps a write id, marking it as a play write for the next device"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// A play's stamp at the limit left on a node does not suppress repairing
/// a later violation that a user's edit introduced: the edit doesn't change
/// the write id, so it starts a fresh chain.
#[tokio::test]
async fn stale_play_stamp_does_not_suppress_repair_of_a_user_edit() -> Result<()> {
    let (service, _tmp, shutdown_tx, task) = setup_verified_when_done("iv_su_stale").await?;

    let (id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_stale".to_string(),
            "task".to_string(),
            json!({
                "status": "open",
                (PLAYBOOK_CHAIN_DEPTH_PROPERTY): MAX_CHAIN_DEPTH,
                (PLAYBOOK_WRITE_ID_PROPERTY): "earlier-chain"
            }),
        ),
    )
    .await?;
    // A user's edit on another device: changes status, leaves the stamps.
    update_via_sync(&service, &id, json!({ "status": "done" })).await?;

    drain_verified_when_done(&service, "iv_su_stale").await?;
    let node = service.get_node(&id).await?.unwrap();
    assert_eq!(
        user_field(&node, "iv_su_stale", "verified"),
        Some(&json!(true)),
        "a stale play stamp must not block repairing a violation a user edit introduced"
    );
    assert_eq!(
        node.properties.get(PLAYBOOK_CHAIN_DEPTH_PROPERTY),
        Some(&json!(1)),
        "the repair starts a fresh chain"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// The same repair with a rule shape save-time validation accepts: a saved
/// Play whose `property_changed` invariant creates a node of another type
/// (it does not update its own trigger node, so it doesn't self-chain).
#[tokio::test]
async fn sync_applied_update_is_repaired_by_a_saved_non_self_chaining_invariant() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;
    create_schema(
        &service,
        "iv_su_legal",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;
    create_schema(
        &service,
        "iv_su_legal_log",
        json!([{ "name": "note", "type": "string" }]),
    )
    .await?;
    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;
    create_play(
        &service,
        "legal-shape-play",
        json!([{
            "name": "log-on-done",
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "node_type": "iv_su_legal",
                "property_key": "iv_su_legal.status"
            },
            "conditions": ["node.status == 'done'"],
            "actions": [{
                "action_type": "create_node",
                "params": { "node_type": "iv_su_legal_log", "content": "done recorded" }
            }]
        }]),
    )
    .await?;
    // The play is saved through the ordinary write path, so the engine
    // validates it before activating it: a play that failed validation would
    // be disabled and could not repair anything below. The engine handles
    // the play's create event before the sync writes that follow it.

    let (id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_legal".to_string(),
            "task".to_string(),
            json!({ "status": "open" }),
        ),
    )
    .await?;
    update_via_sync(&service, &id, json!({ "status": "done" })).await?;

    let repaired = wait_until(|| {
        let service = Arc::clone(&service);
        async move {
            matches!(
                service.query_nodes_by_type("iv_su_legal_log", None).await,
                Ok(nodes) if nodes.len() == 1
            )
        }
    })
    .await;
    assert!(
        repaired,
        "a received update violating a saved, validated invariant must be repaired"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// A received play write at the chain-depth limit is not repaired: the next
/// hop would exceed `MAX_CHAIN_DEPTH`. The write changes the per-write id,
/// which is what marks it as a play hop rather than a user's edit.
#[tokio::test]
async fn sync_update_at_the_chain_depth_limit_is_not_repaired() -> Result<()> {
    let (service, _tmp, shutdown_tx, task) = setup_verified_when_done("iv_su_limit").await?;

    let (id, _) = create_via_sync(
        &service,
        Node::new(
            "iv_su_limit".to_string(),
            "task".to_string(),
            json!({ "status": "open" }),
        ),
    )
    .await?;
    let received = update_via_sync(
        &service,
        &id,
        json!({
            "status": "done",
            (PLAYBOOK_CHAIN_DEPTH_PROPERTY): MAX_CHAIN_DEPTH,
            (PLAYBOOK_WRITE_ID_PROPERTY): "remote-play-write"
        }),
    )
    .await?;

    drain_verified_when_done(&service, "iv_su_limit").await?;
    let node = service.get_node(&id).await?.unwrap();
    assert_eq!(user_field(&node, "iv_su_limit", "verified"), None);
    assert_eq!(node.version, received.version);

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Loop safety across devices: two devices hold contradictory invariants on
/// the same property (`mode` must be `a` on one, `b` on the other: the
/// version-skew case ADR-060 §7 describes). Each device repairs what the
/// other sends, and a relay carries every local-origin write to the other
/// device as a sync apply, the way real sync does. Without a bound this
/// ping-pongs forever. The persisted chain depth carries the hop count
/// across devices, so the exchange stops at `MAX_CHAIN_DEPTH`.
#[tokio::test]
async fn contradictory_invariants_on_two_devices_stop_at_the_chain_depth_limit() -> Result<()> {
    contradictory_invariants_ping_pong(json!({})).await
}

/// The same exchange on a node that already carries a play's stamp (depth 1
/// and a write id) from an earlier chain, started by a write that doesn't
/// touch the stamp (a user's edit). The first repair restarts the count and
/// writes depth 1, the depth the node already carries. The chain must still
/// be seen as continuing on the next device: that write changed the write
/// id even though the depth is unchanged. Restarting whenever the depth
/// stamp was unchanged would let the devices restart on every hop, forever.
#[tokio::test]
async fn contradictory_invariants_stop_at_the_limit_on_an_already_stamped_node() -> Result<()> {
    contradictory_invariants_ping_pong(json!({
        (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 1,
        (PLAYBOOK_WRITE_ID_PROPERTY): "earlier-chain"
    }))
    .await
}

async fn contradictory_invariants_ping_pong(initial_properties: serde_json::Value) -> Result<()> {
    const NODE_TYPE: &str = "iv_su_pingpong";
    fn mode_must_be(required: &str) -> serde_json::Value {
        json!([{
            "name": format!("mode-must-be-{required}"),
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "node_type": NODE_TYPE,
                "property_key": format!("{NODE_TYPE}.mode")
            },
            "conditions": [format!("node.mode != '{required}'")],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "mode": required }
                }
            }]
        }])
    }

    async fn device(
        required: &str,
    ) -> Result<(
        Arc<NodeService>,
        TempDir,
        watch::Sender<bool>,
        tokio::task::JoinHandle<Result<()>>,
    )> {
        let (service, tmp) = create_test_service().await?;
        create_schema(
            &service,
            NODE_TYPE,
            json!([{ "name": "mode", "type": "string" }]),
        )
        .await?;
        let (engine, shutdown_tx, task) = spawn_engine(&service).await;
        activate_rules_directly(
            &engine,
            &format!("device-{required}"),
            mode_must_be(required),
        );
        Ok((service, tmp, shutdown_tx, task))
    }

    let (device_a, _tmp_a, shutdown_a, task_a) = device("a").await?;
    let (device_b, _tmp_b, shutdown_b, task_b) = device("b").await?;

    let node = Node::new(
        NODE_TYPE.to_string(),
        "shared".to_string(),
        initial_properties,
    );
    let id = node.id.clone();
    create_via_sync(&device_a, node.clone()).await?;
    create_via_sync(&device_b, node).await?;

    // Relay each device's local-origin writes to the other as sync applies.
    fn relay(
        from: &Arc<NodeService>,
        to: &Arc<NodeService>,
        id: &str,
    ) -> tokio::task::JoinHandle<()> {
        let mut events = from.subscribe_to_events();
        let (to, id) = (Arc::clone(to), id.to_string());
        tokio::spawn(async move {
            while let Ok(envelope) = events.recv().await {
                if envelope.metadata.source_client_id.as_deref() == Some(REPLICATED_APPLY_CLIENT_ID)
                {
                    continue;
                }
                let DomainEvent::NodeUpdated { node_id, node, .. } = envelope.event else {
                    continue;
                };
                if node_id != id {
                    continue;
                }
                let mut properties = json!({
                    "mode": node.properties[NODE_TYPE]["mode"].clone(),
                });
                // Sync carries the whole node, bookkeeping stamps included.
                for key in [PLAYBOOK_CHAIN_DEPTH_PROPERTY, PLAYBOOK_WRITE_ID_PROPERTY] {
                    if let Some(value) = node.properties.get(key) {
                        properties[key] = value.clone();
                    }
                }
                let _ = update_via_sync(&to, &id, properties).await;
            }
        })
    }
    let relay_ab = relay(&device_a, &device_b, &id);
    let relay_ba = relay(&device_b, &device_a, &id);

    // A write from a device holding neither rule arrives on device A.
    update_via_sync(&device_a, &id, json!({ "mode": "x" })).await?;

    // Quiescent: neither device's version moves for a sustained window.
    let mut last = (0, 0);
    let mut stable_polls = 0;
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = (
            device_a.get_node(&id).await?.unwrap().version,
            device_b.get_node(&id).await?.unwrap().version,
        );
        stable_polls = if now == last { stable_polls + 1 } else { 0 };
        last = now;
        if stable_polls >= 20 {
            break;
        }
    }
    assert!(
        stable_polls >= 20,
        "contradictory invariants on two devices must not ping-pong forever (versions still moving: {last:?})"
    );

    let depth_on = |n: &Node| n.properties.get(PLAYBOOK_CHAIN_DEPTH_PROPERTY).cloned();
    let a = device_a.get_node(&id).await?.unwrap();
    let b = device_b.get_node(&id).await?.unwrap();
    assert_eq!(
        depth_on(&a),
        Some(json!(MAX_CHAIN_DEPTH)),
        "the exchange must run to the depth limit and stop there, not stop early"
    );
    assert_eq!(depth_on(&b), Some(json!(MAX_CHAIN_DEPTH)));

    relay_ab.abort();
    relay_ba.abort();
    shutdown_engine(shutdown_a, task_a).await;
    shutdown_engine(shutdown_b, task_b).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Chains through in-transaction invariant hops
// ---------------------------------------------------------------------------
//
// A sync repair's write is local, so it runs this device's pre-commit
// invariant rules on the fields it changed. Those in-transaction hops must
// continue the repair's chain depth; restarting at 0 would let a cycle that
// passes through them run forever across devices.

/// Toggle-style rules: when `{from}.f` changes, copy its `on`/`off` value
/// (inverted when `invert`) into `f` on the node `target_id`.
fn relay_f_rules(from: &str, target_id: &str, invert: bool) -> serde_json::Value {
    let rule = |when: &str, set: &str| {
        json!({
            "name": format!("{from}-{when}-sets-{set}"),
            "class": "invariant",
            "trigger": {
                "type": "graph_event",
                "on": "property_changed",
                "node_type": from,
                "property_key": format!("{from}.f")
            },
            "conditions": [format!("node.f == '{when}'")],
            "actions": [{
                "action_type": "update_node",
                "params": { "node_id": target_id, "properties": { "f": set } }
            }]
        })
    };
    let (on, off) = if invert { ("off", "on") } else { ("on", "off") };
    json!([rule("on", on), rule("off", off)])
}

async fn chain_device(
    rules: &[(&str, serde_json::Value)],
) -> Result<(
    Arc<NodeService>,
    TempDir,
    watch::Sender<bool>,
    tokio::task::JoinHandle<Result<()>>,
)> {
    let (service, tmp) = create_test_service().await?;
    for node_type in ["rv_x", "rv_y", "rv_z"] {
        create_schema(
            &service,
            node_type,
            json!([{ "name": "f", "type": "string" }]),
        )
        .await?;
    }
    let (engine, shutdown_tx, task) = spawn_engine(&service).await;
    for (name, play_rules) in rules {
        activate_rules_directly(&engine, name, play_rules.clone());
    }
    for (id, node_type) in [("rv-x", "rv_x"), ("rv-y", "rv_y"), ("rv-z", "rv_z")] {
        create_via_sync(
            &service,
            Node::new_with_id(
                id.to_string(),
                node_type.to_string(),
                id.to_string(),
                json!({ "f": "none" }),
            ),
        )
        .await?;
    }
    Ok((service, tmp, shutdown_tx, task))
}

fn depth_of(node: &Node) -> Option<serde_json::Value> {
    node.properties.get(PLAYBOOK_CHAIN_DEPTH_PROPERTY).cloned()
}

/// An invariant the repair's own write triggers in-transaction writes one
/// hop further: a received play write at depth 5 on X is repaired onto Y at
/// depth 6, and the invariant Y's change triggers writes Z at depth 7.
#[tokio::test]
async fn in_transaction_invariant_hop_continues_the_repair_chain_depth() -> Result<()> {
    let (service, _tmp, shutdown_tx, task) = chain_device(&[
        ("x-to-y", relay_f_rules("rv_x", "rv-y", false)),
        ("y-to-z", relay_f_rules("rv_y", "rv-z", false)),
    ])
    .await?;

    update_via_sync(
        &service,
        "rv-x",
        json!({
            "f": "on",
            (PLAYBOOK_CHAIN_DEPTH_PROPERTY): 5,
            (PLAYBOOK_WRITE_ID_PROPERTY): "remote-play-write"
        }),
    )
    .await?;

    let reached_z = wait_until(|| {
        let service = Arc::clone(&service);
        async move {
            matches!(
                service.get_node("rv-z").await,
                Ok(Some(n)) if user_field(&n, "rv_z", "f") == Some(&json!("on"))
            )
        }
    })
    .await;
    assert!(
        reached_z,
        "the repair and the invariant it triggers must both run"
    );

    let y = service.get_node("rv-y").await?.unwrap();
    let z = service.get_node("rv-z").await?.unwrap();
    assert_eq!(
        depth_of(&y),
        Some(json!(6)),
        "the repair runs one hop past the received write"
    );
    assert_eq!(
        depth_of(&z),
        Some(json!(7)),
        "an in-transaction invariant hop continues the repair's chain, not restarts it"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Two devices with a cycle that passes through an in-transaction hop:
/// device A holds X→Y and Y→Z, device B holds Z→X inverted, so every pass
/// flips X and the cycle never settles on its own. A relay carries each
/// device's local writes to the other as sync applies. The chain depth must
/// rise through every hop, including the in-transaction Y→Z hop, so the
/// exchange stops at `MAX_CHAIN_DEPTH`.
#[tokio::test]
async fn cycle_through_an_in_transaction_invariant_hop_stops_at_the_limit() -> Result<()> {
    let (device_a, _tmp_a, shutdown_a, task_a) = chain_device(&[
        ("x-to-y", relay_f_rules("rv_x", "rv-y", false)),
        ("y-to-z", relay_f_rules("rv_y", "rv-z", false)),
    ])
    .await?;
    let (device_b, _tmp_b, shutdown_b, task_b) =
        chain_device(&[("z-to-x", relay_f_rules("rv_z", "rv-x", true))]).await?;

    fn relay(from: &Arc<NodeService>, to: &Arc<NodeService>) -> tokio::task::JoinHandle<()> {
        let mut events = from.subscribe_to_events();
        let to = Arc::clone(to);
        tokio::spawn(async move {
            while let Ok(envelope) = events.recv().await {
                if envelope.metadata.source_client_id.as_deref() == Some(REPLICATED_APPLY_CLIENT_ID)
                {
                    continue;
                }
                let DomainEvent::NodeUpdated {
                    node_id,
                    node_type,
                    node,
                    ..
                } = envelope.event
                else {
                    continue;
                };
                if !node_id.starts_with("rv-") {
                    continue;
                }
                // Sync carries the whole node, bookkeeping stamps included.
                let mut properties = json!({ "f": node.properties[&node_type]["f"].clone() });
                for key in [PLAYBOOK_CHAIN_DEPTH_PROPERTY, PLAYBOOK_WRITE_ID_PROPERTY] {
                    if let Some(value) = node.properties.get(key) {
                        properties[key] = value.clone();
                    }
                }
                let _ = update_via_sync(&to, &node_id, properties).await;
            }
        })
    }
    let relay_ab = relay(&device_a, &device_b);
    let relay_ba = relay(&device_b, &device_a);

    // A user's edit on some third device arrives at A.
    update_via_sync(&device_a, "rv-x", json!({ "f": "on" })).await?;

    let versions = |a: &Arc<NodeService>, b: &Arc<NodeService>| {
        let (a, b) = (Arc::clone(a), Arc::clone(b));
        async move {
            let mut v = Vec::new();
            for service in [&a, &b] {
                for id in ["rv-x", "rv-y", "rv-z"] {
                    v.push(service.get_node(id).await?.unwrap().version);
                }
            }
            anyhow::Ok(v)
        }
    };
    let mut last = Vec::new();
    let mut stable_polls = 0;
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = versions(&device_a, &device_b).await?;
        stable_polls = if now == last { stable_polls + 1 } else { 0 };
        last = now;
        if stable_polls >= 20 {
            break;
        }
    }
    assert!(
        stable_polls >= 20,
        "a cross-device cycle through an in-transaction invariant hop must terminate \
         (versions still moving: {last:?})"
    );

    let z_on_b = device_b.get_node("rv-z").await?.unwrap();
    assert_eq!(
        depth_of(&z_on_b),
        Some(json!(MAX_CHAIN_DEPTH)),
        "the exchange must run to the depth limit and stop there"
    );

    relay_ab.abort();
    relay_ba.abort();
    shutdown_engine(shutdown_a, task_a).await;
    shutdown_engine(shutdown_b, task_b).await;
    Ok(())
}
