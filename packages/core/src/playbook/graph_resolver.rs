//! Graph Resolver for the Playbook Engine
//!
//! Resolves dot-paths by walking the data graph via NodeService.
//! Created per rule evaluation, with a segment cache to prevent
//! redundant DB queries for overlapping paths.
//!
//! All graph traversal happens here, ahead of CEL evaluation: paths are
//! resolved with plain `async`/`.await` against NodeService, and the
//! results are injected into the CEL context before `Program::execute()`
//! (which is synchronous) ever runs. This keeps the sync/async boundary
//! at the CEL-evaluation edge instead of bridging it internally.

use crate::models::Node;
use crate::ops::rel_ops::{self, ResolvedRelName};
use crate::ops::OpsError;
use crate::playbook::cel::{json_to_cel, key, scoped_node_value};
use crate::playbook::path_extractor::{CollectionPath, ExtractedPath};
use crate::services::NodeService;
use cel_interpreter::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

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
}

/// Resolves dot-paths against the live data graph.
///
/// Created per work item in the RuleProcessor. Caches resolved segments
/// to avoid redundant DB queries for overlapping paths across conditions
/// in the same rule.
pub struct GraphResolver {
    node_service: Arc<NodeService>,
    /// Cache: (root node id, path segments) → resolved value.
    ///
    /// The root id is part of the key because the same path means different
    /// things from different nodes — `child_of` from one task is not `child_of`
    /// from another. Callers create one resolver per work item, so in practice
    /// a single root dominates, but keying on segments alone would silently
    /// serve one node's answer for another's the moment that stopped holding.
    cache: HashMap<(String, Vec<String>), ResolvedValue>,
    /// The scope a resolved node's CEL value is built at (ADR-078).
    ///
    /// A node reached by traversal is read at the *reading* scope, exactly as
    /// the trigger node is: a Play registered on `task` sees a `bug` child's
    /// `task` fields, with `bug`-only values resolved through `maps_to`.
    /// Without it the child is built at its own scope and an extended value
    /// (`backlog`) reaches a base-scoped condition raw, never matching — a
    /// silent false, not an error.
    ///
    /// `None` reads at each node's own scope, which is every Play in a
    /// database where nothing declares `extends`.
    scope: Option<crate::playbook::cel::CelScope>,
}

impl GraphResolver {
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self {
            node_service,
            cache: HashMap::new(),
            scope: None,
        }
    }

    /// Set the scope resolved nodes are read at. See [`GraphResolver::scope`].
    pub fn with_scope(mut self, scope: Option<crate::playbook::cel::CelScope>) -> Self {
        self.set_scope(scope);
        self
    }

    /// Point an existing resolver at a different reading scope.
    ///
    /// One resolver is reused across the rules of a work item, and each rule
    /// carries its own registered scope. The segment cache holds `ResolvedValue`s
    /// — raw `Node`s, not yet projected — so it stays valid across a scope
    /// change and is deliberately kept: projection happens at read time in
    /// `enrich_context`, after the cache is consulted.
    pub fn set_scope(&mut self, scope: Option<crate::playbook::cel::CelScope>) {
        self.scope = scope;
    }

    /// Resolve a dot-path starting from a root node.
    ///
    /// Walks segments left-to-right:
    /// 1. Check if the segment is a property on the current node → Scalar
    /// 2. If not, try as a relationship name → fetch related node(s)
    /// 3. For "one" relationships, continue walking with the target node
    /// 4. For "many" relationships, return Collection
    ///
    /// A relationship segment may name either side of an edge: `has_child`
    /// walks to the children, `child_of` to the parent. Reverse segments walk
    /// and chain exactly like forward ones (`node.assignee.email`), since
    /// direction is resolved per segment inside `fetch_related_nodes`.
    ///
    /// Uses the segment cache: if a prefix has already been resolved, starts from there.
    pub async fn resolve_path(&mut self, root_node: &Node, segments: &[String]) -> ResolvedValue {
        if segments.is_empty() {
            return ResolvedValue::Node(root_node.clone());
        }

        // Every cache entry is scoped to the node this walk started from.
        let root_id = root_node.id.clone();
        let cache_key = |segs: &[String]| (root_id.clone(), segs.to_vec());

        // Check cache for the full path first
        if let Some(cached) = self.cache.get(&cache_key(segments)) {
            return cached.clone();
        }

        // Find the longest cached prefix
        let mut start_idx = 0;
        let mut current_node = root_node.clone();

        for i in (1..segments.len()).rev() {
            let prefix = &segments[..i];
            if let Some(cached) = self.cache.get(&cache_key(prefix)) {
                match cached {
                    ResolvedValue::Node(n) => {
                        current_node = n.clone();
                        start_idx = i;
                        break;
                    }
                    ResolvedValue::Collection(_) | ResolvedValue::Scalar(_) => {
                        // Can't continue walking from a collection or scalar
                        let result = ResolvedValue::Missing;
                        self.cache.insert(cache_key(segments), result.clone());
                        return result;
                    }
                    ResolvedValue::Missing => {
                        let result = ResolvedValue::Missing;
                        self.cache.insert(cache_key(segments), result.clone());
                        return result;
                    }
                }
            }
        }

        // Walk remaining segments
        for i in start_idx..segments.len() {
            let segment = &segments[i];
            let is_last = i == segments.len() - 1;

            // A core Node field is not a property and lives in no bucket, so
            // the property lookup below cannot see it. Without this, walking to
            // a related node and reading its identity — `node.child_of.id`, the
            // shape an action needs to address that node — resolves to
            // `Missing` and fails the action, even though `node.child_of`
            // alone resolves fine.
            //
            // Checked before properties so these names mean the node's
            // identity consistently, rather than being shadowed by a
            // same-named user property on some types but not others.
            //
            // This walk starts at `i == 0`, so the rule applies to the ROOT
            // node's own first segment as well as to traversed nodes: a task
            // storing a user property literally named `content` resolves
            // `node.content` to the struct field, not that property. Core-wins
            // is the deliberate choice — it matches what `node.id` already
            // means in every CEL condition (`cel.rs`'s `is_core_key`), and the
            // alternative would make a path's meaning depend on which types
            // happen to declare a colliding field.
            if let Some(core_val) = core_field_value(&current_node, segment) {
                let result = ResolvedValue::Scalar(core_val);
                self.cache
                    .insert(cache_key(&segments[..=i]), result.clone());
                if is_last {
                    self.cache.insert(cache_key(segments), result.clone());
                    return result;
                }
                // A scalar is terminal: there is nothing to walk into.
                let missing = ResolvedValue::Missing;
                self.cache.insert(cache_key(segments), missing.clone());
                return missing;
            }

            // Try as a property first (check node.properties)
            if let Some(prop_val) = get_node_property(&current_node, segment) {
                let result = ResolvedValue::Scalar(prop_val);
                self.cache
                    .insert(cache_key(&segments[..=i]), result.clone());
                if is_last {
                    self.cache.insert(cache_key(segments), result.clone());
                    return result;
                }
                // Can't walk further into a scalar
                let missing = ResolvedValue::Missing;
                self.cache.insert(cache_key(segments), missing.clone());
                return missing;
            }

            // Try as a relationship
            let related = self.fetch_related_nodes(&current_node, segment).await;

            // A relationship's DECLARED "many" cardinality means its
            // resolved shape must always be a Collection, regardless of how
            // many rows CURRENTLY match (0, 1, or N) -- inferring shape
            // purely from the current row count, as the fallback below does
            // for relationships this lookup can't identify (an undeclared
            // segment/typo, or one of the four built-ins, which predate
            // per-relationship cardinality metadata), makes a declared
            // "many" relationship's resolved type silently flip between
            // Missing/Node/Collection as its item count crosses 0 and 1.
            // That's wrong on both sides: a Cycle with exactly one Issue is
            // not "the Issue itself" the way walking a genuine "one"
            // relationship would be, and a Cycle with zero Issues yet is an
            // ordinary state, not a missing/misconfigured path. Returning
            // Missing/Node instead of an empty/one-item Collection here made
            // `for_each` (and this engine's `sum`/`count` aggregate calls,
            // which resolve their collection through this same function)
            // hard-fail an action -- disabling the WHOLE play (see
            // `rule_processor_loop`'s `ActionResult::Failed` handling) --
            // for a Cycle with zero or exactly one Issue, an entirely
            // ordinary and common state.
            //
            // Only checked for 0/1 current matches: for N>=2 the fallback
            // below already returns Collection(nodes) when `is_last` (and
            // Missing otherwise) regardless of declared cardinality, so the
            // outcome is identical either way -- skipping the schema lookup
            // there avoids an extra DB round trip on the common multi-match
            // path, where it can't change anything.
            //
            // Walking FURTHER into a many-relationship (any current count)
            // past this segment stays unsupported by this simple dot-path
            // walk, same as the existing N>=2 case already enforced -- this
            // only changes the TERMINAL-segment shape.
            let ambiguous_match_count = matches!(&related, Ok(nodes) if nodes.len() <= 1);
            if ambiguous_match_count
                && self
                    .is_declared_many_relationship(&current_node.node_type, segment)
                    .await
            {
                let result = match related {
                    Ok(nodes) => ResolvedValue::Collection(nodes),
                    Err(e) => {
                        warn!(
                            "Failed to fetch related nodes for {}.{}: {}",
                            current_node.id, segment, e
                        );
                        ResolvedValue::Missing
                    }
                };
                self.cache
                    .insert(cache_key(&segments[..=i]), result.clone());
                if is_last {
                    self.cache.insert(cache_key(segments), result.clone());
                    return result;
                }
                // Can't walk further into a collection with simple dot-path.
                let missing = ResolvedValue::Missing;
                self.cache.insert(cache_key(segments), missing.clone());
                return missing;
            }

            match related {
                Ok(nodes) if nodes.is_empty() => {
                    let result = ResolvedValue::Missing;
                    self.cache
                        .insert(cache_key(&segments[..=i]), result.clone());
                    self.cache.insert(cache_key(segments), result.clone());
                    return result;
                }
                Ok(nodes) if nodes.len() == 1 => {
                    let node = nodes.into_iter().next().unwrap();
                    self.cache.insert(
                        cache_key(&segments[..=i]),
                        ResolvedValue::Node(node.clone()),
                    );
                    if is_last {
                        let result = ResolvedValue::Node(node);
                        self.cache.insert(cache_key(segments), result.clone());
                        return result;
                    }
                    current_node = node;
                }
                Ok(nodes) => {
                    // Multiple related nodes — this is a collection
                    let result = ResolvedValue::Collection(nodes);
                    self.cache
                        .insert(cache_key(&segments[..=i]), result.clone());
                    if is_last {
                        self.cache.insert(cache_key(segments), result.clone());
                        return result;
                    }
                    // Can't walk further into a collection with simple dot-path
                    let missing = ResolvedValue::Missing;
                    self.cache.insert(cache_key(segments), missing.clone());
                    return missing;
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch related nodes for {}.{}: {}",
                        current_node.id, segment, e
                    );
                    let result = ResolvedValue::Missing;
                    self.cache.insert(cache_key(segments), result.clone());
                    return result;
                }
            }
        }

        ResolvedValue::Node(current_node)
    }

    /// Resolve a collection path and return the collection nodes.
    pub async fn resolve_collection(
        &mut self,
        root_node: &Node,
        collection: &ExtractedPath,
    ) -> Vec<Node> {
        // The collection path is like ["node", "tasks"] — skip "node" (the root)
        let segments = &collection.segments;
        if segments.len() < 2 {
            return vec![];
        }

        match self.resolve_path(root_node, &segments[1..]).await {
            ResolvedValue::Collection(nodes) => nodes,
            ResolvedValue::Node(n) => vec![n],
            _ => vec![],
        }
    }

    /// Fetch related nodes via NodeService, in whichever direction the segment
    /// names.
    ///
    /// A path segment may spell either side of a relationship. The forward name
    /// traverses outbound, exactly as before; a reverse name — a built-in's
    /// fixed inverse (`child_of`) or a schema's declared `reverse_name`
    /// (`assignee`) — addresses the same stored row from its other end, so it
    /// rewrites the name to the forward spelling and queries inbound. Resolution
    /// is shared with the CLI's read path ([`rel_ops::resolve_relationship_name`])
    /// so both answer a given name identically.
    ///
    /// Unlike that path, an unresolvable name is NOT an error here. The resolver
    /// tries every segment as a relationship only after it fails as a property,
    /// so "not a relationship either" is the ordinary way a path turns out to be
    /// `Missing` — which CEL renders as a false condition. Surfacing it as an
    /// error would make every non-matching Play condition log a warning.
    async fn fetch_related_nodes(
        &self,
        node: &Node,
        relationship_name: &str,
    ) -> Result<Vec<Node>, String> {
        let resolved = match rel_ops::resolve_relationship_name(
            &self.node_service,
            &node.id,
            &node.node_type,
            relationship_name,
        )
        .await
        {
            Ok(resolved) => resolved,
            // Undeclared in either direction — an empty traversal, not a
            // failure. This is the ordinary way a path turns out missing.
            Err(OpsError::InvalidParams(_)) => return Ok(vec![]),
            // Anything else is infrastructure failing (an unreadable schema, a
            // locked database), not a statement about this path. Propagate it
            // so it is logged and the condition is not quietly false — the same
            // treatment the `get_related_nodes` call below already gets.
            Err(e) => return Err(e.to_string()),
        };

        let (name, direction, source_type) = match &resolved {
            ResolvedRelName::Builtin | ResolvedRelName::Forward => {
                (relationship_name.to_string(), "out", None)
            }
            // The node sits at the far end of someone else's forward
            // declaration, so the edge is already stored pointing at it.
            ResolvedRelName::InboundForward => (relationship_name.to_string(), "in", None),
            ResolvedRelName::Reverse {
                forward_name,
                source_type,
            } => (forward_name.clone(), "in", source_type.clone()),
        };

        let nodes = self
            .node_service
            .get_related_nodes(&node.id, &name, direction)
            .await
            .map_err(|e| e.to_string())?;

        // The store keys an "in" query on relationship_type alone, so every
        // schema declaring this forward name toward this type answers. A reverse
        // name belongs to exactly one of them — keep only that declarer's nodes.
        // This is live, not hypothetical: `tasks` is declared both on `project`
        // (reverse `project`) and on `person` (reverse `assignee`), so an
        // unnarrowed `node.assignee` would return the project too.
        //
        // Matched against the declarer's whole descendant set, not its exact
        // id: `task.blocks` declares reverse `blocked_by` with source_type
        // `task`, and an `issue` IS a task (ADR-078), so an issue blocking an
        // issue must survive this filter. Comparing the concrete type alone
        // silently dropped every subtype instance, which read as "nothing
        // blocks this" rather than as an error.
        let Some(source_type) = source_type else {
            return Ok(nodes);
        };
        //
        // Memoized per node_type rather than per node: a traversal commonly
        // returns many nodes of one type, and the chain is a property of the
        // type, so resolving it once per distinct type is the same answer for
        // a fraction of the queries.
        let mut verdict: HashMap<String, bool> = HashMap::new();
        let mut kept = Vec::with_capacity(nodes.len());
        for n in nodes {
            let satisfies = match verdict.get(&n.node_type) {
                Some(known) => *known,
                None => {
                    let chain = self
                        .node_service
                        .resolve_type_chain(&n.node_type)
                        .await
                        .map_err(|e| e.to_string())?;
                    let answer = chain.contains(&source_type);
                    verdict.insert(n.node_type.clone(), answer);
                    answer
                }
            };
            if satisfies {
                kept.push(n);
            }
        }
        Ok(kept)
    }

    /// Whether `segment` is declared as a "many" cardinality relationship on
    /// `node_type`, checked against the *effective* relationship set --
    /// `node_type`'s own directly-declared relationships plus everything
    /// inherited across the ADR-078 `extends` chain (`resolve_relationships`),
    /// not just this schema's own declarations. A relationship declared only
    /// on an ancestor schema and inherited (not redeclared) by `node_type`
    /// must still be recognized here, the same extends-chain gap fixed for
    /// `resolve_field_owners`/`resolve_relationships`'s other callers.
    ///
    /// Only called when a relationship fetch already returned zero or
    /// exactly one row -- the only counts where cardinality can change the
    /// resolved shape (see the call site's doc: for two or more rows the
    /// outcome is identical regardless of declared cardinality, so callers
    /// skip this lookup there). Distinguishes "no such relationship" from "a
    /// declared many-relationship with zero or one current matches", which
    /// the raw row count alone can't tell apart. Any lookup failure (schema
    /// not found, service error) conservatively resolves to `false` -- i.e.
    /// today's existing row-count-only behavior -- rather than guessing.
    async fn is_declared_many_relationship(&self, node_type: &str, segment: &str) -> bool {
        matches!(
            self.node_service.resolve_relationships(node_type).await,
            Ok((rels, _owners)) if rels.iter().any(|r| {
                r.name == segment
                    && r.cardinality == crate::models::schema::RelationshipCardinality::Many
            })
        )
    }

    /// Build an enriched CEL context with graph-resolved paths.
    ///
    /// Takes the base node and extracted paths, resolves each path against
    /// the graph, and injects the resolved values as nested CEL Maps.
    pub async fn enrich_context(
        &mut self,
        root_node: &Node,
        paths: &[ExtractedPath],
        collections: &[CollectionPath],
    ) -> HashMap<Vec<String>, Value> {
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
            if path.segments.len() == 2 && get_node_property(root_node, &path.segments[1]).is_some()
            {
                continue;
            }

            // Resolve the relationship chain (skip "node" prefix)
            let segments = &path.segments[1..];
            match self.resolve_path(root_node, segments).await {
                ResolvedValue::Node(n) => {
                    resolved_values.insert(
                        path.segments.clone(),
                        scoped_node_value(&n, self.scope.as_ref()),
                    );
                }
                ResolvedValue::Scalar(v) => {
                    resolved_values.insert(path.segments.clone(), json_to_cel(&v));
                }
                ResolvedValue::Collection(nodes) => {
                    let list: Vec<Value> = nodes
                        .iter()
                        .map(|n| scoped_node_value(n, self.scope.as_ref()))
                        .collect();
                    resolved_values.insert(path.segments.clone(), Value::List(list.into()));
                }
                ResolvedValue::Missing => {
                    // Missing path → will evaluate to false via NoSuchKey in CEL
                }
            }
        }

        // Resolve collection paths
        for coll in collections {
            if coll.collection.root != "node" {
                continue;
            }
            let nodes = self.resolve_collection(root_node, &coll.collection).await;
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
                let list: Vec<Value> = nodes
                    .iter()
                    .map(|n| scoped_node_value(n, self.scope.as_ref()))
                    .collect();
                resolved_values.insert(coll.collection.segments.clone(), Value::List(list.into()));
            }
        }

        resolved_values
    }
}

/// A core Node field read by name, or `None` if the name is not one.
///
/// These are node identity/metadata, not schema properties: they live in their
/// own struct fields rather than in any type bucket, so no property lookup can
/// reach them. The same set `cel.rs` exposes on a CEL `node` map (`is_core_key`)
/// — the two must agree, or a name resolves in a condition but not in the
/// action binding that acts on it.
fn core_field_value(node: &Node, name: &str) -> Option<serde_json::Value> {
    match name {
        "id" => Some(serde_json::Value::String(node.id.clone())),
        "node_type" => Some(serde_json::Value::String(node.node_type.clone())),
        "content" => Some(serde_json::Value::String(node.content.clone())),
        "version" => Some(serde_json::Value::from(node.version)),
        "lifecycle_status" => Some(serde_json::Value::String(node.lifecycle_status.clone())),
        _ => None,
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
    for (path, value) in resolved {
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
                    "fields": [{"name": "status", "type": "string"}, {"name": "title", "type": "string"}]
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
            create_schema(&svc, "gr_task", json!([])).await;

            let node = make_node("gr-t1", "gr_task", json!({"status": "open"}));
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
            create_schema(&svc, "gr_task2", json!([])).await;

            let node = make_node("gr-t2", "gr_task2", json!({"status": "open"}));
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

            // Create schemas: gr_story has no rels, gr_issue -> story
            create_schema(&svc, "gr_story", json!([])).await;
            create_schema(
                &svc,
                "gr_issue",
                json!([{
                    "name": "story",
                    "targetType": "gr_story",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            // Create nodes
            let story = make_node("gr-s1", "gr_story", json!({"status": "active"}));
            svc.create_node(story.clone()).await.unwrap();

            let issue = make_node("gr-i1", "gr_issue", json!({"status": "open"}));
            svc.create_node(issue.clone()).await.unwrap();

            // Create relationship
            svc.create_relationship("gr-i1", "story", "gr-s1", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&issue, &["story".to_string()]).await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-s1"),
                other => panic!("expected Node, got {:?}", other),
            }
        }

        /// Regression test: resolve_path/resolve_collection/enrich_context must
        /// work on a single-threaded runtime. The old `block_in_place` bridge
        /// would panic here; this proves the fix, not just its absence.
        #[tokio::test(flavor = "current_thread")]
        async fn resolve_path_and_enrich_context_work_under_current_thread_runtime() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr_story_ct", json!([])).await;
            create_schema(
                &svc,
                "gr_issue_ct",
                json!([{
                    "name": "story",
                    "targetType": "gr_story_ct",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let story = make_node("gr-s-ct1", "gr_story_ct", json!({"status": "active"}));
            svc.create_node(story.clone()).await.unwrap();

            let issue = make_node("gr-i-ct1", "gr_issue_ct", json!({"status": "open"}));
            svc.create_node(issue.clone()).await.unwrap();

            svc.create_relationship("gr-i-ct1", "story", "gr-s-ct1", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&issue, &["story".to_string()]).await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-s-ct1"),
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
            let enriched = resolver.enrich_context(&issue, &paths, &[]).await;
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

            // Chain: gr_task3 -> story -> epic
            create_schema(&svc, "gr_epic", json!([])).await;
            create_schema(
                &svc,
                "gr_story3",
                json!([{
                    "name": "epic",
                    "targetType": "gr_epic",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "stories",
                    "reverseCardinality": "many"
                }]),
            )
            .await;
            create_schema(
                &svc,
                "gr_task3",
                json!([{
                    "name": "story",
                    "targetType": "gr_story3",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let epic = make_node("gr-e1", "gr_epic", json!({"status": "in_progress"}));
            svc.create_node(epic).await.unwrap();

            let story = make_node("gr-s3", "gr_story3", json!({"status": "active"}));
            svc.create_node(story).await.unwrap();

            let task = make_node("gr-t3", "gr_task3", json!({"status": "open"}));
            svc.create_node(task.clone()).await.unwrap();

            svc.create_relationship("gr-t3", "story", "gr-s3", json!({}))
                .await
                .unwrap();
            svc.create_relationship("gr-s3", "epic", "gr-e1", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));

            // Resolve task -> story -> epic
            let result = resolver
                .resolve_path(&task, &["story".to_string(), "epic".to_string()])
                .await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-e1"),
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

            create_schema(&svc, "gr_item_empty", json!([])).await;
            create_schema(
                &svc,
                "gr_parent_empty",
                json!([{
                    "name": "items",
                    "targetType": "gr_item_empty",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "parent",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            // Parent created with NO items ever attached -- the exact
            // "freshly created Cycle with no Issues yet" shape.
            let parent = make_node("gr-p-empty", "gr_parent_empty", json!({}));
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

            create_schema(&svc, "gr_cycle_unset", json!([])).await;
            create_schema(
                &svc,
                "gr_issue_unset",
                json!([{
                    "name": "cycle",
                    "targetType": "gr_cycle_unset",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            // Issue created with NO cycle relationship ever added.
            let issue = make_node("gr-i-unset", "gr_issue_unset", json!({}));
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
        /// path) must still resolve to Missing -- `is_declared_many_relationship`
        /// must not produce a false positive just because the fetch happened
        /// to return zero rows.
        #[tokio::test(flavor = "multi_thread")]
        async fn undeclared_segment_still_resolves_to_missing() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr_lonely", json!([])).await;
            let node = make_node("gr-lonely-1", "gr_lonely", json!({}));
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

            create_schema(&svc, "gr_story4", json!([])).await;
            create_schema(
                &svc,
                "gr_task4",
                json!([{
                    "name": "story",
                    "targetType": "gr_story4",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let story = make_node("gr-s4", "gr_story4", json!({"status": "done"}));
            svc.create_node(story).await.unwrap();
            let task = make_node("gr-t4", "gr_task4", json!({}));
            svc.create_node(task.clone()).await.unwrap();
            svc.create_relationship("gr-t4", "story", "gr-s4", json!({}))
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

            create_schema(&svc, "gr_subtask", json!([])).await;
            create_schema(
                &svc,
                "gr_parent",
                json!([{
                    "name": "subtasks",
                    "targetType": "gr_subtask",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "parent_task",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let sub1 = make_node("gr-sub1", "gr_subtask", json!({"status": "done"}));
            let sub2 = make_node("gr-sub2", "gr_subtask", json!({"status": "open"}));
            svc.create_node(sub1).await.unwrap();
            svc.create_node(sub2).await.unwrap();

            let parent = make_node("gr-p1", "gr_parent", json!({}));
            svc.create_node(parent.clone()).await.unwrap();

            svc.create_relationship("gr-p1", "subtasks", "gr-sub1", json!({}))
                .await
                .unwrap();
            svc.create_relationship("gr-p1", "subtasks", "gr-sub2", json!({}))
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
                    assert!(ids.contains(&"gr-sub1"));
                    assert!(ids.contains(&"gr-sub2"));
                }
                other => panic!("expected Collection, got {:?}", other),
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn enrich_context_builds_cel_values() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr_target5", json!([])).await;
            create_schema(
                &svc,
                "gr_source5",
                json!([{
                    "name": "target",
                    "targetType": "gr_target5",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "sources",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let target = make_node("gr-tgt5", "gr_target5", json!({"status": "ready"}));
            svc.create_node(target).await.unwrap();
            let source = make_node("gr-src5", "gr_source5", json!({}));
            svc.create_node(source.clone()).await.unwrap();
            svc.create_relationship("gr-src5", "target", "gr-tgt5", json!({}))
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

            let result = resolver.enrich_context(&source, &paths, &[]).await;
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
            create_schema(&svc, "gr_task6", json!([])).await;

            let node = make_node("gr-t6", "gr_task6", json!({}));
            svc.create_node(node.clone()).await.unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver.resolve_path(&node, &[]).await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-t6"),
                other => panic!("expected Node, got {:?}", other),
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn cannot_walk_past_scalar() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr_task7", json!([])).await;

            let node = make_node("gr-t7", "gr_task7", json!({"status": "open"}));
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

            // Chain: gr_task8 -> story8 -> epic8
            create_schema(&svc, "gr_epic8", json!([])).await;
            create_schema(
                &svc,
                "gr_story8",
                json!([{
                    "name": "epic",
                    "targetType": "gr_epic8",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "stories",
                    "reverseCardinality": "many"
                }]),
            )
            .await;
            create_schema(
                &svc,
                "gr_task8",
                json!([{
                    "name": "story",
                    "targetType": "gr_story8",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let epic = make_node("gr-e8", "gr_epic8", json!({"status": "in_progress"}));
            svc.create_node(epic).await.unwrap();
            let story = make_node("gr-s8", "gr_story8", json!({"status": "active"}));
            svc.create_node(story).await.unwrap();
            let task = make_node("gr-t8", "gr_task8", json!({"status": "open"}));
            svc.create_node(task.clone()).await.unwrap();

            svc.create_relationship("gr-t8", "story", "gr-s8", json!({}))
                .await
                .unwrap();
            svc.create_relationship("gr-s8", "epic", "gr-e8", json!({}))
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

            let result = resolver.enrich_context(&task, &paths, &[]).await;

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

        #[tokio::test(flavor = "multi_thread")]
        async fn enrich_context_multi_hop_path_excludes_internal_key() {
            // A multi-hop CEL path (node.related_node._internalKey, segment
            // length > 2) is resolved via get_node_property, not
            // node_to_cel_value -- this exercises that a `_`-prefixed
            // internal-bookkeeping value on a RELATED node stays unreadable,
            // matching the direct-node case already enforced by
            // node_to_cel_value.
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr_related11", json!([])).await;
            create_schema(
                &svc,
                "gr_root11",
                json!([{
                    "name": "related_node",
                    "targetType": "gr_related11",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "roots",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let related = make_node(
                "gr-rel11",
                "gr_related11",
                json!({"status": "active", "_playbookChainDepth": 7}),
            );
            svc.create_node(related).await.unwrap();
            let root = make_node("gr-root11", "gr_root11", json!({}));
            svc.create_node(root.clone()).await.unwrap();
            svc.create_relationship("gr-root11", "related_node", "gr-rel11", json!({}))
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
                .await;

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

            create_schema(&svc, "gr_item9", json!([])).await;
            create_schema(
                &svc,
                "gr_parent9",
                json!([{
                    "name": "items",
                    "targetType": "gr_item9",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "collection",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let item1 = make_node("gr-i9a", "gr_item9", json!({"status": "done"}));
            let item2 = make_node("gr-i9b", "gr_item9", json!({"status": "open"}));
            svc.create_node(item1).await.unwrap();
            svc.create_node(item2).await.unwrap();

            let parent = make_node("gr-p9", "gr_parent9", json!({}));
            svc.create_node(parent.clone()).await.unwrap();

            svc.create_relationship("gr-p9", "items", "gr-i9a", json!({}))
                .await
                .unwrap();
            svc.create_relationship("gr-p9", "items", "gr-i9b", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));

            use crate::playbook::path_extractor::ExtractedPath;

            // resolve_collection expects segments like ["node", "items"]
            let collection_path = ExtractedPath {
                segments: vec!["node".to_string(), "items".to_string()],
                root: "node".to_string(),
            };

            let nodes = resolver.resolve_collection(&parent, &collection_path).await;
            assert_eq!(nodes.len(), 2, "should resolve 2 collection nodes");
            let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
            assert!(ids.contains(&"gr-i9a"));
            assert!(ids.contains(&"gr-i9b"));
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn enrich_context_with_collection() {
            let (svc, _tmp) = create_test_service().await;

            create_schema(&svc, "gr_sub10", json!([])).await;
            create_schema(
                &svc,
                "gr_parent10",
                json!([{
                    "name": "tasks",
                    "targetType": "gr_sub10",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "parent_task",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let sub1 = make_node("gr-s10a", "gr_sub10", json!({"status": "done"}));
            let sub2 = make_node("gr-s10b", "gr_sub10", json!({"status": "open"}));
            svc.create_node(sub1).await.unwrap();
            svc.create_node(sub2).await.unwrap();

            let parent = make_node("gr-p10", "gr_parent10", json!({}));
            svc.create_node(parent.clone()).await.unwrap();

            svc.create_relationship("gr-p10", "tasks", "gr-s10a", json!({}))
                .await
                .unwrap();
            svc.create_relationship("gr-p10", "tasks", "gr-s10b", json!({}))
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

            let result = resolver.enrich_context(&parent, &[], &collections).await;

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
            create_schema(&svc, "gr_rev_task", json!([])).await;

            let parent = make_node("gr-rev-p1", "gr_rev_task", json!({"status": "open"}));
            svc.create_node(parent.clone()).await.unwrap();
            let child = make_node("gr-rev-c1", "gr_rev_task", json!({"status": "done"}));
            svc.create_node(child.clone()).await.unwrap();

            svc.create_relationship("gr-rev-p1", "has_child", "gr-rev-c1", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&child, &["child_of".to_string()])
                .await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-rev-p1"),
                other => panic!("expected the parent Node, got {:?}", other),
            }

            // The forward direction must still walk the other way, from the
            // same edge: reverse support is additive, not a redirect.
            let forward = resolver
                .resolve_path(&parent, &["has_child".to_string()])
                .await;
            match forward {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-rev-c1"),
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
            create_schema(&svc, "gr_core_task", json!([])).await;

            let parent = make_node("gr-core-p1", "gr_core_task", json!({"status": "open"}));
            svc.create_node(parent.clone()).await.unwrap();
            let child = make_node("gr-core-c1", "gr_core_task", json!({"status": "done"}));
            svc.create_node(child.clone()).await.unwrap();
            svc.create_relationship("gr-core-p1", "has_child", "gr-core-c1", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            for (segment, want) in [("id", "gr-core-p1"), ("node_type", "gr_core_task")] {
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

            // A core field on the root node itself resolves the same way, with
            // no traversal involved.
            match resolver.resolve_path(&child, &["id".to_string()]).await {
                ResolvedValue::Scalar(v) => assert_eq!(v.as_str(), Some("gr-core-c1")),
                other => panic!("expected the node's own id, got {other:?}"),
            }
        }

        /// A schema-declared `reverseName` resolves the same way, and chains.
        ///
        /// `gr_rev_person` declares `tasks`; a task reaching its owner spells
        /// that `assignee`. The second hop (`.email`) proves a reverse segment
        /// leaves the walk in the same state a forward one does — the resolved
        /// node keeps being walkable, so multi-hop paths work through it.
        #[tokio::test(flavor = "multi_thread")]
        async fn declared_reverse_name_resolves_and_chains() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "gr_rev_ticket", json!([])).await;
            create_schema(
                &svc,
                "gr_rev_person",
                json!([{
                    "name": "tasks",
                    "targetType": "gr_rev_ticket",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "assignee",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let person = make_node(
                "gr-rev-u1",
                "gr_rev_person",
                json!({"email": "ada@example.com"}),
            );
            svc.create_node(person.clone()).await.unwrap();
            let ticket = make_node("gr-rev-k1", "gr_rev_ticket", json!({"status": "open"}));
            svc.create_node(ticket.clone()).await.unwrap();

            svc.create_relationship("gr-rev-u1", "tasks", "gr-rev-k1", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let result = resolver
                .resolve_path(&ticket, &["assignee".to_string()])
                .await;
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-rev-u1"),
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
            create_schema(&svc, "gr_nar_ticket", json!([])).await;
            create_schema(
                &svc,
                "gr_nar_person",
                json!([{
                    "name": "tasks",
                    "targetType": "gr_nar_ticket",
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
                "gr_nar_project",
                json!([{
                    "name": "tasks",
                    "targetType": "gr_nar_ticket",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "project",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let person = make_node("gr-nar-u1", "gr_nar_person", json!({"email": "grace@x.io"}));
            svc.create_node(person.clone()).await.unwrap();
            let project = make_node("gr-nar-pr1", "gr_nar_project", json!({"name": "Apollo"}));
            svc.create_node(project.clone()).await.unwrap();
            let ticket = make_node("gr-nar-k1", "gr_nar_ticket", json!({"status": "open"}));
            svc.create_node(ticket.clone()).await.unwrap();

            // The same ticket is linked from both ends, under the same name.
            svc.create_relationship("gr-nar-u1", "tasks", "gr-nar-k1", json!({}))
                .await
                .unwrap();
            svc.create_relationship("gr-nar-pr1", "tasks", "gr-nar-k1", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            match resolver
                .resolve_path(&ticket, &["assignee".to_string()])
                .await
            {
                ResolvedValue::Node(n) => assert_eq!(
                    n.id, "gr-nar-u1",
                    "assignee must be the person, not the project"
                ),
                other => panic!("expected exactly the person Node, got {:?}", other),
            }

            // And the other declarer's reverse name resolves to its own node.
            match resolver
                .resolve_path(&ticket, &["project".to_string()])
                .await
            {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-nar-pr1"),
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
            create_schema(&svc, "gr_sib_task", json!([])).await;

            let parent = make_node("gr-sib-p1", "gr_sib_task", json!({"status": "open"}));
            svc.create_node(parent.clone()).await.unwrap();
            for id in ["gr-sib-c1", "gr-sib-c2"] {
                let child = make_node(id, "gr_sib_task", json!({"status": "done"}));
                svc.create_node(child.clone()).await.unwrap();
                svc.create_relationship("gr-sib-p1", "has_child", id, json!({}))
                    .await
                    .unwrap();
            }
            let child = svc.get_node("gr-sib-c1").await.unwrap().unwrap();

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
            create_schema(&svc, "gr_inf_doc", json!([])).await;
            create_schema(
                &svc,
                "gr_inf_author",
                json!([{
                    "name": "wrote",
                    "targetType": "gr_inf_doc",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "written_by",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let author = make_node("gr-inf-a1", "gr_inf_author", json!({"name": "Kay"}));
            svc.create_node(author.clone()).await.unwrap();
            let doc = make_node("gr-inf-d1", "gr_inf_doc", json!({"status": "draft"}));
            svc.create_node(doc.clone()).await.unwrap();
            svc.create_relationship("gr-inf-a1", "wrote", "gr-inf-d1", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            // The doc spells the edge by the author's forward name.
            match resolver.resolve_path(&doc, &["wrote".to_string()]).await {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-inf-a1"),
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
                    assert_eq!(nodes[0].id, "gr-inf-d1");
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
            create_schema(&svc, "gr_term_ticket", json!([])).await;
            create_schema(
                &svc,
                "gr_term_person",
                json!([{
                    "name": "tasks",
                    "targetType": "gr_term_ticket",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "assignee",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            svc.create_node(make_node(
                "gr-term-u1",
                "gr_term_person",
                json!({"email": "ada@example.com"}),
            ))
            .await
            .unwrap();
            let ticket = make_node("gr-term-k1", "gr_term_ticket", json!({"status": "open"}));
            svc.create_node(ticket.clone()).await.unwrap();
            svc.create_relationship("gr-term-u1", "tasks", "gr-term-k1", json!({}))
                .await
                .unwrap();

            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            let paths = vec![ExtractedPath {
                segments: vec!["node".to_string(), "assignee".to_string()],
                root: "node".to_string(),
            }];

            let result = resolver.enrich_context(&ticket, &paths, &[]).await;

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
            create_schema(&svc, "gr_cache_task", json!([])).await;

            // Two independent parent/child pairs.
            for (parent, child) in [("gr-cc-p1", "gr-cc-c1"), ("gr-cc-p2", "gr-cc-c2")] {
                svc.create_node(make_node(
                    parent,
                    "gr_cache_task",
                    json!({"status": "open"}),
                ))
                .await
                .unwrap();
                svc.create_node(make_node(child, "gr_cache_task", json!({"status": "done"})))
                    .await
                    .unwrap();
                svc.create_relationship(parent, "has_child", child, json!({}))
                    .await
                    .unwrap();
            }

            let child1 = svc.get_node("gr-cc-c1").await.unwrap().unwrap();
            let child2 = svc.get_node("gr-cc-c2").await.unwrap().unwrap();

            // One resolver, the same path, two different roots.
            let mut resolver = GraphResolver::new(Arc::clone(&svc));
            match resolver
                .resolve_path(&child1, &["child_of".to_string()])
                .await
            {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-cc-p1"),
                other => panic!("expected p1, got {:?}", other),
            }
            match resolver
                .resolve_path(&child2, &["child_of".to_string()])
                .await
            {
                ResolvedValue::Node(n) => assert_eq!(
                    n.id, "gr-cc-p2",
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
            create_schema(&svc, "gr_unk_task", json!([])).await;

            let node = make_node("gr-unk-1", "gr_unk_task", json!({"status": "open"}));
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
        /// `is_declared_many_relationship`. Before the fix that check read
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
                    "name": "gr_ext_item",
                    "fields": []
                }),
            )
            .await
            .expect("target schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "gr_ext_base",
                    "fields": [],
                    "relationships": [{
                        "name": "items",
                        "targetType": "gr_ext_item",
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
                    "name": "gr_ext_sub",
                    "extends": "gr_ext_base",
                    "fields": []
                }),
            )
            .await
            .expect("subtype schema creation failed");

            // A subtype instance with NO items ever attached -- the relationship
            // is only declared on the ancestor, never redeclared here.
            let parent = make_node("gr-ext-p1", "gr_ext_sub", json!({}));
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
    }
}
