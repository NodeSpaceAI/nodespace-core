//! Tauri command proxies for `AgentSessionService` gRPC RPCs.
//!
//! Each command wraps one RPC. `launch_session` additionally spawns a
//! background task that reads the `StreamOutput` server-streaming response
//! and emits Tauri events keyed by session ID so the frontend's
//! `PtyTerminal.svelte` can subscribe via `listen("pty-output-{sessionId}")`.
//!
//! ## Streaming task lifecycle
//!
//! Each launched session gets a `CancellationToken` stored in
//! `StreamingTaskRegistry`. When `terminate_session` is called, the registry
//! cancels the token, which causes the background streaming loop to exit
//! promptly rather than waiting for the next gRPC message or a closed stream.
//!
//! ## Client-side timeouts
//!
//! The shared lazy channel has no client-side timeout, so a wedged h2
//! connection (healthy daemon, stuck transport) would hang a unary call until
//! the app's channel probe rebuilds it. Every unary session command is bounded
//! ([`PTY_RPC_TIMEOUT`], or [`TERMINATE_RPC_TIMEOUT`] for `terminate_session`)
//! so a wedge surfaces as an error instead of a frozen terminal. A timeout does
//! not prove the channel is wedged — the daemon can also be legitimately slow
//! (see the constants) — so it is reported, never used to force a reconnect.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use futures::StreamExt;
use nodespace_proto::{
    CheckAvailabilityRequest, LaunchSessionRequest, ListSessionsRequest, ResizeRequest,
    TerminateSessionRequest, WriteInputRequest,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;
use tonic::Request;

use crate::commands::nodes::CommandError;
use crate::services::GrpcClient;

// ---------------------------------------------------------------------------
// Streaming task registry — tracks cancellation tokens by session ID
// ---------------------------------------------------------------------------

/// Tauri managed state that maps session IDs to cancellation tokens for their
/// background `StreamOutput` reader tasks. Allows `terminate_session` to stop
/// the reader promptly rather than waiting for the gRPC stream to drain.
#[derive(Default)]
pub struct StreamingTaskRegistry {
    tokens: Mutex<HashMap<String, CancellationToken>>,
}

impl StreamingTaskRegistry {
    pub fn insert(&self, session_id: &str, token: CancellationToken) {
        if let Ok(mut map) = self.tokens.lock() {
            map.insert(session_id.to_string(), token);
        }
    }

    pub fn cancel_and_remove(&self, session_id: &str) {
        if let Ok(mut map) = self.tokens.lock() {
            if let Some(token) = map.remove(session_id) {
                token.cancel();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Frontend-facing types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSessionResult {
    pub session_id: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PtyOutputPayload {
    pub data: Vec<u8>,
    pub timestamp_ms: i64,
    /// Non-zero when the daemon dropped this many chunks because the stream
    /// fell behind; `data` is then empty and the frontend shows a truncation
    /// notice in place of the missing output.
    pub dropped_chunks: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtySessionInfo {
    pub session_id: String,
    pub agent_type: String,
    pub started_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsResult {
    pub sessions: Vec<PtySessionInfo>,
    pub count: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminateSessionResult {
    pub session_id: String,
    pub was_running: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentAvailabilityInfo {
    pub agent_type: String,
    pub binary: String,
    pub binary_found: bool,
    pub auth_found: bool,
    pub binary_path: Option<String>,
    pub install_hint: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckAvailabilityResult {
    pub agents: Vec<AgentAvailabilityInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchSessionInput {
    pub agent_type: String,
    pub prompt: Option<String>,
    pub cols: u32,
    pub rows: u32,
    /// ID of the `ai-chat` node this PTY session is a view onto (provider mode
    /// 2d). Capture backfills this node at session end. See ADR-034.
    pub node_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Bound on the unary PTY RPCs other than `TerminateSession`. These are fast
/// on a healthy daemon, with one exception: `WriteInput` blocks on the PTY
/// write, so an agent that stops reading its stdin (e.g. a large paste into a
/// busy agent) can exceed it without any channel fault.
const PTY_RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// Bound on `TerminateSession`. The daemon SIGHUPs the agent and waits,
/// unbounded, for it to exit — a CLI that shuts down gracefully can take a few
/// seconds, so this is looser than [`PTY_RPC_TIMEOUT`].
const TERMINATE_RPC_TIMEOUT: Duration = Duration::from_secs(15);

/// Await a unary RPC under `timeout`, mapping an elapsed deadline to
/// `DeadlineExceeded` so it flows through [`status_to_command_error`] like any
/// other gRPC failure.
async fn with_timeout<T>(
    timeout: Duration,
    rpc: &str,
    call: impl Future<Output = Result<tonic::Response<T>, tonic::Status>>,
) -> Result<T, CommandError> {
    match tokio::time::timeout(timeout, call).await {
        Ok(result) => result
            .map(tonic::Response::into_inner)
            .map_err(status_to_command_error),
        Err(_elapsed) => Err(status_to_command_error(tonic::Status::deadline_exceeded(
            format!("{rpc} timed out"),
        ))),
    }
}

fn status_to_command_error(status: tonic::Status) -> CommandError {
    let code = match status.code() {
        tonic::Code::NotFound => "SESSION_NOT_FOUND",
        tonic::Code::InvalidArgument => "INVALID_ARGUMENT",
        tonic::Code::FailedPrecondition => "AGENT_NOT_READY",
        _ => "GRPC_ERROR",
    }
    .to_string();
    CommandError {
        message: status.message().to_string(),
        code,
        details: Some(format!("{:?}", status.code())),
        conflict_data: None,
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Launch a PTY agent session and start streaming its output as Tauri events.
///
/// The frontend should listen on `"pty-output-{sessionId}"` immediately after
/// calling this command. The background streaming task runs until the session
/// closes, the client disconnects, or `terminate_session` cancels it.
#[tauri::command]
pub async fn launch_session(
    client: State<'_, GrpcClient>,
    registry: State<'_, StreamingTaskRegistry>,
    app: AppHandle,
    input: LaunchSessionInput,
) -> Result<LaunchSessionResult, CommandError> {
    let mut c = client.agent_session_client().await;

    let resp = c
        .launch_session(Request::new(LaunchSessionRequest {
            agent_type: input.agent_type,
            prompt: input.prompt,
            cols: input.cols,
            rows: input.rows,
            node_id: input.node_id,
        }))
        .await
        .map_err(status_to_command_error)?;

    let inner = resp.into_inner();
    let session_id = inner.session_id.clone();
    let created_at = inner.created_at;

    // Obtain the client before spawning (State<'_> has a non-'static lifetime).
    let mut stream_client = client.agent_session_client().await;

    let cancel_token = CancellationToken::new();
    registry.insert(&session_id, cancel_token.clone());

    // Spawn background task: reads StreamOutput and emits Tauri events.
    let session_id_for_task = session_id.clone();
    tauri::async_runtime::spawn(async move {
        let stream_result = stream_client
            .stream_output(Request::new(nodespace_proto::StreamOutputRequest {
                session_id: session_id_for_task.clone(),
            }))
            .await;

        let mut stream = match stream_result {
            Ok(r) => r.into_inner(),
            Err(e) => {
                tracing::warn!(
                    session_id = %session_id_for_task,
                    error = %e,
                    "Failed to open StreamOutput for session"
                );
                return;
            }
        };

        let event_name = format!("pty-output-{}", session_id_for_task);
        loop {
            tokio::select! {
                // Stop the loop when terminate_session cancels the token.
                _ = cancel_token.cancelled() => {
                    tracing::debug!(session_id = %session_id_for_task, "StreamOutput reader cancelled");
                    break;
                }
                chunk_result = stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            let payload = PtyOutputPayload {
                                data: chunk.data.to_vec(),
                                timestamp_ms: chunk.timestamp_ms,
                                dropped_chunks: chunk.dropped_chunks,
                            };
                            if let Err(e) = app.emit(&event_name, payload) {
                                tracing::warn!(
                                    session_id = %session_id_for_task,
                                    error = %e,
                                    "Failed to emit pty-output event"
                                );
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            tracing::debug!(
                                session_id = %session_id_for_task,
                                error = %e,
                                "StreamOutput ended"
                            );
                            break;
                        }
                        None => break,
                    }
                }
            }
        }

        // Emit a sentinel so the frontend knows the stream is done.
        let _ = app.emit(&format!("pty-closed-{}", session_id_for_task), ());
    });

    Ok(LaunchSessionResult {
        session_id,
        created_at,
    })
}

/// Write raw bytes (keystrokes) to a PTY session's stdin.
#[tauri::command]
pub async fn write_input(
    client: State<'_, GrpcClient>,
    session_id: String,
    data: Vec<u8>,
) -> Result<i64, CommandError> {
    let mut c = client.agent_session_client().await;
    let resp = with_timeout(
        PTY_RPC_TIMEOUT,
        "WriteInput",
        c.write_input(Request::new(WriteInputRequest { session_id, data })),
    )
    .await?;
    Ok(resp.bytes_written)
}

/// Notify the PTY session of a terminal resize.
#[tauri::command]
pub async fn resize_terminal(
    client: State<'_, GrpcClient>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> Result<(), CommandError> {
    let mut c = client.agent_session_client().await;
    with_timeout(
        PTY_RPC_TIMEOUT,
        "ResizeTerminal",
        c.resize_terminal(Request::new(ResizeRequest {
            session_id,
            cols,
            rows,
        })),
    )
    .await?;
    Ok(())
}

/// Terminate a PTY session and clean up its resources.
///
/// Once the daemon confirms the `TerminateSession` RPC, cancels the background
/// `StreamOutput` reader task so it exits immediately rather than waiting for
/// the next message from a now-dead stream. When the RPC fails or times out the
/// reader is left running instead: if the request never reached the daemon the
/// session is still alive and still needs its reader, and if it did, the
/// session is shutting down and the reader ends on its own when the stream
/// closes.
#[tauri::command]
pub async fn terminate_session(
    client: State<'_, GrpcClient>,
    registry: State<'_, StreamingTaskRegistry>,
    session_id: String,
) -> Result<TerminateSessionResult, CommandError> {
    let mut c = client.agent_session_client().await;
    let inner = with_timeout(
        TERMINATE_RPC_TIMEOUT,
        "TerminateSession",
        c.terminate_session(Request::new(TerminateSessionRequest {
            session_id: session_id.clone(),
        })),
    )
    .await?;
    registry.cancel_and_remove(&session_id);
    Ok(TerminateSessionResult {
        session_id: inner.session_id,
        was_running: inner.was_running,
    })
}

/// List all active PTY sessions.
#[tauri::command]
pub async fn list_sessions(
    client: State<'_, GrpcClient>,
) -> Result<ListSessionsResult, CommandError> {
    let mut c = client.agent_session_client().await;
    let inner = with_timeout(
        PTY_RPC_TIMEOUT,
        "ListSessions",
        c.list_sessions(Request::new(ListSessionsRequest {})),
    )
    .await?;
    let sessions = inner
        .sessions
        .into_iter()
        .map(|s| PtySessionInfo {
            session_id: s.session_id,
            agent_type: s.agent_type,
            started_at: s.started_at,
        })
        .collect();
    Ok(ListSessionsResult {
        sessions,
        count: inner.count,
    })
}

/// Check which PTY agents have their binary and auth credentials configured.
#[tauri::command]
pub async fn check_agent_availability(
    client: State<'_, GrpcClient>,
) -> Result<CheckAvailabilityResult, CommandError> {
    let mut c = client.agent_session_client().await;
    let inner = with_timeout(
        PTY_RPC_TIMEOUT,
        "CheckAgentAvailability",
        c.check_agent_availability(Request::new(CheckAvailabilityRequest {})),
    )
    .await?;
    let agents = inner
        .agents
        .into_iter()
        .map(|a| AgentAvailabilityInfo {
            agent_type: a.agent_type,
            binary: a.binary,
            binary_found: a.binary_found,
            auth_found: a.auth_found,
            binary_path: a.binary_path,
            install_hint: a.install_hint,
        })
        .collect();
    Ok(CheckAvailabilityResult { agents })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn with_timeout_maps_a_hung_call_to_deadline_exceeded() {
        let hung = std::future::pending::<Result<tonic::Response<()>, tonic::Status>>();
        let err = with_timeout(Duration::from_millis(10), "WriteInput", hung)
            .await
            .unwrap_err();
        assert_eq!(err.code, "GRPC_ERROR");
        assert_eq!(err.details.as_deref(), Some("DeadlineExceeded"));
        assert_eq!(err.message, "WriteInput timed out");
    }

    #[tokio::test]
    async fn with_timeout_passes_through_responses_and_statuses() {
        let ok = async { Ok(tonic::Response::new(7_i64)) };
        assert_eq!(
            with_timeout(PTY_RPC_TIMEOUT, "WriteInput", ok)
                .await
                .unwrap(),
            7
        );

        let not_found =
            async { Err::<tonic::Response<()>, _>(tonic::Status::not_found("no session")) };
        let err = with_timeout(PTY_RPC_TIMEOUT, "WriteInput", not_found)
            .await
            .unwrap_err();
        assert_eq!(err.code, "SESSION_NOT_FOUND");
    }
}
