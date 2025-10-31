//! Phase 3 Embedding and Mention Endpoints for HTTP Dev Server
//!
//! This module implements embedding generation, semantic search, and node mention
//! operations needed for Phase 3 testing (#212). These endpoints mirror the
//! corresponding Tauri IPC commands in `commands/embeddings.rs` and mention-related
//! commands from `commands/nodes.rs`.
//!
//! # Endpoints
//!
//! ## Embedding Operations
//! - `POST /api/embeddings/generate` - Generate embedding for a topic node
//! - `POST /api/embeddings/search` - Search topics by semantic similarity
//! - `PATCH /api/embeddings/:id` - Update topic embedding immediately
//! - `POST /api/embeddings/batch` - Batch generate embeddings for multiple topics
//! - `GET /api/embeddings/stale-count` - Get count of stale topics needing re-embedding
//! - `POST /api/embeddings/on-topic-closed` - Smart trigger when topic is closed/unfocused
//! - `POST /api/embeddings/on-topic-idle` - Smart trigger after 30s of idle time
//! - `POST /api/embeddings/sync` - Manually sync all stale topics
//!
//! ## Node Mention Operations
//! - `POST /api/nodes/container` - Create a container node (root node with no parent)
//! - `POST /api/nodes/mention` - Create mention relationship between two nodes
//!
//! # Implementation Status
//!
//! ✅ **Phase 3 Complete**: NodeEmbeddingService is integrated into AppState and all endpoints
//! use the real embedding service implementation. All embedding operations are fully functional.
//!
//! # Security (Production Considerations)
//!
//! **IMPORTANT**: These are dev-only endpoints. If adapted for production:
//! - Add authentication/authorization middleware
//! - Implement rate limiting (especially for batch operations and search)
//! - Add request size limits to prevent resource exhaustion
//! - Enable audit logging for all embedding operations
//! - Add monitoring and alerting for abnormal usage patterns

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{get, patch, post},
    Router,
};

use crate::commands::embeddings::{BatchEmbeddingResult, SearchContainersParams};
use crate::commands::nodes::CreateContainerNodeInput;
use crate::dev_server::{AppState, HttpError};
use nodespace_core::services::NodeEmbeddingService;
use nodespace_core::Node;
use std::sync::Arc;

// ============================================================================
// Helper Functions
// ============================================================================

/// Acquire the embedding service from AppState
///
/// This helper extracts the embedding service from the shared AppState,
/// properly handling lock acquisition and Arc cloning. The lock is released
/// immediately after cloning the Arc, minimizing lock hold time.
///
/// # Errors
///
/// Returns `HttpError` with `LOCK_ERROR` code if the RwLock is poisoned.
fn get_embedding_service(state: &AppState) -> Result<Arc<NodeEmbeddingService>, HttpError> {
    let lock = state.embedding_service.read().map_err(|e| {
        HttpError::new(
            format!("Failed to acquire embedding service read lock: {}", e),
            "LOCK_ERROR",
        )
    })?;
    Ok(Arc::clone(&*lock))
}

// ============================================================================
// Embedding Endpoints
// ============================================================================

/// Generate embedding for a topic node
///
/// Calls the NodeEmbeddingService to create a vector embedding for the specified
/// topic node. The embedding is generated from the topic's content and child nodes.
///
/// # Request Body
///
/// JSON object with:
/// - `containerId` (string): ID of the topic node to embed
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/embeddings/generate \
///   -H "Content-Type: application/json" \
///   -d '{"containerId": "topic-uuid-123"}'
/// ```
///
/// # Errors
///
/// - `NODE_NOT_FOUND`: Topic node doesn't exist
/// - `EMBEDDING_GENERATION_FAILED`: Embedding service failed
async fn generate_container_embedding(
    State(state): State<AppState>,
    Json(payload): Json<GenerateEmbeddingRequest>,
) -> Result<StatusCode, HttpError> {
    // Validate container_id is not empty
    if payload.container_id.is_empty() {
        return Err(HttpError::with_details(
            "Topic ID cannot be empty",
            "INVALID_INPUT",
            "container_id must be a non-empty string",
        ));
    }

    let embedding_service = get_embedding_service(&state)?;

    // Generate embedding
    embedding_service
        .embed_container(&payload.container_id)
        .await
        .map_err(|e| {
            tracing::error!(
                "Embedding generation failed for {}: {:?}",
                payload.container_id,
                e
            );
            HttpError::new(
                format!("Embedding generation failed: {}", e),
                "EMBEDDING_GENERATION_FAILED",
            )
        })?;

    Ok(StatusCode::OK)
}

/// Request body for generate embedding
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateEmbeddingRequest {
    container_id: String,
}

/// Search topics by semantic similarity
///
/// Uses Turso's native vector search (DiskANN) for fast approximate nearest neighbors,
/// or exact cosine distance for accurate results.
///
/// # Request Body
///
/// JSON object with search parameters:
/// - `query` (string): Search query text
/// - `threshold` (number, optional): Similarity threshold 0.0-1.0, default 0.7
/// - `limit` (number, optional): Maximum results, default 20
/// - `exact` (boolean, optional): Use exact search instead of approximate, default false
///
/// # Returns
///
/// Array of topic nodes sorted by similarity (most similar first)
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/embeddings/search \
///   -H "Content-Type: application/json" \
///   -d '{
///     "query": "machine learning",
///     "threshold": 0.7,
///     "limit": 20,
///     "exact": false
///   }'
/// ```
///
async fn search_containers(
    State(state): State<AppState>,
    Json(params): Json<SearchContainersParams>,
) -> Result<Json<Vec<Node>>, HttpError> {
    // Validate query is not empty
    if params.query.is_empty() {
        return Err(HttpError::with_details(
            "Search query cannot be empty",
            "INVALID_INPUT",
            "query must be a non-empty string",
        ));
    }

    let threshold = params.threshold.unwrap_or(0.7);
    let limit = params.limit.unwrap_or(20);
    let exact = params.exact.unwrap_or(false);

    let embedding_service = get_embedding_service(&state)?;

    // Perform search
    let results = if exact {
        embedding_service
            .exact_search_containers(&params.query, threshold, limit)
            .await
    } else {
        embedding_service
            .search_containers(&params.query, threshold, limit)
            .await
    };

    results.map(Json).map_err(|e| {
        tracing::error!(
            "Container search failed for query '{}': {:?}",
            params.query,
            e
        );
        HttpError::new(format!("Topic search failed: {}", e), "TOPIC_SEARCH_FAILED")
    })
}

/// Update embedding for a topic node immediately
///
/// Re-generates the embedding for a topic node. Use this for explicit user actions
/// like "Regenerate Embedding" button. For automatic updates on content changes,
/// use the smart triggers (on_container_closed, on_container_idle) instead.
///
/// # Path Parameters
///
/// - `id`: Topic node ID
///
/// # Example
///
/// ```bash
/// curl -X PATCH http://localhost:3001/api/embeddings/topic-uuid-123
/// ```
///
async fn update_container_embedding(
    State(state): State<AppState>,
    Path(container_id): Path<String>,
) -> Result<StatusCode, HttpError> {
    // Validate container_id is not empty
    if container_id.is_empty() {
        return Err(HttpError::with_details(
            "Topic ID cannot be empty",
            "INVALID_INPUT",
            "container_id must be a non-empty string",
        ));
    }

    let embedding_service = get_embedding_service(&state)?;

    // Update embedding
    embedding_service
        .embed_container(&container_id)
        .await
        .map_err(|e| {
            tracing::error!("Embedding update failed for {}: {:?}", container_id, e);
            HttpError::new(
                format!("Embedding update failed: {}", e),
                "EMBEDDING_UPDATE_FAILED",
            )
        })?;

    Ok(StatusCode::OK)
}

/// Batch generate embeddings for multiple topics
///
/// Useful for initial embedding generation or bulk operations.
/// Continues processing even if some embeddings fail.
///
/// # Request Body
///
/// JSON object with:
/// - `containerIds` (array of strings): Topic IDs to embed
///
/// # Returns
///
/// JSON object with:
/// - `successCount` (number): Number of successfully embedded topics
/// - `failedEmbeddings` (array): Details of topics that failed with error messages
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/embeddings/batch \
///   -H "Content-Type: application/json" \
///   -d '{"containerIds": ["topic-1", "topic-2", "topic-3"]}'
/// ```
///
/// # TODO
///
/// Replace placeholder with actual NodeEmbeddingService call once added to AppState.
async fn batch_generate_embeddings(
    State(state): State<AppState>,
    Json(payload): Json<BatchGenerateRequest>,
) -> Result<Json<BatchEmbeddingResult>, HttpError> {
    // Validate container_ids array is not empty
    if payload.container_ids.is_empty() {
        return Err(HttpError::with_details(
            "Topic IDs array cannot be empty",
            "INVALID_INPUT",
            "Must provide at least one topic ID",
        ));
    }

    let embedding_service = get_embedding_service(&state)?;

    let mut success_count = 0;
    let mut failed_embeddings = Vec::new();

    // Process each container ID and generate embeddings
    // TODO: Consider using futures::future::join_all for parallel processing to improve performance
    // Sequential processing is acceptable for now and ensures proper error tracking per container
    for container_id in payload.container_ids {
        match embedding_service.embed_container(&container_id).await {
            Ok(()) => {
                success_count += 1;
            }
            Err(e) => {
                tracing::error!("Failed to generate embedding for {}: {:?}", container_id, e);
                failed_embeddings.push(crate::commands::embeddings::BatchEmbeddingError {
                    container_id: container_id.clone(),
                    error: e.to_string(),
                });
            }
        }
    }

    tracing::info!(
        "Batch embedding completed: {} succeeded, {} failed",
        success_count,
        failed_embeddings.len()
    );

    Ok(Json(BatchEmbeddingResult {
        success_count,
        failed_embeddings,
    }))
}

/// Request body for batch embedding generation
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchGenerateRequest {
    container_ids: Vec<String>,
}

/// Get count of stale topics needing re-embedding
///
/// Returns the number of topics that have been modified but not yet re-embedded.
/// Useful for showing status indicators in UI (e.g., "5 topics need indexing").
///
/// # Example
///
/// ```bash
/// curl http://localhost:3001/api/embeddings/stale-count
/// ```
///
async fn get_stale_container_count(
    State(state): State<AppState>,
) -> Result<Json<usize>, HttpError> {
    let embedding_service = get_embedding_service(&state)?;

    // Get stale containers
    let containers = embedding_service
        .get_all_stale_containers()
        .await
        .map_err(|e| {
            tracing::error!("Failed to get stale containers: {:?}", e);
            HttpError::new(format!("Stale count failed: {}", e), "STALE_COUNT_FAILED")
        })?;

    Ok(Json(containers.len()))
}

/// Smart trigger: Topic closed/unfocused
///
/// Called when user closes or navigates away from a topic.
/// If the topic was recently edited, triggers immediate re-embedding.
///
/// # Request Body
///
/// JSON object with:
/// - `containerId` (string): ID of the topic that was closed
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/embeddings/on-topic-closed \
///   -H "Content-Type: application/json" \
///   -d '{"containerId": "topic-uuid-123"}'
/// ```
async fn on_container_closed(
    State(state): State<AppState>,
    Json(payload): Json<TopicIdRequest>,
) -> Result<StatusCode, HttpError> {
    // Validate container_id is not empty
    if payload.container_id.is_empty() {
        return Err(HttpError::with_details(
            "Topic ID cannot be empty",
            "INVALID_INPUT",
            "container_id must be a non-empty string",
        ));
    }

    let embedding_service = get_embedding_service(&state)?;

    // Handle container closed event
    embedding_service
        .on_container_closed(&payload.container_id)
        .await
        .map_err(|e| {
            tracing::error!(
                "Topic close handler failed for {}: {:?}",
                payload.container_id,
                e
            );
            HttpError::new(format!("Topic close failed: {}", e), "TOPIC_CLOSE_FAILED")
        })?;

    Ok(StatusCode::OK)
}

/// Smart trigger: Idle timeout
///
/// Called when user has stopped editing for 30+ seconds.
/// Triggers re-embedding if topic is stale.
///
/// # Request Body
///
/// JSON object with:
/// - `containerId` (string): ID of the topic to check
///
/// # Returns
///
/// Boolean indicating whether re-embedding was triggered
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/embeddings/on-topic-idle \
///   -H "Content-Type: application/json" \
///   -d '{"containerId": "topic-uuid-123"}'
/// ```
async fn on_container_idle(
    State(state): State<AppState>,
    Json(payload): Json<TopicIdRequest>,
) -> Result<Json<bool>, HttpError> {
    // Validate container_id is not empty
    if payload.container_id.is_empty() {
        return Err(HttpError::with_details(
            "Topic ID cannot be empty",
            "INVALID_INPUT",
            "container_id must be a non-empty string",
        ));
    }

    let embedding_service = get_embedding_service(&state)?;

    // Handle idle timeout
    let was_embedded = embedding_service
        .on_idle_timeout(&payload.container_id)
        .await
        .map_err(|e| {
            tracing::error!(
                "Idle timeout handler failed for {}: {:?}",
                payload.container_id,
                e
            );
            HttpError::new(format!("Idle timeout failed: {}", e), "IDLE_TIMEOUT_FAILED")
        })?;

    Ok(Json(was_embedded))
}

/// Manually sync all stale topics
///
/// Processes all stale topics immediately. Useful for explicit user action
/// like "Sync All" button or app startup.
///
/// # Returns
///
/// Number of topics re-embedded
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/embeddings/sync
/// ```
async fn sync_embeddings(State(state): State<AppState>) -> Result<Json<usize>, HttpError> {
    let embedding_service = get_embedding_service(&state)?;

    // Sync all stale containers
    let count = embedding_service
        .sync_all_stale_containers()
        .await
        .map_err(|e| {
            tracing::error!("Sync all stale containers failed: {:?}", e);
            HttpError::new(format!("Sync failed: {}", e), "SYNC_FAILED")
        })?;

    Ok(Json(count))
}

/// Request body for topic ID operations
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TopicIdRequest {
    container_id: String,
}

// ============================================================================
// Node Mention Endpoints
// ============================================================================

/// Create a container node (root node with no parent)
///
/// Container nodes are root-level nodes that can contain other nodes. They always have:
/// - parent_id = NULL
/// - container_node_id = NULL (they ARE containers)
/// - before_sibling_id = NULL
///
/// # Request Body
///
/// JSON object with:
/// - `content` (string): Node content
/// - `nodeType` (string): Type of node (text, task, date)
/// - `properties` (object, optional): Additional properties
/// - `mentionedBy` (string, optional): ID of node that mentions this container
///
/// # Returns
///
/// ID of the created container node
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/nodes/container \
///   -H "Content-Type: application/json" \
///   -d '{
///     "content": "Project Planning",
///     "nodeType": "text",
///     "properties": {},
///     "mentionedBy": "daily-note-id"
///   }'
/// ```
async fn create_container_node(
    State(state): State<AppState>,
    Json(input): Json<CreateContainerNodeInput>,
) -> Result<Json<String>, HttpError> {
    use crate::constants::ALLOWED_NODE_TYPES;
    use chrono::Utc;
    use uuid::Uuid;

    // Validate node type (same as Tauri command)
    if !ALLOWED_NODE_TYPES.contains(&input.node_type.as_str()) {
        return Err(HttpError::with_details(
            format!(
                "Only text, task, and date nodes are supported. Got: {}",
                input.node_type
            ),
            "INVALID_NODE_TYPE",
            format!(
                "Allowed types: {:?}, received: '{}'",
                ALLOWED_NODE_TYPES, input.node_type
            ),
        ));
    }

    let now = Utc::now();
    let node_id = Uuid::new_v4().to_string();

    let container_node = nodespace_core::Node {
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

    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node service read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };
    node_service
        .create_node(container_node)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "NODE_SERVICE_ERROR"))?;

    // If mentioned_by is provided, create mention relationship
    if let Some(mentioning_node_id) = input.mentioned_by {
        node_service
            .create_mention(&mentioning_node_id, &node_id)
            .await
            .map_err(|e| HttpError::from_anyhow(e.into(), "NODE_SERVICE_ERROR"))?;
    }

    tracing::debug!("Created container node: {}", node_id);

    Ok(Json(node_id))
}

/// Create a mention relationship between two nodes
///
/// Records that one node mentions another in the node_mentions table.
/// This enables backlink/references functionality.
///
/// # Request Body
///
/// JSON object with:
/// - `mentioningNodeId` (string): ID of the node that contains the mention
/// - `mentionedNodeId` (string): ID of the node being mentioned
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/nodes/mention \
///   -H "Content-Type: application/json" \
///   -d '{
///     "mentioningNodeId": "daily-note-id",
///     "mentionedNodeId": "project-planning-id"
///   }'
/// ```
async fn create_node_mention(
    State(state): State<AppState>,
    Json(payload): Json<CreateMentionRequest>,
) -> Result<StatusCode, HttpError> {
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node service read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };
    node_service
        .create_mention(&payload.mentioning_node_id, &payload.mentioned_node_id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "NODE_SERVICE_ERROR"))?;

    tracing::debug!(
        "Created mention: {} -> {}",
        payload.mentioning_node_id,
        payload.mentioned_node_id
    );

    Ok(StatusCode::OK)
}

/// Request body for creating a mention
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateMentionRequest {
    mentioning_node_id: String,
    mentioned_node_id: String,
}

/// GET /api/nodes/:id/mentioning-containers
///
/// Get containers of nodes that mention the target node (backlinks at container level).
///
/// This resolves incoming mentions to their container nodes and deduplicates.
/// Exceptions: Task and ai-chat nodes are treated as their own containers.
///
/// # Path Parameters
///
/// - `id` (string): The node ID to find backlinks for
///
/// # Response
///
/// JSON array of unique container node IDs:
/// ```json
/// ["container-id-1", "container-id-2"]
/// ```
///
/// # Example
///
/// ```bash
/// curl http://localhost:3001/api/nodes/some-node-id/mentioning-containers
/// ```
async fn get_mentioning_containers(
    State(state): State<AppState>,
    Path(node_id): Path<String>,
) -> Result<Json<Vec<String>>, HttpError> {
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node service read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };

    let container_ids = node_service
        .get_mentioning_containers(&node_id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "NODE_SERVICE_ERROR"))?;

    tracing::debug!(
        "Found {} mentioning containers for node {}",
        container_ids.len(),
        node_id
    );

    Ok(Json(container_ids))
}

// ============================================================================
// Router Configuration
// ============================================================================

/// Create router with all Phase 3 embedding and mention endpoints
///
/// This function is called by the main router in `mod.rs` to register
/// all Phase 3 endpoints.
pub fn routes(state: AppState) -> Router {
    Router::new()
        // Embedding endpoints
        .route(
            "/api/embeddings/generate",
            post(generate_container_embedding),
        )
        .route("/api/embeddings/search", post(search_containers))
        .route("/api/embeddings/:id", patch(update_container_embedding))
        .route("/api/embeddings/batch", post(batch_generate_embeddings))
        .route(
            "/api/embeddings/stale-count",
            get(get_stale_container_count),
        )
        .route("/api/embeddings/on-topic-closed", post(on_container_closed))
        .route("/api/embeddings/on-topic-idle", post(on_container_idle))
        .route("/api/embeddings/sync", post(sync_embeddings))
        // Node mention endpoints
        .route("/api/nodes/container", post(create_container_node))
        .route("/api/nodes/mention", post(create_node_mention))
        .route(
            "/api/nodes/:id/mentioning-containers",
            get(get_mentioning_containers),
        )
        .with_state(state)
}
