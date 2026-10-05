//! `nodespace query` — structured property queries with comparison operators,
//! and `nodespace query run` — a saved query, run by its id or title.
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
use clap::{Args, Subcommand};
use nodespace_daemon::nodespace::{ExecuteQueryRequest, RunSavedQueryRequest};

use crate::output;
use crate::NodeClient;

#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub struct QueryArgs {
    #[command(subcommand)]
    pub command: Option<QueryCommand>,
    /// Target node type ("task", "text", etc.) or "*" for all types.
    #[arg(long = "type", required = true)]
    pub target_type: Option<String>,
    /// JSON array of filter conditions, e.g.
    /// `[{"type":"property","operator":"equals","property":"status","value":"open"}]`.
    /// Supported types: property, content, metadata, relationship, related, permitted.
    /// A relationship filter names a `path` of relationship names and the
    /// `node_id` it must reach, e.g.
    /// `[{"type":"relationship","operator":"equals","path":["child_of"],"node_id":"<id>"}]`.
    /// Supported operators: equals, contains, gt, lt, gte, lte, in, exists.
    /// A property filter on a date field takes `relative_date` in place of
    /// `value` for a date relative to the day the query runs, e.g.
    /// `[{"type":"property","operator":"lte","property":"due_date","relative_date":{"anchor":"today","offset_days":7}}]`.
    /// Any filter takes `"negate": true` to keep the nodes it does not hold
    /// for; on a related filter that is "the path reaches no node matching
    /// the nested filter". A property may be a path into an object field's
    /// value or a link field's parts: `"property":"repository.url"` is the
    /// `url` of the link field `repository`, and `repository.title` its title.
    /// A permitted filter keeps the nodes for which setting `property` to
    /// `value` would not be rejected by a rule, as a dry run of that change
    /// predicts it, e.g.
    /// `[{"type":"permitted","operator":"equals","property":"status","value":"in_progress"}]`.
    /// It is evaluated after the other filters, and sort and limit apply to
    /// what it keeps.
    #[arg(long)]
    pub filters: Option<String>,
    /// JSON array of sort configs, e.g. `[{"field":"due_date","direction":"desc"}]`.
    /// An enum field sorts in the order its schema declares its values, not
    /// alphabetically.
    #[arg(long)]
    pub sorting: Option<String>,
    /// Max results to return (0 = server default of 50).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..))]
    pub limit: u32,
}

#[derive(Subcommand, Debug)]
pub enum QueryCommand {
    /// Run a saved query node by its id or title, with its stored filters,
    /// sorting and limit. The skills attached to the query come back beside
    /// its nodes.
    Run(RunArgs),
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// The saved query's id, or its title (quoted when it has spaces). A
    /// title must name exactly one saved query.
    pub query: String,
    /// JSON array of filter conditions ANDed with the stored ones for this
    /// run, in the shape `nodespace query --filters` takes. The saved query
    /// is not changed.
    #[arg(long)]
    pub filters: Option<String>,
    /// At most this many results (0 = the query's own limit; a query with
    /// none returns every match, up to the server's cap of 500). It can
    /// lower the stored limit, never raise it.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..))]
    pub limit: u32,
    /// Return each result with its context, as `node context` reads it: the
    /// nodes its type's context paths reach, the skills that apply to it and
    /// the version of that read. A skill several results share is printed
    /// once. The limit applies to the results, and at most 50 come back.
    #[arg(long = "with-context")]
    pub with_context: bool,
}

pub async fn run(client: &mut NodeClient, args: QueryArgs, json: bool) -> Result<()> {
    let response = match args.command {
        Some(QueryCommand::Run(run)) => {
            let response = client
                .run_saved_query(RunSavedQueryRequest {
                    query: run.query,
                    filters_json: run.filters,
                    limit: run.limit,
                    with_context: run.with_context,
                })
                .await
                .context("RunSavedQuery RPC failed")?
                .into_inner();
            if run.with_context {
                return output::print_saved_query_context_run(&response, json);
            }
            return output::print_saved_query_run(&response, json);
        }
        None => client
            .execute_query(ExecuteQueryRequest {
                target_type: args
                    .target_type
                    .context("--type is required: the node type to query, or \"*\"")?,
                filters_json: args.filters,
                sorting_json: args.sorting,
                limit: args.limit,
            })
            .await
            .context("ExecuteQuery RPC failed")?
            .into_inner(),
    };

    output::print_node_list(&response, json)
}
