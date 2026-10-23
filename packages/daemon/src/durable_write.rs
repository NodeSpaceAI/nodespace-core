//! Crash-durable "write a temp file, then rename it into place".
//!
//! A bare `write` + `rename` is atomic against a process kill but not against
//! power loss or a kernel panic: the rename can reach disk before the data and
//! leave a zero-length file at the destination. This flushes the temp file
//! before the rename and the parent directory after it.

use std::io::Write;
use std::path::Path;

/// The durability-relevant operations, behind a seam so tests can observe
/// their order and inject failures. [`RealOps`] performs the real syscalls.
pub(crate) trait DurableOps {
    fn sync_file(&mut self, file: &std::fs::File) -> std::io::Result<()>;
    fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn sync_dir(&mut self, dir: &Path) -> std::io::Result<()>;
}

pub(crate) struct RealOps;

impl DurableOps for RealOps {
    fn sync_file(&mut self, file: &std::fs::File) -> std::io::Result<()> {
        file.sync_all()
    }
    fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }
    #[cfg(unix)]
    fn sync_dir(&mut self, dir: &Path) -> std::io::Result<()> {
        std::fs::File::open(dir)?.sync_all()
    }
    /// Windows cannot open a directory as a file; NTFS journals the rename.
    #[cfg(not(unix))]
    fn sync_dir(&mut self, _dir: &Path) -> std::io::Result<()> {
        Ok(())
    }
}

/// Write `contents` to `tmp`, `sync_all` it, rename it over `path`, then sync
/// the parent directory. `tmp` must be in the same directory as `path`. On a
/// failure before the rename completes, the temp file is removed and `path` is
/// untouched. A failed directory sync (EINVAL/ENOTSUP on some FUSE and
/// network mounts) is only logged: the rename already replaced the file, so
/// reporting an error would make callers believe the save did not happen.
/// Blocking.
pub(crate) fn write_durably(tmp: &Path, path: &Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(test)]
    if let Some(log) = tests::recorder_for(path) {
        return write_durably_with(tmp, path, contents, &mut tests::Recording(log));
    }
    write_durably_with(tmp, path, contents, &mut RealOps)
}

pub(crate) fn write_durably_with(
    tmp: &Path,
    path: &Path,
    contents: &[u8],
    ops: &mut dyn DurableOps,
) -> std::io::Result<()> {
    let staged = (|| {
        let mut file = std::fs::File::create(tmp)?;
        file.write_all(contents)?;
        ops.sync_file(&file)
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }
    if let Err(e) = ops.rename(tmp, path) {
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    if let Err(e) = ops.sync_dir(dir) {
        tracing::warn!(dir = %dir.display(), error = %e, "could not sync directory after rename; continuing");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Step {
        SyncTemp,
        Rename,
        SyncDir,
    }

    pub(crate) type Log = Arc<Mutex<Vec<Step>>>;

    static RECORDERS: Mutex<Option<HashMap<std::path::PathBuf, Log>>> = Mutex::new(None);

    pub(crate) fn recorder_for(path: &Path) -> Option<Log> {
        RECORDERS.lock().unwrap().as_ref()?.get(path).cloned()
    }

    /// Route `write_durably(_, path, _)` through a recording [`DurableOps`]
    /// that still performs the real syscalls. Lets a test prove a caller
    /// (e.g. `Registry::save`) goes through the helper, and in what order.
    pub(crate) fn record_writes_to(path: &Path) -> Log {
        let log = Log::default();
        RECORDERS
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(path.to_path_buf(), log.clone());
        log
    }

    pub(crate) struct Recording(pub(crate) Log);

    impl DurableOps for Recording {
        fn sync_file(&mut self, file: &std::fs::File) -> std::io::Result<()> {
            self.0.lock().unwrap().push(Step::SyncTemp);
            RealOps.sync_file(file)
        }
        fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
            self.0.lock().unwrap().push(Step::Rename);
            RealOps.rename(from, to)
        }
        fn sync_dir(&mut self, dir: &Path) -> std::io::Result<()> {
            self.0.lock().unwrap().push(Step::SyncDir);
            RealOps.sync_dir(dir)
        }
    }

    /// Fails at one step, logging every call.
    struct Failing {
        at: Step,
        calls: Vec<Step>,
    }

    impl DurableOps for Failing {
        fn sync_file(&mut self, file: &std::fs::File) -> std::io::Result<()> {
            self.calls.push(Step::SyncTemp);
            if self.at == Step::SyncTemp {
                return Err(std::io::Error::other("injected"));
            }
            RealOps.sync_file(file)
        }
        fn rename(&mut self, from: &Path, to: &Path) -> std::io::Result<()> {
            self.calls.push(Step::Rename);
            if self.at == Step::Rename {
                return Err(std::io::Error::other("injected"));
            }
            RealOps.rename(from, to)
        }
        fn sync_dir(&mut self, dir: &Path) -> std::io::Result<()> {
            self.calls.push(Step::SyncDir);
            if self.at == Step::SyncDir {
                return Err(std::io::Error::other("injected"));
            }
            RealOps.sync_dir(dir)
        }
    }

    #[test]
    fn syncs_the_file_before_the_rename_and_the_directory_after() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.toml");
        let tmp = dir.path().join("f.toml.tmp");
        let log = record_writes_to(&path);
        write_durably(&tmp, &path, b"new").unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            [Step::SyncTemp, Step::Rename, Step::SyncDir]
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!tmp.exists());
    }

    #[test]
    fn a_failure_before_the_rename_keeps_the_old_file_and_removes_the_temp() {
        for at in [Step::SyncTemp, Step::Rename] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("f.toml");
            let tmp = dir.path().join("f.toml.tmp");
            std::fs::write(&path, b"old").unwrap();
            let mut ops = Failing { at, calls: vec![] };
            let err = write_durably_with(&tmp, &path, b"new", &mut ops).unwrap_err();
            assert_eq!(err.to_string(), "injected");
            assert_eq!(std::fs::read(&path).unwrap(), b"old", "{at:?}");
            assert!(!tmp.exists(), "{at:?}");
        }
    }

    /// The rename already replaced the file, so a directory sync that the
    /// filesystem rejects must not turn the save into an error.
    #[test]
    fn a_directory_sync_failure_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.toml");
        let tmp = dir.path().join("f.toml.tmp");
        let mut ops = Failing {
            at: Step::SyncDir,
            calls: vec![],
        };
        write_durably_with(&tmp, &path, b"new", &mut ops).unwrap();
        assert_eq!(ops.calls, [Step::SyncTemp, Step::Rename, Step::SyncDir]);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }
}
