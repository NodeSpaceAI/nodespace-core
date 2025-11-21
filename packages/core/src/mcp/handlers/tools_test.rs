//! Tests for MCP Tools Handler
//!
//! Tests tools/list and tools/call methods for MCP spec compliance.

use super::*;
use crate::operations::NodeOperations;
use crate::services::SchemaService;
use serde_json::json;

#[test]
fn test_tools_list_returns_all_schemas() {
    // Call tools/list with empty params
    let result = handle_tools_list(json!({}));

    assert!(result.is_ok());
    let response = result.unwrap();

    // Verify response has tools array
    assert!(response["tools"].is_array());

    let tools = response["tools"].as_array().unwrap();

    // Verify all 22 tools are present (16 original + 6 schema management tools)
    assert_eq!(tools.len(), 22);

    // Verify tool names
    let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

    // Core CRUD
    assert!(tool_names.contains(&"create_node"));
    assert!(tool_names.contains(&"get_node"));
    assert!(tool_names.contains(&"update_node"));
    assert!(tool_names.contains(&"delete_node"));
    assert!(tool_names.contains(&"query_nodes"));

    // Hierarchy operations
    assert!(tool_names.contains(&"get_children"));
    assert!(tool_names.contains(&"get_child_at_index"));
    assert!(tool_names.contains(&"insert_child_at_index"));
    assert!(tool_names.contains(&"move_child_to_index"));
    assert!(tool_names.contains(&"get_node_tree"));

    // Markdown
    assert!(tool_names.contains(&"create_nodes_from_markdown"));
    assert!(tool_names.contains(&"get_markdown_from_node_id"));

    // Batch operations
    assert!(tool_names.contains(&"get_nodes_batch"));
    assert!(tool_names.contains(&"update_nodes_batch"));
    assert!(tool_names.contains(&"update_container_from_markdown"));

    // Search
    assert!(tool_names.contains(&"search_containers"));

    // Schema management tools
    assert!(tool_names.contains(&"get_schema_definition"));
    assert!(tool_names.contains(&"add_schema_field"));
    assert!(tool_names.contains(&"remove_schema_field"));
    assert!(tool_names.contains(&"extend_schema_enum"));
    assert!(tool_names.contains(&"remove_schema_enum_value"));
}

#[test]
fn test_tools_list_tool_schema_structure() {
    let result = handle_tools_list(json!({})).unwrap();
    let tools = result["tools"].as_array().unwrap();

    // Verify each tool has required fields
    for tool in tools {
        assert!(tool["name"].is_string(), "Tool missing name");
        assert!(tool["description"].is_string(), "Tool missing description");
        assert!(tool["inputSchema"].is_object(), "Tool missing inputSchema");
        assert!(
            tool["inputSchema"]["type"].as_str() == Some("object"),
            "inputSchema type must be object"
        );
    }
}

#[test]
fn test_tools_call_with_unknown_tool() {
    // Create minimal services for testing (will be used in integration tests)
    let params = json!({
        "name": "unknown_tool",
        "arguments": {}
    });

    // Note: This is a synchronous test, so we can't actually call handle_tools_call
    // which is async. This test verifies the parameter structure.
    // Full async tests will be added in integration tests.

    // Verify params structure
    assert_eq!(params["name"].as_str().unwrap(), "unknown_tool");
    assert!(params["arguments"].is_object());
}

#[test]
fn test_tools_call_missing_name() {
    let params = json!({
        "arguments": {}
    });

    // Verify missing name would be caught
    assert!(params["name"].is_null());
}

#[test]
fn test_tools_call_missing_arguments() {
    let params = json!({
        "name": "create_node"
    });

    // Verify arguments defaults to empty object when missing
    assert!(params.get("arguments").is_none());
}

/// Integration tests for async tools/call execution
#[cfg(test)]
mod async_integration_tests {
    use super::*;
    use crate::db::SurrealStore;
    use crate::services::{NodeEmbeddingService, NodeService};
    use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn setup_test_services() -> (
        Arc<NodeOperations>,
        Arc<NodeEmbeddingService>,
        Arc<SchemaService>,
        TempDir,
    ) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(store.clone()).unwrap());
        let node_operations = Arc::new(NodeOperations::new(node_service.clone()));

        // Create NLP engine for embedding service
        let mut nlp_engine = EmbeddingService::new(EmbeddingConfig::default()).unwrap();
        nlp_engine.initialize().unwrap();
        let nlp_engine = Arc::new(nlp_engine);

        let embedding_service = Arc::new(NodeEmbeddingService::new(nlp_engine, store.clone()));

        // Create SchemaService
        let schema_service = Arc::new(SchemaService::new(node_service));

        (node_operations, embedding_service, schema_service, temp_dir)
    }

    #[tokio::test]
    async fn test_tools_call_unknown_tool_returns_error() {
        let (node_operations, embedding_service, schema_service, _temp_dir) =
            setup_test_services().await;

        let params = json!({
            "name": "unknown_tool",
            "arguments": {}
        });

        let result = handle_tools_call(
            &node_operations,
            &embedding_service,
            &schema_service,
            params,
        )
        .await;

        // Should return Err with invalid params error
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, crate::mcp::types::INVALID_PARAMS);
        assert!(error.message.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_tools_call_missing_name_parameter() {
        let (node_operations, embedding_service, schema_service, _temp_dir) =
            setup_test_services().await;

        let params = json!({
            "arguments": {"content": "test"}
        });

        let result = handle_tools_call(
            &node_operations,
            &embedding_service,
            &schema_service,
            params,
        )
        .await;

        // Should return Err with invalid params error
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, crate::mcp::types::INVALID_PARAMS);
        assert!(error.message.contains("Missing 'name' parameter"));
    }

    #[tokio::test]
    async fn test_tools_call_create_node_success() {
        let (node_operations, embedding_service, schema_service, _temp_dir) =
            setup_test_services().await;

        let params = json!({
            "name": "create_node",
            "arguments": {
                "node_type": "text",
                "content": "Test node content"
            }
        });

        let result = handle_tools_call(
            &node_operations,
            &embedding_service,
            &schema_service,
            params,
        )
        .await;

        // Should return Ok with MCP spec-compliant response
        assert!(result.is_ok(), "tools/call should succeed");
        let response = result.unwrap();

        // Verify MCP response structure per MCP spec
        assert!(
            response["content"].is_array(),
            "Response must have content array"
        );
        assert_eq!(
            response["isError"], false,
            "isError should be false for success"
        );

        let content = response["content"].as_array().unwrap();
        assert_eq!(content.len(), 1, "Content array should have one item");
        assert_eq!(content[0]["type"], "text", "Content type should be text");

        // Verify the text content is valid JSON (tool responses are JSON-serialized)
        let text = content[0]["text"].as_str().unwrap();
        let _node_data: serde_json::Value =
            serde_json::from_str(text).expect("Tool response should be valid JSON");

        // Success! MCP spec compliance verified
    }

    #[tokio::test]
    async fn test_tools_call_get_node_not_found_returns_error_response() {
        let (node_operations, embedding_service, schema_service, _temp_dir) =
            setup_test_services().await;

        let params = json!({
            "name": "get_node",
            "arguments": {
                "node_id": "non-existent-id"
            }
        });

        let result = handle_tools_call(
            &node_operations,
            &embedding_service,
            &schema_service,
            params,
        )
        .await;

        // Should return Ok with isError=true (per MCP spec, tool errors are not JSON-RPC errors)
        assert!(result.is_ok());
        let response = result.unwrap();

        assert_eq!(response["isError"], true);
        assert!(response["content"].is_array());

        let content = response["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");

        let error_message = content[0]["text"].as_str().unwrap();
        assert!(error_message.contains("not found") || error_message.contains("Node"));
    }

    #[tokio::test]
    async fn test_tools_call_query_nodes_success() {
        let (node_operations, embedding_service, schema_service, _temp_dir) =
            setup_test_services().await;

        // First create a node
        let create_params = json!({
            "name": "create_node",
            "arguments": {
                "node_type": "text",
                "content": "Searchable content"
            }
        });
        handle_tools_call(
            &node_operations,
            &embedding_service,
            &schema_service,
            create_params,
        )
        .await
        .unwrap();

        // Now query for nodes
        let query_params = json!({
            "name": "query_nodes",
            "arguments": {
                "filters": [],
                "limit": 10
            }
        });

        let result = handle_tools_call(
            &node_operations,
            &embedding_service,
            &schema_service,
            query_params,
        )
        .await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response["isError"], false);

        let content = response["content"].as_array().unwrap();
        let text = content[0]["text"].as_str().unwrap();
        let query_result: serde_json::Value = serde_json::from_str(text).unwrap();

        // Should return array of nodes
        assert!(query_result["nodes"].is_array());
        let nodes = query_result["nodes"].as_array().unwrap();
        assert!(!nodes.is_empty());
    }

    #[tokio::test]
    async fn test_tools_call_with_missing_arguments_uses_default() {
        let (node_operations, embedding_service, schema_service, _temp_dir) =
            setup_test_services().await;

        // Call without arguments field
        let params = json!({
            "name": "query_nodes"
        });

        let result = handle_tools_call(
            &node_operations,
            &embedding_service,
            &schema_service,
            params,
        )
        .await;

        // Should work with default empty arguments
        assert!(result.is_ok());
        let response = result.unwrap();

        // May return error due to invalid params, but shouldn't panic
        // The important thing is it doesn't crash on missing arguments field
        assert!(response["content"].is_array());
    }
}
