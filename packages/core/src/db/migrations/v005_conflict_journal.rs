//! Local-only conflict journal (ADR-068): a durable, resolvable record of a
//! convergence conflict, replacing the `_possible_duplicate` boolean property.
//!
//! `conflict` holds one row per detected conflict, keyed by a deterministic id
//! (see `services::node_service::conflicts::deterministic_conflict_id`) so
//! re-detection updates `occurrences`/`last_seen_at` on the same row instead of
//! appending a duplicate. `conflict_participant` is a companion table so "is
//! this node in an open conflict" is an index hit on `node_id`, not a scan
//! over the `node_ids` JSON array in `conflict`.
//!
//! No FK from `conflict_participant.node_id` to `node(id)`: a participant may
//! be hard-deleted, and the record of the conflict must survive that (a
//! reconciliation sweep closes the record later — see the conflict-journal
//! spec §5.4 — this table must not lose the row to a cascade in the meantime).
//! `conflict_id` DOES cascade — deleting a conflict record (this build has no
//! such path today, but the schema should not forbid one) should take its
//! participant rows with it.

use anyhow::{Context, Result};

const CONFLICT_JOURNAL_SQL: &str = r#"
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

CREATE TABLE IF NOT EXISTS conflict_participant (
    conflict_id TEXT NOT NULL REFERENCES conflict(id) ON DELETE CASCADE,
    node_id     TEXT NOT NULL,
    PRIMARY KEY (conflict_id, node_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_conflict_participant_node ON conflict_participant (node_id);
"#;

pub async fn apply(tx: &libsql::Transaction) -> Result<()> {
    for stmt in CONFLICT_JOURNAL_SQL.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        tx.execute(stmt, ())
            .await
            .with_context(|| format!("Failed to execute DDL: {}", &stmt[..stmt.len().min(80)]))?;
    }

    Ok(())
}
