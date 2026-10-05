//! `SqliteStore` methods — the structural rules (ADR-089): which children a
//! type's nodes may have, and where they may sit in the `has_child` tree.
//!
//! The database triggers refuse every edge and retype that would break a
//! rule, on every write path. These guards ask the same questions of the same
//! `structural_rule` rows before a write, so a caller gets a
//! [`TreeInvariantViolation`](super::TreeInvariantViolation) naming the rule
//! and the nodes instead of a trigger's abort.
//!
//! One rule has no edge for a trigger to see: a node whose type needs a
//! parent, created or left without one. [`SqliteStore::assert_may_be_root`]
//! is its only enforcement, so every path that creates a root or makes a node
//! one calls it.
use super::*;
use crate::db::schema::{
    has_child_checks, has_child_violations_sql, missing_parent_sql, structural_rule,
    STRUCTURAL_RULE_TABLE, TYPE_ANCESTRY_TABLE,
};
use crate::models::{SchemaChildrenRule, SchemaParentRule};

/// A structural rule a write would break, and the type whose schema
/// declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BrokenRule {
    rule: String,
    declared_by: String,
}

/// A node on one end of a `has_child` edge under check. A node not yet
/// created has no id.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Placed<'a> {
    pub id: Option<&'a str>,
    pub node_type: &'a str,
}

impl<'a> Placed<'a> {
    pub(crate) fn existing(id: &'a str, node_type: &'a str) -> Self {
        Self {
            id: Some(id),
            node_type,
        }
    }

    pub(crate) fn new_node(node_type: &'a str) -> Self {
        Self {
            id: None,
            node_type,
        }
    }

    pub(super) fn describe(&self) -> String {
        match self.id {
            Some(id) => format!("{} '{}'", self.node_type, id),
            None => format!("a new {}", self.node_type),
        }
    }
}

/// Where a structural check reads: a pooled reader, or a transaction's own
/// connection, which sees what the transaction has written.
#[derive(Clone, Copy)]
enum Reader<'a> {
    Pool(&'a SqliteStore),
    Tx(&'a libsql::Transaction),
}

impl Reader<'_> {
    /// Every row of `sql`, each column read as text, drained so no cursor is
    /// held across the next read. A `libsql::Row` reads through its cursor,
    /// so the values are copied out before the cursor moves on. Every query
    /// here selects non-null TEXT columns (ids, type names, rule names); one
    /// that selects anything else needs its own read.
    async fn rows(&self, sql: &str, params: Vec<libsql::Value>) -> Result<Vec<Vec<String>>> {
        fn texts(row: &libsql::Row) -> Result<Vec<String>> {
            (0..row.column_count())
                .map(|i| Ok(row.get::<String>(i)?))
                .collect()
        }
        let mut out = Vec::new();
        match self {
            Self::Pool(store) => {
                let mut rows = store.read().await?.query(sql, params).await?;
                while let Some(row) = rows.next().await? {
                    out.push(texts(&row)?);
                }
            }
            Self::Tx(tx) => {
                let mut rows = tx.query(sql, params).await?;
                while let Some(row) = rows.next().await? {
                    out.push(texts(&row)?);
                }
            }
        }
        Ok(out)
    }

    async fn first_broken(&self, sql: &str, types: &[&str]) -> Result<Option<BrokenRule>> {
        let params = types
            .iter()
            .map(|t| libsql::Value::Text(t.to_string()))
            .collect();
        let rows = self
            .rows(&format!("{sql} LIMIT 1"), params)
            .await
            .context("Failed to check a structural rule")?;
        Ok(rows.into_iter().next().map(|mut row| BrokenRule {
            declared_by: row.swap_remove(1),
            rule: row.swap_remove(0),
        }))
    }

    /// The rule a `has_child` edge from a `parent_type` node to a
    /// `child_type` node would break.
    async fn broken_edge_rule(
        &self,
        parent_type: &str,
        child_type: &str,
    ) -> Result<Option<BrokenRule>> {
        self.first_broken(
            &has_child_violations_sql("?1", "?2"),
            &[parent_type, child_type],
        )
        .await
    }

    /// The rule a `node_type` node with no parent would break.
    async fn broken_root_rule(&self, node_type: &str) -> Result<Option<BrokenRule>> {
        self.first_broken(&missing_parent_sql("?1", None), &[node_type])
            .await
    }

    /// The `must_be_root` rule a `node_type` node breaks by having any parent.
    async fn broken_by_any_parent(&self, node_type: &str) -> Result<Option<BrokenRule>> {
        let [(_, must_be_root), ..] = has_child_checks("?1", "?1");
        self.first_broken(&must_be_root, &[node_type]).await
    }

    /// The types `declared_by` lists for `rule`.
    async fn named_types(&self, declared_by: &str, rule: &str) -> Result<Vec<String>> {
        let rows = self
            .rows(
                &format!(
                    "SELECT target FROM {STRUCTURAL_RULE_TABLE} \
                     WHERE node_type = ?1 AND rule = ?2 ORDER BY target"
                ),
                vec![
                    libsql::Value::Text(declared_by.to_string()),
                    libsql::Value::Text(rule.to_string()),
                ],
            )
            .await
            .context("Failed to read a structural rule's named types")?;
        Ok(rows.into_iter().flatten().collect())
    }

    /// The stored type of each of `ids` that exists.
    async fn node_types(&self, ids: &[&str]) -> Result<HashMap<String, String>> {
        const ID_CHUNK: usize = 900;
        let mut unique: Vec<&str> = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        let mut types = HashMap::with_capacity(unique.len());
        for chunk in unique.chunks(ID_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len()).map(|i| format!("?{i}")).collect();
            let rows = self
                .rows(
                    &format!(
                        "SELECT id, node_type FROM node WHERE id IN ({})",
                        placeholders.join(", ")
                    ),
                    chunk
                        .iter()
                        .map(|id| libsql::Value::Text(id.to_string()))
                        .collect(),
                )
                .await
                .context("Failed to read node types for a structural check")?;
            for mut row in rows {
                let node_type = row.swap_remove(1);
                types.insert(row.swap_remove(0), node_type);
            }
        }
        Ok(types)
    }

    async fn violation(
        &self,
        broken: BrokenRule,
        parent: Option<Placed<'_>>,
        child: Placed<'_>,
    ) -> Result<anyhow::Error> {
        use super::TreeInvariantViolation as V;
        let BrokenRule { rule, declared_by } = broken;
        let violation = match (rule.as_str(), parent) {
            (structural_rule::CHILDREN_NONE, Some(parent)) => {
                V::children_none(&declared_by, parent, child)
            }
            (structural_rule::CHILDREN_EXCEPT, Some(parent)) => {
                V::child_not_allowed(&declared_by, parent, child)
            }
            (structural_rule::PARENT_OF, parent) => {
                let allowed = self.named_types(&declared_by, &rule).await?;
                V::parent_required(&declared_by, &allowed, parent, child)
            }
            _ => V::must_be_root(&declared_by, child.id),
        };
        Ok(anyhow::Error::new(violation))
    }

    async fn assert_edge(&self, parent: Placed<'_>, child: Placed<'_>) -> Result<()> {
        match self
            .broken_edge_rule(parent.node_type, child.node_type)
            .await?
        {
            Some(broken) => Err(self.violation(broken, Some(parent), child).await?),
            None => Ok(()),
        }
    }

    async fn assert_edges(&self, edges: &[(&str, &str)]) -> Result<()> {
        let ids: Vec<&str> = edges.iter().flat_map(|(p, c)| [*p, *c]).collect();
        let types = self.node_types(&ids).await?;
        // Most edges of a batch share a few type pairs: one check per pair.
        let mut allowed: HashSet<(&str, &str)> = HashSet::new();
        for (parent_id, child_id) in edges {
            // A missing node is the caller's own "not found" to report.
            let (Some(parent_type), Some(child_type)) =
                (types.get(*parent_id), types.get(*child_id))
            else {
                continue;
            };
            if allowed.contains(&(parent_type.as_str(), child_type.as_str())) {
                continue;
            }
            self.assert_edge(
                Placed::existing(parent_id, parent_type),
                Placed::existing(child_id, child_type),
            )
            .await?;
            allowed.insert((parent_type.as_str(), child_type.as_str()));
        }
        Ok(())
    }

    async fn assert_root(&self, node: Placed<'_>) -> Result<()> {
        match self.broken_root_rule(node.node_type).await? {
            Some(broken) => Err(self.violation(broken, None, node).await?),
            None => Ok(()),
        }
    }

    async fn assert_parentable(&self, node: Placed<'_>) -> Result<()> {
        match self.broken_by_any_parent(node.node_type).await? {
            Some(broken) => Err(self.violation(broken, None, node).await?),
            None => Ok(()),
        }
    }

    /// [`Self::assert_root`] for an existing node, by id.
    async fn assert_node_root(&self, node_id: &str) -> Result<()> {
        let types = self.node_types(&[node_id]).await?;
        match types.get(node_id) {
            Some(node_type) => self.assert_root(Placed::existing(node_id, node_type)).await,
            None => Ok(()),
        }
    }

    /// The `has_child` children of `parent_id`, as edges under `new_parent_id`.
    async fn assert_adoption(&self, new_parent_id: &str, parent_id: &str) -> Result<()> {
        let rows = self
            .rows(
                "SELECT out_node FROM relationship \
                 WHERE in_node = ?1 AND relationship_type = 'has_child'",
                vec![libsql::Value::Text(parent_id.to_string())],
            )
            .await
            .context("Failed to read a node's children for a structural check")?;
        let children: Vec<String> = rows.into_iter().flatten().collect();
        let edges: Vec<(&str, &str)> = children
            .iter()
            .filter(|child| child.as_str() != new_parent_id)
            .map(|child| (new_parent_id, child.as_str()))
            .collect();
        self.assert_edges(&edges).await
    }

    /// The rows of a bulk insert: each root row against its type's `parent`
    /// rule, each other row against its parent, which is another row of the
    /// batch or a node already stored.
    async fn assert_bulk(&self, rows: &[BulkNodeRow]) -> Result<()> {
        let in_batch: HashMap<&str, &str> = rows
            .iter()
            .map(|row| (row.0.as_str(), row.1.as_str()))
            .collect();
        let stored_parents: Vec<&str> = rows
            .iter()
            .filter_map(|row| row.3.as_deref())
            .filter(|parent| !in_batch.contains_key(parent))
            .collect();
        let stored = self.node_types(&stored_parents).await?;

        // An import is thousands of rows of a few types: one check per
        // distinct root type and per distinct (parent type, child type).
        let mut roots: HashSet<&str> = HashSet::new();
        let mut edges: HashSet<(&str, &str)> = HashSet::new();
        for (id, node_type, _, parent_id, ..) in rows {
            let child = Placed::existing(id, node_type);
            let Some(parent_id) = parent_id.as_deref() else {
                if roots.insert(node_type) {
                    self.assert_root(child).await?;
                }
                continue;
            };
            let parent_type = match in_batch.get(parent_id) {
                Some(parent_type) => *parent_type,
                None => match stored.get(parent_id) {
                    Some(parent_type) => parent_type.as_str(),
                    // A missing parent is the insert's own failure to report.
                    None => continue,
                },
            };
            if edges.insert((parent_type, node_type)) {
                self.assert_edge(Placed::existing(parent_id, parent_type), child)
                    .await?;
            }
        }
        Ok(())
    }

    async fn assert_retype(&self, node_id: &str, new_type: &str) -> Result<()> {
        let node = Placed::existing(node_id, new_type);
        let parent = self
            .rows(
                "SELECT p.id, p.node_type FROM relationship r JOIN node p ON p.id = r.in_node \
                 WHERE r.out_node = ?1 AND r.relationship_type = 'has_child' LIMIT 1",
                vec![libsql::Value::Text(node_id.to_string())],
            )
            .await
            .context("Failed to read a node's parent for a retype")?;
        match parent.first() {
            Some(row) => {
                self.assert_edge(Placed::existing(&row[0], &row[1]), node)
                    .await?;
            }
            None => self.assert_root(node).await?,
        }
        // One child per distinct type is enough to name in a refusal.
        let children = self
            .rows(
                "SELECT min(c.id), c.node_type FROM relationship r JOIN node c ON c.id = r.out_node \
                 WHERE r.in_node = ?1 AND r.relationship_type = 'has_child' GROUP BY c.node_type",
                vec![libsql::Value::Text(node_id.to_string())],
            )
            .await
            .context("Failed to read a node's children for a retype")?;
        for row in children {
            self.assert_edge(node, Placed::existing(&row[0], &row[1]))
                .await?;
        }
        Ok(())
    }

    async fn rules_in_force(
        &self,
        node_type: &str,
    ) -> Result<(SchemaChildrenRule, SchemaParentRule)> {
        // Furthest ancestor first, so each nearer declaration lands on top.
        let rows = self
            .rows(
                &format!(
                    "SELECT s.node_type, s.rule, s.target FROM {TYPE_ANCESTRY_TABLE} a \
                     JOIN {STRUCTURAL_RULE_TABLE} s ON s.node_type = a.ancestor \
                     WHERE a.node_type = ?1 ORDER BY a.depth DESC, s.rule, s.target"
                ),
                vec![libsql::Value::Text(node_type.to_string())],
            )
            .await
            .context("Failed to read a type's structural rules")?;
        Ok(compose_rules(&rows))
    }
}

/// One `structural_rule` row: `[declaring type, rule, target]`.
type DeclaredRule = Vec<String>;

/// The rules in force for a type, from the rows its chain declares, furthest
/// ancestor first.
///
/// This reads the same rows the triggers do and agrees with them: `none`
/// wins over a list, the `any_except` lists of the whole chain add up, and
/// the nearest `must_have_parent_of` list is the one in force, since a
/// subtype's can only be narrower.
fn compose_rules(declared: &[DeclaredRule]) -> (SchemaChildrenRule, SchemaParentRule) {
    let mut childless = false;
    let mut root_only = false;
    let mut except: Vec<String> = Vec::new();
    let mut parent_of: Vec<String> = Vec::new();
    let mut parent_of_declared_by: Option<&str> = None;
    for row in declared {
        let [declared_by, rule, target] = row.as_slice() else {
            continue;
        };
        match rule.as_str() {
            structural_rule::CHILDREN_NONE => childless = true,
            structural_rule::MUST_BE_ROOT => root_only = true,
            structural_rule::CHILDREN_EXCEPT if !except.contains(target) => {
                except.push(target.clone());
            }
            structural_rule::PARENT_OF => {
                if parent_of_declared_by != Some(declared_by.as_str()) {
                    parent_of.clear();
                    parent_of_declared_by = Some(declared_by);
                }
                parent_of.push(target.clone());
            }
            _ => {}
        }
    }
    let children = if childless {
        SchemaChildrenRule::None
    } else if except.is_empty() {
        SchemaChildrenRule::Any
    } else {
        SchemaChildrenRule::AnyExcept { types: except }
    };
    let parent = if root_only {
        SchemaParentRule::MustBeRoot
    } else if parent_of.is_empty() {
        SchemaParentRule::Any
    } else {
        SchemaParentRule::MustHaveParentOf { types: parent_of }
    };
    (children, parent)
}

impl SqliteStore {
    /// Refuse a `has_child` edge from `parent` to `child` that either end's
    /// structural rules do not allow: the child's `parent` rule and the
    /// parent's `children` rule, each resolved through its `extends` chain.
    pub(crate) async fn assert_has_child_allowed(
        &self,
        parent: Placed<'_>,
        child: Placed<'_>,
    ) -> Result<()> {
        Reader::Pool(self).assert_edge(parent, child).await
    }

    /// [`Self::assert_has_child_allowed`] for `(parent id, child id)` edges
    /// between existing nodes.
    pub(crate) async fn assert_has_child_edges_allowed(
        &self,
        edges: &[(&str, &str)],
    ) -> Result<()> {
        Reader::Pool(self).assert_edges(edges).await
    }

    /// `_in_tx` twin of [`Self::assert_has_child_edges_allowed`].
    pub(crate) async fn assert_has_child_edges_allowed_in_tx(
        tx: &Tx<'_>,
        edges: &[(&str, &str)],
    ) -> Result<()> {
        Reader::Tx(tx.conn()).assert_edges(edges).await
    }

    /// Refuse a node of a type that needs a parent (`must_have_parent_of`)
    /// being created, or left, without one. The database cannot refuse this
    /// itself: there is no edge for a trigger to check.
    pub async fn assert_may_be_root(&self, node_type: &str, node_id: Option<&str>) -> Result<()> {
        Reader::Pool(self)
            .assert_root(Placed {
                id: node_id,
                node_type,
            })
            .await
    }

    /// `_in_tx` twin of [`Self::assert_may_be_root`].
    pub(crate) async fn assert_may_be_root_in_tx(
        tx: &Tx<'_>,
        node_type: &str,
        node_id: Option<&str>,
    ) -> Result<()> {
        Reader::Tx(tx.conn())
            .assert_root(Placed {
                id: node_id,
                node_type,
            })
            .await
    }

    /// Refuse a parent for a node of `node_type` when its type is always a
    /// root, whatever the parent: the check a create owes before it resolves,
    /// or creates, the parent it was given.
    pub(crate) async fn assert_may_have_parent(
        &self,
        node_type: &str,
        node_id: Option<&str>,
    ) -> Result<()> {
        Reader::Pool(self)
            .assert_parentable(Placed {
                id: node_id,
                node_type,
            })
            .await
    }

    /// [`Self::assert_may_be_root`] for an existing node about to lose its
    /// parent.
    pub(crate) async fn assert_node_may_be_root(&self, node_id: &str) -> Result<()> {
        Reader::Pool(self).assert_node_root(node_id).await
    }

    /// `_in_tx` twin of [`Self::assert_node_may_be_root`].
    pub(crate) async fn assert_node_may_be_root_in_tx(tx: &Tx<'_>, node_id: &str) -> Result<()> {
        Reader::Tx(tx.conn()).assert_node_root(node_id).await
    }

    /// The structural rules for the rows of a bulk hierarchy insert, read on
    /// the insert's own transaction before any row is written: a root row's
    /// type must not need a parent, and every other row must be allowed under
    /// its parent.
    pub(crate) async fn assert_bulk_rows_allowed(
        tx: &libsql::Transaction,
        rows: &[BulkNodeRow],
    ) -> Result<()> {
        Reader::Tx(tx).assert_bulk(rows).await
    }

    /// Refuse re-pointing the children of `parent_id` under `new_parent_id`
    /// where the rules do not allow one of them there.
    pub(crate) async fn assert_may_adopt_children_in_tx(
        tx: &Tx<'_>,
        new_parent_id: &str,
        parent_id: &str,
    ) -> Result<()> {
        Reader::Tx(tx.conn())
            .assert_adoption(new_parent_id, parent_id)
            .await
    }

    /// Refuse retyping `node_id` to `new_type` where the new type's rules do
    /// not hold against the parent and children the node has, or theirs
    /// against it.
    pub(crate) async fn assert_retype_keeps_structure(
        &self,
        node_id: &str,
        new_type: &str,
    ) -> Result<()> {
        Reader::Pool(self).assert_retype(node_id, new_type).await
    }

    /// `_in_tx` twin of [`Self::assert_retype_keeps_structure`]: reads the
    /// node's parent and children on the transaction, so it sees a move or a
    /// create the transaction made before the retype.
    pub(crate) async fn assert_retype_keeps_structure_in_tx(
        tx: &Tx<'_>,
        node_id: &str,
        new_type: &str,
    ) -> Result<()> {
        Reader::Tx(tx.conn()).assert_retype(node_id, new_type).await
    }

    /// The structural rules in force for `node_type`: what it and its whole
    /// `extends` chain declare, composed.
    pub async fn structural_rules_in_force(
        &self,
        node_type: &str,
    ) -> Result<(SchemaChildrenRule, SchemaParentRule)> {
        Reader::Pool(self).rules_in_force(node_type).await
    }

    /// A node of `node_type`, or of a type extending it, that `children` and
    /// `parent` would leave in violation where it sits now, with what it
    /// breaks: the check a type owes before its rules tighten (ADR-089).
    /// `None` when every such node already satisfies both.
    pub async fn node_breaking_rules(
        &self,
        node_type: &str,
        children: &SchemaChildrenRule,
        parent: &SchemaParentRule,
    ) -> Result<Option<(String, &'static str)>> {
        // ?1 is the type; a rule's named types are bound from ?2 on.
        // A type is its own ancestor at depth 0, so this covers the type too.
        let subtypes = format!("SELECT node_type FROM {TYPE_ANCESTRY_TABLE} WHERE ancestor = ?1");
        let of_type = format!("n.node_type IN ({subtypes})");
        let is_named = |column: &str, named: &[String]| {
            let placeholders: Vec<String> = (2..named.len() + 2).map(|i| format!("?{i}")).collect();
            format!(
                "{column} IN (SELECT node_type FROM {TYPE_ANCESTRY_TABLE} WHERE ancestor IN ({}))",
                placeholders.join(", ")
            )
        };
        let has_parent = "EXISTS (SELECT 1 FROM relationship r \
                          WHERE r.out_node = n.id AND r.relationship_type = 'has_child')";
        let has_child = "EXISTS (SELECT 1 FROM relationship r \
                         WHERE r.in_node = n.id AND r.relationship_type = 'has_child')";

        let mut checks: Vec<(String, &[String], &'static str)> = Vec::new();
        match children {
            SchemaChildrenRule::Any => {}
            SchemaChildrenRule::None => checks.push((
                has_child.to_string(),
                &[],
                "has children, which the type would no longer take",
            )),
            SchemaChildrenRule::AnyExcept { types } => checks.push((
                format!(
                    "EXISTS (SELECT 1 FROM relationship r JOIN node c ON c.id = r.out_node \
                              WHERE r.in_node = n.id AND r.relationship_type = 'has_child' \
                                AND {})",
                    is_named("c.node_type", types)
                ),
                types,
                "has a child of a type the type would refuse",
            )),
        }
        match parent {
            SchemaParentRule::Any => {}
            SchemaParentRule::MustBeRoot => checks.push((
                has_parent.to_string(),
                &[],
                "has a parent, and the type would always be a root",
            )),
            SchemaParentRule::MustHaveParentOf { types } => checks.push((
                format!(
                    "NOT EXISTS (SELECT 1 FROM relationship r JOIN node p ON p.id = r.in_node \
                                  WHERE r.out_node = n.id AND r.relationship_type = 'has_child' \
                                    AND {})",
                    is_named("p.node_type", types)
                ),
                types,
                "has no parent of a type the type would need",
            )),
        }

        for (breaks, named, what) in checks {
            let mut params = vec![libsql::Value::Text(node_type.to_string())];
            params.extend(named.iter().map(|t| libsql::Value::Text(t.clone())));
            let rows = Reader::Pool(self)
                .rows(
                    &format!("SELECT n.id FROM node n WHERE {of_type} AND {breaks} LIMIT 1"),
                    params,
                )
                .await
                .context("Failed to check existing nodes against a structural rule")?;
            if let Some(node_id) = rows.into_iter().flatten().next() {
                return Ok(Some((node_id, what)));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(declared: &[(&str, &str, &str)]) -> Vec<DeclaredRule> {
        declared
            .iter()
            .map(|(by, rule, target)| vec![by.to_string(), rule.to_string(), target.to_string()])
            .collect()
    }

    #[test]
    fn rules_compose_from_the_furthest_ancestor_down() {
        assert_eq!(
            compose_rules(&[]),
            (SchemaChildrenRule::Any, SchemaParentRule::Any)
        );
        // `none` wins over a list declared anywhere in the chain.
        assert_eq!(
            compose_rules(&rows(&[
                ("base", "children_except", "task"),
                ("kind", "children_none", "")
            ]))
            .0,
            SchemaChildrenRule::None
        );
        // The lists of the whole chain add up.
        assert_eq!(
            compose_rules(&rows(&[
                ("base", "children_except", "task"),
                ("kind", "children_except", "person"),
                ("kind", "children_except", "task"),
            ]))
            .0,
            SchemaChildrenRule::AnyExcept {
                types: vec!["task".to_string(), "person".to_string()]
            }
        );
        assert_eq!(
            compose_rules(&rows(&[("base", "must_be_root", "")])).1,
            SchemaParentRule::MustBeRoot
        );
        // The nearest list replaces the base's: it can only be narrower.
        assert_eq!(
            compose_rules(&rows(&[
                ("base", "parent_of", "thread"),
                ("base", "parent_of", "topic"),
                ("kind", "parent_of", "support-thread"),
            ]))
            .1,
            SchemaParentRule::MustHaveParentOf {
                types: vec!["support-thread".to_string()]
            }
        );
    }
}
