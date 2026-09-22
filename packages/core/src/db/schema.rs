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
//! Connection-level PRAGMAs (`journal_mode`, `foreign_keys`, `synchronous`,
//! `busy_timeout`) are deliberately absent: they are per-connection session
//! settings rather than persisted schema state, and SQLite forbids changing
//! `synchronous` inside a transaction. `SqliteStore` sets them on every
//! connection it opens.

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
async fn create_schema_body(conn: &libsql::Connection) -> Result<()> {
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

    // FTS5 external-content index over `node.content`, kept in sync by the
    // triggers below.
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS node_fts USING fts5(id UNINDEXED, content, content='node', content_rowid='rowid')",
        ()
    ).await.context("Failed to create FTS5 table")?;

    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS node_fts_insert AFTER INSERT ON node BEGIN
            INSERT INTO node_fts(rowid, id, content) VALUES (new.rowid, new.id, new.content);
        END"#,
        (),
    )
    .await
    .context("Failed to create FTS5 insert trigger")?;

    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS node_fts_update AFTER UPDATE ON node BEGIN
            INSERT INTO node_fts(node_fts, rowid, id, content) VALUES('delete', old.rowid, old.id, old.content);
            INSERT INTO node_fts(rowid, id, content) VALUES (new.rowid, new.id, new.content);
        END"#,
        ()
    ).await.context("Failed to create FTS5 update trigger")?;

    conn.execute(
        r#"CREATE TRIGGER IF NOT EXISTS node_fts_delete AFTER DELETE ON node BEGIN
            INSERT INTO node_fts(node_fts, rowid, id, content) VALUES('delete', old.rowid, old.id, old.content);
        END"#,
        ()
    ).await.context("Failed to create FTS5 delete trigger")?;

    // FTS5 index over `node.title` — a SEPARATE table from `node_fts` above,
    // not a second column on it, because the two answer different questions:
    // `node_fts` over `content` answers "which nodes *discuss* X?" (one long row
    // per node, the whole body), while this one answers "which node *is* X?"
    // (one short row per nameable node, whose text is that node's own name).
    // Sharing a table would degrade both — a query for an entity name would rank
    // every node whose body merely mentions it alongside the node itself.
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
    // NOT an external-content table (no `content='node'`), unlike `node_fts`.
    // That is deliberate and load-bearing: FTS5 has no partial-index syntax, so
    // "only when title IS NOT NULL" lives in the triggers below — but an
    // external-content table's `'rebuild'` command re-derives every row from the
    // content table, which would silently reinstate exactly the NULL-title rows
    // the triggers skip. (`backfill_fts_if_stale` in `sqlite_store/mod.rs` issues
    // that rebuild for `node_fts`.) A standalone table owns its own rows, so the
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
pub async fn create_schema(conn: &libsql::Connection) -> Result<()> {
    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .context("Failed to begin schema-creation transaction")?;

    if let Err(e) = create_schema_body(conn).await {
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
