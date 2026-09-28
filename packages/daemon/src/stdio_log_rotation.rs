//! Size-bounded rotation of the log file behind the daemon's own stdout/stderr.
//!
//! Every way `nodespaced` is run as a service hands it its log as *inherited
//! stdio* rather than a file it opens itself: launchd's
//! `StandardOutPath`/`StandardErrorPath` (the desktop app's plist and the
//! Homebrew formula's `service do` block alike) and systemd's
//! `StandardOutput=append:`. Everything the process writes to fd 1/2 lands
//! there — `tracing` output, `eprintln!` diagnostics and panic messages — so
//! bounding it has to bound the fds themselves, not just the `tracing` sink.
//!
//! A headless install (`brew services`, a hand-written systemd unit) has no
//! supervising process alive between restarts to rotate that file, and under
//! `KeepAlive`/`Restart=` a healthy daemon may never restart at all. So the
//! daemon rotates it itself: [`spawn`] checks both fds on an interval and,
//! once the regular file behind them has grown past [`LOG_MAX_BYTES`], renames
//! it to `<name>.1` (shifting older generations down to `.<LOG_KEEP>`), opens
//! a fresh file at the original path and `dup2`s it over every stdio fd that
//! pointed at the old one. Writes made between the rename and the `dup2` land
//! in `<name>.1`, so nothing is lost; the service manager holds no handle of
//! its own on the file, so nothing keeps writing to the rotated copy.
//!
//! An fd that is not a regular file — a terminal, a pipe, `/dev/null`, or the
//! journald stream socket of a unit without `StandardOutput=append:` — is left
//! alone: its size is already someone else's to bound.

use std::fs::{File, OpenOptions};
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Size past which the live log file is rotated. Matches the desktop app's
/// startup-time rotation threshold, so a file rotates at the same size
/// whichever side gets to it.
pub const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Rotated generations kept per log file (`.1` … `.3`); older ones are
/// deleted. Together with [`LOG_MAX_BYTES`] and [`CHECK_INTERVAL`] this bounds
/// a log's disk footprint at roughly `LOG_MAX_BYTES * (LOG_KEEP + 1)`.
pub const LOG_KEEP: u32 = 3;

/// How often [`spawn`] re-checks the stdio fds. Two `fstat`s per tick, so a
/// short interval costs nothing; it bounds how far past [`LOG_MAX_BYTES`] the
/// live file can get before it is rotated.
const CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Start the periodic stdout/stderr rotation check on a dedicated thread for
/// the life of the process.
///
/// A plain thread rather than a tokio task: it must keep running regardless of
/// which runtime the daemon's serve loop is on, and it only sleeps and makes
/// blocking filesystem calls.
pub fn spawn() {
    let spawned = std::thread::Builder::new()
        .name("stdio-log-rotation".into())
        .spawn(|| loop {
            std::thread::sleep(CHECK_INTERVAL);
            rotate_if_oversized(&[libc::STDOUT_FILENO, libc::STDERR_FILENO], LOG_MAX_BYTES);
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "Could not start stdio log rotation; the daemon log will grow unbounded");
    }
}

/// A regular file one or more of the checked fds currently write to.
struct LogTarget {
    path: PathBuf,
    size: u64,
    mode: u32,
    fds: Vec<RawFd>,
}

/// Rotate the regular file behind each of `fds` that has grown past
/// `max_bytes`, pointing every fd that shared it at a fresh file.
///
/// fds are grouped by the file they refer to (device + inode), not by fd:
/// the Homebrew formula points `log_path` and `error_log_path` at the same
/// file, which launchd opens twice, so stdout and stderr are two open file
/// descriptions of one file and must be rotated as one.
///
/// Best-effort: every failure is logged and leaves the fd writing where it
/// was, since a rotation problem must never take the daemon's logging down.
pub fn rotate_if_oversized(fds: &[RawFd], max_bytes: u64) {
    for target in log_targets(fds) {
        if target.size > max_bytes {
            rotate(&target);
        }
    }
}

fn log_targets(fds: &[RawFd]) -> Vec<LogTarget> {
    let mut targets: Vec<(u64, u64, LogTarget)> = Vec::new();
    for &fd in fds {
        // Borrow the fd as a `File` without taking ownership: dropping the
        // `ManuallyDrop` never closes it.
        // SAFETY: `fd` is only read through `metadata()` while it is open.
        let file = ManuallyDrop::new(unsafe { File::from_raw_fd(fd) });
        let Ok(meta) = file.metadata() else { continue };
        if !meta.file_type().is_file() {
            continue;
        }
        if let Some((_, _, target)) = targets
            .iter_mut()
            .find(|(dev, ino, _)| *dev == meta.dev() && *ino == meta.ino())
        {
            target.fds.push(fd);
            continue;
        }
        let Some(path) = fd_path(fd) else { continue };
        // The path must still name the file the fd writes to — if something
        // else already renamed or replaced it, rotating `path` would roll a
        // file this process is not writing to.
        match std::fs::metadata(&path) {
            Ok(on_disk) if on_disk.dev() == meta.dev() && on_disk.ino() == meta.ino() => {}
            _ => continue,
        }
        targets.push((
            meta.dev(),
            meta.ino(),
            LogTarget {
                path,
                size: meta.len(),
                mode: meta.mode() & 0o7777,
                fds: vec![fd],
            },
        ));
    }
    targets.into_iter().map(|(_, _, target)| target).collect()
}

/// The path of the file `fd` refers to, if the platform can report it.
#[cfg(target_os = "macos")]
fn fd_path(fd: RawFd) -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;

    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes a NUL-terminated path of at most PATH_MAX bytes
    // into `buf`, which is exactly that size.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } == -1 {
        return None;
    }
    let path = CStr::from_bytes_until_nul(&buf).ok()?;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(path.to_bytes())))
}

/// The path of the file `fd` refers to, if the platform can report it.
#[cfg(target_os = "linux")]
fn fd_path(fd: RawFd) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/self/fd/{fd}")).ok()
}

/// The path of the file `fd` refers to, if the platform can report it.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn fd_path(_fd: RawFd) -> Option<PathBuf> {
    None
}

fn generation(path: &Path, n: u32) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{n}"));
    path.with_file_name(name)
}

fn rotate(target: &LogTarget) {
    let path = &target.path;

    // Drop the oldest generation, then shift the rest down: .2 -> .3, .1 -> .2.
    // Walking downwards keeps each destination free before it is written to.
    let oldest = generation(path, LOG_KEEP);
    if let Err(e) = std::fs::remove_file(&oldest) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %oldest.display(), error = %e, "Could not remove oldest rotated daemon log");
        }
    }
    for n in (1..LOG_KEEP).rev() {
        let from = generation(path, n);
        if from.exists() {
            if let Err(e) = std::fs::rename(&from, generation(path, n + 1)) {
                tracing::warn!(from = %from.display(), error = %e, "Could not shift rotated daemon log generation");
            }
        }
    }

    if let Err(e) = std::fs::rename(path, generation(path, 1)) {
        tracing::warn!(path = %path.display(), error = %e, "Could not rotate daemon log — it will keep growing");
        return;
    }

    // Everything written from here until the dup2 below still lands in the
    // renamed `.1` file through the old descriptions, so nothing is lost.
    let fresh = match OpenOptions::new()
        .create(true)
        .append(true)
        .mode(target.mode)
        .open(path)
    {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "Could not open a fresh daemon log after rotating — logging continues into the rotated file"
            );
            return;
        }
    };

    for &fd in &target.fds {
        // dup2 replaces `fd` atomically, so no concurrent write sees it closed.
        // SAFETY: both fds are open; `fd` stays owned by the process's stdio.
        if unsafe { libc::dup2(fresh.as_raw_fd(), fd) } == -1 {
            tracing::warn!(
                fd,
                error = %std::io::Error::last_os_error(),
                "Could not redirect a stdio fd to the fresh daemon log"
            );
        }
    }

    tracing::info!(
        path = %path.display(),
        size_bytes = target.size,
        "Rotated daemon log past size threshold"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn open_append(path: &Path) -> File {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
    }

    /// Write through a borrowed raw fd, the way stdio writes to fd 1/2.
    fn write_fd(fd: RawFd, bytes: &[u8]) {
        let mut file = ManuallyDrop::new(unsafe { File::from_raw_fd(fd) });
        file.write_all(bytes).unwrap();
    }

    #[test]
    fn oversized_file_rolls_to_dot_one_and_fd_writes_to_a_fresh_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nodespaced.log");
        let file = open_append(&path);
        write_fd(file.as_raw_fd(), b"old history\n");

        rotate_if_oversized(&[file.as_raw_fd()], 4);
        write_fd(file.as_raw_fd(), b"after rotation\n");

        assert_eq!(
            std::fs::read_to_string(generation(&path, 1)).unwrap(),
            "old history\n"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after rotation\n");
    }

    #[test]
    fn file_at_or_under_threshold_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nodespaced.log");
        let file = open_append(&path);
        write_fd(file.as_raw_fd(), b"1234");

        rotate_if_oversized(&[file.as_raw_fd()], 4);

        assert!(!generation(&path, 1).exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "1234");
    }

    /// The Homebrew formula's `log_path` and `error_log_path` name one file,
    /// which launchd opens twice: both descriptions must move together, and
    /// the file must be rotated once, not twice.
    #[test]
    fn two_descriptions_of_one_file_rotate_once_and_both_follow() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nodespaced.log");
        let out = open_append(&path);
        let err = open_append(&path);
        write_fd(out.as_raw_fd(), b"stdout before\n");
        write_fd(err.as_raw_fd(), b"stderr before\n");

        rotate_if_oversized(&[out.as_raw_fd(), err.as_raw_fd()], 4);
        write_fd(out.as_raw_fd(), b"stdout after\n");
        write_fd(err.as_raw_fd(), b"stderr after\n");

        assert_eq!(
            std::fs::read_to_string(generation(&path, 1)).unwrap(),
            "stdout before\nstderr before\n"
        );
        assert!(!generation(&path, 2).exists());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "stdout after\nstderr after\n"
        );
    }

    #[test]
    fn generations_shift_down_and_the_oldest_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nodespaced.log");
        for n in 1..=LOG_KEEP {
            std::fs::write(generation(&path, n), format!("gen {n}")).unwrap();
        }
        let file = open_append(&path);
        write_fd(file.as_raw_fd(), b"live");

        rotate_if_oversized(&[file.as_raw_fd()], 1);

        assert_eq!(
            std::fs::read_to_string(generation(&path, 1)).unwrap(),
            "live"
        );
        for n in 2..=LOG_KEEP {
            assert_eq!(
                std::fs::read_to_string(generation(&path, n)).unwrap(),
                format!("gen {}", n - 1)
            );
        }
        assert!(!generation(&path, LOG_KEEP + 1).exists());
    }

    #[test]
    fn fresh_file_keeps_the_rotated_files_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nodespaced.log");
        let file = open_append(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_fd(file.as_raw_fd(), b"old history\n");

        rotate_if_oversized(&[file.as_raw_fd()], 4);

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn non_regular_fd_is_skipped() {
        let mut fds = [0 as RawFd; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };

        assert!(log_targets(&[write.as_raw_fd()]).is_empty());
        drop((read, write));
    }

    /// A path that no longer names the fd's file — something else already
    /// moved it — must not be rotated on the fd's behalf.
    #[test]
    fn fd_whose_path_was_replaced_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nodespaced.log");
        let file = open_append(&path);
        write_fd(file.as_raw_fd(), b"old history\n");
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "someone else's file").unwrap();

        rotate_if_oversized(&[file.as_raw_fd()], 4);

        assert!(!generation(&path, 1).exists());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "someone else's file"
        );
    }
}
