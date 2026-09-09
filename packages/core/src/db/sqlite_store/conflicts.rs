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

    /// `_in_tx` twin of [`Self::resolve_conflict`] (ADR-069) — used by
    /// `merge_nodes` to close the conflict record in the same transaction as
    /// the merge's other writes.
    pub(crate) async fn resolve_conflict_in_tx(
        tx: &Tx<'_>,
        conflict_id: &str,
        resolution: &Resolution,
    ) -> Result<()> {
        let status = resolution.terminal_status();
        let resolution_json =
            serde_json::to_string(resolution).context("Failed to serialize conflict resolution")?;
        let now = Utc::now().to_rfc3339();

        let affected = tx
            .conn()
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
            .context("Failed to resolve conflict in tx")?;

        if affected == 0 {
            anyhow::bail!("conflict record '{}' not found", conflict_id);
        }
        Ok(())
    }

    /// Merge `loser` into `survivor` (conflict-journal-and-resolution.md
    /// §5.2), in one transaction:
    ///
    /// 1. **Property union.** Every property present on `loser` but absent on
    ///    `survivor` is copied. Where both hold a value, `survivor` wins and
    ///    the loser's value is captured into the returned `superseded` map —
    ///    merge never silently discards a user value.
    /// 2. **Edge re-pointing.** Every `relationship` row with `in_node` or
    ///    `out_node` = `loser` is re-pointed to `survivor`. A re-point that
    ///    would collide with an existing survivor edge on the
    ///    `(in_node, out_node, relationship_type)` unique constraint is
    ///    dropped, not inserted — counted, never an error. A self-edge
    ///    produced by re-pointing (loser→survivor becoming survivor→survivor)
    ///    is dropped too. `has_child`'s `properties.order` (and every other
    ///    edge property) travels with the re-point for free: this is a plain
    ///    endpoint `UPDATE`, not a delete-and-reinsert.
    /// 3. **Archive the loser** — `lifecycle_status = 'archived'`. NOT a
    ///    literal ADR-068 "deleted" tombstone: this codebase has no such
    ///    lifecycle value (`LIFECYCLE_STATUSES` is `["active", "archived"]`,
    ///    enforced by `validate_lifecycle_status`), and ADR-042's "tombstone"
    ///    turns out to be a cloud/sync-layer concept (`is_deleted`/
    ///    `deleted_at` in the Postgres schema), not a local `lifecycle_status`
    ///    value at all — so introducing a new local state would invent a
    ///    fourth concept where the existing `archived` state already fully
    ///    satisfies the requirement (excluded from every `active`-scoped
    ///    detection query, nothing destroyed, fully reversible). NOT
    ///    `delete_subtree_atomic` — that cascades over `has_child`
    ///    descendants, which step 2 has already re-pointed away; any it
    ///    missed would be destroyed outright.
    /// 4. Caller closes the conflict record (if any) via
    ///    [`Self::resolve_conflict_in_tx`] with a `Resolution::Merge`.
    ///
    /// Returns `(properties_merged, superseded, edges_repointed, edges_dropped)`.
    pub(crate) async fn merge_nodes_in_tx(
        tx: &Tx<'_>,
        survivor_id: &str,
        loser_id: &str,
    ) -> Result<(u32, Value, u32, u32)> {
        let conn = tx.conn();

        let mut survivor_rows = conn
            .query(
                "SELECT * FROM node WHERE id = ?1",
                libsql::params![survivor_id.to_string()],
            )
            .await
            .context("Failed to read survivor node")?;
        let survivor = survivor_rows
            .next()
            .await?
            .ok_or_else(|| anyhow::anyhow!("survivor node '{}' not found", survivor_id))
            .and_then(|row| Self::row_to_node(&row))?;

        let mut loser_rows = conn
            .query(
                "SELECT * FROM node WHERE id = ?1",
                libsql::params![loser_id.to_string()],
            )
            .await
            .context("Failed to read loser node")?;
        let loser = loser_rows
            .next()
            .await?
            .ok_or_else(|| anyhow::anyhow!("loser node '{}' not found", loser_id))
            .and_then(|row| Self::row_to_node(&row))?;

        // --- Step 1: property union, survivor wins ties ---
        //
        // Node properties are namespaced by type (`properties.<node_type>.<field>`,
        // e.g. `properties.person.email` — see `models/core_schemas.rs`), so a
        // union at the top level would only ever compare the single
        // `node_type` key itself (present on both sides whenever the two
        // nodes share a type, which is the case merge exists for) and never
        // reach the actual fields. Union one level deeper, inside the shared
        // `node_type` namespace object, and fall back to a top-level union
        // for any additional non-namespaced keys (e.g. `_seed`).
        let mut merged_properties = survivor.properties.clone();
        let mut superseded = serde_json::Map::new();
        let mut properties_merged: u32 = 0;

        fn union_object(
            merged: &mut serde_json::Map<String, Value>,
            loser: &serde_json::Map<String, Value>,
            superseded: &mut serde_json::Map<String, Value>,
            properties_merged: &mut u32,
        ) {
            for (key, loser_value) in loser {
                match merged.get(key) {
                    None => {
                        merged.insert(key.clone(), loser_value.clone());
                        *properties_merged += 1;
                    }
                    Some(survivor_value) if survivor_value != loser_value => {
                        // Survivor wins; snapshot what it overwrote.
                        superseded.insert(key.clone(), loser_value.clone());
                    }
                    Some(_) => {
                        // Identical value on both sides — nothing to merge or supersede.
                    }
                }
            }
        }

        if let (Some(merged_obj), Some(loser_obj)) = (
            merged_properties.as_object_mut(),
            loser.properties.as_object(),
        ) {
            // The shared type namespace, e.g. `person` — where the real
            // per-field data lives when both nodes are the same type.
            if survivor.node_type == loser.node_type {
                if let Some(loser_ns) = loser_obj.get(&loser.node_type).and_then(|v| v.as_object())
                {
                    let merged_ns = merged_obj
                        .entry(survivor.node_type.clone())
                        .or_insert_with(|| Value::Object(serde_json::Map::new()))
                        .as_object_mut()
                        .expect("just inserted or pre-existing object under node_type key");
                    union_object(merged_ns, loser_ns, &mut superseded, &mut properties_merged);
                }
            }

            // Any remaining top-level keys outside the type namespace
            // (`_seed`, a future non-namespaced bookkeeping key, …).
            for (key, loser_value) in loser_obj {
                if *key == loser.node_type {
                    continue; // already unioned one level deeper, above
                }
                match merged_obj.get(key) {
                    None => {
                        merged_obj.insert(key.clone(), loser_value.clone());
                        properties_merged += 1;
                    }
                    Some(survivor_value) if survivor_value != loser_value => {
                        superseded.insert(key.clone(), loser_value.clone());
                    }
                    Some(_) => {}
                }
            }
        }

        if properties_merged > 0 {
            let props_json = serde_json::to_string(&merged_properties)
                .context("Failed to serialize merged properties")?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE node SET properties = ?1, version = version + 1, modified_at = ?2 WHERE id = ?3",
                libsql::params![props_json, now, survivor_id.to_string()],
            )
            .await
            .context("Failed to write merged properties onto survivor")?;
        }

        // --- Step 2: re-point every edge touching the loser ---
        let mut edge_rows = conn
            .query(
                "SELECT id, in_node, out_node, relationship_type, properties \
                 FROM relationship WHERE in_node = ?1 OR out_node = ?1",
                libsql::params![loser_id.to_string()],
            )
            .await
            .context("Failed to read loser's relationship edges")?;
        let mut edges = Vec::new();
        while let Some(row) = edge_rows.next().await? {
            edges.push(Self::row_to_relationship(&row)?);
        }

        let mut edges_repointed: u32 = 0;
        let mut edges_dropped: u32 = 0;
        let now = Utc::now().to_rfc3339();

        for edge in edges {
            let new_in = if edge.in_node == loser_id {
                survivor_id
            } else {
                edge.in_node.as_str()
            };
            let new_out = if edge.out_node == loser_id {
                survivor_id
            } else {
                edge.out_node.as_str()
            };

            // Re-pointing both endpoints to the survivor produces a self-edge
            // (loser→survivor becoming survivor→survivor) — drop it.
            if new_in == new_out {
                conn.execute(
                    "DELETE FROM relationship WHERE id = ?1",
                    libsql::params![edge.id.clone()],
                )
                .await
                .context("Failed to drop self-edge produced by merge re-point")?;
                edges_dropped += 1;
                continue;
            }

            // Would the re-pointed edge collide with one the survivor
            // already has? `(in_node, out_node, relationship_type)` is
            // unique — drop rather than error, per spec.
            let mut collision_rows = conn
                .query(
                    "SELECT 1 FROM relationship \
                     WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = ?3 AND id != ?4 \
                     LIMIT 1",
                    libsql::params![
                        new_in.to_string(),
                        new_out.to_string(),
                        edge.relationship_type.clone(),
                        edge.id.clone()
                    ],
                )
                .await
                .context("Failed to check for a re-pointed edge collision")?;
            if collision_rows.next().await?.is_some() {
                conn.execute(
                    "DELETE FROM relationship WHERE id = ?1",
                    libsql::params![edge.id.clone()],
                )
                .await
                .context("Failed to drop edge colliding with an existing survivor edge")?;
                edges_dropped += 1;
                continue;
            }

            conn.execute(
                "UPDATE relationship SET in_node = ?1, out_node = ?2, version = version + 1, modified_at = ?3 WHERE id = ?4",
                libsql::params![new_in.to_string(), new_out.to_string(), now.clone(), edge.id.clone()],
            )
            .await
            .context("Failed to re-point relationship edge during merge")?;
            edges_repointed += 1;
        }

        // --- Step 3: archive the loser (see doc comment above for why
        // "archived" and not a literal "deleted" tombstone) ---
        Self::validate_lifecycle_status("archived")?;
        conn.execute(
            "UPDATE node SET lifecycle_status = 'archived', version = version + 1, modified_at = ?1 WHERE id = ?2",
            libsql::params![now, loser_id.to_string()],
        )
        .await
        .context("Failed to archive the merge loser")?;

        Ok((
            properties_merged,
            Value::Object(superseded),
            edges_repointed,
            edges_dropped,
        ))
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
