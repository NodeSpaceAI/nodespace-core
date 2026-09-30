//! Which daemon this app installs, registers and starts, and what it expects of it.
//!
//! Every choice that depends on the daemon's identity goes through the process's
//! active [`DaemonProfile`]: the sidecar to install, the image name to kill on
//! Windows, and the extra environment the service registration carries. The
//! build's default profile is chosen in one place, `daemon_setup`'s
//! `profile_for_this_build`, so no other code branches on the build's edition to
//! pick a binary or an environment.
//!
//! The launchd label, socket, UI pid file and incompatible-database marker are
//! not part of a profile: they follow the product's identity and are selected in
//! `daemon_setup` and its neighbours.

use std::sync::OnceLock;

use crate::daemon_setup::DAEMON_BINARY_NAME;

/// Which daemon this app installs, registers and starts, and what it expects of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonProfile {
    /// Sidecar the launcher installs, registers and starts; no `.exe`.
    pub binary_name: &'static str,
    /// Extra environment in the service registration, after core's own
    /// `NODESPACED_SOCKET` and `NODESPACE_UI_BINARY`. Rendered into the macOS
    /// launchd plist only, as today; the systemd unit and the Windows spawn
    /// do not carry it. A key must not repeat one of core's own.
    pub service_env: Vec<(String, String)>,
    /// The product the app expects the running daemon to belong to.
    pub product: &'static str,
}

impl DaemonProfile {
    /// The daemon a build of core alone installs.
    pub fn community() -> Self {
        Self {
            binary_name: DAEMON_BINARY_NAME,
            service_env: Vec::new(),
            product: "community",
        }
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
/// Initialised on first read with the build's default profile and kept for the
/// life of the process.
pub(crate) fn active() -> &'static DaemonProfile {
    ACTIVE.get_or_init(crate::daemon_setup::profile_for_this_build)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn community_profile_carries_the_community_daemon_and_no_service_env() {
        let profile = DaemonProfile::community();
        assert_eq!(profile.binary_name, "nodespaced");
        assert!(profile.service_env.is_empty());
        assert_eq!(profile.product, "community");
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
}
