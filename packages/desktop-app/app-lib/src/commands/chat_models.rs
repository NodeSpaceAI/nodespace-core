//! Tauri commands for chat model management.
//!
//! Thin gRPC proxy to `LocalAgentService` model management RPCs in the daemon.
//! Download progress is forwarded through the `model://download-progress`
//! Tauri event channel.

use crate::agent_events;
use crate::commands::nodes::{refusal_or, CommandError};
use crate::services::GrpcClient;
use nodespace_proto::nodespace::{
    CancelModelDownloadRequest, DeleteModelRequest, DownloadModelRequest, GetSystemRamRequest,
    ListModelsRequest, LoadModelRequest, RecommendedModelRequest, UnloadModelRequest,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio_stream::StreamExt;

/// Helper to map model errors into [`CommandError`].
fn model_error(message: impl Into<String>) -> CommandError {
    CommandError {
        message: message.into(),
        code: "MODEL_ERROR".to_string(),
        details: None,
        conflict_data: None,
        requires_extension: None,
    }
}

fn grpc_err(msg: impl std::fmt::Display) -> CommandError {
    CommandError {
        message: msg.to_string(),
        code: "GRPC_ERROR".to_string(),
        details: None,
        conflict_data: None,
        requires_extension: None,
    }
}

/// List models in the catalog with their current status.
///
/// Returns every GGUF catalog model plus every model discovered at a
/// configured OpenAI-compatible endpoint.
///
/// OpenAI-compat discovery is cached daemon-side for a short TTL (see
/// `OPENAI_COMPAT_DISCOVERY_CACHE_TTL` in `local_agent_service.rs`) so the
/// three call sites that list models on mount don't each re-query every
/// endpoint. Pass `force_refresh: true` to bypass a fresh cache hit — used by
/// the explicit "Refresh remote models" action in Settings.
#[tauri::command]
pub async fn chat_model_list(
    grpc: State<'_, GrpcClient>,
    force_refresh: Option<bool>,
) -> Result<Vec<serde_json::Value>, CommandError> {
    let mut client = grpc.local_agent_client().await;

    let resp = client
        .list_models(ListModelsRequest {
            force_refresh: force_refresh.unwrap_or(false),
        })
        .await
        .map_err(|e| refusal_or(e, |e| grpc_err(e.message())))?;

    let models = resp
        .into_inner()
        .models
        .into_iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.id,
                "name": entry.name,
                "backend": entry.backend,
                "status": serde_json::from_str::<serde_json::Value>(&entry.status_json)
                    .unwrap_or(serde_json::Value::Null),
                "sizeBytes": entry.size_bytes,
                "quantization": entry.quantization,
                "minMemoryGb": entry.min_memory_gb,
            })
        })
        .collect();

    Ok(models)
}

/// Get the recommended model based on system RAM.
#[tauri::command]
pub async fn chat_model_recommended(grpc: State<'_, GrpcClient>) -> Result<String, CommandError> {
    let mut client = grpc.local_agent_client().await;
    let resp = client
        .recommended_model(RecommendedModelRequest {})
        .await
        .map_err(|e| model_error(format!("Failed to get recommended model: {e}")))?;
    Ok(resp.into_inner().model_id)
}

/// Download a model. Progress events are emitted on `model://download-progress`.
///
/// Streams `ModelLoadProgressEvent` from the daemon, forwarding progress to
/// the frontend via Tauri events.
#[tauri::command]
pub async fn chat_model_download(
    model_id: String,
    app: AppHandle,
    grpc: State<'_, GrpcClient>,
) -> Result<(), CommandError> {
    let mut client = grpc.local_agent_client().await;
    let mut stream = client
        .download_model(DownloadModelRequest {
            model_id: model_id.clone(),
        })
        .await
        .map_err(|e| model_error(format!("Download failed for {model_id}: {e}")))?
        .into_inner();

    while let Some(event_result) = stream.next().await {
        let event = event_result.map_err(|e| model_error(e.message().to_string()))?;
        match event.event_type.as_str() {
            "downloading" => {
                #[derive(Serialize)]
                struct ProgressEvent {
                    model_id: String,
                    bytes_downloaded: i64,
                    bytes_total: i64,
                }
                let _ = app.emit(
                    agent_events::MODEL_DOWNLOAD_PROGRESS,
                    &ProgressEvent {
                        model_id: event.model_id,
                        bytes_downloaded: event.bytes_downloaded.unwrap_or(0),
                        bytes_total: event.bytes_total.unwrap_or(0),
                    },
                );
            }
            "ready" => {
                #[derive(Serialize)]
                struct ReadyEvent {
                    model_id: String,
                }
                let _ = app.emit(
                    agent_events::MODEL_DOWNLOAD_READY,
                    &ReadyEvent {
                        model_id: event.model_id,
                    },
                );
            }
            "error" => {
                let msg = event
                    .error_message
                    .unwrap_or_else(|| "Unknown download error".to_string());
                return Err(model_error(msg));
            }
            _ => {}
        }
    }

    Ok(())
}

/// Cancel an in-progress model download.
#[tauri::command]
pub async fn chat_model_cancel_download(
    model_id: String,
    grpc: State<'_, GrpcClient>,
) -> Result<(), CommandError> {
    let mut client = grpc.local_agent_client().await;
    client
        .cancel_model_download(CancelModelDownloadRequest { model_id })
        .await
        .map_err(|e| model_error(format!("Failed to cancel download: {e}")))?;
    Ok(())
}

/// Delete a downloaded model from disk.
#[tauri::command]
pub async fn chat_model_delete(
    model_id: String,
    grpc: State<'_, GrpcClient>,
) -> Result<(), CommandError> {
    let mut client = grpc.local_agent_client().await;
    client
        .delete_model(DeleteModelRequest { model_id })
        .await
        .map_err(|e| model_error(format!("Failed to delete model: {e}")))?;
    Ok(())
}

/// Load a downloaded model into memory for inference.
#[tauri::command]
pub async fn chat_model_load(
    model_id: String,
    grpc: State<'_, GrpcClient>,
) -> Result<(), CommandError> {
    let mut client = grpc.local_agent_client().await;
    client
        .load_model(LoadModelRequest { model_id })
        .await
        .map_err(|e| model_error(format!("Failed to load model: {e}")))?;
    Ok(())
}

/// Unload the currently loaded model, freeing resources.
#[tauri::command]
pub async fn chat_model_unload(grpc: State<'_, GrpcClient>) -> Result<(), CommandError> {
    let mut client = grpc.local_agent_client().await;
    client
        .unload_model(UnloadModelRequest {})
        .await
        .map_err(|e| model_error(format!("Failed to unload model: {e}")))?;
    Ok(())
}

/// Return total system RAM in GiB (rounded down).
///
/// Used by the model manager UI to dim cards for models that exceed the
/// machine's available RAM.
#[tauri::command]
pub async fn get_system_ram_gb(grpc: State<'_, GrpcClient>) -> Result<u64, CommandError> {
    let mut client = grpc.local_agent_client().await;
    let resp = client
        .get_system_ram(GetSystemRamRequest {})
        .await
        .map_err(|e| model_error(format!("Failed to get system RAM: {e}")))?;
    Ok(resp.into_inner().ram_bytes / (1024 * 1024 * 1024))
}
