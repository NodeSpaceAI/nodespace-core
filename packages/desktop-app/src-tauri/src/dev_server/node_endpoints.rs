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
/// This endpoint only prepares the database directory structure.
/// The actual database initialization happens when the dev-server binary starts.
/// Tests should ensure the dev-server is running before calling this endpoint.
async fn init_database(
    State(state): State<AppState>,
    Query(params): Query<InitDbQuery>,
) -> Result<Json<InitDbResponse>, HttpError> {
    use std::path::PathBuf;
    use tokio::fs;

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
            .join("nodespace-dev.db")
    };

    // Ensure database directory exists
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| HttpError::from_anyhow(e.into(), "FILESYSTEM_ERROR"))?;
    }

    let db_path_str = db_path
        .to_str()
        .ok_or_else(|| HttpError::new("Invalid database path", "PATH_ERROR"))?
        .to_string();

    // Create new DatabaseService and NodeService for this database
    use nodespace_core::{DatabaseService, NodeService};
    let new_db = DatabaseService::new(db_path.clone())
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "DATABASE_INIT_ERROR"))?;

    let new_node_service = NodeService::new(new_db.clone())
        .map_err(|e| HttpError::from_anyhow(e.into(), "NODE_SERVICE_INIT_ERROR"))?;

    // Replace the services in AppState using RwLock
    {
        let mut db_lock = state.db.write().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire database write lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        *db_lock = Arc::new(new_db);
    }
    {
        let mut ns_lock = state.node_service.write().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node service write lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        *ns_lock = Arc::new(new_node_service);
    }

    tracing::info!("🔄 Database SWAPPED to: {}", db_path_str);

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
    use chrono::Utc;

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

    let now = Utc::now();
    let full_node = Node {
        id: node.id.clone(),
        node_type: node.node_type,
        content: node.content,
        parent_id: node.parent_id,
        container_node_id: node.container_node_id,
        before_sibling_id: node.before_sibling_id,
        created_at: now,
        modified_at: now,
        properties: node.properties,
        embedding_vector: node.embedding_vector,
        mentions: Vec::new(),
        mentioned_by: Vec::new(),
    };

    // Access node_service through RwLock
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node service read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };
    node_service.create_node(full_node).await.map_err(|e| {
        tracing::error!("❌ Node creation failed for {}: {:?}", node.id, e);
        HttpError::from_anyhow(e.into(), "NODE_SERVICE_ERROR")
    })?;

    tracing::debug!("✅ Created node: {}", node.id);

    Ok(Json(node.id))
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
/// JSON object with fields to update (partial update supported)
///
/// # Example
///
/// ```bash
/// curl -X PATCH http://localhost:3001/api/nodes/test-node-1 \
///   -H "Content-Type: application/json" \
///   -d '{"content": "Updated content"}'
/// ```
async fn update_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(update): Json<NodeUpdate>,
) -> Result<StatusCode, HttpError> {
    tracing::info!(
        "📝 UPDATE request for node: {} with update: {:?}",
        id,
        update
    );
    let node_service = {
        let lock = state.node_service.read().map_err(|e| {
            HttpError::new(
                format!("Failed to acquire node service read lock: {}", e),
                "LOCK_ERROR",
            )
        })?;
        Arc::clone(&*lock)
    };
    let result = node_service.update_node(&id, update).await;

    match result {
        Ok(_) => {
            tracing::info!("✅ Updated node: {}", id);
            Ok(StatusCode::OK)
        }
        Err(e) => {
            tracing::error!("❌ Node update failed for {}: {:?}", id, e);
            Err(HttpError::from_anyhow(e.into(), "NODE_SERVICE_ERROR"))
        }
    }
}

/// Delete a node by ID
///
/// # Path Parameters
///
/// - `id`: Node ID
///
/// # Example
///
/// ```bash
/// curl -X DELETE http://localhost:3001/api/nodes/test-node-1
/// ```
async fn delete_node(
    State(state): State<AppState>,
    Path(id): Path<String>,
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
    let result = node_service
        .delete_node(&id)
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "NODE_SERVICE_ERROR"))?;

    if result.existed {
        tracing::debug!("✅ Deleted node: {}", id);
    } else {
        tracing::debug!("✅ Delete idempotent (node not found): {}", id);
    }

    Ok(StatusCode::OK)
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
