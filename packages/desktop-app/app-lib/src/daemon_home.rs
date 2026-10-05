//! Which NodeSpace home this app's daemon serves, and how the app runs it there.
//!
//! `NODESPACE_HOME` moves every daemon state path. When it is unset, or names
//! the user's home directory, the app's daemon is the user's registered service
//! under the shared service identity (the launchd job, the systemd user unit,
//! the Windows Run value). When it names another directory, the app runs the
//! daemon as its own child process instead and uses no service manager, so a
//! run against another home never changes the user's registration. Either
//! way, every path the app derives for its daemon (its binary, logs, socket,
//! UI pid file and incompatible-database marker) is under that home.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context, Result};

/// The NodeSpace home whose `.nodespace/` this app's daemon serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DaemonHome {
    /// The user's home directory: the daemon is the user's registered service.
    User(PathBuf),
    /// Another directory, named by `NODESPACE_HOME`: the daemon is this app's
    /// child process, and no service manager is involved.
    Other(PathBuf),
}

impl DaemonHome {
    /// The home by the daemon's rule, `NODESPACE_HOME` when it is set and the
    /// user's home otherwise; `None` when neither is known.
    ///
    /// A `NODESPACE_HOME` that names the user's home directory, however it is
    /// spelled, is the user's home, held as `user_home` spells it. Every path
    /// the app derives for its daemon then comes out exactly as it does with
    /// the variable unset.
    pub(crate) fn resolve(
        nodespace_home: Option<String>,
        user_home: Option<PathBuf>,
    ) -> Option<Self> {
        match (nodespace_home.map(PathBuf::from), user_home) {
            (Some(home), Some(user)) if same_directory(&home, &user) => Some(Self::User(user)),
            (Some(home), _) => Some(Self::Other(home)),
            (None, Some(user)) => Some(Self::User(user)),
            (None, None) => None,
        }
    }

    /// [`Self::resolve`] from this process's environment. `std::env::var`, as
    /// the daemon reads it, so a value that is not valid UTF-8 counts as unset
    /// on both sides.
    pub(crate) fn current() -> Option<Self> {
        Self::resolve(std::env::var("NODESPACE_HOME").ok(), dirs::home_dir())
    }

    /// The home directory itself, the parent of its `.nodespace/`.
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::User(path) | Self::Other(path) => path,
        }
    }

    /// Whether this is another home than the user's.
    pub(crate) fn is_other(&self) -> bool {
        matches!(self, Self::Other(_))
    }
}

/// Whether `a` and `b` name one directory: equal paths, or paths that resolve
/// to the same existing directory.
fn same_directory(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// The Named Pipe this app dials and serves when `NODESPACED_SOCKET` names
/// none: the shared pipe for the user's home, and for another home that name
/// followed by a hash of the home path.
///
/// The pipe namespace is machine-wide, so another home's daemon on the shared
/// pipe would answer the user's app and CLI. The hash keeps a home on the same
/// pipe across runs.
#[cfg(any(windows, test))]
pub(crate) fn default_pipe_name(home: Option<&DaemonHome>) -> String {
    use sha2::{Digest, Sha256};

    let shared = nodespace_proto::socket::DAEMON_PIPE_NAME;
    match home {
        Some(DaemonHome::Other(home)) => {
            let digest = Sha256::digest(home.as_os_str().as_encoded_bytes());
            let hash: String = digest[..8]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            format!("{shared}-{hash}")
        }
        _ => shared.to_string(),
    }
}

/// The `taskkill` arguments that stop this app's daemon on Windows, or `None`
/// when there is none this app may stop.
///
/// The user's daemon is stopped by image name, as it always has been. Another
/// home's daemon is stopped by the process id this app started it under, and
/// never by name, which would stop the user's daemon as well.
#[cfg(any(windows, test))]
pub(crate) fn windows_stop_args(
    home: &DaemonHome,
    image_name: &str,
    child: Option<u32>,
) -> Option<Vec<String>> {
    match home {
        DaemonHome::User(_) => Some(vec!["/F".into(), "/IM".into(), image_name.into()]),
        DaemonHome::Other(_) => child.map(|pid| vec!["/F".into(), "/PID".into(), pid.to_string()]),
    }
}

/// Process id of the daemon this app started for another home while it runs,
/// 0 when there is none.
static CHILD_PID: AtomicU32 = AtomicU32::new(0);

/// The process id of the daemon this app started for another home, while it
/// runs.
#[cfg(windows)]
pub(crate) fn child_pid() -> Option<u32> {
    match CHILD_PID.load(Ordering::SeqCst) {
        0 => None,
        pid => Some(pid),
    }
}

/// The command that runs `daemon_bin` for another home: headless, since the
/// launcher arguments ask for a tray, serving `home` on `endpoint`.
///
/// Both variables are set explicitly. `NODESPACE_HOME` would be inherited
/// anyway; `NODESPACED_SOCKET` would not when the endpoint is the home's
/// default, and the daemon's own fallback is the socket under the user's home.
pub(crate) fn child_command(
    daemon_bin: &Path,
    home: &Path,
    endpoint: &Path,
) -> std::process::Command {
    let mut command = std::process::Command::new(daemon_bin);
    command
        .env("NODESPACE_HOME", home)
        .env(nodespace_proto::socket::SOCKET_ENV_VAR, endpoint)
        .stdin(std::process::Stdio::null());
    command
}

/// Refuses a Unix socket path the OS cannot bind, before a daemon is started
/// to fail on it. A path under a deep home can exceed the limit (about 104
/// bytes on macOS).
#[cfg(unix)]
pub(crate) fn check_socket_path(endpoint: &Path) -> Result<()> {
    std::os::unix::net::SocketAddr::from_pathname(endpoint)
        .map(|_| ())
        .with_context(|| {
            format!(
                "the daemon socket path {} is too long for a Unix socket; set {} to a shorter path",
                endpoint.display(),
                nodespace_proto::socket::SOCKET_ENV_VAR
            )
        })
}

/// Starts `daemon_bin` as this app's child for another home, with its output
/// appended to the daemon log files in `log_dir`, and returns its process id.
///
/// A thread waits on the child, so it is reaped when it exits and its id is
/// forgotten. The app stops it when it quits, as it stops the user's daemon.
pub(crate) fn spawn_child(
    daemon_bin: &Path,
    home: &Path,
    endpoint: &Path,
    log_dir: &Path,
) -> Result<u32> {
    #[cfg(unix)]
    check_socket_path(endpoint)?;

    let (stdout_log, stderr_log) = crate::daemon_setup::daemon_log_paths(log_dir);
    let mut command = child_command(daemon_bin, home, endpoint);
    command
        .stdout(crate::daemon_setup::daemon_log_stdio(&stdout_log))
        .stderr(crate::daemon_setup::daemon_log_stdio(&stderr_log));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window for a console program started from the GUI app.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to start {}", daemon_bin.display()))?;
    let pid = child.id();
    CHILD_PID.store(pid, Ordering::SeqCst);
    std::thread::spawn(move || {
        let _ = child.wait();
        let _ = CHILD_PID.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst);
    });
    tracing::info!(
        pid,
        home = %home.display(),
        endpoint = %endpoint.display(),
        "started the daemon for another NodeSpace home as this app's child"
    );
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_the_users_own_directory_is_the_users_home() {
        let user = tempfile::tempdir().expect("user home");
        let user_path = user.path().to_path_buf();

        assert_eq!(
            DaemonHome::resolve(None, Some(user_path.clone())),
            Some(DaemonHome::User(user_path.clone()))
        );
        // Spelled with a trailing separator, or through a symlink: still the
        // user's home, held as the user's home is spelled.
        let trailing = format!("{}/", user_path.display());
        assert_eq!(
            DaemonHome::resolve(Some(trailing), Some(user_path.clone())),
            Some(DaemonHome::User(user_path.clone()))
        );
        #[cfg(unix)]
        {
            let links = tempfile::tempdir().expect("link dir");
            let link = links.path().join("home-link");
            std::os::unix::fs::symlink(&user_path, &link).expect("symlink");
            assert_eq!(
                DaemonHome::resolve(Some(link.display().to_string()), Some(user_path.clone())),
                Some(DaemonHome::User(user_path))
            );
        }
    }

    #[test]
    fn any_other_directory_is_another_home() {
        let user = tempfile::tempdir().expect("user home");
        let other = tempfile::tempdir().expect("other home");
        let other_path = other.path().to_path_buf();

        assert_eq!(
            DaemonHome::resolve(
                Some(other_path.display().to_string()),
                Some(user.path().to_path_buf())
            ),
            Some(DaemonHome::Other(other_path.clone()))
        );
        // A home that does not exist yet is not the user's either.
        let missing = other_path.join("not-yet");
        assert_eq!(
            DaemonHome::resolve(
                Some(missing.display().to_string()),
                Some(user.path().to_path_buf())
            ),
            Some(DaemonHome::Other(missing))
        );
        assert_eq!(
            DaemonHome::resolve(Some(other_path.display().to_string()), None),
            Some(DaemonHome::Other(other_path))
        );
        assert_eq!(DaemonHome::resolve(None, None), None);
    }

    /// Another home's daemon never answers on the shared pipe, and a home keeps
    /// one pipe; the user's home keeps the shared pipe.
    #[test]
    fn another_home_gets_its_own_pipe() {
        let shared = nodespace_proto::socket::DAEMON_PIPE_NAME;
        let user = DaemonHome::User(PathBuf::from(r"C:\Users\me"));
        assert_eq!(default_pipe_name(None), shared);
        assert_eq!(default_pipe_name(Some(&user)), shared);

        let a = DaemonHome::Other(PathBuf::from(r"C:\scratch\a"));
        let b = DaemonHome::Other(PathBuf::from(r"C:\scratch\b"));
        let pipe_a = default_pipe_name(Some(&a));
        assert!(pipe_a.starts_with(&format!("{shared}-")), "{pipe_a}");
        assert_eq!(pipe_a.len(), shared.len() + 1 + 16, "{pipe_a}");
        assert_eq!(pipe_a, default_pipe_name(Some(&a)), "stable for one home");
        assert_ne!(pipe_a, default_pipe_name(Some(&b)), "distinct per home");
    }

    /// The user's daemon is stopped by name, as always; another home's only by
    /// the process id this app started, since its name is the user daemon's.
    #[test]
    fn another_homes_daemon_is_stopped_by_process_id_never_by_name() {
        let user = DaemonHome::User(PathBuf::from(r"C:\Users\me"));
        let other = DaemonHome::Other(PathBuf::from(r"C:\scratch"));

        assert_eq!(
            windows_stop_args(&user, "nodespaced.exe", Some(42)),
            Some(vec![
                "/F".to_string(),
                "/IM".into(),
                "nodespaced.exe".into()
            ])
        );
        assert_eq!(
            windows_stop_args(&other, "nodespaced.exe", Some(42)),
            Some(vec!["/F".to_string(), "/PID".into(), "42".into()])
        );
        assert_eq!(
            windows_stop_args(&other, "nodespaced.exe", None),
            None,
            "nothing this app started, so nothing to stop"
        );
    }

    /// Headless (no tray flag), with the home and the endpoint set explicitly.
    #[test]
    fn the_child_runs_headless_on_the_homes_endpoint() {
        let command = child_command(
            Path::new("/scratch/.nodespace/bin/nodespaced"),
            Path::new("/scratch"),
            Path::new("/scratch/.nodespace/daemon-dev.sock"),
        );

        assert_eq!(command.get_program(), "/scratch/.nodespace/bin/nodespaced");
        assert_eq!(command.get_args().count(), 0, "no launcher arguments");
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&(
            std::ffi::OsStr::new("NODESPACE_HOME"),
            Some(std::ffi::OsStr::new("/scratch"))
        )));
        assert!(envs.contains(&(
            std::ffi::OsStr::new("NODESPACED_SOCKET"),
            Some(std::ffi::OsStr::new("/scratch/.nodespace/daemon-dev.sock"))
        )));
    }

    /// A socket path the OS cannot bind is refused before anything starts, with
    /// the variable that fixes it.
    #[cfg(unix)]
    #[test]
    fn a_socket_path_too_long_to_bind_is_refused_before_starting() {
        let logs = tempfile::tempdir().expect("log dir");
        let long = PathBuf::from(format!("/tmp/{}/daemon-dev.sock", "d".repeat(120)));

        let error = spawn_child(
            Path::new("/usr/bin/true"),
            Path::new("/tmp"),
            &long,
            logs.path(),
        )
        .expect_err("a socket path past the limit must be refused");

        assert!(
            format!("{error:#}").contains("NODESPACED_SOCKET"),
            "{error:#}"
        );
        assert!(
            std::fs::read_dir(logs.path())
                .expect("read log dir")
                .next()
                .is_none(),
            "nothing was started, so no log was opened"
        );
        assert!(check_socket_path(Path::new("/tmp/nsd/d.sock")).is_ok());
    }
}
