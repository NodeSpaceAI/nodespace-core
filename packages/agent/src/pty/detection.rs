//! Binary and auth detection for PTY agent CLIs.
//!
//! `detect_all_agents()` iterates the static AGENT_CATALOG and checks two
//! things per agent: (1) the binary is reachable on [`agent_search_path`], and
//! (2) the user has configured an auth credential.
//!
//! PATH augmentation runs `/usr/libexec/path_helper` on macOS first so that
//! shell-configured PATH entries (e.g. Homebrew, nvm) are visible even when
//! the daemon was launched outside a login shell.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::agent_catalog::registry::AGENT_CATALOG;
use crate::agent_types::AgentType;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Per-agent availability status returned by `detect_all_agents`.
#[derive(Debug, Clone)]
pub struct AgentAvailability {
    pub agent_type: AgentType,
    /// Binary name (e.g. "claude").
    pub binary: &'static str,
    /// `true` when the binary was found on the augmented PATH.
    pub binary_found: bool,
    /// `true` when an auth credential (env var or config file) was found.
    pub auth_found: bool,
    /// Absolute path to the binary, when found.
    pub binary_path: Option<PathBuf>,
    /// Human-readable install hint shown in the UI when `binary_found` is `false`.
    pub install_hint: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Run binary and auth checks for every agent in the catalog, looking for
/// each binary on [`agent_search_path`].
pub fn detect_all_agents() -> Vec<AgentAvailability> {
    detect_all_agents_on(&agent_search_path())
}

/// [`detect_all_agents`] on a given search path: the one a launch will then
/// resolve the binary on.
pub fn detect_all_agents_on(path: &OsString) -> Vec<AgentAvailability> {
    AGENT_CATALOG
        .iter()
        .map(|def| detect_one(def.agent_type, def.binary, path))
        .collect()
}

// ---------------------------------------------------------------------------
// Per-agent detection
// ---------------------------------------------------------------------------

/// Resolve `binary` on `path`, the one way detection and launch both do.
pub fn resolve_binary(binary: &str, path: &OsString) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    which::which_in(binary, Some(path), &cwd).ok()
}

fn detect_one(agent_type: AgentType, binary: &'static str, path: &OsString) -> AgentAvailability {
    let binary_path = resolve_binary(binary, path);
    let binary_found = binary_path.is_some();
    let install_hint = if binary_found {
        None
    } else {
        Some(install_hint_for(agent_type))
    };

    AgentAvailability {
        agent_type,
        binary,
        binary_found,
        auth_found: true,
        binary_path,
        install_hint,
    }
}

// ---------------------------------------------------------------------------
// Install hints
// ---------------------------------------------------------------------------

fn install_hint_for(agent_type: AgentType) -> &'static str {
    match agent_type {
        AgentType::ClaudeCode => {
            "npm install -g @anthropic-ai/claude-code — https://claude.ai/code"
        }
        AgentType::Codex => "npm install -g @openai/codex — https://openai.com/codex",
        AgentType::Antigravity => "curl -fsSL https://antigravity.google/cli/install.sh | bash",
        AgentType::Pi => "https://pi.dev",
        AgentType::OpenCode => "https://opencode.ai",
    }
}

// ---------------------------------------------------------------------------
// PATH augmentation
// ---------------------------------------------------------------------------

/// The search path agent binaries are found on: common user-level install
/// locations and, on macOS, the system-managed entries from
/// `/usr/libexec/path_helper`, ahead of the daemon's own `PATH`.
///
/// Detection and launch both resolve on it, and a launched session gets it as
/// its `PATH`, so a binary detection found is one launch can start and one
/// the session itself can run.
///
/// Spawns a process on macOS: call it off the async executor.
pub fn agent_search_path() -> OsString {
    let mut extra: Vec<PathBuf> = Vec::new();

    // macOS: ask path_helper for the shell-configured PATH.
    #[cfg(target_os = "macos")]
    {
        if let Some(shell_path) = path_helper_output() {
            extra.extend(std::env::split_paths(&shell_path));
        }
    }

    if let Some(home) = dirs::home_dir() {
        extra.push(home.join(".npm-global").join("bin"));
        extra.push(home.join(".local").join("bin"));
        extra.push(home.join(".cargo").join("bin"));
        extra.push(home.join("go").join("bin"));
    }
    extra.push(PathBuf::from("/opt/homebrew/bin"));
    extra.push(PathBuf::from("/usr/local/bin"));

    // Append existing PATH so user's configured entries are still searched.
    let existing = std::env::var_os("PATH").unwrap_or_default();
    extra.extend(std::env::split_paths(&existing));

    std::env::join_paths(extra).unwrap_or(existing)
}

/// Run `/usr/libexec/path_helper -s` and extract the PATH value from its output.
///
/// Output looks like: `PATH="/usr/local/bin:/usr/bin:/bin"; export PATH;`
#[cfg(target_os = "macos")]
fn path_helper_output() -> Option<OsString> {
    let output = std::process::Command::new("/usr/libexec/path_helper")
        .arg("-s")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    // Find the PATH= line and extract the quoted value defensively using
    // strip_prefix/strip_suffix rather than positional find('"') so that
    // unexpected whitespace or ordering doesn't silently yield a wrong path.
    let path_line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with("PATH="))?;
    let after_prefix = path_line.trim_start().strip_prefix("PATH=\"")?;
    // Accept either `"…"; export PATH;` or a bare closing quote.
    let value = after_prefix
        .strip_suffix("\"; export PATH;")
        .or_else(|| after_prefix.strip_suffix('"'))?;
    Some(OsString::from(value))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_all_returns_five_entries() {
        let results = detect_all_agents();
        assert_eq!(results.len(), AGENT_CATALOG.len());
    }

    #[test]
    fn binary_found_implies_path_present() {
        for av in detect_all_agents() {
            if av.binary_found {
                assert!(
                    av.binary_path.is_some(),
                    "{}: binary_found but binary_path is None",
                    av.binary
                );
            }
        }
    }

    #[test]
    fn install_hint_absent_when_binary_found() {
        for av in detect_all_agents() {
            if av.binary_found {
                assert!(
                    av.install_hint.is_none(),
                    "{}: binary_found but install_hint is still set",
                    av.binary
                );
            } else {
                assert!(
                    av.install_hint.is_some(),
                    "{}: binary missing but no install_hint",
                    av.binary
                );
            }
        }
    }

    #[test]
    fn the_search_path_is_non_empty() {
        let p = agent_search_path();
        assert!(!p.is_empty());
    }

    #[test]
    fn agent_types_match_catalog_order() {
        let results = detect_all_agents();
        let expected = [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::Antigravity,
            AgentType::Pi,
            AgentType::OpenCode,
        ];
        for (av, expected_type) in results.iter().zip(expected.iter()) {
            assert_eq!(av.agent_type, *expected_type);
        }
    }
}
