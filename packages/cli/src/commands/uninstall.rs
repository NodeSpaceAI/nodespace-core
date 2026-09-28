use anyhow::Result;
use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::skill;

#[derive(Args, Debug)]
pub struct UninstallArgs {}

fn home_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| anyhow::anyhow!("$HOME is unset — cannot determine home directory"))?;
    Ok(PathBuf::from(home))
}

pub fn run(_args: UninstallArgs) -> Result<()> {
    stop_daemon();

    let home = home_dir()?;

    remove_bin_dir(&home);
    remove_sock(&home);
    report_skill_removal(&mut std::io::stdout(), skill::resolve_installer());

    println!("NodeSpace uninstalled. Your data at ~/.nodespace/database/ has been preserved.");

    Ok(())
}

/// Remove the NodeSpace skill from every detected agent harness (Claude
/// Code, Codex, Gemini CLI/Antigravity, OpenCode, Pi) via the same
/// cross-harness installer `nodespace skill uninstall` uses, rather than
/// hardcoding a single Claude-Code-specific path. Best-effort and never
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
    for agent in &outcome.installed {
        let _ = writeln!(w, "✓ {agent}: skill removed");
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
        let plist = PathBuf::from(home)
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
        let service = PathBuf::from(home)
            .join(".config")
            .join("systemd")
            .join("user")
            .join("nodespace.service");
        let _ = fs::remove_file(&service);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn stop_daemon() {}

fn remove_bin_dir(home: &Path) {
    let bin_dir = home.join(".nodespace").join("bin");
    let _ = fs::remove_dir_all(&bin_dir);
}

/// Remove every build variant's socket, not just the community one. A single
/// `nodespace` binary uninstalls whichever app is installed, and it cannot tell
/// from its own build which variant left a socket behind — a stale Pro or dev
/// socket file would otherwise survive an uninstall.
fn remove_sock(home: &Path) {
    let dir = home.join(nodespace_proto::socket::STATE_DIR);
    for name in nodespace_proto::socket::DAEMON_SOCKET_NAMES {
        let _ = fs::remove_file(dir.join(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut perms = fs::metadata(&binary).expect("stat fake installer").permissions();
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

        assert!(
            out.contains("✓ claude-code: skill removed"),
            "got: {out}"
        );
        assert!(out.contains("✓ codex: skill removed"), "got: {out}");
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
