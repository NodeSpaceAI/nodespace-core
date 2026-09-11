//! Save-Time Validation for Plays (Phase 7)
//!
//! Validates play rule definitions before persisting. Reuses the CEL parser
//! from `cel.rs` — no divergence between what validates and what executes.
//!
//! CEL condition syntax is validated earlier, by `parse_rule` — a `ParsedRule`
//! cannot exist with an uncompiled condition. This module validates everything
//! that depends on knowing the schema graph.
//!
//! # Checks performed
//!
//! 1. All referenced `node_type` values must exist as schema nodes
//! 2. All referenced `version` values in action params must match the schema's `schema_version`
//! 3. All property paths in conditions resolve against the schema graph
//! 4. All relationship types in actions must exist on the referenced schemas
//!
//! If any check fails, the play is not saved. All errors are collected
//! (not short-circuited) so the caller can present every issue at once.

use crate::models::SchemaNode;
use crate::playbook::path_extractor;
use crate::playbook::types::{
    ActionType, GraphEventType, ParsedAction, ParsedRule, ParsedTrigger, RuleClass,
};
use crate::services::NodeService;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tracing::debug;

// ---------------------------------------------------------------------------
// Validation Errors
// ---------------------------------------------------------------------------

/// A single validation error found during save-time checks.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayValidationError {
    /// A referenced node_type does not exist as a schema node.
    UnknownNodeType {
        node_type: String,
        /// Where the reference was found (e.g., "rule[0].trigger", "rule[1].action[2]")
        location: String,
    },
    /// A `version` value in an action doesn't match the schema's `schema_version`.
    VersionMismatch {
        node_type: String,
        declared_version: String,
        actual_version: u32,
        location: String,
    },
    /// A relationship type in an action doesn't exist on the referenced schema.
    UnknownRelationshipType {
        relationship_type: String,
        node_type: String,
        location: String,
    },
    /// A required param is missing from an action definition.
    MissingActionParam { param: String, location: String },
    /// A dot-path in a condition references a field or relationship that doesn't
    /// exist on the schema graph.
    BrokenPath {
        path: String,
        segment: String,
        message: String,
        location: String,
    },
    /// A `scheduled` trigger's cron expression failed to parse.
    InvalidCronExpression {
        cron: String,
        message: String,
        location: String,
    },
    /// An invariant rule (ADR-060) contains an action that is not a local graph
    /// write. Invariant rules run inside the creating transaction, which cannot
    /// await an LLM call, network request, PTY, or external service.
    InvariantNonLocalAction { action: String, location: String },
    /// An invariant rule's condition uses a non-deterministic function (a
    /// wall-clock read such as `today`/`days_since`/`days_until`). Two devices
    /// reasoning about the same node must agree on what the invariant requires.
    InvariantNonDeterministic { function: String, location: String },
    /// An invariant rule's action addresses a node outside the trigger's graph
    /// scope — a literal/arbitrary node id rather than a binding derived from the
    /// trigger node or a prior action's output.
    InvariantOutOfScopeTarget {
        action: String,
        param: String,
        value: String,
        location: String,
    },
    /// An invariant rule's own action would re-satisfy its own trigger, forming a
    /// chain. Invariant rules must be non-chaining (depth 1) — unbounded
    /// recursion inside a transaction is unacceptable.
    InvariantChaining {
        action: String,
        trigger: String,
        location: String,
    },
}

impl std::fmt::Display for PlayValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownNodeType {
                node_type,
                location,
            } => write!(f, "unknown node_type '{}' at {}", node_type, location),
            Self::VersionMismatch {
                node_type,
                declared_version,
                actual_version,
                location,
            } => write!(
                f,
                "version mismatch for '{}' at {}: declared '{}', schema has {}",
                node_type, location, declared_version, actual_version
            ),
            Self::UnknownRelationshipType {
                relationship_type,
                node_type,
                location,
            } => write!(
                f,
                "unknown relationship_type '{}' on schema '{}' at {}",
                relationship_type, node_type, location
            ),
            Self::MissingActionParam { param, location } => {
                write!(f, "missing required param '{}' at {}", param, location)
            }
            Self::BrokenPath {
                path,
                segment,
                message,
                location,
            } => {
                write!(
                    f,
                    "broken path '{}' at {}: segment '{}' — {}",
                    path, location, segment, message
                )
            }
            Self::InvalidCronExpression {
                cron,
                message,
                location,
            } => write!(
                f,
                "invalid cron expression '{}' at {}: {}",
                cron, location, message
            ),
            Self::InvariantNonLocalAction { action, location } => write!(
                f,
                "invariant rule action '{}' at {} is not a local write \
                 (invariant rules may not call an LLM, the network, a PTY, or an external service)",
                action, location
            ),
            Self::InvariantNonDeterministic { function, location } => write!(
                f,
                "invariant rule uses non-deterministic function '{}' at {} \
                 (invariant rules must be deterministic — no wall-clock reads or random values)",
                function, location
            ),
            Self::InvariantOutOfScopeTarget {
                action,
                param,
                value,
                location,
            } => write!(
                f,
                "invariant rule action '{}' at {} targets an out-of-scope node via {} = '{}' \
                 (invariant actions may only address the trigger node or nodes it references, \
                 not a literal/arbitrary node id)",
                action, location, param, value
            ),
            Self::InvariantChaining {
                action,
                trigger,
                location,
            } => write!(
                f,
                "invariant rule action '{}' at {} would re-satisfy its own '{}' trigger \
                 (invariant rules must be non-chaining, depth 1)",
                action, location, trigger
            ),
        }
    }
}

/// Result of play validation: either Ok or a non-empty list of errors.
pub type ValidationResult = Result<(), Vec<PlayValidationError>>;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Validate a set of parsed rules before saving a play.
///
/// Queries schema nodes via `NodeService` to verify node_type existence,
/// schema_version matching, and relationship type existence. CEL condition
/// syntax is already guaranteed valid — `rules` are `ParsedRule`s, which can
/// only be constructed with successfully compiled conditions.
///
/// Returns `Ok(())` if all checks pass, or `Err(Vec<...>)` with all errors found.
pub async fn validate_play(
    rules: &[Arc<ParsedRule>],
    node_service: &NodeService,
) -> ValidationResult {
    let mut errors: Vec<PlayValidationError> = Vec::new();

    // Collect all referenced node_types and fetch schemas once
    let mut schema_cache: HashMap<String, Option<SchemaNode>> = HashMap::new();

    for (rule_idx, rule) in rules.iter().enumerate() {
        // -- Validate trigger node_type --
        let trigger_node_type = trigger_node_type(rule);
        if let Some(nt) = &trigger_node_type {
            ensure_schema_cached(nt, node_service, &mut schema_cache).await;
            if schema_cache
                .get(nt.as_str())
                .and_then(|s| s.as_ref())
                .is_none()
            {
                errors.push(PlayValidationError::UnknownNodeType {
                    node_type: nt.clone(),
                    location: format!("rule[{}].trigger", rule_idx),
                });
            }
        }

        // -- Validate cron expression on scheduled triggers --
        if let ParsedTrigger::Scheduled { cron, .. } = &rule.trigger {
            if let Err(e) = cron::Schedule::from_str(cron) {
                errors.push(PlayValidationError::InvalidCronExpression {
                    cron: cron.clone(),
                    message: e.to_string(),
                    location: format!("rule[{}].trigger", rule_idx),
                });
            }
        }

        // -- Validate CEL condition paths --
        //
        // Conditions are already compiled (and guaranteed valid) by `parse_rule`
        // before a `ParsedRule` can exist, so only schema-aware path validation
        // remains to do here.
        for (cond_idx, condition) in rule.conditions.iter().enumerate() {
            let location = format!("rule[{}].condition[{}]", rule_idx, cond_idx);

            // Schema-aware path validation: extract dot-paths and
            // verify each segment resolves to a field or relationship on the schema graph
            if let Some(nt) = &trigger_node_type {
                if let Ok(extraction) = path_extractor::extract_paths(&condition.source) {
                    for path in &extraction.paths {
                        if path.root == "node" && path.segments.len() > 2 {
                            validate_schema_path(
                                &path.segments,
                                nt,
                                &location,
                                node_service,
                                &mut schema_cache,
                                &mut errors,
                            )
                            .await;
                        }
                    }
                    for coll in &extraction.collections {
                        if coll.collection.root == "node" && coll.collection.segments.len() > 1 {
                            validate_schema_path(
                                &coll.collection.segments,
                                nt,
                                &location,
                                node_service,
                                &mut schema_cache,
                                &mut errors,
                            )
                            .await;
                        }
                    }
                }
            }
        }

        // -- Validate actions --
        for (action_idx, action) in rule.actions.iter().enumerate() {
            let location = format!("rule[{}].action[{}]", rule_idx, action_idx);
            validate_action(
                action,
                &location,
                trigger_node_type.as_deref(),
                node_service,
                &mut schema_cache,
                &mut errors,
            )
            .await;
        }

        // -- Validate invariant-rule eligibility (ADR-060 §2) --
        //
        // Only invariant rules are gated; reactive rules (the default, and every
        // rule authored so far) are unaffected. All checks are static — they
        // inspect the parsed rule, not the schema graph — so no DB lookup is
        // needed here.
        if rule.class == RuleClass::Invariant {
            validate_invariant_eligibility(rule, rule_idx, &mut errors);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract the node_type from a parsed trigger.
fn trigger_node_type(rule: &ParsedRule) -> Option<String> {
    match &rule.trigger {
        ParsedTrigger::GraphEvent { node_type, .. } => Some(node_type.clone()),
        ParsedTrigger::Scheduled { node_type, .. } => Some(node_type.clone()),
    }
}

/// Ensure a schema is in the cache, fetching from DB if not yet loaded.
async fn ensure_schema_cached(
    node_type: &str,
    node_service: &NodeService,
    cache: &mut HashMap<String, Option<SchemaNode>>,
) {
    if cache.contains_key(node_type) {
        return;
    }
    let schema = match node_service.get_schema_node(node_type).await {
        Ok(s) => s,
        Err(e) => {
            debug!(
                "Failed to query schema for '{}': {} — treating as missing",
                node_type, e
            );
            None
        }
    };
    cache.insert(node_type.to_string(), schema);
}

/// Validate a dot-path against the schema graph.
///
/// Walks the path segments starting from the trigger schema, checking each segment:
/// 1. Is it a field on the current schema? → terminal (scalar property)
/// 2. Is it a relationship on the current schema? → follow to target schema
/// 3. Neither → broken path error
///
/// Path format: `["node", "story", "epic", "status"]`
/// - First segment ("node") is skipped (it's the root variable)
/// - Second segment ("story") checked against the trigger schema
/// - Remaining segments checked against subsequent schemas
async fn validate_schema_path(
    segments: &[String],
    trigger_node_type: &str,
    location: &str,
    node_service: &NodeService,
    schema_cache: &mut HashMap<String, Option<SchemaNode>>,
    errors: &mut Vec<PlayValidationError>,
) {
    if segments.len() < 2 {
        return; // Single-segment paths (just "node") don't need validation
    }

    let full_path = segments.join(".");
    let mut current_type = trigger_node_type.to_string();

    // Walk from segments[1] onward (skipping "node")
    for (i, segment) in segments[1..].iter().enumerate() {
        ensure_schema_cached(&current_type, node_service, schema_cache).await;

        let schema = match schema_cache.get(&current_type).and_then(|s| s.as_ref()) {
            Some(s) => s,
            None => {
                // Schema not found — can't validate further
                // (UnknownNodeType error is already reported by trigger validation)
                return;
            }
        };

        // Check if the segment is a field on this schema
        let is_field = schema.fields.iter().any(|f| f.name == *segment);
        if is_field {
            // Fields are terminal — if there are more segments after this, it's broken
            if i + 1 < segments.len() - 1 {
                errors.push(PlayValidationError::BrokenPath {
                    path: full_path.clone(),
                    segment: segment.clone(),
                    message: format!(
                        "'{}' is a field on '{}', not a relationship (cannot traverse further)",
                        segment, current_type
                    ),
                    location: location.to_string(),
                });
            }
            return;
        }

        // Check if the segment is a relationship on this schema
        let relationship = schema.relationships.iter().find(|r| r.name == *segment);
        if let Some(rel) = relationship {
            if let Some(ref target_type) = rel.target_type {
                // Follow the relationship to the target schema
                current_type = target_type.clone();
            } else {
                // Relationship has no target_type — can't traverse further
                if i + 1 < segments.len() - 1 {
                    errors.push(PlayValidationError::BrokenPath {
                        path: full_path.clone(),
                        segment: segment.clone(),
                        message: format!(
                            "relationship '{}' on '{}' has no target_type (cannot traverse further)",
                            segment, current_type
                        ),
                        location: location.to_string(),
                    });
                }
                return;
            }
        } else {
            // Neither a field nor a relationship — broken path
            // But only report if the schema actually exists (to avoid duplicate errors)
            errors.push(PlayValidationError::BrokenPath {
                path: full_path.clone(),
                segment: segment.clone(),
                message: format!(
                    "'{}' is not a field or relationship on schema '{}'",
                    segment, current_type
                ),
                location: location.to_string(),
            });
            return;
        }
    }
}

/// Validate a single action's params.
async fn validate_action(
    action: &ParsedAction,
    location: &str,
    trigger_node_type: Option<&str>,
    node_service: &NodeService,
    schema_cache: &mut HashMap<String, Option<SchemaNode>>,
    errors: &mut Vec<PlayValidationError>,
) {
    match action.action_type {
        ActionType::CreateNode => {
            validate_create_node_action(
                &action.params,
                location,
                node_service,
                schema_cache,
                errors,
            )
            .await;
        }
        ActionType::UpdateNode => {
            // update_node may optionally reference a node_type for type conversion
            if let Some(nt) = action.params.get("node_type").and_then(|v| v.as_str()) {
                ensure_schema_cached(nt, node_service, schema_cache).await;
                if schema_cache.get(nt).and_then(|s| s.as_ref()).is_none() {
                    errors.push(PlayValidationError::UnknownNodeType {
                        node_type: nt.to_string(),
                        location: location.to_string(),
                    });
                }
            }
        }
        ActionType::AddRelationship | ActionType::RemoveRelationship => {
            validate_relationship_action(
                &action.params,
                location,
                trigger_node_type,
                node_service,
                schema_cache,
                errors,
            )
            .await;
        }
    }
}

/// Validate `create_node` action: node_type must exist, version must match.
async fn validate_create_node_action(
    params: &serde_json::Value,
    location: &str,
    node_service: &NodeService,
    schema_cache: &mut HashMap<String, Option<SchemaNode>>,
    errors: &mut Vec<PlayValidationError>,
) {
    // node_type is required
    let node_type = match params.get("node_type").and_then(|v| v.as_str()) {
        Some(nt) => nt,
        None => {
            if params.get("node_type").is_some() {
                // Non-string node_type (e.g., number, object) — can't validate, skip
                return;
            }
            errors.push(PlayValidationError::MissingActionParam {
                param: "node_type".to_string(),
                location: location.to_string(),
            });
            return;
        }
    };

    // Skip validation for binding templates like "{trigger.node.node_type}"
    if node_type.contains('{') {
        return;
    }

    ensure_schema_cached(node_type, node_service, schema_cache).await;

    let schema = match schema_cache.get(node_type).and_then(|s| s.as_ref()) {
        Some(s) => s,
        None => {
            errors.push(PlayValidationError::UnknownNodeType {
                node_type: node_type.to_string(),
                location: location.to_string(),
            });
            return;
        }
    };

    // Check version if declared
    if let Some(version_val) = params.get("version") {
        let owned_str;
        let declared = match version_val.as_str() {
            Some(s) => s,
            None => {
                owned_str = version_val.to_string();
                &owned_str
            }
        };
        // Schema version is a u32; the play may declare it as a string like "1" or "2"
        let declared_num: Option<u32> = declared.parse().ok();
        if declared_num != Some(schema.schema_version) {
            errors.push(PlayValidationError::VersionMismatch {
                node_type: node_type.to_string(),
                declared_version: declared.to_string(),
                actual_version: schema.schema_version,
                location: location.to_string(),
            });
        }
    }
}

/// Validate relationship actions: relationship_type must exist on the trigger's schema.
async fn validate_relationship_action(
    params: &serde_json::Value,
    location: &str,
    trigger_node_type: Option<&str>,
    node_service: &NodeService,
    schema_cache: &mut HashMap<String, Option<SchemaNode>>,
    errors: &mut Vec<PlayValidationError>,
) {
    let rel_type = match params.get("relationship_type").and_then(|v| v.as_str()) {
        Some(rt) => rt,
        None => {
            errors.push(PlayValidationError::MissingActionParam {
                param: "relationship_type".to_string(),
                location: location.to_string(),
            });
            return;
        }
    };

    // Skip validation for binding templates
    if rel_type.contains('{') {
        return;
    }

    // We need the trigger's schema to check if the relationship exists.
    // If the trigger node_type is unknown (already flagged), skip this check.
    let Some(nt) = trigger_node_type else {
        return;
    };

    ensure_schema_cached(nt, node_service, schema_cache).await;

    if let Some(Some(schema)) = schema_cache.get(nt) {
        let rel_exists = schema.relationships.iter().any(|r| r.name == rel_type);

        if !rel_exists {
            errors.push(PlayValidationError::UnknownRelationshipType {
                relationship_type: rel_type.to_string(),
                node_type: nt.to_string(),
                location: location.to_string(),
            });
        }
    }
    // If schema is None, we already flagged the missing node_type
}

// ---------------------------------------------------------------------------
// Invariant-rule eligibility (ADR-060 §2)
// ---------------------------------------------------------------------------

/// Validate that an invariant rule provably terminates inside a transaction, per
/// ADR-060 §2. Any violation is pushed to `errors`, naming the offending action
/// or function. All checks are static (they read the parsed rule only).
///
/// # What is enforced statically here vs. deferred to the runtime guard
///
/// - **Local writes only** — fully enforced via [`ActionType::is_local_write`].
///   Every current action type is a local write, so this passes today; it is a
///   forward-looking gate that rejects any future non-local action type (LLM,
///   network, PTY, external) added to an invariant rule.
/// - **Deterministic** — fully enforced against the only surface that can
///   express non-determinism today: wall-clock CEL functions in the rule's
///   conditions (`today`/`days_since`/`days_until`, see
///   [`crate::playbook::cel::NON_DETERMINISTIC_FUNCTIONS`]). Action params are
///   pure `{binding}` data references (see `actions.rs`) with no function-call
///   surface, and no random-value function is registered anywhere, so conditions
///   are the complete non-deterministic surface.
/// - **Same-graph scope** — enforced by requiring every action *target* node id
///   (`node_id`, `source_id`, `target_id`) to be a `{binding}` derived from the
///   trigger node or a prior action, rejecting a literal/arbitrary node id.
/// - **Non-chaining, depth 1** — the statically decidable *self-chaining* case
///   is enforced here: an action that would re-satisfy the rule's **own**
///   trigger. The fully general form — an action output matching a *different*
///   rule's trigger (in this or another play), and multi-device causal
///   cycles — needs whole-corpus analysis plus the causal depth carried on the
///   event, so it is deferred to the runtime causal-depth guard (ADR-060 §5),
///   built in a later slice. `validate_play` sees only the rules of the
///   play being saved, so cross-play chains are not even visible here.
fn validate_invariant_eligibility(
    rule: &ParsedRule,
    rule_idx: usize,
    errors: &mut Vec<PlayValidationError>,
) {
    // Local writes only.
    for (action_idx, action) in rule.actions.iter().enumerate() {
        if !action.action_type.is_local_write() {
            errors.push(PlayValidationError::InvariantNonLocalAction {
                action: action.action_type.as_str().to_string(),
                location: format!("rule[{}].action[{}]", rule_idx, action_idx),
            });
        }
    }

    // Deterministic — no wall-clock reads in conditions.
    for (cond_idx, condition) in rule.conditions.iter().enumerate() {
        // `condition.source` already compiled successfully to reach a
        // `ParsedRule`, so a parse error here is not expected; if it somehow
        // occurs we simply skip (the determinism check adds no false positives).
        if let Ok(functions) = path_extractor::extract_function_names(&condition.source) {
            for function in functions {
                if crate::playbook::cel::NON_DETERMINISTIC_FUNCTIONS.contains(&function.as_str()) {
                    errors.push(PlayValidationError::InvariantNonDeterministic {
                        function,
                        location: format!("rule[{}].condition[{}]", rule_idx, cond_idx),
                    });
                }
            }
        }
    }

    // Same-graph scope — action targets must be trigger-derived bindings.
    for (action_idx, action) in rule.actions.iter().enumerate() {
        let location = format!("rule[{}].action[{}]", rule_idx, action_idx);
        for param in target_id_params(&action.action_type) {
            if let Some(value) = action.params.get(param).and_then(|v| v.as_str()) {
                if !is_binding_template(value) {
                    errors.push(PlayValidationError::InvariantOutOfScopeTarget {
                        action: action.action_type.as_str().to_string(),
                        param: param.to_string(),
                        value: value.to_string(),
                        location: location.clone(),
                    });
                }
            }
        }
    }

    // Non-chaining, depth 1 — self-trigger detection (statically checkable part).
    check_invariant_self_chaining(rule, rule_idx, errors);
}

/// The action params that name an *existing* node the action addresses.
///
/// `create_node` addresses no existing node (it makes one), so it contributes no
/// target and cannot violate same-graph scope through a target id.
fn target_id_params(action_type: &ActionType) -> &'static [&'static str] {
    match action_type {
        ActionType::CreateNode => &[],
        ActionType::UpdateNode => &["node_id"],
        ActionType::AddRelationship | ActionType::RemoveRelationship => &["source_id", "target_id"],
    }
}

/// Whether a param value references graph state via a `{dot.path}` binding.
///
/// Bindings are rooted at `trigger`, `actions`, or `item` (see `actions.rs`) —
/// all derived from the trigger node or the rule's own prior outputs, so a
/// binding stays within the trigger's graph scope. A plain literal id addresses
/// an arbitrary node whose presence depends on sync state, which ADR-060 §2
/// forbids for invariant rules.
fn is_binding_template(value: &str) -> bool {
    value.contains('{') && value.contains('}')
}

/// Detect the statically checkable case of ADR-060 §2's "non-chaining, depth 1":
/// an invariant rule whose own action re-satisfies its own graph-event trigger.
///
/// Scheduled triggers are not re-satisfied by graph writes, so they are exempt.
/// The general cross-rule / multi-device chaining case is deferred to the runtime
/// causal-depth guard (see [`validate_invariant_eligibility`] docs).
fn check_invariant_self_chaining(
    rule: &ParsedRule,
    rule_idx: usize,
    errors: &mut Vec<PlayValidationError>,
) {
    let ParsedTrigger::GraphEvent { on, node_type, .. } = &rule.trigger else {
        return;
    };

    for (action_idx, action) in rule.actions.iter().enumerate() {
        let re_satisfies = match (on, &action.action_type) {
            // Creating a node of the trigger's own type re-fires `node_created`.
            (GraphEventType::NodeCreated, ActionType::CreateNode) => {
                action_creates_node_type(action, node_type)
            }
            // Updating the trigger node re-fires `property_changed` on it. This is
            // conservative: it flags any update to the trigger node regardless of
            // which property the update touches, because the whole-object
            // `properties` param (with bindings) cannot be statically matched
            // against the trigger's watched `property_key`.
            (GraphEventType::PropertyChanged, ActionType::UpdateNode) => {
                action_targets_trigger_node(action, "node_id")
            }
            // Adding/removing a relationship whose source is the trigger node
            // re-fires the relationship trigger (which matches on source type).
            (GraphEventType::RelationshipAdded, ActionType::AddRelationship) => {
                action_targets_trigger_node(action, "source_id")
            }
            (GraphEventType::RelationshipRemoved, ActionType::RemoveRelationship) => {
                action_targets_trigger_node(action, "source_id")
            }
            _ => false,
        };

        if re_satisfies {
            errors.push(PlayValidationError::InvariantChaining {
                action: action.action_type.as_str().to_string(),
                trigger: graph_event_name(on).to_string(),
                location: format!("rule[{}].action[{}]", rule_idx, action_idx),
            });
        }
    }
}

/// Whether a `create_node` action creates a node of `node_type` — either as a
/// literal `node_type` param, or via the binding that resolves to the trigger
/// node's own type (accepted in both `snake_case` and `camelCase` spellings).
fn action_creates_node_type(action: &ParsedAction, node_type: &str) -> bool {
    match action.params.get("node_type").and_then(|v| v.as_str()) {
        Some(nt) => {
            nt == node_type || nt == "{trigger.node.node_type}" || nt == "{trigger.node.nodeType}"
        }
        None => false,
    }
}

/// Whether an action's `param` targets the trigger node itself via the
/// `{trigger.node.id}` binding.
fn action_targets_trigger_node(action: &ParsedAction, param: &str) -> bool {
    matches!(
        action.params.get(param).and_then(|v| v.as_str()),
        Some("{trigger.node.id}")
    )
}

/// The JSON name of a graph event type, for error messages.
fn graph_event_name(on: &GraphEventType) -> &'static str {
    match on {
        GraphEventType::NodeCreated => "node_created",
        GraphEventType::PropertyChanged => "property_changed",
        GraphEventType::RelationshipAdded => "relationship_added",
        GraphEventType::RelationshipRemoved => "relationship_removed",
    }
}

// ---------------------------------------------------------------------------
// Schema Change Impact Analysis (Phase 2)
// ---------------------------------------------------------------------------

/// A play affected by a schema change, with the specific broken paths.
#[derive(Debug, Clone, PartialEq)]
pub struct AffectedPlay {
    /// The play node ID
    pub play_id: String,
    /// Human-readable play name (from content/title)
    pub play_name: String,
    /// Dot-paths in conditions that traverse through the changed schema
    pub broken_paths: Vec<String>,
}

impl std::fmt::Display for AffectedPlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "play '{}' ({}): paths [{}]",
            self.play_name,
            self.play_id,
            self.broken_paths.join(", ")
        )
    }
}

/// Check which active plays would be affected by a schema change.
///
/// Queries all active play nodes, parses their rules, and checks whether
/// any trigger, condition, or action references the given schema's node_type.
/// Specifically checks:
/// - Trigger node_type matches
/// - Condition dot-paths that traverse through the schema's node_type
/// - `create_node` actions targeting the schema's node_type
/// - Relationship actions whose `relationship_type` matches the schema's node_type
///
/// TODO: This is currently over-broad — any change to a schema (including adding
/// new fields, which can't break plays) triggers the warning. Making this
/// diff-aware (only flag breaking changes like field removal/rename) requires
/// accepting the proposed schema changes as a parameter, which is a larger
/// refactor. The conservative approach is acceptable for v1.
///
/// Returns a list of affected plays with their broken paths.
pub async fn check_schema_change_impact(
    schema_node_type: &str,
    node_service: &NodeService,
) -> Result<Vec<AffectedPlay>, String> {
    use crate::playbook::types::{parse_rule, parse_rules_from_properties};

    let play_nodes = node_service
        .query_nodes_by_type("play", Some("active"))
        .await
        .map_err(|e| format!("Failed to query play nodes: {}", e))?;

    let mut affected = Vec::new();

    for pb_node in &play_nodes {
        let rule_defs = match parse_rules_from_properties(&pb_node.properties) {
            Ok(defs) => defs,
            Err(_) => continue, // Skip unparseable plays
        };

        let mut broken_paths = Vec::new();

        for def in &rule_defs {
            let parsed = match parse_rule(def) {
                Ok(r) => r,
                Err(_) => continue,
            };

            // Check trigger node_type
            let trigger_nt = match &parsed.trigger {
                ParsedTrigger::GraphEvent { node_type, .. } => Some(node_type.as_str()),
                ParsedTrigger::Scheduled { node_type, .. } => Some(node_type.as_str()),
            };
            if trigger_nt == Some(schema_node_type) {
                broken_paths.push(format!("trigger.node_type={}", schema_node_type));
            }

            // Check condition paths
            for condition in &parsed.conditions {
                if let Ok(extraction) = path_extractor::extract_paths(&condition.source) {
                    for path in &extraction.paths {
                        if path.segments.iter().any(|s| s == schema_node_type) {
                            broken_paths.push(path.segments.join("."));
                        }
                    }
                    for coll in &extraction.collections {
                        if coll
                            .collection
                            .segments
                            .iter()
                            .any(|s| s == schema_node_type)
                        {
                            broken_paths.push(coll.collection.segments.join("."));
                        }
                    }
                }
            }

            // Check action params for schema references
            for (i, action) in parsed.actions.iter().enumerate() {
                let action_loc = format!("action[{}]", i);
                match action.action_type {
                    ActionType::CreateNode | ActionType::UpdateNode => {
                        if let Some(nt) = action.params.get("node_type").and_then(|v| v.as_str()) {
                            if nt == schema_node_type {
                                broken_paths.push(format!("{}.node_type={}", action_loc, nt));
                            }
                        }
                    }
                    ActionType::AddRelationship | ActionType::RemoveRelationship => {
                        if let Some(rt) = action
                            .params
                            .get("relationship_type")
                            .and_then(|v| v.as_str())
                        {
                            if rt == schema_node_type {
                                broken_paths
                                    .push(format!("{}.relationship_type={}", action_loc, rt));
                            }
                        }
                        // Also check target_type if it references the schema
                        if let Some(tt) = action.params.get("target_type").and_then(|v| v.as_str())
                        {
                            if tt == schema_node_type {
                                broken_paths.push(format!("{}.target_type={}", action_loc, tt));
                            }
                        }
                    }
                }
            }
        }

        if !broken_paths.is_empty() {
            // Deduplicate paths
            broken_paths.sort();
            broken_paths.dedup();
            affected.push(AffectedPlay {
                play_id: pb_node.id.clone(),
                play_name: pb_node
                    .title
                    .clone()
                    .unwrap_or_else(|| pb_node.content.clone()),
                broken_paths,
            });
        }
    }

    Ok(affected)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::cel::compile_condition;
    use crate::playbook::types::{
        ActionType, GraphEventType, ParsedAction, ParsedRule, ParsedTrigger, RuleClass,
    };

    // -- CEL condition validation tests (no NodeService needed) --

    fn compile_conditions(conditions: Vec<&str>) -> Vec<crate::playbook::cel::CompiledCondition> {
        conditions
            .into_iter()
            .map(|s| crate::playbook::cel::CompiledCondition::compile(s).expect("valid CEL"))
            .collect()
    }

    fn make_rule(
        node_type: &str,
        conditions: Vec<&str>,
        actions: Vec<ParsedAction>,
    ) -> Arc<ParsedRule> {
        Arc::new(ParsedRule {
            name: "test-rule".to_string(),
            class: RuleClass::Reactive,
            trigger: ParsedTrigger::GraphEvent {
                on: GraphEventType::NodeCreated,
                node_type: node_type.to_string(),
                property_key: None,
            },
            conditions: compile_conditions(conditions),
            actions,
        })
    }

    fn make_scheduled_rule(cron: &str, node_type: &str, conditions: Vec<&str>) -> Arc<ParsedRule> {
        Arc::new(ParsedRule {
            name: "test-scheduled-rule".to_string(),
            class: RuleClass::Reactive,
            trigger: ParsedTrigger::Scheduled {
                cron: cron.to_string(),
                node_type: node_type.to_string(),
            },
            conditions: compile_conditions(conditions),
            actions: vec![],
        })
    }

    fn make_create_action(node_type: &str, version: Option<&str>) -> ParsedAction {
        let mut params = serde_json::json!({
            "node_type": node_type,
            "content": "Test",
            "properties": {}
        });
        if let Some(v) = version {
            params["version"] = serde_json::json!(v);
        }
        ParsedAction {
            action_type: ActionType::CreateNode,
            params,
            for_each: None,
        }
    }

    fn make_relationship_action(rel_type: &str) -> ParsedAction {
        ParsedAction {
            action_type: ActionType::AddRelationship,
            params: serde_json::json!({
                "source_id": "{trigger.node.id}",
                "relationship_type": rel_type,
                "target_id": "some-target"
            }),
            for_each: None,
        }
    }

    // -- Pure CEL compile tests (synchronous, no DB) --

    #[test]
    fn test_valid_cel_conditions_compile() {
        assert!(compile_condition("node.status == 'open'").is_ok());
        assert!(compile_condition("node.amount > 1000").is_ok());
        assert!(compile_condition("node.priority == 'high' && node.status == 'open'").is_ok());
    }

    #[test]
    fn test_invalid_cel_condition_detected() {
        let err = compile_condition("1 + + 2").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn test_validation_error_display() {
        let err = PlayValidationError::UnknownNodeType {
            node_type: "foo".to_string(),
            location: "rule[0].trigger".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "unknown node_type 'foo' at rule[0].trigger"
        );

        let err = PlayValidationError::VersionMismatch {
            node_type: "invoice".to_string(),
            declared_version: "3".to_string(),
            actual_version: 2,
            location: "rule[0].action[0]".to_string(),
        };
        assert!(err.to_string().contains("version mismatch"));
        assert!(err.to_string().contains("declared '3'"));
        assert!(err.to_string().contains("schema has 2"));

        let err = PlayValidationError::UnknownRelationshipType {
            relationship_type: "foo_bar".to_string(),
            node_type: "task".to_string(),
            location: "rule[0].action[0]".to_string(),
        };
        assert!(err.to_string().contains("unknown relationship_type"));

        let err = PlayValidationError::MissingActionParam {
            param: "node_type".to_string(),
            location: "rule[0].action[0]".to_string(),
        };
        assert!(err.to_string().contains("missing required param"));
    }

    #[test]
    fn test_trigger_node_type_extraction() {
        let rule = make_rule("task", vec![], vec![]);
        assert_eq!(trigger_node_type(&rule), Some("task".to_string()));

        let rule = make_scheduled_rule("0 * * * * * *", "invoice", vec![]);
        assert_eq!(trigger_node_type(&rule), Some("invoice".to_string()));
    }

    #[test]
    fn test_multiple_cel_errors_collected() {
        // Verify that multiple invalid conditions each produce an error
        let bad1 = compile_condition("1 + + 2");
        let bad2 = compile_condition("3 * * 4");
        assert!(bad1.is_err());
        assert!(bad2.is_err());
    }

    #[test]
    fn test_binding_template_in_node_type_not_validated() {
        // Actions with binding templates like "{trigger.node.node_type}"
        // can't be validated at save time — they should be skipped
        let action = ParsedAction {
            action_type: ActionType::CreateNode,
            params: serde_json::json!({
                "node_type": "{trigger.node.node_type}",
                "content": "Test"
            }),
            for_each: None,
        };
        // The node_type contains '{', so validate_create_node_action should skip
        assert!(action.params["node_type"].as_str().unwrap().contains('{'));
    }

    #[test]
    fn test_binding_template_in_relationship_type_not_validated() {
        let action = make_relationship_action("{trigger.node.rel_type}");
        assert!(action.params["relationship_type"]
            .as_str()
            .unwrap()
            .contains('{'));
    }

    #[test]
    fn test_update_node_action_without_node_type_is_ok() {
        // update_node doesn't require node_type (it's optional for type conversion)
        let action = ParsedAction {
            action_type: ActionType::UpdateNode,
            params: serde_json::json!({
                "node_id": "{trigger.node.id}",
                "properties": {"status": "done"}
            }),
            for_each: None,
        };
        assert!(action.params.get("node_type").is_none());
    }

    #[test]
    fn test_remove_relationship_action_validates_type() {
        let action = ParsedAction {
            action_type: ActionType::RemoveRelationship,
            params: serde_json::json!({
                "source_id": "src",
                "relationship_type": "some_rel",
                "target_id": "tgt"
            }),
            for_each: None,
        };
        assert_eq!(
            action.params["relationship_type"].as_str(),
            Some("some_rel")
        );
    }

    #[test]
    fn test_missing_relationship_type_param() {
        let action = ParsedAction {
            action_type: ActionType::AddRelationship,
            params: serde_json::json!({
                "source_id": "src",
                "target_id": "tgt"
                // missing relationship_type
            }),
            for_each: None,
        };
        assert!(action.params.get("relationship_type").is_none());
    }

    // -- Async integration tests with real NodeService --

    mod integration {
        use super::*;
        use crate::db::SqliteStore;
        use crate::models::Node;
        use crate::services::NodeService;
        use serde_json::json;
        use std::sync::Arc;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        /// Helper: create a minimal schema node in the database.
        ///
        /// Relationship declarations route through the REAL write path
        /// (`set_schema_relationships` → relationship-table rows) — hand-writing
        /// a `relationships` JSON key into properties would bypass storage and
        /// silently read back as an empty declaration list.
        ///
        /// Note: schemas with relationships that reference target types require
        /// those target schemas to exist first (declaration edges are
        /// FK-constrained to the target schema node).
        async fn create_schema(
            node_service: &NodeService,
            type_name: &str,
            schema_version: u32,
            relationships: serde_json::Value,
        ) {
            let schema_node = Node::new_with_id(
                type_name.to_string(),
                "schema".to_string(),
                type_name.to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": schema_version,
                    "description": format!("{} schema", type_name),
                    "fields": [
                        {"name": "status", "type": "string"}
                    ]
                }),
            );
            node_service
                .create_node(schema_node)
                .await
                .unwrap_or_else(|_| panic!("Failed to create schema '{}'", type_name));

            let declarations: Vec<crate::models::schema::SchemaRelationship> =
                serde_json::from_value(relationships)
                    .unwrap_or_else(|e| panic!("Invalid relationships fixture: {e}"));
            if !declarations.is_empty() {
                node_service
                    .set_schema_relationships(type_name, &declarations)
                    .await
                    .unwrap_or_else(|e| {
                        panic!("Failed to declare relationships on '{}': {e}", type_name)
                    });
            }
        }

        // Use custom type names (prefixed "vt_") to avoid collisions
        // with core schemas seeded by NodeService::new (task, text, date, etc.)

        #[tokio::test]
        async fn test_valid_play_passes_validation() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_widget", 1, json!([])).await;

            let rules = vec![make_rule(
                "vt_widget",
                vec!["node.status == 'open'"],
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_unknown_trigger_node_type_fails() {
            let (svc, _tmp) = create_test_service().await;

            let rules = vec![make_rule("nonexistent_xyzzy", vec![], vec![])];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert_eq!(errors.len(), 1);
            match &errors[0] {
                PlayValidationError::UnknownNodeType {
                    node_type,
                    location,
                } => {
                    assert_eq!(node_type, "nonexistent_xyzzy");
                    assert_eq!(location, "rule[0].trigger");
                }
                other => panic!("expected UnknownNodeType, got {:?}", other),
            }
        }

        #[tokio::test]
        async fn test_unknown_action_node_type_fails() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_order", 1, json!([])).await;

            let rules = vec![make_rule(
                "vt_order",
                vec![],
                vec![make_create_action("nonexistent_type_abc", None)],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(errors
                .iter()
                .any(|e| matches!(e, PlayValidationError::UnknownNodeType { node_type, .. } if node_type == "nonexistent_type_abc")));
        }

        #[tokio::test]
        async fn test_version_mismatch_fails() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_receipt", 2, json!([])).await;
            create_schema(&svc, "vt_trigger", 1, json!([])).await;

            // Play declares version "3" but schema is at version 2
            let rules = vec![make_rule(
                "vt_trigger",
                vec![],
                vec![make_create_action("vt_receipt", Some("3"))],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(errors.iter().any(|e| matches!(
                e,
                PlayValidationError::VersionMismatch {
                    declared_version,
                    actual_version,
                    ..
                } if declared_version == "3" && *actual_version == 2
            )));
        }

        #[tokio::test]
        async fn test_matching_version_passes() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_bill", 2, json!([])).await;
            create_schema(&svc, "vt_src", 1, json!([])).await;

            let rules = vec![make_rule(
                "vt_src",
                vec![],
                vec![make_create_action("vt_bill", Some("2"))],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_unknown_relationship_type_fails() {
            let (svc, _tmp) = create_test_service().await;
            // Create schema with a known relationship
            create_schema(
                &svc,
                "vt_project",
                1,
                json!([
                    {
                        "name": "owned_by",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "owns",
                        "reverseCardinality": "many"
                    }
                ]),
            )
            .await;

            let rules = vec![make_rule(
                "vt_project",
                vec![],
                vec![make_relationship_action("nonexistent_rel")],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(errors.iter().any(|e| matches!(
                e,
                PlayValidationError::UnknownRelationshipType {
                    relationship_type,
                    ..
                } if relationship_type == "nonexistent_rel"
            )));
        }

        #[tokio::test]
        async fn test_valid_relationship_type_passes() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(
                &svc,
                "vt_ticket",
                1,
                json!([
                    {
                        "name": "linked_to",
                        "direction": "out",
                        "cardinality": "many",
                        "reverseName": "linked_from",
                        "reverseCardinality": "many"
                    }
                ]),
            )
            .await;

            let rules = vec![make_rule(
                "vt_ticket",
                vec![],
                vec![make_relationship_action("linked_to")],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_multiple_errors_collected() {
            let (svc, _tmp) = create_test_service().await;
            // No custom schemas — multiple errors expected.
            //
            // CEL syntax errors are now caught earlier, by `parse_rule` (a
            // `ParsedRule` can't exist with an uncompiled condition), so this
            // exercises the remaining schema-level checks collected together:
            // unknown trigger node_type + unknown action node_type.
            let rules = vec![make_rule(
                "nonexistent_aaa",
                vec![],
                vec![make_create_action("nonexistent_bbb", None)],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(
                errors.len() >= 2,
                "expected >= 2 errors, got {}",
                errors.len()
            );
        }

        #[tokio::test]
        async fn test_scheduled_trigger_node_type_validated() {
            let (svc, _tmp) = create_test_service().await;
            // "vt_cron_target" doesn't exist

            let rules = vec![make_scheduled_rule(
                "0 * * * * * *",
                "vt_cron_target",
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(errors
                .iter()
                .any(|e| matches!(e, PlayValidationError::UnknownNodeType { node_type, .. } if node_type == "vt_cron_target")));
        }

        #[tokio::test]
        async fn test_invalid_cron_expression_rejected() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_cron_valid_target", 1, json!([])).await;

            let rules = vec![make_scheduled_rule(
                "not a cron expression",
                "vt_cron_valid_target",
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvalidCronExpression { cron, .. } if cron == "not a cron expression"
                )),
                "expected InvalidCronExpression error, got {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn test_wrong_field_count_cron_rejected() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_cron_field_count", 1, json!([])).await;

            // Standard 5-field cron (no seconds/year) — this engine requires 7 fields
            let rules = vec![make_scheduled_rule(
                "* * * * *",
                "vt_cron_field_count",
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::InvalidCronExpression { .. })),
                "expected InvalidCronExpression error for wrong field count, got {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn test_valid_cron_expression_passes() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_cron_ok_target", 1, json!([])).await;

            let rules = vec![make_scheduled_rule(
                "0 * * * * * *",
                "vt_cron_ok_target",
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_ok(), "valid cron should pass: {:?}", result);
        }

        #[tokio::test]
        async fn test_empty_rules_passes() {
            let (svc, _tmp) = create_test_service().await;

            let rules: Vec<Arc<ParsedRule>> = vec![];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_binding_template_node_type_skips_validation() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_dynamic", 1, json!([])).await;

            // Action with binding template node_type — should not fail
            let action = ParsedAction {
                action_type: ActionType::CreateNode,
                params: json!({
                    "node_type": "{trigger.node.node_type}",
                    "content": "Dynamic"
                }),
                for_each: None,
            };
            let rules = vec![make_rule("vt_dynamic", vec![], vec![action])];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_core_schema_types_pass_validation() {
            let (svc, _tmp) = create_test_service().await;
            // "task" is a core schema seeded by NodeService::new — should pass

            let rules = vec![make_rule("task", vec!["node.status == 'open'"], vec![])];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_ok());
        }

        // -- Schema-aware path validation tests --

        #[tokio::test]
        async fn test_valid_multi_hop_path_passes() {
            let (svc, _tmp) = create_test_service().await;

            // Chain: vp_task -> story (rel) -> vp_story
            create_schema(&svc, "vp_story", 1, json!([])).await;
            create_schema(
                &svc,
                "vp_task",
                1,
                json!([{
                    "name": "story",
                    "targetType": "vp_story",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "issues",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            // Condition: node.story.status — "story" is a relationship, "status" is a field on vp_story
            let rules = vec![make_rule(
                "vp_task",
                vec!["node.story.status == 'active'"],
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "valid multi-hop path should pass: {:?}",
                result
            );
        }

        #[tokio::test]
        async fn test_broken_path_unknown_segment_fails() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vp_task2", 1, json!([])).await;

            // "nonexistent" is neither a field nor relationship on vp_task2
            let rules = vec![make_rule(
                "vp_task2",
                vec!["node.nonexistent.foo == 'bar'"],
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::BrokenPath { segment, .. } if segment == "nonexistent"
                )),
                "should report broken path for 'nonexistent': {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn test_broken_path_field_as_non_terminal() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vp_task3", 1, json!([])).await;

            // "status" is a field on vp_task3 — can't traverse further
            let rules = vec![make_rule(
                "vp_task3",
                vec!["node.status.deeper == 'x'"],
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::BrokenPath { segment, .. } if segment == "status"
                )),
                "should report broken path for field-as-non-terminal: {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn test_single_hop_property_path_skips_validation() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vp_task4", 1, json!([])).await;

            // Single-hop (node.status) is handled by existing property-level evaluation
            // and should NOT be validated against the schema graph
            let rules = vec![make_rule("vp_task4", vec!["node.status == 'open'"], vec![])];
            let result = validate_play(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "single-hop paths should skip schema validation"
            );
        }

        #[tokio::test]
        async fn test_broken_path_relationship_without_target_type() {
            let (svc, _tmp) = create_test_service().await;
            // Relationship with no target_type
            create_schema(
                &svc,
                "vp_task5",
                1,
                json!([{
                    "name": "linked",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "linked_from",
                    "reverseCardinality": "many"
                    // no target_type
                }]),
            )
            .await;

            // Trying to traverse past a relationship without target_type
            let rules = vec![make_rule(
                "vp_task5",
                vec!["node.linked.status == 'x'"],
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::BrokenPath { segment, .. } if segment == "linked"
                )),
                "should report broken path for rel without target_type: {:?}",
                errors
            );
        }

        // ---------------------------------------------------------------
        // check_schema_change_impact tests (Phase 2)
        // ---------------------------------------------------------------

        /// Helper: create a play node in the database.
        async fn create_play(node_service: &NodeService, id: &str, rules_json: serde_json::Value) {
            let node = Node::new_with_id(
                id.to_string(),
                "play".to_string(),
                format!("Play {}", id),
                json!({ "rules": rules_json }),
            );
            node_service
                .create_node(node)
                .await
                .unwrap_or_else(|_| panic!("Failed to create play '{}'", id));
        }

        #[tokio::test]
        async fn test_schema_impact_detects_affected_plays() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_task", 1, json!([])).await;

            // Create a play that triggers on "vi_task"
            create_play(
                &svc,
                "pb-impact-1",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vi_task" },
                    "conditions": ["node.status == 'open'"],
                    "actions": []
                }]),
            )
            .await;

            let affected = check_schema_change_impact("vi_task", &svc).await.unwrap();
            assert_eq!(affected.len(), 1);
            assert_eq!(affected[0].play_id, "pb-impact-1");
            assert!(
                affected[0]
                    .broken_paths
                    .iter()
                    .any(|p| p.contains("vi_task")),
                "should list the trigger path: {:?}",
                affected[0].broken_paths
            );
        }

        #[tokio::test]
        async fn test_schema_impact_unrelated_schema_passes_clean() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_order", 1, json!([])).await;
            create_schema(&svc, "vi_invoice", 1, json!([])).await;

            // Create a play that triggers on "vi_order" only
            create_play(
                &svc,
                "pb-impact-2",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vi_order" },
                    "conditions": ["node.status == 'open'"],
                    "actions": []
                }]),
            )
            .await;

            // Changing "vi_invoice" should not affect the vi_order play
            let affected = check_schema_change_impact("vi_invoice", &svc)
                .await
                .unwrap();
            assert!(
                affected.is_empty(),
                "unrelated schema change should not affect plays: {:?}",
                affected
            );
        }

        #[tokio::test]
        async fn test_schema_impact_detects_path_traversal() {
            let (svc, _tmp) = create_test_service().await;
            // Create vi_epic first (target of relationship)
            create_schema(&svc, "vi_epic", 1, json!([])).await;
            // Create vi_story with a relationship to vi_epic, so the play passes validation
            // Note: SchemaRelationship uses camelCase serialization
            create_schema(
                &svc,
                "vi_story",
                1,
                json!([{
                    "name": "vi_epic",
                    "direction": "out",
                    "cardinality": "one",
                    "targetType": "vi_epic",
                    "reverseName": "vi_children",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            // Play triggers on vi_story but has a condition traversing through vi_epic
            create_play(
                &svc,
                "pb-impact-3",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vi_story" },
                    "conditions": ["node.vi_epic.status == 'active'"],
                    "actions": []
                }]),
            )
            .await;

            let affected = check_schema_change_impact("vi_epic", &svc).await.unwrap();
            assert_eq!(affected.len(), 1);
            assert_eq!(affected[0].play_id, "pb-impact-3");
            assert!(
                affected[0]
                    .broken_paths
                    .iter()
                    .any(|p| p.contains("vi_epic")),
                "should detect path traversal through vi_epic: {:?}",
                affected[0].broken_paths
            );
        }
    }

    // ---------------------------------------------------------------
    // NodeService synchronous validation gate tests (Phase 1)
    // ---------------------------------------------------------------

    mod sync_gate_tests {
        use crate::db::SqliteStore;
        use crate::models::Node;
        use crate::services::NodeService;
        use serde_json::json;
        use std::sync::Arc;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        async fn create_schema(node_service: &NodeService, type_name: &str, schema_version: u32) {
            let schema_node = Node::new_with_id(
                type_name.to_string(),
                "schema".to_string(),
                type_name.to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": schema_version,
                    "description": format!("{} schema", type_name),
                    "fields": [
                        {"name": "status", "type": "string"}
                    ],
                    "relationships": []
                }),
            );
            node_service
                .create_node(schema_node)
                .await
                .unwrap_or_else(|_| panic!("Failed to create schema '{}'", type_name));
        }

        #[tokio::test]
        async fn test_invalid_play_rejected_on_create() {
            let (svc, _tmp) = create_test_service().await;
            // Don't create a schema for "nonexistent_type" — it should be rejected

            let play_node = Node::new_with_id(
                "pb-gate-1".to_string(),
                "play".to_string(),
                "Test Play".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "nonexistent_type" },
                        "conditions": [],
                        "actions": []
                    }]
                }),
            );

            let result = svc.create_node(play_node).await;
            assert!(result.is_err(), "invalid play should be rejected on create");
            let err = result.unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("Play validation failed"),
                "error should indicate validation failure: {}",
                msg
            );
            assert!(
                msg.contains("nonexistent_type"),
                "error should mention the bad node_type: {}",
                msg
            );
        }

        #[tokio::test]
        async fn test_valid_play_accepted_on_create() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vg_widget", 1).await;

            let play_node = Node::new_with_id(
                "pb-gate-2".to_string(),
                "play".to_string(),
                "Valid Play".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vg_widget" },
                        "conditions": ["node.status == 'open'"],
                        "actions": []
                    }]
                }),
            );

            let result = svc.create_node(play_node).await;
            assert!(
                result.is_ok(),
                "valid play should be accepted: {:?}",
                result
            );
        }

        #[tokio::test]
        async fn test_invalid_cel_rejected_on_create() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vg_item", 1).await;

            let play_node = Node::new_with_id(
                "pb-gate-3".to_string(),
                "play".to_string(),
                "Bad CEL Play".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vg_item" },
                        "conditions": ["1 + + 2"],
                        "actions": []
                    }]
                }),
            );

            let result = svc.create_node(play_node).await;
            assert!(result.is_err(), "play with invalid CEL should be rejected");
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("Play validation failed"),
                "error should indicate validation failure: {}",
                msg
            );
        }

        #[tokio::test]
        async fn test_invalid_cron_rejected_on_create() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vg_cron_item", 1).await;

            let play_node = Node::new_with_id(
                "pb-gate-6".to_string(),
                "play".to_string(),
                "Bad Cron Play".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "scheduled", "cron": "not a cron expression", "node_type": "vg_cron_item" },
                        "conditions": [],
                        "actions": []
                    }]
                }),
            );

            let result = svc.create_node(play_node).await;
            assert!(
                result.is_err(),
                "play with invalid cron expression should be rejected"
            );
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("Play validation failed"),
                "error should indicate validation failure: {}",
                msg
            );
        }

        #[tokio::test]
        async fn test_update_with_broken_rules_rejected() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vg_part", 1).await;

            // Create a valid play first
            let play_node = Node::new_with_id(
                "pb-gate-4".to_string(),
                "play".to_string(),
                "Initially Valid Play".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vg_part" },
                        "conditions": [],
                        "actions": []
                    }]
                }),
            );
            svc.create_node(play_node).await.unwrap();

            // Now update it with broken rules (reference nonexistent node_type)
            let update = crate::models::NodeUpdate {
                properties: Some(json!({
                    "rules": [{
                        "name": "r1_updated",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vanished_type" },
                        "conditions": [],
                        "actions": []
                    }]
                })),
                ..Default::default()
            };

            let result = svc.update_node("pb-gate-4", 1, update).await;
            assert!(
                result.is_err(),
                "update with broken rules should be rejected"
            );
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("Play validation failed"),
                "error should indicate validation failure: {}",
                msg
            );
        }

        #[tokio::test]
        async fn test_parse_error_rejected_on_create() {
            let (svc, _tmp) = create_test_service().await;

            // Play with an invalid trigger type
            let play_node = Node::new_with_id(
                "pb-gate-5".to_string(),
                "play".to_string(),
                "Bad Trigger Play".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "bad_trigger_type", "on": "node_created", "node_type": "task" },
                        "conditions": [],
                        "actions": []
                    }]
                }),
            );

            let result = svc.create_node(play_node).await;
            assert!(
                result.is_err(),
                "play with invalid trigger type should be rejected"
            );
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("Play validation failed"),
                "error should indicate validation failure: {}",
                msg
            );
        }
    }

    // -----------------------------------------------------------------------
    // Invariant-rule eligibility (ADR-060 §2)
    // -----------------------------------------------------------------------

    mod invariant_eligibility {
        use super::*;
        use serde_json::json;
        use std::sync::Arc;

        /// Build an invariant graph-event rule for eligibility testing.
        fn invariant_rule(
            on: GraphEventType,
            node_type: &str,
            property_key: Option<&str>,
            conditions: Vec<&str>,
            actions: Vec<ParsedAction>,
        ) -> ParsedRule {
            ParsedRule {
                name: "inv".to_string(),
                class: RuleClass::Invariant,
                trigger: ParsedTrigger::GraphEvent {
                    on,
                    node_type: node_type.to_string(),
                    property_key: property_key.map(str::to_string),
                },
                conditions: compile_conditions(conditions),
                actions,
            }
        }

        fn create_action(node_type: &str) -> ParsedAction {
            ParsedAction {
                action_type: ActionType::CreateNode,
                params: json!({ "node_type": node_type, "content": "x" }),
                for_each: None,
            }
        }

        fn update_action(node_id: &str) -> ParsedAction {
            ParsedAction {
                action_type: ActionType::UpdateNode,
                params: json!({ "node_id": node_id, "properties": { "custom:tag": "v" } }),
                for_each: None,
            }
        }

        fn add_rel_action(source_id: &str, target_id: &str) -> ParsedAction {
            ParsedAction {
                action_type: ActionType::AddRelationship,
                params: json!({
                    "source_id": source_id,
                    "relationship_type": "linked_to",
                    "target_id": target_id,
                }),
                for_each: None,
            }
        }

        fn eligibility_errors(rule: &ParsedRule) -> Vec<PlayValidationError> {
            let mut errors = Vec::new();
            validate_invariant_eligibility(rule, 0, &mut errors);
            errors
        }

        // -- Local writes only --

        #[test]
        fn all_current_action_types_are_local_writes() {
            // ADR-060 §2 "local writes only" is a forward-looking gate: every v1
            // action type IS a local write, so an invariant rule passes it today.
            // The classification is an explicit exhaustive match (not a hardcoded
            // `true` at the call site), so a future non-local action type will
            // fail to compile until it is classified here.
            for at in [
                ActionType::CreateNode,
                ActionType::UpdateNode,
                ActionType::AddRelationship,
                ActionType::RemoveRelationship,
            ] {
                assert!(at.is_local_write(), "{:?} should be a local write", at);
            }
        }

        // -- Valid invariant rule --

        #[test]
        fn valid_invariant_rule_has_no_eligibility_errors() {
            // Canonical invariant: on task creation, stamp a property on the
            // trigger node inside the txn. Local write, deterministic, in-scope
            // (targets the trigger node), and does not re-fire `node_created`.
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec!["node.status == 'open'"],
                vec![update_action("{trigger.node.id}")],
            );
            let errors = eligibility_errors(&rule);
            assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
        }

        // -- Deterministic --

        #[test]
        fn invariant_non_deterministic_condition_rejected() {
            for (expr, func) in [
                ("days_since(node.created_date) > 7", "days_since"),
                ("days_until(node.due_date) < 3", "days_until"),
                ("size(today()) == 10", "today"),
            ] {
                let rule = invariant_rule(
                    GraphEventType::NodeCreated,
                    "task",
                    None,
                    vec![expr],
                    vec![],
                );
                let errors = eligibility_errors(&rule);
                assert!(
                    errors.iter().any(|e| matches!(
                        e,
                        PlayValidationError::InvariantNonDeterministic { function, .. }
                            if function == func
                    )),
                    "expected non-deterministic '{}' error for `{}`, got {:?}",
                    func,
                    expr,
                    errors
                );
            }
        }

        #[test]
        fn invariant_deterministic_condition_accepted() {
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec!["node.priority == 'high' && node.amount > 1000"],
                vec![],
            );
            assert!(eligibility_errors(&rule).is_empty());
        }

        // -- Same-graph scope --

        #[test]
        fn invariant_literal_update_target_rejected() {
            // update_node with a literal node_id addresses an arbitrary node.
            let rule = invariant_rule(
                GraphEventType::PropertyChanged,
                "task",
                Some("status"),
                vec![],
                vec![update_action("some-fixed-node-id")],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantOutOfScopeTarget { action, param, .. }
                        if action == "update_node" && param == "node_id"
                )),
                "expected out-of-scope node_id error, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_literal_relationship_target_rejected() {
            // Binding source, but a literal target_id addresses an arbitrary node.
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![add_rel_action("{trigger.node.id}", "collection-hr")],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantOutOfScopeTarget { param, value, .. }
                        if param == "target_id" && value == "collection-hr"
                )),
                "expected out-of-scope target_id error, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_trigger_derived_bindings_accepted() {
            // Both relationship endpoints are trigger-derived bindings, and the
            // trigger event (node_created) is not re-satisfied by add_relationship.
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![add_rel_action(
                    "{trigger.node.id}",
                    "{trigger.node.owner_id}",
                )],
            );
            let errors = eligibility_errors(&rule);
            assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
        }

        // -- Non-chaining, depth 1 (self-trigger detection) --

        #[test]
        fn invariant_self_chaining_create_same_type_rejected() {
            // node_created(task) + create_node(task) re-fires the same trigger.
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![create_action("task")],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantChaining { action, trigger, .. }
                        if action == "create_node" && trigger == "node_created"
                )),
                "expected chaining error, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_create_different_type_not_chaining() {
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![create_action("audit_log")],
            );
            assert!(
                !eligibility_errors(&rule)
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::InvariantChaining { .. })),
                "creating a different node type must not self-chain"
            );
        }

        #[test]
        fn invariant_self_chaining_property_update_of_trigger_node_rejected() {
            let rule = invariant_rule(
                GraphEventType::PropertyChanged,
                "task",
                Some("status"),
                vec![],
                vec![update_action("{trigger.node.id}")],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantChaining { action, trigger, .. }
                        if action == "update_node" && trigger == "property_changed"
                )),
                "expected chaining error, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_self_chaining_add_relationship_from_trigger_rejected() {
            let rule = invariant_rule(
                GraphEventType::RelationshipAdded,
                "task",
                None,
                vec![],
                vec![add_rel_action(
                    "{trigger.node.id}",
                    "{trigger.node.owner_id}",
                )],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantChaining { action, trigger, .. }
                        if action == "add_relationship" && trigger == "relationship_added"
                )),
                "expected chaining error, got {:?}",
                errors
            );
        }

        // -- End-to-end through validate_play: reactive bypass + invariant gate --

        async fn create_test_service() -> (Arc<crate::services::NodeService>, tempfile::TempDir) {
            let temp_dir = tempfile::TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<crate::db::SqliteStore> =
                Arc::new(crate::db::SqliteStore::new(db_path).await.unwrap());
            let node_service =
                Arc::new(crate::services::NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        async fn create_schema(node_service: &crate::services::NodeService, type_name: &str) {
            let schema_node = crate::models::Node::new_with_id(
                type_name.to_string(),
                "schema".to_string(),
                type_name.to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": 1,
                    "description": format!("{} schema", type_name),
                    "fields": [{ "name": "status", "type": "string" }],
                    "relationships": []
                }),
            );
            node_service.create_node(schema_node).await.unwrap();
        }

        #[tokio::test]
        async fn reactive_rule_bypasses_invariant_gate() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_react").await;

            // A REACTIVE rule (default class) that would violate §2 if invariant:
            // non-deterministic condition + self-chaining create_node of the same
            // type. Reactive rules are not gated → accepted.
            let rule = Arc::new(invariant_rule(
                GraphEventType::NodeCreated,
                "vi_react",
                None,
                vec!["days_since(node.created) > 7"],
                vec![create_action("vi_react")],
            ));
            let reactive = Arc::new(ParsedRule {
                class: RuleClass::Reactive,
                ..(*rule).clone()
            });
            let result = validate_play(&[reactive], &svc).await;
            assert!(
                result.is_ok(),
                "reactive rule must bypass the §2 gate: {:?}",
                result
            );
        }

        #[tokio::test]
        async fn invariant_rule_gated_through_validate_play() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_inv").await;

            // Same rule as above, but INVARIANT → the §2 gate fires with both a
            // non-determinism and a self-chaining error, proving the gate is wired
            // into validate_play.
            let rule = Arc::new(invariant_rule(
                GraphEventType::NodeCreated,
                "vi_inv",
                None,
                vec!["days_since(node.created) > 7"],
                vec![create_action("vi_inv")],
            ));
            let errors = validate_play(&[rule], &svc).await.unwrap_err();
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantNonDeterministic { function, .. }
                        if function == "days_since"
                )),
                "expected non-determinism error, got {:?}",
                errors
            );
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantChaining { action, .. }
                        if action == "create_node"
                )),
                "expected chaining error, got {:?}",
                errors
            );
        }
    }
}
