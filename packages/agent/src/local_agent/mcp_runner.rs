//! The `tool-mcp` runner: calls ONE tool, named in the node, on an MCP server
//! over stdio or streamable HTTP.
//!
//! What this runner trusts, and what it does not:
//!
//! - **The node is the contract.** The tool the model sees (name, description,
//!   parameter schema) and the one tool called (`remote_tool_name`) come from
//!   the node and are covered by the device approval. `tools/list` is never
//!   asked for; whatever the server says about its own tools is not used. The
//!   arguments are checked against the node's closed `parameter_schema` before
//!   anything is sent.
//! - **The server is not trusted.** No client capability is advertised
//!   (`capabilities: {}`: no sampling, roots, elicitation, filesystem), and a
//!   request the server makes of the client is answered with a JSON-RPC error
//!   (`ping` is answered with `{}`). Messages are bounded one by one and in
//!   total, unrelated messages and notifications are counted and bounded, a
//!   batch is refused, and the JSON parser keeps its default recursion limit.
//! - **A result is data.** Only `text` content is returned (the host fences it
//!   as external); image, audio and resource content is replaced by a size
//!   note; `isError` is a failure; structured content is used only when there is
//!   no text, and bounded like the rest.
//! - **One server per call.** A stdio server is started, initialized, asked
//!   once and killed (its process group, on every exit path including the call
//!   being dropped). An HTTP session is opened, used once and deleted.
//!
//! The process rules (no shell, a cleared and rebuilt environment, a private
//! working directory, the group kill, credentials only in the child's
//! environment) are [`super::process`]'s; the network rules (https except named
//! loopback, resolve and pin, every address checked, same-origin redirects, no
//! proxy, cookies or decoding, the credential only to the bound origin) are
//! [`super::net_policy`]'s. Credential values are removed from everything the
//! server sends before any of it is returned. The server's stderr is read and
//! discarded: it is never shown to the user or the model.
//!
//! Supported protocol versions: `2025-06-18` and `2025-03-26` on both
//! transports, and `2024-11-05` on stdio (its HTTP form was a different
//! transport, which is not implemented).
//!
//! Limits worth knowing: the arguments are checked against the node's closed
//! schema at the TOP LEVEL only (`validate_arguments`); the shape of nested
//! values is NOT validated, so the server must not be trusted with it. The same
//! origin may be redirected to with a `307`/`308` (the POST is repeated; the
//! credential goes only to that origin). An event stream that ends events with a
//! bare carriage return (no `\n`) is never split into events and fails closed at
//! the message limit. Events that carry nothing (comments, retries, empty data)
//! count against the same bound as non-JSON noise (64). A server that leaves a
//! grandchild holding stderr (outside the killed process group) is not waited
//! for: the stderr reader is detached and ends when the pipe closes.
//!
//! Every MCP call needs the user's confirmation, so (as for `tool-cli`) nothing
//! is offered to the model until a confirmer is installed.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nodespace_core::secrets::SecretStore;
use nodespace_types::{ToolMcpFields, DEFAULT_RESPONSE_BYTES, DEFAULT_TIMEOUT_SECONDS};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::Method;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, ChildStdout};
use url::Url;
use zeroize::Zeroizing;

use super::net_policy::{
    checked_address, credential_note, destination, fail, http_credential, request_error,
    send_following, FixedResolver, HostResolver, Outgoing, ResolvedPin, SystemResolver,
    CONNECT_TIMEOUT,
};
use super::process::{
    check_program_file, credentials_within, environment_note, program_note, reap,
    resolve_env_credentials, spawn_isolated, stop, EnvLookup, Isolated,
};
use super::redaction::{cut, sanitize};
use super::remote_tools::{
    validate_arguments, Confirmation, RemoteCallError, RemoteOutput, RemoteTool, RemoteToolKind,
    RemoteToolRunner,
};

/// The version this client asks for.
const REQUESTED_VERSION: &str = "2025-06-18";
/// Versions accepted over stdio.
const STDIO_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// Versions accepted over HTTP.
const HTTP_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26"];
/// Most messages from the server that are not the answer awaited (notifications,
/// requests, answers to other ids) before the call ends.
const MAX_UNRELATED: usize = 64;
/// Most lines (and bytes) of non-JSON output ignored on stdout.
const MAX_NOISE_LINES: usize = 64;
const MAX_NOISE_BYTES: usize = 64 * 1024;
/// Smallest and largest size of one message from the server.
const MESSAGE_FLOOR: usize = 64 * 1024;
const MESSAGE_CEILING: usize = 4 * 1024 * 1024;
/// Most content items looked at in a result.
const MAX_ITEMS: usize = 256;
/// How much of a failure's text a message carries.
const ERROR_TEXT_BYTES: usize = 1500;
/// How long the cancel notification and the session delete may take.
const CLEANUP_LIMIT: Duration = Duration::from_secs(2);
/// Longest `Mcp-Session-Id` accepted.
const MAX_SESSION_ID: usize = 256;
/// Read size.
const READ_CHUNK: usize = 8192;
/// Longest confirmation text and the shares of it.
const MAX_DESTINATION_BYTES: usize = 4096;
const MAX_ARGV_SHOWN: usize = 300;
const MAX_TOOL_SHOWN: usize = 128;
const MAX_PATH_SHOWN: usize = 1000;

/// Runs MCP tool calls.
pub struct McpRunner {
    secrets: Arc<SecretStore>,
    daemon_env: EnvLookup,
    resolver: Arc<dyn HostResolver>,
    pinned_lookups: Arc<AtomicUsize>,
}

impl McpRunner {
    pub fn new(secrets: Arc<SecretStore>) -> Self {
        Self {
            secrets,
            daemon_env: Arc::new(|name| std::env::var(name).ok()),
            resolver: Arc::new(SystemResolver),
            pinned_lookups: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A runner with its own resolver and view of the environment, for tests.
    pub fn with_parts(
        secrets: Arc<SecretStore>,
        lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
        resolver: Arc<dyn HostResolver>,
    ) -> Self {
        Self {
            secrets,
            daemon_env: Arc::new(lookup),
            resolver,
            pinned_lookups: Arc::new(AtomicUsize::new(0)),
        }
    }
}

fn read_binding(tool: &RemoteTool) -> Result<ToolMcpFields, RemoteCallError> {
    let fields = ToolMcpFields::from_bucket(tool.binding())
        .map_err(|e| RemoteCallError(format!("the tool's binding is not valid: {}", e.0)))?;
    fields
        .check_complete(Some(&tool.parameter_schema))
        .map_err(|e| RemoteCallError(format!("the tool's binding is not complete: {}", e.0)))?;
    Ok(fields)
}

/// The arguments as the object `tools/call` carries.
fn arguments(tool: &RemoteTool, args: &Value) -> Result<Value, RemoteCallError> {
    validate_arguments(&tool.parameter_schema, args)
        .map_err(|e| RemoteCallError(format!("the arguments are not valid: {e}")))?;
    Ok(match args {
        Value::Null => json!({}),
        other => other.clone(),
    })
}

// ---------------------------------------------------------------------------
// The protocol, over any wire
// ---------------------------------------------------------------------------

/// How messages move. The driver does not know which transport it is on.
#[async_trait]
trait Wire: Send {
    /// Sends one message. `awaits_reply` is true for a request: over HTTP its
    /// answer comes on the response to this send.
    async fn send(&mut self, message: &Value, awaits_reply: bool) -> Result<(), RemoteCallError>;
    /// The next message from the server, or `None` when it has no more.
    async fn next(&mut self) -> Result<Option<Value>, RemoteCallError>;
    /// The version both sides agreed on.
    fn negotiated(&mut self, _version: &str) {}
}

/// Limits on what a server may send.
#[derive(Clone, Copy)]
struct Limits {
    message: usize,
    total: usize,
}

impl Limits {
    fn for_cap(cap: usize) -> Self {
        let message = cap.saturating_mul(2).clamp(MESSAGE_FLOOR, MESSAGE_CEILING);
        Self {
            message,
            total: message.saturating_mul(2) + MESSAGE_FLOOR,
        }
    }
}

/// What has been read so far and how much of it was noise.
#[derive(Default)]
struct Tally {
    total: usize,
    noise_lines: usize,
    noise_bytes: usize,
}

impl Tally {
    fn read(&mut self, n: usize, limits: Limits) -> Result<(), RemoteCallError> {
        self.total += n;
        if self.total > limits.total {
            return fail("the server sent more than the limit for one call");
        }
        Ok(())
    }
    fn noise(&mut self, bytes: usize) -> Result<(), RemoteCallError> {
        self.noise_lines += 1;
        self.noise_bytes += bytes;
        if self.noise_lines > MAX_NOISE_LINES || self.noise_bytes > MAX_NOISE_BYTES {
            return fail("the server sent too much output that is not JSON-RPC");
        }
        Ok(())
    }
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn notification(method: &str, params: Option<Value>) -> Value {
    match params {
        Some(p) => json!({"jsonrpc": "2.0", "method": method, "params": p}),
        None => json!({"jsonrpc": "2.0", "method": method}),
    }
}

/// Waits for the answer to request `id`, refusing what the server asks of the
/// client and bounding everything else it says.
async fn await_response(wire: &mut dyn Wire, id: u64) -> Result<Value, RemoteCallError> {
    let mut unrelated = 0usize;
    loop {
        let Some(message) = wire.next().await? else {
            return fail("the server ended before it answered");
        };
        let Value::Object(object) = &message else {
            return fail("the server sent a batch or a non-object message, which is refused");
        };
        let method = object.get("method").and_then(Value::as_str);
        let message_id = object.get("id").filter(|v| !v.is_null());
        match (method, message_id) {
            // A request from the server: none is served.
            (Some(method), Some(request_id)) => {
                unrelated += 1;
                let reply = if method == "ping" {
                    json!({"jsonrpc": "2.0", "id": request_id, "result": {}})
                } else {
                    json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {"code": -32601, "message": "Method not found: this client serves no requests"}
                    })
                };
                wire.send(&reply, false).await?;
            }
            // A notification: ignored, and bounded.
            (Some(_), None) => unrelated += 1,
            (None, Some(answer_id)) => {
                if *answer_id == json!(id) {
                    if let Some(error) = object.get("error") {
                        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                        // Not cut here: a credential must be removed from the
                        // whole text (`scrub`) before the text is shortened.
                        let text = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        return fail(format!(
                            "the server answered with an error ({code}): {text}"
                        ));
                    }
                    return match object.get("result") {
                        Some(result) => Ok(result.clone()),
                        None => fail("the server's answer has neither a result nor an error"),
                    };
                }
                unrelated += 1;
            }
            (None, None) => unrelated += 1,
        }
        if unrelated > MAX_UNRELATED {
            return fail("the server sent too many messages that are not the answer");
        }
    }
}

/// Where a call got to, so a stopped call can be cancelled.
#[derive(Default)]
struct Progress {
    call_sent: bool,
}

/// initialize, initialized, one `tools/call`.
async fn converse(
    wire: &mut dyn Wire,
    versions: &[&str],
    remote_tool: &str,
    arguments: Value,
    progress: &mut Progress,
) -> Result<Value, RemoteCallError> {
    wire.send(
        &request(
            1,
            "initialize",
            json!({
                "protocolVersion": REQUESTED_VERSION,
                // No capabilities: the client serves no sampling, roots,
                // elicitation or filesystem.
                "capabilities": {},
                "clientInfo": {"name": "nodespace-agent", "version": env!("CARGO_PKG_VERSION")},
            }),
        ),
        true,
    )
    .await?;
    let init = await_response(wire, 1).await?;
    let version = init
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| RemoteCallError("the server named no protocol version".to_string()))?;
    if !versions.contains(&version) {
        return fail("the server uses a protocol version this client does not support");
    }
    wire.negotiated(version);
    wire.send(&notification("notifications/initialized", None), false)
        .await?;
    wire.send(
        &request(
            2,
            "tools/call",
            json!({"name": remote_tool, "arguments": arguments}),
        ),
        true,
    )
    .await?;
    progress.call_sent = true;
    await_response(wire, 2).await
}

/// A `tools/call` result as text. Only text content is returned; the rest is
/// replaced by a note that says what was left out and how big it was.
fn render(result: &Value, needles: &[Zeroizing<String>]) -> (String, bool) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut parts: Vec<String> = Vec::new();
    if let Some(items) = result.get("content").and_then(Value::as_array) {
        for item in items.iter().take(MAX_ITEMS) {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                    }
                }
                Some(kind @ ("image" | "audio")) => {
                    let size = item.get("data").and_then(Value::as_str).map_or(0, str::len);
                    parts.push(format!("[{kind} content omitted: {size} bytes]"));
                }
                Some("resource" | "resource_link") => {
                    parts.push("[resource content omitted]".to_string())
                }
                _ => parts.push("[unsupported content omitted]".to_string()),
            }
        }
        if items.len() > MAX_ITEMS {
            parts.push(format!(
                "[{} more content items omitted]",
                items.len() - MAX_ITEMS
            ));
        }
    }
    if parts.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            // Credentials are removed from the string leaves (and keys) first:
            // once serialised, a value with a quote or a backslash in it is
            // escaped and would no longer match.
            let mut clean = structured.clone();
            redact_value(&mut clean, needles);
            parts.push(serde_json::to_string(&clean).unwrap_or_default());
        }
    }
    (parts.join("\n"), is_error)
}

/// Removes credential values from every string in `value`, keys included.
fn redact_value(value: &mut Value, needles: &[Zeroizing<String>]) {
    match value {
        Value::String(s) => *s = sanitize(s.as_bytes(), needles, false),
        Value::Array(items) => items.iter_mut().for_each(|v| redact_value(v, needles)),
        Value::Object(map) => {
            let old = std::mem::take(map);
            for (key, mut v) in old {
                redact_value(&mut v, needles);
                map.insert(sanitize(key.as_bytes(), needles, false), v);
            }
        }
        _ => {}
    }
}

/// The final text: credential values removed, cut to `cap`.
fn finish(
    result: &Value,
    needles: &[Zeroizing<String>],
    cap: usize,
) -> Result<RemoteOutput, RemoteCallError> {
    let (text, is_error) = render(result, needles);
    let mut text = sanitize(text.as_bytes(), needles, false);
    if is_error {
        cut(&mut text, ERROR_TEXT_BYTES);
        return fail(format!("the tool reported an error: {text}"));
    }
    let truncated = text.len() > cap;
    cut(&mut text, cap);
    Ok(RemoteOutput { text, truncated })
}

/// A failure's message with credential values removed (the server's own words
/// can carry them).
fn scrub(error: RemoteCallError, needles: &[Zeroizing<String>]) -> RemoteCallError {
    let mut text = sanitize(error.0.as_bytes(), needles, false);
    // Shortened only after the credential is gone.
    cut(&mut text, ERROR_TEXT_BYTES + 200);
    RemoteCallError(text)
}

// ---------------------------------------------------------------------------
// stdio
// ---------------------------------------------------------------------------

struct StdioWire {
    stdin: ChildStdin,
    stdout: ChildStdout,
    buffer: Vec<u8>,
    limits: Limits,
    tally: Tally,
}

#[async_trait]
impl Wire for StdioWire {
    async fn send(&mut self, message: &Value, _awaits_reply: bool) -> Result<(), RemoteCallError> {
        let mut line = serde_json::to_vec(message).unwrap_or_default();
        line.push(b'\n');
        let gone = |_| RemoteCallError("the server stopped reading its input".to_string());
        self.stdin.write_all(&line).await.map_err(gone)?;
        self.stdin.flush().await.map_err(gone)
    }

    async fn next(&mut self) -> Result<Option<Value>, RemoteCallError> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = self.buffer.drain(..=end).collect();
                line.pop();
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                // The default recursion limit stays on: a deeply nested line
                // is not JSON-RPC to this client, it is noise.
                match serde_json::from_slice::<Value>(&line) {
                    Ok(value) => return Ok(Some(value)),
                    Err(_) => {
                        self.tally.noise(line.len())?;
                        continue;
                    }
                }
            }
            if self.buffer.len() > self.limits.message {
                return fail("a message from the server was larger than the limit");
            }
            let mut chunk = [0u8; READ_CHUNK];
            let n = self.stdout.read(&mut chunk).await.unwrap_or(0);
            if n == 0 {
                return Ok(None);
            }
            self.tally.read(n, self.limits)?;
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }
}

// ---------------------------------------------------------------------------
// streamable HTTP
// ---------------------------------------------------------------------------

/// The server's side of one request's answer.
enum Incoming {
    Messages(Vec<Value>),
    Sse {
        response: reqwest::Response,
        buffer: Vec<u8>,
        done: bool,
    },
}

struct HttpWire<'a> {
    url: Url,
    base: HeaderMap,
    out: Outgoing<'a>,
    session: Option<String>,
    version: Option<String>,
    incoming: Option<Incoming>,
    limits: Limits,
    tally: Tally,
}

fn is_json_type(value: &str) -> bool {
    let kind = value.split(';').next().unwrap_or("").trim();
    kind.eq_ignore_ascii_case("application/json")
}

fn is_sse_type(value: &str) -> bool {
    let kind = value.split(';').next().unwrap_or("").trim();
    kind.eq_ignore_ascii_case("text/event-stream")
}

impl HttpWire<'_> {
    fn headers(&self) -> Result<HeaderMap, RemoteCallError> {
        let mut headers = self.base.clone();
        if let Some(version) = &self.version {
            if let Ok(v) = HeaderValue::from_str(version) {
                headers.insert(HeaderName::from_static("mcp-protocol-version"), v);
            }
        }
        if let Some(session) = &self.session {
            let v = HeaderValue::from_str(session)
                .map_err(|_| RemoteCallError("the session id is not usable".to_string()))?;
            headers.insert(HeaderName::from_static("mcp-session-id"), v);
        }
        Ok(headers)
    }

    /// Reads a whole JSON body, bounded.
    async fn body(&mut self, mut response: reqwest::Response) -> Result<Vec<u8>, RemoteCallError> {
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    self.tally.read(chunk.len(), self.limits)?;
                    bytes.extend_from_slice(&chunk);
                    if bytes.len() > self.limits.message {
                        return fail("a message from the server was larger than the limit");
                    }
                }
                Ok(None) => return Ok(bytes),
                Err(e) => return Err(request_error(e, self.out.limit, self.out.needles)),
            }
        }
    }
}

/// One server-sent event out of `buffer`, if a whole one is there: its `data`.
/// Events of a type other than `message` are skipped.
fn take_event(buffer: &mut Vec<u8>) -> Option<Option<String>> {
    // An event ends at a blank line.
    let text_end = {
        let mut i = 0;
        let mut found = None;
        while i < buffer.len() {
            if buffer[i] == b'\n' {
                if buffer.get(i + 1) == Some(&b'\n') {
                    found = Some((i, i + 2));
                    break;
                }
                if buffer.get(i + 1) == Some(&b'\r') && buffer.get(i + 2) == Some(&b'\n') {
                    found = Some((i, i + 3));
                    break;
                }
            }
            i += 1;
        }
        found
    }?;
    let raw: Vec<u8> = buffer.drain(..text_end.1).collect();
    let raw = String::from_utf8_lossy(&raw[..text_end.0]).into_owned();
    let mut data: Vec<&str> = Vec::new();
    let mut kind = "message";
    for line in raw.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => data.push(value),
            "event" => kind = value,
            _ => {}
        }
    }
    let data = data.join("\n");
    if kind != "message" || data.is_empty() {
        return Some(None);
    }
    Some(Some(data))
}

#[async_trait]
impl Wire for HttpWire<'_> {
    async fn send(&mut self, message: &Value, awaits_reply: bool) -> Result<(), RemoteCallError> {
        let mut headers = self.headers()?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        let body = serde_json::to_vec(message).unwrap_or_default();
        let response = send_following(
            &self.out,
            Method::POST,
            self.url.clone(),
            &headers,
            Some(&body),
        )
        .await?;
        let status = response.status();
        if !awaits_reply {
            // A notification or a response: accepted, and nothing is read.
            if !(status.is_success()) {
                return fail(format!("the server answered HTTP {status}"));
            }
            return Ok(());
        }
        if status.as_u16() == 404 && self.session.is_some() {
            return fail("the server no longer knows the session");
        }
        if status.as_u16() != 200 {
            return fail(format!("the server answered HTTP {status}"));
        }
        if self.session.is_none() {
            if let Some(id) = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
            {
                if id.is_empty()
                    || id.len() > MAX_SESSION_ID
                    || !id.bytes().all(|b| (0x21..=0x7e).contains(&b))
                {
                    return fail("the server sent a session id that is not valid");
                }
                self.session = Some(id.to_string());
            }
        }
        let kind = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if is_sse_type(&kind) {
            self.incoming = Some(Incoming::Sse {
                response,
                buffer: Vec::new(),
                done: false,
            });
        } else if is_json_type(&kind) {
            let bytes = self.body(response).await?;
            // A deeply nested body is refused by the parser's recursion limit.
            let value = serde_json::from_slice::<Value>(&bytes).map_err(|_| {
                RemoteCallError("the server's answer is not valid JSON".to_string())
            })?;
            self.incoming = Some(Incoming::Messages(vec![value]));
        } else {
            return fail(
                "the server answered with a content type that is neither JSON nor an event stream",
            );
        }
        Ok(())
    }

    async fn next(&mut self) -> Result<Option<Value>, RemoteCallError> {
        loop {
            match self.incoming.take() {
                None => return Ok(None),
                Some(Incoming::Messages(mut list)) => {
                    if list.is_empty() {
                        return Ok(None);
                    }
                    let first = list.remove(0);
                    self.incoming = Some(Incoming::Messages(list));
                    return Ok(Some(first));
                }
                Some(Incoming::Sse {
                    mut response,
                    mut buffer,
                    mut done,
                }) => {
                    if let Some(event) = take_event(&mut buffer) {
                        let put_back = |me: &mut Self, response, buffer, done| {
                            me.incoming = Some(Incoming::Sse {
                                response,
                                buffer,
                                done,
                            });
                        };
                        match event {
                            None => {
                                // Comments, retries, other event types and empty
                                // events carry nothing; they are counted so a
                                // flood of them ends the call.
                                self.tally.noise(0)?;
                                put_back(self, response, buffer, done);
                                continue;
                            }
                            Some(data) => {
                                put_back(self, response, buffer, done);
                                match serde_json::from_str::<Value>(&data) {
                                    Ok(value) => return Ok(Some(value)),
                                    Err(_) => {
                                        self.tally.noise(data.len())?;
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                    if done {
                        return Ok(None);
                    }
                    if buffer.len() > self.limits.message {
                        return fail("a message from the server was larger than the limit");
                    }
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            self.tally.read(chunk.len(), self.limits)?;
                            buffer.extend_from_slice(&chunk);
                        }
                        Ok(None) => done = true,
                        Err(e) => return Err(request_error(e, self.out.limit, self.out.needles)),
                    }
                    self.incoming = Some(Incoming::Sse {
                        response,
                        buffer,
                        done,
                    });
                }
            }
        }
    }

    fn negotiated(&mut self, version: &str) {
        self.version = Some(version.to_string());
    }
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

impl McpRunner {
    async fn run_stdio(
        &self,
        fields: &ToolMcpFields,
        pinned_sha256: Option<&str>,
        remote_tool: &str,
        arguments: Value,
        limit: Duration,
        cap: usize,
    ) -> Result<RemoteOutput, RemoteCallError> {
        let program = fields
            .command
            .as_deref()
            .ok_or_else(|| RemoteCallError("the tool's binding has no command".to_string()))?;
        let argv = fields.args.clone().unwrap_or_default();
        let (credentials, left) = credentials_within(
            limit,
            resolve_env_credentials(
                &self.secrets,
                &self.daemon_env,
                fields.env.as_deref().unwrap_or_default(),
            ),
        )
        .await?;
        let needles: Vec<Zeroizing<String>> = credentials.iter().map(|(_, v)| v.clone()).collect();
        // The file must still be the one the approval pinned.
        check_program_file(program, pinned_sha256).await?;
        let mut process = spawn_isolated(
            program,
            &argv,
            &self.daemon_env,
            &credentials,
            std::process::Stdio::piped(),
        )?;
        drop(credentials);
        let Isolated { guard, child, .. } = &mut process;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        // The server's stderr is read (so it never blocks on a full pipe) and
        // discarded: it is not shown to the user or the model.
        let drain = tokio::spawn(async move {
            let mut chunk = [0u8; READ_CHUNK];
            while matches!(stderr.read(&mut chunk).await, Ok(n) if n > 0) {}
        });
        let limits = Limits::for_cap(cap);
        let mut wire = StdioWire {
            stdin,
            stdout,
            buffer: Vec::new(),
            limits,
            tally: Tally::default(),
        };
        let mut progress = Progress::default();
        let outcome = tokio::time::timeout(
            left,
            converse(
                &mut wire,
                STDIO_VERSIONS,
                remote_tool,
                arguments,
                &mut progress,
            ),
        )
        .await;
        let outcome = match outcome {
            Ok(result) => result,
            Err(_) => {
                if progress.call_sent {
                    // Best effort: tell the server, then stop it.
                    let _ = tokio::time::timeout(
                        CLEANUP_LIMIT,
                        wire.send(
                            &notification(
                                "notifications/cancelled",
                                Some(json!({"requestId": 2, "reason": "timed out"})),
                            ),
                            false,
                        ),
                    )
                    .await;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(RemoteCallError(format!(
                    "the server did not answer within {} seconds and was stopped",
                    limit.as_secs()
                )))
            }
        };
        drop(wire);
        drain.abort();
        stop(child);
        reap(child).await;
        if matches!(child.try_wait(), Ok(Some(_))) {
            guard.disarm();
        }
        match outcome {
            Ok(result) => finish(&result, &needles, cap),
            Err(e) => Err(scrub(e, &needles)),
        }
    }

    async fn run_http(
        &self,
        fields: &ToolMcpFields,
        remote_tool: &str,
        arguments: Value,
        limit: Duration,
        cap: usize,
    ) -> Result<RemoteOutput, RemoteCallError> {
        let url = Url::parse(fields.url.as_deref().unwrap_or_default())
            .map_err(|_| RemoteCallError("the tool's binding has no usable url".to_string()))?;
        let (credential, left) = credentials_within(
            limit,
            http_credential(&self.secrets, &self.daemon_env, fields.auth.as_ref()),
        )
        .await?;
        let needles: Vec<Zeroizing<String>> = credential
            .iter()
            .flat_map(|c| c.needles.iter().cloned())
            .collect();
        let mut base = HeaderMap::new();
        if let Some(c) = &credential {
            base.insert(c.name.clone(), c.value.clone());
        }
        let out = Outgoing {
            resolver: &*self.resolver,
            lookups: &self.pinned_lookups,
            limit,
            needles: &needles,
        };
        let mut wire = HttpWire {
            url: url.clone(),
            base: base.clone(),
            out,
            session: None,
            version: None,
            incoming: None,
            limits: Limits::for_cap(cap),
            tally: Tally::default(),
        };
        let mut progress = Progress::default();
        let outcome = tokio::time::timeout(
            left,
            converse(
                &mut wire,
                HTTP_VERSIONS,
                remote_tool,
                arguments,
                &mut progress,
            ),
        )
        .await;
        let outcome = match outcome {
            Ok(result) => result,
            Err(_) => {
                if progress.call_sent {
                    let _ = tokio::time::timeout(
                        CLEANUP_LIMIT,
                        wire.send(
                            &notification(
                                "notifications/cancelled",
                                Some(json!({"requestId": 2, "reason": "timed out"})),
                            ),
                            false,
                        ),
                    )
                    .await;
                }
                Err(RemoteCallError(format!(
                    "the server did not answer within {} seconds",
                    limit.as_secs()
                )))
            }
        };
        // Close the session: best effort, bounded.
        if let Some(session) = wire.session.clone() {
            let mut headers = base;
            if let Ok(v) = HeaderValue::from_str(&session) {
                headers.insert(HeaderName::from_static("mcp-session-id"), v);
            }
            let closer = Outgoing {
                resolver: &*self.resolver,
                lookups: &self.pinned_lookups,
                limit: CLEANUP_LIMIT,
                needles: &needles,
            };
            let _ = tokio::time::timeout(
                CLEANUP_LIMIT,
                send_following(&closer, Method::DELETE, url, &headers, None),
            )
            .await;
        }
        drop(wire);
        match outcome {
            Ok(result) => finish(&result, &needles, cap),
            Err(e) => Err(scrub(e, &needles)),
        }
    }

    /// The confirmation text of a stdio call.
    fn describe_stdio(&self, fields: &ToolMcpFields, remote_tool: &str, args: &Value) -> String {
        let environment = environment_note(fields.env.as_deref().unwrap_or_default());
        let program = fields.command.as_deref().unwrap_or("?");
        let mut argv =
            serde_json::to_string(&fields.args.clone().unwrap_or_default()).unwrap_or_default();
        if argv.len() > MAX_ARGV_SHOWN {
            cut(&mut argv, MAX_ARGV_SHOWN);
            argv.push('\u{2026}');
        }
        // The credentials come first, then the parts that can be long.
        let mut tool_name = remote_tool.to_string();
        cut(&mut tool_name, MAX_TOOL_SHOWN);
        let head = format!(
            "MCP server over stdio{environment}; run {} with arguments {argv}; call tool `{tool_name}`",
            program_note(program, &self.daemon_env)
        );
        with_arguments(head, args)
    }
}

/// `head`, then the model's arguments in whatever room is left: only the
/// arguments are cut.
fn with_arguments(head: String, args: &Value) -> String {
    let prefix = "; arguments: ";
    let budget = MAX_DESTINATION_BYTES.saturating_sub(head.len() + prefix.len() + 3);
    let mut shown = serde_json::to_string(args).unwrap_or_default();
    if shown.len() > budget {
        cut(&mut shown, budget);
        shown.push('\u{2026}');
    }
    format!("{head}{prefix}{shown}")
}

fn describe_http(
    fields: &ToolMcpFields,
    url: &Url,
    resolved: Option<&str>,
    remote_tool: &str,
    args: &Value,
) -> String {
    let host = url.host_str().unwrap_or_default();
    let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
    let mut head = format!("MCP server over HTTP: {}://{host}{port}", url.scheme());
    match resolved {
        Some(addresses) => head.push_str(&format!(" (resolved to {addresses})")),
        None => head.push_str(" (the address is checked when it is sent)"),
    }
    head.push_str(&credential_note(fields.auth.as_ref()));
    let mut path = url[url::Position::BeforePath..].to_string();
    if path.len() > MAX_PATH_SHOWN {
        cut(&mut path, MAX_PATH_SHOWN);
        path.push('\u{2026}');
    }
    head.push_str(&format!(
        "; path and query: {path}; call tool `{remote_tool}`"
    ));
    with_arguments(head, args)
}

#[async_trait]
impl RemoteToolRunner for McpRunner {
    fn kind(&self) -> RemoteToolKind {
        RemoteToolKind::Mcp
    }

    fn destination(&self, tool: &RemoteTool, args: &Value) -> String {
        let fields = match read_binding(tool) {
            Ok(fields) => fields,
            Err(e) => return format!("(unusable binding: {})", e.0),
        };
        let remote_tool = fields.remote_tool_name.clone().unwrap_or_default();
        if fields.transport.as_deref() == Some("stdio") {
            return self.describe_stdio(&fields, &remote_tool, args);
        }
        match Url::parse(fields.url.as_deref().unwrap_or_default()) {
            Ok(url) => describe_http(&fields, &url, None, &remote_tool, args),
            Err(_) => "(unusable binding: the url is not valid)".to_string(),
        }
    }

    async fn confirmation_text(
        &self,
        tool: &RemoteTool,
        args: &Value,
    ) -> Result<String, RemoteCallError> {
        Ok(self.confirmation(tool, args).await?.text)
    }

    async fn confirmation(
        &self,
        tool: &RemoteTool,
        args: &Value,
    ) -> Result<Confirmation, RemoteCallError> {
        let fields = read_binding(tool)?;
        if fields.transport.as_deref() == Some("stdio") {
            return Ok(Confirmation {
                text: self.destination(tool, args),
                pin: None,
            });
        }
        let url = Url::parse(fields.url.as_deref().unwrap_or_default())
            .map_err(|_| RemoteCallError("the tool's binding has no usable url".to_string()))?;
        let target = destination(&url)?;
        let ip = tokio::time::timeout(CONNECT_TIMEOUT, checked_address(&*self.resolver, &target))
            .await
            .map_err(|_| {
                RemoteCallError(format!(
                    "`{}` could not be resolved in time: nothing was sent",
                    target.host
                ))
            })??;
        let remote_tool = fields.remote_tool_name.clone().unwrap_or_default();
        Ok(Confirmation {
            text: describe_http(&fields, &url, Some(&ip.to_string()), &remote_tool, args),
            pin: Some(ResolvedPin {
                host: target.host.clone(),
                address: ip,
            }),
        })
    }

    async fn run_pinned(
        &self,
        tool: &RemoteTool,
        args: &Value,
        pin: &ResolvedPin,
    ) -> Result<RemoteOutput, RemoteCallError> {
        let pinned = McpRunner {
            secrets: Arc::clone(&self.secrets),
            daemon_env: Arc::clone(&self.daemon_env),
            resolver: Arc::new(FixedResolver { pin: pin.clone() }),
            pinned_lookups: Arc::clone(&self.pinned_lookups),
        };
        pinned.run(tool, args).await
    }

    async fn run(&self, tool: &RemoteTool, args: &Value) -> Result<RemoteOutput, RemoteCallError> {
        let fields = read_binding(tool)?;
        let arguments = arguments(tool, args)?;
        let remote_tool = fields.remote_tool_name.clone().unwrap_or_default();
        let limit = Duration::from_secs(fields.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECONDS));
        let cap = DEFAULT_RESPONSE_BYTES as usize;
        match fields.transport.as_deref() {
            Some("stdio") => {
                self.run_stdio(
                    &fields,
                    tool.program_sha256.as_deref(),
                    &remote_tool,
                    arguments,
                    limit,
                    cap,
                )
                .await
            }
            Some("http") => {
                self.run_http(&fields, &remote_tool, arguments, limit, cap)
                    .await
            }
            _ => fail("the tool's binding has no usable transport"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_agent::test_http::{http, ok_json, serve, FakeResolver, Reply};
    use nodespace_core::secrets::InMemorySecretBackend;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    fn schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"q": {"type": "string"}, "n": {"type": "integer"}},
        })
    }

    /// A tool as the gate hands it over: a stdio server's program content
    /// pinned as the approval would pin it (the file as it is now).
    fn tool(binding: Value) -> RemoteTool {
        let program_sha256 = binding
            .get("command")
            .and_then(Value::as_str)
            .and_then(|p| crate::local_agent::process::file_sha256(p).ok());
        RemoteTool {
            node_id: "node-1".into(),
            name: "m_tool".into(),
            kind: RemoteToolKind::Mcp,
            description: "from the node".into(),
            parameter_schema: schema(),
            properties: json!({ "tool-mcp": binding }),
            approval_tag: String::new(),
            program_sha256,
        }
    }

    fn store() -> Arc<SecretStore> {
        Arc::new(SecretStore::new(Arc::new(InMemorySecretBackend::default())))
    }

    fn runner(secrets: Arc<SecretStore>) -> McpRunner {
        McpRunner::with_parts(
            secrets,
            |name| match name {
                "PATH" | "HOME" => std::env::var(name).ok(),
                "HOST_TOKEN" => Some("ENVVALUE-456".to_string()),
                _ => None,
            },
            Arc::new(SystemResolver),
        )
    }

    #[test]
    fn every_mcp_call_needs_confirmation_which_is_what_gates_offering() {
        // The host offers a tool whose calls need confirmation only when a
        // confirmer is installed; none is yet, and there is no read-only
        // exemption for MCP (a server can do anything whatever the tool is called).
        let t = tool(json!({"transport": "stdio", "command": "x", "remote_tool_name": "y"}));
        assert!(t.needs_confirmation());
        assert_eq!(runner(store()).kind(), RemoteToolKind::Mcp);
    }

    // -- the wire framing and results, without a process or a socket --------

    #[test]
    fn render_keeps_text_and_replaces_everything_else_with_a_note() {
        let (text, is_error) = render(
            &json!({"content": [
                {"type": "text", "text": "one"},
                {"type": "image", "data": "AAAA", "mimeType": "image/png"},
                {"type": "audio", "data": "BB"},
                {"type": "resource", "resource": {"uri": "file:///etc/passwd", "text": "SECRET"}},
                {"type": "resource_link", "uri": "file:///x"},
                {"type": "weird"},
                {"type": "text", "text": "two"}
            ]}),
            &[],
        );
        assert!(!is_error);
        assert!(text.contains("one") && text.contains("two"));
        assert!(text.contains("[image content omitted: 4 bytes]"));
        assert!(text.contains("[audio content omitted: 2 bytes]"));
        assert!(text.contains("[resource content omitted]"));
        assert!(text.contains("[unsupported content omitted]"));
        assert!(!text.contains("SECRET") && !text.contains("passwd"));
    }

    /// A wire that answers from a script.
    struct Scripted(Vec<Value>);

    #[async_trait]
    impl Wire for Scripted {
        async fn send(&mut self, _m: &Value, _r: bool) -> Result<(), RemoteCallError> {
            Ok(())
        }
        async fn next(&mut self) -> Result<Option<Value>, RemoteCallError> {
            Ok(if self.0.is_empty() {
                None
            } else {
                Some(self.0.remove(0))
            })
        }
    }

    #[tokio::test]
    async fn a_credential_straddling_the_error_cut_is_removed_at_every_offset() {
        let token = "SECRETVALUE-123456";
        let needles = vec![Zeroizing::new(token.to_string())];
        for offset in 1300..(ERROR_TEXT_BYTES + token.len() + 100) {
            let message = format!("{}{token}{}", "x".repeat(offset), "y".repeat(40));
            let mut wire = Scripted(vec![
                json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -1, "message": message}}),
            ]);
            let error = await_response(&mut wire, 2).await.unwrap_err();
            let shown = scrub(error, &needles).0;
            assert!(!shown.contains("SECR"), "offset {offset}: leaked a prefix");
        }
    }

    #[test]
    fn structured_content_is_used_only_without_text() {
        let (with_text, _) = render(
            &json!({
                "content": [{"type": "text", "text": "t"}],
                "structuredContent": {"a": 1}
            }),
            &[],
        );
        assert_eq!(with_text, "t");
        let (only, _) = render(&json!({"content": [], "structuredContent": {"a": 1}}), &[]);
        assert_eq!(only, r#"{"a":1}"#);
    }

    #[test]
    fn items_are_bounded() {
        let items: Vec<Value> = (0..MAX_ITEMS + 10)
            .map(|_| json!({"type": "text", "text": "x"}))
            .collect();
        let (text, _) = render(&json!({"content": items}), &[]);
        assert!(text.contains("[10 more content items omitted]"));
        assert_eq!(text.matches('x').count(), MAX_ITEMS);
    }

    #[test]
    fn sse_events_are_taken_whole_and_only_message_events() {
        let mut buffer = b": comment\n\nid: 1\ndata:\n\nevent: ping\ndata: skip\n\ndata: {\"a\":\ndata: 1}\n\ndata: tail".to_vec();
        // comment-only event: nothing
        assert_eq!(take_event(&mut buffer), Some(None));
        // empty data
        assert_eq!(take_event(&mut buffer), Some(None));
        // other event type
        assert_eq!(take_event(&mut buffer), Some(None));
        // multi-line data is joined with a newline
        assert_eq!(
            take_event(&mut buffer),
            Some(Some("{\"a\":\n1}".to_string()))
        );
        // an event that is not finished is not taken
        assert_eq!(take_event(&mut buffer), None);
        let mut crlf = b"data: x\r\n\r\n".to_vec();
        assert_eq!(take_event(&mut crlf), Some(Some("x".to_string())));
    }

    #[test]
    fn limits_follow_the_cap_within_bounds() {
        assert_eq!(Limits::for_cap(1).message, MESSAGE_FLOOR);
        assert_eq!(Limits::for_cap(usize::MAX).message, MESSAGE_CEILING);
        let normal = Limits::for_cap(1024 * 1024);
        assert_eq!(normal.message, 2 * 1024 * 1024);
        assert!(normal.total > normal.message);
    }

    // -- stdio, against a scripted fake server -------------------------------

    #[cfg(unix)]
    mod stdio {
        use super::*;

        const FAKE: &str = r#"
import sys, json, os, time
mode, log = sys.argv[1], sys.argv[2]
def out(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
def raw(s):
    sys.stdout.write(s + "\n"); sys.stdout.flush()
def note(s):
    with open(log, "a") as f: f.write(s + "\n")
def reply(i, r):
    out({"jsonrpc": "2.0", "id": i, "result": r})
def text(t, **kw):
    r = {"content": [{"type": "text", "text": t}]}
    r.update(kw)
    return r
token = os.environ.get("MCP_TOKEN", "")
note("pid:%d" % os.getpid())
note("env:" + ",".join(sorted(k for k in os.environ if k in ("MCP_TOKEN", "NS_MCP_INHERITED"))))
for line in sys.stdin:
    try:
        m = json.loads(line)
    except Exception:
        continue
    method = m.get("method")
    note("method:" + method if method else "response:" + json.dumps(m))
    if method == "initialize":
        if mode == "ignoreinit":
            continue
        note("caps:" + json.dumps(m["params"]["capabilities"]))
        v = {"badversion": "1999-01-01", "legacy": "2024-11-05"}.get(mode, m["params"]["protocolVersion"])
        reply(m["id"], {"protocolVersion": v, "capabilities": {"tools": {}}, "serverInfo": {"name": "fake", "version": "1"}})
    elif method == "tools/call":
        i = m["id"]
        note("call:" + json.dumps(m["params"]))
        try:
            if mode in ("ok", "legacy"):
                reply(i, text("hello " + json.dumps(m["params"]["arguments"], separators=(",", ":"))))
            elif mode == "ping":
                out({"jsonrpc": "2.0", "id": 5, "method": "ping"})
                note("response:" + sys.stdin.readline().strip())
                reply(i, text("done"))
            elif mode == "sampling":
                for k, meth in enumerate(["sampling/createMessage", "roots/list", "elicitation/create"]):
                    out({"jsonrpc": "2.0", "id": 90 + k, "method": meth, "params": {}})
                    note("response:" + sys.stdin.readline().strip())
                reply(i, text("done"))
            elif mode == "hugelist":
                out({"jsonrpc": "2.0", "id": 7, "result": {"tools": ["x" * 5000000]}})
                reply(i, text("late"))
            elif mode == "total":
                for _ in range(5):
                    out({"jsonrpc": "2.0", "method": "notifications/message", "params": {"data": "x" * 1900000}})
                reply(i, text("late"))
            elif mode == "echostruct":
                reply(i, {"content": [], "structuredContent": {"k": "pre " + token + " post", "key " + token: 1}})
            elif mode == "chatty":
                for _ in range(1200):
                    sys.stderr.write("e" * 1023 + "\n")
                sys.stderr.flush()
                reply(i, text("after the noise"))
            elif mode == "bigresult":
                reply(i, text("A" * 10000000))
            elif mode == "notify_forever":
                while True:
                    out({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progress": 1}})
            elif mode == "idmismatch":
                reply(999, text("wrong"))
                reply(i, text("right"))
            elif mode == "idmismatch_forever":
                while True:
                    reply(999, text("wrong"))
            elif mode == "noise":
                raw("log: starting")
                raw("{not json")
                reply(i, text("quiet"))
            elif mode == "noise_forever":
                while True:
                    raw("junk junk junk")
            elif mode == "nested100":
                raw('{"jsonrpc":"2.0","id":%d,"result":{"content":[],"structuredContent":%s}}' % (i, '{"a":' * 100 + '1' + '}' * 100))
            elif mode == "nested200":
                raw('{"jsonrpc":"2.0","id":%d,"result":%s}' % (i, '{"a":' * 200 + '1' + '}' * 200))
                reply(i, text("after"))
            elif mode == "echo":
                sys.stderr.write("stderr " + token + "\n"); sys.stderr.flush()
                reply(i, text("token=" + token + " end"))
            elif mode == "echoerr":
                out({"jsonrpc": "2.0", "id": i, "error": {"code": -32000, "message": "bad " + token}})
            elif mode == "die":
                sys.exit(3)
            elif mode == "batch":
                raw(json.dumps([{"jsonrpc": "2.0", "id": i, "result": text("x")}]))
            elif mode == "image":
                reply(i, {"content": [{"type": "image", "data": "A" * 1000, "mimeType": "image/png"}, {"type": "text", "text": "caption"}, {"type": "resource", "resource": {"uri": "file:///etc/passwd", "text": "SECRET"}}]})
            elif mode == "iserror":
                reply(i, text("it failed", isError=True))
            elif mode == "hang":
                pass
        except BrokenPipeError:
            sys.exit(0)
"#;

        struct Fixture {
            _dir: tempfile::TempDir,
            script: String,
            log: std::path::PathBuf,
        }

        fn fixture() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("fake_mcp.py");
            std::fs::write(&script, FAKE).unwrap();
            let log = dir.path().join("log.txt");
            Fixture {
                script: script.display().to_string(),
                log,
                _dir: dir,
            }
        }

        impl Fixture {
            fn binding(&self, mode: &str) -> Value {
                // The full path, as an approval writes it.
                let python = which::which("python3").expect("python3 is on the PATH");
                json!({
                    "transport": "stdio",
                    "command": python.to_str().unwrap(),
                    "args": [self.script, mode, self.log.display().to_string()],
                    "remote_tool_name": "echo",
                    "timeout_seconds": 10
                })
            }
            fn log(&self) -> Vec<String> {
                std::fs::read_to_string(&self.log)
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_string)
                    .collect()
            }
            /// The responses the fake server received from the client.
            fn responses(&self) -> Vec<Value> {
                self.log()
                    .iter()
                    .filter_map(|l| l.strip_prefix("response:"))
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect()
            }
            async fn run(&self, mode: &str) -> Result<RemoteOutput, RemoteCallError> {
                runner(store())
                    .run(&tool(self.binding(mode)), &json!({"q": "hi"}))
                    .await
            }
        }

        fn error_of(r: Result<RemoteOutput, RemoteCallError>) -> String {
            r.expect_err("the call should have failed").0
        }

        #[tokio::test]
        async fn a_call_is_initialize_initialized_then_the_bound_tool_only() {
            let f = fixture();
            let out = f.run("ok").await.unwrap();
            assert_eq!(out.text, r#"hello {"q":"hi"}"#);
            let log = f.log();
            let methods: Vec<&String> = log.iter().filter(|l| l.starts_with("method:")).collect();
            assert_eq!(
                methods,
                [
                    "method:initialize",
                    "method:notifications/initialized",
                    "method:tools/call"
                ]
            );
            // tools/list is never asked for, and no capability is advertised.
            assert!(!log.iter().any(|l| l.contains("tools/list")));
            assert!(log.contains(&"caps:{}".to_string()));
            let call = log.iter().find(|l| l.starts_with("call:")).unwrap();
            let params: Value = serde_json::from_str(&call["call:".len()..]).unwrap();
            assert_eq!(params["name"], "echo");
            assert_eq!(params["arguments"], json!({"q": "hi"}));
        }

        #[tokio::test]
        async fn the_server_is_killed_after_the_call() {
            let f = fixture();
            f.run("ok").await.unwrap();
            let pid = f
                .log()
                .iter()
                .find_map(|l| l.strip_prefix("pid:").map(str::to_string))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(!alive, "the server process outlived the call");
        }

        #[tokio::test]
        async fn arguments_that_do_not_fit_the_schema_start_nothing() {
            let f = fixture();
            let r = runner(store())
                .run(&tool(f.binding("ok")), &json!({"unknown": 1}))
                .await;
            assert!(r.is_err());
            assert!(
                !f.log.exists(),
                "a server was started for invalid arguments"
            );
        }

        #[tokio::test]
        async fn server_requests_are_refused_with_an_error() {
            let f = fixture();
            assert_eq!(f.run("sampling").await.unwrap().text, "done");
            let refused: Vec<Value> = f
                .responses()
                .into_iter()
                .filter(|m| (90..93).contains(&m["id"].as_i64().unwrap_or(0)))
                .collect();
            assert_eq!(refused.len(), 3, "{refused:?}");
            for m in refused {
                assert_eq!(m["error"]["code"], -32601, "{m}");
                assert!(m.get("result").is_none(), "{m}");
            }
        }

        #[tokio::test]
        async fn ping_is_answered_with_an_empty_result() {
            let f = fixture();
            assert_eq!(f.run("ping").await.unwrap().text, "done");
            assert!(f
                .responses()
                .iter()
                .any(|m| m["id"] == 5 && m["result"] == json!({})));
        }

        #[tokio::test]
        async fn a_huge_unsolicited_message_ends_the_call() {
            let f = fixture();
            let e = error_of(f.run("hugelist").await);
            assert!(e.contains("larger than the limit"), "{e}");
        }

        #[tokio::test]
        async fn a_ten_megabyte_result_is_refused_not_buffered() {
            let f = fixture();
            let e = error_of(f.run("bigresult").await);
            assert!(e.contains("larger than the limit"), "{e}");
        }

        #[tokio::test]
        async fn many_large_messages_hit_the_total_limit() {
            let f = fixture();
            let e = error_of(f.run("total").await);
            assert!(e.contains("more than the limit"), "{e}");
        }

        #[tokio::test]
        async fn endless_notifications_end_the_call() {
            let f = fixture();
            let started = Instant::now();
            let e = error_of(f.run("notify_forever").await);
            assert!(e.contains("too many messages"), "{e}");
            assert!(started.elapsed() < Duration::from_secs(8));
        }

        #[tokio::test]
        async fn an_answer_with_another_id_is_ignored_and_bounded() {
            let f = fixture();
            assert_eq!(f.run("idmismatch").await.unwrap().text, "right");
            let e = error_of(f.run("idmismatch_forever").await);
            assert!(e.contains("too many messages"), "{e}");
        }

        #[tokio::test]
        async fn non_json_output_is_ignored_and_bounded() {
            let f = fixture();
            assert_eq!(f.run("noise").await.unwrap().text, "quiet");
            let e = error_of(f.run("noise_forever").await);
            assert!(e.contains("not JSON-RPC"), "{e}");
        }

        #[tokio::test]
        async fn nesting_is_bounded_by_the_parser_not_disabled() {
            let f = fixture();
            // 100 levels parse under the default limit; the result is bounded data.
            let out = f.run("nested100").await.unwrap();
            assert!(out.text.starts_with(r#"{"a":{"a":"#), "{}", out.text);
            // 200 levels do not parse: the line is noise, the next line answers.
            assert_eq!(f.run("nested200").await.unwrap().text, "after");
        }

        #[tokio::test]
        async fn a_credential_the_server_echoes_is_removed_everywhere() {
            let f = fixture();
            let secrets = store();
            secrets.set("tok", "SECRETVALUE-123").unwrap();
            let mut binding = f.binding("echo");
            binding["env"] = json!([{"name": "MCP_TOKEN", "secret": "keychain:tok"}]);
            let out = runner(Arc::clone(&secrets))
                .run(&tool(binding.clone()), &json!({"q": "x"}))
                .await
                .unwrap();
            assert!(!out.text.contains("SECRETVALUE-123"), "{}", out.text);
            assert!(out.text.contains("end"));
            // An error the server writes carries it too.
            binding["args"][1] = json!("echoerr");
            let e = runner(secrets)
                .run(&tool(binding), &json!({"q": "x"}))
                .await
                .unwrap_err()
                .0;
            assert!(!e.contains("SECRETVALUE-123"), "{e}");
            assert!(e.contains("bad"), "{e}");
        }

        #[tokio::test]
        async fn a_chatty_stderr_does_not_stall_the_server() {
            let f = fixture();
            let out = f.run("chatty").await.unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(out.text, "after the noise");
        }

        #[tokio::test]
        async fn structured_content_is_redacted_before_it_is_serialised() {
            let f = fixture();
            let secrets = store();
            // Quote and backslash: the serialised form would be escaped.
            secrets.set("tok", "ab\"cd\\ef-9876").unwrap();
            let mut binding = f.binding("echostruct");
            binding["env"] = json!([{"name": "MCP_TOKEN", "secret": "keychain:tok"}]);
            let out = runner(secrets)
                .run(&tool(binding), &json!({"q": "x"}))
                .await
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(out.text.contains("[redacted]"), "{}", out.text);
            assert!(
                !out.text.contains("ef-9876") && !out.text.contains("ab"),
                "{}",
                out.text
            );
        }

        #[tokio::test]
        async fn a_hung_credential_store_ends_the_call_at_its_timeout() {
            let f = fixture();
            let secrets = Arc::new(SecretStore::new(Arc::new(
                crate::local_agent::test_http::SlowBackend(Duration::from_secs(3)),
            )));
            let mut binding = f.binding("ok");
            binding["env"] = json!([{"name": "MCP_TOKEN", "secret": "keychain:tok"}]);
            binding["timeout_seconds"] = json!(1);
            let started = Instant::now();
            let e = runner(secrets)
                .run(&tool(binding), &json!({"q": "x"}))
                .await
                .unwrap_err()
                .0;
            assert!(
                e.contains("credential store did not answer within 1 seconds"),
                "{e}"
            );
            assert!(started.elapsed() < Duration::from_millis(2500));
            assert!(
                !f.log.exists(),
                "a server was started before the credential was read"
            );
        }

        #[tokio::test]
        async fn the_credential_lookup_counts_against_the_same_timeout() {
            let f = fixture();
            let secrets = Arc::new(SecretStore::new(Arc::new(
                crate::local_agent::test_http::SlowBackend(Duration::from_millis(800)),
            )));
            let mut binding = f.binding("ignoreinit");
            binding["env"] = json!([{"name": "MCP_TOKEN", "secret": "keychain:tok"}]);
            binding["timeout_seconds"] = json!(1);
            let started = Instant::now();
            let e = runner(secrets)
                .run(&tool(binding), &json!({"q": "x"}))
                .await
                .unwrap_err()
                .0;
            assert!(e.contains("did not answer within 1 seconds"), "{e}");
            // One second in all, not the lookup plus a second.
            assert!(
                started.elapsed() < Duration::from_millis(1500),
                "{:?}",
                started.elapsed()
            );
        }

        #[tokio::test]
        async fn the_confirmation_keeps_all_credentials_and_some_room_for_arguments() {
            let f = fixture();
            let mut binding = f.binding("ok");
            binding["args"] = json!(["a".repeat(4000), "b".repeat(4000)]);
            binding["env"] = json!((0..8)
                .map(|i| json!({"name": format!("TOKEN_{i}"), "secret": format!("keychain:{}", "k".repeat(60))}))
                .collect::<Vec<_>>());
            let long = "z".repeat(5000);
            let shown = runner(store()).destination(&tool(binding), &json!({"q": long}));
            for i in 0..8 {
                assert!(
                    shown.contains(&format!("TOKEN_{i} from keychain:")),
                    "{shown}"
                );
            }
            let arguments = shown.split("; arguments: ").last().unwrap();
            assert!(arguments.len() >= 100, "{} {shown}", arguments.len());
            assert!(shown.len() <= MAX_DESTINATION_BYTES + 8);
        }

        #[tokio::test]
        async fn stderr_is_never_returned() {
            let f = fixture();
            let secrets = store();
            secrets.set("tok", "SECRETVALUE-123").unwrap();
            let mut binding = f.binding("echo");
            binding["env"] = json!([{"name": "MCP_TOKEN", "secret": "keychain:tok"}]);
            let out = runner(secrets)
                .run(&tool(binding), &json!({"q": "x"}))
                .await
                .unwrap();
            assert!(!out.text.contains("stderr"), "{}", out.text);
        }

        #[tokio::test]
        async fn the_environment_is_declared_entries_only() {
            std::env::set_var("NS_MCP_INHERITED", "leaked");
            let f = fixture();
            f.run("ok").await.unwrap();
            assert!(f.log().contains(&"env:".to_string()), "{:?}", f.log());
        }

        #[tokio::test]
        async fn a_server_that_dies_mid_call_is_an_error() {
            let f = fixture();
            let e = error_of(f.run("die").await);
            assert!(e.contains("ended before it answered"), "{e}");
        }

        #[tokio::test]
        async fn a_server_that_ignores_initialize_times_out() {
            let f = fixture();
            let mut binding = f.binding("ignoreinit");
            binding["timeout_seconds"] = json!(1);
            let started = Instant::now();
            let e = runner(store())
                .run(&tool(binding), &json!({"q": "x"}))
                .await
                .unwrap_err()
                .0;
            assert!(e.contains("did not answer within 1 seconds"), "{e}");
            assert!(started.elapsed() < Duration::from_secs(8));
        }

        #[tokio::test]
        async fn a_timed_out_call_is_cancelled_then_killed() {
            let f = fixture();
            let mut binding = f.binding("hang");
            binding["timeout_seconds"] = json!(1);
            let e = runner(store())
                .run(&tool(binding), &json!({"q": "x"}))
                .await
                .unwrap_err()
                .0;
            assert!(e.contains("was stopped"), "{e}");
            assert!(
                f.log()
                    .contains(&"method:notifications/cancelled".to_string()),
                "{:?}",
                f.log()
            );
            let pid = f
                .log()
                .iter()
                .find_map(|l| l.strip_prefix("pid:").map(str::to_string))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(!alive);
        }

        #[tokio::test]
        async fn a_batch_answer_is_refused() {
            let f = fixture();
            let e = error_of(f.run("batch").await);
            assert!(e.contains("batch"), "{e}");
        }

        #[tokio::test]
        async fn an_unsupported_protocol_version_is_refused() {
            let f = fixture();
            let e = error_of(f.run("badversion").await);
            assert!(e.contains("protocol version"), "{e}");
            // The calls never went further than initialize.
            assert!(!f.log().iter().any(|l| l.starts_with("call:")));
        }

        #[tokio::test]
        async fn the_legacy_version_is_accepted_on_stdio() {
            let f = fixture();
            let out = f.run("legacy").await.unwrap_or_else(|e| panic!("{e}"));
            assert!(out.text.starts_with("hello"), "{}", out.text);
        }

        #[tokio::test]
        async fn media_is_replaced_by_a_note_and_errors_are_failures() {
            let f = fixture();
            let out = f.run("image").await.unwrap();
            assert!(out.text.contains("[image content omitted: 1000 bytes]"));
            assert!(out.text.contains("caption"));
            assert!(!out.text.contains("SECRET") && !out.text.contains("passwd"));
            let e = error_of(f.run("iserror").await);
            assert!(e.contains("the tool reported an error: it failed"), "{e}");
        }

        #[tokio::test]
        async fn the_confirmation_lists_program_credentials_tool_and_cuts_only_arguments() {
            let f = fixture();
            let mut binding = f.binding("ok");
            binding["env"] = json!([{"name": "MCP_TOKEN", "secret": "keychain:tok"}]);
            let t = tool(binding);
            let long = "z".repeat(20_000);
            let shown = runner(store()).destination(&t, &json!({"q": long}));
            let python = which::which("python3").unwrap();
            assert!(
                shown.starts_with(&format!(
                    "MCP server over stdio; credentials in the environment: MCP_TOKEN from keychain:tok; run `{}`",
                    python.display()
                )),
                "{shown}"
            );
            assert!(shown.contains("MCP_TOKEN from keychain:tok"));
            assert!(shown.contains("call tool `echo`"));
            assert!(shown.contains("; arguments: "));
            assert!(shown.len() <= MAX_DESTINATION_BYTES + 8, "{}", shown.len());
            assert!(shown.ends_with('\u{2026}'));
        }
    }

    // -- streamable HTTP, against a loopback fake ----------------------------

    fn http_binding(port: u16) -> Value {
        json!({
            "transport": "http",
            "url": format!("http://127.0.0.1:{port}/mcp"),
            "remote_tool_name": "echo",
            "timeout_seconds": 10
        })
    }

    fn rpc(id: u64, result: Value) -> String {
        json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
    }

    fn init_reply(version: &str) -> Reply {
        Reply::Whole(http(
            "200 OK",
            &[
                ("Content-Type", "application/json"),
                ("Mcp-Session-Id", "sess-1"),
            ],
            rpc(1, json!({"protocolVersion": version, "capabilities": {}})).as_bytes(),
        ))
    }

    fn sse(body: &str) -> Reply {
        Reply::Whole(http(
            "200 OK",
            &[("Content-Type", "text/event-stream")],
            body.as_bytes(),
        ))
    }

    fn accepted() -> Reply {
        Reply::Whole(http("202 Accepted", &[], b""))
    }

    /// A server whose `tools/call` answer is made by `call`.
    async fn mcp_server(
        version: &'static str,
        call: impl Fn() -> Reply + Send + Sync + 'static,
    ) -> crate::local_agent::test_http::Server {
        serve(move |req| {
            if req.head.starts_with("DELETE") {
                return Reply::Whole(http("200 OK", &[], b""));
            }
            let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
            match body["method"].as_str() {
                Some("initialize") => init_reply(version),
                Some("tools/call") => call(),
                _ => accepted(),
            }
        })
        .await
    }

    fn methods(server: &crate::local_agent::test_http::Server) -> Vec<String> {
        server
            .requests()
            .iter()
            .map(|r| {
                if r.head.starts_with("DELETE") {
                    "DELETE".to_string()
                } else {
                    serde_json::from_slice::<Value>(&r.body).unwrap_or_default()["method"]
                        .as_str()
                        .unwrap_or("(response)")
                        .to_string()
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn http_json_answer_and_the_session_lifecycle() {
        let server = mcp_server("2025-06-18", || {
            ok_json(&rpc(
                2,
                json!({"content": [{"type": "text", "text": "pong"}]}),
            ))
        })
        .await;
        let out = runner(store())
            .run(&tool(http_binding(server.port)), &json!({"q": "x"}))
            .await
            .unwrap();
        assert_eq!(out.text, "pong");
        assert_eq!(
            methods(&server),
            [
                "initialize",
                "notifications/initialized",
                "tools/call",
                "DELETE"
            ]
        );
        let reqs = server.requests();
        let lower = |i: usize| reqs[i].head.to_ascii_lowercase();
        // The session and version go on every request after initialize.
        assert!(!lower(0).contains("mcp-session-id"));
        for (i, r) in reqs.iter().enumerate().skip(1) {
            assert!(lower(i).contains("mcp-session-id: sess-1"), "{}", r.head);
        }
        assert!(lower(2).contains("mcp-protocol-version: 2025-06-18"));
        assert!(lower(0).contains("accept: application/json, text/event-stream"));
        // The calls name the bound tool and nothing was listed.
        let call: Value = serde_json::from_slice(&reqs[2].body).unwrap();
        assert_eq!(call["params"]["name"], "echo");
        assert!(!methods(&server).iter().any(|m| m == "tools/list"));
        assert_eq!(runner(store()).pinned_lookups.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn http_event_stream_answer_with_a_server_request_refused() {
        let server = mcp_server("2025-03-26", || {
            sse(&format!(
                "data: {}\n\ndata: {}\n\ndata: {}\n\n",
                json!({"jsonrpc": "2.0", "id": 50, "method": "sampling/createMessage", "params": {}}),
                json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}),
                rpc(2, json!({"content": [{"type": "text", "text": "streamed"}]})),
            ))
        })
        .await;
        let out = runner(store())
            .run(&tool(http_binding(server.port)), &json!({"q": "x"}))
            .await
            .unwrap();
        assert_eq!(out.text, "streamed");
        let refusal = server
            .requests()
            .into_iter()
            .filter_map(|r| serde_json::from_slice::<Value>(&r.body).ok())
            .find(|b| b["id"] == 50)
            .expect("the server's request was answered");
        assert_eq!(refusal["error"]["code"], -32601);
        assert!(refusal.get("result").is_none());
    }

    #[tokio::test]
    async fn http_a_session_id_with_unusable_characters_is_refused() {
        for bad in ["has space", &"s".repeat(MAX_SESSION_ID + 1)] {
            let id = bad.to_string();
            let server = serve(move |_| {
                Reply::Whole(http(
                    "200 OK",
                    &[
                        ("Content-Type", "application/json"),
                        ("Mcp-Session-Id", id.as_str()),
                    ],
                    rpc(1, json!({"protocolVersion": "2025-06-18"})).as_bytes(),
                ))
            })
            .await;
            let e = runner(store())
                .run(&tool(http_binding(server.port)), &json!({"q": "x"}))
                .await
                .unwrap_err()
                .0;
            assert!(e.contains("session id that is not valid"), "{e}");
            assert_eq!(server.hits(), 1);
        }
    }

    #[tokio::test]
    async fn http_a_flood_of_empty_events_ends_the_call_promptly() {
        let body = "\n\n".repeat(2 * 1024 * 1024);
        let server = mcp_server("2025-06-18", move || sse(&body)).await;
        let started = Instant::now();
        let e = runner(store())
            .run(&tool(http_binding(server.port)), &json!({"q": "x"}))
            .await
            .unwrap_err()
            .0;
        assert!(e.contains("not JSON-RPC"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn http_a_hung_credential_store_ends_the_call_at_its_timeout() {
        let server = mcp_server("2025-06-18", || ok_json("{}")).await;
        let secrets = Arc::new(SecretStore::new(Arc::new(
            crate::local_agent::test_http::SlowBackend(Duration::from_secs(3)),
        )));
        let mut binding = http_binding(server.port);
        binding["auth"] = json!({"scheme": "bearer", "secret": "keychain:tok"});
        binding["timeout_seconds"] = json!(1);
        let started = Instant::now();
        let e = runner(secrets)
            .run(&tool(binding), &json!({"q": "x"}))
            .await
            .unwrap_err()
            .0;
        assert!(
            e.contains("credential store did not answer within 1 seconds"),
            "{e}"
        );
        assert!(started.elapsed() < Duration::from_millis(2500));
        assert_eq!(server.hits(), 0);
    }

    #[tokio::test]
    async fn http_only_supports_the_versions_of_its_transport() {
        let server = mcp_server("2024-11-05", || ok_json("{}")).await;
        let e = runner(store())
            .run(&tool(http_binding(server.port)), &json!({"q": "x"}))
            .await
            .unwrap_err()
            .0;
        assert!(e.contains("protocol version"), "{e}");
        assert!(!methods(&server).contains(&"tools/call".to_string()));
    }

    #[tokio::test]
    async fn http_credential_is_sent_and_removed_from_the_result() {
        let server = mcp_server("2025-06-18", || {
            ok_json(&rpc(
                2,
                json!({"content": [{"type": "text", "text": "echo SECRETVALUE-123 done"}]}),
            ))
        })
        .await;
        let secrets = store();
        secrets.set("tok", "SECRETVALUE-123").unwrap();
        let mut binding = http_binding(server.port);
        binding["auth"] = json!({"scheme": "bearer", "secret": "keychain:tok"});
        let out = runner(secrets)
            .run(&tool(binding), &json!({"q": "x"}))
            .await
            .unwrap();
        assert!(!out.text.contains("SECRETVALUE-123"), "{}", out.text);
        assert!(out.text.contains("done"));
        for r in server.requests() {
            assert!(
                r.head
                    .to_ascii_lowercase()
                    .contains("authorization: bearer secretvalue-123"),
                "{}",
                r.head
            );
        }
    }

    #[tokio::test]
    async fn http_redirect_to_another_origin_is_not_followed_and_carries_no_credential() {
        let other = serve(|_| ok_json("{}")).await;
        let target = format!("http://127.0.0.1:{}/x", other.port);
        let server = serve(move |_| {
            Reply::Whole(http(
                "307 Temporary Redirect",
                &[("Location", target.as_str())],
                b"",
            ))
        })
        .await;
        let secrets = store();
        secrets.set("tok", "SECRETVALUE-123").unwrap();
        let mut binding = http_binding(server.port);
        binding["auth"] = json!({"scheme": "bearer", "secret": "keychain:tok"});
        let e = runner(secrets)
            .run(&tool(binding), &json!({"q": "x"}))
            .await
            .unwrap_err()
            .0;
        assert!(e.contains("different origin"), "{e}");
        assert_eq!(other.hits(), 0);
    }

    #[tokio::test]
    async fn http_oversized_answer_is_refused() {
        let big = "A".repeat(10 * 1024 * 1024);
        let server = mcp_server("2025-06-18", move || {
            ok_json(&rpc(2, json!({"content": [{"type": "text", "text": big}]})))
        })
        .await;
        let e = runner(store())
            .run(&tool(http_binding(server.port)), &json!({"q": "x"}))
            .await
            .unwrap_err()
            .0;
        assert!(e.contains("larger than the limit"), "{e}");
    }

    #[tokio::test]
    async fn http_answer_that_is_not_json_or_a_stream_is_refused() {
        let server = mcp_server("2025-06-18", || {
            Reply::Whole(http("200 OK", &[("Content-Type", "text/html")], b"<html>"))
        })
        .await;
        let e = runner(store())
            .run(&tool(http_binding(server.port)), &json!({"q": "x"}))
            .await
            .unwrap_err()
            .0;
        assert!(e.contains("neither JSON nor an event stream"), "{e}");
    }

    #[tokio::test]
    async fn http_a_server_that_never_answers_times_out() {
        let server = serve(|_| Reply::Stream {
            head: b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n".to_vec(),
            chunk: b": keep-alive\n\n".to_vec(),
            every: Duration::from_millis(100),
        })
        .await;
        let mut binding = http_binding(server.port);
        binding["timeout_seconds"] = json!(1);
        let started = Instant::now();
        let e = runner(store())
            .run(&tool(binding), &json!({"q": "x"}))
            .await
            .unwrap_err()
            .0;
        assert!(
            e.contains("did not finish") || e.contains("did not answer"),
            "{e}"
        );
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[tokio::test]
    async fn http_a_named_host_is_resolved_checked_and_pinned() {
        let server = mcp_server("2025-06-18", || {
            ok_json(&rpc(
                2,
                json!({"content": [{"type": "text", "text": "ok"}]}),
            ))
        })
        .await;
        let resolver = FakeResolver::with("localhost", vec![vec!["127.0.0.1".parse().unwrap()]]);
        let r = McpRunner::with_parts(store(), |_| None, resolver);
        let mut binding = http_binding(server.port);
        binding["url"] = json!(format!("http://localhost:{}/mcp", server.port));
        assert_eq!(
            r.run(&tool(binding), &json!({"q": "x"}))
                .await
                .unwrap()
                .text,
            "ok"
        );
        assert!(r.pinned_lookups.load(Ordering::SeqCst) >= 4);
    }

    #[tokio::test]
    async fn http_a_private_address_is_refused_before_anything_is_sent() {
        let resolver = FakeResolver::with("evil.example", vec![vec!["10.0.0.5".parse().unwrap()]]);
        let r = McpRunner::with_parts(store(), |_| None, resolver);
        let binding = json!({
            "transport": "http",
            "url": "https://evil.example/mcp",
            "remote_tool_name": "echo"
        });
        let e = r
            .run(&tool(binding), &json!({"q": "x"}))
            .await
            .unwrap_err()
            .0;
        assert!(
            e.to_lowercase().contains("not allowed")
                || e.contains("private")
                || e.contains("address"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn http_confirmation_shows_url_resolution_credential_tool_and_cuts_only_arguments() {
        let resolver = FakeResolver::with("localhost", vec![vec!["127.0.0.1".parse().unwrap()]]);
        let r = McpRunner::with_parts(store(), |_| None, resolver);
        let binding = json!({
            "transport": "http",
            "url": "http://localhost:9/mcp",
            "remote_tool_name": "echo",
            "auth": {"scheme": "bearer", "secret": "keychain:tok"}
        });
        let long = "z".repeat(20_000);
        let shown = r
            .confirmation_text(&tool(binding), &json!({"q": long}))
            .await
            .unwrap();
        assert!(
            shown.starts_with("MCP server over HTTP: http://localhost:9 (resolved to 127.0.0.1)"),
            "{shown}"
        );
        assert!(shown.contains("credential (bearer) from keychain:tok"));
        assert!(shown.contains("call tool `echo`"));
        assert!(shown.len() <= MAX_DESTINATION_BYTES + 8);
        assert!(shown.ends_with('\u{2026}'));
    }

    #[tokio::test]
    async fn http_the_run_uses_the_address_the_confirmation_resolved() {
        let server = mcp_server("2025-06-18", || {
            ok_json(&rpc(
                2,
                json!({"content": [{"type": "text", "text": "ok"}]}),
            ))
        })
        .await;
        let resolver = FakeResolver::with(
            "localhost",
            vec![
                vec!["127.0.0.1".parse().unwrap()],
                vec!["10.0.0.5".parse().unwrap()],
            ],
        );
        let r = McpRunner::with_parts(store(), |_| None, resolver.clone());
        let mut binding = http_binding(server.port);
        binding["url"] = json!(format!("http://localhost:{}/mcp", server.port));
        let t = tool(binding);
        let confirmation = r.confirmation(&t, &json!({"q": "x"})).await.unwrap();
        let pin = confirmation.pin.expect("an http server is pinned");
        let out = r.run_pinned(&t, &json!({"q": "x"}), &pin).await.unwrap();
        assert_eq!(out.text, "ok");
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        // A stdio server resolves nothing and has nothing to pin.
        let stdio = tool(json!({"transport": "stdio", "command": "x", "remote_tool_name": "y"}));
        assert!(r
            .confirmation(&stdio, &json!({}))
            .await
            .unwrap()
            .pin
            .is_none());
    }

    /// The approval pinned the server program's content: a file that differs
    /// is not started, one that matches is.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_stdio_server_whose_file_changed_since_the_approval_is_not_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let binding = json!({
            "transport": "stdio",
            "command": "/bin/sh",
            "args": ["-c", format!("touch {}", marker.display())],
            "remote_tool_name": "y",
            "timeout_seconds": 5
        });
        let mut changed = tool(binding.clone());
        changed.program_sha256 = Some("0".repeat(64));
        let err = runner(store())
            .run(&changed, &json!({"q": "hi"}))
            .await
            .unwrap_err();
        assert!(
            err.0.contains("changed since the tool was approved"),
            "{}",
            err.0
        );
        assert!(!marker.exists(), "the server was started");
        let mut pinned = tool(binding);
        pinned.program_sha256 = Some(crate::local_agent::process::file_sha256("/bin/sh").unwrap());
        let _ = runner(store()).run(&pinned, &json!({"q": "hi"})).await;
        assert!(marker.exists(), "the pinned server was not started");
    }
}
