//! Tests for MCP types module
//!
//! Verifies JSON-RPC 2.0 request/response parsing and error handling.

#[cfg(test)]
mod tests {
    use crate::mcp::types::{
        MCPError, MCPNotification, MCPRequest, MCPResponse, INTERNAL_ERROR, INVALID_PARAMS,
        INVALID_REQUEST, METHOD_NOT_FOUND, NODE_CREATION_FAILED, NODE_DELETE_FAILED,
        NODE_NOT_FOUND, NODE_UPDATE_FAILED, PARSE_ERROR, VALIDATION_ERROR,
    };
    use serde_json::json;

    #[test]
    fn test_parse_valid_request() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "id": 123,
            "method": "create_node",
            "params": {
                "node_type": "text",
                "content": "Test content"
            }
        }"#;

        let request: MCPRequest = serde_json::from_str(json_str).unwrap();

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.id, 123);
        assert_eq!(request.method, "create_node");
        assert!(request.params.is_object());
    }

    #[test]
    fn test_parse_request_missing_jsonrpc() {
        let json_str = r#"{
            "id": 123,
            "method": "create_node",
            "params": {}
        }"#;

        let result: Result<MCPRequest, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_serialize_success_response() {
        let response = MCPResponse {
            jsonrpc: "2.0".to_string(),
            id: 42,
            result: Some(json!({"success": true, "node_id": "abc123"})),
            error: None,
        };

        let json = serde_json::to_value(&response).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 42);
        assert_eq!(json["result"]["success"], true);
        assert_eq!(json["result"]["node_id"], "abc123");
        assert!(json.get("error").is_none()); // Should be omitted
    }

    #[test]
    fn test_serialize_error_response() {
        let response = MCPResponse {
            jsonrpc: "2.0".to_string(),
            id: 99,
            result: None,
            error: Some(MCPError {
                code: NODE_NOT_FOUND,
                message: "Node not found: xyz789".to_string(),
                data: None,
            }),
        };

        let json = serde_json::to_value(&response).unwrap();

        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 99);
        assert_eq!(json["error"]["code"], NODE_NOT_FOUND);
        assert_eq!(json["error"]["message"], "Node not found: xyz789");
        assert!(json.get("result").is_none()); // Should be omitted
    }

    #[test]
    fn test_error_codes_constants() {
        // Standard JSON-RPC error codes
        assert_eq!(PARSE_ERROR, -32700);
        assert_eq!(INVALID_REQUEST, -32600);
        assert_eq!(METHOD_NOT_FOUND, -32601);
        assert_eq!(INVALID_PARAMS, -32602);
        assert_eq!(INTERNAL_ERROR, -32603);

        // Custom NodeSpace error codes (start at -32000 per spec)
        assert_eq!(NODE_NOT_FOUND, -32000);
        assert_eq!(NODE_CREATION_FAILED, -32001);
        assert_eq!(NODE_UPDATE_FAILED, -32002);
        assert_eq!(NODE_DELETE_FAILED, -32003);
        assert_eq!(VALIDATION_ERROR, -32004);
    }

    #[test]
    fn test_request_with_empty_params() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "get_node",
            "params": {"node_id": "test-123"}
        }"#;

        let request: MCPRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(request.params["node_id"], "test-123");
    }

    #[test]
    fn test_response_serialization() {
        let response = MCPResponse {
            jsonrpc: "2.0".to_string(),
            id: 777,
            result: Some(json!({"data": [1, 2, 3]})),
            error: None,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 777);
        assert_eq!(parsed["result"]["data"][0], 1);
        assert_eq!(parsed["result"]["data"][1], 2);
        assert_eq!(parsed["result"]["data"][2], 3);
        assert!(parsed.get("error").is_none());
    }

    #[test]
    fn test_mcp_error_serialization() {
        let error = MCPError {
            code: INVALID_PARAMS,
            message: "Missing required field: node_type".to_string(),
            data: None,
        };

        let json = serde_json::to_value(&error).unwrap();

        assert_eq!(json["code"], INVALID_PARAMS);
        assert_eq!(json["message"], "Missing required field: node_type");
    }

    #[test]
    fn test_mcp_error_helper_methods() {
        let parse_err = MCPError::parse_error("Invalid JSON".to_string());
        assert_eq!(parse_err.code, PARSE_ERROR);

        let not_found = MCPError::node_not_found("abc123");
        assert_eq!(not_found.code, NODE_NOT_FOUND);
        assert!(not_found.message.contains("abc123"));

        let invalid_params = MCPError::invalid_params("Missing field".to_string());
        assert_eq!(invalid_params.code, INVALID_PARAMS);
    }

    #[test]
    fn test_mcp_response_helper_methods() {
        let success = MCPResponse::success(42, json!({"result": "ok"}));
        assert_eq!(success.id, 42);
        assert_eq!(success.jsonrpc, "2.0");
        assert!(success.error.is_none());
        assert!(success.result.is_some());

        let error_resp = MCPResponse::error(99, MCPError::node_not_found("xyz"));
        assert_eq!(error_resp.id, 99);
        assert_eq!(error_resp.jsonrpc, "2.0");
        assert!(error_resp.result.is_none());
        assert!(error_resp.error.is_some());
    }

    // Notification tests

    #[test]
    fn test_parse_valid_notification() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        }"#;

        let notification: MCPNotification = serde_json::from_str(json_str).unwrap();

        assert_eq!(notification.jsonrpc, "2.0");
        assert_eq!(notification.method, "initialized");
        assert!(notification.params.is_object());
    }

    #[test]
    fn test_parse_notification_with_params() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "progress",
            "params": {
                "token": "abc123",
                "value": 50
            }
        }"#;

        let notification: MCPNotification = serde_json::from_str(json_str).unwrap();

        assert_eq!(notification.method, "progress");
        assert_eq!(notification.params["token"], "abc123");
        assert_eq!(notification.params["value"], 50);
    }

    #[test]
    fn test_notification_missing_jsonrpc() {
        let json_str = r#"{
            "method": "initialized",
            "params": {}
        }"#;

        let result: Result<MCPNotification, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_notification_invalid_jsonrpc_version() {
        let json_str = r#"{
            "jsonrpc": "1.0",
            "method": "initialized",
            "params": {}
        }"#;

        let result: Result<MCPNotification, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_notification_missing_method() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "params": {}
        }"#;

        let result: Result<MCPNotification, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_notification_with_id_should_be_request() {
        // If there's an id field, it should parse as request not notification
        let json_str = r#"{
            "jsonrpc": "2.0",
            "id": 123,
            "method": "initialize",
            "params": {}
        }"#;

        // Should parse as request
        let request: Result<MCPRequest, _> = serde_json::from_str(json_str);
        assert!(request.is_ok());

        // Should fail as notification (deny_unknown_fields will reject 'id')
        let notification: Result<MCPNotification, _> = serde_json::from_str(json_str);
        assert!(notification.is_err());
    }

    #[test]
    fn test_request_without_id_should_fail() {
        // Requests must have an id field
        let json_str = r#"{
            "jsonrpc": "2.0",
            "method": "create_node",
            "params": {}
        }"#;

        let result: Result<MCPRequest, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
    }
}
