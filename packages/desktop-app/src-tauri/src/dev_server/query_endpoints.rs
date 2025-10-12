//! Phase 2 Query Endpoints
//!
//! Additional endpoints for querying nodes by various criteria.
//! These endpoints extend the Phase 1 CRUD operations with query capabilities.
//!
//! # Endpoints
//!
//! - `GET /api/nodes/query` - Query nodes by parent, container, or other criteria
//! - `GET /api/nodes/by-origin/:origin_id` - Get nodes by origin ID
//!
//! # Usage
//!
//! ```bash
//! # Query root nodes (parent_id = null)
//! curl "http://localhost:3001/api/nodes/query?parent_id=null"
//!
//! # Query children of a specific node
//! curl "http://localhost:3001/api/nodes/query?parent_id=node-123"
//!
//! # Query by container
//! curl "http://localhost:3001/api/nodes/query?container_id=container-1"
//! ```

use crate::dev_server::AppState;
use axum::{
    extract::{Path, Query, State},
    response::Json,
    routing::get,
    Router,
};
use nodespace_core::{Node, NodeFilter};
use serde::Deserialize;

use super::http_error::HttpError;

/// Validate node ID format
///
/// Node IDs can be:
/// - UUIDs (e.g., "550e8400-e29b-41d4-a716-446655440000")
/// - Date strings for DateNodes (e.g., "2025-01-15")
fn is_valid_node_id(id: &str) -> bool {
    // Try parsing as UUID
    if uuid::Uuid::parse_str(id).is_ok() {
        return true;
    }

    // Try parsing as date (YYYY-MM-DD format for DateNodes)
    if chrono::NaiveDate::parse_from_str(id, "%Y-%m-%d").is_ok() {
        return true;
    }

    false
}

/// Query parameters for simple node queries
///
/// Supports querying by parent_id and/or container_id.
/// Use "null" string for parent_id to query root nodes.
#[derive(Debug, Deserialize)]
pub struct QueryNodesParams {
    /// Container ID to filter by (optional)
    pub container_id: Option<String>,

    /// Parent ID to filter by (optional)
    /// Use "null" string to query root nodes (parentId = null)
    pub parent_id: Option<String>,
}

/// Query nodes by parent and/or container
///
/// # Query Parameters
///
/// - `parent_id` (optional): Filter by parent ID. Use "null" to get root nodes (parentId = null).
/// - `container_id` (optional): Filter by container ID.
///
/// # Examples
///
/// ```bash
/// # Get all root nodes
/// curl "http://localhost:3001/api/nodes/query?parent_id=null"
///
/// # Get children of a specific node
/// curl "http://localhost:3001/api/nodes/query?parent_id=abc-123"
///
/// # Get nodes in a container
/// curl "http://localhost:3001/api/nodes/query?container_id=container-1"
///
/// # Combine filters
/// curl "http://localhost:3001/api/nodes/query?parent_id=abc-123&container_id=container-1"
/// ```
async fn query_nodes_simple(
    State(state): State<AppState>,
    Query(params): Query<QueryNodesParams>,
) -> Result<Json<Vec<Node>>, HttpError> {
    tracing::debug!("Query nodes: {:?}", params);

    // Validate parent_id format if provided
    if let Some(ref id) = params.parent_id {
        if id != "null" && !id.is_empty() && !is_valid_node_id(id) {
            return Err(HttpError::new(
                format!(
                    "Invalid parent_id format '{}'. Must be a valid UUID or date (YYYY-MM-DD)",
                    id
                ),
                "INVALID_INPUT",
            ));
        }
    }

    // Validate container_id format if provided
    if let Some(ref id) = params.container_id {
        if !is_valid_node_id(id) {
            return Err(HttpError::new(
                format!(
                    "Invalid container_id format '{}'. Must be a valid UUID or date (YYYY-MM-DD)",
                    id
                ),
                "INVALID_INPUT",
            ));
        }
    }

    // Convert "null" string to actual None for querying root nodes
    // The string "null" from the query parameter is converted to SQL NULL
    // so we can query for nodes where parent_id IS NULL
    // Empty string is rejected to prevent ambiguous queries
    let parent_id = match params.parent_id.as_deref() {
        Some("null") => None,
        Some("") => {
            return Err(HttpError::new(
                "parent_id cannot be empty string. Use 'null' for root nodes or omit parameter for no filter",
                "INVALID_INPUT",
            ))
        }
        Some(id) => Some(id.to_string()),
        None => None,
    };

    // Build filter
    let filter = NodeFilter {
        parent_id,
        container_node_id: params.container_id,
        ..Default::default()
    };

    // Execute query with timing
    let start = std::time::Instant::now();
    let node_service = crate::dev_server::node_endpoints::create_node_service(&state).await?;
    let nodes = node_service
        .query_nodes(filter.clone())
        .await
        .map_err(|e| HttpError::from_anyhow(e.into(), "NODE_QUERY_ERROR"))?;
    let elapsed = start.elapsed();

    tracing::debug!(
        "Found {} nodes in {:?} (parent_id={:?}, container_id={:?})",
        nodes.len(),
        elapsed,
        filter.parent_id,
        filter.container_node_id
    );

    // Warn if query is slow (>100ms)
    if elapsed.as_millis() > 100 {
        tracing::warn!(
            "Slow query detected: found {} nodes in {:?} (parent_id={:?}, container_id={:?})",
            nodes.len(),
            elapsed,
            filter.parent_id,
            filter.container_node_id
        );
    }

    Ok(Json(nodes))
}

/// Get nodes by origin ID
///
/// # Path Parameters
///
/// - `origin_id`: Origin ID to search for
///
/// # Example
///
/// ```bash
/// curl http://localhost:3001/api/nodes/by-origin/original-node-123
/// ```
async fn get_nodes_by_origin_id(
    State(_state): State<AppState>,
    Path(origin_id): Path<String>,
) -> Result<Json<Vec<Node>>, HttpError> {
    tracing::debug!("Get nodes by origin: {}", origin_id);

    // NodeFilter doesn't have origin_id field yet
    // Return explicit error to indicate unimplemented functionality
    Err(HttpError::new(
        "Origin ID queries not yet implemented - NodeFilter missing origin_id field",
        "NOT_IMPLEMENTED",
    ))
}

/// Create router with all Phase 2 query endpoints
///
/// This function is called by the main router in `mod.rs` to register
/// all Phase 2 query endpoints.
pub fn routes(state: AppState) -> Router {
    Router::new()
        .route("/api/nodes/query", get(query_nodes_simple))
        .route(
            "/api/nodes/by-origin/:origin_id",
            get(get_nodes_by_origin_id),
        )
        .with_state(state)
}
