//! Integration tests for `PlaybookEngine::start()`.
//!
//! `PlaybookEngine::start()` is instantiated at daemon startup
//! (`packages/daemon/src/services/assembly.rs`) but had no test coverage
//! driving it as a running engine: the engine, `RuleProcessor`, and
//! `CronRunner` are exercised here as they actually run in production —
//! subscribed to a real `NodeService` broadcast channel, against a real
//! SQLite-backed store — rather than through the lifecycle/queue unit tests
//! elsewhere in `packages/core/src/playbook/`, which construct
//! `ExecutionWorkItem`s and `TriggerIndex` state directly without a running
//! engine loop.
//!
//! Covers:
//! - A play fires end-to-end for a local mutation (trigger → condition → action).
//! - An event tagged as sync-originated (the existing `source_client_id`
//!   convention, ADR-027) does NOT reach trigger evaluation (ADR-073).
//! - `CronRunner`'s 60-second poll loop is actually spawned by `start()` and
//!   ticks on schedule.
//!
//! Per ADR-073's Consequences section: this gate makes reactive rules
//! local-device-only until ADR-060 lands, and does NOT make
//! `RuleClass::Invariant` rules sync-safe — their fail-closed guarantee still
//! depends on unbuilt ADR-060 §2/§7 mechanisms. Every rule below is the
//! default `RuleClass::Reactive`; this suite does not exercise invariant
//! rules or claim anything about their sync safety.

use anyhow::Result;
use nodespace_core::db::events::SYNC_SERVICE_CLIENT_ID;
use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;
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

/// Create a minimal schema node for a user-defined node type, with the given
/// fields (each `{"name": ..., "type": ...}`). No relationships.
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

/// Create and persist a play node with the given rules JSON (the same wire
/// format `validate_play_rules`/`PlaybookLifecycleManager::activate_play`
/// parse — see `packages/core/src/playbook/types.rs::parse_rules_from_properties`).
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

/// Read a user-defined-type field from a `Node`'s stored properties.
///
/// User-defined node types store their fields nested under
/// `properties[node_type][field_name]` (see
/// `NodeService::normalize_flat_properties_to_namespace`), NOT as bare
/// top-level keys — the CEL evaluator's graph resolver flattens this back out
/// for `node.<field>` access inside conditions/actions, but reading a
/// `Node`'s raw `properties` (as these tests do, to verify what a play action
/// actually wrote) must go through the same nesting the store uses.
fn user_field<'a>(node: &'a Node, node_type: &str, field: &str) -> Option<&'a serde_json::Value> {
    node.properties.get(node_type).and_then(|p| p.get(field))
}

/// Poll `check` every 25ms (real time) up to `~2s` total, returning true as
/// soon as it returns true. Used instead of a single fixed sleep so the
/// happy path is fast and a slow CI runner still gets a fair chance.
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
    // Wire the write path to the same live TriggerIndex the engine just
    // built (ADR-060 §1), exactly as `assembly.rs` does at daemon startup —
    // required for `RuleClass::Invariant` dispatch; a no-op for every
    // existing reactive-only test in this file (the lookup it performs
    // inside `create_node_in_tx` only ever matches an `Invariant`-class
    // rule, of which none of those tests register any).
    service.set_playbook_lifecycle(engine.lifecycle().clone());
    let task = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.start(shutdown_rx).await })
    };
    // Let the engine finish subscribing (step 1 of `start()`, which happens
    // before `load_active_plays()`) before the caller starts creating nodes —
    // not required for correctness (the broadcast channel buffers events
    // once subscribed), but keeps the tests' timing intuitive.
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

/// Acceptance criterion: "A play fires end-to-end for a local mutation
/// (integration test covering trigger → condition → action)."
///
/// Exercises the full reactive pipeline against a REAL running engine:
/// - Play installed reactively (its own `NodeCreated` event, `node_type ==
///   "play"`, handled by `PlaybookEngine::handle_play_created`) rather than
///   pre-loaded at startup — this is the path a user authoring a play through
///   the running daemon actually takes.
/// - A local mutation (`node_created` on the trigger's `node_type`) is
///   matched by `TriggerKey` lookup, its CEL condition evaluated against the
///   real node, and — only when the condition passes — its action executed
///   against the real store via `NodeService`.
/// - A second node whose condition evaluates false is left untouched,
///   proving the condition genuinely gates the action rather than every
///   trigger match firing unconditionally.
#[tokio::test]
async fn play_fires_end_to_end_for_local_mutation() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    create_schema(
        &service,
        "pb_task",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    // Install the play reactively while the engine is already running.
    create_play(
        &service,
        "close-open-tasks",
        json!([{
            "name": "auto-close",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "pb_task" },
            "conditions": ["node.status == 'open'"],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "status": "done" }
                }
            }]
        }]),
    )
    .await?;

    // Give the reactive install a moment to land in the TriggerIndex before
    // the triggering mutation below — otherwise this test would depend on
    // event-ordering luck rather than deterministic install-then-trigger.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Local mutation whose condition passes — should fire the action.
    let matching = Node::new(
        "pb_task".to_string(),
        "task that should auto-close".to_string(),
        json!({ "status": "open" }),
    );
    let matching_id = matching.id.clone();
    service.create_node(matching).await?;

    // Local mutation whose condition fails — action must NOT fire.
    let non_matching = Node::new(
        "pb_task".to_string(),
        "task that should stay untouched".to_string(),
        json!({ "status": "closed" }),
    );
    let non_matching_id = non_matching.id.clone();
    service.create_node(non_matching).await?;

    let fired = wait_until(|| {
        let service = Arc::clone(&service);
        let id = matching_id.clone();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, "pb_task", "status").and_then(|v| v.as_str()) == Some("done")
            )
        }
    })
    .await;
    assert!(
        fired,
        "play action must have set status='done' on the matching node \
         (trigger → condition → action end-to-end)"
    );

    // The condition-failing node must be left exactly as created.
    let untouched = service
        .get_node(&non_matching_id)
        .await?
        .expect("non-matching node must still exist");
    assert_eq!(
        user_field(&untouched, "pb_task", "status").and_then(|v| v.as_str()),
        Some("closed"),
        "a node whose condition evaluates false must not be touched by the action"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Acceptance criterion: "An event tagged as sync-originated (via the
/// existing `source_client_id` convention) verifiably does NOT reach trigger
/// evaluation (integration test)."
///
/// This is the safety-critical half of ADR-073: a node created through
/// `NodeService::with_client(SYNC_SERVICE_CLIENT_ID)` — the exact tagging
/// convention ADR-027 already establishes for the local-first sync service —
/// must be excluded from trigger evaluation before it ever reaches
/// `TriggerKey` lookup. If the gate in
/// `packages/core/src/playbook/engine.rs::is_sync_originated` were absent or
/// wrong, this node would be updated exactly like the local-mutation test
/// above.
#[tokio::test]
async fn sync_originated_event_does_not_reach_trigger_evaluation() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    create_schema(
        &service,
        "pb_sync_task",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    create_play(
        &service,
        "close-open-sync-tasks",
        json!([{
            "name": "auto-close-sync",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "pb_sync_task" },
            "conditions": ["node.status == 'open'"],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "status": "done" }
                }
            }]
        }]),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Simulate a sync-applied write: the exact tagging convention ADR-027
    // establishes for the local-first sync service (`with_client`, read by
    // `EventMetadata::source_client_id` on the resulting `NodeCreated`
    // event). Condition would pass (status == "open") if evaluated at all.
    let sync_service = service.with_client(SYNC_SERVICE_CLIENT_ID);
    let synced_node = Node::new(
        "pb_sync_task".to_string(),
        "task applied via sync".to_string(),
        json!({ "status": "open" }),
    );
    let synced_id = synced_node.id.clone();
    sync_service.create_node(synced_node).await?;

    // Also create a genuinely local node with the identical shape, on the
    // SAME running engine, to prove the engine is alive and would have fired
    // for this exact play/condition/action if origin hadn't gated it out —
    // ruling out "the play never activated" as a false-negative explanation
    // for the sync node staying untouched.
    let control = Node::new(
        "pb_sync_task".to_string(),
        "control: local task, same shape".to_string(),
        json!({ "status": "open" }),
    );
    let control_id = control.id.clone();
    service.create_node(control).await?;

    let control_fired = wait_until(|| {
        let service = Arc::clone(&service);
        let id = control_id.clone();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, "pb_sync_task", "status").and_then(|v| v.as_str()) == Some("done")
            )
        }
    })
    .await;
    assert!(
        control_fired,
        "control: a LOCAL node with the identical trigger/condition/action shape \
         must fire — otherwise a non-firing sync node proves nothing about the gate"
    );

    // The sync-tagged node must remain untouched — its NodeCreated event
    // never reached trigger evaluation at all.
    let synced_after = service
        .get_node(&synced_id)
        .await?
        .expect("sync-tagged node must still exist (it was created, just not acted on)");
    assert_eq!(
        user_field(&synced_after, "pb_sync_task", "status").and_then(|v| v.as_str()),
        Some("open"),
        "a sync-originated NodeCreated event must be excluded from trigger evaluation \
         before rule matching (ADR-073) — this node must be left exactly as created"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Regression test: the real sync-apply shape uses `NodeService::bulk_create`
/// (`nodespaced-pro`'s catch-up/reconnect path batches pulled pages through
/// `bulk_create`, not one `create_node` per row), not the single-row
/// `create_node` the test above exercises. `SqliteStore::batch_create_nodes`
/// previously hardcoded `source: None` for every node in the batch,
/// discarding whatever client_id the calling `NodeService` was tagged with —
/// so a sync-tagged `bulk_create` call emitted events with
/// `source_client_id: None`, which `is_sync_originated` (correctly) does NOT
/// treat as sync-originated, and the ADR-073 gate let it straight through.
/// This is exactly the "catch-up replay re-firing history" failure mode
/// ADR-073 exists to prevent, for every device reconnecting with a backlog
/// of a teammate's new nodes. Covers the same shape as
/// `sync_originated_event_does_not_reach_trigger_evaluation` above, but
/// through `bulk_create` instead of `create_node`.
#[tokio::test]
async fn sync_originated_bulk_create_does_not_reach_trigger_evaluation() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    create_schema(
        &service,
        "pb_bulk_sync_task",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    create_play(
        &service,
        "close-open-bulk-sync-tasks",
        json!([{
            "name": "auto-close-bulk-sync",
            "trigger": { "type": "graph_event", "on": "node_created", "node_type": "pb_bulk_sync_task" },
            "conditions": ["node.status == 'open'"],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "status": "done" }
                }
            }]
        }]),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Simulate the real sync-apply shape: a sync-tagged NodeService batching
    // pulled rows through `bulk_create` (as `nodespaced-pro`'s catch-up path
    // does via `apply_node_upserts_batched`), not one `create_node` call per
    // row.
    let sync_service = service.with_client(SYNC_SERVICE_CLIENT_ID);
    let synced_node = Node::new(
        "pb_bulk_sync_task".to_string(),
        "task applied via sync bulk_create".to_string(),
        json!({ "status": "open" }),
    );
    let synced_id = synced_node.id.clone();
    sync_service.bulk_create(vec![synced_node]).await?;

    // Control: a genuinely local `bulk_create` call with the identical
    // shape, on the SAME running engine, proving the engine is alive and
    // would have fired for this exact play/condition/action if origin
    // hadn't gated it out.
    let control = Node::new(
        "pb_bulk_sync_task".to_string(),
        "control: local bulk_create, same shape".to_string(),
        json!({ "status": "open" }),
    );
    let control_id = control.id.clone();
    service.bulk_create(vec![control]).await?;

    let control_fired = wait_until(|| {
        let service = Arc::clone(&service);
        let id = control_id.clone();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, "pb_bulk_sync_task", "status").and_then(|v| v.as_str()) == Some("done")
            )
        }
    })
    .await;
    assert!(
        control_fired,
        "control: a LOCAL bulk_create call with the identical trigger/condition/action \
         shape must fire — otherwise a non-firing sync bulk_create node proves nothing \
         about the gate"
    );

    let synced_after = service
        .get_node(&synced_id)
        .await?
        .expect("sync-tagged bulk-created node must still exist (created, just not acted on)");

    // The sync-tagged bulk-created node must remain untouched — its
    // NodeCreated event must carry source_client_id = Some(SYNC_SERVICE_CLIENT_ID)
    // (not None) and never reach trigger evaluation at all.
    //
    // Unlike `create_node`, `NodeService::bulk_create` does not run node
    // properties through `normalize_flat_properties_to_namespace` before
    // insert (a separate, pre-existing inconsistency, out of scope for this
    // fix) — a node it creates stores properties flat at the top level
    // (`properties.status`), not nested under the node_type key the way
    // `create_node`/`update_node` do. If the update action HAD fired, it
    // would have deep-merged a namespaced `{"pb_bulk_sync_task": {"status":
    // "done"}}` alongside that flat shape (as the control assertion above
    // relies on) — so asserting its absence, together with the untouched
    // flat property, is the correct and shape-agnostic way to prove this
    // node was never acted on.
    assert_eq!(
        user_field(&synced_after, "pb_bulk_sync_task", "status").and_then(|v| v.as_str()),
        None,
        "the update action's namespaced 'done' marker must be absent — the sync-tagged \
         bulk_create event must never have reached trigger evaluation (ADR-073)"
    );
    assert_eq!(
        synced_after
            .properties
            .get("status")
            .and_then(|v| v.as_str()),
        Some("open"),
        "a sync-originated bulk_create NodeCreated event must be excluded from trigger \
         evaluation before rule matching (ADR-073) — this node must be left exactly as created"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Acceptance criterion: "CronRunner's poll loop verifiably runs on a
/// schedule (test or diagnostic confirms it is spawned and ticking)."
///
/// `PlaybookEngine::start()` spawns `cron_runner::cron_runner_loop` as a real
/// tokio task (see `engine.rs` step 5). That loop wakes every 60 real
/// seconds; this test uses a paused virtual clock
/// (`#[tokio::test(start_paused = true)]` + `tokio::time::advance`) to prove
/// it actually ticks and fires a scheduled play's action, without a real
/// 60-second wait. Only the timer is virtual — the schema/play/node setup and
/// the tick's own DB scan and action execution are real, unmocked
/// `NodeService`/SQLite calls.
#[tokio::test(start_paused = true)]
async fn cron_runner_ticks_and_fires_a_scheduled_play() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    create_schema(
        &service,
        "pb_cron_task",
        json!([{ "name": "touched", "type": "boolean" }]),
    )
    .await?;

    let target = Node::new(
        "pb_cron_task".to_string(),
        "cron target".to_string(),
        json!({ "touched": false }),
    );
    let target_id = target.id.clone();
    service.create_node(target).await?;

    // Registered before the engine starts, so `load_active_plays()` picks it
    // up at startup and `CronRunner`'s first tick has a live registry entry
    // to check — this test is specifically about the poll loop firing, not
    // about reactive install.
    create_play(
        &service,
        "touch-on-schedule",
        json!([{
            "name": "touch-scheduled",
            "trigger": { "type": "scheduled", "cron": "0 * * * * * *", "node_type": "pb_cron_task" },
            "conditions": [],
            "actions": [{
                "action_type": "update_node",
                "params": {
                    "node_id": "{trigger.node.id}",
                    "properties": { "touched": true }
                }
            }]
        }]),
    )
    .await?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let engine = Arc::new(PlaybookEngine::new(Arc::clone(&service)));
    let task = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.start(shutdown_rx).await })
    };

    // Let the engine's own startup chain (subscribe, load_active_plays,
    // spawn RuleProcessor, spawn CronRunner) actually run to the point where
    // CronRunner reaches its `sleep(POLL_INTERVAL)` — `tokio::spawn` only
    // schedules the task, so without yielding first, `advance` below can run
    // before the spawned chain has been polled even once.
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }

    // CronRunner wakes every 60 real seconds (`POLL_INTERVAL` in
    // cron_runner.rs). Advance the paused virtual clock past that so the
    // `tokio::time::sleep` inside `cron_runner_loop` resolves — real (i.e.
    // non-timer) async work like the DB scan and rule execution the tick
    // triggers is not gated by the paused clock and runs normally.
    tokio::time::advance(Duration::from_secs(61)).await;

    let ticked = wait_until(|| {
        let service = Arc::clone(&service);
        let id = target_id.clone();
        async move {
            matches!(
                service.get_node(&id).await,
                Ok(Some(n)) if user_field(&n, "pb_cron_task", "touched").and_then(|v| v.as_bool()) == Some(true)
            )
        }
    })
    .await;
    assert!(
        ticked,
        "CronRunner's poll loop must have ticked after 60 (virtual) seconds and fired \
         the scheduled play's action — the target node's 'touched' property must be true"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Acceptance criterion: "A scheduled Play can compute
/// `end_date = start_date + N days` and write it as a new node's field
/// value end-to-end."
///
/// Exercises the whole path this issue adds, through a REAL running engine:
/// `add_days` (registered as a CEL function in `cel.rs`) is reachable from
/// an action's computed field value through the new function-call binding
/// syntax (`{add_days(...)}`, parsed by
/// `BindingContext::resolve_function_call` in `actions.rs`) -- not just from
/// a CEL condition, which is all that existed before this issue. A
/// scheduled play scans for `pb_cycle_source` nodes and creates one
/// `pb_cycle_result` node per match whose `end_date` is computed as
/// `start_date + 14 days` entirely through the play's own JSON -- no Rust
/// test code computes the date; the assertion below only checks what the
/// engine wrote.
#[tokio::test(start_paused = true)]
async fn scheduled_play_computes_end_date_via_add_days_and_writes_it_to_a_new_node() -> Result<()> {
    let (service, _tmp) = create_test_service().await?;

    create_schema(
        &service,
        "pb_cycle_source",
        json!([{ "name": "start_date", "type": "string" }]),
    )
    .await?;
    // A distinct node type for the action to create -- reusing the
    // trigger's own type would additionally trip the self-chaining check
    // (`create_node` of the trigger's own type re-satisfies `node_created`),
    // which is unrelated to what this test is about.
    create_schema(
        &service,
        "pb_cycle_result",
        json!([{ "name": "end_date", "type": "string" }]),
    )
    .await?;

    let source = Node::new(
        "pb_cycle_source".to_string(),
        "cycle source".to_string(),
        json!({ "start_date": "2026-01-01" }),
    );
    service.create_node(source).await?;

    // Registered before the engine starts, same as
    // `cron_runner_ticks_and_fires_a_scheduled_play` above, so
    // `load_active_plays()` picks it up and the first tick has a live
    // registry entry to scan against.
    create_play(
        &service,
        "compute-cycle-end-date",
        json!([{
            "name": "compute-end-date-scheduled",
            "trigger": { "type": "scheduled", "cron": "0 * * * * * *", "node_type": "pb_cycle_source" },
            "conditions": [],
            "actions": [{
                "action_type": "create_node",
                "params": {
                    "node_type": "pb_cycle_result",
                    "content": "computed cycle result",
                    "properties": {
                        "end_date": "{add_days(trigger.node.properties.pb_cycle_source.start_date, 14)}"
                    }
                }
            }]
        }]),
    )
    .await?;

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let engine = Arc::new(PlaybookEngine::new(Arc::clone(&service)));
    let task = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.start(shutdown_rx).await })
    };

    // Let the engine's startup chain reach CronRunner's `sleep(POLL_INTERVAL)`
    // before advancing the paused virtual clock -- same rationale as
    // `cron_runner_ticks_and_fires_a_scheduled_play` above.
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(Duration::from_secs(61)).await;

    let fired = wait_until(|| {
        let service = Arc::clone(&service);
        async move {
            let results = service
                .query_nodes_by_type("pb_cycle_result", Some("active"))
                .await
                .unwrap_or_default();
            results.iter().any(|n| {
                user_field(n, "pb_cycle_result", "end_date").and_then(|v| v.as_str())
                    == Some("2026-01-15")
            })
        }
    })
    .await;
    assert!(
        fired,
        "scheduled play must have created a pb_cycle_result node whose \
         end_date was computed by the engine as start_date (2026-01-01) \
         + 14 days = 2026-01-15, via {{add_days(...)}} in the action's \
         properties param"
    );

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// Regression test for a log-node fingerprint collision in the validation-
/// error logging path shared by `load_active_plays`/`handle_play_created`/
/// `handle_play_updated`: it used to pass a constant `rule_name="validation"`
/// and `error_location_index=0` into `create_or_update_log_node` for every
/// error on a play, so `error_fingerprint` (which deliberately excludes the
/// error message from its hash) collapsed any two validation errors on the
/// SAME play onto the SAME fingerprint — the second error's text was
/// silently discarded and the surviving log node's `occurrences` count
/// misleadingly implied a repeat of one issue rather than two distinct ones.
///
/// This play has two rules with two structurally DIFFERENT validation
/// errors: a `reject` action declared on a (default) `Reactive` rule
/// (`RejectActionOnReactiveRule`), and a second rule whose action references
/// a schema type that does not exist (`UnknownNodeType`). Both errors surface
/// through `load_active_plays` (`PlaybookEngine::start()`'s startup load) —
/// not `handle_play_created`, because `NodeService::create_node`'s own
/// primary validation gate would refuse to persist a play this invalid in
/// the first place (`validate_play_rules` runs there too, ahead of the
/// engine's belt-and-suspenders re-validation). Exactly like
/// `reject_on_reactive_rule_bypassing_save_time_validation_is_not_activated_at_load`
/// in `playbook_invariant_integration_test.rs`, this test writes the invalid
/// play directly through the store, bypassing that gate, to reach the real
/// production scenario `load_active_plays`'s re-validation guards against: a
/// play that reached the DB without going through this build's create/update
/// validation (a replicated write from another device, or one written before
/// a check existed). With the fix, each error's own `(location, kind)` is
/// folded into the fingerprint, so both must surface as separate
/// `playbook_log` nodes with their own distinct message text — not one node
/// with `occurrences: 2`.
#[tokio::test]
async fn distinct_validation_errors_on_the_same_play_produce_separate_log_nodes() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);

    create_schema(
        &service,
        "pb_log_dedup_task",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let invalid_play = Node::new(
        "play".to_string(),
        "two-distinct-validation-errors".to_string(),
        json!({ "rules": [
            {
                "name": "reject-on-reactive",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "pb_log_dedup_task" },
                "conditions": ["node.status == 'blocked'"],
                "actions": [{
                    "action_type": "reject",
                    "params": { "message": "blocked tasks may not be created" }
                }]
            },
            {
                "name": "create-unknown-type",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "pb_log_dedup_task" },
                "conditions": [],
                "actions": [{
                    "action_type": "create_node",
                    "params": {
                        "node_type": "pb_log_dedup_nonexistent_type",
                        "content": "x",
                        "properties": {}
                    }
                }]
            }
        ] }),
    );
    let play_id = invalid_play.id.clone();

    // Bypasses NodeService::create_node's validate_play_rules gate entirely
    // — the point of this test (see the doc comment above).
    store.create_node(invalid_play, None, None).await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    // Both errors are logged as `playbook_log` nodes for the SAME play_id —
    // this is exactly the case that used to collide onto one fingerprint.
    let got_both = wait_until(|| {
        let service = Arc::clone(&service);
        async move {
            let logs = service
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
                .unwrap_or_default();
            logs.len() >= 2
        }
    })
    .await;
    assert!(
        got_both,
        "expected two distinct playbook_log nodes for the two structurally \
         different validation errors on this play"
    );

    let logs = service
        .query_nodes_by_type("playbook_log", Some("active"))
        .await?;
    let logs_for_play: Vec<_> = logs
        .iter()
        .filter(|n| {
            n.properties
                .get("playbook_log")
                .and_then(|ns| ns.get("play_id"))
                .and_then(|v| v.as_str())
                == Some(play_id.as_str())
        })
        .collect();

    assert_eq!(
        logs_for_play.len(),
        2,
        "two structurally different validation errors on the same play must \
         produce two separate log nodes (fingerprint collision would collapse \
         them into one with occurrences: 2), got: {:?}",
        logs_for_play.iter().map(|n| &n.content).collect::<Vec<_>>()
    );

    let reject_log = logs_for_play
        .iter()
        .find(|n| n.content.contains("reject action"))
        .expect("one log node must be the RejectActionOnReactiveRule error");
    let unknown_type_log = logs_for_play
        .iter()
        .find(|n| n.content.contains("pb_log_dedup_nonexistent_type"))
        .expect("the other log node must be the UnknownNodeType error");

    assert_ne!(
        reject_log.id, unknown_type_log.id,
        "the two errors must be two distinct nodes, not the same node matched twice"
    );

    for log in [reject_log, unknown_type_log] {
        let occurrences = log
            .properties
            .get("playbook_log")
            .and_then(|ns| ns.get("occurrences"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            occurrences,
            Some(1),
            "each distinct error must be its own fresh log node (occurrences: 1), \
             not a dedup-incremented repeat of the other's fingerprint"
        );
    }

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// `(location, kind)` alone is not always unique within a single
/// `validate_play` pass: one condition can reference two distinct broken
/// dot-paths, and `validate_schema_path` is called once per path with the
/// SAME `location` string (the condition's own location, computed once by
/// the caller) — so both `BrokenPath` errors share both `location` and
/// `kind`. Without further disambiguation these would collide onto one
/// `error_fingerprint`, silently discarding the second broken path's own
/// message — exactly the bug `log_validation_errors` exists to prevent, just
/// narrower (same-location, not just same-play). `log_validation_errors`
/// must break the tie by ordinal so both still surface as distinct
/// `playbook_log` nodes.
#[tokio::test]
async fn two_broken_paths_in_the_same_condition_produce_separate_log_nodes() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);

    create_schema(
        &service,
        "pb_same_loc_task",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let invalid_play = Node::new(
        "play".to_string(),
        "two-broken-paths-same-condition".to_string(),
        json!({ "rules": [
            {
                "name": "condition-with-two-broken-paths",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "pb_same_loc_task" },
                "conditions": ["node.nonexistent_a.x == node.nonexistent_b.y"],
                "actions": [{
                    "action_type": "create_node",
                    "params": {
                        "node_type": "pb_same_loc_task",
                        "content": "x",
                        "properties": {}
                    }
                }]
            }
        ] }),
    );
    let play_id = invalid_play.id.clone();

    // Bypasses NodeService::create_node's validate_play_rules gate entirely
    // — same rationale as the test above.
    store.create_node(invalid_play, None, None).await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    let got_both = wait_until(|| {
        let service = Arc::clone(&service);
        async move {
            let logs = service
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
                .unwrap_or_default();
            logs.len() >= 2
        }
    })
    .await;
    assert!(
        got_both,
        "expected two distinct playbook_log nodes for the two BrokenPath \
         errors at the same rule[0].condition[0] location"
    );

    let logs = service
        .query_nodes_by_type("playbook_log", Some("active"))
        .await?;
    let logs_for_play: Vec<_> = logs
        .iter()
        .filter(|n| {
            n.properties
                .get("playbook_log")
                .and_then(|ns| ns.get("play_id"))
                .and_then(|v| v.as_str())
                == Some(play_id.as_str())
        })
        .collect();

    assert_eq!(
        logs_for_play.len(),
        2,
        "two BrokenPath errors sharing the same (location, kind) must still \
         produce two separate log nodes, got: {:?}",
        logs_for_play.iter().map(|n| &n.content).collect::<Vec<_>>()
    );

    let a_log = logs_for_play
        .iter()
        .find(|n| n.content.contains("nonexistent_a"))
        .expect("one log node must be the broken path through 'nonexistent_a'");
    let b_log = logs_for_play
        .iter()
        .find(|n| n.content.contains("nonexistent_b"))
        .expect("the other log node must be the broken path through 'nonexistent_b'");

    assert_ne!(
        a_log.id, b_log.id,
        "the two same-location BrokenPath errors must be two distinct nodes, \
         not the same node matched twice"
    );

    for log in [a_log, b_log] {
        let occurrences = log
            .properties
            .get("playbook_log")
            .and_then(|ns| ns.get("occurrences"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            occurrences,
            Some(1),
            "each distinct same-location error must be its own fresh log \
             node (occurrences: 1), not a dedup-incremented repeat of the \
             other's fingerprint"
        );
    }

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// A second same-`(location, kind)` collision case: `validate_invariant_eligibility`'s
/// same-graph-scope check walks `target_id_params` (`["source_id", "target_id"]`
/// for `add_relationship`) in a loop over ONE action, so a single `add_relationship`
/// whose `source_id` AND `target_id` are both literal (non-binding) node ids pushes
/// two `InvariantOutOfScopeTarget` errors that share both `location`
/// (`rule[0].action[0]`) and `kind` (`invariant_out_of_scope_target`) — the same
/// shape of collision as the two-broken-paths case above, just through a different
/// validator.
#[tokio::test]
async fn two_out_of_scope_targets_on_the_same_action_produce_separate_log_nodes() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);

    create_schema(
        &service,
        "pb_oos_task",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    // Declare the relationship the invalid rule below references, so the
    // ONLY error it produces is the out-of-scope-target one under test —
    // an undeclared relationship_type would additionally fail
    // `UnknownRelationshipType` validation and inflate the log-node count
    // for reasons unrelated to this test.
    let relationships: Vec<nodespace_core::models::schema::SchemaRelationship> =
        serde_json::from_value(json!([{
            "name": "linked_to",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "linked_from",
            "reverseCardinality": "many"
        }]))?;
    service
        .set_schema_relationships("pb_oos_task", &relationships)
        .await?;

    let invalid_play = Node::new(
        "play".to_string(),
        "both-ends-out-of-scope".to_string(),
        json!({ "rules": [
            {
                "name": "invariant-both-ends-out-of-scope",
                "class": "invariant",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "pb_oos_task" },
                "conditions": [],
                "actions": [{
                    "action_type": "add_relationship",
                    "params": {
                        "source_id": "literal-source-node",
                        "relationship_type": "linked_to",
                        "target_id": "literal-target-node"
                    }
                }]
            }
        ] }),
    );
    let play_id = invalid_play.id.clone();

    // Bypasses NodeService::create_node's validate_play_rules gate entirely
    // — same rationale as the tests above.
    store.create_node(invalid_play, None, None).await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    let got_both = wait_until(|| {
        let service = Arc::clone(&service);
        async move {
            let logs = service
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
                .unwrap_or_default();
            logs.len() >= 2
        }
    })
    .await;
    assert!(
        got_both,
        "expected two distinct playbook_log nodes for the two \
         InvariantOutOfScopeTarget errors at the same rule[0].action[0] location"
    );

    let logs = service
        .query_nodes_by_type("playbook_log", Some("active"))
        .await?;
    let logs_for_play: Vec<_> = logs
        .iter()
        .filter(|n| {
            n.properties
                .get("playbook_log")
                .and_then(|ns| ns.get("play_id"))
                .and_then(|v| v.as_str())
                == Some(play_id.as_str())
        })
        .collect();

    assert_eq!(
        logs_for_play.len(),
        2,
        "two InvariantOutOfScopeTarget errors sharing the same (location, kind) \
         must still produce two separate log nodes, got: {:?}",
        logs_for_play.iter().map(|n| &n.content).collect::<Vec<_>>()
    );

    let source_log = logs_for_play
        .iter()
        .find(|n| n.content.contains("literal-source-node"))
        .expect("one log node must be the out-of-scope source_id error");
    let target_log = logs_for_play
        .iter()
        .find(|n| n.content.contains("literal-target-node"))
        .expect("the other log node must be the out-of-scope target_id error");

    assert_ne!(
        source_log.id, target_log.id,
        "the two same-location InvariantOutOfScopeTarget errors must be two \
         distinct nodes, not the same node matched twice"
    );

    for log in [source_log, target_log] {
        let occurrences = log
            .properties
            .get("playbook_log")
            .and_then(|ns| ns.get("occurrences"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            occurrences,
            Some(1),
            "each distinct same-location error must be its own fresh log \
             node (occurrences: 1), not a dedup-incremented repeat of the \
             other's fingerprint"
        );
    }

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}

/// A third same-`(location, kind)` collision case: `validate_invariant_eligibility`'s
/// determinism check calls `extract_function_names` once per condition and pushes
/// one `InvariantNonDeterministic` error per matched non-deterministic function
/// name, all at that SAME condition's location. A single condition referencing
/// two different non-deterministic functions (`days_since` and `days_until`) — as
/// opposed to two broken paths or two out-of-scope action params — therefore
/// collides the same way, through yet another one of the three loops
/// `validate_invariant_eligibility` runs.
#[tokio::test]
async fn two_non_deterministic_functions_in_the_same_condition_produce_separate_log_nodes(
) -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let service = Arc::new(NodeService::new(&mut store).await?);

    create_schema(
        &service,
        "pb_nondet_task",
        json!([
            { "name": "created_at", "type": "string" },
            { "name": "due_at", "type": "string" }
        ]),
    )
    .await?;
    // A distinct node type for the rule's action to create — reusing the
    // trigger's own type would additionally trip the self-chaining check
    // (`create_node` of the trigger's own type re-satisfies `node_created`),
    // inflating the log-node count for reasons unrelated to this test.
    create_schema(
        &service,
        "pb_nondet_other",
        json!([{ "name": "status", "type": "string" }]),
    )
    .await?;

    let invalid_play = Node::new(
        "play".to_string(),
        "two-non-deterministic-functions-same-condition".to_string(),
        json!({ "rules": [
            {
                "name": "invariant-two-non-deterministic-functions",
                "class": "invariant",
                "trigger": { "type": "graph_event", "on": "node_created", "node_type": "pb_nondet_task" },
                "conditions": ["days_since(node.created_at) > 5 && days_until(node.due_at) < 2"],
                "actions": [{
                    "action_type": "create_node",
                    "params": {
                        "node_type": "pb_nondet_other",
                        "content": "x",
                        "properties": {}
                    }
                }]
            }
        ] }),
    );
    let play_id = invalid_play.id.clone();

    // Bypasses NodeService::create_node's validate_play_rules gate entirely
    // — same rationale as the tests above.
    store.create_node(invalid_play, None, None).await?;

    let (_engine, shutdown_tx, task) = spawn_engine(&service).await;

    let got_both = wait_until(|| {
        let service = Arc::clone(&service);
        async move {
            let logs = service
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
                .unwrap_or_default();
            logs.len() >= 2
        }
    })
    .await;
    assert!(
        got_both,
        "expected two distinct playbook_log nodes for the two \
         InvariantNonDeterministic errors at the same rule[0].condition[0] location"
    );

    let logs = service
        .query_nodes_by_type("playbook_log", Some("active"))
        .await?;
    let logs_for_play: Vec<_> = logs
        .iter()
        .filter(|n| {
            n.properties
                .get("playbook_log")
                .and_then(|ns| ns.get("play_id"))
                .and_then(|v| v.as_str())
                == Some(play_id.as_str())
        })
        .collect();

    assert_eq!(
        logs_for_play.len(),
        2,
        "two InvariantNonDeterministic errors sharing the same (location, kind) \
         must still produce two separate log nodes, got: {:?}",
        logs_for_play.iter().map(|n| &n.content).collect::<Vec<_>>()
    );

    let days_since_log = logs_for_play
        .iter()
        .find(|n| n.content.contains("days_since"))
        .expect("one log node must be the 'days_since' non-determinism error");
    let days_until_log = logs_for_play
        .iter()
        .find(|n| n.content.contains("days_until"))
        .expect("the other log node must be the 'days_until' non-determinism error");

    assert_ne!(
        days_since_log.id, days_until_log.id,
        "the two same-location InvariantNonDeterministic errors must be two \
         distinct nodes, not the same node matched twice"
    );

    for log in [days_since_log, days_until_log] {
        let occurrences = log
            .properties
            .get("playbook_log")
            .and_then(|ns| ns.get("occurrences"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            occurrences,
            Some(1),
            "each distinct same-location error must be its own fresh log \
             node (occurrences: 1), not a dedup-incremented repeat of the \
             other's fingerprint"
        );
    }

    shutdown_engine(shutdown_tx, task).await;
    Ok(())
}
