//! `nodespace playbook ...` — inspect and control Play automation rule-sets.
//!
//! Per ADR-035's capability-parity clause, three of these four operations are
//! thin reductions to an existing write or read rather than bespoke RPC/CLI
//! surface:
//! - `list` -> `QueryNodesSimple` filtered to `node_type: "play"`, reported
//!   with each Play's state (on, off or suspended) beside its lifecycle
//! - `enable`/`disable` -> the typed play update, writing the Play's
//!   `enabled` field. That is the user's switch (ADR-087 §5): it is not
//!   `lifecycle_status`, which is governance, and the engine never changes it.
//!   Enabling also clears a suspension the engine recorded
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
    GetNodeRequest, GetWorkflowStateRequest, NodeData, NodeSortOrder, QueryNodesSimpleRequest,
    UpdatePlayNodeRequest,
};
use serde_json::{json, Value};

use crate::output;
use crate::NodeClient;

#[derive(Subcommand, Debug)]
pub enum PlaybookAction {
    /// List the installed Plays with each one's state: on, off (disabled),
    /// or suspended by the engine on this device, with the reason and time.
    List(PlaybookListArgs),
    /// Switch a Play on. Also clears a suspension: the engine checks the
    /// Play again and suspends it again if the problem remains.
    Enable(PlaybookIdArgs),
    /// Switch a Play off: it stops running and stays in the list.
    Disable(PlaybookIdArgs),
    /// Evaluate a node against every active Play rule that could apply to
    /// its type, and report which conditions are satisfied, not yet met, or
    /// unresolvable (a likely typo in a condition's path).
    #[command(name = "get-workflow-state")]
    GetWorkflowState(GetWorkflowStateArgs),
}

#[derive(Args, Debug)]
pub struct PlaybookListArgs {
    /// Also list archived Plays. An archived Play runs nowhere, whatever
    /// its switch says.
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
        PlaybookAction::Enable(args) => set_enabled(client, args, true, json).await,
        PlaybookAction::Disable(args) => set_enabled(client, args, false, json).await,
        PlaybookAction::GetWorkflowState(args) => get_workflow_state(client, args, json).await,
    }
}

/// A Play's own state, as `playbook list` reports it: its switch and any
/// suspension. Whether the node is archived is governance, not the Play's
/// state, and is listed beside it; an archived Play runs nowhere whatever
/// its state says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    On,
    /// The user switched it off (`enabled` is `false`).
    Off,
    /// The engine took it out of service on this device.
    Suspended,
}

impl PlayState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Suspended => "suspended",
        }
    }

    /// The state of a Play in the CLI's node shape (`output::node_to_json`).
    /// The order is the engine's: the switch, then the suspension.
    pub fn of(play: &Value) -> Self {
        let field = |key: &str| play["properties"].get(key).filter(|v| !v.is_null());
        if field("enabled") == Some(&Value::Bool(false)) {
            Self::Off
        } else if field("suspended_at").is_some() {
            Self::Suspended
        } else {
            Self::On
        }
    }
}

/// One Play as `playbook list --json` reports it: the node, plus its `state`.
fn play_to_json(node: &NodeData) -> Value {
    let mut play = output::node_to_json(node);
    let state = PlayState::of(&play);
    play["state"] = json!(state.as_str());
    play
}

/// One Play's line in the human listing: its id, state, lifecycle and name,
/// and for a suspended Play the reason, the time and the diagnostic.
fn play_line(play: &Value, lifecycle: &str) -> String {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let state = text(&play["state"]);
    let mut line = format!(
        "{}  {:<9}  {:<8}  {}",
        text(&play["id"]),
        state,
        lifecycle,
        text(&play["content"])
    );
    if state == PlayState::Suspended.as_str() {
        let properties = &play["properties"];
        line.push_str(&format!(
            "\n    {} at {}: {}",
            text(&properties["suspended_reason"]),
            text(&properties["suspended_at"]),
            text(&properties["suspended_message"])
        ));
    }
    line
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

    let plays: Vec<Value> = response.nodes.iter().map(play_to_json).collect();
    if json {
        let value = json!({ "count": response.count, "nodes": plays });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if plays.is_empty() {
        println!("No Plays installed");
        return Ok(());
    }
    println!("{} Play(s):", plays.len());
    for (play, node) in plays.iter().zip(&response.nodes) {
        println!("{}", play_line(play, output::lifecycle_label(node)));
    }
    Ok(())
}

/// Shared implementation for `enable`/`disable`: both write the Play's
/// `enabled` field through the typed play update, the same write the app
/// makes. The update validates only what it changes, so a Play whose rules no
/// longer validate can still be switched off.
async fn set_enabled(
    client: &mut NodeClient,
    args: PlaybookIdArgs,
    enabled: bool,
    json: bool,
) -> Result<()> {
    let current = client
        .get_node(GetNodeRequest {
            node_id: args.play_id.clone(),
        })
        .await
        .context("GetNode RPC failed")?
        .into_inner()
        .node_data
        .context("daemon returned no node_data")?;

    let response = client
        .update_play_node(UpdatePlayNodeRequest {
            node_id: args.play_id,
            version: current.version,
            update_json: json!({ "enabled": enabled }).to_string(),
        })
        .await
        .context("UpdatePlayNode RPC failed")?
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

#[cfg(test)]
mod tests {
    use super::*;

    fn play(properties: Value) -> Value {
        json!({
            "id": "play-1",
            "content": "Roll completion up",
            "properties": properties,
        })
    }

    #[test]
    fn a_play_is_on_unless_it_is_off_or_suspended() {
        assert_eq!(PlayState::of(&play(json!({ "rules": [] }))), PlayState::On);
        assert_eq!(
            PlayState::of(&play(json!({ "enabled": true, "suspended_at": null }))),
            PlayState::On
        );
        assert_eq!(
            PlayState::of(&play(json!({ "enabled": false }))),
            PlayState::Off
        );
        assert_eq!(
            PlayState::of(&play(
                json!({ "enabled": true, "suspended_at": "2026-10-02T10:00:00Z" })
            )),
            PlayState::Suspended
        );
    }

    /// The engine's order: a Play the user switched off is off even if it
    /// also carries a suspension.
    #[test]
    fn the_state_follows_the_engines_order() {
        assert_eq!(
            PlayState::of(&play(
                json!({ "enabled": false, "suspended_at": "2026-10-02T10:00:00Z" })
            )),
            PlayState::Off
        );
    }

    #[test]
    fn a_suspended_plays_line_gives_the_reason_time_and_diagnostic() {
        let mut suspended = play(json!({
            "suspended_reason": "action_failed",
            "suspended_at": "2026-10-02T10:00:00Z",
            "suspended_message": "Rule 'close-parent': node 'x' not found"
        }));
        suspended["state"] = json!(PlayState::of(&suspended).as_str());
        let line = play_line(&suspended, "active");
        assert!(
            line.starts_with("play-1  suspended  active    Roll completion up"),
            "{line}"
        );
        assert!(
            line.contains("action_failed at 2026-10-02T10:00:00Z"),
            "{line}"
        );
        assert!(line.contains("node 'x' not found"), "{line}");

        // The lifecycle is listed beside the state, as the node carries it.
        let mut on = play(json!({}));
        on["state"] = json!(PlayState::of(&on).as_str());
        assert_eq!(
            play_line(&on, "archived"),
            "play-1  on         archived  Roll completion up"
        );
    }

    /// `playbook --help` names the verbs that exist, and no other.
    #[test]
    fn the_help_names_only_the_verbs_that_exist() {
        use clap::CommandFactory;
        let mut cli = crate::Cli::command();
        let playbook = cli
            .find_subcommand_mut("playbook")
            .expect("a playbook subcommand");
        let verbs: Vec<String> = playbook
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        assert_eq!(verbs, ["list", "enable", "disable", "get-workflow-state"]);

        let about = playbook.get_about().expect("about text").to_string();
        assert!(!about.contains("logs"), "{about}");
        let long_help = playbook.render_long_help().to_string();
        assert!(
            !long_help.to_lowercase().contains("archive a play"),
            "{long_help}"
        );
    }
}
