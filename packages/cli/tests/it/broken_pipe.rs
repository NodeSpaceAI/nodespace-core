//! `nodespace ... | head`: a closed stdout pipe must end the CLI quietly.
//!
//! Spawns the compiled binary against an in-process daemon with the read end
//! of its stdout already closed, so the first write fails with EPIPE exactly as
//! it does when `head` has exited.

use std::process::{Output, Stdio};
use std::time::Duration;

use nodespace_daemon::nodespace::CreateNodeRequest;
use tokio::process::Command;

use crate::cli_integration::spawn_routing_daemon;

const BROKEN_PIPE_EXIT_CODE: i32 = 141;

/// Run `nodespace --socket <sock> <args>` with stdout's reader gone.
async fn run_with_closed_stdout(
    sock: &std::path::Path,
    home: &std::path::Path,
    args: &[&str],
) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_nodespace"))
        .arg("--socket")
        .arg(sock)
        .args(args)
        .env("NODESPACE_HOME", home)
        .env("HOME", home)
        .env_remove("NODESPACE_DATABASE")
        .env_remove("NODESPACED_SOCKET")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn nodespace");
    // Close the read end before the CLI can write: the reader has "exited".
    drop(child.stdout.take());
    tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .expect("nodespace finished")
        .expect("nodespace ran")
}

#[tokio::test]
async fn closed_stdout_exits_quietly_without_a_panic() {
    let (sock, _shutdown, home) = spawn_routing_daemon().await;
    let mut seed = nodespace_cli::connect(&sock, nodespace_cli::DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    // Enough output to exceed a pipe buffer if the reader were alive.
    for i in 0..300 {
        seed.create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: format!("node number {i} {}", "x".repeat(300)),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed node");
    }

    let out = run_with_closed_stdout(&sock, home.path(), &["query", "--type", "text"]).await;
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        !stderr.contains("panicked"),
        "panic text on stderr: {stderr}"
    );
    assert!(
        !stderr.contains("Broken pipe"),
        "broken pipe reported: {stderr}"
    );
    assert_eq!(out.status.code(), Some(BROKEN_PIPE_EXIT_CODE), "{stderr}");
}
