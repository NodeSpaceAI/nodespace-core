//! A daemon told to stop while its shared embedding model is still loading
//! exits promptly, instead of lingering until the load finishes.
//!
//! The load runs inside `spawn_blocking`, and dropping a tokio runtime joins
//! every in-flight blocking task, so without an explicit early exit the
//! process outlives its own completed graceful shutdown by however long the
//! load still has to go.
//!
//! The model path is a FIFO nobody writes to: the load's integrity check
//! blocks reading it, so the load is reliably still in flight when the signal
//! arrives, and never finishes on its own. A regression therefore shows up as
//! a daemon that never exits, not merely a slow one.

#![cfg(unix)]

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long the daemon may take to bind its socket after spawning.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the daemon may take to exit after SIGTERM. Graceful shutdown with
/// no clients takes milliseconds; this only needs to sit well below "never".
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

fn wait_for_socket(child: &mut Child, socket: &Path) {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while !socket.exists() {
        if let Some(status) = child.try_wait().expect("poll daemon") {
            panic!("daemon exited before binding its socket: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not bind its socket in time"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sigterm_while_model_loads_exits_promptly() {
    let home = tempfile::tempdir().expect("create daemon home");
    let socket = home.path().join("d.sock");
    let model = home.path().join("model.gguf");
    mkfifo(&model);

    let mut child = Command::new(env!("CARGO_BIN_EXE_nodespaced"))
        .env("NODESPACE_HOME", home.path())
        .env("NODESPACED_SOCKET", &socket)
        .env("NODESPACED_MODEL_PATH", &model)
        .env("RUST_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn nodespaced");

    wait_for_socket(&mut child, &socket);

    // SAFETY: signalling a child process we spawned and have not yet reaped.
    let rc = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(rc, 0, "kill: {}", std::io::Error::last_os_error());

    let deadline = Instant::now() + EXIT_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll daemon") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("daemon still running {EXIT_TIMEOUT:?} after SIGTERM mid model load");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "daemon exited with {status}");
}
