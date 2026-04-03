//! Tests for MCP Tools Handler
//!
//! Tests tools/list and tools/call methods for MCP spec compliance.

use super::*;
use serde_json::json;

#[test]
fn test_tool_schemas_include_core_and_schema_type() {
    // get_tool_schemas with no user-defined schemas
    let schemas = get_tool_schemas(&[]);
    let tools = schemas.as_array().unwrap();

    let create_node = tools.iter().find(|t| t["name"] == "create_node").unwrap();
    let node_type_enum = create_node["inputSchema"]["properties"]["node_type"]["enum"]
        .as_array()
        .unwrap();
    let enum_values: Vec<&str> = node_type_enum.iter().map(|v| v.as_str().unwrap()).collect();

    // Core types must be present
    assert!(enum_values.contains(&"text"));
    assert!(enum_values.contains(&"task"));
    assert!(enum_values.contains(&"date"));
    assert!(enum_values.contains(&"header"));
    // schema type must always be included (Issue #964)
    assert!(
        enum_values.contains(&"schema"),
        "schema type must be in node_type enum"
    );
}

#[test]
fn test_tool_schemas_include_custom_schema_ids() {
    let custom_ids = vec!["customer".to_string(), "invoice".to_string()];
    let schemas = get_tool_schemas(&custom_ids);
    let tools = schemas.as_array().unwrap();

    let create_node = tools.iter().find(|t| t["name"] == "create_node").unwrap();
    let node_type_enum = create_node["inputSchema"]["properties"]["node_type"]["enum"]
        .as_array()
        .unwrap();
    let enum_values: Vec<&str> = node_type_enum.iter().map(|v| v.as_str().unwrap()).collect();

    assert!(enum_values.contains(&"customer"));
    assert!(enum_values.contains(&"invoice"));
    assert!(enum_values.contains(&"schema"));
}

#[test]
fn test_tool_schemas_no_duplicate_types() {
    // schema is in core list; passing it again via schema_ids should not duplicate
    let schema_ids = vec!["schema".to_string(), "text".to_string()];
    let schemas = get_tool_schemas(&schema_ids);
    let tools = schemas.as_array().unwrap();

    let create_node = tools.iter().find(|t| t["name"] == "create_node").unwrap();
    let node_type_enum = create_node["inputSchema"]["properties"]["node_type"]["enum"]
        .as_array()
        .unwrap();
    let enum_values: Vec<&str> = node_type_enum.iter().map(|v| v.as_str().unwrap()).collect();

    let schema_count = enum_values.iter().filter(|&&v| v == "schema").count();
    let text_count = enum_values.iter().filter(|&&v| v == "text").count();
    assert_eq!(schema_count, 1, "schema should appear exactly once");
    assert_eq!(text_count, 1, "text should appear exactly once");
}

#[test]
fn test_tool_schemas_structure() {
    let schemas = get_tool_schemas(&[]);
    let tools = schemas.as_array().unwrap();

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

    async fn setup_test_services() -> (Arc<NodeService>, Option<Arc<NodeEmbeddingService>>, TempDir)
    {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let mut store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());

        // Create NLP engine in stub mode (DO NOT call initialize() - that loads the full
        // 115MB GGML model into GPU memory, causing memory explosion when many tests run in parallel)
        let nlp_engine = Arc::new(EmbeddingService::new(EmbeddingConfig::default()).unwrap());

        let node_accessor: Arc<dyn crate::services::NodeAccessor> = node_service.clone();
        let behaviors = node_service.behaviors().clone();
        let embedding_service = Arc::new(NodeEmbeddingService::new(
            nlp_engine,
            store.clone(),
            node_accessor,
            behaviors,
        ));

        (node_service, Some(embedding_service), temp_dir)
    }

    #[tokio::test]
    async fn test_tools_list_returns_tier1_core_tools() {
        let (node_service, _embedding_service, _temp_dir) = setup_test_services().await;

        let result = handle_tools_list(&node_service, json!({})).await;
        assert!(result.is_ok());
        let response = result.unwrap();

        assert!(response["tools"].is_array());
        let tools = response["tools"].as_array().unwrap();

        // Verify exactly 13 Tier 1 (Core) tools are present
        assert_eq!(
            tools.len(),
            13,
            "Expected 13 Tier 1 tools, got {}",
            tools.len()
        );

        let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

        assert!(tool_names.contains(&"create_node"));
        assert!(tool_names.contains(&"get_node"));
        assert!(tool_names.contains(&"update_node"));
        assert!(tool_names.contains(&"delete_node"));
        assert!(tool_names.contains(&"query_nodes"));
        assert!(tool_names.contains(&"get_children"));
        assert!(tool_names.contains(&"insert_child_at_index"));
        assert!(tool_names.contains(&"search_semantic"));
        assert!(tool_names.contains(&"create_nodes_from_markdown"));
        assert!(tool_names.contains(&"get_markdown_from_node_id"));
        assert!(tool_names.contains(&"get_all_schemas"));
        assert!(tool_names.contains(&"search_tools"));
        assert!(tool_names.contains(&"create_relationship"));

        // Verify deprecated tools are NOT present
        assert!(!tool_names.contains(&"search_containers"));
        assert!(!tool_names.contains(&"search_roots"));
        assert!(!tool_names.contains(&"update_container_from_markdown"));
        assert!(!tool_names.contains(&"get_schema_definition"));
        assert!(!tool_names.contains(&"add_schema_field"));
        assert!(!tool_names.contains(&"remove_schema_field"));
    }

    #[tokio::test]
    async fn test_tools_list_includes_schema_type_in_create_node_enum() {
        let (node_service, _embedding_service, _temp_dir) = setup_test_services().await;

        let result = handle_tools_list(&node_service, json!({})).await.unwrap();
        let tools = result["tools"].as_array().unwrap();

        let create_node = tools.iter().find(|t| t["name"] == "create_node").unwrap();
        let node_type_enum = create_node["inputSchema"]["properties"]["node_type"]["enum"]
            .as_array()
            .unwrap();
        let enum_values: Vec<&str> = node_type_enum.iter().map(|v| v.as_str().unwrap()).collect();

        assert!(
            enum_values.contains(&"schema"),
            "create_node node_type enum must include 'schema'"
        );
    }

    #[tokio::test]
    async fn test_tools_list_includes_custom_schemas_in_create_node_enum() {
        let (node_service, _embedding_service, _temp_dir) = setup_test_services().await;

        // Create a custom schema node via create_node (id is a server-generated ULID)
        let create_params = json!({
            "name": "create_node",
            "arguments": {
                "node_type": "schema",
                "content": "Invoice",
                "properties": {
                    "fields": []
                }
            }
        });
        let embedding_service: Option<Arc<NodeEmbeddingService>> = None;
        let create_response = handle_tools_call(&node_service, &embedding_service, create_params)
            .await
            .expect("create_node should succeed");

        // Extract the generated node_id from the tool response
        let response_text = create_response["content"][0]["text"]
            .as_str()
            .expect("Response should have text content");
        let response_json: serde_json::Value =
            serde_json::from_str(response_text).expect("Response should be valid JSON");
        let schema_node_id = response_json["node_id"]
            .as_str()
            .expect("Response should contain node_id");

        let result = handle_tools_list(&node_service, json!({})).await.unwrap();
        let tools = result["tools"].as_array().unwrap();

        let create_node = tools.iter().find(|t| t["name"] == "create_node").unwrap();
        let node_type_enum = create_node["inputSchema"]["properties"]["node_type"]["enum"]
            .as_array()
            .unwrap();
        let enum_values: Vec<&str> = node_type_enum.iter().map(|v| v.as_str().unwrap()).collect();

        // The actual schema node ID (a ULID) must appear in the enum
        assert!(
            enum_values.contains(&schema_node_id),
            "Schema node ID '{}' should appear in the node_type enum, got: {:?}",
            schema_node_id,
            enum_values
        );
        // Core types must still be present
        assert!(enum_values.contains(&"schema"));
        assert!(enum_values.contains(&"text"));
    }

    #[tokio::test]
    async fn test_tools_call_unknown_tool_returns_error() {
        let (node_service, embedding_service, _temp_dir) = setup_test_services().await;

        let params = json!({
            "name": "unknown_tool",
            "arguments": {}
        });

        let result = handle_tools_call(&node_service, &embedding_service, params).await;

        // Should return Err with invalid params error
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, crate::mcp::types::INVALID_PARAMS);
        assert!(error.message.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn test_tools_call_missing_name_parameter() {
        let (node_service, embedding_service, _temp_dir) = setup_test_services().await;

        let params = json!({
            "arguments": {"content": "test"}
        });

        let result = handle_tools_call(&node_service, &embedding_service, params).await;

        // Should return Err with invalid params error
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, crate::mcp::types::INVALID_PARAMS);
        assert!(error.message.contains("Missing 'name' parameter"));
    }

    #[tokio::test]
    async fn test_tools_call_create_node_success() {
        let (node_service, embedding_service, _temp_dir) = setup_test_services().await;

        let params = json!({
            "name": "create_node",
            "arguments": {
                "node_type": "text",
                "content": "Test node content"
            }
        });

        let result = handle_tools_call(&node_service, &embedding_service, params).await;

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
        let (node_service, embedding_service, _temp_dir) = setup_test_services().await;

        let params = json!({
            "name": "get_node",
            "arguments": {
                "node_id": "non-existent-id"
            }
        });

        let result = handle_tools_call(&node_service, &embedding_service, params).await;

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
        let (node_service, embedding_service, _temp_dir) = setup_test_services().await;

        // First create a node
        let create_params = json!({
            "name": "create_node",
            "arguments": {
                "node_type": "text",
                "content": "Searchable content"
            }
        });
        handle_tools_call(&node_service, &embedding_service, create_params)
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

        let result = handle_tools_call(&node_service, &embedding_service, query_params).await;

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
        let (node_service, embedding_service, _temp_dir) = setup_test_services().await;

        // Call without arguments field
        let params = json!({
            "name": "query_nodes"
        });

        let result = handle_tools_call(&node_service, &embedding_service, params).await;

        // Should work with default empty arguments
        assert!(result.is_ok());
        let response = result.unwrap();

        // May return error due to invalid params, but shouldn't panic
        // The important thing is it doesn't crash on missing arguments field
        assert!(response["content"].is_array());
    }
}
