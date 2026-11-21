//! `nodespace tool approve` through the compiled binary: without a terminal it
//! is refused before any daemon is contacted, so a program a tool runs (which
//! has no terminal) cannot approve a tool through the command line.

use std::process::Stdio;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

#[tokio::test]
async fn approving_without_a_terminal_is_refused_before_the_daemon_is_contacted() {
    let home = TempDir::new().unwrap();
    // No daemon listens here: a refusal that came after connecting would be a
    // connection error instead.
    let sock = home.path().join("absent.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_nodespace"))
        .arg("--socket")
        .arg(&sock)
        .args(["tool", "approve", "some-node"])
        .env("NODESPACE_HOME", home.path())
        .env("HOME", home.path())
        .env_remove("NODESPACE_DATABASE")
        .env_remove("NODESPACED_SOCKET")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("nodespace ran");
    // The name typed on a pipe is not an approval.
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(b"lookup\n").await.unwrap();
    drop(pipe);
    let out = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .expect("nodespace finished")
        .expect("nodespace output");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("must be done at a terminal"), "{text}");
}
