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
        n.properties
            .get("error_fingerprint")
            .and_then(|v| v.as_str())
            == Some(&fingerprint)
    });

    if let Some(log_node) = existing {
        // Deduplicate: increment occurrences and update last_seen
        let current_occurrences = log_node
            .properties
            .get("occurrences")
            .and_then(|v| v.as_u64())
            .unwrap_or(1);

        let mut new_properties = log_node.properties.clone();
        new_properties["occurrences"] = json!(current_occurrences + 1);
        new_properties["last_seen"] = json!(now);
        // Update the latest trigger_node_id for debugging context
        new_properties["trigger_node_id"] = json!(trigger_node_id);

        let update =
            crate::models::NodeUpdate::new().with_properties(new_properties);

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
