//! Stdio NDJSON transport for ACP agent processes.
//!
//! Spawns an external agent binary as a child process with stdin/stdout/stderr piped,
//! and implements bidirectional JSON-RPC 2.0 communication using newline-delimited JSON.
//!
//! Internal architecture uses two tokio tasks (reader + writer) connected via mpsc
//! channels, enabling concurrent send/receive without deadlock. Stderr is captured
//! by a third background task and logged via `tracing` at warn level.
//!
//! Issue #1001: ACP stdio NDJSON transport layer.

use crate::agent_types::{AcpMessage, AcpTransport, TransportError};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex, Notify};
use tracing::{debug, error, info, warn};

/// Configuration for spawning an ACP agent subprocess.
#[derive(Debug, Clone)]
pub struct StdioTransportConfig {
    /// Path to the agent binary.
    pub binary: String,
    /// Command-line arguments.
    pub args: Vec<String>,
    /// Environment variables to set (in addition to inherited environment).
    pub env: HashMap<String, String>,
    /// Working directory for the subprocess. If `None`, inherits the parent's cwd.
    pub working_dir: Option<String>,
}

/// Internal state shared between the transport and its background tasks.
struct TransportInner {
    /// Sender for outgoing messages (consumed by the writer task).
    write_tx: mpsc::Sender<AcpMessage>,
    /// Receiver for incoming messages (produced by the reader task).
    read_rx: Mutex<mpsc::Receiver<AcpMessage>>,
    /// Handle to the child process for lifecycle management.
    child: Mutex<Option<Child>>,
    /// Signaled when shutdown is requested.
    shutdown: Notify,
    /// Tracks whether the transport has been shut down.
    is_shutdown: Mutex<bool>,
}

/// Stdio-based NDJSON transport implementing [`AcpTransport`].
///
/// Manages a child process with piped stdin/stdout/stderr. Messages are serialized
/// as single-line JSON (NDJSON) with a trailing newline. Background tasks handle
/// the actual I/O, decoupled from callers via mpsc channels.
pub struct StdioTransport {
    inner: Arc<TransportInner>,
    /// Join handle for the writer task (held for shutdown coordination).
    _writer_handle: tokio::task::JoinHandle<()>,
    /// Join handle for the reader task.
    _reader_handle: tokio::task::JoinHandle<()>,
    /// Join handle for the stderr capture task.
    _stderr_handle: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport").finish_non_exhaustive()
    }
}

impl StdioTransport {
    /// Spawn a new agent subprocess and start background I/O tasks.
    ///
    /// Returns a connected `StdioTransport` or a `TransportError` if the
    /// process could not be spawned.
    pub async fn spawn(config: StdioTransportConfig) -> Result<Self, TransportError> {
        info!(
            binary = %config.binary,
            args = ?config.args,
            "Spawning ACP agent subprocess"
        );

        let mut cmd = Command::new(&config.binary);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        if let Some(ref dir) = config.working_dir {
            cmd.current_dir(dir);
        }

        let mut child = cmd.spawn().map_err(|e| {
            TransportError::SendFailed(format!("failed to spawn agent process: {e}"))
        })?;

        // Take ownership of piped handles before moving child into shared state.
        let child_stdin = child.stdin.take().ok_or_else(|| {
            TransportError::SendFailed("failed to open stdin pipe".to_string())
        })?;
        let child_stdout = child.stdout.take().ok_or_else(|| {
            TransportError::SendFailed("failed to open stdout pipe".to_string())
        })?;
        let child_stderr = child.stderr.take().ok_or_else(|| {
            TransportError::SendFailed("failed to open stderr pipe".to_string())
        })?;

        // Channels connecting callers to the I/O tasks.
        // Writer: caller -> writer task -> stdin
        let (write_tx, write_rx) = mpsc::channel::<AcpMessage>(64);
        // Reader: stdout -> reader task -> caller
        let (read_tx, read_rx) = mpsc::channel::<AcpMessage>(64);

        let shutdown = Notify::new();

        let inner = Arc::new(TransportInner {
            write_tx,
            read_rx: Mutex::new(read_rx),
            child: Mutex::new(Some(child)),
            shutdown,
            is_shutdown: Mutex::new(false),
        });

        // --- Writer task ---
        let writer_inner = Arc::clone(&inner);
        let writer_handle = tokio::spawn(writer_task(write_rx, child_stdin, writer_inner));

        // --- Reader task ---
        let reader_inner = Arc::clone(&inner);
        let reader_handle = tokio::spawn(reader_task(read_tx, child_stdout, reader_inner));

        // --- Stderr capture task ---
        let stderr_handle = tokio::spawn(stderr_task(child_stderr));

        info!("ACP agent subprocess started successfully");

        Ok(Self {
            inner,
            _writer_handle: writer_handle,
            _reader_handle: reader_handle,
            _stderr_handle: stderr_handle,
        })
    }
}

/// Background task: reads messages from the write channel and writes them to stdin.
async fn writer_task(
    mut write_rx: mpsc::Receiver<AcpMessage>,
    mut stdin: tokio::process::ChildStdin,
    inner: Arc<TransportInner>,
) {
    loop {
        tokio::select! {
            msg = write_rx.recv() => {
                match msg {
                    Some(message) => {
                        let result = write_message(&mut stdin, &message).await;
                        if let Err(e) = result {
                            error!(error = %e, "Writer task: failed to write to stdin");
                            break;
                        }
                    }
                    None => {
                        debug!("Writer task: channel closed, exiting");
                        break;
                    }
                }
            }
            _ = inner.shutdown.notified() => {
                debug!("Writer task: shutdown signal received");
                break;
            }
        }
    }
    // Drop stdin to signal EOF to the child process.
    drop(stdin);
    debug!("Writer task exited");
}

/// Serialize an `AcpMessage` as a single JSON line + newline, then flush.
async fn write_message(
    stdin: &mut tokio::process::ChildStdin,
    message: &AcpMessage,
) -> Result<(), TransportError> {
    let json = serde_json::to_string(message)
        .map_err(|e| TransportError::SendFailed(format!("JSON serialization failed: {e}")))?;
    stdin
        .write_all(json.as_bytes())
        .await
        .map_err(|e| TransportError::SendFailed(format!("stdin write failed: {e}")))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|e| TransportError::SendFailed(format!("stdin newline write failed: {e}")))?;
    stdin
        .flush()
        .await
        .map_err(|e| TransportError::SendFailed(format!("stdin flush failed: {e}")))?;
    Ok(())
}

/// Background task: reads NDJSON lines from stdout and sends parsed messages to the read channel.
async fn reader_task(
    read_tx: mpsc::Sender<AcpMessage>,
    stdout: tokio::process::ChildStdout,
    inner: Arc<TransportInner>,
) {
    let mut reader = BufReader::new(stdout);
    let mut line_buf = String::new();

    loop {
        line_buf.clear();

        tokio::select! {
            result = reader.read_line(&mut line_buf) => {
                match result {
                    Ok(0) => {
                        // EOF — child closed stdout.
                        info!("Reader task: stdout EOF, agent process likely exited");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line_buf.trim();
                        if trimmed.is_empty() {
                            // Blank lines are not protocol messages; skip.
                            continue;
                        }
                        match serde_json::from_str::<AcpMessage>(trimmed) {
                            Ok(msg) => {
                                if read_tx.send(msg).await.is_err() {
                                    debug!("Reader task: receive channel closed");
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    line = %trimmed,
                                    "Reader task: failed to parse NDJSON line, skipping"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Reader task: stdout read error");
                        break;
                    }
                }
            }
            _ = inner.shutdown.notified() => {
                debug!("Reader task: shutdown signal received");
                break;
            }
        }
    }
    debug!("Reader task exited");
}

/// Background task: reads stderr lines and logs them as warnings.
async fn stderr_task(stderr: tokio::process::ChildStderr) {
    let mut reader = BufReader::new(stderr);
    let mut line_buf = String::new();

    loop {
        line_buf.clear();
        match reader.read_line(&mut line_buf).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line_buf.trim();
                if !trimmed.is_empty() {
                    warn!(line = %trimmed, "ACP agent stderr");
                }
            }
            Err(e) => {
                error!(error = %e, "Stderr capture: read error");
                break;
            }
        }
    }
    debug!("Stderr capture task exited");
}

#[async_trait]
impl AcpTransport for StdioTransport {
    async fn send(&self, message: AcpMessage) -> Result<(), TransportError> {
        let is_shutdown = *self.inner.is_shutdown.lock().await;
        if is_shutdown {
            return Err(TransportError::NotConnected);
        }

        self.inner
            .write_tx
            .send(message)
            .await
            .map_err(|_| TransportError::SendFailed("writer channel closed".to_string()))
    }

    async fn receive(&self) -> Result<AcpMessage, TransportError> {
        let is_shutdown = *self.inner.is_shutdown.lock().await;
        if is_shutdown {
            return Err(TransportError::NotConnected);
        }

        let mut rx = self.inner.read_rx.lock().await;
        rx.recv()
            .await
            .ok_or(TransportError::ProcessExited("agent stdout closed".to_string()))
    }

    async fn is_alive(&self) -> bool {
        let is_shutdown = *self.inner.is_shutdown.lock().await;
        if is_shutdown {
            return false;
        }

        let mut child_guard = self.inner.child.lock().await;
        if let Some(ref mut child) = *child_guard {
            // try_wait returns Ok(None) if the process is still running.
            match child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    debug!(status = ?status, "Agent process has exited");
                    false
                }
                Err(e) => {
                    error!(error = %e, "Failed to check agent process status");
                    false
                }
            }
        } else {
            false
        }
    }

    async fn shutdown(&self) -> Result<(), TransportError> {
        {
            let mut is_shutdown = self.inner.is_shutdown.lock().await;
            if *is_shutdown {
                debug!("Transport already shut down, ignoring duplicate call");
                return Ok(());
            }
            *is_shutdown = true;
        }

        info!("Initiating graceful shutdown of ACP agent");

        // Signal background tasks to exit their loops.
        self.inner.shutdown.notify_waiters();

        // Give the writer task a moment to drop stdin (signals EOF to child).
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut child_guard = self.inner.child.lock().await;
        if let Some(ref mut child) = *child_guard {
            // Phase 1: Wait up to 5 seconds for the process to exit on its own
            // (it should notice stdin EOF and exit gracefully).
            let exited = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                child.wait(),
            )
            .await;

            if let Ok(Ok(status)) = exited {
                info!(status = ?status, "Agent process exited gracefully");
                return Ok(());
            }

            // Phase 2: SIGTERM (Unix) or kill (Windows) + wait 2 seconds.
            #[cfg(unix)]
            {
                if let Some(pid) = child.id() {
                    debug!(pid = pid, "Sending SIGTERM to agent process");
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGTERM);
                    }
                }
            }
            #[cfg(not(unix))]
            {
                // On non-Unix platforms, go straight to kill.
                let _ = child.start_kill();
            }

            let exited_after_term = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                child.wait(),
            )
            .await;

            if let Ok(Ok(status)) = exited_after_term {
                info!(status = ?status, "Agent process exited after SIGTERM");
                return Ok(());
            }

            // Phase 3: SIGKILL (forceful).
            warn!("Agent process did not exit after SIGTERM, sending SIGKILL");
            let kill_result = child.kill().await;
            match kill_result {
                Ok(()) => {
                    info!("Agent process killed (SIGKILL)");
                }
                Err(e) => {
                    // Process may have already exited between our check and kill.
                    warn!(error = %e, "Failed to SIGKILL agent process (may have already exited)");
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_types::AcpTransport;

    /// Helper: create a config that runs a simple echo script.
    ///
    /// The script reads lines from stdin and echoes them back to stdout,
    /// which is the simplest possible "agent" for testing NDJSON transport.
    fn echo_config() -> StdioTransportConfig {
        // Use a bash one-liner that reads stdin line-by-line and echoes each line.
        // `IFS=` prevents leading/trailing whitespace trimming.
        // `-r` prevents backslash interpretation.
        StdioTransportConfig {
            binary: "/bin/bash".to_string(),
            args: vec![
                "-c".to_string(),
                "while IFS= read -r line; do echo \"$line\"; done".to_string(),
            ],
            env: HashMap::new(),
            working_dir: None,
        }
    }

    /// Helper: create a test AcpMessage (JSON-RPC request).
    fn test_request(id: u64, method: &str) -> AcpMessage {
        AcpMessage {
            jsonrpc: "2.0".to_string(),
            method: Some(method.to_string()),
            params: Some(serde_json::json!({})),
            id: Some(serde_json::json!(id)),
            result: None,
            error: None,
        }
    }

    #[tokio::test]
    async fn test_spawn_and_is_alive() {
        let transport = StdioTransport::spawn(echo_config()).await.unwrap();
        assert!(transport.is_alive().await);
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_send_receive_echo() {
        let transport = StdioTransport::spawn(echo_config()).await.unwrap();

        let msg = test_request(1, "test/echo");
        transport.send(msg.clone()).await.unwrap();

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            transport.receive(),
        )
        .await
        .expect("receive timed out")
        .expect("receive failed");

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.method.as_deref(), Some("test/echo"));
        assert_eq!(response.id, Some(serde_json::json!(1)));

        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_messages() {
        let transport = StdioTransport::spawn(echo_config()).await.unwrap();

        for i in 1..=5 {
            let msg = test_request(i, &format!("test/msg_{i}"));
            transport.send(msg).await.unwrap();
        }

        for i in 1..=5 {
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                transport.receive(),
            )
            .await
            .expect("receive timed out")
            .expect("receive failed");

            assert_eq!(response.id, Some(serde_json::json!(i)));
        }

        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_graceful_shutdown() {
        let transport = StdioTransport::spawn(echo_config()).await.unwrap();
        assert!(transport.is_alive().await);

        transport.shutdown().await.unwrap();

        // After shutdown, is_alive should return false.
        assert!(!transport.is_alive().await);
    }

    #[tokio::test]
    async fn test_shutdown_idempotent() {
        let transport = StdioTransport::spawn(echo_config()).await.unwrap();
        transport.shutdown().await.unwrap();
        // Second shutdown should not error.
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_send_after_shutdown_returns_not_connected() {
        let transport = StdioTransport::spawn(echo_config()).await.unwrap();
        transport.shutdown().await.unwrap();

        let result = transport.send(test_request(1, "test")).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, TransportError::NotConnected),
            "Expected NotConnected, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_receive_after_shutdown_returns_not_connected() {
        let transport = StdioTransport::spawn(echo_config()).await.unwrap();
        transport.shutdown().await.unwrap();

        let result = transport.receive().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, TransportError::NotConnected),
            "Expected NotConnected, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_broken_pipe_detection() {
        // Spawn a process that exits immediately (broken pipe scenario).
        let config = StdioTransportConfig {
            binary: "/bin/bash".to_string(),
            args: vec!["-c".to_string(), "exit 0".to_string()],
            env: HashMap::new(),
            working_dir: None,
        };

        let transport = StdioTransport::spawn(config).await.unwrap();

        // Give the process a moment to exit.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // The process should no longer be alive.
        assert!(!transport.is_alive().await);

        // Receiving should return a ProcessExited error since stdout is closed.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            transport.receive(),
        )
        .await
        .expect("receive timed out");

        assert!(result.is_err());

        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_stderr_does_not_interfere_with_protocol() {
        // Agent that writes to both stdout and stderr.
        let config = StdioTransportConfig {
            binary: "/bin/bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"while IFS= read -r line; do echo "STDERR: debug info" >&2; echo "$line"; done"#
                    .to_string(),
            ],
            env: HashMap::new(),
            working_dir: None,
        };

        let transport = StdioTransport::spawn(config).await.unwrap();

        let msg = test_request(42, "test/stderr_check");
        transport.send(msg).await.unwrap();

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            transport.receive(),
        )
        .await
        .expect("receive timed out")
        .expect("receive failed");

        // The response should be the echoed message, not stderr output.
        assert_eq!(response.id, Some(serde_json::json!(42)));
        assert_eq!(response.method.as_deref(), Some("test/stderr_check"));

        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_spawn_nonexistent_binary_fails() {
        let config = StdioTransportConfig {
            binary: "/nonexistent/binary/path".to_string(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
        };

        let result = StdioTransport::spawn(config).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, TransportError::SendFailed(_)),
            "Expected SendFailed, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn test_env_vars_passed_to_subprocess() {
        // Spawn a process that echoes an environment variable.
        let mut env = HashMap::new();
        env.insert("ACP_TEST_VAR".to_string(), "hello_acp".to_string());

        let config = StdioTransportConfig {
            binary: "/bin/bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"echo "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"$ACP_TEST_VAR\"}""#
                    .to_string(),
            ],
            env,
            working_dir: None,
        };

        let transport = StdioTransport::spawn(config).await.unwrap();

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            transport.receive(),
        )
        .await
        .expect("receive timed out")
        .expect("receive failed");

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(
            response.result,
            Some(serde_json::json!("hello_acp"))
        );

        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_partial_line_buffering() {
        // Agent that sends a message in two writes (simulating partial line).
        // The reader should buffer until the newline arrives.
        let config = StdioTransportConfig {
            binary: "/bin/bash".to_string(),
            args: vec![
                "-c".to_string(),
                // Use printf to write partial data, then complete the line.
                r#"printf '{"jsonrpc":"2.0",' && sleep 0.1 && echo '"id":1,"method":"test/partial","params":{}}'""#
                    .to_string(),
            ],
            env: HashMap::new(),
            working_dir: None,
        };

        let transport = StdioTransport::spawn(config).await.unwrap();

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            transport.receive(),
        )
        .await
        .expect("receive timed out")
        .expect("receive failed");

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, Some(serde_json::json!(1)));
        assert_eq!(response.method.as_deref(), Some("test/partial"));

        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_invalid_json_line_skipped() {
        // Agent that sends one invalid line followed by a valid one.
        let config = StdioTransportConfig {
            binary: "/bin/bash".to_string(),
            args: vec![
                "-c".to_string(),
                r#"echo 'not valid json' && echo '{"jsonrpc":"2.0","id":1,"method":"test/valid","params":{}}'"#
                    .to_string(),
            ],
            env: HashMap::new(),
            working_dir: None,
        };

        let transport = StdioTransport::spawn(config).await.unwrap();

        // Should skip the invalid line and return the valid message.
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            transport.receive(),
        )
        .await
        .expect("receive timed out")
        .expect("receive failed");

        assert_eq!(response.id, Some(serde_json::json!(1)));
        assert_eq!(response.method.as_deref(), Some("test/valid"));

        transport.shutdown().await.unwrap();
    }
}
