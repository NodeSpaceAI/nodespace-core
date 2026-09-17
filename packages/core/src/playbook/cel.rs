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

/// The scope a Play's conditions are evaluated at (ADR-078).
///
/// Built once per rule dispatch by the engine, which has store access; CEL
/// evaluation itself stays synchronous and schema-free. Carries the trigger's
/// registered type, the buckets visible at it, and the field definitions both
/// scopes declare — the latter two being what `maps_to` resolution needs.
#[derive(Debug, Clone)]
pub struct CelScope {
    /// The `node_type` the rule's trigger was registered against.
    pub scope_type: String,
    /// That type's own chain, nearest-first — the buckets in scope.
    pub chain: Vec<String>,
    /// Effective fields at the trigger's scope: the vocabulary a condition
    /// authored against it can refer to.
    pub scope_fields: Vec<crate::models::SchemaField>,
    /// Effective fields at the concrete node's scope, where `maps_to` lives.
    pub node_fields: Vec<crate::models::SchemaField>,
}

impl CelScope {
    /// Whether this scope differs from the node's own, i.e. whether values
    /// could need resolving. False for a node of exactly the registered type,
    /// which is already reading natively.
    fn resolves(&self, node_type: &str) -> bool {
        node_type != self.scope_type
    }

    /// Whether the reading scope declares this field — i.e. whether a
    /// condition authored at this scope is entitled to see it.
    fn declares(&self, name: &str) -> bool {
        self.scope_fields.iter().any(|f| f.name == name)
    }
}

/// The node's own chain: its type plus the scope's ancestors, so the full
/// stored view is assembled before projection narrows it.
fn node_own_chain<'a>(node: &'a Node, scope: &'a CelScope) -> Vec<&'a str> {
    let mut chain: Vec<&str> = vec![node.node_type.as_str()];
    for ancestor in &scope.chain {
        if ancestor != &node.node_type {
            chain.push(ancestor.as_str());
        }
    }
    chain
}

/// Keys the CEL map carries that are node metadata rather than schema fields.
fn is_core_key(key: &str) -> bool {
    matches!(
        key,
        "id" | "node_type" | "content" | "version" | "lifecycle_status"
    )
}

/// A node's CEL value, projected and value-resolved at `scope`.
///
/// Projection picks which fields exist; `maps_to` decides what a surviving
/// field reads as. A value that cannot be expressed at the scope is dropped
/// rather than surfaced raw — handing a base-scoped condition a value it has
/// never heard of is the hazard `maps_to` exists to prevent, and an absent key
/// makes the condition simply not match.
fn scoped_node_value(node: &Node, scope: Option<&CelScope>) -> Value {
    let Some(scope) = scope else {
        return node_to_cel_value(node);
    };

    if !scope.resolves(&node.node_type) {
        let chain: Vec<&str> = scope.chain.iter().map(String::as_str).collect();
        return node_to_cel_value_at_scope(node, &chain);
    }

    // Build from the NODE's own full view, then keep only what the reading
    // scope declares — rather than reading only the scope's buckets.
    //
    // Which bucket a field physically lives in is not a reliable proxy for
    // which scope owns it: extending an inherited enum materializes the field
    // onto the extending schema, moving its storage to the subtype's bucket
    // while it remains a field of the base. Reading the scope's buckets alone
    // would lose exactly the fields `maps_to` exists to translate.
    let own_chain = node_own_chain(node, scope);
    let projected = node_to_cel_value_at_scope(node, &own_chain);

    let Value::Map(map) = &projected else {
        return projected;
    };

    let mut out: HashMap<cel_interpreter::objects::Key, Value> = HashMap::new();
    for (k, v) in map.map.iter() {
        let cel_interpreter::objects::Key::String(field) = k else {
            out.insert(k.clone(), v.clone());
            continue;
        };
        // Projection: a field the reading scope does not declare is absent,
        // so a base-scoped Play cannot come to depend on a subtype's own
        // field. Core keys (`id`, `content`, …) are not schema fields and are
        // always kept.
        if !is_core_key(field) && !scope.declares(field) {
            continue;
        }
        // Only string values carry an enum vocabulary to resolve through.
        let Value::String(stored) = v else {
            out.insert(k.clone(), v.clone());
            continue;
        };
        match crate::schema::extends_chain::resolve_value_at_scope(
            field,
            stored,
            &scope.node_fields,
            &scope.scope_fields,
        ) {
            Some(resolved) => {
                out.insert(k.clone(), Value::String(Arc::new(resolved)));
            }
            None => {
                // `resolve_value_at_scope` returns None for EVERY string that
                // is not a declared enum value at the reading scope — which
                // includes every plain `string` field and the core keys
                // (`id`, `node_type`, `content`, `lifecycle_status`) this map
                // carries. So `None` alone does not mean "unresolvable enum".
                //
                // The `field_is_enum` guard below IS the mechanism that tells
                // the two apart, not a redundant belt-and-braces check.
                // Removing it would silently strip `node.id`, `node.content`
                // and every string property from every base-scoped Play's
                // environment.
                //
                // An enum whose value has no meaning at this scope is dropped:
                // a condition then simply does not match, rather than
                // comparing against a value its author never knew about.
                if !field_is_enum(&scope.scope_fields, field) {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
    }

    Value::Map(cel_interpreter::objects::Map { map: Arc::new(out) })
}

/// Whether the named field is an enum at this scope.
fn field_is_enum(fields: &[crate::models::SchemaField], name: &str) -> bool {
    fields
        .iter()
        .any(|f| f.name == name && f.field_type == "enum")
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
    node_to_cel_value_at_scope(node, std::slice::from_ref(&node.node_type.as_str()))
}

/// Build a CEL `Value` from a Node, projected to an explicit scope chain
/// (ADR-078).
///
/// The general form of [`node_to_cel_value`], which is the node's-own-scope
/// case. A Play registered against a base type evaluates its conditions at
/// *that* scope, so a Play on `task` firing against an `issue` node passes
/// `["task"]` and sees task's fields only — `node.severity` does not resolve
/// there. That is deliberate: a base-scoped Play then behaves identically
/// whether it fired on a plain task or a subtype, and cannot come to depend on
/// a field only some of its matches carry.
///
/// Projection also closes a hazard the single-bucket version had under
/// `extends`: its fallback branch treats any non-matching, non-`_` top-level
/// key as a flat property, so an unprojected sibling bucket would be inserted
/// wholesale as a nested map named after the ancestor type (`node.task`),
/// rather than dropped or unwrapped. Walking an explicit chain removes the
/// branch's ability to see a sibling bucket at all.
pub fn node_to_cel_value_at_scope(node: &Node, scope_chain: &[&str]) -> Value {
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
        // Walk the scope chain first, nearest scope wins. Done ahead of the
        // loop below so a bucket in the chain is never also seen by the
        // flat-property branch.
        for scope in scope_chain {
            let Some(bucket) = obj.get(*scope).and_then(|v| v.as_object()) else {
                continue;
            };
            for (ik, iv) in bucket {
                // Skip internal fields like _schema_version
                if !ik.starts_with('_') {
                    map.entry(key(ik)).or_insert_with(|| json_to_cel(iv));
                }
            }
        }

        for (k, v) in obj {
            if scope_chain.contains(&k.as_str()) {
                // Already unwrapped above.
                continue;
            } else if v.is_object() && obj.contains_key(&node.node_type) {
                // A sibling type-namespace bucket on a node that is in
                // storage shape: an ancestor's bucket outside this read's
                // scope, or a dormant namespace from a type change. Either
                // way it is not a flat property of this node — inserting it
                // would surface `node.task` as a nested map.
                continue;
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
    build_condition_context_with_resolved(node, event, &HashMap::new(), None)
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
    scope: Option<&CelScope>,
) -> Context<'a> {
    let mut ctx = Context::default();

    // `node` variable — the trigger node in wire format, enriched with resolved paths
    let base_node = scoped_node_value(node, scope);
    let enriched_node = inject_resolved_paths(&base_node, resolved_values);
    ctx.add_variable_from_value("node", enriched_node);

    // `trigger` variable — event-specific context
    let mut trigger_map: HashMap<cel_interpreter::objects::Key, Value> = HashMap::new();

    // Add trigger.node as an alias (also enriched with resolved paths)
    let trigger_node_value =
        inject_resolved_paths(&scoped_node_value(node, scope), resolved_values);
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
    evaluate_conditions_at_scope(conditions, node, event, resolver, None).await
}

/// [`evaluate_conditions`], evaluated at an explicit trigger scope (ADR-078).
///
/// `scope` is the `node_type` the rule's trigger was registered against, with
/// the effective field sets needed to read at it. A Play registered on `task`
/// firing against an `issue` sees task's fields only — `node.severity` does
/// not resolve there — and an extended enum value reads as the base-scope
/// value it maps to, so `node.status == 'todo'` matches a node storing
/// `backlog`.
///
/// That is the point of scoping rather than a limitation of it: a base-scoped
/// Play then behaves identically whether it fired on a plain task or a
/// subtype, and cannot come to depend on a field only some of its matches
/// carry. `None` evaluates at the node's own scope, which is every Play in a
/// database where nothing declares `extends`.
pub async fn evaluate_conditions_at_scope(
    conditions: &[CompiledCondition],
    node: &Node,
    event: &DomainEvent,
    resolver: Option<&mut GraphResolver>,
    scope: Option<&CelScope>,
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

    let ctx = build_condition_context_with_resolved(node, event, &resolved_values, scope);

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
