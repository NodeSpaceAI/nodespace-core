//! Action Executor for the Play Engine (Phase 4)
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
//!
//! # Derived Identity (ADR-060 §3, ADR-074)
//!
//! `create_node` action outputs get a deterministic id --
//! [`deterministic_action_output_id`]`(rule_id, action_index, iteration_path)`
//! -- instead of a random one, so N devices independently executing the same
//! rule against the same trigger (or one device re-processing a re-delivered
//! event) converge on the SAME node id and the writes collapse to one row on
//! sync instead of N siblings.
//!
//! `iteration_path` (see [`crate::playbook::types::IterationPath`]) starts as
//! `[trigger_node.id]` -- this serves both the `graph_event` trigger-node
//! case and the `scheduled` trigger's scanned-node case, since the engine
//! hands the scanned node to the action executor as the "trigger node" for a
//! scheduled work item too -- and gains one more real node id per nested
//! `for_each` level entered. It is never built from a loop/positional index:
//! two devices iterating the same set in different orders must still agree
//! on which item produced which id.
//!
//! `rule_id` identifies the specific rule bound to a specific play
//! ([`rule_id_for`]). It is derived from the play id (already available via
//! [`PlaybookExecutionContext::source_playbook_id`], no extra threading
//! required) and the rule's own parsed action list -- NOT a positional
//! `rule_index`. `ParsedRule` carries no id of its own, and threading a
//! positional index into [`execute_actions`] would require changing its only
//! call site. Consequences worth naming:
//! - Editing a rule's actions (reordering, adding, or changing one) changes
//!   `rule_id`, and therefore every id derived from it, from that edit
//!   onward -- the same trade-off ADR-060 §3 already accepts for a
//!   positional `rule_index`.
//! - Two DIFFERENT rules in the SAME play with byte-identical action lists
//!   would collide onto the same `rule_id`. Accepted as a narrow, unlikely
//!   gap (it requires a near-exact duplicate rule) given the alternative
//!   requires a signature change at the engine call site.
//!
//! ## What this does NOT solve
//!
//! ADR-060 failure mode 5 (divergent scan results across devices) is
//! unchanged by this mechanism: a device mid-catch-up can compute a
//! different SET of matches than a fully-synced device (fewer or more items
//! in a `for_each` scan). Derived identity guarantees that whatever outputs
//! two devices DO produce converge to the same rows; it does not guarantee
//! the two devices produce the SAME NUMBER of outputs. That gap is separate,
//! pre-existing, and not addressed here.

use crate::db::events::{DomainEvent, PlaybookExecutionContext};
use crate::models::{Node, NodeUpdate};
use crate::playbook::graph_resolver::GraphResolver;
use crate::playbook::types::{ActionType, IterationPath, ParsedAction};
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
    ServiceError {
        message: String,
        action_index: usize,
    },
    /// Version conflict during `update_node` (optimistic concurrency).
    VersionConflict {
        node_id: String,
        action_index: usize,
    },
    /// `for_each` collection could not be resolved or is not an array.
    ForEachResolutionFailed { path: String, message: String },
    /// A `for_each` item has no resolvable real node id, so it can't produce
    /// a valid [`crate::playbook::types::IterationPath`] element (ADR-074).
    ///
    /// Deliberately NOT handled by falling back to the item's positional
    /// index in the collection -- that would make the derived id depend on
    /// scan order, defeating the reason derived identity exists.
    IterationPathResolutionFailed {
        action_index: usize,
        item_index: usize,
        message: String,
    },
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
                write!(f, "for_each resolution failed for '{}': {}", path, message)
            }
            Self::IterationPathResolutionFailed {
                action_index,
                item_index,
                message,
            } => {
                write!(
                    f,
                    "action[{}] for_each item[{}]: could not resolve a real node id for iteration_path: {}",
                    action_index, item_index, message
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
    /// An action failed -- rule should be aborted, play disabled.
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
    /// The trigger node as a `Node` struct (for graph traversal)
    trigger_node_model: Node,
    /// `trigger.property.{key,old_value,new_value}` for PropertyChanged events
    trigger_property: Option<PropertyBindings>,
    /// `actions[N].result` -- results from completed actions
    action_results: Vec<Value>,
    /// `item` -- current `for_each` iteration element
    current_item: Option<Value>,
    /// Optional graph resolver for multi-hop dot-path resolution
    graph_resolver: Option<GraphResolver>,
    /// Ordered path of real node ids identifying which execution of the
    /// current action this is (ADR-060 §3, ADR-074). Starts as
    /// `[trigger_node.id]` and gains one more real node id per nested
    /// `for_each` level entered; popped back off when that level's item
    /// finishes executing. See the module doc for the full formula.
    iteration_path: IterationPath,
}

impl BindingContext {
    /// Create a new context from the trigger node and the domain event.
    ///
    /// Populates `trigger.node` with the full JSON representation and, for
    /// `NodeUpdated` events, populates `trigger.property` from the first
    /// changed property.
    pub fn new(
        trigger_node: &Node,
        event: &DomainEvent,
        graph_resolver: Option<GraphResolver>,
    ) -> Self {
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
            trigger_node_model: trigger_node.clone(),
            trigger_property,
            action_results: Vec::new(),
            current_item: None,
            graph_resolver,
            iteration_path: vec![trigger_node.id.clone()],
        }
    }

    /// Resolve a dot-path binding against the context.
    ///
    /// Supported roots: `trigger`, `actions`, `item`.
    ///
    /// Handles both `actions[0].result.field` and `actions.0.result.field` formats.
    pub async fn resolve_binding(&mut self, path: &str) -> Result<Value, String> {
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
            "trigger" => self.resolve_trigger_path(&segments[1..]).await,
            "actions" => self.resolve_action_path(&segments[1..]),
            "item" => self.resolve_item_path(&segments[1..]),
            other => Err(format!("unknown binding root: '{}'", other)),
        }
    }

    async fn resolve_trigger_path(&mut self, segments: &[&str]) -> Result<Value, String> {
        match segments.first().copied() {
            Some("node") => {
                // Try JSON navigation first (direct properties)
                match navigate_json(&self.trigger_node, &segments[1..]) {
                    Ok(val) => Ok(val),
                    Err(_) if segments.len() > 2 => {
                        // JSON navigation failed and we have a multi-hop path
                        // Try graph traversal via GraphResolver
                        if let Some(ref mut resolver) = self.graph_resolver {
                            let path_segments: Vec<String> =
                                segments[1..].iter().map(|s| s.to_string()).collect();
                            match resolver
                                .resolve_path(&self.trigger_node_model, &path_segments)
                                .await
                            {
                                crate::playbook::graph_resolver::ResolvedValue::Node(n) => {
                                    serde_json::to_value(&n).map_err(|e| e.to_string())
                                }
                                crate::playbook::graph_resolver::ResolvedValue::Collection(
                                    nodes,
                                ) => serde_json::to_value(&nodes).map_err(|e| e.to_string()),
                                crate::playbook::graph_resolver::ResolvedValue::Scalar(v) => Ok(v),
                                crate::playbook::graph_resolver::ResolvedValue::Missing => {
                                    Err(format!(
                                        "path segment '{}' not found (graph traversal)",
                                        segments.last().unwrap_or(&"")
                                    ))
                                }
                            }
                        } else {
                            Err(format!(
                                "path segment '{}' not found",
                                segments.last().unwrap_or(&"")
                            ))
                        }
                    }
                    Err(e) => Err(e),
                }
            }
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
        let index_str = index_segment.trim_start_matches('[').trim_end_matches(']');
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
// Derived identity (ADR-060 §3, ADR-074)
// ---------------------------------------------------------------------------

/// Stable namespace for playbook action-output ids (UUIDv5). A fixed,
/// arbitrary UUID -- do NOT change it: changing it would silently mint a
/// fresh id for every existing derived node on its next execution, defeating
/// the convergence this whole mechanism exists for. Distinct from
/// `collection_service::COLLECTION_ID_NAMESPACE` and
/// `node_service::conflicts::CONFLICT_ID_NAMESPACE` -- the three id spaces
/// must never collide.
const ACTION_OUTPUT_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6f2a4c8e_3b7d_4f1a_9e6c_2d5b8a1f4c7eu128);

/// Derive a stable id for a playbook action's output: `deterministic_id(rule_id,
/// action_index, iteration_path)` per ADR-060 §3, generalized by ADR-074.
///
/// Two independent executions of the same rule -- the same device
/// re-processing a re-delivered event, or two different devices that each
/// computed the same trigger/scan/`for_each` path independently -- derive the
/// SAME id, so the writes converge to one row on sync instead of N siblings.
/// See the module doc for the full formula and its trade-offs.
pub fn deterministic_action_output_id(
    rule_id: &str,
    action_index: usize,
    iteration_path: &[String],
) -> String {
    let seed = format!("{rule_id}|{action_index}|{}", iteration_path.join("\u{1}"));
    uuid::Uuid::new_v5(&ACTION_OUTPUT_ID_NAMESPACE, seed.as_bytes()).to_string()
}

/// Derive a stable identity for the rule bound to a specific play, from
/// content already available at the action-executor boundary rather than a
/// positional `rule_index`. See the module doc for why, and the trade-offs
/// this implies.
fn rule_id_for(play_id: &str, actions: &[ParsedAction]) -> String {
    let mut seed = String::from(play_id);
    for action in actions {
        seed.push('\u{1}');
        seed.push_str(action.action_type.as_str());
        seed.push('\u{1}');
        if let Some(for_each) = &action.for_each {
            seed.push_str(for_each);
        }
        seed.push('\u{1}');
        seed.push_str(&action.params.to_string());
    }
    seed
}

/// Resolve a `for_each` item's own real node id for [`IterationPath`]
/// purposes (ADR-074). Accepts either shape a resolved collection element
/// can take:
/// - a bare string, e.g. an item from `trigger.node.mentions` (`Vec<String>`
///   of node ids) -- the string itself IS the id;
/// - an object with a non-empty string `"id"` field, e.g. a wire-format
///   `Node` resolved via the graph resolver, or a `{"id": ...}` entry from a
///   scanned properties array.
///
/// Anything else (a number, bool, null, an object with no usable `"id"`) has
/// no real node id to key on and is a hard error -- see
/// [`ActionError::IterationPathResolutionFailed`].
fn resolve_iteration_path_item_id(item: &Value) -> Result<String, String> {
    match item {
        Value::String(s) if !s.is_empty() => Ok(s.clone()),
        Value::String(_) => Err("for_each item is an empty string".to_string()),
        Value::Object(_) => match item.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => Ok(id.to_string()),
            _ => Err(
                "for_each item is an object with no resolvable non-empty string \"id\" field"
                    .to_string(),
            ),
        },
        other => Err(format!(
            "for_each item is neither a real node id string nor an object with an \"id\" field: {other}"
        )),
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
async fn resolve_bindings_in_value(
    value: &Value,
    ctx: &mut BindingContext,
) -> Result<Value, ActionError> {
    match value {
        Value::String(s) => resolve_bindings_in_string(s, ctx).await,
        Value::Object(obj) => {
            let mut resolved = serde_json::Map::new();
            for (k, v) in obj {
                let resolved_v = Box::pin(resolve_bindings_in_value(v, ctx)).await?;
                resolved.insert(k.clone(), resolved_v);
            }
            Ok(Value::Object(resolved))
        }
        Value::Array(arr) => {
            let mut resolved = Vec::with_capacity(arr.len());
            for v in arr {
                resolved.push(Box::pin(resolve_bindings_in_value(v, ctx)).await?);
            }
            Ok(Value::Array(resolved))
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
async fn resolve_bindings_in_string(
    s: &str,
    ctx: &mut BindingContext,
) -> Result<Value, ActionError> {
    // Fast path: entire string is a single binding like "{trigger.node.id}"
    if s.starts_with('{') && s.ends_with('}') && !s[1..s.len() - 1].contains('{') {
        let path = &s[1..s.len() - 1];
        return ctx.resolve_binding(path).await.map_err(|msg| {
            ActionError::BindingResolutionFailed {
                path: path.to_string(),
                message: msg,
            }
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
                let resolved = ctx.resolve_binding(&path).await.map_err(|msg| {
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
    // Extract the play id before `execution_context` is moved into
    // `scoped_for_playbook` below -- it anchors `rule_id` (ADR-060 §3,
    // ADR-074; see module doc).
    let play_id = execution_context.source_playbook_id.clone();

    // Create a scoped NodeService that tags all mutations with the execution context.
    // This ensures events emitted by actions carry playbook_context for cycle detection.
    let scoped_service = Arc::new(node_service.scoped_for_playbook(execution_context));
    let graph_resolver = GraphResolver::new(Arc::clone(node_service));
    let mut ctx = BindingContext::new(trigger_node, event, Some(graph_resolver));
    let rule_id = rule_id_for(&play_id, actions);

    for (i, action) in actions.iter().enumerate() {
        if let Some(for_each_path) = &action.for_each {
            // ---------------------------------------------------------------
            // for_each execution
            // ---------------------------------------------------------------

            // Step 1: Resolve the full collection before iteration begins
            let collection = match ctx.resolve_binding(for_each_path).await {
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

                // This item's own real node id becomes the next
                // `iteration_path` element (ADR-074) -- never the loop
                // position, so two devices iterating this same set in a
                // different order still derive the same id per item.
                let item_node_id = match resolve_iteration_path_item_id(item) {
                    Ok(id) => id,
                    Err(message) => {
                        return ActionResult::Failed(ActionError::IterationPathResolutionFailed {
                            action_index: i,
                            item_index: item_idx,
                            message,
                        });
                    }
                };
                ctx.iteration_path.push(item_node_id);

                // Re-resolve params with the item binding available
                let item_params = match resolve_bindings_in_value(&action.params, &mut ctx).await {
                    Ok(p) => p,
                    Err(e) => {
                        ctx.iteration_path.pop();
                        return ActionResult::Failed(e);
                    }
                };

                let result = execute_single_action(
                    i,
                    &action.action_type,
                    &item_params,
                    &scoped_service,
                    &rule_id,
                    &ctx.iteration_path,
                )
                .await;
                ctx.iteration_path.pop();

                match result {
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
            let resolved_params = match resolve_bindings_in_value(&action.params, &mut ctx).await {
                Ok(p) => p,
                Err(e) => return ActionResult::Failed(e),
            };

            match execute_single_action(
                i,
                &action.action_type,
                &resolved_params,
                &scoped_service,
                &rule_id,
                &ctx.iteration_path,
            )
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
    rule_id: &str,
    iteration_path: &[String],
) -> Result<Value, ActionError> {
    match action_type {
        ActionType::CreateNode => {
            execute_create_node(action_index, params, node_service, rule_id, iteration_path).await
        }
        ActionType::UpdateNode => execute_update_node(action_index, params, node_service).await,
        ActionType::AddRelationship => {
            execute_add_relationship(action_index, params, node_service).await
        }
        ActionType::RemoveRelationship => {
            execute_remove_relationship(action_index, params, node_service).await
        }
    }
}

/// Execute a `create_node` action, deriving the output node's id from
/// `(rule_id, action_index, iteration_path)` instead of assigning a random
/// one (ADR-060 §3, ADR-074 -- see module doc for the full formula).
///
/// A second independent computation of this exact action -- this device
/// re-processing the same trigger, or another device converging via sync --
/// derives the SAME id. If a node already exists at that id, this is the
/// intended convergence outcome, not a failure: it is returned as-is rather
/// than attempting (and failing) a duplicate insert, which would otherwise
/// surface as `ActionResult::Failed` and disable the whole play (see
/// `rule_processor_loop` in `engine.rs`).
async fn execute_create_node(
    action_index: usize,
    params: &Value,
    node_service: &Arc<NodeService>,
    rule_id: &str,
    iteration_path: &[String],
) -> Result<Value, ActionError> {
    let node_type =
        params
            .get("node_type")
            .and_then(|v| v.as_str())
            .ok_or(ActionError::MissingParam {
                param: "node_type".to_string(),
                action_index,
            })?;
    let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let properties = params.get("properties").cloned().unwrap_or(json!({}));

    let node_id = deterministic_action_output_id(rule_id, action_index, iteration_path);

    if let Some(existing) =
        node_service
            .get_node(&node_id)
            .await
            .map_err(|e| ActionError::ServiceError {
                message: e.to_string(),
                action_index,
            })?
    {
        debug!(
            "action[{}] create_node converged onto existing node '{}'",
            action_index, node_id
        );
        return serde_json::to_value(&existing).map_err(|e| ActionError::ServiceError {
            message: e.to_string(),
            action_index,
        });
    }

    let node = Node::new_with_id(
        node_id.clone(),
        node_type.to_string(),
        content.to_string(),
        properties,
    );

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
    let node_id =
        params
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
    let relationship_type = params
        .get("relationship_type")
        .and_then(|v| v.as_str())
        .ok_or(ActionError::MissingParam {
            param: "relationship_type".to_string(),
            action_index,
        })?;
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
    let relationship_type = params
        .get("relationship_type")
        .and_then(|v| v.as_str())
        .ok_or(ActionError::MissingParam {
            param: "relationship_type".to_string(),
            action_index,
        })?;
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
            node: make_test_node(node_id, node_type),
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

    #[tokio::test]
    async fn test_resolve_trigger_node_id() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = ctx.resolve_binding("trigger.node.id").await.unwrap();
        assert_eq!(result, json!("node-123"));
    }

    #[tokio::test]
    async fn test_resolve_trigger_node_type() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = ctx.resolve_binding("trigger.node.nodeType").await.unwrap();
        assert_eq!(result, json!("task"));
    }

    #[tokio::test]
    async fn test_resolve_trigger_node_content() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = ctx.resolve_binding("trigger.node.content").await.unwrap();
        assert_eq!(result, json!("Test content"));
    }

    #[tokio::test]
    async fn test_resolve_trigger_node_nested_properties() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = ctx
            .resolve_binding("trigger.node.properties.task.status")
            .await
            .unwrap();
        assert_eq!(result, json!("open"));
    }

    #[tokio::test]
    async fn test_resolve_trigger_node_title() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = ctx.resolve_binding("trigger.node.title").await.unwrap();
        assert_eq!(result, json!("Test Node"));
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — trigger.property paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_trigger_property_key() {
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
        let mut ctx = BindingContext::new(&node, &event, None);

        assert_eq!(
            ctx.resolve_binding("trigger.property.key").await.unwrap(),
            json!("task.status")
        );
    }

    #[tokio::test]
    async fn test_resolve_trigger_property_old_value() {
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
        let mut ctx = BindingContext::new(&node, &event, None);

        assert_eq!(
            ctx.resolve_binding("trigger.property.old_value")
                .await
                .unwrap(),
            json!("open")
        );
    }

    #[tokio::test]
    async fn test_resolve_trigger_property_new_value() {
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
        let mut ctx = BindingContext::new(&node, &event, None);

        assert_eq!(
            ctx.resolve_binding("trigger.property.new_value")
                .await
                .unwrap(),
            json!("done")
        );
    }

    #[tokio::test]
    async fn test_resolve_trigger_property_null_old_value() {
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
        let mut ctx = BindingContext::new(&node, &event, None);

        assert_eq!(
            ctx.resolve_binding("trigger.property.old_value")
                .await
                .unwrap(),
            Value::Null
        );
    }

    #[tokio::test]
    async fn test_resolve_trigger_property_not_available_on_created() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx
            .resolve_binding("trigger.property.key")
            .await
            .unwrap_err();
        assert!(err.contains("not a PropertyChanged event"));
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — actions[N].result paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_action_result() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results
            .push(json!({"id": "new-node-456", "nodeType": "text"}));

        let result = ctx.resolve_binding("actions[0].result.id").await.unwrap();
        assert_eq!(result, json!("new-node-456"));
    }

    #[tokio::test]
    async fn test_resolve_action_result_no_bracket_syntax() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results.push(json!({"id": "abc"}));

        // Also works with just the number (without brackets)
        let result = ctx.resolve_binding("actions.0.result.id").await.unwrap();
        assert_eq!(result, json!("abc"));
    }

    #[tokio::test]
    async fn test_resolve_action_result_not_yet_available() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx
            .resolve_binding("actions[0].result.id")
            .await
            .unwrap_err();
        assert!(err.contains("has no result yet"));
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — item paths
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_item_path() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.current_item = Some(json!({"id": "item-1", "name": "First"}));

        assert_eq!(
            ctx.resolve_binding("item.id").await.unwrap(),
            json!("item-1")
        );
        assert_eq!(
            ctx.resolve_binding("item.name").await.unwrap(),
            json!("First")
        );
    }

    #[tokio::test]
    async fn test_resolve_item_not_in_loop() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx.resolve_binding("item.id").await.unwrap_err();
        assert!(err.contains("not in a for_each loop"));
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — error cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_unknown_root() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx.resolve_binding("unknown.field").await.unwrap_err();
        assert!(err.contains("unknown binding root"));
    }

    // -----------------------------------------------------------------------
    // resolve_bindings_in_string tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_single_binding_preserves_type_number() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        // version is a number, should be preserved as json number
        let result = resolve_bindings_in_string("{trigger.node.version}", &mut ctx)
            .await
            .unwrap();
        assert_eq!(result, json!(1));
    }

    #[tokio::test]
    async fn test_single_binding_preserves_type_string() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = resolve_bindings_in_string("{trigger.node.id}", &mut ctx)
            .await
            .unwrap();
        assert_eq!(result, json!("node-123"));
    }

    #[tokio::test]
    async fn test_single_binding_preserves_type_object() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = resolve_bindings_in_string("{trigger.node.properties.task}", &mut ctx)
            .await
            .unwrap();
        assert_eq!(result, json!({"status": "open", "priority": "high"}));
    }

    #[tokio::test]
    async fn test_mixed_text_and_bindings() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = resolve_bindings_in_string(
            "Node {trigger.node.id} is type {trigger.node.nodeType}",
            &mut ctx,
        )
        .await
        .unwrap();
        assert_eq!(result, json!("Node node-123 is type task"));
    }

    #[tokio::test]
    async fn test_no_bindings_returns_literal() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = resolve_bindings_in_string("just a plain string", &mut ctx)
            .await
            .unwrap();
        assert_eq!(result, json!("just a plain string"));
    }

    #[tokio::test]
    async fn test_binding_resolution_failed_error() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = resolve_bindings_in_string("{nonexistent.path}", &mut ctx)
            .await
            .unwrap_err();
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

    #[tokio::test]
    async fn test_resolve_bindings_in_value_object() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let params = json!({
            "node_type": "text",
            "content": "Created from {trigger.node.id}",
            "properties": {
                "source": "{trigger.node.id}"
            }
        });

        let resolved = resolve_bindings_in_value(&params, &mut ctx).await.unwrap();
        assert_eq!(resolved["content"], json!("Created from node-123"));
        assert_eq!(resolved["properties"]["source"], json!("node-123"));
        // node_type has no binding, preserved as-is
        assert_eq!(resolved["node_type"], json!("text"));
    }

    #[tokio::test]
    async fn test_resolve_bindings_in_value_array() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let params = json!(["{trigger.node.id}", "literal", "{trigger.node.nodeType}"]);

        let resolved = resolve_bindings_in_value(&params, &mut ctx).await.unwrap();
        assert_eq!(resolved[0], json!("node-123"));
        assert_eq!(resolved[1], json!("literal"));
        assert_eq!(resolved[2], json!("task"));
    }

    #[tokio::test]
    async fn test_resolve_bindings_in_value_non_string_passthrough() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let params = json!(42);
        let resolved = resolve_bindings_in_value(&params, &mut ctx).await.unwrap();
        assert_eq!(resolved, json!(42));

        let params = json!(true);
        let resolved = resolve_bindings_in_value(&params, &mut ctx).await.unwrap();
        assert_eq!(resolved, json!(true));

        let params = json!(null);
        let resolved = resolve_bindings_in_value(&params, &mut ctx).await.unwrap();
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

    #[test]
    fn test_action_error_display_iteration_path_resolution_failed() {
        let e = ActionError::IterationPathResolutionFailed {
            action_index: 1,
            item_index: 3,
            message: "no id field".to_string(),
        };
        let s = format!("{}", e);
        assert!(s.contains("action[1]"));
        assert!(s.contains("item[3]"));
        assert!(s.contains("no id field"));
    }

    // -----------------------------------------------------------------------
    // Derived identity — pure unit tests (ADR-060 §3, ADR-074)
    // -----------------------------------------------------------------------

    fn make_action(action_type: ActionType, params: Value, for_each: Option<&str>) -> ParsedAction {
        ParsedAction {
            action_type,
            params,
            for_each: for_each.map(|s| s.to_string()),
        }
    }

    #[test]
    fn deterministic_action_output_id_is_stable_across_calls() {
        let path = vec!["trigger-1".to_string()];
        let a = deterministic_action_output_id("rule-a", 0, &path);
        let b = deterministic_action_output_id("rule-a", 0, &path);
        assert_eq!(a, b, "same inputs must derive the same id every time");
    }

    #[test]
    fn deterministic_action_output_id_differs_by_action_index() {
        let path = vec!["trigger-1".to_string()];
        let a = deterministic_action_output_id("rule-a", 0, &path);
        let b = deterministic_action_output_id("rule-a", 1, &path);
        assert_ne!(
            a, b,
            "two actions on the same trigger must not collide onto one id"
        );
    }

    #[test]
    fn deterministic_action_output_id_differs_by_iteration_path_content() {
        let a = deterministic_action_output_id("rule-a", 0, &["item-1".to_string()]);
        let b = deterministic_action_output_id("rule-a", 0, &["item-2".to_string()]);
        assert_ne!(a, b);
    }

    #[test]
    fn deterministic_action_output_id_differs_by_iteration_path_order() {
        // Order encodes nesting depth (outer scan node, then inner item),
        // so swapping it must NOT be treated as the same path.
        let a = deterministic_action_output_id(
            "rule-a",
            0,
            &["cycle-1".to_string(), "issue-1".to_string()],
        );
        let b = deterministic_action_output_id(
            "rule-a",
            0,
            &["issue-1".to_string(), "cycle-1".to_string()],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn deterministic_action_output_id_differs_by_rule_id() {
        let path = vec!["trigger-1".to_string()];
        let a = deterministic_action_output_id("rule-a", 0, &path);
        let b = deterministic_action_output_id("rule-b", 0, &path);
        assert_ne!(
            a, b,
            "two different rules reacting to the same trigger must not collide"
        );
    }

    #[test]
    fn rule_id_for_is_stable_for_the_same_play_and_actions() {
        let actions = vec![make_action(
            ActionType::CreateNode,
            json!({"node_type": "text", "content": "hi"}),
            None,
        )];
        let a = rule_id_for("play-1", &actions);
        let b = rule_id_for("play-1", &actions);
        assert_eq!(a, b);
    }

    #[test]
    fn rule_id_for_differs_across_plays_with_identical_actions() {
        let actions = vec![make_action(
            ActionType::CreateNode,
            json!({"node_type": "text", "content": "hi"}),
            None,
        )];
        let a = rule_id_for("play-1", &actions);
        let b = rule_id_for("play-2", &actions);
        assert_ne!(
            a, b,
            "the same rule template installed under two different plays must not collide"
        );
    }

    /// A rule edit that changes action content, order, or nesting structure
    /// changes `rule_id` -- and therefore every id derived from it -- from
    /// that point on. This is an explicit, accepted trade-off (ADR-060 §3),
    /// not a bug: it is the mechanism that keeps two DIFFERENT rule
    /// revisions from silently colliding onto the same output id.
    #[test]
    fn rule_id_for_changes_when_the_rule_is_edited() {
        let original = vec![make_action(
            ActionType::CreateNode,
            json!({"node_type": "text", "content": "original"}),
            None,
        )];
        let edited_content = vec![make_action(
            ActionType::CreateNode,
            json!({"node_type": "text", "content": "edited"}),
            None,
        )];
        let reordered = vec![
            make_action(ActionType::CreateNode, json!({"node_type": "text"}), None),
            make_action(
                ActionType::UpdateNode,
                json!({"node_id": "{trigger.node.id}"}),
                None,
            ),
        ];
        let reordered_swapped = vec![
            make_action(
                ActionType::UpdateNode,
                json!({"node_id": "{trigger.node.id}"}),
                None,
            ),
            make_action(ActionType::CreateNode, json!({"node_type": "text"}), None),
        ];
        let nested = vec![make_action(
            ActionType::CreateNode,
            json!({"node_type": "text"}),
            Some("trigger.node.mentions"),
        )];

        let base = rule_id_for("play-1", &original);
        assert_ne!(base, rule_id_for("play-1", &edited_content));
        assert_ne!(
            rule_id_for("play-1", &reordered),
            rule_id_for("play-1", &reordered_swapped),
            "reordering actions within a rule must change its identity"
        );
        assert_ne!(
            base,
            rule_id_for("play-1", &nested),
            "adding for_each nesting must change the rule's identity"
        );
    }

    #[test]
    fn resolve_iteration_path_item_id_accepts_bare_string() {
        let item = json!("node-abc");
        assert_eq!(resolve_iteration_path_item_id(&item).unwrap(), "node-abc");
    }

    #[test]
    fn resolve_iteration_path_item_id_accepts_object_with_id() {
        let item = json!({"id": "node-xyz", "title": "Issue"});
        assert_eq!(resolve_iteration_path_item_id(&item).unwrap(), "node-xyz");
    }

    #[test]
    fn resolve_iteration_path_item_id_rejects_object_without_id() {
        let item = json!({"title": "Issue"});
        assert!(resolve_iteration_path_item_id(&item).is_err());
    }

    #[test]
    fn resolve_iteration_path_item_id_rejects_empty_string() {
        assert!(resolve_iteration_path_item_id(&json!("")).is_err());
    }

    #[test]
    fn resolve_iteration_path_item_id_rejects_scalar() {
        assert!(resolve_iteration_path_item_id(&json!(42)).is_err());
        assert!(resolve_iteration_path_item_id(&json!(null)).is_err());
        assert!(resolve_iteration_path_item_id(&json!(true)).is_err());
    }

    // -----------------------------------------------------------------------
    // Derived identity — integration tests against a real NodeService
    // (ADR-060 §3, ADR-074 acceptance criteria)
    // -----------------------------------------------------------------------

    mod derived_identity_integration {
        use super::*;
        use crate::db::events::PlaybookExecutionContext;
        use crate::db::SqliteStore;
        use crate::services::NodeService;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        fn make_trigger_node(id: &str, node_type: &str, properties: Value) -> Node {
            Node {
                id: id.to_string(),
                node_type: node_type.to_string(),
                content: format!("{id} content"),
                version: 1,
                created_at: Utc::now(),
                modified_at: Utc::now(),
                properties,
                mentions: vec![],
                mentioned_in: vec![],
                title: Some(format!("{id} title")),
                lifecycle_status: "active".to_string(),
            }
        }

        fn exec_ctx(play_id: &str) -> PlaybookExecutionContext {
            PlaybookExecutionContext {
                originating_event_id: uuid::Uuid::new_v4().to_string(),
                depth: 0,
                source_playbook_id: play_id.to_string(),
            }
        }

        /// AC: two independent executions of the same graph_event-triggered
        /// rule against the same trigger node produce the same derived id
        /// (depth-1 case), and converge to a single row rather than erroring
        /// or duplicating.
        #[tokio::test]
        async fn graph_event_depth_1_two_executions_converge_to_one_node() {
            let (svc, _tmp) = create_test_service().await;
            let trigger = make_trigger_node("task-1", "task", json!({}));
            let event = make_node_created_event("task-1", "task");
            let actions = vec![make_action(
                ActionType::CreateNode,
                json!({"node_type": "text", "content": "reminder"}),
                None,
            )];

            let expected_id = deterministic_action_output_id(
                &rule_id_for("play-1", &actions),
                0,
                &["task-1".to_string()],
            );

            let r1 = execute_actions(&actions, &trigger, &event, &svc, exec_ctx("play-1")).await;
            assert!(matches!(r1, ActionResult::Success), "{r1:?}");

            // Simulate a second, fully independent execution (e.g. a re-delivered
            // event, or another device converging via sync) against the same store.
            let r2 = execute_actions(&actions, &trigger, &event, &svc, exec_ctx("play-1")).await;
            assert!(matches!(r2, ActionResult::Success), "{r2:?}");

            let created = svc.get_node(&expected_id).await.unwrap();
            assert!(
                created.is_some(),
                "output node must exist at the derived id"
            );

            let all_text_nodes = svc
                .query_nodes_by_type("text", None)
                .await
                .unwrap()
                .into_iter()
                .filter(|n| n.lifecycle_status == "active")
                .count();
            assert_eq!(
                all_text_nodes, 1,
                "two executions must converge to exactly one node, not two"
            );
        }

        /// AC: two independent executions of a scheduled rule with a
        /// `for_each` over a scanned set of N items produce N distinct
        /// derived ids, one per item -- not one collapsed id for the batch.
        #[tokio::test]
        async fn scheduled_for_each_produces_one_distinct_id_per_item() {
            let (svc, _tmp) = create_test_service().await;
            // The scanned node the scheduled trigger matched (e.g. a cycle).
            let trigger = make_trigger_node(
                "cycle-1",
                "cycle",
                json!({
                    "items": [
                        {"id": "issue-1"},
                        {"id": "issue-2"},
                        {"id": "issue-3"}
                    ]
                }),
            );
            let event = make_node_created_event("cycle-1", "cycle");
            let actions = vec![make_action(
                ActionType::CreateNode,
                json!({"node_type": "text", "content": "migrated from {item.id}"}),
                Some("trigger.node.properties.items"),
            )];

            let result =
                execute_actions(&actions, &trigger, &event, &svc, exec_ctx("play-2")).await;
            assert!(matches!(result, ActionResult::Success), "{result:?}");

            let rule_id = rule_id_for("play-2", &actions);
            let mut expected_ids: Vec<String> = ["issue-1", "issue-2", "issue-3"]
                .iter()
                .map(|item_id| {
                    deterministic_action_output_id(
                        &rule_id,
                        0,
                        &["cycle-1".to_string(), item_id.to_string()],
                    )
                })
                .collect();
            expected_ids.sort();
            assert_eq!(
                expected_ids
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len(),
                3,
                "the three per-item ids must be distinct from each other"
            );

            for id in &expected_ids {
                assert!(
                    svc.get_node(id).await.unwrap().is_some(),
                    "expected a node at derived id {id}"
                );
            }

            let all_text_nodes = svc
                .query_nodes_by_type("text", None)
                .await
                .unwrap()
                .into_iter()
                .filter(|n| n.lifecycle_status == "active")
                .count();
            assert_eq!(
                all_text_nodes, 3,
                "for_each over 3 items must create 3 distinct nodes, not 1 collapsed node"
            );
        }

        /// AC: nested `for_each` (the outer scan/trigger level plus the
        /// inner `for_each` level) produces distinct ids per leaf item,
        /// correctly incorporating BOTH levels' real node ids -- not just
        /// the leaf. Proven by showing the SAME leaf item id under two
        /// DIFFERENT outer scan nodes derives two DIFFERENT output ids.
        #[tokio::test]
        async fn nested_for_each_incorporates_both_iteration_levels() {
            let (svc, _tmp) = create_test_service().await;
            let actions = vec![make_action(
                ActionType::CreateNode,
                json!({"node_type": "text", "content": "migrated from {item.id}"}),
                Some("trigger.node.properties.items"),
            )];

            let cycle_a = make_trigger_node(
                "cycle-a",
                "cycle",
                json!({"items": [{"id": "issue-shared"}]}),
            );
            let cycle_b = make_trigger_node(
                "cycle-b",
                "cycle",
                json!({"items": [{"id": "issue-shared"}]}),
            );

            let event_a = make_node_created_event("cycle-a", "cycle");
            let event_b = make_node_created_event("cycle-b", "cycle");

            let ra = execute_actions(&actions, &cycle_a, &event_a, &svc, exec_ctx("play-3")).await;
            let rb = execute_actions(&actions, &cycle_b, &event_b, &svc, exec_ctx("play-3")).await;
            assert!(matches!(ra, ActionResult::Success), "{ra:?}");
            assert!(matches!(rb, ActionResult::Success), "{rb:?}");

            let rule_id = rule_id_for("play-3", &actions);
            let id_under_a = deterministic_action_output_id(
                &rule_id,
                0,
                &["cycle-a".to_string(), "issue-shared".to_string()],
            );
            let id_under_b = deterministic_action_output_id(
                &rule_id,
                0,
                &["cycle-b".to_string(), "issue-shared".to_string()],
            );

            assert_ne!(
                id_under_a, id_under_b,
                "the same leaf item under two different outer scan nodes must derive different ids"
            );
            assert!(svc.get_node(&id_under_a).await.unwrap().is_some());
            assert!(svc.get_node(&id_under_b).await.unwrap().is_some());

            let all_text_nodes = svc
                .query_nodes_by_type("text", None)
                .await
                .unwrap()
                .into_iter()
                .filter(|n| n.lifecycle_status == "active")
                .count();
            assert_eq!(
                all_text_nodes, 2,
                "two distinct leaf nodes, one per outer scan node"
            );
        }

        /// AC: scan order differing across two simulated "devices" does not
        /// change the resulting SET of derived ids -- content-keyed, not
        /// position-keyed. Two independent stores ("devices") execute the
        /// same rule against the same trigger with the for_each collection
        /// in a different order; the sets of ids they each produce must match.
        #[tokio::test]
        async fn scan_order_does_not_affect_the_resulting_id_set() {
            let (svc_device_a, _tmp_a) = create_test_service().await;
            let (svc_device_b, _tmp_b) = create_test_service().await;

            let actions = vec![make_action(
                ActionType::CreateNode,
                json!({"node_type": "text", "content": "migrated from {item.id}"}),
                Some("trigger.node.properties.items"),
            )];
            let event = make_node_created_event("cycle-1", "cycle");

            // Device A scans in one order...
            let trigger_a = make_trigger_node(
                "cycle-1",
                "cycle",
                json!({"items": [{"id": "issue-1"}, {"id": "issue-2"}, {"id": "issue-3"}]}),
            );
            // ...device B scans the SAME set in a different order.
            let trigger_b = make_trigger_node(
                "cycle-1",
                "cycle",
                json!({"items": [{"id": "issue-3"}, {"id": "issue-1"}, {"id": "issue-2"}]}),
            );

            let ra = execute_actions(
                &actions,
                &trigger_a,
                &event,
                &svc_device_a,
                exec_ctx("play-4"),
            )
            .await;
            let rb = execute_actions(
                &actions,
                &trigger_b,
                &event,
                &svc_device_b,
                exec_ctx("play-4"),
            )
            .await;
            assert!(matches!(ra, ActionResult::Success), "{ra:?}");
            assert!(matches!(rb, ActionResult::Success), "{rb:?}");

            let mut ids_a: Vec<String> = svc_device_a
                .query_nodes_by_type("text", None)
                .await
                .unwrap()
                .into_iter()
                .map(|n| n.id)
                .collect();
            let mut ids_b: Vec<String> = svc_device_b
                .query_nodes_by_type("text", None)
                .await
                .unwrap()
                .into_iter()
                .map(|n| n.id)
                .collect();
            ids_a.sort();
            ids_b.sort();

            assert_eq!(ids_a.len(), 3);
            assert_eq!(
                ids_a, ids_b,
                "two devices scanning the same set in different orders must derive the same set of ids"
            );
        }

        /// A `for_each` item with no resolvable real node id (e.g. a plain
        /// scalar collection with nothing to key identity on) fails the rule
        /// rather than silently falling back to a scan-order-dependent
        /// positional index.
        #[tokio::test]
        async fn for_each_item_without_resolvable_id_fails_the_rule() {
            let (svc, _tmp) = create_test_service().await;
            let trigger = make_trigger_node("cycle-1", "cycle", json!({"items": [1, 2, 3]}));
            let event = make_node_created_event("cycle-1", "cycle");
            let actions = vec![make_action(
                ActionType::CreateNode,
                json!({"node_type": "text", "content": "x"}),
                Some("trigger.node.properties.items"),
            )];

            let result =
                execute_actions(&actions, &trigger, &event, &svc, exec_ctx("play-5")).await;
            match result {
                ActionResult::Failed(ActionError::IterationPathResolutionFailed { .. }) => {}
                other => panic!("expected IterationPathResolutionFailed, got {other:?}"),
            }
        }
    }
}
