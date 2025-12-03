//! MCP Server Service
//!
//! Managed service wrapper for the MCP (Model Context Protocol) server.
//! Provides a structured interface for starting and managing the MCP server
//! as part of the application's service layer.
//!
//! This service can be used in:
//! - Tauri desktop app (with event callbacks for UI reactivity)
//! - Browser dev mode (standalone, no callbacks)
//! - Future headless mode
//!
//! # Architecture
//!
//! The service wraps the core MCP server (`mcp::server`) and handles:
//! - Port configuration (via `MCP_PORT` env var or default 3100)
//! - Service lifecycle (start, with future stop/restart support)
//! - Transport selection (HTTP by default, stdio available)
//!
//! # Example (Browser Mode - No Callbacks)
//!
//! ```ignore
//! let mcp_service = McpServerService::new(
//!     node_service,
//!     embedding_service,
//!     3100,
//! );
//! mcp_service.start().await?;
//! ```
//!
//! # Example (Tauri Mode - With Callbacks)
//!
//! ```ignore
//! let mcp_service = McpServerService::new(
//!     node_service,
//!     embedding_service,
//!     3100,
//! );
//! mcp_service.start_with_callback(callback).await?;
//! ```

use crate::mcp;
use crate::services::{NodeEmbeddingService, NodeService};
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

/// Callback type for handling successful MCP responses
///
/// Invoked after each successful MCP operation with (method_name, result_value).
/// Used by Tauri integration to emit events for UI reactivity.
pub type McpResponseCallback = Arc<dyn Fn(&str, &Value) + Send + Sync>;

/// MCP Server Service
///
/// Managed service that wraps the core MCP server, providing a clean interface
/// for integration with different runtimes (Tauri, dev-mcp binary, etc.).
///
/// Generic over the database connection type `C` to support both local
/// embedded database (Db) and HTTP client connections.
///
/// # Thread Safety
///
/// This service is `Clone` and uses `Arc` internally, making it safe to share
/// across async tasks and register with Tauri's state management.
#[derive(Clone)]
pub struct McpServerService<C = surrealdb::engine::local::Db>
where
    C: surrealdb::Connection,
{
    node_service: Arc<NodeService<C>>,
    embedding_service: Arc<NodeEmbeddingService<C>>,
    port: u16,
}

impl<C> McpServerService<C>
where
    C: surrealdb::Connection,
{
    /// Create a new MCP server service
    ///
    /// # Arguments
    ///
    /// * `node_service` - Shared NodeService instance for node operations
    /// * `embedding_service` - Shared embedding service for semantic search
    /// * `port` - HTTP port to listen on (typically 3100)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let port = std::env::var("MCP_PORT")
    ///     .ok()
    ///     .and_then(|p| p.parse().ok())
    ///     .unwrap_or(3100);
    ///
    /// let service = McpServerService::new(node_service, embedding_service, port);
    /// ```
    pub fn new(
        node_service: Arc<NodeService<C>>,
        embedding_service: Arc<NodeEmbeddingService<C>>,
        port: u16,
    ) -> Self {
        Self {
            node_service,
            embedding_service,
            port,
        }
    }

    /// Get the configured port
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Start the MCP server without callbacks (browser mode / headless)
    ///
    /// Starts the HTTP server and blocks until shutdown. This is suitable for
    /// standalone usage where no UI reactivity is needed.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on graceful shutdown, or `Err` on fatal error.
    pub async fn start(&self) -> anyhow::Result<()> {
        info!("Starting MCP server on port {}...", self.port);

        let services = mcp::server::McpServices {
            node_service: self.node_service.clone(),
            embedding_service: self.embedding_service.clone(),
        };

        mcp::run_mcp_server_with_callback(
            services,
            mcp::server::McpTransport::Http { port: self.port },
            None,
        )
        .await
    }

    /// Start the MCP server with a response callback (Tauri mode)
    ///
    /// Starts the HTTP server with a callback that's invoked after each
    /// successful operation. The callback receives (method_name, result_value)
    /// and is typically used to emit Tauri events for UI reactivity.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function called after each successful MCP operation
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on graceful shutdown, or `Err` on fatal error.
    pub async fn start_with_callback(&self, callback: McpResponseCallback) -> anyhow::Result<()> {
        info!(
            "Starting MCP server on port {} with event callback...",
            self.port
        );

        let services = mcp::server::McpServices {
            node_service: self.node_service.clone(),
            embedding_service: self.embedding_service.clone(),
        };

        mcp::run_mcp_server_with_callback(
            services,
            mcp::server::McpTransport::Http { port: self.port },
            Some(callback),
        )
        .await
    }
}

/// Get default MCP port from environment variable or fallback
///
/// Reads `MCP_PORT` environment variable, falling back to 3100 if not set.
/// This is a standalone function (not a method) to avoid needing type annotations
/// when called without a service instance.
pub fn default_mcp_port() -> u16 {
    std::env::var("MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3100)
}
