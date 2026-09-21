//! `nodespaced` library surface.
//!
//! The daemon crate ships both a binary (`nodespaced`) and a library so
//! integration tests can spin the gRPC server up in-process without shelling
//! out. Proto types are provided by the `nodespace-proto` crate; this lib
//! re-exports them alongside the service implementations.

pub mod db_routing;
pub mod router;
pub mod services;
pub mod tray;

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Resolve NodeSpace's home directory — the parent of the `.nodespace/` state
/// directory that holds the database registry (`databases.toml`) and the
/// managed database files.
///
/// Honors `NODESPACE_HOME` so a test harness or alternate deployment can
/// redirect *all* NodeSpace state — registry and databases together — with a
/// single override. This is what keeps a redirected database from poisoning the
/// real user's registry: without it, pointing only the database elsewhere (via
/// `NODESPACED_DB_PATH`) while inheriting the real home dir would seed the real
/// `~/.nodespace/databases.toml` with a throwaway path (ADR-053). Falls back to
/// the user's home directory.
pub fn nodespace_home() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("NODESPACE_HOME") {
        return Ok(PathBuf::from(custom));
    }
    dirs::home_dir().context(
        "Cannot determine the NodeSpace home directory: home directory is unknown and NODESPACE_HOME not set",
    )
}

/// The `.nodespace/` state directory under [`nodespace_home`].
pub fn nodespace_dir() -> Result<PathBuf> {
    Ok(nodespace_home()?.join(".nodespace"))
}

/// Resolve the on-disk database path the daemon (and any in-process clients
/// such as the CLI's `diagnostics` subcommand) should consult.
///
/// Honors `NODESPACED_DB_PATH` if set so integration tests and alternate
/// deployments can redirect a single database file without recompiling;
/// otherwise defaults to `<nodespace_dir>/database/nodespace.db` (which itself
/// follows `NODESPACE_HOME`).
pub fn resolve_db_path() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("NODESPACED_DB_PATH") {
        return Ok(PathBuf::from(custom));
    }

    Ok(nodespace_dir()?.join("database").join("nodespace.db"))
}

/// Create a directory (and any missing parents) owner-only from the instant it
/// exists, and restrict it to owner-only even if it already existed at a wider
/// mode. `create_dir_all` applies the ambient umask to a default `0o777`, so a
/// plain create-then-`chmod` — or no `chmod` at all — leaves the directory
/// briefly (or permanently) group/other-traversable: a process of another uid
/// that opens a directory descriptor on it during that window keeps resolving
/// names inside it afterward even once a later `chmod` lands, because path
/// resolution from an already-open directory descriptor never re-checks the
/// execute bit. `umask` can only clear bits, so requesting `0o700` at creation
/// (`DirBuilder::mode`) is owner-only under every ambient umask; the explicit
/// `set_permissions` afterward is the backstop for a directory that something
/// else — an earlier daemon version, or a different subsystem that shares this
/// directory and created it first during this same boot — already left at a
/// wider mode.
///
/// Every daemon-owned path under `nodespace_dir()` should be created through
/// this helper: it holds the UDS whose mode is the whole local-authorization
/// boundary (ADR-052), the database file, the database registry, and the
/// settings file (which carries third-party API keys).
///
/// `dir` is not always daemon-owned, though: `NODESPACED_SOCKET`,
/// `NODESPACE_HOME` and `NODESPACED_DB_PATH` let a caller point straight at a
/// directory this process never created and does not own — most commonly a
/// shared system directory such as `/tmp` used directly (rather than a fresh,
/// owned subdirectory under it) for a one-off isolated/CI/sandboxed daemon.
/// `chmod` on a directory you don't own always fails with `EPERM`, so
/// treating every `chmod` failure as fatal made even a directory this
/// process legitimately owns, but can't `chmod` for some other reason (a
/// filesystem like virtiofs/9p/FUSE that rejects `chmod` independent of
/// actual access, say), refuse to start — with a bare, unexplained `EPERM`
/// and no indication of why.
///
/// A failed `chmod` is therefore only tolerated when re-stating `dir`
/// afterward proves BOTH that this process owns it (matching effective uid)
/// AND that its mode is already owner-only: mode bits alone are not enough,
/// because a directory some other uid owns can show a mode of `0o700` and
/// still grant this process zero access whatsoever — those bits govern the
/// *owner's* access, and a non-owner is locked out by them just the same as
/// by any wider mode's group/other bits being unset for a different reason.
/// Failing that combined check is a hard, named error explaining exactly
/// what's wrong (this is a stronger check than `bind_uds_owner_only`'s own
/// fail-closed re-check right before binding, which only looks at mode bits
/// — so a caller that skips this helper is still caught there for the
/// foreign-mode-bits case, if not the foreign-owner-with-narrow-bits one).
#[cfg(unix)]
pub async fn create_dir_owner_only(dir: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    tokio::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .await
        .with_context(|| format!("create dir {}", dir.display()))?;
    if let Err(chmod_err) =
        tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await
    {
        let metadata = std::fs::metadata(dir)
            .with_context(|| format!("stat {} after failed chmod", dir.display()))?;
        let mode = metadata.permissions().mode() & 0o777;
        let owner_uid = metadata.uid();
        // SAFETY: `geteuid()` takes no arguments, has no preconditions, and
        // cannot fail -- it just returns this process's effective uid.
        let our_uid = unsafe { libc::geteuid() };
        anyhow::ensure!(
            owner_uid == our_uid && mode & 0o077 == 0,
            "{} is owned by uid {owner_uid} (this process is uid {our_uid}) at mode {mode:o}, and this \
             process cannot narrow it to owner-only ({chmod_err}). A narrow mode alone doesn't \
             make a directory usable: owned by a different uid, it grants this process no \
             access at all, not even to create a socket inside it -- point at a directory this \
             process actually owns (e.g. a dedicated subdirectory), not a shared system \
             directory like its parent",
            dir.display(),
        );
    }
    Ok(())
}

/// Windows has no POSIX permission-bit model to apply here. Directory access
/// control on that platform is a separate, currently-open gap (see the Named
/// Pipe DACL note in `main.rs`), so this is a plain recursive create.
#[cfg(not(unix))]
pub async fn create_dir_owner_only(dir: &std::path::Path) -> Result<()> {
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("create dir {}", dir.display()))
}

// Re-export proto types from the lightweight nodespace-proto crate so existing
// consumers of `nodespace-daemon` types continue to work without changing imports.
pub use nodespace_proto::nodespace;
pub use nodespace_proto::{
    AgentAvailability, AgentSessionServiceClient, AgentSessionServiceServer, CaptureContentLevel,
    CaptureSettingsResponse, CheckAvailabilityRequest, CheckAvailabilityResponse,
    DatabaseServiceClient, DatabaseServiceServer, EmbeddingsServiceClient, EmbeddingsServiceServer,
    GetCaptureSettingsRequest, ImportServiceClient, ImportServiceServer, LaunchSessionRequest,
    LaunchSessionResponse, ListSessionsRequest, ListSessionsResponse, LocalAgentServiceClient,
    LocalAgentServiceServer, NodeData, NodeServiceClient, NodeServiceServer, ResizeRequest,
    ResizeResponse, SessionInfo, SettingsServiceClient, SettingsServiceServer, StreamOutputRequest,
    TerminateSessionRequest, TerminateSessionResponse, UpdateCaptureSettingsRequest,
    WriteInputRequest, WriteInputResponse,
};

pub use db_routing::{DbManagerLayer, DATABASE_ID_HEADER};
pub use router::{build_base_router, BaseServices};
pub use services::{
    build_database_services, build_shared_services, AgentSessionHandler, DatabaseManager,
    DatabaseServiceImpl, DatabaseServices, EmbeddingsServiceImpl, ImportServiceImpl,
    LocalAgentServiceImpl, McpConfig, NodeServiceImpl, SettingsServiceImpl, SharedContext,
    SharedLocalAgent, SharedServices, SubtreeGateFactory,
};
// The `nodespace` CLI's `mcp` subcommand (a separate process from the
// daemon) reads/writes these directly against `~/.nodespace/daemon.toml` --
// see `packages/cli/src/commands/mcp.rs` and `McpConfig`'s doc comment.
pub use services::settings_service::{read_mcp_settings, set_mcp_enabled};
