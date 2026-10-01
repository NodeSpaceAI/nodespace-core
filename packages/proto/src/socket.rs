//! The daemon endpoint every NodeSpace process agrees on.
//!
//! Which socket the daemon binds and which socket a client dials is part of the
//! transport contract, not an implementation detail of either side — so the
//! variant table lives here, next to the header keys and message limits, rather
//! than being copied into each crate that needs it.
//!
//! The socket filename is scoped by build flavour so that a dev build and a
//! release build can run on one machine without fighting over the same
//! endpoint:
//!
//! | debug | socket                     |
//! |-------|----------------------------|
//! | no    | `.nodespace/daemon.sock`   |
//! | yes   | `.nodespace/daemon-dev.sock` |
//!
//! The flavour is the only discriminator. Every daemon registered under core's
//! service identity uses these names, whichever app registered it.
//!
//! The flavour is a property of the *calling binary*, which is why
//! [`daemon_socket_relative`] takes it as a parameter instead of reading
//! `cfg!(debug_assertions)` here: `nodespace-proto` is compiled once for the
//! whole workspace, and a per-package profile override can build it with
//! different settings from the binary that links it, so a `cfg!()` evaluated
//! inside this crate could silently answer for the wrong binary.
//!
//! The `NODESPACED_SOCKET` environment variable overrides the default on both
//! sides. It is an override only: every side must be able to derive the same
//! endpoint without it, or losing the variable (a `launchctl kickstart -k` that
//! reuses a stale job definition, say) leaves a healthy daemon serving a socket
//! nobody dials.
//!
//! A daemon also takes an exclusive advisory lock on a lock file beside its
//! socket, named `<socket stem>.lock` (`daemon.sock` -> `daemon.lock`,
//! `daemon-dev.sock` -> `daemon-dev.lock`), so at most one daemon serves a
//! socket. The lock follows the socket rather than sitting at a fixed path:
//! an isolated daemon that sets its own `NODESPACED_SOCKET` never contends
//! with the user's daemon. The library function that takes it is
//! `nodespace_daemon::single_instance::acquire`.

/// Environment variable that overrides the daemon endpoint on every side —
/// the daemon's bind address, the desktop app's dial target, the CLI's default.
pub const SOCKET_ENV_VAR: &str = "NODESPACED_SOCKET";

/// The `.nodespace/` state directory's name, relative to the user's home.
pub const STATE_DIR: &str = ".nodespace";

/// Every daemon socket filename, ordered canonical-first: release, then dev.
///
/// Exposed for callers that must consider both flavours at once rather than
/// their own — the CLI probes these in order to find whichever daemon is
/// actually running, since it is a single binary that may be driving either.
pub const DAEMON_SOCKET_NAMES: [&str; 2] = ["daemon.sock", "daemon-dev.sock"];

/// The daemon socket filename for one build flavour.
///
/// `is_debug` should come from the caller's own `cfg!(debug_assertions)` — see
/// the module docs for why it is not read here.
pub const fn daemon_socket_name(is_debug: bool) -> &'static str {
    if is_debug {
        DAEMON_SOCKET_NAMES[1]
    } else {
        DAEMON_SOCKET_NAMES[0]
    }
}

/// The daemon socket path for one build flavour, relative to the user's home
/// directory (e.g. `.nodespace/daemon-dev.sock`).
///
/// This is the form the launchd plist writes into `NODESPACED_SOCKET` and the
/// form both the app and the daemon fall back to when that variable is absent,
/// so the two agree by construction.
pub const fn daemon_socket_relative(is_debug: bool) -> &'static str {
    if is_debug {
        ".nodespace/daemon-dev.sock"
    } else {
        ".nodespace/daemon.sock"
    }
}

/// The Windows Named Pipe the daemon serves. Windows has no per-variant
/// scoping: the pipe namespace is machine-global rather than per-home, and the
/// desktop app spawns the daemon directly there instead of registering a
/// long-lived service, so there is no plist-equivalent to drift out of sync.
pub const DAEMON_PIPE_NAME: &str = r"\\.\pipe\nodespace-daemon";

/// The command-line flag that opts the daemon into tray mode.
///
/// The daemon is headless unless told otherwise, so the desktop app's daemon,
/// the one deployment that wants a tray icon, passes this through
/// [`LAUNCHER_ARGS`]. It is a flag rather than an environment variable because
/// the Windows autorun entry is a bare command line with no way to carry one.
pub const TRAY_FLAG: &str = "--tray";

/// The arguments the desktop app's launchers pass to the daemon they register:
/// the launchd plist, the systemd unit, the Windows spawn and the Windows
/// autorun entry. The static plist the `.pkg` installs repeats them by hand.
/// A headless registration, such as the Homebrew formula's service, passes
/// none.
///
/// A daemon registered under core's service identity must accept each entry.
/// Adding an argument is an extension-API change, because a daemon built
/// elsewhere on this crate has to accept it too (ADR-082 section 6).
///
/// Each entry is one plain word: the systemd `ExecStart` line and the Windows
/// autorun value join the entries with spaces and quote nothing.
pub const LAUNCHER_ARGS: [&str; 1] = [TRAY_FLAG];

/// Every UI pid-file filename, in the same build-flavour order as
/// [`DAEMON_SOCKET_NAMES`].
///
/// The desktop app writes its own pid here at startup (Unix only — Windows
/// signals the UI process by image name instead, see the daemon crate's
/// `tray::signal_ui_to_quit`); the daemon's tray "Quit" reads it to find and
/// close the running window, the mirror image of how the desktop app finds
/// and stops the daemon via [`daemon_socket_relative`]. Scoped by build
/// flavour for the same reason that table is: a dev build's file must never
/// collide with a release build's, or a release Quit could reach into a
/// stray dev UI process's window (or vice versa).
pub const UI_PID_NAMES: [&str; 2] = ["ui.pid", "ui-dev.pid"];

/// The UI pid-file filename for one build flavour. See [`daemon_socket_name`]
/// for the parameter contract — identical here.
pub const fn ui_pid_name(is_debug: bool) -> &'static str {
    if is_debug {
        UI_PID_NAMES[1]
    } else {
        UI_PID_NAMES[0]
    }
}

/// The UI pid-file path for one build flavour, relative to the user's home
/// directory (e.g. `.nodespace/ui-dev.pid`). Mirrors
/// [`daemon_socket_relative`]'s contract exactly: both the desktop app and
/// the daemon derive this path from their own build flavour rather than one
/// side copying the other's answer.
pub const fn ui_pid_relative(is_debug: bool) -> &'static str {
    if is_debug {
        ".nodespace/ui-dev.pid"
    } else {
        ".nodespace/ui.pid"
    }
}

/// Every incompatible-database marker filename, in the same build-flavour
/// order as [`DAEMON_SOCKET_NAMES`].
///
/// The daemon writes this marker into the `.nodespace/` state directory when
/// it refuses to open its default database because the file was created by a
/// different version with a different table shape, and then exits cleanly so
/// the service manager does not respawn it into the same failure. The desktop
/// app reads it to tell a user *why* the daemon is not running, and removes it
/// once the database has been moved aside. The daemon removes it itself on the
/// next successful open. Scoped by build flavour for the same reason the
/// socket is: a dev build and a release build can disagree about the schema,
/// so one flavour's refusal says nothing about the other's.
pub const INCOMPATIBLE_DATABASE_NAMES: [&str; 2] = [
    "incompatible-database.json",
    "incompatible-database-dev.json",
];

/// The incompatible-database marker filename for one build flavour. See
/// [`daemon_socket_name`] for the parameter contract — identical here.
///
/// A filename rather than a home-relative path: the daemon resolves its state
/// directory through `NODESPACE_HOME` (so an isolated run never writes into
/// the real one), and each side joins this name onto its own state directory.
pub const fn incompatible_database_name(is_debug: bool) -> &'static str {
    if is_debug {
        INCOMPATIBLE_DATABASE_NAMES[1]
    } else {
        INCOMPATIBLE_DATABASE_NAMES[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAVOURS: [bool; 2] = [false, true];

    #[test]
    fn socket_table_holds_exactly_the_release_and_debug_names() {
        // Only the build flavour separates daemon identities: every daemon
        // registered under core's service identity uses one of these two.
        assert_eq!(DAEMON_SOCKET_NAMES, ["daemon.sock", "daemon-dev.sock"]);
        assert_eq!(daemon_socket_name(false), "daemon.sock");
        assert_eq!(daemon_socket_name(true), "daemon-dev.sock");
    }

    #[test]
    fn relative_path_is_state_dir_joined_to_name() {
        for is_debug in FLAVOURS {
            assert_eq!(
                daemon_socket_relative(is_debug),
                format!("{}/{}", STATE_DIR, daemon_socket_name(is_debug)),
                "flavour (debug={is_debug})"
            );
        }
    }

    #[test]
    fn canonical_name_is_the_release_socket() {
        // The CLI dials DAEMON_SOCKET_NAMES[0] when no daemon is running, and
        // reports "is the daemon running?" against it — that must be the socket
        // a shipped install actually uses.
        assert_eq!(DAEMON_SOCKET_NAMES[0], daemon_socket_name(false));
    }

    #[test]
    fn ui_pid_table_holds_exactly_the_release_and_debug_names() {
        assert_eq!(UI_PID_NAMES, ["ui.pid", "ui-dev.pid"]);
        assert_eq!(ui_pid_name(false), "ui.pid");
        assert_eq!(ui_pid_name(true), "ui-dev.pid");
    }

    #[test]
    fn ui_pid_relative_path_is_state_dir_joined_to_name() {
        for is_debug in FLAVOURS {
            assert_eq!(
                ui_pid_relative(is_debug),
                format!("{}/{}", STATE_DIR, ui_pid_name(is_debug)),
                "flavour (debug={is_debug})"
            );
        }
    }

    #[test]
    fn ui_pid_name_never_collides_with_a_daemon_socket_name() {
        // The two files live in the same `.nodespace/` directory, so a
        // collision here would mean the desktop app's pid file and the
        // daemon's own socket could shadow each other on disk.
        for is_debug in FLAVOURS {
            assert_ne!(ui_pid_name(is_debug), daemon_socket_name(is_debug));
        }
    }

    #[test]
    fn launcher_args_survive_an_unquoted_command_line() {
        for arg in LAUNCHER_ARGS {
            assert!(
                !arg.is_empty() && !arg.contains(|c: char| c.is_whitespace() || "\"'".contains(c)),
                "launcher argument {arg:?} would be split or mangled by a launcher that joins \
                 the entries with spaces and quotes nothing"
            );
        }
    }

    #[test]
    fn marker_table_holds_exactly_the_release_and_debug_names() {
        assert_eq!(
            INCOMPATIBLE_DATABASE_NAMES,
            [
                "incompatible-database.json",
                "incompatible-database-dev.json"
            ]
        );
        assert_eq!(
            incompatible_database_name(false),
            "incompatible-database.json"
        );
        assert_eq!(
            incompatible_database_name(true),
            "incompatible-database-dev.json"
        );
    }
}
