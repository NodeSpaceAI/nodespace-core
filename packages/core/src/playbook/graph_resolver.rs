//! Graph Resolver for the Playbook Engine
//!
//! Resolves dot-paths by walking the data graph via NodeService.
//! Created per rule evaluation, with a segment cache to prevent
//! redundant DB queries for overlapping paths.
//!
//! Uses `tokio::task::block_in_place` for the sync bridge since
//! cel-interpreter's `Program::execute()` is synchronous while
//! NodeService is async.

use crate::models::Node;
use crate::playbook::cel::{json_to_cel, key, node_to_cel_value};
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
    /// Cache: path segments → resolved value
    cache: HashMap<Vec<String>, ResolvedValue>,
}

impl GraphResolver {
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self {
            node_service,
            cache: HashMap::new(),
        }
    }

    /// Resolve a dot-path starting from a root node.
    ///
    /// Walks segments left-to-right:
    /// 1. Check if the segment is a property on the current node → Scalar
    /// 2. If not, try as a relationship name → fetch related node(s)
    /// 3. For "one" relationships, continue walking with the target node
    /// 4. For "many" relationships, return Collection
    ///
    /// Uses the segment cache: if a prefix has already been resolved, starts from there.
    pub fn resolve_path(&mut self, root_node: &Node, segments: &[String]) -> ResolvedValue {
        if segments.is_empty() {
            return ResolvedValue::Node(root_node.clone());
        }

        // Check cache for the full path first
        if let Some(cached) = self.cache.get(segments) {
            return cached.clone();
        }

        // Find the longest cached prefix
        let mut start_idx = 0;
        let mut current_node = root_node.clone();

        for i in (1..segments.len()).rev() {
            let prefix = &segments[..i];
            if let Some(cached) = self.cache.get(prefix) {
                match cached {
                    ResolvedValue::Node(n) => {
                        current_node = n.clone();
                        start_idx = i;
                        break;
                    }
                    ResolvedValue::Collection(_) | ResolvedValue::Scalar(_) => {
                        // Can't continue walking from a collection or scalar
                        let result = ResolvedValue::Missing;
                        self.cache.insert(segments.to_vec(), result.clone());
                        return result;
                    }
                    ResolvedValue::Missing => {
                        let result = ResolvedValue::Missing;
                        self.cache.insert(segments.to_vec(), result.clone());
                        return result;
                    }
                }
            }
        }

        // Walk remaining segments
        for i in start_idx..segments.len() {
            let segment = &segments[i];
            let is_last = i == segments.len() - 1;

            // Try as a property first (check node.properties)
            if let Some(prop_val) = get_node_property(&current_node, segment) {
                let result = ResolvedValue::Scalar(prop_val);
                self.cache.insert(segments[..=i].to_vec(), result.clone());
                if is_last {
                    self.cache.insert(segments.to_vec(), result.clone());
                    return result;
                }
                // Can't walk further into a scalar
                let missing = ResolvedValue::Missing;
                self.cache.insert(segments.to_vec(), missing.clone());
                return missing;
            }

            // Try as a relationship
            let related = self.fetch_related_nodes(&current_node.id, segment);
            match related {
                Ok(nodes) if nodes.is_empty() => {
                    let result = ResolvedValue::Missing;
                    self.cache.insert(segments[..=i].to_vec(), result.clone());
                    self.cache.insert(segments.to_vec(), result.clone());
                    return result;
                }
                Ok(nodes) if nodes.len() == 1 => {
                    let node = nodes.into_iter().next().unwrap();
                    self.cache
                        .insert(segments[..=i].to_vec(), ResolvedValue::Node(node.clone()));
                    if is_last {
                        let result = ResolvedValue::Node(node);
                        self.cache.insert(segments.to_vec(), result.clone());
                        return result;
                    }
                    current_node = node;
                }
                Ok(nodes) => {
                    // Multiple related nodes — this is a collection
                    let result = ResolvedValue::Collection(nodes);
                    self.cache.insert(segments[..=i].to_vec(), result.clone());
                    if is_last {
                        self.cache.insert(segments.to_vec(), result.clone());
                        return result;
                    }
                    // Can't walk further into a collection with simple dot-path
                    let missing = ResolvedValue::Missing;
                    self.cache.insert(segments.to_vec(), missing.clone());
                    return missing;
                }
                Err(e) => {
                    warn!(
                        "Failed to fetch related nodes for {}.{}: {}",
                        current_node.id, segment, e
                    );
                    let result = ResolvedValue::Missing;
                    self.cache.insert(segments.to_vec(), result.clone());
                    return result;
                }
            }
        }

        ResolvedValue::Node(current_node)
    }

    /// Resolve a collection path and return the collection nodes.
    pub fn resolve_collection(
        &mut self,
        root_node: &Node,
        collection: &ExtractedPath,
    ) -> Vec<Node> {
        // The collection path is like ["node", "tasks"] — skip "node" (the root)
        let segments = &collection.segments;
        if segments.len() < 2 {
            return vec![];
        }

        match self.resolve_path(root_node, &segments[1..]) {
            ResolvedValue::Collection(nodes) => nodes,
            ResolvedValue::Node(n) => vec![n],
            _ => vec![],
        }
    }

    /// Fetch related nodes using the sync bridge.
    ///
    /// Uses `tokio::task::block_in_place` + `Handle::current().block_on()`
    /// which is safe because:
    /// - We're on a multi-threaded tokio runtime
    /// - Only the single RuleProcessor task calls this
    fn fetch_related_nodes(
        &self,
        node_id: &str,
        relationship_name: &str,
    ) -> Result<Vec<Node>, String> {
        let handle = tokio::runtime::Handle::current();
        let node_service = Arc::clone(&self.node_service);
        let node_id = node_id.to_string();
        let rel_name = relationship_name.to_string();

        tokio::task::block_in_place(|| {
            handle.block_on(async {
                node_service
                    .get_related_nodes(&node_id, &rel_name, "out")
                    .await
                    .map_err(|e| e.to_string())
            })
        })
    }

    /// Build an enriched CEL context with graph-resolved paths.
    ///
    /// Takes the base node and extracted paths, resolves each path against
    /// the graph, and injects the resolved values as nested CEL Maps.
    pub fn enrich_context(
        &mut self,
        root_node: &Node,
        paths: &[ExtractedPath],
        collections: &[CollectionPath],
    ) -> HashMap<Vec<String>, Value> {
        let mut resolved_values: HashMap<Vec<String>, Value> = HashMap::new();

        // Resolve flat paths (skip "node" root — those beyond property-level)
        for path in paths {
            if path.root != "node" || path.segments.len() <= 2 {
                // Single-level paths (node.status) are handled by existing context building
                continue;
            }

            // Resolve the relationship chain (skip "node" prefix)
            let segments = &path.segments[1..];
            match self.resolve_path(root_node, segments) {
                ResolvedValue::Node(n) => {
                    resolved_values.insert(path.segments.clone(), node_to_cel_value(&n));
                }
                ResolvedValue::Scalar(v) => {
                    resolved_values.insert(path.segments.clone(), json_to_cel(&v));
                }
                ResolvedValue::Collection(nodes) => {
                    let list: Vec<Value> = nodes.iter().map(node_to_cel_value).collect();
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
            let nodes = self.resolve_collection(root_node, &coll.collection);
            if !nodes.is_empty() {
                let list: Vec<Value> = nodes.iter().map(node_to_cel_value).collect();
                resolved_values.insert(coll.collection.segments.clone(), Value::List(list.into()));
            }
        }

        resolved_values
    }
}

/// Get a property value from a node, checking both namespaced and bare keys.
fn get_node_property(node: &Node, key: &str) -> Option<serde_json::Value> {
    if let Some(obj) = node.properties.as_object() {
        // Direct match
        if let Some(val) = obj.get(key) {
            return Some(val.clone());
        }
        // Try with namespace prefix (e.g., "status" → "custom:status")
        for (k, v) in obj {
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
