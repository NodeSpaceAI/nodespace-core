//! ADR-048 seam coverage for the app's check of which daemon is running.
//!
//! A real headless `nodespaced` reports the path of its own executable through
//! `GetDaemonVersion`, and the app's `product_check` reads that report against
//! the active profile's daemon binary: the same daemon matches the community
//! profile and mismatches a profile naming another binary. The unhappy paths of
//! the RPC helper (nothing listening, a daemon that never answers) must come
//! back as "unknown", never as a mismatch that would evict a daemon.

use nodespace_app_lib::daemon_profile::DaemonProfile;
use nodespace_app_lib::daemon_setup::{
    product_check, running_daemon_executable, wait_for_daemon, DaemonStatus, ProductCheck,
};
use nodespace_app_test_support::{SpawnedDaemon, DAEMON_CONNECT_TIMEOUT};

#[tokio::test]
async fn a_real_daemon_reports_its_executable_and_the_check_reads_it() {
    let daemon = SpawnedDaemon::spawn();
    assert_eq!(
        wait_for_daemon(&daemon.socket_path, DAEMON_CONNECT_TIMEOUT).await,
        DaemonStatus::Healthy,
        "the daemon never became healthy"
    );

    let reported = running_daemon_executable(&daemon.socket_path)
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
    use std::time::Duration;

    #[tokio::test]
    async fn a_daemon_that_cannot_be_reached_is_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");

        let reported = running_daemon_executable(&dir.path().join("absent.sock")).await;

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

        // Bounded here, far above the helper's own timeout, so a helper with no
        // timeout of its own fails this test instead of hanging the suite.
        let reported =
            tokio::time::timeout(Duration::from_secs(20), running_daemon_executable(&socket))
                .await
                .expect("running_daemon_executable must give up on a daemon that never answers");

        assert_eq!(reported, None);
    }
}
