//! Coverage for the schema bootstrap (`db::schema::create_schema`): a fresh
//! database gets the complete current shape in one pass, and creating it again
//! over an existing database changes nothing.

use nodespace_core::db::schema;

async fn open_raw(db_path: &std::path::Path) -> libsql::Connection {
    nodespace_core::db::ensure_sqlite_vec_registered().await;
    let database = libsql::Builder::new_local(db_path)
        .build()
        .await
        .expect("build libsql database");
    database.connect().expect("connect")
}

async fn names_of(conn: &libsql::Connection, kind: &str) -> Vec<String> {
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master WHERE type = ?1 ORDER BY name",
            libsql::params![kind.to_string()],
        )
        .await
        .expect("query sqlite_master");
    let mut names = Vec::new();
    while let Some(row) = rows.next().await.expect("next row") {
        names.push(row.get::<String>(0).expect("read name"));
    }
    names
}

async fn columns_of(conn: &libsql::Connection, table: &str) -> Vec<String> {
    let mut rows = conn
        .query(&format!("PRAGMA table_info({table})"), ())
        .await
        .expect("table_info");
    let mut cols = Vec::new();
    while let Some(row) = rows.next().await.expect("next row") {
        cols.push(row.get::<String>(1).expect("column name"));
    }
    cols
}

/// The bootstrap must produce every table the store depends on — including the
/// conflict journal that arrived late in the old migration ladder, which a naive
/// collapse would silently drop — and keep the unused legacy columns
/// (`embedding.origin`, `node.sync_seq`) that databases from earlier builds carry.
#[tokio::test]
async fn fresh_database_gets_the_complete_current_schema() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let conn = open_raw(&temp_dir.path().join("fresh.db")).await;

    schema::create_schema(&conn).await.expect("create schema");

    let tables = names_of(&conn, "table").await;
    for expected in [
        "node",
        "relationship",
        "embedding",
        "conflict",
        "conflict_participant",
        "node_title_fts",
        "vec_embeddings",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "missing table {expected}; got {tables:?}"
        );
    }

    assert!(
        columns_of(&conn, "embedding")
            .await
            .iter()
            .any(|c| c == "origin"),
        "embedding.origin is kept so databases created by earlier builds pass the shape check"
    );

    assert!(
        columns_of(&conn, "node")
            .await
            .iter()
            .any(|c| c == "sync_seq"),
        "node.sync_seq is kept so databases created by earlier builds pass the shape check"
    );

    assert!(
        columns_of(&conn, "relationship")
            .await
            .iter()
            .any(|c| c == "reverse_relationship_type"),
        "the relationship table must carry the reverse name as a column"
    );

    let indexes = names_of(&conn, "index").await;
    for expected in [
        "idx_node_type",
        "idx_node_modified",
        "idx_node_lifecycle",
        "idx_task_status",
        "idx_task_due_date",
        "idx_task_priority",
        "idx_task_status_due_date",
        "idx_project_status",
        "idx_project_priority",
        "idx_rel_type",
        "idx_rel_in",
        "idx_rel_out",
        "idx_rel_unique",
        "idx_rel_reverse",
        "idx_emb_node",
        "idx_emb_stale_mod",
        "idx_emb_unique",
        "idx_conflict_status",
        "idx_conflict_participant_node",
    ] {
        assert!(
            indexes.iter().any(|i| i == expected),
            "missing index {expected}; got {indexes:?}"
        );
    }

    let triggers = names_of(&conn, "trigger").await;
    for expected in [
        "node_title_fts_insert",
        "node_title_fts_update",
        "node_title_fts_delete",
    ] {
        assert!(
            triggers.iter().any(|t| t == expected),
            "missing trigger {expected}; got {triggers:?}"
        );
    }
}

/// A database created by an earlier build still carries `idx_emb_modified`,
/// an index the current DDL no longer creates. Opening it must succeed and
/// leave the index alone.
#[tokio::test]
async fn a_database_that_still_has_idx_emb_modified_opens() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let conn = open_raw(&temp_dir.path().join("legacy-index.db")).await;

    schema::create_schema(&conn).await.expect("create schema");
    conn.execute(
        "CREATE INDEX idx_emb_modified ON embedding (origin, modified_at, node_id, chunk_index)",
        (),
    )
    .await
    .expect("create the legacy index by hand");

    schema::create_schema(&conn)
        .await
        .expect("a database with the legacy index must still pass create_schema");

    let indexes = names_of(&conn, "index").await;
    assert!(
        indexes.iter().any(|i| i == "idx_emb_modified"),
        "create_schema leaves the legacy index in place; got {indexes:?}"
    );
}

/// End-to-end confirmation that the planner actually picks a partial expression
/// index for the exact filter shape `QueryService` generates — a stronger
/// guarantee than asserting the index exists by name, since SQLite silently
/// ignores an expression index whose expression does not match the query's
/// byte-for-byte and falls back to a full scan.
///
/// Asserts index *coverage*, not one index by name: `idx_task_status` and
/// `idx_task_status_due_date` both cover this filter, and which one the planner
/// picks depends on table statistics. Either proves the expression matches,
/// which is the property that can silently break.
#[tokio::test]
async fn task_status_filter_uses_a_partial_expression_index() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let conn = open_raw(&temp_dir.path().join("plan.db")).await;

    schema::create_schema(&conn).await.expect("create schema");

    conn.execute(
        "INSERT INTO node (id, node_type, content, properties, lifecycle_status, version, created_at, modified_at) \
         VALUES ('t1', 'task', 'Task 1', '{\"task\":{\"status\":\"open\"}}', 'active', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        (),
    )
    .await
    .expect("insert task");

    let mut rows = conn
        .query(
            "EXPLAIN QUERY PLAN SELECT * FROM node WHERE node_type = 'task' AND json_extract(properties, '$.task.status') = 'open'",
            (),
        )
        .await
        .expect("explain query plan");

    let mut plan = String::new();
    while let Some(row) = rows.next().await.expect("next plan row") {
        plan.push_str(&row.get::<String>(3).expect("plan detail column"));
        plan.push('\n');
    }

    assert!(
        plan.contains("idx_task_status"),
        "planner must use a status-covering partial expression index for this \
         filter shape (a full scan means the index expression no longer matches \
         what QueryService generates), got plan: {plan}"
    );
}

/// Creating the schema over a database that already has it must neither fail
/// nor disturb existing rows — this is what reopening a database does on every
/// startup.
#[tokio::test]
async fn create_schema_is_idempotent_and_preserves_data() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let conn = open_raw(&temp_dir.path().join("repeat.db")).await;

    schema::create_schema(&conn).await.expect("first create");
    conn.execute(
        "INSERT INTO node (id, node_type, content, created_at, modified_at) \
         VALUES ('keep-me', 'text', 'content', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        (),
    )
    .await
    .expect("insert row");

    schema::create_schema(&conn).await.expect("second create");

    let mut rows = conn
        .query("SELECT count(*) FROM node WHERE id = 'keep-me'", ())
        .await
        .unwrap();
    let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert_eq!(count, 1, "re-creating the schema must not disturb data");
}
