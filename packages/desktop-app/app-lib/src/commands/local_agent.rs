//! Tauri commands for the local agent — model management and token stream subscription.
//!
//! Session IPC (StartSession, SendMessage, EndSession) has been removed.
//! The daemon now drives inference in response to ai-chat node changes.
//! The Tauri process subscribes once to `SubscribeTokenStream` and forwards
//! token events to the frontend via Tauri events.

use crate::agent_events;
use crate::commands::nodes::CommandError;
use crate::services::GrpcClient;
use nodespace_proto::nodespace::{
    CancelTurnRequest, EnsureModelReadyRequest, GetLocalStatusRequest, ListModelsRequest,
    SubscribeTokenStreamRequest,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio_stream::StreamExt;

fn grpc_err(msg: impl std::fmt::Display) -> CommandError {
    CommandError {
        message: msg.to_string(),
        code: "GRPC_ERROR".to_string(),
        details: None,
        conflict_data: None,
    }
}

// ---------------------------------------------------------------------------
// Token stream subscription (called once from app setup)
// ---------------------------------------------------------------------------

/// Open a long-lived gRPC subscription to token events from the daemon.
///
/// This runs as a background task. All inference token events (for all ai-chat
/// nodes) are forwarded to the frontend as Tauri events on `local-agent://chunk`.
/// The `node_id` field on each chunk tells the frontend which node is streaming.
pub fn start_token_stream_subscription(app: AppHandle, grpc: GrpcClient) {
    tokio::spawn(async move {
        loop {
            match try_subscribe(&app, &grpc).await {
                Ok(()) => {
                    tracing::info!("Token stream subscription ended; reconnecting in 2s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Token stream subscription failed; reconnecting in 2s");
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            }
        }
    });
}

async fn try_subscribe(app: &AppHandle, grpc: &GrpcClient) -> Result<(), String> {
    let mut client = grpc.local_agent_client().await;
    // Interrupt a wedged stream when the shared channel is rebuilt after a
    // recovery (GrpcClient::reconnect bumps this) or the active database
    // switches — mirrors the node watcher so the outer loop re-subscribes on the
    // fresh channel instead of hanging forever on the dead one.
    let mut db_changed = grpc.subscribe_active_database();
    let mut stream = client
        .subscribe_token_stream(SubscribeTokenStreamRequest {})
        .await
        .map_err(|e| e.message().to_string())?
        .into_inner();

    loop {
        let chunk_result = tokio::select! {
            biased;
            _ = db_changed.changed() => return Ok(()),
            item = stream.next() => match item {
                Some(item) => item,
                None => return Ok(()),
            },
        };
        let chunk = chunk_result.map_err(|e| e.message().to_string())?;

        match chunk.chunk_type.as_str() {
            "token" => {
                if let Some(text) = chunk.token_text {
                    #[derive(Serialize)]
                    struct TokenChunk<'a> {
                        #[serde(rename = "type")]
                        chunk_type: &'a str,
                        text: String,
                        node_id: Option<String>,
                    }
                    let _ = app.emit(
                        agent_events::LOCAL_AGENT_CHUNK,
                        &TokenChunk {
                            chunk_type: "token",
                            text,
                            node_id: chunk.node_id,
                        },
                    );
                }
            }
            "tool_call_start" => {
                if let (Some(id), Some(name)) = (chunk.tool_call_id, chunk.tool_name) {
                    #[derive(Serialize)]
                    struct ToolEvent {
                        id: String,
                        name: String,
                        node_id: Option<String>,
                    }
                    let _ = app.emit(
                        agent_events::LOCAL_AGENT_TOOL,
                        &ToolEvent {
                            id: id.clone(),
                            name: name.clone(),
                            node_id: chunk.node_id.clone(),
                        },
                    );
                    #[derive(Serialize)]
                    struct ToolStartChunk<'a> {
                        #[serde(rename = "type")]
                        chunk_type: &'a str,
                        id: String,
                        name: String,
                        node_id: Option<String>,
                    }
                    let _ = app.emit(
                        agent_events::LOCAL_AGENT_CHUNK,
                        &ToolStartChunk {
                            chunk_type: "tool_call_start",
                            id,
                            name,
                            node_id: chunk.node_id,
                        },
                    );
                }
            }
            "tool_call_args" => {
                if let (Some(id), Some(args_json)) = (chunk.tool_call_id, chunk.tool_args_json) {
                    #[derive(Serialize)]
                    struct ToolArgsChunk<'a> {
                        #[serde(rename = "type")]
                        chunk_type: &'a str,
                        id: String,
                        args_json: String,
                        node_id: Option<String>,
                    }
                    let _ = app.emit(
                        agent_events::LOCAL_AGENT_CHUNK,
                        &ToolArgsChunk {
                            chunk_type: "tool_call_args",
                            id,
                            args_json,
                            node_id: chunk.node_id,
                        },
                    );
                }
            }
            "done" => {
                #[derive(Serialize)]
                struct DoneChunk<'a> {
                    #[serde(rename = "type")]
                    chunk_type: &'a str,
                    prompt_tokens: i32,
                    completion_tokens: i32,
                    node_id: Option<String>,
                }
                let _ = app.emit(
                    agent_events::LOCAL_AGENT_CHUNK,
                    &DoneChunk {
                        chunk_type: "done",
                        prompt_tokens: chunk.prompt_tokens.unwrap_or(0),
                        completion_tokens: chunk.completion_tokens.unwrap_or(0),
                        node_id: chunk.node_id,
                    },
                );
            }
            "cancelled" => {
                #[derive(Serialize)]
                struct CancelledChunk<'a> {
                    #[serde(rename = "type")]
                    chunk_type: &'a str,
                    node_id: Option<String>,
                }
                let _ = app.emit(
                    agent_events::LOCAL_AGENT_CHUNK,
                    &CancelledChunk {
                        chunk_type: "cancelled",
                        node_id: chunk.node_id,
                    },
                );
            }
            "error" => {
                let msg = chunk
                    .error_message
                    .unwrap_or_else(|| "Unknown error".to_string());
                let _ = app.emit(agent_events::LOCAL_AGENT_ERROR, &msg);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Turn management
// ---------------------------------------------------------------------------

/// Cancel an in-progress inference turn for a given ai-chat node.
#[tauri::command]
pub async fn local_agent_cancel_turn(
    node_id: String,
    grpc: State<'_, GrpcClient>,
) -> Result<(), CommandError> {
    let mut client = grpc.local_agent_client().await;
    client
        .cancel_turn(CancelTurnRequest { node_id })
        .await
        .map_err(|e| grpc_err(e.message()))?;
    Ok(())
}

/// Get the current status of the local agent.
#[tauri::command]
pub async fn local_agent_status(
    grpc: State<'_, GrpcClient>,
) -> Result<crate::types::LocalAgentStatus, CommandError> {
    let mut client = grpc.local_agent_client().await;
    let resp = client
        .get_status(GetLocalStatusRequest { session_id: None })
        .await
        .map_err(|e| grpc_err(e.message()))?;
    serde_json::from_str(&resp.into_inner().status_json)
        .map_err(|e| grpc_err(format!("Failed to deserialize status: {e}")))
}

// ---------------------------------------------------------------------------
// Model loading
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct ModelStatusEvent {
    model_id: String,
    status: String,
    message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DownloadProgressEvent {
    model_id: String,
    bytes_downloaded: i64,
    bytes_total: i64,
}

/// Ensure a model is downloaded, loaded, and the inference engine is ready.
#[tauri::command]
pub async fn ensure_model_ready(
    model_id: String,
    app: AppHandle,
    grpc: State<'_, GrpcClient>,
) -> Result<bool, CommandError> {
    let mut client = grpc.local_agent_client().await;
    let mut stream = client
        .ensure_model_ready(EnsureModelReadyRequest {
            model_id: model_id.clone(),
        })
        .await
        .map_err(|e| grpc_err(e.message()))?
        .into_inner();

    let mut engine_swapped = false;
    let mut saw_terminal_event = false;

    while let Some(event_result) = stream.next().await {
        let event = event_result.map_err(|e| grpc_err(e.message()))?;

        match event.event_type.as_str() {
            "downloading" => {
                let _ = app.emit(
                    agent_events::MODEL_STATUS,
                    &ModelStatusEvent {
                        model_id: event.model_id.clone(),
                        status: "downloading".to_string(),
                        message: event.message.clone(),
                    },
                );
                if let (Some(dl), Some(tot)) = (event.bytes_downloaded, event.bytes_total) {
                    let _ = app.emit(
                        agent_events::MODEL_DOWNLOAD_PROGRESS,
                        &DownloadProgressEvent {
                            model_id: event.model_id,
                            bytes_downloaded: dl,
                            bytes_total: tot,
                        },
                    );
                }
            }
            "verifying" => {
                let _ = app.emit(
                    agent_events::MODEL_STATUS,
                    &ModelStatusEvent {
                        model_id: event.model_id,
                        status: "verifying".to_string(),
                        message: event.message,
                    },
                );
            }
            "loading" => {
                let _ = app.emit(
                    agent_events::MODEL_STATUS,
                    &ModelStatusEvent {
                        model_id: event.model_id,
                        status: "loading".to_string(),
                        message: event.message,
                    },
                );
            }
            "ready" => {
                engine_swapped = event.engine_swapped.unwrap_or(false);
                saw_terminal_event = true;
                let _ = app.emit(
                    agent_events::MODEL_STATUS,
                    &ModelStatusEvent {
                        model_id: event.model_id,
                        status: "ready".to_string(),
                        message: event.message,
                    },
                );
            }
            "error" => {
                let msg = event
                    .error_message
                    .unwrap_or_else(|| "Unknown error".to_string());
                return Err(grpc_err(msg));
            }
            _ => {}
        }
    }

    // The daemon always ends a well-formed stream with exactly one of
    // "ready" or "error" (the latter returns above). A stream that closes
    // without either — e.g. the daemon-side load task panicking and
    // dropping its sender — must not be reported as success: the caller
    // otherwise cannot distinguish "model loaded" from "the daemon crashed
    // mid-load and never told us."
    if !saw_terminal_event {
        return Err(grpc_err(
            "Model load stream ended without a ready or error event".to_string(),
        ));
    }

    Ok(engine_swapped)
}

/// List all models available in the local catalog.
#[tauri::command]
pub async fn list_local_models(
    grpc: State<'_, GrpcClient>,
) -> Result<Vec<serde_json::Value>, CommandError> {
    let mut client = grpc.local_agent_client().await;
    let resp = client
        .list_models(ListModelsRequest {
            force_refresh: false,
        })
        .await
        .map_err(|e| grpc_err(e.message()))?
        .into_inner();

    let models = resp
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
