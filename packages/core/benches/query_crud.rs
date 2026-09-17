//! CRUD and query benchmarks for NodeSpace core.
//!
//! Run with: `cargo bench -p nodespace-core --bench query_crud`
//!
//! Every benchmark here is hermetic: it runs against a fresh tempfile SQLite
//! store with no network access and no model. The vector-search benchmark uses
//! precomputed pseudo-random vectors upserted directly into the store, so the
//! `vec0` KNN path is exercised without loading an embedding model.
//!
//! Coverage:
//! - CRUD: single insert, bulk insert (1000 nodes), update, delete, single get.
//! - Query: `query_nodes_simple` by type, by content substring, by `mentioned_by`
//!   (the mention-join path), and a large result set.
//! - JSON-path: `QueryService` filtering on an indexed column (`node_type`) vs a
//!   non-indexed `json_extract` property, over the same corpus; also at 1k/10k/
//!   100k scale, contrasting a `task.status` filter covered by the
//!   `idx_task_status` partial expression index against the equivalent
//!   non-indexed `text.category` filter.
//! - Vector search: `search_embeddings` (`vec0` KNN) over random unit vectors.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use nodespace_core::db::SqliteStore;
use nodespace_core::models::{NewEmbedding, NodeQuery};
use nodespace_core::services::query_service::{
    FilterOperator, FilterType, QueryDefinition, QueryFilter,
};
use nodespace_core::services::{NodeService, QueryService};
use nodespace_core::Node;
use nodespace_core::NodeUpdate;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::runtime::Runtime;

/// Embedding dimension the `vec0` mirror is created with (see `db::schema`).
const EMBEDDING_DIM: usize = 768;

/// Build a fresh store + node service backed by a tempfile database.
///
/// The returned `TempDir` owns the on-disk files and must be kept alive for as
/// long as the store is used.
async fn setup() -> (Arc<SqliteStore>, Arc<NodeService>, TempDir) {
    let temp_dir = TempDir::new().expect("create temp dir");
    let db_path = temp_dir.path().join("bench.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await.expect("open store"));
    let node_service = Arc::new(NodeService::new(&mut store).await.expect("init service"));
    (store, node_service, temp_dir)
}

/// Deterministic, dependency-free pseudo-random f32 generator (xorshift64*).
///
/// Used to fabricate embedding vectors without pulling in a model or an RNG
/// crate, keeping the vector benchmark hermetic and reproducible.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform f32 in [-1.0, 1.0).
    fn next_unit(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32; // [0, 1)
        bits * 2.0 - 1.0
    }
}

/// Produce a unit-normalized random vector of length [`EMBEDDING_DIM`].
fn random_unit_vector(rng: &mut Rng) -> Vec<f32> {
    let mut v: Vec<f32> = (0..EMBEDDING_DIM).map(|_| rng.next_unit()).collect();
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

// ---------------------------------------------------------------------------
// CRUD benchmarks
// ---------------------------------------------------------------------------

/// Single-node insert, single-node get, and version-checked update.
///
/// Each measurement runs against a store created once per `iters` batch, so the
/// timed loop reflects steady-state operation cost rather than store setup.
fn bench_crud_core(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("crud");

    group.bench_function("insert_single", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let (_store, svc, _temp) = setup().await;
                let start = std::time::Instant::now();
                for i in 0..iters {
                    let node = Node::new("text".to_string(), format!("Node {}", i), json!({}));
                    black_box(svc.create_node(node).await.expect("insert"));
                }
                start.elapsed()
            })
        });
    });

    group.bench_function("get_single", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let (_store, svc, _temp) = setup().await;
                let id = svc
                    .create_node(Node::new(
                        "text".to_string(),
                        "Lookup target".to_string(),
                        json!({}),
                    ))
                    .await
                    .expect("insert");

                let start = std::time::Instant::now();
                for _ in 0..iters {
                    black_box(svc.get_node(&id).await.expect("get"));
                }
                start.elapsed()
            })
        });
    });

    group.bench_function("update_versioned", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let (_store, svc, _temp) = setup().await;
                let id = svc
                    .create_node(Node::new(
                        "text".to_string(),
                        "Update target".to_string(),
                        json!({}),
                    ))
                    .await
                    .expect("insert");

                let start = std::time::Instant::now();
                for i in 0..iters {
                    let node = svc.get_node(&id).await.expect("get").expect("exists");
                    svc.update_node(
                        &id,
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
                    .expect("update");
                }
                start.elapsed()
            })
        });
    });

    group.finish();
}

/// Bulk insert of 1000 nodes via `bulk_create`.
fn bench_bulk_insert(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("crud");
    group.sample_size(10);

    group.bench_function("bulk_insert_1000", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let mut total = std::time::Duration::ZERO;
                for _ in 0..iters {
                    let (_store, svc, _temp) = setup().await;
                    let nodes: Vec<Node> = (0..1000)
                        .map(|i| {
                            Node::new("text".to_string(), format!("Bulk node {}", i), json!({}))
                        })
                        .collect();

                    let start = std::time::Instant::now();
                    black_box(svc.bulk_create(nodes).await.expect("bulk insert"));
                    total += start.elapsed();
                }
                total
            })
        });
    });

    group.finish();
}

/// Single-node delete (version-checked, subtree-atomic).
///
/// Nodes are created untimed, then deletion of each is timed, so the reported
/// figure is per-delete cost only.
fn bench_delete(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("crud");

    group.bench_function("delete_single", |b| {
        b.iter_custom(|iters| {
            rt.block_on(async {
                let (_store, svc, _temp) = setup().await;

                // Pre-create `iters` nodes (untimed).
                let mut ids = Vec::with_capacity(iters as usize);
                for i in 0..iters {
                    let id = svc
                        .create_node(Node::new(
                            "text".to_string(),
                            format!("Delete target {}", i),
                            json!({}),
                        ))
                        .await
                        .expect("insert");
                    ids.push(id);
                }

                let start = std::time::Instant::now();
                for id in &ids {
                    // version 1: freshly created, never updated.
                    black_box(svc.delete_node(id, 1).await.expect("delete"));
                }
                start.elapsed()
            })
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Query benchmarks (`query_nodes_simple`)
// ---------------------------------------------------------------------------

/// Number of nodes in the query corpus.
const QUERY_CORPUS: usize = 500;

/// Seed a corpus of `text` nodes and return the service (+ temp dir to keep the
/// DB alive). Half the nodes contain the token "needle" in their content.
async fn seed_query_corpus() -> (Arc<NodeService>, TempDir) {
    let (_store, svc, temp) = setup().await;
    let nodes: Vec<Node> = (0..QUERY_CORPUS)
        .map(|i| {
            let content = if i % 2 == 0 {
                format!("Document {} discussing the needle topic", i)
            } else {
                format!("Document {} about unrelated matters", i)
            };
            Node::new("text".to_string(), content, json!({}))
        })
        .collect();
    svc.bulk_create(nodes).await.expect("seed corpus");
    (svc, temp)
}

/// `query_nodes_simple` across type filter, content substring, and a large
/// result set. Corpus is built once and shared across all cases in the group.
fn bench_query_simple(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let (svc, _temp) = rt.block_on(seed_query_corpus());

    let mut group = c.benchmark_group("query");
    group.sample_size(30);

    group.bench_function("by_type", |b| {
        b.iter(|| {
            rt.block_on(async {
                let q = NodeQuery::by_type("text".to_string()).with_limit(100);
                black_box(svc.query_nodes_simple(q).await.expect("query"))
            })
        });
    });

    group.bench_function("content_contains", |b| {
        b.iter(|| {
            rt.block_on(async {
                let q = NodeQuery::content_contains("needle".to_string()).with_limit(100);
                black_box(svc.query_nodes_simple(q).await.expect("query"))
            })
        });
    });

    group.bench_function("large_result_set", |b| {
        b.iter(|| {
            rt.block_on(async {
                let q = NodeQuery::by_type("text".to_string()).with_limit(QUERY_CORPUS);
                black_box(svc.query_nodes_simple(q).await.expect("query"))
            })
        });
    });

    group.finish();
}

/// `query_nodes_simple` on the `mentioned_by` path at increasing mention counts.
///
/// A single target node is mentioned by N sibling roots; the benchmark measures
/// resolving "who mentions the target?" via the mention join.
fn bench_query_mentioned_by(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("query/mentioned_by");
    group.sample_size(20);

    for count in [10usize, 100, 500] {
        // Build corpus once per size: 1 target + `count` mentioning roots.
        let (svc, _temp, target_id) = rt.block_on(async {
            let (_store, svc, temp) = setup().await;
            let target_id = svc
                .create_node(Node::new(
                    "text".to_string(),
                    "Mention target".to_string(),
                    json!({}),
                ))
                .await
                .expect("target");

            for i in 0..count {
                let src = svc
                    .create_node(Node::new(
                        "text".to_string(),
                        format!("Mentioning root {}", i),
                        json!({}),
                    ))
                    .await
                    .expect("source");
                svc.create_mention(&src, &target_id).await.expect("mention");
            }
            (svc, temp, target_id)
        });

        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let q = NodeQuery::mentioned_by(target_id.clone()).with_limit(count);
                    black_box(svc.query_nodes_simple(q).await.expect("query"))
                })
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// JSON-path benchmarks (indexed column/property vs non-indexed property)
// ---------------------------------------------------------------------------

/// `QueryService` filtering on the indexed `node_type` column versus a
/// non-indexed `json_extract` property, over the same corpus.
///
/// Both queries scan the same `text` corpus whose namespaced properties carry a
/// `category` field; the property path has no expression index, so this
/// contrasts an indexed lookup against a JSON full scan.
fn bench_jsonpath_query(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");

    let (store, _temp) = rt.block_on(async {
        let (store, svc, temp) = setup().await;
        let nodes: Vec<Node> = (0..QUERY_CORPUS)
            .map(|i| {
                let category = if i % 2 == 0 { "alpha" } else { "beta" };
                Node::new(
                    "text".to_string(),
                    format!("Categorized node {}", i),
                    json!({ "text": { "category": category } }),
                )
            })
            .collect();
        svc.bulk_create(nodes).await.expect("seed");
        (store, temp)
    });

    let query_service = QueryService::new(Arc::clone(&store));

    let mut group = c.benchmark_group("jsonpath");
    group.sample_size(30);

    // Indexed: filter purely by node_type (uses idx_node_type).
    group.bench_function("indexed_node_type", |b| {
        b.iter(|| {
            rt.block_on(async {
                let q = QueryDefinition {
                    target_type: "text".to_string(),
                    filters: vec![],
                    sorting: None,
                    limit: Some(QUERY_CORPUS),
                };
                black_box(query_service.execute(&q).await.expect("query"))
            })
        });
    });

    // Non-indexed: filter on a JSON property via json_extract (full scan).
    group.bench_function("non_indexed_property", |b| {
        b.iter(|| {
            rt.block_on(async {
                let q = QueryDefinition {
                    target_type: "text".to_string(),
                    filters: vec![QueryFilter {
                        filter_type: FilterType::Property,
                        operator: FilterOperator::Equals,
                        property: Some("category".to_string()),
                        value: Some(json!("alpha")),
                        case_sensitive: None,
                        relationship_type: None,
                        node_id: None,
                    }],
                    sorting: None,
                    limit: Some(QUERY_CORPUS),
                };
                black_box(query_service.execute(&q).await.expect("query"))
            })
        });
    });

    group.finish();
}

/// Seed a corpus of `task` nodes (half `status: "open"`, half `status: "done"`)
/// and return the store + service (+ temp dir to keep the DB alive).
async fn seed_task_corpus(count: usize) -> (Arc<SqliteStore>, Arc<NodeService>, TempDir) {
    let (store, svc, temp) = setup().await;
    let nodes: Vec<Node> = (0..count)
        .map(|i| {
            let status = if i % 2 == 0 { "open" } else { "done" };
            Node::new(
                "task".to_string(),
                format!("Task {}", i),
                json!({ "task": { "status": status } }),
            )
        })
        .collect();
    svc.bulk_create(nodes).await.expect("seed task corpus");
    (store, svc, temp)
}

/// `task.status` filter (the `idx_task_status` partial expression
/// index) versus the equivalent non-indexed `text.category` filter, at
/// increasing corpus sizes, quantifying the index's before/after win as the
/// table grows.
fn bench_jsonpath_indexed_vs_non_indexed_at_scale(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");
    let mut group = c.benchmark_group("jsonpath/scale");
    group.sample_size(10);

    for count in [1_000usize, 10_000, 100_000] {
        let (task_store, _task_temp) = rt.block_on(async {
            let (store, _svc, temp) = seed_task_corpus(count).await;
            (store, temp)
        });
        let task_query_service = QueryService::new(Arc::clone(&task_store));

        group.bench_with_input(
            BenchmarkId::new("indexed_task_status", count),
            &count,
            |b, &count| {
                b.iter(|| {
                    rt.block_on(async {
                        let q = QueryDefinition {
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
                            limit: Some(count),
                        };
                        black_box(task_query_service.execute(&q).await.expect("query"))
                    })
                });
            },
        );

        let (text_store, _text_temp) = rt.block_on(async {
            let (store, svc, temp) = setup().await;
            let nodes: Vec<Node> = (0..count)
                .map(|i| {
                    let category = if i % 2 == 0 { "alpha" } else { "beta" };
                    Node::new(
                        "text".to_string(),
                        format!("Categorized node {}", i),
                        json!({ "text": { "category": category } }),
                    )
                })
                .collect();
            svc.bulk_create(nodes).await.expect("seed");
            (store, temp)
        });
        let text_query_service = QueryService::new(Arc::clone(&text_store));

        group.bench_with_input(
            BenchmarkId::new("non_indexed_category", count),
            &count,
            |b, &count| {
                b.iter(|| {
                    rt.block_on(async {
                        let q = QueryDefinition {
                            target_type: "text".to_string(),
                            filters: vec![QueryFilter {
                                filter_type: FilterType::Property,
                                operator: FilterOperator::Equals,
                                property: Some("category".to_string()),
                                value: Some(json!("alpha")),
                                case_sensitive: None,
                                relationship_type: None,
                                node_id: None,
                            }],
                            sorting: None,
                            limit: Some(count),
                        };
                        black_box(text_query_service.execute(&q).await.expect("query"))
                    })
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Vector-search benchmark (model-free)
// ---------------------------------------------------------------------------

/// Nodes seeded with fabricated embedding vectors.
const VECTOR_CORPUS: usize = 500;

/// `vec0` KNN search over random unit vectors upserted directly into the store.
///
/// No embedding model is loaded: vectors are generated deterministically and
/// written via `upsert_embeddings`, then the store's `search_embeddings` KNN
/// path is timed with a random query vector. This keeps the default
/// `cargo bench` model-free while still exercising the real vector-search SQL.
fn bench_vector_search(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime");

    let (store, _temp) = rt.block_on(async {
        let (store, svc, temp) = setup().await;
        let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);

        for i in 0..VECTOR_CORPUS {
            let id = svc
                .create_node(Node::new(
                    "text".to_string(),
                    format!("Vector node {}", i),
                    json!({}),
                ))
                .await
                .expect("node");
            let vector = random_unit_vector(&mut rng);
            let embedding = NewEmbedding::single_chunk(id, vector, format!("hash-{}", i), 32, 8);
            store
                .upsert_embeddings(&embedding.node_id.clone(), vec![embedding])
                .await
                .expect("upsert embedding");
        }
        (store, temp)
    });

    let mut group = c.benchmark_group("vector");
    group.sample_size(30);

    let query_vector = {
        let mut rng = Rng::new(0x1234_5678_9ABC_DEF0);
        random_unit_vector(&mut rng)
    };

    for k in [10i64, 50] {
        group.bench_with_input(BenchmarkId::new("knn_search", k), &k, |b, &k| {
            b.iter(|| {
                rt.block_on(async {
                    black_box(
                        store
                            .search_embeddings(&query_vector, k, Some(-1.0))
                            .await
                            .expect("vector search"),
                    )
                })
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_crud_core,
    bench_bulk_insert,
    bench_delete,
    bench_query_simple,
    bench_query_mentioned_by,
    bench_jsonpath_query,
    bench_jsonpath_indexed_vs_non_indexed_at_scale,
    bench_vector_search,
);
criterion_main!(benches);
