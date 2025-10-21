//! MCP Server with Transport Abstraction
//!
//! Async Tokio task that handles JSON-RPC 2.0 requests over multiple transports:
//! - stdio: for CLI tools and testing
//! - HTTP: for GUI apps and Claude Code integration
//!
//! Both transports share the same request handler logic and optional callbacks.

use crate::mcp::handlers::nodes;
use crate::mcp::types::{MCPError, MCPNotification, MCPRequest, MCPResponse};
use crate::services::{NodeEmbeddingService, NodeService};
use axum::{extract::State, http::StatusCode, routing::get, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, instrument, warn};

/// Transport mode for MCP server
#[derive(Debug, Clone, Copy)]
pub enum McpTransport {
    /// Stdio transport (CLI tools, testing, headless mode)
    Stdio,
    /// HTTP transport (GUI apps, Claude Code integration)
    Http { port: u16 },
}

/// Combined services for MCP handlers
pub struct McpServices {
    pub node_service: Arc<NodeService>,
    pub embedding_service: Arc<NodeEmbeddingService>,
}

/// Server state tracking initialization status
///
/// Uses Arc<AtomicBool> for thread-safe sharing across async tasks.
/// The initialized flag must persist across requests in HTTP mode
/// to maintain session state according to the MCP protocol.
///
/// The MCP protocol requires:
/// 1. Client sends `initialize` request
/// 2. Server responds with capabilities
/// 3. Client sends `initialized` notification
/// 4. Only then can other methods be called
struct ServerState {
    /// Whether the client has completed the initialization handshake
    initialized: Arc<AtomicBool>,
}

/// Callback type for handling successful responses
///
/// Receives (method_name, result_value) after successful operation execution.
/// Useful for event emissions or logging in framework integrations.
pub type ResponseCallback = Arc<dyn Fn(&str, &Value) + Send + Sync>;

/// Run the MCP server with specified transport
///
/// Starts the MCP server using the specified transport (stdio or HTTP).
/// Both transports use the same request handler logic.
///
/// # Arguments
///
/// * `services` - Combined McpServices struct with both NodeService and NodeEmbeddingService
/// * `transport` - Transport mode (Stdio or Http with port)
///
/// # Returns
///
/// Returns Ok(()) when the server shuts down, or Err on fatal errors
#[instrument(skip(services))]
pub async fn run_mcp_server(services: McpServices, transport: McpTransport) -> anyhow::Result<()> {
    run_mcp_server_with_callback(services, transport, None).await
}

/// Run the MCP server with specified transport and optional callback
///
/// Starts the MCP server using the specified transport, with an optional
/// callback function invoked after each successful operation.
///
/// # Arguments
///
/// * `services` - Combined McpServices struct with both NodeService and NodeEmbeddingService
/// * `transport` - Transport mode (Stdio or Http with port)
/// * `callback` - Optional callback invoked with (method, result) on success
///
/// # Returns
///
/// Returns Ok(()) when the server shuts down, or Err on fatal errors
#[instrument(skip(services, callback))]
pub async fn run_mcp_server_with_callback(
    services: McpServices,
    transport: McpTransport,
    callback: Option<ResponseCallback>,
) -> anyhow::Result<()> {
    match transport {
        McpTransport::Stdio => run_stdio_server(services, callback).await,
        McpTransport::Http { port } => run_http_server(services, port, callback).await,
    }
}

/// Run the MCP server over stdio
///
/// Reads JSON-RPC requests from stdin, processes them via handlers,
/// and writes responses to stdout. Runs indefinitely until EOF on stdin.
#[instrument(skip(services, callback))]
async fn run_stdio_server(
    services: McpServices,
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
                let response = handle_request(&services, &state, request).await;

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

/// Run the MCP server over HTTP
///
/// Starts an HTTP server listening on the specified port that accepts
/// JSON-RPC 2.0 requests via POST requests to /mcp endpoint.
/// Also provides a /health endpoint for monitoring.
#[instrument(skip(services, callback))]
async fn run_http_server(
    services: McpServices,
    port: u16,
    callback: Option<ResponseCallback>,
) -> anyhow::Result<()> {
    info!(
        "🔌 MCP HTTP server (GUI mode) starting on http://localhost:{}",
        port
    );

    // Wrap services, callback, and state in Arc for sharing across requests
    let shared_services = Arc::new(services);
    let shared_callback = callback;
    let shared_state = Arc::new(ServerState {
        initialized: Arc::new(AtomicBool::new(false)),
    });

    // Create router with MCP handler and health check
    let app = Router::new()
        .route("/mcp", post(handle_http_mcp_request))
        .route("/health", get(handle_health_check))
        .layer(TraceLayer::new_for_http())
        .with_state((shared_services, shared_callback, shared_state));

    // Bind to localhost only (no network exposure)
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;

    info!(
        "✅ MCP HTTP server (GUI mode) listening on http://localhost:{}",
        port
    );
    info!("   📍 MCP endpoint: http://localhost:{}/mcp", port);
    info!("   🏥 Health check: http://localhost:{}/health", port);

    // Run the server
    axum::serve(listener, app).await?;

    info!("🔌 MCP HTTP server (GUI mode) stopped");
    Ok(())
}

/// Health check endpoint
///
/// Returns a simple JSON response indicating server is running.
async fn handle_health_check() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "NodeSpace MCP Server",
        "transport": "http"
    }))
}

/// Handle HTTP MCP request
///
/// Accepts JSON-RPC 2.0 requests in the body, processes them,
/// and returns JSON-RPC responses.
///
/// Note: For HTTP transport, the `initialized` notification is automatically
/// handled after a successful `initialize` request, since HTTP is stateless
/// and doesn't support notifications without responses.
async fn handle_http_mcp_request(
    State((services, callback, state)): State<(
        Arc<McpServices>,
        Option<ResponseCallback>,
        Arc<ServerState>,
    )>,
    Json(request): Json<MCPRequest>,
) -> Result<Json<MCPResponse>, StatusCode> {
    debug!("📥 HTTP MCP request: {}", request.method);

    let request_id = request.id;
    let method = request.method.clone();

    // Special handling for initialize: automatically mark as initialized after successful response
    // This is necessary because HTTP transport can't receive the separate `initialized` notification
    let is_initialize = method == "initialize";

    // Handle the request using shared state
    let response = handle_request(services.as_ref(), &state, request).await;

    // Auto-initialize for HTTP transport after successful initialize request
    if is_initialize && response.result.is_some() {
        state.initialized.store(true, Ordering::SeqCst);
        info!("✅ MCP session initialized (HTTP mode) - ready for operations");
    }

    // Invoke callback on successful response
    if let Some(ref cb) = callback {
        if let Some(ref result) = response.result {
            cb(&method, result);
        }
    }

    debug!(
        "📤 HTTP MCP response for method '{}' (id={})",
        method, request_id
    );

    Ok(Json(response))
}

/// Handle a JSON-RPC request and return a response
#[instrument(skip(services, state), fields(method = %request.method, id = %request.id))]
async fn handle_request(
    services: &McpServices,
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
        // CRITICAL: Initialize must be first interaction (doesn't need any services)
        "initialize" => crate::mcp::handlers::initialize::handle_initialize(request.params),

        // Ping for connection health checks (doesn't need services or initialization)
        "ping" => Ok(json!({})),

        // Normal node operations (require initialization to have completed first)
        "create_node" => nodes::handle_create_node(&services.node_service, request.params).await,
        "get_node" => nodes::handle_get_node(&services.node_service, request.params).await,
        "update_node" => nodes::handle_update_node(&services.node_service, request.params).await,
        "delete_node" => nodes::handle_delete_node(&services.node_service, request.params).await,
        "query_nodes" => nodes::handle_query_nodes(&services.node_service, request.params).await,
        "create_nodes_from_markdown" => {
            crate::mcp::handlers::markdown::handle_create_nodes_from_markdown(
                &services.node_service,
                request.params,
            )
            .await
        }
        "get_markdown_from_node_id" => {
            crate::mcp::handlers::markdown::handle_get_markdown_from_node_id(
                &services.node_service,
                request.params,
            )
            .await
        }

        // Search operations (require initialization)
        "search_containers" => {
            crate::mcp::handlers::search::handle_search_containers(
                &services.embedding_service,
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode as HttpStatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[test]
    fn test_transport_enum_creation() {
        let _stdio = McpTransport::Stdio;
        let _http = McpTransport::Http { port: 3001 };
        // Tests compile successfully
    }

    #[test]
    fn test_transport_stdio_variant() {
        let transport = McpTransport::Stdio;
        match transport {
            McpTransport::Stdio => {
                // Expected
            }
            McpTransport::Http { .. } => {
                panic!("Expected Stdio variant");
            }
        }
    }

    #[test]
    fn test_transport_http_variant_with_port() {
        let port = 3001;
        let transport = McpTransport::Http { port };
        match transport {
            McpTransport::Http { port: p } => {
                assert_eq!(p, 3001);
            }
            McpTransport::Stdio => {
                panic!("Expected Http variant");
            }
        }
    }

    #[test]
    fn test_transport_http_different_ports() {
        let http_3001 = McpTransport::Http { port: 3001 };
        let http_8080 = McpTransport::Http { port: 8080 };

        if let McpTransport::Http { port: p1 } = http_3001 {
            if let McpTransport::Http { port: p2 } = http_8080 {
                assert_eq!(p1, 3001);
                assert_eq!(p2, 8080);
                assert_ne!(p1, p2);
            }
        }
    }

    #[test]
    fn test_response_callback_type_compiles() {
        // Verify the ResponseCallback type is correctly defined
        // and can be used with Arc and Send + Sync
        let _callback: Option<ResponseCallback> = None;

        let _callback: Option<ResponseCallback> =
            Some(Arc::new(|_method: &str, _result: &Value| {
                // No-op callback for testing
            }));
    }

    // Helper to create test services
    async fn create_test_services() -> McpServices {
        use crate::db::DatabaseService;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = DatabaseService::new(db_path).await.unwrap();
        let db_arc = Arc::new(db);
        let node_service = Arc::new(NodeService::new((*db_arc).clone()).unwrap());
        let embedding_service = Arc::new(NodeEmbeddingService::new_with_defaults(db_arc).unwrap());

        McpServices {
            node_service,
            embedding_service,
        }
    }

    #[tokio::test]
    async fn test_http_health_check() {
        let services = create_test_services().await;
        let shared_state = Arc::new(ServerState {
            initialized: Arc::new(AtomicBool::new(false)),
        });

        let app = Router::new()
            .route("/health", get(handle_health_check))
            .with_state((Arc::new(services), None::<ResponseCallback>, shared_state));

        let request = Request::builder()
            .uri("/health")
            .method("GET")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), HttpStatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "NodeSpace MCP Server");
        assert_eq!(json["transport"], "http");
    }

    #[tokio::test]
    async fn test_http_initialize_request() {
        let services = create_test_services().await;
        let shared_state = Arc::new(ServerState {
            initialized: Arc::new(AtomicBool::new(false)),
        });

        let app = Router::new()
            .route("/mcp", post(handle_http_mcp_request))
            .with_state((
                Arc::new(services),
                None::<ResponseCallback>,
                shared_state.clone(),
            ));

        let init_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0.0"
                }
            }
        });

        let request = Request::builder()
            .uri("/mcp")
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&init_request).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), HttpStatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert!(json["result"].is_object());
        assert!(json["result"]["capabilities"].is_object());

        // Verify state was updated (HTTP auto-initializes)
        assert!(shared_state.initialized.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_http_ping_without_initialization() {
        let services = create_test_services().await;
        let shared_state = Arc::new(ServerState {
            initialized: Arc::new(AtomicBool::new(false)),
        });

        let app = Router::new()
            .route("/mcp", post(handle_http_mcp_request))
            .with_state((Arc::new(services), None::<ResponseCallback>, shared_state));

        let ping_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "ping",
            "params": {}
        });

        let request = Request::builder()
            .uri("/mcp")
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&ping_request).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), HttpStatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert!(json["result"].is_object());
    }

    #[tokio::test]
    async fn test_http_request_before_initialization_fails() {
        let services = create_test_services().await;
        let shared_state = Arc::new(ServerState {
            initialized: Arc::new(AtomicBool::new(false)),
        });

        let app = Router::new()
            .route("/mcp", post(handle_http_mcp_request))
            .with_state((Arc::new(services), None::<ResponseCallback>, shared_state));

        let create_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "create_node",
            "params": {
                "node_type": "text",
                "content": "test"
            }
        });

        let request = Request::builder()
            .uri("/mcp")
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&create_request).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), HttpStatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert!(json["error"].is_object());
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not initialized"));
    }

    #[tokio::test]
    async fn test_http_callback_invoked() {
        use std::sync::Mutex;

        let services = create_test_services().await;
        let shared_state = Arc::new(ServerState {
            initialized: Arc::new(AtomicBool::new(true)), // Pre-initialized
        });

        let callback_invoked = Arc::new(Mutex::new(false));
        let callback_invoked_clone = callback_invoked.clone();

        let callback: ResponseCallback = Arc::new(move |_method, _result| {
            *callback_invoked_clone.lock().unwrap() = true;
        });

        let app = Router::new()
            .route("/mcp", post(handle_http_mcp_request))
            .with_state((Arc::new(services), Some(callback), shared_state));

        let ping_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "ping",
            "params": {}
        });

        let request = Request::builder()
            .uri("/mcp")
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&ping_request).unwrap()))
            .unwrap();

        let _response = app.oneshot(request).await.unwrap();

        assert!(*callback_invoked.lock().unwrap());
    }
}
