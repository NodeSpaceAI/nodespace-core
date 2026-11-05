//! Definitions shared by the desktop app (which writes the bundle marker) and
//! the CLI (which reads it), so the two cannot drift apart.

use std::path::Path;

/// File the desktop app writes beside the `nodespace` copy it stages in
/// `~/.nodespace/bin`: the path of the bundled `nodespace` the copy was made
/// from. The staged copy has no installer or `skill/` beside it, so the CLI
/// follows this path to the bundle that has them.
pub const BUNDLE_MARKER_NAME: &str = ".nodespace.bundle";

/// The desktop app's own executable in a Windows install directory, where the
/// bundled `nodespace`, `nodespaced`, `nodespace-skill-installer` and the
/// `resources/skill` tree sit beside it.
pub const WINDOWS_APP_EXE_NAME: &str = "nodespace-app.exe";

/// The bundled `nodespace` in a Windows install directory.
const WINDOWS_CLI_EXE_NAME: &str = "nodespace.exe";

/// Whether `exe` is the `nodespace` of an installed desktop app, the only
/// layouts a recorded bundle path is trusted in: [`is_app_bundle_exe`] on
/// macOS, [`is_windows_install_exe`] on Windows.
pub fn is_app_install_exe(exe: &Path) -> bool {
    is_app_bundle_exe(exe) || is_windows_install_exe(exe)
}

/// Whether `exe` is `nodespace.exe` in a directory that also holds the
/// desktop app's executable ([`WINDOWS_APP_EXE_NAME`]), as the Windows
/// installer lays them out (`<install dir>\nodespace.exe` beside
/// `<install dir>\nodespace-app.exe`). The rule reads names and one sibling
/// file, never a drive or `\\?\` prefix, so it holds for the paths
/// `canonicalize` returns on Windows and works on any platform.
pub fn is_windows_install_exe(exe: &Path) -> bool {
    exe.file_name().is_some_and(|n| {
        n.to_string_lossy()
            .eq_ignore_ascii_case(WINDOWS_CLI_EXE_NAME)
    }) && exe
        .parent()
        .is_some_and(|dir| dir.join(WINDOWS_APP_EXE_NAME).is_file())
}

/// Whether `exe` sits at `<name>.app/Contents/MacOS/<file>`, the macOS layout
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

    /// Lays out a Windows install directory as the installer does.
    fn windows_install(dir: &Path) -> std::path::PathBuf {
        std::fs::write(dir.join(WINDOWS_APP_EXE_NAME), b"").expect("app exe");
        let cli = dir.join("nodespace.exe");
        std::fs::write(&cli, b"").expect("cli exe");
        cli
    }

    #[test]
    fn accepts_the_windows_install_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cli = windows_install(dir.path());
        assert!(is_windows_install_exe(&cli));
        assert!(is_app_install_exe(&cli));
        // File names compare case-insensitively, as the Windows filesystem does.
        assert!(is_windows_install_exe(&dir.path().join("NODESPACE.EXE")));
    }

    #[test]
    fn rejects_other_windows_layouts() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No app executable beside it: the staged copy's own directory.
        std::fs::write(dir.path().join("nodespace.exe"), b"").expect("cli exe");
        assert!(!is_app_install_exe(&dir.path().join("nodespace.exe")));
        // The app executable is there, but the file is some other program.
        let app = tempfile::tempdir().expect("tempdir");
        windows_install(app.path());
        assert!(!is_app_install_exe(&app.path().join("nodespaced.exe")));
        assert!(!is_app_install_exe(&app.path().join("nodespace")));
        // A directory named like the app exe is not the app exe.
        let fake = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(fake.path().join(WINDOWS_APP_EXE_NAME)).expect("dir");
        assert!(!is_app_install_exe(&fake.path().join("nodespace.exe")));
    }
}
