//! One running agent process attached to a pseudo-terminal.
//!
//! A [`PtySession`] owns:
//!
//! * the master end of a PTY pair (`portable-pty`),
//! * a writer into the PTY's stdin,
//! * a [`portable_pty::ChildKiller`] handle for the spawned process,
//! * a background blocking task that reads the PTY's output and fans it out
//!   through `broadcast::Sender<OutputChunk>`,
//! * a watcher task (which owns the actual `Child`) that broadcasts the
//!   process exit status through `broadcast::Sender<ExitStatus>`.
//!
//! The agent runs in the working directory the launch names: a project's
//! folder on this machine (ADR-093 §8). A launch that names none runs in a
//! private folder at `~/.nodespace/agent-sessions/<session-uuid>/`, which is
//! not deleted when the session ends. Nothing is written into either at
//! launch: the session gets NodeSpace's context through its harness plugin
//! and the `nodespace` CLI, which the environment set here points at the
//! right database.
//!
//! ## Concurrency
//!
//! The exit-watcher task owns the [`portable_pty::Child`] exclusively so it
//! can block in `wait()` without holding any lock the session might need.
//! [`PtySession::terminate`] kills the child through a separately-cloned
//! [`portable_pty::ChildKiller`], avoiding the obvious deadlock of a kill
//! call waiting on a mutex the wait-loop already holds.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, watch, Mutex};
use uuid::Uuid;

use crate::agent_catalog::registry::SystemAgentRegistry;
use crate::agent_types::AgentType;
use crate::pty::capture::SessionCapture;
use crate::pty::detection::resolve_binary;

/// Names this session to the `nodespace` commands run inside it: the PTY
/// session's id. A harness plugin reports the harness's own session id
/// against it.
pub const SESSION_ENV_VAR: &str = "NODESPACE_SESSION";

/// Names what the session was launched for: the task when the launch named
/// one, otherwise the chat node the session is a view onto. A harness plugin
/// opens with that work's context.
pub const LAUNCHED_FOR_ENV_VAR: &str = "NODESPACE_LAUNCHED_FOR";

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
/// dropped via `env_clear()`; an agent's own variables are layered in via
/// [`crate::agent_catalog::registry::AgentDefinition::env_vars`].
const BASE_ENV_ALLOWLIST: &[&str] = &["HOME", "PATH", "TERM", "SHELL", "LANG", "USER", "TMPDIR"];

/// Clear the command's inherited environment and repopulate it from
/// `std::env::var` using `BASE_ENV_ALLOWLIST` plus `extra_vars` (typically an
/// agent's `env_vars`). Only variables actually present in the daemon's
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

/// What a launch asks for.
#[derive(Debug, Clone)]
pub struct SessionLaunch {
    /// Which agent to start.
    pub agent_type: AgentType,
    /// Passed to the agent as its first argument, when given.
    pub initial_prompt: Option<String>,
    /// The `ai-chat-pty` node the session is a view onto, when there is one.
    pub node_id: Option<String>,
    /// The folder the agent runs in: the project's, on this machine. `None`
    /// runs it in a private session folder.
    pub working_dir: Option<PathBuf>,
    /// What NodeSpace sets in the session's environment on top of the
    /// allowlist: the database, the daemon socket and what the session was
    /// launched for. [`SESSION_ENV_VAR`] is added by the launch itself.
    pub env: Vec<(String, String)>,
}

impl SessionLaunch {
    /// A launch of `agent_type` with nothing else named.
    pub fn new(agent_type: AgentType) -> Self {
        Self {
            agent_type,
            initial_prompt: None,
            node_id: None,
            working_dir: None,
            env: Vec::new(),
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

    /// The harness's own id for the conversation, once its plugin has
    /// reported one. The latest report stands: a harness that starts a new
    /// conversation mid-session reports again.
    harness_session_id: std::sync::Mutex<Option<String>>,

    /// The folder the agent runs in: the project's, or the session's private
    /// folder at `~/.nodespace/agent-sessions/<uuid>/`.
    pub working_dir: PathBuf,
}

/// What [`PtySession::spawn_in_pty`] starts: a resolved binary, where it runs
/// and the environment it gets.
struct Spawn<'a> {
    id: Uuid,
    agent_type: AgentType,
    binary_path: PathBuf,
    args: Vec<String>,
    working_dir: PathBuf,
    /// Variables forwarded from the daemon's environment when set there, on
    /// top of the base allowlist.
    forwarded_vars: &'a [&'a str],
    /// The child's `PATH`, when it is not the daemon's own.
    search_path: Option<OsString>,
    /// Variables set to the given values.
    env: Vec<(String, String)>,
}

impl PtySession {
    /// Spawn the agent binary a launch names in a fresh PTY.
    ///
    /// Steps, in order:
    ///
    /// 1. Generate a session UUID.
    /// 2. Take the launch's working directory, or create the session's
    ///    private folder at `~/.nodespace/agent-sessions/<uuid>/`.
    /// 3. Resolve the agent binary on `search_path`, the one detection used.
    /// 4. Open a PTY pair and spawn the binary there, with the allowlisted
    ///    environment plus what the launch sets, and `search_path` as its
    ///    `PATH`.
    /// 5. Start the reader and exit-watcher tasks.
    pub fn launch(launch: SessionLaunch, search_path: OsString) -> anyhow::Result<Self> {
        let SessionLaunch {
            agent_type,
            initial_prompt,
            node_id,
            working_dir,
            mut env,
        } = launch;
        let session_id = Uuid::new_v4();

        let definition = SystemAgentRegistry::new()
            .get(agent_type)
            .ok_or_else(|| anyhow::anyhow!("agent {:?} missing from catalog", agent_type))?;

        let binary_path = resolve_binary(definition.binary, &search_path).ok_or_else(|| {
            anyhow::anyhow!(
                "agent binary '{}' not found on the search path",
                definition.binary
            )
        })?;

        let working_dir = match working_dir {
            Some(dir) => dir,
            None => {
                let dir = dirs::home_dir()
                    .context("HOME not set")?
                    .join(".nodespace")
                    .join("agent-sessions")
                    .join(session_id.to_string());
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("create session dir {}", dir.display()))?;
                dir
            }
        };

        env.push((SESSION_ENV_VAR.to_string(), session_id.to_string()));

        let mut session = Self::spawn_in_pty(Spawn {
            id: session_id,
            agent_type,
            binary_path,
            args: initial_prompt.into_iter().collect(),
            working_dir,
            forwarded_vars: definition.env_vars,
            search_path: Some(search_path),
            env,
        })?;
        session.node_id = node_id;
        Ok(session)
    }

    /// Opens the PTY, spawns the process, and wires up the reader and
    /// exit-watcher tasks.
    fn spawn_in_pty(spawn: Spawn<'_>) -> anyhow::Result<Self> {
        let Spawn {
            id,
            agent_type,
            binary_path,
            args,
            working_dir,
            forwarded_vars,
            search_path,
            env,
        } = spawn;
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: DEFAULT_PTY_ROWS,
            cols: DEFAULT_PTY_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(binary_path);
        cmd.cwd(&working_dir);
        apply_env_allowlist(&mut cmd, forwarded_vars);
        if let Some(path) = search_path {
            cmd.env("PATH", path);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }
        for arg in args {
            cmd.arg(arg);
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
            harness_session_id: std::sync::Mutex::new(None),
            working_dir,
        })
    }

    /// Record the id the harness gave its own conversation, as its plugin
    /// reported it. A later report replaces an earlier one.
    pub fn report_harness_session_id(&self, id: String) {
        *self
            .harness_session_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(id);
    }

    /// The harness's own id for the conversation, when one was reported.
    pub fn harness_session_id(&self) -> Option<String> {
        self.harness_session_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
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
    /// `Arc<PtySession>` from `get()`). The working directory is left as it
    /// is.
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
    /// Test-only constructor: spawn an arbitrary binary in a PTY, so tests
    /// can use shell utilities (`cat`, `sh -c '...'`) instead of depending on
    /// a real agent binary.
    ///
    /// Creates a temporary directory under `std::env::temp_dir()` for the
    /// session. The directory persists until the OS cleans up the temp dir.
    ///
    /// Gated by the `testing` feature so it is reachable from integration
    /// tests in sibling crates (e.g. `nodespace-daemon`) without being part
    /// of the production surface.
    pub fn launch_for_test(binary: &str, args: Vec<String>) -> anyhow::Result<Self> {
        Self::launch_for_test_as(binary, args, SessionLaunch::new(AgentType::ClaudeCode))
    }

    /// Test-only constructor: spawn an arbitrary binary the way
    /// [`launch`](Self::launch) spawns an agent's, with `launch`'s working
    /// directory, environment and node. The agent's own variables are
    /// forwarded as they are for a real launch.
    pub fn launch_for_test_as(
        binary: &str,
        args: Vec<String>,
        launch: SessionLaunch,
    ) -> anyhow::Result<Self> {
        let id = Uuid::new_v4();
        let working_dir = match launch.working_dir {
            Some(dir) => dir,
            None => {
                let dir = std::env::temp_dir()
                    .join("nodespace-agent-test-sessions")
                    .join(id.to_string());
                std::fs::create_dir_all(&dir)
                    .with_context(|| format!("create test session dir {}", dir.display()))?;
                dir
            }
        };

        let binary_path = which::which(binary)
            .map_err(|e| anyhow::anyhow!("test binary '{}' not on PATH: {}", binary, e))?;
        let forwarded_vars = SystemAgentRegistry::new()
            .get(launch.agent_type)
            .map(|definition| definition.env_vars)
            .unwrap_or_default();

        let mut env = launch.env;
        env.push((SESSION_ENV_VAR.to_string(), id.to_string()));

        let mut session = Self::spawn_in_pty(Spawn {
            id,
            agent_type: launch.agent_type,
            binary_path,
            args,
            working_dir,
            forwarded_vars,
            search_path: None,
            env,
        })?;
        session.node_id = launch.node_id;
        Ok(session)
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
        assert!(session.working_dir.exists());

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

        let dir_path = session.working_dir.clone();
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

    // ---- What a launch sets ------------------------------------------------

    /// Run `script` under `sh` the way a launch spawns an agent, and return
    /// what it printed.
    async fn run_as_launched(launch: SessionLaunch, script: &str) -> (PtySession, String) {
        let session =
            PtySession::launch_for_test_as("sh", vec!["-c".into(), script.into()], launch)
                .expect("launch session");
        let mut rx = session.subscribe_output();
        let mut exit_rx = session.subscribe_exit();
        let output = collect_output(&mut rx, Duration::from_secs(2), 64).await;
        await_exit(&mut exit_rx, Duration::from_secs(2)).await;
        (session, String::from_utf8_lossy(&output).into_owned())
    }

    #[tokio::test]
    async fn a_session_runs_in_the_folder_the_launch_names() {
        let project = tempfile::TempDir::new().unwrap();
        let folder = project.path().canonicalize().unwrap();
        let launch = SessionLaunch {
            working_dir: Some(folder.clone()),
            ..SessionLaunch::new(AgentType::ClaudeCode)
        };

        let (session, text) = run_as_launched(launch, "pwd -P").await;

        assert_eq!(session.working_dir, folder);
        assert!(
            text.contains(&*folder.to_string_lossy()),
            "the agent should run in the named folder, got: {text:?}"
        );
        // Nothing is written there at launch: no context file, no skill copy.
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn the_environment_carries_what_the_launch_sets_and_the_session_id() {
        let launch = SessionLaunch {
            env: vec![
                ("NODESPACE_DATABASE".to_string(), "db-two".to_string()),
                (LAUNCHED_FOR_ENV_VAR.to_string(), "task-1".to_string()),
            ],
            ..SessionLaunch::new(AgentType::ClaudeCode)
        };

        let (session, text) = run_as_launched(launch, "env").await;

        assert!(text.contains("NODESPACE_DATABASE=db-two"), "{text:?}");
        assert!(
            text.contains(&format!("{LAUNCHED_FOR_ENV_VAR}=task-1")),
            "{text:?}"
        );
        assert!(
            text.contains(&format!("{SESSION_ENV_VAR}={}", session.id)),
            "{text:?}"
        );
    }

    /// An agent's own variables reach it when the daemon has them, and no
    /// other agent's do: the allowlist stays an allowlist.
    #[tokio::test]
    async fn an_agents_own_variables_are_forwarded_and_no_others() {
        let _config = EnvVarGuard::set("CLAUDE_CONFIG_DIR", "/profiles/work");
        let _other = EnvVarGuard::set("NODESPACE_TEST_NOT_LISTED_3547", "should-not-leak");

        let (_, claude) = run_as_launched(SessionLaunch::new(AgentType::ClaudeCode), "env").await;
        assert!(
            claude.contains("CLAUDE_CONFIG_DIR=/profiles/work"),
            "{claude:?}"
        );
        assert!(
            !claude.contains("NODESPACE_TEST_NOT_LISTED_3547"),
            "{claude:?}"
        );

        let (_, codex) = run_as_launched(SessionLaunch::new(AgentType::Codex), "env").await;
        assert!(!codex.contains("CLAUDE_CONFIG_DIR"), "{codex:?}");
    }

    #[tokio::test]
    async fn the_latest_reported_harness_session_id_stands() {
        let session = PtySession::launch_for_test("true", vec![]).expect("launch session");
        assert_eq!(session.harness_session_id(), None);

        session.report_harness_session_id("first".to_string());
        session.report_harness_session_id("second".to_string());

        assert_eq!(session.harness_session_id().as_deref(), Some("second"));
    }
}
