//! The desktop app and the daemon must resolve the same registry and the same
//! default database for a given environment.
//!
//! When they do not, the app asks one daemon for a database id remembered from
//! another, and every call answers `no database registered with id`. These tests
//! start a real `nodespaced` with no socket or database override, so it resolves
//! its own paths the way an install does, and compare that with where an app
//! launched in the same environment looks.
//!
//! Two environments matter:
//!
//! * `HOME` only: the app's default socket is the daemon's socket, and the
//!   default database the app lists is the one in the registry file under that
//!   home.
//! * `NODESPACE_HOME` set apart from `HOME`: only the daemon moves. The app still
//!   dials under its own home, so it does not reach this daemon, and nothing is
//!   written under that home. A scratch launch of the app therefore has to move
//!   `HOME` (and the socket) as well; setting `NODESPACE_HOME` alone leaves the
//!   app on whatever daemon the account's own home runs.

#![cfg(unix)]

use std::path::Path;

use nodespace_app_lib::daemon_setup::{check_daemon_socket, wait_for_daemon, DaemonStatus};
use nodespace_app_lib::services::grpc_client::default_socket_path_in;
use nodespace_app_lib::services::GrpcClient;
use nodespace_app_test_support::{EnvGuard, SpawnedDaemon, CONNECT_MUTEX, DAEMON_CONNECT_TIMEOUT};
use nodespace_proto::nodespace::ListDatabasesRequest;
use nodespace_proto::socket::STATE_DIR;

/// The `default_database` the registry file under `home` names, if any.
fn registry_default(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join(STATE_DIR).join("databases.toml")).ok()?;
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("default_database")?;
        let value = value.trim_start().strip_prefix('=')?;
        Some(value.trim().trim_matches('"').to_string())
    })
}

#[tokio::test]
async fn app_and_daemon_share_the_default_database_under_one_home() {
    let _mutex_guard = CONNECT_MUTEX.lock().await;
    let home = tempfile::tempdir().unwrap();

    let daemon = SpawnedDaemon::spawn_in_homes(home.path(), None);
    let app_socket = default_socket_path_in(home.path(), cfg!(debug_assertions));
    assert_eq!(
        app_socket, daemon.socket_path,
        "the socket the app dials by default must be the one the daemon binds"
    );
    assert_eq!(
        wait_for_daemon(&app_socket, DAEMON_CONNECT_TIMEOUT).await,
        DaemonStatus::Healthy,
        "a daemon started in this home must be reachable at the app's default socket"
    );

    let _env_guard = EnvGuard::set("NODESPACED_SOCKET", &app_socket);
    let client = GrpcClient::connect().await.expect("connect to the daemon");
    let mut databases = client.database_service_client().await;
    let listing = databases
        .list(ListDatabasesRequest {})
        .await
        .expect("list databases")
        .into_inner();

    assert!(
        !listing.default_database_id.is_empty(),
        "a daemon seeds a default database for the app to list"
    );
    assert_eq!(
        registry_default(home.path()).as_deref(),
        Some(listing.default_database_id.as_str()),
        "the default the app lists must be the one in the registry file under the shared home"
    );
}

#[tokio::test]
async fn a_redirected_daemon_is_not_the_one_the_app_dials_by_default() {
    let _mutex_guard = CONNECT_MUTEX.lock().await;
    let account_home = tempfile::tempdir().unwrap();
    let redirect = tempfile::tempdir().unwrap();

    let daemon = SpawnedDaemon::spawn_in_homes(account_home.path(), Some(redirect.path()));
    assert_eq!(
        wait_for_daemon(&daemon.socket_path, DAEMON_CONNECT_TIMEOUT).await,
        DaemonStatus::Healthy,
        "the redirected daemon serves under its redirected home"
    );
    assert!(
        daemon.socket_path.starts_with(redirect.path()),
        "the redirected daemon binds inside NODESPACE_HOME, not HOME"
    );

    let app_socket = default_socket_path_in(account_home.path(), cfg!(debug_assertions));
    assert_ne!(app_socket, daemon.socket_path);
    assert_eq!(
        check_daemon_socket(&app_socket).await,
        DaemonStatus::NotRunning,
        "an app in the account's home does not reach a daemon redirected elsewhere: \
         a scratch launch has to move HOME and the socket too"
    );
    assert!(
        !account_home.path().join(STATE_DIR).exists(),
        "the redirected daemon created state under the account's home"
    );
}
