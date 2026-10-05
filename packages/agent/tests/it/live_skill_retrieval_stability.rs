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
//! cargo test -p nodespace-agent --test it live_skill_retrieval_stability:: -- --ignored --nocapture
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::SkillCandidate;
use nodespace_agent::local_agent::routing::{
    lookup_retrieval_query, select_candidates, RETRIEVAL_FETCH, RETRIEVAL_TOP_K,
};
use nodespace_agent::skill_pipeline::seed_skill_nodes;
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::{prepare_nodes_from_template, NodeTemplate};
use nodespace_core::methodology::linear;
use nodespace_core::models::SkillFields;
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
    seed_and_embed_registry(seed_skill_nodes()).await
}

/// [`seed_and_embed`] over an explicit registry, for tests that compare the
/// seeded registry against a modified copy of it.
async fn seed_and_embed_registry(
    registry: Vec<NodeTemplate>,
) -> Option<(Arc<NodeEmbeddingService>, Arc<NodeService>, TempDir)> {
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

    for tmpl in registry {
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

/// Scenario 8a: `find_skills` ranks by KNN score alone. It calls
/// `semantic_search_nodes_of_type` (pure KNN over `node_type = 'skill'`
/// roots) rather than the hybrid `semantic_search_nodes`, whose tiering puts
/// any keyword hit ahead of a better embedding match. Schema Creation — the
/// highest raw embedding score of the whole registry on this query — must
/// rank first, not merely survive top-K.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn find_skills_ranks_by_knn_score_alone() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };

    let query = "Start keeping the calls we make on how the system is built, and who made each one";

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
         ranked below another skill — find_skills must rank by KNN score \
         alone, not the hybrid BM25+KNN tiering. Full ranking: {:?}",
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

/// Regression test for a measured defect: Relationship Management's seeded
/// description never used the words a user actually says for linking two
/// things ("point at", "link", "connect"), so it lost Stage-2 retrieval
/// outright against the dev-workflow matrix's scenario 11c prompt — its own
/// score did not even reach the printed top-3. Asserts the fix's acceptance
/// criterion directly: across `REPS` identical calls, "Relationship
/// Management" clears `RETRIEVAL_TOP_K` every time.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn scenario_11c_routes_relationship_management_every_rep() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };

    let query = "point rebuild task at the decision it has to respect";
    let rankings = repeated_rankings(&embedding_service, &node_service, query).await;

    let mut misses = Vec::new();
    for (i, ranked) in rankings.iter().enumerate() {
        eprintln!("rep {}: {:?}", i + 1, ranked);
        if !ranked.iter().any(|n| n == "Relationship Management") {
            misses.push(i + 1);
        }
    }

    assert!(
        misses.is_empty(),
        "Relationship Management dropped out of the top-{RETRIEVAL_TOP_K} on rep(s) {misses:?} \
         of {REPS} for query {query:?} — rankings were: {rankings:?}"
    );
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

/// Second control case for the same fix, chosen deliberately rather than
/// picked at random: Organization is the one other seeded skill besides
/// Relationship Management that whitelists `create_relationship`
/// (`packages/agent/src/skill_pipeline.rs`), and its own description leans on
/// "categorize"/"group"/"collection" language that sits semantically close to
/// the new wording's "connect"/"associate"/"link" vocabulary. A fix that
/// stabilizes Relationship Management's retrieval must not crowd Organization
/// out of the top-3 on a prompt that already routes to it correctly.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn control_prompt_still_routes_organization_every_rep() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };

    let query = "Add this note to my reading list collection";
    let rankings = repeated_rankings(&embedding_service, &node_service, query).await;

    let mut misses = Vec::new();
    for (i, ranked) in rankings.iter().enumerate() {
        eprintln!("rep {}: {:?}", i + 1, ranked);
        if !ranked.iter().any(|n| n == "Organization") {
            misses.push(i + 1);
        }
    }

    assert!(
        misses.is_empty(),
        "Organization dropped out of the top-{RETRIEVAL_TOP_K} on rep(s) {misses:?} of {REPS} \
         for query {query:?} — rankings were: {rankings:?}"
    );
}

/// Top `limit` skills for `query` as `name=confidence`, for diagnosing *how*
/// a skill missed the cut rather than only *that* it did.
async fn scored_ranking(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    query: &str,
    limit: usize,
) -> Vec<String> {
    let output = find_skills(
        embedding_service,
        node_service,
        FindSkillsInput {
            query: query.to_string(),
            limit: Some(limit),
        },
    )
    .await
    .expect("find_skills must succeed");
    output
        .skills
        .iter()
        .map(|s| {
            format!(
                "{}={:.3}",
                s.get("name").and_then(|v| v.as_str()).unwrap_or_default(),
                s.get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or_default()
            )
        })
        .collect()
}

/// `skill`'s raw confidence for `query` across the whole registry, or `None`
/// if it is not returned.
async fn skill_confidence(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    query: &str,
    skill: &str,
) -> Option<f64> {
    find_skills(
        embedding_service,
        node_service,
        FindSkillsInput {
            query: query.to_string(),
            limit: Some(10),
        },
    )
    .await
    .expect("find_skills must succeed")
    .skills
    .iter()
    .find(|s| s.get("name").and_then(|v| v.as_str()) == Some(skill))
    .and_then(|s| s.get("confidence").and_then(|v| v.as_f64()))
}

/// Queries whose top-`RETRIEVAL_TOP_K` ranking misses `skill` on any rep, or —
/// with `rank_one` — does not put it first. Prints each query's wider ranking
/// with scores so a miss shows by how much.
async fn routing_misses<'a>(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    queries: &[&'a str],
    skill: &str,
    rank_one: bool,
) -> Vec<&'a str> {
    let mut misses = Vec::new();
    for &query in queries {
        eprintln!(
            "{query:?}: {:?}",
            scored_ranking(embedding_service, node_service, query, 6).await
        );
        let rankings = repeated_rankings(embedding_service, node_service, query).await;
        let hit = |ranked: &Vec<String>| {
            if rank_one {
                ranked.first().is_some_and(|n| n == skill)
            } else {
                ranked.iter().any(|n| n == skill)
            }
        };
        if !rankings.iter().all(hit) {
            misses.push(query);
        }
    }
    misses
}

/// "Mark X resolved/done/closed" sets a field on an existing record, so it
/// must reach Graph Editing — the skill that whitelists `update_node`. The
/// completion word shares vocabulary with other skills ("resolve" with the
/// conflict skill, before it was retitled Conflict Journal), and a lexical
/// false positive there left no write tool on Stage 2's surface: the model
/// found the record and then could not change it. Stage 2 does not recover from that on the locked model, so the
/// right skill has to be retrieved in the first place.
///
/// Covers the raw message (what retrieval embeds when Stage 1 emits no usable
/// query) and capability phrasings Stage 1 produces for such requests.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn completion_state_updates_route_graph_editing() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &[
            "The incident Rowan was on call for — mark it resolved",
            "mark the incident as resolved",
            "mark incident resolved",
            "set the incident's resolved field to true",
            "mark the invoice as paid",
            "close out the support ticket",
            "mark the outage report done",
            "mark the offline sync spec as signed off",
        ],
        "Graph Editing",
        false,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Graph Editing missed the top-{RETRIEVAL_TOP_K} for {misses:?}"
    );
}

/// Control for the case above: requests that really are about the conflict
/// journal must keep routing to Conflict Journal first, or widening Graph
/// Editing's vocabulary — or narrowing Conflict Journal's own — has only
/// moved the false positive.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn control_conflict_requests_still_route_conflict_journal() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &[
            "resolve the conflict between the two Sarah Chen records",
            "show me the open conflicts",
            "dismiss that duplicate collision, it's fine",
            "are there any unresolved conflicts?",
            "keep the existing node for that conflict",
        ],
        "Conflict Journal",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Conflict Journal lost rank 1 for {misses:?}"
    );
}

/// Control for the destructive direction: deletion requests that carry a
/// completion-state word ("the resolved incidents", "closed tickets") must not
/// rank Graph Editing above Node Deletion. Rank, not top-3, is what matters
/// here — `delete_node` is offered only from the top tool-bearing candidate,
/// so a Graph Editing that outranks Node Deletion silently withholds it.
///
/// Scoped to Graph Editing on purpose; the conflict skill's pull on
/// "resolved" is guarded separately by
/// `deletion_requests_mentioning_resolved_route_node_deletion`.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn control_deletion_requests_are_not_outranked_by_graph_editing() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let mut misses = Vec::new();
    for query in [
        "delete the resolved incidents",
        "remove the closed tickets",
        "get rid of the paid invoices",
    ] {
        let ranked = scored_ranking(&embedding_service, &node_service, query, 6).await;
        eprintln!("{query:?}: {ranked:?}");
        let rank = |skill: &str| {
            ranked
                .iter()
                .position(|r| r.starts_with(&format!("{skill}=")))
        };
        let deletion = rank("Node Deletion");
        if deletion.is_none() || rank("Graph Editing").is_some_and(|g| Some(g) < deletion) {
            misses.push(query);
        }
    }
    assert!(
        misses.is_empty(),
        "Graph Editing outranked Node Deletion for {misses:?}"
    );
}

/// Deletion requests that mention "resolved" must put Node Deletion at rank 1.
/// The conflict skill used to be titled "Conflict Resolution", and the shared
/// word "resolve" ranked it first on every one of these (e.g. "delete the
/// resolved incidents": 0.978 vs Node Deletion's 0.904). Since `delete_node` is
/// offered only from the top tool-bearing candidate, the deletion was
/// silently withheld.
///
/// "remove the resolved tickets" failed for a different reason: Graph
/// Editing's "mark it resolved" out-ranked Node Deletion on it (0.855 vs
/// 0.841), which no wording of either description could separate from "mark
/// incident resolved". Graph Editing's `exclusion` is what holds it now.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn deletion_requests_mentioning_resolved_route_node_deletion() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &[
            "delete the resolved incidents",
            "get rid of all the resolved bugs",
            "purge resolved alerts from last month",
            "delete the incident, it's resolved",
            "remove the resolved tickets",
        ],
        "Node Deletion",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Node Deletion lost rank 1 for {misses:?}"
    );
}

/// "remove" is the weakest deletion verb, and a completion-state word after it
/// ("the done tasks", "the paid invoices") is exactly what Graph Editing's
/// description names. Without Graph Editing's `exclusion`, "remove the
/// resolved tickets" ranked it first and `delete_node` was silently withheld.
/// Rank 1 on every rep, since `delete_node` is offered only from the winner.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn remove_requests_mentioning_a_state_route_node_deletion() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &[
            "remove the resolved tickets",
            "remove all the resolved incidents",
            "remove the closed tickets",
            "remove the done tasks",
            "remove the completed items",
            "remove the paid invoices",
        ],
        "Node Deletion",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Node Deletion lost rank 1 for {misses:?}"
    );
}

/// The cost side of Graph Editing's deletion-verb `exclusion`: a request that
/// removes a *field* rather than a record also says "remove", and the
/// exclusion lowers Graph Editing on it. It must still reach the top 3, or the
/// turn loses `update_node`.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn removing_a_field_still_reaches_graph_editing() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &[
            "remove the due date from the launch task",
            "clear the assignee on the onboarding ticket",
        ],
        "Graph Editing",
        false,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Graph Editing missed the top-{RETRIEVAL_TOP_K} for {misses:?}"
    );
}

/// An exclusion must cost a skill nothing on the requests it is meant to
/// serve. The penalty applies only where a query matches the exclusion better
/// than the description; this pins that on the real model by scoring the
/// completion-state requests with and without Graph Editing's exclusion and
/// requiring identical scores — not merely the same rank.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn graph_editing_exclusion_leaves_completion_state_scores_unchanged() {
    let Some((with, with_ns, _t1)) = seed_and_embed().await else {
        return;
    };
    let stripped: Vec<NodeTemplate> = seed_skill_nodes()
        .into_iter()
        .map(|t| {
            let mut skill =
                SkillFields::from_properties(&t.root_properties).expect("seed decodes as a skill");
            skill.exclusion = None;
            NodeTemplate {
                root_properties: skill.properties(),
                ..t
            }
        })
        .collect();
    let Some((without, without_ns, _t2)) = seed_and_embed_registry(stripped).await else {
        return;
    };

    let mut changed = Vec::new();
    for query in [
        "The incident Rowan was on call for — mark it resolved",
        "mark the incident as resolved",
        "mark incident resolved",
        "set the incident's resolved field to true",
        "mark the invoice as paid",
        "mark the outage report done",
        "mark the task as done",
    ] {
        // Raw confidences, not `scored_ranking`'s 3-decimal strings, which
        // would hide a penalty below 0.0005.
        let a = skill_confidence(&with, &with_ns, query, "Graph Editing").await;
        let b = skill_confidence(&without, &without_ns, query, "Graph Editing").await;
        eprintln!("{query:?}: with={a:?} without={b:?}");
        let (Some(a), Some(b)) = (a, b) else {
            panic!("Graph Editing must be ranked for {query:?}: with={a:?} without={b:?}");
        };
        if a != b {
            changed.push(query);
        }
    }
    assert!(
        changed.is_empty(),
        "Graph Editing's exclusion changed its score on completion-state requests {changed:?}"
    );
}

/// Retrieval queries for the lookups below, as the system builds them from the
/// topics Stage 1 was measured producing
/// (`live_stage1_golden_prompts::stage1_routes_a_knowledge_question_as_a_lookup`).
fn lookup_queries(topics: &[&str]) -> Vec<String> {
    topics
        .iter()
        .map(|topic| lookup_retrieval_query(topic))
        .collect()
}

/// A lookup must put Research & Search at rank 1: a question about what the
/// user has stored, or a request to find, look up, or list it.
///
/// Rank 1, not merely top 3: the leading candidate is the one a turn is
/// recorded as routed to. The skill's description used to name no retrieval
/// verb a user says, so "find nodes in nodespace" ranked Organization, Node
/// Creation and Node Deletion — each of which says "nodes" or "records" — and
/// did not retrieve it at all. A question fared worse: it has no verb, so it
/// embeds nearest whichever skill shares a noun with its topic, and "how is
/// the debounce logic applied?" retrieved Node Deletion and Node Merge.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn lookups_route_research_and_search() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let queries = lookup_queries(&[
        "how to find nodes in nodespace",
        "how our front end persistence layer works exactly",
        "debounce logic applied",
        "shared data layer on the front end",
        "why we picked sqlite over postgres",
        "write-up on the embedding pipeline",
        "release checklist",
        "onboarding notes",
        "Lisbon offsite record",
        "how to decide which cycle a slipped spec moves into",
        "when we signed off Kestrel",
        "who owns the billing service",
        "deploy runbook",
        "retry policy for failed uploads",
    ]);
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &queries.iter().map(String::as_str).collect::<Vec<_>>(),
        "Research & Search",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Research & Search lost rank 1 for {misses:?}"
    );
}

/// A lookup about one field of a named record — the date it was signed off,
/// who owns it — must lead with Research & Search as any other lookup does.
/// The topic is a record's name and a field's name, with no verb and no
/// question word, so it sits close to every skill that acts on a record:
/// retrieved as "search stored knowledge for Kestrel Sync sign off date" it
/// ranked Node Deletion 0.801, Research & Search 0.800, Conflict Journal
/// 0.799. The verbs the retrieval query now opens with
/// (`routing::lookup_retrieval_query`) put the search skill first on it by
/// 0.024.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn lookups_about_a_records_field_route_research_and_search() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let queries = lookup_queries(&[
        "Kestrel Sync sign off date",
        "Kestrel Gateway sign off date",
        "Q4 cycle end date",
        "Lantern Autosave owner",
        "offline sync spec review date",
        "billing service on-call owner",
    ]);
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &queries.iter().map(String::as_str).collect::<Vec<_>>(),
        "Research & Search",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Research & Search lost rank 1 for {misses:?}"
    );
}

/// The raw message is what retrieval embeds when Stage 1 emits no usable
/// decision. A request that names a retrieval verb, or a question whose topic
/// is no other skill's subject, still reaches Research & Search at rank 1
/// unaided.
///
/// No wording of the description moved the remaining bare questions there —
/// "how is the debounce logic applied?", "why did we pick sqlite over
/// postgres?" — which is why a lookup is retrieved by capability rather than
/// by topic.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn unrouted_find_requests_still_route_research_and_search() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &[
            "find nodes in nodespace",
            "could you find the write-up on the embedding pipeline?",
            "locate the onboarding notes",
            "search for anything mentioning the billing migration",
            "how our front end persistence layer works exactly?",
            "explain how the frontend persistence layer works",
        ],
        "Research & Search",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Research & Search lost rank 1 for {misses:?}"
    );
}

/// A lookup whose topic is another skill's own subject — "the merge gate"
/// beside Node Merge, "sync conflicts" beside Conflict Journal — may rank that
/// skill first: the request is genuinely close to both. Research & Search must
/// still clear the top 3, so `search_semantic` is on the surface for the
/// model, and for the system to run when the model does not.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn lookups_naming_another_skills_subject_still_reach_research_and_search() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let queries = lookup_queries(&[
        "merge gate",
        "how sync conflicts get detected",
        "specs on sync",
    ]);
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &queries.iter().map(String::as_str).collect::<Vec<_>>(),
        "Research & Search",
        false,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Research & Search missed the top-{RETRIEVAL_TOP_K} for {misses:?}"
    );
}

/// The cost side of Research & Search's retrieval vocabulary: naming "records"
/// and "nodes of any type" must not pull it ahead of the skill that owns a
/// write. Each request here must still lead with its own skill — a deletion
/// that leads with a read-only skill has `delete_node` withheld, and a create
/// or update that does loses its write tool to the top-3 lottery.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn research_and_search_does_not_displace_write_skills() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let mut misses = Vec::new();
    for (queries, skill) in [
        (
            &[
                "delete the resolved incidents",
                "remove the closed tickets",
                "get rid of the paid invoices",
            ][..],
            "Node Deletion",
        ),
        (
            &[
                "Add a task to renew the domain",
                "create a note about the offsite agenda",
            ][..],
            "Node Creation",
        ),
        (
            &["mark the invoice as paid", "mark the outage report done"][..],
            "Graph Editing",
        ),
        (
            &["Start keeping the calls we make on how the system is built, and who made each one"]
                [..],
            "Schema Creation",
        ),
    ] {
        for &query in queries {
            let ranked = scored_ranking(&embedding_service, &node_service, query, 6).await;
            eprintln!("{query:?}: {ranked:?}");
            let rank = |name: &str| {
                ranked
                    .iter()
                    .position(|r| r.starts_with(&format!("{name}=")))
            };
            let own = rank(skill);
            if own.is_none() || rank("Research & Search").is_some_and(|r| Some(r) < own) {
                misses.push(query);
            }
        }
    }
    assert!(
        misses.is_empty(),
        "Research & Search outranked the owning write skill for {misses:?}"
    );
}

/// The candidates Stage 2 judges for `query`: retrieval's ranking, asked for
/// one past the bound, through `routing::select_candidates` — what
/// `agent_loop`'s `route` does with a single query.
async fn stage2_candidate_names(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    query: &str,
) -> Vec<String> {
    let output = find_skills(
        embedding_service,
        node_service,
        FindSkillsInput {
            query: query.to_string(),
            limit: Some(RETRIEVAL_FETCH),
        },
    )
    .await
    .expect("find_skills must succeed");
    let ranked: Vec<SkillCandidate> = output
        .skills
        .iter()
        .map(|s| {
            let text = |key: &str| {
                s.get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            SkillCandidate {
                id: text("id"),
                name: text("name"),
                description: text("description"),
                score: s.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                tools: s
                    .get("tools")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|t| t.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                instructions: String::new(),
                schema_metadata: serde_json::json!([]),
                schemas_linked: false,
                pinned: false,
            }
        })
        .collect();
    select_candidates(ranked)
        .into_iter()
        .map(|c| c.name)
        .collect()
}

/// A request to start tracking something reads, to an embedding, a good deal
/// like a request to find it — both are about stored records — so Research &
/// Search places on it. No wording of that skill's description kept its
/// lookups at rank 1 and stayed below Schema Creation here: "keep track of
/// decisions behind each feature" (what Stage 1 makes of "start keeping track
/// of the decisions behind each feature") ranks Graph Editing, Research &
/// Search, Organization, then Schema Creation. An `exclusion` naming the
/// tracking verbs put Schema Creation back, and dropped Research & Search
/// from the top 3 on short lookups ("list specs on sync").
///
/// So the place is not contested in the description at all: a read-only skill
/// that does not lead a turn is not counted against the skills that write
/// (`routing::select_candidates`). This pins that the skill owning
/// `create_schema` reaches Stage 2 on that request, and that the lookup still
/// does.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn a_lookup_placing_on_a_tracking_request_does_not_cost_schema_creation_its_place() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let query = "keep track of decisions behind each feature";
    eprintln!(
        "{query:?}: {:?}",
        scored_ranking(&embedding_service, &node_service, query, 6).await
    );
    let judged = stage2_candidate_names(&embedding_service, &node_service, query).await;
    assert!(
        judged.iter().any(|n| n == "Schema Creation"),
        "Schema Creation must reach Stage 2 for {query:?}; judged: {judged:?}"
    );
    assert!(
        judged.iter().any(|n| n == "Research & Search"),
        "the lookup keeps its place too; judged: {judged:?}"
    );
    assert!(
        judged.len() <= RETRIEVAL_FETCH,
        "never more than one past the bound; judged: {judged:?}"
    );
}

/// The registry of a workspace with the Linear-style Playbook installed: the
/// built-ins plus every skill the install seeds, overview included. Only each
/// root's title and description are embedded, so the overview's
/// install-time "Installed in this workspace" section is irrelevant here.
fn linear_workspace_registry() -> Vec<NodeTemplate> {
    let playbook = linear::playbook();
    let mut registry = seed_skill_nodes();
    registry.extend(playbook.skills.iter().map(|skill| skill.template()));
    registry.push(playbook.overview.template());
    registry
}

/// A Playbook skill competes with the built-ins on the words a request
/// arrives in, not on what its guidance covers. Each seeded Linear skill must
/// rank FIRST for its own intent — phrased the way a user says it ("file a
/// bug", "start the sprint", "why can't I close this"), none of which names
/// the skill's own vocabulary.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn linear_playbook_skills_win_their_own_intents() {
    let Some((embedding_service, node_service, _temp_dir)) =
        seed_and_embed_registry(linear_workspace_registry()).await
    else {
        return;
    };

    let cases: [(&str, &[&'static str]); 3] = [
        (
            "Creating an Issue",
            &[
                "file a bug for the login timeout",
                "report a bug: checkout crashes on Safari",
                "open a ticket for the flaky CI job",
                "raise an issue about the broken CSV export",
                "log a bug against the sync engine",
                "create an issue for the onboarding redesign",
            ],
        ),
        (
            "Sprints and Cycles",
            &[
                "start the sprint",
                "start a new two-week cycle on Monday",
                "add this issue to the current sprint",
                "move the unfinished work into the next sprint",
                "plan the next cycle",
                "how many points are in this sprint?",
            ],
        ),
        (
            "Issue Validation Rules",
            &[
                "why can't I close this?",
                "it won't let me mark this done",
                "why was my status change rejected?",
                "I can't start this issue, it says it's blocked",
                "why won't it let me move this to in progress?",
            ],
        ),
    ];

    let mut misses = Vec::new();
    for (skill, queries) in cases {
        for query in routing_misses(&embedding_service, &node_service, queries, skill, true).await {
            misses.push(format!("{skill} <- {query:?}"));
        }
    }
    assert!(misses.is_empty(), "lost rank 1: {misses:#?}");
}

/// The cost side of the case above: a Playbook skill written in a user's
/// verbs must not take a general request away from the built-in that serves
/// it. "Take away" is measured against the same query on the built-ins alone,
/// in the two senses retrieval acts on: an owner that ranked first must still
/// rank first (destructive tools come only from the winner), and an owner
/// inside the top-`RETRIEVAL_TOP_K` must stay inside it (Stage 2 sees only
/// those). A Playbook skill ranking above an owner that was already third is
/// no regression; pushing that owner out of the window is.
///
/// Two general requests sat closer to a Playbook skill than any description
/// wording could fix, and the skills' `exclusion`s are what hold them: "add a
/// reminder to renew my passport" (Node Creation, third at 0.753) for Sprints
/// and Cycles and Creating an Issue, and "point rebuild task at the decision
/// it has to respect" (Relationship Management, second at 0.837) for Issue
/// Validation Rules. "Add a new task to follow up with the vendor next week"
/// was held by retitling "Working with Cycles", whose title alone outranked
/// Node Creation on it.
///
/// Accepted, not held: on "Create a new task called 'Review Q3 report'",
/// Creating an Issue ranks first (0.890 when measured) and Node Creation moves
/// from second (0.867, behind Graph Editing's 0.877) to third — the last slot
/// in the window. `create_node` stays reachable and the issue guidance itself
/// sends a plain to-do to `task`. A further Playbook skill that scores above
/// 0.867 there will push Node Creation out and fail this test; that is the
/// test doing its job, and the fix is that skill's wording or `exclusion`.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn linear_playbook_skills_do_not_displace_built_ins() {
    let Some((built_ins, built_ins_ns, _t1)) = seed_and_embed().await else {
        return;
    };
    let Some((linear, linear_ns, _t2)) = seed_and_embed_registry(linear_workspace_registry()).await
    else {
        return;
    };

    let mut misses = Vec::new();
    for (query, owner) in [
        ("create a note about today's standup", "Node Creation"),
        (
            "Create a new task called 'Review Q3 report'",
            "Node Creation",
        ),
        (
            "Add a new task to follow up with the vendor next week",
            "Node Creation",
        ),
        ("add a reminder to renew my passport", "Node Creation"),
        ("mark the task as done", "Graph Editing"),
        ("change the due date on the launch task", "Graph Editing"),
        ("set the onboarding task to in progress", "Graph Editing"),
        ("remove the done tasks", "Node Deletion"),
        ("delete the closed tickets", "Node Deletion"),
        (
            "why hasn't my automation fired for this node?",
            "Play Workflow State",
        ),
        (
            "point rebuild task at the decision it has to respect",
            "Relationship Management",
        ),
        (
            "Add this note to my reading list collection",
            "Organization",
        ),
        ("Create an invoice tracking database", "Schema Creation"),
        (
            "Search my notes for anything about embeddings",
            "Research & Search",
        ),
    ] {
        let before = scored_ranking(&built_ins, &built_ins_ns, query, 20).await;
        let after = scored_ranking(&linear, &linear_ns, query, 20).await;
        eprintln!("{query:?}: {:?}", &after[..after.len().min(6)]);
        let rank = |ranked: &[String]| {
            ranked
                .iter()
                .position(|r| r.starts_with(&format!("{owner}=")))
                .unwrap_or(usize::MAX)
        };
        let (was, now) = (rank(&before), rank(&after));
        let lost_first = was == 0 && now != 0;
        let left_window = was < RETRIEVAL_TOP_K && now >= RETRIEVAL_TOP_K;
        if lost_first || left_window {
            misses.push(format!(
                "{query:?}: {owner} rank {was} -> {now}; now {:?}",
                &after[..after.len().min(4)]
            ));
        }
    }
    assert!(misses.is_empty(), "displaced a built-in: {misses:#?}");
}

/// A request to start tracking something asks for a new type, and the model
/// can only make one when the skill owning `create_schema` reaches Stage 2.
/// Schema Creation's description used to say "keep track of" and not "start
/// tracking", and "start tracking planning cycles" (what Stage 1 makes of
/// "start tracking our planning cycles") ranked Graph Editing, Relationship
/// Management, Node Creation, then Schema Creation at 0.778: `create_schema`
/// was off the surface and the turn tried `create_node` until it gave up.
///
/// Rank 1, since the leading candidate is the one a turn is recorded as routed
/// to. Covers the raw message as well as Stage 1's wording of it.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn start_tracking_requests_route_schema_creation() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &[
            "start tracking planning cycles",
            "start tracking our planning cycles",
            "start tracking release trains",
            "begin tracking design decisions",
            "we should start tracking production incidents",
            "track planning cycles",
        ],
        "Schema Creation",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Schema Creation lost rank 1 for {misses:?}"
    );
}

/// A request for a new kind of record that names the details each one carries
/// ("set up Postmortems with a severity and a review date") asks for a type.
/// It must lead with Schema Creation, not merely place it: the fields declared
/// on a write tool come only from the candidates at the turn's top score
/// (`routing::declare_write_tool_fields`).
///
/// The requests are what Stage 1 makes of such a message, in both the "set
/// up" and the "keep track of" wording, over several nouns and details so the
/// shape is what is measured.
///
/// The shape leads where the nouns are no other skill's: by 0.012 to 0.036 on
/// the first three. It does not where they are, and that is recorded here and
/// not accepted. An incident and its severity read as Graph Editing's
/// completion states, and a runbook as an automation:
///
/// - "set up Postmortems with a severity and review date": Graph Editing
///   0.864, Schema Creation 0.856.
/// - "keep track of incident postmortems with severity and review date":
///   Graph Editing 0.889, Schema Creation 0.868.
/// - "set up Runbooks with a service and a last reviewed date": Play
///   Authoring 0.844, Schema Creation 0.814.
///
/// No wording of either description separated them without a cost elsewhere.
/// Naming the details a new kind carries in Schema Creation's ("…such as a
/// priority, an owner, or a date") won the first by 0.004 and lifted that
/// skill by 0.01 to 0.02 on requests of every kind: it then led "add Lantern
/// Autosave to specs we keep" and "move the offline sync spec's review to
/// next week", and passed the search skill on a lookup. Narrowing Graph
/// Editing to one named item moved it by 0.005 at most, and an `exclusion`
/// naming the setting-up verbs did not move it at all: these requests sit
/// closer to its description than to any exclusion. Having Stage 1 say
/// "record type" or "a new kind of record" lifted both skills together.
///
/// For those three only the second half is asserted: Schema Creation is in
/// the top 3, so `create_schema` is offered.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn a_new_kind_of_record_with_named_details_leads_or_places_schema_creation() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let mut misses = Vec::new();
    for (query, lead_asserted) in [
        (
            "set up Retrospectives with an owner and a follow-up date",
            true,
        ),
        (
            "keep track of vendor contracts with a renewal date and an owner",
            true,
        ),
        (
            "keep track of customer interviews with a persona and an interview date",
            true,
        ),
        ("set up Postmortems with a severity and review date", false),
        (
            "keep track of incident postmortems with severity and review date",
            false,
        ),
        (
            "set up Runbooks with a service and a last reviewed date",
            false,
        ),
    ] {
        eprintln!(
            "{query:?}: {:?}",
            scored_ranking(&embedding_service, &node_service, query, 6).await
        );
        let rankings = repeated_rankings(&embedding_service, &node_service, query).await;
        let holds = |ranked: &Vec<String>| {
            let place = ranked.iter().position(|n| n == "Schema Creation");
            if lead_asserted {
                place == Some(0)
            } else {
                place.is_some()
            }
        };
        if !rankings.iter().all(holds) {
            misses.push(query);
        }
    }
    assert!(
        misses.is_empty(),
        "Schema Creation lost rank 1, or missed the top-{RETRIEVAL_TOP_K} where the lead is not \
         asserted, for {misses:?}"
    );
}

/// The cost side of Schema Creation opening with the tracking verb: a request
/// to track one record that already exists is an update or a create, and the
/// skill that owns `create_schema` holds neither tool. Node Creation or Graph
/// Editing must still reach Stage 2 on it.
///
/// Graph Editing leads three of these four. Schema Creation leads "start
/// tracking the offline sync spec's sign-off" (0.853, Graph Editing 0.811,
/// Node Creation 0.792): that turn is recorded as routed to the type skill,
/// with `create_schema` offered beside the record tools. Who leads is not
/// asserted here; `updates_to_one_record_do_not_lead_with_schema_creation`
/// asserts it for requests that carry no tracking verb.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn tracking_one_existing_record_still_reaches_a_record_skill() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let mut misses = Vec::new();
    for query in [
        "start tracking the login timeout bug",
        "track this task's progress",
        "start tracking the offline sync spec's sign-off",
        "keep track of the Q4 cycle's status",
    ] {
        eprintln!(
            "{query:?}: {:?}",
            scored_ranking(&embedding_service, &node_service, query, 6).await
        );
        let rankings = repeated_rankings(&embedding_service, &node_service, query).await;
        let holds = |ranked: &Vec<String>| {
            ranked
                .iter()
                .any(|n| n == "Node Creation" || n == "Graph Editing")
        };
        if !rankings.iter().all(holds) {
            misses.push(query);
        }
    }
    assert!(
        misses.is_empty(),
        "neither Node Creation nor Graph Editing reached the top-{RETRIEVAL_TOP_K} for {misses:?}"
    );
}

/// The other cost side of Schema Creation's opening: a change to one record,
/// with no tracking verb in it, must not lead with the type skill. An earlier
/// opening ("Start tracking or begin tracking a new kind of thing: …") lifted
/// Schema Creation by about 0.03 on requests of every kind and took the lead
/// from Graph Editing on a request to move one record's date (0.799 against
/// 0.793) and from Conflict Journal on the second of these. "Start tracking:
/// …" wins the tracking requests by margins as large and leaves both leaders
/// where they were: on the first of these Graph Editing leads at 0.758, with
/// Play Authoring and Schema Creation behind it at 0.705.
///
/// One cost of the opening is recorded here and not accepted: "mark the
/// offline sync spec as signed off" now leads with Schema Creation by 0.001
/// (0.791, Graph Editing 0.790). Without the opening Graph Editing led it,
/// 0.790 to 0.773. Schema Creation's description lists specs, and no opening
/// measured both won the tracking requests and stayed under Graph Editing
/// there. It is left out of the lead assertion below, and
/// `completion_state_updates_route_graph_editing` asserts Graph Editing still
/// reaches Stage 2 on it, so `update_node` is offered, though without its
/// declared fields, which come only from the top-scoring candidates
/// (`routing::declare_write_tool_fields`).
///
/// Each request must also keep a skill that can write a record in the top 3:
/// the second leads with Conflict Journal, which holds no `update_node`.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn updates_to_one_record_do_not_lead_with_schema_creation() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let mut misses = Vec::new();
    for query in [
        "update Kestrel Gateway sign-off to 2 April 2025",
        "move the offline sync spec's review to next week",
        "change the owner of the Q4 cycle to Priya",
    ] {
        eprintln!(
            "{query:?}: {:?}",
            scored_ranking(&embedding_service, &node_service, query, 6).await
        );
        let rankings = repeated_rankings(&embedding_service, &node_service, query).await;
        let holds = |ranked: &Vec<String>| {
            ranked.first().is_some_and(|n| n != "Schema Creation")
                && ranked
                    .iter()
                    .any(|n| n == "Graph Editing" || n == "Node Creation")
        };
        if !rankings.iter().all(holds) {
            misses.push(query);
        }
    }
    assert!(
        misses.is_empty(),
        "Schema Creation led a change to one record, or no record skill reached the \
         top-{RETRIEVAL_TOP_K}, for {misses:?}"
    );
}

/// The opening's cost side on adds: one named record added to a list that
/// already exists must not lead with the type skill, and a skill that holds
/// `create_node` must be in the top 3. Which skill leads is not asserted: on
/// "Add Lantern Autosave to the specs we keep." Graph Editing leads at 0.840,
/// and Schema Creation (0.826), Node Creation (0.825) and Play Authoring
/// (0.821) follow inside 0.005 of each other. Graph Editing and Node Creation
/// can create the record; Schema Creation and Play Authoring cannot.
///
/// A second cost of the opening is recorded here and not accepted: "add the
/// offline sync spec to the specs for this cycle" led with Node Creation,
/// 0.841 to Schema Creation's 0.835, and now leads with Schema Creation,
/// 0.846 to 0.841. Like the completion-state request recorded on
/// `updates_to_one_record_do_not_lead_with_schema_creation`, it names specs,
/// which Schema Creation's description lists. For that request only the
/// second half is asserted: Node Creation is in the top 3, so `create_node`
/// is offered. It is offered without its declared fields, which come only
/// from the top-scoring candidates (`routing::declare_write_tool_fields`).
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn adds_to_an_existing_list_keep_a_skill_that_can_create() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let mut misses = Vec::new();
    for (query, lead_asserted) in [
        ("add Lantern Autosave to specs we keep", true),
        ("Add Lantern Autosave to the specs we keep.", true),
        ("add the Q4 cycle to our planning cycles", true),
        (
            "add the offline sync spec to the specs for this cycle",
            false,
        ),
    ] {
        eprintln!(
            "{query:?}: {:?}",
            scored_ranking(&embedding_service, &node_service, query, 6).await
        );
        let rankings = repeated_rankings(&embedding_service, &node_service, query).await;
        let holds = |ranked: &Vec<String>| {
            (!lead_asserted || ranked.first().is_some_and(|n| n != "Schema Creation"))
                && ranked
                    .iter()
                    .any(|n| n == "Node Creation" || n == "Graph Editing")
        };
        if !rankings.iter().all(holds) {
            misses.push(query);
        }
    }
    assert!(
        misses.is_empty(),
        "Schema Creation led an add to an existing list, or no skill holding create_node \
         reached the top-{RETRIEVAL_TOP_K}, for {misses:?}"
    );
}

/// A request to change an automation, or to switch it, must reach Play
/// Authoring: `update_play` is whitelisted by that skill alone, so a turn it
/// misses cannot write the play at all.
///
/// Covers the user's own words and the capability phrasings Stage 1 produces
/// for them, including requests worded with "fire" and "run": the verbs a
/// question about why a rule has not fired uses too, and so the ones the
/// skill's exclusion must not cost it. The closest is "stop that rule firing
/// for low priority tasks", 0.868 to Play Workflow State's 0.860.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn play_change_requests_route_play_authoring() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &[
            "change the roll-up play so it also runs when a task is cancelled",
            "make that rule fire only for high priority tasks",
            "stop that rule firing for low priority tasks",
            "make the rule not run on weekends",
            "have the play run when a story is closed too",
            "add a rule to the sprint close-out play",
            "remove the second rule from this automation",
            "edit the conditions of an automation rule",
            "turn this play off",
            "disable the weekly triage automation",
            "switch the play back on",
        ],
        "Play Authoring",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Play Authoring lost rank 1 for {misses:?}"
    );

    // Once linked as the daemon links it, retrieval hands the skill the
    // `play` schema as a linked set, which is what holds its turn to plays.
    nodespace_agent::skill_pipeline::link_seeded_skills(&node_service)
        .await
        .expect("the seeded skills link");
    let output = find_skills(
        &embedding_service,
        &node_service,
        FindSkillsInput {
            query: "turn this play off".to_string(),
            limit: Some(RETRIEVAL_TOP_K),
        },
    )
    .await
    .expect("find_skills must succeed");
    let authoring = output
        .skills
        .iter()
        .find(|s| s.get("name").and_then(|v| v.as_str()) == Some("Play Authoring"))
        .expect("Play Authoring is retrieved");
    assert_eq!(authoring["schemas_linked"], true, "{authoring}");
    let types: Vec<&str> = authoring["schema_metadata"]
        .as_array()
        .expect("schema metadata")
        .iter()
        .filter_map(|s| s["type_id"].as_str())
        .collect();
    assert_eq!(types, ["play"]);
}

/// Control for the case above. The two play skills share every noun (play,
/// rule, automation, workflow), and a question about why a rule has not fired
/// is Play Workflow State's: it must lead, so a question that asks for no
/// change is not answered with a write skill first.
///
/// Play Authoring's `exclusion` is what holds this. Without it "why didn't the
/// play trigger for that story?" led with Play Authoring, 0.887 to 0.863; with
/// it Play Workflow State leads, 0.863 to 0.846.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn control_why_a_rule_has_not_fired_still_routes_play_workflow_state() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &embedding_service,
        &node_service,
        &[
            "why hasn't the sprint close-out rule fired for this ticket?",
            "what is still missing before the automation runs on this task?",
            "why didn't the play trigger for that story?",
            "check which conditions of the workflow are unmet for this node",
        ],
        "Play Workflow State",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Play Workflow State lost rank 1 for {misses:?}"
    );
}
