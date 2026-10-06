//! Graph Resolver for the Playbook Engine
//!
//! Resolves the dot-paths of a play's conditions and bindings against the
//! data graph.
//!
//! A dot-path such as `node.story.epic.status` is a [`RelationshipPath`]
//! ending in a property. The resolver turns its relationship segments into
//! resolved hops ([`crate::ops::path_ops`]) and hands them to the store,
//! which walks the whole run of hops in one SQL statement for every root at
//! once ([`crate::db::SqliteStore::resolve_relationship_path`]). Nothing here
//! walks the graph one hop, or one node, at a time.
//!
//! All of it happens ahead of CEL evaluation: paths are resolved with plain
//! `async`/`.await`, and the results are injected into the CEL context before
//! `Program::execute()` (which is synchronous) ever runs. This keeps the
//! sync/async boundary at the CEL-evaluation edge instead of bridging it
//! internally.

use crate::models::Node;
use crate::ops::path_ops::{resolve_hop, HopResolution};
use crate::playbook::cel::{json_to_cel, key, scoped_node_value, CelScope};
use crate::playbook::path_extractor::{CollectionPath, ExtractedPath};
use crate::services::NodeService;
use cel_interpreter::Value;
use nodespace_types::{RelationshipHop, RelationshipPath, ResolvedHop, ResolvedPath};
use std::collections::HashMap;
use std::sync::Arc;

/// Resolved value from a graph traversal.
#[derive(Debug, Clone)]
pub enum ResolvedValue {
    /// A single node
    Node(Node),
    /// A collection of nodes (from a "many" relationship)
    Collection(Vec<Node>),
    /// A scalar property value
    Scalar(serde_json::Value),
    /// Path could not be resolved (missing relationship or property)
    Missing,
    /// A lookup the walk depended on failed (a locked database, an unreadable
    /// schema), so whether the path resolves is unknown. Never cached, and
    /// never folded into `Missing`: a negative condition (`!has(node.epic)`)
    /// reads `Missing` as "no epic" and would fire on an infrastructure error.
    /// Condition evaluation reports it as `ConditionResult::Unresolved`.
    Unresolved(String),
}

/// Resolved paths, keyed by the node each was resolved from and then by the
/// path's segments.
///
/// The root id is part of the key because the same path means different
/// things from different nodes — `child_of` from one task is not `child_of`
/// from another. Keying on segments alone would silently serve one node's
/// answer for another's.
pub type PathCache = HashMap<String, HashMap<Vec<String>, ResolvedValue>>;

/// Where a root's walk starts.
enum Start {
    /// From this point in the path.
    Walk(Walk),
    /// Nowhere: an already-resolved prefix decides the whole path.
    Settled(ResolvedValue),
}

/// A walk in progress: where one root's resolution of a path has got to.
struct Walk {
    /// The node the path is being resolved for. Results are keyed by it.
    root_id: String,
    /// The single node the walk currently stands on.
    current: Node,
    /// Index of the next segment to resolve.
    position: usize,
}

/// Resolves dot-paths against the live data graph.
///
/// Created per work item in the RuleProcessor, and per scan in the
/// CronRunner. Caches resolved paths so overlapping paths across the
/// conditions of a rule cost one resolution.
pub struct GraphResolver {
    node_service: Arc<NodeService>,
    /// Cache: (root node id, path segments) → resolved value. See
    /// [`PathCache`] for why the root id is part of the key.
    cache: PathCache,
    /// The type the rule reading through this resolver was registered on
    /// (ADR-078).
    ///
    /// A traversed node of that type or a subtype of it is read at this type,
    /// exactly as the trigger node is: a Play registered on `task` sees a
    /// `bug` child's `task` fields, with `bug`-only values resolved through
    /// `maps_to`. Without it the child is built at its own scope and an
    /// extended value (`backlog`) reaches a base-scoped condition raw, never
    /// matching — a silent false, not an error.
    ///
    /// It is the rule's registered type, not the trigger's scope: a Play on
    /// `ticket` must read a related `bug` the same way whether a plain ticket
    /// or a bug fired it. `None` — a wildcard rule, which has no vocabulary
    /// of its own — reads every node at its own type.
    reading_type: Option<String>,
    /// Each node type's own `extends` chain, nearest-first. A node's
    /// inherited fields live in its ancestors' buckets, so every property
    /// read on a traversed node needs its chain, not just its type.
    chains: HashMap<String, Vec<String>>,
    /// The scope each concrete node type is read at under `reading_type`,
    /// built once per type. Cleared whenever `reading_type` changes.
    node_scopes: HashMap<String, Option<CelScope>>,
    /// What each relationship name resolves to from each type. A name's
    /// meaning depends only on the schemas, so it is resolved once per type
    /// however many nodes of that type the walk meets.
    hops: HashMap<(Option<String>, String), HopResolution>,
    /// How many path statements this resolver has run. A path resolved for a
    /// whole scan costs one, however many nodes the scan holds.
    statements: usize,
}

impl GraphResolver {
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self {
            node_service,
            cache: HashMap::new(),
            reading_type: None,
            chains: HashMap::new(),
            node_scopes: HashMap::new(),
            hops: HashMap::new(),
            statements: 0,
        }
    }

    /// Set the type resolved nodes are read at. See
    /// [`GraphResolver::reading_type`].
    pub fn with_reading_type(mut self, reading_type: Option<String>) -> Self {
        self.set_reading_type(reading_type);
        self
    }

    /// The service this resolver reads through — also what `.where(...)`
    /// item scoping needs for its schema lookups (`actions::BindingContext`).
    pub(crate) fn node_service(&self) -> &Arc<NodeService> {
        &self.node_service
    }

    /// Point an existing resolver at a different reading type.
    ///
    /// One resolver is reused across the rules of a work item, and each rule
    /// carries its own registered type. The path cache holds `ResolvedValue`s
    /// — raw `Node`s, not yet projected — so it stays valid across a change
    /// and is deliberately kept: projection happens at read time in
    /// `enrich_context`, after the cache is consulted.
    pub fn set_reading_type(&mut self, reading_type: Option<String>) {
        if self.reading_type != reading_type {
            self.reading_type = reading_type;
            self.node_scopes.clear();
        }
    }

    /// How many path statements this resolver has run against the store.
    pub fn statements_run(&self) -> usize {
        self.statements
    }

    /// Hand over everything this resolver resolved, keyed by root. A scan
    /// resolves its rules' paths for every node at once and passes the result
    /// to the work items it enqueues.
    pub fn into_cache(self) -> PathCache {
        self.cache
    }

    /// Start from paths already resolved for `root_id` — the ones a scan
    /// resolved for all of its nodes at once. Only that root's entries are
    /// taken: another root's answers are never this root's.
    pub fn seed(&mut self, root_id: &str, resolved: &PathCache) {
        if let Some(paths) = resolved.get(root_id) {
            self.cache
                .entry(root_id.to_string())
                .or_default()
                .extend(paths.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }

    fn cached(&self, root_id: &str, segments: &[String]) -> Option<&ResolvedValue> {
        self.cache.get(root_id)?.get(segments)
    }

    fn remember(&mut self, root_id: &str, segments: &[String], value: ResolvedValue) {
        self.cache
            .entry(root_id.to_string())
            .or_default()
            .insert(segments.to_vec(), value);
    }

    /// `node_type`'s own chain, nearest-first.
    ///
    /// A resolver failure is an error, not the type alone: reading only the
    /// node's own bucket misses every inherited field, and a missed field is
    /// exactly what a negative condition reads as "absent".
    async fn chain_of(&mut self, node_type: &str) -> Result<Vec<String>, String> {
        if let Some(chain) = self.chains.get(node_type) {
            return Ok(chain.clone());
        }
        let chain = self
            .node_service
            .resolve_type_chain(node_type)
            .await
            .map_err(|e| format!("failed to resolve the extends chain of '{node_type}': {e}"))?;
        self.chains.insert(node_type.to_string(), chain.clone());
        Ok(chain)
    }

    /// Read `node`'s property `key` across its own chain, so an inherited
    /// field resolves from its declaring ancestor's bucket.
    async fn node_property(
        &mut self,
        node: &Node,
        key: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let chain = self.chain_of(&node.node_type).await?;
        let chain: Vec<&str> = chain.iter().map(String::as_str).collect();
        Ok(get_node_property_at_scope(node, key, &chain))
    }

    /// What `segment` reads as on `node` itself, without leaving it: a core
    /// field, a derived attribute of its type, or a property. `None` when the
    /// node holds none of them, which is when the segment is tried as a
    /// relationship.
    ///
    /// A core Node field is not a property and lives in no bucket, so the
    /// property lookup cannot see it. Without the first check, walking to a
    /// related node and reading its identity — `node.child_of.id`, the shape
    /// an action needs to address that node — would not resolve.
    ///
    /// Core fields are checked before properties so these names mean the
    /// node's identity consistently, rather than being shadowed by a
    /// same-named user property on some types but not others. That holds for
    /// the root node's own first segment as well as for traversed nodes: a
    /// task storing a user property literally named `content` resolves
    /// `node.content` to the struct field. Core-wins is the deliberate choice
    /// — it matches what `node.id` already means in every CEL condition
    /// (`cel.rs`'s `is_core_key`).
    async fn own_value(
        &mut self,
        node: &Node,
        segment: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        if let Some(core) = core_field_value(node, segment) {
            return Ok(Some(core));
        }
        // The one core key that is not a struct field: the node's type and
        // every type it extends, from the schemas.
        if segment == crate::playbook::cel::TYPE_CHAIN_KEY {
            let chain = self.chain_of(&node.node_type).await?;
            return Ok(Some(serde_json::json!(chain)));
        }
        // A derived attribute of the node's type is computed from its
        // content, ahead of any property: nothing stored can stand in for it.
        let chain = self.chain_of(&node.node_type).await?;
        if let Some(attribute) = crate::models::CoreNodeType::derived_attribute_in(&chain, segment)
        {
            return Ok(Some(attribute.derive(&node.content)));
        }
        self.node_property(node, segment).await
    }

    /// A traversed node's CEL value (ADR-078).
    ///
    /// A node in the reading type's family — that type or a subtype of it —
    /// is read at that type, so a base-scoped Play sees a related subtype
    /// through its own vocabulary. Any other node is read at its own type:
    /// the reading type names no base to project it to, and filtering it by
    /// another type's fields would hide every field it has.
    ///
    /// `Err` when the scope cannot be built, rather than reading a raw bucket
    /// that misses inherited fields or leaving the path absent — absent is
    /// what a negative condition matches on.
    async fn node_value(&mut self, node: &Node) -> Result<Value, String> {
        if !self.node_scopes.contains_key(&node.node_type) {
            let chain = self.chain_of(&node.node_type).await?;
            let read_at = match &self.reading_type {
                Some(reading_type) if chain.contains(reading_type) => reading_type.clone(),
                _ => node.node_type.clone(),
            };
            let scope = CelScope::resolve(&self.node_service, &read_at, node)
                .await
                .map_err(|e| {
                    format!(
                        "failed to build the '{read_at}' scope for a '{}' node: {e}",
                        node.node_type
                    )
                })?;
            self.node_scopes.insert(node.node_type.clone(), scope);
        }
        let scope = self
            .node_scopes
            .get(&node.node_type)
            .and_then(Option::as_ref);
        Ok(scoped_node_value(node, scope))
    }

    /// [`Self::node_value`] for every node, or `Err` if any one cannot be
    /// read: a collection missing an item would make `.all(...)` vacuously
    /// true of the rest.
    async fn node_values(&mut self, nodes: &[Node]) -> Result<Vec<Value>, String> {
        let mut list = Vec::with_capacity(nodes.len());
        for node in nodes {
            list.push(self.node_value(node).await?);
        }
        Ok(list)
    }

    /// What `name` resolves to from `node_type`, resolved once per type.
    async fn hop(&mut self, node_type: Option<&str>, name: &str) -> Result<HopResolution, String> {
        let key = (node_type.map(str::to_string), name.to_string());
        if let Some(known) = self.hops.get(&key) {
            return Ok(known.clone());
        }
        let resolution = resolve_hop(&self.node_service, node_type, &RelationshipHop::fixed(name))
            .await
            .map_err(|e| format!("failed to resolve relationship '{name}': {e}"))?;
        self.hops.insert(key, resolution.clone());
        Ok(resolution)
    }

    /// The run of relationship hops `segments` names from a node of
    /// `node_type`: as many leading segments as the schemas alone can
    /// resolve.
    ///
    /// The run ends at the first segment that is not a relationship the
    /// schemas can name from here: a name the type does not declare (it may
    /// be a property of the node the walk reaches), or a schema-declared name
    /// after a hop with no declared target type (it can only be resolved from
    /// the concrete node that hop reaches). The walk picks up from there with
    /// the nodes in hand.
    async fn run_of_hops(
        &mut self,
        node_type: &str,
        segments: &[String],
    ) -> Result<Vec<ResolvedHop>, String> {
        let mut run = Vec::new();
        let mut current = Some(node_type.to_string());
        for segment in segments {
            match self.hop(current.as_deref(), segment).await? {
                HopResolution::Resolved(hop) => {
                    current = hop.far_type.clone();
                    run.push(hop);
                }
                HopResolution::Undeclared | HopResolution::TypeUnknown => break,
            }
        }
        Ok(run)
    }

    /// Resolve a dot-path starting from a root node.
    ///
    /// Each segment is read, in order, as:
    /// 1. a core field or a property of the node the walk stands on → Scalar
    /// 2. otherwise a relationship → the related node(s)
    ///
    /// A relationship reaching one node continues the walk from it; one
    /// declared `many`, or reaching several nodes, is a Collection and ends
    /// it.
    ///
    /// A relationship segment may name either side of an edge: `has_child`
    /// walks to the children, `child_of` to the parent. Reverse segments walk
    /// and chain exactly like forward ones (`node.assignee.email`).
    ///
    /// A failed lookup anywhere in the walk yields `Unresolved`, never
    /// `Missing`, and is not cached — see [`ResolvedValue::Unresolved`].
    pub async fn resolve_path(&mut self, root_node: &Node, segments: &[String]) -> ResolvedValue {
        self.resolve_path_for(std::slice::from_ref(root_node), segments)
            .await
            .remove(&root_node.id)
            .unwrap_or(ResolvedValue::Missing)
    }

    /// Resolve one dot-path for every root at once.
    ///
    /// The relationship hops of the path run as one statement for all the
    /// roots of a type ([`crate::db::SqliteStore::resolve_relationship_path`]),
    /// so the cost grows with the number of distinct paths, not with roots
    /// times hops. Returns each root's value, keyed by root id, and caches
    /// every one that resolved.
    pub async fn resolve_path_for(
        &mut self,
        roots: &[Node],
        segments: &[String],
    ) -> HashMap<String, ResolvedValue> {
        let mut results: HashMap<String, ResolvedValue> = HashMap::new();
        let mut walks: Vec<Walk> = Vec::new();

        for root in roots {
            if results.contains_key(&root.id) {
                continue;
            }
            if segments.is_empty() {
                results.insert(root.id.clone(), ResolvedValue::Node(root.clone()));
                continue;
            }
            if let Some(cached) = self.cached(&root.id, segments) {
                results.insert(root.id.clone(), cached.clone());
                continue;
            }
            match self.resume_point(root, segments) {
                Start::Walk(walk) => walks.push(walk),
                Start::Settled(value) => {
                    if !matches!(value, ResolvedValue::Unresolved(_)) {
                        self.remember(&root.id, segments, value.clone());
                    }
                    results.insert(root.id.clone(), value);
                }
            }
        }

        while !walks.is_empty() {
            walks = self.advance(walks, segments, &mut results).await;
        }
        results
    }

    /// Where a root's walk starts: at the root, or at the node its longest
    /// already-resolved prefix reached. Settled when a cached prefix already
    /// decides the path — nothing can be walked through a collection, a
    /// scalar or a missing relationship.
    fn resume_point(&self, root: &Node, segments: &[String]) -> Start {
        for position in (1..segments.len()).rev() {
            match self.cached(&root.id, &segments[..position]) {
                Some(ResolvedValue::Node(node)) => {
                    return Start::Walk(Walk {
                        root_id: root.id.clone(),
                        current: node.clone(),
                        position,
                    });
                }
                Some(
                    ResolvedValue::Collection(_)
                    | ResolvedValue::Scalar(_)
                    | ResolvedValue::Missing,
                ) => return Start::Settled(ResolvedValue::Missing),
                // Never cached — the arm exists for exhaustiveness.
                Some(ResolvedValue::Unresolved(reason)) => {
                    return Start::Settled(ResolvedValue::Unresolved(reason.clone()));
                }
                None => {}
            }
        }
        Start::Walk(Walk {
            root_id: root.id.clone(),
            current: root.clone(),
            position: 0,
        })
    }

    /// Finish a root's walk with `value` at `position`, the segment it was
    /// decided on. A value that is not the path's last segment makes the
    /// whole path `Missing`: nothing can be walked through a scalar or a
    /// collection.
    fn settle(
        &mut self,
        results: &mut HashMap<String, ResolvedValue>,
        root_id: &str,
        segments: &[String],
        position: usize,
        value: ResolvedValue,
    ) {
        // A lookup that failed says nothing about the path, so it is reported
        // but never remembered.
        if matches!(value, ResolvedValue::Unresolved(_)) {
            results.insert(root_id.to_string(), value);
            return;
        }
        let is_last = position + 1 == segments.len();
        self.remember(root_id, &segments[..=position], value.clone());
        let outcome = if is_last {
            value
        } else {
            ResolvedValue::Missing
        };
        if !is_last {
            self.remember(root_id, segments, outcome.clone());
        }
        results.insert(root_id.to_string(), outcome);
    }

    /// Move every walk forward by one run of hops: read the next segment off
    /// each walk's own node where it is a field, and resolve the rest as
    /// relationships, one statement per node type. Returns the walks that
    /// still have segments to resolve.
    async fn advance(
        &mut self,
        walks: Vec<Walk>,
        segments: &[String],
        results: &mut HashMap<String, ResolvedValue>,
    ) -> Vec<Walk> {
        // Walks whose next segment is a relationship, by the type of the node
        // they stand on: a name's meaning depends on that type alone.
        let mut by_type: Vec<(String, Vec<Walk>)> = Vec::new();
        for walk in walks {
            match self
                .own_value(&walk.current, &segments[walk.position])
                .await
            {
                Ok(Some(value)) => self.settle(
                    results,
                    &walk.root_id,
                    segments,
                    walk.position,
                    ResolvedValue::Scalar(value),
                ),
                Ok(None) => {
                    match by_type.iter_mut().find(|(node_type, group)| {
                        *node_type == walk.current.node_type && group[0].position == walk.position
                    }) {
                        Some((_, group)) => group.push(walk),
                        None => by_type.push((walk.current.node_type.clone(), vec![walk])),
                    }
                }
                Err(reason) => {
                    results.insert(walk.root_id, ResolvedValue::Unresolved(reason));
                }
            }
        }

        let mut continuing = Vec::new();
        for (node_type, group) in by_type {
            let position = group[0].position;
            let run = match self.run_of_hops(&node_type, &segments[position..]).await {
                Ok(run) => run,
                Err(reason) => {
                    for walk in group {
                        results.insert(walk.root_id, ResolvedValue::Unresolved(reason.clone()));
                    }
                    continue;
                }
            };
            if run.is_empty() {
                // Neither a field of the node nor a relationship of its type:
                // the ordinary way a path turns out missing. Not an error —
                // CEL renders it as a false condition.
                for walk in group {
                    self.settle(
                        results,
                        &walk.root_id,
                        segments,
                        position,
                        ResolvedValue::Missing,
                    );
                }
                continue;
            }

            // One statement walks the whole run from every node in the group.
            let mut start_ids: Vec<String> =
                group.iter().map(|walk| walk.current.id.clone()).collect();
            start_ids.sort_unstable();
            start_ids.dedup();
            let path = ResolvedPath { hops: run };
            self.statements += 1;
            let reach = match self
                .node_service
                .store()
                // An archived node is no target of a play (ADR-087 §2): it
                // is in no collection a condition counts or a `for_each`
                // iterates, and the walk does not pass through one.
                .resolve_relationship_path(&start_ids, &path, false)
                .await
            {
                Ok(reach) => reach,
                Err(e) => {
                    // A failed walk is not a statement about the path:
                    // whether "no epic" or "the epic lookup failed",
                    // `Missing` would let `!has(node.epic)` fire.
                    let reason = format!(
                        "failed to walk {} from '{node_type}' nodes: {e}",
                        segments[position..position + path.len()].join(".")
                    );
                    for walk in group {
                        results.insert(walk.root_id, ResolvedValue::Unresolved(reason.clone()));
                    }
                    continue;
                }
            };

            for walk in group {
                if let Some(next) = self
                    .follow_run(walk, &path, &reach, segments, results)
                    .await
                {
                    continuing.push(next);
                }
            }
        }
        continuing
    }

    /// Apply one run's results to one walk, hop by hop, until the walk is
    /// settled or the run is used up. Returns the walk when it still stands
    /// on a single node with segments left to resolve.
    ///
    /// A hop's shape follows its declaration, not how many nodes it reaches
    /// today. A relationship DECLARED `many` is a Collection whether it holds
    /// zero, one or twenty nodes: a cycle with exactly one issue is not "the
    /// issue itself", and a cycle with none yet is an ordinary state rather
    /// than a missing path. Inferring the shape from the row count would make
    /// `for_each`, `sum` and `count` fail an action — suspending the whole
    /// play — for those ordinary states.
    ///
    /// A relationship with no declared cardinality (a built-in) takes its
    /// shape from what it reaches: nothing is Missing, one node is that node,
    /// several are a Collection.
    ///
    /// Walking further into a collection is not supported by a dot-path: the
    /// path is `Missing` there, whatever the later hops reached.
    async fn follow_run(
        &mut self,
        walk: Walk,
        run: &ResolvedPath,
        reach: &crate::db::PathReach,
        segments: &[String],
        results: &mut HashMap<String, ResolvedValue>,
    ) -> Option<Walk> {
        let Walk {
            root_id,
            current,
            position,
        } = walk;
        // The statement keys its rows by the node the run started from. Every
        // later hop's rows are still keyed by that node, so they are only
        // this walk's own while the walk has stood on a single node at every
        // hop so far — which is exactly when the loop below is still running.
        let start_id = current.id.clone();
        let mut current = current;

        for (index, hop) in run.hops.iter().enumerate() {
            let at = position + index;
            // The node the previous hop reached may hold this segment as a
            // field of its own. A field wins over a relationship.
            if index > 0 {
                match self.own_value(&current, &segments[at]).await {
                    Ok(Some(value)) => {
                        self.settle(
                            results,
                            &root_id,
                            segments,
                            at,
                            ResolvedValue::Scalar(value),
                        );
                        return None;
                    }
                    Ok(None) => {}
                    Err(reason) => {
                        results.insert(root_id, ResolvedValue::Unresolved(reason));
                        return None;
                    }
                }
            }

            let related = reach.at(index, &start_id);
            // A declaration with no target type may point at any type, so it
            // names a relationship of this node only when an edge actually
            // reaches it. Otherwise the segment resolves to nothing here.
            let declared_many = hop.declared_many && !(hop.untyped && related.is_empty());
            let value = match related {
                nodes if declared_many => ResolvedValue::Collection(nodes.to_vec()),
                [] => ResolvedValue::Missing,
                [node] => ResolvedValue::Node(node.clone()),
                nodes => ResolvedValue::Collection(nodes.to_vec()),
            };
            match value {
                ResolvedValue::Node(node) if at + 1 < segments.len() => {
                    self.remember(
                        &root_id,
                        &segments[..=at],
                        ResolvedValue::Node(node.clone()),
                    );
                    current = node;
                }
                value => {
                    self.settle(results, &root_id, segments, at, value);
                    return None;
                }
            }
        }

        Some(Walk {
            root_id,
            current,
            position: position + run.len(),
        })
    }

    /// Resolve a collection path and return the collection nodes.
    ///
    /// `Err` when the walk was [`ResolvedValue::Unresolved`]: an empty list
    /// there would read as "no children", which a negative condition matches.
    pub async fn resolve_collection(
        &mut self,
        root_node: &Node,
        collection: &ExtractedPath,
    ) -> Result<Vec<Node>, String> {
        // The collection path is like ["node", "tasks"] — skip "node" (the root)
        let segments = &collection.segments;
        if segments.len() < 2 {
            return Ok(vec![]);
        }

        match self.resolve_path(root_node, &segments[1..]).await {
            ResolvedValue::Collection(nodes) => Ok(nodes),
            ResolvedValue::Node(n) => Ok(vec![n]),
            ResolvedValue::Unresolved(reason) => Err(reason),
            ResolvedValue::Scalar(_) | ResolvedValue::Missing => Ok(vec![]),
        }
    }

    /// Resolve every graph path `paths` and `collections` name, for all of
    /// `roots` at once, so a later [`Self::enrich_context`] for any of them
    /// reads the cache.
    ///
    /// This is what a scheduled scan calls: one statement per distinct path
    /// for the whole scan, rather than a walk per node. A path that fails to
    /// resolve here is simply not cached, and is resolved again — and
    /// reported — when its node is evaluated.
    pub async fn resolve_ahead(
        &mut self,
        roots: &[Node],
        paths: &[ExtractedPath],
        collections: &[CollectionPath],
    ) {
        let mut distinct: Vec<&[String]> = Vec::new();
        let graph_paths = paths
            .iter()
            .chain(collections.iter().map(|c| &c.collection))
            .filter(|path| path.root == "node" && path.segments.len() >= 2)
            .map(|path| &path.segments[1..]);
        for segments in graph_paths {
            if !distinct.contains(&segments) {
                distinct.push(segments);
            }
        }
        // Longest first: a walk remembers every hop it passes, so resolving
        // `story.epic` also answers `story`, and the shorter path then costs
        // no statement of its own.
        distinct.sort_by_key(|segments| std::cmp::Reverse(segments.len()));
        for segments in distinct {
            self.resolve_path_for(roots, segments).await;
        }
    }

    /// Build an enriched CEL context with graph-resolved paths.
    ///
    /// Takes the base node and extracted paths, resolves each path against
    /// the graph, and injects the resolved values as nested CEL Maps.
    ///
    /// `Err` when any path's walk failed on a lookup error. A missing key is
    /// how a path reads as absent, so no partial context is returned: the
    /// failed path would be absent from it, and a negative condition over it
    /// would match.
    pub async fn enrich_context(
        &mut self,
        root_node: &Node,
        paths: &[ExtractedPath],
        collections: &[CollectionPath],
    ) -> Result<HashMap<Vec<String>, Value>, String> {
        let mut resolved_values: HashMap<Vec<String>, Value> = HashMap::new();

        // Resolve flat paths (skip "node" root — those beyond property-level)
        for path in paths {
            if path.root != "node" || path.segments.len() < 2 {
                continue;
            }

            // `node.status` is a property, already in the base CEL context —
            // resolving it again would be wasted work.
            //
            // The check is "is this a property?", not "is this path short?". A
            // two-segment path used to be a property by definition, because a
            // relationship needed a further hop to produce a value. A terminal
            // reverse segment (`node.assignee`) breaks that: the related node
            // IS the value, so a length test would skip the very paths this
            // resolver exists to answer.
            if path.segments.len() == 2
                && self
                    .node_property(root_node, &path.segments[1])
                    .await?
                    .is_some()
            {
                continue;
            }

            // Resolve the relationship chain (skip "node" prefix)
            let segments = &path.segments[1..];
            match self.resolve_path(root_node, segments).await {
                ResolvedValue::Node(n) => {
                    let value = self.node_value(&n).await?;
                    resolved_values.insert(path.segments.clone(), value);
                }
                ResolvedValue::Scalar(v) => {
                    // A field of a traversed node reads through that node's
                    // scoped value — projected and `maps_to`-resolved exactly
                    // as the node itself would be — so `node.child_of.state`
                    // and `node.child_of` agree about the same node.
                    if segments.len() > 1 {
                        let (owner_path, field) = segments.split_at(segments.len() - 1);
                        match self.resolve_path(root_node, owner_path).await {
                            ResolvedValue::Node(owner) => {
                                if let Value::Map(owner) = self.node_value(&owner).await? {
                                    if let Some(value) = owner.map.get(&key(&field[0])) {
                                        resolved_values
                                            .insert(path.segments.clone(), value.clone());
                                    }
                                }
                                continue;
                            }
                            ResolvedValue::Unresolved(reason) => return Err(reason),
                            _ => {}
                        }
                    }
                    // Defensive: a scalar is only ever reached through a node,
                    // so a multi-segment path always takes the branch above.
                    resolved_values.insert(path.segments.clone(), json_to_cel(&v));
                }
                ResolvedValue::Collection(nodes) => {
                    let list = self.node_values(&nodes).await?;
                    resolved_values.insert(path.segments.clone(), Value::List(list.into()));
                }
                ResolvedValue::Missing => {
                    // Missing path → will evaluate to false via NoSuchKey in CEL
                }
                ResolvedValue::Unresolved(reason) => return Err(reason),
            }
        }

        // Resolve collection paths
        for coll in collections {
            if coll.collection.root != "node" {
                continue;
            }
            let nodes = self.resolve_collection(root_node, &coll.collection).await?;
            // Load-bearing: an empty collection leaves the key ABSENT rather
            // than injecting an empty list.
            //
            // The mechanism is the absence, not CEL semantics: CEL's `.all()`
            // over an empty list returns `true` (vacuous truth), exactly as the
            // spec says. What produces `false` is that the key is missing, so
            // evaluation raises `NoSuchKey`, which `evaluate_conditions_at_scope`
            // maps to `Fail`. Inserting an empty list here would hand `.all()`
            // a real empty list, it would return vacuously true, and a childless
            // parent would auto-complete itself (ADR-079 §4).
            if !nodes.is_empty() {
                let mut list = self.node_values(&nodes).await?;
                let derived = derived_names_read_on_items(collections, &coll.collection.segments);
                absent_derived_read_as_null(&mut list, &derived);
                resolved_values.insert(coll.collection.segments.clone(), Value::List(list.into()));
            }
        }

        Ok(resolved_values)
    }
}

/// The derived attributes the comprehensions over the collection at
/// `segments` read on their items (`c.checked` in
/// `node.has_child.exists(c, c.checked == false)`).
fn derived_names_read_on_items<'a>(
    collections: &'a [CollectionPath],
    segments: &[String],
) -> Vec<&'a str> {
    let mut names: Vec<&str> = collections
        .iter()
        .filter(|coll| coll.collection.segments == segments)
        .flat_map(|coll| {
            coll.item_paths
                .iter()
                .filter(|path| path.root == coll.iter_var && path.segments.len() >= 2)
                .map(|path| path.segments[1].as_str())
        })
        .filter(|name| !crate::models::DerivedAttribute::declared_as(name).is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Give each item that has no value for a derived attribute in `names` a
/// `null` for it.
///
/// A collection reached through a built-in relationship holds nodes of any
/// type: a task's children are its checkboxes and its notes. A comprehension
/// that reads a derived attribute on them would otherwise fail on the first
/// item whose type does not derive it, and a failed condition is false
/// whatever the other items hold. With `null`, `c.checked == false` is simply
/// false for a note, so the comprehension answers for the items that do have
/// the attribute. Only a derived attribute the comprehension reads is filled
/// in, and only where it is absent.
fn absent_derived_read_as_null(items: &mut [Value], names: &[&str]) {
    if names.is_empty() {
        return;
    }
    for item in items {
        let Value::Map(map) = item else {
            continue;
        };
        if names.iter().all(|name| map.map.contains_key(&key(name))) {
            continue;
        }
        let mut filled = (*map.map).clone();
        for name in names {
            filled.entry(key(name)).or_insert(Value::Null);
        }
        *item = Value::Map(cel_interpreter::objects::Map {
            map: Arc::new(filled),
        });
    }
}

/// A core Node field read by name, or `None` if the name is not one.
///
/// These are node identity/metadata, not schema properties: they live in their
/// own struct fields rather than in any type bucket, so no property lookup can
/// reach them. The same set `cel.rs` exposes on a CEL `node` map (`is_core_key`)
/// — the two must agree, or a name resolves in a condition but not in the
/// action binding that acts on it. `type_chain` is the exception: it comes
/// from the schemas, so `GraphResolver::own_value` reads it.
fn core_field_value(node: &Node, name: &str) -> Option<serde_json::Value> {
    match name {
        "id" => Some(serde_json::Value::String(node.id.clone())),
        "node_type" => Some(serde_json::Value::String(node.node_type.clone())),
        "content" => Some(serde_json::Value::String(node.content.clone())),
        "version" => Some(serde_json::Value::from(node.version)),
        _ => None,
    }
}

/// The declared item type of the collection `segments` reaches from a node of
/// `start_type`: the far-end type of the last relationship walked.
///
/// This is the type a `.where(...)` predicate is authored against — validated
/// against it at save time (`playbook::validation`) and read at it per item at
/// run time (`actions::BindingContext`), so an `issue` reached through
/// `cycle.tasks → task` is read at `task` scope with its extended values
/// resolved through `maps_to`. Both sides call this one function, which
/// resolves the segments as the [`RelationshipPath`] they are
/// ([`crate::ops::path_ops::resolve_path`]), so the two cannot drift apart.
///
/// The START type can differ, though: validation starts from the rule's
/// registered trigger type, the runtime from the trigger node's concrete type.
/// Relationships are inherited down the `extends` chain, so the two agree
/// unless a subtype re-declares a same-named relationship with a different
/// target — the one case where a predicate could be evaluated at a type other
/// than the one it was validated against.
///
/// `Ok(None)` means the path has no declared item type — it is empty, names
/// something that is not a relationship, ends on a built-in structural
/// relationship (any type may sit at either end of one), or crosses a
/// relationship with no `target_type`. `Err` is a schema lookup failure,
/// never folded into `None`.
pub(crate) async fn declared_collection_type(
    node_service: &NodeService,
    start_type: &str,
    segments: &[&str],
) -> Result<Option<String>, String> {
    use crate::ops::path_ops::{resolve_path, PathResolveError};

    if segments.is_empty() {
        return Ok(None);
    }
    let path = RelationshipPath::from_names(segments.iter().copied());
    match resolve_path(node_service, Some(start_type), &path).await {
        Ok(resolved) => Ok(resolved.far_type().map(str::to_string)),
        Err(PathResolveError::Lookup { error, .. }) => Err(error),
        Err(_) => Ok(None),
    }
}

/// Get a property value from a node, checking multiple formats.
///
/// NodeSpace stores properties in a type-namespaced format:
/// `{"task": {"status": "open"}}` — so we check inside the type namespace too.
/// Also checks `custom:key` namespace prefix format.
///
/// Internal `_`-prefixed bookkeeping keys (`_seed`, `_schemaVersion`,
/// `_playbookChainDepth`, ...) are excluded, same convention and same check
/// as `node_to_cel_value` in `cel.rs` (see `NodeService::normalize_flat_properties_to_namespace`
/// for where the convention originates). NOTE: Parallel logic exists in
/// `cel::node_to_cel_value` — if the property storage format changes, both
/// must be updated.
///
/// `pub(crate)`: also reused by `actions::sum_numeric_field` for the
/// `sum(collection, field)` action-binding call, so a collection item's field
/// is read through the same type-namespace-aware lookup `for_each`'s
/// resolved items are subject to, instead of a naive flat lookup that would
/// silently read `None` for every real (type-namespaced) node.
pub(crate) fn get_node_property(node: &Node, key: &str) -> Option<serde_json::Value> {
    get_node_property_at_scope(node, key, std::slice::from_ref(&node.node_type.as_str()))
}

/// Read a node property, projected to an explicit scope chain (ADR-078).
///
/// The general form of [`get_node_property`], which is the node's-own-scope
/// case. Buckets are searched nearest-scope-first, so an inherited field
/// resolves from its declaring ancestor's bucket while a field outside the
/// chain does not resolve at all.
///
/// A cleared field is stored as `null` and is returned as `Some(Null)`, not
/// `None`. This reader also feeds action bindings, where a cleared field
/// binds to `null`; `None` would turn that into a missing path, which fails
/// the action. A condition never sees the `null`: its values come from
/// `cel::scoped_node_value`, which leaves a cleared field out.
fn get_node_property_at_scope(
    node: &Node,
    key: &str,
    scope_chain: &[&str],
) -> Option<serde_json::Value> {
    if let Some(obj) = node.properties.as_object() {
        // Direct match and type-namespaced match both look up `key` verbatim,
        // so the raw stored key being checked is `key` itself in both cases.
        if !key.starts_with('_') {
            // Direct match (e.g., key "status" on {"status": "open"})
            if let Some(val) = obj.get(key) {
                // Don't return a type namespace wrapper as a property — not
                // the node's own, nor any ancestor bucket in scope.
                let is_namespace_wrapper =
                    val.is_object() && (key == node.node_type || scope_chain.contains(&key));
                if !is_namespace_wrapper {
                    return Some(val.clone());
                }
            }

            // Check inside each type-namespaced bucket in scope, nearest
            // first (e.g., {"task": {"status": "open"}}).
            for scope in scope_chain {
                if let Some(type_obj) = obj.get(*scope).and_then(|v| v.as_object()) {
                    if let Some(val) = type_obj.get(key) {
                        return Some(val.clone());
                    }
                }
            }
        }

        // Try with colon namespace prefix (e.g., "status" → "custom:status").
        // The filter checks the raw stored key `k` (with its colon prefix),
        // not the colon-stripped bare name -- matching node_to_cel_value's
        // rule that a leading `_` only makes a key internal when it's on the
        // WHOLE stored key. "custom:_internal" is a legal, visible user
        // field, not bookkeeping, even though its bare name starts with `_`.
        for (k, v) in obj {
            if k.starts_with('_') {
                continue;
            }
            if let Some(bare) = k.find(':').map(|i| &k[i + 1..]) {
                if bare == key {
                    return Some(v.clone());
                }
            }
        }

        // A prefixed field inside a bucket in scope, nearest first —
        // `update_node` stores `custom:x` on a core type under that type's
        // namespace, not at the top level.
        for scope in scope_chain {
            let Some(type_obj) = obj.get(*scope).and_then(|v| v.as_object()) else {
                continue;
            };
            for (k, v) in type_obj {
                if k.starts_with('_') {
                    continue;
                }
                if k.find(':').map(|i| &k[i + 1..]) == Some(key) {
                    return Some(v.clone());
                }
            }
        }
    }
    None
}

/// Inject resolved graph values into a CEL node Map.
///
/// Given a base node CEL value and resolved paths, creates nested Maps
/// so that `node.story.epic.status` resolves correctly during evaluation.
pub fn inject_resolved_paths(
    base_node_value: &Value,
    resolved: &HashMap<Vec<String>, Value>,
) -> Value {
    if resolved.is_empty() {
        return base_node_value.clone();
    }

    // Start with the base node map
    let mut map = match base_node_value {
        Value::Map(m) => (*m.map).clone(),
        _ => return base_node_value.clone(),
    };

    // For each resolved path, inject into the nested structure.
    // Path like ["node", "story", "epic", "status"] with resolved value "active":
    // We need to set node.story.epic.status = "active" and node.story.epic = Map{...}
    // and node.story = Map{...}
    // Shortest paths first. A terminal write replaces whatever is at its key,
    // so injecting `node.plan` after `node.plan.spec` would drop the `spec`
    // just nested under it — and `resolved` is a HashMap, so which came first
    // varied run to run. Deeper paths then merge into the maps shorter ones
    // left behind; a deeper path under a scalar or list replaces it with a
    // map, since no condition can walk a further hop through either.
    let mut ordered: Vec<_> = resolved.iter().collect();
    ordered.sort_by_key(|(path, _)| path.len());
    for (path, value) in ordered {
        if path.len() < 2 || path[0] != "node" {
            continue;
        }

        // Build nested maps from the outside in
        // For ["node", "story", "epic", "status"] → inject at map["story"]["epic"]["status"]
        inject_nested_value(&mut map, &path[1..], value);
    }

    Value::Map(cel_interpreter::objects::Map { map: Arc::new(map) })
}

/// Recursively inject a value at a nested path in a CEL Map.
fn inject_nested_value(
    map: &mut HashMap<cel_interpreter::objects::Key, Value>,
    segments: &[String],
    value: &Value,
) {
    if segments.is_empty() {
        return;
    }

    if segments.len() == 1 {
        // Terminal segment — set the value directly
        map.insert(key(&segments[0]), value.clone());
        return;
    }

    // Non-terminal segment — ensure intermediate Map exists, then recurse
    let k = key(&segments[0]);
    let existing = map.get(&k).cloned();
    let mut inner_map = match existing {
        Some(Value::Map(m)) => (*m.map).clone(),
        _ => HashMap::new(),
    };

    inject_nested_value(&mut inner_map, &segments[1..], value);

    map.insert(
        k,
        Value::Map(cel_interpreter::objects::Map {
            map: Arc::new(inner_map),
        }),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use cel_interpreter::objects::Key;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // inject_resolved_paths / inject_nested_value — pure unit tests (no DB)
    // -----------------------------------------------------------------------

    fn make_cel_map(pairs: Vec<(&str, Value)>) -> Value {
        let map: HashMap<Key, Value> = pairs
            .into_iter()
            .map(|(k, v)| (Key::String(Arc::new(k.to_string())), v))
            .collect();
        Value::Map(cel_interpreter::objects::Map { map: Arc::new(map) })
    }

    fn get_map_field(val: &Value, field: &str) -> Option<Value> {
        match val {
            Value::Map(m) => m
                .map
                .get(&Key::String(Arc::new(field.to_string())))
                .cloned(),
            _ => None,
        }
    }

    #[test]
    fn inject_single_level_path() {
        let base = make_cel_map(vec![("id", Value::String(Arc::new("n1".to_string())))]);
        let mut resolved = HashMap::new();
        resolved.insert(
            vec!["node".to_string(), "status".to_string()],
            Value::String(Arc::new("open".to_string())),
        );

        let result = inject_resolved_paths(&base, &resolved);
        assert_eq!(
            get_map_field(&result, "status"),
            Some(Value::String(Arc::new("open".to_string())))
        );
        // Original field preserved
        assert_eq!(
            get_map_field(&result, "id"),
            Some(Value::String(Arc::new("n1".to_string())))
        );
    }

    #[test]
    fn inject_nested_path_creates_intermediate_maps() {
        let base = make_cel_map(vec![("id", Value::String(Arc::new("n1".to_string())))]);
        let mut resolved = HashMap::new();
        resolved.insert(
            vec![
                "node".to_string(),
                "story".to_string(),
                "epic".to_string(),
                "status".to_string(),
            ],
            Value::String(Arc::new("active".to_string())),
        );

        let result = inject_resolved_paths(&base, &resolved);

        // node.story should be a Map
        let story = get_map_field(&result, "story");
        assert!(story.is_some(), "story should exist");
        // node.story.epic should be a Map
        let epic = get_map_field(&story.unwrap(), "epic");
        assert!(epic.is_some(), "epic should exist");
        // node.story.epic.status should be "active"
        let status = get_map_field(&epic.unwrap(), "status");
        assert_eq!(status, Some(Value::String(Arc::new("active".to_string()))));
    }

    #[test]
    fn inject_multiple_paths_same_prefix() {
        let base = make_cel_map(vec![]);
        let mut resolved = HashMap::new();
        resolved.insert(
            vec!["node".to_string(), "story".to_string(), "title".to_string()],
            Value::String(Arc::new("My Story".to_string())),
        );
        resolved.insert(
            vec![
                "node".to_string(),
                "story".to_string(),
                "status".to_string(),
            ],
            Value::String(Arc::new("active".to_string())),
        );

        let result = inject_resolved_paths(&base, &resolved);
        let story = get_map_field(&result, "story").unwrap();
        assert_eq!(
            get_map_field(&story, "title"),
            Some(Value::String(Arc::new("My Story".to_string())))
        );
        assert_eq!(
            get_map_field(&story, "status"),
            Some(Value::String(Arc::new("active".to_string())))
        );
    }

    #[test]
    fn inject_empty_resolved_returns_base() {
        let base = make_cel_map(vec![("id", Value::Int(42))]);
        let resolved = HashMap::new();
        let result = inject_resolved_paths(&base, &resolved);
        assert_eq!(get_map_field(&result, "id"), Some(Value::Int(42)));
    }

    #[test]
    fn inject_non_node_root_paths_ignored() {
        let base = make_cel_map(vec![]);
        let mut resolved = HashMap::new();
        // Path with root "trigger" (not "node") should be ignored
        resolved.insert(
            vec![
                "trigger".to_string(),
                "property".to_string(),
                "key".to_string(),
            ],
            Value::String(Arc::new("status".to_string())),
        );
        let result = inject_resolved_paths(&base, &resolved);
        // Should not inject anything
        assert!(get_map_field(&result, "property").is_none());
    }

    /// A relationship and a path through it resolved together: the shorter
    /// terminal write must not clobber the deeper one's nested value.
    ///
    /// `resolved` is a HashMap with a fresh random seed each construction, so
    /// the unfixed code dropped the nested key on roughly half of iterations.
    #[test]
    fn inject_keeps_a_deeper_path_under_a_shorter_one() {
        for _ in 0..64 {
            let base = make_cel_map(vec![]);
            let plan = make_cel_map(vec![("plan_status", Value::String(Arc::new("x".into())))]);
            let spec = make_cel_map(vec![("spec_status", Value::String(Arc::new("y".into())))]);
            let mut resolved = HashMap::new();
            resolved.insert(vec!["node".to_string(), "plan".to_string()], plan);
            resolved.insert(
                vec!["node".to_string(), "plan".to_string(), "spec".to_string()],
                spec.clone(),
            );

            let result = inject_resolved_paths(&base, &resolved);
            let plan = get_map_field(&result, "plan").expect("plan injected");
            assert!(get_map_field(&plan, "plan_status").is_some());
            assert_eq!(get_map_field(&plan, "spec"), Some(spec));
        }
    }

    #[test]
    fn inject_list_value() {
        let base = make_cel_map(vec![]);
        let list = Value::List(
            vec![
                Value::String(Arc::new("a".to_string())),
                Value::String(Arc::new("b".to_string())),
            ]
            .into(),
        );
        let mut resolved = HashMap::new();
        resolved.insert(vec!["node".to_string(), "tasks".to_string()], list.clone());
        let result = inject_resolved_paths(&base, &resolved);
        let tasks = get_map_field(&result, "tasks");
        assert!(matches!(tasks, Some(Value::List(_))));
    }

    // -----------------------------------------------------------------------
    // get_node_property — unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn get_property_direct_key() {
        let node = crate::models::Node {
            id: "n1".to_string(),
            node_type: "task".to_string(),
            content: "".to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: json!({"status": "open"}),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };
        assert_eq!(get_node_property(&node, "status"), Some(json!("open")));
        assert_eq!(get_node_property(&node, "missing"), None);
    }

    #[test]
    fn get_property_with_namespace_prefix() {
        let node = crate::models::Node {
            id: "n1".to_string(),
            node_type: "task".to_string(),
            content: "".to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: json!({"custom:amount": 1500}),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };
        // "amount" should match "custom:amount"
        assert_eq!(get_node_property(&node, "amount"), Some(json!(1500)));
    }

    #[test]
    fn get_property_with_type_namespace() {
        // DB-stored format: properties are wrapped under the node_type key
        let node = crate::models::Node {
            id: "n1".to_string(),
            node_type: "task".to_string(),
            content: "".to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: json!({"task": {"status": "open", "priority": "high"}}),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };
        assert_eq!(get_node_property(&node, "status"), Some(json!("open")));
        assert_eq!(get_node_property(&node, "priority"), Some(json!("high")));
        // "task" itself should NOT be returned as a property (it's the namespace wrapper)
        assert_eq!(get_node_property(&node, "task"), None);
        assert_eq!(get_node_property(&node, "missing"), None);
    }

    /// Parity with `cel::node_to_cel_value`: a prefixed field stored inside
    /// the type bucket resolves by its bare name, and an unprefixed field of
    /// the same bare name wins.
    #[test]
    fn get_property_strips_prefix_inside_the_type_namespace() {
        let node = crate::models::Node {
            id: "n1".to_string(),
            node_type: "task".to_string(),
            content: "".to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: json!({"task": {
                "status": "open",
                "custom:status": "shadow",
                "custom:review_notes": "ran the tests",
            }}),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };
        assert_eq!(
            get_node_property(&node, "review_notes"),
            Some(json!("ran the tests"))
        );
        assert_eq!(get_node_property(&node, "status"), Some(json!("open")));
    }

    /// Parity with `cel::node_to_cel_value_at_scope` across an `extends`
    /// chain: an ancestor bucket's unprefixed key beats a nearer bucket's
    /// prefixed one.
    #[test]
    fn get_property_prefers_unprefixed_keys_across_the_whole_scope_chain() {
        let node = crate::models::Node {
            id: "n1".to_string(),
            node_type: "issue".to_string(),
            content: "".to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: json!({
                "issue": {"custom:status": "shadow"},
                "task": {"status": "open"},
            }),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };
        assert_eq!(
            get_node_property_at_scope(&node, "status", &["issue", "task"]),
            Some(json!("open"))
        );
    }

    #[test]
    fn get_property_excludes_underscore_prefixed_keys() {
        // Internal bookkeeping keys are stored both flat and inside the type
        // namespace, mirroring NodeService::normalize_flat_properties_to_namespace.
        let node = crate::models::Node {
            id: "n1".to_string(),
            node_type: "task".to_string(),
            content: "".to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: json!({
                "_playbookChainDepth": 3,
                "task": {"status": "open", "_seed": "abc123"},
                "custom:_internal": "visible-to-user"
            }),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        };

        // Flat internal key is excluded.
        assert_eq!(get_node_property(&node, "_playbookChainDepth"), None);
        // Internal key nested inside the type namespace is excluded.
        assert_eq!(get_node_property(&node, "_seed"), None);
        // A normal property alongside an internal one in the same namespace
        // is unaffected.
        assert_eq!(get_node_property(&node, "status"), Some(json!("open")));
        // A colon-namespaced field is not internal just because its bare
        // name (after stripping the namespace) starts with `_` -- only a
        // leading `_` on the whole stored key means bookkeeping. This
        // matches node_to_cel_value's identical rule.
        assert_eq!(
            get_node_property(&node, "_internal"),
            Some(json!("visible-to-user"))
        );
    }

    // -----------------------------------------------------------------------
    // ResolvedValue — basic enum tests
    // -----------------------------------------------------------------------

    #[test]
    fn resolved_value_missing_is_default() {
        let rv = ResolvedValue::Missing;
        assert!(matches!(rv, ResolvedValue::Missing));
    }

    #[test]
    fn resolved_value_scalar() {
        let rv = ResolvedValue::Scalar(json!("hello"));
        match rv {
            ResolvedValue::Scalar(v) => assert_eq!(v, json!("hello")),
            _ => panic!("expected Scalar"),
        }
    }

    // -----------------------------------------------------------------------
    // Integration tests with real NodeService (requires multi_thread runtime)
    // -----------------------------------------------------------------------

    mod integration {
        use super::*;
        use crate::db::SqliteStore;
        use crate::models::Node;
        use crate::services::NodeService;
        use serde_json::json;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        async fn create_schema(
            svc: &Arc<NodeService>,
            type_name: &str,
            relationships: serde_json::Value,
        ) {
            let schema_node = Node::new_with_id(
                type_name.to_string(),
                "schema".to_string(),
                type_name.to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": format!("{} schema", type_name),
                    "fields": [{"name": "status", "type": "text"}, {"name": "title", "type": "text"}]
                }),
            );
            svc.create_node(schema_node)
                .await
                .unwrap_or_else(|_| panic!("Failed to create schema '{}'", type_name));

            // Declarations go through the real write path (relationship-table
            // rows), matching production storage.
            let declarations: Vec<crate::models::schema::SchemaRelationship> =
                serde_json::from_value(relationships)
                    .unwrap_or_else(|e| panic!("Invalid relationships fixture: {e}"));
            if !declarations.is_empty() {
                svc.set_schema_relationships(type_name, &declarations)
                    .await
                    .unwrap_or_else(|e| {
                        panic!("Failed to declare relationships on '{}': {e}", type_name)
                    });
            }
        }

        fn make_node(id: &str, node_type: &str, props: serde_json::Value) -> Node {
            Node {
                id: id.to_string(),
                node_type: node_type.to_string(),
                content: format!("{} content", id),
                version: 1,
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                properties: props,
                mentions: vec![],
                mentioned_in: vec![],
                title: Some(format!("{} title", id)),
                lifecycle_status: "active".to_string(),
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn resolve_property_on_root_node() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-task", json!([])).await;

            let node = make_node(
                "a5fab10d-1e9c-5fb3-89f8-1f9288a0d2e7",
                "gr-task",
                json!({"status": "open"}),
            );
            svc.create_node(node.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&node, &["status".to_string()]).await;
            match result {
                ResolvedValue::Scalar(v) => assert_eq!(v, json!("open")),
                other => panic!("expected Scalar, got {:?}", other),
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn resolve_missing_property_returns_missing() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-task2", json!([])).await;

            let node = make_node(
                "82cead27-2991-5a3d-8822-dc0d548c481b",
                "gr-task2",
                json!({"status": "open"}),
            );
            svc.create_node(node.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            // "nonexistent" is neither a property nor a relationship
            let result = resolver
                .resolve_path(&node, &["nonexistent".to_string()])
                .await;
            assert!(matches!(result, ResolvedValue::Missing));
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn resolve_single_hop_relationship() {
            let (svc, _tmp) = create_test_service().await;

            // Create schemas: gr-story has no rels, gr-issue -> story
            create_schema(&svc, "gr-story", json!([])).await;
            create_schema(
                &svc,
                "gr-issue",
                json!([{
                    "name": "story",
                    "targetType": "gr-story",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            // Create nodes
            let story = make_node(
                "c7c9cf0b-71a4-5bc1-833d-60e538bd42b7",
                "gr-story",
                json!({"status": "active"}),
            );
            svc.create_node(story.clone()).await.unwrap();

            let issue = make_node(
                "8c6a5e17-6d57-5ef5-8a93-cab9b3cb5089",
                "gr-issue",
                json!({"status": "open"}),
            );
            svc.create_node(issue.clone()).await.unwrap();

            // Create relationship
            svc.create_relationship(
                "8c6a5e17-6d57-5ef5-8a93-cab9b3cb5089",
                "story",
                "c7c9cf0b-71a4-5bc1-833d-60e538bd42b7",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&issue, &["story".to_string()]).await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "c7c9cf0b-71a4-5bc1-833d-60e538bd42b7"),
                other => panic!("expected Node, got {:?}", other),
            }
        }

        /// A failed related-node fetch is `Unresolved`, never `Missing`, and a
        /// negative condition over it neither passes nor fails. Before this,
        /// the error folded into `Missing` and `!has(node.story)` fired on a
        /// locked or broken database exactly as on a genuinely unlinked issue.
        /// Forces a real failure by dropping the `relationship` table, as
        /// `a_resolver_db_error_is_propagated_not_folded_into_none` does.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_failed_fetch_is_unresolved_not_missing() {
            use crate::db::events::DomainEvent;
            use crate::playbook::cel::{
                evaluate_conditions_at_scope, CompiledCondition, ConditionResult,
            };

            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-story-err", json!([])).await;
            create_schema(
                &svc,
                "gr-issue-err",
                json!([{
                    "name": "story",
                    "targetType": "gr-story-err",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;
            let issue = make_node(
                "17d5e68b-85b7-5f36-81fd-b137954515bd",
                "gr-issue-err",
                json!({"status": "open"}),
            );
            svc.create_node(issue.clone()).await.unwrap();

            let conditions = vec![CompiledCondition::compile("!has(node.story)").unwrap()];
            let event = DomainEvent::NodeCreated {
                node_type: "gr-issue-err".to_string(),
                node_id: issue.id.clone(),
            };

            // Control: with the database healthy, an unlinked issue matches.
            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let healthy = evaluate_conditions_at_scope(
                &conditions,
                &issue,
                &event,
                Some(&mut resolver),
                None,
            )
            .await;
            assert!(
                matches!(healthy, ConditionResult::Pass),
                "an issue with no story must match !has(node.story), got {healthy:?}"
            );

            // Warm the resolver's schema knowledge while the database is
            // healthy, with a second issue of the same type: the extends chain
            // and what `story` means from this type are both read from the
            // `relationship` table, so without this they would fail first and
            // the walk below would never run.
            let other_issue = make_node(
                "17d5e68b-85b7-5f36-81fd-b137954515be",
                "gr-issue-err",
                json!({"status": "open"}),
            );
            svc.create_node(other_issue.clone()).await.unwrap();
            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            assert!(matches!(
                resolver
                    .resolve_path(&other_issue, &["story".to_string()])
                    .await,
                ResolvedValue::Missing
            ));

            svc.store()
                .write()
                .await
                .execute("DROP TABLE relationship", ())
                .await
                .expect("dropping the relationship table should succeed");

            let result = resolver.resolve_path(&issue, &["story".to_string()]).await;
            match &result {
                ResolvedValue::Unresolved(reason) => assert!(
                    reason.contains("failed to walk story"),
                    "expected the walk to be what failed: {reason}"
                ),
                other => panic!("a failed walk must be Unresolved, got {other:?}"),
            }
            // A failure is never remembered: it says nothing about the path.
            assert!(matches!(
                resolver.resolve_path(&issue, &["story".to_string()]).await,
                ResolvedValue::Unresolved(_)
            ));

            let broken = evaluate_conditions_at_scope(
                &conditions,
                &issue,
                &event,
                Some(&mut resolver),
                None,
            )
            .await;
            assert!(
                matches!(broken, ConditionResult::Unresolved { .. }),
                "a failed fetch must not let !has(node.story) fire, got {broken:?}"
            );
        }

        /// Regression test: resolve_path/resolve_collection/enrich_context must
        /// work on a single-threaded runtime. The old `block_in_place` bridge
        /// would panic here; this proves the fix, not just its absence.
        #[tokio::test(flavor = "current_thread")]
        async fn resolve_path_and_enrich_context_work_under_current_thread_runtime() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-story-ct", json!([])).await;
            create_schema(
                &svc,
                "gr-issue-ct",
                json!([{
                    "name": "story",
                    "targetType": "gr-story-ct",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let story = make_node(
                "c8848aed-f332-53d6-a408-a277e359f8ce",
                "gr-story-ct",
                json!({"status": "active"}),
            );
            svc.create_node(story.clone()).await.unwrap();

            let issue = make_node(
                "a023b9dc-00c7-510b-884e-da40974a72fc",
                "gr-issue-ct",
                json!({"status": "open"}),
            );
            svc.create_node(issue.clone()).await.unwrap();

            svc.create_relationship(
                "a023b9dc-00c7-510b-884e-da40974a72fc",
                "story",
                "c8848aed-f332-53d6-a408-a277e359f8ce",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&issue, &["story".to_string()]).await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "c8848aed-f332-53d6-a408-a277e359f8ce"),
                other => panic!("expected Node, got {:?}", other),
            }

            use crate::playbook::path_extractor::ExtractedPath;
            let paths = vec![ExtractedPath {
                segments: vec![
                    "node".to_string(),
                    "story".to_string(),
                    "status".to_string(),
                ],
                root: "node".to_string(),
            }];
            let enriched = resolver.enrich_context(&issue, &paths, &[]).await.unwrap();
            let key = vec![
                "node".to_string(),
                "story".to_string(),
                "status".to_string(),
            ];
            assert!(
                enriched.contains_key(&key),
                "enrich_context should resolve node.story.status on a current_thread runtime"
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn resolve_multi_hop_relationship_chain() {
            let (svc, _tmp) = create_test_service().await;

            // Chain: gr-task3 -> story -> epic
            create_schema(&svc, "gr-epic", json!([])).await;
            create_schema(
                &svc,
                "gr-story3",
                json!([{
                    "name": "epic",
                    "targetType": "gr-epic",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "stories",
                    "reverseCardinality": "many"
                }]),
            )
            .await;
            create_schema(
                &svc,
                "gr-task3",
                json!([{
                    "name": "story",
                    "targetType": "gr-story3",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let epic = make_node(
                "2504876b-ac56-57ef-8353-22706def408f",
                "gr-epic",
                json!({"status": "in_progress"}),
            );
            svc.create_node(epic).await.unwrap();

            let story = make_node(
                "27c083ca-7466-59cd-b737-a9c076bbf28c",
                "gr-story3",
                json!({"status": "active"}),
            );
            svc.create_node(story).await.unwrap();

            let task = make_node(
                "1139a1a0-6f33-5282-97c4-3bf0c8db6f10",
                "gr-task3",
                json!({"status": "open"}),
            );
            svc.create_node(task.clone()).await.unwrap();

            svc.create_relationship(
                "1139a1a0-6f33-5282-97c4-3bf0c8db6f10",
                "story",
                "27c083ca-7466-59cd-b737-a9c076bbf28c",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "27c083ca-7466-59cd-b737-a9c076bbf28c",
                "epic",
                "2504876b-ac56-57ef-8353-22706def408f",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));

            // Resolve task -> story -> epic
            let result = resolver
                .resolve_path(&task, &["story".to_string(), "epic".to_string()])
                .await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "2504876b-ac56-57ef-8353-22706def408f"),
                other => panic!("expected Node for story.epic, got {:?}", other),
            }

            // Resolve task -> story -> epic -> status (scalar property on the target)
            let result = resolver
                .resolve_path(
                    &task,
                    &[
                        "story".to_string(),
                        "epic".to_string(),
                        "status".to_string(),
                    ],
                )
                .await;
            match result {
                ResolvedValue::Scalar(v) => assert_eq!(v, json!("in_progress")),
                other => panic!("expected Scalar for story.epic.status, got {:?}", other),
            }
        }

        /// A declared "many" relationship with ZERO current related nodes
        /// must resolve to an empty Collection, not Missing -- a freshly
        /// created parent with no children yet (a Cycle with no Issues, a
        /// Project with no Tasks) is an ordinary state, not a
        /// missing/misconfigured path. Before this test's fix, this
        /// resolved to Missing, which made `for_each` (and this engine's
        /// `sum`/`count` aggregate action-binding calls, which resolve their
        /// collection through this exact same function) hard-fail the
        /// action the very first time the parent had zero related items --
        /// often the very first time the rule ever ran.
        #[tokio::test(flavor = "multi_thread")]
        async fn many_relationship_with_zero_matches_resolves_to_empty_collection() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-item-empty", json!([])).await;
            create_schema(
                &svc,
                "gr-parent-empty",
                json!([{
                    "name": "items",
                    "targetType": "gr-item-empty",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "parent",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            // Parent created with NO items ever attached -- the exact
            // "freshly created Cycle with no Issues yet" shape.
            let parent = make_node(
                "15bfaa70-906c-54ed-ae38-5b02022afb56",
                "gr-parent-empty",
                json!({}),
            );
            svc.create_node(parent.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&parent, &["items".to_string()]).await;
            match result {
                ResolvedValue::Collection(nodes) => assert!(
                    nodes.is_empty(),
                    "expected an empty Collection, got {} nodes",
                    nodes.len()
                ),
                other => panic!(
                    "expected an empty Collection (not Missing) for a declared many-relationship \
                     with zero current matches, got {:?}",
                    other
                ),
            }
        }

        /// Contrast case: a declared "one" relationship with zero current
        /// matches keeps the EXISTING Missing semantics -- unaffected by the
        /// fix above. "Missing" correctly means "this optional single
        /// relationship isn't set yet" (e.g. an Issue with no Cycle
        /// assigned), which `for_each` never resolves anyway (it requires an
        /// array) and conditions already treat as "not met" rather than an
        /// empty node to act on.
        #[tokio::test(flavor = "multi_thread")]
        async fn one_relationship_with_zero_matches_still_resolves_to_missing() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-cycle-unset", json!([])).await;
            create_schema(
                &svc,
                "gr-issue-unset",
                json!([{
                    "name": "cycle",
                    "targetType": "gr-cycle-unset",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            // Issue created with NO cycle relationship ever added.
            let issue = make_node(
                "d8d25f33-0604-59de-955c-e0fdc76bc325",
                "gr-issue-unset",
                json!({}),
            );
            svc.create_node(issue.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&issue, &["cycle".to_string()]).await;
            assert!(
                matches!(result, ResolvedValue::Missing),
                "a 'one' relationship with zero matches must still resolve to Missing \
                 (unset optional relationship), got {:?}",
                result
            );
        }

        /// Regression guard for the fix above: a segment that is NEITHER a
        /// property NOR any declared relationship (a genuine typo/nonexistent
        /// path) must still resolve to Missing -- the declared-cardinality check
        /// must not produce a false positive just because the fetch happened
        /// to return zero rows.
        #[tokio::test(flavor = "multi_thread")]
        async fn undeclared_segment_still_resolves_to_missing() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-lonely", json!([])).await;
            let node = make_node(
                "9f68169c-2c13-59df-87d6-ecd6e7f418f0",
                "gr-lonely",
                json!({}),
            );
            svc.create_node(node.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&node, &["nonexistent_relationship".to_string()])
                .await;
            assert!(
                matches!(result, ResolvedValue::Missing),
                "an undeclared segment must resolve to Missing, not an empty Collection, \
                 got {:?}",
                result
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn resolve_path_cache_hit() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-story4", json!([])).await;
            create_schema(
                &svc,
                "gr-task4",
                json!([{
                    "name": "story",
                    "targetType": "gr-story4",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let story = make_node(
                "107dc6a4-d317-5b21-9f39-6d1d8943c681",
                "gr-story4",
                json!({"status": "done"}),
            );
            svc.create_node(story).await.unwrap();
            let task = make_node(
                "cc70c452-a76f-5ced-900f-87737c548736",
                "gr-task4",
                json!({}),
            );
            svc.create_node(task.clone()).await.unwrap();
            svc.create_relationship(
                "cc70c452-a76f-5ced-900f-87737c548736",
                "story",
                "107dc6a4-d317-5b21-9f39-6d1d8943c681",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));

            // First call populates cache
            let r1 = resolver
                .resolve_path(&task, &["story".to_string(), "status".to_string()])
                .await;
            assert!(matches!(r1, ResolvedValue::Scalar(_)));

            // Second call should hit cache (same result)
            let r2 = resolver
                .resolve_path(&task, &["story".to_string(), "status".to_string()])
                .await;
            assert!(matches!(r2, ResolvedValue::Scalar(_)));
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn resolve_collection_returns_multiple_nodes() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-subtask", json!([])).await;
            create_schema(
                &svc,
                "gr-parent",
                json!([{
                    "name": "subtasks",
                    "targetType": "gr-subtask",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "parent_task",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let sub1 = make_node(
                "76d0533a-fca3-53fc-ae07-c8a6fff28407",
                "gr-subtask",
                json!({"status": "done"}),
            );
            let sub2 = make_node(
                "197f434b-77de-5206-965b-5464fa621440",
                "gr-subtask",
                json!({"status": "open"}),
            );
            svc.create_node(sub1).await.unwrap();
            svc.create_node(sub2).await.unwrap();

            let parent = make_node(
                "fc562f69-fc5e-50c3-b4da-d89de50e6f79",
                "gr-parent",
                json!({}),
            );
            svc.create_node(parent.clone()).await.unwrap();

            svc.create_relationship(
                "fc562f69-fc5e-50c3-b4da-d89de50e6f79",
                "subtasks",
                "76d0533a-fca3-53fc-ae07-c8a6fff28407",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "fc562f69-fc5e-50c3-b4da-d89de50e6f79",
                "subtasks",
                "197f434b-77de-5206-965b-5464fa621440",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&parent, &["subtasks".to_string()])
                .await;
            match result {
                ResolvedValue::Collection(nodes) => {
                    assert_eq!(nodes.len(), 2);
                    let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
                    assert!(ids.contains(&"76d0533a-fca3-53fc-ae07-c8a6fff28407"));
                    assert!(ids.contains(&"197f434b-77de-5206-965b-5464fa621440"));
                }
                other => panic!("expected Collection, got {:?}", other),
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn enrich_context_builds_cel_values() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-target5", json!([])).await;
            create_schema(
                &svc,
                "gr-source5",
                json!([{
                    "name": "target",
                    "targetType": "gr-target5",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "sources",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let target = make_node(
                "ac296fa4-9a76-56d8-b333-866646d27c09",
                "gr-target5",
                json!({"status": "ready"}),
            );
            svc.create_node(target).await.unwrap();
            let source = make_node(
                "9147b150-a60c-550a-b49f-2fe7d9d20974",
                "gr-source5",
                json!({}),
            );
            svc.create_node(source.clone()).await.unwrap();
            svc.create_relationship(
                "9147b150-a60c-550a-b49f-2fe7d9d20974",
                "target",
                "ac296fa4-9a76-56d8-b333-866646d27c09",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));

            let paths = vec![ExtractedPath {
                segments: vec![
                    "node".to_string(),
                    "target".to_string(),
                    "status".to_string(),
                ],
                root: "node".to_string(),
            }];

            let result = resolver.enrich_context(&source, &paths, &[]).await.unwrap();
            // Should have resolved node.target.status
            let key = vec![
                "node".to_string(),
                "target".to_string(),
                "status".to_string(),
            ];
            assert!(result.contains_key(&key), "should contain resolved path");
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn resolve_empty_segments_returns_root_node() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-task6", json!([])).await;

            let node = make_node(
                "49638dad-3ddf-51c0-9ee9-d01cf0d23db2",
                "gr-task6",
                json!({}),
            );
            svc.create_node(node.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&node, &[]).await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "49638dad-3ddf-51c0-9ee9-d01cf0d23db2"),
                other => panic!("expected Node, got {:?}", other),
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn cannot_walk_past_scalar() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-task7", json!([])).await;

            let node = make_node(
                "6831fac3-a02b-5d53-a66b-814c006e379f",
                "gr-task7",
                json!({"status": "open"}),
            );
            svc.create_node(node.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            // "status" is a scalar property, can't walk further
            let result = resolver
                .resolve_path(&node, &["status".to_string(), "deeper".to_string()])
                .await;
            assert!(matches!(result, ResolvedValue::Missing));
        }

        // -------------------------------------------------------------------
        // enrich_context and resolve_collection — direct integration tests
        // -------------------------------------------------------------------

        #[tokio::test(flavor = "multi_thread")]
        async fn enrich_context_resolves_multi_hop_path() {
            let (svc, _tmp) = create_test_service().await;

            // Chain: gr-task8 -> story8 -> epic8
            create_schema(&svc, "gr-epic8", json!([])).await;
            create_schema(
                &svc,
                "gr-story8",
                json!([{
                    "name": "epic",
                    "targetType": "gr-epic8",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "stories",
                    "reverseCardinality": "many"
                }]),
            )
            .await;
            create_schema(
                &svc,
                "gr-task8",
                json!([{
                    "name": "story",
                    "targetType": "gr-story8",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let epic = make_node(
                "169f3f29-1189-5671-b858-e6f349db079b",
                "gr-epic8",
                json!({"status": "in_progress"}),
            );
            svc.create_node(epic).await.unwrap();
            let story = make_node(
                "7e8706b4-1465-5989-b7fd-e25d6288e32b",
                "gr-story8",
                json!({"status": "active"}),
            );
            svc.create_node(story).await.unwrap();
            let task = make_node(
                "d80b5203-ae2f-5735-906d-7043d9e52394",
                "gr-task8",
                json!({"status": "open"}),
            );
            svc.create_node(task.clone()).await.unwrap();

            svc.create_relationship(
                "d80b5203-ae2f-5735-906d-7043d9e52394",
                "story",
                "7e8706b4-1465-5989-b7fd-e25d6288e32b",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "7e8706b4-1465-5989-b7fd-e25d6288e32b",
                "epic",
                "169f3f29-1189-5671-b858-e6f349db079b",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));

            use crate::playbook::path_extractor::ExtractedPath;

            // Multi-hop path: node.story.epic.status (3+ segments, root="node")
            let paths = vec![ExtractedPath {
                segments: vec![
                    "node".to_string(),
                    "story".to_string(),
                    "epic".to_string(),
                    "status".to_string(),
                ],
                root: "node".to_string(),
            }];

            let result = resolver.enrich_context(&task, &paths, &[]).await.unwrap();

            let key = vec![
                "node".to_string(),
                "story".to_string(),
                "epic".to_string(),
                "status".to_string(),
            ];
            assert!(
                result.contains_key(&key),
                "enrich_context should resolve multi-hop path node.story.epic.status"
            );
            // The value should be a CEL String("in_progress")
            match result.get(&key) {
                Some(Value::String(s)) => assert_eq!(s.as_ref(), "in_progress"),
                other => panic!("expected CEL String('in_progress'), got {:?}", other),
            }
        }

        /// A cleared field (stored `null`) on a node reached through a
        /// relationship reads exactly as one that was never set.
        #[tokio::test(flavor = "multi_thread")]
        async fn cleared_field_through_a_dot_path_reads_as_absent() {
            use crate::playbook::cel::{evaluate_conditions, CompiledCondition, ConditionResult};

            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-story-null", json!([])).await;
            create_schema(
                &svc,
                "gr-task-null",
                json!([{
                    "name": "story",
                    "targetType": "gr-story-null",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let pass = ConditionResult::Pass;
            let fail = ConditionResult::Fail { condition_index: 0 };
            let cases = [
                (
                    "cleared",
                    "0b7c1a52-6a3e-4f0b-9d2b-5c1f6e1a0001",
                    "0b7c1a52-6a3e-4f0b-9d2b-5c1f6e1a0002",
                    json!({"status": "active", "priority": null}),
                    false,
                ),
                (
                    "never set",
                    "0b7c1a52-6a3e-4f0b-9d2b-5c1f6e1a0003",
                    "0b7c1a52-6a3e-4f0b-9d2b-5c1f6e1a0004",
                    json!({"status": "active"}),
                    false,
                ),
                (
                    "set",
                    "0b7c1a52-6a3e-4f0b-9d2b-5c1f6e1a0005",
                    "0b7c1a52-6a3e-4f0b-9d2b-5c1f6e1a0006",
                    json!({"status": "active", "priority": "high"}),
                    true,
                ),
            ];

            for (which, story_id, task_id, story_props, has_value) in cases {
                svc.create_node(make_node(story_id, "gr-story-null", story_props.clone()))
                    .await
                    .unwrap();
                // The task carries the same field, for the collection read
                // from the story's side below.
                let task = make_node(task_id, "gr-task-null", story_props);
                svc.create_node(task.clone()).await.unwrap();
                svc.create_relationship(task_id, "story", story_id, json!({}))
                    .await
                    .unwrap();

                // The fixture must really store what the case is named for.
                let stored = svc.get_node(story_id).await.unwrap().unwrap();
                let stored_priority = stored.properties["gr-story-null"].get("priority");
                match which {
                    "cleared" => assert_eq!(stored_priority, Some(&json!(null))),
                    "never set" => assert_eq!(stored_priority, None),
                    _ => assert_eq!(stored_priority, Some(&json!("high"))),
                }

                let event = crate::db::events::DomainEvent::NodeCreated {
                    node_type: "gr-task-null".to_string(),
                    node_id: task_id.to_string(),
                };
                for (expr, without_value, with_value) in [
                    ("has(node.story.priority)", &fail, &pass),
                    ("has(node.story) && !has(node.story.priority)", &pass, &fail),
                    // Without the `has(node.story)` guard nothing puts the
                    // story itself in the context, so a field of it with no
                    // value is a missing path and fails the condition.
                    ("!has(node.story.priority)", &fail, &fail),
                    ("node.story.priority == null", &fail, &fail),
                    ("node.story.priority != null", &fail, &pass),
                ] {
                    let conditions = vec![CompiledCondition::compile(expr).unwrap()];
                    let mut resolver = GraphResolver::new(Arc::clone(&svc));
                    let result =
                        evaluate_conditions(&conditions, &task, &event, Some(&mut resolver)).await;
                    assert_eq!(
                        &result,
                        if has_value { with_value } else { without_value },
                        "`{expr}` on a task whose story's priority is {which}"
                    );
                }

                // A collection item is read the same way as a single node.
                for (expr, without_value, with_value) in [
                    ("node.issues.exists(i, has(i.priority))", &fail, &pass),
                    ("node.issues.exists(i, !has(i.priority))", &pass, &fail),
                ] {
                    let conditions = vec![CompiledCondition::compile(expr).unwrap()];
                    let mut resolver = GraphResolver::new(Arc::clone(&svc));
                    let result =
                        evaluate_conditions(&conditions, &stored, &event, Some(&mut resolver))
                            .await;
                    assert_eq!(
                        &result,
                        if has_value { with_value } else { without_value },
                        "`{expr}` on a story whose task's priority is {which}"
                    );
                }

                // An action binding is a different read: a cleared field
                // binds to `null`, and only a never-set one is a missing path.
                let mut resolver = GraphResolver::new(Arc::clone(&svc));
                let resolved = resolver
                    .resolve_path(&task, &["story".to_string(), "priority".to_string()])
                    .await;
                match which {
                    "cleared" => {
                        assert!(matches!(&resolved, ResolvedValue::Scalar(v) if v.is_null()))
                    }
                    "never set" => assert!(matches!(resolved, ResolvedValue::Missing)),
                    _ => assert!(matches!(&resolved, ResolvedValue::Scalar(v) if v == "high")),
                }
            }
        }

        /// A link's parts are read off the node's own value: the resolver
        /// finds no relationship to walk and leaves the link where it is.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_link_fields_parts_read_through_the_resolver() {
            use crate::playbook::cel::{evaluate_conditions, CompiledCondition, ConditionResult};

            let (svc, _tmp) = create_test_service().await;
            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr-linked",
                    "fields": [{ "name": "repository", "type": "link" }]
                }),
            )
            .await
            .unwrap();

            let event = crate::db::events::DomainEvent::NodeCreated {
                node_type: "gr-linked".to_string(),
                node_id: "unused".to_string(),
            };
            let cases = [
                (
                    "5d0e3b1c-0c57-4f0e-8a41-2f1f3c7a0001",
                    json!({"repository": {"title": "Core", "url": "https://example.com/core"}}),
                    true,
                ),
                ("5d0e3b1c-0c57-4f0e-8a41-2f1f3c7a0002", json!({}), false),
            ];
            for (id, props, is_set) in cases {
                let node = make_node(id, "gr-linked", props);
                svc.create_node(node.clone()).await.unwrap();
                for (expr, when_set, when_unset) in [
                    (
                        "node.repository.url == 'https://example.com/core'",
                        true,
                        false,
                    ),
                    ("node.repository.title == 'Core'", true, false),
                    ("has(node.repository)", true, false),
                    ("!has(node.repository)", false, true),
                ] {
                    let conditions = vec![CompiledCondition::compile(expr).unwrap()];
                    let mut resolver = GraphResolver::new(Arc::clone(&svc));
                    let result =
                        evaluate_conditions(&conditions, &node, &event, Some(&mut resolver)).await;
                    let passes = if is_set { when_set } else { when_unset };
                    assert_eq!(
                        result == ConditionResult::Pass,
                        passes,
                        "`{expr}` with the link set: {is_set} gave {result:?}"
                    );
                }
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn enrich_context_multi_hop_path_excludes_internal_key() {
            // A multi-hop CEL path (node.related_node._internalKey, segment
            // length > 2) is resolved via get_node_property, not
            // node_to_cel_value -- this exercises that a `_`-prefixed
            // internal-bookkeeping value on a RELATED node stays unreadable,
            // matching the direct-node case already enforced by
            // node_to_cel_value.
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-related11", json!([])).await;
            create_schema(
                &svc,
                "gr-root11",
                json!([{
                    "name": "related_node",
                    "targetType": "gr-related11",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "roots",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let related = make_node(
                "fd1c1afe-18cb-558c-b0d3-9fc930d72c7b",
                "gr-related11",
                json!({"status": "active", "_playbookChainDepth": 7}),
            );
            svc.create_node(related).await.unwrap();
            let root = make_node(
                "cd5d5d14-9148-5bed-91cd-754185704f03",
                "gr-root11",
                json!({}),
            );
            svc.create_node(root.clone()).await.unwrap();
            svc.create_relationship(
                "cd5d5d14-9148-5bed-91cd-754185704f03",
                "related_node",
                "fd1c1afe-18cb-558c-b0d3-9fc930d72c7b",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));

            use crate::playbook::path_extractor::ExtractedPath;

            // Multi-hop path targeting the internal key on the related node:
            // node.related_node._playbookChainDepth (3 segments, root="node").
            let internal_path = ExtractedPath {
                segments: vec![
                    "node".to_string(),
                    "related_node".to_string(),
                    "_playbookChainDepth".to_string(),
                ],
                root: "node".to_string(),
            };
            // Control path: an ordinary property on the same related node
            // resolves normally, proving the traversal itself works and the
            // internal key's absence isn't an unrelated resolution failure.
            let normal_path = ExtractedPath {
                segments: vec![
                    "node".to_string(),
                    "related_node".to_string(),
                    "status".to_string(),
                ],
                root: "node".to_string(),
            };

            let result = resolver
                .enrich_context(&root, &[internal_path, normal_path], &[])
                .await
                .unwrap();

            let internal_key = vec![
                "node".to_string(),
                "related_node".to_string(),
                "_playbookChainDepth".to_string(),
            ];
            assert!(
                !result.contains_key(&internal_key),
                "internal-bookkeeping key on a related node must not be CEL-readable via a multi-hop path"
            );

            let normal_key = vec![
                "node".to_string(),
                "related_node".to_string(),
                "status".to_string(),
            ];
            assert!(
                result.contains_key(&normal_key),
                "ordinary property on the related node should still resolve"
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn resolve_collection_with_collection_path() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-item9", json!([])).await;
            create_schema(
                &svc,
                "gr-parent9",
                json!([{
                    "name": "items",
                    "targetType": "gr-item9",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "collection",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let item1 = make_node(
                "f1e99148-e345-55b0-a3bf-fc7289df6a41",
                "gr-item9",
                json!({"status": "done"}),
            );
            let item2 = make_node(
                "fa650a80-244b-5e73-980e-cc70dd49adf7",
                "gr-item9",
                json!({"status": "open"}),
            );
            svc.create_node(item1).await.unwrap();
            svc.create_node(item2).await.unwrap();

            let parent = make_node(
                "ceee2e5f-adf7-5091-b85a-67e5ae80ec1b",
                "gr-parent9",
                json!({}),
            );
            svc.create_node(parent.clone()).await.unwrap();

            svc.create_relationship(
                "ceee2e5f-adf7-5091-b85a-67e5ae80ec1b",
                "items",
                "f1e99148-e345-55b0-a3bf-fc7289df6a41",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "ceee2e5f-adf7-5091-b85a-67e5ae80ec1b",
                "items",
                "fa650a80-244b-5e73-980e-cc70dd49adf7",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));

            use crate::playbook::path_extractor::ExtractedPath;

            // resolve_collection expects segments like ["node", "items"]
            let collection_path = ExtractedPath {
                segments: vec!["node".to_string(), "items".to_string()],
                root: "node".to_string(),
            };

            let nodes = resolver
                .resolve_collection(&parent, &collection_path)
                .await
                .unwrap();
            assert_eq!(nodes.len(), 2, "should resolve 2 collection nodes");
            let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
            assert!(ids.contains(&"f1e99148-e345-55b0-a3bf-fc7289df6a41"));
            assert!(ids.contains(&"fa650a80-244b-5e73-980e-cc70dd49adf7"));
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn enrich_context_with_collection() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-sub10", json!([])).await;
            create_schema(
                &svc,
                "gr-parent10",
                json!([{
                    "name": "tasks",
                    "targetType": "gr-sub10",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "parent_task",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let sub1 = make_node(
                "d4ff61ea-e0fa-5cad-8db0-e0cdac5f32f2",
                "gr-sub10",
                json!({"status": "done"}),
            );
            let sub2 = make_node(
                "87f5b39d-b67c-5d48-add2-1d7a7bcfc023",
                "gr-sub10",
                json!({"status": "open"}),
            );
            svc.create_node(sub1).await.unwrap();
            svc.create_node(sub2).await.unwrap();

            let parent = make_node(
                "93150e2e-5d1d-5292-8bdc-77f86393d43f",
                "gr-parent10",
                json!({}),
            );
            svc.create_node(parent.clone()).await.unwrap();

            svc.create_relationship(
                "93150e2e-5d1d-5292-8bdc-77f86393d43f",
                "tasks",
                "d4ff61ea-e0fa-5cad-8db0-e0cdac5f32f2",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "93150e2e-5d1d-5292-8bdc-77f86393d43f",
                "tasks",
                "87f5b39d-b67c-5d48-add2-1d7a7bcfc023",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));

            use crate::playbook::path_extractor::{CollectionPath, ExtractedPath};

            let collections = vec![CollectionPath {
                collection: ExtractedPath {
                    segments: vec!["node".to_string(), "tasks".to_string()],
                    root: "node".to_string(),
                },
                iter_var: "t".to_string(),
                item_paths: vec![ExtractedPath {
                    segments: vec!["t".to_string(), "status".to_string()],
                    root: "t".to_string(),
                }],
            }];

            let result = resolver
                .enrich_context(&parent, &[], &collections)
                .await
                .unwrap();

            let key = vec!["node".to_string(), "tasks".to_string()];
            assert!(
                result.contains_key(&key),
                "enrich_context should resolve collection path node.tasks"
            );

            // The value should be a CEL List with 2 elements
            match result.get(&key) {
                Some(Value::List(list)) => {
                    assert_eq!(list.len(), 2, "collection should have 2 nodes");
                }
                other => panic!("expected CEL List, got {:?}", other),
            }
        }

        // ================================================================
        // Reverse-direction traversal
        // ================================================================

        /// A built-in's reverse name walks to the node at the edge's other end.
        ///
        /// `has_child` is stored parent → child, so a child reaching its parent
        /// is the same row read backwards. This is the traversal a
        /// complete-the-parent Play needs, and the one the resolver could not
        /// express while direction was hardcoded to `"out"`.
        #[tokio::test(flavor = "multi_thread")]
        async fn builtin_reverse_name_walks_to_the_parent() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-rev-task", json!([])).await;

            let parent = make_node(
                "9c98927a-4041-56c4-b1d8-2c9a06d5723a",
                "gr-rev-task",
                json!({"status": "open"}),
            );
            svc.create_node(parent.clone()).await.unwrap();
            let child = make_node(
                "984353e4-bfac-5202-96f2-54010d96232c",
                "gr-rev-task",
                json!({"status": "done"}),
            );
            svc.create_node(child.clone()).await.unwrap();

            svc.create_relationship(
                "9c98927a-4041-56c4-b1d8-2c9a06d5723a",
                "has_child",
                "984353e4-bfac-5202-96f2-54010d96232c",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&child, &["child_of".to_string()])
                .await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "9c98927a-4041-56c4-b1d8-2c9a06d5723a"),
                other => panic!("expected the parent Node, got {:?}", other),
            }

            // The forward direction must still walk the other way, from the
            // same edge: reverse support is additive, not a redirect.
            let forward = resolver
                .resolve_path(&parent, &["has_child".to_string()])
                .await;
            match forward {
                ResolvedValue::Node(n) => assert_eq!(n.id, "984353e4-bfac-5202-96f2-54010d96232c"),
                other => panic!("expected the child Node, got {:?}", other),
            }
        }

        /// A related node's core fields resolve, not just its properties.
        ///
        /// `id`/`node_type`/`content` are struct fields, in no type bucket, so
        /// the property lookup cannot see them. Reaching a node and reading its
        /// identity is how an action addresses it — `{trigger.node.child_of.id}`
        /// is exactly what the parent-completion Play's `update_node` needs
        /// (ADR-079) — and without this the path resolved to `Missing` and
        /// failed the action, which disables the whole Play, even though
        /// `node.child_of` alone resolved fine.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_related_nodes_core_fields_resolve() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-core-task", json!([])).await;

            let parent = make_node(
                "61fc0710-eddf-5aa3-872b-12132075a3ab",
                "gr-core-task",
                json!({"status": "open"}),
            );
            svc.create_node(parent.clone()).await.unwrap();
            let child = make_node(
                "0ad70408-ba33-5384-aa53-9e6db860be23",
                "gr-core-task",
                json!({"status": "done"}),
            );
            svc.create_node(child.clone()).await.unwrap();
            svc.create_relationship(
                "61fc0710-eddf-5aa3-872b-12132075a3ab",
                "has_child",
                "0ad70408-ba33-5384-aa53-9e6db860be23",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            for (segment, want) in [
                ("id", "61fc0710-eddf-5aa3-872b-12132075a3ab"),
                ("node_type", "gr-core-task"),
            ] {
                let result = resolver
                    .resolve_path(&child, &["child_of".to_string(), segment.to_string()])
                    .await;
                match result {
                    ResolvedValue::Scalar(v) => assert_eq!(
                        v.as_str(),
                        Some(want),
                        "child_of.{segment} must resolve to the parent's {segment}"
                    ),
                    other => panic!("expected a Scalar for child_of.{segment}, got {other:?}"),
                }
            }

            // `type_chain` comes from the schemas, not from a struct field,
            // and resolves on a related node like the others.
            match resolver
                .resolve_path(&child, &["child_of".to_string(), "type_chain".to_string()])
                .await
            {
                ResolvedValue::Scalar(v) => assert_eq!(v, json!(["gr-core-task"])),
                other => panic!("expected a Scalar for child_of.type_chain, got {other:?}"),
            }

            // A core field on the root node itself resolves the same way, with
            // no traversal involved.
            match resolver.resolve_path(&child, &["id".to_string()]).await {
                ResolvedValue::Scalar(v) => {
                    assert_eq!(v.as_str(), Some("0ad70408-ba33-5384-aa53-9e6db860be23"))
                }
                other => panic!("expected the node's own id, got {other:?}"),
            }
        }

        /// A schema-declared `reverseName` resolves the same way, and chains.
        ///
        /// `gr-rev-person` declares `tasks`; a task reaching its owner spells
        /// that `assignee`. The second hop (`.email`) proves a reverse segment
        /// leaves the walk in the same state a forward one does — the resolved
        /// node keeps being walkable, so multi-hop paths work through it.
        #[tokio::test(flavor = "multi_thread")]
        async fn declared_reverse_name_resolves_and_chains() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-rev-ticket", json!([])).await;
            create_schema(
                &svc,
                "gr-rev-person",
                json!([{
                    "name": "tasks",
                    "targetType": "gr-rev-ticket",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "assignee",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let person = make_node(
                "22396a0f-ef2c-5998-a93c-b626687a36da",
                "gr-rev-person",
                json!({"email": "ada@example.com"}),
            );
            svc.create_node(person.clone()).await.unwrap();
            let ticket = make_node(
                "c00e5c50-aba9-5350-8a3f-ad2d4adca7a4",
                "gr-rev-ticket",
                json!({"status": "open"}),
            );
            svc.create_node(ticket.clone()).await.unwrap();

            svc.create_relationship(
                "22396a0f-ef2c-5998-a93c-b626687a36da",
                "tasks",
                "c00e5c50-aba9-5350-8a3f-ad2d4adca7a4",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&ticket, &["assignee".to_string()])
                .await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "22396a0f-ef2c-5998-a93c-b626687a36da"),
                other => panic!("expected the assignee Node, got {:?}", other),
            }

            // Multi-hop through the reverse segment: node.assignee.email
            let chained = resolver
                .resolve_path(&ticket, &["assignee".to_string(), "email".to_string()])
                .await;
            match chained {
                ResolvedValue::Scalar(v) => assert_eq!(v, json!("ada@example.com")),
                other => panic!("expected a Scalar email, got {:?}", other),
            }
        }

        /// A reverse name returns only the schema that declared it.
        ///
        /// The store keys an inbound query on `relationship_type` alone, so two
        /// schemas declaring the same forward name toward one type both answer
        /// it. Without narrowing, a ticket asking for its `assignee` also gets
        /// the project back — a wrong node, not merely an extra one, since the
        /// resolver reports a single match as `Node` and two as `Collection`.
        ///
        /// This is the shape real data already has: `tasks` is declared on both
        /// `project` and `person` in the core schemas.
        #[tokio::test(flavor = "multi_thread")]
        async fn reverse_name_excludes_another_schemas_same_forward_name() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-nar-ticket", json!([])).await;
            create_schema(
                &svc,
                "gr-nar-person",
                json!([{
                    "name": "tasks",
                    "targetType": "gr-nar-ticket",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "assignee",
                    "reverseCardinality": "one"
                }]),
            )
            .await;
            // A second schema declaring the SAME forward name at the same type.
            create_schema(
                &svc,
                "gr-nar-project",
                json!([{
                    "name": "tasks",
                    "targetType": "gr-nar-ticket",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "project",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let person = make_node(
                "de34e23d-e992-584a-b512-96a69307fb69",
                "gr-nar-person",
                json!({"email": "grace@x.io"}),
            );
            svc.create_node(person.clone()).await.unwrap();
            let project = make_node(
                "2052f379-c47b-5a08-8aa7-2be327e25aae",
                "gr-nar-project",
                json!({"name": "Apollo"}),
            );
            svc.create_node(project.clone()).await.unwrap();
            let ticket = make_node(
                "050d7c67-e214-512c-90f4-0667accac799",
                "gr-nar-ticket",
                json!({"status": "open"}),
            );
            svc.create_node(ticket.clone()).await.unwrap();

            // The same ticket is linked from both ends, under the same name.
            svc.create_relationship(
                "de34e23d-e992-584a-b512-96a69307fb69",
                "tasks",
                "050d7c67-e214-512c-90f4-0667accac799",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "2052f379-c47b-5a08-8aa7-2be327e25aae",
                "tasks",
                "050d7c67-e214-512c-90f4-0667accac799",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            match resolver
                .resolve_path(&ticket, &["assignee".to_string()])
                .await
            {
                ResolvedValue::Node(n) => assert_eq!(
                    n.id, "de34e23d-e992-584a-b512-96a69307fb69",
                    "assignee must be the person, not the project"
                ),
                other => panic!("expected exactly the person Node, got {:?}", other),
            }

            // And the other declarer's reverse name resolves to its own node.
            match resolver
                .resolve_path(&ticket, &["project".to_string()])
                .await
            {
                ResolvedValue::Node(n) => assert_eq!(n.id, "2052f379-c47b-5a08-8aa7-2be327e25aae"),
                other => panic!("expected exactly the project Node, got {:?}", other),
            }
        }

        /// A reverse segment feeds collection comprehensions like any other.
        ///
        /// `node.child_of.has_child` — from a child, up to the parent, then back
        /// down to all its children — is the path an all-siblings-done condition
        /// walks.
        #[tokio::test(flavor = "multi_thread")]
        async fn reverse_segment_feeds_a_collection() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-sib-task", json!([])).await;

            let parent = make_node(
                "d2c02095-f68f-5e79-851a-2d28484980cf",
                "gr-sib-task",
                json!({"status": "open"}),
            );
            svc.create_node(parent.clone()).await.unwrap();
            for id in [
                "2e51a202-4d99-50c8-9d6c-e7dfda6b1c32",
                "da1b5799-6677-53c9-b277-282337ff2d1b",
            ] {
                let child = make_node(id, "gr-sib-task", json!({"status": "done"}));
                svc.create_node(child.clone()).await.unwrap();
                svc.create_relationship(
                    "d2c02095-f68f-5e79-851a-2d28484980cf",
                    "has_child",
                    id,
                    json!({}),
                )
                .await
                .unwrap();
            }
            let child = svc
                .get_node("2e51a202-4d99-50c8-9d6c-e7dfda6b1c32")
                .await
                .unwrap()
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&child, &["child_of".to_string(), "has_child".to_string()])
                .await;
            match result {
                ResolvedValue::Collection(nodes) => {
                    assert_eq!(nodes.len(), 2, "the parent has two children");
                }
                other => panic!("expected a Collection of siblings, got {:?}", other),
            }
        }

        /// Another schema's FORWARD name, read from the target's end, walks
        /// inbound rather than resolving to nothing.
        ///
        /// This is the one case whose direction changed rather than merely
        /// becoming reachable. It cannot alter an existing Play: the name is
        /// not declared on this node's own schema (that resolves as `Forward`
        /// and still walks outbound), so the old hardcoded `"out"` query asked
        /// for edges that by construction never left this node — always empty,
        /// always a false condition. Serving the real inbound nodes is new
        /// capability, not a redirect of a working traversal.
        #[tokio::test(flavor = "multi_thread")]
        async fn another_schemas_forward_name_walks_inbound() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-inf-doc", json!([])).await;
            create_schema(
                &svc,
                "gr-inf-author",
                json!([{
                    "name": "wrote",
                    "targetType": "gr-inf-doc",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "written_by",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let author = make_node(
                "05305480-70d4-5232-865e-3e0017a9b2f9",
                "gr-inf-author",
                json!({"name": "Kay"}),
            );
            svc.create_node(author.clone()).await.unwrap();
            let doc = make_node(
                "5ffbd4e3-edac-54da-adc8-62e235f0e3f9",
                "gr-inf-doc",
                json!({"status": "draft"}),
            );
            svc.create_node(doc.clone()).await.unwrap();
            svc.create_relationship(
                "05305480-70d4-5232-865e-3e0017a9b2f9",
                "wrote",
                "5ffbd4e3-edac-54da-adc8-62e235f0e3f9",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            // The doc spells the edge by the author's forward name.
            match resolver.resolve_path(&doc, &["wrote".to_string()]).await {
                ResolvedValue::Node(n) => assert_eq!(n.id, "05305480-70d4-5232-865e-3e0017a9b2f9"),
                other => panic!("expected the author Node, got {:?}", other),
            }

            // The declaring end still walks outbound by that same name. It
            // resolves to a Collection rather than a Node even though exactly
            // one doc matches: `wrote` is declared `cardinality: "many"`, and a
            // declared "many" keeps its shape regardless of the current row
            // count. The reverse side above is a Node because its
            // `reverseCardinality` is "one" — the two ends are asked about
            // independently, which is the whole point of declaring both.
            match resolver.resolve_path(&author, &["wrote".to_string()]).await {
                ResolvedValue::Collection(nodes) => {
                    assert_eq!(nodes.len(), 1);
                    assert_eq!(nodes[0].id, "5ffbd4e3-edac-54da-adc8-62e235f0e3f9");
                }
                other => panic!("expected a Collection holding the doc, got {:?}", other),
            }
        }

        /// A two-segment reverse path (`node.assignee`) resolves through
        /// `enrich_context`, the boundary a Play condition actually calls.
        ///
        /// `resolve_path` is the unit; `enrich_context` is what CEL evaluation
        /// goes through. A gate there used to skip any path of two segments on
        /// the assumption that it must be a property — true while relationships
        /// only ever appeared mid-path, since a relationship needed a further
        /// hop to yield a value. A terminal reverse segment breaks that: the
        /// relationship IS the value. Without this, `node.assignee` silently
        /// evaluated to a false condition.
        #[tokio::test(flavor = "multi_thread")]
        async fn enrich_context_resolves_a_terminal_reverse_segment() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-term-ticket", json!([])).await;
            create_schema(
                &svc,
                "gr-term-person",
                json!([{
                    "name": "tasks",
                    "targetType": "gr-term-ticket",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "assignee",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            svc.create_node(make_node(
                "dcbb6d10-5e53-544f-a363-e1cca6fba6dd",
                "gr-term-person",
                json!({"email": "ada@example.com"}),
            ))
            .await
            .unwrap();
            let ticket = make_node(
                "9e86de83-ef32-573a-a09d-4326792b60a6",
                "gr-term-ticket",
                json!({"status": "open"}),
            );
            svc.create_node(ticket.clone()).await.unwrap();
            svc.create_relationship(
                "dcbb6d10-5e53-544f-a363-e1cca6fba6dd",
                "tasks",
                "9e86de83-ef32-573a-a09d-4326792b60a6",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let paths = vec![ExtractedPath {
                segments: vec!["node".to_string(), "assignee".to_string()],
                root: "node".to_string(),
            }];

            let result = resolver.enrich_context(&ticket, &paths, &[]).await.unwrap();

            let key = vec!["node".to_string(), "assignee".to_string()];
            assert!(
                result.contains_key(&key),
                "node.assignee must resolve through enrich_context, not just resolve_path"
            );
        }

        /// The segment cache is scoped to the node a walk started from.
        ///
        /// Two nodes asking the same path must get their own answers. Keyed on
        /// segments alone, the second walk would be served the first's result —
        /// a wrong node returned confidently, with no error anywhere. Callers
        /// build one resolver per work item today, so this guards the
        /// invariant rather than a current caller; reverse traversal makes
        /// reuse across roots (`child_of`, then `has_child` from the parent)
        /// the natural thing to reach for.
        #[tokio::test(flavor = "multi_thread")]
        async fn cache_does_not_leak_between_root_nodes() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-cache-task", json!([])).await;

            // Two independent parent/child pairs.
            for (parent, child) in [
                (
                    "00e9b22a-bdc6-5aef-a44b-002688132ed2",
                    "4c2c749d-796e-5bb2-95c7-056b7127c2d6",
                ),
                (
                    "63812817-f6d8-5e14-bcc4-c20c57761ffd",
                    "a9e21afa-8f60-5975-bfa5-36a4bf85a6c9",
                ),
            ] {
                svc.create_node(make_node(
                    parent,
                    "gr-cache-task",
                    json!({"status": "open"}),
                ))
                .await
                .unwrap();
                svc.create_node(make_node(child, "gr-cache-task", json!({"status": "done"})))
                    .await
                    .unwrap();
                svc.create_relationship(parent, "has_child", child, json!({}))
                    .await
                    .unwrap();
            }

            let child1 = svc
                .get_node("4c2c749d-796e-5bb2-95c7-056b7127c2d6")
                .await
                .unwrap()
                .unwrap();
            let child2 = svc
                .get_node("a9e21afa-8f60-5975-bfa5-36a4bf85a6c9")
                .await
                .unwrap()
                .unwrap();

            // One resolver, the same path, two different roots.
            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            match resolver
                .resolve_path(&child1, &["child_of".to_string()])
                .await
            {
                ResolvedValue::Node(n) => assert_eq!(n.id, "00e9b22a-bdc6-5aef-a44b-002688132ed2"),
                other => panic!("expected p1, got {:?}", other),
            }
            match resolver
                .resolve_path(&child2, &["child_of".to_string()])
                .await
            {
                ResolvedValue::Node(n) => assert_eq!(
                    n.id, "63812817-f6d8-5e14-bcc4-c20c57761ffd",
                    "the second root must not be served the first root's cached parent"
                ),
                other => panic!("expected p2, got {:?}", other),
            }
        }

        /// An unresolvable segment is `Missing`, not an error.
        ///
        /// Every segment is tried as a relationship once it fails as a property,
        /// so a plain typo reaches the relationship resolver. That resolver
        /// errors on an undeclared name (the CLI wants to say "no such
        /// relationship"), but here it must degrade to the empty traversal CEL
        /// reads as a false condition.
        #[tokio::test(flavor = "multi_thread")]
        async fn undeclared_segment_is_missing_rather_than_an_error() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr-unk-task", json!([])).await;

            let node = make_node(
                "6dc01f21-c30f-5698-9183-5871f63ce349",
                "gr-unk-task",
                json!({"status": "open"}),
            );
            svc.create_node(node.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&node, &["not_a_relationship".to_string()])
                .await;
            assert!(
                matches!(result, ResolvedValue::Missing),
                "expected Missing, got {:?}",
                result
            );
        }

        /// Regression: a "many" relationship declared only on an ancestor
        /// schema (ADR-078 `extends`), inherited but never redeclared by the
        /// subtype, must still be recognized as many-cardinality by
        /// the declared-cardinality check. Before the fix that check read
        /// `node_type`'s own directly-declared relationships only
        /// (`get_schema_node` + `Schema::get_relationship`), so a bare
        /// subtype schema had nothing to find and the lookup silently
        /// returned "not many" -- exactly the same extends-chain gap
        /// `resolve_relationships` was introduced to close for
        /// `get_workflow_state` and `validate_play`. With zero current
        /// matches this misclassification resolved the relationship to
        /// `Missing` instead of an empty `Collection`, which is the wrong
        /// shape for `for_each`/`sum`/`count` to iterate.
        #[tokio::test(flavor = "multi_thread")]
        async fn inherited_many_relationship_with_zero_matches_resolves_to_empty_collection() {
            let (svc, _tmp) = create_test_service().await;

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr-ext-item",
                    "fields": []
                }),
            )
            .await
            .expect("target schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr-ext-base",
                    "fields": [],
                    "relationships": [{
                        "name": "items",
                        "targetType": "gr-ext-item",
                        "direction": "out",
                        "cardinality": "many",
                        "reverseName": "parent",
                        "reverseCardinality": "one"
                    }]
                }),
            )
            .await
            .expect("base schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr-ext-sub",
                    "extends": "gr-ext-base",
                    "fields": []
                }),
            )
            .await
            .expect("subtype schema creation failed");

            // A subtype instance with NO items ever attached -- the relationship
            // is only declared on the ancestor, never redeclared here.
            let parent = make_node(
                "aed64f08-54fd-51a3-bf3e-1a114273d61e",
                "gr-ext-sub",
                json!({}),
            );
            svc.create_node(parent.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&parent, &["items".to_string()]).await;
            match result {
                ResolvedValue::Collection(nodes) => assert!(
                    nodes.is_empty(),
                    "expected an empty Collection, got {} nodes",
                    nodes.len()
                ),
                other => panic!(
                    "expected an empty Collection (not Missing) for an inherited many-relationship \
                     with zero current matches, got {:?}",
                    other
                ),
            }
        }

        /// Regression: the declared-cardinality check must also recognize a
        /// relationship whose "many" side is the REVERSE-declared end, not
        /// just the forward-declared end the sibling tests above cover.
        ///
        /// `task` declares the forward relationship `project` (cardinality
        /// "one") with `reverseName: "tasks"` and `reverseCardinality:
        /// "many"` -- "many tasks belong to one project." Walking
        /// `project.tasks` on a project with ZERO attached tasks queries by
        /// the reverse name, so before the fix the declared-cardinality check
        /// only ever checked `project`'s own forward-declared relationships
        /// (none), never found `tasks`, and fell through to count-based
        /// inference -- misclassifying it as not-many and resolving to
        /// `Missing` instead of an empty `Collection`.
        #[tokio::test(flavor = "multi_thread")]
        async fn reverse_declared_many_relationship_with_zero_matches_resolves_to_empty_collection()
        {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-revmany-project", json!([])).await;
            create_schema(
                &svc,
                "gr-revmany-task",
                json!([{
                    "name": "project",
                    "targetType": "gr-revmany-project",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "tasks",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            // A project with NO tasks ever attached -- the relationship is
            // declared on `task` (the forward side), not on `project`, so
            // `project`'s own schema has no relationships of its own at all.
            let project = make_node(
                "84dcef65-8d1e-5e2a-a148-730006f96faa",
                "gr-revmany-project",
                json!({}),
            );
            svc.create_node(project.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&project, &["tasks".to_string()])
                .await;
            match result {
                ResolvedValue::Collection(nodes) => assert!(
                    nodes.is_empty(),
                    "expected an empty Collection, got {} nodes",
                    nodes.len()
                ),
                other => panic!(
                    "expected an empty Collection (not Missing) for a reverse-declared \
                     many-relationship with zero current matches, got {:?}",
                    other
                ),
            }
        }

        /// Same gap, at exactly ONE current match: before the fix this
        /// resolved to a bare `Node` (the row-count fallback's single-match
        /// case) instead of a one-item `Collection`, which is the wrong
        /// shape for `for_each`/`sum`/`count` to iterate.
        #[tokio::test(flavor = "multi_thread")]
        async fn reverse_declared_many_relationship_with_one_match_resolves_to_collection() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-revmany1-project", json!([])).await;
            create_schema(
                &svc,
                "gr-revmany1-task",
                json!([{
                    "name": "project",
                    "targetType": "gr-revmany1-project",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "tasks",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let project = make_node(
                "4034b8e7-b8b9-589e-9633-695772d2b4df",
                "gr-revmany1-project",
                json!({}),
            );
            svc.create_node(project.clone()).await.unwrap();
            let task = make_node(
                "d145deac-8b96-5a23-83cf-2c3b94cbd877",
                "gr-revmany1-task",
                json!({}),
            );
            svc.create_node(task.clone()).await.unwrap();
            svc.create_relationship(
                "d145deac-8b96-5a23-83cf-2c3b94cbd877",
                "project",
                "4034b8e7-b8b9-589e-9633-695772d2b4df",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&project, &["tasks".to_string()])
                .await;
            match result {
                ResolvedValue::Collection(nodes) => {
                    assert_eq!(nodes.len(), 1);
                    assert_eq!(nodes[0].id, "d145deac-8b96-5a23-83cf-2c3b94cbd877");
                }
                other => panic!(
                    "expected a one-item Collection (not a bare Node) for a reverse-declared \
                     many-relationship with exactly one current match, got {:?}",
                    other
                ),
            }
        }

        /// Regression: the reverse-cardinality check must exclude the
        /// `extends`/`extended_by` type-system relationship, exactly as
        /// `resolve_relationships` already excludes it from the forward set.
        ///
        /// `extends` is stored as an ordinary `SchemaRelationship` row (on
        /// the subtype's schema, targeting its parent) with `reverseName:
        /// "extended_by"` and `reverseCardinality: "many"` -- so ANY base
        /// schema with at least one subtype has an inbound `extends`
        /// declaration reaching it. `get_inbound_relationships`, unlike
        /// `resolve_relationships`, does not filter type-system
        /// relationships out, so walking `extended_by` on an ordinary
        /// instance of the base type must still resolve to `Missing` (no
        /// data node ever carries such an edge) rather than being
        /// misidentified as a declared many-relationship and forced to an
        /// empty `Collection`.
        #[tokio::test(flavor = "multi_thread")]
        async fn extended_by_segment_is_not_treated_as_a_declared_many_relationship() {
            let (svc, _tmp) = create_test_service().await;

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr-extlk-base",
                    "fields": []
                }),
            )
            .await
            .expect("base schema creation failed");

            // A subtype exists solely so the base schema has an inbound
            // `extends` declaration (reverseName "extended_by") to leak.
            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr-extlk-sub",
                    "extends": "gr-extlk-base",
                    "fields": []
                }),
            )
            .await
            .expect("subtype schema creation failed");

            // An ordinary instance of the BASE type -- not a schema node,
            // and no `extends`/`extended_by` edge ever points at it.
            let base = make_node(
                "fcf3f955-9d07-57f3-96a9-101ddfe28607",
                "gr-extlk-base",
                json!({}),
            );
            svc.create_node(base.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&base, &["extended_by".to_string()])
                .await;
            assert!(
                matches!(result, ResolvedValue::Missing),
                "expected Missing for the type-system 'extended_by' segment, got {:?}",
                result
            );
        }

        /// Regression: the SAME gap as the reverse-name tests above, but for
        /// `ResolvedRelName::InboundForward` -- segment is the declaring
        /// schema's own FORWARD name, walked inbound from the target's side
        /// (not its `reverse_name`).
        ///
        /// `task` declares the forward relationship `owner_project`
        /// (cardinality "one") targeting `project`, with `reverseName:
        /// "owned_tasks"` and `reverseCardinality: "many"`. Walking
        /// `project_node.owner_project` -- the literal forward spelling,
        /// not the reverse name -- on a project with ZERO currently-owned
        /// tasks is exactly the traversal `another_schemas_forward_name_walks_inbound`
        /// above exercises, but with a "many" reverse cardinality instead of
        /// "one": before this fix, the declared-cardinality check only ever
        /// matched a segment against `reverse_name`, never against `name`, so
        /// this resolved to `Missing` instead of an empty `Collection` --
        /// InboundForward and Reverse are the same "walk inbound" direction
        /// and must be governed by the same `reverse_cardinality` field
        /// regardless of which name spelling the caller used.
        #[tokio::test(flavor = "multi_thread")]
        async fn inbound_forward_name_many_relationship_with_zero_matches_resolves_to_empty_collection(
        ) {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-infmany-project", json!([])).await;
            create_schema(
                &svc,
                "gr-infmany-task",
                json!([{
                    "name": "owner_project",
                    "targetType": "gr-infmany-project",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "owned_tasks",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            // A project with NO tasks ever attached.
            let project = make_node(
                "39d73c85-e744-5931-a830-fcbb7e15cbf6",
                "gr-infmany-project",
                json!({}),
            );
            svc.create_node(project.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            // The FORWARD spelling ("owner_project"), not the reverse name
            // ("owned_tasks") the sibling reverse-name tests use.
            let result = resolver
                .resolve_path(&project, &["owner_project".to_string()])
                .await;
            match result {
                ResolvedValue::Collection(nodes) => assert!(
                    nodes.is_empty(),
                    "expected an empty Collection, got {} nodes",
                    nodes.len()
                ),
                other => panic!(
                    "expected an empty Collection (not Missing) for an InboundForward-resolved \
                     many-relationship with zero current matches, got {:?}",
                    other
                ),
            }
        }

        /// Same gap, at exactly ONE current match -- before the fix this
        /// resolved to a bare `Node` instead of a one-item `Collection`.
        #[tokio::test(flavor = "multi_thread")]
        async fn inbound_forward_name_many_relationship_with_one_match_resolves_to_collection() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr-infmany1-project", json!([])).await;
            create_schema(
                &svc,
                "gr-infmany1-task",
                json!([{
                    "name": "owner_project",
                    "targetType": "gr-infmany1-project",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "owned_tasks",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let project = make_node(
                "e1b1f6b3-cc9c-58ef-8c58-9fbf29c13fe3",
                "gr-infmany1-project",
                json!({}),
            );
            svc.create_node(project.clone()).await.unwrap();
            let task = make_node(
                "29b9b1df-e10c-574d-ba78-09abe0b5549e",
                "gr-infmany1-task",
                json!({}),
            );
            svc.create_node(task.clone()).await.unwrap();
            svc.create_relationship(
                "29b9b1df-e10c-574d-ba78-09abe0b5549e",
                "owner_project",
                "e1b1f6b3-cc9c-58ef-8c58-9fbf29c13fe3",
                json!({}),
            )
            .await
            .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&project, &["owner_project".to_string()])
                .await;
            match result {
                ResolvedValue::Collection(nodes) => {
                    assert_eq!(nodes.len(), 1);
                    assert_eq!(nodes[0].id, "29b9b1df-e10c-574d-ba78-09abe0b5549e");
                }
                other => panic!(
                    "expected a one-item Collection (not a bare Node) for an InboundForward-resolved \
                     many-relationship with exactly one current match, got {:?}",
                    other
                ),
            }
        }

        /// A sibling to the zero-match case above, but with a REAL edge
        /// attached to a subtype instance via the extends chain -- before
        /// the fix, this still resolved to an empty `Collection`,
        /// indistinguishable from "nothing attached".
        ///
        /// A forward name must be looked up in the ADR-078
        /// `extends`-chain-merged set `resolve_relationships` provides, not
        /// in the type's own directly-declared relationships. Looked up in
        /// the latter, `items`, declared only on `gr-ext-base` and inherited
        /// (not redeclared) by `gr-ext-sub`, is invisible: the name resolves
        /// as undeclared, which a path treats as an empty result -- even
        /// though the write path (`create_relationship` ->
        /// `resolve_declared_relationship`) is already chain-aware and
        /// happily attached the edge below. The sibling test above only
        /// covers the zero-match *classification* fix; it never exercised
        /// this fetch, so a genuinely populated edge on a subtype instance
        /// stayed silently unreadable until now.
        #[tokio::test(flavor = "multi_thread")]
        async fn inherited_relationship_with_real_edge_resolves_to_populated_collection() {
            let (svc, _tmp) = create_test_service().await;

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr-ext-item2",
                    "fields": []
                }),
            )
            .await
            .expect("target schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr-ext-base2",
                    "fields": [],
                    "relationships": [{
                        "name": "items",
                        "targetType": "gr-ext-item2",
                        "direction": "out",
                        "cardinality": "many",
                        "reverseName": "parent",
                        "reverseCardinality": "one"
                    }]
                }),
            )
            .await
            .expect("base schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr-ext-sub2",
                    "extends": "gr-ext-base2",
                    "fields": []
                }),
            )
            .await
            .expect("subtype schema creation failed");

            // A subtype instance with a REAL edge attached, even though
            // `items` is only declared on the ancestor schema.
            let parent = make_node(
                "a2edd255-83dd-5de3-ba8d-76e3fa4195e9",
                "gr-ext-sub2",
                json!({}),
            );
            svc.create_node(parent.clone()).await.unwrap();
            let item = make_node(
                "0224d98d-8686-5ae6-9f57-4d386d5fd0ed",
                "gr-ext-item2",
                json!({}),
            );
            svc.create_node(item.clone()).await.unwrap();

            svc.create_relationship(
                "a2edd255-83dd-5de3-ba8d-76e3fa4195e9",
                "items",
                "0224d98d-8686-5ae6-9f57-4d386d5fd0ed",
                json!({}),
            )
            .await
            .expect(
                "create_relationship must succeed for an inherited relationship -- the \
                     write path is already extends-chain aware",
            );

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&parent, &["items".to_string()]).await;
            match result {
                ResolvedValue::Collection(nodes) => {
                    assert_eq!(
                        nodes.len(),
                        1,
                        "expected the real attached edge to be readable through the \
                         extends chain, got {} nodes",
                        nodes.len()
                    );
                    assert_eq!(nodes[0].id, "0224d98d-8686-5ae6-9f57-4d386d5fd0ed");
                }
                other => panic!(
                    "expected a populated Collection containing the real attached edge, got {:?}",
                    other
                ),
            }
        }

        // -- Batched resolution: one statement per path for a whole scan --

        /// `count` items, each linked to its own story, each story to its own
        /// epic: item i → story i → epic i. Returns the items.
        async fn seed_item_chains(svc: &Arc<NodeService>, prefix: &str, count: usize) -> Vec<Node> {
            let (epic, story, item) = (
                format!("{prefix}-epic"),
                format!("{prefix}-story"),
                format!("{prefix}-item"),
            );
            create_schema(svc, &epic, json!([])).await;
            create_schema(
                svc,
                &story,
                json!([{
                    "name": "epic", "targetType": epic, "direction": "out",
                    "cardinality": "one", "reverseName": "stories", "reverseCardinality": "many"
                }]),
            )
            .await;
            create_schema(
                svc,
                &item,
                json!([{
                    "name": "story", "targetType": story, "direction": "out",
                    "cardinality": "one", "reverseName": "items", "reverseCardinality": "many"
                }]),
            )
            .await;

            let id = |kind: u8, i: usize| format!("b{kind}000000-0000-4000-8000-{i:012}");
            let mut items = Vec::with_capacity(count);
            for i in 0..count {
                let (epic_id, story_id, item_id) = (id(1, i), id(2, i), id(3, i));
                svc.create_node(make_node(
                    &epic_id,
                    &epic,
                    json!({"status": format!("epic-{i}")}),
                ))
                .await
                .unwrap();
                svc.create_node(make_node(&story_id, &story, json!({"status": "open"})))
                    .await
                    .unwrap();
                let node = make_node(&item_id, &item, json!({"status": "open"}));
                svc.create_node(node.clone()).await.unwrap();
                svc.create_relationship(&story_id, "epic", &epic_id, json!({}))
                    .await
                    .unwrap();
                svc.create_relationship(&item_id, "story", &story_id, json!({}))
                    .await
                    .unwrap();
                items.push(node);
            }
            items
        }

        fn segments(path: &[&str]) -> Vec<String> {
            path.iter().map(|s| s.to_string()).collect()
        }

        /// A path is resolved for every root of a scan in ONE statement: the
        /// cost grows with the number of distinct paths, not with roots times
        /// hops. And each root gets its own answer.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_path_resolves_for_every_root_in_one_statement() {
            let (svc, _tmp) = create_test_service().await;
            let items = seed_item_chains(&svc, "gr-batch", 25).await;

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let resolved = resolver
                .resolve_path_for(&items, &segments(&["story", "epic", "status"]))
                .await;

            assert_eq!(
                resolver.statements_run(),
                1,
                "two hops for 25 roots must be one statement, not 50 walks"
            );
            assert_eq!(resolved.len(), items.len());
            for (i, item) in items.iter().enumerate() {
                match &resolved[&item.id] {
                    ResolvedValue::Scalar(status) => assert_eq!(
                        status,
                        &json!(format!("epic-{i}")),
                        "item {i} must read its own epic, not another root's"
                    ),
                    other => panic!("item {i}: expected its epic's status, got {other:?}"),
                }
            }

            // Everything the statement reached is now cached per root, so the
            // prefixes and a second read cost nothing more.
            for item in &items {
                assert!(matches!(
                    resolver.resolve_path(item, &segments(&["story"])).await,
                    ResolvedValue::Node(_)
                ));
                assert!(matches!(
                    resolver
                        .resolve_path(item, &segments(&["story", "epic"]))
                        .await,
                    ResolvedValue::Node(_)
                ));
            }
            assert_eq!(resolver.statements_run(), 1);
        }

        /// `resolve_ahead` is what a scheduled scan calls: every distinct
        /// path the conditions read, one statement each, for all the nodes.
        #[tokio::test(flavor = "multi_thread")]
        async fn resolving_ahead_costs_one_statement_per_distinct_path() {
            use crate::db::events::DomainEvent;
            use crate::playbook::cel::{
                evaluate_conditions_at_scope, CompiledCondition, ConditionResult,
            };

            let (svc, _tmp) = create_test_service().await;
            let items = seed_item_chains(&svc, "gr-ahead", 10).await;

            let conditions = [
                // A prefix of the next path, a repeat of it, and a property
                // of the node itself: none costs a statement of its own.
                CompiledCondition::compile("node.story.status == 'open'").unwrap(),
                CompiledCondition::compile("node.story.epic.status != 'done'").unwrap(),
                CompiledCondition::compile("node.story.status != 'blocked'").unwrap(),
                CompiledCondition::compile("node.status == 'open'").unwrap(),
                // A different relationship is a different path.
                CompiledCondition::compile("node.child_of.status != 'done'").unwrap(),
            ];
            let (paths, collections) = crate::playbook::cel::condition_paths(&conditions);

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            resolver.resolve_ahead(&items, &paths, &collections).await;
            assert_eq!(
                resolver.statements_run(),
                2,
                "one statement for `story.epic` (which also answers `story`), one for `child_of`"
            );

            // Evaluating each node afterwards reads the cache.
            for item in &items {
                let event = DomainEvent::NodeCreated {
                    node_type: item.node_type.clone(),
                    node_id: item.id.clone(),
                };
                let result = evaluate_conditions_at_scope(
                    &conditions,
                    item,
                    &event,
                    Some(&mut resolver),
                    None,
                )
                .await;
                // No item has a parent, so the last condition is not met; the
                // point is that deciding so read nothing further.
                assert!(
                    matches!(result, ConditionResult::Fail { condition_index: 4 }),
                    "{result:?}"
                );
            }
            assert_eq!(resolver.statements_run(), 2);
        }

        /// What a scan resolved is handed to each work item's resolver keyed
        /// by root: a node starts from its own answers and never another's.
        #[tokio::test(flavor = "multi_thread")]
        async fn seeding_takes_only_the_roots_own_paths() {
            let (svc, _tmp) = create_test_service().await;
            let items = seed_item_chains(&svc, "gr-seed", 2).await;
            let path = segments(&["story", "status"]);

            let mut scan = GraphResolver::new(Arc::clone(&svc));
            scan.resolve_path_for(&items, &path).await;
            let resolved = scan.into_cache();
            assert_eq!(resolved.len(), 2, "one entry per root");

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            resolver.seed(&items[0].id, &resolved);

            // The seeded root reads its answer without a statement.
            assert!(matches!(
                resolver.resolve_path(&items[0], &path).await,
                ResolvedValue::Scalar(_)
            ));
            assert_eq!(resolver.statements_run(), 0);

            // The other root was not seeded, so it is resolved for itself.
            assert!(matches!(
                resolver.resolve_path(&items[1], &path).await,
                ResolvedValue::Scalar(_)
            ));
            assert_eq!(resolver.statements_run(), 1);
        }

        /// A hop the schemas cannot name from here (a declared relationship
        /// after a built-in one, which any type may sit at the end of) is
        /// resolved from the concrete nodes the walk reached: a second
        /// statement for the whole group, still not one per node.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_declared_hop_after_a_builtin_one_is_resolved_from_the_nodes_reached() {
            let (svc, _tmp) = create_test_service().await;
            let items = seed_item_chains(&svc, "gr-stage", 6).await;

            // A note under each item: note → child_of → item → story.
            let mut notes = Vec::new();
            for (i, item) in items.iter().enumerate() {
                let note = make_node(
                    &format!("b4000000-0000-4000-8000-{i:012}"),
                    "text",
                    json!({}),
                );
                svc.create_node(note.clone()).await.unwrap();
                svc.create_relationship(&item.id, "has_child", &note.id, json!({}))
                    .await
                    .unwrap();
                notes.push(note);
            }

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let resolved = resolver
                .resolve_path_for(&notes, &segments(&["child_of", "story", "epic", "status"]))
                .await;

            assert_eq!(
                resolver.statements_run(),
                2,
                "one statement to the parents, one from the parents on"
            );
            for (i, note) in notes.iter().enumerate() {
                assert!(
                    matches!(&resolved[&note.id], ResolvedValue::Scalar(v) if v == &json!(format!("epic-{i}"))),
                    "note {i}: {:?}",
                    resolved[&note.id]
                );
            }
        }
    }
}
