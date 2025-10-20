//! MCP stdio Server
//!
//! Async Tokio task that handles JSON-RPC 2.0 requests over stdin/stdout.
//! Runs as a background task in the Tauri application process.

use crate::mcp::handlers::nodes;
use crate::mcp::types::{MCPError, MCPRequest, MCPResponse};
use nodespace_core::NodeService;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tracing::{debug, error, info, warn};

/// Run the MCP stdio server
///
/// Reads JSON-RPC requests from stdin, processes them via handlers,
/// and writes responses to stdout. Runs indefinitely until EOF on stdin.
///
/// # Arguments
///
/// * `node_service` - Shared NodeService instance
/// * `app` - Tauri AppHandle for event emission
///
/// # Returns
///
/// Returns Ok(()) when stdin is closed, or Err on fatal errors
pub async fn run_mcp_server(node_service: Arc<NodeService>, app: AppHandle) -> anyhow::Result<()> {
    info!("🔌 MCP stdio server started");

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let reader = BufReader::new(stdin);
    let mut writer = BufWriter::new(stdout);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        debug!("📥 MCP request: {}", line);

        // Parse JSON-RPC request
        let request: MCPRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                warn!("❌ Failed to parse JSON-RPC request: {}", e);
                let error_response = MCPResponse::error(
                    0, // Unknown ID since parsing failed
                    MCPError::parse_error(format!("Invalid JSON: {}", e)),
                );
                write_response(&mut writer, &error_response).await?;
                continue;
            }
        };

        let request_id = request.id;
        let method = request.method.clone();

        // Handle request
        let response = handle_request(&node_service, &app, request).await;

        debug!(
            "📤 MCP response for method '{}' (id={})",
            method, request_id
        );

        // Write response
        write_response(&mut writer, &response).await?;
    }

    info!("🔌 MCP stdio server stopped (stdin closed)");
    Ok(())
}

/// Handle a JSON-RPC request and return a response
async fn handle_request(
    service: &Arc<NodeService>,
    app: &AppHandle,
    request: MCPRequest,
) -> MCPResponse {
    let result = match request.method.as_str() {
        "create_node" => nodes::handle_create_node(service, app, request.params).await,
        "get_node" => nodes::handle_get_node(service, request.params).await,
        "update_node" => nodes::handle_update_node(service, app, request.params).await,
        "delete_node" => nodes::handle_delete_node(service, app, request.params).await,
        "query_nodes" => nodes::handle_query_nodes(service, request.params).await,
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
