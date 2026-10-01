//! ADR-048 seam coverage for the app's check of which daemon is running.
//!
//! A real headless `nodespaced` reports the path of its own executable through
//! `GetDaemonVersion`, and the app's `product_check` reads that report against
//! the active profile's daemon binary: the same daemon matches the community
//! profile and mismatches a profile naming another binary. The unhappy paths of
//! the RPC helper (nothing listening, a daemon that never answers) must come
//! back as "unknown", never as a mismatch that would evict a daemon.

use nodespace_app_lib::daemon_profile::DaemonProfile;
use nodespace_app_lib::daemon_setup::{product_check, running_daemon_executable, ProductCheck};
use nodespace_app_test_support::{SpawnedDaemon, TauriTestApp, DAEMON_CONNECT_TIMEOUT};

#[tokio::test]
async fn a_real_daemon_reports_its_executable_and_the_check_reads_it() {
    let daemon = SpawnedDaemon::spawn();
    let harness = TauriTestApp::connect(&daemon, DAEMON_CONNECT_TIMEOUT).await;
    let client = harness.client_state();

    let reported = running_daemon_executable(&client)
        .await
        .expect("a healthy daemon must answer GetDaemonVersion");

    let file_name = std::path::Path::new(&reported)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    assert_eq!(
        file_name.strip_suffix(".exe").unwrap_or(file_name),
        "nodespaced",
        "the daemon must report its own executable path, got {reported:?}"
    );

    assert_eq!(
        product_check(Some(&reported), &DaemonProfile::community()),
        ProductCheck::Match
    );
    let other = DaemonProfile {
        binary_name: "custom-daemon",
        ..DaemonProfile::community()
    };
    assert_eq!(
        product_check(Some(&reported), &other),
        ProductCheck::Mismatch
    );
}

#[cfg(unix)]
mod unhappy_paths {
    use nodespace_app_lib::daemon_profile::DaemonProfile;
    use nodespace_app_lib::daemon_setup::{product_check, running_daemon_executable, ProductCheck};
    use nodespace_app_lib::services::GrpcClient;
    use nodespace_app_test_support::{EnvGuard, CONNECT_MUTEX};
    use std::path::Path;
    use std::time::Duration;

    /// A lazy client whose channel dials `socket`. The path is read from
    /// `NODESPACED_SOCKET` when the client is built and captured by its channel,
    /// so the process-global variable need only be held for that moment.
    async fn client_dialing(socket: &Path) -> GrpcClient {
        let _mutex = CONNECT_MUTEX.lock().await;
        let _env = EnvGuard::set("NODESPACED_SOCKET", socket);
        GrpcClient::connect_lazy()
    }

    #[tokio::test]
    async fn a_daemon_that_cannot_be_reached_is_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let client = client_dialing(&dir.path().join("absent.sock")).await;

        let reported = running_daemon_executable(&client).await;

        assert_eq!(reported, None);
        assert_eq!(
            product_check(reported.as_deref(), &DaemonProfile::community()),
            ProductCheck::Unknown
        );
    }

    #[tokio::test]
    async fn a_daemon_that_never_answers_is_unknown_after_the_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("hung.sock");
        // Connections complete in the kernel's backlog and nothing ever reads
        // them: a daemon stuck before it serves.
        let _hung = std::os::unix::net::UnixListener::bind(&socket).expect("bind test socket");
        let client = client_dialing(&socket).await;

        // Bounded here, far above the helper's own timeout, so a helper with no
        // timeout of its own fails this test instead of hanging the suite.
        let reported =
            tokio::time::timeout(Duration::from_secs(20), running_daemon_executable(&client))
                .await
                .expect("running_daemon_executable must give up on a daemon that never answers");

        assert_eq!(reported, None);
    }
}
