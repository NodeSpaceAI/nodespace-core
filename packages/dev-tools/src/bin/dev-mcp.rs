//! Development MCP Server for Browser Mode
//!
//! This is a standalone MCP (Model Context Protocol) server that enables AI agent
//! integration during browser mode development. It runs alongside dev-proxy to
//! provide the same MCP functionality available in the Tauri desktop app.
//!
//! Architecture:
//!   AI Agent (Claude Code) → HTTP (port 3100) → MCP Server → NodeService → SurrealDB (port 8000)
//!                                                                                    ↓
//!   Frontend             → HTTP (port 3001) → dev-proxy → NodeService → SurrealDB (port 8000)
//!
//! # Key Features
//!
//! - Uses McpServerService from nodespace-core for managed lifecycle
//! - Connects to SurrealDB HTTP server (same as dev-proxy)
//! - Creates its own NodeService and EmbeddingService instances
//! - MCP is stateless - queries go directly to NodeService (no SSE needed)
//!
//! # Usage
//!
//! ```bash
//! # Start SurrealDB first
//! bun run dev:db
//!
//! # Then start dev-mcp
//! cargo run --bin dev-mcp
//!
//! # Or use the npm script
//! bun run dev:mcp
//! ```
//!
//! # Port Configuration
//!
//! Uses `MCP_PORT` environment variable, defaults to 3100 if not specified.
//! This avoids conflicts with dev-proxy (3001) and SurrealDB (8000).

use nodespace_core::{
    db::HttpStore,
    services::{default_mcp_port, McpServerService, NodeEmbeddingService, NodeService},
};
use nodespace_nlp_engine::EmbeddingService;
use std::sync::Arc;

// Type alias for HTTP client types
type HttpNodeService = NodeService<surrealdb::engine::remote::http::Client>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("dev_mcp=debug,nodespace_core=debug")
        .init();

    println!("🔧 Initializing dev-mcp (MCP server for browser mode)...");

    // Connect to SurrealDB HTTP server (must be running on port 8000)
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
    // NodeService::new() takes &mut Arc to enable cache updates during seeding (Issue #704)
    println!("🧠 Initializing NodeService...");
    let node_service: Arc<HttpNodeService> = match NodeService::new(&mut store).await {
        Ok(s) => {
            println!("✅ NodeService initialized");
            // Set client ID for MCP server so domain events have source_client_id (Issue #715)
            // This allows browser frontend to filter out MCP-originated events
            Arc::new(s.with_client("mcp-server"))
        }
        Err(e) => {
            eprintln!("❌ Failed to initialize NodeService: {}", e);
            return Err(e.into());
        }
    };

    // Initialize NLP engine for embeddings (used by semantic search)
    println!("🧠 Initializing NLP engine for embeddings...");
    let mut nlp_engine = EmbeddingService::new(Default::default())
        .map_err(|e| anyhow::anyhow!("Failed to create NLP engine: {}", e))?;

    nlp_engine
        .initialize()
        .map_err(|e| anyhow::anyhow!("Failed to initialize NLP engine: {}", e))?;

    let nlp_engine_arc = Arc::new(nlp_engine);
    println!("✅ NLP engine initialized");

    // Initialize embedding service
    let embedding_service = Arc::new(NodeEmbeddingService::new(nlp_engine_arc, store.clone()));

    // Create MCP server service
    let port = default_mcp_port();
    let mcp_service = McpServerService::new(node_service, Some(embedding_service), port);

    println!("\n🚀 Starting MCP server...");
    println!("   Port: {}", port);
    println!("   Transport: HTTP");
    println!("   NodeService → SurrealDB (port 8000)");
    println!(
        "\n   AI agents can now connect to: http://127.0.0.1:{}\n",
        port
    );

    // Start MCP server (blocks until shutdown)
    mcp_service.start().await
}
