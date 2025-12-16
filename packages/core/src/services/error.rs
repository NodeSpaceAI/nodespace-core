//! Service Layer Error Types
//!
//! This module defines error types for service-layer operations, providing
//! detailed error handling for business logic failures.

use crate::db::DatabaseError;
use crate::models::ValidationError;
use thiserror::Error;

/// Service operation errors
///
/// Provides high-level error types for all service operations,
/// with detailed context and proper error chaining.
#[derive(Error, Debug)]
pub enum NodeServiceError {
    /// Node not found by ID
    #[error("Node not found: {id}")]
    NodeNotFound { id: String },

    /// Validation failed for node
    #[error("Node validation failed: {0}")]
    ValidationFailed(#[from] ValidationError),

    /// Database operation failed
    #[error("Database operation failed: {0}")]
    DatabaseError(#[from] DatabaseError),

    /// Invalid parent reference
    #[error("Invalid parent node: {parent_id}")]
    InvalidParent { parent_id: String },

    /// Invalid root reference
    #[error("Invalid root node: {root_node_id}")]
    InvalidRoot { root_node_id: String },

    /// Circular reference detected
    #[error("Circular reference detected: {context}")]
    CircularReference { context: String },

    /// Node hierarchy constraint violation
    #[error("Hierarchy constraint violated: {0}")]
    HierarchyViolation(String),

    /// Bulk operation failed
    #[error("Bulk operation failed: {context}")]
    BulkOperationFailed { context: String },

    /// Transaction failed
    #[error("Transaction failed: {context}")]
    TransactionFailed { context: String },

    /// Invalid update operation
    #[error("Invalid update: {0}")]
    InvalidUpdate(String),

    /// Serialization/deserialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Query execution error
    #[error("Query failed: {0}")]
    QueryFailed(String),

    /// Version conflict (optimistic concurrency control)
    #[error("Version conflict for node {node_id}: expected version {expected_version}, found {actual_version}")]
    VersionConflict {
        node_id: String,
        expected_version: i64,
        actual_version: i64,
    },

    /// Service initialization failed
    #[error("Initialization error: {0}")]
    InitializationError(String),

    /// Collection path error
    #[error("Invalid collection path: {0}")]
    InvalidCollectionPath(String),

    /// Collection not found
    #[error("Collection not found: {0}")]
    CollectionNotFound(String),

    /// Collection hierarchy cycle detected
    #[error("Collection hierarchy cycle detected: {0}")]
    CollectionCycle(String),

    /// Maximum collection depth exceeded
    #[error("Collection path exceeds maximum depth of {max_depth} levels: {path}")]
    CollectionDepthExceeded { path: String, max_depth: usize },
}

impl NodeServiceError {
    /// Create a node not found error
    pub fn node_not_found(id: impl Into<String>) -> Self {
        Self::NodeNotFound { id: id.into() }
    }

    /// Create an invalid parent error
    pub fn invalid_parent(parent_id: impl Into<String>) -> Self {
        Self::InvalidParent {
            parent_id: parent_id.into(),
        }
    }

    /// Create an invalid root error
    pub fn invalid_root(root_node_id: impl Into<String>) -> Self {
        Self::InvalidRoot {
            root_node_id: root_node_id.into(),
        }
    }

    /// Create a circular reference error
    pub fn circular_reference(context: impl Into<String>) -> Self {
        Self::CircularReference {
            context: context.into(),
        }
    }

    /// Create a hierarchy violation error
    pub fn hierarchy_violation(msg: impl Into<String>) -> Self {
        Self::HierarchyViolation(msg.into())
    }

    /// Create a bulk operation failed error
    pub fn bulk_operation_failed(context: impl Into<String>) -> Self {
        Self::BulkOperationFailed {
            context: context.into(),
        }
    }

    /// Create a transaction failed error
    pub fn transaction_failed(context: impl Into<String>) -> Self {
        Self::TransactionFailed {
            context: context.into(),
        }
    }

    /// Create an invalid update error
    pub fn invalid_update(msg: impl Into<String>) -> Self {
        Self::InvalidUpdate(msg.into())
    }

    /// Create a serialization error
    pub fn serialization_error(msg: impl Into<String>) -> Self {
        Self::SerializationError(msg.into())
    }

    /// Create a query failed error
    pub fn query_failed(msg: impl Into<String>) -> Self {
        Self::QueryFailed(msg.into())
    }

    /// Create a version conflict error
    pub fn version_conflict(
        node_id: impl Into<String>,
        expected_version: i64,
        actual_version: i64,
    ) -> Self {
        Self::VersionConflict {
            node_id: node_id.into(),
            expected_version,
            actual_version,
        }
    }

    /// Create an initialization error
    pub fn initialization_error(msg: impl Into<String>) -> Self {
        Self::InitializationError(msg.into())
    }

    /// Create an invalid collection path error
    pub fn invalid_collection_path(msg: impl Into<String>) -> Self {
        Self::InvalidCollectionPath(msg.into())
    }

    /// Create a collection not found error
    pub fn collection_not_found(name: impl Into<String>) -> Self {
        Self::CollectionNotFound(name.into())
    }

    /// Create a collection cycle error
    pub fn collection_cycle(context: impl Into<String>) -> Self {
        Self::CollectionCycle(context.into())
    }

    /// Create a collection depth exceeded error
    pub fn collection_depth_exceeded(path: impl Into<String>, max_depth: usize) -> Self {
        Self::CollectionDepthExceeded {
            path: path.into(),
            max_depth,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_not_found_error() {
        let err = NodeServiceError::node_not_found("test-id");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::NodeNotFound { .. }));
        assert!(msg.contains("test-id"));
    }

    #[test]
    fn test_invalid_parent_error() {
        let err = NodeServiceError::invalid_parent("parent-123");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::InvalidParent { .. }));
        assert!(msg.contains("parent-123"));
    }

    #[test]
    fn test_invalid_root_error() {
        let err = NodeServiceError::invalid_root("root-456");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::InvalidRoot { .. }));
        assert!(msg.contains("root-456"));
    }

    #[test]
    fn test_circular_reference_error() {
        let err = NodeServiceError::circular_reference("node A -> B -> A");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::CircularReference { .. }));
        assert!(msg.contains("Circular reference"));
    }

    #[test]
    fn test_hierarchy_violation_error() {
        let err = NodeServiceError::hierarchy_violation("Cannot move root node");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::HierarchyViolation(_)));
        assert!(msg.contains("Hierarchy constraint"));
    }

    #[test]
    fn test_bulk_operation_failed_error() {
        let err = NodeServiceError::bulk_operation_failed("3 of 5 nodes failed");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::BulkOperationFailed { .. }));
        assert!(msg.contains("Bulk operation"));
    }

    #[test]
    fn test_transaction_failed_error() {
        let err = NodeServiceError::transaction_failed("Commit failed");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::TransactionFailed { .. }));
        assert!(msg.contains("Transaction failed"));
    }

    #[test]
    fn test_invalid_update_error() {
        let err = NodeServiceError::invalid_update("Cannot change node type");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::InvalidUpdate(_)));
        assert!(msg.contains("Invalid update"));
    }

    #[test]
    fn test_serialization_error() {
        let err = NodeServiceError::serialization_error("Invalid JSON");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::SerializationError(_)));
        assert!(msg.contains("Serialization error"));
    }

    #[test]
    fn test_query_failed_error() {
        let err = NodeServiceError::query_failed("Syntax error in SQL");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::QueryFailed(_)));
        assert!(msg.contains("Query failed"));
    }

    #[test]
    fn test_version_conflict_error() {
        let err = NodeServiceError::version_conflict("node-789", 5, 3);
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::VersionConflict { .. }));
        assert!(msg.contains("Version conflict"));
        assert!(msg.contains("expected version 5"));
        assert!(msg.contains("found 3"));
    }

    #[test]
    fn test_initialization_error() {
        let err = NodeServiceError::initialization_error("Database connection failed");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::InitializationError(_)));
        assert!(msg.contains("Initialization error"));
    }

    #[test]
    fn test_invalid_collection_path_error() {
        let err = NodeServiceError::invalid_collection_path("Empty path segment");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::InvalidCollectionPath(_)));
        assert!(msg.contains("Invalid collection path"));
    }

    #[test]
    fn test_collection_not_found_error() {
        let err = NodeServiceError::collection_not_found("favorites");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::CollectionNotFound(_)));
        assert!(msg.contains("Collection not found"));
    }

    #[test]
    fn test_collection_cycle_error() {
        let err = NodeServiceError::collection_cycle("A -> B -> A");
        let msg = err.to_string();
        assert!(matches!(err, NodeServiceError::CollectionCycle(_)));
        assert!(msg.contains("Collection hierarchy cycle"));
    }

    #[test]
    fn test_collection_depth_exceeded_error() {
        let err = NodeServiceError::collection_depth_exceeded("a:b:c:d:e:f", 5);
        let msg = err.to_string();
        assert!(matches!(
            err,
            NodeServiceError::CollectionDepthExceeded { .. }
        ));
        assert!(msg.contains("maximum depth of 5"));
    }

    #[test]
    fn test_error_from_validation_error() {
        let validation_err = ValidationError::MissingField("test_field".to_string());
        let err: NodeServiceError = validation_err.into();
        assert!(matches!(err, NodeServiceError::ValidationFailed(_)));
    }

    #[test]
    fn test_error_from_database_error() {
        let db_err = DatabaseError::initialization_failed("Connection lost");
        let err: NodeServiceError = db_err.into();
        assert!(matches!(err, NodeServiceError::DatabaseError(_)));
    }
}
