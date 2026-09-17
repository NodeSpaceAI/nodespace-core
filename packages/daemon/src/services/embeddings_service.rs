//! tonic `EmbeddingsService` implementation backed by `nodespace-core`.
//!
//! Wraps `NodeEmbeddingService` and `EmbeddingProcessor`. The GPU drain
//! protocol (`release_gpu_context`) is handled on daemon shutdown, not by
//! any individual RPC caller.
//!
//! The embedding model loads asynchronously after the socket is bound. While
//! loading, all RPCs return `UNAVAILABLE` with a descriptive message; if the
//! load permanently fails, they return `FAILED_PRECONDITION` instead so a
//! polling caller knows to stop retrying. Once loaded, they work normally
//! without any client reconnect.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use nodespace_core::models::EmbeddingConfig;
use nodespace_core::ops::search_ops::{self, SearchSemanticInput};
use nodespace_core::services::{EmbeddingProcessor, NodeEmbeddingService, NodeService};
use tokio::sync::RwLock;
use tonic::{Request, Response, Status};

use crate::nodespace::{
    embeddings_service_server::EmbeddingsService as GrpcEmbeddingsService, BatchEmbeddingFailure,
    BatchQueueEmbeddingsRequest, BatchQueueEmbeddingsResponse, EmbeddingStatusResponse,
    GetEmbeddingStatusRequest, GetStaleCountRequest, GetStaleCountResponse, QueueEmbeddingRequest,
    QueueEmbeddingResponse, RegenerateEmbeddingRequest, RegenerateEmbeddingResponse,
    SearchSemanticRequest, SearchSemanticResponse, TriggerBatchEmbedRequest,
    TriggerBatchEmbedResponse,
};
use crate::services::node_service::{nodes_to_proto, ops_error_to_status};

/// Live embedding state once the model has finished loading.
pub struct EmbeddingReady {
    pub embedding_service: Arc<NodeEmbeddingService>,
    pub processor: Arc<EmbeddingProcessor>,
}

#[derive(Clone)]
pub struct EmbeddingsServiceImpl {
    node_service: Arc<NodeService>,
    /// `None` while the model is still loading; populated by the background task.
    state: Arc<RwLock<Option<EmbeddingReady>>>,
    /// Set once, permanently, if the shared background load fails. `state`
    /// alone cannot distinguish "still loading" from "failed, will never
    /// complete" -- both leave it `None` forever -- so `unavailable()` reads
    /// this to decide which of the two an RPC caller is actually looking at.
    load_failed: Arc<AtomicBool>,
}

impl EmbeddingsServiceImpl {
    pub fn new(
        node_service: Arc<NodeService>,
        state: Arc<RwLock<Option<EmbeddingReady>>>,
        load_failed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            node_service,
            state,
            load_failed,
        }
    }

    /// `UNAVAILABLE` (a status gRPC clients conventionally treat as safe to
    /// retry) while the model is still loading; `FAILED_PRECONDITION` (not
    /// safe to retry) once the load has permanently failed -- so a client
    /// polling this actually gets to stop polling instead of retrying a load
    /// that will never happen.
    fn unavailable(&self) -> Status {
        if self.load_failed.load(Ordering::SeqCst) {
            Status::failed_precondition(
                "embedding model failed to load — semantic search unavailable",
            )
        } else {
            Status::unavailable("embedding model loading, please retry")
        }
    }

    /// Resolve which database this request targets (ADR-053) and return that
    /// database's embeddings service. The routing contract lives in
    /// [`crate::db_routing::routed_database_services`]: a header selects a
    /// registered database, header-less requests hit the default, and with no
    /// routing middleware installed a header-less request falls back to `self`
    /// while a header-carrying one is rejected rather than silently served from
    /// the active database.
    async fn route<T>(&self, request: &Request<T>) -> Result<EmbeddingsServiceImpl, Status> {
        match crate::db_routing::routed_database_services(request).await? {
            // The embedding model is process-global, so a registered
            // EmbeddingsService implies every open database has one; if the
            // target somehow has none, answering from `self` would silently
            // serve another database, so fail instead.
            Some(services) => services
                .embeddings_service_grpc
                .clone()
                .ok_or_else(|| Status::internal("the target database has no embeddings service")),
            None => Ok(self.clone()),
        }
    }

    /// Shared implementation for stale-count queries used by both
    /// `get_embedding_status` and `get_stale_count` to avoid duplication.
    async fn stale_count_inner(&self) -> Result<i32, Status> {
        let ids = self
            .node_service
            .store()
            .get_stale_embedding_root_ids(None, 0, EmbeddingConfig::default().max_retries)
            .await
            .map_err(|e| Status::internal(format!("Failed to get stale count: {}", e)))?;
        Ok(i32::try_from(ids.len()).unwrap_or(i32::MAX))
    }
}

#[tonic::async_trait]
impl GrpcEmbeddingsService for EmbeddingsServiceImpl {
    async fn get_embedding_status(
        &self,
        request: Request<GetEmbeddingStatusRequest>,
    ) -> Result<Response<EmbeddingStatusResponse>, Status> {
        let this = self.route(&request).await?;
        let available = this.state.read().await.is_some();
        let stale_count = if available {
            this.stale_count_inner().await?
        } else {
            0
        };
        Ok(Response::new(EmbeddingStatusResponse {
            available,
            stale_count,
        }))
    }

    async fn search_semantic(
        &self,
        request: Request<SearchSemanticRequest>,
    ) -> Result<Response<SearchSemanticResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        if req.query.trim().is_empty() {
            return Err(Status::invalid_argument("query cannot be empty"));
        }

        let guard = this.state.read().await;
        let state = guard.as_ref().ok_or_else(|| this.unavailable())?;
        let embedding_service = Arc::clone(&state.embedding_service);

        // Route through the same guarded op the NodeService.SearchNodes RPC and
        // the agent tool-calling loop use, rather than reaching into
        // `store.search_embeddings` directly. That direct call was a second,
        // parallel implementation of semantic search that silently skipped
        // enumerate-query handling and the ADR-029 archived/scope filtering the
        // guarded path applies, so the desktop GUI — its only caller — got
        // different results from every other caller for the same query.
        let input = SearchSemanticInput {
            query: req.query,
            // `None` here means "use the op's default", which is now reachable
            // separately from an explicit 0.0 (admit every match) thanks to the
            // optional wire field.
            threshold: req.threshold,
            limit: if req.limit == 0 {
                None
            } else {
                Some(req.limit as usize)
            },
            collection_id: None,
            collection: None,
            exclude_collections: None,
            // The GUI renders result rows from node fields alone, so attaching
            // subtree markdown would be per-result subtree reads nothing displays.
            include_markdown: Some(0),
            include_archived: None,
            scope: req.scope,
            node_types: None,
            property_filters: None,
            include_edges: None,
            graph_boost: None,
            include_title_matches: Some(req.include_title_matches),
        };

        let output = search_ops::search_semantic(&this.node_service, &embedding_service, input)
            .await
            .map_err(ops_error_to_status)?;

        // `matched_nodes` is the same ranked set as `output.nodes`, already
        // hydrated by search — mapping it straight onto the wire type avoids a
        // re-read per result.
        let nodes = nodes_to_proto(&this.node_service, output.matched_nodes).await?;

        Ok(Response::new(SearchSemanticResponse { nodes }))
    }

    async fn regenerate_embedding(
        &self,
        request: Request<RegenerateEmbeddingRequest>,
    ) -> Result<Response<RegenerateEmbeddingResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let guard = this.state.read().await;
        let state = guard.as_ref().ok_or_else(|| this.unavailable())?;

        let node = this
            .node_service
            .get_node(&req.node_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to get node: {}", e)))?
            .ok_or_else(|| Status::not_found(format!("Node not found: {}", req.node_id)))?;

        state
            .embedding_service
            .queue_for_embedding(&node.id)
            .await
            .map_err(|e| Status::internal(format!("Failed to queue embedding: {}", e)))?;

        Ok(Response::new(RegenerateEmbeddingResponse {}))
    }

    async fn queue_embedding(
        &self,
        request: Request<QueueEmbeddingRequest>,
    ) -> Result<Response<QueueEmbeddingResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let guard = this.state.read().await;
        let state = guard.as_ref().ok_or_else(|| this.unavailable())?;

        let node = this
            .node_service
            .get_node(&req.node_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to get node: {}", e)))?
            .ok_or_else(|| Status::not_found(format!("Node not found: {}", req.node_id)))?;

        state
            .embedding_service
            .queue_for_embedding(&node.id)
            .await
            .map_err(|e| Status::internal(format!("Failed to queue embedding: {}", e)))?;

        Ok(Response::new(QueueEmbeddingResponse {}))
    }

    async fn trigger_batch_embed(
        &self,
        request: Request<TriggerBatchEmbedRequest>,
    ) -> Result<Response<TriggerBatchEmbedResponse>, Status> {
        let this = self.route(&request).await?;
        let guard = this.state.read().await;
        let state = guard.as_ref().ok_or_else(|| this.unavailable())?;

        state
            .processor
            .trigger_batch_embed()
            .map_err(|e| Status::internal(format!("Failed to trigger batch embed: {}", e)))?;

        Ok(Response::new(TriggerBatchEmbedResponse {}))
    }

    async fn get_stale_count(
        &self,
        request: Request<GetStaleCountRequest>,
    ) -> Result<Response<GetStaleCountResponse>, Status> {
        let this = self.route(&request).await?;
        // No model required — queries the DB stale-embedding table directly.
        let count = this.stale_count_inner().await?;
        Ok(Response::new(GetStaleCountResponse { count }))
    }

    async fn batch_queue_embeddings(
        &self,
        request: Request<BatchQueueEmbeddingsRequest>,
    ) -> Result<Response<BatchQueueEmbeddingsResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let guard = this.state.read().await;
        let state = guard.as_ref().ok_or_else(|| this.unavailable())?;

        let mut success_count = 0i32;
        let mut failures = Vec::new();

        for node_id in req.node_ids {
            match this.node_service.get_node(&node_id).await {
                Ok(Some(node)) => {
                    match state.embedding_service.queue_for_embedding(&node.id).await {
                        Ok(_) => success_count += 1,
                        Err(e) => failures.push(BatchEmbeddingFailure {
                            node_id: node_id.clone(),
                            error: format!("Failed to queue embedding: {}", e),
                        }),
                    }
                }
                Ok(None) => failures.push(BatchEmbeddingFailure {
                    node_id: node_id.clone(),
                    error: "Node not found".to_string(),
                }),
                Err(e) => failures.push(BatchEmbeddingFailure {
                    node_id: node_id.clone(),
                    error: format!("Failed to get node: {}", e),
                }),
            }
        }

        Ok(Response::new(BatchQueueEmbeddingsResponse {
            success_count,
            failures,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::services::NodeService as CoreNodeService;
    use tempfile::TempDir;

    async fn test_service(load_failed: bool) -> (EmbeddingsServiceImpl, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let mut store = Arc::new(
            SqliteStore::new(tmp.path().join("test.db"))
                .await
                .expect("SqliteStore"),
        );
        let node_service = Arc::new(CoreNodeService::new(&mut store).await.expect("NodeService"));
        let svc = EmbeddingsServiceImpl::new(
            node_service,
            Arc::new(RwLock::new(None)),
            Arc::new(AtomicBool::new(load_failed)),
        );
        (svc, tmp)
    }

    /// While the model is genuinely still loading, RPCs must keep reporting
    /// `UNAVAILABLE` -- unchanged, pre-existing behavior a client is
    /// expected to treat as safe to retry.
    #[tokio::test]
    async fn unavailable_reports_loading_when_not_failed() {
        let (svc, _tmp) = test_service(false).await;
        let status = svc.unavailable();
        assert_eq!(status.code(), tonic::Code::Unavailable);
        assert!(
            status.message().contains("loading"),
            "expected a loading message, got: {}",
            status.message()
        );
    }

    /// The bug this exists to prevent: once the background load has
    /// permanently failed, RPCs must stop claiming the model is "loading" --
    /// a client that retries `UNAVAILABLE` forever never learns the load
    /// isn't coming. `FAILED_PRECONDITION` signals "don't retry" instead.
    #[tokio::test]
    async fn unavailable_reports_failed_precondition_once_load_failed() {
        let (svc, _tmp) = test_service(true).await;
        let status = svc.unavailable();
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert!(
            status.message().contains("failed to load"),
            "expected a failure message distinct from 'loading', got: {}",
            status.message()
        );
    }
}
