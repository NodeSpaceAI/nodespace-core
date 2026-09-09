//! Skill discovery operations.
//!
//! Shared logic for skill search used by the local agent's `search_skills`
//! tool and the MCP `find_skills` handler exposed to external agents.

use crate::services::{flatten_subtree_content, NodeEmbeddingService, NodeService};
use serde_json::{json, Value};
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

/// Maximum schemas to include in `schema_metadata` when no `node_types` scope
/// is set on the matched skill. Bounds token cost for general-purpose skills.
/// Skills that declare an explicit `node_types` list are not capped.
const MAX_UNSCOPED_SCHEMA_METADATA: usize = 5;

/// Upper bound on `limit` requested by the caller.
///
/// Skill libraries are small in practice (~8-20 seeded skills plus a handful
/// of user-defined ones). A cap of 10 is large enough to expose every skill
/// in a typical workspace yet keeps the response token-cheap for small local
/// models. Revisit if user-defined skill libraries grow past ~30 skills.
const MAX_SKILL_LIMIT: usize = 10;

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

/// Render a skill node's child subtree as flat markdown.
///
/// Fetches the full subtree in a single DB query, then walks it depth-first
/// (root children first, their children next) and joins each node's content
/// with a blank line separator. The skill root itself is excluded — callers
/// already have its `name`/`description`.
///
/// Empty/childless skills return an empty string without error.
async fn render_skill_instructions(node_service: &NodeService, skill_id: &str) -> String {
    // root_node unused — callers already have the skill's name/description
    let (_, node_map, adjacency_list) = match node_service.get_subtree_data(skill_id).await {
        Ok(data) => data,
        Err(e) => {
            tracing::warn!(
                error = %e,
                skill_id = %skill_id,
                "render_skill_instructions: failed to fetch subtree"
            );
            return String::new();
        }
    };

    // One get_subtree_data query per skill. Acceptable under MAX_SKILL_LIMIT = 10;
    // a batch API would eliminate serial round trips if the limit grows.
    flatten_subtree_content(skill_id, &node_map, &adjacency_list).join("\n\n")
}

/// A skill node's discovery-relevant properties, decoded from whichever shape
/// `node.properties` is actually in.
#[derive(Debug, PartialEq)]
struct SkillProperties {
    description: String,
    tool_whitelist: Value,
    scoped_type_ids: Vec<String>,
}

impl SkillProperties {
    /// Decode from a skill node's `properties`.
    ///
    /// `skill` has a registered core schema (ADR-030), so `NodeService` hoists
    /// its schema-defined fields under `properties.skill.*` on write (same as
    /// `task` under `properties.task.*` — see `behaviors/mod.rs`'s
    /// task-property validation). Reading `node.properties` flat found
    /// nothing on any seeded skill node, so every skill's tools and
    /// entity-type guidance silently vanished at the routing gate. Fall back
    /// to the flat top level for a node that predates hoisting or was
    /// constructed directly, as every test in this module does.
    fn from_node_properties(properties: &Value) -> Self {
        let skill_props = properties
            .get("skill")
            .filter(|v| v.is_object())
            .unwrap_or(properties);

        let description = skill_props
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tool_whitelist = skill_props
            .get("tool_whitelist")
            .cloned()
            .unwrap_or(json!([]));
        let scoped_type_ids = skill_props
            .get("node_types")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Self {
            description,
            tool_whitelist,
            scoped_type_ids,
        }
    }
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
/// tool's declaration. Per-skill `node_types` (the other mechanism
/// `find_skills` already supports) cannot fix this case: the seeded skills
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
            && (mentions_phrase(&query_lower, &s.id.to_lowercase())
                || mentions_phrase(&query_lower, &s.content.to_lowercase()))
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

/// Search for skill nodes via semantic search and return flat results with
/// schema metadata for the matched skill's scoped types.
///
/// Returns up to `limit` matches (default 3) with `id`, `name`, `description`,
/// `confidence`, `tools`, `schema_metadata`, and `instructions`. The `instructions`
/// field is the skill's child subtree rendered to markdown — the actual procedure
/// the model must follow. The `schema_metadata` field contains type IDs,
/// field names, and enum values for entity types associated with the skill's
/// `tool_whitelist` scope.
///
/// No filtering or bucketing — the caller (model or MCP client) inspects the
/// raw confidence score and decides how to act. An empty `skills` array is a
/// meaningful signal: "no skill is even loosely related to this query."
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
    let skill_results = embedding_service
        .semantic_search_nodes_of_type(&input.query, "skill", limit, SKILL_SEARCH_THRESHOLD)
        .await
        .map_err(|e| OpsError::Internal(format!("Skill search failed: {}", e)))?;

    // Fetch all schemas once; used to attach metadata to each matched skill.
    let all_schemas = node_service
        .get_all_schemas()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "find_skills: failed to fetch schemas for metadata attachment");
        })
        .unwrap_or_default();

    let total_results = skill_results.len();
    let mut skills = Vec::with_capacity(total_results);

    // Computed once per call, not per matched skill: it depends only on the
    // query text and the schema list, neither of which varies across
    // `skill_results`. See `schema_named_in_query` — `None` when the query
    // doesn't determine a single non-core type, in which case every
    // unscoped-branch candidate below keeps today's fallback unchanged.
    let query_named_schema = schema_named_in_query(&input.query, &all_schemas);

    for (node, confidence) in &skill_results {
        let SkillProperties {
            description,
            tool_whitelist,
            scoped_type_ids,
        } = SkillProperties::from_node_properties(&node.properties);

        // Attach schema metadata for entity types relevant to this skill.
        // The skill's `node_types` property lists the type IDs in scope. When
        // absent: if the query itself names exactly one non-core type, scope
        // to that type (see `schema_named_in_query`); otherwise fall back to
        // all custom (non-core) schemas, capped at MAX_UNSCOPED_SCHEMA_METADATA
        // to bound token cost for general-purpose skills whose query didn't
        // resolve to one type.
        let schema_metadata: Vec<Value> = if scoped_type_ids.is_empty() {
            match query_named_schema {
                Some(named) => vec![
                    super::entity_types_block::EntityTypeDescriptor::from_schema(named).to_json(),
                ],
                None => all_schemas
                    .iter()
                    .filter(|s| !s.is_core)
                    .take(MAX_UNSCOPED_SCHEMA_METADATA)
                    .map(|s| {
                        // Encoded from the same descriptor the prompt block
                        // renders from, so this JSON cannot describe a schema
                        // differently than the model is told about it.
                        super::entity_types_block::EntityTypeDescriptor::from_schema(s).to_json()
                    })
                    .collect(),
            }
        } else {
            all_schemas
                .iter()
                .filter(|s| scoped_type_ids.contains(&s.id))
                .take(scoped_type_ids.len())
                .map(|s| super::entity_types_block::EntityTypeDescriptor::from_schema(s).to_json())
                .collect()
        };

        let instructions = render_skill_instructions(node_service.as_ref(), &node.id).await;

        skills.push(json!({
            "id": node.id,
            "name": node.content,
            "description": description,
            "confidence": confidence,
            "tools": tool_whitelist,
            "schema_metadata": schema_metadata,
            "instructions": instructions,
        }));
    }

    let all_scores = format_all_scores(&skill_results);

    tracing::info!(
        query = %input.query,
        results_found = total_results,
        top_score = skill_results.first().map(|(_, s)| *s).unwrap_or(0.0),
        all_scores = %all_scores,
        "find_skills executed"
    );

    Ok(FindSkillsOutput {
        skills,
        query: input.query,
        total_results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Node, SchemaNode};
    use serde_json::json;
    use std::collections::HashMap;

    fn make_schema(id: &str, content: &str, is_core: bool) -> SchemaNode {
        SchemaNode::from_node(Node::new_with_id(
            id.to_string(),
            "schema".to_string(),
            content.to_string(),
            json!({ "isCore": is_core, "fields": [] }),
        ))
        .expect("schema node")
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
        assert_eq!(found.map(|s| s.id.as_str()), Some("ticket"));
    }

    #[test]
    fn schema_named_in_query_matches_a_snake_case_id_by_id_alone() {
        // Regression: the id-path must work even when the display name
        // (content) doesn't share the same words as the id, so this only
        // passes via `id` matching, not `content`.
        let schemas = vec![
            make_schema("release_plan", "Q3 Rollout", false),
            make_schema("adr", "ADR", false),
        ];
        let found = schema_named_in_query("create a release plan for q3", &schemas);
        assert_eq!(found.map(|s| s.id.as_str()), Some("release_plan"));
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
        assert_eq!(found.map(|s| s.id.as_str()), Some("adr"));
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
        assert_eq!(found.map(|s| s.id.as_str()), Some("ticket"));
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
                    matched.id, query
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
        assert_eq!(found.map(|s| s.id.as_str()), Some("ticket"));
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
            found.map(|s| s.id.as_str()),
            Some("note"),
            "a schema literally named `Note` lexically matches an unrelated \
             use of the word \"note\" — confirmed, accepted risk, not a bug \
             in this test"
        );

        let schemas = vec![make_schema("meeting", "Meeting", false)];
        let found = schema_named_in_query("what time does the meeting start", &schemas);
        assert_eq!(found.map(|s| s.id.as_str()), Some("meeting"));

        // Every one of the 29 risky words is ALSO the exact word a genuine
        // true-positive query for that same type would use — there is no
        // lexical signal available to `mentions_phrase` that tells the two
        // apart, which is why no length-floor or stopword-exclusion guard
        // is added here (see the doc comment on `schema_named_in_query`).
        let schemas = vec![make_schema("log", "Log", false)];
        let false_positive = schema_named_in_query("log me out of this session", &schemas);
        let true_positive = schema_named_in_query("add a log for today's workout", &schemas);
        assert_eq!(false_positive.map(|s| s.id.as_str()), Some("log"));
        assert_eq!(true_positive.map(|s| s.id.as_str()), Some("log"));
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
    fn skill_properties_reads_the_hoisted_shape_node_service_actually_writes() {
        // The shape every seeded skill node has in the live database: `skill`
        // is a schema-typed node type (ADR-030), so NodeService hoists its
        // schema-defined fields under `properties.skill.*` on write, the same
        // as `task` under `properties.task.*`. A reader expecting flat
        // `properties.description` finds nothing on a real node.
        let properties = json!({
            "skill": {
                "description": "Modify existing nodes",
                "tool_whitelist": ["update_node", "resolve_query"],
                "node_types": ["invoice"],
            }
        });

        assert_eq!(
            SkillProperties::from_node_properties(&properties),
            SkillProperties {
                description: "Modify existing nodes".to_string(),
                tool_whitelist: json!(["update_node", "resolve_query"]),
                scoped_type_ids: vec!["invoice".to_string()],
            }
        );
    }

    #[test]
    fn skill_properties_falls_back_to_flat_shape() {
        // A node with no `skill` namespace key at all (predates hoisting, or
        // constructed directly the way every other test in this module does)
        // must still read correctly rather than silently returning defaults.
        let properties = json!({
            "description": "Modify existing nodes",
            "tool_whitelist": ["update_node"],
        });

        assert_eq!(
            SkillProperties::from_node_properties(&properties),
            SkillProperties {
                description: "Modify existing nodes".to_string(),
                tool_whitelist: json!(["update_node"]),
                scoped_type_ids: vec![],
            }
        );
    }

    #[test]
    fn skill_properties_falls_back_to_flat_shape_when_skill_key_is_null() {
        // `properties.get("skill")` returning `Some(&Value::Null)` must not
        // be treated as "the namespace is present" — a bare `.unwrap_or`
        // would substitute `Null` instead of falling back, silently
        // discarding any flat data sitting alongside it. Not reachable
        // through NodeService's real write path (hoisting always leaves an
        // object, never `null`), but a node built by hand or through a raw
        // store write could have this shape.
        let properties = json!({
            "skill": null,
            "description": "Modify existing nodes",
            "tool_whitelist": ["update_node"],
        });

        assert_eq!(
            SkillProperties::from_node_properties(&properties),
            SkillProperties {
                description: "Modify existing nodes".to_string(),
                tool_whitelist: json!(["update_node"]),
                scoped_type_ids: vec![],
            }
        );
    }

    #[test]
    fn skill_properties_missing_entirely_defaults_safely() {
        assert_eq!(
            SkillProperties::from_node_properties(&json!({})),
            SkillProperties {
                description: String::new(),
                tool_whitelist: json!([]),
                scoped_type_ids: vec![],
            }
        );
    }

    #[test]
    fn flatten_subtree_content_empty_skill() {
        let node_map: HashMap<String, Node> = HashMap::new();
        let adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        let parts = flatten_subtree_content("skill-root", &node_map, &adjacency_list);
        assert!(parts.is_empty());
    }

    #[test]
    fn flatten_subtree_content_flat_children() {
        let mut node_map = HashMap::new();
        node_map.insert("c1".to_string(), make_node("c1", "Step one"));
        node_map.insert("c2".to_string(), make_node("c2", "Step two"));
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        adjacency_list.insert(
            "skill-root".to_string(),
            vec!["c1".to_string(), "c2".to_string()],
        );
        let parts = flatten_subtree_content("skill-root", &node_map, &adjacency_list);
        assert_eq!(parts, vec!["Step one", "Step two"]);
    }

    #[test]
    fn flatten_subtree_content_nested_children() {
        let mut node_map = HashMap::new();
        node_map.insert("c1".to_string(), make_node("c1", "Section header"));
        node_map.insert("c1a".to_string(), make_node("c1a", "Sub-step A"));
        node_map.insert("c2".to_string(), make_node("c2", "Another section"));
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        adjacency_list.insert(
            "skill-root".to_string(),
            vec!["c1".to_string(), "c2".to_string()],
        );
        adjacency_list.insert("c1".to_string(), vec!["c1a".to_string()]);
        let parts = flatten_subtree_content("skill-root", &node_map, &adjacency_list);
        assert_eq!(
            parts,
            vec!["Section header", "Sub-step A", "Another section"]
        );
    }

    #[test]
    fn flatten_subtree_content_skips_empty_content() {
        let mut node_map = HashMap::new();
        node_map.insert("c1".to_string(), make_node("c1", ""));
        node_map.insert("c2".to_string(), make_node("c2", "Has content"));
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        adjacency_list.insert(
            "skill-root".to_string(),
            vec!["c1".to_string(), "c2".to_string()],
        );
        let parts = flatten_subtree_content("skill-root", &node_map, &adjacency_list);
        assert_eq!(parts, vec!["Has content"]);
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
        assert!(filter.matches("skill", &empty_props));
        assert!(!filter.matches("text", &empty_props));
        assert!(!filter.matches("schema", &empty_props));
        assert!(!filter.matches("ai-chat", &empty_props));
    }

    #[test]
    fn flatten_subtree_content_join_produces_instructions_string() {
        // Verify the full flatten_subtree_content → join pipeline that produces the
        // `instructions` value delivered to the model. A regression in the flatten
        // logic (e.g. wrong separator, missing DFS step) breaks this test.
        let mut node_map = HashMap::new();
        node_map.insert("c1".to_string(), make_node("c1", "Step one"));
        node_map.insert("c2".to_string(), make_node("c2", "Step two"));
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        adjacency_list.insert(
            "skill-root".to_string(),
            vec!["c1".to_string(), "c2".to_string()],
        );
        let instructions =
            flatten_subtree_content("skill-root", &node_map, &adjacency_list).join("\n\n");
        assert_eq!(instructions, "Step one\n\nStep two");
    }
}
