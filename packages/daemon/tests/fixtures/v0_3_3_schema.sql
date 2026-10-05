-- The schema a v0.3.3 `nodespaced` created, copied from that release's
-- `packages/core/src/db/schema.rs`, with one node written into it.
--
-- When this fixture was added, every table here had the columns that build
-- defined. What a v0.3.3 database lacks is the tables added since,
-- `type_ancestry` and `structural_rule` among them (with the triggers and
-- rows that maintain them), so the table-set comparison is what tells it apart
-- from a current one. The vec0 table needs `sqlite-vec` registered on the
-- connection that runs this.

CREATE TABLE node (
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
CREATE INDEX idx_node_type      ON node (node_type);
CREATE INDEX idx_node_modified  ON node (modified_at);
CREATE INDEX idx_node_lifecycle ON node (lifecycle_status);
CREATE INDEX idx_task_status ON node (json_extract(properties, '$.task.status')) WHERE node_type = 'task';
CREATE INDEX idx_task_due_date ON node (json_extract(properties, '$.task.due_date')) WHERE node_type = 'task';
CREATE INDEX idx_task_priority ON node (json_extract(properties, '$.task.priority')) WHERE node_type = 'task';
CREATE INDEX idx_task_status_due_date ON node (json_extract(properties, '$.task.status'), json_extract(properties, '$.task.due_date')) WHERE node_type = 'task';
CREATE INDEX idx_project_status ON node (json_extract(properties, '$.project.status')) WHERE node_type = 'project';
CREATE INDEX idx_project_priority ON node (json_extract(properties, '$.project.priority')) WHERE node_type = 'project';

CREATE TABLE relationship (
    id                TEXT    PRIMARY KEY DEFAULT (lower(hex(randomblob(16)))),
    in_node           TEXT    NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    out_node          TEXT    NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    relationship_type TEXT    NOT NULL,
    reverse_relationship_type TEXT,
    properties        TEXT    NOT NULL DEFAULT '{}',
    version           INTEGER NOT NULL DEFAULT 1,
    created_at        TEXT    NOT NULL,
    modified_at       TEXT    NOT NULL
) STRICT;
CREATE INDEX idx_rel_type  ON relationship (relationship_type);
CREATE INDEX idx_rel_in    ON relationship (in_node, relationship_type);
CREATE INDEX idx_rel_out   ON relationship (out_node, relationship_type);
CREATE UNIQUE INDEX idx_rel_unique ON relationship (in_node, out_node, relationship_type);
CREATE INDEX idx_rel_reverse ON relationship (out_node, reverse_relationship_type);

CREATE TABLE embedding (
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
CREATE INDEX idx_emb_node      ON embedding (node_id);
CREATE INDEX idx_emb_stale_mod ON embedding (stale, modified_at);
CREATE UNIQUE INDEX idx_emb_unique ON embedding (node_id, model_name, chunk_index);

CREATE TABLE conflict (
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
CREATE INDEX idx_conflict_status ON conflict (status);

CREATE TABLE conflict_participant (
    conflict_id TEXT NOT NULL REFERENCES conflict(id) ON DELETE CASCADE,
    node_id     TEXT NOT NULL,
    PRIMARY KEY (conflict_id, node_id)
) STRICT;
CREATE INDEX idx_conflict_participant_node ON conflict_participant (node_id);

CREATE VIRTUAL TABLE node_title_fts USING fts5(id UNINDEXED, title);

CREATE TRIGGER node_title_fts_insert AFTER INSERT ON node BEGIN
    INSERT INTO node_title_fts(rowid, id, title)
    SELECT new.rowid, new.id, new.title WHERE nullif(new.title, '') IS NOT NULL;
END;

CREATE TRIGGER node_title_fts_update AFTER UPDATE ON node BEGIN
    DELETE FROM node_title_fts WHERE rowid = old.rowid;
    INSERT INTO node_title_fts(rowid, id, title)
    SELECT new.rowid, new.id, new.title WHERE nullif(new.title, '') IS NOT NULL;
END;

CREATE TRIGGER node_title_fts_delete AFTER DELETE ON node BEGIN
    DELETE FROM node_title_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER collection_is_root_edge BEFORE INSERT ON relationship
WHEN new.relationship_type = 'has_child'
  AND (SELECT node_type FROM node WHERE id = new.out_node) = 'collection'
BEGIN
    SELECT RAISE(ABORT, 'collection_not_root: a collection cannot have a parent; collections nest through member_of (ADR-059 §2)');
END;

CREATE TRIGGER collection_is_root_type BEFORE UPDATE OF node_type ON node
WHEN new.node_type = 'collection'
  AND EXISTS (SELECT 1 FROM relationship
              WHERE out_node = new.id AND relationship_type = 'has_child')
BEGIN
    SELECT RAISE(ABORT, 'collection_not_root: a node with a parent cannot become a collection; collections nest through member_of (ADR-059 §2)');
END;

CREATE TRIGGER schema_is_root_edge BEFORE INSERT ON relationship
WHEN new.relationship_type = 'has_child'
  AND (SELECT node_type FROM node WHERE id = new.out_node) = 'schema'
BEGIN
    SELECT RAISE(ABORT, 'schema_not_root: a schema cannot have a parent; schemas are always roots');
END;

CREATE TRIGGER schema_is_root_type BEFORE UPDATE OF node_type ON node
WHEN new.node_type = 'schema'
  AND EXISTS (SELECT 1 FROM relationship
              WHERE out_node = new.id AND relationship_type = 'has_child')
BEGIN
    SELECT RAISE(ABORT, 'schema_not_root: a node with a parent cannot become a schema; schemas are always roots');
END;

CREATE TRIGGER schema_core_status_fixed BEFORE UPDATE OF node_type, properties ON node
WHEN (old.node_type = 'schema' AND coalesce(json_type(old.properties, '$.isCore') = 'true', 0))
  IS NOT (new.node_type = 'schema' AND coalesce(json_type(new.properties, '$.isCore') = 'true', 0))
BEGIN
    SELECT RAISE(ABORT, 'schema_is_core: whether a schema is core is fixed when it is created');
END;

CREATE VIRTUAL TABLE vec_embeddings USING vec0(
    embedding_id TEXT PRIMARY KEY,
    vector FLOAT[768] distance_metric=cosine
);

INSERT INTO node (id, node_type, content, title, created_at, modified_at)
VALUES (
    '3f6c1d2e-8a4b-4c5d-9e7f-0a1b2c3d4e5f',
    'text',
    'Written by v0.3.3',
    'Written by v0.3.3',
    '2026-10-01T00:00:00Z',
    '2026-10-01T00:00:00Z'
);

ANALYZE;
