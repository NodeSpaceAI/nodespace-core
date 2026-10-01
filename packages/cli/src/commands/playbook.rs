//! `nodespace playbook ...` — inspect and control Play automation rule-sets.
//!
//! Per ADR-035's capability-parity clause, three of these four operations are
//! thin reductions to an existing generic verb rather than bespoke RPC/CLI
//! surface:
//! - `list` -> `QueryNodesSimple` filtered to `node_type: "play"`
//! - `enable`/`disable` -> `UpdateNode` unarchiving / archiving the Play. An
//!   archived node participates in nothing (ADR-087), the play engine
//!   included, so an archived Play doesn't run and is in no list
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
    GetWorkflowStateRequest, NodeSortOrder, QueryNodesSimpleRequest, UpdateNodeRequest,
};

use crate::output;
use crate::NodeClient;

#[derive(Subcommand, Debug)]
pub enum PlaybookAction {
    /// List the installed Plays that can run. An archived (disabled) Play
    /// is listed only with `--include-archived`.
    List(PlaybookListArgs),
    /// Unarchive a Play so it runs again, after fixing the underlying issue.
    Enable(PlaybookIdArgs),
    /// Archive a Play: it stops running and leaves every list.
    Disable(PlaybookIdArgs),
    /// Evaluate a node against every active Play rule that could apply to
    /// its type, and report which conditions are satisfied, not yet met, or
    /// unresolvable (a likely typo in a condition's path).
    #[command(name = "get-workflow-state")]
    GetWorkflowState(GetWorkflowStateArgs),
}

#[derive(Args, Debug)]
pub struct PlaybookListArgs {
    /// Also list archived Plays, to find the id of one to `enable`.
    #[arg(long)]
    pub include_archived: bool,
}

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

async fn list(client: &mut NodeClient, args: PlaybookListArgs, json: bool) -> Result<()> {
    let response = client
        .query_nodes_simple(QueryNodesSimpleRequest {
            include_archived: args.include_archived,
            id: None,
            mentioned_by: None,
            content_contains: None,
            title_contains: None,
            node_type: Some("play".to_string()),
            limit: 0,
            offset: 0,
            order_by: NodeSortOrder::Unspecified as i32,
        })
        .await
        .context("QueryNodesSimple RPC failed")?
        .into_inner();

    output::print_node_list(&response, json)
}

/// Shared implementation for `enable`/`disable`: both are exactly a
/// lifecycle update on the Play node, through the generic update. The engine
/// asks the participation check whether a Play runs, so no Play-specific verb
/// or validation is needed.
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
            typed_client: false,
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
