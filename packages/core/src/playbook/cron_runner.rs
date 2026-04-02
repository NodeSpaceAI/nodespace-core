//! CronRunner — 60-second polling loop for scheduled playbook triggers.
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
//!   same `(cron_expression, node_type)` pair, so only one DB query is issued
//!   per unique pair.
//! - **Missed runs are skipped.** If NodeSpace was not running when a cron
//!   expression was due, execution is not retried.
//! - **Dynamic registration.** The registry is re-read on each wake, so
//!   playbook activations/deactivations take effect within 60 seconds.
//! - **Cron expressions use 7-field format** (sec min hour dom month dow year)
//!   as required by the `cron` crate. Example: `"0 * * * * * *"` fires every
//!   minute at second 0.

use crate::db::events::{DomainEvent, EventEnvelope, EventMetadata};
use crate::playbook::lifecycle::PlaybookLifecycleManager;
use crate::playbook::types::{CronEntry, ExecutionWorkItem};
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
/// cron expression matches the current minute window, queries all active nodes
/// of that type and enqueues work items.
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

    loop {
        tokio::select! {
            () = tokio::time::sleep(POLL_INTERVAL) => {
                check_and_enqueue(&lifecycle, &node_service, &queue_tx).await;
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

/// Check all cron entries and enqueue work items for matching nodes.
pub(crate) async fn check_and_enqueue(
    lifecycle: &Arc<RwLock<PlaybookLifecycleManager>>,
    node_service: &Arc<NodeService>,
    queue_tx: &mpsc::Sender<ExecutionWorkItem>,
) {
    let now = chrono::Local::now();

    // Read cron registry (short read lock, then release)
    let entries: Vec<CronEntry> = {
        let guard = lifecycle.read().expect("lifecycle lock poisoned");
        guard.cron_registry().clone()
    };

    if entries.is_empty() {
        return;
    }

    debug!("CronRunner checking {} cron entries", entries.len());

    for entry in &entries {
        let schedule: Schedule = match Schedule::from_str(&entry.cron_expression) {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    "Invalid cron expression '{}' for node_type '{}': {}",
                    entry.cron_expression, entry.node_type, e
                );
                continue;
            }
        };

        // Check if the cron expression fired in the last 60-second window.
        // We look backward from `now` by the poll interval — if there's an
        // occurrence between (now - 60s) and now, the expression matches.
        let window_start = now - chrono::Duration::seconds(POLL_INTERVAL.as_secs() as i64);
        let matches = schedule.after(&window_start).take(1).any(|t| t <= now);

        if !matches {
            continue;
        }

        debug!(
            "Cron expression '{}' matched for node_type '{}'",
            entry.cron_expression, entry.node_type
        );

        // Query all active nodes of this type (single DB scan per cron+node_type pair)
        let nodes = match node_service
            .query_nodes_by_type(&entry.node_type, Some("active"))
            .await
        {
            Ok(nodes) => nodes,
            Err(e) => {
                error!(
                    "Failed to query nodes of type '{}' for cron trigger: {}",
                    entry.node_type, e
                );
                continue;
            }
        };

        debug!(
            "Cron trigger: found {} active '{}' nodes, enqueueing with {} rules",
            nodes.len(),
            entry.node_type,
            entry.rules.len()
        );

        // Enqueue each node with all matching rules
        for node in nodes {
            let work_item = ExecutionWorkItem {
                rules: entry.rules.clone(),
                trigger_event: synthetic_cron_envelope(&node.id),
                trigger_node: node,
            };

            if let Err(e) = queue_tx.try_send(work_item) {
                match e {
                    mpsc::error::TrySendError::Full(_) => {
                        warn!(
                            "ExecutionQueue full, dropping cron work item for node_type '{}'",
                            entry.node_type
                        );
                    }
                    mpsc::error::TrySendError::Closed(_) => {
                        debug!("ExecutionQueue closed, CronRunner stopping enqueue");
                        return;
                    }
                }
            }
        }
    }
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
        ActionType, CronEntry, OrderedRuleRef, ParsedAction, ParsedRule, ParsedTrigger,
    };
    use chrono::Utc;
    use std::sync::Arc;

    /// Helper: create a CronEntry with the given expression, node_type, and a single rule.
    fn make_cron_entry(cron_expr: &str, node_type: &str) -> CronEntry {
        let rule = Arc::new(ParsedRule {
            name: "test-cron-rule".to_string(),
            trigger: ParsedTrigger::Scheduled {
                cron: cron_expr.to_string(),
                node_type: node_type.to_string(),
            },
            conditions: vec![],
            actions: vec![ParsedAction {
                action_type: ActionType::UpdateNode,
                params: serde_json::json!({"target": "trigger.node"}),
                for_each: None,
            }],
        });

        CronEntry {
            cron_expression: cron_expr.to_string(),
            node_type: node_type.to_string(),
            rules: vec![OrderedRuleRef {
                playbook_id: "playbook-1".to_string(),
                playbook_created_at: Utc::now(),
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
        let matches = schedule.after(&window_start).take(1).any(|t| t <= now);
        assert!(matches, "every-minute cron should match within 60s window");
    }

    #[test]
    fn test_cron_expression_far_future_does_not_match() {
        // "0 0 0 29 2 * 2099" = midnight Feb 29, 2099 (far future)
        let schedule = cron::Schedule::from_str("0 0 0 29 2 * 2099").unwrap();
        let now = chrono::Local::now();
        let window_start = now - chrono::Duration::seconds(60);

        let matches = schedule.after(&window_start).take(1).any(|t| t <= now);
        assert!(
            !matches,
            "far-future cron expression should not match current window"
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
            trigger: ParsedTrigger::Scheduled {
                cron: "0 * * * * * *".to_string(),
                node_type: "task".to_string(),
            },
            conditions: vec![],
            actions: vec![],
        });

        let rule_b = Arc::new(ParsedRule {
            name: "rule-b".to_string(),
            trigger: ParsedTrigger::Scheduled {
                cron: "0 * * * * * *".to_string(),
                node_type: "task".to_string(),
            },
            conditions: vec![],
            actions: vec![],
        });

        let entry = CronEntry {
            cron_expression: "0 * * * * * *".to_string(),
            node_type: "task".to_string(),
            rules: vec![
                OrderedRuleRef {
                    playbook_id: "pb-1".to_string(),
                    playbook_created_at: Utc::now(),
                    rule_index: 0,
                    rule: rule_a,
                },
                OrderedRuleRef {
                    playbook_id: "pb-2".to_string(),
                    playbook_created_at: Utc::now(),
                    rule_index: 0,
                    rule: rule_b,
                },
            ],
        };

        // One CronEntry → one DB query, but two rules applied per node
        assert_eq!(entry.rules.len(), 2);
        assert_eq!(entry.cron_expression, "0 * * * * * *");
        assert_eq!(entry.node_type, "task");
    }

    #[test]
    fn test_make_cron_entry_helper() {
        let entry = make_cron_entry("0 30 9 * * * *", "invoice");
        assert_eq!(entry.cron_expression, "0 30 9 * * * *");
        assert_eq!(entry.node_type, "invoice");
        assert_eq!(entry.rules.len(), 1);
        assert_eq!(entry.rules[0].playbook_id, "playbook-1");
    }

    // -----------------------------------------------------------------------
    // Integration tests — check_and_enqueue with real NodeService + lifecycle
    // -----------------------------------------------------------------------

    mod integration {
        use super::*;
        use crate::db::SurrealStore;
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
            let mut store: Arc<SurrealStore> = Arc::new(SurrealStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        fn make_lifecycle_with_cron(
            cron_expr: &str,
            node_type: &str,
        ) -> Arc<RwLock<PlaybookLifecycleManager>> {
            let mut lm = PlaybookLifecycleManager::new();

            // Manually create a playbook node with a scheduled trigger and activate it
            let playbook_node = Node {
                id: "pb-cron-1".to_string(),
                node_type: "playbook".to_string(),
                content: "cron playbook".to_string(),
                version: 1,
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                properties: json!({
                    "rules": [{
                        "name": "cron-rule-1",
                        "trigger": {
                            "type": "scheduled",
                            "cron": cron_expr,
                            "node_type": node_type
                        },
                        "conditions": [],
                        "actions": [{
                            "action_type": "update_node",
                            "params": {"target": "trigger.node"}
                        }]
                    }]
                }),
                mentions: vec![],
                mentioned_in: vec![],
                title: Some("Cron Playbook".to_string()),
                lifecycle_status: "active".to_string(),
            };
            lm.activate_playbook(&playbook_node).unwrap();

            Arc::new(RwLock::new(lm))
        }

        #[tokio::test]
        async fn check_and_enqueue_enqueues_for_matching_cron() {
            let (svc, _tmp) = create_test_service().await;

            // Create schema for the node type
            let schema = Node::new_with_id(
                "cr_task".to_string(),
                "schema".to_string(),
                "cr_task".to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "cr_task schema",
                    "fields": [{"name": "status", "type": "string"}],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            // Create active nodes of type "cr_task"
            let node1 = Node::new_with_id(
                "cr-t1".to_string(),
                "cr_task".to_string(),
                "task 1".to_string(),
                json!({"status": "open"}),
            );
            let node2 = Node::new_with_id(
                "cr-t2".to_string(),
                "cr_task".to_string(),
                "task 2".to_string(),
                json!({"status": "open"}),
            );
            svc.create_node(node1).await.unwrap();
            svc.create_node(node2).await.unwrap();

            // Use "every minute" cron — guaranteed to match in any 60s window
            let lifecycle = make_lifecycle_with_cron("0 * * * * * *", "cr_task");

            let (tx, mut rx) = mpsc::channel::<ExecutionWorkItem>(100);
            check_and_enqueue(&lifecycle, &svc, &tx).await;

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
            let ids: Vec<&str> = received.iter().map(|w| w.trigger_node.id.as_str()).collect();
            assert!(ids.contains(&"cr-t1"));
            assert!(ids.contains(&"cr-t2"));
        }

        #[tokio::test]
        async fn check_and_enqueue_skips_non_matching_cron() {
            let (svc, _tmp) = create_test_service().await;

            // Create schema
            let schema = Node::new_with_id(
                "cr_task2".to_string(),
                "schema".to_string(),
                "cr_task2".to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "cr_task2 schema",
                    "fields": [],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            let node = Node::new_with_id(
                "cr-t3".to_string(),
                "cr_task2".to_string(),
                "task 3".to_string(),
                json!({}),
            );
            svc.create_node(node).await.unwrap();

            // Far-future cron expression — should NOT match current window
            let lifecycle = make_lifecycle_with_cron("0 0 0 29 2 * 2099", "cr_task2");

            let (tx, mut rx) = mpsc::channel::<ExecutionWorkItem>(100);
            check_and_enqueue(&lifecycle, &svc, &tx).await;

            // Should not enqueue anything
            assert!(
                rx.try_recv().is_err(),
                "non-matching cron should not enqueue any work items"
            );
        }
    }
}
