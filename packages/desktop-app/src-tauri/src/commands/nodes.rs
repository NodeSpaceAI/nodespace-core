//! Node CRUD operation commands for Text, Task, and Date nodes
//!
//! As of Issue #676, all commands route through NodeService directly.
//! NodeOperations layer has been removed - NodeService contains all business logic.

use nodespace_core::operations::CreateNodeParams;
use nodespace_core::services::SchemaService;
use nodespace_core::{
    EdgeRecord, Node, NodeQuery, NodeService, NodeServiceError, NodeUpdate, SurrealStore,
};
use serde::{Deserialize, Serialize};
use tauri::State;

/// Client ID for Tauri frontend (used for event filtering - Issue #665)
const TAURI_CLIENT_ID: &str = "tauri-main";

/// Input for creating a node - timestamps generated server-side
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNodeInput {
    pub id: String,
    pub node_type: String,
    pub content: String,
    pub parent_id: Option<String>,
    // root_id removed - backend auto-derives root from parent chain (Issue #533)
    /// Sibling node ID to insert after (None = insert at beginning of siblings)
    /// Used for correct ordering when creating child nodes via Enter key
    #[serde(default)]
    pub insert_after_node_id: Option<String>,
    pub properties: serde_json::Value,
    #[serde(default)]
    pub embedding_vector: Option<Vec<u8>>,
}

/// Structured error type for Tauri commands
///
/// Provides better observability and debugging by including error codes
/// and optional details alongside user-facing messages.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    /// User-facing error message
    pub message: String,
    /// Machine-readable error code
    pub code: String,
    /// Optional detailed error information for debugging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl From<NodeServiceError> for CommandError {
    fn from(err: NodeServiceError) -> Self {
        // Map specific error types to appropriate codes
        let code = match &err {
            NodeServiceError::NodeNotFound { .. } => "NODE_NOT_FOUND",
            NodeServiceError::VersionConflict { .. } => "VERSION_CONFLICT",
            NodeServiceError::InvalidParent { .. } => "INVALID_PARENT",
            NodeServiceError::CircularReference { .. } => "CIRCULAR_REFERENCE",
            NodeServiceError::HierarchyViolation(_) => "HIERARCHY_VIOLATION",
            _ => "NODE_SERVICE_ERROR",
        };
        CommandError {
            message: format!("Node operation failed: {}", err),
            code: code.to_string(),
            details: Some(format!("{:?}", err)),
        }
    }
}

/// Validate that node type has a schema
///
/// Checks if a schema exists for the given node type. This enables
/// custom entity types while ensuring type safety.
///
/// # Arguments
/// * `node_type` - Node type string to validate
/// * `schema_service` - SchemaService instance for schema lookups
///
/// # Returns
/// * `Ok(())` if schema exists for this node type
/// * `Err(CommandError)` if no schema found
async fn validate_node_type(
    node_type: &str,
    schema_service: &SchemaService,
) -> Result<(), CommandError> {
    // Check if schema exists for this type
    match schema_service.get_schema(node_type).await {
        Ok(_) => Ok(()),
        Err(NodeServiceError::NodeNotFound { .. }) => Err(CommandError {
            message: format!(
                "No schema found for node type: {}. Create a schema first.",
                node_type
            ),
            code: "SCHEMA_NOT_FOUND".to_string(),
            details: Some(format!(
                "Node type '{}' does not have a schema definition. \
                 Use the schema API to create a schema before creating nodes of this type.",
                node_type
            )),
        }),
        Err(e) => Err(CommandError::from(e)),
    }
}

/// Create a new node of any type with a registered schema
///
/// Routes through NodeService which contains all business logic (Issue #676).
/// Node type must have a corresponding schema defined in the system.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `schema_service` - SchemaService instance for validating node type
/// * `node` - Node data to create (node_type must have a schema)
///
/// # Returns
/// * `Ok(String)` - ID of the created node
/// * `Err(CommandError)` - Error with details if creation fails
///
/// # Errors
/// Returns error if:
/// - Node type schema doesn't exist (create schema first)
/// - Business rule validation fails (container requirements, sibling chains, etc.)
/// - Database operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const nodeId = await invoke('create_node', {
///   node: {
///     node_type: 'text',
///     content: 'Hello World',
///     properties: {}
///   }
/// });
/// ```
#[tauri::command]
pub async fn create_node(
    service: State<'_, NodeService>,
    schema_service: State<'_, SchemaService>,
    node: CreateNodeInput,
) -> Result<String, CommandError> {
    validate_node_type(&node.node_type, &schema_service).await?;

    // Use NodeService to create node with business rule enforcement
    // Pass frontend-generated ID so frontend can track node before persistence completes
    service
        .with_client(TAURI_CLIENT_ID)
        .create_node_with_parent(CreateNodeParams {
            id: Some(node.id), // Frontend provides UUID for local state tracking
            node_type: node.node_type,
            content: node.content,
            parent_id: node.parent_id,
            // root_id removed - backend auto-derives root from parent chain (Issue #533)
            // insert_after_node_id: Sibling to insert after for correct ordering (Issue #657)
            insert_after_node_id: node.insert_after_node_id,
            properties: node.properties,
        })
        .await
        .map_err(Into::into)
}

/// Input for creating a root node (top-level container)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRootNodeInput {
    pub content: String,
    pub node_type: String,
    #[serde(default)]
    pub properties: serde_json::Value,
    #[serde(default)]
    pub mentioned_by: Option<String>,
}

/// Input for saving a node with automatic parent creation
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveNodeWithParentInput {
    pub node_id: String,
    pub content: String,
    pub node_type: String,
    pub parent_id: String,
    pub root_id: String,
    // before_sibling_id removed - backend uses fractional ordering on has_child edges (Issue #616)
}

/// Create a new root node (top-level node that can contain other nodes)
///
/// Routes through NodeService which contains all business logic (Issue #676).
/// Root nodes are top-level nodes that can contain other nodes (pages, topics, etc.).
/// Node type must have a corresponding schema defined in the system.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `schema_service` - SchemaService instance for validating node type
/// * `input` - Root node data
///
/// # Returns
/// * `Ok(String)` - ID of the created root node
/// * `Err(CommandError)` - Error with details if creation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const nodeId = await invoke('create_root_node', {
///   input: {
///     content: 'Project Planning',
///     nodeType: 'text',
///     properties: {},
///     mentionedBy: 'daily-note-id' // Optional: node that mentions this root
///   }
/// });
/// ```
#[tauri::command]
pub async fn create_root_node(
    service: State<'_, NodeService>,
    schema_service: State<'_, SchemaService>,
    input: CreateRootNodeInput,
) -> Result<String, CommandError> {
    validate_node_type(&input.node_type, &schema_service).await?;

    // Create root node with NodeService (parent_id = None means root)
    let node_id = service
        .with_client(TAURI_CLIENT_ID)
        .create_node_with_parent(CreateNodeParams {
            id: None, // Let NodeService generate ID for root nodes
            node_type: input.node_type,
            content: input.content,
            parent_id: None,            // parent_id = None for root nodes
            insert_after_node_id: None, // No sibling positioning for root nodes
            properties: input.properties,
        })
        .await?;

    // If mentioned_by is provided, create mention relationship
    if let Some(mentioning_node_id) = input.mentioned_by {
        service
            .with_client(TAURI_CLIENT_ID)
            .create_mention(&mentioning_node_id, &node_id)
            .await?;
    }

    Ok(node_id)
}

/// Create a mention relationship between two nodes
///
/// Records that one node mentions another in the node_mentions table.
/// This enables backlink/references functionality.
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `mentioning_node_id` - ID of the node that contains the mention
/// * `mentioned_node_id` - ID of the node being mentioned
///
/// # Returns
/// * `Ok(())` - Mention created successfully
/// * `Err(CommandError)` - Error if either node doesn't exist or operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('create_node_mention', {
///   mentioningNodeId: 'daily-note-id',
///   mentionedNodeId: 'project-planning-id'
/// });
/// ```
#[tauri::command]
pub async fn create_node_mention(
    service: State<'_, NodeService>,
    mentioning_node_id: String,
    mentioned_node_id: String,
) -> Result<(), CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .create_mention(&mentioning_node_id, &mentioned_node_id)
        .await
        .map_err(Into::into)
}

/// Get a node by ID
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `id` - Unique identifier of the node to retrieve
///
/// # Returns
/// * `Ok(Some(Node))` - Node data if found
/// * `Ok(None)` - No node exists with the given ID
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Errors
/// Returns error if:
/// - Database operation fails
/// - ID format is invalid
///
/// # Example Frontend Usage
/// ```typescript
/// const node = await invoke('get_node', { id: 'node-123' });
/// if (node) {
///   console.log('Found node:', node.content);
/// }
/// ```
#[tauri::command]
pub async fn get_node(
    service: State<'_, NodeService>,
    id: String,
) -> Result<Option<Node>, CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .get_node(&id)
        .await
        .map_err(Into::into)
}

/// Update an existing node
///
/// Routes through NodeService which contains all business logic (Issue #676).
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `id` - Unique identifier of the node to update
/// * `version` - Expected version for optimistic concurrency control
/// * `update` - Fields to update on the node
///
/// # Returns
/// * `Ok(Node)` - Updated node with new version
/// * `Err(CommandError)` - Error with details if update fails
///
/// # Errors
/// Returns error if:
/// - Node with given ID doesn't exist
/// - Version conflict (concurrent modification)
/// - Database operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('update_node', {
///   id: 'node-123',
///   version: 5,
///   update: {
///     content: 'Updated content',
///     properties: { priority: 1 }
///   }
/// });
/// ```
#[tauri::command]
pub async fn update_node(
    service: State<'_, NodeService>,
    id: String,
    version: i64,
    update: NodeUpdate,
) -> Result<Node, CommandError> {
    // Use update_node_with_occ for OCC-protected updates
    // Returns the updated Node so frontend can refresh its local version
    service
        .with_client(TAURI_CLIENT_ID)
        .update_node_with_occ(&id, version, update)
        .await
        .map_err(Into::into)
}

/// Delete a node by ID with cascade deletion
///
/// Routes through NodeService which contains all business logic (Issue #676).
/// Cascades delete to all child nodes recursively.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `id` - Unique identifier of the node to delete
/// * `version` - Expected version for optimistic concurrency control
///
/// # Returns
/// * `Ok(DeleteResult)` - Deletion result (existed: true if node was deleted)
/// * `Err(CommandError)` - Error with details if deletion fails
///
/// # Errors
/// Returns error if:
/// - Version conflict (concurrent modification)
/// - Database operation fails
///
/// # Warning
/// This operation is destructive and cannot be undone. Deletes all child nodes
/// recursively.
///
/// # Example Frontend Usage
/// ```typescript
/// const result = await invoke('delete_node', { id: 'node-123', version: 5 });
/// console.log(`Node existed: ${result.existed}`);
/// ```
#[tauri::command]
pub async fn delete_node(
    service: State<'_, NodeService>,
    id: String,
    version: i64,
) -> Result<nodespace_core::models::DeleteResult, CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .delete_node_with_occ(&id, version)
        .await
        .map_err(Into::into)
}

/// Atomically move a node to a new parent with new sibling position (with OCC)
///
/// Performs a single database transaction that:
/// - Validates version for optimistic concurrency control
/// - Deletes the old parent-child edge
/// - Updates the node's before_sibling_id field
/// - Creates the new parent-child edge (if new parent specified)
/// - Bumps the node version after successful move
///
/// This ensures database consistency without race conditions.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `node_id` - ID of the node to move
/// * `version` - Expected version for OCC (prevents concurrent modification conflicts)
/// * `new_parent_id` - New parent (None = root node)
/// * `insert_after_node_id` - New position in sibling chain
///
/// # Returns
/// * `Ok(())` - Move completed successfully
/// * `Err(CommandError)` - Error if move validation fails or version conflict
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('move_node', {
///   nodeId: 'node-123',
///   version: 5,
///   newParentId: 'parent-456',
///   insertAfterNodeId: 'node-789'
/// });
/// ```
#[tauri::command]
pub async fn move_node(
    service: State<'_, NodeService>,
    node_id: String,
    version: i64,
    new_parent_id: Option<String>,
    insert_after_node_id: Option<String>,
) -> Result<(), CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .move_node_with_occ(
            &node_id,
            version,
            new_parent_id.as_deref(),
            insert_after_node_id.as_deref(),
        )
        .await
        .map_err(Into::into)
}

/// Reorder a node by changing its sibling position
///
/// Routes through NodeService which contains all business logic (Issue #676).
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `node_id` - ID of the node to reorder
/// * `version` - Expected version for optimistic concurrency control
/// * `insert_after_node_id` - Optional ID of sibling to place after (None = first position)
///
/// # Returns
/// * `Ok(())` - Node reordered successfully
/// * `Err(CommandError)` - Error if reorder validation fails
///
/// # Errors
/// Returns error if:
/// - Node doesn't exist
/// - Version conflict (concurrent modification)
/// - Sibling doesn't exist
/// - Root node cannot be reordered
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('reorder_node', { nodeId: 'node-123', version: 5, insertAfterNodeId: 'sibling-456' });
/// ```
#[tauri::command]
pub async fn reorder_node(
    service: State<'_, NodeService>,
    node_id: String,
    version: i64,
    insert_after_node_id: Option<String>,
) -> Result<(), CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .reorder_node_with_occ(&node_id, version, insert_after_node_id.as_deref())
        .await
        .map_err(Into::into)
}

/// Get child nodes of a parent node
///
/// Retrieves all nodes that have the specified node as their parent,
/// supporting hierarchical organization of nodes.
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `parent_id` - ID of the parent node
///
/// # Returns
/// * `Ok(Vec<Node>)` - List of child nodes (empty vec if no children)
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Errors
/// Returns error if:
/// - Database operation fails
/// - Parent ID format is invalid
///
/// # Notes
/// - Returns empty vec if parent has no children
/// - Returns empty vec if parent node doesn't exist (not an error)
/// - Order of returned nodes is not guaranteed
///
/// # Example Frontend Usage
/// ```typescript
/// const children = await invoke('get_children', {
///   parent_id: 'parent-node-123'
/// });
/// console.log(`Found ${children.length} child nodes`);
/// ```
#[tauri::command]
pub async fn get_children(
    service: State<'_, NodeService>,
    parent_id: String,
) -> Result<Vec<Node>, CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .get_children(&parent_id)
        .await
        .map_err(Into::into)
}

/// Get a node with its entire subtree as a nested tree structure
///
/// Returns a single parent node with all its descendants recursively nested as a tree.
/// This optimization eliminates the need for frontend tree reconstruction by fetching
/// the entire subtree in a single database query using SurrealDB's recursive FETCH.
///
/// # Performance
/// - Single recursive query to database
/// - Eliminates O(n) tree reconstruction on frontend
/// - Much faster than fetching flat children array + edges + reconstructing
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `parent_id` - ID of the root node to fetch with its entire subtree
///
/// # Returns
/// * `Ok(NodeWithChildren)` - Root node with complete nested tree structure
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const tree = await invoke('get_children_tree', {
///   parent_id: 'my-root-id'
/// });
/// // tree has structure:
/// // {
/// //   id: 'my-root-id',
/// //   nodeType: 'text',
/// //   content: 'Root',
/// //   children: [
/// //     { id: 'child-1', content: '...', children: [...] },
/// //     { id: 'child-2', content: '...', children: [...] }
/// //   ]
/// // }
/// ```
#[tauri::command]
pub async fn get_children_tree(
    service: State<'_, NodeService>,
    parent_id: String,
) -> Result<serde_json::Value, CommandError> {
    service
        .get_children_tree(&parent_id)
        .await
        .map_err(Into::into)
}

/// Bulk fetch all nodes belonging to a root node (viewer/page)
///
/// This is the efficient way to load a complete document tree:
/// - Single database query fetches all nodes with the same root
/// - In-memory hierarchy reconstruction using parent_id and before_sibling_id
///
/// Phase 5 (Issue #511): Uses graph edges instead of root_id field
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `root_id` - ID of the root node (e.g., date page ID)
///
/// # Returns
/// * `Ok(Vec<Node>)` - All children of this root
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const nodes = await invoke('get_nodes_by_root_id', {
///   rootId: '2025-10-05'
/// });
/// console.log(`Loaded ${nodes.length} nodes for this date`);
/// ```
#[tauri::command]
pub async fn get_nodes_by_root_id(
    service: State<'_, NodeService>,
    root_id: String,
) -> Result<Vec<Node>, CommandError> {
    // Phase 5 (Issue #511): Redirect to get_children (graph-native)
    service
        .with_client(TAURI_CLIENT_ID)
        .get_children(&root_id)
        .await
        .map_err(Into::into)
}

/// Query nodes with flexible filtering
///
/// Supports queries by:
/// - ID (exact match)
/// - mentioned_by (finds nodes that mention the specified node ID)
/// - content_contains (case-insensitive substring search)
/// - node_type (filter by type)
/// - limit (maximum results to return)
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `query` - Query parameters (all fields optional)
///
/// # Returns
/// * `Ok(Vec<Node>)` - Matching nodes (empty if no matches)
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Query Priority
/// The query uses priority-based selection - first matching field determines query type:
/// 1. ID query (exact match, returns 0 or 1 result)
/// 2. mentioned_by query (finds backlinks)
/// 3. content_contains query (can be combined with node_type)
/// 4. node_type query (type filter only)
///
/// # Example Frontend Usage
/// ```typescript
/// // Find nodes that mention a specific node (backlinks)
/// const backlinks = await invoke('query_nodes_simple', {
///   query: { mentionedBy: 'node-123', limit: 50 }
/// });
///
/// // Search by content
/// const results = await invoke('query_nodes_simple', {
///   query: { contentContains: 'project', nodeType: 'text' }
/// });
///
/// // Get specific node
/// const node = await invoke('query_nodes_simple', {
///   query: { id: 'node-123' }
/// });
/// ```
#[tauri::command]
pub async fn query_nodes_simple(
    service: State<'_, NodeService>,
    query: NodeQuery,
) -> Result<Vec<Node>, CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .query_nodes_simple(query)
        .await
        .map_err(Into::into)
}

/// Mention autocomplete query - specialized endpoint for @mention feature
///
/// Provides optimized autocomplete suggestions for @mention syntax.
/// Designed to evolve with ranking, scoring, and relevance features.
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `query` - Search query string
/// * `limit` - Maximum number of results (default: 10)
///
/// # Returns
/// * `Ok(Vec<Node>)` - Array of matching nodes (tasks and containers only)
/// * `Err(CommandError)` - Error if query fails
///
/// # Filter Behavior
/// - Includes: Task nodes and container nodes (top-level documents)
/// - Excludes: Date nodes (accessible via date shortcuts), nested text children
/// - Search: Case-insensitive content matching
///
/// # Future Enhancements (Phase 2)
/// - Relevance scoring (recency, frequency, proximity)
/// - Context-aware ranking
/// - User preference learning
///
/// # Example Frontend Usage
/// ```typescript
/// const results = await invoke('mention_autocomplete', {
///   query: 'project',
///   limit: 10
/// });
/// ```
#[tauri::command]
pub async fn mention_autocomplete(
    service: State<'_, NodeService>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<Node>, CommandError> {
    // Build NodeQuery with mention-specific defaults
    let node_query = NodeQuery {
        content_contains: if query.is_empty() { None } else { Some(query) },
        limit,
        ..Default::default()
    };

    service
        .query_nodes_simple(node_query)
        .await
        .map_err(Into::into)
}

/// Save a node with automatic parent creation - unified upsert operation
///
/// Ensures the parent node exists (creates if needed), then upserts the node.
/// All operations happen in a single database transaction to prevent locking issues.
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `schema_service` - SchemaService instance for node type validation
/// * `input` - SaveNodeWithParentInput containing node data
///
/// # Returns
/// * `Ok(())` - Save successful
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Errors
/// Returns error if:
/// - Node type schema doesn't exist
/// - Database operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('save_node_with_parent', {
///   nodeId: 'node-123',
///   content: 'Updated content',
///   nodeType: 'text',
///   parentId: '2025-10-05',
///   rootId: '2025-10-05',
///   beforeSiblingId: null
/// });
/// ```
///
/// # Architecture Note: NodeOperations Bypass
///
/// ⚠️ **IMPORTANT**: This command uses `NodeService` directly instead of `NodeOperations`.
///
/// **Why the bypass?**
/// - This is a specialized **transactional upsert** that combines "ensure parent exists" + "upsert node"
/// - The transaction must complete atomically to prevent database locking issues during auto-save
/// - NodeOperations enforces business rules but doesn't support transactional parent creation yet
///
/// **Safety measures:**
/// - Still validates node type via `validate_node_type()`
/// - Limited to specific auto-save use case (frontend debounced typing)
/// - All other node operations (`create_node`, `update_node`, `move_node`, `reorder_node`) use NodeOperations
///
/// **Future improvement**: Consider adding `NodeOperations::upsert_node_with_parent()` method
/// to enforce business rules within the transaction semantics (tracked in follow-up issue).
#[tauri::command]
pub async fn save_node_with_parent(
    service: State<'_, NodeService>,
    schema_service: State<'_, SchemaService>,
    input: SaveNodeWithParentInput,
) -> Result<(), CommandError> {
    validate_node_type(&input.node_type, &schema_service).await?;

    // Use single-transaction upsert method (bypasses NodeOperations for transactional reasons)
    service
        .with_client(TAURI_CLIENT_ID)
        .upsert_node_with_parent(
            &input.node_id,
            &input.content,
            &input.node_type,
            &input.parent_id,
            &input.root_id,
            None, // before_sibling_id removed - backend uses fractional ordering (Issue #616)
        )
        .await
        .map_err(Into::into)
}

/// Get outgoing mentions (nodes that this node mentions)
///
/// Retrieves all nodes that are mentioned in this node's content.
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `node_id` - ID of the node to query
///
/// # Returns
/// * `Ok(Vec<String>)` - List of mentioned node IDs (empty if no mentions)
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const mentions = await invoke('get_outgoing_mentions', {
///   nodeId: 'node-123'
/// });
/// ```
#[tauri::command]
pub async fn get_outgoing_mentions(
    service: State<'_, NodeService>,
    node_id: String,
) -> Result<Vec<String>, CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .get_mentions(&node_id)
        .await
        .map_err(Into::into)
}

/// Get incoming mentions (nodes that mention this node - BACKLINKS)
///
/// Retrieves all nodes that mention this node in their content.
/// This enables bidirectional linking and backlink discovery.
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `node_id` - ID of the node to query for backlinks
///
/// # Returns
/// * `Ok(Vec<String>)` - List of node IDs that mention this node (empty if no backlinks)
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const backlinks = await invoke('get_incoming_mentions', {
///   nodeId: 'node-123'
/// });
/// ```
#[tauri::command]
pub async fn get_incoming_mentions(
    service: State<'_, NodeService>,
    node_id: String,
) -> Result<Vec<String>, CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .get_mentioned_by(&node_id)
        .await
        .map_err(Into::into)
}

/// Get root nodes of nodes that mention the target node (backlinks at root level)
///
/// This resolves incoming mentions to their root nodes and deduplicates.
///
/// Unlike `get_incoming_mentions` which returns individual mentioning nodes,
/// this resolves to their root nodes and deduplicates automatically.
///
/// # Root Resolution Logic
/// - For task and ai-chat nodes: Returns the node's own ID (they are their own roots)
/// - For other nodes: Returns their root node ID (or the node ID itself if it's a root)
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `node_id` - ID of the node to find backlinks for
///
/// # Returns
/// * `Ok(Vec<String>)` - List of unique root node IDs (empty if no backlinks)
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Example
/// If nodes A and B (both children of Root X) mention target node,
/// returns `['root-x-id']` instead of `['node-a-id', 'node-b-id']`
///
/// # Example Frontend Usage
/// ```typescript
/// const roots = await invoke('get_mentioning_roots', {
///   nodeId: 'node-123'
/// });
/// ```
#[tauri::command]
pub async fn get_mentioning_roots(
    service: State<'_, NodeService>,
    node_id: String,
) -> Result<Vec<String>, CommandError> {
    // Note: NodeService method still uses legacy name, will be updated in Phase 6
    service
        .get_mentioning_containers(&node_id)
        .await
        .map_err(Into::into)
}

/// Delete a mention relationship between two nodes
///
/// Removes the record that one node mentions another.
/// This is called when mentions are removed from node content.
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `mentioning_node_id` - ID of the node that contains the mention
/// * `mentioned_node_id` - ID of the node being mentioned
///
/// # Returns
/// * `Ok(())` - Mention deleted successfully
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('delete_node_mention', {
///   mentioningNodeId: 'node-123',
///   mentionedNodeId: 'node-456'
/// });
/// ```
#[tauri::command]
pub async fn delete_node_mention(
    service: State<'_, NodeService>,
    mentioning_node_id: String,
    mentioned_node_id: String,
) -> Result<(), CommandError> {
    service
        .with_client(TAURI_CLIENT_ID)
        .remove_mention(&mentioning_node_id, &mentioned_node_id)
        .await
        .map_err(Into::into)
}

/// Retrieves all has_child edges from the database
///
/// Used for bulk-loading the tree structure on startup rather than waiting
/// for incremental LIVE SELECT events to populate the tree.
#[tauri::command]
pub async fn get_all_edges(
    store: State<'_, SurrealStore>,
) -> Result<Vec<EdgeRecord>, CommandError> {
    store.get_all_edges().await.map_err(|e| CommandError {
        message: format!("Failed to fetch edges: {}", e),
        code: "GET_EDGES_FAILED".to_string(),
        details: Some(format!("{:?}", e)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Integration tests for schema-based validation are in the integration test suite
    // since they require database setup and SchemaService instances.
    // Unit tests here focus on command error serialization.

    #[test]
    fn test_command_error_serialization() {
        let err = CommandError {
            message: "Test error".to_string(),
            code: "TEST_ERROR".to_string(),
            details: Some("Debug info".to_string()),
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Test error"));
        assert!(json.contains("TEST_ERROR"));
        assert!(json.contains("Debug info"));
    }

    #[test]
    fn test_command_error_without_details() {
        let err = CommandError {
            message: "Simple error".to_string(),
            code: "SIMPLE".to_string(),
            details: None,
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Simple error"));
        // Details field should be omitted when None
        assert!(!json.contains("details"));
    }
}
