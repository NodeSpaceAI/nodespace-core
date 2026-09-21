//! Regression coverage: a `NODESPACED_SOCKET` override pointed at a custom
//! path used to make `nodespaced` silently never bind/listen —
//! no "gRPC server listening" line, no error, no socket file, while every
//! other startup step (schema seeding, embedding-model load, Play engine)
//! kept right on succeeding and logging normally. Root cause: the daemon's
//! socket-directory setup unconditionally re-`chmod`'d the socket's parent
//! directory to `0o700` even when that directory already existed and this
//! process didn't own it (most commonly the literal system temp root, e.g.
//! `NODESPACED_SOCKET=/tmp/my.sock`) — a `chmod` on a directory you don't own
//! always fails with `EPERM`, and that failure propagated as a plain `Err`
//! with nothing logging it loudly before the process eventually exited.
//!
//! Unix-only (`#[cfg(unix)]`): the bug, the fix, and both scenarios below are
//! about POSIX directory-permission handling. `create_dir_owner_only`'s
//! Windows variant does no `chmod` at all (Windows security for the daemon's
//! Named Pipe is DACL-based, a different mechanism entirely), so this
//! failure mode cannot occur there.
//!
//! Two scenarios, matching the two outcomes that must now hold:
//!   - A custom socket in a directory the daemon can actually secure (either
//!     because it creates the directory itself, or the directory is already
//!     owner-only) must bind and serve exactly like the default socket path.
//!   - A custom socket landing directly in a shared, foreign-owned directory
//!     genuinely cannot be secured (ADR-052's owner-only-directory invariant
//!     is meaningless there), so it must keep failing -- but now fast and
//!     loud, with a specific, actionable reason logged immediately, instead
//!     of silently hanging or failing with no visible signal.
//!
//! The second scenario deliberately hardcodes `/tmp` rather than using
//! `std::env::temp_dir()`/`TMPDIR`: on macOS those resolve to a *per-user*
//! directory under `/var/folders/.../T` that this test process itself owns
//! (confirmed directly -- a daemon pointed there via `NODESPACED_SOCKET`
//! binds and serves just fine, same as the first scenario below, because
//! `create_dir_owner_only`'s `chmod` succeeds when you own the directory).
//! `/tmp` itself, unlike `TMPDIR`, is the one location guaranteed on every
//! Unix to be a pre-existing, shared, non-owned directory (root-owned,
//! `0o1777`) -- the only reliable way to exercise this failure mode at all.

use nodespace_app_lib::daemon_setup::{wait_for_daemon, DaemonStatus};
use nodespace_app_test_support::{EnvGuard, SpawnedDaemon, DAEMON_CONNECT_TIMEOUT};
use std::time::Duration;

/// Neither test in this file cares about the embedding model -- they're
/// about socket-directory permission handling, which runs and resolves
/// before the (backgrounded) model load even starts. Pointing
/// `NODESPACED_MODEL_PATH` at a path that doesn't exist makes
/// `resolve_model_path()` return `None`, so no model-loading task is spawned
/// at all: no llama.cpp/Metal init, no GPU contention with whatever else is
/// running on the machine, and none of llama.cpp's very verbose native
/// stderr logging (not gated by `RUST_LOG`) cluttering the captured log this
/// file's assertions inspect.
fn no_model_load_guard() -> EnvGuard {
    EnvGuard::set(
        "NODESPACED_MODEL_PATH",
        std::env::temp_dir().join("nonexistent-nodespace-test-model.gguf"),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn custom_socket_in_a_freshly_created_owned_directory_binds_and_serves() {
    let _model_guard = no_model_load_guard();
    let tmp_dir = tempfile::tempdir().expect("create temp dir for daemon fixture");
    // `nested/` does not exist yet -- the daemon itself must create it
    // (owner-only) before it can bind a socket inside it. No existing test
    // exercised this: `SpawnedDaemon::spawn()`'s default socket path always
    // sits directly in a directory `tempfile::tempdir()` already created.
    let socket_path = tmp_dir.path().join("nested").join("custom.sock");
    let daemon = SpawnedDaemon::spawn_with_socket(tmp_dir, socket_path.clone());

    let status = wait_for_daemon(&socket_path, DAEMON_CONNECT_TIMEOUT).await;
    assert_eq!(
        status,
        DaemonStatus::Healthy,
        "a custom NODESPACED_SOCKET in a directory the daemon can secure must bind and serve, \
         matching the default socket path's behavior; log so far: {:#?}",
        daemon.captured_log()
    );
    assert!(
        socket_path.exists(),
        "the socket file must actually appear on disk once the daemon reports healthy"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn custom_socket_directly_in_a_foreign_shared_directory_fails_fast_and_loud() {
    let _model_guard = no_model_load_guard();
    // Isolate NODESPACE_HOME/the database as usual; only the socket path
    // itself is redirected to land directly in `/tmp` -- see the module doc
    // comment for why the literal path, not `std::env::temp_dir()`.
    let tmp_dir = tempfile::tempdir().expect("create temp dir for daemon fixture");
    let socket_path = std::path::PathBuf::from(format!(
        "/tmp/ns-regression-2759-{}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&socket_path);

    let mut daemon = SpawnedDaemon::spawn_with_socket(tmp_dir, socket_path.clone());

    // Bounded, but still generous relative to DAEMON_CONNECT_TIMEOUT (60s):
    // this must resolve in well under it -- the whole point of the fix is
    // that this is no longer an indefinite, silent hang. A `None` here
    // (still running) is itself a failure.
    let status = daemon
        .wait_for_exit(Duration::from_secs(60))
        .unwrap_or_else(|| {
            panic!(
                "daemon must exit promptly when its custom socket's directory cannot be secured, \
                 not hang silently; log so far: {:#?}",
                daemon.captured_log()
            )
        });

    assert!(
        !status.success(),
        "a daemon that could not bind its socket must exit non-zero, not silently keep running"
    );
    assert!(
        !socket_path.exists(),
        "the socket must never have been created when the bind never happened"
    );

    let log = daemon.captured_log().join("\n");
    assert!(
        log.contains("group/other-accessible") || log.contains("cannot be narrowed to owner-only"),
        "the failure must be logged immediately with a specific, actionable reason -- not a bare \
         permission error and not silence -- got:\n{log}"
    );
}
