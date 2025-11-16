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
    models::{
        schema::{ProtectionLevel, SchemaDefinition, SchemaField},
        Node, NodeFilter, NodeUpdate,
    },
    services::{NodeService, NodeServiceError, SchemaService},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use uuid::Uuid;

// Type alias for HTTP client types
type HttpNodeService = NodeService<surrealdb::engine::remote::http::Client>;
type HttpSchemaService = SchemaService<surrealdb::engine::remote::http::Client>;

/// Application state shared across handlers
#[derive(Clone)]
struct AppState {
    node_service: Arc<HttpNodeService>,
    schema_service: Arc<HttpSchemaService>,
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

/// Result of schema field mutation operations
///
/// Matches Tauri's SchemaFieldResult to ensure consistent responses
/// between HTTP and Tauri entry points.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaFieldResult {
    /// ID of the modified schema
    pub schema_id: String,
    /// New schema version after the operation
    pub new_version: i32,
}

/// Standard result types for HTTP handlers
type ApiResult<T> = Result<Json<T>, (StatusCode, Json<ApiError>)>;
type ApiStatusResult = Result<StatusCode, (StatusCode, Json<ApiError>)>;
type ApiSchemaResult = Result<Json<SchemaFieldResult>, (StatusCode, Json<ApiError>)>;

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

/// Add schema field request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddSchemaFieldRequest {
    pub name: String,
    pub field_type: String,
    pub indexed: bool,
    pub required: Option<bool>,
    pub default: Option<serde_json::Value>,
    pub description: Option<String>,
    pub item_type: Option<String>,
    pub enum_values: Option<Vec<String>>,
    pub extensible: Option<bool>,
}

/// Extend enum field request
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtendEnumRequest {
    pub value: String,
}

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

/// Maps NodeServiceError to HTTP status code and structured ApiError for schema operations
///
/// Provides more specific error codes for schema-related operations.
fn map_schema_error(err: NodeServiceError) -> (StatusCode, Json<ApiError>) {
    let error_str = err.to_string();

    let (status, code) = if error_str.contains("not found") {
        (StatusCode::NOT_FOUND, "SCHEMA_NOT_FOUND")
    } else if error_str.contains("already exists") {
        (StatusCode::BAD_REQUEST, "FIELD_ALREADY_EXISTS")
    } else if error_str.contains("validation") || error_str.contains("protected") {
        (StatusCode::BAD_REQUEST, "VALIDATION_ERROR")
    } else if error_str.contains("version") || error_str.contains("conflict") {
        (StatusCode::CONFLICT, "VERSION_CONFLICT")
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, "SCHEMA_SERVICE_ERROR")
    };

    (
        status,
        Json(ApiError::new(
            code,
            format!("Schema operation failed: {}", error_str),
        )),
    )
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

    // Initialize SchemaService
    println!("🔖 Initializing SchemaService...");
    let schema_service = Arc::new(SchemaService::new(node_service.clone()));
    println!("✅ SchemaService initialized");

    let state = AppState {
        node_service,
        schema_service,
    };

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
            get(get_children_by_parent),
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
        .route("/api/schemas/:id/fields", post(add_schema_field))
        .route("/api/schemas/:id/fields/:name", delete(remove_schema_field))
        .route(
            "/api/schemas/:id/fields/:name/enum",
            post(extend_schema_enum),
        )
        .route(
            "/api/schemas/:id/fields/:name/enum/:value",
            delete(remove_schema_enum_value),
        )
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
    tracing::debug!(
        "create_node request: parent_id={:?}, container_node_id={:?}",
        req.parent_id,
        req.container_node_id
    );

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
        .map_err(map_node_service_error)?;

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
        .map_err(map_node_service_error)?;

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
        .map_err(map_node_service_error)?;

    Ok(StatusCode::NO_CONTENT) // 204 No Content (success)
}

async fn delete_node(State(state): State<AppState>, Path(id): Path<String>) -> ApiStatusResult {
    state
        .node_service
        .delete_node(&id)
        .await
        .map_err(map_node_service_error)?;

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
        .map_err(map_node_service_error)?;

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
        .map_err(map_node_service_error)?;

    Ok(Json(nodes))
}

async fn get_children_by_parent(
    State(state): State<AppState>,
    Path(parent_id): Path<String>,
) -> ApiResult<Vec<Node>> {
    // Phase 5 (Issue #511): Use get_children instead of get_nodes_by_container_id
    // Graph edges replace container_node_id field
    let nodes = state
        .node_service
        .get_children(&parent_id)
        .await
        .map_err(map_node_service_error)?;

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

/// Get schema definition by schema ID
///
/// Retrieves the complete schema definition including all fields,
/// protection levels, and metadata. Uses SchemaService for proper
/// schema validation and retrieval.
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
/// ```json
/// {
///   "id": "task",
///   "name": "Task",
///   "version": 1,
///   "fields": [
///     {
///       "name": "status",
///       "fieldType": "enum",
///       "protection": "core",
///       "coreValues": ["todo", "in_progress", "done"],
///       "indexed": true
///     }
///   ]
/// }
/// ```
///
/// # Errors
/// - `404 NOT FOUND`: Schema doesn't exist
/// - `500 INTERNAL SERVER ERROR`: Database error
async fn get_schema(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<SchemaDefinition> {
    let schema = state
        .schema_service
        .get_schema(&id)
        .await
        .map_err(map_schema_error)?;

    Ok(Json(schema))
}

/// Add a new field to a schema
///
/// Creates a new user-protected field in the specified schema. Only user
/// fields can be added through this endpoint - core and system fields
/// require schema definition changes.
///
/// The schema version is automatically incremented on success, and the
/// new version is returned in the response (matching Tauri behavior).
///
/// # HTTP Endpoint
/// ```text
/// POST /api/schemas/:id/fields
/// Content-Type: application/json
/// ```
///
/// # Parameters
/// - `id`: Schema ID to modify (e.g., "task", "person")
///
/// # Request Body
/// ```json
/// {
///   "name": "custom:priority",
///   "fieldType": "number",
///   "indexed": false,
///   "required": false,
///   "default": 0,
///   "description": "Task priority level"
/// }
/// ```
///
/// # Response (200 OK)
/// ```json
/// {
///   "schemaId": "task",
///   "newVersion": 2
/// }
/// ```
///
/// # Errors
/// - `404 NOT FOUND`: Schema doesn't exist
/// - `400 BAD REQUEST`: Field already exists, validation error, or protection violation
/// - `500 INTERNAL SERVER ERROR`: Database error
///
/// # Field Naming
/// User fields MUST use namespace prefixes (e.g., `custom:`, `org:`, `plugin:`)
/// to prevent conflicts with future core fields.
async fn add_schema_field(
    State(state): State<AppState>,
    Path(schema_id): Path<String>,
    Json(request): Json<AddSchemaFieldRequest>,
) -> ApiSchemaResult {
    // Build SchemaField from request
    let field = SchemaField {
        name: request.name,
        field_type: request.field_type,
        protection: ProtectionLevel::User, // Only user fields can be added
        core_values: None,                 // User fields don't have core values
        user_values: request.enum_values,
        indexed: request.indexed,
        required: request.required,
        extensible: request.extensible,
        default: request.default,
        description: request.description,
        item_type: request.item_type,
        fields: None,      // Nested fields not supported via this API yet
        item_fields: None, // Array of objects not supported via this API yet
    };

    // Add field to schema
    state
        .schema_service
        .add_field(&schema_id, field)
        .await
        .map_err(map_schema_error)?;

    // Get updated schema to return new version (matches Tauri behavior)
    let updated_schema = state
        .schema_service
        .get_schema(&schema_id)
        .await
        .map_err(map_schema_error)?;

    Ok(Json(SchemaFieldResult {
        schema_id,
        new_version: updated_schema.version as i32,
    }))
}

/// Remove a field from a schema
///
/// Removes a user-protected field from the specified schema. Core and
/// system fields cannot be removed through this endpoint.
///
/// The schema version is automatically incremented on success.
///
/// # HTTP Endpoint
/// ```text
/// DELETE /api/schemas/:id/fields/:name
/// ```
///
/// # Parameters
/// - `id`: Schema ID (e.g., "task")
/// - `name`: Field name to remove (e.g., "custom:priority")
///
/// # Response (200 OK)
/// ```json
/// {
///   "schemaId": "task",
///   "newVersion": 3
/// }
/// ```
///
/// # Errors
/// - `404 NOT FOUND`: Schema or field doesn't exist
/// - `400 BAD REQUEST`: Cannot remove core/system field (protection violation)
/// - `500 INTERNAL SERVER ERROR`: Database error
async fn remove_schema_field(
    State(state): State<AppState>,
    Path((schema_id, field_name)): Path<(String, String)>,
) -> ApiSchemaResult {
    // Remove field from schema
    state
        .schema_service
        .remove_field(&schema_id, &field_name)
        .await
        .map_err(map_schema_error)?;

    // Get updated schema to return new version (matches Tauri behavior)
    let updated_schema = state
        .schema_service
        .get_schema(&schema_id)
        .await
        .map_err(map_schema_error)?;

    Ok(Json(SchemaFieldResult {
        schema_id,
        new_version: updated_schema.version as i32,
    }))
}

/// Extend an enum field with a new user value
///
/// Adds a new user value to an extensible enum field. Only works on
/// fields with `extensible: true` and adds to `userValues` array
/// (core values cannot be modified).
///
/// The schema version is automatically incremented on success.
///
/// # HTTP Endpoint
/// ```text
/// POST /api/schemas/:id/fields/:name/enum
/// Content-Type: application/json
/// ```
///
/// # Parameters
/// - `id`: Schema ID (e.g., "task")
/// - `name`: Enum field name (e.g., "status")
///
/// # Request Body
/// ```json
/// {
///   "value": "blocked"
/// }
/// ```
///
/// # Response (200 OK)
/// ```json
/// {
///   "schemaId": "task",
///   "newVersion": 4
/// }
/// ```
///
/// # Errors
/// - `404 NOT FOUND`: Schema or field doesn't exist
/// - `400 BAD REQUEST`: Field is not an enum, not extensible, or value already exists
/// - `500 INTERNAL SERVER ERROR`: Database error
async fn extend_schema_enum(
    State(state): State<AppState>,
    Path((schema_id, field_name)): Path<(String, String)>,
    Json(request): Json<ExtendEnumRequest>,
) -> ApiSchemaResult {
    // Extend enum field with new user value
    state
        .schema_service
        .extend_enum_field(&schema_id, &field_name, request.value)
        .await
        .map_err(map_schema_error)?;

    // Get updated schema to return new version (matches Tauri behavior)
    let updated_schema = state
        .schema_service
        .get_schema(&schema_id)
        .await
        .map_err(map_schema_error)?;

    Ok(Json(SchemaFieldResult {
        schema_id,
        new_version: updated_schema.version as i32,
    }))
}

/// Remove a user value from an enum field
///
/// Removes a user value from an enum field's `userValues` array. Core
/// values cannot be removed through this endpoint.
///
/// The schema version is automatically incremented on success.
///
/// # HTTP Endpoint
/// ```text
/// DELETE /api/schemas/:id/fields/:name/enum/:value
/// ```
///
/// # Parameters
/// - `id`: Schema ID (e.g., "task")
/// - `name`: Enum field name (e.g., "status")
/// - `value`: Value to remove (e.g., "blocked")
///
/// # Response (200 OK)
/// ```json
/// {
///   "schemaId": "task",
///   "newVersion": 5
/// }
/// ```
///
/// # Errors
/// - `404 NOT FOUND`: Schema, field, or value doesn't exist
/// - `400 BAD REQUEST`: Cannot remove core value (protection violation)
/// - `500 INTERNAL SERVER ERROR`: Database error
async fn remove_schema_enum_value(
    State(state): State<AppState>,
    Path((schema_id, field_name, value)): Path<(String, String, String)>,
) -> ApiSchemaResult {
    // Remove user value from enum field
    state
        .schema_service
        .remove_enum_value(&schema_id, &field_name, &value)
        .await
        .map_err(map_schema_error)?;

    // Get updated schema to return new version (matches Tauri behavior)
    let updated_schema = state
        .schema_service
        .get_schema(&schema_id)
        .await
        .map_err(map_schema_error)?;

    Ok(Json(SchemaFieldResult {
        schema_id,
        new_version: updated_schema.version as i32,
    }))
}
