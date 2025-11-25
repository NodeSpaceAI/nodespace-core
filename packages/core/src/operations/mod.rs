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
//! 1. **Root Type Validation**: Container/root types (date, topic, project) must
//!    have parent_id = None (no hierarchy fields allowed).
//!
//! 2. **Auto-Derive Root from Parent Chain**: Every non-root node's container/root
//!    is automatically derived by traversing parent edges to find the top-level node.
//!
//! 3. **Sibling Position Calculation**: If insert_after_node_id is not provided,
//!    the node is placed as the last sibling automatically.
//!
//! 4. **Update Operations Restrictions**: Content updates cannot change hierarchy.
//!    Use move_node() or reorder_node() explicitly for hierarchy changes.
//!
//! # Examples
//!
//! ```no_run
//! use nodespace_core::operations::{NodeOperations, CreateNodeParams};
//! use nodespace_core::services::NodeService;
//! use nodespace_core::db::SurrealStore;
//! use std::sync::Arc;
//! use std::path::PathBuf;
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let db = Arc::new(SurrealStore::new(PathBuf::from("./data/test.db")).await?);
//!     let node_service = Arc::new(NodeService::new(db)?);
//!     let operations = NodeOperations::new(node_service);
//!
//!     // Create a node with automatic root/container derivation from parent
//!     let node_id = operations.create_node(CreateNodeParams {
//!         id: None, // Auto-generate ID
//!         node_type: "text".to_string(),
//!         content: "Hello World".to_string(),
//!         parent_id: Some("parent-id".to_string()),
//!         insert_after_node_id: None, // Will be placed as last sibling
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
///     insert_after_node_id: None,
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
///     insert_after_node_id: None,
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
    /// Optional parent node ID (container/root will be auto-derived from parent chain)
    pub parent_id: Option<String>,
    /// Optional sibling to insert after (if None, appends to end)
    pub insert_after_node_id: Option<String>,
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
/// # use nodespace_core::operations::{NodeOperations, CreateNodeParams};
/// # use nodespace_core::services::NodeService;
/// # use nodespace_core::db::SurrealStore;
/// # use std::sync::Arc;
/// # use std::path::PathBuf;
/// # use serde_json::json;
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
/// # let node_service = Arc::new(NodeService::new(db)?);
/// let operations = NodeOperations::new(node_service);
///
/// // All operations automatically enforce business rules
/// let node_id = operations.create_node(CreateNodeParams {
///     id: None, // Auto-generate ID
///     node_type: "text".to_string(),
///     content: "Content".to_string(),
///     parent_id: Some("parent-id".to_string()),
///     insert_after_node_id: None, // Placed as last sibling
///     properties: json!({}),
/// }).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct NodeOperations<C = surrealdb::engine::local::Db>
where
    C: surrealdb::Connection,
{
    /// Underlying NodeService for database operations
    node_service: Arc<NodeService<C>>,
}

impl<C> NodeOperations<C>
where
    C: surrealdb::Connection,
{
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
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// let node_service = Arc::new(NodeService::new(db)?);
    /// let operations = NodeOperations::new(node_service);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(node_service: Arc<NodeService<C>>) -> Self {
        Self { node_service }
    }

    // =========================================================================
    // Private Helper Methods
    // =========================================================================

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

        // Check if date container already exists IN THE DATABASE
        // IMPORTANT: Call store.get_node() directly to bypass virtual date node logic
        // in node_service.get_node(). The virtual date nodes are only for read operations,
        // we need to check actual database state for auto-creation.
        let exists = self
            .node_service
            .store
            .get_node(node_id)
            .await
            .map_err(|e| NodeOperationError::DatabaseError(e.to_string()))?
            .is_some();
        if exists {
            return Ok(()); // Already exists in database
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

    /// Resolve root_id for non-root nodes
    ///
    /// Business Rule 2: Every non-root node MUST have a root_id.
    /// If not provided explicitly, infer it from the parent.
    ///
    /// # Arguments
    ///
    /// * `root_id` - Explicitly provided root (may be None)
    /// * `parent_id` - Parent node ID (may be None)
    ///
    /// # Returns
    ///
    /// The resolved root_id or an error if it cannot be determined
    ///
    /// # Errors
    ///
    /// - `NodeNotFound` if parent doesn't exist
    /// - `NonContainerMustHaveContainer` if container cannot be resolved
    async fn resolve_container(
        &self,
        node_id: &str,
        node_type: &str,
        parent_id: Option<&str>,
    ) -> Result<String, NodeOperationError> {
        // Container is always inferred from parent chain (Issue #533 - removed explicit container parameter)
        if let Some(parent_id) = parent_id {
            // Verify parent exists
            self.node_service
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(parent_id.to_string()))?;

            // Get the root by traversing up the parent chain
            // If parent is a root (no parent of its own), use the parent as the root
            // Otherwise, get the parent's root recursively
            let root_id = self.node_service.get_root_id(parent_id).await?;
            return Ok(root_id);
        }

        // Cannot resolve root
        Err(NodeOperationError::non_root_must_have_root(
            node_id.to_string(),
            node_type.to_string(),
        ))
    }

    /// Calculate sibling position for a new/moved node
    ///
    /// Business Rule 3: If insert_after_node_id is not provided, place the node
    /// as the last sibling automatically.
    ///
    /// # Arguments
    ///
    /// * `parent_id` - Parent node ID (may be None for root nodes)
    /// * `insert_after_node_id` - Sibling to insert after (None means append at end)
    ///
    /// # Returns
    ///
    /// The insert_after_node_id to use:
    /// - Some(id) if explicitly provided (validated) or if last child found (append at end)
    /// - None if no siblings exist (will insert at beginning, which is also the end)
    ///
    /// # Errors
    ///
    /// - `InvalidSiblingChain` if insert_after_node_id is invalid
    /// - `NodeNotFound` if the sibling doesn't exist
    async fn calculate_sibling_position(
        &self,
        parent_id: Option<&str>,
        insert_after_node_id: Option<String>,
    ) -> Result<Option<String>, NodeOperationError> {
        // If explicitly provided, validate it exists
        if let Some(ref sibling_id) = insert_after_node_id {
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

            return Ok(insert_after_node_id);
        }

        // No explicit sibling provided - find last child to append at end
        // Note: create_parent_edge treats None as "insert at beginning", so we need
        // to explicitly find the last child if we want "append at end" behavior.
        if let Some(parent_id) = parent_id {
            let children = self.node_service.get_children(parent_id).await?;
            if let Some(last_child) = children.last() {
                return Ok(Some(last_child.id.clone()));
            }
        }

        // No parent or no existing children - None means "first position" (only position)
        Ok(None)
    }

    /// Validate parent exists and hierarchy is valid
    ///
    /// This ensures nodes are created with valid parent references,
    /// preventing orphaned nodes and broken hierarchies.
    ///
    /// Note: Container consistency validation was removed in Issue #533
    /// as containers are now auto-derived from parent chain traversal.
    async fn validate_parent_hierarchy(
        &self,
        parent_id: Option<&str>,
    ) -> Result<(), NodeOperationError> {
        if let Some(parent_id) = parent_id {
            // Verify parent exists
            self.node_service
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(parent_id.to_string()))?;
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
    /// * `parent_id` - Optional parent node reference (root auto-derived from parent chain)
    /// * `insert_after_node_id` - Optional sibling to insert after (calculated as last if None)
    /// * `properties` - JSON object with entity-specific fields
    ///
    /// # Returns
    ///
    /// The ID of the created node
    ///
    /// # Errors
    ///
    /// Returns `NodeOperationError` if:
    /// - Root/container nodes have parent/sibling set
    /// - Non-root nodes have no parent
    /// - Invalid sibling chain
    /// - Node validation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::operations::{NodeOperations, CreateNodeParams};
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let node_service = Arc::new(NodeService::new(db)?);
    /// # let operations = NodeOperations::new(node_service);
    /// // Create a date node (root/container)
    /// let date_id = operations.create_node(CreateNodeParams {
    ///     id: None, // Auto-generate ID
    ///     node_type: "date".to_string(),
    ///     content: "2025-01-03".to_string(),
    ///     parent_id: None, // Root nodes have no parent
    ///     insert_after_node_id: None, // Root nodes have no siblings
    ///     properties: json!({}),
    /// }).await?;
    ///
    /// // Create a text node (root auto-derived from parent chain)
    /// let text_id = operations.create_node(CreateNodeParams {
    ///     id: None, // Auto-generate ID
    ///     node_type: "text".to_string(),
    ///     content: "Hello".to_string(),
    ///     parent_id: Some(date_id.clone()), // Root auto-derived from date_id
    ///     insert_after_node_id: None, // Placed as last sibling
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
    /// # use nodespace_core::operations::{NodeOperations, CreateNodeParams};
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # async fn example(operations: Arc<NodeOperations>) -> Result<(), Box<dyn std::error::Error>> {
    /// let date_id = operations.create_node(CreateNodeParams {
    ///     id: None, // Auto-generate ID
    ///     node_type: "date".to_string(),
    ///     content: "2025-10-23".to_string(),  // This becomes the ID
    ///     parent_id: None,
    ///     insert_after_node_id: None,
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

        // Validate parent exists (if provided) - fail fast on broken hierarchies
        self.validate_parent_hierarchy(params.parent_id.as_deref())
            .await?;

        // Business Rule 1: Auto-derive container/root from parent chain
        // If node has a parent, traverse parent edges to find the root node
        // If node has NO parent, it IS the root (root_id = None)
        let (final_parent_id, _final_root_id, last_sibling_id) = if params.parent_id.is_some() {
            // Node has a parent - derive container from parent chain
            let resolved_container = self
                .resolve_container(
                    &params.content, // Passed for error context only (node ID not yet assigned)
                    &params.node_type,
                    params.parent_id.as_deref(), // Container derived from parent chain
                )
                .await?;

            // Business Rule 2: Calculate sibling position
            let calculated_sibling = self
                .calculate_sibling_position(
                    params.parent_id.as_deref(),
                    params.insert_after_node_id,
                )
                .await?;

            (
                params.parent_id,
                Some(resolved_container),
                calculated_sibling,
            )
        } else {
            // Node has NO parent - it's a root-level node
            // Root nodes can still have siblings (other root-level nodes)
            let calculated_sibling = self
                .calculate_sibling_position(None, params.insert_after_node_id)
                .await?;

            (None, None, calculated_sibling)
        };

        // Create the node using NodeService
        // Use provided ID if given (allows frontend to pre-generate UUIDs for local state management)
        // Otherwise, special case: date nodes use their content (YYYY-MM-DD) as the ID
        // Otherwise: generate a new UUID
        let node_id = if let Some(provided_id) = params.id {
            // Validate that provided ID is either:
            // 1. A proper UUID format (for regular nodes)
            // 2. A valid date format (YYYY-MM-DD) for date nodes
            // 3. A string ID for schema nodes (e.g., "task", "date", "text")
            // 4. Test IDs (start with "test-") for testing
            if params.node_type == "date"
                || params.node_type == "schema"
                || provided_id.starts_with("test-")
            {
                // Date, schema, and test nodes can use their own ID format
                provided_id
            } else {
                // Production nodes must use UUID format (security check)
                uuid::Uuid::parse_str(&provided_id).map_err(|_| {
                    NodeOperationError::invalid_operation(format!(
                        "Provided ID '{}' is not a valid UUID format (required for non-date/non-schema nodes)",
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
            version: 1,
            properties: params.properties,
            mentions: vec![],
            mentioned_by: vec![],
            created_at: Utc::now(),
            modified_at: Utc::now(),
            embedding_vector: None,
        };

        let created_id = self.node_service.create_node(node).await?;

        // Create parent edge atomically if parent_id was specified
        // This establishes the has_child graph edge: parent->has_child->child
        if let Some(parent_id) = final_parent_id {
            // Use create_parent_edge which preserves insert_after_node_id atomically
            self.node_service
                .create_parent_edge(&created_id, &parent_id, last_sibling_id.as_deref())
                .await?;
            tracing::debug!(
                "Created parent edge: {} -> has_child -> {} (insert_after: {:?})",
                parent_id,
                created_id,
                last_sibling_id
            );
        }

        // Note: Container relationship is NOT stored as an edge.
        // Container is determined by traversing UP the parent chain to find the root node.
        // The final_root_id is used for validation/queries but doesn't create database edges.
        // See NodeService::get_root_id() for the graph traversal implementation.

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
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let node_service = Arc::new(NodeService::new(db)?);
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
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let node_service = Arc::new(NodeService::new(db)?);
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

    /// Get all descendants of a root node
    ///
    /// Returns all nodes within the root's hierarchy (not just direct children).
    pub async fn get_descendants(&self, root_id: &str) -> Result<Vec<Node>, NodeOperationError> {
        Ok(self.node_service.get_nodes_by_root_id(root_id).await?)
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
    pub async fn get_root_id(&self, node_id: &str) -> Result<String, NodeOperationError> {
        // Delegate to NodeService which implements the traversal algorithm
        Ok(self.node_service.get_root_id(node_id).await?)
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
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let node_service = Arc::new(NodeService::new(db)?);
    /// # let operations = NodeOperations::new(node_service);
    /// operations.update_node(
    ///     "node-id",
    ///     1, // expected_version for optimistic concurrency control
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
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let node_service = Arc::new(NodeService::new(db)?);
    /// # let operations = NodeOperations::new(node_service);
    /// let update = NodeUpdate::new()
    ///     .with_content("Updated content".to_string());
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

        // Fetch current node to validate it exists (hierarchy changes go through move_node)
        let _current = self
            .node_service
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

        // NOTE: Hierarchy changes (parent_id, root_id) are no longer supported in update_node.
        // Use the move_node() API instead for changing node hierarchy.
        // Sibling ordering is now handled via has_child edge order field, not before_sibling_id.

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
    /// - Validates parent-root consistency
    /// - Updates root_id if needed
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
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let node_service = Arc::new(NodeService::new(db)?);
    /// # let operations = NodeOperations::new(node_service);
    /// operations.move_node("node-id", 1, Some("new-parent-id")).await?;
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

        // Validate new parent exists
        if let Some(parent_id) = new_parent_id {
            // Ensure parent exists
            self.node_service
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeOperationError::node_not_found(parent_id.to_string()))?;
        }

        // Delegate to NodeService to perform the edge operations:
        // 1. Delete existing parent edge (via: DELETE has_child WHERE out = $child_thing)
        // 2. Create new parent edge if specified (via: RELATE $parent_thing->has_child->$child_thing)
        // Note: Container edge doesn't exist - container is determined by traversing to root
        self.node_service
            .move_node(node_id, new_parent_id, None)
            .await?;

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
    /// * `insert_after` - The sibling to place this node after (None = first position)
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
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let node_service = Arc::new(NodeService::new(db)?);
    /// # let operations = NodeOperations::new(node_service);
    /// // Move node to end of sibling list
    /// operations.reorder_node("node-id", 1, None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reorder_node(
        &self,
        node_id: &str,
        expected_version: i64,
        insert_after: Option<&str>,
    ) -> Result<(), NodeOperationError> {
        // Get current node and verify version (optimistic concurrency control)
        let node = self
            .node_service
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeOperationError::node_not_found(node_id.to_string()))?;

        // Check version before proceeding
        if node.version != expected_version {
            return Err(NodeOperationError::version_conflict(
                node_id.to_string(),
                expected_version,
                node.version,
                node,
            ));
        }

        // Root nodes cannot be reordered (they have no parent)
        if self.node_service.is_root_node(node_id).await? {
            return Err(NodeOperationError::invalid_operation(format!(
                "Root node '{}' cannot be reordered (it has no parent)",
                node_id
            )));
        }

        // Use graph-native reordering via NodeService
        // This operates on the `order` field of has_child edges, not node fields
        self.node_service
            .reorder_child(node_id, insert_after)
            .await?;

        // Bump the node's version to support OCC (optimistic concurrency control).
        // Even though we're only modifying edge ordering, we bump the node version
        // so that concurrent reorder operations will fail with version conflict.
        self.node_service
            .update_node_with_version_bump(node_id, expected_version)
            .await?;

        Ok(())
    }

    // =========================================================================
    // DELETE Operations
    // =========================================================================

    /// Delete a node with sibling chain integrity preservation
    ///
    /// With graph-native architecture using fractional ordering on has_child edges,
    /// sibling chain integrity is automatically maintained. Deleting a node simply
    /// removes its edge, and remaining siblings keep their relative order.
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
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let node_service = Arc::new(NodeService::new(db)?);
    /// # let operations = NodeOperations::new(node_service);
    /// let result = operations.delete_node("node-id", 1).await?;
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
        let _node = match self.node_service.get_node(node_id).await? {
            Some(n) => n,
            None => {
                // Node doesn't exist - return false immediately
                return Ok(DeleteResult { existed: false });
            }
        };

        // 2. NOTE: Sibling ordering is now managed via has_child edge order field.
        // No sibling chain repair is needed - edge deletion handles ordering automatically.

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
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create a child WITHOUT root_id - should infer from parent
        let child_id = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Child content".to_string(),
                parent_id: Some(date_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Graph Architecture Note: Parent-child and root relationships are now managed
        // via graph edges in the SurrealDB schema, not via parent_id/root_id fields.
        // The relationship assertions would be verified via edge queries.
        let _child = operations.get_node(&child_id).await.unwrap().unwrap();
        // TODO: Verify parent-child relationship via graph edge query when edge API is available
    }

    #[tokio::test]
    #[ignore = "TODO(#533): Re-enable after root concept cleanup"]
    async fn test_parent_root_mismatch_error() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create two separate date containers
        let _date1 = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let _date2 = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-04".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await;

        // Verify error is ParentRootMismatch
        assert!(
            result.is_err(),
            "Should error when parent and child have different roots"
        );
        let error = result.unwrap_err();
        assert!(
            matches!(error, NodeOperationError::ParentRootMismatch { .. }),
            "Error should be ParentRootMismatch, got: {:?}",
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
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create first node under date container
        // API behavior: insert_after_node_id = None means "append at end"
        let first = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "First".to_string(),
                parent_id: Some(date.clone()),
                insert_after_node_id: None, // Appends at end (first child)
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create second node under date container (appends after first)
        let second = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Second".to_string(),
                parent_id: Some(date.clone()),
                insert_after_node_id: None, // Appends at end (after first)
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create third node AFTER first under date container
        // This should place third between first and second in the children order
        let third = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "text".to_string(),
                content: "Third".to_string(),
                parent_id: Some(date.clone()),
                insert_after_node_id: Some(first.clone()), // Insert after first
                properties: json!({}),
            })
            .await
            .unwrap();

        // Verify ordering via children order (graph-native architecture uses fractional order on edges)
        let children = operations.get_children(&date).await.unwrap();
        assert_eq!(children.len(), 3, "Should have 3 children");

        // With documented API behavior (None = append at end):
        // - first was added at end (only child at time)
        // - second was added at end (after first)
        // - third was added after first (between first and second)
        // So final order should be: [first, third, second]
        assert_eq!(
            children[0].id, first,
            "First should be first (added first, appended at end)"
        );
        assert_eq!(
            children[1].id, third,
            "Third should be in the middle (added after first)"
        );
        assert_eq!(
            children[2].id, second,
            "Second should be last (third was inserted after first)"
        );
    }

    // =========================================================================
    // Delete Operations - Sibling Chain Integrity Tests
    // =========================================================================

    #[tokio::test]
    async fn test_delete_node_fixes_sibling_chain_middle() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let _date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Note: Sibling ordering is now on has_child edge order field, not node.before_sibling_id
        let _a = operations.get_node(&node_a).await.unwrap().unwrap();
        let b = operations.get_node(&node_b).await.unwrap().unwrap();
        let _c = operations.get_node(&node_c).await.unwrap().unwrap();

        // Delete B (middle node)
        let result = operations.delete_node(&node_b, b.version).await.unwrap();
        assert!(result.existed);

        // Verify B is deleted
        let deleted = operations.get_node(&node_b).await.unwrap();
        assert!(deleted.is_none());

        // Verify A and C still exist
        let _a_after = operations.get_node(&node_a).await.unwrap().unwrap();
        let _c_after = operations.get_node(&node_c).await.unwrap().unwrap();
        // Sibling ordering integrity is maintained via edge order field
    }

    #[tokio::test]
    async fn test_delete_node_first_in_chain() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let _date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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

        // Verify second node still exists
        // Note: Sibling ordering is now on has_child edge order field
        let _second_after = operations.get_node(&second).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_delete_node_last_in_chain() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let _date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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

        // Verify first node still exists
        // Note: Sibling ordering is now on has_child edge order field
        let _first_after = operations.get_node(&first).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn test_delete_node_with_children() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create date container
        let _date = operations
            .create_node(CreateNodeParams {
                id: None, // Test generates ID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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

        // Verify sibling still exists
        // Note: Sibling ordering is now on has_child edge order field
        let _sibling_after = operations.get_node(&sibling).await.unwrap().unwrap();
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Graph Architecture Note: Parent-child and root relationships are now managed
        // via graph edges in the SurrealDB schema, not via parent_id/root_id fields.
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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

    // TODO(issue-533): Re-enable once move_node implements version checking
    // Currently move_node accepts expected_version parameter but doesn't validate it
    // See move_node implementation at line 1091 - version parameter is prefixed with underscore
    #[ignore]
    #[tokio::test]
    async fn test_concurrent_move_version_conflict() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create root
        let _root_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Create two parent nodes in the root
        let parent1_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "text".to_string(),
                content: "Parent 1".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
            "Should get VersionConflict on concurrent move, got: {:?}",
            result
        );

        // Graph Architecture Note: Parent relationship verification via graph edges
        let _final_child = operations.get_node(&child_id).await.unwrap().unwrap();
        // TODO: Verify parent via graph edge query when edge API is available
    }

    #[tokio::test]
    async fn test_concurrent_reorder_version_conflict() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create root
        let _root_id = operations
            .create_node(CreateNodeParams {
                id: None, // Auto-generate UUID
                node_type: "date".to_string(),
                content: "2025-01-03".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Two clients read node B
        let b1 = operations.get_node(&node_b).await.unwrap().unwrap();
        let b2 = operations.get_node(&node_b).await.unwrap().unwrap();

        // Client 1 reorders B after A (B was before A, now B will be after A)
        operations
            .reorder_node(&node_b, b1.version, Some(&node_a))
            .await
            .unwrap();

        // Client 2 tries to reorder B with stale version (would move to beginning)
        let result = operations.reorder_node(&node_b, b2.version, None).await;

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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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
                insert_after_node_id: None,
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

        // Create root and node
        let root_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: Some(root_id.clone()),
                insert_after_node_id: None,
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

        // Create root
        let root_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                insert_after_node_id: None,
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
                parent_id: Some(root_id.clone()),
                insert_after_node_id: None,
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
                parent_id: Some(root_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Graph Architecture Note: Parent-child relationships now managed via graph edges
        // Indent operation would create/update graph edges instead of setting parent_id field
        // Note: Sibling ordering is now on has_child edge order field, not node.before_sibling_id
        let update = NodeUpdate::new().with_content("Updated content".to_string());

        let updated = operations
            .update_node_with_hierarchy(&child_id, 1, update)
            .await
            .unwrap();

        assert_eq!(updated.content, "Updated content");
        assert_eq!(updated.version, 2);
    }

    #[tokio::test]
    async fn test_update_node_with_hierarchy_combined_update() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        // Create root and parent
        let root_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let _parent_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Parent".to_string(),
                parent_id: Some(root_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let child_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Child".to_string(),
                parent_id: Some(root_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        // Combined update: change content
        // Graph Architecture Note: Parent relationship managed via graph edges
        // Note: Sibling ordering is now on has_child edge order field, not node.before_sibling_id
        let update = NodeUpdate::new().with_content("Updated child".to_string());

        let updated = operations
            .update_node_with_hierarchy(&child_id, 1, update)
            .await
            .unwrap();

        assert_eq!(updated.content, "Updated child");
        assert_eq!(updated.version, 2);
    }

    #[tokio::test]
    async fn test_update_node_with_hierarchy_empty_update_fails() {
        let (operations, _temp_dir) = setup_test_operations().await.unwrap();

        let root_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: Some(root_id.clone()),
                insert_after_node_id: None,
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

        let root_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "date".to_string(),
                content: "2025-11-13".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            })
            .await
            .unwrap();

        let node_id = operations
            .create_node(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Original".to_string(),
                parent_id: Some(root_id.clone()),
                insert_after_node_id: None,
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
