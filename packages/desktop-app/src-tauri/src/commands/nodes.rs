//! Node CRUD operation commands for Text, Task, and Date nodes
//!
//! All commands proxy through the in-process gRPC server
//! (nodespace-daemon) instead of calling `packages/core` directly.

use crate::types::{
    node_to_typed_value as types_node_to_typed_value,
    nodes_to_typed_values as types_nodes_to_typed_values, DeleteResult, Node, NodeQuery,
    NodeReference, NodeUpdate, TaskNodeUpdate,
};
use chrono::{DateTime, Utc};
use nodespace_proto::nodespace::{
    ChildMove, CreateMentionRequest, CreateNodeRequest, CreateRelationshipRequest,
    DeleteMentionRequest, DeleteNodeRequest, DeleteRelationshipRequest, FindDuplicateRequest,
    GetChildrenRequest, GetChildrenTreeRequest, GetNodeRelationshipsRequest, GetNodeRequest,
    GetSchemaDefinitionRequest, MentionAutocompleteRequest, MentionTargetRequest,
    MoveChildrenToParentRequest, MoveNodeRequest, NodeData, NodeResponse, NodeSortOrder,
    OptionalStringClear, OptionalTimestampClear, QueryNodesSimpleRequest, ReorderNodeRequest,
    UpdateNodeRequest, UpdateRelationshipPropertiesRequest, UpdateTaskNodeRequest,
    UpsertNodeWithParentRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, State};
use tonic::Request;

use crate::services::GrpcClient;

/// Per-child OCC token for `move_children_to_parent`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildMoveInput {
    pub node_id: String,
    pub version: i64,
}

/// Explicit insertion position for new/moved nodes.
///
/// Serializes as `{"type":"beginning"}`, `{"type":"end"}`, or
/// `{"type":"after","siblingId":"<uuid>"}` from the TypeScript side.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum InsertPositionInput {
    Beginning,
    End,
    After {
        #[serde(rename = "siblingId")]
        sibling_id: String,
    },
}

impl InsertPositionInput {
    /// Encode into the proto oneof field for CreateNodeRequest.
    pub fn into_create_proto_position(
        self,
    ) -> Option<nodespace_proto::nodespace::create_node_request::Position> {
        use nodespace_proto::nodespace::create_node_request::Position;
        Some(match self {
            InsertPositionInput::Beginning => Position::Beginning(true),
            InsertPositionInput::End => Position::End(true),
            InsertPositionInput::After { sibling_id } => Position::After(sibling_id),
        })
    }

    /// Encode into the proto oneof field for MoveNodeRequest.
    pub fn into_move_proto_position(
        self,
    ) -> Option<nodespace_proto::nodespace::move_node_request::Position> {
        use nodespace_proto::nodespace::move_node_request::Position;
        Some(match self {
            InsertPositionInput::Beginning => Position::Beginning(true),
            InsertPositionInput::End => Position::End(true),
            InsertPositionInput::After { sibling_id } => Position::After(sibling_id),
        })
    }

    /// Encode into the proto oneof field for ReorderNodeRequest.
    pub fn into_reorder_proto_position(
        self,
    ) -> Option<nodespace_proto::nodespace::reorder_node_request::Position> {
        use nodespace_proto::nodespace::reorder_node_request::Position;
        Some(match self {
            InsertPositionInput::Beginning => Position::Beginning(true),
            InsertPositionInput::End => Position::End(true),
            InsertPositionInput::After { sibling_id } => Position::After(sibling_id),
        })
    }
}

/// Input for creating a node - timestamps generated server-side
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNodeInput {
    pub id: String,
    pub node_type: String,
    pub content: String,
    pub parent_id: Option<String>,
    /// Where to insert the new node among siblings. Omit (or null) for End.
    #[serde(default)]
    pub insert_position: Option<InsertPositionInput>,
    pub properties: serde_json::Value,
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
    /// Structured conflict payload for VERSION_CONFLICT errors
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_data: Option<serde_json::Value>,
}

pub(crate) fn status_to_command_error(status: tonic::Status) -> CommandError {
    // A cascade delete refused by the ADR-041 subtree access gate carries the
    // inaccessible-node count in `x-subtree-inaccessible-count` metadata. Only a
    // FailedPrecondition WITH that metadata is an access refusal — the daemon
    // returns FailedPrecondition from unrelated paths too (node-create/schema
    // failures), which must not be branded as a security refusal.
    let subtree_inaccessible_count: Option<u64> =
        if status.code() == tonic::Code::FailedPrecondition {
            status
                .metadata()
                .get("x-subtree-inaccessible-count")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
        } else {
            None
        };

    let code = match status.code() {
        tonic::Code::NotFound => "NODE_NOT_FOUND",
        tonic::Code::Aborted => "VERSION_CONFLICT",
        tonic::Code::AlreadyExists => "COLLECTION_EXISTS",
        tonic::Code::InvalidArgument => "INVALID_ARGUMENT",
        // Distinct from ordinary validation so the frontend can restore the
        // optimistically-removed node and show a dedicated refusal modal.
        tonic::Code::FailedPrecondition if subtree_inaccessible_count.is_some() => {
            "SUBTREE_ACCESS_DENIED"
        }
        _ => "GRPC_ERROR",
    }
    .to_string();

    let conflict_data = if status.code() == tonic::Code::Aborted {
        status
            .metadata()
            .get("x-version-conflict")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| serde_json::from_str(s).ok())
    } else {
        // Present only for a genuine subtree refusal (count metadata parsed above).
        subtree_inaccessible_count.map(
            |inaccessible_count| serde_json::json!({ "inaccessibleCount": inaccessible_count }),
        )
    };

    CommandError {
        message: status.message().to_string(),
        code,
        details: Some(format!("{:?}", status.code())),
        conflict_data,
    }
}

/// Convert proto NodeData → core Node
pub(crate) fn proto_node_data_to_node(nd: NodeData) -> Result<Node, CommandError> {
    let properties = serde_json::from_str::<serde_json::Value>(&nd.properties)
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let created_at = DateTime::parse_from_rfc3339(&nd.created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| CommandError {
            message: format!("Invalid created_at timestamp: {}", e),
            code: "PARSE_ERROR".to_string(),
            details: Some(nd.created_at.clone()),
            conflict_data: None,
        })?;
    let modified_at = DateTime::parse_from_rfc3339(&nd.modified_at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| CommandError {
            message: format!("Invalid modified_at timestamp: {}", e),
            code: "PARSE_ERROR".to_string(),
            details: Some(nd.modified_at.clone()),
            conflict_data: None,
        })?;

    Ok(Node {
        id: nd.id,
        node_type: nd.node_type,
        content: nd.content,
        version: nd.version,
        created_at,
        modified_at,
        properties,
        lifecycle_status: nd.lifecycle_status,
        mentions: vec![],
        mentioned_in: vec![],
        title: None,
    })
}

/// Convert proto NodeResponse → core Node
fn proto_node_response_to_node(resp: NodeResponse) -> Result<Node, CommandError> {
    let nd = resp.node_data.ok_or_else(|| CommandError {
        message: "gRPC response missing node_data".to_string(),
        code: "GRPC_ERROR".to_string(),
        details: None,
        conflict_data: None,
    })?;
    proto_node_data_to_node(nd)
}

/// Validate that node type has a schema via gRPC GetSchemaDefinition RPC
async fn validate_node_type(
    node_type: &str,
    client: &mut crate::services::NodeClient,
) -> Result<(), CommandError> {
    match client
        .get_schema_definition(Request::new(GetSchemaDefinitionRequest {
            schema_id: node_type.to_string(),
        }))
        .await
    {
        Ok(_) => Ok(()),
        Err(s)
            if s.code() == tonic::Code::NotFound || s.code() == tonic::Code::FailedPrecondition =>
        {
            Err(CommandError {
                message: format!("No schema found for node type: {}", node_type),
                code: "SCHEMA_NOT_FOUND".to_string(),
                details: None,
                conflict_data: None,
            })
        }
        Err(s) => Err(status_to_command_error(s)),
    }
}

/// Convert a Node to its strongly-typed JSON representation
pub fn node_to_typed_value(node: Node) -> Result<Value, CommandError> {
    types_node_to_typed_value(node).map_err(|e| CommandError {
        message: e.clone(),
        code: "CONVERSION_ERROR".to_string(),
        details: Some(e),
        conflict_data: None,
    })
}

/// Convert a list of Nodes to their strongly-typed JSON representations
pub fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<Value>, CommandError> {
    types_nodes_to_typed_values(nodes).map_err(|e| CommandError {
        message: e.clone(),
        code: "CONVERSION_ERROR".to_string(),
        details: Some(e),
        conflict_data: None,
    })
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
    // before_sibling_id removed - backend uses fractional ordering on has_child edges
}

/// Create a new node of any type with a registered schema
#[tauri::command]
pub async fn create_node(
    client: State<'_, GrpcClient>,
    node: CreateNodeInput,
) -> Result<String, CommandError> {
    let mut c = client.client().await;
    validate_node_type(&node.node_type, &mut c).await?;

    let properties_str = node.properties.to_string();
    let position = node
        .insert_position
        .and_then(InsertPositionInput::into_create_proto_position);
    let resp = c
        .create_node(Request::new(CreateNodeRequest {
            id: if node.id.is_empty() {
                None
            } else {
                Some(node.id)
            },
            node_type: node.node_type,
            content: node.content,
            parent_id: node.parent_id,
            position,
            properties: properties_str,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        }))
        .await
        .map_err(status_to_command_error)?;

    Ok(resp.into_inner().node_id)
}

/// Create a new root node (top-level node that can contain other nodes)
#[tauri::command]
pub async fn create_root_node(
    client: State<'_, GrpcClient>,
    input: CreateRootNodeInput,
) -> Result<String, CommandError> {
    let mut c = client.client().await;
    validate_node_type(&input.node_type, &mut c).await?;

    let properties_str = input.properties.to_string();
    let resp = c
        .create_node(Request::new(CreateNodeRequest {
            id: None,
            node_type: input.node_type,
            content: input.content,
            parent_id: None,
            position: None,
            properties: properties_str,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
        }))
        .await
        .map_err(status_to_command_error)?;

    let node_id = resp.into_inner().node_id;

    // If mentioned_by is provided, create mention relationship
    if let Some(mentioning_node_id) = input.mentioned_by {
        c.create_mention(Request::new(CreateMentionRequest {
            mentioning_node_id,
            mentioned_node_id: node_id.clone(),
        }))
        .await
        .map_err(status_to_command_error)?;
    }

    Ok(node_id)
}

/// Create a mention relationship between two nodes
#[tauri::command]
pub async fn create_node_mention(
    client: State<'_, GrpcClient>,
    mentioning_node_id: String,
    mentioned_node_id: String,
) -> Result<(), CommandError> {
    let mut c = client.client().await;
    c.create_mention(Request::new(CreateMentionRequest {
        mentioning_node_id,
        mentioned_node_id,
    }))
    .await
    .map_err(status_to_command_error)?;
    Ok(())
}

/// Get a node by ID
#[tauri::command]
pub async fn get_node(
    client: State<'_, GrpcClient>,
    id: String,
) -> Result<Option<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_node(Request::new(GetNodeRequest { node_id: id }))
        .await;

    match resp {
        Ok(r) => {
            let node = proto_node_response_to_node(r.into_inner())?;
            Ok(Some(node_to_typed_value(node)?))
        }
        Err(s) if s.code() == tonic::Code::NotFound => Ok(None),
        Err(s) => Err(status_to_command_error(s)),
    }
}

/// Suggest-don't-block uniqueness lookup (ADR-065): returns the
/// existing active node whose `field` matches `value` for `node_type`, or
/// `None` when the field isn't flagged `unique` on the schema, `value` is
/// empty, or there is simply no conflict. This never errors on "no match" —
/// unlike `get_node`, a miss is not a `NotFound` gRPC status but an ordinary
/// empty `NodeResponse` (empty `node_id`, no `node_data`), so callers use the
/// result to offer an adopt-existing suggestion, never to reject a write.
///
/// `exclude_id` should be the id of the node the caller is creating/editing —
/// pass it whenever that node's own value could already be `value` (e.g. a
/// check that runs concurrently with, or after, that node's own save), or the
/// lookup can spuriously match the node against itself and hide a real,
/// different duplicate (the query has no `ORDER BY`, so with two matching
/// rows — the node itself and the real duplicate — which one comes back is
/// unspecified).
#[tauri::command]
pub async fn find_duplicate(
    client: State<'_, GrpcClient>,
    node_type: String,
    field: String,
    value: String,
    exclude_id: Option<String>,
) -> Result<Option<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .find_duplicate(Request::new(FindDuplicateRequest {
            node_type,
            field,
            value,
            exclude_id,
        }))
        .await
        .map_err(status_to_command_error)?
        .into_inner();

    match resp.node_data {
        Some(nd) => {
            let node = proto_node_data_to_node(nd)?;
            Ok(Some(node_to_typed_value(node)?))
        }
        None => Ok(None),
    }
}

/// Probe the shared gRPC channel for a wedge and recover it in place.
///
/// The desktop app rides every service client and the `WatchNodes` stream on a
/// single h2 channel (see [`GrpcClient`]). A stream churn during a heavy sync
/// can wedge that connection: reads and writes then hang forever (the lazy
/// channel carries no client-side timeout) even though the daemon socket is
/// healthy — a freshly-dialed client answers fine. The socket-based
/// `check_daemon_status` therefore keeps reporting "healthy" and
/// `onDaemonReconnect` never fires, so the journal stays stuck on "Loading…".
///
/// This runs one real, lightweight RPC bounded by a short timeout. A `NotFound`
/// (the sentinel id never exists) — or any other completion — proves the
/// channel is live and returns `false`. A timeout means the channel is wedged:
/// rebuild it with [`GrpcClient::reconnect`] and re-probe once, returning `true`
/// only if the rebuilt channel answers (the frontend then re-fires its
/// reconnect listeners so panes re-fetch on the fresh channel).
///
/// The Pro cloud-sync client caches its own clone of the shared channel, so
/// after a rebuild it is rebound to the fresh channel too — otherwise every
/// subsequent cloud-sync call would keep riding the dead connection.
#[tauri::command]
pub async fn probe_and_recover_channel(
    app: AppHandle,
    client: State<'_, GrpcClient>,
) -> Result<bool, ()> {
    use std::time::Duration;
    const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
    const PROBE_ID: &str = "__ns_channel_probe__";

    // A completed RPC (any status, including NotFound) means the channel is
    // alive; only a timeout indicates a wedge.
    async fn probe(client: &GrpcClient) -> Result<(), tokio::time::error::Elapsed> {
        let mut c = client.client().await;
        tokio::time::timeout(PROBE_TIMEOUT, async move {
            let _ = c
                .get_node(Request::new(GetNodeRequest {
                    node_id: PROBE_ID.to_string(),
                }))
                .await;
        })
        .await
    }

    if probe(client.inner()).await.is_ok() {
        return Ok(false);
    }

    tracing::warn!("gRPC channel probe timed out — rebuilding wedged channel");
    client.reconnect().await;
    // The Pro client holds its own clone of the shared channel; point it at the
    // freshly-rebuilt one so cloud-sync calls stop riding the dead connection.
    // Absent in community mode — skip cleanly if it was never registered.
    if let Some(pro) = app.try_state::<crate::services::ProClient>() {
        pro.rebind(client.channel().await).await;
    }
    Ok(probe(client.inner()).await.is_ok())
}

/// Update an existing node
#[tauri::command]
pub async fn update_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: NodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.client().await;

    let content_preview = update.content.as_ref().map(|c| {
        if c.len() > 50 {
            format!("{}...", &c[..50])
        } else {
            c.clone()
        }
    });
    tracing::debug!(
        "update_node: id={}, version={}, content={:?}, node_type={:?}",
        id,
        version,
        content_preview,
        update.node_type
    );

    let req = UpdateNodeRequest {
        node_id: id.clone(),
        version: Some(version),
        node_type: update.node_type,
        content: update.content,
        properties: update.properties.map(|p| p.to_string()),
        add_to_collections: Vec::new(),
        add_to_collection_ids: Vec::new(),
        remove_from_collection_ids: Vec::new(),
        lifecycle_status: update.lifecycle_status,
    };

    let resp = c
        .update_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;

    tracing::debug!(
        "update_node: SUCCESS id={}, new_version={}",
        id,
        node.version
    );

    node_to_typed_value(node)
}

/// Delete a node by ID with cascade deletion
#[tauri::command]
pub async fn delete_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
) -> Result<DeleteResult, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .delete_node(Request::new(DeleteNodeRequest {
            node_id: id,
            version: Some(version),
        }))
        .await
        .map_err(status_to_command_error)?;

    let dr = resp.into_inner();
    Ok(DeleteResult {
        existed: dr.existed,
        deleted_count: dr.deleted_count,
    })
}

/// Atomically move a node to a new parent with new sibling position (with OCC)
#[tauri::command]
pub async fn move_node(
    client: State<'_, GrpcClient>,
    node_id: String,
    version: i64,
    new_parent_id: Option<String>,
    insert_position: Option<InsertPositionInput>,
) -> Result<Value, CommandError> {
    let mut c = client.client().await;
    let position = insert_position.and_then(InsertPositionInput::into_move_proto_position);
    let resp = c
        .move_node(Request::new(MoveNodeRequest {
            node_id,
            version,
            new_parent_id,
            position,
        }))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Reorder a node by changing its sibling position
#[tauri::command]
pub async fn reorder_node(
    client: State<'_, GrpcClient>,
    node_id: String,
    version: i64,
    insert_position: Option<InsertPositionInput>,
) -> Result<(), CommandError> {
    let mut c = client.client().await;
    let position = insert_position.and_then(InsertPositionInput::into_reorder_proto_position);
    c.reorder_node(Request::new(ReorderNodeRequest {
        node_id,
        version,
        position,
    }))
    .await
    .map_err(status_to_command_error)?;
    Ok(())
}

/// Atomically re-parent an ordered set of children to a new parent (one RPC, one transaction)
#[tauri::command]
pub async fn move_children_to_parent(
    client: State<'_, GrpcClient>,
    new_parent_id: String,
    children: Vec<ChildMoveInput>,
) -> Result<Vec<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .move_children_to_parent(Request::new(MoveChildrenToParentRequest {
            new_parent_id,
            children: children
                .into_iter()
                .map(|cm| ChildMove {
                    node_id: cm.node_id,
                    version: cm.version,
                })
                .collect(),
        }))
        .await
        .map_err(status_to_command_error)?;

    let nodes: Result<Vec<Node>, CommandError> = resp
        .into_inner()
        .children
        .into_iter()
        .map(proto_node_data_to_node)
        .collect();

    nodes_to_typed_values(nodes?)
}

/// Get child nodes of a parent node
#[tauri::command]
pub async fn get_children(
    client: State<'_, GrpcClient>,
    parent_id: String,
) -> Result<Vec<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_children(Request::new(GetChildrenRequest { node_id: parent_id }))
        .await
        .map_err(status_to_command_error)?;

    let nodes: Result<Vec<Node>, CommandError> = resp
        .into_inner()
        .nodes
        .into_iter()
        .map(proto_node_data_to_node)
        .collect();

    nodes_to_typed_values(nodes?)
}

/// Get a node with its entire subtree as a nested tree structure
#[tauri::command]
pub async fn get_children_tree(
    client: State<'_, GrpcClient>,
    parent_id: String,
) -> Result<serde_json::Value, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_children_tree(Request::new(GetChildrenTreeRequest { node_id: parent_id }))
        .await
        .map_err(status_to_command_error)?;

    let tree_json = resp.into_inner().tree_json;
    serde_json::from_str(&tree_json).map_err(|e| CommandError {
        message: format!("Failed to parse tree JSON: {}", e),
        code: "PARSE_ERROR".to_string(),
        details: Some(tree_json),
        conflict_data: None,
    })
}

/// Bulk fetch all nodes belonging to a root node (viewer/page)
#[tauri::command]
pub async fn get_nodes_by_root_id(
    client: State<'_, GrpcClient>,
    root_id: String,
) -> Result<Vec<Value>, CommandError> {
    let mut c = client.client().await;
    // Phase 5: Redirect to get_children (graph-native)
    let resp = c
        .get_children(Request::new(GetChildrenRequest { node_id: root_id }))
        .await
        .map_err(status_to_command_error)?;

    let nodes: Result<Vec<Node>, CommandError> = resp
        .into_inner()
        .nodes
        .into_iter()
        .map(proto_node_data_to_node)
        .collect();

    nodes_to_typed_values(nodes?)
}

/// Query nodes with flexible filtering
#[tauri::command]
pub async fn query_nodes_simple(
    client: State<'_, GrpcClient>,
    query: NodeQuery,
) -> Result<Vec<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .query_nodes_simple(Request::new(QueryNodesSimpleRequest {
            id: query.id,
            mentioned_by: query.mentioned_by,
            content_contains: query.content_contains,
            title_contains: query.title_contains,
            node_type: query.node_type,
            limit: query.limit.unwrap_or(0) as u32,
            offset: query.offset.unwrap_or(0) as u32,
            // No frontend order-by control yet — the server's deterministic
            // default (created ascending) still makes limit/offset paging
            // stable, which is a strict improvement over the prior
            // unordered behavior.
            order_by: NodeSortOrder::Unspecified as i32,
        }))
        .await
        .map_err(status_to_command_error)?;

    let nodes: Result<Vec<Node>, CommandError> = resp
        .into_inner()
        .nodes
        .into_iter()
        .map(proto_node_data_to_node)
        .collect();

    nodes_to_typed_values(nodes?)
}

/// Mention autocomplete query - specialized endpoint for @mention feature
#[tauri::command]
pub async fn mention_autocomplete(
    client: State<'_, GrpcClient>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .mention_autocomplete(Request::new(MentionAutocompleteRequest {
            query,
            limit: limit.unwrap_or(0) as u32,
        }))
        .await
        .map_err(status_to_command_error)?;

    let nodes: Result<Vec<Node>, CommandError> = resp
        .into_inner()
        .nodes
        .into_iter()
        .map(proto_node_data_to_node)
        .collect();

    nodes_to_typed_values(nodes?)
}

/// Save a node with automatic parent creation - unified upsert operation
#[tauri::command]
pub async fn save_node_with_parent(
    client: State<'_, GrpcClient>,
    input: SaveNodeWithParentInput,
) -> Result<(), CommandError> {
    let mut c = client.client().await;
    validate_node_type(&input.node_type, &mut c).await?;

    c.upsert_node_with_parent(Request::new(UpsertNodeWithParentRequest {
        node_id: input.node_id,
        content: input.content,
        node_type: input.node_type,
        parent_id: input.parent_id,
        root_id: input.root_id,
    }))
    .await
    .map_err(status_to_command_error)?;

    Ok(())
}

/// Get outgoing mentions (nodes that this node mentions)
#[tauri::command]
pub async fn get_outgoing_mentions(
    client: State<'_, GrpcClient>,
    node_id: String,
) -> Result<Vec<String>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_outgoing_mentions(Request::new(MentionTargetRequest { node_id }))
        .await
        .map_err(status_to_command_error)?;

    Ok(resp.into_inner().node_ids)
}

/// Get incoming mentions (nodes that mention this node - BACKLINKS)
#[tauri::command]
pub async fn get_incoming_mentions(
    client: State<'_, GrpcClient>,
    node_id: String,
) -> Result<Vec<String>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_incoming_mentions(Request::new(MentionTargetRequest { node_id }))
        .await
        .map_err(status_to_command_error)?;

    Ok(resp.into_inner().node_ids)
}

/// Get root nodes of nodes that mention the target node (backlinks at root level)
#[tauri::command]
pub async fn get_mentioning_roots(
    client: State<'_, GrpcClient>,
    node_id: String,
) -> Result<Vec<NodeReference>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_mentioning_roots(Request::new(MentionTargetRequest { node_id }))
        .await
        .map_err(status_to_command_error)?;

    let references = resp
        .into_inner()
        .references
        .into_iter()
        .map(|r| NodeReference {
            id: r.id,
            title: r.title,
            node_type: r.node_type,
        })
        .collect();

    Ok(references)
}

/// Build UpdateTaskNodeRequest from TaskNodeUpdate
fn task_update_to_proto(id: &str, version: i64, update: TaskNodeUpdate) -> UpdateTaskNodeRequest {
    UpdateTaskNodeRequest {
        node_id: id.to_string(),
        version,
        status: update.status.map(|s| s.as_str().to_string()),
        priority: update.priority.map(|opt| match opt {
            None => OptionalStringClear {
                clear: true,
                value: String::new(),
            },
            Some(p) => OptionalStringClear {
                clear: false,
                value: p.as_str().to_string(),
            },
        }),
        due_date: update.due_date.map(|opt| match opt {
            None => OptionalTimestampClear {
                clear: true,
                value: String::new(),
            },
            Some(s) => OptionalTimestampClear {
                clear: false,
                value: s,
            },
        }),
        started_at: update.started_at.map(|opt| match opt {
            None => OptionalTimestampClear {
                clear: true,
                value: String::new(),
            },
            Some(s) => OptionalTimestampClear {
                clear: false,
                value: s,
            },
        }),
        completed_at: update.completed_at.map(|opt| match opt {
            None => OptionalTimestampClear {
                clear: true,
                value: String::new(),
            },
            Some(s) => OptionalTimestampClear {
                clear: false,
                value: s,
            },
        }),
        content: update.content,
        properties: None,
    }
}

/// Update a task node with type-safe property updates
#[tauri::command]
pub async fn update_task_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: TaskNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.client().await;
    let req = task_update_to_proto(&id, version, update);
    let resp = c
        .update_task_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// List a node's schema-declared typed relationships (read-only).
///
/// Returns the aggregate assembled by `rel_ops::get_node_relationships`: the
/// node's typed relationships grouped by (name, direction) across BOTH
/// directions (outbound declared on the node's own schema + inbound resolved via
/// the relationship cache), each related node carrying its connecting edge's
/// properties. The daemon serializes the well-typed Rust struct to JSON; this
/// command parses it back into a `serde_json::Value` for the frontend (matching
/// `get_children_tree`). Built-in structural relationships (`has_child`,
/// `mentions`, `member_of`, `has_role`) are excluded by the aggregation.
#[tauri::command]
pub async fn get_node_relationships(
    client: State<'_, GrpcClient>,
    node_id: String,
) -> Result<Value, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_node_relationships(Request::new(GetNodeRelationshipsRequest { node_id }))
        .await
        .map_err(status_to_command_error)?;

    let json = resp.into_inner().relationships_json;
    serde_json::from_str(&json).map_err(|e| CommandError {
        message: format!("Failed to parse relationships JSON: {}", e),
        code: "PARSE_ERROR".to_string(),
        details: Some(json),
        conflict_data: None,
    })
}

/// Create a schema-declared typed relationship edge between two nodes.
///
/// Wraps `rel_ops::create_relationship`: the daemon validates the relationship
/// against the source node's schema (target type, cardinality) before writing.
/// `edge_data` carries the edge's `edge_fields` values as a JSON object; omit or
/// pass `null` for a bare edge. Returns `()` — the frontend reloads via
/// `get_node_relationships` to see the new edge in context.
#[tauri::command]
pub async fn create_relationship(
    client: State<'_, GrpcClient>,
    source_id: String,
    relationship_name: String,
    target_id: String,
    edge_data: Option<Value>,
) -> Result<(), CommandError> {
    let edge_data_json = match edge_data {
        Some(v) if !v.is_null() => Some(serde_json::to_string(&v).map_err(|e| CommandError {
            message: format!("Failed to serialize edge_data: {}", e),
            code: "SERIALIZE_ERROR".to_string(),
            details: None,
            conflict_data: None,
        })?),
        _ => None,
    };
    let mut c = client.client().await;
    c.create_relationship(Request::new(CreateRelationshipRequest {
        source_id,
        relationship_name,
        target_id,
        edge_data_json,
    }))
    .await
    .map_err(status_to_command_error)?;
    Ok(())
}

/// Delete a schema-declared typed relationship edge.
///
/// Wraps `rel_ops::delete_relationship`. Idempotent — deleting a nonexistent
/// edge succeeds. The daemon rejects removing the last edge of a `required`
/// relationship; that surfaces here as a `CommandError` the caller should show.
#[tauri::command]
pub async fn delete_relationship(
    client: State<'_, GrpcClient>,
    source_id: String,
    relationship_name: String,
    target_id: String,
) -> Result<(), CommandError> {
    let mut c = client.client().await;
    c.delete_relationship(Request::new(DeleteRelationshipRequest {
        source_id,
        relationship_name,
        target_id,
    }))
    .await
    .map_err(status_to_command_error)?;
    Ok(())
}

/// Replace the edge attributes on an existing typed relationship edge.
///
/// Wraps `rel_ops::update_relationship_properties`: overwrites the edge's stored
/// `properties` (its `edge_fields` values) wholesale with `properties`. The edge
/// must already exist — a missing edge surfaces as a `CommandError`. Edits values
/// only; endpoints are immutable (remove + re-add to re-point an edge).
#[tauri::command]
pub async fn update_relationship_properties(
    client: State<'_, GrpcClient>,
    source_id: String,
    relationship_name: String,
    target_id: String,
    properties: Value,
) -> Result<(), CommandError> {
    let properties_json = serde_json::to_string(&properties).map_err(|e| CommandError {
        message: format!("Failed to serialize properties: {}", e),
        code: "SERIALIZE_ERROR".to_string(),
        details: None,
        conflict_data: None,
    })?;
    let mut c = client.client().await;
    c.update_relationship_properties(Request::new(UpdateRelationshipPropertiesRequest {
        source_id,
        relationship_name,
        target_id,
        properties_json,
    }))
    .await
    .map_err(status_to_command_error)?;
    Ok(())
}

/// Delete a mention relationship between two nodes
#[tauri::command]
pub async fn delete_node_mention(
    client: State<'_, GrpcClient>,
    mentioning_node_id: String,
    mentioned_node_id: String,
) -> Result<(), CommandError> {
    let mut c = client.client().await;
    c.delete_mention(Request::new(DeleteMentionRequest {
        mentioning_node_id,
        mentioned_node_id,
    }))
    .await
    .map_err(status_to_command_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_error_serialization() {
        let err = CommandError {
            message: "Test error".to_string(),
            code: "TEST_ERROR".to_string(),
            details: Some("Debug info".to_string()),
            conflict_data: None,
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
            conflict_data: None,
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Simple error"));
        // Details field should be omitted when None
        assert!(!json.contains("details"));
    }
}
