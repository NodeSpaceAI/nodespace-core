//! `SqliteStore` methods — relationships concern (split from the god-object per ADR-053 prep).
use super::*;
use crate::models::schema::{SchemaRelationship, BUILTIN_RELATIONSHIP_NAMES};

/// SQL fragment excluding the built-in structural relationship types, for
/// queries that must see only schema-declared relationships. One definition so
/// every declaration query scopes identically.
fn builtin_exclusion_sql(column: &str) -> String {
    let quoted: Vec<String> = BUILTIN_RELATIONSHIP_NAMES
        .iter()
        .map(|n| format!("'{}'", n))
        .collect();
    format!("{} NOT IN ({})", column, quoted.join(", "))
}

/// The outcome of replacing a schema's relationship declarations, itemized so
/// the service layer can emit one domain event per actual change (created /
/// updated / deleted edge). Unchanged declarations keep their row and id and
/// appear in none of the lists.
#[derive(Debug, Default)]
pub struct SchemaDeclarationChanges {
    /// (relationship row id, out_node, declaration) for newly inserted edges.
    pub created: Vec<(String, String, SchemaRelationship)>,
    /// (relationship row id, out_node, declaration) for edges whose stored
    /// declaration (properties and/or target endpoint) was rewritten in place.
    pub updated: Vec<(String, String, SchemaRelationship)>,
    /// (relationship row id, out_node, relationship name) for removed edges.
    pub deleted: Vec<(String, String, String)>,
}

/// Resolves incoming `mentions` sources to their containers: a single query,
/// seeding one row per mentioning source and walking `has_child` upward from
/// each seed in lockstep (one recursive step per depth level, not per source)
/// until each branch runs out of parents. `top` keeps only the deepest row per
/// source (its root), unless the source is itself a task/ai-chat node, which
/// is its own container. Takes one bound parameter (`?1`, the target node id).
///
/// IMPORTANT: the recursive step is written as a correlated subquery
/// (`(SELECT r.in_node FROM relationship r WHERE r.out_node = w.nid AND
/// r.relationship_type = 'has_child')`) rather than a `JOIN ... WHERE
/// r.relationship_type = 'has_child'`. The join form lets SQLite pick
/// `idx_rel_type` (relationship_type) as the driving index and scan every
/// `has_child` edge in the database per recursive step; the correlated
/// subquery form forces the `out_node = ?` lookup to use `idx_rel_out
/// (out_node, relationship_type)` instead, which is the difference between an
/// 8ms query and a 393ms query on a database with ~31k `has_child` edges. Do
/// not "simplify" this back into a join — the query-plan regression test in
/// this file's `#[cfg(test)] mod tests` binds this exact constant and fails if
/// the plan ever falls back to `idx_rel_type`.
const MENTION_CONTAINERS_QUERY: &str = r#"WITH RECURSIVE
    src(sid) AS (
        SELECT DISTINCT in_node FROM relationship
        WHERE out_node = ?1 AND relationship_type = 'mentions'
    ),
    walk(sid, nid, d) AS (
        SELECT sid, sid, 0 FROM src
        UNION ALL
        SELECT w.sid,
               (SELECT r.in_node FROM relationship r
                WHERE r.out_node = w.nid AND r.relationship_type = 'has_child'),
               w.d + 1
        FROM walk w
        WHERE w.d < 100
          AND EXISTS (SELECT 1 FROM relationship r
                      WHERE r.out_node = w.nid AND r.relationship_type = 'has_child')
    ),
    top AS (
        SELECT sid, nid, ROW_NUMBER() OVER (PARTITION BY sid ORDER BY d DESC) AS rn
        FROM walk
    )
SELECT DISTINCT
    CASE WHEN (SELECT node_type FROM node WHERE id = t.sid) IN ('task', 'ai-chat')
         THEN t.sid ELSE t.nid END AS container_id
FROM top t WHERE t.rn = 1"#;

impl SqliteStore {
    pub async fn create_mention(&self, source_id: &str, target_id: &str) -> Result<Option<String>> {
        // Guard spans the existence check and the insert: without it two
        // concurrent calls both see "no mention" and both insert.
        let db = self.write().await;
        let mut rows = db.query(
            "SELECT id FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'mentions'",
            libsql::params![source_id.to_string(), target_id.to_string()],
        ).await.context("Failed to check for existing mention")?;

        if rows.next().await?.is_some() {
            return Ok(None);
        }

        let rel_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        db.execute(
            "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'mentions', '{}', 1, ?4, ?5)",
            libsql::params![rel_id.clone(), source_id.to_string(), target_id.to_string(), now.clone(), now],
        ).await.context("Failed to create mention")?;

        Ok(Some(rel_id))
    }

    pub async fn delete_mention(&self, source_id: &str, target_id: &str) -> Result<Option<String>> {
        // Guard spans the id lookup and the delete, so the id reported as
        // deleted is the row this call actually removed.
        let db = self.write().await;
        let mut rows = db.query(
            "SELECT id FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'mentions'",
            libsql::params![source_id.to_string(), target_id.to_string()],
        ).await.context("Failed to get mention id")?;

        let existing_id: Option<String> = if let Some(row) = rows.next().await? {
            Some(row.get(0)?)
        } else {
            None
        };

        db.execute(
            "DELETE FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'mentions'",
            libsql::params![source_id.to_string(), target_id.to_string()],
        ).await.context("Failed to delete mention")?;

        Ok(existing_id.map(|id| format!("relationship:{}", id)))
    }

    pub async fn get_outgoing_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        let mut rows = match self.read().await?.query(
            "SELECT out_node FROM relationship WHERE in_node = ?1 AND relationship_type = 'mentions'",
            libsql::params![node_id.to_string()],
        ).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Failed to query outgoing mentions for {}: {}", node_id, e);
                return Ok(Vec::new());
            }
        };

        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }

    pub async fn get_incoming_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        let mut rows = self.read().await?.query(
            "SELECT in_node FROM relationship WHERE out_node = ?1 AND relationship_type = 'mentions'",
            libsql::params![node_id.to_string()],
        ).await.context("Failed to get incoming mentions")?;

        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }

    pub async fn get_incoming_mention_containers(
        &self,
        node_id: &str,
    ) -> Result<Vec<crate::models::NodeReference>> {
        let start = std::time::Instant::now();

        // Scoped so the cursor (and the reader connection it owns — see
        // `ReadConn::query`'s doc comment) drops before the batch container
        // fetch below checks out a second reader connection.
        let mut container_ids: HashSet<String> = HashSet::new();
        {
            let mut rows = self
                .read()
                .await?
                .query(
                    MENTION_CONTAINERS_QUERY,
                    libsql::params![node_id.to_string()],
                )
                .await
                .context("Failed to resolve mentioning containers")?;

            while let Some(row) = rows.next().await? {
                container_ids.insert(row.get(0)?);
            }
        }

        if container_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Batch fetch containers. Chunk under SQLite's compiled
        // SQLITE_MAX_VARIABLE_NUMBER (32766) — a node mentioned from a very large
        // number of sources would otherwise blow the ceiling in one `IN (...)`.
        const ID_CHUNK: usize = 900;
        let container_id_list: Vec<String> = container_ids.into_iter().collect();
        let mut result = Vec::new();
        for chunk in container_id_list.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "SELECT id, title, node_type FROM node WHERE id IN ({})",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            let mut container_rows = self
                .read()
                .await?
                .query(&sql, params)
                .await
                .context("Failed to fetch containers")?;

            while let Some(row) = container_rows.next().await? {
                result.push(crate::models::NodeReference {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    node_type: row.get(2)?,
                });
            }
        }

        tracing::debug!(
            "get_incoming_mention_containers: {} containers in {:?} for node_id={}",
            result.len(),
            start.elapsed(),
            node_id
        );

        Ok(result)
    }

    async fn get_next_order_for_relationship(
        &self,
        node_id: &str,
        relationship_type: &str,
        use_out_as_anchor: bool,
    ) -> Result<f64> {
        let anchor_field = if use_out_as_anchor {
            "out_node"
        } else {
            "in_node"
        };
        let sql = format!(
            "SELECT json_extract(properties, '$.order') as ord FROM relationship WHERE {} = ?1 AND relationship_type = ?2 ORDER BY json_extract(properties, '$.order') DESC LIMIT 1",
            anchor_field
        );

        let mut rows = self
            .read()
            .await?
            .query(
                &sql,
                libsql::params![node_id.to_string(), relationship_type.to_string()],
            )
            .await
            .context("Failed to get last order for relationship")?;

        // presence-not-sign — append after the max sibling even when its
        // order is <= 0 (a `> 0.0` sentinel misreads a lone child at 0.0 as none).
        let last_order: Option<f64> = if let Some(row) = rows.next().await? {
            Some(row.get::<Option<f64>>(0)?.unwrap_or(0.0))
        } else {
            None
        };

        Ok(FractionalOrderCalculator::calculate_order(last_order, None))
    }

    pub async fn get_next_member_order(&self, collection_id: &str) -> Result<f64> {
        self.get_next_order_for_relationship(collection_id, "member_of", true)
            .await
    }

    /// Append a `has_child` edge from `parent_id` to `child_id`, assigning the
    /// next fractional sibling order — reading the current max order, computing
    /// the next key, and inserting the edge as ONE atomic unit under the store's
    /// write guard. The ordered `has_child` analogue of `add_to_collection`'s
    /// `member_of` path.
    ///
    /// Splitting the read from the write (a separate "get next order" call
    /// followed by a generic relationship insert) let a concurrent reorder
    /// interleave between them, so two appends computed the same order key
    /// against a now-stale max and collided. Holding the guard across the
    /// read → compute → write serializes this against `move_node`,
    /// `create_child_node_atomic`, and `move_children_to_parent`, matching their
    /// discipline. The guard scopes exactly the order read/compute/write — no
    /// unrelated async work runs under it.
    ///
    /// No re-entrancy: `get_next_order_for_relationship` reads through the
    /// read-only connection and the raw INSERT uses this guard, so nothing
    /// re-acquires it.
    ///
    /// Uses `INSERT OR IGNORE` (matching `create_generic_relationship`) so a
    /// duplicate edge — already guarded by the caller's existence check — is a
    /// benign no-op rather than a unique-index error. Returns the assigned order
    /// and the new relationship id.
    pub async fn append_child_edge(
        &self,
        parent_id: &str,
        child_id: &str,
    ) -> Result<(f64, String)> {
        let db = self.write().await;

        let new_order = self
            .get_next_order_for_relationship(parent_id, "has_child", false)
            .await?;

        let rel_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let props = serde_json::json!({ "order": new_order }).to_string();
        db.execute(
            "INSERT OR IGNORE INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'has_child', ?4, 1, ?5, ?6)",
            libsql::params![rel_id.clone(), parent_id.to_string(), child_id.to_string(), props, now.clone(), now],
        ).await.context("Failed to insert has_child edge")?;

        Ok((new_order, rel_id))
    }

    /// ADR-059 §2 — a content node may hold a `member_of` edge only when it is a
    /// **root** node (no `has_child` parent). Collections (nesting) and person
    /// nodes (grantee membership, ADR-037 §4) are exempt. Enforced at the store's
    /// three `member_of` INSERT sites (`add_to_collection`,
    /// `bulk_add_to_collections`, and the generic `create_generic_relationship`
    /// when its `rel_type` is `member_of`), so every write path is covered without
    /// a per-path check: CLI, graph import, play `add_relationship`, and the
    /// sync-apply cold-sweep (which calls `bulk_add_to_collections` directly). A
    /// batched, chunked query keeps the bulk/cold-sweep path a single round trip.
    /// Members that don't exist yet are left to the INSERT's foreign-key check.
    async fn assert_root_only_membership(&self, member_ids: &[&str]) -> Result<()> {
        if member_ids.is_empty() {
            return Ok(());
        }
        let mut unique: Vec<&str> = member_ids.to_vec();
        unique.sort_unstable();
        unique.dedup();

        // Chunk the `IN (...)` under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766).
        const ID_CHUNK: usize = 900;
        for chunk in unique.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "SELECT n.id, n.node_type, \
                 EXISTS(SELECT 1 FROM relationship r \
                        WHERE r.out_node = n.id AND r.relationship_type = 'has_child') \
                 FROM node n WHERE n.id IN ({})",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.to_string()))
                .collect();
            let mut rows = self
                .read()
                .await?
                .query(&sql, params)
                .await
                .context("Failed to validate root-only membership")?;
            while let Some(row) = rows.next().await? {
                let id: String = row.get(0)?;
                let node_type: String = row.get(1)?;
                let has_parent: i64 = row.get(2)?;
                if has_parent != 0 && node_type != "collection" && node_type != "person" {
                    return Err(anyhow::anyhow!(
                        "member_of_not_root: content node '{}' (type '{}') has a parent, so it cannot be a member of a collection directly — file its root node instead",
                        id,
                        node_type
                    ));
                }
            }
        }
        Ok(())
    }

    pub async fn add_to_collection(
        &self,
        member_id: &str,
        collection_id: &str,
        edge_data: &serde_json::Value,
    ) -> Result<Option<(String, serde_json::Value)>> {
        self.assert_root_only_membership(&[member_id]).await?;

        // A collection filed into another collection can close a hierarchy cycle
        // (A member_of B + B member_of A), corrupting the member walk into a loop.
        // The per-row service path guards this; guard the raw store path too so
        // direct callers (e.g. markdown import) are covered without relying on the
        // service. Content/person members have no member_of descendants and can't
        // cycle, so the check is limited to collection-typed members.
        if self.get_node_type(member_id).await?.as_deref() == Some("collection") {
            self.validate_no_member_of_cycle(member_id, collection_id)
                .await?;
        }

        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin add_to_collection transaction")?;

        let mut rows = tx
            .query(
                "SELECT id FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'member_of' LIMIT 1",
                libsql::params![member_id.to_string(), collection_id.to_string()],
            )
            .await
            .context("Failed to check existing membership")?;

        if rows.next().await?.is_some() {
            return Ok(None);
        }

        let mut order_rows = tx
            .query(
                "SELECT json_extract(properties, '$.order') as ord FROM relationship WHERE out_node = ?1 AND relationship_type = 'member_of' ORDER BY json_extract(properties, '$.order') DESC LIMIT 1",
                libsql::params![collection_id.to_string()],
            )
            .await
            .context("Failed to get last member order")?;

        // presence-not-sign — append after the max member even at order <= 0.
        let last_order: Option<f64> = if let Some(row) = order_rows.next().await? {
            Some(row.get::<Option<f64>>(0)?.unwrap_or(0.0))
        } else {
            None
        };
        let new_order = FractionalOrderCalculator::calculate_order(last_order, None);

        let mut props_obj = match edge_data {
            serde_json::Value::Object(map) => map.clone(),
            serde_json::Value::Null => serde_json::Map::new(),
            other => {
                // Every current caller passes an object or `json!({})`; a
                // non-object, non-null edge_data is a caller bug. Coerce to an
                // empty map rather than failing the membership write, but log
                // it — silent data loss here would otherwise be invisible.
                tracing::debug!(
                    "add_to_collection: edge_data for member_of({member_id} -> {collection_id}) \
                     was neither an object nor null ({other}); ignoring it"
                );
                serde_json::Map::new()
            }
        };
        props_obj.insert("order".to_string(), serde_json::json!(new_order));
        let merged_props = serde_json::Value::Object(props_obj);

        let rel_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let props = merged_props.to_string();

        tx.execute(
            "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'member_of', ?4, 1, ?5, ?6)",
            libsql::params![rel_id.clone(), member_id.to_string(), collection_id.to_string(), props, now.clone(), now],
        )
        .await
        .context("Failed to add to collection")?;

        tx.commit()
            .await
            .context("Failed to commit add_to_collection")?;

        Ok(Some((rel_id, merged_props)))
    }

    pub async fn remove_from_collection(
        &self,
        member_id: &str,
        collection_id: &str,
    ) -> Result<Option<String>> {
        // Guard spans the id lookup and the delete, so the id reported as
        // removed is the row this call actually removed.
        let db = self.write().await;
        let mut rows = db.query(
            "SELECT id FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'member_of'",
            libsql::params![member_id.to_string(), collection_id.to_string()],
        ).await.context("Failed to get membership ID")?;

        let existing_id: Option<String> = if let Some(row) = rows.next().await? {
            Some(row.get(0)?)
        } else {
            None
        };

        db.execute(
            "DELETE FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'member_of'",
            libsql::params![member_id.to_string(), collection_id.to_string()],
        ).await.context("Failed to remove from collection")?;

        Ok(existing_id.map(|id| format!("relationship:{}", id)))
    }

    pub async fn get_node_memberships(&self, node_id: &str) -> Result<Vec<String>> {
        let mut rows = self.read().await?.query(
            "SELECT out_node FROM relationship WHERE in_node = ?1 AND relationship_type = 'member_of'",
            libsql::params![node_id.to_string()],
        ).await.context("Failed to get node memberships")?;

        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }

    /// Every `member_of` edge belonging to a node that is a member of MORE THAN
    /// ONE collection, as `(member_id, collection_id)` pairs. A node's first
    /// membership rides its atomic cloud node insert, so only these SECONDARY
    /// memberships need a separate push; the cloud-sync membership sweep uses this
    /// to converge them. Bounding to multi-membership nodes keeps the sweep cheap
    /// — the single-membership majority (already covered atomically) is never
    /// enumerated. Idempotent re-push of the one atomic-covered edge among a
    /// multi-membership node's edges is a benign no-op on the cloud side.
    ///
    /// `person` members are excluded: their collection memberships are RBAC state
    /// managed server-side by the membership RPCs (invite / set_member) and are
    /// not the sweep's concern — re-asserting them would bypass those gates and
    /// waste round-trips. Content and nested-collection memberships are kept.
    pub async fn get_multi_membership_edges(&self) -> Result<Vec<(String, String)>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT r.in_node, r.out_node FROM relationship r \
                 JOIN node n ON n.id = r.in_node \
                 WHERE r.relationship_type = 'member_of' \
                   AND n.node_type != 'person' \
                   AND r.in_node IN ( \
                     SELECT in_node FROM relationship \
                     WHERE relationship_type = 'member_of' \
                     GROUP BY in_node HAVING COUNT(*) > 1 \
                   )",
                (),
            )
            .await
            .context("Failed to get multi-membership edges")?;

        let mut edges = Vec::new();
        while let Some(row) = rows.next().await? {
            let member: String = row.get(0)?;
            let collection: String = row.get(1)?;
            edges.push((member, collection));
        }
        Ok(edges)
    }

    pub async fn get_collection_members(&self, collection_id: &str) -> Result<Vec<Node>> {
        let start = std::time::Instant::now();

        let mut rows = self.read().await?.query(
            "SELECT n.* FROM node n JOIN relationship r ON r.in_node = n.id WHERE r.out_node = ?1 AND r.relationship_type = 'member_of' ORDER BY json_extract(r.properties, '$.order') ASC",
            libsql::params![collection_id.to_string()],
        ).await.context("Failed to get collection members")?;

        let mut nodes = Vec::new();
        while let Some(row) = rows.next().await? {
            nodes.push(Self::row_to_node(&row)?);
        }

        tracing::debug!(
            "get_collection_members: {:?} for {} nodes",
            start.elapsed(),
            nodes.len()
        );

        Ok(nodes)
    }

    /// Restricted to `lifecycle_status = 'active'`, matching the pattern
    /// `find_conflicting_unique` established for the schema-declared `unique`
    /// mechanism (ADR-065): an archived collection no longer holds its name,
    /// so archiving one and creating a new collection with the same name is a
    /// legitimate way to free up that name rather than a collision. Callers
    /// that specifically need to look up an archived/deleted collection must
    /// query the `node` table directly rather than use this method.
    pub async fn get_collection_by_name(&self, name: &str) -> Result<Option<Node>> {
        let normalized = name.to_lowercase();

        // Drain and drop the cursor before `get_node` checks out a second reader
        // connection — see `ReadRows` in `connections.rs`.
        let id: Option<String> = {
            let mut rows = self
                .read()
                .await?
                .query(
                    "SELECT id FROM node WHERE node_type = 'collection' AND LOWER(title) = ?1 \
                 AND lifecycle_status = 'active' LIMIT 1",
                    libsql::params![normalized],
                )
                .await
                .context("Failed to search for collection by name")?;
            match rows.next().await? {
                Some(row) => Some(row.get(0)?),
                None => None,
            }
        };

        match id {
            Some(id) => self.get_node(&id).await,
            None => Ok(None),
        }
    }

    pub async fn get_collections_by_names(
        &self,
        names: &[String],
    ) -> Result<HashMap<String, Node>> {
        if names.is_empty() {
            return Ok(HashMap::new());
        }

        let normalized: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
        let placeholders: Vec<String> = (1..=normalized.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "SELECT id, title FROM node WHERE node_type = 'collection' AND LOWER(title) IN ({})",
            placeholders.join(", ")
        );

        let params: Vec<libsql::Value> = normalized
            .iter()
            .map(|n| libsql::Value::Text(n.clone()))
            .collect();
        // Collect the matched (id, title) pairs and drop the cursor before the
        // per-row `get_node` calls check out a second reader connection — see
        // `ReadRows` in `connections.rs`.
        let mut matches: Vec<(String, Option<String>)> = Vec::new();
        {
            let mut rows = self
                .read()
                .await?
                .query(&sql, params)
                .await
                .context("Failed to batch search collections by names")?;

            while let Some(row) = rows.next().await? {
                matches.push((row.get(0)?, row.get(1)?));
            }
        }

        let mut collections = HashMap::new();
        for (id, title) in matches {
            if let Ok(Some(node)) = self.get_node(&id).await {
                let key = title.unwrap_or_default().to_lowercase();
                collections.insert(key, node);
            }
        }

        Ok(collections)
    }

    pub async fn get_collection_members_recursive(
        &self,
        collection_id: &str,
    ) -> Result<Vec<String>> {
        // Get all collections in the subtree using WITH RECURSIVE.
        //
        // collection hierarchy is built from `member_of` edges (a
        // sub-collection is a member_of its parent), NOT `has_child` — the old
        // recursive arm followed `has_child` and so matched nothing, leaving
        // `coll_subtree` as just the seed and silently dropping every
        // sub-collection's members. member_of stores in_node = member/child,
        // out_node = collection/parent, so we descend parent→child by joining on
        // `r.out_node = cs.node_id` and taking `r.in_node`, restricted to
        // collection children (a content member isn't a sub-collection). A depth
        // cap bounds traversal in case a cycle slips in.
        let mut rows = self
            .read()
            .await?
            .query(
                r#"WITH RECURSIVE coll_subtree(node_id, depth) AS (
                SELECT ?1, 0
                UNION ALL
                SELECT r.in_node, cs.depth + 1 FROM relationship r
                JOIN coll_subtree cs ON r.out_node = cs.node_id
                JOIN node n ON n.id = r.in_node AND n.node_type = 'collection'
                WHERE r.relationship_type = 'member_of' AND cs.depth < 100
            )
            SELECT DISTINCT r.in_node FROM relationship r
            JOIN coll_subtree cs ON r.out_node = cs.node_id
            WHERE r.relationship_type = 'member_of'"#,
                libsql::params![collection_id.to_string()],
            )
            .await
            .context("Failed to get recursive collection members")?;

        let mut member_ids: Vec<String> = Vec::new();
        while let Some(row) = rows.next().await? {
            member_ids.push(row.get(0)?);
        }

        member_ids.sort();
        member_ids.dedup();
        Ok(member_ids)
    }

    pub async fn get_all_collection_names(&self) -> Result<Vec<String>> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT content FROM node WHERE node_type = 'collection' ORDER BY content ASC",
                (),
            )
            .await
            .context("Failed to get all collection names")?;

        let mut names = Vec::new();
        while let Some(row) = rows.next().await? {
            names.push(row.get(0)?);
        }
        Ok(names)
    }

    pub async fn get_all_collections_with_member_counts(
        &self,
    ) -> Result<Vec<(Node, usize, Vec<String>)>> {
        let collections = self.get_all_collections().await?;
        if collections.is_empty() {
            return Ok(vec![]);
        }

        // Count CONTENT members per collection. This count drives the sidebar's
        // member badge AND its empty-collection pruning, so it must match what the
        // collection viewer actually lists — user-authored content only. Counting
        // raw `member_of` edges instead let a collection whose only members are the
        // person roster (or other system nodes) report a non-zero count, show in the
        // sidebar, then open empty because the viewer filters those members out.
        //
        // The excluded types mirror the frontend's NON_CONTENT_NODE_TYPES
        // (collections.svelte.ts): `person`/`schema`/`database-settings` are
        // system/definition nodes, `collection` members are shown in the tree
        // itself, and `horizontal-line` is a decorative divider. Keep the two lists
        // in sync.
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT r.out_node, COUNT(*) \
             FROM relationship r \
             JOIN node n ON n.id = r.in_node \
             WHERE r.relationship_type = 'member_of' \
               AND n.node_type NOT IN \
                 ('schema', 'person', 'database-settings', 'collection', 'horizontal-line') \
             GROUP BY r.out_node",
                (),
            )
            .await
            .context("Failed to get member counts")?;

        let mut count_map: HashMap<String, usize> = HashMap::new();
        while let Some(row) = rows.next().await? {
            let coll_id: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            count_map.insert(coll_id, count as usize);
        }

        // Get collection-to-collection hierarchy edges
        let mut rows2 = self.read().await?.query(
            "SELECT r.in_node, r.out_node FROM relationship r JOIN node n1 ON n1.id = r.in_node JOIN node n2 ON n2.id = r.out_node WHERE r.relationship_type = 'member_of' AND n1.node_type = 'collection' AND n2.node_type = 'collection'",
            (),
        ).await.context("Failed to get collection hierarchy")?;

        let mut parent_map: HashMap<String, Vec<String>> = HashMap::new();
        while let Some(row) = rows2.next().await? {
            let child: String = row.get(0)?;
            let parent: String = row.get(1)?;
            parent_map.entry(child).or_default().push(parent);
        }

        Ok(collections
            .into_iter()
            .map(|node| {
                let count = count_map.get(&node.id).copied().unwrap_or(0);
                let parents = parent_map.get(&node.id).cloned().unwrap_or_default();
                (node, count, parents)
            })
            .collect())
    }

    async fn get_all_collections(&self) -> Result<Vec<Node>> {
        self.query_nodes_from_sql(
            "SELECT * FROM node WHERE node_type = 'collection' ORDER BY content ASC",
            (),
        )
        .await
    }

    /// Bulk-create `has_child` edges (parent → child) in ONE transaction, for the
    /// sync-apply cold-sweep's batched reconnect path. Each tuple is `(parent, child,
    /// order)` where `order` is the sender's sibling order; `get_children` sorts by
    /// `json_extract(properties, '$.order')` ASC, so a fresh parent's children
    /// reproduce that order exactly.
    ///
    /// Idempotent and safe for a from-scratch sweep: a child that ALREADY has a
    /// parent `has_child` edge is skipped (a node has at most one parent), so this
    /// only ever attaches genuinely-unparented children — it never re-parents.
    /// Direction matches `move_node`/`get_children`: `in_node = parent, out_node =
    /// child`. Returns the edges actually inserted, so the caller can emit one
    /// `RelationshipCreated` per edge.
    pub async fn bulk_create_has_child(
        &self,
        edges: &[(String, String, f64)],
    ) -> Result<Vec<(String, String, f64)>> {
        if edges.is_empty() {
            return Ok(Vec::new());
        }

        // ADR-059 §2 (reparent side): this cold-sweep attach path must not give a
        // `has_child` parent to a node that holds a `member_of` edge — symmetric
        // with the forward guard on `bulk_add_to_collections`. Run before the
        // write guard is taken, since it only reads. See `assert_may_gain_parent`.
        let child_ids: Vec<&str> = edges.iter().map(|(_, child, _)| child.as_str()).collect();
        self.assert_may_gain_parent(&child_ids).await?;

        let now = Utc::now().to_rfc3339();
        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin bulk has_child transaction")?;

        let mut created = Vec::with_capacity(edges.len());
        for (parent, child, order) in edges {
            // A node has at most one parent — skip if this child is already parented
            // (idempotent re-run; never re-parent a child that landed via another path).
            let mut rows = tx
                .query(
                    "SELECT 1 FROM relationship WHERE out_node = ?1 AND relationship_type = 'has_child' LIMIT 1",
                    libsql::params![child.clone()],
                )
                .await
                .context("Failed to check existing parent edge")?;
            if rows.next().await?.is_some() {
                continue;
            }

            let rel_id = uuid::Uuid::new_v4().to_string();
            let props = serde_json::json!({ "order": order }).to_string();
            tx.execute(
                "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'has_child', ?4, 1, ?5, ?6)",
                libsql::params![rel_id, parent.clone(), child.clone(), props, now.clone(), now.clone()],
            )
            .await
            .context("Failed to insert has_child edge")?;
            created.push((parent.clone(), child.clone(), *order));
        }

        tx.commit()
            .await
            .context("Failed to commit bulk has_child")?;
        Ok(created)
    }

    /// Bulk-insert `member_of` edges, returning the edges actually created
    /// (skipping ones that already existed) as `(rel_id, node_id, collection_id,
    /// order)`. Callers that need cloud sync route through
    /// [`crate::services::NodeService::bulk_add_to_collections_notify`], which
    /// emits a `RelationshipCreated` event per returned edge — this raw store
    /// method emits nothing, so on its own the edges never push to cloud.
    /// The subset of `ids` whose node is a `collection`, chunked under SQLite's
    /// bound-parameter ceiling. Used to limit the cycle check in
    /// `bulk_add_to_collections` to members that can actually form a hierarchy.
    async fn collection_typed_ids(&self, ids: &[&str]) -> Result<HashSet<String>> {
        let mut set = HashSet::new();
        if ids.is_empty() {
            return Ok(set);
        }
        let mut unique: Vec<&str> = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        const ID_CHUNK: usize = 900;
        for chunk in unique.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "SELECT id FROM node WHERE node_type = 'collection' AND id IN ({})",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.to_string()))
                .collect();
            let mut rows = self
                .read()
                .await?
                .query(&sql, params)
                .await
                .context("Failed to fetch collection-typed members")?;
            while let Some(row) = rows.next().await? {
                set.insert(row.get::<String>(0)?);
            }
        }
        Ok(set)
    }

    /// Whether adding `member member_of collection` would make a collection a
    /// descendant of itself, evaluated against `tx` so it sees edges inserted
    /// earlier in the same transaction. Mirrors the store's
    /// `validate_no_member_of_cycle` walk but returns a bool for the bulk
    /// skip-and-continue path instead of raising.
    async fn bulk_edge_would_cycle(
        tx: &libsql::Transaction,
        member: &str,
        collection: &str,
    ) -> Result<bool> {
        if member == collection {
            return Ok(true);
        }
        let mut rows = tx
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
                libsql::params![member.to_string(), collection.to_string()],
            )
            .await
            .context("Failed to check for member_of cycle in bulk add")?;
        Ok(rows.next().await?.is_some())
    }

    pub async fn bulk_add_to_collections(
        &self,
        memberships: &[(String, String)],
    ) -> Result<Vec<(String, String, String, f64)>> {
        if memberships.is_empty() {
            return Ok(Vec::new());
        }

        let member_ids: Vec<&str> = memberships
            .iter()
            .map(|(node_id, _)| node_id.as_str())
            .collect();
        self.assert_root_only_membership(&member_ids).await?;

        // Which members are themselves collections. Only a collection can be filed
        // such that it becomes a descendant of itself (a collection-hierarchy
        // cycle) — an ordinary content member has no `member_of` descendants — so
        // the per-edge cycle check below is paid only for these. The per-row path
        // (`create_relationship`) runs this guard; this bulk path historically did
        // not, letting a cyclic collection pair be written straight through.
        let collection_members = self.collection_typed_ids(&member_ids).await?;

        let start = std::time::Instant::now();

        // Group by collection to calculate orders correctly
        let mut by_collection: HashMap<&str, Vec<&str>> = HashMap::new();
        for (node_id, collection_id) in memberships {
            by_collection
                .entry(collection_id.as_str())
                .or_default()
                .push(node_id.as_str());
        }

        // Current max `member_of` order per target collection, read in ONE
        // round trip rather than one per collection. This runs under the write
        // guard (the orders written below are computed from it), so every extra
        // round trip here is time no other writer in the daemon can use.
        //
        // Matches `get_next_order_for_relationship`'s semantics exactly:
        // `MAX()` skips NULL orders, so a collection whose edges all carry a
        // NULL order yields a group with a NULL max — the same "present but
        // unordered" case its `unwrap_or(0.0)` handles — while a collection
        // with no edges at all produces no group, the `None` case.
        let collection_ids: Vec<&str> = by_collection.keys().copied().collect();
        let mut max_order: HashMap<&str, Option<f64>> = HashMap::new();
        // Chunk under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766).
        const ID_CHUNK: usize = 900;

        let db = self.write().await;

        for chunk in collection_ids.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "SELECT out_node, MAX(json_extract(properties, '$.order')) FROM relationship \
                 WHERE out_node IN ({}) AND relationship_type = 'member_of' \
                 GROUP BY out_node",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.to_string()))
                .collect();
            let mut rows = db
                .query(&sql, params)
                .await
                .context("Failed to read existing member orders")?;
            while let Some(row) = rows.next().await? {
                let collection_id: String = row.get(0)?;
                let ord: Option<f64> = row.get(1)?;
                // Re-borrow the caller's `&str` key so the map stays borrow-free.
                if let Some(key) = chunk.iter().find(|id| **id == collection_id) {
                    max_order.insert(key, ord);
                }
            }
        }

        let mut ordered: Vec<(String, String, f64)> = Vec::with_capacity(memberships.len());
        for (collection_id, node_ids) in &by_collection {
            // Present-with-NULL-order counts as 0.0; absent stays None, so an
            // empty collection starts a fresh sequence.
            let last_order = max_order.get(collection_id).map(|ord| ord.unwrap_or(0.0));
            let base_order = FractionalOrderCalculator::calculate_order(last_order, None);
            for (i, node_id) in node_ids.iter().enumerate() {
                ordered.push((
                    node_id.to_string(),
                    collection_id.to_string(),
                    base_order + i as f64,
                ));
            }
        }

        let now = Utc::now().to_rfc3339();
        let tx = db
            .transaction()
            .await
            .context("Failed to begin bulk add transaction")?;

        let mut created: Vec<(String, String, String, f64)> = Vec::new();
        for (node_id, collection_id, order) in &ordered {
            // Skip a collection-membership edge that would make a collection a
            // descendant of itself. Checked against the transaction so it also
            // catches a cycle closed by an edge inserted earlier in this same
            // batch, not only one already committed. Skipping (rather than failing
            // the batch) is the correct resolution: the cyclic edge must never
            // land, while the batch's valid edges still persist.
            if collection_members.contains(node_id.as_str())
                && Self::bulk_edge_would_cycle(&tx, node_id, collection_id).await?
            {
                tracing::warn!(
                    member = %node_id,
                    collection = %collection_id,
                    "bulk_add_to_collections: skipping member_of edge that would create a collection-hierarchy cycle"
                );
                continue;
            }

            let mut rows = tx.query(
                "SELECT id FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'member_of' LIMIT 1",
                libsql::params![node_id.clone(), collection_id.clone()],
            ).await.context("Failed to check existing membership")?;

            if rows.next().await?.is_some() {
                continue;
            }

            let rel_id = uuid::Uuid::new_v4().to_string();
            let props = serde_json::json!({"order": order}).to_string();
            tx.execute(
                "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'member_of', ?4, 1, ?5, ?6)",
                libsql::params![rel_id.clone(), node_id.clone(), collection_id.clone(), props, now.clone(), now.clone()],
            ).await.context("Failed to insert membership")?;
            created.push((rel_id, node_id.clone(), collection_id.clone(), *order));
        }

        tx.commit().await.context("Failed to commit bulk add")?;

        tracing::debug!(
            "bulk_add_to_collections: {} memberships in {:?}",
            created.len(),
            start.elapsed()
        );

        Ok(created)
    }

    pub async fn bulk_create_mentions(&self, mentions: &[(String, String)]) -> Result<usize> {
        if mentions.is_empty() {
            return Ok(0);
        }

        let start = std::time::Instant::now();
        let candidate: Vec<_> = mentions.iter().filter(|(s, t)| s != t).collect();
        let candidate_len = candidate.len();
        if candidate_len == 0 {
            return Ok(0);
        }

        // a mention edge is FK-constrained — `relationship.in_node` /
        // `out_node` are `NOT NULL REFERENCES node(id)` with `PRAGMA foreign_keys
        // = ON` — so a mention to a NON-EXISTENT node (a dangling `[[link]]` to a
        // doc that wasn't imported, a typo, or an external ref) is an FK violation
        // on INSERT. Previously all mentions were inserted in ONE transaction with
        // `?`-propagation, so a single dangling link rolled back the WHOLE batch
        // and the import created ZERO cross-references. Pre-filter to pairs whose
        // BOTH endpoints exist; dangling links are skipped (and logged), never
        // fatal.
        let mut endpoints: std::collections::HashSet<String> = std::collections::HashSet::new();
        for m in &candidate {
            endpoints.insert(m.0.clone());
            endpoints.insert(m.1.clone());
        }
        let endpoint_ids: Vec<String> = endpoints.into_iter().collect();
        // Chunk the `IN (...)` under SQLite's compiled SQLITE_MAX_VARIABLE_NUMBER (32766).
        const ID_CHUNK: usize = 900;
        let mut existing: std::collections::HashSet<String> = std::collections::HashSet::new();
        for chunk in endpoint_ids.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "SELECT id FROM node WHERE id IN ({})",
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
                .context("Failed to check mention endpoints")?;
            while let Some(row) = rows.next().await? {
                let id: String = row.get(0)?;
                existing.insert(id);
            }
        }

        let valid_mentions: Vec<_> = candidate
            .into_iter()
            .filter(|m| existing.contains(m.0.as_str()) && existing.contains(m.1.as_str()))
            .collect();
        let skipped = candidate_len - valid_mentions.len();
        if skipped > 0 {
            tracing::warn!(
                "bulk_create_mentions: skipped {} mention(s) with a missing endpoint node (dangling [[link]]); keeping {} valid",
                skipped,
                valid_mentions.len()
            );
        }
        if valid_mentions.is_empty() {
            return Ok(0);
        }

        let now = Utc::now().to_rfc3339();
        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin bulk mentions transaction")?;

        let mut created = 0;
        for (source_id, target_id) in &valid_mentions {
            let mut rows = tx.query(
                "SELECT id FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = 'mentions' LIMIT 1",
                libsql::params![source_id.clone().to_string(), target_id.clone().to_string()],
            ).await.context("Failed to check existing mention")?;

            if rows.next().await?.is_some() {
                continue;
            }

            let rel_id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, 'mentions', '{}', 1, ?4, ?5)",
                libsql::params![rel_id, source_id.clone().to_string(), target_id.clone().to_string(), now.clone(), now.clone()],
            ).await.context("Failed to insert mention")?;
            created += 1;
        }

        tx.commit()
            .await
            .context("Failed to commit bulk mentions")?;

        tracing::debug!(
            "bulk_create_mentions: {} mentions in {:?}",
            created,
            start.elapsed()
        );

        Ok(created)
    }

    pub async fn check_relationship_exists(&self, source_id: &str, rel_type: &str) -> Result<i64> {
        let mut rows = self.read().await?.query(
            "SELECT COUNT(*) as cnt FROM relationship WHERE in_node = ?1 AND relationship_type = ?2",
            libsql::params![source_id.to_string(), rel_type.to_string()],
        ).await.context("Failed to check relationship existence")?;
        let row = rows
            .next()
            .await
            .context("No row returned")?
            .ok_or_else(|| anyhow::anyhow!("Empty result for relationship count"))?;
        Ok(row.get::<i64>(0).unwrap_or(0))
    }

    pub async fn relationship_exists(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &str,
    ) -> Result<bool> {
        let mut rows = self.read().await?.query(
            "SELECT 1 FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = ?3 LIMIT 1",
            libsql::params![source_id.to_string(), target_id.to_string(), rel_type.to_string()],
        ).await.context("Failed to check relationship existence")?;
        Ok(rows.next().await?.is_some())
    }

    pub async fn create_generic_relationship(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &str,
        properties: &serde_json::Value,
    ) -> Result<String> {
        // ADR-059 §2 applies to a `member_of` edge no matter which API builds it.
        // This generic path carries `member_of` whenever a caller supplies an
        // explicit `order` — `NodeService::create_relationship`'s non-auto-order
        // fork, the play `add_relationship` action, and the CLI
        // `relationship create --edge-data` — so it must be gated too. (Auto-order
        // `member_of` goes through `add_to_collection` instead; both are guarded.)
        if rel_type == "member_of" {
            self.assert_root_only_membership(&[source_id]).await?;
        }
        let now = chrono::Utc::now().to_rfc3339();
        let rel_id = uuid::Uuid::new_v4().to_string();
        let props_json = serde_json::to_string(properties).unwrap_or_else(|_| "{}".to_string());
        self.write().await.execute(
            "INSERT OR IGNORE INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)",
            libsql::params![rel_id.clone(), source_id.to_string(), target_id.to_string(), rel_type.to_string(), props_json, now.clone(), now],
        ).await.context("Failed to create generic relationship")?;
        Ok(rel_id)
    }

    pub async fn get_relationship_id(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &str,
    ) -> Result<Option<String>> {
        let mut rows = self.read().await?.query(
            "SELECT id FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = ?3 LIMIT 1",
            libsql::params![source_id.to_string(), target_id.to_string(), rel_type.to_string()],
        ).await.context("Failed to get relationship ID")?;
        if let Some(row) = rows.next().await? {
            Ok(Some(row.get::<String>(0)?))
        } else {
            Ok(None)
        }
    }

    /// Fetch a single relationship edge (with its properties) by endpoints and type.
    ///
    /// Complements `get_relationship_id` when the caller needs the edge's stored
    /// properties (e.g. the `role`/`status` carried on a `has_role` edge), not just
    /// its id. Returns `None` when no such edge exists.
    pub async fn get_relationship_record(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &str,
    ) -> Result<Option<RelationshipRecord>> {
        let mut rows = self.read().await?.query(
            "SELECT id, in_node, out_node, relationship_type, properties FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = ?3 LIMIT 1",
            libsql::params![source_id.to_string(), target_id.to_string(), rel_type.to_string()],
        ).await.context("Failed to get relationship record")?;
        if let Some(row) = rows.next().await? {
            Ok(Some(Self::row_to_relationship(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_generic_relationship(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &str,
    ) -> Result<()> {
        self.write().await.execute(
            "DELETE FROM relationship WHERE in_node = ?1 AND out_node = ?2 AND relationship_type = ?3",
            libsql::params![source_id.to_string(), target_id.to_string(), rel_type.to_string()],
        ).await.context("Failed to delete relationship")?;
        Ok(())
    }

    /// Replace the stored `properties` JSON on a single relationship edge.
    ///
    /// Targets the edge identified by (`source_id` = `in_node`, `target_id` =
    /// `out_node`, `rel_type`) and overwrites its `properties` column wholesale
    /// (edge attributes are a small self-contained blob; callers that want a
    /// merge read-modify-write at a higher layer). Bumps `version` and
    /// `modified_at` so the change participates in sync like any other edge
    /// write. Returns the edge id when a row was updated, or `None` when no such
    /// edge exists (so the caller can surface a not-found rather than silently
    /// succeeding on a no-op UPDATE).
    pub async fn update_relationship_properties(
        &self,
        source_id: &str,
        target_id: &str,
        rel_type: &str,
        properties: &serde_json::Value,
    ) -> Result<Option<String>> {
        let rel_id = self
            .get_relationship_id(source_id, target_id, rel_type)
            .await?;
        let Some(rel_id) = rel_id else {
            return Ok(None);
        };
        let now = chrono::Utc::now().to_rfc3339();
        let props_json = serde_json::to_string(properties).unwrap_or_else(|_| "{}".to_string());
        self.write()
            .await
            .execute(
                "UPDATE relationship SET properties = ?1, version = version + 1, modified_at = ?2 WHERE id = ?3",
                libsql::params![props_json, now, rel_id.clone()],
            )
            .await
            .context("Failed to update relationship properties")?;
        Ok(Some(rel_id))
    }

    pub async fn get_nodes_by_relationship(
        &self,
        node_id: &str,
        rel_type: &str,
        direction: &str,
    ) -> Result<Vec<Node>> {
        let sql = match direction {
            "out" => "SELECT n.* FROM node n JOIN relationship r ON r.out_node = n.id WHERE r.in_node = ?1 AND r.relationship_type = ?2",
            "in" => "SELECT n.* FROM node n JOIN relationship r ON r.in_node = n.id WHERE r.out_node = ?1 AND r.relationship_type = ?2",
            _ => return Err(anyhow::anyhow!("Invalid direction: {}", direction)),
        };
        let mut rows = self
            .read()
            .await?
            .query(
                sql,
                libsql::params![node_id.to_string(), rel_type.to_string()],
            )
            .await
            .context("Failed to get related nodes")?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next().await? {
            nodes.push(Self::row_to_node(&row)?);
        }
        Ok(nodes)
    }

    /// Like [`get_nodes_by_relationship`](Self::get_nodes_by_relationship), but
    /// returns each edge's stored `properties` JSON alongside the related node.
    ///
    /// The plain `get_nodes_by_relationship` selects only node columns (`SELECT
    /// n.*`) and drops the edge data on the `relationship` row. The relationship
    /// viewer needs the edge attributes — a Task→Person
    /// `assigned_to` edge might carry `role`/`assigned_at` — so this also selects
    /// `r.properties`. `n.*` expands to the ten `node` columns (indices 0-9,
    /// matching [`row_to_node`](Self::row_to_node)); the trailing `r.properties`
    /// is column 10. Returns `(node, edge_properties)` pairs.
    pub async fn get_related_nodes_with_edges(
        &self,
        node_id: &str,
        rel_type: &str,
        direction: &str,
    ) -> Result<Vec<(Node, serde_json::Value)>> {
        let sql = match direction {
            "out" => "SELECT n.*, r.properties FROM node n JOIN relationship r ON r.out_node = n.id WHERE r.in_node = ?1 AND r.relationship_type = ?2",
            "in" => "SELECT n.*, r.properties FROM node n JOIN relationship r ON r.in_node = n.id WHERE r.out_node = ?1 AND r.relationship_type = ?2",
            _ => return Err(anyhow::anyhow!("Invalid direction: {}", direction)),
        };
        let mut rows = self
            .read()
            .await?
            .query(
                sql,
                libsql::params![node_id.to_string(), rel_type.to_string()],
            )
            .await
            .context("Failed to get related nodes with edges")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().await? {
            let node = Self::row_to_node(&row)?;
            let props_str: String = row.get(10)?;
            let properties: serde_json::Value =
                serde_json::from_str(&props_str).unwrap_or_else(|_| serde_json::json!({}));
            out.push((node, properties));
        }
        Ok(out)
    }

    pub async fn get_relationship_orders(
        &self,
        node_id: &str,
        rel_type: &str,
        direction: &str,
    ) -> Result<Vec<Option<f64>>> {
        let filter_col = if direction == "in_node" {
            "in_node"
        } else {
            "out_node"
        };
        let sql = format!(
            "SELECT json_extract(properties, '$.order') as ord FROM relationship WHERE {} = ?1 AND relationship_type = ?2 ORDER BY json_extract(properties, '$.order') ASC",
            filter_col
        );
        let mut rows = self
            .read()
            .await?
            .query(
                &sql,
                libsql::params![node_id.to_string(), rel_type.to_string()],
            )
            .await
            .context("Failed to get relationship orders")?;
        let mut orders = Vec::new();
        while let Some(row) = rows.next().await? {
            let order: Option<f64> = row.get(0).ok();
            orders.push(order);
        }
        Ok(orders)
    }

    pub async fn get_relationship_count(
        &self,
        node_id: &str,
        rel_type: &str,
        direction: &str,
    ) -> Result<usize> {
        let filter_col = if direction == "in_node" {
            "in_node"
        } else {
            "out_node"
        };
        let sql = format!(
            "SELECT COUNT(*) FROM relationship WHERE {} = ?1 AND relationship_type = ?2",
            filter_col
        );
        let mut rows = self
            .read()
            .await?
            .query(
                &sql,
                libsql::params![node_id.to_string(), rel_type.to_string()],
            )
            .await
            .context("Failed to count relationships")?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| anyhow::anyhow!("No count result"))?;
        Ok(row.get::<i64>(0).unwrap_or(0) as usize)
    }

    // ========================================================================
    // Schema relationship declarations (issue: table-backed declarations)
    //
    // A declaration is a `relationship` row between two SCHEMA nodes:
    // in_node = declaring schema, out_node = target schema (or the declaring
    // schema itself when the declaration is untyped, `target_type: None`),
    // relationship_type = the declared name, and the full `SchemaRelationship`
    // serialized in the row's `properties`. The `properties` JSON is the
    // authoritative declaration; the endpoints exist for FK integrity and
    // indexed lookup. These four methods are the ONLY paths that read or write
    // declaration rows — every consumer goes through them (usually indirectly,
    // via the hydrated `SchemaNode.relationships`).
    // ========================================================================

    /// Parse a declaration row's stored `properties` into a `SchemaRelationship`.
    ///
    /// A malformed row is a bug (only `set_schema_declarations` writes these),
    /// so it is surfaced loudly via `tracing::warn` and skipped rather than
    /// silently defaulted — the historical `.ok()`-swallowing on the JSON read
    /// path is exactly what this storage model replaces.
    fn parse_declaration_row(schema_id: &str, props_json: &str) -> Option<SchemaRelationship> {
        match serde_json::from_str::<SchemaRelationship>(props_json) {
            Ok(rel) => Some(rel),
            Err(e) => {
                tracing::warn!(
                    schema = %schema_id,
                    error = %e,
                    "skipping malformed schema relationship declaration row"
                );
                None
            }
        }
    }

    /// All relationship declarations made BY `schema_id`, in declaration order.
    pub async fn get_schema_declarations(
        &self,
        schema_id: &str,
    ) -> Result<Vec<SchemaRelationship>> {
        let sql = format!(
            "SELECT properties FROM relationship \
             WHERE in_node = ?1 AND {} ORDER BY rowid",
            builtin_exclusion_sql("relationship_type")
        );
        let mut rows = self
            .read()
            .await?
            .query(&sql, libsql::params![schema_id.to_string()])
            .await
            .context("Failed to query schema relationship declarations")?;

        let mut declarations = Vec::new();
        while let Some(row) = rows.next().await? {
            let props: String = row.get(0)?;
            if let Some(rel) = Self::parse_declaration_row(schema_id, &props) {
                declarations.push(rel);
            }
        }
        Ok(declarations)
    }

    /// All relationship declarations in the database, keyed by declaring schema
    /// id. One query; used to hydrate `get_all_schemas` without an N+1.
    ///
    /// The join restricts to rows whose `in_node` is a schema node — combined
    /// with the builtin-name exclusion this is the declaration invariant (see
    /// module docs on `crate::models::schema`).
    pub async fn get_all_schema_declarations(
        &self,
    ) -> Result<HashMap<String, Vec<SchemaRelationship>>> {
        let sql = format!(
            "SELECT r.in_node, r.properties FROM relationship r \
             JOIN node s ON s.id = r.in_node \
             WHERE s.node_type = 'schema' AND {} ORDER BY r.rowid",
            builtin_exclusion_sql("r.relationship_type")
        );
        let mut rows = self
            .read()
            .await?
            .query(&sql, ())
            .await
            .context("Failed to query all schema relationship declarations")?;

        let mut by_schema: HashMap<String, Vec<SchemaRelationship>> = HashMap::new();
        while let Some(row) = rows.next().await? {
            let schema_id: String = row.get(0)?;
            let props: String = row.get(1)?;
            if let Some(rel) = Self::parse_declaration_row(&schema_id, &props) {
                by_schema.entry(schema_id).or_default().push(rel);
            }
        }
        Ok(by_schema)
    }

    /// Replace the relationship declarations of `schema_id` with `relationships`,
    /// atomically, diffing by name so unchanged declarations keep their row (no
    /// delete/reinsert churn, stable edge ids, no spurious events).
    ///
    /// The NAME invariants are enforced here, at the lowest write layer, so no
    /// direct caller (core-schema seeding, tests) can bypass them: a declared
    /// name may not collide with a built-in structural relationship (the
    /// builtin exclusion in every declaration query keys on exactly this), and
    /// names must be unique within one schema (the diff-by-name and the
    /// `relationship_type` addressing both assume it). Higher-level validation
    /// with friendlier errors (target existence, live-instance-edge
    /// protection) lives in `NodeService::set_schema_relationships`.
    pub async fn set_schema_declarations(
        &self,
        schema_id: &str,
        relationships: &[SchemaRelationship],
    ) -> Result<SchemaDeclarationChanges> {
        let mut seen_names: HashSet<&str> = HashSet::new();
        for rel in relationships {
            if crate::models::schema::is_builtin_relationship(&rel.name) {
                return Err(anyhow::anyhow!(
                    "relationship name '{}' is reserved for a built-in structural relationship",
                    rel.name
                ));
            }
            if !seen_names.insert(rel.name.as_str()) {
                return Err(anyhow::anyhow!(
                    "duplicate relationship declaration name '{}' on schema '{}'",
                    rel.name,
                    schema_id
                ));
            }
        }

        let now = Utc::now().to_rfc3339();
        let db = self.write().await;
        let tx = db
            .transaction()
            .await
            .context("Failed to begin schema declaration transaction")?;

        // Existing declaration rows, keyed by declared name.
        let select_sql = format!(
            "SELECT id, out_node, relationship_type, properties FROM relationship \
             WHERE in_node = ?1 AND {}",
            builtin_exclusion_sql("relationship_type")
        );
        let mut rows = tx
            .query(&select_sql, libsql::params![schema_id.to_string()])
            .await
            .context("Failed to read existing schema declarations")?;
        let mut existing: HashMap<String, (String, String, String)> = HashMap::new();
        while let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            let out_node: String = row.get(1)?;
            let rel_type: String = row.get(2)?;
            let props: String = row.get(3)?;
            existing.insert(rel_type, (id, out_node, props));
        }

        let mut changes = SchemaDeclarationChanges::default();

        // Upsert the new set.
        for rel in relationships {
            let out_node = rel.target_type.as_deref().unwrap_or(schema_id).to_string();
            let props_json = serde_json::to_string(rel)
                .context("Failed to serialize schema relationship declaration")?;

            match existing.remove(&rel.name) {
                Some((_, ref old_out, ref old_props))
                    if *old_out == out_node && *old_props == props_json =>
                {
                    // Unchanged — keep the row as-is.
                }
                Some((id, _, _)) => {
                    tx.execute(
                        "UPDATE relationship SET out_node = ?1, properties = ?2, \
                         version = version + 1, modified_at = ?3 WHERE id = ?4",
                        libsql::params![out_node.clone(), props_json, now.clone(), id.clone()],
                    )
                    .await
                    .context("Failed to update schema declaration edge")?;
                    changes.updated.push((id, out_node, rel.clone()));
                }
                None => {
                    let rel_id = uuid::Uuid::new_v4().to_string();
                    tx.execute(
                        "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)",
                        libsql::params![
                            rel_id.clone(),
                            schema_id.to_string(),
                            out_node.clone(),
                            rel.name.clone(),
                            props_json,
                            now.clone(),
                            now.clone()
                        ],
                    )
                    .await
                    .context("Failed to insert schema declaration edge")?;
                    changes.created.push((rel_id, out_node, rel.clone()));
                }
            }
        }

        // Whatever remains in `existing` is no longer declared — delete.
        for (name, (id, out_node, _)) in existing {
            tx.execute(
                "DELETE FROM relationship WHERE id = ?1",
                libsql::params![id.clone()],
            )
            .await
            .context("Failed to delete schema declaration edge")?;
            changes.deleted.push((id, out_node, name));
        }

        tx.commit()
            .await
            .context("Failed to commit schema declaration transaction")?;
        Ok(changes)
    }

    /// `_in_tx` twin of [`Self::set_schema_declarations`] (ADR-069 §1a/S3).
    /// Identical upsert/delete logic, run against the caller's `tx` instead
    /// of opening its own — this is what lets `handle_create_schema` and
    /// `handle_update_schema` compose the relationship-declaration write
    /// into the same transaction as the schema node and description
    /// subtree writes.
    pub(crate) async fn set_schema_declarations_in_tx(
        tx: &super::tx::Tx<'_>,
        schema_id: &str,
        relationships: &[SchemaRelationship],
    ) -> Result<SchemaDeclarationChanges> {
        let mut seen_names: HashSet<&str> = HashSet::new();
        for rel in relationships {
            if crate::models::schema::is_builtin_relationship(&rel.name) {
                return Err(anyhow::anyhow!(
                    "relationship name '{}' is reserved for a built-in structural relationship",
                    rel.name
                ));
            }
            if !seen_names.insert(rel.name.as_str()) {
                return Err(anyhow::anyhow!(
                    "duplicate relationship declaration name '{}' on schema '{}'",
                    rel.name,
                    schema_id
                ));
            }
        }

        let now = Utc::now().to_rfc3339();
        let conn = tx.conn();

        let select_sql = format!(
            "SELECT id, out_node, relationship_type, properties FROM relationship \
             WHERE in_node = ?1 AND {}",
            builtin_exclusion_sql("relationship_type")
        );
        let mut rows = conn
            .query(&select_sql, libsql::params![schema_id.to_string()])
            .await
            .context("Failed to read existing schema declarations")?;
        let mut existing: HashMap<String, (String, String, String)> = HashMap::new();
        while let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            let out_node: String = row.get(1)?;
            let rel_type: String = row.get(2)?;
            let props: String = row.get(3)?;
            existing.insert(rel_type, (id, out_node, props));
        }

        let mut changes = SchemaDeclarationChanges::default();

        for rel in relationships {
            let out_node = rel.target_type.as_deref().unwrap_or(schema_id).to_string();
            let props_json = serde_json::to_string(rel)
                .context("Failed to serialize schema relationship declaration")?;

            match existing.remove(&rel.name) {
                Some((_, ref old_out, ref old_props))
                    if *old_out == out_node && *old_props == props_json =>
                {
                    // Unchanged — keep the row as-is.
                }
                Some((id, _, _)) => {
                    conn.execute(
                        "UPDATE relationship SET out_node = ?1, properties = ?2, \
                         version = version + 1, modified_at = ?3 WHERE id = ?4",
                        libsql::params![out_node.clone(), props_json, now.clone(), id.clone()],
                    )
                    .await
                    .context("Failed to update schema declaration edge")?;
                    changes.updated.push((id, out_node, rel.clone()));
                }
                None => {
                    let rel_id = uuid::Uuid::new_v4().to_string();
                    conn.execute(
                        "INSERT INTO relationship (id, in_node, out_node, relationship_type, properties, version, created_at, modified_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)",
                        libsql::params![
                            rel_id.clone(),
                            schema_id.to_string(),
                            out_node.clone(),
                            rel.name.clone(),
                            props_json,
                            now.clone(),
                            now.clone()
                        ],
                    )
                    .await
                    .context("Failed to insert schema declaration edge")?;
                    changes.created.push((rel_id, out_node, rel.clone()));
                }
            }
        }

        for (name, (id, out_node, _)) in existing {
            conn.execute(
                "DELETE FROM relationship WHERE id = ?1",
                libsql::params![id.clone()],
            )
            .await
            .context("Failed to delete schema declaration edge")?;
            changes.deleted.push((id, out_node, name));
        }

        Ok(changes)
    }

    /// Count the INSTANCE-level edges written under a declaration: edges whose
    /// `relationship_type` is the declared name and whose source node is an
    /// instance of the declaring type. The declaring schema node itself has
    /// `node_type = 'schema'`, so declaration edges never match — this is the
    /// shared scoping rule (endpoint type, not name) that keeps declarations
    /// and instance edges apart in the one table.
    pub async fn count_instance_edges_for_declaration(
        &self,
        declaring_type: &str,
        relationship_name: &str,
    ) -> Result<i64> {
        let mut rows = self
            .read()
            .await?
            .query(
                "SELECT COUNT(*) FROM relationship r \
                 JOIN node s ON s.id = r.in_node \
                 WHERE r.relationship_type = ?1 AND s.node_type = ?2",
                libsql::params![relationship_name.to_string(), declaring_type.to_string()],
            )
            .await
            .context("Failed to count instance edges for declaration")?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| anyhow::anyhow!("No count result"))?;
        Ok(row.get::<i64>(0).unwrap_or(0))
    }

    /// Refuse to delete a schema node that still has relationship declarations
    /// pointing at or from it. `relationship.in_node`/`out_node` carry
    /// `ON DELETE CASCADE`, so an unguarded delete would silently destroy the
    /// declarations (and strand any instance edges written under them). A
    /// non-schema node passes without a query.
    pub(super) async fn assert_schema_deletable(&self, node: &Node) -> Result<()> {
        if node.node_type != "schema" {
            return Ok(());
        }
        let count = self.count_schema_declaration_edges(&node.id).await?;
        if count > 0 {
            return Err(anyhow::anyhow!(
                "schema_has_declarations: schema '{}' has {} relationship declaration(s) \
                 pointing at or from it; remove them via update_schema before deleting the schema",
                node.id,
                count
            ));
        }
        Ok(())
    }

    /// Count declaration edges touching `schema_id` on either end (declared BY
    /// it or targeting it). Used to block deleting a schema node out from under
    /// its declarations — `relationship.in_node`/`out_node` carry `ON DELETE
    /// CASCADE`, so an unguarded node delete would silently destroy them.
    pub async fn count_schema_declaration_edges(&self, schema_id: &str) -> Result<i64> {
        let sql = format!(
            "SELECT COUNT(*) FROM relationship r \
             JOIN node a ON a.id = r.in_node \
             JOIN node b ON b.id = r.out_node \
             WHERE (r.in_node = ?1 OR r.out_node = ?1) \
               AND a.node_type = 'schema' AND b.node_type = 'schema' \
               AND {}",
            builtin_exclusion_sql("r.relationship_type")
        );
        let mut rows = self
            .read()
            .await?
            .query(&sql, libsql::params![schema_id.to_string()])
            .await
            .context("Failed to count schema declaration edges")?;
        let row = rows
            .next()
            .await?
            .ok_or_else(|| anyhow::anyhow!("No count result"))?;
        Ok(row.get::<i64>(0).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Query-plan regression check for `get_incoming_mention_containers`'s
    /// recursive CTE. The recursive step must resolve via the correlated
    /// subquery form (`out_node = ? AND relationship_type = 'has_child'`
    /// inside a `SELECT`), which SQLite drives through `idx_rel_out
    /// (out_node, relationship_type)`. A `JOIN ... WHERE r.relationship_type
    /// = 'has_child'` form instead lets SQLite pick `idx_rel_type` as the
    /// driving index and scan every `has_child` edge per recursive step —
    /// measured as the difference between an 8ms and a 393ms query on a
    /// database with ~31k `has_child` edges. This test locks the query
    /// shape actually used by `get_incoming_mention_containers` so that
    /// regression can't silently return.
    #[tokio::test]
    async fn container_resolution_query_does_not_scan_idx_rel_type() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let store = SqliteStore::new(db_path).await?;

        // Binds the exact same query text `get_incoming_mention_containers`
        // runs (see `MENTION_CONTAINERS_QUERY`'s doc comment) — this can't
        // silently drift out of sync with the production query.
        let mut rows = store
            .read()
            .await?
            .query(
                &format!("EXPLAIN QUERY PLAN {MENTION_CONTAINERS_QUERY}"),
                libsql::params!["x".to_string()],
            )
            .await
            .context("Failed to run EXPLAIN QUERY PLAN")?;

        let mut detail = String::new();
        while let Some(row) = rows.next().await? {
            let d: String = row.get(3)?; // EXPLAIN QUERY PLAN: (id, parent, notused, detail)
            detail.push_str(&d);
            detail.push(' ');
        }

        assert!(
            !detail.contains("idx_rel_type"),
            "container-resolution query must not use idx_rel_type (full has_child scan); plan was: {detail}"
        );
        Ok(())
    }
}
