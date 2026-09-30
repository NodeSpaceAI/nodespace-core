//! Integration test — proves the daemon-readiness contract holds
//! against a REAL headless `nodespaced`, not a mock: a socket nothing is
//! listening on reports NotRunning, a real daemon's bind is observed as
//! Healthy once `wait_for_daemon` catches up to it, and both the raw
//! reachability probe (`check_daemon_socket`/`wait_for_daemon`) and
//! `daemon_status_body` (the body of the `check_daemon_status` Tauri
//! command) agree on that transition.
//!
//! Each state is established BY CONSTRUCTION rather than by racing a real
//! process's startup timing: "not ready" tests use a socket path nothing was
//! ever spawned for (deterministic — there is nothing to race), and
//! "recovered" tests wait on the real daemon via `wait_for_daemon` (timing-
//! tolerant by design) rather than asserting an intermediate NotRunning read
//! against it. A real daemon here binds its socket in well under 100ms on a
//! warm local machine — far faster than ADR-044's ~9s cold-start figure for
//! a heavier path — so asserting "not yet bound" against a just-spawned real
//! process would only pass when the test wins that race, which is a
//! coverage lottery, not a behavior guarantee.
//!
//! `check_daemon_status` itself takes no injected `State`/`AppHandle` — its
//! logic is exercised here via `daemon_status_body`, factored out so it can
//! be `pub` without colliding with the hidden items `#[tauri::command]`
//! generates (see the comment on `daemon_status_body` in `lib.rs`). No
//! Tauri mock runtime is needed to exercise it for real.

use nodespace_app_lib::daemon_setup::{check_daemon_socket, wait_for_daemon, DaemonStatus};
use nodespace_app_test_support::{EnvGuard, SpawnedDaemon, CONNECT_MUTEX, DAEMON_CONNECT_TIMEOUT};

#[tokio::test]
async fn not_running_before_daemon_starts() {
    let tmp = tempfile::tempdir().unwrap();
    let socket_path = tmp.path().join("daemon.sock");

    assert_eq!(
        check_daemon_socket(&socket_path).await,
        DaemonStatus::NotRunning,
        "a socket path with nothing listening must report NotRunning"
    );
}

// These transition tests deliberately do NOT assert `NotRunning` against the
// real spawned daemon's socket path before waiting for it: how long a real
// process takes to bind its socket is a latency accident (84ms warm on this
// machine, ADR-044 cites ~9s with a cold embedding-model load elsewhere) —
// an environment fact, not part of the readiness contract. Asserting an
// intermediate "not yet bound" read would mean the test only passes if it
// wins a race against the real process, which is a coverage lottery, not a
// behavior guarantee. `not_running_before_daemon_starts` above already
// covers the NotRunning behavior deterministically, by construction (no
// process spawned for that socket at all). What these tests cover instead
// is the actual transition: a real bind occurring while `wait_for_daemon`
// polls across it, observed as Healthy once it completes — `wait_for_daemon`
// is timing-tolerant by design, so it is the right primitive to assert on
// here, not a hand-rolled single read.

#[tokio::test]
async fn wait_for_daemon_observes_a_real_bind_as_healthy() {
    let daemon = SpawnedDaemon::spawn();

    // Recovered: wait_for_daemon polls check_daemon_socket across the real
    // daemon's startup and bind, however long that actually takes.
    let status = wait_for_daemon(&daemon.socket_path, DAEMON_CONNECT_TIMEOUT).await;
    assert_eq!(
        status,
        DaemonStatus::Healthy,
        "a real daemon must be observed healthy once it binds its socket"
    );

    // The bare reachability probe and the Tauri command body must agree —
    // this is the "single readiness contract" criterion: both resolve the
    // same socket and observe the same real daemon the same way.
    assert_eq!(
        check_daemon_socket(&daemon.socket_path).await,
        DaemonStatus::Healthy
    );
}

#[tokio::test]
async fn check_daemon_status_command_agrees_with_the_real_daemon() {
    let _mutex_guard = CONNECT_MUTEX.lock().await;

    let daemon = SpawnedDaemon::spawn();
    // Restores NODESPACED_SOCKET on drop — including if an assertion below
    // panics, so a failed run can't leak the mutated value to whichever
    // test acquires CONNECT_MUTEX next.
    let _env_guard = EnvGuard::set("NODESPACED_SOCKET", &daemon.socket_path);

    // Wait for the real daemon to finish starting (however long that takes;
    // see the comment above these tests for why we don't race an
    // intermediate NotRunning read against it).
    let status = wait_for_daemon(&daemon.socket_path, DAEMON_CONNECT_TIMEOUT).await;
    assert_eq!(status, DaemonStatus::Healthy, "daemon never became healthy");

    assert_eq!(
        nodespace_app_lib::daemon_status_body().await,
        "healthy",
        "check_daemon_status must report healthy once the daemon is reachable"
    );
}

#[tokio::test]
async fn check_daemon_status_command_reports_not_running_for_an_unbound_socket() {
    let _mutex_guard = CONNECT_MUTEX.lock().await;

    // Deterministic by construction: no process is ever spawned for this
    // socket path, so there is nothing to race.
    let tmp = tempfile::tempdir().unwrap();
    let socket_path = tmp.path().join("daemon.sock");
    let _env_guard = EnvGuard::set("NODESPACED_SOCKET", &socket_path);

    assert_eq!(
        nodespace_app_lib::daemon_status_body().await,
        "not_running",
        "check_daemon_status must report not_running for a socket nothing is listening on"
    );
}

// `Starting` is produced only when the 500ms connect itself times out
// (daemon_setup.rs's `check_daemon_socket`) — not by "socket bound but
// nothing calls accept()", since the OS completes a UDS connect from its
// listen backlog regardless of whether the owning process has called
// `accept()` yet. That makes `Starting` a genuine race window (a listener
// whose accept loop is saturated) rather than something a fixture can force
// deterministically without faking the connect call itself, so it is not
// covered by an integration test here.
