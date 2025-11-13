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
    http::{header, Method},
    Router,
};
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};

use nodespace_core::services::NodeEmbeddingService;
use nodespace_core::{NodeService, SurrealStore};

// Phase 1: Node CRUD endpoints (this issue #209)
mod node_endpoints;

// Phase 2: Query and advanced operations (added by #211)
mod query_endpoints;

// Phase 3: Embeddings and mentions (added by #212)
mod embedding_endpoints;

// Schema management endpoints (added by #386)
mod schema_endpoints;

// Shared HTTP error handling
mod http_error;

// Re-export HttpError for use by endpoint modules
pub use http_error::HttpError;

/// Type alias for services that can be dynamically swapped during tests
///
/// The Arc<RwLock<Arc<T>>> pattern enables:
/// - Thread-safe shared ownership (outer Arc)
/// - Safe service replacement via write lock (RwLock)
/// - Multiple readers or single writer semantics (inner Arc)
type SharedService<T> = Arc<RwLock<Arc<T>>>;

/// Application state shared across all endpoints
///
/// Uses RwLock to allow dynamic database switching for test isolation (Issue #255).
/// Each test can call /api/database/init with a unique database path, and the init
/// endpoint will:
/// 1. Drain connections from the old database
/// 2. Create a new SurrealStore
/// 3. Atomically swap the services
///
/// This ensures proper synchronization without stale connections.
///
/// # SurrealDB Write Serialization (Issue #266)
///
/// The `write_lock` mutex serializes all database write operations (create, update, delete)
/// to reduce SurrealDB write contention under rapid concurrent operations. This significantly
/// improves test reliability.
///
/// Read operations do NOT acquire the write lock and can execute concurrently.
#[derive(Clone)]
pub struct AppState {
    pub store: SharedService<SurrealStore>,
    pub node_service: SharedService<NodeService>,
    pub embedding_service: SharedService<NodeEmbeddingService>,
    pub write_lock: Arc<Mutex<()>>,
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
        .merge(query_endpoints::routes(state.clone()))
        // Phase 3: Embeddings and mentions (added by #212)
        .merge(embedding_endpoints::routes(state.clone()))
        // Schema management endpoints (added by #386)
        .merge(schema_endpoints::routes(state.clone()))
        .layer(cors_layer())
}

/// Create CORS layer for development
///
/// Allows requests from Vite dev server. Supports configurable origins via
/// CORS_ALLOW_ORIGIN environment variable for flexibility when Vite uses
/// different ports.
///
/// Default: http://localhost:1420
/// Configure: CORS_ALLOW_ORIGIN="http://localhost:5173" cargo run ...
fn cors_layer() -> CorsLayer {
    // Allow multiple common Vite ports
    let default_origins = [
        "http://localhost:1420", // NodeSpace default
        "http://localhost:5173", // Vite default
        "http://localhost:1421", // Fallback if 1420 busy
    ];

    // Check for custom CORS origin from environment
    let origins: Vec<header::HeaderValue> =
        if let Ok(custom_origin) = std::env::var("CORS_ALLOW_ORIGIN") {
            vec![custom_origin
                .parse::<header::HeaderValue>()
                .expect("Invalid CORS_ALLOW_ORIGIN - must be valid HTTP origin")]
        } else {
            default_origins
                .iter()
                .map(|o| o.parse::<header::HeaderValue>().unwrap())
                .collect()
        };

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers(Any)
        .allow_credentials(false)
}

/// Start the HTTP dev server
///
/// # Arguments
///
/// * `store` - SurrealDB store instance
/// * `node_service` - Node service instance
/// * `embedding_service` - Embedding service instance
/// * `port` - Port to listen on (typically 3001)
///
/// # Errors
///
/// Returns error if server fails to bind or start.
pub async fn start_server(
    store: SharedService<SurrealStore>,
    node_service: SharedService<NodeService>,
    embedding_service: SharedService<NodeEmbeddingService>,
    port: u16,
) -> anyhow::Result<()> {
    let state = AppState {
        store,
        node_service,
        embedding_service,
        write_lock: Arc::new(Mutex::new(())),
    };
    let app = create_router(state);

    let addr = format!("127.0.0.1:{}", port);
    tracing::info!("🚀 HTTP dev server starting on http://{}", addr);
    tracing::info!("📡 CORS enabled for http://localhost:1420");
    tracing::info!("⚠️  Development mode only - NOT for production use");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
