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
//! moving the database aside and starting fresh. It compares column *names*
//! only: a change to a column's type or constraints, or to an index or trigger
//! body, is not detected, since `IF NOT EXISTS` leaves an existing index or
//! trigger as it was.
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
    created_at       TEXT    NOT NULL,
    modified_at      TEXT    NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_node_type      ON node (node_type);
CREATE INDEX IF NOT EXISTS idx_node_modified  ON node (modified_at);
CREATE INDEX IF NOT EXISTS idx_node_lifecycle ON node (lifecycle_status);

-- Expression indexes on the hot task/project properties the agent filters and
-- sorts on. `node.properties` is a JSON blob with no index on individual
-- values, so `QueryService`'s `json_extract(properties, '$.<type>.<field>')`
-- filters would otherwise evaluate `json_extract` per row (plus a filesort for
-- `ORDER BY`).
--
-- None of them filters on `node_type`. A core field stays in its base type's
-- bucket on a subtype's node (ADR-078), so an index keyed on the bucket path
-- covers `task` and every type extending it, where a `WHERE node_type = ...`
-- partial index would leave every subtype's rows unindexed (ADR-086 §5). Each
-- is partial on the value being present instead, so it holds only the rows
-- that carry the field. A comparison on the expression implies that
-- condition, which is what lets the planner use the index for it.
CREATE INDEX IF NOT EXISTS idx_task_status ON node (json_extract(properties, '$.task.status')) WHERE json_extract(properties, '$.task.status') IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_task_due_date ON node (json_extract(properties, '$.task.due_date')) WHERE json_extract(properties, '$.task.due_date') IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_task_priority ON node (json_extract(properties, '$.task.priority')) WHERE json_extract(properties, '$.task.priority') IS NOT NULL;
-- Serves "open tasks ordered by due date" (`status` equality + `due_date`
-- range/sort) without a filesort. `idx_task_status` is redundant with this one
-- for reads under SQLite's leftmost-prefix rule, but is kept: a single-column
-- index is cheaper to maintain for the common status-only filter.
CREATE INDEX IF NOT EXISTS idx_task_status_due_date ON node (json_extract(properties, '$.task.status'), json_extract(properties, '$.task.due_date')) WHERE json_extract(properties, '$.task.status') IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_project_status ON node (json_extract(properties, '$.project.status')) WHERE json_extract(properties, '$.project.status') IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_project_priority ON node (json_extract(properties, '$.project.priority')) WHERE json_extract(properties, '$.project.priority') IS NOT NULL;

-- Holds BOTH instance-level edges (person→task tasks/assignee, has_child, …)
-- and schema relationship DECLARATIONS: a declaration row connects two schema
-- nodes (in_node = declaring schema, out_node = target schema, or a self-edge
-- when untyped or targeting schema nodes) under the declared name, with the
-- full SchemaRelationship JSON in `properties` (ADR-070). The two kinds share
-- relationship_type values and are told apart by the source's node_type,
-- never by name: a declaration's source is a schema node and an instance
-- edge's never is. (An instance edge of an untyped relationship, or of one
-- declared to target schema nodes, may point at a schema node.) That is why
-- declared names may not collide with the built-in structural types.
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
    created_at   TEXT    NOT NULL,
    modified_at  TEXT    NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_emb_node      ON embedding (node_id);
CREATE INDEX IF NOT EXISTS idx_emb_stale_mod ON embedding (stale, modified_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_emb_unique ON embedding (node_id, model_name, chunk_index);

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

-- Shipped changes to a seeded node that reconciliation held back because the
-- user had edited that aspect (ADR-094 §8): one row per seeded node and
-- aspect, with the fingerprint of the shipped version the user has yet to
-- keep their own against or take.
--
-- Local bookkeeping, deliberately not a node or a node property: nothing that
-- reads nodes can see it. A deleted seed takes its rows with it.
CREATE TABLE IF NOT EXISTS pending_seed_update (
    node_id         TEXT NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    aspect          TEXT NOT NULL CHECK (aspect IN ('config', 'guidance')),
    shipped_version TEXT NOT NULL,
    recorded_at     TEXT NOT NULL,
    PRIMARY KEY (node_id, aspect)
) STRICT, WITHOUT ROWID;

-- The resolved `extends` chain of every type, so SQL can apply a base type's
-- rule to its subtypes (ADR-086 §5): one row per (type, ancestor) pair, the
-- type itself included at depth 0. `issue extends task` holds
-- (issue, issue, 0) and (issue, task, 1).
--
-- A trigger or an index cannot call the type registry, and comparing
-- `node_type` with a literal misses every subtype. Joining against this table
-- is how "is this node a collection, or a subtype of one?" is asked in SQL.
--
-- It is derived data, kept in step by the triggers below rather than by the
-- code that writes schemas: they fire inside the transaction that writes the
-- schema node or the `extends` edge, on every write path.
CREATE TABLE IF NOT EXISTS type_ancestry (
    node_type TEXT    NOT NULL,
    ancestor  TEXT    NOT NULL,
    depth     INTEGER NOT NULL,
    PRIMARY KEY (node_type, ancestor)
) STRICT, WITHOUT ROWID;

-- "Every type that is, or extends, X": the lookup a base-type rule makes.
CREATE INDEX IF NOT EXISTS idx_type_ancestry_ancestor ON type_ancestry (ancestor, node_type);

-- The structural rules each type declares (ADR-089): which children its nodes
-- may have, and where they may sit in the `has_child` tree. One row per
-- declared rule, and one per named type for the two rules that take a list:
--
--   children_none    its nodes take no children
--   children_except  its nodes take no child of type `target`
--   must_be_root     its nodes never have a parent
--   parent_of        its nodes sit only under a node of type `target`
--
-- A type with no row for a rule declares `any`. A row binds the declaring type
-- and every type extending it, and a `target` covers its own subtypes: both
-- are resolved by joining `type_ancestry`.
--
-- Like `type_ancestry` it is derived data. A core type's rows come from the
-- registry. Every other type's are kept in step with its schema node by the
-- structural-rule triggers, inside the statement that writes the schema.
CREATE TABLE IF NOT EXISTS structural_rule (
    node_type TEXT NOT NULL,
    rule      TEXT NOT NULL CHECK (rule IN ('children_none', 'children_except', 'must_be_root', 'parent_of')),
    target    TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (node_type, rule, target)
) STRICT, WITHOUT ROWID;
"#;

/// The name of the table holding every type's resolved `extends` chain.
pub const TYPE_ANCESTRY_TABLE: &str = "type_ancestry";

/// The name of the table holding every type's declared structural rules.
pub const STRUCTURAL_RULE_TABLE: &str = "structural_rule";

/// The `rule` values of [`STRUCTURAL_RULE_TABLE`].
pub mod structural_rule {
    /// The type's nodes take no children.
    pub const CHILDREN_NONE: &str = "children_none";
    /// The type's nodes take no child of the row's `target` type.
    pub const CHILDREN_EXCEPT: &str = "children_except";
    /// The type's nodes never have a parent.
    pub const MUST_BE_ROOT: &str = "must_be_root";
    /// The type's nodes sit only under a node of the row's `target` type.
    pub const PARENT_OF: &str = "parent_of";
}

/// The four checks a `has_child` edge must pass (ADR-089), for a parent of
/// type `parent_type` and a child of type `child_type` (both SQL expressions).
///
/// Each entry is `(rule, sql)`: the SQL yields a `(rule, declared_by)` row for
/// every type whose declaration of that rule the edge would break, where
/// `declared_by` is the type whose schema declares it. No row means the rule
/// allows the edge. The triggers and the store's readable pre-check both run
/// these, so the two cannot disagree about what a rule refuses.
///
/// Every check resolves a type through `type_ancestry`, so a rule declared on
/// a base type binds its subtypes and a named type covers its own.
pub fn has_child_checks(parent_type: &str, child_type: &str) -> [(&'static str, String); 4] {
    use structural_rule::{CHILDREN_EXCEPT, CHILDREN_NONE, MUST_BE_ROOT, PARENT_OF};
    let rules = STRUCTURAL_RULE_TABLE;
    let ancestry = TYPE_ANCESTRY_TABLE;
    let declared = |rule: &str, by: &str| {
        format!(
            "SELECT s.rule AS rule, s.node_type AS declared_by \
               FROM {ancestry} a JOIN {rules} s ON s.node_type = a.ancestor \
              WHERE a.node_type = {by} AND s.rule = '{rule}'"
        )
    };
    [
        (MUST_BE_ROOT, declared(MUST_BE_ROOT, child_type)),
        (CHILDREN_NONE, declared(CHILDREN_NONE, parent_type)),
        (
            CHILDREN_EXCEPT,
            format!(
                "{} AND s.target IN (SELECT ancestor FROM {ancestry} WHERE node_type = {child_type})",
                declared(CHILDREN_EXCEPT, parent_type)
            ),
        ),
        (PARENT_OF, missing_parent_sql(child_type, Some(parent_type))),
    ]
}

/// Every rule a `has_child` edge would break, as one query: the union of
/// [`has_child_checks`].
pub fn has_child_violations_sql(parent_type: &str, child_type: &str) -> String {
    has_child_checks(parent_type, child_type)
        .map(|(_, sql)| sql)
        .join(" UNION ALL ")
}

/// SQL that finds the `parent_of` rule a node of type `child_type` breaks
/// when its parent has type `parent_type`, or when it has no parent at all
/// (`None`): one `(rule, declared_by)` row per type in the child's chain
/// whose list names no type the parent is.
pub fn missing_parent_sql(child_type: &str, parent_type: Option<&str>) -> String {
    use structural_rule::PARENT_OF;
    let rules = STRUCTURAL_RULE_TABLE;
    let ancestry = TYPE_ANCESTRY_TABLE;
    let unsatisfied = match parent_type {
        Some(parent_type) => format!(
            " AND NOT EXISTS (SELECT 1 FROM {rules} t \
                               WHERE t.node_type = s.node_type AND t.rule = '{PARENT_OF}' \
                                 AND t.target IN (SELECT ancestor FROM {ancestry} \
                                                   WHERE node_type = {parent_type}))"
        ),
        None => String::new(),
    };
    format!(
        "SELECT DISTINCT s.rule AS rule, s.node_type AS declared_by \
           FROM {ancestry} a JOIN {rules} s ON s.node_type = a.ancestor \
          WHERE a.node_type = {child_type} AND s.rule = '{PARENT_OF}'{unsatisfied}"
    )
}

/// A SQL predicate that is true when `column` holds one of `bases` or a type
/// extending one of them, resolved through the ancestry table.
///
/// This is the SQL form of "apply this base type's rule" (ADR-086 §5). The
/// type ids come from the registry, never from user input, so they are
/// inlined rather than bound.
pub fn is_a_sql(column: &str, bases: &[crate::models::CoreNodeType]) -> String {
    format!(
        "{column} IN (SELECT node_type FROM {TYPE_ANCESTRY_TABLE} WHERE ancestor IN ({}))",
        sql_type_list(bases)
    )
}

/// [`is_a_sql`] for a type known only when the statement runs: a SQL predicate
/// that is true when `column` holds the type bound at `placeholder`, or a
/// type extending it.
///
/// A user-defined type is not in the registry, so it cannot be inlined the
/// way a core type is; it is bound, and resolved through the same ancestry
/// table. The equality arm covers a type with no schema, which has no
/// ancestry rows and is its own chain.
pub fn is_a_bound_sql(column: &str, placeholder: &str) -> String {
    format!(
        "({column} = {placeholder} OR {column} IN (SELECT node_type FROM {TYPE_ANCESTRY_TABLE} WHERE ancestor = {placeholder}))"
    )
}

/// The negation of [`is_a_sql`]: `column` is none of `bases` and extends none
/// of them.
pub fn is_not_a_sql(column: &str, bases: &[crate::models::CoreNodeType]) -> String {
    format!(
        "{column} NOT IN (SELECT node_type FROM {TYPE_ANCESTRY_TABLE} WHERE ancestor IN ({}))",
        sql_type_list(bases)
    )
}

/// A SQL predicate that is true when `column` holds exactly the stored id of
/// `core`. Only for a type nothing can extend: the `schema` meta-type, which
/// has no schema node of its own to be an `extends` target.
pub fn is_exactly_sql(column: &str, core: crate::models::CoreNodeType) -> String {
    format!("{column} = '{}'", core.as_str())
}

/// Registry type ids as a quoted SQL list: `'collection', 'schema'`.
fn sql_type_list(types: &[crate::models::CoreNodeType]) -> String {
    types
        .iter()
        .map(|t| format!("'{}'", t.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

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

    create_type_ancestry_objects(conn).await?;

    create_structural_rule_objects(conn).await?;

    // Whether a row is a core schema is fixed when it is created. The core
    // schema delete refusal reads `isCore` from the row, so an update that
    // clears it — or retypes the row away from `schema` — would make a core
    // type deletable; one that sets it would make a user type undeletable.
    let old_is_schema = is_exactly_sql("old.node_type", crate::models::CoreNodeType::Schema);
    let new_is_schema = is_exactly_sql("new.node_type", crate::models::CoreNodeType::Schema);
    conn.execute(
        &format!(
            r#"CREATE TRIGGER IF NOT EXISTS schema_core_status_fixed BEFORE UPDATE OF node_type, properties ON node
            WHEN ({old_is_schema} AND coalesce(json_type(old.properties, '$.isCore') = 'true', 0))
              IS NOT ({new_is_schema} AND coalesce(json_type(new.properties, '$.isCore') = 'true', 0))
            BEGIN
                SELECT RAISE(ABORT, 'schema_is_core: whether a schema is core is fixed when it is created');
            END"#
        ),
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

/// The rows and triggers that keep [`TYPE_ANCESTRY_TABLE`] equal to the
/// closure of the schema `extends` edges.
///
/// The table is maintained entirely in SQL so that no write path can leave it
/// behind: each trigger runs inside the statement that changed a schema node
/// or an `extends` edge, and so inside that write's transaction.
///
/// A schema has at most one parent, which is what makes the incremental
/// updates exact: every path from a descendant of `C` to an ancestor of `P`
/// runs through the one edge `C -> P`, so adding or removing that edge adds or
/// removes exactly the pairs (descendant-or-self of `C`) x (ancestor-or-self
/// of `P`).
async fn create_type_ancestry_objects(conn: &libsql::Connection) -> Result<()> {
    use crate::models::schema::EXTENDS_RELATIONSHIP;
    use crate::models::CoreNodeType;

    // The core types' chains come from the registry, so a base-type rule holds
    // from the first statement on a new database, before any schema node is
    // seeded. `schema` is the one core type with no schema node of its own, so
    // this is also the only place its row comes from.
    for core in CoreNodeType::ALL {
        for (depth, ancestor) in core.chain().into_iter().enumerate() {
            conn.execute(
                &format!(
                    "INSERT OR IGNORE INTO {TYPE_ANCESTRY_TABLE} (node_type, ancestor, depth) \
                     VALUES (?1, ?2, ?3)"
                ),
                libsql::params![core.as_str(), ancestor.as_str(), depth as i64],
            )
            .await
            .with_context(|| format!("Failed to seed the type ancestry of '{core}'"))?;
        }
    }

    let new_is_schema = is_exactly_sql("new.node_type", CoreNodeType::Schema);
    let old_is_schema = is_exactly_sql("old.node_type", CoreNodeType::Schema);
    // A core type's chain is the registry's and outlives any schema node that
    // shares its id, so the rows seeded above are never dropped. `schema` is
    // the case that matters: it has no schema node of its own, and a user
    // schema named after it must not take the meta-type's row with it.
    let old_is_not_core = format!("old.id NOT IN ({})", sql_type_list(&CoreNodeType::ALL));

    // A schema node is a type: it is its own ancestor at depth 0.
    let add_self = format!(
        "INSERT OR IGNORE INTO {TYPE_ANCESTRY_TABLE} (node_type, ancestor, depth) \
         VALUES (new.id, new.id, 0);"
    );
    // A type that stops existing takes its whole chain with it: its own rows,
    // and the pairs that ran through it for every type extending it.
    let drop_type = format!(
        "DELETE FROM {TYPE_ANCESTRY_TABLE} \
         WHERE node_type IN (SELECT node_type FROM {TYPE_ANCESTRY_TABLE} WHERE ancestor = old.id) \
           AND ancestor IN (SELECT ancestor FROM {TYPE_ANCESTRY_TABLE} WHERE node_type = old.id);"
    );
    // `in_node` extends `out_node`: every descendant-or-self of the child
    // gains every ancestor-or-self of the parent.
    let link = format!(
        "INSERT OR REPLACE INTO {TYPE_ANCESTRY_TABLE} (node_type, ancestor, depth) \
         SELECT d.node_type, a.ancestor, d.depth + 1 + a.depth \
         FROM {TYPE_ANCESTRY_TABLE} d, {TYPE_ANCESTRY_TABLE} a \
         WHERE d.ancestor = new.in_node AND a.node_type = new.out_node;"
    );
    let unlink = format!(
        "DELETE FROM {TYPE_ANCESTRY_TABLE} \
         WHERE node_type IN (SELECT node_type FROM {TYPE_ANCESTRY_TABLE} WHERE ancestor = old.in_node) \
           AND ancestor IN (SELECT ancestor FROM {TYPE_ANCESTRY_TABLE} WHERE node_type = old.out_node);"
    );

    let triggers = [
        format!(
            "CREATE TRIGGER IF NOT EXISTS type_ancestry_schema_insert AFTER INSERT ON node \
             WHEN {new_is_schema} BEGIN {add_self} END"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS type_ancestry_schema_delete AFTER DELETE ON node \
             WHEN {old_is_schema} AND {old_is_not_core} BEGIN {drop_type} END"
        ),
        // A node retyped into or out of `schema` starts or stops being a type.
        format!(
            "CREATE TRIGGER IF NOT EXISTS type_ancestry_schema_retype_in AFTER UPDATE OF node_type ON node \
             WHEN {new_is_schema} AND NOT {old_is_schema} BEGIN {add_self} END"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS type_ancestry_schema_retype_out AFTER UPDATE OF node_type ON node \
             WHEN {old_is_schema} AND NOT {new_is_schema} AND {old_is_not_core} \
             BEGIN {drop_type} END"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS type_ancestry_extends_insert AFTER INSERT ON relationship \
             WHEN new.relationship_type = '{EXTENDS_RELATIONSHIP}' BEGIN {link} END"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS type_ancestry_extends_delete AFTER DELETE ON relationship \
             WHEN old.relationship_type = '{EXTENDS_RELATIONSHIP}' BEGIN {unlink} END"
        ),
        // A re-target rewrites `out_node` in place, so the old edge's pairs go
        // before the new edge's are added.
        format!(
            "CREATE TRIGGER IF NOT EXISTS type_ancestry_extends_update \
             AFTER UPDATE OF in_node, out_node, relationship_type ON relationship \
             WHEN old.relationship_type = '{EXTENDS_RELATIONSHIP}' \
               OR new.relationship_type = '{EXTENDS_RELATIONSHIP}' \
             BEGIN \
               DELETE FROM {TYPE_ANCESTRY_TABLE} \
               WHERE old.relationship_type = '{EXTENDS_RELATIONSHIP}' \
                 AND node_type IN (SELECT node_type FROM {TYPE_ANCESTRY_TABLE} WHERE ancestor = old.in_node) \
                 AND ancestor IN (SELECT ancestor FROM {TYPE_ANCESTRY_TABLE} WHERE node_type = old.out_node); \
               INSERT OR REPLACE INTO {TYPE_ANCESTRY_TABLE} (node_type, ancestor, depth) \
               SELECT d.node_type, a.ancestor, d.depth + 1 + a.depth \
               FROM {TYPE_ANCESTRY_TABLE} d, {TYPE_ANCESTRY_TABLE} a \
               WHERE new.relationship_type = '{EXTENDS_RELATIONSHIP}' \
                 AND d.ancestor = new.in_node AND a.node_type = new.out_node; \
             END"
        ),
    ];
    for trigger in triggers {
        conn.execute(&trigger, ())
            .await
            .with_context(|| format!("Failed to create type-ancestry trigger: {trigger}"))?;
    }
    Ok(())
}

/// The rows and triggers behind the structural rules (ADR-089): what a type's
/// nodes may have as children, and where they may sit in the `has_child` tree.
///
/// Enforced here rather than at each Rust insert site because `has_child`
/// edges are written from a dozen places (create, append, move, bulk hierarchy
/// attach, seeding, generic relationship create) and a node can take a type
/// with tighter rules by a type switch. Foreign keys are immediate, so both
/// node rows exist by the time their edge is inserted.
///
/// A trigger cannot see an edge that is never written, so one rule is not
/// enforced here: a node whose type needs a parent, created or left without
/// one. The store's create and move paths refuse that before they write
/// (`SqliteStore::assert_may_be_root`).
async fn create_structural_rule_objects(conn: &libsql::Connection) -> Result<()> {
    use crate::models::{ChildrenRule, CoreNodeType, ParentRule};
    use structural_rule::{CHILDREN_EXCEPT, CHILDREN_NONE, MUST_BE_ROOT, PARENT_OF};

    // The core types' rules come from the registry, like their chains: they
    // hold from the first statement on a new database, and `schema`, which has
    // no schema node of its own, gets its row nowhere else.
    for core in CoreNodeType::ALL {
        let declared = core.declared_structure();
        let mut rows: Vec<(&str, &str)> = Vec::new();
        match declared.children {
            ChildrenRule::Any => {}
            ChildrenRule::None => rows.push((CHILDREN_NONE, "")),
            ChildrenRule::AnyExcept(types) => {
                rows.extend(types.iter().map(|t| (CHILDREN_EXCEPT, t.as_str())));
            }
        }
        match declared.parent {
            ParentRule::Any => {}
            ParentRule::MustBeRoot => rows.push((MUST_BE_ROOT, "")),
            ParentRule::MustHaveParentOf(types) => {
                rows.extend(types.iter().map(|t| (PARENT_OF, t.as_str())));
            }
        }
        for (rule, target) in rows {
            conn.execute(
                &format!(
                    "INSERT OR IGNORE INTO {STRUCTURAL_RULE_TABLE} (node_type, rule, target) \
                     VALUES (?1, ?2, ?3)"
                ),
                libsql::params![core.as_str(), rule, target],
            )
            .await
            .with_context(|| format!("Failed to seed the structural rules of '{core}'"))?;
        }
    }

    let new_is_schema = is_exactly_sql("new.node_type", CoreNodeType::Schema);
    let old_is_schema = is_exactly_sql("old.node_type", CoreNodeType::Schema);
    // A core type's rules are the registry's, whatever its schema node says.
    let core_ids = sql_type_list(&CoreNodeType::ALL);
    let new_is_not_core = format!("new.id NOT IN ({core_ids})");
    let old_is_not_core = format!("old.id NOT IN ({core_ids})");

    // The schema node `new`'s `children` and `parent` properties, as rule
    // rows. A `json_each` over a path that is absent yields no rows.
    let declare = format!(
        "INSERT OR IGNORE INTO {STRUCTURAL_RULE_TABLE} (node_type, rule, target) \
           SELECT new.id, '{CHILDREN_NONE}', '' \
            WHERE {new_is_schema} AND json_extract(new.properties, '$.children.rule') = 'none'; \
         INSERT OR IGNORE INTO {STRUCTURAL_RULE_TABLE} (node_type, rule, target) \
           SELECT new.id, '{CHILDREN_EXCEPT}', j.value \
             FROM json_each(new.properties, '$.children.types') j \
            WHERE {new_is_schema} \
              AND json_extract(new.properties, '$.children.rule') = 'any_except'; \
         INSERT OR IGNORE INTO {STRUCTURAL_RULE_TABLE} (node_type, rule, target) \
           SELECT new.id, '{MUST_BE_ROOT}', '' \
            WHERE {new_is_schema} \
              AND json_extract(new.properties, '$.parent.rule') = 'must_be_root'; \
         INSERT OR IGNORE INTO {STRUCTURAL_RULE_TABLE} (node_type, rule, target) \
           SELECT new.id, '{PARENT_OF}', j.value \
             FROM json_each(new.properties, '$.parent.types') j \
            WHERE {new_is_schema} \
              AND json_extract(new.properties, '$.parent.rule') = 'must_have_parent_of';"
    );
    let undeclare = format!("DELETE FROM {STRUCTURAL_RULE_TABLE} WHERE node_type = old.id;");

    // One `RAISE` per rule: its message is a literal, and it leads with the
    // rule's name, as `TreeInvariantRule::as_str` does for the Rust guards.
    let message = |rule: &str| match rule {
        MUST_BE_ROOT => {
            "must_be_root: a node of this type is always a root and cannot have a parent"
        }
        CHILDREN_NONE => "children_none: a node of this type cannot have children",
        CHILDREN_EXCEPT => {
            "child_not_allowed: a node of this type cannot have a child of that type"
        }
        _ => "parent_required: a node of this type cannot sit under a parent of that type",
    };
    let raise = |rule: &str, broken: String| {
        format!("SELECT RAISE(ABORT, '{}') WHERE {broken};", message(rule))
    };

    // The `has_child` edge `new` must break no rule of either end's type.
    let refuse_edge = has_child_checks(
        "(SELECT node_type FROM node WHERE id = new.in_node)",
        "(SELECT node_type FROM node WHERE id = new.out_node)",
    )
    .map(|(rule, check)| raise(rule, format!("EXISTS ({check})")))
    .join(" ");

    // The node `new` takes a type: its rules must hold against the parent it
    // has and each child it has, and theirs against it.
    let as_child = has_child_checks("pn.node_type", "new.node_type");
    let as_parent = has_child_checks("new.node_type", "cn.node_type");
    let refuse_retype = as_child
        .into_iter()
        .zip(as_parent)
        .map(|((rule, under_parent), (_, over_child))| {
            raise(
                rule,
                format!(
                    "EXISTS (SELECT 1 FROM relationship up JOIN node pn ON pn.id = up.in_node \
                              WHERE up.out_node = new.id AND up.relationship_type = 'has_child' \
                                AND EXISTS ({under_parent})) \
                     OR EXISTS (SELECT 1 FROM relationship down \
                                  JOIN node cn ON cn.id = down.out_node \
                                 WHERE down.in_node = new.id \
                                   AND down.relationship_type = 'has_child' \
                                   AND EXISTS ({over_child}))"
                ),
            )
        })
        .collect::<Vec<_>>()
        .join(" ");

    let triggers = [
        format!(
            "CREATE TRIGGER IF NOT EXISTS structural_rule_schema_insert AFTER INSERT ON node \
             WHEN {new_is_schema} AND {new_is_not_core} BEGIN {declare} END"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS structural_rule_schema_delete AFTER DELETE ON node \
             WHEN {old_is_schema} AND {old_is_not_core} BEGIN {undeclare} END"
        ),
        // A schema whose declaration changed, or a node retyped into or out
        // of `schema`: its rows are rewritten from what it now declares.
        format!(
            "CREATE TRIGGER IF NOT EXISTS structural_rule_schema_update \
             AFTER UPDATE OF node_type, properties ON node \
             WHEN ({old_is_schema} OR {new_is_schema}) AND {old_is_not_core} \
             BEGIN {undeclare} {declare} END"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS structure_has_child_insert BEFORE INSERT ON relationship \
             WHEN new.relationship_type = 'has_child' BEGIN {refuse_edge} END"
        ),
        // An edge re-pointed in place, or turned into a `has_child` edge.
        format!(
            "CREATE TRIGGER IF NOT EXISTS structure_has_child_update \
             BEFORE UPDATE OF in_node, out_node, relationship_type ON relationship \
             WHEN new.relationship_type = 'has_child' BEGIN {refuse_edge} END"
        ),
        format!(
            "CREATE TRIGGER IF NOT EXISTS structure_node_retype BEFORE UPDATE OF node_type ON node \
             WHEN new.node_type IS NOT old.node_type BEGIN {refuse_retype} END"
        ),
    ];
    for trigger in triggers {
        conn.execute(&trigger, ())
            .await
            .with_context(|| format!("Failed to create structural-rule trigger: {trigger}"))?;
    }
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
                "relationship",
                "structural_rule",
                "type_ancestry"
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

    // ---- The type-ancestry table (ADR-086 §5) ----

    async fn fresh() -> (libsql::Connection, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("ancestry.db")).await;
        create_schema(&conn).await.unwrap();
        conn.execute("PRAGMA foreign_keys = ON", ()).await.unwrap();
        (conn, dir)
    }

    async fn add_schema(conn: &libsql::Connection, id: &str) {
        conn.execute(
            "INSERT INTO node (id, node_type, created_at, modified_at) VALUES (?1, 'schema', 't', 't')",
            libsql::params![id],
        )
        .await
        .unwrap();
    }

    async fn extend(conn: &libsql::Connection, child: &str, parent: &str) {
        conn.execute(
            "INSERT INTO relationship (in_node, out_node, relationship_type, created_at, modified_at) \
             VALUES (?1, ?2, 'extends', 't', 't')",
            libsql::params![child, parent],
        )
        .await
        .unwrap();
    }

    /// `node_type`'s chain as the table holds it: `(ancestor, depth)` nearest
    /// first.
    async fn chain(conn: &libsql::Connection, node_type: &str) -> Vec<(String, i64)> {
        let mut rows = conn
            .query(
                "SELECT ancestor, depth FROM type_ancestry WHERE node_type = ?1 ORDER BY depth",
                libsql::params![node_type],
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push((row.get::<String>(0).unwrap(), row.get::<i64>(1).unwrap()));
        }
        out
    }

    fn pairs(items: &[(&str, i64)]) -> Vec<(String, i64)> {
        items.iter().map(|(a, d)| (a.to_string(), *d)).collect()
    }

    /// Every core type is its own ancestor from the first statement on a new
    /// database, before any schema node exists, the `schema` meta-type
    /// included.
    #[tokio::test]
    async fn a_fresh_database_holds_every_core_types_chain() {
        let (conn, _dir) = fresh().await;
        for core in crate::models::CoreNodeType::ALL {
            let expected: Vec<(String, i64)> = core
                .chain()
                .into_iter()
                .enumerate()
                .map(|(depth, t)| (t.as_str().to_string(), depth as i64))
                .collect();
            assert_eq!(chain(&conn, core.as_str()).await, expected, "{core}");
        }
    }

    /// Adding an `extends` edge gives the subtype, and every type below it,
    /// the whole chain above; removing it takes exactly those pairs away.
    #[tokio::test]
    async fn the_ancestry_follows_extends_edges_as_they_are_added_and_removed() {
        let (conn, _dir) = fresh().await;
        for id in ["base", "mid", "leaf"] {
            add_schema(&conn, id).await;
        }
        assert_eq!(chain(&conn, "leaf").await, pairs(&[("leaf", 0)]));

        // Linked bottom-up, so the second edge has to lift `leaf` too.
        extend(&conn, "leaf", "mid").await;
        extend(&conn, "mid", "base").await;
        assert_eq!(
            chain(&conn, "leaf").await,
            pairs(&[("leaf", 0), ("mid", 1), ("base", 2)])
        );
        assert_eq!(chain(&conn, "mid").await, pairs(&[("mid", 0), ("base", 1)]));
        assert_eq!(chain(&conn, "base").await, pairs(&[("base", 0)]));

        conn.execute(
            "DELETE FROM relationship WHERE in_node = 'mid' AND relationship_type = 'extends'",
            (),
        )
        .await
        .unwrap();
        assert_eq!(chain(&conn, "mid").await, pairs(&[("mid", 0)]));
        assert_eq!(
            chain(&conn, "leaf").await,
            pairs(&[("leaf", 0), ("mid", 1)]),
            "the leaf keeps its own parent and loses what was above it"
        );
    }

    /// A re-target rewrites the edge in place; the old chain goes and the new
    /// one arrives in the same statement.
    #[tokio::test]
    async fn retargeting_an_extends_edge_swaps_the_chain() {
        let (conn, _dir) = fresh().await;
        for id in ["old_base", "new_base", "kind", "sub_kind"] {
            add_schema(&conn, id).await;
        }
        extend(&conn, "kind", "old_base").await;
        extend(&conn, "sub_kind", "kind").await;

        conn.execute(
            "UPDATE relationship SET out_node = 'new_base' \
             WHERE in_node = 'kind' AND relationship_type = 'extends'",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            chain(&conn, "kind").await,
            pairs(&[("kind", 0), ("new_base", 1)])
        );
        assert_eq!(
            chain(&conn, "sub_kind").await,
            pairs(&[("sub_kind", 0), ("kind", 1), ("new_base", 2)])
        );
    }

    /// A subtype of a core type reaches the core type's row, which is what
    /// lets a rule written against the core type hold for it.
    #[tokio::test]
    async fn a_subtype_of_a_core_type_resolves_to_it() {
        let (conn, _dir) = fresh().await;
        add_schema(&conn, "collection").await;
        add_schema(&conn, "team").await;
        extend(&conn, "team", "collection").await;
        assert_eq!(
            chain(&conn, "team").await,
            pairs(&[("team", 0), ("collection", 1)])
        );
    }

    /// The table is written by the statement that writes the schema or the
    /// edge, so it is inside that write's transaction: a rollback leaves no
    /// trace of either.
    #[tokio::test]
    async fn the_ancestry_is_written_in_the_schema_writes_transaction() {
        let (conn, _dir) = fresh().await;
        add_schema(&conn, "base").await;

        conn.execute("BEGIN", ()).await.unwrap();
        add_schema(&conn, "draft").await;
        extend(&conn, "draft", "base").await;
        assert_eq!(
            chain(&conn, "draft").await,
            pairs(&[("draft", 0), ("base", 1)]),
            "visible inside the transaction that wrote the edge"
        );
        conn.execute("ROLLBACK", ()).await.unwrap();

        assert!(chain(&conn, "draft").await.is_empty());
        assert_eq!(chain(&conn, "base").await, pairs(&[("base", 0)]));
    }

    /// Deleting a schema node removes its type: its own rows, and the edge
    /// its `extends` declaration cascades away.
    #[tokio::test]
    async fn deleting_a_schema_removes_its_ancestry() {
        let (conn, _dir) = fresh().await;
        for id in ["base", "kind"] {
            add_schema(&conn, id).await;
        }
        extend(&conn, "kind", "base").await;

        conn.execute("DELETE FROM node WHERE id = 'kind'", ())
            .await
            .unwrap();
        assert!(chain(&conn, "kind").await.is_empty());
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM type_ancestry WHERE ancestor = 'kind'"
            )
            .await,
            0
        );
        assert_eq!(chain(&conn, "base").await, pairs(&[("base", 0)]));
    }

    async fn add_node(conn: &libsql::Connection, id: &str, node_type: &str) {
        conn.execute(
            "INSERT INTO node (id, node_type, created_at, modified_at) VALUES (?1, ?2, 't', 't')",
            libsql::params![id, node_type],
        )
        .await
        .unwrap();
    }

    async fn add_child(
        conn: &libsql::Connection,
        parent: &str,
        child: &str,
    ) -> std::result::Result<u64, libsql::Error> {
        conn.execute(
            "INSERT INTO relationship (in_node, out_node, relationship_type, created_at, modified_at) \
             VALUES (?1, ?2, 'has_child', 't', 't')",
            libsql::params![parent, child],
        )
        .await
    }

    /// A core type's row survives a schema node of the same id being deleted.
    /// `schema` has no schema node of its own, so its row has no other source.
    #[tokio::test]
    async fn deleting_a_schema_named_after_a_core_type_keeps_the_core_row() {
        let (conn, _dir) = fresh().await;
        for id in ["schema", "task"] {
            add_schema(&conn, id).await;
            conn.execute("DELETE FROM node WHERE id = ?1", libsql::params![id])
                .await
                .unwrap();
            assert_eq!(chain(&conn, id).await, pairs(&[(id, 0)]), "{id}");
        }
    }

    /// The root-only rule is enforced against the ancestry table, so it holds
    /// for a subtype of a root-only type on both paths that could break it:
    /// giving such a node a parent, and retyping a node that has one.
    #[tokio::test]
    async fn a_subtype_of_a_root_only_type_is_root_only() {
        let (conn, _dir) = fresh().await;
        add_schema(&conn, "collection").await;
        add_schema(&conn, "team").await;
        extend(&conn, "team", "collection").await;

        add_node(&conn, "page", "text").await;
        add_node(&conn, "note", "text").await;
        add_node(&conn, "core-team", "team").await;

        let err = add_child(&conn, "page", "core-team")
            .await
            .expect_err("a team is a collection, and a collection is a root");
        assert!(err.to_string().contains("must_be_root"), "{err}");

        add_child(&conn, "page", "note").await.unwrap();
        let err = conn
            .execute("UPDATE node SET node_type = 'team' WHERE id = 'note'", ())
            .await
            .expect_err("a node with a parent cannot become a team");
        assert!(err.to_string().contains("must_be_root"), "{err}");

        // A team may still hold children of its own.
        add_child(&conn, "core-team", "page").await.unwrap();
    }

    /// A schema node declaring `properties`, as a user-defined type does.
    async fn add_schema_declaring(conn: &libsql::Connection, id: &str, properties: &str) {
        conn.execute(
            "INSERT INTO node (id, node_type, properties, created_at, modified_at) \
             VALUES (?1, 'schema', ?2, 't', 't')",
            libsql::params![id, properties],
        )
        .await
        .unwrap();
    }

    async fn rules(conn: &libsql::Connection, node_type: &str) -> Vec<(String, String)> {
        let mut rows = conn
            .query(
                "SELECT rule, target FROM structural_rule WHERE node_type = ?1 ORDER BY rule, target",
                libsql::params![node_type],
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            out.push((row.get(0).unwrap(), row.get(1).unwrap()));
        }
        out
    }

    fn rule_rows(rows: &[(&str, &str)]) -> Vec<(String, String)> {
        rows.iter()
            .map(|(rule, target)| (rule.to_string(), target.to_string()))
            .collect()
    }

    /// The core types' rules are in the table from the first statement, with
    /// no schema node seeded: they come from the registry.
    #[tokio::test]
    async fn the_registrys_structural_rules_are_seeded() {
        let (conn, _dir) = fresh().await;
        for root_only in ["collection", "schema", "date"] {
            assert_eq!(
                rules(&conn, root_only).await,
                rule_rows(&[("must_be_root", "")]),
                "{root_only}"
            );
        }
        for leaf in [
            "code-block",
            "ordered-list",
            "horizontal-line",
            "table",
            "query",
            "tool",
            "database-settings",
        ] {
            assert_eq!(
                rules(&conn, leaf).await,
                rule_rows(&[("children_none", "")]),
                "{leaf}"
            );
        }
        for open in ["text", "task", "ai-chat"] {
            assert!(rules(&conn, open).await.is_empty(), "{open}");
        }
    }

    /// A schema's declared rules are rows from the statement that writes the
    /// schema, follow an update of the declaration, and go with the schema.
    #[tokio::test]
    async fn a_schemas_declared_rules_follow_the_schema_node() {
        let (conn, _dir) = fresh().await;
        add_schema_declaring(
            &conn,
            "reply",
            r#"{"children":{"rule":"none"},"parent":{"rule":"must_have_parent_of","types":["thread","task"]}}"#,
        )
        .await;
        assert_eq!(
            rules(&conn, "reply").await,
            rule_rows(&[
                ("children_none", ""),
                ("parent_of", "task"),
                ("parent_of", "thread")
            ])
        );

        conn.execute(
            r#"UPDATE node SET properties = '{"children":{"rule":"any_except","types":["collection"]},"parent":{"rule":"must_be_root"}}' WHERE id = 'reply'"#,
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            rules(&conn, "reply").await,
            rule_rows(&[("children_except", "collection"), ("must_be_root", "")])
        );

        conn.execute("DELETE FROM node WHERE id = 'reply'", ())
            .await
            .unwrap();
        assert!(rules(&conn, "reply").await.is_empty());
    }

    /// A core type's rules are the registry's: a schema node of its id that
    /// declares something else, or goes away, changes nothing.
    #[tokio::test]
    async fn a_core_types_rules_ignore_its_schema_node() {
        let (conn, _dir) = fresh().await;
        add_schema_declaring(&conn, "query", r#"{"children":{"rule":"any"}}"#).await;
        add_schema_declaring(&conn, "text", r#"{"children":{"rule":"none"}}"#).await;
        assert_eq!(
            rules(&conn, "query").await,
            rule_rows(&[("children_none", "")])
        );
        assert!(rules(&conn, "text").await.is_empty());

        conn.execute("UPDATE node SET properties = '{}' WHERE id = 'query'", ())
            .await
            .unwrap();
        conn.execute("DELETE FROM node WHERE id = 'query'", ())
            .await
            .unwrap();
        assert_eq!(
            rules(&conn, "query").await,
            rule_rows(&[("children_none", "")])
        );
    }

    /// `children: none` refuses every child, for the type and its subtypes,
    /// on an edge insert and on a retype of a node that has children.
    #[tokio::test]
    async fn a_childless_type_refuses_children() {
        let (conn, _dir) = fresh().await;
        add_schema(&conn, "query").await;
        add_schema(&conn, "saved-search").await;
        extend(&conn, "saved-search", "query").await;

        add_node(&conn, "note", "text").await;
        add_node(&conn, "other", "text").await;
        for (id, node_type) in [("q", "query"), ("s", "saved-search")] {
            add_node(&conn, id, node_type).await;
            let err = add_child(&conn, id, "note")
                .await
                .expect_err("a query takes no children");
            assert!(err.to_string().contains("children_none"), "{err}");
        }

        add_child(&conn, "note", "other").await.unwrap();
        let err = conn
            .execute(
                "UPDATE node SET node_type = 'saved-search' WHERE id = 'note'",
                (),
            )
            .await
            .expect_err("a node with children cannot become a query");
        assert!(err.to_string().contains("children_none"), "{err}");

        // A leaf may itself be a child.
        add_child(&conn, "other", "q").await.unwrap();
    }

    /// `any_except` refuses the named types and their subtypes, and nothing
    /// else, on an insert and on a retype of the child.
    #[tokio::test]
    async fn any_except_refuses_the_named_types_and_their_subtypes() {
        let (conn, _dir) = fresh().await;
        add_schema(&conn, "task").await;
        add_schema(&conn, "issue").await;
        extend(&conn, "issue", "task").await;
        add_schema_declaring(
            &conn,
            "journal",
            r#"{"children":{"rule":"any_except","types":["task"]}}"#,
        )
        .await;

        add_node(&conn, "j", "journal").await;
        add_node(&conn, "note", "text").await;
        add_node(&conn, "t", "task").await;
        add_node(&conn, "i", "issue").await;

        for refused in ["t", "i"] {
            let err = add_child(&conn, "j", refused)
                .await
                .expect_err("a journal takes no task");
            assert!(err.to_string().contains("child_not_allowed"), "{err}");
        }
        add_child(&conn, "j", "note").await.unwrap();

        let err = conn
            .execute("UPDATE node SET node_type = 'issue' WHERE id = 'note'", ())
            .await
            .expect_err("a journal's child cannot become a task");
        assert!(err.to_string().contains("child_not_allowed"), "{err}");
    }

    /// `must_have_parent_of` accepts only the named types and their subtypes
    /// as a parent, on an insert, on a re-pointed edge, and when the parent or
    /// the child is retyped.
    #[tokio::test]
    async fn must_have_parent_of_accepts_only_the_named_parents() {
        let (conn, _dir) = fresh().await;
        add_schema(&conn, "thread").await;
        add_schema(&conn, "support-thread").await;
        extend(&conn, "support-thread", "thread").await;
        add_schema_declaring(
            &conn,
            "reply",
            r#"{"parent":{"rule":"must_have_parent_of","types":["thread"]}}"#,
        )
        .await;

        add_node(&conn, "page", "text").await;
        add_node(&conn, "th", "thread").await;
        add_node(&conn, "sth", "support-thread").await;
        add_node(&conn, "r1", "reply").await;
        add_node(&conn, "r2", "reply").await;
        add_node(&conn, "note", "text").await;

        let err = add_child(&conn, "page", "r1")
            .await
            .expect_err("a reply sits only under a thread");
        assert!(err.to_string().contains("parent_required"), "{err}");
        add_child(&conn, "th", "r1").await.unwrap();
        add_child(&conn, "sth", "r2").await.unwrap();

        let err = conn
            .execute(
                "UPDATE relationship SET in_node = 'page' WHERE out_node = 'r1'",
                (),
            )
            .await
            .expect_err("a reply cannot be re-pointed under a page");
        assert!(err.to_string().contains("parent_required"), "{err}");

        let err = conn
            .execute("UPDATE node SET node_type = 'text' WHERE id = 'th'", ())
            .await
            .expect_err("a thread holding replies cannot stop being a thread");
        assert!(err.to_string().contains("parent_required"), "{err}");

        add_child(&conn, "page", "note").await.unwrap();
        let err = conn
            .execute("UPDATE node SET node_type = 'reply' WHERE id = 'note'", ())
            .await
            .expect_err("a page's child cannot become a reply");
        assert!(err.to_string().contains("parent_required"), "{err}");
    }

    /// The hand-written per-type root triggers are gone: one set of generic
    /// triggers enforces every declared rule.
    #[tokio::test]
    async fn the_per_type_root_triggers_are_replaced_by_the_structural_ones() {
        let (conn, _dir) = fresh().await;
        let mut rows = conn
            .query(
                "SELECT name FROM sqlite_master WHERE type = 'trigger' \
                   AND (name LIKE '%is_root%' OR name LIKE 'structure_%') ORDER BY name",
                (),
            )
            .await
            .unwrap();
        let mut names: Vec<String> = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            names.push(row.get(0).unwrap());
        }
        assert_eq!(
            names,
            vec![
                "structure_has_child_insert",
                "structure_has_child_update",
                "structure_node_retype"
            ]
        );
    }

    /// A retype that changes nothing structural goes through, and other edge
    /// types are not the rules' business.
    #[tokio::test]
    async fn the_rules_leave_other_writes_alone() {
        let (conn, _dir) = fresh().await;
        add_node(&conn, "page", "text").await;
        add_node(&conn, "note", "text").await;
        add_node(&conn, "q", "query").await;
        add_child(&conn, "page", "note").await.unwrap();
        conn.execute("UPDATE node SET node_type = 'task' WHERE id = 'note'", ())
            .await
            .unwrap();
        conn.execute(
            "INSERT INTO relationship (in_node, out_node, relationship_type, created_at, modified_at) \
             VALUES ('q', 'page', 'mentions', 't', 't')",
            (),
        )
        .await
        .unwrap();
    }
}
