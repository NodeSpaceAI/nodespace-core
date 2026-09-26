//! `nodespace relationship ...` — create and query typed relationships.
//!
//! Distinct from `nodespace mention`: mentions are inline references captured
//! from markdown content, while relationships are named, schema-defined edges
//! between nodes (e.g. `assigned_to`, `blocks`) created explicitly via
//! `create_schema`/`update_schema` relationship definitions.

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use nodespace_daemon::nodespace::{CreateRelationshipRequest, GetRelatedNodesRequest};
use serde_json::json;

use crate::NodeClient;

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum Direction {
    Out,
    In,
}

impl Direction {
    fn as_wire_str(self) -> &'static str {
        match self {
            Direction::Out => "out",
            Direction::In => "in",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum RelationshipAction {
    /// Create a typed relationship edge from one node to another.
    Create(CreateArgs),
    /// List nodes related to a given node via a named relationship.
    Get(GetArgs),
}

#[derive(Args, Debug)]
pub struct CreateArgs {
    /// Source node ID.
    #[arg(long)]
    pub from: String,
    /// Relationship name (as defined on the source node's schema).
    #[arg(long = "type")]
    pub relationship_name: String,
    /// Target node ID.
    #[arg(long)]
    pub to: String,
    /// Optional JSON-encoded edge properties.
    #[arg(long)]
    pub edge_data: Option<String>,
}

#[derive(Args, Debug)]
pub struct GetArgs {
    /// Node ID to query relationships for.
    pub id: String,
    /// Relationship name (as defined on the node's schema).
    #[arg(long = "type")]
    pub relationship_name: String,
    /// Direction to traverse.
    #[arg(long, value_enum, default_value_t = Direction::Out)]
    pub direction: Direction,
}

pub async fn run(client: &mut NodeClient, action: RelationshipAction, json: bool) -> Result<()> {
    match action {
        RelationshipAction::Create(args) => create(client, args, json).await,
        RelationshipAction::Get(args) => get(client, args, json).await,
    }
}

async fn create(client: &mut NodeClient, args: CreateArgs, json_out: bool) -> Result<()> {
    let response = client
        .create_relationship(CreateRelationshipRequest {
            source_id: args.from,
            relationship_name: args.relationship_name,
            target_id: args.to,
            edge_data_json: args.edge_data,
        })
        .await
        .context("CreateRelationship RPC failed")?
        .into_inner();

    if json_out {
        let replaced: Vec<_> = response
            .replaced
            .iter()
            .map(|edge| {
                json!({
                    "source_id": edge.source_id,
                    "relationship_name": edge.relationship_name,
                    "target_id": edge.target_id,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "source_id": response.source_id,
                "relationship_name": response.relationship_name,
                "target_id": response.target_id,
                "replaced": replaced,
            }))?
        );
    } else {
        println!(
            "Created relationship: {} --[{}]--> {}",
            response.source_id, response.relationship_name, response.target_id
        );
        // A cardinality-one end replaces rather than rejects, so a create can
        // silently unassign someone else — say so.
        for edge in &response.replaced {
            println!(
                "Replaced relationship: {} --[{}]--> {}",
                edge.source_id, edge.relationship_name, edge.target_id
            );
        }
    }
    Ok(())
}

async fn get(client: &mut NodeClient, args: GetArgs, json_out: bool) -> Result<()> {
    let response = client
        .get_related_nodes(GetRelatedNodesRequest {
            node_id: args.id,
            relationship_name: args.relationship_name,
            direction: args.direction.as_wire_str().to_string(),
        })
        .await
        .context("GetRelatedNodes RPC failed")?
        .into_inner();

    let related: serde_json::Value = serde_json::from_str(&response.related_nodes_json)
        .context("daemon returned malformed related_nodes_json")?;
    // The daemon serializes these nodes itself, in the frontend's typed shape.
    // Re-key them so this command's nodes match every other command's.
    let related = match &related {
        serde_json::Value::Array(nodes) => serde_json::Value::Array(
            nodes
                .iter()
                .map(crate::output::related_node_to_json)
                .collect(),
        ),
        other => other.clone(),
    };

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "node_id": response.node_id,
                "relationship_name": response.relationship_name,
                "direction": response.direction,
                "count": response.count,
                "nodes": related,
            }))?
        );
    } else {
        // The daemon echoes back the traversal it actually ran, which is not
        // always the one asked for: a declared `reverseName` resolves to the
        // forward name read the other way. Draw the arrow from that, so an
        // inbound traversal never renders as an outbound edge.
        let arrow = if response.direction == "in" {
            format!("<--{}--", response.relationship_name)
        } else {
            format!("--{}-->", response.relationship_name)
        };
        println!(
            "{} related node(s) [{} {} ]:",
            response.count, response.node_id, arrow
        );
        println!("{}", serde_json::to_string_pretty(&related)?);
    }
    Ok(())
}
