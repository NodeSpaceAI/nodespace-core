//! Tauri commands for the daemon's registry of local databases (ADR-053:
//! "One Daemon, Multiple Local Databases").
//!
//! Thin wrappers over `DatabaseService` plus the desktop-local "which database
//! am I viewing" selection. The registry operations (list/create/register/
//! rename/remove/set-default) are global to the daemon and never routed;
//! `set_active_database` is a purely local selection that stamps the routing
//! header on the data-plane clients without touching the daemon-wide default.

use crate::services::GrpcClient;
use nodespace_proto::nodespace::{
    CreateDatabaseRequest, DatabaseInfo, DatabaseStatus, GetDaemonVersionRequest,
    ListDatabasesRequest, RegisterDatabaseRequest, RemoveDatabaseRequest, RenameDatabaseRequest,
    SetDefaultDatabaseRequest,
};
use tonic::Request;

/// Report the running daemon's version (its compiled `CARGO_PKG_VERSION`).
///
/// The frontend compares this against its own version on startup to detect a
/// stale *detached* daemon left over from a previous install still bound to the
/// socket, so it can boot out the mismatched daemon and relaunch a matching one
/// Daemon-wide, never routed.
#[tauri::command]
pub async fn get_daemon_version(
    grpc_client: tauri::State<'_, GrpcClient>,
) -> Result<String, String> {
    let mut client = grpc_client.client().await;
    let resp = client
        .get_daemon_version(Request::new(GetDaemonVersionRequest {}))
        .await
        .map_err(|e| format!("Failed to get daemon version: {}", e))?
        .into_inner();
    Ok(resp.version)
}

/// A registered database as surfaced to the frontend (camelCase).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_default: bool,
    /// "closed" | "open" | "missing" | "unknown".
    pub status: String,
    pub created_at: String,
    pub last_opened_at: Option<String>,
    /// The cloud tenant schema this database is bound to (ADR-053); `None` when
    /// the database is local-only (not bound to any tenant).
    pub bound_tenant_schema: Option<String>,
    /// The default (landing) collection id within the bound tenant (ADR-053), a
    /// per-install root; `None` on the public/legacy tenant, where the
    /// frontend falls back to the well-known root id for tree filtering.
    pub bound_tenant_collection: Option<String>,
}

/// The full registry listing plus the daemon-wide default id.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseListing {
    pub databases: Vec<DatabaseEntry>,
    /// Identifier of the daemon-wide default database; empty when none is set.
    pub default_database_id: String,
}

/// Human-readable name for a `DatabaseStatus` enum value.
fn status_str(status: i32) -> String {
    match DatabaseStatus::try_from(status) {
        Ok(DatabaseStatus::Closed) => "closed",
        Ok(DatabaseStatus::Open) => "open",
        Ok(DatabaseStatus::Missing) => "missing",
        Err(_) => "unknown",
    }
    .to_string()
}

fn to_entry(info: DatabaseInfo) -> DatabaseEntry {
    DatabaseEntry {
        id: info.id,
        name: info.name,
        path: info.path,
        is_default: info.is_default,
        status: status_str(info.status),
        created_at: info.created_at,
        last_opened_at: info.last_opened_at,
        bound_tenant_schema: info.bound_tenant_schema,
        bound_tenant_collection: info.bound_tenant_collection,
    }
}

/// List every registered database and identify the daemon-wide default.
#[tauri::command]
pub async fn list_databases(
    grpc_client: tauri::State<'_, GrpcClient>,
) -> Result<DatabaseListing, String> {
    let mut client = grpc_client.database_service_client().await;
    let listing = client
        .list(ListDatabasesRequest {})
        .await
        .map_err(|e| format!("Failed to list databases: {}", e))?
        .into_inner();

    Ok(DatabaseListing {
        databases: listing.databases.into_iter().map(to_entry).collect(),
        default_database_id: listing.default_database_id,
    })
}

/// Create a brand-new database and register it. When `path` is omitted the
/// daemon places the file under its managed database directory.
#[tauri::command]
pub async fn create_database(
    grpc_client: tauri::State<'_, GrpcClient>,
    name: String,
    path: Option<String>,
) -> Result<DatabaseEntry, String> {
    let mut client = grpc_client.database_service_client().await;
    let info = client
        .create(CreateDatabaseRequest { name, path })
        .await
        .map_err(|e| format!("Failed to create database: {}", e))?
        .into_inner();
    Ok(to_entry(info))
}

/// Register an existing database file already present on disk.
#[tauri::command]
pub async fn register_database(
    grpc_client: tauri::State<'_, GrpcClient>,
    path: String,
) -> Result<DatabaseEntry, String> {
    let mut client = grpc_client.database_service_client().await;
    let info = client
        .register(RegisterDatabaseRequest { path })
        .await
        .map_err(|e| format!("Failed to register database: {}", e))?
        .into_inner();
    Ok(to_entry(info))
}

/// Set the daemon-wide default database (used when no database is selected).
#[tauri::command]
pub async fn set_default_database(
    grpc_client: tauri::State<'_, GrpcClient>,
    id: String,
) -> Result<DatabaseEntry, String> {
    let mut client = grpc_client.database_service_client().await;
    let info = client
        .set_default(SetDefaultDatabaseRequest { id })
        .await
        .map_err(|e| format!("Failed to set default database: {}", e))?
        .into_inner();
    Ok(to_entry(info))
}

/// Rename a registered database's human-facing label (does not move the file).
#[tauri::command]
pub async fn rename_database(
    grpc_client: tauri::State<'_, GrpcClient>,
    id: String,
    name: String,
) -> Result<DatabaseEntry, String> {
    let mut client = grpc_client.database_service_client().await;
    let info = client
        .rename(RenameDatabaseRequest { id, name })
        .await
        .map_err(|e| format!("Failed to rename database: {}", e))?
        .into_inner();
    Ok(to_entry(info))
}

/// Unregister a database from the registry. This never deletes the underlying
/// database file — it only removes the registry entry.
#[tauri::command]
pub async fn remove_database(
    grpc_client: tauri::State<'_, GrpcClient>,
    id: String,
) -> Result<String, String> {
    let mut client = grpc_client.database_service_client().await;
    let response = client
        .remove(RemoveDatabaseRequest { id })
        .await
        .map_err(|e| format!("Failed to remove database: {}", e))?
        .into_inner();
    Ok(response.id)
}

/// The database this launch was asked to open, if any.
///
/// The daemon's tray sets `NODESPACE_INITIAL_DATABASE` on the process it spawns
/// when the user picks a specific database from the Databases submenu (the name
/// is `nodespace_daemon::tray::INITIAL_DATABASE_ENV`; the desktop app has no
/// dependency on the daemon crate, so it is spelled out here — keep the two in
/// step).
///
/// Answers the same value for the life of the process, deliberately. Consuming
/// it on first read is tempting — it describes a launch, not a preference — but
/// the store calls this from `load()`, and a second `load()` runs on every
/// launch (the daemon-reconnect listener fires one). A consumed second read
/// returns nothing, and that load then resolves to the remembered database and
/// overwrites the tray's pick. A stable answer keeps concurrent loads agreeing.
///
/// The cost is that a webview reload would re-apply the launch selection over a
/// database the user switched to in-app. Nothing reloads the webview today; if
/// something does, consume it at *apply* time in the store rather than here.
#[tauri::command]
pub fn initial_database_id() -> Option<String> {
    std::env::var("NODESPACE_INITIAL_DATABASE")
        .ok()
        .filter(|id| !id.trim().is_empty())
}

/// Select which database the desktop app is viewing. Rebuilds the routed
/// data-plane clients so subsequent node/import/embeddings/agent-session
/// requests target `id` (or the daemon default when `id` is `None`), and
/// signals the node-event watcher to re-subscribe. This is a desktop-local
/// selection only — it does NOT change the daemon-wide default.
#[tauri::command]
pub async fn set_active_database(
    grpc_client: tauri::State<'_, GrpcClient>,
    id: Option<String>,
) -> Result<(), String> {
    grpc_client.set_active_database(id).await;
    Ok(())
}
