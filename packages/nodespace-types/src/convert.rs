use chrono::{DateTime, NaiveDate, Utc};
use std::str::FromStr;

use crate::ai_chat::{AiChatMessage, AiChatNode};
use crate::node::Node;
use crate::person::PersonNode;
use crate::project::{ProjectNode, DEFAULT_PROJECT_STATUS};
use crate::schema::SchemaNode;
use crate::task::{TaskNode, TaskPriority, TaskStatus};

fn normalize_date_field(s: &str) -> String {
    if NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return s.to_string();
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt.format("%Y-%m-%d").to_string();
    }
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return dt.format("%Y-%m-%d").to_string();
    }
    s.to_string()
}

/// Convert a `Node` to its strongly-typed JSON representation for the frontend.
///
/// For typed nodes (`task`, `person`, `project`, `ai-chat`, `schema`), promotes type-specific
/// properties to top-level fields. For all other types, returns the generic
/// node shape. Adds a `nodespace://` URI field for rich client rendering.
///
/// This is the single canonical implementation used by all entry points
/// (Tauri commands, MCP, HTTP) and the SOLE authority for property flattening
/// and `nodespace://` URI production. Do NOT re-implement either in another layer
/// (e.g. a TypeScript-side flatten in the frontend converters) — the frontend
/// `nodeTo*` converters trust the flat, top-level shape this function guarantees.
/// The `wire_contract` tests below pin that shape.
pub fn node_to_typed_value(node: Node) -> Result<serde_json::Value, String> {
    let mut node = node;
    flatten_properties_for_api(&mut node);

    let node_id = node.id.clone();
    let mut value = match node.node_type.as_str() {
        "task" => task_node_to_value(node),
        "ai-chat" => ai_chat_node_to_value(node),
        "person" => person_node_to_value(node),
        "project" => project_node_to_value(node),
        "schema" => SchemaNode::from_node(node).and_then(|s| {
            serde_json::to_value(s).map_err(|e| format!("Failed to serialize schema: {}", e))
        }),
        _ => serde_json::to_value(node).map_err(|e| format!("Failed to serialize node: {}", e)),
    }?;

    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "uri".to_string(),
            serde_json::Value::String(format!("nodespace://{}", node_id)),
        );
    }

    Ok(value)
}

/// Convert a vec of nodes to their strongly-typed JSON representations.
pub fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<serde_json::Value>, String> {
    nodes.into_iter().map(node_to_typed_value).collect()
}

/// Flatten namespaced properties into the flat API shape.
///
/// Storage format: `{ "task": { "status": "open" } }`
/// API format:     `{ "status": "open" }`
///
/// This is the single definition of the rule. Callers that hold a `Node` should
/// go through [`node_to_typed_value`]; this function exists for the callers that
/// do not — notably the CLI, which only ever has the gRPC `NodeData` (whose
/// `properties` is a JSON-encoded string) and so cannot build a `Node` to pass.
/// Both share this body so the two surfaces cannot drift apart.
///
/// `_`-prefixed keys (`_schema_version`, and sibling namespaces like `_seed`)
/// are internal and never exposed. Dormant namespaces left by a previous type
/// change are not exposed either.
///
/// Note the asymmetry between the branches: inside the type's own namespace an
/// object is a real schema-defined field value and is preserved, whereas in the
/// already-flat fallback a nested object can only be another type's namespace
/// and is dropped.
///
/// This governs human-readable CLI output as well as JSON: `write_human_node`
/// and `node_to_json` share one call into it, so the two cannot disagree.
pub fn flatten_namespaced_properties(
    properties: &serde_json::Value,
    node_type: &str,
) -> serde_json::Value {
    flatten_namespaced_properties_at_scope(properties, std::slice::from_ref(&node_type))
}

/// Flatten namespaced properties at an explicit scope chain (ADR-078).
///
/// The general form of [`flatten_namespaced_properties`], which is now the
/// single-scope case. `scope_chain` is nearest-scope-first: reading an `issue`
/// node at its own scope passes `["issue", "task"]` and yields both its own
/// and its inherited fields, while reading the same node at `task` scope
/// passes `["task"]` and yields task's fields only — the issue's own fields
/// **absent**, not merely unresolved. That truncation is what makes a
/// base-scoped query return a homogeneous result set.
///
/// A nearer scope wins any key collision. Well-formed chains have none
/// (redeclaration is rejected at write time); this only decides the
/// retroactive-collision case ADR-078 leaves open.
///
/// Stays pure and free of any schema lookup: the caller resolves the chain,
/// because every caller is in code that can reach the store and this crate
/// cannot. Passing a single scope preserves the pre-`extends` behavior exactly.
pub fn flatten_namespaced_properties_at_scope(
    properties: &serde_json::Value,
    scope_chain: &[&str],
) -> serde_json::Value {
    let Some(props_obj) = properties.as_object() else {
        return properties.clone();
    };

    // Any bucket in the chain present? If so, the node is in storage shape and
    // the chain decides what is visible.
    let has_any_bucket = scope_chain
        .iter()
        .any(|scope| props_obj.get(*scope).and_then(|v| v.as_object()).is_some());

    if has_any_bucket {
        let mut out = serde_json::Map::new();
        for scope in scope_chain {
            let Some(bucket) = props_obj.get(*scope).and_then(|v| v.as_object()) else {
                continue;
            };
            for (k, v) in bucket {
                if k.starts_with('_') {
                    continue;
                }
                out.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        return serde_json::Value::Object(out);
    }

    // Already-flat fallback, unchanged: a nested object here can only be
    // another type's namespace, so it is dropped.
    serde_json::Value::Object(
        props_obj
            .iter()
            .filter(|(k, v)| !v.is_object() && !k.starts_with('_'))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

/// Flatten namespaced properties for API response, in place.
///
/// Single-scope by design: this crate has no store access and cannot resolve
/// an `extends` chain. The service layer collapses a node's inherited buckets
/// into its own before the wire boundary
/// (`NodeService::collapse_chain_for_wire`), so one bucket carries the whole
/// effective property set by the time it reaches here — and a dormant bucket
/// left by an earlier type change stays excluded, as it always has been.
fn flatten_properties_for_api(node: &mut Node) {
    node.properties = flatten_namespaced_properties(&node.properties, &node.node_type);
}

/// The core fields a typed conversion promotes out of `properties`, as
/// `(storage key, wire key)` pairs: `due_date` is stored in the `task` bucket
/// and travels as the top-level `dueDate`.
///
/// Promotion is a move, not a copy — [`node_to_typed_value`] removes these keys
/// from `properties`, so each core field has exactly one home on the wire and
/// `properties` carries only extension fields. Consumers that want the flat,
/// storage-keyed view back (the CLI's snake_case node shape, the agent's
/// model-facing property map) rebuild it with [`flat_properties_view`] rather
/// than hard-coding these lists.
pub fn promoted_fields(node_type: &str) -> &'static [(&'static str, &'static str)] {
    match node_type {
        "task" => &[
            ("status", "status"),
            ("priority", "priority"),
            ("due_date", "dueDate"),
            ("started_at", "startedAt"),
            ("completed_at", "completedAt"),
        ],
        "person" => &[
            ("first_name", "firstName"),
            ("last_name", "lastName"),
            ("email", "email"),
        ],
        "project" => &[
            ("status", "status"),
            ("priority", "priority"),
            ("start_date", "startDate"),
            ("end_date", "endDate"),
        ],
        _ => &[],
    }
}

/// Remove a type's promoted core fields from its flat `properties`, under both
/// spellings a stored node can carry (`due_date` and the legacy `dueDate`).
fn without_promoted(mut properties: serde_json::Value, node_type: &str) -> serde_json::Value {
    if let Some(obj) = properties.as_object_mut() {
        for (storage_key, wire_key) in promoted_fields(node_type) {
            obj.remove(*storage_key);
            obj.remove(*wire_key);
        }
    }
    properties
}

/// Rebuild a typed node value's flat, storage-keyed property map: its
/// `properties` plus every promoted core field folded back under its storage
/// key (`dueDate` → `due_date`). Null and absent core fields are left out.
///
/// For consumers whose contract is the flat property map — the CLI's node
/// shape and the agent's model-facing summaries, both of which also *write*
/// with these bare keys. The frontend reads the typed fields directly.
pub fn flat_properties_view(typed: &serde_json::Value) -> serde_json::Value {
    let mut props = typed
        .get("properties")
        .and_then(|p| p.as_object())
        .cloned()
        .unwrap_or_default();
    if let Some(node_type) = typed.get("nodeType").and_then(|v| v.as_str()) {
        for (storage_key, wire_key) in promoted_fields(node_type) {
            if let Some(value) = typed.get(*wire_key).filter(|v| !v.is_null()) {
                props
                    .entry(storage_key.to_string())
                    .or_insert_with(|| value.clone());
            }
        }
    }
    serde_json::Value::Object(props)
}

fn task_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let props = &node.properties;

    let status: TaskStatus = props
        .get("status")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .unwrap_or_default();

    let priority = props
        .get("priority")
        .and_then(|v| v.as_str())
        .map(|s| TaskPriority::from_str(s).unwrap_or_default());

    let due_date = props
        .get("dueDate")
        .or_else(|| props.get("due_date"))
        .and_then(|v| v.as_str())
        .map(normalize_date_field);

    let started_at = props
        .get("startedAt")
        .or_else(|| props.get("started_at"))
        .and_then(|v| v.as_str())
        .map(normalize_date_field);

    let completed_at = props
        .get("completedAt")
        .or_else(|| props.get("completed_at"))
        .and_then(|v| v.as_str())
        .map(normalize_date_field);

    let lifecycle_status = node.lifecycle_status.clone();
    let title = node.title.clone();
    let task = TaskNode {
        id: node.id,
        node_type: node.node_type,
        content: node.content,
        title,
        version: node.version,
        created_at: node.created_at,
        modified_at: node.modified_at,
        properties: without_promoted(node.properties, "task"),
        lifecycle_status,
        status,
        priority,
        due_date,
        started_at,
        completed_at,
    };

    serde_json::to_value(&task).map_err(|e| format!("Failed to serialize task node: {}", e))
}

fn string_prop(props: &serde_json::Value, key: &str) -> Option<String> {
    props.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn person_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let props = &node.properties;
    let first_name = string_prop(props, "first_name");
    let last_name = string_prop(props, "last_name");
    let email = string_prop(props, "email");

    let person = PersonNode {
        id: node.id,
        node_type: node.node_type,
        content: node.content,
        title: node.title,
        version: node.version,
        created_at: node.created_at,
        modified_at: node.modified_at,
        properties: without_promoted(node.properties, "person"),
        lifecycle_status: node.lifecycle_status,
        first_name,
        last_name,
        email,
    };

    serde_json::to_value(&person).map_err(|e| format!("Failed to serialize person node: {}", e))
}

fn project_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let props = &node.properties;
    let status = string_prop(props, "status").unwrap_or_else(|| DEFAULT_PROJECT_STATUS.to_string());
    let priority = string_prop(props, "priority");
    let start_date = props
        .get("start_date")
        .and_then(|v| v.as_str())
        .map(normalize_date_field);
    let end_date = props
        .get("end_date")
        .and_then(|v| v.as_str())
        .map(normalize_date_field);

    let project = ProjectNode {
        id: node.id,
        node_type: node.node_type,
        content: node.content,
        title: node.title,
        version: node.version,
        created_at: node.created_at,
        modified_at: node.modified_at,
        properties: without_promoted(node.properties, "project"),
        lifecycle_status: node.lifecycle_status,
        status,
        priority,
        start_date,
        end_date,
    };

    serde_json::to_value(&project).map_err(|e| format!("Failed to serialize project node: {}", e))
}

fn ai_chat_node_to_value(node: Node) -> Result<serde_json::Value, String> {
    let props = &node.properties;

    // Canonical snake_case only — see `nodespace_core::models::AiChatNode::
    // from_node`'s doc comment for why a camelCase fallback here would be
    // actively wrong (a stale camelCase key permanently shadowing fresh
    // canonical writes), not just redundant.
    let turn_status = props
        .get("turn_status")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let session_status = props
        .get("session_status")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let provider = props
        .get("provider")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let model = props
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Decoded per message, not as one `Vec`: decoding the whole array at once
    // means a single unreadable message blanks the entire conversation in the
    // UI. Matches `AiChatNode::from_node`, which contains the same failure the
    // same way — the invariant has to hold on both paths or the stricter
    // message type just relocates the problem.
    let messages = props
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| serde_json::from_value::<AiChatMessage>(m.clone()).ok())
                .collect()
        })
        .unwrap_or_default();

    let lifecycle_status = node.lifecycle_status.clone();
    let chat = AiChatNode {
        id: node.id,
        node_type: node.node_type,
        content: node.content,
        version: node.version,
        created_at: node.created_at,
        modified_at: node.modified_at,
        properties: node.properties,
        lifecycle_status,
        turn_status,
        session_status,
        provider,
        model,
        messages,
    };

    serde_json::to_value(&chat).map_err(|e| format!("Failed to serialize ai-chat node: {}", e))
}

#[cfg(test)]
mod wire_contract {
    use super::*;
    use crate::node::Node;

    // These tests pin the wire contract that the frontend TS converters trust:
    // for typed nodes, type-specific fields are promoted to TOP-LEVEL and the
    // namespaced `properties.<type>` form is flattened away. If this contract
    // ever changes, the frontend nodeTo* converters must change in lockstep.

    #[test]
    fn task_promotes_fields_top_level_and_flattens_properties() {
        let node = Node::new(
            "task".to_string(),
            "Buy milk".to_string(),
            serde_json::json!({
                "task": { "status": "in_progress", "priority": "high", "custom:store": "Costco" }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        // Core fields promoted to top level.
        assert_eq!(out["status"], "in_progress");
        assert_eq!(out["priority"], "high");
        // `properties` is flattened: no `task` namespace survives.
        assert!(out["properties"].get("task").is_none());
        // User/custom fields remain in flat `properties`.
        assert_eq!(out["properties"]["custom:store"], "Costco");
        // Promotion is a move: core fields have one home, the top level.
        assert_eq!(
            out["properties"],
            serde_json::json!({ "custom:store": "Costco" })
        );
        // URI is injected by the backend.
        assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
    }

    #[test]
    fn task_removes_both_date_spellings_from_properties() {
        let node = Node::new(
            "task".to_string(),
            "Buy milk".to_string(),
            serde_json::json!({
                "task": { "due_date": "2026-05-01", "startedAt": "2026-04-01" }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["dueDate"], "2026-05-01");
        assert_eq!(out["startedAt"], "2026-04-01");
        assert_eq!(out["properties"], serde_json::json!({}));
    }

    #[test]
    fn flat_properties_view_folds_promoted_fields_back_under_storage_keys() {
        let node = Node::new(
            "task".to_string(),
            "Buy milk".to_string(),
            serde_json::json!({
                "task": { "status": "done", "due_date": "2026-05-01", "custom:store": "Costco" }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(
            flat_properties_view(&out),
            serde_json::json!({
                "status": "done",
                "due_date": "2026-05-01",
                "custom:store": "Costco"
            })
        );
    }

    #[test]
    fn flat_properties_view_leaves_untyped_nodes_alone() {
        let node = Node::new(
            "invoice".to_string(),
            "INV-1".to_string(),
            serde_json::json!({ "invoice": { "amount": 5 } }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(
            flat_properties_view(&out),
            serde_json::json!({ "amount": 5 })
        );
    }

    #[test]
    fn person_promotes_fields_top_level_and_flattens_properties() {
        let mut node = Node::new(
            "person".to_string(),
            String::new(),
            serde_json::json!({
                "person": {
                    "first_name": "Ada",
                    "last_name": "Lovelace",
                    "email": "ada@example.com",
                    "custom:team": "Engines"
                }
            }),
        );
        node.title = Some("Ada Lovelace".to_string());
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "person");
        assert_eq!(out["firstName"], "Ada");
        assert_eq!(out["lastName"], "Lovelace");
        assert_eq!(out["email"], "ada@example.com");
        assert_eq!(out["title"], "Ada Lovelace");
        assert!(out["properties"].get("person").is_none());
        assert_eq!(out["properties"]["custom:team"], "Engines");
        assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
    }

    #[test]
    fn person_with_unset_or_null_fields_omits_them() {
        let node = Node::new(
            "person".to_string(),
            String::new(),
            serde_json::json!({ "person": { "first_name": "Ada", "email": null } }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["firstName"], "Ada");
        assert!(out.get("lastName").is_none());
        assert!(out.get("email").is_none());
    }

    #[test]
    fn project_promotes_fields_top_level_and_normalizes_dates() {
        let node = Node::new(
            "project".to_string(),
            "Launch".to_string(),
            serde_json::json!({
                "project": {
                    "status": "active",
                    "priority": "high",
                    "start_date": "2026-03-01T09:00:00Z",
                    "end_date": "2026-04-30",
                    "custom:budget": 1200
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["nodeType"], "project");
        assert_eq!(out["status"], "active");
        assert_eq!(out["priority"], "high");
        assert_eq!(out["startDate"], "2026-03-01");
        assert_eq!(out["endDate"], "2026-04-30");
        assert!(out["properties"].get("project").is_none());
        assert_eq!(out["properties"]["custom:budget"], 1200);
    }

    #[test]
    fn project_without_status_defaults_to_planning() {
        let node = Node::new(
            "project".to_string(),
            "Launch".to_string(),
            serde_json::json!({ "project": {} }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["status"], "planning");
        assert!(out.get("priority").is_none());
        assert!(out.get("startDate").is_none());
    }

    #[test]
    fn ai_chat_promotes_fields_top_level_and_flattens_properties() {
        let node = Node::new(
            "ai-chat".to_string(),
            "Chat".to_string(),
            serde_json::json!({
                "ai-chat": {
                    "turn_status": "idle",
                    "session_status": "active",
                    "provider": "openai-compat",
                    "messages": [{ "role": "user", "content": "hi" }]
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(out["turnStatus"], "idle");
        assert_eq!(out["sessionStatus"], "active");
        assert_eq!(out["provider"], "openai-compat");
        assert_eq!(out["messages"][0]["content"], "hi");
        assert!(out["properties"].get("ai-chat").is_none());
        assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
    }

    /// The split's wire-level guarantee: archiving a session must not disturb
    /// the promoted turn-state field, and vice versa — both are promoted
    /// independently rather than through one shared `status` key.
    #[test]
    fn ai_chat_turn_status_and_session_status_promote_independently() {
        let node = Node::new(
            "ai-chat".to_string(),
            "Chat".to_string(),
            serde_json::json!({
                "ai-chat": {
                    "turn_status": "processing",
                    "session_status": "archived",
                    "messages": []
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(
            out["turnStatus"], "processing",
            "turn_status must promote even while the session is archived"
        );
        assert_eq!(
            out["sessionStatus"], "archived",
            "session_status must promote even while a turn is processing"
        );
    }

    /// Deliberately NOT recognized: see `nodespace_core::models::AiChatNode::
    /// from_node`'s doc comment. Writes here deep-merge onto the existing
    /// stored object rather than replacing it, so a camelCase key, once
    /// written, is never cleaned up — recognizing it would let a stale value
    /// permanently shadow every subsequent fresh canonical write. Every
    /// writer must use the canonical snake_case name; this promotion must
    /// stay blind to the camelCase form, not just deprioritize it.
    #[test]
    fn ai_chat_ignores_camel_case_turn_and_session_status() {
        let node = Node::new(
            "ai-chat".to_string(),
            "Chat".to_string(),
            serde_json::json!({
                "ai-chat": {
                    "turnStatus": "processing",
                    "sessionStatus": "archived",
                    "messages": []
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        assert_eq!(
            out["turnStatus"], "",
            "camelCase must not promote the turn axis"
        );
        assert_eq!(
            out["sessionStatus"], "",
            "camelCase must not promote the session axis"
        );
    }

    /// One unreadable message must not blank the whole conversation in the UI.
    ///
    /// `canonicalArgs` is required on a completed write, so a record without one
    /// fails to decode. Decoding the array as a single `Vec` would fail
    /// wholesale on that one element and send the frontend an empty history for
    /// a conversation that still has readable messages.
    #[test]
    fn ai_chat_one_unreadable_message_does_not_blank_the_conversation() {
        let node = Node::new(
            "ai-chat".to_string(),
            "Chat".to_string(),
            serde_json::json!({
                "ai-chat": {
                    "turn_status": "idle",
                    "session_status": "active",
                    "messages": [
                        { "role": "user", "content": "hi" },
                        {
                            "role": "assistant",
                            "content": "Added.",
                            "completedWrites": [
                                { "tool": "create_node", "nodeId": "nodespace://n1" }
                            ]
                        },
                        { "role": "user", "content": "thanks" }
                    ]
                }
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        let messages = out["messages"].as_array().expect("messages array");
        assert_eq!(messages.len(), 2, "only the unreadable message may be lost");
        assert_eq!(messages[0]["content"], "hi");
        assert_eq!(messages[1]["content"], "thanks");
    }

    #[test]
    fn schema_promotes_fields_top_level_and_injects_uri() {
        let node = Node::new(
            "schema".to_string(),
            "Task schema".to_string(),
            serde_json::json!({
                "isCore": true,
                "schemaVersion": 2,
                "description": "Task type",
                "fields": []
            }),
        );
        let out = node_to_typed_value(node).unwrap();

        // Schema fields are promoted to the top level by SchemaNode.
        assert_eq!(out["isCore"], true);
        assert_eq!(out["schemaVersion"], 2);
        assert_eq!(out["description"], "Task type");
        assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
    }

    /// A schema node with malformed `fields` JSON must not zero out the rest
    /// of an unrelated batch read: `nodes_to_typed_values` `.collect()`s a
    /// `Vec<Result<_, _>>` into a single `Result<Vec<_>, _>`, so if
    /// `SchemaNode::from_node`'s fields-parse failure were ever propagated as
    /// this function's own `Err` (rather than defaulting to an empty `Vec`
    /// with a diagnostic printed on the side), one bad schema node would fail
    /// every other node in the same batch. This pins the batch-safety
    /// property the fix for the silent-swallow bug deliberately preserved.
    #[test]
    fn schema_with_malformed_fields_does_not_fail_an_unrelated_batch_read() {
        let good_task = Node::new(
            "task".to_string(),
            "Buy milk".to_string(),
            serde_json::json!({ "task": { "status": "open" } }),
        );
        let bad_schema = Node::new(
            "schema".to_string(),
            "Broken schema".to_string(),
            serde_json::json!({
                "isCore": false,
                "schemaVersion": 1,
                // `type` must be a string — this is a genuine parse failure,
                // not merely an absent/defaulted field.
                "fields": [{ "name": "status", "type": 42 }],
            }),
        );
        let other_good_task = Node::new(
            "task".to_string(),
            "Walk dog".to_string(),
            serde_json::json!({ "task": { "status": "done" } }),
        );

        let out = nodes_to_typed_values(vec![good_task, bad_schema, other_good_task])
            .expect("one malformed schema node must not fail the whole batch");

        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["status"], "open");
        // The malformed schema node still degrades to an empty `fields` Vec
        // (unchanged behavior) rather than dropping out of the batch.
        assert_eq!(out[1]["fields"], serde_json::json!([]));
        assert_eq!(out[2]["status"], "done");
    }

    /// Mirrors `schema_with_malformed_fields_does_not_fail_an_unrelated_batch_read`
    /// above for the sibling `relationships` field, which has the identical
    /// silent-swallow-shaped fix (`SchemaNode::from_node` delegates to
    /// `parse_relationships`, analogous to `parse_fields`).
    #[test]
    fn schema_with_malformed_relationships_does_not_fail_an_unrelated_batch_read() {
        let good_task = Node::new(
            "task".to_string(),
            "Buy milk".to_string(),
            serde_json::json!({ "task": { "status": "open" } }),
        );
        let bad_schema = Node::new(
            "schema".to_string(),
            "Broken schema".to_string(),
            serde_json::json!({
                "isCore": false,
                "schemaVersion": 1,
                // `direction` must be "out"/"in" — this is a genuine parse
                // failure, not merely an absent/defaulted field.
                "relationships": [{ "name": "assigned_to", "direction": 42, "cardinality": "one" }],
            }),
        );
        let other_good_task = Node::new(
            "task".to_string(),
            "Walk dog".to_string(),
            serde_json::json!({ "task": { "status": "done" } }),
        );

        let out = nodes_to_typed_values(vec![good_task, bad_schema, other_good_task])
            .expect("one malformed schema node must not fail the whole batch");

        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["status"], "open");
        // The malformed schema node still degrades to an empty `relationships`
        // Vec (unchanged behavior) rather than dropping out of the batch.
        assert_eq!(out[1]["relationships"], serde_json::json!([]));
        assert_eq!(out[2]["status"], "done");
    }
}

/// Property tests for the Node → wire-JSON promotion contract.
///
/// The wire conversion is intentionally one-directional (a `Node` becomes a flat,
/// top-level JSON shape for the frontend), so these assert the property that
/// *matters* for silent data loss: **every field a typed node stores under its
/// `properties.<type>` namespace is promoted to a top-level key in the output**.
///
/// If someone adds a field to the stored shape but forgets to model it on the
/// wire struct (`TaskNode` / `AiChatNode`), the promoted field vanishes from the
/// output and the corresponding proptest fails — turning a silent data-drop into
/// a test failure, which is the whole point of this guard.
#[cfg(test)]
mod promotion_proptests {
    use super::*;
    use crate::node::Node;
    use proptest::prelude::*;

    /// Arbitrary task `status` string (both core and user-defined values).
    fn task_status() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("open".to_string()),
            Just("in_progress".to_string()),
            Just("done".to_string()),
            Just("cancelled".to_string()),
            "[a-z][a-z_]{0,15}".prop_map(|s| s),
        ]
    }

    /// Arbitrary task `priority` string (core and user-defined values).
    fn task_priority() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("highest".to_string()),
            Just("high".to_string()),
            Just("medium".to_string()),
            Just("low".to_string()),
            Just("lowest".to_string()),
            "[a-z][a-z_]{0,15}".prop_map(|s| s),
        ]
    }

    /// Arbitrary date-only string (`YYYY-MM-DD`) that `normalize_date_field`
    /// passes through unchanged.
    fn date_string() -> impl Strategy<Value = String> {
        (2000u32..2100, 1u32..=12, 1u32..=28)
            .prop_map(|(y, m, d)| format!("{:04}-{:02}-{:02}", y, m, d))
    }

    proptest! {
        /// Every task field stored under `properties.task` is promoted to a
        /// top-level key, and the `nodespace://` uri is injected.
        #[test]
        fn task_promotes_all_stored_fields(
            status in task_status(),
            priority in task_priority(),
            due_date in date_string(),
            started_at in date_string(),
            completed_at in date_string(),
        ) {
            let node = Node::new(
                "task".to_string(),
                "Some task".to_string(),
                serde_json::json!({
                    "task": {
                        "status": status,
                        "priority": priority,
                        "dueDate": due_date,
                        "startedAt": started_at,
                        "completedAt": completed_at,
                    }
                }),
            );

            let out = node_to_typed_value(node).unwrap();

            // Each stored field promoted to the top level, verbatim.
            prop_assert_eq!(&out["status"], &serde_json::json!(status));
            prop_assert_eq!(&out["priority"], &serde_json::json!(priority));
            prop_assert_eq!(&out["dueDate"], &serde_json::json!(due_date));
            prop_assert_eq!(&out["startedAt"], &serde_json::json!(started_at));
            prop_assert_eq!(&out["completedAt"], &serde_json::json!(completed_at));
            // Namespace flattened away, uri injected.
            prop_assert!(out["properties"].get("task").is_none());
            prop_assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
        }

        /// Every ai-chat field stored under `properties.ai-chat` is promoted to a
        /// top-level key, and the `nodespace://` uri is injected. `turn_status`
        /// and `session_status` promote independently — arbitrary, unrelated
        /// generated values for each must both survive without either clobbering
        /// the other.
        #[test]
        fn ai_chat_promotes_all_stored_fields(
            turn_status in "[a-z][a-z_]{0,15}",
            session_status in "[a-z][a-z_]{0,15}",
            provider in "[a-z][a-z0-9_-]{0,15}",
            model in "[a-zA-Z0-9._:-]{1,25}",
            message in "[ -~]{0,40}",
        ) {
            let node = Node::new(
                "ai-chat".to_string(),
                "A chat".to_string(),
                serde_json::json!({
                    "ai-chat": {
                        "turn_status": turn_status,
                        "session_status": session_status,
                        "provider": provider,
                        "model": model,
                        "messages": [{ "role": "user", "content": message }],
                    }
                }),
            );

            let out = node_to_typed_value(node).unwrap();

            prop_assert_eq!(&out["turnStatus"], &serde_json::json!(turn_status));
            prop_assert_eq!(&out["sessionStatus"], &serde_json::json!(session_status));
            prop_assert_eq!(&out["provider"], &serde_json::json!(provider));
            prop_assert_eq!(&out["model"], &serde_json::json!(model));
            prop_assert_eq!(&out["messages"][0]["content"], &serde_json::json!(message));
            prop_assert!(out["properties"].get("ai-chat").is_none());
            prop_assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
        }

        /// Every schema field stored in the schema node's flat properties is
        /// promoted to a top-level key, and the `nodespace://` uri is injected.
        #[test]
        fn schema_promotes_all_stored_fields(
            is_core in any::<bool>(),
            schema_version in 1u32..100,
            description in "[ -~]{0,40}",
        ) {
            let node = Node::new(
                "schema".to_string(),
                "A schema".to_string(),
                serde_json::json!({
                    "isCore": is_core,
                    "schemaVersion": schema_version,
                    "description": description,
                    "fields": [],
                }),
            );

            let out = node_to_typed_value(node).unwrap();

            prop_assert_eq!(&out["isCore"], &serde_json::json!(is_core));
            prop_assert_eq!(&out["schemaVersion"], &serde_json::json!(schema_version));
            prop_assert_eq!(&out["description"], &serde_json::json!(description));
            prop_assert!(out["uri"].as_str().unwrap().starts_with("nodespace://"));
        }
    }
}
