//! `nodespace` CLI library surface.
//!
//! Exposed primarily so integration tests can drive the command handlers
//! against an in-process daemon without shelling out to the built binary.

pub mod commands;
pub mod output;
pub mod terminal;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use nodespace_daemon::{
    AgentSessionServiceClient, DatabaseServiceClient, ImportServiceClient, LocalAgentServiceClient,
    NodeServiceClient,
};
use nodespace_proto::with_message_limits;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::interceptor::InterceptedService;
use tonic::service::Interceptor;
use tonic::transport::Channel;

/// Stamps the ADR-053 `x-ns-database-id` routing header on every outgoing
/// request so the daemon routes it to a specific local database.
///
/// Concrete (not a closure) so the intercepted client types stay nameable and
/// aliasable — see [`NodeClient`] and friends. `database_id: None` stamps
/// nothing, letting the daemon fall back to its default database; that variant
/// is applied uniformly so a client's type is the same whether or not a
/// database was selected.
///
/// Deliberately does NOT stamp `x-ns-client-id` (ADR-026 C5 extension,
/// implemented in the desktop app's own `DatabaseIdInterceptor` at
/// `packages/desktop-app/src-tauri/src/services/grpc_client.rs`) — a separate,
/// same-named struct in a different crate. The CLI is a one-shot process per
/// invocation that never opens `WatchNodes`, so it has no same-origin echo to
/// suppress; leaving its writes untagged means they carry no
/// `source_client_id` and are therefore always visible as foreign writes to
/// any other subscriber (e.g. a desktop window), which is the correct
/// behavior. If the CLI ever needs its own stable client id, add it here to
/// this struct — the desktop-app copy is independent and not shared.
#[derive(Clone)]
pub struct DatabaseIdInterceptor {
    // Some(id) → stamp header on every request; None → stamp nothing (daemon
    // uses its default database).
    database_id: Option<MetadataValue<Ascii>>,
}

impl DatabaseIdInterceptor {
    /// No routing header — the daemon serves its default database.
    pub fn none() -> Self {
        Self { database_id: None }
    }

    /// Stamp `x-ns-database-id: <id>` on every request. `id` must be an already
    /// resolved registry identifier (ULID); the daemon resolves the header as an
    /// id only, never a name.
    pub fn for_id(id: &str) -> Result<Self> {
        let value = MetadataValue::try_from(id)
            .with_context(|| format!("database id '{id}' is not a valid gRPC header value"))?;
        Ok(Self {
            database_id: Some(value),
        })
    }
}

impl Interceptor for DatabaseIdInterceptor {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(id) = &self.database_id {
            req.metadata_mut()
                .insert(nodespace_daemon::DATABASE_ID_HEADER, id.clone());
        }
        Ok(req)
    }
}

/// A UDS channel wrapped so every request carries the database routing header.
pub type Intercepted = InterceptedService<Channel, DatabaseIdInterceptor>;
/// `NodeService` client bound to a selected database.
pub type NodeClient = NodeServiceClient<Intercepted>;
/// `ImportService` client bound to a selected database.
pub type ImportClient = ImportServiceClient<Intercepted>;
/// `AgentSessionService` client bound to a selected database.
pub type SessionClient = AgentSessionServiceClient<Intercepted>;
/// `LocalAgentService` client bound to a selected database.
pub type LocalAgentClient = LocalAgentServiceClient<Intercepted>;

#[derive(Parser, Debug)]
#[command(
    name = "nodespace",
    version,
    about = "Command-line interface for NodeSpace — talks to the local nodespaced daemon over gRPC.",
    long_about = "nodespace is a stateless gRPC client that connects to the nodespaced daemon \
                  via Unix Domain Socket (macOS/Linux) or Named Pipe (Windows) and exposes the \
                  knowledge graph as shell commands.\n\n\
                  Start the daemon with `nodespaced` before invoking subcommands."
)]
pub struct Cli {
    /// Emit raw JSON instead of human-readable output.
    #[arg(long, global = true)]
    pub json: bool,

    /// Override the socket path (macOS/Linux) or Named Pipe name (Windows).
    /// With no flag and no environment variable: on macOS/Linux the CLI dials
    /// ~/.nodespace/daemon.sock, or auto-discovers a running dev/Pro daemon's
    /// socket if that one is absent; on Windows it dials the fixed
    /// `\\.\pipe\nodespace-daemon` pipe.
    /// Honors the `NODESPACED_SOCKET` environment variable when this flag is absent.
    #[arg(long, global = true, env = "NODESPACED_SOCKET")]
    pub socket: Option<String>,

    /// Target a specific local database by name or id (ADR-053).
    /// When omitted, requests route to the daemon's default database.
    /// Honors the `NODESPACE_DATABASE` environment variable when this flag is absent.
    #[arg(long, global = true, env = "NODESPACE_DATABASE")]
    pub database: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Operate on individual nodes (get, create, update, delete, children, query, export, batch-get, batch-update).
    Node {
        #[command(subcommand)]
        action: commands::node::NodeAction,
    },
    /// Manage the local inference model (list, load, recommended).
    Model {
        #[command(subcommand)]
        action: commands::model::ModelAction,
    },
    /// Semantic search across the knowledge graph.
    Search(commands::search::SearchArgs),
    /// Structured property query with comparison operators (equals/contains/gt/lt/gte/lte/in/exists).
    Query(commands::query::QueryArgs),
    /// Developer diagnostics: database path, size, node counts, schema count,
    /// daemon process memory.
    Diagnostics(commands::diagnostics::DiagnosticsArgs),
    /// Import markdown files into NodeSpace.
    Import {
        #[command(subcommand)]
        action: commands::import::ImportAction,
    },
    /// Manage mention relationships between nodes.
    Mention {
        #[command(subcommand)]
        action: commands::mention::MentionAction,
    },
    /// Inspect and manage node type schema definitions.
    Schema {
        #[command(subcommand)]
        action: commands::schema::SchemaAction,
    },
    /// Inspect and control Play automation rule-sets (list, logs, enable,
    /// disable, get-workflow-state).
    Playbook {
        #[command(subcommand)]
        action: commands::playbook::PlaybookAction,
    },
    /// Manage typed relationship edges between nodes (distinct from mentions).
    Relationship {
        #[command(subcommand)]
        action: commands::relationship::RelationshipAction,
    },
    /// Inspect and resolve the local conflict journal (list, show, dismiss, adopt, merge).
    Conflicts {
        #[command(subcommand)]
        action: commands::conflicts::ConflictsAction,
    },
    /// Manage PTY agent sessions (launch, attach, list, kill).
    Session {
        #[command(subcommand)]
        action: commands::session::SessionAction,
    },
    /// Manage the daemon's registry of local databases (list, create, register, remove, rename, use).
    Database {
        #[command(subcommand)]
        action: commands::database::DatabaseAction,
    },
    /// Uninstall NodeSpace: stop daemon, remove binaries and service registration.
    Uninstall(commands::uninstall::UninstallArgs),
    /// Install, remove, or check the NodeSpace skill for detected AI-agent
    /// harnesses (Claude Code, Codex, Gemini CLI, OpenCode) -- the CLI-only
    /// equivalent of the desktop app's first-launch skill installer.
    Skill {
        #[command(subcommand)]
        action: commands::skill::SkillAction,
    },
    /// Host a stdio MCP server exposing one passthrough tool, for bash-less
    /// MCP surfaces (e.g. Claude Desktop's Chat tab) that cannot shell this
    /// CLI directly — see `commands::mcp` for the architecture and its
    /// ADR-038 trust-boundary controls. Disabled until `nodespace mcp
    /// install` explicitly turns it on. With no subcommand, hosts the stdio
    /// server itself — what a client config launches, not something a
    /// person types directly.
    Mcp {
        #[command(subcommand)]
        action: Option<commands::mcp::McpAction>,
    },
}

/// Resolve the socket path from an explicit override or env/default.
#[cfg(unix)]
pub fn resolve_socket_path(override_: Option<&str>) -> std::path::PathBuf {
    if let Some(p) = override_ {
        return std::path::PathBuf::from(p);
    }
    if let Ok(p) = std::env::var(nodespace_proto::socket::SOCKET_ENV_VAR) {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    discover_socket_in(&std::path::PathBuf::from(home).join(nodespace_proto::socket::STATE_DIR))
}

/// Pick the daemon socket to dial when none is set explicitly. The daemon socket
/// filename is scoped by the app's build variant (release/dev × community/Pro),
/// so a running dev or Pro app does not listen on the canonical `daemon.sock`.
/// Prefer the canonical path, but if it is absent, auto-discover a running
/// daemon of another variant so the CLI works against whichever app is open
/// without needing `NODESPACED_SOCKET`. When none exist, return the canonical
/// path so the caller reports a clean "is the daemon running?" error.
#[cfg(unix)]
fn discover_socket_in(dir: &std::path::Path) -> std::path::PathBuf {
    // Order = preference: canonical first, then the other build variants. The
    // names come from the shared transport contract so this probe list cannot
    // drift from what the daemon actually binds.
    use nodespace_proto::socket::DAEMON_SOCKET_NAMES;
    for name in DAEMON_SOCKET_NAMES {
        let candidate = dir.join(name);
        if candidate.exists() {
            return candidate;
        }
    }
    dir.join(DAEMON_SOCKET_NAMES[0])
}

/// Resolve the Named Pipe name from an explicit override or env/default.
/// Windows counterpart of [`resolve_socket_path`] — kept as a separate
/// function (rather than folded into one cross-platform `resolve_socket_path`)
/// because the two transports resolve differently: Unix probes several
/// build-variant-scoped socket files on disk, while the pipe namespace is
/// machine-global with no per-variant scoping and nothing to probe (see
/// `nodespace_proto::socket::DAEMON_PIPE_NAME`'s doc comment). Returns a
/// `String` since that is what [`tokio::net::windows::named_pipe::ClientOptions::open`]
/// takes; callers that need a `Path` (to stay call-site-compatible with the
/// Unix side) wrap it themselves.
#[cfg(windows)]
pub fn resolve_pipe_name(override_: Option<&str>) -> String {
    if let Some(p) = override_ {
        return p.to_string();
    }
    if let Ok(p) = std::env::var(nodespace_proto::socket::SOCKET_ENV_VAR) {
        return p;
    }
    nodespace_proto::socket::DAEMON_PIPE_NAME.to_string()
}

/// Build a tonic `Channel` connected over a Unix Domain Socket.
#[cfg(unix)]
async fn dial_channel(sock: &std::path::Path) -> Result<Channel> {
    use hyper_util::rt::TokioIo;
    use tokio::net::UnixStream;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let sock = sock.to_path_buf();
    // The URI host is ignored for UDS — tonic needs a syntactically valid URI.
    let channel = Endpoint::from_static("http://localhost")
        .connect_with_connector(service_fn(move |_: Uri| {
            let sock = sock.clone();
            async move { UnixStream::connect(&sock).await.map(TokioIo::new) }
        }))
        .await?;
    Ok(channel)
}

/// Build a tonic `Channel` connected over a Named Pipe (Windows). Mirrors the
/// daemon's server-side setup (`packages/daemon/src/main.rs`) and the desktop
/// app's own client-side pipe transport
/// (`packages/desktop-app/src-tauri/src/services/grpc_client.rs`) — same pipe
/// name convention, same connector shape, just a CLI-local copy since this
/// crate has no dependency on the desktop-app crate.
///
/// `pipe` arrives as a `Path` (matching [`dial_channel`]'s Unix signature) so
/// every `connect*` helper below stays platform-agnostic; only its resolution
/// (see [`resolve_pipe_name`]) and this dial step are Windows-specific.
#[cfg(windows)]
async fn dial_channel(pipe: &std::path::Path) -> Result<Channel> {
    use hyper_util::rt::TokioIo;
    use tokio::net::windows::named_pipe::ClientOptions;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let pipe = pipe.to_string_lossy().into_owned();
    // The URI host is ignored for a Named Pipe — tonic needs a syntactically
    // valid URI, same as the UDS connector above.
    let channel = Endpoint::from_static("http://localhost")
        .connect_with_connector(service_fn(move |_: Uri| {
            let pipe = pipe.clone();
            async move { ClientOptions::new().open(&pipe).map(TokioIo::new) }
        }))
        .await?;
    Ok(channel)
}

/// Friendly "daemon isn't running" context for a failed connect. Shared by
/// both transports — `sock` names either a Unix socket path or a Windows
/// Named Pipe, `.display()` renders either correctly.
fn connect_error_context(sock: &std::path::Path) -> String {
    format!(
        "Could not connect to nodespaced at {}.\n\
         Is the daemon running? Start it with `nodespaced` in another terminal.",
        sock.display()
    )
}

/// Connect a `NodeService` client bound to the selected database, returning a
/// friendly error if the daemon isn't running.
pub async fn connect(
    sock: &std::path::Path,
    interceptor: DatabaseIdInterceptor,
) -> Result<NodeClient> {
    dial_channel(sock)
        .await
        .map(|channel| {
            with_message_limits!(NodeServiceClient::with_interceptor(channel, interceptor))
        })
        .with_context(|| connect_error_context(sock))
}

/// Connect an `ImportService` client bound to the selected database.
pub async fn connect_import(
    sock: &std::path::Path,
    interceptor: DatabaseIdInterceptor,
) -> Result<ImportClient> {
    dial_channel(sock)
        .await
        .map(|channel| {
            with_message_limits!(ImportServiceClient::with_interceptor(channel, interceptor))
        })
        .with_context(|| connect_error_context(sock))
}

/// Connect an `AgentSessionService` client bound to the selected database.
pub async fn connect_session(
    sock: &std::path::Path,
    interceptor: DatabaseIdInterceptor,
) -> Result<SessionClient> {
    dial_channel(sock)
        .await
        .map(|channel| {
            with_message_limits!(AgentSessionServiceClient::with_interceptor(
                channel,
                interceptor
            ))
        })
        .with_context(|| connect_error_context(sock))
}

/// Connect a `LocalAgentService` client bound to the selected database.
pub async fn connect_local_agent(
    sock: &std::path::Path,
    interceptor: DatabaseIdInterceptor,
) -> Result<LocalAgentClient> {
    dial_channel(sock)
        .await
        .map(|channel| {
            with_message_limits!(LocalAgentServiceClient::with_interceptor(
                channel,
                interceptor
            ))
        })
        .with_context(|| connect_error_context(sock))
}

/// Connect a `DatabaseService` client. This operates on the daemon's database
/// registry globally, so it carries no routing header (unlike the data-plane
/// clients above).
pub async fn connect_database(sock: &std::path::Path) -> Result<DatabaseServiceClient<Channel>> {
    dial_channel(sock)
        .await
        .map(|channel| with_message_limits!(DatabaseServiceClient::new(channel)))
        .with_context(|| connect_error_context(sock))
}

/// Resolve the `--database` selection into a routing interceptor plus the
/// resolved database id (if any).
///
/// The daemon resolves the `x-ns-database-id` header as an id (ULID) only, so a
/// selection given as a name is resolved to its id here, against the registry,
/// before any data-plane request is made. `None` selection routes to the
/// daemon's default database. The returned id is `None` for the default and
/// `Some(id)` for an explicit selection — diagnostics needs it to identify which
/// registry entry it targeted.
async fn resolve_routing(
    sock: &std::path::Path,
    selection: Option<&str>,
) -> Result<(DatabaseIdInterceptor, Option<String>)> {
    match selection {
        None => Ok((DatabaseIdInterceptor::none(), None)),
        Some(sel) => {
            let mut db = connect_database(sock).await?;
            let id = commands::database::resolve_database_id_by_selection(&mut db, sel).await?;
            let interceptor = DatabaseIdInterceptor::for_id(&id)?;
            Ok((interceptor, Some(id)))
        }
    }
}

/// Top-level dispatch — wired by `main.rs` and reused by integration tests.
///
/// `sock` names the daemon endpoint for whichever transport this platform
/// uses — a Unix Domain Socket path on macOS/Linux, a Named Pipe name
/// (wrapped as a `Path` so every downstream `connect*` helper stays
/// platform-agnostic) on Windows. Everything below this resolution is shared.
pub async fn run(cli: Cli) -> Result<()> {
    #[cfg(unix)]
    let sock = resolve_socket_path(cli.socket.as_deref());
    #[cfg(windows)]
    let sock = std::path::PathBuf::from(resolve_pipe_name(cli.socket.as_deref()));

    let json = cli.json;
    let selection = cli.database.as_deref();

    match cli.command {
        Command::Node { action } => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect(&sock, interceptor).await?;
            commands::node::run(&mut client, action, json).await
        }
        Command::Model { action } => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect_local_agent(&sock, interceptor).await?;
            commands::model::run(&mut client, action, json).await
        }
        Command::Search(args) => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect(&sock, interceptor).await?;
            commands::search::run(&mut client, args, json).await
        }
        Command::Query(args) => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect(&sock, interceptor).await?;
            commands::query::run(&mut client, args, json).await
        }
        Command::Diagnostics(args) => {
            let (interceptor, target_id) = resolve_routing(&sock, selection).await?;
            let mut node_client = connect(&sock, interceptor).await?;
            let mut db_client = connect_database(&sock).await?;
            commands::diagnostics::run(
                &mut node_client,
                &mut db_client,
                target_id.as_deref(),
                args,
                json,
            )
            .await
        }
        Command::Import { action } => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect_import(&sock, interceptor).await?;
            commands::import::run(&mut client, action, json).await
        }
        Command::Mention { action } => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect(&sock, interceptor).await?;
            commands::mention::run(&mut client, action, json).await
        }
        Command::Schema { action } => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect(&sock, interceptor).await?;
            commands::schema::run(&mut client, action, json).await
        }
        Command::Playbook { action } => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect(&sock, interceptor).await?;
            commands::playbook::run(&mut client, action, json).await
        }
        Command::Relationship { action } => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect(&sock, interceptor).await?;
            commands::relationship::run(&mut client, action, json).await
        }
        Command::Conflicts { action } => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect(&sock, interceptor).await?;
            commands::conflicts::run(&mut client, action, json).await
        }
        Command::Session { action } => {
            let (interceptor, _) = resolve_routing(&sock, selection).await?;
            let mut client = connect_session(&sock, interceptor).await?;
            commands::session::run(&mut client, action, json).await
        }
        // The `database` subcommands operate on the registry globally and are
        // never routed by `--database` — they use a plain DatabaseService client.
        Command::Database { action } => {
            let mut client = connect_database(&sock).await?;
            commands::database::run(&mut client, action, json).await
        }
        Command::Uninstall(args) => commands::uninstall::run(args),
        // `install`/`uninstall`/`status` never touch the daemon -- they shell
        // out to the bundled/compiled skill installer directly. `guidance`
        // and `reset` are the two `skill` subcommands that do: `guidance`
        // fetches procedural guidance from the graph via the same
        // `NodeService.SearchNodes` RPC `search` uses, and `reset` calls the
        // new `NodeService.ResetSeedNode` RPC -- both need a routed
        // `NodeClient` here.
        Command::Skill { action } => match action {
            commands::skill::SkillAction::Guidance(args) => {
                let (interceptor, _) = resolve_routing(&sock, selection).await?;
                let mut client = connect(&sock, interceptor).await?;
                commands::skill::run_guidance(&mut client, args, json).await
            }
            commands::skill::SkillAction::Reset(args) => {
                let (interceptor, _) = resolve_routing(&sock, selection).await?;
                let mut client = connect(&sock, interceptor).await?;
                commands::skill::run_reset(&mut client, args, json).await
            }
            other => commands::skill::run(other),
        },
        // `mcp` doesn't connect to the daemon itself — each dispatched call
        // shells back out to this same binary (see `commands::mcp`), so it
        // only needs the resolved socket path and raw database selection,
        // not a client. `install`/`uninstall`/`status` need neither: they
        // touch `~/.nodespace/daemon.toml` and a client's own MCP config
        // directly.
        Command::Mcp { action } => {
            commands::mcp::run(action, sock.clone(), cli.database.clone()).await
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::discover_socket_in;

    /// With no socket set, the CLI dials the canonical `daemon.sock` when it
    /// exists, otherwise auto-discovers whichever build-variant daemon is
    /// actually running, and falls back to the canonical path (for a clean
    /// error) when none exist.
    #[test]
    fn discover_socket_prefers_canonical_then_falls_back_to_a_variant() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // Nothing running → canonical default (so the connect error is clean).
        assert_eq!(discover_socket_in(dir), dir.join("daemon.sock"));

        // Only a dev-Pro daemon is up → discover it without NODESPACED_SOCKET.
        std::fs::write(dir.join("daemon-dev-pro.sock"), b"").unwrap();
        assert_eq!(discover_socket_in(dir), dir.join("daemon-dev-pro.sock"));

        // Canonical present → always preferred over the variants.
        std::fs::write(dir.join("daemon.sock"), b"").unwrap();
        assert_eq!(discover_socket_in(dir), dir.join("daemon.sock"));
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::resolve_pipe_name;
    use std::sync::Mutex;

    /// Both tests below mutate `NODESPACED_SOCKET`, which is process-global —
    /// serialize them so they don't race each other under a multi-threaded
    /// test runner. Same shape as the equivalent lock in
    /// `desktop-app/src-tauri/src/services/grpc_client.rs`'s `windows_tests`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_lock_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn resolve_pipe_name_prefers_explicit_override_over_env_and_default() {
        let _guard = env_lock_guard();
        let prev = std::env::var_os("NODESPACED_SOCKET");
        std::env::set_var("NODESPACED_SOCKET", r"\\.\pipe\ns-env");

        assert_eq!(
            resolve_pipe_name(Some(r"\\.\pipe\ns-flag")),
            r"\\.\pipe\ns-flag",
            "an explicit --socket override must win over NODESPACED_SOCKET"
        );

        match prev {
            Some(v) => std::env::set_var("NODESPACED_SOCKET", v),
            None => std::env::remove_var("NODESPACED_SOCKET"),
        }
    }

    #[test]
    fn resolve_pipe_name_honors_env_override_then_falls_back_to_default() {
        let _guard = env_lock_guard();
        let prev = std::env::var_os("NODESPACED_SOCKET");

        std::env::set_var("NODESPACED_SOCKET", r"\\.\pipe\ns-test");
        assert_eq!(
            resolve_pipe_name(None),
            r"\\.\pipe\ns-test",
            "NODESPACED_SOCKET override must win when no --socket flag is given"
        );

        std::env::remove_var("NODESPACED_SOCKET");
        assert_eq!(
            resolve_pipe_name(None),
            r"\\.\pipe\nodespace-daemon",
            "default pipe name must match the daemon's own DAEMON_PIPE_NAME"
        );

        match prev {
            Some(v) => std::env::set_var("NODESPACED_SOCKET", v),
            None => std::env::remove_var("NODESPACED_SOCKET"),
        }
    }
}
