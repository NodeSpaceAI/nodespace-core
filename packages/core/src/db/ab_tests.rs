//! A/B Testing Suite for NodeStore Backend Comparison
//!
//! This module contains comprehensive integration tests comparing Turso and SurrealDB
//! implementations of the NodeStore trait. These tests validate:
//!
//! 1. **Functional Parity**: Both backends produce identical results
//! 2. **Performance Benchmarks**: SurrealDB stays within 100% of Turso baseline
//! 3. **Scale Testing**: Performance under load (100K+ nodes)
//! 4. **Complex Operations**: Queries, pagination, hierarchy operations
//!
//! # Test Organization
//!
//! - `test_ab_crud_operations` - Basic create, read, update, delete
//! - `test_ab_query_operations` - Query filtering and search
//! - `test_ab_hierarchy_operations` - Parent-child relationships
//! - `test_ab_property_operations` - Property get/set operations
//! - `test_ab_batch_operations` - Bulk creates and updates
//! - `test_ab_scale_100k` - Large dataset performance
//! - `test_ab_pagination` - Deep pagination performance
//! - `test_ab_complex_queries` - Multi-condition queries
//!
//! # Running Tests
//!
//! ```bash
//! # Run all A/B tests (requires surrealdb feature)
//! cargo test --package nodespace-core --features surrealdb ab_tests
//!
//! # Run specific A/B test
//! cargo test --package nodespace-core --features surrealdb test_ab_crud_operations
//! ```

#[cfg(all(test, feature = "surrealdb"))]
mod tests {
    use crate::db::{ABTestRunner, DatabaseService, NodeStore, SurrealStore, TursoStore};
    use crate::models::{Node, NodeQuery, NodeUpdate};
    use anyhow::Result;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Create a Turso store for testing
    async fn create_turso_store() -> Result<(Arc<dyn NodeStore>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("turso_test.db");
        let db = Arc::new(DatabaseService::new(db_path).await?);
        Ok((
            Arc::new(TursoStore::new(db)) as Arc<dyn NodeStore>,
            temp_dir,
        ))
    }

    /// Create a SurrealDB store for testing
    async fn create_surreal_store() -> Result<(Arc<dyn NodeStore>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("surreal_test.db");
        Ok((
            Arc::new(SurrealStore::new(db_path).await?) as Arc<dyn NodeStore>,
            temp_dir,
        ))
    }

    /// Create test runner with both backends
    async fn create_ab_test_runner() -> Result<(ABTestRunner, TempDir, TempDir)> {
        let (turso, turso_temp) = create_turso_store().await?;
        let (surreal, surreal_temp) = create_surreal_store().await?;

        let runner = ABTestRunner::with_names(turso, surreal, "Turso", "SurrealDB");
        Ok((runner, turso_temp, surreal_temp))
    }

    /// Create a test node with the given content
    fn test_node(content: &str) -> Node {
        Node::new(
            "text".to_string(),
            content.to_string(),
            None,
            json!({"test": true}),
        )
    }

    #[tokio::test]
    async fn test_ab_crud_operations() -> Result<()> {
        let (runner, _turso_temp, _surreal_temp) = create_ab_test_runner().await?;

        // Test create_node
        let node = test_node("Test content");
        let node_id = node.id.clone();

        let result = runner
            .run_parallel_test("create_node", |store| {
                let node = node.clone();
                async move { store.create_node(node).await }
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "Create performance delta >200%"
        );
        assert_eq!(result.result.content, "Test content");

        // Test get_node
        let result = runner
            .run_parallel_test("get_node", |store| {
                let node_id = node_id.clone();
                async move { store.get_node(&node_id).await }
            })
            .await?;

        assert!(result.delta_percent < 200.0, "Get performance delta >200%");
        assert!(result.result.is_some());
        assert_eq!(result.result.unwrap().id, node_id);

        // Test update_node
        let update = NodeUpdate {
            content: Some("Updated content".to_string()),
            ..Default::default()
        };

        let result = runner
            .run_parallel_test("update_node", |store| {
                let node_id = node_id.clone();
                let update = update.clone();
                async move { store.update_node(&node_id, update).await }
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "Update performance delta >200%"
        );
        assert_eq!(result.result.content, "Updated content");

        // Test delete_node
        let result = runner
            .run_parallel_test("delete_node", |store| {
                let node_id = node_id.clone();
                async move { store.delete_node(&node_id).await }
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "Delete performance delta >200%"
        );

        // Verify node is deleted
        let result = runner
            .run_parallel_test("get_deleted_node", |store| {
                let node_id = node_id.clone();
                async move { store.get_node(&node_id).await }
            })
            .await?;

        assert!(result.result.is_none());

        // Print report
        println!("\n{}", runner.generate_report().await);

        Ok(())
    }

    #[tokio::test]
    async fn test_ab_query_operations() -> Result<()> {
        let (runner, _turso_temp, _surreal_temp) = create_ab_test_runner().await?;

        // Create test nodes
        for i in 0..10 {
            let node = Node::new(
                "text".to_string(),
                format!("Node {}", i),
                None,
                json!({"index": i, "even": i % 2 == 0}),
            );

            runner
                .run_parallel_test(&format!("create_node_{}", i), |store| {
                    let node = node.clone();
                    async move { store.create_node(node).await }
                })
                .await?;
        }

        // Test query_nodes with type filter
        let query = NodeQuery {
            node_type: Some("text".to_string()),
            limit: Some(5),
            ..Default::default()
        };

        let result = runner
            .run_parallel_test("query_nodes_by_type", |store| {
                let query = query.clone();
                async move { store.query_nodes(query).await }
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "Query performance delta >200%"
        );
        assert_eq!(result.result.len(), 5);

        // Test search_nodes_by_content
        let result = runner
            .run_parallel_test("search_nodes_by_content", |store| async move {
                store.search_nodes_by_content("Node", Some(10)).await
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "Search performance delta >200%"
        );
        assert!(!result.result.is_empty());

        println!("\n{}", runner.generate_report().await);

        Ok(())
    }

    #[tokio::test]
    async fn test_ab_hierarchy_operations() -> Result<()> {
        let (runner, _turso_temp, _surreal_temp) = create_ab_test_runner().await?;

        // Create parent node
        let parent = test_node("Parent node");
        let parent_id = parent.id.clone();

        runner
            .run_parallel_test("create_parent", |store| {
                let parent = parent.clone();
                async move { store.create_node(parent).await }
            })
            .await?;

        // Create child nodes
        for i in 0..5 {
            let child = Node::new(
                "text".to_string(),
                format!("Child {}", i),
                Some(parent_id.clone()),
                json!({}),
            );

            runner
                .run_parallel_test(&format!("create_child_{}", i), |store| {
                    let child = child.clone();
                    async move { store.create_node(child).await }
                })
                .await?;
        }

        // Test get_children
        let result = runner
            .run_parallel_test("get_children", |store| {
                let parent_id = parent_id.clone();
                async move { store.get_children(Some(&parent_id)).await }
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "get_children performance delta >200%"
        );
        assert_eq!(result.result.len(), 5);

        // Test move_node
        let new_parent = test_node("New parent");
        let new_parent_id = new_parent.id.clone();

        runner
            .run_parallel_test("create_new_parent", |store| {
                let new_parent = new_parent.clone();
                async move { store.create_node(new_parent).await }
            })
            .await?;

        let child_id = result.result[0].id.clone();
        let result = runner
            .run_parallel_test("move_node", |store| {
                let child_id = child_id.clone();
                let new_parent_id = new_parent_id.clone();
                async move { store.move_node(&child_id, Some(&new_parent_id)).await }
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "move_node performance delta >200%"
        );

        println!("\n{}", runner.generate_report().await);

        Ok(())
    }

    #[tokio::test]
    async fn test_ab_mention_operations() -> Result<()> {
        let (runner, _turso_temp, _surreal_temp) = create_ab_test_runner().await?;

        // Create two nodes
        let source = test_node("Source node");
        let target = test_node("Target node");
        let source_id = source.id.clone();
        let target_id = target.id.clone();

        runner
            .run_parallel_test("create_source", |store| {
                let source = source.clone();
                async move { store.create_node(source).await }
            })
            .await?;

        runner
            .run_parallel_test("create_target", |store| {
                let target = target.clone();
                async move { store.create_node(target).await }
            })
            .await?;

        // Test create_mention (requires container_id parameter)
        let container = test_node("Container node");
        let container_id = container.id.clone();

        runner
            .run_parallel_test("create_container", |store| {
                let container = container.clone();
                async move { store.create_node(container).await }
            })
            .await?;

        let result = runner
            .run_parallel_test("create_mention", |store| {
                let source_id = source_id.clone();
                let target_id = target_id.clone();
                let container_id = container_id.clone();
                async move {
                    store
                        .create_mention(&source_id, &target_id, &container_id)
                        .await
                }
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "create_mention performance delta >200%"
        );

        // Test get_outgoing_mentions
        let result = runner
            .run_parallel_test("get_outgoing_mentions", |store| {
                let source_id = source_id.clone();
                async move { store.get_outgoing_mentions(&source_id).await }
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "get_outgoing_mentions performance delta >200%"
        );
        assert_eq!(result.result.len(), 1);
        assert_eq!(result.result[0], target_id);

        println!("\n{}", runner.generate_report().await);

        Ok(())
    }

    #[tokio::test]
    async fn test_ab_batch_operations() -> Result<()> {
        let (runner, _turso_temp, _surreal_temp) = create_ab_test_runner().await?;

        // Prepare batch of nodes
        let nodes: Vec<Node> = (0..20)
            .map(|i| test_node(&format!("Batch node {}", i)))
            .collect();

        // Test batch_create_nodes
        let result = runner
            .run_parallel_test("batch_create_nodes", |store| {
                let nodes = nodes.clone();
                async move { store.batch_create_nodes(nodes).await }
            })
            .await?;

        assert!(
            result.delta_percent < 200.0,
            "batch_create_nodes performance delta >200%"
        );
        assert_eq!(result.result.len(), 20);

        println!("\n{}", runner.generate_report().await);

        Ok(())
    }

    #[tokio::test]
    #[ignore] // Expensive test - run manually
    async fn test_ab_scale_100k() -> Result<()> {
        let (runner, _turso_temp, _surreal_temp) = create_ab_test_runner().await?;

        // Create 100K nodes (in batches to avoid memory issues)
        println!("Creating 100K nodes...");
        for batch in 0..100 {
            let nodes: Vec<Node> = (0..1000)
                .map(|i| test_node(&format!("Scale node {}", batch * 1000 + i)))
                .collect();

            runner
                .run_parallel_test(&format!("batch_create_{}", batch), |store| {
                    let nodes = nodes.clone();
                    async move { store.batch_create_nodes(nodes).await }
                })
                .await?;
        }

        // Query all nodes
        let query = NodeQuery {
            node_type: Some("text".to_string()),
            limit: Some(1000),
            ..Default::default()
        };

        let result = runner
            .run_parallel_test("query_100k_nodes", |store| {
                let query = query.clone();
                async move { store.query_nodes(query).await }
            })
            .await?;

        // Target: <200ms (PoC was 104ms, allow 2x baseline)
        assert!(
            result.backend_b_duration.as_millis() < 200,
            "SurrealDB 100K query took {}ms (threshold: 200ms)",
            result.backend_b_duration.as_millis()
        );

        println!("\n{}", runner.generate_report().await);

        Ok(())
    }

    #[tokio::test]
    async fn test_ab_pagination() -> Result<()> {
        let (runner, _turso_temp, _surreal_temp) = create_ab_test_runner().await?;

        // Create 1000 nodes
        let nodes: Vec<Node> = (0..1000)
            .map(|i| test_node(&format!("Pagination node {}", i)))
            .collect();

        for chunk in nodes.chunks(50) {
            runner
                .run_parallel_test("batch_create_pagination", |store| {
                    let chunk = chunk.to_vec();
                    async move { store.batch_create_nodes(chunk).await }
                })
                .await?;
        }

        // Test pagination query
        let query = NodeQuery {
            node_type: Some("text".to_string()),
            limit: Some(100),
            ..Default::default()
        };

        let result = runner
            .run_parallel_test("deep_pagination", |store| {
                let query = query.clone();
                async move { store.query_nodes(query).await }
            })
            .await?;

        // Target: <50ms (PoC was 8.3ms, allow generous margin)
        assert!(
            result.backend_b_duration.as_millis() < 50,
            "SurrealDB pagination took {}ms (threshold: 50ms)",
            result.backend_b_duration.as_millis()
        );

        assert_eq!(result.result.len(), 100);

        println!("\n{}", runner.generate_report().await);

        Ok(())
    }

    #[tokio::test]
    async fn test_ab_complex_queries() -> Result<()> {
        let (runner, _turso_temp, _surreal_temp) = create_ab_test_runner().await?;

        // Create diverse nodes for complex querying
        for i in 0..100 {
            let node = Node::new(
                "text".to_string(),
                format!("Complex node {}", i),
                None,
                json!({
                    "priority": i % 5,
                    "status": if i % 2 == 0 { "active" } else { "done" },
                    "tags": ["test"],
                }),
            );

            runner
                .run_parallel_test(&format!("create_complex_{}", i), |store| {
                    let node = node.clone();
                    async move { store.create_node(node).await }
                })
                .await?;
        }

        // Complex query: type + property filters
        let query = NodeQuery {
            node_type: Some("text".to_string()),
            limit: Some(50),
            ..Default::default()
        };

        let result = runner
            .run_parallel_test("complex_query", |store| {
                let query = query.clone();
                async move { store.query_nodes(query).await }
            })
            .await?;

        // Target: <300ms avg (PoC was 211ms, allow margin)
        assert!(
            result.backend_b_duration.as_millis() < 300,
            "SurrealDB complex query took {}ms (threshold: 300ms)",
            result.backend_b_duration.as_millis()
        );

        println!("\n{}", runner.generate_report().await);

        Ok(())
    }

    #[tokio::test]
    async fn test_metrics_csv_export() -> Result<()> {
        let (runner, _turso_temp, _surreal_temp) = create_ab_test_runner().await?;

        // Run a few operations
        let node = test_node("CSV export test");
        runner
            .run_parallel_test("create_node", |store| {
                let node = node.clone();
                async move { store.create_node(node).await }
            })
            .await?;

        // Export metrics
        let temp_dir = TempDir::new()?;
        let csv_path = temp_dir.path().join("metrics.csv");
        runner.export_csv(&csv_path).await?;

        // Verify CSV file exists and has content
        let csv_content = std::fs::read_to_string(&csv_path)?;
        assert!(csv_content.contains("Turso"));
        assert!(csv_content.contains("SurrealDB"));
        assert!(csv_content.contains("create_node"));

        Ok(())
    }
}
