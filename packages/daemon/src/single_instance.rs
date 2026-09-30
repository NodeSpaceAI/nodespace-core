//! The single-instance lock: at most one daemon serves a socket.
//!
//! Nothing else stops a second daemon on Unix. Both serve loops delete an
//! existing socket file before they bind, so a second daemon silently takes
//! over the socket while the first keeps its databases open. The lock closes
//! that: a daemon takes an exclusive advisory lock on a lock file beside its
//! socket before it does anything else, and a daemon that cannot take it exits.
//!
//! The lock file sits beside the socket rather than at a fixed path, so an
//! isolated daemon that sets its own `NODESPACED_SOCKET` never contends with
//! the user's daemon. The file name is [`lock_path_for`]'s: the socket path
//! with the extension `lock` (`daemon.sock` -> `daemon.lock`).
//!
//! The lock is `flock` on Unix. It belongs to the open file description, so the
//! OS releases it when the process exits and a crash leaves no stale lock. A
//! file descriptor Rust opens is close-on-exec, so an agent process the daemon
//! spawns does not inherit it and cannot keep the lock held once the daemon has
//! gone.
//!
//! The lock is advisory: a process that ignores it is not stopped. It prevents
//! accidents, not a local attacker, who already has full access through the
//! socket (ADR-052). Windows needs no lock, because a named pipe admits only
//! one first server instance.
//!
//! Every daemon built on core's crates calls [`acquire`], so the guarantee
//! holds whichever daemon a launcher registers.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Read;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::create_dir_owner_only_blocking;

/// The most bytes read back from a lock file to recover the holder's pid. A pid
/// has at most ten digits, so this leaves room for a newline and nothing else.
const PID_READ_LIMIT: u64 = 32;

/// The lock file that guards `socket`: the socket path with the extension
/// `lock`, so it sits in the same directory.
pub fn lock_path_for(socket: &Path) -> PathBuf {
    socket.with_extension("lock")
}

/// A held single-instance lock. Dropping it, or the process exiting, releases
/// the lock.
#[derive(Debug)]
pub struct InstanceLock {
    /// Kept open only because the lock lives as long as this description does.
    _file: File,
}

/// The outcome of [`acquire`].
#[derive(Debug)]
pub enum Acquire {
    /// This process holds the lock and may serve the socket. Keep the value
    /// alive for as long as the process serves.
    Held(InstanceLock),
    /// Another process holds the lock. `holder_pid` is the pid it recorded, if
    /// one could be read: a holder that has just taken the lock may not have
    /// written it yet, so treat it as a hint for a log line and nothing more.
    HeldByAnother { holder_pid: Option<u32> },
}

/// Takes the single-instance lock for the daemon that serves `socket`.
///
/// Blocking and synchronous, so a daemon can call it before its async runtime
/// exists. In order, it:
///
/// 1. creates the lock file's directory owner-only, by the rule the daemon uses
///    for every path it owns;
/// 2. opens or creates the lock file with mode `0o600`, without truncating it;
/// 3. tries the lock without waiting;
/// 4. on success, records this process's pid in the file;
/// 5. on contention, reads the holder's pid (best effort).
///
/// Any other failure is an error, never a `HeldByAnother`: a daemon that cannot
/// tell whether it holds the lock must not serve, and must not exit as if a
/// healthy peer did.
pub fn acquire(socket: &Path) -> Result<Acquire> {
    let lock_path = lock_path_for(socket);
    anyhow::ensure!(
        lock_path != socket,
        "socket path {} has the extension `lock`, which would make it its own lock file",
        socket.display()
    );

    if let Some(parent) = lock_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        create_dir_owner_only_blocking(parent)?;
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .with_context(|| format!("open lock file {}", lock_path.display()))?;

    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)
                .and_then(|()| file.write_all_at(std::process::id().to_string().as_bytes(), 0))
                .with_context(|| format!("record pid in lock file {}", lock_path.display()))?;
            Ok(Acquire::Held(InstanceLock { _file: file }))
        }
        Err(TryLockError::WouldBlock) => Ok(Acquire::HeldByAnother {
            holder_pid: read_holder_pid(&file),
        }),
        Err(TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("lock file {}", lock_path.display()))
        }
    }
}

/// The pid recorded in a lock file another process holds, or `None` if it is
/// empty, unreadable or not a pid.
fn read_holder_pid(file: &File) -> Option<u32> {
    let mut recorded = String::new();
    file.take(PID_READ_LIMIT)
        .read_to_string(&mut recorded)
        .ok()?;
    recorded.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn held(outcome: Acquire) -> InstanceLock {
        match outcome {
            Acquire::Held(lock) => lock,
            other => panic!("expected the lock to be free, got {other:?}"),
        }
    }

    fn holder_pid(outcome: Acquire) -> Option<u32> {
        match outcome {
            Acquire::HeldByAnother { holder_pid } => holder_pid,
            other => panic!("expected the lock to be held, got {other:?}"),
        }
    }

    #[test]
    fn lock_path_sits_beside_the_socket_with_the_lock_extension() {
        assert_eq!(
            lock_path_for(Path::new("/home/u/.nodespace/daemon.sock")),
            Path::new("/home/u/.nodespace/daemon.lock")
        );
        assert_eq!(
            lock_path_for(Path::new("/home/u/.nodespace/daemon-dev.sock")),
            Path::new("/home/u/.nodespace/daemon-dev.lock")
        );
    }

    #[test]
    fn a_free_lock_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");

        let _lock = held(acquire(&socket).unwrap());
    }

    #[test]
    fn a_second_acquire_on_the_same_socket_is_held_by_another_and_names_the_holder() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");

        let _first = held(acquire(&socket).unwrap());
        let second = acquire(&socket).unwrap();

        // flock belongs to the open file description, so two opens in one
        // process conflict, and the recorded pid is this process's.
        assert_eq!(holder_pid(second), Some(std::process::id()));
    }

    #[test]
    fn dropping_the_lock_frees_it() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");

        let first = held(acquire(&socket).unwrap());
        drop(first);

        let _again = held(acquire(&socket).unwrap());
    }

    #[test]
    fn sockets_in_one_directory_do_not_contend() {
        let dir = tempfile::tempdir().unwrap();

        let _daemon = held(acquire(&dir.path().join("daemon.sock")).unwrap());
        let _dev = held(acquire(&dir.path().join("daemon-dev.sock")).unwrap());
    }

    #[test]
    fn the_parent_directory_is_created_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("nested").join(".nodespace");
        let socket = state_dir.join("d.sock");

        let _lock = held(acquire(&socket).unwrap());

        let mode = std::fs::metadata(&state_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn the_lock_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");

        let _lock = held(acquire(&socket).unwrap());

        let mode = std::fs::metadata(lock_path_for(&socket))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode & 0o077, 0, "lock file mode is {mode:o}");
    }

    #[test]
    fn a_stale_pid_from_a_dead_holder_is_replaced_not_appended_to() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");
        // A crashed daemon leaves its pid behind, and a longer one than ours
        // would survive an overwrite that forgot to truncate.
        std::fs::write(lock_path_for(&socket), "99999999999").unwrap();

        let _lock = held(acquire(&socket).unwrap());

        assert_eq!(
            std::fs::read_to_string(lock_path_for(&socket)).unwrap(),
            std::process::id().to_string()
        );
    }

    #[test]
    fn an_unreadable_recorded_pid_is_reported_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");
        // Hold the lock by hand, so what the file says is whatever we wrote.
        let lock_path = lock_path_for(&socket);
        let holder = File::create(&lock_path).unwrap();
        holder.try_lock().unwrap();
        holder.write_all_at(b"not a pid", 0).unwrap();

        assert_eq!(holder_pid(acquire(&socket).unwrap()), None);
    }

    #[test]
    fn a_socket_path_that_is_its_own_lock_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();

        let err = acquire(&dir.path().join("d.lock")).unwrap_err();

        assert!(
            err.to_string().contains("its own lock file"),
            "unexpected error: {err:#}"
        );
    }
}
