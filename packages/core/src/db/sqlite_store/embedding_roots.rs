//! `SqliteStore` methods — embedding roots and the access boundaries that cut
//! them (ADR-059 §6/§7).
//!
//! An embedding root is normally a tree root: its vector aggregates its whole
//! `has_child` subtree. The one exception is a descendant whose access differs
//! from its aggregating root's. Two constraints make that shape unreachable by
//! construction (ADR-059 §2): only a root may hold a `member_of` edge
//! (`assert_may_gain_parent` and the `member_of` insert guards), and a
//! collection is always a root (the `collection_is_root_*` schema triggers).
//! Finding one is therefore a defect. The descendant is kept out of the root's
//! vector and becomes its own embedding root, so authorized readers can still
//! find it by meaning.
use super::*;
use std::collections::BTreeSet;

/// Upper bound on the upward access walk, matching the cloud walk's cycle
/// backstop (`user_can_access_node` in `cloud-sync.md`).
const MAX_ACCESS_WALK_DEPTH: usize = 64;

/// Upper bound on a `has_child` chain, as a backstop against a cyclic tree.
const MAX_PARENT_CHAIN_DEPTH: usize = 1000;

/// SQL for a restricted collection at `alias`. JSON `true` and the string
/// `"true"` both count, as they do for the cloud walk's text comparison
/// (`properties->'collection'->>'restrictedToMembers' = 'true'`).
fn restricted_collection_sql(alias: &str) -> String {
    format!(
        "({alias}.node_type = 'collection' \
          AND (json_type({alias}.properties, '$.collection.restrictedToMembers') = 'true' \
               OR json_extract({alias}.properties, '$.collection.restrictedToMembers') = 'true'))"
    )
}

/// SQL for "the node `id_expr` might have access that differs from its outline
/// ancestry's": it holds a `member_of` edge. That is the only way a node gains
/// its own classification, since a collection (the only thing that restricts)
/// can never sit inside an outline. A `person`'s `member_of` edges are RBAC
/// membership, gated server-side rather than by the reachability walk, so they
/// do not classify the person node.
///
/// A candidate is only a *possible* boundary; [`SqliteStore::access_boundary`]
/// decides. The cheap filter keeps the walk off the common case.
fn access_candidate_sql(id_expr: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM node cn WHERE cn.id = {id_expr} \
            AND cn.node_type != 'person' \
            AND EXISTS (SELECT 1 FROM relationship cm \
                WHERE cm.in_node = cn.id AND cm.relationship_type = 'member_of'))"
    )
}

impl SqliteStore {
    /// The nearest restricted collections governing `node_id` through its
    /// **own** classification, ignoring its outline position: the walk up from
    /// its `member_of` edges, and on through each collection's own `member_of`
    /// edges (ADR-059 §1). A collection never has a `has_child` parent, so
    /// collection nesting is the whole walk. Of the restricted collections
    /// found, only the minimum-depth set is returned (§3: nearest boundary
    /// wins, ties grant). Empty means the node's access is inherited from its
    /// outline ancestry.
    ///
    /// The node itself is never counted, even if it is a restricted
    /// collection. A collection is only ever a tree root, and never embedded,
    /// so the one case this affects is a defect descendant of a restricted
    /// collection root. It is treated as a boundary, which errs toward
    /// excluding more.
    pub(crate) async fn access_boundary(&self, node_id: &str) -> Result<BTreeSet<String>> {
        let restricted = restricted_collection_sql("n");
        let mut seen: HashSet<String> = HashSet::from([node_id.to_string()]);
        let mut frontier: Vec<String> = vec![node_id.to_string()];

        for _ in 0..MAX_ACCESS_WALK_DEPTH {
            let mut next = Vec::new();
            for id in &frontier {
                let mut rows = self
                    .read()
                    .await?
                    .query(
                        "SELECT out_node FROM relationship \
                         WHERE in_node = ?1 AND relationship_type = 'member_of'",
                        libsql::params![id.clone()],
                    )
                    .await
                    .context("Failed to walk access ancestry")?;
                while let Some(row) = rows.next().await? {
                    let up: String = row.get(0)?;
                    if seen.insert(up.clone()) {
                        next.push(up);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            let found = self.restricted_among(&next, &restricted).await?;
            if !found.is_empty() {
                return Ok(found);
            }
            frontier = next;
        }
        Ok(BTreeSet::new())
    }

    async fn restricted_among(&self, ids: &[String], restricted: &str) -> Result<BTreeSet<String>> {
        let mut found = BTreeSet::new();
        const ID_CHUNK: usize = 900;
        for chunk in ids.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{}", i)).collect();
            let sql = format!(
                "SELECT n.id FROM node n WHERE n.id IN ({}) AND {}",
                placeholders.join(", "),
                restricted
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
                .context("Failed to check restricted collections")?;
            while let Some(row) = rows.next().await? {
                found.insert(row.get(0)?);
            }
        }
        Ok(found)
    }

    /// The topmost access candidates in `start`'s `has_child` subtree
    /// (excluding `start`): the walk does not descend below a candidate.
    async fn topmost_access_candidates(&self, start: &str) -> Result<Vec<String>> {
        // `INDEXED BY idx_rel_in` pins the fan-out step to the parent's own
        // edges; see `extends_closure_sql` for why the planner otherwise picks
        // `idx_rel_type` and scans every `has_child` edge per level.
        let sql = format!(
            r#"WITH RECURSIVE sub(node_id, depth) AS (
                SELECT ?1, 0
                UNION ALL
                SELECT r.out_node, s.depth + 1
                FROM sub s
                JOIN relationship r INDEXED BY idx_rel_in
                  ON r.in_node = s.node_id AND r.relationship_type = 'has_child'
                WHERE s.depth < 100 AND (s.depth = 0 OR NOT {stop})
            )
            SELECT DISTINCT node_id FROM sub WHERE depth > 0 AND {keep}"#,
            stop = access_candidate_sql("s.node_id"),
            keep = access_candidate_sql("sub.node_id"),
        );
        let mut rows = self
            .read()
            .await?
            .query(&sql, libsql::params![start.to_string()])
            .await
            .context("Failed to find access candidates in subtree")?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push(row.get(0)?);
        }
        Ok(ids)
    }

    /// The descendants of embedding root `root_id` whose access differs from
    /// the root's (ADR-059 §7). Only the topmost are returned: each one's
    /// subtree is out of the root's vector as a whole, and each one is itself
    /// an embedding root that answers the same question for its own subtree.
    ///
    /// Empty whenever the root-only membership invariant holds.
    pub async fn access_boundaries_under(&self, root_id: &str) -> Result<HashSet<String>> {
        let mut boundaries = HashSet::new();
        let mut root_access: Option<BTreeSet<String>> = None;
        let mut starts = vec![root_id.to_string()];
        while let Some(start) = starts.pop() {
            for candidate in self.topmost_access_candidates(&start).await? {
                if root_access.is_none() {
                    root_access = Some(self.access_boundary(root_id).await?);
                }
                let access = self.access_boundary(&candidate).await?;
                if !access.is_empty() && Some(&access) != root_access.as_ref() {
                    boundaries.insert(candidate);
                } else {
                    // Same access as the root: a boundary can still sit below it.
                    starts.push(candidate);
                }
            }
        }
        Ok(boundaries)
    }

    /// Resolve the embedding root of `node_id`: its tree root, unless an
    /// access boundary (see [`Self::access_boundaries_under`]) sits between
    /// them, in which case the nearest such boundary at or above the node.
    pub async fn embedding_root_id(&self, node_id: &str) -> Result<String> {
        // `chain[0]` is the node, the last element its tree root.
        let mut chain = vec![node_id.to_string()];
        while let Some(parent) = self.get_parent_id(chain.last().expect("non-empty")).await? {
            if chain.len() >= MAX_PARENT_CHAIN_DEPTH || chain.contains(&parent) {
                anyhow::bail!(
                    "has_child chain above {} exceeds {} or cycles",
                    node_id,
                    MAX_PARENT_CHAIN_DEPTH
                );
            }
            chain.push(parent);
        }
        let (tree_root, below_root) = chain.split_last().expect("non-empty");
        if below_root.is_empty() {
            return Ok(tree_root.clone());
        }

        let placeholders: Vec<String> = (1..=below_root.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "SELECT n.id FROM node n WHERE n.id IN ({}) AND {}",
            placeholders.join(", "),
            access_candidate_sql("n.id")
        );
        let params: Vec<libsql::Value> = below_root
            .iter()
            .map(|id| libsql::Value::Text(id.clone()))
            .collect();
        let candidates: HashSet<String> = {
            let mut rows = self
                .read()
                .await?
                .query(&sql, params)
                .await
                .context("Failed to find access candidates on parent chain")?;
            let mut ids = HashSet::new();
            while let Some(row) = rows.next().await? {
                ids.insert(row.get(0)?);
            }
            ids
        };
        if candidates.is_empty() {
            return Ok(tree_root.clone());
        }

        // Top-down, each boundary becomes the root the next one is compared to.
        let mut root = tree_root.clone();
        let mut root_access = self.access_boundary(&root).await?;
        for id in below_root.iter().rev() {
            if !candidates.contains(id) {
                continue;
            }
            let access = self.access_boundary(id).await?;
            if !access.is_empty() && access != root_access {
                root = id.clone();
                root_access = access;
            }
        }
        Ok(root)
    }
}
