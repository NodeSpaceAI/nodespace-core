//! `nodespace mcp` — a stdio MCP server exposing exactly one passthrough
//! tool, for bash-less MCP surfaces (e.g. Claude Desktop's Chat tab, which
//! has no shell, file I/O, or code execution and can only reach the local
//! machine through a stdio MCP connector).
//!
//! Claude Desktop spawns this as a child process and speaks JSON-RPC 2.0
//! over its stdin/stdout — the MCP stdio transport, one message per line,
//! no embedded newlines. The daemon is not involved in this transport: this
//! process is just one more `nodespace` invocation against `daemon.sock`,
//! same as every other subcommand, using the same `--socket`/`--database`
//! resolution.
//!
//! The server exposes exactly one tool, `nodespace(args: string)`, where
//! `args` is the argument list that would follow `nodespace` on a shell
//! line (e.g. `args: "search \"auth tokens\""`). Each call:
//!
//! 1. Splits `args` the way a POSIX shell would ([`shell_words::split`]), so
//!    quoting behaves the same as every other CLI invocation.
//! 2. Prepends the resolved `--socket`/`--database` this `mcp` process was
//!    started with, and appends `--json` (unless already present), so the
//!    tool result is always structured.
//! 3. Shells out to this same compiled `nodespace` binary with that argv.
//!
//! Step 3 is deliberately a subprocess, not an in-process call to the
//! existing command handlers: those handlers `println!` their output, and
//! that would land on this process's own stdout — the same stream carrying
//! the JSON-RPC transport — interleaving CLI output into the middle of
//! protocol messages. Spawning the compiled binary keeps the exact same
//! code path (clap parsing, dispatch, the one gRPC client) while capturing
//! its output cleanly, so this stays a transport adapter rather than a
//! second client or a reimplementation of the subcommands.
//!
//! No consent gate is added here: the CLI has none today (`node delete`
//! deletes immediately, no confirmation prompt), and every call dispatches
//! through the same unmodified handlers. The resolve-then-confirm discipline
//! the skill documents for destructive verbs is enforced by the calling
//! model reading that guidance before it decides to invoke the tool — the
//! same as it is for every other shell-capable surface — not by anything in
//! this transport.
//!
//! # Trust boundary (ADR-038)
//!
//! `nodespace mcp` is the external-tool surface ADR-038's Trust Boundary
//! section anticipates, scoped to what "one passthrough tool, potentially
//! multiple external consumers" actually needs:
//!
//! - **Explicit user enablement.** [`run_server`] refuses to serve anything
//!   -- not even `initialize` -- unless `~/.nodespace/daemon.toml`'s `[mcp]
//!   enabled` flag is `true` (see [`enforce_enabled`]). The flag defaults to
//!   `false` ([`nodespace_daemon::McpConfig`]) and is only ever set by
//!   [`install`]/[`uninstall`] below, so a client pointed at this binary
//!   (whether via the installer or a hand-edited config) cannot get a live
//!   connection until a user has actually run `nodespace mcp install`.
//! - **Provenance marking.** Every `tools/list` entry carries
//!   `_meta.source: "external"` ([`TOOL_SOURCE`]) -- see that constant's doc
//!   comment for why this is the scoped analog of ADR-038's registry-node
//!   `source` marker rather than the marker itself.
//! - **Generated-schema validation.** [`validate_tool_schema`] enforces
//!   bounded nesting, no unbounded `additionalProperties`, and a size cap on
//!   the tool's own `inputSchema` before [`tools_list_result`] ever returns
//!   it.
//!
//! # Installer (`nodespace mcp install`/`uninstall`/`status`)
//!
//! Configures a bash-less MCP client (currently Claude Desktop) to launch
//! this server and flips the enablement flag above -- the CLI-only
//! equivalent, for this transport, of what `nodespace skill install` already
//! does for the skill files. See [`install`], [`uninstall`], [`status`], and
//! `packages/skill/src/mcp-installer.ts` (the client-config writer these
//! subcommands shell out to, via the same compiled installer
//! `commands::skill::resolve_installer` already resolves).

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nodespace_daemon::{read_mcp_settings, set_mcp_enabled, McpConfig};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as ChildCommand;

/// MCP protocol revision this server speaks. Always advertised as-is,
/// regardless of what a client requests in `initialize` — the surface is
/// fixed (one tool, no resources/prompts/sampling), so there is nothing to
/// negotiate.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// The one tool this server exposes. `pub` so `packages/skill/SKILL.md`'s
/// Preflight Check (Branch 2, the MCP passthrough) can be tested against this
/// value directly (`packages/cli/tests/skill_md_generation.rs`) rather than a
/// second, hand-typed copy that could silently drift from a future rename.
pub const TOOL_NAME: &str = "nodespace";

const TOOL_DESCRIPTION: &str = "Run a `nodespace` CLI command. `args` is the exact argument list \
    that would follow `nodespace` on a shell line (e.g. `search \"auth tokens\"`, `node get <id>`). \
    Returns the command's --json output. See the NodeSpace skill for available commands.";

/// A single dispatched invocation may run for this long before being killed
/// and reported as a timeout. Generous enough for a slow `import`/`query`,
/// but finite: `session launch`/`session attach` are designed to stream and
/// block indefinitely (interactive PTY attachment), which would otherwise
/// wedge this server's single-threaded request loop forever.
///
/// `pub` for the same reason as [`TOOL_NAME`]: SKILL.md's Branch 2 documents
/// this exact timeout, and a test pins the two together.
pub const DISPATCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Provenance marker stamped on every `tools/list` entry's `_meta.source`.
///
/// ADR-038's Trust Boundary requires "Provenance is marked. Tool/skill nodes
/// carry a `source` marker (e.g. internal-generated vs external) visible to
/// the retrieval filter, the Stage-2 judge, and the UI." That marker lives on
/// registry *nodes* consumed by the local agent's routing pipeline
/// (`packages/agent`) -- and no such node-backed registry for external tools
/// exists in the codebase yet (ADR-038's Implementation Notes item 5, "tools
/// as nodes… trust boundary," is still open; this issue does not build it).
///
/// This constant is the scoped analog for what does exist today: the actual
/// wire-level tool definition this server hands to a client. Stamping it here
/// makes the provenance visible in real protocol traffic now, and gives a
/// future registry-node integration a value to carry forward rather than a
/// decision to make from scratch.
pub const TOOL_SOURCE: &str = "external";

/// Maximum nesting depth [`validate_tool_schema`] allows in the passthrough
/// tool's `inputSchema` (`properties`/`items`/object-valued
/// `additionalProperties`/`anyOf`/`oneOf`/`allOf`, each one level). The real
/// schema is two levels deep (`{type: object, properties: {args: {type:
/// string}}}`); six is generous headroom for it to grow without coming close
/// to the point ADR-038 warns about ("the same nesting that makes typed tools
/// win makes an unbounded externally-supplied schema a context-flood
/// vector").
const MAX_SCHEMA_NESTING_DEPTH: usize = 6;

/// Maximum serialized size, in bytes, [`validate_tool_schema`] allows for the
/// passthrough tool's `inputSchema`. The real schema is well under 200 bytes;
/// this caps how large a future edit could grow it before validation refuses
/// to serve it, per ADR-038's "size cap" requirement.
const MAX_SCHEMA_BYTES: usize = 4096;

/// ADR-038 Trust Boundary: "Generated schemas are validated. Typed tools
/// generated from registry nodes (internal or external) must pass schema
/// validation before reaching inference: bounded nesting depth, no unbounded
/// `additionalProperties`, size cap." This tool's schema is static today
/// rather than literally generated from a registry node, but the same
/// invariant applies to it: nothing else stops a future edit to
/// [`tools_list_result`] from growing it into an unbounded,
/// context-flooding shape before a client ever sees it. Called once per
/// `tools/list` response, before it is returned -- see that function.
///
/// Returns `Err` with a human-readable reason on the first violation found,
/// rather than collecting every violation -- this is a startup/response-time
/// invariant check on a hand-authored schema, not a linter report.
fn validate_tool_schema(schema: &Value) -> Result<(), String> {
    let serialized =
        serde_json::to_string(schema).map_err(|e| format!("schema is not serializable: {e}"))?;
    if serialized.len() > MAX_SCHEMA_BYTES {
        return Err(format!(
            "tool schema is {} bytes, exceeds the {}-byte cap",
            serialized.len(),
            MAX_SCHEMA_BYTES
        ));
    }
    check_schema_depth(schema, 0)
}

/// Recursive worker for [`validate_tool_schema`]: walks every JSON Schema
/// construct that can nest (`properties`, `items`, an object-valued
/// `additionalProperties`, and the `anyOf`/`oneOf`/`allOf` combinators),
/// rejecting depth beyond [`MAX_SCHEMA_NESTING_DEPTH`] and any
/// `additionalProperties: true` (unbounded) at any level. A schema fragment
/// that isn't a JSON object (a leaf like `{"type": "string"}`'s scalar
/// values) has nothing further to walk and passes trivially.
fn check_schema_depth(node: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_SCHEMA_NESTING_DEPTH {
        return Err(format!(
            "tool schema nesting exceeds the maximum depth of {MAX_SCHEMA_NESTING_DEPTH}"
        ));
    }
    let Some(map) = node.as_object() else {
        return Ok(());
    };

    if let Some(additional) = map.get("additionalProperties") {
        if additional.as_bool() == Some(true) {
            return Err("tool schema has unbounded additionalProperties: true".to_string());
        }
        if additional.is_object() {
            check_schema_depth(additional, depth + 1)?;
        }
    }
    if let Some(props) = map.get("properties").and_then(Value::as_object) {
        for value in props.values() {
            check_schema_depth(value, depth + 1)?;
        }
    }
    if let Some(items) = map.get("items") {
        check_schema_depth(items, depth + 1)?;
    }
    for combinator in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = map.get(combinator).and_then(Value::as_array) {
            for variant in variants {
                check_schema_depth(variant, depth + 1)?;
            }
        }
    }
    Ok(())
}

/// `nodespace mcp install`/`uninstall`/`status`.
#[derive(Subcommand, Debug)]
pub enum McpAction {
    /// Configure a detected bash-less MCP client (currently Claude Desktop)
    /// to launch `nodespace mcp`, and enable the passthrough tool. Safe to
    /// re-run.
    Install(InstallArgs),
    /// Remove the MCP config this wrote from every detected client and
    /// disable the passthrough tool again.
    Uninstall,
    /// Report whether the passthrough tool is enabled and which clients
    /// currently have a config pointing at it.
    Status,
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Install without prompting for confirmation. Implied automatically
    /// when stdin/stdout isn't a terminal — mirrors `nodespace skill
    /// install`'s `--yes`.
    #[arg(long)]
    pub yes: bool,
}

/// Top-level dispatch for `nodespace mcp [install|uninstall|status]`. With no
/// subcommand — how a client config (e.g. `claude_desktop_config.json`)
/// invokes this binary — hosts the stdio server; see [`run_server`].
pub async fn run(action: Option<McpAction>, sock: PathBuf, database: Option<String>) -> Result<()> {
    match action {
        None => run_server(sock, database).await,
        Some(McpAction::Install(args)) => install(args).await,
        Some(McpAction::Uninstall) => uninstall().await,
        Some(McpAction::Status) => status().await,
    }
}

/// Resolves `~/.nodespace/daemon.toml` — the same file
/// `nodespace_daemon::SettingsServiceImpl` reads/writes — independently of
/// the daemon: this whole file is a separate process with no gRPC connection
/// to it, for the server loop ([`run_server`]'s enablement check) as much as
/// for `install`/`uninstall`/`status` below.
fn daemon_config_path() -> Result<PathBuf> {
    let home =
        std::env::var("HOME").context("$HOME is unset — cannot locate NodeSpace settings")?;
    Ok(PathBuf::from(home)
        .join(nodespace_proto::socket::STATE_DIR)
        .join("daemon.toml"))
}

/// Prompt on a real terminal; auto-confirm (stating that the prompt was
/// skipped, so the choice is visible in captured output) when stdin or
/// stdout isn't one — mirrors `commands::skill::confirm_install`.
fn confirm_enable() -> Result<bool> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        println!(
            "No interactive terminal detected -- proceeding with install (pass --yes to silence \
             this message)."
        );
        return Ok(true);
    }

    print!(
        "Enable the NodeSpace MCP passthrough tool and configure detected clients (e.g. Claude \
         Desktop) to launch `nodespace mcp`? A configured client will be able to run any \
         `nodespace` CLI command through this connection. [Y/n] "
    );
    use std::io::Write;
    std::io::stdout().flush().ok();

    let mut reply = String::new();
    std::io::stdin()
        .read_line(&mut reply)
        .context("Failed to read confirmation from stdin")?;
    let reply = reply.trim().to_ascii_lowercase();
    Ok(reply.is_empty() || reply == "y" || reply == "yes")
}

/// Writes a detected client's MCP config (via the shared skill/MCP
/// installer — see `commands::skill::resolve_installer`) and, only once at
/// least one client was actually configured, enables the passthrough tool.
/// This ordering is the explicit-user-enablement gate itself: a run that
/// configures nothing (nothing detected, or every candidate skipped) leaves
/// the tool disabled, exactly as it was before.
async fn install(args: InstallArgs) -> Result<()> {
    if !args.yes && !confirm_enable()? {
        println!("Skipped.");
        return Ok(());
    }

    let installer = super::skill::resolve_installer()?;
    let outcome = super::skill::run_installer_subcommand(&installer, "mcp-install")?;

    if outcome.installed.is_empty() && outcome.skipped.is_empty() {
        println!("No supported bash-less MCP clients detected (checked: Claude Desktop).");
        return Ok(());
    }
    for client in &outcome.installed {
        println!("✓ {client}: MCP config written");
    }
    for skipped in &outcome.skipped {
        println!("⚠ {}: {}", skipped.agent, skipped.reason);
    }
    if outcome.installed.is_empty() {
        println!("Nothing was configured -- the passthrough tool stays disabled.");
        return Ok(());
    }

    let config_path = daemon_config_path()?;
    set_mcp_enabled(&config_path, true)
        .await
        .context("enable the NodeSpace MCP passthrough tool")?;
    println!("The NodeSpace MCP passthrough tool is now enabled.");
    Ok(())
}

/// Removes every detected client's MCP config and always disables the
/// passthrough tool afterward — unconditionally, not just when a config was
/// actually found, so this is a reliable way back to "definitely disabled"
/// regardless of what state the client configs are in.
async fn uninstall() -> Result<()> {
    let installer = super::skill::resolve_installer()?;
    let outcome = super::skill::run_installer_subcommand(&installer, "mcp-uninstall")?;

    if outcome.installed.is_empty() && outcome.skipped.is_empty() {
        println!("No MCP client configs found.");
    } else {
        for client in &outcome.installed {
            println!("✓ {client}: MCP config removed");
        }
        for skipped in &outcome.skipped {
            println!("  {}: {}", skipped.agent, skipped.reason);
        }
    }

    let config_path = daemon_config_path()?;
    set_mcp_enabled(&config_path, false)
        .await
        .context("disable the NodeSpace MCP passthrough tool")?;
    println!("The NodeSpace MCP passthrough tool is now disabled.");
    Ok(())
}

/// Reports the enablement flag plus which detected clients currently have a
/// config pointing at this server. Read-only — touches neither the flag nor
/// any client config.
async fn status() -> Result<()> {
    let config_path = daemon_config_path()?;
    let settings = read_mcp_settings(&config_path)
        .await
        .context("read NodeSpace MCP settings")?;
    println!(
        "Passthrough tool enabled: {}",
        if settings.enabled { "yes" } else { "no" }
    );

    let installer = super::skill::resolve_installer()?;
    let outcome = super::skill::run_installer_subcommand(&installer, "mcp-status")?;
    for client in &outcome.installed {
        println!("✓ {client}: config present");
    }
    for skipped in &outcome.skipped {
        println!("  {}: {}", skipped.agent, skipped.reason);
    }
    Ok(())
}

/// The trust-boundary gate itself: `nodespace mcp` (this whole server) must
/// not be live/registered until a user has explicitly turned it on (ADR-038
/// Trust Boundary, "Registration is gated… external tools require explicit
/// user enablement"). Pure so the message is unit-testable without file I/O
/// — [`run_server`] is the only caller that actually reads the setting.
fn enforce_enabled(settings: &McpConfig) -> Result<()> {
    if settings.enabled {
        return Ok(());
    }
    anyhow::bail!(
        "The NodeSpace MCP passthrough tool is not enabled. Run `nodespace mcp install` to \
         enable it and configure a client (e.g. Claude Desktop), or `nodespace mcp status` to \
         check the current state."
    )
}

/// Hosts the stdio MCP server: reads JSON-RPC requests from stdin, one per
/// line, dispatches them, and writes JSON-RPC responses to stdout, one per
/// line, until stdin closes (the client disconnected).
///
/// `sock` is the already-resolved daemon socket path — honoring `--socket` /
/// `NODESPACED_SOCKET` / auto-discovery, exactly as every other subcommand
/// resolves it via [`crate::resolve_socket_path`] — and `database` is the
/// raw `--database` selection, if any. Both are forwarded to every
/// dispatched child invocation, so the whole MCP session targets one
/// consistent daemon/database: the one `nodespace mcp` itself was started
/// with.
///
/// Refuses to serve anything — not even `initialize` — unless the
/// passthrough tool is explicitly enabled (see [`enforce_enabled`]),
/// checked once before entering the request loop: this is the strongest
/// form the trust boundary can take, since a disabled server never accepts a
/// client's first message at all rather than accepting a connection and
/// gating individual calls.
pub async fn run_server(sock: PathBuf, database: Option<String>) -> Result<()> {
    let config_path = daemon_config_path()?;
    let settings = read_mcp_settings(&config_path)
        .await
        .context("read NodeSpace MCP settings")?;
    enforce_enabled(&settings)?;

    let exe = std::env::current_exe().context("resolve the nodespace executable's own path")?;

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await.context("read stdin")? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(response) = handle_message(line, &exe, &sock, database.as_deref()).await {
            let text = serde_json::to_string(&response).context("serialize JSON-RPC response")?;
            stdout.write_all(text.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }

    Ok(())
}

/// Handles one JSON-RPC message (a single stdin line). Returns `None` for
/// notifications (no `id`) and whenever a notification-shaped message would
/// otherwise trigger a response — JSON-RPC forbids responding to those.
/// Never panics: every failure becomes either a JSON-RPC error object or,
/// for a recognized tool call that fails, a tool result with
/// `isError: true`.
async fn handle_message(
    line: &str,
    exe: &Path,
    sock: &Path,
    database: Option<&str>,
) -> Option<Value> {
    let request: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(error_response(
                Value::Null,
                -32700,
                &format!("Parse error: {e}"),
            ));
        }
    };

    let id = request.get("id").cloned();
    let is_notification = id.is_none();

    let method = match request.get("method").and_then(Value::as_str) {
        Some(m) => m,
        None => {
            return if is_notification {
                None
            } else {
                Some(error_response(
                    id.unwrap_or(Value::Null),
                    -32600,
                    "Invalid Request: missing \"method\"",
                ))
            };
        }
    };

    match method {
        "initialize" => id.map(|id| success_response(id, initialize_result())),
        "notifications/initialized" | "notifications/cancelled" => None,
        "ping" => id.map(|id| success_response(id, json!({}))),
        "tools/list" => id.map(|id| match tools_list_result() {
            Ok(result) => success_response(id, result),
            Err(msg) => error_response(
                id,
                -32603,
                &format!("Internal error: passthrough tool schema failed validation: {msg}"),
            ),
        }),
        "tools/call" => {
            let id = id?;
            let params = request.get("params").cloned().unwrap_or(Value::Null);
            Some(match extract_tool_call(&params) {
                Ok(args_str) => {
                    let result = call_tool(exe, &args_str, sock, database).await;
                    success_response(id, result)
                }
                Err(msg) => error_response(id, -32602, &msg),
            })
        }
        other => {
            if is_notification {
                None
            } else {
                Some(error_response(
                    id.unwrap_or(Value::Null),
                    -32601,
                    &format!("Method not found: {other}"),
                ))
            }
        }
    }
}

fn success_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "nodespace", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// Builds the `tools/list` response, running the tool's `inputSchema`
/// through [`validate_tool_schema`] before returning it — ADR-038's Trust
/// Boundary requires this happen "before reaching inference," and a
/// `tools/list` response is the first point this schema reaches any client's
/// inference. `_meta.source` carries [`TOOL_SOURCE`], the provenance marker;
/// see its doc comment.
fn tools_list_result() -> Result<Value, String> {
    let schema = json!({
        "type": "object",
        "properties": {
            "args": {
                "type": "string",
                "description": "The argument list that would follow `nodespace` on a shell line.",
            }
        },
        "required": ["args"],
    });
    validate_tool_schema(&schema)?;

    Ok(json!({
        "tools": [{
            "name": TOOL_NAME,
            "description": TOOL_DESCRIPTION,
            "inputSchema": schema,
            "_meta": {"source": TOOL_SOURCE},
        }],
    }))
}

/// Validates a `tools/call` request's `params` and extracts the `args`
/// string, without running anything yet. The returned `Err` is the message
/// for a JSON-RPC `Invalid params` error: an unknown tool name or a
/// missing/non-string `args` is a malformed call, not a tool execution
/// failure, so it is reported at the protocol level rather than through
/// [`call_tool`]'s `isError` result.
fn extract_tool_call(params: &Value) -> Result<String, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tools/call params missing string \"name\"".to_string())?;
    if name != TOOL_NAME {
        return Err(format!("Unknown tool: {name}"));
    }
    params
        .get("arguments")
        .and_then(|a| a.get("args"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("the \"{TOOL_NAME}\" tool requires a string \"args\" argument"))
}

/// Builds the child argv for one dispatched call: the resolved
/// `--socket`/`--database` this `mcp` process was started with, then the
/// shell-split `args`, then `--json` (unless already present) so the tool
/// result is always structured.
///
/// Pure and synchronous so quoting/splitting behavior is unit-testable
/// without spawning a process. If the caller's `args` explicitly repeats
/// `--socket`/`--database`, its value wins over the prefix (clap keeps the
/// last occurrence of a non-repeatable flag), so an explicit override in the
/// tool call is still honored.
fn build_child_args(
    args_str: &str,
    sock: &Path,
    database: Option<&str>,
) -> Result<Vec<String>, String> {
    let tail = shell_words::split(args_str)
        .map_err(|e| format!("could not parse \"args\" as shell-style arguments: {e}"))?;

    let mut argv = vec!["--socket".to_string(), sock.display().to_string()];
    if let Some(db) = database {
        argv.push("--database".to_string());
        argv.push(db.to_string());
    }
    argv.extend(tail);

    if !argv.iter().any(|a| a == "--json") {
        argv.push("--json".to_string());
    }

    Ok(argv)
}

async fn call_tool(exe: &Path, args_str: &str, sock: &Path, database: Option<&str>) -> Value {
    match build_child_args(args_str, sock, database) {
        Ok(argv) => run_child(exe, &argv, DISPATCH_TIMEOUT).await,
        Err(msg) => tool_error(msg),
    }
}

/// Spawns `exe` with `argv`, waits up to `timeout`, and turns the outcome
/// into an MCP tool-call result. Never returns an `Err`: every failure mode
/// — spawn failure, non-zero exit, timeout — becomes a normal
/// `isError: true` result, so a failed dispatch reads as an actionable
/// answer rather than crashing the server or the caller's turn.
///
/// `kill_on_drop(true)` ensures a child that outlives `timeout` (e.g. a
/// `session launch` that never exits on its own) is actually killed when the
/// timed-out future is dropped, rather than left running as an orphan.
///
/// `stdin(Stdio::null())` is load-bearing, not cosmetic: unlike
/// `std::process::Command::output()`, which nulls stdin by default,
/// `tokio::process::Command::output()` leaves stdin untouched — i.e.
/// inherited from this `mcp` process, which is this server's real JSON-RPC
/// transport. Without this call, dispatching a subcommand that itself reads
/// stdin (`session attach`/`session launch`, which stream a PTY) would race
/// this function's own stdin loop for bytes off the same pipe, silently
/// diverting a live client's subsequent JSON-RPC messages into an unrelated
/// terminal session instead of the request loop. Do not remove it.
async fn run_child(exe: &Path, argv: &[String], timeout: Duration) -> Value {
    let mut command = ChildCommand::new(exe);
    command
        .args(argv)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());

    match tokio::time::timeout(timeout, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            json!({"content": [{"type": "text", "text": text}], "isError": false})
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let text = if !stderr.is_empty() {
                clean_cli_error(&stderr)
            } else {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if stdout.is_empty() {
                    format!("nodespace exited with {} and no output", output.status)
                } else {
                    stdout
                }
            };
            tool_error(text)
        }
        Ok(Err(e)) => tool_error(format!(
            "Failed to run the nodespace CLI at {}: {e}. Reinstall NodeSpace or confirm its CLI \
             binary is present and executable.",
            exe.display()
        )),
        Err(_) => tool_error(format!(
            "`nodespace {}` did not complete within {}s. Commands that stream or block (e.g. \
             `session launch`/`session attach`) are not supported through this passthrough — use \
             a shell-capable surface (Claude Code, the Claude Desktop Code tab) for interactive \
             sessions.",
            argv.join(" "),
            timeout.as_secs(),
        )),
    }
}

fn tool_error(text: String) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

/// Cleans up a dispatched invocation's raw stderr into a tool-friendly
/// message: strips the `Error: ` prefix `std::process::Termination` adds for
/// a failing `anyhow::Result`, and drops the `Caused by:` chain of internal
/// (often OS-level) detail that follows it — the headline anyhow message is
/// already the actionable part (e.g. `connect_error_context`'s "Is the
/// daemon running?"); the chain exists for human debugging, not for a model
/// deciding what to do next. Anything not shaped like that (e.g. clap's own
/// usage/parse errors, which start with a lowercase `error:`) is left as-is.
fn clean_cli_error(stderr: &str) -> String {
    let text = stderr.strip_prefix("Error: ").unwrap_or(stderr);
    let text = text.split("\n\nCaused by:").next().unwrap_or(text);
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_child_args_splits_quoted_args_and_appends_json() {
        let argv = build_child_args(
            r#"search "auth tokens""#,
            Path::new("/tmp/daemon.sock"),
            None,
        )
        .expect("valid shell syntax");
        assert_eq!(
            argv,
            vec![
                "--socket",
                "/tmp/daemon.sock",
                "search",
                "auth tokens",
                "--json"
            ]
        );
    }

    #[test]
    fn build_child_args_forwards_database_selection() {
        let argv = build_child_args(
            "node get abc-123",
            Path::new("/tmp/daemon.sock"),
            Some("work"),
        )
        .expect("valid shell syntax");
        assert_eq!(
            argv,
            vec![
                "--socket",
                "/tmp/daemon.sock",
                "--database",
                "work",
                "node",
                "get",
                "abc-123",
                "--json",
            ]
        );
    }

    #[test]
    fn build_child_args_does_not_duplicate_an_explicit_json_flag() {
        let argv = build_child_args("node get abc-123 --json", Path::new("/s"), None)
            .expect("valid shell syntax");
        assert_eq!(argv.iter().filter(|a| *a == "--json").count(), 1);
    }

    #[test]
    fn build_child_args_rejects_unterminated_quotes() {
        let err = build_child_args(r#"search "unterminated"#, Path::new("/s"), None)
            .expect_err("unterminated quote must not panic or silently truncate");
        assert!(err.contains("could not parse"));
    }

    #[test]
    fn build_child_args_handles_empty_args_string() {
        // A bare tool call with an empty `args` string is well-formed shell
        // syntax (zero tokens) — the dispatched binary is responsible for
        // rejecting "no subcommand given", the same as running `nodespace`
        // alone.
        let argv = build_child_args("", Path::new("/s"), None).expect("empty is valid");
        assert_eq!(argv, vec!["--socket", "/s", "--json"]);
    }

    #[test]
    fn build_child_args_lets_an_explicit_socket_override_win() {
        // shell_words splits "--socket" and its value as ordinary tokens; they
        // land after our own --socket/value pair, and clap keeps the last
        // occurrence of a non-repeatable flag, so the caller's explicit
        // override is what the dispatched binary actually uses.
        let argv = build_child_args(
            "--socket /explicit/other.sock node get abc",
            Path::new("/default.sock"),
            None,
        )
        .expect("valid shell syntax");
        assert_eq!(
            argv,
            vec![
                "--socket",
                "/default.sock",
                "--socket",
                "/explicit/other.sock",
                "node",
                "get",
                "abc",
                "--json",
            ]
        );
    }

    #[test]
    fn tools_list_exposes_exactly_one_tool_with_a_string_args_parameter() {
        let result = tools_list_result().expect("the real schema must pass validation");
        let tools = result["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 1, "exactly one tool must be exposed");
        assert_eq!(tools[0]["name"], TOOL_NAME);
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
        assert_eq!(
            tools[0]["inputSchema"]["properties"]["args"]["type"],
            "string"
        );
        assert_eq!(tools[0]["inputSchema"]["required"][0], "args");
    }

    #[test]
    fn tools_list_marks_provenance_as_external() {
        let result = tools_list_result().expect("the real schema must pass validation");
        assert_eq!(result["tools"][0]["_meta"]["source"], TOOL_SOURCE);
        assert_eq!(TOOL_SOURCE, "external");
    }

    // --- validate_tool_schema / check_schema_depth -------------------------

    #[test]
    fn validate_tool_schema_accepts_the_real_passthrough_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "args": {"type": "string", "description": "..."}
            },
            "required": ["args"],
        });
        assert!(validate_tool_schema(&schema).is_ok());
    }

    #[test]
    fn validate_tool_schema_rejects_unbounded_additional_properties_at_top_level() {
        let schema = json!({"type": "object", "additionalProperties": true});
        let err = validate_tool_schema(&schema).expect_err("must reject");
        assert!(err.contains("additionalProperties"), "got: {err}");
    }

    #[test]
    fn validate_tool_schema_rejects_unbounded_additional_properties_nested_inside_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "nested": {"type": "object", "additionalProperties": true}
            },
        });
        let err = validate_tool_schema(&schema).expect_err("must reject");
        assert!(err.contains("additionalProperties"), "got: {err}");
    }

    #[test]
    fn validate_tool_schema_accepts_additional_properties_false() {
        let schema = json!({"type": "object", "additionalProperties": false});
        assert!(validate_tool_schema(&schema).is_ok());
    }

    #[test]
    fn validate_tool_schema_accepts_a_bounded_object_valued_additional_properties() {
        let schema = json!({
            "type": "object",
            "additionalProperties": {"type": "string"},
        });
        assert!(validate_tool_schema(&schema).is_ok());
    }

    #[test]
    fn validate_tool_schema_rejects_excessive_nesting_depth() {
        // Build a schema nested one level deeper than MAX_SCHEMA_NESTING_DEPTH
        // via `properties`, so it must be rejected regardless of the exact
        // constant's value.
        let mut schema = json!({"type": "string"});
        for _ in 0..(MAX_SCHEMA_NESTING_DEPTH + 2) {
            schema = json!({"type": "object", "properties": {"x": schema}});
        }
        let err = validate_tool_schema(&schema).expect_err("must reject excessive nesting");
        assert!(err.contains("depth"), "got: {err}");
    }

    #[test]
    fn validate_tool_schema_accepts_nesting_at_exactly_the_bound() {
        // MAX_SCHEMA_NESTING_DEPTH levels of `properties` nesting must still
        // pass -- the bound is inclusive, not off-by-one.
        let mut schema = json!({"type": "string"});
        for _ in 0..MAX_SCHEMA_NESTING_DEPTH {
            schema = json!({"type": "object", "properties": {"x": schema}});
        }
        assert!(validate_tool_schema(&schema).is_ok());
    }

    #[test]
    fn validate_tool_schema_rejects_excessive_nesting_via_items() {
        let mut schema = json!({"type": "string"});
        for _ in 0..(MAX_SCHEMA_NESTING_DEPTH + 2) {
            schema = json!({"type": "array", "items": schema});
        }
        let err = validate_tool_schema(&schema).expect_err("must reject");
        assert!(err.contains("depth"), "got: {err}");
    }

    #[test]
    fn validate_tool_schema_rejects_excessive_nesting_via_any_of() {
        let mut schema = json!({"type": "string"});
        for _ in 0..(MAX_SCHEMA_NESTING_DEPTH + 2) {
            schema = json!({"anyOf": [schema]});
        }
        let err = validate_tool_schema(&schema).expect_err("must reject");
        assert!(err.contains("depth"), "got: {err}");
    }

    #[test]
    fn validate_tool_schema_rejects_a_schema_larger_than_the_byte_cap() {
        let huge_description = "x".repeat(MAX_SCHEMA_BYTES);
        let schema = json!({"type": "string", "description": huge_description});
        let err = validate_tool_schema(&schema).expect_err("must reject oversized schema");
        assert!(err.contains("bytes"), "got: {err}");
    }

    #[test]
    fn tools_list_surfaces_a_validation_failure_as_a_jsonrpc_internal_error() {
        // handle_message's "tools/list" arm must translate a hypothetical
        // future validation failure into a JSON-RPC error object, not panic
        // or silently serve an invalid schema -- exercised directly against
        // the error-mapping arithmetic rather than by actually breaking the
        // real (currently valid) schema.
        let err = validate_tool_schema(&json!({"additionalProperties": true}))
            .expect_err("fixture must be invalid");
        let response = error_response(
            json!(1),
            -32603,
            &format!("Internal error: passthrough tool schema failed validation: {err}"),
        );
        assert_eq!(response["error"]["code"], -32603);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("additionalProperties"));
    }

    // --- enforce_enabled -----------------------------------------------

    #[test]
    fn enforce_enabled_rejects_a_disabled_config_with_an_actionable_message() {
        let err =
            enforce_enabled(&McpConfig { enabled: false }).expect_err("disabled must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("not enabled"), "got: {msg}");
        assert!(msg.contains("nodespace mcp install"), "got: {msg}");
    }

    #[test]
    fn enforce_enabled_accepts_an_enabled_config() {
        assert!(enforce_enabled(&McpConfig { enabled: true }).is_ok());
    }

    #[test]
    fn extract_tool_call_rejects_unknown_tool_name() {
        let params = json!({"name": "not-nodespace", "arguments": {"args": "search x"}});
        let err = extract_tool_call(&params).expect_err("unknown tool must be rejected");
        assert!(err.contains("Unknown tool"));
    }

    #[test]
    fn extract_tool_call_rejects_missing_args_string() {
        let params = json!({"name": TOOL_NAME, "arguments": {}});
        let err = extract_tool_call(&params).expect_err("missing args must be rejected");
        assert!(err.contains("args"));
    }

    #[test]
    fn extract_tool_call_rejects_non_string_args() {
        let params = json!({"name": TOOL_NAME, "arguments": {"args": 5}});
        assert!(extract_tool_call(&params).is_err());
    }

    #[test]
    fn extract_tool_call_accepts_well_formed_params() {
        let params = json!({"name": TOOL_NAME, "arguments": {"args": "search x"}});
        assert_eq!(
            extract_tool_call(&params).expect("valid params"),
            "search x"
        );
    }

    #[test]
    fn clean_cli_error_strips_the_termination_prefix_and_caused_by_chain() {
        let raw = "Error: Could not connect to nodespaced at /tmp/x.sock.\nIs the daemon running? Start it with `nodespaced` in another terminal.\n\nCaused by:\n    0: transport error\n    1: No such file or directory (os error 2)";
        let cleaned = clean_cli_error(raw);
        assert_eq!(
            cleaned,
            "Could not connect to nodespaced at /tmp/x.sock.\nIs the daemon running? Start it with `nodespaced` in another terminal."
        );
        assert!(!cleaned.to_lowercase().contains("os error"));
    }

    #[test]
    fn clean_cli_error_leaves_non_anyhow_shaped_errors_untouched() {
        // clap's own usage errors start with a lowercase `error:`, not the
        // `Error: ` prefix std's Termination impl adds — must pass through.
        let raw = "error: unrecognized subcommand 'bogus'\n\nUsage: nodespace <COMMAND>";
        assert_eq!(clean_cli_error(raw), raw);
    }

    #[test]
    fn clean_cli_error_handles_a_bare_single_line_message() {
        assert_eq!(clean_cli_error("Error: invalid status"), "invalid status");
    }

    #[tokio::test]
    async fn handle_message_initialize_returns_protocol_version_and_server_info() {
        let response = handle_message(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            Path::new("/bin/true"),
            Path::new("/tmp/daemon.sock"),
            None,
        )
        .await
        .expect("initialize is a request, not a notification");
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(response["result"]["serverInfo"]["name"], "nodespace");
    }

    #[tokio::test]
    async fn handle_message_notification_gets_no_response() {
        let response = handle_message(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            Path::new("/bin/true"),
            Path::new("/tmp/daemon.sock"),
            None,
        )
        .await;
        assert!(
            response.is_none(),
            "JSON-RPC forbids responding to notifications"
        );
    }

    #[tokio::test]
    async fn handle_message_unknown_method_is_method_not_found() {
        let response = handle_message(
            r#"{"jsonrpc":"2.0","id":7,"method":"resources/list","params":{}}"#,
            Path::new("/bin/true"),
            Path::new("/tmp/daemon.sock"),
            None,
        )
        .await
        .expect("a request always gets a response");
        assert_eq!(response["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn handle_message_malformed_json_is_parse_error_with_null_id() {
        let response = handle_message(
            "not json at all",
            Path::new("/bin/true"),
            Path::new("/tmp/daemon.sock"),
            None,
        )
        .await
        .expect("a parse failure must still be reported");
        assert_eq!(response["error"]["code"], -32700);
        assert_eq!(response["id"], Value::Null);
    }

    #[tokio::test]
    async fn handle_message_tools_call_with_unknown_tool_is_invalid_params() {
        let response = handle_message(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"bogus","arguments":{}}}"#,
            Path::new("/bin/true"),
            Path::new("/tmp/daemon.sock"),
            None,
        )
        .await
        .expect("a request always gets a response");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn handle_message_tools_list_advertises_the_one_tool() {
        let response = handle_message(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}"#,
            Path::new("/bin/true"),
            Path::new("/tmp/daemon.sock"),
            None,
        )
        .await
        .expect("a request always gets a response");
        assert_eq!(response["result"]["tools"][0]["name"], TOOL_NAME);
    }
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::*;

    #[tokio::test]
    async fn run_child_captures_stdout_on_success() {
        let result = run_child(
            Path::new("/bin/echo"),
            &["hello".to_string()],
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(result["isError"], false);
        assert_eq!(result["content"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn run_child_surfaces_stderr_on_nonzero_exit() {
        let result = run_child(
            Path::new("/bin/sh"),
            &["-c".to_string(), "echo boom >&2; exit 1".to_string()],
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(result["isError"], true);
        assert_eq!(result["content"][0]["text"], "boom");
    }

    #[tokio::test]
    async fn run_child_times_out_and_kills_a_blocking_child() {
        let result = run_child(
            Path::new("/bin/sleep"),
            &["5".to_string()],
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("did not complete within"));
        assert!(text.contains("session launch"));
    }

    #[tokio::test]
    async fn run_child_reports_spawn_failure_actionably() {
        let result = run_child(
            Path::new("/no/such/binary-at-all"),
            &["node".to_string()],
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Failed to run the nodespace CLI"));
    }

    /// `cat` with no arguments reads its stdin until EOF and echoes it back.
    /// A null stdin (what `run_child` must give the dispatched process — see
    /// its doc comment for why) hits EOF immediately, so this returns fast
    /// with empty output; a real regression (dropping `stdin(Stdio::null())`
    /// so the child inherits this test process's own stdin) is only
    /// observable here when that inherited stream is itself open and
    /// unclosed, which this harness does not control — the primary
    /// protection against that regression is `run_child`'s explanatory doc
    /// comment plus the source-level fact that `tokio::process::Command`'s
    /// `output()`, unlike `std::process::Command`'s, does not null stdin on
    /// its own. This test still exercises the exact code path and pins the
    /// intended fast/empty behavior.
    #[tokio::test]
    async fn run_child_gives_the_dispatched_process_a_null_stdin() {
        let result = run_child(Path::new("/bin/cat"), &[], Duration::from_secs(5)).await;
        assert_eq!(result["isError"], false);
        assert_eq!(result["content"][0]["text"], "");
    }
}
