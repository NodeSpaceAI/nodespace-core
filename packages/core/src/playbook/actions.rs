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
//! against the live graph state. A binding may also be a supported
//! function-call form, e.g. `{add_days(item.start_date, 14)}`, which
//! resolves its argument(s) through the same binding context and then
//! applies a fixed, explicitly-supported function (currently just
//! `add_days` -- see [`crate::playbook::cel::compute_add_days`]) -- this is
//! a scoped extension of the path-substitution scheme, not a general CEL
//! evaluator reachable from action values. See `BindingContext::resolve_binding`
//! and `parse_function_call` for the exact grammar and why it cannot change
//! any existing bare-`{path}` binding's resolution.
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
//! call site. Two consequences of THIS choice, neither one blessed by an
//! existing ADR -- they are named here because they are new, not because
//! they were already accepted elsewhere:
//! - ADR-060 §3 names one narrow trade-off: reordering actions shifts
//!   `action_index`, which changes the derived id of everything from the
//!   reorder point on. Hashing the WHOLE action list into one `rule_id` (as
//!   `rule_id_for` does) is a strictly broader trade-off than that: editing
//!   ANY single action's params -- content, order, or nesting, anywhere in
//!   the rule -- changes `rule_id`, and therefore changes the derived id of
//!   EVERY action in that rule, including untouched siblings. E.g. editing a
//!   notification action's message text silently changes a sibling
//!   `create_node` action's derived id too, even though that action's own
//!   params never changed. This is a real, sharper consequence than ADR-060
//!   §3 describes and is not something the ADR already accepted -- see
//!   [`rule_id_for`]'s own doc and the `rule_id_for_changes_when_the_rule_is_edited`
//!   test for what actually changes and why.
//! - Two DIFFERENT rules in the SAME play with byte-identical action lists
//!   collide onto the same `rule_id`. This is not narrow: the realistic
//!   trigger is copy-paste rule authoring (duplicate a rule, change its
//!   trigger/condition, leave the action list untouched), and the two rules
//!   need not even fire from the same event -- they only need to eventually
//!   act on the same node, e.g. rule A on `node_created` and rule B later on
//!   `property_changed` for that same node, landing on the same
//!   `iteration_path` and `action_index`. The failure is a SILENT INCORRECT
//!   MERGE, not a visible duplicate or error: `execute_create_node`'s
//!   existing-node check treats rule B's output as "already converged" and
//!   quietly returns rule A's node. See
//!   `two_rules_with_identical_actions_silently_share_one_output_node` for
//!   the behavior made explicit at the executor level -- that stays true by
//!   design, since `execute_actions` has no way to know a second rule is
//!   even involved. What such a play can no longer do is get SAVED in the
//!   first place: `playbook::validation::validate_no_duplicate_action_lists`
//!   rejects two same-play rules whose [`action_list_signature`] matches,
//!   naming both offending rules.
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

use crate::db::events::{DomainEvent, PlaybookExecutionContext, PLAYBOOK_CHAIN_DEPTH_PROPERTY};
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
    /// A `reject` action (ADR-060 §2) was reached and executed. Distinct from
    /// every other variant here: those describe something going WRONG
    /// (a binding failing to resolve, a service call erroring); this
    /// describes an invariant rule doing exactly what it was authored to do
    /// — deliberately vetoing the triggering write. Callers that need to
    /// distinguish "the rule rejected this write" from "the rule itself
    /// malfunctioned" (see `services::node_service::invariants`) match on
    /// this variant specifically rather than treating it as a generic
    /// execution failure.
    Rejected {
        message: String,
        action_index: usize,
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
            Self::Rejected {
                message,
                action_index,
            } => {
                write!(
                    f,
                    "action[{}] rejected the write: {}",
                    action_index, message
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

    /// Resolve a dot-path binding, or a supported function-call binding
    /// (e.g. `add_days(item.start_date, 14)`), against the context.
    ///
    /// Supported roots: `trigger`, `actions`, `item`. Supported functions:
    /// `add_days` (see [`Self::resolve_function_call`]).
    ///
    /// The function-call form is detected ONLY when the entire path is
    /// `name(...)` -- see [`parse_function_call`] for why that can never
    /// misfire on, or change the resolution of, an existing bare dot-path
    /// binding: `(` is not a legal character in any path segment a play
    /// author can write today.
    ///
    /// Handles both `actions[0].result.field` and `actions.0.result.field` formats.
    pub async fn resolve_binding(&mut self, path: &str) -> Result<Value, String> {
        if let Some((name, args)) = parse_function_call(path) {
            return self.resolve_function_call(name, args).await;
        }

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

    /// Resolve a function-call binding, e.g. `add_days(item.start_date, 14)`.
    ///
    /// This is a fixed, explicitly-supported function set matched by name
    /// below -- NOT a general dispatch mechanism. There is deliberately no
    /// registration table or lookup keyed by an action-param-supplied
    /// string; an unrecognized name is a hard, immediate error.
    async fn resolve_function_call(&mut self, name: &str, args: &str) -> Result<Value, String> {
        match name {
            "add_days" => self.resolve_add_days_call(args).await,
            other => Err(format!(
                "unknown function '{}' in binding (supported: add_days)",
                other
            )),
        }
    }

    /// `add_days(date, n)` -- resolves `date` and `n` through this same
    /// binding context, then applies
    /// [`crate::playbook::cel::compute_add_days`].
    ///
    /// `date` must be a dot-path (resolving to a string). `n` may be either
    /// a dot-path or a bare integer literal. Neither argument may itself be
    /// a function call -- nesting is rejected with a clear error rather than
    /// evaluated, keeping this a fixed one-level substitution rather than a
    /// general expression evaluator reachable through argument position.
    async fn resolve_add_days_call(&mut self, args: &str) -> Result<Value, String> {
        let parts = split_top_level_args(args);
        if parts.len() != 2 {
            return Err(format!(
                "add_days expects 2 arguments (date, days), got {}",
                parts.len()
            ));
        }
        let date_arg = parts[0].trim();
        let days_arg = parts[1].trim();

        if parse_function_call(date_arg).is_some() || parse_function_call(days_arg).is_some() {
            return Err("add_days does not support nested function-call arguments".to_string());
        }

        // Recursion is indirect (resolve_binding -> resolve_function_call ->
        // here -> resolve_binding) and therefore must be boxed, same as the
        // recursive calls in `resolve_bindings_in_value` below -- an async
        // fn calling itself, even indirectly, produces an infinitely-sized
        // future type unless one hop in the cycle is heap-indirected.
        let date_value = Box::pin(self.resolve_binding(date_arg)).await?;
        let date_str = date_value.as_str().ok_or_else(|| {
            format!(
                "add_days: first argument ('{}') did not resolve to a string date, got: {}",
                date_arg, date_value
            )
        })?;

        let days: i64 = if let Ok(n) = days_arg.parse::<i64>() {
            n
        } else {
            let days_value = Box::pin(self.resolve_binding(days_arg)).await?;
            days_value.as_i64().ok_or_else(|| {
                format!(
                    "add_days: second argument ('{}') did not resolve to an integer, got: {}",
                    days_arg, days_value
                )
            })?
        };

        crate::playbook::cel::compute_add_days(date_str, days)
            .map(Value::String)
            .map_err(|e| e.to_string())
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
    format!("{play_id}{}", action_list_signature(actions))
}

/// The action-list half of [`rule_id_for`]'s seed -- everything it hashes
/// EXCEPT `play_id`. Two rules whose action lists produce the same
/// signature are byte-identical in exactly the shape `rule_id_for` cares
/// about: the same `action_type`, `for_each`, and `params` for every action,
/// in order.
///
/// Encoded as a canonical JSON array of `[action_type, for_each, params]`
/// triples (one per action), rather than joining fields with a delimiter
/// character: `for_each` is parsed straight from user-authored play JSON
/// with no charset validation (unlike `conditions`, which must compile as
/// CEL), so a hand-picked delimiter is reachable by an ordinary play author
/// and would let a crafted `for_each` string (containing that exact
/// character) make one action list's fields bleed across the boundary into
/// the next, producing the same seed as an unrelated, differently-shaped
/// action list. JSON's own escaping is structural -- a string's content can
/// never be crafted to look like an array or string boundary -- so nesting
/// each action's fields in a `Value` and letting `serde_json` serialize the
/// whole array closes that off entirely. `Value`'s `Map` is also key-sorted
/// (see the module note on `preserve_order` not being enabled), so JSON key
/// order within `params` doesn't affect equality either.
///
/// `pub(crate)` so `playbook::validation`'s save-time check for two
/// same-play rules with byte-identical action lists
/// (`validate_no_duplicate_action_lists`) can compare rules without
/// duplicating this hashing logic.
pub(crate) fn action_list_signature(actions: &[ParsedAction]) -> String {
    let list: Vec<Value> = actions
        .iter()
        .map(|action| {
            Value::Array(vec![
                Value::String(action.action_type.as_str().to_string()),
                match &action.for_each {
                    Some(for_each) => Value::String(for_each.clone()),
                    None => Value::Null,
                },
                action.params.clone(),
            ])
        })
        .collect();
    Value::Array(list).to_string()
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
// Function-call binding syntax (e.g. `add_days(item.start_date, 14)`)
// ---------------------------------------------------------------------------
//
// A `{path}` binding may ALSO be a single supported function call, e.g.
// `{add_days(item.start_date, 14)}`. This is deliberately NOT a general
// function-registration framework: the two helpers below only ever
// recognize the shape "identifier(args)" and hand it to
// `BindingContext::resolve_function_call`'s fixed, hardcoded `match` --
// there is no way for an action param to reach anything beyond the
// explicitly-supported function set (currently just `add_days`).

/// Detect whether an entire binding path -- the text between a `{` `}` pair,
/// e.g. the `add_days(item.start_date, 14)` in
/// `"{add_days(item.start_date, 14)}"` -- is a function-call form rather
/// than a bare dot-path. Returns the function name and the raw (unsplit)
/// argument-list text when it is.
///
/// Deliberately conservative: the WHOLE trimmed path must be
/// `<identifier>(...)` with a balanced trailing `)`, where `<identifier>`
/// is `[A-Za-z_][A-Za-z0-9_]*`. This can never match, or change the
/// resolution of, any EXISTING bare dot-path binding: `(` is not a legal
/// character in a `trigger`/`actions`/`item` path segment a play author can
/// write today (`actions[N]` uses square brackets, not parens), so every
/// string this function recognizes as a function call was already a hard
/// error under the old bare-path-only resolver -- never a successfully
/// resolving binding whose behavior this could silently change.
fn parse_function_call(path: &str) -> Option<(&str, &str)> {
    let path = path.trim();
    let open = path.find('(')?;
    if !path.ends_with(')') {
        return None;
    }
    let name = &path[..open];
    let mut chars = name.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((name, &path[open + 1..path.len() - 1]))
}

/// Split a function-call's argument-list text on top-level commas,
/// respecting nested parentheses so that a nested call's own commas don't
/// fracture the outer argument list into a shape that could be misread as a
/// different, valid argument count. This never evaluates or recurses into
/// anything nested -- it only produces argument-text boundaries; detecting
/// and rejecting a nested function-call argument is the caller's job (see
/// `BindingContext::resolve_add_days_call`).
fn split_top_level_args(args: &str) -> Vec<&str> {
    if args.trim().is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0usize;
    for (i, ch) in args.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&args[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&args[start..]);
    parts
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
    // Extract the depth this rule execution runs at (already `parent_depth + 1`,
    // computed by `rule_processor_loop` before building this context) so
    // `execute_create_node`/`execute_update_node` can persist it onto any
    // node they produce (ADR-060 §5). This is the value a downstream device
    // must see if this chain hops across a sync boundary -- see
    // `PLAYBOOK_CHAIN_DEPTH_PROPERTY`'s doc in `db::events`.
    let depth = execution_context.depth;

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
                    depth,
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
                depth,
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
    depth: u8,
) -> Result<Value, ActionError> {
    match action_type {
        ActionType::CreateNode => {
            execute_create_node(
                action_index,
                params,
                node_service,
                rule_id,
                iteration_path,
                depth,
            )
            .await
        }
        ActionType::UpdateNode => {
            execute_update_node(action_index, params, node_service, depth).await
        }
        ActionType::AddRelationship => {
            execute_add_relationship(action_index, params, node_service).await
        }
        ActionType::RemoveRelationship => {
            execute_remove_relationship(action_index, params, node_service).await
        }
        ActionType::Reject => execute_reject(action_index, params),
    }
}

/// Execute a `reject` action (ADR-060 §2): deterministically fails with
/// [`ActionError::Rejected`], carrying the author-supplied `message` param
/// (already binding-resolved by the caller via `resolve_bindings_in_value`,
/// same as every other action's params). Performs no I/O and never succeeds
/// — reaching this function at all IS the rejection; the rule's `conditions`
/// are what gate whether it is reached, not anything in this executor. One
/// implementation shared by both the reactive (`execute_single_action`) and
/// transaction-scoped (`execute_single_action_in_tx`) executors below: unlike
/// `create_node`/`update_node`/relationship actions, `reject` touches no
/// store state, so there is nothing for a `_in_tx` twin to do differently.
fn execute_reject(action_index: usize, params: &Value) -> Result<Value, ActionError> {
    // `message` is normally a plain string, but when it's a single
    // `{binding}` template, `resolve_bindings_in_string`'s fast path
    // preserves the resolved value's own JSON type instead of stringifying
    // it (same as every other action's params) -- e.g. `"message":
    // "{trigger.node.priority}"` against a numeric `priority` resolves to a
    // JSON number, not a string. Accepting any scalar here (not just
    // `Value::String`) and rendering it the same way
    // `resolve_bindings_in_string`'s own mixed-text branch does keeps the
    // author's message intact instead of silently losing it to a
    // `MissingParam` for a binding that resolved successfully, just not to
    // a string.
    let message = match params.get("message") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => {
            return Err(ActionError::MissingParam {
                param: "message".to_string(),
                action_index,
            });
        }
        Some(other) => other.to_string(),
    };
    Err(ActionError::Rejected {
        message,
        action_index,
    })
}

/// Merge the current chain depth into an action's output properties
/// (ADR-060 §5).
///
/// Every node a play action creates or updates gets this stamp, regardless
/// of whether the rule's own `properties` param set anything else — an
/// `update_node` action that only changes `lifecycle_status`, for example,
/// still needs its depth recorded, since the classic runaway-chain shape is
/// a node repeatedly re-triggering itself through a non-`properties` field
/// just as easily as through one. Stored under
/// `PLAYBOOK_CHAIN_DEPTH_PROPERTY`, a `_`-prefixed key, so
/// `NodeService::normalize_flat_properties_to_namespace` keeps it at the
/// top level of `properties` independent of the node's type.
///
/// `properties` is expected to already be a JSON object (both call sites
/// pass either the resolved `properties` param or `json!({})`); a non-object
/// value is replaced with a fresh object carrying just the depth stamp
/// rather than silently dropping it.
fn stamp_chain_depth(properties: Value, depth: u8) -> Value {
    let mut obj = match properties {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    obj.insert(PLAYBOOK_CHAIN_DEPTH_PROPERTY.to_string(), json!(depth));
    Value::Object(obj)
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
    depth: u8,
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
    let properties = stamp_chain_depth(properties, depth);

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
    depth: u8,
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

    // Every node an update action touches carries the chain's current depth
    // (ADR-060 §5), independent of whether the action's own params set
    // `properties` at all -- see `stamp_chain_depth`'s doc. `NodeService`
    // deep-merges `update.properties` into the node's existing properties
    // rather than replacing them, so this never clobbers unrelated fields.
    update.properties = Some(stamp_chain_depth(
        update.properties.unwrap_or_else(|| json!({})),
        depth,
    ));

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
// Transaction-scoped action execution (ADR-060 §1)
// ---------------------------------------------------------------------------
//
// `execute_actions_in_tx` is the invariant-rule twin of `execute_actions`:
// same binding context, `for_each` iteration, and derived-identity logic
// (all read-only or pure with respect to the transaction, so reused
// unchanged), but every actual graph write goes through a `NodeServiceTx`
// `_in_tx` method instead of an ordinary `NodeService` method that would open
// (and commit) its own transaction. An invariant action's failure returns
// `ActionResult::Failed` exactly like the reactive path; the difference is
// entirely in what the CALLER does with that failure — `create_node_in_tx`
// propagates it as an `Err` that fails the whole enclosing transaction
// (fail-closed, ADR-060 §1), where `rule_processor_loop` instead disables the
// play and logs (fail-open, no rollback).
//
// Binding resolution (`resolve_bindings_in_value`, `GraphResolver`) reads via
// ordinary (non-tx) `NodeService` calls even here. This is safe: save-time
// eligibility (`playbook::validation`) restricts an invariant rule's actions
// to the trigger node and nodes it already references (ADR-060 §2's
// same-graph-scope rule), i.e. data that, if not the trigger node itself
// (served from the in-memory `trigger_node: &Node`, no read at all), already
// committed before this transaction began — a non-tx read sees it correctly.
// Only a WRITE targeting the trigger node itself needs tx-consistent reads,
// which is exactly what the `_in_tx` leaf executors below use.

/// Bundles the two "where to write" parameters every tx-scoped action
/// executor needs. Without this, `execute_single_action_in_tx` would take 8
/// positional arguments (clippy::too_many_arguments's limit is 7); bundling
/// `node_service` and `tx` -- always passed together, never independently --
/// brings every tx-scoped executor below that limit without hiding anything
/// behind a generic "context" grab-bag.
struct TxCtx<'a> {
    node_service: &'a Arc<NodeService>,
    tx: &'a crate::services::node_service::NodeServiceTx<'a>,
}

/// Tx-scoped twin of [`execute_actions`]. See the module section doc above.
pub(crate) async fn execute_actions_in_tx(
    actions: &[ParsedAction],
    trigger_node: &Node,
    event: &DomainEvent,
    node_service: &Arc<NodeService>,
    tx: &crate::services::node_service::NodeServiceTx<'_>,
    execution_context: PlaybookExecutionContext,
) -> ActionResult {
    let play_id = execution_context.source_playbook_id.clone();
    let depth = execution_context.depth;

    // Scoped so any buffered event these actions produce (flushed after this
    // transaction commits) carries `playbook_context` like a reactive
    // action's does -- `client_id` and everything else is preserved by
    // `scoped_for_playbook`'s clone-and-set-one-field shape.
    let scoped_service = Arc::new(node_service.scoped_for_playbook(execution_context));
    let txc = TxCtx {
        node_service: &scoped_service,
        tx,
    };
    let graph_resolver = GraphResolver::new(Arc::clone(node_service));
    let mut ctx = BindingContext::new(trigger_node, event, Some(graph_resolver));
    let rule_id = rule_id_for(&play_id, actions);

    for (i, action) in actions.iter().enumerate() {
        if let Some(for_each_path) = &action.for_each {
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
                "action[{}] for_each (in_tx) over {} items from '{}'",
                i,
                collection.len(),
                for_each_path,
            );

            for (item_idx, item) in collection.iter().enumerate() {
                ctx.current_item = Some(item.clone());

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

                let item_params = match resolve_bindings_in_value(&action.params, &mut ctx).await {
                    Ok(p) => p,
                    Err(e) => {
                        ctx.iteration_path.pop();
                        return ActionResult::Failed(e);
                    }
                };

                let result = execute_single_action_in_tx(
                    i,
                    &action.action_type,
                    &item_params,
                    &txc,
                    &rule_id,
                    &ctx.iteration_path,
                    depth,
                )
                .await;
                ctx.iteration_path.pop();

                if let Err(e) = result {
                    warn!(
                        "action[{}] for_each (in_tx) item[{}] failed, aborting rule: {}",
                        i, item_idx, e
                    );
                    return ActionResult::Failed(e);
                }
            }

            ctx.current_item = None;
            ctx.action_results.push(Value::Null);
        } else {
            let resolved_params = match resolve_bindings_in_value(&action.params, &mut ctx).await {
                Ok(p) => p,
                Err(e) => return ActionResult::Failed(e),
            };

            match execute_single_action_in_tx(
                i,
                &action.action_type,
                &resolved_params,
                &txc,
                &rule_id,
                &ctx.iteration_path,
                depth,
            )
            .await
            {
                Ok(result_value) => {
                    ctx.action_results.push(result_value);
                }
                Err(e) => {
                    warn!("action[{}] (in_tx) failed, aborting rule: {}", i, e);
                    return ActionResult::Failed(e);
                }
            }
        }
    }

    ActionResult::Success
}

async fn execute_single_action_in_tx(
    action_index: usize,
    action_type: &ActionType,
    params: &Value,
    txc: &TxCtx<'_>,
    rule_id: &str,
    iteration_path: &[String],
    depth: u8,
) -> Result<Value, ActionError> {
    match action_type {
        ActionType::CreateNode => {
            execute_create_node_in_tx(action_index, params, txc, rule_id, iteration_path, depth)
                .await
        }
        ActionType::UpdateNode => execute_update_node_in_tx(action_index, params, txc, depth).await,
        ActionType::AddRelationship => {
            execute_add_relationship_in_tx(action_index, params, txc).await
        }
        ActionType::RemoveRelationship => {
            execute_remove_relationship_in_tx(action_index, params, txc).await
        }
        ActionType::Reject => execute_reject(action_index, params),
    }
}

/// Tx-scoped twin of [`execute_create_node`]. Derived identity (ADR-060 §3,
/// ADR-074) applies identically: the existing-node check reads via
/// `SqliteStore::get_node_in_tx` (tx-consistent, unlike the ordinary path's
/// `NodeService::get_node`), so a duplicate `create_node` action within the
/// same transaction — or one this rule already produced earlier in the same
/// `for_each` iteration — converges onto the existing row instead of
/// attempting (and failing) a second insert at the same id.
async fn execute_create_node_in_tx(
    action_index: usize,
    params: &Value,
    txc: &TxCtx<'_>,
    rule_id: &str,
    iteration_path: &[String],
    depth: u8,
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
    let properties = stamp_chain_depth(properties, depth);

    let node_id = deterministic_action_output_id(rule_id, action_index, iteration_path);

    if let Some(existing) = crate::db::SqliteStore::get_node_in_tx(txc.tx.store_tx(), &node_id)
        .await
        .map_err(|e| ActionError::ServiceError {
            message: e.to_string(),
            action_index,
        })?
    {
        debug!(
            "action[{}] create_node (in_tx) converged onto existing node '{}'",
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

    // `insert_node_in_tx_no_invariant_dispatch`, NOT `create_node_in_tx`: an
    // invariant action's own `create_node` must not recurse into invariant
    // dispatch for whatever it just created — ADR-060 §2 requires invariant
    // rules to be non-chaining, depth 1, and this is what makes a DIFFERENT
    // rule's trigger matching this output impossible by construction rather
    // than relying on a runtime depth counter. See
    // `NodeService::create_node_in_tx`'s doc for the full reasoning.
    let created = txc
        .node_service
        .insert_node_in_tx_no_invariant_dispatch(txc.tx, node)
        .await
        .map_err(|e| ActionError::ServiceError {
            message: e.to_string(),
            action_index,
        })?;

    serde_json::to_value(&created).map_err(|e| ActionError::ServiceError {
        message: e.to_string(),
        action_index,
    })
}

async fn execute_update_node_in_tx(
    action_index: usize,
    params: &Value,
    txc: &TxCtx<'_>,
    depth: u8,
) -> Result<Value, ActionError> {
    let node_id =
        params
            .get("node_id")
            .and_then(|v| v.as_str())
            .ok_or(ActionError::MissingParam {
                param: "node_id".to_string(),
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

    update.properties = Some(stamp_chain_depth(
        update.properties.unwrap_or_else(|| json!({})),
        depth,
    ));

    let updated = txc
        .node_service
        .update_node_in_tx(txc.tx, node_id, update)
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

async fn execute_add_relationship_in_tx(
    action_index: usize,
    params: &Value,
    txc: &TxCtx<'_>,
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

    txc.node_service
        .create_relationship_in_tx(txc.tx, source_id, relationship_type, target_id, edge_data)
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

async fn execute_remove_relationship_in_tx(
    action_index: usize,
    params: &Value,
    txc: &TxCtx<'_>,
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

    txc.node_service
        .remove_relationship_in_tx(txc.tx, source_id, relationship_type, target_id)
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

    /// Helper: like [`make_test_node`], but with caller-supplied properties
    /// -- used by the `add_days` binding tests, which need date-shaped
    /// fields `make_test_node`'s fixed `status`/`priority` shape doesn't have.
    fn make_test_node_with_properties(id: &str, node_type: &str, properties: Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            content: "Test content".to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties,
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
    // execute_reject (ADR-060 §2)
    // -----------------------------------------------------------------------

    #[test]
    fn execute_reject_returns_rejected_with_the_message() {
        let params = json!({ "message": "cannot close while children are open" });
        let err = execute_reject(0, &params).unwrap_err();
        match err {
            ActionError::Rejected {
                message,
                action_index,
            } => {
                assert_eq!(message, "cannot close while children are open");
                assert_eq!(action_index, 0);
            }
            other => panic!("expected Rejected, got {:?}", other),
        }
    }

    #[test]
    fn execute_reject_never_returns_ok() {
        // Reaching this executor at all IS the rejection — there is no
        // success path, unlike every other action executor.
        let params = json!({ "message": "no" });
        assert!(execute_reject(0, &params).is_err());
    }

    #[test]
    fn execute_reject_without_message_is_missing_param_not_rejected() {
        // Defensive: save-time validation (`playbook::validation`) already
        // requires `message`, but a raw executor must still handle its
        // absence explicitly rather than panicking or fabricating a message
        // — and it must be reported as `MissingParam`, not silently treated
        // as a (message-less) rejection.
        let params = json!({});
        let err = execute_reject(0, &params).unwrap_err();
        assert!(matches!(
            err,
            ActionError::MissingParam { param, .. } if param == "message"
        ));
    }

    #[test]
    fn execute_reject_accepts_a_message_binding_that_resolves_to_a_non_string() {
        // When `message` is a single `{binding}` template,
        // `resolve_bindings_in_string`'s fast path preserves the resolved
        // value's own JSON type rather than stringifying it -- a param like
        // `"message": "{trigger.node.priority}"` against a numeric priority
        // resolves to `Value::Number`, not `Value::String`, before this
        // executor ever sees it. It must still be treated as a genuine
        // rejection (the message rendered as text), not misclassified as a
        // missing param -- losing the author's message and reporting the
        // wrong error kind to the caller.
        let params = json!({ "message": 5 });
        let err = execute_reject(0, &params).unwrap_err();
        match err {
            ActionError::Rejected { message, .. } => assert_eq!(message, "5"),
            other => panic!("expected Rejected, got {:?}", other),
        }
    }

    #[test]
    fn execute_reject_treats_null_message_as_missing_not_rejected() {
        // `Value::Null` is the resolved shape of a binding to a genuinely
        // absent/undefined value -- treated the same as the param being
        // entirely absent, not stringified to the literal text "null".
        let params = json!({ "message": null });
        let err = execute_reject(0, &params).unwrap_err();
        assert!(matches!(
            err,
            ActionError::MissingParam { param, .. } if param == "message"
        ));
    }

    #[tokio::test]
    async fn execute_actions_short_circuits_on_reject_before_a_later_action() {
        // Reject as action[0] of a two-action list: action[1] must never run
        // — proven here by action_results only ever growing on success, so a
        // `Failed` result with no way to observe action[1]'s effects is the
        // whole point of this test at the `execute_actions` entry-point
        // level (the loop-level short-circuit, not just `execute_reject`'s
        // own return value in isolation).
        let actions = vec![
            ParsedAction {
                action_type: ActionType::Reject,
                params: json!({ "message": "vetoed" }),
                for_each: None,
            },
            ParsedAction {
                action_type: ActionType::CreateNode,
                params: json!({ "node_type": "text", "content": "should never be created" }),
                for_each: None,
            },
        ];

        let trigger = make_test_node("trigger-1", "task");
        let event = make_node_created_event("trigger-1", "task");

        let temp_dir = tempfile::TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let mut store: Arc<crate::db::SqliteStore> =
            Arc::new(crate::db::SqliteStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());

        let execution_context = PlaybookExecutionContext {
            originating_event_id: "evt-1".to_string(),
            depth: 0,
            source_playbook_id: "play-1".to_string(),
        };

        let result =
            execute_actions(&actions, &trigger, &event, &node_service, execution_context).await;

        match result {
            ActionResult::Failed(ActionError::Rejected { message, .. }) => {
                assert_eq!(message, "vetoed");
            }
            other => panic!("expected Failed(Rejected), got {:?}", other),
        }
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
    // parse_function_call / split_top_level_args (pure parsing) tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_function_call_recognizes_a_simple_call() {
        let (name, args) = parse_function_call("add_days(item.start_date, 14)").unwrap();
        assert_eq!(name, "add_days");
        assert_eq!(args, "item.start_date, 14");
    }

    #[test]
    fn parse_function_call_trims_surrounding_whitespace() {
        let (name, args) = parse_function_call("  add_days(item.start_date, 14)  ").unwrap();
        assert_eq!(name, "add_days");
        assert_eq!(args, "item.start_date, 14");
    }

    #[test]
    fn parse_function_call_returns_none_for_ordinary_dot_paths() {
        // Every existing bare-path shape must NOT be misread as a function
        // call -- `(` never appears in any of these today.
        for path in [
            "trigger.node.id",
            "trigger.node.properties.task.status",
            "item.name",
            "actions[0].result.id",
            "actions.0.result.id",
            "",
        ] {
            assert!(
                parse_function_call(path).is_none(),
                "expected None for bare path '{path}'"
            );
        }
    }

    #[test]
    fn parse_function_call_rejects_unterminated_or_malformed_forms() {
        for path in [
            "add_days(item.start_date, 14",   // missing close paren
            "(item.start_date, 14)",          // empty function name
            "2add_days(item.start_date, 14)", // name starts with a digit
            "add days(item.start_date, 14)",  // space in name
        ] {
            assert!(
                parse_function_call(path).is_none(),
                "expected None for malformed form '{path}'"
            );
        }
    }

    #[test]
    fn split_top_level_args_splits_simple_args() {
        assert_eq!(
            split_top_level_args("item.start_date, 14"),
            vec!["item.start_date", " 14"]
        );
    }

    #[test]
    fn split_top_level_args_respects_nested_parens() {
        // A nested call's own comma must not fracture the outer argument
        // list -- this is what lets `resolve_add_days_call` reliably detect
        // "exactly 2 arguments, one of which is itself a function call" and
        // reject it with a clear message, rather than silently
        // misinterpreting arg count.
        assert_eq!(
            split_top_level_args("add_days(item.start_date, 1), 2"),
            vec!["add_days(item.start_date, 1)", " 2"]
        );
    }

    #[test]
    fn split_top_level_args_empty_input_is_empty_vec() {
        assert!(split_top_level_args("").is_empty());
        assert!(split_top_level_args("   ").is_empty());
    }

    // -----------------------------------------------------------------------
    // add_days(...) function-call binding tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn add_days_binding_resolves_path_arg_and_integer_literal() {
        let node = make_test_node_with_properties(
            "node-123",
            "cycle",
            json!({ "cycle": { "start_date": "2026-01-01" } }),
        );
        let event = make_node_created_event("node-123", "cycle");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = ctx
            .resolve_binding("add_days(trigger.node.properties.cycle.start_date, 14)")
            .await
            .unwrap();
        assert_eq!(result, json!("2026-01-15"));
    }

    #[tokio::test]
    async fn add_days_binding_resolves_days_arg_from_a_path() {
        let node = make_test_node_with_properties(
            "node-123",
            "cycle",
            json!({ "cycle": { "start_date": "2026-01-01" } }),
        );
        let event = make_node_created_event("node-123", "cycle");
        let mut ctx = BindingContext::new(&node, &event, None);
        ctx.current_item = Some(json!({ "duration_days": 30 }));

        let result = ctx
            .resolve_binding(
                "add_days(trigger.node.properties.cycle.start_date, item.duration_days)",
            )
            .await
            .unwrap();
        assert_eq!(result, json!("2026-01-31"));
    }

    #[tokio::test]
    async fn add_days_binding_supports_negative_offset() {
        let node = make_test_node_with_properties(
            "node-123",
            "cycle",
            json!({ "cycle": { "start_date": "2026-01-15" } }),
        );
        let event = make_node_created_event("node-123", "cycle");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = ctx
            .resolve_binding("add_days(trigger.node.properties.cycle.start_date, -14)")
            .await
            .unwrap();
        assert_eq!(result, json!("2026-01-01"));
    }

    #[tokio::test]
    async fn add_days_binding_full_curly_brace_form_preserves_string_type() {
        // The fast path in `resolve_bindings_in_string` (entire string is a
        // single `{...}`) must return the resolved value with its own JSON
        // type, exactly like a bare `{path}` binding does today.
        let node = make_test_node_with_properties(
            "node-123",
            "cycle",
            json!({ "cycle": { "start_date": "2026-01-01" } }),
        );
        let event = make_node_created_event("node-123", "cycle");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = resolve_bindings_in_string(
            "{add_days(trigger.node.properties.cycle.start_date, 14)}",
            &mut ctx,
        )
        .await
        .unwrap();
        assert_eq!(result, json!("2026-01-15"));
    }

    #[tokio::test]
    async fn add_days_binding_mixed_with_literal_text_is_interpolated() {
        let node = make_test_node_with_properties(
            "node-123",
            "cycle",
            json!({ "cycle": { "start_date": "2026-01-01" } }),
        );
        let event = make_node_created_event("node-123", "cycle");
        let mut ctx = BindingContext::new(&node, &event, None);

        let result = resolve_bindings_in_string(
            "End date: {add_days(trigger.node.properties.cycle.start_date, 14)}",
            &mut ctx,
        )
        .await
        .unwrap();
        assert_eq!(result, json!("End date: 2026-01-15"));
    }

    #[tokio::test]
    async fn add_days_binding_wrong_arg_count_is_a_clean_error() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx
            .resolve_binding("add_days(trigger.node.id)")
            .await
            .unwrap_err();
        assert!(err.contains("expects 2 arguments"), "got: {err}");
    }

    #[tokio::test]
    async fn add_days_binding_unresolvable_first_arg_is_a_clean_error_not_a_panic() {
        // Mirrors the issue's own malformed-input example:
        // `{add_days(bad.path, "not a number")}` -- neither a broken first
        // argument nor a non-numeric second argument may panic; both must
        // surface as an ordinary `Err`.
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx
            .resolve_binding("add_days(bad.path, \"not a number\")")
            .await
            .unwrap_err();
        assert!(err.contains("unknown binding root: 'bad'"), "got: {err}");
    }

    #[tokio::test]
    async fn add_days_binding_non_numeric_days_arg_is_a_clean_error_not_a_panic() {
        let node = make_test_node_with_properties(
            "node-123",
            "cycle",
            json!({ "cycle": { "start_date": "2026-01-01" } }),
        );
        let event = make_node_created_event("node-123", "cycle");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx
            .resolve_binding("add_days(trigger.node.properties.cycle.start_date, \"not a number\")")
            .await
            .unwrap_err();
        // "not a number" fails i64::parse, then is tried as a dot-path and
        // fails there too (not a known binding root) -- a clean Err either
        // way, never a panic.
        assert!(err.contains("unknown binding root"), "got: {err}");
    }

    #[tokio::test]
    async fn add_days_binding_date_arg_resolving_to_a_non_string_is_a_clean_error() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        // `trigger.node.version` resolves to a number (1), not a string.
        let err = ctx
            .resolve_binding("add_days(trigger.node.version, 5)")
            .await
            .unwrap_err();
        assert!(
            err.contains("did not resolve to a string date"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn add_days_binding_invalid_date_string_is_a_clean_error() {
        let node = make_test_node_with_properties(
            "node-123",
            "cycle",
            json!({ "cycle": { "start_date": "not-a-date" } }),
        );
        let event = make_node_created_event("node-123", "cycle");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx
            .resolve_binding("add_days(trigger.node.properties.cycle.start_date, 5)")
            .await
            .unwrap_err();
        assert!(err.contains("invalid date string"), "got: {err}");
    }

    #[tokio::test]
    async fn add_days_binding_rejects_a_nested_function_call_argument() {
        let node = make_test_node_with_properties(
            "node-123",
            "cycle",
            json!({ "cycle": { "start_date": "2026-01-01" } }),
        );
        let event = make_node_created_event("node-123", "cycle");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx
            .resolve_binding("add_days(add_days(trigger.node.properties.cycle.start_date, 1), 2)")
            .await
            .unwrap_err();
        assert!(
            err.contains("does not support nested function-call arguments"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn unsupported_function_name_is_a_clean_error_not_a_silent_passthrough() {
        // Proves the function-call surface is a fixed, explicitly-supported
        // set -- not an arbitrary-dispatch mechanism reachable by any
        // identifier a play author writes.
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let err = ctx
            .resolve_binding("delete_everything(trigger.node.id)")
            .await
            .unwrap_err();
        assert!(
            err.contains("unknown function 'delete_everything'") && err.contains("add_days"),
            "got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // Regression: existing {path} / bare-path behavior is unaffected
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn existing_bracket_action_path_unaffected_by_function_call_parsing() {
        // "actions[0].result.id" contains neither the function-call shape
        // NOR a top-level '(' -- confirms the new early-return in
        // `resolve_binding` is a true no-op for this path shape, which is
        // the one existing form most visually adjacent to a function call.
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);
        ctx.action_results.push(json!({"id": "abc"}));

        assert_eq!(
            ctx.resolve_binding("actions[0].result.id").await.unwrap(),
            json!("abc")
        );
    }

    #[tokio::test]
    async fn existing_plain_path_bindings_are_byte_identical_after_the_change() {
        // A representative sweep of every existing binding root/shape,
        // asserting the SAME outcomes the pre-existing tests above already
        // pin -- explicit regression coverage that add_days support changed
        // nothing about ordinary `{path}` resolution.
        let node = make_test_node("node-123", "task");
        let event = make_property_changed_event(
            "node-123",
            "task",
            vec![PropertyChange {
                key: "status".to_string(),
                old_value: Some(json!("open")),
                new_value: Some(json!("done")),
            }],
        );
        let mut ctx = BindingContext::new(&node, &event, None);
        ctx.action_results.push(json!({"id": "abc"}));
        ctx.current_item = Some(json!({"id": "item-1", "name": "First"}));

        assert_eq!(
            ctx.resolve_binding("trigger.node.id").await.unwrap(),
            json!("node-123")
        );
        assert_eq!(
            ctx.resolve_binding("trigger.property.old_value")
                .await
                .unwrap(),
            json!("open")
        );
        assert_eq!(
            ctx.resolve_binding("actions[0].result.id").await.unwrap(),
            json!("abc")
        );
        assert_eq!(
            ctx.resolve_binding("actions.0.result.id").await.unwrap(),
            json!("abc")
        );
        assert_eq!(
            ctx.resolve_binding("item.name").await.unwrap(),
            json!("First")
        );
        assert_eq!(
            resolve_bindings_in_string(
                "Node {trigger.node.id} is type {trigger.node.nodeType}",
                &mut ctx
            )
            .await
            .unwrap(),
            json!("Node node-123 is type task")
        );
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

    /// Regression: `action_list_signature` used to join each action's
    /// `action_type`/`for_each`/`params` fields with a raw `'\u{1}'`
    /// separator. `for_each` is parsed straight from user-authored play
    /// JSON with no charset validation, so an ordinary play author could
    /// set it to a string containing that exact character and make an
    /// unrelated, differently-shaped action list produce the same seed --
    /// a genuine collision, not a contrived one. The fix nests each
    /// action's fields in a JSON array instead, whose escaping is
    /// structural rather than delimiter-based.
    #[test]
    fn action_list_signature_is_not_confused_by_control_characters_in_for_each() {
        let two_actions = vec![
            make_action(ActionType::CreateNode, json!("X"), None),
            make_action(ActionType::CreateNode, Value::Null, None),
        ];
        // Under the old delimiter-joined seed, this single action's
        // `for_each` -- crafted to contain the exact bytes the two actions
        // above would have produced around the `action_type`/`params`
        // boundary -- collided with `two_actions`'s seed.
        let one_action_with_crafted_for_each = vec![make_action(
            ActionType::CreateNode,
            Value::Null,
            Some("\u{1}\"X\"\u{1}create_node\u{1}"),
        )];

        assert_ne!(
            action_list_signature(&two_actions),
            action_list_signature(&one_action_with_crafted_for_each),
            "a for_each value must never make an unrelated action list collide"
        );
    }

    /// A rule edit that changes action content, order, or nesting structure
    /// changes `rule_id` -- and therefore every id derived from it, for
    /// EVERY action in the rule, including ones the edit didn't touch --
    /// from that point on. This is a deliberate consequence of hashing the
    /// whole action list into one `rule_id` (see the module doc), not a bug:
    /// it is the mechanism that keeps two DIFFERENT rule revisions from
    /// silently colliding onto the same output id. It is broader than what
    /// ADR-060 §3 itself describes (reordering shifting `action_index`) --
    /// see the module doc for why this implementation's own trade-off is
    /// wider than the ADR's.
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

        /// KNOWN GAP, made explicit rather than left implicit (see the module
        /// doc's "rule_id" section and the tracked follow-up for a save-time
        /// `validation.rs` guard): two DIFFERENT rules in the SAME play with
        /// byte-identical action lists derive the SAME `rule_id`
        /// (`rule_id_for` hashes only `(play_id, actions)`, which can't tell
        /// two such rules apart). If they later act on the same real node --
        /// realistic via copy-paste rule authoring, e.g. rule A on
        /// `node_created` and rule B on `property_changed` for that same
        /// node -- they derive the SAME output id too.
        ///
        /// This test proves the resulting failure mode is a SILENT INCORRECT
        /// MERGE, not a duplicate or an error: rule B's `create_node` never
        /// actually runs -- `execute_create_node`'s existing-node-at-derived-id
        /// check (built for the legitimate case of the SAME rule re-firing)
        /// can't distinguish that from a different rule colliding, so it
        /// quietly hands back rule A's node as though it were rule B's own
        /// converged output. Nothing here signals that two distinct rules
        /// were involved.
        #[tokio::test]
        async fn two_rules_with_identical_actions_silently_share_one_output_node() {
            let (svc, _tmp) = create_test_service().await;
            let trigger = make_trigger_node("task-1", "task", json!({}));

            // Rule A and rule B are authored independently (e.g. rule B is a
            // copy-paste of rule A with only the trigger changed) but end up
            // with byte-identical action lists.
            let rule_a_actions = vec![make_action(
                ActionType::CreateNode,
                json!({"node_type": "text", "content": "rule A output"}),
                None,
            )];
            let rule_b_actions = vec![make_action(
                ActionType::CreateNode,
                json!({"node_type": "text", "content": "rule A output"}),
                None,
            )];

            // Root cause, asserted directly: same play, byte-identical
            // actions -> same rule_id -> same derived output id, even though
            // these are conceptually two different rules.
            let rule_a_id = rule_id_for("play-collision", &rule_a_actions);
            let rule_b_id = rule_id_for("play-collision", &rule_b_actions);
            assert_eq!(
                rule_a_id, rule_b_id,
                "byte-identical action lists in the same play are indistinguishable to rule_id_for"
            );

            // Rule A fires first, on node_created.
            let event_a = make_node_created_event("task-1", "task");
            let result_a = execute_actions(
                &rule_a_actions,
                &trigger,
                &event_a,
                &svc,
                exec_ctx("play-collision"),
            )
            .await;
            assert!(matches!(result_a, ActionResult::Success), "{result_a:?}");

            let all_after_a = svc.query_nodes_by_type("text", None).await.unwrap();
            assert_eq!(all_after_a.len(), 1, "rule A creates exactly one node");
            let rule_a_node_id = all_after_a[0].id.clone();

            // Rule B fires later, on property_changed for the SAME node --
            // a different event, a different (hypothetical) rule, but the
            // same trigger node and byte-identical actions.
            let event_b = make_property_changed_event(
                "task-1",
                "task",
                vec![PropertyChange {
                    key: "task.status".to_string(),
                    old_value: Some(json!("open")),
                    new_value: Some(json!("done")),
                }],
            );
            let result_b = execute_actions(
                &rule_b_actions,
                &trigger,
                &event_b,
                &svc,
                exec_ctx("play-collision"),
            )
            .await;

            // No error, no duplicate -- this is the silent part.
            assert!(
                matches!(result_b, ActionResult::Success),
                "rule B's execution reports success, masking that it wrote nothing of its own: {result_b:?}"
            );

            let all_after_b = svc.query_nodes_by_type("text", None).await.unwrap();
            assert_eq!(
                all_after_b.len(),
                1,
                "still exactly one node -- rule B silently 'converged' onto rule A's node \
                 instead of producing its own"
            );
            assert_eq!(
                all_after_b[0].id, rule_a_node_id,
                "the single node is rule A's, not a merge of both rules' intent"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Persisted chain depth (ADR-060 §5) — integration tests against a real
    // NodeService
    // -----------------------------------------------------------------------

    mod chain_depth_persistence_integration {
        use super::*;
        use crate::db::events::{PlaybookExecutionContext, PLAYBOOK_CHAIN_DEPTH_PROPERTY};
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

        fn exec_ctx(play_id: &str, depth: u8) -> PlaybookExecutionContext {
            PlaybookExecutionContext {
                originating_event_id: uuid::Uuid::new_v4().to_string(),
                depth,
                source_playbook_id: play_id.to_string(),
            }
        }

        /// AC: a node produced by `create_node` persists the execution
        /// context's depth under the reserved `_playbookChainDepth`
        /// property, matching the chain's actual depth at the moment the
        /// action ran -- not 0, not 1, whatever depth this rule fired at.
        #[tokio::test]
        async fn create_node_action_persists_chain_depth() {
            let (svc, _tmp) = create_test_service().await;
            let trigger = make_trigger_node("task-1", "task", json!({}));
            let event = make_node_created_event("task-1", "task");
            let actions = vec![make_action(
                ActionType::CreateNode,
                json!({"node_type": "text", "content": "reminder"}),
                None,
            )];

            let result =
                execute_actions(&actions, &trigger, &event, &svc, exec_ctx("play-1", 5)).await;
            assert!(matches!(result, ActionResult::Success), "{result:?}");

            let expected_id = deterministic_action_output_id(
                &rule_id_for("play-1", &actions),
                0,
                &["task-1".to_string()],
            );
            let created = svc
                .get_node(&expected_id)
                .await
                .unwrap()
                .expect("output node must exist at the derived id");
            assert_eq!(
                created.properties[PLAYBOOK_CHAIN_DEPTH_PROPERTY],
                json!(5),
                "created node must carry the execution context's depth"
            );
        }

        /// AC: `update_node` stamps the same depth onto the node it
        /// updates, merging it alongside whatever else the action changed
        /// rather than replacing that node's other properties.
        #[tokio::test]
        async fn update_node_action_persists_chain_depth_and_preserves_other_properties() {
            let (svc, _tmp) = create_test_service().await;

            let existing = Node::new_with_id(
                "node:target-1".to_string(),
                "text".to_string(),
                "original".to_string(),
                json!({"text": {"tag": "keep-me"}}),
            );
            svc.create_node(existing).await.unwrap();

            let trigger = make_trigger_node("task-1", "task", json!({}));
            let event = make_node_created_event("task-1", "task");
            let actions = vec![make_action(
                ActionType::UpdateNode,
                json!({"node_id": "node:target-1", "content": "updated"}),
                None,
            )];

            let result =
                execute_actions(&actions, &trigger, &event, &svc, exec_ctx("play-1", 3)).await;
            assert!(matches!(result, ActionResult::Success), "{result:?}");

            let updated = svc.get_node("node:target-1").await.unwrap().unwrap();
            assert_eq!(
                updated.properties[PLAYBOOK_CHAIN_DEPTH_PROPERTY],
                json!(3),
                "update_node must stamp the chain depth even though the action's own params never set `properties`"
            );
            assert_eq!(
                updated.properties["text"]["tag"],
                json!("keep-me"),
                "the depth stamp must merge in, not replace, the node's existing properties"
            );
            assert_eq!(updated.content, "updated");
        }

        /// AC (device-hop half): when this same device later processes a
        /// FURTHER hop against a node that already carries a persisted
        /// depth -- e.g. re-processing after a prior device's write synced
        /// in -- the new stamp reflects the execution context's depth for
        /// THIS hop, overwriting the older value rather than leaving both
        /// around or silently ignoring the new one.
        #[tokio::test]
        async fn create_node_action_overwrites_a_previously_persisted_depth() {
            let (svc, _tmp) = create_test_service().await;
            let trigger = make_trigger_node("task-1", "task", json!({}));
            let event = make_node_created_event("task-1", "task");
            let actions = vec![make_action(
                ActionType::CreateNode,
                json!({
                    "node_type": "text",
                    "content": "reminder",
                    // Simulates params that (unusually) already carry a
                    // stale depth value from elsewhere -- the action
                    // executor's own stamp must win. `json!` treats a bare
                    // key as a string literal, not a variable reference, so
                    // the constant must be parenthesized to be used as a key.
                    "properties": {(PLAYBOOK_CHAIN_DEPTH_PROPERTY): 1}
                }),
                None,
            )];

            let result =
                execute_actions(&actions, &trigger, &event, &svc, exec_ctx("play-1", 8)).await;
            assert!(matches!(result, ActionResult::Success), "{result:?}");

            let expected_id = deterministic_action_output_id(
                &rule_id_for("play-1", &actions),
                0,
                &["task-1".to_string()],
            );
            let created = svc.get_node(&expected_id).await.unwrap().unwrap();
            assert_eq!(created.properties[PLAYBOOK_CHAIN_DEPTH_PROPERTY], json!(8));
        }
    }
}
