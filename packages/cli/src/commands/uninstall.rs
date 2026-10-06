use anyhow::Result;
use clap::Args;
use std::fs;
use std::path::Path;
use std::process::Command;

use super::skill;

#[derive(Args, Debug)]
pub struct UninstallArgs {}

/// Where the app is installed, on the one platform that installs one.
#[cfg(target_os = "macos")]
const INSTALLED_APP: Option<&str> = Some("/Applications/NodeSpace.app");
#[cfg(not(target_os = "macos"))]
const INSTALLED_APP: Option<&str> = None;

/// The `Info.plist` key an app bundle declares its product in (ADR-084), and
/// the value the free NodeSpace declares.
const PRODUCT_KEY: &str = "NodeSpaceProduct";
const COMMUNITY_PRODUCT: &str = "community";

const NOT_COMMUNITY_MESSAGE: &str = "The NodeSpace app on this Mac is a different NodeSpace \
    product, or an older NodeSpace that does not say which product it is. This command removes \
    only the free NodeSpace. To remove that app, move /Applications/NodeSpace.app to the Trash, \
    then run this command again to remove the rest.";

pub fn run(_args: UninstallArgs) -> Result<()> {
    // The state directory comes from the daemon's own resolver, so it follows
    // `NODESPACE_HOME`.
    let state_dir = nodespace_daemon::nodespace_dir()?;
    let redirected = nodespace_daemon::nodespace_home_override().is_some();

    uninstall(&state_dir, redirected, INSTALLED_APP.map(Path::new), || {
        stop_daemon();
        // Before the bin directory goes: the skill installer sits in it.
        report_skill_removal(&mut std::io::stdout(), skill::resolve_installer());
    })
}

/// Uninstalls the NodeSpace whose state directory is `state_dir`.
///
/// The service registration and the agent skills live in the user's own
/// home, not the NodeSpace home, and belong to the install there;
/// `remove_from_user_home` removes them. A run `redirected` by
/// `NODESPACE_HOME` removes only what is under its own home, so it never
/// calls it.
///
/// `installed_app` is the app bundle that install's daemon and per-user files
/// belong to. This command removes only the free NodeSpace: beside any other
/// app it would stop that app's daemon and delete its per-user files while
/// leaving the app itself behind, so it refuses before touching anything (see
/// [`uninstall_blocker`]). A redirected run is not checked, because it leaves
/// the daemon and everything else outside its own home alone, whichever app is
/// installed.
fn uninstall(
    state_dir: &Path,
    redirected: bool,
    installed_app: Option<&Path>,
    remove_from_user_home: impl FnOnce(),
) -> Result<()> {
    if redirected {
        println!(
            "NODESPACE_HOME is set: the daemon service and agent skills in your own home were left in place."
        );
    } else {
        if let Some(message) = installed_app.and_then(uninstall_blocker) {
            anyhow::bail!(message);
        }
        remove_from_user_home();
    }
    remove_bin_dir(state_dir);
    remove_sock(state_dir);

    println!(
        "NodeSpace uninstalled. Your data at {} has been preserved.",
        state_dir.join("database").display()
    );

    Ok(())
}

/// Why `uninstall` must not run with the app bundle at `app_bundle`
/// installed, or `None` when it may.
///
/// No bundle means a headless install, which is the free NodeSpace. A bundle
/// may proceed only when its `Contents/Info.plist` (XML or binary) declares
/// [`PRODUCT_KEY`] = [`COMMUNITY_PRODUCT`]. Anything else refuses: another
/// value, no key (an app built before the key existed, which cannot say what
/// it is), a value that isn't a string, or a plist that can't be read.
fn uninstall_blocker(app_bundle: &Path) -> Option<String> {
    if let Ok(false) = app_bundle.try_exists() {
        return None;
    }
    let info = plist::Value::from_file(app_bundle.join("Contents").join("Info.plist")).ok();
    let product = info
        .as_ref()
        .and_then(plist::Value::as_dictionary)
        .and_then(|info| info.get(PRODUCT_KEY))
        .and_then(plist::Value::as_string);
    if product == Some(COMMUNITY_PRODUCT) {
        None
    } else {
        Some(NOT_COMMUNITY_MESSAGE.to_owned())
    }
}

/// Remove the NodeSpace skill from every detected agent harness (Claude
/// Code, Codex, Antigravity, OpenCode, Pi), with the plugin or the
/// instructions block installed beside it, via the same cross-harness
/// installer `nodespace skill uninstall` uses, rather than hardcoding a
/// single Claude-Code-specific path. Best-effort and never
/// fatal to the rest of the uninstall: a missing or failing installer
/// (a source checkout with no built sidecar/script, or neither `bun` nor
/// `node` on `$PATH`) prints a clear, explicit message instead of silently
/// leaving harness skill files behind.
///
/// Takes `installer` as an already-resolved `Result` (rather than calling
/// [`skill::resolve_installer`] itself) and writes through a generic `w`
/// (rather than `println!` directly) so a test can exercise every branch —
/// installer missing, installer failing, and a real multi-agent outcome —
/// against a fake installer and a captured buffer instead of the real `$HOME`.
fn report_skill_removal(w: &mut impl std::io::Write, installer: Result<skill::Installer>) {
    let installer = match installer {
        Ok(installer) => installer,
        Err(e) => {
            let _ = writeln!(
                w,
                "⚠ Could not locate the skill installer — skill files were not removed from any agent harness: {e}"
            );
            return;
        }
    };

    let outcome = match skill::run_installer_subcommand(&installer, "uninstall") {
        Ok(outcome) => outcome,
        Err(e) => {
            let _ = writeln!(
                w,
                "⚠ Skill uninstall failed — skill files were not removed from any agent harness: {e}"
            );
            return;
        }
    };

    if outcome.installed.is_empty() && outcome.skipped.is_empty() {
        let _ = writeln!(w, "No installed NodeSpace skills found.");
        return;
    }
    // "removed" (not "skill removed") to match `commands::skill::uninstall`'s
    // own wording exactly — `nodespace skill uninstall` and this code path
    // now report the same underlying outcome and should read identically.
    for agent in &outcome.installed {
        let _ = writeln!(w, "✓ {agent}: removed");
    }
    for skipped in &outcome.skipped {
        let _ = writeln!(w, "⚠ {}: {}", skipped.agent, skipped.reason);
    }
}

#[cfg(target_os = "macos")]
fn stop_daemon() {
    let uid = unsafe { libc::getuid() };
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}"), "app.nodespace.daemon"])
        .status();

    if let Ok(home) = std::env::var("HOME") {
        let plist = Path::new(&home)
            .join("Library")
            .join("LaunchAgents")
            .join("app.nodespace.daemon.plist");
        let _ = fs::remove_file(&plist);
    }
}

#[cfg(target_os = "linux")]
fn stop_daemon() {
    let _ = Command::new("systemctl")
        .args(["--user", "stop", "nodespace"])
        .status();
    let _ = Command::new("systemctl")
        .args(["--user", "disable", "nodespace"])
        .status();

    if let Ok(home) = std::env::var("HOME") {
        let service = Path::new(&home)
            .join(".config")
            .join("systemd")
            .join("user")
            .join("nodespace.service");
        let _ = fs::remove_file(&service);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn stop_daemon() {}

fn remove_bin_dir(state_dir: &Path) {
    let _ = fs::remove_dir_all(state_dir.join("bin"));
}

/// Remove both build flavours' sockets, not just the release one. A single
/// `nodespace` binary uninstalls whichever build is installed, and it cannot
/// tell from its own build which flavour left a socket behind — a stale dev
/// socket file would otherwise survive an uninstall.
///
/// Each socket's single-instance lock file goes with it. Windows has no lock
/// files: its named pipe admits one server by itself.
fn remove_sock(state_dir: &Path) {
    for name in nodespace_proto::socket::DAEMON_SOCKET_NAMES {
        let socket = state_dir.join(name);
        let _ = fs::remove_file(&socket);
        #[cfg(unix)]
        let _ = fs::remove_file(nodespace_daemon::single_instance::lock_path_for(&socket));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Creates `<dir>/NodeSpace.app/Contents` and returns the bundle path.
    /// Every bundle lives in a tempdir; no test reads the real install.
    fn app_bundle(dir: &Path) -> PathBuf {
        let bundle = dir.join("NodeSpace.app");
        fs::create_dir_all(bundle.join("Contents")).expect("create bundle");
        bundle
    }

    fn info_plist(bundle: &Path) -> PathBuf {
        bundle.join("Contents").join("Info.plist")
    }

    /// An `Info.plist` like the app's, with `product` under [`PRODUCT_KEY`]
    /// when given.
    fn info(product: Option<plist::Value>) -> plist::Value {
        let mut info = plist::Dictionary::new();
        info.insert("CFBundleIdentifier".into(), "com.nodespace.desktop".into());
        if let Some(product) = product {
            info.insert(PRODUCT_KEY.into(), product);
        }
        plist::Value::Dictionary(info)
    }

    /// The blocker for a bundle whose XML `Info.plist` is `info`.
    fn blocker_for_xml(info: plist::Value) -> Option<String> {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = app_bundle(dir.path());
        info.to_file_xml(info_plist(&bundle))
            .expect("write Info.plist");
        uninstall_blocker(&bundle)
    }

    fn refused() -> Option<String> {
        Some(NOT_COMMUNITY_MESSAGE.to_owned())
    }

    /// Written out whole, so a change to the constant cannot pass unnoticed:
    /// ADR-084 fixes this wording.
    #[test]
    fn the_refusal_is_the_decided_wording() {
        assert_eq!(
            NOT_COMMUNITY_MESSAGE,
            "The NodeSpace app on this Mac is a different NodeSpace product, or an older \
             NodeSpace that does not say which product it is. This command removes only the \
             free NodeSpace. To remove that app, move /Applications/NodeSpace.app to the \
             Trash, then run this command again to remove the rest."
        );
    }

    /// A headless install has no app, and uninstalls as it always has.
    #[test]
    fn uninstall_blocker_allows_no_app() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(uninstall_blocker(&dir.path().join("NodeSpace.app")), None);
    }

    #[test]
    fn uninstall_blocker_allows_an_app_declaring_community() {
        assert_eq!(blocker_for_xml(info(Some(COMMUNITY_PRODUCT.into()))), None);
    }

    /// Built bundles can carry a binary plist; the check reads both formats.
    #[test]
    fn uninstall_blocker_allows_a_binary_plist_declaring_community() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = app_bundle(dir.path());
        info(Some(COMMUNITY_PRODUCT.into()))
            .to_file_binary(info_plist(&bundle))
            .expect("write binary Info.plist");
        assert!(fs::read(info_plist(&bundle))
            .expect("read back")
            .starts_with(b"bplist00"));

        assert_eq!(uninstall_blocker(&bundle), None);
    }

    #[test]
    fn uninstall_blocker_refuses_an_app_declaring_another_product() {
        for product in ["other-product", ""] {
            assert_eq!(
                blocker_for_xml(info(Some(product.into()))),
                refused(),
                "{PRODUCT_KEY} = {product:?}"
            );
        }
    }

    /// An app built before the key existed can't say which product it is.
    #[test]
    fn uninstall_blocker_refuses_an_app_without_the_key() {
        assert_eq!(blocker_for_xml(info(None)), refused());
    }

    #[test]
    fn uninstall_blocker_refuses_a_product_that_is_not_a_string() {
        assert_eq!(blocker_for_xml(info(Some(true.into()))), refused());
    }

    #[test]
    fn uninstall_blocker_refuses_an_unparseable_info_plist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = app_bundle(dir.path());
        fs::write(info_plist(&bundle), "not a property list").expect("write Info.plist");
        assert_eq!(uninstall_blocker(&bundle), refused());
    }

    #[test]
    fn uninstall_blocker_refuses_an_app_without_an_info_plist() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(uninstall_blocker(&app_bundle(dir.path())), refused());
    }

    /// A state directory holding an installed binary, and that binary's path.
    fn installed_state_dir(home: &Path) -> (PathBuf, PathBuf) {
        let state_dir = home.join(nodespace_proto::socket::STATE_DIR);
        let binary = state_dir.join("bin").join("nodespace");
        fs::create_dir_all(binary.parent().expect("bin dir")).expect("create bin dir");
        fs::write(&binary, "").expect("write binary");
        (state_dir, binary)
    }

    /// A bundle [`uninstall_blocker`] refuses: its `Info.plist` has no
    /// [`PRODUCT_KEY`].
    fn app_without_the_key(dir: &Path) -> PathBuf {
        let bundle = app_bundle(dir);
        info(None)
            .to_file_xml(info_plist(&bundle))
            .expect("write Info.plist");
        bundle
    }

    /// Beside an app that is not the free NodeSpace the command stops before
    /// it removes anything, in the user's home or the state directory.
    #[test]
    fn uninstall_refuses_beside_another_app_and_removes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (state_dir, binary) = installed_state_dir(dir.path());
        let app = app_without_the_key(dir.path());
        let mut removed_from_user_home = false;

        let error = uninstall(&state_dir, false, Some(&app), || {
            removed_from_user_home = true;
        })
        .expect_err("refused");

        assert_eq!(error.to_string(), NOT_COMMUNITY_MESSAGE);
        assert!(!removed_from_user_home, "the service and skills must stay");
        assert!(binary.exists(), "the state directory must be left alone");
    }

    /// Beside the free NodeSpace's app, or with no app at all (a headless
    /// install, and every platform but macOS), the whole install goes.
    #[test]
    fn uninstall_proceeds_beside_the_community_app_and_with_no_app() {
        let dir = tempfile::tempdir().expect("tempdir");
        let community = app_bundle(dir.path());
        info(Some(COMMUNITY_PRODUCT.into()))
            .to_file_xml(info_plist(&community))
            .expect("write Info.plist");
        let absent = dir.path().join("absent").join("NodeSpace.app");

        for app in [Some(community.as_path()), Some(absent.as_path()), None] {
            let home = tempfile::tempdir().expect("tempdir");
            let (state_dir, binary) = installed_state_dir(home.path());
            let mut removed_from_user_home = false;

            uninstall(&state_dir, false, app, || removed_from_user_home = true)
                .expect("uninstalled");

            assert!(removed_from_user_home, "app: {app:?}");
            assert!(!binary.exists(), "app: {app:?}");
        }
    }

    /// A run redirected by `NODESPACE_HOME` touches only its own home, so the
    /// app installed on the machine neither stops it nor is affected by it.
    #[test]
    fn a_redirected_uninstall_is_not_checked_against_the_installed_app() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (state_dir, binary) = installed_state_dir(dir.path());
        let app = app_without_the_key(dir.path());
        let mut removed_from_user_home = false;

        uninstall(&state_dir, true, Some(&app), || {
            removed_from_user_home = true;
        })
        .expect("uninstalled");

        assert!(!removed_from_user_home, "the service and skills must stay");
        assert!(!binary.exists(), "the redirected install must go");
    }

    /// `run` is the one caller that names the real install, and no test may
    /// run it against this machine's app, so its hand-off is checked in the
    /// source: the installed app goes to [`uninstall`], whose refusal the
    /// tests above cover.
    #[test]
    fn run_hands_the_installed_app_to_the_checked_uninstall() {
        let source = include_str!("uninstall.rs");
        let run = &source[source.find("pub fn run(").expect("run")..];
        let run = &run[..run.find("\n}\n").expect("end of run")];
        assert!(
            run.contains("uninstall(&state_dir, redirected, INSTALLED_APP.map(Path::new), "),
            "run no longer passes the installed app to uninstall:\n{run}"
        );
        #[cfg(target_os = "macos")]
        assert_eq!(INSTALLED_APP, Some("/Applications/NodeSpace.app"));
    }

    /// Every flavour's socket and lock file must go, and nothing beside them.
    /// The daemon leaves its lock file behind on every exit, so a lock that
    /// survived an uninstall would sit in `~/.nodespace` forever.
    #[cfg(unix)]
    #[test]
    fn remove_sock_removes_both_flavours_sockets_and_lock_files_only() {
        let home = tempfile::tempdir().expect("tempdir");
        let state_dir = home.path().join(nodespace_proto::socket::STATE_DIR);
        fs::create_dir_all(&state_dir).expect("create state dir");
        let bystander = state_dir.join("databases.toml");
        fs::write(&bystander, "").expect("write bystander");
        let mut leftovers = Vec::new();
        for name in nodespace_proto::socket::DAEMON_SOCKET_NAMES {
            let socket = state_dir.join(name);
            let lock = nodespace_daemon::single_instance::lock_path_for(&socket);
            fs::write(&socket, "").expect("write socket stand-in");
            fs::write(&lock, "1234").expect("write lock file");
            leftovers.extend([socket, lock]);
        }

        remove_sock(&state_dir);

        for leftover in leftovers {
            assert!(!leftover.exists(), "{} survived", leftover.display());
        }
        assert!(bystander.exists(), "unrelated state must be left alone");
    }

    /// Writes an executable shell script at `dir/installer.sh` and wraps it
    /// as an `Installer::Compiled` — a fake standing in for the real
    /// compiled sidecar so `report_skill_removal` can be exercised against
    /// controlled output without needing a real installer built or any
    /// agent harness actually present on this machine.
    #[cfg(unix)]
    fn fake_installer(dir: &std::path::Path, script_body: &str) -> skill::Installer {
        use std::os::unix::fs::PermissionsExt;

        let binary = dir.join("installer.sh");
        fs::write(&binary, format!("#!/bin/sh\n{script_body}\n")).expect("write fake installer");
        let mut perms = fs::metadata(&binary)
            .expect("stat fake installer")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&binary, perms).expect("chmod fake installer");

        skill::Installer::Compiled {
            binary,
            // Unused by the fake script; the real installer's
            // `--resource-root` resolution is `skill.rs`'s own concern.
            resource_root: dir.join("skill"),
        }
    }

    /// A missing installer (no compiled sidecar staged, no built
    /// `dist/install.js`) must degrade to a clear, explicit message rather
    /// than uninstall silently completing with harness skill files left
    /// behind.
    #[test]
    fn report_skill_removal_explains_when_the_installer_cannot_be_located() {
        let mut buf = Vec::new();
        report_skill_removal(
            &mut buf,
            Err(anyhow::anyhow!("no sidecar or built script found")),
        );
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Could not locate the skill installer"));
        assert!(out.contains("no sidecar or built script found"));
    }

    /// An installer that runs but exits non-zero (mirroring a real failure
    /// mode: a corrupt sidecar, a broken `bun`/`node` invocation) must also
    /// degrade to a clear message, not a silent partial uninstall.
    #[cfg(unix)]
    #[test]
    fn report_skill_removal_explains_when_the_installer_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let installer = fake_installer(dir.path(), "echo boom 1>&2; exit 1");

        let mut buf = Vec::new();
        report_skill_removal(&mut buf, Ok(installer));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("Skill uninstall failed"));
        assert!(out.contains("boom"));
    }

    /// The core fix this issue exists for: `uninstall` must invoke the same
    /// cross-harness removal `nodespace skill uninstall` uses (proven here by
    /// the fake script asserting it was called with the `uninstall`
    /// subcommand, not e.g. `install`) and report every agent's outcome —
    /// installed-and-removed and skipped-with-reason alike — not just a
    /// single hardcoded Claude Code path.
    #[cfg(unix)]
    #[test]
    fn report_skill_removal_invokes_cross_harness_uninstall_and_reports_every_agent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let installer = fake_installer(
            dir.path(),
            r#"
            if [ "$1" != "uninstall" ]; then
                echo "expected 'uninstall' subcommand, got '$1'" 1>&2
                exit 1
            fi
            echo '✓ claude-code: removed 3 file(s)'
            echo '✓ codex: removed 2 file(s)'
            echo '⚠ opencode: not installed, nothing to remove'
            "#,
        );

        let mut buf = Vec::new();
        report_skill_removal(&mut buf, Ok(installer));
        let out = String::from_utf8(buf).unwrap();

        assert!(out.contains("✓ claude-code: removed"), "got: {out}");
        assert!(out.contains("✓ codex: removed"), "got: {out}");
        assert!(
            out.contains("⚠ opencode: not installed, nothing to remove"),
            "got: {out}"
        );
    }

    /// No agent harnesses detected at all (a machine with none of the
    /// supported harnesses installed) must say so plainly rather than
    /// printing nothing.
    #[cfg(unix)]
    #[test]
    fn report_skill_removal_reports_when_nothing_was_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let installer = fake_installer(dir.path(), "true");

        let mut buf = Vec::new();
        report_skill_removal(&mut buf, Ok(installer));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("No installed NodeSpace skills found."));
    }
}
