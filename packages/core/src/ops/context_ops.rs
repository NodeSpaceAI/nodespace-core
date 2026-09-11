//! Workspace context assembly for AI agent prompts.
//!
//! Builds a compact representation of collections and active playbooks from
//! the database. The output is formatted as a token-efficient string suitable
//! for injection into a small-model system prompt.
//!
//! When a query + embedding service are provided, schemas semantically relevant
//! to the query are injected as an EXISTING SCHEMAS block, covering implicit
//! references (e.g. "track my clients" → `customer` schema).
//!
//! That block is the *only* per-type field metadata the model receives on a
//! creation turn, so it renders each field's name, type, and required-ness. The
//! node-creation guidance conditions its `properties` population on exactly
//! those three, and a name-only rendering left those instructions with no
//! referent — the model then omitted `properties` entirely and persisted bare
//! shells.
//!
//! The block itself is rendered by [`super::entity_types_block`], which the
//! skill-routing path shares. The two used to hold independent renderers and
//! drifted into exactly the failure above; the shared descriptor is what now
//! prevents that.
//!
//! The retrieval query itself is assembled by [`build_retrieval_query`], which
//! blends the preceding conversation turns with the current message so that
//! follow-ups referring to their subject by pronoun or ellipsis still retrieve
//! the right schema.

use crate::models::{Node, SchemaNode};
use crate::services::{CollectionService, NodeEmbeddingService, NodeService};
use std::sync::Arc;

use super::OpsError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Assembled workspace context from the database.
#[derive(Default)]
pub struct WorkspaceContext {
    pub collections: Vec<String>,
    pub active_playbooks: Vec<PlaybookInfo>,
    /// Schemas semantically relevant to the current query (may be empty).
    pub relevant_schemas: Vec<SchemaNode>,
    /// Schemas one relationship hop from `relevant_schemas`, never matched by
    /// the query itself (may be empty). Rendered name-only — see
    /// `related_one_hop_schemas` for the traversal. Only ever populated when
    /// `relevant_schemas` is non-empty — there is nothing to traverse from
    /// otherwise.
    pub related_schemas: Vec<SchemaNode>,
    /// Count of `relevant_schemas` entries that came from semantic retrieval,
    /// BEFORE `append_schemas_named_in_query`'s lexical backstop ran.
    ///
    /// A caller that adds its own entries to `relevant_schemas` on top of this
    /// (e.g. `local_agent_service.rs`'s recently-created-schema injector) needs
    /// a budget computed against what semantic search actually returned, not
    /// against the post-lexical-append length — otherwise an unrelated,
    /// unbounded signal writing into the same vector can silently zero out the
    /// injector's remaining slots. See `local_agent_service.rs::build_workspace_context`.
    ///
    /// An upper bound on the true final semantic-sourced count, not always
    /// exact: it is captured from the raw retrieval hits, before the
    /// hydration step re-resolves each hit against the full schema corpus and
    /// can drop one that no longer exists there (e.g. deleted between
    /// retrieval and hydration). That direction of error is harmless for the
    /// budget above — it can only make a caller slightly more conservative
    /// (fewer slots believed available than truly are), never reproduce the
    /// starvation this field exists to prevent.
    pub semantic_schema_count: usize,
}

/// An active playbook.
pub struct PlaybookInfo {
    pub name: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Similarity threshold for schema semantic search.
///
/// 0.2 is intentionally permissive — the full user message is embedded verbatim
/// (not entity-extracted), so action-oriented phrasing like "Add an invoice for
/// $500 due next Friday" produces a noisier embedding than a bare entity name.
/// At 0.4 the Invoice schema was missed for that query. The MAX_SEMANTIC_SCHEMAS
/// cap keeps the prompt compact even with a lower threshold.
const SCHEMA_SIMILARITY_THRESHOLD: f32 = 0.2;

/// Maximum number of schemas to inject per turn.
///
/// Five covers the common multi-entity case (primary type + related types).
/// The lower threshold above means more candidates pass; the cap keeps the
/// injected EXISTING SCHEMAS section from bloating in large workspaces.
const MAX_SEMANTIC_SCHEMAS: usize = 5;

/// Number of preceding messages blended into the retrieval query.
///
/// Counts messages, not exchanges — two is typically one prior round trip
/// (the previous user message and the assistant's reply), which is the shape
/// that was measured.
///
/// Two is what that measurement produced: recall over multi-turn conversations
/// went from 77% to 100% with the last two messages prepended, while
/// topic-switch recall held at 100%. The latest message still dominates the
/// embedding, and the cap is `MAX_SEMANTIC_SCHEMAS`, not one, so the extra
/// context adds candidates rather than displacing the right one.
const BLENDED_HISTORY_TURNS: usize = 2;

/// Heading for a rendered "existing schemas" listing, shared by every site
/// that shows the model retrieved/candidate schema metadata.
///
/// The anti-copy clause is inline in the heading — the same place the model
/// reads the fields it must not reuse — rather than only in resident/skill
/// prose elsewhere in the prompt. A prose-only rule measured no effect: the
/// model still copied an unrelated schema's fields verbatim onto a new type
/// even with a rule against it present in the Schema Creation skill
/// instructions.
///
/// A single constant, not independently-worded copies at each call site: this
/// heading is rendered at two of them (this module's resident workspace
/// context, and `local_agent::routing`'s Stage-2 candidate metadata), and an
/// earlier version of this fix touched only one, leaving the other to
/// reinforce the exact contamination the first was changed to guard against.
/// A shared constant makes that class of drift a compile error instead of a
/// silent one.
///
/// Residual: even with the clause present at both sites, contamination is
/// reduced but not eliminated on the locked 4B model (gemma-4-e4b-q4km) — 4 of
/// 5 independent measured trials were clean, 1 of 5 was not. A fully
/// deterministic fix needs context assembly to depend on the routing
/// decision (e.g. omitting other custom-type schemas' fields when the turn is
/// routed toward `create_schema`), which is a larger change since today's
/// context block is assembled before ADR-038's Stage 2 routing runs — tracked
/// as a follow-up rather than blocking this measured improvement on it.
pub const EXISTING_SCHEMAS_HEADER: &str =
    "EXISTING SCHEMAS (do not recreate these; do not copy their fields onto a new type):";

/// Character budget applied to each blended prior turn.
///
/// A prior turn contributes context, not content: all it has to supply is the
/// vocabulary a pronoun or ellipsis refers back to. An assistant turn is model
/// output and has no natural length bound — it can be long-form prose or a
/// pasted list — and the query embedding path applies no truncation of its own,
/// so an unbounded turn would both dominate the pooled embedding and enlarge
/// the embedding context's batch size for the life of the process.
///
/// The trailing characters are kept rather than the leading ones: a referent
/// introduced mid-turn is nearest the end, and the current message is appended
/// after, so the words closest to the follow-up survive.
const MAX_CHARS_PER_BLENDED_TURN: usize = 400;

/// Last `max_chars` characters of `text`, respecting char boundaries.
///
/// Returns `text` unchanged when it is already within the budget. `max_chars`
/// is expected to be >= 1; `0` yields the final character rather than an empty
/// string, since an empty budget has no caller and no useful meaning here.
fn trailing_chars(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth_back(max_chars.saturating_sub(1)) {
        Some((start, _)) => &text[start..],
        None => text,
    }
}

/// Find schemas one relationship hop from `retrieved`, searching `all_schemas`.
///
/// Traversal is bidirectional: a schema in `retrieved` reaches a target via its
/// own `relationships[].target_type` (outgoing), and is also reached by any
/// other schema in `all_schemas` whose `relationships[].target_type` names it
/// (incoming). Incoming reachability is required — a hub schema like
/// `customer` typically declares no outgoing relationships of its own, only
/// incoming ones from `invoice`, `freelance_gig`, etc. A one-directional
/// (outgoing-only) traversal would miss that case entirely.
///
/// No recursive expansion: only schemas directly one hop from the retrieved
/// set are returned, regardless of what those schemas relate to in turn.
/// Schemas already present in `retrieved` are never duplicated into the
/// result.
fn related_one_hop_schemas(
    retrieved: &[SchemaNode],
    all_schemas: &[SchemaNode],
) -> Vec<SchemaNode> {
    let retrieved_ids: std::collections::HashSet<&str> =
        retrieved.iter().map(|s| s.id.as_str()).collect();

    let mut related_ids: Vec<&str> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for schema in retrieved {
        // Outgoing: this schema declares a relationship to another type.
        for rel in &schema.relationships {
            if let Some(target) = rel.target_type.as_deref() {
                if !retrieved_ids.contains(target) && seen.insert(target) {
                    related_ids.push(target);
                }
            }
        }
    }

    // Incoming: some other schema in the corpus declares a relationship
    // targeting this schema.
    for candidate in all_schemas {
        if retrieved_ids.contains(candidate.id.as_str()) {
            continue;
        }
        let points_at_retrieved = candidate.relationships.iter().any(|rel| {
            rel.target_type
                .as_deref()
                .is_some_and(|t| retrieved_ids.contains(t))
        });
        if points_at_retrieved && seen.insert(candidate.id.as_str()) {
            related_ids.push(candidate.id.as_str());
        }
    }

    // filter_map, not map: an outgoing relationship's target_type can name a
    // schema id that no longer exists in the corpus (e.g. deleted without
    // cleaning up the relationship that pointed at it). There is nothing to
    // render for a schema we don't have, so a dangling reference is silently
    // dropped here rather than surfaced as an error.
    //
    // is_core is excluded here too: a user-defined schema can perfectly
    // ordinarily declare a relationship to a core type (task, text, date),
    // which would otherwise place that core type in `related_schemas`. It
    // renders under the RELATED heading rather than EXISTING SCHEMAS, so
    // this isn't the same ADR-063 hazard `parse_and_filter_non_core_schemas`
    // guards against — but there's no reason for a core type to appear in
    // either block, so the two stay consistent.
    related_ids
        .into_iter()
        .filter_map(|id| all_schemas.iter().find(|s| s.id == id).cloned())
        .filter(|s| !s.is_core)
        .collect()
}

/// Parse semantic search results into [`SchemaNode`]s, excluding core types.
///
/// The results are raw storage nodes, so the parsed schemas carry NO
/// relationships (declarations are relationship-table rows, not a properties
/// key) — callers that need them re-resolve each hit from the hydrated corpus
/// returned by `get_all_schemas` (see `build_workspace_context`).
///
/// Retrieval is scoped only by `node_type == "schema"`, and `text`/`task`/
/// `date` are stored schema nodes with embeddable content, so an unfiltered
/// pass-through can surface them. Excluding `is_core` here mirrors the guard
/// `local_agent_service.rs`'s recently-created-schema injection already
/// applies before writing into this same `relevant_schemas` vector.
///
/// The `create_node` guidance treats presence in EXISTING SCHEMAS as
/// proof a type is user-defined (bare property keys) versus built-in
/// (`custom:`-prefixed) — a core type reaching this block would make the
/// model write a bare key onto a core type, the exact ADR-063 violation that
/// guidance exists to prevent.
fn parse_and_filter_non_core_schemas(results: Vec<(Node, f64)>) -> Vec<SchemaNode> {
    results
        .into_iter()
        .filter_map(|(node, _score)| SchemaNode::from_node(node).ok())
        .filter(|s| !s.is_core)
        .collect()
}

/// Append any non-core schema the query names outright to the semantically
/// retrieved set, skipping ones already present.
///
/// The lexical backstop for schema retrieval. Split out as a pure function so
/// the behaviour is testable without a live `NodeService` and embedding
/// service; see `build_workspace_context` for why it exists.
///
/// Append rather than replace: semantic retrieval and naming answer different
/// questions, and a type named outright is the strongest available evidence
/// that it is relevant. Order is preserved so semantic relevance still leads.
fn append_schemas_named_in_query(
    mut hits: Vec<SchemaNode>,
    all_schemas: &[SchemaNode],
    query: &str,
) -> Vec<SchemaNode> {
    let q_lower = query.to_lowercase();
    for s in all_schemas.iter().filter(|s| !s.is_core) {
        let named = crate::ops::skill_ops::mentions_phrase(&q_lower, &s.id.to_lowercase())
            || crate::ops::skill_ops::mentions_phrase(&q_lower, &s.content.to_lowercase());
        if named && !hits.iter().any(|h| h.id == s.id) {
            tracing::debug!(
                schema_id = %s.id,
                query = query,
                "workspace_context: schema recovered by name (not yet embedded, or below \
                 the similarity threshold)"
            );
            hits.push(s.clone());
        }
    }
    hits
}

/// Build the embedding query for schema retrieval from conversation context.
///
/// The last [`BLENDED_HISTORY_TURNS`] turns are prepended to `current_message`,
/// oldest first, so a follow-up that names its subject only by pronoun or
/// ellipsis ("Set the Redwood one to rejected", "Which ones are still out?")
/// still carries the discriminating words from the turn that introduced it.
/// Each prior turn is capped at [`MAX_CHARS_PER_BLENDED_TURN`]; the current
/// message is never truncated.
///
/// Raw text is concatenated verbatim. Summarizing or entity-extracting the
/// turns first was measured and made recall *worse* (100% → 73%): the
/// abstraction discards the surface words the embedder matches on. Callers
/// should pass conversational turns only — synthetic system messages dilute the
/// query without adding discriminating terms.
///
/// This affects the embedding input only. The rendered prompt block is built
/// from the retrieved schemas and is unchanged by blending.
pub fn build_retrieval_query(prior_turns: &[&str], current_message: &str) -> String {
    let recent: Vec<&str> = prior_turns
        .iter()
        .rev()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .take(BLENDED_HISTORY_TURNS)
        .map(|t| trailing_chars(t, MAX_CHARS_PER_BLENDED_TURN))
        .collect();

    let mut parts: Vec<&str> = recent.into_iter().rev().collect();
    let current = current_message.trim();
    if !current.is_empty() {
        parts.push(current);
    }
    parts.join("\n")
}

/// Build workspace context by querying collections and playbooks.
///
/// When `embedding_service` and `query` are both provided, schema nodes
/// semantically similar to the query are retrieved and injected into the
/// context. Falls back to schema-free context when the embedding service is
/// unavailable or the query is empty.
pub async fn build_workspace_context(
    node_service: &Arc<NodeService>,
    embedding_service: Option<&Arc<NodeEmbeddingService>>,
    query: Option<&str>,
) -> Result<WorkspaceContext, OpsError> {
    // Fetch collection names
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let collections = collection_service
        .get_all_collection_names()
        .await
        .unwrap_or_default();

    // Fetch active playbooks
    let playbook_nodes = node_service
        .query_nodes_by_type("play", Some("active"))
        .await
        .unwrap_or_default();

    // Convert playbook nodes
    let active_playbooks: Vec<PlaybookInfo> = playbook_nodes
        .into_iter()
        .map(|node| PlaybookInfo {
            name: node.content.clone(),
            description: node
                .properties
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect();

    // Semantic schema retrieval: find schemas relevant to the query.
    // Only runs when both an embedding service and a non-empty query are present.
    let retrieved_hits = match (embedding_service, query.filter(|q| !q.trim().is_empty())) {
        (Some(emb), Some(q)) => {
            match emb
                .semantic_search_nodes_of_type(
                    q,
                    "schema",
                    MAX_SEMANTIC_SCHEMAS,
                    SCHEMA_SIMILARITY_THRESHOLD,
                )
                .await
            {
                Ok(results) => {
                    let schemas = parse_and_filter_non_core_schemas(results);
                    tracing::debug!(
                        count = schemas.len(),
                        query = q,
                        "workspace_context: semantic schema retrieval"
                    );
                    schemas
                }
                Err(e) => {
                    tracing::warn!(error = %e, "workspace_context: semantic schema search failed, omitting schemas");
                    vec![]
                }
            }
        }
        _ => vec![],
    };

    // Captured before the lexical backstop below appends to `retrieved_hits` —
    // this is the count a caller layering its own additions on top of
    // `relevant_schemas` (e.g. the recently-created-schema injector in
    // `local_agent_service.rs`) needs for a budget that isn't blind to what
    // the lexical backstop already consumed. See `semantic_schema_count`.
    let semantic_schema_count = retrieved_hits.len();

    // Lexical backstop for a schema the semantic index cannot see yet.
    //
    // Embeddings are generated on a ~30s debounce (`EmbeddingService`'s
    // `debounce_duration_secs`), so a type the user just defined is NOT
    // semantically retrievable for half a minute after `create_schema` returns.
    // There is no fallback below this point: an empty `retrieved_hits` omits the
    // EXISTING SCHEMAS block entirely, which tells the model the type does not
    // exist. It then either invents a `node_type` or re-runs `create_schema`.
    //
    // Measured on the locked model, create-then-use inside the window:
    //     [tool] create_node [ERROR]  Unknown node_type 'feature_writeups'
    // and in the run that succeeded, the instance was created 1.4s after its
    // schema's embedding landed — the chain passed on timing alone.
    //
    // This is a real user-facing sequence ("track my write-ups" → "add one"),
    // not only an eval artifact, so the repair belongs here rather than in the
    // harness. Named types are appended to whatever semantic retrieval found
    // rather than replacing it: the two signals answer different questions, and
    // a type named outright in the request is the strongest evidence available
    // that it is relevant.
    //
    // Purely mechanical and already precedented — `skill_ops::schema_named_in_query`
    // matches non-core schemas against the query the same way, with the same
    // token-boundary matcher, for the same reason. Core schemas stay excluded
    // (as they are from the semantic path via `parse_and_filter_non_core_schemas`),
    // and dedup keeps a hit found by both signals from being injected twice.
    // The schema corpus, fetched ONCE and shared by both consumers below: the
    // lexical backstop needs it to match names, and the hydration step needs it
    // in full anyway (incoming reachability depends on schemas outside the
    // retrieved set). Two separate `get_all_schemas()` awaits here meant two
    // full-table reads per turn where one serves.
    //
    // Still conditional, though. Neither consumer exists on a turn with no
    // query and no hits — a resident context build with no message to retrieve
    // against — and the read before this change was gated on `retrieved_hits`
    // being non-empty. Fetching unconditionally would add a full-table read to
    // exactly the turns that previously did none.
    let lexical_query = query.filter(|q| !q.trim().is_empty());
    let all_schemas = if lexical_query.is_none() && retrieved_hits.is_empty() {
        None
    } else {
        match node_service.get_all_schemas().await {
            Ok(schemas) => Some(schemas),
            Err(e) => {
                tracing::warn!(error = %e, "workspace_context: fetching the schema corpus failed; the lexical backstop and relationship hydration are both skipped this turn");
                None
            }
        }
    };

    let retrieved_hits = match (lexical_query, &all_schemas) {
        (Some(q), Some(schemas)) => append_schemas_named_in_query(retrieved_hits, schemas, q),
        _ => retrieved_hits,
    };

    // Search results are raw storage nodes, and relationship declarations are
    // relationship-table rows rather than a `properties` key — so the parsed
    // hits carry no relationships. Re-resolve each hit (preserving relevance
    // order) from the hydrated schema corpus.
    let (relevant_schemas, related_schemas) = match (&all_schemas, retrieved_hits.is_empty()) {
        (_, true) => (vec![], vec![]),
        (Some(schemas), false) => {
            let relevant: Vec<SchemaNode> = retrieved_hits
                .iter()
                .filter_map(|hit| schemas.iter().find(|s| s.id == hit.id).cloned())
                .collect();
            let related = related_one_hop_schemas(&relevant, schemas);
            (relevant, related)
        }
        // Corpus unavailable: fall back to the unhydrated retrieval hits rather
        // than dropping them, and omit related schemas. Same behaviour as
        // before, now expressed once instead of in a second error arm.
        (None, false) => (retrieved_hits, vec![]),
    };

    Ok(WorkspaceContext {
        collections,
        active_playbooks,
        relevant_schemas,
        related_schemas,
        semantic_schema_count,
    })
}

// ---------------------------------------------------------------------------
// Formatter
// ---------------------------------------------------------------------------

impl WorkspaceContext {
    /// Format context as a compact string for injection into a system prompt.
    ///
    /// Semantically-relevant schemas are injected when present (retrieved via
    /// vector similarity by `build_workspace_context` — covers implicit type
    /// references like "clients" → `customer` schema). All other entity
    /// types remain on-demand via `search_skills`.
    ///
    /// `max_chars` is a rough character budget for the combined output.
    pub fn format_for_prompt(&self, max_chars: usize) -> String {
        let mut out = String::new();

        // Collections section
        if !self.collections.is_empty() {
            let section = format!("COLLECTIONS: {}\n", self.collections.join(", "));
            if out.len() + section.len() <= max_chars {
                out.push_str(&section);
            }
        }

        // Relevant schemas section (query-matched via semantic retrieval)
        if !self.relevant_schemas.is_empty() {
            let header = format!("\n{EXISTING_SCHEMAS_HEADER}\n");
            if out.len() + header.len() <= max_chars {
                out.push_str(&header);
                for schema in &self.relevant_schemas {
                    // Rendered through the shared descriptor so this block and
                    // the skill-routing one cannot drift: a field added to
                    // `SchemaField` reaches the prompt only via that choke
                    // point. The renderer carries the reasoning for what each
                    // part of the line is for.
                    let line = format!(
                        "{}\n",
                        super::entity_types_block::EntityTypeDescriptor::from_schema(schema)
                            .render_line()
                    );
                    if out.len() + line.len() > max_chars {
                        break;
                    }
                    out.push_str(&line);
                }
            }
        }

        // Related schemas section (one hop via relationship, name-only)
        if !self.related_schemas.is_empty() {
            let header = "\nRELATED (via relationship, not directly matched):\n";
            if out.len() + header.len() <= max_chars {
                out.push_str(header);
                for schema in &self.related_schemas {
                    let line = format!("- {}: {}\n", schema.id, schema.content);
                    if out.len() + line.len() > max_chars {
                        break;
                    }
                    out.push_str(&line);
                }
            }
        }

        // Playbooks section
        if !self.active_playbooks.is_empty() {
            let header = "\nACTIVE PLAYBOOKS:\n";
            if out.len() + header.len() <= max_chars {
                out.push_str(header);
                for pb in &self.active_playbooks {
                    let line = if pb.description.is_empty() {
                        format!("- \"{}\"\n", pb.name)
                    } else {
                        format!("- \"{}\": {}\n", pb.name, pb.description)
                    };
                    if out.len() + line.len() > max_chars {
                        break;
                    }
                    out.push_str(&line);
                }
            }
        }

        out
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_context() -> WorkspaceContext {
        WorkspaceContext {
            collections: vec!["Projects".into(), "Clients".into(), "Research".into()],
            active_playbooks: vec![PlaybookInfo {
                name: "Task completion".into(),
                description: "When task.status -> Done, evaluate project progress".into(),
            }],
            relevant_schemas: vec![],
            related_schemas: vec![],
            semantic_schema_count: 0,
        }
    }

    fn sample_schema(id: &str, display_name: &str, fields: &[&str]) -> crate::models::SchemaNode {
        sample_schema_with_relationships(id, display_name, fields, vec![])
    }

    /// Matrix scenario 13's seeded SCHEMA reaches the system prompt; its seeded
    /// INSTANCES do not. This is the boundary that scenario is built on.
    ///
    /// 13 asks the model to resolve "the incident Rowan was on call for"
    /// against records seeded outside any scored turn. The absence proof in the
    /// daemon (`scenario_13_seeded_referent_is_absent_from_history`) covers the
    /// CHAT HISTORY channel and is correct about it. It is not the only channel,
    /// which is what this test exists to say out loud.
    ///
    /// Seeding creates a schema as well as three instances, and workspace
    /// context retrieves schemas semantically (`SCHEMA_SIMILARITY_THRESHOLD`,
    /// a permissive 0.2) and interpolates them into the system prompt. So the
    /// seeded TYPE NAME and its FIELD NAMES — including `on_call`, the very
    /// property 13's reference keys off — are visible to the model on the turn
    /// being scored.
    ///
    /// WHY 13 IS STILL SOUND. What leaks is the VOCABULARY, not the ANSWER.
    /// Only `"schema"`-type nodes are retrieved here, so the incident titles
    /// and — critically — the `rowan` -> `search index corruption` mapping stay
    /// out. Knowing that an `incident_report` type exists with an `on_call`
    /// field tells the model how to ASK; it does not tell it which incident to
    /// update. The read is still forced, which is the property #2248 asked for.
    ///
    /// SCOPE OF EACH HALF, because overstating exactly this is what went wrong
    /// three times in this area and a reader deserves to know which assertions
    /// are load-bearing:
    ///
    ///   - The POSITIVE half is the real content. It is what refuted the
    ///     original claim that seeded state leaves no trace in the prompt at
    ///     all, and it fails the moment schema rendering stops including type
    ///     or field names.
    ///   - The NEGATIVE half is close to vacuous by construction, and is
    ///     documented rather than dressed up: `relevant_schemas` is
    ///     `Vec<SchemaNode>`, which has no field capable of holding an instance
    ///     title or property value, so no input to this renderer could make
    ///     those assertions fail. They record the INVARIANT 13 depends on —
    ///     schema vocabulary in, instance data out — rather than actively
    ///     policing it.
    ///
    /// So this is not a tripwire for the dangerous change. A future path that
    /// routed instance data into the prompt would do so through a different
    /// field or a different block, and would sail past this test. What actually
    /// guards 13 is that only `"schema"`-type nodes are retrieved
    /// (`semantic_search_nodes_of_type` above); if that ever widens, this test
    /// will not notice and 13's referent becomes directly matchable.
    #[test]
    fn scenario_13_seeded_schema_reaches_the_prompt_but_its_instances_do_not() {
        let ctx = WorkspaceContext {
            collections: vec![],
            active_playbooks: vec![],
            relevant_schemas: vec![sample_schema(
                "incident_report",
                "incident_report",
                &["on_call", "resolved"],
            )],
            related_schemas: vec![],
            semantic_schema_count: 0,
        };

        let rendered = ctx.format_for_prompt(4000);

        // The leak, asserted rather than assumed away.
        assert!(
            rendered.contains("incident_report"),
            "the seeded type name is expected to reach the prompt via workspace \
             context — if it no longer does, scenario 13's comments overstate \
             the leak and should be relaxed: {rendered}"
        );
        assert!(
            rendered.contains("on_call"),
            "the seeded field name is expected to reach the prompt — this is the \
             property scenario 13's reference keys off, and knowing it exists is \
             what lets the model form the lookup at all: {rendered}"
        );

        // The boundary that keeps scenario 13 winnable-only-by-lookup. None of
        // these is instance data the schema block has any business carrying.
        for absent in [
            "search index corruption",
            "checkout latency spike",
            "auth token expiry storm",
            "rowan",
        ] {
            assert!(
                !rendered.to_lowercase().contains(absent),
                "'{absent}' is INSTANCE data and must never reach workspace \
                 context — if it does, scenario 13's referent is directly \
                 matchable from the system prompt and the scenario has \
                 degraded into the defect #2242 and #2250 each found: {rendered}"
            );
        }
    }

    fn schema_search_result(id: &str, is_core: bool) -> (Node, f64) {
        let node = Node::new_with_id(
            id.to_string(),
            "schema".to_string(),
            id.to_string(),
            serde_json::json!({ "isCore": is_core, "fields": [] }),
        );
        (node, 0.5)
    }

    /// A core type (`text`/`task`/`date`) is a stored schema node with
    /// embeddable content, so unfiltered retrieval would otherwise return it.
    /// Its presence in EXISTING SCHEMAS is what the `create_node`
    /// guidance reads as proof a type is user-defined, so an unfiltered core
    /// hit would make the model write a bare property key onto a core type —
    /// the ADR-063 violation that guidance exists to prevent.
    #[test]
    fn parse_and_filter_non_core_schemas_excludes_core_types() {
        let results = vec![
            schema_search_result("text", true),
            schema_search_result("customer", false),
            schema_search_result("task", true),
        ];

        let schemas = parse_and_filter_non_core_schemas(results);
        let ids: Vec<&str> = schemas.iter().map(|s| s.id.as_str()).collect();

        assert_eq!(ids, vec!["customer"]);
    }

    // -- lexical backstop for schema retrieval -------------------------------

    fn named_schema(id: &str, display: &str, is_core: bool) -> SchemaNode {
        SchemaNode::from_node(Node::new_with_id(
            id.to_string(),
            "schema".to_string(),
            display.to_string(),
            serde_json::json!({ "isCore": is_core, "fields": [] }),
        ))
        .expect("valid schema node")
    }

    /// Embeddings run on a ~30s debounce, so a type the user just defined is
    /// not semantically retrievable yet. Without this backstop the EXISTING
    /// SCHEMAS block is omitted entirely and the model is effectively told the
    /// type does not exist — observed live as
    /// `create_node [ERROR] Unknown node_type 'feature_writeups'`.
    #[test]
    fn schema_named_in_query_is_recovered_when_semantic_retrieval_is_empty() {
        let all = vec![named_schema("feature_write_up", "feature write-up", false)];
        let hits = append_schemas_named_in_query(vec![], &all, "Put one down for feature write-up");
        let ids: Vec<&str> = hits.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["feature_write_up"]);
    }

    /// Matched by snake_case id as well as by display name — `create_node`
    /// takes the id, and a user echoing it back is the same evidence.
    #[test]
    fn schema_named_by_snake_case_id_is_recovered() {
        let all = vec![named_schema("release_plan", "Release Plan", false)];
        let hits = append_schemas_named_in_query(vec![], &all, "add a release_plan for Q3");
        assert_eq!(hits.len(), 1);
    }

    /// A hit semantic retrieval already found is not injected twice.
    #[test]
    fn a_schema_found_by_both_signals_is_not_duplicated() {
        let all = vec![named_schema("invoice", "Invoice", false)];
        let already = vec![named_schema("invoice", "Invoice", false)];
        let hits = append_schemas_named_in_query(already, &all, "log an invoice");
        assert_eq!(hits.len(), 1, "duplicate schema injected: {hits:?}");
    }

    /// Core types stay excluded, exactly as they are from the semantic path.
    /// Their presence in EXISTING SCHEMAS is what `create_node` guidance reads
    /// as proof a type is user-defined, so admitting one here would reintroduce
    /// the ADR-063 violation `parse_and_filter_non_core_schemas` prevents.
    #[test]
    fn a_named_core_schema_is_not_recovered() {
        let all = vec![named_schema("task", "Task", true)];
        let hits = append_schemas_named_in_query(vec![], &all, "add a task to fix the build");
        assert!(hits.is_empty(), "core schema leaked into context: {hits:?}");
    }

    /// Token-boundary matching, inherited from `mentions_phrase`: a query that
    /// merely contains the id as a substring does not name the type.
    #[test]
    fn a_substring_match_does_not_count_as_naming_the_type() {
        let all = vec![named_schema("adr", "ADR", false)];
        let hits = append_schemas_named_in_query(vec![], &all, "update the address field");
        assert!(hits.is_empty(), "substring matched as a name: {hits:?}");
    }

    /// Semantic relevance still leads; the named type is appended after.
    #[test]
    fn semantic_hits_keep_their_order_ahead_of_a_named_recovery() {
        let all = vec![
            named_schema("invoice", "Invoice", false),
            named_schema("venue", "Venue", false),
        ];
        let hits = append_schemas_named_in_query(
            vec![named_schema("invoice", "Invoice", false)],
            &all,
            "book the venue for the invoice run",
        );
        let ids: Vec<&str> = hits.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["invoice", "venue"]);
    }

    /// The count a caller needs to budget a SEPARATE addition against
    /// `relevant_schemas` (e.g. `local_agent_service.rs`'s recently-created
    /// schema injector) must reflect what semantic retrieval actually found —
    /// NOT the length after this unbounded lexical backstop has appended to
    /// it. A turn naming several types by name can push the vector well past
    /// any per-turn cap on its own; a caller that read the post-append length
    /// as "how many slots has semantic search used" would see zero slots
    /// remaining and skip its own injection even when semantic search
    /// contributed nothing. Recorded as `semantic_schema_count` in
    /// `build_workspace_context`, captured before this function runs — this
    /// pins that the two counts diverge exactly when they need to.
    #[test]
    fn lexical_append_does_not_retroactively_inflate_the_semantic_count() {
        let all = vec![
            named_schema("invoice", "Invoice", false),
            named_schema("venue", "Venue", false),
            named_schema("customer", "Customer", false),
        ];
        let semantic_hits = vec![named_schema("invoice", "Invoice", false)];
        let semantic_schema_count = semantic_hits.len();

        let hits = append_schemas_named_in_query(
            semantic_hits,
            &all,
            "book the venue and log the customer for the invoice run",
        );

        assert_eq!(hits.len(), 3, "expected all three named schemas: {hits:?}");
        assert_eq!(
            semantic_schema_count, 1,
            "the count captured before lexical append must stay untouched by it"
        );
    }

    fn sample_schema_with_relationships(
        id: &str,
        display_name: &str,
        fields: &[&str],
        relationships: Vec<crate::models::schema::SchemaRelationship>,
    ) -> crate::models::SchemaNode {
        use crate::models::schema::SchemaField;
        crate::models::SchemaNode {
            id: id.to_string(),
            content: display_name.to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            is_core: false,
            schema_version: 1,
            fields: fields
                .iter()
                .map(|name| SchemaField {
                    name: name.to_string(),
                    friendly_name: name.to_string(),
                    field_type: "string".to_string(),
                    local_only: false,
                    protection: crate::models::schema::SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: None,
                    extensible: None,
                    default: None,
                    description: None,
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                })
                .collect(),
            relationships,
            title_template: None,
            properties_header_summary_template: None,
        }
    }

    fn outgoing_relationship(
        name: &str,
        target_type: &str,
    ) -> crate::models::schema::SchemaRelationship {
        use crate::models::schema::{
            RelationshipCardinality, RelationshipDirection, SchemaRelationship,
        };
        SchemaRelationship {
            name: name.to_string(),
            target_type: Some(target_type.to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: format!("reverse_{name}"),
            reverse_cardinality: RelationshipCardinality::Many,
            edge_fields: None,
            description: None,
        }
    }

    #[test]
    fn format_for_prompt_includes_collections_and_playbooks() {
        let ctx = sample_context();
        let output = ctx.format_for_prompt(4000);

        // Collections and playbooks are still injected
        assert!(output.contains("COLLECTIONS:"));
        assert!(output.contains("Projects"));
        assert!(output.contains("ACTIVE PLAYBOOKS:"));
        assert!(output.contains("Task completion"));

        // No schemas when relevant_schemas is empty
        assert!(!output.contains("EXISTING SCHEMAS"));
    }

    #[test]
    fn format_for_prompt_includes_relevant_schemas() {
        let mut ctx = sample_context();
        ctx.relevant_schemas = vec![sample_schema("customer", "Customer", &["name", "email"])];
        let output = ctx.format_for_prompt(4000);

        assert!(output.contains(EXISTING_SCHEMAS_HEADER));
        assert!(output.contains("customer \"Customer\""));
        // Each field carries its type, so the node-creation guidance's
        // instruction to read field names *and* types has a referent.
        assert!(output.contains("name: string; email: string"));
    }

    #[test]
    fn format_for_prompt_marks_required_fields() {
        use crate::models::schema::{SchemaField, SchemaProtectionLevel};

        let mut schema = sample_schema("invoice", "Invoice", &["reference"]);
        schema.fields.push(SchemaField {
            name: "amount".to_string(),
            friendly_name: "Amount".to_string(),
            field_type: "number".to_string(),
            protection: SchemaProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(true),
            extensible: None,
            default: None,
            description: None,
            item_type: None,
            fields: None,
            item_fields: None,
            unique: None,
            unique_case_insensitive: None,
            local_only: false,
        });

        let mut ctx = sample_context();
        ctx.relevant_schemas = vec![schema];
        let output = ctx.format_for_prompt(4000);

        // Required-ness is rendered; the guidance conditions inclusion on it.
        assert!(output.contains("amount: number, required"));
        // Fields without the flag are not marked required.
        assert!(output.contains("reference: string;"));
        assert!(!output.contains("reference: string, required"));
    }

    #[test]
    fn format_for_prompt_renders_enum_values() {
        use crate::models::schema::{EnumValue, SchemaField, SchemaProtectionLevel};

        let mut schema = sample_schema("ticket", "Ticket", &[]);
        schema.fields.push(SchemaField {
            name: "status".to_string(),
            friendly_name: "Status".to_string(),
            field_type: "enum".to_string(),
            protection: SchemaProtectionLevel::User,
            core_values: Some(vec![EnumValue {
                value: "open".to_string(),
                label: "Open".to_string(),
            }]),
            user_values: Some(vec![EnumValue {
                value: "blocked".to_string(),
                label: "Blocked".to_string(),
            }]),
            indexed: false,
            required: Some(true),
            extensible: None,
            default: None,
            description: None,
            item_type: None,
            fields: None,
            item_fields: None,
            unique: None,
            unique_case_insensitive: None,
            local_only: false,
        });

        let mut ctx = sample_context();
        ctx.relevant_schemas = vec![schema];
        let output = ctx.format_for_prompt(4000);

        // Core and user values both listed, so the model picks a legal one
        // instead of inventing a value the write path will reject.
        assert!(
            output.contains("status: enum {open, blocked}, required"),
            "got: {output}"
        );
    }

    #[test]
    fn format_for_prompt_renders_title_template() {
        let mut schema = sample_schema("invoice", "Invoice", &["reference"]);
        schema.title_template = Some("{reference}".to_string());

        let mut ctx = sample_context();
        ctx.relevant_schemas = vec![schema];
        let output = ctx.format_for_prompt(4000);

        // create_node's description promises the template is shown here.
        assert!(
            output.contains("[title_template: {reference}]"),
            "got: {output}"
        );
    }

    #[test]
    fn format_for_prompt_schema_no_fields() {
        let ctx = WorkspaceContext {
            collections: vec![],
            active_playbooks: vec![],
            relevant_schemas: vec![sample_schema("invoice", "Invoice", &[])],
            related_schemas: vec![],
            semantic_schema_count: 0,
        };
        let output = ctx.format_for_prompt(4000);
        assert!(output.contains("invoice \"Invoice\"\n"));
        // No `->` field-list marker when there are no fields.
        assert!(!output.contains("Invoice\" ->"));
    }

    #[test]
    fn format_for_prompt_truncates_on_budget() {
        let ctx = sample_context();
        // Very small budget — output is silently capped (no truncation note emitted)
        let output = ctx.format_for_prompt(100);
        assert!(output.len() <= 100);
    }

    #[test]
    fn format_for_prompt_empty_context() {
        let ctx = WorkspaceContext {
            collections: vec![],
            active_playbooks: vec![],
            relevant_schemas: vec![],
            related_schemas: vec![],
            semantic_schema_count: 0,
        };
        let output = ctx.format_for_prompt(4000);
        assert!(output.is_empty());
    }

    #[test]
    fn format_for_prompt_collections_only() {
        let ctx = WorkspaceContext {
            collections: vec!["Projects".into(), "Clients".into()],
            active_playbooks: vec![],
            relevant_schemas: vec![],
            related_schemas: vec![],
            semantic_schema_count: 0,
        };
        let output = ctx.format_for_prompt(4000);
        assert!(output.contains("COLLECTIONS:"));
        assert!(output.contains("Projects"));
        assert!(output.contains("Clients"));
        assert!(!output.contains("ACTIVE PLAYBOOKS:"));
    }

    // -----------------------------------------------------------------------
    // One-hop related-schema traversal
    // -----------------------------------------------------------------------

    #[test]
    fn related_schemas_reachable_via_outgoing_relationship() {
        // invoice -> customer (outgoing from the retrieved schema).
        let invoice = sample_schema_with_relationships(
            "invoice",
            "Invoice",
            &["amount"],
            vec![outgoing_relationship("billed_to", "customer")],
        );
        let customer = sample_schema("customer", "Customer", &["name"]);

        let related =
            related_one_hop_schemas(std::slice::from_ref(&invoice), &[invoice.clone(), customer]);

        assert_eq!(related.len(), 1);
        assert_eq!(related[0].id, "customer");
    }

    /// A user-defined schema relating to a core type (e.g. a project schema
    /// with a `has_task` relationship to `task`) is an ordinary thing to
    /// model, so the outgoing traversal can reach a core schema in the
    /// corpus. It must not surface into `related_schemas` even though it
    /// renders under a different heading than EXISTING SCHEMAS — there's
    /// no reason for a core type to appear in either block.
    #[test]
    fn related_schemas_excludes_core_types_reached_via_outgoing_relationship() {
        let project = sample_schema_with_relationships(
            "project",
            "Project",
            &["name"],
            vec![outgoing_relationship("has_task", "task")],
        );
        let mut task = sample_schema("task", "Task", &[]);
        task.is_core = true;

        let related =
            related_one_hop_schemas(std::slice::from_ref(&project), &[project.clone(), task]);

        assert!(
            related.is_empty(),
            "core type must not appear in related_schemas: {related:?}"
        );
    }

    #[test]
    fn related_schemas_reachable_via_incoming_relationship() {
        // customer is retrieved alone; it declares no outgoing relationships
        // of its own. invoice and freelance_gig each point AT customer, so
        // bidirectional traversal must still surface them.
        let customer = sample_schema("customer", "Customer", &["name"]);
        let invoice = sample_schema_with_relationships(
            "invoice",
            "Invoice",
            &["amount"],
            vec![outgoing_relationship("billed_to", "customer")],
        );
        let freelance_gig = sample_schema_with_relationships(
            "freelance_gig",
            "Freelance Gig",
            &[],
            vec![outgoing_relationship("client", "customer")],
        );

        let all = vec![customer.clone(), invoice, freelance_gig];
        let related = related_one_hop_schemas(&[customer], &all);

        let related_ids: std::collections::HashSet<&str> =
            related.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(related_ids, ["invoice", "freelance_gig"].into());
    }

    #[test]
    fn related_schemas_no_relationships_is_a_no_op() {
        let customer = sample_schema("customer", "Customer", &["name"]);
        let other = sample_schema("venue", "Venue", &["capacity"]);

        let related =
            related_one_hop_schemas(std::slice::from_ref(&customer), &[customer.clone(), other]);

        assert!(related.is_empty());
    }

    #[test]
    fn related_schemas_never_duplicate_already_retrieved() {
        // invoice -> customer, and customer is ALSO directly retrieved.
        let invoice = sample_schema_with_relationships(
            "invoice",
            "Invoice",
            &[],
            vec![outgoing_relationship("billed_to", "customer")],
        );
        let customer = sample_schema("customer", "Customer", &["name"]);

        let related =
            related_one_hop_schemas(&[invoice.clone(), customer.clone()], &[invoice, customer]);

        assert!(related.is_empty());
    }

    #[test]
    fn related_schemas_no_recursive_expansion() {
        // invoice -> customer -> region. Retrieving invoice alone must
        // surface customer (one hop) but NOT region (two hops).
        let invoice = sample_schema_with_relationships(
            "invoice",
            "Invoice",
            &[],
            vec![outgoing_relationship("billed_to", "customer")],
        );
        let customer = sample_schema_with_relationships(
            "customer",
            "Customer",
            &[],
            vec![outgoing_relationship("located_in", "region")],
        );
        let region = sample_schema("region", "Region", &[]);

        let related = related_one_hop_schemas(
            std::slice::from_ref(&invoice),
            &[invoice.clone(), customer, region],
        );

        let related_ids: Vec<&str> = related.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(related_ids, vec!["customer"]);
    }

    #[test]
    fn related_schemas_both_directions_fire_in_one_call() {
        // Two retrieved schemas, each reaching a DIFFERENT related schema via
        // a DIFFERENT direction in the same traversal call: invoice reaches
        // customer via its own outgoing relationship, while venue is a hub
        // with no outgoing relationships of its own and is reached only via
        // event's incoming relationship. Both must surface from one call —
        // this is the literal "bidirectional" acceptance criterion, not two
        // isolated single-direction fixtures.
        let invoice = sample_schema_with_relationships(
            "invoice",
            "Invoice",
            &[],
            vec![outgoing_relationship("billed_to", "customer")],
        );
        let customer = sample_schema("customer", "Customer", &["name"]);
        let venue = sample_schema("venue", "Venue", &["capacity"]);
        let event = sample_schema_with_relationships(
            "event",
            "Event",
            &[],
            vec![outgoing_relationship("held_at", "venue")],
        );

        let retrieved = vec![invoice.clone(), venue.clone()];
        let all = vec![invoice, customer, venue, event];
        let related = related_one_hop_schemas(&retrieved, &all);

        let related_ids: std::collections::HashSet<&str> =
            related.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(related_ids, ["customer", "event"].into());
    }

    #[test]
    fn related_schemas_convergent_paths_dedupe_to_one() {
        // customer is reachable via TWO different paths in the same call:
        // outgoing from invoice, AND incoming from freelance_gig (which
        // targets customer directly). It must appear exactly once.
        let invoice = sample_schema_with_relationships(
            "invoice",
            "Invoice",
            &[],
            vec![outgoing_relationship("billed_to", "customer")],
        );
        let customer = sample_schema("customer", "Customer", &["name"]);
        let freelance_gig = sample_schema_with_relationships(
            "freelance_gig",
            "Freelance Gig",
            &[],
            vec![outgoing_relationship("client", "customer")],
        );

        let retrieved = vec![invoice.clone(), customer.clone()];
        let all = vec![invoice, customer, freelance_gig];
        let related = related_one_hop_schemas(&retrieved, &all);

        // customer is directly retrieved, so it must not appear in `related`
        // at all (never-duplicate-already-retrieved) — freelance_gig is the
        // only schema that should surface, exactly once, despite invoice's
        // outgoing edge and freelance_gig's incoming edge both terminating
        // on customer.
        let related_ids: Vec<&str> = related.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(related_ids, vec!["freelance_gig"]);
    }

    #[test]
    fn format_for_prompt_renders_related_section_name_only() {
        let mut ctx = sample_context();
        ctx.relevant_schemas = vec![sample_schema("invoice", "Invoice", &["amount"])];
        ctx.related_schemas = vec![sample_schema("customer", "Customer", &["name", "email"])];

        let output = ctx.format_for_prompt(4000);

        assert!(output.contains("RELATED (via relationship, not directly matched):"));
        assert!(output.contains("- customer: Customer"));
        // Name-only: no field names in the related section.
        assert!(!output.contains("customer: Customer (name, email)"));
    }

    #[test]
    fn format_for_prompt_no_related_section_when_empty() {
        let ctx = sample_context();
        let output = ctx.format_for_prompt(4000);
        assert!(!output.contains("RELATED (via relationship"));
    }

    // -----------------------------------------------------------------------
    // Retrieval query blending
    // -----------------------------------------------------------------------
    //
    // These assert the property the recall gain rests on: the words that
    // discriminate the target schema are present in the embedded string. A
    // follow-up phrased as a pronoun or an ellipsis carries none of its own, so
    // the query is only usable if the earlier turn's words survive into it.

    #[test]
    fn retrieval_query_keeps_discriminating_words_for_pronoun_reference() {
        // "the Redwood one" names no type; "conference proposal" did.
        let prior = [
            "Add a conference proposal for Redwood Summit",
            "Added the Redwood Summit proposal.",
        ];
        let query = build_retrieval_query(&prior, "Set the Redwood one to rejected");

        assert!(query.contains("conference proposal"));
        assert!(query.contains("Set the Redwood one to rejected"));
    }

    #[test]
    fn retrieval_query_keeps_discriminating_words_for_ellipsis() {
        // "Which ones are still out?" elides its subject entirely.
        let prior = [
            "Track the invoices I send to clients",
            "Created the invoice type.",
        ];
        let query = build_retrieval_query(&prior, "Which ones are still out?");

        assert!(query.contains("invoices"));
        assert!(query.contains("Which ones are still out?"));
    }

    #[test]
    fn retrieval_query_preserves_topic_switch() {
        // A self-contained message after an unrelated exchange must still lead
        // with its own words — this is the no-regression case for blending.
        let prior = ["Add an invoice for $500", "Invoice recorded."];
        let query = build_retrieval_query(&prior, "Create a venue named The Fillmore");

        assert!(query.contains("Create a venue named The Fillmore"));
        assert!(query.ends_with("Create a venue named The Fillmore"));
    }

    #[test]
    fn retrieval_query_blends_turns_oldest_first() {
        let prior = ["first turn", "second turn"];
        let query = build_retrieval_query(&prior, "current message");
        assert_eq!(query, "first turn\nsecond turn\ncurrent message");
    }

    #[test]
    fn retrieval_query_uses_only_the_last_two_turns() {
        let prior = ["oldest turn", "middle turn", "newest turn"];
        let query = build_retrieval_query(&prior, "current message");

        assert!(!query.contains("oldest turn"));
        assert_eq!(query, "middle turn\nnewest turn\ncurrent message");
    }

    #[test]
    fn retrieval_query_without_history_is_the_message_alone() {
        // First turn of a conversation must be byte-identical to the old
        // behaviour, so single-turn retrieval is unaffected.
        assert_eq!(
            build_retrieval_query(&[], "Add an invoice"),
            "Add an invoice"
        );
    }

    #[test]
    fn retrieval_query_skips_blank_turns() {
        let prior = ["real turn", "   ", ""];
        let query = build_retrieval_query(&prior, "current message");
        assert_eq!(query, "real turn\ncurrent message");
    }

    #[test]
    fn retrieval_query_caps_each_prior_turn_but_never_the_current_message() {
        let long_turn = "x".repeat(5_000);
        let long_current = "y".repeat(5_000);
        let query = build_retrieval_query(&[&long_turn], &long_current);

        let (prior, current) = query.split_once('\n').expect("prior turn and current");
        assert_eq!(
            prior.chars().count(),
            MAX_CHARS_PER_BLENDED_TURN,
            "a prior turn is capped"
        );
        assert_eq!(
            current.chars().count(),
            5_000,
            "the current message is never truncated"
        );
    }

    #[test]
    fn retrieval_query_keeps_the_end_of_a_long_prior_turn() {
        // The referent a follow-up points back to sits nearest the end.
        let long_turn = format!("{} the Redwood Summit proposal", "filler ".repeat(200));
        let query = build_retrieval_query(&[&long_turn], "Set that one to rejected");

        assert!(query.contains("the Redwood Summit proposal"));
        assert!(query.ends_with("Set that one to rejected"));
    }

    #[test]
    fn retrieval_query_truncation_respects_char_boundaries() {
        // Slicing a multi-byte string on a byte index would panic.
        let long_turn = "é".repeat(1_000);
        let query = build_retrieval_query(&[&long_turn], "follow up");

        let prior = query.split('\n').next().expect("prior turn");
        assert_eq!(prior.chars().count(), MAX_CHARS_PER_BLENDED_TURN);
        assert!(prior.chars().all(|c| c == 'é'));
    }

    #[test]
    fn retrieval_query_leaves_short_turns_untouched() {
        let short = "Add a conference proposal";
        assert!(short.chars().count() < MAX_CHARS_PER_BLENDED_TURN);
        assert_eq!(
            build_retrieval_query(&[short], "Set it to rejected"),
            "Add a conference proposal\nSet it to rejected"
        );
    }

    #[test]
    fn retrieval_query_trims_and_tolerates_empty_current_message() {
        assert_eq!(
            build_retrieval_query(&["  prior turn  "], "  current  "),
            "prior turn\ncurrent"
        );
        assert_eq!(build_retrieval_query(&["prior turn"], "   "), "prior turn");
        assert_eq!(build_retrieval_query(&[], "   "), "");
    }
}
