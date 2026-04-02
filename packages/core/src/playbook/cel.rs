//! CEL Evaluator for the Playbook Engine
//!
//! Compiles and evaluates CEL (Common Expression Language) conditions against
//! trigger nodes. Uses the `cel-interpreter` crate with custom variable resolution
//! and built-in functions for date operations.
//!
//! # Wire Format
//!
//! Before evaluation, nodes are converted to wire format via `flatten_properties_for_api`.
//! The CEL context sees flat properties: `node.status`, not `node.properties.task.status`.
//! Namespace prefixes are stripped: `custom:amount` → `node.amount`.
//!
//! # Missing Path Behavior
//!
//! In conditions, a missing path evaluates to `false` — the condition is not met,
//! but the playbook stays active. This matches the spec: relationships are built
//! progressively, so a condition checking `node.story.epic.status` should wait
//! until the chain exists, not disable itself.
//!
//! # Phase 3 Scope
//!
//! This phase covers property-level condition evaluation on the trigger node.
//! Graph traversal (dot-path relationship walking like `node.story.epic.status`)
//! requires async NodeService calls and is deferred to a future phase.

use cel_interpreter::{Context, ExecutionError, Program, Value};
use chrono::{Local, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::db::events::DomainEvent;
use crate::models::Node;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors from CEL compilation (parse-time).
#[derive(Debug, Clone)]
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
    /// A condition had a compile error (should have been caught at save time).
    CompileError {
        condition_index: usize,
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------

/// Compile a CEL expression string into a reusable Program.
///
/// Used at playbook save time for validation and at runtime for evaluation.
pub fn compile_condition(expr: &str) -> Result<Program, CelCompileError> {
    Program::compile(expr).map_err(|e| CelCompileError {
        expression: expr.to_string(),
        message: e.to_string(),
    })
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
fn json_to_cel(json: &serde_json::Value) -> Value {
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
            Value::Map(cel_interpreter::objects::Map {
                map: Arc::new(map),
            })
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

    // Flatten properties into the map, stripping namespace prefixes
    if let Some(obj) = node.properties.as_object() {
        for (k, v) in obj {
            // Strip namespace prefix: "custom:amount" → "amount"
            let bare_key = k
                .find(':')
                .map(|i| &k[i + 1..])
                .unwrap_or(k.as_str());
            map.insert(key(bare_key), json_to_cel(v));
        }
    }

    Value::Map(cel_interpreter::objects::Map {
        map: Arc::new(map),
    })
}

/// Convenience: create a CEL Map Key from a string.
fn key(s: &str) -> cel_interpreter::objects::Key {
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
pub fn build_condition_context<'a>(
    node: &Node,
    event: &DomainEvent,
) -> Context<'a> {
    let mut ctx = Context::default();

    // `node` variable — the trigger node in wire format
    ctx.add_variable_from_value("node", node_to_cel_value(node));

    // `trigger` variable — event-specific context
    let mut trigger_map: HashMap<cel_interpreter::objects::Key, Value> = HashMap::new();

    // Add trigger.node as an alias
    trigger_map.insert(key("node"), node_to_cel_value(node));

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
                    pc.old_value.as_ref().map(json_to_cel).unwrap_or(Value::Null),
                );
                m.insert(
                    key("new_value"),
                    pc.new_value.as_ref().map(json_to_cel).unwrap_or(Value::Null),
                );
                Value::Map(cel_interpreter::objects::Map {
                    map: Arc::new(m),
                })
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

    ctx
}

// ---------------------------------------------------------------------------
// Custom CEL Functions
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Condition Evaluation
// ---------------------------------------------------------------------------

/// Evaluate all conditions for a rule against the trigger node and event.
///
/// Conditions are evaluated in order with short-circuit on first failure.
/// An empty conditions list results in `ConditionResult::Pass`.
///
/// Missing path errors (NoSuchKey, UndeclaredReference) evaluate to `false`
/// per the spec — the condition fails but the playbook remains active.
pub fn evaluate_conditions(
    conditions: &[String],
    node: &Node,
    event: &DomainEvent,
) -> ConditionResult {
    if conditions.is_empty() {
        return ConditionResult::Pass;
    }

    let ctx = build_condition_context(node, event);

    for (i, condition) in conditions.iter().enumerate() {
        let program = match compile_condition(condition) {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    "CEL compile error in condition[{}] '{}': {}",
                    i, condition, e.message
                );
                return ConditionResult::CompileError {
                    condition_index: i,
                    message: e.message,
                };
            }
        };

        match program.execute(&ctx) {
            Ok(Value::Bool(true)) => {
                debug!("Condition[{}] passed: {}", i, condition);
                // Continue to next condition
            }
            Ok(Value::Bool(false)) => {
                debug!("Condition[{}] failed (false): {}", i, condition);
                return ConditionResult::Fail {
                    condition_index: i,
                };
            }
            Ok(other) => {
                // Non-boolean result — treat as failure
                debug!(
                    "Condition[{}] returned non-boolean {:?}: {}",
                    i, other, condition
                );
                return ConditionResult::Fail {
                    condition_index: i,
                };
            }
            Err(ExecutionError::NoSuchKey(_))
            | Err(ExecutionError::UndeclaredReference(_)) => {
                // Missing path → false (spec: condition not met, playbook stays active)
                debug!(
                    "Condition[{}] has missing path (evaluates to false): {}",
                    i, condition
                );
                return ConditionResult::Fail {
                    condition_index: i,
                };
            }
            Err(e) => {
                // Other runtime errors — treat as condition failure, not compile error
                // The playbook stays active; it's the condition that doesn't match.
                debug!(
                    "Condition[{}] runtime error (evaluates to false): {} — {}",
                    i, condition, e
                );
                return ConditionResult::Fail {
                    condition_index: i,
                };
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
    fn node_updated_event(
        node_type: &str,
        changes: Vec<PropertyChange>,
    ) -> DomainEvent {
        DomainEvent::NodeUpdated {
            node_type: node_type.to_string(),
            node_id: "test-node-1".to_string(),
            changed_properties: changes,
        }
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
        assert_eq!(json_to_cel(&json!(3.14)), Value::Float(3.14));
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
        assert_eq!(result, Ok(Value::String(Arc::new("test-node-1".to_string()))));

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

    // -- Condition evaluation tests --

    #[test]
    fn empty_conditions_pass() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        assert_eq!(
            evaluate_conditions(&[], &node, &event),
            ConditionResult::Pass
        );
    }

    #[test]
    fn simple_true_condition_passes() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &["node.status == 'open'".to_string()],
            &node,
            &event,
        );
        assert_eq!(result, ConditionResult::Pass);
    }

    #[test]
    fn simple_false_condition_fails() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &["node.status == 'done'".to_string()],
            &node,
            &event,
        );
        assert_eq!(
            result,
            ConditionResult::Fail {
                condition_index: 0
            }
        );
    }

    #[test]
    fn multiple_conditions_all_pass() {
        let node = test_node("task", json!({"status": "open", "priority": "high"}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &[
                "node.status == 'open'".to_string(),
                "node.priority == 'high'".to_string(),
            ],
            &node,
            &event,
        );
        assert_eq!(result, ConditionResult::Pass);
    }

    #[test]
    fn multiple_conditions_short_circuit_on_first_failure() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &[
                "node.status == 'done'".to_string(),
                "node.nonexistent == true".to_string(),
            ],
            &node,
            &event,
        );
        // Should fail on condition 0, not 1
        assert_eq!(
            result,
            ConditionResult::Fail {
                condition_index: 0
            }
        );
    }

    #[test]
    fn missing_property_evaluates_to_false() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &["node.nonexistent_field == 'something'".to_string()],
            &node,
            &event,
        );
        assert_eq!(
            result,
            ConditionResult::Fail {
                condition_index: 0
            }
        );
    }

    #[test]
    fn compile_error_in_condition() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &["invalid @@@ syntax".to_string()],
            &node,
            &event,
        );
        match result {
            ConditionResult::CompileError {
                condition_index: 0,
                ..
            } => {} // OK
            other => panic!("expected CompileError, got {:?}", other),
        }
    }

    #[test]
    fn non_boolean_result_treated_as_failure() {
        let node = test_node("task", json!({"status": "open"}));
        let event = node_created_event("task");
        // Expression returns a string, not a boolean
        let result = evaluate_conditions(
            &["node.status".to_string()],
            &node,
            &event,
        );
        assert_eq!(
            result,
            ConditionResult::Fail {
                condition_index: 0
            }
        );
    }

    // -- PropertyChanged trigger context tests --

    #[test]
    fn property_changed_trigger_context() {
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
            &["trigger.property.old_value == 'open'".to_string()],
            &node,
            &event,
        );
        assert_eq!(result, ConditionResult::Pass);

        let result = evaluate_conditions(
            &["trigger.property.new_value == 'done'".to_string()],
            &node,
            &event,
        );
        assert_eq!(result, ConditionResult::Pass);
    }

    // -- Custom function tests --

    #[test]
    fn today_function_returns_date_string() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        // today() should return a string matching YYYY-MM-DD pattern
        let result = evaluate_conditions(
            &["size(today()) == 10".to_string()],
            &node,
            &event,
        );
        assert_eq!(result, ConditionResult::Pass);
    }

    #[test]
    fn days_since_past_date() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        // A date far in the past should have days_since > 0
        let result = evaluate_conditions(
            &["days_since('2020-01-01') > 0".to_string()],
            &node,
            &event,
        );
        assert_eq!(result, ConditionResult::Pass);
    }

    #[test]
    fn days_until_future_date() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        // A date far in the future should have days_until > 0
        let result = evaluate_conditions(
            &["days_until('2099-12-31') > 0".to_string()],
            &node,
            &event,
        );
        assert_eq!(result, ConditionResult::Pass);
    }

    #[test]
    fn days_since_invalid_date_evaluates_to_false() {
        let node = test_node("task", json!({}));
        let event = node_created_event("task");
        // Invalid date string should cause function error → condition fails
        let result = evaluate_conditions(
            &["days_since('not-a-date') > 0".to_string()],
            &node,
            &event,
        );
        assert_eq!(
            result,
            ConditionResult::Fail {
                condition_index: 0
            }
        );
    }

    // -- Numeric comparison tests --

    #[test]
    fn numeric_property_comparison() {
        let node = test_node("invoice", json!({"amount": 1500}));
        let event = node_created_event("invoice");
        let result = evaluate_conditions(
            &["node.amount > 1000".to_string()],
            &node,
            &event,
        );
        assert_eq!(result, ConditionResult::Pass);
    }

    // -- Boolean property tests --

    #[test]
    fn boolean_property_evaluation() {
        let node = test_node("task", json!({"archived": false}));
        let event = node_created_event("task");
        let result = evaluate_conditions(
            &["node.archived == false".to_string()],
            &node,
            &event,
        );
        assert_eq!(result, ConditionResult::Pass);
    }

    // -- Helper for direct node-in-context testing --

    fn eval_context_with_node(node: &Node) -> Context<'static> {
        let mut ctx = Context::default();
        ctx.add_variable_from_value("node", node_to_cel_value(node));
        ctx
    }
}
