//! `nodespace database ...` — manage the daemon's registry of local databases
//! (ADR-053: "One Daemon, Multiple Local Databases").
//!
//! These subcommands operate on the registry globally: they list, create,
//! register, remove, rename, and choose the default database. Unlike the
//! data-plane commands they are never routed by the global `--database` flag —
//! they always talk to the daemon's single `DatabaseService`.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nodespace_daemon::nodespace::{
    CreateDatabaseRequest, DatabaseInfo, DatabaseStatus, ListDatabasesRequest,
    RegisterDatabaseRequest, RemoveDatabaseRequest, RenameDatabaseRequest,
    SetDefaultDatabaseRequest,
};
use nodespace_daemon::DatabaseServiceClient;
use serde_json::json;
use tonic::transport::Channel;

#[derive(Subcommand, Debug)]
pub enum DatabaseAction {
    /// List every registered database with its status and the default marker.
    List,
    /// Create a brand-new database and register it.
    Create(CreateArgs),
    /// Register an existing database file already present on disk.
    Register(RegisterArgs),
    /// Unregister a database (never deletes the underlying file).
    Remove(RemoveArgs),
    /// Rename a registered database's human-facing label.
    Rename(RenameArgs),
    /// Set the daemon-wide default database (used when no database is selected).
    Use(UseArgs),
}

#[derive(Args, Debug)]
pub struct CreateArgs {
    /// Human-facing label for the new database.
    pub name: String,
    /// Explicit path for the new database file. When omitted the daemon places
    /// it under its managed database directory.
    #[arg(long)]
    pub path: Option<String>,
}

#[derive(Args, Debug)]
pub struct RegisterArgs {
    /// Absolute path to an existing database file to register.
    pub path: String,
}

#[derive(Args, Debug)]
pub struct RemoveArgs {
    /// Database to unregister, by name or id.
    pub database: String,
}

#[derive(Args, Debug)]
pub struct RenameArgs {
    /// Database to rename, by name or id.
    pub database: String,
    /// New human-facing label.
    pub new_name: String,
}

#[derive(Args, Debug)]
pub struct UseArgs {
    /// Database to make the default, by name or id.
    pub database: String,
}

pub async fn run(
    client: &mut DatabaseServiceClient<Channel>,
    action: DatabaseAction,
    json: bool,
) -> Result<()> {
    match action {
        DatabaseAction::List => list(client, json).await,
        DatabaseAction::Create(args) => create(client, args, json).await,
        DatabaseAction::Register(args) => register(client, args, json).await,
        DatabaseAction::Remove(args) => remove(client, args, json).await,
        DatabaseAction::Rename(args) => rename(client, args, json).await,
        DatabaseAction::Use(args) => use_default(client, args, json).await,
    }
}

/// Resolve a database selection (name or id) to its registry id.
///
/// The daemon resolves the `x-ns-database-id` routing header as an id (ULID)
/// only, so the CLI must resolve names to ids itself. An exact id match wins;
/// otherwise a unique name match is used. An unknown selection, or a name shared
/// by multiple databases, is a hard error — the latter tells the caller to
/// disambiguate by id. Shared by the global `--database` flag and the
/// `remove`/`rename`/`use` subcommands.
pub async fn resolve_database_id_by_selection(
    client: &mut DatabaseServiceClient<Channel>,
    selection: &str,
) -> Result<String> {
    let listed = client
        .list(ListDatabasesRequest {})
        .await
        .context("List RPC failed")?
        .into_inner();

    // An exact id match is unambiguous — prefer it over any name match.
    if let Some(info) = listed.databases.iter().find(|d| d.id == selection) {
        return Ok(info.id.clone());
    }

    let by_name: Vec<&DatabaseInfo> = listed
        .databases
        .iter()
        .filter(|d| d.name == selection)
        .collect();
    match by_name.as_slice() {
        [] => anyhow::bail!(
            "no database named or with id '{selection}' is registered; \
             run `nodespace database list`"
        ),
        [one] => Ok(one.id.clone()),
        many => {
            let ids: Vec<&str> = many.iter().map(|d| d.id.as_str()).collect();
            anyhow::bail!(
                "'{selection}' is ambiguous — {} databases share that name ({}); \
                 select by id instead",
                many.len(),
                ids.join(", ")
            )
        }
    }
}

/// The registry id of the daemon's default database, if one is set.
///
/// Lets the CLI stamp an explicit routing header even when the caller passed no
/// `--database`, so the target of an invocation is a fact the CLI knows rather
/// than one the daemon resolves silently per request. See `resolve_routing`.
///
/// `Ok(None)` means the daemon reports no default. That is left for the command
/// to surface: an unstamped request would fail downstream on the same
/// condition, and turning it into a routing error here would report it in the
/// wrong vocabulary.
///
/// Reads `default_database_id` rather than scanning the entries for
/// `is_default`. The response carries the same fact twice — the scalar is the
/// registry's own answer, the per-entry flag is derived from it — and today
/// they cannot disagree, since both are built from one snapshot under one lock.
/// The scalar is still the right one to trust: were that invariant ever to slip
/// (a default id naming an entry the listing omits), a scan would match nothing
/// and this would return `None`, quietly restoring the unstamped routing this
/// function exists to replace. An empty string is the daemon's "unset", not an
/// id.
pub async fn resolve_default_database_id(
    client: &mut DatabaseServiceClient<Channel>,
) -> Result<Option<String>> {
    let listed = client
        .list(ListDatabasesRequest {})
        .await
        .context("List RPC failed")?
        .into_inner();

    Ok(Some(listed.default_database_id).filter(|id| !id.is_empty()))
}

async fn list(client: &mut DatabaseServiceClient<Channel>, json: bool) -> Result<()> {
    let listed = client
        .list(ListDatabasesRequest {})
        .await
        .context("List RPC failed")?
        .into_inner();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "databases": listed.databases.iter().map(info_to_json).collect::<Vec<_>>(),
                "default_database_id": listed.default_database_id,
            }))?
        );
        return Ok(());
    }

    if listed.databases.is_empty() {
        println!("No databases registered.");
        return Ok(());
    }

    // Leading column marks the default with `*`. Widths are generous enough for
    // ULID ids and typical labels while staying scannable.
    println!("{:<1} {:<28} {:<20} {:<8} PATH", "", "ID", "NAME", "STATUS");
    for d in &listed.databases {
        let marker = if d.is_default { "*" } else { " " };
        println!(
            "{marker:<1} {:<28} {:<20} {:<8} {}",
            d.id,
            d.name,
            status_str(d.status),
            d.path
        );
    }
    Ok(())
}

async fn create(
    client: &mut DatabaseServiceClient<Channel>,
    args: CreateArgs,
    json: bool,
) -> Result<()> {
    let info = client
        .create(CreateDatabaseRequest {
            name: args.name,
            path: args.path,
        })
        .await
        .context("Create RPC failed")?
        .into_inner();

    print_info(&info, json, |i| {
        format!("Created database '{}' ({}) at {}", i.name, i.id, i.path)
    })
}

async fn register(
    client: &mut DatabaseServiceClient<Channel>,
    args: RegisterArgs,
    json: bool,
) -> Result<()> {
    let info = client
        .register(RegisterDatabaseRequest { path: args.path })
        .await
        .context("Register RPC failed")?
        .into_inner();

    print_info(&info, json, |i| {
        format!("Registered database '{}' ({}) at {}", i.name, i.id, i.path)
    })
}

async fn remove(
    client: &mut DatabaseServiceClient<Channel>,
    args: RemoveArgs,
    json: bool,
) -> Result<()> {
    let id = resolve_database_id_by_selection(client, &args.database).await?;
    let response = client
        .remove(RemoveDatabaseRequest { id })
        .await
        .context("Remove RPC failed")?
        .into_inner();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "id": response.id }))?
        );
    } else {
        println!(
            "Unregistered database {} (the underlying file was not deleted).",
            response.id
        );
    }
    Ok(())
}

async fn rename(
    client: &mut DatabaseServiceClient<Channel>,
    args: RenameArgs,
    json: bool,
) -> Result<()> {
    let id = resolve_database_id_by_selection(client, &args.database).await?;
    let info = client
        .rename(RenameDatabaseRequest {
            id,
            name: args.new_name,
        })
        .await
        .context("Rename RPC failed")?
        .into_inner();

    print_info(&info, json, |i| {
        format!("Renamed database {} to '{}'", i.id, i.name)
    })
}

async fn use_default(
    client: &mut DatabaseServiceClient<Channel>,
    args: UseArgs,
    json: bool,
) -> Result<()> {
    let id = resolve_database_id_by_selection(client, &args.database).await?;
    let info = client
        .set_default(SetDefaultDatabaseRequest { id })
        .await
        .context("SetDefault RPC failed")?
        .into_inner();

    // This sets the daemon-wide default: the CLI is stateless, so the registry's
    // default is the single source of truth for every client's header-less
    // requests (those without `--database`/`NODESPACE_DATABASE`).
    print_info(&info, json, |i| {
        format!(
            "Default database is now '{}' ({}); \
             requests without --database route here.",
            i.name, i.id
        )
    })
}

/// Emit a `DatabaseInfo` result: the full record as JSON, or a caller-supplied
/// one-line human summary.
fn print_info(
    info: &DatabaseInfo,
    json: bool,
    human: impl FnOnce(&DatabaseInfo) -> String,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&info_to_json(info))?);
    } else {
        println!("{}", human(info));
    }
    Ok(())
}

fn info_to_json(info: &DatabaseInfo) -> serde_json::Value {
    json!({
        "id": info.id,
        "name": info.name,
        "path": info.path,
        "created_at": info.created_at,
        "last_opened_at": info.last_opened_at,
        "is_default": info.is_default,
        "status": status_str(info.status),
    })
}

/// Human-readable name for a `DatabaseStatus` enum value.
pub(crate) fn status_str(status: i32) -> &'static str {
    match DatabaseStatus::try_from(status) {
        Ok(DatabaseStatus::Closed) => "closed",
        Ok(DatabaseStatus::Open) => "open",
        Ok(DatabaseStatus::Missing) => "missing",
        Err(_) => "unknown",
    }
}
