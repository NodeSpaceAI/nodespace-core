//! Which daemon this app installs, registers and starts, and what it expects of it.
//!
//! Which daemon binary to run, which image name to kill on Windows, and what
//! extra environment the service registration carries all go through the
//! process's active [`DaemonProfile`]. `run` installs it before it builds the
//! app: the profile the app's extension names
//! (`AppExtensions::daemon_profile`), or the build's default, which is chosen in
//! one place, `daemon_setup`'s `profile_for_this_build`.
//!
//! The launchd label, socket, UI pid file and incompatible-database marker are
//! not part of a profile: they are core's shared service identity, separated
//! only by build flavour, and are selected in `daemon_setup` and its neighbours.

use std::sync::OnceLock;

use nodespace_proto::socket::SOCKET_ENV_VAR;

use crate::daemon_setup::DAEMON_BINARY_NAME;

/// The variable naming the GUI binary the daemon's tray relaunches.
pub(crate) const UI_BINARY_ENV_VAR: &str = "NODESPACE_UI_BINARY";

/// The variables core's launcher itself writes into the daemon's launchd
/// registration, ahead of a profile's `service_env`. A profile may not repeat
/// one: launchd accepts the repeated key, and the profile's value, which comes
/// later, would silently replace core's.
pub(crate) const CORE_SERVICE_ENV: [&str; 2] = [SOCKET_ENV_VAR, UI_BINARY_ENV_VAR];

/// Which daemon this app installs, registers and starts, and what it expects of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonProfile {
    /// Sidecar the launcher installs, registers and starts; no `.exe`. Core and
    /// every app built on it use `nodespaced`, so the name does not tell the
    /// products apart: [`Self::extensions`] does.
    pub binary_name: &'static str,
    /// The extension ids the running daemon must report supporting, which the
    /// startup product check compares as a set. Empty for core's daemon.
    pub extensions: Vec<String>,
    /// Extra environment in the service registration, after core's own
    /// `NODESPACED_SOCKET` and `NODESPACE_UI_BINARY`. Rendered into the macOS
    /// launchd plist only, as today; the systemd unit and the Windows spawn
    /// do not carry it. A key must not repeat one of core's own.
    pub service_env: Vec<(String, String)>,
}

impl DaemonProfile {
    /// The daemon a build of core alone installs.
    pub fn community() -> Self {
        Self {
            binary_name: DAEMON_BINARY_NAME,
            extensions: Vec::new(),
            service_env: Vec::new(),
        }
    }

    /// The first `service_env` key that is one of [`CORE_SERVICE_ENV`], if any.
    /// Keys match exactly, as launchd compares them.
    pub(crate) fn reserved_service_env_key(&self) -> Option<&str> {
        self.service_env
            .iter()
            .map(|(key, _)| key.as_str())
            .find(|key| CORE_SERVICE_ENV.contains(key))
    }

    /// The Windows `taskkill /IM` image name of this profile's daemon.
    ///
    /// Pure method, gated on `any(windows, test)` rather than `windows` alone
    /// (mirroring `daemon_log_paths`/`open_daemon_log`), so the bug class it
    /// guards against — an image name that can silently drift from
    /// `binary_name` — is directly testable on any platform, not just
    /// compile-checked against the Windows target.
    ///
    /// The `.exe` suffix assumes the installed daemon binary carries that
    /// extension on Windows. Confirmed for real on a Windows box: this
    /// assumption did NOT hold before `extract_sidecar_if_changed`'s `dest` was
    /// fixed to route through `bundled_sidecar_name` — with the old bare
    /// (`.exe`-less) install name, a real `taskkill /F /IM nodespaced.exe`
    /// against a real running bare-named `nodespaced` process failed outright
    /// (`ERROR: The process "nodespaced.exe" not found.`) while the process kept
    /// running. With that fix in place, `daemon_bin`'s install name and this
    /// image name agree by construction.
    #[cfg(any(windows, test))]
    pub(crate) fn image_name(&self) -> String {
        format!("{}.exe", self.binary_name)
    }
}

static ACTIVE: OnceLock<DaemonProfile> = OnceLock::new();

/// The profile every daemon-identity read in this process goes through.
///
/// `run` sets it with [`install`] before it builds the app, so every read in
/// the app sees the installed profile. A read before any install, which only a
/// test that never calls `run` makes, gets the build's default, and that then
/// stays fixed for the life of the process.
pub(crate) fn active() -> &'static DaemonProfile {
    ACTIVE.get_or_init(crate::daemon_setup::profile_for_this_build)
}

/// Fixes the profile this process runs with: `supplied`, or the build's
/// default when nothing is supplied. `run` calls it once, before it builds the
/// app.
///
/// # Panics
///
/// If the profile is already set. Something read it, or installed one, before
/// `run` did, and switching now would leave the earlier reads acting on another
/// daemon. That is an ordering bug, never a state to recover from.
pub(crate) fn install(supplied: Option<DaemonProfile>) {
    if let Err(rejected) = install_into(&ACTIVE, supplied) {
        panic!(
            "the daemon profile was already set to `{}` when `run` came to install `{}`: \
             something read the daemon profile before `run` installed it, and every read \
             must come after the install",
            active().binary_name,
            rejected.binary_name
        );
    }
}

/// Sets `cell` to `supplied`, or to the build's default when nothing is
/// supplied. Hands the profile back, leaving `cell` as it was, when `cell` is
/// already set.
fn install_into(
    cell: &OnceLock<DaemonProfile>,
    supplied: Option<DaemonProfile>,
) -> Result<(), DaemonProfile> {
    cell.set(supplied.unwrap_or_else(crate::daemon_setup::profile_for_this_build))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_setup::profile_for_this_build;

    fn custom() -> DaemonProfile {
        DaemonProfile {
            binary_name: "custom-daemon",
            extensions: vec!["custom".to_string()],
            service_env: vec![("CUSTOM_MODE".to_string(), "on".to_string())],
        }
    }

    /// The whole struct, not field by field: a field added to the profile
    /// fails to compile here until this test states core's value for it.
    #[test]
    fn community_profile_is_the_core_daemon_with_no_extensions_or_service_env() {
        assert_eq!(
            DaemonProfile::community(),
            DaemonProfile {
                binary_name: "nodespaced",
                extensions: Vec::new(),
                service_env: Vec::new(),
            }
        );
    }

    #[test]
    fn image_name_is_the_binary_name_with_the_windows_extension() {
        assert_eq!(DaemonProfile::community().image_name(), "nodespaced.exe");

        let custom = DaemonProfile {
            binary_name: "custom-daemon",
            ..DaemonProfile::community()
        };
        assert_eq!(custom.image_name(), "custom-daemon.exe");
    }
    #[test]
    fn core_service_env_names_the_socket_and_the_ui_binary() {
        assert_eq!(
            CORE_SERVICE_ENV,
            ["NODESPACED_SOCKET", "NODESPACE_UI_BINARY"]
        );
    }

    #[test]
    fn a_reserved_service_env_key_is_found_wherever_it_sits() {
        assert_eq!(custom().reserved_service_env_key(), None);
        assert_eq!(DaemonProfile::community().reserved_service_env_key(), None);

        let mut profile = custom();
        profile
            .service_env
            .push(("NODESPACE_UI_BINARY".to_string(), "/elsewhere".to_string()));
        assert_eq!(
            profile.reserved_service_env_key(),
            Some("NODESPACE_UI_BINARY")
        );
    }

    /// A key is reserved only when it matches exactly, as launchd compares
    /// keys: a profile may add a variable that merely shares a prefix.
    #[test]
    fn a_key_that_only_resembles_a_reserved_one_is_allowed() {
        let mut profile = custom();
        profile
            .service_env
            .push(("NODESPACED_SOCKET_TIMEOUT".to_string(), "5".to_string()));
        profile
            .service_env
            .push(("nodespaced_socket".to_string(), "x".to_string()));
        assert_eq!(profile.reserved_service_env_key(), None);
    }

    #[test]
    fn install_into_stores_a_supplied_profile() {
        let cell = OnceLock::new();
        assert_eq!(install_into(&cell, Some(custom())), Ok(()));
        assert_eq!(cell.get(), Some(&custom()));
    }

    /// `AppExtensions::none()` supplies no profile, so this is the profile a
    /// core app runs with.
    #[test]
    fn install_into_falls_back_to_the_build_default_when_nothing_is_supplied() {
        let cell = OnceLock::new();
        assert_eq!(install_into(&cell, None), Ok(()));
        assert_eq!(cell.get(), Some(&profile_for_this_build()));
    }

    #[test]
    fn install_into_refuses_a_cell_that_is_already_set() {
        let cell = OnceLock::new();
        cell.set(DaemonProfile::community())
            .expect("the cell starts empty");

        assert_eq!(install_into(&cell, Some(custom())), Err(custom()));
        assert_eq!(
            cell.get(),
            Some(&DaemonProfile::community()),
            "a refused install leaves the profile that was there"
        );
    }

    /// Set in the child process [`in_own_process`] starts.
    const OWN_PROCESS_ENV: &str = "NODESPACE_DAEMON_PROFILE_TEST_OWN_PROCESS";

    /// Runs this module's test `name` again in a child process of this test
    /// binary, as the only test there, and returns whether it passed and what
    /// it printed.
    ///
    /// The crate's unit tests share one process under `cargo test`, and many of
    /// them read the process-wide profile, so a test that installs one must not
    /// run among them.
    fn in_own_process(name: &str) -> (bool, String) {
        let output = std::process::Command::new(
            std::env::current_exe().expect("the test binary's own path"),
        )
        .args([
            format!("daemon_profile::tests::{name}").as_str(),
            "--exact",
            "--test-threads=1",
        ])
        .env(OWN_PROCESS_ENV, "1")
        .output()
        .expect("the test binary runs again");
        let printed = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            printed.contains("running 1 test"),
            "the child process must run exactly `{name}`: {printed}"
        );
        (output.status.success(), printed)
    }

    /// The install `run` makes is what every later read sees, the Windows
    /// image name included.
    #[test]
    fn an_installed_profile_is_what_every_read_sees() {
        if std::env::var_os(OWN_PROCESS_ENV).is_none() {
            let (passed, printed) = in_own_process("an_installed_profile_is_what_every_read_sees");
            assert!(passed, "{printed}");
            return;
        }

        install(Some(custom()));
        assert_eq!(active(), &custom());
        assert_eq!(active().image_name(), "custom-daemon.exe");
    }

    /// An install after the profile was read fails loudly, naming the ordering
    /// bug, rather than being ignored.
    #[test]
    fn installing_after_a_read_panics() {
        if std::env::var_os(OWN_PROCESS_ENV).is_none() {
            let (passed, printed) = in_own_process("installing_after_a_read_panics");
            assert!(!passed, "a late install must panic: {printed}");
            assert!(
                printed.contains("something read the daemon profile before `run` installed it"),
                "the panic must name the ordering bug: {printed}"
            );
            return;
        }

        assert_eq!(active(), &profile_for_this_build());
        install(Some(custom()));
    }
}
