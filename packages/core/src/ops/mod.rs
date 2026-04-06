//! Operations Layer
//!
//! Shared orchestration logic that both MCP handlers and local agent tools call.
//! Each function accepts typed inputs, coordinates service calls (collection resolution,
//! OCC auto-fetch, lifecycle management, search post-filtering), and returns typed outputs.

pub mod context_ops;
pub mod node_ops;
pub mod rel_ops;
pub mod search_ops;
pub mod skill_ops;

use crate::services::NodeServiceError;

/// Shared error type for operations layer
#[derive(Debug, thiserror::Error)]
pub enum OpsError {
    #[error("Not found: {id}")]
    NotFound { id: String },

    #[error("Version conflict on {node_id}: expected {expected}, got {actual}")]
    VersionConflict {
        node_id: String,
        expected: i64,
        actual: i64,
        current_node: Option<serde_json::Value>,
    },

    #[error("Validation failed: {0}")]
    ValidationFailed(String),

    #[error("Invalid parameters: {0}")]
    InvalidParams(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<NodeServiceError> for OpsError {
    fn from(err: NodeServiceError) -> Self {
        match err {
            NodeServiceError::NodeNotFound { id } => OpsError::NotFound { id },
            NodeServiceError::VersionConflict {
                node_id,
                expected_version,
                actual_version,
            } => OpsError::VersionConflict {
                node_id,
                expected: expected_version,
                actual: actual_version,
                current_node: None,
            },
            NodeServiceError::ValidationFailed(e) => OpsError::ValidationFailed(e.to_string()),
            NodeServiceError::InvalidParent { parent_id } => {
                OpsError::ValidationFailed(format!("Invalid parent: {}", parent_id))
            }
            NodeServiceError::InvalidRoot { root_node_id } => {
                OpsError::ValidationFailed(format!("Invalid root: {}", root_node_id))
            }
            NodeServiceError::CircularReference { context } => {
                OpsError::ValidationFailed(format!("Circular reference: {}", context))
            }
            NodeServiceError::HierarchyViolation(msg) => {
                OpsError::ValidationFailed(format!("Hierarchy violation: {}", msg))
            }
            NodeServiceError::PlaybookValidationFailed { .. } => {
                OpsError::InvalidParams(err.to_string())
            }
            NodeServiceError::CollectionNotFound(name) => OpsError::NotFound { id: name },
            other => OpsError::Internal(other.to_string()),
        }
    }
}
