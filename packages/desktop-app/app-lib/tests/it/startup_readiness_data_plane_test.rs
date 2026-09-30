//! ADR-048 priority flow 5 — startup readiness, extended to the DATA PLANE.
//!
//! `daemon_readiness_test.rs` already covers the readiness SIGNAL
//! (`check_daemon_status` / `wait_for_daemon` agreeing on not_running vs.
//! healthy for a real daemon). What it does not cover is what actually
//! happens on the command layer during the not-ready window: does a data
//! command fail cleanly (a real, catchable error) rather than hanging or
//! panicking, and does the exact same command succeed immediately once the
//! daemon is healthy — using the SAME `GrpcClient`, so this also proves the
//! lazy-connect story (a client constructed before the daemon is reachable
//! recovers on its own, with no explicit reconnect step) that the frontend
//! actually relies on at app startup.
//!
//! "Degraded state" as a Svelte-visible concept (`daemonStatus.unreachable`,
//! store self-healing on reconnect) is ADR-044's contract and already has
//! real, thorough e2e coverage in
//! `src/tests/e2e/daemon-readiness.e2e.ts` — deliberately not duplicated
//! here. There is no separate Rust/daemon-side "degraded" enum variant
//! (confirmed: `DaemonStatus` is `Healthy` / `Starting` / `NotRunning`);
//! "degraded" is a frontend-store concept built on top of that signal.

use std::time::Duration;

use nodespace_app_lib::commands::nodes::{create_node, CreateNodeInput};
use nodespace_app_test_support::{
    hold_connect_mutex_and_socket_env, SpawnedDaemon, DAEMON_CONNECT_TIMEOUT,
};
use serde_json::json;
use tauri::Manager;

fn text_input(id: &str) -> CreateNodeInput {
    CreateNodeInput {
        id: id.to_string(),
        node_type: "text".to_string(),
        content: "hello".to_string(),
        parent_id: None,
        insert_position: None,
        properties: json!({}),
    }
}

/// A `create_node` call issued while the daemon is not yet reachable must
/// fail with a real, catchable error — not hang indefinitely and not panic
/// — and the SAME client, retried after the daemon becomes healthy, must
/// then succeed with no reconnect step of its own. This is the data-plane
/// analogue of `daemon_readiness_test.rs`'s not_running -> healthy signal
/// transition, and it is what the frontend's lazy `GrpcClient` (managed at
/// app setup before the daemon socket necessarily exists — see
/// `services/grpc_client.rs`'s `connect_lazy` doc comment) depends on in
/// production.
#[tokio::test]
async fn a_command_fails_cleanly_before_readiness_and_succeeds_after_without_reconnecting() {
    let daemon = SpawnedDaemon::spawn();

    // Point a lazy client at the not-yet-bound socket — mirrors
    // GrpcClient::connect_lazy(), which the Tauri app actually manages at
    // startup so an early command gets a retryable error instead of a fatal
    // "state not managed" panic. Held for the whole test since connect_lazy
    // reads NODESPACED_SOCKET synchronously at call time, same hazard
    // documented on hold_connect_mutex_and_socket_env.
    let _socket_guard = hold_connect_mutex_and_socket_env(&daemon).await;
    let client = nodespace_app_lib::services::GrpcClient::connect_lazy();

    let app = tauri::test::mock_app();
    app.manage(client);
    let state = app.state::<nodespace_app_lib::services::GrpcClient>();

    // Not ready yet: the daemon binary was only just spawned and does not
    // bind its socket instantly. A command issued now must return a real
    // error promptly, not hang.
    let early_result = tokio::time::timeout(
        Duration::from_secs(5),
        create_node(state.clone(), text_input(&uuid::Uuid::new_v4().to_string())),
    )
    .await;
    match early_result {
        Ok(Ok(_)) => {
            // The daemon happened to bind fast enough that this "early" call
            // landed after readiness — acceptable (timing, not a contract
            // violation), but then the interesting assertion below is moot
            // for this run. Rare on a cold process; not treated as a failure.
        }
        Ok(Err(_)) => {
            // Expected: a real, structured CommandError — not a hang.
        }
        Err(_) => panic!("create_node before daemon readiness must fail promptly, not hang"),
    }

    // Now wait for real readiness and retry with the SAME client/state — no
    // new GrpcClient, no explicit reconnect call.
    let status = nodespace_app_lib::daemon_setup::wait_for_daemon(
        &daemon.socket_path,
        DAEMON_CONNECT_TIMEOUT,
    )
    .await;
    assert_eq!(
        status,
        nodespace_app_lib::daemon_setup::DaemonStatus::Healthy,
        "daemon never became healthy"
    );

    let id = uuid::Uuid::new_v4().to_string();
    create_node(state.clone(), text_input(&id))
        .await
        .expect("create_node must succeed once the daemon is healthy, using the same client");
}
