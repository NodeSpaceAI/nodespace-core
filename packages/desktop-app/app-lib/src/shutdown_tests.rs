//! Regression coverage for the `CloseRequested`-veto shutdown race: Tauri
//! fires `WindowEvent::CloseRequested` the instant the OS asks to close a
//! window — before the frontend's `onCloseRequested` veto (which flushes
//! pending writes first) has had any chance to run. `graceful_shutdown` must
//! never be driven from that event, only from `ExitRequested`/`Exit`, which
//! Tauri can't reach until the window has actually been destroyed. See
//! `handle_run_event`'s doc comment in `lib.rs` for the full reasoning.
//!
//! These use `tauri::test`'s `MockRuntime` (a real Tauri event bus and
//! window lifecycle, no webview/display) rather than plain unit tests,
//! because the bug this covers lived in *which* `RunEvent` variant triggers
//! shutdown — a mapping that only exists inside Tauri's actual window-close
//! machinery. Most of the types involved (`CloseRequestApi`, `ExitRequestApi`)
//! have no public constructor a hand-built `RunEvent` could stand in for
//! (confirmed against the `tauri` 2.11 source), so exercising the real
//! dispatch is the only faithful way to test the routing.

use std::sync::mpsc;
use std::time::Duration;

use tauri::Manager;

use super::{arm_quit_watchdog, graceful_shutdown, handle_run_event, ShutdownToken};

/// Bound on every synchronization wait below. Generous relative to
/// `MockRuntime::run()`'s own 1s idle-poll interval (it sleeps 1s between
/// checking for queued window messages — see the `tauri` source), but still
/// short enough that a real regression fails fast instead of hanging the
/// suite.
const WAIT: Duration = Duration::from_secs(10);

/// `graceful_shutdown` cancels the token on its first call and is a no-op —
/// not a panic, not a double-cancel, not un-cancelling — on every call
/// after. Mirrors `handle_run_event` calling it from both `ExitRequested`
/// and `Exit` for the same exit.
#[test]
fn graceful_shutdown_is_idempotent() {
    let app = tauri::test::mock_app();
    let token = ShutdownToken::new();
    app.manage(token.clone());
    let handle = app.handle().clone();

    assert!(!token.is_cancelled(), "token starts uncancelled");

    graceful_shutdown(&handle);
    assert!(token.is_cancelled(), "first call cancels the token");

    graceful_shutdown(&handle);
    assert!(
        token.is_cancelled(),
        "second call must be a harmless no-op, not a panic"
    );
}

/// The actual regression. Drives a real (mocked) window through the exact
/// sequence the frontend produces when it vetoes a close to flush pending
/// writes — `CloseRequested` observed and prevented, then later an explicit
/// `destroy()` once the flush is done — through `handle_run_event`, the
/// same function `run()` wires into `app.run` in production.
///
/// Asserts the shutdown token is still live at the moment `CloseRequested`
/// is observed (the bug: the old code cancelled it right there, before the
/// frontend had decided anything) and only goes down once the window is
/// actually destroyed and the app exits. Reverting `handle_run_event`'s
/// `CloseRequested` arm to call `graceful_shutdown` (the pre-fix behavior)
/// makes this fail at the first assertion below.
///
/// MockRuntime has no real webview, so there's no JS `onCloseRequested`
/// listener for Tauri's own manager to detect — the mechanism that makes
/// Tauri hold every close open pending the frontend's decision in the real
/// app. This test stands in for that by calling `api.prevent_close()`
/// itself upon observing the event, which is the same signal Tauri's
/// manager would have sent on the listener's behalf.
#[test]
fn close_requested_does_not_cancel_token_until_window_is_destroyed() {
    // Hard ceiling on this test's wall-clock cost: if the synchronization
    // below is ever wrong in a way that makes `app.run()` block forever, fail
    // loudly instead of hanging the suite. Harmless on the success path —
    // process exit at the end of the test binary kills this thread outright.
    std::thread::spawn(|| {
        std::thread::sleep(WAIT * 3);
        eprintln!(
            "close_requested_does_not_cancel_token_until_window_is_destroyed \
             did not finish in time — aborting to avoid hanging the suite"
        );
        std::process::exit(101);
    });

    let app = tauri::test::mock_app();
    let token = ShutdownToken::new();
    app.manage(token.clone());

    let window = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("failed to build mock window");

    // Synchronizes the background "frontend" thread with the app's event
    // loop: it must not call close()/destroy() before the loop is actually
    // running (a message sent before `run()` starts is handled synchronously
    // and bypasses RunEvent delivery entirely — see `RuntimeContext::
    // send_message` in the `tauri` source), and it must not call destroy()
    // until this test has observed and recorded the CloseRequested state.
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (close_observed_tx, close_observed_rx) = mpsc::channel::<()>();

    let bg_window = window.clone();
    let simulated_frontend = std::thread::spawn(move || {
        ready_rx
            .recv_timeout(WAIT)
            .expect("app never signalled RunEvent::Ready");
        bg_window.close().expect("close() failed");

        close_observed_rx
            .recv_timeout(WAIT)
            .expect("CloseRequested was never observed");
        // Mirrors the frontend calling `currentWindow.destroy()` once its
        // flush resolves (or hits its own timeout) — the only thing that can
        // actually close a window once a close-requested listener has
        // vetoed it.
        bg_window.destroy().expect("destroy() failed");
    });

    // Carries the token's cancelled-state observed *inside* the run-loop
    // callback back out to this test. A plain `let mut` captured by the
    // `move` closure below would NOT do this: `bool`/`Option<bool>` are
    // `Copy`, so the closure would silently mutate its own copy and this
    // outer scope would only ever see the initial value — a channel (or an
    // `Arc<Mutex<_>>`) is required to actually observe closure-internal state
    // from outside it.
    let (state_tx, state_rx) = mpsc::channel::<bool>();
    let token_for_run = token.clone();

    app.run(move |app_handle, event| {
        if matches!(event, tauri::RunEvent::Ready) {
            let _ = ready_tx.send(());
        }

        // The exact function `run()` wires into `app.run` in production.
        handle_run_event(app_handle, &event);

        if let tauri::RunEvent::WindowEvent {
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } = &event
        {
            let _ = state_tx.send(token_for_run.is_cancelled());
            api.prevent_close();
            let _ = close_observed_tx.send(());
        }
    });

    simulated_frontend
        .join()
        .expect("simulated-frontend thread panicked");

    let token_cancelled_at_close_requested = state_rx
        .recv_timeout(WAIT)
        .expect("test never observed CloseRequested");
    assert!(
        !token_cancelled_at_close_requested,
        "shutdown token must not be cancelled while the close is still vetoed"
    );
    assert!(
        token.is_cancelled(),
        "shutdown token must be cancelled once the window is actually \
         destroyed and the app exits"
    );
}

/// The crashed/hung-webview case this watchdog exists to guard against: if
/// the normal close path never reaches `ExitRequested`/`Exit` at all, the
/// watchdog's `on_timeout` must still fire so tray "Quit" doesn't hang
/// forever.
///
/// Uses a channel rather than a fixed sleep-then-check margin: this test
/// waits exactly as long as it takes for `on_timeout` to actually fire (fast
/// in practice), bounded by a generous `recv_timeout` so a loaded CI runner
/// delaying the watchdog thread's own scheduling can't produce a false
/// failure the way a fixed short sleep could.
#[test]
fn arm_quit_watchdog_forces_exit_when_shutdown_never_starts() {
    let token = ShutdownToken::new();
    let (tx, rx) = mpsc::channel::<()>();

    arm_quit_watchdog(token, Duration::from_millis(20), move || {
        let _ = tx.send(());
    });

    rx.recv_timeout(WAIT)
        .expect("on_timeout must fire once the timeout elapses with the token never started");
}

/// The healthy-webview case: once the normal close path has already driven
/// `graceful_shutdown` -- i.e. `ExitRequested`/`Exit` genuinely fired --
/// forcing another exit would be redundant, so the watchdog must find that
/// and skip calling `on_timeout`.
#[test]
fn arm_quit_watchdog_is_a_no_op_once_shutdown_already_started() {
    let app = tauri::test::mock_app();
    let token = ShutdownToken::new();
    app.manage(token.clone());
    let handle = app.handle().clone();

    // Simulates ExitRequested/Exit having already fired via the normal path
    // before the watchdog gets a chance to check.
    graceful_shutdown(&handle);

    let (tx, rx) = mpsc::channel::<()>();
    arm_quit_watchdog(token, Duration::from_millis(20), move || {
        let _ = tx.send(());
    });

    // Proving a negative still requires waiting out the deadline (a channel
    // alone can't shortcut that), but the wait here is bounded to the
    // timeout plus a fixed, generous margin rather than a wholly separate
    // guessed sleep duration.
    match rx.recv_timeout(Duration::from_millis(300)) {
        // Timeout: on_timeout was never called at all, so `tx` is still
        // parked inside it, unsent. Disconnected: on_timeout's closure (and
        // the `tx` it owns) was dropped without sending, once the watchdog
        // thread found shutdown already started and skipped calling it.
        // Both correctly mean "never fired" -- only Ok(()) means it did.
        Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {}
        Ok(()) => {
            panic!("on_timeout must not fire once shutdown has already started before the deadline")
        }
    }
}
