//! `SqliteStore` methods for the local-only conflict journal (ADR-068).
//! See `db::schema` for the table shape and
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
    pub async fn get_conflict(&self, id: &str) -> Result<Option<ConflictRecord>> {
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
    /// 0. **Tree invariants.** Checked before anything is written; a
    ///    violation refuses the whole merge, the same as `move_node` would.
    ///    The survivor is left with at most one `has_child` parent: its own if
    ///    it has one, otherwise the loser's (see step 2). If it ends up with a
    ///    parent, the merge is refused when it is a `collection`
    ///    (`collection_not_root`) or when either side holds `member_of` and
    ///    the survivor's type may not hold membership under a parent
    ///    (`member_of_not_root`, ADR-059 §2). It is also refused
    ///    (`merge_would_cycle`) when the survivor sits deeper than a direct
    ///    child in the loser's subtree: the loser's children would re-point
    ///    onto their own descendant. A direct parent/child pair merges fine,
    ///    because the edge between them becomes a self-edge.
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
    ///
    ///    `has_child` keeps the tree single-parent. The loser's children
    ///    always re-point and join the survivor's tree. The loser's own parent
    ///    edge re-points only when the survivor is otherwise a root and that
    ///    parent lies outside the survivor's subtree. Otherwise the edge is
    ///    dropped and counted, and the survivor keeps its own position (a
    ///    parent inside its own subtree would close a cycle). `member_of`
    ///    re-points as usual:
    ///    step 0 has already refused any merge where it would land on a node
    ///    with a parent.
    ///
    ///    This step is schema-blind — it re-points every edge unconditionally
    ///    and has no notion of a declared relationship's `cardinality`/
    ///    `reverse_cardinality` (that knowledge lives in `NodeService`'s
    ///    schema resolution, not the store). A re-point can therefore leave
    ///    the survivor with two live edges where a declared relationship
    ///    allows at most one — e.g. survivor and loser each held their own
    ///    compliant `cardinality: One` edge toward different targets before
    ///    the merge. The caller, [`crate::services::NodeService::merge_nodes`],
    ///    closes that gap immediately afterward, in the same transaction, via
    ///    `NodeService::enforce_cardinality_after_merge_in_tx` — using the
    ///    returned `repointed_edges` below to know which edges to re-check.
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
    /// Returns `(properties_merged, superseded, repointed_edges, edges_dropped)`,
    /// where `repointed_edges` is `(relationship_type, new_source_id,
    /// new_target_id)` for every edge this step actually re-pointed (i.e.
    /// `in_node`/`out_node` after the update) — `edges_dropped` counts only
    /// this step's own self-edge, parent-edge and unique-constraint drops,
    /// not anything the caller's cardinality pass may additionally evict.
    pub(crate) async fn merge_nodes_in_tx(
        tx: &Tx<'_>,
        survivor_id: &str,
        loser_id: &str,
    ) -> Result<(u32, Value, Vec<(String, String, String)>, u32)> {
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

        // --- Step 0: tree invariants, checked before anything is written ---
        //
        // Each side's `has_child` parent, ignoring an edge between the two
        // nodes themselves: that edge becomes a self-edge and is dropped.
        let survivor_parent = Self::get_parent_id_in_tx(tx, survivor_id)
            .await?
            .filter(|p| p != loser_id);
        let loser_parent = Self::get_parent_id_in_tx(tx, loser_id)
            .await?
            .filter(|p| p != survivor_id);
        // Re-pointing the loser's children onto the survivor closes a cycle
        // when the survivor sits below one of them.
        if let Some(survivor_parent) = survivor_parent.as_deref() {
            if Self::is_ancestor_in_tx(tx, loser_id, survivor_parent).await? {
                anyhow::bail!(
                    "merge_would_cycle: survivor '{}' sits inside the subtree of '{}', so merging would make the survivor its own ancestor. Move the survivor out of that subtree first.",
                    survivor_id,
                    loser_id
                );
            }
        }
        // The survivor keeps its own position. The loser's parent edge is
        // re-pointed only onto a survivor that would otherwise be a root, and
        // only when that parent is outside the survivor's subtree; taking a
        // parent from its own subtree would close a cycle.
        let takes_loser_parent = match (&survivor_parent, &loser_parent) {
            (None, Some(loser_parent)) => {
                !Self::is_ancestor_in_tx(tx, survivor_id, loser_parent).await?
            }
            _ => false,
        };
        let has_parent_after = survivor_parent.is_some() || takes_loser_parent;

        if has_parent_after {
            if survivor.node_type == "collection" {
                anyhow::bail!(super::collection_not_root(Some(survivor_id)));
            }
            if !super::relationships::member_may_have_parent(&survivor.node_type) {
                let memberships =
                    Self::member_of_targets_in_tx(tx, &[survivor_id, loser_id]).await?;
                if !memberships.is_empty() {
                    anyhow::bail!(
                        "member_of_not_root: merging '{}' into '{}' would leave survivor '{}' holding collection membership ({}) while it has a parent — only root nodes may hold collection membership (ADR-059 §2). Remove the node from the collection(s) first, or move it to the root.",
                        loser_id,
                        survivor_id,
                        survivor_id,
                        memberships.join(", ")
                    );
                }
            }
        }

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

        let mut repointed_edges: Vec<(String, String, String)> = Vec::new();
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

            // The loser's own parent edge, when the survivor does not take it
            // (step 0): the survivor already has a parent, or that parent is
            // inside the survivor's own subtree.
            if edge.relationship_type == "has_child"
                && edge.out_node == loser_id
                && !takes_loser_parent
            {
                conn.execute(
                    "DELETE FROM relationship WHERE id = ?1",
                    libsql::params![edge.id.clone()],
                )
                .await
                .context("Failed to drop the loser's parent edge")?;
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
            repointed_edges.push((
                edge.relationship_type.clone(),
                new_in.to_string(),
                new_out.to_string(),
            ));
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
            repointed_edges,
            edges_dropped,
        ))
    }

    /// The collections any of `member_ids` is filed into (`member_of`
    /// targets), deduplicated. Read through `tx` so the merge checks its own
    /// view of the edges.
    async fn member_of_targets_in_tx(tx: &Tx<'_>, member_ids: &[&str]) -> Result<Vec<String>> {
        let placeholders: Vec<String> = (1..=member_ids.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT DISTINCT out_node FROM relationship \
             WHERE relationship_type = 'member_of' AND in_node IN ({}) ORDER BY out_node",
            placeholders.join(", ")
        );
        let params: Vec<libsql::Value> = member_ids
            .iter()
            .map(|id| libsql::Value::Text(id.to_string()))
            .collect();
        let mut rows = tx
            .conn()
            .query(&sql, params)
            .await
            .context("Failed to read collection memberships")?;
        let mut targets = Vec::new();
        while let Some(row) = rows.next().await? {
            targets.push(row.get::<String>(0)?);
        }
        Ok(targets)
    }

    /// Whether `ancestor_id` is `node_id` or one of its `has_child`
    /// ancestors. Walks up the parent chain, so the cost is the node's depth,
    /// not the size of any subtree.
    async fn is_ancestor_in_tx(tx: &Tx<'_>, ancestor_id: &str, node_id: &str) -> Result<bool> {
        let mut rows = tx
            .conn()
            .query(
                r#"WITH RECURSIVE up(node_id) AS (
                    SELECT ?2
                    UNION
                    SELECT r.in_node FROM relationship r
                    JOIN up u ON r.out_node = u.node_id
                    WHERE r.relationship_type = 'has_child'
                )
                SELECT 1 FROM up WHERE node_id = ?1 LIMIT 1"#,
                libsql::params![ancestor_id.to_string(), node_id.to_string()],
            )
            .await
            .context("Failed to walk the parent chain")?;
        Ok(rows.next().await?.is_some())
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
