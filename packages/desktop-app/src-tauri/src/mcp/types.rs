//! MCP JSON-RPC 2.0 Types
//!
//! Type definitions for Model Context Protocol communication.
//! Implements JSON-RPC 2.0 specification for stdio-based MCP transport.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 request structure
///
/// # Example
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "id": 123,
///     "method": "create_node",
///     "params": {
///         "node_type": "task",
///         "content": "Review quarterly reports"
///     }
/// }
/// ```
#[derive(Debug, Deserialize)]
pub struct MCPRequest {
    /// JSON-RPC version (must be "2.0")
    pub jsonrpc: String,

    /// Request identifier (used to match responses)
    pub id: u64,

    /// Method name to invoke
    pub method: String,

    /// Method parameters as JSON value
    pub params: Value,
}

/// JSON-RPC 2.0 response structure
///
/// # Success Example
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "id": 123,
///     "result": { "node_id": "abc123", "success": true }
/// }
/// ```
///
/// # Error Example
///
/// ```json
/// {
///     "jsonrpc": "2.0",
///     "id": 123,
///     "error": {
///         "code": -32600,
///         "message": "Invalid request"
///     }
/// }
/// ```
#[derive(Debug, Serialize)]
pub struct MCPResponse {
    /// JSON-RPC version (always "2.0")
    pub jsonrpc: String,

    /// Request identifier (matches request)
    pub id: u64,

    /// Success result (mutually exclusive with error)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,

    /// Error information (mutually exclusive with result)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<MCPError>,
}

/// JSON-RPC 2.0 error structure
#[derive(Debug, Serialize, Clone)]
pub struct MCPError {
    /// Error code (standard JSON-RPC or NodeSpace-specific)
    pub code: i32,

    /// Human-readable error message
    pub message: String,
}

// JSON-RPC 2.0 standard error codes
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

// NodeSpace-specific error codes (application errors: -32000 to -32099)
pub const NODE_NOT_FOUND: i32 = -32000;
pub const NODE_CREATION_FAILED: i32 = -32001;
pub const NODE_UPDATE_FAILED: i32 = -32002;
pub const NODE_DELETE_FAILED: i32 = -32003;
pub const VALIDATION_ERROR: i32 = -32004;

impl MCPError {
    /// Create a parse error
    pub fn parse_error(message: String) -> Self {
        Self {
            code: PARSE_ERROR,
            message,
        }
    }

    /// Create an invalid request error
    pub fn invalid_request(message: String) -> Self {
        Self {
            code: INVALID_REQUEST,
            message,
        }
    }

    /// Create a method not found error
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: METHOD_NOT_FOUND,
            message: format!("Method not found: {}", method),
        }
    }

    /// Create an invalid params error
    pub fn invalid_params(message: String) -> Self {
        Self {
            code: INVALID_PARAMS,
            message,
        }
    }

    /// Create an internal error
    pub fn internal_error(message: String) -> Self {
        Self {
            code: INTERNAL_ERROR,
            message,
        }
    }

    /// Create a node not found error
    pub fn node_not_found(node_id: &str) -> Self {
        Self {
            code: NODE_NOT_FOUND,
            message: format!("Node not found: {}", node_id),
        }
    }

    /// Create a node creation failed error
    pub fn node_creation_failed(message: String) -> Self {
        Self {
            code: NODE_CREATION_FAILED,
            message,
        }
    }

    /// Create a node update failed error
    pub fn node_update_failed(message: String) -> Self {
        Self {
            code: NODE_UPDATE_FAILED,
            message,
        }
    }

    /// Create a node delete failed error
    pub fn node_delete_failed(message: String) -> Self {
        Self {
            code: NODE_DELETE_FAILED,
            message,
        }
    }

    /// Create a validation error
    pub fn validation_error(message: String) -> Self {
        Self {
            code: VALIDATION_ERROR,
            message,
        }
    }
}

impl MCPResponse {
    /// Create a success response
    pub fn success(id: u64, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response
    pub fn error(id: u64, error: MCPError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}
