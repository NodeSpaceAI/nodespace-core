//! Versioned DDL migration runner.
//!
//! This is the SQL schema bootstrap — how a database gets its tables at all — and is
//! *not* the kind of migration the project's zero-users rule prohibits. That rule bans
//! **data** migrations: startup backfills and format upgrades that carry existing rows
//! from an old shape to a new one. Under zero users the answer to "how do existing
//! databases get the new shape?" is to change the shape and reset the database, never
//! to write a backfill. A fresh database still needs its tables created, which is what
//! this runner does. The shared word "migration" is the only thing the two have in common.
//!
//! Schema changes are tracked via SQLite's `PRAGMA user_version` (an integer stored
//! in the database file header). On startup, `run` compares the database's current
//! version against the known migrations and applies any with a higher version, in
//! order, each in its own transaction that also bumps `user_version` — so a crash
//! mid-migration leaves the database at a consistent, re-resumable version.
//!
//! Adding a schema change: add a new `vNNN_description` module below, add its
//! version to `LATEST_VERSION`, and add a matching arm to `apply_migration`. Never
//! edit a past migration's `apply` fn once it has shipped — existing databases will
//! already have applied it under its old definition, and rerunning a changed
//! version won't happen (each migration runs at most once per database).

mod v001_initial_schema;
mod v002_embedding_origin;
mod v003_property_indexes;
mod v004_schema_relationship_edges;
mod v005_conflict_journal;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Highest migration version known to this build. Bump when adding a migration.
pub const LATEST_VERSION: i64 = 5;

/// Pre-migration backups retained per database. A new release that ships schema
/// changes writes one snapshot before touching the existing data; older ones are
/// pruned so backups don't grow without bound.
const MAX_BACKUPS: usize = 5;

async fn apply_migration(tx: &libsql::Transaction, version: i64) -> Result<()> {
    match version {
        1 => v001_initial_schema::apply(tx).await,
        2 => v002_embedding_origin::apply(tx).await,
        3 => v003_property_indexes::apply(tx).await,
        4 => v004_schema_relationship_edges::apply(tx).await,
        5 => v005_conflict_journal::apply(tx).await,
        _ => unreachable!("no migration defined for version {version}"),
    }
}

fn migration_name(version: i64) -> &'static str {
    match version {
        1 => "initial_schema",
        2 => "embedding_origin",
        3 => "property_indexes",
        4 => "schema_relationship_edges",
        5 => "conflict_journal",
        _ => "unknown",
    }
}

/// Read `PRAGMA user_version` (0 on a brand-new database).
async fn current_version(conn: &libsql::Connection) -> Result<i64> {
    let mut rows = conn
        .query("PRAGMA user_version", ())
        .await
        .context("Failed to read PRAGMA user_version")?;
    let version: i64 = rows
        .next()
        .await?
        .map(|row| row.get(0))
        .transpose()?
        .unwrap_or(0);
    Ok(version)
}

/// Apply all migrations with `current_version < version <= target_version`, in
/// ascending order. Each migration runs in its own transaction that also bumps
/// `user_version` to that migration's version, so a database is never left
/// between two versions.
///
/// Errors if `current_version > target_version` — a database written by a
/// newer build than this one — rather than silently doing nothing.
pub async fn run_up_to(conn: &libsql::Connection, target_version: i64) -> Result<()> {
    let start_version = current_version(conn).await?;

    // A version higher than we know how to migrate means this database was last
    // written by a newer build. Without this check the range below is empty, the
    // loop never runs, and `run` returns `Ok(())` — silently opening the store
    // against a schema this build doesn't understand instead of failing loudly.
    if start_version > target_version {
        bail!(
            "Database schema version {start_version} is newer than this build supports \
             ({target_version}). Update NodeSpace, or reopen this store with the version \
             that wrote it."
        );
    }

    for version in (start_version + 1)..=target_version {
        let tx = conn
            .transaction()
            .await
            .with_context(|| format!("Failed to begin transaction for migration {version}"))?;

        apply_migration(&tx, version).await.with_context(|| {
            format!(
                "Migration {:03}_{} failed",
                version,
                migration_name(version)
            )
        })?;

        tx.execute(&format!("PRAGMA user_version = {version}"), ())
            .await
            .with_context(|| format!("Failed to bump user_version to {version}"))?;

        tx.commit()
            .await
            .with_context(|| format!("Failed to commit migration {version}"))?;
    }

    Ok(())
}

/// Apply all pending migrations, bringing the database to [`LATEST_VERSION`].
pub async fn run(conn: &libsql::Connection) -> Result<()> {
    run_up_to(conn, LATEST_VERSION).await
}

/// Snapshot the database file BEFORE any pending migration runs, so a new release
/// whose migration is destructive or buggy leaves the pre-update data recoverable.
///
/// This is the data-safety guarantee for app updates: the local store already
/// lives outside the app bundle (so replacing the app never touches it), and each
/// migration is transactional (so a crash mid-migration is safe) — this closes the
/// remaining gap, a *committed* bad migration, by keeping a copy of the data as it
/// was under the previous release.
///
/// Fires only when a migration will actually run (`user_version < LATEST_VERSION`)
/// AND the database holds data worth protecting. It does NOT assume `0 == empty`:
/// a pre-versioning database can sit at `user_version 0` with a full schema+data
/// (`v001` is `CREATE TABLE IF NOT EXISTS`), so the gate is real data (a populated
/// `node` table), not the version number — which also avoids writing empty backups.
/// Returns the backup path when one was written.
///
/// The snapshot is taken with `VACUUM INTO`, SQLite's consistent hot-backup: it
/// reads under a transaction, so the copy is transactionally complete even if
/// another connection (the daemon while the CLI opens the same file) holds the
/// database or has un-checkpointed WAL frames. A plain file copy could miss WAL
/// data (a busy checkpoint truncates nothing) or capture a torn file — the exact
/// silent-partial-backup failure this feature must not have.
///
/// Callers treat this as best-effort — a backup failure is logged and must not
/// block startup (the additive migrations shipped to date do not lose data; this
/// is defense in depth for future ones).
pub async fn backup_before_pending_migrations(
    conn: &libsql::Connection,
    db_path: &Path,
) -> Result<Option<PathBuf>> {
    let current = current_version(conn).await?;
    if current >= LATEST_VERSION || !db_path.exists() || !db_has_data(conn).await {
        return Ok(None);
    }

    let dir = db_path.parent().unwrap_or_else(|| Path::new("."));
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups)
        .with_context(|| format!("Failed to create backup dir {}", backups.display()))?;

    let stem = db_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("database");
    // Nanoseconds + pid so two processes (daemon + CLI) backing up in the same
    // second cannot collide on one dest and corrupt each other's copy.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dest = backups.join(format!("{stem}.v{current}.{ts}.{}.bak", std::process::id()));

    let dest_str = dest.to_string_lossy().to_string();
    conn.execute("VACUUM INTO ?1", libsql::params![dest_str])
        .await
        .with_context(|| format!("VACUUM INTO {} failed", dest.display()))?;

    prune_backups(&backups, stem, MAX_BACKUPS);
    tracing::info!(
        from_version = current,
        to_version = LATEST_VERSION,
        backup = %dest.display(),
        "Backed up database before applying pending migrations"
    );
    Ok(Some(dest))
}

/// Whether the database holds user data worth snapshotting — a populated `node`
/// table (the primary content table, created by migration v001). An absent table
/// (query errors) or zero rows means a freshly-created database with nothing to
/// protect, so no backup is written.
async fn db_has_data(conn: &libsql::Connection) -> bool {
    let mut rows = match conn.query("SELECT count(*) FROM node", ()).await {
        Ok(rows) => rows,
        Err(_) => return false, // table not created yet → brand-new database
    };
    matches!(rows.next().await, Ok(Some(row)) if row.get::<i64>(0).unwrap_or(0) > 0)
}

/// Keep only the newest `keep` backups for `stem`, deleting older ones by mtime.
fn prune_backups(dir: &Path, stem: &str, keep: usize) {
    let prefix = format!("{stem}.v");
    let mut backups: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| n.starts_with(&prefix) && n.ends_with(".bak"))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => return,
    };
    if backups.len() <= keep {
        return;
    }
    backups.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    let remove = backups.len() - keep;
    for p in backups.into_iter().take(remove) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards against the panic in `apply_migration`'s `unreachable!` arm: if
    /// `LATEST_VERSION` is bumped without adding a matching `apply_migration` arm,
    /// `run` would panic against a real user database instead of failing a build.
    /// This test converts that mistake into a compile-time-adjacent, always-run
    /// test failure instead.
    #[tokio::test]
    async fn every_version_up_to_latest_has_a_defined_migration() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let db_path = temp_dir.path().join("version_coverage.db");
        crate::db::ensure_sqlite_vec_registered().await;
        let database = libsql::Builder::new_local(&db_path).build().await.unwrap();
        let conn = database.connect().unwrap();

        // Panics via `unreachable!` if any version in range lacks an arm.
        run_up_to(&conn, LATEST_VERSION).await.unwrap();
    }

    async fn open(path: &std::path::Path) -> libsql::Connection {
        crate::db::ensure_sqlite_vec_registered().await;
        libsql::Builder::new_local(path)
            .build()
            .await
            .unwrap()
            .connect()
            .unwrap()
    }

    async fn user_version(conn: &libsql::Connection) -> i64 {
        current_version(conn).await.unwrap()
    }

    async fn node_count(conn: &libsql::Connection) -> i64 {
        let mut rows = conn.query("SELECT count(*) FROM node", ()).await.unwrap();
        rows.next().await.unwrap().unwrap().get(0).unwrap()
    }

    async fn insert_marker(conn: &libsql::Connection) {
        conn.execute(
            "INSERT INTO node (id, node_type, content, created_at, modified_at) \
             VALUES ('marker', 'text', 'keep me', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn backs_up_an_existing_db_with_data_before_pending_migrations() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("data.db");
        let conn = open(&db_path).await;
        // An existing database from a prior release: migrated up to LATEST-1 with
        // real data, so a new release (LATEST) has a pending migration to run.
        run_up_to(&conn, LATEST_VERSION - 1).await.unwrap();
        insert_marker(&conn).await;
        assert_eq!(user_version(&conn).await, LATEST_VERSION - 1);

        let backup = backup_before_pending_migrations(&conn, &db_path)
            .await
            .unwrap()
            .expect("a pending migration over a populated db must produce a backup");
        assert!(backup.exists(), "backup file must be written");
        assert!(backup.starts_with(dir.path().join("backups")));

        // The snapshot must be the database AS IT WAS pre-migration — same version
        // AND the data intact — independent of the live db (migrated forward next).
        let restored = open(&backup).await;
        assert_eq!(
            user_version(&restored).await,
            LATEST_VERSION - 1,
            "backup must capture the pre-migration schema version"
        );
        assert_eq!(
            node_count(&restored).await,
            1,
            "backup must preserve the data"
        );
    }

    #[tokio::test]
    async fn backs_up_a_populated_version_0_db() {
        // A pre-versioning database: full schema + data but user_version still 0.
        // Must be protected too (the gate is data, not the version number).
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("legacy.db");
        let conn = open(&db_path).await;
        run_up_to(&conn, 1).await.unwrap(); // create the schema
        insert_marker(&conn).await;
        conn.execute("PRAGMA user_version = 0", ()).await.unwrap(); // simulate pre-versioning
        assert_eq!(user_version(&conn).await, 0);

        let backup = backup_before_pending_migrations(&conn, &db_path)
            .await
            .unwrap()
            .expect("a populated version-0 db must be backed up");
        assert_eq!(node_count(&open(&backup).await).await, 1);
    }

    #[tokio::test]
    async fn no_backup_for_an_empty_db_even_with_pending_migrations() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("empty.db");
        let conn = open(&db_path).await;
        run_up_to(&conn, LATEST_VERSION - 1).await.unwrap(); // schema, no rows
        let backup = backup_before_pending_migrations(&conn, &db_path)
            .await
            .unwrap();
        assert!(backup.is_none(), "an empty db has no data to protect");
    }

    #[tokio::test]
    async fn no_backup_when_already_current() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("data.db");
        let conn = open(&db_path).await;
        run(&conn).await.unwrap(); // already at LATEST — no migration will run
        insert_marker(&conn).await;
        let backup = backup_before_pending_migrations(&conn, &db_path)
            .await
            .unwrap();
        assert!(backup.is_none(), "a current database must not be backed up");
        assert!(!dir.path().join("backups").exists());
    }

    /// v004: JSON relationship declarations move to relationship-table rows and
    /// the `relationships` key is stripped from schema node properties.
    #[tokio::test]
    async fn v004_moves_json_declarations_to_relationship_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("declarations.db");
        let conn = open(&db_path).await;
        run_up_to(&conn, 3).await.unwrap();

        // A pre-v004 database: schemas with declarations in properties JSON.
        for (id, props) in [
            (
                "widget",
                r#"{"isCore":false,"fields":[],"relationships":[]}"#,
            ),
            (
                "assembly",
                r#"{"isCore":false,"fields":[],"relationships":[{"name":"widgets","targetType":"widget","direction":"out","cardinality":"many"},{"name":"related","direction":"out","cardinality":"many"},{"name":"has_child","targetType":"widget","direction":"out","cardinality":"many"}]}"#,
            ),
        ] {
            conn.execute(
                "INSERT INTO node (id, node_type, content, properties, created_at, modified_at) \
                 VALUES (?1, 'schema', ?1, ?2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                libsql::params![id.to_string(), props.to_string()],
            )
            .await
            .unwrap();
        }

        run_up_to(&conn, 4).await.unwrap();

        // Typed declaration → edge to the target schema.
        let mut rows = conn
            .query(
                "SELECT out_node, properties FROM relationship \
                 WHERE in_node = 'assembly' AND relationship_type = 'widgets'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("widgets declaration row");
        assert_eq!(row.get::<String>(0).unwrap(), "widget");
        let props: serde_json::Value =
            serde_json::from_str(&row.get::<String>(1).unwrap()).unwrap();
        assert_eq!(props["targetType"], "widget");

        // Untyped declaration → self-edge.
        let mut rows = conn
            .query(
                "SELECT out_node FROM relationship \
                 WHERE in_node = 'assembly' AND relationship_type = 'related'",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("related declaration row");
        assert_eq!(row.get::<String>(0).unwrap(), "assembly");

        // A legacy declaration named after a built-in structural relationship
        // is dropped, NOT converted — converting would create a real
        // `has_child` edge between schema nodes that hierarchy traversals
        // would follow.
        let mut rows = conn
            .query(
                "SELECT count(*) FROM relationship \
                 WHERE in_node = 'assembly' AND relationship_type = 'has_child'",
                (),
            )
            .await
            .unwrap();
        let builtin_rows: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(
            builtin_rows, 0,
            "builtin-named legacy declaration must be dropped, not converted"
        );

        // The properties key is stripped from every schema node, empty arrays included.
        let mut rows = conn
            .query(
                "SELECT count(*) FROM node WHERE node_type = 'schema' \
                 AND json_type(properties, '$.relationships') IS NOT NULL",
                (),
            )
            .await
            .unwrap();
        let remaining: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(remaining, 0, "no schema node may keep a relationships key");
    }

    /// Opening a store whose `user_version` is ahead of what this build knows
    /// (e.g. a newer release wrote it, then an older build — or an older CLI —
    /// opens the same file) must fail loudly rather than silently no-op: without
    /// the guard in `run_up_to`, `(start_version + 1)..=target_version` is an
    /// empty range, the loop never runs, and the caller gets `Ok(())` while
    /// proceeding against a schema it doesn't understand.
    #[tokio::test]
    async fn run_up_to_rejects_a_database_newer_than_this_build() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("from_the_future.db");
        let conn = open(&db_path).await;
        // Simulate a database written by a future build one version ahead of
        // what this build knows about.
        run_up_to(&conn, LATEST_VERSION).await.unwrap();
        let future_version = LATEST_VERSION + 1;
        conn.execute(&format!("PRAGMA user_version = {future_version}"), ())
            .await
            .unwrap();
        assert_eq!(user_version(&conn).await, future_version);

        let err = run_up_to(&conn, LATEST_VERSION)
            .await
            .expect_err("opening a newer-than-supported database must fail, not no-op");
        let message = err.to_string();
        assert!(
            message.contains(&future_version.to_string()),
            "error must name the database's version: {message}"
        );
        assert!(
            message.contains(&LATEST_VERSION.to_string()),
            "error must name the version this build supports: {message}"
        );

        // The database must be left untouched — no migration ran, no partial state.
        assert_eq!(user_version(&conn).await, future_version);
    }

    #[tokio::test]
    async fn no_backup_for_a_brand_new_db() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("data.db");
        let conn = open(&db_path).await; // user_version 0, no data to protect
        let backup = backup_before_pending_migrations(&conn, &db_path)
            .await
            .unwrap();
        assert!(
            backup.is_none(),
            "a brand-new database has nothing to back up"
        );
    }
}
