//! A database the daemon refused because another version of NodeSpace created
//! it — telling the user, and moving it aside so NodeSpace can start fresh.
//!
//! NodeSpace does not migrate databases between versions: when the default
//! database does not match the current schema, `nodespaced` refuses to
//! open it, records why in a marker file (`nodespace_proto::socket::
//! incompatible_database_name`, shape [`IncompatibleDatabase`]) and exits
//! cleanly so its service manager does not restart it into the same failure.
//! This module is the app's half of that contract:
//!
//! - [`daemon_down_status`] turns "the daemon is not running" into
//!   `incompatible_database` when the marker says why, so the frontend shows
//!   an explanation instead of the generic not-running banner.
//! - [`reset_incompatible_database`] moves the refused file aside — renamed
//!   next to itself with a timestamp, never deleted — and starts the daemon
//!   again, which creates a fresh database at the original path.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use nodespace_types::IncompatibleDatabase;
use serde::Serialize;
use tauri::AppHandle;

use crate::daemon_setup::{self, DaemonStatus};

/// Status string [`daemon_down_status`] reports when the marker is present.
pub const INCOMPATIBLE_DATABASE_STATUS: &str = "incompatible_database";

/// This build flavour's marker path, in the `.nodespace/` state directory
/// under `home`. The daemon derives the same path from its own build flavour.
fn marker_path_in(home: &Path) -> PathBuf {
    home.join(nodespace_proto::socket::STATE_DIR).join(
        nodespace_proto::socket::incompatible_database_name(cfg!(debug_assertions)),
    )
}

fn marker_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| marker_path_in(&home))
}

/// The refusal recorded at `marker`, if any. A marker that cannot be parsed
/// is treated as absent (and logged): the generic not-running banner is the
/// honest fallback when the explanation itself is unreadable.
fn read_marker(marker: &Path) -> Option<IncompatibleDatabase> {
    let bytes = match std::fs::read(marker) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(marker = %marker.display(), error = %e, "could not read incompatible-database marker");
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(record) => Some(record),
        Err(e) => {
            tracing::warn!(marker = %marker.display(), error = %e, "could not parse incompatible-database marker");
            None
        }
    }
}

/// The database the daemon last refused, if the refusal still stands.
pub fn current() -> Option<IncompatibleDatabase> {
    marker_path().and_then(|marker| read_marker(&marker))
}

/// The status string for this app's daemon not running: why, when that is
/// known, and the generic `not_running` otherwise. Another daemon holding the
/// socket ([`crate::coexistence`]) comes first: this app's daemon never got to
/// open a database then.
pub fn daemon_down_status() -> &'static str {
    if crate::coexistence::other_daemon().is_some() {
        crate::coexistence::OTHER_DAEMON_STATUS
    } else if current().is_some() {
        INCOMPATIBLE_DATABASE_STATUS
    } else {
        "not_running"
    }
}

/// Remove the marker. Missing is fine — the daemon may already have cleared it.
fn remove_marker(marker: &Path) -> Result<()> {
    match std::fs::remove_file(marker) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", marker.display())),
    }
}

/// Remove a marker left by an earlier refusal before starting the daemon, so
/// the next start attempt's wait for the daemon
/// ([`crate::daemon_setup::answering_daemon`]) only stops early for a refusal
/// *this* start produced. The daemon rewrites it if the database is still
/// incompatible.
pub fn clear_stale_marker() {
    if let Some(marker) = marker_path() {
        if let Err(e) = remove_marker(&marker) {
            tracing::warn!(
                error = format!("{e:#}"),
                "could not clear incompatible-database marker"
            );
        }
    }
}

/// Whether the daemon has recorded a refusal. Used to stop waiting for a
/// daemon that has already stopped on purpose.
pub fn refusal_recorded() -> bool {
    marker_path().is_some_and(|marker| marker.exists())
}

/// How many same-second backup names [`move_database_aside`] tries before
/// giving up. Far beyond any real collision; bounds the search regardless.
const MAX_BACKUP_NAME_ATTEMPTS: u32 = 1000;

/// Rename `database` and its SQLite sidecars (`-wal`, `-shm`) to
/// `<name>.incompatible-<stamp>` beside it, and return the new database path.
///
/// Nothing is deleted. The sidecars move first and under the same new base
/// name, so the backup stays a complete, openable SQLite database, and a
/// fresh database created at the original path can never pick up the old
/// one's write-ahead log. If the database itself then fails to move, the
/// sidecars are moved back.
///
/// Returns `Ok(None)` when there is nothing at `database` at all — someone
/// already moved it aside by hand — so the reset can still go on to start the
/// daemon fresh instead of stranding the user behind a banner that can never
/// clear. Leftover sidecars at that path are still moved aside, but with no
/// database file there is no backup database to name, so the result is `None`
/// then too.
fn move_database_aside(database: &Path, stamp: &str) -> Result<Option<PathBuf>> {
    let present = match std::fs::symlink_metadata(database) {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => bail!(
            "{} is not a regular file; not moving it",
            database.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            return Err(e).with_context(|| format!("inspect {}", database.display()));
        }
    };
    let sidecars_present = ["-wal", "-shm"]
        .iter()
        .any(|suffix| sidecar(database, suffix).exists());
    if !present && !sidecars_present {
        return Ok(None);
    }
    let name = database
        .file_name()
        .context("database path has no file name")?
        .to_string_lossy()
        .into_owned();

    let backup = (0..MAX_BACKUP_NAME_ATTEMPTS)
        .map(|n| {
            let suffix = if n == 0 {
                String::new()
            } else {
                format!("-{n}")
            };
            database.with_file_name(format!("{name}.incompatible-{stamp}{suffix}"))
        })
        .find(|candidate| {
            !candidate.exists()
                && !sidecar(candidate, "-wal").exists()
                && !sidecar(candidate, "-shm").exists()
        })
        .with_context(|| {
            format!(
                "no free backup name beside {} after {MAX_BACKUP_NAME_ATTEMPTS} attempts",
                database.display()
            )
        })?;

    let mut moved_sidecars = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let from = sidecar(database, suffix);
        if from.exists() {
            let to = sidecar(&backup, suffix);
            if let Err(e) = std::fs::rename(&from, &to) {
                restore(&moved_sidecars);
                return Err(e)
                    .with_context(|| format!("move {} to {}", from.display(), to.display()));
            }
            moved_sidecars.push((from, to));
        }
    }
    if !present {
        for (from, to) in &moved_sidecars {
            tracing::info!(from = %from.display(), to = %to.display(), "moved orphaned database sidecar aside");
        }
        return Ok(None);
    }
    if let Err(e) = std::fs::rename(database, &backup) {
        restore(&moved_sidecars);
        return Err(e)
            .with_context(|| format!("move {} to {}", database.display(), backup.display()));
    }
    Ok(Some(backup))
}

fn sidecar(database: &Path, suffix: &str) -> PathBuf {
    let mut name = database.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn restore(moved: &[(PathBuf, PathBuf)]) {
    for (original, moved_to) in moved {
        if let Err(e) = std::fs::rename(moved_to, original) {
            tracing::error!(
                from = %moved_to.display(),
                to = %original.display(),
                error = %e,
                "could not move a database sidecar back after a failed move-aside"
            );
        }
    }
}

/// The database the daemon refused, for the frontend's explanation. `None`
/// when there is no standing refusal.
#[tauri::command]
pub async fn get_incompatible_database() -> Option<IncompatibleDatabase> {
    current()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetIncompatibleDatabaseResult {
    /// Where the refused database now lives, or `None` when it was already
    /// gone (moved by hand) and there was nothing to move.
    pub backup_path: Option<String>,
    /// The daemon's status after restarting on a fresh database — the same
    /// strings `check_daemon_status` reports.
    pub status: String,
}

/// Move the refused database aside and start the daemon on a fresh one.
///
/// Refuses when there is no recorded refusal, or when the daemon is up or
/// coming up (anything but not-running): then the file at that path may be
/// open, or may no longer be the one the daemon refused.
#[tauri::command]
pub async fn reset_incompatible_database(
    app: AppHandle,
) -> Result<ResetIncompatibleDatabaseResult, String> {
    reset(&app).await.map_err(|e| format!("{e:#}"))
}

async fn reset(app: &AppHandle) -> Result<ResetIncompatibleDatabaseResult> {
    let marker = marker_path().context("cannot resolve the home directory")?;
    let record = read_marker(&marker).context("there is no incompatible database to reset")?;

    let socket_path = crate::services::grpc_client::resolve_socket_path();
    if daemon_setup::check_daemon_socket(socket_path.as_path()).await != DaemonStatus::NotRunning {
        bail!(
            "the NodeSpace background service is running or starting, so its database may be \
             in use; try again once it has stopped"
        );
    }

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let backup = move_database_aside(Path::new(&record.database_path), &stamp)?;
    match &backup {
        Some(backup) => tracing::info!(
            from = %record.database_path,
            to = %backup.display(),
            "moved incompatible database aside; starting fresh"
        ),
        None => tracing::info!(
            path = %record.database_path,
            "incompatible database is already gone; starting fresh"
        ),
    }
    remove_marker(&marker)?;

    // No interim "starting" emit: the frontend keeps the banner (and its
    // in-progress state) up until this command returns, then applies the
    // final status this emits.
    let status = daemon_setup::start_daemon_and_report(app).await;

    Ok(ResetIncompatibleDatabaseResult {
        backup_path: backup.map(|b| b.display().to_string()),
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn moves_the_database_and_its_sidecars_aside_under_one_name() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("nodespace.db");
        write(&db, "main");
        write(&sidecar(&db, "-wal"), "wal");
        write(&sidecar(&db, "-shm"), "shm");

        let backup = move_database_aside(&db, "20260928-101500")
            .unwrap()
            .unwrap();

        assert_eq!(
            backup,
            dir.path().join("nodespace.db.incompatible-20260928-101500")
        );
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "main");
        assert_eq!(
            std::fs::read_to_string(sidecar(&backup, "-wal")).unwrap(),
            "wal"
        );
        assert_eq!(
            std::fs::read_to_string(sidecar(&backup, "-shm")).unwrap(),
            "shm"
        );
        assert!(!db.exists());
        assert!(
            !sidecar(&db, "-wal").exists(),
            "an old WAL must not be left for a fresh database"
        );
        assert!(!sidecar(&db, "-shm").exists());
    }

    #[test]
    fn never_overwrites_an_earlier_backup() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("nodespace.db");
        write(&db, "first");
        let first = move_database_aside(&db, "20260928-101500")
            .unwrap()
            .unwrap();
        write(&db, "second");
        let second = move_database_aside(&db, "20260928-101500")
            .unwrap()
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "second");
    }

    /// A database the user already moved by hand is not an error: the reset
    /// must still go on to start fresh, or the banner could never clear.
    #[test]
    fn an_already_missing_database_is_nothing_to_move_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            move_database_aside(&dir.path().join("missing.db"), "s").unwrap(),
            None
        );
    }

    /// Sidecars left behind without their database still move aside, so a
    /// fresh database at that path never opens onto a stale WAL.
    #[test]
    fn orphaned_sidecars_are_moved_aside_even_without_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("nodespace.db");
        write(&sidecar(&db, "-wal"), "wal");

        assert_eq!(
            move_database_aside(&db, "s").unwrap(),
            None,
            "with no database file there is no backup database to name"
        );

        assert!(!sidecar(&db, "-wal").exists());
        let moved = dir.path().join("nodespace.db.incompatible-s-wal");
        assert_eq!(std::fs::read_to_string(moved).unwrap(), "wal");
    }

    #[test]
    fn refuses_to_move_anything_but_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_file = dir.path().join("dir.db");
        std::fs::create_dir(&not_a_file).unwrap();
        assert!(move_database_aside(&not_a_file, "s").is_err());
        assert!(
            not_a_file.is_dir(),
            "a refused move must leave the path alone"
        );
    }

    #[test]
    fn reads_the_marker_the_daemon_writes_and_ignores_a_garbled_one() {
        let dir = tempfile::tempdir().unwrap();
        let marker = marker_path_in(dir.path());
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        assert_eq!(read_marker(&marker), None);

        write(
            &marker,
            r#"{"databasePath":"/x/nodespace.db","detail":"relationship: missing reverse_relationship_type","detectedAt":"2026-09-28T10:00:00Z"}"#,
        );
        assert_eq!(
            read_marker(&marker).map(|r| r.database_path),
            Some("/x/nodespace.db".to_string())
        );

        write(&marker, "{ not json");
        assert_eq!(read_marker(&marker), None);

        remove_marker(&marker).unwrap();
        remove_marker(&marker).expect("removing an absent marker is not an error");
    }

    #[test]
    fn the_marker_lives_in_the_state_directory_under_this_flavours_name() {
        let marker = marker_path_in(Path::new("/home/u"));
        assert_eq!(marker.parent(), Some(Path::new("/home/u/.nodespace")));
        assert_eq!(
            marker.file_name().and_then(|name| name.to_str()),
            Some(nodespace_proto::socket::incompatible_database_name(cfg!(
                debug_assertions
            )))
        );
    }
}
