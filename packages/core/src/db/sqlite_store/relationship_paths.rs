//! Compiling a resolved [`RelationshipPath`](nodespace_types::RelationshipPath)
//! to SQL, and running it (ADR-086 §11).
//!
//! This is the one place that walks the `relationship` table along a path.
//! Query relationship filters, a play's `for_each` and the dot-paths in a
//! play's conditions all compile here, so the idioms below are written once.
//!
//! # The shape of a walk
//!
//! A path compiles to one statement: a chain of common table expressions, one
//! per hop, each holding the nodes that hop reaches.
//!
//! - **One correlated subquery per hop, never a JOIN against `relationship`.**
//!   A hop looks its edges up by the node it is leaving
//!   (`WHERE r.in_node = <current>` or `r.out_node = <current>`), inside a
//!   scalar subquery that aggregates the far ends into a JSON array;
//!   `json_each` then turns that array back into rows. Written this way the
//!   only access path into `relationship` is the endpoint lookup, which the
//!   `(in_node, relationship_type)` and `(out_node, relationship_type)`
//!   indexes serve. Written as a JOIN, SQLite is free to drive the walk from
//!   `idx_rel_type` and scan every edge of that type at each step; on a
//!   database with tens of thousands of `has_child` edges that form measured
//!   about 50 times slower. The query-plan tests below pin the indexed form.
//! - **`WITH RECURSIVE` for an open-ended hop.** The walker's rows are the
//!   nodes reached and nothing else, and its arms are joined with `UNION`
//!   (not `UNION ALL`), so a node already reached is not walked from again:
//!   the walk ends when it stops finding new nodes, cycles included, at
//!   whatever depth that is. There is deliberately no depth column. With one,
//!   the same node at two depths would be two rows, and a cycle would lap
//!   until a cap stopped it.
//! - **Participation in the walk.** A caller that walks for automation asks
//!   for participating nodes only: each hop then reaches only nodes the
//!   governance check admits ([`crate::governance::visible_sql`]), so an
//!   archived node is neither reached nor walked through.
//! - **Keyed by root.** A batched walk carries the root it started from
//!   through every hop, so one root's answer is never returned for another.
//! - **Batched across roots.** The roots are bound as one JSON array, so a
//!   scan resolves a path for every root in one statement with a fixed number
//!   of parameters, whatever the size of the scan.

use super::*;
use nodespace_types::{HopDirection, ResolvedHop, ResolvedPath};

/// How many columns a `node` row has. A walk's statement selects `n.*` and
/// then its own columns, which start here.
const NODE_COLUMNS: i32 = 9;

/// The nodes each hop of a path reaches, keyed by the root the walk started
/// from.
///
/// Every hop is kept, not only the last: a caller that needs "the parent, and
/// the parent's children" reads both from one statement, and a caller that
/// treats a hop reaching several nodes differently from one reaching a single
/// node can tell which happened at each step.
#[derive(Debug, Default)]
pub struct PathReach {
    /// `hops[i]` maps a root id to the nodes hop `i` reaches from it. The
    /// nodes one node leads to come in edge insertion order; a hop that
    /// arrives from several nodes lists each one's in turn, so a node two of
    /// them lead to appears twice. A root a hop reaches nothing from has no
    /// entry.
    hops: Vec<HashMap<String, Vec<Node>>>,
}

impl PathReach {
    /// The nodes hop `hop` (0-based) reaches from `root_id`.
    pub fn at(&self, hop: usize, root_id: &str) -> &[Node] {
        self.hops
            .get(hop)
            .and_then(|by_root| by_root.get(root_id))
            .map_or(&[], Vec::as_slice)
    }
}

/// One step of a compiled walk: which end of the edge the walk leaves from,
/// which it arrives at, and the hop that names the edges.
///
/// A path walked forward leaves from the hop's near end. The same path walked
/// from its far end (to ask "which nodes reach this one?") takes the hops in
/// reverse and swaps the two columns; the edges are the same rows.
struct Step<'a> {
    from: &'static str,
    to: &'static str,
    hop: &'a ResolvedHop,
}

impl<'a> Step<'a> {
    fn forward(hop: &'a ResolvedHop) -> Self {
        match hop.direction {
            HopDirection::Outbound => Self {
                from: "in_node",
                to: "out_node",
                hop,
            },
            HopDirection::Inbound => Self {
                from: "out_node",
                to: "in_node",
                hop,
            },
        }
    }

    fn backward(hop: &'a ResolvedHop) -> Self {
        let forward = Self::forward(hop);
        Self {
            from: forward.to,
            to: forward.from,
            hop,
        }
    }
}

/// The correlated subquery of one step: the far ends of the step's edges that
/// leave `current`, as a JSON array in edge insertion order.
///
/// `source_type` narrows a reverse-name hop to its declarer: the edge's source
/// (`in_node`, whichever way the walk crosses it) must be that type or a type
/// extending it, read from the ancestry table.
///
/// `include_archived` says whether the hop reaches archived nodes. Without
/// it, the node the hop arrives at must participate.
fn step_lookup(
    step: &Step<'_>,
    current: &str,
    include_archived: bool,
    bind: &mut dyn FnMut(libsql::Value) -> String,
) -> String {
    let relationship_type = bind(libsql::Value::Text(step.hop.relationship_type.clone()));
    let source_filter = match &step.hop.source_type {
        Some(source_type) => {
            let source_type = bind(libsql::Value::Text(source_type.clone()));
            format!(
                " AND EXISTS (SELECT 1 FROM node s WHERE s.id = r.in_node AND {})",
                crate::db::schema::is_a_bound_sql("s.node_type", &source_type),
            )
        }
        None => String::new(),
    };
    let participation_filter = match crate::governance::visible_sql("t", include_archived) {
        Some(visible) => format!(
            " AND EXISTS (SELECT 1 FROM node t WHERE t.id = r.{to} AND {visible})",
            to = step.to,
        ),
        None => String::new(),
    };
    format!(
        "SELECT json_group_array(v) FROM (SELECT r.{to} AS v FROM relationship r \
         WHERE r.{from} = {current} AND r.relationship_type = {relationship_type}\
         {source_filter}{participation_filter} ORDER BY r.rowid)",
        from = step.from,
        to = step.to,
    )
}

/// The common table expressions walking `steps` from `seed`, and the name of
/// the one holding each step's nodes.
///
/// `seed` selects the walk's starting rows: `(root_id, node_id)` when
/// `track_root`, else `(node_id)`. Every returned table has those same
/// columns, so a caller reads any step the same way.
fn walk_ctes(
    seed: &str,
    steps: &[Step<'_>],
    track_root: bool,
    include_archived: bool,
    bind: &mut dyn FnMut(libsql::Value) -> String,
) -> (String, Vec<String>) {
    let columns = if track_root {
        "root_id, node_id"
    } else {
        "node_id"
    };
    let carry = |alias: &str| {
        if track_root {
            format!("{alias}.root_id, ")
        } else {
            String::new()
        }
    };

    let mut ctes = vec![format!("h0({columns}) AS ({seed})")];
    let mut names = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let previous = format!("h{index}");
        let name = format!("h{}", index + 1);
        if step.hop.open_ended {
            // `UNION` makes the table a set of the nodes reached: a node is
            // walked from once, however many ways lead to it.
            let first = step_lookup(step, "p.node_id", include_archived, bind);
            let again = step_lookup(step, "w.node_id", include_archived, bind);
            ctes.push(format!(
                "{name}({columns}) AS (\
                 SELECT {carry_p}j.value FROM {previous} p, json_each(({first})) j \
                 UNION \
                 SELECT {carry_w}j.value FROM {name} w, json_each(({again})) j)",
                carry_p = carry("p"),
                carry_w = carry("w"),
            ));
        } else {
            let lookup = step_lookup(step, "p.node_id", include_archived, bind);
            ctes.push(format!(
                "{name}({columns}) AS (\
                 SELECT {carry_p}j.value FROM {previous} p, json_each(({lookup})) j)",
                carry_p = carry("p"),
            ));
        }
        names.push(name);
    }
    (ctes.join(", "), names)
}

/// The statement resolving `path` for every root in one JSON array parameter:
/// one row per (hop, root, reached node), the node's columns first.
fn batch_statement(
    path: &ResolvedPath,
    roots_json: String,
    include_archived: bool,
) -> (String, Vec<libsql::Value>) {
    let mut params = vec![libsql::Value::Text(roots_json)];
    let mut bind = |value: libsql::Value| {
        params.push(value);
        format!("?{}", params.len())
    };
    let steps: Vec<Step<'_>> = path.hops.iter().map(Step::forward).collect();
    let (ctes, names) = walk_ctes(
        "SELECT value, value FROM json_each(?1)",
        &steps,
        true,
        include_archived,
        &mut bind,
    );
    // `CROSS JOIN` fixes the loop order: the hop's rows drive, and each node
    // is fetched by primary key.
    let selects: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            format!(
                "SELECT n.*, {index} AS hop, h.root_id FROM {name} h \
                 CROSS JOIN node n ON n.id = h.node_id"
            )
        })
        .collect();
    (
        format!("WITH RECURSIVE {ctes} {}", selects.join(" UNION ALL ")),
        params,
    )
}

/// A SQL condition that is true when `path`, walked from the node whose id is
/// `column`, reaches a node `seed` selects.
///
/// The walk runs from the seed backward rather than from every candidate
/// forward: it is evaluated once for the statement, starting from the few
/// nodes the seed names, instead of once per candidate row.
///
/// `seed` is a `SELECT` of node ids; its values are bound by the caller.
/// `bind` records a value and returns its placeholder.
///
/// The walk passes through every node, archived or not: which rows a query
/// returns is the query's own participation condition, on the candidate.
pub(crate) fn path_reaches_condition(
    column: &str,
    path: &ResolvedPath,
    seed: &str,
    bind: &mut dyn FnMut(libsql::Value) -> String,
) -> String {
    let steps: Vec<Step<'_>> = path.hops.iter().rev().map(Step::backward).collect();
    let (ctes, names) = walk_ctes(seed, &steps, false, true, bind);
    let reached = names.last().map_or("h0", String::as_str);
    format!("{column} IN (WITH RECURSIVE {ctes} SELECT node_id FROM {reached})")
}

impl SqliteStore {
    /// Walk `path` from every node in `root_ids`, in one statement.
    ///
    /// Returns the nodes each hop reaches, keyed by the root the walk started
    /// from. An empty path, or no roots, reaches nothing.
    ///
    /// Without `include_archived` the walk reaches participating nodes only,
    /// and does not continue through an archived one (ADR-087). The roots are
    /// the caller's: a walk from an archived root is still a walk.
    pub async fn resolve_relationship_path(
        &self,
        root_ids: &[String],
        path: &ResolvedPath,
        include_archived: bool,
    ) -> Result<PathReach> {
        let mut reach = PathReach {
            hops: vec![HashMap::new(); path.len()],
        };
        if root_ids.is_empty() || path.is_empty() {
            return Ok(reach);
        }
        let roots_json =
            serde_json::to_string(root_ids).context("Failed to encode path root ids")?;
        let (sql, params) = batch_statement(path, roots_json, include_archived);
        let mut rows = self
            .read()
            .await?
            .query(&sql, params)
            .await
            .context("Failed to resolve a relationship path")?;
        while let Some(row) = rows.next().await? {
            let node = Self::row_to_node(&row)?;
            let hop: i64 = row.get(NODE_COLUMNS)?;
            let root_id: String = row.get(NODE_COLUMNS + 1)?;
            let by_root = reach
                .hops
                .get_mut(hop as usize)
                .context("A relationship path row named a hop the path does not have")?;
            by_root.entry(root_id).or_default().push(node);
        }
        Ok(reach)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn store() -> (SqliteStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = SqliteStore::new(dir.path().join("test.db")).await.unwrap();
        (store, dir)
    }

    async fn node(store: &SqliteStore, id: &str, node_type: &str) {
        store
            .create_node(
                Node::new_with_id(
                    id.to_string(),
                    node_type.to_string(),
                    id.to_string(),
                    serde_json::json!({}),
                ),
                None,
                None,
            )
            .await
            .unwrap();
    }

    async fn edge(store: &SqliteStore, from: &str, relationship_type: &str, to: &str) {
        let now = Utc::now().to_rfc3339();
        store
            .write()
            .await
            .execute(
                "INSERT INTO relationship (in_node, out_node, relationship_type, created_at, modified_at) \
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                libsql::params![
                    from.to_string(),
                    to.to_string(),
                    relationship_type.to_string(),
                    now
                ],
            )
            .await
            .unwrap();
    }

    fn hop(relationship_type: &str, direction: HopDirection) -> ResolvedHop {
        ResolvedHop {
            name: relationship_type.to_string(),
            relationship_type: relationship_type.to_string(),
            direction,
            source_type: None,
            far_type: None,
            declared_many: false,
            untyped: false,
            open_ended: false,
        }
    }

    fn open_ended(mut hop: ResolvedHop) -> ResolvedHop {
        hop.open_ended = true;
        hop
    }

    fn path(hops: Vec<ResolvedHop>) -> ResolvedPath {
        ResolvedPath { hops }
    }

    impl PathReach {
        /// The nodes the whole path reaches from `root_id`.
        fn reached(&self, root_id: &str) -> &[Node] {
            match self.hops.len() {
                0 => &[],
                n => self.at(n - 1, root_id),
            }
        }
    }

    fn ids(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|n| n.id.as_str()).collect()
    }

    fn roots(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    /// grand → parent → {a, b}; a → c. `other` is a second tree.
    async fn family(store: &SqliteStore) {
        for id in ["grand", "parent", "a", "b", "c", "other", "other-child"] {
            node(store, id, "text").await;
        }
        edge(store, "grand", "has_child", "parent").await;
        edge(store, "parent", "has_child", "a").await;
        edge(store, "parent", "has_child", "b").await;
        edge(store, "a", "has_child", "c").await;
        edge(store, "other", "has_child", "other-child").await;
    }

    async fn plan(store: &SqliteStore, sql: &str, params: Vec<libsql::Value>) -> String {
        let mut rows = store
            .read()
            .await
            .unwrap()
            .query(&format!("EXPLAIN QUERY PLAN {sql}"), params)
            .await
            .unwrap();
        let mut detail = String::new();
        while let Some(row) = rows.next().await.unwrap() {
            // EXPLAIN QUERY PLAN: (id, parent, notused, detail)
            detail.push_str(&row.get::<String>(3).unwrap());
            detail.push('\n');
        }
        detail
    }

    /// Every access to `relationship` in a plan must be an endpoint lookup.
    fn assert_endpoint_lookups_only(plan: &str) {
        assert!(
            !plan.contains("idx_rel_type"),
            "a path walk must not drive off idx_rel_type, which scans every edge \
             of a type at each step; plan was:\n{plan}"
        );
        let relationship_lines: Vec<&str> = plan
            .lines()
            .filter(|line| line.contains(" r ") || line.ends_with(" r"))
            .collect();
        assert!(
            !relationship_lines.is_empty(),
            "the plan should read the relationship table; plan was:\n{plan}"
        );
        for line in relationship_lines {
            assert!(
                line.contains("SEARCH")
                    && (line.contains("in_node=?") || line.contains("out_node=?")),
                "every relationship access must be a lookup by endpoint, got `{line}`; \
                 plan was:\n{plan}"
            );
        }
    }

    #[tokio::test]
    async fn a_two_hop_path_resolves_for_every_root_in_one_statement() {
        let (store, _dir) = store().await;
        family(&store).await;

        // "My parent, then my parent's children": the siblings, self included.
        let siblings = path(vec![
            hop("has_child", HopDirection::Inbound),
            hop("has_child", HopDirection::Outbound),
        ]);
        let reach = store
            .resolve_relationship_path(
                &roots(&["a", "b", "c", "grand", "missing"]),
                &siblings,
                false,
            )
            .await
            .unwrap();

        assert_eq!(ids(reach.at(0, "a")), ["parent"]);
        assert_eq!(ids(reach.reached("a")), ["a", "b"]);
        assert_eq!(ids(reach.reached("b")), ["a", "b"]);
        assert_eq!(ids(reach.at(0, "c")), ["a"]);
        assert_eq!(ids(reach.reached("c")), ["c"]);
        // A root with no parent reaches nothing, at either hop.
        assert!(reach.at(0, "grand").is_empty());
        assert!(reach.reached("grand").is_empty());
        assert!(reach.reached("missing").is_empty());
    }

    /// The same path from two roots is two answers. Sharing one would hand a
    /// node another node's parent.
    #[tokio::test]
    async fn results_are_keyed_by_root_never_shared_between_roots() {
        let (store, _dir) = store().await;
        family(&store).await;

        let parent = path(vec![hop("has_child", HopDirection::Inbound)]);
        let reach = store
            .resolve_relationship_path(&roots(&["a", "other-child", "c"]), &parent, false)
            .await
            .unwrap();

        assert_eq!(ids(reach.reached("a")), ["parent"]);
        assert_eq!(ids(reach.reached("other-child")), ["other"]);
        assert_eq!(ids(reach.reached("c")), ["a"]);
    }

    #[tokio::test]
    async fn an_open_ended_hop_walks_to_every_depth() {
        let (store, _dir) = store().await;
        family(&store).await;

        let ancestors = path(vec![open_ended(hop("has_child", HopDirection::Inbound))]);
        let reach = store
            .resolve_relationship_path(&roots(&["c", "b"]), &ancestors, false)
            .await
            .unwrap();
        let mut of_c = ids(reach.reached("c"));
        of_c.sort_unstable();
        assert_eq!(of_c, ["a", "grand", "parent"]);
        let mut of_b = ids(reach.reached("b"));
        of_b.sort_unstable();
        assert_eq!(of_b, ["grand", "parent"]);

        let descendants = path(vec![open_ended(hop("has_child", HopDirection::Outbound))]);
        let reach = store
            .resolve_relationship_path(&roots(&["grand"]), &descendants, false)
            .await
            .unwrap();
        let mut of_grand = ids(reach.reached("grand"));
        of_grand.sort_unstable();
        assert_eq!(of_grand, ["a", "b", "c", "parent"]);
    }

    /// A cycle in the edges must end the walk once every node on it has been
    /// reached. Counted on the walker's own rows, not on the distinct result:
    /// a walker that laps the cycle still returns each node once, and only
    /// its row count shows the laps.
    #[tokio::test]
    async fn an_open_ended_walk_over_a_cycle_visits_each_node_once() {
        let (store, _dir) = store().await;
        for id in ["x", "y", "z"] {
            node(&store, id, "text").await;
        }
        edge(&store, "x", "relates_to", "y").await;
        edge(&store, "y", "relates_to", "z").await;
        edge(&store, "z", "relates_to", "x").await;

        let around = path(vec![open_ended(hop("relates_to", HopDirection::Outbound))]);
        let reach = store
            .resolve_relationship_path(&roots(&["x"]), &around, false)
            .await
            .unwrap();
        let mut reached = ids(reach.reached("x"));
        reached.sort_unstable();
        assert_eq!(reached, ["x", "y", "z"]);

        let mut params = vec![libsql::Value::Text("[\"x\"]".to_string())];
        let mut bind = |value: libsql::Value| {
            params.push(value);
            format!("?{}", params.len())
        };
        let steps: Vec<Step<'_>> = around.hops.iter().map(Step::forward).collect();
        let (ctes, names) = walk_ctes(
            "SELECT value, value FROM json_each(?1)",
            &steps,
            true,
            false,
            &mut bind,
        );
        let walked = store
            .count_nodes_raw(
                &format!("WITH RECURSIVE {ctes} SELECT COUNT(*) FROM {}", names[0]),
                params,
            )
            .await
            .unwrap();
        assert_eq!(
            walked, 3,
            "the walker must hold one row per node, not one per lap"
        );
    }

    /// A hop continues from an open-ended one: every ancestor's children.
    #[tokio::test]
    async fn a_fixed_hop_follows_an_open_ended_one() {
        let (store, _dir) = store().await;
        family(&store).await;

        let cousins = path(vec![
            open_ended(hop("has_child", HopDirection::Inbound)),
            hop("has_child", HopDirection::Outbound),
        ]);
        let reach = store
            .resolve_relationship_path(&roots(&["c"]), &cousins, false)
            .await
            .unwrap();
        let mut reached = ids(reach.reached("c"));
        reached.sort_unstable();
        // a's children (c), parent's children (a, b), grand's children (parent).
        assert_eq!(reached, ["a", "b", "c", "parent"]);
    }

    /// Two schemas may store edges under one name toward the same type. A
    /// reverse name belongs to one declarer, and only that declarer's edges
    /// (and its subtypes') answer it.
    #[tokio::test]
    async fn a_source_type_keeps_only_the_declaring_types_edges() {
        let (store, _dir) = store().await;
        node(&store, "the-task", "task").await;
        node(&store, "the-project", "project").await;
        node(&store, "the-person", "person").await;
        edge(&store, "the-project", "tasks", "the-task").await;
        edge(&store, "the-person", "tasks", "the-task").await;

        let mut assignee = hop("tasks", HopDirection::Inbound);
        assignee.source_type = Some("person".to_string());
        let reach = store
            .resolve_relationship_path(&roots(&["the-task"]), &path(vec![assignee]), false)
            .await
            .unwrap();
        assert_eq!(ids(reach.reached("the-task")), ["the-person"]);

        let unnarrowed = hop("tasks", HopDirection::Inbound);
        let reach = store
            .resolve_relationship_path(&roots(&["the-task"]), &path(vec![unnarrowed]), false)
            .await
            .unwrap();
        assert_eq!(
            ids(reach.reached("the-task")),
            ["the-project", "the-person"]
        );
    }

    /// A walk for participating nodes neither reaches an archived node nor
    /// passes through one; a walk that opts in sees it like any other.
    #[tokio::test]
    async fn an_archived_node_is_neither_reached_nor_walked_through() {
        let (store, _dir) = store().await;
        family(&store).await;
        store
            .write()
            .await
            .execute(
                "UPDATE node SET lifecycle_status = 'archived' WHERE id = 'a'",
                (),
            )
            .await
            .unwrap();

        let children = path(vec![hop("has_child", HopDirection::Outbound)]);
        let grandchildren = path(vec![
            hop("has_child", HopDirection::Outbound),
            hop("has_child", HopDirection::Outbound),
        ]);
        let descendants = path(vec![open_ended(hop("has_child", HopDirection::Outbound))]);

        let reach = store
            .resolve_relationship_path(&roots(&["parent"]), &children, false)
            .await
            .unwrap();
        assert_eq!(
            ids(reach.reached("parent")),
            ["b"],
            "the archived child is left out"
        );

        // `c` is active, but the only way to it is through archived `a`.
        let reach = store
            .resolve_relationship_path(&roots(&["parent"]), &grandchildren, false)
            .await
            .unwrap();
        assert!(reach.reached("parent").is_empty());
        let reach = store
            .resolve_relationship_path(&roots(&["parent"]), &descendants, false)
            .await
            .unwrap();
        assert_eq!(ids(reach.reached("parent")), ["b"]);

        let reach = store
            .resolve_relationship_path(&roots(&["parent"]), &grandchildren, true)
            .await
            .unwrap();
        assert_eq!(ids(reach.reached("parent")), ["c"]);
    }

    #[tokio::test]
    async fn an_empty_path_or_no_roots_reaches_nothing() {
        let (store, _dir) = store().await;
        family(&store).await;

        let reach = store
            .resolve_relationship_path(&roots(&["a"]), &path(vec![]), false)
            .await
            .unwrap();
        assert!(reach.reached("a").is_empty());

        let parent = path(vec![hop("has_child", HopDirection::Inbound)]);
        let reach = store
            .resolve_relationship_path(&[], &parent, false)
            .await
            .unwrap();
        assert!(reach.reached("a").is_empty());
    }

    /// The membership condition a query filter compiles to: which nodes reach
    /// a given node along the path.
    #[tokio::test]
    async fn the_reaches_condition_selects_the_nodes_that_reach_the_seed() {
        let (store, _dir) = store().await;
        family(&store).await;

        let matching = |path: ResolvedPath, anchor: &'static str| {
            let store = &store;
            async move {
                let mut params = Vec::new();
                let mut bind = |value: libsql::Value| {
                    params.push(value);
                    format!("?{}", params.len())
                };
                let anchor = bind(libsql::Value::Text(anchor.to_string()));
                let condition = path_reaches_condition(
                    "node.id",
                    &path,
                    &format!("SELECT {anchor}"),
                    &mut bind,
                );
                let mut found = store
                    .query_node_ids_raw(&format!("SELECT id FROM node WHERE {condition}"), params)
                    .await
                    .unwrap();
                found.sort_unstable();
                found
            }
        };

        // Nodes whose parent is `parent`.
        let child_of = path(vec![hop("has_child", HopDirection::Inbound)]);
        assert_eq!(matching(child_of, "parent").await, ["a", "b"]);

        // Nodes with `grand` among their ancestors.
        let under = path(vec![open_ended(hop("has_child", HopDirection::Inbound))]);
        assert_eq!(matching(under, "grand").await, ["a", "b", "c", "parent"]);

        // Nodes whose parent's parent is `grand`.
        let grandchild_of = path(vec![
            hop("has_child", HopDirection::Inbound),
            hop("has_child", HopDirection::Inbound),
        ]);
        assert_eq!(matching(grandchild_of, "grand").await, ["a", "b"]);
    }

    /// The walk must look edges up by endpoint at every hop, fixed and
    /// open-ended, in both the batched and the membership form.
    #[tokio::test]
    async fn every_hop_looks_edges_up_by_endpoint_never_by_relationship_type() {
        let (store, _dir) = store().await;
        family(&store).await;

        let mut narrowed = hop("tasks", HopDirection::Inbound);
        narrowed.source_type = Some("person".to_string());
        let walked = path(vec![
            hop("has_child", HopDirection::Inbound),
            open_ended(hop("has_child", HopDirection::Outbound)),
            narrowed,
        ]);

        let (sql, params) = batch_statement(&walked, "[\"a\"]".to_string(), false);
        assert_eq!(
            sql.matches("FROM relationship").count(),
            // One correlated subquery per fixed hop, two (first step and
            // repeat) for the open-ended one.
            4,
            "{sql}"
        );
        assert!(!sql.contains("JOIN relationship"), "{sql}");
        assert_endpoint_lookups_only(&plan(&store, &sql, params).await);

        let mut params = Vec::new();
        let mut bind = |value: libsql::Value| {
            params.push(value);
            format!("?{}", params.len())
        };
        let anchor = bind(libsql::Value::Text("grand".to_string()));
        let condition =
            path_reaches_condition("node.id", &walked, &format!("SELECT {anchor}"), &mut bind);
        let sql = format!("SELECT id FROM node WHERE {condition}");
        assert!(!sql.contains("JOIN relationship"), "{sql}");
        assert_endpoint_lookups_only(&plan(&store, &sql, params).await);
    }
}
