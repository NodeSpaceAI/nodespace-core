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
use nodespace_agent::agent_types::AgentType;
use nodespace_agent::pty::{PtySession, PtySessionManager, SessionLaunch, LAUNCHED_FOR_ENV_VAR};
use nodespace_core::models::{CoreNodeType, Node, NodeUpdate};
use nodespace_core::services::NodeService as CoreNodeService;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::nodespace::{
    agent_session_service_server::AgentSessionService, AgentAvailability, CheckAvailabilityRequest,
    CheckAvailabilityResponse, LaunchSessionRequest, LaunchSessionResponse, ListSessionsRequest,
    ListSessionsResponse, OutputChunk, ReportHarnessSessionRequest, ReportHarnessSessionResponse,
    ResizeRequest, ResizeResponse, SessionInfo, StreamOutputRequest, TerminateSessionRequest,
    TerminateSessionResponse, WriteInputRequest, WriteInputResponse,
};
use crate::services::capture_service::{
    finalize_capture, CaptureConfig, CompletedSession, SessionSummarizer,
};

/// gRPC adapter that owns shared handles to the PTY engine.
#[derive(Clone)]
pub struct AgentSessionHandler {
    manager: Arc<PtySessionManager>,
    node_service: Arc<CoreNodeService>,
    /// Derives a finished session's summary, when capture saves one.
    summarizer: Arc<dyn SessionSummarizer>,
    /// Cancelled when the daemon stops serving. Open `StreamOutput` streams end
    /// on it: the PTY session set outlives the server's drain, and the drain
    /// waits for every open stream.
    serving_ended: CancellationToken,
}

impl AgentSessionHandler {
    pub fn new(
        manager: Arc<PtySessionManager>,
        node_service: Arc<CoreNodeService>,
        summarizer: Arc<dyn SessionSummarizer>,
    ) -> Self {
        Self {
            manager,
            node_service,
            summarizer,
            serving_ended: CancellationToken::new(),
        }
    }

    /// Ends this handler's output streams when `serving_ended` is cancelled.
    pub fn with_serving_ended(mut self, serving_ended: CancellationToken) -> Self {
        self.serving_ended = serving_ended;
        self
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
    /// per-database node service (for the project's folder and for capture),
    /// so it follows the targeted database.
    async fn route<T>(&self, request: &Request<T>) -> Result<AgentSessionHandler, Status> {
        crate::db_routing::route_or_self(self, request, |s| &s.agent_session).await
    }

    /// The software project a task is linked to: its own, else its direct
    /// parent's. The task's own link wins, so one parent can hold sub-issues
    /// for different repositories. A plain project does not count. `None`
    /// when neither route reaches a software project.
    async fn task_software_project(&self, task: &Node) -> Result<Option<String>, Status> {
        let read_failed = |e: &dyn std::fmt::Display| {
            Status::internal(format!("reading {} failed: {e}", task.id))
        };
        if let Some(project) = self.linked_software_project(&task.id).await? {
            return Ok(Some(project));
        }
        match self
            .node_service
            .get_parent(&task.id)
            .await
            .map_err(|e| read_failed(&e))?
        {
            Some(parent) => self.linked_software_project(&parent.id).await,
            None => Ok(None),
        }
    }

    /// The software project whose `tasks` link reaches `task_id`, if any.
    async fn linked_software_project(&self, task_id: &str) -> Result<Option<String>, Status> {
        let read_failed = |e: &dyn std::fmt::Display| {
            Status::internal(format!("reading the project of {task_id} failed: {e}"))
        };
        let projects = self
            .node_service
            .get_related_nodes(task_id, "tasks", "in")
            .await
            .map_err(|e| read_failed(&e))?;
        for project in projects {
            if self
                .node_service
                .type_is_a(&project.node_type, CoreNodeType::SoftwareProject)
                .await
                .map_err(|e| read_failed(&e))?
            {
                return Ok(Some(project.id));
            }
        }
        Ok(None)
    }

    /// The folder a launch runs in. A named project decides it. With only a
    /// task, the folder is that of the task's software project (its own, else
    /// its parent's); a plain task with none runs in the session's private
    /// folder, and an issue with none is refused, naming the missing link.
    async fn launch_folder(
        &self,
        project_id: Option<&str>,
        given_folder: Option<&str>,
        task: Option<&Node>,
    ) -> Result<Option<PathBuf>, Status> {
        if let Some(project_id) = project_id {
            return self
                .project_folder(project_id, given_folder)
                .await
                .map(Some);
        }
        let Some(task) = task else {
            return Ok(None);
        };
        if let Some(project_id) = self.task_software_project(task).await? {
            return self.project_folder(&project_id, None).await.map(Some);
        }
        let is_issue = self
            .node_service
            .type_is_a(&task.node_type, CoreNodeType::Issue)
            .await
            .map_err(|e| Status::internal(format!("reading {} failed: {e}", task.id)))?;
        if is_issue {
            return Err(Status::failed_precondition(format!(
                "issue '{}' is not linked to a software project, and neither is its parent task: \
                 link the issue (or its parent task) to a software project, or name one with --project",
                task.content
            )));
        }
        Ok(None)
    }

    /// The folder a session launched for `project_id` runs in: the software
    /// project's `checkout_path`, its folder on this machine (ADR-093 §8).
    /// Only a software project has one (ADR-098): a plain project is refused.
    ///
    /// `given` is the folder the launch names, for a project that has none
    /// yet or whose folder is being changed. It is checked and then stored on
    /// the project, so the next launch needs no folder named.
    async fn project_folder(
        &self,
        project_id: &str,
        given: Option<&str>,
    ) -> Result<PathBuf, Status> {
        let project = self
            .node_service
            .get_node(project_id)
            .await
            .map_err(|e| Status::internal(format!("reading project {project_id} failed: {e}")))?
            .ok_or_else(|| Status::not_found(format!("project not found: {project_id}")))?;
        let is_software_project = self
            .node_service
            .type_is_a(&project.node_type, CoreNodeType::SoftwareProject)
            .await
            .map_err(|e| Status::internal(format!("reading project {project_id} failed: {e}")))?;
        if !is_software_project {
            return Err(Status::invalid_argument(format!(
                "{project_id} is {}, not a software project: only a software project has a folder a session runs in",
                nodespace_core::utils::with_indefinite_article(&project.node_type)
            )));
        }

        let stored = project
            .properties
            .get(CoreNodeType::SoftwareProject.as_str())
            .and_then(|bucket| bucket.get("checkout_path"))
            .and_then(|value| value.as_str())
            .filter(|path| !path.is_empty());

        let Some(given) = given else {
            let stored = stored.ok_or_else(|| {
                Status::failed_precondition(format!(
                    "project '{}' has no folder on this machine: name its checkout folder to launch a session for it",
                    project.content
                ))
            })?;
            // The stored value can be written by any update of the project,
            // so it gets the checks a named folder gets.
            return checked_folder(stored).map_err(|problem| {
                Status::failed_precondition(format!(
                    "the folder of project '{}' cannot be used: {problem}",
                    project.content
                ))
            });
        };

        let folder = checked_folder(given).map_err(Status::invalid_argument)?;
        if stored != Some(given) {
            // The generic update, which stores the field in the bucket of
            // the schema that declares it for any type that extends
            // `software-project`.
            self.node_service
                .update_node(
                    project_id,
                    project.version,
                    NodeUpdate::default()
                        .with_properties(serde_json::json!({ "checkout_path": given })),
                )
                .await
                .map_err(|e| match e {
                    // The project changed between the read and the write.
                    nodespace_core::services::NodeServiceError::VersionConflict { .. } => {
                        Status::aborted(format!(
                            "project {project_id} changed while its folder was being stored: launch again"
                        ))
                    }
                    other => Status::internal(format!(
                        "storing the folder of project {project_id} failed: {other}"
                    )),
                })?;
        }
        Ok(folder)
    }
}

/// A project's folder as a path a session can run in: absolute, and an
/// existing directory on this machine. The error says which it is not.
fn checked_folder(folder: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(folder);
    if !path.is_absolute() {
        return Err(format!(
            "a project's folder is an absolute path, got: {folder}"
        ));
    }
    if !path.is_dir() {
        return Err(format!("not a directory on this machine: {folder}"));
    }
    Ok(path)
}

#[tonic::async_trait]
impl AgentSessionService for AgentSessionHandler {
    async fn launch_session(
        &self,
        request: Request<LaunchSessionRequest>,
    ) -> Result<Response<LaunchSessionResponse>, Status> {
        let this = self.route(&request).await?;
        let database_id = crate::db_routing::routed_database_id(&request).await?;
        let req = request.into_inner();

        let agent_type = parse_agent_type(&req.agent_type).map_err(Status::invalid_argument)?;

        // Defense-in-depth: reject the launch if the agent is not ready.
        // The UI calls CheckAgentAvailability before showing the Launch button,
        // but the RPC guard ensures the daemon never spawns a doomed process
        // even if the UI check is skipped or stale.
        //
        // Building the search path spawns /usr/libexec/path_helper via
        // std::process::Command on macOS — a blocking call that must not run
        // on a Tokio executor thread.
        let manager = this.manager.clone();
        // The path the agent is found ready on is the one it is launched
        // with: built once, used for both.
        let (availability, search_path) = tokio::task::spawn_blocking(move || {
            let (agents, path) = manager.detect_agents_with_path();
            (
                agents.into_iter().find(|a| a.agent_type == agent_type),
                path,
            )
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

        // Everything the launch names is checked before the project's folder
        // is stored: a refused launch changes nothing.
        if req.project_id.is_none() && req.project_folder.is_some() {
            return Err(Status::invalid_argument(
                "a project folder was named without a project",
            ));
        }
        let task = match req.task_id.as_deref() {
            Some(task_id) => Some(
                this.node_service
                    .get_node(task_id)
                    .await
                    .map_err(|e| Status::internal(format!("reading {task_id} failed: {e}")))?
                    .ok_or_else(|| Status::not_found(format!("node not found: {task_id}")))?,
            ),
            None => None,
        };
        let working_dir = this
            .launch_folder(
                req.project_id.as_deref(),
                req.project_folder.as_deref(),
                task.as_ref(),
            )
            .await?;
        // Beside the daemon's other state, so it follows NODESPACE_HOME.
        let session_folders = crate::nodespace_dir()
            .map_err(|e| Status::internal(format!("resolving the NodeSpace home failed: {e}")))?
            .join("agent-sessions");

        let launch = SessionLaunch {
            agent_type,
            initial_prompt: req.prompt,
            session_folders: Some(session_folders),
            env: session_environment(
                database_id,
                this.manager.daemon_socket(),
                req.task_id.as_deref().or(req.node_id.as_deref()),
            ),
            node_id: req.node_id,
            working_dir,
        };
        let id = this
            .manager
            .launch(launch, search_path)
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
        let working_dir = session
            .as_ref()
            .map(|s| s.working_dir.to_string_lossy().into_owned())
            .unwrap_or_default();

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

        // Spawn a task that waits for the session to exit, then marks the
        // session's `ai-chat-pty` node ended and, if capture is enabled, saves
        // what the session left behind. This runs after the launch response is
        // returned — it does not block session start.
        //
        // The node already exists (created up front, ADR-088); the session
        // carries its id, so that node is written rather than a new one minted.
        //
        // Capture config is read once here, from the session's database at
        // launch time, so finalize_capture doesn't re-read it on every session
        // end. Sessions started before the handler initializes are not covered
        // (no manager.get hit).
        if let Some(ref session) = this.manager.get(&id).await {
            let session = session.clone();
            let node_service = this.node_service.clone();
            let summarizer = this.summarizer.clone();

            tokio::spawn(async move {
                let config = match node_service.database_settings().await {
                    Ok((settings, _)) => CaptureConfig::from(&settings),
                    Err(e) => {
                        tracing::warn!(error = %e, "capture: failed to read the database's settings, saving no session content");
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
                    node_id: session.node_id.clone(),
                    ended_at: Utc::now(),
                    exit_status,
                    harness_session_id: session.harness_session_id(),
                };
                // Deriving the summary can wait a long time for the model;
                // the finished PTY session is not kept alive for it.
                drop(session);

                if let Err(e) = finalize_capture(
                    &completed,
                    &capture,
                    &node_service,
                    &config,
                    summarizer.as_ref(),
                )
                .await
                {
                    tracing::warn!(
                        session_id = %completed.id,
                        error = %e,
                        "failed to record the session's end on its node (non-fatal)"
                    );
                }
            });
        }

        Ok(Response::new(LaunchSessionResponse {
            session_id: id.to_string(),
            created_at,
            working_dir,
        }))
    }

    async fn report_harness_session(
        &self,
        request: Request<ReportHarnessSessionRequest>,
    ) -> Result<Response<ReportHarnessSessionResponse>, Status> {
        let req = request.into_inner();
        let id = parse_session_id(&req.session_id).map_err(Status::invalid_argument)?;
        let harness_session_id = req.harness_session_id.trim();
        if harness_session_id.is_empty() {
            return Err(Status::invalid_argument("harness_session_id is empty"));
        }
        let session = self
            .manager
            .get(&id)
            .await
            .ok_or_else(|| Status::not_found(format!("session not found: {id}")))?;

        session.report_harness_session_id(harness_session_id.to_string());

        Ok(Response::new(ReportHarnessSessionResponse {
            node_id: session.node_id.clone(),
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

        let serving_ended = self.serving_ended.clone();
        let stream = async_stream::stream! {
            loop {
                let received = tokio::select! {
                    received = rx.recv() => received,
                    _ = serving_ended.cancelled() => break,
                };
                match received {
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
                node_id: m.node_id,
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
        // Building the search path spawns /usr/libexec/path_helper via
        // std::process::Command on macOS — a blocking call that must not run
        // on a Tokio executor thread.
        let manager = self.manager.clone();
        let results = tokio::task::spawn_blocking(move || manager.detect_agents())
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

/// Convert the proto's `agent_type` string into the [`AgentType`] it names.
/// The only accepted form is the agent's id ([`AgentType::id`]).
fn parse_agent_type(raw: &str) -> Result<AgentType, String> {
    AgentType::from_id(raw).ok_or_else(|| {
        format!(
            "unknown agent_type '{raw}'; expected one of: {}",
            AgentType::ALL.map(AgentType::id).join(", ")
        )
    })
}

fn agent_type_to_string(agent_type: AgentType) -> String {
    agent_type.id().to_string()
}

/// What NodeSpace sets in a launched session's environment, on top of the
/// allowlist the PTY engine forwards: the database the session was launched
/// from and the socket of the daemon that launched it, so a `nodespace`
/// command inside reaches both, and what the session was launched for.
///
/// A value that is not known is left out, never set empty: the CLI reads an
/// absent variable as "the default".
fn session_environment(
    database_id: Option<String>,
    daemon_socket: Option<&std::path::Path>,
    launched_for: Option<&str>,
) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if let Some(database_id) = database_id {
        env.push((nodespace_proto::DATABASE_ENV_VAR.to_string(), database_id));
    }
    if let Some(socket) = daemon_socket {
        env.push((
            nodespace_proto::socket::SOCKET_ENV_VAR.to_string(),
            socket.to_string_lossy().into_owned(),
        ));
    }
    if let Some(launched_for) = launched_for {
        env.push((LAUNCHED_FOR_ENV_VAR.to_string(), launched_for.to_string()));
    }
    env
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
            parse_agent_type("antigravity").unwrap(),
            AgentType::Antigravity
        );
        assert_eq!(parse_agent_type("pi").unwrap(), AgentType::Pi);
        assert_eq!(parse_agent_type("opencode").unwrap(), AgentType::OpenCode);
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
            AgentType::Antigravity,
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

    // -- the folder a launch for a task runs in --------------------------------

    use nodespace_core::SqliteStore;
    use serde_json::json;
    use tempfile::TempDir;

    struct Fixture {
        handler: AgentSessionHandler,
        service: Arc<CoreNodeService>,
        _tmp: TempDir,
    }

    struct NoSummary;

    #[tonic::async_trait]
    impl SessionSummarizer for NoSummary {
        async fn summarize(&self, _plain_text: &str) -> Option<String> {
            None
        }
    }

    impl Fixture {
        async fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let mut store = Arc::new(SqliteStore::new(tmp.path().join("db")).await.unwrap());
            let service = Arc::new(CoreNodeService::new(&mut store).await.unwrap());
            let handler = AgentSessionHandler::new(
                Arc::new(PtySessionManager::new()),
                service.clone(),
                Arc::new(NoSummary),
            );
            Self {
                handler,
                service,
                _tmp: tmp,
            }
        }

        async fn node(
            &self,
            node_type: &str,
            content: &str,
            properties: serde_json::Value,
        ) -> Node {
            let id = self
                .service
                .create_node(Node::new(
                    node_type.to_string(),
                    content.to_string(),
                    properties,
                ))
                .await
                .unwrap();
            self.service.get_node(&id).await.unwrap().unwrap()
        }

        async fn link(&self, source: &Node, name: &str, target: &Node) {
            self.service
                .create_relationship(&source.id, name, &target.id, json!({}))
                .await
                .unwrap();
        }

        /// A software project whose checkout folder is a new directory, and
        /// that folder.
        async fn software_project(&self, name: &str) -> (Node, PathBuf) {
            let folder = self._tmp.path().join(name);
            std::fs::create_dir(&folder).unwrap();
            let project = self
                .node(
                    "software-project",
                    name,
                    json!({ "checkout_path": folder.to_string_lossy() }),
                )
                .await;
            (project, folder)
        }

        async fn folder_for(&self, task: &Node) -> Result<Option<PathBuf>, Status> {
            self.handler.launch_folder(None, None, Some(task)).await
        }
    }

    #[tokio::test]
    async fn a_task_launch_runs_in_its_own_software_project() {
        let f = Fixture::new().await;
        let (project, folder) = f.software_project("repo-a").await;
        let issue = f.node("issue", "Fix it", json!({})).await;
        f.link(&project, "tasks", &issue).await;
        assert_eq!(f.folder_for(&issue).await.unwrap(), Some(folder));
    }

    #[tokio::test]
    async fn a_task_launch_falls_back_to_its_parents_software_project() {
        let f = Fixture::new().await;
        let (project, folder) = f.software_project("repo-a").await;
        let parent = f.node("task", "Parent", json!({})).await;
        f.link(&project, "tasks", &parent).await;
        let issue = f.node("issue", "Fix it", json!({})).await;
        f.link(&parent, "has_child", &issue).await;
        assert_eq!(f.folder_for(&issue).await.unwrap(), Some(folder));
    }

    #[tokio::test]
    async fn the_tasks_own_project_wins_over_its_parents() {
        let f = Fixture::new().await;
        let (parent_project, _) = f.software_project("repo-parent").await;
        let (own_project, own_folder) = f.software_project("repo-own").await;
        let parent = f.node("task", "Parent", json!({})).await;
        f.link(&parent_project, "tasks", &parent).await;
        let issue = f.node("issue", "Fix it", json!({})).await;
        f.link(&parent, "has_child", &issue).await;
        f.link(&own_project, "tasks", &issue).await;
        assert_eq!(f.folder_for(&issue).await.unwrap(), Some(own_folder));
    }

    #[tokio::test]
    async fn a_named_project_still_decides_the_folder() {
        let f = Fixture::new().await;
        let (task_project, _) = f.software_project("repo-task").await;
        let (named, named_folder) = f.software_project("repo-named").await;
        let issue = f.node("issue", "Fix it", json!({})).await;
        f.link(&task_project, "tasks", &issue).await;
        let folder = f
            .handler
            .launch_folder(Some(&named.id), None, Some(&issue))
            .await
            .unwrap();
        assert_eq!(folder, Some(named_folder));
    }

    #[tokio::test]
    async fn an_issue_with_no_software_project_is_refused_naming_the_link() {
        let f = Fixture::new().await;
        let plain = f.node("project", "A plain project", json!({})).await;
        let parent = f.node("task", "Parent", json!({})).await;
        let issue = f.node("issue", "Fix it", json!({})).await;
        f.link(&parent, "has_child", &issue).await;

        let refused = f.folder_for(&issue).await.unwrap_err();
        assert_eq!(refused.code(), tonic::Code::FailedPrecondition);
        assert!(
            refused
                .message()
                .contains("not linked to a software project"),
            "{}",
            refused.message()
        );

        // A plain project does not satisfy it, on either route.
        f.link(&plain, "tasks", &issue).await;
        f.link(&plain, "tasks", &parent).await;
        let refused = f.folder_for(&issue).await.unwrap_err();
        assert!(
            refused
                .message()
                .contains("not linked to a software project"),
            "{}",
            refused.message()
        );
    }

    #[tokio::test]
    async fn a_plain_task_with_no_software_project_launches_in_the_private_folder() {
        let f = Fixture::new().await;
        let task = f.node("task", "Chore", json!({})).await;
        assert_eq!(f.folder_for(&task).await.unwrap(), None);
        // Nothing named at all is unchanged too.
        assert_eq!(
            f.handler.launch_folder(None, None, None).await.unwrap(),
            None
        );
    }
}
