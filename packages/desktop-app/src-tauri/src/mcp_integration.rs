//! Tauri MCP Integration
//!
//! Wraps core MCP protocol with Tauri-specific functionality (event emissions).
//! This is the integration layer that connects the pure protocol (in nodespace-core)
//! with the Tauri desktop application.

use nodespace_core::mcp::{self, MCPError};
use nodespace_core::NodeService;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

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
/// Wraps the core MCP server and adds event emission after each successful operation.
/// This ensures the UI updates reactively when AI agents modify data.
pub async fn run_mcp_server_with_events(
    node_service: Arc<NodeService>,
    app: AppHandle,
) -> anyhow::Result<()> {
    // For now, we'll just call the core server
    // TODO: Wrap handlers to emit events after successful operations

    // The clean way would be to:
    // 1. Listen to MCP responses
    // 2. Extract operation type and IDs from responses
    // 3. Emit corresponding Tauri events

    // For this refactor, we'll use the simpler approach:
    // Create a wrapper that intercepts responses and emits events

    run_mcp_with_event_wrapper(node_service, app).await
}

/// Internal implementation that wraps MCP with event emissions
async fn run_mcp_with_event_wrapper(
    node_service: Arc<NodeService>,
    app: AppHandle,
) -> anyhow::Result<()> {
    use tracing::{debug, info};

    info!("🔌 MCP stdio server started (with Tauri events)");

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let reader = BufReader::new(stdin);
    let mut writer = BufWriter::new(stdout);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        debug!("📥 MCP request: {}", line);

        // Parse request to extract method
        let request: Value = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                let error_response = json!({
                    "jsonrpc": "2.0",
                    "id": 0,
                    "error": {
                        "code": -32700,
                        "message": format!("Invalid JSON: {}", e)
                    }
                });
                write_json(&mut writer, &error_response).await?;
                continue;
            }
        };

        let method = request["method"].as_str().unwrap_or("");
        let request_id = request["id"].as_u64().unwrap_or(0);
        let params = request["params"].clone();

        // Call core MCP handlers
        let result = match method {
            "create_node" => mcp::handlers::nodes::handle_create_node(&node_service, params).await,
            "get_node" => mcp::handlers::nodes::handle_get_node(&node_service, params).await,
            "update_node" => mcp::handlers::nodes::handle_update_node(&node_service, params).await,
            "delete_node" => mcp::handlers::nodes::handle_delete_node(&node_service, params).await,
            "query_nodes" => mcp::handlers::nodes::handle_query_nodes(&node_service, params).await,
            _ => Err(MCPError::method_not_found(method)),
        };

        // Build response and emit events
        let response = match result {
            Ok(result_value) => {
                // Emit Tauri event based on method
                emit_event_for_method(&app, method, &result_value);

                json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": result_value
                })
            }
            Err(error) => {
                json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {
                        "code": error.code,
                        "message": error.message
                    }
                })
            }
        };

        write_json(&mut writer, &response).await?;
    }

    info!("🔌 MCP stdio server stopped (stdin closed)");
    Ok(())
}

/// Emit Tauri event based on MCP method and result
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
                app.emit("node-created", &event).ok();
            }
        }
        "update_node" => {
            if let Some(node_id) = result["node_id"].as_str() {
                let event = NodeUpdatedEvent {
                    node_id: node_id.to_string(),
                };
                app.emit("node-updated", &event).ok();
            }
        }
        "delete_node" => {
            if let Some(node_id) = result["node_id"].as_str() {
                let event = NodeDeletedEvent {
                    node_id: node_id.to_string(),
                };
                app.emit("node-deleted", &event).ok();
            }
        }
        _ => {} // No events for get/query operations
    }
}

/// Write JSON response to stdout
async fn write_json(
    writer: &mut BufWriter<tokio::io::Stdout>,
    value: &Value,
) -> anyhow::Result<()> {
    let json = serde_json::to_string(value)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}
