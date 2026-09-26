//! `nodespace node ...` subcommands — thin gRPC wrappers around NodeService.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nodespace_daemon::nodespace::{
    CreateNodeRequest, DeleteNodeRequest, ExportMarkdownRequest, GetChildrenRequest,
    GetNodeRequest, GetNodesBatchRequest, NodeSortOrder, QueryNodesSimpleRequest,
    UpdateNodeRequest, UpdateNodesBatchRequest,
};
use serde_json::json;

use crate::output;
use crate::NodeClient;

#[derive(Subcommand, Debug)]
pub enum NodeAction {
    /// Retrieve a node by ID.
    Get(GetArgs),
    /// Create a new node.
    Create(CreateArgs),
    /// Update an existing node's content and/or properties.
    Update(UpdateArgs),
    /// Set a task node's status (dedicated verb — do not use `update` for this).
    #[command(name = "set-status")]
    SetStatus(SetStatusArgs),
    /// Delete a node.
    Delete(DeleteArgs),
    /// List the direct children of a node.
    Children(ChildrenArgs),
    /// Query nodes with structured filters.
    Query(QueryArgs),
    /// Export a node and its subtree as markdown.
    Export(ExportArgs),
    /// Fetch multiple nodes in one request.
    #[command(name = "batch-get")]
    BatchGet(BatchGetArgs),
    /// Update multiple nodes in one request (OCC-aware).
    #[command(name = "batch-update")]
    BatchUpdate(BatchUpdateArgs),
}

#[derive(Args, Debug)]
pub struct GetArgs {
    /// Node ID (UUID).
    pub id: String,
}

#[derive(Args, Debug)]
pub struct CreateArgs {
    /// Node type, e.g. `text`, `task`, `date`.
    #[arg(long = "type")]
    pub node_type: String,
    /// Content (plain text or markdown). Omit for a type with a title
    /// template (e.g. `person`): its name comes from the template's fields,
    /// set with `--property`, and content is rejected.
    #[arg(long)]
    pub content: Option<String>,
    /// Parent node ID (omit to create a root node).
    #[arg(long)]
    pub parent: Option<String>,
    /// Set one or more properties: `--property key=value` (repeatable). Values
    /// are parsed as JSON when possible (numbers, booleans, `null`, arrays,
    /// objects), otherwise treated as a plain string. Required this way for
    /// any schema field that is `required` with no default — validation runs
    /// at create time, so there is no way to supply it afterward via `update`.
    #[arg(long = "property", value_parser = parse_property)]
    pub properties: Vec<(String, serde_json::Value)>,
    /// Collection path to file the node under, `:`-delimited for hierarchy
    /// (e.g. `docs:rust`) — the same syntax `import` and `search` take.
    /// Missing segments are created. Repeatable to join several collections
    /// in one call. Mutually exclusive with --collection-id.
    #[arg(
        long = "collection",
        value_name = "PATH",
        conflicts_with = "collection_ids"
    )]
    pub collections: Vec<String>,
    /// Collection ID to file the node under (repeatable). Prefer
    /// --collection, which takes a readable path and needs no lookup.
    #[arg(long = "collection-id", value_name = "ID")]
    pub collection_ids: Vec<String>,
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Node ID to update.
    pub id: String,
    /// New content. Omit to leave content unchanged (e.g. when only setting properties).
    #[arg(long)]
    pub content: Option<String>,
    /// Set one or more properties: `--property key=value` (repeatable). Values
    /// are parsed as JSON when possible (numbers, booleans, `null`, arrays,
    /// objects), otherwise treated as a plain string. Deep-merged into the
    /// node's existing properties (unspecified keys are left untouched). Do
    /// NOT use this to change a task's status; use `node set-status` instead.
    #[arg(long = "property", value_parser = parse_property)]
    pub properties: Vec<(String, serde_json::Value)>,
    /// Collection path to add the node to, `:`-delimited for hierarchy
    /// (e.g. `docs:rust`). Missing segments are created. Repeatable.
    /// Mutually exclusive with --collection-id.
    #[arg(
        long = "collection",
        value_name = "PATH",
        conflicts_with = "collection_ids"
    )]
    pub collections: Vec<String>,
    /// Collection ID to add the node to (repeatable). Prefer --collection.
    #[arg(long = "collection-id", value_name = "ID")]
    pub collection_ids: Vec<String>,
    /// Collection ID to remove the node from (repeatable).
    #[arg(long = "remove-collection-id", value_name = "ID")]
    pub remove_collection_ids: Vec<String>,
}

/// Parses a `key=value` CLI arg into (key, JSON value). `value` is parsed as
/// JSON when possible so numbers/booleans/null/arrays/objects round-trip
/// without extra quoting; falls back to a plain string otherwise.
fn parse_property(s: &str) -> Result<(String, serde_json::Value), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("expected key=value, got '{s}'"))?;
    if key.is_empty() {
        return Err(format!("property key must not be empty in '{s}'"));
    }
    let parsed = serde_json::from_str(value)
        .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
    Ok((key.to_string(), parsed))
}

#[derive(Args, Debug)]
pub struct SetStatusArgs {
    /// Task node ID.
    pub id: String,
    /// New status. Must be one of the values the `task` schema's `status`
    /// field declares — the four built-ins (open, in_progress, done,
    /// cancelled) plus any added since. An invalid value is rejected with the
    /// current list.
    pub status: String,
}

#[derive(Args, Debug)]
pub struct DeleteArgs {
    /// Node ID to delete.
    pub id: String,
}

#[derive(Args, Debug)]
pub struct ChildrenArgs {
    /// Parent node ID.
    pub id: String,
}

#[derive(Args, Debug)]
pub struct QueryArgs {
    /// Filter by exact node ID.
    #[arg(long)]
    pub id: Option<String>,
    /// Filter nodes that mention this node ID.
    #[arg(long)]
    pub mentioned_by: Option<String>,
    /// Filter by substring in content.
    #[arg(long)]
    pub content_contains: Option<String>,
    /// Filter by substring in title.
    #[arg(long)]
    pub title_contains: Option<String>,
    /// Filter by node type (e.g. `text`, `task`).
    #[arg(long = "type")]
    pub node_type: Option<String>,
    /// Maximum number of results (0 = server default).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..))]
    pub limit: u32,
    /// Result offset for pagination.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..))]
    pub offset: u32,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Node ID to export.
    pub id: String,
    /// Include children recursively (default: true).
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub children: bool,
    /// Maximum recursion depth (0 = server default of 20).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u32).range(0..))]
    pub max_depth: u32,
    /// Embed HTML comments with node IDs for OCC (default: true).
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    pub node_ids: bool,
}

#[derive(Args, Debug)]
pub struct BatchGetArgs {
    /// Node IDs to fetch (repeatable: --id <id1> --id <id2>).
    #[arg(long = "id", required = true)]
    pub ids: Vec<String>,
}

#[derive(Args, Debug)]
pub struct BatchUpdateArgs {
    /// JSON-encoded array of update objects: [{"node_id":"…","content":"…","version":N}].
    /// Each item may have: node_id (required), version (optional), content, node_type, properties.
    #[arg(long)]
    pub updates: String,
}

pub async fn run(client: &mut NodeClient, action: NodeAction, json: bool) -> Result<()> {
    match action {
        NodeAction::Get(args) => get(client, args, json).await,
        NodeAction::Create(args) => create(client, args, json).await,
        NodeAction::Update(args) => update(client, args, json).await,
        NodeAction::SetStatus(args) => set_status(client, args, json).await,
        NodeAction::Delete(args) => delete(client, args, json).await,
        NodeAction::Children(args) => children(client, args, json).await,
        NodeAction::Query(args) => query(client, args, json).await,
        NodeAction::Export(args) => export(client, args, json).await,
        NodeAction::BatchGet(args) => batch_get(client, args, json).await,
        NodeAction::BatchUpdate(args) => batch_update(client, args, json).await,
    }
}

async fn get(client: &mut NodeClient, args: GetArgs, json: bool) -> Result<()> {
    let response = client
        .get_node(GetNodeRequest { node_id: args.id })
        .await
        .context("GetNode RPC failed")?
        .into_inner();

    let node = response.node_data.context("daemon returned no node_data")?;
    output::print_node(&node, json)
}

async fn create(client: &mut NodeClient, args: CreateArgs, json: bool) -> Result<()> {
    let properties = if args.properties.is_empty() {
        String::new()
    } else {
        let map: serde_json::Map<String, serde_json::Value> = args.properties.into_iter().collect();
        serde_json::Value::Object(map).to_string()
    };

    let response = client
        .create_node(CreateNodeRequest {
            node_type: args.node_type,
            content: args.content.unwrap_or_default(),
            parent_id: args.parent,
            properties,
            collections: args.collections,
            collection_ids: args.collection_ids,
            lifecycle_status: None,
            id: None,
            position: None, // CLI create defaults to End
        })
        .await
        .context("CreateNode RPC failed")?
        .into_inner();

    let node = response.node_data.context("daemon returned no node_data")?;
    output::print_node(&node, json)
}

async fn update(client: &mut NodeClient, args: UpdateArgs, json: bool) -> Result<()> {
    if args.content.is_none()
        && args.properties.is_empty()
        && args.collections.is_empty()
        && args.collection_ids.is_empty()
        && args.remove_collection_ids.is_empty()
    {
        anyhow::bail!(
            "update requires --content, --property, --collection, --collection-id, or --remove-collection-id"
        );
    }

    let properties = if args.properties.is_empty() {
        None
    } else {
        let map: serde_json::Map<String, serde_json::Value> = args.properties.into_iter().collect();
        Some(serde_json::Value::Object(map).to_string())
    };

    let response = client
        .update_node(UpdateNodeRequest {
            node_id: args.id,
            version: None, // auto-fetch current version on the server
            node_type: None,
            content: args.content,
            properties,
            add_to_collections: args.collections,
            add_to_collection_ids: args.collection_ids,
            remove_from_collection_ids: args.remove_collection_ids,
            lifecycle_status: None,
        })
        .await
        .context("UpdateNode RPC failed")?
        .into_inner();

    let node = response.node_data.context("daemon returned no node_data")?;
    output::print_node(&node, json)
}

async fn set_status(client: &mut NodeClient, args: SetStatusArgs, json: bool) -> Result<()> {
    const VALID_STATUSES: &[&str] = &["open", "in_progress", "done", "cancelled"];
    if !VALID_STATUSES.contains(&args.status.as_str()) {
        anyhow::bail!(
            "invalid status '{}'; must be one of: {}",
            args.status,
            VALID_STATUSES.join(", ")
        );
    }

    let properties = json!({ "status": args.status }).to_string();

    let response = client
        .update_node(UpdateNodeRequest {
            node_id: args.id,
            version: None, // auto-fetch current version on the server
            node_type: None,
            content: None,
            properties: Some(properties),
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        })
        .await
        .context("UpdateNode RPC failed")?
        .into_inner();

    let node = response.node_data.context("daemon returned no node_data")?;
    output::print_node(&node, json)
}

async fn delete(client: &mut NodeClient, args: DeleteArgs, json: bool) -> Result<()> {
    let response = client
        .delete_node(DeleteNodeRequest {
            node_id: args.id,
            version: None,
        })
        .await
        .context("DeleteNode RPC failed")?
        .into_inner();

    output::print_delete(&response, json)
}

async fn children(client: &mut NodeClient, args: ChildrenArgs, json: bool) -> Result<()> {
    let response = client
        .get_children(GetChildrenRequest { node_id: args.id })
        .await
        .context("GetChildren RPC failed")?
        .into_inner();

    output::print_node_list(&response, json)
}

async fn query(client: &mut NodeClient, args: QueryArgs, json: bool) -> Result<()> {
    let response = client
        .query_nodes_simple(QueryNodesSimpleRequest {
            id: args.id,
            mentioned_by: args.mentioned_by,
            content_contains: args.content_contains,
            title_contains: args.title_contains,
            node_type: args.node_type,
            limit: args.limit,
            offset: args.offset,
            // No CLI flag for this yet — the server's deterministic default
            // (created ascending) is fine for ad hoc querying.
            order_by: NodeSortOrder::Unspecified as i32,
        })
        .await
        .context("QueryNodesSimple RPC failed")?
        .into_inner();

    output::print_node_list(&response, json)
}

async fn export(client: &mut NodeClient, args: ExportArgs, json: bool) -> Result<()> {
    let response = client
        .export_markdown(ExportMarkdownRequest {
            node_id: args.id,
            include_children: Some(args.children),
            max_depth: args.max_depth,
            include_node_ids: Some(args.node_ids),
        })
        .await
        .context("ExportMarkdown RPC failed")?
        .into_inner();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "markdown": response.markdown,
                "node_count": response.node_count,
            }))?
        );
    } else {
        print!("{}", response.markdown);
    }
    Ok(())
}

async fn batch_get(client: &mut NodeClient, args: BatchGetArgs, json: bool) -> Result<()> {
    let response = client
        .get_nodes_batch(GetNodesBatchRequest { node_ids: args.ids })
        .await
        .context("GetNodesBatch RPC failed")?
        .into_inner();

    if json {
        let nodes: Vec<_> = response.nodes.iter().map(output::node_to_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "count": response.count,
                "nodes": nodes,
                "not_found": response.not_found,
            }))?
        );
    } else {
        println!(
            "{} found, {} not found:",
            response.count,
            response.not_found.len()
        );
        for node in &response.nodes {
            println!();
            crate::output::print_node(node, false)?;
        }
        if !response.not_found.is_empty() {
            println!("\nNot found:");
            for id in &response.not_found {
                println!("  {id}");
            }
        }
    }
    Ok(())
}

async fn batch_update(client: &mut NodeClient, args: BatchUpdateArgs, json: bool) -> Result<()> {
    use nodespace_daemon::nodespace::BatchUpdateItem;

    let raw: serde_json::Value = serde_json::from_str(&args.updates)
        .context("--updates must be a JSON array of update objects")?;
    let arr = raw.as_array().context("--updates must be a JSON array")?;

    let mut updates: Vec<BatchUpdateItem> = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let node_id = item["node_id"]
            .as_str()
            .with_context(|| format!("updates[{i}] missing string field 'node_id'"))?
            .to_string();
        updates.push(BatchUpdateItem {
            node_id,
            version: item["version"].as_i64(),
            content: item["content"].as_str().map(str::to_string),
            node_type: item["node_type"].as_str().map(str::to_string),
            properties: item
                .get("properties")
                .filter(|v| !v.is_null())
                .map(|v| v.to_string()),
        });
    }

    let response = client
        .update_nodes_batch(UpdateNodesBatchRequest { updates })
        .await
        .context("UpdateNodesBatch RPC failed")?
        .into_inner();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "count": response.count,
                "updated": response.updated,
                "failed": response.failed.iter().map(|f| json!({
                    "node_id": f.node_id,
                    "error": f.error,
                })).collect::<Vec<_>>(),
            }))?
        );
    } else {
        println!("{} node(s) updated", response.count);
        if !response.failed.is_empty() {
            println!("{} failed:", response.failed.len());
            for f in &response.failed {
                println!("  {}: {}", f.node_id, f.error);
            }
        }
    }
    Ok(())
}
