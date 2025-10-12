//! Node CRUD operation commands for Text, Task, and Date nodes

use chrono::Utc;
use nodespace_core::{Node, NodeQuery, NodeService, NodeServiceError, NodeUpdate};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

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
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `node` - Node data to create (must have node_type: text|task|date)
///
/// # Returns
/// * `Ok(String)` - ID of the created node
/// * `Err(CommandError)` - Error with details if creation fails
///
/// # Errors
/// Returns error if:
/// - Node type is not one of: text, task, date
/// - Database operation fails
/// - Node validation fails
/// - Required fields are missing
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
    node: CreateNodeInput,
) -> Result<String, CommandError> {
    validate_node_type(&node.node_type)?;

    // Database will auto-generate timestamps via DEFAULT CURRENT_TIMESTAMP
    // We still need to provide placeholder timestamps for the Node struct
    let now = Utc::now();

    let full_node = Node {
        id: node.id,
        node_type: node.node_type,
        content: node.content,
        parent_id: node.parent_id,
        container_node_id: node.container_node_id,
        before_sibling_id: node.before_sibling_id,
        created_at: now,  // Placeholder - DB will use its own timestamp
        modified_at: now, // Placeholder - DB will use its own timestamp
        properties: node.properties,
        embedding_vector: node.embedding_vector,
        mentions: Vec::new(),     // Empty on create - populated separately
        mentioned_by: Vec::new(), // Empty on create - computed from database
    };

    service.create_node(full_node).await.map_err(Into::into)
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
/// Container nodes are root-level nodes that can contain other nodes.
/// They always have:
/// - parent_id = NULL
/// - container_node_id = NULL (they ARE containers)
/// - before_sibling_id = NULL
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
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
    service: State<'_, NodeService>,
    input: CreateContainerNodeInput,
) -> Result<String, CommandError> {
    validate_node_type(&input.node_type)?;

    let now = Utc::now();
    let node_id = Uuid::new_v4().to_string();

    let container_node = Node {
        id: node_id.clone(),
        node_type: input.node_type,
        content: input.content,
        parent_id: None,         // Always null for containers
        container_node_id: None, // Always null for containers (they ARE containers)
        before_sibling_id: None, // No sibling ordering for root nodes
        created_at: now,
        modified_at: now,
        properties: input.properties,
        embedding_vector: None,
        mentions: Vec::new(), // Will be populated when this node mentions others
        mentioned_by: Vec::new(), // Will be computed from node_mentions table
    };

    service.create_node(container_node).await?;

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
/// # Arguments
/// * `service` - Node service instance from Tauri state
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
/// - Database operation fails
/// - Update validation fails
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
    service: State<'_, NodeService>,
    id: String,
    update: NodeUpdate,
) -> Result<(), CommandError> {
    service.update_node(&id, update).await.map_err(Into::into)
}

/// Delete a node by ID
///
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `id` - Unique identifier of the node to delete
///
/// # Returns
/// * `Ok(())` - Deletion successful
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
/// await invoke('delete_node', { id: 'node-123' });
/// ```
#[tauri::command]
pub async fn delete_node(
    service: State<'_, NodeService>,
    id: String,
) -> Result<nodespace_core::models::DeleteResult, CommandError> {
    service.delete_node(&id).await.map_err(Into::into)
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
/// # Arguments
/// * `service` - Node service instance from Tauri state
/// * `container_node_id` - ID of the origin node (e.g., date page ID)
///
/// # Returns
/// * `Ok(Vec<Node>)` - All nodes belonging to this origin
/// * `Err(CommandError)` - Error with details if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const nodes = await invoke('get_nodes_by_origin_id', {
///   originNodeId: '2025-10-05'
/// });
/// console.log(`Loaded ${nodes.length} nodes for this date`);
/// ```
#[tauri::command]
pub async fn get_nodes_by_origin_id(
    service: State<'_, NodeService>,
    container_node_id: String,
) -> Result<Vec<Node>, CommandError> {
    service
        .get_nodes_by_origin_id(&container_node_id)
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

    // Use single-transaction upsert method
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
