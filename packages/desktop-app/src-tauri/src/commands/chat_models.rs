//! Tauri commands for chat model management.
//!
//! Bridges the Svelte frontend to [`CompositeModelManager`] via Tauri IPC.
//! Download progress is forwarded through the `model://download-progress`
//! Tauri event channel. When Ollama is running, both built-in GGUF models
//! and `ollama:`-prefixed Ollama models appear in the unified model list.
//!
//! Issue #1058

use crate::agent_events;
use crate::commands::nodes::CommandError;
use nodespace_agent::agent_types::{DownloadEvent, ModelInfo, ModelManager};
use nodespace_agent::local_agent::composite_model_manager::CompositeModelManager;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

/// Helper to map model errors into [`CommandError`].
fn model_error(message: impl Into<String>) -> CommandError {
    CommandError {
        message: message.into(),
        code: "MODEL_ERROR".to_string(),
        details: None,
    }
}

/// List all models in the catalog with their current status.
///
/// Returns built-in GGUF models plus `ollama:`-prefixed models when the
/// Ollama daemon is running.
#[tauri::command]
pub async fn chat_model_list(
    manager: State<'_, Arc<CompositeModelManager>>,
) -> Result<Vec<ModelInfo>, CommandError> {
    manager
        .list()
        .await
        .map_err(|e| model_error(format!("Failed to list models: {e}")))
}

/// Get the recommended model based on system RAM.
#[tauri::command]
pub async fn chat_model_recommended(
    manager: State<'_, Arc<CompositeModelManager>>,
) -> Result<String, CommandError> {
    manager
        .recommended_model()
        .await
        .map_err(|e| model_error(format!("Failed to get recommended model: {e}")))
}

/// Download a model. Progress events are emitted on `model://download-progress`.
///
/// This command spawns the download in a background task so the frontend
/// is not blocked. Progress is delivered via Tauri events.
#[tauri::command]
pub async fn chat_model_download(
    model_id: String,
    app: AppHandle,
    manager: State<'_, Arc<CompositeModelManager>>,
) -> Result<(), CommandError> {
    // Register progress callbacks for both backends
    let app_gguf = app.clone();
    manager
        .set_gguf_progress_callback(Box::new(move |evt: DownloadEvent| {
            let _ = app_gguf.emit(agent_events::MODEL_DOWNLOAD_PROGRESS, &evt);
        }))
        .await;

    let app_ollama = app.clone();
    manager
        .set_ollama_progress_callback(Box::new(move |evt: DownloadEvent| {
            let _ = app_ollama.emit(agent_events::MODEL_DOWNLOAD_PROGRESS, &evt);
        }))
        .await;

    manager
        .download(&model_id)
        .await
        .map_err(|e| model_error(format!("Download failed for {model_id}: {e}")))
}

/// Cancel an in-progress model download.
#[tauri::command]
pub async fn chat_model_cancel_download(
    model_id: String,
    manager: State<'_, Arc<CompositeModelManager>>,
) -> Result<(), CommandError> {
    manager
        .cancel_download(&model_id)
        .await
        .map_err(|e| model_error(format!("Failed to cancel download: {e}")))
}

/// Delete a downloaded model from disk.
#[tauri::command]
pub async fn chat_model_delete(
    model_id: String,
    manager: State<'_, Arc<CompositeModelManager>>,
) -> Result<(), CommandError> {
    manager
        .delete(&model_id)
        .await
        .map_err(|e| model_error(format!("Failed to delete model {model_id}: {e}")))
}

/// Load a downloaded model into memory for inference.
#[tauri::command]
pub async fn chat_model_load(
    model_id: String,
    manager: State<'_, Arc<CompositeModelManager>>,
) -> Result<(), CommandError> {
    manager
        .load(&model_id)
        .await
        .map_err(|e| model_error(format!("Failed to load model {model_id}: {e}")))
}

/// Unload the currently loaded model, freeing resources.
///
/// Also clears the agent's active_model_id so that reloading the same model
/// afterwards correctly re-installs the inference engine.
#[tauri::command]
pub async fn chat_model_unload(
    manager: State<'_, Arc<CompositeModelManager>>,
    agent_state: State<'_, std::sync::Arc<crate::commands::local_agent::ManagedAgentState>>,
) -> Result<(), CommandError> {
    manager
        .unload()
        .await
        .map_err(|e| model_error(format!("Failed to unload model: {e}")))?;

    agent_state.clear_active_model().await;
    Ok(())
}

/// Check whether the Ollama daemon is running and reachable.
#[tauri::command]
pub async fn ollama_available(
    manager: State<'_, Arc<CompositeModelManager>>,
) -> Result<bool, CommandError> {
    Ok(manager.ollama_available().await)
}
