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
//! Two binding-call forms reduce a resolved collection to a single value
//! instead of navigating to one: `sum(<collection-path>, <field>)` and
//! `count(<collection-path>)` (see [`parse_aggregate_call`]) — the
//! `for_each`-shaped collection resolution `for_each` itself uses, but
//! collapsed to one aggregate instead of iterated. Recognized by
//! `BindingContext::resolve_binding` ahead of its normal dot-path dispatch.
//! Usable anywhere any other `{binding}` is (an `update_node` action's
//! `properties`, a `create_node`'s, nested inside a `for_each` item's own
//! params, ...) — writing the aggregate's result is the existing
//! `update_node`/`create_node` action-writing mechanism, not a new one.
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

    /// Resolve a dot-path binding against the context.
    ///
    /// Supported roots: `trigger`, `actions`, `item`.
    ///
    /// Also recognizes the `sum(<collection-path>, <field>)` and
    /// `count(<collection-path>)` aggregate call forms (see
    /// [`parse_aggregate_call`]) ahead of the plain dot-path dispatch below,
    /// since their syntax (`sum(...)`) doesn't parse as a dot-path at all.
    ///
    /// Handles both `actions[0].result.field` and `actions.0.result.field` formats.
    pub async fn resolve_binding(&mut self, path: &str) -> Result<Value, String> {
        if let Some(call) = parse_aggregate_call(path) {
            // Recursive (resolve_binding resolving the call's own collection
            // sub-path) — boxed for the same reason every other recursive
            // async call in this file is (`resolve_bindings_in_value`, etc.):
            // a directly-recursive `async fn` has an infinite-sized future
            // unless indirected through the heap.
            return Box::pin(self.resolve_aggregate_call(call)).await;
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

    async fn resolve_trigger_path(&mut self, segments: &[&str]) -> Result<Value, String> {
        match segments.first().copied() {
            Some("node") => {
                // Try JSON navigation first (direct properties)
                match navigate_json(&self.trigger_node, &segments[1..]) {
                    Ok(val) => Ok(val),
                    // `segments` still includes the leading "node", so `> 1`
                    // is "at least one real segment past it" -- i.e. ANY
                    // relationship-name reference, not just a 2+-hop one.
                    // This used to read `> 2`, which silently excluded the
                    // single-hop case (`{trigger.node.<relationship>}`,
                    // landing on a Node OR a "many" Collection) from ever
                    // reaching `GraphResolver` below: `resolve_path` itself
                    // handles a one-segment path correctly (see its
                    // `segments.is_empty()` early return and its per-segment
                    // walk), the old guard just never gave it the chance,
                    // returning the raw JSON-navigation miss instead. Found
                    // while building `sum(collection, field)`
                    // (`resolve_aggregate_call`): a relationship-based
                    // collection is exactly this single-hop shape
                    // (`trigger.node.issues`, not `trigger.node.issues.x`),
                    // and `for_each` resolves its own collection path through
                    // this SAME function -- so `for_each` over a genuine
                    // relationship (as opposed to an array-valued property,
                    // the only shape previously exercised by this file's own
                    // tests) was equally unreachable before this fix.
                    // Strictly additive: this arm only runs when JSON
                    // navigation already failed, so no path that used to
                    // resolve successfully is affected.
                    Err(_) if segments.len() > 1 => {
                        // JSON navigation failed -- try graph traversal via GraphResolver
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

    /// Execute a parsed `sum(...)`/`count(...)` aggregate call (see
    /// [`AggregateCall`]/[`parse_aggregate_call`]).
    ///
    /// Resolves `call.collection_path` through the exact same
    /// [`Self::resolve_binding`] entry point `for_each` resolves its own
    /// collection through (see `execute_actions`'s `for_each` branch) --
    /// there is no second, parallel graph-traversal implementation here, only
    /// a reduction step over whatever `resolve_binding` already returns.
    async fn resolve_aggregate_call(&mut self, call: AggregateCall) -> Result<Value, String> {
        let resolved = Box::pin(self.resolve_binding(&call.collection_path)).await?;
        let items = match resolved {
            Value::Array(items) => items,
            other => {
                return Err(format!(
                    "aggregate collection path '{}' did not resolve to an array (got {})",
                    call.collection_path,
                    json_kind(&other)
                ));
            }
        };

        // Bounds the cost of the aggregation ITSELF (the reduction below),
        // not the collection fetch that already happened inside
        // `resolve_binding` above -- that fetch is `for_each`'s own,
        // pre-existing, already-unbounded `GraphResolver` traversal (see
        // `AGGREGATE_CALL_MAX_ITEMS`'s doc). A collection that already
        // exceeded this size paid its (uncapped) DB-read cost before this
        // check ever runs; what this prevents is a huge in-memory array
        // (however it got resolved) also paying an unbounded reduction cost.
        if items.len() > AGGREGATE_CALL_MAX_ITEMS {
            return Err(format!(
                "aggregate collection at '{}' has {} items, exceeding the {} item aggregation cap",
                call.collection_path,
                items.len(),
                AGGREGATE_CALL_MAX_ITEMS
            ));
        }

        match call.function {
            AggregateFn::Count => Ok(json!(items.len() as i64)),
            AggregateFn::Sum => {
                let field = call
                    .field
                    .as_deref()
                    .expect("parse_aggregate_call always sets `field` for AggregateFn::Sum");
                Ok(sum_numeric_field(&items, field))
            }
        }
    }
}

/// Human-readable JSON value kind, for error messages only (no behavior
/// depends on this).
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

// ---------------------------------------------------------------------------
// sum(collection, field) / count(collection) -- aggregate binding calls
// ---------------------------------------------------------------------------
//
// See the module doc's "Binding Context" section: `{dot.path}` bindings are
// the ONLY mechanism action params evaluate today (there is no CEL surface
// in action values -- see `playbook::validation`'s invariant-eligibility doc,
// which is updated alongside this to note the new call-form surface). These
// two call forms are recognized by `BindingContext::resolve_binding` ahead of
// its plain dot-path dispatch, computed via a shared, pure Rust reduction
// (`sum_numeric_field`) so there is exactly one implementation of "read a
// numeric field off a resolved collection item", not two.

/// Row/item cap on `sum(...)`/`count(...)` aggregation (see
/// `BindingContext::resolve_aggregate_call`'s doc for exactly what this does
/// and does not bound). Chosen generously above any realistic single-user
/// collection (a Cycle's assigned Issues, a Project's Tasks, ...) while still
/// giving a huge/pathological collection a fixed, fast failure instead of an
/// unbounded reduction cost -- the same "fixed cost regardless of table size"
/// reasoning as `TITLE_STEM_FALLBACK_CANDIDATE_CAP` in
/// `db::sqlite_store::mod`.
const AGGREGATE_CALL_MAX_ITEMS: usize = 10_000;

/// Which reduction an [`AggregateCall`] performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AggregateFn {
    Sum,
    Count,
}

/// A parsed `sum(<collection-path>, <field>)` or `count(<collection-path>)`
/// binding call.
#[derive(Debug, Clone, PartialEq)]
struct AggregateCall {
    function: AggregateFn,
    /// Raw (unbraced) binding path to the collection, e.g.
    /// `trigger.node.issues` -- passed straight to `resolve_binding`, the
    /// same way `for_each`'s own path is.
    collection_path: String,
    /// Field name to sum on each item. `Some` for `Sum`, `None` for `Count`.
    field: Option<String>,
}

/// Parse a `sum(...)`/`count(...)` aggregate call out of a raw (unbraced)
/// binding path. Returns `None` for anything else, so an ordinary dot-path
/// binding (including one that happens to start with a segment named
/// literally "sum" or "count", however unlikely) falls through unchanged to
/// `resolve_binding`'s normal dispatch.
///
/// Grammar (deliberately minimal, no nested calls, no expressions):
/// - `sum(<collection-path>, <field>)` -- exactly two comma-separated
///   arguments; `<field>` may optionally be quoted (`"estimate"` or
///   `'estimate'`), matching how the equivalent CEL-style call would read the
///   field as a string literal.
/// - `count(<collection-path>)` -- exactly one argument.
fn parse_aggregate_call(path: &str) -> Option<AggregateCall> {
    let trimmed = path.trim();
    if let Some(inner) = trimmed
        .strip_prefix("sum(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let mut parts = inner.splitn(2, ',');
        let collection_path = parts.next()?.trim();
        let field = parts.next()?.trim();
        if collection_path.is_empty() || field.is_empty() {
            return None;
        }
        return Some(AggregateCall {
            function: AggregateFn::Sum,
            collection_path: collection_path.to_string(),
            field: Some(strip_matching_quotes(field).to_string()),
        });
    }

    if let Some(inner) = trimmed
        .strip_prefix("count(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let collection_path = inner.trim();
        if collection_path.is_empty() {
            return None;
        }
        return Some(AggregateCall {
            function: AggregateFn::Count,
            collection_path: collection_path.to_string(),
            field: None,
        });
    }

    None
}

/// Strip one matching pair of leading/trailing `"` or `'` characters, if
/// present. `sum(path, "estimate")` and `sum(path, estimate)` are both
/// accepted -- quoting is optional here (unlike CEL, where a bare `estimate`
/// would parse as an identifier reference, not a string).
fn strip_matching_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Sum a numeric field across a resolved collection of items.
///
/// Items are the SAME shape `for_each` items already are: JSON-serialized
/// `Node`s (type-namespaced properties), not flattened maps -- collection
/// paths resolve through `GraphResolver`/`resolve_binding` exactly like
/// `for_each`'s do (see `resolve_aggregate_call`). Each item is deserialized
/// back into a `Node` and read through `graph_resolver::get_node_property`
/// (the same namespace-aware lookup `for_each`'s `{item.properties.<type>.*}`
/// bindings ultimately rely on), so `estimate` correctly finds
/// `properties.agg_issue.estimate` / `properties["custom:estimate"]`, not
/// just a flat top-level `estimate` key.
///
/// Falls back to a flat top-level lookup (`item.get(field)`) when an item
/// isn't a full `Node` (e.g. a plain scalar/object collection, as in the
/// unit tests below) -- a collection resolved via `GraphResolver` is always
/// `Vec<Node>`, so this fallback exists for robustness on non-node
/// collections, not as the primary path.
///
/// An item that is missing the field entirely, or whose field isn't numeric,
/// contributes 0 rather than failing the whole aggregation: a running total
/// over a partially-populated collection (some items simply haven't had the
/// field set yet) is the realistic common case for a report-style derived
/// value like this, not an error condition.
fn sum_numeric_field(items: &[Value], field: &str) -> Value {
    let mut sum_int: i64 = 0;
    let mut sum_float: f64 = 0.0;
    let mut is_float = false;

    for item in items {
        let field_value = serde_json::from_value::<Node>(item.clone())
            .ok()
            .and_then(|node| crate::playbook::graph_resolver::get_node_property(&node, field))
            .or_else(|| item.get(field).cloned());

        let Some(Value::Number(n)) = field_value else {
            continue;
        };
        if let Some(i) = n.as_i64() {
            sum_int += i;
            sum_float += i as f64;
        } else if let Some(f) = n.as_f64() {
            is_float = true;
            sum_float += f;
        }
    }

    if is_float {
        json!(sum_float)
    } else {
        json!(sum_int)
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
    // parse_aggregate_call — grammar
    // -----------------------------------------------------------------------

    #[test]
    fn parse_sum_call_bare_field() {
        let call = parse_aggregate_call("sum(trigger.node.issues, estimate)").unwrap();
        assert_eq!(call.function, AggregateFn::Sum);
        assert_eq!(call.collection_path, "trigger.node.issues");
        assert_eq!(call.field.as_deref(), Some("estimate"));
    }

    #[test]
    fn parse_sum_call_double_quoted_field() {
        let call = parse_aggregate_call(r#"sum(trigger.node.issues, "estimate")"#).unwrap();
        assert_eq!(call.field.as_deref(), Some("estimate"));
    }

    #[test]
    fn parse_sum_call_single_quoted_field() {
        let call = parse_aggregate_call("sum(trigger.node.issues, 'estimate')").unwrap();
        assert_eq!(call.field.as_deref(), Some("estimate"));
    }

    #[test]
    fn parse_sum_call_tolerates_extra_whitespace() {
        let call = parse_aggregate_call("  sum( trigger.node.issues ,  estimate ) ").unwrap();
        assert_eq!(call.collection_path, "trigger.node.issues");
        assert_eq!(call.field.as_deref(), Some("estimate"));
    }

    #[test]
    fn parse_count_call() {
        let call = parse_aggregate_call("count(trigger.node.issues)").unwrap();
        assert_eq!(call.function, AggregateFn::Count);
        assert_eq!(call.collection_path, "trigger.node.issues");
        assert_eq!(call.field, None);
    }

    #[test]
    fn parse_aggregate_call_rejects_sum_with_missing_field() {
        assert!(parse_aggregate_call("sum(trigger.node.issues)").is_none());
    }

    #[test]
    fn parse_aggregate_call_rejects_sum_with_empty_field() {
        assert!(parse_aggregate_call("sum(trigger.node.issues, )").is_none());
    }

    #[test]
    fn parse_aggregate_call_rejects_count_with_empty_path() {
        assert!(parse_aggregate_call("count()").is_none());
    }

    #[test]
    fn parse_aggregate_call_returns_none_for_an_ordinary_dot_path() {
        // Ordinary bindings (including anything that isn't a `sum(...)`/
        // `count(...)` call) must fall through unchanged to the normal
        // dot-path dispatch in `resolve_binding`.
        assert!(parse_aggregate_call("trigger.node.id").is_none());
        assert!(parse_aggregate_call("item.status").is_none());
        assert!(parse_aggregate_call("summary.trigger.node.id").is_none());
    }

    // -----------------------------------------------------------------------
    // BindingContext::resolve_binding — sum(...)/count(...) aggregate calls
    // -----------------------------------------------------------------------

    /// Helper: a Node-shaped collection item, matching what a real
    /// `GraphResolver`-resolved collection item looks like (type-namespaced
    /// properties), the same shape `for_each` items are.
    fn make_collection_item_node(id: &str, node_type: &str, field: &str, value: Value) -> Value {
        serde_json::to_value(Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            content: String::new(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties: json!({ node_type: { field: value } }),
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn sum_over_node_shaped_collection_reads_type_namespaced_field() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results.push(json!([
            make_collection_item_node("i1", "agg_issue", "estimate", json!(3)),
            make_collection_item_node("i2", "agg_issue", "estimate", json!(5)),
        ]));

        let result = ctx
            .resolve_binding("sum(actions[0].result, estimate)")
            .await
            .unwrap();
        assert_eq!(result, json!(8));
    }

    #[tokio::test]
    async fn count_over_node_shaped_collection() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results.push(json!([
            make_collection_item_node("i1", "agg_issue", "estimate", json!(3)),
            make_collection_item_node("i2", "agg_issue", "estimate", json!(5)),
            make_collection_item_node("i3", "agg_issue", "estimate", json!(1)),
        ]));

        let result = ctx
            .resolve_binding("count(actions[0].result)")
            .await
            .unwrap();
        assert_eq!(result, json!(3));
    }

    #[tokio::test]
    async fn count_of_empty_collection_is_zero() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results.push(json!([]));

        let result = ctx
            .resolve_binding("count(actions[0].result)")
            .await
            .unwrap();
        assert_eq!(result, json!(0));
    }

    #[tokio::test]
    async fn sum_skips_items_missing_the_field_instead_of_failing() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results.push(json!([
            make_collection_item_node("i1", "agg_issue", "estimate", json!(3)),
            // No "estimate" set on this issue yet -- a realistic partially
            // populated collection, not malformed input.
            make_collection_item_node("i2", "agg_issue", "other_field", json!("x")),
        ]));

        let result = ctx
            .resolve_binding("sum(actions[0].result, estimate)")
            .await
            .unwrap();
        assert_eq!(result, json!(3));
    }

    #[tokio::test]
    async fn sum_skips_items_with_a_non_numeric_field_instead_of_failing() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results.push(json!([
            make_collection_item_node("i1", "agg_issue", "estimate", json!(3)),
            make_collection_item_node("i2", "agg_issue", "estimate", json!("not a number")),
        ]));

        let result = ctx
            .resolve_binding("sum(actions[0].result, estimate)")
            .await
            .unwrap();
        assert_eq!(result, json!(3));
    }

    #[tokio::test]
    async fn sum_falls_back_to_flat_lookup_for_non_node_items() {
        // A collection whose items aren't full serialized `Node`s (e.g. an
        // array literal stored directly on a property, the same shape
        // for_each's own existing tests use) -- must still work via the flat
        // `item.get(field)` fallback.
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results
            .push(json!([{"name": "a", "points": 2}, {"name": "b", "points": 4}]));

        let result = ctx
            .resolve_binding("sum(actions[0].result, points)")
            .await
            .unwrap();
        assert_eq!(result, json!(6));
    }

    #[tokio::test]
    async fn sum_preserves_float_result_when_any_value_is_a_float() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results.push(json!([
            make_collection_item_node("i1", "agg_issue", "estimate", json!(2)),
            make_collection_item_node("i2", "agg_issue", "estimate", json!(1.5)),
        ]));

        let result = ctx
            .resolve_binding("sum(actions[0].result, estimate)")
            .await
            .unwrap();
        assert_eq!(result, json!(3.5));
    }

    #[tokio::test]
    async fn sum_errors_when_the_collection_path_does_not_resolve_to_an_array() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results.push(json!(42));

        let err = ctx
            .resolve_binding("sum(actions[0].result, estimate)")
            .await
            .unwrap_err();
        assert!(err.contains("did not resolve to an array"));
    }

    #[tokio::test]
    async fn sum_reuses_for_eachs_own_collection_resolution_path() {
        // Same `trigger.node.properties.items` collection shape the
        // existing `for_each` unit tests already exercise (see
        // `Some("trigger.node.properties.items")` elsewhere in this file) --
        // proves the aggregate call resolves its collection through the
        // exact same `resolve_binding`/JSON-navigation path `for_each` uses,
        // not a second, parallel implementation.
        let mut node = make_test_node("node-123", "task");
        node.properties = json!({
            "items": [
                {"name": "a", "points": 2},
                {"name": "b", "points": 4},
                {"name": "c", "points": 6},
            ]
        });
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let sum = ctx
            .resolve_binding("sum(trigger.node.properties.items, points)")
            .await
            .unwrap();
        assert_eq!(sum, json!(12));

        let count = ctx
            .resolve_binding("count(trigger.node.properties.items)")
            .await
            .unwrap();
        assert_eq!(count, json!(3));
    }

    #[tokio::test]
    async fn sum_call_over_max_items_cap_errors() {
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let oversized: Vec<Value> = (0..(AGGREGATE_CALL_MAX_ITEMS + 1))
            .map(|i| json!({"points": i}))
            .collect();
        ctx.action_results.push(Value::Array(oversized));

        let err = ctx
            .resolve_binding("sum(actions[0].result, points)")
            .await
            .unwrap_err();
        assert!(err.contains("aggregation cap"));
    }

    #[tokio::test]
    async fn count_call_at_exactly_max_items_cap_succeeds() {
        // Proves the cap is an exclusive upper bound (`> MAX`, not `>= MAX`)
        // -- a collection sized exactly at the cap must still succeed.
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        let at_cap: Vec<Value> = (0..AGGREGATE_CALL_MAX_ITEMS)
            .map(|i| json!({"points": i}))
            .collect();
        ctx.action_results.push(Value::Array(at_cap));

        let result = ctx
            .resolve_binding("count(actions[0].result)")
            .await
            .unwrap();
        assert_eq!(result, json!(AGGREGATE_CALL_MAX_ITEMS as i64));
    }

    #[tokio::test]
    async fn sum_is_usable_inside_an_update_node_actions_params() {
        // Acceptance criterion: "Result is written to a target node's field
        // via existing action-writing mechanism" -- proves `sum(...)` is
        // reachable from the SAME `resolve_bindings_in_value` recursive walk
        // `execute_update_node`'s params go through, not just from a direct
        // `resolve_binding` call.
        let node = make_test_node("node-123", "task");
        let event = make_node_created_event("node-123", "task");
        let mut ctx = BindingContext::new(&node, &event, None);

        ctx.action_results.push(json!([
            make_collection_item_node("i1", "agg_issue", "estimate", json!(3)),
            make_collection_item_node("i2", "agg_issue", "estimate", json!(5)),
        ]));

        let params = json!({
            "node_id": "{trigger.node.id}",
            "properties": { "total_estimate": "{sum(actions[0].result, estimate)}" }
        });

        let resolved = resolve_bindings_in_value(&params, &mut ctx).await.unwrap();
        assert_eq!(resolved["properties"]["total_estimate"], json!(8));
        assert_eq!(resolved["node_id"], json!("node-123"));
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
    // Single-hop relationship binding resolution — regression coverage for
    // the `resolve_trigger_path` guard fix (`> 2` → `> 1`) this change made.
    // Integration tests against a real NodeService + a real schema-declared
    // relationship, since the bug only manifests through `GraphResolver`
    // (a unit test with a synthetic `item`/`actions[N].result` JSON value
    // can't exercise it -- there's no relationship to walk).
    // -----------------------------------------------------------------------

    mod single_hop_relationship_binding_integration {
        use super::*;
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

        async fn declare_schema(svc: &NodeService, node_type: &str, relationships: Value) {
            let schema = Node::new_with_id(
                node_type.to_string(),
                "schema".to_string(),
                node_type.to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": format!("{node_type} schema"),
                    "fields": [{"name": "estimate", "type": "number"}],
                    "relationships": []
                }),
            );
            svc.create_node(schema).await.unwrap();

            let declarations: Vec<crate::models::schema::SchemaRelationship> =
                serde_json::from_value(relationships).unwrap();
            if !declarations.is_empty() {
                svc.set_schema_relationships(node_type, &declarations)
                    .await
                    .unwrap();
            }
        }

        /// Before this fix, `{trigger.node.<relationship>}` (a SINGLE hop,
        /// landing directly on the related node -- no property segment
        /// after it) failed with a raw JSON-navigation "not found" error
        /// without ever reaching `GraphResolver`, because `resolve_trigger_path`
        /// only attempted graph traversal for 2+-hop paths. Proves the
        /// single-hop "one" case now resolves to the full related node.
        #[tokio::test(flavor = "multi_thread")]
        async fn single_hop_one_relationship_resolves_to_the_related_node() {
            let (svc, _tmp) = create_test_service().await;

            declare_schema(&svc, "sh_cycle", json!([])).await;
            declare_schema(
                &svc,
                "sh_issue",
                json!([{
                    "name": "cycle",
                    "targetType": "sh_cycle",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let cycle = make_trigger_node("cycle-1", "sh_cycle", json!({}));
            svc.create_node(cycle).await.unwrap();
            let issue =
                make_trigger_node("issue-1", "sh_issue", json!({"sh_issue": {"estimate": 3}}));
            svc.create_node(issue.clone()).await.unwrap();
            svc.create_relationship("issue-1", "cycle", "cycle-1", json!({}))
                .await
                .unwrap();

            let event = make_node_created_event("issue-1", "sh_issue");
            let resolver = GraphResolver::new(Arc::clone(&svc));
            let mut ctx = BindingContext::new(&issue, &event, Some(resolver));

            let result = ctx.resolve_binding("trigger.node.cycle").await.unwrap();
            assert_eq!(
                result.get("id").and_then(|v| v.as_str()),
                Some("cycle-1"),
                "single-hop `trigger.node.<relationship>` must resolve to the \
                 full related node, not fail before ever reaching GraphResolver"
            );
        }

        /// Same fix, "many" side: a single-hop relationship collection --
        /// exactly the shape `sum(trigger.node.issues, estimate)` needs for
        /// its primary real-world case (a Cycle's related Issues), and the
        /// same shape a genuine relationship-based `for_each` needs (as
        /// opposed to the property-array-valued for_each this file's other
        /// tests exercise).
        #[tokio::test(flavor = "multi_thread")]
        async fn single_hop_many_relationship_resolves_to_the_collection_and_sums() {
            let (svc, _tmp) = create_test_service().await;

            // Target schema must exist before a relationship can declare it
            // as `targetType` -- declare the "many" side (sh_issue2) first.
            declare_schema(&svc, "sh_issue2", json!([])).await;
            declare_schema(
                &svc,
                "sh_cycle2",
                json!([{
                    "name": "issues",
                    "targetType": "sh_issue2",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "cycle",
                    "reverseCardinality": "one"
                }]),
            )
            .await;

            let cycle = make_trigger_node("cycle-2", "sh_cycle2", json!({}));
            svc.create_node(cycle.clone()).await.unwrap();
            let issue_a = make_trigger_node(
                "issue-a",
                "sh_issue2",
                json!({"sh_issue2": {"estimate": 3}}),
            );
            let issue_b = make_trigger_node(
                "issue-b",
                "sh_issue2",
                json!({"sh_issue2": {"estimate": 5}}),
            );
            svc.create_node(issue_a).await.unwrap();
            svc.create_node(issue_b).await.unwrap();
            svc.create_relationship("cycle-2", "issues", "issue-a", json!({}))
                .await
                .unwrap();
            svc.create_relationship("cycle-2", "issues", "issue-b", json!({}))
                .await
                .unwrap();

            let event = make_node_created_event("cycle-2", "sh_cycle2");
            let resolver = GraphResolver::new(Arc::clone(&svc));
            let mut ctx = BindingContext::new(&cycle, &event, Some(resolver));

            let collection = ctx.resolve_binding("trigger.node.issues").await.unwrap();
            assert!(
                matches!(collection, Value::Array(ref items) if items.len() == 2),
                "single-hop `trigger.node.<many-relationship>` must resolve to \
                 the full 2-item collection: {collection:?}"
            );

            let sum = ctx
                .resolve_binding("sum(trigger.node.issues, estimate)")
                .await
                .unwrap();
            assert_eq!(sum, json!(8), "sum(...) must reuse this same resolution");
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
