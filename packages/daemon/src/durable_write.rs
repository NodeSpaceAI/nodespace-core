//! Crash-durable "write a temp file, then rename it into place".
//!
//! A bare `write` + `rename` is atomic against a process kill but not against
//! power loss or a kernel panic: the rename can reach disk before the data and
//! leave a zero-length file at the destination. This flushes the temp file
//! before the rename and the parent directory after it.

use std::io::Write;
use std::path::Path;

/// One step of [`write_durably_with`], reported to the `observe` hook so a test
/// can assert the order (and inject a failure at a chosen step).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    WriteTemp,
    SyncTemp,
    Rename,
    SyncDir,
}

/// Write `contents` to `tmp`, `sync_all` it, rename it over `path`, then sync
/// the parent directory. `tmp` must be in the same directory as `path`. On any
/// failure before the rename completes, the temp file is removed and `path` is
/// untouched. Blocking.
pub(crate) fn write_durably(tmp: &Path, path: &Path, contents: &[u8]) -> std::io::Result<()> {
    write_durably_with(tmp, path, contents, &mut |_| Ok(()))
}

pub(crate) fn write_durably_with(
    tmp: &Path,
    path: &Path,
    contents: &[u8],
    observe: &mut dyn FnMut(Step) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let staged = (|| {
        let mut file = std::fs::File::create(tmp)?;
        file.write_all(contents)?;
        observe(Step::WriteTemp)?;
        file.sync_all()?;
        observe(Step::SyncTemp)
    })();
    if let Err(e) = staged {
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }
    let renamed = observe(Step::Rename).and_then(|()| std::fs::rename(tmp, path));
    if let Err(e) = renamed {
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }
    // The new content is already visible; a failed directory sync only means
    // the rename itself might not survive power loss, so it is still reported.
    observe(Step::SyncDir)?;
    sync_parent_dir(path)
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::File::open(dir)?.sync_all()
}

/// Windows cannot open a directory as a file; NTFS journals the rename.
#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncs_the_file_before_the_rename_and_the_directory_after() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.toml");
        let tmp = dir.path().join("f.toml.tmp");
        let mut steps = Vec::new();
        write_durably_with(&tmp, &path, b"new", &mut |s| {
            steps.push(s);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            steps,
            [Step::WriteTemp, Step::SyncTemp, Step::Rename, Step::SyncDir]
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!tmp.exists());
    }

    #[test]
    fn a_failure_before_the_rename_keeps_the_old_file_and_removes_the_temp() {
        for fail_at in [Step::WriteTemp, Step::SyncTemp, Step::Rename] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("f.toml");
            let tmp = dir.path().join("f.toml.tmp");
            std::fs::write(&path, b"old").unwrap();
            let err = write_durably_with(&tmp, &path, b"new", &mut |s| {
                if s == fail_at {
                    Err(std::io::Error::other("injected"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert_eq!(err.to_string(), "injected");
            assert_eq!(std::fs::read(&path).unwrap(), b"old", "{fail_at:?}");
            assert!(!tmp.exists(), "{fail_at:?}");
        }
    }
}
