//! Definitions shared by the desktop app (which writes the bundle marker) and
//! the CLI (which reads it), so the two cannot drift apart.

use std::path::Path;

/// File the desktop app writes beside the `nodespace` copy it stages in
/// `~/.nodespace/bin`: the path of the bundled `nodespace` the copy was made
/// from. The staged copy has no installer or `skill/` beside it, so the CLI
/// follows this path to the bundle that has them.
pub const BUNDLE_MARKER_NAME: &str = ".nodespace.bundle";

/// Whether `exe` sits at `<name>.app/Contents/MacOS/<file>`, the only layout
/// a recorded bundle path is trusted in. Names compare ASCII
/// case-insensitively because the default macOS filesystem does.
pub fn is_app_bundle_exe(exe: &Path) -> bool {
    let Some(macos) = exe.parent() else {
        return false;
    };
    let Some(contents) = macos.parent() else {
        return false;
    };
    let Some(app) = contents.parent() else {
        return false;
    };
    let name_is = |p: &Path, want: &str| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(want))
    };
    name_is(macos, "MacOS")
        && name_is(contents, "Contents")
        && app
            .extension()
            .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("app"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_bundle_layout_in_any_case() {
        assert!(is_app_bundle_exe(Path::new(
            "/A/N.app/Contents/MacOS/nodespace"
        )));
        assert!(is_app_bundle_exe(Path::new(
            "/A/N.APP/contents/macos/nodespace"
        )));
    }

    #[test]
    fn rejects_other_layouts() {
        assert!(!is_app_bundle_exe(Path::new(
            "/A/N/Contents/MacOS/nodespace"
        )));
        assert!(!is_app_bundle_exe(Path::new(
            "/A/N.app/Contents/Resources/nodespace"
        )));
        assert!(!is_app_bundle_exe(Path::new("/A/N.app/MacOS/nodespace")));
        assert!(!is_app_bundle_exe(Path::new("nodespace")));
    }
}
