//! Node Operations Business Logic Layer
//!
//! This module provides a centralized business logic layer that enforces
//! data integrity rules for node creation, updates, and hierarchy management.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │  MCP Handlers + Tauri Commands          │
//! └──────────────┬──────────────────────────┘
//!                │
//!                ▼
//! ┌─────────────────────────────────────────┐
//! │  NodeOperations (Business Logic) ✅     │
//! │  - Container validation                 │
//! │  - Sibling position calculation         │
//! │  - Parent-container consistency         │
//! │  - Type-specific rules                  │
//! └──────────────┬──────────────────────────┘
//!                │
//!                ▼
//! ┌─────────────────────────────────────────┐
//! │  NodeService (Database CRUD)            │
//! └─────────────────────────────────────────┘
//! ```
//!
//! # Business Rules Enforced
//!
//! 1. **Container Type Validation**: Container types (date, topic, project) force
//!    all hierarchy fields (parent_id, container_node_id, before_sibling_id) to None.
//!
//! 2. **Container Requirement**: Every non-container node MUST have a container_node_id.
//!    If not provided, it's inferred from the parent.
//!
//! 3. **Sibling Position Calculation**: If before_sibling_id is not provided,
//!    the node is placed as the last sibling automatically.
//!
//! 4. **Parent-Container Consistency**: Parent and child must be in the same container.
//!
//! 5. **Update Operations Restrictions**: Content updates cannot change hierarchy.
//!    Use move_node() or reorder_node() explicitly for hierarchy changes.
//!
//! # Examples
//!
//! ```no_run
//! use nodespace_core::operations::NodeOperations;
//! use nodespace_core::services::NodeService;
//! use nodespace_core::db::DatabaseService;
//! use std::path::PathBuf;
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let db = DatabaseService::new(PathBuf::from("./data/test.db")).await?;
//!     let node_service = NodeService::new(db)?;
//!     let operations = NodeOperations::new(node_service);
//!
//!     // Create a node with automatic container inference
//!     let node_id = operations.create_node(
//!         "text".to_string(),
//!         "Hello World".to_string(),
//!         Some("parent-id".to_string()),
//!         None, // container_id will be inferred from parent
//!         None, // will be placed as last sibling
//!         json!({}),
//!     ).await?;
//!
//!     println!("Created node: {}", node_id);
//!     Ok(())
//! }
//! ```

pub mod error;

// Re-export error type for convenience
pub use error::NodeOperationError;

use crate::models::{DeleteResult, Node, NodeFilter, NodeUpdate};
use crate::services::NodeService;
use chrono::Utc;
use serde_json::Value;
use std::sync::Arc;

/// Core business logic layer for node operations
///
/// This struct enforces all business rules for node creation, updates, and
/// hierarchy management. It wraps NodeService and provides a higher-level
/// API that guarantees data integrity.
///
/// # Thread Safety
///
/// NodeOperations is `Clone` and thread-safe, using Arc internally for
/// efficient sharing across threads.
///
/// # Examples
///
/// ```no_run
/// # use nodespace_core::operations::NodeOperations;
/// # use nodespace_core::services::NodeService;
/// # use nodespace_core::db::DatabaseService;
/// # use std::path::PathBuf;
/// # use serde_json::json;
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
/// # let node_service = NodeService::new(db)?;
/// let operations = NodeOperations::new(node_service);
///
/// // All operations automatically enforce business rules
/// let node_id = operations.create_node(
///     "text".to_string(),
///     "Content".to_string(),
///     Some("parent-id".to_string()),
///     None, // container_id inferred
///     None, // placed as last sibling
///     json!({}),
/// ).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct NodeOperations {
    /// Underlying NodeService for database operations
    node_service: Arc<NodeService>,
}

impl NodeOperations {
    /// Create a new NodeOperations instance
    ///
    /// # Arguments
    ///
    /// * `node_service` - The NodeService instance to use for database operations
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// let node_service = NodeService::new(db)?;
    /// let operations = NodeOperations::new(node_service);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self { node_service }
    }

    // =========================================================================
    // Private Helper Methods
    // =========================================================================

    /// Check if a node type is a container (date, topic, project)
    ///
    /// Container types define their own scope and cannot have parent/container/sibling.
    ///
    /// # Arguments
    ///
    /// * `node_type` - The node type to check
    ///
    /// # Returns
    ///
    /// true if the node type is a container, false otherwise
    ///
    /// # Examples
    ///
    /// ```
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// // These are internal examples for documentation
    /// // assert!(operations.can_be_container_type("date"));
    /// // assert!(operations.can_be_container_type("text"));
    /// // assert!(!operations.can_be_container_type("task"));
    /// # Ok(())
    /// # }
    /// ```
    fn can_be_container_type(node_type: &str) -> bool {
        // Check if a node type is ALLOWED to be a container.
        // This does NOT mean the node IS a container - that depends on hierarchy fields
        // (a node IS a container if parent_id=None AND container_node_id=None).
        //
        // Container-capable types:
        // - date: Auto-created date containers (virtual)
        // - text: Simple text containers (e.g., document titles)
        // - header: Header containers (e.g., "# Project Name")
        // Multi-line types (code-block, quote-block, ordered-list) cannot be containers
        matches!(node_type, "date" | "text" | "header")
    }

    /// Resolve container_node_id for non-container nodes
    ///
    /// Business Rule 2: Every non-container node MUST have a container_node_id.
    /// If not provided explicitly, infer it from the parent.
    ///
    /// # Arguments
    ///
    /// * `container_node_id` - Explicitly provided container (may be None)
    /// * `parent_id` - Parent node ID (may be None)
    ///
    /// # Returns
    ///
    /// The resolved container_node_id or an error if it cannot be determined
    ///
    /// # Errors
    ///
    /// - `NodeNotFound` if parent doesn't exist
    /// - `NonContainerMustHaveContainer` if container cannot be resolved
    async fn resolve_container(
        &self,
        node_id: &str,
        node_type: &str,
        container_node_id: Option<String>,
        parent_id: Option<&str>,
    ) -> Result<String, NodeOperationError> {
        // If container is explicitly provided, use it
        if let Some(container_id) = container_node_id {
            return Ok(container_id);
        }

        // Otherwise, infer from parent
        if let Some(parent_id) = parent_id {
            let parent = self
                .node_service
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(parent_id.to_string()))?;

            // If parent has a container, use it
            if let Some(parent_container) = parent.container_node_id {
                return Ok(parent_container);
            }

            // If parent IS a container (container_node_id = None), use parent as container
            if parent.is_root() {
                return Ok(parent_id.to_string());
            }
        }

        // Cannot resolve container
        Err(NodeOperationError::non_container_must_have_container(
            node_id.to_string(),
            node_type.to_string(),
        ))
    }

    /// Calculate sibling position for a new/moved node
    ///
    /// Business Rule 3: If before_sibling_id is not provided, place the node
    /// as the last sibling automatically.
    ///
    /// # Arguments
    ///
    /// * `parent_id` - Parent node ID (may be None for root nodes)
    /// * `before_sibling_id` - Explicitly provided sibling position (may be None)
    ///
    /// # Returns
    ///
    /// The calculated before_sibling_id (None means last position)
    ///
    /// # Errors
    ///
    /// - `InvalidSiblingChain` if before_sibling_id is invalid
    /// - `NodeNotFound` if before_sibling doesn't exist
    async fn calculate_sibling_position(
        &self,
        parent_id: Option<&str>,
        before_sibling_id: Option<String>,
    ) -> Result<Option<String>, NodeOperationError> {
        // If explicitly provided, validate it exists
        if let Some(ref sibling_id) = before_sibling_id {
            let sibling = self
                .node_service
                .get_node(sibling_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(sibling_id.to_string()))?;

            // Verify sibling has same parent
            if sibling.parent_id.as_deref() != parent_id {
                return Err(NodeOperationError::invalid_sibling_chain(format!(
                    "Sibling '{}' has different parent than the node being created/moved",
                    sibling_id
                )));
            }

            return Ok(before_sibling_id);
        }

        // No explicit sibling provided - place as last sibling (None)
        Ok(None)
    }

    /// Validate parent-container consistency
    ///
    /// Business Rule 4: Parent and child must be in the same container.
    ///
    /// # Arguments
    ///
    /// * `parent_id` - Parent node ID (may be None)
    /// * `container_node_id` - Container node ID for the child
    ///
    /// # Errors
    ///
    /// - `ParentContainerMismatch` if parent and child have different containers
    /// - `NodeNotFound` if parent doesn't exist
    async fn validate_parent_container_consistency(
        &self,
        parent_id: Option<&str>,
        container_node_id: &str,
    ) -> Result<(), NodeOperationError> {
        if let Some(parent_id) = parent_id {
            let parent = self
                .node_service
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(parent_id.to_string()))?;

            // Parent must have same container
            if let Some(parent_container) = parent.container_node_id {
                if parent_container != container_node_id {
                    return Err(NodeOperationError::parent_container_mismatch(
                        parent_container,
                        container_node_id.to_string(),
                    ));
                }
            } else {
                // Parent has no container - this is invalid unless parent is a container itself
                // (in which case parent_id would be the container_node_id)
                if parent_id != container_node_id {
                    return Err(NodeOperationError::parent_container_mismatch(
                        "None".to_string(),
                        container_node_id.to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    // =========================================================================
    // CREATE Operations
    // =========================================================================

    /// Create a new node with business rule enforcement
    ///
    /// This method enforces all 5 business rules:
    /// 1. Container validation
    /// 2. Container requirement (with auto-inference)
    /// 3. Sibling position calculation
    /// 4. Parent-container consistency
    /// 5. Type-specific validation
    ///
    /// # Arguments
    ///
    /// * `node_type` - Type identifier (e.g., "text", "task", "date")
    /// * `content` - Primary content/text
    /// * `parent_id` - Optional parent node reference
    /// * `container_node_id` - Optional container reference (inferred from parent if None)
    /// * `before_sibling_id` - Optional sibling ordering (calculated as last if None)
    /// * `properties` - JSON object with entity-specific fields
    ///
    /// # Returns
    ///
    /// The ID of the created node
    ///
    /// # Errors
    ///
    /// Returns `NodeOperationError` if:
    /// - Container nodes have parent/container/sibling set
    /// - Non-container nodes lack container_node_id and parent doesn't exist
    /// - Parent-container mismatch
    /// - Invalid sibling chain
    /// - Node validation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// // Create a date node (container)
    /// let date_id = operations.create_node(
    ///     "date".to_string(),
    ///     "2025-01-03".to_string(),
    ///     None, // containers cannot have parent
    ///     None, // containers cannot have container
    ///     None, // containers cannot have sibling
    ///     json!({}),
    /// ).await?;
    ///
    /// // Create a text node with automatic container inference
    /// let text_id = operations.create_node(
    ///     "text".to_string(),
    ///     "Hello".to_string(),
    ///     Some(date_id.clone()), // parent
    ///     None, // container_id inferred from parent (date_id)
    ///     None, // placed as last sibling
    ///     json!({}),
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Date Node Special Case
    ///
    /// Date nodes use their content (YYYY-MM-DD format) as their ID instead of
    /// generating a UUID. This enables deterministic "get-or-create" semantics
    /// for daily notes.
    ///
    /// ```rust,no_run
    /// # use nodespace_core::NodeOperations;
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # async fn example(operations: Arc<NodeOperations>) -> Result<(), Box<dyn std::error::Error>> {
    /// let date_id = operations.create_node(
    ///     "date".to_string(),
    ///     "2025-10-23".to_string(),  // This becomes the ID
    ///     None, None, None, json!({})
    /// ).await?;
    ///
    /// assert_eq!(date_id, "2025-10-23");  // ID matches content
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_node(
        &self,
        node_type: String,
        content: String,
        parent_id: Option<String>,
        container_node_id: Option<String>,
        before_sibling_id: Option<String>,
        properties: Value,
    ) -> Result<String, NodeOperationError> {
        // Business Rule 1: Determine if this node IS a container based on hierarchy fields
        // A node is a container if it has NO parent and NO container
        // (not just because its type CAN be a container)
        let is_container_node = parent_id.is_none() && container_node_id.is_none();

        let (final_parent_id, final_container_id, final_sibling_id) = if is_container_node {
            // This node IS a container - validate that its type allows being a container
            if !Self::can_be_container_type(&node_type) {
                return Err(NodeOperationError::invalid_container_type(
                    content.clone(),
                    node_type.clone(),
                ));
            }

            // Container nodes MUST have before_sibling_id as None as well
            if before_sibling_id.is_some() {
                return Err(NodeOperationError::container_cannot_have_sibling(
                    content.clone(),
                    node_type.clone(),
                ));
            }

            (None, None, None)
        } else {
            // Non-container nodes: apply business rules 2-4

            // Business Rule 2: Resolve container_node_id (with parent inference)
            let resolved_container = self
                .resolve_container(
                    &content, // Passed for error context only (node ID not yet assigned)
                    &node_type,
                    container_node_id,
                    parent_id.as_deref(),
                )
                .await?;

            // Business Rule 4: Validate parent-container consistency
            self.validate_parent_container_consistency(parent_id.as_deref(), &resolved_container)
                .await?;

            // Business Rule 3: Calculate sibling position
            let calculated_sibling = self
                .calculate_sibling_position(parent_id.as_deref(), before_sibling_id)
                .await?;

            (parent_id, Some(resolved_container), calculated_sibling)
        };

        // Create the node using NodeService
        // Special case: date nodes use their content (YYYY-MM-DD) as the ID
        let node_id = if node_type == "date" {
            content.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };

        let node = Node {
            id: node_id,
            node_type,
            content,
            parent_id: final_parent_id,
            container_node_id: final_container_id,
            before_sibling_id: final_sibling_id,
            properties,
            mentions: vec![],
            mentioned_by: vec![],
            created_at: Utc::now(),
            modified_at: Utc::now(),
            embedding_vector: None,
        };

        let created_id = self.node_service.create_node(node).await?;

        Ok(created_id)
    }

    // =========================================================================
    // READ Operations (simple delegation to NodeService)
    // =========================================================================

    /// Get a node by ID
    ///
    /// Simple delegation to NodeService with no additional business logic.
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to retrieve
    ///
    /// # Returns
    ///
    /// The node if found, None otherwise
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// let node = operations.get_node("node-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_node(&self, id: &str) -> Result<Option<Node>, NodeOperationError> {
        Ok(self.node_service.get_node(id).await?)
    }

    /// Query nodes with filter
    ///
    /// Simple delegation to NodeService with no additional business logic.
    ///
    /// # Arguments
    ///
    /// * `filter` - NodeFilter specifying query criteria
    ///
    /// # Returns
    ///
    /// Vector of matching nodes
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::NodeFilter;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// let filter = NodeFilter::new().with_node_type("text".to_string());
    /// let nodes = operations.query_nodes(filter).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_nodes(&self, filter: NodeFilter) -> Result<Vec<Node>, NodeOperationError> {
        Ok(self.node_service.query_nodes(filter).await?)
    }

    // =========================================================================
    // UPDATE Operations
    // =========================================================================

    /// Update node content and properties (no hierarchy changes)
    ///
    /// This method enforces Rule 5: content updates cannot change hierarchy.
    /// Use move_node() or reorder_node() for hierarchy changes.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to update
    /// * `content` - Optional new content
    /// * `node_type` - Optional new node type
    /// * `properties` - Optional new properties
    ///
    /// # Errors
    ///
    /// Returns `NodeOperationError` if node doesn't exist or validation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// operations.update_node(
    ///     "node-id",
    ///     Some("Updated content".to_string()),
    ///     None,
    ///     Some(json!({"status": "done"})),
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_node(
        &self,
        node_id: &str,
        content: Option<String>,
        node_type: Option<String>,
        properties: Option<Value>,
    ) -> Result<(), NodeOperationError> {
        // Business Rule 5: Content updates cannot change hierarchy
        // This method only updates content, node_type, and properties
        // Use move_node() or reorder_node() for hierarchy changes

        // Verify node exists
        let _node = self
            .node_service
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

        // Create NodeUpdate with only content/type/properties (no hierarchy changes)
        let mut update = NodeUpdate::new();
        if let Some(c) = content {
            update = update.with_content(c);
        }
        if let Some(t) = node_type {
            update = update.with_node_type(t);
        }
        if let Some(p) = properties {
            update = update.with_properties(p);
        }

        // Update using NodeService (hierarchy fields are not touched)
        self.node_service.update_node(node_id, update).await?;

        Ok(())
    }

    /// Move a node to a new parent
    ///
    /// This method enforces Rules 2, 4, and 5:
    /// - Validates parent-container consistency
    /// - Updates container_node_id if needed
    /// - Separate from content updates
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to move
    /// * `new_parent_id` - The new parent (None to make it root)
    ///
    /// # Errors
    ///
    /// Returns `NodeOperationError` if parent-container mismatch or node doesn't exist
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// operations.move_node("node-id", Some("new-parent-id")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn move_node(
        &self,
        node_id: &str,
        new_parent_id: Option<&str>,
    ) -> Result<(), NodeOperationError> {
        // Get current node
        let node = self
            .node_service
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

        // Container nodes cannot be moved (they are root nodes with no parent/container)
        // Check actual container status, not just type
        if node.is_root() {
            return Err(NodeOperationError::invalid_operation(format!(
                "Container node '{}' cannot be moved (it's a root node)",
                node_id
            )));
        }

        // Resolve new container from new parent (if parent exists)
        let new_container = if let Some(parent_id) = new_parent_id {
            let parent = self
                .node_service
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(parent_id.to_string()))?;

            parent.container_node_id.ok_or_else(|| {
                NodeOperationError::invalid_operation(format!(
                    "Parent '{}' has no container_node_id (parent should be in a container or be a container itself)",
                    parent_id
                ))
            })?
        } else {
            // Moving to root - node must have explicit container
            node.container_node_id.ok_or_else(|| {
                NodeOperationError::non_container_must_have_container(
                    node_id.to_string(),
                    node.node_type.clone(),
                )
            })?
        };

        // Validate parent-container consistency
        self.validate_parent_container_consistency(new_parent_id, &new_container)
            .await?;

        // Create NodeUpdate with new parent and container
        let mut update = NodeUpdate::new();
        update.parent_id = Some(new_parent_id.map(String::from));
        update.container_node_id = Some(Some(new_container));

        // Update node with new parent and container
        self.node_service.update_node(node_id, update).await?;

        Ok(())
    }

    /// Reorder a node within its siblings
    ///
    /// This method enforces Rule 3: sibling position calculation.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to reorder
    /// * `before_sibling_id` - The sibling to place this node before (None = last)
    ///
    /// # Errors
    ///
    /// Returns `NodeOperationError` if invalid sibling chain or node doesn't exist
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// // Move node to end of sibling list
    /// operations.reorder_node("node-id", None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reorder_node(
        &self,
        node_id: &str,
        before_sibling_id: Option<&str>,
    ) -> Result<(), NodeOperationError> {
        // Get current node
        let node = self
            .node_service
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

        // Container nodes cannot be reordered (they are root nodes with no siblings)
        // Check actual container status, not just type
        if node.is_root() {
            return Err(NodeOperationError::invalid_operation(format!(
                "Container node '{}' cannot be reordered (it's a root node with no siblings)",
                node_id
            )));
        }

        // Validate sibling position
        let new_sibling = self
            .calculate_sibling_position(
                node.parent_id.as_deref(),
                before_sibling_id.map(String::from),
            )
            .await?;

        // Create NodeUpdate with new sibling position
        let mut update = NodeUpdate::new();
        update.before_sibling_id = Some(new_sibling);

        // Update node with new sibling position
        self.node_service.update_node(node_id, update).await?;

        Ok(())
    }

    // =========================================================================
    // DELETE Operations
    // =========================================================================

    /// Delete a node
    ///
    /// Simple delegation to NodeService with cascade handling.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to delete
    ///
    /// # Returns
    ///
    /// DeleteResult indicating whether the node existed
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// let result = operations.delete_node("node-id").await?;
    /// println!("Node existed: {}", result.existed);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_node(&self, node_id: &str) -> Result<DeleteResult, NodeOperationError> {
        Ok(self.node_service.delete_node(node_id).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DatabaseService;
    use serde_json::json;
    use tempfile::TempDir;

    /// Helper to create a test database and NodeOperations instance
    async fn setup_test_operations() -> Result<(NodeOperations, TempDir), Box<dyn std::error::Error>>
    {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = DatabaseService::new(db_path).await?;
        let node_service = NodeService::new(db)?;
        let operations = NodeOperations::new(Arc::new(node_service));
        Ok((operations, temp_dir))
    }

    #[tokio::test]
    async fn test_operations_creation() {
        let result = setup_test_operations().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_operations_cloning() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();
        let _cloned = operations.clone();

        // Both instances should work independently
        // (Full functionality tests will be added in Phase 4)
        // No assertion needed - just verifying clone compiles and works
    }

    // =========================================================================
    // Phase 2: Container Type Detection Tests
    // =========================================================================

    #[test]
    fn test_can_be_container_type_date() {
        assert!(NodeOperations::can_be_container_type("date"));
    }

    #[test]
    fn test_can_be_container_type_text() {
        assert!(NodeOperations::can_be_container_type("text"));
    }

    #[test]
    fn test_can_be_container_type_header() {
        assert!(NodeOperations::can_be_container_type("header"));
    }

    #[test]
    fn test_can_be_container_type_task_is_not_container() {
        assert!(!NodeOperations::can_be_container_type("task"));
    }

    #[test]
    fn test_can_be_container_type_unknown_is_not_container() {
        assert!(!NodeOperations::can_be_container_type("unknown"));
        assert!(!NodeOperations::can_be_container_type("custom-type"));
    }

    // =========================================================================
    // Critical Integration Tests for Business Rules
    // =========================================================================

    #[tokio::test]
    async fn test_container_inference_from_parent() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create a date container
        let date_id = operations
            .create_node(
                "date".to_string(),
                "2025-01-03".to_string(),
                None,
                None,
                None,
                json!({}),
            )
            .await
            .unwrap();

        // Create a child WITHOUT container_node_id - should infer from parent
        let child_id = operations
            .create_node(
                "text".to_string(),
                "Child content".to_string(),
                Some(date_id.clone()),
                None, // No container_node_id provided
                None,
                json!({}),
            )
            .await
            .unwrap();

        // Verify container was inferred from parent
        let child = operations.get_node(&child_id).await.unwrap().unwrap();
        assert_eq!(
            child.container_node_id,
            Some(date_id.clone()),
            "Container should be inferred from parent"
        );
        assert_eq!(
            child.parent_id,
            Some(date_id),
            "Parent should be set correctly"
        );
    }

    #[tokio::test]
    async fn test_parent_container_mismatch_error() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create two separate date containers
        let date1 = operations
            .create_node(
                "date".to_string(),
                "2025-01-03".to_string(),
                None,
                None,
                None,
                json!({}),
            )
            .await
            .unwrap();

        let date2 = operations
            .create_node(
                "date".to_string(),
                "2025-01-04".to_string(),
                None,
                None,
                None,
                json!({}),
            )
            .await
            .unwrap();

        // Create a parent in date1 container
        let parent = operations
            .create_node(
                "text".to_string(),
                "Parent in date1".to_string(),
                None,
                Some(date1.clone()),
                None,
                json!({}),
            )
            .await
            .unwrap();

        // Try to create child with different container than parent - should fail
        let result = operations
            .create_node(
                "text".to_string(),
                "Child in date2".to_string(),
                Some(parent.clone()),
                Some(date2.clone()), // Different container!
                None,
                json!({}),
            )
            .await;

        // Verify error is ParentContainerMismatch
        assert!(
            result.is_err(),
            "Should error when parent and child have different containers"
        );
        let error = result.unwrap_err();
        assert!(
            matches!(error, NodeOperationError::ParentContainerMismatch { .. }),
            "Error should be ParentContainerMismatch, got: {:?}",
            error
        );
    }

    #[tokio::test]
    async fn test_sibling_chain_ordering() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let date = operations
            .create_node(
                "date".to_string(),
                "2025-01-03".to_string(),
                None,
                None,
                None,
                json!({}),
            )
            .await
            .unwrap();

        // Create first node (will be last in chain since no before_sibling_id)
        let first = operations
            .create_node(
                "text".to_string(),
                "First".to_string(),
                None,
                Some(date.clone()),
                None, // No before_sibling_id = goes to end
                json!({}),
            )
            .await
            .unwrap();

        // Create second node (also goes to end, after first)
        let second = operations
            .create_node(
                "text".to_string(),
                "Second".to_string(),
                None,
                Some(date.clone()),
                None, // No before_sibling_id = goes to end
                json!({}),
            )
            .await
            .unwrap();

        // Create third node BEFORE second (so ordering becomes: first → third → second)
        let third = operations
            .create_node(
                "text".to_string(),
                "Third".to_string(),
                None,
                Some(date.clone()),
                Some(second.clone()), // Insert before second
                json!({}),
            )
            .await
            .unwrap();

        // Verify ordering
        let first_node = operations.get_node(&first).await.unwrap().unwrap();
        let second_node = operations.get_node(&second).await.unwrap().unwrap();
        let third_node = operations.get_node(&third).await.unwrap().unwrap();

        // first has no before_sibling_id (it's first in chain)
        assert_eq!(
            first_node.before_sibling_id, None,
            "First node should have no before_sibling_id"
        );

        // third comes before second
        assert_eq!(
            third_node.before_sibling_id,
            Some(second.clone()),
            "Third node should come before second"
        );

        // second should still have no before_sibling_id (it's at the end)
        assert_eq!(
            second_node.before_sibling_id, None,
            "Second node should have no before_sibling_id (end of chain)"
        );
    }
}
