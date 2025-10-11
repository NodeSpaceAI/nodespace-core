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
//! 1. DatabaseService - SQLite database access
//! 2. NodeService - Node CRUD operations
//!
//! Future phases will add embedding services and other functionality.

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use nodespace_core::{DatabaseService, NodeService};

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

    let db_service = DatabaseService::new(db_path.clone()).await?;
    let node_service = NodeService::new(db_service.clone())?;

    tracing::info!("✅ Services initialized");

    // Start HTTP server
    nodespace_app_lib::dev_server::start_server(Arc::new(db_service), Arc::new(node_service), port)
        .await?;

    Ok(())
}
