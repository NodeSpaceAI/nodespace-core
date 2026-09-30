//! The single-instance lock: at most one daemon serves a socket.
//!
//! Nothing else stops a second daemon on Unix. Both serve loops delete an
//! existing socket file before they bind, so a second daemon silently takes
//! over the socket while the first keeps its databases open. The lock closes
//! that: a daemon takes an exclusive advisory lock on a lock file beside its
//! socket before it does anything else, and a daemon that cannot take it exits.
//!
//! A daemon that finds the lock held waits for it, for a bounded time, before it
//! gives up. A restart starts the new daemon while the old one may still be
//! draining and has not yet exited, and a daemon that exits with status 0 is a
//! deliberate stop that a `KeepAlive { SuccessfulExit = false }` registration
//! never restarts, so a new daemon that lost that race at once would leave the
//! user with no daemon at all. Only a lock still held after the wait means a
//! peer is really serving.
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
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::create_dir_owner_only_blocking;

/// How long [`acquire`] waits for a lock another process holds before it reports
/// [`Acquire::HeldByAnother`]. It must stay comfortably above the time a daemon
/// takes to drain after a stop signal (the daemon's shutdown watchdog is 15 s),
/// or a restart during a slow drain still ends with no daemon.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(20);

/// How often a waiting daemon retries the lock.
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Environment variable, in milliseconds, that replaces [`DEFAULT_LOCK_WAIT`].
/// It exists so a test that starts a real second daemon need not sit out the
/// full wait. It is not a supported setting, and it is not a command-line
/// argument: a daemon exits with status 2 on any argument it does not know.
pub const LOCK_WAIT_ENV_VAR: &str = "NODESPACED_LOCK_WAIT_MS";

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

/// The outcome of [`acquire`] and [`acquire_waiting`].
#[derive(Debug)]
pub enum Acquire {
    /// This process holds the lock and may serve the socket. Keep the value
    /// alive for as long as the process serves.
    Held(InstanceLock),
    /// Another process still held the lock when the wait ended. `holder_pid` is
    /// the pid it recorded, if
    /// one could be read: a holder that has just taken the lock may not have
    /// written it yet, so treat it as a hint for a log line and nothing more.
    HeldByAnother { holder_pid: Option<u32> },
}

/// Takes the single-instance lock for the daemon that serves `socket`, waiting
/// up to [`DEFAULT_LOCK_WAIT`] (or [`LOCK_WAIT_ENV_VAR`]'s override) for a
/// holder to let go. See [`acquire_waiting`].
pub fn acquire(socket: &Path) -> Result<Acquire> {
    acquire_waiting(socket, lock_wait())
}

/// Takes the single-instance lock for the daemon that serves `socket`, waiting
/// up to `max_wait` for another holder to let go.
///
/// Blocking and synchronous, so a daemon can call it before its async runtime
/// exists. In order, it:
///
/// 1. creates the lock file's directory owner-only, by the rule the daemon uses
///    for every path it owns;
/// 2. opens or creates the lock file with mode `0o600`, without truncating it;
/// 3. tries the lock, at once, without waiting, so a free lock costs no delay;
/// 4. on success, records this process's pid in the file;
/// 5. on contention, retries every 250 ms until `max_wait` has passed, then
///    reads the holder's pid (best effort) and reports [`Acquire::HeldByAnother`].
///
/// `Duration::ZERO` makes a single attempt.
///
/// Any other failure is an error, never a `HeldByAnother`: a daemon that cannot
/// tell whether it holds the lock must not serve, and must not exit as if a
/// healthy peer did.
pub fn acquire_waiting(socket: &Path, max_wait: Duration) -> Result<Acquire> {
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

    let started = Instant::now();
    let mut announced = false;
    loop {
        match file.try_lock() {
            Ok(()) => {
                file.set_len(0)
                    .and_then(|()| file.write_all_at(std::process::id().to_string().as_bytes(), 0))
                    .with_context(|| format!("record pid in lock file {}", lock_path.display()))?;
                return Ok(Acquire::Held(InstanceLock { _file: file }));
            }
            Err(TryLockError::WouldBlock) => {
                let waited = started.elapsed();
                if waited >= max_wait {
                    return Ok(Acquire::HeldByAnother {
                        holder_pid: read_holder_pid(&file),
                    });
                }
                if !announced {
                    announced = true;
                    tracing::info!(
                        holder_pid = ?read_holder_pid(&file),
                        max_wait_secs = max_wait.as_secs_f32(),
                        "waiting for the single-instance lock: another nodespaced holds it"
                    );
                }
                std::thread::sleep(LOCK_POLL_INTERVAL.min(max_wait - waited));
            }
            Err(TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("lock file {}", lock_path.display()));
            }
        }
    }
}

/// How long [`acquire`] waits: [`LOCK_WAIT_ENV_VAR`] in milliseconds if it
/// holds a number, otherwise [`DEFAULT_LOCK_WAIT`].
fn lock_wait() -> Duration {
    wait_from_override(std::env::var(LOCK_WAIT_ENV_VAR).ok().as_deref())
}

/// [`lock_wait`] for a given value of [`LOCK_WAIT_ENV_VAR`], so the parsing can
/// be tested without touching the process environment.
fn wait_from_override(millis: Option<&str>) -> Duration {
    millis
        .and_then(|millis| millis.trim().parse().ok())
        .map_or(DEFAULT_LOCK_WAIT, Duration::from_millis)
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

    /// One attempt, with no wait: what the tests about who holds the lock need.
    fn acquire_now(socket: &Path) -> Result<Acquire> {
        acquire_waiting(socket, Duration::ZERO)
    }

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

        let _lock = held(acquire_now(&socket).unwrap());
    }

    #[test]
    fn a_second_acquire_on_the_same_socket_is_held_by_another_and_names_the_holder() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");

        let _first = held(acquire_now(&socket).unwrap());
        let second = acquire_now(&socket).unwrap();

        // flock belongs to the open file description, so two opens in one
        // process conflict, and the recorded pid is this process's.
        assert_eq!(holder_pid(second), Some(std::process::id()));
    }

    #[test]
    fn a_lock_released_during_the_wait_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");
        let old_daemon = held(acquire_now(&socket).unwrap());
        // The old daemon is still draining when the new one starts, and exits
        // a moment later.
        let started = Instant::now();
        let exiting = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(old_daemon);
        });

        let outcome = acquire_waiting(&socket, Duration::from_secs(20)).unwrap();
        let waited = started.elapsed();

        exiting.join().unwrap();
        let _lock = held(outcome);
        assert!(
            waited >= Duration::from_millis(300),
            "the lock cannot have been taken before its holder let go, but only {waited:?} passed"
        );
    }

    #[test]
    fn a_lock_that_stays_held_is_reported_only_after_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");
        let _serving = held(acquire_now(&socket).unwrap());
        let wait = Duration::from_millis(600);

        let started = Instant::now();
        let outcome = acquire_waiting(&socket, wait).unwrap();
        let waited = started.elapsed();

        assert_eq!(holder_pid(outcome), Some(std::process::id()));
        assert!(
            waited >= wait,
            "gave up after {waited:?}, before the {wait:?} wait was over"
        );
    }

    #[test]
    fn the_wait_override_is_read_in_milliseconds_and_falls_back_to_the_default() {
        assert_eq!(wait_from_override(None), DEFAULT_LOCK_WAIT);
        assert_eq!(
            wait_from_override(Some("1500")),
            Duration::from_millis(1500)
        );
        assert_eq!(wait_from_override(Some(" 0 ")), Duration::ZERO);
        assert_eq!(wait_from_override(Some("soon")), DEFAULT_LOCK_WAIT);
        assert_eq!(wait_from_override(Some("")), DEFAULT_LOCK_WAIT);
    }

    #[test]
    fn dropping_the_lock_frees_it() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");

        let first = held(acquire_now(&socket).unwrap());
        drop(first);

        let _again = held(acquire_now(&socket).unwrap());
    }

    #[test]
    fn sockets_in_one_directory_do_not_contend() {
        let dir = tempfile::tempdir().unwrap();

        let _daemon = held(acquire_now(&dir.path().join("daemon.sock")).unwrap());
        let _dev = held(acquire_now(&dir.path().join("daemon-dev.sock")).unwrap());
    }

    #[test]
    fn the_parent_directory_is_created_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("nested").join(".nodespace");
        let socket = state_dir.join("d.sock");

        let _lock = held(acquire_now(&socket).unwrap());

        let mode = std::fs::metadata(&state_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn the_lock_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("d.sock");

        let _lock = held(acquire_now(&socket).unwrap());

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

        let _lock = held(acquire_now(&socket).unwrap());

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

        assert_eq!(holder_pid(acquire_now(&socket).unwrap()), None);
    }

    #[test]
    fn a_socket_path_that_is_its_own_lock_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();

        let err = acquire_now(&dir.path().join("d.lock")).unwrap_err();

        assert!(
            err.to_string().contains("its own lock file"),
            "unexpected error: {err:#}"
        );
    }
}
