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

/// Get a property value from a node, checking multiple formats.
///
/// NodeSpace stores properties in a type-namespaced format:
/// `{"task": {"status": "open"}}` — so we check inside the type namespace too.
/// Also checks `custom:key` namespace prefix format.
fn get_node_property(node: &Node, key: &str) -> Option<serde_json::Value> {
    if let Some(obj) = node.properties.as_object() {
        // Direct match (e.g., key "status" on {"status": "open"})
        if let Some(val) = obj.get(key) {
            // Don't return the whole type namespace object as a "property"
            if !val.is_object() || obj.len() > 1 {
                return Some(val.clone());
            }
        }

        // Check inside the type-namespaced object (e.g., {"task": {"status": "open"}})
        // The type namespace key matches the node_type
        if let Some(type_obj) = obj.get(&node.node_type).and_then(|v| v.as_object()) {
            if let Some(val) = type_obj.get(key) {
                return Some(val.clone());
            }
        }

        // Try with colon namespace prefix (e.g., "status" → "custom:status")
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
        Value::Map(cel_interpreter::objects::Map {
            map: Arc::new(map),
        })
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
            vec![
                "node".to_string(),
                "story".to_string(),
                "title".to_string(),
            ],
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
        resolved.insert(
            vec!["node".to_string(), "tasks".to_string()],
            list.clone(),
        );
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
        assert_eq!(
            get_node_property(&node, "status"),
            Some(json!("open"))
        );
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
        use crate::db::SurrealStore;
        use crate::models::Node;
        use crate::services::NodeService;
        use serde_json::json;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SurrealStore> =
                Arc::new(SurrealStore::new(db_path).await.unwrap());
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
                    "fields": [{"name": "status", "type": "string"}, {"name": "title", "type": "string"}],
                    "relationships": relationships
                }),
            );
            svc.create_node(schema_node)
                .await
                .unwrap_or_else(|_| panic!("Failed to create schema '{}'", type_name));
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
            let result = resolver.resolve_path(&node, &["status".to_string()]);
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
            let result = resolver.resolve_path(&node, &["nonexistent".to_string()]);
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
                    "target_type": "gr_story",
                    "direction": "out",
                    "cardinality": "one"
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
            let result = resolver.resolve_path(&issue, &["story".to_string()]);
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-s1"),
                other => panic!("expected Node, got {:?}", other),
            }
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
                    "target_type": "gr_epic",
                    "direction": "out",
                    "cardinality": "one"
                }]),
            )
            .await;
            create_schema(
                &svc,
                "gr_task3",
                json!([{
                    "name": "story",
                    "target_type": "gr_story3",
                    "direction": "out",
                    "cardinality": "one"
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
            let result = resolver.resolve_path(
                &task,
                &["story".to_string(), "epic".to_string()],
            );
            match result {
                ResolvedValue::Node(n) => assert_eq!(n.id, "gr-e1"),
                other => panic!("expected Node for story.epic, got {:?}", other),
            }

            // Resolve task -> story -> epic -> status (scalar property on the target)
            let result = resolver.resolve_path(
                &task,
                &[
                    "story".to_string(),
                    "epic".to_string(),
                    "status".to_string(),
                ],
            );
            match result {
                ResolvedValue::Scalar(v) => assert_eq!(v, json!("in_progress")),
                other => panic!("expected Scalar for story.epic.status, got {:?}", other),
            }
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
                    "target_type": "gr_story4",
                    "direction": "out",
                    "cardinality": "one"
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
            let r1 = resolver.resolve_path(
                &task,
                &["story".to_string(), "status".to_string()],
            );
            assert!(matches!(r1, ResolvedValue::Scalar(_)));

            // Second call should hit cache (same result)
            let r2 = resolver.resolve_path(
                &task,
                &["story".to_string(), "status".to_string()],
            );
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
                    "target_type": "gr_subtask",
                    "direction": "out",
                    "cardinality": "many"
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
            let result = resolver.resolve_path(&parent, &["subtasks".to_string()]);
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
                    "target_type": "gr_target5",
                    "direction": "out",
                    "cardinality": "one"
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

            let result = resolver.enrich_context(&source, &paths, &[]);
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
            let result = resolver.resolve_path(&node, &[]);
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
            let result = resolver.resolve_path(
                &node,
                &["status".to_string(), "deeper".to_string()],
            );
            assert!(matches!(result, ResolvedValue::Missing));
        }
    }
}
