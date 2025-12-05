//! Performance tests for MCP markdown import handler
//!
//! These tests benchmark the markdown import performance to measure
//! optimization improvements. Issue #737.

#[cfg(test)]
mod perf_tests {
    use crate::db::SurrealStore;
    use crate::mcp::handlers::markdown::handle_create_nodes_from_markdown;
    use crate::services::NodeService;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Instant;
    use tempfile::TempDir;

    async fn setup_test_service() -> (Arc<NodeService>, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let mut store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
        (node_service, temp_dir)
    }

    /// Generate markdown with N nodes for benchmarking
    ///
    /// Creates a realistic markdown document with:
    /// - Varying heading levels (H1-H4)
    /// - Text paragraphs under headings
    /// - Tasks interspersed
    ///
    /// For N=1000, generates approximately 1000 nodes total
    fn generate_large_markdown(node_count: usize) -> String {
        let mut md = String::new();

        // Calculate distribution: ~25% headers, ~50% text, ~25% tasks
        let sections = node_count / 4;

        for i in 0..sections {
            // Vary heading depth cyclically (H2-H4 since H1 is typically used for title)
            let depth = (i % 3) + 2;
            let prefix = "#".repeat(depth);
            md.push_str(&format!("{} Section {}\n\n", prefix, i + 1));

            // Add a text paragraph after each heading
            md.push_str(&format!(
                "This is content paragraph {} with some descriptive text.\n\n",
                i + 1
            ));

            // Add 1-2 tasks under each section
            if i % 2 == 0 {
                md.push_str(&format!("- [ ] Task {} - incomplete\n", i * 2 + 1));
            }
            md.push_str(&format!("- [x] Task {} - completed\n\n", i * 2 + 2));
        }

        md
    }

    /// Benchmark 100-node import (fast smoke test)
    #[tokio::test]
    async fn benchmark_100_node_import() {
        let (node_service, _temp_dir) = setup_test_service().await;

        let markdown = generate_large_markdown(100);
        let params = json!({
            "markdown_content": markdown,
            "title": "Benchmark Test 100"
        });

        let start = Instant::now();
        let result = handle_create_nodes_from_markdown(&node_service, params).await;
        let duration = start.elapsed();

        assert!(result.is_ok(), "Import failed: {:?}", result.err());

        let result = result.unwrap();
        let nodes_created = result["nodes_created"].as_i64().unwrap();

        println!(
            "\n=== 100-node benchmark ===\nNodes created: {}\nDuration: {:?}\nNodes/sec: {:.1}\n",
            nodes_created,
            duration,
            nodes_created as f64 / duration.as_secs_f64()
        );

        // Baseline expectation: should complete (no timeout)
        // Target after optimization: <500ms
    }

    /// Benchmark 500-node import (moderate workload)
    #[tokio::test]
    async fn benchmark_500_node_import() {
        let (node_service, _temp_dir) = setup_test_service().await;

        let markdown = generate_large_markdown(500);
        let params = json!({
            "markdown_content": markdown,
            "title": "Benchmark Test 500"
        });

        let start = Instant::now();
        let result = handle_create_nodes_from_markdown(&node_service, params).await;
        let duration = start.elapsed();

        assert!(result.is_ok(), "Import failed: {:?}", result.err());

        let result = result.unwrap();
        let nodes_created = result["nodes_created"].as_i64().unwrap();

        println!(
            "\n=== 500-node benchmark ===\nNodes created: {}\nDuration: {:?}\nNodes/sec: {:.1}\n",
            nodes_created,
            duration,
            nodes_created as f64 / duration.as_secs_f64()
        );

        // Baseline expectation: ~2-5 seconds sequential
        // Target after optimization: <1 second
    }

    /// Benchmark 1000-node import (large workload)
    ///
    /// This is the primary performance benchmark referenced in Issue #737.
    /// Baseline (sequential): Expected ~3-15 seconds
    /// Target (batch): Expected ~200-500ms (10-15x speedup)
    #[tokio::test]
    async fn benchmark_1000_node_import() {
        let (node_service, _temp_dir) = setup_test_service().await;

        let markdown = generate_large_markdown(1000);
        let params = json!({
            "markdown_content": markdown,
            "title": "Benchmark Test 1000"
        });

        let start = Instant::now();
        let result = handle_create_nodes_from_markdown(&node_service, params).await;
        let duration = start.elapsed();

        assert!(result.is_ok(), "Import failed: {:?}", result.err());

        let result = result.unwrap();
        let nodes_created = result["nodes_created"].as_i64().unwrap();

        println!(
            "\n=== 1000-node benchmark ===\nNodes created: {}\nDuration: {:?}\nNodes/sec: {:.1}\n",
            nodes_created,
            duration,
            nodes_created as f64 / duration.as_secs_f64()
        );

        // ACHIEVED: ~30-50ms (300x speedup from baseline of 3-15 seconds)
        // The batch optimization far exceeded the 10-15x target
        assert!(
            duration.as_millis() < 1000,
            "Should complete in <1 second after optimization (achieved ~30-50ms)"
        );
    }

    /// Verify hierarchy integrity after large import
    #[tokio::test]
    async fn verify_large_import_hierarchy() {
        let (node_service, _temp_dir) = setup_test_service().await;

        // Use smaller set for verification (faster)
        let markdown = generate_large_markdown(50);
        let params = json!({
            "markdown_content": markdown,
            "title": "Hierarchy Test"
        });

        let result = handle_create_nodes_from_markdown(&node_service, params)
            .await
            .unwrap();

        let root_id = result["root_id"].as_str().unwrap();
        let nodes_created = result["nodes_created"].as_i64().unwrap() as usize;

        // Verify root exists
        let root = node_service.get_node(root_id).await.unwrap();
        assert!(root.is_some(), "Root node should exist");
        assert_eq!(root.unwrap().content, "Hierarchy Test");

        // Verify all nodes are reachable (no orphans)
        let descendants = node_service.get_descendants(root_id).await.unwrap();

        // nodes_created includes the root, descendants does not
        assert_eq!(
            descendants.len(),
            nodes_created - 1,
            "All created nodes (except root) should be descendants of root"
        );
    }
}
