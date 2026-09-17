//! Integration Tests for QueryService
//!
//! These tests validate query execution against a real SQLite database,
//! testing SQL generation and result retrieval for all filter types.
//! All queries use the unified node table with JSON properties.

#[cfg(test)]
mod tests {
    use crate::db::SqliteStore;
    use crate::models::{Node, TaskPriority};
    use crate::services::node_service::{CreateNodeParams, NodeService};
    use crate::services::query_service::{
        FilterOperator, FilterType, QueryDefinition, QueryFilter, QueryService, RelationshipType,
        SortConfig, SortDirection,
    };
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper to create test services with SQLite database
    async fn create_test_services() -> (Arc<QueryService>, Arc<NodeService>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let mut store = Arc::new(SqliteStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
        let query_service = Arc::new(QueryService::new(store.clone()));

        (query_service, node_service, temp_dir)
    }

    #[tokio::test]
    async fn test_simple_type_filter() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create test nodes with different types
        let task1 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task 1".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };
        let task2 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task 2".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "done"}}),
            lifecycle_status: None,
        };
        let text1 = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Text node".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(task1).await.unwrap();
        node_service.create_node_with_parent(task2).await.unwrap();
        node_service.create_node_with_parent(text1).await.unwrap();

        // Query for task nodes only
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(results.len(), 2, "Should return only task nodes");
        assert!(
            results.iter().all(|n| n.node_type == "task"),
            "All results should be task type"
        );
    }

    #[tokio::test]
    async fn test_property_filter_equals() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create task nodes with different statuses (namespaced properties)
        let task1 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task 1".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };
        let task2 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task 2".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "done"}}),
            lifecycle_status: None,
        };
        let task3 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task 3".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(task1).await.unwrap();
        node_service.create_node_with_parent(task2).await.unwrap();
        node_service.create_node_with_parent(task3).await.unwrap();

        // Query for open tasks only
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::Equals,
                property: Some("status".to_string()),
                value: Some(json!("open")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(results.len(), 2, "Should return only open tasks");
        assert!(
            results
                .iter()
                .all(|n| n.properties["task"]["status"] == "open"),
            "All results should have open status"
        );
    }

    /// A scalar `in` value fails loudly rather than matching nothing.
    ///
    /// #2182 was reported as a silent zero-result — the failure mode that is
    /// indistinguishable from a genuinely empty search and therefore invisible
    /// to the user. It is not: `build_filter_condition` rejects a non-array `in`
    /// value outright, so the tool call errors and the model has to recover.
    /// Pinned as a test because the distinction decides how much machinery a
    /// malformed `in` filter justifies, and reading it back off the code is
    /// exactly the step that got skipped when the issue was written.
    ///
    /// The agent-side repair (`repair_scalar_in_operator_values`) exists so this
    /// error is not reached in the first place; this asserts what it is
    /// protecting against, from the other side of the boundary.
    #[tokio::test]
    async fn test_property_filter_in_rejects_a_scalar_value() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::In,
                property: Some("status".to_string()),
                // What the model emits today: the members comma-joined into one
                // string instead of an array.
                value: Some(json!("open,in_progress")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let err = query_service
            .execute(&query)
            .await
            .expect_err("a scalar `in` value must error, not quietly match nothing");
        assert!(
            err.to_string().contains("In requires array value"),
            "the error must name the actual defect; got: {err}"
        );
    }

    #[tokio::test]
    async fn test_property_filter_in() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create task nodes with different statuses
        for (i, status) in ["open", "in_progress", "done", "cancelled"]
            .iter()
            .enumerate()
        {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: format!("Task {}", i + 1),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": status}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Query for open or in_progress tasks
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::In,
                property: Some("status".to_string()),
                value: Some(json!(["open", "in_progress"])),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(results.len(), 2, "Should return open and in_progress tasks");
        assert!(
            results.iter().all(|n| {
                let status = n.properties["task"]["status"].as_str().unwrap();
                status == "open" || status == "in_progress"
            }),
            "All results should be open or in_progress"
        );
    }

    #[tokio::test]
    async fn test_content_filter_contains_case_sensitive() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create nodes with different content
        let nodes = vec![
            ("Task with Important keyword", true),
            ("Task with important keyword", false),
            ("Task without the keyword", false),
            ("Task with IMPORTANT in caps", false),
        ];

        for (content, _) in &nodes {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": "open"}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Query for "Important" (case-sensitive)
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Content,
                operator: FilterOperator::Contains,
                property: None,
                value: Some(json!("Important")),
                case_sensitive: Some(true),
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(
            results.len(),
            1,
            "Should return only nodes with exact case match"
        );
        assert!(
            results[0].content.contains("Important"),
            "Result should contain 'Important' with capital I"
        );
    }

    #[tokio::test]
    async fn test_content_filter_contains_case_insensitive() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create nodes with different content
        let nodes = vec![
            "Task with Important keyword",
            "Task with important keyword",
            "Task without the keyword",
            "Task with IMPORTANT in caps",
        ];

        for content in &nodes {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": "open"}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Query for "important" (case-insensitive)
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Content,
                operator: FilterOperator::Contains,
                property: None,
                value: Some(json!("important")),
                case_sensitive: Some(false),
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(
            results.len(),
            3,
            "Should return all nodes with case-insensitive match"
        );
    }

    #[tokio::test]
    async fn test_relationship_filter_children() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create parent node
        let parent = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Parent".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };
        let parent_id = node_service.create_node_with_parent(parent).await.unwrap();

        // Create child nodes
        let child1 = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Child 1".to_string(),
            parent_id: Some(parent_id.clone()),
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };
        let child2 = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Child 2".to_string(),
            parent_id: Some(parent_id.clone()),
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(child1).await.unwrap();
        node_service.create_node_with_parent(child2).await.unwrap();

        // Query for children of parent
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Relationship,
                operator: FilterOperator::Equals,
                property: None,
                value: None,
                case_sensitive: None,
                relationship_type: Some(RelationshipType::Children),
                node_id: Some(parent_id.clone()),
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(results.len(), 2, "Should return both children");
        assert!(
            results.iter().all(|n| n.content.starts_with("Child")),
            "All results should be child nodes"
        );
    }

    #[tokio::test]
    async fn test_sorting() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create tasks and sort by content (alphabetically)
        // This avoids timing issues and uses a field that's always present
        let tasks = vec!["Apple", "Banana", "Cherry"];
        for content in tasks {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": "open"}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Query with descending sort by content
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: Some(vec![SortConfig {
                field: "content".to_string(),
                direction: SortDirection::Descending,
            }]),
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(results.len(), 3, "Should return all tasks");
        // Verify descending alphabetical order
        assert!(results[0].content == "Cherry", "First should be 'Cherry'");
        assert!(results[1].content == "Banana", "Second should be 'Banana'");
        assert!(results[2].content == "Apple", "Last should be 'Apple'");
    }

    #[tokio::test]
    async fn test_limit() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create 10 task nodes
        for i in 1..=10 {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: format!("Task {}", i),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": "open"}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Query with limit of 5
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: None,
            limit: Some(5),
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(results.len(), 5, "Should return only 5 tasks");
    }

    #[tokio::test]
    async fn test_combined_filters() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create various task nodes (namespaced properties)
        let tasks = vec![
            ("High priority open task", "open", "high"),
            ("Low priority open task", "open", "low"),
            ("High priority done task", "done", "high"),
            ("Medium priority open task", "open", "medium"),
        ];

        for (content, status, priority) in &tasks {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({
                    "task": {
                        "status": status,
                        "priority": priority
                    }
                }),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Query for open tasks with high priority
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![
                QueryFilter {
                    filter_type: FilterType::Property,
                    operator: FilterOperator::Equals,
                    property: Some("status".to_string()),
                    value: Some(json!("open")),
                    case_sensitive: None,
                    relationship_type: None,
                    node_id: None,
                },
                QueryFilter {
                    filter_type: FilterType::Property,
                    operator: FilterOperator::Equals,
                    property: Some("priority".to_string()),
                    value: Some(json!("high")),
                    case_sensitive: None,
                    relationship_type: None,
                    node_id: None,
                },
            ],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(results.len(), 1, "Should return only one matching task");
        assert_eq!(
            results[0].content, "High priority open task",
            "Should be the high priority open task"
        );
        assert_eq!(results[0].properties["task"]["status"], "open");
        assert_eq!(results[0].properties["task"]["priority"], "high");
    }

    #[tokio::test]
    async fn test_metadata_filter() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create nodes with different types
        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task node".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };
        let text = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Text node".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(task).await.unwrap();
        node_service.create_node_with_parent(text).await.unwrap();

        // Query using metadata filter for node_type
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Metadata,
                operator: FilterOperator::Equals,
                property: Some("node_type".to_string()),
                value: Some(json!("task")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(results.len(), 1, "Should return only task node");
        assert_eq!(results[0].node_type, "task");
    }

    #[tokio::test]
    async fn test_empty_results() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        // Query for type with no matching records (task table exists but is empty initially)
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::Equals,
                property: Some("status".to_string()),
                value: Some(json!("nonexistent_status")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();

        assert_eq!(results.len(), 0, "Should return empty results");
    }

    #[tokio::test]
    async fn test_content_filter_equals() {
        let (query_service, node_service, _temp) = create_test_services().await;

        let task1 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Exact Match".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };
        let task2 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Different".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(task1).await.unwrap();
        node_service.create_node_with_parent(task2).await.unwrap();

        // Query for exact content match
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Content,
                operator: FilterOperator::Equals,
                property: None,
                value: Some(json!("Exact Match")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 1, "Should return only exact match");
        assert_eq!(results[0].content, "Exact Match");
    }

    #[tokio::test]
    async fn test_sorting_ascending() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create tasks with different content
        for content in &["Zebra", "Apple", "Mango"] {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": "open"}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Sort ascending by content
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: Some(vec![SortConfig {
                field: "content".to_string(),
                direction: SortDirection::Ascending,
            }]),
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].content, "Apple");
        assert_eq!(results[1].content, "Mango");
        assert_eq!(results[2].content, "Zebra");
    }

    #[tokio::test]
    async fn test_query_any_node_type() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // create_node_with_parent now rejects a node_type with no registered
        // behavior and no schema node, so a custom type needs a schema seeded
        // first — same as an agent calling create_schema before create_node.
        let schema = crate::models::Node::new_with_id(
            "custom_type".to_string(),
            "schema".to_string(),
            "Custom Type".to_string(),
            json!({ "fields": [] }),
        );
        node_service
            .store()
            .create_node(schema, None, None)
            .await
            .unwrap();

        // Create a node with custom type
        let custom_node = CreateNodeParams {
            id: None,
            node_type: "custom_type".to_string(),
            content: "Custom node".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"custom_field": "value"}),
            lifecycle_status: None,
        };
        node_service
            .create_node_with_parent(custom_node)
            .await
            .unwrap();

        // Query for the custom type - should work with unified node table
        let query = QueryDefinition {
            target_type: "custom_type".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        };

        let result = query_service.execute(&query).await;
        assert!(result.is_ok(), "Should succeed for any node type");
        let nodes = result.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_type, "custom_type");
    }

    #[tokio::test]
    async fn test_relationship_filter_parent() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create a child with a known parent
        let parent = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Parent".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };
        let parent_id = node_service.create_node_with_parent(parent).await.unwrap();

        let child = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Child".to_string(),
            parent_id: Some(parent_id.clone()),
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };
        let child_id = node_service.create_node_with_parent(child).await.unwrap();

        // Query for the parent of the child node
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Relationship,
                operator: FilterOperator::Equals,
                property: None,
                value: None,
                case_sensitive: None,
                relationship_type: Some(RelationshipType::Parent),
                node_id: Some(child_id.clone()),
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 1, "Should find the parent");
        assert_eq!(results[0].content, "Parent");
    }

    // =========================================================================
    // Wildcard Query Tests (target_type = "*")
    // =========================================================================

    #[tokio::test]
    async fn test_wildcard_query_with_metadata_filter() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create nodes with different types
        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task content".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };
        let text = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Text content".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(task).await.unwrap();
        node_service.create_node_with_parent(text).await.unwrap();

        // Wildcard query with metadata filter for node_type
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Metadata,
                operator: FilterOperator::Equals,
                property: Some("node_type".to_string()),
                value: Some(json!("task")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].node_type, "task");
    }

    #[tokio::test]
    async fn test_wildcard_query_content_filter() {
        let (query_service, node_service, _temp) = create_test_services().await;

        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Important meeting".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };
        let text = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Random text".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(task).await.unwrap();
        node_service.create_node_with_parent(text).await.unwrap();

        // Wildcard query with content filter (case-sensitive)
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Content,
                operator: FilterOperator::Contains,
                property: None,
                value: Some(json!("Important")),
                case_sensitive: Some(true),
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Important"));
    }

    #[tokio::test]
    async fn test_wildcard_query_with_sorting_and_limit() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create multiple tasks
        for content in &["Zebra", "Alpha", "Beta"] {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": "open"}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Wildcard query with sorting (ascending) and limit
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Metadata,
                operator: FilterOperator::Equals,
                property: Some("node_type".to_string()),
                value: Some(json!("task")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: Some(vec![SortConfig {
                field: "content".to_string(),
                direction: SortDirection::Ascending,
            }]),
            limit: Some(2),
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "Alpha");
        assert_eq!(results[1].content, "Beta");
    }

    // =========================================================================
    // Error Condition Tests
    // =========================================================================

    #[tokio::test]
    async fn test_metadata_filter_invalid_field() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        // Try to use invalid metadata field
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Metadata,
                operator: FilterOperator::Equals,
                property: Some("invalid_field".to_string()),
                value: Some(json!("value")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let result = query_service.execute(&query).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid metadata field"));
    }

    #[tokio::test]
    async fn test_relationship_filter_missing_node_id() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        // Missing node_id in relationship filter
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Relationship,
                operator: FilterOperator::Equals,
                property: None,
                value: None,
                case_sensitive: None,
                relationship_type: Some(RelationshipType::Children),
                node_id: None, // Missing!
            }],
            sorting: None,
            limit: None,
        };

        let result = query_service.execute(&query).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Missing nodeId"));
    }

    #[tokio::test]
    async fn test_relationship_filter_missing_type() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        // Missing relationship_type
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Relationship,
                operator: FilterOperator::Equals,
                property: None,
                value: None,
                case_sensitive: None,
                relationship_type: None, // Missing!
                node_id: Some("test-id".to_string()),
            }],
            sorting: None,
            limit: None,
        };

        let result = query_service.execute(&query).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Missing relationshipType"));
    }

    #[tokio::test]
    async fn test_property_filter_missing_property() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        // Missing property field in property filter
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::Equals,
                property: None, // Missing!
                value: Some(json!("value")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let result = query_service.execute(&query).await;
        assert!(result.is_err());
    }

    // =========================================================================
    // Sorting on Different Fields
    // =========================================================================

    #[tokio::test]
    async fn test_sort_by_created_at() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create tasks with a small delay to have different timestamps
        for content in &["First", "Second", "Third"] {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": "open"}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Sort by created_at descending (most recent first)
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: Some(vec![SortConfig {
                field: "created_at".to_string(),
                direction: SortDirection::Descending,
            }]),
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 3);
        // Most recent should be first (Third was created last)
        assert_eq!(results[0].content, "Third");
    }

    #[tokio::test]
    async fn test_sort_by_node_type() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create nodes with different types
        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };
        let text = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Text".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(task).await.unwrap();
        node_service.create_node_with_parent(text).await.unwrap();

        // Wildcard query sorted by node_type ascending
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![],
            sorting: Some(vec![SortConfig {
                field: "node_type".to_string(),
                direction: SortDirection::Ascending,
            }]),
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        // Should be sorted: task comes before text alphabetically
        assert!(results.len() >= 2);
    }

    // =========================================================================
    // Operator Tests (Exists)
    // =========================================================================

    #[tokio::test]
    async fn test_property_filter_exists_status() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create tasks - status is a schema-defined field
        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "With status".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(task).await.unwrap();

        // Query for tasks where status exists (should always be true for tasks)
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::Exists,
                property: Some("status".to_string()),
                value: None,
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "With status");
    }

    // =========================================================================
    // Content Filter Edge Cases
    // =========================================================================

    #[tokio::test]
    async fn test_content_filter_equals_case_insensitive() {
        let (query_service, node_service, _temp) = create_test_services().await;

        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "EXACT Match".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };
        node_service.create_node_with_parent(task).await.unwrap();

        // Exact equals matches case-sensitive by default
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Content,
                operator: FilterOperator::Equals,
                property: None,
                value: Some(json!("EXACT Match")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    // =========================================================================
    // Multiple Filter Combination Tests
    // =========================================================================

    #[tokio::test]
    async fn test_multiple_metadata_and_property_filters() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create tasks with different types and statuses
        let task1 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Open task".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "open"}}),
            lifecycle_status: None,
        };
        let task2 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Done task".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {"status": "done"}}),
            lifecycle_status: None,
        };

        node_service.create_node_with_parent(task1).await.unwrap();
        node_service.create_node_with_parent(task2).await.unwrap();

        // Wildcard query with metadata (node_type) and content filters
        let query = QueryDefinition {
            target_type: "*".to_string(),
            filters: vec![
                QueryFilter {
                    filter_type: FilterType::Metadata,
                    operator: FilterOperator::Equals,
                    property: Some("node_type".to_string()),
                    value: Some(json!("task")),
                    case_sensitive: None,
                    relationship_type: None,
                    node_id: None,
                },
                QueryFilter {
                    filter_type: FilterType::Content,
                    operator: FilterOperator::Contains,
                    property: None,
                    value: Some(json!("Open")),
                    case_sensitive: Some(true),
                    relationship_type: None,
                    node_id: None,
                },
            ],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Open task");
    }

    // =========================================================================
    // Sort with Empty Results
    // =========================================================================

    #[tokio::test]
    async fn test_sort_with_no_results() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        // Query with no matching results and sorting configured
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::Equals,
                property: Some("status".to_string()),
                value: Some(json!("nonexistent")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: Some(vec![SortConfig {
                field: "content".to_string(),
                direction: SortDirection::Ascending,
            }]),
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 0);
    }

    // =========================================================================
    // Sort by Schema Property Tests
    // =========================================================================

    #[tokio::test]
    async fn test_sort_by_status_property() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create tasks with different statuses (status is a schema field)
        let tasks = vec![
            ("Z Task", "open"),
            ("A Task", "done"),
            ("M Task", "in_progress"),
        ];
        for (content, status) in tasks {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": status}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Sort by status property ascending
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: Some(vec![SortConfig {
                field: "status".to_string(),
                direction: SortDirection::Ascending,
            }]),
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 3);
        // Sorted by status alphabetically: done, in_progress, open (namespaced properties)
        assert_eq!(results[0].properties["task"]["status"], "done");
        assert_eq!(results[1].properties["task"]["status"], "in_progress");
        assert_eq!(results[2].properties["task"]["status"], "open");
    }

    #[tokio::test]
    async fn test_sort_by_modified_at() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create tasks with sequential creation
        for content in &["First", "Second", "Third"] {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": "open"}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Sort by modified_at descending (should be same as created_at for new nodes)
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: Some(vec![SortConfig {
                field: "modified_at".to_string(),
                direction: SortDirection::Descending,
            }]),
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 3);
        // Most recently created/modified should be first
        assert_eq!(results[0].content, "Third");
    }

    #[tokio::test]
    async fn test_multiple_sort_fields() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create tasks with same status but different content
        let tasks = vec![
            ("Beta Task", "open"),
            ("Alpha Task", "open"),
            ("Gamma Task", "done"),
        ];
        for (content, status) in tasks {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"status": status}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Sort by status ascending, then content ascending
        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: Some(vec![
                SortConfig {
                    field: "status".to_string(),
                    direction: SortDirection::Ascending,
                },
                SortConfig {
                    field: "content".to_string(),
                    direction: SortDirection::Ascending,
                },
            ]),
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(results.len(), 3);
        // done < open alphabetically, then within same status sort by content
        assert_eq!(results[0].content, "Gamma Task"); // done
        assert_eq!(results[1].content, "Alpha Task"); // open, comes before Beta
        assert_eq!(results[2].content, "Beta Task"); // open
    }

    #[tokio::test]
    async fn test_content_filter_contains_wildcards_treated_as_literals() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Node whose content contains SQL LIKE wildcards and a backslash
        let node = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "50% off sale".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };
        node_service.create_node_with_parent(node).await.unwrap();

        let unrelated = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "full price item".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({}),
            lifecycle_status: None,
        };
        node_service
            .create_node_with_parent(unrelated)
            .await
            .unwrap();

        // Case-insensitive contains with '%' — must be treated as a literal, not a wildcard
        let query = QueryDefinition {
            target_type: "text".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Content,
                operator: FilterOperator::Contains,
                property: None,
                value: Some(serde_json::Value::String("50%".to_string())),
                case_sensitive: Some(false),
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let results = query_service.execute(&query).await.unwrap();
        assert_eq!(
            results.len(),
            1,
            "'%' in search value must match literally, not as a LIKE wildcard"
        );
        assert_eq!(results[0].content, "50% off sale");
    }

    // =========================================================================
    // Priority rank ordering
    //
    // task.priority is a string enum whose alphabetical order is meaningless
    // (`high, highest, low, lowest, medium`), so sorting ranks it instead.
    // Two independent sites implement that rank — the SQL CASE in
    // resolve_order_field and compare_priority_values in Rust — and the Rust
    // pass runs last, so a SQL-only regression would be invisible here unless
    // a LIMIT forces the database's ordering to matter. Hence both a
    // full-result test and a LIMIT test below.
    // =========================================================================

    /// Create one task per priority value, in an order chosen so that neither
    /// insertion order nor alphabetical order matches the expected result.
    async fn create_tasks_with_priorities(node_service: &NodeService, priorities: &[&str]) {
        for priority in priorities {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: format!("Task {priority}"),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({"task": {"priority": priority}}),
                lifecycle_status: None,
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }
    }

    fn priorities_of(results: &[Node]) -> Vec<String> {
        results
            .iter()
            .map(|n| {
                n.properties["task"]["priority"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    fn priority_query(direction: SortDirection, limit: Option<usize>) -> QueryDefinition {
        QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: Some(vec![SortConfig {
                field: "priority".to_string(),
                direction,
            }]),
            limit,
        }
    }

    #[tokio::test]
    async fn test_sort_by_priority_ascending_uses_rank_not_alphabetical() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Deliberately inserted out of order; alphabetical would give
        // high, highest, low, lowest, medium — the bug this guards against.
        create_tasks_with_priorities(
            &node_service,
            &["low", "highest", "medium", "lowest", "high"],
        )
        .await;

        let results = query_service
            .execute(&priority_query(SortDirection::Ascending, None))
            .await
            .unwrap();

        assert_eq!(
            priorities_of(&results),
            ["highest", "high", "medium", "low", "lowest"],
            "ascending priority must follow urgency rank, not string order"
        );
    }

    #[tokio::test]
    async fn test_sort_by_priority_descending_reverses_rank() {
        let (query_service, node_service, _temp) = create_test_services().await;

        create_tasks_with_priorities(
            &node_service,
            &["medium", "lowest", "highest", "high", "low"],
        )
        .await;

        let results = query_service
            .execute(&priority_query(SortDirection::Descending, None))
            .await
            .unwrap();

        assert_eq!(
            priorities_of(&results),
            ["lowest", "low", "medium", "high", "highest"]
        );
    }

    #[tokio::test]
    async fn test_sort_by_priority_places_user_defined_values_last() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // A user-defined priority must be added to the schema's user_values
        // before any node may carry it — enum validation rejects it otherwise.
        crate::schema::handle_update_schema(
            &node_service,
            json!({
                "schema_id": "task",
                "add_field_values": [{
                    "field": "priority",
                    "values": [
                        {"value": "critical", "label": "Critical"},
                        {"value": "blocker", "label": "Blocker"}
                    ]
                }]
            }),
        )
        .await
        .expect("priority is an extensible enum field");

        // Both sort after every core value despite "blocker"/"critical"
        // preceding "low"/"medium" alphabetically, and order lexicographically
        // against each other.
        create_tasks_with_priorities(
            &node_service,
            &["critical", "low", "highest", "blocker", "medium"],
        )
        .await;

        let results = query_service
            .execute(&priority_query(SortDirection::Ascending, None))
            .await
            .unwrap();

        assert_eq!(
            priorities_of(&results),
            ["highest", "medium", "low", "blocker", "critical"],
            "user-defined priorities sort after all core values, \
             lexicographically among themselves"
        );
    }

    #[tokio::test]
    async fn test_sort_by_priority_with_limit_ranks_in_sql() {
        let (query_service, node_service, _temp) = create_test_services().await;

        create_tasks_with_priorities(
            &node_service,
            &["low", "highest", "medium", "lowest", "high"],
        )
        .await;

        // LIMIT applies in SQL, before the in-Rust re-sort, so the database
        // alone decides which rows survive. This fails if the CASE in
        // resolve_order_field regresses, even though the Rust pass would still
        // order whatever rows it received.
        let results = query_service
            .execute(&priority_query(SortDirection::Ascending, Some(2)))
            .await
            .unwrap();

        assert_eq!(
            priorities_of(&results),
            ["highest", "high"],
            "LIMIT must cut by rank in SQL, not alphabetically"
        );
    }

    #[tokio::test]
    async fn test_sort_by_priority_sorts_absent_before_present() {
        let (query_service, node_service, _temp) = create_test_services().await;

        create_tasks_with_priorities(&node_service, &["low", "highest"]).await;
        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "No priority".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {}}),
            lifecycle_status: None,
        };
        node_service.create_node_with_parent(task).await.unwrap();

        let results = query_service
            .execute(&priority_query(SortDirection::Ascending, None))
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert!(
            results[0].properties["task"].get("priority").is_none(),
            "an absent priority sorts first ascending, matching SQL NULL ordering"
        );
        assert_eq!(priorities_of(&results[1..]), ["highest", "low"]);
    }

    #[tokio::test]
    async fn test_sort_by_priority_with_limit_keeps_absent_priority_first() {
        let (query_service, node_service, _temp) = create_test_services().await;

        create_tasks_with_priorities(&node_service, &["low", "highest"]).await;
        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "No priority".to_string(),
            parent_id: None,
            position: crate::services::InsertPositionOwned::End,
            properties: json!({"task": {}}),
            lifecycle_status: None,
        };
        node_service.create_node_with_parent(task).await.unwrap();

        // The combination the two preceding tests each miss: without a LIMIT
        // the Rust re-sort masks whatever SQL did, and the other LIMIT test
        // uses only core values, where the layers happen to agree. A simple
        // `CASE <expr> WHEN ...` ranks NULL via ELSE (it compares with `=`, and
        // `NULL = 'highest'` is NULL, not true), which would drop this task off
        // the end of an ascending query that should return it first.
        let results = query_service
            .execute(&priority_query(SortDirection::Ascending, Some(2)))
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(
            results[0].properties["task"].get("priority").is_none(),
            "SQL must rank an absent priority before the scale, not via ELSE: \
             got {:?}",
            priorities_of(&results)
        );
        assert_eq!(priorities_of(&results[1..]), ["highest"]);
    }

    #[tokio::test]
    async fn test_sql_priority_rank_matches_enum_rank() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        let sql = query_service.resolve_order_field("priority", "task", "ASC");

        // Pins the SQL CASE to TaskPriority::rank(). If a rank changes on one
        // side only, the SQL and Rust orderings disagree and results depend on
        // whether a LIMIT was present.
        for priority in [
            TaskPriority::Highest,
            TaskPriority::High,
            TaskPriority::Medium,
            TaskPriority::Low,
            TaskPriority::Lowest,
        ] {
            let arm = format!("= '{}' THEN {}", priority.as_str(), priority.rank());
            assert!(
                sql.contains(&arm),
                "ORDER BY expression must rank {} as {}: {sql}",
                priority.as_str(),
                priority.rank()
            );
        }

        assert!(
            sql.contains(&format!("ELSE {} END", TaskPriority::USER_RANK)),
            "user-defined priorities must fall to USER_RANK: {sql}"
        );
        // A searched CASE with an explicit IS NULL arm, not a simple CASE: the
        // latter compares with `=`, so NULL matches nothing and an absent
        // priority would be ranked as a user value instead of before the scale.
        assert!(
            sql.contains(&format!("IS NULL THEN {}", TaskPriority::ABSENT_RANK)),
            "an absent priority must rank ABSENT_RANK via an explicit IS NULL \
             arm: {sql}"
        );
        assert!(
            sql.contains("json_extract(properties, '$.task.priority') ASC"),
            "the raw value must remain as a tiebreaker for user-defined values: {sql}"
        );
    }

    #[tokio::test]
    async fn test_resolve_order_field_leaves_non_priority_fields_alone() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        assert_eq!(
            query_service.resolve_order_field("status", "task", "ASC"),
            "json_extract(properties, '$.task.status') ASC"
        );
        assert_eq!(
            query_service.resolve_order_field("created_at", "task", "DESC"),
            "created_at DESC"
        );
        // project.priority is a separate 3-level field with its own open
        // question about scale; the task rank must not silently apply to it.
        assert_eq!(
            query_service.resolve_order_field("priority", "project", "ASC"),
            "json_extract(properties, '$.project.priority') ASC"
        );
    }

    #[tokio::test]
    async fn test_wildcard_priority_sort_does_not_apply_task_rank() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        // A wildcard query resolves the namespace from each row's own
        // node_type, so a single CASE would rank every type's priority on the
        // task scale — and rank a non-task row's NULL as a user value rather
        // than sorting it first. The Rust comparator deliberately ranks only
        // when both nodes are tasks, so ranking here too would make the two
        // layers disagree, with the winner depending on whether a LIMIT was
        // present. Rank only where the scale is actually defined.
        let sql = query_service.resolve_order_field("priority", "*", "ASC");

        assert!(
            !sql.contains("CASE"),
            "wildcard priority sort must not assume the task scale: {sql}"
        );
        assert_eq!(
            sql,
            "json_extract(properties, '$.' || node_type || '.priority') ASC"
        );
    }

    // =========================================================================
    // Index-string coupling
    //
    // `db/schema.rs` hardcodes partial expression indexes
    // whose expressions must byte-for-byte match what resolve_field/
    // build_property_filter generate, or SQLite won't recognize the index as
    // covering the query and will silently fall back to a full scan. These
    // tests fail loudly if either side drifts.
    // =========================================================================

    #[tokio::test]
    async fn test_resolve_field_matches_indexed_expression() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        for (field, expected) in [
            ("status", "json_extract(properties, '$.task.status')"),
            ("due_date", "json_extract(properties, '$.task.due_date')"),
            ("priority", "json_extract(properties, '$.task.priority')"),
        ] {
            assert_eq!(query_service.resolve_field(field, "task"), expected);
        }

        assert_eq!(
            query_service.resolve_field("status", "project"),
            "json_extract(properties, '$.project.status')"
        );
    }

    #[tokio::test]
    async fn test_build_property_filter_matches_indexed_expression() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        let filter = QueryFilter {
            filter_type: FilterType::Property,
            operator: FilterOperator::Equals,
            property: Some("status".to_string()),
            value: Some(json!("open")),
            case_sensitive: None,
            relationship_type: None,
            node_id: None,
        };

        let sql = query_service
            .build_property_filter(&filter, "task")
            .unwrap();
        assert!(
            sql.starts_with("json_extract(properties, '$.task.status')"),
            "build_property_filter output must start with the exact expression \
             idx_task_status is built on, or the index won't cover this filter: {sql}"
        );
    }

    // ========== count ==========

    /// Create `n` tasks with the given status, for the count tests.
    async fn seed_tasks(node_service: &Arc<NodeService>, n: usize, status: &str) {
        for i in 0..n {
            node_service
                .create_node_with_parent(CreateNodeParams {
                    id: None,
                    node_type: "task".to_string(),
                    content: format!("Task {i}"),
                    parent_id: None,
                    position: crate::services::InsertPositionOwned::End,
                    properties: json!({ "task": { "status": status } }),
                    lifecycle_status: None,
                })
                .await
                .unwrap();
        }
    }

    /// The count must be of the same rows `execute` returns — same type filter,
    /// same property filter, same total.
    #[tokio::test]
    async fn test_count_matches_executed_rows() {
        let (query_service, node_service, _temp) = create_test_services().await;

        seed_tasks(&node_service, 3, "open").await;
        seed_tasks(&node_service, 2, "done").await;
        node_service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Not a task".to_string(),
                parent_id: None,
                position: crate::services::InsertPositionOwned::End,
                properties: json!({}),
                lifecycle_status: None,
            })
            .await
            .unwrap();

        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::Equals,
                property: Some("status".to_string()),
                value: Some(json!("open")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let rows = query_service.execute(&query).await.unwrap();
        let count = query_service.count(&query).await.unwrap();

        assert_eq!(count, 3, "Should count only the open tasks");
        assert_eq!(
            count,
            rows.len() as i64,
            "count must agree with execute on the same definition"
        );
    }

    /// The reason this exists: a count must see past the limit that caps a
    /// result set. Counting `execute`'s rows would report the limit itself.
    #[tokio::test]
    async fn test_count_ignores_limit() {
        let (query_service, node_service, _temp) = create_test_services().await;

        seed_tasks(&node_service, 7, "open").await;

        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: None,
            limit: Some(2),
        };

        assert_eq!(
            query_service.execute(&query).await.unwrap().len(),
            2,
            "execute is capped by the limit"
        );
        assert_eq!(
            query_service.count(&query).await.unwrap(),
            7,
            "count must report the true total, not saturate at the limit"
        );
    }

    /// Ordering cannot change how many rows match, and the count SQL must not
    /// emit an ORDER BY at all — including for `task.priority`, whose ordering
    /// builds a CASE expression that has no business in a COUNT(*).
    #[tokio::test]
    async fn test_count_ignores_sorting() {
        let (query_service, node_service, _temp) = create_test_services().await;

        seed_tasks(&node_service, 4, "open").await;

        let sorted = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: Some(vec![SortConfig {
                field: "priority".to_string(),
                direction: SortDirection::Descending,
            }]),
            limit: None,
        };
        let unsorted = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        };

        assert_eq!(
            query_service.count(&sorted).await.unwrap(),
            query_service.count(&unsorted).await.unwrap(),
            "sorting must not change a count"
        );

        let sql = query_service.build_count_query(&sorted).unwrap();
        assert!(
            !sql.contains("ORDER BY") && !sql.contains("LIMIT"),
            "count SQL must carry neither ORDER BY nor LIMIT: {sql}"
        );
    }

    /// The count and the select must be built from one WHERE clause, so a
    /// filter can never select one set of rows and count another.
    #[tokio::test]
    async fn test_count_query_shares_where_clause_with_select() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::Equals,
                property: Some("status".to_string()),
                value: Some(json!("open")),
                case_sensitive: None,
                relationship_type: None,
                node_id: None,
            }],
            sorting: None,
            limit: None,
        };

        let where_clause = query_service.build_where_clause(&query).unwrap();
        assert!(
            where_clause.contains("node_type = 'task'")
                && where_clause.contains("json_extract(properties, '$.task.status') = 'open'"),
            "WHERE clause should carry both the type and property conditions: {where_clause}"
        );
        assert_eq!(
            query_service.build_count_query(&query).unwrap(),
            format!("SELECT COUNT(*) FROM node{where_clause};"),
            "count SQL must be COUNT(*) over exactly the shared WHERE clause"
        );
        assert!(
            query_service
                .build_query(&query)
                .unwrap()
                .contains(&where_clause),
            "the select must be built from that same WHERE clause"
        );
    }

    /// A query matching nothing counts zero rather than erroring on the empty
    /// result — `count_from_sql` requires a row back, and COUNT(*) always
    /// returns one.
    #[tokio::test]
    async fn test_count_empty_result_is_zero() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        let query = QueryDefinition {
            target_type: "task".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        };

        assert_eq!(query_service.count(&query).await.unwrap(), 0);
    }
}
