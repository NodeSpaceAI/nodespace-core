//! Tauri commands for the local agent (ReAct loop + session management).
//!
//! Bridges the Svelte frontend to [`LocalAgentService`] via Tauri IPC.
//! Streaming output is forwarded to the frontend through Tauri event channels.
//!
//! The `ManagedAgentState` wrapper holds a `RwLock<LocalAgentService>`
//! that starts with a `NoOpInferenceEngine`. When a model is loaded via
//! `ensure_model_ready`, the engine is swapped to a real
//! `LlamaChatInferenceEngine`.
//!
//! Issue #1008

use crate::agent_events;
use crate::commands::nodes::CommandError;
use async_trait::async_trait;
use nodespace_agent::agent_types::{
    AgentSession, AgentToolExecutor, AgentTurnResult, ChatInferenceEngine, ChatModelSpec,
    InferenceError, InferenceUsage, LocalAgentStatus, ModelManager, ModelStatus, StreamingChunk,
};
use nodespace_agent::local_agent::agent_loop::LocalAgentService;
use nodespace_agent::local_agent::composite_model_manager::CompositeModelManager;
use nodespace_nlp_engine::chat::ChatConfig;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Placeholder inference engine (returns "no model loaded")
// ---------------------------------------------------------------------------

/// Stub inference engine used when no chat model is loaded.
///
/// Every method returns [`InferenceError::NoModelLoaded`]. This allows
/// the `LocalAgentService` to be constructed at startup without a real
/// model. When a model is loaded via the model manager, the
/// `ManagedAgentState` is re-initialized with a real engine.
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

    async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
        Ok(None)
    }

    async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
        // Rough estimate: 1 token ≈ 4 chars
        Ok((text.len() as f32 / 4.0).ceil() as u32)
    }
}

// ---------------------------------------------------------------------------
// ManagedAgentState (Tauri managed state)
// ---------------------------------------------------------------------------

/// Tauri managed state for the local agent subsystem.
///
/// Holds the active `LocalAgentService` behind a `RwLock` so it can be
/// replaced when a new model is loaded, or cleared when the model is unloaded.
///
/// The service uses trait objects (`dyn ChatInferenceEngine` and
/// `dyn AgentToolExecutor`) to avoid propagating generics to the Tauri state.
pub struct ManagedAgentState {
    inner: RwLock<LocalAgentService<dyn ChatInferenceEngine, dyn AgentToolExecutor>>,
    app_services: crate::app_services::AppServices,
    /// Model ID currently installed in the inference engine (None = NoOp).
    /// Uses Mutex (not RwLock) so check-and-set in replace_engine_if_changed is atomic.
    active_model_id: tokio::sync::Mutex<Option<String>>,
}

impl ManagedAgentState {
    /// Create with a no-op inference engine.
    ///
    /// The `app_services` parameter is used to resolve NodeService and
    /// NodeEmbeddingService when constructing the tool executor. At startup,
    /// services may not be initialized yet, so the executor starts without them.
    pub fn new(app_services: crate::app_services::AppServices) -> Self {
        use nodespace_agent::local_agent::tools::GraphToolExecutor;

        let engine: Arc<dyn ChatInferenceEngine> = Arc::new(NoOpInferenceEngine);
        // At startup, services aren't initialized yet. The executor handles
        // None gracefully (returns "service unavailable" errors).
        let executor: Arc<dyn AgentToolExecutor> = Arc::new(GraphToolExecutor {
            node_service: None,
            embedding_service: None,
        });
        let service = LocalAgentService::new(engine, executor, None);

        Self {
            inner: RwLock::new(service),
            app_services,
            active_model_id: tokio::sync::Mutex::new(None),
        }
    }

    /// Get a read reference to the inner service.
    pub async fn service(
        &self,
    ) -> tokio::sync::RwLockReadGuard<
        '_,
        LocalAgentService<dyn ChatInferenceEngine, dyn AgentToolExecutor>,
    > {
        self.inner.read().await
    }

    /// Replace the inference engine (called when a model is loaded).
    ///
    /// Creates a fresh `LocalAgentService` with the new engine and a tool
    /// executor backed by the current services from AppServices.
    /// Existing sessions are dropped.
    pub async fn replace_engine(&self, engine: Arc<dyn ChatInferenceEngine>) {
        use nodespace_agent::local_agent::tools::GraphToolExecutor;

        // Resolve services from AppServices (should be initialized by now).
        let node_service = self.app_services.node_service().await.ok();
        let embedding_service = self.app_services.embedding_service().await.ok();

        let executor: Arc<dyn AgentToolExecutor> = Arc::new(
            GraphToolExecutor::new_with_optional_services(node_service.clone(), embedding_service.clone()),
        );

        // Create skill pipeline with embedding service for intent-driven skill injection
        let skill_pipeline = embedding_service.map(|emb| {
            Arc::new(nodespace_agent::skill_pipeline::SkillPipeline::new(Some(
                emb,
            )))
        });

        // Create prompt assembler backed by NodeService so prompt content
        // comes from graph-stored prompt nodes rather than hardcoded templates.
        let prompt_assembler = node_service
            .map(|ns| Arc::new(nodespace_agent::prompt_assembler::PromptAssembler::new(ns)));

        let service = LocalAgentService::new_with_assembler(engine, executor, skill_pipeline, prompt_assembler);

        let mut guard = self.inner.write().await;
        *guard = service;

        tracing::info!("ManagedAgentState: inference engine replaced");
    }

    /// Replace the engine only if the given model_id differs from what's currently active.
    ///
    /// The check-and-set is performed under a single Mutex lock to prevent two
    /// concurrent callers from both passing the check and both replacing the engine.
    ///
    /// Returns true if the engine was replaced (caller should create a new session),
    /// false if the model was already active (sessions are preserved).
    pub async fn replace_engine_if_changed(
        &self,
        model_id: &str,
        engine: Arc<dyn ChatInferenceEngine>,
    ) -> bool {
        let active = self.active_model_id.lock().await;
        if active.as_deref() == Some(model_id) {
            return false;
        }
        // Drop the lock before the async replace_engine call (which acquires inner write lock).
        // This is safe: we've already decided to replace; any concurrent caller will block
        // on the Mutex and then see the updated active_model_id after we re-acquire.
        drop(active);

        self.replace_engine(engine).await;

        *self.active_model_id.lock().await = Some(model_id.to_string());
        true
    }

    /// Clear the active model ID (called on reset/unload).
    pub async fn clear_active_model(&self) {
        *self.active_model_id.lock().await = None;
    }

    /// Reset the inference engine to NoOp (called during graceful shutdown).
    ///
    /// Replaces the real inference engine with a NoOpInferenceEngine,
    /// causing any Arc references to the ChatEngine to be dropped.
    /// This must be called BEFORE `release_llama_backend()` to ensure
    /// proper cleanup order and avoid use-after-free crashes in Metal.
    pub async fn reset_to_noop_engine(&self) {
        use nodespace_agent::local_agent::tools::GraphToolExecutor;

        let engine: Arc<dyn ChatInferenceEngine> = Arc::new(NoOpInferenceEngine);
        let executor: Arc<dyn AgentToolExecutor> = Arc::new(GraphToolExecutor {
            node_service: None,
            embedding_service: None,
        });
        let service = LocalAgentService::new(engine, executor, None);

        let mut guard = self.inner.write().await;
        *guard = service;
        drop(guard);

        // Clear active model so the next ensure_model_ready always re-installs
        // the real engine rather than skipping it as "already active".
        self.clear_active_model().await;

        tracing::debug!("ManagedAgentState: inference engine reset to NoOp");
    }

    /// Reset the inference engine to NoOp (for use in sync shutdown context).
    ///
    /// Creates a fresh single-threaded runtime to avoid the "cannot block_on
    /// inside a runtime" panic when called from within a Tokio runtime thread.
    ///
    /// Must be called BEFORE releasing GPU resources to ensure proper cleanup
    /// order and avoid use-after-free crashes in Metal.
    pub fn reset_engine_sync(&self) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build shutdown runtime");
        rt.block_on(self.reset_to_noop_engine());
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Helper to map arbitrary errors into [`CommandError`].
fn agent_error(message: impl Into<String>) -> CommandError {
    CommandError {
        message: message.into(),
        code: "AGENT_ERROR".to_string(),
        details: None,
    }
}

// ---------------------------------------------------------------------------
// Model status event payload
// ---------------------------------------------------------------------------

/// Payload for `model://status` events.
#[derive(Debug, Clone, Serialize)]
struct ModelStatusEvent {
    model_id: String,
    status: String,
    message: Option<String>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Ensure a model is downloaded, loaded, and the inference engine is ready.
///
/// This is the main entry point for the frontend to prepare the local agent.
/// It handles the full lifecycle: download → load → engine swap.
///
/// Emits `model://status` events for each phase transition so the frontend
/// can update the status bar.
#[tauri::command]
/// Returns `true` if the inference engine was (re-)installed, meaning existing
/// sessions were dropped and the caller must create a new session.
/// Returns `false` if the model was already loaded and the engine is unchanged.
pub async fn ensure_model_ready(
    model_id: String,
    app: AppHandle,
    manager: State<'_, Arc<CompositeModelManager>>,
    agent_state: State<'_, ManagedAgentState>,
) -> Result<bool, CommandError> {
    tracing::info!(model_id = %model_id, "ensure_model_ready called");

    // Check current model status
    let models = manager
        .list()
        .await
        .map_err(|e| agent_error(e.to_string()))?;
    let model = models
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| agent_error(format!("Unknown model: {model_id}")))?;

    // If the requested model is already loaded, skip engine replacement to
    // preserve active sessions.
    let already_loaded = manager
        .loaded_model()
        .await
        .ok()
        .flatten()
        .map(|id| id == model_id)
        .unwrap_or(false);
    tracing::info!(model_id = %model_id, already_loaded, "ensure_model_ready: checking if already loaded");
    // For Ollama models, "already loaded" only means the model is warm in Ollama's memory.
    // The ManagedAgentState engine may still be NoOp after an app restart — always
    // (re-)install the OllamaInferenceEngine so the agent can actually call it.
    if already_loaded && !CompositeModelManager::is_ollama(&model_id) {
        return Ok(false); // engine unchanged
    }

    // --- Ollama fast path ---
    if CompositeModelManager::is_ollama(&model_id) {
        use nodespace_agent::local_agent::ollama_inference::OllamaInferenceEngine;

        let ollama_name = CompositeModelManager::strip_ollama_prefix(&model_id).to_string();

        // Emit loading status
        let _ = app.emit(
            agent_events::MODEL_STATUS,
            &ModelStatusEvent {
                model_id: model_id.clone(),
                status: "loading".to_string(),
                message: Some(format!("Connecting to Ollama model {}...", ollama_name)),
            },
        );

        // Warm-load the model in Ollama (keeps it resident in memory)
        manager
            .load(&model_id)
            .await
            .map_err(|e| agent_error(format!("Failed to load Ollama model: {e}")))?;

        // Create the Ollama inference engine using the same base URL as the model manager,
        // so any configured Ollama URL is respected for both listing and inference.
        let ollama_base_url = manager.ollama_manager().base_url().to_string();
        let engine = OllamaInferenceEngine::with_base_url(ollama_name.clone(), ollama_base_url);
        let swapped = agent_state
            .replace_engine_if_changed(&model_id, Arc::new(engine))
            .await;

        let _ = app.emit(
            agent_events::MODEL_STATUS,
            &ModelStatusEvent {
                model_id: model_id.clone(),
                status: "ready".to_string(),
                message: Some(format!("Ollama model {} ready", ollama_name)),
            },
        );

        tracing::info!(
            model = %ollama_name,
            engine_swapped = swapped,
            "Ollama model loaded and inference engine ready"
        );
        return Ok(swapped); // true only if engine was replaced (sessions dropped)
    }

    match &model.status {
        ModelStatus::Loaded => {
            tracing::info!("Model '{}' already loaded", model_id);
            return Ok(false); // engine unchanged
        }
        ModelStatus::Downloading { .. } | ModelStatus::Verifying => {
            return Err(agent_error(format!(
                "Model '{}' is currently being downloaded",
                model_id
            )));
        }
        ModelStatus::Error { message } => {
            tracing::warn!(
                "Model '{}' in error state: {}, retrying...",
                model_id,
                message
            );
            // Fall through to re-download
        }
        ModelStatus::NotDownloaded => {
            // Need to download first
            let _ = app.emit(
                agent_events::MODEL_STATUS,
                &ModelStatusEvent {
                    model_id: model_id.clone(),
                    status: "downloading".to_string(),
                    message: Some(format!("Downloading {}...", model_id)),
                },
            );

            // Register progress callback for GGUF download
            let app_progress = app.clone();
            manager
                .set_gguf_progress_callback(Box::new(move |evt| {
                    let _ = app_progress.emit(agent_events::MODEL_DOWNLOAD_PROGRESS, &evt);
                }))
                .await;

            manager
                .download(&model_id)
                .await
                .map_err(|e| agent_error(format!("Download failed: {e}")))?;

            tracing::info!("Model '{}' downloaded successfully", model_id);
        }
        ModelStatus::Ready => {
            // Already on disk, just need to load
        }
    }

    // --- Load the model into the inference engine ---
    let _ = app.emit(
        agent_events::MODEL_STATUS,
        &ModelStatusEvent {
            model_id: model_id.clone(),
            status: "loading".to_string(),
            message: Some(format!("Loading {}...", model_id)),
        },
    );

    // Get the model file path (GGUF path — Ollama models are handled above)
    let model_path = manager
        .gguf_manager()
        .model_path(&model_id)
        .map_err(|e| agent_error(format!("Failed to resolve model path: {e}")))?;

    let model_path_str = model_path.to_string_lossy().to_string();

    // Mark as loaded in the model manager
    manager
        .load(&model_id)
        .await
        .map_err(|e| agent_error(format!("Failed to mark model as loaded: {e}")))?;

    // Create the real inference engine (blocking: loads GGUF + compiles Metal kernels)
    let engine = tokio::task::spawn_blocking(move || {
        use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
        LlamaChatInferenceEngine::load(&model_path_str, ChatConfig::default())
    })
    .await
    .map_err(|e| agent_error(format!("Task join error: {e}")))?
    .map_err(|e| agent_error(format!("Failed to load inference engine: {e}")))?;

    // Swap the engine into the agent state
    agent_state.replace_engine(Arc::new(engine)).await;

    let _ = app.emit(
        agent_events::MODEL_STATUS,
        &ModelStatusEvent {
            model_id: model_id.clone(),
            status: "ready".to_string(),
            message: Some(format!("{} ready", model_id)),
        },
    );

    tracing::info!("Model '{}' loaded and inference engine ready", model_id);
    Ok(true) // engine installed, sessions dropped
}

/// Get the current status of the local agent.
#[tauri::command]
pub async fn local_agent_status(
    state: State<'_, ManagedAgentState>,
) -> Result<LocalAgentStatus, CommandError> {
    let service = state.service().await;
    let sessions = service.get_sessions().await;
    if sessions.is_empty() {
        return Ok(LocalAgentStatus::Idle);
    }
    // Return last session's status
    Ok(sessions
        .last()
        .map(|(_, s)| s.clone())
        .unwrap_or(LocalAgentStatus::Idle))
}

/// Create a new local agent conversation session.
///
/// Returns the session ID. Populates the session's dynamic context with
/// the current workspace schemas, collections, and active playbooks.
#[tauri::command]
pub async fn local_agent_new_session(
    model_id: String,
    state: State<'_, ManagedAgentState>,
) -> Result<String, CommandError> {
    let service = state.service().await;
    let session_id = service.create_session(Some(model_id)).await;

    // Build workspace context for the local agent prompt
    if let Ok(ns) = state.app_services.node_service().await {
        let context = nodespace_core::ops::context_ops::build_workspace_context(&ns)
            .await
            .unwrap_or_default();
        let context_str = context.format_for_prompt(1500);
        service.set_session_context(&session_id, context_str).await;
    }

    tracing::info!(session_id = %session_id, "Local agent session created");
    Ok(session_id)
}

/// Send a user message to a local agent session.
///
/// Before inference, refreshes the workspace context (entity types, collections,
/// playbooks) to ensure the AI knows about any schemas added mid-session.
/// This enables dynamic schema discovery without restarting the session.
///
/// Streams [`StreamingChunk`] events on the `local-agent://chunk` channel,
/// [`LocalAgentStatus`] updates on `local-agent://status`, and
/// tool events on `local-agent://tool`.
///
/// Returns the final [`AgentTurnResult`] when the turn completes.
#[tauri::command]
pub async fn local_agent_send(
    session_id: String,
    message: String,
    app: AppHandle,
    state: State<'_, ManagedAgentState>,
) -> Result<AgentTurnResult, CommandError> {
    // Refresh workspace context per turn to capture any newly-added schemas
    if let Ok(ns) = state.app_services.node_service().await {
        let context = nodespace_core::ops::context_ops::build_workspace_context(&ns)
            .await
            .unwrap_or_default();
        let context_str = context.format_for_prompt(1500);
        let service = state.service().await;
        service.set_session_context(&session_id, context_str).await;
    }

    let app_status = app.clone();
    let app_chunk = app.clone();
    let app_tool = app.clone();

    let on_status = move |status: LocalAgentStatus| {
        let _ = app_status.emit(agent_events::LOCAL_AGENT_STATUS, &status);
    };

    let on_chunk = move |chunk: StreamingChunk| {
        let _ = app_chunk.emit(agent_events::LOCAL_AGENT_CHUNK, &chunk);
        // Forward tool call starts as dedicated tool events
        if let StreamingChunk::ToolCallStart { ref id, ref name } = chunk {
            #[derive(Serialize)]
            struct ToolEvent {
                id: String,
                name: String,
            }
            let _ = app_tool.emit(
                agent_events::LOCAL_AGENT_TOOL,
                &ToolEvent {
                    id: id.clone(),
                    name: name.clone(),
                },
            );
        }
    };

    let service = state.service().await;
    service
        .send_message(&session_id, &message, on_status, on_chunk)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            let _ = app.emit(agent_events::LOCAL_AGENT_ERROR, &msg);
            agent_error(msg)
        })
}

/// Cancel an in-progress generation for the given session.
#[tauri::command]
pub async fn local_agent_cancel(
    session_id: String,
    state: State<'_, ManagedAgentState>,
) -> Result<(), CommandError> {
    let service = state.service().await;
    service.cancel(&session_id).await;
    tracing::info!(session_id = %session_id, "Local agent generation cancelled");
    Ok(())
}

/// End and remove a session, freeing all resources.
#[tauri::command]
pub async fn local_agent_end_session(
    session_id: String,
    state: State<'_, ManagedAgentState>,
) -> Result<(), CommandError> {
    let service = state.service().await;
    service.end_session(&session_id).await;
    tracing::info!(session_id = %session_id, "Local agent session ended");
    Ok(())
}

/// Get all active agent sessions.
#[tauri::command]
pub async fn local_agent_get_sessions(
    state: State<'_, ManagedAgentState>,
) -> Result<Vec<AgentSession>, CommandError> {
    let service = state.service().await;
    let session_pairs = service.get_sessions().await;
    let mut sessions = Vec::with_capacity(session_pairs.len());
    for (id, _) in &session_pairs {
        if let Some(session) = service.get_session(id).await {
            sessions.push(session);
        }
    }
    Ok(sessions)
}
