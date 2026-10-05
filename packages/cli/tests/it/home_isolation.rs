//! `NODESPACE_HOME` isolates the commands that work on local files.
//!
//! `nodespace uninstall` and `nodespace logs` talk to no daemon: they act on
//! files under the NodeSpace home. Each test gives the compiled binary a
//! "real" home through `HOME`, holding a full install, and a separate
//! `NODESPACE_HOME`, then checks that the real home is byte-for-byte what it
//! was.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use nodespace_proto::socket::{DAEMON_SOCKET_NAMES, STATE_DIR};

/// Every file under `root`, by relative path, with its contents.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let relative = path.strip_prefix(root).expect("under root").to_path_buf();
                files.insert(relative, fs::read(&path).expect("read file"));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, contents).expect("write file");
}

/// The files an install leaves under a home's state directory: the binaries,
/// each flavour's socket and lock file, a database and a log. `label` goes
/// into the contents, so the two homes' files differ.
fn populate_state_dir(home: &Path, label: &str) {
    let state_dir = home.join(STATE_DIR);
    write(&state_dir.join("bin").join("nodespace"), label);
    write(&state_dir.join("database").join("nodespace.db"), label);
    write(&state_dir.join("logs").join("nodespaced.log"), label);
    for name in DAEMON_SOCKET_NAMES {
        let socket = state_dir.join(name);
        write(&socket, label);
        #[cfg(unix)]
        write(
            &nodespace_daemon::single_instance::lock_path_for(&socket),
            label,
        );
    }
}

/// A user's home with NodeSpace installed: the state directory, plus the
/// service registration each platform's service manager reads.
fn real_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("real home");
    populate_state_dir(home.path(), "real");
    write(
        &home
            .path()
            .join("Library")
            .join("LaunchAgents")
            .join("app.nodespace.daemon.plist"),
        "real",
    );
    write(
        &home
            .path()
            .join(".config")
            .join("systemd")
            .join("user")
            .join("nodespace.service"),
        "real",
    );
    home
}

/// Runs `nodespace <args>` with `HOME` at `real` and `NODESPACE_HOME` at
/// `isolated`.
///
/// `PATH` is an empty directory, so the command can start no service manager
/// and no script runtime: nothing a test does reaches this machine's own
/// daemon or agent skills.
fn nodespace(real: &Path, isolated: &Path, args: &[&str]) -> Output {
    let empty_path = tempfile::tempdir().expect("empty PATH dir");
    Command::new(env!("CARGO_BIN_EXE_nodespace"))
        .args(args)
        .env("HOME", real)
        .env("NODESPACE_HOME", isolated)
        .env("PATH", empty_path.path())
        .env_remove("NODESPACED_SOCKET")
        .env_remove("NODESPACE_DATABASE")
        .output()
        .expect("run nodespace")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn uninstall_removes_the_redirected_install_and_leaves_the_real_home_alone() {
    let real = real_home();
    let isolated = tempfile::tempdir().expect("isolated home");
    populate_state_dir(isolated.path(), "isolated");
    let real_before = snapshot(real.path());

    let output = nodespace(real.path(), isolated.path(), &["uninstall"]);

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(
        snapshot(real.path()),
        real_before,
        "the real home must be untouched"
    );

    // What is left of the redirected install is its data and its log.
    let state_dir = PathBuf::from(STATE_DIR);
    let left: Vec<PathBuf> = snapshot(isolated.path()).into_keys().collect();
    assert_eq!(
        left,
        [
            state_dir.join("database").join("nodespace.db"),
            state_dir.join("logs").join("nodespaced.log"),
        ]
    );

    let out = stdout(&output);
    let kept = isolated.path().join(STATE_DIR).join("database");
    assert!(
        out.contains(&format!(
            "Your data at {} has been preserved.",
            kept.display()
        )),
        "got: {out}"
    );
    assert!(
        !out.contains(&real.path().display().to_string()),
        "the real home must not be reported: {out}"
    );
}

#[test]
fn logs_resolves_the_redirected_homes_log() {
    let real = real_home();
    let isolated = tempfile::tempdir().expect("isolated home");
    populate_state_dir(isolated.path(), "isolated");
    let real_before = snapshot(real.path());

    let path_only = nodespace(real.path(), isolated.path(), &["logs", "--path-only"]);
    let expected = isolated
        .path()
        .join(STATE_DIR)
        .join("logs")
        .join("nodespaced.log");
    assert!(path_only.status.success(), "stderr: {}", stderr(&path_only));
    assert_eq!(
        stdout(&path_only).trim_end(),
        expected.display().to_string()
    );

    // The log that is read is the redirected one, not the real home's.
    let read = nodespace(real.path(), isolated.path(), &["logs"]);
    assert!(read.status.success(), "stderr: {}", stderr(&read));
    assert_eq!(stdout(&read).trim_end(), "isolated");

    assert_eq!(snapshot(real.path()), real_before);
}

/// A redirected home with no log reports none, and names only its own path:
/// it does not fall back to the real home's log.
#[test]
fn logs_does_not_fall_back_to_the_real_homes_log() {
    let real = real_home();
    let isolated = tempfile::tempdir().expect("isolated home");

    let output = nodespace(real.path(), isolated.path(), &["logs", "--path-only"]);

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    let err = stderr(&output);
    let looked = isolated
        .path()
        .join(STATE_DIR)
        .join("logs")
        .join("nodespaced.log");
    assert!(
        err.contains(&looked.display().to_string()),
        "the redirected log path must be named: {err}"
    );
    assert!(
        !err.contains(&real.path().display().to_string()),
        "the real home must not be searched: {err}"
    );
    assert!(
        !err.contains("homebrew") && !err.contains("/usr/local"),
        "no other install's log is a candidate: {err}"
    );
}
