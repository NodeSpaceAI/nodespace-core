//! CronRunner — 60-second polling loop for scheduled play triggers.
//!
//! A single tokio task wakes every 60 seconds, reads the `CronRegistry` from
//! `PlaybookLifecycleManager`, parses cron expressions via the `cron` crate,
//! and enqueues `ExecutionWorkItem`s for matching nodes.
//!
//! # Design decisions
//!
//! - **Simple polling, not a scheduler.** The `cron` crate is used only for
//!   expression-to-time matching, not as a scheduler.
//! - **Deduplication built-in.** Each `CronEntry` groups all rules sharing the
//!   same `(cron_expression, selector)` pair, so only one query is issued per
//!   unique pair.
//! - **Selected and resolved in SQL.** An entry's selector runs as one query
//!   (its type and filters, or a saved query's), and the rules' condition
//!   paths are resolved for every selected node in one statement per path
//!   before any node is enqueued.
//! - **Missed runs are skipped.** If NodeSpace was not running when a cron
//!   expression was due, execution is not retried.
//! - **Contiguous windows.** Each check covers `(last_checked, now]`, where
//!   `last_checked` is the previous check's `now`. The loop sleeps *after*
//!   work, so a slow check (large cron registry, big node scans) pushes the
//!   next one later; anchoring on the previous check rather than on
//!   `now - POLL_INTERVAL` keeps that delay from opening a gap in which an
//!   occurrence would match neither window. A long stall (e.g. system sleep)
//!   fires each due entry at most once when checking resumes.
//! - **Dynamic registration.** The registry is re-read on each wake, so
//!   play activations/deactivations take effect within 60 seconds.
//! - **Cron expressions use 7-field format** (sec min hour dom month dow year)
//!   as required by the `cron` crate. Example: `"0 * * * * * *"` fires every
//!   minute at second 0.

use crate::db::events::{DomainEvent, EventEnvelope, EventMetadata};
use crate::playbook::cel::condition_paths;
use crate::playbook::graph_resolver::GraphResolver;
use crate::playbook::lifecycle::PlaybookLifecycleManager;
use crate::playbook::selectors::select_nodes;
use crate::playbook::types::{CronEntry, ExecutionWorkItem, ScanContext};
use crate::services::NodeService;
use cron::Schedule;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// Interval between cron checks (60 seconds).
const POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Run the cron polling loop.
///
/// Wakes every 60 seconds, reads the `CronRegistry`, and for each entry whose
/// cron expression matches the current minute window, runs the entry's
/// selector and enqueues a work item per node it matches.
///
/// Exits when `shutdown_rx` receives `true` or the watch sender is dropped.
pub async fn cron_runner_loop(
    lifecycle: Arc<RwLock<PlaybookLifecycleManager>>,
    node_service: Arc<NodeService>,
    queue_tx: mpsc::Sender<ExecutionWorkItem>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    debug!(
        "CronRunner started, polling every {} seconds",
        POLL_INTERVAL.as_secs()
    );

    let mut last_checked = chrono::Local::now();

    loop {
        tokio::select! {
            () = tokio::time::sleep(POLL_INTERVAL) => {
                last_checked =
                    check_and_enqueue(&lifecycle, &node_service, &queue_tx, last_checked).await;
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    debug!("CronRunner received shutdown signal");
                    break;
                }
            }
        }
    }

    debug!("CronRunner stopped");
}

/// Start of the window a check ending at `now` covers.
///
/// Normally `last_checked`, so consecutive windows are contiguous however long
/// the previous check took. Never later than `now - POLL_INTERVAL`: the timer
/// runs on tokio's clock while `now` is wall-clock time, and when those diverge
/// (a paused tokio clock in tests) consecutive wall-clock checks can be nearly
/// simultaneous. If the wall clock steps backward, this floor makes the window
/// overlap the previous one, so an occurrence in the overlap can fire twice.
fn window_start(
    last_checked: chrono::DateTime<chrono::Local>,
    now: chrono::DateTime<chrono::Local>,
) -> chrono::DateTime<chrono::Local> {
    let min_lookback = now - chrono::Duration::seconds(POLL_INTERVAL.as_secs() as i64);
    last_checked.min(min_lookback)
}

/// Whether `schedule` has an occurrence in the half-open window `(start, end]`.
fn fires_in_window(
    schedule: &Schedule,
    start: &chrono::DateTime<chrono::Local>,
    end: &chrono::DateTime<chrono::Local>,
) -> bool {
    schedule.after(start).take(1).any(|t| t <= *end)
}

/// Check all cron entries against the window since `last_checked` and enqueue
/// work items for matching nodes. Returns the window end, which the caller
/// passes back as `last_checked` on the next check.
pub(crate) async fn check_and_enqueue(
    lifecycle: &Arc<RwLock<PlaybookLifecycleManager>>,
    node_service: &Arc<NodeService>,
    queue_tx: &mpsc::Sender<ExecutionWorkItem>,
    last_checked: chrono::DateTime<chrono::Local>,
) -> chrono::DateTime<chrono::Local> {
    let now = chrono::Local::now();
    let start = window_start(last_checked, now);

    // Read cron registry (short read lock, then release).
    // A poisoned lock (some other thread panicked while holding it) still
    // holds a valid `PlaybookLifecycleManager` — recover it instead of
    // propagating the panic, which would permanently kill this polling loop.
    // `into_inner()` trades a hard failure (permanently dead cron loop) for a
    // best-effort read of whatever state existed at the panic: `active_playbooks`,
    // `trigger_index`, and `cron_registry` are plain HashMap/Vec collections with
    // no unsafe invariants, so the recovered guard can't be a type-invalid or
    // corrupted value — but `activate_play` does update `trigger_index`/
    // `cron_registry` before `active_playbooks` in a loop over a play's rules,
    // so a panic mid-activation could in principle leave a stale/partial entry
    // for a play that never finished activating. That's an existing risk of
    // this lock (any `.expect(...)` reader hitting it mid-activation would panic
    // instead), not something this recovery path introduces — it just avoids
    // amplifying "one bad activation" into "the cron loop is dead forever."
    let entries: Vec<CronEntry> = {
        let guard = lifecycle
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.cron_registry().clone()
    };

    if entries.is_empty() {
        return now;
    }

    debug!("CronRunner checking {} cron entries", entries.len());

    for entry in &entries {
        let schedule: Schedule = match Schedule::from_str(&entry.cron_expression) {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "Invalid cron expression '{}' for selector '{}': {}",
                    entry.cron_expression, entry.select, e
                );
                continue;
            }
        };

        if !fires_in_window(&schedule, &start, &now) {
            continue;
        }

        debug!(
            "Cron expression '{}' matched for selector '{}'",
            entry.cron_expression, entry.select
        );

        // The selector runs as one query, in SQL, per (cron, selector) pair.
        // Archived nodes never match it.
        let (target_type, nodes) = match select_nodes(node_service, &entry.select).await {
            Ok(selected) => selected,
            Err(e) => {
                error!(
                    "Failed to run selector '{}' for cron trigger: {}",
                    entry.select, e
                );
                continue;
            }
        };

        // Resolve the rules' condition paths for every selected node at once:
        // one statement per path for the whole scan, rather than a walk per
        // node when each is evaluated.
        let mut resolver = GraphResolver::new(Arc::clone(node_service));
        for rule_ref in &entry.rules {
            let (paths, collections) = condition_paths(&rule_ref.rule.conditions);
            resolver.resolve_ahead(&nodes, &paths, &collections).await;
        }
        debug!(
            "Cron trigger: selector '{}' matched {} nodes, enqueueing with {} rules \
             ({} path statements)",
            entry.select,
            nodes.len(),
            entry.rules.len(),
            resolver.statements_run(),
        );
        let scan = Arc::new(ScanContext {
            target_type,
            paths: resolver.into_cache(),
        });

        // Enqueue each node with all matching rules
        for node in nodes {
            let work_item = ExecutionWorkItem {
                rules: entry.rules.clone(),
                trigger_event: synthetic_cron_envelope(&node.id),
                trigger_node: node,
                scan: Some(Arc::clone(&scan)),
            };

            if let Err(e) = queue_tx.try_send(work_item) {
                match e {
                    mpsc::error::TrySendError::Full(_) => {
                        warn!(
                            "ExecutionQueue full, dropping cron work item for selector '{}'",
                            entry.select
                        );
                    }
                    mpsc::error::TrySendError::Closed(_) => {
                        debug!("ExecutionQueue closed, CronRunner stopping enqueue");
                        return now;
                    }
                }
            }
        }
    }

    now
}

/// Create a synthetic `EventEnvelope` for cron-triggered work items.
///
/// Cron triggers don't correspond to a real domain event. The RuleProcessor
/// uses `trigger_event.metadata.playbook_context` for chain depth (always 0
/// for cron since there's no originating event). The `event` field is set to
/// a `NodeCreated` placeholder — the processor doesn't inspect it for
/// cron-sourced work items.
fn synthetic_cron_envelope(node_id: &str) -> EventEnvelope {
    EventEnvelope {
        event: DomainEvent::NodeCreated {
            node_id: node_id.to_string(),
            node_type: "cron_trigger".to_string(),
        },
        metadata: EventMetadata {
            source_client_id: Some("cron-runner".to_string()),
            playbook_context: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::types::{
        ActionType, CronEntry, OrderedRuleRef, ParsedAction, ParsedRule, ParsedTrigger, RuleClass,
        Selector,
    };
    use std::sync::Arc;

    /// Helper: create a CronEntry with the given expression, node_type, and a single rule.
    fn make_cron_entry(cron_expr: &str, node_type: &str) -> CronEntry {
        let rule = Arc::new(ParsedRule {
            name: "test-cron-rule".to_string(),
            class: RuleClass::Reactive,
            trigger: ParsedTrigger::Scheduled {
                cron: cron_expr.to_string(),
                select: Selector::of_type(node_type),
            },
            conditions: vec![],
            actions: vec![ParsedAction {
                action_type: ActionType::UpdateNode,
                params: serde_json::json!({"node_id": "{trigger.node.id}"}),
                for_each: None,
            }],
        });

        CronEntry {
            cron_expression: cron_expr.to_string(),
            select: Selector::of_type(node_type),
            rules: vec![OrderedRuleRef {
                play_id: "play-1".to_string(),
                rule_index: 0,
                rule,
            }],
        }
    }

    #[test]
    fn test_synthetic_envelope_has_no_playbook_context() {
        let envelope = synthetic_cron_envelope("node-123");

        // playbook_context is None → depth 0 in processor
        assert!(envelope.metadata.playbook_context.is_none());
        assert_eq!(
            envelope.metadata.source_client_id.as_deref(),
            Some("cron-runner")
        );
    }

    #[test]
    fn test_synthetic_envelope_event_variant() {
        let envelope = synthetic_cron_envelope("node-abc");

        match &envelope.event {
            DomainEvent::NodeCreated { node_id, node_type } => {
                assert_eq!(node_id, "node-abc");
                assert_eq!(node_type, "cron_trigger");
            }
            _ => panic!("expected NodeCreated variant in synthetic envelope"),
        }
    }

    #[test]
    fn test_cron_expression_every_minute_matches_within_window() {
        // 7-field cron: "sec min hour dom month dow year"
        // "0 * * * * * *" = every minute at second 0
        let schedule = cron::Schedule::from_str("0 * * * * * *").unwrap();
        let now = chrono::Local::now();
        let window_start = now - chrono::Duration::seconds(60);

        // Every-minute expression should always have a match in a 60s window
        let matches = fires_in_window(&schedule, &window_start, &now);
        assert!(matches, "every-minute cron should match within 60s window");
    }

    #[test]
    fn test_cron_expression_far_future_does_not_match() {
        // "0 0 0 29 2 * 2099" = midnight Feb 29, 2099 (far future)
        let schedule = cron::Schedule::from_str("0 0 0 29 2 * 2099").unwrap();
        let now = chrono::Local::now();
        let window_start = now - chrono::Duration::seconds(60);

        let matches = fires_in_window(&schedule, &window_start, &now);
        assert!(
            !matches,
            "far-future cron expression should not match current window"
        );
    }

    /// A slow check delays the next one past a full poll interval. The next
    /// window must reach back to where the previous one ended, or an
    /// occurrence in the delay matches neither window.
    #[test]
    fn test_slow_check_leaves_no_gap_between_windows() {
        use chrono::TimeZone;
        // Fires once a day at 09:00:30.
        let schedule = cron::Schedule::from_str("30 0 9 * * * *").unwrap();
        let at = |h, m, s| {
            chrono::Local
                .with_ymd_and_hms(2026, 3, 10, h, m, s)
                .unwrap()
        };

        // Previous check ended at 08:59:50. Its work took 45s, then the loop
        // slept 60s, so this check runs at 09:01:35 — 09:00:30 is 65s ago,
        // outside a fixed 60s lookback.
        let last_checked = at(8, 59, 50);
        let now = at(9, 1, 35);
        assert!(!fires_in_window(
            &schedule,
            &(now - chrono::Duration::seconds(60)),
            &now
        ));

        let start = window_start(last_checked, now);
        assert_eq!(start, last_checked);
        assert!(fires_in_window(&schedule, &start, &now));
        // …and the check before that (ending at `last_checked`, starting a
        // poll interval earlier) did not already cover it.
        assert!(!fires_in_window(
            &schedule,
            &window_start(at(8, 58, 50), last_checked),
            &last_checked
        ));
    }

    #[test]
    fn test_window_start_looks_back_at_least_one_poll_interval() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 3, 10, 9, 0, 0)
            .unwrap();
        // Checks closer together than the poll interval (clock set back,
        // virtual test clock) still get a full lookback.
        let last_checked = now - chrono::Duration::seconds(5);
        assert_eq!(
            window_start(last_checked, now),
            now - chrono::Duration::seconds(60)
        );
    }

    #[test]
    fn test_invalid_cron_expression_is_handled() {
        let result = cron::Schedule::from_str("not a cron expression");
        assert!(result.is_err(), "invalid expression should fail to parse");
    }

    #[test]
    fn test_cron_entry_deduplication_structure() {
        // Verify that a CronEntry with multiple rules produces a single entry
        // (deduplication is structural — same cron+node_type = one CronEntry)
        let rule_a = Arc::new(ParsedRule {
            name: "rule-a".to_string(),
            class: RuleClass::Reactive,
            trigger: ParsedTrigger::Scheduled {
                cron: "0 * * * * * *".to_string(),
                select: Selector::of_type("task"),
            },
            conditions: vec![],
            actions: vec![],
        });

        let rule_b = Arc::new(ParsedRule {
            name: "rule-b".to_string(),
            class: RuleClass::Reactive,
            trigger: ParsedTrigger::Scheduled {
                cron: "0 * * * * * *".to_string(),
                select: Selector::of_type("task"),
            },
            conditions: vec![],
            actions: vec![],
        });

        let entry = CronEntry {
            cron_expression: "0 * * * * * *".to_string(),
            select: Selector::of_type("task"),
            rules: vec![
                OrderedRuleRef {
                    play_id: "pb-1".to_string(),
                    rule_index: 0,
                    rule: rule_a,
                },
                OrderedRuleRef {
                    play_id: "pb-2".to_string(),
                    rule_index: 0,
                    rule: rule_b,
                },
            ],
        };

        // One CronEntry → one DB query, but two rules applied per node
        assert_eq!(entry.rules.len(), 2);
        assert_eq!(entry.cron_expression, "0 * * * * * *");
        assert_eq!(entry.select, Selector::of_type("task"));
    }

    #[test]
    fn test_make_cron_entry_helper() {
        let entry = make_cron_entry("0 30 9 * * * *", "invoice");
        assert_eq!(entry.cron_expression, "0 30 9 * * * *");
        assert_eq!(entry.select, Selector::of_type("invoice"));
        assert_eq!(entry.rules.len(), 1);
        assert_eq!(entry.rules[0].play_id, "play-1");
    }

    // -----------------------------------------------------------------------
    // Integration tests — check_and_enqueue with real NodeService + lifecycle
    // -----------------------------------------------------------------------

    mod integration {
        use super::*;
        use crate::db::SqliteStore;
        use crate::models::Node;
        use crate::playbook::lifecycle::PlaybookLifecycleManager;
        use crate::services::NodeService;
        use serde_json::json;
        use std::sync::{Arc, RwLock};
        use tempfile::TempDir;
        use tokio::sync::mpsc;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        fn make_lifecycle_with_cron(
            cron_expr: &str,
            node_type: &str,
        ) -> Arc<RwLock<PlaybookLifecycleManager>> {
            let mut lm = PlaybookLifecycleManager::new();

            // Manually create a play node with a scheduled trigger and activate it
            let play_node = Node {
                id: "f63d374f-e304-519e-9876-3213bae0b20a".to_string(),
                node_type: "play".to_string(),
                content: "cron play".to_string(),
                version: 1,
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                properties: json!({
                    "play": {
                        "rules": [{
                            "name": "cron-rule-1",
                            "description": "Test rule",
                            "trigger": {
                                "type": "scheduled",
                                "cron": cron_expr,
                                "select": { "target_type": node_type }
                            },
                            "conditions": [],
                            "actions": [{
                                "description": "Test action",
                                "action_type": "update_node",
                                "params": {"node_id": "{trigger.node.id}"}
                            }]
                        }]
                    }
                }),
                mentions: vec![],
                mentioned_in: vec![],
                title: Some("Cron Play".to_string()),
                lifecycle_status: "active".to_string(),
            };
            lm.activate_play(&play_node).unwrap();

            Arc::new(RwLock::new(lm))
        }

        #[tokio::test]
        async fn check_and_enqueue_enqueues_for_matching_cron() {
            let (svc, _tmp) = create_test_service().await;

            // Create schema for the node type
            let schema = Node::new_with_id(
                "cr-task".to_string(),
                "schema".to_string(),
                "cr-task".to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "cr-task schema",
                    "fields": [{"name": "status", "type": "text"}],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            // Create active nodes of type "cr-task"
            let node1 = Node::new_with_id(
                "c3a9129b-ee73-5379-b845-e13aef9e81a6".to_string(),
                "cr-task".to_string(),
                "task 1".to_string(),
                json!({"status": "open"}),
            );
            let node2 = Node::new_with_id(
                "2806fa65-22e1-5e87-a087-874666c63f01".to_string(),
                "cr-task".to_string(),
                "task 2".to_string(),
                json!({"status": "open"}),
            );
            svc.create_node(node1).await.unwrap();
            svc.create_node(node2).await.unwrap();

            // Use "every minute" cron — guaranteed to match in any 60s window
            let lifecycle = make_lifecycle_with_cron("0 * * * * * *", "cr-task");

            let (tx, mut rx) = mpsc::channel::<ExecutionWorkItem>(100);
            check_and_enqueue(&lifecycle, &svc, &tx, chrono::Local::now()).await;

            // Should have enqueued work items for both nodes
            let mut received = vec![];
            while let Ok(item) = rx.try_recv() {
                received.push(item);
            }
            assert_eq!(
                received.len(),
                2,
                "should enqueue one work item per matching node"
            );

            // Each work item should carry the cron rule
            for item in &received {
                assert_eq!(item.rules.len(), 1);
                assert_eq!(item.rules[0].rule.name, "cron-rule-1");
            }

            // Both node IDs should be represented
            let ids: Vec<&str> = received
                .iter()
                .map(|w| w.trigger_node.id.as_str())
                .collect();
            assert!(ids.contains(&"c3a9129b-ee73-5379-b845-e13aef9e81a6"));
            assert!(ids.contains(&"2806fa65-22e1-5e87-a087-874666c63f01"));
        }

        /// A scan selects no archived node (ADR-087 §2): the scheduled rule
        /// is enqueued for the participating node only.
        #[tokio::test]
        async fn check_and_enqueue_skips_an_archived_node() {
            let (svc, _tmp) = create_test_service().await;

            let schema = Node::new_with_id(
                "cr-task-archived".to_string(),
                "schema".to_string(),
                "cr-task-archived".to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "cr-task-archived schema",
                    "fields": [{"name": "status", "type": "text"}],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            let live = Node::new(
                "cr-task-archived".to_string(),
                "live task".to_string(),
                json!({"status": "open"}),
            );
            let live_id = svc.create_node(live).await.unwrap();
            let retired = Node::new(
                "cr-task-archived".to_string(),
                "retired task".to_string(),
                json!({"status": "open"}),
            );
            let retired_id = svc.create_node(retired).await.unwrap();
            let version = svc.get_node(&retired_id).await.unwrap().unwrap().version;
            svc.update_node(
                &retired_id,
                version,
                crate::models::NodeUpdate::new()
                    .with_lifecycle_status(crate::governance::ARCHIVED.to_string()),
            )
            .await
            .unwrap();

            let lifecycle = make_lifecycle_with_cron("0 * * * * * *", "cr-task-archived");
            let (tx, mut rx) = mpsc::channel::<ExecutionWorkItem>(100);
            check_and_enqueue(&lifecycle, &svc, &tx, chrono::Local::now()).await;

            let mut scanned = vec![];
            while let Ok(item) = rx.try_recv() {
                scanned.push(item.trigger_node.id);
            }
            assert_eq!(scanned, vec![live_id], "an archived node is no scan target");
        }

        #[tokio::test]
        async fn check_and_enqueue_skips_non_matching_cron() {
            let (svc, _tmp) = create_test_service().await;

            // Create schema
            let schema = Node::new_with_id(
                "cr-task2".to_string(),
                "schema".to_string(),
                "cr-task2".to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "cr-task2 schema",
                    "fields": [],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            let node = Node::new_with_id(
                "9607d693-7b48-5724-9923-ad25df0c4278".to_string(),
                "cr-task2".to_string(),
                "task 3".to_string(),
                json!({}),
            );
            svc.create_node(node).await.unwrap();

            // Far-future cron expression — should NOT match current window
            let lifecycle = make_lifecycle_with_cron("0 0 0 29 2 * 2099", "cr-task2");

            let (tx, mut rx) = mpsc::channel::<ExecutionWorkItem>(100);
            check_and_enqueue(&lifecycle, &svc, &tx, chrono::Local::now()).await;

            // Should not enqueue anything
            assert!(
                rx.try_recv().is_err(),
                "non-matching cron should not enqueue any work items"
            );
        }

        /// An occurrence 90s ago, after the previous check ended 120s ago,
        /// lies outside a fixed 60s lookback. `check_and_enqueue` must still
        /// match it by starting its window at `last_checked`.
        #[tokio::test]
        async fn check_and_enqueue_matches_occurrence_since_last_check() {
            use chrono::Timelike;
            let (svc, _tmp) = create_test_service().await;

            let schema = Node::new_with_id(
                "cr-task-gap".to_string(),
                "schema".to_string(),
                "cr-task-gap".to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "cr-task-gap schema",
                    "fields": [],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();
            let node = Node::new_with_id(
                "6645c195-4778-5ecc-83c3-a647c8dd9620".to_string(),
                "cr-task-gap".to_string(),
                "task 5".to_string(),
                json!({}),
            );
            svc.create_node(node).await.unwrap();

            // Daily, at the wall-clock time 90s ago.
            let now = chrono::Local::now();
            let due = now - chrono::Duration::seconds(90);
            let cron_expr = format!("{} {} {} * * * *", due.second(), due.minute(), due.hour());
            let lifecycle = make_lifecycle_with_cron(&cron_expr, "cr-task-gap");

            let (tx, mut rx) = mpsc::channel::<ExecutionWorkItem>(100);

            // A check whose previous one just ended sees only the last 60s.
            check_and_enqueue(&lifecycle, &svc, &tx, now).await;
            assert!(
                rx.try_recv().is_err(),
                "an occurrence 90s ago is outside a 60s lookback"
            );

            let returned =
                check_and_enqueue(&lifecycle, &svc, &tx, now - chrono::Duration::seconds(120))
                    .await;
            assert_eq!(
                rx.try_recv().expect("work item enqueued").trigger_node.id,
                "6645c195-4778-5ecc-83c3-a647c8dd9620",
                "the window must reach back to the previous check's end"
            );
            assert!(
                returned >= now,
                "the returned window end is this check's `now`"
            );
        }

        #[tokio::test]
        async fn check_and_enqueue_recovers_from_poisoned_lock() {
            let (svc, _tmp) = create_test_service().await;

            let schema = Node::new_with_id(
                "cr-task-poison".to_string(),
                "schema".to_string(),
                "cr-task-poison".to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "cr-task-poison schema",
                    "fields": [{"name": "status", "type": "text"}],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            let node = Node::new_with_id(
                "348bc89e-0178-526d-a678-50d1e84f2813".to_string(),
                "cr-task-poison".to_string(),
                "task 4".to_string(),
                json!({"status": "open"}),
            );
            svc.create_node(node).await.unwrap();

            let lifecycle = make_lifecycle_with_cron("0 * * * * * *", "cr-task-poison");

            // Poison the lock: panic while holding the write guard.
            {
                let lifecycle = Arc::clone(&lifecycle);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _guard = lifecycle.write().unwrap();
                    panic!("simulated panic while holding lifecycle lock");
                }));
                assert!(result.is_err(), "the simulated panic should have occurred");
            }
            assert!(
                lifecycle.is_poisoned(),
                "lock should be poisoned after the panic"
            );

            // check_and_enqueue must recover from the poisoned lock instead of
            // panicking itself — that panic would permanently kill the cron loop.
            let (tx, mut rx) = mpsc::channel::<ExecutionWorkItem>(100);
            check_and_enqueue(&lifecycle, &svc, &tx, chrono::Local::now()).await;

            let mut received = vec![];
            while let Ok(item) = rx.try_recv() {
                received.push(item);
            }
            assert_eq!(
                received.len(),
                1,
                "should still enqueue work items after recovering from a poisoned lock"
            );
        }

        // -- Selectors: a scheduled scan selects the way a query does --

        const EVERY_MINUTE: &str = "0 * * * * * *";

        /// A lifecycle manager holding one play with the given rules.
        fn lifecycle_with_rules(rules: serde_json::Value) -> Arc<RwLock<PlaybookLifecycleManager>> {
            let mut lm = PlaybookLifecycleManager::new();
            let mut play = Node::new_with_id(
                "0a1f6c1e-2c56-4c0e-9a4e-6f1d2b3c4d5e".to_string(),
                "play".to_string(),
                "selector play".to_string(),
                json!({ "play": { "rules": rules } }),
            );
            play.lifecycle_status = "active".to_string();
            lm.activate_play(&play).unwrap();
            Arc::new(RwLock::new(lm))
        }

        fn scheduled_rule(
            select: serde_json::Value,
            conditions: serde_json::Value,
        ) -> serde_json::Value {
            json!([{
                "name": "scan",
                "description": "Test rule",
                "trigger": { "type": "scheduled", "cron": EVERY_MINUTE, "select": select },
                "conditions": conditions,
                "actions": [{
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "seen": true } }
                }]
            }])
        }

        /// A `sel-ticket` type with a `state` field and a `sel-bug` subtype,
        /// and four tickets: two open (one of them a bug), one closed, and
        /// one open but archived.
        async fn seed_tickets(svc: &Arc<NodeService>) {
            crate::schema::handle_create_schema(
                svc,
                json!({ "name": "sel-ticket", "fields": [{ "name": "state", "type": "text" }] }),
            )
            .await
            .unwrap();
            crate::schema::handle_create_schema(
                svc,
                json!({ "name": "sel-bug", "extends": "sel-ticket", "fields": [] }),
            )
            .await
            .unwrap();

            for (id, node_type, state) in [
                (OPEN_TICKET, "sel-ticket", "open"),
                (OPEN_BUG, "sel-bug", "open"),
                (CLOSED_TICKET, "sel-ticket", "closed"),
                (ARCHIVED_TICKET, "sel-ticket", "open"),
            ] {
                svc.create_node(Node::new_with_id(
                    id.to_string(),
                    node_type.to_string(),
                    id.to_string(),
                    json!({ "state": state }),
                ))
                .await
                .unwrap();
            }
            let archived = svc.get_node(ARCHIVED_TICKET).await.unwrap().unwrap();
            svc.update_node(
                ARCHIVED_TICKET,
                archived.version,
                crate::models::NodeUpdate::default().with_lifecycle_status("archived".to_string()),
            )
            .await
            .unwrap();
        }

        const OPEN_TICKET: &str = "c1000000-0000-4000-8000-000000000001";
        const OPEN_BUG: &str = "c1000000-0000-4000-8000-000000000002";
        const CLOSED_TICKET: &str = "c1000000-0000-4000-8000-000000000003";
        const ARCHIVED_TICKET: &str = "c1000000-0000-4000-8000-000000000004";

        /// Run one scan and return what it enqueued, sorted by node id.
        async fn scan(
            svc: &Arc<NodeService>,
            lifecycle: &Arc<RwLock<PlaybookLifecycleManager>>,
        ) -> Vec<ExecutionWorkItem> {
            let (tx, mut rx) = mpsc::channel::<ExecutionWorkItem>(100);
            check_and_enqueue(lifecycle, svc, &tx, chrono::Local::now()).await;
            let mut items = Vec::new();
            while let Ok(item) = rx.try_recv() {
                items.push(item);
            }
            items.sort_by(|a, b| a.trigger_node.id.cmp(&b.trigger_node.id));
            items
        }

        fn scanned_ids(items: &[ExecutionWorkItem]) -> Vec<&str> {
            items.iter().map(|w| w.trigger_node.id.as_str()).collect()
        }

        fn open_filter() -> serde_json::Value {
            json!([{ "type": "property", "operator": "equals", "property": "state", "value": "open" }])
        }

        /// A bare type selects every participating node of the type and its
        /// subtypes; archived nodes never match.
        #[tokio::test]
        async fn a_type_selector_selects_the_type_its_subtypes_and_no_archived_node() {
            let (svc, _tmp) = create_test_service().await;
            seed_tickets(&svc).await;
            let lifecycle = lifecycle_with_rules(scheduled_rule(
                json!({ "target_type": "sel-ticket" }),
                json!([]),
            ));

            let items = scan(&svc, &lifecycle).await;
            assert_eq!(scanned_ids(&items), [OPEN_TICKET, OPEN_BUG, CLOSED_TICKET]);
        }

        /// An inline selector's filters run in SQL: the scan enqueues only
        /// the nodes they match, instead of every node of the type for CEL
        /// to discard one at a time.
        #[tokio::test]
        async fn an_inline_selector_filters_in_the_query() {
            let (svc, _tmp) = create_test_service().await;
            seed_tickets(&svc).await;
            let lifecycle = lifecycle_with_rules(scheduled_rule(
                json!({ "target_type": "sel-ticket", "filters": open_filter() }),
                json!([]),
            ));

            let items = scan(&svc, &lifecycle).await;
            assert_eq!(scanned_ids(&items), [OPEN_TICKET, OPEN_BUG]);
            for item in &items {
                let scan = item
                    .scan
                    .as_ref()
                    .expect("a scheduled work item carries its scan");
                assert_eq!(scan.target_type, "sel-ticket");
            }
        }

        /// A selector may name a saved query node: the scan selects what that
        /// query's type and filters select when the schedule fires, so an
        /// edit to the query takes effect at the next scan. The query's
        /// `limit` belongs to its viewer and does not cap the scan.
        #[tokio::test]
        async fn a_selector_can_reference_a_saved_query() {
            let (svc, _tmp) = create_test_service().await;
            seed_tickets(&svc).await;
            let query_id = "c2000000-0000-4000-8000-000000000001";
            svc.create_node(Node::new_with_id(
                query_id.to_string(),
                "query".to_string(),
                "Open tickets".to_string(),
                json!({ "target_type": "sel-ticket", "filters": open_filter(), "limit": 1 }),
            ))
            .await
            .unwrap();
            let lifecycle =
                lifecycle_with_rules(scheduled_rule(json!({ "query_id": query_id }), json!([])));

            let items = scan(&svc, &lifecycle).await;
            assert_eq!(scanned_ids(&items), [OPEN_TICKET, OPEN_BUG]);
            // The rule names no type of its own; the scan read it off the query.
            assert_eq!(items[0].scan.as_ref().unwrap().target_type, "sel-ticket");
            assert_eq!(items[0].rules[0].rule.trigger.registered_type(), None);

            // Repoint the saved query; the same play now scans the other set.
            let query = svc.get_node(query_id).await.unwrap().unwrap();
            let update: crate::models::QueryNodeUpdate = serde_json::from_value(json!({
                "filters": [{ "type": "property", "operator": "equals", "property": "state", "value": "closed" }]
            }))
            .unwrap();
            svc.update_query_node(query_id, query.version, update)
                .await
                .unwrap();

            let items = scan(&svc, &lifecycle).await;
            assert_eq!(scanned_ids(&items), [CLOSED_TICKET]);
        }

        /// A selector takes the negated filter forms, written into the rule
        /// or read off a saved query: "not open", and "has no child that is
        /// open".
        #[tokio::test]
        async fn a_selector_accepts_negated_filters() {
            let (svc, _tmp) = create_test_service().await;
            seed_tickets(&svc).await;
            // The open ticket is the parent of an open bug, and the closed
            // ticket of a closed one. A bug keeps its inherited `state` in
            // the `sel-ticket` bucket, so each child's state has to be read
            // there for the two parents to come apart.
            const CLOSED_BUG: &str = "c1000000-0000-4000-8000-000000000005";
            svc.create_node(Node::new_with_id(
                CLOSED_BUG.to_string(),
                "sel-bug".to_string(),
                CLOSED_BUG.to_string(),
                json!({ "state": "closed" }),
            ))
            .await
            .unwrap();
            svc.create_relationship(OPEN_TICKET, "has_child", OPEN_BUG, json!({}))
                .await
                .unwrap();
            svc.create_relationship(CLOSED_TICKET, "has_child", CLOSED_BUG, json!({}))
                .await
                .unwrap();

            let not_open = json!([{
                "type": "property", "operator": "equals", "property": "state",
                "value": "open", "negate": true
            }]);
            let inline = lifecycle_with_rules(scheduled_rule(
                json!({ "target_type": "sel-ticket", "filters": not_open }),
                json!([]),
            ));
            assert_eq!(
                scanned_ids(&scan(&svc, &inline).await),
                [CLOSED_TICKET, CLOSED_BUG]
            );

            let query_id = "c2000000-0000-4000-8000-000000000002";
            svc.create_node(Node::new_with_id(
                query_id.to_string(),
                "query".to_string(),
                "No open child".to_string(),
                json!({ "target_type": "sel-ticket", "filters": [{
                    "type": "related", "operator": "exists", "path": ["has_child"], "negate": true,
                    "filter": {
                        "type": "property", "operator": "equals", "property": "state",
                        "value": "closed", "negate": true
                    }
                }] }),
            ))
            .await
            .unwrap();
            let saved =
                lifecycle_with_rules(scheduled_rule(json!({ "query_id": query_id }), json!([])));
            // Every ticket but the one whose child is open: the bugs have no
            // children, and the closed ticket's only child is closed.
            assert_eq!(
                scanned_ids(&scan(&svc, &saved).await),
                [OPEN_BUG, CLOSED_TICKET, CLOSED_BUG]
            );
        }

        /// A selector that cannot run selects nothing and enqueues nothing;
        /// the scan carries on with the other entries.
        #[tokio::test]
        async fn a_selector_naming_a_missing_query_enqueues_nothing() {
            let (svc, _tmp) = create_test_service().await;
            seed_tickets(&svc).await;
            let lifecycle = lifecycle_with_rules(scheduled_rule(
                json!({ "query_id": "c2000000-0000-4000-8000-00000000dead" }),
                json!([]),
            ));

            assert!(scan(&svc, &lifecycle).await.is_empty());
        }

        /// The scan resolves its rules' condition paths for every selected
        /// node before enqueuing, and each work item carries its own node's
        /// answers.
        #[tokio::test]
        async fn a_scan_resolves_condition_paths_for_every_selected_node() {
            use crate::playbook::graph_resolver::ResolvedValue;

            let (svc, _tmp) = create_test_service().await;
            seed_tickets(&svc).await;
            // Each open ticket gets a parent whose content names the ticket.
            for (parent, child) in [
                ("c3000000-0000-4000-8000-000000000001", OPEN_TICKET),
                ("c3000000-0000-4000-8000-000000000002", OPEN_BUG),
            ] {
                svc.create_node(Node::new_with_id(
                    parent.to_string(),
                    "text".to_string(),
                    format!("parent of {child}"),
                    json!({}),
                ))
                .await
                .unwrap();
                svc.create_relationship(parent, "has_child", child, json!({}))
                    .await
                    .unwrap();
            }
            let lifecycle = lifecycle_with_rules(scheduled_rule(
                json!({ "target_type": "sel-ticket", "filters": open_filter() }),
                json!([{ "expr": "node.child_of.content != ''", "description": "Test condition" }]),
            ));

            let items = scan(&svc, &lifecycle).await;
            assert_eq!(scanned_ids(&items), [OPEN_TICKET, OPEN_BUG]);
            let path = vec!["child_of".to_string(), "content".to_string()];
            for item in &items {
                let scan = item.scan.as_ref().unwrap();
                let own = scan
                    .paths
                    .get(&item.trigger_node.id)
                    .and_then(|paths| paths.get(&path));
                assert!(
                    matches!(own, Some(ResolvedValue::Scalar(v))
                        if v == &json!(format!("parent of {}", item.trigger_node.id))),
                    "{}: {own:?}",
                    item.trigger_node.id
                );
            }
        }
    }
}
