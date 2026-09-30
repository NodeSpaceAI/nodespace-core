//! At most one daemon serves a socket, a second one leaves it alone, and a new
//! daemon started while the old one is still draining waits for it, and exits
//! with an error rather than 0 if nothing ever serves.
//!
//! Both serve loops delete an existing socket file before they bind, so
//! without the lock a second daemon silently takes the socket over while the
//! first keeps its databases open. Every test here starts real `nodespaced`
//! processes against a tempdir home and socket, and never the user's daemon.
//!
//! The model path is a FIFO nobody writes to: the model load blocks on it
//! forever, so a daemon reaches "serving" without ever needing a real model.

#![cfg(unix)]

use std::ffi::CString;
use std::fs::File;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use nodespace_daemon::single_instance::{
    acquire_waiting, Acquire, InstanceLock, LOCK_WAIT_ENV_VAR,
};

/// How long a daemon may take to start accepting connections.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a daemon may take to exit. A daemon that loses the lock, or is told
/// to stop with no clients, ends in milliseconds; this only needs to sit well
/// below "never".
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

fn mkfifo(path: &Path) {
    let c_path = CString::new(path.as_os_str().as_bytes()).expect("fifo path has no NUL");
    // SAFETY: `c_path` is a valid NUL-terminated path for the call's duration.
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(
        rc,
        0,
        "mkfifo {}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
}

/// An isolated NodeSpace home with a socket and a never-ready model in it.
struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create daemon home");
        mkfifo(&dir.path().join("model.gguf"));
        Self { dir }
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("d.sock")
    }

    /// Starts a daemon on this home's socket. Its output goes to `<name>.log`.
    fn spawn(&self, name: &str) -> Daemon {
        self.spawn_with(name, |command| command)
    }

    /// [`Self::spawn`], but the daemon gives up on a held lock after
    /// `lock_wait` instead of the default, so a test that expects it to lose
    /// need not sit out the full wait.
    fn spawn_giving_up_after(&self, name: &str, lock_wait: Duration) -> Daemon {
        self.spawn_with(name, |command| {
            command.env(LOCK_WAIT_ENV_VAR, lock_wait.as_millis().to_string())
        })
    }

    fn spawn_with(
        &self,
        name: &str,
        configure: impl FnOnce(&mut Command) -> &mut Command,
    ) -> Daemon {
        let log = File::create(self.log(name)).expect("create daemon log");
        let mut command = Command::new(env!("CARGO_BIN_EXE_nodespaced"));
        command
            .env("NODESPACE_HOME", self.dir.path())
            .env("NODESPACED_SOCKET", self.socket())
            .env("NODESPACED_MODEL_PATH", self.dir.path().join("model.gguf"))
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("clone log handle"))
            .stderr(log);
        configure(&mut command);
        Daemon {
            child: command.spawn().expect("spawn nodespaced"),
        }
    }

    /// Takes the lock from this test process, as a daemon that is still
    /// draining holds it.
    fn hold_lock(&self) -> InstanceLock {
        match acquire_waiting(&self.socket(), Duration::ZERO).expect("take the lock") {
            Acquire::Held(lock) => lock,
            other => panic!("expected the lock to be free, got {other:?}"),
        }
    }

    fn log(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!("{name}.log"))
    }
}

/// A daemon this test started. It is killed on drop, so a failing assertion
/// never leaves one running.
struct Daemon {
    child: Child,
}

impl Daemon {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn is_running(&mut self) -> bool {
        self.child.try_wait().expect("poll daemon").is_none()
    }

    /// Waits until the daemon's log at `log` contains `text`. Fails if the
    /// daemon exits first, since it would then never write it.
    fn wait_for_log(&mut self, log: &Path, text: &str) {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            let written = std::fs::read_to_string(log).unwrap_or_default();
            if written.contains(text) {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("poll daemon") {
                panic!("daemon exited ({status}) before logging {text:?}; log: {written}");
            }
            assert!(
                Instant::now() < deadline,
                "daemon did not log {text:?} in time; log: {written}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Waits until the daemon accepts connections on `socket`. A stale socket
    /// file left by a crashed daemon exists but refuses connections, so the
    /// file's presence alone is not proof.
    fn wait_until_serving(&mut self, socket: &Path) {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        while UnixStream::connect(socket).is_err() {
            if let Some(status) = self.child.try_wait().expect("poll daemon") {
                panic!("daemon exited before serving its socket: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "daemon did not start serving in time"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn wait_for_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + EXIT_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll daemon") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "daemon {} still running {EXIT_TIMEOUT:?} after it should have exited",
                self.pid()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn signal(&self, signal: libc::c_int) {
        // SAFETY: signalling a child process we spawned and have not yet reaped.
        let rc = unsafe { libc::kill(self.pid() as libc::pid_t, signal) };
        assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_second_daemon_on_a_served_socket_exits_zero_and_leaves_the_socket_alone() {
    let home = Home::new();
    let socket = home.socket();
    let mut first = home.spawn("first");
    first.wait_until_serving(&socket);
    let served_inode = std::fs::metadata(&socket).expect("stat socket").ino();

    let lock_wait = Duration::from_millis(1500);
    let started = Instant::now();
    let mut second = home.spawn_giving_up_after("second", lock_wait);
    let status = second.wait_for_exit();
    let lived = started.elapsed();

    assert_eq!(
        status.code(),
        Some(0),
        "a daemon that loses the lock is a deliberate stop, not a crash"
    );
    assert!(
        lived >= lock_wait,
        "the second daemon gave up after {lived:?}, before its {lock_wait:?} wait for the lock"
    );
    assert!(first.is_running(), "the first daemon must keep serving");
    assert_eq!(
        std::fs::metadata(&socket).expect("stat socket").ino(),
        served_inode,
        "the second daemon must not delete and rebind the socket"
    );
    UnixStream::connect(&socket).expect("the first daemon still accepts connections");
    // The line it exits on, not an earlier one, must name the holder: the pid
    // is read again at the end of the wait.
    let second_log = std::fs::read_to_string(home.log("second")).expect("read second log");
    let exit_line = second_log
        .lines()
        .find(|line| line.contains("after waiting; exiting"))
        .unwrap_or_else(|| panic!("the second daemon logged no exit line; log: {second_log}"));
    assert!(
        exit_line.contains(&format!("Some({})", first.pid())),
        "the exit line should name the holder, pid {}; line: {exit_line}",
        first.pid()
    );

    first.signal(libc::SIGTERM);
    assert!(first.wait_for_exit().success());
}

#[test]
fn daemons_on_different_sockets_do_not_block_each_other() {
    let (home_a, home_b) = (Home::new(), Home::new());
    let mut a = home_a.spawn("a");
    a.wait_until_serving(&home_a.socket());

    let mut b = home_b.spawn("b");
    b.wait_until_serving(&home_b.socket());

    assert!(a.is_running() && b.is_running());
}

#[test]
fn a_crashed_daemon_leaves_no_lock_behind() {
    let home = Home::new();
    let socket = home.socket();
    let mut crashed = home.spawn("crashed");
    crashed.wait_until_serving(&socket);
    crashed.signal(libc::SIGKILL);
    let status = crashed.wait_for_exit();
    assert!(!status.success(), "SIGKILL is not a clean exit: {status}");

    // The lock file and a stale socket are both still on disk.
    let mut successor = home.spawn("successor");
    successor.wait_until_serving(&socket);

    assert!(successor.is_running());
}

#[test]
fn a_daemon_started_while_the_lock_is_held_waits_for_it_and_then_serves() {
    let home = Home::new();
    let socket = home.socket();
    // The old daemon has been told to stop but is still draining, so it holds
    // the lock and no longer serves. It exits with status 0 once it is done.
    let draining = home.hold_lock();

    let mut successor = home.spawn("successor");
    successor.wait_for_log(
        &home.log("successor"),
        "waiting for the single-instance lock",
    );
    assert!(
        successor.is_running(),
        "a daemon that finds the lock held must wait for it, not exit"
    );

    drop(draining);
    successor.wait_until_serving(&socket);

    assert!(successor.is_running());
}

#[test]
fn a_daemon_that_finds_the_lock_held_but_nothing_serving_exits_with_an_error() {
    let home = Home::new();
    // A holder that never serves: still starting, or stuck in a slow drain. A
    // status of 0 would be a deliberate stop that launchd never restarts, and
    // the user would be left with no daemon.
    let _stuck = home.hold_lock();

    let mut successor = home.spawn_giving_up_after("successor", Duration::from_millis(500));
    let status = successor.wait_for_exit();

    assert!(
        !status.success(),
        "a daemon that finds nothing serving must not exit as if a peer did: {status}"
    );
    let log = std::fs::read_to_string(home.log("successor")).expect("read successor log");
    assert!(
        log.contains("nothing answers on the socket"),
        "the exit should say why; log: {log}"
    );
    assert!(
        !home.socket().exists(),
        "a daemon that never held the lock must not create the socket"
    );
}
