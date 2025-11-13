//! Development HTTP Server Binary
//!
//! Standalone binary that starts an HTTP server exposing Rust backend
//! functionality as REST APIs. This enables web-mode testing with real
//! database access.
//!
//! # Usage
//!
//! ```bash
//! # Start with default settings (port 3001, default DB path)
//! cargo run --bin dev-server --features dev-server
//!
//! # Or via npm script
//! bun run dev:server
//!
//! # Custom port
//! DEV_SERVER_PORT=3002 cargo run --bin dev-server --features dev-server
//! ```
//!
//! # Environment Variables
//!
//! - `DEV_SERVER_PORT`: Server port (default: 3001)
//! - `RUST_LOG`: Logging level (e.g., "info", "debug", "trace")
//!
//! # Security
//!
//! **DEVELOPMENT ONLY** - This binary should never be used in production:
//! - No authentication
//! - CORS restricted to localhost
//! - Feature-gated to prevent production builds
//!
//! # Architecture
//!
//! The server initializes the same services used by the Tauri app:
//! 1. SurrealStore - SurrealDB database access
//! 2. NodeService - Node CRUD operations
//!
//! Future phases will add embedding services and other functionality.

use std::env;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use nodespace_core::services::NodeEmbeddingService;
use nodespace_core::{NodeService, SurrealStore};
use nodespace_nlp_engine::EmbeddingService;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("🚀 NodeSpace HTTP Dev Server");
    tracing::info!("==================================");

    // Get server port from environment or use default
    let port = env::var("DEV_SERVER_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3001);

    tracing::info!("📡 Port: {}", port);

    // Determine database path (use dev-specific database)
    let home_dir =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?;

    let db_path: PathBuf = home_dir
        .join(".nodespace")
        .join("database")
        .join("nodespace-dev.db");

    // Ensure database directory exists
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tracing::info!("📦 Database: {}", db_path.display());

    // Initialize services (same as Tauri app)
    tracing::info!("🔧 Initializing services...");

    // Initialize SurrealDB store
    let store = Arc::new(SurrealStore::new(db_path.clone()).await?);

    // Initialize node service with SurrealStore
    let node_service = NodeService::new(store.clone())?;

    // Initialize NLP engine for embeddings (temporarily stubbed - Issue #481)
    let nlp_engine = Arc::new(
        EmbeddingService::new(Default::default())
            .map_err(|e| anyhow::anyhow!("Failed to initialize NLP engine: {}", e))?,
    );

    // Initialize embedding service (temporarily stubbed - Issue #481)
    let embedding_service = NodeEmbeddingService::new(nlp_engine);

    tracing::info!("✅ Services initialized");

    // Wrap services in RwLock for dynamic database switching during tests (Issue #255)
    let store_arc = Arc::new(RwLock::new(store));
    let ns_arc = Arc::new(RwLock::new(Arc::new(node_service)));
    let es_arc = Arc::new(RwLock::new(Arc::new(embedding_service)));

    // Start HTTP server
    nodespace_app_lib::dev_server::start_server(store_arc, ns_arc, es_arc, port).await?;

    Ok(())
}
