//! Seeds a search corpus directly into a `SqliteStore`, matching the exact
//! methodology `packages/core/tests/search_result_scaling_test.rs` uses for
//! its own in-process benchmark (synthetic 768-dim vectors, no real
//! embedding model involved in seeding) — so a corpus built here is a
//! controlled, apples-to-apples match against that prior FLAT in-process
//! result. Pointing a real daemon at the resulting database and searching
//! it over gRPC isolates what an in-process benchmark can't exercise: real
//! transport latency and a real (blocking) `generate_embedding()` call on
//! the query text itself, rather than a stubbed or synchronous path.
//!
//! Usage: `cargo run --release --example seed_search_corpus -p nodespace-core -- <db_path> <count>`

use nodespace_core::db::SqliteStore;
use nodespace_core::models::{NewEmbedding, Node};
use nodespace_core::services::NodeService;
use serde_json::json;
use std::sync::Arc;

const DIM: usize = 768;

fn seeded_vector(i: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; DIM];
    v[0] = 1.0;
    v[1 + (i % (DIM - 1))] = 0.01 + (i % 100) as f32 * 0.0005;
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / norm).collect()
}

fn embedding_for(node_id: &str, i: usize) -> NewEmbedding {
    NewEmbedding {
        node_id: node_id.to_string(),
        vector: seeded_vector(i),
        model_name: Some("seed-script-synthetic".to_string()),
        chunk_index: 0,
        chunk_start: 0,
        chunk_end: 100,
        total_chunks: 1,
        content_hash: format!("hash-{i}"),
        token_count: 10,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let db_path = args
        .get(1)
        .expect("usage: seed_search_corpus <db_path> <count>");
    let count: usize = args
        .get(2)
        .expect("usage: seed_search_corpus <db_path> <count>")
        .parse()
        .expect("count must be a number");

    let mut store = Arc::new(SqliteStore::new(std::path::PathBuf::from(db_path)).await?);
    let _service = NodeService::new(&mut store).await?;

    let start = std::time::Instant::now();
    for i in 0..count {
        let content = format!(
            "Seeded search corpus document {i}. It carries several sentences of \
             body text so that hydrating a search result moves a realistic amount \
             of row data rather than a bare identifier. Topic marker: tech stack, \
             architecture, persistence, indexing, retrieval."
        );
        let node = Node::new("text".to_string(), content, json!({}));
        let id = node.id.clone();
        store.create_node(node, None, None).await?;
        store
            .upsert_embeddings(&id, vec![embedding_for(&id, i)])
            .await?;
        if i > 0 && i % 5000 == 0 {
            eprintln!(
                "seeded {i}/{count} ({:.1}s elapsed)",
                start.elapsed().as_secs_f64()
            );
        }
    }
    eprintln!(
        "done: seeded {count} nodes with embeddings in {:.1}s -> {}",
        start.elapsed().as_secs_f64(),
        db_path
    );
    Ok(())
}
