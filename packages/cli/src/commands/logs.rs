//! `nodespace logs` — read the daemon's log.
//!
//! Play execution errors (a failed action, a cycle-limit breach, a rule that
//! would not compile) are operational telemetry rather than knowledge, so they
//! go to the daemon log rather than becoming nodes. This is how you read them
//! back.
//!
//! Local-only: the log is a file on this machine, written by whichever
//! supervisor started the daemon, so there is nothing to ask the daemon for —
//! and asking would fail in exactly the case you most want the log, namely a
//! daemon that will not start.
//!
//! The path differs by install method, which is the whole reason this verb
//! exists: telling a user to `grep ~/.nodespace/logs/...` is wrong for a
//! Homebrew install, and a caller has no reliable way to know which they have.

use anyhow::{bail, Result};
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct LogsArgs {
    /// Show only lines containing this text — a play id, a rule name, an
    /// error type. Matched literally, not as a regex.
    #[arg(long)]
    pub filter: Option<String>,

    /// How many matching lines to show, most recent last.
    #[arg(long, default_value_t = 50)]
    pub lines: usize,

    /// Print the resolved log file path and exit without reading it.
    #[arg(long)]
    pub path_only: bool,
}

/// Candidate log locations, in the order they are checked.
///
/// The desktop app's supervisor writes under the user's home; a Homebrew
/// service writes under the prefix. Both are real and neither is discoverable
/// from the other, so the resolution is "first one that exists".
fn candidate_paths() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // $HOME rather than a `dirs` dependency, matching `uninstall.rs`'s own
    // home resolution in this crate.
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".nodespace")
                .join("logs")
                .join("nodespaced.log"),
        );
    }

    // Homebrew's `var` lives under the prefix, which differs by architecture.
    for prefix in ["/opt/homebrew", "/usr/local"] {
        candidates.push(
            PathBuf::from(prefix)
                .join("var")
                .join("log")
                .join("nodespace")
                .join("nodespaced.log"),
        );
    }

    candidates
}

fn resolve_log_path() -> Option<PathBuf> {
    candidate_paths().into_iter().find(|p| p.exists())
}

pub fn run(args: LogsArgs, json: bool) -> Result<()> {
    let Some(path) = resolve_log_path() else {
        let looked = candidate_paths()
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "No daemon log found. Looked in:\n{looked}\n\n\
             A daemon that has never started writes no log — check `nodespace diagnostics` first."
        );
    };

    if args.path_only {
        if json {
            println!(
                "{}",
                serde_json::json!({ "path": path.display().to_string() })
            );
        } else {
            println!("{}", path.display());
        }
        return Ok(());
    }

    let body = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Could not read {}: {e}", path.display()))?;

    let matched: Vec<&str> = body
        .lines()
        .filter(|line| match &args.filter {
            Some(f) => line.contains(f.as_str()),
            None => true,
        })
        .collect();

    // Most recent last, so the tail of the output is the newest — the same way
    // reading a log file normally goes.
    let shown = matched
        .iter()
        .rev()
        .take(args.lines)
        .rev()
        .copied()
        .collect::<Vec<_>>();

    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": path.display().to_string(),
                "matched": matched.len(),
                "shown": shown.len(),
                "lines": shown,
            })
        );
    } else {
        for line in &shown {
            println!("{line}");
        }
        if matched.len() > shown.len() {
            eprintln!(
                "\n({} of {} matching lines shown — raise --lines for more)",
                shown.len(),
                matched.len()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both install layouts are checked, or the verb reintroduces exactly the
    /// wrong-path problem it exists to remove.
    #[test]
    fn candidates_cover_both_install_layouts() {
        let paths: Vec<String> = candidate_paths()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        let joined = paths.join("\n");

        assert!(
            joined.contains(".nodespace/logs/nodespaced.log"),
            "the desktop app's log path must be a candidate: {joined}"
        );
        assert!(
            joined.contains("var/log/nodespace/nodespaced.log"),
            "a Homebrew service's log path must be a candidate: {joined}"
        );
    }
}
