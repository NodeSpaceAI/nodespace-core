//! Per-database window size/position persistence. Keyed by database id
//! rather than window label — the id is the stable identity that survives
//! the window itself closing and reopening later, including across app
//! restarts, whereas a label (particularly the bootstrap window's static
//! "main") is not.
//!
//! Uses `atomic_file::write_json` (the same atomic write-to-temp-then-rename
//! mechanics `preferences.rs` uses) but keeps its own file
//! (`window_state.json`) so display/import preferences and window geometry
//! evolve independently.
//!
//! The `*_at` functions below take an explicit directory instead of an
//! `AppHandle` so they are unit-testable against a temp directory without
//! ever touching a real (or even mock) app's actual config directory — the
//! `AppHandle`-taking wrappers ([`load_geometry`], [`save_geometry`]) are
//! thin, untested-in-isolation adapters used only by production call sites.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};
use tokio::fs;

use crate::atomic_file;

const WINDOW_STATE_FILE: &str = "window_state.json";

/// Serializes every `save_geometry_at` call process-wide. Without this, two
/// concurrent saves (e.g. rapid `WindowEvent::Resized` deliveries during a
/// drag spawning one save task per event, or two windows on different
/// databases resizing at once) each run their own read-modify-write cycle
/// against `window_state.json` and both write through the SAME temp path —
/// either race can silently lose one save's update (the second task reads
/// the map before the first task's write lands, then overwrites it on
/// rename) or, at the OS level, interleave writes to the shared temp file.
/// A single global lock is enough because there is only ever one real
/// config directory (and therefore one `window_state.json`) per running app
/// process; it does not attempt to reduce how often a drag gesture triggers
/// a save, only to make concurrent saves correct rather than racy.
static SAVE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One window's saved content (inner) size + frame (outer) position, in
/// physical pixels — no logical-vs-physical (DPI) conversion is needed on
/// either side of the round trip. Size and position are deliberately
/// different halves of the inner/outer distinction: `width`/`height` are
/// captured via `WebviewWindow::inner_size()` and restored via
/// `set_size()`, which `tauri-runtime-wry` dispatches to `Window::
/// set_inner_size` — so both sides of that round trip must be inner, not
/// outer, or every restore inflates the window by the decoration height.
/// `x`/`y` are captured via `outer_position()` and restored via
/// `set_position()` (-> `Window::set_outer_position`), so both sides of
/// that round trip are outer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
}

/// The narrowest sane window size on any desktop platform this app ships
/// on — smaller than this is more likely corrupt or default-initialized
/// data than a real saved size a user resized to. `x`/`y` are deliberately
/// NOT bounds-checked here: negative screen coordinates are legitimate on a
/// multi-monitor setup where a secondary display sits above or left of the
/// primary.
const MIN_PLAUSIBLE_DIMENSION: u32 = 100;

impl WindowGeometry {
    /// Whether this geometry is sane enough to apply to a real window.
    /// Guards [`crate::window_routing::pin_window_database`]'s restore path
    /// against a degenerate saved size (e.g. `0x0`, which a corrupt write or
    /// a future bug could produce) that would leave the window invisible or
    /// effectively unusable with no in-app way to recover it.
    pub fn is_plausible(&self) -> bool {
        self.width >= MIN_PLAUSIBLE_DIMENSION && self.height >= MIN_PLAUSIBLE_DIMENSION
    }
}

/// `database_id -> WindowGeometry`.
type GeometryMap = HashMap<String, WindowGeometry>;

async fn load_all_at(dir: &Path) -> GeometryMap {
    try_load_all_at(dir).await.unwrap_or_default()
}

/// Like [`load_all_at`], but distinguishes the file genuinely not existing
/// yet (nothing has ever been saved — an expected, harmless empty-state
/// case) from any OTHER read failure: EMFILE under fd pressure (e.g. many
/// active PTY/agent sessions holding descriptors open), a transient EPERM
/// from an AV scanner briefly locking the file on Windows, and the like.
///
/// [`save_geometry_at`]'s read-modify-write needs that distinction —
/// [`load_geometry_at`] (a pure read, never destructive) does not, and
/// keeps collapsing both cases to "nothing saved" via [`load_all_at`]
/// above, same as before.
async fn try_load_all_at(dir: &Path) -> std::io::Result<GeometryMap> {
    let path = dir.join(WINDOW_STATE_FILE);
    match fs::read_to_string(&path).await {
        Ok(contents) => Ok(serde_json::from_str(&contents).unwrap_or_default()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GeometryMap::default()),
        Err(e) => Err(e),
    }
}

/// The database's saved geometry, if any was ever recorded for it.
pub async fn load_geometry_at(dir: &Path, database_id: &str) -> Option<WindowGeometry> {
    load_all_at(dir).await.get(database_id).copied()
}

/// Record `database_id`'s current geometry. Read-modify-write against the
/// shared per-database map so windows on different databases never clobber
/// each other's entries — serialized by [`SAVE_LOCK`] so concurrent callers
/// can't race that read-modify-write cycle or the underlying atomic write.
///
/// The read half of that cycle refuses to proceed on a genuine (non-missing)
/// read failure rather than falling back to an empty map the way a pure
/// read does: silently treating a transient failure as "nothing was ever
/// saved" would insert only `database_id`'s entry into that empty map and
/// atomically rename it over the real file, discarding every other
/// database's saved geometry for good. A file that simply doesn't exist yet
/// is not such a failure — the very first save for a fresh install must
/// still succeed and create it.
pub async fn save_geometry_at(
    dir: &Path,
    database_id: &str,
    geometry: WindowGeometry,
) -> Result<(), String> {
    let _guard = SAVE_LOCK.lock().await;

    let mut all = try_load_all_at(dir).await.map_err(|e| {
        format!(
            "refusing to save window geometry: {WINDOW_STATE_FILE} could not be read ({e}) — \
             saving now would silently discard every other database's saved geometry"
        )
    })?;
    all.insert(database_id.to_string(), geometry);

    atomic_file::write_json(dir, WINDOW_STATE_FILE, &all).await
}

fn config_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    app.path().app_config_dir().ok()
}

/// [`load_geometry_at`] against `app`'s real config directory. `None` when
/// the config directory can't be resolved or nothing was ever saved for
/// `database_id`. Generic over `Runtime` (rather than the default `Wry`
/// alias `preferences.rs` uses) so `lib.rs`'s `handle_run_event`, which is
/// itself generic for `tauri::test`'s `MockRuntime`, can call this directly.
pub async fn load_geometry<R: Runtime>(
    app: &AppHandle<R>,
    database_id: &str,
) -> Option<WindowGeometry> {
    let dir = config_dir(app)?;
    load_geometry_at(&dir, database_id).await
}

/// [`save_geometry_at`] against `app`'s real config directory.
pub async fn save_geometry<R: Runtime>(
    app: &AppHandle<R>,
    database_id: &str,
    geometry: WindowGeometry,
) -> Result<(), String> {
    let dir = config_dir(app).ok_or_else(|| "Failed to get config directory".to_string())?;
    save_geometry_at(&dir, database_id, geometry).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_file_yields_no_saved_geometry() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(load_geometry_at(dir.path(), "db-1").await, None);
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let geom = WindowGeometry {
            width: 1024,
            height: 768,
            x: 12,
            y: 34,
        };
        save_geometry_at(dir.path(), "db-1", geom)
            .await
            .expect("save failed");
        assert_eq!(load_geometry_at(dir.path(), "db-1").await, Some(geom));
    }

    #[tokio::test]
    async fn different_databases_do_not_clobber_each_other() {
        let dir = tempfile::tempdir().expect("tempdir");
        let geom_a = WindowGeometry {
            width: 800,
            height: 600,
            x: 0,
            y: 0,
        };
        let geom_b = WindowGeometry {
            width: 1600,
            height: 900,
            x: 100,
            y: 50,
        };
        save_geometry_at(dir.path(), "db-a", geom_a)
            .await
            .expect("save a failed");
        save_geometry_at(dir.path(), "db-b", geom_b)
            .await
            .expect("save b failed");

        assert_eq!(load_geometry_at(dir.path(), "db-a").await, Some(geom_a));
        assert_eq!(load_geometry_at(dir.path(), "db-b").await, Some(geom_b));
    }

    #[tokio::test]
    async fn saving_again_for_the_same_database_overwrites_its_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = WindowGeometry {
            width: 800,
            height: 600,
            x: 0,
            y: 0,
        };
        let second = WindowGeometry {
            width: 1000,
            height: 700,
            x: 10,
            y: 10,
        };
        save_geometry_at(dir.path(), "db-1", first)
            .await
            .expect("save failed");
        save_geometry_at(dir.path(), "db-1", second)
            .await
            .expect("save failed");

        assert_eq!(load_geometry_at(dir.path(), "db-1").await, Some(second));
    }

    #[tokio::test]
    async fn a_corrupt_file_is_treated_as_no_saved_state_rather_than_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join(WINDOW_STATE_FILE), "not valid json")
            .await
            .expect("write failed");
        assert_eq!(load_geometry_at(dir.path(), "db-1").await, None);
    }

    /// A transient, non-missing read failure (EMFILE under fd pressure,
    /// EPERM from an AV scanner briefly locking the file on Windows) must
    /// refuse the save outright rather than silently treating it as "no
    /// saved state" and clobbering the real file with a single-entry map —
    /// discarding every other database's saved geometry.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_transient_non_missing_read_failure_refuses_to_save_rather_than_wiping_the_file() {
        // SAFETY: geteuid() is always safe to call and takes no arguments.
        if unsafe { libc::geteuid() } == 0 {
            // Root ignores Unix permission bits, so the chmod(0) below
            // wouldn't actually induce a read failure under a root test
            // runner — skip rather than false-fail.
            return;
        }

        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let geom_a = WindowGeometry {
            width: 800,
            height: 600,
            x: 0,
            y: 0,
        };
        let geom_b = WindowGeometry {
            width: 1024,
            height: 768,
            x: 10,
            y: 10,
        };
        save_geometry_at(dir.path(), "db-a", geom_a)
            .await
            .expect("save a failed");
        save_geometry_at(dir.path(), "db-b", geom_b)
            .await
            .expect("save b failed");

        let state_path = dir.path().join(WINDOW_STATE_FILE);
        // Reproduce a real transient read failure with a Unix permission
        // bit: `rename` — what the write side of a save uses, via
        // `atomic_file::write_json` — only needs write+execute permission
        // on the *directory*, not read permission on the target file being
        // replaced, so this blocks reads without blocking a would-be
        // clobbering write. That gap is exactly what the fix closes.
        fs::set_permissions(&state_path, std::fs::Permissions::from_mode(0o000))
            .await
            .expect("chmod failed");

        let result = save_geometry_at(
            dir.path(),
            "db-c",
            WindowGeometry {
                width: 640,
                height: 480,
                x: 0,
                y: 0,
            },
        )
        .await;

        // Restore permissions before any assertion can bail the test out
        // early and leave an unreadable temp file behind.
        fs::set_permissions(&state_path, std::fs::Permissions::from_mode(0o644))
            .await
            .expect("chmod restore failed");

        assert!(
            result.is_err(),
            "a non-missing read failure must refuse to save, not silently succeed"
        );
        // Both earlier databases' data must have survived completely
        // untouched — proving the refusal happened before any write was
        // attempted, not after a write that merely happened to fail too.
        assert_eq!(load_geometry_at(dir.path(), "db-a").await, Some(geom_a));
        assert_eq!(load_geometry_at(dir.path(), "db-b").await, Some(geom_b));
        assert_eq!(load_geometry_at(dir.path(), "db-c").await, None);
    }

    #[tokio::test]
    async fn save_leaves_no_temp_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        save_geometry_at(
            dir.path(),
            "db-1",
            WindowGeometry {
                width: 1,
                height: 1,
                x: 0,
                y: 0,
            },
        )
        .await
        .expect("save failed");
        assert!(!dir.path().join(format!("{WINDOW_STATE_FILE}.tmp")).exists());
        assert!(dir.path().join(WINDOW_STATE_FILE).exists());
    }

    #[test]
    fn is_plausible_rejects_degenerate_dimensions_but_allows_negative_position() {
        assert!(WindowGeometry {
            width: 1200,
            height: 800,
            x: 0,
            y: 0
        }
        .is_plausible());
        assert!(
            !WindowGeometry {
                width: 0,
                height: 0,
                x: 0,
                y: 0
            }
            .is_plausible(),
            "a 0x0 saved geometry must never be treated as restorable"
        );
        assert!(
            !WindowGeometry {
                width: 1200,
                height: 1,
                x: 0,
                y: 0
            }
            .is_plausible(),
            "one degenerate dimension is enough to reject the whole geometry"
        );
        // Negative x/y is a legitimate multi-monitor position (a secondary
        // display above/left of the primary) — must not be rejected.
        assert!(WindowGeometry {
            width: 1200,
            height: 800,
            x: -1920,
            y: -200
        }
        .is_plausible());
    }

    /// Proves `SAVE_LOCK` actually prevents the lost-update race: many
    /// concurrent saves for DIFFERENT database ids, all racing the same
    /// read-modify-write-rename cycle against the same file. Without the
    /// lock this reliably loses entries (each task can read the map before
    /// an earlier task's write lands, then overwrite it on rename) — with
    /// it, every entry must survive regardless of scheduling order.
    #[tokio::test]
    async fn concurrent_saves_for_different_databases_all_survive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let mut tasks = Vec::new();
        for i in 0..20u32 {
            let dir_path = dir_path.clone();
            tasks.push(tokio::spawn(async move {
                save_geometry_at(
                    &dir_path,
                    &format!("db-{i}"),
                    WindowGeometry {
                        width: 800 + i,
                        height: 600,
                        x: 0,
                        y: 0,
                    },
                )
                .await
            }));
        }
        for t in tasks {
            t.await.expect("task panicked").expect("save failed");
        }

        for i in 0..20u32 {
            let geom = load_geometry_at(&dir_path, &format!("db-{i}"))
                .await
                .unwrap_or_else(|| panic!("db-{i}'s save was lost to a concurrent-write race"));
            assert_eq!(geom.width, 800 + i);
        }
    }
}
