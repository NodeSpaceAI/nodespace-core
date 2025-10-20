//! Tauri MCP Integration
//!
//! Wraps core MCP protocol with Tauri-specific functionality (event emissions).
//! This is the integration layer that connects the pure protocol (in nodespace-core)
//! with the Tauri desktop application.

use nodespace_core::mcp;
use nodespace_core::NodeService;
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tracing::warn;

/// Event payload for node-created event
#[derive(Debug, Serialize)]
struct NodeCreatedEvent {
    node_id: String,
    node_type: String,
}

/// Event payload for node-updated event
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
/// method type.
pub async fn run_mcp_server_with_events(
    node_service: Arc<NodeService>,
    app: AppHandle,
) -> anyhow::Result<()> {
    // Create callback that emits Tauri events
    let callback = Arc::new(move |method: &str, result: &Value| {
        emit_event_for_method(&app, method, result);
    });

    // Run core MCP server with event-emitting callback
    mcp::run_mcp_server_with_callback(node_service, Some(callback)).await
}

/// Emit Tauri event based on MCP method and result
///
/// Examines the method name and result payload to emit appropriate Tauri
/// events for UI reactivity. Only mutation operations (create, update, delete)
/// emit events; read operations (get, query) do not.
fn emit_event_for_method(app: &AppHandle, method: &str, result: &Value) {
    match method {
        "create_node" => {
            if let (Some(node_id), Some(node_type)) =
                (result["node_id"].as_str(), result["node_type"].as_str())
            {
                let event = NodeCreatedEvent {
                    node_id: node_id.to_string(),
                    node_type: node_type.to_string(),
                };
                if let Err(e) = app.emit("node-created", &event) {
                    warn!("Failed to emit node-created event: {}", e);
                }
            }
        }
        "update_node" => {
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
