//! What it takes to start an ACP agent: a descriptor with no shell syntax in it.

use nodespace_types::EnvSecret;

use super::wire::AcpError;

/// Most arguments, environment entries and credential references one launch has.
const MAX_ENTRIES: usize = 64;
/// Longest program path, argument or environment value.
const MAX_FIELD_BYTES: usize = 8192;

/// How an ACP agent is started. The program is run directly (never through a
/// shell). Its environment is not inherited: it is the base variables of the
/// shared process helper, the declared literal entries, the named daemon
/// variables that are set, and the declared credentials.
#[derive(Debug, Clone, Default)]
pub struct AcpLaunch {
    /// The program: an absolute path, or a name found on the daemon's `PATH`.
    pub program: String,
    pub args: Vec<String>,
    /// Environment entries with fixed values.
    pub literal_env: Vec<(String, String)>,
    /// Names of daemon variables passed through when set (beyond the base set).
    pub daemon_env: Vec<String>,
    /// Environment entries whose values are credentials, read from the secret
    /// store inside the startup time limit.
    pub secrets: Vec<EnvSecret>,
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 128
}

impl AcpLaunch {
    /// Refuses a descriptor that could not be run as written.
    pub(crate) fn validate(&self) -> Result<(), AcpError> {
        let bad = |m: &str| Err(AcpError::Launch(m.to_string()));
        if self.program.is_empty() || self.program.contains('\0') {
            return bad("the program is empty or has a NUL byte");
        }
        if self.program.len() > MAX_FIELD_BYTES
            || self.args.len() > MAX_ENTRIES
            || self.literal_env.len() > MAX_ENTRIES
            || self.daemon_env.len() > MAX_ENTRIES
            || self.secrets.len() > MAX_ENTRIES
        {
            return bad("the launch descriptor is too large");
        }
        if self
            .args
            .iter()
            .any(|a| a.contains('\0') || a.len() > MAX_FIELD_BYTES)
        {
            return bad("an argument is too long or has a NUL byte");
        }
        let names = self
            .literal_env
            .iter()
            .map(|(n, _)| n.as_str())
            .chain(self.daemon_env.iter().map(String::as_str))
            .chain(self.secrets.iter().map(|s| s.name.as_str()));
        let mut seen = std::collections::HashSet::new();
        for name in names {
            if !valid_env_name(name) {
                return bad("an environment name is not valid");
            }
            if crate::local_agent::process::BASE_ENV.contains(&name) || !seen.insert(name) {
                return bad("an environment name is repeated or is one of the base variables");
            }
        }
        if self
            .literal_env
            .iter()
            .any(|(_, v)| v.contains('\0') || v.len() > MAX_FIELD_BYTES)
        {
            return bad("an environment value is too long or has a NUL byte");
        }
        Ok(())
    }
}
