//! Search Operations
//!
//! Semantic search orchestration extracted from MCP handlers.
//! Handles collection resolution, scope filtering, lifecycle filtering,
//! over-fetching, and optional markdown inlining.

use crate::models::{Node, NodeFilter, NodeQuery};
use crate::ops::OpsError;
use crate::services::{
    CollectionService, NodeEmbeddingService, NodeService, NodeServiceError, SearchNodeFilters,
    SearchScope,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Safety cap on how many nodes are pulled from the DB for an "enumerate"
/// query (see [`normalize_enumerate_query`]) before service-layer filters
/// (scope/archived/collection/node_types/property_filters) and `limit` are
/// applied — mirrors `semantic_search_nodes`'s over-fetch behavior, but an
/// enumerate has no similarity ranking to over-fetch against, so this is a
/// flat cap instead of a multiple of `limit`.
const ENUMERATE_FETCH_CAP: usize = 1000;

/// Recognize the query spellings that mean "list everything (of this type)"
/// rather than a literal search term, so `search_nodes` (agent path) and
/// `search_nodes`/`search_semantic` (CLI/gRPC path) agree on what counts as
/// an enumerate request. Returns `None` for an enumerate query (empty,
/// whitespace-only, or the conventional wildcard `"*"`); returns
/// `Some(trimmed)` for anything else, to be used as a literal search term.
pub fn normalize_enumerate_query(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed == "*" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Whether the default `Knowledge` scope's allowlist (text/header/code-block/
/// schema/table) should be bypassed for an explicit `node_types` filter.
///
/// An explicit `node_types` filter is a more specific statement of intent
/// than the default scope guess: without this, a search scoped to
/// `node_types: ["invoice"]` would return zero results not because no
/// invoices exist, but because "invoice" isn't a knowledge-scope type — and
/// no caller of `search_semantic` can express `--scope everything` today (no
/// CLI/RPC surface for it — see `packages/cli/src/commands/search.rs`).
///
/// Scoped to `is_enumerate` only: an ordinary semantic query that also sets
/// `node_types` (e.g. "overdue payments" + node_types: ["invoice"]) must
/// keep the default scope filter — bypassing it for every `node_types`-
/// bearing call would silently disable scope filtering for real semantic
/// searches too, not just the enumerate case this exists for. An
/// explicitly-requested non-default `scope` always applies as normal
/// (`scope_is_default` is `false` in that case).
fn should_skip_scope_filter(
    is_enumerate: bool,
    has_node_types: bool,
    scope_is_default: bool,
) -> bool {
    is_enumerate && has_node_types && scope_is_default
}

/// Maximum depth for markdown tree traversal
const MARKDOWN_MAX_DEPTH: usize = 20;

/// Hard cap on how many top-ranked results `include_markdown` will attach
/// full subtree markdown to, regardless of what a caller requests — the
/// single source of truth every layer that also bounds this value (the
/// `SearchRequest.include_markdown` clamp in `packages/daemon`, the CLI's
/// `--include-content` flag) should read from or stay in sync with by hand
/// if it can't depend on this crate directly.
pub const MAX_INCLUDE_MARKDOWN_RESULTS: usize = 5;

// ============================================================================
// Input / Output types
// ============================================================================

#[derive(Debug)]
pub struct SearchSemanticInput {
    pub query: String,
    pub threshold: Option<f32>,
    pub limit: Option<usize>,
    pub collection_id: Option<String>,
    pub collection: Option<String>,
    pub exclude_collections: Option<Vec<String>>,
    pub include_markdown: Option<usize>,
    pub include_archived: Option<bool>,
    pub scope: Option<String>,
    /// Filter by specific node types (e.g., ["task", "text"])
    pub node_types: Option<Vec<String>>,
    /// Filter by node properties (key-value pairs, AND logic)
    pub property_filters: Option<serde_json::Value>,

    /// When true, attach outgoing relationships of each result node as an "edges" array.
    /// Each edge entry has: {"relationship": "...", "target_id": "...", "target_title": "..."}
    /// Default: false (no edge data included)
    pub include_edges: Option<bool>,

    /// When true, re-rank results by blending vector similarity with graph connectivity degree.
    /// Blending formula: combined_score = 0.7 * similarity + 0.3 * normalized_degree
    /// where normalized_degree = (out_degree + in_degree) / max_degree_in_result_set
    /// Surfaces well-connected, central knowledge nodes over isolated but textually similar ones.
    /// Default: false (pure similarity ranking)
    pub graph_boost: Option<bool>,

    /// When true, also run a title/content keyword search and merge its hits
    /// into the ranked result set ahead of the embedding hits.
    ///
    /// Embeddings only cover the embeddable types (text/header/code-block/
    /// schema/table — ADR-029), so a `task` named "Draft Q3 architecture
    /// review" has no vector to match and is unreachable by meaning-based
    /// search at any threshold. A human typing a title into a search box
    /// expects to find it regardless, which is what this flag is for; the
    /// agent/CLI meaning-based paths leave it off. See [`title_match_nodes`]
    /// for the matching and ranking rules.
    /// Default: false.
    pub include_title_matches: Option<bool>,
}

#[derive(Debug)]
pub struct SearchSemanticOutput {
    pub nodes: Vec<Value>,
    /// The same results as `nodes`, in the same order, as typed [`Node`]s.
    ///
    /// Search already fetched every one of these to rank and filter it, so a
    /// consumer that needs node structs (the gRPC handler, which maps them
    /// straight onto the wire type) can take them from here. Re-reading them
    /// from the store by id costs one query per result and returns exactly the
    /// rows search is already holding.
    pub matched_nodes: Vec<Node>,
    pub count: usize,
    pub query: String,
    pub threshold: f32,
    pub collection_id: Option<String>,
    pub include_markdown: usize,
    pub include_archived: bool,
    pub scope: String,
}

// ============================================================================
// Helpers
// ============================================================================

/// Recursively build markdown from a node tree
fn build_markdown_recursive(
    node: &Node,
    node_map: &HashMap<String, Node>,
    adjacency_list: &HashMap<String, Vec<String>>,
    output: &mut String,
    depth: usize,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }

    match node.node_type.as_str() {
        "header" => {
            output.push_str(&node.content);
            output.push_str("\n\n");
        }
        "text" => {
            output.push_str(&node.content);
            output.push_str("\n\n");
        }
        "task" => {
            let status = node
                .properties
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("todo");
            let checkbox = if status == "done" { "[x]" } else { "[ ]" };
            output.push_str(&format!("- {} {}\n", checkbox, node.content));
        }
        "code-block" => {
            let language = node
                .properties
                .get("language")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            output.push_str(&format!("```{}\n{}\n```\n\n", language, node.content));
        }
        "quote-block" => {
            for line in node.content.lines() {
                output.push_str(&format!("> {}\n", line));
            }
            output.push('\n');
        }
        "ordered-list" => {
            output.push_str(&format!("1. {}\n", node.content));
        }
        _ => {
            output.push_str(&node.content);
            output.push_str("\n\n");
        }
    }

    if let Some(child_ids) = adjacency_list.get(&node.id) {
        for child_id in child_ids {
            if let Some(child) = node_map.get(child_id) {
                build_markdown_recursive(
                    child,
                    node_map,
                    adjacency_list,
                    output,
                    depth + 1,
                    max_depth,
                );
            }
        }
    }
}

fn parse_scope(scope: Option<&str>) -> Result<SearchScope, OpsError> {
    match scope {
        Some("conversations") => Ok(SearchScope::Conversations),
        Some("everything") => Ok(SearchScope::Everything),
        Some("knowledge") | None => Ok(SearchScope::Knowledge),
        Some(unknown) => Err(OpsError::InvalidParams(format!(
            "Invalid scope '{}'. Valid values: knowledge, conversations, everything",
            unknown
        ))),
    }
}

/// Structural "list everything (of this type)" retrieval for an enumerate
/// query (see [`normalize_enumerate_query`]) — a DB listing rather than an
/// embedding-similarity search, since there is no query text to embed.
///
/// Returns `(Node, similarity)` pairs with a synthetic `similarity` of `1.0`
/// so the result shape matches `semantic_search_nodes` and can flow through
/// the same downstream scope/collection/archived filtering, graph_boost, and
/// output building in [`search_semantic`].
///
/// Every entry in `filters.node_types` is pushed down to the DB query as a
/// separate `with_node_type` fetch (each independently capped and merged) —
/// not fetched unfiltered up to a shared cap and post-filtered by type. A
/// shared-cap-then-post-filter approach silently undercounts once the total
/// node count (across *all* types) exceeds the cap: the very "silent wrong
/// count" failure class this function exists to eliminate, just relocated to
/// a multi-type caller instead of a `"*"` query. `property_filters`, which
/// has no DB-level pushdown here, is still applied as a post-filter via
/// [`SearchNodeFilters::matches`], matching how `semantic_search_nodes`
/// applies the same filters.
async fn enumerate_nodes(
    node_service: &Arc<NodeService>,
    limit: usize,
    filters: Option<&SearchNodeFilters>,
) -> Result<Vec<(Node, f64)>, OpsError> {
    let per_query_limit = ENUMERATE_FETCH_CAP.min(limit.max(1) * 3);
    let node_types = filters.and_then(|f| f.node_types.as_ref());

    let nodes = match node_types {
        Some(types) if !types.is_empty() => {
            let mut merged = Vec::new();
            for node_type in types {
                let node_filter = NodeFilter::new()
                    .with_node_type(node_type.clone())
                    .with_limit(per_query_limit);
                let mut matched = node_service
                    .query_nodes(node_filter)
                    .await
                    .map_err(|e| OpsError::Internal(format!("Failed to enumerate nodes: {}", e)))?;
                merged.append(&mut matched);
            }
            merged
        }
        _ => {
            let node_filter = NodeFilter::new().with_limit(per_query_limit);
            node_service
                .query_nodes(node_filter)
                .await
                .map_err(|e| OpsError::Internal(format!("Failed to enumerate nodes: {}", e)))?
        }
    };

    Ok(nodes
        .into_iter()
        .filter(|node| {
            filters
                .map(|f| f.matches(&node.node_type, &node.properties))
                .unwrap_or(true)
        })
        .take(limit)
        .map(|node| (node, 1.0))
        .collect())
}

/// Score a keyword hit by how completely the query accounts for the text it
/// matched, so an exact title outranks a title that merely contains the query.
///
/// Returns a value in `(0.0, 1.0]`, or `None` when `text` does not contain
/// `query_lower` at all. The tiers are deliberately coarse — exact match,
/// prefix match, then containment — because the underlying store match is a
/// plain `LIKE` with no relevance signal of its own to refine. Within the
/// containment tier, shorter matched text scores higher: a query that is most
/// of a short title is a better hit than the same query buried in a long one.
fn keyword_match_score(text: &str, query_lower: &str) -> Option<f64> {
    let text_lower = text.trim().to_lowercase();
    if text_lower.is_empty() || !text_lower.contains(query_lower) {
        return None;
    }
    if text_lower == query_lower {
        return Some(1.0);
    }
    // Ratio of query length to matched-text length, so a near-complete match
    // lands just under an exact one and a small fragment of a long text lands
    // near the floor. Both tiers are compressed into bands above the semantic
    // range so keyword hits sort ahead of vector hits without any single band
    // reaching the 1.0 reserved for an exact match.
    let coverage = query_lower.chars().count() as f64 / text_lower.chars().count().max(1) as f64;
    if text_lower.starts_with(query_lower) {
        Some(0.9 + 0.09 * coverage)
    } else {
        Some(0.8 + 0.09 * coverage)
    }
}

/// Keyword (title/content substring) retrieval for human-facing search.
///
/// Runs two store queries rather than one because `NodeQuery`'s
/// `title_contains` and `content_contains` are AND-chained into a single
/// `WHERE` (see `build_scalar_conditions` in `sqlite_store/nodes.rs`) — what
/// search needs is their union: a node whose *title* matches, or whose
/// *content* does. Setting both on one query would instead demand the term
/// appear in both fields, which almost never holds for a node whose title is
/// derived from its content.
///
/// The title query is issued first and its hits kept on a tie, because
/// `query_nodes` applies a title-stem fallback (a "groceries" query resolving
/// a "grocery store" title) that has no content-side equivalent. Results are
/// scored by [`keyword_match_score`] against the better of the node's title
/// and content, deduplicated by id, and returned highest-scoring first.
///
/// `node_types` from `filters` is pushed down to the store per type, matching
/// [`enumerate_nodes`]; `property_filters` has no pushdown and is applied as a
/// post-filter via [`SearchNodeFilters::matches`].
async fn title_match_nodes(
    node_service: &Arc<NodeService>,
    query: &str,
    limit: usize,
    filters: Option<&SearchNodeFilters>,
) -> Result<Vec<(Node, f64)>, OpsError> {
    let query_lower = query.trim().to_lowercase();
    if query_lower.is_empty() {
        return Ok(Vec::new());
    }

    let per_query_limit = ENUMERATE_FETCH_CAP.min(limit.max(1) * 3);
    let node_types = filters
        .and_then(|f| f.node_types.as_ref())
        .filter(|types| !types.is_empty());

    // (query-builder, applies-to) pairs: title first so its stem-fallback hits
    // win ties against a plain content substring match for the same node.
    let mut candidates: Vec<Node> = Vec::new();
    for by_title in [true, false] {
        let base = || {
            let mut q = NodeQuery::new();
            if by_title {
                q.title_contains = Some(query.trim().to_string());
            } else {
                q.content_contains = Some(query.trim().to_string());
            }
            q.limit = Some(per_query_limit);
            q
        };

        match node_types {
            Some(types) => {
                for node_type in types {
                    let mut q = base();
                    q.node_type = Some(node_type.clone());
                    let mut matched = node_service.query_nodes_simple(q).await.map_err(|e| {
                        OpsError::Internal(format!("Failed to run title search: {}", e))
                    })?;
                    candidates.append(&mut matched);
                }
            }
            None => {
                let mut matched = node_service.query_nodes_simple(base()).await.map_err(|e| {
                    OpsError::Internal(format!("Failed to run title search: {}", e))
                })?;
                candidates.append(&mut matched);
            }
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut scored: Vec<(Node, f64)> = Vec::new();
    for node in candidates {
        if !seen.insert(node.id.clone()) {
            continue;
        }
        if let Some(f) = filters {
            if !f.matches(&node.node_type, &node.properties) {
                continue;
            }
        }
        // Score against whichever field matched better. A node surfaced by the
        // title-stem fallback matches neither field as a literal substring, so
        // it keeps a floor score rather than being dropped — the store already
        // judged it a title match, and re-deciding that here would discard the
        // fallback's whole purpose.
        let title_score = node
            .title
            .as_deref()
            .and_then(|t| keyword_match_score(t, &query_lower));
        let content_score = keyword_match_score(&node.content, &query_lower);
        let score = title_score
            .into_iter()
            .chain(content_score)
            .fold(f64::NAN, f64::max);
        let score = if score.is_nan() { 0.8 } else { score };
        scored.push((node, score));
    }

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    Ok(scored)
}

// ============================================================================
// Operation
// ============================================================================

/// Semantic search with collection resolution, scope/lifecycle filtering,
/// over-fetching, optional markdown inlining, edge attachment, and graph boost re-ranking.
pub async fn search_semantic(
    node_service: &Arc<NodeService>,
    embedding_service: &Arc<NodeEmbeddingService>,
    input: SearchSemanticInput,
) -> Result<SearchSemanticOutput, OpsError> {
    // Apply defaults
    let threshold = input.threshold.unwrap_or(0.7);
    let limit = input.limit.unwrap_or(20);
    let include_markdown = input
        .include_markdown
        .unwrap_or(1)
        .min(MAX_INCLUDE_MARKDOWN_RESULTS);
    let include_archived = input.include_archived.unwrap_or(false);
    let include_edges = input.include_edges.unwrap_or(false);
    let graph_boost = input.graph_boost.unwrap_or(false);
    let scope = parse_scope(input.scope.as_deref())?;

    // Validate
    if !(0.0..=1.0).contains(&threshold) {
        return Err(OpsError::InvalidParams(
            "threshold must be between 0.0 and 1.0".to_string(),
        ));
    }
    if limit > 1000 {
        return Err(OpsError::InvalidParams(
            "limit cannot exceed 1000".to_string(),
        ));
    }
    // An empty/whitespace query or the conventional wildcard "*" means
    // "enumerate" rather than "search for this literal term" — same rule the
    // agent-tool `search_nodes` path applies (see `normalize_enumerate_query`).
    // Embedding a punctuation-only or empty string has no meaningful
    // similarity signal, so this bypasses `semantic_search_nodes` entirely in
    // favor of a structural DB listing (`enumerate_nodes` below), which
    // returns a synthetic similarity of 1.0 and otherwise flows through the
    // same scope/collection/archived post-filters and output building as a
    // real semantic search.
    let is_enumerate = normalize_enumerate_query(&input.query).is_none();

    // Resolve collection include filter
    let (collection_id, collection_member_ids): (Option<String>, Option<HashSet<String>>) =
        if let Some(path) = &input.collection {
            let collection_service =
                CollectionService::new(embedding_service.store(), node_service);
            match collection_service.resolve_path(path).await {
                Ok(resolved) => {
                    let coll_id = resolved.leaf_id().to_string();
                    let members = collection_service
                        .get_collection_members(&coll_id)
                        .await
                        .map_err(|e| {
                            OpsError::Internal(format!("Failed to get collection members: {}", e))
                        })?;
                    (
                        Some(coll_id),
                        Some(members.into_iter().map(|n| n.id).collect()),
                    )
                }
                Err(NodeServiceError::CollectionNotFound(_)) => {
                    return Ok(SearchSemanticOutput {
                        nodes: vec![],
                        matched_nodes: vec![],
                        count: 0,
                        query: input.query,
                        threshold,
                        collection_id: None,
                        include_markdown,
                        include_archived,
                        scope: format!("{:?}", scope),
                    });
                }
                Err(e) => {
                    return Err(OpsError::Internal(format!(
                        "Failed to resolve collection path: {}",
                        e
                    )));
                }
            }
        } else if let Some(coll_id) = &input.collection_id {
            let collection_service =
                CollectionService::new(embedding_service.store(), node_service);
            let members = collection_service
                .get_collection_members(coll_id)
                .await
                .map_err(|e| {
                    OpsError::Internal(format!("Failed to get collection members: {}", e))
                })?;
            (
                Some(coll_id.clone()),
                Some(members.into_iter().map(|n| n.id).collect()),
            )
        } else {
            (None, None)
        };

    // Resolve excluded collections
    let excluded_node_ids: HashSet<String> = if let Some(exclude_paths) = &input.exclude_collections
    {
        let collection_service = CollectionService::new(embedding_service.store(), node_service);
        let mut excluded = HashSet::new();
        for path in exclude_paths {
            match collection_service.resolve_path(path).await {
                Ok(resolved) => {
                    let coll_id = resolved.leaf_id().to_string();
                    if let Ok(members) = collection_service.get_collection_members(&coll_id).await {
                        excluded.extend(members.into_iter().map(|n| n.id));
                    }
                }
                Err(NodeServiceError::CollectionNotFound(_)) => {
                    tracing::debug!("Excluded collection '{}' not found, skipping", path);
                }
                Err(e) => {
                    tracing::warn!("Failed to resolve excluded collection '{}': {}", path, e);
                }
            }
        }
        excluded
    } else {
        HashSet::new()
    };

    tracing::info!(
        "Semantic search for: '{}' (scope: {:?})",
        input.query,
        scope
    );

    // Build service-layer filters for node_types and property_filters.
    // node_types/property_filters over-fetching is handled inside semantic_search_nodes.
    let search_filters = if input.node_types.is_some() || input.property_filters.is_some() {
        Some(SearchNodeFilters {
            node_types: input.node_types.clone(),
            property_filters: input.property_filters.clone(),
        })
    } else {
        None
    };

    // Over-fetch when post-filtering is needed
    let scope_filters = !matches!(scope, SearchScope::Everything);
    let has_post_filters = collection_member_ids.is_some()
        || !excluded_node_ids.is_empty()
        || !include_archived
        || scope_filters;
    let effective_limit = if has_post_filters { limit * 3 } else { limit };

    let include_title_matches = input.include_title_matches.unwrap_or(false);

    let results = if is_enumerate {
        enumerate_nodes(node_service, effective_limit, search_filters.as_ref()).await?
    } else {
        let semantic = embedding_service
            .semantic_search_nodes(
                &input.query,
                effective_limit,
                threshold,
                search_filters.as_ref(),
            )
            .await
            .map_err(|e| {
                let err_msg = e.to_string();
                if err_msg.contains("not initialized") || err_msg.contains("not available") {
                    OpsError::Internal("Embedding service not ready".to_string())
                } else if err_msg.contains("no embeddings") || err_msg.contains("not found") {
                    OpsError::InvalidParams(
                        "No content available for semantic search. Try adding content first."
                            .to_string(),
                    )
                } else if err_msg.contains("database") || err_msg.contains("Database") {
                    OpsError::Internal(format!("Database error during search: {}", e))
                } else {
                    OpsError::Internal(format!("Search failed: {}", e))
                }
            });

        // With keyword search in play, an unusable embedding index degrades the
        // search instead of failing it: the keyword half is independent of
        // embeddings and is the half that answers a title query. Without the
        // flag there is no second half to fall back on, so the error stands.
        let semantic = match semantic {
            Ok(results) => results,
            Err(e) if include_title_matches => {
                tracing::warn!(
                    error = %e,
                    "semantic search unavailable; serving keyword matches only"
                );
                Vec::new()
            }
            Err(e) => return Err(e),
        };

        if include_title_matches {
            // Keyword hits first, then semantic hits for nodes the keyword pass
            // did not already surface. Both are scored on the same 0..1 scale,
            // with keyword scores banded above the similarity range
            // (`keyword_match_score`), so the concatenation is already in
            // descending score order for the common case and downstream
            // `graph_boost` re-ranking (which sorts explicitly) still composes.
            let keyword = title_match_nodes(
                node_service,
                &input.query,
                effective_limit,
                search_filters.as_ref(),
            )
            .await?;

            let keyword_ids: HashSet<String> =
                keyword.iter().map(|(node, _)| node.id.clone()).collect();
            keyword
                .into_iter()
                .chain(
                    semantic
                        .into_iter()
                        .filter(|(node, _)| !keyword_ids.contains(&node.id)),
                )
                .collect()
        } else {
            semantic
        }
    };

    let skip_scope_filter = should_skip_scope_filter(
        is_enumerate,
        input.node_types.is_some(),
        input.scope.is_none(),
    );

    // Apply filters
    let filtered_results: Vec<_> = results
        .into_iter()
        .filter(|(node, _)| {
            if !skip_scope_filter && !NodeEmbeddingService::matches_scope(&node.node_type, &scope) {
                return false;
            }
            if node.lifecycle_status == "archived" && !include_archived {
                return false;
            }
            if let Some(ref member_ids) = collection_member_ids {
                if !member_ids.contains(&node.id) {
                    return false;
                }
            }
            if excluded_node_ids.contains(&node.id) {
                return false;
            }
            true
        })
        .take(limit)
        .collect();

    // Pre-fetch outgoing "mentions" edges when needed by graph_boost or include_edges.
    // Cache them to avoid redundant DB round-trips when both features are enabled.
    let mut outgoing_edges_cache: HashMap<String, Vec<Node>> = HashMap::new();
    if graph_boost || include_edges {
        for (node, _) in &filtered_results {
            let related = node_service
                .get_related_nodes(&node.id, "mentions", "out")
                .await
                .unwrap_or_default();
            outgoing_edges_cache.insert(node.id.clone(), related);
        }
    }

    // graph_boost: re-rank by blending similarity with normalized edge degree.
    //
    // Formula: combined_score = 0.7 * similarity + 0.3 * normalized_degree
    //
    // Degree is (out_degree + in_degree) counted over "mentions" relationships only.
    // Degrees are normalized to [0, 1] within the result set so that the boost is
    // relative — a node only benefits when it is more connected than its peers.
    let filtered_results: Vec<(Node, f64)> = if graph_boost && !filtered_results.is_empty() {
        let mut degrees: HashMap<String, usize> = HashMap::new();
        for (node, _) in &filtered_results {
            let out_count = outgoing_edges_cache
                .get(&node.id)
                .map(|v| v.len())
                .unwrap_or(0);
            let in_count = node_service
                .get_related_nodes(&node.id, "mentions", "in")
                .await
                .map(|v| v.len())
                .unwrap_or(0);
            degrees.insert(node.id.clone(), out_count + in_count);
        }

        let max_degree = degrees.values().copied().max().unwrap_or(1).max(1);

        let mut boosted: Vec<(Node, f64)> = filtered_results
            .into_iter()
            .map(|(node, similarity)| {
                let degree = degrees.get(&node.id).copied().unwrap_or(0);
                let normalized_degree = degree as f64 / max_degree as f64;
                let combined = 0.7 * similarity + 0.3 * normalized_degree;
                (node, combined)
            })
            .collect();

        boosted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        boosted.truncate(limit);
        boosted
    } else {
        filtered_results
    };

    // include_edges: build edge data from cached outgoing relationships.
    let mut edges_map: HashMap<String, Vec<Value>> = HashMap::new();
    if include_edges {
        for (node, _) in &filtered_results {
            let related = outgoing_edges_cache.remove(&node.id).unwrap_or_default();

            let edges: Vec<Value> = related
                .into_iter()
                .map(|target| {
                    json!({
                        "relationship": "mentions",
                        "target_id": target.id,
                        "target_title": target.title
                    })
                })
                .collect();

            edges_map.insert(node.id.clone(), edges);
        }
    }

    // Fetch markdown for top N results
    let mut markdown_contents: HashMap<String, String> = HashMap::new();
    if include_markdown > 0 {
        for (node, _) in filtered_results.iter().take(include_markdown) {
            match node_service.get_subtree_data(&node.id).await {
                Ok((Some(root_node), node_map, adjacency_list)) => {
                    let mut markdown = String::new();
                    markdown.push_str(&root_node.content);
                    markdown.push_str("\n\n");

                    if let Some(child_ids) = adjacency_list.get(&root_node.id) {
                        for child_id in child_ids {
                            if let Some(child) = node_map.get(child_id) {
                                build_markdown_recursive(
                                    child,
                                    &node_map,
                                    &adjacency_list,
                                    &mut markdown,
                                    0,
                                    MARKDOWN_MAX_DEPTH,
                                );
                            }
                        }
                    }

                    markdown_contents.insert(node.id.clone(), markdown.trim().to_string());
                }
                // Both arms leave this result out of `markdown_contents`, so the
                // caller gets no `markdown` field for it — indistinguishable from
                // "nothing to attach" without this log. Worth knowing about: the
                // caller asked for this result's content and silently didn't get
                // it, rather than the id/depth genuinely having no extra content.
                Ok((None, _, _)) => {
                    tracing::warn!(
                        node_id = %node.id,
                        "include_markdown: subtree fetch returned no root node"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        node_id = %node.id,
                        error = %e,
                        "include_markdown: failed to fetch subtree for markdown attachment"
                    );
                }
            }
        }
    }

    // Build output
    let nodes: Vec<Value> = filtered_results
        .iter()
        .enumerate()
        .map(|(idx, (node, similarity))| {
            let mut node_json = json!({
                "id": node.id,
                "nodeType": node.node_type,
                "content": node.content,
                "title": node.title,
                "version": node.version,
                "createdAt": node.created_at,
                "modifiedAt": node.modified_at,
                "properties": node.properties,
                "similarity": similarity
            });

            if idx < include_markdown {
                if let Some(markdown) = markdown_contents.get(&node.id) {
                    node_json["markdown"] = json!(markdown);
                }
            }

            if include_edges {
                let edges = edges_map.get(&node.id).cloned().unwrap_or_default();
                node_json["edges"] = json!(edges);
            }

            node_json
        })
        .collect();

    let count = nodes.len();
    let matched_nodes: Vec<Node> = filtered_results.into_iter().map(|(node, _)| node).collect();

    Ok(SearchSemanticOutput {
        nodes,
        matched_nodes,
        count,
        query: input.query,
        threshold,
        collection_id,
        include_markdown,
        include_archived,
        scope: format!("{:?}", scope),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scope_valid() {
        assert!(matches!(parse_scope(None), Ok(SearchScope::Knowledge)));
        assert!(matches!(
            parse_scope(Some("knowledge")),
            Ok(SearchScope::Knowledge)
        ));
        assert!(matches!(
            parse_scope(Some("conversations")),
            Ok(SearchScope::Conversations)
        ));
        assert!(matches!(
            parse_scope(Some("everything")),
            Ok(SearchScope::Everything)
        ));
    }

    #[test]
    fn test_parse_scope_invalid() {
        let err = parse_scope(Some("bogus")).unwrap_err();
        assert!(matches!(err, OpsError::InvalidParams(_)));
    }

    #[test]
    fn test_normalize_enumerate_query_recognizes_enumerate_spellings() {
        assert_eq!(normalize_enumerate_query(""), None);
        assert_eq!(normalize_enumerate_query("   "), None);
        assert_eq!(normalize_enumerate_query("\t\n"), None);
        assert_eq!(normalize_enumerate_query("*"), None);
        assert_eq!(normalize_enumerate_query("  *  "), None);
    }

    #[test]
    fn test_normalize_enumerate_query_passes_through_literal_terms() {
        assert_eq!(
            normalize_enumerate_query("invoice"),
            Some("invoice".to_string())
        );
        assert_eq!(
            normalize_enumerate_query("  invoice  "),
            Some("invoice".to_string())
        );
        // "*" only means enumerate as the whole (trimmed) query — a wildcard
        // embedded in a longer literal term is not special-cased.
        assert_eq!(
            normalize_enumerate_query("invoice*"),
            Some("invoice*".to_string())
        );
    }

    #[test]
    fn test_should_skip_scope_filter_only_for_enumerate_with_explicit_node_types_and_default_scope()
    {
        // The one case that should skip: enumerate + explicit node_types + no
        // explicit scope requested.
        assert!(should_skip_scope_filter(true, true, true));

        // A literal/semantic query must never skip scope filtering, even with
        // node_types set — this is the bug the review caught: widening
        // `skip_scope_filter` to every `node_types`-bearing call would
        // silently disable scope filtering for real semantic searches.
        assert!(!should_skip_scope_filter(false, true, true));

        // No explicit node_types → nothing to be more specific than the
        // scope guess, so never skip.
        assert!(!should_skip_scope_filter(true, false, true));
        assert!(!should_skip_scope_filter(false, false, true));

        // An explicitly-requested non-default scope always applies as
        // normal, regardless of enumerate/node_types.
        assert!(!should_skip_scope_filter(true, true, false));
        assert!(!should_skip_scope_filter(false, true, false));
    }

    /// Verify that graph_boost re-ranking formula correctly promotes well-connected nodes.
    ///
    /// Setup: Node A has similarity=0.9 and degree=0, Node B has similarity=0.6 and degree=5.
    /// Without boost A ranks first. With boost:
    ///   A: 0.7 * 0.9 + 0.3 * 0.0 = 0.63
    ///   B: 0.7 * 0.6 + 0.3 * 1.0 = 0.72  (normalized_degree = 5/5 = 1.0)
    /// So B should rank first after boosting.
    #[test]
    fn test_graph_boost_reranking_formula() {
        let high_similarity: f64 = 0.9;
        let low_similarity: f64 = 0.6;

        let max_degree: usize = 5;
        let degree_a: usize = 0;
        let degree_b: usize = 5;

        let norm_a = degree_a as f64 / max_degree as f64;
        let norm_b = degree_b as f64 / max_degree as f64;

        let combined_a = 0.7 * high_similarity + 0.3 * norm_a;
        let combined_b = 0.7 * low_similarity + 0.3 * norm_b;

        assert!(
            combined_b > combined_a,
            "graph_boost should promote well-connected node B ({combined_b}) above high-similarity node A ({combined_a})"
        );
    }

    #[test]
    fn test_include_edges_defaults_false() {
        let input = SearchSemanticInput {
            query: "test".to_string(),
            threshold: None,
            limit: None,
            collection_id: None,
            collection: None,
            exclude_collections: None,
            include_markdown: None,
            include_archived: None,
            scope: None,
            node_types: None,
            property_filters: None,
            include_edges: None,
            graph_boost: None,
            include_title_matches: None,
        };
        assert!(!input.include_edges.unwrap_or(false));
    }

    #[test]
    fn keyword_score_ranks_exact_above_prefix_above_containment() {
        let exact = keyword_match_score("Migration plan", "migration plan").unwrap();
        let prefix = keyword_match_score("Migration plan draft", "migration plan").unwrap();
        let contained =
            keyword_match_score("Revisit the migration plan later", "migration plan").unwrap();

        assert_eq!(exact, 1.0, "an exact title match is the strongest hit");
        assert!(
            exact > prefix && prefix > contained,
            "expected exact ({exact}) > prefix ({prefix}) > contained ({contained})"
        );
    }

    #[test]
    fn keyword_score_is_case_insensitive_and_ignores_surrounding_space() {
        assert_eq!(
            keyword_match_score("  MIGRATION PLAN  ", "migration plan"),
            Some(1.0)
        );
    }

    #[test]
    fn keyword_score_rejects_text_without_the_query() {
        assert_eq!(
            keyword_match_score("Vendor security questionnaire", "migration plan"),
            None
        );
        assert_eq!(keyword_match_score("", "migration plan"), None);
    }

    /// Every keyword tier has to outrank any similarity a vector hit can carry,
    /// or a merged list would interleave a weak semantic match above the exact
    /// title the person typed. Similarity is a cosine score in 0..=1, and the
    /// weakest keyword tier (a one-character query buried in a long text) still
    /// has to clear the strongest plausible similarity.
    #[test]
    fn keyword_scores_outrank_the_semantic_similarity_range() {
        let weakest_containment = keyword_match_score(&"x".repeat(1000), "x").unwrap();

        assert!(
            weakest_containment > 0.8,
            "weakest keyword hit ({weakest_containment}) must stay above the semantic band"
        );
        assert!(
            weakest_containment <= 1.0,
            "no keyword hit may exceed the 1.0 reserved for an exact match"
        );
    }

    /// A longer title dilutes the same query: the query accounts for less of
    /// what it matched, so it is the weaker hit of the two.
    #[test]
    fn keyword_score_prefers_the_shorter_matched_text() {
        let tight = keyword_match_score("migration plan v2", "migration plan").unwrap();
        let diluted = keyword_match_score(
            "migration plan plus every other open workstream we tracked",
            "migration plan",
        )
        .unwrap();

        assert!(
            tight > diluted,
            "expected the tighter match ({tight}) to outrank the diluted one ({diluted})"
        );
    }

    #[test]
    fn test_include_title_matches_defaults_false() {
        let input = SearchSemanticInput {
            query: "test".to_string(),
            threshold: None,
            limit: None,
            collection_id: None,
            collection: None,
            exclude_collections: None,
            include_markdown: None,
            include_archived: None,
            scope: None,
            node_types: None,
            property_filters: None,
            include_edges: None,
            graph_boost: None,
            include_title_matches: None,
        };
        assert!(!input.include_title_matches.unwrap_or(false));
    }

    #[test]
    fn test_graph_boost_defaults_false() {
        let input = SearchSemanticInput {
            query: "test".to_string(),
            threshold: None,
            limit: None,
            collection_id: None,
            collection: None,
            exclude_collections: None,
            include_markdown: None,
            include_archived: None,
            scope: None,
            node_types: None,
            property_filters: None,
            include_edges: None,
            graph_boost: None,
            include_title_matches: None,
        };
        assert!(!input.graph_boost.unwrap_or(false));
    }
}
