//! Repeated-prompt retrieval-stability check for Stage-2 skill retrieval.
//!
//! Closes a test-coverage gap: every prior skill-routing test either mocks
//! retrieval out entirely (`prompt_assembly_snapshot.rs`) or drives the full
//! Stage-1 LLM (`live_stage1_golden_prompts.rs`), so nothing exercised live
//! Stage-2 retrieval — `find_skills` against the real seeded registry with a
//! real embedding model. That gap hid a real defect: `find_skills` used to
//! call the hybrid BM25+KNN `semantic_search_nodes`, whose result order is a
//! hard tier partition (BM25∩KNN, then KNN-only, then BM25-only) rather than
//! a blended score — so a skill whose guidance markdown happened to share a
//! keyword with the query could rank ahead of a much stronger semantic match
//! purely from that tier membership. `find_skills` now calls
//! `semantic_search_nodes_of_type` (pure KNN over `skill`-typed roots)
//! instead, which this test guards against regressing.
//!
//! This test embeds the full `seed_skill_nodes()` registry once with the real
//! `nomic-embed-text-v1.5` model, then calls `find_skills` N times per query
//! and asserts the scenario-8a-critical skill clears `RETRIEVAL_TOP_K` on
//! every rep — not just on average.
//!
//! Ignored by default — loads a real embedding model from the standard
//! NodeSpace catalog path. Run explicitly:
//!
//! ```text
//! cargo test -p nodespace-agent --test live_skill_retrieval_stability -- --ignored --nocapture
//! ```

use std::sync::Arc;

use nodespace_agent::local_agent::routing::RETRIEVAL_TOP_K;
use nodespace_agent::skill_pipeline::seed_skill_nodes;
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::ops::skill_ops::{find_skills, FindSkillsInput};
use nodespace_core::services::node_service::CreateNodeParams;
use nodespace_core::services::{
    InsertPositionOwned, NodeAccessor, NodeEmbeddingService, NodeService,
};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use tempfile::TempDir;

/// Number of identical reps per query. Matches the issue's own reproduction
/// (`--runs 3`) as a floor — the observed defect rate was 1-in-3, so fewer
/// reps risks a clean run proving nothing.
const REPS: usize = 5;

/// Seed the skill registry into a fresh DB and embed every skill root with a
/// real model. Returns `None` if the embedding model isn't on disk, so the
/// test can skip cleanly rather than fail on an unrelated machine.
async fn seed_and_embed() -> Option<(Arc<NodeEmbeddingService>, Arc<NodeService>, TempDir)> {
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
            "SKIP live_skill_retrieval_stability: nomic-embed-text-v1.5 model not found on disk"
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

    for tmpl in seed_skill_nodes() {
        let prepared = prepare_nodes_from_template(&tmpl).expect("template must parse");
        // Pre-assigned ids from `prepare_nodes_from_template` are reused verbatim
        // (`CreateNodeParams::id`), so parent_id references need no remapping.
        for p in &prepared {
            node_service
                .create_node_with_parent(CreateNodeParams {
                    id: Some(p.id.clone()),
                    node_type: p.node_type.clone(),
                    content: p.content.clone(),
                    parent_id: p.parent_id.clone(),
                    position: InsertPositionOwned::End,
                    properties: p.properties.clone(),
                    lifecycle_status: None,
                })
                .await
                .expect("skill node must insert");
        }
        let root_id = &prepared[0].id;
        embedding_service
            .embed_root_node(root_id)
            .await
            .expect("skill root must embed");
    }

    Some((embedding_service, node_service, temp_dir))
}

/// Run `find_skills` `REPS` times for `query` and return, for each rep, the
/// ranked candidate names (score-descending, as `find_skills`/`semantic_search_nodes`
/// already returns them).
async fn repeated_rankings(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    query: &str,
) -> Vec<Vec<String>> {
    let mut rankings = Vec::with_capacity(REPS);
    for _ in 0..REPS {
        let output = find_skills(
            embedding_service,
            node_service,
            FindSkillsInput {
                query: query.to_string(),
                limit: Some(RETRIEVAL_TOP_K),
            },
        )
        .await
        .expect("find_skills must succeed");

        let names: Vec<String> = output
            .skills
            .iter()
            .map(|s| {
                s.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        rankings.push(names);
    }
    rankings
}

/// Scenario 8a from the dev-workflow matrix: a schema-defining prompt that
/// also carries attribution language ("who made each one"), which was found
/// to intermittently rank Schema Creation out of the top-N against an
/// identical query.
///
/// This asserts the fix's acceptance criterion directly: across `REPS`
/// identical calls, "Schema Creation" clears `RETRIEVAL_TOP_K` every time —
/// not on average, since an intermittent defect is exactly what an averaged
/// or single-run assertion cannot catch.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn scenario_8a_routes_schema_creation_every_rep() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };

    let query = "Start keeping the calls we make on how the system is built, and who made each one";
    let rankings = repeated_rankings(&embedding_service, &node_service, query).await;

    let mut misses = Vec::new();
    for (i, ranked) in rankings.iter().enumerate() {
        eprintln!("rep {}: {:?}", i + 1, ranked);
        if !ranked.iter().any(|n| n == "Schema Creation") {
            misses.push(i + 1);
        }
    }

    assert!(
        misses.is_empty(),
        "Schema Creation dropped out of the top-{RETRIEVAL_TOP_K} on rep(s) {misses:?} of {REPS} \
         for query {query:?} — rankings were: {rankings:?}"
    );
}

/// Diagnostic: dump which skill roots BM25 matches directly for the literal
/// scenario 8a query, to see whether Tier 1 (BM25 ∩ KNN) is what's promoting
/// Bulk Import / Research & Search ahead of Schema Creation's higher raw
/// embedding score — `semantic_search` orders tier1 ++ tier2 ++ tier3, so a
/// Tier-1 keyword hit outranks a much better Tier-2 embedding match
/// regardless of score magnitude. `find_skills` now calls
/// `semantic_search_nodes_of_type` (pure KNN over `node_type = 'skill'`
/// roots) instead of the hybrid `semantic_search_nodes`, specifically to
/// avoid this. This test pins that invariant: `bm25_search_roots` still
/// matches Research & Search / Bulk Import on this query (BM25 itself is
/// untouched — it still indexes a skill's full guidance-markdown subtree),
/// but that must no longer influence `find_skills`'s ranking at all, so
/// Schema Creation — the highest raw embedding score of the whole
/// registry on this query — must rank first, not merely survive top-K.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn find_skills_ranking_is_unaffected_by_bm25_keyword_collisions() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };

    let query = "Start keeping the calls we make on how the system is built, and who made each one";

    // Sanity check on the premise: BM25 alone, uninvolved in `find_skills`
    // today, still matches unrelated skills on this query via their guidance
    // markdown — proving the collision this test guards against is real, not
    // hypothetical.
    let bm25_roots = embedding_service
        .store()
        .bm25_search_roots(query, 20)
        .await
        .expect("bm25 must succeed");
    assert!(
        !bm25_roots.is_empty(),
        "expected this query to still trigger a BM25 keyword collision against \
         guidance-markdown content — if this now matches nothing, the fixture \
         prompt may need updating to keep exercising the regression"
    );

    let output = find_skills(
        &embedding_service,
        &node_service,
        FindSkillsInput {
            query: query.to_string(),
            limit: Some(RETRIEVAL_TOP_K),
        },
    )
    .await
    .expect("find_skills must succeed");

    let top_name = output
        .skills
        .first()
        .and_then(|s| s.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert_eq!(
        top_name, "Schema Creation",
        "Schema Creation has the highest raw embedding score for this query but \
         ranked below a BM25-only keyword collision — find_skills must rank by \
         KNN score alone, not the hybrid BM25+KNN tiering. Full ranking: {:?}",
        output.skills
    );
}

/// Diagnostic: the earlier golden-prompt run (`live_stage1_golden_prompts.rs`)
/// showed Stage 1 reformulates the SAME raw user message into different query
/// strings across reps (temperature sampling), e.g. "start tracking albums to
/// listen to" vs. the recorded baseline "create listening queue or watchlist
/// for music albums". Retrieval is deterministic for a fixed query string (as
/// `scenario_8a_routes_schema_creation_every_rep` above found — 5/5 identical
/// rankings), so any end-to-end instability has to come from Stage-1 wording
/// variance changing the query, not from embedding or KNN nondeterminism at
/// fixed input. This test checks several plausible Stage-1 paraphrases of the
/// scenario 8a prompt against the SAME registry to see whether wording alone
/// flips the winner.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn scenario_8a_paraphrases_show_wording_sensitivity() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };

    let paraphrases = [
        "Start keeping the calls we make on how the system is built, and who made each one",
        "track architecture decisions and who made them",
        "log the design decisions for the system and who authored each one",
        "create a record of who made which architecture decision",
        "keep a history of decisions made about how the system is built",
    ];

    for query in paraphrases {
        let output = find_skills(
            &embedding_service,
            &node_service,
            FindSkillsInput {
                query: query.to_string(),
                limit: Some(10),
            },
        )
        .await
        .expect("find_skills must succeed");
        let scored: Vec<String> = output
            .skills
            .iter()
            .map(|s| {
                let name = s.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                let score = s.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
                format!("{name}={score:.4}")
            })
            .collect();
        eprintln!("{query:?} -> {scored:?}");
    }
}

/// Control case, run alongside 8a per the golden-prompt file's own
/// methodology: a fix that stabilizes 8a must not silently regress a prompt
/// that already routes correctly.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn control_prompt_still_routes_node_creation_every_rep() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };

    let query = "Add a new task to follow up with the vendor next week";
    let rankings = repeated_rankings(&embedding_service, &node_service, query).await;

    let mut misses = Vec::new();
    for (i, ranked) in rankings.iter().enumerate() {
        eprintln!("rep {}: {:?}", i + 1, ranked);
        if !ranked.iter().any(|n| n == "Node Creation") {
            misses.push(i + 1);
        }
    }

    assert!(
        misses.is_empty(),
        "Node Creation dropped out of the top-{RETRIEVAL_TOP_K} on rep(s) {misses:?} of {REPS} \
         for query {query:?} — rankings were: {rankings:?}"
    );
}
