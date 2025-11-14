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
    response::Json,
    routing::{delete, get, patch, post},
    Router,
};
use chrono::Utc;
use nodespace_core::{
    db::HttpStore,
    models::{Node, NodeFilter, NodeUpdate},
    services::NodeService,
};
use serde::Deserialize;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

// Type alias for HTTP client NodeService
type HttpNodeService = NodeService<surrealdb::engine::remote::http::Client>;

/// Application state shared across handlers
#[derive(Clone)]
struct AppState {
    node_service: Arc<HttpNodeService>,
}

/// Standard error response format
type ApiResult<T> = Result<Json<T>, (StatusCode, String)>;
type ApiStatusResult = Result<StatusCode, (StatusCode, String)>;

/// Update node request with OCC version
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateNodeRequest {
    /// Expected version for optimistic concurrency control
    /// Note: Currently not used - NodeService handles OCC internally
    #[allow(dead_code)]
    pub version: i64,
    /// Fields to update
    #[serde(flatten)]
    pub update: NodeUpdate,
}

/// Mention creation/deletion request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MentionRequest {
    pub source_id: String,
    pub target_id: String,
}

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
/// - `container_node_id`: Optional container for spatial/collection relationships.
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
    pub parent_id: Option<String>,
    pub container_node_id: Option<String>,
    pub before_sibling_id: Option<String>,
    pub properties: serde_json::Value,
    pub embedding_vector: Option<Vec<u8>>,
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
    let store =
        match HttpStore::new_http("127.0.0.1:8000", "nodespace", "nodes", "root", "root").await {
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
    println!("🧠 Initializing NodeService...");
    let node_service = match NodeService::new(Arc::clone(&store)) {
        Ok(s) => {
            println!("✅ NodeService initialized");
            Arc::new(s)
        }
        Err(e) => {
            eprintln!("❌ Failed to initialize NodeService: {}", e);
            return Err(e.into());
        }
    };

    let state = AppState { node_service };

    // Build HTTP router
    let app = Router::new()
        // Health check (useful for testing)
        .route("/health", get(health_check))
        // Database initialization (no-op - already initialized on startup)
        .route("/api/database/init", post(init_database))
        // Node CRUD endpoints
        .route("/api/nodes", post(create_node))
        .route("/api/nodes/:id", get(get_node))
        .route("/api/nodes/:id", patch(update_node))
        .route("/api/nodes/:id", delete(delete_node))
        // Query endpoints
        .route("/api/nodes/:id/children", get(get_children))
        .route("/api/query", post(query_nodes))
        .route(
            "/api/nodes/by-container/:container_id",
            get(get_nodes_by_container),
        )
        // Mention endpoints
        .route("/api/mentions", post(create_mention))
        .route("/api/mentions", delete(delete_mention))
        .route(
            "/api/nodes/:id/mentions/outgoing",
            get(get_outgoing_mentions),
        )
        .route(
            "/api/nodes/:id/mentions/incoming",
            get(get_incoming_mentions),
        )
        .route(
            "/api/nodes/:id/mentions/containers",
            get(get_mentioning_containers),
        )
        // Schema endpoints
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
    println!("   NodeService → SurrealDB (port 8000)");
    println!("   Surrealist can connect to port 8000\n");

    axum::serve(listener, app).await?;

    Ok(())
}

// === Handler Functions ===

async fn health_check() -> &'static str {
    "OK"
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
    Json(req): Json<CreateNodeRequest>,
) -> ApiResult<String> {
    // Convert CreateNodeRequest to Node by adding timestamps and version
    // The frontend sends a partial node without these fields
    let now = Utc::now();
    let node = Node {
        id: req.id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        node_type: req.node_type,
        content: req.content,
        parent_id: req.parent_id,
        container_node_id: req.container_node_id,
        before_sibling_id: req.before_sibling_id,
        version: 1, // New nodes always start at version 1
        created_at: now,
        modified_at: now,
        properties: req.properties,
        embedding_vector: req.embedding_vector,
        mentions: req.mentions,
        mentioned_by: vec![], // New nodes have no backlinks initially
    };

    let id = state
        .node_service
        .create_node(node)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(id))
}

async fn get_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Option<Node>> {
    // ⭐ This call includes ALL business logic:
    // - Virtual date node creation (YYYY-MM-DD format)
    // - populate_mentions() (backlinks)
    // - backfill_schema_version() (adds _schema_version to properties)
    // - apply_lazy_migration() (upgrades nodes to latest schema)
    let node = state
        .node_service
        .get_node(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(node))
}

async fn update_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateNodeRequest>,
) -> ApiStatusResult {
    // NodeService.update_node doesn't take version - it handles OCC internally
    // The version in the request is ignored for now
    state
        .node_service
        .update_node(&id, request.update)
        .await
        .map_err(|e| {
            let error_str = e.to_string();

            // Map NodeService errors to HTTP status codes
            // This helps frontend handle errors correctly
            if error_str.contains("version") || error_str.contains("Version conflict") {
                (StatusCode::CONFLICT, error_str) // 409 Conflict
            } else if error_str.contains("not found") {
                (StatusCode::NOT_FOUND, error_str) // 404 Not Found
            } else if error_str.contains("validation") {
                (StatusCode::BAD_REQUEST, error_str) // 400 Bad Request
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, error_str) // 500 Internal Error
            }
        })?;

    Ok(StatusCode::NO_CONTENT) // 204 No Content (success)
}

async fn delete_node(State(state): State<AppState>, Path(id): Path<String>) -> ApiStatusResult {
    state.node_service.delete_node(&id).await.map_err(|e| {
        let error_str = e.to_string();
        if error_str.contains("not found") {
            (StatusCode::NOT_FOUND, error_str)
        } else {
            (StatusCode::INTERNAL_SERVER_ERROR, error_str)
        }
    })?;

    Ok(StatusCode::NO_CONTENT)
}

async fn get_children(
    State(state): State<AppState>,
    Path(parent_id): Path<String>,
) -> ApiResult<Vec<Node>> {
    let children = state
        .node_service
        .get_children(&parent_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(children))
}

async fn query_nodes(
    State(state): State<AppState>,
    Json(filter): Json<NodeFilter>,
) -> ApiResult<Vec<Node>> {
    let nodes = state
        .node_service
        .query_nodes(filter)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(nodes))
}

async fn get_nodes_by_container(
    State(state): State<AppState>,
    Path(container_id): Path<String>,
) -> ApiResult<Vec<Node>> {
    let nodes = state
        .node_service
        .get_nodes_by_container_id(&container_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(nodes))
}

async fn create_mention(
    State(state): State<AppState>,
    Json(request): Json<MentionRequest>,
) -> ApiStatusResult {
    state
        .node_service
        .create_mention(&request.source_id, &request.target_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

async fn get_outgoing_mentions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Vec<String>> {
    let mentions = state
        .node_service
        .get_mentions(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(container_ids))
}

async fn get_schema(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    // Get the schema node (schemas are stored as nodes with id = node_type)
    let schema_node = state
        .node_service
        .get_node(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Return just the properties (SchemaDefinition), not the whole node
    // Frontend expects SchemaDefinition, not Node
    match schema_node {
        Some(node) => Ok(Json(node.properties)),
        None => Err((StatusCode::NOT_FOUND, format!("Schema not found: {}", id))),
    }
}
