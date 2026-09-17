//! CEL Evaluator for the Play Engine
//!
//! Compiles and evaluates CEL (Common Expression Language) conditions against
//! trigger nodes. Uses the `cel-interpreter` crate with custom variable resolution
//! and built-in functions for date operations.
//!
//! # Property Flattening
//!
//! Before evaluation, node properties are flattened into the CEL Map and namespace
//! prefixes are stripped: `custom:amount` → `node.amount`. Properties are read
//! directly from `node.properties` (the raw DB format), not via
//! `flatten_properties_for_api`.
//!
//! # Graph Traversal
//!
//! Dot-path relationship walking (e.g., `node.story.epic.status`) is resolved
//! before evaluation by the `GraphResolver`. The path extractor parses CEL ASTs
//! to discover multi-hop paths, the resolver fetches related nodes, and the
//! results are injected as nested CEL Maps into the evaluation context.
//!
//! # Missing Path Behavior
//!
//! In conditions, a missing path evaluates to `false` — the condition is not met,
//! but the play stays active. This matches the spec: relationships are built
//! progressively, so a condition checking `node.story.epic.status` should wait
//! until the chain exists, not disable itself.

use cel_interpreter::{Context, ExecutionError, Program, Value};
use chrono::{Local, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

use crate::db::events::DomainEvent;
use crate::models::Node;
use crate::playbook::graph_resolver::{inject_resolved_paths, GraphResolver};
use crate::playbook::path_extractor;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from CEL compilation (parse-time).
#[derive(Debug, Clone, PartialEq)]
pub struct CelCompileError {
    pub expression: String,
    pub message: String,
}

impl std::fmt::Display for CelCompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CEL compile error in '{}': {}",
            self.expression, self.message
        )
    }
}

/// Result of evaluating all conditions for a rule.
#[derive(Debug, Clone, PartialEq)]
pub enum ConditionResult {
    /// All conditions passed (or no conditions were specified).
    Pass,
    /// At least one condition evaluated to false.
    Fail {
        /// Index of the first failing condition.
        condition_index: usize,
    },
}

// ---------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------

/// Compile a CEL expression string into a reusable Program.
///
/// Used at play save time for validation and at runtime for evaluation.
pub fn compile_condition(expr: &str) -> Result<Program, CelCompileError> {
    Program::compile(expr).map_err(|e| CelCompileError {
        expression: expr.to_string(),
        message: e.to_string(),
    })
}

/// A CEL condition compiled once at parse/save time and cached for reuse.
///
/// `Program` is not `Clone`, so the compiled program is wrapped in `Arc`
/// to allow `ParsedRule` (and `CompiledCondition` itself) to be cheaply cloned.
#[derive(Debug, Clone)]
pub struct CompiledCondition {
    /// The original CEL expression source, needed for path extraction and diagnostics.
    pub source: String,
    pub program: Arc<Program>,
}

impl CompiledCondition {
    pub fn compile(expr: &str) -> Result<Self, CelCompileError> {
        let program = compile_condition(expr)?;
        Ok(Self {
            source: expr.to_string(),
            program: Arc::new(program),
        })
    }
}

// ---------------------------------------------------------------------------
// Node → CEL Value Conversion
// ---------------------------------------------------------------------------

/// Convert a `serde_json::Value` to a CEL `Value`.
///
/// Maps JSON types to CEL types:
/// - null → Null
/// - bool → Bool
/// - number (integer) → Int
/// - number (float) → Float
/// - string → String
/// - array → List
/// - object → Map
pub fn json_to_cel(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                // u64 that doesn't fit in i64
                Value::UInt(n.as_u64().unwrap_or(0))
            }
        }
        serde_json::Value::String(s) => Value::String(Arc::new(s.clone())),
        serde_json::Value::Array(arr) => {
            Value::List(arr.iter().map(json_to_cel).collect::<Vec<_>>().into())
        }
        serde_json::Value::Object(obj) => {
            let map: HashMap<cel_interpreter::objects::Key, Value> = obj
                .iter()
                .map(|(k, v)| {
                    (
                        cel_interpreter::objects::Key::String(Arc::new(k.clone())),
                        json_to_cel(v),
                    )
                })
                .collect();
            Value::Map(cel_interpreter::objects::Map { map: Arc::new(map) })
        }
    }
}

/// Build a CEL `Value` (Map) from a Node in wire format.
///
/// The resulting map has these top-level keys:
/// - `id`: String
/// - `node_type`: String
/// - `content`: String
/// - `version`: Int
/// - `lifecycle_status`: String
/// - All flattened properties as additional keys
///
/// Namespace prefixes on properties are stripped: `custom:status` → `status`.
/// Internal `_`-prefixed bookkeeping keys (`_seed`, `_schemaVersion`,
/// `_playbookChainDepth`, ...) are excluded entirely, whether they appear
/// nested inside the type namespace or -- their actual stored shape,
/// per `NodeService::normalize_flat_properties_to_namespace` -- at the top
/// level alongside it.
pub fn node_to_cel_value(node: &Node) -> Value {
    let mut map: HashMap<cel_interpreter::objects::Key, Value> = HashMap::new();

    // Core fields
    map.insert(key("id"), Value::String(Arc::new(node.id.clone())));
    map.insert(
        key("node_type"),
        Value::String(Arc::new(node.node_type.clone())),
    );
    map.insert(
        key("content"),
        Value::String(Arc::new(node.content.clone())),
    );
    map.insert(key("version"), Value::Int(node.version));
    map.insert(
        key("lifecycle_status"),
        Value::String(Arc::new(node.lifecycle_status.clone())),
    );

    // Flatten properties into the map, stripping namespace prefixes.
    //
    // NodeSpace stores properties in a type-namespaced format after create_node:
    //   {"task": {"status": "open", "priority": "high"}}
    // We unwrap the type namespace so CEL conditions can use `node.status` directly.
    // NOTE: Parallel logic exists in graph_resolver::get_node_property — if the
    // property storage format changes, both must be updated.
    // Also handles colon-prefixed namespaces: "custom:amount" → "amount".
    if let Some(obj) = node.properties.as_object() {
        for (k, v) in obj {
            if k == &node.node_type {
                // Type namespace: unwrap inner properties
                if let Some(inner_obj) = v.as_object() {
                    for (ik, iv) in inner_obj {
                        // Skip internal fields like _schema_version
                        if !ik.starts_with('_') {
                            map.insert(key(ik), json_to_cel(iv));
                        }
                    }
                }
            } else if !k.starts_with('_') {
                // Skip internal bookkeeping fields (`_seed`, `_schemaVersion`,
                // `_playbookChainDepth`, ...) -- same `_`-prefix convention as
                // `NodeService::normalize_flat_properties_to_namespace`, which
                // is what decides these keys stay unnamespaced at the top
                // level in the first place. The check is on the raw key `k`,
                // not a colon-stripped bare name, matching
                // `validate_schema_field_name`'s rule that only a leading `_`
                // on the WHOLE stored key is internal -- `custom:_internal`
                // is a legal, visible user field, not bookkeeping.
                //
                // Strip colon namespace prefix: "custom:amount" → "amount"
                let bare_key = k.find(':').map(|i| &k[i + 1..]).unwrap_or(k.as_str());
                map.insert(key(bare_key), json_to_cel(v));
            }
        }
    }

    Value::Map(cel_interpreter::objects::Map { map: Arc::new(map) })
}

/// Convenience: create a CEL Map Key from a string.
pub fn key(s: &str) -> cel_interpreter::objects::Key {
    cel_interpreter::objects::Key::String(Arc::new(s.to_string()))
}

// ---------------------------------------------------------------------------
// Context Building
// ---------------------------------------------------------------------------

/// Build a CEL evaluation context for a rule's conditions.
///
/// Variables available in conditions:
/// - `node`: The trigger node (wire-format, flat properties)
/// - `trigger.property.old_value`: Previous value (PropertyChanged only)
/// - `trigger.property.new_value`: New value (PropertyChanged only)
///
/// Functions:
/// - `days_since(date_string)`: Days elapsed since ISO 8601 date
/// - `days_until(date_string)`: Days remaining until ISO 8601 date
/// - `today()`: Current date as ISO 8601 string
/// - `add_days(date_string, n)`: A new ISO 8601 date, `n` days offset from `date_string`
pub fn build_condition_context<'a>(node: &Node, event: &DomainEvent) -> Context<'a> {
    build_condition_context_with_resolved(node, event, &HashMap::new())
}

/// Build a CEL evaluation context with pre-resolved graph paths injected.
///
/// `resolved_values` maps path segments → CEL Values resolved by GraphResolver.
/// These are injected as nested Maps into the `node` variable so that
/// expressions like `node.story.epic.status` resolve correctly.
fn build_condition_context_with_resolved<'a>(
    node: &Node,
    event: &DomainEvent,
    resolved_values: &HashMap<Vec<String>, Value>,
) -> Context<'a> {
    let mut ctx = Context::default();

    // `node` variable — the trigger node in wire format, enriched with resolved paths
    let base_node = node_to_cel_value(node);
    let enriched_node = inject_resolved_paths(&base_node, resolved_values);
    ctx.add_variable_from_value("node", enriched_node);

    // `trigger` variable — event-specific context
    let mut trigger_map: HashMap<cel_interpreter::objects::Key, Value> = HashMap::new();

    // Add trigger.node as an alias (also enriched with resolved paths)
    let trigger_node_value = inject_resolved_paths(&node_to_cel_value(node), resolved_values);
    trigger_map.insert(key("node"), trigger_node_value);

    // For PropertyChanged events, add trigger.property with old/new values
    if let DomainEvent::NodeUpdated {
        changed_properties, ..
    } = event
    {
        if let Some(first_prop) = changed_properties.first() {
            let mut prop_map: HashMap<cel_interpreter::objects::Key, Value> = HashMap::new();
            prop_map.insert(key("key"), Value::String(Arc::new(first_prop.key.clone())));
            prop_map.insert(
                key("old_value"),
                first_prop
                    .old_value
                    .as_ref()
                    .map(json_to_cel)
                    .unwrap_or(Value::Null),
            );
            prop_map.insert(
                key("new_value"),
                first_prop
                    .new_value
                    .as_ref()
                    .map(json_to_cel)
                    .unwrap_or(Value::Null),
            );
            trigger_map.insert(
                key("property"),
                Value::Map(cel_interpreter::objects::Map {
                    map: Arc::new(prop_map),
                }),
            );
        }

        // Also add trigger.properties (all changed properties) for multi-prop events
        let props_list: Vec<Value> = changed_properties
            .iter()
            .map(|pc| {
                let mut m: HashMap<cel_interpreter::objects::Key, Value> = HashMap::new();
                m.insert(key("key"), Value::String(Arc::new(pc.key.clone())));
                m.insert(
                    key("old_value"),
                    pc.old_value
                        .as_ref()
                        .map(json_to_cel)
                        .unwrap_or(Value::Null),
                );
                m.insert(
                    key("new_value"),
                    pc.new_value
                        .as_ref()
                        .map(json_to_cel)
                        .unwrap_or(Value::Null),
                );
                Value::Map(cel_interpreter::objects::Map { map: Arc::new(m) })
            })
            .collect();
        trigger_map.insert(key("properties"), Value::List(props_list.into()));
    }

    ctx.add_variable_from_value(
        "trigger",
        Value::Map(cel_interpreter::objects::Map {
            map: Arc::new(trigger_map),
        }),
    );

    // Register custom functions
    ctx.add_function("days_since", cel_days_since);
    ctx.add_function("days_until", cel_days_until);
    ctx.add_function("today", cel_today);
    ctx.add_function("add_days", cel_add_days);

    ctx
}

// ---------------------------------------------------------------------------
// Custom CEL Functions
// ---------------------------------------------------------------------------

/// CEL functions that read wall-clock time and are therefore **non-deterministic**
/// across devices: two devices evaluating the same condition at different moments
/// can disagree.
///
/// ADR-060 §2 forbids these in an invariant rule's conditions (see
/// [`crate::playbook::validation`]). Keep this list in sync with the functions
/// registered in [`build_condition_context_with_resolved`] — it is the single
/// source of truth for "which CEL functions are non-deterministic". There is no
/// random-value function registered, so wall-clock reads are the only
/// non-deterministic surface CEL can express today.
pub const NON_DETERMINISTIC_FUNCTIONS: &[&str] = &["today", "days_since", "days_until"];

/// `days_since(date_string)` — Parse ISO 8601 date and return days elapsed.
///
/// Returns negative for future dates. Returns error for invalid input.
fn cel_days_since(date_str: Arc<String>) -> Result<Value, ExecutionError> {
    parse_date_and_compute_days(&date_str, true)
}

/// `days_until(date_string)` — Parse ISO 8601 date and return days remaining.
///
/// Returns negative for past dates. Returns error for invalid input.
fn cel_days_until(date_str: Arc<String>) -> Result<Value, ExecutionError> {
    parse_date_and_compute_days(&date_str, false)
}

/// `today()` — Return current local date as ISO 8601 string.
fn cel_today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// Parse a date string and compute days since or until.
fn parse_date_and_compute_days(date_str: &str, since: bool) -> Result<Value, ExecutionError> {
    // Try parsing as full ISO 8601 datetime first
    let date = if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(date_str) {
        dt.with_timezone(&Utc).date_naive()
    } else if let Ok(dt) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        dt
    } else {
        return Err(ExecutionError::function_error(
            if since { "days_since" } else { "days_until" },
            format!("invalid date string: '{}'", date_str),
        ));
    };

    let today = Utc::now().date_naive();
    let diff = if since {
        (today - date).num_days()
    } else {
        (date - today).num_days()
    };

    Ok(Value::Int(diff))
}

/// `add_days(date_string, n)` — compute a NEW date, `n` days offset from
/// `date_string` (negative `n` walks backward). This is the one function in
/// this module that computes a value rather than comparing an existing date
/// to "now" — see the module doc for where it's consumed (an action's
/// computed field value, via `playbook::actions`'s function-call binding
/// syntax, as well as ordinary CEL conditions).
///
/// Unlike `days_since`/`days_until`/`today`, this never reads wall-clock
/// time — it is a pure function of its two arguments, so it is deliberately
/// NOT listed in [`NON_DETERMINISTIC_FUNCTIONS`] and is safe to use in an
/// invariant rule's conditions (ADR-060 §2).
///
/// Accepts the same two input shapes as `parse_date_and_compute_days`: a
/// full RFC 3339 datetime or a bare `YYYY-MM-DD` date. The returned string
/// matches the input's own format — a bare date in yields a bare date out;
/// a full datetime in (time-of-day and UTC offset preserved, only the
/// calendar date shifted) yields a full datetime out.
///
/// All arithmetic is checked, not wrapping/panicking: an `n` large enough to
/// overflow `chrono`'s internal representation, or to shift the date outside
/// `chrono`'s representable range, is a clean function error — never a
/// panic — since `n` ultimately comes from user-authored play content.
pub(crate) fn compute_add_days(date_str: &str, n: i64) -> Result<String, ExecutionError> {
    let out_of_range = || {
        ExecutionError::function_error(
            "add_days",
            format!("day offset {} is out of range for '{}'", n, date_str),
        )
    };

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(date_str) {
        let delta = chrono::Duration::try_days(n).ok_or_else(out_of_range)?;
        let shifted = dt.checked_add_signed(delta).ok_or_else(out_of_range)?;
        return Ok(shifted.to_rfc3339());
    }

    if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        let delta = chrono::Duration::try_days(n).ok_or_else(out_of_range)?;
        let shifted = date.checked_add_signed(delta).ok_or_else(out_of_range)?;
        return Ok(shifted.format("%Y-%m-%d").to_string());
    }

    Err(ExecutionError::function_error(
        "add_days",
        format!("invalid date string: '{}'", date_str),
    ))
}

/// `add_days(date_string, n)` — CEL wrapper around [`compute_add_days`] for
/// condition-expression use, e.g. `add_days(node.start_date, 14) ==
/// node.end_date`.
fn cel_add_days(date_str: Arc<String>, n: i64) -> Result<Value, ExecutionError> {
    compute_add_days(&date_str, n).map(|s| Value::String(Arc::new(s)))
}

// ---------------------------------------------------------------------------
// Condition Evaluation
// ---------------------------------------------------------------------------

/// Evaluate all conditions for a rule against the trigger node and event.
///
/// Each condition's `Program` was compiled once, at parse/save time (see
/// `CompiledCondition::compile`), and is executed here directly — no
/// recompilation on the hot path.
///
/// Conditions are evaluated in order with short-circuit on first failure.
/// An empty conditions list results in `ConditionResult::Pass`.
///
/// When `resolver` is provided, dot-path references (e.g., `node.story.epic.status`)
/// are pre-resolved via graph traversal before CEL evaluation. Without a resolver,
/// only property-level conditions on the trigger node are evaluated (backward
/// compatible with the Phase 3 behavior).
///
/// Missing path errors (NoSuchKey, UndeclaredReference) evaluate to `false`
/// per the spec — the condition fails but the play remains active.
pub async fn evaluate_conditions(
    conditions: &[CompiledCondition],
    node: &Node,
    event: &DomainEvent,
    resolver: Option<&mut GraphResolver>,
) -> ConditionResult {
    if conditions.is_empty() {
        return ConditionResult::Pass;
    }

    // Pre-resolve graph paths if a resolver is available
    let resolved_values = if let Some(resolver) = resolver {
        // Extract all paths from all conditions
        let mut all_paths = Vec::new();
        let mut all_collections = Vec::new();
        for condition in conditions {
            if let Ok(extraction) = path_extractor::extract_paths(&condition.source) {
                all_paths.extend(extraction.paths);
                all_collections.extend(extraction.collections);
            }
        }

        // Resolve paths that need graph traversal (multi-hop)
        if !all_paths.is_empty() || !all_collections.is_empty() {
            resolver
                .enrich_context(node, &all_paths, &all_collections)
                .await
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };

    let ctx = build_condition_context_with_resolved(node, event, &resolved_values);

    for (i, condition) in conditions.iter().enumerate() {
        match condition.program.execute(&ctx) {
            Ok(Value::Bool(true)) => {
                debug!("Condition[{}] passed: {}", i, condition.source);
                // Continue to next condition
            }
            Ok(Value::Bool(false)) => {
                debug!("Condition[{}] failed (false): {}", i, condition.source);
                return ConditionResult::Fail { condition_index: i };
            }
            Ok(other) => {
                // Non-boolean result — treat as failure
                debug!(
                    "Condition[{}] returned non-boolean {:?}: {}",
                    i, other, condition.source
                );
                return ConditionResult::Fail { condition_index: i };
            }
            Err(ExecutionError::NoSuchKey(_)) | Err(ExecutionError::UndeclaredReference(_)) => {
                // Missing path → false (spec: condition not met, play stays active)
                debug!(
                    "Condition[{}] has missing path (evaluates to false): {}",
                    i, condition.source
                );
                return ConditionResult::Fail { condition_index: i };
            }
            Err(e) => {
                // Other runtime errors — treat as condition failure, not compile error
                // The play stays active; it's the condition that doesn't match.
                debug!(
                    "Condition[{}] runtime error (evaluates to false): {} — {}",
                    i, condition.source, e
                );
                return ConditionResult::Fail { condition_index: i };
            }
        }
    }

    ConditionResult::Pass
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::events::PropertyChange;
    use chrono::Utc;
    use serde_json::json;

    /// Helper: create a test node with the given properties (already in wire format).
    fn test_node(node_type: &str, properties: serde_json::Value) -> Node {
        Node {
            id: "test-node-1".to_string(),
            node_type: node_type.to_string(),
            content: "Test content".to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            properties,
            mentions: vec![],
            mentioned_in: vec![],
            title: None,
            lifecycle_status: "active".to_string(),
        }
    }

    /// Helper: create a NodeCreated event.
    fn node_created_event(node_type: &str) -> DomainEvent {
        DomainEvent::NodeCreated {
            node_type: node_type.to_string(),
            node_id: "test-node-1".to_string(),
        }
    }

    /// Helper: create a NodeUpdated event with property changes.
    fn node_updated_event(node_type: &str, changes: Vec<PropertyChange>) -> DomainEvent {
        DomainEvent::NodeUpdated {
            node_type: node_type.to_string(),
            node_id: "test-node-1".to_string(),
            node: test_node(node_type, json!({})),
            changed_properties: changes,
        }
    }

    /// Helper: compile a single condition for tests.
    fn conds(exprs: &[&str]) -> Vec<CompiledCondition> {
        exprs
            .iter()
            .map(|e| CompiledCondition::compile(e).expect("valid CEL"))
            .collect()
    }

    // -- Compilation tests --

    #[test]
    fn compile_valid_expression() {
        assert!(compile_condition("1 + 1 == 2").is_ok());
    }

    #[test]
    fn compile_invalid_expression() {
        let err = compile_condition("1 + + 2").unwrap_err();
        assert_eq!(err.expression, "1 + + 2");
        assert!(!err.message.is_empty());
    }

    // -- json_to_cel tests --

    #[test]
    fn json_null_to_cel() {
        assert_eq!(json_to_cel(&json!(null)), Value::Null);
    }

    #[test]
    fn json_bool_to_cel() {
        assert_eq!(json_to_cel(&json!(true)), Value::Bool(true));
    }

    #[test]
    fn json_int_to_cel() {
        assert_eq!(json_to_cel(&json!(42)), Value::Int(42));
    }

    #[test]
    fn json_float_to_cel() {
        assert_eq!(json_to_cel(&json!(3.15)), Value::Float(3.15));
    }

    #[test]
    fn json_string_to_cel() {
        assert_eq!(
            json_to_cel(&json!("hello")),
            Value::String(Arc::new("hello".to_string()))
        );
    }

    #[test]
    fn json_array_to_cel() {
        let cel = json_to_cel(&json!([1, 2, 3]));
        match cel {
            Value::List(items) => assert_eq!(items.len(), 3),
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn json_object_to_cel() {
        let cel = json_to_cel(&json!({"key": "value"}));
        match cel {
            Value::Map(_) => {} // OK
            other => panic!("expected Map, got {:?}", other),
        }
    }

    // -- node_to_cel_value tests --

    #[test]
    fn node_to_cel_includes_core_fields() {
        let node = test_node("task", json!({"status": "open", "priority": "high"}));
        let _cel = node_to_cel_value(&node);

        let ctx = eval_context_with_node(&node);
        let result = Program::compile("node.id").unwrap().execute(&ctx);
        assert_eq!(
            result,
            Ok(Value::String(Arc::new("test-node-1".to_string())))
        );

        let result = Program::compile("node.node_type").unwrap().execute(&ctx);
        assert_eq!(result, Ok(Value::String(Arc::new("task".to_string()))));
    }

    #[test]
    fn node_to_cel_flattens_properties() {
        let node = test_node("task", json!({"status": "open", "priority": "high"}));
        let ctx = eval_context_with_node(&node);

        let result = Program::compile("node.status == 'open'")
            .unwrap()
            .execute(&ctx);
        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn node_to_cel_strips_namespace_prefix() {
        let node = test_node(
            "invoice",
            json!({"custom:amount": 1500, "custom:status": "pending"}),
        );
        let ctx = eval_context_with_node(&node);

        // "custom:amount" should be accessible as "node.amount"
        let result = Program::compile("node.amount == 1500")
            .unwrap()
            .execute(&ctx);
        assert_eq!(result, Ok(Value::Bool(true)));
    }

    #[test]
    fn node_to_cel_excludes_internal_bookkeeping_keys_at_the_top_level() {
        // `_`-prefixed internal-bookkeeping keys (`_playbookChainDepth`,
        // `_seed`, `_schemaVersion`, ...) are stored at the TOP level of
        // `properties`, never nested under the node's own type namespace
        // (`NodeService::normalize_flat_properties_to_namespace` keeps them
        // there regardless of `node_type`). That is the `else` branch of
        // `node_to_cel_value`'s property loop -- the one that previously had
        // no `_`-prefix filter, unlike the type-namespace-unwrap branch.
        let node = test_node(
            "task",
            json!({"status": "open", "_playbookChainDepth": 4, "_seed": {"tier": "starter"}}),
        );

        let cel = node_to_cel_value(&node);
        let map = match cel {
            Value::Map(m) => m,
            other => panic!("expected Map, got {:?}", other),
        };

        assert!(
            map.map.contains_key(&key("status")),
            "an ordinary property must still be present"
        );
        assert!(
            !map.map.contains_key(&key("_playbookChainDepth")),
            "internal bookkeeping key must not leak into the CEL map"
        );
        assert!(
            !map.map.contains_key(&key("_seed")),
            "internal bookkeeping key must not leak into the CEL map"
        );
    }

    // -- Condition evaluation tests --

    #[tokio::test]
    async fn empty_conditions_pass() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        assert_eq!(
            evaluate_conditions(&[], &node, &event, None).await,
            ConditionResult::Pass
        );
    }

    #[tokio::test]
    async fn simple_true_condition_passes() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        let result =
            evaluate_conditions(&conds(&["node.status == 'open'"]), &node, &event, None).await;
        assert_eq!(result, ConditionResult::Pass);
    }

    #[tokio::test]
    async fn simple_false_condition_fails() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        let result =
            evaluate_conditions(&conds(&["node.status == 'done'"]), &node, &event, None).await;
        assert_eq!(result, ConditionResult::Fail { condition_index: 0 });
    }

    #[tokio::test]
    async fn multiple_conditions_all_pass() {
        let node = test_node("task", json!({"status": "open", "priority": "high"}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &conds(&["node.status == 'open'", "node.priority == 'high'"]),
            &node,
            &event,
            None,
        )
        .await;
        assert_eq!(result, ConditionResult::Pass);
    }

    #[tokio::test]
    async fn multiple_conditions_short_circuit_on_first_failure() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &conds(&["node.status == 'done'", "node.nonexistent == true"]),
            &node,
            &event,
            None,
        )
        .await;
        // Should fail on condition 0, not 1
        assert_eq!(result, ConditionResult::Fail { condition_index: 0 });
    }

    #[tokio::test]
    async fn missing_property_evaluates_to_false() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &conds(&["node.nonexistent_field == 'something'"]),
            &node,
            &event,
            None,
        )
        .await;
        assert_eq!(result, ConditionResult::Fail { condition_index: 0 });
    }

    #[tokio::test]
    async fn non_boolean_result_treated_as_failure() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        // Expression returns a string, not a boolean
        let result = evaluate_conditions(&conds(&["node.status"]), &node, &event, None).await;
        assert_eq!(result, ConditionResult::Fail { condition_index: 0 });
    }

    // -- PropertyChanged trigger context tests --

    #[tokio::test]
    async fn property_changed_trigger_context() {
        let node = test_node("task", json!({"status": "done"}));
        let event = node_updated_event(
            "task",
            vec![PropertyChange {
                key: "status".to_string(),
                old_value: Some(json!("open")),
                new_value: Some(json!("done")),
            }],
        );

        let result = evaluate_conditions(
            &conds(&["trigger.property.old_value == 'open'"]),
            &node,
            &event,
            None,
        )
        .await;
        assert_eq!(result, ConditionResult::Pass);

        let result = evaluate_conditions(
            &conds(&["trigger.property.new_value == 'done'"]),
            &node,
            &event,
            None,
        )
        .await;
        assert_eq!(result, ConditionResult::Pass);
    }

    // -- Custom function tests --

    #[tokio::test]
    async fn today_function_returns_date_string() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        // today() should return a string matching YYYY-MM-DD pattern
        let result =
            evaluate_conditions(&conds(&["size(today()) == 10"]), &node, &event, None).await;
        assert_eq!(result, ConditionResult::Pass);
    }

    #[tokio::test]
    async fn days_since_past_date() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        // A date far in the past should have days_since > 0
        let result = evaluate_conditions(
            &conds(&["days_since('2020-01-01') > 0"]),
            &node,
            &event,
            None,
        )
        .await;
        assert_eq!(result, ConditionResult::Pass);
    }

    #[tokio::test]
    async fn days_until_future_date() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        // A date far in the future should have days_until > 0
        let result = evaluate_conditions(
            &conds(&["days_until('2099-12-31') > 0"]),
            &node,
            &event,
            None,
        )
        .await;
        assert_eq!(result, ConditionResult::Pass);
    }

    #[tokio::test]
    async fn days_since_invalid_date_evaluates_to_false() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        // Invalid date string should cause function error → condition fails
        let result = evaluate_conditions(
            &conds(&["days_since('not-a-date') > 0"]),
            &node,
            &event,
            None,
        )
        .await;
        assert_eq!(result, ConditionResult::Fail { condition_index: 0 });
    }

    #[test]
    fn add_days_shifts_a_bare_date_forward() {
        assert_eq!(compute_add_days("2026-01-01", 14).unwrap(), "2026-01-15");
    }

    #[test]
    fn add_days_shifts_a_bare_date_backward() {
        assert_eq!(compute_add_days("2026-01-15", -14).unwrap(), "2026-01-01");
    }

    #[test]
    fn add_days_zero_offset_is_a_no_op() {
        assert_eq!(compute_add_days("2026-03-01", 0).unwrap(), "2026-03-01");
    }

    #[test]
    fn add_days_preserves_full_datetime_format_and_time_of_day() {
        // Full RFC 3339 input keeps its time-of-day and offset -- only the
        // calendar date shifts.
        let result = compute_add_days("2026-01-01T10:30:00Z", 5).unwrap();
        assert!(
            result.starts_with("2026-01-06T10:30:00"),
            "expected shifted datetime to start with 2026-01-06T10:30:00, got {result}"
        );
    }

    #[test]
    fn add_days_crosses_month_and_year_boundaries() {
        assert_eq!(compute_add_days("2026-12-25", 10).unwrap(), "2027-01-04");
    }

    #[test]
    fn add_days_invalid_date_string_is_an_error() {
        let err = compute_add_days("not-a-date", 5).unwrap_err();
        assert!(err.to_string().contains("invalid date string"));
    }

    #[test]
    fn add_days_extreme_offset_is_a_clean_error_not_a_panic() {
        // i64::MAX days would overflow chrono's internal representation --
        // this must be a normal `Err`, never a panic, since `n` ultimately
        // comes from user-authored play content.
        let err = compute_add_days("2026-01-01", i64::MAX).unwrap_err();
        assert!(err.to_string().contains("out of range"));

        let err = compute_add_days("2026-01-01", i64::MIN).unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    #[tokio::test]
    async fn add_days_usable_from_a_cel_condition() {
        let node = test_node("task", json!({"start_date": "2026-01-01"}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &conds(&["add_days(node.start_date, 14) == '2026-01-15'"]),
            &node,
            &event,
            None,
        )
        .await;
        assert_eq!(result, ConditionResult::Pass);
    }

    #[tokio::test]
    async fn add_days_invalid_date_evaluates_to_false_in_a_condition() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &conds(&["add_days('not-a-date', 5) == '2026-01-01'"]),
            &node,
            &event,
            None,
        )
        .await;
        assert_eq!(result, ConditionResult::Fail { condition_index: 0 });
    }

    #[test]
    fn add_days_is_not_in_the_non_deterministic_function_list() {
        // add_days never reads wall-clock time -- it must stay usable in an
        // invariant rule's conditions (ADR-060 §2), unlike today/days_since/
        // days_until.
        assert!(!NON_DETERMINISTIC_FUNCTIONS.contains(&"add_days"));
    }

    // -- Numeric comparison tests --

    #[tokio::test]
    async fn numeric_property_comparison() {
        let node = test_node("invoice", json!({"amount": 1500}));
        let event = node_created_event("invoice");
        let result =
            evaluate_conditions(&conds(&["node.amount > 1000"]), &node, &event, None).await;
        assert_eq!(result, ConditionResult::Pass);
    }

    // -- Boolean property tests --

    #[tokio::test]
    async fn boolean_property_evaluation() {
        let node = test_node("task", json!({"archived": false}));
        let event = node_created_event("task");
        let result =
            evaluate_conditions(&conds(&["node.archived == false"]), &node, &event, None).await;
        assert_eq!(result, ConditionResult::Pass);
    }

    // -- Helper for direct node-in-context testing --

    fn eval_context_with_node(node: &Node) -> Context<'static> {
        let mut ctx = Context::default();
        ctx.add_variable_from_value("node", node_to_cel_value(node));
        ctx
    }
}
