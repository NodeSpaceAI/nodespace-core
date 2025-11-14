//! Phase 1 MVP Node Endpoints for HTTP Dev Server
//!
//! This module implements the basic node CRUD operations needed for
//! Phase 1 testing (#210). These endpoints mirror the corresponding
//! Tauri IPC commands in `commands/nodes.rs` and `commands/db.rs`.
//!
//! # Endpoints
//!
//! - `GET /api/health` - Health check endpoint
//! - `POST /api/database/init?db_path=` - Initialize database
//! - `POST /api/nodes` - Create a new node
//! - `GET /api/nodes/:id` - Get a node by ID
//! - `PATCH /api/nodes/:id` - Update a node
//! - `DELETE /api/nodes/:id` - Delete a node
//! - `GET /api/nodes/:id/children` - Get child nodes

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, patch, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::commands::nodes::CreateNodeInput;
use crate::dev_server::{AppState, HttpError};
use nodespace_core::operations::CreateNodeParams;
use nodespace_core::{Node, NodeUpdate};

/// Query parameters for database initialization
#[derive(Debug, Deserialize)]
pub struct InitDbQuery {
    /// Optional custom database path
    /// If not provided, uses default: ~/.nodespace/database/nodespace-dev.db
    db_path: Option<String>,
}

/// Response from database initialization
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitDbResponse {
    pub db_path: String,
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub status: String,
    pub version: String,
}

/// Update node request with OCC version
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNodeRequest {
    /// Expected version for optimistic concurrency control
    pub version: i64,
    /// Fields to update
    #[serde(flatten)]
    pub update: NodeUpdate,
}

/// Health check endpoint
///
/// Returns server status and version information. Useful for verifying
/// that the dev server is running before executing tests.
///
/// # Example
///
/// ```bash
/// curl http://localhost:3001/api/health
/// ```
async fn health_check() -> Json<HealthStatus> {
    Json(HealthStatus {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// Initialize database with optional custom path
///
/// # Query Parameters
///
/// - `db_path` (optional): Custom database file path
///
/// # Example
///
/// ```bash
/// # Use default path
/// curl -X POST http://localhost:3001/api/database/init
///
/// # Use custom path for testing
/// curl -X POST "http://localhost:3001/api/database/init?db_path=/tmp/test.db"
/// ```
///
/// # Note
///
/// In HTTP mode, the database is already initialized when the dev-server starts.
/// This endpoint just returns the current database path for frontend compatibility.
/// It only allows switching to a DIFFERENT database path for testing purposes.
async fn init_database(
    State(_state): State<AppState>,
    Query(params): Query<InitDbQuery>,
) -> Result<Json<InitDbResponse>, HttpError> {
    use std::path::PathBuf;

    // Determine database path
    let db_path = if let Some(custom_path) = params.db_path {
        PathBuf::from(custom_path)
    } else {
        // Default: ~/.nodespace/database/nodespace-dev.db
        let home_dir = dirs::home_dir()
            .ok_or_else(|| HttpError::new("Failed to get home directory", "PATH_ERROR"))?;

        home_dir
            .join(".nodespace")
            .join("database")
            .join("nodespace-dev")
    };

    let db_path_str = db_path
        .to_str()
        .ok_or_else(|| HttpError::new("Invalid database path", "PATH_ERROR"))?
        .to_string();

    // In HTTP mode, the database is already initialized on server startup
    // Just return success with the path (frontend expects this response)
    tracing::info!("✅ Database already initialized at: {}", db_path_str);

    Ok(Json(InitDbResponse {
        db_path: db_path_str,
    }))
}

/// Create a new node
///
/// # Request Body
///
/// JSON object with node data (matches `CreateNodeInput` from Tauri commands)
///
/// # Example
///
/// ```bash
/// curl -X POST http://localhost:3001/api/nodes \
///   -H "Content-Type: application/json" \
///   -d '{
///     "id": "test-node-1",
///     "nodeType": "text",
///     "content": "Hello World",
///     "parentId": null,
///     "containerNodeId": null,
///     "beforeSiblingId": null,
///     "properties": {}
///   }'
/// ```
async fn create_node(
    State(state): State<AppState>,
    Json(node): Json<CreateNodeInput>,
) -> Result<Json<String>, HttpError> {
    use crate::constants::ALLOWED_NODE_TYPES;

    // Acquire write lock to serialize database writes (Issue #266)
    let _write_guard = state.write_lock.lock().await;

    // Validate node type (same as Tauri command)
    if !ALLOWED_NODE_TYPES.contains(&node.node_type.as_str()) {
        return Err(HttpError::with_details(
            format!(
                "Only text, task, and date nodes are supported. Got: {}",
                node.node_type
            ),
            "INVALID_NODE_TYPE",
            format!(
                "Allowed types: {:?}, received: '{}'",
                ALLOWED_NODE_TYPES, node.node_type
            ),
        ));
    }

    // Use NodeOperations for OCC enforcement (matches Tauri command architecture)
    let operations = {
        let lock = state.node_operations.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node operations read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };

    // Build CreateNodeParams matching Tauri command structure
    let params = CreateNodeParams {
        id: Some(node.id.clone()),
        node_type: node.node_type,
        content: node.content,
        parent_id: node.parent_id,
        container_node_id: node.container_node_id,
        before_sibling_id: node.before_sibling_id,
        properties: node.properties,
    };

    // Create via NodeOperations (with OCC enforcement)
    // Returns the node ID as a String
    let node_id = operations.create_node(params).await?;

    tracing::debug!("✅ Created node: {}", node_id);

    Ok(Json(node_id))
    // Write lock is automatically released when _write_guard goes out of scope
}

/// Get a node by ID
///
/// # Path Parameters
///
/// - `id`: Node ID
///
/// # Example
///
/// ```bash
/// curl http://localhost:3001/api/nodes/test-node-1
/// ```
async fn get_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Option<Node>>, HttpError> {
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node service read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };

    let node = node_service
        .get_node(&id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "NODE_SERVICE_ERROR"))?;

    Ok(Json(node))
}

/// Update an existing node
///
/// # Path Parameters
///
/// - `id`: Node ID
///
/// # Request Body
///
/// JSON object with version and fields to update (OCC enforced)
///
/// # Example
///
/// ```bash
/// curl -X PATCH http://localhost:3001/api/nodes/test-node-1 \
///   -H "Content-Type: application/json" \
///   -d '{"version": 1, "content": "Updated content"}'
/// ```
///
/// Returns 409 Conflict if version mismatch (version conflict)
/// Returns updated Node with new version to keep frontend in sync
async fn update_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateNodeRequest>,
) -> Result<Json<Node>, HttpError> {
    // Acquire write lock to serialize database writes (Issue #266)
    let _write_guard = state.write_lock.lock().await;

    tracing::info!(
        "📝 UPDATE request for node: {} version: {} with update: {:?}",
        id,
        request.version,
        request.update
    );

    // Use NodeOperations for OCC enforcement (matches Tauri command architecture)
    let operations = {
        let lock = state.node_operations.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node operations read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };

    // Update via NodeOperations with version check
    // IMPORTANT: Return the updated Node so frontend can refresh its local version
    // Use update_node_with_hierarchy to handle both content AND hierarchy changes
    let updated_node = operations
        .update_node_with_hierarchy(&id, request.version, request.update)
        .await?;

    tracing::info!(
        "✅ Updated node: {} (new version: {})",
        id,
        updated_node.version
    );
    Ok(Json(updated_node))
    // Write lock is automatically released when _write_guard goes out of scope
}

/// Delete a node by ID with version check
///
/// # Path Parameters
///
/// - `id`: Node ID
///
/// # Query Parameters
///
/// - `version`: Expected version for OCC
///
/// # Example
///
/// ```bash
/// curl -X DELETE "http://localhost:3001/api/nodes/test-node-1?version=1"
/// ```
///
/// Returns 409 Conflict if version mismatch
async fn delete_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<StatusCode, HttpError> {
    // Acquire write lock to serialize database writes (Issue #266)
    let _write_guard = state.write_lock.lock().await;

    // Extract version from query parameters
    let version = params
        .get("version")
        .ok_or_else(|| HttpError::new("Missing 'version' query parameter", "INVALID_INPUT"))?
        .parse::<i64>()
        .map_err(|e| {
            HttpError::with_details(
                "Invalid version parameter",
                "INVALID_INPUT",
                format!("Version must be a valid integer: {}", e),
            )
        })?;

    // Use NodeOperations for OCC enforcement (matches Tauri command architecture)
    let operations = {
        let lock = state.node_operations.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node operations read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };

    let result = operations.delete_node(&id, version).await?;

    if result.existed {
        tracing::debug!("✅ Deleted node: {}", id);
    } else {
        tracing::debug!("✅ Delete idempotent (node not found): {}", id);
    }

    Ok(StatusCode::OK)
    // Write lock is automatically released when _write_guard goes out of scope
}

/// Get child nodes of a parent node
///
/// # Path Parameters
///
/// - `parent_id`: Parent node ID
///
/// # Example
///
/// ```bash
/// curl http://localhost:3001/api/nodes/parent-node-1/children
/// ```
async fn get_children(
    State(state): State<AppState>,
    Path(parent_id): Path<String>,
) -> Result<Json<Vec<Node>>, HttpError> {
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node service read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };

    let children = node_service
        .get_children(&parent_id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "NODE_SERVICE_ERROR"))?;

    Ok(Json(children))
}

/// Create router with all Phase 1 node endpoints
///
/// This function is called by the main router in `mod.rs` to register
/// all Phase 1 endpoints.
pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health_check))
        .route("/api/database/init", post(init_database))
        .route("/api/nodes", post(create_node))
        .route("/api/nodes/:id", get(get_node))
        .route("/api/nodes/:id", patch(update_node))
        .route("/api/nodes/:id", delete(delete_node))
        .route("/api/nodes/:id/children", get(get_children))
        .with_state(state)
}
