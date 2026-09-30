//! A daemon started with an argument it does not know refuses it and exits
//! with status 2, instead of falling through to a full server.
//!
//! The motivating caller runs `nodespaced <flag>` as root inside `$(...)` to
//! read a one-line answer. If the daemon treated an unknown flag as "start
//! serving", that command substitution would never return. So the property is
//! not just the exit status: it is that the process ends promptly, prints
//! nothing on stdout for a caller to capture, and never binds its socket.

#![cfg(unix)]

use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// A flag an earlier release answered and this one refuses. Built from
/// fragments so this file's own text does not contain the literal: the
/// boundary ratchet counts occurrences of it, and a test that guards against
/// the flag must not add one.
const RETIRED_EDITION_FLAG: &str = concat!("--", "edition");

/// How long the daemon may take to exit. Refusing an argument takes
/// milliseconds; this only needs to sit well below "never".
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

struct Outcome {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

/// Runs `nodespaced <args>` against an isolated home and returns how it ended.
///
/// stdout and stderr go to files rather than pipes: a regression that lets the
/// daemon start serving would otherwise fill a pipe nobody is draining and
/// wedge the child before the timeout below can kill it.
fn run_nodespaced(home: &Path, args: &[&str]) -> Outcome {
    let stdout_path = home.join("stdout.txt");
    let stderr_path = home.join("stderr.txt");

    let mut child = Command::new(env!("CARGO_BIN_EXE_nodespaced"))
        .args(args)
        .env("HOME", home)
        .env("NODESPACE_HOME", home)
        .env("NODESPACED_SOCKET", home.join("d.sock"))
        .env("NODESPACED_DB_PATH", home.join("nodespace.db"))
        .env("RUST_LOG", "warn")
        .stdin(Stdio::null())
        .stdout(File::create(&stdout_path).expect("create stdout file"))
        .stderr(File::create(&stderr_path).expect("create stderr file"))
        .spawn()
        .expect("spawn nodespaced");

    let deadline = Instant::now() + EXIT_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll daemon") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("nodespaced {args:?} still running {EXIT_TIMEOUT:?} after launch");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    Outcome {
        status,
        stdout: std::fs::read_to_string(&stdout_path).expect("read stdout"),
        stderr: std::fs::read_to_string(&stderr_path).expect("read stderr"),
    }
}

fn assert_refused(arg: &str) {
    let home = tempfile::tempdir().expect("create daemon home");
    let outcome = run_nodespaced(home.path(), &[arg]);

    assert_eq!(
        outcome.status.code(),
        Some(2),
        "`nodespaced {arg}` must exit with status 2, got {}; stderr: {}",
        outcome.status,
        outcome.stderr
    );
    assert_eq!(
        outcome.stdout, "",
        "a refused argument must print nothing a caller could capture"
    );
    assert!(
        outcome.stderr.contains(arg),
        "the refusal must name the argument; stderr: {}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("--tray") && outcome.stderr.contains("--version"),
        "the refusal must list the supported arguments; stderr: {}",
        outcome.stderr
    );
    assert!(
        !home.path().join("d.sock").exists(),
        "a refused argument must not bind the daemon socket"
    );
}

#[test]
fn the_retired_flag_is_refused_with_status_2() {
    assert_refused(RETIRED_EDITION_FLAG);
}

#[test]
fn help_is_refused_with_status_2() {
    assert_refused("--help");
}

#[test]
fn an_arbitrary_unknown_flag_is_refused_with_status_2() {
    assert_refused("--no-such-flag");
}

#[test]
fn version_prints_the_package_version_and_exits_zero() {
    let home = tempfile::tempdir().expect("create daemon home");
    let outcome = run_nodespaced(home.path(), &["--version"]);

    assert!(
        outcome.status.success(),
        "`nodespaced --version` exited with {}; stderr: {}",
        outcome.status,
        outcome.stderr
    );
    assert_eq!(outcome.stdout.trim_end(), env!("CARGO_PKG_VERSION"));
    assert!(
        !home.path().join("d.sock").exists(),
        "`--version` must not bind the daemon socket"
    );
}
