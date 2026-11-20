//! Running a program the way the runners need it: no shell, an environment that
//! is built (not inherited), a private working directory, its own process group
//! that is killed when the call ends or is cancelled, and credentials resolved
//! only into the child's environment.
//!
//! Shared by the `tool-cli` runner and the stdio transport of the `tool-mcp`
//! runner.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use nodespace_core::secrets::{resolve_credential_with, SecretStore};
use nodespace_types::{CredentialRef, EnvSecret};
use tokio::process::Command;
use zeroize::Zeroizing;

use super::redaction::{cut, MIN_REDACTABLE_BYTES};
use super::remote_tools::RemoteCallError;

/// How the runners read the daemon's environment: the base variables a program
/// is given, and `env:` credential references.
pub(crate) type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The daemon's variables a program is given, when they are set: enough to find
/// other programs and the user's home, and nothing that carries a credential.
pub const BASE_ENV: &[&str] = &["PATH", "HOME", "LANG", "USER", "TMPDIR"];

fn fail<T>(message: impl Into<String>) -> Result<T, RemoteCallError> {
    Err(RemoteCallError(message.into()))
}

/// Kills the program's process group when it is dropped: a call that is
/// cancelled (its future dropped mid-flight) must not leave the program or what
/// it started running. Disarmed once the program has been reaped, so a recycled
/// process id is never signalled.
pub(crate) struct GroupGuard(Option<i32>);

impl GroupGuard {
    pub(crate) fn new(child: &tokio::process::Child) -> Self {
        Self(child.id().map(|pid| pid as i32))
    }

    pub(crate) fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
        }
    }
}

/// A private, empty working directory for one call, removed afterwards.
pub(crate) struct ScratchDir(pub(crate) PathBuf);

impl ScratchDir {
    pub(crate) fn create() -> std::io::Result<Self> {
        let path =
            std::env::temp_dir().join(format!("nodespace-run-{}", uuid::Uuid::new_v4().simple()));
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder
        };
        #[cfg(not(unix))]
        let builder = std::fs::DirBuilder::new();
        builder.create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %self.0.display(),
                    error = %error.kind(),
                    "the working directory of a program run could not be removed"
                );
            }
        }
    }
}

/// How long to wait for a killed program to be reaped.
const REAP_BOUND: Duration = Duration::from_secs(2);

/// Kills the program and everything it started. The child leads its own process
/// group, so the group is killed; the child itself is killed as well, so a
/// failure to signal the group cannot leave it running.
pub(crate) fn stop(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::killpg(pid as i32, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

/// Waits a bounded time for a program that has been told to stop.
pub(crate) async fn reap(child: &mut tokio::process::Child) {
    let _ = tokio::time::timeout(REAP_BOUND, child.wait()).await;
}

/// A running program with everything that must be cleaned up with it. The
/// fields drop in this order: the guard kills the process group first, then the
/// child handle goes, then the working directory is removed.
pub(crate) struct Isolated {
    pub(crate) guard: GroupGuard,
    pub(crate) child: tokio::process::Child,
    _scratch: ScratchDir,
}

impl Isolated {
    /// The private working directory the program runs in.
    pub(crate) fn working_dir(&self) -> &std::path::Path {
        &self._scratch.0
    }
}

/// Starts `program` with `argv`: no shell, the environment cleared and rebuilt
/// from [`BASE_ENV`] (read through `base`) and `credentials`, a fresh private
/// working directory, its own process group, stdout and stderr piped and the
/// given stdin.
pub(crate) fn spawn_isolated(
    program: &str,
    argv: &[String],
    base: &EnvLookup,
    credentials: &[(String, Zeroizing<String>)],
    stdin: Stdio,
) -> Result<Isolated, RemoteCallError> {
    let scratch = ScratchDir::create().map_err(|e| {
        RemoteCallError(format!(
            "a working directory could not be made: {}",
            e.kind()
        ))
    })?;
    let child = {
        let mut command = Command::new(program);
        command
            .args(argv)
            .env_clear()
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .current_dir(&scratch.0);
        for name in BASE_ENV {
            if let Some(value) = base(name) {
                command.env(name, value);
            }
        }
        for (name, value) in credentials {
            command.env(name, value.as_str());
        }
        #[cfg(unix)]
        command.process_group(0);
        command.spawn().map_err(|e| {
            RemoteCallError(format!("the program could not be started: {}", e.kind()))
        })?
    };
    // Recorded so the daemon can tell this program from the user's applications
    // when something connects to its socket (see `crate::spawned`).
    if let Some(pid) = child.id() {
        // It was started with `process_group(0)`: it leads its own group.
        crate::spawned::record(pid, cfg!(unix));
    }
    let guard = GroupGuard::new(&child);
    Ok(Isolated {
        guard,
        child,
        _scratch: scratch,
    })
}

/// Resolves the credentials a binding declares as environment entries: the
/// (name, value) pairs for the child's environment. Runs the platform store off
/// the async threads; each value is read only here and refused if it is too
/// short to be redacted safely.
pub(crate) async fn resolve_env_credentials(
    secrets: &Arc<SecretStore>,
    lookup: &EnvLookup,
    entries: &[EnvSecret],
) -> Result<Vec<(String, Zeroizing<String>)>, RemoteCallError> {
    let mut out = Vec::new();
    for entry in entries {
        let reference = CredentialRef::parse(&entry.secret)
            .map_err(|e| RemoteCallError(format!("the tool's binding is not valid: {}", e.0)))?;
        let secrets = Arc::clone(secrets);
        let lookup = Arc::clone(lookup);
        let value = tokio::task::spawn_blocking(move || {
            resolve_credential_with(&secrets, &reference, |n| lookup(n))
        })
        .await
        .map_err(|_| RemoteCallError("the credential store task failed".to_string()))?
        .map_err(|e| RemoteCallError(e.to_string()))?;
        // The one place a value is read: it goes into the child's environment.
        let text = Zeroizing::new(value.expose().to_string());
        if text.len() < MIN_REDACTABLE_BYTES {
            return fail(format!(
                "the credential for `{}` is too short to be redacted safely",
                entry.name
            ));
        }
        out.push((entry.name.clone(), text));
    }
    Ok(out)
}

/// Most credential references a confirmation lists before saying how many more.
const MAX_ENV_SHOWN: usize = 8;
/// Longest one credential reference is shown.
const MAX_ENV_ENTRY_BYTES: usize = 300;
/// Longest program name or path shown.
const MAX_PROGRAM_SHOWN: usize = 512;
/// Longest resolved path shown.
const MAX_RESOLVED_SHOWN: usize = 160;

/// The credentials a program is given, as a confirmation says it: bounded (up
/// to eight references, then how many more there are, so none is hidden
/// silently) and meant to come before anything long.
pub(crate) fn environment_note(entries: &[EnvSecret]) -> String {
    let references: Vec<String> = entries
        .iter()
        .map(|e| {
            let mut entry = format!("{} from {}", e.name, e.secret);
            cut(&mut entry, MAX_ENV_ENTRY_BYTES);
            entry
        })
        .collect();
    let mut note = if references.is_empty() {
        "; no credentials".to_string()
    } else {
        format!(
            "; credentials in the environment: {}",
            references
                .iter()
                .take(MAX_ENV_SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if references.len() > MAX_ENV_SHOWN {
        note.push_str(&format!(", and {} more", references.len() - MAX_ENV_SHOWN));
    }
    note
}

/// The program as a confirmation says it: the name in backticks and, for a bare
/// name, where it resolves on the daemon's `PATH`.
pub(crate) fn program_note(program: &str, base: &EnvLookup) -> String {
    let mut shown = program.to_string();
    cut(&mut shown, MAX_PROGRAM_SHOWN);
    let resolved = if program.contains(['/', '\\']) {
        String::new()
    } else {
        match base("PATH") {
            None => " (the daemon's PATH is unset; the OS default applies)".to_string(),
            Some(path_var) => match which::which_in(program, Some(path_var), "/") {
                Ok(path) => {
                    let mut found = path.display().to_string();
                    if found.len() > MAX_RESOLVED_SHOWN {
                        cut(&mut found, MAX_RESOLVED_SHOWN);
                        found.push('\u{2026}');
                    }
                    format!(" (found on the daemon's PATH as {found})")
                }
                Err(_) => " (not found on the daemon's PATH)".to_string(),
            },
        }
    };
    format!("`{shown}`{resolved}")
}

/// Runs the credential lookup inside the call's time limit: a credential store
/// that never answers ends the call at its timeout instead of hanging it. Returns
/// the result and what is left of the limit.
pub(crate) async fn credentials_within<T>(
    limit: Duration,
    lookup: impl std::future::Future<Output = Result<T, RemoteCallError>>,
) -> Result<(T, Duration), RemoteCallError> {
    let started = std::time::Instant::now();
    match tokio::time::timeout(limit, lookup).await {
        Ok(found) => Ok((found?, limit.saturating_sub(started.elapsed()))),
        Err(_) => fail(format!(
            "the credential store did not answer within {} seconds",
            limit.as_secs()
        )),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Every program started for a tool is recorded, so the daemon can tell it
    /// from the user's applications when something connects to its socket.
    #[tokio::test]
    async fn a_program_started_through_spawn_isolated_is_recorded_as_a_tools() {
        let base: EnvLookup = Arc::new(|name| std::env::var(name).ok());
        let mut process = spawn_isolated("sleep", &["5".to_string()], &base, &[], Stdio::null())
            .expect("sleep starts");
        let pid = process.child.id().expect("a running child has an id");
        assert_eq!(crate::spawned::classify_peer(pid), Some(true));
        stop(&mut process.child);
        reap(&mut process.child).await;
    }
}
