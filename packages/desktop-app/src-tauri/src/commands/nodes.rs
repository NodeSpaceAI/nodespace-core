//! Node CRUD operation commands for Text, Task, and Date nodes

use nodespace_core::{Node, NodeService, NodeServiceError, NodeUpdate};
use serde::Serialize;
use tauri::State;

/// Allowed node types for initial E2E testing
const ALLOWED_NODE_TYPES: &[&str] = &["text", "task", "date"];

/// Structured error type for Tauri commands
///
/// Provides better observability and debugging by including error codes
/// and optional details alongside user-facing messages.
#[derive(Debug, Serialize)]
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
    node: Node,
) -> Result<String, CommandError> {
    validate_node_type(&node.node_type)?;

    service.create_node(node).await.map_err(Into::into)
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
pub async fn delete_node(service: State<'_, NodeService>, id: String) -> Result<(), CommandError> {
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
