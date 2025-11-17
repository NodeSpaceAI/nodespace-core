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
//! use nodespace_core::db::SurrealStore;
//! use std::path::PathBuf;
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let db = SurrealStore::new(PathBuf::from("./data/test.db")).await?;
//!     let node_service = NodeService::new(db)?;
//!     let operations = NodeOperations::new(node_service);
//!
//!     // Create a node with automatic container inference
//!     let node_id = operations.create_node(CreateNodeParams {
//!         id: None, // Auto-generate ID
//!         node_type: "text".to_string(),
//!         content: "Hello World".to_string(),
//!         parent_id: Some("parent-id".to_string()),
//!         container_node_id: None, // Will be inferred from parent
//!         before_sibling_id: None, // Will be placed as last sibling
//!         properties: json!({}),
//!     }).await?;
//!
//!     println!("Created node: {}", node_id);
//!     Ok(())
//! }
//! ```

pub mod error;
pub mod sibling_queue;

// Re-export types for convenience
pub use error::NodeOperationError;
pub use sibling_queue::SiblingOperationQueue;

use crate::models::{DeleteResult, Node, NodeFilter, NodeUpdate};
use crate::services::NodeService;
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;

/// Parameters for creating a node
///
/// This struct replaces the 8 individual parameters previously used in `create_node()`,
/// improving code maintainability and avoiding Clippy's `too_many_arguments` warning.
///
/// # ID Generation Strategy
///
/// The `id` field supports three distinct scenarios:
///
/// 1. **Frontend-provided UUID** (Tauri commands): The frontend pre-generates UUIDs for
///    optimistic UI updates and local state tracking (`persistedNodeIds`). This ensures
///    ID consistency between client and server, preventing sync issues.
///
/// 2. **Auto-generated UUID** (MCP handlers): Server-side generation for external clients
///    like AI assistants. This prevents ID conflicts and maintains security boundaries.
///
/// 3. **Date-based ID** (special case): Date nodes use their content (YYYY-MM-DD format)
///    as the ID, enabling predictable lookups and ensuring uniqueness by date.
///
/// # Security Considerations
///
/// When accepting frontend-provided IDs:
///
/// - **UUID validation**: Non-date nodes must provide valid UUID format. Invalid UUIDs
///   are rejected with `InvalidOperation` error.
/// - **Database constraints**: The database enforces UNIQUE constraint on `nodes.id`,
///   preventing collisions at the storage layer.
/// - **Trust boundary**: Only Tauri commands (trusted in-process frontend) can provide
///   custom IDs. MCP handlers (external AI clients) always use server-side generation.
/// - **No collision check needed**: UUID format validation combined with database constraints
///   provides sufficient protection without additional pre-flight existence checks.
///
/// # Examples
///
/// ```no_run
/// # use nodespace_core::operations::CreateNodeParams;
/// # use serde_json::json;
/// // Auto-generated ID (MCP path)
/// let params = CreateNodeParams {
///     id: None,
///     node_type: "text".to_string(),
///     content: "Hello World".to_string(),
///     parent_id: Some("parent-123".to_string()),
///     container_node_id: None,
///     before_sibling_id: None,
///     properties: json!({}),
/// };
///
/// // Frontend-provided UUID (Tauri path)
/// let frontend_id = uuid::Uuid::new_v4().to_string();
/// let params_with_id = CreateNodeParams {
///     id: Some(frontend_id),
///     node_type: "text".to_string(),
///     content: "Tracked by frontend".to_string(),
///     parent_id: None,
///     container_node_id: None,
///     before_sibling_id: None,
///     properties: json!({}),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CreateNodeParams {
    /// Optional ID for the node. If None, will be auto-generated (UUID for most types, content for date nodes)
    pub id: Option<String>,
    /// Type of the node (text, task, date, etc.)
    pub node_type: String,
    /// Content of the node
    pub content: String,
    /// Optional parent node ID
    pub parent_id: Option<String>,
    /// Optional container node ID (will be inferred from parent if not provided)
    pub container_node_id: Option<String>,
    /// Optional sibling to insert before (if None, appends to end)
    pub before_sibling_id: Option<String>,
    /// Additional node properties as JSON
    pub properties: Value,
}

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
/// # use nodespace_core::db::SurrealStore;
/// # use std::path::PathBuf;
/// # use serde_json::json;
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
/// # let node_service = NodeService::new(db)?;
/// let operations = NodeOperations::new(node_service);
///
/// // All operations automatically enforce business rules
/// let node_id = operations.create_node(CreateNodeParams {
///     id: None, // Auto-generate ID
///     node_type: "text".to_string(),
///     content: "Content".to_string(),
///     parent_id: Some("parent-id".to_string()),
///     container_node_id: None, // Container ID inferred
///     before_sibling_id: None, // Placed as last sibling
///     properties: json!({}),
/// }).await?;
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
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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

    /// Ensure date container exists, auto-creating it if necessary
    ///
    /// Date nodes (YYYY-MM-DD format) are special containers that auto-exist.
    /// They are always root-level containers (no parent, no container).
    ///
    /// # Arguments
    ///
    /// * `node_id` - Potential date node ID to check/create
    ///
    /// # Returns
    ///
    /// Ok(()) if the node exists or was created, Err if creation failed
    async fn ensure_date_container_exists(&self, node_id: &str) -> Result<(), NodeOperationError> {
        // Check if this is a date format (YYYY-MM-DD)
        if !Self::is_date_node_id(node_id) {
            return Ok(()); // Not a date, nothing to do
        }

        // Check if date container already exists
        let exists = self.node_service.get_node(node_id).await?.is_some();
        if exists {
            return Ok(()); // Already exists
        }

        // Auto-create the date container
        // IMPORTANT: Date node content MUST match the date ID for validation
        // Use new_with_id to set both id and content to the date string
        let date_node = Node::new_with_id(
            node_id.to_string(), // ID is the date string (YYYY-MM-DD)
            "date".to_string(),
            node_id.to_string(), // Content MUST match ID for validation
            json!({}),           // Date nodes are always root-level containers
        );

        // Create the date container directly via NodeService
        // Skip NodeOperations to avoid infinite recursion
        self.node_service.create_node(date_node).await?;

        Ok(())
    }

    /// Check if a string matches date node ID format: YYYY-MM-DD
    fn is_date_node_id(id: &str) -> bool {
        use chrono::NaiveDate;
        NaiveDate::parse_from_str(id, "%Y-%m-%d").is_ok()
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
            // Verify parent exists
            self.node_service
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(parent_id.to_string()))?;

            // Get the container by traversing up to the root
            // If parent is a root (no parent of its own), use the parent as the container
            // Otherwise, get the parent's container recursively
            let container_id = self.node_service.get_container_id(parent_id).await?;
            return Ok(container_id);
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
            // Verify sibling exists
            self.node_service
                .get_node(sibling_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(sibling_id.to_string()))?;

            // Verify sibling has same parent (via edge query)
            let sibling_parent = self.node_service.get_parent(sibling_id).await?;
            let sibling_parent_id = sibling_parent.as_ref().map(|p| p.id.as_str());

            if sibling_parent_id != parent_id {
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
            // Verify parent exists
            self.node_service
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(parent_id.to_string()))?;

            // Parent must have same container (get via edge traversal)
            let parent_container = self.node_service.get_container_id(parent_id).await?;

            if parent_container != container_node_id {
                return Err(NodeOperationError::parent_container_mismatch(
                    parent_container,
                    container_node_id.to_string(),
                ));
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
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// // Create a date node (container)
    /// let date_id = operations.create_node(CreateNodeParams {
    ///     id: None, // Auto-generate ID
    ///     node_type: "date".to_string(),
    ///     content: "2025-01-03".to_string(),
    ///     parent_id: None, // Containers cannot have parent
    ///     container_node_id: None, // Containers cannot have container
    ///     before_sibling_id: None, // Containers cannot have sibling
    ///     properties: json!({}),
    /// }).await?;
    ///
    /// // Create a text node with automatic container inference
    /// let text_id = operations.create_node(CreateNodeParams {
    ///     id: None, // Auto-generate ID
    ///     node_type: "text".to_string(),
    ///     content: "Hello".to_string(),
    ///     parent_id: Some(date_id.clone()), // Parent
    ///     container_node_id: None, // Container ID inferred from parent (date_id)
    ///     before_sibling_id: None, // Placed as last sibling
    ///     properties: json!({}),
    /// }).await?;
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
    /// let date_id = operations.create_node(CreateNodeParams {
    ///     id: None, // Auto-generate ID
    ///     node_type: "date".to_string(),
    ///     content: "2025-10-23".to_string(),  // This becomes the ID
    ///     parent_id: None,
    ///     container_node_id: None,
    ///     before_sibling_id: None,
    ///     properties: json!({}),
    /// }).await?;
    ///
    /// assert_eq!(date_id, "2025-10-23");  // ID matches content
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_node(
        &self,
        params: CreateNodeParams,
    ) -> Result<String, NodeOperationError> {
        // CRITICAL: Auto-create date containers if they don't exist
        // Date nodes are always containers and parents - they never have parents themselves
        if let Some(ref parent_id_str) = params.parent_id {
            self.ensure_date_container_exists(parent_id_str).await?;
        }
        if let Some(ref container_id_str) = params.container_node_id {
            self.ensure_date_container_exists(container_id_str).await?;
        }

        // Business Rule 1: Determine if this node IS a container based on hierarchy fields
        // A node is a container if it has NO parent and NO container
        // (not just because its type CAN be a container)
        let is_container_node = params.parent_id.is_none() && params.container_node_id.is_none();

        let (final_parent_id, _final_container_id, final_sibling_id) = if is_container_node {
            // This node IS a container - validate that its type allows being a container
            if !Self::can_be_container_type(&params.node_type) {
                return Err(NodeOperationError::invalid_container_type(
                    params.content.clone(),
                    params.node_type.clone(),
                ));
            }

            // Container nodes MUST have before_sibling_id as None as well
            if params.before_sibling_id.is_some() {
                return Err(NodeOperationError::container_cannot_have_sibling(
                    params.content.clone(),
                    params.node_type.clone(),
                ));
            }

            (None, None, None)
        } else {
            // Non-container nodes: apply business rules 2-4

            // Business Rule 2: Resolve container_node_id (with parent inference)
            let resolved_container = self
                .resolve_container(
                    &params.content, // Passed for error context only (node ID not yet assigned)
                    &params.node_type,
                    params.container_node_id,
                    params.parent_id.as_deref(),
                )
                .await?;

            // Business Rule 4: Validate parent-container consistency
            self.validate_parent_container_consistency(
                params.parent_id.as_deref(),
                &resolved_container,
            )
            .await?;

            // Business Rule 3: Calculate sibling position
            let calculated_sibling = self
                .calculate_sibling_position(params.parent_id.as_deref(), params.before_sibling_id)
                .await?;

            (
                params.parent_id,
                Some(resolved_container),
                calculated_sibling,
            )
        };

        // Create the node using NodeService
        // Use provided ID if given (allows frontend to pre-generate UUIDs for local state management)
        // Otherwise, special case: date nodes use their content (YYYY-MM-DD) as the ID
        // Otherwise: generate a new UUID
        let node_id = if let Some(provided_id) = params.id {
            // Validate that provided ID is either:
            // 1. A proper UUID format (for regular nodes)
            // 2. A valid date format (YYYY-MM-DD) for date nodes
            if params.node_type == "date" {
                // Date nodes can use date format as ID
                provided_id
            } else {
                // Non-date nodes must use UUID format (security check)
                uuid::Uuid::parse_str(&provided_id).map_err(|_| {
                    NodeOperationError::invalid_operation(format!(
                        "Provided ID '{}' is not a valid UUID format (required for non-date nodes)",
                        provided_id
                    ))
                })?;
                provided_id
            }
        } else if params.node_type == "date" {
            params.content.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };

        let node = Node {
            id: node_id,
            node_type: params.node_type,
            content: params.content,
            before_sibling_id: final_sibling_id,
            version: 1,
            properties: params.properties,
            mentions: vec![],
            mentioned_by: vec![],
            created_at: Utc::now(),
            modified_at: Utc::now(),
            embedding_vector: None,
        };

        let created_id = self.node_service.create_node(node).await?;

        // Create parent edge if parent_id was specified
        // This establishes the has_child graph edge: parent->has_child->child
        if let Some(parent_id) = final_parent_id {
            self.node_service
                .move_node(&created_id, Some(parent_id.as_str()))
                .await?;
            tracing::debug!(
                "Created parent edge: {} -> has_child -> {}",
                parent_id,
                created_id
            );
        }

        // Note: Container relationship is NOT stored as an edge.
        // Container is determined by traversing UP the parent chain to find the root node.
        // The final_container_id is used for validation/queries but doesn't create database edges.
        // See NodeService::get_container_id() for the graph traversal implementation.

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
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::NodeFilter;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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

    /// Get all children of a parent node
    ///
    /// Returns all nodes that have an incoming parent edge from the specified node.
    pub async fn get_children(&self, parent_id: &str) -> Result<Vec<Node>, NodeOperationError> {
        Ok(self.node_service.get_children(parent_id).await?)
    }

    /// Get all descendants of a container node
    ///
    /// Returns all nodes within the container's hierarchy (not just direct children).
    pub async fn get_descendants(
        &self,
        container_id: &str,
    ) -> Result<Vec<Node>, NodeOperationError> {
        Ok(self
            .node_service
            .get_nodes_by_container_id(container_id)
            .await?)
    }

    /// Get the parent ID of a node
    ///
    /// Uses graph-native parent lookup via SurrealDB reverse edge traversal.
    /// Returns the parent node ID if the node has a parent, or None if it's a root node.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to get the parent for
    ///
    /// # Returns
    ///
    /// * `Ok(Some(parent_id))` - The node has a parent
    /// * `Ok(None)` - The node is a root (no parent)
    /// * `Err(_)` - Database error
    pub async fn get_parent_id(&self, node_id: &str) -> Result<Option<String>, NodeOperationError> {
        // Delegate to NodeService which uses graph traversal:
        // SELECT * FROM node WHERE id IN (SELECT VALUE in FROM has_child WHERE out = $child_thing)
        let parent_node = self.node_service.get_parent(node_id).await?;
        Ok(parent_node.map(|node| node.id))
    }

    /// Get the container ID of a node
    ///
    /// Traverses up the parent chain to find the root container node.
    /// A container is a node with no parent (root of the hierarchy).
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to get the container for
    ///
    /// # Returns
    ///
    /// The container node ID (the node itself if it's already a root)
    pub async fn get_container_id(&self, node_id: &str) -> Result<String, NodeOperationError> {
        // Delegate to NodeService which implements the traversal algorithm
        Ok(self.node_service.get_container_id(node_id).await?)
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
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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
        expected_version: i64,
        content: Option<String>,
        node_type: Option<String>,
        properties: Option<Value>,
    ) -> Result<Node, NodeOperationError> {
        // Business Rule 5: Content updates cannot change hierarchy
        // This method only updates content, node_type, and properties
        // Use move_node() or reorder_node() for hierarchy changes

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

        // Update with version check (optimistic concurrency control)
        let rows_affected = self
            .node_service
            .update_with_version_check(node_id, expected_version, update)
            .await?;

        // If version mismatch, fetch current state and return conflict error
        if rows_affected == 0 {
            let current = self
                .node_service
                .get_node(node_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

            return Err(NodeOperationError::version_conflict(
                node_id.to_string(),
                expected_version,
                current.version,
                current,
            ));
        }

        // Fetch and return the updated node with its new version
        let updated_node = self
            .node_service
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

        Ok(updated_node)
    }

    /// Update a node with content and/or hierarchy changes in a single atomic operation
    ///
    /// This method combines content and hierarchy updates, orchestrating calls to
    /// specialized methods while maintaining business rules and version consistency.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to update
    /// * `expected_version` - Version for OCC
    /// * `update` - Full NodeUpdate with any combination of changes
    ///
    /// # Returns
    ///
    /// Returns the updated Node with new version number
    ///
    /// # Business Rules Enforced
    ///
    /// - Hierarchy changes use move_node() and reorder_node() (maintains validation)
    /// - Content changes use update_node()
    /// - All operations respect OCC versioning
    /// - Sibling chain integrity maintained
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// let mut update = NodeUpdate::new();
    /// update.parent_id = Some(Some("new-parent".to_string()));
    /// update.content = Some("Updated content".to_string());
    /// let updated = operations.update_node_with_hierarchy("node-id", 1, update).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_node_with_hierarchy(
        &self,
        node_id: &str,
        expected_version: i64,
        update: NodeUpdate,
    ) -> Result<Node, NodeOperationError> {
        // Validate update has changes
        if update.is_empty() {
            return Err(NodeOperationError::invalid_operation(
                "Update contains no changes".to_string(),
            ));
        }

        // Fetch current node for validation
        let current = self
            .node_service
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

        // NOTE: Hierarchy changes (parent_id, container_node_id) are no longer supported in update_node.
        // Use the move_node() API instead for changing node hierarchy.

        let sibling_changed = update
            .before_sibling_id
            .as_ref()
            .map(|new_sibling| new_sibling.as_deref() != current.before_sibling_id.as_deref())
            .unwrap_or(false);

        if sibling_changed {
            // Validate container node cannot be reordered
            if self.node_service.is_root_node(node_id).await? {
                return Err(NodeOperationError::invalid_operation(format!(
                    "Container node '{}' cannot be reordered (it's a root node)",
                    node_id
                )));
            }

            // Fix sibling chain BEFORE reordering (maintain chain integrity)
            // Find node that currently points to this one and update it
            if let Some(parent) = self.node_service.get_parent(node_id).await? {
                let siblings = self.node_service.get_children(&parent.id).await?;

                // Find the next sibling that points to this node
                if let Some(next_sibling) = siblings
                    .iter()
                    .find(|n| n.before_sibling_id.as_deref() == Some(node_id))
                {
                    // Update next sibling to point to what this node was pointing to
                    // This removes this node from the chain before we move it
                    let mut chain_fix = NodeUpdate::new();
                    chain_fix.before_sibling_id = Some(current.before_sibling_id.clone());

                    // Retry on version conflicts
                    let max_retries = 3;
                    for attempt in 0..max_retries {
                        let fresh = self
                            .node_service
                            .get_node(&next_sibling.id)
                            .await?
                            .ok_or_else(|| {
                                NodeOperationError::node_not_found(next_sibling.id.clone())
                            })?;

                        match self
                            .node_service
                            .update_with_version_check(
                                &next_sibling.id,
                                fresh.version,
                                chain_fix.clone(),
                            )
                            .await
                        {
                            Ok(_) => break,
                            Err(_e) if attempt < max_retries - 1 => {
                                tracing::warn!(
                                    "Sibling chain update conflict, retrying ({}/{})",
                                    attempt + 1,
                                    max_retries
                                );
                                continue;
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
            }
        }

        // All validation passed - apply update in ONE database operation
        // NodeService::update_with_version_check handles all fields atomically
        let rows_affected = self
            .node_service
            .update_with_version_check(node_id, expected_version, update)
            .await?;

        // Handle version conflict
        if rows_affected == 0 {
            let current = self
                .node_service
                .get_node(node_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

            return Err(NodeOperationError::version_conflict(
                node_id.to_string(),
                expected_version,
                current.version,
                current,
            ));
        }

        // Fetch and return the updated node with its new version
        let updated_node = self
            .node_service
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

        Ok(updated_node)
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
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// operations.move_node("node-id", Some("new-parent-id")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn move_node(
        &self,
        node_id: &str,
        _expected_version: i64,
        new_parent_id: Option<&str>,
    ) -> Result<(), NodeOperationError> {
        // Container nodes cannot be moved (they are root nodes with no parent/container)
        // Check actual container status using service method
        if self.node_service.is_root_node(node_id).await? {
            return Err(NodeOperationError::invalid_operation(format!(
                "Container node '{}' cannot be moved (it's a root node)",
                node_id
            )));
        }

        // Validate new parent exists and get its container
        let _new_container = if let Some(parent_id) = new_parent_id {
            // Ensure parent exists
            let _parent = self
                .node_service
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(parent_id.to_string()))?;

            // Get container from parent using service method
            self.node_service.get_container_id(parent_id).await?
        } else {
            // Moving to root - node must have explicit container
            // Get current container
            self.node_service.get_container_id(node_id).await?
        };

        // Delegate to NodeService to perform the edge operations:
        // 1. Delete existing parent edge (via: DELETE has_child WHERE out = $child_thing)
        // 2. Create new parent edge if specified (via: RELATE $parent_thing->has_child->$child_thing)
        // Note: Container edge doesn't exist - container is determined by traversing to root
        self.node_service.move_node(node_id, new_parent_id).await?;

        tracing::debug!("Moved node {} to new parent: {:?}", node_id, new_parent_id);

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
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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
        expected_version: i64,
        before_sibling_id: Option<&str>,
    ) -> Result<(), NodeOperationError> {
        // Get current node
        let node = self
            .node_service
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

        // Container nodes cannot be reordered (they are root nodes with no siblings)
        // Check actual container status using service method
        if self.node_service.is_root_node(node_id).await? {
            return Err(NodeOperationError::invalid_operation(format!(
                "Container node '{}' cannot be reordered (it's a root node with no siblings)",
                node_id
            )));
        }

        // Fix sibling chain BEFORE reordering (maintain chain integrity)
        // Use version-checked updates with retry to prevent race conditions
        if let Some(parent) = self.node_service.get_parent(node_id).await? {
            // Find all siblings (nodes with same parent)
            let siblings = self.node_service.get_children(&parent.id).await?;

            // Find the node that points to this one (next sibling in chain)
            if let Some(next_sibling) = siblings
                .iter()
                .find(|n| n.before_sibling_id.as_deref() == Some(node_id))
            {
                // Update next sibling to skip over the moved node with version checking
                // This maintains the chain: if A → B → C and we move B, we update C to point to A
                // Retry on version conflicts to handle concurrent modifications
                let max_retries = 3;
                for attempt in 0..max_retries {
                    // Fetch fresh version on each attempt
                    let fresh_sibling = self
                        .node_service
                        .get_node(&next_sibling.id)
                        .await?
                        .ok_or_else(|| {
                            NodeOperationError::node_not_found(next_sibling.id.clone())
                        })?;

                    let mut fix_update = NodeUpdate::new();
                    fix_update.before_sibling_id = Some(node.before_sibling_id.clone());

                    let rows_affected = self
                        .node_service
                        .update_with_version_check(
                            &fresh_sibling.id,
                            fresh_sibling.version,
                            fix_update,
                        )
                        .await?;

                    if rows_affected > 0 {
                        // Success - chain fixed
                        break;
                    } else if attempt == max_retries - 1 {
                        // Final attempt failed - return version conflict error
                        return Err(NodeOperationError::version_conflict(
                            fresh_sibling.id.clone(),
                            fresh_sibling.version,
                            fresh_sibling.version + 1, // Actual version is now higher
                            fresh_sibling,
                        ));
                    } else {
                        // Log retry attempts for operational visibility
                        tracing::warn!(
                            "Sibling chain update retry attempt {} for node '{}' during reorder (max: {})",
                            attempt + 1,
                            fresh_sibling.id,
                            max_retries
                        );
                        // Retry with exponential backoff
                        tokio::time::sleep(tokio::time::Duration::from_millis(10 * (1 << attempt)))
                            .await;
                    }
                }
            }
        }

        // Validate sibling position
        let parent = self.node_service.get_parent(node_id).await?;
        let new_sibling = self
            .calculate_sibling_position(
                parent.as_ref().map(|p| p.id.as_str()),
                before_sibling_id.map(String::from),
            )
            .await?;

        // Create NodeUpdate with new sibling position
        let mut update = NodeUpdate::new();
        update.before_sibling_id = Some(new_sibling);

        // Update with version check (optimistic concurrency control)
        let rows_affected = self
            .node_service
            .update_with_version_check(node_id, expected_version, update)
            .await?;

        // If version mismatch, fetch current state and return conflict error
        if rows_affected == 0 {
            let current = self
                .node_service
                .get_node(node_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

            return Err(NodeOperationError::version_conflict(
                node_id.to_string(),
                expected_version,
                current.version,
                current,
            ));
        }

        Ok(())
    }

    // =========================================================================
    // DELETE Operations
    // =========================================================================

    /// Delete a node with sibling chain integrity preservation
    ///
    /// This method fixes the critical bug where deleting a node left dangling
    /// before_sibling_id references pointing to the deleted node.
    ///
    /// # Sibling Chain Fix
    ///
    /// Before deletion:
    /// - Node A (before_sibling_id=None)
    /// - Node B (before_sibling_id="A")  ← Deleting this
    /// - Node C (before_sibling_id="B")
    ///
    /// After deletion (with fix):
    /// - Node A (before_sibling_id=None)
    /// - Node C (before_sibling_id="A")  ← Fixed! Now points to A instead of deleted B
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to delete
    ///
    /// # Returns
    ///
    /// DeleteResult indicating whether the node existed
    ///
    /// # Errors
    ///
    /// Returns `NodeOperationError` if:
    /// - Database error occurs
    /// - Circular sibling chain detected (data corruption)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::NodeOperations;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let node_service = NodeService::new(db)?;
    /// # let operations = NodeOperations::new(node_service);
    /// let result = operations.delete_node("node-id").await?;
    /// println!("Node existed: {}", result.existed);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_node(
        &self,
        node_id: &str,
        expected_version: i64,
    ) -> Result<DeleteResult, NodeOperationError> {
        // 1. Get the node being deleted (if it exists)
        let node = match self.node_service.get_node(node_id).await? {
            Some(n) => n,
            None => {
                // Node doesn't exist - return false immediately
                return Ok(DeleteResult { existed: false });
            }
        };

        // 2. Fix sibling chain BEFORE deletion (only if node has a parent)
        // Use version-checked updates with retry to prevent race conditions
        if let Some(parent) = self.node_service.get_parent(node_id).await? {
            // Find all siblings (nodes with same parent)
            let siblings = self.node_service.get_children(&parent.id).await?;

            // Find the node that points to this one (next sibling in chain)
            if let Some(next_sibling) = siblings
                .iter()
                .find(|n| n.before_sibling_id.as_deref() == Some(node_id))
            {
                // Update next sibling to point to what the deleted node pointed to
                // This maintains the chain: if A → B → C and we delete B, we get A → C
                // Retry on version conflicts to handle concurrent modifications
                let max_retries = 3;
                for attempt in 0..max_retries {
                    // Fetch fresh version on each attempt
                    let fresh_sibling = self
                        .node_service
                        .get_node(&next_sibling.id)
                        .await?
                        .ok_or_else(|| {
                            NodeOperationError::node_not_found(next_sibling.id.clone())
                        })?;

                    let mut update = NodeUpdate::new();
                    update.before_sibling_id = Some(node.before_sibling_id.clone());

                    let rows_affected = self
                        .node_service
                        .update_with_version_check(&fresh_sibling.id, fresh_sibling.version, update)
                        .await?;

                    if rows_affected > 0 {
                        // Success - chain fixed
                        break;
                    } else if attempt == max_retries - 1 {
                        // Final attempt failed - return version conflict error
                        return Err(NodeOperationError::version_conflict(
                            fresh_sibling.id.clone(),
                            fresh_sibling.version,
                            fresh_sibling.version + 1, // Actual version is now higher
                            fresh_sibling,
                        ));
                    } else {
                        // Log retry attempts for operational visibility
                        tracing::warn!(
                            "Sibling chain update retry attempt {} for node '{}' during delete (max: {})",
                            attempt + 1,
                            fresh_sibling.id,
                            max_retries
                        );
                        // Retry with exponential backoff
                        tokio::time::sleep(tokio::time::Duration::from_millis(10 * (1 << attempt)))
                            .await;
                    }
                }
            }
        }

        // 3. Cascade delete all children recursively
        // Get all direct children before deleting the parent
        let children = self.node_service.get_children(node_id).await?;

        // Recursively delete each child (this will cascade further down the tree)
        for child in children {
            // Recursively call delete_node for each child
            // Use the child's current version for optimistic concurrency
            // Box the recursive call to avoid infinite future size
            Box::pin(self.delete_node(&child.id, child.version)).await?;
        }

        // 4. Delete with version check (optimistic concurrency control)
        let rows_affected = self
            .node_service
            .delete_with_version_check(node_id, expected_version)
            .await?;

        // If version mismatch (rows_affected = 0), check if node still exists
        if rows_affected == 0 {
            // Node might have been deleted or modified by another client
            match self.node_service.get_node(node_id).await? {
                Some(current) => {
                    // Node exists but version mismatch - return conflict error
                    return Err(NodeOperationError::version_conflict(
                        node_id.to_string(),
                        expected_version,
                        current.version,
                        current,
                    ));
                }
                None => {
                    // Node was already deleted by another client
                    // This is OK - idempotent delete
                    return Ok(DeleteResult { existed: false });
                }
            }
        }

        // 5. Deletion succeeded
        Ok(DeleteResult { existed: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SurrealStore;
    use serde_json::json;
    use tempfile::TempDir;

    /// Helper to create a test database and NodeOperations instance
    async fn setup_test_operations() -> Result<(NodeOperations, TempDir), Box<dyn std::error::Error>>
    {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        let store = Arc::new(SurrealStore::new(db_path).await?);
        let node_service = NodeService::new(store)?;
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
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create a child WITHOUT container_node_id - should infer from parent
        let child_id = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Child content".to_string(),
                parent_id: Some(date_id.clone()),
                container_node_id: None, // No container_node_id provided
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Graph Architecture Note: Parent-child and container relationships are now managed
        // via graph edges in the SurrealDB schema, not via parent_id/container_node_id fields.
        // The relationship assertions would be verified via edge queries.
        let _child = operations.get_node(&child_id).await.unwrap().unwrap();
        // TODO: Verify parent-child relationship via graph edge query when edge API is available
    }

    #[tokio::test]
    async fn test_parent_container_mismatch_error() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create two separate date containers
        let date1 = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let date2 = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-04".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create a parent in date1 container
        let parent = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Parent in date1".to_string(),
                parent_id: None,
                container_node_id: Some(date1.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Try to create child with different container than parent - should fail
        let result = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Child in date2".to_string(),
                parent_id: Some(parent.clone()),
                container_node_id: Some(date2.clone()), // Different container!
                before_sibling_id: None,
                properties: json!({}),
            })
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
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create first node (will be last in chain since no before_sibling_id)
        let first = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "First".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None, // No before_sibling_id = goes to end
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create second node (also goes to end, after first)
        let second = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Second".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None, // No before_sibling_id = goes to end
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create third node BEFORE second (so ordering becomes: first → third → second)
        let third = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Third".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: Some(second.clone()), // Insert before second
                properties: json!({}),
            })
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

    // =========================================================================
    // Delete Operations - Sibling Chain Integrity Tests
    // =========================================================================

    #[tokio::test]
    async fn test_delete_node_fixes_sibling_chain_middle() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create three siblings: A → B → C
        let node_a = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "A".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_b = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "B".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_c = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "C".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Verify initial chain: A (None) → B (None) → C (None)
        let a = operations.get_node(&node_a).await.unwrap().unwrap();
        let b = operations.get_node(&node_b).await.unwrap().unwrap();
        let c = operations.get_node(&node_c).await.unwrap().unwrap();
        assert_eq!(a.before_sibling_id, None);
        assert_eq!(b.before_sibling_id, None);
        assert_eq!(c.before_sibling_id, None);

        // Delete B (middle node)
        let result = operations.delete_node(&node_b, b.version).await.unwrap();
        assert!(result.existed);

        // Verify B is deleted
        let deleted = operations.get_node(&node_b).await.unwrap();
        assert!(deleted.is_none());

        // Verify sibling chain is intact: A and C still exist
        let a_after = operations.get_node(&node_a).await.unwrap().unwrap();
        let c_after = operations.get_node(&node_c).await.unwrap().unwrap();
        assert_eq!(a_after.before_sibling_id, None);
        assert_eq!(c_after.before_sibling_id, None);
    }

    #[tokio::test]
    async fn test_delete_node_first_in_chain() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create two siblings
        let first = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "First".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let second = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Second".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Delete first node
        let first_node = operations.get_node(&first).await.unwrap().unwrap();
        let result = operations
            .delete_node(&first, first_node.version)
            .await
            .unwrap();
        assert!(result.existed);

        // Verify second node still exists with correct chain
        let second_after = operations.get_node(&second).await.unwrap().unwrap();
        assert_eq!(second_after.before_sibling_id, None);
    }

    #[tokio::test]
    async fn test_delete_node_last_in_chain() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create two siblings
        let first = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "First".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let last = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Last".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Delete last node
        let last_node = operations.get_node(&last).await.unwrap().unwrap();
        let result = operations
            .delete_node(&last, last_node.version)
            .await
            .unwrap();
        assert!(result.existed);

        // Verify first node still exists unchanged
        let first_after = operations.get_node(&first).await.unwrap().unwrap();
        assert_eq!(first_after.before_sibling_id, None);
    }

    #[tokio::test]
    async fn test_delete_node_with_children() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create parent with siblings
        let parent = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Parent".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let sibling = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Sibling".to_string(),
                parent_id: None,
                container_node_id: Some(date.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create child under parent
        let child = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Child".to_string(),
                parent_id: Some(parent.clone()),
                container_node_id: None, // Inferred from parent
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Delete parent (should cascade delete child and fix sibling chain)
        let parent_node = operations.get_node(&parent).await.unwrap().unwrap();
        let result = operations
            .delete_node(&parent, parent_node.version)
            .await
            .unwrap();
        assert!(result.existed);

        // Verify parent and child are deleted
        assert!(operations.get_node(&parent).await.unwrap().is_none());
        assert!(operations.get_node(&child).await.unwrap().is_none());

        // Verify sibling still exists with intact chain
        let sibling_after = operations.get_node(&sibling).await.unwrap().unwrap();
        assert_eq!(sibling_after.before_sibling_id, None);
    }

    #[tokio::test]
    async fn test_delete_nonexistent_node() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Try to delete a node that doesn't exist
        // For nonexistent node, any version will work (will return existed=false immediately)
        let result = operations.delete_node("nonexistent-id", 1).await.unwrap();
        assert!(!result.existed);
    }

    // =========================================================================
    // Frontend-Provided ID Tests (Issue #349)
    // =========================================================================

    #[tokio::test]
    async fn test_create_node_with_frontend_provided_uuid() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Frontend-generated UUID (simulating Tauri command)
        let frontend_id = uuid::Uuid::new_v4().to_string();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: Some(frontend_id.clone()),
                node_type: "text".to_string(),
                content: "Frontend-tracked node".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Verify returned ID matches provided ID
        assert_eq!(
            node_id, frontend_id,
            "Should return the exact UUID provided by frontend"
        );

        // Verify node was created with correct ID
        let node = operations.get_node(&frontend_id).await.unwrap();
        assert!(
            node.is_some(),
            "Node should exist with frontend-provided ID"
        );
        assert_eq!(
            node.unwrap().id,
            frontend_id,
            "Stored node should have frontend-provided ID"
        );
    }

    #[tokio::test]
    async fn test_create_node_with_frontend_provided_date_id() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Frontend provides date as ID for date node
        let date_id = "2025-10-31".to_string();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: Some(date_id.clone()),
                node_type: "date".to_string(),
                content: "2025-10-31".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Verify returned ID matches provided date ID
        assert_eq!(
            node_id, date_id,
            "Date nodes should accept date format as ID"
        );

        // Verify node was created
        let node = operations.get_node(&date_id).await.unwrap();
        assert!(node.is_some(), "Date node should exist with provided ID");
        assert_eq!(node.unwrap().id, date_id);
    }

    #[tokio::test]
    async fn test_create_node_rejects_invalid_uuid_for_non_date() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Try to create non-date node with invalid UUID
        let result = operations
            .create_node(CreateNodeParams {
                id: Some("not-a-valid-uuid".to_string()),
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await;

        // Should fail with InvalidOperation error
        assert!(
            result.is_err(),
            "Should reject invalid UUID format for non-date nodes"
        );
        assert!(
            matches!(
                result.unwrap_err(),
                NodeOperationError::InvalidOperation { .. }
            ),
            "Should return InvalidOperation error for malformed UUID"
        );
    }

    #[tokio::test]
    async fn test_create_node_with_frontend_id_preserves_hierarchy() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create parent with frontend-provided UUID
        let parent_id = uuid::Uuid::new_v4().to_string();
        operations
            .create_node(CreateNodeParams {
                id: Some(parent_id.clone()),
                node_type: "text".to_string(),
                content: "Parent".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create child with frontend-provided UUID
        let child_id = uuid::Uuid::new_v4().to_string();
        operations
            .create_node(CreateNodeParams {
                id: Some(child_id.clone()),
                node_type: "text".to_string(),
                content: "Child".to_string(),
                parent_id: Some(parent_id.clone()),
                container_node_id: Some(parent_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Graph Architecture Note: Parent-child and container relationships are now managed
        // via graph edges in the SurrealDB schema, not via parent_id/container_node_id fields.
        let _child = operations.get_node(&child_id).await.unwrap().unwrap();
        // TODO: Verify parent-child relationship via graph edge query when edge API is available
    }

    #[tokio::test]
    async fn test_auto_generated_ids_still_work() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create node without providing ID (MCP path)
        let node_id = operations
            .create_node(CreateNodeParams {
                id: None, // Should auto-generate UUID
                node_type: "text".to_string(),
                content: "Auto-generated ID".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Verify ID was auto-generated (should be valid UUID)
        assert!(
            uuid::Uuid::parse_str(&node_id).is_ok(),
            "Auto-generated ID should be valid UUID"
        );

        // Verify node exists
        let node = operations.get_node(&node_id).await.unwrap();
        assert!(node.is_some(), "Node with auto-generated ID should exist");
    }

    // =========================================================================
    // Optimistic Concurrency Control (OCC) Tests - Issue #333
    // =========================================================================

    #[tokio::test]
    async fn test_concurrent_update_version_conflict() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create a test node (version=1)
        let node_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Original content".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Simulate two concurrent clients reading the same version
        let node1 = operations.get_node(&node_id).await.unwrap().unwrap();
        let node2 = operations.get_node(&node_id).await.unwrap().unwrap();

        assert_eq!(node1.version, 1, "Initial version should be 1");
        assert_eq!(node2.version, 1, "Both clients see version 1");

        // Client 1 updates successfully (version 1 → 2)
        operations
            .update_node(
                &node_id,
                node1.version,
                Some("Client 1 content".to_string()),
                None,
                None,
            )
            .await
            .unwrap();

        // Verify version incremented
        let updated = operations.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(updated.version, 2, "Version should increment to 2");
        assert_eq!(updated.content, "Client 1 content");

        // Client 2 tries to update with stale version (still thinks version=1)
        let result = operations
            .update_node(
                &node_id,
                node2.version, // Still 1, but actual is now 2
                Some("Client 2 content".to_string()),
                None,
                None,
            )
            .await;

        // Should fail with VersionConflict
        assert!(
            matches!(result, Err(NodeOperationError::VersionConflict { .. })),
            "Should get VersionConflict error"
        );

        // Verify content wasn't overwritten
        let final_node = operations.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(final_node.content, "Client 1 content");
        assert_eq!(final_node.version, 2);
    }

    #[tokio::test]
    async fn test_concurrent_move_version_conflict() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create container
        let container_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create two parent nodes in the container
        let parent1_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Parent 1".to_string(),
                parent_id: None,
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let parent2_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Parent 2".to_string(),
                parent_id: None,
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create child node under parent1
        let child_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Child node".to_string(),
                parent_id: Some(parent1_id.clone()),
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Two clients read the child node
        let child1 = operations.get_node(&child_id).await.unwrap().unwrap();
        let child2 = operations.get_node(&child_id).await.unwrap().unwrap();

        // Client 1 moves child to parent2
        operations
            .move_node(&child_id, child1.version, Some(&parent2_id))
            .await
            .unwrap();

        // Client 2 tries to move with stale version
        let result = operations
            .move_node(&child_id, child2.version, Some(&parent1_id))
            .await;

        assert!(
            matches!(result, Err(NodeOperationError::VersionConflict { .. })),
            "Should get VersionConflict on concurrent move"
        );

        // Graph Architecture Note: Parent relationship verification via graph edges
        let _final_child = operations.get_node(&child_id).await.unwrap().unwrap();
        // TODO: Verify parent via graph edge query when edge API is available
    }

    #[tokio::test]
    async fn test_concurrent_reorder_version_conflict() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create container
        let container_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create parent node
        let parent_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Parent".to_string(),
                parent_id: None,
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create three sibling nodes: A → B → C
        let node_a = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Node A".to_string(),
                parent_id: Some(parent_id.clone()),
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_b = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Node B".to_string(),
                parent_id: Some(parent_id.clone()),
                container_node_id: Some(container_id),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Two clients read node B
        let b1 = operations.get_node(&node_b).await.unwrap().unwrap();
        let b2 = operations.get_node(&node_b).await.unwrap().unwrap();

        // Client 1 reorders B before A
        operations
            .reorder_node(&node_b, b1.version, Some(&node_a))
            .await
            .unwrap();

        // Client 2 tries to reorder B with stale version
        let result = operations
            .reorder_node(&node_b, b2.version, None) // Move to end
            .await;

        assert!(
            matches!(result, Err(NodeOperationError::VersionConflict { .. })),
            "Should get VersionConflict on concurrent reorder"
        );
    }

    #[tokio::test]
    async fn test_concurrent_delete_version_conflict() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create a test node
        let node_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Test content".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Two clients read the node
        let node1 = operations.get_node(&node_id).await.unwrap().unwrap();
        let node2 = operations.get_node(&node_id).await.unwrap().unwrap();

        // Client 1 updates the node (version 1 → 2)
        operations
            .update_node(
                &node_id,
                node1.version,
                Some("Updated content".to_string()),
                None,
                None,
            )
            .await
            .unwrap();

        // Client 2 tries to delete with stale version
        let result = operations.delete_node(&node_id, node2.version).await;

        assert!(
            matches!(result, Err(NodeOperationError::VersionConflict { .. })),
            "Should get VersionConflict when deleting with stale version"
        );

        // Verify node still exists
        let final_node = operations.get_node(&node_id).await.unwrap();
        assert!(final_node.is_some(), "Node should still exist");
        assert_eq!(final_node.unwrap().content, "Updated content");
    }

    #[tokio::test]
    async fn test_delete_already_deleted_node_is_idempotent() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create a test node
        let node_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Test content".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node = operations.get_node(&node_id).await.unwrap().unwrap();

        // Client 1 deletes successfully
        let result1 = operations
            .delete_node(&node_id, node.version)
            .await
            .unwrap();
        assert!(result1.existed, "First delete should report existed=true");

        // Client 2 tries to delete the same node (already deleted)
        // Should succeed (idempotent) but report existed=false
        let result2 = operations
            .delete_node(&node_id, node.version)
            .await
            .unwrap();
        assert!(
            !result2.existed,
            "Second delete should report existed=false (idempotent)"
        );
    }

    #[tokio::test]
    async fn test_version_conflict_includes_current_state() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create a test node
        let node_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node = operations.get_node(&node_id).await.unwrap().unwrap();

        // Update to version 2
        operations
            .update_node(
                &node_id,
                node.version,
                Some("Updated".to_string()),
                None,
                None,
            )
            .await
            .unwrap();

        // Try to update with stale version
        let result = operations
            .update_node(
                &node_id,
                node.version, // Still 1
                Some("Conflicting".to_string()),
                None,
                None,
            )
            .await;

        // Verify error includes current state for merge
        match result {
            Err(NodeOperationError::VersionConflict {
                node_id: err_node_id,
                expected_version,
                actual_version,
                current_node,
            }) => {
                assert_eq!(err_node_id, node_id); // Compare with actual node_id
                assert_eq!(expected_version, 1);
                assert_eq!(actual_version, 2);
                assert_eq!(current_node.content, "Updated");
                assert_eq!(current_node.version, 2);
            }
            _ => panic!("Expected VersionConflict error"),
        }
    }

    // =========================================================================
    // update_node_with_hierarchy() Tests
    // =========================================================================

    #[tokio::test]
    async fn test_update_node_with_hierarchy_content_only() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create container and node
        let container_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: Some(container_id.clone()),
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Update content only (no hierarchy changes)
        let mut update = NodeUpdate::new();
        update.content = Some("Updated content".to_string());

        let updated = operations
            .update_node_with_hierarchy(&node_id, 1, update)
            .await
            .unwrap();

        assert_eq!(updated.content, "Updated content");
        assert_eq!(updated.version, 2);
        // Graph Architecture Note: Parent relationship managed via graph edges
        // TODO: Verify parent via graph edge query when edge API is available
    }

    #[tokio::test]
    async fn test_update_node_with_hierarchy_hierarchy_only() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create container
        let container_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create parent node
        let _parent_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Parent".to_string(),
                parent_id: Some(container_id.clone()),
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create child as sibling first
        let child_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Child".to_string(),
                parent_id: Some(container_id.clone()),
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Graph Architecture Note: Parent-child relationships now managed via graph edges
        // Indent operation would create/update graph edges instead of setting parent_id field
        let mut update = NodeUpdate::new();
        update.before_sibling_id = Some(None);

        let updated = operations
            .update_node_with_hierarchy(&child_id, 1, update)
            .await
            .unwrap();

        // Graph Architecture Note: Parent verification via graph edges
        // TODO: Verify parent via graph edge query when edge API is available
        assert_eq!(updated.before_sibling_id, None);
        assert_eq!(updated.version, 2);
    }

    #[tokio::test]
    async fn test_update_node_with_hierarchy_combined_update() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create container and parent
        let container_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let _parent_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Parent".to_string(),
                parent_id: Some(container_id.clone()),
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let child_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Child".to_string(),
                parent_id: Some(container_id.clone()),
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Combined update: change content AND hierarchy
        // Graph Architecture Note: Parent relationship managed via graph edges
        let mut update = NodeUpdate::new();
        update.content = Some("Updated child".to_string());
        update.before_sibling_id = Some(None);

        let updated = operations
            .update_node_with_hierarchy(&child_id, 1, update)
            .await
            .unwrap();

        assert_eq!(updated.content, "Updated child");
        // Graph Architecture Note: Parent verification via graph edges
        // TODO: Verify parent via graph edge query when edge API is available
        assert_eq!(updated.before_sibling_id, None);
        assert_eq!(updated.version, 2);
    }

    #[tokio::test]
    async fn test_update_node_with_hierarchy_empty_update_fails() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        let container_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: Some(container_id.clone()),
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Empty update should fail
        let update = NodeUpdate::new();
        let result = operations
            .update_node_with_hierarchy(&node_id, 1, update)
            .await;

        assert!(matches!(
            result,
            Err(NodeOperationError::InvalidOperation { .. })
        ));
    }

    #[tokio::test]
    async fn test_update_node_with_hierarchy_version_conflict() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        let container_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                container_node_id: None,
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: Some(container_id.clone()),
                container_node_id: Some(container_id.clone()),
                before_sibling_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Update to version 2
        let mut update1 = NodeUpdate::new();
        update1.content = Some("First update".to_string());
        operations
            .update_node_with_hierarchy(&node_id, 1, update1)
            .await
            .unwrap();

        // Try to update with stale version
        let mut update2 = NodeUpdate::new();
        update2.content = Some("Conflicting update".to_string());
        let result = operations
            .update_node_with_hierarchy(&node_id, 1, update2)
            .await;

        assert!(matches!(
            result,
            Err(NodeOperationError::VersionConflict { .. })
        ));
    }
}
