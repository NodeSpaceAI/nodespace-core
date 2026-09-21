//! Reusable fixtures for ADR-048 Tauri-seam integration tests: a real,
//! headless `nodespaced` with a controlled lifecycle (spawn now, bind the
//! socket whenever the caller lets it), plus a `GrpcClient` connected to it
//! so tests can call the actual `#[tauri::command]` handler functions
//! directly — no webview.
//!
//! `tauri::State<'_, T>` has no public constructor outside Tauri's own IPC
//! machinery (its inner field is private, and there is no `From<&T>` impl —
//! confirmed against the `tauri` 2.11 source). The sanctioned way to get one
//! is `Manager::state::<T>()` after `Manager::manage(state)`, both of which
//! `tauri::test::mock_app()`'s `App<MockRuntime>` implements — and neither
//! needs a `WebviewWindow`. So every test uses `mock_app()` + `.manage(client)`
//! + `.state()` to obtain a real `State<GrpcClient>`, never a hand-built one.
//!
//! This also means the same `app` doubles as the real event bus the
//! optimistic-echo-race test needs (`Emitter`/`Listener`), so there is only
//! one fixture shape, not two.
//!
//! This is its own crate (rather than a `mod support;` duplicated into each
//! `tests/*.rs` integration test binary) so clippy's dead-code lint sees one
//! compilation of these helpers used across every test file, instead of
//! flagging whichever subset any single test binary doesn't happen to call
//! as unused — `-D warnings` in `rust:lint` would otherwise hard-fail on
//! that per-binary view.
//!
//! Running this crate's tests directly with a bare `cargo test -p
//! nodespace-app` uses cargo's default per-binary test concurrency, NOT the
//! `--test-threads=1` the pre-push gate (`scripts/test-gate.ts`) applies —
//! see that file's comment for why serializing matters here. Prefer `cargo
//! test -p nodespace-app -- --test-threads=1` when iterating locally on more
//! than one test in the same file to avoid the same daemon-spawn contention
//! the gate serializes away.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use nodespace_app_lib::services::GrpcClient;
use tauri::Manager;

// Re-exported the same way `daemon_binary_freshness` below is: `pub` purely
// so `tests/sidecar_staging_sync_test.rs` can exercise the real algorithm,
// not because anything in this crate calls it itself. Pulled in via
// `#[path]` rather than a `[build-dependencies]` crate because this crate
// already depends on `nodespace-app` — a build-dependency back onto the same
// package would be a cycle. See `build_support.rs`'s module doc for the full
// story.
#[path = "../../build_support.rs"]
mod build_support;
pub use build_support::sync_stale_sidecar;

/// Default timeout for `TauriTestApp::connect`'s daemon-health wait, shared
/// by every `tests/*.rs` call site instead of each hardcoding its own
/// duplicate literal.
///
/// 60s, not the 30s this used to be: spawning a whole OS process (daemon
/// binary load + socket bind) is CPU-bound work, and within a single
/// `tests/*.rs` binary `#[tokio::test]`s run concurrently by default, so
/// several real daemon spawns can contend for the same cores at once —
/// worse on a machine that also has an unrelated `cargo test` running
/// elsewhere. 30s was tuned for an unloaded machine and had no
/// headroom for that; 60s gives slack without materially raising the
/// wall-clock cost of a clean, unloaded run, since `wait_for_daemon` returns
/// as soon as the socket is healthy rather than sleeping out the full
/// duration. This is a ceiling on the failure case, not an added cost on
/// the success path — a healthy daemon returns in well under a second, so
/// raising this value does not add 60s to every test's typical run time,
/// only to how long a genuinely stuck run takes to fail.
pub const DAEMON_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// A running headless `nodespaced` pointed at a temp-dir socket and database.
/// Killed on drop so a panicking test never leaks the process.
pub struct SpawnedDaemon {
    child: Child,
    pub socket_path: PathBuf,
    _tmp_dir: tempfile::TempDir,
    captured_log: Arc<std::sync::Mutex<Vec<String>>>,
}

impl SpawnedDaemon {
    /// Spawn a real headless `nodespaced` with its socket at a fresh temp path.
    /// Does NOT wait for the socket to be bound — the daemon binds it only
    /// after finishing startup (embedding model load included), so the
    /// window between spawn and bind is exactly the "not ready" state
    /// readiness checks need to observe.
    pub fn spawn() -> Self {
        let tmp_dir = tempfile::tempdir().expect("create temp dir for daemon fixture");
        let socket_path = tmp_dir.path().join("daemon.sock");
        Self::spawn_with_socket(tmp_dir, socket_path)
    }

    /// Like [`Self::spawn`], but with the socket at a caller-chosen path
    /// instead of the standard `<tmp_dir>/daemon.sock` — for tests that need
    /// to control exactly what `NODESPACED_SOCKET` resolves to and where its
    /// directory does or doesn't already exist (e.g. a path the daemon must
    /// create a new owner-only directory for, or one that lands directly in
    /// an already-existing, non-owned directory such as the system temp root).
    /// `NODESPACE_HOME`/the database path are still isolated under `tmp_dir`
    /// regardless of where `socket_path` itself points.
    pub fn spawn_with_socket(tmp_dir: tempfile::TempDir, socket_path: PathBuf) -> Self {
        let db_path = tmp_dir.path().join("db");

        let binary = resolve_daemon_binary();
        tracing::info!(binary = %binary.display(), socket = %socket_path.display(), "spawning nodespaced for readiness test");

        let mut child = Command::new(&binary)
            .env("NODESPACED_SOCKET", &socket_path)
            .env("NODESPACED_DB_PATH", &db_path)
            // Isolate the whole `.nodespace` home under the temp dir so the
            // real user's database registry is never touched by a spawned
            // daemon (ADR-053): without this, the daemon inherits the real HOME
            // and seeds `~/.nodespace/databases.toml` with this temp db path.
            .env("NODESPACE_HOME", tmp_dir.path())
            .env("NODESPACED_HEADLESS", "1")
            // Default to `warn`, but let the caller raise it: diagnosing a
            // daemon-side stall from the test side is impossible when the
            // daemon's own `info!` breadcrumbs are filtered out before they
            // reach the drain threads below. Pair with `E2E_VERBOSE=1`, which
            // is what actually echoes the drained lines.
            .env(
                "RUST_LOG",
                std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".to_string()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn {}: {e}", binary.display()));

        // MUST drain both pipes continuously: `Stdio::piped()` gives each a
        // small OS pipe buffer (~64KB), and llama.cpp's own stderr logging
        // (Metal kernel compilation alone is dozens of lines per model load,
        // outside the `tracing`/`RUST_LOG` filter entirely) fills that buffer
        // during any test that loads a real model. Once full, the daemon
        // blocks on its next write() call and the whole test hangs forever —
        // this is not hypothetical, it reproduced directly while writing the
        // ai-chat integration test. E2E_VERBOSE prints instead of discarding,
        // matching the TypeScript harness's same-named escape hatch.
        let verbose = std::env::var_os("E2E_VERBOSE").is_some();
        let captured_log = Arc::new(std::sync::Mutex::new(Vec::new()));
        if let Some(stdout) = child.stdout.take() {
            spawn_drain_thread(stdout, "stdout", verbose, captured_log.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_drain_thread(stderr, "stderr", verbose, captured_log.clone());
        }

        Self {
            child,
            socket_path,
            _tmp_dir: tmp_dir,
            captured_log,
        }
    }

    /// Poll the child process's exit status for up to `timeout`, returning
    /// as soon as it has exited rather than sleeping out the whole timeout.
    /// For tests asserting a daemon fails fast on a bad startup config,
    /// instead of hanging or being silently killed once the test's own
    /// timeout elsewhere trips.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Every stdout/stderr line the daemon has printed so far, in the order
    /// each pipe delivered it (stdout/stderr interleaving is not preserved
    /// across the two independent drain threads, but every line from either
    /// stream is included). Lets a test assert on the daemon's own log output
    /// — e.g. that a startup failure named a specific, actionable reason —
    /// without needing `E2E_VERBOSE` to merely echo it.
    pub fn captured_log(&self) -> Vec<String> {
        self.captured_log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Continuously read `pipe` to EOF on a background thread: always appends
/// each line to the shared `captured` buffer (so a test can assert on it
/// afterward), and additionally echoes it to this process's own stderr under
/// `E2E_VERBOSE`. Must run for the daemon's entire lifetime — dropping the
/// thread (not reading to EOF) reintroduces the exact deadlock this exists
/// to prevent (see the comment at the call site).
fn spawn_drain_thread(
    pipe: impl Read + Send + 'static,
    label: &'static str,
    verbose: bool,
    captured: Arc<std::sync::Mutex<Vec<String>>>,
) {
    std::thread::spawn(move || {
        let reader = BufReader::new(pipe);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if verbose {
                eprintln!("[nodespaced {label}] {line}");
            }
            captured
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(line);
        }
    });
}

impl Drop for SpawnedDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The ONE process-wide lock for every test in this crate's consumers that
/// mutates the process-global `NODESPACED_SOCKET` env var — whether via
/// `GrpcClient::connect()` (which resolves it internally; there is no
/// per-call socket argument), `GrpcClient::connect_lazy()`, or a raw
/// `EnvGuard::set` for a Tauri command that reads it directly (e.g.
/// `check_daemon_status`). Without this mutex, two
/// `#[tokio::test]`s running concurrently — the default — can interleave
/// their env mutations, so one test's client ends up dialing whichever
/// socket happened to be live at the moment its call actually resolved the
/// env var, not the daemon its own test spawned. Held across `.await`
/// points, so this must be a `tokio::sync::Mutex`, not `std::sync::Mutex`.
///
/// `watcher::run` no longer resolves `NODESPACED_SOCKET` itself — it rides the
/// shared `GrpcClient` channel, which is built from the env var once at
/// `GrpcClient::connect()` time. Tests that spawn the watcher must therefore
/// hold a guard from `hold_connect_mutex_and_socket_env` across the
/// `GrpcClient` connection (and, conservatively, for the watcher's lifetime)
/// so the shared channel dials the daemon the test spawned — see
/// `optimistic_echo_race_test.rs`.
///
/// `pub` so every `tests/*.rs` file that mutates `NODESPACED_SOCKET` shares
/// this ONE lock rather than each defining its own same-purpose mutex under
/// a different name (`daemon_readiness_test.rs` used to define its own
/// `ENV_MUTEX` before this was made shared — harmless in practice, since
/// each `tests/*.rs` file compiles to its own process and the env var is
/// process-local, but a duplicated concept under two names invites a third
/// file inventing a fourth).
pub static CONNECT_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Wait until the daemon's socket is reachable, then connect a real
/// `GrpcClient` to it — the same client type `#[tauri::command]` handlers
/// take as `State<'_, GrpcClient>`. Each call builds an independent
/// `GrpcClient` with its own stable `x-ns-client-id` (ADR-026 C5 extension) —
/// calling `TauriTestApp::connect` twice against the same daemon therefore
/// produces two clients the daemon treats as genuinely different writers, the
/// same as two real desktop windows. See `optimistic_echo_race_test.rs`.
async fn connected_client(daemon: &SpawnedDaemon, timeout: Duration) -> GrpcClient {
    use nodespace_app_lib::daemon_setup::{wait_for_daemon, DaemonStatus};

    let _mutex_guard = CONNECT_MUTEX.lock().await;
    let _env_guard = EnvGuard::set("NODESPACED_SOCKET", &daemon.socket_path);

    let status = wait_for_daemon(&daemon.socket_path, timeout).await;
    assert_eq!(
        status,
        DaemonStatus::Healthy,
        "daemon never became healthy at {}",
        daemon.socket_path.display()
    );

    GrpcClient::connect()
        .await
        .unwrap_or_else(|e| panic!("failed to connect GrpcClient to spawned daemon: {e}"))
}

/// Holds `CONNECT_MUTEX` AND keeps `NODESPACED_SOCKET` set to `daemon`'s
/// socket for as long as the returned guard lives — for tests that spawn
/// `watcher::run`. The watcher rides the shared `GrpcClient` channel (built
/// from the env var at `connect()` time), so keeping the socket env stable
/// across the connection guarantees the channel dials the daemon the test
/// spawned. Drop the returned guard only after the watcher task has been
/// cancelled and joined.
pub async fn hold_connect_mutex_and_socket_env(
    daemon: &SpawnedDaemon,
) -> (tokio::sync::MutexGuard<'static, ()>, EnvGuard) {
    let mutex_guard = CONNECT_MUTEX.lock().await;
    let env_guard = EnvGuard::set("NODESPACED_SOCKET", &daemon.socket_path);
    (mutex_guard, env_guard)
}

/// A `tauri::test::mock_app()` (`MockRuntime`, no webview, no display) with a
/// real `GrpcClient` connected to `daemon` registered as managed state, so
/// tests can obtain `tauri::State<GrpcClient>` the sanctioned way —
/// `Manager::state()` — and call `#[tauri::command]` handler functions
/// directly. Also usable as the real Tauri event bus (`app.emit`/`app.listen`)
/// for tests that need to observe the watcher's forwarded events.
pub struct TauriTestApp {
    pub app: tauri::App<tauri::test::MockRuntime>,
}

impl TauriTestApp {
    /// Build a mock app and connect+manage a `GrpcClient` against `daemon`.
    pub async fn connect(daemon: &SpawnedDaemon, timeout: Duration) -> Self {
        let client = connected_client(daemon, timeout).await;
        let app = tauri::test::mock_app();
        app.manage(client);
        Self { app }
    }

    /// The managed `GrpcClient` as the `State<'_, GrpcClient>` a
    /// `#[tauri::command]` handler expects as its first argument.
    pub fn client_state(&self) -> tauri::State<'_, GrpcClient> {
        self.app.state::<GrpcClient>()
    }

    /// The real `AppHandle` — for commands (e.g. `chat_model_download`,
    /// `local_agent`'s streaming subscriptions) that take `app: AppHandle`
    /// to `emit` progress events, and for tests that `listen` on it.
    pub fn handle(&self) -> tauri::AppHandle<tauri::test::MockRuntime> {
        self.app.handle().clone()
    }
}

/// Sets an env var and restores its prior value on drop — including on a
/// panicking test, where plain "restore at the end of the function" code
/// never runs. Tests that mutate a process-global env var (like
/// `NODESPACED_SOCKET`) under a shared mutex should hold one of these for
/// the duration, so a failed assertion can't leak the mutated value to
/// whichever test acquires the mutex next.
pub struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    pub fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// True if `~/.nodespace/models/<filename>` already exists — the same
/// resolution `GgufModelManager::new()` uses (`$HOME/.nodespace/models`).
/// Tests that need a REAL loaded model (ai-chat send, model-download
/// terminal state) call this to skip cleanly with a clear message on a
/// machine that hasn't downloaded the model, instead of silently kicking
/// off a live multi-gigabyte fetch from HuggingFace and hanging the pre-push
/// gate for anyone but the one machine that already has it cached. There is
/// no provisioning step in `bun install`/`test-gate.ts` for this file today —
/// see the panic message for how to fetch it.
pub fn model_file_available(filename: &str) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    PathBuf::from(home)
        .join(".nodespace")
        .join("models")
        .join(filename)
        .exists()
}

/// The workspace root, derived from this crate's manifest directory
/// (`packages/desktop-app/src-tauri/test-support`). Canonicalised so paths
/// quoted in failure messages read as real locations rather than a chain of
/// `..` segments.
fn workspace_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("..");
    std::fs::canonicalize(&root).unwrap_or(root)
}

/// Crate sources compiled into `nodespaced`. A daemon binary older than the
/// newest file here is not the daemon this checkout describes, so these tests
/// would be asserting on some other build's behaviour.
const DAEMON_SOURCE_DIRS: [&str; 5] = [
    "packages/daemon/src",
    "packages/agent/src",
    "packages/core/src",
    "packages/nlp-engine/src",
    "packages/nodespace-types/src",
];

/// Newest modification time among the `.rs` files under [`DAEMON_SOURCE_DIRS`],
/// with the file that carries it. `None` when none of the directories exist (a
/// partial checkout), which disables the staleness check rather than failing on
/// it.
fn newest_source_mtime(root: &Path) -> Option<(std::time::SystemTime, PathBuf)> {
    fn walk(dir: &Path, newest: &mut Option<(std::time::SystemTime, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                walk(&path, newest);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                    continue;
                };
                if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                    *newest = Some((modified, path));
                }
            }
        }
    }

    let mut newest = None;
    for dir in DAEMON_SOURCE_DIRS {
        walk(&root.join(dir), &mut newest);
    }
    newest
}

/// `Err(explanation)` when `binary` is older than the daemon sources under
/// `root` — i.e. it is not a build of this checkout.
///
/// Every binary [`resolve_daemon_binary`] can pick is a git-ignored artifact
/// that nothing in the repo keeps in sync with the checkout: the sidecar under
/// `src-tauri/binaries/` is staged by hand and can be arbitrarily old, and
/// build outputs get copied between checkouts routinely (a fresh worktree has
/// no Rust build cache, so copying them in is the standard shortcut). Cargo's
/// own freshness check is mtime-based, so copied-in artifacts can date *newer*
/// than the sources they supposedly came from and register as up to date — a
/// preceding `cargo build --bin nodespaced` is therefore not on its own a
/// guarantee that the binary matches the checkout.
///
/// Left unchecked, the resulting failure is silent and deeply misleading: an old
/// daemon still speaks the current gRPC surface, so every RPC succeeds and only
/// its *behaviour* is wrong. It surfaces as an inference turn that never
/// produces a reply — a several-minute timeout in a test that then reads as a
/// regression in whatever change is being pushed, rather than as the wrong
/// binary being under test.
///
/// Unknowable inputs (missing binary, no daemon sources at `root`) return `Ok`:
/// this check exists to catch a specific, silent mismatch, not to add a second
/// way for the fixture to refuse to start.
///
/// `pub` so `tests/daemon_binary_freshness_test.rs` can exercise it against a
/// synthetic tree; the ADR-048 suite is where this crate's own logic gets run.
pub fn daemon_binary_freshness(binary: &Path, root: &Path) -> Result<(), String> {
    let Ok(binary_mtime) = std::fs::metadata(binary).and_then(|m| m.modified()) else {
        return Ok(());
    };
    let Some((source_mtime, source_path)) = newest_source_mtime(root) else {
        return Ok(());
    };
    if binary_mtime >= source_mtime {
        return Ok(());
    }
    Err(format!(
        "nodespaced at {} is OLDER than {} — it is not a build of this checkout, so these \
         tests would assert on a different daemon's behaviour. Rebuild it with \
         `cargo build --bin nodespaced` — and if this checkout's build artifacts were \
         copied in from elsewhere, delete the stale binary first, since cargo's \
         mtime-based freshness check treats a newly copied artifact as up to date. \
         Or point NODESPACED_TEST_BIN at a current build.",
        binary.display(),
        source_path.display(),
    ))
}

/// Resolve the `nodespaced` binary to spawn.
///
/// Checked in order:
///   1. `NODESPACED_TEST_BIN` env override — what `scripts/test-gate.ts` sets
///      to the daemon it builds immediately beforehand.
///   2. `target/debug/nodespaced`, this workspace's own `cargo build --bin
///      nodespaced` output. Preferred over the sidecar because it is the one
///      artifact a developer's own build actually refreshes.
///   3. The triple-suffixed sidecar under `src-tauri/binaries/`, the same
///      path (and naming convention) the TypeScript `DaemonTestHarness`
///      resolves — keeping one binary-provisioning story across both
///      harnesses rather than inventing a second convention.
///
/// None is built automatically; this intentionally mirrors the existing
/// e2e harness rather than adding `nodespace-daemon` as a dev-dependency of
/// this crate; the daemon crate pulls in `nodespace-nlp-engine` (llama.cpp
/// bindgen), which would tax every `cargo test` of this crate to save an
/// occasional manual build step. Whichever is resolved is checked against the
/// daemon's own sources — see [`daemon_binary_freshness`].
fn resolve_daemon_binary() -> PathBuf {
    let root = workspace_root();
    let binary_name = if cfg!(windows) {
        "nodespaced.exe"
    } else {
        "nodespaced"
    };

    let (binary, chosen_via) =
        if let Ok(p) = std::env::var("NODESPACED_TEST_BIN") {
            (PathBuf::from(p), "NODESPACED_TEST_BIN")
        } else {
            let built = root.join("target").join("debug").join(binary_name);
            if built.exists() {
                (built, "target/debug")
            } else {
                let triple = tauri::utils::platform::target_triple()
                    .expect("could not determine host target triple");
                let sidecar = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("binaries")
                    .join(format!("nodespaced-{triple}"));

                assert!(
                sidecar.exists(),
                "no nodespaced binary found: neither {} nor the sidecar at {} exists. Build one \
                 with `cargo build --bin nodespaced`, or set NODESPACED_TEST_BIN to an existing \
                 binary.",
                root.join("target").join("debug").join(binary_name).display(),
                sidecar.display()
            );

                (sidecar, "src-tauri/binaries sidecar")
            }
        };

    if let Err(explanation) = daemon_binary_freshness(&binary, &root) {
        panic!("{explanation} (resolved via {chosen_via})");
    }
    binary
}
