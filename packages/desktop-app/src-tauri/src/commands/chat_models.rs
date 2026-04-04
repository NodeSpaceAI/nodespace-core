//! Tauri commands for GGUF chat model management.
//!
//! Bridges the Svelte frontend to [`GgufModelManager`] via Tauri IPC.
//! Download progress is forwarded through the `model://download-progress`
//! Tauri event channel.
//!
//! Issue #1008

use crate::agent_types::{events, DownloadEvent, ModelInfo, ModelManager};
use crate::commands::nodes::CommandError;
use crate::local_agent::model_manager::GgufModelManager;
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

/// List all models in the local catalog with their current status.
#[tauri::command]
pub async fn chat_model_list(
    manager: State<'_, Arc<GgufModelManager>>,
) -> Result<Vec<ModelInfo>, CommandError> {
    manager
        .list()
        .await
        .map_err(|e| model_error(format!("Failed to list models: {e}")))
}

/// Get the recommended model based on system RAM.
#[tauri::command]
pub async fn chat_model_recommended(
    manager: State<'_, Arc<GgufModelManager>>,
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
    manager: State<'_, Arc<GgufModelManager>>,
) -> Result<(), CommandError> {
    // Register progress callback that emits Tauri events
    let app_progress = app.clone();
    manager
        .set_progress_callback(Box::new(move |evt: DownloadEvent| {
            let _ = app_progress.emit(events::MODEL_DOWNLOAD_PROGRESS, &evt);
        }))
        .await;

    manager.download(&model_id).await.map_err(|e| {
        model_error(format!("Download failed for {model_id}: {e}"))
    })
}

/// Cancel an in-progress model download.
#[tauri::command]
pub async fn chat_model_cancel_download(
    model_id: String,
    manager: State<'_, Arc<GgufModelManager>>,
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
    manager: State<'_, Arc<GgufModelManager>>,
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
    manager: State<'_, Arc<GgufModelManager>>,
) -> Result<(), CommandError> {
    manager
        .load(&model_id)
        .await
        .map_err(|e| model_error(format!("Failed to load model {model_id}: {e}")))
}

/// Unload the currently loaded model, freeing resources.
#[tauri::command]
pub async fn chat_model_unload(
    manager: State<'_, Arc<GgufModelManager>>,
) -> Result<(), CommandError> {
    manager
        .unload()
        .await
        .map_err(|e| model_error(format!("Failed to unload model: {e}")))
}
