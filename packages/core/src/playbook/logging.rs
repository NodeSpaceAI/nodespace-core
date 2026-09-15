//! Play Engine Logging — Log Node Creation and Error Deduplication
//!
//! When the play engine encounters errors (cycle limits, type mismatches,
//! missing paths, etc.), it creates `playbook_log` nodes to make errors visible
//! in the node graph. Repeated errors with the same structural cause are
//! deduplicated via SHA-256 fingerprinting: `occurrences` is incremented and
//! `last_seen` is updated rather than creating a new log node.
//!
//! # Error Fingerprint
//!
//! `hash(play_id, rule_name, error_location_index, error_type)`
//!
//! Dynamic values (node IDs, timestamps) are excluded so that the same
//! structural error always produces the same fingerprint.

use crate::models::Node;
use crate::services::NodeService;
use chrono::Utc;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::Arc;
use tracing::{debug, warn};

/// Maximum depth for play execution chains.
///
/// When `depth + 1 > MAX_CHAIN_DEPTH`, the engine stops processing,
/// disables the offending play, and creates a log node.
pub const MAX_CHAIN_DEPTH: u8 = 10;

/// Error types for log node fingerprinting.
///
/// Used as part of the fingerprint hash to distinguish structurally
/// different error categories for the same play/rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayErrorType {
    /// Execution chain exceeded MAX_CHAIN_DEPTH
    CycleLimit,
    /// A property path referenced in a condition/action does not exist
    MissingPath,
    /// A value has an incompatible type for the operation
    TypeMismatch,
    /// Schema version drift detected (play compiled against older schema)
    VersionConflict,
    /// CEL condition failed to compile
    CompileError,
    /// Action execution failed
    ActionError,
    /// A seeded play carrying an invariant rule was edited or disabled
    /// (ADR-060 §8) — advisory, not a fault in the play itself.
    SeededPlayWarning,
}

impl fmt::Display for PlayErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CycleLimit => write!(f, "cycle_limit"),
            Self::MissingPath => write!(f, "missing_path"),
            Self::TypeMismatch => write!(f, "type_mismatch"),
            Self::VersionConflict => write!(f, "version_conflict"),
            Self::CompileError => write!(f, "compile_error"),
            Self::ActionError => write!(f, "action_error"),
            Self::SeededPlayWarning => write!(f, "seeded_play_warning"),
        }
    }
}

/// Compute a SHA-256 fingerprint for error deduplication.
///
/// The fingerprint is derived from structural identifiers only — dynamic
/// values like node IDs and timestamps are excluded so that repeated
/// occurrences of the same structural error produce the same hash.
///
/// # Arguments
///
/// * `play_id` - The play that encountered the error
/// * `rule_name` - The rule within the play
/// * `error_location_index` - Condition index or action index where the error occurred
/// * `error_type` - The category of error
///
/// # Returns
///
/// A hex-encoded SHA-256 hash string (64 characters).
pub fn error_fingerprint(
    play_id: &str,
    rule_name: &str,
    error_location_index: usize,
    error_type: &PlayErrorType,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(play_id.as_bytes());
    hasher.update(b"|");
    hasher.update(rule_name.as_bytes());
    hasher.update(b"|");
    hasher.update(error_location_index.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(error_type.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Create or update a log node for a play error.
///
/// Uses fingerprint-based deduplication: if a log node with the same
/// fingerprint already exists, its `occurrences` count is incremented
/// and `last_seen` is updated. Otherwise, a new `playbook_log` node
/// is created.
///
/// # Arguments
///
/// * `node_service` - NodeService for creating/querying/updating nodes
/// * `play_id` - The play that encountered the error
/// * `rule_name` - The rule within the play
/// * `error_location_index` - Condition or action index where the error occurred
/// * `error_type` - The category of error
/// * `error_message` - Human-readable error description
/// * `trigger_node_id` - The node that triggered the rule (for context)
pub async fn create_or_update_log_node(
    node_service: &Arc<NodeService>,
    play_id: &str,
    rule_name: &str,
    error_location_index: usize,
    error_type: PlayErrorType,
    error_message: &str,
    trigger_node_id: &str,
) -> anyhow::Result<()> {
    let fingerprint = error_fingerprint(play_id, rule_name, error_location_index, &error_type);
    let now = Utc::now().to_rfc3339();

    // Query existing playbook_log nodes and search for matching fingerprint.
    // NodeService doesn't support property-level queries, so we filter in memory.
    // Acceptable for desktop (low log node counts).
    let existing_logs = node_service
        .query_nodes_by_type("playbook_log", Some("active"))
        .await?;

    let existing = existing_logs.iter().find(|n| {
        // Check both flat and namespaced property formats.
        // NodeService normalizes flat properties under the node_type namespace
        // on storage, so queried nodes have {"playbook_log": {"error_fingerprint": "..."}}.
        let fp = n
            .properties
            .get("error_fingerprint")
            .and_then(|v| v.as_str())
            .or_else(|| {
                n.properties
                    .get("playbook_log")
                    .and_then(|ns| ns.get("error_fingerprint"))
                    .and_then(|v| v.as_str())
            });
        fp == Some(&fingerprint)
    });

    if let Some(log_node) = existing {
        // Deduplicate: increment occurrences and update last_seen
        let current_occurrences = log_node
            .properties
            .get("occurrences")
            .and_then(|v| v.as_u64())
            .or_else(|| {
                log_node
                    .properties
                    .get("playbook_log")
                    .and_then(|ns| ns.get("occurrences"))
                    .and_then(|v| v.as_u64())
            })
            .unwrap_or(1);

        // Update properties within the namespace if present, otherwise flat.
        // Queried nodes have namespaced format: {"playbook_log": {...}}.
        let mut new_properties = log_node.properties.clone();
        if let Some(ns) = new_properties
            .get_mut("playbook_log")
            .and_then(|v| v.as_object_mut())
        {
            ns.insert("occurrences".to_string(), json!(current_occurrences + 1));
            ns.insert("last_seen".to_string(), json!(now));
            ns.insert("trigger_node_id".to_string(), json!(trigger_node_id));
        } else {
            new_properties["occurrences"] = json!(current_occurrences + 1);
            new_properties["last_seen"] = json!(now);
            new_properties["trigger_node_id"] = json!(trigger_node_id);
        }

        let update = crate::models::NodeUpdate::new().with_properties(new_properties);

        if let Err(e) = node_service
            .update_node(&log_node.id, log_node.version, update)
            .await
        {
            warn!(
                "Failed to update log node {} (fingerprint {}): {}",
                log_node.id, fingerprint, e
            );
        } else {
            debug!(
                "Updated log node {} — occurrences now {}",
                log_node.id,
                current_occurrences + 1
            );
        }
    } else {
        // Create new log node
        let log_node = Node::new(
            "playbook_log".to_string(),
            error_message.to_string(),
            json!({
                "play_id": play_id,
                "rule_name": rule_name,
                "error_type": error_type.to_string(),
                "error_location_index": error_location_index,
                "error_fingerprint": fingerprint,
                "trigger_node_id": trigger_node_id,
                "occurrences": 1,
                "first_seen": now,
                "last_seen": now,
            }),
        );

        match node_service.create_node(log_node).await {
            Ok(id) => {
                debug!(
                    "Created log node {} for play {} error (fingerprint {})",
                    id, play_id, fingerprint
                );
            }
            Err(e) => {
                warn!(
                    "Failed to create log node for play {} error: {}",
                    play_id, e
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Repair-and-log (ADR-060 §7)
// ---------------------------------------------------------------------------
//
// A device can receive an already-committed node that violates an invariant
// it holds (the originating device ran an older play version, had the rule
// disabled, or predates the rule). The node cannot be un-committed, so the
// resolution is repair-and-log: apply the invariant's effect and record what
// was repaired and why.
//
// Unlike `create_or_update_log_node`'s fingerprint (an app-level scan over
// this device's own `playbook_log` nodes -- correct for a single device
// logging its own execution errors, since only that device ever writes its
// own fingerprint), a repair can legitimately be performed independently by
// several devices that each received the same violating node via sync and
// each hold the same invariant rule. Scanning this device's own nodes cannot
// prevent that from producing N separately-created log rows that then all
// sync to everyone. Instead the repair log node's *id itself* is derived
// deterministically from `(play_id, rule_name, trigger_node_id)` -- the same
// technique `deterministic_action_output_id` uses for reactive action
// outputs (ADR-060 §3) -- so two devices repairing the same violation
// compute the SAME id and their writes converge to one row via ordinary
// sync upsert, without either device needing to know about the other.

/// Stable namespace for playbook repair-log node ids (UUIDv5). Fixed and
/// arbitrary -- do not change it, for the same reason
/// `actions::ACTION_OUTPUT_ID_NAMESPACE` must not change: doing so would mint
/// a fresh id for every existing repair log on its next occurrence, breaking
/// the cross-device convergence this exists for. Distinct from every other
/// UUIDv5 namespace in this codebase so the three id spaces never collide.
const REPAIR_LOG_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x1a7c9e3f_5d2b_4a6e_8f1c_3b9d7e2a5c4fu128);

/// Derive a repair-log node's id from the violation it records. See the
/// module section doc above for why this must be deterministic rather than
/// randomly generated.
pub fn deterministic_repair_log_id(
    play_id: &str,
    rule_name: &str,
    trigger_node_id: &str,
) -> String {
    let seed = format!("{play_id}\u{1}{rule_name}\u{1}{trigger_node_id}");
    uuid::Uuid::new_v5(&REPAIR_LOG_ID_NAMESPACE, seed.as_bytes()).to_string()
}

/// Create or update a `playbook_log` node recording an invariant repair
/// (ADR-060 §7).
///
/// Idempotent by construction: `deterministic_repair_log_id` always derives
/// the same id for the same `(play_id, rule_name, trigger_node_id)`, so a
/// second call for the same violation -- whether from this device
/// re-processing a redelivered event, or a row that already synced in from
/// another device's independent repair of the same violation -- updates the
/// existing row's `occurrences`/`last_seen` in place instead of creating a
/// second log node.
pub async fn create_or_update_repair_log_node(
    node_service: &Arc<NodeService>,
    play_id: &str,
    rule_name: &str,
    trigger_node_id: &str,
    message: &str,
) -> anyhow::Result<()> {
    let id = deterministic_repair_log_id(play_id, rule_name, trigger_node_id);
    let now = Utc::now().to_rfc3339();

    match node_service.get_node(&id).await? {
        Some(existing) => {
            let current_occurrences = existing
                .properties
                .get("occurrences")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    existing
                        .properties
                        .get("playbook_log")
                        .and_then(|ns| ns.get("occurrences"))
                        .and_then(|v| v.as_u64())
                })
                .unwrap_or(1);

            let mut new_properties = existing.properties.clone();
            if let Some(ns) = new_properties
                .get_mut("playbook_log")
                .and_then(|v| v.as_object_mut())
            {
                ns.insert("occurrences".to_string(), json!(current_occurrences + 1));
                ns.insert("last_seen".to_string(), json!(now));
            } else {
                new_properties["occurrences"] = json!(current_occurrences + 1);
                new_properties["last_seen"] = json!(now);
            }

            let update = crate::models::NodeUpdate::new().with_properties(new_properties);
            if let Err(e) = node_service
                .update_node(&id, existing.version, update)
                .await
            {
                warn!(
                    "Failed to update repair log node {} for play {} rule '{}': {}",
                    id, play_id, rule_name, e
                );
            } else {
                debug!(
                    "Updated repair log node {} — occurrences now {}",
                    id,
                    current_occurrences + 1
                );
            }
        }
        None => {
            let log_node = Node::new_with_id(
                id.clone(),
                "playbook_log".to_string(),
                message.to_string(),
                json!({
                    "play_id": play_id,
                    "rule_name": rule_name,
                    "kind": "repair",
                    "trigger_node_id": trigger_node_id,
                    "occurrences": 1,
                    "first_seen": now,
                    "last_seen": now,
                }),
            );

            match node_service.create_node(log_node).await {
                Ok(created_id) => {
                    debug!(
                        "Created repair log node {} for play {} rule '{}'",
                        created_id, play_id, rule_name
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to create repair log node for play {} rule '{}': {}",
                        play_id, rule_name, e
                    );
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // error_fingerprint — pure unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn fingerprint_consistent_for_same_inputs() {
        let fp1 = error_fingerprint("pb-1", "rule-a", 0, &PlayErrorType::CycleLimit);
        let fp2 = error_fingerprint("pb-1", "rule-a", 0, &PlayErrorType::CycleLimit);
        assert_eq!(
            fp1, fp2,
            "same inputs should produce identical fingerprints"
        );
        // SHA-256 hex is 64 chars
        assert_eq!(fp1.len(), 64);
    }

    #[test]
    fn fingerprint_differs_for_different_inputs() {
        let fp1 = error_fingerprint("pb-1", "rule-a", 0, &PlayErrorType::CycleLimit);
        let fp2 = error_fingerprint("pb-2", "rule-a", 0, &PlayErrorType::CycleLimit);
        let fp3 = error_fingerprint("pb-1", "rule-b", 0, &PlayErrorType::CycleLimit);
        let fp4 = error_fingerprint("pb-1", "rule-a", 1, &PlayErrorType::CycleLimit);
        assert_ne!(fp1, fp2, "different play_id should differ");
        assert_ne!(fp1, fp3, "different rule_name should differ");
        assert_ne!(fp1, fp4, "different error_location_index should differ");
    }

    #[test]
    fn fingerprint_differs_for_different_error_types() {
        let fp_cycle = error_fingerprint("pb-1", "rule-a", 0, &PlayErrorType::CycleLimit);
        let fp_missing = error_fingerprint("pb-1", "rule-a", 0, &PlayErrorType::MissingPath);
        let fp_type = error_fingerprint("pb-1", "rule-a", 0, &PlayErrorType::TypeMismatch);
        let fp_compile = error_fingerprint("pb-1", "rule-a", 0, &PlayErrorType::CompileError);
        assert_ne!(fp_cycle, fp_missing);
        assert_ne!(fp_cycle, fp_type);
        assert_ne!(fp_cycle, fp_compile);
        assert_ne!(fp_missing, fp_type);
    }

    // -----------------------------------------------------------------------
    // Integration tests — create_or_update_log_node with real NodeService
    // -----------------------------------------------------------------------

    mod integration {
        use super::*;
        use crate::db::SqliteStore;
        use crate::services::NodeService;
        use std::sync::Arc;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        #[tokio::test]
        async fn create_log_node_creates_new_node() {
            let (svc, _tmp) = create_test_service().await;

            // Create a schema for playbook_log so NodeService accepts it
            let schema = crate::models::Node::new_with_id(
                "playbook_log".to_string(),
                "schema".to_string(),
                "playbook_log".to_string(),
                serde_json::json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "play log schema",
                    "fields": [],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            create_or_update_log_node(
                &svc,
                "pb-1",
                "rule-a",
                0,
                PlayErrorType::CycleLimit,
                "Cycle limit exceeded",
                "trigger-node-1",
            )
            .await
            .unwrap();

            // Verify a playbook_log node was created
            let logs = svc
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
                .unwrap();
            assert_eq!(logs.len(), 1);
            // Properties are stored under the "playbook_log" namespace by NodeService
            let props = &logs[0].properties["playbook_log"];
            assert_eq!(props["occurrences"], 1);
            assert_eq!(props["error_type"], "cycle_limit");
            assert_eq!(props["play_id"], "pb-1");
            assert_eq!(props["rule_name"], "rule-a");
            assert_eq!(props["trigger_node_id"], "trigger-node-1");
        }

        #[tokio::test]
        async fn create_log_node_deduplicates_same_fingerprint() {
            let (svc, _tmp) = create_test_service().await;

            let schema = crate::models::Node::new_with_id(
                "playbook_log".to_string(),
                "schema".to_string(),
                "playbook_log".to_string(),
                serde_json::json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "play log schema",
                    "fields": [],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            // First call — creates node
            create_or_update_log_node(
                &svc,
                "pb-1",
                "rule-a",
                0,
                PlayErrorType::MissingPath,
                "Path not found",
                "trigger-node-1",
            )
            .await
            .unwrap();

            // Second call with same structural params — should deduplicate
            create_or_update_log_node(
                &svc,
                "pb-1",
                "rule-a",
                0,
                PlayErrorType::MissingPath,
                "Path not found (again)",
                "trigger-node-2",
            )
            .await
            .unwrap();

            let logs = svc
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
                .unwrap();
            assert_eq!(
                logs.len(),
                1,
                "should still have only 1 log node after dedup"
            );
        }

        #[tokio::test]
        async fn create_log_node_increments_occurrences_on_dedup() {
            let (svc, _tmp) = create_test_service().await;

            let schema = crate::models::Node::new_with_id(
                "playbook_log".to_string(),
                "schema".to_string(),
                "playbook_log".to_string(),
                serde_json::json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "play log schema",
                    "fields": [],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            // Create initial log
            create_or_update_log_node(
                &svc,
                "pb-1",
                "rule-a",
                0,
                PlayErrorType::ActionError,
                "Action failed",
                "trigger-1",
            )
            .await
            .unwrap();

            // Dedup — occurrences should go to 2
            create_or_update_log_node(
                &svc,
                "pb-1",
                "rule-a",
                0,
                PlayErrorType::ActionError,
                "Action failed again",
                "trigger-2",
            )
            .await
            .unwrap();

            // Dedup again — occurrences should go to 3
            create_or_update_log_node(
                &svc,
                "pb-1",
                "rule-a",
                0,
                PlayErrorType::ActionError,
                "Action failed yet again",
                "trigger-3",
            )
            .await
            .unwrap();

            let logs = svc
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
                .unwrap();
            assert_eq!(logs.len(), 1);
            let props = &logs[0].properties["playbook_log"];
            assert_eq!(
                props["occurrences"], 3,
                "occurrences should be 3 after initial + 2 dedup calls"
            );
            // Latest trigger_node_id should be updated
            assert_eq!(props["trigger_node_id"], "trigger-3");
        }

        // -------------------------------------------------------------------
        // Repair-and-log (ADR-060 §7) — deterministic id + idempotency
        // -------------------------------------------------------------------

        async fn create_playbook_log_schema(svc: &NodeService) {
            let schema = crate::models::Node::new_with_id(
                "playbook_log".to_string(),
                "schema".to_string(),
                "playbook_log".to_string(),
                serde_json::json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": "play log schema",
                    "fields": [],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();
        }

        #[test]
        fn repair_log_id_is_deterministic_and_key_sensitive() {
            let a = deterministic_repair_log_id("pb-1", "rule-a", "node-1");
            let b = deterministic_repair_log_id("pb-1", "rule-a", "node-1");
            assert_eq!(a, b, "same key must derive the same id every time");

            let different_play = deterministic_repair_log_id("pb-2", "rule-a", "node-1");
            let different_rule = deterministic_repair_log_id("pb-1", "rule-b", "node-1");
            let different_node = deterministic_repair_log_id("pb-1", "rule-a", "node-2");
            assert_ne!(a, different_play);
            assert_ne!(a, different_rule);
            assert_ne!(a, different_node);
        }

        /// Direct test of the convergence property ADR-060 §7 requires:
        /// several devices independently repairing the SAME violation must
        /// not each mint their own log node. Since real cross-device
        /// concurrency can't be constructed in one process, this calls the
        /// function twice with the identical `(play_id, rule_name,
        /// trigger_node_id)` key — exactly what two devices computing the
        /// same deterministic id independently amounts to from either
        /// device's own local perspective — and asserts one row, not two.
        #[tokio::test]
        async fn repeated_repair_of_the_same_violation_converges_to_one_log_node() {
            let (svc, _tmp) = create_test_service().await;
            create_playbook_log_schema(&svc).await;

            create_or_update_repair_log_node(
                &svc,
                "pb-privacy",
                "default-private",
                "node-A",
                "Repaired invariant 'default-private' (play pb-privacy) on node received via sync",
            )
            .await
            .unwrap();

            // Simulates a second device (or a redelivered event on this same
            // device) independently repairing the identical violation.
            create_or_update_repair_log_node(
                &svc,
                "pb-privacy",
                "default-private",
                "node-A",
                "Repaired invariant 'default-private' (play pb-privacy) on node received via sync",
            )
            .await
            .unwrap();

            let logs = svc
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
                .unwrap();
            assert_eq!(
                logs.len(),
                1,
                "two repairs of the same violation must converge to ONE log node, got {:?}",
                logs.iter().map(|n| &n.id).collect::<Vec<_>>()
            );
            let props = &logs[0].properties["playbook_log"];
            assert_eq!(
                props["occurrences"], 2,
                "occurrences must reflect both repair calls"
            );
            assert_eq!(props["kind"], "repair");

            let expected_id =
                deterministic_repair_log_id("pb-privacy", "default-private", "node-A");
            assert_eq!(
                logs[0].id, expected_id,
                "the log node's id must be the deterministic id, not a random one"
            );
        }

        #[tokio::test]
        async fn repairs_of_different_violations_stay_as_separate_log_nodes() {
            let (svc, _tmp) = create_test_service().await;
            create_playbook_log_schema(&svc).await;

            create_or_update_repair_log_node(
                &svc,
                "pb-privacy",
                "default-private",
                "node-A",
                "repaired A",
            )
            .await
            .unwrap();
            create_or_update_repair_log_node(
                &svc,
                "pb-privacy",
                "default-private",
                "node-B",
                "repaired B",
            )
            .await
            .unwrap();

            let logs = svc
                .query_nodes_by_type("playbook_log", Some("active"))
                .await
                .unwrap();
            assert_eq!(
                logs.len(),
                2,
                "two distinct violations (different trigger nodes) must not collapse into one log node"
            );
        }
    }
}
