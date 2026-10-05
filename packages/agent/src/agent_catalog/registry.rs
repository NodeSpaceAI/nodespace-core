//! PTY agent catalog (ADR-032).
//!
//! Hardcoded catalog of external agent CLIs that can be spawned in a PTY.
//! Each entry names the binary and the environment variables the agent reads
//! its credentials and configuration from.

use crate::agent_types::AgentType;

/// Static description of an external agent CLI spawned via PTY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDefinition {
    /// Stable identifier used to look the agent up.
    pub agent_type: AgentType,
    /// Human-readable display name.
    pub name: &'static str,
    /// Binary name to spawn, resolved on the search path
    /// [`crate::pty::detection`] builds.
    pub binary: &'static str,
    /// The environment variables this agent reads its credentials and its
    /// configuration directory from. PTY children get a cleared environment
    /// plus a fixed base allowlist plus these, never the daemon's full
    /// environment (see [`crate::pty::session`]).
    pub env_vars: &'static [&'static str],
}

/// Hardcoded catalog of PTY-spawnable external agents.
pub const AGENT_CATALOG: &[AgentDefinition] = &[
    AgentDefinition {
        agent_type: AgentType::ClaudeCode,
        name: "Claude Code",
        binary: "claude",
        // `CLAUDE_CONFIG_DIR` selects the profile the skill installer wrote
        // the plugin to, so the session loads the same one.
        env_vars: &[
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CONFIG_DIR",
        ],
    },
    AgentDefinition {
        agent_type: AgentType::Codex,
        name: "Codex",
        binary: "codex",
        env_vars: &["OPENAI_API_KEY"],
    },
    AgentDefinition {
        agent_type: AgentType::Antigravity,
        name: "Antigravity CLI",
        binary: "agy",
        env_vars: &["GEMINI_API_KEY"],
    },
    AgentDefinition {
        agent_type: AgentType::Pi,
        name: "Pi",
        binary: "pi",
        // Pi is multi-provider; pass through every provider key it may need.
        env_vars: &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
        ],
    },
    AgentDefinition {
        agent_type: AgentType::OpenCode,
        name: "OpenCode",
        binary: "opencode",
        env_vars: &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
        ],
    },
];

/// In-process catalog handle. Stateless and cheap to construct.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemAgentRegistry;

impl SystemAgentRegistry {
    pub const fn new() -> Self {
        Self
    }

    /// Return every known agent definition.
    pub fn all(&self) -> &'static [AgentDefinition] {
        AGENT_CATALOG
    }

    /// Look up a definition by [`AgentType`].
    pub fn get(&self, agent_type: AgentType) -> Option<&'static AgentDefinition> {
        AGENT_CATALOG.iter().find(|d| d.agent_type == agent_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_holds_every_agent_once_in_order() {
        let types: Vec<AgentType> = AGENT_CATALOG.iter().map(|d| d.agent_type).collect();
        assert_eq!(types, AgentType::ALL);
    }

    /// One vocabulary: an agent's id is its serde form, the form the skill
    /// installer and the CLI use.
    #[test]
    fn an_agents_id_is_its_serde_form() {
        for agent in AgentType::ALL {
            assert_eq!(serde_json::json!(agent), serde_json::json!(agent.id()));
            assert_eq!(AgentType::from_id(agent.id()), Some(agent));
        }
        assert_eq!(
            AgentType::ALL.map(AgentType::id),
            ["claude-code", "codex", "antigravity", "pi", "opencode"]
        );
        assert_eq!(AgentType::from_id("open-code"), None);
        assert_eq!(AgentType::from_id("antigravity-cli"), None);
    }

    /// The skill installer names the agents it installs into by the same ids.
    #[test]
    fn the_skill_installer_names_agents_by_the_same_ids() {
        let types = include_str!("../../../skill/src/types.ts");
        let declaration = types
            .lines()
            .find(|line| line.starts_with("export type AgentName"))
            .expect("the installer declares AgentName");
        let mut names: Vec<&str> = declaration.split('\'').skip(1).step_by(2).collect();
        names.sort_unstable();
        let mut ids = AgentType::ALL.map(AgentType::id).to_vec();
        ids.sort_unstable();
        assert_eq!(names, ids);
    }

    #[test]
    fn every_catalog_entry_declares_at_least_one_env_var() {
        for def in SystemAgentRegistry::new().all() {
            assert!(
                !def.env_vars.is_empty(),
                "{:?} should declare env vars for the PTY allowlist",
                def.agent_type
            );
        }
    }

    #[test]
    fn claude_code_gets_its_credentials_and_its_config_directory() {
        let def = SystemAgentRegistry::new()
            .get(AgentType::ClaudeCode)
            .unwrap();
        assert_eq!(def.binary, "claude");
        for var in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CONFIG_DIR",
        ] {
            assert!(def.env_vars.contains(&var), "{var}");
        }
    }
}
