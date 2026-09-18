//! `nodespace playbook ...` — inspect and control Play automation rule-sets.
//!
//! Per ADR-035's capability-parity clause, three of these four operations are
//! thin reductions to an existing generic verb rather than bespoke RPC/CLI
//! surface:
//! - `list` -> `ExecuteQuery` filtered to `node_type: "play"`
//! - `enable`/`disable` -> `UpdateNode` setting `lifecycle_status` to
//!   `"active"`/`"archived"` (the engine's `handle_play_updated` already
//!   treats any non-`"active"` status as disabled)
//!
//! `get-workflow-state` is the one operation that needs purpose-built
//! evaluation — it runs the engine's condition logic out of band from a live
//! trigger — and is the only new RPC this adds (`GetWorkflowState`). See
//! `nodespace_core::playbook::workflow_state` for the evaluation design
//! (fired-state scoping, synthetic-event substitution, typo-vs-unmet
//! classification).

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nodespace_daemon::nodespace::{
    ExecuteQueryRequest, GetWorkflowStateRequest, UpdateNodeRequest,
};

use crate::output;
use crate::NodeClient;

#[derive(Subcommand, Debug)]
pub enum PlaybookAction {
    /// List all installed Plays and their lifecycle status.
    List(PlaybookListArgs),
    /// Re-enable a disabled Play after fixing the underlying issue.
    Enable(PlaybookIdArgs),
    /// Manually disable a Play.
    Disable(PlaybookIdArgs),
    /// Evaluate a node against every active Play rule that could apply to
    /// its type, and report which conditions are satisfied, not yet met, or
    /// unresolvable (a likely typo in a condition's path).
    #[command(name = "get-workflow-state")]
    GetWorkflowState(GetWorkflowStateArgs),
}

#[derive(Args, Debug)]
pub struct PlaybookListArgs {}

#[derive(Args, Debug)]
pub struct PlaybookIdArgs {
    /// Play ID (node ID of the `play` node).
    pub play_id: String,
}

#[derive(Args, Debug)]
pub struct GetWorkflowStateArgs {
    /// ID of the node to evaluate active Play rules against.
    pub node_id: String,
}

pub async fn run(client: &mut NodeClient, action: PlaybookAction, json: bool) -> Result<()> {
    match action {
        PlaybookAction::List(args) => list(client, args, json).await,
        PlaybookAction::Enable(args) => set_lifecycle_status(client, args, "active", json).await,
        PlaybookAction::Disable(args) => set_lifecycle_status(client, args, "archived", json).await,
        PlaybookAction::GetWorkflowState(args) => get_workflow_state(client, args, json).await,
    }
}

async fn list(client: &mut NodeClient, _args: PlaybookListArgs, json: bool) -> Result<()> {
    let response = client
        .execute_query(ExecuteQueryRequest {
            target_type: "play".to_string(),
            filters_json: None,
            sorting_json: None,
            limit: 0,
        })
        .await
        .context("ExecuteQuery RPC failed")?
        .into_inner();

    output::print_node_list(&response, json)
}

/// Shared implementation for `enable`/`disable`: both are exactly a
/// `lifecycle_status` update on the Play node — the engine's own
/// `handle_play_updated` (packages/core/src/playbook/engine.rs) already
/// treats any non-`"active"` status as disabled, so no Play-specific verb or
/// validation is needed beyond the generic update path.
async fn set_lifecycle_status(
    client: &mut NodeClient,
    args: PlaybookIdArgs,
    lifecycle_status: &str,
    json: bool,
) -> Result<()> {
    let response = client
        .update_node(UpdateNodeRequest {
            node_id: args.play_id,
            version: None,
            node_type: None,
            content: None,
            properties: None,
            add_to_collections: vec![],
            remove_from_collection_ids: vec![],
            lifecycle_status: Some(lifecycle_status.to_string()),
            add_to_collection_ids: vec![],
        })
        .await
        .context("UpdateNode RPC failed")?
        .into_inner();

    output::print_node(
        &response.node_data.context("daemon returned no node_data")?,
        json,
    )
}

/// `_json` is unused: the response is already a structured report (rules,
/// per-condition state, the fired-state disclaimer) rather than a node or
/// node list, so — like `schema.rs`'s `print_schema_result` — there is no
/// separate human-rendered form to fall back to; both modes print the same
/// pretty JSON.
async fn get_workflow_state(
    client: &mut NodeClient,
    args: GetWorkflowStateArgs,
    _json: bool,
) -> Result<()> {
    let response = client
        .get_workflow_state(GetWorkflowStateRequest {
            node_id: args.node_id,
        })
        .await
        .context("GetWorkflowState RPC failed")?
        .into_inner();

    let value: serde_json::Value = serde_json::from_str(&response.result_json)
        .context("daemon returned malformed result_json")?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
