//! `nodespace skill ...` — install, remove, or check the NodeSpace skill for
//! detected AI-agent harnesses (Claude Code, Codex, Gemini CLI, OpenCode),
//! reachable without the desktop app.
//!
//! Closes the CLI → skill loop: `packages/skill`'s own README tells a user
//! who arrives from the skill to install the CLI via `install.sh`, but until
//! this subcommand existed nothing on the CLI side then installed the skill
//! itself. GUI users already get this via `skill_setup.rs`'s first-launch
//! installer; this is the equivalent entry point for a CLI-only install, and
//! also the only path that revisits a harness installed *after* the initial
//! setup (`install_skill` only runs during GUI onboarding).
//!
//! # How the installer is invoked
//!
//! Mirrors `skill_setup.rs`'s `Installer` enum, minus every Tauri-specific
//! resource-resolution piece (this crate has no `AppHandle` and must not
//! depend on `desktop-app/src-tauri`):
//!
//!   1. **Compiled sidecar** (preferred): `nodespace-skill-installer`, built
//!      for the same headless targets `nodespace`/`nodespaced` ship for (see
//!      `.github/workflows/release.yml`'s `build-headless` job) and placed
//!      beside them — in `NS_BIN_DIR` after a real install, or beside the
//!      `nodespace` binary under test. Resolved relative to
//!      `current_exe()`, the same sidecar-adjacency convention
//!      `daemon_setup::sidecar_path_from_exe` uses for `nodespaced`.
//!   2. **Script fallback**: `packages/skill/dist/install.js`, run via `bun`
//!      then `node` — a source checkout that hasn't downloaded/built the
//!      compiled sidecar (dev, or `cargo run` against the monorepo).
//!
//! Both invocations share the exact "✓ agent: ..." / "⚠ agent: reason" stdout
//! contract `packages/skill/src/install.ts` prints and `skill_setup.rs`
//! already parses; [`parse_installer_output`] here is a close copy of that
//! parser rather than a shared dependency, since the two crates cannot
//! depend on each other.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Subcommand, Debug)]
pub enum SkillAction {
    /// Detect AI-agent harnesses and install the NodeSpace skill into them.
    /// Safe to re-run: already-installed harnesses are left alone, and a
    /// harness installed since the last run is picked up.
    Install(InstallArgs),
    /// Remove the NodeSpace skill from detected (or specified) harnesses.
    Uninstall(UninstallArgs),
    /// Report which harnesses currently have the skill installed.
    Status,
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Install without prompting for confirmation. Implied automatically
    /// when stdin/stdout isn't a terminal (CI, a script, an agent's
    /// non-interactive shell) — mirrors install.sh's `--gui`/`--no-gui`
    /// no-TTY default: never hang waiting on a prompt that can't be
    /// answered.
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args, Debug)]
pub struct UninstallArgs {}

pub fn run(action: SkillAction) -> Result<()> {
    match action {
        SkillAction::Install(args) => install(args),
        SkillAction::Uninstall(_args) => uninstall(),
        SkillAction::Status => status(),
    }
}

fn install(args: InstallArgs) -> Result<()> {
    let installer = resolve_installer()?;

    if !args.yes && !confirm_install()? {
        println!("Skipped.");
        return Ok(());
    }

    let outcome = run_installer_subcommand(&installer, "install")?;
    report_install_outcome(&outcome);
    Ok(())
}

/// Prompt on a real terminal; auto-confirm (matching install.sh's no-TTY
/// default of proceeding with the documented default action) when stdin or
/// stdout isn't one, stating that the prompt was skipped so the choice is
/// visible in captured output.
fn confirm_install() -> Result<bool> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        println!("No interactive terminal detected -- proceeding with install (pass --yes to silence this message).");
        return Ok(true);
    }

    print!("Install the NodeSpace skill into detected agent harnesses? [Y/n] ");
    use std::io::Write;
    std::io::stdout().flush().ok();

    let mut reply = String::new();
    std::io::stdin()
        .read_line(&mut reply)
        .context("Failed to read confirmation from stdin")?;
    let reply = reply.trim().to_ascii_lowercase();
    Ok(reply.is_empty() || reply == "y" || reply == "yes")
}

fn uninstall() -> Result<()> {
    let installer = resolve_installer()?;
    let outcome = run_installer_subcommand(&installer, "uninstall")?;
    if outcome.installed.is_empty() && outcome.skipped.is_empty() {
        println!("No installed NodeSpace skills found.");
        return Ok(());
    }
    for agent in &outcome.installed {
        println!("✓ {agent}: removed");
    }
    for skipped in &outcome.skipped {
        println!("⚠ {}: {}", skipped.agent, skipped.reason);
    }
    Ok(())
}

fn status() -> Result<()> {
    let installer = resolve_installer()?;
    let outcome = run_installer_subcommand(&installer, "status")?;
    for agent in &outcome.installed {
        println!("✓ {agent}: present");
    }
    for skipped in &outcome.skipped {
        println!("  {}: {}", skipped.agent, skipped.reason);
    }
    if outcome.installed.is_empty() && outcome.skipped.is_empty() {
        println!("No agent harnesses detected.");
    }
    Ok(())
}

fn report_install_outcome(outcome: &InstallOutcome) {
    if outcome.installed.is_empty() && outcome.skipped.is_empty() {
        println!("No supported agent harnesses detected.");
        return;
    }
    for agent in &outcome.installed {
        println!("✓ {agent}: skill installed");
    }
    for skipped in &outcome.skipped {
        println!("⚠ {}: {}", skipped.agent, skipped.reason);
    }
}

/// One "agent: reason" pairing from a "⚠ agent: reason" installer line.
///
/// `pub(crate)`: reused as-is by `commands::mcp`'s `install`/`uninstall`/
/// `status` subcommands, which invoke the exact same compiled installer
/// binary/script (it bundles `packages/skill`'s full `dist/`, not just the
/// skill-file installer) with different subcommand strings
/// (`mcp-install`/`mcp-uninstall`/`mcp-status`) and parse its output through
/// the same "✓ name: ..." / "⚠ name: reason" contract. One MCP client name
/// (e.g. `claude-desktop`) plays the same role here that an agent name plays
/// for the skill installer, so the field is still called `agent` rather than
/// generalized to `item` -- renaming it is not worth the diff noise.
#[derive(Debug)]
pub(crate) struct SkippedAgent {
    pub(crate) agent: String,
    pub(crate) reason: String,
}

/// Parsed result of one installer invocation: agents actually acted on
/// (installed, removed, or found present, depending on subcommand), and
/// agents detected but skipped with a reason.
///
/// `pub(crate)` -- see [`SkippedAgent`]'s doc comment for why `commands::mcp`
/// shares this instead of a parallel copy.
#[derive(Debug)]
pub(crate) struct InstallOutcome {
    pub(crate) installed: Vec<String>,
    pub(crate) skipped: Vec<SkippedAgent>,
}

/// How to invoke the skill installer, resolved once by [`resolve_installer`].
///
/// `pub(crate)` -- see [`SkippedAgent`]'s doc comment; `commands::mcp` shares
/// this resolution rather than re-implementing sidecar/script discovery for
/// what is, on disk, the exact same installer.
#[derive(Debug)]
pub(crate) enum Installer {
    /// The compiled standalone sidecar -- no bun/node dependency. Mirrors
    /// `skill_setup.rs`'s `Installer::Compiled`, minus the Tauri resource
    /// resolver: `resource_root` is derived from the sidecar's own
    /// directory instead (see [`resolve_installer`]).
    Compiled {
        binary: PathBuf,
        resource_root: PathBuf,
    },
    /// The plain JS build, run via `bun` or `node`.
    Script { path: PathBuf },
}

/// Locate the skill installer: the compiled sidecar beside the running
/// `nodespace` executable if present, the source-checkout script otherwise.
///
/// `pub(crate)` -- shared with `commands::mcp`; see [`SkippedAgent`]'s doc
/// comment.
pub(crate) fn resolve_installer() -> Result<Installer> {
    if let Some(installer) = resolve_compiled_installer() {
        return Ok(installer);
    }
    resolve_script_installer()
}

/// The installed sidecar's filename: the bare name plus the platform's
/// native executable extension. Mirrors `daemon_setup::bundled_sidecar_name`
/// (that function is `pub(crate)` to the desktop-app crate and unreachable
/// from here).
fn bundled_sidecar_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// The compiled `nodespace-skill-installer` sidecar, if one is staged beside
/// the running `nodespace` executable -- the same directory `install.sh`
/// and the Homebrew formula place `nodespace`/`nodespaced` in. Its resource
/// root (SKILL.md/shims/references) is staged as a sibling `skill/`
/// directory next to the sidecar, laid down by the same release step that
/// places the sidecar itself (see `scripts/build-skill.ts` and
/// `.github/workflows/release.yml`). Returns `None` when either piece is
/// missing -- a dev/source checkout that hasn't downloaded the sidecar, or a
/// platform this hasn't been wired up for -- and the caller falls through
/// to [`resolve_script_installer`].
fn resolve_compiled_installer() -> Option<Installer> {
    let exe = std::env::current_exe().ok()?;
    compiled_installer_beside(&exe)
}

/// Pure form of [`resolve_compiled_installer`]: takes the executable path as
/// a parameter (rather than calling `current_exe()` itself) so this is
/// exercisable against a synthetic directory in a unit test, independent of
/// where cargo actually places the test binary -- the same seam
/// `daemon_setup::sidecar_path_from_exe` uses for the identical reason.
fn compiled_installer_beside(exe: &Path) -> Option<Installer> {
    let dir = exe.parent()?;
    let binary = dir.join(bundled_sidecar_name("nodespace-skill-installer"));
    if !binary.exists() {
        return None;
    }
    let resource_root = dir.join("skill");
    if !resource_root.join("SKILL.md").exists() {
        return None;
    }
    Some(Installer::Compiled {
        binary,
        resource_root,
    })
}

/// The plain JS installer script (`dist/install.js`), resolved relative to
/// this crate's own location in a monorepo checkout -- the fallback used
/// when no compiled sidecar is staged. Mirrors `skill_setup.rs`'s
/// `resolve_installer_path` fallback branch.
fn resolve_script_installer() -> Result<Installer> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("skill")
        .join("dist")
        .join("install.js");
    if !path.exists() {
        anyhow::bail!(
            "Skill installer not found. Expected a compiled `nodespace-skill-installer` sidecar \
             beside this binary, or a built {} (run `bun run build:skill` from a source checkout).",
            path.display()
        );
    }
    Ok(Installer::Script { path })
}

/// Runtimes capable of executing the installer script, tried in this order
/// -- see `skill_setup.rs`'s `INSTALLER_RUNTIMES` for why `bun` is tried
/// first and `node` second.
const INSTALLER_RUNTIMES: [&str; 2] = ["bun", "node"];

/// `pub(crate)` -- shared with `commands::mcp`; see [`SkippedAgent`]'s doc
/// comment.
pub(crate) fn run_installer_subcommand(
    installer: &Installer,
    subcommand: &str,
) -> Result<InstallOutcome> {
    match installer {
        Installer::Compiled {
            binary,
            resource_root,
        } => {
            let output = Command::new(binary)
                .arg(subcommand)
                .arg("--resource-root")
                .arg(resource_root)
                .output()
                .with_context(|| {
                    format!("Failed to launch the compiled skill installer at {binary:?}")
                })?;
            parse_installer_output(output)
        }
        Installer::Script { path } => run_script_with_runtimes(path, subcommand),
    }
}

fn run_script_with_runtimes(path: &Path, subcommand: &str) -> Result<InstallOutcome> {
    let mut output = None;
    for runtime in INSTALLER_RUNTIMES {
        match Command::new(runtime).arg(path).arg(subcommand).output() {
            Ok(out) => {
                output = Some(out);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => anyhow::bail!("Failed to launch {runtime}: {e}"),
        }
    }

    let Some(output) = output else {
        anyhow::bail!(
            "Neither `bun` nor `node` was found on $PATH. One of them is required to install \
             NodeSpace's AI-agent integrations (Claude Code, Codex, Gemini CLI, OpenCode). \
             Install Node from https://nodejs.org (or Bun from https://bun.sh) and re-run."
        );
    };
    parse_installer_output(output)
}

/// Parse a finished installer invocation's exit status and stdout into
/// agent names, or an error. Close copy of `skill_setup.rs`'s
/// `parse_installer_output` -- see this module's doc comment for why it
/// isn't shared instead.
fn parse_installer_output(output: std::process::Output) -> Result<InstallOutcome> {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        let detail = if stderr.is_empty() { &stdout } else { &stderr };
        anyhow::bail!(
            "Skill installer exited with status {}: {}",
            output.status,
            detail.trim()
        );
    }

    fn agent_after_marker(line: &str, marker: char) -> Option<&str> {
        let rest = line.trim_start();
        let rest = rest.strip_prefix(marker)?.trim_start();
        let agent = rest.split(':').next()?.trim();
        if agent.is_empty() {
            None
        } else {
            Some(agent)
        }
    }

    let installed: Vec<String> = stdout
        .lines()
        .filter_map(|line| agent_after_marker(line, '✓'))
        .map(str::to_string)
        .collect();

    let skipped: Vec<SkippedAgent> = stdout
        .lines()
        .filter_map(|line| {
            let agent = agent_after_marker(line, '⚠')?;
            let reason = line
                .trim_start()
                .strip_prefix('⚠')?
                .trim_start()
                .split_once(':')?
                .1
                .trim();
            Some(SkippedAgent {
                agent: agent.to_string(),
                reason: reason.to_string(),
            })
        })
        .collect();

    Ok(InstallOutcome { installed, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_sidecar_name_is_platform_bare_on_unix() {
        if !cfg!(windows) {
            assert_eq!(
                bundled_sidecar_name("nodespace-skill-installer"),
                "nodespace-skill-installer"
            );
        }
    }

    #[test]
    fn compiled_installer_beside_is_none_when_the_sidecar_binary_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fake_exe = dir.path().join("nodespace");
        std::fs::write(&fake_exe, b"").expect("write fake exe");
        // No sidecar written -- resource root present would be irrelevant.
        std::fs::create_dir_all(dir.path().join("skill")).expect("mkdir skill");
        std::fs::write(dir.path().join("skill").join("SKILL.md"), b"").expect("write SKILL.md");

        assert!(compiled_installer_beside(&fake_exe).is_none());
    }

    #[test]
    fn compiled_installer_beside_is_none_when_the_resource_root_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fake_exe = dir.path().join("nodespace");
        std::fs::write(&fake_exe, b"").expect("write fake exe");
        std::fs::write(
            dir.path()
                .join(bundled_sidecar_name("nodespace-skill-installer")),
            b"",
        )
        .expect("write fake sidecar");
        // No skill/SKILL.md staged alongside it.

        assert!(compiled_installer_beside(&fake_exe).is_none());
    }

    #[test]
    fn compiled_installer_beside_resolves_both_pieces_when_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fake_exe = dir.path().join("nodespace");
        std::fs::write(&fake_exe, b"").expect("write fake exe");
        std::fs::write(
            dir.path()
                .join(bundled_sidecar_name("nodespace-skill-installer")),
            b"",
        )
        .expect("write fake sidecar");
        std::fs::create_dir_all(dir.path().join("skill")).expect("mkdir skill");
        std::fs::write(dir.path().join("skill").join("SKILL.md"), b"").expect("write SKILL.md");

        let installer = compiled_installer_beside(&fake_exe).expect("both pieces staged");
        match installer {
            Installer::Compiled {
                binary,
                resource_root,
            } => {
                assert_eq!(
                    binary,
                    dir.path()
                        .join(bundled_sidecar_name("nodespace-skill-installer"))
                );
                assert_eq!(resource_root, dir.path().join("skill"));
            }
            Installer::Script { .. } => panic!("expected a compiled installer to resolve"),
        }
    }

    /// Runs a throwaway shell script (the only way to get a real
    /// `std::process::Output` — `ExitStatus` has no public success
    /// constructor) that prints exactly the mixed stdout a real installer
    /// invocation can produce, mirroring `skill_setup.rs`'s own
    /// `fake_output` test fixture.
    fn fake_output(stdout_script: &str) -> std::process::Output {
        Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s\\n' {stdout_script}"))
            .output()
            .expect("sh must be available to run this test's fixture script")
    }

    #[test]
    fn parse_installer_output_captures_every_installed_agent() {
        let output = fake_output(
            "'✓ claude-code: installed 3 file(s)' '  → /fake/SKILL.md' '✓ codex: installed 2 file(s)'",
        );
        let outcome = parse_installer_output(output).expect("both agents installed cleanly");
        assert_eq!(outcome.installed, vec!["claude-code", "codex"]);
        assert!(outcome.skipped.is_empty());
    }

    #[test]
    fn parse_installer_output_captures_skipped_agents_with_reason() {
        let output = fake_output(
            "'⚠ claude-code: already installed via the Claude Code plugin marketplace, not overwriting'",
        );
        let outcome = parse_installer_output(output).expect("a skip is not an error");
        assert!(outcome.installed.is_empty());
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].agent, "claude-code");
        assert_eq!(
            outcome.skipped[0].reason,
            "already installed via the Claude Code plugin marketplace, not overwriting"
        );
    }

    #[test]
    fn parse_installer_output_errors_on_nonzero_exit() {
        let output = Command::new("sh")
            .arg("-c")
            .arg("echo 'boom' 1>&2; exit 1")
            .output()
            .expect("sh must be available to run this test's fixture script");
        let err =
            parse_installer_output(output).expect_err("a non-zero exit must surface as an error");
        assert!(err.to_string().contains("boom"));
    }

    #[test]
    fn resolve_script_installer_errors_with_actionable_message_when_missing() {
        // This crate's own checkout always has packages/skill as a sibling,
        // and CI never runs `bun run build:skill` before `cargo test`, so
        // dist/install.js genuinely does not exist at test time — exercising
        // the real not-found path rather than a synthetic one.
        let dist_install = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("skill")
            .join("dist")
            .join("install.js");
        if dist_install.exists() {
            return;
        }
        let err =
            resolve_script_installer().expect_err("dist/install.js is not built in this checkout");
        assert!(err.to_string().contains("build:skill"));
    }
}
