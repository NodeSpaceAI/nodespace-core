//! tonic `LocalAgentService` implementation — node-as-message-queue architecture.
//!
//! The daemon watches for `NodeUpdated`/`NodeCreated` events on `ai-chat` nodes.
//! When the last message in `properties['ai-chat']['messages']` has `role: 'user'`
//! and `status == 'processing'`, it triggers an inference turn in-process.
//!
//! Streaming tokens are broadcast to any connected `SubscribeTokenStream` client
//! (the Tauri process), which translates them to Tauri events for the frontend.
//!
//! Session IPC (StartSession, SendMessage, EndSession) is removed. The node
//! is the sole source of truth for conversation state.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nodespace_agent::agent_types::{
    AgentToolExecutor, ChatInferenceEngine, ChatMessage, ChatModelSpec, ClarifyPrompt,
    InferenceError, InferenceUsage, LocalAgentStatus, ModelManager, ModelStatus, PriorWrite, Role,
    StreamingChunk, ToolExecutionRecord,
};
use nodespace_agent::local_agent::agent_loop::{
    canonical_args, canonical_args_identity, LocalAgentService,
};
use nodespace_agent::local_agent::model_manager::GgufModelManager;
use nodespace_agent::local_agent::tools::{
    is_cross_turn_guarded_tool, is_write_tool, resolves_entities_tool, GraphToolExecutor,
    SharedEmbeddingService,
};
use nodespace_core::models::{
    AiChatCompletedWrite, AiChatMessage, AiChatNode, AiChatResolvedEntity, NodeFilter, NodeUpdate,
};
use nodespace_core::services::{NodeEmbeddingService, NodeService, NodeServiceError};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};

use crate::nodespace::{
    local_agent_service_server::LocalAgentService as GrpcLocalAgentService, AgentChunk,
    CancelModelDownloadRequest, CancelModelDownloadResponse, CancelTurnRequest, CancelTurnResponse,
    DeleteModelRequest, DeleteModelResponse, DownloadModelRequest, EnsureModelReadyRequest,
    GetLocalStatusRequest, GetSystemRamRequest, GetSystemRamResponse, ListModelsRequest,
    ListModelsResponse, LoadModelRequest, LoadModelResponse, LocalAgentStatusResponse, ModelEntry,
    ModelLoadProgressEvent, RecommendedModelRequest, RecommendedModelResponse,
    SubscribeTokenStreamRequest, UnloadModelRequest, UnloadModelResponse,
};

// ---------------------------------------------------------------------------
// Stub inference engine
// ---------------------------------------------------------------------------

struct NoOpInferenceEngine;

#[async_trait]
impl ChatInferenceEngine for NoOpInferenceEngine {
    async fn generate(
        &self,
        _request: nodespace_agent::agent_types::InferenceRequest,
        _on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
    ) -> Result<InferenceUsage, InferenceError> {
        Err(InferenceError::NoModelLoaded)
    }

    async fn model_info(
        &self,
    ) -> Result<Option<nodespace_agent::agent_types::ChatModelSpec>, InferenceError> {
        Ok(None)
    }

    async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
        Ok((text.len() as f32 / 4.0).ceil() as u32)
    }
}

// ---------------------------------------------------------------------------
// LocalAgentServiceImpl
// ---------------------------------------------------------------------------

type AgentService = Arc<LocalAgentService<dyn ChatInferenceEngine, dyn AgentToolExecutor>>;

/// Cancellation tokens keyed by node_id for in-progress turns.
type TurnTokens = Arc<Mutex<HashMap<String, CancellationToken>>>;

/// How long the engine-swap geometry snapshot may take before it is abandoned.
/// Generous for a local read; the bound exists only so a remote engine that
/// stalls cannot hold up the model load that awaits it.
const MODEL_SPEC_SNAPSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Bound on the routing-reliability probe run during an OpenAI-compat model
/// load (see `nodespace_agent::local_agent::routing_probe`).
///
/// The request itself intentionally carries no `max_tokens` cap — it mirrors
/// production's real Stage-2 tool-calling request, which is uncapped for the
/// same reason (a truncated tool-call argument is invalid JSON). A tool call,
/// when the model makes one, fires within the first handful of tokens, so a
/// normal probe resolves in seconds; this timeout exists only to bound the
/// pathological case (a model that answers in prose at length instead of
/// calling a tool) rather than to change what gets measured. On timeout the
/// probe is treated as an engine error — unmeasured, not a suppression
/// verdict — so model loading is never blocked past this bound.
const ROUTING_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// TTL for the OpenAI-compat discovery cache (see
/// `SharedLocalAgent::openai_compat_discovery_cache`).
///
/// Short by design: long enough that the model selector's three call sites
/// (`model-store`, `agent-store`, `ai-chat-model-selector`) mounting in quick
/// succession share one discovery round instead of each paying
/// `DISCOVERY_TIMEOUT`, short enough that a model becoming available (e.g.
/// `ollama pull`) shows up without restarting the app. A user who wants it
/// sooner has the explicit "Refresh remote models" button, which sets
/// `ListModelsRequest::force_refresh` to bypass this TTL entirely.
///
/// No event-driven invalidation: `SettingsServiceImpl` and `SharedLocalAgent`
/// share no state today, and this cache's only writer — Settings' config
/// add/edit/delete — already calls `refreshRemoteModels` with
/// `force_refresh: true` on the frontend right after saving, which gets the
/// same "list reflects a config change immediately" behavior with none of the
/// cross-service wiring. Deferred as YAGNI, not ruled out — revisit if a second
/// config writer appears that can't reach for the same frontend hook.
const OPENAI_COMPAT_DISCOVERY_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Attempts for an ai-chat read-modify-write before giving up. These writes
/// race the frontend's own writes to the same node on every turn, so a
/// conflict is expected; the retry re-reads the winning version and reapplies.
const MAX_WRITE_ATTEMPTS: usize = 5;

/// gRPC error message for local-GGUF-model-management calls made on a daemon
/// whose `GgufModelManager::new()` failed at construction (see
/// `SharedLocalAgent::new`). Chat turns and OpenAI-compatible models are
/// unaffected — only local GGUF model management (list/download/delete/load/
/// unload/cancel/recommended) is degraded.
const MODEL_MANAGER_UNAVAILABLE: &str =
    "local GGUF model manager is unavailable (see daemon logs for the initialization error); \
     chat turns and OpenAI-compatible models are unaffected";

/// Process-global local-inference state (ADR-053: one daemon, many databases).
///
/// The loaded chat model is a machine resource, not a per-database one: it is
/// gigabytes of weights on one GPU/CPU, and the desktop app exposes a single
/// model selector for the whole daemon. Building one engine per open database
/// would load N copies of the same file and re-pay a multi-second load on every
/// database switch, so the engine — and the model catalog, discovery cache, and
/// "which model is active" bookkeeping that describes it — lives here, once,
/// behind an `Arc` handed to every database's [`LocalAgentServiceImpl`] through
/// [`crate::SharedContext`].
///
/// What stays per-database is everything bound to one graph: the tool executor
/// and prompt assembler (they read that database's nodes), the in-flight turn
/// map, the token broadcast, and the ai-chat event watcher. Those live on
/// [`LocalAgentServiceInner`] and are reached by routing a request to the right
/// database's service set.
pub struct SharedLocalAgent {
    /// The chat engine every database runs its turns through. Swapped by a
    /// model load; a turn already running keeps the engine it started with,
    /// since it holds its own `Arc`.
    engine: RwLock<Arc<dyn ChatInferenceEngine>>,
    /// `None` when `GgufModelManager::new()` failed at construction (e.g.
    /// `$HOME` unset, an unwritable/occupied models directory) — a recoverable
    /// environmental condition, not a programming error. The daemon still
    /// starts; local GGUF model management (list/download/delete/load/unload/
    /// cancel/recommended) reports `UNAVAILABLE` instead, while everything
    /// else (ai-chat turns, OpenAI-compatible models, status) is unaffected.
    /// See [`SharedLocalAgent::new`] and [`SharedLocalAgent::model_manager`].
    model_manager: Option<Arc<GgufModelManager>>,
    active_model_id: Mutex<Option<String>>,
    /// Whether Stage-2 candidate injection is disabled for the currently
    /// active model, from a cached routing-probe verdict (see
    /// `nodespace_agent::local_agent::routing_probe`).
    ///
    /// `false` unless an OpenAI-compat model load's probe found suppression.
    /// The native/GGUF path is never probed here — ADR-056 already measures
    /// the locked native model clean, so probing it on every load would pay a
    /// generation for a question already answered.
    active_model_routing_disabled: Mutex<bool>,
    /// The `(base_url, served_model)` pair `active_model_routing_disabled`
    /// was actually measured against, or `None` before any OpenAI-compat load.
    ///
    /// `model_id` alone (what `replace_engine_if_changed`'s `active_model_id`
    /// tracks) is not a reliable cache key here: a single-model server config
    /// with no `/models` discovery resolves to the bare `openai-compat:<uuid>`
    /// model id regardless of what the config's `model` field is set to, so
    /// editing that field in Settings changes the served model without
    /// changing `model_id` or tripping `replace_engine_if_changed`'s "already
    /// loaded" short-circuit. Keying on the pair that was actually probed
    /// (not the opaque id used for engine-swap dedup) means an in-place model
    /// edit is detected even when the engine-swap path sees no change.
    active_model_routing_key: Mutex<Option<(String, String)>>,
    /// The loaded model's geometry, captured at engine-swap time.
    ///
    /// Read by `get_status` instead of calling `model_spec()`, which reaches a
    /// `std::sync::Mutex` that the native engine holds for the *entire*
    /// duration of a generation. Querying it live would make a status RPC
    /// block a tokio worker for the length of a turn (60-180s). The geometry
    /// only changes on engine swap, and `replace_engine` is the single choke
    /// point for that, so a value cached there is always current.
    loaded_model_spec: Mutex<Option<ChatModelSpec>>,
    /// Bound on the engine-swap geometry snapshot. A field rather than a
    /// constant so tests can drive the timeout path without paying it in
    /// wall-clock time.
    model_spec_snapshot_timeout: std::time::Duration,
    /// Path to `~/.nodespace/daemon.toml`, read to resolve OpenAI-compatible
    /// provider configs by UUID when loading an `openai-compat:<uuid>` model.
    daemon_config_path: std::path::PathBuf,
    /// Short-TTL cache over `discover_openai_compat_models`'s result (see
    /// `OPENAI_COMPAT_DISCOVERY_CACHE_TTL`). `None` until the first discovery
    /// round completes. Bypassed (but still refreshed) when a `ListModels`
    /// call sets `force_refresh`.
    openai_compat_discovery_cache: Mutex<
        Option<(
            std::time::Instant,
            Vec<nodespace_agent::agent_types::ModelInfo>,
        )>,
    >,
}

impl SharedLocalAgent {
    /// Build the process-global inference state. Called once, from
    /// [`crate::build_shared_services`].
    pub fn new(daemon_config_path: std::path::PathBuf) -> Arc<Self> {
        // A failed model-manager init is a recoverable environmental condition
        // (`$HOME` unset, an unwritable/occupied models directory), not a
        // programming error: degrade the local-GGUF RPCs to `UNAVAILABLE` the
        // same way a missing NLP model disables only the embedding wiring,
        // rather than failing daemon startup outright.
        let model_manager = match GgufModelManager::new() {
            Ok(m) => Some(Arc::new(m)),
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "GgufModelManager initialization failed — local GGUF model management \
                     disabled (chat turns and OpenAI-compatible models are unaffected)"
                );
                None
            }
        };
        Self::from_model_manager(
            daemon_config_path,
            model_manager,
            MODEL_SPEC_SNAPSHOT_TIMEOUT,
        )
    }

    /// Shared construction path for [`Self::new`] and for tests, which use it to
    /// cover the degraded shape a failed `GgufModelManager::new()` produces
    /// (`model_manager: None`) without breaking `$HOME` or the real models
    /// directory, and to shorten `model_spec_snapshot_timeout` so the
    /// stalled-`model_info` path is exercised without paying the production
    /// bound in wall-clock time.
    fn from_model_manager(
        daemon_config_path: std::path::PathBuf,
        model_manager: Option<Arc<GgufModelManager>>,
        model_spec_snapshot_timeout: std::time::Duration,
    ) -> Arc<Self> {
        let noop: Arc<dyn ChatInferenceEngine> = Arc::new(NoOpInferenceEngine);
        Arc::new(Self {
            engine: RwLock::new(noop),
            model_manager,
            active_model_id: Mutex::new(None),
            active_model_routing_disabled: Mutex::new(false),
            active_model_routing_key: Mutex::new(None),
            loaded_model_spec: Mutex::new(None),
            model_spec_snapshot_timeout,
            daemon_config_path,
            openai_compat_discovery_cache: Mutex::new(None),
        })
    }

    /// The engine a turn starting now should run against.
    async fn engine(&self) -> Arc<dyn ChatInferenceEngine> {
        self.engine.read().await.clone()
    }

    /// The GGUF model manager, or `None` when `GgufModelManager::new()` failed
    /// at construction (see the field's doc comment). Every gRPC handler that
    /// manages local GGUF models (list/download/delete/load/unload/cancel/
    /// recommended) routes through this instead of unwrapping the field
    /// directly, pairing it with `MODEL_MANAGER_UNAVAILABLE` via
    /// `.ok_or_else(...)?` so a failed init degrades those RPCs one at a time
    /// rather than being able to panic anywhere the field is read.
    ///
    /// Returns `Option`, not a `Result<_, Status>`, so this helper's own
    /// return type does not trip `clippy::result_large_err` — `Status` is
    /// ~176 bytes, and unlike the gRPC handlers themselves (whose `Ok` side,
    /// `Response<T>`, is comparably large), the natural `Ok` side here is a
    /// single pointer.
    fn model_manager(&self) -> Option<&Arc<GgufModelManager>> {
        self.model_manager.as_ref()
    }

    /// Publish `engine` as the daemon's loaded model. Every open database picks
    /// it up on its next turn, which reads this slot when it wires the engine to
    /// that database's tools.
    async fn set_engine(&self, engine: Arc<dyn ChatInferenceEngine>) {
        // Snapshot the model geometry here, the one place an engine can change.
        // Safe to query now: a swap happens between turns, so the engine mutex
        // this reaches is uncontended, unlike the same call from `get_status`.
        //
        // Bounded because this is not always a local read: a remote engine
        // answers `model_info` with an HTTP round-trip on a client with no
        // default timeout, so an endpoint that accepts the connection and then
        // stalls would hang the model-load RPC that awaits this. Losing the
        // geometry only costs a degraded status report; blocking the swap
        // would cost the load itself.
        let spec = match tokio::time::timeout(self.model_spec_snapshot_timeout, engine.model_info())
            .await
        {
            Ok(Ok(spec)) => spec,
            Ok(Err(e)) => {
                tracing::warn!(
                    error = %e,
                    "model_info failed during engine swap; status will report no model loaded"
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    timeout = ?self.model_spec_snapshot_timeout,
                    "model_info timed out during engine swap; status will report no model loaded"
                );
                None
            }
        };

        *self.engine.write().await = engine;
        // The engine lock is released before taking `loaded_model_spec`: no
        // site holds two of these locks at once, which is what keeps the
        // ordering acyclic.
        *self.loaded_model_spec.lock().await = spec;
    }

    /// Swap in `engine` unless `model_id` is already the active model. Returns
    /// whether a swap happened.
    async fn set_engine_if_changed(
        &self,
        model_id: &str,
        engine: Arc<dyn ChatInferenceEngine>,
    ) -> bool {
        {
            let active = self.active_model_id.lock().await;
            if active.as_deref() == Some(model_id) {
                return false;
            }
        }
        self.set_engine(engine).await;
        *self.active_model_id.lock().await = Some(model_id.to_string());
        true
    }

    /// Drop back to the no-op engine, clearing every fact that described the
    /// model that was loaded.
    pub async fn reset_to_noop_engine(&self) {
        self.set_engine(Arc::new(NoOpInferenceEngine)).await;
        // `set_engine` already re-snapshots the geometry (the no-op engine
        // reports none), but the rest of the description is cleared here — and
        // `loaded_model_spec` with it, so this reads as one complete reset
        // rather than depending on what the no-op engine happens to answer.
        *self.active_model_id.lock().await = None;
        *self.active_model_routing_disabled.lock().await = false;
        *self.active_model_routing_key.lock().await = None;
        *self.loaded_model_spec.lock().await = None;
        tracing::debug!("SharedLocalAgent: inference engine reset to NoOp");
    }
}

/// The per-database half of the local agent: everything bound to one graph.
/// The engine and model catalog it runs against live on the process-global
/// [`SharedLocalAgent`].
struct LocalAgentServiceInner {
    /// Process-global inference state — the loaded engine and model catalog.
    shared: Arc<SharedLocalAgent>,
    node_service: Arc<NodeService>,
    embedding_service: SharedEmbeddingService,
    /// Broadcast channel for streaming tokens → all SubscribeTokenStream clients.
    token_tx: broadcast::Sender<AgentChunk>,
    /// Cancellation tokens keyed by node_id.
    turn_tokens: TurnTokens,
    /// Cancels this database's background event watcher, and gates new turns,
    /// once the database is closed (ADR-053: per-database compute scoping).
    /// Shared across the cheap `Arc` clones tonic hands to request handlers, so
    /// a single `shutdown()` stops the watcher spawned from any clone.
    shutdown_token: CancellationToken,
}

/// tonic-compatible handle. `Clone` (cheap Arc clone) so tonic can hand
/// copies to concurrent request handlers.
#[derive(Clone)]
pub struct LocalAgentServiceImpl {
    inner: Arc<LocalAgentServiceInner>,
}

impl LocalAgentServiceImpl {
    /// Build the local-agent service for one database, running against the
    /// process-global engine in `shared`.
    pub fn new(
        shared: Arc<SharedLocalAgent>,
        node_service: Arc<NodeService>,
        embedding_service: SharedEmbeddingService,
    ) -> Self {
        // Channel capacity: enough headroom for burst token output (~256 tokens per broadcast).
        let (token_tx, _) = broadcast::channel(512);

        Self {
            inner: Arc::new(LocalAgentServiceInner {
                shared,
                node_service,
                embedding_service,
                token_tx,
                turn_tokens: Arc::new(Mutex::new(HashMap::new())),
                shutdown_token: CancellationToken::new(),
            }),
        }
    }

    /// Tear down this database's local-agent compute (ADR-053: per-database
    /// compute scoping). Called from [`crate::DatabaseServices::shutdown`] on
    /// every path that retires a service set — deliberate close, idle eviction,
    /// daemon shutdown, and a lost open race.
    ///
    /// Stops the event watcher, refuses any further turns, and cancels the ones
    /// already running. Cancelling matters as much as stopping the watcher: a
    /// turn is a separately spawned task holding its own clones of this
    /// database's handles, so it would otherwise keep generating (and writing)
    /// against a database the manager no longer considers open — and leave the
    /// chat node in `processing`, which the next open's recovery scan would pick
    /// up and run a *second* time. Cancelled turns instead settle the node back
    /// to `idle`, so there is nothing for recovery to re-run.
    ///
    /// Idempotent and cheap — cancelling an already-cancelled token is a no-op.
    pub async fn shutdown(&self) {
        self.inner.shutdown_token.cancel();
        for token in self.inner.turn_tokens.lock().await.values() {
            token.cancel();
        }
    }

    /// Whether this database has an ai-chat turn in flight. Read by the idle
    /// reaper, which must not evict a database mid-turn.
    pub async fn has_active_turns(&self) -> bool {
        !self.inner.turn_tokens.lock().await.is_empty()
    }

    /// Claim `node_id` for a turn, returning the cancellation token to run it
    /// under — or `None` when a turn is already in flight for that node, or this
    /// database has been shut down.
    ///
    /// The check-and-insert is atomic so `NodeCreated`/`NodeUpdated` arriving in
    /// close succession, or the recovery scan racing a live event, cannot both
    /// start the same turn.
    pub(crate) async fn begin_turn(&self, node_id: &str) -> Option<CancellationToken> {
        if self.inner.shutdown_token.is_cancelled() {
            return None;
        }
        let mut tokens = self.inner.turn_tokens.lock().await;
        if tokens.contains_key(node_id) {
            return None;
        }
        let cancel = CancellationToken::new();
        tokens.insert(node_id.to_string(), cancel.clone());
        Some(cancel)
    }

    /// Release the claim [`Self::begin_turn`] took, once the turn has settled.
    pub(crate) async fn end_turn(&self, node_id: &str) {
        self.inner.turn_tokens.lock().await.remove(node_id);
    }

    /// This database's agent service for a turn starting now: the daemon's
    /// currently-loaded engine wired to *this* database's tools and prompt
    /// assembler.
    ///
    /// Built per turn rather than cached. Construction is a handful of `Arc`
    /// moves and two empty maps, and the sessions it holds are created and
    /// ended within the turn, so a cache would buy nothing — while keeping a
    /// stale one alive would pin the previously-loaded model's weights in a
    /// database that simply hasn't been chatted with since the swap.
    async fn get_service(&self) -> AgentService {
        let engine = self.inner.shared.engine().await;
        // Hand the executor the *shared* embedding handle, not a snapshot. The
        // executor reads the current value per call, so search_semantic and
        // skill retrieval work as soon as the embedding model finishes loading
        // in the background — no engine swap required, and no construction
        // site can wire a stale or `None` service.
        //
        // `inference_engine` is different: it's not a shared handle updated in
        // place, just the plain engine this executor should use for
        // `resolve_query`'s nested decomposition call. The whole service is
        // rebuilt per turn, so there is no separate "wire once, update later"
        // path to support for it.
        let executor: Arc<dyn AgentToolExecutor> = Arc::new(GraphToolExecutor {
            node_service: Some(self.inner.node_service.clone()),
            embedding_service: self.inner.embedding_service.clone(),
            inference_engine: Some(engine.clone()),
        });

        let prompt_assembler = Some(Arc::new(
            nodespace_agent::prompt_assembler::PromptAssembler::new(
                self.inner.node_service.clone(),
            ),
        ));

        Arc::new(LocalAgentService::new_with_assembler(
            engine,
            executor,
            prompt_assembler,
        ))
    }

    /// Resolve which database this request targets (ADR-053) and return that
    /// database's local-agent service. The routing contract lives in
    /// [`crate::db_routing::routed_database_services`]: a header selects a
    /// registered database, header-less requests hit the default, and with no
    /// routing middleware installed a header-less request falls back to `self`
    /// while a header-carrying one is rejected.
    ///
    /// Only the handlers that read or write per-database turn state route —
    /// `SubscribeTokenStream`, `CancelTurn`, `GetStatus`. Model management
    /// (list/download/delete/load/unload/cancel/ensure-ready/recommended/RAM)
    /// acts on the process-global [`SharedLocalAgent`], so it reaches the same
    /// engine and catalog regardless of which database a caller names — routing
    /// it would be a no-op that only obscured that fact.
    async fn route<T>(&self, request: &Request<T>) -> Result<LocalAgentServiceImpl, Status> {
        match crate::db_routing::routed_database_services(request).await? {
            Some(services) => Ok(services.local_agent.clone()),
            None => Ok(self.clone()),
        }
    }

    /// The process-global GGUF model manager. See
    /// [`SharedLocalAgent::model_manager`] for why this is an `Option` and how
    /// handlers pair it with `MODEL_MANAGER_UNAVAILABLE`.
    fn model_manager(&self) -> Option<&Arc<GgufModelManager>> {
        self.inner.shared.model_manager()
    }

    /// Swap the daemon's loaded engine. Every open database's next turn runs
    /// against it.
    async fn replace_engine(&self, engine: Arc<dyn ChatInferenceEngine>) {
        self.inner.shared.set_engine(engine).await;
    }

    async fn replace_engine_if_changed(
        &self,
        model_id: &str,
        engine: Arc<dyn ChatInferenceEngine>,
    ) -> bool {
        self.inner
            .shared
            .set_engine_if_changed(model_id, engine)
            .await
    }

    // ---------------------------------------------------------------------------
    // Event watcher — subscribes to NodeService and reacts to ai-chat changes
    // ---------------------------------------------------------------------------

    /// Spawn a background task that subscribes to the NodeService event bus and
    /// handles ai-chat node changes. Call once from the daemon startup after
    /// `LocalAgentServiceImpl::new`.
    ///
    /// Also scans for nodes stuck in `status: 'processing'` at startup (daemon
    /// restart recovery) and retries those turns.
    pub fn start_event_watcher(&self) {
        let this = self.clone();
        tokio::spawn(async move {
            // Recovery pass: find any ai-chat nodes stuck in processing.
            this.recover_stuck_turns().await;

            let mut rx = this.inner.node_service.subscribe_to_events();
            loop {
                let event = tokio::select! {
                    // Stop promptly when the owning database is closed (ADR-053),
                    // even if no further events arrive on the bus.
                    _ = this.inner.shutdown_token.cancelled() => {
                        tracing::info!(
                            "LocalAgentService event watcher: database closed, stopping"
                        );
                        break;
                    }
                    event = rx.recv() => event,
                };

                match event {
                    Ok(envelope) => {
                        let (node_id, node_type) = match &envelope.event {
                            nodespace_core::db::events::DomainEvent::NodeCreated {
                                node_id,
                                node_type,
                            } => (node_id.clone(), node_type.clone()),
                            nodespace_core::db::events::DomainEvent::NodeUpdated {
                                node_id,
                                node_type,
                                ..
                            } => (node_id.clone(), node_type.clone()),
                            _ => continue,
                        };

                        if node_type != "ai-chat" {
                            continue;
                        }

                        let this2 = this.clone();
                        tokio::spawn(async move {
                            this2.maybe_handle_ai_chat_node(&node_id).await;
                        });
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(
                            skipped,
                            "LocalAgentService event watcher lagged; some ai-chat events dropped"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("LocalAgentService event watcher: channel closed, stopping");
                        break;
                    }
                }
            }
        });
    }

    /// Check the node and start an inference turn if appropriate.
    async fn maybe_handle_ai_chat_node(&self, node_id: &str) {
        let node = match self.inner.node_service.get_node(node_id).await {
            Ok(Some(n)) => n,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(node_id, error = %e, "failed to fetch ai-chat node");
                return;
            }
        };

        let ai_chat = match AiChatNode::from_node(node) {
            Ok(c) => c,
            Err(_) => return,
        };

        // Only trigger when the frontend has set status: processing, signalling
        // it wants an inference turn. Any other status (idle, error) is not
        // actionable here.
        if ai_chat.turn_status != "processing" {
            return;
        }

        // Check that the last message is from the user.
        match ai_chat.messages.last() {
            Some(last) if last.role == "user" => {}
            _ => return,
        }

        let Some(cancel) = self.begin_turn(node_id).await else {
            return;
        };

        tracing::info!(node_id, "ai-chat turn triggered");
        self.run_ai_chat_turn(node_id.to_string(), cancel).await;
    }

    /// Execute a full inference turn for the given ai-chat node.
    /// `cancel` is already stored in `turn_tokens` by the caller.
    /// The caller is responsible for having already set status: processing
    /// before triggering this turn — writing it again here would re-emit a
    /// nodeUpdated event and cause a re-entry loop.
    async fn run_ai_chat_turn(&self, node_id: String, cancel: CancellationToken) {
        // One read of the chat node serves both of the things this turn needs
        // from it: the rendered inference history, and the record of what
        // earlier turns wrote (which seeds the duplicate guard below).
        let messages = load_chat_messages(&self.inner.node_service, &node_id).await;
        let prior_writes = prior_writes_from_history(&messages);
        let history = node_history_from_messages(messages);
        if history.is_empty() {
            tracing::warn!(node_id, "ai-chat history empty — skipping turn");
            if let Err(e) = self.write_ai_chat_turn_status(&node_id, "idle", None).await {
                tracing::warn!(node_id, error = %e, "failed to reset ai-chat status to idle");
            }
            self.end_turn(&node_id).await;
            return;
        }

        // Separate the user message (last) from the prior history.
        let user_message = match history.last() {
            Some(m) if m.role == Role::User => m.content.clone(),
            _ => {
                tracing::warn!(node_id, "ai-chat last message is not from user — skipping");
                if let Err(e) = self.write_ai_chat_turn_status(&node_id, "idle", None).await {
                    tracing::warn!(node_id, error = %e, "failed to reset ai-chat status to idle");
                }
                self.end_turn(&node_id).await;
                return;
            }
        };
        let prior_history: Vec<ChatMessage> = history[..history.len() - 1].to_vec();

        let service = self.get_service().await;

        // Refresh workspace context before creating the session.
        //
        // Schema retrieval embeds the blended query, NOT `user_message` alone —
        // passing the bare message here is the regression this call site exists
        // to avoid, and it fails silently (retrieval just gets worse). See
        // `schema_retrieval_query`.
        let emb = self.inner.embedding_service.read().await.clone();
        let retrieval_query = schema_retrieval_query(&prior_history, &user_message);
        let ctx =
            build_workspace_context(&self.inner.node_service, emb, Some(&retrieval_query)).await;

        // Create an ephemeral session seeded with prior history.
        let session_id = service.create_session(None, prior_history).await;

        if let Ok(ctx_str) = ctx {
            service.set_session_context(&session_id, ctx_str).await;
        }

        // Carry the currently active model's cached routing-probe verdict
        // onto this session (see `load_model_and_collect_events`'s OpenAI-compat
        // branch, where the probe runs and this flag is set).
        if *self.inner.shared.active_model_routing_disabled.lock().await {
            service
                .set_session_routing_disabled(&session_id, true)
                .await;
        }

        // Seed the deterministic duplicate guard with what earlier turns wrote
        // (read above, alongside the history). The prompt note built from the
        // same record tells the model the work is done; this makes the
        // tool-execution path enforce it regardless of whether the model heeds
        // that note.
        if !prior_writes.is_empty() {
            service
                .set_session_prior_writes(&session_id, prior_writes)
                .await;
        }

        let token_tx = self.inner.token_tx.clone();
        let node_id2 = node_id.clone();

        let send_fut = service.send_message(
            &session_id,
            &user_message,
            move |_status: LocalAgentStatus| {},
            move |chunk: StreamingChunk| {
                // `Done` is conveyed via the turn result, not a wire chunk.
                // `Reasoning` is intentionally not streamed live (it is captured
                // and surfaced via the persisted message; see issue design):
                // skip it here so the live answer stream stays clean.
                if !matches!(
                    chunk,
                    StreamingChunk::Done { .. } | StreamingChunk::Reasoning { .. }
                ) {
                    let mut proto = streaming_chunk_to_proto(chunk);
                    proto.node_id = Some(node_id2.clone());
                    // Ignore send errors — no subscribers connected is fine.
                    let _ = token_tx.send(proto);
                }
            },
        );

        // Race inference against the cancellation token.
        // `needs_idle_reset`: append_assistant_message sets status: idle on success,
        // so only cancelled/failed paths need an explicit reset.
        let needs_idle_reset;

        // Captured only on the `Err` arm below so the post-select match can
        // tell "inference failed" apart from "cancelled" — both collapse to
        // `turn_result: None`, but only the former is a failure that must be
        // surfaced on the node (a cancellation is an intentional user action,
        // not an error worth a visible message).
        let mut turn_error: Option<InferenceError> = None;

        let turn_result = tokio::select! {
            result = send_fut => {
                match result {
                    Ok(r) => Some(r),
                    Err(e) => {
                        tracing::warn!(node_id, error = %e, "inference turn failed");
                        let _ = self.inner.token_tx.send(AgentChunk {
                            chunk_type: "error".to_string(),
                            error_message: Some(e.to_string()),
                            node_id: Some(node_id.clone()),
                            ..Default::default()
                        });
                        turn_error = Some(e);
                        None
                    }
                }
            },
            () = cancel.cancelled() => {
                tracing::info!(node_id, "ai-chat turn cancelled");
                let _ = self.inner.token_tx.send(AgentChunk {
                    chunk_type: "cancelled".to_string(),
                    node_id: Some(node_id.clone()),
                    ..Default::default()
                });
                None
            }
        };

        // End the ephemeral session.
        service.end_session(&session_id).await;

        match turn_result {
            Some(result) => {
                // Emit done chunk to subscribers.
                let _ = self.inner.token_tx.send(AgentChunk {
                    chunk_type: "done".to_string(),
                    prompt_tokens: Some(result.usage.prompt_tokens as i32),
                    completion_tokens: Some(result.usage.completion_tokens as i32),
                    node_id: Some(node_id.clone()),
                    ..Default::default()
                });

                // Append assistant message; also atomically sets status: idle.
                // `AgentTurnResult` is the authoritative current-turn output: every
                // return path (normal, no-tools-final, synthesized fallback, empty)
                // sets `response`/`reasoning` for *this* turn. Using it directly avoids
                // a session-history scan that, on the fallback branches (which do not
                // push an assistant message), could surface a *previous* turn's
                // content/reasoning instead.
                match self
                    .append_assistant_message(
                        &node_id,
                        &result.response,
                        result.reasoning.as_deref(),
                        completed_writes_from(&result.tool_calls_made),
                        resolved_entities_from(&result.tool_calls_made),
                        result.clarify.as_ref(),
                    )
                    .await
                {
                    Ok(()) => needs_idle_reset = false,
                    Err(e) => {
                        tracing::warn!(node_id, error = %e, "failed to append assistant message");
                        needs_idle_reset = true;
                    }
                }
            }
            None => {
                match turn_error {
                    Some(e) => {
                        // A turn whose inference call failed (context window
                        // exceeded, engine error, ...) must fail *visibly* —
                        // ADR-062's "refuse loudly, don't clamp silently"
                        // principle, applied here at the per-turn level, not
                        // just at model load. Recording nothing and quietly
                        // resetting to "idle" is indistinguishable, to any
                        // caller polling this node (or the frontend, which
                        // only renders `user`/`assistant` messages), from a
                        // turn that is still running — the exact opaque
                        // failure ADR-062 says is worse than an actionable
                        // one. Appending an assistant-role message reuses the
                        // one existing "turn produced visible output"
                        // convention (`assistant_count` polling,
                        // frontend rendering) rather than inventing a second,
                        // differently-handled channel for errors.
                        // This text becomes a normal `role: "assistant"` history
                        // entry, so a later turn's model call sees it like any
                        // other prior reply (`node_history_from_messages`,
                        // `terse_assistant_facts` — the latter returns `None`
                        // here since `completed_writes` is empty, so it cannot
                        // trip the cross-turn duplicate-write guard). Accepted
                        // rather than filtered out or specially tagged: the two
                        // existing safety nets already cover it — unbounded
                        // history growth is bounded by `maybe_summarize_history`
                        // the same as any other turn, and a model that sees its
                        // own stated failure is exactly the context it needs to
                        // avoid repeating the same failing action blindly.
                        let error_text = format!("This turn failed and could not complete: {e}");
                        match self
                            .append_assistant_message(
                                &node_id,
                                &error_text,
                                None,
                                Vec::new(),
                                Vec::new(),
                                None,
                            )
                            .await
                        {
                            Ok(()) => needs_idle_reset = false,
                            Err(append_err) => {
                                tracing::warn!(
                                    node_id,
                                    error = %append_err,
                                    "failed to append error message after inference failure"
                                );
                                needs_idle_reset = true;
                            }
                        }
                    }
                    None => {
                        // Cancelled — an intentional user action, not a
                        // failure; no visible message, matching the
                        // `"cancelled"` chunk already sent above.
                        needs_idle_reset = true;
                    }
                }
            }
        }

        if needs_idle_reset {
            if let Err(e) = self.write_ai_chat_turn_status(&node_id, "idle", None).await {
                tracing::warn!(node_id, error = %e, "failed to reset ai-chat status to idle");
            }
        }

        self.end_turn(&node_id).await;
        tracing::info!(node_id, "ai-chat turn complete");
    }

    /// Scan for ai-chat nodes stuck in `status: 'processing'` at daemon startup
    /// and retry their turns (handles daemon restart mid-turn).
    async fn recover_stuck_turns(&self) {
        let filter = NodeFilter::new().with_node_type("ai-chat".to_string());

        let nodes = match self.inner.node_service.query_nodes(filter).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "recovery scan failed");
                return;
            }
        };

        for node in nodes {
            let node_id = node.id.clone();
            let ai_chat = match AiChatNode::from_node(node) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if ai_chat.turn_status != "processing" {
                continue;
            }
            // Verify last message is from user before retrying.
            let is_trailing_user = ai_chat
                .messages
                .last()
                .map(|m| m.role == "user")
                .unwrap_or(false);

            if is_trailing_user {
                tracing::info!(node_id = %node_id, "recovering stuck ai-chat turn");
                let Some(cancel) = self.begin_turn(&node_id).await else {
                    continue;
                };
                let this = self.clone();
                tokio::spawn(async move {
                    this.run_ai_chat_turn(node_id, cancel).await;
                });
            } else {
                // Stuck in processing but no trailing user message — reset to idle.
                // This is the recovery sweep for already-stuck nodes, so a silent
                // failure here means recovery quietly did not happen and the node
                // stays stuck across restarts with nothing in the log to find it by.
                if let Err(e) = self.write_ai_chat_turn_status(&node_id, "idle", None).await {
                    tracing::warn!(
                        node_id,
                        error = %e,
                        "failed to reset stuck ai-chat node to idle during recovery"
                    );
                }
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Node write helpers
    // ---------------------------------------------------------------------------

    /// Write `properties['ai-chat']['turn_status']` to the node.
    ///
    /// Daemon-owned axis only — never touches `session_status` (the PTY-owned
    /// lifecycle), so this cannot un-archive or archive a session as a side
    /// effect of a turn-state write.
    ///
    /// Retries the full read-modify-write on version conflict: the frontend
    /// writes to the same node (appending a user message, setting
    /// `processing`), so a conflict here is an ordinary race, not a fault.
    /// Giving up early would drop the turn's terminal status write and strand
    /// the node in `processing` forever.
    async fn write_ai_chat_turn_status(
        &self,
        node_id: &str,
        turn_status: &str,
        model: Option<&str>,
    ) -> Result<(), String> {
        for attempt in 0..MAX_WRITE_ATTEMPTS {
            let node = self
                .inner
                .node_service
                .get_node(node_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("node {node_id} not found"))?;

            let version = node.version;
            let mut props = node.properties.clone();
            let mut ai_chat = AiChatNode::from_node(node).map_err(|e| e.to_string())?;

            ai_chat.turn_status = turn_status.to_string();
            if let Some(m) = model {
                ai_chat.model = Some(m.to_string());
            }

            // Splice the updated namespace back, preserving sibling namespaces.
            props["ai-chat"] = ai_chat.to_properties_value();

            let update = NodeUpdate::new().with_properties(props);
            match self
                .inner
                .node_service
                .update_node(node_id, version, update)
                .await
            {
                Ok(_) => return Ok(()),
                // Only a lost race is retryable. A deterministic fault (invalid
                // update, database error) fails the same way every attempt, so
                // retrying it just burns the budget in a tight loop and buries
                // the real cause behind a generic exhaustion message.
                Err(NodeServiceError::VersionConflict { .. })
                    if attempt + 1 < MAX_WRITE_ATTEMPTS =>
                {
                    tracing::debug!(
                        node_id,
                        attempt,
                        "version conflict writing ai-chat turn_status, retrying"
                    );
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Err(format!(
            "failed to write ai-chat turn_status for {node_id} after {MAX_WRITE_ATTEMPTS} attempts"
        ))
    }

    /// Append an assistant message to `properties['ai-chat']['messages']`.
    ///
    /// Retries the full read-modify-write on version conflict for the same
    /// reason as `write_ai_chat_turn_status` — losing this write loses the reply.
    /// `reasoning` is the model's captured
    /// chain-of-thought, persisted alongside the answer when present.
    ///
    /// `completed_writes` records the graph writes this turn performed. The agent
    /// session is rebuilt from these persisted messages on every turn, so this is
    /// the only durable evidence that the turn's write actually happened; without
    /// it the next turn can re-execute an instruction it already satisfied.
    async fn append_assistant_message(
        &self,
        node_id: &str,
        content: &str,
        reasoning: Option<&str>,
        completed_writes: Vec<AiChatCompletedWrite>,
        resolved_entities: Vec<AiChatResolvedEntity>,
        clarify: Option<&ClarifyPrompt>,
    ) -> Result<(), String> {
        for attempt in 0..MAX_WRITE_ATTEMPTS {
            let node = self
                .inner
                .node_service
                .get_node(node_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("node {node_id} not found"))?;

            let version = node.version;
            let mut props = node.properties.clone();
            let mut ai_chat = AiChatNode::from_node(node).map_err(|e| e.to_string())?;

            // Persist reasoning only when the model produced some, keeping the
            // message shape minimal for plain answers.
            let reasoning = reasoning
                .filter(|r| !r.trim().is_empty())
                .map(|r| r.to_string());
            ai_chat.messages.push(AiChatMessage {
                role: "assistant".to_string(),
                content: content.to_string(),
                timestamp: Some(chrono::Utc::now().to_rfc3339()),
                reasoning,
                completed_writes: completed_writes.clone(),
                resolved_entities: resolved_entities.clone(),
                question: clarify.map(|c| c.question.clone()),
                options: clarify.map(|c| c.options.clone()).unwrap_or_default(),
            });

            // Set status to idle here too (atomic with message append).
            ai_chat.turn_status = "idle".to_string();

            // Splice the updated namespace back, preserving sibling namespaces.
            props["ai-chat"] = ai_chat.to_properties_value();

            let update = NodeUpdate::new().with_properties(props);
            match self
                .inner
                .node_service
                .update_node(node_id, version, update)
                .await
            {
                Ok(_) => return Ok(()),
                // Retry only a lost race — see write_ai_chat_turn_status.
                Err(NodeServiceError::VersionConflict { .. })
                    if attempt + 1 < MAX_WRITE_ATTEMPTS =>
                {
                    tracing::debug!(
                        node_id,
                        attempt,
                        "version conflict appending assistant message, retrying"
                    );
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Err(format!(
            "failed to append assistant message to {node_id} after {MAX_WRITE_ATTEMPTS} attempts"
        ))
    }
}

// ---------------------------------------------------------------------------
// gRPC trait implementation
// ---------------------------------------------------------------------------

#[tonic::async_trait]
impl GrpcLocalAgentService for LocalAgentServiceImpl {
    type SubscribeTokenStreamStream = ReceiverStream<Result<AgentChunk, Status>>;

    async fn subscribe_token_stream(
        &self,
        request: Request<SubscribeTokenStreamRequest>,
    ) -> Result<Response<Self::SubscribeTokenStreamStream>, Status> {
        let this = self.route(&request).await?;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<AgentChunk, Status>>(128);
        let mut broadcast_rx = this.inner.token_tx.subscribe();

        tokio::spawn(async move {
            loop {
                match broadcast_rx.recv().await {
                    Ok(chunk) => {
                        if tx.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(n, "SubscribeTokenStream subscriber lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            tracing::debug!("SubscribeTokenStream: client disconnected");
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn cancel_turn(
        &self,
        request: Request<CancelTurnRequest>,
    ) -> Result<Response<CancelTurnResponse>, Status> {
        let this = self.route(&request).await?;
        let node_id = request.into_inner().node_id;
        let tokens = this.inner.turn_tokens.lock().await;
        if let Some(token) = tokens.get(&node_id) {
            token.cancel();
            tracing::info!(node_id, "ai-chat turn cancelled");
        }
        Ok(Response::new(CancelTurnResponse {}))
    }

    async fn get_status(
        &self,
        request: Request<GetLocalStatusRequest>,
    ) -> Result<Response<LocalAgentStatusResponse>, Status> {
        // Routed: idle-vs-streaming is a fact about the targeted database's
        // turns. The model identity and window below come from the
        // process-global engine and read the same from any database.
        let this = self.route(&request).await?;
        // Scoped so the turn map is not held across the reads below: they are
        // process-global state this database's turns never touch, and holding
        // its map meanwhile would block that database starting or finishing a
        // turn for the length of a status poll.
        let status = {
            let tokens = this.inner.turn_tokens.lock().await;
            if tokens.is_empty() {
                LocalAgentStatus::Idle
            } else {
                LocalAgentStatus::Streaming
            }
        };
        let status_json = serde_json::to_string(&status)
            .map_err(|e| Status::internal(format!("Failed to serialize status: {e}")))?;

        // Report the loaded model's real geometry alongside the activity status,
        // from the snapshot taken at engine-swap time. Deliberately NOT queried
        // live: `model_spec()` reaches a `std::sync::Mutex` held for the whole
        // of a generation, so a live call would block a tokio worker for the
        // length of a turn — the exact hang a status poller would trip over.
        let spec = self.inner.shared.loaded_model_spec.lock().await.clone();
        // Report the catalog id the model was loaded BY, not the resolved GGUF
        // path the engine reports. Callers compare this against the id they
        // asked for ("gemma-4-e4b-q4km"), which no path substring matches.
        let active_model_id = self.inner.shared.active_model_id.lock().await.clone();
        // The id and the window degrade independently. A snapshot that failed
        // or timed out costs the window, not the identity: `active_model_id` is
        // set by the same swap and is still authoritative, so a loaded model
        // keeps reporting itself with `granted_n_ctx == 0` (window unknown)
        // rather than vanishing into "nothing loaded".
        let (model_id, granted_n_ctx) = match spec {
            Some(spec) => (
                active_model_id.unwrap_or(spec.model_id),
                spec.context_window,
            ),
            None => (active_model_id.unwrap_or_default(), 0),
        };

        Ok(Response::new(LocalAgentStatusResponse {
            status_json,
            model_id,
            granted_n_ctx,
        }))
    }

    type EnsureModelReadyStream = ReceiverStream<Result<ModelLoadProgressEvent, Status>>;

    async fn ensure_model_ready(
        &self,
        request: Request<EnsureModelReadyRequest>,
    ) -> Result<Response<Self::EnsureModelReadyStream>, Status> {
        let model_id = request.into_inner().model_id;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ModelLoadProgressEvent, Status>>(16);

        // Cloned so the load runs in its own task and streams events to `tx`
        // as each phase happens, rather than the caller awaiting the whole
        // load before anything is sent (which is what made a slow phase like
        // integrity verification read as a frozen "preparing model" — no
        // event arrived until it was already over).
        let this = self.clone();
        let tx_for_panic = tx.clone();
        let model_id_for_panic = model_id.clone();
        let join_handle = tokio::spawn(async move {
            this.load_model_and_collect_events(&model_id, Some(&tx))
                .await;
        });
        tokio::spawn(async move {
            // A detached task's panic does not propagate to this stream —
            // unlike the caller `.await`-ing the load directly (the
            // pre-refactor shape), a panic here would otherwise just drop
            // `tx` and close the stream with no terminal event. The Tauri
            // command on the other end treats a closed stream with no
            // "ready"/"error" event as a *successful* (inert) call, so a
            // silent close would misreport a crash as success. Surface it
            // as an explicit "error" event instead.
            if let Err(join_err) = join_handle.await {
                let _ = tx_for_panic
                    .send(Ok(ModelLoadProgressEvent {
                        event_type: "error".to_string(),
                        model_id: model_id_for_panic,
                        error_message: Some(format!("Model load task failed: {join_err}")),
                        ..Default::default()
                    }))
                    .await;
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn list_models(
        &self,
        request: Request<ListModelsRequest>,
    ) -> Result<Response<ListModelsResponse>, Status> {
        let force_refresh = request.into_inner().force_refresh;

        // A missing local model manager contributes no GGUF catalog rows rather
        // than failing the whole listing — the OpenAI-compatible catalog below
        // is independent of it and must still be usable.
        let mut models = match self.model_manager() {
            Some(manager) => manager
                .list()
                .await
                .map_err(|e| Status::internal(format!("Failed to list models: {e}")))?,
            None => Vec::new(),
        };

        models.extend(self.discover_openai_compat_models(force_refresh).await);

        let entries = models
            .into_iter()
            .map(|m| {
                let status_json = serde_json::to_string(&m.status).unwrap_or_default();
                let backend = m.backend.as_wire_str().to_string();
                ModelEntry {
                    id: m.id,
                    name: m.name,
                    backend,
                    status_json,
                    size_bytes: m.size_bytes as i64,
                    quantization: m.quantization,
                    min_memory_gb: m.min_memory_gb as u32,
                }
            })
            .collect();

        Ok(Response::new(ListModelsResponse { models: entries }))
    }

    type DownloadModelStream = ReceiverStream<Result<ModelLoadProgressEvent, Status>>;

    async fn download_model(
        &self,
        request: Request<DownloadModelRequest>,
    ) -> Result<Response<Self::DownloadModelStream>, Status> {
        let model_id = request.into_inner().model_id;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ModelLoadProgressEvent, Status>>(16);

        let model_id_clone = model_id.clone();
        let manager = self
            .model_manager()
            .ok_or_else(|| Status::unavailable(MODEL_MANAGER_UNAVAILABLE))?
            .clone();

        let tx_gguf = tx.clone();
        let mid_gguf = model_id.clone();
        manager
            .set_progress_callback(
                &model_id,
                Box::new(move |evt| {
                    let event = ModelLoadProgressEvent {
                        event_type: "downloading".to_string(),
                        model_id: mid_gguf.clone(),
                        bytes_downloaded: Some(evt.bytes_downloaded as i64),
                        bytes_total: Some(evt.bytes_total as i64),
                        ..Default::default()
                    };
                    let _ = tx_gguf.try_send(Ok(event));
                }),
            )
            .await;

        tokio::spawn(async move {
            match manager.download(&model_id_clone).await {
                Ok(()) => {
                    let _ = tx
                        .send(Ok(ModelLoadProgressEvent {
                            event_type: "ready".to_string(),
                            model_id: model_id_clone.clone(),
                            ..Default::default()
                        }))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(Ok(ModelLoadProgressEvent {
                            event_type: "error".to_string(),
                            model_id: model_id_clone.clone(),
                            error_message: Some(e.to_string()),
                            ..Default::default()
                        }))
                        .await;
                }
            }
            drop(tx);

            // Each progress callback holds a `Sender` clone, so the stream
            // above only closes (and the Tauri command awaiting it only
            // returns) once those clones are also dropped. Without this, a
            // completed download's channel is kept open indefinitely by its
            // own now-unused callback, hanging the frontend's await forever.
            // Cleared by model_id, not wholesale, so a concurrent download of
            // a different model is unaffected.
            manager.clear_progress_callback(&model_id_clone).await;
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn delete_model(
        &self,
        request: Request<DeleteModelRequest>,
    ) -> Result<Response<DeleteModelResponse>, Status> {
        let model_id = request.into_inner().model_id;

        // Independent of the model manager's own loaded-guard below: that
        // guard only compares against its own catalog bookkeeping
        // (`loaded_model_id`), which can lag the actual engine on
        // `SharedLocalAgent` -- the engine only swaps lazily, on next
        // inference use. `active_model_id` names whichever model that real
        // engine was last swapped in for, so checking it here catches a
        // still-resident engine even if some future path moves the catalog's
        // own bookkeeping off this model_id without swapping the engine too.
        //
        // Check-then-act, not atomic with the delete below: a concurrent
        // `load_model` for this same id could re-activate it in the gap.
        // Accepted rather than closed with a lock spanning both calls --
        // `GgufModelManager::delete`'s own catalog guard has the identical
        // shape, and closing only this one instance would still leave a
        // caller racing `load_model` against `delete_model` with no true
        // fix; that needs its own design, not a narrower patch here.
        if self.inner.shared.active_model_id.lock().await.as_deref() == Some(model_id.as_str()) {
            return Err(Status::failed_precondition(format!(
                "cannot delete model '{model_id}' while it is the active inference engine"
            )));
        }

        // `model_manager()` is re-checked here (not reused from the guard
        // above, which reads only `active_model_id` and never touches the
        // manager) so `delete` still runs through its own catalog guard --
        // the two guards are independent checks against independent state,
        // not a single check split in two.
        self.model_manager()
            .ok_or_else(|| Status::unavailable(MODEL_MANAGER_UNAVAILABLE))?
            .delete(&model_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to delete model: {e}")))?;
        Ok(Response::new(DeleteModelResponse {}))
    }

    async fn load_model(
        &self,
        request: Request<LoadModelRequest>,
    ) -> Result<Response<LoadModelResponse>, Status> {
        let model_id = request.into_inner().model_id;
        // Checked up front so a missing model manager reports the specific
        // `MODEL_MANAGER_UNAVAILABLE` status, matching every other model-
        // management RPC, rather than falling through into
        // `load_model_and_collect_events`'s own (differently-worded) internal
        // check and error event.
        self.model_manager()
            .ok_or_else(|| Status::unavailable(MODEL_MANAGER_UNAVAILABLE))?;
        // Drive the same eager load-and-swap path `EnsureModelReady` streams
        // (`load_model_and_collect_events` with no live sender just awaits the
        // whole load and returns its events). `GgufModelManager::load()` alone
        // only flips catalog bookkeeping -- the actual `SharedLocalAgent`
        // engine would otherwise only swap lazily, on the next inference use.
        // That gap let `load_model(B)` mark B as loaded in the catalog while
        // the resident engine was still A's, so a following `delete_model(A)`
        // read the catalog's loaded-guard as clear and deleted a file the
        // engine still referenced. Loading the real engine synchronously here,
        // before the catalog is marked loaded, closes that window.
        let events = self.load_model_and_collect_events(&model_id, None).await;
        match events.last() {
            Some(event) if event.event_type == "error" => Err(Status::internal(format!(
                "Failed to load model: {}",
                event
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "unknown error".to_string())
            ))),
            Some(event) if event.event_type == "ready" => Ok(Response::new(LoadModelResponse {})),
            _ => Err(Status::internal(
                "Failed to load model: no terminal event produced",
            )),
        }
    }

    async fn unload_model(
        &self,
        _request: Request<UnloadModelRequest>,
    ) -> Result<Response<UnloadModelResponse>, Status> {
        self.model_manager()
            .ok_or_else(|| Status::unavailable(MODEL_MANAGER_UNAVAILABLE))?
            .unload()
            .await
            .map_err(|e| Status::internal(format!("Failed to unload model: {e}")))?;
        // model_manager().unload() above only flips catalog bookkeeping
        // (loaded_model_id / status). It has no reference to the actual
        // engine, which lives separately on `shared` -- without this, the
        // multi-GB engine stays resident and keeps serving turns even
        // though the catalog now reports the model as unloaded.
        self.inner.shared.reset_to_noop_engine().await;
        Ok(Response::new(UnloadModelResponse {}))
    }

    async fn cancel_model_download(
        &self,
        request: Request<CancelModelDownloadRequest>,
    ) -> Result<Response<CancelModelDownloadResponse>, Status> {
        let model_id = request.into_inner().model_id;
        self.model_manager()
            .ok_or_else(|| Status::unavailable(MODEL_MANAGER_UNAVAILABLE))?
            .cancel_download(&model_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to cancel download: {e}")))?;
        Ok(Response::new(CancelModelDownloadResponse {}))
    }

    async fn recommended_model(
        &self,
        _request: Request<RecommendedModelRequest>,
    ) -> Result<Response<RecommendedModelResponse>, Status> {
        let model_id = self
            .model_manager()
            .ok_or_else(|| Status::unavailable(MODEL_MANAGER_UNAVAILABLE))?
            .recommended_model()
            .await
            .map_err(|e| Status::internal(format!("Failed to get recommended model: {e}")))?;
        Ok(Response::new(RecommendedModelResponse { model_id }))
    }

    async fn get_system_ram(
        &self,
        _request: Request<GetSystemRamRequest>,
    ) -> Result<Response<GetSystemRamResponse>, Status> {
        let ram_bytes = nodespace_agent::local_agent::model_manager::detect_system_ram();
        Ok(Response::new(GetSystemRamResponse { ram_bytes }))
    }
}

impl LocalAgentServiceImpl {
    /// Query every configured OpenAI-compatible endpoint for the models it
    /// serves, as catalog rows — through a short-TTL cache
    /// (`OPENAI_COMPAT_DISCOVERY_CACHE_TTL`) so repeated `ListModels` calls
    /// (three frontend call sites hit it on mount) don't each re-pay the
    /// endpoint round trip.
    ///
    /// `force_refresh` bypasses a fresh cache hit — set by the explicit
    /// "Refresh remote models" action in Settings — but a stale or absent
    /// cache is always queried live regardless of this flag.
    ///
    /// Endpoints are queried concurrently: they are independent network calls,
    /// and the model selector awaits this whole listing before it can render.
    /// An endpoint that is unreachable or misconfigured contributes nothing
    /// rather than failing the catalog — a user with one dead provider must
    /// still see the models from every other one.
    ///
    /// No single-flight de-duplication on a concurrent cache miss: the lock is
    /// released before the network round trip (so a slow discovery round never
    /// blocks a concurrent cache *hit*), which means two callers racing a cold
    /// cache each run their own discovery round rather than one waiting on the
    /// other's in-flight result. Accepted, not overlooked — it only costs an
    /// extra fan-out on the very first mount; every call after either round
    /// completes is a cache hit.
    async fn discover_openai_compat_models(
        &self,
        force_refresh: bool,
    ) -> Vec<nodespace_agent::agent_types::ModelInfo> {
        {
            let cache = self.inner.shared.openai_compat_discovery_cache.lock().await;
            if let Some((fetched_at, models)) = cache.as_ref() {
                if !force_refresh && fetched_at.elapsed() < OPENAI_COMPAT_DISCOVERY_CACHE_TTL {
                    return models.clone();
                }
            }
        }

        let discovered = self.discover_openai_compat_models_uncached().await;

        let mut cache = self.inner.shared.openai_compat_discovery_cache.lock().await;
        *cache = Some((std::time::Instant::now(), discovered.clone()));
        discovered
    }

    /// The actual discovery round `discover_openai_compat_models` caches.
    async fn discover_openai_compat_models_uncached(
        &self,
    ) -> Vec<nodespace_agent::agent_types::ModelInfo> {
        use nodespace_agent::local_agent::openai_compat_discovery::{
            discover_models_or_empty, discovered_model_info,
        };

        let configs = match crate::services::settings_service::load_openai_compat_configs(
            &self.inner.shared.daemon_config_path,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "failed to read OpenAI-compat configs for discovery");
                return Vec::new();
            }
        };

        let mut lookups = tokio::task::JoinSet::new();
        for config in configs {
            lookups.spawn(async move {
                let models = discover_models_or_empty(&config.base_url, &config.api_key).await;
                models
                    .into_iter()
                    .map(|model| discovered_model_info(&config.id, &config.name, &model))
                    .collect::<Vec<_>>()
            });
        }

        let mut discovered = Vec::new();
        while let Some(result) = lookups.join_next().await {
            match result {
                Ok(rows) => discovered.extend(rows),
                Err(e) => tracing::warn!(error = %e, "OpenAI-compat discovery task failed"),
            }
        }
        discovered
    }

    /// Load a model, both collecting every emitted event (for callers that
    /// want the full sequence, e.g. tests) and — when `live_tx` is provided —
    /// sending each event on it as soon as it happens. Phases like model
    /// download already report progress live via their own callback
    /// (`download`'s `set_progress_callback`); `live_tx` is what lets a
    /// caller *also* see the "verifying" / "loading" / "ready" phase
    /// transitions in real time instead of only after the whole load
    /// finishes, which is what made "preparing model" read as a hang when
    /// integrity verification took minutes (see `EnsureModelReady`'s use
    /// below).
    async fn load_model_and_collect_events(
        &self,
        model_id: &str,
        live_tx: Option<&tokio::sync::mpsc::Sender<Result<ModelLoadProgressEvent, Status>>>,
    ) -> Vec<ModelLoadProgressEvent> {
        use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
        use nodespace_agent::local_agent::openai_compat_inference::{
            is_openai_compat, parse_openai_compat_id, OpenAiCompatInferenceEngine,
        };
        use nodespace_nlp_engine::chat::ChatConfig;

        let mut events = Vec::new();
        macro_rules! emit {
            ($event:expr) => {{
                let event = $event;
                if let Some(tx) = live_tx {
                    let _ = tx.send(Ok(event.clone())).await;
                }
                events.push(event);
            }};
        }

        // OpenAI-compat configs are user-defined (stored in daemon.toml), not part
        // of the model catalog `list()` returns — resolve and branch on them first
        // so they never fall through to the "Unknown model" / GGUF path below.
        if is_openai_compat(model_id) {
            // A discovered model carries its own identifier after the config
            // UUID; without one, fall back to the config's pinned `model`.
            let (config_id, discovered_model) = parse_openai_compat_id(model_id);

            emit!(ModelLoadProgressEvent {
                event_type: "loading".to_string(),
                model_id: model_id.to_string(),
                message: Some("Connecting to OpenAI-compatible endpoint...".to_string()),
                ..Default::default()
            });

            let config = match crate::services::settings_service::find_openai_compat_config(
                &self.inner.shared.daemon_config_path,
                config_id,
            )
            .await
            {
                Ok(Some(c)) => c,
                Ok(None) => {
                    emit!(ModelLoadProgressEvent {
                        event_type: "error".to_string(),
                        model_id: model_id.to_string(),
                        error_message: Some(format!(
                            "No OpenAI-compatible provider config found for '{config_id}'. Check Settings > Integrations."
                        )),
                        ..Default::default()
                    });
                    return events;
                }
                Err(e) => {
                    emit!(ModelLoadProgressEvent {
                        event_type: "error".to_string(),
                        model_id: model_id.to_string(),
                        error_message: Some(format!(
                            "Failed to read OpenAI-compatible provider config: {e}"
                        )),
                        ..Default::default()
                    });
                    return events;
                }
            };

            // `model` is the wire-protocol identifier (e.g. "gpt-4o" or
            // "mistral:7b"); `name` is only a cosmetic UI label and must never
            // be sent as the request's "model" field — real OpenAI-API and
            // multi-model servers reject or misroute an arbitrary display
            // string. A model discovered via /models wins over the config's
            // pinned value, since it names one of several models the same
            // endpoint serves.
            let model = match discovered_model {
                Some(m) => m.to_string(),
                None if !config.model.is_empty() => config.model.clone(),
                // Only for configs created before the field existed;
                // single-model local servers generally ignore it.
                None => "default".to_string(),
            };
            let engine = Arc::new(OpenAiCompatInferenceEngine::new(
                config.base_url.clone(),
                config.api_key.clone(),
                model.clone(),
            ));
            let swapped = self
                .replace_engine_if_changed(model_id, engine.clone())
                .await;

            // Routing-reliability probe (Option C): the matrix in
            // `tests/live_openai_compat_routing.rs` found Stage-2 candidate
            // injection suppresses tool-calling on some served models,
            // independent of the block's content, and that this is a
            // per-model property rather than a native-vs-served split. Run
            // once per (base_url, model) and cache the verdict on the config
            // rather than paying a generation on every ambiguous turn.
            //
            // Keyed on the resolved `(base_url, model)` pair, NOT on `swapped`.
            // A single-model server config with no `/models` discovery
            // resolves to the same `model_id` regardless of the config's
            // `model` field, so editing that field in Settings can change the
            // served model without `replace_engine_if_changed` seeing a
            // change — `swapped` would then wrongly read as "still the model
            // last probed."
            let this_key = (config.base_url.clone(), model.clone());
            let cached_key_matches =
                *self.inner.shared.active_model_routing_key.lock().await == Some(this_key.clone());
            // `None` here means "the probe did not run / could not complete
            // this load" — distinct from a `Some(false)` verdict. Only a
            // `Some` result updates `active_model_routing_key`, so an errored
            // probe is retried on the very next load rather than being
            // mistaken later for an already-probed model.
            let verdict: Option<bool> = if !swapped && cached_key_matches {
                // Same engine AND the same (base_url, model) already probed
                // this load — the cached verdict on `self.inner` still
                // describes this model; no need to touch disk or re-probe.
                let disabled = *self.inner.shared.active_model_routing_disabled.lock().await;
                Some(!disabled)
            } else if let Some(cached) = config.routing_ok.get(&model).copied() {
                // Keyed by the served model, not the config as a whole — one
                // config can discover several models (see the field's doc
                // comment on `OpenAiCompatConfig::routing_ok`), each with its
                // own independent verdict.
                Some(cached)
            } else {
                let probe_result = match tokio::time::timeout(
                    ROUTING_PROBE_TIMEOUT,
                    nodespace_agent::local_agent::routing_probe::probe_routing_ok(engine.as_ref()),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(nodespace_agent::agent_types::InferenceError::Engine(
                        format!("routing probe exceeded {ROUTING_PROBE_TIMEOUT:?}"),
                    )),
                };
                match probe_result {
                    Ok(routing_ok) => {
                        if !routing_ok {
                            tracing::warn!(
                                model_id,
                                base_url = %config.base_url,
                                served_model = %model,
                                "Routing probe: Stage-2 candidate injection suppresses \
                                 tool-calling on this served model. Disabling injection for \
                                 this model; routing falls back to the full tool surface."
                            );
                        }
                        if let Err(e) =
                            crate::services::settings_service::record_routing_probe_verdict(
                                &self.inner.shared.daemon_config_path,
                                config_id,
                                &config.base_url,
                                &model,
                                routing_ok,
                            )
                            .await
                        {
                            tracing::warn!(
                                error = %e,
                                model_id,
                                "failed to persist routing probe verdict; will re-probe next load"
                            );
                        }
                        Some(routing_ok)
                    }
                    Err(e) => {
                        // An unmeasured model is not the same as a suppressed
                        // one — do not cache a guess, and do not disable
                        // routing on an engine error that says nothing about
                        // whether injection is safe.
                        tracing::warn!(
                            error = %e,
                            model_id,
                            "routing probe failed to complete; leaving routing enabled for this \
                             load"
                        );
                        None
                    }
                }
            };
            let routing_disabled = !verdict.unwrap_or(true);
            *self.inner.shared.active_model_routing_disabled.lock().await = routing_disabled;
            *self.inner.shared.active_model_routing_key.lock().await =
                verdict.map(|_| this_key.clone());

            emit!(ModelLoadProgressEvent {
                event_type: "ready".to_string(),
                model_id: model_id.to_string(),
                message: Some(format!("{} ready", config.name)),
                engine_swapped: Some(swapped),
                ..Default::default()
            });

            return events;
        }

        let manager = match self.model_manager() {
            Some(m) => m,
            None => {
                emit!(ModelLoadProgressEvent {
                    event_type: "error".to_string(),
                    model_id: model_id.to_string(),
                    error_message: Some(MODEL_MANAGER_UNAVAILABLE.to_string()),
                    ..Default::default()
                });
                return events;
            }
        };

        let models = match manager.list().await {
            Ok(m) => m,
            Err(e) => {
                emit!(ModelLoadProgressEvent {
                    event_type: "error".to_string(),
                    model_id: model_id.to_string(),
                    error_message: Some(e.to_string()),
                    ..Default::default()
                });
                return events;
            }
        };

        let model = match models.iter().find(|m| m.id == model_id) {
            Some(m) => m,
            None => {
                emit!(ModelLoadProgressEvent {
                    event_type: "error".to_string(),
                    model_id: model_id.to_string(),
                    error_message: Some(format!("Unknown model: {model_id}")),
                    ..Default::default()
                });
                return events;
            }
        };

        {
            let active = self.inner.shared.active_model_id.lock().await;
            if active.as_deref() == Some(model_id) {
                emit!(ModelLoadProgressEvent {
                    event_type: "ready".to_string(),
                    model_id: model_id.to_string(),
                    message: Some(format!("{model_id} already loaded")),
                    engine_swapped: Some(false),
                    ..Default::default()
                });
                return events;
            }
        }

        match &model.status {
            ModelStatus::Loaded | ModelStatus::Ready => {}
            ModelStatus::NotDownloaded | ModelStatus::Error { .. } => {
                emit!(ModelLoadProgressEvent {
                    event_type: "downloading".to_string(),
                    model_id: model_id.to_string(),
                    message: Some(format!("Downloading {model_id}...")),
                    ..Default::default()
                });

                if let Err(e) = manager.download(model_id).await {
                    emit!(ModelLoadProgressEvent {
                        event_type: "error".to_string(),
                        model_id: model_id.to_string(),
                        error_message: Some(format!("Download failed: {e}")),
                        ..Default::default()
                    });
                    return events;
                }
            }
            ModelStatus::Downloading { .. } | ModelStatus::Verifying => {
                emit!(ModelLoadProgressEvent {
                    event_type: "error".to_string(),
                    model_id: model_id.to_string(),
                    error_message: Some(format!(
                        "Model '{model_id}' is currently being downloaded"
                    )),
                    ..Default::default()
                });
                return events;
            }
        }

        let model_path = match manager.model_path(model_id) {
            Ok(p) => p,
            Err(e) => {
                emit!(ModelLoadProgressEvent {
                    event_type: "error".to_string(),
                    model_id: model_id.to_string(),
                    error_message: Some(format!("Failed to resolve model path: {e}")),
                    ..Default::default()
                });
                return events;
            }
        };

        // Only announce a distinct "verifying" phase when a real hash is
        // about to run (cache miss) — this is the case that used to read as
        // a frozen "preparing model" for minutes. A cache hit resolves
        // near-instantly inside `ChatEngine::load_model` below, so it is
        // folded into "loading" rather than flashing a phase the user has no
        // real time to see.
        if let Some(expected_sha256) = &model.sha256 {
            if !nodespace_nlp_engine::config::is_verification_cached(&model_path, expected_sha256) {
                emit!(ModelLoadProgressEvent {
                    event_type: "verifying".to_string(),
                    model_id: model_id.to_string(),
                    message: Some(format!("Verifying {model_id} integrity...")),
                    ..Default::default()
                });
            }
        }

        emit!(ModelLoadProgressEvent {
            event_type: "loading".to_string(),
            model_id: model_id.to_string(),
            message: Some(format!("Loading {model_id}...")),
            ..Default::default()
        });

        let (family, chat_config) = match manager.model_spec_for(model_id) {
            Ok(spec) => {
                let config = ChatConfig {
                    n_ctx: spec.context_window,
                    default_temperature: spec.default_temperature,
                    type_k: spec.type_k,
                    type_v: spec.type_v,
                    ..ChatConfig::default()
                };
                (spec.family, config)
            }
            Err(e) => {
                emit!(ModelLoadProgressEvent {
                    event_type: "error".to_string(),
                    model_id: model_id.to_string(),
                    error_message: Some(format!("Failed to look up model spec: {e}")),
                    ..Default::default()
                });
                return events;
            }
        };

        let model_path_str = model_path.to_string_lossy().to_string();
        let engine_result = tokio::task::spawn_blocking(move || {
            LlamaChatInferenceEngine::load(&model_path_str, family, chat_config)
        })
        .await;

        let engine = match engine_result {
            Ok(Ok(e)) => e,
            Ok(Err(e)) => {
                emit!(ModelLoadProgressEvent {
                    event_type: "error".to_string(),
                    model_id: model_id.to_string(),
                    error_message: Some(format!("Failed to load inference engine: {e}")),
                    ..Default::default()
                });
                return events;
            }
            Err(e) => {
                emit!(ModelLoadProgressEvent {
                    event_type: "error".to_string(),
                    model_id: model_id.to_string(),
                    error_message: Some(format!("Task join error: {e}")),
                    ..Default::default()
                });
                return events;
            }
        };

        if let Err(e) = manager.load(model_id).await {
            emit!(ModelLoadProgressEvent {
                event_type: "error".to_string(),
                model_id: model_id.to_string(),
                error_message: Some(format!("Failed to mark model as loaded: {e}")),
                ..Default::default()
            });
            return events;
        }

        self.replace_engine(Arc::new(engine)).await;
        *self.inner.shared.active_model_id.lock().await = Some(model_id.to_string());
        // Native/GGUF path: never probed here (see the field's doc comment),
        // and a previous session's OpenAI-compat probe verdict must not leak
        // onto this model.
        *self.inner.shared.active_model_routing_disabled.lock().await = false;
        *self.inner.shared.active_model_routing_key.lock().await = None;

        emit!(ModelLoadProgressEvent {
            event_type: "ready".to_string(),
            model_id: model_id.to_string(),
            message: Some(format!("{model_id} ready")),
            engine_swapped: Some(true),
            ..Default::default()
        });

        events
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn streaming_chunk_to_proto(chunk: StreamingChunk) -> AgentChunk {
    match chunk {
        StreamingChunk::Token { text } => AgentChunk {
            chunk_type: "token".to_string(),
            token_text: Some(text),
            ..Default::default()
        },
        // Reasoning is not streamed live (filtered out before this function in the
        // send callback); it reaches the UI via the persisted message. This arm
        // exists only for exhaustiveness and tags the chunk so any future live
        // consumer can distinguish/ignore it rather than render it as the answer.
        StreamingChunk::Reasoning { text } => AgentChunk {
            chunk_type: "reasoning".to_string(),
            token_text: Some(text),
            ..Default::default()
        },
        StreamingChunk::ToolCallStart { id, name, .. } => AgentChunk {
            chunk_type: "tool_call_start".to_string(),
            tool_call_id: Some(id),
            tool_name: Some(name),
            ..Default::default()
        },
        StreamingChunk::ToolCallArgs { id, args_json } => AgentChunk {
            chunk_type: "tool_call_args".to_string(),
            tool_call_id: Some(id),
            tool_args_json: Some(args_json),
            ..Default::default()
        },
        StreamingChunk::Done { usage } => AgentChunk {
            chunk_type: "done".to_string(),
            prompt_tokens: Some(usage.prompt_tokens as i32),
            completion_tokens: Some(usage.completion_tokens as i32),
            ..Default::default()
        },
        StreamingChunk::Error { message } => AgentChunk {
            chunk_type: "error".to_string(),
            error_message: Some(message),
            ..Default::default()
        },
    }
}

/// Maximum length of an evidence summary before it is clipped.
const SUMMARY_MAX_CHARS: usize = 120;

/// Which argument identifies the thing a write acted on, per tool.
///
/// An explicit mapping rather than a probe across likely key names: the write
/// tools do not share one argument shape, and a probe silently degrades to a
/// bare tool name for the ones it does not happen to cover. That is worst
/// exactly where the evidence matters most — `create_nodes_from_markdown`
/// imports a whole subtree, so a repeat duplicates all of it.
///
/// Returning `None` here means "this tool changes graph state but is not
/// described by a single argument"; `create_relationship` is rendered from its
/// own fields instead (see `completed_writes_from`).
fn write_summary_arg(tool: &str) -> Option<&'static [&'static str]> {
    match tool {
        "create_node" | "update_node" | "update_task_status" | "delete_node" => {
            Some(&["content", "title", "id", "node_id"])
        }
        // `create_schema` is keyed on the display name it declares; `update_schema`
        // identifies its target by `schema_id` and has no top-level `name` (the
        // only `name` keys sit nested inside `add_fields[]`, out of reach here).
        "create_schema" => Some(&["name"]),
        "update_schema" => Some(&["schema_id"]),
        "create_nodes_from_markdown" => Some(&["markdown"]),
        // `create_relationship` has no single describing argument; the call site
        // renders the edge from its own fields instead.
        _ => None,
    }
}

/// Clip an evidence label, marking it when clipped so a truncated summary is
/// not mistaken for a complete one.
fn clip_summary(s: &str) -> String {
    // Newlines would let user-supplied content shape the evidence block's
    // line structure; the label is a single line by construction.
    let flat = s.replace(['\n', '\r'], " ");
    if flat.chars().count() > SUMMARY_MAX_CHARS {
        let head: String = flat.chars().take(SUMMARY_MAX_CHARS).collect();
        format!("{head}…")
    } else {
        flat
    }
}

/// Pull the successful graph writes out of a turn's tool executions.
///
/// Failed calls are excluded: a write that errored did not happen, and recording
/// it would tell the next turn not to retry work that never landed.
pub fn completed_writes_from(executions: &[ToolExecutionRecord]) -> Vec<AiChatCompletedWrite> {
    executions
        .iter()
        .filter(|r| !r.is_error && is_write_tool(&r.name))
        .map(|r| {
            // Every write tool that reports an affected node does so under `id`
            // (as a `nodespace://` URI — the same form the model uses to refer to
            // nodes elsewhere, so the evidence matches what it already reads).
            // Relationship writes report no node id at all. Schema writes report
            // no `id` either, but `create_schema`/`update_schema` return the
            // schema's own identifier — the same string a later `create_node`
            // call must copy into `node_type` — so it is captured here under
            // the same field rather than left blank; a terse fact built from
            // this record needs that id to reference the schema by the name a
            // later turn will actually use.
            //
            // The key is `schemaId`, not `schema_id`: `CreateSchemaOutput` and
            // `SchemaUpdateOutput` both carry
            // `#[serde(rename_all = "camelCase")]`, so camelCase is what
            // reaches the wire. The snake_case spelling this used to look for
            // matched nothing, silently degrading every schema write's history
            // to the id-less phrasing of `terse_write_fact`.
            //
            // Only the camelCase spelling is accepted. `result` is populated
            // exclusively by `exec_create_schema`/`exec_update_schema`, which
            // serialize those structs, so a snake_case key has no producer —
            // tolerating one would encode a false claim about the wire format.
            let node_id = r
                .result
                .get("id")
                .or_else(|| r.result.get("schemaId"))
                .and_then(|v| v.as_str())
                .map(str::to_string);

            // A label makes the evidence self-describing, so the model can match
            // it against the instruction still in history.
            let summary = match write_summary_arg(&r.name) {
                Some(keys) => keys
                    .iter()
                    .find_map(|k| r.args.get(*k))
                    .and_then(|v| v.as_str())
                    .map(clip_summary),
                // A relationship has no single describing argument; render the
                // edge itself, which is what identifies it.
                None if r.name == "create_relationship" => {
                    let field = |k: &str| {
                        r.args
                            .get(k)
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string()
                    };
                    Some(clip_summary(&format!(
                        "{} -[{}]-> {}",
                        field("from_id"),
                        field("relationship_type"),
                        field("to_id")
                    )))
                }
                None => None,
            };

            // Identity for the cross-turn duplicate guard. Canonicalised through
            // the same function the per-turn detector uses, so the two agree on
            // what "the same call" means, then reduced to a storable identity —
            // verbatim when small, a digest when not. The execution path derives
            // the incoming call's identity the same way; that shared derivation
            // is what keeps the two comparable.
            let canonical_args = canonical_args_identity(&canonical_args(&r.args.to_string()));

            AiChatCompletedWrite {
                tool: r.name.clone(),
                node_id,
                summary,
                canonical_args,
            }
        })
        .collect()
}

/// Maximum distinct entities carried forward from one turn's reads.
///
/// A read-only turn can surface many nodes (a broad `search_nodes` call); only
/// the ones most likely to be the subject of a follow-up ("that", "it") are
/// worth the context cost of remembering. Capped by count, not by time — this
/// mirrors the per-turn recompute-from-history model the rest of the ephemeral
/// session already uses, rather than introducing a TTL concept nothing else
/// here has.
const MAX_RESOLVED_ENTITIES: usize = 20;

/// Pull the graph entities a turn's read-only tool calls surfaced.
///
/// The read-side counterpart to `completed_writes_from`. Reads never get a
/// structured record today — only the assistant's prose reply survives once
/// the ephemeral session ends — so a turn that reports "one task node" leaves
/// nothing for a follow-up ("update that") to resolve against. This derives
/// that record from the same tool-execution results `completed_writes_from`
/// already reads, filtered to tools whose results describe concrete nodes
/// (`Tool::resolves_entities`) instead of tools that write.
///
/// Failed calls are excluded, same as writes: a lookup that errored surfaced
/// nothing. Deduplicated by node id (last occurrence wins, so a later, more
/// specific mention of the same node overrides an earlier one) and capped to
/// `MAX_RESOLVED_ENTITIES` distinct entities, most-recent-first.
pub fn resolved_entities_from(executions: &[ToolExecutionRecord]) -> Vec<AiChatResolvedEntity> {
    // Every entity-resolving tool's per-node JSON uses the same `id`/`title`
    // key names (see `run_node_query`, `exec_get_node`, `exec_get_related_nodes`
    // in `nodespace-agent`), but `id` itself is not uniform: the raw
    // `get_node` payload carries a bare UUID while every list/query tool's
    // summary carries the `nodespace://`-prefixed URI form. Node identity
    // elsewhere in this file (`completed_writes_from`) is stored exactly as
    // the tool reported it rather than normalised, so the same choice is made
    // here — the node id is opaque to this function, just captured verbatim.
    let node_type_of = |v: &serde_json::Value| -> Option<String> {
        v.get("nodeType")
            .or_else(|| v.get("type"))
            .and_then(|t| t.as_str())
            .map(str::to_string)
    };
    let entity_from_node = |v: &serde_json::Value| -> Option<AiChatResolvedEntity> {
        let node_id = v.get("id").and_then(|id| id.as_str())?.to_string();
        let title = v.get("title").and_then(|t| t.as_str()).map(clip_summary);
        let node_type = node_type_of(v);
        Some(AiChatResolvedEntity {
            node_id,
            title,
            node_type,
        })
    };

    let mut entities: Vec<AiChatResolvedEntity> = Vec::new();
    for r in executions
        .iter()
        .filter(|r| !r.is_error && resolves_entities_tool(&r.name))
    {
        // `get_node` returns a single node object at the top level.
        // `search_nodes`/`get_related_nodes` nest a list under `"nodes"`,
        // `search_semantic` under `"results"`, and `resolve_query` returns
        // either a single resolved node (`"resolved": true`, fields at top
        // level) or a `"candidates"` list when ambiguous.
        if let Some(candidates) = r
            .result
            .get("nodes")
            .or_else(|| r.result.get("results"))
            .or_else(|| r.result.get("candidates"))
            .and_then(|v| v.as_array())
        {
            entities.extend(candidates.iter().filter_map(entity_from_node));
        } else if let Some(entity) = entity_from_node(&r.result) {
            entities.push(entity);
        }
    }

    // Dedup by node id, keeping the last (most recent) occurrence, then cap
    // to the most recent MAX_RESOLVED_ENTITIES distinct entities.
    let mut seen = std::collections::HashSet::new();
    let mut deduped: Vec<AiChatResolvedEntity> = Vec::new();
    for entity in entities.into_iter().rev() {
        if seen.insert(entity.node_id.clone()) {
            deduped.push(entity);
        }
    }
    deduped.truncate(MAX_RESOLVED_ENTITIES);
    deduped
}

/// Rebuild the duplicate-guard's view of earlier turns from persisted messages.
///
/// Filtering to the guarded tools here keeps the set small, since the
/// execution-path check applies the same restriction anyway. Every recorded
/// write carries an identity, so none are dropped.
fn prior_writes_from_history(messages: &[AiChatMessage]) -> Vec<PriorWrite> {
    messages
        .iter()
        .flat_map(|m| m.completed_writes.iter())
        .filter(|w| is_cross_turn_guarded_tool(&w.tool))
        .map(|w| PriorWrite {
            tool: w.tool.clone(),
            canonical_args: w.canonical_args.clone(),
            node_id: w.node_id.clone(),
            summary: w.summary.clone(),
        })
        .collect()
}

/// Render persisted writes as a system-role note for the rebuilt history.
///
/// The assistant's prose ("I have added X") is an unverifiable claim; a model
/// weighing it against the user's still-present instruction may re-execute. This
/// restores the missing half of the record — concrete proof, stated as
/// fact-of-record rather than as the model's own narration.
///
/// `Role::System`, deliberately, not `Role::Tool`. A tool-role message is
/// rendered with a `tool_call_id` and must be preceded by the assistant
/// tool-call turn it answers (see `chat_message_to_oai_value` in
/// `nlp-engine/src/chat/mod.rs`). Those tool calls are not persisted, so a
/// tool-role message here would be an orphan tool result — the shape the
/// summarization back-off in `agent_loop.rs` exists specifically to avoid,
/// and which can abort a turn with llama.cpp `ffi error -3`.
fn completed_writes_message(writes: &[AiChatCompletedWrite]) -> Option<ChatMessage> {
    if writes.is_empty() {
        return None;
    }
    let mut lines = String::from(
        "Record of graph writes already completed in the previous turn. \
         These are done — do not repeat them:\n",
    );
    for w in writes {
        lines.push_str(&format!("- {}", w.tool));
        if let Some(ref s) = w.summary {
            lines.push_str(&format!(" \"{s}\""));
        }
        if let Some(ref id) = w.node_id {
            lines.push_str(&format!(" -> {id}"));
        }
        lines.push('\n');
    }
    Some(ChatMessage::text(
        Role::System,
        lines.trim_end().to_string(),
    ))
}

/// Render persisted resolved entities as a system-role note for the rebuilt
/// history.
///
/// The read-side counterpart to `completed_writes_message`: a turn's prose
/// reply ("one task node") carries no node id, so a follow-up ("update that")
/// has nothing concrete to resolve against. This restates the same reads as a
/// fact-of-record the model can match a pronoun/ellipsis reference against,
/// the same way `completed_writes_message` lets it match "that" for a write.
///
/// `Role::System` for the same reason `completed_writes_message` uses it, not
/// `Role::Tool`: no tool-call turn precedes it in persisted history, so a
/// tool-role message here would be an orphan result.
fn resolved_entities_message(entities: &[AiChatResolvedEntity]) -> Option<ChatMessage> {
    if entities.is_empty() {
        return None;
    }
    let mut lines = String::from(
        "Record of graph entities looked up in the previous turn. If the \
         user's next message refers to one by pronoun or description \
         (\"that\", \"it\", \"the task\"), use the matching id below:\n",
    );
    for e in entities {
        lines.push_str(&format!("- {}", e.node_id));
        if let Some(ref t) = e.title {
            lines.push_str(&format!(" \"{t}\""));
        }
        if let Some(ref nt) = e.node_type {
            lines.push_str(&format!(" ({nt})"));
        }
        lines.push('\n');
    }
    Some(ChatMessage::text(
        Role::System,
        lines.trim_end().to_string(),
    ))
}

/// Load an ai-chat node's persisted messages.
///
/// The single read a turn makes of its own chat node. Both things a turn needs
/// from it — the rendered history and the completed-write record — derive from
/// these messages, so fetching once and deriving twice avoids a redundant read
/// per turn. Any failure is logged here, once, and yields no messages.
async fn load_chat_messages(node_service: &Arc<NodeService>, node_id: &str) -> Vec<AiChatMessage> {
    let node = match node_service.get_node(node_id).await {
        Ok(Some(n)) => n,
        Ok(None) => {
            tracing::warn!(node_id, "ai-chat node not found for history");
            return vec![];
        }
        Err(e) => {
            tracing::error!(node_id, error = %e, "failed to load ai-chat node for history");
            return vec![];
        }
    };

    match AiChatNode::from_node(node) {
        Ok(c) => c.messages,
        Err(e) => {
            tracing::warn!(node_id, error = %e, "node is not an ai-chat node");
            vec![]
        }
    }
}

/// Render a single completed write as a short "Fact: ..." statement, pulling
/// the field/property detail out of its canonicalised call arguments.
///
/// This is the terse-history replacement for an assistant turn's own prose.
/// Confirmed on the golden scenario-6 sequence
/// (`packages/agent/tests/golden_scenario6_sequence.rs`): history rendered as
/// short declarative facts ("Fact: a schema with id 'X' was created, with
/// fields Y (type) and Z (type).") keeps later turns emitting well-formed
/// tool calls; the model's own narrative reply to the same turn (paragraphs,
/// numbered lists, a question back to the user) reproducibly does not, at
/// matched token count — style is the measured driver, not raw size.
/// `canonical_args` already carries
/// everything a fact needs — the schema's field list, an instance's property
/// values — so this reads the structured write record rather than the
/// model's own account of it, which is both cheaper and cannot drift from
/// what was actually written.
///
/// Returns `None` for a write this function does not yet know how to phrase
/// as a fact (e.g. `create_relationship`, `create_nodes_from_markdown`); the
/// caller falls back to `w.summary` for those so no write goes unrecorded.
fn terse_write_fact(w: &AiChatCompletedWrite) -> Option<String> {
    // `canonical_args` is either the canonical call JSON verbatim or a
    // `sha256:`-prefixed digest of it (see `canonical_args_identity`) when the
    // call was too large to store — the digest form starts with that prefix
    // rather than `{`, so parsing it as JSON and getting nothing back is the
    // correct, silent fallback to the plain summary line below.
    let args: serde_json::Value = serde_json::from_str(&w.canonical_args).ok()?;

    match w.tool.as_str() {
        "create_schema" => {
            let name = w.summary.as_deref().unwrap_or("an entity type");
            let id = w.node_id.as_deref();
            let fields = args.get("fields").and_then(|v| v.as_array());
            let field_list = fields.map(|fs| {
                fs.iter()
                    .filter_map(|f| {
                        let fname = f.get("name")?.as_str()?;
                        let ftype = f.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                        Some(format!("{fname} ({ftype})"))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            });
            let subject = match id {
                Some(id) => format!("a schema with id '{id}' ('{name}')"),
                None => format!("a schema named '{name}'"),
            };
            Some(match field_list {
                Some(fl) if !fl.is_empty() => {
                    format!("Fact: {subject} was created, with fields {fl}.")
                }
                _ => format!("Fact: {subject} was created."),
            })
        }
        "update_schema" => {
            let id = w.node_id.as_deref().or(w.summary.as_deref())?;
            Some(format!("Fact: the schema '{id}' was updated."))
        }
        "create_node" => {
            let node_type = args.get("node_type").and_then(|v| v.as_str());
            let props = args
                .get("field_values")
                .and_then(|v| v.as_object())
                .filter(|o| !o.is_empty());
            let prop_list = props.map(|o| {
                o.iter()
                    .map(|(k, v)| format!("{k} {}", terse_value(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            });
            let title = w.summary.as_deref();
            let mut fact = String::from("Fact: ");
            match (node_type, title) {
                (Some(t), Some(title)) => {
                    fact.push_str(&format!("a {t} node was created with title '{title}'"))
                }
                (Some(t), None) => fact.push_str(&format!("a {t} node was created")),
                (None, Some(title)) => {
                    fact.push_str(&format!("a node titled '{title}' was created"))
                }
                (None, None) => fact.push_str("a node was created"),
            }
            if let Some(pl) = prop_list.filter(|s| !s.is_empty()) {
                fact.push_str(&format!(" and properties {pl}"));
            }
            if let Some(id) = w.node_id.as_deref() {
                fact.push_str(&format!(" (id {id})"));
            }
            fact.push('.');
            Some(fact)
        }
        "update_node" => {
            let id = w.node_id.as_deref()?;
            let props = args
                .get("field_values")
                .and_then(|v| v.as_object())
                .filter(|o| !o.is_empty());
            let prop_list = props.map(|o| {
                o.iter()
                    .map(|(k, v)| format!("{k} {}", terse_value(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            });
            Some(match prop_list.filter(|s| !s.is_empty()) {
                Some(pl) => format!("Fact: node {id} was updated with {pl}."),
                None => format!("Fact: node {id} was updated."),
            })
        }
        "update_task_status" => {
            let id = w.node_id.as_deref()?;
            let status = args.get("status").and_then(|v| v.as_str());
            Some(match status {
                Some(s) => format!("Fact: task {id} status was set to {s}."),
                None => format!("Fact: task {id} status was updated."),
            })
        }
        "delete_node" => {
            let id = w.node_id.as_deref()?;
            Some(format!("Fact: node {id} was deleted."))
        }
        _ => None,
    }
}

/// Render a JSON scalar/short value the way a terse fact states it — bare for
/// strings and numbers, without the quoting/braces that would make the fact
/// read like a data dump rather than a sentence.
fn terse_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// Render an assistant turn's completed writes as terse factual statements,
/// one line per write, falling back to the tool name and summary for any
/// write `terse_write_fact` does not have a phrasing for.
///
/// `None` when the turn made no writes (a read-only or conversational turn) —
/// callers fall back to the turn's own reply text in that case, since there is
/// no structured record to render instead.
fn terse_assistant_facts(writes: &[AiChatCompletedWrite]) -> Option<String> {
    if writes.is_empty() {
        return None;
    }
    let lines: Vec<String> = writes
        .iter()
        .map(|w| {
            terse_write_fact(w).unwrap_or_else(|| match w.summary.as_deref() {
                Some(s) => format!("Fact: {} completed (\"{s}\").", w.tool),
                None => format!("Fact: {} completed.", w.tool),
            })
        })
        .collect();
    Some(lines.join(" "))
}

/// Render persisted messages as the inference history for this turn.
///
/// Separate from `prior_writes_from_history` because `ChatMessage` has no room
/// for the per-message write record — the very erasure that let the original
/// duplicate through. Both read the same loaded messages.
///
/// A prior ASSISTANT turn's own reply text is narrative prose by construction
/// — the model's account of what it did, written for a person to read in the
/// moment. Confirmed on the golden scenario-6 sequence: feeding that prose
/// back in as history reproducibly degrades later tool-calling behavior at a
/// fraction of the size where the existing token-budget summarizer
/// (`maybe_summarize_history`, `agent_loop.rs`) ever triggers — the failure is
/// driven by style (narrative vs. terse-factual), not by raw token count. So
/// an assistant turn that completed graph writes is rendered here as terse
/// "Fact: ..." statements derived from those writes' own structured record
/// (`terse_assistant_facts`) instead of its verbatim reply. A turn with no
/// writes (a read-only answer, a clarifying question) carries no such record
/// to render, so its own text is kept — dropping it there would erase real
/// conversational content with nothing to replace it. User turns are never
/// touched: they are the user's own words, not the model's narration, and the
/// dilution effect this guards against was only ever measured against
/// assistant-authored prose.
pub fn node_history_from_messages(messages: Vec<AiChatMessage>) -> Vec<ChatMessage> {
    messages
        .into_iter()
        .flat_map(|m| {
            let role = match m.role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => return Vec::new(),
            };
            let content = if role == Role::Assistant {
                terse_assistant_facts(&m.completed_writes).unwrap_or(m.content)
            } else {
                m.content
            };
            let mut msg = ChatMessage::text(role, content);
            // Round-trip any persisted reasoning so reloaded history retains it.
            msg.reasoning = m.reasoning;
            // Follow the assistant turn with the record of what it actually wrote
            // and/or looked up, so the next turn can tell a completed instruction
            // from a pending one, and can resolve a pronoun back to a node id.
            let mut out = vec![msg];
            out.extend(completed_writes_message(&m.completed_writes));
            out.extend(resolved_entities_message(&m.resolved_entities));
            out
        })
        .collect()
}

/// Build the embedding query that schema retrieval runs for this turn.
///
/// Blends the preceding conversational turns with the current message, so a
/// follow-up that names its subject only by pronoun or ellipsis still matches
/// the schema the earlier turn introduced.
///
/// Only user/assistant turns are blended. `node_history_from_messages` also
/// interleaves the synthetic `completed_writes` records, which are fixed
/// boilerplate ("Record of graph writes already completed…") carrying no
/// discriminating vocabulary — embedding them would dilute the query.
fn schema_retrieval_query(prior_history: &[ChatMessage], user_message: &str) -> String {
    let prior_turns: Vec<&str> = prior_history
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
        .map(|m| m.content.as_str())
        .collect();
    nodespace_core::ops::context_ops::build_retrieval_query(&prior_turns, user_message)
}

async fn build_workspace_context(
    node_service: &Arc<NodeService>,
    embedding_service: Option<Arc<NodeEmbeddingService>>,
    query: Option<&str>,
) -> Result<String, ()> {
    let mut context = nodespace_core::ops::context_ops::build_workspace_context(
        node_service,
        embedding_service.as_ref(),
        query,
    )
    .await
    .map_err(|_| ())?;

    // Inject schemas created in the last 5 minutes that may not yet be indexed
    // in the embedding store (30s debounce). This ensures the model sees custom
    // types it just created when composing the next turn in the same session.
    //
    // Budgeted against `semantic_schema_count`, NOT `relevant_schemas.len()`.
    // `relevant_schemas` also receives entries from `append_schemas_named_in_query`
    // (a separate, unbounded lexical backstop inside `build_workspace_context`)
    // before this injector ever runs. Budgeting off the post-append length let
    // that backstop silently zero out `remaining_slots` on any turn naming
    // several schemas by name, starving this injector even though it had never
    // consumed a "slot" of its own — found in a post-merge audit.
    const MAX_SCHEMAS: usize = 5;
    if let Ok(all_schemas) = node_service.get_all_schemas().await {
        let cutoff = chrono::Utc::now() - chrono::Duration::minutes(5);
        let existing_ids: std::collections::HashSet<String> = context
            .relevant_schemas
            .iter()
            .map(|s| s.id.clone())
            .collect();
        let remaining_slots = MAX_SCHEMAS.saturating_sub(context.semantic_schema_count);
        let mut injected = 0;
        for schema in all_schemas {
            if injected >= remaining_slots {
                break;
            }
            if schema.is_core {
                continue; // skip built-in types
            }
            if existing_ids.contains(&schema.id) {
                continue; // already present from semantic search
            }
            if schema.created_at >= cutoff {
                tracing::debug!(
                    schema_id = %schema.id,
                    "workspace_context: injecting recently-created schema (debounce bypass)"
                );
                context.relevant_schemas.push(schema);
                injected += 1;
            }
        }
    }

    Ok(context.format_for_prompt(4000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_agent::local_agent::agent_loop::CANONICAL_ARGS_MAX_CHARS;
    use nodespace_core::models::Node;
    use nodespace_core::{NodeService as CoreNodeService, SqliteStore};

    /// A completed `create_schema` write must capture the new type's id.
    ///
    /// `completed_writes_from` reads the affected node's id from the tool
    /// RESULT, falling back to the schema's own identifier for schema writes
    /// (which report no `id`). That fallback has to spell the key the way the
    /// result actually serializes it: `CreateSchemaOutput` carries
    /// `#[serde(rename_all = "camelCase")]`, so the wire key is `schemaId`,
    /// and a lookup for `schema_id` silently matches nothing.
    ///
    /// The consequence is not cosmetic, and it lands on exactly the chains the
    /// matrix scores. With no id captured, `terse_write_fact` renders the
    /// weaker of its two phrasings — "a schema named 'Architecture Decision'
    /// was created" instead of "a schema with id 'architecture_decision' ...".
    /// The id is the string a later `create_node` must copy into `node_type`,
    /// so dropping it removes from history the one token the next turn needs,
    /// and leaves the model to guess the normalized spelling from the display
    /// name. Every multi-turn chain that creates a type and then records an
    /// instance against it depends on this.
    ///
    /// Asserts against the real serialization rather than a hand-written key,
    /// so a future rename of the output struct's serde policy re-breaks this
    /// test rather than silently re-breaking history.
    #[test]
    fn completed_create_schema_write_captures_the_schema_id() {
        let output = nodespace_core::schema::CreateSchemaOutput {
            schema_id: "architecture_decision".to_string(),
            is_core: false,
            version: 1,
            description: String::new(),
            fields: Vec::new(),
            relationships: Vec::new(),
            warnings: None,
        };
        let result = serde_json::to_value(&output).expect("output serializes");

        // Guard the premise: this test is only meaningful if the result really
        // does carry the id under a key other than `id`.
        assert!(
            result.get("id").is_none(),
            "premise broken: create_schema now reports a top-level `id`, so the \
             schema-id fallback this test covers is no longer the path taken"
        );

        let writes = completed_writes_from(&[ToolExecutionRecord {
            tool_call_id: "c1".to_string(),
            name: "create_schema".to_string(),
            args: serde_json::json!({"name": "Architecture Decision"}),
            result,
            is_error: false,
            duration_ms: 0,
        }]);

        assert_eq!(writes.len(), 1, "the create_schema write must be recorded");
        assert_eq!(
            writes[0].node_id.as_deref(),
            Some("architecture_decision"),
            "the schema's own id must be captured from the tool result — without \
             it the terse fact rendered into the next turn's history omits the \
             identifier a later create_node has to copy into node_type"
        );
    }

    /// Load and render a chat node's history, the way a turn does.
    ///
    /// A turn splits these steps so one read can also feed the duplicate guard;
    /// tests that only care about the rendered history compose them back.
    async fn load_node_history(node_service: &Arc<NodeService>, node_id: &str) -> Vec<ChatMessage> {
        node_history_from_messages(load_chat_messages(node_service, node_id).await)
    }

    /// Build a `LocalAgentServiceImpl` backed by a temp-dir SqliteStore, over
    /// its own process-global inference state. Returns the `TempDir` so it
    /// outlives the test body.
    async fn test_service() -> (LocalAgentServiceImpl, Arc<NodeService>, tempfile::TempDir) {
        let (svc, node_service, _shared, tempdir) =
            test_service_with(true, MODEL_SPEC_SNAPSHOT_TIMEOUT).await;
        (svc, node_service, tempdir)
    }

    /// Like `test_service`, but with the GGUF model manager forced absent —
    /// the shape `SharedLocalAgent::new` produces when `GgufModelManager::new()`
    /// fails (unwritable models directory, `$HOME` unset). Regression coverage
    /// for that failure staying a degradation of the local-GGUF RPCs rather
    /// than the panic it used to be, which — because it ran on every
    /// per-database open under ADR-053, not just once at startup — would
    /// otherwise take down the whole daemon process and every other open
    /// database with it.
    async fn test_service_without_model_manager(
    ) -> (LocalAgentServiceImpl, Arc<NodeService>, tempfile::TempDir) {
        let (svc, node_service, _shared, tempdir) =
            test_service_with(false, MODEL_SPEC_SNAPSHOT_TIMEOUT).await;
        (svc, node_service, tempdir)
    }

    /// The construction all the `test_service*` helpers share. `model_manager`
    /// selects between a real `GgufModelManager` and the degraded `None` shape;
    /// `spec_timeout` bounds the engine-swap geometry snapshot, so a test can
    /// drive the stalled-`model_info` path without paying the production bound
    /// in wall-clock time. Also hands back the `SharedLocalAgent` for tests that
    /// need to reach the process-global engine or model state directly.
    async fn test_service_with(
        model_manager: bool,
        spec_timeout: std::time::Duration,
    ) -> (
        LocalAgentServiceImpl,
        Arc<NodeService>,
        Arc<SharedLocalAgent>,
        tempfile::TempDir,
    ) {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let node_service = test_node_service(tempdir.path().join("daemon-db")).await;
        let daemon_config_path = tempdir.path().join("daemon.toml");
        let shared = if model_manager {
            SharedLocalAgent::from_model_manager(
                daemon_config_path,
                GgufModelManager::new().ok().map(Arc::new),
                spec_timeout,
            )
        } else {
            SharedLocalAgent::from_model_manager(daemon_config_path, None, spec_timeout)
        };
        let embedding: SharedEmbeddingService = Arc::new(RwLock::new(None));
        let svc = LocalAgentServiceImpl::new(shared.clone(), node_service.clone(), embedding);
        (svc, node_service, shared, tempdir)
    }

    /// A `NodeService` over a fresh SqliteStore at `path`.
    async fn test_node_service(path: std::path::PathBuf) -> Arc<NodeService> {
        let mut store = Arc::new(SqliteStore::new(path).await.expect("SqliteStore"));
        Arc::new(CoreNodeService::new(&mut store).await.expect("NodeService"))
    }

    async fn create_ai_chat_node(node_service: &Arc<NodeService>) -> String {
        let node = Node::new(
            "ai-chat".to_string(),
            "Test chat".to_string(),
            serde_json::json!({ "ai-chat": { "messages": [] } }),
        );
        node_service
            .create_node(node)
            .await
            .expect("create ai-chat")
    }

    /// Create a non-core schema node, freshly persisted — so `created_at` is
    /// "now" and it is eligible for the recency injector's 5-minute window.
    async fn create_schema_node(node_service: &Arc<NodeService>, display_name: &str) -> String {
        let node = Node::new(
            "schema".to_string(),
            display_name.to_string(),
            serde_json::json!({ "isCore": false, "fields": [] }),
        );
        node_service.create_node(node).await.expect("create schema")
    }

    /// End-to-end regression for the starvation bug fixed by
    /// `semantic_schema_count`: an unbounded lexical name-match can no longer
    /// exhaust the recency injector's budget before the injector runs.
    ///
    /// No embedding service is configured, so semantic retrieval always
    /// returns zero hits — every entry in `relevant_schemas` after this call
    /// comes from either the lexical backstop or the recency injector, which
    /// isolates their interaction from semantic search noise. Five schemas
    /// are created and named outright in the query (enough to exhaust
    /// `MAX_SCHEMAS = 5` through the lexical backstop alone); a sixth,
    /// unnamed schema is also freshly created and therefore recency-eligible.
    /// Before the fix, `remaining_slots` was computed from the post-append
    /// `relevant_schemas.len()` — already 5 by the time the injector ran — so
    /// the sixth schema was silently dropped even though semantic search
    /// itself found nothing and never "used" a slot of its own.
    #[tokio::test]
    async fn lexical_backstop_does_not_starve_the_recency_injector() {
        let (_svc, node_service, _tempdir) = test_service().await;

        let named = [
            "Invoice",
            "Venue",
            "Customer",
            "Release Plan",
            "Incident Report",
        ];
        for display_name in named {
            create_schema_node(&node_service, display_name).await;
        }
        create_schema_node(&node_service, "Feature Writeup").await;

        let query = "book the venue, log the customer, raise an invoice, add a \
                     release plan, and file an incident report";
        let rendered = build_workspace_context(&node_service, None, Some(query))
            .await
            .expect("workspace context");

        for named_type in named {
            assert!(
                rendered.contains(named_type),
                "expected the lexically-named schema '{named_type}' in the rendered \
                 context:\n{rendered}"
            );
        }
        assert!(
            rendered.contains("Feature Writeup"),
            "the recently-created, unnamed schema was starved by the lexical \
             backstop's unrelated schemas exhausting a budget it never drew \
             from:\n{rendered}"
        );
    }

    /// Create an ai-chat node already sitting in `status: "processing"` with a
    /// trailing user message — the exact shape the frontend produces via
    /// batch-update before triggering a turn (mirrors `scripts/aichat.ts`).
    async fn create_processing_node_with_user_message(
        node_service: &Arc<NodeService>,
        user_text: &str,
    ) -> String {
        let node_id = create_ai_chat_node(node_service).await;
        let node = node_service
            .get_node(&node_id)
            .await
            .expect("get node")
            .expect("node exists");
        let version = node.version;
        let mut ai_chat = AiChatNode::from_node(node).expect("from_node");
        ai_chat.turn_status = "processing".to_string();
        ai_chat.messages.push(AiChatMessage {
            role: "user".to_string(),
            content: user_text.to_string(),
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
            reasoning: None,
            completed_writes: Vec::new(),
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        });
        let mut props = serde_json::json!({});
        props["ai-chat"] = ai_chat.to_properties_value();
        node_service
            .update_node(&node_id, version, NodeUpdate::new().with_properties(props))
            .await
            .expect("set processing + user message");
        node_id
    }

    async fn get_ai_chat(node_service: &Arc<NodeService>, node_id: &str) -> AiChatNode {
        let node = node_service
            .get_node(node_id)
            .await
            .expect("get node")
            .expect("node exists");
        AiChatNode::from_node(node).expect("from_node")
    }

    // -- Stub inference engine -------------------------------------------

    /// Canned engine returning a fixed text reply with no tool calls — mirrors
    /// `agent_loop.rs`'s `MockEngine::single_text` pattern, one level up at the
    /// daemon-service seam. Counts constructions/generations so tests can
    /// assert engine-reuse ("second message reuses the loaded engine").
    struct StubEngine {
        reply: String,
        generate_count: std::sync::atomic::AtomicUsize,
    }

    impl StubEngine {
        fn new(reply: &str) -> Self {
            Self {
                reply: reply.to_string(),
                generate_count: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl ChatInferenceEngine for StubEngine {
        async fn generate(
            &self,
            _request: nodespace_agent::agent_types::InferenceRequest,
            on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            self.generate_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            on_chunk(StreamingChunk::Token {
                text: self.reply.clone(),
            });
            on_chunk(StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                },
            });
            Ok(InferenceUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
            })
        }

        async fn model_info(
            &self,
        ) -> Result<Option<nodespace_agent::agent_types::ChatModelSpec>, InferenceError> {
            Ok(None)
        }

        async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
            Ok((text.len() as f32 / 4.0).ceil() as u32)
        }
    }

    /// An engine whose `generate` blocks until the test releases it, so a
    /// turn can be cancelled mid-flight deterministically.
    struct BlockingEngine {
        release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl BlockingEngine {
        fn new(release: tokio::sync::oneshot::Receiver<()>) -> Self {
            Self {
                release: tokio::sync::Mutex::new(Some(release)),
            }
        }
    }

    #[async_trait]
    impl ChatInferenceEngine for BlockingEngine {
        async fn generate(
            &self,
            _request: nodespace_agent::agent_types::InferenceRequest,
            _on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            let rx = self.release.lock().await.take();
            if let Some(rx) = rx {
                let _ = rx.await;
            }
            Err(InferenceError::Engine("should not complete".into()))
        }

        async fn model_info(
            &self,
        ) -> Result<Option<nodespace_agent::agent_types::ChatModelSpec>, InferenceError> {
            Ok(None)
        }

        async fn token_count(&self, _text: &str) -> Result<u32, InferenceError> {
            Ok(0)
        }
    }

    /// Engine reporting a fixed geometry, so status assertions have real
    /// values to check rather than the `None` the other stubs return.
    struct SpecEngine {
        model_id: String,
        context_window: u32,
    }

    #[async_trait]
    impl ChatInferenceEngine for SpecEngine {
        async fn generate(
            &self,
            _request: nodespace_agent::agent_types::InferenceRequest,
            _on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            Err(InferenceError::Engine("not used".into()))
        }

        async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
            Ok(Some(ChatModelSpec {
                model_id: self.model_id.clone(),
                family: nodespace_agent::agent_types::ModelFamily::Gemma4,
                context_window: self.context_window,
                default_temperature: 0.7,
                type_k: None,
                type_v: None,
            }))
        }

        async fn token_count(&self, _text: &str) -> Result<u32, InferenceError> {
            Ok(0)
        }
    }

    /// Engine that models the native `LlamaChatEngine` locking discipline: a
    /// `std::sync::Mutex` held for the *whole* of a generation, which
    /// `model_info` must also take. This is the shape that makes a live
    /// `model_spec()` call from `get_status` block for the length of a turn.
    struct MutexHeldDuringGenerationEngine {
        /// Stands in for `LlamaChatEngine`'s state mutex.
        state: Arc<std::sync::Mutex<()>>,
        /// Signals that `generate` has taken the lock.
        started: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        /// Released by the test to let `generate` finish.
        release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl ChatInferenceEngine for MutexHeldDuringGenerationEngine {
        async fn generate(
            &self,
            _request: nodespace_agent::agent_types::InferenceRequest,
            _on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            let started = self.started.lock().await.take();
            let release = self.release.lock().await.take();
            let state = self.state.clone();
            // Take the lock on a blocking thread and hold it for the whole
            // "generation", as the native engine's `generate_blocking` does.
            // A `std::sync::MutexGuard` cannot be held across an await point,
            // which is exactly why the real lock lives off the async path.
            tokio::task::spawn_blocking(move || {
                let _guard = state.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(tx) = started {
                    let _ = tx.send(());
                }
                if let Some(rx) = release {
                    let _ = rx.blocking_recv();
                }
            })
            .await
            .expect("generation thread joins");
            Err(InferenceError::Engine("should not complete".into()))
        }

        async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
            let _guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            Ok(Some(ChatModelSpec {
                model_id: "held-model".to_string(),
                family: nodespace_agent::agent_types::ModelFamily::Gemma4,
                context_window: 8192,
                default_temperature: 0.7,
                type_k: None,
                type_v: None,
            }))
        }

        async fn token_count(&self, _text: &str) -> Result<u32, InferenceError> {
            Ok(0)
        }
    }

    // -- get_status model geometry -----------------------------------------

    #[tokio::test]
    async fn get_status_reports_geometry_from_swapped_engine() {
        let (svc, _node_service, _tempdir) = test_service().await;
        svc.replace_engine_if_changed(
            "gemma-4-e4b-q4km",
            Arc::new(SpecEngine {
                model_id: "/models/resolved-path.gguf".to_string(),
                context_window: 16384,
            }),
        )
        .await;

        let status = svc
            .get_status(Request::new(GetLocalStatusRequest { session_id: None }))
            .await
            .expect("get_status")
            .into_inner();

        // The catalog id the model was loaded BY, not the engine's GGUF path.
        assert_eq!(status.model_id, "gemma-4-e4b-q4km");
        assert_eq!(status.granted_n_ctx, 16384);
    }

    #[tokio::test]
    async fn get_status_reports_no_model_after_reset() {
        let (svc, _node_service, _tempdir) = test_service().await;
        svc.replace_engine_if_changed(
            "gemma-4-e4b-q4km",
            Arc::new(SpecEngine {
                model_id: "/models/resolved-path.gguf".to_string(),
                context_window: 16384,
            }),
        )
        .await;
        svc.inner.shared.reset_to_noop_engine().await;

        let status = svc
            .get_status(Request::new(GetLocalStatusRequest { session_id: None }))
            .await
            .expect("get_status")
            .into_inner();

        // The cached geometry must be cleared on reset, not left stale.
        assert_eq!(status.model_id, "");
        assert_eq!(status.granted_n_ctx, 0);
    }

    /// `unload_model` used to only flip the model manager's own catalog
    /// bookkeeping (`loaded_model_id`/status), never touching the actual
    /// engine held on `SharedLocalAgent` -- so a multi-GB engine stayed
    /// resident (and `get_status` kept reporting its geometry) even after
    /// the RPC reported success. Regression guard: after `unload_model`,
    /// both halves of the state must agree that nothing is loaded.
    #[tokio::test]
    async fn unload_model_releases_the_engine_not_just_catalog_bookkeeping() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let models_dir = tempdir.path().join("models");

        // `with_dir` snapshots each catalog entry's status once at
        // construction time by checking whether its file already exists —
        // so discover a real entry's id/filename from a throwaway instance
        // first, write the fake file, then construct the instance actually
        // used by the test so its initial scan sees it as `Ready`.
        let discovery = GgufModelManager::with_dir(models_dir.clone()).expect("model manager");
        let entry = discovery
            .list()
            .await
            .expect("list")
            .into_iter()
            .next()
            .expect("catalog has at least one entry");
        let filename = entry
            .filename
            .clone()
            .expect("catalog entry has a filename");
        std::fs::write(models_dir.join(&filename), b"fake gguf").expect("write fake model file");

        let mgr = Arc::new(GgufModelManager::with_dir(models_dir.clone()).expect("model manager"));
        mgr.load(&entry.id).await.expect("load");

        let daemon_config_path = tempdir.path().join("daemon.toml");
        let shared = SharedLocalAgent::from_model_manager(
            daemon_config_path,
            Some(mgr.clone()),
            MODEL_SPEC_SNAPSHOT_TIMEOUT,
        );
        let node_service = test_node_service(tempdir.path().join("daemon-db")).await;
        let embedding: SharedEmbeddingService = Arc::new(RwLock::new(None));
        let svc = LocalAgentServiceImpl::new(shared, node_service, embedding);

        // Simulate the real engine actually being swapped in, as happens on
        // first inference use -- this is the piece `unload_model` must
        // release, not just the model manager's own bookkeeping.
        svc.replace_engine_if_changed(
            &entry.id,
            Arc::new(SpecEngine {
                model_id: "/fake/resolved.gguf".to_string(),
                context_window: 4096,
            }),
        )
        .await;

        svc.unload_model(Request::new(UnloadModelRequest {}))
            .await
            .expect("unload_model");

        assert_eq!(
            mgr.loaded_model().await.expect("loaded_model"),
            None,
            "catalog bookkeeping must be cleared"
        );

        let status = svc
            .get_status(Request::new(GetLocalStatusRequest { session_id: None }))
            .await
            .expect("get_status")
            .into_inner();
        assert_eq!(
            status.model_id, "",
            "the actual engine must be released too, not just the catalog -- \
             a stale model_id here means the multi-GB engine is still resident"
        );
        assert_eq!(status.granted_n_ctx, 0);
    }

    /// `GgufModelManager::load()` clears the *previous* model's catalog entry
    /// internally (to mark the new one loaded), but that alone never touches
    /// the actual engine on `SharedLocalAgent` -- the engine only swaps
    /// lazily, on next inference use. So a catalog switch to B without an
    /// intervening `unload_model` could leave the catalog's own loaded-guard
    /// checking B while the resident engine was still A's, letting
    /// `delete_model(A)` slip through and delete a file the engine still
    /// referenced. `delete_model`'s RPC handler now also checks
    /// `SharedLocalAgent::active_model_id` -- the real engine identity --
    /// independent of the catalog's own bookkeeping, so this holds even if
    /// something other than `load_model`'s own (now eager-swapping) RPC path
    /// moves the catalog off A without the engine following. Regression
    /// guard: once A's engine is genuinely resident (simulating actual
    /// inference use) and the catalog has moved on to B (simulating a
    /// completed model switch), `delete_model(A)` must still be rejected.
    #[tokio::test]
    async fn delete_model_guard_survives_a_load_switch_without_intervening_unload() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let models_dir = tempdir.path().join("models");

        let discovery = GgufModelManager::with_dir(models_dir.clone()).expect("model manager");
        let mut entries = discovery.list().await.expect("list").into_iter();
        let model_a = entries.next().expect("catalog has at least two entries");
        let model_b = entries.next().expect("catalog has at least two entries");
        for entry in [&model_a, &model_b] {
            let filename = entry
                .filename
                .clone()
                .expect("catalog entry has a filename");
            std::fs::write(models_dir.join(&filename), b"fake gguf")
                .expect("write fake model file");
        }

        let mgr = Arc::new(GgufModelManager::with_dir(models_dir.clone()).expect("model manager"));
        mgr.load(&model_a.id).await.expect("load A");

        let daemon_config_path = tempdir.path().join("daemon.toml");
        let shared = SharedLocalAgent::from_model_manager(
            daemon_config_path,
            Some(mgr.clone()),
            MODEL_SPEC_SNAPSHOT_TIMEOUT,
        );
        let node_service = test_node_service(tempdir.path().join("daemon-db")).await;
        let embedding: SharedEmbeddingService = Arc::new(RwLock::new(None));
        let svc = LocalAgentServiceImpl::new(shared, node_service, embedding);

        // A has actually been used for inference: the real engine is genuinely
        // A's, matching `replace_engine_if_changed`'s lazy on-first-use swap.
        svc.replace_engine_if_changed(
            &model_a.id,
            Arc::new(SpecEngine {
                model_id: "/fake/a-resolved.gguf".to_string(),
                context_window: 4096,
            }),
        )
        .await;

        // Model switch to B without an intervening `unload_model` call: only
        // the catalog moves on, exactly as `GgufModelManager::load()` does
        // internally. The resident engine is untouched -- still A's.
        mgr.load(&model_b.id).await.expect("load B");
        assert_eq!(
            mgr.loaded_model().await.expect("loaded_model"),
            Some(model_b.id.clone()),
            "catalog must report B as loaded after the switch"
        );

        let delete_result = svc
            .delete_model(Request::new(DeleteModelRequest {
                model_id: model_a.id.clone(),
            }))
            .await;

        assert!(
            delete_result.is_err(),
            "delete_model(A) must be rejected while A's engine is still resident, \
             even though the catalog's loaded_model_id now says B"
        );

        let filename = model_a.filename.expect("catalog entry has a filename");
        assert!(
            models_dir.join(&filename).exists(),
            "A's GGUF file must not be deleted while its engine is still resident"
        );
    }

    /// `load_model`'s RPC handler used to call only
    /// `GgufModelManager::load()` -- catalog bookkeeping alone, with no real
    /// engine ever built or swapped in. It now drives the same
    /// load-and-swap path `EnsureModelReady` streams, so a failure building
    /// the real engine (an unreadable/invalid GGUF file, here) must fail the
    /// RPC and must not mark the catalog loaded or touch the resident
    /// engine -- the eager swap only takes effect once the real engine
    /// actually exists, closing the window the catalog-only path left open.
    #[tokio::test]
    async fn load_model_rpc_does_not_mark_loaded_when_the_real_engine_fails_to_build() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let models_dir = tempdir.path().join("models");

        let discovery = GgufModelManager::with_dir(models_dir.clone()).expect("model manager");
        let entry = discovery
            .list()
            .await
            .expect("list")
            .into_iter()
            .next()
            .expect("catalog has at least one entry");
        let filename = entry
            .filename
            .clone()
            .expect("catalog entry has a filename");
        // Not a real GGUF file -- LlamaChatInferenceEngine::load must reject it.
        std::fs::write(models_dir.join(&filename), b"not a real gguf file")
            .expect("write fake model file");

        let mgr = Arc::new(GgufModelManager::with_dir(models_dir.clone()).expect("model manager"));
        let daemon_config_path = tempdir.path().join("daemon.toml");
        let shared = SharedLocalAgent::from_model_manager(
            daemon_config_path,
            Some(mgr.clone()),
            MODEL_SPEC_SNAPSHOT_TIMEOUT,
        );
        let node_service = test_node_service(tempdir.path().join("daemon-db")).await;
        let embedding: SharedEmbeddingService = Arc::new(RwLock::new(None));
        let svc = LocalAgentServiceImpl::new(shared, node_service, embedding);

        let result = svc
            .load_model(Request::new(LoadModelRequest {
                model_id: entry.id.clone(),
            }))
            .await;

        assert!(
            result.is_err(),
            "load_model must fail when the real inference engine cannot be built, \
             not report success on catalog bookkeeping alone"
        );
        assert_eq!(
            mgr.loaded_model().await.expect("loaded_model"),
            None,
            "catalog must not be marked loaded when the real engine failed to build"
        );

        let status = svc
            .get_status(Request::new(GetLocalStatusRequest { session_id: None }))
            .await
            .expect("get_status")
            .into_inner();
        assert_eq!(
            status.model_id, "",
            "no engine should be resident after a failed load"
        );
    }

    /// Engine whose `model_info` never returns — the shape of a remote
    /// endpoint that accepts the connection and then stalls.
    struct HangingModelInfoEngine;

    #[async_trait]
    impl ChatInferenceEngine for HangingModelInfoEngine {
        async fn generate(
            &self,
            _request: nodespace_agent::agent_types::InferenceRequest,
            _on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            Err(InferenceError::Engine("not used".into()))
        }

        async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
            std::future::pending().await
        }

        async fn token_count(&self, _text: &str) -> Result<u32, InferenceError> {
            Ok(0)
        }
    }

    /// A remote engine that stalls on `model_info` must not hold up the swap.
    /// Losing the geometry degrades the status report; blocking here would
    /// block the model-load RPC that awaits `replace_engine`.
    #[tokio::test]
    async fn engine_swap_completes_when_model_info_hangs() {
        // Drive the timeout path without paying the production bound in
        // wall-clock time on every run.
        let short = std::time::Duration::from_millis(50);
        let (svc, _node_service, _shared, _tempdir) = test_service_with(true, short).await;

        // Both calls run on a runtime of their own, on a dedicated OS thread,
        // and the test waits on a channel. Neither may share a runtime with
        // this test: if the swap timeout regresses, `replace_engine` never
        // returns; and if `get_status` regresses to a live engine query, it
        // blocks its thread outright. Either one, awaited on a shared runtime,
        // hangs the whole suite rather than failing this test — which is what
        // it did before, but only under concurrent load, making it a latent
        // flake. Waiting on a channel cannot be starved by tokio scheduling.
        let swap_svc = svc.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("swap runtime");
            let result = rt.block_on(async move {
                swap_svc
                    .replace_engine_if_changed("hanging-model", Arc::new(HangingModelInfoEngine))
                    .await;
                swap_svc
                    .get_status(Request::new(GetLocalStatusRequest { session_id: None }))
                    .await
            });
            // A send failure means the test already timed out and moved on.
            let _ = done_tx.send(result);
        });

        let status = done_rx
            .recv_timeout(short + std::time::Duration::from_secs(5))
            .expect("engine swap must not block on a stalled model_info")
            .expect("get_status")
            .into_inner();

        // Swap completed rather than hanging the caller, and the model still
        // reports its identity — only the window is unknown. Reporting an
        // empty id here would make a loaded model look absent.
        assert_eq!(status.model_id, "hanging-model");
        assert_eq!(status.granted_n_ctx, 0);
    }

    /// The core regression guard: `get_status` must not touch the engine
    /// mutex, which a generation holds for its entire duration.
    ///
    /// Querying `model_spec()` live here would block until the turn finished.
    /// The generation is deliberately never released before the status call, so
    /// against the pre-fix code the status RPC blocks outright rather than
    /// merely returning wrong values, and the timeout turns that into a
    /// failure. Needs the multi-thread runtime: the blocking lock wedges a
    /// worker thread, and on the default single-threaded runtime there would be
    /// no thread left to fire the timeout — the test would hang instead of
    /// failing cleanly.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn get_status_does_not_block_while_a_generation_holds_the_engine_lock() {
        let (svc, node_service, _tempdir) = test_service().await;

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        svc.replace_engine_if_changed(
            "held-model",
            Arc::new(MutexHeldDuringGenerationEngine {
                state: Arc::new(std::sync::Mutex::new(())),
                started: tokio::sync::Mutex::new(Some(started_tx)),
                release: tokio::sync::Mutex::new(Some(release_rx)),
            }),
        )
        .await;

        // Kick off a turn and wait until it actually holds the engine lock.
        let node_id = create_processing_node_with_user_message(&node_service, "Hi").await;
        let turn_svc = svc.clone();
        let turn = tokio::spawn(async move {
            turn_svc.maybe_handle_ai_chat_node(&node_id).await;
        });
        started_rx.await.expect("generation should take the lock");

        // Run the status call on a runtime of its own, on a dedicated OS
        // thread, and wait on a channel rather than on the task.
        //
        // A pre-fix `get_status` blocks the thread it runs on outright. Any
        // construction that shares a runtime with it — a spawned task, a
        // timeout wrapped around the call — can have every worker consumed
        // before the timer is scheduled, which hangs the whole suite instead
        // of failing this test. Whether that happens depends on what else the
        // harness is running concurrently, so it is a latent flake in any
        // shared-runtime form. An isolated runtime cannot be starved by the
        // blocked call, so the failure stays clean and deterministic.
        let status_svc = svc.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("status runtime");
            let result = rt.block_on(
                status_svc.get_status(Request::new(GetLocalStatusRequest { session_id: None })),
            );
            // A send failure means the test already timed out and moved on.
            let _ = done_tx.send(result);
        });

        let status = done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("get_status must return while a generation holds the engine lock")
            .expect("get_status")
            .into_inner();

        // Served from the cache, so the real geometry is still reported.
        assert_eq!(status.model_id, "held-model");
        assert_eq!(status.granted_n_ctx, 8192);

        let _ = release_tx.send(());
        let _ = turn.await;
    }

    // -- Agent-flow (stuck-state) coverage with a stub model ----------------

    #[tokio::test]
    async fn send_message_reaches_idle_with_stub_engine() {
        let (svc, node_service, _tempdir) = test_service().await;
        svc.replace_engine(Arc::new(StubEngine::new("Hello back!")))
            .await;

        let node_id = create_processing_node_with_user_message(&node_service, "Hi there").await;

        svc.maybe_handle_ai_chat_node(&node_id).await;

        let ai_chat = get_ai_chat(&node_service, &node_id).await;
        assert_eq!(
            ai_chat.turn_status, "idle",
            "turn must terminate, never stuck processing"
        );
        let assistant = ai_chat
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant reply appended");
        assert_eq!(assistant.content, "Hello back!");
        assert!(
            svc.inner.turn_tokens.lock().await.get(&node_id).is_none(),
            "no lingering turn token after completion"
        );
    }

    // -- ADR-053: per-database routing over a daemon-global engine ----------

    /// A two-database manager whose service sets share one process-global
    /// `SharedLocalAgent`, assembled exactly the way the daemon assembles them.
    /// The model manager is forced absent so the test never touches the real
    /// models directory; nothing here reaches a GGUF RPC.
    async fn routed_manager() -> (
        Arc<crate::DatabaseManager>,
        crate::services::DatabaseId,
        crate::services::DatabaseId,
        tempfile::TempDir,
    ) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let (_tx, model) = tokio::sync::watch::channel::<
            Option<Arc<nodespace_nlp_engine::EmbeddingService>>,
        >(None);
        let context = crate::SharedContext {
            pty_manager: Arc::new(nodespace_agent::pty::PtySessionManager::new()),
            model,
            has_model: false,
            model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scheduler: Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
            subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
            local_agent: SharedLocalAgent::from_model_manager(
                dir.path().join("daemon.toml"),
                None,
                MODEL_SPEC_SNAPSHOT_TIMEOUT,
            ),
        };
        let manager = Arc::new(
            crate::DatabaseManager::load(dir.path().join("databases.toml"), context)
                .await
                .expect("manager"),
        );
        let default_id = manager
            .ensure_default_registered("Default".into(), dir.path().join("default.db"))
            .await
            .expect("default database");
        let second = manager
            .create("Second".into(), Some(dir.path().join("second.db")))
            .await
            .expect("second database");
        (manager, default_id, second.id, dir)
    }

    /// Poll until the node leaves `processing`. A turn is driven by whichever
    /// of the explicit trigger and the database's own event watcher claims it
    /// first — they dedup on the same claim — so tests wait for the node to
    /// settle rather than assuming which one got there.
    async fn await_settled_ai_chat(node_service: &Arc<NodeService>, node_id: &str) -> AiChatNode {
        for _ in 0..200 {
            let chat = get_ai_chat(node_service, node_id).await;
            if chat.turn_status != "processing" {
                return chat;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        panic!("ai-chat node never left `processing`");
    }

    /// Turn-state RPCs act on the database the request names, not on whichever
    /// one the daemon booted with.
    ///
    /// The router registers a single `LocalAgentServiceServer`, built from the
    /// boot-time default database's service set. Turn state, though, is
    /// per-database: the cancellation tokens and the busy/idle answer belong to
    /// the database whose ai-chat node is being generated into. Unrouted, every
    /// such call lands on the default's instance — so on any other database
    /// `CancelTurn` silently no-ops and `GetStatus` reports the wrong
    /// database's activity.
    #[tokio::test]
    async fn turn_state_rpcs_route_to_the_targeted_database() {
        let (manager, default_id, second_id, _dir) = routed_manager().await;

        // The impl the router holds is the default database's, exactly as the
        // serve loops wire it into `BaseServices`.
        let registered = manager
            .get_or_open(&default_id)
            .await
            .unwrap()
            .local_agent
            .clone();
        let second = manager
            .get_or_open(&second_id)
            .await
            .unwrap()
            .local_agent
            .clone();

        let turn = second
            .begin_turn("chat-node")
            .await
            .expect("second database accepts a turn");

        let idle_json = serde_json::to_string(&LocalAgentStatus::Idle).unwrap();
        let streaming_json = serde_json::to_string(&LocalAgentStatus::Streaming).unwrap();

        let mut header_less = Request::new(GetLocalStatusRequest { session_id: None });
        header_less.extensions_mut().insert(manager.clone());
        let status = registered
            .get_status(header_less)
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            status.status_json, idle_json,
            "the default database has no turn running, so a header-less status must read idle"
        );

        let mut targeted = Request::new(GetLocalStatusRequest { session_id: None });
        targeted.extensions_mut().insert(manager.clone());
        targeted.metadata_mut().insert(
            crate::db_routing::DATABASE_ID_HEADER,
            second_id.as_str().parse().unwrap(),
        );
        let status = registered.get_status(targeted).await.unwrap().into_inner();
        assert_eq!(
            status.status_json, streaming_json,
            "status must report the named database's activity; reading the default's instead \
             shows Idle while another database is mid-turn"
        );

        let mut cancel = Request::new(CancelTurnRequest {
            node_id: "chat-node".to_string(),
        });
        cancel.extensions_mut().insert(manager.clone());
        cancel.metadata_mut().insert(
            crate::db_routing::DATABASE_ID_HEADER,
            second_id.as_str().parse().unwrap(),
        );
        registered.cancel_turn(cancel).await.unwrap();

        assert!(
            turn.is_cancelled(),
            "cancel must reach the database actually running the turn — looked up in the \
             default's token map it matches nothing and silently no-ops"
        );
    }

    /// A live token stream follows the database the subscriber named.
    ///
    /// Each database broadcasts its turn's tokens on its own channel, so a
    /// subscription bound to the default database sees nothing at all while
    /// another database is generating.
    #[tokio::test]
    async fn token_stream_subscribes_to_the_targeted_database() {
        use tokio_stream::StreamExt;

        let (manager, default_id, second_id, _dir) = routed_manager().await;
        let registered = manager
            .get_or_open(&default_id)
            .await
            .unwrap()
            .local_agent
            .clone();
        let second = manager
            .get_or_open(&second_id)
            .await
            .unwrap()
            .local_agent
            .clone();

        let mut subscribe = Request::new(SubscribeTokenStreamRequest {});
        subscribe.extensions_mut().insert(manager.clone());
        subscribe.metadata_mut().insert(
            crate::db_routing::DATABASE_ID_HEADER,
            second_id.as_str().parse().unwrap(),
        );
        let mut stream = registered
            .subscribe_token_stream(subscribe)
            .await
            .unwrap()
            .into_inner();

        // What a turn running on the second database emits.
        second
            .inner
            .token_tx
            .send(AgentChunk {
                chunk_type: "token".to_string(),
                token_text: Some("hi".to_string()),
                node_id: Some("chat-node".to_string()),
                ..Default::default()
            })
            .expect("the routed subscriber is listening on this channel");

        let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
            .await
            .expect("a routed subscription must receive the targeted database's tokens")
            .expect("stream is open")
            .expect("chunk is not an error");
        assert_eq!(chunk.node_id.as_deref(), Some("chat-node"));
        assert_eq!(chunk.token_text.as_deref(), Some("hi"));
    }

    /// A model loaded once is the daemon's model, not one database's.
    ///
    /// The engine is a single machine resource behind `SharedLocalAgent`, so a
    /// turn on a database the user never happened to load the model "on" runs
    /// against it normally. Per-database engines instead left every database
    /// but the boot-time default holding a no-op engine, failing each turn with
    /// `NoModelLoaded` and appending a visible error message.
    #[tokio::test]
    async fn a_model_loaded_once_serves_every_database() {
        let (manager, default_id, second_id, _dir) = routed_manager().await;
        let first = manager.get_or_open(&default_id).await.unwrap();
        let second = manager.get_or_open(&second_id).await.unwrap();

        // Load through whichever database the app happened to be showing.
        first
            .local_agent
            .replace_engine_if_changed("stub-model", Arc::new(StubEngine::new("Hello back!")))
            .await;

        // Then chat on the other one.
        let node_service = second.node_service_grpc.node_service();
        let node_id = create_processing_node_with_user_message(&node_service, "Hi there").await;
        second.local_agent.maybe_handle_ai_chat_node(&node_id).await;

        let ai_chat = await_settled_ai_chat(&node_service, &node_id).await;
        assert_eq!(
            ai_chat.turn_status, "idle",
            "turn must terminate, never stuck"
        );
        let assistant = ai_chat
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant reply appended");
        assert_eq!(
            assistant.content, "Hello back!",
            "the turn must run against the daemon's loaded engine; a per-database engine leaves \
             this database on the no-op one and records an inference failure instead"
        );
    }

    /// Engine whose `generate` always fails with a given `InferenceError` —
    /// stands in for the native engine rejecting a turn (context window
    /// exceeded, or any other inference failure) so `run_ai_chat_turn`'s
    /// failure path can be exercised without a real model.
    struct FailingEngine {
        make_error: fn() -> InferenceError,
    }

    #[async_trait]
    impl ChatInferenceEngine for FailingEngine {
        async fn generate(
            &self,
            _request: nodespace_agent::agent_types::InferenceRequest,
            _on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            Err((self.make_error)())
        }

        async fn model_info(
            &self,
        ) -> Result<Option<nodespace_agent::agent_types::ChatModelSpec>, InferenceError> {
            Ok(None)
        }

        async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
            Ok((text.len() as f32 / 4.0).ceil() as u32)
        }
    }

    /// Regression guard for the silent-failure bug: a turn whose inference
    /// call fails (e.g. `InferenceError::ContextOverflow`, the exact shape
    /// the daemon's native engine returns when a system prompt no longer fits
    /// the loaded context window) must reach `idle` WITH a new assistant
    /// message describing the failure — not `idle` with `assistant_count`
    /// unchanged, which is indistinguishable (to any caller polling this
    /// node, and to the frontend, which only renders `user`/`assistant`
    /// messages) from a turn that is still silently stuck. Before the fix,
    /// this path only logged a WARN and reset status to idle.
    #[tokio::test]
    async fn failed_inference_turn_surfaces_a_visible_error_not_silent_idle() {
        let (svc, node_service, _tempdir) = test_service().await;
        svc.replace_engine(Arc::new(FailingEngine {
            make_error: || {
                InferenceError::ContextOverflow(
                    "Prompt uses 6032 tokens but context window is 5120".to_string(),
                )
            },
        }))
        .await;

        let node_id = create_processing_node_with_user_message(&node_service, "Hi there").await;

        svc.maybe_handle_ai_chat_node(&node_id).await;

        let ai_chat = get_ai_chat(&node_service, &node_id).await;
        assert_eq!(
            ai_chat.turn_status, "idle",
            "a failed turn must still terminate, never stuck processing"
        );
        let assistant = ai_chat
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect(
                "a failed inference turn must append a visible assistant message, \
                 not silently reset to idle with no new content",
            );
        assert!(
            assistant.content.contains("context window"),
            "the surfaced message should name the actual failure, got: {:?}",
            assistant.content
        );
        assert!(
            svc.inner.turn_tokens.lock().await.get(&node_id).is_none(),
            "no lingering turn token after a failed turn"
        );
    }

    /// A write against a node that no longer exists must return `Err` on the
    /// first attempt, not retry.
    ///
    /// This became meaningful with the retry narrowing: `update_node`
    /// disambiguates a failed version-gated write into `NodeNotFound` (the row
    /// is gone) versus `VersionConflict` (the version moved), so a concurrent
    /// delete now fails fast instead of burning the whole budget. Both helpers
    /// also bail at their opening `get_node` in this case, which is the path
    /// this test drives.
    ///
    /// Note what this does NOT cover: the retry-exhaustion arm. Reaching that
    /// needs a writer interleaved into the read/write gap of these `&self`
    /// helpers, which is not reachable without adding a seam to production
    /// code. The exhaustion arm returning `Err` rather than the `unreachable!()`
    /// it replaced is therefore still unpinned by any test.
    #[tokio::test]
    async fn ai_chat_writes_on_a_missing_node_return_err() {
        let (svc, node_service, _tempdir) = test_service().await;
        let node_id = create_processing_node_with_user_message(&node_service, "Hello").await;

        let node = node_service
            .get_node(&node_id)
            .await
            .expect("get node")
            .expect("node exists");
        node_service
            .delete_node(&node_id, node.version)
            .await
            .expect("delete node");

        assert!(
            svc.write_ai_chat_turn_status(&node_id, "idle", None)
                .await
                .is_err(),
            "a status write to a missing node must return Err"
        );
        assert!(
            svc.append_assistant_message(&node_id, "Reply.", None, Vec::new(), Vec::new(), None)
                .await
                .is_err(),
            "an append to a missing node must return Err"
        );
    }

    #[tokio::test]
    async fn stuck_processing_node_recovered_on_watcher_restart() {
        let (svc, node_service, _tempdir) = test_service().await;
        svc.replace_engine(Arc::new(StubEngine::new("Recovered reply.")))
            .await;

        // Simulate a daemon crash mid-turn: node left in `processing` with a
        // trailing user message, no in-memory turn_tokens entry (fresh process).
        let node_id =
            create_processing_node_with_user_message(&node_service, "Are you there?").await;

        svc.recover_stuck_turns().await;

        // recover_stuck_turns spawns the retry; poll briefly for completion.
        let mut ai_chat = get_ai_chat(&node_service, &node_id).await;
        for _ in 0..50 {
            if ai_chat.turn_status == "idle" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            ai_chat = get_ai_chat(&node_service, &node_id).await;
        }

        assert_eq!(
            ai_chat.turn_status, "idle",
            "a node stuck in processing at startup must recover to idle, not stay stuck"
        );
        assert!(ai_chat.messages.iter().any(|m| m.role == "assistant"));
    }

    #[tokio::test]
    async fn turn_cancelled_via_cancel_turn_resets_to_idle() {
        let (svc, node_service, _tempdir) = test_service().await;
        let (_release_tx, release_rx) = tokio::sync::oneshot::channel();
        svc.replace_engine(Arc::new(BlockingEngine::new(release_rx)))
            .await;

        let node_id =
            create_processing_node_with_user_message(&node_service, "Take your time").await;

        // Trigger the turn in the background (it will block inside BlockingEngine).
        let svc2 = svc.clone();
        let node_id2 = node_id.clone();
        let handle = tokio::spawn(async move {
            svc2.maybe_handle_ai_chat_node(&node_id2).await;
        });

        // Wait until the turn has actually registered its cancellation token.
        for _ in 0..50 {
            if svc.inner.turn_tokens.lock().await.contains_key(&node_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            svc.inner.turn_tokens.lock().await.contains_key(&node_id),
            "turn should be registered before cancelling"
        );

        // Cancel via the same path the gRPC CancelTurn handler uses.
        {
            let tokens = svc.inner.turn_tokens.lock().await;
            if let Some(token) = tokens.get(&node_id) {
                token.cancel();
            }
        }
        handle.await.expect("turn task joins");

        let ai_chat = get_ai_chat(&node_service, &node_id).await;
        assert_eq!(
            ai_chat.turn_status, "idle",
            "cancelled turn must reset to idle, not stay stuck"
        );
        assert!(
            !ai_chat.messages.iter().any(|m| m.role == "assistant"),
            "a cancelled turn must not append an assistant message"
        );
        assert!(
            svc.inner.turn_tokens.lock().await.get(&node_id).is_none(),
            "no stray cancellation-token entry left behind after cancel"
        );
    }

    #[tokio::test]
    async fn second_message_reuses_loaded_engine() {
        let (svc, node_service, _tempdir) = test_service().await;
        let swapped_first = svc
            .replace_engine_if_changed("stub-model", Arc::new(StubEngine::new("first reply")))
            .await;
        assert!(swapped_first, "first load must swap the engine in");

        let node_id_1 =
            create_processing_node_with_user_message(&node_service, "First message").await;
        svc.maybe_handle_ai_chat_node(&node_id_1).await;
        assert_eq!(
            get_ai_chat(&node_service, &node_id_1).await.turn_status,
            "idle"
        );

        // A second "load" of the same model_id must be a no-op swap — this is
        // the literal "second message reuses the loaded engine" criterion.
        let swapped_second = svc
            .replace_engine_if_changed("stub-model", Arc::new(StubEngine::new("second reply")))
            .await;
        assert!(
            !swapped_second,
            "loading the same model_id again must not swap the engine"
        );

        let node_id_2 =
            create_processing_node_with_user_message(&node_service, "Second message").await;
        svc.maybe_handle_ai_chat_node(&node_id_2).await;

        let ai_chat_2 = get_ai_chat(&node_service, &node_id_2).await;
        assert_eq!(ai_chat_2.turn_status, "idle");
        // Because the swap was skipped, the ORIGINAL engine (first reply) is
        // still the one wired in and answers the second turn too.
        let assistant = ai_chat_2
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant reply appended");
        assert_eq!(assistant.content, "first reply");
    }

    #[tokio::test]
    async fn reasoning_round_trips_through_persist_and_reload() {
        let (svc, node_service, _tempdir) = test_service().await;
        let node_id = create_ai_chat_node(&node_service).await;

        svc.append_assistant_message(
            &node_id,
            "The answer.",
            Some("I reasoned about it."),
            Vec::new(),
            Vec::new(),
            None,
        )
        .await
        .expect("append");

        let history = load_node_history(&node_service, &node_id).await;
        let assistant = history
            .iter()
            .find(|m| matches!(m.role, Role::Assistant))
            .expect("assistant message present");
        assert_eq!(assistant.content, "The answer.");
        assert_eq!(assistant.reasoning.as_deref(), Some("I reasoned about it."));
    }

    /// #1930: a `route_clarify` turn's structured question/options must
    /// persist onto the node alongside the flattened `content` text, not only
    /// as markdown prose — that structure is what the frontend renders as
    /// clickable options instead of parsed-out bullets.
    #[tokio::test]
    async fn clarify_question_and_options_persist_onto_the_node() {
        let (svc, node_service, _tempdir) = test_service().await;
        let node_id = create_ai_chat_node(&node_service).await;

        let clarify = ClarifyPrompt {
            question: "Did you want to track debts or search notes?".to_string(),
            options: vec![
                "Track who owes me money".to_string(),
                "Search existing notes".to_string(),
            ],
        };
        svc.append_assistant_message(
            &node_id,
            "I can take that a couple of ways. Did you want to track debts or search notes?\n\n\
             - Track who owes me money\n- Search existing notes",
            None,
            Vec::new(),
            Vec::new(),
            Some(&clarify),
        )
        .await
        .expect("append");

        let messages = load_chat_messages(&node_service, &node_id).await;
        let assistant = messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant message present");
        assert_eq!(
            assistant.question.as_deref(),
            Some("Did you want to track debts or search notes?")
        );
        assert_eq!(
            assistant.options,
            vec![
                "Track who owes me money".to_string(),
                "Search existing notes".to_string()
            ]
        );
        // The flattened text is still there too — plain-text readers and the
        // LLM-facing history scan are unaffected by adding the structured field.
        assert!(assistant.content.contains("Track who owes me money"));
    }

    /// An ordinary reply (no clarify) must not gain `question`/`options` —
    /// only a genuine `route_clarify` turn should ever render option chips.
    #[tokio::test]
    async fn ordinary_reply_persists_no_clarify_fields() {
        let (svc, node_service, _tempdir) = test_service().await;
        let node_id = create_ai_chat_node(&node_service).await;

        svc.append_assistant_message(
            &node_id,
            "Here's your answer.",
            None,
            Vec::new(),
            Vec::new(),
            None,
        )
        .await
        .expect("append");

        let messages = load_chat_messages(&node_service, &node_id).await;
        let assistant = messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant message present");
        assert!(assistant.question.is_none());
        assert!(assistant.options.is_empty());
    }

    /// Build a successful tool execution record.
    fn exec(name: &str, args: serde_json::Value, result: serde_json::Value) -> ToolExecutionRecord {
        ToolExecutionRecord {
            tool_call_id: format!("tc_{name}"),
            name: name.to_string(),
            args,
            result,
            is_error: false,
            duration_ms: 1,
        }
    }

    /// The cross-turn case. The per-turn `seen_calls` guard cannot cover this:
    /// the agent session is destroyed at the end of every turn, so turn N+1 has
    /// no in-memory record of turn N at all.
    ///
    /// This is the regression that matters — before the fix, the rebuilt history
    /// contained the user's instruction and the assistant's prose claim but no
    /// trace of the write, so the model could satisfy the same instruction twice.
    #[tokio::test]
    async fn completed_write_is_visible_in_the_next_turns_history() {
        let (svc, node_service, _tempdir) = test_service().await;
        let node_id = create_ai_chat_node(&node_service).await;

        let writes = completed_writes_from(&[exec(
            "create_node",
            serde_json::json!({"content": "Kind of Blue by Miles Davis", "node_type": "album_to_listen"}),
            serde_json::json!({"id": "nodespace://f1f25564"}),
        )]);

        svc.append_assistant_message(
            &node_id,
            "I have added \"Kind of Blue\".",
            None,
            writes,
            Vec::new(),
            None,
        )
        .await
        .expect("append");

        // What the NEXT turn actually sees.
        let history = load_node_history(&node_service, &node_id).await;

        let evidence = history
            .iter()
            .find(|m| matches!(m.role, Role::System))
            .expect(
                "next turn must see durable evidence of the completed write; without it the \
                 model re-executes the instruction still present in history",
            );
        assert!(evidence.content.contains("create_node"));
        assert!(evidence.content.contains("Kind of Blue by Miles Davis"));
        assert!(evidence.content.contains("nodespace://f1f25564"));
        // The evidence must follow the assistant turn it describes.
        let assistant_idx = history
            .iter()
            .position(|m| matches!(m.role, Role::Assistant))
            .expect("assistant message present");
        let evidence_idx = history
            .iter()
            .position(|m| matches!(m.role, Role::System))
            .expect("evidence present");
        assert!(evidence_idx > assistant_idx);

        // The evidence must never be a tool-role message: those require a
        // preceding assistant tool-call turn to pair with, and none is
        // persisted. An orphan tool result can abort the turn outright.
        assert!(
            !history.iter().any(|m| matches!(m.role, Role::Tool)),
            "evidence must not be injected as an orphan tool result"
        );
    }

    /// A write that failed did not happen. Recording it would tell the next turn
    /// not to retry work that never landed.
    #[tokio::test]
    async fn failed_writes_are_not_recorded_as_completed() {
        let failed = ToolExecutionRecord {
            tool_call_id: "tc_1".into(),
            name: "create_node".into(),
            args: serde_json::json!({"content": "never landed"}),
            result: serde_json::json!({"error": "validation failed"}),
            is_error: true,
            duration_ms: 1,
        };
        assert!(completed_writes_from(&[failed]).is_empty());
    }

    /// Reads are not writes: repeating a search is wasteful, not a duplicate,
    /// and recording them would bloat every subsequent prompt.
    #[tokio::test]
    async fn reads_are_not_recorded_as_completed_writes() {
        let writes = completed_writes_from(&[
            exec(
                "search_nodes",
                serde_json::json!({"query": "x"}),
                serde_json::json!({"count": 3}),
            ),
            exec(
                "get_node",
                serde_json::json!({"id": "n1"}),
                serde_json::json!({"content": "y"}),
            ),
        ]);
        assert!(writes.is_empty());
    }

    /// `resolved_entities_from` is the read-side counterpart to
    /// `completed_writes_from`: it must pull node identity out of every
    /// entity-resolving tool's result shape (`search_nodes`'s `"nodes"` list,
    /// `get_node`'s single top-level node), and must NOT record a tool that
    /// merely writes (`create_node`) even though it also reports an id.
    #[test]
    fn resolved_entities_from_reads_node_and_search_result_shapes() {
        let entities = resolved_entities_from(&[
            exec(
                "search_nodes",
                serde_json::json!({"query": "tasks"}),
                serde_json::json!({
                    "count": 1,
                    "nodes": [
                        {"id": "nodespace://t1", "title": "Finish the report", "type": "task"}
                    ]
                }),
            ),
            exec(
                "get_node",
                serde_json::json!({"id": "nodespace://t1"}),
                serde_json::json!({"id": "nodespace://t1", "title": "Finish the report", "nodeType": "task"}),
            ),
            exec(
                "create_node",
                serde_json::json!({"content": "New task"}),
                serde_json::json!({"id": "nodespace://t2"}),
            ),
        ]);

        // The write is excluded even though its result also carries an "id".
        assert!(
            !entities.iter().any(|e| e.node_id == "nodespace://t2"),
            "a write tool's result must not be recorded as a resolved entity: {entities:?}"
        );

        let t1 = entities
            .iter()
            .find(|e| e.node_id == "nodespace://t1")
            .expect("search_nodes/get_node result must resolve nodespace://t1");
        assert_eq!(t1.title.as_deref(), Some("Finish the report"));
        assert_eq!(t1.node_type.as_deref(), Some("task"));
    }

    /// A failed read surfaced nothing; recording it would tell the next turn
    /// about a node lookup that never actually returned data.
    #[test]
    fn failed_reads_resolve_no_entities() {
        let failed = ToolExecutionRecord {
            tool_call_id: "tc_1".into(),
            name: "get_node".into(),
            args: serde_json::json!({"id": "n1"}),
            result: serde_json::json!({"error": "not found"}),
            is_error: true,
            duration_ms: 1,
        };
        assert!(resolved_entities_from(&[failed]).is_empty());
    }

    /// Duplicate node ids across multiple read calls in one turn collapse to a
    /// single entry, keeping the most recent occurrence.
    #[test]
    fn resolved_entities_are_deduped_by_node_id() {
        let entities = resolved_entities_from(&[
            exec(
                "get_node",
                serde_json::json!({"id": "nodespace://t1"}),
                serde_json::json!({"id": "nodespace://t1", "title": "Old title", "nodeType": "task"}),
            ),
            exec(
                "get_node",
                serde_json::json!({"id": "nodespace://t1"}),
                serde_json::json!({"id": "nodespace://t1", "title": "Renamed title", "nodeType": "task"}),
            ),
        ]);
        assert_eq!(
            entities.len(),
            1,
            "duplicate node ids must collapse: {entities:?}"
        );
        assert_eq!(entities[0].title.as_deref(), Some("Renamed title"));
    }

    /// A broad search can surface far more nodes than are worth carrying
    /// forward; the cap keeps the per-turn context cost bounded regardless of
    /// how many results a single `search_nodes` call returns.
    #[test]
    fn resolved_entities_are_capped_at_the_maximum() {
        let nodes: Vec<serde_json::Value> = (0..MAX_RESOLVED_ENTITIES + 10)
            .map(|i| {
                serde_json::json!({"id": format!("nodespace://n{i}"), "title": format!("Node {i}"), "type": "task"})
            })
            .collect();
        let entities = resolved_entities_from(&[exec(
            "search_nodes",
            serde_json::json!({"query": "everything"}),
            serde_json::json!({"count": nodes.len(), "nodes": nodes}),
        )]);
        assert_eq!(entities.len(), MAX_RESOLVED_ENTITIES);
    }

    /// The regression this issue exists to fix: a read-only turn ("how many
    /// tasks do we have") resolves a node, and the NEXT turn's rebuilt history
    /// must still carry that node's id — not just the prose reply — so a
    /// follow-up ("update that status") has something concrete to resolve
    /// "that" against.
    #[tokio::test]
    async fn resolved_entity_from_a_read_only_turn_is_visible_in_the_next_turns_history() {
        let (svc, node_service, _tempdir) = test_service().await;
        let node_id = create_ai_chat_node(&node_service).await;

        let entities = resolved_entities_from(&[exec(
            "search_nodes",
            serde_json::json!({"query": "tasks", "node_type": "task"}),
            serde_json::json!({
                "count": 1,
                "nodes": [
                    {"id": "nodespace://one-task", "title": "Finish the report", "type": "task"}
                ]
            }),
        )]);
        assert_eq!(
            entities.len(),
            1,
            "the read must resolve exactly one entity"
        );

        svc.append_assistant_message(
            &node_id,
            "You have one task node: \"Finish the report\".",
            None,
            Vec::new(),
            entities,
            None,
        )
        .await
        .expect("append");

        // What the NEXT turn actually sees.
        let history = load_node_history(&node_service, &node_id).await;

        let evidence = history
            .iter()
            .find(|m| matches!(m.role, Role::System))
            .expect(
                "next turn must see durable evidence of the resolved entity; without it the \
                 model cannot resolve a follow-up pronoun (\"that\", \"it\") back to a node id",
            );
        assert!(evidence.content.contains("nodespace://one-task"));
        assert!(evidence.content.contains("Finish the report"));

        // Never an orphan tool-role message, for the same reason as writes.
        assert!(
            !history.iter().any(|m| matches!(m.role, Role::Tool)),
            "evidence must not be injected as an orphan tool result"
        );
    }

    /// A turn that read but resolved nothing (no writes, no entities) must not
    /// emit an evidence message at all — an empty "writes completed"/"entities
    /// resolved" block would be noise in every prompt.
    #[tokio::test]
    async fn read_only_turn_with_no_resolved_entities_adds_no_evidence_message() {
        let (svc, node_service, _tempdir) = test_service().await;
        let node_id = create_ai_chat_node(&node_service).await;

        svc.append_assistant_message(
            &node_id,
            "I found 3 tasks.",
            None,
            Vec::new(),
            Vec::new(),
            None,
        )
        .await
        .expect("append");

        let history = load_node_history(&node_service, &node_id).await;
        assert!(!history.iter().any(|m| matches!(m.role, Role::System)));
    }

    /// Evidence must be self-describing for every write tool, not just the ones
    /// that happen to take a `content` argument. A bare tool name cannot be
    /// matched against the instruction it satisfied — which is the whole point.
    #[tokio::test]
    async fn every_write_tool_produces_a_usable_label() {
        // A relationship has no describing argument and returns no node id; it
        // must still render as something identifiable.
        let rel = completed_writes_from(&[exec(
            "create_relationship",
            serde_json::json!({
                "from_id": "nodespace://a",
                "to_id": "nodespace://b",
                "relationship_type": "mentions"
            }),
            serde_json::json!({"from_id": "nodespace://a", "to_id": "nodespace://b", "created": true}),
        )]);
        assert_eq!(rel.len(), 1);
        assert_eq!(rel[0].node_id, None);
        let label = rel[0].summary.as_deref().expect("relationship label");
        assert!(label.contains("nodespace://a"), "got {label:?}");
        assert!(label.contains("mentions"), "got {label:?}");
        assert!(label.contains("nodespace://b"), "got {label:?}");

        // A markdown import is the highest-stakes duplicate in the set: its
        // argument key is `markdown`, not `content`.
        let md = completed_writes_from(&[exec(
            "create_nodes_from_markdown",
            serde_json::json!({"markdown": "# Trip plan\n- flights"}),
            serde_json::json!({"created": 4}),
        )]);
        assert_eq!(md.len(), 1);
        assert!(
            md[0].summary.as_deref().unwrap_or("").contains("Trip plan"),
            "markdown import must be identifiable, got {:?}",
            md[0].summary
        );

        // Schemas are named, not contented.
        let schema = completed_writes_from(&[exec(
            "create_schema",
            serde_json::json!({"name": "album_to_listen"}),
            serde_json::json!({"created": true}),
        )]);
        assert_eq!(schema[0].summary.as_deref(), Some("album_to_listen"));

        // `update_schema` identifies its target by `schema_id`, not `name`. A
        // repeat here is destructive — `rename_fields` rekeys property data on
        // every existing node of the type — so it must not degrade to a bare
        // tool name.
        let updated = completed_writes_from(&[exec(
            "update_schema",
            serde_json::json!({
                "schema_id": "album_to_listen",
                "add_fields": [{"name": "rating", "field_type": "number"}]
            }),
            serde_json::json!({"updated": true}),
        )]);
        assert_eq!(updated[0].summary.as_deref(), Some("album_to_listen"));
    }

    /// The node id is stored as the `nodespace://` URI the tools actually
    /// return, matching the form the model uses to refer to nodes elsewhere.
    #[tokio::test]
    async fn node_id_is_recorded_as_the_uri_the_tool_returned() {
        let writes = completed_writes_from(&[exec(
            "create_node",
            serde_json::json!({"content": "a"}),
            serde_json::json!({"id": "nodespace://aaa"}),
        )]);
        assert_eq!(writes[0].node_id.as_deref(), Some("nodespace://aaa"));
    }

    /// A clipped label must be distinguishable from a complete one, and must not
    /// carry newlines that would reshape the evidence block.
    #[tokio::test]
    async fn long_and_multiline_summaries_are_clipped_and_flattened() {
        let writes = completed_writes_from(&[exec(
            "create_node",
            serde_json::json!({"content": format!("{}\nsecond line", "x".repeat(200))}),
            serde_json::json!({"id": "nodespace://a"}),
        )]);
        let s = writes[0].summary.as_deref().expect("summary");
        assert!(s.ends_with('…'), "clipped summary must be marked: {s:?}");
        assert!(!s.contains('\n'));
        assert_eq!(s.chars().count(), SUMMARY_MAX_CHARS + 1);
    }

    #[tokio::test]
    async fn empty_or_whitespace_reasoning_is_omitted() {
        let (svc, node_service, _tempdir) = test_service().await;
        let node_id = create_ai_chat_node(&node_service).await;

        // None and whitespace-only both persist no reasoning field.
        svc.append_assistant_message(
            &node_id,
            "Plain answer.",
            None,
            Vec::new(),
            Vec::new(),
            None,
        )
        .await
        .expect("append none");
        svc.append_assistant_message(
            &node_id,
            "Another answer.",
            Some("   "),
            Vec::new(),
            Vec::new(),
            None,
        )
        .await
        .expect("append whitespace");

        let history = load_node_history(&node_service, &node_id).await;
        let assistants: Vec<_> = history
            .iter()
            .filter(|m| matches!(m.role, Role::Assistant))
            .collect();
        assert_eq!(assistants.len(), 2);
        assert!(assistants.iter().all(|m| m.reasoning.is_none()));
    }

    #[tokio::test]
    async fn load_openai_compat_model_without_config_returns_clear_error() {
        let (svc, _node_service, _tempdir) = test_service().await;

        // No daemon.toml exists yet, so the config lookup returns None. This
        // must surface a specific "no config found" error, not fall through to
        // the GGUF path-resolution failure the bug report described.
        let events = svc
            .load_model_and_collect_events("openai-compat:missing-uuid", None)
            .await;

        let error_event = events
            .iter()
            .find(|e| e.event_type == "error")
            .expect("an error event should be emitted");
        let message = error_event
            .error_message
            .as_deref()
            .expect("error_message should be set");
        assert!(
            message.contains("No OpenAI-compatible provider config found"),
            "unexpected error message: {message}"
        );
        assert!(
            !message.to_lowercase().contains("gguf"),
            "error must not leak the GGUF path-resolution failure: {message}"
        );
    }

    #[tokio::test]
    async fn load_openai_compat_model_with_config_swaps_engine() {
        let (svc, _node_service, tempdir) = test_service().await;
        let config_path = tempdir.path().join("daemon.toml");
        let toml = r#"
[[openai_compat.configs]]
id = "abc-123"
name = "My Endpoint"
base_url = "http://127.0.0.1:9999/v1"
api_key = "sk-test"
"#;
        tokio::fs::write(&config_path, toml)
            .await
            .expect("write daemon.toml");

        let events = svc
            .load_model_and_collect_events("openai-compat:abc-123", None)
            .await;

        let ready_event = events
            .iter()
            .find(|e| e.event_type == "ready")
            .expect("a ready event should be emitted");
        assert_eq!(ready_event.engine_swapped, Some(true));
        assert!(events.iter().all(|e| e.event_type != "error"));
    }

    /// Regression: a single-model server
    /// config (no `/models` discovery) resolves every load to the same bare
    /// `openai-compat:<uuid>` model id regardless of the config's `model`
    /// field, so `replace_engine_if_changed`'s "already loaded" check cannot
    /// see an in-place edit to that field. The routing-probe cache must not
    /// piggyback on that check, or an edited config would keep serving a
    /// verdict measured against the model it used to point at.
    ///
    /// This does not exercise real suppression detection (both base_urls
    /// point at nothing, so the probe always errors — asserted as
    /// `routing_disabled: None` below, i.e. no ready-event field asserts a
    /// verdict either way). It exercises only that the second load re-probes
    /// rather than trusting the first load's cached state, which no assertion
    /// here could otherwise distinguish from a leaked stale verdict.
    #[tokio::test]
    async fn editing_model_on_an_undiscovered_config_forces_a_reprobe() {
        let (svc, _node_service, tempdir) = test_service().await;
        let config_path = tempdir.path().join("daemon.toml");
        let toml = r#"
[[openai_compat.configs]]
id = "abc-123"
name = "My Endpoint"
base_url = "http://127.0.0.1:9999/v1"
api_key = ""
model = "model-a"
"#;
        tokio::fs::write(&config_path, toml)
            .await
            .expect("write daemon.toml");

        // First load: no discovery segment in the id, so `model` resolves
        // from the config's pinned field.
        let first = svc
            .load_model_and_collect_events("openai-compat:abc-123", None)
            .await;
        assert!(
            first.iter().any(|e| e.event_type == "ready"),
            "first load should still reach ready even though the probe cannot reach anything: \
             {first:?}"
        );
        let key_after_first = svc
            .inner
            .shared
            .active_model_routing_key
            .lock()
            .await
            .clone();
        assert_eq!(
            key_after_first, None,
            "an errored probe must not record a routing key, or a later load would treat this \
             unmeasured model as already probed"
        );

        // Edit `model` in place — same config id, same base_url, different
        // served model. The Settings GUI writes exactly this shape.
        let edited_toml = r#"
[[openai_compat.configs]]
id = "abc-123"
name = "My Endpoint"
base_url = "http://127.0.0.1:9999/v1"
api_key = ""
model = "model-b"
"#;
        tokio::fs::write(&config_path, edited_toml)
            .await
            .expect("rewrite daemon.toml");

        // Second load uses the SAME model_id string ("openai-compat:abc-123")
        // as the first — this is the crux of the regression. If the routing
        // cache keyed on `swapped` (which will be `false`: same model_id,
        // engine already active), it would skip straight to the stale
        // `self.inner` state instead of consulting `config.routing_ok` or
        // re-probing.
        let second = svc
            .load_model_and_collect_events("openai-compat:abc-123", None)
            .await;
        assert!(
            second.iter().any(|e| e.event_type == "ready"),
            "second load should also reach ready: {second:?}"
        );

        // Both loads failed to reach anything (nothing listens on :9999), so
        // neither should have recorded a routing key — proving the second
        // load actually re-evaluated against "model-b" rather than silently
        // reusing whatever verdict (if any) "model-a" had produced.
        let key_after_second = svc
            .inner
            .shared
            .active_model_routing_key
            .lock()
            .await
            .clone();
        assert_eq!(
            key_after_second, None,
            "an errored re-probe on the edited model must not record a key either — if this were \
             Some((base_url, \"model-a\")) it would mean the second load reused the first load's \
             state instead of re-evaluating for model-b"
        );
    }

    // -- Cross-turn duplicate-write guard --------------------------------

    /// The guard's identity is `(tool, canonical_args)`, so a write is only
    /// usable by it if the canonical args survive persistence. Without this the
    /// whole mechanism degrades to a no-op that still looks wired up.
    #[tokio::test]
    async fn completed_writes_record_canonical_args_for_the_guard() {
        let writes = completed_writes_from(&[exec(
            "create_node",
            serde_json::json!({"content": "Buy milk", "node_type": "task"}),
            serde_json::json!({"id": "nodespace://n1"}),
        )]);
        assert_eq!(writes.len(), 1);
        let canonical = &writes[0].canonical_args;
        assert!(canonical.contains("Buy milk"), "got {canonical:?}");
    }

    /// Key order is a serialisation artefact, not a difference in intent. If it
    /// leaked into the identity, the same call re-emitted with reordered keys
    /// would slip past the guard.
    #[tokio::test]
    async fn canonical_args_ignore_key_order() {
        let a = completed_writes_from(&[exec(
            "create_node",
            serde_json::json!({"content": "x", "node_type": "task"}),
            serde_json::json!({"id": "nodespace://n1"}),
        )]);
        let b = completed_writes_from(&[exec(
            "create_node",
            serde_json::json!({"node_type": "task", "content": "x"}),
            serde_json::json!({"id": "nodespace://n1"}),
        )]);
        assert_eq!(a[0].canonical_args, b[0].canonical_args);
    }

    /// Oversized args are digested, never truncated and never dropped.
    ///
    /// A truncated string could compare equal to a different call sharing a long
    /// prefix — a size limit turned into a wrongly-blocked write. Dropping the
    /// identity avoids that but leaves the tool unguarded, and this is the tool
    /// where a repeat is worst: it duplicates an entire subtree. A digest is the
    /// form that has neither problem.
    #[tokio::test]
    async fn oversized_args_are_digested_rather_than_dropped_or_truncated() {
        let huge = "#".repeat(CANONICAL_ARGS_MAX_CHARS + 100);
        let writes = completed_writes_from(&[exec(
            "create_nodes_from_markdown",
            serde_json::json!({"markdown": huge}),
            serde_json::json!({"created": 3}),
        )]);
        assert_eq!(writes.len(), 1);
        let identity = &writes[0].canonical_args;
        assert!(
            identity.starts_with("sha256:"),
            "oversized args must still yield an identity, as a digest: {identity:?}"
        );
        // The point of the digest: the import itself is not copied back into the
        // chat node's own history.
        assert!(
            !identity.contains(&huge),
            "the import must not be re-stored"
        );
        // Exactly "sha256:" plus 64 hex chars. An exact length, not a loose
        // bound: a bound generous enough to be safe would also pass on a
        // truncated identity, which is the regression this is here to catch.
        assert_eq!(identity.len(), 71, "identity must be a full digest");
        // The evidence label is unaffected: the write is still reported.
        assert!(writes[0].summary.is_some());
    }

    /// The digest must identify the *specific* call, not merely "something big".
    /// If it collapsed all oversized calls together, a second, genuinely
    /// different import would be refused — a wrong suppression, the one failure
    /// mode this guard must not have.
    #[tokio::test]
    async fn different_oversized_imports_get_different_identities() {
        let identity_for = |body: &str| {
            completed_writes_from(&[exec(
                "create_nodes_from_markdown",
                serde_json::json!({ "markdown": body }),
                serde_json::json!({"created": 3}),
            )])[0]
                .canonical_args
                .clone()
        };
        let a = identity_for(&format!(
            "# Groceries\n{}",
            "a".repeat(CANONICAL_ARGS_MAX_CHARS)
        ));
        let b = identity_for(&format!(
            "# Recipes\n{}",
            "a".repeat(CANONICAL_ARGS_MAX_CHARS)
        ));
        assert_ne!(a, b, "distinct imports must not share an identity");
    }

    /// Key-order normalisation must survive the digest — it is applied to the
    /// canonical form *before* hashing. Were it applied after, the same call with
    /// reordered keys would hash differently and slip the guard above the cap
    /// while being caught below it.
    #[tokio::test]
    async fn oversized_identity_still_ignores_key_order() {
        let huge = "#".repeat(CANONICAL_ARGS_MAX_CHARS + 100);
        let a = completed_writes_from(&[exec(
            "create_nodes_from_markdown",
            serde_json::json!({"markdown": huge, "parent_id": "n1"}),
            serde_json::json!({"created": 3}),
        )]);
        let b = completed_writes_from(&[exec(
            "create_nodes_from_markdown",
            serde_json::json!({"parent_id": "n1", "markdown": huge}),
            serde_json::json!({"created": 3}),
        )]);
        assert_eq!(a[0].canonical_args, b[0].canonical_args);
        assert!(a[0].canonical_args.starts_with("sha256:"));
    }

    /// The guard reads its state back out of persisted messages. Every recorded
    /// guarded write carries an identity, so every one is replayed — including
    /// a digested one, which is precisely the case that used to be dropped.
    #[tokio::test]
    async fn prior_writes_are_rebuilt_from_persisted_messages() {
        let msgs = vec![AiChatMessage {
            role: "assistant".to_string(),
            content: "Added it.".to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: vec![
                AiChatCompletedWrite {
                    tool: "create_node".to_string(),
                    node_id: Some("nodespace://n1".to_string()),
                    summary: Some("Buy milk".to_string()),
                    canonical_args: r#"{"content":"Buy milk"}"#.to_string(),
                },
                AiChatCompletedWrite {
                    tool: "create_nodes_from_markdown".to_string(),
                    node_id: Some("nodespace://n2".to_string()),
                    summary: Some("big import".to_string()),
                    canonical_args: "sha256:abc123".to_string(),
                },
            ],
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        }];

        let prior = prior_writes_from_history(&msgs);
        assert_eq!(prior.len(), 2, "every guarded write must be replayed");
        assert_eq!(prior[0].node_id.as_deref(), Some("nodespace://n1"));
        assert_eq!(
            prior[1].canonical_args, "sha256:abc123",
            "a digested identity must be carried through unchanged"
        );
    }

    /// #2123 regression: the wire parameter carrying field values on
    /// create_node/update_node was renamed from `properties` to `field_values`
    /// (a parameter literally named `properties` collides with JSON Schema's
    /// own `properties` keyword and is silently dropped by the Gemma-4 chat
    /// template before the model ever sees it). `terse_write_fact` reads a
    /// completed write's recorded canonical args by that same key to render
    /// the "Fact: ..." line a later turn's history depends on, so it must read
    /// `field_values` — pins the exact key against a rendered assertion, not
    /// merely "does not panic", so a regression back to the old key name
    /// (which would silently render `None` for the field values) is caught.
    #[test]
    fn terse_write_fact_reads_field_values_key() {
        let history = node_history_from_messages(vec![AiChatMessage {
            role: "assistant".to_string(),
            content: "Marked the invoice as paid.".to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: vec![AiChatCompletedWrite {
                tool: "update_node".to_string(),
                node_id: Some("nodespace://n1".to_string()),
                summary: None,
                canonical_args: r#"{"id":"nodespace://n1","field_values":{"status":"paid"}}"#
                    .to_string(),
            }],
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        }]);

        let assistant = history
            .iter()
            .find(|m| matches!(m.role, Role::Assistant))
            .expect("assistant message present");
        assert!(
            assistant.content.contains("status") && assistant.content.contains("paid"),
            "the terse fact must surface the field value recorded under \
             'field_values' — production's actual wire key since #2123 — \
             instead of silently omitting it the way it would if this still \
             read the old 'properties' key: {:?}",
            assistant.content
        );
    }

    /// Build one persisted assistant turn carrying a single completed write.
    fn assistant_turn(content: &str, write: AiChatCompletedWrite) -> AiChatMessage {
        AiChatMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: vec![write],
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        }
    }

    fn user_turn(content: &str) -> AiChatMessage {
        AiChatMessage {
            role: "user".to_string(),
            content: content.to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: vec![],
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        }
    }

    /// The facts agent-matrix scenario 11d depends on ARE in its history.
    ///
    /// 11d ("What did we settle on that the rebuild has to respect?") fails for
    /// every model measured, and the leading hypothesis was that it is
    /// unwinnable by construction: the prompt template drops `role="tool"`
    /// history, so if the earlier `create_relationship` result never reached
    /// the model, no model could traverse a link it cannot see.
    ///
    /// This test settles that, and REFUTES it. Tool-role messages are indeed
    /// dropped by `node_history_from_messages` — the `_ => return Vec::new()`
    /// arm — but the writes they carried are not lost: each one is re-rendered
    /// as a terse "Fact: ..." line plus a system-role record of the write. So
    /// the turn going into 11d can see both endpoint ids AND the edge between
    /// them, stated twice.
    ///
    /// Which inverts the diagnosis. 11d is not starved of the fact; it is
    /// HANDED the answer. A model that reads its history and replies without
    /// traversing is behaving reasonably — the prompt is answerable from what
    /// it was given — and `TOOL_STRATEGY_RULES`'s first bullet ("CONVERSATIONAL
    /// TURNS USE NO TOOLS ... answer directly in text") points the same way.
    /// That makes the across-the-board failure a property of the scenario's
    /// setup rather than a harness defect or a capability gap, and it is why
    /// this is pinned as a test: the next person to read "every model returns
    /// tools: []" should find the refutation here rather than re-derive it.
    #[test]
    fn scenario_11d_history_already_contains_the_link_it_asks_about() {
        let history = node_history_from_messages(vec![
            user_turn("Log a decision: the reports page uses server-side rendering"),
            assistant_turn(
                "I logged the decision.",
                AiChatCompletedWrite {
                    tool: "create_node".to_string(),
                    node_id: Some("nodespace://dec1".to_string()),
                    summary: Some("the reports page uses server-side rendering".to_string()),
                    canonical_args:
                        r#"{"content":"server-side rendering","node_type":"text"}"#.to_string(),
                },
            ),
            user_turn("Add a task to rebuild the reports page"),
            assistant_turn(
                "Added the task.",
                AiChatCompletedWrite {
                    tool: "create_node".to_string(),
                    node_id: Some("nodespace://task1".to_string()),
                    summary: Some("rebuild the reports page".to_string()),
                    canonical_args:
                        r#"{"content":"rebuild the reports page","node_type":"task"}"#.to_string(),
                },
            ),
            user_turn("Point that rebuild task at the decision it has to respect"),
            assistant_turn(
                "Linked them.",
                AiChatCompletedWrite {
                    tool: "create_relationship".to_string(),
                    // Relationship writes report no node id — see
                    // `completed_writes_from`. The edge lives in `summary`.
                    node_id: None,
                    summary: Some(
                        "nodespace://task1 -[mentions]-> nodespace://dec1".to_string(),
                    ),
                    canonical_args: r#"{"from_id":"nodespace://task1","relationship_type":"mentions","to_id":"nodespace://dec1"}"#.to_string(),
                },
            ),
        ]);

        let rendered = history
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            rendered.contains("nodespace://task1") && rendered.contains("nodespace://dec1"),
            "both endpoint ids must survive into 11d's history, or the traversal \
             genuinely has nothing to act on: {rendered}"
        );
        assert!(
            rendered.contains("mentions"),
            "the recorded edge must survive into 11d's history — its absence is \
             what would have made the scenario unwinnable: {rendered}"
        );
        assert!(
            rendered.contains("server-side rendering"),
            "the DECISION's own text — the literal answer to 'what did we settle \
             on' — is present in history, which is why a model can answer 11d \
             without traversing anything: {rendered}"
        );
        assert!(
            !history.iter().any(|m| matches!(m.role, Role::Tool)),
            "tool-role messages really are dropped from rebuilt history, so the \
             assertions above are evidence that the writes are re-rendered \
             through another channel rather than that tool history survives"
        );
    }

    /// The facts agent-matrix scenario 6 depends on ARE in its history.
    ///
    /// 6 asserts the `[resolve_query, update_node]` subsequence for "The
    /// five-day one got signed off — mark it that way". The scenario's own
    /// comment explains the design intent: "the five-day one" is meant to be an
    /// INDIRECT reference that only `resolve_query` can resolve, deliberately
    /// chosen over the spec's name so a plain `search_nodes` could not shortcut
    /// it.
    ///
    /// That intent does not survive contact with the rendered history. The
    /// create_node write from scenario 4 is re-rendered with its property
    /// values inline — "properties estimated_days 5 ... (id nodespace://fw1)" —
    /// so the discriminator AND the node id are both sitting in the prompt as
    /// plain text. "The five-day one" is therefore a direct string match
    /// against history, not an indirect reference at all, and a model can go
    /// straight to `update_node` with the right id.
    ///
    /// This is the mechanism behind the observed failure where a capable model
    /// called `update_node`, produced the correct end state, and scored red
    /// only for skipping `resolve_query`. Pinned here because the fixture's
    /// comment asserts the opposite, and a reader trusting that comment would
    /// look for the bug in `resolve_query` rather than in the history.
    #[test]
    fn scenario_6_history_resolves_its_indirect_reference_directly() {
        let history = node_history_from_messages(vec![
            user_turn("Put one down for offline sync, still a draft, we reckon five days"),
            assistant_turn(
                "Added it.",
                AiChatCompletedWrite {
                    tool: "create_node".to_string(),
                    node_id: Some("nodespace://fw1".to_string()),
                    summary: Some("offline sync".to_string()),
                    canonical_args: r#"{"node_type":"feature_writeup","field_values":{"signed_off":false,"estimated_days":5},"content":"offline sync"}"#.to_string(),
                },
            ),
        ]);

        let rendered = history
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            rendered.contains("estimated_days 5"),
            "the day count scenario 6 discriminates on is rendered inline, which \
             is what turns 'the five-day one' into a direct string match: {rendered}"
        );
        assert!(
            rendered.contains("nodespace://fw1"),
            "the target node's id is in history too, so update_node needs no \
             separate resolution step to obtain it: {rendered}"
        );
    }

    /// What matrix scenario 12's rendered history does and does NOT contain.
    ///
    /// SCOPE, because an earlier version of this test was read as proving more
    /// than it does. It establishes that the history does not LEXICALLY STATE
    /// the ordering — no "biggest", "largest", "highest". It does NOT establish
    /// that the ordering is underivable, and those are different claims.
    ///
    /// They are different because the three estimates and all three ids are
    /// rendered inline, adjacent, in a uniform format. `max(9, 21, 4)` is an
    /// in-context comparison, so a model can pick the right id without reading
    /// anything back. Scenario 12 is therefore a test of comparative reference
    /// RESOLUTION, not of decomposition; the group header in
    /// scripts/eval/fixtures/agent-matrix.ts carries the full reasoning, and
    /// #2248 tracks the decomposition gap that remains open.
    ///
    /// The negative assertion is kept anyway, because it still pins something
    /// real: if a future change to `terse_write_fact` started emitting a
    /// superlative (a "largest estimate" summary line, say), 12d would degrade
    /// from "rank three values" to "match one word" — the exact decay that cost
    /// scenario 6 its indirection — and this test is what would catch it.
    ///
    /// The positive assertions are load-bearing in the other direction: they
    /// stop the negative one passing vacuously on an empty render, which a
    /// broken helper or a changed role filter would otherwise produce.
    #[test]
    fn scenario_12_history_states_the_values_but_not_the_ordering() {
        let history = node_history_from_messages(vec![
            user_turn("Log the checkout rewrite, we think nine days"),
            assistant_turn(
                "Logged it.",
                AiChatCompletedWrite {
                    tool: "create_node".to_string(),
                    node_id: Some("nodespace://fw10".to_string()),
                    summary: Some("checkout rewrite".to_string()),
                    canonical_args:
                        r#"{"node_type":"feature_writeup","field_values":{"estimated_days":9},"content":"checkout rewrite"}"#
                            .to_string(),
                },
            ),
            user_turn("Also the search indexer, that one's twenty-one days"),
            assistant_turn(
                "Logged it.",
                AiChatCompletedWrite {
                    tool: "create_node".to_string(),
                    node_id: Some("nodespace://fw11".to_string()),
                    summary: Some("search indexer".to_string()),
                    canonical_args:
                        r#"{"node_type":"feature_writeup","field_values":{"estimated_days":21},"content":"search indexer"}"#
                            .to_string(),
                },
            ),
            user_turn("And the audit log export, call it four days"),
            assistant_turn(
                "Logged it.",
                AiChatCompletedWrite {
                    tool: "create_node".to_string(),
                    node_id: Some("nodespace://fw12".to_string()),
                    summary: Some("audit log export".to_string()),
                    canonical_args:
                        r#"{"node_type":"feature_writeup","field_values":{"estimated_days":4},"content":"audit log export"}"#
                            .to_string(),
                },
            ),
        ]);

        let rendered = history
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        // Positive: the three instances and their estimates ARE in history.
        // Without these, the negative assertion below could pass on an empty
        // string — which would make the scenario look winnable while measuring
        // nothing at all.
        for (title, days, id) in [
            ("checkout rewrite", "estimated_days 9", "nodespace://fw10"),
            ("search indexer", "estimated_days 21", "nodespace://fw11"),
            ("audit log export", "estimated_days 4", "nodespace://fw12"),
        ] {
            assert!(
                rendered.contains(title),
                "setup instance '{title}' must be in history, or scenario 12 has \
                 nothing to compare: {rendered}"
            );
            assert!(
                rendered.contains(days),
                "'{days}' must be in history, or the comparison has no values to \
                 range over: {rendered}"
            );
            assert!(
                rendered.contains(id),
                "'{id}' must be in history — with every id inline, choosing the \
                 right one is a ranking problem rather than a lookup, which is \
                 what scenario 12 measures: {rendered}"
            );
        }

        // Negative: nothing in history STATES the ordering, so the prompt's
        // words ("the biggest") match no substring of it and 12d cannot be
        // answered by lookup alone. Note the limit of this claim: the values
        // themselves ARE inline, so the ranking is still derivable in-context.
        // See this test's docstring.
        let lowered = rendered.to_lowercase();
        for phrase in [
            "biggest",
            "largest",
            "longest",
            "highest",
            "most days",
            "the max",
        ] {
            assert!(
                !lowered.contains(phrase),
                "history must not name the ordering ('{phrase}'), or scenario 12's \
                 comparative reference degenerates into the direct string match \
                 that cost scenario 6 its indirection: {rendered}"
            );
        }
    }

    /// Matrix scenario 13's referent is absent from its rendered history.
    ///
    /// This is the test scenarios 6 and 12 could not have passed, and it is the
    /// point of seeding 13's state out of band.
    ///
    /// 6 named "the five-day one" and #2242 found `estimated_days 5` and the id
    /// both rendered inline. 12 named a comparative over three written values,
    /// and #2250's review found the values themselves inline, so the ranking was
    /// derivable without a read. Both failed for one underlying reason: every
    /// scalar a scored turn writes is replayed by `terse_write_fact`, so a
    /// referent the AGENT wrote is always in the prompt as literal text.
    ///
    /// 13's incident records are created through the CLI before the group's
    /// first turn, never by a tool call. `completed_writes_from` only records a
    /// turn's own tool executions, so none of that state reaches the rendered
    /// prompt — which this test asserts directly, on the real renderer.
    ///
    /// The history modelled here is 13's actual shape: the scored turn is the
    /// FIRST turn of its group, so the only thing preceding it is nothing at
    /// all. The assertions are written against a history containing an
    /// unrelated earlier exchange as well, which is the stricter case: it
    /// proves the referent is absent even when history is non-empty, so the
    /// negative assertions cannot pass merely because there is nothing to
    /// search.
    #[test]
    fn scenario_13_seeded_referent_is_absent_from_history() {
        let history = node_history_from_messages(vec![
            user_turn("What can you do?"),
            AiChatMessage {
                role: "assistant".to_string(),
                content: "I can help you track work in your graph.".to_string(),
                timestamp: None,
                reasoning: None,
                completed_writes: vec![],
                resolved_entities: Vec::new(),
                question: None,
                options: Vec::new(),
            },
        ]);

        let rendered = history
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        // Positive: history rendered at all. Without this the negative
        // assertions below would pass on an empty string, which is precisely
        // the vacuous-pass trap that let scenario 12's proof look sound.
        assert!(
            rendered.contains("What can you do?"),
            "the unrelated turn must render, or the absence assertions below \
             prove nothing: {rendered}"
        );

        // Negative: nothing about the seeded incidents is in the prompt. Each
        // of these is a separate route by which a model could shortcut the
        // lookup, and all must be closed for 13 to measure decomposition.
        for absent in [
            // The on-call name the prompt refers to.
            "rowan",
            // The target's title — what `contentMatches` scores on.
            "search index corruption",
            // The other two seeded records: present in the graph, and their
            // absence here is what makes the lookup discriminate rather than
            // guess.
            "checkout latency spike",
            "auth token expiry storm",
            // The type and the property name. Absent from HISTORY, which is
            // what this test covers — but NOT absent from the prompt overall:
            // workspace context retrieves the seeded schema semantically and
            // renders both into the system prompt. Asserted here anyway,
            // because history is a channel they have no business appearing in
            // and a change that put them there would be a real regression;
            // just do not read these two lines as proof the model cannot see
            // them. See
            // `scenario_13_seeded_schema_reaches_the_prompt_but_its_instances_do_not`
            // in packages/core/src/ops/context_ops.rs for the channel that
            // does carry them, and why 13 is still sound.
            "incident_report",
            "on_call",
        ] {
            assert!(
                !rendered.to_lowercase().contains(absent),
                "'{absent}' must NOT be in the rendered history — scenario 13's \
                 referent is seeded out of band precisely so no part of it \
                 reaches the prompt. A leak here means 13 has degraded into the \
                 direct string match that cost scenarios 6 and 12 their \
                 indirection: {rendered}"
            );
        }
    }

    /// Scenario 13's ACTUAL history shape: empty.
    ///
    /// The test above deliberately uses a non-empty unrelated history, which is
    /// the stricter case for the negative assertions — it proves the referent is
    /// absent even when there is text to find it in. This pins the shape 13
    /// really runs with, since 13 is the first and only scenario in its group.
    ///
    /// Trivial by construction, and that is the point worth recording: with no
    /// prior turns there is no history channel at all, so the referent's absence
    /// from it is structural rather than contingent on what got rendered.
    #[test]
    fn scenario_13_runs_with_no_prior_history() {
        let history = node_history_from_messages(Vec::new());
        assert!(
            history.is_empty(),
            "13 is the first turn of its group, so it has no prior history — the \
             referent cannot leak through a channel that carries nothing: \
             {history:?}"
        );
    }

    /// A turn's OWN write leaks the target's title into the next turn — which
    /// is why scenario 13 is a single-turn group.
    ///
    /// Discovered while proving 13's referent absent. The `Fact:` line for an
    /// `update_node` is id-and-values only, so that channel is clean. The
    /// EVIDENCE BLOCK is not: `completed_writes_message` renders
    /// `- update_node "<summary>" -> <id>`, and `write_summary_arg` resolves an
    /// update's summary through `content`/`title`, so the node's title lands in
    /// the prompt verbatim.
    ///
    /// For scenario 13 as written this is harmless: it is the only scenario in
    /// its group, so nothing ever reads the history its write produces. It is
    /// pinned anyway because it is a live constraint on that group — APPENDING
    /// A SECOND SCENARIO TO 13'S GROUP WOULD HAND THE MODEL THE SEEDED TITLE,
    /// re-creating exactly the direct-string-match defect that cost scenarios 6
    /// and 12 their indirection, and it would do so silently.
    ///
    /// Asserts current behavior deliberately. If `write_summary_arg` is ever
    /// narrowed for updates, this test fails and the constraint above can be
    /// relaxed.
    #[test]
    fn a_turns_own_write_leaks_its_targets_title_via_the_evidence_block() {
        let history = node_history_from_messages(vec![
            user_turn("The incident Rowan was on call for — mark it resolved"),
            assistant_turn(
                "Marked it resolved.",
                AiChatCompletedWrite {
                    tool: "update_node".to_string(),
                    node_id: Some("nodespace://inc2".to_string()),
                    summary: Some("search index corruption".to_string()),
                    canonical_args: r#"{"id":"nodespace://inc2","field_values":{"resolved":true}}"#
                        .to_string(),
                },
            ),
        ]);

        let rendered = history
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        // The write's own fact renders — the turn did happen.
        assert!(
            rendered.contains("nodespace://inc2"),
            "the turn's own write must render as a fact: {rendered}"
        );

        // The `Fact:` phrasing for update_node is id-and-values only.
        assert!(
            rendered.contains("was updated with resolved true"),
            "update_node's fact should carry its values, not its title: {rendered}"
        );

        // But the evidence block DOES carry the title. Pinned as the constraint
        // it is, not asserted away.
        assert!(
            rendered.contains(r#"update_node "search index corruption""#),
            "EXPECTED LEAK: the evidence block renders the update's summary, \
             which resolves to the node's title. If this no longer holds, \
             `write_summary_arg` was narrowed and scenario 13's group may safely \
             gain a second scenario — see this test's docstring: {rendered}"
        );

        // The on-call value is not part of the WRITE's rendering. It is in the
        // rendered history — but only because the user's own prompt is replayed
        // verbatim, which is unavoidable and harmless: the model already said
        // it. What matters is that the write does not ALSO surface it as
        // established fact, so scope this to the write's own lines rather than
        // to the whole transcript.
        let write_lines: String = rendered
            .lines()
            .filter(|l| l.starts_with("Fact:") || l.trim_start().starts_with("- "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !write_lines.to_lowercase().contains("rowan"),
            "the on-call value is not part of this write and must not be \
             rendered as a completed fact: {write_lines}"
        );
    }

    /// The blended retrieval query is assembled from the same history the turn
    /// renders, so this drives real `AiChatMessage`s through
    /// `node_history_from_messages` rather than hand-building `ChatMessage`s.
    ///
    /// Pins the query builder against the real history shape: the earlier
    /// turn's discriminating words reach the query, and the synthetic
    /// completed-writes record — which sits between the assistant turn and the
    /// follow-up — does not.
    ///
    /// It does not pin that `run_ai_chat_turn` calls this rather than embedding
    /// `user_message` directly; that line needs live inference to reach, so the
    /// ADR-048 seam test is what exercises it end to end.
    #[test]
    fn retrieval_query_blends_history_and_excludes_completed_writes() {
        let history = node_history_from_messages(vec![
            AiChatMessage {
                role: "user".to_string(),
                content: "Add a conference proposal for Redwood Summit".to_string(),
                timestamp: None,
                reasoning: None,
                completed_writes: vec![],
                resolved_entities: Vec::new(),
                question: None,
                options: Vec::new(),
            },
            AiChatMessage {
                role: "assistant".to_string(),
                content: "Added the Redwood Summit proposal.".to_string(),
                timestamp: None,
                reasoning: None,
                completed_writes: vec![AiChatCompletedWrite {
                    tool: "create_node".to_string(),
                    node_id: Some("nodespace://p1".to_string()),
                    summary: Some("Redwood Summit".to_string()),
                    canonical_args: r#"{"content":"Redwood Summit"}"#.to_string(),
                }],
                resolved_entities: Vec::new(),
                question: None,
                options: Vec::new(),
            },
        ]);

        // The completed-writes record really is in the rendered history — so its
        // absence from the query below is the filter working, not a vacuous pass.
        assert!(
            history.iter().any(|m| m.role == Role::System),
            "fixture must contain the synthetic completed-writes message"
        );

        let query = schema_retrieval_query(&history, "Set the Redwood one to rejected");

        assert!(
            query.contains("conference proposal"),
            "the earlier turn's discriminating words must reach the query: {query:?}"
        );
        assert!(
            !query.contains("Record of graph writes"),
            "completed-writes boilerplate must not dilute the query: {query:?}"
        );
        assert!(
            query.ends_with("Set the Redwood one to rejected"),
            "the current message must stay last: {query:?}"
        );
    }

    /// A first turn has no history, so the query must be the message alone —
    /// byte-identical to what retrieval received before blending existed.
    #[test]
    fn retrieval_query_for_first_turn_is_the_message_alone() {
        assert_eq!(
            schema_retrieval_query(&[], "Add an invoice for $500"),
            "Add an invoice for $500"
        );
    }

    /// Updates are idempotent: re-setting the same status or content is a no-op,
    /// not a duplicate. Carrying them into the guard would block a user
    /// legitimately re-asserting a value.
    #[tokio::test]
    async fn idempotent_updates_are_not_carried_into_the_guard() {
        let msgs = vec![AiChatMessage {
            role: "assistant".to_string(),
            content: "Done.".to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: vec![
                AiChatCompletedWrite {
                    tool: "update_task_status".to_string(),
                    node_id: Some("nodespace://t1".to_string()),
                    summary: Some("t1".to_string()),
                    canonical_args: r#"{"status":"done"}"#.to_string(),
                },
                AiChatCompletedWrite {
                    tool: "update_node".to_string(),
                    node_id: Some("nodespace://t2".to_string()),
                    summary: Some("t2".to_string()),
                    canonical_args: r#"{"content":"x"}"#.to_string(),
                },
                AiChatCompletedWrite {
                    tool: "update_schema".to_string(),
                    node_id: None,
                    summary: Some("s1".to_string()),
                    canonical_args: r#"{"schema_id":"s1"}"#.to_string(),
                },
            ],
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        }];

        assert!(
            prior_writes_from_history(&msgs).is_empty(),
            "no update tool may be guarded across turns"
        );
    }

    /// The guarded set must stay a subset of the tools recognised as writes:
    /// a guarded tool absent from `is_write_tool` would never have its args
    /// recorded, so the guard could never fire for it.
    ///
    /// Iterates the registry itself rather than a hardcoded list. A literal
    /// list would pass vacuously on exactly the change this is meant to catch —
    /// a newly added tool simply would not appear in it.
    #[tokio::test]
    async fn every_guarded_tool_is_also_recorded_as_a_write() {
        use nodespace_agent::local_agent::tools::Tool;

        for tool in Tool::ALL {
            let name = tool.name();
            if is_cross_turn_guarded_tool(name) {
                assert!(
                    is_write_tool(name),
                    "{name} is guarded across turns but not recorded as a write, \
                     so its canonical args are never persisted and the guard cannot fire"
                );
            }
        }
    }

    /// Every guarded tool must also produce an evidence label, otherwise a
    /// refusal can only say "duplicate" without naming what already exists.
    #[tokio::test]
    async fn every_guarded_tool_can_describe_what_it_wrote() {
        use nodespace_agent::local_agent::tools::Tool;

        for tool in Tool::ALL.iter().filter(|t| t.duplicate_is_destructive()) {
            let name = tool.name();
            assert!(
                write_summary_arg(name).is_some() || name == "create_relationship",
                "{name} is guarded but has no way to describe its write, so a \
                 refusal could not name the existing node"
            );
        }
    }

    // -- OpenAI-compat discovery cache ---------------------

    fn fake_discovered_model(label: &str) -> nodespace_agent::agent_types::ModelInfo {
        use nodespace_agent::local_agent::openai_compat_discovery::discovered_model_info;
        discovered_model_info("test-config", "Test Provider", label)
    }

    /// Seed the discovery cache directly (test-only access via `Arc::get_mut`,
    /// same pattern `engine_swap_completes_when_model_info_hangs` uses for
    /// `model_spec_snapshot_timeout`), bypassing the network round trip that
    /// would otherwise back it.
    async fn seed_discovery_cache(
        svc: &LocalAgentServiceImpl,
        fetched_at: std::time::Instant,
        models: Vec<nodespace_agent::agent_types::ModelInfo>,
    ) {
        let mut cache = svc.inner.shared.openai_compat_discovery_cache.lock().await;
        *cache = Some((fetched_at, models));
    }

    /// A fresh cache entry is served as-is — no endpoint is queried, so an
    /// empty `daemon.toml` (zero configured endpoints, which would otherwise
    /// make discovery trivially return the same empty result) cannot mask a
    /// bug here: the cached entry contains one fake model, which only comes
    /// back if the cache path is actually taken.
    #[tokio::test]
    async fn fresh_cache_entry_is_returned_without_requerying() {
        let (svc, _node_service, _tempdir) = test_service().await;
        let cached = vec![fake_discovered_model("cached-model")];
        seed_discovery_cache(&svc, std::time::Instant::now(), cached.clone()).await;

        let result = svc.discover_openai_compat_models(false).await;

        assert_eq!(
            result.iter().map(|m| &m.id).collect::<Vec<_>>(),
            cached.iter().map(|m| &m.id).collect::<Vec<_>>(),
            "a fresh cache entry must be served as-is, not re-queried"
        );
    }

    /// A cache entry older than the TTL is not returned — the discovery round
    /// re-runs (against zero configured endpoints here, so it resolves to
    /// empty), proving staleness is actually checked rather than the cache
    /// being treated as permanent.
    #[tokio::test]
    async fn stale_cache_entry_triggers_a_fresh_discovery_round() {
        let (svc, _node_service, _tempdir) = test_service().await;
        let stale_at = std::time::Instant::now()
            .checked_sub(OPENAI_COMPAT_DISCOVERY_CACHE_TTL + std::time::Duration::from_secs(1))
            .expect("test clock has enough headroom to go this far back");
        seed_discovery_cache(&svc, stale_at, vec![fake_discovered_model("stale-model")]).await;

        let result = svc.discover_openai_compat_models(false).await;

        assert!(
            result.is_empty(),
            "a stale cache entry must not be served; expected a fresh (empty, \
             no endpoints configured) discovery round, got {result:?}"
        );
    }

    /// `force_refresh` bypasses even a fresh cache entry — the explicit
    /// "Refresh remote models" action must always re-query.
    #[tokio::test]
    async fn force_refresh_bypasses_a_fresh_cache_entry() {
        let (svc, _node_service, _tempdir) = test_service().await;
        seed_discovery_cache(
            &svc,
            std::time::Instant::now(),
            vec![fake_discovered_model("cached-model")],
        )
        .await;

        let result = svc.discover_openai_compat_models(true).await;

        assert!(
            result.is_empty(),
            "force_refresh must bypass the cache and re-query (no endpoints \
             configured here, so the fresh round is empty), got {result:?}"
        );
    }

    /// A successful discovery round populates the cache, so a second call
    /// within the TTL is served from it rather than querying again.
    #[tokio::test]
    async fn discovery_result_is_cached_for_subsequent_calls() {
        let (svc, _node_service, _tempdir) = test_service().await;

        // No endpoints configured, so the first (uncached) round is empty —
        // but it must still populate the cache with that empty result rather
        // than leaving it `None`, otherwise every call would re-run discovery.
        let first = svc.discover_openai_compat_models(false).await;
        assert!(first.is_empty());

        let first_fetched_at = svc
            .inner
            .shared
            .openai_compat_discovery_cache
            .lock()
            .await
            .as_ref()
            .expect(
                "a completed discovery round must populate the cache even when \
                 the result is empty, otherwise ListModels never benefits from it",
            )
            .0;

        // The distinguishing assertion: a second call must be served from the
        // cache, not re-discovered — if it re-ran discovery, this would
        // repopulate the cache with a new `Instant`, changing the timestamp.
        let _ = svc.discover_openai_compat_models(false).await;
        let second_fetched_at = svc
            .inner
            .shared
            .openai_compat_discovery_cache
            .lock()
            .await
            .as_ref()
            .expect("cache must remain populated")
            .0;

        assert_eq!(
            first_fetched_at, second_fetched_at,
            "a second call within the TTL must be served from the cache, not \
             re-discovered — the fetch timestamp must not change"
        );
    }

    // -- Model-manager init failure (degraded mode, not a panic) ------------

    /// The real environmental failure this regression guards against:
    /// `GgufModelManager::with_dir` (what `GgufModelManager::new` calls after
    /// resolving the default directory) fails, without panicking, when a
    /// plain file already occupies the path it needs to `create_dir_all` —
    /// one of the two ordinary conditions (`$HOME` unset is the other,
    /// documented on `default_models_dir`) that used to reach an `expect` in
    /// `LocalAgentServiceImpl::new`.
    #[test]
    fn gguf_model_manager_new_fails_cleanly_when_models_dir_path_is_a_file() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let occupied_path = tempdir.path().join("models");
        std::fs::write(&occupied_path, b"not a directory").expect("create blocking file");

        let result = GgufModelManager::with_dir(occupied_path);

        assert!(
            result.is_err(),
            "create_dir_all over an existing file must fail (and be caught as an \
             Err, not a panic) rather than succeed"
        );
    }

    /// Drives `SharedLocalAgent::new` itself — not `from_model_manager`'s
    /// injected shortcut — through a real `GgufModelManager::new()` failure, by
    /// pointing `$HOME` at a directory whose `.nodespace/models` path is
    /// occupied by a plain file. This is the one test that actually exercises
    /// the `match GgufModelManager::new() { Ok(..) => Some(..), Err(..) =>
    /// None }` arm that replaced the original `.expect(...)`: the other
    /// degraded-mode tests construct via `from_model_manager(..., None, ..)`
    /// directly and would keep passing even if that match arm regressed back
    /// to a panic.
    ///
    /// Mutating `$HOME` is process-global, so this is deliberately narrow: the
    /// window is exactly the synchronous `SharedLocalAgent::new` call (it
    /// performs no `.await`, so no other task can interleave before `$HOME` is
    /// restored), and a repo-wide audit confirms no other test in this crate's
    /// unit-test binary reads `$HOME` — `SettingsServiceImpl::
    /// with_default_path` and `assembly::build_shared_services`'s
    /// `dirs::home_dir()` calls are only reached from real daemon startup
    /// (`main.rs`) and from separate integration-test *binaries* (their own
    /// OS processes, unaffected by an env mutation in this one).
    #[tokio::test]
    async fn shared_local_agent_new_survives_a_real_model_manager_init_failure() {
        let fake_home = tempfile::TempDir::new().expect("fake home tempdir");
        let models_path = fake_home.path().join(".nodespace").join("models");
        std::fs::create_dir_all(
            models_path
                .parent()
                .expect("models path has a .nodespace parent"),
        )
        .expect("create .nodespace dir");
        std::fs::write(&models_path, b"not a directory").expect("occupy the models path");

        // Under a separate tempdir from `fake_home` so the config path and the
        // (broken) models directory don't collide.
        let config_tempdir = tempfile::TempDir::new().expect("config tempdir");
        let daemon_config_path = config_tempdir.path().join("daemon.toml");

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", fake_home.path());
        let shared = SharedLocalAgent::new(daemon_config_path);
        match original_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }

        // The construction above must not have panicked (if it had, this test
        // would already be reported as a failure) and must have landed in
        // degraded mode, not silently succeeded with some other directory.
        assert!(
            shared.model_manager.is_none(),
            "a real GgufModelManager::new() failure during SharedLocalAgent::new \
             must degrade (model_manager: None), not panic or silently recover"
        );
    }

    /// The regression this issue guards against: a failed `GgufModelManager`
    /// init must degrade this database's local-model-management RPCs, not
    /// crash the process. Every RPC that reaches the model manager returns a
    /// clean error (or, for `list_models`, an empty local catalog) instead of
    /// panicking — proving the daemon and every other open database survive a
    /// single database's model-manager init failure rather than going down
    /// with it.
    #[tokio::test]
    async fn model_management_rpcs_degrade_when_model_manager_init_failed() {
        let (svc, _node_service, _tempdir) = test_service_without_model_manager().await;

        // list_models: no local GGUF catalog, but the call itself succeeds —
        // OpenAI-compatible discovery (independent of the local manager) must
        // still be reachable, matching how a missing OpenAI-compat endpoint
        // already contributes nothing rather than failing the whole listing.
        let models = svc
            .list_models(Request::new(ListModelsRequest {
                force_refresh: false,
            }))
            .await
            .expect("list_models must not fail outright when the model manager is absent")
            .into_inner()
            .models;
        assert!(
            models.is_empty(),
            "no GGUF catalog rows should be reported without a model manager"
        );

        // Every other model-management RPC reports UNAVAILABLE rather than
        // panicking or otherwise misbehaving.
        let download_err = svc
            .download_model(Request::new(DownloadModelRequest {
                model_id: "gemma-4-e4b-q4km".to_string(),
            }))
            .await
            .expect_err("download_model must fail cleanly, not panic");
        assert_eq!(download_err.code(), tonic::Code::Unavailable);

        let delete_err = svc
            .delete_model(Request::new(DeleteModelRequest {
                model_id: "gemma-4-e4b-q4km".to_string(),
            }))
            .await
            .expect_err("delete_model must fail cleanly, not panic");
        assert_eq!(delete_err.code(), tonic::Code::Unavailable);

        let load_err = svc
            .load_model(Request::new(LoadModelRequest {
                model_id: "gemma-4-e4b-q4km".to_string(),
            }))
            .await
            .expect_err("load_model must fail cleanly, not panic");
        assert_eq!(load_err.code(), tonic::Code::Unavailable);

        let unload_err = svc
            .unload_model(Request::new(UnloadModelRequest {}))
            .await
            .expect_err("unload_model must fail cleanly, not panic");
        assert_eq!(unload_err.code(), tonic::Code::Unavailable);

        let cancel_err = svc
            .cancel_model_download(Request::new(CancelModelDownloadRequest {
                model_id: "gemma-4-e4b-q4km".to_string(),
            }))
            .await
            .expect_err("cancel_model_download must fail cleanly, not panic");
        assert_eq!(cancel_err.code(), tonic::Code::Unavailable);

        let recommended_err = svc
            .recommended_model(Request::new(RecommendedModelRequest {}))
            .await
            .expect_err("recommended_model must fail cleanly, not panic");
        assert_eq!(recommended_err.code(), tonic::Code::Unavailable);
    }

    /// The other half of the regression: everything NOT routed through the
    /// GGUF model manager keeps working normally on a database whose
    /// `GgufModelManager` failed to initialize — an ai-chat turn runs to
    /// completion end to end. A database opened in this degraded mode is
    /// still a working database, just one without local GGUF model
    /// management; it is not an unopenable or half-broken one.
    #[tokio::test]
    async fn ai_chat_turn_still_works_when_model_manager_init_failed() {
        let (svc, node_service, _tempdir) = test_service_without_model_manager().await;
        svc.replace_engine_if_changed("stub-model", Arc::new(StubEngine::new("Hello there")))
            .await;

        let node_id = create_processing_node_with_user_message(&node_service, "Hi").await;
        svc.maybe_handle_ai_chat_node(&node_id).await;

        let ai_chat = get_ai_chat(&node_service, &node_id).await;
        assert_eq!(ai_chat.turn_status, "idle");
        assert_eq!(ai_chat.messages.len(), 2);
        assert_eq!(ai_chat.messages[1].role, "assistant");
        assert_eq!(ai_chat.messages[1].content, "Hello there");
    }
}
