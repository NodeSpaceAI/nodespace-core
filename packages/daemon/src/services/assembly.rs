//! Daemon service assembly (ADR-053: one daemon, multiple local databases).
//!
//! Splits startup into the process-global [`SharedServices`] (built once) and
//! the per-database [`DatabaseServices`] (one per open database). Lives in the
//! library so the [`crate::services::database_manager::DatabaseManager`] can
//! build and cache per-database service sets, and the `nodespaced` binary just
//! calls these entry points.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use nodespace_agent::agent_catalog::context_assembly::GraphContextAssembler;
use nodespace_agent::prompt_assembler::PromptAssembler;
use nodespace_agent::pty::PtySessionManager;
use nodespace_agent::skill_pipeline::{seed_skill_nodes, seed_tool_nodes};
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::services::node_service::access_gate::SubtreeAccessGate;
use nodespace_core::services::{
    EmbeddingProcessor, EmbeddingScheduler, NodeAccessor, NodeEmbeddingService,
};
use nodespace_core::{NodeService as CoreNodeService, PlaybookEngine, SqliteStore};
use nodespace_nlp_engine::EmbeddingService;
use tokio::sync::{watch, RwLock};

use super::{
    AgentSessionHandler, EmbeddingReady, EmbeddingsServiceImpl, ImportServiceImpl,
    LocalAgentServiceImpl, NodeServiceImpl, SettingsServiceImpl, SharedLocalAgent,
};

/// The process-global build context every per-database service set needs
/// (ADR-053: one daemon, multiple local databases): the shared PTY manager and
/// the single embedding model. Cloneable and cheap to hold, so the
/// [`crate::services::database_manager::DatabaseManager`] keeps a copy and
/// builds databases on demand via [`build_database_services`].
#[derive(Clone)]
pub struct SharedContext {
    /// PTY sessions are process-global — one manager backs all databases.
    pub pty_manager: Arc<PtySessionManager>,
    /// The embedding model, loaded once for the whole process and published over
    /// a watch channel so each database's embedding wiring can await it. Holds
    /// `None` until the background load completes; a closed channel means the
    /// load failed or no model file exists.
    pub model: watch::Receiver<Option<Arc<EmbeddingService>>>,
    /// Whether an NLP model file was found at startup. Gates both the
    /// per-database embedding wiring and the `EmbeddingsService` registration.
    pub has_model: bool,
    /// Set once, permanently, by `load_shared_embedding_model_bg` if the
    /// background load fails (corrupt file, engine init error). A closed
    /// `model` channel alone is ambiguous -- it also looks that way while
    /// still loading, since nothing has been sent on it yet -- so
    /// `EmbeddingsServiceImpl` reads this flag to distinguish "still
    /// loading, retry later" from "failed, retrying will never help" when
    /// answering an RPC while `model` has not yielded a value.
    pub model_load_failed: Arc<AtomicBool>,
    /// Process-global embedding scheduler (ADR-053: per-database compute
    /// scoping). Grants the active database's embedding batches priority over
    /// other open databases so foreground work is not blocked by another
    /// database's backlog. Shared by every database's `EmbeddingProcessor`.
    pub scheduler: Arc<EmbeddingScheduler>,
    /// Builds the pre-delete subtree access gate (ADR-041) for a database as it
    /// is opened. Empty by default, so every database keeps `NodeService`'s
    /// always-allow gate; a host that enforces access fills it in (ADR-082).
    ///
    /// A factory rather than a single gate because a gate is bound to the one
    /// database it guards: `NodeService::set_subtree_access_gate` ignores a
    /// second call, so sharing one instance across databases would pin the first
    /// database's identity and then answer every other database against the
    /// wrong database's access rules — worse than not gating them at all.
    ///
    /// `OnceLock` because a host may only be able to build the factory after the
    /// `DatabaseManager` that holds this context exists. Startup therefore hands
    /// over the context first and fills the factory in immediately after;
    /// databases opened on demand (always later than that) see it. The boot
    /// database, opened before the hand-over, is gated explicitly by the host
    /// instead.
    pub subtree_gate_factory: Arc<OnceLock<SubtreeGateFactory>>,
    /// The daemon's single chat engine and model catalog, shared by every
    /// database's `LocalAgentServiceImpl`.
    ///
    /// The loaded model is a machine resource — gigabytes of weights on one
    /// accelerator, chosen from the app's single model selector — so it belongs
    /// here next to the embedding model rather than being rebuilt per database.
    /// What each database keeps is the graph-bound half: its own tool executor,
    /// prompt assembler, in-flight turns, and ai-chat event watcher.
    pub local_agent: Arc<SharedLocalAgent>,
    /// Test-only: the extension ids this context supports, so a test inside
    /// this crate can open a database that requires one. Absent outside this
    /// crate's own unit tests; see [`SharedContext::supported_extensions`].
    #[cfg(test)]
    pub(crate) supported_extensions: Vec<String>,
}

impl SharedContext {
    /// The extension ids this daemon supports. A database whose settings node
    /// lists any other id in `required_extensions` is refused when opened
    /// (ADR-083 §2).
    ///
    /// Empty: core supports no extension. It is fixed here rather than taken
    /// from [`build_shared_services`]'s caller until a composing build gets a
    /// versioned hook to declare its own (ADR-082 §5, §8). This crate's unit
    /// tests set a fixture set through the test-only field.
    pub(crate) fn supported_extensions(&self) -> &[String] {
        #[cfg(test)]
        {
            &self.supported_extensions
        }
        #[cfg(not(test))]
        {
            &[]
        }
    }
}

/// The refusal of a database whose settings node lists, in
/// `required_extensions`, extensions this daemon does not support (ADR-083
/// §2). [`build_database_services`] returns it before it writes anything to
/// the database.
///
/// This is a compatibility guard, not a security control: the database is a
/// plain SQLite file that any process running as the user can read or edit.
/// It keeps this build from misreading types or edge fields an extension
/// wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseRequiresExtensions {
    /// The required extension ids this daemon does not support, in stored
    /// order.
    pub unsupported: Vec<String>,
}

impl DatabaseRequiresExtensions {
    /// The refusal anywhere in `err`'s chain, so a caller can recognise it
    /// through the context an open path adds.
    pub fn find_in(err: &anyhow::Error) -> Option<&Self> {
        err.chain().find_map(|cause| cause.downcast_ref::<Self>())
    }

    /// The gRPC status a request routed to the refused database receives:
    /// `FAILED_PRECONDITION`, the refusal message, and the
    /// `x-requires-extension-bin` payload (see
    /// [`nodespace_proto::requires_extension`]).
    pub fn to_status(&self) -> tonic::Status {
        nodespace_proto::requires_extension::status(&self.unsupported)
    }
}

impl std::fmt::Display for DatabaseRequiresExtensions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&nodespace_proto::extension_names::refusal_message(
            &self.unsupported,
        ))
    }
}

impl std::error::Error for DatabaseRequiresExtensions {}

/// The guard could not read a database's `required_extensions`: the file is
/// not a readable database, or the field holds something other than a list of
/// strings. The open fails closed, since the guard cannot tell what the
/// database needs, and like a refusal it concerns that database alone.
#[derive(Debug)]
pub struct RequiredExtensionsUnreadable {
    /// The database the guard could not read.
    pub path: std::path::PathBuf,
    source: anyhow::Error,
}

impl RequiredExtensionsUnreadable {
    /// The failure anywhere in `err`'s chain.
    pub fn find_in(err: &anyhow::Error) -> Option<&Self> {
        err.chain().find_map(|cause| cause.downcast_ref::<Self>())
    }
}

impl std::fmt::Display for RequiredExtensionsUnreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not read the required extensions of {}",
            self.path.display()
        )
    }
}

impl std::error::Error for RequiredExtensionsUnreadable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// The extensions the database at `db_path` requires that `shared` does not
/// support. Reads the file through its own read-only connection and writes
/// nothing (see [`nodespace_core::db::required_extensions`]), so it is safe to
/// call for a database that is not open, or that another process is writing.
pub(crate) async fn unsupported_required_extensions(
    db_path: &std::path::Path,
    shared: &SharedContext,
) -> Result<Vec<String>> {
    let required =
        nodespace_core::db::required_extensions::read_required_extensions(db_path).await?;
    let supported = shared.supported_extensions();
    Ok(required
        .into_iter()
        .filter(|id| !supported.contains(id))
        .collect())
}

/// Builds the subtree access gate guarding `database_id`. See
/// [`SharedContext::subtree_gate_factory`].
pub type SubtreeGateFactory =
    Arc<dyn Fn(&str) -> Arc<dyn SubtreeAccessGate> + Send + Sync + 'static>;

/// Process-global services shared across every database the daemon serves
/// (ADR-053: one daemon, multiple local databases). Built once by
/// [`build_shared_services`]. `settings` is registered directly on the router;
/// `context` is what each per-database service set is built from and what the
/// [`crate::services::database_manager::DatabaseManager`] caches.
pub struct SharedServices {
    /// Daemon-wide settings (`daemon.toml`); registered once on the router.
    pub settings: SettingsServiceImpl,
    /// The build context handed to every per-database service set.
    pub context: SharedContext,
}

/// The service set backing a single database. One of these is assembled per
/// open database by [`build_database_services`]; the shared model and PTY
/// manager come from [`SharedServices`].
pub struct DatabaseServices {
    pub node_service_grpc: NodeServiceImpl,
    pub agent_session: AgentSessionHandler,
    pub import: ImportServiceImpl,
    pub local_agent: LocalAgentServiceImpl,
    /// Always registered when a model exists — returns `UNAVAILABLE` while the
    /// model loads, then serves normally. `None` only when no NLP model file
    /// exists at all.
    pub embeddings_service_grpc: Option<EmbeddingsServiceImpl>,
    /// Held so we can drain GPU resources after the server shuts down.
    /// Populated by the background embedding-wiring task.
    pub embedding_state: Arc<RwLock<Option<EmbeddingReady>>>,
    /// Cancelled by [`DatabaseServices::shutdown`] so every `WatchNodes`
    /// stream this database's `node_service_grpc` has open ends instead of
    /// surviving as a zombie (ADR-053: idle eviction reopens an evicted
    /// database as a fresh instance with its own event bus, which an
    /// already-open stream from before eviction can never observe on its
    /// own — see `NodeServiceImpl::shutdown_token`'s doc comment). The same
    /// `tokio_util::sync::CancellationToken` is cloned into
    /// `node_service_grpc` by [`build_database_services`]; kept here too so
    /// `shutdown` has a handle to cancel without reaching back into the
    /// gRPC impl.
    shutdown_token: tokio_util::sync::CancellationToken,
    /// Stops this database's conflict-journal reconciliation sweep
    /// (ADR-068 §5.4) on retirement. `None` when the sweep never started
    /// (should not happen in practice — kept `Option` for symmetry with
    /// how other optional background tasks are represented here).
    conflict_sweep_shutdown: Option<watch::Sender<bool>>,
    /// Stops this database's play (playbook) engine — event subscriber,
    /// `RuleProcessor`, and `CronRunner` — on retirement. `None` when the
    /// engine never started (should not happen in practice — kept `Option`
    /// for symmetry with `conflict_sweep_shutdown`).
    playbook_shutdown: Option<watch::Sender<bool>>,
}

impl DatabaseServices {
    /// Stop everything [`build_database_services`] started for this database
    /// (ADR-053: per-database compute scoping): the ai-chat event watcher, any
    /// turn still in flight, and this database's embedding processor.
    ///
    /// This is the *only* way a service set is retired, and every path that
    /// retires one must go through it — deliberate close, idle eviction, daemon
    /// shutdown, and a set discarded for losing an open race. Dropping the
    /// `Arc` is not equivalent and never was: the watcher task holds its own
    /// clone of this database's `NodeService`, which owns the event sender the
    /// watcher is receiving from, so the channel it waits on can never close on
    /// its own. A set dropped without this call keeps its watcher — and the
    /// store, node service, and embedding processor it pins — alive for the rest
    /// of the process's life, invisible to the registry that no longer lists it.
    ///
    /// Idempotent: calling it twice is a no-op.
    pub async fn shutdown(&self) {
        self.local_agent.shutdown().await;
        // Drop only this database's embedding processor (stops its background
        // task on drop). The shared model is left untouched.
        if let Some(ready) = self.embedding_state.write().await.take() {
            drop(ready.processor);
        }
        if let Some(tx) = &self.conflict_sweep_shutdown {
            let _ = tx.send(true);
        }
        if let Some(tx) = &self.playbook_shutdown {
            let _ = tx.send(true);
        }
        // End every live `WatchNodes` stream on this database rather than
        // leaving them as zombies once the database is gone. Idempotent —
        // cancelling an already-cancelled token is a no-op — which is what
        // makes this safe to call from every retirement path (idle eviction,
        // deliberate close, daemon shutdown, a set discarded for losing an
        // open race) without tracking whether shutdown already ran.
        self.shutdown_token.cancel();
    }
}

/// Build the process-global services shared across every database (ADR-053):
/// the PTY manager, daemon settings, and the single embedding model. The model
/// is loaded once in the background and published over a watch channel so each
/// database's embedding wiring can await it. Returns the shared set plus the
/// model-load task handle (`None` when no model file exists).
pub async fn build_shared_services() -> Result<(SharedServices, Option<tokio::task::JoinHandle<()>>)>
{
    let pty_manager = Arc::new(PtySessionManager::new());
    let settings = SettingsServiceImpl::with_default_path()
        .map_err(|e| anyhow::anyhow!("Failed to initialize SettingsService: {}", e))?;

    // One embedding model backs every database. Determine the path now (cheap);
    // if absent, no task spawns and the channel stays closed so per-database
    // wiring exits quietly with semantic search disabled.
    let model_path = resolve_model_path();
    let has_model = model_path.is_some();
    let (model_tx, model_rx) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
    let model_load_failed = Arc::new(AtomicBool::new(false));
    let model_task = model_path.map(|path| {
        let model_load_failed = model_load_failed.clone();
        // Flagged in flight at scheduling time, not when the task first runs, so
        // a shutdown that finishes before then still sees the pending load.
        let in_flight = SharedModelLoadGuard::start();
        tokio::spawn(async move {
            load_shared_embedding_model_bg(path, model_tx, model_load_failed, in_flight).await;
        })
    });

    // One scheduler backs every database's embedding processor so the active
    // database's batches take priority on the single shared model (ADR-053).
    let scheduler = Arc::new(EmbeddingScheduler::new());

    // One chat engine and model catalog back every database, for the same
    // reason the embedding model does: it is a single machine resource.
    // Resolved through `nodespace_dir` so provider configs follow
    // NODESPACE_HOME exactly as the database and the registry do — reading the
    // real home instead let an isolated daemon serving a temp database take its
    // OpenAI-compat provider configs from (and write probe verdicts into) the
    // user's own `~/.nodespace`.
    let local_agent = SharedLocalAgent::new(crate::nodespace_dir()?.join("daemon.toml"));

    Ok((
        SharedServices {
            settings,
            context: SharedContext {
                pty_manager,
                model: model_rx,
                has_model,
                model_load_failed,
                subtree_gate_factory: Arc::new(OnceLock::new()),
                scheduler,
                local_agent,
                #[cfg(test)]
                supported_extensions: Vec::new(),
            },
        },
        model_task,
    ))
}

/// Open one database and assemble its gRPC service implementations, sharing the
/// process-global services from `shared` (ADR-053).
///
/// Fast (~100ms): initialize `NodeService`, seed schemas, build all gRPC
/// handlers. The embedding model is NOT loaded here — a background task (the
/// returned handle) wires this database's `NodeEmbeddingService` +
/// `EmbeddingProcessor` and populates `embedding_state` once the shared model is
/// ready.
///
/// Refuses, with [`DatabaseRequiresExtensions`], a database that requires an
/// extension this daemon does not support, before anything is written.
pub async fn build_database_services(
    db_path: &std::path::Path,
    shared: &SharedContext,
    database_id: &str,
) -> Result<(DatabaseServices, Option<tokio::task::JoinHandle<()>>)> {
    // The required-extensions guard (ADR-083 §2). It runs first, before the
    // directory below is created or re-restricted and before `SqliteStore::new`,
    // because opening the store is already a write: it switches the journal to
    // WAL, runs the schema DDL (so the guard also precedes the table-shape
    // check) and seeds. A refused database is left exactly as it was: nothing
    // is seeded, no marker is written and nothing is renamed. Every open goes
    // through here — `DatabaseManager::get_or_open`, the boot default and any
    // host calling this directly — so none can bypass it.
    //
    // Not a security control: the file is plain SQLite that any process running
    // as the user can read or edit. The guard only keeps this build from
    // misreading what an extension wrote.
    let unsupported = unsupported_required_extensions(db_path, shared)
        .await
        .map_err(|source| RequiredExtensionsUnreadable {
            path: db_path.to_path_buf(),
            source,
        })?;
    if !unsupported.is_empty() {
        return Err(DatabaseRequiresExtensions { unsupported }.into());
    }

    if let Some(parent) = db_path.parent() {
        // Owner-only from birth (and re-restricted if it already existed at a
        // wider mode): this directory holds the raw SQLite file for every
        // database the daemon opens, default or otherwise registered.
        crate::create_dir_owner_only(parent)
            .await
            .with_context(|| {
                format!("Failed to create database parent dir: {}", parent.display())
            })?;
    }

    let mut store = Arc::new(
        SqliteStore::new(db_path.to_path_buf())
            .await
            .context("Failed to initialize SqliteStore")?,
    );

    let mut node_service = CoreNodeService::new(&mut store)
        .await
        .context("Failed to initialize NodeService")?;

    seed_agent_nodes(&mut node_service).await;

    // ADR-041: gate this database's cascade deletes against its own access rules.
    // Every database gets a gate built for its own id, because a request routed
    // here by `x-ns-database-id` would otherwise reach a service still carrying
    // the always-allow default. Absent unless a host installed a gate factory.
    if let Some(build_gate) = shared.subtree_gate_factory.get() {
        node_service.set_subtree_access_gate(build_gate(database_id));
    }

    let embedding_state: Arc<RwLock<Option<EmbeddingReady>>> = Arc::new(RwLock::new(None));
    // Separate handle for consumers that only need Arc<NodeEmbeddingService> (assembler, etc.)
    let embedding_svc_state: Arc<RwLock<Option<Arc<NodeEmbeddingService>>>> =
        Arc::new(RwLock::new(None));

    let node_service = Arc::new(node_service);

    // See `DatabaseServices::shutdown_token`'s doc comment: cancelled when
    // this database's service set is retired, so every `WatchNodes` stream
    // `node_service_grpc` has open ends instead of surviving as a zombie.
    let shutdown_token = tokio_util::sync::CancellationToken::new();

    // Constructed here (ahead of node_service_grpc) so its lifecycle-manager
    // handle can be threaded into NodeServiceImpl for get-workflow-state —
    // `PlaybookEngine::start` is spawned later, once the rest of this
    // database's services exist, but the engine object and its lifecycle
    // handle are needed now.
    let playbook_engine = Arc::new(PlaybookEngine::new(node_service.clone()));

    // Give this database's NodeService write path a handle onto the same
    // live TriggerIndex the engine will build (ADR-060 §1), so
    // `create_node`/`create_node_in_tx` can look up and dispatch
    // `RuleClass::Invariant` rules synchronously, pre-commit.
    //
    // Correct regardless of exactly where this call lands relative to
    // `seed_agent_nodes` above (which DOES already write through
    // `node_service` — prompt/skill/tool template nodes): dispatch reads the
    // TriggerIndex `playbook_engine.lifecycle()` exposes, and that index is
    // empty until `PlaybookEngine::start()` (`load_active_plays`, spawned
    // further below, after every other database service is built) runs —
    // so no write anywhere in this function, seeding included, can match an
    // invariant rule yet. Injecting the handle here rather than there is
    // just convenience (the engine object already exists); it carries no
    // ordering requirement of its own.
    node_service.set_playbook_lifecycle(playbook_engine.lifecycle().clone());
    // Same handle-injection reasoning as the lifecycle line above, for the
    // engine's `ancestry_dirty` flag: lets `get_workflow_state` (which only
    // ever receives `node_service`, not the engine itself) report when its
    // graph-event candidate path's `ancestor_cache` read is of known-bad
    // staleness, via `NodeService::playbook_ancestry_dirty`.
    node_service.set_playbook_ancestry_dirty(playbook_engine.ancestry_dirty().clone());

    let node_service_grpc = NodeServiceImpl::new(
        node_service.clone(),
        embedding_state.clone(),
        shared.scheduler.clone(),
    )
    .with_database_id(database_id.to_string())
    .with_shutdown_token(shutdown_token.clone())
    .with_playbook_lifecycle(playbook_engine.lifecycle().clone());

    // EmbeddingsService is only registered when a model file exists at startup
    // (the shared model). If the model appears later, the endpoint is absent
    // until daemon restart — intentional, not a regression from prior behavior.
    let embeddings_service_grpc = shared.has_model.then(|| {
        EmbeddingsServiceImpl::new(
            node_service.clone(),
            embedding_state.clone(),
            shared.model_load_failed.clone(),
        )
    });

    let assembler = Arc::new(GraphContextAssembler::new(
        node_service.clone(),
        embedding_svc_state.clone(),
    ));
    // Resolved through `nodespace_dir` so it follows NODESPACE_HOME, exactly as
    // the database and the ADR-053 registry do. Reading it from the real home
    // instead left an isolated daemon serving a temp database while taking its
    // OpenAI-compat provider configs from the user's own `~/.nodespace`.
    let capture_config_path = crate::nodespace_dir()?.join("daemon.toml");
    let agent_session = AgentSessionHandler::new(
        shared.pty_manager.clone(),
        assembler,
        node_service.clone(),
        capture_config_path,
    );

    let import = ImportServiceImpl::new(node_service.clone());
    // The engine and model catalog come from the process-global
    // `SharedLocalAgent`; what is built here is this database's own turn state
    // and its ai-chat event watcher, which reacts to *this* node service's bus.
    // Wired with this database's Play engine lifecycle handle (constructed
    // above, ahead of node_service_grpc) so the local agent's
    // `get_workflow_state` tool shares the same live TriggerIndex/CronRegistry
    // `node_service_grpc` does.
    let local_agent = LocalAgentServiceImpl::new_with_playbook_lifecycle(
        shared.local_agent.clone(),
        node_service.clone(),
        embedding_svc_state.clone(),
        playbook_engine.lifecycle().clone(),
    );
    local_agent.start_event_watcher();

    // Wire this database's embedding processor from the shared model once it
    // loads. Spawned only when a model file exists; otherwise embeddings stay
    // disabled for this database.
    let embedding_task = shared.has_model.then(|| {
        let model = shared.model.clone();
        let store = store.clone();
        let ns = node_service.clone();
        let state = embedding_state.clone();
        let svc_state = embedding_svc_state.clone();
        let scheduler = shared.scheduler.clone();
        let db_id = database_id.to_string();
        tokio::spawn(async move {
            wire_database_embeddings_bg(model, store, ns, state, svc_state, scheduler, db_id).await;
        })
    });

    // Conflict-journal reconciliation sweep (ADR-068 §5.4) — a low-frequency
    // backstop, not required for S1-S3 correctness. One per database, its own
    // watch-channel shutdown signal (mirroring cron_runner_loop's shape),
    // stopped by `DatabaseServices::shutdown` alongside this database's other
    // background tasks.
    let (conflict_sweep_shutdown_tx, conflict_sweep_shutdown_rx) = watch::channel(false);
    tokio::spawn(nodespace_core::conflict_sweep::conflict_sweep_loop(
        node_service.clone(),
        conflict_sweep_shutdown_rx,
    ));

    // Play (playbook) engine — event subscriber, sequential RuleProcessor,
    // and CronRunner's 60-second poll loop, per ADR-073 hard-gated to
    // locally-originated events until ADR-060's multi-device semantics land
    // (see `nodespace_core::playbook::engine::is_replicated_apply`). One per
    // database, its own watch-channel shutdown signal (mirroring the
    // conflict-journal sweep's shape above), stopped by
    // `DatabaseServices::shutdown` alongside this database's other
    // background tasks. `PlaybookEngine::start` subscribes to this
    // database's domain-event broadcast channel FIRST (before loading active
    // plays, to avoid missing an event racing the initial load) and spawns
    // `CronRunner` itself — nothing else needs to.
    let (playbook_shutdown_tx, playbook_shutdown_rx) = watch::channel(false);
    let playbook_db_id = database_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = playbook_engine.start(playbook_shutdown_rx).await {
            tracing::error!(
                database_id = %playbook_db_id,
                error = %e,
                "Play engine exited with an error"
            );
        }
    });

    Ok((
        DatabaseServices {
            node_service_grpc,
            agent_session,
            import,
            local_agent,
            embeddings_service_grpc,
            embedding_state,
            shutdown_token,
            conflict_sweep_shutdown: Some(conflict_sweep_shutdown_tx),
            playbook_shutdown: Some(playbook_shutdown_tx),
        },
        embedding_task,
    ))
}

/// A service set bound to no registered database, for a router that routes
/// every request through [`crate::DbManagerLayer`].
///
/// [`crate::build_base_router`] takes concrete service values, which a daemon
/// normally takes from its default database's set. Behind the routing layer
/// they never serve a database: every per-database handler resolves its target
/// through the manager (ADR-053), and the handlers that do not route — the
/// daemon's version and memory, PTY sessions, the shared model catalog — serve
/// process-global state, which this set shares with every other. So when the
/// default database is refused because it requires an extension this daemon
/// does not support (ADR-083 §2), the daemon builds its router from this set
/// and keeps serving the other databases. Requests for the default receive the
/// refusal.
///
/// The set's own node service runs over a private in-memory database and no
/// background work is started for it: no Play engine, conflict sweep,
/// embedding wiring or ai-chat watcher. Nothing is written to disk.
pub async fn build_unrouted_services(shared: &SharedContext) -> Result<DatabaseServices> {
    // A shared-cache in-memory database, named uniquely so two sets in one
    // process never share it: the store opens a writer and pooled readers, and
    // a plain `:memory:` gives each connection its own empty database.
    let uri = format!(
        "file:nodespace-unrouted-{}?mode=memory&cache=shared",
        ulid::Ulid::new()
    );
    let mut store = Arc::new(
        SqliteStore::new(std::path::PathBuf::from(uri))
            .await
            .context("Failed to initialize the unrouted in-memory store")?,
    );
    let node_service = Arc::new(
        CoreNodeService::new(&mut store)
            .await
            .context("Failed to initialize the unrouted NodeService")?,
    );
    let embedding_state: Arc<RwLock<Option<EmbeddingReady>>> = Arc::new(RwLock::new(None));
    let embedding_svc_state: Arc<RwLock<Option<Arc<NodeEmbeddingService>>>> =
        Arc::new(RwLock::new(None));

    let node_service_grpc = NodeServiceImpl::new(
        node_service.clone(),
        embedding_state.clone(),
        shared.scheduler.clone(),
    );
    let embeddings_service_grpc = shared.has_model.then(|| {
        EmbeddingsServiceImpl::new(
            node_service.clone(),
            embedding_state.clone(),
            shared.model_load_failed.clone(),
        )
    });
    let assembler = Arc::new(GraphContextAssembler::new(
        node_service.clone(),
        embedding_svc_state.clone(),
    ));
    let agent_session = AgentSessionHandler::new(
        shared.pty_manager.clone(),
        assembler,
        node_service.clone(),
        crate::nodespace_dir()?.join("daemon.toml"),
    );
    let import = ImportServiceImpl::new(node_service.clone());
    let local_agent = LocalAgentServiceImpl::new(
        shared.local_agent.clone(),
        node_service,
        embedding_svc_state,
    );

    Ok(DatabaseServices {
        node_service_grpc,
        agent_session,
        import,
        local_agent,
        embeddings_service_grpc,
        embedding_state,
        shutdown_token: tokio_util::sync::CancellationToken::new(),
        conflict_sweep_shutdown: None,
        playbook_shutdown: None,
    })
}

/// The service set to build a daemon's router from when opening its default
/// database failed with `err`.
///
/// A default refused by the required-extensions guard (ADR-083 §2), because it
/// requires an extension this daemon does not support or because the guard
/// could not read what it requires, concerns that one database, not the
/// daemon: it is logged, and an unrouted set ([`build_unrouted_services`]) is
/// returned so the daemon keeps serving the other databases. This differs from
/// a table-shape refusal, which stops the daemon. Any other failure is
/// returned unchanged.
pub async fn unrouted_services_if_default_refused(
    err: anyhow::Error,
    shared: &SharedContext,
) -> Result<Arc<DatabaseServices>> {
    if let Some(refusal) = DatabaseRequiresExtensions::find_in(&err) {
        tracing::warn!(
            unsupported_extensions = ?refusal.unsupported,
            "{refusal}: the default database stays closed and the daemon serves the other databases"
        );
    } else if RequiredExtensionsUnreadable::find_in(&err).is_some() {
        tracing::warn!(
            error = format!("{err:#}"),
            "the default database stays closed and the daemon serves the other databases"
        );
    } else {
        return Err(err);
    }
    Ok(Arc::new(build_unrouted_services(shared).await?))
}

/// Seed prompt, skill, and tool nodes on first launch. Idempotent — existing nodes are skipped.
async fn seed_agent_nodes(node_service: &mut CoreNodeService) {
    let prompt_templates = PromptAssembler::seed_agent_guidance_nodes();
    let skill_templates = seed_skill_nodes();
    let tool_templates = seed_tool_nodes();

    let mut all_template_nodes = Vec::new();
    for tmpl in prompt_templates
        .iter()
        .chain(skill_templates.iter())
        .chain(tool_templates.iter())
    {
        match prepare_nodes_from_template(tmpl) {
            Ok(nodes) => all_template_nodes.push(nodes),
            Err(e) => {
                tracing::warn!(error = ?e, title = %tmpl.title, "Failed to expand seed template")
            }
        }
    }

    if let Err(e) = node_service
        .seed_nodes_from_templates(all_template_nodes)
        .await
    {
        tracing::warn!(error = %e, "Failed to seed agent nodes (non-fatal)");
    }
}

/// Resolve the NLP model path without loading it. Returns `None` when absent.
///
/// Always `~/.nodespace/models` by default, reading the REAL home rather than
/// [`crate::nodespace_dir`] — deliberately unlike the config path above.
///
/// That directory is the shared model store: read-only, and large enough that
/// sharing is the whole point (a dev machine mid-evaluation held 9 GGUFs
/// totalling 39 GB). An isolated daemon should reuse them, not start from an
/// empty directory and re-download. `NODESPACED_MODEL_PATH` overrides it for a
/// run that genuinely needs a different file.
///
/// The config path is not analogous: `daemon.toml` is WRITTEN to (the routing
/// probe caches verdicts there), so resolving it against the real home let an
/// isolated run mutate the user's own configuration.
fn resolve_model_path() -> Option<std::path::PathBuf> {
    let p = if let Ok(custom) = std::env::var("NODESPACED_MODEL_PATH") {
        std::path::PathBuf::from(custom)
    } else {
        let home = dirs::home_dir()?;
        home.join(".nodespace")
            .join("models")
            .join("nomic-embed-text-v1.5.Q8_0.gguf")
    };
    if !p.exists() {
        tracing::warn!(path = %p.display(), "NLP model not found — semantic search disabled");
        return None;
    }
    Some(p)
}

/// Whether the shared embedding model is still loading. See
/// [`shared_model_load_in_flight`]. Process-global because the model is: one
/// load per process, shared by every database, never per-database state.
static SHARED_MODEL_LOAD_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Whether the shared embedding model's native load is still running.
///
/// The load runs inside `spawn_blocking`, which cannot be cancelled, and
/// dropping a tokio runtime waits for every in-flight blocking task. A daemon
/// told to stop mid-load would otherwise stay alive until the load finished,
/// seconds after its graceful shutdown had already completed. `main` checks
/// this once shutdown is done and, when it is set, ends the process without
/// waiting on the load: the load's only output is a `watch` send nobody will
/// read.
pub fn shared_model_load_in_flight() -> bool {
    SHARED_MODEL_LOAD_IN_FLIGHT.load(Ordering::SeqCst)
}

/// Marks the shared model load in flight until dropped, so the flag clears
/// however the load ends: success, error, or a panic unwinding the closure.
struct SharedModelLoadGuard;

impl SharedModelLoadGuard {
    fn start() -> Self {
        SHARED_MODEL_LOAD_IN_FLIGHT.store(true, Ordering::SeqCst);
        Self
    }
}

impl Drop for SharedModelLoadGuard {
    fn drop(&mut self) {
        SHARED_MODEL_LOAD_IN_FLIGHT.store(false, Ordering::SeqCst);
    }
}

/// Background task: load the NLP embedding model once for the whole process and
/// publish it over `model_tx`. Non-fatal — on failure the channel simply never
/// yields a model and embeddings stay disabled everywhere, but `load_failed`
/// is set first so `EmbeddingsServiceImpl` can tell a client the load is
/// never going to complete, instead of a channel that just looks the same as
/// "still loading" forever.
async fn load_shared_embedding_model_bg(
    model_path: std::path::PathBuf,
    model_tx: watch::Sender<Option<Arc<EmbeddingService>>>,
    load_failed: Arc<AtomicBool>,
    in_flight: SharedModelLoadGuard,
) {
    tracing::info!(path = %model_path.display(), "Loading shared embedding model in background");

    let config = nodespace_nlp_engine::EmbeddingConfig {
        model_path: Some(model_path),
        ..Default::default()
    };

    // `EmbeddingService::new` + `initialize` are synchronous CPU/IO-bound operations
    // (~6-8s). Use spawn_blocking so they don't park a tokio worker thread.
    // The in-flight guard moves into the closure, so the flag tracks the native
    // load itself rather than this task's await.
    let nlp = match tokio::task::spawn_blocking(move || {
        let _in_flight = in_flight;
        let mut svc = EmbeddingService::new(config).map_err(|e| {
            tracing::warn!(error = %e, "Failed to create NLP engine — semantic search disabled");
            e
        })?;
        svc.initialize().map_err(|e| {
            tracing::warn!(error = %e, "Failed to load NLP model — semantic search disabled");
            e
        })?;
        Ok::<_, nodespace_nlp_engine::EmbeddingError>(svc)
    })
    .await
    {
        Ok(Ok(svc)) => Arc::new(svc),
        Ok(Err(_)) | Err(_) => {
            load_failed.store(true, Ordering::SeqCst);
            return;
        }
    };

    // A send error only means every database's wiring task has already gone away
    // (daemon shutting down) — nothing left to wire.
    let _ = model_tx.send(Some(nlp));
    tracing::info!("Shared embedding model loaded — semantic search now available");
}

/// Background task: once the shared embedding model is ready, wire this
/// database's `NodeEmbeddingService` + `EmbeddingProcessor` and publish them to
/// the per-database state the gRPC handlers read. Awaits the model over the
/// shared watch channel; a closed channel (no model / load failed) exits quietly
/// with embeddings disabled for this database. Non-fatal throughout.
async fn wire_database_embeddings_bg(
    mut model: watch::Receiver<Option<Arc<EmbeddingService>>>,
    store: Arc<SqliteStore>,
    node_service: Arc<CoreNodeService>,
    state: Arc<RwLock<Option<EmbeddingReady>>>,
    svc_state: Arc<RwLock<Option<Arc<NodeEmbeddingService>>>>,
    scheduler: Arc<EmbeddingScheduler>,
    db_id: String,
) {
    // Wait for the shared model to be published (or the sender to drop).
    let nlp = loop {
        if let Some(nlp) = model.borrow_and_update().clone() {
            break nlp;
        }
        if model.changed().await.is_err() {
            return; // sender dropped — no model will ever arrive
        }
    };

    let node_accessor: Arc<dyn NodeAccessor> = Arc::new((*node_service).clone());
    let behaviors = node_service.behaviors().clone();
    let embedding_service = Arc::new(NodeEmbeddingService::new(
        nlp,
        store,
        node_accessor,
        behaviors,
    ));

    let processor = match EmbeddingProcessor::new(embedding_service.clone(), scheduler, db_id) {
        Ok(p) => Arc::new(p),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to init EmbeddingProcessor — semantic search disabled");
            return;
        }
    };

    // Wire up automatic wake-on-change now that the processor exists.
    node_service.set_embedding_waker(processor.waker());
    // Process any nodes that became stale during the load window.
    processor.wake();

    // Publish to both state handles atomically enough for practical purposes.
    *svc_state.write().await = Some(embedding_service.clone());
    *state.write().await = Some(EmbeddingReady {
        embedding_service,
        processor,
    });
    tracing::info!("Embedding processor wired for database — semantic search now available");
}
