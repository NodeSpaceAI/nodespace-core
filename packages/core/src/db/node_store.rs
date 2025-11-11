//! NodeStore Trait - Database Abstraction Layer
//!
//! This module defines the `NodeStore` trait that abstracts database operations
//! for nodes in NodeSpace. The trait enables multiple backend implementations
//! (Turso/libsql, SurrealDB) without changing business logic in NodeService.
//!
//! # Architecture
//!
//! - **Abstraction Point**: Between NodeService (business logic) and database implementation
//! - **Multiple Backends**: Supports Turso (Phase 1) and SurrealDB (Phase 2+)
//! - **Zero Regressions**: All operations preserve existing behavior exactly
//! - **Performance Target**: <5% overhead from trait dispatch
//!
//! # Design Decisions
//!
//! 1. **Async-First**: All methods are async to support both embedded (Turso) and
//!    network (SurrealDB) backends
//! 2. **Ownership Semantics**: Methods take ownership of values to avoid unnecessary
//!    cloning (caller can clone if needed)
//! 3. **Error Handling**: Uses `anyhow::Result` for flexible error context
//! 4. **Transaction Support**: Deferred to Phase 2 (requires more design)
//!
//! # Examples
//!
//! ```rust,no_run
//! use nodespace_core::db::{NodeStore, TursoStore, DatabaseService};
//! use nodespace_core::models::Node;
//! use std::sync::Arc;
//! use serde_json::json;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create database service
//!     let db = Arc::new(DatabaseService::new_in_memory().await?);
//!
//!     // Wrap in NodeStore trait
//!     let store: Arc<dyn NodeStore> = Arc::new(TursoStore::new(db));
//!
//!     // Use abstraction layer
//!     let node = Node::new(
//!         "text".to_string(),
//!         "My note".to_string(),
//!         None,
//!         json!({}),
//!     );
//!     let created = store.create_node(node).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Phase 1 Scope (Epic #461)
//!
//! - Define trait with 22 methods (transaction deferred)
//! - Implement TursoStore wrapper (delegates to DatabaseService)
//! - Refactor NodeService to use trait instead of direct DatabaseService
//! - Validate: Zero test regressions, <5% performance overhead
//!
//! For detailed architecture, see:
//! `/docs/architecture/data/node-store-abstraction.md`

use crate::models::{DeleteResult, Node, NodeQuery, NodeUpdate};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// Abstraction layer for node persistence operations
///
/// This trait enables parallel implementations of different database backends
/// (Turso/libsql, SurrealDB) without changing business logic in NodeService.
///
/// All methods are async to support both embedded (Turso) and network (SurrealDB) backends.
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to allow usage in async contexts where
/// futures may be moved between threads.
///
/// # Method Categories
///
/// - **Core CRUD**: 4 methods (create, read, update, delete)
/// - **Querying**: 4 methods (filter, search, pagination)
/// - **Hierarchy**: 2 methods (move, reorder)
/// - **Mentions**: 5 methods (reference graph operations)
/// - **Schema**: 2 methods (dynamic property definitions)
/// - **Embeddings**: 3 methods (vector search, AI integration)
/// - **Batch**: 1 method (performance optimization)
/// - **Lifecycle**: 1 method (resource management)
///
/// Total: 22 methods covering complete NodeSpace database API
#[async_trait]
pub trait NodeStore: Send + Sync {
    //
    // CORE CRUD OPERATIONS
    //

    /// Create a new node in the database
    ///
    /// # Arguments
    ///
    /// * `node` - Node to create (ownership transferred to avoid cloning)
    ///
    /// # Returns
    ///
    /// Created node with any generated fields (timestamps, etc.)
    ///
    /// # Ownership
    ///
    /// Takes ownership of node to avoid unnecessary cloning. Caller can clone
    /// before calling if they need to retain the original.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node ID already exists (duplicate key)
    /// - Parent node doesn't exist (foreign key violation)
    /// - Validation fails (invalid node type, properties, etc.)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let node = Node::new(
    ///     "text".to_string(),
    ///     "My note".to_string(),
    ///     None,
    ///     json!({}),
    /// );
    /// let created = store.create_node(node).await?;
    /// println!("Created node: {}", created.id);
    /// # Ok(())
    /// # }
    /// ```
    async fn create_node(&self, node: Node) -> Result<Node>;

    /// Get node by ID
    ///
    /// # Arguments
    ///
    /// * `id` - Node ID to retrieve
    ///
    /// # Returns
    ///
    /// - `Ok(Some(node))` if node exists
    /// - `Ok(None)` if node doesn't exist (not an error)
    /// - `Err(_)` if database error occurs
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// match store.get_node("node-123").await? {
    ///     Some(node) => println!("Found: {}", node.content),
    ///     None => println!("Node not found"),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    async fn get_node(&self, id: &str) -> Result<Option<Node>>;

    /// Update node properties
    ///
    /// # Arguments
    ///
    /// * `id` - Node ID to update
    /// * `update` - Fields to update (sparse update, only provided fields changed)
    ///
    /// # Returns
    ///
    /// Updated node with all fields (not just changed fields)
    ///
    /// # Ownership
    ///
    /// Takes ownership of NodeUpdate to avoid cloning. Returns owned Node.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node doesn't exist
    /// - Optimistic concurrency check fails (version mismatch)
    /// - Validation fails (invalid updates)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let update = NodeUpdate {
    ///     content: Some("Updated content".to_string()),
    ///     ..Default::default()
    /// };
    /// let updated = store.update_node("node-123", update).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn update_node(&self, id: &str, update: NodeUpdate) -> Result<Node>;

    /// Delete node and its children (cascading delete)
    ///
    /// # Cascade Delete Semantics
    ///
    /// **Behavior**: Recursively deletes the target node and all descendants in the node hierarchy.
    ///
    /// **Implementation Requirements**:
    ///
    /// 1. **Atomicity**: All deletes MUST occur in a single transaction (all-or-nothing).
    ///    - If any delete fails, the entire operation rolls back
    ///    - No partial deletions allowed
    ///
    /// 2. **Traversal Order**: Depth-first, leaf-to-root deletion order
    ///    - Prevents foreign key violations
    ///    - Children deleted before parents
    ///
    /// 3. **Mention Cleanup**: Delete all mention relationships where:
    ///    - `source_id` equals any deleted node ID, OR
    ///    - `target_id` equals any deleted node ID
    ///    - This prevents orphaned references
    ///
    /// 4. **Recursion Limit**: Implementations must support hierarchies up to 1,000 levels deep
    ///    - Exceeding 1,000 levels returns validation error (not a crash)
    ///    - Realistic limit for NodeSpace use cases
    ///
    /// 5. **Performance Requirements**:
    ///    - Use bulk DELETE queries where backend supports them (avoid N+1 queries)
    ///    - Target: Delete subtree of 10,000 nodes in <5 seconds
    ///    - Monitor stack depth to prevent stack overflow
    ///
    /// 6. **Error Handling**:
    ///    - Database constraint violations → roll back with clear error message
    ///    - Concurrent modification detected → conflict error, caller should retry
    ///    - Node not found → succeed silently (idempotent delete)
    ///
    /// # Arguments
    ///
    /// * `id` - Node ID to delete
    ///
    /// # Returns
    ///
    /// Delete result with count of deleted nodes
    ///
    /// # Test Cases
    ///
    /// - Single node deletion (no children): <10ms
    /// - Node with 1,000 descendants: <1 second
    /// - Node with 10,000 descendants: <5 seconds
    /// - Concurrent deletion + creation: One operation wins, other fails cleanly
    /// - Deletion during mention creation: Atomic (mention points to valid node or delete fails)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let result = store.delete_node("node-123").await?;
    /// println!("Deleted {} nodes", result.deleted_count);
    /// # Ok(())
    /// # }
    /// ```
    async fn delete_node(&self, id: &str) -> Result<DeleteResult>;

    //
    // QUERYING & FILTERING
    //

    /// Query nodes with filters, ordering, and pagination
    ///
    /// # Arguments
    ///
    /// * `query` - Query parameters (filter, order_by, limit, offset)
    ///
    /// # Query Semantics
    ///
    /// - All filter fields combined with AND logic
    /// - `None` values ignored (no filtering on that field)
    /// - Empty query returns all nodes
    /// - Implementations should use indexed queries where possible
    ///
    /// # Returns
    ///
    /// Vector of nodes matching query criteria, ordered as specified
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # use nodespace_core::models::{NodeQuery, NodeFilter};
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let query = NodeQuery {
    ///     filter: Some(NodeFilter {
    ///         node_type: Some("task".to_string()),
    ///         ..Default::default()
    ///     }),
    ///     limit: Some(10),
    ///     ..Default::default()
    /// };
    /// let nodes = store.query_nodes(query).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn query_nodes(&self, query: NodeQuery) -> Result<Vec<Node>>;

    /// Get all children of a parent node (ordered by before_sibling_id chain)
    ///
    /// # Arguments
    ///
    /// * `parent_id` - Parent node ID, or None for root nodes (nodes with parent_id = NULL)
    ///
    /// # Returns
    ///
    /// Children ordered by before_sibling_id linked list (preserves user-defined order)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// // Get root nodes
    /// let roots = store.get_children(None).await?;
    ///
    /// // Get children of specific node
    /// let children = store.get_children(Some("parent-123")).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn get_children(&self, parent_id: Option<&str>) -> Result<Vec<Node>>;

    /// Get nodes by container ID (for viewer pages)
    ///
    /// Container nodes group related nodes (e.g., DateNode contains tasks for that date).
    /// Nodes where `container_node_id = NULL` are themselves containers.
    ///
    /// # Arguments
    ///
    /// * `container_id` - Container node ID
    ///
    /// # Returns
    ///
    /// All nodes with `container_node_id = container_id`, ordered by before_sibling_id
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// // Get all tasks for a specific date
    /// let tasks = store.get_nodes_by_container("2025-01-03").await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn get_nodes_by_container(&self, container_id: &str) -> Result<Vec<Node>>;

    /// Search nodes by content (full-text search)
    ///
    /// # Arguments
    ///
    /// * `query` - Search query string
    /// * `limit` - Max results to return (None = no limit)
    ///
    /// # Search Semantics
    ///
    /// - Case-insensitive substring match on `content` field
    /// - Future: May use FTS (Full-Text Search) index if available
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let results = store.search_nodes_by_content("meeting", Some(10)).await?;
    /// for node in results {
    ///     println!("Found: {}", node.content);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    async fn search_nodes_by_content(&self, query: &str, limit: Option<i64>) -> Result<Vec<Node>>;

    //
    // HIERARCHY OPERATIONS
    //

    /// Move node to new parent (update parent_id)
    ///
    /// # Arguments
    ///
    /// * `id` - Node ID to move
    /// * `new_parent_id` - New parent node ID, or None to move to root
    ///
    /// # Side Effects
    ///
    /// - Updates `parent_id` field
    /// - May update `before_sibling_id` to place at end of new parent's children
    /// - May trigger container_node_id recalculation (if moving between containers)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node doesn't exist
    /// - New parent doesn't exist
    /// - Would create circular reference (node becomes ancestor of itself)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// // Move node to new parent
    /// store.move_node("node-123", Some("new-parent-456")).await?;
    ///
    /// // Move node to root
    /// store.move_node("node-123", None).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn move_node(&self, id: &str, new_parent_id: Option<&str>) -> Result<()>;

    /// Reorder node within siblings (update before_sibling_id)
    ///
    /// # Arguments
    ///
    /// * `id` - Node ID to reorder
    /// * `new_before_sibling_id` - ID of sibling this node should appear before,
    ///   or None to move to end of sibling list
    ///
    /// # Side Effects
    ///
    /// Updates `before_sibling_id` field to reposition node in sibling order
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// // Move node before specific sibling
    /// store.reorder_node("node-123", Some("sibling-456")).await?;
    ///
    /// // Move node to end
    /// store.reorder_node("node-123", None).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn reorder_node(&self, id: &str, new_before_sibling_id: Option<&str>) -> Result<()>;

    //
    // MENTION GRAPH OPERATIONS
    //

    /// Create mention relationship (source node mentions target node)
    ///
    /// Used for [[wiki-style links]], @mentions, and backlinks.
    ///
    /// # Arguments
    ///
    /// * `source_id` - Node that contains the mention
    /// * `target_id` - Node being mentioned
    /// * `container_id` - Container node where the mention appears (for backlink grouping)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Source or target node doesn't exist
    /// - Mention already exists (duplicate)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// store.create_mention("source-123", "target-456", "container-789").await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn create_mention(
        &self,
        source_id: &str,
        target_id: &str,
        container_id: &str,
    ) -> Result<()>;

    /// Delete mention relationship
    ///
    /// # Arguments
    ///
    /// * `source_id` - Node that contains the mention
    /// * `target_id` - Node being mentioned
    ///
    /// # Idempotency
    ///
    /// Deleting non-existent mention succeeds (no-op)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// store.delete_mention("source-123", "target-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn delete_mention(&self, source_id: &str, target_id: &str) -> Result<()>;

    /// Get outgoing mentions from node (nodes this node mentions)
    ///
    /// # Returns
    ///
    /// Vector of target node IDs (nodes being mentioned)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let mentioned_ids = store.get_outgoing_mentions("node-123").await?;
    /// println!("This node mentions {} other nodes", mentioned_ids.len());
    /// # Ok(())
    /// # }
    /// ```
    async fn get_outgoing_mentions(&self, node_id: &str) -> Result<Vec<String>>;

    /// Get incoming mentions to node (nodes that mention this node)
    ///
    /// # Returns
    ///
    /// Vector of source node IDs (nodes that contain mentions of this node)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let mentioning_ids = store.get_incoming_mentions("node-123").await?;
    /// println!("{} nodes mention this node", mentioning_ids.len());
    /// # Ok(())
    /// # }
    /// ```
    async fn get_incoming_mentions(&self, node_id: &str) -> Result<Vec<String>>;

    /// Get containers mentioning this node (for backlinks)
    ///
    /// Used to show "Where is this node referenced?" in UI.
    ///
    /// # Returns
    ///
    /// Vector of container nodes where this node is mentioned
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let containers = store.get_mentioning_containers("node-123").await?;
    /// println!("Referenced in {} containers", containers.len());
    /// # Ok(())
    /// # }
    /// ```
    async fn get_mentioning_containers(&self, node_id: &str) -> Result<Vec<Node>>;

    //
    // SCHEMA OPERATIONS (Dynamic Properties)
    //

    /// Get schema definition for node type
    ///
    /// # Returns
    ///
    /// JSON schema definition, or None if no schema defined
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// match store.get_schema("task").await? {
    ///     Some(schema) => println!("Task schema: {}", schema),
    ///     None => println!("No schema defined for task"),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    async fn get_schema(&self, node_type: &str) -> Result<Option<Value>>;

    /// Update schema definition
    ///
    /// # Arguments
    ///
    /// * `node_type` - Node type to update schema for
    /// * `schema` - JSON schema definition
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # use serde_json::json;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let schema = json!({
    ///     "type": "object",
    ///     "properties": {
    ///         "status": {"type": "string", "enum": ["todo", "done"]},
    ///         "priority": {"type": "string"}
    ///     }
    /// });
    /// store.update_schema("task", &schema).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn update_schema(&self, node_type: &str, schema: &Value) -> Result<()>;

    //
    // EMBEDDING OPERATIONS (Vector Search)
    //

    /// Get nodes without embeddings (for background processing)
    ///
    /// Used by embedding service to find nodes needing vector generation.
    ///
    /// # Arguments
    ///
    /// * `limit` - Max results to return (None = no limit)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let nodes = store.get_nodes_without_embeddings(Some(100)).await?;
    /// println!("{} nodes need embeddings", nodes.len());
    /// # Ok(())
    /// # }
    /// ```
    async fn get_nodes_without_embeddings(&self, limit: Option<i64>) -> Result<Vec<Node>>;

    /// Update node embedding vector
    ///
    /// # Arguments
    ///
    /// * `node_id` - Node ID to update
    /// * `embedding` - Binary-encoded embedding vector (IEEE 754 float32, little-endian)
    ///
    /// # Embedding Format
    ///
    /// - **Model**: BAAI/bge-small-en-v1.5 (384 dimensions)
    /// - **Storage**: Binary `Vec<u8>` (1,536 bytes = 384 dims × 4 bytes/float)
    /// - **Encoding**: Little-endian IEEE 754 float32
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore, embedding_bytes: Vec<u8>) -> anyhow::Result<()> {
    /// store.update_embedding("node-123", &embedding_bytes).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn update_embedding(&self, node_id: &str, embedding: &[u8]) -> Result<()>;

    /// Search nodes by embedding similarity (vector search)
    ///
    /// # Arguments
    ///
    /// * `embedding` - Query embedding vector (binary-encoded)
    /// * `limit` - Max results to return
    ///
    /// # Returns
    ///
    /// Vector of (node, similarity_score) tuples ordered by similarity (descending).
    /// Similarity scores are cosine similarity values (-1.0 to 1.0, typically 0.3 to 0.95).
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore, query_embedding: Vec<u8>) -> anyhow::Result<()> {
    /// let results = store.search_by_embedding(&query_embedding, 10).await?;
    /// for (node, score) in results {
    ///     println!("Node: {}, Similarity: {:.2}", node.content, score);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    async fn search_by_embedding(&self, embedding: &[u8], limit: i64) -> Result<Vec<(Node, f64)>>;

    //
    // BATCH OPERATIONS
    //

    /// Batch create nodes (optimized insertion)
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of nodes to create (ownership transferred)
    ///
    /// # Returns
    ///
    /// Vector of created nodes with generated fields
    ///
    /// # Performance
    ///
    /// Implementation should use bulk insert for performance (single transaction).
    ///
    /// # Ownership
    ///
    /// Takes ownership of Vec<Node> to avoid cloning
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// let nodes = vec![
    ///     Node::new("text".to_string(), "Note 1".to_string(), None, json!({})),
    ///     Node::new("text".to_string(), "Note 2".to_string(), None, json!({})),
    /// ];
    /// let created = store.batch_create_nodes(nodes).await?;
    /// println!("Created {} nodes", created.len());
    /// # Ok(())
    /// # }
    /// ```
    async fn batch_create_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Node>>;

    //
    // DATABASE LIFECYCLE
    //

    /// Close database connection and cleanup resources
    ///
    /// Should be called when shutting down the application to ensure:
    /// - All pending writes are flushed
    /// - Connections are properly closed
    /// - Resources are released
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::NodeStore;
    /// # async fn example(store: &dyn NodeStore) -> anyhow::Result<()> {
    /// // When shutting down
    /// store.close().await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn close(&self) -> Result<()>;
}
