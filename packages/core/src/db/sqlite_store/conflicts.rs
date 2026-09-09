//! `SqliteStore` methods for the local-only conflict journal (ADR-068).
//! See `db::migrations::v005_conflict_journal` for the table shape and
//! `services::node_service::conflicts` for the deterministic-id helper and
//! the `NodeService`-level detection call sites that use these methods.
use super::*;
use crate::models::conflict::{ConflictKind, ConflictRecord, ConflictStatus, Resolution};
use std::str::FromStr;

impl SqliteStore {
    /// Upsert-by-derived-id: insert a new open record, or — if a record with
    /// this exact `id` already exists and is still `open` — bump its
    /// `occurrences`/`last_seen_at` in place. Never reopens a `resolved` or
    /// `dismissed` record (the `WHERE status = 'open'` guard on the `DO
    /// UPDATE` is what makes a re-detection of an already-decided conflict a
    /// no-op instead of resurrecting it — this is the dismiss-persistence
    /// property `_possible_duplicate` never had).
    ///
    /// `id` must already be the caller's deterministic id (see
    /// `services::node_service::conflicts::deterministic_conflict_id`) — this
    /// method does not compute it, so the same conflict always upserts the
    /// same row regardless of which of its (symmetric) participants is
    /// "new."
    pub async fn record_conflict(
        &self,
        id: &str,
        kind: ConflictKind,
        node_ids: &[String],
        detail: Value,
        detected_by: Option<&str>,
    ) -> Result<ConflictRecord> {
        let mut sorted_ids = node_ids.to_vec();
        sorted_ids.sort();
        let node_ids_json =
            serde_json::to_string(&sorted_ids).context("Failed to serialize conflict node_ids")?;
        let detail_json =
            serde_json::to_string(&detail).context("Failed to serialize conflict detail")?;
        let now = Utc::now().to_rfc3339();

        self.write()
            .await
            .execute(
                "INSERT INTO conflict \
                 (id, kind, node_ids, detail, status, detected_at, detected_by, occurrences, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, 'open', ?5, ?6, 1, ?5) \
                 ON CONFLICT(id) DO UPDATE SET \
                     occurrences = occurrences + 1, \
                     last_seen_at = excluded.last_seen_at \
                 WHERE conflict.status = 'open'",
                libsql::params![
                    id.to_string(),
                    kind.as_str().to_string(),
                    node_ids_json,
                    detail_json,
                    now.clone(),
                    detected_by.map(|s| s.to_string()),
                ],
            )
            .await
            .context("Failed to upsert conflict record")?;

        for node_id in &sorted_ids {
            self.write()
                .await
                .execute(
                    "INSERT OR IGNORE INTO conflict_participant (conflict_id, node_id) VALUES (?1, ?2)",
                    libsql::params![id.to_string(), node_id.clone()],
                )
                .await
                .context("Failed to upsert conflict_participant row")?;
        }

        self.get_conflict(id).await?.ok_or_else(|| {
            anyhow::anyhow!("conflict record '{}' missing immediately after upsert", id)
        })
    }

    /// Read a single conflict record by id, if it exists.
    async fn get_conflict(&self, id: &str) -> Result<Option<ConflictRecord>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT id, kind, node_ids, detail, status, detected_at, detected_by, \
                 occurrences, last_seen_at, resolved_at, resolution \
                 FROM conflict WHERE id = ?1",
                libsql::params![id.to_string()],
            )
            .await
            .context("Failed to query conflict by id")?;

        match rows.next().await? {
            Some(row) => Ok(Some(row_to_conflict_record(&row)?)),
            None => Ok(None),
        }
    }

    /// List conflict records, optionally filtered by status and/or kind, most
    /// recently detected first. `limit` caps the result count (no limit when
    /// `None`).
    pub async fn list_conflicts(
        &self,
        status: Option<ConflictStatus>,
        kind: Option<ConflictKind>,
        limit: Option<u32>,
    ) -> Result<Vec<ConflictRecord>> {
        let mut sql = String::from(
            "SELECT id, kind, node_ids, detail, status, detected_at, detected_by, \
             occurrences, last_seen_at, resolved_at, resolution FROM conflict WHERE 1=1",
        );
        let mut params: Vec<libsql::Value> = Vec::new();

        if let Some(status) = status {
            params.push(libsql::Value::Text(status.as_str().to_string()));
            sql.push_str(&format!(" AND status = ?{}", params.len()));
        }
        if let Some(kind) = kind {
            params.push(libsql::Value::Text(kind.as_str().to_string()));
            sql.push_str(&format!(" AND kind = ?{}", params.len()));
        }
        sql.push_str(" ORDER BY detected_at DESC");
        if let Some(limit) = limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        let mut rows = self
            .read()
            .await?
            .query(&sql, params)
            .await
            .context("Failed to list conflicts")?;

        let mut records = Vec::new();
        while let Some(row) = rows.next().await? {
            records.push(row_to_conflict_record(&row)?);
        }
        Ok(records)
    }

    /// Every conflict record naming `node_id` as a participant, most recently
    /// detected first. Backed by `idx_conflict_participant_node` — an index
    /// hit, not a scan over `conflict.node_ids`' JSON, since this runs on
    /// every node render for the inline "is this node in an open conflict"
    /// indicator (conflict-journal-and-resolution.md §2.1).
    pub async fn conflicts_for_node(&self, node_id: &str) -> Result<Vec<ConflictRecord>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT c.id, c.kind, c.node_ids, c.detail, c.status, c.detected_at, \
                 c.detected_by, c.occurrences, c.last_seen_at, c.resolved_at, c.resolution \
                 FROM conflict c \
                 JOIN conflict_participant p ON p.conflict_id = c.id \
                 WHERE p.node_id = ?1 \
                 ORDER BY c.detected_at DESC",
                libsql::params![node_id.to_string()],
            )
            .await
            .context("Failed to query conflicts_for_node")?;

        let mut records = Vec::new();
        while let Some(row) = rows.next().await? {
            records.push(row_to_conflict_record(&row)?);
        }
        Ok(records)
    }

    /// Apply a resolution to a conflict record: sets `status` to the
    /// resolution's terminal status (`Resolution::terminal_status`),
    /// `resolved_at` to now, and `resolution` to the resolution's JSON form.
    pub async fn resolve_conflict(
        &self,
        conflict_id: &str,
        resolution: Resolution,
    ) -> Result<ConflictRecord> {
        let status = resolution.terminal_status();
        let resolution_json = serde_json::to_string(&resolution)
            .context("Failed to serialize conflict resolution")?;
        let now = Utc::now().to_rfc3339();

        let affected = self
            .write()
            .await
            .execute(
                "UPDATE conflict SET status = ?1, resolved_at = ?2, resolution = ?3 WHERE id = ?4",
                libsql::params![
                    status.as_str().to_string(),
                    now,
                    resolution_json,
                    conflict_id.to_string(),
                ],
            )
            .await
            .context("Failed to resolve conflict")?;

        if affected == 0 {
            anyhow::bail!("conflict record '{}' not found", conflict_id);
        }

        self.get_conflict(conflict_id).await?.ok_or_else(|| {
            anyhow::anyhow!("conflict record '{}' missing after resolve", conflict_id)
        })
    }
}

fn row_to_conflict_record(row: &libsql::Row) -> Result<ConflictRecord> {
    let id: String = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let node_ids_json: String = row.get(2)?;
    let detail_json: String = row.get(3)?;
    let status_str: String = row.get(4)?;
    let detected_at: String = row.get(5)?;
    let detected_by: Option<String> = row.get(6)?;
    let occurrences: i64 = row.get(7)?;
    let last_seen_at: String = row.get(8)?;
    let resolved_at: Option<String> = row.get(9)?;
    let resolution_json: Option<String> = row.get(10)?;

    Ok(ConflictRecord {
        id,
        kind: ConflictKind::from_str(&kind_str)
            .map_err(|e| anyhow::anyhow!("corrupt conflict.kind: {}", e))?,
        node_ids: serde_json::from_str(&node_ids_json)
            .context("Failed to deserialize conflict.node_ids")?,
        detail: serde_json::from_str(&detail_json)
            .context("Failed to deserialize conflict.detail")?,
        status: ConflictStatus::from_str(&status_str)
            .map_err(|e| anyhow::anyhow!("corrupt conflict.status: {}", e))?,
        detected_at,
        detected_by,
        occurrences,
        last_seen_at,
        resolved_at,
        resolution: resolution_json
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .context("Failed to deserialize conflict.resolution")?,
    })
}
