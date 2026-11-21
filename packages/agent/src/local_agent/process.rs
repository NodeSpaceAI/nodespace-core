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

/// What an approval learns about a program file, all from one open of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgramInspection {
    /// SHA-256 of its content, lowercase hex.
    pub(crate) sha256: String,
    /// Programs running as this user can replace it, or an interpreter its
    /// `#!` line names (that one's own included; see [`path_replaceable`]), or
    /// that interpreter is a relative path: then the pin does not hold against
    /// them.
    pub(crate) replaceable: bool,
    /// For a `#!/usr/bin/env X` script (or one whose interpreter is such a
    /// script), the `X` it looks up on the `PATH` at each start, which is not
    /// pinned.
    pub(crate) starts_from_path: Option<String>,
}

/// SHA-256 of the file at `path` (following a symbolic link), as lowercase
/// hex: what an approval pins for a program, and what a runner checks before
/// it starts one. See [`inspect_program`] for what is refused.
pub(crate) fn file_sha256(path: &str) -> std::io::Result<String> {
    hash_program(path).map(|(sha256, _)| sha256)
}

/// What an approval pins of the program file at `path`, and how far that
/// holds. The file is opened once, without waiting (a FIFO does not block the
/// thread) and without becoming a controlling terminal; only a regular file of
/// at most [`MAX_PROGRAM_BYTES`] is read (checked on the open file: a device, a
/// FIFO or a directory is refused, and so is a file that is too large), as a
/// stream from that same open file, never past the limit. Its `#!` line, if
/// any, is read from the same bytes, as far as the system reads one
/// ([`SHEBANG_BYTES`]), and the interpreter it names is followed (one that is
/// itself a script too, up to [`MAX_INTERPRETERS`] deep).
pub(crate) fn inspect_program(path: &str) -> std::io::Result<ProgramInspection> {
    inspect_with(path, &path_replaceable)
}

/// Most bytes of a `#!` line the system reads (macOS reads 512).
const SHEBANG_BYTES: usize = 512;
/// Most `#!` interpreters followed from a program: an interpreter that is
/// itself a script, and so on.
const MAX_INTERPRETERS: usize = 4;
/// Most symbolic links followed in one path (the system's own limit is 32).
const MAX_LINK_HOPS: usize = 32;

/// [`inspect_program`], with `is_replaceable` saying which files programs of
/// this user can replace.
fn inspect_with(
    path: &str,
    is_replaceable: &dyn Fn(&std::path::Path) -> bool,
) -> std::io::Result<ProgramInspection> {
    let (sha256, head) = hash_program(path)?;
    let (replaceable, starts_from_path) =
        replaceable_with(std::path::Path::new(path), head, is_replaceable);
    Ok(ProgramInspection {
        sha256,
        replaceable,
        starts_from_path,
    })
}

/// Opens the file at `path` without waiting (a FIFO does not block the
/// thread) and without it becoming a controlling terminal, and refuses
/// anything but a regular file.
fn open_regular(path: &std::path::Path) -> std::io::Result<(std::fs::File, std::fs::Metadata)> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok((file, meta))
}

/// The SHA-256 of the program file at `path` and its first
/// [`SHEBANG_BYTES`].
fn hash_program(path: &str) -> std::io::Result<(String, Vec<u8>)> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let refuse = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, why.to_string());
    let (file, meta) = open_regular(std::path::Path::new(path))?;
    if meta.len() > MAX_PROGRAM_BYTES {
        return Err(refuse("larger than a program file may be"));
    }
    let mut hasher = Sha256::new();
    let mut reader = file.take(MAX_PROGRAM_BYTES + 1);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut head = Vec::new();
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
        if head.len() < SHEBANG_BYTES {
            let take = (SHEBANG_BYTES - head.len()).min(n);
            head.extend_from_slice(&buffer[..take]);
        }
        hasher.update(&buffer[..n]);
    }
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((sha256, head))
}

/// The first [`SHEBANG_BYTES`] of the regular file at `path` (an
/// interpreter, for its own `#!` line), opened as [`hash_program`] opens one.
fn read_head(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let (file, _) = open_regular(path)?;
    let mut head = Vec::with_capacity(SHEBANG_BYTES);
    file.take(SHEBANG_BYTES as u64).read_to_end(&mut head)?;
    Ok(head)
}

/// What a script's `#!` line names: the interpreter's path, and for
/// `#!/usr/bin/env X` the program `X` looked up on the `PATH`.
fn shebang(head: &[u8]) -> (Option<String>, Option<String>) {
    let text = String::from_utf8_lossy(head);
    let Some(line) = text.lines().next().and_then(|l| l.strip_prefix("#!")) else {
        return (None, None);
    };
    let mut words = line.split_whitespace();
    let Some(interpreter) = words.next() else {
        return (None, None);
    };
    let from_path = (interpreter == "/usr/bin/env")
        .then(|| words.find(|w| !w.starts_with('-')).map(str::to_string))
        .flatten();
    (Some(interpreter.to_string()), from_path)
}

/// Whether `program` (whose first bytes are `head`), the interpreter its
/// `#!` line names, or that interpreter's own, up to [`MAX_INTERPRETERS`]
/// deep, is one `is_replaceable` says programs of this user can replace; and
/// the first program a `#!/usr/bin/env X` line on the way starts from the
/// `PATH`. A relative interpreter is found from the working folder of
/// whatever starts the script, not a fixed file: replaceable.
fn replaceable_with(
    program: &std::path::Path,
    head: Vec<u8>,
    is_replaceable: &dyn Fn(&std::path::Path) -> bool,
) -> (bool, Option<String>) {
    let mut replaceable = is_replaceable(program);
    let mut starts_from_path = None;
    let mut head = head;
    for _ in 0..MAX_INTERPRETERS {
        let (Some(interpreter), from_path) = shebang(&head) else {
            break;
        };
        starts_from_path = starts_from_path.or(from_path);
        let interpreter = std::path::PathBuf::from(interpreter);
        if !interpreter.is_absolute() {
            replaceable = true;
            break;
        }
        replaceable |= is_replaceable(&interpreter);
        match read_head(&interpreter) {
            Ok(next) => head = next,
            Err(_) => break,
        }
    }
    (replaceable, starts_from_path)
}

/// Whether programs running as this user can replace the file at `path`: the
/// file or any folder above it, as written, after each symbolic link on the
/// way is followed (wherever it lies in the path, one hop at a time), or as
/// it resolves, is owned by this user or writable by this user.
pub(crate) fn path_replaceable(path: &std::path::Path) -> bool {
    any_above(path, &mine_or_writable)
}

/// Whether `is_replaceable` holds for `path` or a folder above it: as
/// written, at each hop of its resolution ([`link_hops`]), or as it resolves.
fn any_above(path: &std::path::Path, is_replaceable: &dyn Fn(&std::path::Path) -> bool) -> bool {
    let resolved = std::fs::canonicalize(path).ok();
    let hops = link_hops(path);
    hops.iter()
        .chain(resolved.iter())
        .flat_map(|p| p.ancestors())
        .filter(|p| !p.as_os_str().is_empty())
        .any(is_replaceable)
}

/// The paths `path` passes through as it resolves: as written, then with its
/// first symbolic link (in any component) replaced by what the link points
/// to, and so on, up to [`MAX_LINK_HOPS`] links.
fn link_hops(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut hops = vec![path.to_path_buf()];
    let mut current = path.to_path_buf();
    for _ in 0..MAX_LINK_HOPS {
        let Some((link, rest)) = first_link(&current) else {
            break;
        };
        let Ok(target) = std::fs::read_link(&link) else {
            break;
        };
        // A relative target is read from the link's own folder.
        let mut next = link
            .parent()
            .unwrap_or_else(|| std::path::Path::new("/"))
            .join(target);
        if !rest.as_os_str().is_empty() {
            next.push(rest);
        }
        hops.push(next.clone());
        current = next;
    }
    hops
}

/// The first component of `path`, from its start, that is a symbolic link
/// (as the path up to it), and the rest of the path after it.
fn first_link(path: &std::path::Path) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let mut prefix = std::path::PathBuf::new();
    let mut components = path.components();
    while let Some(component) = components.next() {
        prefix.push(component);
        if std::fs::symlink_metadata(&prefix).is_ok_and(|m| m.file_type().is_symlink()) {
            return Some((prefix, components.as_path().to_path_buf()));
        }
    }
    None
}

/// Whether `path` itself (not what a link there points to) is owned by this
/// user, or this user may write it.
fn mine_or_writable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;
        let me = unsafe { libc::geteuid() };
        let owned = std::fs::symlink_metadata(path).is_ok_and(|m| m.uid() == me);
        let writable = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
            .unwrap_or(false);
        owned || writable
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
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

    /// A file in a folder this user owns or can write can be replaced by
    /// programs of this user, and so can one under a symbolic link that lies
    /// in such a folder; a system program cannot. A `#!/usr/bin/env X` script
    /// starts `X` from the `PATH`.
    #[test]
    fn what_a_program_file_says_about_its_pin() {
        let dir = tempfile::TempDir::new().unwrap();
        let script = dir.path().join("tool");
        std::fs::write(&script, "#!/usr/bin/env -S python3 -u\nprint(1)\n").unwrap();
        let seen = inspect_program(script.to_str().unwrap()).unwrap();
        assert!(seen.replaceable);
        assert_eq!(seen.starts_from_path.as_deref(), Some("python3"));
        // Owned by this user though not writable: still replaceable.
        let locked = dir.path().join("locked");
        std::fs::write(&locked, "#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o555))
            .unwrap();
        assert!(mine_or_writable(&locked));
        assert_eq!(
            inspect_program(locked.to_str().unwrap())
                .unwrap()
                .starts_from_path,
            None
        );
        if unsafe { libc::geteuid() } != 0 {
            assert!(!mine_or_writable(std::path::Path::new("/bin/sh")));
            assert!(!path_replaceable(std::path::Path::new("/bin/sh")));
            let system = inspect_program("/bin/sh").unwrap();
            assert!(!system.replaceable);
            assert_eq!(system.starts_from_path, None);
        }
    }

    /// The walk covers the folders above the path as written (where a link
    /// lies) and above what it resolves to.
    #[test]
    fn a_program_is_replaceable_through_any_folder_above_it_or_its_link() {
        let dir = tempfile::TempDir::new().unwrap();
        let mine = std::fs::canonicalize(dir.path()).unwrap();
        let only_mine = |p: &std::path::Path| p == mine;
        // A link in that folder to a system program: the link can be swapped.
        let link = mine.join("sh");
        std::os::unix::fs::symlink("/bin/sh", &link).unwrap();
        assert!(any_above(&link, &only_mine));
        // A program reached through a link elsewhere is replaceable when the
        // folder it resolves into is.
        let real = mine.join("bin").join("tool");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "x").unwrap();
        let elsewhere = tempfile::TempDir::new().unwrap();
        let via = elsewhere.path().join("tool");
        std::os::unix::fs::symlink(&real, &via).unwrap();
        assert!(any_above(&via, &only_mine));
        assert!(!any_above(std::path::Path::new("/bin/sh"), &only_mine));
        // A link in the middle of the chain, in that folder, between a link
        // elsewhere and a file elsewhere: neither the path as written nor
        // what it resolves to is in the folder, but the hop is.
        let far = tempfile::TempDir::new().unwrap();
        let file = far.path().join("tool");
        std::fs::write(&file, "x").unwrap();
        let hop = mine.join("hop");
        std::os::unix::fs::symlink(&file, &hop).unwrap();
        let first = elsewhere.path().join("first");
        std::os::unix::fs::symlink(&hop, &first).unwrap();
        assert!(any_above(&first, &only_mine));
        // The same through a folder link in the middle of the path.
        let far_bin = far.path().join("bin");
        std::fs::create_dir(&far_bin).unwrap();
        std::fs::write(far_bin.join("tool"), "x").unwrap();
        std::os::unix::fs::symlink(&far_bin, mine.join("bin-hop")).unwrap();
        let first_bin = elsewhere.path().join("first-bin");
        std::os::unix::fs::symlink(mine.join("bin-hop"), &first_bin).unwrap();
        assert!(any_above(&first_bin.join("tool"), &only_mine));
        let unrelated = tempfile::TempDir::new().unwrap();
        let direct = unrelated.path().join("direct");
        std::os::unix::fs::symlink(&file, &direct).unwrap();
        assert!(!any_above(&direct, &only_mine));
    }

    /// The system reads up to 512 bytes of a `#!` line (macOS): an
    /// interpreter named past the 256th byte is the one checked.
    #[test]
    fn a_shebang_line_is_read_as_far_as_the_system_reads_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let deep = dir.path().join("a".repeat(150)).join("b".repeat(150));
        std::fs::create_dir_all(&deep).unwrap();
        let interpreter = deep.join("python3");
        std::fs::write(&interpreter, "x").unwrap();
        assert!(interpreter.as_os_str().len() > 300);
        let script = dir.path().join("tool");
        std::fs::write(&script, format!("#!{}\nprint(1)\n", interpreter.display())).unwrap();
        let only_interpreter = |p: &std::path::Path| p == interpreter;
        assert!(
            inspect_with(script.to_str().unwrap(), &only_interpreter)
                .unwrap()
                .replaceable
        );
    }

    /// An interpreter that is itself a script is followed, a few levels deep:
    /// a replaceable interpreter, or one started from the `PATH`, at any
    /// level counts. A loop ends.
    #[test]
    fn an_interpreter_that_is_itself_a_script_is_followed() {
        let dir = tempfile::TempDir::new().unwrap();
        let inner = dir.path().join("inner");
        std::fs::write(&inner, "x").unwrap();
        let outer = dir.path().join("outer");
        std::fs::write(&outer, format!("#!{} -x\n", inner.display())).unwrap();
        let script = dir.path().join("tool");
        std::fs::write(&script, format!("#!{}\nrun\n", outer.display())).unwrap();
        let only_inner = |p: &std::path::Path| p == inner;
        assert!(
            inspect_with(script.to_str().unwrap(), &only_inner)
                .unwrap()
                .replaceable
        );
        let by_env = dir.path().join("by-env");
        std::fs::write(&by_env, "#!/usr/bin/env -S node --flag\n").unwrap();
        let via_env = dir.path().join("via-env");
        std::fs::write(&via_env, format!("#!{}\n", by_env.display())).unwrap();
        let seen = inspect_with(via_env.to_str().unwrap(), &|_| false).unwrap();
        assert_eq!(seen.starts_from_path.as_deref(), Some("node"));
        assert!(!seen.replaceable);
        let looped = dir.path().join("looped");
        std::fs::write(&looped, format!("#!{}\n", looped.display())).unwrap();
        assert!(
            !inspect_with(looped.to_str().unwrap(), &|_| false)
                .unwrap()
                .replaceable
        );
    }

    /// A relative `#!` interpreter is found from the working folder of
    /// whatever starts the script, not a fixed file: replaceable.
    #[test]
    fn a_relative_interpreter_is_replaceable() {
        let dir = tempfile::TempDir::new().unwrap();
        for line in ["#!python3\n", "#!./bin/python3 -u\n"] {
            let script = dir.path().join("tool");
            std::fs::write(&script, line).unwrap();
            assert!(
                inspect_with(script.to_str().unwrap(), &|_| false)
                    .unwrap()
                    .replaceable,
                "{line}"
            );
        }
    }

    /// A script the user cannot replace that names an interpreter the user
    /// can replace is replaceable too.
    #[test]
    fn a_replaceable_interpreter_makes_its_script_replaceable() {
        let dir = tempfile::TempDir::new().unwrap();
        let interpreter = dir.path().join("python3");
        let script = dir.path().join("tool");
        std::fs::write(
            &script,
            format!("#!{} -u\nprint(1)\n", interpreter.display()),
        )
        .unwrap();
        let only_interpreter = |p: &std::path::Path| p == interpreter;
        assert!(
            inspect_with(script.to_str().unwrap(), &only_interpreter)
                .unwrap()
                .replaceable
        );
        assert!(
            !inspect_with(script.to_str().unwrap(), &|_| false)
                .unwrap()
                .replaceable
        );
        let replaceable = |p: &std::path::Path| p.starts_with("/home/me");
        let script = std::path::Path::new("/usr/local/libexec/tool");
        assert_eq!(
            replaceable_with(script, b"print(1)\n".to_vec(), &replaceable),
            (false, None)
        );
        assert_eq!(
            replaceable_with(script, b"#!/usr/bin/python3\n".to_vec(), &replaceable),
            (false, None)
        );
        assert_eq!(
            replaceable_with(script, b"#!/home/me/bin/python3\n".to_vec(), &replaceable),
            (true, None)
        );
        assert_eq!(
            shebang(b"#!/home/me/bin/python3 -u\nprint(1)\n"),
            (Some("/home/me/bin/python3".to_string()), None)
        );
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
