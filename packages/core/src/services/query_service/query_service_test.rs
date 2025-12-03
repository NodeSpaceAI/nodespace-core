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
}
