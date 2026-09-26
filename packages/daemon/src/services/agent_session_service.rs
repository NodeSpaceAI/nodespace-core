//! tonic `AgentSessionService` implementation backed by `PtySessionManager`.
//!
//! Each RPC adapts a proto request into the corresponding `PtySessionManager`
//! call and converts results back into proto messages. `StreamOutput` is the
//! only server-streaming RPC: it subscribes to the session's broadcast channel
//! and forwards [`OutputChunk`](crate::nodespace::OutputChunk) messages until
//! the session closes or the client disconnects.
//!
//! The handler owns `Arc` handles to the shared engine state so it stays cheap
//! to construct and clone, and so multiple concurrent RPCs see the same set of
//! sessions.

use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use chrono::Utc;
use nodespace_agent::agent_catalog::context_assembly::GraphContextAssembler;
use nodespace_agent::agent_types::AgentType;
use nodespace_agent::pty::{detect_all_agents, PtySession, PtySessionManager};
use nodespace_core::services::NodeService as CoreNodeService;
use tokio::sync::broadcast::error::RecvError;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::nodespace::{
    agent_session_service_server::AgentSessionService, AgentAvailability, CheckAvailabilityRequest,
    CheckAvailabilityResponse, LaunchSessionRequest, LaunchSessionResponse, ListSessionsRequest,
    ListSessionsResponse, OutputChunk, ResizeRequest, ResizeResponse, SessionInfo,
    StreamOutputRequest, TerminateSessionRequest, TerminateSessionResponse, WriteInputRequest,
    WriteInputResponse,
};
use crate::services::capture_service::{finalize_capture, CompletedSession};
use crate::services::settings_service::{read_capture_settings, CaptureConfig};

/// gRPC adapter that owns shared handles to the PTY engine.
#[derive(Clone)]
pub struct AgentSessionHandler {
    manager: Arc<PtySessionManager>,
    assembler: Arc<GraphContextAssembler>,
    node_service: Arc<CoreNodeService>,
    config_path: PathBuf,
}

impl AgentSessionHandler {
    pub fn new(
        manager: Arc<PtySessionManager>,
        assembler: Arc<GraphContextAssembler>,
        node_service: Arc<CoreNodeService>,
        config_path: PathBuf,
    ) -> Self {
        Self {
            manager,
            assembler,
            node_service,
            config_path,
        }
    }

    /// Resolve which database this request targets (ADR-053) and return that
    /// database's handler. The routing contract lives in
    /// [`crate::db_routing::routed_database_services`]: a header selects a
    /// registered database, header-less requests hit the default, and with no
    /// routing middleware installed a header-less request falls back to `self`
    /// while a header-carrying one is rejected.
    ///
    /// Only [`launch_session`](Self::launch_session) routes: the `PtySessionManager`
    /// is process-global (the same `Arc` backs every database), so PTY
    /// operations (stream/write/resize/terminate/list) act on the shared session
    /// set regardless of database. Launch is the sole handler that reads the
    /// per-database context assembler and node service (for context + capture),
    /// so it follows the targeted database.
    async fn route<T>(&self, request: &Request<T>) -> Result<AgentSessionHandler, Status> {
        match crate::db_routing::routed_database_services(request).await? {
            Some(services) => Ok(services.agent_session.clone()),
            None => Ok(self.clone()),
        }
    }
}

#[tonic::async_trait]
impl AgentSessionService for AgentSessionHandler {
    async fn launch_session(
        &self,
        request: Request<LaunchSessionRequest>,
    ) -> Result<Response<LaunchSessionResponse>, Status> {
        let this = self.route(&request).await?;
        let req = request.into_inner();

        let agent_type = parse_agent_type(&req.agent_type).map_err(Status::invalid_argument)?;
        let agent_type_str = agent_type_to_string(agent_type);

        // Defense-in-depth: reject the launch if the agent is not ready.
        // The UI calls CheckAgentAvailability before showing the Launch button,
        // but the RPC guard ensures the daemon never spawns a doomed process
        // even if the UI check is skipped or stale.
        //
        // detect_all_agents() spawns /usr/libexec/path_helper via
        // std::process::Command on macOS — a blocking call that must not run
        // on a Tokio executor thread.
        let availability = tokio::task::spawn_blocking(move || {
            detect_all_agents()
                .into_iter()
                .find(|a| a.agent_type == agent_type)
        })
        .await
        .map_err(|e| Status::internal(format!("agent detection task panicked: {e}")))?;
        if let Some(av) = availability {
            if !av.binary_found || !av.auth_found {
                let reason = match (av.binary_found, av.auth_found) {
                    (false, false) => format!(
                        "agent '{}' is not ready: binary '{}' not found and auth not configured",
                        req.agent_type, av.binary
                    ),
                    (false, _) => format!(
                        "agent '{}' is not ready: binary '{}' not found on PATH{}",
                        req.agent_type,
                        av.binary,
                        av.install_hint
                            .map(|h| format!(" — {h}"))
                            .unwrap_or_default()
                    ),
                    (_, false) => format!(
                        "agent '{}' is not ready: auth credential not configured",
                        req.agent_type
                    ),
                    _ => unreachable!(),
                };
                return Err(Status::failed_precondition(reason));
            }
        }

        let id = this
            .manager
            .launch(agent_type, req.prompt, &this.assembler)
            .await
            .map_err(|e| Status::internal(format!("launch session failed: {e}")))?;

        // Pin the session via one lookup so (a) `started_at` comes from the
        // authoritative PtySession value (matching what ListSessions reports)
        // and (b) the optional initial resize cannot race the auto-prune
        // watcher into a spurious NotFound for a session we just created.
        // `unwrap_or` fallback covers the vanishingly unlikely case where the
        // spawned child has already exited and the auto-prune watcher has
        // already removed the entry.
        let session = this.manager.get(&id).await;
        let created_at = session
            .as_ref()
            .map(|s| s.started_at.timestamp())
            .unwrap_or_else(current_unix_secs);

        // Apply requested dimensions when the caller passed non-zero values.
        // Zero on either axis means "keep the engine default" (80x24);
        // ResizeTerminal, in contrast, rejects zero outright — see its proto
        // comment.
        //
        // If the auto-prune race removed the entry between launch() and get()
        // above, the resize is silently skipped — the response is still Ok
        // because the launch itself succeeded; the next RPC against this id
        // will naturally return NotFound. Log at warn so the dropped resize is
        // visible in server-side traces.
        if req.cols != 0 && req.rows != 0 {
            match session {
                Some(session) => resize_session(&session, req.cols, req.rows).await?,
                None => tracing::warn!(
                    session_id = %id,
                    cols = req.cols,
                    rows = req.rows,
                    "LaunchSession: session auto-pruned before initial resize could apply"
                ),
            }
        }

        // Spawn a capture task that waits for the session to exit, then
        // backfills the session's ai-chat node if capture is enabled. This runs
        // after the launch response is returned — it does not block session
        // start.
        //
        // The node already exists (created up front, provider mode 2d per
        // ADR-034); `node_id` identifies it so capture backfills that node
        // rather than minting a new one.
        //
        // Capture config is read once here (at launch time) so finalize_capture
        // doesn't re-hit the filesystem on every session end. Sessions started
        // before the handler initializes are not covered (no manager.get hit).
        if let Some(ref session) = this.manager.get(&id).await {
            let session = session.clone();
            let node_service = this.node_service.clone();
            let config_path = this.config_path.clone();
            let started_at = session.started_at;
            let node_id = req.node_id.clone();

            tokio::spawn(async move {
                let config = match read_capture_settings(&config_path).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "capture: failed to read config, skipping");
                        CaptureConfig::default()
                    }
                };

                // Wait for the child process to exit.
                let mut exit_rx = session.subscribe_exit();
                let exit_status = loop {
                    if let Some(status) = *exit_rx.borrow() {
                        break status;
                    }
                    if exit_rx.changed().await.is_err() {
                        // Sender dropped — session already gone.
                        return;
                    }
                };

                let capture = session.snapshot_capture().await;
                let completed = CompletedSession {
                    id: session.id,
                    node_id,
                    agent_type: agent_type_str,
                    started_at,
                    ended_at: Utc::now(),
                    exit_status,
                };

                if let Err(e) = finalize_capture(&completed, &capture, &node_service, &config).await
                {
                    tracing::warn!(
                        session_id = %session.id,
                        error = %e,
                        "session capture failed (non-fatal)"
                    );
                }
            });
        }

        Ok(Response::new(LaunchSessionResponse {
            session_id: id.to_string(),
            created_at,
        }))
    }

    type StreamOutputStream =
        Pin<Box<dyn tokio_stream::Stream<Item = Result<OutputChunk, Status>> + Send + 'static>>;

    async fn stream_output(
        &self,
        request: Request<StreamOutputRequest>,
    ) -> Result<Response<Self::StreamOutputStream>, Status> {
        let id =
            parse_session_id(&request.into_inner().session_id).map_err(Status::invalid_argument)?;
        let session = self
            .manager
            .get(&id)
            .await
            .ok_or_else(|| Status::not_found(format!("session not found: {id}")))?;

        let mut rx = session.subscribe_output();

        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(chunk) => {
                        let timestamp_ms = chunk.timestamp.timestamp_millis();
                        yield Ok(OutputChunk {
                            data: chunk.data,
                            timestamp_ms,
                            dropped_chunks: 0,
                        });
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        // Client (or this handler) fell behind the broadcast
                        // buffer. Keep draining rather than tearing the
                        // stream down — losing a render frame is preferable
                        // to losing the whole session view — but tell the
                        // client about the gap with a loss-marker chunk so it
                        // can show that output was truncated.
                        debug_assert!(skipped > 0, "RecvError::Lagged with zero skipped chunks");
                        tracing::warn!(
                            session_id = %id,
                            skipped,
                            "StreamOutput subscriber lagged; some chunks dropped"
                        );
                        yield Ok(OutputChunk {
                            data: Vec::new(),
                            timestamp_ms: Utc::now().timestamp_millis(),
                            dropped_chunks: skipped,
                        });
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }

    async fn write_input(
        &self,
        request: Request<WriteInputRequest>,
    ) -> Result<Response<WriteInputResponse>, Status> {
        let req = request.into_inner();
        let id = parse_session_id(&req.session_id).map_err(Status::invalid_argument)?;
        let session = self
            .manager
            .get(&id)
            .await
            .ok_or_else(|| Status::not_found(format!("session not found: {id}")))?;

        let len = req.data.len();
        session
            .write_input(&req.data)
            .await
            .map_err(|e| Status::internal(format!("write_input failed: {e}")))?;

        // PtySession::write_input writes the entire buffer atomically and
        // flushes, so on success bytes_written always equals the input length.
        // The proto field is int64 (widened from int32 during review)
        // so a 64-bit `usize` cannot truncate — fits the full `data.len()`
        // range on every realistic platform.
        Ok(Response::new(WriteInputResponse {
            bytes_written: len as i64,
        }))
    }

    async fn resize_terminal(
        &self,
        request: Request<ResizeRequest>,
    ) -> Result<Response<ResizeResponse>, Status> {
        let req = request.into_inner();
        let id = parse_session_id(&req.session_id).map_err(Status::invalid_argument)?;

        // Unlike LaunchSession, ResizeTerminal has no "0 means default"
        // semantic. Reject zero at the API boundary so the underlying
        // portable_pty call never sees PtySize { rows: 0, cols: 0, .. }
        // (which is platform-dependent and reliably surprising).
        if req.cols == 0 || req.rows == 0 {
            return Err(Status::invalid_argument(format!(
                "ResizeTerminal requires non-zero dimensions; got cols={}, rows={}",
                req.cols, req.rows
            )));
        }

        let session = self
            .manager
            .get(&id)
            .await
            .ok_or_else(|| Status::not_found(format!("session not found: {id}")))?;
        resize_session(&session, req.cols, req.rows).await?;
        Ok(Response::new(ResizeResponse {}))
    }

    async fn terminate_session(
        &self,
        request: Request<TerminateSessionRequest>,
    ) -> Result<Response<TerminateSessionResponse>, Status> {
        let req = request.into_inner();
        let id = parse_session_id(&req.session_id).map_err(Status::invalid_argument)?;

        let was_running = self
            .manager
            .terminate(&id)
            .await
            .map_err(|e| Status::internal(format!("terminate failed: {e}")))?;

        Ok(Response::new(TerminateSessionResponse {
            session_id: id.to_string(),
            was_running,
        }))
    }

    async fn list_sessions(
        &self,
        _request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let metas = self.manager.list().await;
        let sessions: Vec<SessionInfo> = metas
            .into_iter()
            .map(|m| SessionInfo {
                session_id: m.id.to_string(),
                agent_type: agent_type_to_string(m.agent_type),
                started_at: m.started_at.timestamp(),
            })
            .collect();

        // Saturating cast: the manager's HashMap is `usize`-keyed, but `u32`
        // (~4.2B) is well above any realistic session count for a single
        // daemon. Saturating instead of wrapping keeps the count monotonic
        // for clients that compare snapshots.
        let count = u32::try_from(sessions.len()).unwrap_or(u32::MAX);
        Ok(Response::new(ListSessionsResponse { sessions, count }))
    }

    async fn check_agent_availability(
        &self,
        _request: Request<CheckAvailabilityRequest>,
    ) -> Result<Response<CheckAvailabilityResponse>, Status> {
        // detect_all_agents() spawns /usr/libexec/path_helper via
        // std::process::Command on macOS — a blocking call that must not run
        // on a Tokio executor thread.
        let results = tokio::task::spawn_blocking(detect_all_agents)
            .await
            .map_err(|e| Status::internal(format!("agent detection task panicked: {e}")))?;
        let agents: Vec<AgentAvailability> = results
            .into_iter()
            .map(|av| AgentAvailability {
                agent_type: agent_type_to_string(av.agent_type),
                binary: av.binary.to_string(),
                binary_found: av.binary_found,
                auth_found: av.auth_found,
                binary_path: av.binary_path.map(|p| p.to_string_lossy().into_owned()),
                install_hint: av.install_hint.map(str::to_string),
            })
            .collect();
        Ok(Response::new(CheckAvailabilityResponse { agents }))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
//
// The parsers below return `Result<T, String>` rather than `Result<T, Status>`
// so the small-Ok variants don't trip `clippy::result_large_err` (tonic::Status
// is ~176 bytes and dwarfs `Uuid` / `AgentType`). Call sites map the string
// into the appropriate gRPC status code in one line, keeping the parser logic
// status-agnostic and trivially unit-testable.

fn parse_session_id(raw: &str) -> Result<Uuid, String> {
    Uuid::from_str(raw).map_err(|e| format!("invalid session_id '{raw}': {e}"))
}

/// Convert the proto's `agent_type` string into the canonical [`AgentType`].
///
/// The only accepted form is the kebab-case serde representation of
/// [`AgentType`] (`"claude-code"`, `"codex"`, `"antigravity-cli"`, `"pi"`,
/// `"open-code"`). Snake-case is rejected — CLAUDE.md is explicit that
/// greenfield code carries no backward-compat aliases.
fn parse_agent_type(raw: &str) -> Result<AgentType, String> {
    serde_json::from_value::<AgentType>(serde_json::Value::String(raw.to_string())).map_err(|_| {
        format!(
            "unknown agent_type '{raw}'; expected one of: claude-code, codex, antigravity-cli, pi, open-code"
        )
    })
}

fn agent_type_to_string(agent_type: AgentType) -> String {
    // serde serialization mirrors the kebab-case form parse_agent_type accepts.
    // `AgentType` is in-workspace and closed, so the debug-format fallback
    // is genuinely unreachable today — it exists as defense-in-depth, not as
    // a graceful-degradation path. The right way to add a new variant is to
    // extend parse_agent_type / agent_type_to_string together; do NOT rely
    // on the fallback to ship a new variant.
    serde_json::to_value(agent_type)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{agent_type:?}"))
}

/// Apply a resize to an already-located [`PtySession`].
///
/// Takes an `&Arc<PtySession>` rather than a session id so callers that
/// already hold the session (e.g. `launch_session` after its initial
/// lookup) don't take the manager's lock a second time. The `Arc` also
/// guarantees the underlying session can't be auto-pruned between this
/// call and the actual resize — eliminating a small race that would
/// otherwise return `NotFound` for a session we just created.
async fn resize_session(session: &Arc<PtySession>, cols: u32, rows: u32) -> Result<(), Status> {
    let cols = u16::try_from(cols)
        .map_err(|_| Status::invalid_argument(format!("cols {cols} exceeds u16 range")))?;
    let rows = u16::try_from(rows)
        .map_err(|_| Status::invalid_argument(format!("rows {rows} exceeds u16 range")))?;

    session
        .resize(cols, rows)
        .await
        .map_err(|e| Status::internal(format!("resize failed: {e}")))
}

fn current_unix_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_agent_type_accepts_kebab_case() {
        assert_eq!(
            parse_agent_type("claude-code").unwrap(),
            AgentType::ClaudeCode
        );
        assert_eq!(parse_agent_type("codex").unwrap(), AgentType::Codex);
        assert_eq!(
            parse_agent_type("antigravity-cli").unwrap(),
            AgentType::AntigravityCli
        );
        assert_eq!(parse_agent_type("pi").unwrap(), AgentType::Pi);
        assert_eq!(parse_agent_type("open-code").unwrap(), AgentType::OpenCode);
    }

    #[test]
    fn parse_agent_type_rejects_snake_case() {
        // Snake-case is the proto-comment form from the original spec
        // but was dropped in review per CLAUDE.md's no-backwards-
        // compat directive. Pin the rejection so a future refactor can't
        // silently restore the dual-accept path.
        for snake in ["claude_code", "antigravity_cli", "open_code"] {
            assert!(
                parse_agent_type(snake).is_err(),
                "snake_case agent_type '{snake}' must be rejected"
            );
        }
    }

    #[test]
    fn parse_agent_type_rejects_unknown() {
        let err = parse_agent_type("not-a-real-agent").unwrap_err();
        assert!(
            err.contains("not-a-real-agent"),
            "error should echo offending input: {err}"
        );
    }

    #[test]
    fn agent_type_round_trips_through_string() {
        for t in [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::AntigravityCli,
            AgentType::Pi,
            AgentType::OpenCode,
        ] {
            let s = agent_type_to_string(t);
            assert_eq!(parse_agent_type(&s).unwrap(), t);
        }
    }

    #[test]
    fn parse_session_id_rejects_garbage() {
        let err = parse_session_id("not-a-uuid").unwrap_err();
        assert!(
            err.contains("not-a-uuid"),
            "error should echo offending input: {err}"
        );
    }
}
