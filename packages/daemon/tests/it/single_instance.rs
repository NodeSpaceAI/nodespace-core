//! At most one daemon serves a socket, and a second one leaves it alone.
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
        let log = File::create(self.log(name)).expect("create daemon log");
        let child = Command::new(env!("CARGO_BIN_EXE_nodespaced"))
            .env("NODESPACE_HOME", self.dir.path())
            .env("NODESPACED_SOCKET", self.socket())
            .env("NODESPACED_MODEL_PATH", self.dir.path().join("model.gguf"))
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("clone log handle"))
            .stderr(log)
            .spawn()
            .expect("spawn nodespaced");
        Daemon { child }
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

    let mut second = home.spawn("second");
    let status = second.wait_for_exit();

    assert_eq!(
        status.code(),
        Some(0),
        "a daemon that loses the lock is a deliberate stop, not a crash"
    );
    assert!(first.is_running(), "the first daemon must keep serving");
    assert_eq!(
        std::fs::metadata(&socket).expect("stat socket").ino(),
        served_inode,
        "the second daemon must not delete and rebind the socket"
    );
    UnixStream::connect(&socket).expect("the first daemon still accepts connections");
    let second_log = std::fs::read_to_string(home.log("second")).expect("read second log");
    assert!(
        second_log.contains(&first.pid().to_string()),
        "the second daemon should log the holder's pid {}; log: {second_log}",
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
