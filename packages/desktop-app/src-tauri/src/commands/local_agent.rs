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

use crate::agent_types::{
    events, AgentSession, AgentToolExecutor, AgentTurnResult, ChatInferenceEngine, ChatModelSpec,
    InferenceError, InferenceUsage, LocalAgentStatus, ModelManager, ModelStatus, StreamingChunk,
};
use crate::commands::nodes::CommandError;
use crate::local_agent::agent_loop::LocalAgentService;
use crate::local_agent::model_manager::GgufModelManager;
use async_trait::async_trait;
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
        _request: crate::agent_types::InferenceRequest,
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
}

impl ManagedAgentState {
    /// Create with a no-op inference engine.
    ///
    /// The `app_services` parameter is used to construct the tool executor
    /// so it can access NodeService and NodeEmbeddingService per-operation.
    pub fn new(app_services: crate::app_services::AppServices) -> Self {
        use crate::local_agent::tools::GraphToolExecutor;

        let engine: Arc<dyn ChatInferenceEngine> = Arc::new(NoOpInferenceEngine);
        let executor: Arc<dyn AgentToolExecutor> =
            Arc::new(GraphToolExecutor::new(app_services.clone()));
        let service = LocalAgentService::new(engine, executor);

        Self {
            inner: RwLock::new(service),
            app_services,
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
    /// Creates a fresh `LocalAgentService` with the new engine and the
    /// existing tool executor. Existing sessions are dropped.
    pub async fn replace_engine(&self, engine: Arc<dyn ChatInferenceEngine>) {
        use crate::local_agent::tools::GraphToolExecutor;

        let executor: Arc<dyn AgentToolExecutor> =
            Arc::new(GraphToolExecutor::new(self.app_services.clone()));
        let service = LocalAgentService::new(engine, executor);

        let mut guard = self.inner.write().await;
        *guard = service;

        tracing::info!("ManagedAgentState: inference engine replaced");
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
pub async fn ensure_model_ready(
    model_id: String,
    app: AppHandle,
    manager: State<'_, Arc<GgufModelManager>>,
    agent_state: State<'_, ManagedAgentState>,
) -> Result<(), CommandError> {
    // Check current model status
    let models = manager
        .list()
        .await
        .map_err(|e| agent_error(e.to_string()))?;
    let model = models
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| agent_error(format!("Unknown model: {model_id}")))?;

    match &model.status {
        ModelStatus::Loaded => {
            tracing::info!("Model '{}' already loaded", model_id);
            return Ok(());
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
                events::MODEL_STATUS,
                &ModelStatusEvent {
                    model_id: model_id.clone(),
                    status: "downloading".to_string(),
                    message: Some(format!("Downloading {}...", model_id)),
                },
            );

            // Register progress callback
            let app_progress = app.clone();
            manager
                .set_progress_callback(Box::new(move |evt| {
                    let _ = app_progress.emit(events::MODEL_DOWNLOAD_PROGRESS, &evt);
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
        events::MODEL_STATUS,
        &ModelStatusEvent {
            model_id: model_id.clone(),
            status: "loading".to_string(),
            message: Some(format!("Loading {}...", model_id)),
        },
    );

    // Get the model file path
    let model_path = manager
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
        use crate::local_agent::inference::LlamaChatInferenceEngine;
        LlamaChatInferenceEngine::load(&model_path_str, ChatConfig::default())
    })
    .await
    .map_err(|e| agent_error(format!("Task join error: {e}")))?
    .map_err(|e| agent_error(format!("Failed to load inference engine: {e}")))?;

    // Swap the engine into the agent state
    agent_state.replace_engine(Arc::new(engine)).await;

    let _ = app.emit(
        events::MODEL_STATUS,
        &ModelStatusEvent {
            model_id: model_id.clone(),
            status: "ready".to_string(),
            message: Some(format!("{} ready", model_id)),
        },
    );

    tracing::info!("Model '{}' loaded and inference engine ready", model_id);
    Ok(())
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
/// Returns the session ID.
#[tauri::command]
pub async fn local_agent_new_session(
    model_id: String,
    state: State<'_, ManagedAgentState>,
) -> Result<String, CommandError> {
    let service = state.service().await;
    let session_id = service.create_session(Some(model_id)).await;
    tracing::info!(session_id = %session_id, "Local agent session created");
    Ok(session_id)
}

/// Send a user message to a local agent session.
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
    let app_status = app.clone();
    let app_chunk = app.clone();
    let app_tool = app.clone();

    let on_status = move |status: LocalAgentStatus| {
        let _ = app_status.emit(events::LOCAL_AGENT_STATUS, &status);
    };

    let on_chunk = move |chunk: StreamingChunk| {
        let _ = app_chunk.emit(events::LOCAL_AGENT_CHUNK, &chunk);
        // Forward tool call starts as dedicated tool events
        if let StreamingChunk::ToolCallStart { ref id, ref name } = chunk {
            #[derive(Serialize)]
            struct ToolEvent {
                id: String,
                name: String,
            }
            let _ = app_tool.emit(
                events::LOCAL_AGENT_TOOL,
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
            let _ = app.emit(events::LOCAL_AGENT_ERROR, &msg);
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
