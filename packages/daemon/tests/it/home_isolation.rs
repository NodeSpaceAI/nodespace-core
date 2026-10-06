//! `NODESPACE_HOME` alone isolates a daemon: with `NODESPACED_SOCKET` unset,
//! the socket and the lock beside it sit in the redirected home, and the
//! user's own home gets nothing.
//!
//! Each test starts a real `nodespaced` with `HOME` at a temp "real" home and
//! `NODESPACE_HOME` at another. The model path is a FIFO nobody writes to, so
//! a daemon reaches "serving" without loading a model.

#![cfg(unix)]

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nodespace_proto::socket::{daemon_socket_name, STATE_DIR};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn mkfifo(path: &Path) {
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("path has no NUL");
    // SAFETY: `c_path` is a valid NUL-terminated path for the call's duration.
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(rc, 0, "mkfifo: {}", std::io::Error::last_os_error());
}

/// Starts `nodespaced` with `HOME` at `real`, the redirect at `redirect` when
/// there is one, and no socket override. `scratch` holds the FIFO and the log.
fn spawn(real: &Path, redirect: Option<&Path>, scratch: &Path) -> Daemon {
    let fifo = scratch.join("model.gguf");
    mkfifo(&fifo);
    let log = std::fs::File::create(scratch.join("daemon.log")).expect("create log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_nodespaced"));
    command
        .env("HOME", real)
        .env_remove("NODESPACE_HOME")
        .env_remove("NODESPACED_SOCKET")
        .env_remove("NODESPACED_DB_PATH")
        .env("NODESPACED_MODEL_PATH", &fifo)
        .env("RUST_LOG", "info")
        .stdin(Stdio::null())
        .stdout(log.try_clone().expect("clone log"))
        .stderr(log);
    if let Some(redirect) = redirect {
        command.env("NODESPACE_HOME", redirect);
    }
    Daemon(command.spawn().expect("spawn nodespaced"))
}

fn wait_until_serving(daemon: &mut Daemon, socket: &Path, scratch: &Path) {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while UnixStream::connect(socket).is_err() {
        let log = std::fs::read_to_string(scratch.join("daemon.log")).unwrap_or_default();
        if let Some(status) = daemon.0.try_wait().expect("poll daemon") {
            panic!("daemon exited ({status}) before serving {socket:?}; log: {log}");
        }
        assert!(Instant::now() < deadline, "daemon never served; log: {log}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn nodespace_home_alone_moves_the_socket_and_its_lock() {
    let real = tempfile::tempdir().expect("real home");
    let isolated = tempfile::tempdir().expect("isolated home");
    let scratch = tempfile::tempdir().expect("scratch");
    let socket_name = daemon_socket_name(cfg!(debug_assertions));
    let state_dir = isolated.path().join(STATE_DIR);

    let mut daemon = spawn(real.path(), Some(isolated.path()), scratch.path());
    wait_until_serving(&mut daemon, &state_dir.join(socket_name), scratch.path());

    let lock = nodespace_daemon::single_instance::lock_path_for(&state_dir.join(socket_name));
    assert!(lock.exists(), "the lock sits beside the redirected socket");
    // No socket and no lock file in the user's own home. (The local-agent
    // model manager makes a `models` directory there on its own, which is
    // outside what this checks.)
    let real_state_dir = real.path().join(STATE_DIR);
    let leaked: Vec<String> = names_in(&real_state_dir)
        .into_iter()
        .filter(|name| name.ends_with(".sock") || name.ends_with(".lock"))
        .collect();
    assert_eq!(
        leaked,
        Vec::<String>::new(),
        "the real home must hold no socket or lock file"
    );
}

/// With no redirect the socket and lock sit in the user's home, as before.
#[test]
fn without_a_redirect_the_socket_and_lock_sit_in_the_users_home() {
    let real = tempfile::tempdir().expect("real home");
    let scratch = tempfile::tempdir().expect("scratch");
    let socket = real
        .path()
        .join(STATE_DIR)
        .join(daemon_socket_name(cfg!(debug_assertions)));

    let mut daemon = spawn(real.path(), None, scratch.path());
    wait_until_serving(&mut daemon, &socket, scratch.path());

    assert!(nodespace_daemon::single_instance::lock_path_for(&socket).exists());
}
