//! `nodespace schema ...` — inspect and manage node type schemas.
//!
//! `create`/`update` take a single JSON params blob rather than per-field
//! flags: schema params (fields, relationships, title_template, enum
//! definitions) are a nested, evolving shape defined once in
//! `packages/core/src/schema/mod.rs` (`CreateSchemaParams` /
//! `UpdateSchemaParams`). Mirroring that as a flat CLI flag surface would
//! duplicate and drift from the Rust structs; JSON keeps the CLI a thin,
//! schema-agnostic passthrough — the daemon validates and reports errors.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nodespace_daemon::nodespace::{
    DeleteNodeRequest, GetAllSchemasRequest, GetSchemaDefinitionRequest, SchemaParamsRequest,
};

use crate::output;
use crate::NodeClient;

#[derive(Subcommand, Debug)]
pub enum SchemaAction {
    /// List all schema definitions.
    List(SchemaListArgs),
    /// Get a single schema definition by ID.
    Get(SchemaGetArgs),
    /// Create a new schema from a JSON params blob.
    Create(SchemaParamsArgs),
    /// Update an existing schema from a JSON params blob.
    Update(SchemaParamsArgs),
    /// Delete a schema definition by ID.
    ///
    /// The daemon refuses to delete a core schema, or a schema that other
    /// schemas `extends` — delete the extending schemas first. Relationship
    /// declarations on or pointing at the type are removed with it.
    Delete(SchemaDeleteArgs),
}

#[derive(Args, Debug)]
pub struct SchemaListArgs {}

#[derive(Args, Debug)]
pub struct SchemaGetArgs {
    /// Schema ID (node type identifier, e.g. `task`, `person`).
    pub id: String,
}

#[derive(Args, Debug)]
pub struct SchemaDeleteArgs {
    /// Schema ID to delete (node type identifier, e.g. `adr`, `person`).
    pub id: String,
}

#[derive(Args, Debug)]
pub struct SchemaParamsArgs {
    /// JSON params. For `create`: {"name", "description"?, "fields"?,
    /// "relationships"?, "title_template"?, ...} — see CreateSchemaParams.
    /// For `update`: {"schema_id", "add_fields"?, "remove_fields"?,
    /// "rename_fields"?, "add_field_values"?, "add_relationships"?,
    /// "remove_relationships"?, ...}
    /// — see UpdateSchemaParams. Mutually exclusive with `--params-file`.
    #[arg(long, conflicts_with = "params_file")]
    pub params: Option<String>,
    /// Path to a file containing the JSON params (alternative to inline `--params`).
    #[arg(long)]
    pub params_file: Option<String>,
}

impl SchemaParamsArgs {
    fn resolve(self) -> Result<String> {
        match (self.params, self.params_file) {
            (Some(inline), None) => Ok(inline),
            (None, Some(path)) => std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read params file '{path}'")),
            (None, None) => anyhow::bail!("one of --params or --params-file is required"),
            (Some(_), Some(_)) => unreachable!("clap enforces mutual exclusivity"),
        }
    }
}

pub async fn run(client: &mut NodeClient, action: SchemaAction, json: bool) -> Result<()> {
    match action {
        SchemaAction::List(args) => list(client, args, json).await,
        SchemaAction::Get(args) => get(client, args, json).await,
        SchemaAction::Create(args) => create(client, args, json).await,
        SchemaAction::Update(args) => update(client, args, json).await,
        SchemaAction::Delete(args) => delete(client, args, json).await,
    }
}

async fn create(client: &mut NodeClient, args: SchemaParamsArgs, _json: bool) -> Result<()> {
    let params_json = args.resolve()?;
    let response = client
        .create_schema(SchemaParamsRequest { params_json })
        .await
        .context("CreateSchema RPC failed")?
        .into_inner();

    print_schema_result(&response.result_json)
}

async fn update(client: &mut NodeClient, args: SchemaParamsArgs, _json: bool) -> Result<()> {
    let params_json = args.resolve()?;
    let response = client
        .update_schema(SchemaParamsRequest { params_json })
        .await
        .context("UpdateSchema RPC failed")?
        .into_inner();

    print_schema_result(&response.result_json)
}

/// Schema create/update results (schema_id, fields, relationships, warnings)
/// are structured data either way — human and JSON modes render identically,
/// so this takes no `json` flag (unlike other print_* helpers in this file).
fn print_schema_result(result_json: &str) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_str(result_json).context("daemon returned malformed result_json")?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn list(client: &mut NodeClient, _args: SchemaListArgs, json: bool) -> Result<()> {
    let response = client
        .get_all_schemas(GetAllSchemasRequest {})
        .await
        .context("GetAllSchemas RPC failed")?
        .into_inner();

    output::print_node_list(&response, json)
}

async fn get(client: &mut NodeClient, args: SchemaGetArgs, json: bool) -> Result<()> {
    let response = client
        .get_schema_definition(GetSchemaDefinitionRequest { schema_id: args.id })
        .await
        .context("GetSchemaDefinition RPC failed")?
        .into_inner();

    let node = response.node_data.context("daemon returned no node_data")?;
    output::print_node(&node, json)
}

/// A schema *is* a node — its ID is the node type identifier — so deletion
/// goes through `DeleteNode` rather than a schema-specific RPC, and the
/// daemon's own guard supplies the declaration prerequisite (see the variant
/// doc above) without this needing to pre-check it.
async fn delete(client: &mut NodeClient, args: SchemaDeleteArgs, json: bool) -> Result<()> {
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
