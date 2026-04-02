//! Performance benchmarks for NodeSpace core operations
//!
//! Run with: `cargo bench -p nodespace-core`
//!
//! These benchmarks measure critical path performance:
//! - Atomic node operations (create_child_node_atomic)
//! - Markdown import throughput (1000-node imports)
//! - OCC (Optimistic Concurrency Control) overhead
//! - Playbook engine: trigger index lookup, path extraction, graph resolution,
//!   CEL evaluation, activation at scale, event-to-rule matching throughput

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use nodespace_core::db::events::DomainEvent;
use nodespace_core::db::SurrealStore;
use nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown;
use nodespace_core::playbook::cel;
use nodespace_core::playbook::graph_resolver::GraphResolver;
use nodespace_core::playbook::lifecycle::{trigger_keys_for_event, PlaybookLifecycleManager};
use nodespace_core::playbook::path_extractor;
use nodespace_core::playbook::types::*;
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

/// Benchmark BM25 root-walk latency at realistic corpus sizes (Issue #951)
///
/// Measures: BM25 `@@` query + iterative parent-walk to resolve roots.
/// Run at 100, 500, and 1000 nodes with 3-level nesting (root→child→grandchild).
/// This is the BM25 leg of hybrid search; it runs in parallel with KNN (~50-100ms).
/// Target: BM25 path completes well under KNN latency so it stays hidden.
fn bench_bm25_search_roots(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("bm25_search_roots");

    for corpus_size in [100usize, 500, 1000] {
        // Build corpus once outside the benchmark loop
        let (store_arc, _temp_dir) = rt.block_on(async {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("bench.db");
            let mut store_arc = Arc::new(SurrealStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store_arc).await.unwrap());

            // Build corpus: root → child → grandchild trees
            // Every 3rd node is a root; others are children/grandchildren.
            // The term "persistence" appears in ~20% of nodes.
            let roots_count = corpus_size / 3;
            for i in 0..roots_count {
                let root_content = if i % 5 == 0 {
                    format!("Root document {} about persistence coordinator", i)
                } else {
                    format!("Root document {} about general architecture", i)
                };
                let root_id = node_service
                    .create_node(Node::new("text".to_string(), root_content, json!({})))
                    .await
                    .unwrap();

                let child_content = if i % 7 == 0 {
                    format!("Child section {} with debounce and persistence details", i)
                } else {
                    format!("Child section {} with implementation details", i)
                };
                let child_id = node_service
                    .create_node(Node::new("text".to_string(), child_content, json!({})))
                    .await
                    .unwrap();
                node_service
                    .move_node_unchecked(&child_id, Some(&root_id), None)
                    .await
                    .unwrap();

                let grandchild_content = format!("Grandchild node {} leaf content", i);
                let grandchild_id = node_service
                    .create_node(Node::new("text".to_string(), grandchild_content, json!({})))
                    .await
                    .unwrap();
                node_service
                    .move_node_unchecked(&grandchild_id, Some(&child_id), None)
                    .await
                    .unwrap();
            }

            (store_arc, temp_dir)
        });

        group.bench_function(format!("corpus_{}_nodes", corpus_size), |b| {
            b.iter(|| {
                rt.block_on(async {
                    black_box(
                        store_arc
                            .bm25_search_roots("persistence", 100)
                            .await
                            .unwrap(),
                    )
                })
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Playbook Engine Benchmarks
// ---------------------------------------------------------------------------

/// Helper: create a playbook node with the given rules JSON.
fn make_playbook_node(id: &str, rules_json: serde_json::Value) -> Node {
    Node::new_with_id(
        id.to_string(),
        "playbook".to_string(),
        format!("playbook {}", id),
        json!({ "rules": rules_json }),
    )
}

/// Generate N playbooks all triggering on the same node_type.
///
/// Each playbook has a single rule with a `node_created` trigger on the
/// given `node_type`. Conditions and actions are empty because the
/// trigger-index and event-matching benchmarks only exercise lookup, not
/// condition evaluation.
fn generate_playbooks(count: usize, node_type: &str) -> Vec<Node> {
    (0..count)
        .map(|i| {
            make_playbook_node(
                &format!("pb-bench-{}", i),
                json!([{
                    "name": format!("rule-{}", i),
                    "trigger": {
                        "type": "graph_event",
                        "on": "node_created",
                        "node_type": node_type
                    },
                    "conditions": [],
                    "actions": []
                }]),
            )
        })
        .collect()
}

/// Benchmark 1: TriggerIndex lookup performance
///
/// Measures how `lookup_rules()` latency scales with the number of active
/// playbooks. All playbooks trigger on the same node_type, testing the
/// worst-case for a single TriggerKey bucket.
fn bench_trigger_index_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("playbook/trigger_index_lookup");

    for count in [10, 100, 500, 1000] {
        // Setup: activate N playbooks
        let mut lm = PlaybookLifecycleManager::new();
        let playbooks = generate_playbooks(count, "task");
        for pb in &playbooks {
            lm.activate_playbook(pb).unwrap();
        }

        let keys = vec![TriggerKey::NodeEvent {
            event: NodeEventType::NodeCreated,
            node_type: "task".to_string(),
            property_key: None,
        }];

        group.bench_with_input(BenchmarkId::new("playbooks", count), &count, |b, _| {
            b.iter(|| {
                black_box(lm.lookup_rules(&keys));
            });
        });
    }

    group.finish();
}

/// Benchmark 2: Path extraction performance
///
/// Measures `path_extractor::extract_paths()` latency across expressions
/// of varying complexity, from single property access to deep multi-hop
/// paths with comprehension macros.
fn bench_path_extraction(c: &mut Criterion) {
    let mut group = c.benchmark_group("playbook/path_extraction");

    let expressions: Vec<(&str, &str)> = vec![
        ("simple_property", "node.status == 'open'"),
        ("two_conditions", "node.status == 'open' && node.priority == 'high'"),
        ("two_hop_path", "node.story.status == 'active'"),
        ("three_hop_path", "node.story.epic.status == 'active'"),
        (
            "deep_path_with_and",
            "node.story.epic.project.owner == 'alice' && node.status == 'open'",
        ),
        (
            "comprehension_exists",
            "node.tasks.exists(t, t.status == 'done')",
        ),
        (
            "comprehension_with_outer_ref",
            "node.items.exists(t, t.value > node.threshold)",
        ),
        (
            "complex_mixed",
            "node.status == 'open' && node.story.epic.status == 'active' && node.tasks.exists(t, t.status == 'done')",
        ),
    ];

    for (name, expr) in &expressions {
        group.bench_with_input(BenchmarkId::new("expr", *name), expr, |b, expr| {
            b.iter(|| {
                black_box(path_extractor::extract_paths(expr).unwrap());
            });
        });
    }

    group.finish();
}

/// Benchmark 3: GraphResolver path resolution
///
/// Measures `GraphResolver::resolve_path()` for 1-hop, 3-hop, and 5-hop
/// relationship chains. Also benchmarks cache effectiveness by resolving
/// the same path twice and comparing latency.
///
/// Requires a multi-threaded tokio runtime since GraphResolver uses
/// `block_in_place` internally.
fn bench_graph_resolver(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("playbook/graph_resolver");
    group.sample_size(20);

    // Setup: create a schema chain of depth 5
    // type_0 -> type_1 -> type_2 -> type_3 -> type_4
    //
    // SAFETY: _temp_dir must outlive all benchmark iterations — it holds the
    // on-disk DB files that `svc` (via SurrealStore) references. Dropping it
    // early would delete the database out from under the NodeService. The
    // binding is kept alive until `group.finish()` returns at function end.
    let (svc, _temp_dir, root_node, schema_names) = rt.block_on(async {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("bench_resolver.db");
        let mut store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let svc = Arc::new(NodeService::new(&mut store).await.unwrap());

        let depth = 5;
        let schema_names = create_schema_chain(&svc, depth).await;
        let node_ids = create_linked_nodes(&svc, &schema_names).await;

        // Fetch the root node (first in chain)
        let root_node = svc.get_node(&node_ids[0]).await.unwrap().unwrap();

        (svc, temp_dir, root_node, schema_names)
    });

    // Benchmark: 1-hop resolution (root -> type_1)
    {
        let svc_clone = Arc::clone(&svc);
        let root = root_node.clone();
        let rel_name = schema_names[1].clone(); // relationship name is the target type name

        group.bench_function("1_hop_cold", |b| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = std::time::Duration::ZERO;
                    for _ in 0..iters {
                        let mut resolver = GraphResolver::new(Arc::clone(&svc_clone));
                        let start = std::time::Instant::now();
                        black_box(resolver.resolve_path(&root, &[rel_name.clone()]));
                        total += start.elapsed();
                    }
                    total
                })
            });
        });
    }

    // Benchmark: 3-hop resolution (root -> type_1 -> type_2 -> type_3)
    {
        let svc_clone = Arc::clone(&svc);
        let root = root_node.clone();
        let segments: Vec<String> = schema_names[1..4].to_vec();

        group.bench_function("3_hop_cold", |b| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = std::time::Duration::ZERO;
                    for _ in 0..iters {
                        let mut resolver = GraphResolver::new(Arc::clone(&svc_clone));
                        let start = std::time::Instant::now();
                        black_box(resolver.resolve_path(&root, &segments));
                        total += start.elapsed();
                    }
                    total
                })
            });
        });
    }

    // Benchmark: 4-hop + property resolution
    // (root -> type_1 -> type_2 -> type_3 -> type_4 . status)
    // 4 relationship hops through the schema chain, then 1 scalar property access
    {
        let svc_clone = Arc::clone(&svc);
        let root = root_node.clone();
        let mut segments: Vec<String> = schema_names[1..].to_vec(); // 4 relationship segments
        segments.push("status".to_string()); // final property access (not a hop)

        group.bench_function("4_hop_to_property_cold", |b| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = std::time::Duration::ZERO;
                    for _ in 0..iters {
                        let mut resolver = GraphResolver::new(Arc::clone(&svc_clone));
                        let start = std::time::Instant::now();
                        black_box(resolver.resolve_path(&root, &segments));
                        total += start.elapsed();
                    }
                    total
                })
            });
        });
    }

    // Benchmark: cache effectiveness — resolve 3-hop twice
    {
        let svc_clone = Arc::clone(&svc);
        let root = root_node.clone();
        let segments: Vec<String> = schema_names[1..4].to_vec();

        group.bench_function("3_hop_cached", |b| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = std::time::Duration::ZERO;
                    for _ in 0..iters {
                        let mut resolver = GraphResolver::new(Arc::clone(&svc_clone));
                        // First call populates cache
                        resolver.resolve_path(&root, &segments);
                        // Second call should hit cache
                        let start = std::time::Instant::now();
                        black_box(resolver.resolve_path(&root, &segments));
                        total += start.elapsed();
                    }
                    total
                })
            });
        });
    }

    group.finish();
}

/// Create a schema chain of the given depth.
///
/// Returns the type names: `["bench_type_0", "bench_type_1", ..., "bench_type_{depth-1}"]`.
/// Each type (except the last) has a relationship to the next type in the chain.
async fn create_schema_chain(svc: &Arc<NodeService>, depth: usize) -> Vec<String> {
    let type_names: Vec<String> = (0..depth).map(|i| format!("bench_type_{}", i)).collect();

    for i in 0..depth {
        let rels = if i < depth - 1 {
            json!([{
                "name": type_names[i + 1],
                "target_type": type_names[i + 1],
                "direction": "out",
                "cardinality": "one"
            }])
        } else {
            json!([])
        };

        let schema = Node::new_with_id(
            type_names[i].clone(),
            "schema".to_string(),
            type_names[i].clone(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": format!("Bench schema {}", i),
                "fields": [{"name": "status", "type": "string"}],
                "relationships": rels
            }),
        );
        svc.create_node(schema)
            .await
            .unwrap_or_else(|_| panic!("Failed to create schema '{}'", type_names[i]));
    }

    type_names
}

/// Create linked nodes following the schema chain.
///
/// Returns node IDs in chain order. Each node has a relationship to the next.
async fn create_linked_nodes(svc: &Arc<NodeService>, schema_names: &[String]) -> Vec<String> {
    let mut node_ids = Vec::with_capacity(schema_names.len());

    for (i, type_name) in schema_names.iter().enumerate() {
        let node = Node::new_with_id(
            format!("bench-node-{}", i),
            type_name.clone(),
            format!("Bench node {}", i),
            json!({"status": "active"}),
        );
        svc.create_node(node).await.unwrap();
        node_ids.push(format!("bench-node-{}", i));
    }

    // Create relationships between consecutive nodes
    for i in 0..schema_names.len() - 1 {
        svc.create_relationship(
            &node_ids[i],
            &schema_names[i + 1], // relationship name = target type name
            &node_ids[i + 1],
            json!({}),
        )
        .await
        .unwrap();
    }

    node_ids
}

/// Benchmark 4: CEL evaluation with graph traversal (end-to-end)
///
/// Measures the full cost of path extraction + graph resolution + CEL
/// evaluation, comparing simple property conditions (no resolver needed)
/// against multi-hop path conditions (requiring graph traversal).
fn bench_cel_evaluation_e2e(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut group = c.benchmark_group("playbook/cel_evaluation");
    group.sample_size(20);

    // Setup: create a 3-node chain with relationships:
    //   bench_type_0 --[bench_type_1]--> bench_type_1 --[bench_type_2]--> bench_type_2
    // Each node has a "status": "active" property.
    // CEL expressions reference this chain, e.g. `node.bench_type_1.bench_type_2.status`.
    //
    // SAFETY: _temp_dir must outlive all benchmark iterations (holds DB files on disk).
    let (svc, _temp_dir, root_node) = rt.block_on(async {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("bench_cel.db");
        let mut store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let svc = Arc::new(NodeService::new(&mut store).await.unwrap());

        let schemas = create_schema_chain(&svc, 3).await;
        let node_ids = create_linked_nodes(&svc, &schemas).await;
        let root_node = svc.get_node(&node_ids[0]).await.unwrap().unwrap();

        (svc, temp_dir, root_node)
    });

    let event = DomainEvent::NodeCreated {
        node_type: "bench_type_0".to_string(),
        node_id: "bench-node-0".to_string(),
    };

    // Benchmark: simple property condition (no graph traversal)
    {
        let node = root_node.clone();
        let evt = event.clone();

        group.bench_function("simple_property_no_resolver", |b| {
            b.iter(|| {
                black_box(cel::evaluate_conditions(
                    &["node.status == 'active'".to_string()],
                    &node,
                    &evt,
                    None,
                ));
            });
        });
    }

    // Benchmark: 2-hop condition with graph resolver
    {
        let svc_clone = Arc::clone(&svc);
        let node = root_node.clone();
        let evt = event.clone();

        group.bench_function("2_hop_with_resolver", |b| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = std::time::Duration::ZERO;
                    for _ in 0..iters {
                        let mut resolver = GraphResolver::new(Arc::clone(&svc_clone));
                        let start = std::time::Instant::now();
                        black_box(cel::evaluate_conditions(
                            &["node.bench_type_1.bench_type_2.status == 'active'".to_string()],
                            &node,
                            &evt,
                            Some(&mut resolver),
                        ));
                        total += start.elapsed();
                    }
                    total
                })
            });
        });
    }

    // Benchmark: multiple conditions (property + multi-hop)
    {
        let svc_clone = Arc::clone(&svc);
        let node = root_node.clone();
        let evt = event.clone();

        group.bench_function("mixed_conditions_with_resolver", |b| {
            b.iter_custom(|iters| {
                rt.block_on(async {
                    let mut total = std::time::Duration::ZERO;
                    for _ in 0..iters {
                        let mut resolver = GraphResolver::new(Arc::clone(&svc_clone));
                        let start = std::time::Instant::now();
                        black_box(cel::evaluate_conditions(
                            &[
                                "node.status == 'active'".to_string(),
                                "node.bench_type_1.status == 'active'".to_string(),
                            ],
                            &node,
                            &evt,
                            Some(&mut resolver),
                        ));
                        total += start.elapsed();
                    }
                    total
                })
            });
        });
    }

    group.finish();
}

/// Benchmark 5: Playbook activation at scale
///
/// Measures the full activation pipeline — JSON rule parsing
/// (`parse_rules_from_properties`) **and** trigger-index insertion
/// (`activate_playbook`) — as a combined metric. Both steps run inside
/// `activate_playbook()`, so this benchmark reflects the real cost of
/// onboarding N playbooks at startup.
fn bench_playbook_activation(c: &mut Criterion) {
    let mut group = c.benchmark_group("playbook/activation");
    group.sample_size(20);

    for count in [10, 100, 500, 1000] {
        let playbooks = generate_playbooks(count, "task");

        group.bench_with_input(
            BenchmarkId::new("activate_n_playbooks", count),
            &playbooks,
            |b, playbooks| {
                b.iter(|| {
                    let mut lm = PlaybookLifecycleManager::new();
                    for pb in playbooks {
                        lm.activate_playbook(pb).unwrap();
                    }
                    black_box(&lm);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark 6: Event-to-rule matching throughput
///
/// Measures the full `trigger_keys_for_event()` -> `lookup_rules()` pipeline
/// to characterize events-per-second throughput at various playbook scales.
fn bench_event_to_rule_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("playbook/event_matching");

    for count in [10, 100, 500, 1000] {
        // Setup: activate N playbooks
        let mut lm = PlaybookLifecycleManager::new();
        let playbooks = generate_playbooks(count, "task");
        for pb in &playbooks {
            lm.activate_playbook(pb).unwrap();
        }

        // Simulate a NodeCreated event
        let event = DomainEvent::NodeCreated {
            node_type: "task".to_string(),
            node_id: "bench-event-node".to_string(),
        };

        group.bench_with_input(
            BenchmarkId::new("node_created_event", count),
            &count,
            |b, _| {
                b.iter(|| {
                    let keys = trigger_keys_for_event(&event);
                    black_box(lm.lookup_rules(&keys));
                });
            },
        );

        // Simulate a PropertyChanged event (generates exact + wildcard keys)
        let property_event = DomainEvent::NodeUpdated {
            node_type: "task".to_string(),
            node_id: "bench-event-node".to_string(),
            changed_properties: vec![nodespace_core::PropertyChange {
                key: "status".to_string(),
                old_value: Some(json!("open")),
                new_value: Some(json!("done")),
            }],
        };

        group.bench_with_input(
            BenchmarkId::new("property_changed_event", count),
            &count,
            |b, _| {
                b.iter(|| {
                    let keys = trigger_keys_for_event(&property_event);
                    black_box(lm.lookup_rules(&keys));
                });
            },
        );

        // Event that matches NO playbooks (different node_type)
        let unmatched_event = DomainEvent::NodeCreated {
            node_type: "invoice".to_string(),
            node_id: "bench-event-node".to_string(),
        };

        group.bench_with_input(
            BenchmarkId::new("unmatched_event", count),
            &count,
            |b, _| {
                b.iter(|| {
                    let keys = trigger_keys_for_event(&unmatched_event);
                    black_box(lm.lookup_rules(&keys));
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_atomic_operations,
    bench_markdown_import,
    bench_occ_overhead,
    bench_batch_get,
    bench_batch_update,
    bench_bm25_search_roots,
    bench_trigger_index_lookup,
    bench_path_extraction,
    bench_graph_resolver,
    bench_cel_evaluation_e2e,
    bench_playbook_activation,
    bench_event_to_rule_matching,
);
criterion_main!(benches);
