//! Playbook Engine Logging — Log Node Creation and Error Deduplication
//!
//! When the playbook engine encounters errors (cycle limits, type mismatches,
//! missing paths, etc.), it creates `playbook_log` nodes to make errors visible
//! in the node graph. Repeated errors with the same structural cause are
//! deduplicated via SHA-256 fingerprinting: `occurrences` is incremented and
//! `last_seen` is updated rather than creating a new log node.
//!
//! # Error Fingerprint
//!
//! `hash(playbook_id, rule_name, error_location_index, error_type)`
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

/// Maximum depth for playbook execution chains.
///
/// When `depth + 1 > MAX_CHAIN_DEPTH`, the engine stops processing,
/// disables the offending playbook, and creates a log node.
pub const MAX_CHAIN_DEPTH: u8 = 10;

/// Error types for log node fingerprinting.
///
/// Used as part of the fingerprint hash to distinguish structurally
/// different error categories for the same playbook/rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybookErrorType {
    /// Execution chain exceeded MAX_CHAIN_DEPTH
    CycleLimit,
    /// A property path referenced in a condition/action does not exist
    MissingPath,
    /// A value has an incompatible type for the operation
    TypeMismatch,
    /// Schema version drift detected (playbook compiled against older schema)
    VersionConflict,
    /// CEL condition failed to compile
    CompileError,
    /// Action execution failed
    ActionError,
}

impl fmt::Display for PlaybookErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CycleLimit => write!(f, "cycle_limit"),
            Self::MissingPath => write!(f, "missing_path"),
            Self::TypeMismatch => write!(f, "type_mismatch"),
            Self::VersionConflict => write!(f, "version_conflict"),
            Self::CompileError => write!(f, "compile_error"),
            Self::ActionError => write!(f, "action_error"),
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
/// * `playbook_id` - The playbook that encountered the error
/// * `rule_name` - The rule within the playbook
/// * `error_location_index` - Condition index or action index where the error occurred
/// * `error_type` - The category of error
///
/// # Returns
///
/// A hex-encoded SHA-256 hash string (64 characters).
pub fn error_fingerprint(
    playbook_id: &str,
    rule_name: &str,
    error_location_index: usize,
    error_type: &PlaybookErrorType,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(playbook_id.as_bytes());
    hasher.update(b"|");
    hasher.update(rule_name.as_bytes());
    hasher.update(b"|");
    hasher.update(error_location_index.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(error_type.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Create or update a log node for a playbook error.
///
/// Uses fingerprint-based deduplication: if a log node with the same
/// fingerprint already exists, its `occurrences` count is incremented
/// and `last_seen` is updated. Otherwise, a new `playbook_log` node
/// is created.
///
/// # Arguments
///
/// * `node_service` - NodeService for creating/querying/updating nodes
/// * `playbook_id` - The playbook that encountered the error
/// * `rule_name` - The rule within the playbook
/// * `error_location_index` - Condition or action index where the error occurred
/// * `error_type` - The category of error
/// * `error_message` - Human-readable error description
/// * `trigger_node_id` - The node that triggered the rule (for context)
pub async fn create_or_update_log_node(
    node_service: &Arc<NodeService>,
    playbook_id: &str,
    rule_name: &str,
    error_location_index: usize,
    error_type: PlaybookErrorType,
    error_message: &str,
    trigger_node_id: &str,
) -> anyhow::Result<()> {
    let fingerprint = error_fingerprint(playbook_id, rule_name, error_location_index, &error_type);
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
                "playbook_id": playbook_id,
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
                    "Created log node {} for playbook {} error (fingerprint {})",
                    id, playbook_id, fingerprint
                );
            }
            Err(e) => {
                warn!(
                    "Failed to create log node for playbook {} error: {}",
                    playbook_id, e
                );
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
        let fp1 = error_fingerprint("pb-1", "rule-a", 0, &PlaybookErrorType::CycleLimit);
        let fp2 = error_fingerprint("pb-1", "rule-a", 0, &PlaybookErrorType::CycleLimit);
        assert_eq!(
            fp1, fp2,
            "same inputs should produce identical fingerprints"
        );
        // SHA-256 hex is 64 chars
        assert_eq!(fp1.len(), 64);
    }

    #[test]
    fn fingerprint_differs_for_different_inputs() {
        let fp1 = error_fingerprint("pb-1", "rule-a", 0, &PlaybookErrorType::CycleLimit);
        let fp2 = error_fingerprint("pb-2", "rule-a", 0, &PlaybookErrorType::CycleLimit);
        let fp3 = error_fingerprint("pb-1", "rule-b", 0, &PlaybookErrorType::CycleLimit);
        let fp4 = error_fingerprint("pb-1", "rule-a", 1, &PlaybookErrorType::CycleLimit);
        assert_ne!(fp1, fp2, "different playbook_id should differ");
        assert_ne!(fp1, fp3, "different rule_name should differ");
        assert_ne!(fp1, fp4, "different error_location_index should differ");
    }

    #[test]
    fn fingerprint_differs_for_different_error_types() {
        let fp_cycle = error_fingerprint("pb-1", "rule-a", 0, &PlaybookErrorType::CycleLimit);
        let fp_missing = error_fingerprint("pb-1", "rule-a", 0, &PlaybookErrorType::MissingPath);
        let fp_type = error_fingerprint("pb-1", "rule-a", 0, &PlaybookErrorType::TypeMismatch);
        let fp_compile = error_fingerprint("pb-1", "rule-a", 0, &PlaybookErrorType::CompileError);
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
        use crate::db::SurrealStore;
        use crate::services::NodeService;
        use std::sync::Arc;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SurrealStore> = Arc::new(SurrealStore::new(db_path).await.unwrap());
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
                    "description": "playbook log schema",
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
                PlaybookErrorType::CycleLimit,
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
            assert_eq!(props["playbook_id"], "pb-1");
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
                    "description": "playbook log schema",
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
                PlaybookErrorType::MissingPath,
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
                PlaybookErrorType::MissingPath,
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
                    "description": "playbook log schema",
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
                PlaybookErrorType::ActionError,
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
                PlaybookErrorType::ActionError,
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
                PlaybookErrorType::ActionError,
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
    }
}
