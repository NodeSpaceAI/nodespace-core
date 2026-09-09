//! `nodespace conflicts ...` — inspect and resolve the local conflict journal
//! (ADR-068), the durable, resolvable record of a convergence conflict that
//! replaced the `_possible_duplicate` boolean.
//!
//! One journal, one surface, one dismiss mechanism: these commands are a
//! second client onto the exact same `resolve_conflict`/`merge_nodes` store
//! methods the desktop app's Conflicts UI calls, so a dismiss/adopt/merge
//! made here is immediately visible there and vice versa.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nodespace_daemon::nodespace::{
    ConflictsForNodeRequest, GetConflictRequest, ListConflictsRequest, MergeNodesRequest,
    ResolveConflictRequest,
};
use serde_json::json;

use crate::NodeClient;

#[derive(Subcommand, Debug)]
pub enum ConflictsAction {
    /// List conflict records, optionally filtered by status, kind, or participant node.
    List(ListArgs),
    /// Show a single conflict record by its own id, including detail and any prior resolution.
    Show(ShowArgs),
    /// Dismiss a conflict as acceptable — acknowledges it without changing any node.
    Dismiss(DismissArgs),
    /// Resolve a conflict by continuing with an existing node instead of the new one (non-destructive).
    Adopt(AdoptArgs),
    /// Merge a losing node into a surviving node: unions properties, re-points edges, archives the loser.
    Merge(MergeArgs),
}

#[derive(Args, Debug)]
pub struct ListArgs {
    /// Filter by status: open | resolved | dismissed. Omit for every status.
    #[arg(long)]
    pub status: Option<String>,
    /// Filter by kind: unique_field_collision | collection_name_collision.
    #[arg(long)]
    pub kind: Option<String>,
    /// List only conflicts naming this node id as a participant.
    #[arg(long)]
    pub node: Option<String>,
    /// Cap the number of records returned. Ignored when `--node` is set.
    #[arg(long)]
    pub limit: Option<i32>,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    /// Conflict record id.
    pub conflict_id: String,
}

#[derive(Args, Debug)]
pub struct DismissArgs {
    /// Conflict record id.
    pub conflict_id: String,
}

#[derive(Args, Debug)]
pub struct AdoptArgs {
    /// Conflict record id.
    pub conflict_id: String,
    /// The node id to keep — the counterparty is resolved without navigating to it.
    #[arg(long)]
    pub keep: String,
}

#[derive(Args, Debug)]
pub struct MergeArgs {
    /// Surviving node id — receives the union of properties and every re-pointed edge.
    #[arg(long)]
    pub survivor: String,
    /// Losing node id, archived after the merge. Required unless `--conflict-id` names a
    /// two-participant record, in which case the other participant is used.
    #[arg(long)]
    pub loser: Option<String>,
    /// The open conflict record this merge resolves, closed as resolved in the same transaction.
    #[arg(long)]
    pub conflict_id: Option<String>,
}

pub async fn run(client: &mut NodeClient, action: ConflictsAction, json: bool) -> Result<()> {
    match action {
        ConflictsAction::List(args) => list(client, args, json).await,
        ConflictsAction::Show(args) => show(client, args, json).await,
        ConflictsAction::Dismiss(args) => dismiss(client, args, json).await,
        ConflictsAction::Adopt(args) => adopt(client, args, json).await,
        ConflictsAction::Merge(args) => merge(client, args, json).await,
    }
}

async fn list(client: &mut NodeClient, args: ListArgs, json_out: bool) -> Result<()> {
    let records = if let Some(node_id) = args.node {
        client
            .conflicts_for_node(ConflictsForNodeRequest { node_id })
            .await
            .context("ConflictsForNode RPC failed")?
            .into_inner()
            .conflicts
    } else {
        client
            .list_conflicts(ListConflictsRequest {
                status: args.status,
                kind: args.kind,
                limit: args.limit.unwrap_or(0),
            })
            .await
            .context("ListConflicts RPC failed")?
            .into_inner()
            .conflicts
    };

    crate::output::print_conflict_list(&records, json_out)
}

async fn show(client: &mut NodeClient, args: ShowArgs, json_out: bool) -> Result<()> {
    let response = client
        .get_conflict(GetConflictRequest {
            conflict_id: args.conflict_id.clone(),
        })
        .await
        .context("GetConflict RPC failed")?
        .into_inner();

    let record = response
        .conflict
        .with_context(|| format!("no conflict record found with id '{}'", args.conflict_id))?;

    crate::output::print_conflict(&record, json_out)
}

async fn dismiss(client: &mut NodeClient, args: DismissArgs, json_out: bool) -> Result<()> {
    let resolution = serde_json::to_string(&json!({"action": "dismiss"}))
        .context("failed to serialize dismiss resolution")?;
    let response = client
        .resolve_conflict(ResolveConflictRequest {
            conflict_id: args.conflict_id,
            resolution,
        })
        .await
        .context("ResolveConflict RPC failed")?
        .into_inner();

    let record = response
        .conflict
        .context("daemon returned no conflict record")?;
    crate::output::print_conflict(&record, json_out)
}

async fn adopt(client: &mut NodeClient, args: AdoptArgs, json_out: bool) -> Result<()> {
    let resolution = serde_json::to_string(&json!({
        "action": "adopt_existing",
        "adopted": args.keep,
    }))
    .context("failed to serialize adopt-existing resolution")?;
    let response = client
        .resolve_conflict(ResolveConflictRequest {
            conflict_id: args.conflict_id,
            resolution,
        })
        .await
        .context("ResolveConflict RPC failed")?
        .into_inner();

    let record = response
        .conflict
        .context("daemon returned no conflict record")?;
    crate::output::print_conflict(&record, json_out)
}

async fn merge(client: &mut NodeClient, args: MergeArgs, json_out: bool) -> Result<()> {
    let loser_id = match (args.loser, &args.conflict_id) {
        (Some(loser), _) => loser,
        (None, Some(conflict_id)) => {
            let response = client
                .get_conflict(GetConflictRequest {
                    conflict_id: conflict_id.clone(),
                })
                .await
                .context("GetConflict RPC failed")?
                .into_inner();
            let record = response
                .conflict
                .with_context(|| format!("no conflict record found with id '{conflict_id}'"))?;
            let other: Vec<&String> = record
                .node_ids
                .iter()
                .filter(|id| **id != args.survivor)
                .collect();
            match other.as_slice() {
                [only] => (*only).clone(),
                _ => anyhow::bail!(
                    "conflict '{conflict_id}' does not name exactly one other participant \
                     besides --survivor; pass --loser explicitly"
                ),
            }
        }
        (None, None) => {
            anyhow::bail!("merge requires either --loser <node-id> or --conflict-id <id>")
        }
    };

    let response = client
        .merge_nodes(MergeNodesRequest {
            survivor_id: args.survivor,
            loser_id,
            conflict_id: args.conflict_id,
        })
        .await
        .context("MergeNodes RPC failed")?
        .into_inner();

    crate::output::print_merge_outcome(&response, json_out)
}
