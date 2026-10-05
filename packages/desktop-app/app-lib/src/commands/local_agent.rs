//! Tauri commands for the local agent — model management and token stream subscription.
//!
//! Session IPC (StartSession, SendMessage, EndSession) has been removed.
//! The daemon now drives inference in response to ai-chat node changes.
//! The Tauri process subscribes once to `SubscribeTokenStream` and forwards
//! token events to the frontend via Tauri events.

use crate::agent_events;
use crate::commands::nodes::{refusal_or, CommandError};
use crate::services::GrpcClient;
use nodespace_proto::nodespace::{
    CancelTurnRequest, CreatePlayEditChatRequest, EnsureModelReadyRequest, GetLocalStatusRequest,
    ListModelsRequest, SubscribeTokenStreamRequest,
};
use serde::Serialize;
use std::future::Future;
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::watch;
use tokio_stream::StreamExt;

fn grpc_err(msg: impl std::fmt::Display) -> CommandError {
    CommandError {
        message: msg.to_string(),
        code: "GRPC_ERROR".to_string(),
        details: None,
        conflict_data: None,
        requires_extension: None,
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
        resubscribe_forever(
            || grpc.subscribe_active_database(),
            |db_changed| try_subscribe(&app, &grpc, db_changed),
        )
        .await;
    });
}

/// Run `attempt` forever, waiting between attempts as `wait_to_resubscribe`
/// decides.
///
/// `active_database` returns a receiver of the active-database generation
/// (`GrpcClient::subscribe_active_database`), which bumps when the active
/// database switches or the shared channel is rebuilt after a recovery. One is
/// taken before each attempt, so a change during the attempt is not missed.
/// The attempt gets a clone, to interrupt a wedged stream with.
async fn resubscribe_forever<S, A, F>(mut active_database: S, mut attempt: A)
where
    S: FnMut() -> watch::Receiver<u64>,
    A: FnMut(watch::Receiver<u64>) -> F,
    F: Future<Output = Result<(), tonic::Status>>,
{
    loop {
        let mut db_changed = active_database();
        let outcome = attempt(db_changed.clone()).await;
        match &outcome {
            Ok(()) => tracing::info!("Token stream subscription ended; reconnecting in 2s"),
            Err(e) if is_database_refusal(e) => tracing::info!(
                "Token stream refused: the active database requires an extension this build \
                 does not support; resubscribing when the active database changes"
            ),
            Err(e) => {
                tracing::warn!(
                    code = ?e.code(),
                    error = %e.message(),
                    "Token stream subscription failed; reconnecting in 2s"
                )
            }
        }
        wait_to_resubscribe(&outcome, &mut db_changed, RESUBSCRIBE_DELAY).await;
    }
}

/// The pause before the token stream subscribes again after it ends or fails.
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(2);

/// Whether `status` is the daemon's refusal of the active database because it
/// requires an extension this build does not support (ADR-083 §2).
fn is_database_refusal(status: &tonic::Status) -> bool {
    nodespace_proto::requires_extension::unsupported_extensions(status).is_some()
}

/// Wait until the token stream should subscribe again after `outcome`.
///
/// Asking a refused database again cannot change the answer, so after a
/// refusal this waits for `db_changed` (`GrpcClient::subscribe_active_database`,
/// taken before the attempt): a switch to another database, or a rebuilt
/// channel, and then resubscribes at once. Every other outcome waits `delay`.
/// If the client is gone, the refusal falls back to `delay` rather than spin.
async fn wait_to_resubscribe(
    outcome: &Result<(), tonic::Status>,
    db_changed: &mut watch::Receiver<u64>,
    delay: Duration,
) {
    if let Err(status) = outcome {
        if is_database_refusal(status) && db_changed.changed().await.is_ok() {
            return;
        }
    }
    tokio::time::sleep(delay).await;
}

async fn try_subscribe(
    app: &AppHandle,
    grpc: &GrpcClient,
    mut db_changed: watch::Receiver<u64>,
) -> Result<(), tonic::Status> {
    let mut client = grpc.local_agent_client().await;
    let mut stream = client
        .subscribe_token_stream(SubscribeTokenStreamRequest {})
        .await?
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
        let chunk = chunk_result?;

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
        .map_err(|e| refusal_or(e, |e| grpc_err(e.message())))?;
    Ok(())
}

/// Create the chat a play is edited through, and return the chat's id. The
/// chat pins the play-authoring skill and the play, and opens on a message
/// the daemon writes; `provider` and `model` are the model it starts on.
#[tauri::command]
pub async fn local_agent_create_play_edit_chat(
    play_id: String,
    provider: Option<String>,
    model: Option<String>,
    grpc: State<'_, GrpcClient>,
) -> Result<String, CommandError> {
    let mut client = grpc.local_agent_client().await;
    let resp = client
        .create_play_edit_chat(CreatePlayEditChatRequest {
            play_id,
            provider,
            model,
        })
        .await
        .map_err(|e| refusal_or(e, |e| grpc_err(e.message())))?;
    Ok(resp.into_inner().chat_id)
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
        .map_err(|e| refusal_or(e, |e| grpc_err(e.message())))?;
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
        .map_err(|e| refusal_or(e, |e| grpc_err(e.message())))?
        .into_inner();

    let mut engine_swapped = false;
    let mut saw_terminal_event = false;

    while let Some(event_result) = stream.next().await {
        let event = event_result.map_err(|e| refusal_or(e, |e| grpc_err(e.message())))?;

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
        .map_err(|e| refusal_or(e, |e| grpc_err(e.message())))?
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::time::{timeout, Instant};

    /// The status the daemon returns for a request routed to a database that
    /// requires an extension this build does not support.
    fn refusal() -> tonic::Status {
        nodespace_proto::requires_extension::status(&["fixture-ext".to_string()])
    }

    #[tokio::test(start_paused = true)]
    async fn after_a_refusal_it_resubscribes_when_the_active_database_changes_and_not_before() {
        let (switch, mut db_changed) = watch::channel(0u64);
        let outcome = Err(refusal());
        let wait = wait_to_resubscribe(&outcome, &mut db_changed, RESUBSCRIBE_DELAY);
        tokio::pin!(wait);

        // Long past the retry delay, it is still waiting.
        assert!(
            timeout(RESUBSCRIBE_DELAY * 10, &mut wait).await.is_err(),
            "a refused database is not asked again on a timer"
        );

        switch.send(1).unwrap();
        let resumed = Instant::now();
        timeout(RESUBSCRIBE_DELAY * 10, &mut wait)
            .await
            .expect("resubscribes once the active database changes");
        assert_eq!(resumed.elapsed(), Duration::ZERO, "and does so at once");
    }

    #[tokio::test(start_paused = true)]
    async fn any_other_outcome_retries_after_the_delay_as_before() {
        let (_switch, mut db_changed) = watch::channel(0u64);
        let outcomes = [
            Ok(()),
            Err(tonic::Status::unavailable("daemon restarting")),
            Err(tonic::Status::failed_precondition("model not ready")),
        ];
        for outcome in outcomes {
            let start = Instant::now();
            timeout(
                RESUBSCRIBE_DELAY * 10,
                wait_to_resubscribe(&outcome, &mut db_changed, RESUBSCRIBE_DELAY),
            )
            .await
            .expect("retries without waiting for a database change");
            assert_eq!(start.elapsed(), RESUBSCRIBE_DELAY, "{outcome:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_falls_back_to_the_delay_once_the_client_is_gone() {
        let (switch, mut db_changed) = watch::channel(0u64);
        drop(switch);

        let start = Instant::now();
        timeout(
            RESUBSCRIBE_DELAY * 10,
            wait_to_resubscribe(&Err(refusal()), &mut db_changed, RESUBSCRIBE_DELAY),
        )
        .await
        .expect("does not wait forever");
        assert_eq!(start.elapsed(), RESUBSCRIBE_DELAY);
    }

    #[test]
    fn only_the_refusal_is_a_database_refusal() {
        assert!(is_database_refusal(&refusal()));
        assert!(!is_database_refusal(&tonic::Status::failed_precondition(
            "model not ready"
        )));
        assert!(!is_database_refusal(&tonic::Status::unavailable("down")));
    }

    /// Runs `resubscribe_forever` over a fake attempt that fails with
    /// `error` and counts its calls, and an active-database generation the
    /// test bumps, like `GrpcClient::subscribe_active_database`. `on_first`
    /// runs inside the first attempt.
    fn run_loop(
        error: fn() -> tonic::Status,
        on_first: impl Fn(&watch::Sender<u64>) + Send + 'static,
    ) -> (
        Arc<watch::Sender<u64>>,
        Arc<AtomicUsize>,
        tokio::task::JoinHandle<()>,
    ) {
        let generation = Arc::new(watch::channel(0u64).0);
        let attempts = Arc::new(AtomicUsize::new(0));
        let (subscribe, in_attempt, counted) =
            (generation.clone(), generation.clone(), attempts.clone());
        let task = tokio::spawn(resubscribe_forever(
            move || subscribe.subscribe(),
            move |_db_changed| {
                if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                    on_first(&in_attempt);
                }
                std::future::ready(Err(error()))
            },
        ));
        (generation, attempts, task)
    }

    #[tokio::test(start_paused = true)]
    async fn the_loop_waits_out_a_refusal_and_resubscribes_once_the_active_database_changes() {
        let (generation, attempts, task) = run_loop(refusal, |_| {});

        tokio::time::sleep(RESUBSCRIBE_DELAY * 10).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "a refused database is not asked again on a timer"
        );

        generation.send_modify(|g| *g += 1);
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            2,
            "resubscribes once the active database changes"
        );
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn the_loop_does_not_miss_a_change_during_the_refused_attempt() {
        let (_generation, attempts, task) =
            run_loop(refusal, |generation| generation.send_modify(|g| *g += 1));

        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn the_loop_retries_any_other_failure_every_delay_as_before() {
        let (_generation, attempts, task) =
            run_loop(|| tonic::Status::unavailable("daemon restarting"), |_| {});

        // Attempts at 0, 2, 4, ..., 20 seconds.
        tokio::time::sleep(RESUBSCRIBE_DELAY * 10 + Duration::from_millis(1)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 11);
        task.abort();
    }
}
