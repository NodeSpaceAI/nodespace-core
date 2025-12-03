//! Tauri MCP Integration
//!
//! Wraps core MCP protocol with Tauri-specific functionality (event emissions).
//! This is the integration layer that connects the pure protocol (in nodespace-core)
//! with the Tauri desktop application.
//!
//! As of Issue #676, NodeOperations layer was merged into NodeService.
//! All MCP handlers now use NodeService directly.
//!
//! As of Issue #715, MCP server is a managed service (McpServerService).
//! This module provides the Tauri event callback for UI reactivity.

use nodespace_core::services::{
    default_mcp_port, McpResponseCallback, McpServerService, NodeEmbeddingService,
};
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

// NOTE: SchemaUpdatedEvent removed (Issue #690) - schema mutation commands not used by UI

/// Create McpServerService with Tauri event callback
///
/// Creates the managed MCP service configured with a callback that emits
/// Tauri events after successful operations. The callback ensures the UI
/// updates reactively when AI agents modify data via MCP.
///
/// Port configuration: Can be set via `MCP_PORT` environment variable,
/// defaults to 3100 if not specified (avoids conflict with dev-server on 3001).
///
/// # Arguments
/// * `node_service` - Shared NodeService for node operations
/// * `embedding_service` - Shared embedding service for semantic search
/// * `app` - Tauri AppHandle for event emission
///
/// # Returns
/// * `McpServerService` configured with Tauri event callback
/// * `McpResponseCallback` for starting the server with events
pub fn create_mcp_service_with_events(
    node_service: Arc<NodeService>,
    embedding_service: Arc<NodeEmbeddingService>,
    app: AppHandle,
) -> (McpServerService, McpResponseCallback) {
    let port = default_mcp_port();

    // Create callback that emits Tauri events
    let callback: McpResponseCallback = Arc::new(move |method: &str, result: &Value| {
        emit_event_for_method(&app, method, result);
    });

    let service = McpServerService::new(node_service, embedding_service, port);

    (service, callback)
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
        "update_nodes_batch" => {
            // Emit node-updated event for each successfully updated node
            if let Some(updated_ids) = result["updated"].as_array() {
                for node_id_value in updated_ids {
                    if let Some(node_id) = node_id_value.as_str() {
                        let event = NodeUpdatedEvent {
                            node_id: node_id.to_string(),
                        };
                        if let Err(e) = app.emit("node-updated", &event) {
                            warn!(
                                "Failed to emit node-updated event for batch update {}: {}",
                                node_id, e
                            );
                        }
                    }
                }
            }
        }
        "update_container_from_markdown" => {
            // Emit node-created events for all newly created nodes
            // (Note: We don't emit node-deleted events for removed children to avoid excessive events,
            // as the frontend should refresh the container view anyway)
            if let Some(nodes) = result["nodes"].as_array() {
                for node_metadata in nodes {
                    // The result contains NodeMetadata (id + node_type), not full Node objects
                    // So we emit just the node_id and let the frontend fetch if needed
                    if let (Some(_node_id), Some(_node_type)) = (
                        node_metadata["id"].as_str(),
                        node_metadata["node_type"].as_str(),
                    ) {
                        // For simplicity, emit a generic updated event for the container
                        // The frontend should refresh the entire container view
                        if let Some(container_id) = result["container_id"].as_str() {
                            let event = NodeUpdatedEvent {
                                node_id: container_id.to_string(),
                            };
                            if let Err(e) = app.emit("node-updated", &event) {
                                warn!(
                                    "Failed to emit node-updated event for container {}: {}",
                                    container_id, e
                                );
                            }
                            break; // Only emit once for the container, not for each child
                        }
                    }
                }
            }
        }
        // NOTE: Schema mutation commands (add_schema_field, remove_schema_field, etc.)
        // were removed in Issue #690 as they weren't used by UI.
        _ => {} // No events for get/query operations
    }
}
