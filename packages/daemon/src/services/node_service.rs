//! tonic `NodeService` implementation backed by `nodespace-core`.
//!
//! Each RPC handler:
//!   1. Parses the proto request into the corresponding core input type.
//!   2. Calls the matching `nodespace_core::services` method (or `ops` function).
//!   3. Converts the result back into proto messages.
//!   4. Maps `NodeServiceError`/`OpsError` → `tonic::Status`.
//!
//! `Chat` returns `Unimplemented` — covered by a separate streaming issue.

use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use nodespace_core::db::events::DomainEvent;
use nodespace_core::models::{
    Node, NodeQuery, NodeUpdate, OrderBy, TaskNodeUpdate, TaskPriority, TaskStatus,
};
use nodespace_core::ops::{
    collection_ops::{
        self, AddNodeToCollectionByPathInput, AddNodeToCollectionInput, CreateCollectionInput,
        FindCollectionByPathInput, GetAllCollectionsInput, GetCollectionByNameInput,
        GetCollectionMembersInput, GetCollectionMembersRecursiveInput, GetNodeCollectionsInput,
        RemoveNodeFromCollectionInput, RenameCollectionInput,
    },
    node_ops, query_ops, rel_ops,
    search_ops::{self, SearchSemanticInput},
    OpsError,
};
use nodespace_core::services::{
    EmbeddingScheduler, InsertPosition, InsertPositionOwned, NodeAccessor,
    NodeService as CoreNodeService, NodeServiceError,
};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;

use crate::services::embeddings_service::EmbeddingReady;
use tonic::{Request, Response, Status};

use crate::nodespace::{
    node_event::Event as NodeEventKind, node_service_server::NodeService as GrpcNodeService,
    AddNodeToCollectionByPathRequest, AddNodeToCollectionRequest, BatchUpdateFailure, ChatRequest,
    ChatResponse, CollectionIdResponse, CollectionIdsResponse, CollectionInfo,
    CollectionListResponse, CollectionMembersRequest, ConflictListResponse,
    ConflictRecord as ConflictRecordProto, ConflictResponse, ConflictsForNodeRequest,
    CountNodesResponse, CreateCollectionRequest, CreateMentionRequest, CreateNodeRequest,
    CreateRelationshipRequest, CreateRelationshipResponse, DeleteCollectionRequest,
    DeleteMentionRequest, DeleteNodeRequest, DeleteNodeResponse, DeleteRelationshipRequest,
    DeleteRelationshipResponse, Empty, ExecuteQueryRequest, ExportMarkdownRequest,
    ExportMarkdownResponse, FindCollectionByPathRequest, FindDuplicateRequest,
    GetAllCollectionsRequest, GetAllSchemasRequest, GetChildrenRequest, GetChildrenTreeRequest,
    GetCollectionByNameRequest, GetConflictRequest, GetDaemonVersionRequest,
    GetDaemonVersionResponse, GetNodeRelationshipsRequest, GetNodeRelationshipsResponse,
    GetNodeRequest, GetNodesBatchRequest, GetNodesBatchResponse, GetRelatedNodesRequest,
    GetRelatedNodesResponse, GetRootsRequest, GetSchemaDefinitionRequest, ListConflictsRequest,
    MentionAutocompleteRequest, MentionIdsResponse, MentionResponse, MentionTargetRequest,
    MergeNodesRequest, MergeNodesResponse, MoveChildrenToParentRequest,
    MoveChildrenToParentResponse, MoveNodeRequest, NodeCollectionsRequest, NodeData, NodeDeleted,
    NodeEvent, NodeListResponse, NodeReference, NodeReferenceListResponse, NodeResponse,
    NodeSortOrder, NodeTreeResponse, OptionalConflictResponse, OptionalNodeResponse,
    OptionalStringClear, OptionalTimestampClear, QueryNodesSimpleRequest,
    RelationshipDeletedPayload, RelationshipPayload, RemoveNodeFromCollectionRequest,
    RenameCollectionRequest, ReorderNodeRequest, ReorderNodeResponse, ResetSeedNodeRequest,
    ResetSeedNodeResponse, ResolveConflictRequest, SchemaParamsRequest, SchemaResultResponse,
    SearchRequest, SetLocalPersonIdentityRequest, UpdateNodeRequest, UpdateNodesBatchRequest,
    UpdateNodesBatchResponse, UpdateRelationshipPropertiesRequest,
    UpdateRelationshipPropertiesResponse, UpdateTaskNodeRequest, UpsertNodeWithParentRequest,
    WatchRequest,
};

/// gRPC adapter that owns shared handles to the core services.
///
/// `embedding_state` is `None` while the model is loading or when the NLP
/// engine is absent. Semantic search returns `UNAVAILABLE` in both cases.
///
/// **Adding a new unary RPC handler**: call `self.route(&request).await?`
/// first, exactly like every existing handler — this both resolves ADR-053
/// database routing and scopes the returned `node_service` for same-origin
/// write tagging (ADR-026 C5 extension) via the `x-ns-client-id` header.
/// **Adding a new streaming (subscribe-style) handler**: if it should also
/// suppress the subscriber's own writes, read `x-ns-client-id` via the
/// standalone `client_id_header()` helper *before* calling `route()` — see
/// `watch_nodes` for the reference implementation. Do not rely on `route()`
/// alone for a subscribing handler: `route()`'s use of the header scopes
/// *writes*, which a read-only streaming handler never performs.
#[derive(Clone)]
pub struct NodeServiceImpl {
    node_service: Arc<CoreNodeService>,
    /// Shared with EmbeddingsServiceImpl; populated by the background load task.
    embedding_state: Arc<RwLock<Option<EmbeddingReady>>>,
    /// Registry id of the database this impl serves (ADR-053), stamped onto
    /// `WatchNodes` events. Empty when the daemon serves a single unregistered
    /// database (Pro daemon) or when the impl is constructed directly in tests.
    database_id: String,
    /// Process-global embedding scheduler (ADR-053). A live `WatchNodes` stream
    /// marks this database active so its embedding batches take priority.
    scheduler: Arc<EmbeddingScheduler>,
    /// Cancelled when this database's service set is torn down — idle
    /// eviction, deliberate close, or daemon shutdown (see
    /// `DatabaseServices::shutdown`). A live `WatchNodes` stream selects on
    /// this alongside its event receiver so teardown actually ends the
    /// stream instead of leaving it a zombie: the stream's own cloned
    /// `node_service` owns the event sender its receiver reads from, so
    /// absent this signal `rx.recv()` can never observe the channel as
    /// closed on its own once the database that opened it is gone. Defaults
    /// to a token nobody ever cancels, so a caller that doesn't wire one up
    /// (tests, a directly-constructed instance) is unaffected.
    shutdown_token: tokio_util::sync::CancellationToken,
}

impl NodeServiceImpl {
    pub fn new(
        node_service: Arc<CoreNodeService>,
        embedding_state: Arc<RwLock<Option<EmbeddingReady>>>,
        scheduler: Arc<EmbeddingScheduler>,
    ) -> Self {
        Self {
            node_service,
            embedding_state,
            database_id: String::new(),
            scheduler,
            shutdown_token: tokio_util::sync::CancellationToken::new(),
        }
    }

    /// Tag this database's `WatchNodes` events with its registry id (ADR-053).
    /// Set by [`crate::build_database_services`] when a database is opened
    /// through the registry; left empty for the single-database Pro daemon.
    pub fn with_database_id(mut self, database_id: String) -> Self {
        self.database_id = database_id;
        self
    }

    /// Wire this database's teardown signal (see the `shutdown_token` field's
    /// doc comment). Set by [`crate::build_database_services`] so every
    /// `WatchNodes` stream this impl opens ends when the database is
    /// evicted/closed, instead of surviving as a zombie.
    pub fn with_shutdown_token(mut self, token: tokio_util::sync::CancellationToken) -> Self {
        self.shutdown_token = token;
        self
    }

    /// The underlying `NodeService` this impl serves. Lets a caller that opens a
    /// database through the registry (e.g. the Pro daemon binding cloud-sync to
    /// the manager's default database) reuse the *same* `NodeService` the gRPC
    /// handlers serve, rather than constructing a second one on the same file
    /// (which would contend on the single-writer lock).
    pub fn node_service(&self) -> Arc<CoreNodeService> {
        self.node_service.clone()
    }

    /// Resolve which database a request targets (ADR-053) and return that
    /// database's node service. The routing contract lives in
    /// [`crate::db_routing::routed_database_services`]: a header selects a
    /// registered database, header-less requests hit the default, and with no
    /// routing middleware installed a header-less request falls back to `self`
    /// while a header-carrying one is rejected rather than silently served from
    /// the active database.
    ///
    /// Also applies same-origin write tagging (ADR-026 C5 extension): a
    /// request carrying `x-ns-client-id` gets its `node_service` scoped via
    /// `with_client(id)`, so any event this request's write emits is stamped
    /// with that id as `source_client_id` — the signal `watch_nodes` uses to
    /// drop the writer's own echo. Applied here, once, so every RPC handler
    /// (which all start with `let this = self.route(&request).await?`) picks
    /// it up without individually touching the client-id header.
    async fn route<T>(&self, request: &Request<T>) -> Result<NodeServiceImpl, Status> {
        let mut this = match crate::db_routing::routed_database_services(request).await? {
            Some(services) => services.node_service_grpc.clone(),
            None => self.clone(),
        };
        if let Some(client_id) = client_id_header(request).map_err(|e| *e)? {
            this.node_service = Arc::new(this.node_service.with_client(client_id));
        }
        Ok(this)
    }
}

/// Read the `x-ns-client-id` header off a request, if present. Absent →
/// `Ok(None)`, meaning the caller's writes emit no `source_client_id` and are
/// never suppressed on any `WatchNodes` stream — behavior identical to before
/// this header existed. `Status` is boxed on the error path so this small
/// `Option<String>` success type doesn't blow up `Result`'s stack size next
/// to it (`clippy::result_large_err`).
fn client_id_header<T>(request: &Request<T>) -> Result<Option<String>, Box<Status>> {
    request
        .metadata()
        .get(nodespace_proto::CLIENT_ID_HEADER)
        .map(|v| {
            v.to_str().map(str::to_owned).map_err(|_| {
                Box::new(Status::invalid_argument(
                    "x-ns-client-id must be valid ASCII",
                ))
            })
        })
        .transpose()
}

/// RAII release of a `WatchNodes` stream's scheduler active-database claim
/// (see `EmbeddingScheduler::clear_active_if`).
///
/// Held as a local inside the stream's `async_stream::stream!` generator so
/// the claim is retracted on EVERY exit path — the loop's own `break`s
/// (shutdown-token cancellation, `RecvError::Closed`), AND the generator
/// being dropped without ever reaching a `break` at all, which is exactly
/// what happens when the client disconnects: `tonic` drops the response
/// stream, which drops this generator mid-suspension at whatever `.await`
/// it was parked on. A call placed after the loop instead of in a `Drop`
/// impl would never run in that case, leaving the claim (and the eviction
/// immunity it grants — see `EmbeddingScheduler::is_active`) stuck on this
/// database forever.
struct ActiveDbGuard {
    scheduler: Arc<EmbeddingScheduler>,
    database_id: String,
}

impl Drop for ActiveDbGuard {
    fn drop(&mut self) {
        self.scheduler.clear_active_if(&self.database_id);
    }
}

#[tonic::async_trait]
impl GrpcNodeService for NodeServiceImpl {
    async fn create_node(
        &self,
        request: Request<CreateNodeRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let properties = parse_properties(&req.properties).map_err(properties_error)?;

        use crate::nodespace::create_node_request::Position as CreatePos;
        let position = match req.position {
            Some(CreatePos::Beginning(_)) => InsertPositionOwned::Beginning,
            Some(CreatePos::End(_)) => InsertPositionOwned::End,
            Some(CreatePos::After(id)) => InsertPositionOwned::After(id),
            None => InsertPositionOwned::End,
        };

        let input = node_ops::CreateNodeInput {
            id: req.id,
            node_type: req.node_type,
            content: req.content,
            parent_id: req.parent_id,
            position,
            properties,
            collections: req.collections,
            collection_ids: req.collection_ids,
            lifecycle_status: req.lifecycle_status,
        };

        let output = node_ops::create_node(&this.node_service, input)
            .await
            .map_err(ops_error_to_status)?;

        let node = fetch_node(&this.node_service, &output.node_id).await?;
        let node_type = node.node_type.clone();

        Ok(Response::new(NodeResponse {
            node_id: output.node_id,
            node_type,
            node_data: Some(node_to_proto(node)),
        }))
    }

    async fn get_node(
        &self,
        request: Request<GetNodeRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let node = fetch_node(&this.node_service, &req.node_id).await?;
        let node_type = node.node_type.clone();

        Ok(Response::new(NodeResponse {
            node_id: req.node_id,
            node_type,
            node_data: Some(node_to_proto(node)),
        }))
    }

    async fn find_duplicate(
        &self,
        request: Request<FindDuplicateRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        // Suggest-don't-block (ADR-065): a match is a normal result, never an error,
        // and no match returns an EMPTY NodeResponse (empty node_id + no node_data)
        // rather than NotFound — the caller uses it to offer an adopt-existing
        // suggestion, so "no duplicate" is an ordinary answer.
        match this
            .node_service
            .find_duplicate_for(
                &req.node_type,
                &req.field,
                &req.value,
                req.exclude_id.as_deref(),
            )
            .await
            .map_err(service_error_to_status)?
        {
            Some(node) => {
                let node_id = node.id.clone();
                let node_type = node.node_type.clone();
                Ok(Response::new(NodeResponse {
                    node_id,
                    node_type,
                    node_data: Some(node_to_proto(node)),
                }))
            }
            None => Ok(Response::new(NodeResponse {
                node_id: String::new(),
                node_type: String::new(),
                node_data: None,
            })),
        }
    }

    async fn list_conflicts(
        &self,
        request: Request<ListConflictsRequest>,
    ) -> Result<Response<ConflictListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let status = req
            .status
            .as_deref()
            .map(nodespace_core::models::ConflictStatus::from_str)
            .transpose()
            .map_err(Status::invalid_argument)?;
        let kind = req
            .kind
            .as_deref()
            .map(nodespace_core::models::ConflictKind::from_str)
            .transpose()
            .map_err(Status::invalid_argument)?;
        let limit = (req.limit > 0).then_some(req.limit as u32);

        let records = this
            .node_service
            .list_conflicts(status, kind, limit)
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(ConflictListResponse {
            conflicts: records.into_iter().map(conflict_record_to_proto).collect(),
        }))
    }

    async fn conflicts_for_node(
        &self,
        request: Request<ConflictsForNodeRequest>,
    ) -> Result<Response<ConflictListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let records = this
            .node_service
            .conflicts_for_node(&req.node_id)
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(ConflictListResponse {
            conflicts: records.into_iter().map(conflict_record_to_proto).collect(),
        }))
    }

    async fn get_conflict(
        &self,
        request: Request<GetConflictRequest>,
    ) -> Result<Response<OptionalConflictResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let record = this
            .node_service
            .get_conflict(&req.conflict_id)
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(OptionalConflictResponse {
            conflict: record.map(conflict_record_to_proto),
        }))
    }

    async fn resolve_conflict(
        &self,
        request: Request<ResolveConflictRequest>,
    ) -> Result<Response<ConflictResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let resolution: nodespace_core::models::Resolution = serde_json::from_str(&req.resolution)
            .map_err(|e| Status::invalid_argument(format!("invalid resolution JSON: {e}")))?;

        let record = this
            .node_service
            .resolve_conflict(&req.conflict_id, resolution)
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(ConflictResponse {
            conflict: Some(conflict_record_to_proto(record)),
        }))
    }

    async fn merge_nodes(
        &self,
        request: Request<MergeNodesRequest>,
    ) -> Result<Response<MergeNodesResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let outcome = this
            .node_service
            .merge_nodes(&req.survivor_id, &req.loser_id, req.conflict_id.as_deref())
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(MergeNodesResponse {
            survivor_id: outcome.survivor_id,
            loser_id: outcome.loser_id,
            properties_merged: outcome.properties_merged,
            edges_repointed: outcome.edges_repointed,
            edges_dropped: outcome.edges_dropped,
        }))
    }

    async fn update_node(
        &self,
        request: Request<UpdateNodeRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let properties = match req.properties.as_deref() {
            Some(s) => Some(parse_properties(s).map_err(properties_error)?),
            None => None,
        };

        let input = node_ops::UpdateNodeInput {
            node_id: req.node_id,
            version: req.version,
            node_type: req.node_type,
            content: req.content,
            properties,
            add_to_collections: req.add_to_collections,
            add_to_collection_ids: req.add_to_collection_ids,
            remove_from_collection_ids: req.remove_from_collection_ids,
            lifecycle_status: req.lifecycle_status,
        };

        let output = node_ops::update_node(&this.node_service, input)
            .await
            .map_err(ops_error_to_status)?;

        let node = fetch_node(&this.node_service, &output.node_id).await?;
        let node_type = node.node_type.clone();

        Ok(Response::new(NodeResponse {
            node_id: output.node_id,
            node_type,
            node_data: Some(node_to_proto(node)),
        }))
    }

    async fn delete_node(
        &self,
        request: Request<DeleteNodeRequest>,
    ) -> Result<Response<DeleteNodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let input = node_ops::DeleteNodeInput {
            node_id: req.node_id,
            version: req.version,
        };

        // node_ops::delete_node handles auto-fetch; map NotFound to existed=false.
        let output = match node_ops::delete_node(&this.node_service, input).await {
            Ok(o) => o,
            Err(OpsError::NotFound { id }) => {
                return Ok(Response::new(DeleteNodeResponse {
                    node_id: id,
                    existed: false,
                    deleted_count: 0,
                }));
            }
            Err(e) => return Err(ops_error_to_status(e)),
        };

        Ok(Response::new(DeleteNodeResponse {
            node_id: output.node_id,
            existed: output.existed,
            deleted_count: output.deleted_count,
        }))
    }

    async fn reset_seed_node(
        &self,
        request: Request<ResetSeedNodeRequest>,
    ) -> Result<Response<ResetSeedNodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let Some(template) = resolve_seed_template(&req.node_type, &req.seed_key) else {
            return Ok(Response::new(ResetSeedNodeResponse {
                found: false,
                config_reset: false,
                guidance_reset: false,
                config_summary: String::new(),
                guidance_summary: String::new(),
            }));
        };

        let prepared = match nodespace_core::markdown::prepare_nodes_from_template(&template) {
            Ok(p) => p,
            Err(e) => {
                return Err(Status::internal(format!(
                    "Failed to expand seed template '{}': {e}",
                    req.seed_key
                )))
            }
        };

        // Before-state summary — read regardless of dry_run, since the
        // caller needs it either way (a confirmation prompt on dry_run, an
        // audit line on the real run).
        let existing = this
            .node_service
            .query_nodes(nodespace_core::models::NodeFilter {
                node_type: Some(req.node_type.clone()),
                ..Default::default()
            })
            .await
            .map_err(service_error_to_status)?
            .into_iter()
            .find(|n| {
                n.properties
                    .get("_seed")
                    .and_then(|s| s.get("key"))
                    .and_then(|v| v.as_str())
                    == Some(req.seed_key.as_str())
            });

        let Some(existing_node) = existing else {
            return Ok(Response::new(ResetSeedNodeResponse {
                found: false,
                config_reset: false,
                guidance_reset: false,
                config_summary: String::new(),
                guidance_summary: String::new(),
            }));
        };

        let config_summary = if req.reset_config {
            existing_node
                .properties
                .get(req.node_type.as_str())
                .cloned()
                .unwrap_or(existing_node.properties.clone())
                .to_string()
        } else {
            String::new()
        };
        let guidance_summary = if req.reset_guidance {
            let children = this
                .node_service
                .get_children(&existing_node.id)
                .await
                .map_err(service_error_to_status)?;
            format!(
                "{} guidance node(s), current preview: {:.120}",
                children.len(),
                children.first().map(|c| c.content.as_str()).unwrap_or("")
            )
        } else {
            String::new()
        };

        if req.dry_run {
            return Ok(Response::new(ResetSeedNodeResponse {
                found: true,
                config_reset: req.reset_config,
                guidance_reset: req.reset_guidance,
                config_summary,
                guidance_summary,
            }));
        }

        let (config_reset, guidance_reset) = this
            .node_service
            .reset_seed_node(
                &req.node_type,
                &req.seed_key,
                &prepared,
                req.reset_config,
                req.reset_guidance,
            )
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(ResetSeedNodeResponse {
            found: true,
            config_reset,
            guidance_reset,
            config_summary,
            guidance_summary,
        }))
    }

    async fn get_children(
        &self,
        request: Request<GetChildrenRequest>,
    ) -> Result<Response<NodeListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let children = this
            .node_service
            .get_children(&req.node_id)
            .await
            .map_err(service_error_to_status)?;

        let nodes: Vec<NodeData> = children.into_iter().map(node_to_proto).collect();

        let count = nodes.len() as i32;

        Ok(Response::new(NodeListResponse {
            nodes,
            count,
            collection_id: String::new(),
        }))
    }

    async fn get_children_tree(
        &self,
        request: Request<GetChildrenTreeRequest>,
    ) -> Result<Response<NodeTreeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let tree = this
            .node_service
            .get_children_tree(&req.node_id)
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(NodeTreeResponse {
            tree_json: tree.to_string(),
        }))
    }

    async fn get_roots(
        &self,
        request: Request<GetRootsRequest>,
    ) -> Result<Response<NodeListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        // Bound like QueryNodesSimple/CountNodes: 0 means "server default",
        // not "unlimited" — an explicit large value is clamped rather than
        // allowed to return every root node in one response. A caller that
        // only needs a total, not the records, should use CountRoots
        // instead of GetRoots + counting the response.
        const DEFAULT_GET_ROOTS_LIMIT: usize = 100;
        const MAX_GET_ROOTS_LIMIT: usize = 500;
        let limit = Some(if req.limit == 0 {
            DEFAULT_GET_ROOTS_LIMIT
        } else {
            (req.limit as usize).min(MAX_GET_ROOTS_LIMIT)
        });
        let offset = if req.offset == 0 {
            None
        } else {
            Some(req.offset as usize)
        };

        let roots = this
            .node_service
            .get_roots(limit, offset)
            .await
            .map_err(service_error_to_status)?;

        let nodes: Vec<NodeData> = roots.into_iter().map(node_to_proto).collect();
        let count = nodes.len() as i32;

        Ok(Response::new(NodeListResponse {
            nodes,
            count,
            collection_id: String::new(),
        }))
    }

    /// Count root nodes without listing them (see `NodeSortOrder`/`CountNodes`
    /// doc comments in the proto for the rationale — a count-only path so a
    /// caller like `nodespace diagnostics` doesn't pay to transfer records it
    /// only intends to call `.len()` on).
    async fn count_roots(
        &self,
        request: Request<Empty>,
    ) -> Result<Response<CountNodesResponse>, Status> {
        let this = self.route(&request).await?;

        let count = this
            .node_service
            .count_roots()
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(CountNodesResponse { count }))
    }

    async fn search_nodes(
        &self,
        request: Request<SearchRequest>,
    ) -> Result<Response<NodeListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        // Clone the Arc and drop the guard immediately rather than holding the
        // read lock for the rest of this function — `include_markdown` can now
        // do real work (up to 5 subtree traversals) inside `search_semantic`,
        // and there's no reason a concurrent embedding-model swap (which needs
        // the write lock) should wait on that.
        let embedding_service = {
            let guard = this.embedding_state.read().await;
            guard
                .as_ref()
                .map(|r| Arc::clone(&r.embedding_service))
                .ok_or_else(|| Status::unavailable("embedding model loading, please retry"))?
        };

        if !req.semantic {
            tracing::debug!(
                "SearchRequest.semantic=false ignored; structured query mode not yet implemented"
            );
        }

        let threshold = if req.threshold == 0.0 {
            None
        } else {
            Some(req.threshold)
        };
        let limit = if req.limit == 0 {
            None
        } else {
            Some(req.limit as usize)
        };

        let node_types = if req.node_types.is_empty() {
            None
        } else {
            Some(req.node_types)
        };

        let property_filters = if req.filters.is_empty() {
            None
        } else {
            Some(
                serde_json::from_str::<serde_json::Value>(&req.filters).map_err(|e| {
                    Status::invalid_argument(format!("Invalid filters JSON: {}", e))
                })?,
            )
        };

        // Clamp negative/oversized values rather than trusting the wire —
        // search_ops::search_semantic re-applies MAX_INCLUDE_MARKDOWN_RESULTS
        // internally regardless, but a negative i32 -> usize cast would
        // otherwise wrap to a huge number before it ever gets there.
        let include_markdown = Some(
            req.include_markdown
                .clamp(0, search_ops::MAX_INCLUDE_MARKDOWN_RESULTS as i32) as usize,
        );

        let input = SearchSemanticInput {
            query: req.query,
            threshold,
            limit,
            collection_id: req.collection_id,
            collection: req.collection,
            exclude_collections: None,
            include_markdown,
            include_archived: None,
            scope: None,
            node_types,
            property_filters,
            include_edges: None,
            graph_boost: None,
        };

        let output = search_ops::search_semantic(&this.node_service, &embedding_service, input)
            .await
            .map_err(ops_error_to_status)?;

        // Use the nodes search already fetched rather than re-reading each result
        // from the store by id. The re-fetch was one query and one
        // reader-connection checkout per result — work that scales with the
        // result count to reproduce rows search is already holding.
        //
        // `output.nodes` (JSON-shaped) and `output.matched_nodes` (typed) are
        // both built from the same ranked result set in the same order, but
        // matched by id rather than by index — a defensive choice that costs
        // nothing here and doesn't rely on that ordering staying in sync.
        // `output.nodes` is the only place a requested `include_markdown`
        // attachment lives; `matched_nodes` carries none of it.
        let markdown_by_id: std::collections::HashMap<&str, &str> = output
            .nodes
            .iter()
            .filter_map(|value| {
                let id = value.get("id").and_then(|v| v.as_str())?;
                let markdown = value.get("markdown").and_then(|v| v.as_str())?;
                Some((id, markdown))
            })
            .collect();

        let nodes: Vec<NodeData> = output
            .matched_nodes
            .into_iter()
            .map(|node| {
                let mut node_data = node_to_proto(node);
                if let Some(markdown) = markdown_by_id.get(node_data.id.as_str()) {
                    node_data.markdown = markdown.to_string();
                }
                node_data
            })
            .collect();

        let count = nodes.len() as i32;
        Ok(Response::new(NodeListResponse {
            nodes,
            count,
            collection_id: output.collection_id.unwrap_or_default(),
        }))
    }

    async fn query_nodes_simple(
        &self,
        request: Request<QueryNodesSimpleRequest>,
    ) -> Result<Response<NodeListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        // Cap at MAX_QUERY_NODES_SIMPLE_LIMIT regardless of client-requested
        // value — same clamp shape as ExecuteQuery's MAX_EXECUTE_QUERY_LIMIT.
        // Combined with order_by_from_proto's always-deterministic default
        // ordering, this is what makes repeated limit/offset calls tile the
        // full matching set exactly once each instead of relying on the gRPC
        // message-size ceiling to (badly) fail an unbounded response.
        const MAX_QUERY_NODES_SIMPLE_LIMIT: usize = 500;
        let query = NodeQuery {
            id: req.id,
            ids: None,
            mentioned_by: req.mentioned_by,
            content_contains: req.content_contains,
            title_contains: req.title_contains,
            node_type: req.node_type,
            order_by: Some(order_by_from_proto(req.order_by)),
            limit: if req.limit == 0 {
                None
            } else {
                Some((req.limit as usize).min(MAX_QUERY_NODES_SIMPLE_LIMIT))
            },
            offset: if req.offset == 0 {
                None
            } else {
                Some(req.offset as usize)
            },
        };

        let nodes = this
            .node_service
            .query_nodes_simple(query)
            .await
            .map_err(service_error_to_status)?;

        let proto_nodes: Vec<NodeData> = nodes.into_iter().map(node_to_proto).collect();
        let count = proto_nodes.len() as i32;

        Ok(Response::new(NodeListResponse {
            nodes: proto_nodes,
            count,
            collection_id: String::new(),
        }))
    }

    /// Count nodes matching the same filter shape as `QueryNodesSimple`
    /// without transferring the matching records — a caller like `nodespace
    /// diagnostics` that only needs a total should use this instead of
    /// `QueryNodesSimple` + counting the response, which pays to materialize
    /// and transfer up to `MAX_QUERY_NODES_SIMPLE_LIMIT` full records purely
    /// to call `.len()`.
    async fn count_nodes(
        &self,
        request: Request<QueryNodesSimpleRequest>,
    ) -> Result<Response<CountNodesResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        // limit/offset/order_by are meaningless for a scalar count —
        // deliberately dropped here rather than threaded through.
        let query = NodeQuery {
            id: req.id,
            ids: None,
            mentioned_by: req.mentioned_by,
            content_contains: req.content_contains,
            title_contains: req.title_contains,
            node_type: req.node_type,
            order_by: None,
            limit: None,
            offset: None,
        };

        let count = this
            .node_service
            .count_nodes(query)
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(CountNodesResponse { count }))
    }

    async fn execute_query(
        &self,
        request: Request<ExecuteQueryRequest>,
    ) -> Result<Response<NodeListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let filters = match req.filters_json.as_deref() {
            Some(raw) if !raw.is_empty() => serde_json::from_str(raw)
                .map_err(|e| Status::invalid_argument(format!("invalid filters_json: {e}")))?,
            _ => Vec::new(),
        };
        let sorting = match req.sorting_json.as_deref() {
            Some(raw) if !raw.is_empty() => Some(
                serde_json::from_str(raw)
                    .map_err(|e| Status::invalid_argument(format!("invalid sorting_json: {e}")))?,
            ),
            _ => None,
        };

        // Cap at 500 regardless of client-requested value — matches the scale
        // of ExecuteQueryInput's own default (50) while still allowing large
        // explicit pulls, without letting a client request an unbounded scan.
        const MAX_EXECUTE_QUERY_LIMIT: usize = 500;
        let input = query_ops::ExecuteQueryInput {
            target_type: req.target_type,
            filters,
            sorting,
            limit: if req.limit == 0 {
                None
            } else {
                Some((req.limit as usize).min(MAX_EXECUTE_QUERY_LIMIT))
            },
        };

        let nodes = query_ops::execute_query_nodes(&this.node_service, input)
            .await
            .map_err(ops_error_to_status)?;

        let proto_nodes: Vec<NodeData> = nodes.into_iter().map(node_to_proto).collect();
        let count = proto_nodes.len() as i32;

        Ok(Response::new(NodeListResponse {
            nodes: proto_nodes,
            count,
            collection_id: String::new(),
        }))
    }

    async fn mention_autocomplete(
        &self,
        request: Request<MentionAutocompleteRequest>,
    ) -> Result<Response<NodeListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let limit = if req.limit == 0 {
            None
        } else {
            Some(req.limit as usize)
        };

        let nodes = this
            .node_service
            .mention_autocomplete(&req.query, limit)
            .await
            .map_err(service_error_to_status)?;

        let proto_nodes: Vec<NodeData> = nodes.into_iter().map(node_to_proto).collect();
        let count = proto_nodes.len() as i32;

        Ok(Response::new(NodeListResponse {
            nodes: proto_nodes,
            count,
            collection_id: String::new(),
        }))
    }

    async fn upsert_node_with_parent(
        &self,
        request: Request<UpsertNodeWithParentRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        this.node_service
            .upsert_node_with_parent(
                &req.node_id,
                &req.content,
                &req.node_type,
                &req.parent_id,
                &req.root_id,
                None, // before_sibling_id intentionally None for fractional ordering
            )
            .await
            .map_err(service_error_to_status)?;

        let node = fetch_node(&this.node_service, &req.node_id).await?;
        let node_type = node.node_type.clone();
        Ok(Response::new(NodeResponse {
            node_id: req.node_id,
            node_type,
            node_data: Some(node_to_proto(node)),
        }))
    }

    async fn move_node(
        &self,
        request: Request<MoveNodeRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        // Normalize "" to None: both unset and empty-string mean "move to root".
        let new_parent = req.new_parent_id.filter(|s| !s.is_empty());

        use crate::nodespace::move_node_request::Position as MovePos;
        let position = match req.position {
            Some(MovePos::Beginning(_)) => InsertPosition::Beginning,
            Some(MovePos::End(_)) => InsertPosition::End,
            Some(MovePos::After(ref id)) => InsertPosition::After(id.as_str()),
            None => InsertPosition::End,
        };

        let node = this
            .node_service
            .move_node(&req.node_id, req.version, new_parent.as_deref(), position)
            .await
            .map_err(service_error_to_status)?;

        let node_type = node.node_type.clone();
        Ok(Response::new(NodeResponse {
            node_id: node.id.clone(),
            node_type,
            node_data: Some(node_to_proto(node)),
        }))
    }

    async fn reorder_node(
        &self,
        request: Request<ReorderNodeRequest>,
    ) -> Result<Response<ReorderNodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        use crate::nodespace::reorder_node_request::Position as ReorderPos;
        let position = match req.position {
            Some(ReorderPos::Beginning(_)) => InsertPosition::Beginning,
            Some(ReorderPos::End(_)) => InsertPosition::End,
            Some(ReorderPos::After(ref id)) => InsertPosition::After(id.as_str()),
            None => InsertPosition::End,
        };

        this.node_service
            .reorder_node(&req.node_id, req.version, position)
            .await
            .map_err(service_error_to_status)?;

        Ok(Response::new(ReorderNodeResponse {}))
    }

    async fn move_children_to_parent(
        &self,
        request: Request<MoveChildrenToParentRequest>,
    ) -> Result<Response<MoveChildrenToParentResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let children: Vec<(String, i64)> = req
            .children
            .into_iter()
            .map(|c| (c.node_id, c.version))
            .collect();

        let updated = this
            .node_service
            .move_children_to_parent(&req.new_parent_id, &children)
            .await
            .map_err(service_error_to_status)?;

        let children_proto = updated.into_iter().map(node_to_proto).collect();

        Ok(Response::new(MoveChildrenToParentResponse {
            children: children_proto,
        }))
    }

    async fn create_mention(
        &self,
        request: Request<CreateMentionRequest>,
    ) -> Result<Response<MentionResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        this.node_service
            .create_mention(&req.mentioning_node_id, &req.mentioned_node_id)
            .await
            .map_err(service_error_to_status)?;
        Ok(Response::new(MentionResponse {
            mentioning_node_id: req.mentioning_node_id,
            mentioned_node_id: req.mentioned_node_id,
        }))
    }

    async fn delete_mention(
        &self,
        request: Request<DeleteMentionRequest>,
    ) -> Result<Response<MentionResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        this.node_service
            .remove_mention(&req.mentioning_node_id, &req.mentioned_node_id)
            .await
            .map_err(service_error_to_status)?;
        Ok(Response::new(MentionResponse {
            mentioning_node_id: req.mentioning_node_id,
            mentioned_node_id: req.mentioned_node_id,
        }))
    }

    async fn get_outgoing_mentions(
        &self,
        request: Request<MentionTargetRequest>,
    ) -> Result<Response<MentionIdsResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let ids = this
            .node_service
            .get_mentions(&req.node_id)
            .await
            .map_err(service_error_to_status)?;
        Ok(Response::new(MentionIdsResponse { node_ids: ids }))
    }

    async fn get_incoming_mentions(
        &self,
        request: Request<MentionTargetRequest>,
    ) -> Result<Response<MentionIdsResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let ids = this
            .node_service
            .get_mentioned_by(&req.node_id)
            .await
            .map_err(service_error_to_status)?;
        Ok(Response::new(MentionIdsResponse { node_ids: ids }))
    }

    async fn get_mentioning_roots(
        &self,
        request: Request<MentionTargetRequest>,
    ) -> Result<Response<NodeReferenceListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let refs = this
            .node_service
            .get_mentioning_containers(&req.node_id)
            .await
            .map_err(service_error_to_status)?;

        let references = refs
            .into_iter()
            .map(|r| NodeReference {
                id: r.id,
                title: r.title,
                node_type: r.node_type,
            })
            .collect();

        Ok(Response::new(NodeReferenceListResponse { references }))
    }

    async fn get_daemon_version(
        &self,
        _request: Request<GetDaemonVersionRequest>,
    ) -> Result<Response<GetDaemonVersionResponse>, Status> {
        // The daemon's own compiled version — not tenant-scoped, so no routing.
        Ok(Response::new(GetDaemonVersionResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }))
    }

    async fn create_relationship(
        &self,
        request: Request<CreateRelationshipRequest>,
    ) -> Result<Response<CreateRelationshipResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let edge_data =
            match req.edge_data_json.as_deref() {
                Some(raw) if !raw.is_empty() => Some(serde_json::from_str(raw).map_err(|e| {
                    Status::invalid_argument(format!("invalid edge_data_json: {e}"))
                })?),
                _ => None,
            };

        let input = rel_ops::CreateRelInput {
            source_id: req.source_id,
            relationship_name: req.relationship_name,
            target_id: req.target_id,
            edge_data,
        };

        let output = rel_ops::create_relationship(&this.node_service, input)
            .await
            .map_err(ops_error_to_status)?;

        Ok(Response::new(CreateRelationshipResponse {
            source_id: output.source_id,
            relationship_name: output.relationship_name,
            target_id: output.target_id,
        }))
    }

    async fn delete_relationship(
        &self,
        request: Request<DeleteRelationshipRequest>,
    ) -> Result<Response<DeleteRelationshipResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let input = rel_ops::DeleteRelInput {
            source_id: req.source_id,
            relationship_name: req.relationship_name,
            target_id: req.target_id,
        };

        rel_ops::delete_relationship(&this.node_service, input)
            .await
            .map_err(ops_error_to_status)?;

        Ok(Response::new(DeleteRelationshipResponse {}))
    }

    async fn update_relationship_properties(
        &self,
        request: Request<UpdateRelationshipPropertiesRequest>,
    ) -> Result<Response<UpdateRelationshipPropertiesResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let properties: serde_json::Value = serde_json::from_str(&req.properties_json)
            .map_err(|e| Status::invalid_argument(format!("invalid properties_json: {e}")))?;

        let input = rel_ops::UpdateRelPropsInput {
            source_id: req.source_id,
            relationship_name: req.relationship_name,
            target_id: req.target_id,
            properties,
        };

        rel_ops::update_relationship_properties(&this.node_service, input)
            .await
            .map_err(ops_error_to_status)?;

        Ok(Response::new(UpdateRelationshipPropertiesResponse {}))
    }

    async fn get_related_nodes(
        &self,
        request: Request<GetRelatedNodesRequest>,
    ) -> Result<Response<GetRelatedNodesResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        if req.direction != "in" && req.direction != "out" {
            return Err(Status::invalid_argument("direction must be 'in' or 'out'"));
        }

        let input = rel_ops::GetRelatedInput {
            node_id: req.node_id,
            relationship_name: req.relationship_name,
            direction: req.direction,
        };

        let output = rel_ops::get_related_nodes(&this.node_service, input)
            .await
            .map_err(ops_error_to_status)?;

        let related_nodes_json = serde_json::to_string(&output.related_nodes)
            .map_err(|e| Status::internal(format!("failed to serialize related_nodes: {e}")))?;

        Ok(Response::new(GetRelatedNodesResponse {
            node_id: output.node_id,
            relationship_name: output.relationship_name,
            direction: output.direction,
            related_nodes_json,
            count: output.count as i32,
        }))
    }

    async fn get_node_relationships(
        &self,
        request: Request<GetNodeRelationshipsRequest>,
    ) -> Result<Response<GetNodeRelationshipsResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let output = rel_ops::get_node_relationships(&this.node_service, &req.node_id)
            .await
            .map_err(ops_error_to_status)?;

        let relationships_json = serde_json::to_string(&output)
            .map_err(|e| Status::internal(format!("failed to serialize relationships: {e}")))?;

        Ok(Response::new(GetNodeRelationshipsResponse {
            relationships_json,
        }))
    }

    async fn update_task_node(
        &self,
        request: Request<UpdateTaskNodeRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let update = build_task_node_update(
            req.status,
            req.priority,
            req.due_date,
            req.started_at,
            req.completed_at,
            req.content,
        )
        .map_err(Status::invalid_argument)?;

        let task = match this
            .node_service
            .update_task_node(&req.node_id, req.version, update)
            .await
        {
            Ok(t) => t,
            Err(NodeServiceError::VersionConflict {
                node_id,
                expected_version,
                actual_version,
            }) => {
                // Fetch the authoritative current state so the client can
                // hydrate without a second round-trip (mirrors the pattern
                // used by node_ops::update_node for regular-node conflicts).
                // Flattened via `node_to_typed_value` so the payload matches
                // the wire shape of every other response — the client writes
                // it straight into its store, where type-specific fields are
                // read from the top level.
                let current_node = this
                    .node_service
                    .get_node(&node_id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|n| nodespace_core::models::node_to_typed_value(n).ok());
                return Err(ops_error_to_status(OpsError::VersionConflict {
                    node_id,
                    expected: expected_version,
                    actual: actual_version,
                    current_node,
                }));
            }
            Err(e) => return Err(service_error_to_status(e)),
        };

        // Convert TaskNode back to Node for proto wire shape. Frontend reconstructs
        // the typed view via task_node_to_typed_value on the Tauri side.
        let node: Node = task.into_node();
        let node_type = node.node_type.clone();
        let node_id = node.id.clone();

        Ok(Response::new(NodeResponse {
            node_id,
            node_type,
            node_data: Some(node_to_proto(node)),
        }))
    }

    // -- Local identity (ADR-037) ---------------------------------

    async fn get_local_person(
        &self,
        request: Request<Empty>,
    ) -> Result<Response<OptionalNodeResponse>, Status> {
        let this = self.route(&request).await?;

        let person = this
            .node_service
            .get_local_person()
            .await
            .map_err(service_error_to_status)?;

        let node_response = person.map(|n| {
            let node_type = n.node_type.clone();
            let node_id = n.id.clone();
            NodeResponse {
                node_id,
                node_type,
                node_data: Some(node_to_proto(n)),
            }
        });
        Ok(Response::new(OptionalNodeResponse {
            node: node_response,
        }))
    }

    async fn set_local_person_identity(
        &self,
        request: Request<SetLocalPersonIdentityRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let node = this
            .node_service
            .set_local_person_identity(&req.first_name, &req.last_name, &req.email)
            .await
            .map_err(service_error_to_status)?;

        let node_type = node.node_type.clone();
        let node_id = node.id.clone();
        Ok(Response::new(NodeResponse {
            node_id,
            node_type,
            node_data: Some(node_to_proto(node)),
        }))
    }

    // -- Markdown export -----------------------------------------------------

    async fn export_markdown(
        &self,
        request: Request<ExportMarkdownRequest>,
    ) -> Result<Response<ExportMarkdownResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        use serde_json::json;
        let params = json!({
            "node_id": req.node_id,
            "include_children": req.include_children.unwrap_or(true),
            "max_depth": if req.max_depth == 0 { 20u32 } else { req.max_depth },
            "include_node_ids": req.include_node_ids.unwrap_or(true),
        });

        let result =
            nodespace_core::markdown::handle_get_markdown_from_node_id(&this.node_service, params)
                .await
                .map_err(|e| match e {
                    nodespace_core::markdown::MarkdownError::NotFound(m) => Status::not_found(m),
                    nodespace_core::markdown::MarkdownError::InvalidParams(m) => {
                        Status::invalid_argument(m)
                    }
                    nodespace_core::markdown::MarkdownError::CreationFailed(m) => {
                        Status::failed_precondition(format!("Node creation failed: {m}"))
                    }
                    nodespace_core::markdown::MarkdownError::Internal(m) => Status::internal(m),
                })?;

        let markdown = result["markdown"]
            .as_str()
            .ok_or_else(|| Status::internal("ExportMarkdown: missing 'markdown' in response"))?
            .to_string();
        let node_count = result["node_count"]
            .as_u64()
            .ok_or_else(|| Status::internal("ExportMarkdown: missing 'node_count' in response"))?
            as u32;

        Ok(Response::new(ExportMarkdownResponse {
            markdown,
            node_count,
        }))
    }

    // -- Batch operations ----------------------------------------------------

    async fn get_nodes_batch(
        &self,
        request: Request<GetNodesBatchRequest>,
    ) -> Result<Response<GetNodesBatchResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        if req.node_ids.is_empty() {
            return Err(Status::invalid_argument("node_ids cannot be empty"));
        }
        if req.node_ids.len() > 100 {
            return Err(Status::invalid_argument(format!(
                "Batch size exceeds maximum of 100 (got {})",
                req.node_ids.len()
            )));
        }

        let id_refs: Vec<&str> = req.node_ids.iter().map(String::as_str).collect();
        let fetched = this
            .node_service
            .get_nodes(&id_refs)
            .await
            .map_err(service_error_to_status)?;

        let fetched_ids: std::collections::HashSet<String> =
            fetched.iter().map(|n| n.id.clone()).collect();
        let not_found: Vec<String> = req
            .node_ids
            .iter()
            .filter(|id| !fetched_ids.contains(*id))
            .cloned()
            .collect();

        let nodes: Vec<NodeData> = fetched.into_iter().map(node_to_proto).collect();
        let count = nodes.len() as i32;

        Ok(Response::new(GetNodesBatchResponse {
            nodes,
            not_found,
            count,
        }))
    }

    async fn update_nodes_batch(
        &self,
        request: Request<UpdateNodesBatchRequest>,
    ) -> Result<Response<UpdateNodesBatchResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        if req.updates.is_empty() {
            return Err(Status::invalid_argument("updates cannot be empty"));
        }
        if req.updates.len() > 100 {
            return Err(Status::invalid_argument(format!(
                "Batch size exceeds maximum of 100 (got {})",
                req.updates.len()
            )));
        }

        let mut updated = Vec::new();
        let mut failed = Vec::new();

        for item in req.updates {
            let version = match item.version {
                Some(v) => v,
                None => {
                    tracing::warn!(
                        node_id = %item.node_id,
                        "OCC bypassed: version not provided for batch update (race condition possible)"
                    );
                    match this.node_service.get_node(&item.node_id).await {
                        Ok(Some(node)) => node.version,
                        Ok(None) => {
                            failed.push(BatchUpdateFailure {
                                node_id: item.node_id.clone(),
                                error: format!("Node '{}' not found", item.node_id),
                            });
                            continue;
                        }
                        Err(e) => {
                            failed.push(BatchUpdateFailure {
                                node_id: item.node_id.clone(),
                                error: e.to_string(),
                            });
                            continue;
                        }
                    }
                }
            };

            let props = match item.properties {
                Some(ref s) => match parse_properties(s) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        failed.push(BatchUpdateFailure {
                            node_id: item.node_id.clone(),
                            error: format!("invalid properties JSON: {e}"),
                        });
                        continue;
                    }
                },
                None => None,
            };

            let node_update = NodeUpdate {
                content: item.content,
                node_type: item.node_type,
                properties: props,
                title: None,
                lifecycle_status: None,
            };

            match this
                .node_service
                .update_node(&item.node_id, version, node_update)
                .await
            {
                Ok(_) => updated.push(item.node_id),
                Err(e) => {
                    failed.push(BatchUpdateFailure {
                        node_id: item.node_id,
                        error: e.to_string(),
                    });
                }
            }
        }

        let count = updated.len() as i32;
        Ok(Response::new(UpdateNodesBatchResponse {
            updated,
            failed,
            count,
        }))
    }

    // -- Schemas (read-only) -------------------------------------------------

    async fn get_all_schemas(
        &self,
        request: Request<GetAllSchemasRequest>,
    ) -> Result<Response<NodeListResponse>, Status> {
        let this = self.route(&request).await?;
        // Hydrated fetch: relationship declarations are relationship-table rows,
        // not a `properties` key, so the wire node is assembled via
        // `into_wire_node` (which embeds `relationships` back into the
        // properties JSON the desktop client parses).
        let schemas = this
            .node_service
            .get_all_schemas()
            .await
            .map_err(service_error_to_status)?;

        let proto_nodes: Vec<NodeData> = schemas
            .into_iter()
            .map(|schema| node_to_proto(schema.into_wire_node()))
            .collect();
        let count = proto_nodes.len() as i32;

        Ok(Response::new(NodeListResponse {
            nodes: proto_nodes,
            count,
            collection_id: String::new(),
        }))
    }

    async fn get_schema_definition(
        &self,
        request: Request<GetSchemaDefinitionRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        // Distinguish "not a schema" from "absent" for accurate statuses.
        let schema = match this
            .node_service
            .get_schema_node(&req.schema_id)
            .await
            .map_err(service_error_to_status)?
        {
            Some(schema) => schema,
            None => {
                // Absent id → fetch_node's NotFound; present non-schema node →
                // failed_precondition; present schema node that failed to parse
                // → internal (a stored-data bug, not a caller error).
                let node = fetch_node(&this.node_service, &req.schema_id).await?;
                if node.node_type == "schema" {
                    return Err(Status::internal(format!(
                        "Schema node '{}' exists but could not be parsed as a schema definition",
                        req.schema_id
                    )));
                }
                return Err(Status::failed_precondition(format!(
                    "Node '{}' is not a schema (type={})",
                    req.schema_id, node.node_type
                )));
            }
        };
        Ok(Response::new(NodeResponse {
            node_id: req.schema_id,
            node_type: "schema".to_string(),
            node_data: Some(node_to_proto(schema.into_wire_node())),
        }))
    }

    async fn create_schema(
        &self,
        request: Request<SchemaParamsRequest>,
    ) -> Result<Response<SchemaResultResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let params: serde_json::Value = serde_json::from_str(&req.params_json)
            .map_err(|e| Status::invalid_argument(format!("invalid params_json: {e}")))?;

        let result = nodespace_core::schema::handle_create_schema(&this.node_service, params)
            .await
            .map_err(markdown_error_to_status)?;

        Ok(Response::new(SchemaResultResponse {
            result_json: result.to_string(),
        }))
    }

    async fn update_schema(
        &self,
        request: Request<SchemaParamsRequest>,
    ) -> Result<Response<SchemaResultResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let params: serde_json::Value = serde_json::from_str(&req.params_json)
            .map_err(|e| Status::invalid_argument(format!("invalid params_json: {e}")))?;

        let result = nodespace_core::schema::handle_update_schema(&this.node_service, params)
            .await
            .map_err(markdown_error_to_status)?;

        Ok(Response::new(SchemaResultResponse {
            result_json: result.to_string(),
        }))
    }

    // -- Collections ---------------------------------------------------------

    async fn get_all_collections(
        &self,
        request: Request<GetAllCollectionsRequest>,
    ) -> Result<Response<CollectionListResponse>, Status> {
        let this = self.route(&request).await?;
        let output =
            collection_ops::get_all_collections(&this.node_service, GetAllCollectionsInput)
                .await
                .map_err(ops_error_to_status)?;

        let collections = output
            .collections
            .into_iter()
            .map(|e| CollectionInfo {
                node: Some(node_to_proto(e.node)),
                member_count: e.member_count as u32,
                parent_collection_ids: e.parent_collection_ids,
            })
            .collect();

        Ok(Response::new(CollectionListResponse { collections }))
    }

    async fn get_collection_members(
        &self,
        request: Request<CollectionMembersRequest>,
    ) -> Result<Response<NodeListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let output = collection_ops::get_collection_members(
            &this.node_service,
            GetCollectionMembersInput {
                collection_id: req.collection_id,
            },
        )
        .await
        .map_err(ops_error_to_status)?;

        let nodes: Vec<NodeData> = output.members.into_iter().map(node_to_proto).collect();
        let count = nodes.len() as i32;

        Ok(Response::new(NodeListResponse {
            nodes,
            count,
            collection_id: output.collection_id,
        }))
    }

    async fn get_collection_members_recursive(
        &self,
        request: Request<CollectionMembersRequest>,
    ) -> Result<Response<NodeListResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let output = collection_ops::get_collection_members_recursive(
            &this.node_service,
            GetCollectionMembersRecursiveInput {
                collection_id: req.collection_id,
            },
        )
        .await
        .map_err(ops_error_to_status)?;

        let nodes: Vec<NodeData> = output.members.into_iter().map(node_to_proto).collect();
        let count = nodes.len() as i32;

        Ok(Response::new(NodeListResponse {
            nodes,
            count,
            collection_id: output.collection_id,
        }))
    }

    async fn get_node_collections(
        &self,
        request: Request<NodeCollectionsRequest>,
    ) -> Result<Response<CollectionIdsResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let output = collection_ops::get_node_collections(
            &this.node_service,
            GetNodeCollectionsInput {
                node_id: req.node_id,
            },
        )
        .await
        .map_err(ops_error_to_status)?;
        Ok(Response::new(CollectionIdsResponse {
            collection_ids: output.collection_ids,
        }))
    }

    async fn add_node_to_collection(
        &self,
        request: Request<AddNodeToCollectionRequest>,
    ) -> Result<Response<Empty>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        collection_ops::add_node_to_collection(
            &this.node_service,
            AddNodeToCollectionInput {
                node_id: req.node_id,
                collection_id: req.collection_id,
            },
        )
        .await
        .map_err(ops_error_to_status)?;
        Ok(Response::new(Empty {}))
    }

    async fn add_node_to_collection_by_path(
        &self,
        request: Request<AddNodeToCollectionByPathRequest>,
    ) -> Result<Response<CollectionIdResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let output = collection_ops::add_node_to_collection_by_path(
            &this.node_service,
            AddNodeToCollectionByPathInput {
                node_id: req.node_id,
                collection_path: req.collection_path,
            },
        )
        .await
        .map_err(ops_error_to_status)?;
        Ok(Response::new(CollectionIdResponse {
            collection_id: output.collection_id,
        }))
    }

    async fn remove_node_from_collection(
        &self,
        request: Request<RemoveNodeFromCollectionRequest>,
    ) -> Result<Response<Empty>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        collection_ops::remove_node_from_collection(
            &this.node_service,
            RemoveNodeFromCollectionInput {
                node_id: req.node_id,
                collection_id: req.collection_id,
            },
        )
        .await
        .map_err(ops_error_to_status)?;
        Ok(Response::new(Empty {}))
    }

    async fn find_collection_by_path(
        &self,
        request: Request<FindCollectionByPathRequest>,
    ) -> Result<Response<OptionalNodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let output = collection_ops::find_collection_by_path(
            &this.node_service,
            FindCollectionByPathInput {
                collection_path: req.collection_path,
            },
        )
        .await
        .map_err(ops_error_to_status)?;

        let node_response = output.collection.map(|n| {
            let node_type = n.node_type.clone();
            let node_id = n.id.clone();
            NodeResponse {
                node_id,
                node_type,
                node_data: Some(node_to_proto(n)),
            }
        });
        Ok(Response::new(OptionalNodeResponse {
            node: node_response,
        }))
    }

    async fn get_collection_by_name(
        &self,
        request: Request<GetCollectionByNameRequest>,
    ) -> Result<Response<OptionalNodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let output = collection_ops::get_collection_by_name(
            &this.node_service,
            GetCollectionByNameInput { name: req.name },
        )
        .await
        .map_err(ops_error_to_status)?;

        let node_response = output.collection.map(|n| {
            let node_type = n.node_type.clone();
            let node_id = n.id.clone();
            NodeResponse {
                node_id,
                node_type,
                node_data: Some(node_to_proto(n)),
            }
        });
        Ok(Response::new(OptionalNodeResponse {
            node: node_response,
        }))
    }

    async fn create_collection(
        &self,
        request: Request<CreateCollectionRequest>,
    ) -> Result<Response<CollectionIdResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let output = collection_ops::create_collection(
            &this.node_service,
            CreateCollectionInput {
                name: req.name,
                description: req.description,
            },
        )
        .await
        .map_err(ops_error_to_status)?;

        Ok(Response::new(CollectionIdResponse {
            collection_id: output.collection_id,
        }))
    }

    async fn rename_collection(
        &self,
        request: Request<RenameCollectionRequest>,
    ) -> Result<Response<NodeResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        let output = collection_ops::rename_collection(
            &this.node_service,
            RenameCollectionInput {
                collection_id: req.collection_id,
                new_name: req.new_name,
                version: req.version,
            },
        )
        .await
        .map_err(ops_error_to_status)?;

        let node = output.node;
        let node_type = node.node_type.clone();
        let node_id = node.id.clone();
        Ok(Response::new(NodeResponse {
            node_id,
            node_type,
            node_data: Some(node_to_proto(node)),
        }))
    }

    async fn delete_collection(
        &self,
        request: Request<DeleteCollectionRequest>,
    ) -> Result<Response<Empty>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();
        this.node_service
            .delete_node(&req.collection_id, req.version)
            .await
            .map_err(service_error_to_status)?;
        Ok(Response::new(Empty {}))
    }

    // -- Streaming (unimplemented; tracked separately) -----------------------

    type WatchNodesStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<NodeEvent, Status>> + Send + 'static>>;

    async fn watch_nodes(
        &self,
        request: Request<WatchRequest>,
    ) -> Result<Response<Self::WatchNodesStream>, Status> {
        // Read the subscriber's own client id BEFORE `route()`: `route()` also
        // reads `x-ns-client-id`, but to scope *writes* made through the
        // returned `node_service` — irrelevant here, since this handler never
        // writes. The subscriber's id is instead used below to filter its own
        // echoes out of the stream it is opening.
        let subscriber_client_id = client_id_header(&request).map_err(|e| *e)?;
        let this = self.route(&request).await?;

        // A live edit stream marks this database active (ADR-053: per-database
        // compute scoping): its embedding batches now take priority over other
        // open databases' backlogs on the shared model. The desktop opens this
        // stream on the database the user is looking at, so the active signal
        // tracks the foreground database with no extra protocol.
        this.scheduler.set_active(Some(this.database_id.clone()));

        let req = request.into_inner();
        if !req.node_type.is_empty() || !req.root_id.is_empty() {
            // Filtering is intentionally out of scope for the initial implementation
            // (this filtering is a documented Non-Goal). Log so clients can see the
            // request was accepted but the filter is being ignored.
            tracing::debug!(
                node_type = %req.node_type,
                root_id = %req.root_id,
                "WatchNodes filter fields are not yet implemented; streaming all events"
            );
        }

        let mut rx = this.node_service.subscribe_to_events();
        // Clone the Arc so the stream owns its own handle — the stream future
        // outlives `&self` (it is returned to tonic and polled independently),
        // so it cannot borrow from the handler scope.
        let node_service = this.node_service.clone();
        // The database this stream serves (ADR-053) — stamped onto every event.
        let database_id = this.database_id.clone();
        // Cancelled when this database's service set is torn down (idle
        // eviction, deliberate close, or daemon shutdown) — see the
        // `shutdown_token` field's doc comment. Selected against `rx.recv()`
        // below so the stream actually ends instead of surviving as a zombie:
        // the stream's own `node_service` clone above owns the event sender
        // `rx` reads from, so absent this signal `rx.recv()` could never
        // observe the channel as closed on its own once the database that
        // opened it is gone — it would just silently receive nothing forever
        // once that database is evicted and reopened as a fresh instance with
        // its own bus.
        let shutdown_token = this.shutdown_token.clone();
        let scheduler = this.scheduler.clone();

        let stream = async_stream::stream! {
            // Retracts the active-database claim taken above on every exit
            // path from this generator — see `ActiveDbGuard`'s doc comment.
            let _active_guard = ActiveDbGuard {
                scheduler,
                database_id: database_id.clone(),
            };

            loop {
                let envelope_result = tokio::select! {
                    biased;
                    () = shutdown_token.cancelled() => {
                        tracing::debug!(
                            database_id = %database_id,
                            "WatchNodes stream ending: database service set torn down"
                        );
                        break;
                    }
                    result = rx.recv() => result,
                };
                match envelope_result {
                    Ok(envelope) => {
                        // Same-origin echo suppression (ADR-026 C5 extension):
                        // the daemon is the sole authority on "is this my own
                        // write echoed back" — a subscriber with a client id
                        // never sees an event whose write was made through a
                        // `node_service` scoped with that same id. A
                        // subscriber with no client id (legacy/anonymous
                        // caller) suppresses nothing, matching pre-existing
                        // behavior; an event with no `source_client_id`
                        // (write made through an unscoped `node_service`) is
                        // never suppressed either.
                        let is_own_echo = matches!(
                            (&subscriber_client_id, &envelope.metadata.source_client_id),
                            (Some(sub), Some(src)) if sub == src
                        );
                        if is_own_echo {
                            continue;
                        }
                        // Translation is serial: a slow `get_node` lookup will
                        // delay the next `rx.recv()` and increase the risk of
                        // `Lagged`. Acceptable because lookups are SQLite
                        // point-reads and lag is observable downstream. If a
                        // future workload makes this hot, parallelize by
                        // dispatching translations to a bounded mpsc.
                        if let Some(event) = convert_domain_event(&envelope.event, &node_service).await {
                            yield Ok(NodeEvent {
                                event: Some(event),
                                database_id: database_id.clone(),
                            });
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        // The broadcast channel ring buffer overflowed. A slow
                        // client missed `skipped` events. Log and continue —
                        // dropping the stream on lag would be worse than the
                        // client briefly being out of sync, and `Lagged` is
                        // observable from the broadcast layer (not a bug).
                        tracing::warn!(skipped, "WatchNodes subscriber lagged; some events dropped");
                        continue;
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }

    type ChatStream = ReceiverStream<Result<ChatResponse, Status>>;

    async fn chat(
        &self,
        _request: Request<tonic::Streaming<ChatRequest>>,
    ) -> Result<Response<Self::ChatStream>, Status> {
        Err(Status::unimplemented(
            "Chat streaming is not yet implemented — tracked separately",
        ))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_properties(s: &str) -> Result<serde_json::Value, serde_json::Error> {
    if s.is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    serde_json::from_str(s)
}

fn properties_error(e: serde_json::Error) -> Status {
    Status::invalid_argument(format!("Invalid properties JSON: {}", e))
}

async fn fetch_node(service: &Arc<CoreNodeService>, node_id: &str) -> Result<Node, Status> {
    service
        .get_node(node_id)
        .await
        .map_err(service_error_to_status)?
        .ok_or_else(|| Status::not_found(format!("Node not found: {}", node_id)))
}

pub(crate) fn node_to_proto(node: Node) -> NodeData {
    NodeData {
        id: node.id,
        node_type: node.node_type,
        content: node.content,
        properties: node.properties.to_string(),
        version: node.version,
        lifecycle_status: node.lifecycle_status,
        created_at: node.created_at.to_rfc3339(),
        modified_at: node.modified_at.to_rfc3339(),
        markdown: String::new(),
    }
}

fn conflict_record_to_proto(record: nodespace_core::models::ConflictRecord) -> ConflictRecordProto {
    ConflictRecordProto {
        id: record.id,
        kind: record.kind.as_str().to_string(),
        node_ids: record.node_ids,
        detail: record.detail.to_string(),
        status: record.status.as_str().to_string(),
        detected_at: record.detected_at,
        detected_by: record.detected_by,
        occurrences: record.occurrences,
        last_seen_at: record.last_seen_at,
        resolved_at: record.resolved_at,
        resolution: record.resolution.map(|r| r.to_string()),
    }
}

/// Map the wire `NodeSortOrder` to the core `OrderBy` the store applies as a
/// SQL `ORDER BY`. `NODE_SORT_ORDER_UNSPECIFIED` — the zero value, also what
/// an unrecognized raw value decodes to — maps to a deterministic default
/// rather than "no order": `QueryNodesSimple` always sorts before paging, so
/// repeated calls with the same filter and advancing `offset` tile the full
/// matching set exactly once each instead of skipping or repeating rows at
/// page boundaries, which is exactly what an unordered `LIMIT`/`OFFSET`
/// could get wrong.
fn order_by_from_proto(raw: i32) -> OrderBy {
    match NodeSortOrder::try_from(raw) {
        Ok(NodeSortOrder::CreatedDesc) => OrderBy::CreatedDesc,
        Ok(NodeSortOrder::ModifiedAsc) => OrderBy::ModifiedAsc,
        Ok(NodeSortOrder::ModifiedDesc) => OrderBy::ModifiedDesc,
        Ok(NodeSortOrder::ContentAsc) => OrderBy::ContentAsc,
        Ok(NodeSortOrder::ContentDesc) => OrderBy::ContentDesc,
        Ok(NodeSortOrder::NodeTypeAsc) => OrderBy::NodeTypeAsc,
        Ok(NodeSortOrder::NodeTypeDesc) => OrderBy::NodeTypeDesc,
        Ok(NodeSortOrder::CreatedAsc) | Ok(NodeSortOrder::Unspecified) | Err(_) => {
            OrderBy::CreatedAsc
        }
    }
}

/// Translate a core `DomainEvent` into a proto `NodeEvent`.
///
/// Returns `None` for non-node events (relationships) — those are out of scope
/// for `WatchNodes` (a documented Non-Goal: relationship streaming is a
/// separate concern).
///
/// For `NodeCreated` and `NodeUpdated`, fetches the current node payload so
/// clients receive full node data inline and don't need a follow-up `GetNode`.
/// If the node has already been deleted by the time we look it up (a race
/// possible under concurrent mutations), the event is dropped — the next event
/// in the stream will be the corresponding `NodeDeleted`.
async fn convert_domain_event(
    event: &DomainEvent,
    node_service: &Arc<CoreNodeService>,
) -> Option<NodeEventKind> {
    match event {
        DomainEvent::NodeCreated { node_id, .. } => match node_service.get_node(node_id).await {
            Ok(Some(node)) => Some(NodeEventKind::Created(node_to_proto(node))),
            Ok(None) => {
                tracing::debug!(node_id = %node_id, "NodeCreated event skipped: node already gone");
                None
            }
            Err(e) => {
                tracing::warn!(node_id = %node_id, error = %e, "failed to fetch node for NodeCreated event");
                None
            }
        },
        DomainEvent::NodeUpdated { node, .. } => {
            Some(NodeEventKind::Updated(node_to_proto(node.clone())))
        }
        DomainEvent::NodeDeleted { id, node_type } => Some(NodeEventKind::Deleted(NodeDeleted {
            node_id: id.clone(),
            node_type: node_type.clone(),
        })),
        DomainEvent::RelationshipCreated { relationship } => Some(
            NodeEventKind::RelationshipCreated(relationship_to_proto(relationship)),
        ),
        DomainEvent::RelationshipUpdated { relationship } => Some(
            NodeEventKind::RelationshipUpdated(relationship_to_proto(relationship)),
        ),
        DomainEvent::RelationshipDeleted {
            id,
            from_id,
            to_id,
            relationship_type,
        } => Some(NodeEventKind::RelationshipDeleted(
            RelationshipDeletedPayload {
                id: id.clone(),
                from_id: from_id.clone(),
                to_id: to_id.clone(),
                relationship_type: relationship_type.clone(),
            },
        )),
        // ADR-069 §5/F16: not yet wired to a proto payload — this event
        // exists so a future gRPC/frontend consumer CAN be added (the
        // infrastructure gap the ADR calls out), not because `WatchNodes`
        // already surfaces it. Logged so it is at least visible in the
        // daemon's own logs until a wire type is added, same posture as the
        // NodeCreated fetch-failure arms above.
        DomainEvent::BackgroundImportFailed { root_id, error } => {
            tracing::warn!(
                root_id = %root_id,
                error = %error,
                "background import failed (not yet surfaced to WatchNodes clients)"
            );
            None
        }
    }
}

/// Translate a `RelationshipEvent` from the in-process domain channel
/// into the proto wire form. `properties` is JSON-encoded as a string
/// so the proto schema stays stable across additions to the
/// underlying `serde_json::Value` payload — the desktop watcher
/// re-parses it back to JSON before emitting the Tauri event.
fn relationship_to_proto(
    rel: &nodespace_core::db::events::RelationshipEvent,
) -> RelationshipPayload {
    RelationshipPayload {
        id: rel.id.clone(),
        from_id: rel.from_id.clone(),
        to_id: rel.to_id.clone(),
        relationship_type: rel.relationship_type.clone(),
        properties: rel.properties.to_string(),
    }
}

/// Find the currently-compiled seed template for `(node_type, seed_key)`
/// across every seed source the daemon seeds at startup (`seed_agent_nodes`
/// in `services/assembly.rs`). `seed_key` matches `NodeTemplate.title`
/// verbatim (see `prepare_nodes_from_template`, which stamps `_seed.key` from
/// it) — case-sensitive, no normalization.
fn resolve_seed_template(
    node_type: &str,
    seed_key: &str,
) -> Option<nodespace_core::markdown::NodeTemplate> {
    nodespace_agent::prompt_assembler::PromptAssembler::seed_agent_guidance_nodes()
        .into_iter()
        .chain(nodespace_agent::skill_pipeline::seed_skill_nodes())
        .chain(nodespace_agent::skill_pipeline::seed_tool_nodes())
        .find(|t| t.root_node_type == node_type && t.title == seed_key)
}

fn ops_error_to_status(err: OpsError) -> Status {
    match err {
        OpsError::NotFound { id } => Status::not_found(format!("Not found: {}", id)),
        OpsError::AlreadyExists { id } => Status::already_exists(format!("Already exists: {}", id)),
        OpsError::VersionConflict {
            node_id,
            expected,
            actual,
            current_node,
        } => {
            let message = format!(
                "Version conflict on {}: expected {}, got {}",
                node_id, expected, actual
            );
            let payload = serde_json::json!({
                "node_id": node_id,
                "expected": expected,
                "actual": actual,
                "current_node": current_node,
            });
            let mut status = Status::new(tonic::Code::Aborted, message);
            if let Ok(json) = serde_json::to_string(&payload) {
                if let Ok(val) =
                    json.parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
                {
                    status.metadata_mut().insert("x-version-conflict", val);
                }
            }
            status
        }
        OpsError::SubtreeAccessDenied { inaccessible_count } => {
            // Distinct from ordinary validation (INVALID_ARGUMENT): a cascade delete was
            // refused because the actor cannot read every node in the subtree (ADR-041).
            // FAILED_PRECONDITION + the count in metadata lets the frontend recognise the
            // refusal and show a dedicated modal. Mirrors the VersionConflict metadata
            // attachment above.
            let message = format!(
                "Delete refused: subtree contains {} node(s) not accessible to the current actor",
                inaccessible_count
            );
            let mut status = Status::failed_precondition(message);
            if let Ok(val) = inaccessible_count
                .to_string()
                .parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
            {
                status
                    .metadata_mut()
                    .insert("x-subtree-inaccessible-count", val);
            }
            status
        }
        OpsError::TreeTooLarge {
            node_id,
            count,
            max,
        } => {
            // RESOURCE_EXHAUSTED, not INTERNAL or INVALID_ARGUMENT: the request is
            // well-formed and the server is healthy, but this RPC has a stated
            // maximum document size the subtree exceeds. The count/max in metadata
            // let a caller report or log the concrete numbers without parsing the
            // message.
            let message = format!(
                "Subtree at node '{node_id}' has {count} nodes, exceeding the maximum of {max} supported by GetChildrenTree"
            );
            let mut status = Status::resource_exhausted(message);
            if let Ok(val) = count
                .to_string()
                .parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
            {
                status.metadata_mut().insert("x-tree-node-count", val);
            }
            if let Ok(val) = max
                .to_string()
                .parse::<tonic::metadata::MetadataValue<tonic::metadata::Ascii>>()
            {
                status.metadata_mut().insert("x-tree-node-max", val);
            }
            status
        }
        OpsError::ValidationFailed(msg) => {
            Status::invalid_argument(format!("Validation failed: {}", msg))
        }
        OpsError::InvalidParams(msg) => Status::invalid_argument(msg),
        OpsError::Internal(msg) => Status::internal(msg),
    }
}

fn service_error_to_status(err: NodeServiceError) -> Status {
    ops_error_to_status(OpsError::from(err))
}

fn markdown_error_to_status(err: nodespace_core::markdown::MarkdownError) -> Status {
    use nodespace_core::markdown::MarkdownError;
    match err {
        MarkdownError::InvalidParams(msg) => Status::invalid_argument(msg),
        MarkdownError::NotFound(msg) => Status::not_found(msg),
        MarkdownError::CreationFailed(msg) => Status::internal(msg),
        MarkdownError::Internal(msg) => Status::internal(msg),
    }
}

/// Build a `TaskNodeUpdate` from the proto's tri-state wrappers.
///
/// `OptionalStringClear`/`OptionalTimestampClear` encode the
/// Option<Option<T>> pattern: outer `None` ⇒ field unset on the wire, which we
/// surface as "no change". When the wrapper is present, `clear=true` writes
/// `Some(None)` (clear value) and `clear=false` writes `Some(Some(parsed))`.
fn build_task_node_update(
    status: Option<String>,
    priority: Option<OptionalStringClear>,
    due_date: Option<OptionalTimestampClear>,
    started_at: Option<OptionalTimestampClear>,
    completed_at: Option<OptionalTimestampClear>,
    content: Option<String>,
) -> Result<TaskNodeUpdate, String> {
    let status = match status {
        None => None,
        Some(s) => Some(
            serde_json::from_value::<TaskStatus>(serde_json::Value::String(s.clone()))
                .map_err(|e| format!("Invalid task status '{}': {}", s, e))?,
        ),
    };

    let priority = match priority {
        None => None,
        Some(w) if w.clear => Some(None),
        Some(w) => Some(Some(parse_task_priority(&w.value)?)),
    };

    let due_date = parse_optional_timestamp(due_date, "due_date")?;
    let started_at = parse_optional_timestamp(started_at, "started_at")?;
    let completed_at = parse_optional_timestamp(completed_at, "completed_at")?;

    Ok(TaskNodeUpdate {
        status,
        priority,
        due_date,
        started_at,
        completed_at,
        content,
    })
}

fn parse_task_priority(value: &str) -> Result<TaskPriority, String> {
    serde_json::from_value::<TaskPriority>(serde_json::Value::String(value.to_string()))
        .map_err(|e| format!("Invalid task priority '{}': {}", value, e))
}

fn parse_optional_timestamp(
    wrapper: Option<OptionalTimestampClear>,
    field_name: &str,
) -> Result<Option<Option<String>>, String> {
    match wrapper {
        None => Ok(None),
        Some(w) if w.clear => Ok(Some(None)),
        Some(w) => {
            let date_str = normalize_date_for_storage(&w.value).ok_or_else(|| {
                format!(
                    "Invalid date format for {}: '{}'. Expected YYYY-MM-DD or ISO8601",
                    field_name, w.value
                )
            })?;
            Ok(Some(Some(date_str)))
        }
    }
}

fn normalize_date_for_storage(s: &str) -> Option<String> {
    use chrono::NaiveDate;
    if NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return Some(s.to_string());
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.format("%Y-%m-%d").to_string());
    }
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Some(dt.format("%Y-%m-%d").to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_routing::DATABASE_ID_HEADER;
    use crate::services::database_manager::DatabaseManager;
    use crate::services::SharedContext;
    use nodespace_agent::pty::PtySessionManager;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::ops::node_ops;
    use nodespace_core::services::{
        CollectionService, EmbeddingScheduler, NodeService as CoreNodeService,
    };
    use nodespace_nlp_engine::EmbeddingService;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::watch;
    use tokio_stream::StreamExt;

    async fn make_service() -> (Arc<NodeServiceImpl>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await.unwrap());
        let core_svc = Arc::new(CoreNodeService::new(&mut store).await.unwrap());
        let svc = Arc::new(NodeServiceImpl::new(
            core_svc,
            Arc::new(tokio::sync::RwLock::new(None)),
            Arc::new(EmbeddingScheduler::new()),
        ));
        (svc, tmp)
    }

    #[tokio::test]
    async fn node_service_accessor_returns_the_same_underlying_service() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await.unwrap());
        let core_svc = Arc::new(CoreNodeService::new(&mut store).await.unwrap());
        let svc = NodeServiceImpl::new(
            core_svc.clone(),
            Arc::new(tokio::sync::RwLock::new(None)),
            Arc::new(EmbeddingScheduler::new()),
        );
        // The accessor must hand back the exact same NodeService (same allocation)
        // the impl serves, so a caller like the Pro daemon can bind cloud-sync to
        // it without opening a second store on the same file.
        assert!(Arc::ptr_eq(&svc.node_service(), &core_svc));
    }

    /// The FindDuplicate RPC surfaces an existing node on a
    /// uniqueness-flagged match (case-folded), and returns an EMPTY response
    /// (empty node_id, no node_data) — never NotFound, never an error — when there
    /// is no duplicate. That empty-not-error convention is the suggest-don't-block
    /// contract (ADR-065): "no duplicate" is an ordinary answer the caller acts on.
    #[tokio::test]
    async fn find_duplicate_rpc_returns_match_or_empty() {
        let (svc, _tmp) = make_service().await;

        let alice_id = svc
            .create_node(Request::new(crate::nodespace::CreateNodeRequest {
                id: None,
                node_type: "person".to_string(),
                content: "Alice".to_string(),
                parent_id: None,
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                properties: r#"{"person":{"first_name":"Alice","email":"alice@example.com"}}"#
                    .to_string(),
                position: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .node_id;

        // A colliding email (case-folded — person.email is unique_case_insensitive)
        // surfaces the existing person.
        let hit = svc
            .find_duplicate(Request::new(crate::nodespace::FindDuplicateRequest {
                node_type: "person".to_string(),
                field: "email".to_string(),
                value: "ALICE@example.com".to_string(),
                exclude_id: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            hit.node_id, alice_id,
            "a colliding email must surface the existing person"
        );
        assert!(hit.node_data.is_some());

        // A never-seen email → empty response, not an error.
        let miss = svc
            .find_duplicate(Request::new(crate::nodespace::FindDuplicateRequest {
                node_type: "person".to_string(),
                field: "email".to_string(),
                value: "nobody@example.com".to_string(),
                exclude_id: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(miss.node_id.is_empty(), "no duplicate → empty node_id");
        assert!(miss.node_data.is_none(), "no duplicate → no node_data");

        // exclude_id excludes the caller's own node from matching itself — the
        // fix for a real false-negative: a caller that checks AFTER its own
        // write has already landed the same value would otherwise match itself
        // and never see the real, other, existing duplicate.
        let self_excluded = svc
            .find_duplicate(Request::new(crate::nodespace::FindDuplicateRequest {
                node_type: "person".to_string(),
                field: "email".to_string(),
                value: "alice@example.com".to_string(),
                exclude_id: Some(alice_id.clone()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(
            self_excluded.node_id.is_empty(),
            "excluding the only holder of a value must report no duplicate, \
             not the excluded node itself"
        );

        // With a second, genuinely different person holding the same value,
        // excluding the first must still surface the second.
        let bob_id = svc
            .create_node(Request::new(crate::nodespace::CreateNodeRequest {
                id: None,
                node_type: "person".to_string(),
                content: "Bob".to_string(),
                parent_id: None,
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                properties: r#"{"person":{"first_name":"Bob","email":"alice@example.com"}}"#
                    .to_string(),
                position: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .node_id;
        let other_found = svc
            .find_duplicate(Request::new(crate::nodespace::FindDuplicateRequest {
                node_type: "person".to_string(),
                field: "email".to_string(),
                value: "alice@example.com".to_string(),
                exclude_id: Some(alice_id.clone()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            other_found.node_id, bob_id,
            "excluding Alice must still surface Bob as the real duplicate"
        );
    }

    /// ADR-068: the conflict-journal RPCs round-trip a `UniqueFieldCollision`
    /// record end to end through proto — `ListConflicts`, `ConflictsForNode`,
    /// and `ResolveConflict` — proving the wire conversion
    /// (`conflict_record_to_proto`), not just the already-covered core logic.
    #[tokio::test]
    async fn conflict_journal_rpcs_round_trip_a_unique_field_collision() {
        let (svc, _tmp) = make_service().await;

        let alice_id = svc
            .create_node(Request::new(crate::nodespace::CreateNodeRequest {
                id: None,
                node_type: "person".to_string(),
                content: "Alice".to_string(),
                parent_id: None,
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                properties: r#"{"person":{"first_name":"Alice","email":"alice@example.com"}}"#
                    .to_string(),
                position: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .node_id;

        // A colliding email journals a UniqueFieldCollision naming both.
        let bob_id = svc
            .create_node(Request::new(crate::nodespace::CreateNodeRequest {
                id: None,
                node_type: "person".to_string(),
                content: "Bob".to_string(),
                parent_id: None,
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                properties: r#"{"person":{"first_name":"Bob","email":"alice@example.com"}}"#
                    .to_string(),
                position: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .node_id;

        // ConflictsForNode: Alice's node names exactly one open record.
        let for_alice = svc
            .conflicts_for_node(Request::new(crate::nodespace::ConflictsForNodeRequest {
                node_id: alice_id.clone(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(for_alice.conflicts.len(), 1);
        let record = &for_alice.conflicts[0];
        assert_eq!(record.kind, "unique_field_collision");
        assert_eq!(record.status, "open");
        let mut node_ids = record.node_ids.clone();
        node_ids.sort();
        let mut expected = vec![alice_id.clone(), bob_id.clone()];
        expected.sort();
        assert_eq!(node_ids, expected);
        let detail: serde_json::Value = serde_json::from_str(&record.detail).unwrap();
        assert_eq!(detail["field"], "email");

        // ListConflicts with a kind filter surfaces the same record.
        let listed = svc
            .list_conflicts(Request::new(crate::nodespace::ListConflictsRequest {
                status: Some("open".to_string()),
                kind: Some("unique_field_collision".to_string()),
                limit: 0,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(listed.conflicts.iter().any(|c| c.id == record.id));

        // ResolveConflict(dismiss) transitions status and is reflected on
        // both a fresh ListConflicts and ConflictsForNode read.
        let resolved = svc
            .resolve_conflict(Request::new(crate::nodespace::ResolveConflictRequest {
                conflict_id: record.id.clone(),
                resolution: r#"{"action":"dismiss"}"#.to_string(),
            }))
            .await
            .unwrap()
            .into_inner();
        let resolved_record = resolved.conflict.expect("resolve must return the record");
        assert_eq!(resolved_record.status, "dismissed");

        let for_alice_after = svc
            .conflicts_for_node(Request::new(crate::nodespace::ConflictsForNodeRequest {
                node_id: alice_id,
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(for_alice_after.conflicts[0].status, "dismissed");
    }

    /// ADR-037: the RPC wiring for `GetLocalPerson` resolves the
    /// SAME seeded PersonNode a direct core query sees — proves the proto
    /// conversion (node_to_proto / OptionalNodeResponse) round-trips, not
    /// just the already-covered core logic.
    #[tokio::test]
    async fn get_local_person_rpc_resolves_the_seeded_owner() {
        let (svc, _tmp) = make_service().await;

        let seeded = svc
            .node_service
            .query_nodes_by_type("person", None)
            .await
            .unwrap();
        assert_eq!(seeded.len(), 1);
        let seeded_id = seeded[0].id.clone();

        let resp = svc
            .get_local_person(Request::new(crate::nodespace::Empty {}))
            .await
            .unwrap()
            .into_inner();
        let node = resp
            .node
            .expect("a freshly seeded database must resolve a local person");
        assert_eq!(node.node_id, seeded_id);
        assert_eq!(node.node_data.unwrap().content, "");
    }

    /// `SetLocalPersonIdentity` must write into the seeded node (proto round
    /// trip of the core write path already unit-tested directly on
    /// `NodeService::set_local_person_identity`).
    #[tokio::test]
    async fn set_local_person_identity_rpc_writes_to_the_seeded_node() {
        let (svc, _tmp) = make_service().await;

        let seeded = svc
            .node_service
            .query_nodes_by_type("person", None)
            .await
            .unwrap();
        let seeded_id = seeded[0].id.clone();

        let resp = svc
            .set_local_person_identity(Request::new(SetLocalPersonIdentityRequest {
                first_name: "Alice".to_string(),
                last_name: "Example".to_string(),
                email: "alice@example.com".to_string(),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            resp.node_id, seeded_id,
            "the write must land on the seeded node"
        );
        let data = resp.node_data.expect("update returns the updated node");
        let props: serde_json::Value = serde_json::from_str(&data.properties).unwrap();
        assert_eq!(props["person"]["first_name"], "Alice");
        assert_eq!(props["person"]["last_name"], "Example");
        assert_eq!(props["person"]["email"], "alice@example.com");

        // Still exactly one person afterward — no duplicate node was created.
        let people_after = svc
            .node_service
            .query_nodes_by_type("person", None)
            .await
            .unwrap();
        assert_eq!(people_after.len(), 1);
    }

    /// A model-less shared build context for constructing a `DatabaseManager`
    /// in tests (`has_model = false` skips all embedding wiring).
    fn test_context() -> SharedContext {
        let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
        SharedContext {
            pty_manager: Arc::new(PtySessionManager::new()),
            model,
            has_model: false,
            model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scheduler: Arc::new(EmbeddingScheduler::new()),
            subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
            local_agent: crate::SharedLocalAgent::new(
                crate::nodespace_dir()
                    .expect("nodespace dir")
                    .join("daemon.toml"),
            ),
        }
    }

    /// A request carrying an `x-ns-database-id` header is served by that
    /// database (ADR-053): a node created against a second database lands there
    /// and is invisible to the default, an absent header routes to the default,
    /// and an unregistered id is rejected rather than served from the default.
    #[tokio::test]
    async fn routes_requests_by_database_header() {
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(
            DatabaseManager::load(dir.path().join("databases.toml"), test_context())
                .await
                .unwrap(),
        );
        let default_id = manager
            .ensure_default_registered("Default".into(), dir.path().join("db1.db"))
            .await
            .unwrap();
        let db2 = manager
            .create("DB2".into(), Some(dir.path().join("db2.db")))
            .await
            .unwrap();

        // The registered service impl is the default database's, as the serve
        // loops wire it into `BaseServices`.
        let svc = manager
            .get_or_open(&default_id)
            .await
            .unwrap()
            .node_service_grpc
            .clone();

        // Create a node while targeting DB2 by header.
        let mut create = Request::new(crate::nodespace::CreateNodeRequest {
            id: None,
            node_type: "text".into(),
            content: "in-db2".into(),
            parent_id: None,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            properties: "{}".into(),
            position: None,
        });
        create.extensions_mut().insert(manager.clone());
        create
            .metadata_mut()
            .insert(DATABASE_ID_HEADER, db2.id.as_str().parse().unwrap());
        let node_id = svc.create_node(create).await.unwrap().into_inner().node_id;

        // Visible when the same DB2 header is supplied.
        let mut get_db2 = Request::new(crate::nodespace::GetNodeRequest {
            node_id: node_id.clone(),
        });
        get_db2.extensions_mut().insert(manager.clone());
        get_db2
            .metadata_mut()
            .insert(DATABASE_ID_HEADER, db2.id.as_str().parse().unwrap());
        assert!(svc.get_node(get_db2).await.is_ok());

        // Invisible to the default database (no header) — routing isolates DBs.
        let mut get_default = Request::new(crate::nodespace::GetNodeRequest {
            node_id: node_id.clone(),
        });
        get_default.extensions_mut().insert(manager.clone());
        assert_eq!(
            svc.get_node(get_default).await.unwrap_err().code(),
            tonic::Code::NotFound
        );

        // An unregistered database id is rejected, not silently served.
        let mut get_bad = Request::new(crate::nodespace::GetNodeRequest { node_id });
        get_bad.extensions_mut().insert(manager.clone());
        get_bad
            .metadata_mut()
            .insert(DATABASE_ID_HEADER, "ZZZ-UNREGISTERED".parse().unwrap());
        assert_eq!(
            svc.get_node(get_bad).await.unwrap_err().code(),
            tonic::Code::NotFound
        );
    }

    /// A request that names a database on a daemon without the routing
    /// middleware installed is rejected rather than silently served from the
    /// active database — silently answering would be a wrong-database read the
    /// client cannot detect.
    #[tokio::test]
    async fn database_header_without_routing_middleware_is_rejected() {
        let (svc, _tmp) = make_service().await;

        // Header-less requests keep working against the impl's own database.
        let plain = Request::new(crate::nodespace::GetNodeRequest {
            node_id: "no-such-node".into(),
        });
        assert_eq!(
            svc.get_node(plain).await.unwrap_err().code(),
            tonic::Code::NotFound
        );

        // A routing header with no manager injected must be rejected.
        let mut routed = Request::new(crate::nodespace::GetNodeRequest {
            node_id: "no-such-node".into(),
        });
        routed
            .metadata_mut()
            .insert(DATABASE_ID_HEADER, "SOME-DB-ID".parse().unwrap());
        let err = svc.get_node(routed).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
        assert!(
            err.message().contains("SOME-DB-ID"),
            "rejection must name the database the request targeted: {}",
            err.message()
        );
    }

    /// Creating a node via the gRPC handler (with collection + lifecycle_status) must
    /// produce the same persisted state as calling node_ops::create_node directly.
    #[tokio::test]
    async fn create_node_rpc_parity_with_node_ops() {
        let (svc, _tmp) = make_service().await;

        // --- via RPC handler ---
        let rpc_req = Request::new(crate::nodespace::CreateNodeRequest {
            id: None,
            node_type: "text".to_string(),
            content: "parity-test".to_string(),
            parent_id: None,
            collections: vec!["test-collection".to_string()],
            collection_ids: Vec::new(),
            lifecycle_status: Some("archived".to_string()),
            properties: "{}".to_string(),
            position: None,
        });
        let rpc_resp = svc.create_node(rpc_req).await.unwrap().into_inner();
        let rpc_node_id = rpc_resp.node_id.clone();

        // Assert collection membership was set (via dedicated API, not wire response)
        let store = svc.node_service.store();
        let coll_svc = CollectionService::new(store, &svc.node_service);
        let node_collections = coll_svc.get_node_collections(&rpc_node_id).await.unwrap();
        assert!(
            !node_collections.is_empty(),
            "RPC handler must add node to collection"
        );
        // Assert lifecycle_status is reflected in returned NodeData
        let rpc_node_data = rpc_resp.node_data.unwrap();
        assert_eq!(
            rpc_node_data.lifecycle_status, "archived",
            "RPC handler must apply lifecycle_status"
        );

        // --- via node_ops directly (same NodeService, fresh second node) ---
        let ops_input = node_ops::CreateNodeInput {
            id: None,
            node_type: "text".to_string(),
            content: "parity-test".to_string(),
            parent_id: None,
            position: nodespace_core::services::InsertPositionOwned::End,
            properties: serde_json::json!({}),
            collections: vec!["test-collection".to_string()],
            collection_ids: Vec::new(),
            lifecycle_status: Some("archived".to_string()),
        };
        let ops_output = node_ops::create_node(&svc.node_service, ops_input)
            .await
            .unwrap();

        // ops-layer output struct carries collection_ids directly (not the wire response,
        // which no longer exposes per-node collection_id; see NodeData proto).
        // This assertion tests the ops layer's own return contract, not the wire format.
        assert!(
            !ops_output.collection_ids.is_empty(),
            "ops must populate collection_ids"
        );
        let ops_node = svc
            .node_service
            .get_node(&ops_output.node_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ops_node.lifecycle_status, "archived");

        // Both nodes must be distinct (idempotency: each call creates a new node)
        assert_ne!(rpc_node_id, ops_output.node_id);
    }

    /// update_node via RPC with no version supplied must auto-fetch and not return VersionConflict.
    #[tokio::test]
    async fn update_node_rpc_auto_fetch_version() {
        let (svc, _tmp) = make_service().await;

        // Create a node first
        let create_req = Request::new(crate::nodespace::CreateNodeRequest {
            id: None,
            node_type: "text".to_string(),
            content: "original".to_string(),
            parent_id: None,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            properties: "{}".to_string(),
            position: None,
        });
        let created = svc.create_node(create_req).await.unwrap().into_inner();
        let node_id = created.node_id;

        // Update without sending version — handler must auto-fetch
        let update_req = Request::new(crate::nodespace::UpdateNodeRequest {
            node_id: node_id.clone(),
            content: Some("updated".to_string()),
            node_type: None,
            properties: None,
            version: None, // omit version — triggers auto-fetch path
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        });
        let updated = svc.update_node(update_req).await.unwrap().into_inner();
        assert_eq!(updated.node_data.unwrap().content, "updated");
    }

    /// update_node with add_to_collection then remove_from_collection must transition membership.
    #[tokio::test]
    async fn update_node_rpc_collection_add_then_remove() {
        let (svc, _tmp) = make_service().await;

        // Create a node
        let create_req = Request::new(crate::nodespace::CreateNodeRequest {
            id: None,
            node_type: "text".to_string(),
            content: "membership-test".to_string(),
            parent_id: None,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            properties: "{}".to_string(),
            position: None,
        });
        let created = svc.create_node(create_req).await.unwrap().into_inner();
        let node_id = created.node_id;

        // Add to collection via update
        let add_req = Request::new(crate::nodespace::UpdateNodeRequest {
            node_id: node_id.clone(),
            content: None,
            node_type: None,
            properties: None,
            version: None,
            add_to_collections: vec!["membership-coll".to_string()],
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        });
        svc.update_node(add_req).await.unwrap();

        // Verify membership via dedicated API (collection_id no longer in NodeResponse)
        let store = svc.node_service.store();
        let coll_svc = CollectionService::new(store, &svc.node_service);
        let members_before: Vec<String> = coll_svc.get_node_collections(&node_id).await.unwrap();
        assert!(
            !members_before.is_empty(),
            "node must be in collection after add"
        );
        let collection_id = members_before[0].clone();

        // Remove from collection via update
        let remove_req = Request::new(crate::nodespace::UpdateNodeRequest {
            node_id: node_id.clone(),
            content: None,
            node_type: None,
            properties: None,
            version: None,
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: vec![collection_id.clone()],
            lifecycle_status: None,
        });
        svc.update_node(remove_req).await.unwrap();

        let members_after: Vec<String> = coll_svc.get_node_collections(&node_id).await.unwrap();
        assert!(
            !members_after.contains(&collection_id),
            "node must not be in collection after remove"
        );
    }

    /// Deleting a missing node via RPC returns existed=false (idempotent).
    #[tokio::test]
    async fn delete_node_rpc_missing_node_returns_existed_false() {
        let (svc, _tmp) = make_service().await;

        let req = Request::new(crate::nodespace::DeleteNodeRequest {
            node_id: "nonexistent-node-id".to_string(),
            version: None,
        });
        let resp = svc.delete_node(req).await.unwrap().into_inner();
        assert!(!resp.existed);
    }

    #[tokio::test]
    async fn get_daemon_version_reports_the_daemon_crate_version() {
        let (svc, _tmp) = make_service().await;
        let resp = svc
            .get_daemon_version(Request::new(crate::nodespace::GetDaemonVersionRequest {}))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(resp.version, env!("CARGO_PKG_VERSION"));
        assert!(!resp.version.is_empty(), "daemon must report a version");
    }

    // -- Error mapping parity tests ------------------------------------------
    //
    // For each NodeServiceError variant: assert the resulting tonic::Status code.
    // These exercise the full NodeServiceError → OpsError → Status chain.

    fn to_status(err: NodeServiceError) -> tonic::Status {
        service_error_to_status(err)
    }

    #[test]
    fn error_mapping_node_not_found_returns_not_found() {
        let s = to_status(NodeServiceError::node_not_found("abc"));
        assert_eq!(s.code(), tonic::Code::NotFound);
    }

    #[test]
    fn error_mapping_version_conflict_returns_aborted() {
        // Tests the service_error_to_status (From<NodeServiceError>) path — i.e. the
        // generic conversion used for non-task-node service calls. current_node is null
        // because no get_node() call precedes the conversion.
        // The task-node RPC handler intercepts VersionConflict earlier and embeds
        // current_node before calling ops_error_to_status; see
        // update_task_node_version_conflict_embeds_current_node.
        let s = to_status(NodeServiceError::version_conflict("n1", 3, 5));
        assert_eq!(s.code(), tonic::Code::Aborted);
        // The payload must be present so the Tauri command can surface it.
        let header = s
            .metadata()
            .get("x-version-conflict")
            .expect("x-version-conflict header missing");
        let json: serde_json::Value = serde_json::from_str(header.to_str().unwrap()).unwrap();
        assert_eq!(json["node_id"], "n1");
        assert_eq!(json["expected"], 3);
        assert_eq!(json["actual"], 5);
        // current_node is null for the generic From<NodeServiceError> conversion path.
        assert_eq!(json["current_node"], serde_json::Value::Null);
    }

    /// update_task_node RPC handler must embed current_node in the x-version-conflict
    /// header so the frontend can hydrate without a second round-trip.
    ///
    /// Scenario:
    ///  1. Create a task node at version 1.
    ///  2. Update it directly via the core service (version 1 → 2), simulating a
    ///     concurrent remote write.
    ///  3. Attempt UpdateTaskNode via the RPC handler with the old version 1 — must
    ///     return Aborted with x-version-conflict whose current_node is non-null.
    #[tokio::test]
    async fn update_task_node_version_conflict_embeds_current_node() {
        let (svc, _tmp) = make_service().await;

        // Use a fixed UUID so we can reference it by ID in subsequent calls.
        let task_id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";

        // Step 1: Create a task node via the RPC handler.
        let create_req = Request::new(crate::nodespace::CreateNodeRequest {
            id: Some(task_id.to_string()),
            node_type: "task".to_string(),
            content: "Initial task".to_string(),
            parent_id: None,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            properties: "{}".to_string(),
            position: None,
        });
        svc.create_node(create_req).await.unwrap();

        // Step 2: Advance the version by updating the node directly through the core
        // service (simulating a concurrent remote writer at version 1).
        svc.node_service
            .update_node(
                task_id,
                1, // version 1 → 2
                nodespace_core::models::NodeUpdate {
                    content: Some("Remote concurrent edit".to_string()),
                    node_type: None,
                    properties: None,
                    title: None,
                    lifecycle_status: None,
                },
            )
            .await
            .unwrap();

        // Step 3: Attempt UpdateTaskNode with the stale version 1.
        let conflict_req = Request::new(crate::nodespace::UpdateTaskNodeRequest {
            node_id: task_id.to_string(),
            version: 1, // stale — backend is at version 2
            status: Some("done".to_string()),
            priority: None,
            due_date: None,
            started_at: None,
            completed_at: None,
            content: None,
            properties: None,
        });

        let err = svc
            .update_task_node(conflict_req)
            .await
            .expect_err("expected VersionConflict error");

        // Must return Aborted.
        assert_eq!(
            err.code(),
            tonic::Code::Aborted,
            "expected Aborted status code"
        );

        // Must carry x-version-conflict header.
        let header = err
            .metadata()
            .get("x-version-conflict")
            .expect("x-version-conflict header missing for task-node OCC");
        let json: serde_json::Value = serde_json::from_str(header.to_str().unwrap()).unwrap();

        assert_eq!(json["node_id"], task_id);
        assert_eq!(json["expected"], 1);
        // NOTE: task-node update_task_node service sets actual_version=0 ("unknown")
        // because the SQLite transaction abort does not surface the winner's version.
        // The authoritative current state is carried in current_node instead.
        assert_eq!(json["actual"], 0);

        // current_node must be non-null — the handler fetches it before returning.
        assert!(
            !json["current_node"].is_null(),
            "task-node VersionConflict must embed current_node (got null)"
        );
        // The embedded node must reflect the remote write.
        assert_eq!(
            json["current_node"]["content"], "Remote concurrent edit",
            "current_node must reflect the winning remote write"
        );
        // current_node.version must be >= 2 (the winning concurrent write incremented it).
        let embedded_version = json["current_node"]["version"]
            .as_i64()
            .expect("current_node.version must be an integer");
        assert!(
            embedded_version >= 2,
            "current_node must carry the post-conflict version (got {})",
            embedded_version
        );
    }

    /// A generic-path (non-task) VersionConflict must embed `current_node` in the
    /// FLATTENED wire shape the frontend's typed converters expect — the same shape
    /// every successful read/write returns. An ai-chat node whose `turnStatus` and
    /// `messages` sit under `properties["ai-chat"]` instead of at the top level
    /// hydrates the store with a node whose `turnStatus` is undefined, which
    /// strands the viewer's typing indicator after a conflict.
    #[tokio::test]
    async fn update_node_version_conflict_embeds_flattened_current_node() {
        let (svc, _tmp) = make_service().await;

        let chat_id = "b1b2c3d4-e5f6-7890-abcd-ef1234567890";

        let create_req = Request::new(crate::nodespace::CreateNodeRequest {
            id: Some(chat_id.to_string()),
            node_type: "ai-chat".to_string(),
            content: String::new(),
            parent_id: None,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            properties: serde_json::json!({
                "ai-chat": { "turn_status": "processing", "messages": [] }
            })
            .to_string(),
            position: None,
        });
        svc.create_node(create_req).await.unwrap();

        // Concurrent winner: daemon completes the turn and writes turn_status idle.
        svc.node_service
            .update_node(
                chat_id,
                1,
                nodespace_core::models::NodeUpdate {
                    content: None,
                    node_type: None,
                    properties: Some(serde_json::json!({
                        "ai-chat": {
                            "turn_status": "idle",
                            "messages": [{ "role": "assistant", "content": "hi" }]
                        }
                    })),
                    title: None,
                    lifecycle_status: None,
                },
            )
            .await
            .unwrap();

        // Stale frontend write at version 1.
        let conflict_req = Request::new(crate::nodespace::UpdateNodeRequest {
            node_id: chat_id.to_string(),
            version: Some(1),
            node_type: None,
            content: None,
            properties: Some(
                serde_json::json!({ "ai-chat": { "turn_status": "processing" } }).to_string(),
            ),
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        });
        let err = svc
            .update_node(conflict_req)
            .await
            .expect_err("expected VersionConflict error");

        assert_eq!(err.code(), tonic::Code::Aborted);
        let header = err
            .metadata()
            .get("x-version-conflict")
            .expect("x-version-conflict header missing");
        let json: serde_json::Value = serde_json::from_str(header.to_str().unwrap()).unwrap();

        let current = &json["current_node"];
        assert!(!current.is_null(), "current_node must be embedded");

        // The frontend reads these at the TOP level (AiChatNode is a flat shape).
        assert_eq!(
            current["turnStatus"], "idle",
            "current_node must carry flattened top-level turnStatus (got {current})"
        );
        assert!(
            current["messages"].is_array(),
            "current_node must carry flattened top-level messages (got {current})"
        );
    }

    #[test]
    fn error_mapping_version_conflict_from_move_children_batch_returns_aborted() {
        // move_children_to_parent surfaces a VersionConflict for the first offender;
        // the daemon must map it to Aborted so the frontend recognises it as a
        // child-transfer-failure (identical pipeline to single MoveNode conflicts).
        let s = to_status(NodeServiceError::version_conflict("child-k", 1, 5));
        assert_eq!(s.code(), tonic::Code::Aborted);
    }

    #[test]
    fn error_mapping_subtree_access_denied_returns_failed_precondition() {
        // A cascade delete refused by the ADR-041 access gate must map to a DISTINCT
        // status (FAILED_PRECONDITION, not INVALID_ARGUMENT) carrying the inaccessible
        // count in metadata, so the frontend can tell a refusal apart from ordinary
        // validation and surface a dedicated modal.
        let s = to_status(NodeServiceError::subtree_access_denied(3));
        assert_eq!(s.code(), tonic::Code::FailedPrecondition);
        let header = s
            .metadata()
            .get("x-subtree-inaccessible-count")
            .expect("x-subtree-inaccessible-count header missing");
        assert_eq!(header.to_str().unwrap(), "3");
    }

    #[test]
    fn error_mapping_tree_too_large_returns_resource_exhausted() {
        // GetChildrenTree refusing an oversized subtree must surface as a distinct,
        // actionable status (RESOURCE_EXHAUSTED) carrying the concrete count/max in
        // metadata — not the opaque transport-level OutOfRange a caller would
        // otherwise see if the oversized JSON were sent and then failed to decode.
        let s = to_status(NodeServiceError::tree_too_large("root-1", 20_001, 20_000));
        assert_eq!(s.code(), tonic::Code::ResourceExhausted);
        assert!(s.message().contains("root-1"));
        assert!(s.message().contains("20001") || s.message().contains("20,001"));
        let count_header = s
            .metadata()
            .get("x-tree-node-count")
            .expect("x-tree-node-count header missing");
        assert_eq!(count_header.to_str().unwrap(), "20001");
        let max_header = s
            .metadata()
            .get("x-tree-node-max")
            .expect("x-tree-node-max header missing");
        assert_eq!(max_header.to_str().unwrap(), "20000");
    }

    #[test]
    fn error_mapping_validation_failed_returns_invalid_argument() {
        use nodespace_core::models::ValidationError;
        let s = to_status(NodeServiceError::ValidationFailed(
            ValidationError::MissingField("x".into()),
        ));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn error_mapping_invalid_parent_returns_invalid_argument() {
        let s = to_status(NodeServiceError::invalid_parent("p1"));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn error_mapping_invalid_root_returns_invalid_argument() {
        let s = to_status(NodeServiceError::invalid_root("r1"));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn error_mapping_circular_reference_returns_invalid_argument() {
        let s = to_status(NodeServiceError::circular_reference("A→B→A"));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn error_mapping_hierarchy_violation_returns_invalid_argument() {
        let s = to_status(NodeServiceError::hierarchy_violation("root immutable"));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    /// A cycle the *caller proposed* (moving a node under its own descendant)
    /// stays InvalidArgument: the request is genuinely bad and a different
    /// request fixes it. Pinned alongside the case below so the two cannot
    /// silently collapse into one classification.
    #[test]
    fn error_mapping_caller_proposed_cycle_stays_invalid_argument() {
        assert_eq!(
            to_status(NodeServiceError::circular_reference("A→B→A")).code(),
            tonic::Code::InvalidArgument,
            "write-path cycle rejection is a caller error"
        );
        assert_eq!(
            to_status(NodeServiceError::hierarchy_violation("root immutable")).code(),
            tonic::Code::InvalidArgument,
            "write-path hierarchy rejection is a caller error"
        );
    }

    /// A cycle found while *reading* stored data is a server-side fault: the
    /// request was well-formed, retrying it unchanged fails identically, and
    /// only repairing the graph resolves it. Reporting it as InvalidArgument
    /// would blame the client and keep the corruption out of server-error
    /// reporting.
    #[test]
    fn error_mapping_corrupt_hierarchy_returns_internal() {
        let s = to_status(NodeServiceError::corrupt_hierarchy(
            "node 'n1' appears more than once on the same branch",
        ));
        assert_eq!(s.code(), tonic::Code::Internal);
        assert!(
            s.message().contains("n1"),
            "message should name the offending node: {}",
            s.message()
        );
    }

    #[test]
    fn error_mapping_invalid_update_returns_invalid_argument() {
        let s = to_status(NodeServiceError::invalid_update("cannot change type"));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn error_mapping_invalid_collection_path_returns_invalid_argument() {
        let s = to_status(NodeServiceError::invalid_collection_path("empty segment"));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn error_mapping_collection_cycle_returns_invalid_argument() {
        let s = to_status(NodeServiceError::collection_cycle("A→B→A"));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn error_mapping_collection_depth_exceeded_returns_invalid_argument() {
        let s = to_status(NodeServiceError::collection_depth_exceeded(
            "a:b:c:d:e:f",
            5,
        ));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn error_mapping_playbook_validation_failed_returns_invalid_argument() {
        let s = to_status(NodeServiceError::PlaybookValidationFailed {
            errors: "bad rule".into(),
        });
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn error_mapping_collection_not_found_returns_not_found() {
        let s = to_status(NodeServiceError::collection_not_found("inbox"));
        assert_eq!(s.code(), tonic::Code::NotFound);
    }

    #[test]
    fn error_mapping_database_error_returns_internal() {
        use nodespace_core::db::DatabaseError;
        let s = to_status(NodeServiceError::DatabaseError(
            DatabaseError::initialization_failed("conn lost"),
        ));
        assert_eq!(s.code(), tonic::Code::Internal);
    }

    #[test]
    fn error_mapping_transaction_failed_returns_internal() {
        let s = to_status(NodeServiceError::transaction_failed("commit failed"));
        assert_eq!(s.code(), tonic::Code::Internal);
    }

    #[test]
    fn error_mapping_serialization_error_returns_internal() {
        let s = to_status(NodeServiceError::serialization_error("bad json"));
        assert_eq!(s.code(), tonic::Code::Internal);
    }

    #[test]
    fn error_mapping_query_failed_returns_internal() {
        let s = to_status(NodeServiceError::query_failed("syntax error"));
        assert_eq!(s.code(), tonic::Code::Internal);
    }

    #[test]
    fn error_mapping_bulk_operation_failed_returns_internal() {
        let s = to_status(NodeServiceError::bulk_operation_failed("3 of 5 failed"));
        assert_eq!(s.code(), tonic::Code::Internal);
    }

    #[test]
    fn error_mapping_initialization_error_returns_internal() {
        let s = to_status(NodeServiceError::initialization_error("db unavailable"));
        assert_eq!(s.code(), tonic::Code::Internal);
    }

    #[test]
    fn error_mapping_not_a_container_returns_invalid_argument() {
        let s = to_status(NodeServiceError::not_a_container("parent-id", "query"));
        assert_eq!(s.code(), tonic::Code::InvalidArgument);
    }

    // -----------------------------------------------------------------------
    // Same-origin echo suppression (ADR-026 C5 extension)
    //
    // A gRPC connection tags its writes with `x-ns-client-id` (scoped through
    // `NodeService::with_client()` in `route()`) and tags its `WatchNodes`
    // subscription with the same header. The daemon — not the frontend's old
    // content-comparison heuristic — is the sole authority on "is this my own
    // write echoed back": `watch_nodes` drops any envelope whose
    // `source_client_id` matches the subscriber's own id.
    // -----------------------------------------------------------------------

    /// Open a `WatchNodes` stream tagged with `subscriber_client_id` (or
    /// untagged if `None`), and return it pinned so the caller can pull the
    /// next few events with a timeout.
    async fn watch_as(
        svc: &Arc<NodeServiceImpl>,
        subscriber_client_id: Option<&str>,
    ) -> std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<NodeEvent, Status>> + Send>> {
        let mut req = Request::new(WatchRequest::default());
        if let Some(id) = subscriber_client_id {
            req.metadata_mut()
                .insert(nodespace_proto::CLIENT_ID_HEADER, id.parse().unwrap());
        }
        svc.watch_nodes(req).await.unwrap().into_inner()
    }

    /// Pull the next event off a `WatchNodes` stream, or `None` if none
    /// arrives within the timeout — used to assert an echo was suppressed
    /// (absence, not just "some other event came first").
    async fn next_event(
        stream: &mut (impl tokio_stream::Stream<Item = Result<NodeEvent, Status>> + Unpin),
    ) -> Option<NodeEvent> {
        tokio::time::timeout(std::time::Duration::from_millis(500), stream.next())
            .await
            .ok()
            .flatten()
            .map(|r| r.unwrap())
    }

    /// A write made through a connection tagged `x-ns-client-id: alice` is not
    /// delivered back to alice's own `WatchNodes` subscription (same id on
    /// both), but IS delivered to a subscription tagged with a different id
    /// (bob) — genuine foreign-write delivery is unaffected.
    #[tokio::test]
    async fn watch_nodes_suppresses_own_echo_but_delivers_to_other_clients() {
        let (svc, _tmp) = make_service().await;

        let mut alice_stream = watch_as(&svc, Some("alice")).await;
        let mut bob_stream = watch_as(&svc, Some("bob")).await;

        let mut create = Request::new(crate::nodespace::CreateNodeRequest {
            id: None,
            node_type: "text".into(),
            content: "alice's node".into(),
            parent_id: None,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            properties: "{}".into(),
            position: None,
        });
        create
            .metadata_mut()
            .insert(nodespace_proto::CLIENT_ID_HEADER, "alice".parse().unwrap());
        svc.create_node(create).await.unwrap();

        // Bob (a different client) sees the create.
        let bob_event = next_event(&mut bob_stream)
            .await
            .expect("a different client must see a foreign write");
        assert!(matches!(bob_event.event, Some(NodeEventKind::Created(_))));

        // Alice does not see her own write echoed back.
        assert!(
            next_event(&mut alice_stream).await.is_none(),
            "the writer's own WatchNodes subscription must not see its own echo"
        );
    }

    /// A subscriber with no `x-ns-client-id` header suppresses nothing —
    /// matches pre-existing behavior for callers that never adopt the header.
    #[tokio::test]
    async fn watch_nodes_without_client_id_header_suppresses_nothing() {
        let (svc, _tmp) = make_service().await;

        let mut anon_stream = watch_as(&svc, None).await;

        let mut create = Request::new(crate::nodespace::CreateNodeRequest {
            id: None,
            node_type: "text".into(),
            content: "untagged write".into(),
            parent_id: None,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            properties: "{}".into(),
            position: None,
        });
        // Written through a client-id-tagged connection, but the anonymous
        // subscriber has no id of its own to match against.
        create
            .metadata_mut()
            .insert(nodespace_proto::CLIENT_ID_HEADER, "alice".parse().unwrap());
        svc.create_node(create).await.unwrap();

        assert!(
            next_event(&mut anon_stream).await.is_some(),
            "a subscriber with no client id must still see every write"
        );
    }

    /// A write made through a connection with no `x-ns-client-id` header is
    /// never suppressed on any subscription, including one tagged with an id —
    /// only an exact id-to-id match suppresses.
    #[tokio::test]
    async fn watch_nodes_untagged_write_is_delivered_to_tagged_subscriber() {
        let (svc, _tmp) = make_service().await;

        let mut alice_stream = watch_as(&svc, Some("alice")).await;

        // No x-ns-client-id header on this write.
        let create = Request::new(crate::nodespace::CreateNodeRequest {
            id: None,
            node_type: "text".into(),
            content: "anonymous write".into(),
            parent_id: None,
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            properties: "{}".into(),
            position: None,
        });
        svc.create_node(create).await.unwrap();

        assert!(
            next_event(&mut alice_stream).await.is_some(),
            "an untagged write must still reach a client-id-tagged subscriber"
        );
    }

    // -----------------------------------------------------------------------
    // WatchNodes teardown (zombie-stream cleanup + scheduler active-id release)
    // -----------------------------------------------------------------------

    /// A `WatchNodes` stream must actually END when its database's teardown
    /// signal fires, not survive as a zombie parked forever on `rx.recv()`.
    /// This is the exact failure this test guards: the stream's own
    /// `node_service` clone owns the event sender `rx` reads from, so absent
    /// the shutdown-token select, cancelling the token would have no effect
    /// at all on this stream and it would hang — indistinguishable from a
    /// database that's simply quiet.
    #[tokio::test]
    async fn watch_nodes_stream_ends_when_shutdown_token_is_cancelled() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await.unwrap());
        let core_svc = Arc::new(CoreNodeService::new(&mut store).await.unwrap());
        let shutdown_token = tokio_util::sync::CancellationToken::new();
        let svc = Arc::new(
            NodeServiceImpl::new(
                core_svc,
                Arc::new(tokio::sync::RwLock::new(None)),
                Arc::new(EmbeddingScheduler::new()),
            )
            .with_database_id("db-a".into())
            .with_shutdown_token(shutdown_token.clone()),
        );

        let mut stream = watch_as(&svc, None).await;

        // Simulate the database being torn down (idle eviction, deliberate
        // close, daemon shutdown) — exactly what `DatabaseServices::shutdown`
        // does.
        shutdown_token.cancel();

        let next = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect(
                "the stream must end promptly once its database is torn down, \
                 not hang forever as a zombie",
            );
        assert!(
            next.is_none(),
            "a cancelled shutdown token must end the WatchNodes stream cleanly (no more items)"
        );
    }

    /// Opening a `WatchNodes` stream marks its database active (ADR-053); that
    /// claim must be retracted once the stream ends — here, by the caller
    /// simply dropping it (e.g. a client disconnect), which is the path a
    /// `break` at the bottom of the loop can never cover. Without the RAII
    /// guard, a database that silently lost its only stream would stay
    /// scheduler-active, and therefore idle-eviction-immune, forever.
    #[tokio::test]
    async fn watch_nodes_clears_active_database_when_stream_is_dropped() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await.unwrap());
        let core_svc = Arc::new(CoreNodeService::new(&mut store).await.unwrap());
        let scheduler = Arc::new(EmbeddingScheduler::new());
        let svc = Arc::new(
            NodeServiceImpl::new(
                core_svc,
                Arc::new(tokio::sync::RwLock::new(None)),
                scheduler.clone(),
            )
            .with_database_id("db-a".into()),
        );

        let mut stream = watch_as(&svc, None).await;
        assert!(
            scheduler.is_active("db-a"),
            "opening the stream must mark its database active"
        );

        // Drive the stream generator to its first suspension point (parked in
        // the `tokio::select!`) so the RAII guard inside it actually gets
        // constructed — an unpolled `async_stream::stream!` never runs any of
        // its body, active-guard construction included.
        assert!(
            next_event(&mut stream).await.is_none(),
            "no write happened; nothing should have arrived"
        );

        drop(stream);

        assert!(
            !scheduler.is_active("db-a"),
            "dropping the stream must retract its active-database claim, not leave the \
             database stuck eviction-immune forever"
        );
    }
}
