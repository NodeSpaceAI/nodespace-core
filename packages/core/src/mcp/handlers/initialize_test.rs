//! Tests for MCP Initialize Handler

use super::*;
use serde_json::json;

#[test]
fn test_initialize_success() {
    let params = json!({
        "protocolVersion": "2024-11-05",
        "clientInfo": {
            "name": "test-client",
            "version": "1.0.0"
        }
    });

    let result = handle_initialize(params).unwrap();

    // Verify protocol version
    assert_eq!(result["protocolVersion"], "2024-11-05");

    // Verify server info
    assert_eq!(result["serverInfo"]["name"], "nodespace-mcp-server");
    assert!(result["serverInfo"]["version"].is_string());

    // Verify capabilities structure (per MCP 2024-11-05 spec)
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
    assert!(result["capabilities"]["resources"].is_object());
    assert!(result["capabilities"]["prompts"].is_object());
}

#[test]
fn test_initialize_wrong_version() {
    let params = json!({
        "protocolVersion": "1999-01-01",  // Unsupported version
        "clientInfo": {
            "name": "test-client"
        }
    });

    let result = handle_initialize(params);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert_eq!(err.code, crate::mcp::types::INVALID_REQUEST);
    assert!(err.message.contains("Unsupported protocol version"));
    assert!(err.message.contains("1999-01-01"));
    assert!(err.message.contains("2024-11-05"));
}

#[test]
fn test_initialize_missing_version() {
    let params = json!({
        "clientInfo": {
            "name": "test-client"
        }
    });

    let result = handle_initialize(params);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert_eq!(err.code, crate::mcp::types::INVALID_PARAMS);
    assert!(err.message.contains("Missing protocolVersion"));
}

#[test]
fn test_initialize_empty_params() {
    let params = json!({});

    let result = handle_initialize(params);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert_eq!(err.code, crate::mcp::types::INVALID_PARAMS);
}

#[test]
fn test_all_expected_tools_present() {
    // Use tools/list to get tool schemas (per MCP spec, tools are discovered via tools/list)
    let result = crate::mcp::handlers::tools::handle_tools_list(json!({})).unwrap();
    let tools = result["tools"].as_array().unwrap();

    // Verify all expected methods are present
    let expected_tools = [
        // Core CRUD
        "create_node",
        "get_node",
        "update_node",
        "delete_node",
        "query_nodes",
        // Hierarchy operations
        "get_children",
        "get_child_at_index",
        "insert_child_at_index",
        "move_child_to_index",
        "get_node_tree",
        // Markdown
        "create_nodes_from_markdown",
        "get_markdown_from_node_id",
        // Batch operations
        "get_nodes_batch",
        "update_nodes_batch",
        "update_container_from_markdown",
        // Search
        "search_containers",
        // Schema creation - Issue #703: renamed and extended with relationships
        "create_schema",
        // Relationship CRUD - Issue #703
        "create_relationship",
        "delete_relationship",
        "get_related_nodes",
        // NLP Discovery API - Issue #703
        "get_relationship_graph",
        "get_inbound_relationships",
        "get_all_schemas",
    ];

    for expected_tool in &expected_tools {
        assert!(
            tools.iter().any(|t| t["name"] == *expected_tool),
            "Missing tool: {}",
            expected_tool
        );
    }

    // Verify we have exactly 25 tools
    // (19 previous + 6 relationship/NLP discovery tools from Issue #703)
    assert_eq!(tools.len(), 25, "Expected exactly 25 tools");
}

#[test]
fn test_all_schemas_have_required_fields() {
    let result = crate::mcp::handlers::tools::handle_tools_list(json!({})).unwrap();
    let tools = result["tools"].as_array().unwrap();

    assert!(
        tools.len() >= 6,
        "Should have at least 6 tool schemas, found {}",
        tools.len()
    );

    for schema in tools {
        let name = schema["name"].as_str().unwrap_or("<missing>");

        // Every schema must have these fields
        assert!(
            schema["name"].is_string(),
            "Schema missing name field: {:?}",
            schema
        );
        assert!(
            schema["description"].is_string(),
            "Schema '{}' missing description",
            name
        );
        assert!(
            schema["inputSchema"].is_object(),
            "Schema '{}' missing inputSchema",
            name
        );

        // InputSchema must have proper structure
        let input_schema = &schema["inputSchema"];
        assert_eq!(
            input_schema["type"], "object",
            "Schema '{}': inputSchema must be type 'object'",
            name
        );
        assert!(
            input_schema["properties"].is_object(),
            "Schema '{}': inputSchema must have properties",
            name
        );
    }
}

#[test]
fn test_create_node_schema_has_required_fields() {
    let result = crate::mcp::handlers::tools::handle_tools_list(json!({})).unwrap();
    let tools = result["tools"].as_array().unwrap();

    let create_node = tools
        .iter()
        .find(|t| t["name"] == "create_node")
        .expect("create_node schema not found");

    let input_schema = &create_node["inputSchema"];
    let required = input_schema["required"].as_array().unwrap();

    // Verify required fields
    assert!(required.contains(&json!("node_type")));
    assert!(required.contains(&json!("content")));
    assert_eq!(
        required.len(),
        2,
        "create_node should have exactly 2 required fields"
    );

    // Verify node_type has enum constraint
    let node_type = &input_schema["properties"]["node_type"];
    assert!(node_type["enum"].is_array());
    let enum_values = node_type["enum"].as_array().unwrap();
    assert!(enum_values.len() >= 7, "Should have at least 7 node types");

    // Verify some expected node types
    assert!(enum_values.contains(&json!("text")));
    assert!(enum_values.contains(&json!("task")));
    assert!(enum_values.contains(&json!("header")));
}

#[test]
fn test_get_node_schema_structure() {
    let result = crate::mcp::handlers::tools::handle_tools_list(json!({})).unwrap();
    let tools = result["tools"].as_array().unwrap();

    let get_node = tools
        .iter()
        .find(|t| t["name"] == "get_node")
        .expect("get_node schema not found");

    assert_eq!(get_node["description"], "Retrieve a single node by ID");

    let input_schema = &get_node["inputSchema"];
    let required = input_schema["required"].as_array().unwrap();

    assert!(required.contains(&json!("node_id")));
    assert_eq!(required.len(), 1);

    let properties = &input_schema["properties"];
    assert!(properties["node_id"].is_object());
    assert_eq!(properties["node_id"]["type"], "string");
}

#[test]
fn test_update_node_schema_structure() {
    let result = crate::mcp::handlers::tools::handle_tools_list(json!({})).unwrap();
    let tools = result["tools"].as_array().unwrap();

    let update_node = tools
        .iter()
        .find(|t| t["name"] == "update_node")
        .expect("update_node schema not found");

    let input_schema = &update_node["inputSchema"];
    let required = input_schema["required"].as_array().unwrap();

    // Only node_id is required
    assert!(required.contains(&json!("node_id")));
    assert_eq!(required.len(), 1);

    // But schema includes optional content and properties
    let properties = &input_schema["properties"];
    assert!(properties["node_id"].is_object());
    assert!(properties["content"].is_object());
    assert!(properties["properties"].is_object());
}

#[test]
fn test_markdown_import_schema_structure() {
    let result = crate::mcp::handlers::tools::handle_tools_list(json!({})).unwrap();
    let tools = result["tools"].as_array().unwrap();

    let markdown_import = tools
        .iter()
        .find(|t| t["name"] == "create_nodes_from_markdown")
        .expect("create_nodes_from_markdown schema not found");

    // Verify description starts correctly (we added warnings, so check the beginning)
    let description = markdown_import["description"].as_str().unwrap();
    assert!(
        description.starts_with("Parse markdown and create hierarchical nodes"),
        "Description should start with 'Parse markdown and create hierarchical nodes'"
    );

    let input_schema = &markdown_import["inputSchema"];
    let required = input_schema["required"].as_array().unwrap();

    assert!(required.contains(&json!("markdown_content")));
    assert!(required.contains(&json!("container_title")));
    assert_eq!(required.len(), 2);
}

// Integration test for full MCP initialization handshake flow
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::mcp::types::INVALID_REQUEST;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// Simulates ServerState for integration testing
    struct TestServerState {
        initialized: Arc<AtomicBool>,
    }

    impl TestServerState {
        fn new() -> Self {
            Self {
                initialized: Arc::new(AtomicBool::new(false)),
            }
        }

        fn is_initialized(&self) -> bool {
            self.initialized.load(Ordering::SeqCst)
        }

        fn mark_initialized(&self) {
            self.initialized.store(true, Ordering::SeqCst);
        }

        fn check_operation_allowed(&self, method: &str) -> bool {
            // Allow initialize and ping before initialization
            method == "initialize" || method == "ping" || self.is_initialized()
        }
    }

    #[test]
    fn test_full_initialization_handshake() {
        let state = TestServerState::new();

        // Step 1: Verify initial state - not initialized
        assert!(!state.is_initialized(), "State should start uninitialized");

        // Step 2: Operations should be blocked before initialization
        assert!(
            !state.check_operation_allowed("create_node"),
            "create_node should be blocked before initialization"
        );
        assert!(
            !state.check_operation_allowed("get_node"),
            "get_node should be blocked before initialization"
        );

        // Step 3: Initialize and ping should be allowed before initialization
        assert!(
            state.check_operation_allowed("initialize"),
            "initialize should be allowed before initialization"
        );
        assert!(
            state.check_operation_allowed("ping"),
            "ping should be allowed before initialization"
        );

        // Step 4: Send initialize request and verify response
        let init_params = json!({
            "protocolVersion": "2024-11-05",
            "clientInfo": {
                "name": "test-integration-client",
                "version": "1.0.0"
            }
        });

        let init_result = handle_initialize(init_params).unwrap();

        // Verify initialize response structure
        assert_eq!(init_result["protocolVersion"], "2024-11-05");
        assert_eq!(init_result["serverInfo"]["name"], "nodespace-mcp-server");
        assert!(init_result["capabilities"]["tools"].is_object());
        assert_eq!(init_result["capabilities"]["tools"]["listChanged"], false);

        // Step 5: Simulate receiving "initialized" notification
        // (In real server, this would come from client)
        state.mark_initialized();

        // Step 6: Verify state is now initialized
        assert!(
            state.is_initialized(),
            "State should be initialized after notification"
        );

        // Step 7: Verify operations are now allowed
        assert!(
            state.check_operation_allowed("create_node"),
            "create_node should be allowed after initialization"
        );
        assert!(
            state.check_operation_allowed("get_node"),
            "get_node should be allowed after initialization"
        );
        assert!(
            state.check_operation_allowed("update_node"),
            "update_node should be allowed after initialization"
        );
        assert!(
            state.check_operation_allowed("delete_node"),
            "delete_node should be allowed after initialization"
        );
        assert!(
            state.check_operation_allowed("query_nodes"),
            "query_nodes should be allowed after initialization"
        );

        // Step 8: Verify initialize and ping still allowed
        assert!(
            state.check_operation_allowed("initialize"),
            "initialize should still be allowed (idempotent)"
        );
        assert!(
            state.check_operation_allowed("ping"),
            "ping should still be allowed"
        );
    }

    #[test]
    fn test_operation_rejected_before_initialization() {
        let state = TestServerState::new();

        // Simulate what handle_request does: check state before processing
        let blocked_methods = [
            "create_node",
            "get_node",
            "update_node",
            "delete_node",
            "query_nodes",
            "create_nodes_from_markdown",
        ];

        for method in &blocked_methods {
            assert!(
                !state.check_operation_allowed(method),
                "Method '{}' should be blocked before initialization",
                method
            );
        }
    }

    #[test]
    fn test_initialize_then_wrong_version() {
        let state = TestServerState::new();

        // First initialize with correct version
        let params = json!({
            "protocolVersion": "2024-11-05",
            "clientInfo": {"name": "test"}
        });
        let result = handle_initialize(params);
        assert!(result.is_ok(), "First initialize should succeed");

        state.mark_initialized();

        // Try to initialize again with wrong version (should still error)
        let params_wrong = json!({
            "protocolVersion": "1999-01-01",
            "clientInfo": {"name": "test"}
        });
        let result_wrong = handle_initialize(params_wrong);
        assert!(
            result_wrong.is_err(),
            "Initialize with wrong version should fail even after initialization"
        );

        let err = result_wrong.unwrap_err();
        assert_eq!(err.code, INVALID_REQUEST);
        assert!(err.message.contains("Unsupported protocol version"));
    }

    #[test]
    fn test_initialize_idempotent() {
        let state = TestServerState::new();

        let params = json!({
            "protocolVersion": "2024-11-05",
            "clientInfo": {"name": "test"}
        });

        // First initialize
        let result1 = handle_initialize(params.clone()).unwrap();
        state.mark_initialized();

        // Second initialize (should succeed - idempotent)
        let result2 = handle_initialize(params).unwrap();

        // Both should return same structure
        assert_eq!(result1["protocolVersion"], result2["protocolVersion"]);
        assert_eq!(result1["serverInfo"]["name"], result2["serverInfo"]["name"]);
        // Tools capability is now an object per MCP spec
        assert_eq!(
            result1["capabilities"]["tools"]["listChanged"],
            result2["capabilities"]["tools"]["listChanged"]
        );
    }
}
