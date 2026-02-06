//! Performance benchmarks for NodeSpace core operations
//!
//! Run with: `cargo bench -p nodespace-core`
//!
//! These benchmarks measure critical path performance:
//! - Atomic node operations (create_child_node_atomic)
//! - Markdown import throughput (1000-node imports)
//! - OCC (Optimistic Concurrency Control) overhead

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nodespace_core::db::SurrealStore;
use nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown;
use nodespace_core::services::{CreateNodeParams, NodeService};
use nodespace_core::Node;
use nodespace_core::NodeUpdate;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Setup a test service with a fresh database
async fn setup_test_service() -> (Arc<NodeService>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("bench.db");

    let mut store = Arc::new(SurrealStore::new(db_path).await.unwrap());
    let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
    (node_service, temp_dir)
}

/// Setup a SurrealStore directly for low-level benchmarks
async fn setup_test_store() -> (Arc<SurrealStore>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("bench.db");

    let store = Arc::new(SurrealStore::new(db_path).await.unwrap());
    (store, temp_dir)
}

/// Generate markdown with N nodes for benchmarking
fn generate_large_markdown(node_count: usize) -> String {
    let mut md = String::new();
    let sections = node_count / 4;

    for i in 0..sections {
        let depth = (i % 3) + 2;
        let prefix = "#".repeat(depth);
        md.push_str(&format!("{} Section {}\n\n", prefix, i + 1));
        md.push_str(&format!(
            "This is content paragraph {} with some descriptive text.\n\n",
            i + 1
        ));
        if i % 2 == 0 {
            md.push_str(&format!("- [ ] Task {} - incomplete\n", i * 2 + 1));
        }
        md.push_str(&format!("- [x] Task {} - completed\n\n", i * 2 + 2));
    }

    md
}

/// Benchmark atomic child node creation
///
/// Measures P95 latency for create_child_node_atomic operations.
/// Target: P95 < 15ms in isolation (may be higher under system load)
fn bench_atomic_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    c.bench_function("create_child_node_atomic", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let (store, _temp) = setup_test_store().await;

                // Create parent node
                let parent = store
                    .create_node(
                        Node::new("text".to_string(), "Parent".to_string(), json!({})),
                        None,
                    )
                    .await
                    .unwrap();

                let start = std::time::Instant::now();
                for i in 0..iters {
                    let _child = store
                        .create_child_node_atomic(
                            &parent.id,
                            "text",
                            &format!("Child{}", i),
                            json!({}),
                            None,
                        )
                        .await
                        .unwrap();
                }
                start.elapsed()
            })
        });
    });
}

/// Benchmark 1000-node markdown import
///
/// Measures throughput of markdown import for large documents.
/// Target: > 1000 nodes/sec after batch optimization
fn bench_markdown_import(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("markdown_import");
    group.sample_size(10); // Fewer samples for expensive operations

    group.bench_function("1000_nodes", |b| {
        let markdown = generate_large_markdown(1000);

        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = std::time::Duration::ZERO;

                for _ in 0..iters {
                    let (node_service, _temp) = setup_test_service().await;

                    let params = json!({
                        "markdown_content": markdown.clone(),
                        "title": "Benchmark Test"
                    });

                    let start = std::time::Instant::now();
                    let result =
                        handle_create_nodes_from_markdown(&node_service, params.clone()).await;
                    total += start.elapsed();

                    black_box(result.unwrap());
                }

                total
            })
        });
    });

    group.finish();
}

/// Benchmark OCC (Optimistic Concurrency Control) overhead
///
/// Measures average latency for read-modify-write cycles with version checking.
/// Target: < 5ms per operation under normal load
fn bench_occ_overhead(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    c.bench_function("occ_update_cycle", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let (node_service, _temp) = setup_test_service().await;

                // Create test node
                let node_id = node_service
                    .create_node_with_parent(CreateNodeParams {
                        id: None,
                        node_type: "text".to_string(),
                        content: "Performance test".to_string(),
                        parent_id: None,
                        insert_after_node_id: None,
                        properties: json!({}),
                    })
                    .await
                    .unwrap();

                // Warmup
                let node = node_service.get_node(&node_id).await.unwrap().unwrap();
                let _ = node_service
                    .update_node(
                        &node_id,
                        node.version,
                        NodeUpdate {
                            content: Some("warmup".to_string()),
                            node_type: None,
                            properties: None,
                            title: None,
                            lifecycle_status: None,
                        },
                    )
                    .await
                    .unwrap();

                // Benchmark iterations
                let start = std::time::Instant::now();
                for i in 0..iters {
                    let node = node_service.get_node(&node_id).await.unwrap().unwrap();
                    node_service
                        .update_node(
                            &node_id,
                            node.version,
                            NodeUpdate {
                                content: Some(format!("Update {}", i)),
                                node_type: None,
                                properties: None,
                                title: None,
                                lifecycle_status: None,
                            },
                        )
                        .await
                        .unwrap();
                }
                start.elapsed()
            })
        });
    });
}

/// Benchmark batch GET operations vs sequential calls
///
/// Compares performance of get_nodes_batch (single call for 50 nodes)
/// vs 50 individual get_node calls.
///
/// Note: In-memory operations show modest speedup (1.0-1.5x).
/// Real-world speedup over MCP/IPC is much higher (2-10x) due to network overhead.
fn bench_batch_get(c: &mut Criterion) {
    use nodespace_core::mcp::handlers::nodes::handle_get_nodes_batch;

    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("batch_operations");
    group.sample_size(20);

    // Benchmark sequential individual calls
    group.bench_function("get_50_nodes_sequential", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = std::time::Duration::ZERO;

                for _ in 0..iters {
                    let (node_service, _temp) = setup_test_service().await;

                    // Create 50 test nodes
                    let mut node_ids = Vec::new();
                    for i in 0..50 {
                        let node_id = node_service
                            .create_node_with_parent(CreateNodeParams {
                                id: None,
                                node_type: "text".to_string(),
                                content: format!("Node {}", i),
                                parent_id: None,
                                insert_after_node_id: None,
                                properties: json!({}),
                            })
                            .await
                            .unwrap();
                        node_ids.push(node_id);
                    }

                    let start = std::time::Instant::now();
                    for node_id in &node_ids {
                        black_box(node_service.get_node(node_id).await.unwrap());
                    }
                    total += start.elapsed();
                }

                total
            })
        });
    });

    // Benchmark single batch call
    group.bench_function("get_50_nodes_batch", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = std::time::Duration::ZERO;

                for _ in 0..iters {
                    let (node_service, _temp) = setup_test_service().await;

                    // Create 50 test nodes
                    let mut node_ids = Vec::new();
                    for i in 0..50 {
                        let node_id = node_service
                            .create_node_with_parent(CreateNodeParams {
                                id: None,
                                node_type: "text".to_string(),
                                content: format!("Node {}", i),
                                parent_id: None,
                                insert_after_node_id: None,
                                properties: json!({}),
                            })
                            .await
                            .unwrap();
                        node_ids.push(node_id);
                    }

                    let params = json!({ "node_ids": node_ids });

                    let start = std::time::Instant::now();
                    black_box(handle_get_nodes_batch(&node_service, params).await.unwrap());
                    total += start.elapsed();
                }

                total
            })
        });
    });

    group.finish();
}

/// Benchmark batch UPDATE operations vs sequential calls
///
/// Compares performance of update_nodes_batch (single call for 50 nodes)
/// vs 50 individual update_node calls.
fn bench_batch_update(c: &mut Criterion) {
    use nodespace_core::mcp::handlers::nodes::handle_update_nodes_batch;

    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("batch_operations");
    group.sample_size(20);

    // Benchmark sequential individual updates
    group.bench_function("update_50_nodes_sequential", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = std::time::Duration::ZERO;

                for _ in 0..iters {
                    let (node_service, _temp) = setup_test_service().await;

                    // Create root
                    let root = node_service
                        .create_node_with_parent(CreateNodeParams {
                            id: None,
                            node_type: "text".to_string(),
                            content: "# Benchmark Root".to_string(),
                            parent_id: None,
                            insert_after_node_id: None,
                            properties: json!({}),
                        })
                        .await
                        .unwrap();

                    // Create 50 test nodes
                    let mut node_ids = Vec::new();
                    for i in 0..50 {
                        let node_id = node_service
                            .create_node_with_parent(CreateNodeParams {
                                id: None,
                                node_type: "task".to_string(),
                                content: format!("- [ ] Task {}", i),
                                parent_id: Some(root.clone()),
                                insert_after_node_id: None,
                                properties: json!({}),
                            })
                            .await
                            .unwrap();
                        node_ids.push(node_id);
                    }

                    let start = std::time::Instant::now();
                    for node_id in &node_ids {
                        let node = node_service.get_node(node_id).await.unwrap().unwrap();
                        node_service
                            .update_node(
                                node_id,
                                node.version,
                                NodeUpdate {
                                    content: Some("- [x] Updated task".to_string()),
                                    node_type: None,
                                    properties: None,
                                    title: None,
                                    lifecycle_status: None,
                                },
                            )
                            .await
                            .unwrap();
                    }
                    total += start.elapsed();
                }

                total
            })
        });
    });

    // Benchmark single batch update
    group.bench_function("update_50_nodes_batch", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = std::time::Duration::ZERO;

                for _ in 0..iters {
                    let (node_service, _temp) = setup_test_service().await;

                    // Create root
                    let root = node_service
                        .create_node_with_parent(CreateNodeParams {
                            id: None,
                            node_type: "text".to_string(),
                            content: "# Benchmark Root".to_string(),
                            parent_id: None,
                            insert_after_node_id: None,
                            properties: json!({}),
                        })
                        .await
                        .unwrap();

                    // Create 50 test nodes
                    let mut node_ids = Vec::new();
                    for i in 0..50 {
                        let node_id = node_service
                            .create_node_with_parent(CreateNodeParams {
                                id: None,
                                node_type: "task".to_string(),
                                content: format!("- [ ] Task {}", i),
                                parent_id: Some(root.clone()),
                                insert_after_node_id: None,
                                properties: json!({}),
                            })
                            .await
                            .unwrap();
                        node_ids.push(node_id);
                    }

                    let updates: Vec<serde_json::Value> = node_ids
                        .iter()
                        .map(|id| {
                            json!({
                                "id": id,
                                "content": "- [x] Updated task"
                            })
                        })
                        .collect();

                    let params = json!({ "updates": updates });

                    let start = std::time::Instant::now();
                    black_box(
                        handle_update_nodes_batch(&node_service, params)
                            .await
                            .unwrap(),
                    );
                    total += start.elapsed();
                }

                total
            })
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_atomic_operations,
    bench_markdown_import,
    bench_occ_overhead,
    bench_batch_get,
    bench_batch_update
);
criterion_main!(benches);
