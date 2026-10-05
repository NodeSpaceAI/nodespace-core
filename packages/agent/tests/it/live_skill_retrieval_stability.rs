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
    is_add_shaped, leading_tool_bearing_candidate, lookup_retrieval_query, retrieve_candidates,
    select_candidates, skill_can_create_a_record, skill_is_destructive, RETRIEVAL_FETCH,
    RETRIEVAL_TOP_K,
};
use nodespace_agent::skill_pipeline::seed_skill_nodes;
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::{prepare_nodes_from_template, NodeTemplate};
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

/// Names a JSON file of trial wording, so a `use_for` or `not_for` can be
/// measured against the whole live suite before it is written into a seed
/// table (and the crate rebuilt):
///
/// ```json
/// { "Organization": { "use_for": "…", "not_for": null } }
/// ```
///
/// A key that is absent leaves that field as seeded; `null` clears a
/// `not_for`. Every registry the suite seeds takes the wording, so while the
/// variable is set every live guard measures the trial, and the confusion
/// matrix prints its results and then fails: a trial is read, never passed.
pub(crate) const TEXT_OVERRIDES_VAR: &str = "SKILL_TEXT_OVERRIDES";

/// `registry` with the trial wording of [`TEXT_OVERRIDES_VAR`] applied, or
/// unchanged when the variable is unset.
fn with_trial_wording(registry: Vec<NodeTemplate>) -> Vec<NodeTemplate> {
    let Ok(path) = std::env::var(TEXT_OVERRIDES_VAR) else {
        return registry;
    };
    let overrides: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read trial wording"))
            .expect("trial wording must be a JSON object keyed by skill name");
    eprintln!("TRIAL WORDING from {path}: {:?}", overrides.keys());
    // A misspelt skill or field would otherwise measure the seeded wording
    // and report it as the trial's.
    for (skill, wording) in &overrides {
        assert!(
            registry.iter().any(|t| &t.title == skill),
            "trial wording names {skill:?}, which is not a seeded skill"
        );
        let fields = wording
            .as_object()
            .unwrap_or_else(|| panic!("trial wording for {skill:?} must be an object"));
        for field in fields.keys() {
            assert!(
                field == "use_for" || field == "not_for",
                "trial wording for {skill:?} sets {field:?}; only use_for and not_for can be tried"
            );
        }
    }
    registry
        .into_iter()
        .map(|t| {
            let Some(wording) = overrides.get(&t.title) else {
                return t;
            };
            let mut fields =
                SkillFields::from_properties(&t.root_properties).expect("seed decodes as a skill");
            if let Some(use_for) = wording.get("use_for") {
                fields.use_for = use_for
                    .as_str()
                    .expect("use_for must be a string")
                    .to_string();
            }
            if let Some(not_for) = wording.get("not_for") {
                assert!(
                    not_for.is_string() || not_for.is_null(),
                    "not_for must be a string, or null to clear it"
                );
                fields.not_for = not_for.as_str().map(str::to_string);
            }
            NodeTemplate {
                root_properties: fields.properties(),
                ..t
            }
        })
        .collect()
}

/// [`seed_and_embed`] over an explicit registry, for tests that compare the
/// seeded registry against a modified copy of it.
pub(crate) async fn seed_and_embed_registry(
    registry: Vec<NodeTemplate>,
) -> Option<(Arc<NodeEmbeddingService>, Arc<NodeService>, TempDir)> {
    seed_and_embed_exactly(with_trial_wording(registry)).await
}

/// Seed and embed `registry` as given, with no trial wording applied.
async fn seed_and_embed_exactly(
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
pub(crate) async fn repeated_rankings(
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
/// incident resolved". Graph Editing's `not_for` is what holds it now.
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
/// description names. Without Graph Editing's `not_for`, "remove the
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

/// The cost side of Graph Editing's deletion-verb `not_for`: a request that
/// removes a *field* rather than a record also says "remove", and the
/// `not_for` lowers Graph Editing on it. It must still reach the top 3, or the
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

/// The requests among `requests` on which `skill` scores differently with its
/// `not_for` than without it. `None` when the model isn't on disk.
///
/// A `not_for` must cost a skill nothing on the requests it is meant to
/// serve: the penalty applies only where a query matches `not_for` better
/// than `use_for`. This scores each request against the registry as seeded
/// and against a copy with only this skill's `not_for` removed, and compares
/// raw confidences, not `scored_ranking`'s 3-decimal strings, which would
/// hide a penalty below 0.0005.
async fn requests_its_not_for_changes<'a>(
    skill: &str,
    requests: &[&'a str],
) -> Option<Vec<&'a str>> {
    let registry = with_trial_wording(seed_skill_nodes());
    let stripped: Vec<NodeTemplate> = registry
        .iter()
        .cloned()
        .map(|t| {
            if t.title != skill {
                return t;
            }
            let mut fields =
                SkillFields::from_properties(&t.root_properties).expect("seed decodes as a skill");
            assert!(
                fields.not_for.take().is_some(),
                "{skill} carries no not_for to measure"
            );
            NodeTemplate {
                root_properties: fields.properties(),
                ..t
            }
        })
        .collect();
    let (with, with_ns, _t1) = seed_and_embed_exactly(registry).await?;
    let (without, without_ns, _t2) = seed_and_embed_exactly(stripped).await?;

    let mut changed = Vec::new();
    for &request in requests {
        let a = skill_confidence(&with, &with_ns, request, skill).await;
        let b = skill_confidence(&without, &without_ns, request, skill).await;
        eprintln!("{request:?}: with={a:?} without={b:?}");
        let (Some(a), Some(b)) = (a, b) else {
            panic!("{skill} must be ranked for {request:?}: with={a:?} without={b:?}");
        };
        if a != b {
            changed.push(request);
        }
    }
    Some(changed)
}

/// Graph Editing's `not_for` names deletion; the completion-state requests
/// the skill exists for must score exactly as they would without it — not
/// merely rank the same.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn graph_editing_not_for_leaves_completion_state_scores_unchanged() {
    let Some(changed) = requests_its_not_for_changes(
        "Graph Editing",
        &[
            "The incident Rowan was on call for — mark it resolved",
            "mark the incident as resolved",
            "mark incident resolved",
            "set the incident's resolved field to true",
            "mark the invoice as paid",
            "mark the outage report done",
            "mark the task as done",
        ],
    )
    .await
    else {
        return;
    };
    assert!(
        changed.is_empty(),
        "Graph Editing's not_for changed its score on completion-state requests {changed:?}"
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
    select_candidates(
        retrieved_candidates(embedding_service, node_service, query, RETRIEVAL_FETCH).await,
    )
    .into_iter()
    .map(|c| c.name)
    .collect()
}

/// Retrieval's ranking for `query`, as the candidates `agent_loop`'s `route`
/// works with.
async fn retrieved_candidates(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    query: &str,
    limit: usize,
) -> Vec<SkillCandidate> {
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
            let text = |key: &str| {
                s.get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            SkillCandidate {
                id: text("id"),
                name: text("name"),
                use_for: text("use_for"),
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
        .collect()
}

/// The candidates Stage 2 judges for `query`, as `agent_loop`'s `route`
/// arrives at them for a single query: `routing::retrieve_candidates`, then
/// `routing::select_candidates`.
async fn routed_candidates(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    query: &str,
) -> Vec<SkillCandidate> {
    let ranked = retrieve_candidates(query, |q, limit| async move {
        Ok::<_, std::convert::Infallible>(
            retrieved_candidates(embedding_service, node_service, &q, limit).await,
        )
    })
    .await
    .unwrap_or_else(|never| match never {});
    select_candidates(ranked)
}

/// A request to start tracking something reads, to an embedding, a good deal
/// like a request to find it — both are about stored records — so Research &
/// Search places on it. No wording of that skill's description kept its
/// lookups at rank 1 and stayed below Schema Creation here: "keep track of
/// decisions behind each feature" (what Stage 1 makes of "start keeping track
/// of the decisions behind each feature") ranks Graph Editing, Research &
/// Search, Organization, then Schema Creation. A `not_for` naming the
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
/// Editing to one named item moved it by 0.005 at most, and a `not_for`
/// naming the setting-up verbs did not move it at all: these requests sit
/// closer to its `use_for` than to any `not_for`. Having Stage 1 say
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
/// skill's `not_for` must not cost it. The closest is "stop that rule firing
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
/// Play Authoring's `not_for` is what holds this. Without it "why didn't the
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

/// The requests among `requests` that `skill` leads on any rep.
async fn requests_led_by<'a>(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    requests: &[&'a str],
    skill: &str,
) -> Vec<&'a str> {
    let mut led = Vec::new();
    for &request in requests {
        eprintln!(
            "{request:?}: {:?}",
            scored_ranking(embedding_service, node_service, request, 6).await
        );
        let rankings = repeated_rankings(embedding_service, node_service, request).await;
        if rankings
            .iter()
            .any(|ranked| ranked.first().is_some_and(|first| first == skill))
        {
            led.push(request);
        }
    }
    led
}

/// One more record of a kind that already exists, and a saved view of
/// records, share Schema Creation's nouns ("customer", "ticket", "invoice")
/// and define nothing. Its `not_for` names both; without it "add a new
/// customer called Harbor Freight" led with Schema Creation at 0.745 over
/// Node Creation's 0.737.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn one_more_record_or_a_saved_view_does_not_lead_with_schema_creation() {
    let Some((es, ns, _tmp)) = seed_and_embed().await else {
        return;
    };
    let led = requests_led_by(
        &es,
        &ns,
        &[
            "add a new customer called Harbor Freight",
            "create a ticket for the login bug",
            "add another album: Blue Train by Coltrane",
            "save a view of my overdue invoices",
            "create a saved query for open tickets",
            "make a filter for tasks due this week",
        ],
        "Schema Creation",
    )
    .await;
    assert!(
        led.is_empty(),
        "Schema Creation led requests that define no type: {led:?}"
    );
}

/// The cost side of Schema Creation's `not_for`: the tracking and
/// schema-change requests the skill exists for score exactly as they would
/// without it.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn schema_creation_not_for_leaves_tracking_scores_unchanged() {
    let Some(changed) = requests_its_not_for_changes(
        "Schema Creation",
        &[
            "track albums I mean to listen to",
            "I need a tracker for the venues I book",
            "start keeping tabs on who owes me money",
            "set up something to log my freelance gigs",
            "track equipment checkout and return status",
            "add a priority field to my invoices",
            "add a severity field to tickets",
            "define a new type for vendors with a name and a contact",
        ],
    )
    .await
    else {
        return;
    };
    assert!(
        changed.is_empty(),
        "Schema Creation's not_for changed its score on its own requests {changed:?}"
    );
}

/// A request to link two records, in the verbs a user says it with, must
/// lead with Relationship Management. Its earlier wording led none of these:
/// "attach the contract to the Harbor Freight account" ranked it 9th at 0.604
/// behind Graph Editing's 0.672.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn linking_requests_route_relationship_management() {
    let Some((es, ns, _tmp)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &es,
        &ns,
        &[
            "attach the contract to the Harbor Freight account",
            "connect the retro notes to the sprint they belong to",
            "link this invoice to the Acme account",
        ],
        "Relationship Management",
        true,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Relationship Management lost rank 1 for {misses:?}"
    );
}

/// The cost side of Relationship Management's `not_for`, which names
/// starting to track a kind of thing: the linking requests it wins score
/// exactly as they would without it. Three others are lowered and are not
/// here: "link this invoice to the Acme account" by 0.006 (0.793 to 0.787,
/// still first, which `linking_requests_route_relationship_management`
/// holds), "point rebuild task at the decision it has to respect" by 0.024
/// (0.848 to 0.824, second to third, still in the window, which
/// `scenario_11c_routes_relationship_management_every_rep` holds), and "this
/// task depends on the API migration" by 0.020 (0.805 to 0.785), which the
/// skill does not win either way.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn relationship_management_not_for_leaves_linking_scores_unchanged() {
    let Some(changed) = requests_its_not_for_changes(
        "Relationship Management",
        &[
            "attach the contract to the Harbor Freight account",
            "connect the retro notes to the sprint they belong to",
            "which tasks depend on the API migration?",
            "the launch task is blocked by the security review",
        ],
    )
    .await
    else {
        return;
    };
    assert!(
        changed.is_empty(),
        "Relationship Management's not_for changed its score on linking requests {changed:?}"
    );
}

/// What Relationship Management's and Organization's `not_for` exist for:
/// reworded to win their own requests, each rose on a request to start
/// tracking a kind of thing and pushed Schema Creation out of the window
/// ("start keeping tabs on who owes me money": without its `not_for`,
/// Relationship Management scores 0.827 against Schema Creation's 0.805).
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn tracking_requests_are_not_taken_by_linking_or_filing() {
    let Some((es, ns, _tmp)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &es,
        &ns,
        &[
            "start keeping tabs on who owes me money",
            "track albums I mean to listen to",
            "I need a tracker for the venues I book",
        ],
        "Schema Creation",
        false,
    )
    .await;
    assert!(
        misses.is_empty(),
        "Schema Creation missed the top-{RETRIEVAL_TOP_K} for {misses:?}"
    );
}

/// Putting records into a collection is said as filing and moving as often
/// as adding. Organization's earlier wording named only the last: "file these
/// under the Q3 folder" ranked it 10th at 0.737.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn filing_requests_route_organization() {
    let Some((es, ns, _tmp)) = seed_and_embed().await else {
        return;
    };
    let misses = routing_misses(
        &es,
        &ns,
        &[
            "file these under the Q3 folder",
            "move the recipe notes into the Cooking collection",
            "group these notes under Travel",
            "categorize these receipts as business expenses",
        ],
        "Organization",
        true,
    )
    .await;
    assert!(misses.is_empty(), "Organization lost rank 1 for {misses:?}");
}

/// The cost side of Organization's `not_for`: these filing requests score
/// exactly as they would without it. Two others are lowered and keep their
/// place: "categorize these receipts as business expenses" by 0.048 (0.821 to
/// 0.772, still first, which `filing_requests_route_organization` holds) and
/// "Add this note to my reading list collection" by 0.024 (0.833 to 0.809,
/// still in the window, which
/// `control_prompt_still_routes_organization_every_rep` holds). What the
/// `not_for` buys is held by
/// `tracking_requests_are_not_taken_by_linking_or_filing` and
/// `unrouted_find_requests_still_route_research_and_search`.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn organization_not_for_leaves_filing_scores_unchanged() {
    let Some(changed) = requests_its_not_for_changes(
        "Organization",
        &[
            "file these under the Q3 folder",
            "move the recipe notes into the Cooking collection",
            "group these notes under Travel",
        ],
    )
    .await
    else {
        return;
    };
    assert!(
        changed.is_empty(),
        "Organization's not_for changed its score on filing requests {changed:?}"
    );
}

/// Every skill that carries a `not_for`, with the live guards that measured
/// it: at least one showing the confused requests reach the right skill, and
/// one showing the skill's own requests are left as they were.
///
/// A `not_for` that misfires is quieter than a missing tool — the right
/// skill simply ranks lower — and wording that reads as equivalent was
/// measured to behave very differently. So each one is a measured decision,
/// and adding one means adding its guards and naming them here.
const NOT_FOR_GUARDS: &[(&str, &[&str])] = &[
    (
        "Schema Creation",
        &[
            "one_more_record_or_a_saved_view_does_not_lead_with_schema_creation",
            "schema_creation_not_for_leaves_tracking_scores_unchanged",
        ],
    ),
    (
        "Relationship Management",
        &[
            "tracking_requests_are_not_taken_by_linking_or_filing",
            "linking_requests_route_relationship_management",
            "relationship_management_not_for_leaves_linking_scores_unchanged",
        ],
    ),
    (
        "Organization",
        &[
            "tracking_requests_are_not_taken_by_linking_or_filing",
            "unrouted_find_requests_still_route_research_and_search",
            "filing_requests_route_organization",
            "control_prompt_still_routes_organization_every_rep",
            "organization_not_for_leaves_filing_scores_unchanged",
        ],
    ),
    (
        "Graph Editing",
        &[
            "remove_requests_mentioning_a_state_route_node_deletion",
            "removing_a_field_still_reaches_graph_editing",
            "graph_editing_not_for_leaves_completion_state_scores_unchanged",
        ],
    ),
    (
        "Play Authoring",
        &[
            "play_change_requests_route_play_authoring",
            "control_why_a_rule_has_not_fired_still_routes_play_workflow_state",
        ],
    ),
];

/// Runs in the gate, with no model: the live guards themselves are ignored
/// by default, so this is what stops a `not_for` being added without one.
#[test]
fn every_not_for_names_its_live_guards() {
    let mut carrying: Vec<String> = seed_skill_nodes()
        .into_iter()
        .filter(|t| {
            SkillFields::from_properties(&t.root_properties)
                .expect("seed decodes as a skill")
                .not_for
                .is_some()
        })
        .map(|t| t.title)
        .collect();
    carrying.sort();
    let mut guarded: Vec<String> = NOT_FOR_GUARDS
        .iter()
        .map(|(skill, _)| skill.to_string())
        .collect();
    guarded.sort();
    assert_eq!(
        carrying, guarded,
        "the skills that carry a not_for and the skills NOT_FOR_GUARDS lists must be the same"
    );

    let suites = [
        include_str!("live_skill_retrieval_stability.rs"),
        include_str!("skill_confusion_matrix.rs"),
    ];
    for (skill, guards) in NOT_FOR_GUARDS {
        assert!(!guards.is_empty(), "{skill} names no guard");
        for guard in *guards {
            // The attributes too: a function that lost them is no longer run.
            let test = format!(
                "#[tokio::test]\n#[ignore = \"requires the locked nomic-embed-text-v1.5 GGUF on \
                 disk\"]\nasync fn {guard}()"
            );
            assert!(
                suites.iter().any(|source| source.contains(&test)),
                "{skill}'s guard `{guard}` is not a live test in the suite"
            );
        }
    }
}

/// Adds whose record is named after another skill's subject, as Stage 1 words
/// them: the verb first, then the record. An embedding weighs the noun, so on
/// retrieval's own ranking each of these is led by the skill that shares it,
/// and several by one that removes user data.
const ADDS_NAMING_ANOTHER_SKILLS_SUBJECT: [&str; 10] = [
    "Add the cache invalidation decision to our architecture decisions.",
    "add cache invalidation decision to architecture decisions",
    "add the merge queue decision to our architecture decisions",
    "log a bug about the merge conflict in the sync engine",
    "add a spec for the CSV import pipeline",
    "create a task for the removal of the legacy auth module",
    "record the decision to drop the v1 API",
    "add a ticket for duplicate record detection",
    "add the data purge policy decision to our decisions",
    "add a task to delete the old log files",
];

/// An add must reach Stage 2 with a skill that holds `create_node` among its
/// first [`RETRIEVAL_TOP_K`] candidates, and with no skill that removes user
/// data among them at all.
///
/// "add cache invalidation decision to architecture decisions" is what Stage 1
/// made of the request in the first entry. Retrieval ranked it Node Deletion
/// 0.816, Play Workflow State 0.801, Node Merge 0.787, Bulk Import 0.768, and
/// Node Creation sixth at 0.763: the turn was offered `delete_node` and not
/// `create_node`, searched twice, and said it could not add the record. No
/// description wording separates an add from a removal that names the same
/// thing, so the verb is read off the query (`routing::is_add_shaped`) and the
/// candidates follow from it (`routing::retrieve_candidates`).
///
/// The second search runs for three of these: the two cache-invalidation
/// wordings, and "add the offline sync decision to our architecture
/// decisions", which collides with nothing and still ranked Play Authoring,
/// Schema Creation and Conflict Journal ahead of both skills that can create.
/// The rest find one once the deletion and merge skills are out of the
/// ranking.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn adds_naming_another_skills_subject_reach_a_skill_that_can_create() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let mut misses = Vec::new();
    for query in ADDS_NAMING_ANOTHER_SKILLS_SUBJECT
        .into_iter()
        .chain(["add the offline sync decision to our architecture decisions"])
    {
        assert!(is_add_shaped(query), "{query:?} is not shaped like an add");
        let judged = routed_candidates(&embedding_service, &node_service, query).await;
        eprintln!(
            "{query:?}: {:?}",
            judged
                .iter()
                .map(|c| format!("{}={:.3}", c.name, c.score))
                .collect::<Vec<_>>()
        );
        let can_create = judged
            .iter()
            .take(RETRIEVAL_TOP_K)
            .any(skill_can_create_a_record);
        let destructive = judged.iter().any(skill_is_destructive);
        let destructive_lead =
            leading_tool_bearing_candidate(&judged).is_some_and(skill_is_destructive);
        if !can_create || destructive || destructive_lead {
            misses.push(query);
        }
    }
    assert!(
        misses.is_empty(),
        "no skill holding create_node reached the top-{RETRIEVAL_TOP_K}, or a skill that removes \
         user data was a candidate, for {misses:?}"
    );
}

/// The reason for the guard above, kept measurable: on retrieval's own ranking
/// the first two adds reach Stage 2 with a skill that removes user data in the
/// lead and none that can create. If a description change ever fixes that in
/// retrieval, this fails and the rule in `routing::retrieve_candidates` can be
/// weighed against the simpler ranking.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn without_the_add_rule_those_adds_lead_with_a_skill_that_removes() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    for query in &ADDS_NAMING_ANOTHER_SKILLS_SUBJECT[..2] {
        let judged = select_candidates(
            retrieved_candidates(&embedding_service, &node_service, query, RETRIEVAL_FETCH).await,
        );
        assert!(
            leading_tool_bearing_candidate(&judged).is_some_and(skill_is_destructive)
                && !judged.iter().any(skill_can_create_a_record),
            "retrieval alone now serves {query:?}; judged: {:?}",
            judged.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }
}

/// The cost side of reading the verb: a request that opens with an adding
/// verb and belongs to another skill must still reach that skill, in the
/// place it had. The rule only takes the deletion and merge skills out of the
/// ranking and adds a creating skill where none was judged, so each of these
/// is led by the skill that led it on retrieval alone.
#[tokio::test]
#[ignore = "requires the locked nomic-embed-text-v1.5 GGUF on disk"]
async fn add_shaped_requests_for_another_skill_keep_their_leader() {
    let Some((embedding_service, node_service, _temp_dir)) = seed_and_embed().await else {
        return;
    };
    let mut moved = Vec::new();
    for (query, leader) in [
        ("add a priority field to the ticket type", "Schema Creation"),
        (
            "create a database for tracking our feature specs",
            "Schema Creation",
        ),
        (
            "add a rule to the triage play that assigns new bugs to Priya",
            "Play Authoring",
        ),
        (
            "record an edge between the rebuild task and the storage decision",
            "Relationship Management",
        ),
        ("create nodes from this markdown document", "Bulk Import"),
    ] {
        assert!(is_add_shaped(query), "{query:?} is not shaped like an add");
        let before = select_candidates(
            retrieved_candidates(&embedding_service, &node_service, query, RETRIEVAL_FETCH).await,
        );
        let after = routed_candidates(&embedding_service, &node_service, query).await;
        eprintln!(
            "{query:?}: {:?}",
            after
                .iter()
                .map(|c| format!("{}={:.3}", c.name, c.score))
                .collect::<Vec<_>>()
        );
        let lead = |judged: &[SkillCandidate]| {
            leading_tool_bearing_candidate(judged).map(|c| c.name.clone())
        };
        if lead(&after).as_deref() != Some(leader) || lead(&before) != lead(&after) {
            moved.push(query);
        }
    }
    assert!(
        moved.is_empty(),
        "the add rule moved the leader of {moved:?}"
    );

    // An add to a collection keeps the skill that holds `create_relationship`.
    let query = "Add this note to my reading list collection";
    let judged = routed_candidates(&embedding_service, &node_service, query).await;
    assert!(
        judged.iter().any(|c| c.name == "Organization"),
        "Organization must reach Stage 2 for {query:?}; judged: {:?}",
        judged.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
}
