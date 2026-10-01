//! Read a database's required extensions without opening it as a store
//! (ADR-083 §2).
//!
//! A database lists, in its settings node's `required_extensions`, the
//! extensions a reader needs in order to read it correctly. The daemon checks
//! that list before it opens a database and refuses one that lists an
//! extension it does not support. The check has to come before the store opens
//! the file, because opening it is already a write: [`crate::SqliteStore::new`]
//! switches the journal to WAL, runs the schema DDL and seeds. So this module
//! reads the file through its own read-only connection, which never writes the
//! database or its WAL.
//!
//! This is a compatibility guard, not a security control: the database is a
//! plain SQLite file that any process running as the user can read or edit. It
//! exists so this build never misreads types or edge fields an extension wrote.

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::models::core_schemas::REQUIRED_EXTENSIONS_FIELD;

/// Read the `required_extensions` of the database at `db_path`, in stored
/// order and without duplicates.
///
/// The settings singleton is found by its fixed id, whatever its `node_type`,
/// since an extension may retype it to a subtype of `database-settings`
/// (ADR-078); the field stays in the `database-settings` bucket either way.
///
/// Never writes the database or its WAL. When no `-wal` file exists the file is
/// opened `immutable`, which takes no lock and creates no side file. When one
/// does, the file is opened `mode=ro` instead, which reads the committed pages
/// the WAL still holds (an `immutable` read would miss a value that was never
/// checkpointed); like any WAL reader it may create the `-shm` index.
///
/// Returns an empty list when there is no file (opening it creates it), when
/// the file has no `node` table with `id` and `properties` columns (an empty
/// file, or tables of a shape this build does not know, which the store's own
/// table-shape check then reports), and when the database has no settings node
/// or the node no such field. Errors when the file cannot be read as a
/// database, or when the field holds anything but a list of strings: a guard
/// that cannot read the list must not open the database as if it were empty.
pub async fn read_required_extensions(db_path: &Path) -> Result<Vec<String>> {
    match tokio::fs::metadata(db_path).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("Failed to stat {}", db_path.display())),
    }

    let has_wal = tokio::fs::try_exists(wal_path(db_path))
        .await
        .unwrap_or(true);
    let uri = read_only_uri(db_path, if has_wal { "mode=ro" } else { "immutable=1" })?;
    let flags = libsql::OpenFlags::from_bits_retain(
        libsql::ffi::SQLITE_OPEN_READONLY | libsql::ffi::SQLITE_OPEN_URI,
    );
    let database = libsql::Builder::new_local(&uri)
        .flags(flags)
        .build()
        .await
        .with_context(|| format!("Failed to open {} read-only", db_path.display()))?;
    let conn = database
        .connect()
        .with_context(|| format!("Failed to open {} read-only", db_path.display()))?;
    // A `mode=ro` read still takes a shared lock, so wait out a writer
    // committing rather than failing the open on a momentary lock. A
    // connection setting, not a write.
    conn.query("PRAGMA busy_timeout = 5000", ())
        .await
        .context("Failed to set busy_timeout on the read-only connection")?;

    // Only a `node` table holding the two columns read below is queried. Any
    // other shape is left to the store, whose table-shape check refuses it with
    // its own error rather than this guard failing on a missing column.
    let mut columns = conn
        .query(
            "SELECT count(*) FROM pragma_table_info('node') WHERE name IN ('id', 'properties')",
            (),
        )
        .await
        .with_context(|| format!("Failed to read the schema of {}", db_path.display()))?;
    let known_columns: i64 = match columns.next().await? {
        Some(row) => row.get(0)?,
        None => 0,
    };
    if known_columns != 2 {
        return Ok(Vec::new());
    }

    // `json_type` first: `json_extract` returns a stored string as bare text,
    // which must not be mistaken for the JSON text of an array.
    let mut rows = conn
        .query(
            "SELECT json_type(properties, ?2), json_extract(properties, ?2) \
             FROM node WHERE id = ?1",
            [
                crate::services::node_service::DATABASE_SETTINGS_NODE_ID,
                REQUIRED_EXTENSIONS_PATH,
            ],
        )
        .await
        .with_context(|| format!("Failed to read the settings node of {}", db_path.display()))?;
    let Some(row) = rows.next().await? else {
        return Ok(Vec::new());
    };
    match row.get_value(0)? {
        libsql::Value::Null => Ok(Vec::new()),
        libsql::Value::Text(kind) if kind == "null" => Ok(Vec::new()),
        libsql::Value::Text(kind) if kind == "array" => match row.get_value(1)? {
            libsql::Value::Text(json) => parse_list(&json),
            other => bail!("{REQUIRED_EXTENSIONS_FIELD} could not be read, found {other:?}"),
        },
        libsql::Value::Text(kind) => {
            bail!("{REQUIRED_EXTENSIONS_FIELD} must be a list of strings, found a JSON {kind}")
        }
        other => bail!("{REQUIRED_EXTENSIONS_FIELD} could not be read, found {other:?}"),
    }
}

/// The JSON path of the field inside a node's properties: the
/// `database-settings` bucket, where it stays when the node is retyped to a
/// subtype (ADR-078).
const REQUIRED_EXTENSIONS_PATH: &str = "$.\"database-settings\".required_extensions";

/// Parse the stored array's JSON text into its ids.
fn parse_list(json: &str) -> Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(json).with_context(|| {
        format!("{REQUIRED_EXTENSIONS_FIELD} must be a list of strings, found {json}")
    })?;
    let Some(items) = value.as_array() else {
        bail!("{REQUIRED_EXTENSIONS_FIELD} must be a list of strings, found {json}");
    };
    let mut ids: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let Some(id) = item.as_str() else {
            bail!("{REQUIRED_EXTENSIONS_FIELD} must be a list of strings, found {json}");
        };
        if !ids.iter().any(|seen| seen == id) {
            ids.push(id.to_string());
        }
    }
    Ok(ids)
}

/// The WAL file SQLite keeps beside `db_path`.
fn wal_path(db_path: &Path) -> std::path::PathBuf {
    let mut wal = db_path.as_os_str().to_owned();
    wal.push("-wal");
    std::path::PathBuf::from(wal)
}

/// A `file:` URI with an empty authority for `db_path`, with the query `query`.
/// The path is made absolute first: in a URI with an authority a relative path
/// would read as the authority, and SQLite refuses any authority but an empty
/// one or `localhost`.
fn read_only_uri(db_path: &Path, query: &str) -> Result<String> {
    let absolute = std::path::absolute(db_path)
        .with_context(|| format!("Failed to resolve {}", db_path.display()))?;
    let Some(path) = absolute.to_str() else {
        bail!("database path is not valid UTF-8: {}", db_path.display());
    };
    Ok(format!("file://{}?{query}", uri_path(path, cfg!(windows))))
}

/// The path part of a `file:` URI with an empty authority, for an absolute
/// path, so it always begins with `/`. Percent-encodes the three characters a URI path cannot hold
/// literally (`%`, `?`, `#`). A Windows path drops a `\\?\` verbatim prefix and
/// has its separators turned into `/`: a drive path gains a leading `/`
/// (`/C:/…`), and a UNC path keeps its leading `//` (`//server/share/…`),
/// which SQLite hands back to Windows as `\\server\share\…`. A parameter rather
/// than a `cfg` so both forms are tested on every platform.
fn uri_path(path: &str, windows: bool) -> String {
    let path = if windows {
        let path = match path.strip_prefix(r"\\?\UNC\") {
            Some(share) => format!(r"\\{share}"),
            None => path.strip_prefix(r"\\?\").unwrap_or(path).to_string(),
        };
        let path = path.replace('\\', "/");
        if path.starts_with('/') {
            path
        } else {
            format!("/{path}")
        }
    } else {
        path.to_string()
    };
    let mut encoded = String::with_capacity(path.len());
    for ch in path.chars() {
        match ch {
            '%' => encoded.push_str("%25"),
            '?' => encoded.push_str("%3F"),
            '#' => encoded.push_str("%23"),
            other => encoded.push(other),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_of_strings_parses_in_order_without_duplicates() {
        assert_eq!(
            parse_list(r#"["b", "a", "b"]"#).unwrap(),
            vec!["b".to_string(), "a".to_string()]
        );
        assert!(parse_list("[]").unwrap().is_empty());
    }

    #[test]
    fn anything_but_a_list_of_strings_is_an_error() {
        for bad in [r#"["a", 1]"#, r#"[null]"#, r#"{"a": "b"}"#, "true", "3"] {
            assert!(parse_list(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[cfg(unix)]
    #[test]
    fn uri_escapes_the_characters_a_uri_path_cannot_hold() {
        assert_eq!(
            read_only_uri(Path::new("/tmp/a b/50%?#.db"), "immutable=1").unwrap(),
            "file:///tmp/a b/50%25%3F%23.db?immutable=1"
        );
    }

    /// A relative path is resolved against the working directory before it
    /// goes into the URI, where it would otherwise read as the authority.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_relative_path_is_read_from_the_working_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_database(
            dir.path(),
            "database-settings",
            r#"{"database-settings": {"required_extensions": ["pro"]}}"#,
        )
        .await;
        let cwd = std::env::current_dir().unwrap();
        let ups = "../".repeat(cwd.components().count() - 1);
        let relative = std::path::PathBuf::from(format!(
            "{ups}{}",
            path.strip_prefix("/").unwrap().display()
        ));
        assert!(relative.is_relative());

        assert_eq!(
            read_required_extensions(&relative).await.unwrap(),
            vec!["pro".to_string()]
        );
        assert!(read_only_uri(&relative, "immutable=1")
            .unwrap()
            .starts_with("file:///"));
    }

    #[test]
    fn windows_drive_and_share_paths_become_uri_paths() {
        for (path, expected) in [
            (r"C:\Users\a\db.sqlite", "/C:/Users/a/db.sqlite"),
            (r"\\?\C:\Users\a\db.sqlite", "/C:/Users/a/db.sqlite"),
            (r"\\server\share\db.sqlite", "//server/share/db.sqlite"),
            (
                r"\\?\UNC\server\share\db.sqlite",
                "//server/share/db.sqlite",
            ),
            (r"C:\a#b\50%.db", "/C:/a%23b/50%25.db"),
        ] {
            assert_eq!(uri_path(path, true), expected, "{path}");
        }
        // A unix path is left as it is, backslashes included.
        assert_eq!(uri_path(r"/tmp/a\b.db", false), r"/tmp/a\b.db");
    }

    #[test]
    fn the_wal_file_sits_beside_the_database() {
        assert_eq!(
            wal_path(Path::new("/x/y.db")),
            std::path::PathBuf::from("/x/y.db-wal")
        );
    }

    use crate::services::node_service::DATABASE_SETTINGS_NODE_ID;

    /// A WAL-mode database holding a minimal `node` table, written through a
    /// plain read-write connection. The returned connection keeps the file's
    /// WAL alive until it is dropped.
    async fn node_table(path: &Path) -> libsql::Connection {
        let db = libsql::Builder::new_local(path).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.query("PRAGMA journal_mode = WAL", ()).await.unwrap();
        conn.execute(
            "CREATE TABLE node (id TEXT PRIMARY KEY, node_type TEXT NOT NULL, \
             properties TEXT NOT NULL DEFAULT '{}')",
            (),
        )
        .await
        .unwrap();
        conn
    }

    async fn insert(conn: &libsql::Connection, id: &str, node_type: &str, properties: &str) {
        conn.execute(
            "INSERT INTO node (id, node_type, properties) VALUES (?1, ?2, ?3)",
            [id, node_type, properties],
        )
        .await
        .unwrap();
    }

    /// Build a database whose settings singleton has `node_type` and
    /// `properties`, then close it so its WAL is checkpointed and removed.
    async fn closed_database(dir: &Path, node_type: &str, properties: &str) -> std::path::PathBuf {
        let path = dir.join("db.sqlite");
        let conn = node_table(&path).await;
        insert(&conn, DATABASE_SETTINGS_NODE_ID, node_type, properties).await;
        drop(conn);
        assert!(!wal_path(&path).exists(), "closing checkpoints the WAL");
        path
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn a_missing_file_requires_nothing_and_is_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.db");
        assert!(read_required_extensions(&path).await.unwrap().is_empty());
        assert!(!path.exists());
    }

    /// A `node` table of a shape this build does not know is left to the
    /// store's table-shape check rather than failing the guard.
    #[tokio::test]
    async fn a_node_table_of_another_shape_requires_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("other-shape.db");
        let db = libsql::Builder::new_local(&path).build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute("CREATE TABLE node (id TEXT PRIMARY KEY, body TEXT)", ())
            .await
            .unwrap();
        drop(conn);
        assert!(read_required_extensions(&path).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_database_without_a_node_table_or_settings_node_requires_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.db");
        std::fs::write(&empty, b"").unwrap();
        assert!(read_required_extensions(&empty).await.unwrap().is_empty());

        let no_settings = dir.path().join("no-settings.db");
        let conn = node_table(&no_settings).await;
        insert(&conn, "some-node", "text", "{}").await;
        drop(conn);
        assert!(read_required_extensions(&no_settings)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn the_list_is_read_from_the_settings_bucket_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_database(
            dir.path(),
            "database-settings",
            r#"{"database-settings": {"required_extensions": ["pro"]}}"#,
        )
        .await;
        let before = std::fs::read(&path).unwrap();
        let files_before = files_in(dir.path());

        let required = read_required_extensions(&path).await.unwrap();

        assert_eq!(required, vec!["pro".to_string()]);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "file is byte-identical"
        );
        assert_eq!(
            files_in(dir.path()),
            files_before,
            "no side file was created"
        );
    }

    #[tokio::test]
    async fn a_retyped_singleton_is_found_by_its_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = closed_database(
            dir.path(),
            "fixture-settings",
            r#"{"database-settings": {"required_extensions": ["pro"]}, "fixture-settings": {"x": 1}}"#,
        )
        .await;
        assert_eq!(
            read_required_extensions(&path).await.unwrap(),
            vec!["pro".to_string()]
        );
    }

    #[tokio::test]
    async fn a_value_only_in_the_wal_is_read_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.db");
        let writer = node_table(&path).await;
        writer
            .query("PRAGMA wal_autocheckpoint = 0", ())
            .await
            .unwrap();
        insert(
            &writer,
            DATABASE_SETTINGS_NODE_ID,
            "database-settings",
            r#"{"database-settings": {"required_extensions": ["pro"]}}"#,
        )
        .await;
        assert!(wal_path(&path).exists(), "the write is still in the WAL");
        let before = std::fs::read(&path).unwrap();

        let required = read_required_extensions(&path).await.unwrap();

        assert_eq!(required, vec!["pro".to_string()]);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "file is byte-identical"
        );
        drop(writer);
    }

    #[tokio::test]
    async fn an_empty_or_null_list_requires_nothing() {
        for properties in [
            r#"{"database-settings": {"required_extensions": []}}"#,
            r#"{"database-settings": {"required_extensions": null}}"#,
            r#"{"database-settings": {}}"#,
            r#"{}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = closed_database(dir.path(), "database-settings", properties).await;
            assert!(
                read_required_extensions(&path).await.unwrap().is_empty(),
                "{properties}"
            );
        }
    }

    #[tokio::test]
    async fn a_value_that_is_not_a_list_of_strings_is_an_error() {
        for properties in [
            r#"{"database-settings": {"required_extensions": "pro"}}"#,
            r#"{"database-settings": {"required_extensions": "[\"pro\"]"}}"#,
            r#"{"database-settings": {"required_extensions": 42}}"#,
            r#"{"database-settings": {"required_extensions": ["pro", 1]}}"#,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = closed_database(dir.path(), "database-settings", properties).await;
            assert!(
                read_required_extensions(&path).await.is_err(),
                "{properties} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_database_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("garbage.db");
        std::fs::write(&path, vec![0x42u8; 8192]).unwrap();
        assert!(read_required_extensions(&path).await.is_err());
    }
}
