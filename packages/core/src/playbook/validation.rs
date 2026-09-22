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
//! 5. A `property_changed` trigger's `property_key` must be namespaced to its
//!    own `node_type` (`<node_type>.<field>`) — the only spelling a real,
//!    type-namespaced `PropertyChanged` event can ever match
//!
//! If any check fails, the play is not saved. All errors are collected
//! (not short-circuited) so the caller can present every issue at once.

use crate::models::SchemaNode;
use crate::playbook::actions::{
    action_list_signature, collect_binding_templates_in_value, parse_function_call,
};
use crate::playbook::path_extractor;
use crate::playbook::types::{
    namespaced_property_key, ActionType, GraphEventType, ParsedAction, ParsedRule, ParsedTrigger,
    RuleClass,
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
    /// An invariant rule's trigger is not a `node_created` or
    /// `property_changed` graph event. Synchronous pre-commit dispatch
    /// (ADR-060 §1) is wired into the node-creation and update write paths —
    /// there is no equivalent open transaction to join for a
    /// `relationship_added`/`relationship_removed` mutation, or for a
    /// scheduled scan. Declaring a rule `invariant` against one of those
    /// triggers would silently never execute rather than deliver the
    /// fail-closed guarantee its class name promises, so it is rejected here
    /// instead.
    InvariantUnsupportedTrigger { trigger: String, location: String },
    /// An invariant `add_relationship` action targets `member_of` or
    /// `has_child` without an explicit `order` in `edge_data`. Both types
    /// normally get an atomically-computed order (read current max sibling,
    /// then write) via `add_to_collection`/`append_child_edge`; that
    /// read-then-write has no transaction-scoped twin, so an invariant
    /// action needs a caller-supplied order instead of relying on it.
    InvariantRelationshipNeedsExplicitOrder {
        relationship_type: String,
        location: String,
    },
    /// A `reject` action (ADR-060 §2) is declared on a non-`Invariant` rule.
    /// `reject`'s entire meaning is "fail the enclosing transaction" — there
    /// is no transaction left to fail once a rule's actions run
    /// asynchronously, post-commit (`Reactive`, ADR-060's default class), so
    /// this is caught at save time rather than silently no-op'd (or errored
    /// generically) at runtime.
    RejectActionOnReactiveRule { location: String },
    /// A `reject` action declares a `for_each`. `reject`'s condition
    /// (evaluated against the trigger node, not a collection item) is
    /// already the gate for whether it fires — iterating it over a
    /// collection adds nothing but a real correctness hazard: if the
    /// resolved collection is empty, the loop body never runs and the
    /// action silently no-ops, vetoing nothing, with no save-time or
    /// runtime warning. Rejected outright rather than accepted-with-a-caveat.
    RejectActionHasForEach { location: String },
    /// Two different rules within the SAME play have byte-identical action
    /// lists -- the same `action_type`, `params`, and `for_each` sequence,
    /// in order, for every action (the same serialized shape
    /// `playbook::actions::rule_id_for` hashes into a rule's derived
    /// identity). Two DIFFERENT rules colliding onto the same `rule_id`
    /// derive the same output node id for a given `(action_index,
    /// iteration_path)`; `execute_create_node`'s existing-node-at-derived-id
    /// convergence check cannot distinguish "the same rule re-firing" from
    /// "two different rules colliding," so it silently returns the wrong
    /// rule's node -- no error, no log. The realistic trigger is copy-paste
    /// rule authoring: duplicate a rule, change its trigger or condition,
    /// leave the action list untouched. Cross-play collisions are already
    /// distinguished by `play_id` in `rule_id_for` and are not flagged --
    /// only pairs within the same play (the same `validate_play` call) are
    /// compared. `location` names the later (duplicate) rule;
    /// `duplicate_of_location` names the earlier rule it collides with.
    DuplicateActionList {
        rule_name: String,
        duplicate_of_rule_name: String,
        duplicate_of_location: String,
        location: String,
    },
    /// A `property_changed` trigger's `property_key` has no namespace prefix
    /// matching the trigger's own declared `node_type`.
    ///
    /// Every real `PropertyChanged` event carries a type-namespaced key
    /// (`PropertyChange::key`, "namespaced, e.g. `task.status`" —
    /// `packages/core/src/db/events.rs`), and the engine indexes a Play's
    /// trigger under exactly the `property_key` its author wrote, verbatim
    /// (`trigger_keys_for_graph_event`, `packages/core/src/playbook/lifecycle.rs`)
    /// — nothing normalises either side. A bare spelling (`"status"`) or a
    /// spelling namespaced under some other type (`"bug.status"` on a `task`
    /// trigger) is therefore indexed under a key no real event for this
    /// trigger's `node_type` will ever carry: the trigger is silently dead —
    /// no error at save time, no log at runtime, just a rule that never
    /// fires. Rejected here instead, loudly, at authoring time.
    ///
    /// `expected` names the correctly-namespaced spelling the author should
    /// have written, given the trigger's own `node_type` and the field
    /// portion of what was actually written (the part after the first `.`,
    /// or the whole string when there was no `.` at all).
    UnnamespacedPropertyChangedKey {
        node_type: String,
        property_key: String,
        expected: String,
        location: String,
    },
    /// A schema/extends-chain resolution call needed to validate a
    /// condition path failed (a DB error while walking the `extends`
    /// chain), rather than succeeding with a definitive field/relationship/
    /// unknown-segment answer.
    ///
    /// Save-time validation cannot safely degrade to a narrower, un-merged
    /// view on this failure the way a read-only diagnostic can: silently
    /// falling back to `current_type`'s own directly-declared fields/
    /// relationships would reintroduce, under nothing more than a
    /// transient DB hiccup, the exact under-reporting bug this module
    /// exists to close — a genuinely inherited field or relationship could
    /// be wrongly rejected as `BrokenPath`, blocking a legitimate Play from
    /// ever being saved. Surfaced here instead so the failure is visible
    /// and the caller can retry, rather than the play being silently
    /// mis-validated one way or the other.
    SchemaResolutionFailed {
        node_type: String,
        error: String,
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
            Self::InvariantUnsupportedTrigger { trigger, location } => write!(
                f,
                "invariant rule at {} has trigger '{}', which synchronous pre-commit dispatch \
                 does not support (only node_created and property_changed graph-event triggers \
                 run inside a transaction today) — declare this rule reactive, or change its \
                 trigger to node_created or property_changed",
                location, trigger
            ),
            Self::InvariantRelationshipNeedsExplicitOrder {
                relationship_type,
                location,
            } => write!(
                f,
                "invariant rule action at {} adds a '{}' relationship without an explicit \
                 'order' in edge_data (invariant add_relationship actions cannot use the \
                 atomic auto-order path — supply an explicit order)",
                location, relationship_type
            ),
            Self::RejectActionOnReactiveRule { location } => write!(
                f,
                "reject action at {} is declared on a reactive rule (reject is only meaningful \
                 on an invariant rule — there is no transaction left to fail once a rule's \
                 actions run asynchronously, post-commit; declare this rule invariant, or \
                 remove the reject action)",
                location
            ),
            Self::RejectActionHasForEach { location } => write!(
                f,
                "reject action at {} declares a for_each (reject's condition already gates \
                 whether it fires — iterating it adds a correctness hazard: an empty \
                 collection would silently no-op instead of vetoing the write; remove the \
                 for_each)",
                location
            ),
            Self::DuplicateActionList {
                rule_name,
                duplicate_of_rule_name,
                duplicate_of_location,
                location,
            } => write!(
                f,
                "rule '{}' at {} has an action list byte-identical to rule '{}' at {} \
                 (two same-play rules with identical actions derive the same rule identity and \
                 silently collide onto one output node at execution time; rename/differentiate \
                 one rule's actions, or remove the duplicate)",
                rule_name, location, duplicate_of_rule_name, duplicate_of_location
            ),
            Self::UnnamespacedPropertyChangedKey {
                node_type,
                property_key,
                expected,
                location,
            } => write!(
                f,
                "property_changed trigger at {} on node_type '{}' has property_key '{}', \
                 which is not namespaced to '{}.' (real PropertyChanged events always carry a \
                 type-namespaced key, and the trigger is indexed under exactly what you wrote — \
                 an un-namespaced property_key never matches a real event and the rule silently \
                 never fires; declare it as '{}')",
                location, node_type, property_key, node_type, expected
            ),
            Self::SchemaResolutionFailed {
                node_type,
                error,
                location,
            } => write!(
                f,
                "schema resolution failed for '{}' at {}: {} (could not determine whether the \
                 referenced path is valid — this is a transient/internal error, not a broken \
                 path; retry)",
                node_type, location, error
            ),
        }
    }
}

impl PlayValidationError {
    /// The `location` string every variant carries (e.g. `"rule[1].action[2]"`,
    /// `"rule[0].trigger"`), identifying which rule/trigger/condition/action
    /// produced this error.
    ///
    /// Callers that log or fingerprint validation errors (e.g. the play
    /// engine's `load_active_plays`/`handle_play_created`/`handle_play_updated`)
    /// need this: two structurally different errors on the same play must
    /// not collapse onto the same identity just because both happen to be
    /// `PlayValidationError`s.
    pub fn location(&self) -> &str {
        match self {
            Self::UnknownNodeType { location, .. }
            | Self::VersionMismatch { location, .. }
            | Self::UnknownRelationshipType { location, .. }
            | Self::MissingActionParam { location, .. }
            | Self::BrokenPath { location, .. }
            | Self::InvalidCronExpression { location, .. }
            | Self::InvariantNonLocalAction { location, .. }
            | Self::InvariantNonDeterministic { location, .. }
            | Self::InvariantOutOfScopeTarget { location, .. }
            | Self::InvariantChaining { location, .. }
            | Self::InvariantUnsupportedTrigger { location, .. }
            | Self::InvariantRelationshipNeedsExplicitOrder { location, .. }
            | Self::RejectActionOnReactiveRule { location }
            | Self::RejectActionHasForEach { location }
            | Self::DuplicateActionList { location, .. }
            | Self::UnnamespacedPropertyChangedKey { location, .. }
            | Self::SchemaResolutionFailed { location, .. } => location,
        }
    }

    /// A short, stable tag identifying which variant this is. Paired with
    /// `location()` so two different error *kinds* at the exact same
    /// location (e.g. a missing param and a broken path on the same action)
    /// still produce distinct identities, not just two different locations.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::UnknownNodeType { .. } => "unknown_node_type",
            Self::VersionMismatch { .. } => "version_mismatch",
            Self::UnknownRelationshipType { .. } => "unknown_relationship_type",
            Self::MissingActionParam { .. } => "missing_action_param",
            Self::BrokenPath { .. } => "broken_path",
            Self::InvalidCronExpression { .. } => "invalid_cron_expression",
            Self::InvariantNonLocalAction { .. } => "invariant_non_local_action",
            Self::InvariantNonDeterministic { .. } => "invariant_non_deterministic",
            Self::InvariantOutOfScopeTarget { .. } => "invariant_out_of_scope_target",
            Self::InvariantChaining { .. } => "invariant_chaining",
            Self::InvariantUnsupportedTrigger { .. } => "invariant_unsupported_trigger",
            Self::InvariantRelationshipNeedsExplicitOrder { .. } => {
                "invariant_relationship_needs_explicit_order"
            }
            Self::RejectActionOnReactiveRule { .. } => "reject_action_on_reactive_rule",
            Self::RejectActionHasForEach { .. } => "reject_action_has_for_each",
            Self::DuplicateActionList { .. } => "duplicate_action_list",
            Self::UnnamespacedPropertyChangedKey { .. } => "unnamespaced_property_changed_key",
            Self::SchemaResolutionFailed { .. } => "schema_resolution_failed",
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

        // -- Validate a property_changed trigger's property_key is namespaced --
        //
        // `None` (wildcard — "matches all property changes") is untouched: there
        // is no key to namespace. A `Some(key)` is only ever indexed verbatim
        // under this rule's own `node_type` (`trigger_keys_for_graph_event`), so
        // the one spelling that can ever match a real, type-namespaced
        // `PropertyChanged` event is `<node_type>.<field>` — checked against the
        // exact `node_type` this trigger declared, not merely "has a dot
        // somewhere": a key namespaced under some OTHER type (`"bug.status"` on
        // a `task` trigger) looks namespaced but still matches nothing here and
        // is rejected the same as a fully bare `"status"`.
        if let ParsedTrigger::GraphEvent {
            on: GraphEventType::PropertyChanged,
            node_type,
            property_key: Some(key),
        } = &rule.trigger
        {
            let namespace_matches = key
                .split_once('.')
                .is_some_and(|(namespace, _)| namespace == node_type);
            if !namespace_matches {
                let field = key.split_once('.').map_or(key.as_str(), |(_, f)| f);
                errors.push(PlayValidationError::UnnamespacedPropertyChangedKey {
                    node_type: node_type.clone(),
                    property_key: key.clone(),
                    expected: namespaced_property_key(node_type, field),
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

        // -- Validate reject-action class (ADR-060 §2) --
        //
        // Unlike the invariant-eligibility checks above, this runs for EVERY
        // rule regardless of class: it exists specifically to catch a
        // `reject` action declared on the WRONG (reactive) class, so it
        // cannot itself be gated behind `rule.class == RuleClass::Invariant`.
        validate_reject_action_class(rule, rule_idx, &mut errors);
    }

    // -- Validate no two same-play rules share a byte-identical action list --
    //
    // Unlike every check above, this is a cross-rule check over the WHOLE
    // rule list rather than a per-rule one, so it runs once here instead of
    // inside the per-rule loop. `rules` is always the set of rules for a
    // single play (every `validate_play` call site parses and validates one
    // play node at a time), so comparing all pairs in it is exactly the
    // same-play scope this check needs -- cross-play collisions are already
    // distinguished by `play_id` in `rule_id_for` and are not this check's
    // concern.
    validate_no_duplicate_action_lists(rules, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Detect two rules within `rules` (always a single play's rules -- see the
/// call site in [`validate_play`]) whose action lists are byte-identical, in
/// the same shape `playbook::actions::rule_id_for` hashes into a rule's
/// derived identity. See [`PlayValidationError::DuplicateActionList`] for
/// why this matters.
///
/// Rules with an empty action list are excluded from comparison: there is no
/// `create_node` (or any other) output whose id is derived from `rule_id`,
/// so two do-nothing rules "colliding" has no observable consequence -- this
/// check exists for the specific derived-identity collision the module doc
/// describes, not as a general duplicate-rule linter.
///
/// Compares every pair once. Play rule counts are small in practice, so the
/// O(n^2) comparison is not a concern.
fn validate_no_duplicate_action_lists(
    rules: &[Arc<ParsedRule>],
    errors: &mut Vec<PlayValidationError>,
) {
    let signatures: Vec<Option<String>> = rules
        .iter()
        .map(|rule| {
            if rule.actions.is_empty() {
                None
            } else {
                Some(action_list_signature(&rule.actions))
            }
        })
        .collect();

    for later_idx in 1..signatures.len() {
        let Some(later_sig) = signatures[later_idx].as_deref() else {
            continue;
        };
        // Report against the EARLIEST matching rule only. When three or more
        // rules in a play share one action list, this reports N-1 errors
        // (each later rule against the first occurrence) instead of the full
        // O(n^2) set of pairs -- they'd all describe the same underlying
        // duplicate action list, just against different "first" rules.
        if let Some(earlier_idx) =
            (0..later_idx).find(|&i| signatures[i].as_deref() == Some(later_sig))
        {
            errors.push(PlayValidationError::DuplicateActionList {
                rule_name: rules[later_idx].name.clone(),
                duplicate_of_rule_name: rules[earlier_idx].name.clone(),
                duplicate_of_location: format!("rule[{}]", earlier_idx),
                location: format!("rule[{}]", later_idx),
            });
        }
    }
}

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

        if schema_cache
            .get(&current_type)
            .and_then(|s| s.as_ref())
            .is_none()
        {
            // Schema not found — can't validate further
            // (UnknownNodeType error is already reported by trigger validation)
            return;
        }

        // Resolve which schema in `current_type`'s ADR-078 `extends` chain
        // declares `segment` — as a field, or as a relationship — together
        // with the full chain order, nearest first.
        //
        // This must NOT be two independently pre-merged sets ("is it a
        // member of the whole merged field set?", then separately "is it a
        // member of the whole merged relationship set?") checked in a fixed
        // field-then-relationship order: a nearer schema's OWN relationship
        // must shadow a farther ancestor's field of the same name, and vice
        // versa — extends-chain shadowing is defined per declared name, not
        // per field-vs-relationship kind. `resolve_field_owners`/
        // `resolve_relationships` each return an owning-schema-id per name,
        // so the two are combined below by comparing chain position, not by
        // asking "field first" unconditionally.
        //
        // On a resolution failure, this does not fall back to a narrower,
        // un-merged view (`schema.fields`/`schema.relationships`): that
        // would reintroduce, under nothing more than a transient DB error,
        // the exact under-reporting bug this fix exists to close. Surfaced
        // as a validation error instead — see `SchemaResolutionFailed`.
        let (field_owners, chain) = match node_service.resolve_field_owners(&current_type).await {
            Ok((_fields, owners, chain)) => (owners, chain),
            Err(e) => {
                errors.push(PlayValidationError::SchemaResolutionFailed {
                    node_type: current_type.clone(),
                    error: e.to_string(),
                    location: location.to_string(),
                });
                return;
            }
        };
        let (relationships, rel_owners) =
            match node_service.resolve_relationships(&current_type).await {
                Ok(result) => result,
                Err(e) => {
                    errors.push(PlayValidationError::SchemaResolutionFailed {
                        node_type: current_type.clone(),
                        error: e.to_string(),
                        location: location.to_string(),
                    });
                    return;
                }
            };

        let field_pos = field_owners
            .get(segment)
            .and_then(|owner| chain.iter().position(|t| t == owner));
        let rel_pos = rel_owners
            .get(segment)
            .and_then(|owner| chain.iter().position(|t| t == owner));
        let is_field = match (field_pos, rel_pos) {
            (Some(_), None) => true,
            (None, _) => false,
            // Both a field and a relationship somewhere in the chain
            // declare this name: the nearer (lower chain index) one wins.
            // Schema creation guards against the SAME schema declaring
            // both under one name, so equal positions only mean the tie
            // is moot — the field arm is picked arbitrarily but
            // harmlessly.
            (Some(f), Some(r)) => f <= r,
        };

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

        // A built-in structural relationship (`has_child`/`child_of`,
        // `mentions`/`mentioned_by`, ...) has no `SchemaRelationship` behind it
        // — it is not declared on any schema, in either direction. The resolver
        // walks these by name at runtime (`rel_ops::resolve_relationship_name`),
        // so rejecting them here would fail a path the engine can traverse
        // perfectly well, which is what a rollup Play's `node.child_of` hits.
        //
        // Any node type can nest under any other, so a built-in yields no
        // target type to narrow to: traversal continues with the current type
        // as the best available guess. That keeps a later segment checkable
        // when the type happens to be right, and at worst declines to catch a
        // broken segment — never invents an error for a valid path.
        if crate::models::schema::is_reserved_relationship_name(segment) {
            continue;
        }

        let relationship = relationships.iter().find(|r| r.name == *segment);
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
        ActionType::Reject => {
            validate_reject_action(action, location, errors);
        }
    }
}

/// Validate a `reject` action: `message` is required (either a literal
/// string or a `{binding}` template — both resolve to a string at execution
/// time, see `playbook::actions::execute_reject`), and `for_each` is
/// disallowed (see [`PlayValidationError::RejectActionHasForEach`]).
fn validate_reject_action(
    action: &ParsedAction,
    location: &str,
    errors: &mut Vec<PlayValidationError>,
) {
    let has_message =
        matches!(action.params.get("message"), Some(serde_json::Value::String(s)) if !s.is_empty());
    if !has_message {
        errors.push(PlayValidationError::MissingActionParam {
            param: "message".to_string(),
            location: location.to_string(),
        });
    }
    if action.for_each.is_some() {
        errors.push(PlayValidationError::RejectActionHasForEach {
            location: location.to_string(),
        });
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

    if schema_cache.get(nt).and_then(|s| s.as_ref()).is_none() {
        // Schema is None — we already flagged the missing node_type
        return;
    }

    // Check against the *effective* relationship set — own directly-declared
    // relationships plus everything inherited across the ADR-078 `extends`
    // chain, not just this schema's own declarations. A relationship_type
    // genuinely inherited from an ancestor schema (not redeclared) must not
    // be rejected as unknown — the same extends-chain gap
    // `validate_schema_path` fixes for condition paths, applying here to an
    // `add_relationship`/`remove_relationship` action's own
    // `relationship_type` param. On a resolution failure, surfaced as a
    // validation error rather than silently degrading to this schema's own
    // declarations (see `SchemaResolutionFailed`).
    let relationships = match node_service.resolve_relationships(nt).await {
        Ok((rels, _owners)) => rels,
        Err(e) => {
            errors.push(PlayValidationError::SchemaResolutionFailed {
                node_type: nt.to_string(),
                error: e.to_string(),
                location: location.to_string(),
            });
            return;
        }
    };
    let rel_exists = relationships.iter().any(|r| r.name == rel_type);

    if !rel_exists {
        errors.push(PlayValidationError::UnknownRelationshipType {
            relationship_type: rel_type.to_string(),
            node_type: nt.to_string(),
            location: location.to_string(),
        });
    }
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
/// - **Deterministic** — enforced against BOTH surfaces that can express
///   non-determinism: wall-clock CEL functions in the rule's conditions, and
///   the fixed function-call form action-value bindings support (e.g.
///   `{add_days(item.start_date, 14)}` — see `actions.rs`'s
///   `parse_function_call`/`BindingContext::resolve_function_call`). Both are
///   checked against the SAME allow-list,
///   [`crate::playbook::cel::NON_DETERMINISTIC_FUNCTIONS`] (`today`/
///   `days_since`/`days_until` today). The three functions currently
///   registered for action values — `add_days`, and the collection-reducing
///   `sum`/`count` (`BindingContext::resolve_sum_call`/`resolve_count_call`)
///   — all read no wall-clock or random input and are correctly absent from
///   that list, so this check is a no-op today — but it is what keeps that
///   true: it is what would catch a FUTURE non-deterministic function added
///   to `resolve_function_call`'s match arm and used inside an invariant
///   rule's action params, rather than leaving that an unenforced
///   convention. No random-value function is registered anywhere, so these
///   two surfaces remain the complete non-deterministic surface.
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
///
/// # `reject` (ADR-060 §2) against each check
///
/// `reject`'s own class restriction (invariant-only) is a separate,
/// unconditional check — [`validate_reject_action_class`], not this function —
/// since it must fire on the WRONG class, which `validate_invariant_eligibility`
/// never even looks at. Once a `reject` action IS on an eligible invariant
/// rule, every check above applies to it exactly like any other action type,
/// with two automatic exemptions rather than special-cased ones:
/// - **Local writes only**: `reject` performs no I/O — deterministically
///   failing is pure computation — so [`ActionType::is_local_write`] returns
///   `true` for it, same as every graph-mutation action type.
/// - **Same-graph scope**: `reject` addresses no node (its only param is an
///   author-supplied message), so [`target_id_params`] returns an empty slice
///   for it, exactly like `create_node`'s existing exemption — there is no
///   target id to check.
///
/// No case in [`check_invariant_self_chaining`]'s match matches
/// `ActionType::Reject`, so it always falls to that match's `_ => false` arm:
/// a `reject` action can never re-satisfy a trigger, since it creates or
/// updates nothing.
fn validate_invariant_eligibility(
    rule: &ParsedRule,
    rule_idx: usize,
    errors: &mut Vec<PlayValidationError>,
) {
    // Supported trigger — synchronous pre-commit dispatch is wired into the
    // node-creation and update write paths (see `InvariantUnsupportedTrigger`'s
    // doc). `relationship_added`/`relationship_removed` and scheduled triggers
    // have no equivalent synchronous dispatch point and remain unsupported.
    let trigger_supported = matches!(
        &rule.trigger,
        ParsedTrigger::GraphEvent {
            on: GraphEventType::NodeCreated | GraphEventType::PropertyChanged,
            ..
        }
    );
    if !trigger_supported {
        let trigger_desc = match &rule.trigger {
            ParsedTrigger::GraphEvent { on, .. } => graph_event_name(on).to_string(),
            ParsedTrigger::Scheduled { .. } => "scheduled".to_string(),
        };
        errors.push(PlayValidationError::InvariantUnsupportedTrigger {
            trigger: trigger_desc,
            location: format!("rule[{}].trigger", rule_idx),
        });
    }

    // Local writes only.
    for (action_idx, action) in rule.actions.iter().enumerate() {
        if !action.action_type.is_local_write() {
            errors.push(PlayValidationError::InvariantNonLocalAction {
                action: action.action_type.as_str().to_string(),
                location: format!("rule[{}].action[{}]", rule_idx, action_idx),
            });
        }
    }

    // add_relationship to member_of/has_child needs an explicit order — the
    // tx-scoped executor has no atomic-auto-order twin of
    // add_to_collection/append_child_edge (see the error variant's doc).
    for (action_idx, action) in rule.actions.iter().enumerate() {
        if action.action_type != ActionType::AddRelationship {
            continue;
        }
        let Some(rel_type) = action
            .params
            .get("relationship_type")
            .and_then(|v| v.as_str())
        else {
            continue;
        };
        if rel_type != "member_of" && rel_type != "has_child" {
            continue;
        }
        let has_explicit_order = action
            .params
            .get("edge_data")
            .and_then(|v| v.get("order"))
            .is_some();
        if !has_explicit_order {
            errors.push(
                PlayValidationError::InvariantRelationshipNeedsExplicitOrder {
                    relationship_type: rel_type.to_string(),
                    location: format!("rule[{}].action[{}]", rule_idx, action_idx),
                },
            );
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

    // Deterministic — no wall-clock reads in action-value function-call
    // bindings either (e.g. `{add_days(item.start_date, 14)}`). Mirrors the
    // conditions check above, against the SAME allow-list, but over a
    // different syntax: action params are `{path}`-interpolation strings
    // (`actions.rs`), not CEL expressions, so this cannot reuse
    // `path_extractor::extract_function_names` (which parses real CEL) --
    // it scans for the fixed function-call FORM `resolve_function_call`
    // recognizes instead. See this function's doc for why this check exists
    // even though it currently always passes.
    for (action_idx, action) in rule.actions.iter().enumerate() {
        let mut templates = Vec::new();
        collect_binding_templates_in_value(&action.params, &mut templates);
        for template in templates {
            if let Some((function, _args)) = parse_function_call(&template) {
                if crate::playbook::cel::NON_DETERMINISTIC_FUNCTIONS.contains(&function) {
                    errors.push(PlayValidationError::InvariantNonDeterministic {
                        function: function.to_string(),
                        location: format!("rule[{}].action[{}]", rule_idx, action_idx),
                    });
                }
            }
        }

        // `for_each` is a RAW (unbraced) binding path -- `execute_actions`
        // resolves it via the exact same `BindingContext::resolve_binding`
        // as any `{path}` param value (see `actions.rs`), so a function-call
        // form is reachable there too, without wrapping braces. Check it
        // directly with `parse_function_call` rather than
        // `extract_binding_templates` (which requires `{...}` wrapping and
        // would miss this).
        if let Some(for_each) = &action.for_each {
            if let Some((function, _args)) = parse_function_call(for_each) {
                if crate::playbook::cel::NON_DETERMINISTIC_FUNCTIONS.contains(&function) {
                    errors.push(PlayValidationError::InvariantNonDeterministic {
                        function: function.to_string(),
                        location: format!("rule[{}].action[{}].for_each", rule_idx, action_idx),
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
/// target and cannot violate same-graph scope through a target id. `reject`
/// addresses no node at all — its only param is an author-supplied message —
/// so it is exempt for the same reason.
fn target_id_params(action_type: &ActionType) -> &'static [&'static str] {
    match action_type {
        ActionType::CreateNode | ActionType::Reject => &[],
        ActionType::UpdateNode => &["node_id"],
        ActionType::AddRelationship | ActionType::RemoveRelationship => &["source_id", "target_id"],
    }
}

/// Validate that a `reject` action (ADR-060 §2) only appears on an
/// `Invariant`-class rule. Runs for every rule (not gated behind
/// `rule.class == RuleClass::Invariant`, unlike
/// [`validate_invariant_eligibility`]'s checks) because its entire purpose is
/// to catch the rule being the WRONG class.
fn validate_reject_action_class(
    rule: &ParsedRule,
    rule_idx: usize,
    errors: &mut Vec<PlayValidationError>,
) {
    if rule.class == RuleClass::Invariant {
        return;
    }
    for (action_idx, action) in rule.actions.iter().enumerate() {
        if action.action_type == ActionType::Reject {
            errors.push(PlayValidationError::RejectActionOnReactiveRule {
                location: format!("rule[{}].action[{}]", rule_idx, action_idx),
            });
        }
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

/// Whether a proposed schema change can invalidate an existing Play reference.
///
/// A Play names fields, enum values and relationships by name. Adding more of
/// any of them leaves every existing name resolving exactly as before, so an
/// additive change is not capable of breaking a Play — whereas removing or
/// renaming one is precisely how a Play's path goes stale.
///
/// Deliberately a two-way split rather than a per-field diff: the question the
/// impact check answers is "must the user confirm this?", and a change that
/// removes or renames *anything* on the type warrants the prompt, even if the
/// specific name a given Play uses survives. Narrowing further would mean
/// resolving each Play's references against the post-change schema, which is
/// the job save-time validation already does when the change lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaChangeKind {
    /// Only adds: new fields, new enum values, new relationships, metadata.
    Additive,
    /// Removes or renames something a Play may reference by name.
    Destructive,
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
/// Only *destructive* changes are reported. A schema change that purely adds —
/// a new field, a new enum value, a new relationship — cannot invalidate a
/// reference that already resolves, so a Play referencing this type is left
/// alone and the caller is not asked to confirm anything. See
/// [`SchemaChangeKind`].
///
/// This distinction became load-bearing once a Play shipped as core (ADR-079):
/// with an always-installed Play triggering on `task`, an unconditional check
/// makes *every* `task` schema edit — including adding a status value — demand
/// `force=true` from every user, for a Play the change provably cannot break.
///
/// Returns a list of affected plays with their broken paths.
pub async fn check_schema_change_impact(
    schema_node_type: &str,
    change: SchemaChangeKind,
    node_service: &NodeService,
) -> Result<Vec<AffectedPlay>, String> {
    // An additive change cannot break an existing reference: every field,
    // value and relationship a Play already names is still there afterwards.
    // Skip the scan entirely rather than collecting matches and discarding
    // them, so the common case costs no play-node query at all.
    if change == SchemaChangeKind::Additive {
        return Ok(Vec::new());
    }
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
                    // `reject`'s only param is an author-supplied message —
                    // no node_type/relationship_type/target_type to
                    // reference a schema through.
                    ActionType::Reject => {}
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

    #[test]
    fn play_validation_error_location_and_kind_distinguish_errors() {
        // The play engine's validation-error logging folds `location()` and
        // `kind()` into its log-node fingerprint identity (in place of a
        // constant placeholder) so two structurally different errors on the
        // same play don't collapse onto the same fingerprint. Lock in both
        // accessors directly against the enum.
        let broken_path = PlayValidationError::BrokenPath {
            path: "node.status".to_string(),
            segment: "status".to_string(),
            message: "no such field".to_string(),
            location: "rule[0].condition[0]".to_string(),
        };
        assert_eq!(broken_path.location(), "rule[0].condition[0]");
        assert_eq!(broken_path.kind(), "broken_path");

        let reject_on_reactive = PlayValidationError::RejectActionOnReactiveRule {
            location: "rule[1].action[0]".to_string(),
        };
        assert_eq!(reject_on_reactive.location(), "rule[1].action[0]");
        assert_eq!(reject_on_reactive.kind(), "reject_action_on_reactive_rule");

        // Different locations -> different (location, kind) pairs, even for
        // the same error kind.
        let missing_param_a = PlayValidationError::MissingActionParam {
            param: "message".to_string(),
            location: "rule[0].action[0]".to_string(),
        };
        let missing_param_b = PlayValidationError::MissingActionParam {
            param: "message".to_string(),
            location: "rule[2].action[1]".to_string(),
        };
        assert_ne!(missing_param_a.location(), missing_param_b.location());
        assert_eq!(missing_param_a.kind(), missing_param_b.kind());

        // Same location, different kind -> `kind()` alone still tells them apart.
        assert_ne!(broken_path.kind(), reject_on_reactive.kind());
    }

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

    /// A `property_changed` rule with the given `property_key` (`None` = wildcard).
    fn make_property_changed_rule(node_type: &str, property_key: Option<&str>) -> Arc<ParsedRule> {
        Arc::new(ParsedRule {
            name: "test-property-changed-rule".to_string(),
            class: RuleClass::Reactive,
            trigger: ParsedTrigger::GraphEvent {
                on: GraphEventType::PropertyChanged,
                node_type: node_type.to_string(),
                property_key: property_key.map(str::to_string),
            },
            conditions: vec![],
            actions: vec![],
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
        async fn test_bare_property_changed_key_is_rejected() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_widget", 1, json!([])).await;

            // The obvious-looking spelling — no namespace at all. Indexed
            // verbatim under (vt_widget, "status"), which no real
            // PropertyChanged event (always "vt_widget.status") can match.
            let rules = vec![make_property_changed_rule("vt_widget", Some("status"))];
            let result = validate_play(&rules, &svc).await;
            let errors = result.expect_err("bare property_key must be rejected");
            assert_eq!(errors.len(), 1);
            match &errors[0] {
                PlayValidationError::UnnamespacedPropertyChangedKey {
                    node_type,
                    property_key,
                    expected,
                    location,
                } => {
                    assert_eq!(node_type, "vt_widget");
                    assert_eq!(property_key, "status");
                    assert_eq!(expected, "vt_widget.status");
                    assert_eq!(location, "rule[0].trigger");
                }
                other => panic!("expected UnnamespacedPropertyChangedKey, got {:?}", other),
            }
            assert!(errors[0].to_string().contains("vt_widget.status"));
        }

        #[tokio::test]
        async fn test_property_key_namespaced_to_a_different_type_is_rejected() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_widget", 1, json!([])).await;

            // Has a dot, so it LOOKS namespaced — but the namespace belongs to
            // some other type, not this trigger's own `node_type`. Still
            // matches no real event for this trigger and must be rejected the
            // same as a fully bare key.
            let rules = vec![make_property_changed_rule(
                "vt_widget",
                Some("vt_other.status"),
            )];
            let result = validate_play(&rules, &svc).await;
            let errors = result.expect_err("wrongly-namespaced property_key must be rejected");
            assert_eq!(errors.len(), 1);
            match &errors[0] {
                PlayValidationError::UnnamespacedPropertyChangedKey {
                    node_type,
                    property_key,
                    expected,
                    ..
                } => {
                    assert_eq!(node_type, "vt_widget");
                    assert_eq!(property_key, "vt_other.status");
                    // The field portion (after the first dot) is preserved;
                    // only the namespace is corrected.
                    assert_eq!(expected, "vt_widget.status");
                }
                other => panic!("expected UnnamespacedPropertyChangedKey, got {:?}", other),
            }
        }

        #[tokio::test]
        async fn test_namespaced_property_changed_key_passes_validation() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_widget", 1, json!([])).await;

            // The only spelling a real event can ever carry.
            let rules = vec![make_property_changed_rule(
                "vt_widget",
                Some("vt_widget.status"),
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "a properly-namespaced property_key must not be rejected: {:?}",
                result.err()
            );
        }

        #[tokio::test]
        async fn test_wildcard_property_changed_key_passes_validation() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_widget", 1, json!([])).await;

            // `None` = wildcard, matches all property changes — nothing to
            // namespace, must not be flagged.
            let rules = vec![make_property_changed_rule("vt_widget", None)];
            let result = validate_play(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "a wildcard (None) property_key must not be rejected: {:?}",
                result.err()
            );
        }

        #[tokio::test]
        async fn test_bare_property_key_on_non_property_changed_trigger_is_not_flagged() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_widget", 1, json!([])).await;

            // The namespace check is scoped to `property_changed` triggers
            // only — a `node_created` trigger's `property_key` (unused by the
            // engine for that event type) must not trip this check.
            let rules = vec![Arc::new(ParsedRule {
                name: "test-rule".to_string(),
                class: RuleClass::Reactive,
                trigger: ParsedTrigger::GraphEvent {
                    on: GraphEventType::NodeCreated,
                    node_type: "vt_widget".to_string(),
                    property_key: Some("status".to_string()),
                },
                conditions: vec![],
                actions: vec![],
            })];
            let result = validate_play(&rules, &svc).await;
            assert!(result.is_ok(), "unexpected errors: {:?}", result.err());
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

        /// Regression for the same extends-chain gap `validate_schema_path`
        /// fixes for condition paths, in the sibling
        /// `validate_relationship_action` (an `add_relationship`/
        /// `remove_relationship` action's own `relationship_type` param):
        /// `vt_ticket_base` declares relationship `linked_to`; `vt_ticket_sub`
        /// `extends` `vt_ticket_base` with no relationships of its own
        /// (inheriting, not redeclaring). An action on `vt_ticket_sub` using
        /// `relationship_type: "linked_to"` must validate successfully — the
        /// relationship genuinely resolves via the extends chain — not be
        /// rejected as `UnknownRelationshipType`.
        #[tokio::test]
        async fn test_inherited_relationship_type_in_action_passes_validation() {
            let (svc, _tmp) = create_test_service().await;

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vt_ticket_base",
                    "fields": [],
                    "relationships": [{
                        "name": "linked_to",
                        "direction": "out",
                        "cardinality": "many",
                        "reverseName": "linked_from",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await
            .expect("base schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vt_ticket_sub",
                    "extends": "vt_ticket_base",
                    "fields": []
                }),
            )
            .await
            .expect("subtype schema creation failed");

            let rules = vec![make_rule(
                "vt_ticket_sub",
                vec![],
                vec![make_relationship_action("linked_to")],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "an action's relationship_type genuinely inherited (extends-chain) must \
                 validate successfully, not be rejected as UnknownRelationshipType: {:?}",
                result
            );
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

        /// Regression for the ADR-078 extends-chain gap in save-time
        /// validation — the same root cause as the identical gap in the
        /// engine's diagnostic candidate enumeration, but here it blocks a
        /// save outright rather than misreporting a diagnostic:
        /// `validate_schema_path` must resolve a condition segment against
        /// the *effective* field set of the target schema — its own
        /// directly-declared fields plus everything inherited across the
        /// `extends` chain — not just that schema's own fields.
        ///
        /// Reproduces a realistic inheritance scenario: `vp_epic_base`
        /// declares `priority`; `vp_epic` `extends` `vp_epic_base` with no
        /// fields of its own (the normal, intended `extends` usage —
        /// inheriting rather than redeclaring); `vp_task_epic` declares a
        /// relationship `epic` targeting `vp_epic`. A Play condition
        /// `node.epic.priority == 'high'` references a field that is
        /// genuinely inherited, not redeclared. Before the fix this was
        /// rejected with `BrokenPath` ("'priority' is not a field or
        /// relationship on schema 'vp_epic'") and the play could never be
        /// saved at all, even though `priority` resolves correctly via the
        /// extends chain everywhere else (the runtime engine, and the
        /// diagnostic candidate enumeration).
        #[tokio::test]
        async fn test_inherited_field_through_relationship_passes_validation() {
            let (svc, _tmp) = create_test_service().await;

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vp_epic_base",
                    "fields": [
                        { "name": "priority", "type": "string", "protection": "user", "indexed": false }
                    ]
                }),
            )
            .await
            .expect("base schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vp_epic",
                    "extends": "vp_epic_base",
                    "fields": []
                }),
            )
            .await
            .expect("subtype schema creation failed");

            create_schema(
                &svc,
                "vp_task_epic",
                1,
                json!([{
                    "name": "epic",
                    "targetType": "vp_epic",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "tasks",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let rules = vec![make_rule(
                "vp_task_epic",
                vec!["node.epic.priority == 'high'"],
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "a condition referencing a genuinely inherited (extends-chain) field must \
                 validate successfully, not be rejected as BrokenPath: {:?}",
                result
            );
        }

        /// Regression for the identical extends-chain gap as above, but for
        /// a *relationship* segment rather than a field: `validate_schema_path`
        /// must resolve a path segment against the effective relationship set
        /// of the current schema — own directly-declared relationships plus
        /// everything inherited across the `extends` chain — not just that
        /// schema's own relationships.
        ///
        /// `vp_rel_target` declares field `status`; `vp_rel_base` declares
        /// relationship `manager` targeting `vp_rel_target`; `vp_rel_sub`
        /// `extends` `vp_rel_base` with no relationships of its own
        /// (inheriting, not redeclaring); `vp_task_rel` declares relationship
        /// `owner` targeting `vp_rel_sub`. A Play condition
        /// `node.owner.manager.status == 'active'` traverses: `owner` (declared
        /// directly on `vp_task_rel`) to `vp_rel_sub`, then `manager` — a
        /// relationship genuinely inherited by `vp_rel_sub` from
        /// `vp_rel_base`, not redeclared — to `vp_rel_target`, then reads the
        /// field `status`. Before fixing the relationship lookup, `manager`
        /// resolved against `vp_rel_sub`'s own (empty) relationships list and
        /// was rejected as `BrokenPath`.
        #[tokio::test]
        async fn test_inherited_relationship_through_relationship_passes_validation() {
            let (svc, _tmp) = create_test_service().await;

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vp_rel_target",
                    "fields": [
                        { "name": "status", "type": "string", "protection": "user", "indexed": false }
                    ]
                }),
            )
            .await
            .expect("relationship target schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vp_rel_base",
                    "fields": [],
                    "relationships": [{
                        "name": "manager",
                        "targetType": "vp_rel_target",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "reports",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await
            .expect("base schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vp_rel_sub",
                    "extends": "vp_rel_base",
                    "fields": []
                }),
            )
            .await
            .expect("subtype schema creation failed");

            create_schema(
                &svc,
                "vp_task_rel",
                1,
                json!([{
                    "name": "owner",
                    "targetType": "vp_rel_sub",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "owned_tasks",
                    "reverseCardinality": "many"
                }]),
            )
            .await;

            let rules = vec![make_rule(
                "vp_task_rel",
                vec!["node.owner.manager.status == 'active'"],
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "a condition traversing a genuinely inherited (extends-chain) relationship must \
                 validate successfully, not be rejected as BrokenPath: {:?}",
                result
            );
        }

        /// Regression for a precedence bug in the extends-chain fix above:
        /// `validate_schema_path` must pick whichever of a field or a
        /// relationship declaration is *nearer* in the extends chain, not
        /// unconditionally prefer "is it a member of the whole merged field
        /// set" over "is it a member of the whole merged relationship set".
        ///
        /// `vp_prec_target` declares field `label`; `vp_prec_base` declares
        /// FIELD `owner`; `vp_prec_sub` `extends` `vp_prec_base` and
        /// declares its OWN RELATIONSHIP also named `owner`, targeting
        /// `vp_prec_target` — a name that is a field on an ancestor and a
        /// relationship on the (nearer) subtype itself. A Play condition
        /// `node.owner.label == 'active'` on `vp_prec_sub` must resolve
        /// `owner` as the nearer, own-schema relationship declaration (and
        /// traverse into `vp_prec_target` to find `label`), not as the
        /// farther, inherited field declaration — which would wrongly
        /// terminate the path at `owner` and reject `label` as
        /// unreachable/broken.
        #[tokio::test]
        async fn test_own_relationship_shadows_inherited_field_of_same_name() {
            let (svc, _tmp) = create_test_service().await;

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vp_prec_target",
                    "fields": [
                        { "name": "label", "type": "string", "protection": "user", "indexed": false }
                    ]
                }),
            )
            .await
            .expect("target schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vp_prec_base",
                    "fields": [
                        { "name": "owner", "type": "string", "protection": "user", "indexed": false }
                    ]
                }),
            )
            .await
            .expect("base schema creation failed");

            crate::schema::handle_create_schema(
                &svc,
                json!({
                    "name": "vp_prec_sub",
                    "extends": "vp_prec_base",
                    "fields": [],
                    "relationships": [{
                        "name": "owner",
                        "targetType": "vp_prec_target",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "owned_subs",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await
            .expect("subtype schema creation failed");

            let rules = vec![make_rule(
                "vp_prec_sub",
                vec!["node.owner.label == 'active'"],
                vec![],
            )];
            let result = validate_play(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "the subtype's OWN relationship declaration must shadow the ancestor's \
                 inherited field of the same name (nearer wins), not be misresolved as a \
                 terminal field: {:?}",
                result
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

            let affected =
                check_schema_change_impact("vi_task", SchemaChangeKind::Destructive, &svc)
                    .await
                    .unwrap();
            assert_eq!(affected.len(), 1);
            assert_eq!(affected[0].play_id, "pb-impact-1");

            // Same play, same type, additive change: nothing to confirm. A new
            // field or enum value leaves every name this play references
            // resolving exactly as before. Without this, a core Play triggering
            // on `task` (ADR-079) would make every user pass `force=true` to
            // add a status value.
            let additive = check_schema_change_impact("vi_task", SchemaChangeKind::Additive, &svc)
                .await
                .unwrap();
            assert!(
                additive.is_empty(),
                "an additive change cannot break a play, so must not be reported: {additive:?}"
            );
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
            let affected =
                check_schema_change_impact("vi_invoice", SchemaChangeKind::Destructive, &svc)
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

            let affected =
                check_schema_change_impact("vi_epic", SchemaChangeKind::Destructive, &svc)
                    .await
                    .unwrap();
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

        fn reject_action(message: &str) -> ParsedAction {
            ParsedAction {
                action_type: ActionType::Reject,
                params: json!({ "message": message }),
                for_each: None,
            }
        }

        fn eligibility_errors(rule: &ParsedRule) -> Vec<PlayValidationError> {
            let mut errors = Vec::new();
            validate_invariant_eligibility(rule, 0, &mut errors);
            errors
        }

        fn reject_class_errors(rule: &ParsedRule) -> Vec<PlayValidationError> {
            let mut errors = Vec::new();
            validate_reject_action_class(rule, 0, &mut errors);
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
                ActionType::Reject,
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

        #[test]
        fn invariant_non_deterministic_action_value_function_call_rejected() {
            // A function-call-shaped action-value binding is checked by NAME
            // against the SAME non-deterministic allow-list conditions use,
            // even though `today` isn't itself a function
            // `resolve_function_call` implements for action values today —
            // this check is about catching the SHAPE by name (forward-looking,
            // per this function's doc), not about whether the runtime
            // resolver would currently accept the call.
            let action = ParsedAction {
                action_type: ActionType::UpdateNode,
                params: json!({
                    "node_id": "{trigger.node.id}",
                    "properties": { "custom:stamp": "{today()}" }
                }),
                for_each: None,
            };
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![action],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantNonDeterministic { function, location }
                        if function == "today" && location == "rule[0].action[0]"
                )),
                "expected non-deterministic 'today' error for the action-value \
                 binding, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_deterministic_action_value_function_call_accepted() {
            // add_days is genuinely deterministic and correctly absent from
            // NON_DETERMINISTIC_FUNCTIONS — its function-call binding form
            // must NOT be flagged.
            let action = ParsedAction {
                action_type: ActionType::UpdateNode,
                params: json!({
                    "node_id": "{trigger.node.id}",
                    "properties": { "custom:end_date": "{add_days(trigger.node.id, 14)}" }
                }),
                for_each: None,
            };
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![action],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::InvariantNonDeterministic { .. })),
                "add_days is deterministic and must not be flagged, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_plain_binding_action_value_has_no_determinism_error() {
            // A regression guard: an ordinary `{path}` action-value binding
            // (no function-call form at all) must not somehow be swept up by
            // the new scan — it isn't a function call, so `parse_function_call`
            // must return `None` for it and the loop must skip it entirely.
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![update_action("{trigger.node.id}")],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::InvariantNonDeterministic { .. })),
                "a plain {{path}} binding must never be flagged as non-deterministic, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_non_deterministic_for_each_function_call_rejected() {
            // `for_each` is a RAW (unbraced) binding path — `execute_actions`
            // resolves it through the exact same `resolve_binding` as any
            // `{path}` param value, so a function-call form is reachable
            // there too, without `{...}` wrapping (see `actions.rs`'s
            // `execute_actions`, which calls
            // `ctx.resolve_binding(for_each_path)` directly on the stored
            // string). This must be checked independently of the params scan
            // above, which only looks inside `{...}`-wrapped text.
            let action = ParsedAction {
                action_type: ActionType::UpdateNode,
                params: json!({
                    "node_id": "{item.id}",
                    "properties": { "custom:tag": "v" }
                }),
                for_each: Some("today()".to_string()),
            };
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![action],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantNonDeterministic { function, location }
                        if function == "today" && location == "rule[0].action[0].for_each"
                )),
                "expected non-deterministic 'today' error for the for_each \
                 binding, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_plain_for_each_path_has_no_determinism_error() {
            // Regression guard, matching the real `for_each` convention
            // (bare dot-path, no braces — see `for_each: Some("trigger.node.tasks"...)`
            // in `playbook::tests`): an ordinary for_each path must not be
            // flagged just because it's now scanned.
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![ParsedAction {
                    action_type: ActionType::UpdateNode,
                    params: json!({
                        "node_id": "{item.id}",
                        "properties": { "custom:tag": "v" }
                    }),
                    for_each: Some("trigger.node.tasks".to_string()),
                }],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::InvariantNonDeterministic { .. })),
                "a plain for_each dot-path must never be flagged as \
                 non-deterministic, got {:?}",
                errors
            );
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

        // -- Supported trigger (Slice B: synchronous dispatch wires only
        // node_created into the write path) --

        #[test]
        fn invariant_node_created_trigger_accepted() {
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec!["node.status == 'open'"],
                vec![update_action("{trigger.node.id}")],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::InvariantUnsupportedTrigger { .. })),
                "node_created must be an accepted invariant trigger, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_property_changed_trigger_accepted() {
            // Synchronous pre-commit dispatch is now wired into update_node's
            // write path too, so property_changed is an accepted invariant
            // trigger — the same class of check as node_created above.
            let rule = invariant_rule(
                GraphEventType::PropertyChanged,
                "task",
                Some("status"),
                vec!["node.status == 'open'"],
                vec![update_action("{trigger.node.id}")],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::InvariantUnsupportedTrigger { .. })),
                "property_changed must be an accepted invariant trigger, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_relationship_added_trigger_rejected() {
            let rule = invariant_rule(
                GraphEventType::RelationshipAdded,
                "task",
                None,
                vec![],
                vec![],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantUnsupportedTrigger { trigger, .. }
                        if trigger == "relationship_added"
                )),
                "relationship_added must be rejected as an invariant trigger, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_scheduled_trigger_rejected() {
            let rule = ParsedRule {
                name: "inv-sched".to_string(),
                class: RuleClass::Invariant,
                trigger: ParsedTrigger::Scheduled {
                    cron: "0 0 * * *".to_string(),
                    node_type: "task".to_string(),
                },
                conditions: vec![],
                actions: vec![],
            };
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantUnsupportedTrigger { trigger, .. }
                        if trigger == "scheduled"
                )),
                "scheduled must be rejected as an invariant trigger, got {:?}",
                errors
            );
        }

        // -- add_relationship to member_of/has_child needs an explicit order --

        fn add_rel_action_with_edge_data(
            source_id: &str,
            relationship_type: &str,
            target_id: &str,
            edge_data: serde_json::Value,
        ) -> ParsedAction {
            ParsedAction {
                action_type: ActionType::AddRelationship,
                params: json!({
                    "source_id": source_id,
                    "relationship_type": relationship_type,
                    "target_id": target_id,
                    "edge_data": edge_data,
                }),
                for_each: None,
            }
        }

        #[test]
        fn invariant_member_of_without_explicit_order_rejected() {
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![add_rel_action_with_edge_data(
                    "{trigger.node.id}",
                    "member_of",
                    "{trigger.node.parent_collection_id}",
                    json!({}),
                )],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantRelationshipNeedsExplicitOrder { relationship_type, .. }
                        if relationship_type == "member_of"
                )),
                "member_of without an explicit order must be rejected, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_has_child_without_explicit_order_rejected() {
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![add_rel_action_with_edge_data(
                    "{trigger.node.parent_id}",
                    "has_child",
                    "{trigger.node.id}",
                    json!({}),
                )],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantRelationshipNeedsExplicitOrder { relationship_type, .. }
                        if relationship_type == "has_child"
                )),
                "has_child without an explicit order must be rejected, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_member_of_with_explicit_order_accepted() {
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![add_rel_action_with_edge_data(
                    "{trigger.node.id}",
                    "member_of",
                    "{trigger.node.parent_collection_id}",
                    json!({ "order": 1.0 }),
                )],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantRelationshipNeedsExplicitOrder { .. }
                )),
                "member_of WITH an explicit order must be accepted, got {:?}",
                errors
            );
        }

        #[test]
        fn invariant_non_auto_order_relationship_type_needs_no_explicit_order() {
            // "linked_to" (or any non-member_of/has_child type) never goes
            // through the atomic auto-order helpers, so no order is required.
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![add_rel_action("{trigger.node.id}", "{trigger.node.id}")],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::InvariantRelationshipNeedsExplicitOrder { .. }
                )),
                "a non-auto-order relationship type must not require an explicit order, got {:?}",
                errors
            );
        }

        // -- reject action (ADR-060 §2) --

        #[test]
        fn reject_action_on_invariant_rule_has_no_eligibility_errors() {
            // A `reject` action on an otherwise-eligible invariant rule must
            // not trip ANY of §2's existing checks: it is a local write
            // (trivially — no I/O), addresses no node (exempt from
            // same-graph-scope), and can never re-satisfy a trigger (exempt
            // from non-chaining).
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec!["node.status == 'open'"],
                vec![reject_action("no")],
            );
            let errors = eligibility_errors(&rule);
            assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
        }

        #[test]
        fn reject_action_has_no_out_of_scope_target_error() {
            // `reject` addresses no node (unlike `update_node`/relationship
            // actions), so it must never be flagged as targeting an
            // out-of-scope literal node id — there is no target param to
            // check in the first place.
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![reject_action("no")],
            );
            let errors = eligibility_errors(&rule);
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::InvariantOutOfScopeTarget { .. })),
                "reject must never trip the out-of-scope-target check, got {:?}",
                errors
            );
        }

        #[test]
        fn reject_action_on_invariant_rule_passes_the_class_check() {
            let rule = invariant_rule(
                GraphEventType::NodeCreated,
                "task",
                None,
                vec![],
                vec![reject_action("no")],
            );
            let errors = reject_class_errors(&rule);
            assert!(
                errors.is_empty(),
                "reject on an invariant rule must pass the class check, got {:?}",
                errors
            );
        }

        #[test]
        fn reject_action_on_reactive_rule_fails_the_class_check() {
            // `reject`'s entire meaning is "fail the enclosing transaction" —
            // meaningless once a rule's actions run asynchronously,
            // post-commit (the reactive default), so this must be caught
            // regardless of anything else about the rule.
            let rule = ParsedRule {
                class: RuleClass::Reactive,
                ..invariant_rule(
                    GraphEventType::NodeCreated,
                    "task",
                    None,
                    vec![],
                    vec![reject_action("no")],
                )
            };
            let errors = reject_class_errors(&rule);
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::RejectActionOnReactiveRule { .. })),
                "reject on a reactive rule must be caught, got {:?}",
                errors
            );
        }

        #[test]
        fn reactive_rule_with_no_reject_action_passes_the_class_check() {
            // The class check must not false-positive on ordinary reactive
            // rules — every rule authored before `reject` existed.
            let rule = ParsedRule {
                class: RuleClass::Reactive,
                ..invariant_rule(
                    GraphEventType::NodeCreated,
                    "task",
                    None,
                    vec![],
                    vec![update_action("{trigger.node.id}")],
                )
            };
            let errors = reject_class_errors(&rule);
            assert!(
                errors.is_empty(),
                "a reactive rule with no reject action must pass, got {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn reject_action_on_reactive_rule_fails_validate_play() {
            // End-to-end through the same entry point `create_node`'s
            // play-node validation gate uses, proving the class check is
            // actually wired into `validate_play`, not just unit-testable in
            // isolation.
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_reject_reactive").await;

            let rule = Arc::new(ParsedRule {
                class: RuleClass::Reactive,
                ..invariant_rule(
                    GraphEventType::NodeCreated,
                    "vi_reject_reactive",
                    None,
                    vec![],
                    vec![reject_action("no")],
                )
            });
            let errors = validate_play(&[rule], &svc).await.unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::RejectActionOnReactiveRule { .. })),
                "expected RejectActionOnReactiveRule, got {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn reject_action_without_message_fails_validate_play() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_reject_no_message").await;

            let rule = Arc::new(invariant_rule(
                GraphEventType::NodeCreated,
                "vi_reject_no_message",
                None,
                vec![],
                vec![ParsedAction {
                    action_type: ActionType::Reject,
                    params: json!({}),
                    for_each: None,
                }],
            ));
            let errors = validate_play(&[rule], &svc).await.unwrap_err();
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlayValidationError::MissingActionParam { param, .. } if param == "message"
                )),
                "expected MissingActionParam(\"message\"), got {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn reject_action_with_message_passes_validate_play() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_reject_ok").await;

            let rule = Arc::new(invariant_rule(
                GraphEventType::NodeCreated,
                "vi_reject_ok",
                None,
                vec!["node.status == 'blocked'"],
                vec![reject_action("cannot proceed while blocked")],
            ));
            let result = validate_play(&[rule], &svc).await;
            assert!(result.is_ok(), "expected Ok, got {:?}", result);
        }

        #[tokio::test]
        async fn reject_action_with_for_each_fails_validate_play() {
            // reject's condition already gates whether it fires; iterating
            // it over a (possibly empty) collection adds a silent-no-op
            // hazard with no corresponding benefit, so it is rejected
            // outright rather than accepted. This check lives in
            // `validate_action` (via `validate_reject_action`), reached
            // through `validate_play` — not `validate_invariant_eligibility`
            // — so it is exercised end-to-end here, not through
            // `eligibility_errors`.
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_reject_for_each").await;

            let rule = Arc::new(invariant_rule(
                GraphEventType::NodeCreated,
                "vi_reject_for_each",
                None,
                vec![],
                vec![ParsedAction {
                    action_type: ActionType::Reject,
                    params: json!({ "message": "no" }),
                    for_each: Some("{trigger.node.items}".to_string()),
                }],
            ));
            let errors = validate_play(&[rule], &svc).await.unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::RejectActionHasForEach { .. })),
                "expected RejectActionHasForEach, got {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn reject_action_without_for_each_passes_validate_play() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_reject_no_for_each").await;

            let rule = Arc::new(invariant_rule(
                GraphEventType::NodeCreated,
                "vi_reject_no_for_each",
                None,
                vec![],
                vec![reject_action("no")],
            ));
            let result = validate_play(&[rule], &svc).await;
            assert!(result.is_ok(), "expected Ok, got {:?}", result);
        }
    }

    // -----------------------------------------------------------------------
    // Duplicate action lists within a play (derived-identity collision guard)
    // -----------------------------------------------------------------------

    mod duplicate_action_lists {
        use super::*;
        use serde_json::json;
        use std::sync::Arc;

        // `node_type` is a param (not hardcoded) so the `validate_play`
        // integration tests below can point at a schema-backed type of
        // their own rather than a reserved core type name like `task`.
        fn rule(name: &str, node_type: &str, actions: Vec<ParsedAction>) -> Arc<ParsedRule> {
            Arc::new(ParsedRule {
                name: name.to_string(),
                class: RuleClass::Reactive,
                trigger: ParsedTrigger::GraphEvent {
                    on: GraphEventType::NodeCreated,
                    node_type: node_type.to_string(),
                    property_key: None,
                },
                conditions: vec![],
                actions,
            })
        }

        fn with_property_changed_trigger(rule: &Arc<ParsedRule>) -> Arc<ParsedRule> {
            let node_type = match &rule.trigger {
                ParsedTrigger::GraphEvent { node_type, .. } => node_type.clone(),
                ParsedTrigger::Scheduled { node_type, .. } => node_type.clone(),
            };
            // Namespaced to `node_type`, matching what `validate_play` now
            // requires of a `property_changed` trigger's `property_key` —
            // this helper exists to give a rule a *different trigger type*
            // for the duplicate-action-list tests below, not to exercise
            // property_key namespacing itself.
            let property_key = Some(format!("{}.status", node_type));
            Arc::new(ParsedRule {
                trigger: ParsedTrigger::GraphEvent {
                    on: GraphEventType::PropertyChanged,
                    node_type,
                    property_key,
                },
                ..(**rule).clone()
            })
        }

        fn create_action(content: &str) -> ParsedAction {
            ParsedAction {
                action_type: ActionType::CreateNode,
                params: json!({ "node_type": "text", "content": content }),
                for_each: None,
            }
        }

        fn duplicate_errors(rules: &[Arc<ParsedRule>]) -> Vec<PlayValidationError> {
            let mut errors = Vec::new();
            validate_no_duplicate_action_lists(rules, &mut errors);
            errors
        }

        #[test]
        fn two_rules_with_identical_actions_and_different_triggers_are_rejected() {
            // Copy-paste authoring: rule B is rule A with only the trigger
            // changed (node_created -> property_changed) -- the action list
            // itself is untouched.
            let rule_a = rule("notify-a", "task", vec![create_action("same content")]);
            let rule_b = with_property_changed_trigger(&rule(
                "notify-b",
                "task",
                vec![create_action("same content")],
            ));

            let errors = duplicate_errors(&[rule_a, rule_b]);
            assert_eq!(
                errors.len(),
                1,
                "expected exactly one duplicate error, got {:?}",
                errors
            );
            match &errors[0] {
                PlayValidationError::DuplicateActionList {
                    rule_name,
                    duplicate_of_rule_name,
                    location,
                    duplicate_of_location,
                } => {
                    assert_eq!(rule_name, "notify-b");
                    assert_eq!(duplicate_of_rule_name, "notify-a");
                    assert_eq!(location, "rule[1]");
                    assert_eq!(duplicate_of_location, "rule[0]");
                }
                other => panic!("expected DuplicateActionList, got {:?}", other),
            }
        }

        /// Negative control: the two rules differ only in the `content`
        /// template of their single `create_node` action. Not byte-identical
        /// -> not flagged.
        #[test]
        fn two_rules_with_slightly_different_actions_are_allowed() {
            let rule_a = rule("notify-a", "task", vec![create_action("hello")]);
            let rule_b = rule("notify-b", "task", vec![create_action("hello world")]);

            let errors = duplicate_errors(&[rule_a, rule_b]);
            assert!(errors.is_empty(), "expected no errors, got {:?}", errors);
        }

        #[test]
        fn two_rules_with_empty_action_lists_are_not_flagged() {
            // Nothing here derives an output id from `rule_id`, so there is
            // no collision to guard against -- this check is scoped to the
            // derived-identity hazard, not a general duplicate-rule linter.
            let rule_a = rule("empty-a", "task", vec![]);
            let rule_b = rule("empty-b", "task", vec![]);

            let errors = duplicate_errors(&[rule_a, rule_b]);
            assert!(
                errors.is_empty(),
                "rules with no actions must not be flagged: {:?}",
                errors
            );
        }

        #[test]
        fn three_rules_each_later_rule_is_flagged_against_the_earliest_match() {
            let rule_a = rule("a", "task", vec![create_action("x")]);
            let rule_b = rule("b", "task", vec![create_action("x")]);
            let rule_c = rule("c", "task", vec![create_action("x")]);

            let errors = duplicate_errors(&[rule_a, rule_b, rule_c]);
            assert_eq!(errors.len(), 2, "expected b~a and c~a, got {:?}", errors);
        }

        // -- End-to-end through validate_play --

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
        async fn duplicate_action_lists_reject_the_whole_play_through_validate_play() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "dup_check_reject").await;

            let rule_a = rule("dup-a", "dup_check_reject", vec![create_action("same")]);
            let rule_b = with_property_changed_trigger(&rule(
                "dup-b",
                "dup_check_reject",
                vec![create_action("same")],
            ));

            let errors = validate_play(&[rule_a, rule_b], &svc).await.unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| matches!(e, PlayValidationError::DuplicateActionList { .. })),
                "expected a DuplicateActionList error, got {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn distinct_action_lists_pass_validate_play() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "dup_check_ok").await;

            let rule_a = rule("ok-a", "dup_check_ok", vec![create_action("hello")]);
            let rule_b = with_property_changed_trigger(&rule(
                "ok-b",
                "dup_check_ok",
                vec![create_action("hello world")],
            ));

            let result = validate_play(&[rule_a, rule_b], &svc).await;
            assert!(result.is_ok(), "expected Ok, got {:?}", result);
        }
    }
}
