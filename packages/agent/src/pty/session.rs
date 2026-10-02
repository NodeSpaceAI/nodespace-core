//! One running agent process attached to a pseudo-terminal.
//!
//! A [`PtySession`] owns:
//!
//! * a persistent per-session directory at `~/.nodespace/agent-sessions/<uuid>/`
//!   (the agent's working directory — survives session end),
//! * the master end of a PTY pair (`portable-pty`),
//! * a writer into the PTY's stdin,
//! * a [`portable_pty::ChildKiller`] handle for the spawned process,
//! * a background blocking task that reads the PTY's output and fans it out
//!   through `broadcast::Sender<OutputChunk>`,
//! * a watcher task (which owns the actual `Child`) that broadcasts the
//!   process exit status through `broadcast::Sender<ExitStatus>`.
//!
//! The session directory is **not** deleted on session end. It persists under
//! `~/.nodespace/agent-sessions/<session-uuid>/` so artifacts, context files,
//! and agent output survive across restarts.
//!
//! ## Concurrency
//!
//! The exit-watcher task owns the [`portable_pty::Child`] exclusively so it
//! can block in `wait()` without holding any lock the session might need.
//! [`PtySession::terminate`] kills the child through a separately-cloned
//! [`portable_pty::ChildKiller`], avoiding the obvious deadlock of a kill
//! call waiting on a mutex the wait-loop already holds.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch, Mutex};
use uuid::Uuid;

use crate::agent_catalog::context_assembly::GraphContextAssembler;
use crate::agent_catalog::registry::SystemAgentRegistry;
use crate::agent_types::{AgentType, ContextError};
use crate::pty::capture::SessionCapture;

/// Number of buffered chunks per output subscriber. Slow consumers that fall
/// behind by more than this many chunks will see `RecvError::Lagged`; that is
/// preferable to unbounded memory growth when an agent is streaming faster
/// than a UI can render.
const OUTPUT_CHANNEL_CAPACITY: usize = 256;

/// Read buffer for the PTY output thread. PTY data is byte-by-byte in the
/// worst case (one keystroke echo), but typical chunks are tens to a few
/// hundred bytes, so 4 KiB amortises syscalls without delaying small writes.
const READ_BUFFER_BYTES: usize = 4096;

/// Default PTY size used at launch. Callers reshape via [`PtySession::resize`]
/// as soon as they know their terminal geometry.
const DEFAULT_PTY_ROWS: u16 = 24;
const DEFAULT_PTY_COLS: u16 = 80;

/// Environment variables every PTY child gets regardless of which agent is
/// spawned — the minimum a POSIX CLI needs to resolve binaries, find the
/// user's home, and render correctly in a terminal. Everything else is
/// dropped via `env_clear()`; auth vars are layered in per agent via
/// [`AgentDefinition::auth_env_vars`].
const BASE_ENV_ALLOWLIST: &[&str] = &["HOME", "PATH", "TERM", "SHELL", "LANG", "USER", "TMPDIR"];

/// Clear the command's inherited environment and repopulate it from
/// `std::env::var` using `BASE_ENV_ALLOWLIST` plus `extra_vars` (typically an
/// agent's `auth_env_vars`). Only variables actually present in the daemon's
/// environment are forwarded — a missing var is silently skipped rather than
/// passed through empty.
fn apply_env_allowlist(cmd: &mut CommandBuilder, extra_vars: &[&str]) {
    cmd.env_clear();
    for key in BASE_ENV_ALLOWLIST.iter().chain(extra_vars) {
        if let Ok(value) = std::env::var(key) {
            cmd.env(key, value);
        }
    }
}

/// One chunk of raw bytes read from the PTY master.
///
/// The PTY merges stdout and stderr into a single byte stream — there is no
/// way to distinguish them at this layer. Consumers (UI, capture pipeline)
/// are expected to render the stream as a terminal would.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputChunk {
    /// Raw bytes from the PTY. May contain partial UTF-8 sequences and
    /// terminal escape codes.
    pub data: Vec<u8>,
    /// Wall-clock time the chunk was read off the master FD.
    pub timestamp: DateTime<Utc>,
}

/// Exit status observed when the agent process terminates.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ExitStatus {
    /// Raw exit code reported by `portable_pty::ExitStatus`. Zero on clean
    /// exit, non-zero on failure or signal termination.
    pub code: u32,
    /// `true` if the child exited cleanly (code == 0, no signal).
    pub success: bool,
}

/// One agent process running inside a PTY.
pub struct PtySession {
    /// Stable identifier for this session.
    pub id: Uuid,
    /// Which external agent (Claude Code, Codex, ...) is running.
    pub agent_type: AgentType,
    /// When [`PtySession::launch`] returned successfully.
    pub started_at: DateTime<Utc>,
    /// The `ai-chat-pty` node this session is a view onto, when it was
    /// launched for one. A viewer finds its node's running session by it.
    pub node_id: Option<String>,

    /// Master end of the PTY. Held under a mutex so [`resize`](Self::resize)
    /// can be called from any task without racing the output reader.
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Writer to the PTY's stdin. Acquired once at launch via
    /// `master.take_writer()` so [`write_input`](Self::write_input) does not
    /// have to fight the reader for the master.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Kill handle for the child process. The actual [`portable_pty::Child`]
    /// lives in the exit-watcher task; this handle is what we use to send
    /// a signal without contending on that task's ownership.
    child_killer: Arc<Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>>,

    /// Fan-out for output chunks.
    output_tx: broadcast::Sender<OutputChunk>,
    /// Latched exit status. Starts at `None` and transitions to `Some(...)`
    /// exactly once when the watcher observes the child exiting. `watch` (not
    /// `broadcast`) so subscribers created *after* the exit still see the
    /// final value — terminate() and the manager's auto-prune both rely on
    /// this.
    exit_tx: watch::Sender<Option<ExitStatus>>,

    /// Ring buffer that accumulates output for capture. Shared with the
    /// reader task so all chunks land here without extra subscriptions.
    capture: Arc<Mutex<SessionCapture>>,

    /// Persistent working directory for this session. Lives at
    /// `~/.nodespace/agent-sessions/<uuid>/` and is never deleted on session
    /// end — artifacts survive across restarts.
    pub session_dir: PathBuf,
}

impl PtySession {
    /// Spawn the agent binary for `agent_type` in a fresh PTY.
    ///
    /// Steps, in order:
    ///
    /// 1. Generate a session UUID and create a persistent directory at
    ///    `~/.nodespace/agent-sessions/<uuid>/`.
    /// 2. Have `assembler` write the context file (`CLAUDE.md` / `AGENTS.md`) and
    ///    `SKILL.md` into the session directory.
    /// 3. Resolve the agent binary on `PATH` via [`which::which`].
    /// 4. Open a PTY pair and spawn the binary with `cwd` set to the session dir.
    /// 5. Start the reader and exit-watcher tasks.
    pub async fn launch(
        agent_type: AgentType,
        initial_prompt: Option<String>,
        node_id: Option<String>,
        assembler: &GraphContextAssembler,
    ) -> anyhow::Result<Self> {
        let session_id = Uuid::new_v4();
        let sessions_base = dirs::home_dir()
            .context("HOME not set")?
            .join(".nodespace")
            .join("agent-sessions");
        let session_dir = sessions_base.join(session_id.to_string());
        std::fs::create_dir_all(&session_dir)
            .with_context(|| format!("create session dir {}", session_dir.display()))?;

        assembler
            .write_context_file(&session_dir, agent_type)
            .await
            .map_err(|e| match e {
                ContextError::Other(err) => err,
                other => anyhow::Error::new(other),
            })?;

        let definition = SystemAgentRegistry::new()
            .get(agent_type)
            .ok_or_else(|| anyhow::anyhow!("agent {:?} missing from catalog", agent_type))?;

        let binary_path = which::which(definition.binary).map_err(|e| {
            anyhow::anyhow!(
                "agent binary '{}' not found on PATH: {}",
                definition.binary,
                e
            )
        })?;

        let mut session = Self::spawn_in_pty(
            session_id,
            agent_type,
            binary_path,
            initial_prompt,
            session_dir,
            DEFAULT_PTY_ROWS,
            DEFAULT_PTY_COLS,
            definition.auth_env_vars,
        )?;
        session.node_id = node_id;
        Ok(session)
    }

    /// Inner helper that opens the PTY, spawns the process, and wires up
    /// reader / exit-watcher tasks. Split out from [`launch`](Self::launch)
    /// so tests can construct sessions without going through
    /// [`GraphContextAssembler`].
    #[allow(clippy::too_many_arguments)]
    fn spawn_in_pty(
        id: Uuid,
        agent_type: AgentType,
        binary_path: std::path::PathBuf,
        initial_prompt: Option<String>,
        session_dir: PathBuf,
        rows: u16,
        cols: u16,
        auth_env_vars: &[&str],
    ) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(binary_path);
        cmd.cwd(&session_dir);
        apply_env_allowlist(&mut cmd, auth_env_vars);
        if let Some(prompt) = initial_prompt {
            cmd.arg(prompt);
        }

        let child = pair.slave.spawn_command(cmd)?;
        // `portable-pty` recommends dropping the slave handle once the child
        // is spawned so closing the master tears down the PTY cleanly.
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let child_killer = child.clone_killer();

        let (output_tx, _) = broadcast::channel::<OutputChunk>(OUTPUT_CHANNEL_CAPACITY);
        let (exit_tx, _) = watch::channel::<Option<ExitStatus>>(None);

        let started_at = Utc::now();

        let master = Arc::new(Mutex::new(pair.master));
        let writer = Arc::new(Mutex::new(writer));
        let child_killer = Arc::new(Mutex::new(child_killer));
        let capture = Arc::new(Mutex::new(SessionCapture::new()));

        spawn_reader_task(reader, output_tx.clone(), capture.clone());
        spawn_exit_watcher_task(child, exit_tx.clone());

        Ok(Self {
            id,
            agent_type,
            started_at,
            node_id: None,
            master,
            writer,
            child_killer,
            output_tx,
            exit_tx,
            capture,
            session_dir,
        })
    }

    /// Subscribe to the PTY's output byte stream.
    ///
    /// Every subscriber sees the same stream from the point of subscription
    /// forward. Subscribers that fall too far behind will get
    /// `RecvError::Lagged`.
    pub fn subscribe_output(&self) -> broadcast::Receiver<OutputChunk> {
        self.output_tx.subscribe()
    }

    /// Subscribe to the child-exit signal.
    ///
    /// The returned receiver always sees the *latest* value of the exit
    /// status, including if the child has already exited before the call
    /// (the watch channel latches the final `Some(...)`).
    pub fn subscribe_exit(&self) -> watch::Receiver<Option<ExitStatus>> {
        self.exit_tx.subscribe()
    }

    /// Return the exit status if the child has already terminated, or `None`
    /// while it is still running.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        *self.exit_tx.borrow()
    }

    /// Return a cloned snapshot of the capture buffer, briefly locking it.
    pub async fn snapshot_capture(&self) -> SessionCapture {
        self.capture.lock().await.clone()
    }

    /// Write `data` to the PTY's stdin.
    pub async fn write_input(&self, data: &[u8]) -> anyhow::Result<()> {
        let writer = self.writer.clone();
        let buf = data.to_vec();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let mut guard = writer.blocking_lock();
            guard.write_all(&buf)?;
            guard.flush()?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("write_input task panicked: {}", e))?
    }

    /// Resize the PTY to `cols` x `rows`.
    pub async fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        let master = self.master.lock().await;
        master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Kill the child process and wait for the exit-watcher to observe it.
    ///
    /// Takes `&self` rather than consuming the session, because typical
    /// callers hold the session behind an `Arc` (the manager hands out
    /// `Arc<PtySession>` from `get()`). The session directory at
    /// `~/.nodespace/agent-sessions/<uuid>/` is NOT deleted on session end —
    /// it persists so artifacts survive across daemon restarts.
    ///
    /// Safe to call when the child has already exited: returns immediately
    /// without erroring.
    ///
    /// On Unix the kill is `SIGHUP` (what `portable_pty::ChildKiller::kill`
    /// emits); on Windows it is `TerminateProcess`. Most agent CLIs treat
    /// either as a clean shutdown signal. If a future agent needs `SIGTERM`
    /// specifically, signal it directly via `libc::kill` from `cfg(unix)`
    /// code rather than changing this default.
    pub async fn terminate(&self) -> anyhow::Result<()> {
        // Subscribe to the watch channel up front. `watch::Receiver` always
        // sees the latest value, so this works whether the child has already
        // exited or is still running.
        let mut exit_rx = self.exit_tx.subscribe();

        // Fast path: if the watcher has already latched a value, the child
        // is gone and there is nothing to kill.
        if exit_rx.borrow().is_some() {
            return Ok(());
        }

        // Send the kill signal. If the child has already exited, kill()
        // returns an error which we ignore — the watch loop below will
        // observe the exit either way.
        {
            let killer = self.child_killer.clone();
            tokio::task::spawn_blocking(move || {
                let mut guard = killer.blocking_lock();
                let _ = guard.kill();
            })
            .await
            .map_err(|e| anyhow::anyhow!("terminate kill task panicked: {}", e))?;
        }

        // Wait for the watcher to publish a `Some(_)`. `changed()` resolves
        // every time the value transitions; we loop until the latched value
        // is non-None to guard against spurious wakeups.
        loop {
            if exit_rx.borrow().is_some() {
                return Ok(());
            }
            if exit_rx.changed().await.is_err() {
                // Sender dropped — only happens if the session itself was
                // dropped concurrently, which would be a logic error in
                // the caller. Treat as success since the process is gone.
                return Ok(());
            }
        }
    }
}

/// Background task: read from the PTY master and fan bytes out to subscribers.
///
/// Runs on the blocking pool because `portable-pty`'s reader is synchronous.
/// The task ends naturally when the reader returns EOF (child closed the PTY)
/// or hits an error. Each chunk is also pushed into `capture` so the capture
/// service can assemble a transcript or summary after the session ends.
fn spawn_reader_task(
    mut reader: Box<dyn Read + Send>,
    output_tx: broadcast::Sender<OutputChunk>,
    capture: Arc<Mutex<SessionCapture>>,
) {
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; READ_BUFFER_BYTES];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: child closed the PTY.
                Ok(n) => {
                    let chunk = OutputChunk {
                        data: buf[..n].to_vec(),
                        timestamp: Utc::now(),
                    };
                    // Push into the capture buffer synchronously — this is a
                    // blocking task so `blocking_lock` is appropriate here.
                    capture.blocking_lock().push(chunk.clone());
                    // Send errors only happen when no receivers exist; that
                    // is fine — keep draining the PTY so the kernel buffer
                    // does not fill and block the child.
                    let _ = output_tx.send(chunk);
                }
                Err(e) => {
                    tracing::warn!("pty reader error: {}", e);
                    break;
                }
            }
        }
    });
}

#[cfg(any(test, feature = "testing"))]
impl PtySession {
    /// Test-only constructor: spawn an arbitrary binary in a PTY without
    /// going through [`GraphContextAssembler`]. Lets tests use shell utilities
    /// (`cat`, `sh -c '...'`) instead of depending on a real agent binary.
    ///
    /// Creates a temporary directory under `std::env::temp_dir()` for the
    /// session. The directory persists until the OS cleans up the temp dir.
    ///
    /// Gated by the `testing` feature so it is reachable from integration
    /// tests in sibling crates (e.g. `nodespace-daemon`) without being part
    /// of the production surface.
    pub fn launch_for_test(binary: &str, args: Vec<String>) -> anyhow::Result<Self> {
        let id = Uuid::new_v4();
        let session_dir = std::env::temp_dir()
            .join("nodespace-agent-test-sessions")
            .join(id.to_string());
        std::fs::create_dir_all(&session_dir)
            .with_context(|| format!("create test session dir {}", session_dir.display()))?;

        let binary_path = which::which(binary)
            .map_err(|e| anyhow::anyhow!("test binary '{}' not on PATH: {}", binary, e))?;

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: DEFAULT_PTY_ROWS,
            cols: DEFAULT_PTY_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = CommandBuilder::new(binary_path);
        cmd.cwd(&session_dir);
        apply_env_allowlist(&mut cmd, &[]);
        for a in &args {
            cmd.arg(a);
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let child_killer = child.clone_killer();

        let (output_tx, _) = broadcast::channel::<OutputChunk>(OUTPUT_CHANNEL_CAPACITY);
        let (exit_tx, _) = watch::channel::<Option<ExitStatus>>(None);

        let started_at = Utc::now();

        let master = Arc::new(Mutex::new(pair.master));
        let writer = Arc::new(Mutex::new(writer));
        let child_killer = Arc::new(Mutex::new(child_killer));
        let capture = Arc::new(Mutex::new(SessionCapture::new()));

        spawn_reader_task(reader, output_tx.clone(), capture.clone());
        spawn_exit_watcher_task(child, exit_tx.clone());

        Ok(Self {
            id,
            agent_type: AgentType::ClaudeCode,
            started_at,
            node_id: None,
            master,
            writer,
            child_killer,
            output_tx,
            exit_tx,
            capture,
            session_dir,
        })
    }

    /// Test-only accessor: the session's working directory path.
    pub fn session_dir_path(&self) -> &std::path::Path {
        self.session_dir.as_path()
    }
}

/// Background task: own the child, wait for it to exit, latch the status.
fn spawn_exit_watcher_task(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    exit_tx: watch::Sender<Option<ExitStatus>>,
) {
    tokio::task::spawn_blocking(move || {
        let status = match child.wait() {
            Ok(s) => ExitStatus {
                code: s.exit_code(),
                success: s.success(),
            },
            Err(e) => {
                tracing::warn!("pty child wait error: {}", e);
                ExitStatus {
                    code: u32::MAX,
                    success: false,
                }
            }
        };
        let _ = exit_tx.send(Some(status));
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    /// RAII guard that sets a process env var for the duration of a test and
    /// removes it on drop, even if the test panics — avoids leaking test-only
    /// vars into later tests in the same process.
    struct EnvVarGuard {
        key: &'static str,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            // SAFETY: test-only; each call site uses a unique key not read
            // by other tests running in this process.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: see `set` above.
            unsafe {
                std::env::remove_var(self.key);
            }
        }
    }

    /// Drain at most `max_chunks` chunks within `wait` from a subscriber and
    /// return the concatenated bytes. Used by tests that need to inspect the
    /// PTY's output stream without depending on exact chunk boundaries.
    async fn collect_output(
        rx: &mut broadcast::Receiver<OutputChunk>,
        wait: Duration,
        max_chunks: usize,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..max_chunks {
            match timeout(wait, rx.recv()).await {
                Ok(Ok(chunk)) => out.extend_from_slice(&chunk.data),
                _ => break,
            }
        }
        out
    }

    /// Wait for the watch receiver to latch a `Some(_)` exit status, with a
    /// deadline. Returns the latched status or panics on timeout.
    async fn await_exit(
        rx: &mut watch::Receiver<Option<ExitStatus>>,
        deadline: Duration,
    ) -> ExitStatus {
        timeout(deadline, async {
            loop {
                if let Some(s) = *rx.borrow() {
                    return s;
                }
                if rx.changed().await.is_err() {
                    panic!("exit sender dropped before publishing status");
                }
            }
        })
        .await
        .expect("exit status latched within deadline")
    }

    #[tokio::test]
    async fn launch_for_test_runs_command_and_emits_output() {
        let session =
            PtySession::launch_for_test("sh", vec!["-c".into(), "echo hello-from-pty".into()])
                .expect("launch test session");

        assert_eq!(session.agent_type, AgentType::ClaudeCode);
        assert!(session.session_dir_path().exists());

        let mut rx = session.subscribe_output();
        let mut exit_rx = session.subscribe_exit();
        let output = collect_output(&mut rx, Duration::from_secs(2), 32).await;
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("hello-from-pty"),
            "expected echoed text in PTY output, got: {:?}",
            text
        );

        let status = await_exit(&mut exit_rx, Duration::from_secs(2)).await;
        assert!(status.success, "echo should exit successfully");
    }

    #[tokio::test]
    async fn write_input_is_echoed_back_through_pty() {
        // `cat` echoes stdin back to stdout, which the PTY then loops back
        // to its output stream.
        let session = PtySession::launch_for_test("cat", vec![]).expect("launch cat session");

        let mut rx = session.subscribe_output();

        session
            .write_input(b"ping\n")
            .await
            .expect("write input succeeds");

        let output = collect_output(&mut rx, Duration::from_secs(2), 32).await;
        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("ping"),
            "expected echoed input in PTY output, got: {:?}",
            text
        );

        session.terminate().await.expect("terminate cat session");
    }

    #[tokio::test]
    async fn resize_does_not_error() {
        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "sleep 1".into()])
            .expect("launch sleep session");

        session.resize(120, 40).await.expect("resize succeeds");
        session.resize(80, 24).await.expect("resize back succeeds");

        let mut exit_rx = session.subscribe_exit();
        await_exit(&mut exit_rx, Duration::from_secs(3)).await;
    }

    #[tokio::test]
    async fn terminate_kills_long_running_process() {
        // `sleep 30` would normally outlive the test; terminate must end it.
        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "sleep 30".into()])
            .expect("launch sleep session");

        let mut exit_rx = session.subscribe_exit();

        timeout(Duration::from_secs(3), session.terminate())
            .await
            .expect("terminate returns within deadline")
            .expect("terminate succeeds");

        let status = await_exit(&mut exit_rx, Duration::from_secs(1)).await;
        assert!(!status.success, "killed process should not report success");
    }

    #[tokio::test]
    async fn session_dir_persists_after_process_exit_and_session_drop() {
        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "echo done".into()])
            .expect("launch echo session");

        let dir_path = session.session_dir_path().to_path_buf();
        assert!(
            dir_path.exists(),
            "session dir should exist immediately after launch"
        );

        let mut exit_rx = session.subscribe_exit();
        await_exit(&mut exit_rx, Duration::from_secs(2)).await;

        // Session dir outlives the process exit.
        assert!(
            dir_path.exists(),
            "session dir should persist after process exits"
        );

        drop(session);

        // Session dir also outlives the session drop — persistent by design.
        assert!(
            dir_path.exists(),
            "session dir should persist after session is dropped"
        );
    }

    // ---- Regression: late subscribers and double-terminate -------------------

    /// `terminate()` must not hang if the child has already exited naturally
    /// before terminate is called. Regression test for review Finding #1.
    #[tokio::test]
    async fn terminate_after_natural_exit_returns_immediately() {
        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "echo done".into()])
            .expect("launch echo session");

        // Wait until the watcher has latched the exit.
        let mut exit_rx = session.subscribe_exit();
        await_exit(&mut exit_rx, Duration::from_secs(2)).await;
        assert!(session.exit_status().is_some());

        // Now terminate — must not block on a missing broadcast.
        timeout(Duration::from_secs(2), session.terminate())
            .await
            .expect("terminate returns immediately after natural exit")
            .expect("terminate succeeds");
    }

    /// `subscribe_exit()` after the child has exited must immediately observe
    /// the latched status — the `watch` channel does not drop values like a
    /// single-shot `broadcast` does. Regression test for review Finding #2.
    #[tokio::test]
    async fn subscribe_exit_after_exit_observes_status() {
        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "echo done".into()])
            .expect("launch echo session");

        // Wait via a first subscriber.
        let mut exit_rx = session.subscribe_exit();
        await_exit(&mut exit_rx, Duration::from_secs(2)).await;

        // Now create a fresh subscriber after exit. It must see Some(_).
        let late_rx = session.subscribe_exit();
        let latched = *late_rx.borrow();
        assert!(
            latched.is_some(),
            "watch receiver subscribed after exit should still see the status"
        );
        assert!(latched.unwrap().success);
    }

    /// Calling `terminate()` twice must be safe.
    #[tokio::test]
    async fn terminate_is_idempotent() {
        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "sleep 30".into()])
            .expect("launch sleep session");

        timeout(Duration::from_secs(3), session.terminate())
            .await
            .expect("first terminate returns")
            .expect("first terminate succeeds");

        timeout(Duration::from_secs(1), session.terminate())
            .await
            .expect("second terminate returns immediately")
            .expect("second terminate succeeds");
    }

    /// `write_input` and `resize` use independent locks (writer vs master);
    /// hammering both concurrently should not deadlock.
    #[tokio::test]
    async fn concurrent_write_and_resize_do_not_deadlock() {
        let session =
            Arc::new(PtySession::launch_for_test("cat", vec![]).expect("launch cat session"));

        let writer_session = session.clone();
        let writer = tokio::spawn(async move {
            for i in 0..20 {
                writer_session
                    .write_input(format!("line {}\n", i).as_bytes())
                    .await
                    .expect("write succeeds");
            }
        });

        let resizer_session = session.clone();
        let resizer = tokio::spawn(async move {
            for _ in 0..20 {
                let _ = resizer_session.resize(80, 24).await;
                let _ = resizer_session.resize(120, 40).await;
            }
        });

        timeout(Duration::from_secs(5), async {
            writer.await.unwrap();
            resizer.await.unwrap();
        })
        .await
        .expect("writer + resizer complete without deadlock");

        session.terminate().await.expect("terminate cat");
    }

    // ---- Regression: env allowlist ------------------------

    /// A secret-shaped var present in the daemon's own environment must NOT
    /// reach the PTY child — only `BASE_ENV_ALLOWLIST` entries and explicitly
    /// passed `extra_vars` survive `env_clear()`.
    #[tokio::test]
    async fn unlisted_env_var_does_not_reach_pty_child() {
        let _guard = EnvVarGuard::set("NODESPACE_TEST_SECRET_1521", "should-not-leak");

        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "env".into()])
            .expect("launch env session");

        let mut rx = session.subscribe_output();
        let mut exit_rx = session.subscribe_exit();
        let output = collect_output(&mut rx, Duration::from_secs(2), 64).await;
        await_exit(&mut exit_rx, Duration::from_secs(2)).await;

        let text = String::from_utf8_lossy(&output);
        assert!(
            !text.contains("NODESPACE_TEST_SECRET_1521"),
            "unlisted env var leaked into PTY child: {:?}",
            text
        );
    }

    /// Base allowlist entries (`PATH` in particular) must still reach the
    /// child — otherwise `which`-resolved binaries and shell builtins break.
    #[tokio::test]
    async fn base_allowlist_vars_still_reach_pty_child() {
        let session = PtySession::launch_for_test("sh", vec!["-c".into(), "env".into()])
            .expect("launch env session");

        let mut rx = session.subscribe_output();
        let mut exit_rx = session.subscribe_exit();
        let output = collect_output(&mut rx, Duration::from_secs(2), 64).await;
        await_exit(&mut exit_rx, Duration::from_secs(2)).await;

        let text = String::from_utf8_lossy(&output);
        assert!(
            text.contains("PATH="),
            "PATH should be forwarded to the PTY child, got: {:?}",
            text
        );
    }

    /// [`apply_env_allowlist`] forwards `extra_vars` (the per-agent auth env
    /// vars) in addition to the base allowlist, but only when they are
    /// actually set in the current process's environment.
    #[test]
    fn apply_env_allowlist_forwards_extra_vars_when_present() {
        let _guard = EnvVarGuard::set("NODESPACE_TEST_AUTH_VAR_1521", "token-value");

        let mut cmd = CommandBuilder::new("sh");
        apply_env_allowlist(
            &mut cmd,
            &["NODESPACE_TEST_AUTH_VAR_1521", "NODESPACE_TEST_UNSET"],
        );

        let auth_var = cmd
            .get_env("NODESPACE_TEST_AUTH_VAR_1521")
            .map(|v| v.to_owned());
        let unset_var = cmd.get_env("NODESPACE_TEST_UNSET");

        assert_eq!(
            auth_var.as_deref(),
            Some(std::ffi::OsStr::new("token-value")),
            "present extra var should be forwarded"
        );
        assert!(
            unset_var.is_none(),
            "extra var absent from process env should not appear"
        );
    }
}
