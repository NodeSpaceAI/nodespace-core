//! Client-side path resolution. The daemon is a separate long-lived process
//! whose working directory is unrelated to the caller's, so every filesystem
//! path the CLI sends over gRPC must already be absolute.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Resolve `path` against `cwd` into an absolute path string. Already-absolute
/// paths are returned unchanged; the filesystem is not touched, so paths that
/// do not exist yet (e.g. a database about to be created) resolve too.
///
/// An empty path is rejected rather than silently meaning `cwd`. A leading `~`
/// is not expanded (the shell does that); an unexpanded one is treated as a
/// literal directory name under `cwd`.
pub fn absolute_in(path: &str, cwd: &Path) -> Result<String> {
    if path.trim().is_empty() {
        bail!("path must not be empty");
    }
    // `components()` drops interior `.` segments and trailing slashes; `..` is
    // kept as-is so a symlinked parent is still resolved by the OS, not guessed
    // lexically.
    Ok(cwd
        .join(path)
        .components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_joined_onto_cwd() {
        let cwd = Path::new("/work/project");
        assert_eq!(
            absolute_in("docs/a.md", cwd).unwrap(),
            "/work/project/docs/a.md"
        );
        assert_eq!(absolute_in("./docs", cwd).unwrap(), "/work/project/docs");
        assert_eq!(
            absolute_in("notes.db", cwd).unwrap(),
            "/work/project/notes.db"
        );
    }

    #[test]
    fn parent_components_resolve_against_cwd() {
        let cwd = Path::new("/work/project");
        assert_eq!(
            absolute_in("../docs", cwd).unwrap(),
            "/work/project/../docs"
        );
    }

    #[test]
    fn empty_path_is_rejected() {
        let cwd = Path::new("/work/project");
        assert!(absolute_in("", cwd).is_err());
        assert!(absolute_in("  ", cwd).is_err());
    }

    #[test]
    fn trailing_slash_and_interior_dot_are_normalised() {
        let cwd = Path::new("/work/project");
        assert_eq!(absolute_in("docs/", cwd).unwrap(), "/work/project/docs");
        assert_eq!(absolute_in("a/./b", cwd).unwrap(), "/work/project/a/b");
    }

    #[test]
    fn absolute_paths_are_unchanged() {
        let cwd = Path::new("/work/project");
        assert_eq!(absolute_in("/etc/x.md", cwd).unwrap(), "/etc/x.md");
    }
}
