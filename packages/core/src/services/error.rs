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
}
