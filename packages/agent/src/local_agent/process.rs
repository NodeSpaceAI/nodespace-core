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

/// Largest program file an approval hashes.
pub(crate) const MAX_PROGRAM_BYTES: u64 = 256 * 1024 * 1024;

/// SHA-256 of the file at `path` (following a symbolic link), as lowercase
/// hex: what an approval pins for a program. Only a regular file of at most
/// [`MAX_PROGRAM_BYTES`] is hashed: it is opened without waiting (a FIFO does
/// not block the thread), checked through the open file (a device, a FIFO or
/// a directory is refused, and so is a file that is too large), and read as a
/// stream from that same open file, never past the limit.
pub(crate) fn file_sha256(path: &str) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let refuse = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, why.to_string());
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(refuse("not a regular file"));
    }
    if meta.len() > MAX_PROGRAM_BYTES {
        return Err(refuse("larger than a program file may be"));
    }
    let mut hasher = Sha256::new();
    let mut reader = file.take(MAX_PROGRAM_BYTES + 1);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut read = 0u64;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        read += n as u64;
        if read > MAX_PROGRAM_BYTES {
            return Err(refuse("larger than a program file may be"));
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// What a program file says about how far its pin holds: whether programs
/// running as this user can replace it (the file or its folder is writable by
/// this user), and, for a script that starts with `#!/usr/bin/env X`, the
/// program `X` it looks up on the `PATH` at each start (not pinned).
pub(crate) fn program_facts(path: &str) -> (bool, Option<String>) {
    let writable = |p: &std::path::Path| {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            std::ffi::CString::new(p.as_os_str().as_bytes())
                .map(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
                .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = p;
            false
        }
    };
    let file = std::path::Path::new(path);
    let replaceable = writable(file) || file.parent().is_some_and(writable);
    let mut head = [0u8; 256];
    let read = std::fs::File::open(file)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .unwrap_or(0);
    let first_line = String::from_utf8_lossy(&head[..read]);
    let first_line = first_line.lines().next().unwrap_or("");
    let from_path = first_line
        .strip_prefix("#!")
        .map(str::trim)
        .and_then(|line| {
            let mut words = line.split_whitespace();
            (words.next() == Some("/usr/bin/env"))
                .then(|| words.find(|w| !w.starts_with('-')).map(str::to_string))
                .flatten()
        });
    (replaceable, from_path)
}

/// Checks, right before a program is started, that its file is still the one
/// the approval pinned: `expected` is the SHA-256 recorded with the approval.
/// None recorded is refused (the gate never offers such a tool; this does not
/// rely on it). A file that changed (an update, `brew upgrade`, a
/// replacement) is refused and the user is told to approve the tool again.
///
/// The file is read and hashed, then started by its path: a file replaced in
/// the moment between the two is not caught.
pub(crate) async fn check_program_file(
    program: &str,
    expected: Option<&str>,
) -> Result<(), RemoteCallError> {
    let Some(expected) = expected else {
        return fail(format!(
            "no file content was pinned for the program `{program}`: approve the tool again with `nodespace tool approve`"
        ));
    };
    if !std::path::Path::new(program).is_absolute() {
        return fail(format!(
            "the program `{program}` is not a full path, so the file the approval pinned cannot be checked"
        ));
    }
    let path = program.to_string();
    let actual = tokio::task::spawn_blocking(move || file_sha256(&path))
        .await
        .map_err(|_| RemoteCallError("the program file could not be checked".to_string()))?;
    match actual {
        Ok(actual) if actual == expected => Ok(()),
        Ok(_) => fail(format!(
            "the program file `{program}` changed since the tool was approved (an update replaces it): approve the tool again with `nodespace tool approve`"
        )),
        Err(e) => fail(format!(
            "the program file `{program}` cannot be read ({}): approve the tool again once it is back",
            e.kind()
        )),
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

    /// Only a regular file within the size limit is hashed: a device (which
    /// would be read forever), a FIFO (which would block the thread), a
    /// directory and an oversized file are refused, and none of them hangs.
    #[test]
    fn only_a_regular_file_of_bounded_size_is_hashed() {
        let dir = tempfile::TempDir::new().unwrap();
        let regular = dir.path().join("prog");
        std::fs::write(&regular, b"echo hi\n").unwrap();
        let expected: String = {
            use sha2::{Digest, Sha256};
            Sha256::digest(b"echo hi\n")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        assert_eq!(file_sha256(regular.to_str().unwrap()).unwrap(), expected);
        let fifo = dir.path().join("fifo");
        let c_path = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let big = dir.path().join("big");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_PROGRAM_BYTES + 1)
            .unwrap();
        for refused in [
            "/dev/zero",
            fifo.to_str().unwrap(),
            dir.path().to_str().unwrap(),
            big.to_str().unwrap(),
        ] {
            let err = file_sha256(refused).unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::InvalidInput,
                "{refused}: {err}"
            );
        }
    }

    /// A file in a folder this user can write can be replaced by programs of
    /// this user; a system program cannot. A `#!/usr/bin/env X` script starts
    /// `X` from the `PATH`.
    #[test]
    fn what_a_program_file_says_about_its_pin() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = dir.path().join("tool");
        std::fs::write(&script, "#!/usr/bin/env -S python3 -u\nprint(1)\n").unwrap();
        assert_eq!(
            program_facts(script.to_str().unwrap()),
            (true, Some("python3".to_string()))
        );
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "#!/bin/sh\necho hi\n").unwrap();
        assert_eq!(program_facts(plain.to_str().unwrap()), (true, None));
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(program_facts("/bin/sh"), (false, None));
        }
    }

    /// A program tool whose approval pinned no content is not started, even
    /// though the gate already refuses to offer one.
    #[tokio::test]
    async fn a_program_with_no_pinned_content_is_not_started() {
        let err = check_program_file("/bin/sh", None).await.unwrap_err();
        assert!(err.0.contains("no file content was pinned"), "{}", err.0);
        let pin = file_sha256("/bin/sh").unwrap();
        assert!(check_program_file("/bin/sh", Some(&pin)).await.is_ok());
    }

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
