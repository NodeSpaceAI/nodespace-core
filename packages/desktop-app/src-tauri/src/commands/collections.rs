//! Collection CRUD operation commands for collection browsing and management
//!
//! Provides Tauri commands for:
//! - Querying collections (list all, get members)
//! - Managing collection membership (add/remove nodes)
//! - Path-based collection operations

use nodespace_core::services::CollectionService;
use nodespace_core::{models, Node, NodeService};
use serde::Serialize;
use serde_json::Value;
use tauri::State;

use super::nodes::CommandError;

use crate::constants::TAURI_CLIENT_ID;

/// Convert a Node to its strongly-typed JSON representation
fn node_to_typed_value(node: Node) -> Result<Value, CommandError> {
    models::node_to_typed_value(node).map_err(|e| CommandError {
        message: e.clone(),
        code: "CONVERSION_ERROR".to_string(),
        details: Some(e),
    })
}

/// Convert a list of Nodes to their strongly-typed JSON representations
fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<Value>, CommandError> {
    models::nodes_to_typed_values(nodes).map_err(|e| CommandError {
        message: e.clone(),
        code: "CONVERSION_ERROR".to_string(),
        details: Some(e),
    })
}

/// Collection with member count and hierarchy info for UI display
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionInfo {
    /// The collection node
    #[serde(flatten)]
    pub node: Value,
    /// Number of direct members in this collection
    pub member_count: usize,
    /// IDs of parent collections (collections this collection is nested under)
    pub parent_collection_ids: Vec<String>,
}

/// Get all collection nodes in the database
///
/// Returns all nodes with node_type = 'collection', useful for building
/// the collection browser UI in the navigation sidebar.
///
/// Uses a single batch query to fetch collections with member counts,
/// avoiding N+1 query pattern for better performance.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
///
/// # Returns
/// * `Ok(Vec<CollectionInfo>)` - All collection nodes with member counts
/// * `Err(CommandError)` - Error if query fails
///
/// # Example Frontend Usage
/// ```typescript
/// const collections = await invoke('get_all_collections');
/// // Returns array of { id, content, nodeType: 'collection', memberCount: number, ... }
/// ```
#[tauri::command]
pub async fn get_all_collections(
    service: State<'_, NodeService>,
) -> Result<Vec<CollectionInfo>, CommandError> {
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    // Single batch query fetches collections with member counts (avoids N+1 pattern)
    let collections_with_counts = collection_service
        .get_all_collections_with_counts()
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to query collections: {}", e),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    let mut result = Vec::with_capacity(collections_with_counts.len());
    for (collection, member_count) in collections_with_counts {
        // Extract parent collection IDs from member_of field
        // Collections can be nested (a collection is member_of another collection)
        let parent_collection_ids = collection.member_of.clone();
        let node_value = node_to_typed_value(collection)?;
        result.push(CollectionInfo {
            node: node_value,
            member_count,
            parent_collection_ids,
        });
    }

    Ok(result)
}

/// Get members of a specific collection
///
/// Returns all nodes that belong to the specified collection via member_of edge.
/// Single query that traverses the relationship and returns full Node data.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `collection_id` - ID of the collection to get members for
///
/// # Returns
/// * `Ok(Vec<Node>)` - Member nodes (empty if collection has no members)
/// * `Err(CommandError)` - Error if query fails
///
/// # Example Frontend Usage
/// ```typescript
/// const members = await invoke('get_collection_members', {
///   collectionId: 'collection-123'
/// });
/// ```
#[tauri::command]
pub async fn get_collection_members(
    service: State<'_, NodeService>,
    collection_id: String,
) -> Result<Vec<Value>, CommandError> {
    let store = service.store();

    // Single query: traverse relationship and get full node data
    let members = store
        .get_collection_members(&collection_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to get collection members: {}", e),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    nodes_to_typed_values(members)
}

/// Get members of a collection recursively (including descendant collections)
///
/// Returns all nodes that belong to the specified collection or any of its
/// descendant collections.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `collection_id` - ID of the collection
///
/// # Returns
/// * `Ok(Vec<Node>)` - All member nodes (empty if no members)
/// * `Err(CommandError)` - Error if query fails
#[tauri::command]
pub async fn get_collection_members_recursive(
    service: State<'_, NodeService>,
    collection_id: String,
) -> Result<Vec<Value>, CommandError> {
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    let member_ids = collection_service
        .get_collection_members_recursive(&collection_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to get recursive collection members: {}", e),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    // Batch fetch all nodes in a single query (avoids N+1 problem)
    let nodes_map = store
        .get_nodes_by_ids(&member_ids)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to batch fetch nodes: {}", e),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    // Preserve order from member_ids and collect found nodes
    let members: Vec<Node> = member_ids
        .into_iter()
        .filter_map(|id| nodes_map.get(&id).cloned())
        .collect();

    nodes_to_typed_values(members)
}

/// Get all collections a node belongs to
///
/// Returns collection node IDs that the specified node is a member of.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `node_id` - ID of the node to query
///
/// # Returns
/// * `Ok(Vec<String>)` - Collection IDs (empty if node not in any collections)
/// * `Err(CommandError)` - Error if query fails
///
/// # Example Frontend Usage
/// ```typescript
/// const collectionIds = await invoke('get_node_collections', {
///   nodeId: 'node-123'
/// });
/// ```
#[tauri::command]
pub async fn get_node_collections(
    service: State<'_, NodeService>,
    node_id: String,
) -> Result<Vec<String>, CommandError> {
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    collection_service
        .get_node_collections(&node_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to get node collections: {}", e),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })
}

/// Add a node to a collection by collection ID
///
/// Creates a member_of edge from the node to the collection.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `node_id` - ID of the node to add
/// * `collection_id` - ID of the collection to add to
///
/// # Returns
/// * `Ok(())` - Node added successfully
/// * `Err(CommandError)` - Error if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('add_node_to_collection', {
///   nodeId: 'node-123',
///   collectionId: 'collection-456'
/// });
/// ```
#[tauri::command]
pub async fn add_node_to_collection(
    service: State<'_, NodeService>,
    node_id: String,
    collection_id: String,
) -> Result<(), CommandError> {
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    collection_service
        .add_to_collection(&node_id, &collection_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to add node to collection: {}", e),
            code: "COLLECTION_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })
}

/// Add a node to a collection by path (creating collections as needed)
///
/// Resolves the path, creates any missing collections, and adds the node.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `node_id` - ID of the node to add
/// * `collection_path` - Path like "hr:policy:vacation"
///
/// # Returns
/// * `Ok(String)` - ID of the leaf collection
/// * `Err(CommandError)` - Error if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// const collectionId = await invoke('add_node_to_collection_path', {
///   nodeId: 'node-123',
///   collectionPath: 'hr:policy:vacation'
/// });
/// ```
#[tauri::command]
pub async fn add_node_to_collection_path(
    service: State<'_, NodeService>,
    node_id: String,
    collection_path: String,
) -> Result<String, CommandError> {
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    let resolved = collection_service
        .add_to_collection_by_path(&node_id, &collection_path)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to add node to collection path: {}", e),
            code: "COLLECTION_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    Ok(resolved.leaf_id().to_string())
}

/// Remove a node from a collection
///
/// Deletes the member_of edge from the node to the collection.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `node_id` - ID of the node to remove
/// * `collection_id` - ID of the collection to remove from
///
/// # Returns
/// * `Ok(())` - Node removed successfully
/// * `Err(CommandError)` - Error if operation fails
///
/// # Example Frontend Usage
/// ```typescript
/// await invoke('remove_node_from_collection', {
///   nodeId: 'node-123',
///   collectionId: 'collection-456'
/// });
/// ```
#[tauri::command]
pub async fn remove_node_from_collection(
    service: State<'_, NodeService>,
    node_id: String,
    collection_id: String,
) -> Result<(), CommandError> {
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    collection_service
        .remove_from_collection(&node_id, &collection_id)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to remove node from collection: {}", e),
            code: "COLLECTION_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })
}

/// Find a collection by path
///
/// Searches for an existing collection matching the given path.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `collection_path` - Path like "hr:policy:vacation"
///
/// # Returns
/// * `Ok(Some(Node))` - Collection node if found
/// * `Ok(None)` - No collection at this path
/// * `Err(CommandError)` - Error if query fails
#[tauri::command]
pub async fn find_collection_by_path(
    service: State<'_, NodeService>,
    collection_path: String,
) -> Result<Option<Value>, CommandError> {
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    let result = collection_service
        .find_collection_by_path(&collection_path)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to find collection: {}", e),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    match result {
        Some(node) => Ok(Some(node_to_typed_value(node)?)),
        None => Ok(None),
    }
}

/// Get collection by name (case-insensitive)
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `name` - Collection name to find
///
/// # Returns
/// * `Ok(Some(Node))` - Collection node if found
/// * `Ok(None)` - No collection with this name
/// * `Err(CommandError)` - Error if query fails
#[tauri::command]
pub async fn get_collection_by_name(
    service: State<'_, NodeService>,
    name: String,
) -> Result<Option<Value>, CommandError> {
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    let result = collection_service
        .get_collection_by_name(&name)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to get collection by name: {}", e),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    match result {
        Some(node) => Ok(Some(node_to_typed_value(node)?)),
        None => Ok(None),
    }
}

/// Create a new collection
///
/// Creates a collection node with the given name.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `name` - Name for the new collection
/// * `description` - Optional description
///
/// # Returns
/// * `Ok(String)` - ID of the created collection
/// * `Err(CommandError)` - Error if collection already exists or creation fails
#[tauri::command]
pub async fn create_collection(
    service: State<'_, NodeService>,
    name: String,
    description: Option<String>,
) -> Result<String, CommandError> {
    use nodespace_core::services::CreateNodeParams;
    use serde_json::json;

    // Check if collection with this name already exists
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    if collection_service
        .get_collection_by_name(&name)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to check collection: {}", e),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?
        .is_some()
    {
        return Err(CommandError {
            message: format!("Collection '{}' already exists", name),
            code: "COLLECTION_EXISTS".to_string(),
            details: None,
        });
    }

    // Create the collection node
    let properties = match description {
        Some(desc) => json!({ "description": desc }),
        None => json!({}),
    };

    let node_id = service
        .with_client(TAURI_CLIENT_ID)
        .create_node_with_parent(CreateNodeParams {
            id: None,
            node_type: "collection".to_string(),
            content: name,
            parent_id: None,
            insert_after_node_id: None,
            properties,
        })
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to create collection: {}", e),
            code: "CREATE_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    Ok(node_id)
}

/// Rename a collection
///
/// Updates the collection's content (name) field.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `collection_id` - ID of the collection to rename
/// * `version` - Expected version for OCC
/// * `new_name` - New name for the collection
///
/// # Returns
/// * `Ok(Node)` - Updated collection node
/// * `Err(CommandError)` - Error if rename fails
#[tauri::command]
pub async fn rename_collection(
    service: State<'_, NodeService>,
    collection_id: String,
    version: i64,
    new_name: String,
) -> Result<Value, CommandError> {
    use nodespace_core::NodeUpdate;

    // Check if name is already taken by another collection
    let store = service.store();
    let collection_service = CollectionService::new(store, &service);

    if let Some(existing) = collection_service
        .get_collection_by_name(&new_name)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to check collection: {}", e),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?
    {
        if existing.id != collection_id {
            return Err(CommandError {
                message: format!("Collection '{}' already exists", new_name),
                code: "COLLECTION_EXISTS".to_string(),
                details: None,
            });
        }
    }

    // Update the collection
    let update = NodeUpdate {
        content: Some(new_name),
        ..Default::default()
    };

    let node = service
        .with_client(TAURI_CLIENT_ID)
        .update_node(&collection_id, version, update)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to rename collection: {}", e),
            code: "UPDATE_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    node_to_typed_value(node)
}

/// Delete a collection
///
/// Deletes the collection node. Member nodes are NOT deleted, only their
/// membership edges are removed.
///
/// # Arguments
/// * `service` - NodeService instance from Tauri state
/// * `collection_id` - ID of the collection to delete
/// * `version` - Expected version for OCC
///
/// # Returns
/// * `Ok(())` - Collection deleted successfully
/// * `Err(CommandError)` - Error if delete fails
#[tauri::command]
pub async fn delete_collection(
    service: State<'_, NodeService>,
    collection_id: String,
    version: i64,
) -> Result<(), CommandError> {
    // Delete the collection node (member_of edges will be cleaned up by cascade)
    service
        .with_client(TAURI_CLIENT_ID)
        .delete_node(&collection_id, version)
        .await
        .map_err(|e| CommandError {
            message: format!("Failed to delete collection: {}", e),
            code: "DELETE_ERROR".to_string(),
            details: Some(format!("{}", e)),
        })?;

    Ok(())
}
