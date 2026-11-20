//! The session: one agent process, one connection, one actor task that owns
//! both. The handle sends commands to the actor; the actor reads the agent,
//! bounds it, and reports typed events.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use nodespace_core::secrets::SecretStore;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::events::{
    bounded, clean, parse_permission, parse_stop_reason, parse_update, redact, AcpEvent, EndReason,
    Needles, PermissionOption, PermissionRequest, PermissionResolution,
};
use super::launch::AcpLaunch;
use super::wire::{write_message, AcpError, AcpLimits, Frame, LineReader};
use crate::local_agent::process::{
    credentials_within, reap, resolve_env_credentials, spawn_isolated, stop, EnvLookup, Isolated,
};
use crate::local_agent::redaction::{sanitize, MIN_REDACTABLE_BYTES};

/// The protocol version this client speaks.
const PROTOCOL_VERSION: u64 = 1;
/// Longest error text kept.
const ERROR_TEXT_BYTES: usize = 1500;
const MAX_SESSION_ID: usize = 256;
const MAX_AUTH_METHODS: usize = 16;
/// How long a closing agent has to exit by itself after its input is closed.
const CLOSE_GRACE: Duration = Duration::from_millis(500);
const READ_CHUNK: usize = 8192;
/// The agent's JSON-RPC code for "sign in first".
const AUTH_REQUIRED: i64 = -32000;

/// What the agent reported when the session was set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: String,
    pub agent_name: Option<String>,
    pub agent_version: Option<String>,
    /// The agent can reopen a stored session with history replay.
    pub load_session: bool,
    /// The agent can reopen a stored session without replay.
    pub resume: bool,
    /// The ids of the sign-in methods the agent lists (none is run).
    pub auth_methods: Vec<String>,
}

/// The user's answer to a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionChoice {
    /// One of the options the request offered.
    Select(String),
    /// None of them: the agent is told the request was cancelled.
    Cancel,
}

// ---------------------------------------------------------------------------
// messages
// ---------------------------------------------------------------------------

struct RpcError {
    code: i64,
    message: String,
}

enum Inbound {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: Value,
        result: Result<Value, RpcError>,
    },
}

fn usable_id(id: &Value) -> bool {
    match id {
        Value::Number(_) => true,
        Value::String(s) => s.len() <= MAX_SESSION_ID,
        _ => false,
    }
}

fn classify(value: Value, needles: &Needles) -> Option<Inbound> {
    let Value::Object(mut object) = value else {
        return None;
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return None;
    }
    let id = object.remove("id").filter(|v| !v.is_null());
    if let Some(id) = &id {
        if !usable_id(id) {
            return None;
        }
    }
    let params = object.remove("params").unwrap_or(Value::Null);
    match (object.remove("method"), id) {
        (Some(Value::String(method)), Some(id)) if method.len() <= 256 => {
            Some(Inbound::Request { id, method, params })
        }
        (Some(Value::String(method)), None) if method.len() <= 256 => {
            Some(Inbound::Notification { method, params })
        }
        (None, Some(id)) => match (object.remove("result"), object.remove("error")) {
            (Some(result), None) => Some(Inbound::Response {
                id,
                result: Ok(result),
            }),
            (None, Some(error)) => {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                // Cleaned before it is cut, so a credential is never halved.
                let text = error.get("message").and_then(Value::as_str).unwrap_or("");
                Some(Inbound::Response {
                    id,
                    result: Err(RpcError {
                        code,
                        message: clean(needles, text, ERROR_TEXT_BYTES),
                    }),
                })
            }
            _ => None,
        },
        _ => None,
    }
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn cancelled_outcome(id: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": {"outcome": "cancelled"}}})
}

// ---------------------------------------------------------------------------
// the connection: framing, bounds, refusals
// ---------------------------------------------------------------------------

struct Conn {
    stdin: ChildStdin,
    reader: LineReader<ChildStdout>,
    limits: AcpLimits,
    needles: Needles,
    noise_lines: u32,
    noise_bytes: usize,
    oversize: u32,
    /// Requests refused plus stray messages, reset each turn.
    unrelated: u32,
}

impl Conn {
    async fn send(&mut self, message: &Value) -> Result<(), AcpError> {
        write_message(&mut self.stdin, message, self.limits.write).await
    }

    fn note_unrelated(&mut self) -> Result<(), AcpError> {
        self.unrelated += 1;
        if self.unrelated > self.limits.agent_requests {
            return Err(AcpError::Limit(
                "the agent sent too many requests or messages that were not wanted".into(),
            ));
        }
        Ok(())
    }

    /// Answers a request this client does not serve.
    async fn refuse(&mut self, id: &Value, code: i64, message: &str) -> Result<(), AcpError> {
        self.note_unrelated()?;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
            .await
    }

    async fn refuse_method(&mut self, id: &Value) -> Result<(), AcpError> {
        self.refuse(
            id,
            -32601,
            "Method not found: this client serves no requests but permission requests",
        )
        .await
    }

    /// The next JSON-RPC message, with oversize lines dropped and noise bounded.
    /// `None` when the agent closed its output.
    async fn next_message(&mut self) -> Result<Option<Inbound>, AcpError> {
        loop {
            match self.reader.next().await {
                Err(_) => return Err(AcpError::Limit("the agent sent more than the limit".into())),
                Ok(Frame::Eof) => return Ok(None),
                Ok(Frame::Oversize) => {
                    self.oversize += 1;
                    if self.oversize > self.limits.max_oversize {
                        return Err(AcpError::Limit(
                            "the agent sent too many messages over the size limit".into(),
                        ));
                    }
                }
                Ok(Frame::Line(line)) => {
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    // Credentials are removed from the raw bytes, once, before
                    // anything is parsed.
                    let parsed = if self.needles.is_empty() {
                        serde_json::from_slice::<Value>(&line)
                    } else {
                        serde_json::from_slice::<Value>(
                            sanitize(&line, &self.needles, false).as_bytes(),
                        )
                    };
                    // The default recursion limit stays on: a deeply nested
                    // line is noise, not a message.
                    match parsed.ok().and_then(|v| classify(v, &self.needles)) {
                        Some(message) => return Ok(Some(message)),
                        None => {
                            self.noise_lines += 1;
                            self.noise_bytes += line.len();
                            if self.noise_lines > self.limits.noise_lines
                                || self.noise_bytes > self.limits.noise_bytes
                            {
                                return Err(AcpError::Limit(
                                    "the agent sent too much output that is not JSON-RPC".into(),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Waits for the answer to request `id`, refusing everything the agent asks.
    async fn await_response(&mut self, id: u64) -> Result<Result<Value, RpcError>, AcpError> {
        loop {
            match self.next_message().await? {
                None => return Err(AcpError::AgentExited),
                Some(Inbound::Request { id: rid, .. }) => self.refuse_method(&rid).await?,
                Some(Inbound::Notification { .. }) => self.note_unrelated()?,
                Some(Inbound::Response { id: rid, result }) => {
                    if rid == json!(id) {
                        return Ok(result);
                    }
                    self.note_unrelated()?;
                }
            }
        }
    }
}

fn rpc_failure(what: &str, error: RpcError) -> AcpError {
    if error.code == AUTH_REQUIRED {
        AcpError::AuthRequired(error.message)
    } else {
        AcpError::Protocol(format!(
            "{what} was answered with an error ({}): {}",
            error.code, error.message
        ))
    }
}

/// `initialize`, then `session/new`. No client capabilities are offered: the
/// agent uses its own file and shell tools.
async fn handshake(conn: &mut Conn, cwd: &str) -> Result<SessionInfo, AcpError> {
    conn.send(&request(
        1,
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false
            },
            "clientInfo": {
                "name": "nodespace",
                "title": "NodeSpace",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    ))
    .await?;
    let init = conn
        .await_response(1)
        .await?
        .map_err(|e| rpc_failure("initialize", e))?;
    if init.get("protocolVersion").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return Err(AcpError::Protocol(
            "the agent does not speak protocol version 1".into(),
        ));
    }
    let caps = init.get("agentCapabilities");
    let load_session = caps
        .and_then(|c| c.get("loadSession"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let resume = caps
        .and_then(|c| c.get("sessionCapabilities"))
        .and_then(|c| c.get("resume"))
        .is_some_and(|v| !v.is_null());
    let auth_methods = init
        .get("authMethods")
        .and_then(Value::as_array)
        .map(|methods| {
            methods
                .iter()
                .take(MAX_AUTH_METHODS)
                .filter_map(|m| bounded(m.get("id"), 128, &conn.needles))
                .collect()
        })
        .unwrap_or_default();
    let agent_name = bounded(
        init.get("agentInfo").and_then(|i| i.get("name")),
        128,
        &conn.needles,
    );
    let agent_version = bounded(
        init.get("agentInfo").and_then(|i| i.get("version")),
        128,
        &conn.needles,
    );

    conn.send(&request(
        2,
        "session/new",
        json!({"cwd": cwd, "mcpServers": []}),
    ))
    .await?;
    let created = conn
        .await_response(2)
        .await?
        .map_err(|e| rpc_failure("session/new", e))?;
    let session_id = created
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= MAX_SESSION_ID && !s.chars().any(char::is_control))
        .ok_or_else(|| AcpError::Protocol("the agent returned no usable session id".into()))?
        .to_string();
    Ok(SessionInfo {
        session_id,
        agent_name,
        agent_version,
        load_session,
        resume,
        auth_methods,
    })
}

// ---------------------------------------------------------------------------
// the actor
// ---------------------------------------------------------------------------

enum Command {
    Prompt {
        text: String,
        reply: oneshot::Sender<Result<(), AcpError>>,
    },
    Cancel {
        reply: oneshot::Sender<Result<(), AcpError>>,
    },
    Answer {
        request: u64,
        choice: PermissionChoice,
        reply: oneshot::Sender<Result<(), AcpError>>,
    },
    Close,
}

struct Turn {
    id: u64,
    started: Instant,
    last_input: Instant,
    bytes_at_start: u64,
    events: u32,
    permissions: u32,
    cancel_deadline: Option<Instant>,
}

struct Pending {
    agent_id: Value,
    options: Vec<PermissionOption>,
    deadline: Instant,
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Actor {
    conn: Conn,
    process: Isolated,
    _stderr: AbortOnDrop,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<AcpEvent>,
    session_id: String,
    next_id: u64,
    next_request: u64,
    turn: Option<Turn>,
    pending: HashMap<u64, Pending>,
}

fn fault(error: AcpError) -> EndReason {
    match error {
        AcpError::AgentExited => EndReason::AgentExited,
        other => EndReason::Fault(other.to_string()),
    }
}

impl Actor {
    async fn emit(&mut self, event: AcpEvent) -> Result<(), EndReason> {
        match tokio::time::timeout(self.conn.limits.consumer_stall, self.events.send(event)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(EndReason::Closed),
            Err(_) => Err(EndReason::Fault(
                "the reader of the session's events stopped reading".into(),
            )),
        }
    }

    /// The idle clock restarts when a waiting permission request is resolved:
    /// the agent gets the idle period to act on the answer.
    fn touch(&mut self) {
        if let Some(turn) = &mut self.turn {
            turn.last_input = Instant::now();
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let limits = &self.conn.limits;
        let mut deadlines: Vec<Instant> = self.pending.values().map(|p| p.deadline).collect();
        if let Some(turn) = &self.turn {
            deadlines.push(turn.started + limits.turn);
            if self.pending.is_empty() {
                deadlines.push(turn.last_input + limits.idle);
            }
            deadlines.extend(turn.cancel_deadline);
        }
        deadlines.into_iter().min()
    }

    async fn run(mut self) {
        let reason = self.main_loop().await;
        self.finish(reason).await;
    }

    async fn main_loop(&mut self) -> EndReason {
        loop {
            let ceiling = match &self.turn {
                Some(t) => t
                    .bytes_at_start
                    .saturating_add(self.conn.limits.turn_bytes)
                    .min(self.conn.limits.session_bytes),
                None => self.conn.limits.session_bytes,
            };
            self.conn.reader.set_ceiling(ceiling);
            let deadline = self.next_deadline();
            let sleeper = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            };
            let outcome = tokio::select! {
                command = self.commands.recv() => match command {
                    // The handle is gone: nobody can answer anything.
                    None => Err(EndReason::Closed),
                    Some(command) => self.on_command(command).await,
                },
                message = self.conn.next_message() => match message {
                    Err(e) => Err(fault(e)),
                    Ok(None) => Err(EndReason::AgentExited),
                    Ok(Some(message)) => {
                        if let Some(turn) = &mut self.turn {
                            turn.last_input = Instant::now();
                        }
                        self.on_message(message).await
                    }
                },
                _ = sleeper => self.on_timers().await,
            };
            if let Err(reason) = outcome {
                return reason;
            }
        }
    }

    async fn write(&mut self, message: &Value) -> Result<(), EndReason> {
        self.conn.send(message).await.map_err(fault)
    }

    async fn on_command(&mut self, command: Command) -> Result<(), EndReason> {
        match command {
            Command::Prompt { text, reply } => {
                if self.turn.is_some() {
                    let _ = reply.send(Err(AcpError::Busy));
                    return Ok(());
                }
                if text.len() > self.conn.limits.prompt_bytes {
                    let _ = reply.send(Err(AcpError::PromptTooLarge));
                    return Ok(());
                }
                self.next_id += 1;
                let id = self.next_id;
                let message = request(
                    id,
                    "session/prompt",
                    json!({
                        "sessionId": self.session_id,
                        "prompt": [{"type": "text", "text": text}]
                    }),
                );
                if let Err(e) = self.conn.send(&message).await {
                    let _ = reply.send(Err(e.clone()));
                    return Err(fault(e));
                }
                let now = Instant::now();
                self.conn.unrelated = 0;
                self.turn = Some(Turn {
                    id,
                    started: now,
                    last_input: now,
                    bytes_at_start: self.conn.reader.bytes_read(),
                    events: 0,
                    permissions: 0,
                    cancel_deadline: None,
                });
                let _ = reply.send(Ok(()));
                Ok(())
            }
            Command::Cancel { reply } => {
                let result = self.cancel().await;
                let _ = reply.send(Ok(()));
                result
            }
            Command::Answer {
                request,
                choice,
                reply,
            } => {
                let Some(pending) = self.pending.get(&request) else {
                    let _ = reply.send(Err(AcpError::UnknownPermission));
                    return Ok(());
                };
                let (outcome, resolution) = match &choice {
                    PermissionChoice::Cancel => (
                        json!({"outcome": "cancelled"}),
                        PermissionResolution::Cancelled,
                    ),
                    PermissionChoice::Select(option_id) => {
                        if !pending.options.iter().any(|o| &o.option_id == option_id) {
                            let _ = reply.send(Err(AcpError::InvalidOption));
                            return Ok(());
                        }
                        (
                            json!({"outcome": "selected", "optionId": option_id}),
                            PermissionResolution::Answered,
                        )
                    }
                };
                let pending = self.pending.remove(&request).expect("checked above");
                self.touch();
                let _ = reply.send(Ok(()));
                self.write(&json!({"jsonrpc": "2.0", "id": pending.agent_id, "result": {"outcome": outcome}}))
                    .await?;
                self.emit(AcpEvent::PermissionResolved {
                    request,
                    resolution,
                })
                .await
            }
            Command::Close => Err(EndReason::Closed),
        }
    }

    /// Tells the agent to stop the turn, and answers every waiting permission
    /// request as cancelled. The agent has the grace period to end the turn.
    async fn cancel(&mut self) -> Result<(), EndReason> {
        let grace = self.conn.limits.cancel_grace;
        let Some(turn) = &mut self.turn else {
            return Ok(());
        };
        if turn.cancel_deadline.is_some() {
            return Ok(());
        }
        turn.cancel_deadline = Some(Instant::now() + grace);
        let message = json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": {"sessionId": self.session_id}
        });
        self.write(&message).await?;
        let waiting: Vec<(u64, Pending)> = self.pending.drain().collect();
        for (request, pending) in waiting {
            self.write(&cancelled_outcome(&pending.agent_id)).await?;
            self.emit(AcpEvent::PermissionResolved {
                request,
                resolution: PermissionResolution::Cancelled,
            })
            .await?;
        }
        Ok(())
    }

    async fn on_timers(&mut self) -> Result<(), EndReason> {
        let now = Instant::now();
        let limits = self.conn.limits;
        if let Some(turn) = &self.turn {
            if now >= turn.started + limits.turn {
                return Err(EndReason::Fault(
                    AcpError::Timeout("the turn", limits.turn.as_secs()).to_string(),
                ));
            }
            if self.pending.is_empty() && now >= turn.last_input + limits.idle {
                return Err(EndReason::Fault(
                    AcpError::Timeout("anything during the turn", limits.idle.as_secs())
                        .to_string(),
                ));
            }
            if turn.cancel_deadline.is_some_and(|d| now >= d) {
                return Err(EndReason::Fault(
                    "the agent did not end the turn after it was cancelled".into(),
                ));
            }
        }
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.deadline)
            .map(|(k, _)| *k)
            .collect();
        for request in expired {
            if let Some(pending) = self.pending.remove(&request) {
                self.touch();
                self.write(&cancelled_outcome(&pending.agent_id)).await?;
                self.emit(AcpEvent::PermissionResolved {
                    request,
                    resolution: PermissionResolution::TimedOut,
                })
                .await?;
            }
        }
        Ok(())
    }

    async fn on_message(&mut self, message: Inbound) -> Result<(), EndReason> {
        match message {
            Inbound::Response { id, result } => {
                let is_turn = self.turn.as_ref().is_some_and(|t| id == json!(t.id));
                if !is_turn {
                    return self.conn.note_unrelated().map_err(fault);
                }
                self.end_turn(result).await
            }
            Inbound::Request { id, method, params } => {
                if method == "session/request_permission" {
                    self.on_permission(id, &params).await
                } else {
                    self.conn.refuse_method(&id).await.map_err(fault)
                }
            }
            Inbound::Notification { method, params } => {
                if method != "session/update" {
                    return self.conn.note_unrelated().map_err(fault);
                }
                match &mut self.turn {
                    Some(turn) => {
                        turn.events += 1;
                        if turn.events > self.conn.limits.turn_events {
                            return Err(EndReason::Fault(
                                "the agent sent more updates than the limit for one turn".into(),
                            ));
                        }
                    }
                    None => self.conn.note_unrelated().map_err(fault)?,
                }
                match parse_update(&params, &self.session_id, &self.conn.needles) {
                    Some(update) => self.emit(AcpEvent::Update(update)).await,
                    None => Ok(()),
                }
            }
        }
    }

    async fn end_turn(&mut self, result: Result<Value, RpcError>) -> Result<(), EndReason> {
        let waiting: Vec<u64> = self.pending.drain().map(|(k, _)| k).collect();
        for request in waiting {
            self.emit(AcpEvent::PermissionResolved {
                request,
                resolution: PermissionResolution::Dropped,
            })
            .await?;
        }
        self.turn = None;
        self.conn.unrelated = 0;
        match result {
            Ok(value) => {
                let Some(reason) = value.get("stopReason").and_then(Value::as_str) else {
                    return Err(EndReason::Fault(
                        "the answer to the prompt has no stop reason".into(),
                    ));
                };
                let stop_reason = parse_stop_reason(&clean(&self.conn.needles, reason, 128));
                self.emit(AcpEvent::TurnEnded { stop_reason }).await
            }
            Err(error) => {
                self.emit(AcpEvent::TurnFailed {
                    code: error.code,
                    message: error.message,
                    auth_required: error.code == AUTH_REQUIRED,
                })
                .await
            }
        }
    }

    async fn on_permission(&mut self, id: Value, params: &Value) -> Result<(), EndReason> {
        let limits = self.conn.limits;
        let Some(turn) = &mut self.turn else {
            return self.reply_cancelled(&id).await;
        };
        turn.permissions += 1;
        if turn.permissions > limits.turn_permissions {
            return Err(EndReason::Fault(
                "the agent sent more permission requests than the limit for one turn".into(),
            ));
        }
        if turn.cancel_deadline.is_some() || self.pending.len() >= limits.pending_permissions {
            return self.reply_cancelled(&id).await;
        }
        let (tool_call_id, title, kind, options) =
            match parse_permission(params, &self.session_id, &self.conn.needles) {
                Ok(parsed) => parsed,
                Err(why) => return self.conn.refuse(&id, -32602, why).await.map_err(fault),
            };
        if options.is_empty() {
            return self.reply_cancelled(&id).await;
        }
        self.next_request += 1;
        let request = self.next_request;
        self.pending.insert(
            request,
            Pending {
                agent_id: id,
                options: options.clone(),
                deadline: Instant::now() + limits.permission,
            },
        );
        self.emit(AcpEvent::PermissionRequest(PermissionRequest {
            request,
            tool_call_id,
            title,
            kind,
            options,
        }))
        .await
    }

    async fn reply_cancelled(&mut self, id: &Value) -> Result<(), EndReason> {
        self.conn.note_unrelated().map_err(fault)?;
        self.write(&cancelled_outcome(id)).await
    }

    /// Stops the process and everything it started, and says the session ended.
    async fn finish(mut self, reason: EndReason) {
        if matches!(reason, EndReason::Closed) {
            let _ = self.conn.stdin.shutdown().await;
            let _ = tokio::time::timeout(CLOSE_GRACE, self.process.child.wait()).await;
        }
        stop(&mut self.process.child);
        reap(&mut self.process.child).await;
        if matches!(self.process.child.try_wait(), Ok(Some(_))) {
            self.process.guard.disarm();
        }
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            self.events.send(AcpEvent::SessionEnded(reason)),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// the handle
// ---------------------------------------------------------------------------

/// A running session with one agent. Dropping the handle stops the agent.
pub struct AcpSession {
    commands: mpsc::Sender<Command>,
    task: Option<JoinHandle<()>>,
    info: SessionInfo,
    needles: Needles,
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl AcpSession {
    /// Starts the agent, runs `initialize` and `session/new`, and returns the
    /// session with its event stream. Everything before the first prompt,
    /// the credential lookup included, is inside `limits.startup`.
    pub async fn start(
        launch: &AcpLaunch,
        secrets: &Arc<SecretStore>,
        daemon_env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
        limits: AcpLimits,
    ) -> Result<(Self, mpsc::Receiver<AcpEvent>), AcpError> {
        launch.validate()?;
        let lookup: EnvLookup = Arc::new(daemon_env);
        let (mut credentials, left) = credentials_within(
            limits.startup,
            resolve_env_credentials(secrets, &lookup, &launch.secrets),
        )
        .await
        .map_err(|e| AcpError::Credential(e.0))?;
        for (name, value) in &launch.literal_env {
            credentials.push((name.clone(), Zeroizing::new(value.clone())));
        }
        for name in &launch.daemon_env {
            if let Some(value) = lookup(name) {
                credentials.push((name.clone(), Zeroizing::new(value)));
            }
        }
        let needles: Needles = Arc::new(
            credentials
                .iter()
                .filter(|(_, v)| v.len() >= MIN_REDACTABLE_BYTES)
                .map(|(_, v)| v.clone())
                .collect(),
        );
        let mut process = spawn_isolated(
            &launch.program,
            &launch.args,
            &lookup,
            &credentials,
            Stdio::piped(),
        )
        .map_err(|e| AcpError::Launch(e.0))?;
        drop(credentials);
        let cwd = process.working_dir().to_string_lossy().into_owned();
        let stdin = process.child.stdin.take().expect("stdin is piped");
        let stdout = process.child.stdout.take().expect("stdout is piped");
        let mut stderr = process.child.stderr.take().expect("stderr is piped");
        // Read so the agent never blocks on a full pipe, and discarded.
        let drain = AbortOnDrop(tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut chunk = [0u8; READ_CHUNK];
            while matches!(stderr.read(&mut chunk).await, Ok(n) if n > 0) {}
        }));
        let mut conn = Conn {
            stdin,
            reader: LineReader::new(stdout, limits.message_bytes),
            limits,
            needles: Arc::clone(&needles),
            noise_lines: 0,
            noise_bytes: 0,
            oversize: 0,
            unrelated: 0,
        };
        conn.reader.set_ceiling(limits.session_bytes);
        let info = match tokio::time::timeout(left, handshake(&mut conn, &cwd)).await {
            Ok(Ok(info)) => info,
            failed => {
                stop(&mut process.child);
                reap(&mut process.child).await;
                if matches!(process.child.try_wait(), Ok(Some(_))) {
                    process.guard.disarm();
                }
                return Err(match failed {
                    Ok(Err(e)) => e,
                    _ => AcpError::Timeout("starting", limits.startup.as_secs()),
                });
            }
        };
        let (command_tx, commands) = mpsc::channel(16);
        let (events_tx, events_rx) = mpsc::channel(1024);
        let actor = Actor {
            conn,
            process,
            _stderr: drain,
            commands,
            events: events_tx,
            session_id: info.session_id.clone(),
            next_id: 2,
            next_request: 0,
            turn: None,
            pending: HashMap::new(),
        };
        let task = tokio::spawn(actor.run());
        Ok((
            Self {
                commands: command_tx,
                task: Some(task),
                info,
                needles,
            },
            events_rx,
        ))
    }

    pub fn info(&self) -> &SessionInfo {
        &self.info
    }

    /// `text` with any credential the agent was given removed.
    ///
    /// BINDING for every consumer of the event stream: a credential split
    /// across two streamed chunks is invisible to the per-line and per-string
    /// passes, so text assembled from several updates MUST be passed through
    /// this before it is stored or shown, and raw updates must never be logged
    /// or stored.
    pub fn redact(&self, text: &str) -> String {
        redact(&self.needles, text)
    }

    async fn ask(
        &self,
        build: impl FnOnce(oneshot::Sender<Result<(), AcpError>>) -> Command,
    ) -> Result<(), AcpError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| AcpError::Closed)?;
        answer.await.map_err(|_| AcpError::Closed)?
    }

    /// Starts a prompt turn. Its updates and its end arrive as events.
    pub async fn prompt(&self, text: &str) -> Result<(), AcpError> {
        let text = text.to_string();
        self.ask(|reply| Command::Prompt { text, reply }).await
    }

    /// Cancels the running turn (nothing happens when none runs).
    pub async fn cancel(&self) -> Result<(), AcpError> {
        self.ask(|reply| Command::Cancel { reply }).await
    }

    pub async fn answer_permission(
        &self,
        request: u64,
        choice: PermissionChoice,
    ) -> Result<(), AcpError> {
        self.ask(|reply| Command::Answer {
            request,
            choice,
            reply,
        })
        .await
    }

    /// Ends the session and waits (a bounded time) until the agent is stopped.
    pub async fn close(mut self) {
        let _ = self.commands.send(Command::Close).await;
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(Duration::from_secs(10), &mut task)
                .await
                .is_err()
            {
                // Dropping the actor's future runs the guard that kills the group.
                task.abort();
            }
        }
    }
}
