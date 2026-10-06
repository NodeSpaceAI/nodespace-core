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
fn nodespace(real: &Path, isolated: &Path, args: &[&str]) -> Output {
    command(real, args)
        .env("NODESPACE_HOME", isolated)
        .output()
        .expect("run nodespace")
}

/// `nodespace <args>` with `HOME` at `real` and no `NODESPACE_HOME`.
///
/// `PATH` names a directory that does not exist, so the command can start no
/// service manager and no script runtime: nothing a test does reaches this
/// machine's own daemon or agent skills.
fn command(real: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nodespace"));
    command
        .args(args)
        .env("HOME", real)
        .env("PATH", real.join("no-such-directory"))
        .env_remove("NODESPACE_HOME")
        .env_remove("NODESPACED_SOCKET")
        .env_remove("NODESPACE_DATABASE");
    command
}

/// Whether the app installed on this machine stops an `uninstall` that is not
/// redirected: there is one, and it does not declare itself the free
/// NodeSpace (ADR-084).
///
/// The compiled binary checks the machine's real app, which no test can
/// replace, so a test that runs it without a redirect expects whichever
/// outcome that app calls for. The check's own cases are unit-tested against
/// fixture bundles beside the command.
fn installed_app_blocks_uninstall() -> bool {
    let app = Path::new("/Applications/NodeSpace.app");
    if !cfg!(target_os = "macos") || matches!(app.try_exists(), Ok(false)) {
        return false;
    }
    let info = plist::Value::from_file(app.join("Contents").join("Info.plist")).ok();
    let product = info
        .as_ref()
        .and_then(plist::Value::as_dictionary)
        .and_then(|info| info.get("NodeSpaceProduct"))
        .and_then(plist::Value::as_string);
    product != Some("community")
}

/// The refusal beside an app that is not the free NodeSpace: the command
/// fails with the reason and removes nothing from `home`, which held `before`.
fn assert_refused(output: &Output, home: &Path, before: &BTreeMap<PathBuf, Vec<u8>>) {
    assert!(!output.status.success(), "stdout: {}", stdout(output));
    let err = stderr(output);
    assert!(
        err.contains("This command removes only the free NodeSpace."),
        "got: {err}"
    );
    assert_eq!(&snapshot(home), before, "a refusal must remove nothing");
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

    // The whole output: skill removal prints a line on every path it takes,
    // so its absence here is the proof it did not run.
    let kept = isolated.path().join(STATE_DIR).join("database");
    assert_eq!(
        stdout(&output),
        format!(
            "NODESPACE_HOME is set: the daemon service and agent skills in your own home were left in place.\n\
             NodeSpace uninstalled. Your data at {} has been preserved.\n",
            kept.display()
        )
    );
}

/// With no redirect the user's own home is the one uninstalled: the binaries,
/// sockets, lock files and this platform's service registration go, and the
/// data stays. Beside an installed app that is not the free NodeSpace the
/// command refuses instead, and the home stays whole.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn uninstall_without_a_redirect_removes_the_install_in_the_users_home() {
    let real = real_home();
    let before = snapshot(real.path());

    let output = command(real.path(), &["uninstall"])
        .output()
        .expect("run nodespace");

    if installed_app_blocks_uninstall() {
        assert_refused(&output, real.path(), &before);
        return;
    }
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let state_dir = PathBuf::from(STATE_DIR);
    // The other platform's service file stands in for an unrelated file.
    let bystander = if cfg!(target_os = "macos") {
        PathBuf::from(".config/systemd/user/nodespace.service")
    } else {
        PathBuf::from("Library/LaunchAgents/app.nodespace.daemon.plist")
    };
    let mut expected = vec![
        bystander,
        state_dir.join("database").join("nodespace.db"),
        state_dir.join("logs").join("nodespaced.log"),
    ];
    expected.sort();
    let left: Vec<PathBuf> = snapshot(real.path()).into_keys().collect();
    assert_eq!(left, expected);

    let out = stdout(&output);
    let kept = real.path().join(STATE_DIR).join("database");
    assert!(
        out.ends_with(&format!(
            "NodeSpace uninstalled. Your data at {} has been preserved.\n",
            kept.display()
        )),
        "got: {out}"
    );
    assert!(!out.contains("NODESPACE_HOME"), "got: {out}");
}

/// An empty `NODESPACE_HOME` is no redirect. Read as a path it would be the
/// working directory, and `uninstall` would delete `.nodespace/bin` there.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn an_empty_nodespace_home_is_not_a_redirect_to_the_working_directory() {
    let real = real_home();
    let real_before = snapshot(real.path());
    let working_dir = tempfile::tempdir().expect("working dir");
    populate_state_dir(working_dir.path(), "working dir");
    let before = snapshot(working_dir.path());

    let output = command(real.path(), &["uninstall"])
        .env("NODESPACE_HOME", "")
        .current_dir(working_dir.path())
        .output()
        .expect("run nodespace");

    assert_eq!(snapshot(working_dir.path()), before);
    // Only a run that is not redirected is checked against the installed app,
    // so its refusal shows the empty value was no redirect either.
    if installed_app_blocks_uninstall() {
        assert_refused(&output, real.path(), &real_before);
        return;
    }
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        !real.path().join(STATE_DIR).join("bin").exists(),
        "the user's own install is the one removed"
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

/// Binds a listener at `socket` that counts the connections it is dialed with
/// and closes each at once, so a command that reaches it fails quickly.
#[cfg(unix)]
fn count_dials(socket: &Path) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fs::create_dir_all(socket.parent().expect("parent")).expect("create state dir");
    let listener = std::os::unix::net::UnixListener::bind(socket).expect("bind socket");
    let dials = Arc::new(AtomicUsize::new(0));
    let counted = dials.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    dials
}

/// With `NODESPACE_HOME` set and no socket override, the CLI dials the
/// redirected home's socket and never the one in the real home.
#[cfg(unix)]
#[test]
fn the_cli_dials_the_redirected_homes_socket_and_not_the_real_one() {
    use std::sync::atomic::Ordering;

    let real = tempfile::tempdir().expect("real home");
    let isolated = tempfile::tempdir().expect("isolated home");
    let socket = |home: &Path| home.join(STATE_DIR).join(DAEMON_SOCKET_NAMES[0]);
    let real_dials = count_dials(&socket(real.path()));
    let isolated_dials = count_dials(&socket(isolated.path()));

    let output = nodespace(
        real.path(),
        isolated.path(),
        &["node", "get", "no-such-node"],
    );

    assert!(!output.status.success(), "stdout: {}", stdout(&output));
    assert_eq!(
        real_dials.load(Ordering::SeqCst),
        0,
        "the real socket was dialed"
    );
    assert!(
        isolated_dials.load(Ordering::SeqCst) > 0,
        "the redirected socket was never dialed; stderr: {}",
        stderr(&output)
    );
}

/// With no redirect the CLI dials the user's home, as before.
#[cfg(unix)]
#[test]
fn without_a_redirect_the_cli_dials_the_users_home() {
    use std::sync::atomic::Ordering;

    let real = tempfile::tempdir().expect("real home");
    let dials = count_dials(&real.path().join(STATE_DIR).join(DAEMON_SOCKET_NAMES[0]));

    let output = command(real.path(), &["node", "get", "no-such-node"])
        .output()
        .expect("run nodespace");

    assert!(!output.status.success());
    assert!(
        dials.load(Ordering::SeqCst) > 0,
        "stderr: {}",
        stderr(&output)
    );
}

/// `NODESPACED_SOCKET` and `--socket` still win over the home.
#[cfg(unix)]
#[test]
fn the_socket_overrides_win_over_the_redirected_home() {
    use std::sync::atomic::Ordering;

    let real = tempfile::tempdir().expect("real home");
    let isolated = tempfile::tempdir().expect("isolated home");
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let isolated_dials = count_dials(&isolated.path().join(STATE_DIR).join(DAEMON_SOCKET_NAMES[0]));
    let env_socket = elsewhere.path().join("env.sock");
    let flag_socket = elsewhere.path().join("flag.sock");
    let env_dials = count_dials(&env_socket);
    let flag_dials = count_dials(&flag_socket);

    let by_env = command(real.path(), &["node", "get", "no-such-node"])
        .env("NODESPACE_HOME", isolated.path())
        .env("NODESPACED_SOCKET", &env_socket)
        .output()
        .expect("run nodespace");
    assert!(!by_env.status.success());
    assert!(env_dials.load(Ordering::SeqCst) > 0);

    let flag = flag_socket.display().to_string();
    let by_flag = command(
        real.path(),
        &["--socket", &flag, "node", "get", "no-such-node"],
    )
    .env("NODESPACE_HOME", isolated.path())
    .env("NODESPACED_SOCKET", &env_socket)
    .output()
    .expect("run nodespace");
    assert!(!by_flag.status.success());
    assert!(flag_dials.load(Ordering::SeqCst) > 0);

    assert_eq!(isolated_dials.load(Ordering::SeqCst), 0);
}
