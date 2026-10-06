//! Skill discovery operations.
//!
//! Shared logic for skill search used by the local agent's `search_skills`
//! tool and the MCP `find_skills` handler exposed to external agents.

use crate::behaviors::ToolOrigin;
use crate::models::{CoreNodeType, Node, SchemaNode, SkillFields, SkillRole, SKILL_APPLIES_TO};
use crate::services::{render_subtree_markdown, NodeEmbeddingService, NodeService};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

use super::OpsError;

/// Similarity threshold for skill search.
///
/// Set to zero so the model sees every match with strictly positive cosine
/// similarity, including weak ones, and decides for itself which (if any)
/// skill is relevant. The underlying store filter is `composite_score >
/// $threshold`, so a zero match (orthogonal vector) is still excluded — but
/// that's the cosine noise floor, not a confidence judgment call.
/// Server-side bucketing was explicitly removed in favour of letting the
/// LLM judge confidence from the raw score; a non-zero floor here would
/// partially undo that by silently hiding the long tail.
const SKILL_SEARCH_THRESHOLD: f32 = 0.0;

/// Maximum schemas to include in `schema_metadata` when the matched skill
/// links to no schema. Bounds token cost for general-purpose skills. A skill
/// with `applies_to` links carries exactly its linked schemas, uncapped.
const MAX_UNSCOPED_SCHEMA_METADATA: usize = 5;

/// Upper bound on `limit` requested by the caller.
///
/// Skill libraries are small in practice (~8-20 seeded skills plus a handful
/// of user-defined ones). A cap of 10 is large enough to expose every skill
/// in a typical workspace yet keeps the response token-cheap for small local
/// models. Revisit if user-defined skill libraries grow past ~30 skills.
const MAX_SKILL_LIMIT: usize = 10;

/// How many skills `find_skills` scores before applying `not_for` penalties
/// and truncating to the caller's `limit`.
///
/// A penalty only ever lowers a skill, so it can promote a skill that ranked
/// below `limit` on raw similarity. The promotion is exact when the pool holds
/// every skill, which needs a registry of at most this many skills (the typed
/// search scores every skill, see `search_embeddings_by_node_type`). Past
/// that, a skill ranked below the pool on raw similarity cannot be promoted.
/// Four times [`MAX_SKILL_LIMIT`] covers the built-in registry (twenty
/// skills) with room for the skills a user writes.
const SKILL_RERANK_POOL: usize = 4 * MAX_SKILL_LIMIT;

/// Weight on a skill's `not_for` margin in [`not_for_penalized_score`].
///
/// Measured on the locked embedding model against the full seeded registry:
/// at 1.0 Graph Editing's `not_for` puts Node Deletion first on "remove the
/// resolved tickets" by +0.047 (it ranked second, −0.014, without one). An
/// offline sweep at 0.5 left a margin under +0.01, too thin to hold.
const NOT_FOR_PENALTY_WEIGHT: f64 = 1.0;

/// Confidence assigned to a schema recovered by the lexical backstop
/// (`append_named_schema_candidates`) rather than found by semantic search.
///
/// A deterministic, word-boundary name match is a stronger signal than a
/// typical cosine score, and — unlike a genuine semantic hit — must not be
/// left to chance on whether it happens to clear
/// `nodespace_agent::local_agent::routing`'s score gate. Pinned at the top of
/// the scale for the same reason `context_ops::append_schemas_named_in_query`
/// unconditionally injects its own recoveries rather than scoring them.
const LEXICAL_SCHEMA_MATCH_CONFIDENCE: f64 = 1.0;

/// The score a schema-search match needs before a guidance fetch returns it.
///
/// The schema search has no floor and always returns its nearest types, so
/// on its own it hands back a type for every request, related or not.
/// Measured on the locked embedding model, fifteen requests against a
/// workspace with four custom types (two with a skill linked, two without):
///
/// - a type the request was about scored 0.855 to 1.000: "bill the client
///   for the March work" put `invoice` at 0.908, "how many people does the
///   hall hold" put `venue` at 0.855;
/// - a type it was not about scored 0.633 to 0.829: "delete a node" put
///   `cycle` at 0.757, and the highest, "link the rebuild task to the
///   decision it depends on", put `cycle` at 0.829.
///
/// One request about a type fell among the unrelated ones and is not
/// returned: "record what Acme owes us for the redesign" put `invoice` at
/// 0.789, with `cycle` at 0.763 beside it. No bar separates those two.
///
/// The margin is thin (0.855 over 0.829), and it is specific to this model
/// and to the document prefix queries are embedded with: re-measure when
/// either changes. A match below the bar still reaches the reader when a
/// returned skill is linked to the type or the request names it.
const GUIDANCE_SCHEMA_SCORE_BAR: f64 = 0.85;

/// Input for find_skills operation.
#[derive(Debug)]
pub struct FindSkillsInput {
    pub query: String,
    pub limit: Option<usize>,
}

/// Output for find_skills operation.
#[derive(Debug)]
pub struct FindSkillsOutput {
    pub skills: Vec<Value>,
    pub query: String,
    pub total_results: usize,
}

/// Render a node's child subtree as markdown, via the shared
/// ADR-057 subtree-render utility (`render_subtree_markdown`).
///
/// Fetches the full subtree in a single DB query, then walks it depth-first
/// (root children first, their children next), restoring list markers the
/// import stored as structure. `root_id` itself is excluded — callers
/// already have whatever flat metadata (name, `description` property) lives
/// directly on the root node.
///
/// Empty/childless subtrees return an empty string without error. Shared by
/// [`render_skill_instructions`] (a skill's procedure subtree) and
/// [`render_schema_description`] (a schema's own description subtree,
/// `crate::models::schema_node`'s doc comment) — both are "a node's markdown
/// child subtree flattened to text," differing only in which root and what
/// the caller does with the result. Reusing this one function is what keeps
/// there from being a third independent subtree-render implementation.
async fn render_node_subtree(node_service: &NodeService, root_id: &str) -> String {
    let (_, node_map, adjacency_list) = match node_service.get_subtree_data(root_id).await {
        Ok(data) => data,
        Err(e) => {
            tracing::warn!(
                error = %e,
                root_id = %root_id,
                "render_node_subtree: failed to fetch subtree"
            );
            return String::new();
        }
    };

    render_subtree_markdown(root_id, &node_map, &adjacency_list)
}

/// Render a skill node's child subtree as markdown — the actual
/// procedure the model must follow.
///
/// One `get_subtree_data` query per skill. Acceptable for a search, which
/// renders at most `MAX_SKILL_LIMIT = 10`; a batch API would eliminate serial
/// round trips if the limit grows. A failed read renders as no procedure.
async fn render_skill_instructions(node_service: &NodeService, skill_id: &str) -> String {
    render_node_subtree(node_service, skill_id).await
}

/// A skill's procedure for the list's version, where a failed read is an
/// error: an empty body in its place would give a version that says the
/// skill changed.
///
/// One query per skill, for every skill, each time the list is read. Skill
/// libraries are tens of nodes; a batch read is the fix if they grow.
async fn read_skill_instructions(
    node_service: &NodeService,
    skill_id: &str,
) -> Result<String, OpsError> {
    let (_, node_map, adjacency_list) = node_service
        .get_subtree_data(skill_id)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to read the skill {skill_id}: {e}")))?;
    Ok(render_subtree_markdown(
        skill_id,
        &node_map,
        &adjacency_list,
    ))
}

/// Render a schema node's own description subtree as markdown.
///
/// A schema's description is authored as markdown and stored as a child
/// subtree (parsed into text/header nodes), not as a flat property — see
/// `crate::models::schema_node`'s module doc comment. That subtree already
/// feeds the schema's embedding for semantic *retrieval*
/// (`SchemaNodeBehavior::get_aggregated_content`), but retrieval only helps a
/// schema be *found*; it does not deliver the description's actual content to
/// the model once found. This is that delivery path.
async fn render_schema_description(node_service: &NodeService, schema_id: &str) -> String {
    render_node_subtree(node_service, schema_id).await
}

/// One schema in the `schema_metadata` form: its fields and relationships
/// (each with its own `description`, via `EntityTypeDescriptor::to_json`),
/// plus the schema's own description subtree as a sibling `description` key.
///
/// Encoded from the same descriptor the prompt block renders from, so this
/// JSON cannot describe a schema differently than the model is told about it.
/// `description_cache` holds the description subtrees already read in this
/// call: a schema's cannot change mid-call, and the same schema commonly
/// appears for more than one skill.
async fn schema_definition(
    node_service: &NodeService,
    schema: &SchemaNode,
    all_schemas: &[SchemaNode],
    description_cache: &mut HashMap<String, String>,
) -> Value {
    let mut entry =
        super::entity_types_block::EntityTypeDescriptor::from_corpus(schema, all_schemas).to_json();

    let description = match description_cache.get(&schema.envelope.id) {
        Some(cached) => cached.clone(),
        None => {
            let rendered = render_schema_description(node_service, &schema.envelope.id).await;
            description_cache.insert(schema.envelope.id.clone(), rendered.clone());
            rendered
        }
    };
    if !description.is_empty() {
        entry["description"] = json!(description);
    }
    entry
}

/// Whether `phrase` (already lowercased) appears in `haystack` (already
/// lowercased) at word boundaries — not as a substring of a longer word.
///
/// Both `phrase` and `haystack` are tokenized identically, splitting on any
/// non-alphanumeric character (not only whitespace) — a schema id like
/// `release_plan` or `pull-request` must match a query that spells it with
/// spaces, and vice versa, since ids and queries are not guaranteed to use
/// the same separator. Single-token phrases (the common case: a type id
/// like `ticket`) are checked by exact token match. Multi-token phrases (an
/// id or display name like `release_plan` / `Pull Request`) are checked as
/// a contiguous run of exact tokens, so `release` alone does not count as a
/// match.
pub(crate) fn mentions_phrase(haystack: &str, phrase: &str) -> bool {
    fn tokenize(s: &str) -> Vec<&str> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect()
    }
    let words = tokenize(phrase);
    if words.is_empty() {
        return false;
    }
    let tokens = tokenize(haystack);
    if words.len() == 1 {
        tokens.iter().any(|t| *t == words[0])
    } else {
        tokens
            .windows(words.len())
            .any(|window| window.iter().zip(&words).all(|(t, w)| t == w))
    }
}

/// The single non-core schema `query` names by id or display name, when
/// exactly one is nameable this way.
///
/// A purely mechanical, string-level signal — not a new relevance model or a
/// confidence judgment. Two or more named types (the query mentions several
/// non-core types by name) or none (it names none) both return `None`: real
/// ambiguity is left to the caller's existing broader fallback rather than
/// guessed at here.
///
/// This is the narrower, per-query retrieval pass this module implements: a
/// request that plainly says "create a ticket" scopes `schema_metadata` to
/// just `ticket` instead of `find_skills`' unscoped top-N fallback, which
/// would otherwise sweep in every other non-core type in the same fallback
/// window (e.g. `adr`) — the exact imprecision that caused
/// `declare_write_tool_fields` to union unrelated types' fields onto a write
/// tool's declaration. A skill's `applies_to` links (the other mechanism
/// `find_skills` supports) cannot fix this case: the seeded skills
/// whose whitelist includes `create_node`/`update_node` (Node Creation,
/// Graph Editing) are deliberately generic across every non-core type, so
/// there is no single static type list to give them without contradicting
/// their purpose. This mechanism narrows per query instead, so a generic
/// skill still contributes a scoped `schema_metadata` when the query itself
/// determines the type.
///
/// **Known, accepted tradeoff, measured empirically (see below)**: a lexical match can
/// be a false positive if a user-defined schema's id or display name
/// happens to also be a common word or phrase (e.g. a schema literally
/// named `Note`), narrowing to a confidently WRONG type rather than the old
/// broad-but-safe top-N fallback.
///
/// This was measured rather than guessed at (see
/// `schema_named_in_query_measured_zero_false_positives_on_precedent_schema_names`
/// and `schema_named_in_query_measured_false_positive_rate_on_risky_common_word_names`
/// below for the corpora and pinned aggregate results):
///
/// - Every non-core schema name with actual precedent in this codebase
///   (`ticket`, `adr`, `release`, `release_plan`, `venue`, `invoice`,
///   `equipment` — drawn from `packages/agent/goldens/*.toml`, this
///   module's own tests, and the `Venue`/`Invoice` examples in ADR-063 and
///   `schema-management.md`) measures a **0% false-positive rate** against
///   a 52-query corpus spanning both this project's real dev-workflow
///   queries and everyday PKM-assistant chatter. For domain-specific names,
///   the risk is not merely low, it did not reproduce at all.
/// - It is NOT low for schema names that are themselves common English
///   words — plausible names in NodeSpace's own domain, a personal
///   knowledge tool rather than only a dev tracker (`Note`, `Meeting`,
///   `Idea`, `Goal`, `Item`, `Event`, ...). That class measures a real,
///   reproducible **2.2% false-positive rate** (33/1508 pairs over 29 such
///   names), including the review's own worked example verbatim: a schema
///   named `Note` narrows on "please note that the meeting moved to 3pm".
/// - No lightweight guard was added despite that, because none survives
///   the same measurement: every one of those 29 risky words is ALSO the
///   exact word a genuine positive query for that same type would use (a
///   `Log` schema is found by "add a log for today's workout" using the
///   identical token that falsely matches "log me out of this session"). A
///   stopword list or length floor removes the false positives and the true
///   positives together — there is no lexical signal in `mentions_phrase`'s
///   input that tells them apart, so filtering here would just trade one
///   failure mode for a different, equally silent one (a real `Note` schema
///   query silently losing its narrowing, indistinguishable from one that
///   never had it).
///
/// Left as-is rather than guarded, because the failure mode is bounded even
/// in the worst case: this only affects one prompt-assist channel's
/// precision (`schema_metadata`, which shapes a write tool's declared
/// `field_values` sub-schema). That sub-schema is never closed —
/// `with_declared_field_values` (`packages/agent/src/local_agent/tools.rs`)
/// sets no `additionalProperties: false`, and `field_values`'s own
/// description explicitly instructs the model to add any key the user
/// supplied that the declared list doesn't cover — so a wrong narrow
/// degrades to "no typed hint for the correct type's fields on this turn,
/// plus some irrelevant ones," never to "the model cannot express the
/// correct write." Write validation still enforces the real schema at the
/// write boundary regardless of what this function returns. Revisit if
/// real usage ever shows this actually misfiring in practice — there is
/// none to measure against yet.
fn schema_named_in_query<'a>(
    query: &str,
    all_schemas: &'a [crate::models::SchemaNode],
) -> Option<&'a crate::models::SchemaNode> {
    let query_lower = query.to_lowercase();
    let mut named = all_schemas.iter().filter(|s| {
        !s.is_core
            && (mentions_phrase(&query_lower, &s.envelope.id.to_lowercase())
                || mentions_phrase(&query_lower, &s.envelope.content.to_lowercase()))
    });

    let first = named.next()?;
    if named.next().is_some() {
        // Two or more non-core types named in the same query — genuinely
        // ambiguous by this signal. Not this function's job to break the tie.
        None
    } else {
        Some(first)
    }
}

/// Every retrieved skill's name and raw score, for the `all_scores` log
/// field — not only the winner's `top_score`.
///
/// This is the two-part diagnostic ADR-038's routing work needs: a near-tied
/// runner-up (a tiebreak, fixed by retrieval-shape changes) reads identically
/// to a skill with no close competitor scoring badly on its own core use case
/// (a description problem) if only the top score is logged, and those call
/// for different fixes. See
/// `nodespace_agent::local_agent::routing::all_candidate_scores` for the same
/// field on the local-agent Stage-2 path — this is `skill_ops`'s own copy
/// rather than a shared helper, since the two operate on different types
/// (`SkillCandidate` there, `(Node, f32)` tuples here) across a crate
/// boundary `core` cannot depend on `agent` to cross.
///
/// A free function rather than inline formatting specifically so the
/// unfiltered-inclusion behavior is independently testable without spinning
/// up `find_skills`'s live `NodeService`/`NodeEmbeddingService` dependencies.
fn format_all_scores(skill_results: &[(nodespace_types::Node, f64)]) -> String {
    skill_results
        .iter()
        .map(|(node, score)| format!("{}={:.3}", node.content, score))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve `schema`-typed semantic-search hits to the non-core schemas they
/// name in `corpus`, keeping each hit's retrieval confidence. A hit is a raw
/// node row, which holds none of a schema's declaration edges; the corpus
/// schema does.
///
/// Mirrors `context_ops::non_core_schema_hits`'s filter and the
/// reason for it: a core type (`text`/`task`/`date`, ...) is a stored schema
/// node with embeddable content, so an unfiltered pass-through would surface
/// it here as if it were a user-defined discovery result. Kept as this
/// module's own copy rather than a shared helper because the two return
/// differently-shaped results — this one keeps each hit's score, which the
/// `context_ops.rs` caller (building a resident prompt block, not a scored
/// candidate list) has no use for.
fn non_core_schema_hits_with_scores(
    results: Vec<(nodespace_types::Node, f64)>,
    corpus: &[crate::models::SchemaNode],
) -> Vec<(crate::models::SchemaNode, f64)> {
    results
        .into_iter()
        .filter_map(|(node, score)| {
            corpus
                .iter()
                .find(|s| s.envelope.id == node.id)
                .filter(|s| !s.is_core)
                .map(|s| (s.clone(), score))
        })
        .collect()
}

/// Append any non-core schema the query names outright, skipping ones
/// already present — the lexical backstop for schema discovery.
///
/// Mirrors `context_ops::append_schemas_named_in_query`'s reasoning exactly,
/// against the same gap: schema embeddings run on a ~30s debounce
/// (`EmbeddingService`'s `debounce_duration_secs`), so a schema created
/// moments ago is not yet semantically retrievable. Without this, a schema
/// created and then immediately searched for by name would look, through
/// this discovery path, indistinguishable from a schema that does not exist
/// at all — the same failure mode `context_ops.rs`'s own backstop was added
/// to close for the resident "EXISTING SCHEMAS" block. A query that names a
/// schema outright is resolvable deterministically regardless of embedding
/// timing, via the same word-boundary matcher (`mentions_phrase`) that
/// backs `schema_named_in_query` above.
///
/// Appends rather than replaces: semantic retrieval and naming answer
/// different questions, and a type named outright in the query is the
/// strongest available evidence that it is relevant right now.
fn append_named_schema_candidates(
    mut hits: Vec<(crate::models::SchemaNode, f64)>,
    all_schemas: &[crate::models::SchemaNode],
    query: &str,
) -> Vec<(crate::models::SchemaNode, f64)> {
    let query_lower = query.to_lowercase();
    for schema in all_schemas.iter().filter(|s| !s.is_core) {
        let named = mentions_phrase(&query_lower, &schema.envelope.id.to_lowercase())
            || mentions_phrase(&query_lower, &schema.envelope.content.to_lowercase());
        if named
            && !hits
                .iter()
                .any(|(s, _)| s.envelope.id == schema.envelope.id)
        {
            hits.push((schema.clone(), LEXICAL_SCHEMA_MATCH_CONFIDENCE));
        }
    }
    hits
}

/// The schemas a skill's `applies_to` links put in scope: each linked schema
/// and every schema that extends one, directly or through a chain. A subtype
/// is its base type, so guidance about `task` is guidance about `issue` too.
/// Core schemas count: a link names its target outright, unlike the unlinked
/// fallback, which offers custom types only.
///
/// In `all_schemas` order. A link whose target is not among `all_schemas`
/// (an edge to something that is not a schema) contributes nothing.
fn linked_schemas<'a>(targets: &[String], all_schemas: &'a [SchemaNode]) -> Vec<&'a SchemaNode> {
    let is_linked = |schema: &SchemaNode| {
        // Bounded by the corpus size, so a corrupt `extends` cycle ends.
        let mut current = Some(schema);
        for _ in 0..=all_schemas.len() {
            let Some(s) = current else { break };
            if targets.contains(&s.envelope.id) {
                return true;
            }
            current = s
                .extends
                .as_deref()
                .and_then(|parent| all_schemas.iter().find(|p| p.envelope.id == parent));
        }
        false
    };
    all_schemas.iter().filter(|s| is_linked(s)).collect()
}

/// A skill's retrieval score after its `not_for` is applied.
///
/// A skill may carry a `not_for`: requests that belong to another skill.
/// It cannot go in `use_for`, because `use_for` is embedded and an
/// embedding has no negation — "not for deleting" embeds *near* deleting. So
/// `not_for` is embedded on its own and compared with the query, and the
/// skill loses score by however much the query matches its `not_for` better
/// than its `use_for`:
///
/// `score − λ · max(0, not_for_score − score)`
///
/// The margin form is deliberate. Subtracting the `not_for` similarity
/// outright would lower the skill on every query — unrelated texts on this
/// model still score around 0.8 — shifting it against every skill without an
/// `not_for` and through the absolute score bars in routing. Here a query
/// closer to `use_for` than to `not_for` is left exactly as it was,
/// so the penalty acts only where the two genuinely overlap.
fn not_for_penalized_score(score: f64, not_for_score: f64) -> f64 {
    score - NOT_FOR_PENALTY_WEIGHT * (not_for_score - score).max(0.0)
}

/// Apply each skill's `not_for` (see [`not_for_penalized_score`]) to the
/// raw similarity ranking, then re-rank and truncate to `limit`.
///
/// A `not_for` that fails to embed leaves that skill's score unchanged: a
/// missing penalty degrades to the ranking retrieval had before `not_for`
/// existed, rather than failing the whole search.
fn rerank_with_not_for(
    embedding_service: &NodeEmbeddingService,
    query_vector: &[f32],
    pool: Vec<(crate::models::Node, f64)>,
    limit: usize,
) -> Vec<(crate::models::Node, f64)> {
    let mut scored: Vec<(crate::models::Node, f64)> = pool
        .into_iter()
        .map(|(node, score)| {
            let Some(not_for) = SkillFields::from_node(&node)
                .ok()
                .and_then(|skill| skill.not_for)
            else {
                return (node, score);
            };
            // Same shape `SkillNodeBehavior::get_embeddable_content` gives the
            // `use_for` (name, blank line, text), so the two vectors share
            // the name and differ only in what the skill does versus what it
            // excludes. Embedded bare, the `not_for` scored closer to
            // completion-state requests than `use_for` did, and
            // lowered Graph Editing on "mark the outage report done" out of
            // the top 3.
            let not_for_text = format!("{}\n\n{}", node.content, not_for);
            match embedding_service.nlp_engine().embed_document(&not_for_text) {
                Ok(not_for_vector) => {
                    // Scored as a single fully-matching chunk, the same
                    // composite a one-chunk skill node gets in the KNN search.
                    // Assumes the skill's own embedding is one chunk too, as
                    // every seeded skill's is. A `use_for` long enough to
                    // split scores below that full-density composite, so its
                    // `not_for` would weigh more than the same text on a
                    // short skill.
                    let not_for_score = crate::db::composite_similarity_score(
                        crate::db::cosine_similarity(query_vector, &not_for_vector),
                        1,
                        1,
                    );
                    let adjusted = not_for_penalized_score(score, not_for_score);
                    if adjusted < score {
                        tracing::debug!(
                            skill = %node.content,
                            score,
                            not_for_score,
                            adjusted,
                            "find_skills: not_for lowered a skill's score"
                        );
                    }
                    (node, adjusted)
                }
                Err(e) => {
                    tracing::warn!(
                        skill = %node.content,
                        error = %e,
                        "find_skills: failed to embed a skill's not_for; scoring without it"
                    );
                    (node, score)
                }
            }
        })
        .filter(|(_, score)| *score > f64::from(SKILL_SEARCH_THRESHOLD))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    in_lanes(scored, limit)
}

/// Whether a skill is ranked in the procedure lane. A skill whose fields do
/// not decode is not: `find_skills` skips it later.
fn is_procedure(node: &crate::models::Node) -> bool {
    SkillFields::from_node(node)
        .map(|skill| skill.role == SkillRole::Procedure)
        .unwrap_or(false)
}

/// Fill the result from the two lanes (ADR-038): the best `limit` `tool`
/// skills, then the single best `procedure` skill, each lane in the order it
/// arrived. A registry of procedures therefore never changes which tool
/// skills come back or where.
fn in_lanes(
    ranked: Vec<(crate::models::Node, f64)>,
    limit: usize,
) -> Vec<(crate::models::Node, f64)> {
    let (procedures, mut tools): (Vec<_>, Vec<_>) =
        ranked.into_iter().partition(|(node, _)| is_procedure(node));
    tools.truncate(limit);
    tools.extend(procedures.into_iter().take(1));
    tools
}

/// Search for skill nodes via semantic search and return flat results with
/// schema metadata for the matched skill's scoped types.
///
/// Returns up to `limit` matches (default 3) with `id`, `name`, `kind`,
/// `use_for`, `confidence`, `tools`, `schema_metadata`, and `instructions`.
/// The `instructions` field is the skill's child subtree rendered to markdown
/// — the actual procedure the model must follow. The `schema_metadata` field
/// contains type IDs, field names, and enum values for the schemas the skill
/// is about: its `applies_to` targets and their subtypes, or, for a skill
/// with no links, the custom type the query names (else up to
/// [`MAX_UNSCOPED_SCHEMA_METADATA`] custom types).
///
/// Also searches `schema`-typed nodes directly (`kind: "schema"`), so a
/// schema with no hand-authored skill describing it is still discoverable
/// through this same query-driven path — previously it produced no result
/// here at all, regardless of how well-described the schema itself was.
/// Reuses the exact semantic-search primitive
/// (`semantic_search_nodes_of_type(query, "schema", ...)`) that
/// `context_ops.rs`'s resident "EXISTING SCHEMAS" block already indexes
/// from, rather than building a second index, and the same lexical backstop
/// pattern for the ~30s embedding-debounce window (see
/// `append_named_schema_candidates`). A `kind: "schema"` result carries no
/// `tools` (empty) and no `instructions` (a schema has neither a
/// tool_whitelist nor a guidance subtree) — its `schema_metadata` is the one
/// matched schema's own descriptor, so the result is exactly what the query
/// asked about rather than the broader unscoped fallback a matched skill's
/// `schema_metadata` falls back to.
///
/// No filtering or bucketing — the caller (model or MCP client) inspects the
/// raw confidence score and decides how to act. An empty `skills` array is a
/// meaningful signal: "no skill or schema is even loosely related to this
/// query."
pub async fn find_skills(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    input: FindSkillsInput,
) -> Result<FindSkillsOutput, OpsError> {
    let limit = input.limit.unwrap_or(3).min(MAX_SKILL_LIMIT);

    // Skill selection is a small, closed-candidate classification problem
    // (~8-20 skill nodes), not open-ended document recall — so it uses
    // `semantic_search_nodes_of_type`'s exact linear-scan KNN over just the
    // `skill` type, rather than `semantic_search_nodes`'s hybrid BM25+KNN
    // tiering. That tiering orders results tier1(BM25∩KNN) ++ tier2(KNN-only)
    // ++ tier3(BM25-only) — a hard partition, not a blended score — so any
    // BM25 hit outranks every KNN-only result regardless of score magnitude.
    // BM25 there also matches into a skill's full guidance-markdown subtree,
    // not just its title/description, so an incidental keyword collision in
    // instructional prose (irrelevant to the query's actual intent) could
    // rank a weakly-matching skill above the true best semantic match. Pure
    // KNN cosine ranking over skill roots avoids both: no BM25 involvement at
    // all, and no children in scope (only `node_type = 'skill'` roots are
    // indexed by this query).
    //
    // The query is embedded once and shared by the skill search, the schema
    // search below, and the `not_for` scoring in `rerank_with_not_for`.
    let query_vector = embedding_service
        .embed_query_text(&input.query)
        .map_err(|e| OpsError::Internal(format!("Skill search failed: {}", e)))?;
    let skill_pool = embedding_service
        .semantic_search_nodes_of_type_with_vector(
            &query_vector,
            "skill",
            SKILL_RERANK_POOL.max(limit),
            SKILL_SEARCH_THRESHOLD,
        )
        .await
        .map_err(|e| OpsError::Internal(format!("Skill search failed: {}", e)))?;
    let skill_results = rerank_with_not_for(embedding_service, &query_vector, skill_pool, limit);

    // Fetch all schemas once; used to attach metadata to each matched skill
    // AND (below) to resolve and lexically backstop the schema search.
    let all_schemas = node_service
        .get_all_schemas()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "find_skills: failed to fetch schemas for metadata attachment");
        })
        .unwrap_or_default();

    // Schema discovery, independent of any hand-authored skill matching the
    // query — see this function's own doc comment. Same primitive
    // (`semantic_search_nodes_of_type`), same node type ("schema"), same
    // threshold as the skill search above; the two are deliberately
    // separate calls rather than a combined query because they populate two
    // differently-shaped result kinds below.
    //
    // Every schema is ranked, and `limit` applies after core types are
    // dropped: core schemas are embedded too, so a limit on the search itself
    // would let them take the places of the custom types this is for.
    let schema_search_results = embedding_service
        .semantic_search_nodes_of_type_with_vector(
            &query_vector,
            "schema",
            all_schemas.len().max(limit),
            SKILL_SEARCH_THRESHOLD,
        )
        .await
        .map_err(|e| OpsError::Internal(format!("Schema search failed: {}", e)))?;
    let mut schema_hits = non_core_schema_hits_with_scores(schema_search_results, &all_schemas);
    schema_hits.truncate(limit);

    let schema_candidates = append_named_schema_candidates(schema_hits, &all_schemas, &input.query);

    let mut skills = Vec::with_capacity(skill_results.len() + schema_candidates.len());

    // Computed once per call, not per matched skill: it depends only on the
    // query text and the schema list, neither of which varies across
    // `skill_results`. See `schema_named_in_query` — `None` when the query
    // doesn't determine a single non-core type, in which case every
    // unscoped-branch candidate below keeps today's fallback unchanged.
    let query_named_schema = schema_named_in_query(&input.query, &all_schemas);

    // Every candidate's `applies_to` links, read together rather than once
    // per skill. A failed read degrades to the unlinked fallback for this
    // call instead of failing the search.
    let skill_ids: Vec<String> = skill_results.iter().map(|(n, _)| n.id.clone()).collect();
    let applies_to = node_service
        .store()
        .get_edge_targets_by_source(&skill_ids, SKILL_APPLIES_TO)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "find_skills: failed to read applies_to links");
        })
        .unwrap_or_default();

    // Schema-description-subtree fetches are cached per call: the same
    // schema commonly appears in `schema_metadata` for more than one matched
    // skill (e.g. every generic node-creation skill scoped to it), and its
    // description subtree cannot change mid-call, so re-fetching it per skill
    // would be a repeat DB round trip for identical content.
    let mut schema_description_cache: HashMap<String, String> = HashMap::new();

    for (node, confidence) in &skill_results {
        // `not_for` was spent on ranking in `rerank_with_not_for`; it is
        // retrieval-only and never reaches the model.
        let fields = match SkillFields::from_node(node) {
            Ok(skill) => skill,
            Err(e) => {
                // `SkillNodeBehavior::validate` rejects this shape on write,
                // so only a raw store write can produce it. Leave it out
                // rather than hand the turn a guessed tool set.
                tracing::warn!(skill_id = %node.id, error = %e, "find_skills: skipping malformed skill node");
                continue;
            }
        };

        // Entity types relevant to this skill: the schemas its `applies_to`
        // links name, plus their subtypes. With no link that resolves to a
        // schema: if the query itself names exactly one non-core type, scope
        // to that type (see `schema_named_in_query`); otherwise fall back to
        // all custom (non-core) schemas, capped at
        // MAX_UNSCOPED_SCHEMA_METADATA to bound token cost for
        // general-purpose skills whose query didn't resolve to one type.
        let linked = applies_to
            .get(&node.id)
            .map(|targets| linked_schemas(targets, &all_schemas))
            .unwrap_or_default();
        // Whether `schema_metadata` below is the linked set rather than a
        // fallback. A linked set is what the skill is about, so a consumer may
        // hold a turn to it; a fallback is a guess at relevance.
        let schemas_linked = !linked.is_empty();
        let schema_candidates: Vec<&SchemaNode> = if schemas_linked {
            linked
        } else {
            match query_named_schema {
                Some(named) => vec![named],
                None => all_schemas
                    .iter()
                    .filter(|s| !s.is_core)
                    .take(MAX_UNSCOPED_SCHEMA_METADATA)
                    .collect(),
            }
        };

        // Build `schema_metadata`: each candidate's fields/relationships
        // (with their own `description`, via `EntityTypeDescriptor::to_json`)
        // plus the schema's own description subtree, fetched and merged in
        // as a sibling `description` key on the same entry — the schema-level
        // counterpart to the field/relationship-level descriptions already
        // carried by `to_json`.
        let mut schema_metadata: Vec<Value> = Vec::with_capacity(schema_candidates.len());
        for schema in schema_candidates {
            schema_metadata.push(
                schema_definition(
                    node_service.as_ref(),
                    schema,
                    &all_schemas,
                    &mut schema_description_cache,
                )
                .await,
            );
        }

        let instructions = render_skill_instructions(node_service.as_ref(), &node.id).await;

        skills.push(skill_entry(
            node,
            &fields,
            *confidence,
            schema_metadata,
            schemas_linked,
            instructions,
        ));
    }

    // Schema-typed hits: a shape distinguishable from a skill's ("kind":
    // "schema", no `tools`, no `instructions` — a schema owns neither a
    // tool_whitelist nor a guidance subtree). `schema_metadata` carries
    // exactly this one matched schema (not the unscoped/query-named
    // fallback a matched skill's `schema_metadata` uses above), so the
    // result is precisely what the query asked about. Field/relationship
    // `description` content rides along automatically via `to_json` (it's
    // baked into the descriptor). The schema's own top-level description is
    // NOT automatic — it's rendered from a separate subtree fetch above,
    // via `schema_description_cache`, and this loop reuses that exact same
    // cache/fetch rather than adding a second, independent way to get it,
    // so a `kind: "schema"` result never drifts from what a `kind: "skill"`
    // result would show for the identical schema.
    for (schema, confidence) in &schema_candidates {
        let schema_metadata: Vec<Value> = vec![
            schema_definition(
                node_service.as_ref(),
                schema,
                &all_schemas,
                &mut schema_description_cache,
            )
            .await,
        ];

        skills.push(json!({
            "id": schema.envelope.id,
            "name": schema.envelope.content,
            "kind": "schema",
            "use_for": "",
            "confidence": confidence,
            "tools": Value::Array(vec![]),
            "schema_metadata": schema_metadata,
            "schemas_linked": false,
            "instructions": "",
        }));
    }

    // Re-sort the combined list by confidence, descending. A no-op for the
    // skill-only case (the store already returns KNN hits in that order,
    // and `sort_by` is stable) — what this adds is a single coherent
    // ranking across skill and schema candidates together, since the two
    // were retrieved by separate calls above.
    fn confidence_of(v: &Value) -> f64 {
        v.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0)
    }
    skills.sort_by(|a, b| {
        confidence_of(b)
            .partial_cmp(&confidence_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let total_results = skills.len();
    let all_scores = format_all_scores(&skill_results);
    let schema_scores = schema_candidates
        .iter()
        .map(|(schema, score)| format!("{}={:.3}", schema.envelope.id, score))
        .collect::<Vec<_>>()
        .join(", ");
    let top_score = skills.first().map(confidence_of).unwrap_or(0.0);

    tracing::info!(
        query = %input.query,
        results_found = total_results,
        top_score = top_score,
        all_scores = %all_scores,
        schema_candidates_found = schema_candidates.len(),
        schema_scores = %schema_scores,
        "find_skills executed"
    );

    Ok(FindSkillsOutput {
        skills,
        query: input.query,
        total_results,
    })
}

/// A skill as [`find_skills`] and [`skills_by_id`] return it.
fn skill_entry(
    node: &Node,
    fields: &SkillFields,
    confidence: f64,
    schema_metadata: Vec<Value>,
    schemas_linked: bool,
    instructions: String,
) -> Value {
    json!({
        "id": node.id,
        "name": node.content,
        "kind": "skill",
        "use_for": fields.use_for,
        "confidence": confidence,
        "role": fields.role,
        "tools": fields.tool_whitelist,
        "schema_metadata": schema_metadata,
        "schemas_linked": schemas_linked,
        "instructions": instructions,
    })
}

/// The skills among `ids`, each in the shape [`find_skills`] returns a
/// matched skill in, in the order of `ids`. An id that names no participating
/// skill is left out.
///
/// For a skill chosen by something other than a query, such as one a chat
/// pins (ADR-090 §5). It reads no embedding and ranks nothing, so
/// `confidence` is 0. `schema_metadata` is the schemas the skill links to
/// through `applies_to`, with their subtypes: a skill with no link carries
/// none, since the fallback [`find_skills`] uses is a guess from the query.
pub async fn skills_by_id(
    node_service: &NodeService,
    ids: &[String],
) -> Result<Vec<Value>, OpsError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let registry = participating_skills(node_service).await?;
    let nodes: Vec<&Node> = ids
        .iter()
        .filter_map(|id| registry.iter().find(|node| &node.id == id))
        .collect();
    if nodes.is_empty() {
        return Ok(Vec::new());
    }

    let all_schemas = node_service
        .get_all_schemas()
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to read schemas: {}", e)))?;
    let skill_ids: Vec<String> = nodes.iter().map(|node| node.id.clone()).collect();
    let applies_to = node_service
        .store()
        .get_edge_targets_by_source(&skill_ids, SKILL_APPLIES_TO)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to read applies_to links: {}", e)))?;

    let mut description_cache = HashMap::new();
    let mut skills = Vec::with_capacity(nodes.len());
    for node in nodes {
        let Some(fields) = skill_fields(node) else {
            continue;
        };
        let linked = applies_to
            .get(&node.id)
            .map(|targets| linked_schemas(targets, &all_schemas))
            .unwrap_or_default();
        let schemas_linked = !linked.is_empty();
        let mut schema_metadata = Vec::with_capacity(linked.len());
        for schema in linked {
            schema_metadata.push(
                schema_definition(node_service, schema, &all_schemas, &mut description_cache).await,
            );
        }
        let instructions = render_skill_instructions(node_service, &node.id).await;
        skills.push(skill_entry(
            node,
            &fields,
            0.0,
            schema_metadata,
            schemas_linked,
            instructions,
        ));
    }
    Ok(skills)
}

/// One skill in a guidance fetch.
#[derive(Debug, Clone, PartialEq)]
pub struct GuidanceSkill {
    pub id: String,
    pub name: String,
    pub use_for: String,
    /// RFC 3339.
    pub modified_at: String,
    /// The retrieval score the skill search gave it. `None` in a listing,
    /// which ranks nothing, and in a fetch by name.
    pub confidence: Option<f64>,
    /// The skill's stored procedure, as markdown. Empty in a listing.
    pub instructions: String,
    /// The CLI command of each registry tool the skill lists in its
    /// `tool_whitelist` or names in its procedure. Empty in a listing.
    pub tool_commands: Vec<GuidanceToolCommand>,
}

/// A registry tool a fetched skill names, with the `nodespace` command that
/// does what the tool does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuidanceToolCommand {
    /// The tool node the command was read from. A caller that admits a
    /// built-in tool only from its own seeded node decides by this id.
    pub node_id: String,
    /// The tool's name, as a skill names it.
    pub tool: String,
    /// The command and subcommand, with no arguments.
    pub command: String,
}

/// One schema in a guidance fetch: the shape of a type the request touches.
#[derive(Debug, Clone, PartialEq)]
pub struct GuidanceSchema {
    pub id: String,
    pub name: String,
    /// The type's definition in the `schema_metadata` form: `type_id`,
    /// `fields` (each with its type, description and enum values),
    /// `relationships`, and the schema's own `description` where it has one.
    pub definition: Value,
}

/// What a guidance fetch returns: the skills matching a request, and the
/// schemas relevant to it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillGuidance {
    pub skills: Vec<GuidanceSkill>,
    pub schemas: Vec<GuidanceSchema>,
}

/// Every skill in the graph, without procedures, and the version of that
/// list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillListing {
    pub skills: Vec<GuidanceSkill>,
    /// Changes when a skill is added, removed or archived, and when a skill's
    /// name, `use_for`, tool list or procedure changes. Two listings with
    /// no such change between them carry the same version.
    pub version: String,
}

/// A skill node's fields, or `None` for a node that does not decode as a
/// skill. `SkillNodeBehavior::validate` rejects that shape on write, so only
/// a raw store write can produce it.
fn skill_fields(node: &Node) -> Option<SkillFields> {
    SkillFields::from_node(node)
        .map_err(|e| {
            tracing::warn!(skill_id = %node.id, error = %e, "skill guidance: skipping malformed skill node");
        })
        .ok()
}

fn guidance_skill(node: &Node, fields: &SkillFields, confidence: Option<f64>) -> GuidanceSkill {
    GuidanceSkill {
        id: node.id.clone(),
        name: node.content.clone(),
        use_for: fields.use_for.clone(),
        modified_at: node.modified_at.to_rfc3339(),
        confidence,
        instructions: String::new(),
        tool_commands: Vec::new(),
    }
}

/// A skill as a fetch returns it: its procedure, and the command of every
/// registry tool it lists or names.
fn fetched_skill(
    node: &Node,
    fields: &SkillFields,
    confidence: Option<f64>,
    instructions: String,
    registry: &[GuidanceToolCommand],
) -> GuidanceSkill {
    let tool_commands = skill_tool_commands(registry, &fields.tool_whitelist, &instructions);
    GuidanceSkill {
        instructions,
        tool_commands,
        ..guidance_skill(node, fields, confidence)
    }
}

/// Whether `text` names `identifier` as a whole word: not as part of a
/// longer identifier, so `update_node` is not named by
/// `update_nodes_from_markdown`. An underscore is part of a word here, which
/// is why this is not [`mentions_phrase`]: that splits on it, and would find
/// `update_node` in the prose "update node".
pub fn names_identifier(text: &str, identifier: &str) -> bool {
    if identifier.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(identifier).any(|(start, matched)| {
        let before = text[..start].chars().next_back();
        let after = text[start + matched.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// The tools of `registry` that a skill lists in `tool_whitelist` or names in
/// `body`, in registry order.
fn skill_tool_commands(
    registry: &[GuidanceToolCommand],
    tool_whitelist: &[String],
    body: &str,
) -> Vec<GuidanceToolCommand> {
    registry
        .iter()
        .filter(|entry| {
            tool_whitelist.iter().any(|listed| listed == &entry.tool)
                || names_identifier(body, &entry.tool)
        })
        .cloned()
        .collect()
}

/// Every native tool in the registry that records a CLI command, by name.
///
/// A tool takes part like any node, so an archived one is not read. A native
/// tool is called by its `handler` key, which is the name a skill uses for
/// it. A tool with no command has no entry. A failed read returns nothing:
/// the fetch it serves still succeeds, without commands.
async fn registry_tool_commands(node_service: &NodeService) -> Vec<GuidanceToolCommand> {
    let nodes = match node_service
        .query_nodes_by_type(CoreNodeType::Tool.as_str(), false)
        .await
    {
        Ok(nodes) => nodes,
        Err(e) => {
            tracing::warn!(error = %e, "skill guidance: failed to read the tool registry");
            return Vec::new();
        }
    };

    // Each tool subtype's chain, resolved once however many nodes share it.
    let mut origins: HashMap<String, Option<ToolOrigin>> = HashMap::new();
    let mut commands = Vec::new();
    for node in &nodes {
        let origin = match origins.get(&node.node_type) {
            Some(origin) => *origin,
            None => {
                // An unresolved chain is no tool's chain.
                let chain = node_service
                    .resolve_type_chain(&node.node_type)
                    .await
                    .unwrap_or_default();
                let origin = ToolOrigin::of(&chain);
                origins.insert(node.node_type.clone(), origin);
                origin
            }
        };
        if origin != Some(ToolOrigin::Native) {
            continue;
        }
        let field = |name: &str| {
            node.properties
                .get(CoreNodeType::ToolNative.as_str())
                .and_then(|bucket| bucket.get(name))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };
        if let (Some(tool), Some(command)) = (field("handler"), field("cli_command")) {
            commands.push(GuidanceToolCommand {
                node_id: node.id.clone(),
                tool: tool.to_string(),
                command: command.to_string(),
            });
        }
    }
    commands.sort_by(|a, b| a.tool.cmp(&b.tool).then_with(|| a.node_id.cmp(&b.node_id)));
    commands
}

/// The version of a skill list: a digest of every skill's id, name,
/// `use_for`, tool list and procedure. Derived from what is stored, so
/// nothing is written when a skill changes, and a change to any other node
/// leaves it as it was.
fn skill_list_version(skills: &[(&Node, SkillFields, String)]) -> String {
    let mut ordered: Vec<&(&Node, SkillFields, String)> = skills.iter().collect();
    ordered.sort_by(|a, b| a.0.id.cmp(&b.0.id));

    let mut hasher = Sha256::new();
    // Each part is length-prefixed, so no two different lists share a byte
    // stream.
    let mut part = |text: &str| {
        hasher.update((text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    };
    for (node, fields, body) in ordered {
        part(&node.id);
        part(&node.content);
        part(&fields.use_for);
        part(&fields.tool_whitelist.len().to_string());
        for tool in &fields.tool_whitelist {
            part(tool);
        }
        part(body);
    }
    let digest = format!("{:x}", hasher.finalize());
    digest[..16].to_string()
}

/// Every skill in the graph, by name, with its `use_for` and no procedure,
/// and the list's version: what an agent browses to learn which skills exist,
/// and what a client compares to learn whether that list changed.
///
/// Reads no embedding, so it answers while the embedding model is loading.
pub async fn list_skill_guidance(node_service: &NodeService) -> Result<SkillListing, OpsError> {
    let nodes = participating_skills(node_service).await?;

    let mut read: Vec<(&Node, SkillFields, String)> = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let Some(fields) = skill_fields(node) else {
            continue;
        };
        let body = read_skill_instructions(node_service, &node.id).await?;
        read.push((node, fields, body));
    }

    let version = skill_list_version(&read);

    let mut skills: Vec<GuidanceSkill> = read
        .iter()
        .map(|(node, fields, _)| guidance_skill(node, fields, None))
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(SkillListing { skills, version })
}

async fn participating_skills(node_service: &NodeService) -> Result<Vec<Node>, OpsError> {
    node_service
        .query_nodes_by_type(CoreNodeType::Skill.as_str(), false)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to list skills: {}", e)))
}

/// One skill, by its exact name or its id, with what a match returns for it:
/// its procedure, the schemas it is linked to through `applies_to` (and their
/// subtypes), and the command of every registry tool it lists or names.
///
/// Reads no embedding. An id is tried before a name. A name no skill has, or
/// one that several share, is an error that says so.
pub async fn get_skill_guidance(
    node_service: &NodeService,
    name_or_id: &str,
) -> Result<SkillGuidance, OpsError> {
    let key = name_or_id.trim();
    let nodes = participating_skills(node_service).await?;
    let node = match nodes.iter().find(|node| node.id == key) {
        Some(node) => node,
        None => {
            let named: Vec<&Node> = nodes.iter().filter(|node| node.content == key).collect();
            match named.as_slice() {
                [node] => *node,
                [] => {
                    return Err(OpsError::NotFound {
                        id: format!("skill \"{key}\""),
                    })
                }
                several => {
                    let ids: Vec<&str> = several.iter().map(|node| node.id.as_str()).collect();
                    return Err(OpsError::InvalidParams(format!(
                        "{} skills are named \"{key}\"; fetch one by its id: {}",
                        several.len(),
                        ids.join(", ")
                    )));
                }
            }
        }
    };
    if skill_fields(node).is_none() {
        return Err(OpsError::Internal(format!(
            "skill \"{key}\" does not decode as a skill"
        )));
    }
    fetch_skills(node_service, std::slice::from_ref(node)).await
}

/// Each of `skills` as a fetch by name returns it: its procedure, the command
/// of every registry tool it lists or names, and the schemas it is linked to
/// through `applies_to` (and their subtypes). Skills keep their order, and a
/// schema several of them link to appears once. A node that does not decode
/// as a skill is left out.
pub(crate) async fn fetch_skills(
    node_service: &NodeService,
    skills: &[Node],
) -> Result<SkillGuidance, OpsError> {
    let mut guidance = SkillGuidance::default();
    if skills.is_empty() {
        return Ok(guidance);
    }
    let registry = registry_tool_commands(node_service).await;
    let all_schemas = node_service
        .get_all_schemas()
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to read schemas: {}", e)))?;
    let skill_ids: Vec<String> = skills.iter().map(|node| node.id.clone()).collect();
    let applies_to = node_service
        .store()
        .get_edge_targets_by_source(&skill_ids, SKILL_APPLIES_TO)
        .await
        .map_err(|e| OpsError::Internal(format!("Failed to read applies_to links: {}", e)))?;

    let mut description_cache = HashMap::new();
    for node in skills {
        let Some(fields) = skill_fields(node) else {
            continue;
        };
        let instructions = render_skill_instructions(node_service, &node.id).await;
        guidance
            .skills
            .push(fetched_skill(node, &fields, None, instructions, &registry));

        let linked = applies_to
            .get(&node.id)
            .map(|targets| linked_schemas(targets, &all_schemas))
            .unwrap_or_default();
        for schema in linked {
            if guidance
                .schemas
                .iter()
                .any(|known| known.id == schema.envelope.id)
            {
                continue;
            }
            let definition =
                schema_definition(node_service, schema, &all_schemas, &mut description_cache).await;
            guidance.schemas.extend(guidance_schema(&definition));
        }
    }
    Ok(guidance)
}

/// The skills matching `query`, each with its procedure and the command of
/// every registry tool it lists or names, and the schemas relevant to the
/// same query: what an agent outside the app fetches before an operation.
///
/// Ranked by [`find_skills`], so a request gets the skills the in-app agent
/// would get for it, in the same order.
///
/// A schema is returned when the request is about its type, by one of three
/// signs:
///
/// - a returned skill is linked to it through `applies_to`;
/// - the request names it (see [`mentions_phrase`]);
/// - the schema search matched it at or above
///   [`GUIDANCE_SCHEMA_SCORE_BAR`].
///
/// A skill's unlinked fallback (the first few custom types) is left out: it
/// is a guess, and the in-app agent holds it against a tool surface an
/// outside agent does not have. The in-app agent is given every schema match
/// with its score, and its routing judges them; this is the judgment for a
/// reader that has no routing step.
pub async fn find_skill_guidance(
    embedding_service: &Arc<NodeEmbeddingService>,
    node_service: &Arc<NodeService>,
    input: FindSkillsInput,
) -> Result<SkillGuidance, OpsError> {
    let query = input.query.clone();
    let found = find_skills(embedding_service, node_service, input).await?;
    let mut guidance = SkillGuidance {
        skills: Vec::new(),
        schemas: guidance_schemas(&found.skills, &query),
    };
    let registry = registry_tool_commands(node_service).await;

    for entry in found.skills.iter().filter(|entry| !is_schema_entry(entry)) {
        let Some(id) = entry.get("id").and_then(Value::as_str) else {
            continue;
        };
        let node = match node_service.get_node(id).await {
            Ok(Some(node)) => node,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(skill_id = %id, error = %e, "skill guidance: failed to read a matched skill");
                continue;
            }
        };
        let Some(fields) = skill_fields(&node) else {
            continue;
        };
        let confidence = entry.get("confidence").and_then(Value::as_f64);
        let instructions = entry
            .get("instructions")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        guidance.skills.push(fetched_skill(
            &node,
            &fields,
            confidence,
            instructions,
            &registry,
        ));
    }

    Ok(guidance)
}

/// A schema of a guidance fetch, from its `schema_metadata` definition.
fn guidance_schema(definition: &Value) -> Option<GuidanceSchema> {
    let id = definition.get("type_id").and_then(Value::as_str)?;
    let name = definition
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(id)
        .to_string();
    Some(GuidanceSchema {
        id: id.to_string(),
        name,
        definition: definition.clone(),
    })
}

fn is_schema_entry(entry: &Value) -> bool {
    entry.get("kind").and_then(Value::as_str) == Some("schema")
}

/// The schemas a guidance fetch returns, chosen from [`find_skills`]' entries
/// for `query` by the three signs [`find_skill_guidance`] documents. Each
/// type appears once, in the order first met.
fn guidance_schemas(found: &[Value], query: &str) -> Vec<GuidanceSchema> {
    let query_lower = query.to_lowercase();
    let named = |definition: &Value| {
        ["type_id", "name"].iter().any(|key| {
            definition
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|text| mentions_phrase(&query_lower, &text.to_lowercase()))
        })
    };

    let mut schemas: Vec<GuidanceSchema> = Vec::new();
    let mut add = |definition: &Value| {
        if let Some(schema) = guidance_schema(definition) {
            if !schemas.iter().any(|s| s.id == schema.id) {
                schemas.push(schema);
            }
        }
    };

    for entry in found {
        let definitions = entry
            .get("schema_metadata")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if is_schema_entry(entry) {
            let clears_the_bar = entry
                .get("confidence")
                .and_then(Value::as_f64)
                .is_some_and(|score| score >= GUIDANCE_SCHEMA_SCORE_BAR);
            for definition in definitions {
                if clears_the_bar || named(definition) {
                    add(definition);
                }
            }
        } else if entry.get("schemas_linked").and_then(Value::as_bool) == Some(true) {
            for definition in definitions {
                add(definition);
            }
        }
    }
    schemas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Node, SchemaNode};
    use serde_json::json;
    use std::collections::HashMap;

    fn ranked_skill(name: &str, role: SkillRole, score: f64) -> (Node, f64) {
        let skill = SkillFields::new("use", &[], 1).with_role(role);
        (skill.into_node(name), score)
    }

    fn lane_names(ranked: Vec<(Node, f64)>, limit: usize) -> Vec<String> {
        in_lanes(ranked, limit)
            .into_iter()
            .map(|(node, _)| node.content)
            .collect()
    }

    #[test]
    fn procedures_fill_one_place_after_the_tool_skills() {
        let ranked = vec![
            ranked_skill("Completing a Task", SkillRole::Procedure, 0.95),
            ranked_skill("Node Deletion", SkillRole::Tool, 0.9),
            ranked_skill("Writing a Plan", SkillRole::Procedure, 0.85),
            ranked_skill("Graph Editing", SkillRole::Tool, 0.8),
            ranked_skill("Organization", SkillRole::Tool, 0.7),
        ];
        assert_eq!(
            lane_names(ranked, 2),
            ["Node Deletion", "Graph Editing", "Completing a Task"]
        );
    }

    #[test]
    fn the_tool_lane_is_the_same_with_or_without_procedures() {
        let tools = |with_procedures: bool| {
            let mut ranked = vec![
                ranked_skill("Node Deletion", SkillRole::Tool, 0.9),
                ranked_skill("Graph Editing", SkillRole::Tool, 0.8),
                ranked_skill("Organization", SkillRole::Tool, 0.7),
            ];
            if with_procedures {
                ranked.insert(
                    0,
                    ranked_skill("Writing a Spec", SkillRole::Procedure, 0.99),
                );
            }
            lane_names(ranked, 3)
                .into_iter()
                .filter(|name| name != "Writing a Spec")
                .collect::<Vec<_>>()
        };
        assert_eq!(tools(true), tools(false));
    }

    #[test]
    fn no_procedure_adds_nothing() {
        let ranked = vec![ranked_skill("Graph Editing", SkillRole::Tool, 0.8)];
        assert_eq!(lane_names(ranked, 3), ["Graph Editing"]);
    }

    fn make_schema(id: &str, content: &str, is_core: bool) -> SchemaNode {
        crate::models::schema_node::from_storage(
            Node::new_with_id(
                id.to_string(),
                "schema".to_string(),
                content.to_string(),
                json!({ "isCore": is_core, "fields": [] }),
            ),
            Vec::new(),
        )
        .expect("schema node")
    }

    fn extending(id: &str, parent: &str, is_core: bool) -> SchemaNode {
        let mut schema = make_schema(id, id, is_core);
        schema.extends = Some(parent.to_string());
        schema
    }

    fn linked_ids(targets: &[&str], all_schemas: &[SchemaNode]) -> Vec<String> {
        let targets: Vec<String> = targets.iter().map(|t| t.to_string()).collect();
        linked_schemas(&targets, all_schemas)
            .into_iter()
            .map(|s| s.envelope.id.clone())
            .collect()
    }

    #[test]
    fn linked_schemas_are_the_targets_and_every_schema_extending_one() {
        let all = vec![
            make_schema("task", "Task", true),
            make_schema("invoice", "Invoice", false),
            extending("issue", "task", false),
            extending("bug", "issue", false),
            make_schema("venue", "Venue", false),
        ];

        // A linked custom type, alone.
        assert_eq!(linked_ids(&["invoice"], &all), ["invoice"]);
        // A linked core type is honoured, and brings its subtypes at any depth.
        assert_eq!(linked_ids(&["task"], &all), ["task", "issue", "bug"]);
        // A linked subtype brings its own subtypes, not its base.
        assert_eq!(linked_ids(&["issue"], &all), ["issue", "bug"]);
        // Several links are a union, in corpus order.
        assert_eq!(
            linked_ids(&["venue", "issue"], &all),
            ["issue", "bug", "venue"]
        );
    }

    #[test]
    fn a_link_to_something_that_is_not_a_schema_puts_nothing_in_scope() {
        let all = vec![make_schema("invoice", "Invoice", false)];
        assert!(linked_ids(&["9d0c1b7e-not-a-schema"], &all).is_empty());
        assert!(linked_ids(&[], &all).is_empty());
    }

    /// A corrupt `extends` cycle ends instead of spinning.
    #[test]
    fn linked_schemas_terminate_on_an_extends_cycle() {
        let all = vec![extending("a", "b", false), extending("b", "a", false)];
        assert!(linked_ids(&["task"], &all).is_empty());
        assert_eq!(linked_ids(&["a"], &all), ["a", "b"]);
    }

    #[test]
    fn names_identifier_matches_a_tool_name_as_a_whole_word() {
        assert!(names_identifier("call update_node once", "update_node"));
        assert!(names_identifier("update_node", "update_node"));
        assert!(names_identifier("Use `update_node`.", "update_node"));
        assert!(names_identifier("(update_node, get_node)", "get_node"));
        // Inside a longer identifier, on either side.
        assert!(!names_identifier(
            "call update_nodes_from_markdown",
            "update_node"
        ));
        assert!(!names_identifier("call bulk_update_node", "update_node"));
        assert!(!names_identifier("call update_node2", "update_node"));
        // The words of a name are not the name.
        assert!(!names_identifier("then update node X", "update_node"));
        assert!(!names_identifier("anything", ""));
        // A later whole-word use counts after an earlier partial one.
        assert!(names_identifier(
            "update_nodes_from_markdown, then update_node",
            "update_node"
        ));
    }

    fn registry_entry(tool: &str, command: &str) -> GuidanceToolCommand {
        GuidanceToolCommand {
            node_id: format!("node-{tool}"),
            tool: tool.to_string(),
            command: command.to_string(),
        }
    }

    #[test]
    fn skill_tool_commands_returns_listed_and_named_tools_in_registry_order() {
        let registry = vec![
            registry_entry("create_node", "nodespace node create"),
            registry_entry("get_node", "nodespace node get"),
            registry_entry("update_node", "nodespace node update"),
        ];
        let listed = vec!["update_node".to_string(), "not_in_registry".to_string()];

        let commands = skill_tool_commands(&registry, &listed, "First call get_node.");

        let tools: Vec<&str> = commands.iter().map(|c| c.tool.as_str()).collect();
        assert_eq!(tools, ["get_node", "update_node"]);
        assert!(skill_tool_commands(&registry, &[], "No tool here.").is_empty());
    }

    fn versioned_skill(id: &str, name: &str, use_for: &str, tools: &[&str]) -> (Node, SkillFields) {
        let fields = SkillFields::new(use_for, tools, 3);
        let node = Node::new_with_id(
            id.to_string(),
            "skill".to_string(),
            name.to_string(),
            fields.properties(),
        );
        (node, fields)
    }

    #[test]
    fn skill_list_version_follows_what_a_skill_holds_and_not_the_order_read() {
        let (a, a_fields) = versioned_skill("a", "Alpha", "First", &["get_node"]);
        let (b, b_fields) = versioned_skill("b", "Beta", "Second", &[]);
        let version = |skills: &[(&Node, SkillFields, String)]| skill_list_version(skills);

        let base = version(&[
            (&a, a_fields.clone(), "Body A".to_string()),
            (&b, b_fields.clone(), "Body B".to_string()),
        ]);
        assert_eq!(base.len(), 16);
        assert_eq!(
            base,
            version(&[
                (&b, b_fields.clone(), "Body B".to_string()),
                (&a, a_fields.clone(), "Body A".to_string()),
            ]),
            "the order skills are read in is not a change"
        );

        let (renamed, _) = versioned_skill("a", "Alpha Two", "First", &["get_node"]);
        let (_, redescribed) = versioned_skill("a", "Alpha", "First, reworded", &["get_node"]);
        let (_, relisted) = versioned_skill("a", "Alpha", "First", &["get_node", "update_node"]);
        let b_entry = || (&b, b_fields.clone(), "Body B".to_string());
        let changes = [
            version(&[
                (&renamed, a_fields.clone(), "Body A".to_string()),
                b_entry(),
            ]),
            version(&[(&a, redescribed, "Body A".to_string()), b_entry()]),
            version(&[(&a, relisted, "Body A".to_string()), b_entry()]),
            version(&[(&a, a_fields.clone(), "Body A.".to_string()), b_entry()]),
            version(&[(&a, a_fields.clone(), "Body A".to_string())]),
            version(&[]),
        ];
        for (i, changed) in changes.iter().enumerate() {
            assert_ne!(changed, &base, "change {i} left the version as it was");
            assert!(
                !changes[..i].contains(changed),
                "change {i} collides with an earlier one"
            );
        }

        // Text moving between two fields is a change: each part is delimited.
        let (_, split_one) = versioned_skill("a", "Alpha", "ab", &[]);
        let (_, split_two) = versioned_skill("a", "Alpha", "a", &[]);
        assert_ne!(
            version(&[(&a, split_one, "c".to_string())]),
            version(&[(&a, split_two, "bc".to_string())])
        );
    }

    #[test]
    fn mentions_phrase_matches_a_single_word_type_id_at_word_boundaries() {
        assert!(mentions_phrase("create a ticket for the bug", "ticket"));
        assert!(!mentions_phrase("update the address field", "adr"));
        assert!(!mentions_phrase("no match here", "ticket"));
    }

    #[test]
    fn mentions_phrase_matches_a_multi_word_display_name_contiguously() {
        assert!(mentions_phrase(
            "open a pull request for this",
            "pull request"
        ));
        assert!(!mentions_phrase("pull the request later", "pull request"));
    }

    #[test]
    fn mentions_phrase_matches_a_snake_case_id_against_a_space_separated_query() {
        // A multi-word type id like `release_plan` has no whitespace of its
        // own, so it must be tokenized the same way the query is — on any
        // non-alphanumeric separator, not only whitespace — or it can never
        // match a query that spells the same words with spaces.
        assert!(mentions_phrase(
            "create a release plan for q3",
            "release_plan"
        ));
        assert!(!mentions_phrase(
            "create a release for the plan later",
            "release_plan"
        ));
    }

    #[test]
    fn mentions_phrase_matches_a_kebab_case_id_against_a_space_separated_query() {
        assert!(mentions_phrase(
            "open a pull request for this",
            "pull-request"
        ));
    }

    #[test]
    fn schema_named_in_query_scopes_to_the_single_named_non_core_type() {
        let schemas = vec![
            make_schema("ticket", "Ticket", false),
            make_schema("adr", "ADR", false),
        ];
        let found = schema_named_in_query("create a ticket for the login bug", &schemas);
        assert_eq!(found.map(|s| s.envelope.id.as_str()), Some("ticket"));
    }

    #[test]
    fn schema_named_in_query_matches_a_kebab_case_id_by_id_alone() {
        // Regression: the id-path must work even when the display name
        // (content) doesn't share the same words as the id, so this only
        // passes via `id` matching, not `content`.
        let schemas = vec![
            make_schema("release-plan", "Q3 Rollout", false),
            make_schema("adr", "ADR", false),
        ];
        let found = schema_named_in_query("create a release plan for q3", &schemas);
        assert_eq!(found.map(|s| s.envelope.id.as_str()), Some("release-plan"));
    }

    #[test]
    fn schema_named_in_query_matches_by_display_name_too() {
        let schemas = vec![
            make_schema("ticket", "Ticket", false),
            make_schema("adr", "Architecture Decision Record", false),
        ];
        let found = schema_named_in_query(
            "draft an architecture decision record for the new store",
            &schemas,
        );
        assert_eq!(found.map(|s| s.envelope.id.as_str()), Some("adr"));
    }

    #[test]
    fn schema_named_in_query_returns_none_when_two_types_are_named() {
        let schemas = vec![
            make_schema("ticket", "Ticket", false),
            make_schema("adr", "ADR", false),
        ];
        let found = schema_named_in_query("link this ticket to the adr", &schemas);
        assert!(found.is_none());
    }

    #[test]
    fn schema_named_in_query_returns_none_when_no_type_is_named() {
        let schemas = vec![
            make_schema("ticket", "Ticket", false),
            make_schema("adr", "ADR", false),
        ];
        let found = schema_named_in_query("what did we work on yesterday", &schemas);
        assert!(found.is_none());
    }

    #[test]
    fn schema_named_in_query_ignores_core_schemas() {
        // A query naming a core type ("task") alongside a real non-core match
        // must not let the core mention count toward ambiguity — core types
        // are never in the unscoped fallback's candidate pool to begin with.
        let schemas = vec![
            make_schema("task", "Task", true),
            make_schema("ticket", "Ticket", false),
        ];
        let found = schema_named_in_query("create a ticket, not a task", &schemas);
        assert_eq!(found.map(|s| s.envelope.id.as_str()), Some("ticket"));
    }

    // -------------------------------------------------------------------
    // Measurement: false-positive rate of the lexical narrowing.
    // -------------------------------------------------------------------
    //
    // This required measuring, not guessing, whether the risk that a schema
    // id/display-name happening to also be a common English word narrows
    // `schema_metadata` to a confidently WRONG type is practically
    // significant. The two corpora below are the measurement; the tests
    // after them pin its result.

    /// Every non-core schema id/display-name pair that actually appears
    /// anywhere in this codebase's tests, golden fixtures, or docs (grepped
    /// across `packages/agent/goldens/*.toml`, this module's own
    /// `make_schema` calls, and the `Venue`/`Invoice` examples in
    /// ADR-063 and `schema-management.md`) — i.e. names with real precedent
    /// as the kind of type a NodeSpace user actually defines, as opposed to
    /// a hypothetical worst case.
    fn precedent_schema_names() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ticket", "Ticket"),
            ("adr", "ADR"),
            ("release", "Release"),
            ("release_plan", "Q3 Rollout"),
            ("venue", "Venue"),
            ("invoice", "Invoice"),
            ("equipment", "Equipment Checkout Record"),
        ]
    }

    /// Realistic queries this narrowing pass actually has to face, none of
    /// which concern any of `precedent_schema_names()`'s types (the golden
    /// corpus's one query that IS genuinely about a precedent type — "File a
    /// ticket for dana..." — is exercised separately, as a true-positive
    /// check, not here). Two sub-corpora:
    ///
    /// - Verbatim `user = "..."` turns from `packages/agent/goldens/*.toml`
    ///   (including `ablation/`) — the project's own dev-workflow-shaped,
    ///   contamination-guarded query corpus, used elsewhere in this session
    ///   for LLM-behavior measurement and reused here for a lexical one.
    /// - A hand-built "everyday PKM assistant" corpus: NodeSpace is a
    ///   general knowledge tool, not only a dev tracker, so the query
    ///   surface isn't limited to ticket/ADR-shaped requests.
    fn realistic_unrelated_queries() -> Vec<&'static str> {
        vec![
            // From packages/agent/goldens/*.toml `user = "..."` turns.
            "What's sitting in review?",
            "Go ahead and mark it done",
            "What's in dev right now?",
            "Put the CI runner one on priya",
            "The auth one is ready for review now",
            "Anything assigned in sprint S-25 yet?",
            "So what did you find?",
            "What releases do we have open?",
            "2026.8.3 is out on staging now, baking overnight before we ship it",
            "I've pushed the branch for that one — it's ready for review now",
            "The 2400 one came back — set it to returned",
            "Log a laser cutter checked out on the 12th, replacement cost 2400",
            // Hand-built everyday PKM-assistant corpus.
            "please note that the meeting moved to 3pm",
            "just a quick note before I forget — grab milk on the way home",
            "note to self, call the dentist tomorrow",
            "can you remind me to review this later",
            "what's on my calendar for today",
            "let's plan the offsite for next month",
            "I need to update my resume this weekend",
            "can you change the font size in the editor",
            "what time does the meeting start",
            "did you see the update from the team",
            "I have a question about the pricing page",
            "what's the answer to life the universe and everything",
            "post this to the team channel",
            "draft me an email to the landlord",
            "can you file this under the right folder",
            "there's a broken link on the homepage",
            "set an alert for 7am tomorrow",
            "I finished reading that book last night",
            "try this recipe for dinner tonight",
            "I'm working on a new habit tracker for myself",
            "log me out of this session",
            "the order came in late again",
            "leave a comment on the doc",
            "can you summarize this article for me",
            "what's the weather like this weekend",
            "remind me to water the plants",
            "I want to journal about today",
            "add this to my reading list",
            "can we reschedule our call",
            "I have an idea for the new feature",
            "what's my goal for this quarter",
            "open the report from last week",
            "who do I contact about billing",
            "send a message to the team",
            "the entry fee was too high",
            "I need to renew my passport",
            "can you look up my old notes on this topic",
            "let's take notes during the call",
            "any updates on the shipment",
            "review my notes from yesterday",
        ]
    }

    #[test]
    fn schema_named_in_query_measured_zero_false_positives_on_precedent_schema_names() {
        // MEASURED: every schema id/display-name with real precedent in
        // this codebase, checked against every query in
        // `realistic_unrelated_queries()`, produces no match at all — never
        // an incidental false positive. 7 names x 52 queries = 364 pairs, 0
        // false positives. This is the basis for leaving the narrowing
        // as-is for domain-specific names.
        let schemas: Vec<SchemaNode> = precedent_schema_names()
            .into_iter()
            .map(|(id, content)| make_schema(id, content, false))
            .collect();

        for query in realistic_unrelated_queries() {
            if let Some(matched) = schema_named_in_query(query, &schemas) {
                panic!(
                    "unexpected lexical match: schema `{}` matched query {:?}, \
                     which is not about that type — precedent-name corpus was \
                     measured to have zero false positives; this query is a \
                     new one",
                    matched.envelope.id, query
                );
            }
        }
    }

    #[test]
    fn schema_named_in_query_true_positive_still_fires_for_a_precedent_name() {
        // Companion to the false-positive check above: narrowing a
        // precedent domain name still fires correctly on a query genuinely
        // about it (the one golden query excluded from
        // `realistic_unrelated_queries()` for exactly this reason).
        let schemas: Vec<SchemaNode> = precedent_schema_names()
            .into_iter()
            .map(|(id, content)| make_schema(id, content, false))
            .collect();
        let found = schema_named_in_query(
            "File a ticket for dana to fix the flaky retry test in sprint S-25 — \
             it's ready for dev, and it's blocked by the token refresh work",
            &schemas,
        );
        assert_eq!(found.map(|s| s.envelope.id.as_str()), Some("ticket"));
    }

    /// 29 plausible-but-unprecedented schema names — common English words
    /// that are nonetheless names a NodeSpace user could reasonably give a
    /// custom type, since NodeSpace is a personal knowledge tool, not only
    /// a dev tracker. Companion corpus to `precedent_schema_names()`, used
    /// to measure the risk class of a schema name colliding with a common
    /// English word, rather than the class that's actually seen precedent.
    fn risky_schema_names() -> Vec<(&'static str, &'static str)> {
        vec![
            ("note", "Note"),
            ("meeting", "Meeting"),
            ("idea", "Idea"),
            ("goal", "Goal"),
            ("plan", "Plan"),
            ("record", "Record"),
            ("item", "Item"),
            ("event", "Event"),
            ("contact", "Contact"),
            ("order", "Order"),
            ("review", "Review"),
            ("report", "Report"),
            ("request", "Request"),
            ("log", "Log"),
            ("entry", "Entry"),
            ("message", "Message"),
            ("post", "Post"),
            ("draft", "Draft"),
            ("file", "File"),
            ("link", "Link"),
            ("alert", "Alert"),
            ("reminder", "Reminder"),
            ("question", "Question"),
            ("answer", "Answer"),
            ("update", "Update"),
            ("change", "Change"),
            ("book", "Book"),
            ("recipe", "Recipe"),
            ("habit", "Habit"),
        ]
    }

    #[test]
    fn schema_named_in_query_measured_false_positive_rate_on_risky_common_word_names() {
        // MEASURED: the risk of a false-positive lexical match is real, not
        // hypothetical, for schema names that are common English words.
        // Each of the 29 `risky_schema_names()` is checked in
        // isolation (one schema at a time — matching
        // `precedent_schema_names()`'s "no other type is around to create
        // ambiguity" setup, so this measures the same thing the doc
        // comment's "29 names x 52 queries = 1508 pairs" describes, not a
        // differently-shaped measurement) against every query in
        // `realistic_unrelated_queries()`, none of which concern that
        // schema. Total false positives are counted and pinned here, not
        // just reported in prose, so a future change to `mentions_phrase`
        // that shifts this rate has to update this assertion deliberately.
        let queries = realistic_unrelated_queries();
        let mut false_positives = 0usize;
        for (id, content) in risky_schema_names() {
            let schemas = vec![make_schema(id, content, false)];
            for query in &queries {
                if schema_named_in_query(query, &schemas).is_some() {
                    false_positives += 1;
                }
            }
        }
        let total_pairs = risky_schema_names().len() * queries.len();
        assert_eq!(
            (false_positives, total_pairs),
            (33, 1508),
            "false-positive count or corpus size drifted from the measured \
             baseline (33/1508, 2.2%) — if `mentions_phrase` or \
             either corpus changed intentionally, re-measure and update \
             both this assertion and the doc comment on \
             `schema_named_in_query`"
        );
    }

    #[test]
    fn schema_named_in_query_confirmed_false_positive_class_common_word_schema_names() {
        // Two concrete, easy-to-read instances of the false-positive class
        // the aggregate test above measures — a common-word schema name
        // colliding with an unrelated query, and a mixed schema list
        // (alongside an unrelated precedent name) rather than an isolated
        // one, so this doubles as a check that the risk isn't an artifact
        // of testing risky names in isolation.
        let schemas = vec![
            make_schema("ticket", "Ticket", false),
            make_schema("note", "Note", false),
        ];
        let found = schema_named_in_query("please note that the meeting moved to 3pm", &schemas);
        assert_eq!(
            found.map(|s| s.envelope.id.as_str()),
            Some("note"),
            "a schema literally named `Note` lexically matches an unrelated \
             use of the word \"note\" — confirmed, accepted risk, not a bug \
             in this test"
        );

        let schemas = vec![make_schema("meeting", "Meeting", false)];
        let found = schema_named_in_query("what time does the meeting start", &schemas);
        assert_eq!(found.map(|s| s.envelope.id.as_str()), Some("meeting"));

        // Every one of the 29 risky words is ALSO the exact word a genuine
        // true-positive query for that same type would use — there is no
        // lexical signal available to `mentions_phrase` that tells the two
        // apart, which is why no length-floor or stopword-exclusion guard
        // is added here (see the doc comment on `schema_named_in_query`).
        let schemas = vec![make_schema("log", "Log", false)];
        let false_positive = schema_named_in_query("log me out of this session", &schemas);
        let true_positive = schema_named_in_query("add a log for today's workout", &schemas);
        assert_eq!(false_positive.map(|s| s.envelope.id.as_str()), Some("log"));
        assert_eq!(true_positive.map(|s| s.envelope.id.as_str()), Some("log"));
    }

    fn schema_entry(type_id: &str, name: &str, confidence: f64) -> Value {
        json!({
            "id": type_id, "name": name, "kind": "schema", "confidence": confidence,
            "schemas_linked": false,
            "schema_metadata": [{ "type_id": type_id, "name": name, "fields": [] }],
        })
    }

    fn skill_entry(name: &str, linked: bool, types: &[&str]) -> Value {
        let metadata: Vec<Value> = types
            .iter()
            .map(|t| json!({ "type_id": t, "name": t, "fields": [] }))
            .collect();
        json!({
            "id": name, "name": name, "kind": "skill", "confidence": 0.9,
            "schemas_linked": linked, "schema_metadata": metadata,
        })
    }

    fn guidance_schema_ids(found: &[Value], query: &str) -> Vec<String> {
        guidance_schemas(found, query)
            .into_iter()
            .map(|s| s.id)
            .collect()
    }

    #[test]
    fn a_schema_match_is_returned_at_the_bar_and_not_below_it() {
        let found = vec![
            schema_entry("invoice", "Invoice", GUIDANCE_SCHEMA_SCORE_BAR),
            schema_entry("venue", "Venue", GUIDANCE_SCHEMA_SCORE_BAR - 0.01),
        ];
        assert_eq!(guidance_schema_ids(&found, "bill the client"), ["invoice"]);
    }

    #[test]
    fn a_schema_below_the_bar_is_returned_when_the_request_names_it() {
        let found = vec![schema_entry("release_plan", "Release Plan", 0.4)];
        assert_eq!(
            guidance_schema_ids(&found, "delete the release plan for Q3"),
            ["release_plan"]
        );
        assert!(guidance_schema_ids(&found, "delete a node").is_empty());
    }

    #[test]
    fn a_skills_linked_schemas_are_returned_and_its_fallback_is_not() {
        let found = vec![
            skill_entry("Sprints and Cycles", true, &["cycle", "issue"]),
            skill_entry("Node Creation", false, &["venue"]),
        ];
        assert_eq!(
            guidance_schema_ids(&found, "plan the week"),
            ["cycle", "issue"]
        );
    }

    #[test]
    fn a_type_reached_twice_is_returned_once() {
        let found = vec![
            skill_entry("Creating an Issue", true, &["issue"]),
            skill_entry("Sprints and Cycles", true, &["cycle", "issue"]),
            schema_entry("issue", "Issue", 0.95),
        ];
        assert_eq!(
            guidance_schema_ids(&found, "file a bug"),
            ["issue", "cycle"]
        );
    }

    /// An entry that lacks the keys read here contributes nothing, and does
    /// not fail the fetch. These fixtures are written by hand, so this does
    /// not notice `find_skills` renaming a key: that is caught by the live
    /// fetch tests, which read its real output.
    #[test]
    fn an_entry_without_the_expected_keys_contributes_nothing() {
        let found = vec![json!({ "kind": "schema", "score": 0.99, "metadata": [] })];
        assert!(guidance_schema_ids(&found, "anything").is_empty());
    }

    fn make_node(id: &str, content: &str) -> Node {
        Node {
            id: id.to_string(),
            node_type: "text".to_string(),
            content: content.to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: json!({}),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        }
    }

    #[test]
    fn format_all_scores_includes_a_low_scoring_result_a_threshold_would_filter() {
        // The entire point of `all_scores` is visibility into candidates a
        // score gate would otherwise hide — a low score here (well below any
        // of `routing::READ_SKILL_SCORE_BAR` /
        // `MUTATING_SKILL_SCORE_BAR` / `DESTRUCTIVE_SKILL_SCORE_BAR` in the
        // sibling `nodespace-agent` crate) must still appear, unlike a
        // gate-filtered field would render it.
        let results = vec![
            (make_node("s1", "Research & Search"), 0.9),
            (make_node("s2", "Below Any Bar"), 0.01),
        ];
        assert_eq!(
            format_all_scores(&results),
            "Research & Search=0.900, Below Any Bar=0.010"
        );
    }

    #[test]
    fn format_all_scores_is_empty_when_retrieval_returned_nothing() {
        assert_eq!(format_all_scores(&[]), "");
    }

    #[test]
    fn not_for_penalty_is_inert_when_the_query_fits_use_for_better() {
        // The property the margin form exists for: a query closer to what the
        // skill does than to what it excludes keeps its score exactly.
        assert_eq!(not_for_penalized_score(0.82, 0.70), 0.82);
        assert_eq!(not_for_penalized_score(0.82, 0.82), 0.82);
    }

    #[test]
    fn not_for_penalty_lowers_by_the_weighted_margin() {
        // At λ = 1.0: 0.855 − (0.90 − 0.855) = 0.81.
        let adjusted = not_for_penalized_score(0.855, 0.90);
        assert!((adjusted - 0.81).abs() < 1e-12, "{adjusted} != 0.81");
        assert!(adjusted < 0.855);
    }

    #[test]
    fn render_subtree_markdown_empty_skill() {
        let node_map: HashMap<String, Node> = HashMap::new();
        let adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        let rendered = render_subtree_markdown("skill-root", &node_map, &adjacency_list);
        assert!(rendered.is_empty());
    }

    #[test]
    fn render_subtree_markdown_flat_children() {
        let mut node_map = HashMap::new();
        node_map.insert("c1".to_string(), make_node("c1", "Step one"));
        node_map.insert("c2".to_string(), make_node("c2", "Step two"));
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        adjacency_list.insert(
            "skill-root".to_string(),
            vec!["c1".to_string(), "c2".to_string()],
        );
        let rendered = render_subtree_markdown("skill-root", &node_map, &adjacency_list);
        assert_eq!(rendered, "Step one\n\nStep two");
    }

    #[test]
    fn render_subtree_markdown_restores_tight_list_under_its_paragraph() {
        let mut node_map = HashMap::new();
        node_map.insert("p".to_string(), make_node("p", "PARAMETERS:"));
        node_map.insert("b1".to_string(), make_node("b1", "Use 'collection'"));
        node_map.insert("b2".to_string(), make_node("b2", "Use 'node_types'"));
        node_map.insert("next".to_string(), make_node("next", "Next paragraph"));
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        adjacency_list.insert(
            "skill-root".to_string(),
            vec!["p".to_string(), "next".to_string()],
        );
        adjacency_list.insert("p".to_string(), vec!["b1".to_string(), "b2".to_string()]);
        let rendered = render_subtree_markdown("skill-root", &node_map, &adjacency_list);
        assert_eq!(
            rendered,
            "PARAMETERS:\n\n- Use 'collection'\n- Use 'node_types'\n\nNext paragraph"
        );
    }

    #[test]
    fn render_subtree_markdown_indents_nested_list_items() {
        let mut node_map = HashMap::new();
        node_map.insert("p".to_string(), make_node("p", "Intro"));
        node_map.insert("b1".to_string(), make_node("b1", "Outer"));
        node_map.insert("b1a".to_string(), make_node("b1a", "Inner"));
        node_map.insert("b2".to_string(), make_node("b2", "Outer again"));
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        adjacency_list.insert("skill-root".to_string(), vec!["p".to_string()]);
        adjacency_list.insert("p".to_string(), vec!["b1".to_string(), "b2".to_string()]);
        adjacency_list.insert("b1".to_string(), vec!["b1a".to_string()]);
        let rendered = render_subtree_markdown("skill-root", &node_map, &adjacency_list);
        assert_eq!(rendered, "Intro\n\n- Outer\n  - Inner\n- Outer again");
    }

    #[test]
    fn render_subtree_markdown_marks_only_text_under_text() {
        // A header's direct text children are paragraphs, and a code block
        // attached to a paragraph carries its own fences — neither is a list
        // item.
        let mut node_map = HashMap::new();
        let mut header = make_node("h", "# Guidance");
        header.node_type = "header".to_string();
        let mut code = make_node("code", "```\nx\n```");
        code.node_type = "code-block".to_string();
        node_map.insert("h".to_string(), header);
        node_map.insert("p".to_string(), make_node("p", "Example:"));
        node_map.insert("code".to_string(), code);
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        adjacency_list.insert("skill-root".to_string(), vec!["h".to_string()]);
        adjacency_list.insert("h".to_string(), vec!["p".to_string()]);
        adjacency_list.insert("p".to_string(), vec!["code".to_string()]);
        let rendered = render_subtree_markdown("skill-root", &node_map, &adjacency_list);
        assert_eq!(rendered, "# Guidance\n\nExample:\n\n```\nx\n```");
    }

    #[test]
    fn render_subtree_markdown_skips_empty_content() {
        let mut node_map = HashMap::new();
        node_map.insert("c1".to_string(), make_node("c1", ""));
        node_map.insert("c2".to_string(), make_node("c2", "Has content"));
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        adjacency_list.insert(
            "skill-root".to_string(),
            vec!["c1".to_string(), "c2".to_string()],
        );
        let rendered = render_subtree_markdown("skill-root", &node_map, &adjacency_list);
        assert_eq!(rendered, "Has content");
    }

    #[test]
    fn skill_filter_matches_only_skill_nodes() {
        // Validates the post-filter predicate used in find_skills. The hybrid
        // BM25+KNN routing (semantic_search_nodes) requires a live DB and is
        // not unit-testable here; integration coverage lives in embedding_service
        // tests. This test only validates that the SearchNodeFilters predicate
        // correctly restricts results to skill-typed nodes.
        use crate::services::SearchNodeFilters;
        let filter = SearchNodeFilters {
            node_types: Some(vec!["skill".to_string()]),
            property_filters: None,
        };
        let empty_props = serde_json::json!({});
        assert!(filter.matches("skill", &empty_props, &["skill".to_string()]));
        assert!(!filter.matches("text", &empty_props, &["text".to_string()]));
        assert!(!filter.matches("schema", &empty_props, &["schema".to_string()]));
        assert!(!filter.matches("ai-chat", &empty_props, &["ai-chat".to_string()]));
    }

    // -------------------------------------------------------------------
    // Schema discovery: `non_core_schema_hits_with_scores` / `append_named_schema_candidates`
    // -------------------------------------------------------------------
    //
    // Mirrors `context_ops.rs`'s own test suite for
    // `non_core_schema_hits` / `append_schemas_named_in_query`
    // (same functions in spirit, kept separate for this module's own
    // scored-candidate shape — see the doc comments on the two functions
    // under test).

    fn schema_search_result(id: &str, is_core: bool, score: f64) -> (nodespace_types::Node, f64) {
        let node = Node::new_with_id(
            id.to_string(),
            "schema".to_string(),
            id.to_string(),
            json!({ "isCore": is_core, "fields": [] }),
        );
        (node, score)
    }

    #[test]
    fn non_core_schema_hits_with_scores_excludes_core_types_and_keeps_score() {
        let results = vec![
            schema_search_result("text", true, 0.9),
            schema_search_result("sprint", false, 0.42),
            schema_search_result("task", true, 0.8),
        ];

        let corpus = vec![
            named_schema("text", "Text", true),
            named_schema("sprint", "Sprint", false),
            named_schema("task", "Task", true),
        ];

        let hits = non_core_schema_hits_with_scores(results, &corpus);
        let ids_and_scores: Vec<(&str, f64)> = hits
            .iter()
            .map(|(s, score)| (s.envelope.id.as_str(), *score))
            .collect();

        assert_eq!(ids_and_scores, vec![("sprint", 0.42)]);
    }

    fn named_schema(id: &str, display: &str, is_core: bool) -> SchemaNode {
        crate::models::schema_node::from_storage(
            Node::new_with_id(
                id.to_string(),
                "schema".to_string(),
                display.to_string(),
                json!({ "isCore": is_core, "fields": [] }),
            ),
            Vec::new(),
        )
        .expect("valid schema node")
    }

    /// The debounce-window mitigation's core property: a schema created
    /// moments ago (no semantic hit yet — the ~30s embedding debounce) is
    /// still recovered when the query names it outright, mirroring
    /// `context_ops::append_schemas_named_in_query`'s identical fix for the
    /// resident "EXISTING SCHEMAS" block.
    #[test]
    fn named_schema_is_recovered_when_semantic_retrieval_is_empty() {
        let all = vec![named_schema("feature-write-up", "feature write-up", false)];
        let hits =
            append_named_schema_candidates(vec![], &all, "Put one down for feature write-up");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.envelope.id, "feature-write-up");
        assert_eq!(hits[0].1, LEXICAL_SCHEMA_MATCH_CONFIDENCE);
    }

    #[test]
    fn named_schema_matches_by_kebab_case_id_as_well_as_display_name() {
        let all = vec![named_schema("release-plan", "Release Plan", false)];
        let hits = append_named_schema_candidates(vec![], &all, "add a release-plan for Q3");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.envelope.id, "release-plan");
    }

    #[test]
    fn a_schema_found_by_semantic_search_is_not_duplicated_by_the_backstop() {
        let all = vec![named_schema("invoice", "Invoice", false)];
        let already = vec![(named_schema("invoice", "Invoice", false), 0.55)];
        let hits = append_named_schema_candidates(already, &all, "log an invoice");
        assert_eq!(hits.len(), 1, "duplicate schema injected: {hits:?}");
        // The original semantic score survives — the backstop does not
        // clobber a hit semantic search already found with the lexical
        // sentinel confidence.
        assert_eq!(hits[0].1, 0.55);
    }

    #[test]
    fn a_named_core_schema_is_not_recovered() {
        let all = vec![named_schema("task", "Task", true)];
        let hits = append_named_schema_candidates(vec![], &all, "add a task to fix the build");
        assert!(
            hits.is_empty(),
            "core schema leaked into discovery: {hits:?}"
        );
    }

    #[test]
    fn an_unrelated_query_recovers_nothing() {
        let all = vec![named_schema("invoice", "Invoice", false)];
        let hits = append_named_schema_candidates(vec![], &all, "what's the weather like today");
        assert!(hits.is_empty());
    }

    #[test]
    fn semantic_hits_keep_their_order_ahead_of_a_named_recovery() {
        let all = vec![
            named_schema("invoice", "Invoice", false),
            named_schema("venue", "Venue", false),
        ];
        let hits = append_named_schema_candidates(
            vec![(named_schema("invoice", "Invoice", false), 0.61)],
            &all,
            "book the venue for the invoice run",
        );
        let ids: Vec<&str> = hits.iter().map(|(s, _)| s.envelope.id.as_str()).collect();
        assert_eq!(ids, vec!["invoice", "venue"]);
    }
}
