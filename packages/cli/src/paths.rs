//! Client-side path resolution. The daemon is a separate long-lived process
//! whose working directory is unrelated to the caller's, so every filesystem
//! path the CLI sends over gRPC must already be absolute.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Resolve `path` against `cwd` into an absolute path string. Already-absolute
/// paths are returned unchanged; the filesystem is not touched, so paths that
/// do not exist yet (e.g. a database about to be created) resolve too.
pub fn absolute_in(path: &str, cwd: &Path) -> String {
    // `components()` drops interior `.` segments; `..` is kept as-is so a
    // symlinked parent is still resolved by the OS, not guessed lexically.
    cwd.join(path)
        .components()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

/// Resolve `path` against the CLI process's current working directory.
pub fn absolute(path: &str) -> Result<String> {
    let cwd = std::env::current_dir().context("Failed to determine the current directory")?;
    Ok(absolute_in(path, &cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_joined_onto_cwd() {
        let cwd = Path::new("/work/project");
        assert_eq!(absolute_in("docs/a.md", cwd), "/work/project/docs/a.md");
        assert_eq!(absolute_in("./docs", cwd), "/work/project/docs");
        assert_eq!(absolute_in("notes.db", cwd), "/work/project/notes.db");
    }

    #[test]
    fn parent_components_resolve_against_cwd() {
        let cwd = Path::new("/work/project");
        assert_eq!(absolute_in("../docs", cwd), "/work/project/../docs");
    }

    #[test]
    fn absolute_paths_are_unchanged() {
        let cwd = Path::new("/work/project");
        assert_eq!(absolute_in("/etc/x.md", cwd), "/etc/x.md");
    }
}
