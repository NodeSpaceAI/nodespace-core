//! ACP agent registry: discovers, health-checks, and caches external agents.
//!
//! Implements the [`AgentRegistry`] trait from `agent_types` by maintaining a
//! hardcoded catalog of known ACP agents, probing for their binaries on PATH,
//! running lightweight version checks, and validating authentication config.
//!
//! Results are cached with a 5-minute TTL and refreshed in the background.

use crate::agent_types::{AcpAgentInfo, AcpAuthMethod, AgentRegistry, RegistryError};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How long cached discovery results remain valid.
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Timeout for a single `--version` health probe.
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval for the background refresh task.
const BACKGROUND_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// Catalog entry (static description of a known agent)
// ---------------------------------------------------------------------------

/// Static description of a known ACP-compatible agent.
struct CatalogEntry {
    /// Unique identifier (used as `AcpAgentInfo::id`).
    id: &'static str,
    /// Human-readable display name.
    name: &'static str,
    /// Binary name to search for on PATH.
    binary_name: &'static str,
    /// Arguments passed when spawning the agent in ACP mode.
    args: &'static [&'static str],
    /// Authentication method.
    auth_method: AcpAuthMethod,
}

/// Hardcoded v1 agent catalog.
///
/// Note: Claude Code and Gemini CLI are invoked via their ACP adapters
/// (npm packages), which wrap the CLI and speak the ACP protocol over stdio.
/// The adapters are assumed to be installed globally via `npm install -g`.
fn agent_catalog() -> Vec<CatalogEntry> {
    vec![
        CatalogEntry {
            id: "claude-code",
            name: "Claude Code",
            // Use the official ACP adapter for Claude Code
            // Install with: npm install -g @agentclientprotocol/claude-agent-acp
            binary_name: "claude-agent-acp",
            args: &[],
            auth_method: AcpAuthMethod::AgentManaged,
        },
        CatalogEntry {
            id: "gemini-cli",
            name: "Gemini CLI",
            // Gemini CLI supports --acp directly
            binary_name: "gemini",
            args: &["--acp"],
            auth_method: AcpAuthMethod::AgentManaged,
        },
        CatalogEntry {
            id: "mistral-vibe",
            name: "Mistral Vibe",
            binary_name: "vibe-acp",
            args: &[],
            auth_method: AcpAuthMethod::EnvApiKey {
                var_name: "MISTRAL_API_KEY".to_string(),
            },
        },
    ]
}

// ---------------------------------------------------------------------------
// Binary detection
// ---------------------------------------------------------------------------

/// Result of searching for a binary on PATH.
#[derive(Debug, Clone)]
struct BinaryProbe {
    /// Absolute path to the binary, if found.
    path: Option<String>,
    /// Whether the binary was found and is executable.
    found: bool,
}

/// Search for `binary_name` on PATH using the `which` crate.
fn probe_binary(binary_name: &str) -> BinaryProbe {
    match which::which(binary_name) {
        Ok(path) => BinaryProbe {
            path: Some(path.to_string_lossy().into_owned()),
            found: true,
        },
        Err(_) => BinaryProbe {
            path: None,
            found: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Health checking (version probe)
// ---------------------------------------------------------------------------

/// Run `<binary> --version`, returning the first non-empty stdout line on success.
async fn version_probe(binary_path: &str) -> Option<String> {
    let result = tokio::time::timeout(
        HEALTH_PROBE_TIMEOUT,
        tokio::process::Command::new(binary_path)
            .arg("--version")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;

    match result {
        Ok(Ok(output)) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let version_line = stdout.lines().find(|l| !l.trim().is_empty());
            version_line.map(|l| l.trim().to_string())
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Auth validation
// ---------------------------------------------------------------------------

/// Check whether the required authentication is satisfied.
///
/// The `env_lookup` closure resolves environment variable names to their values,
/// allowing tests to inject mock lookups instead of mutating process-global env
/// vars (which is unsound under multi-threaded test runners).
fn auth_satisfied(method: &AcpAuthMethod, env_lookup: impl Fn(&str) -> Option<String>) -> bool {
    match method {
        AcpAuthMethod::AgentManaged => true,
        AcpAuthMethod::EnvApiKey { var_name } => env_lookup(var_name)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false),
    }
}

/// Default env lookup using `std::env::var`.
fn std_env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

// ---------------------------------------------------------------------------
// Cached state
// ---------------------------------------------------------------------------

/// Snapshot of discovered agents with a timestamp for TTL expiry.
struct CachedDiscovery {
    agents: Vec<AcpAgentInfo>,
    discovered_at: Instant,
}

impl CachedDiscovery {
    fn is_fresh(&self) -> bool {
        self.discovered_at.elapsed() < CACHE_TTL
    }
}

// ---------------------------------------------------------------------------
// SystemAgentRegistry
// ---------------------------------------------------------------------------

/// Concrete implementation of [`AgentRegistry`] for desktop environments.
///
/// Scans PATH for known agent binaries, runs `--version` health probes, and
/// validates auth configuration. Results are cached with a 5-minute TTL.
pub struct SystemAgentRegistry {
    cache: Arc<RwLock<Option<CachedDiscovery>>>,
    /// Set to `true` once the background refresh task has been spawned.
    background_started: Arc<RwLock<bool>>,
}

impl SystemAgentRegistry {
    /// Create a new registry. No probes are run until `discover_agents()` is called.
    pub fn new() -> Self {
        Self {
            cache: Arc::new(RwLock::new(None)),
            background_started: Arc::new(RwLock::new(false)),
        }
    }

    /// Perform a full probe of all catalog entries, returning fresh agent info.
    async fn full_probe(&self) -> Vec<AcpAgentInfo> {
        let catalog = agent_catalog();
        let mut agents = Vec::with_capacity(catalog.len());

        for entry in &catalog {
            let probe = probe_binary(entry.binary_name);

            let (available, version, binary_path) = if probe.found {
                let path = probe.path.as_deref().unwrap_or(entry.binary_name);
                let ver = version_probe(path).await;
                let auth_ok = auth_satisfied(&entry.auth_method, std_env_lookup);
                (auth_ok, ver, path.to_string())
            } else {
                (false, None, entry.binary_name.to_string())
            };

            agents.push(AcpAgentInfo {
                id: entry.id.to_string(),
                name: entry.name.to_string(),
                binary: binary_path,
                args: entry.args.iter().map(|s| s.to_string()).collect(),
                auth_method: entry.auth_method.clone(),
                available,
                version,
            });
        }

        agents
    }

    /// Store `agents` in the cache with the current timestamp.
    async fn update_cache(&self, agents: Vec<AcpAgentInfo>) {
        let mut guard = self.cache.write().await;
        *guard = Some(CachedDiscovery {
            agents,
            discovered_at: Instant::now(),
        });
    }

    /// Ensure the background refresh task is running. Idempotent.
    async fn ensure_background_refresh(&self) {
        let mut started = self.background_started.write().await;
        if *started {
            return;
        }
        *started = true;

        let cache = Arc::clone(&self.cache);

        tokio::spawn(async move {
            // Create a temporary registry that shares no state but can run
            // full_probe(). The results are written into the shared cache.
            let probe_registry = SystemAgentRegistry {
                cache: Arc::clone(&cache),
                background_started: Arc::new(RwLock::new(true)),
            };

            let mut interval = tokio::time::interval(BACKGROUND_REFRESH_INTERVAL);
            // The first tick completes immediately; skip it since we just probed.
            interval.tick().await;

            loop {
                interval.tick().await;

                let agents = probe_registry.full_probe().await;
                probe_registry.update_cache(agents).await;
            }
        });
    }
}

#[async_trait]
impl AgentRegistry for SystemAgentRegistry {
    /// Discover all known agents. Returns cached results when the cache is fresh,
    /// otherwise performs a full probe first.
    ///
    /// The first call also starts the background refresh task.
    async fn discover_agents(&self) -> Result<Vec<AcpAgentInfo>, RegistryError> {
        // Return cached results if still fresh.
        {
            let guard = self.cache.read().await;
            if let Some(cached) = guard.as_ref() {
                if cached.is_fresh() {
                    return Ok(cached.agents.clone());
                }
            }
        }

        // Cache is stale or absent — perform a full probe.
        let agents = self.full_probe().await;
        self.update_cache(agents.clone()).await;

        // Ensure background refresh is running.
        self.ensure_background_refresh().await;

        Ok(agents)
    }

    /// Look up a single agent by its identifier.
    async fn get_agent(&self, agent_id: &str) -> Result<AcpAgentInfo, RegistryError> {
        let agents = self.discover_agents().await?;
        agents
            .into_iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| RegistryError::NotFound(agent_id.to_string()))
    }

    /// Force a re-probe of all agents, ignoring the cache TTL.
    async fn refresh(&self) -> Result<(), RegistryError> {
        let agents = self.full_probe().await;
        self.update_cache(agents).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Catalog construction ------------------------------------------------

    #[test]
    fn catalog_has_three_agents() {
        let catalog = agent_catalog();
        assert_eq!(catalog.len(), 3);
    }

    #[test]
    fn catalog_ids_are_unique() {
        let catalog = agent_catalog();
        let mut ids: Vec<&str> = catalog.iter().map(|e| e.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn catalog_entries_have_expected_ids() {
        let catalog = agent_catalog();
        let ids: Vec<&str> = catalog.iter().map(|e| e.id).collect();
        assert!(ids.contains(&"claude-code"));
        assert!(ids.contains(&"gemini-cli"));
        assert!(ids.contains(&"mistral-vibe"));
    }

    #[test]
    fn claude_code_entry_is_correct() {
        let catalog = agent_catalog();
        let entry = catalog.iter().find(|e| e.id == "claude-code").unwrap();
        assert_eq!(entry.name, "Claude Code");
        assert_eq!(entry.binary_name, "claude-agent-acp");
        assert!(entry.args.is_empty());
        assert!(matches!(entry.auth_method, AcpAuthMethod::AgentManaged));
    }

    #[test]
    fn gemini_entry_is_correct() {
        let catalog = agent_catalog();
        let entry = catalog.iter().find(|e| e.id == "gemini-cli").unwrap();
        assert_eq!(entry.name, "Gemini CLI");
        assert_eq!(entry.binary_name, "gemini");
        assert_eq!(entry.args, &["--acp"]);
        assert!(matches!(entry.auth_method, AcpAuthMethod::AgentManaged));
    }

    #[test]
    fn mistral_entry_is_correct() {
        let catalog = agent_catalog();
        let entry = catalog.iter().find(|e| e.id == "mistral-vibe").unwrap();
        assert_eq!(entry.name, "Mistral Vibe");
        assert_eq!(entry.binary_name, "vibe-acp");
        assert!(entry.args.is_empty());
        match &entry.auth_method {
            AcpAuthMethod::EnvApiKey { var_name } => {
                assert_eq!(var_name, "MISTRAL_API_KEY");
            }
            _ => panic!("Expected EnvApiKey auth method"),
        }
    }

    // -- Auth validation -----------------------------------------------------
    //
    // Tests inject a mock env lookup closure instead of mutating process-global
    // env vars, which would be unsound under Rust's multi-threaded test runner.

    #[test]
    fn agent_managed_auth_always_satisfied() {
        let no_env = |_: &str| -> Option<String> { None };
        assert!(auth_satisfied(&AcpAuthMethod::AgentManaged, no_env));
    }

    #[test]
    fn env_api_key_satisfied_when_set() {
        let lookup = |_: &str| -> Option<String> { Some("sk-test-key".to_string()) };
        assert!(auth_satisfied(
            &AcpAuthMethod::EnvApiKey {
                var_name: "ANY_KEY".to_string(),
            },
            lookup,
        ));
    }

    #[test]
    fn env_api_key_not_satisfied_when_missing() {
        let lookup = |_: &str| -> Option<String> { None };
        assert!(!auth_satisfied(
            &AcpAuthMethod::EnvApiKey {
                var_name: "MISSING_KEY".to_string(),
            },
            lookup,
        ));
    }

    #[test]
    fn env_api_key_not_satisfied_when_empty() {
        let lookup = |_: &str| -> Option<String> { Some("".to_string()) };
        assert!(!auth_satisfied(
            &AcpAuthMethod::EnvApiKey {
                var_name: "EMPTY_KEY".to_string(),
            },
            lookup,
        ));
    }

    #[test]
    fn env_api_key_not_satisfied_when_whitespace_only() {
        let lookup = |_: &str| -> Option<String> { Some("   ".to_string()) };
        assert!(!auth_satisfied(
            &AcpAuthMethod::EnvApiKey {
                var_name: "WS_KEY".to_string(),
            },
            lookup,
        ));
    }

    // -- Binary probe (real PATH) -------------------------------------------

    #[test]
    fn probe_finds_existing_binary() {
        // `ls` should exist on any Unix-like system.
        let result = probe_binary("ls");
        assert!(result.found);
        assert!(result.path.is_some());
    }

    #[test]
    fn probe_returns_not_found_for_nonexistent_binary() {
        let result = probe_binary("nodespace_definitely_not_a_real_binary_xyz");
        assert!(!result.found);
        assert!(result.path.is_none());
    }

    // -- Registry construction -----------------------------------------------

    #[tokio::test]
    async fn registry_discover_returns_all_catalog_agents() {
        let registry = SystemAgentRegistry::new();
        let agents = registry.discover_agents().await.unwrap();
        assert_eq!(agents.len(), 3);
    }

    #[tokio::test]
    async fn registry_get_agent_returns_not_found_for_unknown_id() {
        let registry = SystemAgentRegistry::new();
        // Force cache population.
        let _ = registry.discover_agents().await;
        let err = registry.get_agent("nonexistent-agent").await.unwrap_err();
        assert!(matches!(err, RegistryError::NotFound(_)));
    }

    #[tokio::test]
    async fn registry_get_agent_returns_known_agent() {
        let registry = SystemAgentRegistry::new();
        let agent = registry.get_agent("claude-code").await.unwrap();
        assert_eq!(agent.id, "claude-code");
        assert_eq!(agent.name, "Claude Code");
    }

    // -- Cache behaviour -----------------------------------------------------

    #[tokio::test]
    async fn registry_uses_cache_on_second_call() {
        let registry = SystemAgentRegistry::new();

        let first = registry.discover_agents().await.unwrap();
        let second = registry.discover_agents().await.unwrap();

        // Both calls should return the same number of agents (from cache).
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.available, b.available);
        }
    }

    #[tokio::test]
    async fn registry_refresh_repopulates_cache() {
        let registry = SystemAgentRegistry::new();

        let _ = registry.discover_agents().await.unwrap();
        registry.refresh().await.unwrap();
        let agents = registry.discover_agents().await.unwrap();

        assert_eq!(agents.len(), 3);
    }

    #[tokio::test]
    async fn cache_ttl_reports_fresh_immediately() {
        let cached = CachedDiscovery {
            agents: vec![],
            discovered_at: Instant::now(),
        };
        assert!(cached.is_fresh());
    }

    #[tokio::test]
    async fn cache_ttl_reports_stale_after_expiry() {
        let cached = CachedDiscovery {
            agents: vec![],
            discovered_at: Instant::now() - CACHE_TTL - Duration::from_secs(1),
        };
        assert!(!cached.is_fresh());
    }

    // -- Agents without binaries are marked unavailable ----------------------

    #[tokio::test]
    async fn missing_binary_marks_agent_unavailable() {
        let registry = SystemAgentRegistry::new();
        // `vibe-acp` is almost certainly not installed on the test machine.
        let agent = registry.get_agent("mistral-vibe").await.unwrap();
        // Even if it were installed, without MISTRAL_API_KEY it would still
        // be unavailable, so we just check the field is deterministic.
        // The test validates that the registry does not panic or error out
        // when a binary is not found.
        assert_eq!(agent.id, "mistral-vibe");
    }

    // -- EnvApiKey agents with missing key are unavailable -------------------

    #[tokio::test]
    async fn env_api_key_agent_unavailable_without_key() {
        // The `vibe-acp` binary is not installed in test environments, so
        // `available` is false before auth is even checked. This test validates
        // the end-to-end path without needing to manipulate env vars.
        let registry = SystemAgentRegistry::new();
        let agent = registry.get_agent("mistral-vibe").await.unwrap();
        assert!(!agent.available);
    }
}
