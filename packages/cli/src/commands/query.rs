//! `nodespace query` — structured property queries with comparison operators.
//!
//! Distinct from `nodespace node query`, which only supports exact-match
//! filters (id/content_contains/title_contains/type). This command exposes
//! the full `execute_query` operator set (equals/contains/gt/lt/gte/lte/in/exists)
//! plus sorting, backed by the same `QueryService` query op the local agent's
//! `search_nodes` tool routes property filters through.
//!
//! Filters and sorting are JSON rather than per-condition flags: a filter
//! item's `value` is free-form (string, number, bool, array — see
//! `AgentFilterItem` in `packages/core/src/ops/query_ops.rs`), which has no
//! natural single-flag representation for every filter type at once.

use anyhow::{Context, Result};
use clap::Args;
use nodespace_daemon::nodespace::ExecuteQueryRequest;

use crate::output;
use crate::NodeClient;

#[derive(Args, Debug)]
pub struct QueryArgs {
    /// Target node type ("task", "text", etc.) or "*" for all types.
    #[arg(long = "type")]
    pub target_type: String,
    /// JSON array of filter conditions, e.g.
    /// `[{"type":"property","operator":"equals","property":"status","value":"open"}]`.
    /// Supported types: property, content, metadata, relationship, related.
    /// A relationship filter names a `path` of relationship names and the
    /// `node_id` it must reach, e.g.
    /// `[{"type":"relationship","operator":"equals","path":["child_of"],"node_id":"<id>"}]`.
    /// Supported operators: equals, contains, gt, lt, gte, lte, in, exists.
    /// A property filter on a date field takes `relative_date` in place of
    /// `value` for a date relative to the day the query runs, e.g.
    /// `[{"type":"property","operator":"lte","property":"due_date","relative_date":{"anchor":"today","offset_days":7}}]`.
    #[arg(long)]
    pub filters: Option<String>,
    /// JSON array of sort configs, e.g. `[{"field":"due_date","direction":"desc"}]`.
    #[arg(long)]
    pub sorting: Option<String>,
    /// Max results to return (0 = server default of 50).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..))]
    pub limit: u32,
}

pub async fn run(client: &mut NodeClient, args: QueryArgs, json: bool) -> Result<()> {
    let response = client
        .execute_query(ExecuteQueryRequest {
            target_type: args.target_type,
            filters_json: args.filters,
            sorting_json: args.sorting,
            limit: args.limit,
        })
        .await
        .context("ExecuteQuery RPC failed")?
        .into_inner();

    output::print_node_list(&response, json)
}
