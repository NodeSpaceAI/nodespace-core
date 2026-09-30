//! Collection CRUD operation commands for collection browsing and management
//!
//! All commands proxy through the in-process gRPC server
//! (nodespace-daemon) instead of calling `packages/core` directly.

use crate::types::Node;
use nodespace_proto::nodespace::{
    AddNodeToCollectionByPathRequest, AddNodeToCollectionRequest, CollectionMembersRequest,
    CreateCollectionRequest, DeleteCollectionRequest, FindCollectionByPathRequest,
    GetAllCollectionsRequest, GetCollectionByNameRequest, NodeCollectionsRequest,
    RemoveNodeFromCollectionRequest, RenameCollectionRequest,
};
use serde::Serialize;
use serde_json::Value;
use tauri::State;
use tonic::Request;

use super::nodes::{
    node_to_typed_value, nodes_to_typed_values, proto_node_data_to_node, CommandError,
};
use crate::services::GrpcClient;

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
/// # Returns
/// * `Ok(Vec<CollectionInfo>)` - All collection nodes with member counts
/// * `Err(CommandError)` - Error if query fails
#[tauri::command]
pub async fn get_all_collections(
    client: State<'_, GrpcClient>,
) -> Result<Vec<CollectionInfo>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_all_collections(Request::new(GetAllCollectionsRequest {}))
        .await
        .map_err(|s| CommandError {
            message: format!("Failed to query collections: {}", s.message()),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    let collections = resp.into_inner().collections;
    let mut result = Vec::with_capacity(collections.len());
    for proto_info in collections {
        let nd = proto_info.node.ok_or_else(|| CommandError {
            message: "gRPC CollectionInfo missing node_data".to_string(),
            code: "GRPC_ERROR".to_string(),
            details: None,
            conflict_data: None,
        })?;
        let node = proto_node_data_to_node(nd)?;
        let node_value = node_to_typed_value(node)?;
        result.push(CollectionInfo {
            node: node_value,
            member_count: proto_info.member_count as usize,
            parent_collection_ids: proto_info.parent_collection_ids,
        });
    }

    Ok(result)
}

/// Get members of a specific collection
///
/// # Returns
/// * `Ok(Vec<Value>)` - Member nodes (empty if collection has no members)
/// * `Err(CommandError)` - Error if query fails
#[tauri::command]
pub async fn get_collection_members(
    client: State<'_, GrpcClient>,
    collection_id: String,
) -> Result<Vec<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_collection_members(Request::new(CollectionMembersRequest { collection_id }))
        .await
        .map_err(|s| CommandError {
            message: format!("Failed to get collection members: {}", s.message()),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    let nodes: Result<Vec<Node>, CommandError> = resp
        .into_inner()
        .nodes
        .into_iter()
        .map(proto_node_data_to_node)
        .collect();

    nodes_to_typed_values(nodes?)
}

/// Get members of a collection recursively (including descendant collections)
///
/// # Returns
/// * `Ok(Vec<Value>)` - All member nodes (empty if no members)
/// * `Err(CommandError)` - Error if query fails
#[tauri::command]
pub async fn get_collection_members_recursive(
    client: State<'_, GrpcClient>,
    collection_id: String,
) -> Result<Vec<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_collection_members_recursive(Request::new(CollectionMembersRequest { collection_id }))
        .await
        .map_err(|s| CommandError {
            message: format!(
                "Failed to get recursive collection members: {}",
                s.message()
            ),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    let nodes: Result<Vec<Node>, CommandError> = resp
        .into_inner()
        .nodes
        .into_iter()
        .map(proto_node_data_to_node)
        .collect();

    nodes_to_typed_values(nodes?)
}

/// Get all collections a node belongs to
///
/// # Returns
/// * `Ok(Vec<String>)` - Collection IDs (empty if node not in any collections)
/// * `Err(CommandError)` - Error if query fails
#[tauri::command]
pub async fn get_node_collections(
    client: State<'_, GrpcClient>,
    node_id: String,
) -> Result<Vec<String>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_node_collections(Request::new(NodeCollectionsRequest { node_id }))
        .await
        .map_err(|s| CommandError {
            message: format!("Failed to get node collections: {}", s.message()),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    Ok(resp.into_inner().collection_ids)
}

/// Add a node to a collection by collection ID
///
/// # Returns
/// * `Ok(())` - Node added successfully
/// * `Err(CommandError)` - Error if operation fails
#[tauri::command]
pub async fn add_node_to_collection(
    client: State<'_, GrpcClient>,
    node_id: String,
    collection_id: String,
) -> Result<(), CommandError> {
    let mut c = client.client().await;
    c.add_node_to_collection(Request::new(AddNodeToCollectionRequest {
        node_id,
        collection_id,
    }))
    .await
    .map_err(|s| CommandError {
        message: format!("Failed to add node to collection: {}", s.message()),
        code: "COLLECTION_ERROR".to_string(),
        details: Some(format!("{:?}", s.code())),
        conflict_data: None,
    })?;
    Ok(())
}

/// Add a node to a collection by path (creating collections as needed)
///
/// # Returns
/// * `Ok(String)` - ID of the leaf collection
/// * `Err(CommandError)` - Error if operation fails
#[tauri::command]
pub async fn add_node_to_collection_path(
    client: State<'_, GrpcClient>,
    node_id: String,
    collection_path: String,
) -> Result<String, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .add_node_to_collection_by_path(Request::new(AddNodeToCollectionByPathRequest {
            node_id,
            collection_path,
        }))
        .await
        .map_err(|s| CommandError {
            message: format!("Failed to add node to collection path: {}", s.message()),
            code: "COLLECTION_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    Ok(resp.into_inner().collection_id)
}

/// Remove a node from a collection
///
/// # Returns
/// * `Ok(())` - Node removed successfully
/// * `Err(CommandError)` - Error if operation fails
#[tauri::command]
pub async fn remove_node_from_collection(
    client: State<'_, GrpcClient>,
    node_id: String,
    collection_id: String,
) -> Result<(), CommandError> {
    let mut c = client.client().await;
    c.remove_node_from_collection(Request::new(RemoveNodeFromCollectionRequest {
        node_id,
        collection_id,
    }))
    .await
    .map_err(|s| CommandError {
        message: format!("Failed to remove node from collection: {}", s.message()),
        code: "COLLECTION_ERROR".to_string(),
        details: Some(format!("{:?}", s.code())),
        conflict_data: None,
    })?;
    Ok(())
}

/// Find a collection by path
///
/// # Returns
/// * `Ok(Some(Value))` - Collection node if found
/// * `Ok(None)` - No collection at this path
/// * `Err(CommandError)` - Error if query fails
#[tauri::command]
pub async fn find_collection_by_path(
    client: State<'_, GrpcClient>,
    collection_path: String,
) -> Result<Option<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .find_collection_by_path(Request::new(FindCollectionByPathRequest {
            collection_path,
        }))
        .await
        .map_err(|s| CommandError {
            message: format!("Failed to find collection: {}", s.message()),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    match resp.into_inner().node {
        None => Ok(None),
        Some(node_resp) => {
            let nd = node_resp.node_data.ok_or_else(|| CommandError {
                message: "gRPC OptionalNodeResponse missing node_data".to_string(),
                code: "GRPC_ERROR".to_string(),
                details: None,
                conflict_data: None,
            })?;
            let node = proto_node_data_to_node(nd)?;
            Ok(Some(node_to_typed_value(node)?))
        }
    }
}

/// Get collection by name (case-insensitive)
///
/// # Returns
/// * `Ok(Some(Value))` - Collection node if found
/// * `Ok(None)` - No collection with this name
/// * `Err(CommandError)` - Error if query fails
#[tauri::command]
pub async fn get_collection_by_name(
    client: State<'_, GrpcClient>,
    name: String,
) -> Result<Option<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_collection_by_name(Request::new(GetCollectionByNameRequest { name }))
        .await
        .map_err(|s| CommandError {
            message: format!("Failed to get collection by name: {}", s.message()),
            code: "QUERY_ERROR".to_string(),
            details: Some(format!("{:?}", s.code())),
            conflict_data: None,
        })?;

    match resp.into_inner().node {
        None => Ok(None),
        Some(node_resp) => {
            let nd = node_resp.node_data.ok_or_else(|| CommandError {
                message: "gRPC OptionalNodeResponse missing node_data".to_string(),
                code: "GRPC_ERROR".to_string(),
                details: None,
                conflict_data: None,
            })?;
            let node = proto_node_data_to_node(nd)?;
            Ok(Some(node_to_typed_value(node)?))
        }
    }
}

/// Create a new collection
///
/// # Returns
/// * `Ok(String)` - ID of the created collection
/// * `Err(CommandError)` - Error if collection already exists or creation fails
#[tauri::command]
pub async fn create_collection(
    client: State<'_, GrpcClient>,
    name: String,
    description: Option<String>,
) -> Result<String, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .create_collection(Request::new(CreateCollectionRequest {
            name,
            description: description.unwrap_or_default(),
        }))
        .await
        .map_err(|s| {
            let code = if s.code() == tonic::Code::AlreadyExists {
                "COLLECTION_EXISTS"
            } else {
                "CREATE_ERROR"
            };
            CommandError {
                message: format!("Failed to create collection: {}", s.message()),
                code: code.to_string(),
                details: Some(format!("{:?}", s.code())),
                conflict_data: None,
            }
        })?;

    Ok(resp.into_inner().collection_id)
}

/// Rename a collection
///
/// # Returns
/// * `Ok(Value)` - Updated collection node
/// * `Err(CommandError)` - Error if rename fails
#[tauri::command]
pub async fn rename_collection(
    client: State<'_, GrpcClient>,
    collection_id: String,
    version: i64,
    new_name: String,
) -> Result<Value, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .rename_collection(Request::new(RenameCollectionRequest {
            collection_id,
            version,
            new_name,
        }))
        .await
        .map_err(|s| {
            let code = if s.code() == tonic::Code::AlreadyExists {
                "COLLECTION_EXISTS"
            } else {
                "UPDATE_ERROR"
            };
            CommandError {
                message: format!("Failed to rename collection: {}", s.message()),
                code: code.to_string(),
                details: Some(format!("{:?}", s.code())),
                conflict_data: None,
            }
        })?;

    let nd = resp.into_inner().node_data.ok_or_else(|| CommandError {
        message: "gRPC RenameCollection response missing node_data".to_string(),
        code: "GRPC_ERROR".to_string(),
        details: None,
        conflict_data: None,
    })?;
    let node = proto_node_data_to_node(nd)?;
    node_to_typed_value(node)
}

/// Delete a collection
///
/// Deletes the collection node. Member nodes are NOT deleted, only their
/// membership edges are removed.
///
/// # Returns
/// * `Ok(())` - Collection deleted successfully
/// * `Err(CommandError)` - Error if delete fails
#[tauri::command]
pub async fn delete_collection(
    client: State<'_, GrpcClient>,
    collection_id: String,
    version: i64,
) -> Result<(), CommandError> {
    let mut c = client.client().await;
    c.delete_collection(Request::new(DeleteCollectionRequest {
        collection_id,
        version,
    }))
    .await
    .map_err(|s| CommandError {
        message: format!("Failed to delete collection: {}", s.message()),
        code: "DELETE_ERROR".to_string(),
        details: Some(format!("{:?}", s.code())),
        conflict_data: None,
    })?;
    Ok(())
}
