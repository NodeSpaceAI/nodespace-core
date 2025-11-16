//! Node CRUD operation commands for Text, Task, and Date nodes

use nodespace_core::operations::{CreateNodeParams, NodeOperationError, NodeOperations};
use nodespace_core::{Node, NodeQuery, NodeService, NodeServiceError, NodeUpdate};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::constants::ALLOWED_NODE_TYPES;

/// Input for creating a node - timestamps generated server-side
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNodeInput {
    pub id: String,
    pub node_type: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub container_node_id: Option<String>,
    pub before_sibling_id: Option<String>,
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
        CommandError {
            message: format!("Node operation failed: {}", err),
            code: "NODE_SERVICE_ERROR".to_string(),
            details: Some(format!("{:?}", err)),
        }
    }
}

impl From<NodeOperationError> for CommandError {
    fn from(err: NodeOperationError) -> Self {
        CommandError {
            message: format!("Node operation failed: {}", err),
            code: "NODE_OPERATION_ERROR".to_string(),
            details: Some(format!("{:?}", err)),
        }
    }
}

/// Validate that node type is supported
///
/// # Arguments
/// * `node_type` - Node type string to validate
///
/// # Returns
/// * `Ok(())` if node type is valid
/// * `Err(CommandError)` if node type is not supported
fn validate_node_type(node_type: &str) -> Result<(), CommandError> {
    if !ALLOWED_NODE_TYPES.contains(&node_type) {
        return Err(CommandError {
            message: format!(
                "Only text, task, and date nodes are supported. Got: {}",
                node_type
            ),
            code: "INVALID_NODE_TYPE".to_string(),
            details: Some(format!(
                "Allowed types: {:?}, received: '{}'",
                ALLOWED_NODE_TYPES, node_type
            )),
        });
    }
    Ok(())
}

/// Create a new node (Text, Task, or Date only)
///
/// Uses NodeOperations business logic layer to enforce data integrity rules.
///
/// # Arguments
/// * `operations` - NodeOperations instance from Tauri state
/// * `node` - Node data to create (must have node_type: text|task|date)
///
/// # Returns
/// * `Ok(String)` - ID of the created node
/// * `Err(CommandError)` - Error with details if creation fails
///
/// # Errors
/// Returns error if:
/// - Node type is not one of: text, task, date
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
    operations: State<'_, NodeOperations>,
    node: CreateNodeInput,
) -> Result<String, CommandError> {
    validate_node_type(&node.node_type)?;

    // Use NodeOperations to create node with business rule enforcement
    // Pass frontend-generated ID so frontend can track node before persistence completes
    operations
        .create_node(CreateNodeParams {
            id: Some(node.id), // Frontend provides UUID for local state tracking
            node_type: node.node_type,
            content: node.content,
            parent_id: node.parent_id,
            container_node_id: node.container_node_id,
            before_sibling_id: node.before_sibling_id,
            properties: node.properties,
        })
        .await
        .map_err(Into::into)
}

/// Input for creating a container node
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContainerNodeInput {
    pub content: String,
    pub node_type: String,
    #[serde(default)]
    pub properties: serde_json::Value,
    #[serde(default)]
    pub mentioned_by: Option<String>,
}

/// Create a new container node (root node with container_node_id = NULL)
///
/// Uses NodeOperations to create container with business rule enforcement.
/// Container nodes are root-level nodes that can contain other nodes.
/// NodeOperations ensures they have:
/// - parent_id = None
/// - container_node_id = None (they ARE containers)
/// - before_sibling_id = None
///
/// # Arguments
/// * `operations` - NodeOperations instance from Tauri state
/// * `service` - NodeService for mention relationship creation
/// * `input` - Container node data
///
/// # Returns
/// * `Ok(String)` - ID of the created container node
/// * `Err(CommandError)` - Error with details if creation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const nodeId = await invoke('create_container_node', {
///   input: {
///     content: 'Project Planning',
///     nodeType: 'text',
///     properties: {},
///     mentionedBy: 'daily-note-id' // Optional: node that mentions this container
///   }
/// });
/// ```
#[tauri::command]
pub async fn create_container_node(
    operations: State<'_, NodeOperations>,
    service: State<'_, NodeService>,
    input: CreateContainerNodeInput,
) -> Result<String, CommandError> {
    validate_node_type(&input.node_type)?;

    // Create container node with NodeOperations (enforces container rules)
    let node_id = operations
        .create_node(CreateNodeParams {
            id: None, // Let NodeOperations generate ID for containers
            node_type: input.node_type,
            content: input.content,
            parent_id: None,         // parent_id = None for containers
            container_node_id: None, // container_node_id = None for containers
            before_sibling_id: None, // before_sibling_id = None for containers
            properties: input.properties,
        })
        .await?;

    // If mentioned_by is provided, create mention relationship
    if let Some(mentioning_node_id) = input.mentioned_by {
        service
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
    service.get_node(&id).await.map_err(Into::into)
}

/// Update an existing node
///
/// Uses NodeOperations to enforce business rules during updates.
///
/// # Arguments
/// * `operations` - NodeOperations instance from Tauri state
/// * `id` - Unique identifier of the node to update
/// * `update` - Fields to update on the node
///
/// # Returns
/// * `Ok(())` - Update successful
/// * `Err(CommandError)` - Error with details if update fails
///
/// # Errors
/// Returns error if:
/// - Node with given ID doesn't exist
/// - Business rule validation fails
/// - Database operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('update_node', {
///   id: 'node-123',
///   update: {
///     content: 'Updated content',
///     properties: { priority: 1 }
///   }
/// });
/// ```
#[tauri::command]
pub async fn update_node(
    operations: State<'_, NodeOperations>,
    id: String,
    version: i64,
    update: NodeUpdate,
) -> Result<Node, CommandError> {
    // Use update_node_with_hierarchy to handle both content AND hierarchy changes
    // This orchestrates move_node(), reorder_node(), and content updates as needed
    // IMPORTANT: Return the updated Node so frontend can refresh its local version
    operations
        .update_node_with_hierarchy(&id, version, update)
        .await
        .map_err(Into::into)
}

/// Delete a node by ID
///
/// Uses NodeOperations for consistent deletion logic.
///
/// # Arguments
/// * `operations` - NodeOperations instance from Tauri state
/// * `id` - Unique identifier of the node to delete
///
/// # Returns
/// * `Ok(DeleteResult)` - Deletion result with cascaded node IDs
/// * `Err(CommandError)` - Error with details if deletion fails
///
/// # Errors
/// Returns error if:
/// - Node with given ID doesn't exist
/// - Database operation fails
/// - Node has dependencies that prevent deletion
///
/// # Warning
/// This operation is destructive and cannot be undone. Consider implementing
/// soft deletes in the future if undo functionality is required.
///
/// # Example Frontend Usage
/// ```typescript
/// const result = await invoke('delete_node', { id: 'node-123' });
/// console.log(`Deleted ${result.deletedNodeIds.length} nodes`);
/// ```
#[tauri::command]
pub async fn delete_node(
    operations: State<'_, NodeOperations>,
    id: String,
    version: i64,
) -> Result<nodespace_core::models::DeleteResult, CommandError> {
    operations
        .delete_node(&id, version)
        .await
        .map_err(Into::into)
}

/// Move a node to a new parent
///
/// Uses NodeOperations to enforce business rules for hierarchy changes.
/// Validates parent-container consistency and updates container_node_id.
///
/// # Arguments
/// * `operations` - NodeOperations instance from Tauri state
/// * `node_id` - ID of the node to move
/// * `new_parent_id` - Optional new parent ID (None makes it a root node)
///
/// # Returns
/// * `Ok(())` - Node moved successfully
/// * `Err(CommandError)` - Error if move validation fails
///
/// # Errors
/// Returns error if:
/// - Node doesn't exist
/// - New parent doesn't exist
/// - Container node cannot be moved (containers must remain at root)
/// - Parent-container consistency check fails
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('move_node', { nodeId: 'node-123', newParentId: 'parent-456' });
/// ```
#[tauri::command]
pub async fn move_node(
    operations: State<'_, NodeOperations>,
    node_id: String,
    version: i64,
    new_parent_id: Option<String>,
) -> Result<(), CommandError> {
    operations
        .move_node(&node_id, version, new_parent_id.as_deref())
        .await
        .map_err(Into::into)
}

/// Reorder a node by changing its sibling position
///
/// Uses NodeOperations to calculate sibling positions and validate ordering.
///
/// # Arguments
/// * `operations` - NodeOperations instance from Tauri state
/// * `node_id` - ID of the node to reorder
/// * `before_sibling_id` - Optional ID of sibling to place before (None = last position)
///
/// # Returns
/// * `Ok(())` - Node reordered successfully
/// * `Err(CommandError)` - Error if reorder validation fails
///
/// # Errors
/// Returns error if:
/// - Node doesn't exist
/// - before_sibling_id node doesn't exist
/// - Sibling is not in the same parent
/// - Container node cannot be reordered
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('reorder_node', { nodeId: 'node-123', beforeSiblingId: 'sibling-456' });
/// ```
#[tauri::command]
pub async fn reorder_node(
    operations: State<'_, NodeOperations>,
    node_id: String,
    version: i64,
    before_sibling_id: Option<String>,
) -> Result<(), CommandError> {
    operations
        .reorder_node(&node_id, version, before_sibling_id.as_deref())
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
    service.get_children(&parent_id).await.map_err(Into::into)
}

/// Bulk fetch all nodes belonging to an origin node (viewer/page)
///
/// This is the efficient way to load a complete document tree:
/// - Single database query fetches all nodes with the same container_node_id
/// - In-memory hierarchy reconstruction using parent_id and before_sibling_id
///
/// Phase 5 (Issue #511): Uses graph edges instead of container_node_id field
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `container_node_id` - ID of the parent node (e.g., date page ID)
///
/// # Returns
/// * `Ok(Vec<Node>)` - All children of this parent
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const nodes = await invoke('get_nodes_by_container_id', {
///   containerNodeId: '2025-10-05'
/// });
/// console.log(`Loaded ${nodes.length} nodes for this date`);
/// ```
#[tauri::command]
pub async fn get_nodes_by_container_id(
    service: State<'_, NodeService>,
    container_node_id: String,
) -> Result<Vec<Node>, CommandError> {
    // Phase 5 (Issue #511): Redirect to get_children (graph-native)
    service
        .get_children(&container_node_id)
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
    service.query_nodes_simple(query).await.map_err(Into::into)
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
        include_containers_and_tasks: Some(true),
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
/// * `node_id` - ID of the node to save
/// * `content` - Node content
/// * `node_type` - Type of the node (text, task, date)
/// * `parent_id` - ID of the parent node (will be created if doesn't exist)
///
/// # Returns
/// * `Ok(())` - Save successful
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Errors
/// Returns error if:
/// - Node type is not one of: text, task, date
/// - Database operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('save_node_with_parent', {
///   nodeId: 'node-123',
///   content: 'Updated content',
///   nodeType: 'text',
///   parentId: '2025-10-05',
///   originNodeId: '2025-10-05',
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
    node_id: String,
    content: String,
    node_type: String,
    parent_id: String,
    container_node_id: String,
    before_sibling_id: Option<String>,
) -> Result<(), CommandError> {
    validate_node_type(&node_type)?;

    // Use single-transaction upsert method (bypasses NodeOperations for transactional reasons)
    service
        .upsert_node_with_parent(
            &node_id,
            &content,
            &node_type,
            &parent_id,
            &container_node_id,
            before_sibling_id.as_deref(),
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
    service.get_mentions(&node_id).await.map_err(Into::into)
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
    service.get_mentioned_by(&node_id).await.map_err(Into::into)
}

/// Get containers of nodes that mention the target node (backlinks at container level)
///
/// This resolves incoming mentions to their container nodes and deduplicates.
///
/// Unlike `get_incoming_mentions` which returns individual mentioning nodes,
/// this resolves to their container nodes and deduplicates automatically.
///
/// # Container Resolution Logic
/// - For task and ai-chat nodes: Returns the node's own ID (they are their own containers)
/// - For other nodes: Returns their container_node_id (or the node ID itself if it's a root)
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `node_id` - ID of the node to find backlinks for
///
/// # Returns
/// * `Ok(Vec<String>)` - List of unique container node IDs (empty if no backlinks)
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Example
/// If nodes A and B (both children of Container X) mention target node,
/// returns `['container-x-id']` instead of `['node-a-id', 'node-b-id']`
///
/// # Example Frontend Usage
/// ```typescript
/// const containers = await invoke('get_mentioning_containers', {
///   nodeId: 'node-123'
/// });
/// ```
#[tauri::command]
pub async fn get_mentioning_containers(
    service: State<'_, NodeService>,
    node_id: String,
) -> Result<Vec<String>, CommandError> {
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
        .remove_mention(&mentioning_node_id, &mentioned_node_id)
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_type_validation_valid_types() {
        assert!(validate_node_type("text").is_ok());
        assert!(validate_node_type("task").is_ok());
        assert!(validate_node_type("date").is_ok());
    }

    #[test]
    fn test_node_type_validation_invalid_types() {
        // Person and project nodes not in E2E scope
        let result = validate_node_type("person");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, "INVALID_NODE_TYPE");
        assert!(err.message.contains("Only text, task, and date"));

        let result = validate_node_type("project");
        assert!(result.is_err());

        let result = validate_node_type("");
        assert!(result.is_err());

        let result = validate_node_type("unknown");
        assert!(result.is_err());
    }

    #[test]
    fn test_node_type_validation_error_details() {
        let result = validate_node_type("invalid");
        assert!(result.is_err());
        let err = result.unwrap_err();

        // Check that details field contains useful debugging info
        assert!(err.details.is_some());
        let details = err.details.unwrap();
        assert!(details.contains("Allowed types:"));
        assert!(details.contains("invalid"));
    }

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
