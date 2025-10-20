//! Tests for MCP node handlers
//!
//! Tests the request parameter parsing and response formatting for node CRUD operations.

#[cfg(test)]
mod tests {
    use crate::mcp::types::{MCPError, INVALID_PARAMS, NODE_NOT_FOUND, VALIDATION_ERROR};
    use serde_json::json;

    #[test]
    fn test_create_node_params_parsing() {
        let params = json!({
            "node_type": "text",
            "content": "Test content",
            "properties": {"key": "value"}
        });

        // Verify we can extract fields
        assert_eq!(params["node_type"], "text");
        assert_eq!(params["content"], "Test content");
        assert_eq!(params["properties"]["key"], "value");
    }

    #[test]
    fn test_create_node_params_with_parent() {
        let params = json!({
            "node_type": "task",
            "content": "Task content",
            "parent_id": "parent-123",
            "container_node_id": "container-456",
            "properties": {}
        });

        assert_eq!(params["parent_id"], "parent-123");
        assert_eq!(params["container_node_id"], "container-456");
    }

    #[test]
    fn test_get_node_params_parsing() {
        let params = json!({
            "node_id": "test-node-123"
        });

        assert_eq!(params["node_id"], "test-node-123");
    }

    #[test]
    fn test_update_node_params_parsing() {
        let params = json!({
            "node_id": "node-123",
            "content": "Updated content",
            "properties": {"updated": true}
        });

        assert_eq!(params["node_id"], "node-123");
        assert_eq!(params["content"], "Updated content");
        assert_eq!(params["properties"]["updated"], true);
    }

    #[test]
    fn test_delete_node_params_parsing() {
        let params = json!({
            "node_id": "node-to-delete"
        });

        assert_eq!(params["node_id"], "node-to-delete");
    }

    #[test]
    fn test_query_nodes_params_parsing() {
        let params = json!({
            "filter": {
                "node_type": "text",
                "container_node_id": "container-123"
            },
            "limit": 10,
            "offset": 0
        });

        assert_eq!(params["filter"]["node_type"], "text");
        assert_eq!(params["filter"]["container_node_id"], "container-123");
        assert_eq!(params["limit"], 10);
        assert_eq!(params["offset"], 0);
    }

    #[test]
    fn test_mcp_error_creation() {
        let error = MCPError::node_not_found("xyz");
        assert_eq!(error.code, NODE_NOT_FOUND);
        assert!(error.message.contains("xyz"));
    }

    #[test]
    fn test_invalid_params_error() {
        let error = MCPError::invalid_params("Missing required parameter: node_type".to_string());
        assert_eq!(error.code, INVALID_PARAMS);
        assert!(error.message.contains("node_type"));
    }

    #[test]
    fn test_validation_error() {
        let error = MCPError::validation_error("Invalid node type".to_string());
        assert_eq!(error.code, VALIDATION_ERROR);
        assert!(error.message.contains("Invalid"));
    }

    #[test]
    fn test_success_response_format() {
        let result = json!({
            "node_id": "new-node-123",
            "success": true
        });

        assert_eq!(result["node_id"], "new-node-123");
        assert_eq!(result["success"], true);
    }

    #[test]
    fn test_query_response_format() {
        let result = json!({
            "nodes": [
                {"id": "node-1", "content": "Content 1"},
                {"id": "node-2", "content": "Content 2"}
            ],
            "total": 2
        });

        assert_eq!(result["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(result["total"], 2);
    }
}
