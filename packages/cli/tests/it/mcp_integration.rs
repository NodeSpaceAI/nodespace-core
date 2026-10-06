//! End-to-end test of `nodespace mcp`'s stdio transport.
//!
//! Spawns the real compiled `nodespace` binary (via `CARGO_BIN_EXE_nodespace`,
//! not the library surface used by `cli_integration.rs`) and speaks JSON-RPC
//! over its actual stdin/stdout — this is the one CLI surface where the OS
//! process boundary and stdio framing are the thing under test, not
//! incidental plumbing to route around.
//!
//! Unix-only for now: this test's process-spawn/stdio harness has not been
//! exercised on Windows. `nodespace` itself now supports Windows (Named Pipe
//! transport), so enabling this test there is a matter of verifying the
//! harness, not of the CLI's own platform support.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use nodespace_daemon::nodespace::CreateDatabaseRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

use crate::cli_integration::spawn_routing_daemon;

/// Sends one JSON-RPC request line to the child's stdin and reads exactly
/// one response line from its stdout.
async fn send_and_read(
    stdin: &mut (impl tokio::io::AsyncWrite + Unpin),
    stdout: &mut (impl AsyncBufReadExt + Unpin),
    request: Value,
) -> Value {
    let line = serde_json::to_string(&request).expect("serialize request");
    stdin
        .write_all(line.as_bytes())
        .await
        .expect("write request");
    stdin.write_all(b"\n").await.expect("write newline");
    stdin.flush().await.expect("flush stdin");

    let mut response_line = String::new();
    timeout(
        Duration::from_secs(15),
        stdout.read_line(&mut response_line),
    )
    .await
    .expect("mcp server did not respond in time")
    .expect("read response line");
    assert!(
        !response_line.trim().is_empty(),
        "expected a JSON-RPC response line, got EOF"
    );
    serde_json::from_str(response_line.trim())
        .unwrap_or_else(|e| panic!("response line was not valid JSON ({e}): {response_line:?}"))
}

async fn wait_for_clean_exit(mut child: Child) {
    let status = timeout(Duration::from_secs(15), child.wait())
        .await
        .expect("mcp server did not exit after stdin closed")
        .expect("wait on child");
    assert!(
        status.success(),
        "mcp server must exit 0 once stdin closes cleanly, got {status}"
    );
}

/// An isolated `$HOME`, so no test reads or writes the real one.
fn isolated_home() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// `nodespace --socket <sock> mcp ...`, with `$HOME` isolated and no
/// `NODESPACE_HOME` or `NODESPACE_DATABASE`.
fn nodespace_mcp(sock: &Path, home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nodespace"));
    command.arg("--socket").arg(sock);
    command
        .arg("mcp")
        .env("HOME", home)
        .env_remove("NODESPACE_HOME")
        .env_remove("NODESPACE_DATABASE");
    command
}

fn spawn_server(mut command: Command) -> Child {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn nodespace mcp")
}

/// Run `nodespace --socket <sock> <args>` directly (not through the server),
/// the way the user does, and return its stdout; fails the test on a non-zero
/// exit.
async fn run_cli(sock: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nodespace"))
        .arg("--socket")
        .arg(sock)
        .args(args)
        .env_remove("NODESPACE_DATABASE")
        .output()
        .await
        .expect("run nodespace");
    assert!(
        output.status.success(),
        "nodespace {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Send one `tools/call` through the server and return its result object.
async fn call_tool(
    stdin: &mut (impl tokio::io::AsyncWrite + Unpin),
    stdout: &mut (impl AsyncBufReadExt + Unpin),
    id: u64,
    args: &str,
) -> Value {
    let response = send_and_read(
        stdin,
        stdout,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": "nodespace", "arguments": {"args": args}},
        }),
    )
    .await;
    response["result"].clone()
}

/// The text of a successful tool result, parsed as JSON.
fn result_json(result: &Value) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    serde_json::from_str(result["content"][0]["text"].as_str().expect("text")).expect("json")
}

async fn create_database(sock: &Path, name: &str) -> String {
    let mut registry = nodespace_cli::connect_database(sock)
        .await
        .expect("connect registry");
    registry
        .create(CreateDatabaseRequest {
            name: name.to_string(),
            path: None,
        })
        .await
        .expect("create database")
        .into_inner()
        .id
}

#[tokio::test]
async fn mcp_speaks_stdio_jsonrpc_and_exposes_exactly_one_tool() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, home.path()));

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let init = send_and_read(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    )
    .await;
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "nodespace");
    assert_eq!(init["result"]["capabilities"]["tools"], json!({}));

    // A notification (no "id") must draw no response line at all — send it,
    // then immediately follow with a real request and confirm the very next
    // line answers that request, not a stray reply to the notification.
    let notify =
        serde_json::to_string(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .unwrap();
    stdin.write_all(notify.as_bytes()).await.unwrap();
    stdin.write_all(b"\n").await.unwrap();
    stdin.flush().await.unwrap();

    let list = send_and_read(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    )
    .await;
    assert_eq!(list["id"], 2);
    let tools = list["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1, "exactly one tool must be exposed");
    assert_eq!(tools[0]["name"], "nodespace");
    assert_eq!(
        tools[0]["inputSchema"]["properties"]["args"]["type"],
        "string"
    );
    assert_eq!(tools[0]["inputSchema"]["required"][0], "args");

    drop(stdin);
    wait_for_clean_exit(child).await;
}

/// The server reads no settings and binds no database, so it starts and
/// answers the protocol with no daemon running; a call then reports the
/// missing daemon as a tool error.
#[tokio::test]
async fn mcp_serves_with_the_daemon_down_at_start() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    // A path inside a fresh, empty tempdir guarantees nothing is listening,
    // regardless of what daemon happens to run on the host.
    let sock = tempdir.path().join("no-such-daemon.sock");
    let mut child = spawn_server(nodespace_mcp(&sock, tempdir.path()));
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let init = send_and_read(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    )
    .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "nodespace");

    let result = call_tool(&mut stdin, &mut stdout, 2, "node get some-id").await;
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("Could not connect to nodespaced"),
        "{result}"
    );

    drop(stdin);
    wait_for_clean_exit(child).await;
}

#[tokio::test]
async fn mcp_tool_call_reports_a_daemon_that_went_away_actionably_not_as_a_raw_connection_error() {
    let (sock, shutdown, _daemon_dir) = spawn_routing_daemon().await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, home.path()));
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let init = send_and_read(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    )
    .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "nodespace");

    // The server started against a running daemon; now nothing is listening.
    let _ = shutdown.send(());
    tokio::time::sleep(Duration::from_millis(300)).await;

    let call = send_and_read(
        &mut stdin,
        &mut stdout,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "nodespace", "arguments": {"args": "node get some-id"}},
        }),
    )
    .await;

    assert_eq!(
        call["result"]["isError"], true,
        "an unreachable daemon must surface as a tool error, not a JSON-RPC error or success: {call}"
    );
    let text = call["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    assert!(
        text.contains("Could not connect to nodespaced"),
        "expected the CLI's own friendly connection error, got: {text}"
    );
    assert!(
        text.contains("Is the daemon running?") && text.contains("nodespaced"),
        "expected an actionable hint naming the `nodespaced` start step, got: {text}"
    );
    assert!(
        !text.to_lowercase().contains("os error")
            && !text.to_lowercase().contains("connection refused")
            && !text.to_lowercase().contains("no such file or directory"),
        "must not leak the raw OS-level connection error text: {text}"
    );

    drop(stdin);
    wait_for_clean_exit(child).await;
}

#[tokio::test]
async fn mcp_tool_call_rejects_an_unterminated_quote_without_dispatching_anything() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, home.path()));

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let call = send_and_read(
        &mut stdin,
        &mut stdout,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {"name": "nodespace", "arguments": {"args": "search \"unterminated"}},
        }),
    )
    .await;

    assert_eq!(call["result"]["isError"], true);
    let text = call["result"]["content"][0]["text"]
        .as_str()
        .expect("text content");
    assert!(text.contains("could not parse"), "got: {text}");

    drop(stdin);
    wait_for_clean_exit(child).await;
}

/// A call acts on the daemon's active database at that moment: after
/// `database use <other>` the next call, from the same running server,
/// operates on `<other>`, and after switching back it operates on the first.
#[tokio::test]
async fn mcp_acts_on_the_active_database_at_each_call() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    create_database(&sock, "Second").await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, home.path()));
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let create = "node create --type text --content marker";
    let in_default = result_json(&call_tool(&mut stdin, &mut stdout, 1, create).await)["id"]
        .as_str()
        .expect("id")
        .to_string();

    run_cli(&sock, &["database", "use", "Second"]).await;
    let get_default_node = format!("node get {in_default}");
    let missing = call_tool(&mut stdin, &mut stdout, 2, &get_default_node).await;
    assert_eq!(
        missing["isError"], true,
        "after `database use Second` the first database's node must not be reachable: {missing}"
    );
    let in_second = result_json(&call_tool(&mut stdin, &mut stdout, 3, create).await)["id"]
        .as_str()
        .expect("id")
        .to_string();
    run_cli(&sock, &["--database", "Second", "node", "get", &in_second]).await;

    run_cli(&sock, &["database", "use", "Default"]).await;
    let back = call_tool(&mut stdin, &mut stdout, 4, &get_default_node).await;
    assert_eq!(back["isError"], false, "{back}");

    drop(stdin);
    wait_for_clean_exit(child).await;
}

/// A database named in the environment the server started with does not pin
/// its calls: they still act on the active database.
#[tokio::test]
async fn mcp_ignores_a_database_named_in_its_environment() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    let second = create_database(&sock, "Second").await;
    let home = isolated_home();
    let mut command = nodespace_mcp(&sock, home.path());
    command.env("NODESPACE_DATABASE", &second);
    let mut child = spawn_server(command);
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let created = call_tool(
        &mut stdin,
        &mut stdout,
        1,
        "node create --type text --content marker",
    )
    .await;
    let id = result_json(&created)["id"]
        .as_str()
        .expect("id")
        .to_string();
    // The active database is the default: the node is there, not in Second.
    run_cli(&sock, &["node", "get", &id]).await;

    drop(stdin);
    wait_for_clean_exit(child).await;
}

/// A call cannot select a database or another daemon, and cannot run the
/// `database` subcommand; none of these reaches the daemon, so the active
/// database is unchanged.
#[tokio::test]
async fn mcp_refuses_database_selection_other_sockets_and_the_database_subcommand() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    create_database(&sock, "Second").await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, home.path()));
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    for (id, args, expected) in [
        (
            1,
            "--database Second node query",
            "--database cannot be set",
        ),
        (
            2,
            "node query --database=Second",
            "--database cannot be set",
        ),
        (
            3,
            "--socket /elsewhere.sock node query",
            "--socket cannot name another daemon",
        ),
        (4, "database use Second", "`database` subcommand"),
        (5, "nodespace database list", "`database` subcommand"),
    ] {
        let result = call_tool(&mut stdin, &mut stdout, id, args).await;
        assert_eq!(result["isError"], true, "{args}: {result}");
        let text = result["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains(expected), "{args}: {text}");
    }

    let listed = run_cli(&sock, &["--json", "database", "list"]).await;
    let listed: Value = serde_json::from_str(&listed).expect("json");
    let default = listed["databases"]
        .as_array()
        .expect("array")
        .iter()
        .find(|d| d["is_default"] == true)
        .expect("a default database");
    assert_eq!(default["name"], "Default", "{listed}");

    drop(stdin);
    wait_for_clean_exit(child).await;
}

/// A delete preview's printed `confirm_command` replays through the tool and
/// deletes what the preview showed.
#[tokio::test]
async fn mcp_replays_a_printed_delete_confirm_command() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, home.path()));
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    let created = call_tool(
        &mut stdin,
        &mut stdout,
        1,
        "node create --type text --content doomed",
    )
    .await;
    let id = result_json(&created)["id"]
        .as_str()
        .expect("id")
        .to_string();
    let preview =
        result_json(&call_tool(&mut stdin, &mut stdout, 2, &format!("node delete {id}")).await);
    assert_eq!(preview["deleted"], false, "{preview}");
    let confirm = preview["confirm_command"]
        .as_str()
        .expect("confirm_command");
    let deleted = result_json(&call_tool(&mut stdin, &mut stdout, 3, confirm).await);
    assert_eq!(deleted["deleted_count"], 1, "{deleted}");
    assert_eq!(deleted["node_id"], id.as_str(), "{deleted}");

    drop(stdin);
    wait_for_clean_exit(child).await;
}

/// A fake, always-resolvable `nodespace` executable placed first on `PATH`,
/// so `resolveNodespaceBinaryPath`'s real `which nodespace` call (in
/// `packages/skill/src/mcp-installer.ts`) deterministically finds this
/// rather than whatever may or may not be installed system-wide on the
/// machine running this test. Its own content never runs -- Claude Desktop
/// never actually gets launched by this test -- only its path (which `which`
/// reports) matters.
fn fake_nodespace_on_path(dir: &std::path::Path) -> String {
    let bin_dir = dir.join("fakebin");
    std::fs::create_dir_all(&bin_dir).expect("create fake bin dir");
    let fake_exe = bin_dir.join("nodespace");
    std::fs::write(&fake_exe, "#!/bin/sh\necho fake\n").expect("write fake nodespace");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_exe, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake nodespace");
    }
    let existing_path = std::env::var("PATH").unwrap_or_default();
    format!("{}:{existing_path}", bin_dir.display())
}

/// End-to-end proof of the explicit-user-enablement lifecycle described in
/// `commands::mcp`'s doc comment, with no daemon at all: `nodespace mcp
/// install` writes Claude Desktop's real config file via the real
/// (compiled-from-TypeScript) installer script, `status` reports it, and
/// `uninstall` removes it. Every command points at a socket nothing listens
/// on, so none of them may need the daemon.
///
/// Skipped (not failed) when `packages/skill/dist/install.js` isn't built;
/// the merge gate's skill-installer tier builds it first.
#[tokio::test]
async fn mcp_install_status_uninstall_round_trip_needs_no_daemon() {
    let dist_install = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("skill")
        .join("dist")
        .join("install.js");
    if !dist_install.exists() {
        eprintln!(
            "SKIPPING mcp_install_status_uninstall_round_trip...: {} not built \
             (run `bun run build` inside packages/skill, or `bun run build:skill` from the repo \
             root, then re-run this test).",
            dist_install.display()
        );
        return;
    }

    let home = tempfile::tempdir().expect("tempdir");
    let sock = home.path().join("no-such-daemon.sock");
    let claude_dir = home
        .path()
        .join("Library")
        .join("Application Support")
        .join("Claude");
    std::fs::create_dir_all(&claude_dir).expect("create fake Claude Desktop dir");
    let path_with_fake_nodespace = fake_nodespace_on_path(home.path());
    let config_path = claude_dir.join("claude_desktop_config.json");

    // Before install: status reports disabled and no config file exists.
    let status_before = nodespace_mcp(&sock, home.path())
        .arg("status")
        .output()
        .await
        .expect("run mcp status");
    assert!(
        status_before.status.success(),
        "status must succeed with no daemon: {}",
        String::from_utf8_lossy(&status_before.stderr)
    );
    let status_before_stdout = String::from_utf8_lossy(&status_before.stdout);
    assert!(
        status_before_stdout.contains("enabled: no") && !status_before_stdout.contains("atabase"),
        "got: {status_before_stdout}"
    );
    assert!(!config_path.exists());

    // Install: writes the client config, with no daemon and no database named.
    let install_out = nodespace_mcp(&sock, home.path())
        .arg("install")
        .arg("--yes")
        .env("PATH", &path_with_fake_nodespace)
        .output()
        .await
        .expect("run mcp install");
    assert!(
        install_out.status.success(),
        "install must succeed with no daemon: stdout={} stderr={}",
        String::from_utf8_lossy(&install_out.stdout),
        String::from_utf8_lossy(&install_out.stderr)
    );
    let install_stdout = String::from_utf8_lossy(&install_out.stdout);
    assert!(
        install_stdout.contains("claude-desktop") && install_stdout.contains("MCP config written"),
        "got: {install_stdout}"
    );
    assert!(
        install_stdout.contains("passthrough tool is enabled") && !install_stdout.contains("'"),
        "got: {install_stdout}"
    );

    // The real Claude Desktop config file now points at the fake resolved
    // nodespace path with the right args.
    let written_config: Value = serde_json::from_str(
        &std::fs::read_to_string(&config_path).expect("read written claude_desktop_config.json"),
    )
    .expect("written config is valid JSON");
    assert_eq!(written_config["mcpServers"]["nodespace"]["args"][0], "mcp");
    assert!(written_config["mcpServers"]["nodespace"]["command"]
        .as_str()
        .expect("command is a string")
        .ends_with("fakebin/nodespace"));

    // Status now reports the config as present: that is the enablement.
    let status_after_install = nodespace_mcp(&sock, home.path())
        .arg("status")
        .output()
        .await
        .expect("run mcp status");
    let status_stdout = String::from_utf8_lossy(&status_after_install.stdout);
    assert!(
        status_stdout.contains("enabled: yes"),
        "got: {status_stdout}"
    );
    assert!(
        status_stdout.contains("claude-desktop"),
        "got: {status_stdout}"
    );

    // Uninstall works with the daemon stopped and removes the config entry.
    let uninstall_out = nodespace_mcp(&sock, home.path())
        .arg("uninstall")
        .output()
        .await
        .expect("run mcp uninstall");
    assert!(
        uninstall_out.status.success(),
        "uninstall must succeed with no daemon: {}",
        String::from_utf8_lossy(&uninstall_out.stderr)
    );
    let uninstall_stdout = String::from_utf8_lossy(&uninstall_out.stdout);
    assert!(
        uninstall_stdout.contains("claude-desktop")
            && uninstall_stdout.contains("MCP config removed"),
        "got: {uninstall_stdout}"
    );

    let written_config_after: Value = serde_json::from_str(
        &std::fs::read_to_string(&config_path).expect("config file must survive uninstall"),
    )
    .expect("valid JSON");
    assert!(
        written_config_after["mcpServers"]["nodespace"].is_null(),
        "the nodespace entry must be removed, got: {written_config_after}"
    );

    let status_after_uninstall = nodespace_mcp(&sock, home.path())
        .arg("status")
        .output()
        .await
        .expect("run mcp status");
    assert!(
        String::from_utf8_lossy(&status_after_uninstall.stdout).contains("enabled: no"),
        "got: {}",
        String::from_utf8_lossy(&status_after_uninstall.stdout)
    );
}
