//! Development HTTP Proxy for Browser Mode
//!
//! This is a minimal HTTP server that forwards requests to NodeService,
//! preserving all Rust business logic while enabling SurrealDB inspection.
//!
//! Architecture:
//!   Frontend → HTTP (port 3001) → NodeService → SurrealStore (HTTP) → SurrealDB (port 8000)
//!                                                                              ↓
//!   Surrealist ──────────────────────────────────────────────────────────→ SurrealDB (port 8000)
//!
//! # Database Location
//!
//! Unlike the old dev-server which used embedded RocksDB at:
//! `~/.nodespace/database/nodespace-dev/`
//!
//! The dev-proxy connects to a remote SurrealDB HTTP server running on port 8000.
//! The database is stored in-memory by default (started with `bun run dev:db`),
//! or can be persisted to `~/.nodespace/dev.db` using `bun run dev:db:persist`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event, Json, Sse},
    routing::{delete, get, patch, post},
    Router,
};
use futures::stream::Stream;
use nodespace_core::{
    db::HttpStore,
    models,
    models::{Node, NodeFilter, NodeUpdate, SchemaNode},
    services::{CreateNodeParams, NodeService, NodeServiceError},
};
use serde::{Deserialize, Serialize};
use std::{convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use tower_http::cors::CorsLayer;

// Type alias for HTTP client types
type HttpNodeService = NodeService<surrealdb::engine::remote::http::Client>;
// NOTE: SchemaService removed (Issue #690) - schema operations use NodeService directly

// ============================================================================
// SSE Events for Browser Mode Real-Time Sync
// ============================================================================

/// SSE event types for real-time synchronization
///
/// These events mirror the structure expected by the frontend BrowserSyncService.
/// Events are broadcast after each successful database operation to notify
/// all connected browser clients of changes.
///
/// Each event includes an optional client_id field to identify the originating client,
/// allowing SSE handlers to filter out their own events (prevent feedback loop).
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SseEvent {
    /// A new node was created
    NodeCreated {
        #[serde(rename = "nodeId")]
        node_id: String,
        #[serde(rename = "nodeData")]
        node_data: Node,
        #[serde(rename = "clientId", skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },
    /// An existing node was updated
    NodeUpdated {
        #[serde(rename = "nodeId")]
        node_id: String,
        #[serde(rename = "nodeData")]
        node_data: Node,
        #[serde(rename = "clientId", skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },
    /// A node was deleted
    NodeDeleted {
        #[serde(rename = "nodeId")]
        node_id: String,
        #[serde(rename = "clientId", skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },
    /// A parent-child edge was created
    EdgeCreated {
        #[serde(rename = "parentId")]
        parent_id: String,
        #[serde(rename = "childId")]
        child_id: String,
        #[serde(rename = "clientId", skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },
    /// A parent-child edge was deleted
    EdgeDeleted {
        #[serde(rename = "parentId")]
        parent_id: String,
        #[serde(rename = "childId")]
        child_id: String,
        #[serde(rename = "clientId", skip_serializing_if = "Option::is_none")]
        client_id: Option<String>,
    },
}

/// Application state shared across handlers
///
/// As of Issue #676, NodeOperations layer was merged into NodeService.
/// As of Issue #690, SchemaService removed - schema operations use NodeService directly.
/// All business logic now goes directly through NodeService.
#[derive(Clone)]
struct AppState {
    node_service: Arc<HttpNodeService>,
    /// Broadcast channel for SSE events to connected browser clients
    event_tx: broadcast::Sender<SseEvent>,
}

/// Structured error response matching Tauri's CommandError format
///
/// Provides consistent error handling between HTTP (browser mode) and
/// Tauri (desktop app) by returning the same JSON structure:
/// ```json
/// {
///   "message": "User-facing error message",
///   "code": "MACHINE_READABLE_CODE",
///   "details": "Optional debugging information"
/// }
/// ```
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiError {
    /// User-facing error message
    pub message: String,
    /// Machine-readable error code
    pub code: String,
    /// Optional detailed error information for debugging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl ApiError {
    fn new(code: &str, message: String) -> Self {
        Self {
            message: message.clone(),
            code: code.to_string(),
            details: Some(message),
        }
    }
}

// NOTE: SchemaFieldResult and ApiSchemaResult removed (Issue #690)
// Schema mutation operations not used by UI

/// Standard result types for HTTP handlers
type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;
type ApiStatusResult = Result<StatusCode, (StatusCode, Json<ApiError>)>;

/// Convert a Node to its strongly-typed JSON representation (Issue #673)
///
/// Delegates to the canonical `models::node_to_typed_value` and maps errors to ApiError.
fn node_to_typed_value(node: Node) -> Result<serde_json::Value, ApiError> {
    models::node_to_typed_value(node).map_err(|e| ApiError {
        message: e.clone(),
        code: "CONVERSION_ERROR".to_string(),
        details: Some(e),
    })
}

/// Convert a list of Nodes to their strongly-typed JSON representations (Issue #673)
fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<serde_json::Value>, ApiError> {
    models::nodes_to_typed_values(nodes).map_err(|e| ApiError {
        message: e.clone(),
        code: "CONVERSION_ERROR".to_string(),
        details: Some(e),
    })
}

/// Update node request with OCC version
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateNodeRequest {
    /// Expected version for optimistic concurrency control (currently unused - TODO)
    #[allow(dead_code)]
    pub version: i64,
    /// Fields to update
    #[serde(flatten)]
    pub update: NodeUpdate,
}

/// Delete node request with OCC version
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteNodeRequest {
    /// Expected version for optimistic concurrency control (currently unused - TODO)
    #[allow(dead_code)]
    pub version: i64,
}

/// Mention creation/deletion request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MentionRequest {
    pub source_id: String,
    pub target_id: String,
}

/// Mention autocomplete request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MentionAutocompleteRequest {
    pub query: String,
    pub limit: Option<i64>,
}

// NOTE: AddSchemaFieldRequest and ExtendEnumRequest removed (Issue #690)
// Schema mutation operations not used by UI

// === Error Mapping Helpers ===

/// Maps NodeServiceError to HTTP status code and structured ApiError
///
/// This centralizes error mapping logic to ensure consistent HTTP responses
/// across all endpoints. Follows REST best practices:
/// - 404 NOT FOUND: Resource doesn't exist
/// - 400 BAD REQUEST: Validation failures
/// - 409 CONFLICT: Version conflicts (OCC failures)
/// - 500 INTERNAL SERVER ERROR: Unexpected errors
fn map_node_service_error(err: NodeServiceError) -> (StatusCode, Json<ApiError>) {
    let error_str = err.to_string();

    let (status, code) = if error_str.contains("not found") {
        (StatusCode::NOT_FOUND, "RESOURCE_NOT_FOUND")
    } else if error_str.contains("already exists") {
        (StatusCode::BAD_REQUEST, "ALREADY_EXISTS")
    } else if error_str.contains("validation") {
        (StatusCode::BAD_REQUEST, "VALIDATION_ERROR")
    } else if error_str.contains("version") || error_str.contains("conflict") {
        (StatusCode::CONFLICT, "VERSION_CONFLICT")
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR")
    };

    (
        status,
        Json(ApiError::new(
            code,
            format!("Operation failed: {}", error_str),
        )),
    )
}

// NOTE: map_schema_error removed (Issue #690) - schema operations use map_node_service_error

/// Create node request for POST /api/nodes endpoint
///
/// The frontend sends a partial node representation containing only the business data.
/// The backend is responsible for adding infrastructure fields (timestamps, version).
///
/// # Field Semantics
///
/// - `id`: Optional UUID. If omitted, backend generates a new UUID.
///   For deterministic IDs (e.g., date nodes use "YYYY-MM-DD"),
///   the frontend MUST provide this field.
/// - `node_type`: Required. Must be a registered node type (e.g., "text", "task", "date").
/// - `content`: Primary text content. MAY be empty string (Issue #484 - blank nodes allowed).
/// - `parent_id`: Optional parent node for hierarchical relationships.
/// - `root_id`: Optional root for spatial/collection relationships.
/// - `before_sibling_id`: Optional sibling for ordering within parent.
/// - `properties`: Schema-driven JSON properties. Backend applies default values if schema exists.
/// - `embedding_vector`: Optional vector for semantic search (currently unused).
/// - `mentions`: Outgoing references to other nodes. `mentioned_by` is computed server-side
///   and NOT accepted here (it's the inverse relationship).
///
/// # Validation
///
/// Backend performs two-stage validation:
/// 1. Core behavior validation (via `NodeService.behaviors.validate_node()`)
/// 2. Schema validation (via `NodeService.validate_node_against_schema()`)
///
/// # Example
///
/// ```json
/// {
///   "nodeType": "text",
///   "content": "",
///   "parentId": "550e8400-e29b-41d4-a716-446655440000",
///   "properties": {},
///   "mentions": []
/// }
/// ```
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateNodeRequest {
    #[serde(default)]
    pub id: Option<String>,
    pub node_type: String,
    pub content: String,
    // CRITICAL FIX (Issue #528): Re-added parent_id to HTTP API for edge creation
    // While Node type no longer has this field (stored as graph edges),
    // the CREATE operation needs this info to establish parent-child relationships
    // The operations layer (NodeOperations::create_node) handles edge creation
    // Note (Issue #533): root_id removed - backend auto-derives root from parent chain
    pub parent_id: Option<String>,
    pub insert_after_node_id: Option<String>,
    pub properties: serde_json::Value,
    // TODO: Implement embedding_vector and mentions support in operations layer
    #[allow(dead_code)]
    pub embedding_vector: Option<Vec<f32>>,
    #[allow(dead_code)]
    pub mentions: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging (shows debug output in terminal)
    tracing_subscriber::fmt()
        .with_env_filter("dev_proxy=debug,nodespace_core=debug")
        .init();

    println!("🔧 Initializing dev-proxy...");

    // Connect to SurrealDB HTTP server (must be running on port 8000)
    // IMPORTANT: Uses HTTP client mode, NOT embedded RocksDB
    println!("📡 Connecting to SurrealDB HTTP server on port 8000...");
    let mut store =
        match HttpStore::new_http("127.0.0.1:8000", "nodespace", "nodespace", "root", "root").await
        {
            Ok(s) => {
                println!("✅ Connected to SurrealDB");
                Arc::new(s)
            }
            Err(e) => {
                eprintln!("❌ Failed to connect to SurrealDB: {}", e);
                eprintln!("   Make sure SurrealDB server is running:");
                eprintln!("   bun run dev:db");
                eprintln!("\n   Or check if port 8000 is available:");
                eprintln!("   lsof -i :8000");
                return Err(e);
            }
        };

    // Initialize NodeService with all business logic
    // NodeService has ALL the logic (virtual dates, schema backfill, etc.)
    // NodeService::new() takes &mut Arc to enable cache updates during seeding (Issue #704)
    println!("🧠 Initializing NodeService...");
    let node_service = match NodeService::new(&mut store).await {
        Ok(s) => {
            println!("✅ NodeService initialized");
            Arc::new(s)
        }
        Err(e) => {
            eprintln!("❌ Failed to initialize NodeService: {}", e);
            return Err(e.into());
        }
    };

    // NOTE: NodeOperations layer was merged into NodeService (Issue #676)
    // NOTE: SchemaService removed (Issue #690) - schema operations use NodeService directly

    // Create broadcast channel for SSE events (capacity 100 messages)
    // Lagging receivers will skip missed messages rather than blocking senders
    let (event_tx, _) = broadcast::channel::<SseEvent>(100);
    println!("📡 SSE broadcast channel initialized");

    let state = AppState {
        node_service: node_service.clone(),
        event_tx: event_tx.clone(),
    };

    // Build HTTP router
    let app = Router::new()
        // Health check (useful for testing)
        .route("/health", get(health_check))
        // SSE endpoint for real-time sync in browser mode
        .route("/api/events", get(sse_handler))
        // Database initialization (no-op - already initialized on startup)
        .route("/api/database/init", post(init_database))
        // Node CRUD endpoints
        .route("/api/nodes", post(create_node))
        .route("/api/nodes/:id", get(get_node))
        .route("/api/nodes/:id", patch(update_node))
        .route("/api/nodes/:id", delete(delete_node))
        // Hierarchy endpoints
        .route("/api/nodes/:id/parent", post(set_parent))
        // Query endpoints
        .route("/api/nodes/:id/children", get(get_children))
        .route("/api/nodes/:id/children-tree", get(get_children_tree))
        .route("/api/query", post(query_nodes))
        // Mention endpoints
        .route("/api/mentions", post(create_mention))
        .route("/api/mentions", delete(delete_mention))
        .route("/api/mentions/autocomplete", post(mention_autocomplete))
        .route(
            "/api/nodes/:id/mentions/outgoing",
            get(get_outgoing_mentions),
        )
        .route(
            "/api/nodes/:id/mentions/incoming",
            get(get_incoming_mentions),
        )
        .route(
            "/api/nodes/:id/mentions/roots",
            get(get_mentioning_containers),
        )
        // Schema endpoints (read-only - mutation endpoints removed Issue #690, not used by UI)
        .route("/api/schemas", get(get_all_schemas))
        .route("/api/schemas/:id", get(get_schema))
        .with_state(state)
        .layer(CorsLayer::permissive()); // Allow CORS from frontend (localhost:5173)

    // Start HTTP server on port 3001
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3001")
        .await
        .map_err(|e| {
            eprintln!("❌ Failed to bind to port 3001: {}", e);
            eprintln!("   Another process may be using this port.");
            eprintln!("   Check with: lsof -i :3001");
            eprintln!("   Kill with: kill -9 <PID>");
            e
        })?;

    println!("\n🚀 Dev proxy server started!");
    println!("   Listening on: http://127.0.0.1:3001");
    println!("   SSE endpoint: http://127.0.0.1:3001/api/events");
    println!("   NodeService → SurrealDB (port 8000)");
    println!("   Surrealist can connect to port 8000\n");

    axum::serve(listener, app).await?;

    Ok(())
}

// === Handler Functions ===

async fn health_check() -> &'static str {
    "OK"
}

/// SSE endpoint for real-time sync in browser mode
///
/// Browser clients connect to this endpoint to receive real-time updates when
/// database changes occur. This is the browser-mode equivalent of Tauri's
/// domain event subscription.
///
/// # Protocol
/// - Server-Sent Events (SSE) - one-way server→client stream
/// - Events are JSON-encoded SseEvent objects
/// - Includes keepalive comments every 30 seconds to prevent timeouts
///
/// # Client ID Filtering
/// Clients pass their ID via query parameter: `/api/events?clientId=xxx`
/// Events originating from the same client are filtered out to prevent feedback loops.
///
/// # Usage
/// ```javascript
/// const clientId = getClientId(); // from client-id.ts
/// const eventSource = new EventSource(`http://localhost:3001/api/events?clientId=${clientId}`);
/// eventSource.onmessage = (event) => {
///     const data = JSON.parse(event.data);
///     // Handle nodeCreated, nodeUpdated, nodeDeleted, edgeCreated, edgeDeleted
/// };
/// ```
async fn sse_handler(
    State(state): State<AppState>,
    query: axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Extract client ID from query parameters (EventSource API doesn't support custom headers)
    let client_id = query.get("clientId").cloned();

    tracing::debug!("SSE client connected with client_id: {:?}", client_id);

    let rx = state.event_tx.subscribe();

    // Transform broadcast messages into SSE events, filtering out events from this client
    let stream = BroadcastStream::new(rx).filter_map(move |result| {
        let client_id = client_id.clone();
        // Skip lagged/closed errors, only process actual messages
        match result {
            Ok(sse_event) => {
                // Filter out events from this client (prevent feedback loop)
                let event_client_id = match &sse_event {
                    SseEvent::NodeCreated { client_id, .. } => client_id,
                    SseEvent::NodeUpdated { client_id, .. } => client_id,
                    SseEvent::NodeDeleted { client_id, .. } => client_id,
                    SseEvent::EdgeCreated { client_id, .. } => client_id,
                    SseEvent::EdgeDeleted { client_id, .. } => client_id,
                };

                // Skip if this event came from the same client
                if let (Some(this_client), Some(event_client)) = (&client_id, event_client_id) {
                    if this_client == event_client {
                        tracing::debug!(
                            "Filtering out SSE event from same client: {:?}",
                            event_client
                        );
                        return None;
                    }
                }

                // Serialize event to JSON
                match serde_json::to_string(&sse_event) {
                    Ok(json) => Some(Ok(Event::default().data(json))),
                    Err(e) => {
                        tracing::error!("Failed to serialize SSE event: {}", e);
                        None
                    }
                }
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!("SSE client lagged by {} messages", n);
                None
            }
        }
    });

    // Use Axum's built-in keepalive to prevent connection timeouts
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keepalive"),
    )
}

/// Database initialization endpoint (no-op for dev-proxy)
///
/// The database is already initialized when dev-proxy starts, so this just
/// returns success. This endpoint exists for frontend compatibility.
async fn init_database() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "dbPath": "http://127.0.0.1:8000 (via dev-proxy)"
    }))
}

async fn create_node(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<CreateNodeRequest>,
) -> ApiResult<String> {
    // Extract client ID for SSE filtering
    let client_id = headers
        .get("x-client-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Use NodeService directly to handle node creation and edge creation
    // (Issue #676: NodeOperations merged into NodeService)
    tracing::debug!(
        "create_node request: node_type={:?}, parent_id={:?}, insert_after_node_id={:?}",
        req.node_type,
        req.parent_id,
        req.insert_after_node_id
    );

    let params = CreateNodeParams {
        id: req.id,
        node_type: req.node_type,
        content: req.content,
        parent_id: req.parent_id,
        insert_after_node_id: req.insert_after_node_id,
        properties: req.properties,
    };

    let parent_id_clone = params.parent_id.clone();
    let id = state
        .node_service
        .create_node_with_parent(params)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ApiError::new("CREATE_FAILED", e.to_string())),
            )
        })?;

    // Broadcast SSE event for the created node
    // Fetch the full node data for the event payload
    if let Ok(Some(node)) = state.node_service.get_node(&id).await {
        let _ = state.event_tx.send(SseEvent::NodeCreated {
            node_id: id.clone(),
            node_data: node,
            client_id: client_id.clone(),
        });
        tracing::debug!("SSE: NodeCreated event sent for node {}", id);

        // If node has a parent, also broadcast edge created event
        if let Some(parent_id) = parent_id_clone {
            let _ = state.event_tx.send(SseEvent::EdgeCreated {
                parent_id,
                child_id: id.clone(),
                client_id: client_id.clone(),
            });
            tracing::debug!("SSE: EdgeCreated event sent for node {}", id);
        }
    }

    Ok(Json(id))
}

async fn get_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Option<serde_json::Value>> {
    // ⭐ This call includes ALL business logic:
    // - Virtual date node creation (YYYY-MM-DD format)
    // - populate_mentions() (backlinks)
    // - backfill_schema_version() (adds _schema_version to properties)
    // - apply_lazy_migration() (upgrades nodes to latest schema)
    let node = state
        .node_service
        .get_node(&id)
        .await
        .map_err(map_node_service_error)?;

    match node {
        Some(n) => {
            let typed =
                node_to_typed_value(n).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(e)))?;
            Ok(Json(Some(typed)))
        }
        None => Ok(Json(None)),
    }
}

async fn update_node(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<UpdateNodeRequest>,
) -> ApiResult<serde_json::Value> {
    // Extract client ID for SSE filtering
    let client_id = headers
        .get("x-client-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Use optimistic concurrency control via version check
    let rows_affected = state
        .node_service
        .update_with_version_check(&id, request.version, request.update)
        .await
        .map_err(map_node_service_error)?;

    // Check if update succeeded (version matched)
    if rows_affected == 0 {
        return Err((
            StatusCode::CONFLICT,
            Json(ApiError::new(
                "VERSION_CONFLICT",
                format!(
                    "Version conflict: node {} has been modified by another client",
                    id
                ),
            )),
        ));
    }

    // Retrieve and return the updated node
    // This includes all business logic (populate_mentions, backfill_schema_version, etc.)
    let updated_node = state
        .node_service
        .get_node(&id)
        .await
        .map_err(map_node_service_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiError::new(
                    "RESOURCE_NOT_FOUND",
                    format!("Node {} not found after update", id),
                )),
            )
        })?;

    // Broadcast SSE event for the updated node
    let _ = state.event_tx.send(SseEvent::NodeUpdated {
        node_id: id.clone(),
        node_data: updated_node.clone(),
        client_id,
    });
    tracing::debug!("SSE: NodeUpdated event sent for node {}", id);

    let typed = node_to_typed_value(updated_node)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(e)))?;
    Ok(Json(typed))
}

async fn delete_node(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Json(request): Json<DeleteNodeRequest>,
) -> ApiStatusResult {
    // Extract client ID for SSE filtering
    let client_id = headers
        .get("x-client-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Use optimistic concurrency control via version check
    let rows_affected = state
        .node_service
        .delete_with_version_check(&id, request.version)
        .await
        .map_err(map_node_service_error)?;

    // Check if delete succeeded (version matched and node existed)
    if rows_affected == 0 {
        // Check if node exists to distinguish between version conflict and not found
        match state.node_service.get_node(&id).await {
            Ok(Some(_)) => {
                // Node exists but version didn't match - conflict
                return Err((
                    StatusCode::CONFLICT,
                    Json(ApiError::new(
                        "VERSION_CONFLICT",
                        format!(
                            "Version conflict: node {} has been modified by another client",
                            id
                        ),
                    )),
                ));
            }
            Ok(None) => {
                // Node doesn't exist - already deleted or never existed
                // Idempotent DELETE: return success (204) instead of 404
                // This follows RESTful best practices - DELETE should succeed
                // even if the resource is already gone
                return Ok(StatusCode::NO_CONTENT);
            }
            Err(e) => {
                // Database error
                return Err(map_node_service_error(e));
            }
        }
    }

    // Broadcast SSE event for the deleted node
    let _ = state.event_tx.send(SseEvent::NodeDeleted {
        node_id: id.clone(),
        client_id,
    });
    tracing::debug!("SSE: NodeDeleted event sent for node {}", id);

    Ok(StatusCode::NO_CONTENT)
}

/// Set parent request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetParentRequest {
    /// New parent ID (null to make node a root)
    pub parent_id: Option<String>,
    /// Optional sibling to insert after (None = insert at beginning)
    pub insert_after_node_id: Option<String>,
}

async fn set_parent(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(node_id): Path<String>,
    Json(request): Json<SetParentRequest>,
) -> ApiStatusResult {
    // Extract client ID for SSE filtering
    let client_id = headers
        .get("x-client-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Get current parent before moving (for edge deleted event)
    let old_parent_id = state
        .node_service
        .get_parent(&node_id)
        .await
        .ok()
        .flatten()
        .map(|n| n.id);

    state
        .node_service
        .move_node(
            &node_id,
            request.parent_id.as_deref(),
            request.insert_after_node_id.as_deref(),
        )
        .await
        .map_err(map_node_service_error)?;

    // Broadcast SSE edge events
    // If node had an old parent, send edge deleted event
    if let Some(old_parent) = old_parent_id {
        let _ = state.event_tx.send(SseEvent::EdgeDeleted {
            parent_id: old_parent,
            child_id: node_id.clone(),
            client_id: client_id.clone(),
        });
        tracing::debug!("SSE: EdgeDeleted event sent for node {}", node_id);
    }

    // If node has a new parent, send edge created event
    if let Some(ref new_parent) = request.parent_id {
        let _ = state.event_tx.send(SseEvent::EdgeCreated {
            parent_id: new_parent.clone(),
            child_id: node_id.clone(),
            client_id: client_id.clone(),
        });
        tracing::debug!("SSE: EdgeCreated event sent for node {}", node_id);
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn get_children(
    State(state): State<AppState>,
    Path(parent_id): Path<String>,
) -> ApiResult<Vec<serde_json::Value>> {
    let children = state
        .node_service
        .get_children(&parent_id)
        .await
        .map_err(map_node_service_error)?;

    let typed = nodes_to_typed_values(children)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(e)))?;
    Ok(Json(typed))
}

async fn get_children_tree(
    State(state): State<AppState>,
    Path(parent_id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let tree = state
        .node_service
        .get_children_tree(&parent_id)
        .await
        .map_err(map_node_service_error)?;

    Ok(Json(tree))
}

async fn query_nodes(
    State(state): State<AppState>,
    Json(filter): Json<NodeFilter>,
) -> ApiResult<Vec<serde_json::Value>> {
    let nodes = state
        .node_service
        .query_nodes(filter)
        .await
        .map_err(map_node_service_error)?;

    let typed =
        nodes_to_typed_values(nodes).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(e)))?;
    Ok(Json(typed))
}

async fn create_mention(
    State(state): State<AppState>,
    Json(request): Json<MentionRequest>,
) -> ApiStatusResult {
    state
        .node_service
        .create_mention(&request.source_id, &request.target_id)
        .await
        .map_err(map_node_service_error)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn delete_mention(
    State(state): State<AppState>,
    Json(request): Json<MentionRequest>,
) -> ApiStatusResult {
    state
        .node_service
        .delete_mention(&request.source_id, &request.target_id)
        .await
        .map_err(map_node_service_error)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn mention_autocomplete(
    State(state): State<AppState>,
    Json(request): Json<MentionAutocompleteRequest>,
) -> ApiResult<Vec<Node>> {
    // Use NodeService to search nodes by content
    // Case-insensitive search is handled by SurrealStore using string::lowercase()
    let nodes = state
        .node_service
        .store()
        .search_nodes_by_content(&request.query, request.limit)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new(
                    "SEARCH_ERROR",
                    format!("Search failed: {}", e),
                )),
            )
        })?;

    Ok(Json(nodes))
}

async fn get_outgoing_mentions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Vec<String>> {
    let mentions = state
        .node_service
        .get_mentions(&id)
        .await
        .map_err(map_node_service_error)?;

    Ok(Json(mentions))
}

async fn get_incoming_mentions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Vec<String>> {
    let mentions = state
        .node_service
        .get_mentioned_by(&id)
        .await
        .map_err(map_node_service_error)?;

    Ok(Json(mentions))
}

async fn get_mentioning_containers(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Vec<String>> {
    let container_ids = state
        .node_service
        .get_mentioning_containers(&id)
        .await
        .map_err(map_node_service_error)?;

    Ok(Json(container_ids))
}

/// Get all schema definitions with typed fields
///
/// Retrieves all schema definitions stored in the database. This endpoint
/// is used by the frontend SchemaPluginLoader to auto-register custom entity
/// plugins on application startup.
///
/// Returns SchemaNode[] with typed top-level fields (isCore, schemaVersion, description, fields).
///
/// # HTTP Endpoint
/// ```text
/// GET /api/schemas
/// ```
///
/// # Response (200 OK)
/// ```json
/// [
///   {
///     "id": "task",
///     "nodeType": "schema",
///     "content": "Task",
///     "version": 1,
///     "isCore": true,
///     "schemaVersion": 1,
///     "description": "Task schema",
///     "fields": [...]
///   }
/// ]
/// ```
///
/// # Errors
/// - `500 INTERNAL SERVER ERROR`: Database error
async fn get_all_schemas(State(state): State<AppState>) -> ApiResult<Vec<SchemaNode>> {
    // Query all schema nodes via NodeService
    let query = nodespace_core::NodeQuery {
        node_type: Some("schema".to_string()),
        ..Default::default()
    };
    let nodes = state
        .node_service
        .query_nodes_simple(query)
        .await
        .map_err(map_node_service_error)?;

    // Convert to SchemaNode for typed serialization
    let schema_nodes: Vec<SchemaNode> = nodes
        .into_iter()
        .filter_map(|node| SchemaNode::from_node(node).ok())
        .collect();

    Ok(Json(schema_nodes))
}

/// Get schema by ID with typed fields
///
/// Retrieves the complete schema including all fields, protection levels,
/// and metadata. Returns SchemaNode with typed top-level fields.
///
/// # HTTP Endpoint
/// ```text
/// GET /api/schemas/:id
/// ```
///
/// # Parameters
/// - `id`: Schema ID (e.g., "task", "person", "project")
///
/// # Response (200 OK)
/// Returns SchemaNode with typed fields (isCore, schemaVersion, description, fields).
///
/// # Errors
/// - `404 NOT FOUND`: Schema doesn't exist
/// - `500 INTERNAL SERVER ERROR`: Database error
async fn get_schema(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<SchemaNode> {
    let schema_node = state
        .node_service
        .get_schema_node(&id)
        .await
        .map_err(map_node_service_error)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiError::new(
                    "SCHEMA_NOT_FOUND",
                    format!("Schema '{}' not found", id),
                )),
            )
        })?;

    // Return SchemaNode directly - custom Serialize impl outputs typed fields
    Ok(Json(schema_node))
}

// NOTE: Schema mutation functions (add_schema_field, remove_schema_field,
// extend_schema_enum, remove_schema_enum_value) removed in Issue #690.
// These were not used by any UI components.
