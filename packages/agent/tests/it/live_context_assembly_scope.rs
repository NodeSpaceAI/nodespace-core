//! Live, real-embedding-model test: the context file a PTY session is given
//! lists the user's knowledge near its seed nodes, and no system content.
//!
//! Skills, tools and every schema are embedded so retrieval can find them by
//! meaning. Those vectors sit in the index the assembler's neighbour search
//! reads, and a built-in type's name ("Checkbox", "Task") or a skill's
//! description is close to many ordinary notes, so a search with no scope
//! returns them as "related context" for a seed note.
//!
//! Ignored by default — loads a real embedding model from the standard
//! NodeSpace catalog path and skips when the model is not on disk. Run
//! explicitly:
//!
//! ```text
//! .tools/bin/cargo-nextest nextest run -p nodespace-agent --test it \
//!     live_context_assembly_scope:: --run-ignored only --no-capture
//! ```

use std::collections::HashSet;
use std::sync::Arc;

use nodespace_agent::agent_catalog::context_assembly::GraphContextAssembler;
use nodespace_agent::skill_pipeline::{seed_skill_nodes, seed_tool_nodes};
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::models::Node;
use nodespace_core::services::{NodeAccessor, NodeEmbeddingService, NodeService, SearchScope};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::RwLock;

/// A root text note.
async fn note(node_service: &NodeService, content: &str) -> Node {
    let node = Node::new("text".to_string(), content.to_string(), json!({}));
    node_service
        .create_node(node.clone())
        .await
        .expect("note must create");
    node
}

#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn related_context_holds_the_users_knowledge_and_no_system_content() {
    let temp_dir = TempDir::new().expect("tempdir");
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await.expect("store must open"));
    let node_service = Arc::new(
        NodeService::new(&mut store)
            .await
            .expect("node service must init"),
    );

    let mut nlp = EmbeddingService::new(EmbeddingConfig::default()).expect("config must validate");
    if nlp.initialize().is_err() || !nlp.is_initialized() {
        eprintln!(
            "SKIP live_context_assembly_scope: nomic-embed-text-v1.5 model not found on disk"
        );
        return;
    }
    let node_accessor: Arc<dyn NodeAccessor> = node_service.clone();
    let embedding_service = Arc::new(NodeEmbeddingService::new(
        Arc::new(nlp),
        store.clone(),
        node_accessor,
        node_service.behaviors().clone(),
    ));

    let groups: Vec<_> = seed_skill_nodes()
        .iter()
        .chain(seed_tool_nodes().iter())
        .map(|t| prepare_nodes_from_template(t).expect("template must parse"))
        .collect();
    node_service
        .seed_nodes_from_templates(groups)
        .await
        .expect("seeding the built-in skills and tools must succeed");

    let seed = note(
        &node_service,
        "Create a task with a checkbox for every item on the packing list",
    )
    .await;
    let related = note(
        &node_service,
        "Packing list for the offsite: one checkbox per item, tick it when packed",
    )
    .await;

    // Embed what the write paths queued (the built-in schemas, the skills and
    // the tools) and the two notes.
    for node in [&seed, &related] {
        embedding_service
            .embed_root_node(&node.id)
            .await
            .expect("note must embed");
    }
    let queued = store
        .get_stale_embedding_root_ids(None, 0, 3)
        .await
        .expect("the queue must read");
    for id in queued {
        embedding_service
            .embed_root_node(&id)
            .await
            .unwrap_or_else(|e| panic!("queued root {id} must embed: {e}"));
    }

    // What a search with no scope returns for this seed. The test proves
    // nothing unless system content really is among the seed's nearest
    // neighbours.
    let no_user_types = HashSet::new();
    let unscoped = embedding_service
        .semantic_search_nodes(&seed.content, 50, 0.3, None, false)
        .await
        .expect("search must succeed");
    let system: Vec<&Node> = unscoped
        .iter()
        .map(|(node, _)| node)
        .filter(|node| {
            !NodeEmbeddingService::matches_scope(node, &SearchScope::Knowledge, &no_user_types)
        })
        .collect();
    let nearest_five: Vec<&str> = unscoped
        .iter()
        .take(5)
        .map(|(n, _)| n.id.as_str())
        .collect();
    eprintln!(
        "unscoped: {:?}",
        unscoped
            .iter()
            .map(|(n, score)| (n.node_type.as_str(), n.id.as_str(), *score))
            .collect::<Vec<_>>()
    );
    assert!(
        system.iter().any(|n| nearest_five.contains(&n.id.as_str())),
        "the seed must have system content among its five nearest neighbours, got {nearest_five:?}"
    );

    let context = GraphContextAssembler::new(
        node_service.clone(),
        Arc::new(RwLock::new(Some(embedding_service.clone()))),
    )
    .with_seed_nodes(vec![seed.id.clone()])
    .assemble_context()
    .await
    .expect("context must assemble");
    eprintln!("{context}");

    assert!(
        context.contains(&format!("(id: {})", related.id)),
        "the related note must be in the context"
    );
    for node in system {
        assert!(
            !context.contains(&format!("(id: {})", node.id)),
            "{} `{}` is system content and must not be in the context",
            node.node_type,
            node.id
        );
    }
}
