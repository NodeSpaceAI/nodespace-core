//! ADR-048 seam coverage for the app's check of which daemon is running.
//!
//! A real headless `nodespaced` reports the extension ids it supports through
//! `GetDaemonVersion`, and the app's `product_check` compares that set with the
//! active profile's: core's daemon reports none, so it matches the community
//! profile and mismatches a profile that expects an extension. The unhappy
//! paths of the RPC helper (nothing listening, a daemon that never answers)
//! must come back as "unknown", never as a mismatch that would evict a daemon.

use nodespace_app_lib::daemon_profile::DaemonProfile;
use nodespace_app_lib::daemon_setup::{
    product_check, running_daemon_extensions, wait_for_daemon, DaemonStatus, ProductCheck,
};
use nodespace_app_test_support::{SpawnedDaemon, DAEMON_CONNECT_TIMEOUT};

#[tokio::test]
async fn a_real_daemon_reports_its_extensions_and_the_check_reads_them() {
    let daemon = SpawnedDaemon::spawn();
    assert_eq!(
        wait_for_daemon(&daemon.socket_path, DAEMON_CONNECT_TIMEOUT).await,
        DaemonStatus::Healthy,
        "the daemon never became healthy"
    );

    let reported = running_daemon_extensions(&daemon.socket_path)
        .await
        .expect("a healthy daemon must answer GetDaemonVersion");

    assert!(
        reported.is_empty(),
        "core's daemon supports no extension, got {reported:?}"
    );
    assert_eq!(
        product_check(Some(&reported), &DaemonProfile::community()),
        ProductCheck::Match
    );
    let expecting_an_extension = DaemonProfile {
        extensions: vec!["fixture".to_owned()],
        ..DaemonProfile::community()
    };
    assert_eq!(
        product_check(Some(&reported), &expecting_an_extension),
        ProductCheck::Mismatch
    );
}

#[cfg(unix)]
mod unhappy_paths {
    use nodespace_app_lib::daemon_profile::DaemonProfile;
    use nodespace_app_lib::daemon_setup::{product_check, running_daemon_extensions, ProductCheck};
    use std::time::Duration;

    #[tokio::test]
    async fn a_daemon_that_cannot_be_reached_is_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");

        let reported = running_daemon_extensions(&dir.path().join("absent.sock")).await;

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
            tokio::time::timeout(Duration::from_secs(20), running_daemon_extensions(&socket))
                .await
                .expect("running_daemon_extensions must give up on a daemon that never answers");

        assert_eq!(reported, None);
    }
}
