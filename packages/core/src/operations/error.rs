//! Error types for NodeOperations business logic layer
//!
//! This module defines all error types that can occur during node operations,
//! providing clear, actionable error messages for business rule violations.

use thiserror::Error;

/// Errors that can occur during node operations
///
/// These errors represent business rule violations and data integrity issues
/// that are enforced by the NodeOperations layer.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::operations::NodeOperationError;
///
/// // Container type violation
/// let err = NodeOperationError::ContainerCannotHaveParent {
///     node_id: "2025-01-03".to_string(),
///     node_type: "date".to_string(),
/// };
///
/// // Non-container missing container
/// let err = NodeOperationError::NonContainerMustHaveContainer {
///     node_id: "node-123".to_string(),
///     node_type: "text".to_string(),
/// };
/// ```
#[derive(Error, Debug)]
pub enum NodeOperationError {
    /// Container nodes (date, topic, project) cannot have parent_id set
    ///
    /// Container nodes are root-level entities and must have:
    /// - parent_id = None
    /// - container_node_id = None
    /// - before_sibling_id = None
    #[error("Container node '{node_id}' of type '{node_type}' cannot have a parent")]
    ContainerCannotHaveParent { node_id: String, node_type: String },

    /// Container nodes (date, topic, project) cannot have container_node_id set
    ///
    /// Container nodes define their own scope and cannot belong to another container.
    #[error("Container node '{node_id}' of type '{node_type}' cannot have a container_node_id")]
    ContainerCannotHaveContainer { node_id: String, node_type: String },

    /// Container nodes (date, topic, project) cannot have before_sibling_id set
    ///
    /// Container nodes are independent entities that don't participate in sibling ordering.
    #[error("Container node '{node_id}' of type '{node_type}' cannot have a before_sibling_id")]
    ContainerCannotHaveSibling { node_id: String, node_type: String },

    /// Node type cannot be a container
    ///
    /// Only specific node types (date, text, header) can be containers.
    /// Multi-line types (code-block, quote-block, ordered-list, task) cannot be containers.
    #[error("Node '{node_id}' of type '{node_type}' cannot be a container. Only date, text, and header nodes can be containers.")]
    InvalidContainerType { node_id: String, node_type: String },

    /// Non-container nodes must have a container_node_id
    ///
    /// Every non-container node must belong to exactly one container (date, topic, or project).
    /// The container_node_id can be inferred from the parent if not provided explicitly.
    #[error("Non-container node '{node_id}' of type '{node_type}' must have a container_node_id")]
    NonContainerMustHaveContainer { node_id: String, node_type: String },

    /// Parent and child must be in the same container
    ///
    /// A node's parent must be in the same container as the node itself.
    /// This ensures consistent hierarchy within each container.
    #[error("Parent-container mismatch: parent has container '{parent_container}', child has container '{child_container}'")]
    ParentContainerMismatch {
        parent_container: String,
        child_container: String,
    },

    /// Referenced node does not exist
    ///
    /// Occurs when trying to reference a non-existent node as parent, container, or sibling.
    #[error("Node '{node_id}' does not exist")]
    NodeNotFound { node_id: String },

    /// Invalid sibling chain detected
    ///
    /// The before_sibling_id creates an invalid or circular sibling chain.
    #[error("Invalid sibling chain: {reason}")]
    InvalidSiblingChain { reason: String },

    /// Version conflict detected (optimistic concurrency control)
    ///
    /// The node was modified by another process between read and write.
    /// Client should re-read the node and retry the operation with the new version.
    ///
    /// # Error Response Format
    ///
    /// MCP clients receive:
    /// ```json
    /// {
    ///   "code": -32001,
    ///   "message": "Version conflict: expected v5 but current is v6",
    ///   "data": {
    ///     "type": "VersionConflict",
    ///     "expected_version": 5,
    ///     "actual_version": 6,
    ///     "current_node": { /* full node state */ }
    ///   }
    /// }
    /// ```
    ///
    /// Frontend UI shows conflict resolution modal with both versions.
    #[error("Version conflict for node '{node_id}': expected version {expected_version}, but current version is {actual_version}")]
    VersionConflict {
        node_id: String,
        expected_version: i64,
        actual_version: i64,
        /// Current state of the node for merge resolution
        current_node: Box<crate::models::Node>,
    },

    /// Circular reference detected
    ///
    /// Node cannot reference itself as parent, container, or sibling.
    #[error("Circular reference: node '{node_id}' cannot reference itself as {reference_type}")]
    CircularReference {
        node_id: String,
        reference_type: String,
    },

    /// Validation error from NodeBehavior
    ///
    /// Wraps validation errors from the NodeBehavior trait.
    #[error("Validation error: {0}")]
    ValidationError(#[from] crate::models::ValidationError),

    /// Service error from NodeService
    ///
    /// Wraps errors from the underlying NodeService layer.
    #[error("Service error: {0}")]
    ServiceError(#[from] crate::services::error::NodeServiceError),

    /// Invalid operation attempted
    ///
    /// The requested operation is not supported or violates business rules.
    #[error("Invalid operation: {reason}")]
    InvalidOperation { reason: String },

    /// Database operation failed
    ///
    /// Wraps database-level errors.
    #[error("Database error: {0}")]
    DatabaseError(String),

    /// Internal error (should not occur in normal operation)
    ///
    /// Indicates a bug in the business logic layer.
    #[error("Internal error: {0}")]
    InternalError(String),
}

impl NodeOperationError {
    /// Create a ContainerCannotHaveParent error
    pub fn container_cannot_have_parent(node_id: String, node_type: String) -> Self {
        Self::ContainerCannotHaveParent { node_id, node_type }
    }

    /// Create a ContainerCannotHaveContainer error
    pub fn container_cannot_have_container(node_id: String, node_type: String) -> Self {
        Self::ContainerCannotHaveContainer { node_id, node_type }
    }

    /// Create a ContainerCannotHaveSibling error
    pub fn container_cannot_have_sibling(node_id: String, node_type: String) -> Self {
        Self::ContainerCannotHaveSibling { node_id, node_type }
    }

    /// Create an InvalidContainerType error
    pub fn invalid_container_type(node_id: String, node_type: String) -> Self {
        Self::InvalidContainerType { node_id, node_type }
    }

    /// Create a NonContainerMustHaveContainer error
    pub fn non_container_must_have_container(node_id: String, node_type: String) -> Self {
        Self::NonContainerMustHaveContainer { node_id, node_type }
    }

    /// Create a ParentContainerMismatch error
    pub fn parent_container_mismatch(parent_container: String, child_container: String) -> Self {
        Self::ParentContainerMismatch {
            parent_container,
            child_container,
        }
    }

    /// Create a NodeNotFound error
    pub fn node_not_found(node_id: String) -> Self {
        Self::NodeNotFound { node_id }
    }

    /// Create an InvalidSiblingChain error
    pub fn invalid_sibling_chain(reason: String) -> Self {
        Self::InvalidSiblingChain { reason }
    }

    /// Create a VersionConflict error
    pub fn version_conflict(
        node_id: String,
        expected_version: i64,
        actual_version: i64,
        current_node: crate::models::Node,
    ) -> Self {
        Self::VersionConflict {
            node_id,
            expected_version,
            actual_version,
            current_node: Box::new(current_node),
        }
    }

    /// Create a CircularReference error
    pub fn circular_reference(node_id: String, reference_type: String) -> Self {
        Self::CircularReference {
            node_id,
            reference_type,
        }
    }

    /// Create an InvalidOperation error
    pub fn invalid_operation(reason: String) -> Self {
        Self::InvalidOperation { reason }
    }

    /// Create a DatabaseError
    pub fn database_error(message: String) -> Self {
        Self::DatabaseError(message)
    }

    /// Create an InternalError
    pub fn internal_error(message: String) -> Self {
        Self::InternalError(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_cannot_have_parent_error() {
        let err = NodeOperationError::container_cannot_have_parent(
            "2025-01-03".to_string(),
            "date".to_string(),
        );
        assert!(matches!(
            err,
            NodeOperationError::ContainerCannotHaveParent { .. }
        ));
        assert_eq!(
            format!("{}", err),
            "Container node '2025-01-03' of type 'date' cannot have a parent"
        );
    }

    #[test]
    fn test_container_cannot_have_container_error() {
        let err = NodeOperationError::container_cannot_have_container(
            "2025-01-03".to_string(),
            "date".to_string(),
        );
        assert!(matches!(
            err,
            NodeOperationError::ContainerCannotHaveContainer { .. }
        ));
        assert_eq!(
            format!("{}", err),
            "Container node '2025-01-03' of type 'date' cannot have a container_node_id"
        );
    }

    #[test]
    fn test_container_cannot_have_sibling_error() {
        let err = NodeOperationError::container_cannot_have_sibling(
            "2025-01-03".to_string(),
            "date".to_string(),
        );
        assert!(matches!(
            err,
            NodeOperationError::ContainerCannotHaveSibling { .. }
        ));
        assert_eq!(
            format!("{}", err),
            "Container node '2025-01-03' of type 'date' cannot have a before_sibling_id"
        );
    }

    #[test]
    fn test_non_container_must_have_container_error() {
        let err = NodeOperationError::non_container_must_have_container(
            "node-123".to_string(),
            "text".to_string(),
        );
        assert!(matches!(
            err,
            NodeOperationError::NonContainerMustHaveContainer { .. }
        ));
        assert_eq!(
            format!("{}", err),
            "Non-container node 'node-123' of type 'text' must have a container_node_id"
        );
    }

    #[test]
    fn test_parent_container_mismatch_error() {
        let err = NodeOperationError::parent_container_mismatch(
            "date-1".to_string(),
            "date-2".to_string(),
        );
        assert!(matches!(
            err,
            NodeOperationError::ParentContainerMismatch { .. }
        ));
        assert_eq!(
            format!("{}", err),
            "Parent-container mismatch: parent has container 'date-1', child has container 'date-2'"
        );
    }

    #[test]
    fn test_node_not_found_error() {
        let err = NodeOperationError::node_not_found("missing-node".to_string());
        assert!(matches!(err, NodeOperationError::NodeNotFound { .. }));
        assert_eq!(format!("{}", err), "Node 'missing-node' does not exist");
    }

    #[test]
    fn test_invalid_sibling_chain_error() {
        let err = NodeOperationError::invalid_sibling_chain("Circular chain detected".to_string());
        assert!(matches!(
            err,
            NodeOperationError::InvalidSiblingChain { .. }
        ));
        assert_eq!(
            format!("{}", err),
            "Invalid sibling chain: Circular chain detected"
        );
    }

    #[test]
    fn test_circular_reference_error() {
        let err =
            NodeOperationError::circular_reference("node-123".to_string(), "parent".to_string());
        assert!(matches!(err, NodeOperationError::CircularReference { .. }));
        assert_eq!(
            format!("{}", err),
            "Circular reference: node 'node-123' cannot reference itself as parent"
        );
    }

    #[test]
    fn test_invalid_operation_error() {
        let err = NodeOperationError::invalid_operation("Cannot update hierarchy".to_string());
        assert!(matches!(err, NodeOperationError::InvalidOperation { .. }));
        assert_eq!(
            format!("{}", err),
            "Invalid operation: Cannot update hierarchy"
        );
    }

    #[test]
    fn test_database_error() {
        let err = NodeOperationError::database_error("Connection failed".to_string());
        assert!(matches!(err, NodeOperationError::DatabaseError(_)));
        assert_eq!(format!("{}", err), "Database error: Connection failed");
    }

    #[test]
    fn test_internal_error() {
        let err = NodeOperationError::internal_error("Unexpected state".to_string());
        assert!(matches!(err, NodeOperationError::InternalError(_)));
        assert_eq!(format!("{}", err), "Internal error: Unexpected state");
    }

    // Note: Removed equality and cloning tests since ValidationError and NodeServiceError
    // don't implement Clone/PartialEq. This is acceptable since errors are typically
    // not compared or cloned in production code.
}
