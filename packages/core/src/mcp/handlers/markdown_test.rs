//! Tests for MCP markdown import handler
//!
//! Tests markdown parsing, node creation, and hierarchy management.

#[cfg(test)]
mod tests {
    use crate::db::DatabaseService;
    use crate::mcp::handlers::markdown::handle_create_nodes_from_markdown;
    use crate::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn setup_test_service() -> (Arc<NodeService>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = DatabaseService::new(db_path).await.unwrap();
        let service = NodeService::new(db).unwrap();
        (Arc::new(service), temp_dir)
    }

    #[tokio::test]
    async fn test_simple_markdown_with_headings() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = r#"# Main Title
## Subtitle
Some content under subtitle."#;

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Test Document"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);
        assert_eq!(result["nodes_created"], 4); // container + 2 headers + 1 text

        // Verify node_ids array
        let node_ids = result["node_ids"].as_array().unwrap();
        assert_eq!(node_ids.len(), 4);
    }

    #[tokio::test]
    async fn test_heading_hierarchy() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = r#"# H1
## H2 under H1
### H3 under H2
## Another H2
Text under second H2"#;

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Hierarchy Test"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);
        assert_eq!(result["nodes_created"], 6); // container + 4 headers + 1 text

        // Get all created nodes
        let container_id = result["container_node_id"].as_str().unwrap();
        let container = service.get_node(container_id).await.unwrap().unwrap();

        assert_eq!(container.content, "Hierarchy Test");
        assert_eq!(container.node_type, "text");
        assert!(container.is_root()); // container_node_id should be None
    }

    #[tokio::test]
    async fn test_content_preservation() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = r#"## Heading with hashtags
- [ ] Task item
```rust
code block
```
> Quote block"#;

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Content Test"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);

        // Get all nodes and verify content format
        let node_ids = result["node_ids"].as_array().unwrap();

        // Skip container (first node)
        for node_id in node_ids.iter().skip(1) {
            let node = service
                .get_node(node_id.as_str().unwrap())
                .await
                .unwrap()
                .unwrap();

            match node.node_type.as_str() {
                "header" => {
                    // Should have ## prefix
                    assert!(node.content.starts_with("##"));
                }
                "task" => {
                    // Should have - [ ] format
                    assert!(node.content.starts_with("- [ ]"));
                }
                "code-block" => {
                    // Should have ``` fence
                    assert!(node.content.starts_with("```"));
                    assert!(node.content.ends_with("```"));
                }
                "quote-block" => {
                    // Should have > prefix
                    assert!(node.content.starts_with(">"));
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn test_nested_lists() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = r#"- Item 1
  - Nested item 1.1
  - Nested item 1.2
- Item 2"#;

        let params = json!({
            "markdown_content": markdown,
            "container_title": "List Test"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);
        // container + 4 list items
        assert_eq!(result["nodes_created"], 5);
    }

    #[tokio::test]
    async fn test_task_list_with_checked_items() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = r#"- [ ] Unchecked task
- [x] Checked task
- [ ] Another unchecked"#;

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Task List"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);
        assert_eq!(result["nodes_created"], 4); // container + 3 tasks

        // Verify task content (checkboxes in content, no properties)
        let node_ids = result["node_ids"].as_array().unwrap();

        let mut checked_count = 0;
        let mut unchecked_count = 0;

        for node_id in node_ids.iter().skip(1) {
            let node = service
                .get_node(node_id.as_str().unwrap())
                .await
                .unwrap()
                .unwrap();

            if node.node_type == "task" {
                if node.content.contains("[x]") {
                    checked_count += 1;
                } else if node.content.contains("[ ]") {
                    unchecked_count += 1;
                }
            }
        }

        assert_eq!(checked_count, 1);
        assert_eq!(unchecked_count, 2);
    }

    #[tokio::test]
    async fn test_code_block_with_language() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = r#"```rust
fn main() {
    println!("Hello");
}
```"#;

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Code Test"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);
        assert_eq!(result["nodes_created"], 2); // container + code block

        // Verify code block node
        let node_ids = result["node_ids"].as_array().unwrap();
        let code_node = service
            .get_node(node_ids[1].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(code_node.node_type, "code-block");
        assert!(code_node.content.starts_with("```rust"));
        assert!(code_node.content.contains("fn main()"));
        // Language is in content, not properties
    }

    #[tokio::test]
    async fn test_code_block_without_language() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = r#"```
plain code
```"#;

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Plain Code Test"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);

        // Verify code block content (no language fence)
        let node_ids = result["node_ids"].as_array().unwrap();
        let code_node = service
            .get_node(node_ids[1].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(code_node.node_type, "code-block");
        assert!(code_node.content.starts_with("```\n"));
    }

    #[tokio::test]
    async fn test_mixed_content() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = r#"# Project Plan

## Goals
- [ ] Complete phase 1
- [x] Setup infrastructure

## Technical Details
```python
def process():
    pass
```

> Important note about the project

Regular paragraph text."#;

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Mixed Content"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);
        // container + 2 headers + 2 tasks + code block + quote + text
        assert_eq!(result["nodes_created"], 8);
    }

    #[tokio::test]
    async fn test_empty_markdown() {
        let (service, _temp_dir) = setup_test_service().await;

        let params = json!({
            "markdown_content": "",
            "container_title": "Empty Document"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);
        assert_eq!(result["nodes_created"], 1); // Just container
    }

    #[tokio::test]
    async fn test_container_node_structure() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = "# Test";

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Container Test"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        let container_id = result["container_node_id"].as_str().unwrap();
        let container = service.get_node(container_id).await.unwrap().unwrap();

        // Container should be a root node
        assert!(container.is_root());
        assert_eq!(container.node_type, "text");
        assert_eq!(container.content, "Container Test");
        assert!(container.parent_id.is_none());
        assert!(container.container_node_id.is_none());
    }

    #[tokio::test]
    async fn test_all_nodes_share_container() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = r#"# Heading
Text content
- List item"#;

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Shared Container"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        let container_id = result["container_node_id"].as_str().unwrap();
        let node_ids = result["node_ids"].as_array().unwrap();

        // Verify all non-container nodes have the same container_node_id
        for node_id in node_ids.iter().skip(1) {
            let node = service
                .get_node(node_id.as_str().unwrap())
                .await
                .unwrap()
                .unwrap();

            assert_eq!(node.container_node_id, Some(container_id.to_string()));
        }
    }

    #[tokio::test]
    async fn test_invalid_params() {
        let (service, _temp_dir) = setup_test_service().await;

        // Missing container_title
        let params = json!({
            "markdown_content": "# Test"
        });

        let result = handle_create_nodes_from_markdown(&service, params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_inline_code_in_text() {
        let (service, _temp_dir) = setup_test_service().await;

        let markdown = "This has `inline code` in it.";

        let params = json!({
            "markdown_content": markdown,
            "container_title": "Inline Code Test"
        });

        let result = handle_create_nodes_from_markdown(&service, params)
            .await
            .unwrap();

        assert_eq!(result["success"], true);

        // Find the text node
        let node_ids = result["node_ids"].as_array().unwrap();
        let text_node = service
            .get_node(node_ids[1].as_str().unwrap())
            .await
            .unwrap()
            .unwrap();

        // Inline code should be preserved with backticks
        assert!(text_node.content.contains("`inline code`"));
    }
}
