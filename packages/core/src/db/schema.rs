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
    properties        TEXT    NOT NULL DEFAULT '{}',
    version           INTEGER NOT NULL DEFAULT 1,
    created_at        TEXT    NOT NULL,
    modified_at       TEXT    NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_rel_type  ON relationship (relationship_type);
CREATE INDEX IF NOT EXISTS idx_rel_in    ON relationship (in_node, relationship_type);
CREATE INDEX IF NOT EXISTS idx_rel_out   ON relationship (out_node, relationship_type);
CREATE UNIQUE INDEX IF NOT EXISTS idx_rel_unique ON relationship (in_node, out_node, relationship_type);

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
pub async fn create_schema(conn: &libsql::Connection) -> Result<()> {
    // Naive `;`-splitting is safe ONLY because SCHEMA_SQL is plain CREATE
    // TABLE/INDEX statements with no semicolons inside string literals or
    // multi-statement bodies. Triggers and virtual tables are created below via
    // individual `execute` calls for exactly that reason — do not move them up
    // here without switching to a real statement splitter.
    for stmt in SCHEMA_SQL.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
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

    // Refresh planner stats so the expression indexes above are picked
    // immediately, instead of waiting for organic churn to trigger SQLite's
    // automatic ANALYZE.
    conn.execute("ANALYZE", ())
        .await
        .context("Failed to ANALYZE after creating schema")?;

    Ok(())
}
