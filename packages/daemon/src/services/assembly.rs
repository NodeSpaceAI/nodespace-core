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
use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
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
    /// is opened. Empty in community builds, where every database keeps
    /// `NodeService`'s always-allow default.
    ///
    /// A factory rather than a single gate because a gate is bound to the one
    /// database it guards: `NodeService::set_subtree_access_gate` ignores a
    /// second call, so sharing one instance across databases would pin the first
    /// database's identity and then answer every other database against the
    /// wrong tenant — worse than not gating them at all.
    ///
    /// `OnceLock` because the Pro daemon cannot build this until after its cloud
    /// service exists, which in turn needs the `DatabaseManager` that holds this
    /// context. Startup therefore hands over the context first and fills the
    /// factory in immediately after; databases opened on demand (always later
    /// than that) see it. The boot database, opened before the hand-over, is
    /// gated explicitly by the Pro daemon instead.
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
        tokio::spawn(async move {
            load_shared_embedding_model_bg(path, model_tx, model_load_failed).await;
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
pub async fn build_database_services(
    db_path: &std::path::Path,
    shared: &SharedContext,
    database_id: &str,
) -> Result<(DatabaseServices, Option<tokio::task::JoinHandle<()>>)> {
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

    // ADR-041: gate this database's cascade deletes against its OWN tenant. Every
    // database gets a gate built for its own id, because a request routed here by
    // `x-ns-database-id` would otherwise reach a service still carrying the
    // always-allow default. Absent in community builds.
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

    let node_service_grpc = NodeServiceImpl::new(
        node_service.clone(),
        embedding_state.clone(),
        shared.scheduler.clone(),
    )
    .with_database_id(database_id.to_string())
    .with_shutdown_token(shutdown_token.clone());

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
    let local_agent = LocalAgentServiceImpl::new(
        shared.local_agent.clone(),
        node_service.clone(),
        embedding_svc_state.clone(),
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
        },
        embedding_task,
    ))
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
) {
    tracing::info!(path = %model_path.display(), "Loading shared embedding model in background");

    let config = nodespace_nlp_engine::EmbeddingConfig {
        model_path: Some(model_path),
        ..Default::default()
    };

    // `EmbeddingService::new` + `initialize` are synchronous CPU/IO-bound operations
    // (~6-8s). Use spawn_blocking so they don't park a tokio worker thread.
    let nlp = match tokio::task::spawn_blocking(move || {
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
