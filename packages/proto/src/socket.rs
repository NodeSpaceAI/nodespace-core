//! The daemon endpoint every NodeSpace process agrees on.
//!
//! Which socket the daemon binds and which socket a client dials is part of the
//! transport contract, not an implementation detail of either side — so the
//! variant table lives here, next to the header keys and message limits, rather
//! than being copied into each crate that needs it.
//!
//! The socket filename is scoped by build variant so that a dev build, a
//! release build, a community build and a Pro build can all run on one machine
//! without fighting over the same endpoint:
//!
//! | debug | pro | socket                           |
//! |-------|-----|----------------------------------|
//! | no    | no  | `.nodespace/daemon.sock`         |
//! | no    | yes | `.nodespace/daemon-pro.sock`     |
//! | yes   | no  | `.nodespace/daemon-dev.sock`     |
//! | yes   | yes | `.nodespace/daemon-dev-pro.sock` |
//!
//! Both discriminators are properties of the *calling binary*, which is why
//! [`daemon_socket_relative`] takes them as parameters instead of reading
//! `cfg!()` here: `nodespace-proto` is compiled once for the whole workspace,
//! but "am I Pro?" is answered differently by each consumer — the desktop app
//! reads a baked-in `option_env!` cloud URL, the daemon reads its `pro` cargo
//! feature — and a `cfg!()` evaluated inside this crate would silently answer
//! for the wrong binary.
//!
//! The `NODESPACED_SOCKET` environment variable overrides the default on both
//! sides. It is an override only: every side must be able to derive the same
//! endpoint without it, or losing the variable (a `launchctl kickstart -k` that
//! reuses a stale job definition, say) leaves a healthy daemon serving a socket
//! nobody dials.

/// Environment variable that overrides the daemon endpoint on every side —
/// the daemon's bind address, the desktop app's dial target, the CLI's default.
pub const SOCKET_ENV_VAR: &str = "NODESPACED_SOCKET";

/// The `.nodespace/` state directory's name, relative to the user's home.
pub const STATE_DIR: &str = ".nodespace";

/// Every daemon socket filename, ordered canonical-first: release community,
/// release Pro, dev community, dev Pro.
///
/// Exposed for callers that must consider all variants at once rather than
/// their own — the CLI probes these in order to find whichever daemon is
/// actually running, since it is a single binary that may be driving any of them.
pub const DAEMON_SOCKET_NAMES: [&str; 4] = [
    "daemon.sock",
    "daemon-pro.sock",
    "daemon-dev.sock",
    "daemon-dev-pro.sock",
];

/// The daemon socket filename for one build variant.
///
/// `is_debug` should come from the caller's own `cfg!(debug_assertions)`, and
/// `is_pro` from whatever makes the caller a Pro build — see the module docs
/// for why neither is read here.
pub const fn daemon_socket_name(is_debug: bool, is_pro: bool) -> &'static str {
    match (is_debug, is_pro) {
        (false, false) => DAEMON_SOCKET_NAMES[0],
        (false, true) => DAEMON_SOCKET_NAMES[1],
        (true, false) => DAEMON_SOCKET_NAMES[2],
        (true, true) => DAEMON_SOCKET_NAMES[3],
    }
}

/// The daemon socket path for one build variant, relative to the user's home
/// directory (e.g. `.nodespace/daemon-dev.sock`).
///
/// This is the form the launchd plist writes into `NODESPACED_SOCKET` and the
/// form both the app and the daemon fall back to when that variable is absent,
/// so the two agree by construction.
pub const fn daemon_socket_relative(is_debug: bool, is_pro: bool) -> &'static str {
    match (is_debug, is_pro) {
        (false, false) => ".nodespace/daemon.sock",
        (false, true) => ".nodespace/daemon-pro.sock",
        (true, false) => ".nodespace/daemon-dev.sock",
        (true, true) => ".nodespace/daemon-dev-pro.sock",
    }
}

/// The Windows Named Pipe the daemon serves. Windows has no per-variant
/// scoping: the pipe namespace is machine-global rather than per-home, and the
/// desktop app spawns the daemon directly there instead of registering a
/// long-lived service, so there is no plist-equivalent to drift out of sync.
pub const DAEMON_PIPE_NAME: &str = r"\\.\pipe\nodespace-daemon";

/// Every UI pid-file filename, in the same build-variant order as
/// [`DAEMON_SOCKET_NAMES`].
///
/// The desktop app writes its own pid here at startup (Unix only — Windows
/// signals the UI process by image name instead, see the daemon crate's
/// `tray::signal_ui_to_quit`); the daemon's tray "Quit" reads it to find and
/// close the running window, the mirror image of how the desktop app finds
/// and stops the daemon via [`daemon_socket_relative`]. Scoped by build
/// variant for the same reason that table is: a dev build's file must never
/// collide with a release build's, or a release Quit could reach into a
/// stray dev UI process's window (or vice versa).
pub const UI_PID_NAMES: [&str; 4] = ["ui.pid", "ui-pro.pid", "ui-dev.pid", "ui-dev-pro.pid"];

/// The UI pid-file filename for one build variant. See [`daemon_socket_name`]
/// for the parameter contract — identical here.
pub const fn ui_pid_name(is_debug: bool, is_pro: bool) -> &'static str {
    match (is_debug, is_pro) {
        (false, false) => UI_PID_NAMES[0],
        (false, true) => UI_PID_NAMES[1],
        (true, false) => UI_PID_NAMES[2],
        (true, true) => UI_PID_NAMES[3],
    }
}

/// The UI pid-file path for one build variant, relative to the user's home
/// directory (e.g. `.nodespace/ui-dev.pid`). Mirrors
/// [`daemon_socket_relative`]'s contract exactly: both the desktop app and
/// the daemon derive this path from their own build-variant flags rather
/// than one side copying the other's answer.
pub const fn ui_pid_relative(is_debug: bool, is_pro: bool) -> &'static str {
    match (is_debug, is_pro) {
        (false, false) => ".nodespace/ui.pid",
        (false, true) => ".nodespace/ui-pro.pid",
        (true, false) => ".nodespace/ui-dev.pid",
        (true, true) => ".nodespace/ui-dev-pro.pid",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_is_state_dir_joined_to_name() {
        for (is_debug, is_pro) in [(false, false), (false, true), (true, false), (true, true)] {
            assert_eq!(
                daemon_socket_relative(is_debug, is_pro),
                format!("{}/{}", STATE_DIR, daemon_socket_name(is_debug, is_pro)),
                "variant (debug={is_debug}, pro={is_pro})"
            );
        }
    }

    #[test]
    fn every_variant_gets_a_distinct_socket() {
        let mut names: Vec<&str> = [(false, false), (false, true), (true, false), (true, true)]
            .iter()
            .map(|&(d, p)| daemon_socket_name(d, p))
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 4, "build variants must not share a socket");
    }

    #[test]
    fn canonical_name_is_the_release_community_socket() {
        // The CLI dials DAEMON_SOCKET_NAMES[0] when no daemon is running, and
        // reports "is the daemon running?" against it — that must be the socket
        // a shipped community install actually uses.
        assert_eq!(DAEMON_SOCKET_NAMES[0], daemon_socket_name(false, false));
    }

    #[test]
    fn ui_pid_relative_path_is_state_dir_joined_to_name() {
        for (is_debug, is_pro) in [(false, false), (false, true), (true, false), (true, true)] {
            assert_eq!(
                ui_pid_relative(is_debug, is_pro),
                format!("{}/{}", STATE_DIR, ui_pid_name(is_debug, is_pro)),
                "variant (debug={is_debug}, pro={is_pro})"
            );
        }
    }

    #[test]
    fn every_variant_gets_a_distinct_ui_pid_file() {
        let mut names: Vec<&str> = [(false, false), (false, true), (true, false), (true, true)]
            .iter()
            .map(|&(d, p)| ui_pid_name(d, p))
            .collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            4,
            "build variants must not share a UI pid file"
        );
    }

    #[test]
    fn ui_pid_name_never_collides_with_a_daemon_socket_name() {
        // The two files live in the same `.nodespace/` directory, so a
        // collision here would mean the desktop app's pid file and the
        // daemon's own socket could shadow each other on disk.
        for (is_debug, is_pro) in [(false, false), (false, true), (true, false), (true, true)] {
            assert_ne!(
                ui_pid_name(is_debug, is_pro),
                daemon_socket_name(is_debug, is_pro)
            );
        }
    }
}
