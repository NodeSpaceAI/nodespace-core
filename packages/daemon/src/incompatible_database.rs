//! Refusing a database another version of NodeSpace created — without a
//! crash-loop.
//!
//! When the default database's tables do not have the shape this build's DDL
//! defines, or it holds a core type that is not the one this build ships, the
//! store refuses it with a [`SchemaMismatch`]. Retrying can
//! never succeed: NodeSpace does not migrate databases, so the file is the
//! same on every attempt. Yet a daemon that simply fails startup is restarted
//! by its service manager (launchd's `KeepAlive`, systemd's
//! `Restart=on-failure`) every few seconds, forever, with the reason buried in
//! a log file.
//!
//! So on that one failure the daemon records it in a marker file the desktop
//! app reads — which database, and why — and then exits with status 0, which
//! both service managers treat as a deliberate stop rather than a crash (see
//! `write_plist` in the desktop app's `daemon_setup.rs`). The app turns the
//! marker into a message and a way to move the database aside; the next
//! successful open removes it.
//!
//! Everything here lives in the library rather than the `nodespaced` binary so
//! that every daemon built on this crate gets the same behavior: call
//! [`open_default_or_record_refusal`] instead of `DatabaseManager::get_or_open`
//! for the default database, and pass the startup result through
//! [`stop_cleanly_on_incompatible_database`] before returning from `main`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use nodespace_core::db::schema::SchemaMismatch;
use nodespace_types::IncompatibleDatabase;

use crate::services::{DatabaseId, RegistryUnusable};
use crate::{DatabaseManager, DatabaseServices};

/// The marker path for this build flavour, under the same state directory the
/// database registry uses (so it follows `NODESPACE_HOME`).
///
/// The name is the shared one from `nodespace_proto::socket`: every daemon
/// registered under core's service identity writes the same marker, and only
/// the build flavour separates a dev build's marker from a release build's.
pub fn marker_path() -> Result<PathBuf> {
    let name = nodespace_proto::socket::incompatible_database_name(cfg!(debug_assertions));
    Ok(crate::nodespace_dir()?.join(name))
}

/// Open the default database `id` through `manager`, recording a refusal at
/// `marker` when it does not match this build's schema, and clearing a
/// marker left by an earlier refusal when it opens.
///
/// `fallback_path` names the file in the marker if the registry cannot say
/// which path it resolved the default to. The error is returned unchanged
/// either way — [`stop_cleanly_on_incompatible_database`] decides the exit.
pub async fn open_default_or_record_refusal(
    manager: &DatabaseManager,
    id: &DatabaseId,
    fallback_path: &Path,
    marker: &Path,
) -> Result<Arc<DatabaseServices>> {
    match manager.get_or_open(id).await {
        Ok(bundle) => {
            clear(marker).await;
            Ok(bundle)
        }
        Err(e) => {
            if let Some(mismatch) = SchemaMismatch::find_in(&e) {
                let refused = manager
                    .default_database_path()
                    .await
                    .unwrap_or_else(|| fallback_path.to_path_buf());
                if let Err(record_err) = record(marker, &refused, mismatch).await {
                    // The refusal itself is still what gets reported; without
                    // the marker the app only loses the explanation.
                    tracing::warn!(
                        error = format!("{record_err:#}"),
                        "could not write the incompatible-database marker"
                    );
                }
            }
            Err(e)
        }
    }
}

/// Turn a refused, incompatible default database into a clean exit.
///
/// Every other startup failure keeps its non-zero status, and with it the
/// service manager's restart — it might be transient. This one cannot be: the
/// database file is identical on every attempt, and launchd's conditional
/// `KeepAlive` / systemd's `Restart=on-failure` would otherwise respawn the
/// daemon into the same refusal every few seconds, forever. Exiting `0` is
/// what those two treat as a deliberate stop. The marker
/// [`open_default_or_record_refusal`] wrote is how the desktop app learns why.
pub fn stop_cleanly_on_incompatible_database(result: Result<()>) -> Result<()> {
    match result {
        Err(e) if is_incompatible_database(&e) => {
            tracing::error!(
                error = format!("{e:#}"),
                "the daemon cannot open its database: it was created by a different \
                 version of NodeSpace. Stopping without a restart; the NodeSpace app \
                 offers to move the database aside and start fresh."
            );
            Ok(())
        }
        other => other,
    }
}

/// Turn a database registry the daemon cannot use (empty or unparseable
/// `databases.toml`) into a clean exit, for the same reason as
/// [`stop_cleanly_on_incompatible_database`]: the file is identical on every
/// attempt, so a service manager's restart can only repeat the failure every
/// few seconds, forever. The error is logged with the registry path and the
/// recovery step (restore a backup, or delete the file) before the clean exit.
///
/// Unlike the incompatible-database case this writes no marker, so the app
/// shows its generic "daemon not running" state and the log is the
/// explanation. A registry that could not be *read* (permissions, I/O) keeps
/// its non-zero status, since that may be transient.
pub fn stop_cleanly_on_unusable_registry(result: Result<()>) -> Result<()> {
    match result {
        Err(e) if RegistryUnusable::find_in(&e).is_some() => {
            tracing::error!(
                error = format!("{e:#}"),
                "the daemon cannot use its database registry. Stopping without a restart; \
                 fix or delete the file and start NodeSpace again."
            );
            Ok(())
        }
        other => other,
    }
}

/// Write the marker for `database_path` refused with `mismatch`. Written to a
/// sibling temp file and renamed into place, so the app never reads half a
/// record.
pub async fn record(marker: &Path, database_path: &Path, mismatch: &SchemaMismatch) -> Result<()> {
    let record = IncompatibleDatabase {
        database_path: database_path.display().to_string(),
        detail: mismatch.to_string(),
        detected_at: chrono::Utc::now().to_rfc3339(),
    };
    let json =
        serde_json::to_vec_pretty(&record).context("serialize incompatible-database marker")?;
    if let Some(parent) = marker.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let tmp = marker.with_extension("json.tmp");
    let marker_owned = marker.to_path_buf();
    tokio::task::spawn_blocking(move || {
        crate::durable_write::write_durably(&tmp, &marker_owned, &json)
    })
    .await
    .context("incompatible-database marker write task panicked")?
    .with_context(|| format!("write {}", marker.display()))?;
    Ok(())
}

/// Remove the marker, if any. Called once the default database has opened, so
/// a marker left by an earlier refusal never outlives the problem it reports.
pub async fn clear(marker: &Path) {
    match tokio::fs::remove_file(marker).await {
        Ok(()) => {
            tracing::info!(marker = %marker.display(), "removed stale incompatible-database marker")
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(
            marker = %marker.display(),
            error = %e,
            "could not remove incompatible-database marker"
        ),
    }
}

/// Whether a startup failure is the database refusal described in the module
/// docs — the one failure the daemon answers with a clean exit instead of an
/// error status. Anything else still exits non-zero, so a transient failure
/// keeps its restart.
fn is_incompatible_database(err: &anyhow::Error) -> bool {
    SchemaMismatch::find_in(err).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_core::db::schema::TableShapeMismatch;

    fn mismatch() -> SchemaMismatch {
        SchemaMismatch {
            missing_tables: vec![],
            unexpected_tables: vec![],
            core_types: vec![],
            tables: vec![TableShapeMismatch {
                table: "relationship".to_string(),
                missing_columns: vec!["reverse_relationship_type".to_string()],
                unexpected_columns: vec![],
            }],
        }
    }

    #[tokio::test]
    async fn record_then_clear_round_trips_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir
            .path()
            .join(".nodespace")
            .join("incompatible-database.json");
        let db = dir.path().join("nodespace.db");

        record(&marker, &db, &mismatch()).await.unwrap();
        let read: IncompatibleDatabase =
            serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        assert_eq!(read.database_path, db.display().to_string());
        assert!(read
            .detail
            .contains("relationship: missing reverse_relationship_type"));
        assert!(
            !marker.with_extension("json.tmp").exists(),
            "temp file must be renamed away"
        );

        clear(&marker).await;
        assert!(!marker.exists());
        // Clearing an absent marker is a no-op, not an error.
        clear(&marker).await;
    }

    #[tokio::test]
    async fn record_goes_through_the_durable_write_helper() {
        use crate::durable_write::tests::{record_writes_to, Step};
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("incompatible-database.json");
        let log = record_writes_to(&marker);
        record(&marker, &dir.path().join("nodespace.db"), &mismatch())
            .await
            .unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            [Step::SyncTemp, Step::Rename, Step::SyncDir]
        );
    }

    /// The heart of the crash-loop fix: an incompatible database must end
    /// startup with `Ok` (exit 0, no restart), and every other failure must
    /// keep its error (non-zero exit, restart).
    #[test]
    fn only_an_incompatible_database_turns_a_failed_startup_into_a_clean_exit() {
        let refused = anyhow::Error::new(mismatch())
            .context("Failed to create database schema")
            .context("opening database default");
        assert!(stop_cleanly_on_incompatible_database(Err(refused)).is_ok());

        let transient = stop_cleanly_on_incompatible_database(Err(anyhow::anyhow!(
            "SQLite failure: database is locked"
        )));
        assert_eq!(
            transient.unwrap_err().to_string(),
            "SQLite failure: database is locked"
        );

        assert!(stop_cleanly_on_incompatible_database(Ok(())).is_ok());
    }

    #[test]
    fn the_marker_is_the_shared_name_for_this_build_flavour() {
        let expected = nodespace_proto::socket::incompatible_database_name(cfg!(debug_assertions));
        let marker = marker_path().unwrap();
        assert_eq!(
            marker.file_name().and_then(|name| name.to_str()),
            Some(expected)
        );
    }

    #[test]
    fn only_a_schema_mismatch_is_treated_as_an_incompatible_database() {
        let wrapped = anyhow::Error::new(mismatch())
            .context("Failed to create database schema")
            .context("opening database default");
        assert!(is_incompatible_database(&wrapped));
        assert!(!is_incompatible_database(&anyhow::anyhow!(
            "SQLite failure: database is locked"
        )));
    }

    /// An empty or unparseable registry ends the daemon with exit status 0 so
    /// launchd/systemd do not respawn it into the same failure. A registry that
    /// merely cannot be read keeps its non-zero status.
    #[tokio::test]
    async fn an_unusable_registry_stops_cleanly_but_other_failures_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("databases.toml");

        for body in ["", "  \n", "databases = [[[ not toml"] {
            std::fs::write(&path, body).unwrap();
            let err = crate::services::Registry::load(&path).await.unwrap_err();
            assert!(
                stop_cleanly_on_unusable_registry(Err(err)).is_ok(),
                "{body:?} must stop cleanly"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), body, "left alone");
        }

        let wrapped = anyhow::Error::new(RegistryUnusable {
            path: path.clone(),
            detail: "empty".into(),
        })
        .context("loading registry");
        assert!(stop_cleanly_on_unusable_registry(Err(wrapped)).is_ok());

        // A directory where the file should be is a read failure, not content.
        let unreadable = dir.path().join("dir-registry");
        std::fs::create_dir(&unreadable).unwrap();
        let err = crate::services::Registry::load(&unreadable)
            .await
            .unwrap_err();
        assert!(stop_cleanly_on_unusable_registry(Err(err)).is_err());
        assert!(stop_cleanly_on_unusable_registry(Err(anyhow::anyhow!("other"))).is_err());
        assert!(stop_cleanly_on_unusable_registry(Ok(())).is_ok());
    }
}
