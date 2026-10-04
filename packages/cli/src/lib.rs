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
/// `packages/desktop-app/app-lib/src/services/grpc_client.rs`) — a separate,
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
    /// ~/.nodespace/daemon.sock, or auto-discovers a running dev daemon's
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
    /// Read the daemon's log — where Play execution errors go.
    ///
    /// Engine diagnostics are operational telemetry rather than knowledge, so
    /// they are logged rather than written into the graph; there is nothing to
    /// query for them.
    Logs(commands::logs::LogsArgs),
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
    /// Inspect and control Play automation rule-sets (list, enable, disable,
    /// get-workflow-state).
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
    /// harnesses (Claude Code, Codex, Antigravity CLI, OpenCode, Pi) -- the
    /// CLI-only equivalent of the desktop app's first-launch skill
    /// installer -- and fetch the graph's own skills for a task (`guidance`).
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
/// filename is scoped by the app's build flavour (release or dev), so a running
/// dev app does not listen on the canonical `daemon.sock`. Prefer the canonical
/// path, but if it is absent, auto-discover a running dev daemon so the CLI
/// works against whichever app is open without needing `NODESPACED_SOCKET`.
/// When neither exists, return the canonical path so the caller reports a
/// clean "is the daemon running?" error.
#[cfg(unix)]
fn discover_socket_in(dir: &std::path::Path) -> std::path::PathBuf {
    // Order = preference: canonical first, then the dev flavour. The names come
    // from the shared transport contract so this probe list cannot drift from
    // what the daemon actually binds.
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
/// because the two transports resolve differently: Unix probes the
/// build-flavour-scoped socket files on disk, while the pipe namespace is
/// machine-global with no per-flavour scoping and nothing to probe (see
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
/// (`packages/desktop-app/app-lib/src/services/grpc_client.rs`) — same pipe
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
            async move {
                open_with_busy_retry(
                    || ClientOptions::new().open(&pipe),
                    PIPE_BUSY_RETRIES,
                    PIPE_BUSY_DELAY,
                )
                .await
                .map(TokioIo::new)
            }
        }))
        .await?;
    Ok(channel)
}

/// Win32 `ERROR_PIPE_BUSY`: the pipe exists but every instance is momentarily
/// occupied (daemon running, between accepting one client and re-creating its
/// listening instance).
const ERROR_PIPE_BUSY: i32 = 231;

/// How many times a busy pipe is re-tried, and the pause between tries. The
/// budget (~1s) only applies to `ERROR_PIPE_BUSY`; an absent pipe fails at once.
/// `pub` so the cross-platform tests and the Windows dial share one budget without
/// a dead-code warning on Unix builds.
pub const PIPE_BUSY_RETRIES: u32 = 20;
pub const PIPE_BUSY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

/// Why a connect attempt failed, as far as the user-facing message cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectFailure {
    /// Daemon is up but its pipe stayed occupied for the whole retry budget.
    DaemonBusy,
    /// Anything else, including an absent daemon.
    NotRunning,
}

/// True for a raw OS error code equal to `ERROR_PIPE_BUSY`. Pure so it is
/// testable on every platform (on Unix the code never occurs in production).
fn is_pipe_busy(raw_os_error: Option<i32>) -> bool {
    raw_os_error == Some(ERROR_PIPE_BUSY)
}

/// Classify a connect error by scanning its source chain for an `io::Error`
/// carrying `ERROR_PIPE_BUSY`. `ERROR_FILE_NOT_FOUND` and everything else map
/// to [`ConnectFailure::NotRunning`].
fn classify_connect_error(err: &anyhow::Error) -> ConnectFailure {
    let busy = err
        .chain()
        .filter_map(|e| e.downcast_ref::<std::io::Error>())
        .any(|io| is_pipe_busy(io.raw_os_error()));
    if busy {
        ConnectFailure::DaemonBusy
    } else {
        ConnectFailure::NotRunning
    }
}

/// Run `open`, retrying only while it fails with `ERROR_PIPE_BUSY`, up to
/// `retries` extra attempts with `delay` between them. Any other error
/// (notably `ERROR_FILE_NOT_FOUND`) is returned immediately.
pub async fn open_with_busy_retry<T>(
    mut open: impl FnMut() -> std::io::Result<T>,
    retries: u32,
    delay: std::time::Duration,
) -> std::io::Result<T> {
    let mut attempt = 0;
    loop {
        match open() {
            Err(e) if is_pipe_busy(e.raw_os_error()) && attempt < retries => {
                attempt += 1;
                tokio::time::sleep(delay).await;
            }
            other => return other,
        }
    }
}

/// Friendly context for a failed connect. Shared by both transports — `sock`
/// names either a Unix socket path or a Windows Named Pipe, `.display()`
/// renders either correctly.
fn connect_error_context(sock: &std::path::Path, failure: ConnectFailure) -> String {
    match failure {
        ConnectFailure::DaemonBusy => format!(
            "nodespaced is running at {} but is busy (its pipe stayed occupied). \
             Try again in a moment.",
            sock.display()
        ),
        ConnectFailure::NotRunning => format!(
            "Could not connect to nodespaced at {}.\n\
             Is the daemon running? Start it with `nodespaced` in another terminal.",
            sock.display()
        ),
    }
}

/// Attach the right "busy" / "not running" context to a failed dial.
fn connect_failure(sock: &std::path::Path, err: anyhow::Error) -> anyhow::Error {
    let ctx = connect_error_context(sock, classify_connect_error(&err));
    err.context(ctx)
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
        .map_err(|e| connect_failure(sock, e))
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
        .map_err(|e| connect_failure(sock, e))
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
        .map_err(|e| connect_failure(sock, e))
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
        .map_err(|e| connect_failure(sock, e))
}

/// Connect a `DatabaseService` client. This operates on the daemon's database
/// registry globally, so it carries no routing header (unlike the data-plane
/// clients above).
pub async fn connect_database(sock: &std::path::Path) -> Result<DatabaseServiceClient<Channel>> {
    dial_channel(sock)
        .await
        .map(|channel| with_message_limits!(DatabaseServiceClient::new(channel)))
        .map_err(|e| connect_failure(sock, e))
}

/// Resolve the `--database` selection into a routing interceptor plus the
/// resolved database id.
///
/// The daemon resolves the `x-ns-database-id` header as an id (ULID) only, so a
/// selection given as a name is resolved to its id here, against the registry,
/// before any data-plane request is made.
///
/// **No selection resolves the daemon's default to a concrete id and stamps it,
/// rather than sending no header at all.** Both reach the same database, but
/// only one of them can say which. An unstamped request is routed by the daemon
/// against whatever `registry.default_database` holds at the moment it arrives,
/// so the CLI never learns the answer and cannot report it — and a write that
/// went somewhere else (an agent turn runs against its own event watcher's
/// database, not the default) reads back as a well-formed empty result,
/// indistinguishable from a write that never happened. Pinning the id here
/// makes the target knowable to every caller and stable for the whole
/// invocation.
///
/// The returned id is `None` only when the daemon reports no default database,
/// which is also the one case where an unstamped request would have failed
/// downstream anyway — left to the command to surface, rather than turned into
/// a routing error here.
pub async fn resolve_routing(
    sock: &std::path::Path,
    selection: Option<&str>,
) -> Result<(DatabaseIdInterceptor, Option<String>)> {
    let mut db = connect_database(sock).await?;
    let id = match selection {
        Some(sel) => {
            Some(commands::database::resolve_database_id_by_selection(&mut db, sel).await?)
        }
        // Every data-plane command reaches this, including ones that never
        // mention the registry. Re-frame a registry-side failure in the
        // caller's terms rather than surfacing a bare "List RPC failed" from
        // `nodespace search`.
        None => commands::database::resolve_default_database_id(&mut db)
            .await
            .context("could not determine which database to use")?,
    };
    match id {
        Some(id) => {
            let interceptor = DatabaseIdInterceptor::for_id(&id)?;
            Ok((interceptor, Some(id)))
        }
        None => Ok((DatabaseIdInterceptor::none(), None)),
    }
}

/// Top-level dispatch — wired by `main.rs` and reused by integration tests.
///
/// A command routed to a database that requires an extension this build does
/// not support fails with the daemon's refusal (ADR-083 §2); its error is
/// rewritten to the shared refusal text by [`render_refusal`]. `main` prints
/// the error and exits non-zero either way.
pub async fn run(cli: Cli) -> Result<()> {
    dispatch(cli).await.map_err(render_refusal)
}

/// Replace an error that carries the daemon's required-extensions refusal
/// with the refusal message and the download link, rendered by the shared
/// display-name module so the CLI says exactly what the app says. Any other
/// error passes through unchanged.
pub fn render_refusal(err: anyhow::Error) -> anyhow::Error {
    let refusal = err
        .chain()
        .filter_map(|cause| cause.downcast_ref::<tonic::Status>())
        .find_map(nodespace_proto::requires_extension::unsupported_extensions);
    match refusal {
        Some(unsupported) => anyhow::anyhow!(refusal_text(&unsupported)),
        None => err,
    }
}

/// The refusal as the CLI prints it: the message, then the download link.
pub fn refusal_text(unsupported: &[String]) -> String {
    use nodespace_proto::extension_names::{refusal_message, DOWNLOAD_LABEL, DOWNLOAD_URL};
    format!(
        "{}\n{DOWNLOAD_LABEL}: {DOWNLOAD_URL}",
        refusal_message(unsupported)
    )
}

/// The command dispatch behind [`run`].
///
/// `sock` names the daemon endpoint for whichever transport this platform
/// uses — a Unix Domain Socket path on macOS/Linux, a Named Pipe name
/// (wrapped as a `Path` so every downstream `connect*` helper stays
/// platform-agnostic) on Windows. Everything below this resolution is shared.
async fn dispatch(cli: Cli) -> Result<()> {
    #[cfg(unix)]
    let sock = resolve_socket_path(cli.socket.as_deref());
    #[cfg(windows)]
    let sock = std::path::PathBuf::from(resolve_pipe_name(cli.socket.as_deref()));

    let json = cli.json;
    let selection = cli.database.as_deref();

    match cli.command {
        Command::Node { mut action } => {
            if let commands::node::NodeAction::Delete(args) = &mut action {
                let flags = [("--socket", &cli.socket), ("--database", &cli.database)];
                args.routing = flags
                    .into_iter()
                    .filter_map(|(flag, value)| Some([flag.to_string(), value.clone()?]))
                    .flatten()
                    .collect();
            }
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
        // No daemon connection: the log is a local file, and the daemon being
        // down is exactly when it is most worth reading.
        Command::Logs(args) => commands::logs::run(args, json),
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
        // fetches skills and schemas from the graph through
        // `NodeService.GetSkillGuidance`, and `reset` calls
        // `NodeService.ResetSeedNode` -- both need a routed `NodeClient`
        // here.
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

#[cfg(test)]
mod refusal_tests {
    use super::{refusal_text, render_refusal};
    use anyhow::Context;

    fn failed(status: tonic::Status) -> anyhow::Error {
        Err::<(), _>(status)
            .context("GetNode RPC failed")
            .unwrap_err()
    }

    /// The daemon's refusal, found under the context a command adds, becomes
    /// the shared refusal text with the download link and nothing else.
    #[test]
    fn a_refusal_anywhere_in_the_chain_becomes_the_refusal_text() {
        let unsupported = vec!["fixture".to_string()];
        let err = render_refusal(failed(nodespace_proto::requires_extension::status(
            &unsupported,
        )));

        assert_eq!(format!("{err:?}"), refusal_text(&unsupported));
        assert_eq!(
            refusal_text(&unsupported),
            format!(
                "{}\n{}: {}",
                nodespace_proto::extension_names::refusal_message(&unsupported),
                nodespace_proto::extension_names::DOWNLOAD_LABEL,
                nodespace_proto::extension_names::DOWNLOAD_URL
            )
        );
    }

    /// Any other error, including another FAILED_PRECONDITION, is left as it
    /// was.
    #[test]
    fn any_other_error_passes_through() {
        let err = render_refusal(failed(tonic::Status::failed_precondition("a rule refused")));
        assert!(format!("{err:#}").contains("a rule refused"), "{err:#}");
        assert!(
            format!("{err:#}").starts_with("GetNode RPC failed"),
            "{err:#}"
        );
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::discover_socket_in;

    /// With no socket set, the CLI dials the canonical `daemon.sock` when it
    /// exists, otherwise auto-discovers a running dev daemon, and falls back to
    /// the canonical path (for a clean error) when neither exists.
    #[test]
    fn discover_socket_prefers_canonical_then_falls_back_to_dev() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // Nothing running → canonical default (so the connect error is clean).
        assert_eq!(discover_socket_in(dir), dir.join("daemon.sock"));

        // Only a dev daemon is up → discover it without NODESPACED_SOCKET.
        std::fs::write(dir.join("daemon-dev.sock"), b"").unwrap();
        assert_eq!(discover_socket_in(dir), dir.join("daemon-dev.sock"));

        // Canonical present → always preferred over the dev socket.
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
    /// `desktop-app/app-lib/src/services/grpc_client.rs`'s `windows_tests`.
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

#[cfg(test)]
mod connect_failure_tests {
    use super::*;

    #[test]
    fn classify_connect_error_distinguishes_busy_from_absent() {
        const ERROR_FILE_NOT_FOUND: i32 = 2;
        // Mirror tonic's real shape: a wrapper error whose `source()` is the io error.
        #[derive(Debug)]
        struct Transport(std::io::Error);
        impl std::fmt::Display for Transport {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "transport error")
            }
        }
        impl std::error::Error for Transport {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let busy = anyhow::Error::new(Transport(std::io::Error::from_raw_os_error(
            ERROR_PIPE_BUSY,
        )));
        assert_eq!(classify_connect_error(&busy), ConnectFailure::DaemonBusy);

        let absent = anyhow::Error::new(std::io::Error::from_raw_os_error(ERROR_FILE_NOT_FOUND));
        assert_eq!(classify_connect_error(&absent), ConnectFailure::NotRunning);

        let other = anyhow::anyhow!("boom");
        assert_eq!(classify_connect_error(&other), ConnectFailure::NotRunning);
    }

    #[test]
    fn connect_failure_message_matches_classification() {
        let sock = std::path::Path::new("pipe-x");
        let busy = connect_failure(
            sock,
            anyhow::Error::new(std::io::Error::from_raw_os_error(ERROR_PIPE_BUSY)),
        );
        let msg = format!("{busy}");
        assert!(msg.contains("busy") && !msg.contains("Is the daemon running"));

        let gone = connect_failure(sock, anyhow::anyhow!("nope"));
        assert!(format!("{gone}").contains("Is the daemon running"));
    }

    #[tokio::test(start_paused = true)]
    async fn busy_then_available_retries_and_succeeds() {
        let mut calls = 0;
        let out = open_with_busy_retry(
            || {
                calls += 1;
                if calls < 4 {
                    Err(std::io::Error::from_raw_os_error(ERROR_PIPE_BUSY))
                } else {
                    Ok(calls)
                }
            },
            PIPE_BUSY_RETRIES,
            PIPE_BUSY_DELAY,
        )
        .await;
        assert_eq!(out.unwrap(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn persistent_busy_gives_up_after_bounded_retries() {
        let mut calls = 0;
        let out: std::io::Result<()> = open_with_busy_retry(
            || {
                calls += 1;
                Err(std::io::Error::from_raw_os_error(ERROR_PIPE_BUSY))
            },
            3,
            PIPE_BUSY_DELAY,
        )
        .await;
        assert!(is_pipe_busy(out.unwrap_err().raw_os_error()));
        assert_eq!(calls, 4, "1 initial attempt + 3 retries");
    }

    #[tokio::test(start_paused = true)]
    async fn absent_pipe_fails_immediately_without_retry_or_delay() {
        let mut calls = 0;
        let start = tokio::time::Instant::now();
        let out: std::io::Result<()> = open_with_busy_retry(
            || {
                calls += 1;
                Err(std::io::Error::from_raw_os_error(2))
            },
            PIPE_BUSY_RETRIES,
            PIPE_BUSY_DELAY,
        )
        .await;
        assert!(out.is_err());
        assert_eq!(calls, 1);
        assert_eq!(start.elapsed(), std::time::Duration::ZERO);
    }
}
