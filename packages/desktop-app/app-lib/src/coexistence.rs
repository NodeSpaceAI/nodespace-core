//! Other NodeSpace daemons and service registrations this app cannot replace,
//! and the notices that tell the user about them (ADR-084 §4.3).
//!
//! Every NodeSpace product registers its daemon under the same service label
//! and serves the same socket. At startup the app boots out a registration that
//! runs another binary and registers its own daemon
//! ([`crate::daemon_setup::ensure_daemon_running`]). Two situations survive
//! that, and the app can only explain them:
//!
//! - **Another daemon outside the shared label** (a Homebrew service, or one
//!   started by hand) keeps the socket, and the daemon this app registered
//!   exits on the single-instance lock. The startup records the extension ids
//!   that daemon reported here, and the app reports [`OTHER_DAEMON_STATUS`] until a
//!   retry ([`retry_daemon_start`]) finds the socket free of it.
//! - **A foreign machine-wide registration** (macOS): the plist under
//!   `/Library/LaunchAgents` with this app's label runs another binary, so it
//!   starts that daemon for every user at login, and the app cannot remove a
//!   root-owned file.
//!
//! Both are judged against this app's own daemon profile only, so nothing here
//! knows which other product is installed.

use std::sync::{Mutex, PoisonError};

use tauri::AppHandle;

/// Status string reported while another daemon holds the socket.
pub const OTHER_DAEMON_STATUS: &str = "other_daemon";

/// The extension ids the other daemon holding the socket reported,
/// comma-separated; empty when it reported none.
static OTHER_DAEMON: Mutex<Option<String>> = Mutex::new(None);

/// Records the other daemon holding the socket, or clears the record (`None`).
pub(crate) fn record_other_daemon(extensions: Option<String>) {
    *OTHER_DAEMON.lock().unwrap_or_else(PoisonError::into_inner) = extensions;
}

/// The extension ids of the other daemon the last start found holding the
/// socket. Empty when that daemon reported none.
pub fn other_daemon() -> Option<String> {
    OTHER_DAEMON
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The daemon status to report to the frontend: [`OTHER_DAEMON_STATUS`] while
/// another daemon is on record, and what `probe` finds on the socket otherwise.
///
/// Whatever answers on the socket while the record stands is not this app's
/// daemon, until a retry of the start finds otherwise, so the socket is not
/// probed then.
pub(crate) async fn status_unless_other_daemon(
    probe: impl std::future::Future<Output = String>,
) -> String {
    if other_daemon().is_some() {
        return OTHER_DAEMON_STATUS.to_string();
    }
    probe.await
}

/// The other daemon's extension ids, for the notice. `None` when there is none
/// on record.
#[tauri::command]
pub async fn get_other_daemon() -> Option<String> {
    other_daemon()
}

/// Retry, from this notice once the user has stopped the other daemon, and
/// from the not-running banner: starts this app's daemon again, then emits
/// and returns the resulting status, the same strings `check_daemon_status`
/// reports. A start that finds this app's daemon answering also ends the
/// startup hold on the app's channel, even one past its limit.
#[tauri::command]
pub async fn retry_daemon_start(app: AppHandle) -> String {
    crate::daemon_setup::start_daemon_and_report(&app).await
}

/// Whether the machine-wide registration under this app's service label runs
/// another binary than this app's daemon, for the notice. Always false off
/// macOS, which has no machine-wide registration.
#[tauri::command]
pub async fn foreign_machine_wide_registration() -> bool {
    #[cfg(target_os = "macos")]
    {
        let plist = std::path::Path::new(MACHINE_WIDE_LAUNCH_AGENTS)
            .join(crate::daemon_setup::plist_filename());
        match foreign_program(&plist, crate::daemon_profile::active()) {
            Some(program) => {
                tracing::warn!(
                    plist = %plist.display(),
                    program,
                    "the machine-wide service registration runs another daemon"
                );
                true
            }
            None => false,
        }
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Where launchd keeps registrations that start for every user at login.
#[cfg(target_os = "macos")]
const MACHINE_WIDE_LAUNCH_AGENTS: &str = "/Library/LaunchAgents";

/// The program the launchd job at `plist` runs, when it is not `profile`'s
/// daemon binary (compared by file name,
/// [`crate::daemon_setup::runs_profile_binary`]). launchd runs `Program` when it is set and the first of
/// `ProgramArguments` otherwise.
///
/// `None` when there is no file, it cannot be read as a property list, it
/// names no program, or the program is this app's daemon: nothing is known to
/// be wrong then.
#[cfg(target_os = "macos")]
fn foreign_program(
    plist: &std::path::Path,
    profile: &crate::daemon_profile::DaemonProfile,
) -> Option<String> {
    use crate::daemon_setup::runs_profile_binary;

    if !plist.exists() {
        return None;
    }
    let job = match plist::Value::from_file(plist) {
        Ok(job) => job,
        Err(error) => {
            tracing::warn!(plist = %plist.display(), %error, "could not read a service registration");
            return None;
        }
    };
    let job = job.as_dictionary()?;
    let program = match job.get("Program") {
        Some(program) => program.as_string(),
        None => job
            .get("ProgramArguments")
            .and_then(plist::Value::as_array)
            .and_then(|arguments| arguments.first())
            .and_then(plist::Value::as_string),
    }
    .filter(|program| !program.is_empty())?;
    (!runs_profile_binary(program, profile)).then(|| program.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record is process-wide; this is the only test that touches it.
    #[tokio::test]
    async fn the_other_daemon_record_holds_until_a_start_has_its_answer() {
        use crate::daemon_setup::{record_outcome, DaemonStatus};

        record_other_daemon(Some("/opt/somewhere/other-daemon".to_owned()));
        assert_eq!(
            crate::incompatible_database::daemon_down_status(),
            OTHER_DAEMON_STATUS,
            "while another daemon holds the socket, that is why this app's daemon is down"
        );
        // A status check then reports the other daemon without probing the
        // socket, where that daemon would answer as healthy.
        let mut probed = false;
        let status = status_unless_other_daemon(async {
            probed = true;
            "healthy".to_owned()
        })
        .await;
        assert_eq!(status, OTHER_DAEMON_STATUS);
        assert!(!probed, "the socket must not be probed");

        // A start that finds this app's own daemon clears the record, but only
        // once it is over: a status check during it still sees the last answer.
        let status = record_outcome(async {
            assert_eq!(
                other_daemon().as_deref(),
                Some("/opt/somewhere/other-daemon"),
                "the record must hold while the start runs"
            );
            Ok((DaemonStatus::Healthy, None))
        })
        .await
        .expect("start");
        assert_eq!(status, DaemonStatus::Healthy);
        assert_eq!(other_daemon(), None);
        assert_ne!(
            crate::incompatible_database::daemon_down_status(),
            OTHER_DAEMON_STATUS
        );
        assert_eq!(
            status_unless_other_daemon(async { "healthy".to_owned() }).await,
            "healthy",
            "with no other daemon on record the socket's answer is the status"
        );

        // A start that finds another daemon records it.
        let status = record_outcome(async {
            Ok((
                DaemonStatus::NotRunning,
                Some("/usr/local/bin/another-daemon".to_owned()),
            ))
        })
        .await
        .expect("start");
        assert_eq!(status, DaemonStatus::NotRunning);
        assert_eq!(
            other_daemon().as_deref(),
            Some("/usr/local/bin/another-daemon")
        );

        // A start that fails knows nothing about the socket, so the record goes.
        assert!(
            record_outcome(async { Err(anyhow::anyhow!("bootstrap failed")) })
                .await
                .is_err()
        );
        assert_eq!(other_daemon(), None);
    }

    #[cfg(target_os = "macos")]
    mod machine_wide_registration {
        use super::super::foreign_program;
        use crate::daemon_profile::DaemonProfile;
        use std::path::{Path, PathBuf};

        fn job(entries: Vec<(&str, plist::Value)>) -> plist::Value {
            let mut job = plist::Dictionary::new();
            job.insert("Label".into(), "app.nodespace.daemon".into());
            for (key, value) in entries {
                job.insert(key.into(), value);
            }
            plist::Value::Dictionary(job)
        }

        fn arguments(program: &str) -> plist::Value {
            plist::Value::Array(vec![program.into(), "--tray".into()])
        }

        fn write_xml(dir: &Path, job: plist::Value) -> PathBuf {
            let path = dir.join("app.nodespace.daemon.plist");
            job.to_file_xml(&path).expect("write plist");
            path
        }

        fn foreign(job: plist::Value) -> Option<String> {
            let dir = tempfile::tempdir().expect("tempdir");
            foreign_program(&write_xml(dir.path(), job), &DaemonProfile::community())
        }

        #[test]
        fn a_registration_running_this_apps_daemon_is_not_foreign() {
            assert_eq!(
                foreign(job(vec![(
                    "ProgramArguments",
                    arguments("/usr/local/bin/nodespaced")
                )])),
                None
            );
        }

        #[test]
        fn a_registration_running_another_binary_is_foreign() {
            assert_eq!(
                foreign(job(vec![(
                    "ProgramArguments",
                    arguments("/usr/local/bin/other-daemon")
                )])),
                Some("/usr/local/bin/other-daemon".to_owned())
            );
        }

        /// launchd runs `Program` over `ProgramArguments`, so the check reads it
        /// first, in both directions.
        #[test]
        fn program_wins_over_program_arguments() {
            assert_eq!(
                foreign(job(vec![
                    ("Program", "/usr/local/bin/other-daemon".into()),
                    ("ProgramArguments", arguments("/usr/local/bin/nodespaced")),
                ])),
                Some("/usr/local/bin/other-daemon".to_owned())
            );
            assert_eq!(
                foreign(job(vec![
                    ("Program", "/usr/local/bin/nodespaced".into()),
                    ("ProgramArguments", arguments("/usr/local/bin/other-daemon")),
                ])),
                None
            );
        }

        #[test]
        fn a_binary_plist_is_read_too() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("app.nodespace.daemon.plist");
            job(vec![(
                "ProgramArguments",
                arguments("/usr/local/bin/other-daemon"),
            )])
            .to_file_binary(&path)
            .expect("write binary plist");
            assert_eq!(
                foreign_program(&path, &DaemonProfile::community()),
                Some("/usr/local/bin/other-daemon".to_owned())
            );
        }

        /// Nothing is known to be wrong without a file, a readable one, or a
        /// program in it, so none of those raise the notice.
        #[test]
        fn no_registration_or_no_program_is_not_foreign() {
            let dir = tempfile::tempdir().expect("tempdir");
            let missing = dir.path().join("app.nodespace.daemon.plist");
            assert_eq!(foreign_program(&missing, &DaemonProfile::community()), None);

            std::fs::write(&missing, "not a property list").expect("write");
            assert_eq!(foreign_program(&missing, &DaemonProfile::community()), None);

            assert_eq!(foreign(job(vec![])), None);
            assert_eq!(foreign(job(vec![("Program", "".into())])), None);
        }

        /// The comparison is with whichever profile the app runs, not a fixed
        /// name.
        #[test]
        fn the_active_profile_decides_what_is_foreign() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = write_xml(
                dir.path(),
                job(vec![(
                    "ProgramArguments",
                    arguments("/usr/local/bin/nodespaced"),
                )]),
            );
            let other_profile = DaemonProfile {
                binary_name: "other-daemon",
                ..DaemonProfile::community()
            };
            assert_eq!(
                foreign_program(&path, &other_profile),
                Some("/usr/local/bin/nodespaced".to_owned())
            );
        }
    }
}
