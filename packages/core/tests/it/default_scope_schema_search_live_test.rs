//! Live, real-embedding-model test: a search that names no type answers with
//! the user's notes, not with the built-in schemas.
//!
//! Every schema is embedded (its name and its fields) so that skill and schema
//! retrieval can find a type by meaning. Those vectors sit in the same index
//! general search queries, and a built-in type's name or field label
//! ("Checkbox", "Horizontal Line") is close to many ordinary queries, so with
//! the built-in schemas in the default scope a search for a note came back as
//! a list of type definitions. The default scope leaves the built-in schemas
//! out and keeps user-defined ones; naming the `schema` type, and skill
//! retrieval, still find them all.
//!
//! Ignored by default: it loads the real embedding model from the standard
//! NodeSpace catalog path and skips when the model is not on disk. Run it
//! explicitly:
//!
//! ```text
//! .tools/bin/cargo-nextest nextest run -p nodespace-core --test it \
//!     default_scope_schema_search_live_test:: --run-ignored only --no-capture
//! ```

use nodespace_core::db::SqliteStore;
use nodespace_core::models::Node;
use nodespace_core::ops::search_ops::{search_semantic, SearchSemanticInput};
use nodespace_core::ops::skill_ops::{find_skills, FindSkillsInput};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::{NodeAccessor, NodeEmbeddingService, NodeService};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

/// Fresh database and a real, initialized embedding model. `None` (the test
/// skips) when the model is not on disk.
async fn test_env() -> Option<(Arc<NodeEmbeddingService>, Arc<NodeService>, TempDir)> {
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
            "SKIP default_scope_schema_search_live_test: nomic-embed-text-v1.5 model not found on disk"
        );
        return None;
    }
    let nlp = Arc::new(nlp);

    let node_accessor: Arc<dyn NodeAccessor> = node_service.clone();
    let embedding_service = Arc::new(NodeEmbeddingService::new(
        nlp,
        store.clone(),
        node_accessor,
        node_service.behaviors().clone(),
    ));
    Some((embedding_service, node_service, temp_dir))
}

/// A root text note, embedded.
async fn embedded_note(
    embedding_service: &NodeEmbeddingService,
    node_service: &NodeService,
    content: &str,
) -> Node {
    let node = Node::new("text".to_string(), content.to_string(), json!({}));
    node_service
        .create_node(node.clone())
        .await
        .expect("note must create");
    embedding_service
        .embed_root_node(&node.id)
        .await
        .expect("note must embed");
    node
}

fn query(text: &str, node_types: Option<Vec<String>>) -> SearchSemanticInput {
    SearchSemanticInput {
        query: text.to_string(),
        threshold: None,
        limit: None,
        collection_id: None,
        collection: None,
        exclude_collections: None,
        include_markdown: Some(0),
        include_archived: None,
        scope: None,
        node_types,
        property_filters: None,
        include_edges: None,
        graph_boost: None,
        include_title_matches: None,
    }
}

/// The ranked results as `(type, content)`, for assertion messages.
fn ranked(nodes: &[Node]) -> Vec<(&str, &str)> {
    nodes
        .iter()
        .map(|n| (n.node_type.as_str(), n.content.as_str()))
        .collect()
}

/// Whether `node` is one of the schemas core seeds.
fn is_built_in_schema(node: &Node) -> bool {
    node.node_type == "schema" && node.properties["isCore"] == true
}

#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn untyped_search_leaves_built_in_schemas_out_and_keeps_user_schemas() {
    let Some((embedding_service, node_service, _temp_dir)) = test_env().await else {
        return;
    };

    let created = handle_create_schema(
        &node_service,
        json!({
            "name": "Venue",
            "description": "A place that hosts an event, with how many people it holds",
            "fields": [
                { "name": "capacity", "type": "number" },
                { "name": "address", "type": "text" }
            ]
        }),
    )
    .await
    .expect("schema must create");
    let venue_schema = created["schemaId"]
        .as_str()
        .expect("schemaId in create_schema output")
        .to_string();

    // Every schema embedded, core and user-defined, as the queue leaves them
    // once its debounce window has passed.
    let schemas = node_service
        .get_all_schemas()
        .await
        .expect("schemas must list");
    assert!(schemas.len() > 10, "core schemas are seeded");
    for schema in &schemas {
        embedding_service
            .embed_root_node(&schema.envelope.id)
            .await
            .expect("schema must embed");
    }

    let hall_nine = embedded_note(
        &embedding_service,
        &node_service,
        "Hall Nine is booked for the team offsite on the 14th",
    )
    .await;
    let budget = embedded_note(
        &embedding_service,
        &node_service,
        "Marketing budget: split spend between paid social, events and content",
    )
    .await;
    embedded_note(
        &embedding_service,
        &node_service,
        "Groceries: apples, oat milk, coffee beans",
    )
    .await;

    // A search that names no type: the note it is about comes first, and no
    // built-in schema is returned.
    for (text, expected) in [("Hall Nine", &hall_nine), ("marketing budget", &budget)] {
        let output = search_semantic(&node_service, &embedding_service, query(text, None))
            .await
            .expect("search must succeed");
        let results = ranked(&output.matched_nodes);
        eprintln!("untyped {text:?}: {results:?}");
        assert_eq!(
            output.matched_nodes.first().map(|n| n.id.as_str()),
            Some(expected.id.as_str()),
            "{text:?} must rank the user's note first, got {results:?}"
        );
        assert!(
            !output.matched_nodes.iter().any(is_built_in_schema),
            "a search that names no type must return no built-in schema, got {results:?}"
        );
    }

    // A user-defined schema stays in the default scope and is found by meaning.
    let output = search_semantic(
        &node_service,
        &embedding_service,
        query("venue capacity", None),
    )
    .await
    .expect("search must succeed");
    let results = ranked(&output.matched_nodes);
    eprintln!("untyped \"venue capacity\": {results:?}");
    assert!(
        output.matched_nodes.iter().any(|n| n.id == venue_schema),
        "a search that names no type must still return the user-defined Venue schema, got {results:?}"
    );
    assert!(
        !output.matched_nodes.iter().any(is_built_in_schema),
        "a search that names no type must return no built-in schema, got {results:?}"
    );

    // A search that names `schema` gets schemas, built-in and user-defined.
    for (text, expected) in [
        ("checkbox", "checkbox"),
        ("venue capacity", venue_schema.as_str()),
    ] {
        let output = search_semantic(
            &node_service,
            &embedding_service,
            query(text, Some(vec!["schema".to_string()])),
        )
        .await
        .expect("search must succeed");
        let results = ranked(&output.matched_nodes);
        eprintln!("--type schema {text:?}: {results:?}");
        assert!(
            output.matched_nodes.iter().any(|n| n.id == expected),
            "{text:?} with the schema type named must return the `{expected}` schema, got {results:?}"
        );
        assert!(
            output.matched_nodes.iter().all(|n| n.node_type == "schema"),
            "naming `schema` returns only schemas, got {results:?}"
        );
    }

    // Skill and schema retrieval, the fetch-by-task path, still finds the
    // user-defined type by meaning. The query does not name the type, so its
    // name-match backstop cannot be what finds it.
    let output = find_skills(
        &embedding_service,
        &node_service,
        FindSkillsInput {
            query: "find a place to hold the team offsite and check how many people it holds"
                .to_string(),
            limit: Some(5),
        },
    )
    .await
    .expect("find_skills must succeed");
    assert!(
        output.skills.iter().any(|s| {
            s.get("id").and_then(|v| v.as_str()) == Some(venue_schema.as_str())
                && s.get("kind").and_then(|v| v.as_str()) == Some("schema")
        }),
        "skill retrieval must still find the Venue schema: {:?}",
        output.skills
    );
}
