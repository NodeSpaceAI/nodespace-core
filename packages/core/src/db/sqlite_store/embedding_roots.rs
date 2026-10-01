//! `SqliteStore` methods — embedding roots and the defect that splits them
//! (ADR-059 §6/§7).
//!
//! An embedding root is normally a tree root: its vector aggregates its whole
//! `has_child` subtree. Two constraints keep every non-person node of that
//! subtree out of any collection of its own (ADR-059 §2): only a root may
//! hold a `member_of` edge (`assert_may_gain_parent` and the `member_of`
//! insert guards), and a collection is always a root (the
//! `collection_is_root_*` schema triggers).
//! A non-person descendant that holds a `member_of` edge therefore breaks §2,
//! and finding one is a defect whatever any collection's properties say
//! (ADR-083 §5). The descendant is kept out of the root's vector and becomes
//! its own embedding root, so its content stays searchable.
use super::*;

/// Upper bound on a `has_child` chain, as a backstop against a cyclic tree.
const MAX_PARENT_CHAIN_DEPTH: usize = 1000;

/// SQL for "the node `id_expr` is filed into a collection of its own": it
/// holds a `member_of` edge and is not a `person` (or a subtype of one). A
/// person's `member_of` edge makes the person a member of the collection; it
/// does not file the person node as content, so it does not split the person
/// from its root.
fn filed_descendant_sql(id_expr: &str) -> String {
    let not_a_person =
        crate::db::schema::is_not_a_sql("cn.node_type", &[crate::models::CoreNodeType::Person]);
    format!(
        "EXISTS (SELECT 1 FROM node cn WHERE cn.id = {id_expr} \
            AND {not_a_person} \
            AND EXISTS (SELECT 1 FROM relationship cm \
                WHERE cm.in_node = cn.id AND cm.relationship_type = 'member_of'))"
    )
}

impl SqliteStore {
    /// The descendants of embedding root `root_id` that are filed into a
    /// collection of their own (ADR-059 §7). Only the topmost are returned:
    /// each one's subtree is out of the root's vector as a whole, and each one
    /// is itself an embedding root that answers the same question for its own
    /// subtree.
    ///
    /// Empty whenever the root-only membership invariant holds.
    pub async fn access_boundaries_under(&self, root_id: &str) -> Result<HashSet<String>> {
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
            stop = filed_descendant_sql("s.node_id"),
            keep = filed_descendant_sql("sub.node_id"),
        );
        let mut rows = self
            .read()
            .await?
            .query(&sql, libsql::params![root_id.to_string()])
            .await
            .context("Failed to find filed descendants in subtree")?;
        let mut boundaries = HashSet::new();
        while let Some(row) = rows.next().await? {
            boundaries.insert(row.get(0)?);
        }
        Ok(boundaries)
    }

    /// Resolve the embedding root of `node_id`: its tree root, unless a filed
    /// descendant (see [`Self::access_boundaries_under`]) sits between them, in
    /// which case the nearest one at or above the node.
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
            filed_descendant_sql("n.id")
        );
        let params: Vec<libsql::Value> = below_root
            .iter()
            .map(|id| libsql::Value::Text(id.clone()))
            .collect();
        let filed: HashSet<String> = {
            let mut rows = self
                .read()
                .await?
                .query(&sql, params)
                .await
                .context("Failed to find filed descendants on parent chain")?;
            let mut ids = HashSet::new();
            while let Some(row) = rows.next().await? {
                ids.insert(row.get(0)?);
            }
            ids
        };
        // `below_root` runs from the node upward, so the first filed node is
        // the nearest.
        Ok(below_root
            .iter()
            .find(|id| filed.contains(*id))
            .unwrap_or(tree_root)
            .clone())
    }
}
