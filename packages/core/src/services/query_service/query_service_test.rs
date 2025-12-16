//! Integration Tests for QueryService
//!
//! These tests validate query execution against a real SurrealDB database,
//! testing SQL generation and result retrieval for all filter types.

#[cfg(test)]
mod tests {
    use crate::db::SurrealStore;
    use crate::services::node_service::{CreateNodeParams, NodeService};
    use crate::services::query_service::{
        FilterOperator, FilterType, QueryDefinition, QueryFilter, QueryService, RelationshipType,
        SortConfig, SortDirection,
    };
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper to create test services with SurrealDB database
    async fn create_test_services() -> (Arc<QueryService>, Arc<NodeService>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let mut store = Arc::new(SurrealStore::new(db_path).await.unwrap());
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
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
        };
        let task2 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task 2".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({"status": "done"}),
        };
        let text1 = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Text node".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({}),
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

        // Create task nodes with different statuses
        let task1 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task 1".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
        };
        let task2 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task 2".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({"status": "done"}),
        };
        let task3 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task 3".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
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
            results.iter().all(|n| n.properties["status"] == "open"),
            "All results should have open status"
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
                insert_after_node_id: None,
                properties: json!({"status": status}),
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
                let status = n.properties["status"].as_str().unwrap();
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
                insert_after_node_id: None,
                properties: json!({"status": "open"}),
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
                insert_after_node_id: None,
                properties: json!({"status": "open"}),
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
            insert_after_node_id: None,
            properties: json!({}),
        };
        let parent_id = node_service.create_node_with_parent(parent).await.unwrap();

        // Create child nodes
        let child1 = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Child 1".to_string(),
            parent_id: Some(parent_id.clone()),
            insert_after_node_id: None,
            properties: json!({}),
        };
        let child2 = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Child 2".to_string(),
            parent_id: Some(parent_id.clone()),
            insert_after_node_id: None,
            properties: json!({}),
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
                insert_after_node_id: None,
                properties: json!({"status": "open"}),
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
                insert_after_node_id: None,
                properties: json!({"status": "open"}),
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

        // Create various task nodes
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
                insert_after_node_id: None,
                properties: json!({
                    "status": status,
                    "priority": priority
                }),
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
        assert_eq!(results[0].properties["status"], "open");
        assert_eq!(results[0].properties["priority"], "high");
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
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
        };
        let text = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Text node".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({}),
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
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
        };
        let task2 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Different".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
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
                insert_after_node_id: None,
                properties: json!({"status": "open"}),
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
    async fn test_query_with_no_spoke_table() {
        let (query_service, _node_service, _temp) = create_test_services().await;

        // Query for a type without a spoke table (should error)
        let query = QueryDefinition {
            target_type: "nonexistent_type".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        };

        let result = query_service.execute(&query).await;
        assert!(
            result.is_err(),
            "Should error for types without spoke tables"
        );
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
            insert_after_node_id: None,
            properties: json!({}),
        };
        let parent_id = node_service.create_node_with_parent(parent).await.unwrap();

        let child = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Child".to_string(),
            parent_id: Some(parent_id.clone()),
            insert_after_node_id: None,
            properties: json!({}),
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
    // Hub-Centric Query Tests (Wildcard target_type = "*")
    // =========================================================================

    #[tokio::test]
    async fn test_hub_query_wildcard_with_metadata_filter() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create nodes with different types
        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Task content".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
        };
        let text = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Text content".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({}),
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
    async fn test_hub_query_content_filter() {
        let (query_service, node_service, _temp) = create_test_services().await;

        let task = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Important meeting".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
        };
        let text = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Random text".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({}),
        };

        node_service.create_node_with_parent(task).await.unwrap();
        node_service.create_node_with_parent(text).await.unwrap();

        // Hub query with content filter (case-sensitive)
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
    async fn test_hub_query_with_sorting_and_limit() {
        let (query_service, node_service, _temp) = create_test_services().await;

        // Create multiple tasks
        for content in &["Zebra", "Alpha", "Beta"] {
            let task = CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: content.to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({"status": "open"}),
            };
            node_service.create_node_with_parent(task).await.unwrap();
        }

        // Hub query with sorting (ascending) and limit
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
                insert_after_node_id: None,
                properties: json!({"status": "open"}),
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
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
        };
        let text = CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "Text".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({}),
        };

        node_service.create_node_with_parent(task).await.unwrap();
        node_service.create_node_with_parent(text).await.unwrap();

        // Hub query sorted by node_type ascending
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
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
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
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
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
            insert_after_node_id: None,
            properties: json!({"status": "open"}),
        };
        let task2 = CreateNodeParams {
            id: None,
            node_type: "task".to_string(),
            content: "Done task".to_string(),
            parent_id: None,
            insert_after_node_id: None,
            properties: json!({"status": "done"}),
        };

        node_service.create_node_with_parent(task1).await.unwrap();
        node_service.create_node_with_parent(task2).await.unwrap();

        // Hub query with metadata (node_type) and content filters
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
                insert_after_node_id: None,
                properties: json!({"status": status}),
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
        // Sorted by status alphabetically: done, in_progress, open
        assert_eq!(results[0].properties["status"], "done");
        assert_eq!(results[1].properties["status"], "in_progress");
        assert_eq!(results[2].properties["status"], "open");
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
                insert_after_node_id: None,
                properties: json!({"status": "open"}),
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
                insert_after_node_id: None,
                properties: json!({"status": status}),
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
}
