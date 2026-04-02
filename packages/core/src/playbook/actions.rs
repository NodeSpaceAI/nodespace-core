//! Action Executor for the Playbook Engine (Phase 4)
//!
//! Executes graph operations (create_node, update_node, add_relationship,
//! remove_relationship) with a sequential execution model and incremental
//! binding context. On failure, remaining actions are skipped (abort, no rollback).
//!
//! # Binding Context
//!
//! A `BindingContext` accumulates state as each action executes:
//! - `trigger.node.*` — the full wire-format trigger node
//! - `trigger.property.key/old_value/new_value` — for property_changed events
//! - `actions[N].result.*` — result of the Nth completed action
//! - `item.*` — current element during `for_each` iteration
//!
//! `{dot.path}` bindings in action params are resolved at execution time
//! against the live graph state.

use crate::db::events::{DomainEvent, PlaybookExecutionContext};
use crate::models::{Node, NodeUpdate};
use crate::playbook::types::{ActionType, ParsedAction};
use crate::services::{NodeService, NodeServiceError};
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// ActionError
// ---------------------------------------------------------------------------

/// Errors during action execution.
#[derive(Debug)]
pub enum ActionError {
    /// A binding path like `{trigger.node.status}` could not be resolved.
    BindingResolutionFailed { path: String, message: String },
    /// Missing required parameter in action params.
    MissingParam { param: String, action_index: usize },
    /// NodeService error (create/update/relationship failed).
    ServiceError { message: String, action_index: usize },
    /// Version conflict during `update_node` (optimistic concurrency).
    VersionConflict { node_id: String, action_index: usize },
    /// `for_each` collection could not be resolved or is not an array.
    ForEachResolutionFailed { path: String, message: String },
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BindingResolutionFailed { path, message } => {
                write!(f, "binding resolution failed for '{}': {}", path, message)
            }
            Self::MissingParam {
                param,
                action_index,
            } => {
                write!(
                    f,
                    "missing required param '{}' in action[{}]",
                    param, action_index
                )
            }
            Self::ServiceError {
                message,
                action_index,
            } => {
                write!(f, "action[{}] service error: {}", action_index, message)
            }
            Self::VersionConflict {
                node_id,
                action_index,
            } => {
                write!(
                    f,
                    "action[{}] version conflict for node '{}'",
                    action_index, node_id
                )
            }
            Self::ForEachResolutionFailed { path, message } => {
                write!(
                    f,
                    "for_each resolution failed for '{}': {}",
                    path, message
                )
            }
        }
    }
}

impl std::error::Error for ActionError {}

// ---------------------------------------------------------------------------
// ActionResult
// ---------------------------------------------------------------------------

/// Result of executing all actions for a rule.
#[derive(Debug)]
pub enum ActionResult {
    /// All actions completed successfully.
    Success,
    /// An action failed -- rule should be aborted, playbook disabled.
    Failed(ActionError),
}

// ---------------------------------------------------------------------------
// BindingContext
// ---------------------------------------------------------------------------

/// Property-change bindings populated for `property_changed` events.
struct PropertyBindings {
    key: String,
    old_value: Value,
    new_value: Value,
}

/// Binding context that accumulates results as actions execute.
///
/// Built up incrementally:
/// - At rule start: `trigger.node` and (optionally) `trigger.property.*`
/// - After each action: `actions[N].result`
/// - During `for_each`: `item` for the current iteration element
pub struct BindingContext {
    /// `trigger.node` -- the wire-format trigger node as JSON
    trigger_node: Value,
    /// `trigger.property.{key,old_value,new_value}` for PropertyChanged events
    trigger_property: Option<PropertyBindings>,
    /// `actions[N].result` -- results from completed actions
    action_results: Vec<Value>,
    /// `item` -- current `for_each` iteration element
    current_item: Option<Value>,
}

impl BindingContext {
    /// Create a new context from the trigger node and the domain event.
    ///
    /// Populates `trigger.node` with the full JSON representation and, for
    /// `NodeUpdated` events, populates `trigger.property` from the first
    /// changed property.
    pub fn new(trigger_node: &Node, event: &DomainEvent) -> Self {
        let trigger_node_value = serde_json::to_value(trigger_node).unwrap_or(json!({}));

        let trigger_property = if let DomainEvent::NodeUpdated {
            changed_properties, ..
        } = event
        {
            changed_properties.first().map(|pc| PropertyBindings {
                key: pc.key.clone(),
                old_value: pc.old_value.clone().unwrap_or(Value::Null),
                new_value: pc.new_value.clone().unwrap_or(Value::Null),
            })
        } else {
            None
        };

        Self {
            trigger_node: trigger_node_value,
            trigger_property,
            action_results: Vec::new(),
            current_item: None,
        }
    }

    /// Resolve a dot-path binding against the context.
    ///
    /// Supported roots: `trigger`, `actions`, `item`.
    ///
    /// Handles both `actions[0].result.field` and `actions.0.result.field` formats.
    pub fn resolve_binding(&self, path: &str) -> Result<Value, String> {
        let segments: Vec<&str> = path.split('.').collect();
        let first = segments.first().copied().ok_or("empty binding path")?;

        // Handle "actions[N]" as a combined first segment (e.g., "actions[0].result.id")
        if first.starts_with("actions[") {
            let index_part = first.trim_start_matches("actions");
            // Reconstruct segments as if first was "actions" and second was "[N]"
            let mut reconstructed: Vec<&str> = vec![index_part];
            reconstructed.extend_from_slice(&segments[1..]);
            return self.resolve_action_path(&reconstructed);
        }

        match first {
            "trigger" => self.resolve_trigger_path(&segments[1..]),
            "actions" => self.resolve_action_path(&segments[1..]),
            "item" => self.resolve_item_path(&segments[1..]),
            other => Err(format!("unknown binding root: '{}'", other)),
        }
    }

    fn resolve_trigger_path(&self, segments: &[&str]) -> Result<Value, String> {
        match segments.first().copied() {
            Some("node") => navigate_json(&self.trigger_node, &segments[1..]),
            Some("property") => {
                let prop = self
                    .trigger_property
                    .as_ref()
                    .ok_or("no trigger.property available (not a PropertyChanged event)")?;
                match segments.get(1).copied() {
                    Some("key") => Ok(json!(prop.key)),
                    Some("old_value") => Ok(prop.old_value.clone()),
                    Some("new_value") => Ok(prop.new_value.clone()),
                    Some(other) => Err(format!("unknown trigger.property field: '{}'", other)),
                    None => {
                        // Return the whole property object
                        Ok(json!({
                            "key": prop.key,
                            "old_value": prop.old_value,
                            "new_value": prop.new_value,
                        }))
                    }
                }
            }
            Some(other) => Err(format!("unknown trigger field: '{}'", other)),
            None => {
                // Return the whole trigger object
                Ok(json!({
                    "node": self.trigger_node,
                }))
            }
        }
    }

    fn resolve_action_path(&self, segments: &[&str]) -> Result<Value, String> {
        // Expected format: actions[N].result.field... where segments[0] = "[N]"
        let index_segment = segments
            .first()
            .ok_or("actions path requires an index (e.g., actions[0].result)")?;

        // Parse the index -- accept "[N]" or just "N"
        let index_str = index_segment
            .trim_start_matches('[')
            .trim_end_matches(']');
        let index: usize = index_str
            .parse()
            .map_err(|_| format!("invalid action index: '{}'", index_segment))?;

        let result = self
            .action_results
            .get(index)
            .ok_or_else(|| format!("action[{}] has no result yet", index))?;

        // segments[1] should be "result", then navigate remaining
        match segments.get(1).copied() {
            Some("result") => navigate_json(result, &segments[2..]),
            Some(other) => Err(format!(
                "unknown actions[{}] field: '{}' (expected 'result')",
                index, other
            )),
            None => Ok(result.clone()),
        }
    }

    fn resolve_item_path(&self, segments: &[&str]) -> Result<Value, String> {
        let item = self
            .current_item
            .as_ref()
            .ok_or("no item available (not in a for_each loop)")?;
        navigate_json(item, segments)
    }
}

// ---------------------------------------------------------------------------
// JSON navigation
// ---------------------------------------------------------------------------

/// Navigate into a JSON value by path segments.
///
/// Each segment is used as an object key. Returns the value at the terminal
/// segment, or an error if any intermediate segment is missing.
fn navigate_json(value: &Value, segments: &[&str]) -> Result<Value, String> {
    let mut current = value;
    for &segment in segments {
        current = current
            .get(segment)
            .ok_or_else(|| format!("path segment '{}' not found", segment))?;
    }
    Ok(current.clone())
}

// ---------------------------------------------------------------------------
// Binding resolution in JSON values
// ---------------------------------------------------------------------------

/// Resolve all `{binding}` templates in a JSON value recursively.
fn resolve_bindings_in_value(value: &Value, ctx: &BindingContext) -> Result<Value, ActionError> {
    match value {
        Value::String(s) => resolve_bindings_in_string(s, ctx),
        Value::Object(obj) => {
            let mut resolved = serde_json::Map::new();
            for (k, v) in obj {
                resolved.insert(k.clone(), resolve_bindings_in_value(v, ctx)?);
            }
            Ok(Value::Object(resolved))
        }
        Value::Array(arr) => {
            let resolved: Result<Vec<_>, _> =
                arr.iter().map(|v| resolve_bindings_in_value(v, ctx)).collect();
            Ok(Value::Array(resolved?))
        }
        other => Ok(other.clone()),
    }
}

/// Resolve `{binding.path}` in a string.
///
/// If the entire string is a single `{binding}`, the resolved value is returned
/// directly (preserving its JSON type: number, bool, object, etc.).
///
/// If the string contains bindings mixed with literal text, each binding is
/// stringified and interpolated into the result (always returns a string).
fn resolve_bindings_in_string(s: &str, ctx: &BindingContext) -> Result<Value, ActionError> {
    // Fast path: entire string is a single binding like "{trigger.node.id}"
    if s.starts_with('{') && s.ends_with('}') && !s[1..s.len() - 1].contains('{') {
        let path = &s[1..s.len() - 1];
        return ctx
            .resolve_binding(path)
            .map_err(|msg| ActionError::BindingResolutionFailed {
                path: path.to_string(),
                message: msg,
            });
    }

    // General case: scan for `{...}` patterns and interpolate
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(&ch) = chars.peek() {
        if ch == '{' {
            chars.next(); // consume '{'
            let mut path = String::new();
            let mut found_close = false;
            for ch_inner in chars.by_ref() {
                if ch_inner == '}' {
                    found_close = true;
                    break;
                }
                path.push(ch_inner);
            }
            if !found_close {
                // Unterminated brace -- treat as literal
                result.push('{');
                result.push_str(&path);
            } else {
                let resolved = ctx.resolve_binding(&path).map_err(|msg| {
                    ActionError::BindingResolutionFailed {
                        path: path.clone(),
                        message: msg,
                    }
                })?;
                match &resolved {
                    Value::String(sv) => result.push_str(sv),
                    Value::Null => result.push_str("null"),
                    other => result.push_str(&other.to_string()),
                }
            }
        } else {
            result.push(ch);
            chars.next();
        }
    }

    Ok(Value::String(result))
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Execute all actions for a rule sequentially.
///
/// Builds a `BindingContext` from the trigger node and event, then runs each
/// action in order. After each action completes, its result is added to
/// `actions[N].result`. If any action fails, remaining actions are skipped
/// and `ActionResult::Failed` is returned.
///
/// The `execution_context` is threaded through to `NodeService` so that events
/// emitted by action mutations carry `PlaybookExecutionContext` for cycle detection.
pub async fn execute_actions(
    actions: &[ParsedAction],
    trigger_node: &Node,
    event: &DomainEvent,
    node_service: &Arc<NodeService>,
    execution_context: PlaybookExecutionContext,
) -> ActionResult {
    // Create a scoped NodeService that tags all mutations with the execution context.
    // This ensures events emitted by actions carry playbook_context for cycle detection.
    let scoped_service = Arc::new(node_service.with_execution_context(execution_context));
    let mut ctx = BindingContext::new(trigger_node, event);

    for (i, action) in actions.iter().enumerate() {
        if let Some(for_each_path) = &action.for_each {
            // ---------------------------------------------------------------
            // for_each execution
            // ---------------------------------------------------------------

            // Step 1: Resolve the full collection before iteration begins
            let collection = match ctx.resolve_binding(for_each_path) {
                Ok(Value::Array(items)) => items,
                Ok(_) => {
                    return ActionResult::Failed(ActionError::ForEachResolutionFailed {
                        path: for_each_path.clone(),
                        message: "for_each path did not resolve to an array".to_string(),
                    });
                }
                Err(msg) => {
                    return ActionResult::Failed(ActionError::ForEachResolutionFailed {
                        path: for_each_path.clone(),
                        message: msg,
                    });
                }
            };

            debug!(
                "action[{}] for_each over {} items from '{}'",
                i,
                collection.len(),
                for_each_path,
            );

            // Step 2: Execute the action for each item
            for (item_idx, item) in collection.iter().enumerate() {
                ctx.current_item = Some(item.clone());

                // Re-resolve params with the item binding available
                let item_params = match resolve_bindings_in_value(&action.params, &ctx) {
                    Ok(p) => p,
                    Err(e) => return ActionResult::Failed(e),
                };

                match execute_single_action(i, &action.action_type, &item_params, &scoped_service)
                    .await
                {
                    Ok(_) => {
                        debug!("action[{}] for_each item[{}] succeeded", i, item_idx);
                    }
                    Err(e) => {
                        warn!(
                            "action[{}] for_each item[{}] failed, aborting rule: {}",
                            i, item_idx, e
                        );
                        return ActionResult::Failed(e);
                    }
                }
            }

            ctx.current_item = None;
            // for_each doesn't produce a single result -- push Null placeholder
            ctx.action_results.push(Value::Null);
        } else {
            // ---------------------------------------------------------------
            // Single action execution
            // ---------------------------------------------------------------

            // Resolve bindings in params
            let resolved_params = match resolve_bindings_in_value(&action.params, &ctx) {
                Ok(p) => p,
                Err(e) => return ActionResult::Failed(e),
            };

            match execute_single_action(i, &action.action_type, &resolved_params, &scoped_service)
                .await
            {
                Ok(result_value) => {
                    debug!("action[{}] succeeded", i);
                    ctx.action_results.push(result_value);
                }
                Err(e) => {
                    warn!("action[{}] failed, aborting rule: {}", i, e);
                    return ActionResult::Failed(e);
                }
            }
        }
    }

    ActionResult::Success
}

// ---------------------------------------------------------------------------
// Individual action executors
// ---------------------------------------------------------------------------

/// Execute a single action and return the result as JSON.
async fn execute_single_action(
    action_index: usize,
    action_type: &ActionType,
    params: &Value,
    node_service: &Arc<NodeService>,
) -> Result<Value, ActionError> {
    match action_type {
        ActionType::CreateNode => execute_create_node(action_index, params, node_service).await,
        ActionType::UpdateNode => execute_update_node(action_index, params, node_service).await,
        ActionType::AddRelationship => {
            execute_add_relationship(action_index, params, node_service).await
        }
        ActionType::RemoveRelationship => {
            execute_remove_relationship(action_index, params, node_service).await
        }
    }
}

async fn execute_create_node(
    action_index: usize,
    params: &Value,
    node_service: &Arc<NodeService>,
) -> Result<Value, ActionError> {
    let node_type = params
        .get("node_type")
        .and_then(|v| v.as_str())
        .ok_or(ActionError::MissingParam {
            param: "node_type".to_string(),
            action_index,
        })?;
    let content = params
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let properties = params.get("properties").cloned().unwrap_or(json!({}));

    let node = Node::new(node_type.to_string(), content.to_string(), properties);
    let node_id = node.id.clone();

    node_service
        .create_node(node)
        .await
        .map_err(|e| ActionError::ServiceError {
            message: e.to_string(),
            action_index,
        })?;

    // Fetch the created node to return as result
    let created = node_service
        .get_node(&node_id)
        .await
        .map_err(|e| ActionError::ServiceError {
            message: e.to_string(),
            action_index,
        })?
        .ok_or(ActionError::ServiceError {
            message: "created node not found after create".to_string(),
            action_index,
        })?;

    serde_json::to_value(&created).map_err(|e| ActionError::ServiceError {
        message: e.to_string(),
        action_index,
    })
}

async fn execute_update_node(
    action_index: usize,
    params: &Value,
    node_service: &Arc<NodeService>,
) -> Result<Value, ActionError> {
    let node_id = params
        .get("node_id")
        .and_then(|v| v.as_str())
        .ok_or(ActionError::MissingParam {
            param: "node_id".to_string(),
            action_index,
        })?;

    // Fetch current node for optimistic concurrency
    let current = node_service
        .get_node(node_id)
        .await
        .map_err(|e| ActionError::ServiceError {
            message: e.to_string(),
            action_index,
        })?
        .ok_or(ActionError::ServiceError {
            message: format!("node '{}' not found", node_id),
            action_index,
        })?;

    let mut update = NodeUpdate::default();
    if let Some(content) = params.get("content").and_then(|v| v.as_str()) {
        update.content = Some(content.to_string());
    }
    if let Some(properties) = params.get("properties") {
        update.properties = Some(properties.clone());
    }
    if let Some(status) = params.get("lifecycle_status").and_then(|v| v.as_str()) {
        update.lifecycle_status = Some(status.to_string());
    }
    if let Some(node_type) = params.get("node_type").and_then(|v| v.as_str()) {
        update.node_type = Some(node_type.to_string());
    }

    let updated = node_service
        .update_node(node_id, current.version, update)
        .await
        .map_err(|e| match &e {
            NodeServiceError::VersionConflict { .. } => ActionError::VersionConflict {
                node_id: node_id.to_string(),
                action_index,
            },
            _ => ActionError::ServiceError {
                message: e.to_string(),
                action_index,
            },
        })?;

    serde_json::to_value(&updated).map_err(|e| ActionError::ServiceError {
        message: e.to_string(),
        action_index,
    })
}

async fn execute_add_relationship(
    action_index: usize,
    params: &Value,
    node_service: &Arc<NodeService>,
) -> Result<Value, ActionError> {
    let source_id =
        params
            .get("source_id")
            .and_then(|v| v.as_str())
            .ok_or(ActionError::MissingParam {
                param: "source_id".to_string(),
                action_index,
            })?;
    let relationship_type = params.get("relationship_type").and_then(|v| v.as_str()).ok_or(
        ActionError::MissingParam {
            param: "relationship_type".to_string(),
            action_index,
        },
    )?;
    let target_id =
        params
            .get("target_id")
            .and_then(|v| v.as_str())
            .ok_or(ActionError::MissingParam {
                param: "target_id".to_string(),
                action_index,
            })?;
    let edge_data = params.get("edge_data").cloned().unwrap_or(json!({}));

    node_service
        .create_relationship(source_id, relationship_type, target_id, edge_data)
        .await
        .map_err(|e| ActionError::ServiceError {
            message: e.to_string(),
            action_index,
        })?;

    Ok(json!({
        "source_id": source_id,
        "target_id": target_id,
        "relationship_type": relationship_type,
    }))
}

async fn execute_remove_relationship(
    action_index: usize,
    params: &Value,
    node_service: &Arc<NodeService>,
) -> Result<Value, ActionError> {
    let source_id =
        params
            .get("source_id")
            .and_then(|v| v.as_str())
            .ok_or(ActionError::MissingParam {
                param: "source_id".to_string(),
                action_index,
            })?;
    let relationship_type = params.get("relationship_type").and_then(|v| v.as_str()).ok_or(
        ActionError::MissingParam {
            param: "relationship_type".to_string(),
            action_index,
        },
    )?;
    let target_id =
        params
            .get("target_id")
            .and_then(|v| v.as_str())
            .ok_or(ActionError::MissingParam {
                param: "target_id".to_string(),
                action_index,
            })?;

    node_service
        .delete_relationship(source_id, relationship_type, target_id)
        .await
        .map_err(|e| ActionError::ServiceError {
            message: e.to_string(),
            action_index,
        })?;

    Ok(json!({
        "source_id": source_id,
        "target_id": target_id,
        "relationship_type": relationship_type,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::events::{DomainEvent, PropertyChange};
    use crate::models::Node;
    use chrono::Utc;
    use serde_json::json;

    /// Helper: create a minimal node for testing.
    fn make_test_node(id: &str, node_type: &str) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            content: "Test content".to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties: json!({
                "task": { "status": "open", "priority": "high" }
            }),
            mentions: vec![],
            mentioned_in: vec![],
            title: Some("Test Node".to_string()),
            lifecycle_status: "active".to_string(),
        }
    }

    /// Helper: create a NodeCreated event.
    fn make_node_created_event(node_id: &str, node_type: &str) -> DomainEvent {
        DomainEvent::NodeCreated {
            node_id: node_id.to_string(),
            node_type: node_type.to_string(),
        }
    }

    /// Helper: create a NodeUpdated event with property changes.
    fn make_property_changed_event(
        node_id: &str,
        node_type: &str,
        changes: Vec<PropertyChange>,
    ) -> DomainEvent {
        DomainEvent::NodeUpdated {
            node_id: node_id.to_string(),
            node_type: node_type.to_string(),
            changed_properties: changes,
        }
    }

    // -----------------------------------------------------------------------
    // navigate_json tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_navigate_json_simple_path() {
        let value = json!({"a": {"b": {"c": 42}}});
        let result = navigate_json(&value, &["a", "b", "c"]).unwrap();
        assert_eq!(result, json!(42));
    }

    #[test]
    fn test_navigate_json_root_level() {
        let value = json!({"name": "hello"});
        let result = navigate_json(&value, &["name"]).unwrap();
        assert_eq!(result, json!("hello"));
    }

    #[test]
    fn test_navigate_json_empty_path() {
        let value = json!({"a": 1});
        let result = navigate_json(&value, &[]).unwrap();
        assert_eq!(result, json!({"a": 1}));
    }

    #[test]
    fn test_navigate_json_missing_path() {
        let value = json!({"a": 1});
        let err = navigate_json(&value, &["b"]).unwrap_err();
        assert!(err.contains("path segment 'b' not found"));
    }

    #[test]
    fn test_navigate_json_nested_missing() {
        let value = json!({"a": {"b": 1}});
        let err = navigate_json(&value, &["a", "c"]).unwrap_err();
        assert!(err.contains("path segment 'c' not found"));
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — trigger.node paths
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_trigger_node_id() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let result = ctx.resolve_binding("trigger.node.id").unwrap();
        assert_eq!(result, json!("node-123"));
    }

    #[test]
    fn test_resolve_trigger_node_type() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let result = ctx.resolve_binding("trigger.node.nodeType").unwrap();
        assert_eq!(result, json!("task"));
    }

    #[test]
    fn test_resolve_trigger_node_content() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let result = ctx.resolve_binding("trigger.node.content").unwrap();
        assert_eq!(result, json!("Test content"));
    }

    #[test]
    fn test_resolve_trigger_node_nested_properties() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let result = ctx
            .resolve_binding("trigger.node.properties.task.status")
            .unwrap();
        assert_eq!(result, json!("open"));
    }

    #[test]
    fn test_resolve_trigger_node_title() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let result = ctx.resolve_binding("trigger.node.title").unwrap();
        assert_eq!(result, json!("Test Node"));
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — trigger.property paths
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_trigger_property_key() {
        let node = make_test_node("node-123", "task");
        let event = make_property_changed_event(
            "node-123",
            "task",
            vec![PropertyChange {
                key: "task.status".to_string(),
                old_value: Some(json!("open")),
                new_value: Some(json!("done")),
            }],
        );
        let ctx = BindingContext::new(&node, &event);

        assert_eq!(
            ctx.resolve_binding("trigger.property.key").unwrap(),
            json!("task.status")
        );
    }

    #[test]
    fn test_resolve_trigger_property_old_value() {
        let node = make_test_node("node-123", "task");
        let event = make_property_changed_event(
            "node-123",
            "task",
            vec![PropertyChange {
                key: "task.status".to_string(),
                old_value: Some(json!("open")),
                new_value: Some(json!("done")),
            }],
        );
        let ctx = BindingContext::new(&node, &event);

        assert_eq!(
            ctx.resolve_binding("trigger.property.old_value").unwrap(),
            json!("open")
        );
    }

    #[test]
    fn test_resolve_trigger_property_new_value() {
        let node = make_test_node("node-123", "task");
        let event = make_property_changed_event(
            "node-123",
            "task",
            vec![PropertyChange {
                key: "task.status".to_string(),
                old_value: Some(json!("open")),
                new_value: Some(json!("done")),
            }],
        );
        let ctx = BindingContext::new(&node, &event);

        assert_eq!(
            ctx.resolve_binding("trigger.property.new_value").unwrap(),
            json!("done")
        );
    }

    #[test]
    fn test_resolve_trigger_property_null_old_value() {
        let node = make_test_node("node-123", "task");
        let event = make_property_changed_event(
            "node-123",
            "task",
            vec![PropertyChange {
                key: "task.priority".to_string(),
                old_value: None,
                new_value: Some(json!("high")),
            }],
        );
        let ctx = BindingContext::new(&node, &event);

        assert_eq!(
            ctx.resolve_binding("trigger.property.old_value").unwrap(),
            Value::Null
        );
    }

    #[test]
    fn test_resolve_trigger_property_not_available_on_created() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let err = ctx.resolve_binding("trigger.property.key").unwrap_err();
        assert!(err.contains("not a PropertyChanged event"));
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — actions[N].result paths
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_action_result() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event);

        ctx.action_results
            .push(json!({"id": "new-node-456", "nodeType": "text"}));

        let result = ctx.resolve_binding("actions[0].result.id").unwrap();
        assert_eq!(result, json!("new-node-456"));
    }

    #[test]
    fn test_resolve_action_result_no_bracket_syntax() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event);

        ctx.action_results.push(json!({"id": "abc"}));

        // Also works with just the number (without brackets)
        let result = ctx.resolve_binding("actions.0.result.id").unwrap();
        assert_eq!(result, json!("abc"));
    }

    #[test]
    fn test_resolve_action_result_not_yet_available() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let err = ctx.resolve_binding("actions[0].result.id").unwrap_err();
        assert!(err.contains("has no result yet"));
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — item paths
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_item_path() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event);

        ctx.current_item = Some(json!({"id": "item-1", "name": "First"}));

        assert_eq!(ctx.resolve_binding("item.id").unwrap(), json!("item-1"));
        assert_eq!(ctx.resolve_binding("item.name").unwrap(), json!("First"));
    }

    #[test]
    fn test_resolve_item_not_in_loop() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let err = ctx.resolve_binding("item.id").unwrap_err();
        assert!(err.contains("not in a for_each loop"));
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — error cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_unknown_root() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let err = ctx.resolve_binding("unknown.field").unwrap_err();
        assert!(err.contains("unknown binding root"));
    }

    // -----------------------------------------------------------------------
    // resolve_bindings_in_string tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_single_binding_preserves_type_number() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        // version is a number, should be preserved as json number
        let result = resolve_bindings_in_string("{trigger.node.version}", &ctx).unwrap();
        assert_eq!(result, json!(1));
    }

    #[test]
    fn test_single_binding_preserves_type_string() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let result = resolve_bindings_in_string("{trigger.node.id}", &ctx).unwrap();
        assert_eq!(result, json!("node-123"));
    }

    #[test]
    fn test_single_binding_preserves_type_object() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let result =
            resolve_bindings_in_string("{trigger.node.properties.task}", &ctx).unwrap();
        assert_eq!(result, json!({"status": "open", "priority": "high"}));
    }

    #[test]
    fn test_mixed_text_and_bindings() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let result =
            resolve_bindings_in_string("Node {trigger.node.id} is type {trigger.node.nodeType}", &ctx)
                .unwrap();
        assert_eq!(result, json!("Node node-123 is type task"));
    }

    #[test]
    fn test_no_bindings_returns_literal() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let result = resolve_bindings_in_string("just a plain string", &ctx).unwrap();
        assert_eq!(result, json!("just a plain string"));
    }

    #[test]
    fn test_binding_resolution_failed_error() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let err = resolve_bindings_in_string("{nonexistent.path}", &ctx).unwrap_err();
        match err {
            ActionError::BindingResolutionFailed { path, .. } => {
                assert_eq!(path, "nonexistent.path");
            }
            _ => panic!("expected BindingResolutionFailed"),
        }
    }

    // -----------------------------------------------------------------------
    // resolve_bindings_in_value tests (recursive)
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolve_bindings_in_value_object() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let params = json!({
            "node_type": "text",
            "content": "Created from {trigger.node.id}",
            "properties": {
                "source": "{trigger.node.id}"
            }
        });

        let resolved = resolve_bindings_in_value(&params, &ctx).unwrap();
        assert_eq!(resolved["content"], json!("Created from node-123"));
        assert_eq!(resolved["properties"]["source"], json!("node-123"));
        // node_type has no binding, preserved as-is
        assert_eq!(resolved["node_type"], json!("text"));
    }

    #[test]
    fn test_resolve_bindings_in_value_array() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let params = json!(["{trigger.node.id}", "literal", "{trigger.node.nodeType}"]);

        let resolved = resolve_bindings_in_value(&params, &ctx).unwrap();
        assert_eq!(resolved[0], json!("node-123"));
        assert_eq!(resolved[1], json!("literal"));
        assert_eq!(resolved[2], json!("task"));
    }

    #[test]
    fn test_resolve_bindings_in_value_non_string_passthrough() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let ctx = BindingContext::new(&node, &event);

        let params = json!(42);
        let resolved = resolve_bindings_in_value(&params, &ctx).unwrap();
        assert_eq!(resolved, json!(42));

        let params = json!(true);
        let resolved = resolve_bindings_in_value(&params, &ctx).unwrap();
        assert_eq!(resolved, json!(true));

        let params = json!(null);
        let resolved = resolve_bindings_in_value(&params, &ctx).unwrap();
        assert_eq!(resolved, Value::Null);
    }

    // -----------------------------------------------------------------------
    // ActionError Display tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_action_error_display_binding_resolution() {
        let e = ActionError::BindingResolutionFailed {
            path: "trigger.node.missing".to_string(),
            message: "path not found".to_string(),
        };
        let s = format!("{}", e);
        assert!(s.contains("trigger.node.missing"));
        assert!(s.contains("path not found"));
    }

    #[test]
    fn test_action_error_display_missing_param() {
        let e = ActionError::MissingParam {
            param: "node_type".to_string(),
            action_index: 2,
        };
        let s = format!("{}", e);
        assert!(s.contains("node_type"));
        assert!(s.contains("action[2]"));
    }

    #[test]
    fn test_action_error_display_service_error() {
        let e = ActionError::ServiceError {
            message: "database timeout".to_string(),
            action_index: 0,
        };
        let s = format!("{}", e);
        assert!(s.contains("database timeout"));
        assert!(s.contains("action[0]"));
    }

    #[test]
    fn test_action_error_display_version_conflict() {
        let e = ActionError::VersionConflict {
            node_id: "node-abc".to_string(),
            action_index: 1,
        };
        let s = format!("{}", e);
        assert!(s.contains("node-abc"));
        assert!(s.contains("version conflict"));
    }

    #[test]
    fn test_action_error_display_for_each_failed() {
        let e = ActionError::ForEachResolutionFailed {
            path: "trigger.node.mentions".to_string(),
            message: "not an array".to_string(),
        };
        let s = format!("{}", e);
        assert!(s.contains("trigger.node.mentions"));
        assert!(s.contains("not an array"));
    }
}
