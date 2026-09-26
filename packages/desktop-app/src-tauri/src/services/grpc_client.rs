//! gRPC client that connects to the external `nodespaced` daemon over a Unix
//! Domain Socket.
//!
//! Socket path resolution order:
//!   1. `NODESPACED_SOCKET` environment variable
//!   2. Build-variant-scoped default (see daemon_setup::daemon_socket_relative)
//!
//! The `GrpcClient` is registered as Tauri managed state once and cloned
//! cheaply per command (tonic `Channel` is an `Arc` internally).

use std::sync::Arc;

use nodespace_proto::{
    with_message_limits, AgentSessionServiceClient, DatabaseServiceClient, EmbeddingsServiceClient,
    ImportServiceClient, LocalAgentServiceClient, NodeServiceClient, SettingsServiceClient,
    CLIENT_ID_HEADER, DATABASE_ID_HEADER,
};
use tokio::sync::{watch, RwLock};
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::interceptor::InterceptedService;
use tonic::service::Interceptor;
use tonic::transport::Channel;

/// Stamps the ADR-053 `x-ns-database-id` routing header and the ADR-026 C5
/// extension's `x-ns-client-id` identity header on every outgoing request.
///
/// Concrete (not a closure) so the intercepted client types stay nameable and
/// aliasable — see [`NodeClient`] and friends. `database_id: None` stamps
/// nothing, letting the daemon fall back to its default database; the variant
/// is applied uniformly so a routed client's type is the same whether or not a
/// database is currently selected. Same-named struct as the CLI's interceptor
/// (`packages/cli/src/lib.rs`) but an independent implementation in a
/// different crate — the CLI's copy deliberately does not stamp
/// `x-ns-client-id`; see its doc comment for why.
///
/// `client_id` is always `Some` in production — generated once per
/// `GrpcClient` (i.e. once per app process/window) in [`GrpcClient::from_channel`]
/// and carried through every interceptor rebuild (database switches rebuild
/// the routed clients but must keep the SAME client id, or the daemon would
/// stop recognizing this window's own writes on echo suppression after every
/// switch). Stamping it lets the daemon scope this connection's writes via
/// `NodeService::with_client()` and drop their echo on this connection's own
/// `WatchNodes` stream (see `watcher.rs`).
#[derive(Clone)]
pub struct DatabaseIdInterceptor {
    // Some(id) → stamp header on every request; None → stamp nothing (daemon
    // uses its default database).
    database_id: Option<MetadataValue<Ascii>>,
    client_id: MetadataValue<Ascii>,
}

impl DatabaseIdInterceptor {
    /// No routing header — the daemon serves its default database. Still
    /// stamps `client_id` so writes made before any database is selected are
    /// still attributable.
    pub fn none(client_id: MetadataValue<Ascii>) -> Self {
        Self {
            database_id: None,
            client_id,
        }
    }

    /// Stamp `x-ns-database-id: <id>` on every request when `id` is `Some`,
    /// and `x-ns-client-id: <client_id>` unconditionally.
    /// `id` must be an already resolved registry identifier (ULID); the daemon
    /// resolves the header as an id only, never a name. An id that is not a
    /// valid gRPC header value falls back to no routing (default database)
    /// rather than poisoning every subsequent request.
    pub fn for_id(id: Option<&str>, client_id: MetadataValue<Ascii>) -> Self {
        let database_id = id.and_then(|id| match MetadataValue::try_from(id) {
            Ok(value) => Some(value),
            Err(_) => {
                // A resolved ULID is always valid ASCII, so this should be
                // unreachable — but log it rather than silently serving the
                // default database, which would be a hard-to-spot wrong-database
                // fallback.
                tracing::warn!(
                    "database id {id:?} is not a valid routing header; \
                     falling back to the default database"
                );
                None
            }
        });
        Self {
            database_id,
            client_id,
        }
    }
}

impl Interceptor for DatabaseIdInterceptor {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        if let Some(id) = &self.database_id {
            req.metadata_mut().insert(DATABASE_ID_HEADER, id.clone());
        }
        req.metadata_mut()
            .insert(CLIENT_ID_HEADER, self.client_id.clone());
        Ok(req)
    }
}

/// Generate a fresh client id for this `GrpcClient` (one per app
/// process/window, per ADR-026's C5 extension). A UUID is always valid ASCII, so this
/// cannot fail in practice.
fn generate_client_id() -> MetadataValue<Ascii> {
    MetadataValue::try_from(uuid::Uuid::new_v4().to_string())
        .expect("uuid v4 string is always a valid ascii metadata value")
}

/// A channel wrapped so every request carries the database routing header.
pub type Intercepted = InterceptedService<Channel, DatabaseIdInterceptor>;
/// `NodeService` client bound to the active database (ADR-053).
pub type NodeClient = NodeServiceClient<Intercepted>;
/// `ImportService` client bound to the active database.
pub type ImportClient = ImportServiceClient<Intercepted>;
/// `EmbeddingsService` client bound to the active database.
pub type EmbeddingsClient = EmbeddingsServiceClient<Intercepted>;
/// `AgentSessionService` client bound to the active database.
pub type AgentSessionClient = AgentSessionServiceClient<Intercepted>;
/// `LocalAgentService` client bound to the active database (ADR-053).
pub type LocalAgentClient = LocalAgentServiceClient<Intercepted>;

struct GrpcClientInner {
    // Routed data-plane clients — rebuilt with a fresh interceptor whenever the
    // active database changes so their requests carry the routing header. The
    // daemon routes node/import/embeddings/agent_session/local_agent by
    // `x-ns-database-id`.
    node: NodeClient,
    import: ImportClient,
    embeddings: EmbeddingsClient,
    agent_session: AgentSessionClient,
    local_agent: LocalAgentClient,
    // Unrouted clients — registry-global (database_service) or daemon-global
    // (settings); they never carry the routing header.
    settings: SettingsServiceClient<Channel>,
    database_service: DatabaseServiceClient<Channel>,
    /// The desktop-local "which database am I viewing" selection. `None` = the
    /// daemon's default database. Distinct from the daemon-wide default set via
    /// `DatabaseService::SetDefault`.
    active_database_id: Option<String>,
    /// This process/window's stable identity (ADR-026's C5 extension), generated once in
    /// [`GrpcClient::from_channel`] and re-stamped by every interceptor rebuild
    /// in [`GrpcClient::set_active_database`] — it must never change for the
    /// lifetime of this `GrpcClient`, or the daemon would stop recognizing this
    /// window's own writes on its `WatchNodes` echo-suppression check after a
    /// database switch.
    client_id: MetadataValue<Ascii>,
    /// Underlying transport channel — held so Pro-tier services can
    /// ride the same h2 connection via `GrpcClient::channel()`. One
    /// channel, multiple service surfaces. Opening a parallel channel
    /// caused "Service was not ready: transport error" during the
    /// PoC when ProClient's separately-built channel got into a bad
    /// state after the probe stream was dropped.
    channel: Channel,
}

/// Managed Tauri state wrapping the gRPC clients connected to `nodespaced`.
///
/// `Channel` is cheap to clone (it is an `Arc` internally). Commands clone
/// clients per call since tonic's generated methods take `&mut self`.
#[derive(Clone)]
pub struct GrpcClient {
    inner: Arc<RwLock<GrpcClientInner>>,
    /// Bumped by [`set_active_database`] on every switch so the node-event
    /// watcher can re-open its `WatchNodes` stream against the newly-active
    /// database. `watch::Sender` is not `Clone`, hence the `Arc`.
    db_generation: Arc<watch::Sender<u64>>,
}

impl GrpcClient {
    /// Connect to the `nodespaced` daemon over a Unix Domain Socket and return
    /// a fully-initialised client bundle.
    ///
    /// Returns an error if the socket cannot be reached. The Tauri app should
    /// treat this as a fatal startup error (daemon not running).
    #[cfg(unix)]
    pub async fn connect() -> Result<Self, GrpcClientError> {
        let sock = resolve_socket_path();
        tracing::info!(socket = %sock.display(), "Connecting to nodespaced");

        let channel = uds_channel(&sock).await.map_err(GrpcClientError::Connect)?;

        tracing::info!(socket = %sock.display(), "Connected to nodespaced");

        Ok(Self::from_channel(channel))
    }

    /// Connect to the `nodespaced` daemon over a Named Pipe and return
    /// a fully-initialised client bundle.
    #[cfg(windows)]
    pub async fn connect() -> Result<Self, GrpcClientError> {
        let pipe = resolve_pipe_name();
        tracing::info!(pipe = %pipe, "Connecting to nodespaced (Named Pipe)");
        let channel = pipe_channel(&pipe)
            .await
            .map_err(GrpcClientError::Connect)?;
        tracing::info!(pipe = %pipe, "Connected to nodespaced");
        Ok(Self::from_channel(channel))
    }

    /// Wrap an established (or lazy) channel in the full service-client bundle.
    /// Shared by [`connect`] and [`connect_lazy`] so the set of service clients
    /// stays in sync as new services are added. `Channel` is platform-agnostic.
    fn from_channel(channel: Channel) -> Self {
        // One stable id for this GrpcClient's whole lifetime (ADR-026's C5 extension) —
        // generated once here, never regenerated on a database switch.
        let client_id = generate_client_id();
        // No database selected initially → the routed clients carry an empty
        // interceptor and requests fall back to the daemon's default database,
        // exactly as before ADR-053 client routing existed. The client-id
        // header is still stamped.
        let interceptor = DatabaseIdInterceptor::none(client_id.clone());
        let inner = GrpcClientInner {
            node: with_message_limits!(NodeServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone()
            )),
            import: with_message_limits!(ImportServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone()
            )),
            embeddings: with_message_limits!(EmbeddingsServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone(),
            )),
            agent_session: with_message_limits!(AgentSessionServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone(),
            )),
            local_agent: with_message_limits!(LocalAgentServiceClient::with_interceptor(
                channel.clone(),
                interceptor
            )),
            settings: with_message_limits!(SettingsServiceClient::new(channel.clone())),
            database_service: with_message_limits!(DatabaseServiceClient::new(channel.clone())),
            active_database_id: None,
            client_id,
            channel,
        };
        let (db_generation, _) = watch::channel(0u64);
        Self {
            inner: Arc::new(RwLock::new(inner)),
            db_generation: Arc::new(db_generation),
        }
    }

    /// Build a client over a LAZY UDS channel — it connects on the first RPC
    /// rather than now, so this returns synchronously and can be `.manage()`'d at
    /// app setup BEFORE the daemon socket is reachable. This removes the startup
    /// race where the frontend invoked a command (e.g. `get_children_tree`) before
    /// the async `connect()` had run, getting a fatal "state not managed for field
    /// `client`" that closed the view. With a lazy client managed up front, an
    /// early call instead waits / yields a retryable transport error.
    #[cfg(unix)]
    pub fn connect_lazy() -> Self {
        let sock = resolve_socket_path();
        tracing::info!(socket = %sock.display(), "gRPC client (lazy) — connects on first use");
        let channel = uds_channel_lazy(&sock);
        Self::from_channel(channel)
    }

    /// Lazy Named Pipe variant for Windows — connects on the first RPC.
    #[cfg(windows)]
    pub fn connect_lazy() -> Self {
        let pipe = resolve_pipe_name();
        tracing::info!(pipe = %pipe, "gRPC client (lazy) — connects on first use");
        let channel = pipe_channel_lazy(&pipe);
        Self::from_channel(channel)
    }

    /// Borrow a clone of the routed `NodeService` client (carries the active
    /// database's `x-ns-database-id` header).
    pub async fn client(&self) -> NodeClient {
        self.inner.read().await.node.clone()
    }

    /// Borrow a clone of the routed `ImportService` client, paired with the
    /// database id its routing header names (`None` = the daemon's default).
    /// Both are read under one lock so a concurrent database switch can never
    /// pair a client with another database's id — the import commands route
    /// their progress events by this id.
    pub async fn import_client(&self) -> (ImportClient, Option<String>) {
        let inner = self.inner.read().await;
        (inner.import.clone(), inner.active_database_id.clone())
    }

    /// Borrow a clone of the `SettingsServiceClient`. Settings are daemon-global
    /// and never routed by database.
    pub async fn settings_client(&self) -> SettingsServiceClient<Channel> {
        self.inner.read().await.settings.clone()
    }

    /// Borrow a clone of the routed `EmbeddingsService` client.
    ///
    /// Embeddings are always available in the daemon (unlike the old in-process
    /// optional configuration), so this returns the client directly.
    pub async fn embeddings_client(&self) -> EmbeddingsClient {
        self.inner.read().await.embeddings.clone()
    }

    /// Borrow a clone of the routed `AgentSessionService` client.
    pub async fn agent_session_client(&self) -> AgentSessionClient {
        self.inner.read().await.agent_session.clone()
    }

    /// Borrow a clone of the routed `LocalAgentService` client (carries the
    /// active database header).
    ///
    /// The loaded inference model itself is daemon-global, but a chat turn is
    /// not: the turn's cancellation token, its live token stream, and its
    /// busy/idle status all belong to the database whose ai-chat node is being
    /// generated into. Without the header those calls reach whichever database
    /// the daemon happened to boot with, so on any other database cancelling
    /// silently does nothing, no tokens ever arrive, and status reports the
    /// wrong database's activity.
    pub async fn local_agent_client(&self) -> LocalAgentClient {
        self.inner.read().await.local_agent.clone()
    }

    /// Borrow a clone of the `DatabaseServiceClient`. The registry is global to
    /// the daemon and is never routed by database.
    pub async fn database_service_client(&self) -> DatabaseServiceClient<Channel> {
        self.inner.read().await.database_service.clone()
    }

    /// Select which local database the routed data-plane clients target
    /// (ADR-053). `Some(id)` stamps `x-ns-database-id: <id>` on every
    /// node/import/embeddings/agent_session/local_agent request; `None` clears
    /// the header so requests route to the daemon's default database.
    ///
    /// Only the routed clients are rebuilt — over the SAME channel, so no
    /// reconnection happens. `settings`/`database_service` stay unrouted. Bumps
    /// a generation counter so the node-event watcher re-opens its `WatchNodes`
    /// stream against the newly-active database.
    pub async fn set_active_database(&self, id: Option<String>) {
        {
            let mut inner = self.inner.write().await;
            if inner.active_database_id == id {
                return;
            }
            let interceptor = DatabaseIdInterceptor::for_id(id.as_deref(), inner.client_id.clone());
            let channel = inner.channel.clone();
            inner.node = with_message_limits!(NodeServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone()
            ));
            inner.import = with_message_limits!(ImportServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone()
            ));
            inner.embeddings = with_message_limits!(EmbeddingsServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone()
            ));
            inner.agent_session = with_message_limits!(
                AgentSessionServiceClient::with_interceptor(channel.clone(), interceptor.clone())
            );
            inner.local_agent = with_message_limits!(LocalAgentServiceClient::with_interceptor(
                channel,
                interceptor
            ));
            inner.active_database_id = id;
        }
        // Signal the watcher (outside the lock) to re-subscribe to the new
        // database's event stream.
        self.db_generation.send_modify(|g| *g = g.wrapping_add(1));
    }

    /// Subscribe to active-database switches. The value is an opaque generation
    /// counter bumped on every [`set_active_database`]; the node-event watcher
    /// awaits changes to re-open its `WatchNodes` stream against the new
    /// database.
    pub fn subscribe_active_database(&self) -> watch::Receiver<u64> {
        self.db_generation.subscribe()
    }

    /// Clone of the underlying `tonic::transport::Channel`. Used by
    /// `ProClient` so the Pro-tier service rides the same h2
    /// connection (one channel, multiple service surfaces).
    pub async fn channel(&self) -> Channel {
        self.inner.read().await.channel.clone()
    }

    /// Rebuild the underlying lazy channel and every service client from
    /// scratch, preserving the active-database routing — what a full app
    /// restart does, but in place.
    ///
    /// Recovers a **wedged** h2 connection: the daemon can be healthy (it
    /// answers a freshly-dialed client) while this long-lived channel is stuck,
    /// so `get_node`/writes and the WatchNodes stream hang indefinitely (the
    /// lazy channel has no client-side timeout). A stream churn during a heavy
    /// sync can put the single shared connection into that state. Replacing the
    /// channel gives every surface a clean connection on its next call; bumping
    /// `db_generation` makes the node-event watcher re-open its stream on it.
    #[cfg(unix)]
    pub async fn reconnect(&self) {
        let sock = resolve_socket_path();
        let channel = uds_channel_lazy(&sock);
        self.swap_channel(channel).await;
        tracing::info!(socket = %sock.display(), "gRPC client: channel rebuilt (reconnect)");
    }

    /// Named-pipe variant of [`reconnect`].
    #[cfg(windows)]
    pub async fn reconnect(&self) {
        let pipe = resolve_pipe_name();
        let channel = pipe_channel_lazy(&pipe);
        self.swap_channel(channel).await;
        tracing::info!(pipe = %pipe, "gRPC client: channel rebuilt (reconnect)");
    }

    /// Replace the channel behind every service client, keeping the current
    /// `x-ns-database-id` routing, then signal the watcher to re-subscribe.
    async fn swap_channel(&self, channel: Channel) {
        {
            let mut inner = self.inner.write().await;
            // Preserve this window's client id across the rebuild — the daemon
            // scopes echo suppression by it, so a fresh id would make our own
            // writes re-appear as remote changes on the new WatchNodes stream.
            let interceptor = DatabaseIdInterceptor::for_id(
                inner.active_database_id.as_deref(),
                inner.client_id.clone(),
            );
            inner.node = with_message_limits!(NodeServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone()
            ));
            inner.import = with_message_limits!(ImportServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone()
            ));
            inner.embeddings = with_message_limits!(EmbeddingsServiceClient::with_interceptor(
                channel.clone(),
                interceptor.clone()
            ));
            inner.agent_session = with_message_limits!(
                AgentSessionServiceClient::with_interceptor(channel.clone(), interceptor.clone())
            );
            inner.local_agent = with_message_limits!(LocalAgentServiceClient::with_interceptor(
                channel.clone(),
                interceptor
            ));
            inner.settings = with_message_limits!(SettingsServiceClient::new(channel.clone()));
            inner.database_service =
                with_message_limits!(DatabaseServiceClient::new(channel.clone()));
            inner.channel = channel;
        }
        self.db_generation.send_modify(|g| *g = g.wrapping_add(1));
    }
}

/// Resolve the daemon socket path.
///
/// Checks `NODESPACED_SOCKET` env var first, then falls back to the build-variant-
/// scoped default from `daemon_setup::daemon_socket_relative()`. `pub(crate)` so
/// the daemon-reachability checks in `lib.rs` probe the SAME socket the client
/// actually dials — otherwise a `NODESPACED_SOCKET` override (two-window demo,
/// custom setups) makes them check the wrong path and falsely report "not running".
#[cfg(unix)]
pub(crate) fn resolve_socket_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var(nodespace_proto::socket::SOCKET_ENV_VAR) {
        return std::path::PathBuf::from(p);
    }
    default_socket_path_for(cfg!(debug_assertions), crate::daemon_setup::is_pro_build())
}

/// The socket [`resolve_socket_path`] falls back to when `NODESPACED_SOCKET` is
/// absent, for an arbitrary build variant rather than this binary's own.
///
/// Two things are deliberate here. It takes the variant as parameters because a
/// compiled app is only ever one variant, so this is the only way an ordinary
/// `#[test]` can check that the app dials, for every variant, the socket the
/// daemon binds. And it reads no environment at all: `cargo test` runs the whole
/// binary in one process on a thread pool, so a test of an env-reading resolver
/// races every other test that touches the same variable. Keeping the override
/// in the caller leaves this half deterministic and testable, and leaves the
/// override itself covered by a single test that owns the variable.
#[cfg(unix)]
pub(crate) fn default_socket_path_for(is_debug: bool, is_pro: bool) -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(nodespace_proto::socket::daemon_socket_relative(
            is_debug, is_pro,
        ))
}

/// Resolve the Named Pipe name used on Windows.
///
/// Checks `NODESPACED_SOCKET` env var first (mirrors Unix override convention),
/// then falls back to `\\.\pipe\nodespace-daemon`.
#[cfg(windows)]
pub(crate) fn resolve_pipe_name() -> String {
    if let Ok(p) = std::env::var(nodespace_proto::socket::SOCKET_ENV_VAR) {
        return p;
    }
    nodespace_proto::socket::DAEMON_PIPE_NAME.to_string()
}

/// On Windows, return the pipe name as a `PathBuf` so callers that take a `Path`
/// (e.g. `check_daemon_socket`) work without platform-specific call sites.
#[cfg(windows)]
pub(crate) fn resolve_socket_path() -> std::path::PathBuf {
    std::path::PathBuf::from(resolve_pipe_name())
}

/// Build a tonic `Channel` connected over a Unix Domain Socket.
#[cfg(unix)]
async fn uds_channel(sock: &std::path::Path) -> Result<Channel, tonic::transport::Error> {
    use hyper_util::rt::TokioIo;
    use tokio::net::UnixStream;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let sock = sock.to_path_buf();
    // The URI host is ignored for UDS — tonic needs a syntactically valid URI.
    Endpoint::from_static("http://localhost")
        .connect_with_connector(service_fn(move |_: Uri| {
            let sock = sock.clone();
            async move { UnixStream::connect(&sock).await.map(TokioIo::new) }
        }))
        .await
}

/// Lazy variant of [`uds_channel`] — builds the channel without connecting; the
/// first RPC establishes (and later re-establishes) the UDS connection.
#[cfg(unix)]
fn uds_channel_lazy(sock: &std::path::Path) -> Channel {
    use hyper_util::rt::TokioIo;
    use tokio::net::UnixStream;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let sock = sock.to_path_buf();
    Endpoint::from_static("http://localhost").connect_with_connector_lazy(service_fn(
        move |_: Uri| {
            let sock = sock.clone();
            async move { UnixStream::connect(&sock).await.map(TokioIo::new) }
        },
    ))
}

/// Build a tonic `Channel` connected over a Named Pipe (Windows).
#[cfg(windows)]
async fn pipe_channel(pipe: &str) -> Result<Channel, tonic::transport::Error> {
    use hyper_util::rt::TokioIo;
    use tokio::net::windows::named_pipe::ClientOptions;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let pipe = pipe.to_string();
    Endpoint::from_static("http://localhost")
        .connect_with_connector(service_fn(move |_: Uri| {
            let pipe = pipe.clone();
            async move { ClientOptions::new().open(&pipe).map(TokioIo::new) }
        }))
        .await
}

/// Lazy variant of [`pipe_channel`] — builds the channel without connecting.
#[cfg(windows)]
fn pipe_channel_lazy(pipe: &str) -> Channel {
    use hyper_util::rt::TokioIo;
    use tokio::net::windows::named_pipe::ClientOptions;
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    let pipe = pipe.to_string();
    Endpoint::from_static("http://localhost").connect_with_connector_lazy(service_fn(
        move |_: Uri| {
            let pipe = pipe.clone();
            async move { ClientOptions::new().open(&pipe).map(TokioIo::new) }
        },
    ))
}

#[derive(Debug, thiserror::Error)]
pub enum GrpcClientError {
    #[error("Failed to connect to nodespaced: {0}")]
    Connect(tonic::transport::Error),
}

#[cfg(all(test, unix))]
mod tests {
    use super::resolve_socket_path;
    use std::sync::Mutex;

    /// `NODESPACED_SOCKET` is process-global state, and this binary's tests run
    /// in parallel, so serialize anything that touches it. Only one test here
    /// does today — its sibling deliberately reads no environment at all — but
    /// an unguarded mutation is a trap for whichever env-reading test lands
    /// next, and the guard costs nothing. Mirrors `tests.rs`'s `ENV_LOCK`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Recover from a poisoned lock rather than cascading: the guarded block is
    /// plain env read/write assertions, so an earlier panic holding the lock
    /// shouldn't fail every later test too.
    fn env_lock_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Regression for the false "background service not running" banner: the
    /// daemon-reachability checks must resolve the SAME socket the gRPC client
    /// dials, i.e. honor `NODESPACED_SOCKET` over the default.
    #[test]
    fn resolve_socket_path_honors_env_override_then_falls_back() {
        let _guard = env_lock_guard();
        let prev = std::env::var_os("NODESPACED_SOCKET");

        std::env::set_var("NODESPACED_SOCKET", "/tmp/ns-demo-a.sock");
        assert_eq!(
            resolve_socket_path(),
            std::path::PathBuf::from("/tmp/ns-demo-a.sock"),
            "NODESPACED_SOCKET override must win"
        );

        std::env::remove_var("NODESPACED_SOCKET");
        let default_path = resolve_socket_path();
        assert!(
            default_path.starts_with(dirs::home_dir().unwrap()),
            "with no override, fall back to a path under HOME"
        );
        assert!(
            default_path.to_string_lossy().contains(".nodespace/daemon"),
            "with no override, fall back to a .nodespace/daemon* socket"
        );

        // Restore prior state so we don't leak into other tests.
        match prev {
            Some(v) => std::env::set_var("NODESPACED_SOCKET", v),
            None => std::env::remove_var("NODESPACED_SOCKET"),
        }
    }

    /// The app half of the app/daemon agreement check.
    ///
    /// These four strings are pinned literally here AND, identically, in the
    /// daemon's `socket_fallback_variant_tests`. That duplication is the point:
    /// both sides now derive their default from one shared table, so a test that
    /// re-derived the expectation from that same table would still pass if the
    /// table itself were wrong. Pinning the values on each side independently
    /// means the two can only agree by actually being right.
    ///
    /// The failure this guards against is asymmetric and near-invisible: only a
    /// Pro or dev build is affected, since release-community is the one variant
    /// whose scoped and unscoped names coincide.
    ///
    /// Reads no environment, so it cannot race the sibling test above (which
    /// takes `ENV_LOCK` while it owns `NODESPACED_SOCKET`) — see
    /// `default_socket_path_for`'s doc comment.
    #[test]
    fn each_variant_dials_the_socket_its_daemon_binds() {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        for (is_debug, is_pro, expected) in [
            (false, false, ".nodespace/daemon.sock"),
            (false, true, ".nodespace/daemon-pro.sock"),
            (true, false, ".nodespace/daemon-dev.sock"),
            (true, true, ".nodespace/daemon-dev-pro.sock"),
        ] {
            assert_eq!(
                super::default_socket_path_for(is_debug, is_pro),
                home.join(expected),
                "variant (debug={is_debug}, pro={is_pro}) must dial {expected}"
            );
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::{resolve_pipe_name, resolve_socket_path};
    use std::sync::Mutex;

    /// Both tests below mutate `NODESPACED_SOCKET`, which is process-global, so
    /// serialize them — unlike the unix module's pair, these two genuinely race
    /// each other at the `--test-threads=2` this binary now runs under. Same
    /// shape as the unix `ENV_LOCK` above and `tests.rs`'s.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Recover from a poisoned lock rather than cascading: the guarded blocks
    /// are plain env read/write assertions, so an earlier panic holding the
    /// lock shouldn't fail every later test too.
    fn env_lock_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn resolve_pipe_name_honors_env_override_then_falls_back() {
        let _guard = env_lock_guard();
        let prev = std::env::var_os("NODESPACED_SOCKET");

        std::env::set_var("NODESPACED_SOCKET", r"\\.\pipe\ns-test");
        assert_eq!(
            resolve_pipe_name(),
            r"\\.\pipe\ns-test",
            "NODESPACED_SOCKET override must win"
        );

        std::env::remove_var("NODESPACED_SOCKET");
        assert_eq!(
            resolve_pipe_name(),
            r"\\.\pipe\nodespace-daemon",
            "default pipe name must be correct"
        );

        match prev {
            Some(v) => std::env::set_var("NODESPACED_SOCKET", v),
            None => std::env::remove_var("NODESPACED_SOCKET"),
        }
    }

    #[test]
    fn resolve_socket_path_delegates_to_pipe_name() {
        let _guard = env_lock_guard();
        let prev = std::env::var_os("NODESPACED_SOCKET");
        std::env::remove_var("NODESPACED_SOCKET");

        let path = resolve_socket_path();
        assert_eq!(
            path.to_string_lossy().as_ref(),
            r"\\.\pipe\nodespace-daemon"
        );

        match prev {
            Some(v) => std::env::set_var("NODESPACED_SOCKET", v),
            None => std::env::remove_var("NODESPACED_SOCKET"),
        }
    }
}
