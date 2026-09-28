//! `nodespace search <query>` — semantic search via NodeService.SearchNodes.
//!
//! This wraps `SearchRequest` (the `NodeService.SearchNodes` RPC), not the
//! richer `search_semantic` local-agent tool (which calls the separate
//! `EmbeddingsService.SearchSemantic` RPC via `SearchSemanticInput`).
//! `SearchRequest` exposes `collection`/`collection_id`/`filters`/`threshold`/
//! `include_markdown`, all wired here. It does NOT have `graph_boost`,
//! `scope`, or `exclude_collections`/`include_edges` — those are
//! `SearchSemanticInput`-only fields with no proto/RPC surface on
//! `NodeService` today. Full `search_semantic` parity (a new RPC exposing
//! `EmbeddingsService.SearchSemantic` to the CLI, or extending `SearchRequest`)
//! is deferred to a follow-up; this is the documented CLI subset.

use anyhow::{Context, Result};
use clap::Args;
use nodespace_daemon::nodespace::SearchRequest;

use crate::output;
use crate::NodeClient;

/// Requesting more than `search_ops::MAX_INCLUDE_MARKDOWN_RESULTS` (currently
/// 5) is pointless — the server clamps to it regardless — so this asks for
/// exactly that many rather than an arbitrarily large number. Duplicated as a
/// literal rather than importing the constant: `nodespace-core` is a
/// dev-only dependency of this crate (pulling in SQLite/embeddings for one
/// `usize` is disproportionate), so the two must be kept in sync by hand.
/// `search_nodes_oversized_include_markdown_still_caps_at_five` in
/// `packages/daemon/tests/it/grpc_round_trip.rs` guards the server side of that
/// sync.
const REQUESTED_MARKDOWN_RESULTS: i32 = 5;

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Free-text query. Pass an empty string or "*" when using --type for
    /// type-only listing — both enumerate every node of the type rather than
    /// being treated as a literal search term.
    #[arg(default_value = "")]
    pub query: String,
    /// Filter results to one or more node types (e.g. `--type task --type text`).
    #[arg(long = "type", value_name = "TYPE")]
    pub node_types: Vec<String>,
    /// Filter to a collection by path (mutually exclusive with --collection-id).
    #[arg(long, conflicts_with = "collection_id")]
    pub collection: Option<String>,
    /// Filter to a collection by ID (mutually exclusive with --collection).
    #[arg(long)]
    pub collection_id: Option<String>,
    /// JSON-encoded array of {field, operator, value} filter objects.
    #[arg(long)]
    pub filters: Option<String>,
    /// Semantic similarity threshold, 0.0-1.0 (0.0 = server default of 0.7).
    #[arg(long, default_value_t = 0.0)]
    pub threshold: f32,
    /// Maximum number of results to return (0 = server default, currently 20).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(i32).range(0..))]
    pub limit: i32,
    /// Attach each top result's aggregated subtree markdown to the response,
    /// so a hit can be answered from directly instead of needing a
    /// follow-up `node get`/`node export` per result. Bounded server-side to
    /// the top 5 results regardless of `--limit`; off by default so a plain
    /// search stays cheap.
    #[arg(long)]
    pub include_content: bool,
}

pub async fn run(client: &mut NodeClient, args: SearchArgs, json: bool) -> Result<()> {
    let response = client
        .search_nodes(SearchRequest {
            query: args.query,
            node_types: args.node_types,
            collection: args.collection,
            collection_id: args.collection_id,
            limit: args.limit,
            offset: 0,
            threshold: args.threshold,
            semantic: true,
            filters: args.filters.unwrap_or_default(),
            include_markdown: if args.include_content {
                REQUESTED_MARKDOWN_RESULTS
            } else {
                0
            },
        })
        .await
        .context("SearchNodes RPC failed")?
        .into_inner();

    output::print_node_list(&response, json)
}
