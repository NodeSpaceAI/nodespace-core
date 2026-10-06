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

use nodespace_daemon::nodespace::{
    CreateDatabaseRequest, GetNodeRequest, UpdateDatabaseSettingsNodeRequest,
};
use nodespace_types::DATABASE_SETTINGS_NODE_ID;
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

/// `nodespace --socket <sock> [--database <selection>] mcp ...`, with `$HOME`
/// isolated and no `NODESPACE_HOME`.
fn nodespace_mcp(sock: &Path, database: Option<&str>, home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nodespace"));
    command.arg("--socket").arg(sock);
    if let Some(database) = database {
        command.arg("--database").arg(database);
    }
    command
        .arg("mcp")
        .env("HOME", home)
        .env_remove("NODESPACE_HOME");
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

/// Set `external_tools_enabled` on the settings node of the database selected
/// (the daemon's default when `None`), through the typed update the CLI and
/// the Settings screen use.
async fn set_external_tools(sock: &Path, database: Option<&str>, enabled: bool) {
    let (interceptor, _) = nodespace_cli::resolve_routing(sock, database)
        .await
        .expect("resolve routing");
    let mut client = nodespace_cli::connect(sock, interceptor)
        .await
        .expect("connect");
    let node = client
        .get_node(GetNodeRequest {
            node_id: DATABASE_SETTINGS_NODE_ID.to_string(),
        })
        .await
        .expect("read the settings node")
        .into_inner()
        .node_data
        .expect("settings node data");
    client
        .update_database_settings_node(UpdateDatabaseSettingsNodeRequest {
            node_id: DATABASE_SETTINGS_NODE_ID.to_string(),
            version: node.version,
            update_json: json!({ "externalToolsEnabled": enabled }).to_string(),
        })
        .await
        .expect("write external_tools_enabled");
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
    set_external_tools(&sock, None, true).await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, None, home.path()));

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

/// With no daemon to read the database's settings from, the server cannot
/// tell whether tools are enabled, so it refuses to start rather than serve.
#[tokio::test]
async fn mcp_refuses_to_start_when_the_daemon_is_unreachable() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    // A path inside a fresh, empty tempdir guarantees nothing is listening —
    // the daemon-unreachable path triggers deterministically, regardless of
    // whether some other daemon happens to be running on the host executing
    // this test.
    let sock = tempdir.path().join("no-such-daemon.sock");

    let output = timeout(
        Duration::from_secs(15),
        spawn_server(nodespace_mcp(&sock, None, tempdir.path())).wait_with_output(),
    )
    .await
    .expect("mcp server did not exit promptly")
    .expect("wait on child");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Could not connect to nodespaced"),
        "expected the CLI's own friendly connection error, got: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "a server that cannot start must never write to stdout (the JSON-RPC transport)"
    );
}

#[tokio::test]
async fn mcp_tool_call_reports_a_daemon_that_went_away_actionably_not_as_a_raw_connection_error() {
    let (sock, shutdown, _daemon_dir) = spawn_routing_daemon().await;
    set_external_tools(&sock, None, true).await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, None, home.path()));
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
    set_external_tools(&sock, None, true).await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, None, home.path()));

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

/// ADR-038 Trust Boundary, proven end-to-end against the real binary rather
/// than only at the unit level (`enforce_enabled`'s own tests): a database
/// that has never run `nodespace mcp install` has `external_tools_enabled`
/// off, and the server must refuse to serve anything and exit non-zero with
/// an actionable message naming the database, rather than accepting a
/// connection and only gating individual calls.
#[tokio::test]
async fn mcp_refuses_to_start_when_not_enabled() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    let home = isolated_home();

    let output = timeout(
        Duration::from_secs(15),
        spawn_server(nodespace_mcp(&sock, None, home.path())).wait_with_output(),
    )
    .await
    .expect("mcp server did not exit promptly when disabled")
    .expect("wait on child");

    assert!(
        !output.status.success(),
        "a disabled passthrough tool must exit non-zero, got {}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not enabled"),
        "expected an actionable disabled-state message on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("database 'Default'"),
        "expected the message to name the database, got: {stderr}"
    );
    assert!(
        stderr.contains("nodespace mcp install"),
        "expected the message to name the fix, got: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "a disabled server must never write to stdout (the JSON-RPC transport) at all, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Same disabled state, but explicit: a database whose flag was turned on and
/// then off behaves like one that never had it on.
#[tokio::test]
async fn mcp_refuses_to_start_when_explicitly_disabled() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    set_external_tools(&sock, None, true).await;
    set_external_tools(&sock, None, false).await;
    let home = isolated_home();

    let output = timeout(
        Duration::from_secs(15),
        spawn_server(nodespace_mcp(&sock, None, home.path())).wait_with_output(),
    )
    .await
    .expect("mcp server did not exit promptly when disabled")
    .expect("wait on child");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not enabled"));
}

/// Two databases open in one daemon each behave by their own setting
/// (ADR-095): enabling external tools for one leaves the other refusing.
#[tokio::test]
async fn mcp_serves_only_the_databases_that_enabled_external_tools() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    create_database(&sock, "Second").await;
    set_external_tools(&sock, Some("Second"), true).await;
    let home = isolated_home();

    // The default database never enabled them: it refuses.
    let refused = timeout(
        Duration::from_secs(15),
        spawn_server(nodespace_mcp(&sock, None, home.path())).wait_with_output(),
    )
    .await
    .expect("mcp server did not exit promptly when disabled")
    .expect("wait on child");
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("database 'Default'"), "got: {stderr}");

    // The second one did: it serves.
    let mut child = spawn_server(nodespace_mcp(&sock, Some("Second"), home.path()));
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let init = send_and_read(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    )
    .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "nodespace");
    drop(stdin);
    wait_for_clean_exit(child).await;
}

/// A server is bound to the database it checked at start: a call that names
/// another database (one that never enabled external tools) is refused before
/// anything is dispatched, so a client cannot reach it or enable tools there.
#[tokio::test]
async fn mcp_refuses_a_call_that_names_a_database_that_never_enabled_tools() {
    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    create_database(&sock, "Second").await;
    set_external_tools(&sock, Some("Second"), true).await;
    let home = isolated_home();
    let mut child = spawn_server(nodespace_mcp(&sock, Some("Second"), home.path()));
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    for (id, args) in [
        (1, "--database Default node update database-settings-singleton --property external_tools_enabled=true"),
        (2, "node get database-settings-singleton --database=Default"),
        (3, "--socket /elsewhere.sock node get database-settings-singleton"),
    ] {
        let call = send_and_read(
            &mut stdin,
            &mut stdout,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": "nodespace", "arguments": {"args": args}},
            }),
        )
        .await;
        assert_eq!(call["result"]["isError"], true, "{args}: {call}");
        let text = call["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("cannot be set in a call"), "{args}: {text}");
    }

    // The default database was never enabled and is still not.
    drop(stdin);
    wait_for_clean_exit(child).await;
    let output = timeout(
        Duration::from_secs(15),
        spawn_server(nodespace_mcp(&sock, None, home.path())).wait_with_output(),
    )
    .await
    .expect("mcp server did not exit promptly when disabled")
    .expect("wait on child");
    assert!(!output.status.success());
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

/// End-to-end proof of the full explicit-user-enablement lifecycle described
/// in `commands::mcp`'s doc comment: `nodespace mcp install` writes Claude
/// Desktop's real config file and sets the database's `external_tools_enabled`
/// setting via the real (compiled-from-TypeScript) installer script -- not a
/// mock of either side -- after which the server actually accepts a
/// connection, and `nodespace mcp uninstall` reverses both, after which it
/// refuses again. Each command says which database it acted on.
///
/// Skipped (not failed) when `packages/skill/dist/install.js` isn't built --
/// mirrors `commands::skill::tests::
/// resolve_script_installer_errors_with_actionable_message_when_missing`'s
/// own reasoning: CI does not run `bun run build:skill`/`bun run build`
/// before `cargo test`, so this is exercised locally (`bun run build` inside
/// `packages/skill`, then `cargo test`) rather than unconditionally in CI.
#[tokio::test]
async fn mcp_install_uninstall_round_trip_against_the_real_installer_enables_and_disables_the_live_server(
) {
    let dist_install = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("skill")
        .join("dist")
        .join("install.js");
    if !dist_install.exists() {
        eprintln!(
            "SKIPPING mcp_install_uninstall_round_trip...: {} not built \
             (run `bun run build` inside packages/skill, or `bun run build:skill` from the repo \
             root, then re-run this test).",
            dist_install.display()
        );
        return;
    }

    let (sock, _shutdown, _daemon_dir) = spawn_routing_daemon().await;
    let home = tempfile::tempdir().expect("tempdir");
    let claude_dir = home
        .path()
        .join("Library")
        .join("Application Support")
        .join("Claude");
    std::fs::create_dir_all(&claude_dir).expect("create fake Claude Desktop dir");
    let path_with_fake_nodespace = fake_nodespace_on_path(home.path());
    let config_path = claude_dir.join("claude_desktop_config.json");

    // Before install: status reports disabled and no config file exists.
    let status_before = nodespace_mcp(&sock, None, home.path())
        .arg("status")
        .output()
        .await
        .expect("run mcp status");
    assert!(status_before.status.success());
    let status_before_stdout = String::from_utf8_lossy(&status_before.stdout);
    assert!(
        status_before_stdout.contains("database 'Default'")
            && status_before_stdout.contains("enabled for database 'Default'")
            && status_before_stdout.contains(": no"),
        "got: {status_before_stdout}"
    );
    assert!(!config_path.exists());

    // Install: writes the client config and enables the setting.
    let install_out = nodespace_mcp(&sock, None, home.path())
        .arg("install")
        .arg("--yes")
        .env("PATH", &path_with_fake_nodespace)
        .output()
        .await
        .expect("run mcp install");
    assert!(
        install_out.status.success(),
        "install must succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&install_out.stdout),
        String::from_utf8_lossy(&install_out.stderr)
    );
    let install_stdout = String::from_utf8_lossy(&install_out.stdout);
    assert!(
        install_stdout.contains("claude-desktop") && install_stdout.contains("MCP config written"),
        "got: {install_stdout}"
    );
    assert!(
        install_stdout.contains("now enabled for database 'Default'"),
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

    // The settings node now has the field set, and mcp status agrees.
    let mut client = {
        let (interceptor, _) = nodespace_cli::resolve_routing(&sock, None)
            .await
            .expect("resolve routing");
        nodespace_cli::connect(&sock, interceptor)
            .await
            .expect("connect")
    };
    let settings_node = client
        .get_node(GetNodeRequest {
            node_id: DATABASE_SETTINGS_NODE_ID.to_string(),
        })
        .await
        .expect("read settings node")
        .into_inner()
        .node_data
        .expect("node data");
    assert!(
        settings_node
            .properties
            .contains("\"external_tools_enabled\":true"),
        "got: {}",
        settings_node.properties
    );

    let status_after_install = nodespace_mcp(&sock, None, home.path())
        .arg("status")
        .output()
        .await
        .expect("run mcp status");
    let status_stdout = String::from_utf8_lossy(&status_after_install.stdout);
    assert!(status_stdout.contains(": yes"), "got: {status_stdout}");
    assert!(
        status_stdout.contains("claude-desktop"),
        "got: {status_stdout}"
    );

    // The server now actually accepts a connection and answers initialize --
    // proving install didn't just flip a flag but genuinely unblocked the
    // trust-boundary gate `run_server` enforces.
    let mut child = spawn_server(nodespace_mcp(&sock, None, home.path()));
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let init = send_and_read(
        &mut stdin,
        &mut stdout,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    )
    .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "nodespace");
    drop(stdin);
    wait_for_clean_exit(child).await;

    // Uninstall: removes the config entry and disables the setting again.
    let uninstall_out = nodespace_mcp(&sock, None, home.path())
        .arg("uninstall")
        .output()
        .await
        .expect("run mcp uninstall");
    assert!(uninstall_out.status.success());
    let uninstall_stdout = String::from_utf8_lossy(&uninstall_out.stdout);
    assert!(
        uninstall_stdout.contains("claude-desktop")
            && uninstall_stdout.contains("MCP config removed"),
        "got: {uninstall_stdout}"
    );
    assert!(
        uninstall_stdout.contains("now disabled for database 'Default'"),
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

    // The server refuses again -- uninstall genuinely re-closed the gate,
    // not just the client config.
    let output = spawn_server(nodespace_mcp(&sock, None, home.path()))
        .wait_with_output()
        .await
        .expect("wait on child");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not enabled"));
}
