//! MCP stdio Server
//!
//! Async Tokio task that handles JSON-RPC 2.0 requests over stdin/stdout.
//! Pure protocol implementation with no framework dependencies.

use crate::mcp::handlers::nodes;
use crate::mcp::types::{MCPError, MCPNotification, MCPRequest, MCPResponse};
use crate::services::NodeService;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tracing::{debug, error, info, instrument, warn};

/// Server state tracking initialization status
struct ServerState {
    /// Whether the client has completed the initialization handshake
    initialized: Arc<AtomicBool>,
}

/// Callback type for handling successful responses
///
/// Receives (method_name, result_value) after successful operation execution.
/// Useful for event emissions or logging in framework integrations.
pub type ResponseCallback = Arc<dyn Fn(&str, &Value) + Send + Sync>;

/// Run the MCP stdio server
///
/// Reads JSON-RPC requests from stdin, processes them via handlers,
/// and writes responses to stdout. Runs indefinitely until EOF on stdin.
///
/// # Arguments
///
/// * `node_service` - Shared NodeService instance
///
/// # Returns
///
/// Returns Ok(()) when stdin is closed, or Err on fatal errors
#[instrument(skip(node_service))]
pub async fn run_mcp_server(node_service: Arc<NodeService>) -> anyhow::Result<()> {
    run_mcp_server_with_callback(node_service, None).await
}

/// Run the MCP stdio server with an optional response callback
///
/// Same as `run_mcp_server` but allows providing a callback function that
/// will be invoked after each successful operation. This is useful for
/// framework integrations that need to emit events or perform side effects.
///
/// # Arguments
///
/// * `node_service` - Shared NodeService instance
/// * `callback` - Optional callback invoked with (method, result) on success
///
/// # Returns
///
/// Returns Ok(()) when stdin is closed, or Err on fatal errors
#[instrument(skip(node_service, callback))]
pub async fn run_mcp_server_with_callback(
    node_service: Arc<NodeService>,
    callback: Option<ResponseCallback>,
) -> anyhow::Result<()> {
    info!("🔌 MCP stdio server started");

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let reader = BufReader::new(stdin);
    let mut writer = BufWriter::new(stdout);
    let mut lines = reader.lines();

    // Initialize server state
    let state = ServerState {
        initialized: Arc::new(AtomicBool::new(false)),
    };

    while let Some(line) = lines.next_line().await? {
        debug!("📥 MCP message: {}", line);

        // Try parsing as request first (has id field)
        match serde_json::from_str::<MCPRequest>(&line) {
            Ok(request) => {
                let request_id = request.id;
                let method = request.method.clone();

                // Handle request with state tracking
                let response = handle_request(&node_service, &state, request).await;

                // Invoke callback on successful response
                if let Some(ref callback) = callback {
                    if let Some(ref result) = response.result {
                        callback(&method, result);
                    }
                }

                debug!(
                    "📤 MCP response for method '{}' (id={})",
                    method, request_id
                );

                // Write response
                write_response(&mut writer, &response).await?;
                continue;
            }
            Err(request_err) => {
                // Try parsing as notification (no id field)
                match serde_json::from_str::<MCPNotification>(&line) {
                    Ok(notification) => {
                        handle_notification(&state, notification).await;
                        continue; // No response for notifications
                    }
                    Err(notification_err) => {
                        // Neither request nor notification - provide detailed diagnostics
                        warn!(
                            "❌ Failed to parse JSON-RPC message. Request parse error: {}. Notification parse error: {}. Message: {}",
                            request_err, notification_err, line
                        );

                        // Try to determine the issue type for better error message
                        let error_message = if line.trim().is_empty() {
                            "Empty message received".to_string()
                        } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                            // Valid JSON but not a valid JSON-RPC message
                            if value.get("jsonrpc").is_none() {
                                "Missing 'jsonrpc' field (must be \"2.0\")".to_string()
                            } else if value.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
                                format!(
                                    "Invalid 'jsonrpc' version: {:?} (must be \"2.0\")",
                                    value.get("jsonrpc")
                                )
                            } else if value.get("method").is_none() {
                                "Missing 'method' field".to_string()
                            } else if value.get("id").is_some() {
                                // Has id, so should be request
                                format!("Invalid request structure: {}", request_err)
                            } else {
                                // No id, so should be notification
                                format!("Invalid notification structure: {}", notification_err)
                            }
                        } else {
                            format!("Invalid JSON: {}", request_err)
                        };

                        let error_response =
                            MCPResponse::error(0, MCPError::parse_error(error_message));
                        write_response(&mut writer, &error_response).await?;
                    }
                }
            }
        }
    }

    info!("🔌 MCP stdio server stopped (stdin closed)");
    Ok(())
}

/// Handle a JSON-RPC request and return a response
#[instrument(skip(service, state), fields(method = %request.method, id = %request.id))]
async fn handle_request(
    service: &Arc<NodeService>,
    state: &ServerState,
    request: MCPRequest,
) -> MCPResponse {
    // Check initialization state before processing operations
    // Allow only 'initialize' and 'ping' methods before initialization is complete
    if request.method != "initialize"
        && request.method != "ping"
        && !state.initialized.load(Ordering::SeqCst)
    {
        return MCPResponse::error(
            request.id,
            MCPError::invalid_request(
                "Session not initialized. Send initialize request first.".to_string(),
            ),
        );
    }

    let result = match request.method.as_str() {
        // CRITICAL: Initialize must be first interaction (doesn't need NodeService)
        "initialize" => crate::mcp::handlers::initialize::handle_initialize(request.params),

        // Ping for connection health checks (doesn't need NodeService or initialization)
        "ping" => Ok(json!({})),

        // Normal node operations (require initialization to have completed first)
        "create_node" => nodes::handle_create_node(service, request.params).await,
        "get_node" => nodes::handle_get_node(service, request.params).await,
        "update_node" => nodes::handle_update_node(service, request.params).await,
        "delete_node" => nodes::handle_delete_node(service, request.params).await,
        "query_nodes" => nodes::handle_query_nodes(service, request.params).await,
        "create_nodes_from_markdown" => {
            crate::mcp::handlers::markdown::handle_create_nodes_from_markdown(
                service,
                request.params,
            )
            .await
        }
        "get_markdown_from_node_id" => {
            crate::mcp::handlers::markdown::handle_get_markdown_from_node_id(
                service,
                request.params,
            )
            .await
        }
        _ => {
            warn!("⚠️  Unknown MCP method: {}", request.method);
            Err(MCPError::method_not_found(&request.method))
        }
    };

    match result {
        Ok(result) => {
            debug!("✅ MCP request {} succeeded", request.id);
            MCPResponse::success(request.id, result)
        }
        Err(error) => {
            error!(
                "❌ MCP request {} failed: {} (code: {})",
                request.id, error.message, error.code
            );
            MCPResponse::error(request.id, error)
        }
    }
}

/// Handle a JSON-RPC notification (no response expected)
#[instrument(skip(state), fields(method = %notification.method))]
async fn handle_notification(state: &ServerState, notification: MCPNotification) {
    match notification.method.as_str() {
        "initialized" => {
            state.initialized.store(true, Ordering::SeqCst);
            info!("✅ MCP session initialized - ready for operations");
        }
        _ => {
            debug!("Received notification: {}", notification.method);
        }
    }
}

/// Write a JSON-RPC response to stdout
async fn write_response(
    writer: &mut BufWriter<tokio::io::Stdout>,
    response: &MCPResponse,
) -> anyhow::Result<()> {
    let json = serde_json::to_string(response)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}
