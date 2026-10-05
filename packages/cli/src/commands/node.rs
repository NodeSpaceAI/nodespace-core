//! `nodespace node ...` subcommands — thin gRPC wrappers around NodeService.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nodespace_daemon::nodespace::{
    CreateNodeRequest, DeleteNodeRequest, ExportMarkdownRequest, GetChildrenRequest,
    GetNodeContextRequest, GetNodeRequest, GetNodesBatchRequest, MoveNodeRequest, NodeSortOrder,
    QueryNodesSimpleRequest, ReorderNodeRequest, UpdateNodeRequest, UpdateNodesBatchRequest,
};
use nodespace_types::RelationshipPath;
use serde_json::json;

use crate::output;
use crate::NodeClient;

#[derive(Subcommand, Debug)]
pub enum NodeAction {
    /// Retrieve a node by ID.
    Get(GetArgs),
    /// Read a node with the nodes its relationship paths reach, and the
    /// skills attached to any of them.
    Context(ContextArgs),
    /// Create a new node.
    Create(CreateArgs),
    /// Update an existing node's content and/or properties.
    Update(UpdateArgs),
    /// Set a task node's status (dedicated verb — do not use `update` for this).
    #[command(name = "set-status")]
    SetStatus(SetStatusArgs),
    /// Move a node under another parent (or to the root), or change its
    /// position among its siblings. The node keeps its ID and everything
    /// nested under it.
    Move(MoveArgs),
    /// Delete a node and everything nested under it, in two steps: without
    /// `--version`/`--descendants` it only previews what would be removed and
    /// prints the exact command that deletes it.
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
pub struct ContextArgs {
    /// Node ID (UUID).
    pub id: String,
    /// A relationship path to follow from the node (repeatable): relationship
    /// names joined by `.`, e.g. `project` or `spec.decisions`. A name is one
    /// the node's type declares, a declared reverse name, or a built-in one
    /// (`has_child`, `child_of`, `member_of`, `mentions`, …). `*` after a name
    /// follows it repeatedly: `child_of*` reaches every ancestor. A name the
    /// type does not declare is an error. With no path, the node comes back
    /// with the skills attached to it alone.
    #[arg(long = "path", value_name = "PATH", value_parser = parse_path)]
    pub paths: Vec<RelationshipPath>,
}

fn parse_path(s: &str) -> Result<RelationshipPath, String> {
    s.parse()
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
    /// Set several properties at once from one JSON object:
    /// `--properties '{"key":"value"}'`. Each value keeps the JSON type it is
    /// written with, so this is the form for nested values and for a string
    /// that reads as a number (`{"estimate":"3"}`). May be combined with
    /// `--property`, which wins for a key given both ways.
    #[arg(long = "properties", value_name = "JSON", value_parser = parse_properties_object)]
    pub properties_json: Option<serde_json::Map<String, serde_json::Value>>,
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
    /// Set several properties at once from one JSON object:
    /// `--properties '{"key":"value"}'`, deep-merged like `--property`. Each
    /// value keeps the JSON type it is written with. May be combined with
    /// `--property`, which wins for a key given both ways.
    #[arg(long = "properties", value_name = "JSON", value_parser = parse_properties_object)]
    pub properties_json: Option<serde_json::Map<String, serde_json::Value>>,
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
    /// The node version you read. Updates only if the node is still at it;
    /// otherwise nothing is written and the current version is reported.
    /// Omit to update whatever is current.
    #[arg(long)]
    pub version: Option<i64>,
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

/// Parses the `--properties` JSON object. Anything but an object is refused
/// here, so a mistyped blob fails as a usage error instead of reaching the
/// daemon.
fn parse_properties_object(s: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    match serde_json::from_str(s) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        Ok(_) => Err("expected a JSON object, e.g. '{\"key\":\"value\"}'".to_string()),
        Err(e) => Err(format!("invalid JSON: {e}")),
    }
}

/// The properties a `create` or `update` sends: the `--properties` object
/// with each `--property` entry laid over it. `None` when neither was given.
fn merge_properties(
    object: Option<serde_json::Map<String, serde_json::Value>>,
    entries: Vec<(String, serde_json::Value)>,
) -> Option<String> {
    if object.is_none() && entries.is_empty() {
        return None;
    }
    let mut map = object.unwrap_or_default();
    map.extend(entries);
    Some(serde_json::Value::Object(map).to_string())
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
    /// The task version you read. Sets the status only if the task is still
    /// at it; otherwise nothing is written and the current version is
    /// reported. Omit to update whatever is current.
    #[arg(long)]
    pub version: Option<i64>,
}

#[derive(Args, Debug)]
pub struct MoveArgs {
    /// Node ID to move.
    pub id: String,
    /// New parent node ID. Omit (with `--root` also omitted) to keep the
    /// current parent and only change the position among its siblings.
    #[arg(long, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub parent: Option<String>,
    /// Make the node a root node (no parent). Root nodes have no order, so
    /// this takes no position.
    #[arg(long, conflicts_with_all = ["parent", "first", "after"])]
    pub root: bool,
    /// Place the node first among its siblings. With neither `--first` nor
    /// `--after`, a node given a new parent is placed last.
    #[arg(long, conflicts_with = "after")]
    pub first: bool,
    /// Place the node directly after this sibling, which must be a child of
    /// the parent the node ends up under.
    #[arg(
        long,
        value_name = "SIBLING_ID",
        value_parser = clap::builder::NonEmptyStringValueParser::new()
    )]
    pub after: Option<String>,
    /// The node version you read. Moves only if the node is still at it;
    /// otherwise nothing is written and the current version is reported.
    /// Omit to move whatever is current.
    #[arg(long)]
    pub version: Option<i64>,
}

#[derive(Args, Debug)]
pub struct DeleteArgs {
    /// Node ID to delete.
    pub id: String,
    /// The node version its preview showed. Deletes only if it still matches.
    #[arg(long, requires = "descendants")]
    pub version: Option<i64>,
    /// The nested-node count its preview showed. Deletes only if it still
    /// matches.
    #[arg(long, requires = "version")]
    pub descendants: Option<u64>,
    /// The global `--socket`/`--database` flags this run was given, repeated
    /// in the preview's delete command so it reaches the same database. Set
    /// by dispatch, not parsed.
    #[arg(skip)]
    pub routing: Vec<String>,
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
        NodeAction::Context(args) => context(client, args, json).await,
        NodeAction::Create(args) => create(client, args, json).await,
        NodeAction::Update(args) => update(client, args, json).await,
        NodeAction::SetStatus(args) => set_status(client, args, json).await,
        NodeAction::Move(args) => move_node(client, args, json).await,
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

async fn context(client: &mut NodeClient, args: ContextArgs, json: bool) -> Result<()> {
    let response = client
        .get_node_context(GetNodeContextRequest {
            node_id: args.id,
            paths_json: Some(serde_json::to_string(&args.paths)?),
        })
        .await
        .map_err(|status| match status.code() {
            // The daemon's message names the path, or the node, at fault.
            tonic::Code::InvalidArgument | tonic::Code::NotFound => {
                anyhow::anyhow!("{}", status.message())
            }
            _ => anyhow::Error::new(status).context("GetNodeContext RPC failed"),
        })?
        .into_inner();

    output::print_node_context(&response, json)
}

async fn create(client: &mut NodeClient, args: CreateArgs, json: bool) -> Result<()> {
    let properties = merge_properties(args.properties_json, args.properties).unwrap_or_default();

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
        && args
            .properties_json
            .as_ref()
            .is_none_or(|map| map.is_empty())
        && args.collections.is_empty()
        && args.collection_ids.is_empty()
        && args.remove_collection_ids.is_empty()
    {
        anyhow::bail!(
            "update requires --content, --property, --properties, --collection, --collection-id, or --remove-collection-id"
        );
    }

    let properties = merge_properties(args.properties_json, args.properties);

    let response = client
        .update_node(UpdateNodeRequest {
            node_id: args.id,
            version: args.version, // unset: the server reads the current version
            node_type: None,
            content: args.content,
            properties,
            add_to_collections: args.collections,
            add_to_collection_ids: args.collection_ids,
            remove_from_collection_ids: args.remove_collection_ids,
            lifecycle_status: None,
            typed_client: false,
        })
        .await
        .map_err(|status| write_refused(status, "UpdateNode", json))?
        .into_inner();

    let node = response.node_data.context("daemon returned no node_data")?;
    output::print_node(&node, json)
}

async fn set_status(client: &mut NodeClient, args: SetStatusArgs, json: bool) -> Result<()> {
    // No client-side vocabulary check: `task.status` is `extensible: true`
    // (schema update's `add_field_values`), so the daemon's live schema —
    // not a list baked into this binary — is the only source of truth for
    // which values are valid. An invalid value comes back from the RPC
    // below as an InvalidArgument status naming the value and the current
    // list (the update pipeline's enum check).
    let properties = json!({ "status": args.status }).to_string();

    let response = client
        .update_node(UpdateNodeRequest {
            node_id: args.id,
            version: args.version, // unset: the server reads the current version
            node_type: None,
            content: None,
            properties: Some(properties),
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
            typed_client: false,
        })
        .await
        .map_err(|status| write_refused(status, "UpdateNode", json))?
        .into_inner();

    let node = response.node_data.context("daemon returned no node_data")?;
    output::print_node(&node, json)
}

/// A write the daemon refused because the node has changed since the version
/// the caller named (ADR-094 §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionConflict {
    pub node_id: String,
    /// The version the caller gave.
    pub given_version: i64,
    /// The version the node is at now.
    pub current_version: i64,
}

impl VersionConflict {
    /// The conflict a failed update carries, if that is why it failed. The
    /// daemon reports one as `ABORTED` with the versions in a metadata header.
    pub fn from_status(status: &tonic::Status) -> Option<Self> {
        if status.code() != tonic::Code::Aborted {
            return None;
        }
        let header = status.metadata().get("x-version-conflict")?.to_str().ok()?;
        let payload: serde_json::Value = serde_json::from_str(header).ok()?;
        Some(Self {
            node_id: payload.get("node_id")?.as_str()?.to_string(),
            given_version: payload.get("expected")?.as_i64()?,
            current_version: payload.get("actual")?.as_i64()?,
        })
    }

    /// What the caller is told: which node, the version given, the version
    /// it is at now, and that nothing was written.
    pub fn message(&self) -> String {
        format!(
            "Node {} has changed since it was read: version {} was given and it is now at \
             version {}. Nothing was written. Read the node again before deciding what to do.",
            self.node_id, self.given_version, self.current_version
        )
    }

    /// The same as structured output, for `--json`.
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "error": "version_conflict",
            "node_id": self.node_id,
            "given_version": self.given_version,
            "current_version": self.current_version,
            "message": self.message(),
        })
    }
}

/// The error for a failed versioned write (`rpc` names it). A version conflict
/// is reported as what it is, and under `--json` also printed as structured
/// output; any other failure keeps the daemon's status in its chain.
fn write_refused(status: tonic::Status, rpc: &str, json: bool) -> anyhow::Error {
    match VersionConflict::from_status(&status) {
        Some(conflict) => {
            if json {
                println!("{:#}", conflict.to_json());
            }
            anyhow::anyhow!(conflict.message())
        }
        None => anyhow::Error::new(status).context(format!("{rpc} RPC failed")),
    }
}

/// A new parent (or `--root`) is a `MoveNode`; a position alone is a
/// `ReorderNode` under the parent the node already has.
async fn move_node(client: &mut NodeClient, args: MoveArgs, json: bool) -> Result<()> {
    use nodespace_daemon::nodespace::move_node_request::Position as MovePosition;
    use nodespace_daemon::nodespace::reorder_node_request::Position as ReorderPosition;

    let reparent = args.parent.is_some() || args.root;
    if !reparent && !args.first && args.after.is_none() {
        anyhow::bail!("move requires --parent, --root, --first, or --after");
    }

    // `MoveNode` places a node last when the sibling it names is not under the
    // parent, so a mistyped `--after` is caught here. `ReorderNode` refuses
    // one itself.
    if let (Some(parent), Some(sibling)) = (&args.parent, &args.after) {
        require_child_of(client, sibling, parent, &args.id).await?;
    }

    // Both RPCs take the version they write at; without `--version` that is
    // whatever the node is at now.
    let version = match args.version {
        Some(version) => version,
        None => {
            client
                .get_node(GetNodeRequest {
                    node_id: args.id.clone(),
                })
                .await
                .context("GetNode RPC failed")?
                .into_inner()
                .node_data
                .context("daemon returned no node_data")?
                .version
        }
    };

    if reparent {
        let position = match (args.first, args.after) {
            (true, _) => Some(MovePosition::Beginning(true)),
            (false, Some(sibling)) => Some(MovePosition::After(sibling)),
            (false, None) => None, // the server places it last
        };
        let response = client
            .move_node(MoveNodeRequest {
                node_id: args.id,
                version,
                new_parent_id: args.parent, // unset with `--root`
                position,
            })
            .await
            .map_err(|status| write_refused(status, "MoveNode", json))?
            .into_inner();

        let node = response.node_data.context("daemon returned no node_data")?;
        return output::print_node(&node, json);
    }

    let position = match args.after {
        Some(sibling) => ReorderPosition::After(sibling),
        None => ReorderPosition::Beginning(true),
    };
    client
        .reorder_node(ReorderNodeRequest {
            node_id: args.id.clone(),
            version,
            position: Some(position),
        })
        .await
        .map_err(|status| write_refused(status, "ReorderNode", json))?;

    // ReorderNode returns nothing; read the node back for its new version.
    let node = client
        .get_node(GetNodeRequest { node_id: args.id })
        .await
        .context("GetNode RPC failed")?
        .into_inner()
        .node_data
        .context("daemon returned no node_data")?;
    output::print_node(&node, json)
}

/// Fail unless `sibling` is a child of `parent` other than `node` itself.
async fn require_child_of(
    client: &mut NodeClient,
    sibling: &str,
    parent: &str,
    node: &str,
) -> Result<()> {
    if sibling == node {
        anyhow::bail!("Node {node} cannot be placed after itself");
    }
    let children = client
        .get_children(GetChildrenRequest {
            node_id: parent.to_string(),
        })
        .await
        .context("GetChildren RPC failed")?
        .into_inner()
        .nodes;
    if !children.iter().any(|child| child.id == sibling) {
        anyhow::bail!(
            "Node {sibling} is not a child of {parent}; a node can only be placed after a child \
             of the parent it goes under. Nothing was written."
        );
    }
    Ok(())
}

/// A delete names what it removes before it removes it (ADR-080): the bare
/// form previews, and only the form carrying the preview's version and
/// nested-node count deletes — refused if either has changed since.
async fn delete(client: &mut NodeClient, args: DeleteArgs, json: bool) -> Result<()> {
    let dry_run = args.descendants.is_none();
    let response = client
        .delete_node(DeleteNodeRequest {
            node_id: args.id,
            version: args.version,
            dry_run,
            expected_descendant_count: args.descendants,
            expected_node_type: None,
        })
        .await
        .context("DeleteNode RPC failed")?
        .into_inner();

    if dry_run {
        output::print_delete_preview(&response, &args.routing, json)
    } else {
        output::print_delete(&response, json)
    }
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
            include_archived: false,
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

#[cfg(test)]
mod tests {
    use super::{merge_properties, CreateArgs, NodeAction, VersionConflict};
    use clap::Parser;

    fn conflict_status(header: &str) -> tonic::Status {
        let mut status = tonic::Status::aborted("Version conflict on n-1: expected 3, got 5");
        status
            .metadata_mut()
            .insert("x-version-conflict", header.parse().unwrap());
        status
    }

    #[test]
    fn a_version_conflict_is_read_from_the_daemons_status() {
        let status =
            conflict_status(r#"{"node_id":"n-1","expected":3,"actual":5,"current_node":null}"#);
        let conflict = VersionConflict::from_status(&status).expect("a conflict");
        assert_eq!(
            conflict,
            VersionConflict {
                node_id: "n-1".into(),
                given_version: 3,
                current_version: 5,
            }
        );
        assert_eq!(
            conflict.message(),
            "Node n-1 has changed since it was read: version 3 was given and it is now at \
             version 5. Nothing was written. Read the node again before deciding what to do."
        );
    }

    #[test]
    fn any_other_failure_is_not_a_version_conflict() {
        // Another ABORTED, and a conflict header on another code.
        assert_eq!(
            VersionConflict::from_status(&tonic::Status::aborted("something else")),
            None
        );
        let mut other = tonic::Status::invalid_argument("bad");
        other.metadata_mut().insert(
            "x-version-conflict",
            r#"{"node_id":"n-1","expected":3,"actual":5}"#.parse().unwrap(),
        );
        assert_eq!(VersionConflict::from_status(&other), None);
        assert_eq!(
            VersionConflict::from_status(&conflict_status("not json")),
            None
        );
    }

    fn create_args(argv: &[&str]) -> Result<CreateArgs, clap::Error> {
        let argv = ["nodespace", "node", "create", "--type", "task"]
            .iter()
            .chain(argv);
        match crate::Cli::try_parse_from(argv)?.command {
            crate::Command::Node {
                action: NodeAction::Create(args),
            } => Ok(args),
            other => panic!("expected node create, got {other:?}"),
        }
    }

    #[test]
    fn a_properties_object_keeps_each_values_json_type() {
        let args = create_args(&[
            "--properties",
            r#"{"estimate":"3","points":3,"tags":["a"]}"#,
        ])
        .expect("a JSON object parses");
        assert_eq!(
            merge_properties(args.properties_json, args.properties).as_deref(),
            Some(r#"{"estimate":"3","points":3,"tags":["a"]}"#)
        );
    }

    #[test]
    fn a_property_flag_wins_over_the_same_key_in_the_object() {
        let args = create_args(&[
            "--properties",
            r#"{"priority":"low","status":"open"}"#,
            "--property",
            "priority=high",
        ])
        .expect("both forms parse together");
        assert_eq!(
            merge_properties(args.properties_json, args.properties).as_deref(),
            Some(r#"{"priority":"high","status":"open"}"#)
        );
    }

    #[test]
    fn properties_that_are_not_a_json_object_are_a_usage_error() {
        for bad in ["[1,2]", "\"text\"", "{not json"] {
            let err = create_args(&["--properties", bad]).expect_err("refused");
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation, "{bad}");
        }
    }

    #[test]
    fn no_property_flag_sends_no_properties() {
        let args = create_args(&[]).expect("parses");
        assert_eq!(
            merge_properties(args.properties_json, args.properties),
            None
        );
    }
}
