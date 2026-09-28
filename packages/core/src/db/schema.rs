//! The database's schema, created in one pass on a fresh database.
//!
//! NodeSpace has no released builds and no user databases, so there is no older
//! on-disk shape to carry forward: the only database that exists is one this
//! build creates. That makes the schema a single current-shape definition rather
//! than a replayable history. When the shape changes, edit the DDL below and
//! reset the database — never write code that moves existing rows from an old
//! shape to a new one.
//!
//! [`create_schema`] is idempotent (every statement is `IF NOT EXISTS`), so
//! opening a database this build already created is a no-op.
//!
//! That same `IF NOT EXISTS` is also why a database some *other* build created
//! is refused rather than opened: an existing table is never altered, so a
//! column this build's DDL adds never appears in it, and the first index or
//! query naming that column fails. Before any DDL runs, [`create_schema`]
//! compares the columns of every table that already exists against the columns
//! this build's DDL defines, and returns a [`SchemaMismatch`] when they differ.
//! That is a shape check, not a version check — nothing on disk records a
//! version — and it carries nothing forward: the only resolution it offers is
//! moving the database aside and starting fresh.
//!
//! Connection-level PRAGMAs (`journal_mode`, `foreign_keys`, `synchronous`,
//! `busy_timeout`) are deliberately absent: they are per-connection session
//! settings rather than persisted schema state, and SQLite forbids changing
//! `synchronous` inside a transaction. `SqliteStore` sets them on every
//! connection it opens.

use std::collections::BTreeSet;
use std::fmt;

use anyhow::{Context, Result};

/// Every table and index, in dependency order. Virtual tables and triggers are
/// created separately in [`create_schema`] — their bodies contain semicolons,
/// which the naive `;` split below cannot handle.
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS node (
    id               TEXT    PRIMARY KEY,
    node_type        TEXT    NOT NULL,
    content          TEXT    NOT NULL DEFAULT '',
    properties       TEXT    NOT NULL DEFAULT '{}',
    title            TEXT,
    lifecycle_status TEXT    NOT NULL DEFAULT 'active',
    version          INTEGER NOT NULL DEFAULT 1,
    sync_seq         INTEGER,
    created_at       TEXT    NOT NULL,
    modified_at      TEXT    NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_node_type      ON node (node_type);
CREATE INDEX IF NOT EXISTS idx_node_modified  ON node (modified_at);
CREATE INDEX IF NOT EXISTS idx_node_lifecycle ON node (lifecycle_status);

-- Partial expression indexes on the hot task/project properties the agent
-- filters and sorts on. `node.properties` is a JSON blob with no index on
-- individual values, so `QueryService`'s `json_extract(properties,
-- '$.<type>.<field>')` filters would otherwise seek the `node_type` partition
-- and evaluate `json_extract` per row (plus a filesort for `ORDER BY`). Each
-- index covers only rows of its own `node_type`, so it stays cheap to maintain.
CREATE INDEX IF NOT EXISTS idx_task_status ON node (json_extract(properties, '$.task.status')) WHERE node_type = 'task';
CREATE INDEX IF NOT EXISTS idx_task_due_date ON node (json_extract(properties, '$.task.due_date')) WHERE node_type = 'task';
CREATE INDEX IF NOT EXISTS idx_task_priority ON node (json_extract(properties, '$.task.priority')) WHERE node_type = 'task';
-- Serves "open tasks ordered by due date" (`status` equality + `due_date`
-- range/sort) without a filesort. `idx_task_status` is redundant with this one
-- for reads under SQLite's leftmost-prefix rule, but is kept: a single-column
-- index is cheaper to maintain for the common status-only filter.
CREATE INDEX IF NOT EXISTS idx_task_status_due_date ON node (json_extract(properties, '$.task.status'), json_extract(properties, '$.task.due_date')) WHERE node_type = 'task';
CREATE INDEX IF NOT EXISTS idx_project_status ON node (json_extract(properties, '$.project.status')) WHERE node_type = 'project';

-- Holds BOTH instance-level edges (person→task tasks/assignee, has_child, …)
-- and schema relationship DECLARATIONS: a declaration row connects two schema
-- nodes (in_node = declaring schema, out_node = target schema, or a self-edge
-- when untyped) under the declared name, with the full SchemaRelationship JSON
-- in `properties` (ADR-070). The two kinds share relationship_type values and
-- are distinguished by endpoint node_type ('schema' vs instance), never by
-- name — which is why declared names may not collide with the built-in
-- structural types.
CREATE TABLE IF NOT EXISTS relationship (
    id                TEXT    PRIMARY KEY DEFAULT (lower(hex(randomblob(16)))),
    in_node           TEXT    NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    out_node          TEXT    NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    relationship_type TEXT    NOT NULL,
    -- The name this edge reads by from the target's end — `child_of` for a
    -- `has_child` row, `project` for a `tasks` declaration. Every edge is named
    -- from both ends, but only the declaring side's name was ever a column.
    -- The reverse lived inside the `properties` JSON blob (for schema-declared
    -- relationships) or nowhere at all (for the built-in structural ones), so
    -- no query could filter or traverse by it.
    reverse_relationship_type TEXT,
    properties        TEXT    NOT NULL DEFAULT '{}',
    version           INTEGER NOT NULL DEFAULT 1,
    created_at        TEXT    NOT NULL,
    modified_at       TEXT    NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_rel_type  ON relationship (relationship_type);
CREATE INDEX IF NOT EXISTS idx_rel_in    ON relationship (in_node, relationship_type);
CREATE INDEX IF NOT EXISTS idx_rel_out   ON relationship (out_node, relationship_type);
CREATE UNIQUE INDEX IF NOT EXISTS idx_rel_unique ON relationship (in_node, out_node, relationship_type);
-- Mirrors idx_rel_out for reverse traversal: "which nodes point at this one
-- under the reverse name" is the shape a Play condition walking `child_of`
-- issues, and without this it is a full scan of the edge table.
CREATE INDEX IF NOT EXISTS idx_rel_reverse ON relationship (out_node, reverse_relationship_type);

CREATE TABLE IF NOT EXISTS embedding (
    id           TEXT    PRIMARY KEY DEFAULT (lower(hex(randomblob(16)))),
    node_id      TEXT    NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    vector       BLOB    NOT NULL,
    dimension    INTEGER NOT NULL DEFAULT 768,
    model_name   TEXT    NOT NULL DEFAULT 'nomic-embed-text-v1.5',
    chunk_index  INTEGER NOT NULL DEFAULT 0,
    chunk_start  INTEGER NOT NULL DEFAULT 0,
    chunk_end    INTEGER,
    total_chunks INTEGER NOT NULL DEFAULT 1,
    content_hash TEXT,
    token_count  INTEGER,
    stale        INTEGER NOT NULL DEFAULT 1,
    error_count  INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT,
    -- 'local' = generated on this device, 'remote' = pulled from another device
    -- via cloud sync. The cloud-push sweep reads only 'local' rows, so a pulled
    -- vector never gets re-pushed (no cross-device re-push loop).
    origin       TEXT    NOT NULL DEFAULT 'local',
    created_at   TEXT    NOT NULL,
    modified_at  TEXT    NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_emb_node      ON embedding (node_id);
CREATE INDEX IF NOT EXISTS idx_emb_stale_mod ON embedding (stale, modified_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_emb_unique ON embedding (node_id, model_name, chunk_index);
-- `SqliteStore::embeddings_modified_since` does an `origin = 'local' AND
-- modified_at >= ?` range scan ORDER BY modified_at, node_id, chunk_index.
-- Leading on `origin` (equality) then `modified_at` (range) makes the recurring
-- cloud-push sweep an index range scan that also covers the ORDER BY.
CREATE INDEX IF NOT EXISTS idx_emb_modified ON embedding (origin, modified_at, node_id, chunk_index);

-- Local-only conflict journal (ADR-068): a durable, resolvable record of a
-- convergence conflict. `conflict` holds one row per detected conflict, keyed
-- by a deterministic id (see `deterministic_conflict_id`) so re-detection
-- updates `occurrences`/`last_seen_at` on the same row instead of appending a
-- duplicate.
CREATE TABLE IF NOT EXISTS conflict (
    id            TEXT    PRIMARY KEY,
    kind          TEXT    NOT NULL,
    node_ids      TEXT    NOT NULL,
    detail        TEXT    NOT NULL,
    status        TEXT    NOT NULL DEFAULT 'open',
    detected_at   TEXT    NOT NULL,
    detected_by   TEXT,
    occurrences   INTEGER NOT NULL DEFAULT 1,
    last_seen_at  TEXT    NOT NULL,
    resolved_at   TEXT,
    resolution    TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS idx_conflict_status ON conflict (status);

-- Companion table so "is this node in an open conflict" is an index hit on
-- `node_id`, not a scan over the `node_ids` JSON array in `conflict`.
--
-- No FK from `node_id` to `node(id)`: a participant may be hard-deleted, and
-- the record of the conflict must survive that (a reconciliation sweep closes
-- the record later — conflict-journal spec §5.4). `conflict_id` DOES cascade:
-- deleting a conflict record should take its participant rows with it.
CREATE TABLE IF NOT EXISTS conflict_participant (
    conflict_id TEXT NOT NULL REFERENCES conflict(id) ON DELETE CASCADE,
    node_id     TEXT NOT NULL,
    PRIMARY KEY (conflict_id, node_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_conflict_participant_node ON conflict_participant (node_id);
"#;

/// Create the full schema on `conn`. Idempotent: safe to call on a database
/// this build already created.
///
/// Runs the whole thing — every `CREATE TABLE`/`CREATE INDEX` in
/// [`SCHEMA_SQL`], the FTS5 table and its triggers, the vec0 table, and the
/// closing `ANALYZE` — inside one `BEGIN IMMEDIATE` / `COMMIT`. Two things
/// this closes:
///
/// - `BEGIN IMMEDIATE` takes the write lock up front rather than lazily on
///   first write, so a second connection racing to create the same fresh
///   schema (see `DatabaseManager::get_or_open` in the daemon, which
///   explicitly allows two concurrent callers to each open a writer and
///   race here) blocks — subject to `busy_timeout`, now set before any
///   other pragma on this connection — instead of interleaving with this
///   transaction statement-by-statement.
/// - Without an explicit transaction, `create_schema` was a sequence of
///   separate autocommit statements: `relationship` could commit, and
///   THEN a differently-timed statement on some other connection could
///   observe it, before every index on it had committed too. Wrapping the
///   whole sequence means every other connection sees either none of this
///   schema or all of it — never a table with some of its indexes (or, on
///   a from-scratch table, some of its columns) missing.
async fn create_schema_body(conn: &libsql::Connection, expected: &[ExpectedTable]) -> Result<()> {
    // Refuse a database whose existing tables have a different shape before
    // touching it: the DDL below would otherwise fail part-way on the first
    // index or trigger naming a column the old table lacks, with an error that
    // says nothing about why.
    let mismatches = find_shape_mismatches(conn, expected).await?;
    if !mismatches.is_empty() {
        return Err(anyhow::Error::new(SchemaMismatch { tables: mismatches }));
    }

    execute_schema_sql(conn).await?;

    create_schema_objects(conn).await
}

/// Run every `CREATE TABLE`/`CREATE INDEX` in [`SCHEMA_SQL`] on `conn`.
async fn execute_schema_sql(conn: &libsql::Connection) -> Result<()> {
    // Naive `;`-splitting is safe ONLY because SCHEMA_SQL is plain CREATE
    // TABLE/INDEX statements with no semicolons inside string literals,
    // comments, or multi-statement bodies. Triggers and virtual tables are
    // created below via individual `execute` calls for exactly that reason — do
    // not move them up here without switching to a real statement splitter.
    for stmt in SCHEMA_SQL.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        // A fragment may open with `--` comment lines, but once those are
        // stripped it must begin an actual statement. Anything else means a `;`
        // inside a comment or literal split a statement in half.
        debug_assert!(
            stmt.lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with("--"))
                .is_some_and(|line| line.starts_with("CREATE")),
            "SCHEMA_SQL fragment does not begin a statement, so a `;` inside a \
             comment or literal split one in half: {}",
            &stmt[..stmt.len().min(160)]
        );
        conn.execute(stmt, ())
            .await
            .with_context(|| format!("Failed to execute DDL: {}", &stmt[..stmt.len().min(80)]))?;
    }
    Ok(())
}

/// The virtual tables, triggers and closing `ANALYZE` that [`SCHEMA_SQL`]
/// cannot carry.
async fn create_schema_objects(conn: &libsql::Connection) -> Result<()> {
    // FTS5 index over `node.title`. It answers "which node *is* X?" (one short
    // row per nameable node, whose text is that node's own name), and it is the
    // only full-text index: general search's keyword half matches documents by
    // title, never body text — body text is reached through the root's
    // embedding.
    //
    // `title` is the system's own maintained answer to "is this a nameable
    // thing": `NodeService::compute_title()` sets it for title-templated schema
    // instances, tasks, collections and root nodes, and leaves it NULL for
    // child/body nodes. Indexing only titled rows therefore makes every row in
    // this index an entity.
    //
    // "Titled" means `nullif(title, '') IS NOT NULL` — empty string as well as
    // NULL. `compute_title()` can return `Some("")` by two routes: a
    // `titleTemplate` whose referenced fields are all absent (unresolved tokens
    // are skipped, so the interpolation yields ""), and `strip_markdown("")` on
    // an empty-content task or collection. An empty string is not NULL, so
    // without `nullif` those rows would be indexed as term-less rows: matched by
    // nothing, but still occupying the index and falsifying "every row is an
    // entity".
    //
    // The tokenizer is FTS5's default `unicode61`, which folds diacritics
    // ("Ståhl" matches "stahl") and splits on non-alphanumerics, but does NOT
    // segment CJK — a Chinese or Japanese entity name indexes as one token and
    // matches only on the whole run. Recorded here because it bounds what
    // entity resolution can match, and changing it later re-tokenizes the index.
    //
    // All writes to `node` are plain INSERT/UPDATE/DELETE, so the triggers below
    // see every one. `REPLACE INTO node` (or `INSERT OR REPLACE`) would NOT be
    // safe: REPLACE fires neither AFTER UPDATE nor — with the default
    // `recursive_triggers=OFF` — AFTER DELETE, so it would strand a stale row at
    // the old rowid. None exists in the codebase today; adding one means
    // maintaining this index explicitly.
    //
    // NOT an external-content table (no `content='node'`). That is deliberate
    // and load-bearing: FTS5 has no partial-index syntax, so
    // "only when title IS NOT NULL" lives in the triggers below — but an
    // external-content table's `'rebuild'` command re-derives every row from the
    // content table, which would silently reinstate exactly the NULL-title rows
    // the triggers skip. A standalone table owns its own rows, so the
    // partial invariant survives; the cost is that it duplicates the title text,
    // which is a short string per nameable node.
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS node_title_fts USING fts5(id UNINDEXED, title)",
        (),
    )
    .await
    .context("Failed to create title FTS5 table")?;

    // The `WHERE` guard rides on `INSERT ... SELECT` because a plain `VALUES`
    // clause cannot carry one.
    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS node_title_fts_insert AFTER INSERT ON node BEGIN
            INSERT INTO node_title_fts(rowid, id, title)
            SELECT new.rowid, new.id, new.title WHERE nullif(new.title, '') IS NOT NULL;
        END"#,
        (),
    )
    .await
    .context("Failed to create title FTS5 insert trigger")?;

    // Delete-then-conditionally-reinsert. The unconditional DELETE is what
    // removes a row whose title transitions non-NULL -> NULL: the reinsert is
    // then skipped by the same guard, so the node leaves the index rather than
    // keeping a stale name. A conditional delete would strand that row forever.
    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS node_title_fts_update AFTER UPDATE ON node BEGIN
            DELETE FROM node_title_fts WHERE rowid = old.rowid;
            INSERT INTO node_title_fts(rowid, id, title)
            SELECT new.rowid, new.id, new.title WHERE nullif(new.title, '') IS NOT NULL;
        END"#,
        (),
    )
    .await
    .context("Failed to create title FTS5 update trigger")?;

    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS node_title_fts_delete AFTER DELETE ON node BEGIN
            DELETE FROM node_title_fts WHERE rowid = old.rowid;
        END"#,
        (),
    )
    .await
    .context("Failed to create title FTS5 delete trigger")?;

    // A collection is always a root (ADR-059 §2): collections nest through
    // `member_of`, never `has_child`. A collection may still HAVE `has_child`
    // children; it may not BE one. Enforced here rather than at each Rust
    // insert site because `has_child` edges are written from a dozen places
    // (create, append, move, bulk hierarchy, sync cold-sweep, seeding, generic
    // relationship create) and a node can become a collection by a type switch.
    // Foreign keys are immediate, so a child's node row always exists by the
    // time its edge is inserted.
    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS collection_is_root_edge BEFORE INSERT ON relationship
        WHEN new.relationship_type = 'has_child'
          AND (SELECT node_type FROM node WHERE id = new.out_node) = 'collection'
        BEGIN
            SELECT RAISE(ABORT, 'collection_not_root: a collection cannot have a parent; collections nest through member_of (ADR-059 §2)');
        END"#,
        (),
    )
    .await
    .context("Failed to create collection-is-root edge trigger")?;

    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS collection_is_root_type BEFORE UPDATE OF node_type ON node
        WHEN new.node_type = 'collection'
          AND EXISTS (SELECT 1 FROM relationship
                      WHERE out_node = new.id AND relationship_type = 'has_child')
        BEGIN
            SELECT RAISE(ABORT, 'collection_not_root: a node with a parent cannot become a collection; collections nest through member_of (ADR-059 §2)');
        END"#,
        (),
    )
    .await
    .context("Failed to create collection-is-root type trigger")?;

    // A schema is always a root, for the same reason and by the same means as
    // a collection: a subtree delete (e.g. `update_schema` replacing a
    // description subtree) cascades its descendants without the schema delete
    // guard, so a nested schema — core or extended by others — would be
    // removed unchecked.
    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS schema_is_root_edge BEFORE INSERT ON relationship
        WHEN new.relationship_type = 'has_child'
          AND (SELECT node_type FROM node WHERE id = new.out_node) = 'schema'
        BEGIN
            SELECT RAISE(ABORT, 'schema_not_root: a schema cannot have a parent; schemas are always roots');
        END"#,
        (),
    )
    .await
    .context("Failed to create schema-is-root edge trigger")?;

    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS schema_is_root_type BEFORE UPDATE OF node_type ON node
        WHEN new.node_type = 'schema'
          AND EXISTS (SELECT 1 FROM relationship
                      WHERE out_node = new.id AND relationship_type = 'has_child')
        BEGIN
            SELECT RAISE(ABORT, 'schema_not_root: a node with a parent cannot become a schema; schemas are always roots');
        END"#,
        (),
    )
    .await
    .context("Failed to create schema-is-root type trigger")?;

    // Whether a row is a core schema is fixed when it is created. The core
    // schema delete refusal reads `isCore` from the row, so an update that
    // clears it — or retypes the row away from `schema` — would make a core
    // type deletable; one that sets it would make a user type undeletable.
    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS schema_core_status_fixed BEFORE UPDATE OF node_type, properties ON node
        WHEN (old.node_type = 'schema' AND coalesce(json_type(old.properties, '$.isCore') = 'true', 0))
          IS NOT (new.node_type = 'schema' AND coalesce(json_type(new.properties, '$.isCore') = 'true', 0))
        BEGIN
            SELECT RAISE(ABORT, 'schema_is_core: whether a schema is core is fixed when it is created');
        END"#,
        (),
    )
    .await
    .context("Failed to create schema-core-status trigger")?;

    // sqlite-vec virtual table for embedding KNN search. Keyed by `embedding.id`
    // (the per-chunk UUID); holds ONLY real, non-stale vectors (see upsert/
    // delete/mark-stale paths). vec0 is a fast brute-force SIMD scan, not an ANN
    // index.
    conn.execute(
        &format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vec_embeddings USING vec0(\
                embedding_id TEXT PRIMARY KEY, \
                vector FLOAT[{}] distance_metric=cosine\
            )",
            crate::models::embedding::DEFAULT_EMBEDDING_DIMENSION
        ),
        (),
    )
    .await
    .context("Failed to create vec0 embeddings table")?;

    // Refresh planner stats so the expression indexes above are picked from the
    // first query, rather than waiting for organic churn to trigger SQLite's
    // automatic ANALYZE. Running on every open (not once at creation) also keeps
    // the stats current as the corpus grows, which is what keeps the planner
    // choosing those indexes later. It scans indexes rather than table content,
    // so the cost stays flat — single-digit milliseconds on a populated database.
    conn.execute("ANALYZE", ())
        .await
        .context("Failed to ANALYZE after creating schema")?;

    Ok(())
}

/// Entry point: run [`create_schema_body`] inside one `BEGIN IMMEDIATE` /
/// `COMMIT` on `conn`. See [`create_schema_body`]'s doc comment for why.
///
/// `BEGIN IMMEDIATE` rather than a bare `BEGIN`/`conn.transaction()` (which
/// would be `BEGIN DEFERRED`): deferred acquires the write lock lazily, on
/// this transaction's first write, so two racing connections could both get
/// past their first few statements before either actually blocks. Immediate
/// takes it up front, so the loser blocks (subject to `busy_timeout`) before
/// executing anything at all.
///
/// On any failure, rolls back before returning the error — this connection
/// is the store's one long-lived writer, so leaving an open transaction on
/// it would make every subsequent write on the connection fail with "cannot
/// start a transaction within a transaction" for the rest of the process's
/// life.
///
/// Returns an error whose root cause is a [`SchemaMismatch`] when the database
/// already holds tables whose columns differ from this build's DDL; nothing is
/// written in that case. Callers that need to tell that apart from any other
/// failure use [`SchemaMismatch::find_in`].
pub async fn create_schema(conn: &libsql::Connection) -> Result<()> {
    // Derived before the transaction opens: it is served from a process-wide
    // cache after the first call, and the first call opens a separate
    // in-memory database, which has no business happening under this
    // connection's write lock.
    let expected = expected_shape().await?;

    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .context("Failed to begin schema-creation transaction")?;

    if let Err(e) = create_schema_body(conn, expected).await {
        // Best-effort: if the rollback itself fails, the original DDL error
        // is what the caller needs to see, not the rollback's.
        let _ = conn.execute("ROLLBACK", ()).await;
        return Err(e);
    }

    if let Err(e) = conn
        .execute("COMMIT", ())
        .await
        .context("Failed to commit schema-creation transaction")
    {
        // A failed COMMIT does not always leave the transaction open — SQLite
        // auto-rolls-back most commit-time I/O errors on its own — but
        // SQLITE_BUSY on COMMIT specifically does not, so roll back
        // unconditionally here too rather than special-casing which commit
        // failures need it. A ROLLBACK issued after SQLite already rolled
        // back on its own is a harmless no-op, not a second error.
        let _ = conn.execute("ROLLBACK", ()).await;
        return Err(e);
    }

    Ok(())
}

/// One table this build's DDL defines, with its columns in declaration order.
#[derive(Debug)]
struct ExpectedTable {
    name: String,
    columns: Vec<String>,
}

/// The tables and columns [`SCHEMA_SQL`] defines, derived by running it on a
/// throwaway in-memory database and reading the result back — so the check
/// can never drift from the DDL, as a hand-kept column list would. Computed
/// once per process.
///
/// Only [`SCHEMA_SQL`]'s ordinary tables are covered, not the FTS5 and vec0
/// virtual tables `create_schema_objects` adds.
async fn expected_shape() -> Result<&'static [ExpectedTable]> {
    static EXPECTED: tokio::sync::OnceCell<Vec<ExpectedTable>> = tokio::sync::OnceCell::const_new();
    let tables = EXPECTED
        .get_or_try_init(|| async {
            let db = libsql::Builder::new_local(":memory:")
                .build()
                .await
                .context("Failed to open in-memory database for the schema shape")?;
            let conn = db
                .connect()
                .context("Failed to connect to in-memory database for the schema shape")?;
            execute_schema_sql(&conn).await?;
            let mut tables = Vec::new();
            for name in ordinary_tables(&conn).await? {
                let columns = table_columns(&conn, &name).await?;
                tables.push(ExpectedTable { name, columns });
            }
            Ok::<_, anyhow::Error>(tables)
        })
        .await?;
    Ok(tables)
}

/// Every table on `conn` other than SQLite's own internal ones.
async fn ordinary_tables(conn: &libsql::Connection) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' \
               AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             ORDER BY name",
            (),
        )
        .await
        .context("Failed to list tables")?;
    let mut names = Vec::new();
    while let Some(row) = rows.next().await.context("Failed to read table list")? {
        names.push(row.get::<String>(0).context("Failed to read table name")?);
    }
    Ok(names)
}

/// The columns of `table`, in declaration order. Empty when the table does not
/// exist.
async fn table_columns(conn: &libsql::Connection, table: &str) -> Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT name FROM pragma_table_info(?1) ORDER BY cid",
            libsql::params![table],
        )
        .await
        .with_context(|| format!("Failed to read the columns of table {table}"))?;
    let mut columns = Vec::new();
    while let Some(row) = rows
        .next()
        .await
        .with_context(|| format!("Failed to read the columns of table {table}"))?
    {
        columns.push(row.get::<String>(0).context("Failed to read column name")?);
    }
    Ok(columns)
}

/// Compare each table [`SCHEMA_SQL`] defines that already exists on `conn`
/// against its expected columns. A table that does not exist yet is not a
/// mismatch — the DDL creates it.
async fn find_shape_mismatches(
    conn: &libsql::Connection,
    expected: &[ExpectedTable],
) -> Result<Vec<TableShapeMismatch>> {
    let mut mismatches = Vec::new();
    for table in expected {
        let actual = table_columns(conn, &table.name).await?;
        if actual.is_empty() {
            continue;
        }
        let actual_set: BTreeSet<&str> = actual.iter().map(String::as_str).collect();
        let expected_set: BTreeSet<&str> = table.columns.iter().map(String::as_str).collect();
        let missing_columns: Vec<String> = table
            .columns
            .iter()
            .filter(|c| !actual_set.contains(c.as_str()))
            .cloned()
            .collect();
        let unexpected_columns: Vec<String> = actual
            .iter()
            .filter(|c| !expected_set.contains(c.as_str()))
            .cloned()
            .collect();
        if !missing_columns.is_empty() || !unexpected_columns.is_empty() {
            mismatches.push(TableShapeMismatch {
                table: table.name.clone(),
                missing_columns,
                unexpected_columns,
            });
        }
    }
    Ok(mismatches)
}

/// How one existing table differs from the shape this build's DDL defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableShapeMismatch {
    pub table: String,
    /// Columns this build defines that the existing table lacks.
    pub missing_columns: Vec<String>,
    /// Columns the existing table has that this build does not define.
    pub unexpected_columns: Vec<String>,
}

/// The database was created by a different build of NodeSpace: at least one
/// of its existing tables has columns other than the ones this build's DDL
/// defines.
///
/// There is no migration path by design (see the module docs). The database
/// can only be moved aside and replaced with a fresh one, so callers treat
/// this as a stop condition rather than a transient failure worth retrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaMismatch {
    /// Every mismatched table, in [`SCHEMA_SQL`]'s table-name order.
    pub tables: Vec<TableShapeMismatch>,
}

impl SchemaMismatch {
    /// The `SchemaMismatch` anywhere in `err`'s cause chain, if there is one.
    /// Callers wrap [`create_schema`]'s error in their own context on the way
    /// up, so the mismatch is rarely the outermost error.
    pub fn find_in(err: &anyhow::Error) -> Option<&SchemaMismatch> {
        err.chain()
            .find_map(|cause| cause.downcast_ref::<SchemaMismatch>())
    }
}

impl fmt::Display for SchemaMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "this database was created by a different version of NodeSpace and its tables \
             do not match this version's schema"
        )?;
        let mut details = Vec::new();
        for table in &self.tables {
            let mut parts = Vec::new();
            if !table.missing_columns.is_empty() {
                parts.push(format!("missing {}", table.missing_columns.join(", ")));
            }
            if !table.unexpected_columns.is_empty() {
                parts.push(format!(
                    "unexpected {}",
                    table.unexpected_columns.join(", ")
                ));
            }
            details.push(format!("{}: {}", table.table, parts.join("; ")));
        }
        if !details.is_empty() {
            write!(f, " ({})", details.join("; "))?;
        }
        write!(
            f,
            ". NodeSpace does not migrate databases between versions: move this database \
             aside and start with a fresh one"
        )
    }
}

impl std::error::Error for SchemaMismatch {}

#[cfg(test)]
mod tests {
    use super::*;

    /// `relationship` exactly as it stood before `reverse_relationship_type`
    /// was added — the shape a database created by an earlier release carries.
    const RELATIONSHIP_WITHOUT_REVERSE_TYPE: &str = "CREATE TABLE relationship (
        id                TEXT    PRIMARY KEY DEFAULT (lower(hex(randomblob(16)))),
        in_node           TEXT    NOT NULL REFERENCES node(id) ON DELETE CASCADE,
        out_node          TEXT    NOT NULL REFERENCES node(id) ON DELETE CASCADE,
        relationship_type TEXT    NOT NULL,
        properties        TEXT    NOT NULL DEFAULT '{}',
        version           INTEGER NOT NULL DEFAULT 1,
        created_at        TEXT    NOT NULL,
        modified_at       TEXT    NOT NULL
    ) STRICT";

    async fn open(path: &std::path::Path) -> libsql::Connection {
        crate::db::ensure_sqlite_vec_registered().await;
        let db = libsql::Builder::new_local(path).build().await.unwrap();
        db.connect().unwrap()
    }

    /// Build a database with the full current schema, then rebuild
    /// `relationship` without `reverse_relationship_type` — an old-shape
    /// database, carrying a row so the rebuild is not trivially empty.
    async fn old_shape_database(path: &std::path::Path) {
        let conn = open(path).await;
        create_schema(&conn).await.unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO node (id, node_type, created_at, modified_at) VALUES ('a', 'text', 't', 't');
             INSERT INTO node (id, node_type, created_at, modified_at) VALUES ('b', 'text', 't', 't');
             DROP TABLE relationship;
             {RELATIONSHIP_WITHOUT_REVERSE_TYPE};
             INSERT INTO relationship (in_node, out_node, relationship_type, created_at, modified_at)
                 VALUES ('a', 'b', 'has_child', 't', 't');"
        ))
        .await
        .unwrap();
    }

    async fn count(conn: &libsql::Connection, sql: &str) -> i64 {
        let mut rows = conn.query(sql, ()).await.unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    #[tokio::test]
    async fn a_database_missing_a_column_is_refused_with_a_schema_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        old_shape_database(&path).await;

        let conn = open(&path).await;
        let err = create_schema(&conn)
            .await
            .expect_err("an old-shape database must be refused");
        let mismatch = SchemaMismatch::find_in(&err)
            .unwrap_or_else(|| panic!("expected a SchemaMismatch, got: {err:#}"));
        assert_eq!(
            mismatch.tables,
            vec![TableShapeMismatch {
                table: "relationship".to_string(),
                missing_columns: vec!["reverse_relationship_type".to_string()],
                unexpected_columns: vec![],
            }]
        );
        let message = err.to_string();
        assert!(
            message.contains("relationship: missing reverse_relationship_type"),
            "the message must name the table and column: {message}"
        );

        // Refused, not half-applied: the transaction rolled back, so the index
        // naming the missing column was never created, the row is intact, and
        // the connection is not left inside an open transaction.
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM sqlite_master WHERE name = 'idx_rel_reverse'"
            )
            .await,
            0
        );
        assert_eq!(count(&conn, "SELECT count(*) FROM relationship").await, 1);
        assert!(conn.is_autocommit(), "the refusal must roll back");
    }

    #[tokio::test]
    async fn a_database_with_an_extra_column_is_refused_with_a_schema_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("extra.db");
        let conn = open(&path).await;
        create_schema(&conn).await.unwrap();
        conn.execute("ALTER TABLE conflict ADD COLUMN retired_field TEXT", ())
            .await
            .unwrap();

        let err = create_schema(&conn).await.unwrap_err();
        let mismatch = SchemaMismatch::find_in(&err).expect("a SchemaMismatch");
        assert_eq!(
            mismatch.tables,
            vec![TableShapeMismatch {
                table: "conflict".to_string(),
                missing_columns: vec![],
                unexpected_columns: vec!["retired_field".to_string()],
            }]
        );
    }

    #[tokio::test]
    async fn a_fresh_database_and_a_reopened_one_pass_the_shape_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.db");
        let conn = open(&path).await;
        create_schema(&conn).await.expect("fresh database");
        drop(conn);
        let conn = open(&path).await;
        create_schema(&conn)
            .await
            .expect("reopening a current-shape database");
    }

    /// A table this build adds that an older database never had is created,
    /// not reported: `IF NOT EXISTS` DDL handles a missing table on its own.
    #[tokio::test]
    async fn a_missing_table_is_created_rather_than_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.db");
        let conn = open(&path).await;
        create_schema(&conn).await.unwrap();
        conn.execute("DROP TABLE conflict_participant", ())
            .await
            .unwrap();

        create_schema(&conn)
            .await
            .expect("a missing table is not a mismatch");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM sqlite_master WHERE name = 'conflict_participant'"
            )
            .await,
            1
        );
    }

    /// The expected shape is read back from the DDL, so every ordinary table
    /// in it is covered and none of the virtual tables' internals leak in.
    #[tokio::test]
    async fn the_expected_shape_covers_every_ordinary_table_in_the_ddl() {
        let names: Vec<&str> = expected_shape()
            .await
            .unwrap()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "conflict",
                "conflict_participant",
                "embedding",
                "node",
                "relationship"
            ]
        );
        let relationship = expected_shape()
            .await
            .unwrap()
            .iter()
            .find(|t| t.name == "relationship")
            .unwrap();
        assert!(relationship
            .columns
            .iter()
            .any(|c| c == "reverse_relationship_type"));
    }
}
