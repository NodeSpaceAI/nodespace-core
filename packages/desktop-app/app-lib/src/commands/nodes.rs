//! Node CRUD operation commands for Text, Task, and Date nodes
//!
//! All commands proxy through the in-process gRPC server
//! (nodespace-daemon) instead of calling `packages/core` directly.

use crate::types::{
    node_to_typed_value as types_node_to_typed_value,
    nodes_to_typed_values as types_nodes_to_typed_values, CollectionNodeUpdate,
    DatabaseSettingsNodeUpdate, DecisionNodeUpdate, DeleteResult, Node, NodeQuery, NodeReference,
    NodeUpdate, PersonNodeUpdate, PlanNodeUpdate, PlayNodeUpdate, Priority, ProjectNodeUpdate,
    QueryNodeUpdate, SkillNodeUpdate, SpecNodeUpdate, TaskNodeUpdate,
};
use chrono::{DateTime, Utc};
use nodespace_proto::nodespace::{
    ChildMove, CreateMentionRequest, CreateNodeRequest, CreateRelationshipRequest,
    DeleteMentionRequest, DeleteNodeRequest, DeleteRelationshipRequest, ExecuteQueryRequest,
    FindDuplicateRequest, GetChildrenRequest, GetChildrenTreeRequest, GetNodeRelationshipsRequest,
    GetNodeRequest, GetRelatedNodesRequest, GetSchemaDefinitionRequest, MentionAutocompleteRequest,
    MentionTargetRequest, MoveChildrenToParentRequest, MoveNodeRequest, NodeData, NodeResponse,
    NodeSortOrder, OptionalJsonClear, OptionalStringClear, OptionalTimestampClear,
    QueryNodesSimpleRequest, ReorderNodeRequest, UpdateCollectionNodeRequest,
    UpdateDatabaseSettingsNodeRequest, UpdateDecisionNodeRequest, UpdateNodeRequest,
    UpdatePersonNodeRequest, UpdatePlanNodeRequest, UpdatePlayNodeRequest,
    UpdateProjectNodeRequest, UpdateQueryNodeRequest, UpdateRelationshipPropertiesRequest,
    UpdateSkillNodeRequest, UpdateSpecNodeRequest, UpdateTaskNodeRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, State};
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
    /// What a refused database requires and how to present the refusal, on a
    /// REQUIRES_EXTENSION error only (ADR-083 §2). Serialized as
    /// `requiresExtension`. Boxed so the payload, present on one error code,
    /// does not grow every `Result<_, CommandError>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_extension: Option<Box<RequiresExtensionPayload>>,
}

/// The payload of a REQUIRES_EXTENSION error: the extensions the database
/// requires that this build does not support, and the refusal message and
/// download link the app shows, rendered by the shared display-name module so
/// the app says exactly what the CLI and the tray say (ADR-083 §2, ADR-084
/// §1). Serialized camelCase: `{ unsupportedExtensions, message,
/// downloadLabel, downloadUrl }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequiresExtensionPayload {
    pub unsupported_extensions: Vec<String>,
    pub message: String,
    pub download_label: String,
    pub download_url: String,
}

impl RequiresExtensionPayload {
    fn for_unsupported(unsupported_extensions: Vec<String>) -> Self {
        use nodespace_proto::extension_names::{refusal_message, DOWNLOAD_LABEL, DOWNLOAD_URL};
        Self {
            message: refusal_message(&unsupported_extensions),
            unsupported_extensions,
            download_label: DOWNLOAD_LABEL.to_string(),
            download_url: DOWNLOAD_URL.to_string(),
        }
    }
}

/// The REQUIRES_EXTENSION error for a status that carries the daemon's
/// required-extensions refusal (FAILED_PRECONDITION with the
/// `x-requires-extension-bin` payload), or `None` for any other status. Every
/// command that maps a status from a routed request checks this first, so a
/// request to a refused database reaches the frontend as the same error
/// whichever command made it.
pub(crate) fn requires_extension_error(status: &tonic::Status) -> Option<CommandError> {
    let unsupported = nodespace_proto::requires_extension::unsupported_extensions(status)?;
    let payload = RequiresExtensionPayload::for_unsupported(unsupported);
    Some(CommandError {
        message: payload.message.clone(),
        code: "REQUIRES_EXTENSION".to_string(),
        details: Some(format!("{:?}", status.code())),
        conflict_data: None,
        requires_extension: Some(Box::new(payload)),
    })
}

/// The error text for a command that reports failures as a plain string: the
/// shared refusal message when `status` is the daemon's required-extensions
/// refusal, so the user reads the sentence every other surface shows, and the
/// status's own text otherwise.
pub(crate) fn status_message(status: tonic::Status) -> String {
    match nodespace_proto::requires_extension::unsupported_extensions(&status) {
        Some(unsupported) => nodespace_proto::extension_names::refusal_message(&unsupported),
        None => status.to_string(),
    }
}

/// `fallback(status)`, unless `status` is the daemon's required-extensions
/// refusal, which maps to REQUIRES_EXTENSION for every command alike. For a
/// command that maps its other statuses to codes of its own.
pub(crate) fn refusal_or(
    status: tonic::Status,
    fallback: impl FnOnce(tonic::Status) -> CommandError,
) -> CommandError {
    requires_extension_error(&status).unwrap_or_else(|| fallback(status))
}

pub(crate) fn status_to_command_error(status: tonic::Status) -> CommandError {
    if let Some(refused) = requires_extension_error(&status) {
        return refused;
    }
    // A cascade delete refused by the ADR-041 subtree access gate carries the
    // inaccessible-node count in `x-subtree-inaccessible-count` metadata, and a
    // Play-rule rejection (ADR-060 §2) carries its own structured payload in
    // the BINARY `x-play-rule-rejected-bin` metadata key (not a plain ASCII
    // one — the payload embeds an author-supplied rejection message, which
    // routinely contains non-ASCII bytes; gRPC's `-bin` metadata convention
    // transports it as raw bytes, base64-encoded on the wire, so it can never
    // be silently dropped the way an ASCII-only key would be). Both are only
    // meaningful gated on FailedPrecondition WITH the matching metadata
    // present — the daemon returns FailedPrecondition from unrelated paths
    // too (node-create/schema failures), which must not be mis-branded as
    // either of these.
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
    let play_rule_rejected_payload: Option<serde_json::Value> =
        if status.code() == tonic::Code::FailedPrecondition {
            status
                .metadata()
                .get_bin("x-play-rule-rejected-bin")
                .and_then(|v| v.to_bytes().ok())
                .and_then(|b| serde_json::from_slice(&b).ok())
        } else {
            None
        };
    // A tree-invariant refusal (a cycle, a collection given a parent, or a
    // member moved below a parent) carries `{ rule, node_id, related_ids,
    // detail }` in the binary `x-tree-invariant-violation-bin` key, under the
    // same FailedPrecondition gating as the two refusals above.
    let tree_invariant_payload: Option<serde_json::Value> =
        if status.code() == tonic::Code::FailedPrecondition {
            status
                .metadata()
                .get_bin("x-tree-invariant-violation-bin")
                .and_then(|v| v.to_bytes().ok())
                .and_then(|b| serde_json::from_slice(&b).ok())
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
        // Distinct from ordinary validation and from SUBTREE_ACCESS_DENIED so
        // the frontend can roll back the optimistic write and show the
        // rejecting rule's own message, not a generic write-failure toast.
        tonic::Code::FailedPrecondition if play_rule_rejected_payload.is_some() => {
            "PLAY_RULE_REJECTED"
        }
        // Distinct so the frontend can name the rule that fired and the nodes
        // involved instead of logging an opaque write failure.
        tonic::Code::FailedPrecondition if tree_invariant_payload.is_some() => {
            "TREE_INVARIANT_VIOLATION"
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
    } else if let Some(payload) = play_rule_rejected_payload {
        Some(payload)
    } else if let Some(payload) = tree_invariant_payload {
        Some(payload)
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
        requires_extension: None,
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
            requires_extension: None,
        })?;
    let modified_at = DateTime::parse_from_rfc3339(&nd.modified_at)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| CommandError {
            message: format!("Invalid modified_at timestamp: {}", e),
            code: "PARSE_ERROR".to_string(),
            details: Some(nd.modified_at.clone()),
            conflict_data: None,
            requires_extension: None,
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
        title: nd.title,
    })
}

/// Where a create or move placed its node's `has_child` edge: the store's
/// order key for that edge, plus the new key of every sibling a re-spread
/// rewrote. The frontend applies these to its structure tree when the call
/// resolves — its own relationship events are echo-suppressed, so the reply
/// is the only place it learns them.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildPlacementOutput {
    pub parent_id: String,
    pub order: f64,
    pub respread: Vec<SiblingOrderOutput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiblingOrderOutput {
    pub node_id: String,
    pub order: f64,
}

impl From<nodespace_proto::nodespace::ChildPlacement> for ChildPlacementOutput {
    fn from(p: nodespace_proto::nodespace::ChildPlacement) -> Self {
        Self {
            parent_id: p.parent_id,
            order: p.order,
            respread: p
                .respread
                .into_iter()
                .map(|s| SiblingOrderOutput {
                    node_id: s.node_id,
                    order: s.order,
                })
                .collect(),
        }
    }
}

/// Result of `create_node`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatedNodeOutput {
    pub id: String,
    pub placement: Option<ChildPlacementOutput>,
}

/// Result of `move_node`: the node with its bumped version, and its placement
/// (`None` for a move to root).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovedNodeOutput {
    pub node: Value,
    pub placement: Option<ChildPlacementOutput>,
}

/// Result of `move_children_to_parent`: the children with their bumped
/// versions, and the order key the store gave each one's new edge.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MovedChildrenOutput {
    pub nodes: Vec<Value>,
    pub orders: Vec<SiblingOrderOutput>,
}

/// Convert proto NodeResponse → core Node
fn proto_node_response_to_node(resp: NodeResponse) -> Result<Node, CommandError> {
    let nd = resp.node_data.ok_or_else(|| CommandError {
        message: "gRPC response missing node_data".to_string(),
        code: "GRPC_ERROR".to_string(),
        details: None,
        conflict_data: None,
        requires_extension: None,
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
                requires_extension: None,
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
        requires_extension: None,
    })
}

/// Convert a list of Nodes to their strongly-typed JSON representations
pub fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<Value>, CommandError> {
    types_nodes_to_typed_values(nodes).map_err(|e| CommandError {
        message: e.clone(),
        code: "CONVERSION_ERROR".to_string(),
        details: Some(e),
        conflict_data: None,
        requires_extension: None,
    })
}

/// Input for creating a root node (top-level container)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRootNodeInput {
    pub content: String,
    pub node_type: String,
    pub properties: serde_json::Value,
    #[serde(default)]
    pub mentioned_by: Option<String>,
}

/// Create a new node of any type with a registered schema
#[tauri::command]
pub async fn create_node(
    client: State<'_, GrpcClient>,
    node: CreateNodeInput,
) -> Result<CreatedNodeOutput, CommandError> {
    let mut c = client.echo_suppressed_client().await;
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
        .map_err(status_to_command_error)?
        .into_inner();

    Ok(CreatedNodeOutput {
        id: resp.node_id,
        placement: resp.placement.map(Into::into),
    })
}

/// Create a new root node (top-level node that can contain other nodes)
#[tauri::command]
pub async fn create_root_node(
    client: State<'_, GrpcClient>,
    input: CreateRootNodeInput,
) -> Result<String, CommandError> {
    let mut c = client.echo_suppressed_client().await;
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
    let mut c = client.echo_suppressed_client().await;
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
/// This runs one real, lightweight RPC bounded by a short timeout
/// ([`GrpcClient::data_plane_round_trip`]). A `NotFound` (the sentinel id never
/// exists) — or any other completion — proves the channel is live and returns
/// `false`. A timeout means the channel is wedged: rebuild it with
/// [`GrpcClient::reconnect`] and re-probe once, returning `true`
/// only if the rebuilt channel answers (the frontend then re-fires its
/// reconnect listeners so panes re-fetch on the fresh channel).
///
/// After a rebuild, the channel-rebuilt hooks of the app's extensions run before
/// the re-probe, so anything an extension cached on the old channel is replaced
/// before this returns (see [`AppExtensions::on_channel_rebuilt`]).
///
/// [`AppExtensions::on_channel_rebuilt`]: crate::extensions::AppExtensions::on_channel_rebuilt
#[tauri::command]
pub async fn probe_and_recover_channel(
    app: AppHandle,
    client: State<'_, GrpcClient>,
) -> Result<bool, ()> {
    Ok(probe_and_recover(&app, client.inner()).await)
}

/// The recovery [`probe_and_recover_channel`] runs, on any Tauri runtime so the
/// seam tests can drive it on a mock app.
pub async fn probe_and_recover<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    client: &GrpcClient,
) -> bool {
    use crate::services::{DataPlaneRoundTrip, DATA_PLANE_PROBE_TIMEOUT};

    // A completed RPC (any status, including NotFound) means the channel is
    // alive; only a timeout indicates a wedge.
    if !matches!(
        client.data_plane_round_trip(DATA_PLANE_PROBE_TIMEOUT).await,
        DataPlaneRoundTrip::TimedOut
    ) {
        return false;
    }

    tracing::warn!("gRPC channel probe timed out — rebuilding wedged channel");
    client.reconnect().await;
    crate::extensions::run_channel_rebuilt_hooks(app, client.channel().await).await;
    !matches!(
        client.data_plane_round_trip(DATA_PLANE_PROBE_TIMEOUT).await,
        DataPlaneRoundTrip::TimedOut
    )
}

/// Update an existing node
#[tauri::command]
pub async fn update_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: NodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;

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
        typed_client: true,
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
    let mut c = client.echo_suppressed_client().await;
    let resp = c
        .delete_node(Request::new(DeleteNodeRequest {
            node_id: id,
            version: Some(version),
            ..Default::default()
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
) -> Result<MovedNodeOutput, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let position = insert_position.and_then(InsertPositionInput::into_move_proto_position);
    let mut resp = c
        .move_node(Request::new(MoveNodeRequest {
            node_id,
            version,
            new_parent_id,
            position,
        }))
        .await
        .map_err(status_to_command_error)?
        .into_inner();

    let placement = resp.placement.take().map(Into::into);
    let node = proto_node_response_to_node(resp)?;
    Ok(MovedNodeOutput {
        node: node_to_typed_value(node)?,
        placement,
    })
}

/// Reorder a node by changing its sibling position
#[tauri::command]
pub async fn reorder_node(
    client: State<'_, GrpcClient>,
    node_id: String,
    version: i64,
    insert_position: Option<InsertPositionInput>,
) -> Result<(), CommandError> {
    let mut c = client.echo_suppressed_client().await;
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
) -> Result<MovedChildrenOutput, CommandError> {
    let mut c = client.echo_suppressed_client().await;
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
        .map_err(status_to_command_error)?
        .into_inner();

    let nodes: Result<Vec<Node>, CommandError> = resp
        .children
        .into_iter()
        .map(proto_node_data_to_node)
        .collect();

    Ok(MovedChildrenOutput {
        nodes: nodes_to_typed_values(nodes?)?,
        orders: resp
            .orders
            .into_iter()
            .map(|s| SiblingOrderOutput {
                node_id: s.node_id,
                order: s.order,
            })
            .collect(),
    })
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
        requires_extension: None,
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
            include_archived: false,
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

/// Arguments for [`execute_query`], mirroring proto `ExecuteQueryRequest`.
///
/// `filters_json` / `sorting_json` stay JSON strings the whole way down: the
/// shape is defined once by serde in `query_ops` (`AgentFilterItem` /
/// `AgentSortItem`), and a filter's free-form `value` has no natural proto
/// representation. Re-modeling it here would be a third copy to keep in step.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteQueryArgs {
    pub target_type: String,
    pub filters_json: Option<String>,
    pub sorting_json: Option<String>,
    /// 0 = unset; the daemon applies its own default and clamp.
    #[serde(default)]
    pub limit: u32,
}

/// Execute a structured query through the backend's `QueryService`.
///
/// Unlike [`query_nodes_simple`], which scopes by type/text and cannot order,
/// this carries property filters with comparison operators and the caller's
/// sort configuration. Saved queries run here so their semantics have one
/// implementation rather than a re-derived copy in the frontend.
#[tauri::command]
pub async fn execute_query(
    client: State<'_, GrpcClient>,
    request: ExecuteQueryArgs,
) -> Result<Vec<Value>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .execute_query(Request::new(ExecuteQueryRequest {
            target_type: request.target_type,
            filters_json: request.filters_json,
            sorting_json: request.sorting_json,
            limit: request.limit,
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

/// Count the nodes a structured query matches, without transferring them.
///
/// The counting counterpart to [`execute_query`], backing the query editor's
/// preview. Takes the same [`ExecuteQueryArgs`] so a caller can count exactly
/// what it would execute; the daemon ignores `sorting_json`/`limit`, neither of
/// which can change a total.
#[tauri::command]
pub async fn count_query(
    client: State<'_, GrpcClient>,
    request: ExecuteQueryArgs,
) -> Result<i64, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .count_query(Request::new(ExecuteQueryRequest {
            target_type: request.target_type,
            filters_json: request.filters_json,
            sorting_json: request.sorting_json,
            limit: request.limit,
        }))
        .await
        .map_err(status_to_command_error)?;

    Ok(resp.into_inner().count)
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

/// The related-node fields `get_parent` reads from a `GetRelatedNodes` payload,
/// whose entries are full nodes in the camelCase wire shape.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelatedNodePayload {
    id: String,
    node_type: String,
    title: Option<String>,
}

/// Get a node's parent: the one node it is a `has_child` child of, or `None` for a root.
#[tauri::command]
pub async fn get_parent(
    client: State<'_, GrpcClient>,
    node_id: String,
) -> Result<Option<NodeReference>, CommandError> {
    let mut c = client.client().await;
    let resp = c
        .get_related_nodes(Request::new(GetRelatedNodesRequest {
            node_id,
            relationship_name: "has_child".to_string(),
            direction: "in".to_string(),
        }))
        .await
        .map_err(status_to_command_error)?;

    let json = resp.into_inner().related_nodes_json;
    parse_parent(&json).map_err(|e| CommandError {
        message: format!("Failed to parse parent JSON: {}", e),
        code: "PARSE_ERROR".to_string(),
        details: Some(json),
        conflict_data: None,
        requires_extension: None,
    })
}

/// The first related node of a `GetRelatedNodes` payload, as a reference.
fn parse_parent(related_nodes_json: &str) -> Result<Option<NodeReference>, serde_json::Error> {
    let related: Vec<RelatedNodePayload> = serde_json::from_str(related_nodes_json)?;
    Ok(related.into_iter().next().map(|r| NodeReference {
        id: r.id,
        title: r.title,
        node_type: r.node_type,
    }))
}

/// Encode a tri-state update field for the proto's clearable wrapper:
/// `None` → unset (no change), `Some(None)` → clear, `Some(Some(v))` → set.
fn string_clear(value: Option<Option<String>>) -> Option<OptionalStringClear> {
    value.map(|opt| OptionalStringClear {
        clear: opt.is_none(),
        value: opt.unwrap_or_default(),
    })
}

/// [`string_clear`] for a `priority` field on the shared scale.
fn priority_clear(value: Option<Option<Priority>>) -> Option<OptionalStringClear> {
    string_clear(value.map(|opt| opt.map(|priority| priority.as_str().to_string())))
}

/// Date variant of [`string_clear`].
fn timestamp_clear(value: Option<Option<String>>) -> Option<OptionalTimestampClear> {
    value.map(|opt| OptionalTimestampClear {
        clear: opt.is_none(),
        value: opt.unwrap_or_default(),
    })
}

/// [`string_clear`] for a structured field (a link, a list of links), which
/// travels JSON encoded.
fn json_clear<T: Serialize>(
    value: Option<Option<T>>,
    field: &str,
) -> Result<Option<OptionalJsonClear>, CommandError> {
    value
        .map(|opt| match opt {
            None => Ok(OptionalJsonClear {
                clear: true,
                value_json: String::new(),
            }),
            Some(value) => Ok(OptionalJsonClear {
                clear: false,
                value_json: typed_update_json(&value, field)?,
            }),
        })
        .transpose()
}

/// Encode a typed update for a request that carries it as JSON; the daemon
/// decodes the same struct.
fn typed_update_json<T: Serialize>(update: &T, node_type: &str) -> Result<String, CommandError> {
    serde_json::to_string(update).map_err(|e| CommandError {
        message: format!("Failed to serialize {node_type} update: {e}"),
        code: "SERIALIZE_ERROR".to_string(),
        details: None,
        conflict_data: None,
        requires_extension: None,
    })
}

/// Update a task node's core fields (status, priority, due/started/completed
/// dates, pull request, commits).
#[tauri::command]
pub async fn update_task_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: TaskNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdateTaskNodeRequest {
        node_id: id,
        version,
        status: update.status.map(|status| status.as_str().to_string()),
        priority: priority_clear(update.priority),
        due_date: timestamp_clear(update.due_date),
        started_at: timestamp_clear(update.started_at),
        completed_at: timestamp_clear(update.completed_at),
        pull_request: json_clear(update.pull_request, "pull_request")?,
        commits: json_clear(update.commits, "commits")?,
    };
    let resp = c
        .update_task_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update a person node's core fields (first name, last name, email).
#[tauri::command]
pub async fn update_person_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: PersonNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdatePersonNodeRequest {
        node_id: id,
        version,
        first_name: string_clear(update.first_name),
        last_name: string_clear(update.last_name),
        email: string_clear(update.email),
    };
    let resp = c
        .update_person_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update a project node's core fields (status, priority, start/end dates,
/// repository).
#[tauri::command]
pub async fn update_project_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: ProjectNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdateProjectNodeRequest {
        node_id: id,
        version,
        status: update.status.map(|status| status.as_str().to_string()),
        priority: priority_clear(update.priority),
        start_date: timestamp_clear(update.start_date),
        end_date: timestamp_clear(update.end_date),
        repository: json_clear(update.repository, "repository")?,
        checkout_path: string_clear(update.checkout_path),
    };
    let resp = c
        .update_project_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update a spec's core fields (objective, boundaries, spec status).
#[tauri::command]
pub async fn update_spec_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: SpecNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdateSpecNodeRequest {
        node_id: id,
        version,
        update_json: typed_update_json(&update, "spec")?,
    };
    let resp = c
        .update_spec_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update a plan's core fields (approach, risks, plan status).
#[tauri::command]
pub async fn update_plan_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: PlanNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdatePlanNodeRequest {
        node_id: id,
        version,
        update_json: typed_update_json(&update, "plan")?,
    };
    let resp = c
        .update_plan_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update a decision's core field (decision status).
#[tauri::command]
pub async fn update_decision_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: DecisionNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdateDecisionNodeRequest {
        node_id: id,
        version,
        update_json: typed_update_json(&update, "decision")?,
    };
    let resp = c
        .update_decision_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update a saved query's fields (definition, generated_by, generator
/// context, view config).
#[tauri::command]
pub async fn update_query_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: QueryNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdateQueryNodeRequest {
        node_id: id,
        version,
        update_json: typed_update_json(&update, "query")?,
    };
    let resp = c
        .update_query_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update a play's fields (rules, description).
#[tauri::command]
pub async fn update_play_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: PlayNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdatePlayNodeRequest {
        node_id: id,
        version,
        update_json: typed_update_json(&update, "play")?,
    };
    let resp = c
        .update_play_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update a collection's core field (description).
#[tauri::command]
pub async fn update_collection_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: CollectionNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdateCollectionNodeRequest {
        node_id: id,
        version,
        update_json: typed_update_json(&update, "collection")?,
    };
    let resp = c
        .update_collection_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update a skill's core fields (use_for, not_for, tool whitelist, max
/// iterations, node types).
#[tauri::command]
pub async fn update_skill_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: SkillNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdateSkillNodeRequest {
        node_id: id,
        version,
        update_json: typed_update_json(&update, "skill")?,
    };
    let resp = c
        .update_skill_node(Request::new(req))
        .await
        .map_err(status_to_command_error)?;

    let node = proto_node_response_to_node(resp.into_inner())?;
    node_to_typed_value(node)
}

/// Update the database-settings node's core field (required extensions).
#[tauri::command]
pub async fn update_database_settings_node(
    client: State<'_, GrpcClient>,
    id: String,
    version: i64,
    update: DatabaseSettingsNodeUpdate,
) -> Result<Value, CommandError> {
    let mut c = client.echo_suppressed_client().await;
    let req = UpdateDatabaseSettingsNodeRequest {
        node_id: id,
        version,
        update_json: typed_update_json(&update, "database-settings")?,
    };
    let resp = c
        .update_database_settings_node(Request::new(req))
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
        requires_extension: None,
    })
}

/// Create a schema-declared typed relationship edge between two nodes.
///
/// Wraps `rel_ops::create_relationship`: the daemon validates the relationship
/// against the source node's schema (target type, edge fields) before writing.
/// A `cardinality: One` source or `reverse_cardinality: One` target does not
/// reject a second edge — the prior edge is replaced — and the evicted edges
/// come back in `replaced`. The viewer confirms a reassignment BEFORE calling
/// this (it knows the far end's cardinality); `replaced` reports what actually
/// happened. `edge_data` carries the edge's `edge_fields` values as a JSON
/// object; omit or pass `null` for a bare edge. The frontend reloads via
/// `get_node_relationships` to see the new edge in context.
///
/// Typed-edge commands use the untagged client: the frontend store does not
/// apply them, so their `relationship:*` events must reach this window.
#[tauri::command]
pub async fn create_relationship(
    client: State<'_, GrpcClient>,
    source_id: String,
    relationship_name: String,
    target_id: String,
    edge_data: Option<Value>,
) -> Result<Value, CommandError> {
    let edge_data_json = match edge_data {
        Some(v) if !v.is_null() => Some(serde_json::to_string(&v).map_err(|e| CommandError {
            message: format!("Failed to serialize edge_data: {}", e),
            code: "SERIALIZE_ERROR".to_string(),
            details: None,
            conflict_data: None,
            requires_extension: None,
        })?),
        _ => None,
    };
    let mut c = client.client().await;
    let response = c
        .create_relationship(Request::new(CreateRelationshipRequest {
            source_id,
            relationship_name,
            target_id,
            edge_data_json,
        }))
        .await
        .map_err(status_to_command_error)?
        .into_inner();
    let replaced: Vec<Value> = response
        .replaced
        .into_iter()
        .map(|edge| {
            serde_json::json!({
                "sourceId": edge.source_id,
                "relationshipName": edge.relationship_name,
                "targetId": edge.target_id,
            })
        })
        .collect();
    Ok(serde_json::json!({ "replaced": replaced }))
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
        requires_extension: None,
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
    let mut c = client.echo_suppressed_client().await;
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

    /// The refusal the daemon returns for a request routed to a database
    /// that requires `ids`.
    fn refusal(ids: &[&str]) -> tonic::Status {
        let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
        nodespace_proto::requires_extension::status(&ids)
    }

    /// The contract the frontend's refusal view codes against: code
    /// REQUIRES_EXTENSION, the module's message, and a camelCase
    /// `requiresExtension` payload with the ids, the message and the download
    /// link.
    #[test]
    fn a_refusal_maps_to_requires_extension_with_its_payload() {
        use nodespace_proto::extension_names::{refusal_message, DOWNLOAD_LABEL, DOWNLOAD_URL};

        let err = status_to_command_error(refusal(&["pro"]));

        assert_eq!(err.code, "REQUIRES_EXTENSION");
        assert_eq!(err.message, refusal_message(&["pro"]));
        assert!(err.conflict_data.is_none());
        assert_eq!(
            serde_json::to_value(&err).unwrap(),
            serde_json::json!({
                "message": refusal_message(&["pro"]),
                "code": "REQUIRES_EXTENSION",
                "details": "FailedPrecondition",
                "requiresExtension": {
                    "unsupportedExtensions": ["pro"],
                    "message": refusal_message(&["pro"]),
                    "downloadLabel": DOWNLOAD_LABEL,
                    "downloadUrl": DOWNLOAD_URL,
                },
            })
        );
    }

    /// Another FAILED_PRECONDITION is not the refusal, and an error that is
    /// not a refusal carries no `requiresExtension` key at all.
    #[test]
    fn another_failed_precondition_is_not_a_refusal() {
        let err = status_to_command_error(tonic::Status::failed_precondition("no"));
        assert_ne!(err.code, "REQUIRES_EXTENSION");
        assert!(err.requires_extension.is_none());
        let json = serde_json::to_value(&err).unwrap();
        assert!(json.get("requiresExtension").is_none(), "{json}");
    }

    /// A command that reports failures as plain strings shows the refusal as
    /// the shared message, not the status's debug text, and any other status
    /// as before.
    #[test]
    fn status_message_renders_the_refusal_as_the_shared_message() {
        assert_eq!(
            status_message(refusal(&["pro"])),
            nodespace_proto::extension_names::refusal_message(&["pro"])
        );
        let other = tonic::Status::internal("boom");
        assert_eq!(status_message(other.clone()), other.to_string());
    }

    /// A command that maps statuses to codes of its own still reports the
    /// refusal as REQUIRES_EXTENSION, and keeps its own mapping for any other
    /// status.
    #[test]
    fn refusal_or_takes_the_refusal_before_the_commands_own_mapping() {
        let own = |s: tonic::Status| CommandError {
            message: s.message().to_string(),
            code: "OWN_CODE".to_string(),
            details: None,
            conflict_data: None,
            requires_extension: None,
        };
        assert_eq!(
            refusal_or(refusal(&["fixture"]), own).code,
            "REQUIRES_EXTENSION"
        );
        assert_eq!(
            refusal_or(tonic::Status::internal("boom"), own).code,
            "OWN_CODE"
        );
    }

    #[test]
    fn test_command_error_serialization() {
        let err = CommandError {
            message: "Test error".to_string(),
            code: "TEST_ERROR".to_string(),
            details: Some("Debug info".to_string()),
            conflict_data: None,
            requires_extension: None,
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
            requires_extension: None,
        };

        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("Simple error"));
        // Details field should be omitted when None
        assert!(!json.contains("details"));
    }

    #[test]
    fn status_to_command_error_maps_play_rule_rejected() {
        // Mirrors the daemon's `OpsError::PlayRuleRejected` mapping
        // (FAILED_PRECONDITION + BINARY x-play-rule-rejected-bin metadata,
        // ADR-060 §2): the Tauri layer must brand it distinctly from
        // SUBTREE_ACCESS_DENIED (which shares the same status code but a
        // different metadata key) and from an ordinary FailedPrecondition
        // with neither key present. Binary, not ASCII — the payload embeds
        // an author-supplied rejection message, which can contain non-ASCII
        // bytes an ASCII metadata value would silently drop.
        let mut status = tonic::Status::failed_precondition("Play rule 'r' rejected the write");
        let payload = serde_json::json!({
            "node_id": "n1",
            "play_id": "p1",
            "rule_name": "r1",
            "message": "cannot close while children are open",
        });
        let json = serde_json::to_string(&payload).unwrap();
        let val =
            tonic::metadata::MetadataValue::<tonic::metadata::Binary>::from_bytes(json.as_bytes());
        status
            .metadata_mut()
            .insert_bin("x-play-rule-rejected-bin", val);

        let err = status_to_command_error(status);
        assert_eq!(err.code, "PLAY_RULE_REJECTED");
        let conflict_data = err.conflict_data.expect("conflict_data must be present");
        assert_eq!(conflict_data["rule_name"], "r1");
        assert_eq!(
            conflict_data["message"],
            "cannot close while children are open"
        );
    }

    #[test]
    fn status_to_command_error_preserves_non_ascii_play_rule_rejected_message() {
        // Regression guard: the old ASCII-only metadata key silently
        // dropped conflict_data for any non-ASCII byte in the rejection
        // message. The binary key must carry it through intact.
        let message = "cannot close — \u{201c}Renew\u{201d} needs café review \u{1F6AB}";
        let mut status = tonic::Status::failed_precondition("rejected");
        let payload = serde_json::json!({
            "node_id": "n1",
            "play_id": "p1",
            "rule_name": "r1",
            "message": message,
        });
        let json = serde_json::to_string(&payload).unwrap();
        let val =
            tonic::metadata::MetadataValue::<tonic::metadata::Binary>::from_bytes(json.as_bytes());
        status
            .metadata_mut()
            .insert_bin("x-play-rule-rejected-bin", val);

        let err = status_to_command_error(status);
        assert_eq!(err.code, "PLAY_RULE_REJECTED");
        let conflict_data = err.conflict_data.expect("conflict_data must be present");
        assert_eq!(conflict_data["message"], message);
    }

    #[test]
    fn status_to_command_error_maps_tree_invariant_violation() {
        // Mirrors the daemon's `OpsError::TreeInvariantViolation` mapping
        // (FAILED_PRECONDITION + BINARY x-tree-invariant-violation-bin): the
        // rule and nodes involved reach the frontend as structured data.
        let mut status =
            tonic::Status::failed_precondition("member_of_not_root: node 'n1' holds membership");
        let payload = serde_json::json!({
            "rule": "member_of_not_root",
            "node_id": "n1",
            "related_ids": ["c1"],
            "detail": "node 'n1' holds collection membership — remove it first",
        });
        let json = serde_json::to_string(&payload).unwrap();
        let val =
            tonic::metadata::MetadataValue::<tonic::metadata::Binary>::from_bytes(json.as_bytes());
        status
            .metadata_mut()
            .insert_bin("x-tree-invariant-violation-bin", val);

        let err = status_to_command_error(status);
        assert_eq!(err.code, "TREE_INVARIANT_VIOLATION");
        let conflict_data = err.conflict_data.expect("conflict_data must be present");
        assert_eq!(conflict_data["rule"], "member_of_not_root");
        assert_eq!(conflict_data["node_id"], "n1");
        assert_eq!(conflict_data["related_ids"], serde_json::json!(["c1"]));
    }

    #[test]
    fn status_to_command_error_plain_failed_precondition_is_generic() {
        // No x-play-rule-rejected or x-subtree-inaccessible-count metadata
        // present — must not be mis-branded as either specific refusal.
        let status = tonic::Status::failed_precondition("some unrelated failure");
        let err = status_to_command_error(status);
        assert_eq!(err.code, "GRPC_ERROR");
        assert!(err.conflict_data.is_none());
    }

    fn sample_node_data(title: Option<String>) -> NodeData {
        NodeData {
            id: "n1".to_string(),
            node_type: "person".to_string(),
            content: String::new(),
            properties: "{}".to_string(),
            version: 1,
            lifecycle_status: "active".to_string(),
            created_at: "2026-09-13T00:00:00Z".to_string(),
            modified_at: "2026-09-13T00:00:00Z".to_string(),
            markdown: String::new(),
            title,
        }
    }

    /// This conversion used to hardcode `title: None` regardless of what the
    /// daemon sent, silently dropping the title a second time even after the
    /// proto/wire fix — every Tauri command that returns a node goes through
    /// this function.
    #[test]
    fn proto_node_data_to_node_carries_a_present_title_through() {
        let node = proto_node_data_to_node(sample_node_data(Some("Michael Libio".to_string())))
            .expect("valid timestamps must convert");

        assert_eq!(node.title.as_deref(), Some("Michael Libio"));
    }

    /// Absence must stay absence, not turn into `Some("")` or similar.
    #[test]
    fn proto_node_data_to_node_carries_an_absent_title_through() {
        let node =
            proto_node_data_to_node(sample_node_data(None)).expect("valid timestamps must convert");

        assert_eq!(node.title, None);
    }

    /// The channel-rebuilt hooks must run on the rebuilt channel and finish
    /// before the re-probe whose result tells the frontend whether recovery
    /// worked.
    #[test]
    fn probe_and_recover_runs_the_channel_rebuilt_hooks_between_the_rebuild_and_the_reprobe() {
        let source = include_str!("nodes.rs");
        let start = source
            .find("pub async fn probe_and_recover<")
            .expect("probe_and_recover not found in nodes.rs");
        let end = source[start..]
            .find("#[tauri::command]")
            .map(|offset| start + offset)
            .expect("the command after probe_and_recover not found in nodes.rs");
        let body = &source[start..end];

        let rebuilt = body
            .find("client.reconnect().await")
            .expect("probe_and_recover must rebuild the channel");
        let hooks = body
            .find("run_channel_rebuilt_hooks(")
            .expect("probe_and_recover must run the channel-rebuilt hooks");
        let reprobe = body
            .rfind("data_plane_round_trip(")
            .expect("probe_and_recover must re-probe the rebuilt channel");

        assert!(
            rebuilt < hooks,
            "the hooks run after the channel is rebuilt, so they get the new one"
        );
        assert!(
            hooks < reprobe,
            "the hooks run before the re-probe, so recovery reports healthy only after them"
        );
    }

    #[test]
    fn parse_parent_reads_the_single_parent_and_none_for_a_root() {
        let parent = parse_parent(
            r#"[{"id":"chat-1","nodeType":"ai-chat-native","title":null,"content":"","version":1}]"#,
        )
        .unwrap();
        assert_eq!(
            parent,
            Some(NodeReference {
                id: "chat-1".to_string(),
                title: None,
                node_type: "ai-chat-native".to_string(),
            })
        );
        assert_eq!(parse_parent("[]").unwrap(), None);
        assert!(parse_parent("not json").is_err());
    }
}
