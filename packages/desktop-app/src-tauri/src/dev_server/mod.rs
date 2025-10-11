//! Development-only HTTP server for web-mode testing
//!
//! This module provides a REST API that mirrors Tauri IPC commands,
//! enabling web mode testing with real database access. The server
//! only compiles when the `dev-server` feature is enabled and should
//! NEVER be included in production builds.
//!
//! # Architecture
//!
//! The dev server is organized into modular endpoint modules:
//! - `node_endpoints`: Phase 1 MVP (basic CRUD operations)
//! - Future phases will add additional modules as needed
//!
//! # Usage
//!
//! Start the server with:
//! ```bash
//! cargo run --bin dev-server --features dev-server
//! ```
//!
//! Or via npm script:
//! ```bash
//! bun run dev:server
//! ```
//!
//! # Security
//!
//! - CORS restricted to localhost:1420 (Vite dev server)
//! - No authentication (local development only)
//! - Never runs in production (feature-gated)

use axum::{
    http::{header, Method, StatusCode},
    response::{IntoResponse, Json, Response},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use nodespace_core::{DatabaseService, NodeService};

// Phase 1: Node CRUD endpoints (this issue #209)
mod node_endpoints;

// Phase 2: Query and advanced operations (added by #211)
// mod query_endpoints;

// Phase 3: Embeddings and mentions (added by #212)
// mod embedding_endpoints;

/// HTTP error response matching Tauri's CommandError structure
///
/// This ensures consistent error handling between Tauri IPC and HTTP modes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpError {
    /// User-facing error message
    pub message: String,
    /// Machine-readable error code
    pub code: String,
    /// Optional detailed error information for debugging
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl HttpError {
    /// Create a new HTTP error
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
            details: None,
        }
    }

    /// Create a new HTTP error with details
    pub fn with_details(
        message: impl Into<String>,
        code: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
            details: Some(details.into()),
        }
    }

    /// Convert from anyhow::Error
    pub fn from_anyhow(err: anyhow::Error, code: impl Into<String>) -> Self {
        Self {
            message: err.to_string(),
            code: code.into(),
            details: Some(format!("{:?}", err)),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let status = match self.code.as_str() {
            "NODE_NOT_FOUND" | "RESOURCE_NOT_FOUND" => StatusCode::NOT_FOUND,
            "INVALID_INPUT" | "INVALID_NODE_TYPE" | "VALIDATION_ERROR" => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (status, Json(self)).into_response()
    }
}

/// Application state shared across all endpoints
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DatabaseService>,
    pub node_service: Arc<NodeService>,
}

/// Create the main application router with all endpoint modules
///
/// This function uses axum's modular routing pattern, allowing each
/// phase to add their endpoints independently via `.merge()`.
///
/// # Architecture for Multi-Phase Development
///
/// Each phase adds a new module and merges its routes here:
///
/// ```rust,ignore
/// pub fn create_router(state: AppState) -> Router {
///     Router::new()
///         .merge(node_endpoints::routes(state.clone()))      // Phase 1
///         .merge(query_endpoints::routes(state.clone()))     // Phase 2 (added by #211)
///         .merge(embedding_endpoints::routes(state.clone())) // Phase 3 (added by #212)
///         .layer(cors_layer())
/// }
/// ```
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // Phase 1: Basic node CRUD operations (this issue #209)
        .merge(node_endpoints::routes(state.clone()))
        // Phase 2: Query and advanced operations (added by #211)
        // .merge(query_endpoints::routes(state.clone()))
        // Phase 3: Embeddings and mentions (added by #212)
        // .merge(embedding_endpoints::routes(state.clone()))
        .layer(cors_layer())
}

/// Create CORS layer for development
///
/// Allows requests from Vite dev server (localhost:1420) only.
fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin("http://localhost:1420".parse::<header::HeaderValue>().unwrap())
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers(Any)
        .allow_credentials(false)
}

/// Start the HTTP dev server
///
/// # Arguments
///
/// * `db` - Database service instance
/// * `node_service` - Node service instance
/// * `port` - Port to listen on (typically 3001)
///
/// # Errors
///
/// Returns error if server fails to bind or start.
pub async fn start_server(
    db: Arc<DatabaseService>,
    node_service: Arc<NodeService>,
    port: u16,
) -> anyhow::Result<()> {
    let state = AppState { db, node_service };
    let app = create_router(state);

    let addr = format!("127.0.0.1:{}", port);
    tracing::info!("🚀 HTTP dev server starting on http://{}", addr);
    tracing::info!("📡 CORS enabled for http://localhost:1420");
    tracing::info!("⚠️  Development mode only - NOT for production use");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
