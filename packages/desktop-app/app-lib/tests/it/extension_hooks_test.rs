//! The extension hooks against a real `nodespaced`: an extension's daemon-ready
//! task and channel-rebuilt hook reach the daemon on the channel core owns.
//!
//! The unit tests in `extensions/fixture_tests.rs` cover the hooks' semantics on
//! a mock runtime with no daemon: ordering, timeouts, panics, run-once. What
//! they cannot show is that the channel a hook is handed is one that works, so
//! these tests make a real RPC on it.

use nodespace_app_lib::extensions::{spawn_daemon_ready_tasks, CancellationToken};
use nodespace_app_lib::AppExtensions;
use nodespace_app_test_support::{SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT};
use nodespace_proto::{DatabaseServiceClient, ListDatabasesRequest};
use tonic::transport::Channel;

/// Lists the registered databases over `channel`, reporting how many there are.
async fn list_databases(channel: Channel) -> Result<usize, String> {
    DatabaseServiceClient::new(channel)
        .list(ListDatabasesRequest {})
        .await
        .map(|response| response.into_inner().databases.len())
        .map_err(|status| status.to_string())
}

#[tokio::test]
async fn daemon_ready_task_reaches_the_daemon_on_the_shared_channel() {
    let daemon = SpawnedDaemon::spawn();
    let (report, reported) = tokio::sync::oneshot::channel::<Result<usize, String>>();
    let harness = TauriTestApp::connect_assembled(
        &daemon,
        DAEMON_CONNECT_TIMEOUT,
        AppExtensions::none().on_daemon_ready(move |ready| async move {
            let _ = report.send(list_databases(ready.grpc.channel().await).await);
        }),
    )
    .await;

    // What core's startup does once the daemon is up, with the managed client.
    let managed = harness.client_state().inner().clone();
    spawn_daemon_ready_tasks(&harness.handle(), managed, &CancellationToken::new());

    let listed = tokio::time::timeout(DAEMON_CONNECT_TIMEOUT, reported)
        .await
        .expect("the daemon-ready task ran")
        .expect("the task reported back");
    assert!(
        listed.is_ok(),
        "a DatabaseService List on the shared channel must succeed, got {listed:?}"
    );
}

/// Binds a socket that accepts connections and never answers on them, so a
/// client dialing it hangs the way a wedged channel does. Holds every accepted
/// connection open until the returned task is aborted.
#[cfg(unix)]
fn silent_listener(path: &std::path::Path) -> tokio::task::JoinHandle<()> {
    let listener = tokio::net::UnixListener::bind(path).expect("bind the silent socket");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((connection, _)) = listener.accept().await {
            held.push(connection);
        }
    })
}

#[cfg(unix)]
#[tokio::test]
async fn a_wedged_channel_rebuild_runs_the_hooks_before_recovery_returns() {
    use std::sync::{Arc, Mutex};

    use nodespace_app_lib::commands::nodes::probe_and_recover;
    use nodespace_app_lib::services::GrpcClient;
    use nodespace_app_test_support::{hold_connect_mutex_and_socket_env, EnvGuard, CONNECT_MUTEX};

    let daemon = SpawnedDaemon::spawn();
    let silent_dir = tempfile::tempdir().expect("create a directory for the silent socket");
    let silent_socket = silent_dir.path().join("silent.sock");
    let silent = silent_listener(&silent_socket);

    // A client whose channel dials the silent socket: every call on it hangs.
    // The socket is resolved when the client is built, so the override only
    // needs to hold until then.
    let wedged = {
        let _mutex = CONNECT_MUTEX.lock().await;
        let _socket = EnvGuard::set("NODESPACED_SOCKET", &silent_socket);
        GrpcClient::connect_lazy()
    };

    let hook_result: Arc<Mutex<Option<Result<usize, String>>>> = Arc::default();
    let recorded = Arc::clone(&hook_result);
    // `connect_assembled` takes the connect mutex itself, so it runs between
    // the two sections that hold it.
    let harness = TauriTestApp::connect_assembled(
        &daemon,
        DAEMON_CONNECT_TIMEOUT,
        AppExtensions::none().on_channel_rebuilt(move |_app, channel| {
            let recorded = Arc::clone(&recorded);
            async move {
                let result = list_databases(channel).await;
                *recorded.lock().unwrap() = Some(result);
            }
        }),
    )
    .await;

    // The rebuild resolves the socket again, and this time it must find the
    // real daemon. Held until the end, restored in reverse order.
    let (_mutex, _socket) = hold_connect_mutex_and_socket_env(&daemon).await;
    let recovered = probe_and_recover(&harness.handle(), &wedged).await;

    assert!(
        recovered,
        "the channel rebuilt on the live daemon must answer the re-probe"
    );
    let hook_result = hook_result.lock().unwrap().take();
    assert!(
        matches!(hook_result, Some(Ok(_))),
        "the hook's List on the rebuilt channel must have completed before recovery \
         returned, got {hook_result:?}"
    );

    silent.abort();
}
