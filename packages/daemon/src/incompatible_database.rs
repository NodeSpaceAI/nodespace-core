//! Refusing a database another version of NodeSpace created — without a
//! crash-loop.
//!
//! When the default database's tables do not have the shape this build's DDL
//! defines, `create_schema` refuses it with a [`SchemaMismatch`]. Retrying can
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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use nodespace_core::db::schema::SchemaMismatch;
use nodespace_types::IncompatibleDatabase;

/// This build variant's marker path, under the same state directory the
/// database registry uses (so it follows `NODESPACE_HOME`).
pub fn marker_path() -> Result<PathBuf> {
    Ok(
        crate::nodespace_dir()?.join(nodespace_proto::socket::incompatible_database_name(
            cfg!(debug_assertions),
            cfg!(feature = "pro"),
        )),
    )
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
    tokio::fs::write(&tmp, json)
        .await
        .with_context(|| format!("write {}", tmp.display()))?;
    tokio::fs::rename(&tmp, marker)
        .await
        .with_context(|| format!("rename {} into place", marker.display()))?;
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
pub fn is_incompatible_database(err: &anyhow::Error) -> bool {
    SchemaMismatch::find_in(err).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_core::db::schema::TableShapeMismatch;

    fn mismatch() -> SchemaMismatch {
        SchemaMismatch {
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
}
