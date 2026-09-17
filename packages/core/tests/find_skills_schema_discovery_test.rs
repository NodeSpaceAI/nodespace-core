//! Live, real-embedding-model integration test for `find_skills`'s schema
//! discovery path (see ADR-038's schema-discovery amendment): a schema with
//! no hand-authored skill describing it must still be discoverable through
//! `find_skills`, and a schema created moments ago (inside the ~30s
//! embedding-debounce window) must still be recoverable by name via the
//! lexical backstop.
//!
//! Every DB in this file seeds ZERO skill nodes — not just a schema with no
//! *matching* skill, but a registry with no skill at all — so a passing
//! result here cannot be explained by an incidental skill match riding
//! along; the schema-typed result is the only thing that could have
//! produced a hit.
//!
//! Ignored by default — loads a real embedding model from the standard
//! NodeSpace catalog path. Run explicitly:
//!
//! ```text
//! cargo test -p nodespace-core --test find_skills_schema_discovery_test -- --ignored --nocapture
//! ```

use nodespace_core::db::SqliteStore;
use nodespace_core::ops::skill_ops::{find_skills, FindSkillsInput};
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::{NodeAccessor, NodeEmbeddingService, NodeService};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

/// Fresh DB + a real, initialized embedding model. Returns `None` (test
/// skips cleanly) when the model isn't on disk, matching
/// `nodespace_agent`'s `live_skill_retrieval_stability.rs` convention for
/// this exact class of test.
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
            "SKIP find_skills_schema_discovery_test: nomic-embed-text-v1.5 model not found on disk"
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

/// The core acceptance criterion: a schema with no associated hand-authored
/// skill is discoverable via `find_skills` once its embedding lands.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn schema_with_no_skill_is_discoverable_once_embedded() {
    let Some((embedding_service, node_service, _temp_dir)) = test_env().await else {
        return;
    };

    let created = handle_create_schema(
        &node_service,
        json!({
            "name": "Sprint",
            "description": "A time-boxed iteration of work items tracked to completion",
            "fields": [
                { "name": "start_date", "type": "date" },
                { "name": "velocity_points", "type": "number" }
            ]
        }),
    )
    .await
    .expect("schema must create");
    let schema_id = created["schemaId"]
        .as_str()
        .expect("schemaId in create_schema output")
        .to_string();

    embedding_service
        .embed_root_node(&schema_id)
        .await
        .expect("schema root must embed");

    let output = find_skills(
        &embedding_service,
        &node_service,
        FindSkillsInput {
            query: "plan the next sprint for the team and track velocity".to_string(),
            limit: Some(5),
        },
    )
    .await
    .expect("find_skills must succeed");

    let hit = output
        .skills
        .iter()
        .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(schema_id.as_str()));
    assert!(
        hit.is_some(),
        "a schema with no associated skill must be discoverable via find_skills: {:?}",
        output.skills
    );
    let hit = hit.unwrap();

    assert_eq!(
        hit.get("kind").and_then(|v| v.as_str()),
        Some("schema"),
        "must be distinguishable in shape from a skill result: {hit:?}"
    );
    assert_eq!(
        hit.get("tools").and_then(|v| v.as_array()).map(Vec::len),
        Some(0),
        "a schema result carries no tool_whitelist: {hit:?}"
    );
    assert_eq!(
        hit.get("instructions").and_then(|v| v.as_str()),
        Some(""),
        "a schema has no guidance subtree to render as instructions: {hit:?}"
    );

    let metadata = hit
        .get("schema_metadata")
        .and_then(|v| v.as_array())
        .expect("schema_metadata must be an array");
    assert_eq!(
        metadata.len(),
        1,
        "schema_metadata must carry exactly the one matched schema, not the \
         broader unscoped fallback a matched skill uses: {metadata:?}"
    );
    assert_eq!(
        metadata[0].get("type_id").and_then(|v| v.as_str()),
        Some(schema_id.as_str())
    );
    let field_names: Vec<&str> = metadata[0]["fields"]
        .as_array()
        .expect("fields array")
        .iter()
        .filter_map(|f| f.get("name").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(field_names, vec!["start_date", "velocity_points"]);

    // The schema's own top-level description (authored as markdown, stored
    // as a child subtree, rendered via the same `render_schema_description`/
    // `schema_description_cache` machinery the skill-riding case uses) must
    // reach a `kind: "schema"` result too, not just field-level shape —
    // otherwise this discovery path would silently degrade to less content
    // than the same schema carries when it happens to ride along with a
    // matched skill instead.
    assert_eq!(
        metadata[0].get("description").and_then(|v| v.as_str()),
        Some("A time-boxed iteration of work items tracked to completion"),
        "the schema's own description subtree must be rendered into \
         schema_metadata for a kind: \"schema\" result: {metadata:?}"
    );
}

/// Debounce-window mitigation, tested end-to-end rather than only at the
/// pure-function level (`append_named_schema_candidates`'s unit tests cover
/// the mechanism in isolation): a schema created moments ago — deliberately
/// never embedded in this test, standing in for the ~30s window before its
/// embedding job lands — is still recovered when the query names it
/// outright.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn unembedded_schema_named_in_the_query_is_still_recovered() {
    let Some((embedding_service, node_service, _temp_dir)) = test_env().await else {
        return;
    };

    let created = handle_create_schema(
        &node_service,
        json!({
            "name": "Feature Writeup",
            "fields": [ { "name": "summary", "type": "text" } ]
        }),
    )
    .await
    .expect("schema must create");
    let schema_id = created["schemaId"]
        .as_str()
        .expect("schemaId in create_schema output")
        .to_string();

    // Deliberately no `embedding_service.embed_root_node(...)` call here —
    // this schema has no embedding at all, the same state it would be in
    // during the live debounce window right after `create_schema` returns.

    let output = find_skills(
        &embedding_service,
        &node_service,
        FindSkillsInput {
            query: "start a feature writeup for the new onboarding flow".to_string(),
            limit: Some(5),
        },
    )
    .await
    .expect("find_skills must succeed");

    let hit = output
        .skills
        .iter()
        .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(schema_id.as_str()));
    assert!(
        hit.is_some(),
        "an unembedded, freshly-created schema named outright in the query must \
         still be recovered via the lexical backstop — without it, this schema \
         is indistinguishable through this discovery path from one that does \
         not exist: {:?}",
        output.skills
    );
    assert_eq!(
        hit.unwrap().get("kind").and_then(|v| v.as_str()),
        Some("schema")
    );
}

/// Negative control for the debounce test above: an unembedded schema that
/// the query does NOT name is not recovered by the lexical backstop (it
/// only recovers a schema named outright, not every unembedded schema) —
/// confirms the recovery above is really about naming, not merely "any
/// unembedded schema shows up regardless of the query."
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn unembedded_schema_not_named_in_the_query_is_not_recovered() {
    let Some((embedding_service, node_service, _temp_dir)) = test_env().await else {
        return;
    };

    let created = handle_create_schema(
        &node_service,
        json!({
            "name": "Feature Writeup",
            "fields": [ { "name": "summary", "type": "text" } ]
        }),
    )
    .await
    .expect("schema must create");
    let schema_id = created["schemaId"]
        .as_str()
        .expect("schemaId in create_schema output")
        .to_string();

    let output = find_skills(
        &embedding_service,
        &node_service,
        FindSkillsInput {
            query: "what's the weather like this weekend".to_string(),
            limit: Some(5),
        },
    )
    .await
    .expect("find_skills must succeed");

    assert!(
        !output
            .skills
            .iter()
            .any(|s| s.get("id").and_then(|v| v.as_str()) == Some(schema_id.as_str())),
        "an unnamed, unembedded schema must not appear — the lexical backstop \
         is a targeted recovery, not a fallback that lists every schema: {:?}",
        output.skills
    );
}
