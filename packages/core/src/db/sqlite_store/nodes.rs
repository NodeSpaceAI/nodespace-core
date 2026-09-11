//! `SqliteStore` methods — nodes concern (split from the god-object per ADR-053 prep).
use super::*;

impl SqliteStore {
    pub async fn create_node(
        &self,
        node: Node,
        source: Option<String>,
        playbook_context: Option<crate::db::events::PlaybookExecutionContext>,
    ) -> Result<Node> {
        // Collection-name collisions are suggest-don't-block (ADR-065 posture),
        // never a hard rejection: NodeSpace is local-first, and two offline
        // devices can each validly create a collection with the same name. A
        // hard rejection here used to propagate straight out of the sync-apply
        // `create_node` call (`nodespace-sync`'s `apply_node_upsert`), and per
        // that function's own contract the cursor never advances past a failed
        // row — a benign duplicate collection name could wedge a sync cursor
        // permanently. Detect the collision (if any) BEFORE the insert below so
        // it unambiguously identifies the pre-existing OTHER node, then let the
        // write proceed unconditionally and mark both sides afterward.
        let colliding_collection = if node.node_type == "collection" {
            self.get_collection_by_name(&node.content).await?
        } else {
            None
        };

        Self::validate_lifecycle_status(&node.lifecycle_status)?;

        let properties = if node.properties.is_null() {
            serde_json::json!({})
        } else {
            node.properties.clone()
        };
        let props_json =
            serde_json::to_string(&properties).context("Failed to serialize properties")?;
        let now = Utc::now().to_rfc3339();

        self.write()
            .await
            .execute(
                "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                libsql::params![
                    node.id.clone(),
                    node.node_type.clone(),
                    node.content.clone(),
                    props_json,
                    node.title.clone(),
                    node.lifecycle_status.clone(),
                    node.version,
                    now.clone(),
                    now,
                ],
            )
            .await
            .context("Failed to create node")?;

        self.notify(StoreChange {
            operation: StoreOperation::Created,
            node: node.clone(),
            source,
            previous_node: None,
            playbook_context,
        });

        if let Some(existing) = colliding_collection {
            // Best-effort and non-blocking: the node above is already durably
            // written. A marker-write failure must never fail node creation —
            // mirrors the swallow-errors posture of nodespace-sync's own
            // `mark_possible_duplicate` (ADR-065).
            self.mark_collection_name_collision(&node.id, &existing.id)
                .await;
        }

        Ok(node)
    }

    /// Journal a `CollectionNameCollision` conflict record (ADR-068) naming
    /// both sides of a collection-name collision: `new_id` (the node that was
    /// just created/renamed) and `existing_id` (the pre-existing collection
    /// it collides with by name). This is its own `ConflictKind`, distinct
    /// from `UniqueFieldCollision` — it is enforced by a different predicate
    /// (`get_collection_by_name`, not `find_conflicting_unique`), is not
    /// schema-declared, and resolves differently (renaming one collection is
    /// a valid resolution with no analogue for a duplicate person). Errors
    /// are logged and swallowed: the write this follows has already
    /// succeeded and must not be undone by a journal-write failure.
    ///
    /// Called from `create_node` for a collision detected on insert, and also
    /// from `update_node` / `update_node_with_version_check` for a collision
    /// detected on a `content` change that leaves the node (effectively)
    /// collection-typed — i.e. a rename. All three callers detect BEFORE
    /// their write and journal AFTER it, and all three exclude the node's own
    /// id from the pre-write match so a no-op or case-only rename onto itself
    /// never self-journals.
    ///
    /// The collision detection this backs (`get_collection_by_name`, called
    /// by each caller before its write) has a real TOCTOU gap under genuine
    /// concurrent writes that would land on the same name: two concurrent
    /// calls can each run their pre-write lookup before either write lands,
    /// so both see "no collision" and neither gets journaled. This is
    /// accepted as best-effort, same as `UniqueFieldCollision` detection
    /// (conflict-journal-and-resolution.md §4) — a periodic reconciliation
    /// sweep is the backstop, not a tighter lock.
    ///
    /// `get_collection_by_name` itself only matches `lifecycle_status =
    /// 'active'` collections, so a collision against an archived collection
    /// is never detected or journaled by any caller — archiving a collection
    /// frees up its name.
    async fn mark_collection_name_collision(&self, new_id: &str, existing_id: &str) {
        use crate::models::conflict::ConflictKind;

        let mut node_ids = vec![new_id.to_string(), existing_id.to_string()];
        node_ids.sort();

        // Discriminator is the case-folded name (conflict-journal-and-
        // resolution.md §2.3) — the same fold `get_collection_by_name` and
        // `normalize_collection_name` apply, so a collision detected via
        // either the create or the rename path always derives the same id.
        let name = match self.get_node(new_id).await {
            Ok(Some(node)) => node.content.trim().to_lowercase(),
            _ => {
                // The node we just wrote is unreadable immediately after —
                // should not happen, but journaling nothing is safer than
                // guessing a discriminator that could collide with an
                // unrelated name's conflict id.
                tracing::warn!(
                    node_id = %new_id,
                    "failed to re-read node for collection-name-collision discriminator; skipping journal write"
                );
                return;
            }
        };

        let id = crate::services::node_service::deterministic_conflict_id(
            ConflictKind::CollectionNameCollision,
            &node_ids,
            &name,
        );
        let detail = serde_json::json!({ "name": name });

        if let Err(e) = self
            .record_conflict(
                &id,
                ConflictKind::CollectionNameCollision,
                &node_ids,
                detail,
                None,
            )
            .await
        {
            tracing::warn!(
                new_id = %new_id,
                existing_id = %existing_id,
                error = %e,
                "failed to journal collection-name-collision conflict (triggering write unaffected)"
            );
        }
    }

    pub async fn create_child_node_atomic(
        &self,
        parent_id: &str,
        node_type: &str,
        content: &str,
        properties: Value,
        source: Option<String>,
    ) -> Result<Node> {
        self.validate_node_type(node_type)?;

        let node_id = uuid::Uuid::new_v4().to_string();

        let parent_exists = self.get_node(parent_id).await?;
        if parent_exists.is_none() {
            return Err(anyhow::anyhow!("Parent node not found: {}", parent_id));
        }

        self.validate_no_cycle(parent_id, &node_id).await?;

        // Held across the sibling-order read → compute → insert. Without it a
        // concurrent create/move under the same parent reads the same max order
        // and assigns a colliding key. No re-entrancy: nothing below takes the
        // guard again.
        let db = self.write().await;

        // Get last child order
        let mut rows = db.query(
            "SELECT json_extract(r.properties, '$.order') as ord FROM relationship r WHERE r.in_node = ?1 AND r.relationship_type = 'has_child' ORDER BY json_extract(r.properties, '$.order') DESC LIMIT 1",
            libsql::params![parent_id.to_string()],
        ).await.context("Failed to get last child order")?;

        // distinguish "no siblings" from "a sibling at order <= 0".
        // Fractional ordering legitimately yields orders <= 0 (a prepend gives
        // 0.0, then -1.0…), and `unwrap_or(0.0)` maps a NULL order to 0.0. The old
        // `last_order > 0.0` sentinel misread a lone child at 0.0 as "no children",
        // so the next append also got 1.0 → two siblings collided at 1.0. Track
        // presence as Option and append after the max regardless of sign.
        let last_order: Option<f64> = if let Some(row) = rows.next().await? {
            Some(row.get::<Option<f64>>(0)?.unwrap_or(0.0))
        } else {
            None
        };

        let new_order = FractionalOrderCalculator::calculate_order(last_order, None);

        let properties = if properties.is_null() {
            serde_json::json!({})
        } else {
            properties
        };
        let props_json =
            serde_json::to_string(&properties).context("Failed to serialize properties")?;
        let now = Utc::now().to_rfc3339();

        let tx = db
            .transaction()
            .await
            .context("Failed to begin transaction")?;

        tx.execute(
            "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, NULL, 'active', 1, ?5, ?6)",
            libsql::params![node_id.clone(), node_type.to_string(), content.to_string(), props_json, now.clone(), now.clone()],
        ).await.context("Failed to insert child node")?;

        let rel_id = uuid::Uuid::new_v4().to_string();
        let rel_props = serde_json::json!({"order": new_order}).to_string();
        tx.execute(
            "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'has_child', ?4, 1, ?5, ?6)",
            libsql::params![rel_id, parent_id.to_string(), node_id.clone(), rel_props, now.clone(), now],
        ).await.context("Failed to insert parent-child relationship")?;

        tx.commit().await.context("Failed to commit transaction")?;
        drop(db);

        let node = self
            .get_node(&node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after creation: {}", node_id))?;

        self.notify(StoreChange {
            operation: StoreOperation::Created,
            node: node.clone(),
            source,
            previous_node: None,
            playbook_context: None,
        });

        Ok(node)
    }

    /// `_in_tx` twin of [`Self::create_node`] (ADR-069 §1a). Inserts the node
    /// row against `tx` and returns the inserted `Node` directly from its
    /// inputs, rather than reading it back — a read-back through `self.read()`
    /// would use a pooled connection that cannot see this transaction's
    /// uncommitted insert.
    ///
    /// Does **not** call `self.notify` — an event fired here would announce a
    /// write that may still roll back. The caller buffers the equivalent
    /// `StoreChange`/`DomainEvent` and emits it only after the transaction
    /// housing this call commits (ADR-069 §2). Also skips the
    /// collection-name-collision marker `create_node` writes post-insert:
    /// that marker uses `set_property_bool`, which is deliberately excluded
    /// from the transaction boundary (ADR-069 §4) — a caller composing this
    /// method is responsible for calling `mark_collection_name_collision`'s
    /// equivalent itself, after commit, if it needs that behavior.
    pub(crate) async fn create_node_in_tx(tx: &Tx<'_>, node: &Node) -> Result<()> {
        Self::validate_lifecycle_status(&node.lifecycle_status)?;

        let properties = if node.properties.is_null() {
            serde_json::json!({})
        } else {
            node.properties.clone()
        };
        let props_json =
            serde_json::to_string(&properties).context("Failed to serialize properties")?;
        let now = Utc::now().to_rfc3339();

        tx.conn()
            .execute(
                "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                libsql::params![
                    node.id.clone(),
                    node.node_type.clone(),
                    node.content.clone(),
                    props_json,
                    node.title.clone(),
                    node.lifecycle_status.clone(),
                    node.version,
                    now.clone(),
                    now,
                ],
            )
            .await
            .context("Failed to create node")?;

        Ok(())
    }

    /// `_in_tx` twin of the has_child edge insert inside
    /// [`Self::create_child_node_atomic`] (ADR-069 §1a/S2), generalized to
    /// accept an insert position the way the store's `move_node` does for
    /// its new-parent branch. Assumes `child_id` has no existing `has_child`
    /// edge (true for every caller — a brand-new node) so, unlike
    /// `move_node`, there is nothing to DELETE first: this is an insert-only
    /// analogue of `move_node`'s cross-parent branch.
    ///
    /// `insert_after_sibling_id`: `None` inserts first; `Some(id)` inserts
    /// immediately after that sibling, falling back to last if `id` is not a
    /// current child of `parent_id` (mirrors `move_node`'s same fallback).
    pub(crate) async fn create_has_child_edge_in_tx(
        tx: &Tx<'_>,
        parent_id: &str,
        child_id: &str,
        insert_after_sibling_id: Option<&str>,
    ) -> Result<f64> {
        let mut rows = tx.conn().query(
            "SELECT out_node, json_extract(properties, '$.order') as ord FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child' ORDER BY json_extract(properties, '$.order') ASC",
            libsql::params![parent_id.to_string()],
        ).await.context("Failed to get sibling relationships")?;

        let mut siblings: Vec<(String, f64)> = Vec::new();
        while let Some(row) = rows.next().await? {
            let sibling_id: String = row.get(0)?;
            let ord: Option<f64> = row.get(1)?;
            siblings.push((sibling_id, ord.unwrap_or(0.0)));
        }

        let new_order = if let Some(after_id) = insert_after_sibling_id {
            if let Some(after_index) = siblings.iter().position(|(id, _)| id == after_id) {
                let prev_order = siblings[after_index].1;
                let next_order = siblings.get(after_index + 1).map(|(_, o)| *o);
                FractionalOrderCalculator::calculate_order(Some(prev_order), next_order)
            } else {
                let last = siblings.last().map(|(_, o)| *o);
                FractionalOrderCalculator::calculate_order(last, None)
            }
        } else {
            let first = siblings.first().map(|(_, o)| *o);
            FractionalOrderCalculator::calculate_order(None, first)
        };

        let rel_id = uuid::Uuid::new_v4().to_string();
        let rel_props = serde_json::json!({"order": new_order}).to_string();
        let now = Utc::now().to_rfc3339();
        tx.conn()
            .execute(
                "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'has_child', ?4, 1, ?5, ?6)",
                libsql::params![rel_id, parent_id.to_string(), child_id.to_string(), rel_props, now.clone(), now],
            )
            .await
            .context("Failed to insert parent-child relationship")?;

        Ok(new_order)
    }

    pub async fn get_node(&self, id: &str) -> Result<Option<Node>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT * FROM node WHERE id = ?1 LIMIT 1",
                libsql::params![id.to_string()],
            )
            .await
            .context("Failed to query node")?;

        if let Some(row) = rows.next().await? {
            Ok(Some(Self::row_to_node(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn node_exists(&self, id: &str) -> Result<bool> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT 1 FROM node WHERE id = ?1 LIMIT 1",
                libsql::params![id.to_string()],
            )
            .await
            .context("Failed to check node existence")?;
        Ok(rows.next().await?.is_some())
    }

    pub async fn get_nodes_by_ids(&self, ids: &[String]) -> Result<HashMap<String, Node>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        // Chunk under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766); a large
        // `IN (...)` would otherwise fail outright (a directory import can list
        // thousands of files). Mirrors the chunking in the other bulk store queries.
        const ID_CHUNK: usize = 900;
        let mut result = HashMap::new();
        for chunk in ids.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "SELECT * FROM node WHERE id IN ({})",
                placeholders.join(", ")
            );

            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            let mut rows = self
                .read()
                .await?
                .query(&sql, params)
                .await
                .context("Failed to batch query nodes")?;

            while let Some(row) = rows.next().await? {
                let node = Self::row_to_node(&row)?;
                result.insert(node.id.clone(), node);
            }
        }
        Ok(result)
    }

    pub async fn update_node(
        &self,
        id: &str,
        update: NodeUpdate,
        source: Option<String>,
    ) -> Result<Node> {
        if let Some(ref status) = update.lifecycle_status {
            Self::validate_lifecycle_status(status)?;
        }

        let current = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", id))?;

        let updated_content = update.content.unwrap_or_else(|| current.content.clone());
        let updated_node_type = update.node_type.unwrap_or(current.node_type.clone());

        // Rename-path collision detection, mirroring create_node's suggest-
        // don't-block posture (see mark_collection_name_collision's doc
        // comment): a sync-applied update that changes a collection's
        // `content` to a name already used by a DIFFERENT local collection is
        // detected here BEFORE the write, then both sides are marked after it
        // succeeds. Only triggered when content is actually changing on an
        // (effectively) collection-typed node — a no-op rename (including a
        // case-only one, which would otherwise match itself) or a
        // non-content update never re-checks or re-marks.
        //
        // Self-exclusion is a post-query `.filter`, not a SQL-level `id !=`
        // (unlike `find_conflicting_unique`'s `exclude_id`), because
        // `get_collection_by_name` has no exclude param — widening its
        // signature is out of scope here (it has several other callers; see
        // its own doc comment). This is provably safe despite the unordered
        // `LIMIT 1`: a self-match can only occur when this row's OLD title,
        // lowercased, already equals the NEW content — which means any THIRD
        // row also matching the new content necessarily already matched the
        // OLD title too, so a genuine collision was already detectable (and
        // would already have been marked) before this rename, not newly
        // hidden by filtering out the self-match here.
        let colliding_collection =
            if updated_node_type == "collection" && updated_content != current.content {
                self.get_collection_by_name(&updated_content)
                    .await?
                    .filter(|existing| existing.id != id)
            } else {
                None
            };

        let properties_update = if let Some(ref updated_props) = update.properties {
            let mut merged = current.properties.as_object().cloned().unwrap_or_default();
            if let Some(new_props) = updated_props.as_object() {
                for (key, value) in new_props {
                    merged.insert(key.clone(), value.clone());
                }
            }
            Some(serde_json::Value::Object(merged))
        } else {
            None
        };

        let now = Utc::now().to_rfc3339();

        // One guard across the whole update, AND every statement below runs
        // inside one SQL transaction (not just under the guard): the guard alone
        // only serializes this store's own callers against each other — it does
        // nothing for a concurrent READER, which runs on a separate pooled
        // connection (see `read()`) that isn't gated by this mutex at all. Absent
        // a real transaction, each `execute` below autocommits the instant it
        // runs, so a reader landing between them (or a crash / driver error
        // landing between them) would see — or durably persist — content and
        // properties already updated but title or lifecycle_status still stale.
        // Wrapping them in `db.transaction()` + `commit()` makes the whole
        // update atomic and invisible to readers until it lands as one unit; an
        // error on any statement drops `tx` without committing, which rolls the
        // whole thing back rather than leaving a half-updated row.
        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin update_node transaction")?;

        if let Some(ref props) = properties_update {
            let props_json =
                serde_json::to_string(props).context("Failed to serialize properties")?;
            tx.execute(
                "UPDATE node SET content = ?1, node_type = ?2, properties = ?3 WHERE id = ?4",
                libsql::params![
                    updated_content.clone(),
                    updated_node_type.clone(),
                    props_json,
                    id.to_string()
                ],
            )
            .await
            .context("Failed to update node")?;
        } else {
            tx.execute(
                "UPDATE node SET content = ?1, node_type = ?2 WHERE id = ?3",
                libsql::params![
                    updated_content.clone(),
                    updated_node_type.clone(),
                    id.to_string()
                ],
            )
            .await
            .context("Failed to update node")?;
        }

        if let Some(title) = update.title {
            tx.execute(
                "UPDATE node SET title = ?1 WHERE id = ?2",
                libsql::params![title, id.to_string()],
            )
            .await
            .context("Failed to update title")?;
        }

        if let Some(status) = update.lifecycle_status {
            tx.execute(
                "UPDATE node SET lifecycle_status = ?1 WHERE id = ?2",
                libsql::params![status, id.to_string()],
            )
            .await
            .context("Failed to update lifecycle_status")?;
        }

        // Exactly one version/modified_at bump per call, in its own statement
        // decoupled from which of the field groups above actually ran — so a
        // title-only or lifecycle_status-only update bumps OCC state the same
        // way a content/properties update does, instead of that guarantee
        // depending on the content statement happening to always execute.
        tx.execute(
            "UPDATE node SET version = version + 1, modified_at = ?1 WHERE id = ?2",
            libsql::params![now.clone(), id.to_string()],
        )
        .await
        .context("Failed to bump node version")?;

        tx.commit()
            .await
            .context("Failed to commit update_node transaction")?;

        // Released before the read-back and before `mark_collection_name_
        // collision`, which takes the guard itself (it is not re-entrant).
        drop(db);

        let updated_node = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after update"))?;

        self.notify(StoreChange {
            operation: StoreOperation::Updated,
            node: updated_node.clone(),
            source,
            previous_node: None,
            playbook_context: None,
        });

        if let Some(existing) = colliding_collection {
            // Best-effort and non-blocking, same posture as create_node's call
            // site: the update above is already durably written.
            self.mark_collection_name_collision(id, &existing.id).await;
        }

        Ok(updated_node)
    }

    /// `_in_tx` twin of [`Self::update_node`] (ADR-069 §1a/S3). No OCC — same
    /// contract as the non-`_in_tx` method. Run against the caller's `tx`
    /// instead of opening its own, so `update_node_unchecked_in_tx` (the
    /// `NodeService`-level caller) can land this write in the same
    /// transaction as `rename_schema_field_in_tx`'s data migration (closing
    /// F3: a failure in either step now rolls back both). Does not notify or
    /// mark collection-name collisions; the caller buffers the equivalent
    /// event and handles the marker post-commit.
    pub(crate) async fn update_node_in_tx(tx: &Tx<'_>, id: &str, update: NodeUpdate) -> Result<()> {
        if let Some(ref status) = update.lifecycle_status {
            Self::validate_lifecycle_status(status)?;
        }

        let mut rows = tx
            .conn()
            .query(
                "SELECT * FROM node WHERE id = ?1 LIMIT 1",
                libsql::params![id.to_string()],
            )
            .await
            .context("Failed to query node")?;
        let current = match rows.next().await? {
            Some(row) => Self::row_to_node(&row)?,
            None => return Err(anyhow::anyhow!("Node not found: {}", id)),
        };

        let updated_content = update.content.unwrap_or_else(|| current.content.clone());
        let updated_node_type = update.node_type.unwrap_or(current.node_type.clone());

        let properties_update = if let Some(ref updated_props) = update.properties {
            let mut merged = current.properties.as_object().cloned().unwrap_or_default();
            if let Some(new_props) = updated_props.as_object() {
                for (key, value) in new_props {
                    merged.insert(key.clone(), value.clone());
                }
            }
            Some(serde_json::Value::Object(merged))
        } else {
            None
        };

        let now = Utc::now().to_rfc3339();

        if let Some(ref props) = properties_update {
            let props_json =
                serde_json::to_string(props).context("Failed to serialize properties")?;
            tx.conn()
                .execute(
                    "UPDATE node SET content = ?1, node_type = ?2, properties = ?3 WHERE id = ?4",
                    libsql::params![
                        updated_content.clone(),
                        updated_node_type.clone(),
                        props_json,
                        id.to_string()
                    ],
                )
                .await
                .context("Failed to update node")?;
        } else {
            tx.conn()
                .execute(
                    "UPDATE node SET content = ?1, node_type = ?2 WHERE id = ?3",
                    libsql::params![
                        updated_content.clone(),
                        updated_node_type.clone(),
                        id.to_string()
                    ],
                )
                .await
                .context("Failed to update node")?;
        }

        if let Some(title) = update.title {
            tx.conn()
                .execute(
                    "UPDATE node SET title = ?1 WHERE id = ?2",
                    libsql::params![title, id.to_string()],
                )
                .await
                .context("Failed to update title")?;
        }

        if let Some(status) = update.lifecycle_status {
            tx.conn()
                .execute(
                    "UPDATE node SET lifecycle_status = ?1 WHERE id = ?2",
                    libsql::params![status, id.to_string()],
                )
                .await
                .context("Failed to update lifecycle_status")?;
        }

        tx.conn()
            .execute(
                "UPDATE node SET version = version + 1, modified_at = ?1 WHERE id = ?2",
                libsql::params![now, id.to_string()],
            )
            .await
            .context("Failed to bump node version")?;

        Ok(())
    }

    pub async fn switch_node_type_atomic(
        &self,
        node_id: &str,
        new_type: &str,
        new_properties: Value,
        source: Option<String>,
    ) -> Result<Node> {
        self.validate_node_type(new_type)?;

        let new_properties = if new_properties.is_null() {
            serde_json::json!({})
        } else {
            new_properties
        };
        let props_json =
            serde_json::to_string(&new_properties).context("Failed to serialize properties")?;
        let now = Utc::now().to_rfc3339();

        self.write()
            .await
            .execute(
                "UPDATE node SET node_type = ?1, properties = ?2, version = version + 1, modified_at = ?3 WHERE id = ?4",
                libsql::params![new_type.to_string(), props_json, now, node_id.to_string()],
            )
            .await
            .context("Failed to switch node type")?;

        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after type switch: {}", node_id))?;

        self.notify(StoreChange {
            operation: StoreOperation::Updated,
            node: node.clone(),
            source,
            previous_node: None,
            playbook_context: None,
        });

        Ok(node)
    }

    /// Version of the REAL persisted `node` row, or `None` if no such row exists.
    /// Unlike `get_node`, this does NOT virtualize a date-page node — so a
    /// concurrently-deleted date page reports `None`, not a phantom version 1.
    /// Used to disambiguate a no-op version-checked update.
    pub async fn persisted_version(&self, id: &str) -> Result<Option<i64>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT version FROM node WHERE id = ?1",
                libsql::params![id.to_string()],
            )
            .await
            .context("Failed to read persisted node version")?;
        Ok(match rows.next().await? {
            Some(row) => Some(row.get::<i64>(0)?),
            None => None,
        })
    }

    pub async fn update_node_with_version_check(
        &self,
        id: &str,
        expected_version: i64,
        update: NodeUpdate,
        source: Option<String>,
        playbook_context: Option<crate::db::events::PlaybookExecutionContext>,
    ) -> Result<Option<Node>> {
        if let Some(ref status) = update.lifecycle_status {
            Self::validate_lifecycle_status(status)?;
        }

        let current = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", id))?;

        let previous_node = current.clone();
        let new_version = expected_version + 1;

        let updated_content = update.content.unwrap_or(current.content);
        let updated_node_type = update.node_type.unwrap_or(current.node_type.clone());

        // Rename-path collision detection — see the equivalent block in
        // `update_node` and `mark_collection_name_collision`'s doc comment.
        // Compares against `previous_node` (cloned above, before `current`
        // was partially moved from) rather than `current` directly.
        let colliding_collection =
            if updated_node_type == "collection" && updated_content != previous_node.content {
                self.get_collection_by_name(&updated_content)
                    .await?
                    .filter(|existing| existing.id != id)
            } else {
                None
            };

        let updated_props = match update.properties {
            Some(p) => serde_json::to_string(&p).context("Failed to serialize properties")?,
            None => serde_json::to_string(&current.properties)
                .context("Failed to serialize current properties")?,
        };
        let updated_title = match update.title {
            Some(t) => t,          // Some(Some(x)) sets, Some(None) clears
            None => current.title, // None means no change
        };
        let updated_status = update
            .lifecycle_status
            .unwrap_or(current.lifecycle_status.clone());
        let now = Utc::now().to_rfc3339();

        let rows_affected = self.write().await.execute(
            "UPDATE node SET content = ?1, node_type = ?2, properties = ?3, title = ?4, lifecycle_status = ?5, version = ?6, modified_at = ?7 WHERE id = ?8 AND version = ?9",
            libsql::params![updated_content, updated_node_type, updated_props, updated_title, updated_status, new_version, now, id.to_string(), expected_version],
        ).await.context("Failed to update node with version check")?;

        if rows_affected == 0 {
            return Ok(None);
        }

        let node = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after update"))?;

        self.notify(StoreChange {
            operation: StoreOperation::Updated,
            node: node.clone(),
            source,
            previous_node: Some(previous_node),
            playbook_context,
        });

        if let Some(existing) = colliding_collection {
            // Best-effort and non-blocking, same posture as create_node's call
            // site. Only reached when rows_affected != 0 above, i.e. the
            // version check actually succeeded and the write landed.
            self.mark_collection_name_collision(id, &existing.id).await;
        }

        Ok(Some(node))
    }

    /// `_in_tx` twin of [`Self::update_node_with_version_check`] (ADR-069
    /// §1a/§3). The pre-read and the version-gated `UPDATE` both run against
    /// `tx`, so the OCC check is sound against this transaction's own view —
    /// no TOCTOU window between reading `current` and writing, since nothing
    /// else can observe or mutate this row mid-transaction.
    ///
    /// Returns `Ok(None)` on a version mismatch (rows_affected == 0), exactly
    /// like the non-`_in_tx` method — this is a normal, expected outcome the
    /// caller surfaces as `NodeServiceError::VersionConflict`, never as
    /// `TransactionFailed` (ADR-069 §2a). Does not notify or mark
    /// collection-name collisions; the caller buffers the equivalent event
    /// and handles the marker post-commit, same posture as
    /// `create_node_in_tx`.
    /// `Ok(Ok(node))` on success, `Ok(Err(actual_version))` on an OCC
    /// mismatch — the real persisted version at the time of the check, not
    /// a guess, so a caller building `NodeServiceError::VersionConflict` can
    /// report a genuine value instead of fabricating one (e.g.
    /// `expected_version + 1`, which is wrong the instant more than one
    /// concurrent writer has landed since `expected_version`).
    pub(crate) async fn update_node_with_version_check_in_tx(
        tx: &Tx<'_>,
        id: &str,
        expected_version: i64,
        update: NodeUpdate,
    ) -> Result<std::result::Result<Node, i64>> {
        if let Some(ref status) = update.lifecycle_status {
            Self::validate_lifecycle_status(status)?;
        }

        let mut rows = tx
            .conn()
            .query(
                "SELECT * FROM node WHERE id = ?1 LIMIT 1",
                libsql::params![id.to_string()],
            )
            .await
            .context("Failed to query node")?;
        let current = match rows.next().await? {
            Some(row) => Self::row_to_node(&row)?,
            None => return Err(anyhow::anyhow!("Node not found: {}", id)),
        };

        if current.version != expected_version {
            return Ok(Err(current.version));
        }

        let new_version = expected_version + 1;
        let updated_content = update.content.unwrap_or(current.content);
        let updated_node_type = update.node_type.unwrap_or(current.node_type.clone());
        let updated_props = match update.properties {
            Some(p) => serde_json::to_string(&p).context("Failed to serialize properties")?,
            None => serde_json::to_string(&current.properties)
                .context("Failed to serialize current properties")?,
        };
        let updated_title = match update.title {
            Some(t) => t,
            None => current.title,
        };
        let updated_status = update
            .lifecycle_status
            .unwrap_or(current.lifecycle_status.clone());
        let now = Utc::now().to_rfc3339();

        let rows_affected = tx.conn().execute(
            "UPDATE node SET content = ?1, node_type = ?2, properties = ?3, title = ?4, lifecycle_status = ?5, version = ?6, modified_at = ?7 WHERE id = ?8 AND version = ?9",
            libsql::params![updated_content, updated_node_type, updated_props, updated_title, updated_status, new_version, now, id.to_string(), expected_version],
        ).await.context("Failed to update node with version check")?;

        if rows_affected == 0 {
            // Lost a race between the read above and this UPDATE, inside the
            // same transaction — vanishingly unlikely (nothing else can
            // observe or mutate this row mid-transaction) but report the
            // pre-write snapshot's version rather than fabricate one, since
            // it is at least a real value this call observed.
            return Ok(Err(current.version));
        }

        let mut rows = tx
            .conn()
            .query(
                "SELECT * FROM node WHERE id = ?1 LIMIT 1",
                libsql::params![id.to_string()],
            )
            .await
            .context("Failed to query node after update")?;
        let node = match rows.next().await? {
            Some(row) => Self::row_to_node(&row)?,
            None => return Err(anyhow::anyhow!("Node not found after update")),
        };

        Ok(Ok(node))
    }

    pub async fn update_lifecycle_status(&self, id: &str, status: &str) -> Result<()> {
        Self::validate_lifecycle_status(status)?;
        self.write()
            .await
            .execute(
                "UPDATE node SET lifecycle_status = ?1 WHERE id = ?2",
                libsql::params![status.to_string(), id.to_string()],
            )
            .await
            .context("Failed to update lifecycle_status")?;
        Ok(())
    }

    pub async fn delete_node(&self, id: &str, source: Option<String>) -> Result<DeleteResult> {
        let node = match self.get_node(id).await? {
            Some(n) => n,
            None => {
                return Ok(DeleteResult {
                    existed: false,
                    deleted_count: 0,
                })
            }
        };

        self.assert_schema_deletable(&node).await?;

        // FK CASCADE handles relationship and embedding deletion
        self.write()
            .await
            .execute(
                "DELETE FROM node WHERE id = ?1",
                libsql::params![id.to_string()],
            )
            .await
            .context("Failed to delete node")?;

        self.notify(StoreChange {
            operation: StoreOperation::Deleted,
            node,
            source,
            previous_node: None,
            playbook_context: None,
        });

        Ok(DeleteResult {
            existed: true,
            deleted_count: 1,
        })
    }

    /// Atomically delete multiple nodes in a SINGLE transaction. Either
    /// every existing target row is removed or none are — a mid-batch failure rolls
    /// the whole thing back, so a caller that gets `Err` can rely on nothing having
    /// been deleted. FK CASCADE removes each node's relationships + embeddings (same
    /// as `delete_node`); children not in `ids` are left in place (edges cascade).
    /// Emits one `Deleted` notification per node that existed, AFTER commit. Returns
    /// the nodes that were deleted.
    pub async fn bulk_delete(&self, ids: &[String], source: Option<String>) -> Result<Vec<Node>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        // Snapshot the rows that actually exist (for the post-commit notifications).
        let existing = self.get_nodes_by_ids(ids).await?;
        let to_delete: Vec<Node> = ids
            .iter()
            .filter_map(|id| existing.get(id).cloned())
            .collect();
        if to_delete.is_empty() {
            return Ok(vec![]);
        }

        for node in &to_delete {
            self.assert_schema_deletable(node).await?;
        }

        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin bulk delete transaction")?;
        // Chunk under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766) — same
        // ceiling and pattern as delete_subtree_atomic above. All chunks execute
        // against this same `tx`, so a failure partway through rolls back the
        // whole batch rather than leaving it half-deleted.
        const ID_CHUNK: usize = 900;
        for chunk in to_delete.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let sql = format!("DELETE FROM node WHERE id IN ({})", placeholders.join(", "));
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|n| libsql::Value::Text(n.id.clone()))
                .collect();
            tx.execute(&sql, params)
                .await
                .context("Failed to delete nodes in bulk")?;
        }
        tx.commit()
            .await
            .context("Failed to commit bulk delete transaction")?;
        drop(db);

        for node in &to_delete {
            self.notify(StoreChange {
                operation: StoreOperation::Deleted,
                node: node.clone(),
                source: source.clone(),
                previous_node: None,
                playbook_context: None,
            });
        }
        Ok(to_delete)
    }

    /// Walk `has_child` recursively from `node_id` and return the target followed by every
    /// descendant id. Used both by `delete_subtree_atomic` (to know what to delete) and by a
    /// pre-delete access gate (to know what to check access for) — computed once and shared so
    /// the two never see different sets.
    pub async fn collect_subtree_ids(&self, node_id: &str) -> Result<Vec<String>> {
        let mut id_rows = self.read().await?.query(
            r#"WITH RECURSIVE subtree(node_id) AS (
                SELECT out_node FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child'
                UNION ALL
                SELECT r.out_node FROM relationship r
                JOIN subtree s ON r.in_node = s.node_id
                WHERE r.relationship_type = 'has_child'
            )
            SELECT DISTINCT node_id FROM subtree"#,
            libsql::params![node_id.to_string()],
        ).await.context("Failed to collect subtree descendants")?;

        let mut all_ids: Vec<String> = vec![node_id.to_string()];
        while let Some(row) = id_rows.next().await? {
            all_ids.push(row.get(0)?);
        }
        Ok(all_ids)
    }

    /// Atomically delete a node and its entire `has_child` subtree in a single transaction.
    ///
    /// **OCC contract:** Version-checks only the target node at `expected_version`. Descendants
    /// are removed unconditionally inside the same transaction. A target version mismatch rolls
    /// back the entire transaction — no partial deletion is possible.
    ///
    /// **Event emission:** Emits one `NodeDeleted` notification per deleted node (target +
    /// every descendant) after the commit, so the frontend store reconciles each removal.
    ///
    /// `subtree_ids`: the target + all descendants, as previously computed by
    /// `collect_subtree_ids`. Passed in (rather than re-walked here) so a caller that already
    /// ran a pre-delete access gate check against this exact set doesn't pay for — or risk
    /// drifting from — a second walk.
    ///
    /// Returns `(existed, deleted_nodes)` where `deleted_nodes` contains the target and all
    /// descendants that were deleted (empty vec when target didn't exist).
    pub async fn delete_subtree_atomic(
        &self,
        node_id: &str,
        expected_version: i64,
        subtree_ids: &[String],
        source: Option<String>,
    ) -> Result<(bool, Vec<Node>)> {
        // Collect the target + all descendants before mutating.
        let target = match self.get_node(node_id).await? {
            Some(n) => n,
            None => return Ok((false, vec![])),
        };

        // Only the target can be a schema node (descendants are its description
        // subtree, which is ordinary content), so the declaration guard runs on
        // the target alone.
        self.assert_schema_deletable(&target).await?;

        // OCC check on target before entering the transaction.
        if target.version != expected_version {
            return Err(anyhow::anyhow!(
                "version_conflict:{}:{}:{}",
                node_id,
                expected_version,
                target.version
            ));
        }

        let all_ids: Vec<String> = subtree_ids.to_vec();

        // Fetch all node records (needed for post-commit notifications). Chunk
        // under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766) — an
        // unchunked IN (...) over a whole subtree's ids fails to prepare once
        // the subtree crosses that count. 900 mirrors the chunk size used by
        // the other bulk store queries.
        const ID_CHUNK: usize = 900;
        let mut nodes_to_delete: Vec<Node> = Vec::new();
        // Taken before the fetch, not just before the transaction: the fetched
        // rows are what this call reports as deleted, so they must be read from
        // the same serialized point the delete runs at.
        let db = self.write().await;
        for chunk in all_ids.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let fetch_sql = format!(
                "SELECT * FROM node WHERE id IN ({})",
                placeholders.join(", ")
            );
            let fetch_params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            let mut node_rows = db
                .query(&fetch_sql, fetch_params)
                .await
                .context("Failed to fetch subtree nodes for deletion")?;
            while let Some(row) = node_rows.next().await? {
                nodes_to_delete.push(Self::row_to_node(&row)?);
            }
        }

        // Single-transaction delete — FK CASCADE cleans relationship and embedding rows.
        let tx = db
            .transaction()
            .await
            .context("Failed to begin subtree delete transaction")?;

        // Re-check target version inside the transaction to close the TOCTOU window.
        let check_sql = "SELECT version FROM node WHERE id = ?1";
        let mut check_rows = tx
            .query(check_sql, libsql::params![node_id.to_string()])
            .await
            .context("Failed to re-check target version")?;
        let actual_version: i64 = match check_rows.next().await? {
            Some(row) => row.get(0)?,
            None => {
                // Node disappeared between pre-check and transaction — idempotent.
                return Ok((false, vec![]));
            }
        };
        if actual_version != expected_version {
            return Err(anyhow::anyhow!(
                "version_conflict:{}:{}:{}",
                node_id,
                expected_version,
                actual_version
            ));
        }

        // Chunked, but every chunk executes against the same `tx` opened above —
        // one transaction covers the OCC re-check and all delete chunks, so a
        // failure partway through rolls back everything already deleted in this
        // call rather than leaving the subtree half-deleted.
        for chunk in all_ids.chunks(ID_CHUNK) {
            let del_placeholders: Vec<String> =
                (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let del_sql = format!(
                "DELETE FROM node WHERE id IN ({})",
                del_placeholders.join(", ")
            );
            let del_params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            tx.execute(&del_sql, del_params)
                .await
                .context("Failed to delete subtree nodes")?;
        }

        tx.commit()
            .await
            .context("Failed to commit subtree delete transaction")?;
        drop(db);

        // Emit one NodeDeleted notification per deleted node after commit.
        for node in &nodes_to_delete {
            self.notify(StoreChange {
                operation: StoreOperation::Deleted,
                node: node.clone(),
                source: source.clone(),
                previous_node: None,
                playbook_context: None,
            });
        }

        Ok((true, nodes_to_delete))
    }

    /// Delete all descendants of `parent_id` (the full child subtree) without touching the parent.
    ///
    /// Uses a recursive CTE to collect all descendant IDs, then deletes them in one statement.
    /// FK CASCADE cleans relationship and embedding rows automatically.
    /// No OCC check — intended for internal system operations where version checks are unnecessary.
    ///
    /// Bypasses `assert_schema_deletable`: the ids reached here are `has_child`
    /// descendants (ordinary content, e.g. a schema's description subtree) —
    /// never schema nodes themselves. Do not route schema-node ids through this.
    pub async fn delete_children_subtree_unchecked(&self, parent_id: &str) -> Result<()> {
        self.write()
            .await
            .execute(
                r#"WITH RECURSIVE subtree(node_id) AS (
                    SELECT out_node FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child'
                    UNION ALL
                    SELECT r.out_node FROM relationship r
                    JOIN subtree s ON r.in_node = s.node_id
                    WHERE r.relationship_type = 'has_child'
                )
                DELETE FROM node WHERE id IN (SELECT DISTINCT node_id FROM subtree)"#,
                libsql::params![parent_id.to_string()],
            )
            .await
            .context("Failed to delete children subtree")?;
        Ok(())
    }

    /// `_in_tx` twin of [`Self::delete_children_subtree_unchecked`] (ADR-069
    /// §1a/S3, closing F4). Run against the caller's `tx` instead of opening
    /// its own — this is what lets `replace_description_subtree` compose the
    /// delete and the recreate that follows into one transaction, so a
    /// failure recreating the subtree rolls back the delete instead of
    /// leaving the schema's description gone with nothing replacing it.
    pub(crate) async fn delete_children_subtree_unchecked_in_tx(
        tx: &Tx<'_>,
        parent_id: &str,
    ) -> Result<()> {
        tx.conn()
            .execute(
                r#"WITH RECURSIVE subtree(node_id) AS (
                    SELECT out_node FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child'
                    UNION ALL
                    SELECT r.out_node FROM relationship r
                    JOIN subtree s ON r.in_node = s.node_id
                    WHERE r.relationship_type = 'has_child'
                )
                DELETE FROM node WHERE id IN (SELECT DISTINCT node_id FROM subtree)"#,
                libsql::params![parent_id.to_string()],
            )
            .await
            .context("Failed to delete children subtree")?;
        Ok(())
    }

    /// Delete exactly the given node ids (and, via FK CASCADE, their
    /// relationship/embedding rows). No OCC check — for internal system
    /// operations. Used to prune a document's *previous* subtree during an
    /// idempotent re-import only after the fresh subtree has been inserted, so
    /// a failed insert never destroys the old content.
    ///
    /// Bypasses `assert_schema_deletable`: callers pass imported-content ids,
    /// never schema nodes. Do not route schema-node ids through this — the
    /// FK CASCADE would silently destroy their relationship declarations.
    pub async fn delete_nodes_by_ids_unchecked(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        // Chunk under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766).
        const ID_CHUNK: usize = 900;
        // One guard across all chunks so a multi-chunk delete is not interleaved
        // with another writer halfway through.
        let db = self.write().await;
        for chunk in ids.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!("DELETE FROM node WHERE id IN ({})", placeholders.join(", "));
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            db.execute(&sql, params)
                .await
                .context("Failed to delete nodes by id")?;
        }
        Ok(())
    }

    /// Remove a single top-level key from a node's properties JSON in-place using `json_remove`.
    ///
    /// Used by migrations to clear legacy fields (e.g. `description`) after they have been
    /// moved to the child-subtree representation, making the migration idempotent.
    pub async fn remove_property_key(&self, node_id: &str, key: &str) -> Result<()> {
        let json_path = format!("$.{}", key);
        self.write()
            .await
            .execute(
                "UPDATE node SET properties = json_remove(properties, ?1), modified_at = ?2 WHERE id = ?3",
                libsql::params![json_path, chrono::Utc::now().to_rfc3339(), node_id.to_string()],
            )
            .await
            .context("Failed to remove property key")?;
        Ok(())
    }

    /// Set a boolean value at a JSON path within a node's properties in-place
    /// using `json_set`. `json_path` must start with `$.` (e.g. `$._seed.user_modified`).
    ///
    /// Best-effort and OCC-bypassing: no version check, no version bump. Used
    /// to stamp bookkeeping metadata (e.g. `_seed.user_modified`) alongside a
    /// caller's own version-checked update without racing it or requiring the
    /// caller to round-trip the flag through its own `NodeUpdate`. Only safe
    /// for values where a lost or doubled write is harmless — e.g. an
    /// idempotent boolean flag, where setting it to the same value twice has
    /// no observable effect beyond an undercounted `version`. Do not use for
    /// values a concurrent writer could legitimately race to change.
    pub async fn set_property_bool(
        &self,
        node_id: &str,
        json_path: &str,
        value: bool,
    ) -> Result<()> {
        // json_set's value argument must be JSON, not SQLite's 0/1 integer
        // affinity, or the stored value round-trips as an int instead of a bool.
        let json_value = if value { "true" } else { "false" };
        self.write()
            .await
            .execute(
                "UPDATE node SET properties = json_set(properties, ?1, json(?2)) WHERE id = ?3",
                libsql::params![json_path.to_string(), json_value, node_id.to_string()],
            )
            .await
            .context("Failed to set property key")?;
        Ok(())
    }

    /// Set a string value at a JSON path within a node's properties in-place.
    /// Same best-effort, OCC-bypassing posture as [`Self::set_property_bool`]
    /// — see its doc comment for when this is (and isn't) safe to use.
    ///
    /// Unlike the bool variant, `json_set` is passed the raw string directly
    /// (not wrapped in `json(...)`), since `json()` would parse the string as
    /// JSON syntax rather than storing it as a JSON string value — a
    /// content-hash string like a hex digest happens not to be valid JSON on
    /// its own, but relying on that would be fragile.
    pub async fn set_property_string(
        &self,
        node_id: &str,
        json_path: &str,
        value: &str,
    ) -> Result<()> {
        self.write()
            .await
            .execute(
                "UPDATE node SET properties = json_set(properties, ?1, ?2) WHERE id = ?3",
                libsql::params![
                    json_path.to_string(),
                    value.to_string(),
                    node_id.to_string()
                ],
            )
            .await
            .context("Failed to set property key")?;
        Ok(())
    }

    pub async fn delete_with_version_check(
        &self,
        id: &str,
        expected_version: i64,
        source: Option<String>,
    ) -> Result<usize> {
        let node = match self.get_node(id).await? {
            Some(n) => n,
            None => return Ok(0),
        };

        if node.version != expected_version {
            return Ok(0);
        }

        let result = self.delete_node(id, source).await?;
        Ok(if result.existed { 1 } else { 0 })
    }

    pub async fn query_nodes(&self, query: NodeQuery) -> Result<Vec<Node>> {
        if let Some(ref mentioned_node_id) = query.mentioned_by {
            let mut rows = self.read().await?.query(
                "SELECT n.* FROM node n JOIN relationship r ON r.in_node = n.id WHERE r.out_node = ?1 AND r.relationship_type = 'mentions'",
                libsql::params![mentioned_node_id.clone()],
            ).await.context("Failed to query mentioned_by nodes")?;
            let mut nodes = Vec::new();
            while let Some(row) = rows.next().await? {
                nodes.push(Self::row_to_node(&row)?);
            }
            return Ok(nodes);
        }

        // Collect every scalar condition (content/title/type) into one shared
        // AND-chain so `content_contains` composes with `node_type`,
        // `title_contains`, and (below) `ids` instead of one filter silently
        // dropping the others. Shared with `count_nodes` via
        // `build_scalar_conditions` so a filter added here composes the same
        // way in both.
        let (conditions, bind_values) = Self::build_scalar_conditions(&query);

        // id-scoping (e.g. a collection's members). Build `id IN (…)` and
        // CHUNK it under SQLite's bound-parameter ceiling so a large member set
        // can't overflow the limit. Each chunk also carries the
        // content/title/node_type conditions already collected, so this path
        // stays scoped the same way the unchunked path below is. `limit`/
        // `offset` are intentionally NOT applied per-chunk — the caller
        // (`ops::query_nodes`) paginates the merged, id-scoped set in memory.
        // Per-chunk row order is not globally meaningful (chunk boundaries are
        // arbitrary), so `order_by`, when present, is applied once via an
        // in-memory sort over the FULL merged set below rather than per chunk.
        // Empty id set ⇒ no rows can match.
        if let Some(ref ids) = query.ids {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            const ID_CHUNK: usize = 900;
            let mut nodes = Vec::new();
            for chunk in ids.chunks(ID_CHUNK) {
                let mut conds = conditions.clone();
                let mut binds = bind_values.clone();
                let start = binds.len();
                let placeholders: Vec<String> = chunk
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", start + i + 1))
                    .collect();
                conds.push(format!("id IN ({})", placeholders.join(", ")));
                for id in chunk {
                    binds.push(libsql::Value::Text(id.clone()));
                }
                let sql = format!("SELECT * FROM node WHERE {}", conds.join(" AND "));
                let chunk_nodes = self
                    .query_nodes_from_sql(&sql, binds)
                    .await
                    .context("Failed to query nodes by id set")?;
                nodes.extend(chunk_nodes);
            }
            if let Some(ref order_by) = query.order_by {
                Self::sort_nodes_by(&mut nodes, order_by);
            }
            return Ok(nodes);
        }

        let where_clause = if !conditions.is_empty() {
            format!("WHERE {}", conditions.join(" AND "))
        } else {
            String::new()
        };

        // ORDER BY must run before LIMIT/OFFSET are applied — SQLite makes no
        // ordering guarantee for a plain table scan, so without this the
        // LIMIT/OFFSET below would slice an arbitrary (effectively
        // insertion-order) subset instead of the documented order, and
        // repeated calls with the same query could return different rows or
        // the same rows in a different order.
        let order_by_clause = match query.order_by.as_ref() {
            Some(order_by) => format!(" ORDER BY {}", Self::order_by_sql(order_by)),
            None => String::new(),
        };

        let limit_offset = match (query.limit, query.offset) {
            (None, None) => String::new(),
            (Some(l), None) => format!(" LIMIT {}", l),
            (None, Some(o)) => format!(" LIMIT -1 OFFSET {}", o),
            (Some(l), Some(o)) => format!(" LIMIT {} OFFSET {}", l, o),
        };

        let sql = format!(
            "SELECT * FROM node {}{}{}",
            where_clause, order_by_clause, limit_offset
        );
        let nodes = self
            .query_nodes_from_sql(&sql, bind_values)
            .await
            .context("Failed to query nodes")?;

        // Exact-substring title match found nothing: fall back to a
        // stem-token match so a word variant of a title (e.g. "groceries"
        // query vs a "...grocery store" title) still resolves. Plain
        // substring-of-title matching per token can't handle this case at all
        // ("groceries" is not a substring of "grocery store" in either
        // direction), so the fallback compares tokenized query words against
        // tokenized title words by shared stem instead. Only engaged on a
        // genuine miss so the precise exact-substring match (and its
        // ordering) stays authoritative whenever it finds anything. Skipped
        // when `content_contains` is also set — the fallback has no notion of
        // content matching, so running it would silently drop that condition.
        if nodes.is_empty() && query.content_contains.is_none() {
            if let Some(ref search_q) = query.title_contains {
                return self
                    .query_nodes_title_stem_fallback(
                        search_q,
                        query.node_type.as_deref(),
                        query.limit,
                        query.offset,
                    )
                    .await;
            }
        }
        Ok(nodes)
    }

    /// Build the shared AND-chain of scalar conditions (content/title/type)
    /// used by both `query_nodes` and `count_nodes`, so a filter added to one
    /// automatically composes the same way in the other. Deliberately
    /// excludes `id`/`ids`/`mentioned_by`, which have their own dedicated
    /// handling (early-return / id-chunking / join) in both callers. Returns
    /// the SQL condition fragments plus their positional bind values,
    /// numbered from `?1`.
    fn build_scalar_conditions(query: &NodeQuery) -> (Vec<String>, Vec<libsql::Value>) {
        let mut conditions = Vec::new();
        let mut bind_values: Vec<libsql::Value> = Vec::new();

        if let Some(ref search_q) = query.content_contains {
            let search_lower = format!("%{}%", search_q.to_lowercase());
            conditions.push(format!("LOWER(content) LIKE ?{}", bind_values.len() + 1));
            bind_values.push(libsql::Value::Text(search_lower));
        }

        if let Some(ref search_q) = query.title_contains {
            let search_lower = format!("%{}%", search_q.to_lowercase());
            conditions.push(format!(
                "title IS NOT NULL AND LOWER(title) LIKE ?{}",
                bind_values.len() + 1
            ));
            bind_values.push(libsql::Value::Text(search_lower));
        }

        // "*" is the codebase-wide wildcard convention for "all types" (see
        // `ExecuteQueryInput::target_type` and `QueryService::build_query`'s
        // identical `!= "*"` guard) — filtering for the *literal* node_type
        // '*' would never match a real row (no stored type name can be '*';
        // `validate_identifier` reserves it), so a caller-supplied wildcard
        // must skip the condition entirely rather than add one that can only
        // ever produce zero rows.
        if let Some(ref nt) = query.node_type {
            if nt != "*" {
                conditions.push(format!("node_type = ?{}", bind_values.len() + 1));
                bind_values.push(libsql::Value::Text(nt.clone()));
            }
        }

        (conditions, bind_values)
    }

    /// Count nodes matching `query` without materializing full records — the
    /// counting counterpart to `query_nodes` for callers (e.g. `nodespace
    /// diagnostics`) that only need a total, not the rows themselves. A
    /// `SELECT COUNT(*)` over a table too large to page through in full is
    /// still O(1) in response size, unlike listing every row up to some
    /// generous limit purely to call `.len()` on the result.
    ///
    /// Mirrors `query_nodes`'s priority order (`mentioned_by` > `ids` >
    /// scalar conditions) and the same AND-chain composition via
    /// `build_scalar_conditions`, but issues `SELECT COUNT(*)` instead of
    /// `SELECT *` and ignores `order_by`/`limit`/`offset` — meaningless for a
    /// scalar count. Does NOT handle `query.id` (exact id lookup); that
    /// priority tier is handled by `NodeService::count_nodes` via
    /// `get_node`, mirroring how `query_nodes_simple` handles it above the
    /// store layer.
    ///
    /// Does NOT replicate `query_nodes`'s title-stem fallback: a
    /// `title_contains` value with zero exact-substring matches counts as
    /// zero here even though `query_nodes` would then retry with stem
    /// matching. No current caller sets `title_contains` when counting; if
    /// one starts to, this is the place to add the same fallback.
    pub async fn count_nodes(&self, query: &NodeQuery) -> Result<i64> {
        if let Some(ref mentioned_node_id) = query.mentioned_by {
            return self
                .count_from_sql(
                    "SELECT COUNT(*) FROM node n JOIN relationship r ON r.in_node = n.id WHERE r.out_node = ?1 AND r.relationship_type = 'mentions'",
                    libsql::params![mentioned_node_id.clone()],
                )
                .await
                .context("Failed to count mentioned_by nodes");
        }

        let (conditions, bind_values) = Self::build_scalar_conditions(query);

        // Same id-chunking as `query_nodes` (SQLite's bound-parameter
        // ceiling), summing the per-chunk counts. Chunks are disjoint slices
        // of `ids`, so this only double-counts a row if `ids` itself
        // contains a duplicate id — matching `query_nodes`'s existing
        // (also non-deduplicating) behavior for the same input.
        if let Some(ref ids) = query.ids {
            if ids.is_empty() {
                return Ok(0);
            }
            const ID_CHUNK: usize = 900;
            let mut total: i64 = 0;
            for chunk in ids.chunks(ID_CHUNK) {
                let mut conds = conditions.clone();
                let mut binds = bind_values.clone();
                let start = binds.len();
                let placeholders: Vec<String> = chunk
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", start + i + 1))
                    .collect();
                conds.push(format!("id IN ({})", placeholders.join(", ")));
                for id in chunk {
                    binds.push(libsql::Value::Text(id.clone()));
                }
                let sql = format!("SELECT COUNT(*) FROM node WHERE {}", conds.join(" AND "));
                total += self
                    .count_from_sql(&sql, binds)
                    .await
                    .context("Failed to count nodes by id set")?;
            }
            return Ok(total);
        }

        let where_clause = if !conditions.is_empty() {
            format!("WHERE {}", conditions.join(" AND "))
        } else {
            String::new()
        };
        let sql = format!("SELECT COUNT(*) FROM node {}", where_clause);
        self.count_from_sql(&sql, bind_values)
            .await
            .context("Failed to count nodes")
    }

    /// Count root nodes (nodes with no parent edge) without listing them —
    /// the counting counterpart to `get_roots`. Mirrors `get_roots`'s
    /// `WHERE` clause exactly, so the two never disagree on which rows
    /// qualify.
    pub async fn count_roots(&self) -> Result<i64> {
        self.count_from_sql(
            "SELECT COUNT(*) FROM node WHERE id NOT IN (SELECT out_node FROM relationship WHERE relationship_type = 'has_child')",
            (),
        )
        .await
        .context("Failed to count roots")
    }

    /// Map an `OrderBy` value to a SQL `ORDER BY` fragment (column list only,
    /// no `ORDER BY` keyword). `id ASC` is always appended as a deterministic
    /// tiebreaker so two nodes that share the same primary sort value (e.g.
    /// identical `created_at` timestamps) still sort the same way on every
    /// call — without it, SQLite's tie order is unspecified and pagination
    /// across repeated calls could reorder or duplicate/skip rows at the
    /// boundary.
    fn order_by_sql(order_by: &OrderBy) -> &'static str {
        match order_by {
            OrderBy::CreatedAsc => "created_at ASC, id ASC",
            OrderBy::CreatedDesc => "created_at DESC, id ASC",
            OrderBy::ModifiedAsc => "modified_at ASC, id ASC",
            OrderBy::ModifiedDesc => "modified_at DESC, id ASC",
            OrderBy::ContentAsc => "LOWER(content) ASC, id ASC",
            OrderBy::ContentDesc => "LOWER(content) DESC, id ASC",
            OrderBy::NodeTypeAsc => "node_type ASC, id ASC",
            OrderBy::NodeTypeDesc => "node_type DESC, id ASC",
        }
    }

    /// In-memory equivalent of [`Self::order_by_sql`], used where results are
    /// merged from multiple SQL queries (the id-chunked path) and a single
    /// SQL `ORDER BY` can't cover the whole result set. Mirrors the same
    /// column choice and `id` tiebreaker so behavior matches the SQL path.
    fn sort_nodes_by(nodes: &mut [Node], order_by: &OrderBy) {
        match order_by {
            OrderBy::CreatedAsc => nodes.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.id.cmp(&b.id))
            }),
            OrderBy::CreatedDesc => nodes.sort_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| a.id.cmp(&b.id))
            }),
            OrderBy::ModifiedAsc => nodes.sort_by(|a, b| {
                a.modified_at
                    .cmp(&b.modified_at)
                    .then_with(|| a.id.cmp(&b.id))
            }),
            OrderBy::ModifiedDesc => nodes.sort_by(|a, b| {
                b.modified_at
                    .cmp(&a.modified_at)
                    .then_with(|| a.id.cmp(&b.id))
            }),
            OrderBy::ContentAsc => nodes.sort_by(|a, b| {
                a.content
                    .to_lowercase()
                    .cmp(&b.content.to_lowercase())
                    .then_with(|| a.id.cmp(&b.id))
            }),
            OrderBy::ContentDesc => nodes.sort_by(|a, b| {
                b.content
                    .to_lowercase()
                    .cmp(&a.content.to_lowercase())
                    .then_with(|| a.id.cmp(&b.id))
            }),
            OrderBy::NodeTypeAsc => {
                nodes.sort_by(|a, b| a.node_type.cmp(&b.node_type).then_with(|| a.id.cmp(&b.id)))
            }
            OrderBy::NodeTypeDesc => {
                nodes.sort_by(|a, b| b.node_type.cmp(&a.node_type).then_with(|| a.id.cmp(&b.id)))
            }
        }
    }

    /// Stem-token fallback for `title_contains` when the exact-substring
    /// match returns zero rows. Reuses the same tokenization/stop-word rules
    /// `bm25_search_roots` applies for FTS queries, but compares word-to-word
    /// by shared stem rather than via FTS5 (titles aren't in `node_fts`,
    /// which indexes only `content`) or via substring-of-whole-title (which
    /// can't match a word variant like "groceries" against "grocery store").
    ///
    /// Candidates are narrowed by `node_type` in SQL (cheap, exact), then the
    /// stem comparison runs in Rust over that set — SQL has no clean way to
    /// express "any word in title shares a stem with any query token".
    ///
    /// Matches are OR'd across query tokens (any shared stem qualifies), so a
    /// multi-word query is a broader net than the exact-substring path's
    /// implicit AND. Results are ranked by how many distinct query tokens
    /// matched before `limit`/`offset` apply, so a `limit`-truncated result
    /// keeps the closest matches rather than an arbitrary OR-qualifying set.
    async fn query_nodes_title_stem_fallback(
        &self,
        search_q: &str,
        node_type: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<Node>> {
        let query_tokens: Vec<String> = search_q
            .split_whitespace()
            .map(|t| {
                t.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|t| !t.is_empty() && !BM25_STOP_WORDS.contains(&t.as_str()))
            .take(BM25_MAX_TOKENS)
            .collect();

        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }
        let query_stems: Vec<String> = query_tokens.iter().map(|t| Self::stem_word(t)).collect();

        let (sql, params) = match node_type {
            Some(nt) => (
                "SELECT * FROM node WHERE title IS NOT NULL AND node_type = ?1".to_string(),
                vec![libsql::Value::Text(nt.to_string())],
            ),
            None => (
                "SELECT * FROM node WHERE title IS NOT NULL".to_string(),
                vec![],
            ),
        };
        let candidates = self.query_nodes_from_sql(&sql, params).await?;

        // (node, number of distinct query stems it matched) — the match count
        // is the ranking key so a truncated result keeps the closest matches.
        let mut matched: Vec<(Node, usize)> = candidates
            .into_iter()
            .filter_map(|n| {
                let title_stems: std::collections::HashSet<String> = n
                    .title
                    .as_deref()
                    .unwrap_or("")
                    .split_whitespace()
                    .map(|w| {
                        Self::stem_word(
                            &w.trim_matches(|c: char| !c.is_alphanumeric())
                                .to_lowercase(),
                        )
                    })
                    .collect();
                let match_count = query_stems
                    .iter()
                    .filter(|qs| title_stems.contains(*qs))
                    .count();
                (match_count > 0).then_some((n, match_count))
            })
            .collect();

        matched.sort_by(|(_, a), (_, b)| b.cmp(a));
        let mut matched: Vec<Node> = matched.into_iter().map(|(n, _)| n).collect();

        if let Some(o) = offset {
            matched = matched.into_iter().skip(o).collect();
        }
        if let Some(l) = limit {
            matched.truncate(l);
        }
        Ok(matched)
    }

    /// Strip a common plural suffix from a lowercased word token, so two
    /// tokens that are the same word variant (e.g. "grocery" / "groceries",
    /// "task" / "tasks") stem to the same value.
    ///
    /// Deliberately conservative — a small, explicit suffix list rather than a
    /// full stemmer (Porter/Snowball). A fixed-length shared-prefix heuristic
    /// was tried first and rejected: it produced false positives on unrelated
    /// words that happen to share a stem-length prefix ("cart"/"carton",
    /// "part"/"party", "read"/"ready"). Suffix stripping avoids all of those
    /// because it only trims a recognized morpheme, not an arbitrary prefix
    /// length. Known gap: doesn't undo consonant-doubling gerunds
    /// ("run"/"running") or silent-e plurals ("date"/"dates") — those don't
    /// stem equal here, which is a false negative, not a false positive, so
    /// it's the safe direction to be wrong in for a fallback matcher.
    fn stem_word(w: &str) -> String {
        if w.len() > 4 && w.ends_with("ies") {
            return format!("{}y", &w[..w.len() - 3]);
        }
        if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("es") {
            return w[..w.len() - 1].to_string();
        }
        if w.len() > 4 && w.ends_with("es") {
            return w[..w.len() - 2].to_string();
        }
        w.to_string()
    }

    pub async fn get_children(&self, parent_id: &str) -> Result<Vec<Node>> {
        let mut rows = self.read().await?.query(
            "SELECT n.* FROM node n JOIN relationship r ON r.out_node = n.id WHERE r.in_node = ?1 AND r.relationship_type = 'has_child' ORDER BY json_extract(r.properties, '$.order') ASC",
            libsql::params![parent_id.to_string()],
        ).await.context("Failed to get children")?;

        let mut nodes = Vec::new();
        while let Some(row) = rows.next().await? {
            nodes.push(Self::row_to_node(&row)?);
        }
        Ok(nodes)
    }

    pub async fn get_roots(
        &self,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<Node>> {
        let limit_offset = match (limit, offset) {
            (None, None) => String::new(),
            (Some(l), None) => format!(" LIMIT {}", l),
            (None, Some(o)) => format!(" LIMIT -1 OFFSET {}", o),
            (Some(l), Some(o)) => format!(" LIMIT {} OFFSET {}", l, o),
        };

        let sql = format!(
            "SELECT * FROM node WHERE id NOT IN (SELECT out_node FROM relationship WHERE relationship_type = 'has_child') ORDER BY id ASC{}",
            limit_offset
        );

        self.query_nodes_from_sql(&sql, ()).await
    }

    pub async fn get_parent(&self, child_id: &str) -> Result<Option<Node>> {
        let mut rows = self.read().await?.query(
            "SELECT n.* FROM node n JOIN relationship r ON r.in_node = n.id WHERE r.out_node = ?1 AND r.relationship_type = 'has_child' LIMIT 1",
            libsql::params![child_id.to_string()],
        ).await.context("Failed to get parent")?;

        if let Some(row) = rows.next().await? {
            Ok(Some(Self::row_to_node(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_parent_id(&self, child_id: &str) -> Result<Option<String>> {
        let mut rows = self.read().await?.query(
            "SELECT in_node FROM relationship WHERE out_node = ?1 AND relationship_type = 'has_child' LIMIT 1",
            libsql::params![child_id.to_string()],
        ).await.context("Failed to get parent id")?;

        if let Some(row) = rows.next().await? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_node_type(&self, node_id: &str) -> Result<Option<String>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT node_type FROM node WHERE id = ?1 LIMIT 1",
                libsql::params![node_id.to_string()],
            )
            .await
            .context("Failed to get node type")?;

        if let Some(row) = rows.next().await? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_nodes_in_subtree(&self, root_id: &str) -> Result<Vec<Node>> {
        let (all_nodes, _) = self.get_subtree_with_relationships(root_id).await?;
        Ok(all_nodes.into_iter().filter(|n| n.id != root_id).collect())
    }

    pub async fn get_subtree_with_relationships(
        &self,
        root_id: &str,
    ) -> Result<(Vec<Node>, Vec<RelationshipRecord>)> {
        let start = std::time::Instant::now();

        // WITH RECURSIVE to collect all descendants. Scoped so the cursor drops
        // before the `get_node` below checks out a second reader connection —
        // see `ReadRows` in `connections.rs`.
        let mut descendant_ids = vec![root_id.to_string()];
        {
            let mut id_rows = self.read().await?.query(
                r#"WITH RECURSIVE subtree(node_id, depth) AS (
                SELECT out_node, 1 FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child'
                UNION ALL
                SELECT r.out_node, s.depth + 1 FROM relationship r
                JOIN subtree s ON r.in_node = s.node_id
                WHERE r.relationship_type = 'has_child' AND s.depth < 100
            )
            SELECT DISTINCT node_id FROM subtree"#,
                libsql::params![root_id.to_string()],
            ).await.context("Failed to query descendants")?;

            while let Some(row) = id_rows.next().await? {
                descendant_ids.push(row.get(0)?);
            }
        }

        if descendant_ids.len() == 1 {
            // Just the root — verify it exists
            let root = match self.get_node(root_id).await? {
                Some(n) => n,
                None => return Ok((vec![], vec![])),
            };
            return Ok((vec![root], vec![]));
        }

        // Fetch all nodes. Chunk under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER
        // (32766) — a subtree with more descendants than that would otherwise fail
        // to prepare, making a subtree too large to delete also too large to open.
        const ID_CHUNK: usize = 900;
        let mut all_nodes = Vec::new();
        for chunk in descendant_ids.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "SELECT * FROM node WHERE id IN ({})",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            let mut node_rows = self
                .read()
                .await?
                .query(&sql, params)
                .await
                .context("Failed to fetch subtree nodes")?;
            while let Some(row) = node_rows.next().await? {
                all_nodes.push(Self::row_to_node(&row)?);
            }
        }

        if all_nodes.is_empty() {
            return Ok((vec![], vec![]));
        }

        // Fetch relationships within subtree, same chunking.
        let mut relationships = Vec::new();
        for chunk in descendant_ids.chunks(ID_CHUNK) {
            let rel_placeholders: Vec<String> =
                (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let rel_sql = format!(
                "SELECT id, in_node, out_node, relationship_type, properties FROM relationship WHERE in_node IN ({}) AND relationship_type = 'has_child'",
                rel_placeholders.join(", ")
            );
            let rel_params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            let mut rel_rows = self
                .read()
                .await?
                .query(&rel_sql, rel_params)
                .await
                .context("Failed to fetch subtree relationships")?;
            while let Some(row) = rel_rows.next().await? {
                relationships.push(Self::row_to_relationship(&row)?);
            }
        }

        tracing::debug!(
            "get_subtree_with_relationships: {:?} for root_id={} ({} nodes)",
            start.elapsed(),
            root_id,
            all_nodes.len()
        );

        Ok((all_nodes, relationships))
    }

    pub async fn get_relationships_in_subtree(
        &self,
        root_id: &str,
    ) -> Result<Vec<RelationshipRecord>> {
        let (_, relationships) = self.get_subtree_with_relationships(root_id).await?;
        Ok(relationships)
    }

    async fn validate_no_cycle(&self, parent_id: &str, child_id: &str) -> Result<()> {
        // Check if parent_id is a descendant of child_id (would create cycle)
        let mut rows = self.read().await?.query(
            r#"WITH RECURSIVE desc(node_id, depth) AS (
                SELECT out_node, 1 FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child'
                UNION ALL
                SELECT r.out_node, d.depth + 1 FROM relationship r
                JOIN desc d ON r.in_node = d.node_id
                WHERE r.relationship_type = 'has_child' AND d.depth < 100
            )
            SELECT node_id FROM desc WHERE node_id = ?2 LIMIT 1"#,
            libsql::params![child_id.to_string(), parent_id.to_string()],
        ).await.context("Failed to check for cycles")?;

        if rows.next().await?.is_some() {
            return Err(anyhow::anyhow!(
                "Cannot create parent-child relationship: would create cycle. \
                Node '{}' is a descendant of node '{}'.",
                parent_id,
                child_id
            ));
        }
        Ok(())
    }

    /// Cycle guard for collection-hierarchy `member_of` edges. The
    /// `has_child` tree has `validate_no_cycle`, but collection hierarchy is built
    /// from `member_of` (a sub-collection is a member_of its parent) and had no
    /// equivalent — so `a member_of b` + `b member_of a` produced a cycle in the
    /// supposed DAG, which now makes the recursive members walk loop.
    ///
    /// `member_of` stores in_node = child/member, out_node = parent/collection.
    /// Adding `source member_of target` makes `target` an ancestor of `source`;
    /// that is a cycle iff `target` is already a DESCENDANT of `source`. Walk the
    /// member_of subtree downward from `source` (out_node = current → in_node =
    /// child) and fail if it reaches `target`. A self-edge is trivially a cycle.
    pub async fn validate_no_member_of_cycle(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<()> {
        if source_id == target_id {
            return Err(anyhow::anyhow!(
                "collection_cycle: '{}' cannot be a member of itself",
                source_id
            ));
        }
        let mut rows = self
            .read()
            .await?
            .query(
                r#"WITH RECURSIVE descendants(node_id, depth) AS (
                SELECT in_node, 1 FROM relationship
                  WHERE out_node = ?1 AND relationship_type = 'member_of'
                UNION ALL
                SELECT r.in_node, d.depth + 1 FROM relationship r
                JOIN descendants d ON r.out_node = d.node_id
                WHERE r.relationship_type = 'member_of' AND d.depth < 100
            )
            SELECT node_id FROM descendants WHERE node_id = ?2 LIMIT 1"#,
                libsql::params![source_id.to_string(), target_id.to_string()],
            )
            .await
            .context("Failed to check for member_of cycle")?;
        if rows.next().await?.is_some() {
            return Err(anyhow::anyhow!(
                "collection_cycle: '{}' is already a descendant of '{}', so making it the parent would create a cycle",
                target_id,
                source_id
            ));
        }
        Ok(())
    }

    /// Re-spread a parent's `has_child` order keys evenly.
    ///
    /// Takes the caller's writer connection rather than acquiring one: its only
    /// caller (`move_node`) is already holding the guard around its own
    /// read → compute → write, and the guard is not re-entrant. Passing it down
    /// also keeps the rebalance inside that same serialized span, so the
    /// re-read `move_node` does afterwards is guaranteed to see these new keys.
    async fn rebalance_children_for_parent(
        &self,
        db: &libsql::Connection,
        parent_id: &str,
    ) -> Result<()> {
        let mut rows = db.query(
            "SELECT id, out_node FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child' ORDER BY json_extract(properties, '$.order') ASC",
            libsql::params![parent_id.to_string()],
        ).await.context("Failed to get children for rebalancing")?;

        let mut rels: Vec<(String, String)> = Vec::new();
        while let Some(row) = rows.next().await? {
            rels.push((row.get(0)?, row.get(1)?));
        }

        if rels.is_empty() {
            return Ok(());
        }

        let new_orders = FractionalOrderCalculator::rebalance(rels.len());
        let tx = db
            .transaction()
            .await
            .context("Failed to begin rebalance transaction")?;

        for (i, (rel_id, _)) in rels.iter().enumerate() {
            let props = serde_json::json!({"order": new_orders[i]}).to_string();
            tx.execute(
                "UPDATE relationship SET properties = ?1 WHERE id = ?2",
                libsql::params![props, rel_id.clone()],
            )
            .await
            .context("Failed to rebalance relationship")?;
        }

        tx.commit().await.context("Failed to commit rebalance")?;
        Ok(())
    }

    /// ADR-059 §2 (reparent side of the root-only content-membership rule): a node
    /// that holds a `member_of` edge is a root member and must not be given a
    /// `has_child` parent — that would make it a forbidden interior member with no
    /// `member_of` write for the store's forward guard to catch. Called at BOTH
    /// store-level sites that attach an *existing* node to a parent — `move_node`
    /// (the service `move_node` and `upsert_node_with_parent` reparent paths) and
    /// `bulk_create_has_child` (the sync-apply cold-sweep) — so every reparent
    /// path is covered, symmetrically with the forward guard `assert_root_only_
    /// membership` on the `member_of` INSERT sites. (Fresh-node attach sites can't
    /// pre-hold a membership; `move_children_to_parent` only moves already-interior
    /// nodes.) Rejects rather than dropping the membership (a node can hold several
    /// grants, each an independent access path). `collection` (nesting) and
    /// `person` (grantee, ADR-037 §4) nodes are exempt. A single chunked query
    /// keeps the bulk/cold-sweep path a single round trip.
    pub(crate) async fn assert_may_gain_parent(&self, node_ids: &[&str]) -> Result<()> {
        if node_ids.is_empty() {
            return Ok(());
        }
        let mut unique: Vec<&str> = node_ids.to_vec();
        unique.sort_unstable();
        unique.dedup();

        // Chunk the `IN (...)` under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766).
        const ID_CHUNK: usize = 900;
        for chunk in unique.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            // Offenders: non-exempt nodes that already hold a `member_of` edge.
            let sql = format!(
                "SELECT n.id FROM node n \
                 WHERE n.id IN ({}) \
                   AND n.node_type NOT IN ('collection', 'person') \
                   AND EXISTS(SELECT 1 FROM relationship r \
                              WHERE r.in_node = n.id AND r.relationship_type = 'member_of')",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.to_string()))
                .collect();
            // Drain and drop the cursor before `get_node_memberships` checks out a
            // second reader connection — see `ReadRows` in `connections.rs`.
            let offender: Option<String> = {
                let mut rows = self
                    .read()
                    .await?
                    .query(&sql, params)
                    .await
                    .context("Failed to validate root-only membership on reparent")?;
                match rows.next().await? {
                    Some(row) => Some(row.get(0)?),
                    None => None,
                }
            };
            if let Some(offender) = offender {
                let memberships = self.get_node_memberships(&offender).await?;
                return Err(anyhow::anyhow!(
                    "member_of_not_root: node '{}' holds collection membership ({}) and cannot be moved under a parent — only root nodes may hold collection membership (ADR-059 §2). Remove it from the collection(s) first, or move its root instead.",
                    offender,
                    memberships.join(", ")
                ));
            }
        }
        Ok(())
    }

    pub async fn move_node(
        &self,
        node_id: &str,
        new_parent_id: Option<&str>,
        insert_after_sibling_id: Option<&str>,
    ) -> Result<f64> {
        let node_id = node_id.to_string();
        let new_parent_id = new_parent_id.map(|s| s.to_string());
        let insert_after_sibling_id = insert_after_sibling_id.map(|s| s.to_string());

        if !self.node_exists(&node_id).await? {
            return Err(anyhow::anyhow!("Node not found: {}", node_id));
        }

        let current_parent_id = self.get_parent_id(&node_id).await?;
        let is_same_parent_reorder = match (&new_parent_id, &current_parent_id) {
            (Some(new_pid), Some(cur_pid)) => new_pid == cur_pid,
            (None, None) => true,
            _ => false,
        };

        if let Some(ref parent_id) = new_parent_id {
            if !self.node_exists(parent_id).await? {
                return Err(anyhow::anyhow!("Parent node not found: {}", parent_id));
            }
            self.validate_no_cycle(parent_id, &node_id).await?;
            // ADR-059 §2: a member cannot be moved into an interior position.
            self.assert_may_gain_parent(&[node_id.as_str()]).await?;
        }

        // Held across the sibling-order read → fractional-key compute → write-back
        // (including any rebalance) until the function returns. Without it, two
        // concurrent same-parent reorders interleave and compute overlapping order
        // keys from the same stale snapshot, corrupting the final sibling order.
        let db = self.write().await;

        let new_order = if let Some(ref parent_id) = new_parent_id {
            // Get ordered siblings excluding the moving node
            let mut rows = db.query(
                "SELECT out_node, json_extract(properties, '$.order') as ord FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child' AND out_node != ?2 ORDER BY json_extract(properties, '$.order') ASC",
                libsql::params![parent_id.clone(), node_id.clone()],
            ).await.context("Failed to get sibling relationships")?;

            let mut siblings: Vec<(String, f64)> = Vec::new();
            while let Some(row) = rows.next().await? {
                let child_id: String = row.get(0)?;
                let ord: Option<f64> = row.get(1)?;
                siblings.push((child_id, ord.unwrap_or(0.0)));
            }

            if let Some(after_id) = insert_after_sibling_id {
                if let Some(after_index) = siblings.iter().position(|(id, _)| id == &after_id) {
                    let prev_order = siblings[after_index].1;
                    let next_order = siblings.get(after_index + 1).map(|(_, o)| *o);

                    if let Some(next) = next_order {
                        if (next - prev_order) < 0.0001 {
                            self.rebalance_children_for_parent(&db, parent_id).await?;
                            // Re-query after rebalancing
                            let mut rows2 = db.query(
                                "SELECT out_node, json_extract(properties, '$.order') as ord FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child' AND out_node != ?2 ORDER BY json_extract(properties, '$.order') ASC",
                                libsql::params![parent_id.clone(), node_id.clone()],
                            ).await.context("Failed to get siblings after rebalancing")?;
                            let mut siblings2: Vec<(String, f64)> = Vec::new();
                            while let Some(row) = rows2.next().await? {
                                let cid: String = row.get(0)?;
                                let ord: Option<f64> = row.get(1)?;
                                siblings2.push((cid, ord.unwrap_or(0.0)));
                            }
                            if let Some(after_index2) =
                                siblings2.iter().position(|(id, _)| id == &after_id)
                            {
                                let prev2 = siblings2[after_index2].1;
                                let next2 = siblings2.get(after_index2 + 1).map(|(_, o)| *o);
                                FractionalOrderCalculator::calculate_order(Some(prev2), next2)
                            } else {
                                let last = siblings2.last().map(|(_, o)| *o);
                                FractionalOrderCalculator::calculate_order(last, None)
                            }
                        } else {
                            FractionalOrderCalculator::calculate_order(Some(prev_order), next_order)
                        }
                    } else {
                        FractionalOrderCalculator::calculate_order(Some(prev_order), None)
                    }
                } else {
                    let last = siblings.last().map(|(_, o)| *o);
                    FractionalOrderCalculator::calculate_order(last, None)
                }
            } else {
                let first = siblings.first().map(|(_, o)| *o);
                FractionalOrderCalculator::calculate_order(None, first)
            }
        } else {
            0.0
        };

        let now = Utc::now().to_rfc3339();

        if let Some(ref parent_id) = new_parent_id {
            if is_same_parent_reorder {
                let props = serde_json::json!({"order": new_order}).to_string();
                db.execute(
                    "UPDATE relationship SET properties = ?1, version = version + 1, modified_at = ?2 WHERE in_node = ?3 AND out_node = ?4 AND relationship_type = 'has_child'",
                    libsql::params![props, now, parent_id.clone(), node_id.clone()],
                ).await.context("Failed to update relationship order")?;
            } else {
                // Cross-parent move: delete the old has_child edge, create the new
                // one. These MUST be one transaction — if the INSERT fails
                // (constraint / IO / crash / cancel) after the DELETE committed, the
                // node is left with NO has_child edge: a silently-orphaned root.
                // Wrapping in a tx makes it all-or-nothing, matching the atomicity
                // of `move_children_to_parent` / `delete_subtree_atomic`.
                let rel_id = uuid::Uuid::new_v4().to_string();
                let props = serde_json::json!({"order": new_order}).to_string();
                let tx = db
                    .transaction()
                    .await
                    .context("Failed to begin move_node reparent transaction")?;
                tx.execute(
                    "DELETE FROM relationship WHERE out_node = ?1 AND relationship_type = 'has_child'",
                    libsql::params![node_id.clone()],
                ).await.context("Failed to delete old parent relationship")?;
                tx.execute(
                    "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'has_child', ?4, 1, ?5, ?6)",
                    libsql::params![rel_id, parent_id.clone(), node_id.clone(), props, now.clone(), now],
                ).await.context("Failed to create new parent relationship")?;
                tx.commit()
                    .await
                    .context("Failed to commit move_node reparent transaction")?;
            }
        } else {
            // Make root: delete parent relationship
            db.execute(
                "DELETE FROM relationship WHERE out_node = ?1 AND relationship_type = 'has_child'",
                libsql::params![node_id.clone()],
            )
            .await
            .context("Failed to delete parent relationship")?;
        }

        Ok(new_order)
    }

    /// Re-parent an ordered set of existing children to `new_parent_id` in a
    /// single transaction. Validates each child's version inside the transaction
    /// using `SELECT changes()` after a version-gated DELETE — any mismatch causes
    /// the full transaction to roll back (all-or-nothing OCC).
    ///
    /// Returns the assigned fractional order for each child, preserving input
    /// array order as sibling order under the new parent.
    pub async fn move_children_to_parent(
        &self,
        new_parent_id: &str,
        children: &[(&str, i64)],
    ) -> Result<Vec<f64>> {
        if children.is_empty() {
            return Ok(Vec::new());
        }

        let now = Utc::now().to_rfc3339();

        // Held across the existing-max-order read → compute → write below, so a
        // concurrent move/create under the same parent cannot assign a colliding
        // key. No re-entrancy: the transaction below issues raw DELETE/INSERT and
        // calls no other guard-taking path.
        let db = self.write().await;

        // Append the moved children AFTER new_parent_id's existing children: read
        // the current max has_child order under the new parent and seed the
        // sequence from it. Previously this assigned 1.0, 2.0, 3.0… ignoring
        // existing siblings, so moving into a NON-EMPTY parent collided their
        // order keys even single-threaded.
        let existing_max: Option<f64> = {
            let mut rows = db.query(
                "SELECT json_extract(properties, '$.order') as ord FROM relationship WHERE in_node = ?1 AND relationship_type = 'has_child' ORDER BY json_extract(properties, '$.order') DESC LIMIT 1",
                libsql::params![new_parent_id.to_string()],
            ).await.context("Failed to read existing child order for move_children_to_parent")?;
            if let Some(row) = rows.next().await? {
                Some(row.get::<Option<f64>>(0)?.unwrap_or(0.0))
            } else {
                None
            }
        };

        // Compute sequential fractional orders: the first appended after
        // `existing_max` (or from scratch when the parent is empty), each
        // subsequent after the previously assigned order.
        let mut orders: Vec<f64> = Vec::with_capacity(children.len());
        for _ in 0..children.len() {
            let prev = orders.last().copied().or(existing_max);
            orders.push(FractionalOrderCalculator::calculate_order(prev, None));
        }

        let tx = db
            .transaction()
            .await
            .context("Failed to begin move_children_to_parent transaction")?;

        for ((child_id, expected_version), &order) in children.iter().zip(orders.iter()) {
            let child_id = child_id.to_string();

            // Delete the old has_child edge only if the node's version matches.
            // If no rows are affected the version has been bumped by a concurrent
            // writer — detect that with SELECT changes() and abort the transaction.
            tx.execute(
                "DELETE FROM relationship WHERE out_node = ?1 AND relationship_type = 'has_child' AND EXISTS (SELECT 1 FROM node WHERE id = ?1 AND version = ?2)",
                libsql::params![child_id.clone(), *expected_version],
            )
            .await
            .context("Failed to delete old has_child edge")?;

            // Verify the DELETE actually removed a row (i.e. the version matched).
            let mut changes_rows = tx
                .query("SELECT changes()", libsql::params![])
                .await
                .context("Failed to query changes()")?;
            if let Some(row) = changes_rows.next().await? {
                let affected: i64 = row.get(0)?;
                if affected == 0 {
                    return Err(anyhow::anyhow!(
                        "VERSION_CONFLICT: node '{}' version mismatch (expected {})",
                        child_id,
                        expected_version
                    ));
                }
            }

            // Insert new has_child edge under new_parent_id.
            let rel_id = uuid::Uuid::new_v4().to_string();
            let rel_props = serde_json::json!({"order": order}).to_string();
            tx.execute(
                "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'has_child', ?4, 1, ?5, ?6)",
                libsql::params![
                    rel_id,
                    new_parent_id.to_string(),
                    child_id,
                    rel_props,
                    now.clone(),
                    now.clone()
                ],
            )
            .await
            .context("Failed to insert new has_child edge")?;
        }

        tx.commit()
            .await
            .context("Failed to commit move_children_to_parent")?;

        Ok(orders)
    }

    pub async fn get_schema(&self, node_type: &str) -> Result<Option<Value>> {
        let node = self.get_node(node_type).await?;
        Ok(node.map(|n| n.properties))
    }

    pub async fn rename_schema_field(&self, type_id: &str, from: &str, to: &str) -> Result<u64> {
        if from.is_empty() || to.is_empty() {
            return Err(anyhow::anyhow!("Field names must not be empty"));
        }
        if from == to {
            return Err(anyhow::anyhow!(
                "Source and destination field names are the same: '{}'",
                from
            ));
        }

        // Guard held across the whole read-modify-write, and every UPDATE runs
        // in ONE transaction.
        //
        // The guard has to span the read: the rewritten properties are computed
        // from the rows read here, so a concurrent property write landing
        // between the read and the UPDATE would be silently clobbered.
        //
        // It also has to span the whole loop, which does block other writers
        // for the duration on a type with many instances. Releasing and
        // re-taking it between batches would trade that for a correctness hole,
        // not just a weaker guarantee: a concurrent `create_node`/`update_node`
        // could write an instance carrying the OLD field name after the rename
        // has already swept past it, leaving the type permanently half-renamed.
        // A schema field rename is a rare, user-initiated migration, so paying
        // for it with a bounded write pause is the right side of that trade.
        //
        // The transaction earns its place twice over: it makes a partial
        // failure roll back to a coherent state instead of leaving half the
        // instances renamed, and it collapses N autocommits into one fsync,
        // which is what actually keeps the pause bounded.
        let db = self.write().await;
        let mut rows = db
            .query(
                "SELECT id, properties FROM node WHERE node_type = ?1",
                libsql::params![type_id.to_string()],
            )
            .await
            .context("Failed to fetch nodes for field rename")?;

        let mut nodes: Vec<(String, Value)> = Vec::new();
        while let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            let props_str: String = row.get(1)?;
            let props: Value = serde_json::from_str(&props_str).unwrap_or(serde_json::json!({}));
            nodes.push((id, props));
        }

        let mut affected = 0u64;
        let now = Utc::now().to_rfc3339();

        let tx = db
            .transaction()
            .await
            .context("Failed to begin schema field rename transaction")?;

        for (node_id, mut properties) in nodes {
            let had_field = if let Some(ns_obj) = properties
                .as_object_mut()
                .and_then(|p| p.get_mut(type_id))
                .and_then(|ns| ns.as_object_mut())
            {
                if let Some(value) = ns_obj.remove(from) {
                    ns_obj.insert(to.to_string(), value);
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if had_field {
                let props_json =
                    serde_json::to_string(&properties).context("Failed to serialize properties")?;
                tx.execute(
                    "UPDATE node SET properties = ?1, modified_at = ?2 WHERE id = ?3",
                    libsql::params![props_json, now.clone(), node_id],
                )
                .await
                .context("Failed to update node during field rename")?;
                affected += 1;
            }
        }

        tx.commit()
            .await
            .context("Failed to commit schema field rename")?;
        drop(db);

        tracing::info!(
            type_id = %type_id,
            from = %from,
            to = %to,
            affected = affected,
            "rename_schema_field: migrated {} node(s)",
            affected
        );

        Ok(affected)
    }

    /// `_in_tx` twin of [`Self::rename_schema_field`] (ADR-069 §1a/S3,
    /// closing F3). Identical migration logic, run against the caller's `tx`
    /// instead of opening its own — this is what lets `rename_schema_field`'s
    /// data migration and the schema-definition rewrite that follows it
    /// (`update_node_unchecked_in_tx`) land in one `NodeService`-level
    /// transaction, so a failure in the second step rolls back the first
    /// instead of leaving instance data rekeyed under a name the schema no
    /// longer declares.
    pub(crate) async fn rename_schema_field_in_tx(
        tx: &Tx<'_>,
        type_id: &str,
        from: &str,
        to: &str,
    ) -> Result<u64> {
        if from.is_empty() || to.is_empty() {
            return Err(anyhow::anyhow!("Field names must not be empty"));
        }
        if from == to {
            return Err(anyhow::anyhow!(
                "Source and destination field names are the same: '{}'",
                from
            ));
        }

        let mut rows = tx
            .conn()
            .query(
                "SELECT id, properties FROM node WHERE node_type = ?1",
                libsql::params![type_id.to_string()],
            )
            .await
            .context("Failed to fetch nodes for field rename")?;

        let mut nodes: Vec<(String, Value)> = Vec::new();
        while let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            let props_str: String = row.get(1)?;
            let props: Value = serde_json::from_str(&props_str).unwrap_or(serde_json::json!({}));
            nodes.push((id, props));
        }

        let mut affected = 0u64;
        let now = Utc::now().to_rfc3339();

        for (node_id, mut properties) in nodes {
            let had_field = if let Some(ns_obj) = properties
                .as_object_mut()
                .and_then(|p| p.get_mut(type_id))
                .and_then(|ns| ns.as_object_mut())
            {
                if let Some(value) = ns_obj.remove(from) {
                    ns_obj.insert(to.to_string(), value);
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if had_field {
                let props_json =
                    serde_json::to_string(&properties).context("Failed to serialize properties")?;
                tx.conn()
                    .execute(
                        "UPDATE node SET properties = ?1, modified_at = ?2 WHERE id = ?3",
                        libsql::params![props_json, now.clone(), node_id],
                    )
                    .await
                    .context("Failed to update node during field rename")?;
                affected += 1;
            }
        }

        Ok(affected)
    }

    /// Atomically update a batch of nodes in a single transaction.
    ///
    /// **OCC contract:** This is an intentional last-write-wins fast-path for trusted internal
    /// callers (e.g. AI skill pipelines, markdown import). It does NOT perform version-checked
    /// OCC. Callers that need conflict detection should use `update_node` (the single-node path)
    /// in a loop or use the daemon `update_nodes_batch` RPC which threads per-node versions.
    ///
    /// **TOCTOU note:** All fields are written without a pre-read. Unspecified fields
    /// (`None`) are preserved via SQL `COALESCE` — the entire read-modify-write is one
    /// atomic SQL statement per node inside the transaction with no pool reads escaping
    /// the tx.
    ///
    /// **`properties` semantics:** `Some(value)` **replaces** the existing JSON object
    /// entirely (unlike `update_node`, which merges key-by-key). `None` keeps the existing
    /// value unchanged.
    ///
    /// **`title` semantics:** `Some(Some("text"))` sets a new title; `Some(None)` clears
    /// it to NULL; `None` leaves the existing title unchanged.
    ///
    /// Returns an error (and rolls back) if any node id is not found.
    pub async fn bulk_update(&self, updates: Vec<(String, NodeUpdate)>) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        const MAX_BATCH_SIZE: usize = 1000;
        if updates.len() > MAX_BATCH_SIZE {
            return Err(anyhow::anyhow!(
                "Bulk update batch size ({}) exceeds maximum ({})",
                updates.len(),
                MAX_BATCH_SIZE
            ));
        }

        let now = Utc::now().to_rfc3339();
        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin bulk update transaction")?;

        for (id, update) in &updates {
            if let Some(ref status) = update.lifecycle_status {
                Self::validate_lifecycle_status(status)?;
            }

            let props_json = update
                .properties
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .context("Failed to serialize properties in bulk update")?;

            // All five NodeUpdate fields are covered. COALESCE preserves the existing
            // column value when the caller passes None, closing the TOCTOU window with
            // no pre-read escaping the transaction.
            let affected = tx
                .execute(
                    "UPDATE node SET \
                    content          = COALESCE(?1, content), \
                    node_type        = COALESCE(?2, node_type), \
                    properties       = COALESCE(?3, properties), \
                    lifecycle_status = COALESCE(?4, lifecycle_status), \
                    version          = version + 1, \
                    modified_at      = ?5 \
                WHERE id = ?6",
                    libsql::params![
                        update.content.clone(),
                        update.node_type.clone(),
                        props_json,
                        update.lifecycle_status.clone(),
                        now.clone(),
                        id.clone()
                    ],
                )
                .await
                .context("Failed to update node in bulk update")?;

            if affected == 0 {
                return Err(anyhow::anyhow!("Node not found: {}", id));
            }

            // `title` uses Option<Option<String>>: Some(Some(t)) sets, Some(None) clears
            // to NULL, None skips. COALESCE can't express "write NULL intentionally", so
            // we handle title with a separate statement only when the caller touches it.
            if let Some(title) = &update.title {
                tx.execute(
                    "UPDATE node SET title = ?1 WHERE id = ?2",
                    libsql::params![title.clone(), id.clone()],
                )
                .await
                .context("Failed to update title in bulk update")?;
            }
        }

        tx.commit().await.context("Failed to commit bulk update")?;
        Ok(())
    }

    /// `source`/`playbook_context` are threaded through to every node's
    /// `StoreChange` notification exactly like single-row `create_node` —
    /// callers that need every event tagged (e.g. `NodeService::bulk_create`
    /// passing `self.client_id.clone()`) must NOT be silently downgraded to
    /// an untagged write. A caller that doesn't care passes `None`/`None`,
    /// same as `create_node`'s own callers do.
    pub async fn batch_create_nodes(
        &self,
        nodes: Vec<Node>,
        source: Option<String>,
        playbook_context: Option<crate::db::events::PlaybookExecutionContext>,
    ) -> Result<Vec<Node>> {
        let mut created = Vec::new();
        for node in nodes {
            created.push(
                self.create_node(node, source.clone(), playbook_context.clone())
                    .await?,
            );
        }
        Ok(created)
    }

    /// Insert a batch of nodes (and optional parent→child relationships) in a single transaction.
    ///
    /// **Partial-failure contract:** All inserts occur inside one transaction. Any error
    /// (duplicate id, constraint violation, serialization failure) triggers an early `?`-return,
    /// which drops `tx` without calling `commit()` — the entire batch is rolled back atomically.
    /// Callers never observe a partial write.
    ///
    /// **Validation note:** `validate_node_type` is a pure in-memory check against
    /// `self.valid_node_types`; it does not touch the database and therefore cannot read
    /// uncommitted state from `tx`.
    pub async fn bulk_create_hierarchy(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        let now = Utc::now().to_rfc3339();
        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin bulk hierarchy transaction")?;

        for (id, node_type, content, parent_id, order, properties) in &nodes {
            self.validate_node_type(node_type)?;

            let properties = if properties.is_null() {
                serde_json::json!({})
            } else {
                properties.clone()
            };
            let props_json =
                serde_json::to_string(&properties).context("Failed to serialize properties")?;

            let title =
                Self::compute_title_for_bulk_insert(node_type, parent_id.as_deref(), content);

            tx.execute(
                "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, ?5, 'active', 1, ?6, ?7)",
                libsql::params![id.clone(), node_type.clone(), content.clone(), props_json, title, now.clone(), now.clone()],
            ).await.context("Failed to insert node in bulk hierarchy")?;

            if let Some(parent) = parent_id {
                let rel_id = uuid::Uuid::new_v4().to_string();
                let rel_props = serde_json::json!({"order": order}).to_string();
                tx.execute(
                    "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'has_child', ?4, 1, ?5, ?6)",
                    libsql::params![rel_id, parent.clone(), id.clone(), rel_props, now.clone(), now.clone()],
                ).await.context("Failed to insert relationship in bulk hierarchy")?;
            }
        }

        tx.commit()
            .await
            .context("Failed to commit bulk hierarchy")?;
        // Release the writer before the notify loop — it reads and fans out
        // events, and must not keep other writers queued while it does.
        drop(db);

        let ids: Vec<String> = nodes.into_iter().map(|(id, ..)| id).collect();

        // `_in_tx` twin below reuses this same insert logic against the
        // caller's transaction — see its own doc comment.

        // Notify for each created node
        for id in &ids {
            if let Ok(Some(node)) = self.get_node(id).await {
                self.notify(StoreChange {
                    operation: StoreOperation::Created,
                    node,
                    source: None,
                    previous_node: None,
                    playbook_context: None,
                });
            }
        }

        Ok(ids)
    }

    /// `_in_tx` twin of [`Self::bulk_create_hierarchy`] (ADR-069 §1a/S3).
    /// Identical insert logic, run against the caller's `tx` instead of
    /// opening its own — this is what lets `handle_create_schema`'s
    /// description-subtree creation (via `create_description_subtree`,
    /// which calls this) land in the same transaction as the schema node
    /// and its relationship declarations. Returns the ids without
    /// notifying; the caller buffers the equivalent `Created` events.
    pub(crate) async fn bulk_create_hierarchy_in_tx(
        &self,
        tx: &Tx<'_>,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        let now = Utc::now().to_rfc3339();

        for (id, node_type, content, parent_id, order, properties) in &nodes {
            self.validate_node_type(node_type)?;

            let properties = if properties.is_null() {
                serde_json::json!({})
            } else {
                properties.clone()
            };
            let props_json =
                serde_json::to_string(&properties).context("Failed to serialize properties")?;

            let title =
                Self::compute_title_for_bulk_insert(node_type, parent_id.as_deref(), content);

            tx.conn().execute(
                "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, ?5, 'active', 1, ?6, ?7)",
                libsql::params![id.clone(), node_type.clone(), content.clone(), props_json, title, now.clone(), now.clone()],
            ).await.context("Failed to insert node in bulk hierarchy")?;

            if let Some(parent) = parent_id {
                let rel_id = uuid::Uuid::new_v4().to_string();
                let rel_props = serde_json::json!({"order": order}).to_string();
                tx.conn().execute(
                    "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'has_child', ?4, 1, ?5, ?6)",
                    libsql::params![rel_id, parent.clone(), id.clone(), rel_props, now.clone(), now.clone()],
                ).await.context("Failed to insert relationship in bulk hierarchy")?;
            }
        }

        Ok(nodes.into_iter().map(|(id, ..)| id).collect())
    }

    pub async fn bulk_create_hierarchy_root_notify(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
        root_ids: Vec<String>,
    ) -> Result<Vec<String>> {
        let created = self.bulk_create_hierarchy(nodes).await?;

        // Create stale embedding markers for roots
        self.create_stale_embedding_markers_bulk(&root_ids).await?;

        Ok(created)
    }

    pub async fn create_node_streaming(
        &self,
        id: String,
        node_type: String,
        content: String,
        parent_id: Option<String>,
        order: f64,
        properties: serde_json::Value,
    ) -> Result<String> {
        self.validate_node_type(&node_type)?;

        let properties = if properties.is_null() {
            serde_json::json!({})
        } else {
            properties
        };
        let props_json =
            serde_json::to_string(&properties).context("Failed to serialize properties")?;
        let now = Utc::now().to_rfc3339();

        // One guard across the node insert and its parent edge, so no other
        // writer can observe (or transactionally absorb) the half-built pair.
        let db = self.write().await;
        db.execute(
            "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, NULL, 'active', 1, ?5, ?6)",
            libsql::params![id.clone(), node_type.clone(), content.clone(), props_json, now.clone(), now.clone()],
        ).await.context("Failed to create node (streaming)")?;

        if let Some(ref parent) = parent_id {
            let rel_id = uuid::Uuid::new_v4().to_string();
            let rel_props = serde_json::json!({"order": order}).to_string();
            db.execute(
                "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'has_child', ?4, 1, ?5, ?6)",
                libsql::params![rel_id, parent.clone(), id.clone(), rel_props, now.clone(), now],
            ).await.context("Failed to create relationship (streaming)")?;
        }
        drop(db);

        let node = Node {
            id: id.clone(),
            node_type: node_type.clone(),
            content,
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties,
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };
        self.notify(StoreChange {
            operation: StoreOperation::Created,
            node,
            source: Some("streaming_import".to_string()),
            previous_node: None,
            playbook_context: None,
        });

        Ok(id)
    }

    fn compute_title_for_bulk_insert(
        node_type: &str,
        parent_id: Option<&str>,
        content: &str,
    ) -> Option<String> {
        if matches!(node_type, "date" | "schema" | "checkbox") {
            None
        } else if parent_id.is_none() || matches!(node_type, "task" | "collection") {
            let stripped = crate::utils::strip_markdown(content);
            Some(stripped)
        } else {
            None
        }
    }

    pub async fn get_task_node(&self, id: &str) -> Result<Option<crate::models::TaskNode>> {
        let node = self.get_node(id).await?;
        Ok(node.and_then(|n| {
            if n.node_type != "task" {
                return None;
            }
            let props = &n.properties;
            let task_props = props.get("task").cloned().unwrap_or(serde_json::json!({}));
            Some(crate::models::TaskNode {
                id: n.id,
                node_type: n.node_type,
                content: n.content,
                version: n.version,
                created_at: n.created_at,
                modified_at: n.modified_at,
                properties: n.properties,
                status: task_props
                    .get("status")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_default(),
                priority: task_props
                    .get("priority")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok()),
                due_date: task_props
                    .get("due_date")
                    .and_then(|v| v.as_str())
                    .map(normalize_date_field),
                started_at: task_props
                    .get("started_at")
                    .and_then(|v| v.as_str())
                    .map(normalize_date_field),
                completed_at: task_props
                    .get("completed_at")
                    .and_then(|v| v.as_str())
                    .map(normalize_date_field),
            })
        }))
    }

    pub async fn update_task_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::TaskNodeUpdate,
        title: Option<String>,
    ) -> Result<crate::models::TaskNode> {
        let current = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Task node not found: {}", id))?;

        if current.version != expected_version {
            return Err(anyhow::anyhow!(
                "VersionMismatch: expected {}, got {}",
                expected_version,
                current.version
            ));
        }

        // Merge the update's task-property fields into the node's properties via the
        // shared helper (also used by the service layer to compute a title_template
        // title against the post-merge node — keeping the two in lockstep). The
        // helper is a best-effort no-op on malformed shapes; the persist path keeps
        // the stricter contract and rejects them loudly, as before this refactor.
        let mut props = current.properties.clone();
        let root = props
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Invalid properties"))?;
        if root.get("task").is_some_and(|task| !task.is_object()) {
            return Err(anyhow::anyhow!("Invalid task properties"));
        }
        update.apply_to_properties(&mut props);

        let props_json = serde_json::to_string(&props).context("Failed to serialize properties")?;
        let now = Utc::now().to_rfc3339();
        let new_version = expected_version + 1;

        let mut sql = "UPDATE node SET properties = ?1, version = ?2, modified_at = ?3".to_string();
        let mut sql_params: Vec<libsql::Value> = vec![
            libsql::Value::Text(props_json),
            libsql::Value::Integer(new_version),
            libsql::Value::Text(now),
        ];

        if let Some(ref content) = update.content {
            sql.push_str(&format!(", content = ?{}", sql_params.len() + 1));
            sql_params.push(libsql::Value::Text(content.clone()));
        }

        // Persist the recomputed title (when the caller determined content changed)
        // in the same UPDATE as content, so a task's indexed `title` column never
        // goes stale relative to its content.
        if let Some(ref new_title) = title {
            sql.push_str(&format!(", title = ?{}", sql_params.len() + 1));
            sql_params.push(libsql::Value::Text(new_title.clone()));
        }

        sql.push_str(&format!(" WHERE id = ?{}", sql_params.len() + 1));
        sql_params.push(libsql::Value::Text(id.to_string()));

        self.write()
            .await
            .execute(&sql, sql_params)
            .await
            .context("Failed to update task node")?;

        self.get_task_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Task node '{}' not found after update", id))
    }

    /// Fetch a schema with `relationships` hydrated from its declaration edges
    /// in the `relationship` table. One caller-facing fetch: fields come from
    /// the node row, relationships from `get_schema_declarations` — no consumer
    /// needs to know declarations aren't stored in `properties`.
    pub async fn get_schema_node(&self, id: &str) -> Result<Option<crate::models::SchemaNode>> {
        // Drain and drop the cursor before `get_schema_declarations` checks out a
        // second reader connection — see `ReadRows` in `connections.rs`.
        let node = {
            let mut rows = self
                .read()
                .await?
                .query(
                    "SELECT * FROM node WHERE id = ?1 AND node_type = 'schema' LIMIT 1",
                    libsql::params![id.to_string()],
                )
                .await
                .context("Failed to query schema node")?;
            match rows.next().await? {
                Some(row) => Self::row_to_node(&row)?,
                None => return Ok(None),
            }
        };

        match crate::models::SchemaNode::from_node(node) {
            Ok(mut schema) => {
                schema.relationships = self.get_schema_declarations(id).await?;
                Ok(Some(schema))
            }
            Err(e) => {
                tracing::warn!("Failed to parse schema node '{}': {}", id, e);
                Ok(None)
            }
        }
    }

    /// All schemas, each with `relationships` hydrated from declaration edges
    /// (one batched query — no per-schema round trip).
    pub async fn get_all_schemas(&self) -> Result<Vec<crate::models::SchemaNode>> {
        // Drain and drop the cursor before `get_all_schema_declarations` checks out
        // a second reader connection — see `ReadRows` in `connections.rs`.
        let mut schemas = Vec::new();
        {
            let mut rows = self
                .read()
                .await?
                .query(
                    "SELECT * FROM node WHERE node_type = 'schema' ORDER BY id",
                    (),
                )
                .await
                .context("Failed to query all schema nodes")?;

            while let Some(row) = rows.next().await? {
                let node = Self::row_to_node(&row)?;
                match crate::models::SchemaNode::from_node(node) {
                    Ok(schema) => schemas.push(schema),
                    Err(e) => tracing::warn!("Skipping invalid schema node: {}", e),
                }
            }
        }

        let mut declarations = self.get_all_schema_declarations().await?;
        for schema in &mut schemas {
            if let Some(rels) = declarations.remove(&schema.id) {
                schema.relationships = rels;
            }
        }
        Ok(schemas)
    }

    pub async fn count_nodes_by_type(&self, node_type: &str) -> Result<i64> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT COUNT(*) FROM node WHERE node_type = ?1",
                libsql::params![node_type.to_string()],
            )
            .await
            .context("Failed to count nodes by type")?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| anyhow::anyhow!("No result for count"))?;
        Ok(row.get::<i64>(0).unwrap_or(0))
    }

    pub async fn query_node_ids_raw(&self, sql: &str) -> Result<Vec<String>> {
        let mut rows = self
            .read()
            .await?
            .query(sql, ())
            .await
            .context("Failed to execute node query")?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            if let Ok(id) = row.get::<String>(0) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Find an active node of `node_type` whose namespaced `field` property equals
    /// `value`, excluding `exclude_id` (used to ignore self on update).
    ///
    /// This backs a uniqueness *suggestion*, not a constraint: callers use it to
    /// surface a likely-duplicate to the user, never to reject a write. Scope is
    /// per-database (ADR-053) — two offline devices can each validly hold the same
    /// value, so email and similar fields are treated as claims, not identity keys.
    ///
    /// The query is bound to `node_type` so it rides the `idx_node_type` index and
    /// filters to `lifecycle_status = 'active'`, so an archived or deleted duplicate
    /// is not reported. Property access matches the namespaced storage shape used
    /// everywhere else: `properties.$.<node_type>.<field>`. When `case_insensitive`
    /// is set, both sides are folded with `LOWER`. The caller is responsible for
    /// skipping empty/whitespace values.
    pub async fn find_conflicting_unique(
        &self,
        node_type: &str,
        field: &str,
        value: &str,
        exclude_id: Option<&str>,
        case_insensitive: bool,
    ) -> Result<Option<String>> {
        // `node_type`/`field` are interpolated into the SQL text below (SQLite has
        // no bind-parameter form for identifiers/JSON-path segments). Every
        // current call path reaches here only after `validate_schema_field_name`
        // (behaviors/mod.rs) has already constrained a persisted field name to
        // this exact grammar, and `SqliteStore::validate_node_type` allow-lists
        // `node_type` outright — so this is defense-in-depth, not the only line
        // of defense. It is still real defense: this method is `pub` and could
        // gain a less-guarded caller later, so it re-validates rather than trust
        // an upstream check it cannot see from here.
        //
        // The grammar must be a SUPERSET of what `validate_schema_field_name`
        // permits, or a legitimately-declared field is silently invisible to its
        // own `unique` rule. That function allows a bare name OR a
        // `<namespace>:<bare>` pair (ADR-063 requires the namespaced form for any
        // field added to a CORE type, e.g. `custom:employeeId`), each segment
        // being Unicode-alphanumeric or `_` — so this check mirrors exactly that,
        // not a stricter ASCII-only, colon-less approximation of it.
        fn is_safe_segment(s: &str) -> bool {
            !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
        }
        // `node_type` is never namespaced (it's a schema id, e.g. `person`, or a
        // UUID-shaped extension-schema id — hence `-` is allowed here only).
        fn is_safe_node_type(s: &str) -> bool {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        }
        // `field` may be bare or `<namespace>:<bare>` — at most one `:`.
        fn is_safe_field(s: &str) -> bool {
            let mut parts = s.split(':');
            match (parts.next(), parts.next(), parts.next()) {
                (Some(bare), None, _) => is_safe_segment(bare),
                (Some(ns), Some(bare), None) => is_safe_segment(ns) && is_safe_segment(bare),
                _ => false,
            }
        }
        if !is_safe_node_type(node_type) || !is_safe_field(field) {
            tracing::warn!(
                node_type,
                field,
                "find_conflicting_unique: rejected a non-identifier node_type/field \
                 rather than build unsafe SQL from it; treating as no-conflict"
            );
            return Ok(None);
        }

        let extract = format!("json_extract(properties, '$.{}.{}')", node_type, field);
        let (lhs, needle) = if case_insensitive {
            // SQLite LOWER() folds ASCII only, so a value differing solely in the
            // case of a non-ASCII character would not be matched here. Acceptable:
            // this backs a suggestion (never a hard block) and the first consumer
            // (email) is effectively ASCII.
            (format!("LOWER({})", extract), value.to_lowercase())
        } else {
            (extract, value.to_string())
        };

        // An absent exclude_id compares against the empty string, which no node id
        // equals, so nothing is excluded.
        let exclude = exclude_id.unwrap_or("").to_string();

        let sql = format!(
            "SELECT id FROM node WHERE node_type = ?1 AND {} = ?2 AND id != ?3 \
             AND lifecycle_status = 'active' LIMIT 1",
            lhs
        );

        let mut rows = self
            .read()
            .await?
            .query(
                &sql,
                libsql::params![node_type.to_string(), needle, exclude],
            )
            .await
            .context("Failed to search for conflicting unique value")?;

        if let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod find_conflicting_unique_tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn bare_store() -> Result<(Arc<SqliteStore>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteStore::new(db_path).await?);
        Ok((store, temp_dir))
    }

    /// Create a person with a namespaced email property and the given lifecycle
    /// status, returning its id.
    async fn make_person(
        store: &SqliteStore,
        email: &str,
        lifecycle_status: &str,
    ) -> Result<String> {
        let mut node = Node::new(
            "person".to_string(),
            email.to_string(),
            json!({ "person": { "email": email } }),
        );
        node.lifecycle_status = lifecycle_status.to_string();
        let id = node.id.clone();
        store.create_node(node, None, None).await?;
        Ok(id)
    }

    #[tokio::test]
    async fn exact_match_is_a_conflict() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let existing = make_person(&store, "a@x.com", "active").await?;

        let hit = store
            .find_conflicting_unique("person", "email", "a@x.com", None, false)
            .await?;
        assert_eq!(hit, Some(existing));
        Ok(())
    }

    #[tokio::test]
    async fn case_insensitive_match_is_a_conflict() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let existing = make_person(&store, "a@x.com", "active").await?;

        // Different casing must still collide when case_insensitive is set.
        let hit = store
            .find_conflicting_unique("person", "email", "A@x.com", None, true)
            .await?;
        assert_eq!(hit, Some(existing));

        // ...and must NOT collide under case-sensitive comparison.
        let miss = store
            .find_conflicting_unique("person", "email", "A@x.com", None, false)
            .await?;
        assert_eq!(miss, None);
        Ok(())
    }

    #[tokio::test]
    async fn no_match_returns_none() -> Result<()> {
        let (store, _t) = bare_store().await?;
        make_person(&store, "a@x.com", "active").await?;

        let miss = store
            .find_conflicting_unique("person", "email", "b@x.com", None, true)
            .await?;
        assert_eq!(miss, None);
        Ok(())
    }

    #[tokio::test]
    async fn excluding_self_returns_none() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let existing = make_person(&store, "a@x.com", "active").await?;

        // The only node with this value is excluded, so there is no conflict.
        let miss = store
            .find_conflicting_unique("person", "email", "a@x.com", Some(&existing), false)
            .await?;
        assert_eq!(miss, None);
        Ok(())
    }

    #[tokio::test]
    async fn archived_duplicate_is_not_a_conflict() -> Result<()> {
        let (store, _t) = bare_store().await?;
        make_person(&store, "a@x.com", "archived").await?;

        // Only active nodes count toward uniqueness suggestions.
        let miss = store
            .find_conflicting_unique("person", "email", "a@x.com", None, false)
            .await?;
        assert_eq!(miss, None);
        Ok(())
    }

    #[tokio::test]
    async fn different_node_type_is_not_matched() -> Result<()> {
        let (store, _t) = bare_store().await?;
        make_person(&store, "a@x.com", "active").await?;

        // The predicate is bound to node_type, so a query for a different type
        // never matches a person's value.
        let miss = store
            .find_conflicting_unique("organization", "email", "a@x.com", None, false)
            .await?;
        assert_eq!(miss, None);
        Ok(())
    }

    /// `node_type`/`field` are spliced into the SQL text (no bind-parameter form
    /// exists for a JSON-path segment), so a malicious identifier must be
    /// rejected rather than executed. This does not error — it degrades to "no
    /// conflict found", consistent with the suggest-don't-block contract (a
    /// malformed lookup should never surface as a hard failure to the caller).
    #[tokio::test]
    async fn malicious_field_identifier_is_rejected_not_executed() -> Result<()> {
        let (store, _t) = bare_store().await?;
        make_person(&store, "a@x.com", "active").await?;

        let attempt = store
            .find_conflicting_unique(
                "person",
                "email') OR '1'='1", // would match everything if spliced unguarded
                "a@x.com",
                None,
                false,
            )
            .await?;
        assert_eq!(attempt, None, "a non-identifier field must never match");

        let attempt2 = store
            .find_conflicting_unique(
                "person'; DROP TABLE node; --",
                "email",
                "a@x.com",
                None,
                false,
            )
            .await?;
        assert_eq!(
            attempt2, None,
            "a non-identifier node_type must never match"
        );

        // And the table must still be intact and queryable afterward.
        let sane = store
            .find_conflicting_unique("person", "email", "a@x.com", None, false)
            .await?;
        assert!(
            sane.is_some(),
            "store must be unharmed by the rejected attempts"
        );
        Ok(())
    }

    /// A namespaced field name (`<namespace>:<bare>`) is exactly what ADR-063
    /// REQUIRES for any field added to a core type, and is what `unique` must
    /// support for the "declarable on schema-driven types" acceptance criterion
    /// to mean anything on a core type extension. The identifier guard must
    /// accept this shape, not just a bare name, or a legitimately-declared
    /// unique field is silently invisible to its own rule.
    #[tokio::test]
    async fn namespaced_field_name_is_accepted_and_matched() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let mut node = Node::new(
            "task".to_string(),
            "Employee record".to_string(),
            json!({ "task": { "custom:employee_id": "E-42" } }),
        );
        node.lifecycle_status = "active".to_string();
        store.create_node(node, None, None).await?;

        let hit = store
            .find_conflicting_unique("task", "custom:employee_id", "E-42", None, false)
            .await?;
        assert!(
            hit.is_some(),
            "a namespaced field name must be usable, not silently rejected \
             as if it were a malicious identifier"
        );
        Ok(())
    }
}

#[cfg(test)]
mod title_contains_stem_fallback_tests {
    use super::*;
    use crate::models::NodeQuery;
    use serde_json::json;
    use tempfile::TempDir;

    async fn bare_store() -> Result<(Arc<SqliteStore>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteStore::new(db_path).await?);
        Ok((store, temp_dir))
    }

    async fn make_task(store: &SqliteStore, title: &str) -> Result<String> {
        let mut node = Node::new("task".to_string(), title.to_string(), json!({}));
        node.title = Some(title.to_string());
        let id = node.id.clone();
        store.create_node(node, None, None).await?;
        Ok(id)
    }

    #[tokio::test]
    async fn exact_substring_still_matches_without_fallback() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let id = make_task(&store, "Buy something from grocery store").await?;

        let nodes = store
            .query_nodes(NodeQuery {
                title_contains: Some("grocery store".to_string()),
                node_type: Some("task".to_string()),
                ..Default::default()
            })
            .await?;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, id);
        Ok(())
    }

    #[tokio::test]
    async fn word_variant_falls_back_to_stem_match() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let id = make_task(&store, "Buy something from grocery store").await?;

        // "groceries" is not a substring of "grocery store" in either
        // direction — the exact match misses, so the stem fallback should
        // still find it: "groceries" and "grocery" share the stem "grocery".
        let nodes = store
            .query_nodes(NodeQuery {
                title_contains: Some("groceries".to_string()),
                node_type: Some("task".to_string()),
                ..Default::default()
            })
            .await?;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, id);
        Ok(())
    }

    #[tokio::test]
    async fn unrelated_words_with_shared_prefix_do_not_match() -> Result<()> {
        let (store, _t) = bare_store().await?;
        make_task(&store, "Put the cart in the hallway").await?;
        make_task(&store, "Read the new part number").await?;

        // The reverse-direction case is what actually exercises the fallback:
        // "cart" IS a substring of "carton" (and "part" of "party"), so those
        // queries already match on exact substring and never reach the stem
        // fallback. Querying the LONGER word against a title with only the
        // SHORTER one is what a fixed-length shared-prefix fallback would
        // false-positive on ("carton" vs "cart", "party" vs "part") —
        // stemming must not.
        for query in ["carton", "party", "ready"] {
            let nodes = store
                .query_nodes(NodeQuery {
                    title_contains: Some(query.to_string()),
                    node_type: Some("task".to_string()),
                    ..Default::default()
                })
                .await?;
            assert!(
                nodes.is_empty(),
                "query {:?} should not match an unrelated word sharing its prefix, got: {:?}",
                query,
                nodes.iter().map(|n| &n.title).collect::<Vec<_>>()
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn plural_variants_match_in_both_directions() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let id = make_task(&store, "Pack the boxes for the move").await?;

        // Singular query against plural title word.
        let nodes = store
            .query_nodes(NodeQuery {
                title_contains: Some("box".to_string()),
                node_type: Some("task".to_string()),
                ..Default::default()
            })
            .await?;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, id);
        Ok(())
    }

    #[tokio::test]
    async fn fallback_respects_node_type_filter() -> Result<()> {
        let (store, _t) = bare_store().await?;
        make_task(&store, "Buy something from grocery store").await?;

        // Same title-word overlap, but scoped to a node_type with no matches.
        let nodes = store
            .query_nodes(NodeQuery {
                title_contains: Some("groceries".to_string()),
                node_type: Some("text".to_string()),
                ..Default::default()
            })
            .await?;
        assert!(nodes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn genuinely_unrelated_query_returns_empty() -> Result<()> {
        let (store, _t) = bare_store().await?;
        make_task(&store, "Buy something from grocery store").await?;

        let nodes = store
            .query_nodes(NodeQuery {
                title_contains: Some("quarterly report".to_string()),
                node_type: Some("task".to_string()),
                ..Default::default()
            })
            .await?;
        assert!(nodes.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn multi_token_matches_are_ranked_by_match_count_before_limit() -> Result<()> {
        let (store, _t) = bare_store().await?;
        // Matches only the low-signal "buy" token.
        make_task(&store, "Buy new running shoes").await?;
        // Matches both "buy" and a stem of "tickets".
        let best_id = make_task(&store, "Buy tickets for the concert").await?;
        // Matches only "buy" too.
        make_task(&store, "Buy a book for the trip").await?;

        // With limit=1, the OR-matching fallback would (pre-ranking) return an
        // arbitrary one of three "buy"-matching tasks in table order. Ranking
        // by distinct-query-token match count must put the task matching BOTH
        // query words first, so a `limit`-truncated result still surfaces it.
        let nodes = store
            .query_nodes(NodeQuery {
                title_contains: Some("buy tickets".to_string()),
                node_type: Some("task".to_string()),
                limit: Some(1),
                ..Default::default()
            })
            .await?;
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes[0].id, best_id,
            "the task matching both query tokens must rank first, not be truncated out"
        );
        Ok(())
    }

    #[test]
    fn stem_word_true_positives() {
        assert_eq!(SqliteStore::stem_word("groceries"), "grocery");
        assert_eq!(SqliteStore::stem_word("grocery"), "grocery");
        assert_eq!(SqliteStore::stem_word("tasks"), "task");
        assert_eq!(SqliteStore::stem_word("boxes"), "box");
        assert_eq!(SqliteStore::stem_word("box"), "box");
    }

    #[test]
    fn stem_word_does_not_over_strip_unrelated_words() {
        // These must NOT stem to the same value as a shorter unrelated word —
        // the false positives a fixed-length shared-prefix match produced.
        assert_ne!(
            SqliteStore::stem_word("carton"),
            SqliteStore::stem_word("cart")
        );
        assert_ne!(
            SqliteStore::stem_word("party"),
            SqliteStore::stem_word("part")
        );
        assert_ne!(
            SqliteStore::stem_word("ready"),
            SqliteStore::stem_word("read")
        );
        assert_ne!(SqliteStore::stem_word("iss"), SqliteStore::stem_word("is"));
    }

    #[test]
    fn stem_word_known_gaps_are_false_negatives_not_false_positives() {
        // Documented limitation: consonant-doubling gerunds and silent-e
        // plurals don't stem equal. This is intentional — a missed match is
        // safer than a spurious one for a fallback matcher.
        assert_ne!(
            SqliteStore::stem_word("running"),
            SqliteStore::stem_word("run")
        );
        assert_ne!(
            SqliteStore::stem_word("dates"),
            SqliteStore::stem_word("date")
        );
    }
}

#[cfg(test)]
mod query_nodes_order_and_scoping_tests {
    use super::*;
    use crate::models::{NodeQuery, OrderBy};
    use serde_json::json;
    use tempfile::TempDir;

    async fn bare_store() -> Result<(Arc<SqliteStore>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteStore::new(db_path).await?);
        Ok((store, temp_dir))
    }

    /// Insert a node with an explicit `created_at`, overriding `Node::new`'s
    /// `Utc::now()` so tests get deterministic, well-separated timestamps
    /// instead of racing the clock's resolution.
    async fn make_node_at(
        store: &SqliteStore,
        node_type: &str,
        content: &str,
        created_at: DateTime<Utc>,
    ) -> Result<String> {
        let mut node = Node::new(node_type.to_string(), content.to_string(), json!({}));
        node.created_at = created_at;
        let id = node.id.clone();
        store.create_node(node, None, None).await?;
        Ok(id)
    }

    /// Regression test for the bug where `query_nodes` applied LIMIT/OFFSET
    /// over an unordered `SELECT * FROM node` result set: `order_by` was
    /// accepted on `NodeFilter`/`NodeQuery` but never turned into a SQL
    /// `ORDER BY`, so pagination sliced an effectively arbitrary subset and
    /// could return different rows (or the same rows reordered) across two
    /// otherwise-identical calls. Twelve nodes get explicit, strictly
    /// increasing `created_at` timestamps; a correct `CreatedDesc` query must
    /// therefore return them newest-first, with adjacent pages tiling the
    /// full set exactly once each, and repeated calls must agree with each
    /// other and with that ground truth — not just with themselves.
    #[tokio::test]
    async fn order_by_created_desc_yields_stable_correctly_ordered_pagination() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let base = Utc::now();

        let mut ascending_ids = Vec::new();
        for i in 0..12i64 {
            let id = make_node_at(
                &store,
                "task",
                &format!("Task {}", i),
                base + chrono::Duration::seconds(i),
            )
            .await?;
            ascending_ids.push(id);
        }
        // Newest-first is the reverse of insertion order, since each node got
        // a strictly later `created_at` than the one before it.
        let mut expected_desc_ids = ascending_ids.clone();
        expected_desc_ids.reverse();

        let query = |limit: usize, offset: usize| NodeQuery {
            node_type: Some("task".to_string()),
            order_by: Some(OrderBy::CreatedDesc),
            limit: Some(limit),
            offset: Some(offset),
            ..Default::default()
        };

        let page1 = store.query_nodes(query(6, 0)).await?;
        let page2 = store.query_nodes(query(6, 6)).await?;
        let page1_ids: Vec<String> = page1.iter().map(|n| n.id.clone()).collect();
        let page2_ids: Vec<String> = page2.iter().map(|n| n.id.clone()).collect();

        assert_eq!(
            page1_ids,
            expected_desc_ids[0..6].to_vec(),
            "page 1 must be the 6 newest nodes, newest first"
        );
        assert_eq!(
            page2_ids,
            expected_desc_ids[6..12].to_vec(),
            "page 2 must be the 6 oldest nodes, still newest-first within the page"
        );

        // The two pages tiled together, in order, must reconstruct the full
        // CreatedDesc ordering exactly — no row repeated across the page
        // boundary and none skipped, which is exactly what an unordered
        // LIMIT/OFFSET could get wrong.
        let mut combined = page1_ids.clone();
        combined.extend(page2_ids.clone());
        assert_eq!(combined, expected_desc_ids);

        // Repeating the identical query (same filter, same page) multiple
        // times must return the identical rows in the identical order every
        // time — this is the actual "stable pagination" property, not just
        // that a single call is internally ordered.
        for attempt in 0..3 {
            let repeat = store.query_nodes(query(6, 6)).await?;
            let repeat_ids: Vec<String> = repeat.iter().map(|n| n.id.clone()).collect();
            assert_eq!(
                repeat_ids, page2_ids,
                "attempt {attempt}: repeated identical query must return the identical page"
            );
        }

        Ok(())
    }

    /// Without `order_by`, behavior is unchanged (store-defined order) —
    /// this just pins that a `None` order_by doesn't panic or otherwise
    /// misbehave now that the field flows all the way to SQL.
    #[tokio::test]
    async fn no_order_by_still_returns_all_matching_rows() -> Result<()> {
        let (store, _t) = bare_store().await?;
        for i in 0..5 {
            make_node_at(&store, "task", &format!("Task {}", i), Utc::now()).await?;
        }
        let nodes = store
            .query_nodes(NodeQuery {
                node_type: Some("task".to_string()),
                ..Default::default()
            })
            .await?;
        assert_eq!(nodes.len(), 5);
        Ok(())
    }

    /// Regression test: the `content_contains` branch used to return early
    /// with a bare `SELECT * FROM node WHERE LOWER(content) LIKE ?1`,
    /// dropping `node_type` (and `ids`/`title_contains`) entirely — so a
    /// content search "scoped" to one node type actually matched content in
    /// every type. Two node types share the same content substring; a query
    /// scoped to `task` must return only the task matches.
    #[tokio::test]
    async fn content_contains_combines_with_node_type_via_and() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let now = Utc::now();
        let task_a = make_node_at(&store, "task", "schedule the team meeting", now).await?;
        let task_b = make_node_at(&store, "task", "reschedule the client meeting", now).await?;
        // Same content substring, different node_type — must NOT leak into a
        // `node_type: "task"`-scoped content search.
        make_node_at(&store, "text", "notes from last week's meeting", now).await?;
        make_node_at(&store, "collection", "meeting rooms", now).await?;

        let nodes = store
            .query_nodes(NodeQuery {
                content_contains: Some("meeting".to_string()),
                node_type: Some("task".to_string()),
                ..Default::default()
            })
            .await?;

        let ids: std::collections::HashSet<String> = nodes.into_iter().map(|n| n.id).collect();
        assert_eq!(
            ids,
            [task_a, task_b].into_iter().collect(),
            "content_contains scoped to node_type=task must return only task matches, \
             not matches from every type"
        );
        Ok(())
    }

    /// Regression test: the same dropped-filter bug for `ids` scoping — a
    /// content search restricted to an explicit id set (e.g. a collection's
    /// members) must not return content matches from nodes OUTSIDE that set.
    /// Also exercises `node_type` + `ids` + `content_contains` combined,
    /// matching how `ops::query_nodes` composes a collection-scoped,
    /// type-filtered content search in practice.
    #[tokio::test]
    async fn content_contains_combines_with_ids_and_node_type_via_and() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let now = Utc::now();
        // In scope: task, in the id set, matches content.
        let in_scope = make_node_at(&store, "task", "urgent: fix the meeting bug", now).await?;
        // Out of scope by id: same type, same content match, NOT in the id set.
        make_node_at(&store, "task", "urgent: another meeting bug", now).await?;
        // Out of scope by type: in the id set, but wrong node_type.
        let out_of_scope_by_type =
            make_node_at(&store, "text", "urgent: meeting bug notes", now).await?;

        let nodes = store
            .query_nodes(NodeQuery {
                content_contains: Some("meeting bug".to_string()),
                node_type: Some("task".to_string()),
                ids: Some(vec![in_scope.clone(), out_of_scope_by_type.clone()]),
                ..Default::default()
            })
            .await?;

        let ids: Vec<String> = nodes.into_iter().map(|n| n.id).collect();
        assert_eq!(
            ids,
            vec![in_scope],
            "content_contains + node_type + ids must AND together: only the node that \
             satisfies content, type, AND id-scope should be returned"
        );
        Ok(())
    }

    /// Regression test: `content_contains` must also compose with
    /// `title_contains` rather than the title condition being silently
    /// dropped once a content filter is present.
    #[tokio::test]
    async fn content_contains_combines_with_title_contains_via_and() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let now = Utc::now();
        let mut matching = Node::new(
            "task".to_string(),
            "notes about the quarterly meeting".to_string(),
            json!({}),
        );
        matching.title = Some("Quarterly Planning".to_string());
        matching.created_at = now;
        let matching_id = matching.id.clone();
        store.create_node(matching, None, None).await?;

        // Same content match, different title — must be excluded once
        // title_contains is also part of the query.
        let mut wrong_title = Node::new(
            "task".to_string(),
            "notes about the quarterly meeting".to_string(),
            json!({}),
        );
        wrong_title.title = Some("Random Grocery List".to_string());
        wrong_title.created_at = now;
        store.create_node(wrong_title, None, None).await?;

        let nodes = store
            .query_nodes(NodeQuery {
                content_contains: Some("quarterly meeting".to_string()),
                title_contains: Some("Quarterly Planning".to_string()),
                ..Default::default()
            })
            .await?;

        let ids: Vec<String> = nodes.into_iter().map(|n| n.id).collect();
        assert_eq!(ids, vec![matching_id]);
        Ok(())
    }

    /// `count_nodes` must agree with `query_nodes(...).len()` for the same
    /// filter across every priority tier (scalar AND-chain, `ids` scoping,
    /// `mentioned_by`) — it shares `build_scalar_conditions` with
    /// `query_nodes` specifically so the two can't drift apart.
    #[tokio::test]
    async fn count_nodes_agrees_with_query_nodes_len_across_filter_shapes() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let now = Utc::now();

        let task_a = make_node_at(&store, "task", "schedule the team meeting", now).await?;
        let _task_b = make_node_at(&store, "task", "reschedule the client meeting", now).await?;
        let text_c = make_node_at(&store, "text", "notes from the meeting", now).await?;
        make_node_at(&store, "collection", "meeting rooms", now).await?;

        // Plain node_type filter.
        let by_type = NodeQuery {
            node_type: Some("task".to_string()),
            ..Default::default()
        };
        assert_eq!(
            store.count_nodes(&by_type).await?,
            store.query_nodes(by_type).await?.len() as i64
        );

        // content_contains + node_type AND-chain.
        let content_and_type = NodeQuery {
            content_contains: Some("meeting".to_string()),
            node_type: Some("task".to_string()),
            ..Default::default()
        };
        let count = store.count_nodes(&content_and_type).await?;
        assert_eq!(count, 2, "only the two tasks match content AND type");
        assert_eq!(
            count,
            store.query_nodes(content_and_type).await?.len() as i64
        );

        // ids scoping, combined with a scalar condition.
        let ids_scoped = NodeQuery {
            content_contains: Some("meeting".to_string()),
            ids: Some(vec![task_a.clone(), text_c.clone()]),
            ..Default::default()
        };
        assert_eq!(
            store.count_nodes(&ids_scoped).await?,
            store.query_nodes(ids_scoped).await?.len() as i64
        );

        // Empty ids set: count_nodes must short-circuit to 0, mirroring
        // query_nodes's "empty id set ⇒ no rows can match".
        let empty_ids = NodeQuery {
            ids: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(store.count_nodes(&empty_ids).await?, 0);

        // mentioned_by (join-based path, bypasses build_scalar_conditions
        // entirely).
        let mentioner = Node::new(
            "text".to_string(),
            "mentions someone".to_string(),
            json!({}),
        );
        let mentioner_id = mentioner.id.clone();
        store.create_node(mentioner, None, None).await?;
        // mentioned_by semantics (see query_nodes's mentioned_by branch):
        // `in_node` is the returned/target node, `out_node` is the "by" node.
        store
            .create_generic_relationship(&task_a, &mentioner_id, "mentions", &json!({}))
            .await?;
        let mentioned_by_query = NodeQuery {
            mentioned_by: Some(mentioner_id.clone()),
            ..Default::default()
        };
        assert_eq!(
            store.count_nodes(&mentioned_by_query).await?,
            1,
            "task_a is the only node mentioned by mentioner_id"
        );
        assert_eq!(
            store.count_nodes(&mentioned_by_query).await?,
            store.query_nodes(mentioned_by_query).await?.len() as i64
        );

        // Query with no conditions at all must count every row in the table —
        // agreeing with an unfiltered query_nodes.
        let all = NodeQuery::default();
        assert_eq!(
            store.count_nodes(&all).await?,
            store.query_nodes(all).await?.len() as i64
        );

        Ok(())
    }

    /// `count_roots` must agree with `get_roots(None, None).await?.len()` —
    /// they share the same `WHERE` clause by construction, but this pins the
    /// contract so the two can never silently diverge.
    #[tokio::test]
    async fn count_roots_agrees_with_get_roots_len() -> Result<()> {
        let (store, _t) = bare_store().await?;

        assert_eq!(store.count_roots().await?, 0, "a fresh store has no nodes");

        let root_a = store
            .create_node(
                Node::new("text".to_string(), "root a".to_string(), json!({})),
                None,
                None,
            )
            .await?;
        store
            .create_node(
                Node::new("text".to_string(), "root b".to_string(), json!({})),
                None,
                None,
            )
            .await?;
        let child = store
            .create_node(
                Node::new("text".to_string(), "child of a".to_string(), json!({})),
                None,
                None,
            )
            .await?;
        // has_child semantics (see get_children/get_roots): `in_node` is the
        // parent, `out_node` is the child.
        store
            .create_generic_relationship(&root_a.id, &child.id, "has_child", &json!({"order": 0.0}))
            .await?;

        let roots = store.get_roots(None, None).await?;
        assert_eq!(roots.len(), 2, "child should not count as a root");
        assert_eq!(store.count_roots().await?, roots.len() as i64);
        Ok(())
    }
}

#[cfg(test)]
mod query_nodes_wildcard_type_tests {
    use super::*;
    use crate::models::NodeQuery;
    use serde_json::json;
    use tempfile::TempDir;

    async fn bare_store() -> Result<(Arc<SqliteStore>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteStore::new(db_path).await?);
        Ok((store, temp_dir))
    }

    async fn make_node(store: &SqliteStore, node_type: &str, content: &str) -> Result<String> {
        let node = Node::new(node_type.to_string(), content.to_string(), json!({}));
        let id = node.id.clone();
        store.create_node(node, None, None).await?;
        Ok(id)
    }

    /// A saved query's `node_type: "*"` must return nodes of every type, not
    /// zero rows — the same "all types" meaning `QueryService::build_query`
    /// already gives `target_type == "*"` for the agent-facing query engine.
    #[tokio::test]
    async fn wildcard_node_type_returns_every_type() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let task_id = make_node(&store, "task", "Buy milk").await?;
        let text_id = make_node(&store, "text", "Some note").await?;
        let date_id = make_node(&store, "date", "2026-08-27").await?;

        let nodes = store
            .query_nodes(NodeQuery {
                node_type: Some("*".to_string()),
                limit: Some(50),
                ..Default::default()
            })
            .await?;

        let ids: std::collections::HashSet<_> = nodes.iter().map(|n| n.id.clone()).collect();
        assert!(ids.contains(&task_id));
        assert!(ids.contains(&text_id));
        assert!(ids.contains(&date_id));
        Ok(())
    }

    /// A concrete type still filters normally — the wildcard skip must not
    /// leak into the ordinary, non-"*" path.
    #[tokio::test]
    async fn concrete_node_type_still_filters() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let task_id = make_node(&store, "task", "Buy milk").await?;
        make_node(&store, "text", "Some note").await?;

        let nodes = store
            .query_nodes(NodeQuery {
                node_type: Some("task".to_string()),
                limit: Some(50),
                ..Default::default()
            })
            .await?;

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, task_id);
        Ok(())
    }
}

/// Regression coverage for a subtree whose descendant count crosses SQLite's
/// compiled `SQLITE_MAX_VARIABLE_NUMBER` (32766, verified against the vendored
/// `libsql-ffi` build — see the `ID_CHUNK` comments throughout this module).
/// Below the fix, `delete_subtree_atomic` and `get_subtree_with_relationships`
/// each built one `IN (...)` with a bound parameter per subtree id; past the
/// ceiling the statement fails to *prepare* (not merely to bind), so the whole
/// operation aborted before touching a single row — a subtree that large was
/// permanently undeletable and unopenable.
///
/// **Gated behind `RUN_LONG_TESTS=1`** (the env var `rust:test:long` already
/// wires up) and skipped otherwise: creating 30k+ nodes and deleting them
/// again runs each `node` INSERT/DELETE through the `node_fts` FTS5 sync
/// triggers (`v001_initial_schema`), which dominates the cost at this row
/// count — measured ~90-390s per test depending on machine load, far past
/// what belongs in the default `cargo test` / `bun run test:all` / pre-push
/// path. Run explicitly before merging a change to this chunking:
/// `RUN_LONG_TESTS=1 cargo test --lib -p nodespace-core large_subtree_chunking_tests -- --nocapture`.
#[cfg(test)]
mod large_subtree_chunking_tests {
    use super::*;
    use tempfile::TempDir;

    /// Skip unless explicitly opted in — see the module doc comment.
    macro_rules! require_long_tests {
        () => {
            if std::env::var("RUN_LONG_TESTS").is_err() {
                eprintln!(
                    "skipping {}: set RUN_LONG_TESTS=1 to run (creates 30k+ nodes; slow)",
                    module_path!()
                );
                return Ok(());
            }
        };
    }

    async fn bare_store() -> Result<(Arc<SqliteStore>, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteStore::new(db_path).await?);
        Ok((store, temp_dir))
    }

    /// One past SQLite's compiled ceiling, so a single unchunked `IN (...)`
    /// over `root + descendants` (32767 + 1 = 32768 bound params) cannot
    /// possibly succeed — proving the fix actually chunks rather than merely
    /// happening to stay under the line by coincidence of test size.
    const DESCENDANT_COUNT: usize = 32_767;

    /// Create `root_id` plus `count` flat `has_child` descendants directly
    /// with large multi-row `INSERT`s, bypassing the store's node-by-node
    /// `bulk_create_hierarchy` (too slow at this scale for a test — it does
    /// two round trips per node plus a `get_node` per id for notification).
    /// All descendants are direct children of the root: this is the same
    /// "wide" shape `import dir` produces for a large flat folder, and it
    /// keeps every recursive CTE well under its own depth cap (`depth < 100`)
    /// while still exercising the exact `IN (...)` chunking under test.
    async fn seed_flat_subtree(
        store: &SqliteStore,
        root_id: &str,
        count: usize,
    ) -> Result<Vec<String>> {
        let now = Utc::now().to_rfc3339();
        let child_ids: Vec<String> = (0..count)
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();

        // created_at/modified_at and the root id are embedded as SQL literals
        // rather than bound params — they're constant across every row, and
        // both are values this test generates itself (a UUID and an RFC3339
        // timestamp), never external input, so literal-embedding carries no
        // injection risk. Only the per-row id varies, which keeps the bound
        // parameter count low enough to insert thousands of rows per
        // statement instead of one per round trip.
        const SEED_CHUNK: usize = 5_000;

        for chunk in child_ids.chunks(SEED_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len())
                .map(|i| format!("(?{i}, 'text', '', '{{}}', NULL, 'active', 1, '{now}', '{now}')"))
                .collect();
            let sql = format!(
                "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) VALUES {}",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            store
                .write()
                .await
                .execute(&sql, params)
                .await
                .context("Failed to seed subtree node chunk")?;
        }

        for chunk in child_ids.chunks(SEED_CHUNK) {
            // `relationship.id` has a `DEFAULT (lower(hex(randomblob(16))))` —
            // omitting it from the column list lets SQLite mint it, so only
            // `out_node` needs a bound param per row.
            let placeholders: Vec<String> = (1..=chunk.len())
                .map(|i| format!("('{root_id}', ?{i}, 'has_child', '{{}}', 1, '{now}', '{now}')"))
                .collect();
            let sql = format!(
                "INSERT INTO relationship (in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES {}",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            store
                .write()
                .await
                .execute(&sql, params)
                .await
                .context("Failed to seed subtree relationship chunk")?;
        }

        Ok(child_ids)
    }

    #[tokio::test]
    async fn get_subtree_with_relationships_survives_over_ceiling_descendant_count() -> Result<()> {
        require_long_tests!();
        let (store, _t) = bare_store().await?;
        let root = store
            .create_node(
                Node::new(
                    "text".to_string(),
                    "root".to_string(),
                    serde_json::json!({}),
                ),
                None,
                None,
            )
            .await?;
        let child_ids = seed_flat_subtree(&store, &root.id, DESCENDANT_COUNT).await?;

        // Pre-fix, this failed to prepare the unchunked `SELECT * FROM node
        // WHERE id IN (...)` once `descendant_ids.len()` (root + descendants)
        // crossed 32766 — surfacing as "Failed to fetch subtree nodes" and
        // making the subtree unopenable.
        let (all_nodes, relationships) = store.get_subtree_with_relationships(&root.id).await?;

        assert_eq!(
            all_nodes.len(),
            DESCENDANT_COUNT + 1,
            "root + every descendant"
        );
        assert_eq!(
            relationships.len(),
            DESCENDANT_COUNT,
            "one has_child edge per descendant"
        );
        let returned_ids: HashSet<String> = all_nodes.iter().map(|n| n.id.clone()).collect();
        assert!(returned_ids.contains(&root.id));
        for id in &child_ids {
            assert!(
                returned_ids.contains(id),
                "descendant {id} missing from subtree load"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn delete_subtree_atomic_survives_over_ceiling_descendant_count() -> Result<()> {
        require_long_tests!();
        let (store, _t) = bare_store().await?;
        let root = store
            .create_node(
                Node::new(
                    "text".to_string(),
                    "root".to_string(),
                    serde_json::json!({}),
                ),
                None,
                None,
            )
            .await?;
        let child_ids = seed_flat_subtree(&store, &root.id, DESCENDANT_COUNT).await?;

        let subtree_ids = store.collect_subtree_ids(&root.id).await?;
        assert_eq!(subtree_ids.len(), DESCENDANT_COUNT + 1);

        // Pre-fix, both the pre-delete fetch and the DELETE itself built one
        // unchunked `IN (...)` over `subtree_ids` — past the ceiling neither
        // statement could even prepare, and the whole call failed with
        // "Failed to fetch subtree nodes for deletion" / "Failed to delete
        // subtree nodes" without deleting anything.
        let (existed, deleted_nodes) = store
            .delete_subtree_atomic(&root.id, root.version, &subtree_ids, None)
            .await?;

        assert!(existed);
        assert_eq!(deleted_nodes.len(), DESCENDANT_COUNT + 1);

        // The whole subtree — root and every descendant — is actually gone,
        // not partially deleted by a chunk loop that ran outside the
        // transaction and aborted partway through.
        assert!(store.get_node(&root.id).await?.is_none());
        for id in [
            &child_ids[0],
            &child_ids[child_ids.len() / 2],
            &child_ids[child_ids.len() - 1],
        ] {
            assert!(
                !store.node_exists(id).await?,
                "descendant {id} survived the delete"
            );
        }

        // Chunking the DELETE didn't split it out of the transaction that
        // holds the OCC re-check: a second delete attempt against the same
        // (now-nonexistent) target is the documented idempotent no-op, not a
        // version-conflict error, which is only possible if the very first
        // call's chunks committed together as one unit.
        let (existed_again, _) = store
            .delete_subtree_atomic(&root.id, root.version, &subtree_ids, None)
            .await?;
        assert!(!existed_again);

        Ok(())
    }

    /// Twice the production `ID_CHUNK` (900); the id list is `root +
    /// MULTI_CHUNK_COUNT` (1801), which `.chunks(900)` splits into 3 (900,
    /// 900, 1) — big enough to actually exercise the chunk-loop accumulation
    /// across more than one chunk (merge-across-chunks, not just a single
    /// chunk that happens to hold everything), nowhere near SQLite's 32766
    /// ceiling.
    /// Unlike `DESCENDANT_COUNT` above, this runs unconditionally in the
    /// default suite: the two ceiling-proving tests are the only ones slow
    /// enough to need `RUN_LONG_TESTS`, and a bug in the loop-and-merge logic
    /// itself (wrong accumulator, an off-by-one on the last chunk, a chunk's
    /// results overwriting instead of extending) would otherwise ship
    /// undetected by every test that runs by default.
    const MULTI_CHUNK_COUNT: usize = 1_800;

    #[tokio::test]
    async fn get_subtree_with_relationships_merges_results_across_multiple_chunks() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let root = store
            .create_node(
                Node::new(
                    "text".to_string(),
                    "root".to_string(),
                    serde_json::json!({}),
                ),
                None,
                None,
            )
            .await?;
        let child_ids = seed_flat_subtree(&store, &root.id, MULTI_CHUNK_COUNT).await?;

        let (all_nodes, relationships) = store.get_subtree_with_relationships(&root.id).await?;

        assert_eq!(
            all_nodes.len(),
            MULTI_CHUNK_COUNT + 1,
            "root + every descendant"
        );
        assert_eq!(relationships.len(), MULTI_CHUNK_COUNT);
        let returned_ids: HashSet<String> = all_nodes.iter().map(|n| n.id.clone()).collect();
        for id in &child_ids {
            assert!(
                returned_ids.contains(id),
                "descendant {id} missing — a chunk's results were dropped, not merged"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn delete_subtree_atomic_deletes_across_multiple_chunks() -> Result<()> {
        let (store, _t) = bare_store().await?;
        let root = store
            .create_node(
                Node::new(
                    "text".to_string(),
                    "root".to_string(),
                    serde_json::json!({}),
                ),
                None,
                None,
            )
            .await?;
        let child_ids = seed_flat_subtree(&store, &root.id, MULTI_CHUNK_COUNT).await?;
        let subtree_ids = store.collect_subtree_ids(&root.id).await?;
        assert_eq!(subtree_ids.len(), MULTI_CHUNK_COUNT + 1);

        let (existed, deleted_nodes) = store
            .delete_subtree_atomic(&root.id, root.version, &subtree_ids, None)
            .await?;

        assert!(existed);
        assert_eq!(deleted_nodes.len(), MULTI_CHUNK_COUNT + 1);
        assert!(store.get_node(&root.id).await?.is_none());
        for id in &child_ids {
            assert!(
                !store.node_exists(id).await?,
                "descendant {id} survived the delete — a chunk's DELETE was skipped or its \
                 result silently dropped"
            );
        }
        Ok(())
    }
}
