//! Tauri MCP Integration
//!
//! Wraps core MCP protocol with Tauri-specific functionality (event emissions).
//! This is the integration layer that connects the pure protocol (in nodespace-core)
//! with the Tauri desktop application.

use nodespace_core::mcp;
use nodespace_core::services::NodeEmbeddingService;
use nodespace_core::{Node, NodeService};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tracing::warn;

/// Event payload for node-created event
#[derive(Debug, Serialize)]
struct NodeCreatedEvent {
    node: Node,
}

/// Event payload for node-updated event (uses hybrid approach - frontend fetches full node)
#[derive(Debug, Serialize)]
struct NodeUpdatedEvent {
    node_id: String,
}

/// Event payload for node-deleted event
#[derive(Debug, Serialize)]
struct NodeDeletedEvent {
    node_id: String,
}

/// Run MCP server with Tauri event emissions
///
/// Uses the core MCP server with a callback that emits Tauri events after
/// successful operations. This ensures the UI updates reactively when AI
/// agents modify data via MCP.
///
/// Implementation: Provides a callback to the core server that inspects
/// successful responses and emits appropriate Tauri events based on the
/// method type. Uses HTTP transport for GUI app integration.
///
/// Port configuration: Can be set via `MCP_PORT` environment variable,
/// defaults to 3001 if not specified.
pub async fn run_mcp_server_with_events(
    node_service: Arc<NodeService>,
    embedding_service: Arc<NodeEmbeddingService>,
    app: AppHandle,
) -> anyhow::Result<()> {
    // Get port from environment variable or use default
    let port = std::env::var("MCP_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3001);

    // Create callback that emits Tauri events
    let callback = Arc::new(move |method: &str, result: &Value| {
        emit_event_for_method(&app, method, result);
    });

    // Create combined services struct for MCP
    let services = mcp::server::McpServices {
        node_service,
        embedding_service,
    };

    // Run core MCP server with HTTP transport and event-emitting callback
    mcp::run_mcp_server_with_callback(
        services,
        mcp::server::McpTransport::Http { port },
        Some(callback),
    )
    .await
}

/// Emit Tauri event based on MCP method and result
///
/// Examines the method name and result payload to emit appropriate Tauri
/// events for UI reactivity. Only mutation operations (create, update, delete)
/// emit events; read operations (get, query) do not.
///
/// For create operations, the full Node is deserialized and emitted to avoid
/// extra database fetches on the frontend. For update operations, only the
/// node_id is emitted (hybrid approach - frontend fetches full node).
fn emit_event_for_method(app: &AppHandle, method: &str, result: &Value) {
    match method {
        "create_node" => {
            // Deserialize full Node from result and emit
            match serde_json::from_value::<Node>(result.clone()) {
                Ok(node) => {
                    let event = NodeCreatedEvent { node };
                    if let Err(e) = app.emit("node-created", &event) {
                        warn!("Failed to emit node-created event: {}", e);
                    }
                }
                Err(e) => {
                    warn!("Failed to deserialize node from create_node result: {}", e);
                }
            }
        }
        "create_nodes_from_markdown" => {
            // Emit node-created event for each node in the nodes array
            if let Some(nodes) = result["nodes"].as_array() {
                for node_value in nodes {
                    match serde_json::from_value::<Node>(node_value.clone()) {
                        Ok(node) => {
                            let event = NodeCreatedEvent { node };
                            if let Err(e) = app.emit("node-created", &event) {
                                warn!(
                                    "Failed to emit node-created event for markdown import node {}: {}",
                                    event.node.id, e
                                );
                            }
                        }
                        Err(e) => {
                            warn!("Failed to deserialize node from markdown import: {}", e);
                        }
                    }
                }
            }
        }
        "update_node" => {
            // Hybrid approach: emit only node_id, frontend fetches full node
            if let Some(node_id) = result["node_id"].as_str() {
                let event = NodeUpdatedEvent {
                    node_id: node_id.to_string(),
                };
                if let Err(e) = app.emit("node-updated", &event) {
                    warn!("Failed to emit node-updated event: {}", e);
                }
            }
        }
        "delete_node" => {
            if let Some(node_id) = result["node_id"].as_str() {
                let event = NodeDeletedEvent {
                    node_id: node_id.to_string(),
                };
                if let Err(e) = app.emit("node-deleted", &event) {
                    warn!("Failed to emit node-deleted event: {}", e);
                }
            }
        }
        _ => {} // No events for get/query operations
    }
}
