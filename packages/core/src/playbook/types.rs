//! Play Engine Types
//!
//! Core data structures for the play engine: parsed play representation,
//! trigger keys for O(1) rule matching, and execution work items.
//!
//! These types are the in-memory representation used by the engine at runtime.
//! They are parsed from the JSON properties stored on play nodes.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::playbook::cel::CompiledCondition;

// ---------------------------------------------------------------------------
// Trigger types
// ---------------------------------------------------------------------------

/// Event types for node-level triggers.
///
/// Maps to the `on` field in a `graph_event` trigger definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeEventType {
    /// Fires when a node of the specified type is created
    NodeCreated,
    /// Fires when a property on a matching node changes
    PropertyChanged,
}

/// Event types for relationship-level triggers.
///
/// Maps to the `on` field in a `graph_event` trigger definition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RelEventType {
    /// Fires when a relationship is added from a matching source node
    RelationshipAdded,
    /// Fires when a relationship is removed from a matching source node
    RelationshipRemoved,
}

/// Key for O(1) rule lookup in the TriggerIndex.
///
/// An enum — not a flat tuple — because relationship triggers have no
/// `property_key` dimension. The `RelationshipEvent` variant intentionally
/// omits `relationship_type` in v1.
///
/// `PropertyChanged` triggers require dual lookup: exact `property_key` match
/// AND wildcard (`None`) match, with results merged maintaining sort order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TriggerKey {
    NodeEvent {
        event: NodeEventType,
        node_type: String,
        /// Required for PropertyChanged only. `None` = wildcard (matches all property changes).
        property_key: Option<String>,
    },
    RelationshipEvent {
        event: RelEventType,
        source_node_type: String,
    },
}

/// Build a type-namespaced `property_changed` key (`<node_type>.<field>`).
///
/// This is the only spelling `trigger_keys_for_graph_event` ever indexes a
/// specific-property `PropertyChanged` trigger under, and the only spelling
/// `validate_play`'s `UnnamespacedPropertyChangedKey` check accepts — so
/// every playbook-domain site that builds or rebuilds one of these keys
/// (registering a trigger, validating one, re-namespacing one for ancestor
/// fan-out, or enumerating candidate keys for a diagnostic) must produce
/// exactly this shape or it can never match a real event.
///
/// Distinct from the near-identical `<node_type>.<field>` discriminator
/// `node_service::conflicts` builds for unique-field-collision ids — that is
/// a different domain (conflict identity, not trigger matching) and
/// deliberately not unified with this one.
pub fn namespaced_property_key(node_type: &str, field: &str) -> String {
    format!("{node_type}.{field}")
}

// ---------------------------------------------------------------------------
// Parsed play representation (in-memory)
// ---------------------------------------------------------------------------

/// A rule reference with ordering information for deterministic execution.
///
/// Rules are sorted by `(play_id, rule_index)` — cross-play by the play's
/// stable, content-derived id (ADR-060 §5), within-play by array index.
/// Ordering on `play_id` rather than the play's wall-clock `created_at` gives
/// every device the same evaluation order regardless of clock skew or the
/// order in which plays were installed/synced.
#[derive(Debug, Clone)]
pub struct OrderedRuleRef {
    pub play_id: String,
    pub rule_index: usize,
    pub rule: Arc<ParsedRule>,
}

/// Equality is by identity (play + rule index), not by ordering fields.
/// This allows `sort() + dedup()` to work correctly in `lookup_rules()`:
/// same-identity refs always share the same ordering key, so sort groups them adjacently.
impl PartialEq for OrderedRuleRef {
    fn eq(&self, other: &Self) -> bool {
        self.play_id == other.play_id && self.rule_index == other.rule_index
    }
}

impl Eq for OrderedRuleRef {}

impl PartialOrd for OrderedRuleRef {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedRuleRef {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.play_id
            .cmp(&other.play_id)
            .then_with(|| self.rule_index.cmp(&other.rule_index))
    }
}

/// A parsed play — the in-memory representation of a play node's rules.
#[derive(Debug, Clone)]
pub struct ParsedPlay {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub rules: Vec<Arc<ParsedRule>>,
    /// Lifecycle status: "active" or "disabled"
    pub status: PlayStatus,
}

/// Play lifecycle status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayStatus {
    Active,
    Disabled,
}

/// Execution class of a play rule (ADR-060).
///
/// - `Reactive` is today's behavior: the rule runs asynchronously, post-commit,
///   on every device that observes the triggering event.
/// - `Invariant` rules run synchronously inside the creating transaction,
///   fail-closed, on the originating device only. They are subject to the
///   save-time eligibility checks in [`crate::playbook::validation`] (ADR-060 §2).
///
/// The class is a property of the rule, declared in the play and validated at
/// save time. It defaults to `Reactive` so every rule authored before this class
/// existed keeps its current semantics unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleClass {
    /// Synchronous, in-transaction, origin-device-only, fail-closed.
    Invariant,
    /// Asynchronous, post-commit, every device. The default.
    #[default]
    Reactive,
}

/// A single parsed rule from a play's `rules` array.
#[derive(Debug, Clone)]
pub struct ParsedRule {
    pub name: String,
    /// Execution class (ADR-060). Defaults to `Reactive`.
    pub class: RuleClass,
    pub trigger: ParsedTrigger,
    pub conditions: Vec<CompiledCondition>,
    pub actions: Vec<ParsedAction>,
}

/// Parsed trigger definition — either a graph event or a scheduled cron.
#[derive(Debug, Clone)]
pub enum ParsedTrigger {
    GraphEvent {
        on: GraphEventType,
        node_type: String,
        /// Only present for `PropertyChanged`
        property_key: Option<String>,
    },
    Scheduled {
        cron: String,
        node_type: String,
    },
}

/// Graph event types as stored in the play JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphEventType {
    NodeCreated,
    PropertyChanged,
    RelationshipAdded,
    RelationshipRemoved,
}

/// A parsed action from a rule's `actions` array.
#[derive(Debug, Clone)]
pub struct ParsedAction {
    pub action_type: ActionType,
    pub params: serde_json::Value,
    /// Optional iteration over a collection
    pub for_each: Option<String>,
}

/// Action types supported by the engine (v1 — graph operations only, plus
/// `Reject` (ADR-060 §2) — the one non-graph-mutating action, whose entire
/// effect is vetoing the triggering write rather than augmenting it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionType {
    CreateNode,
    UpdateNode,
    AddRelationship,
    RemoveRelationship,
    /// Deterministically fails the enclosing transaction with an
    /// author-supplied message (`params.message`), instead of writing
    /// anything. Meaningful only on a `RuleClass::Invariant` rule — there is
    /// no transaction left to fail once a rule's actions run asynchronously,
    /// post-commit (`Reactive`, ADR-060's default class), so save-time
    /// validation (`playbook::validation::validate_reject_action_class`)
    /// rejects a `Reject` action declared on a `Reactive` rule. See
    /// `playbook::actions::execute_reject` for the execution-time contract.
    Reject,
}

impl ActionType {
    /// The canonical JSON name for this action type (the value parsed from a
    /// rule's `action_type` field).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CreateNode => "create_node",
            Self::UpdateNode => "update_node",
            Self::AddRelationship => "add_relationship",
            Self::RemoveRelationship => "remove_relationship",
            Self::Reject => "reject",
        }
    }

    /// Whether this action performs only a local graph write.
    ///
    /// ADR-060 §2 requires an invariant rule's actions to be **local writes
    /// only**: a transaction cannot await an LLM call, a network request, a PTY,
    /// or any external service, and holding a write transaction across an
    /// unbounded wait is a correctness and liveness hazard.
    ///
    /// Every v1 graph-mutation action type is a pure local graph mutation, so
    /// this returns `true` for all of them. `Reject` also returns `true`: it
    /// performs no I/O of any kind — deterministically returning an error is
    /// pure computation — so it can never violate the local-writes-only
    /// guarantee this check exists to enforce. It is written as an exhaustive
    /// `match` rather than a blanket `true` deliberately: adding a non-local
    /// action type later (LLM/network/PTY/external) will fail to compile
    /// until it is classified here, so such an action can never silently
    /// become eligible for an invariant rule.
    pub fn is_local_write(&self) -> bool {
        match self {
            Self::CreateNode
            | Self::UpdateNode
            | Self::AddRelationship
            | Self::RemoveRelationship
            | Self::Reject => true,
        }
    }
}

// ---------------------------------------------------------------------------
// Derived identity path (ADR-060 §3, ADR-074)
// ---------------------------------------------------------------------------

/// Ordered sequence of real node ids identifying which execution of a
/// playbook action this is, for
/// [`crate::playbook::actions::deterministic_action_output_id`].
///
/// Never a positional/loop index and never sorted -- order encodes nesting
/// depth (the outer trigger/scanned node first, then each nested `for_each`
/// item's own id, in nesting order), and every element must be a real,
/// already-existing node id, so two devices that scan or iterate the same
/// set in different orders still agree on which item produced which id:
/// - `graph_event` trigger, no `for_each`: `[trigger_node_id]`
/// - `scheduled` trigger scanning nodes, no `for_each`: `[scanned_node_id]`
///   (the engine hands the scanned node to the action executor as the
///   "trigger node" for a scheduled work item too, so this is the same slot)
/// - nested `for_each`: `[scanned_node_id, item_node_id]`, generalizing to
///   further nesting depth by appending one more real id per level entered
pub type IterationPath = Vec<String>;

// ---------------------------------------------------------------------------
// ExecutionWorkItem
// ---------------------------------------------------------------------------

/// Work item for the RuleProcessor queue.
///
/// Carries everything the processor needs to evaluate and execute matched rules:
/// the sorted rules, the original event envelope (for playbook_context/depth),
/// and the pre-fetched trigger node in wire format.
///
/// Created by the EventSubscriber (for event-triggered rules) and the CronRunner
/// (for scheduled rules). Consumed by the single RuleProcessor tokio task.
#[derive(Debug)]
pub struct ExecutionWorkItem {
    /// Matched rules to evaluate, sorted by (play_id, rule_index)
    pub rules: Vec<OrderedRuleRef>,
    /// Original event envelope (carries playbook_context for cycle detection depth)
    pub trigger_event: crate::db::events::EventEnvelope,
    /// Pre-fetched node that fired the trigger (wire-format)
    pub trigger_node: crate::models::Node,
}

// ---------------------------------------------------------------------------
// TriggerIndex
// ---------------------------------------------------------------------------

/// The trigger index: HashMap from TriggerKey → sorted Vec of rule references.
///
/// This is a plain type alias. `PlaybookEngine` wraps it in `Arc<RwLock<PlaybookLifecycleManager>>`
/// for concurrent read (event subscriber) / write (lifecycle ops) access.
pub type TriggerIndex = HashMap<TriggerKey, Vec<OrderedRuleRef>>;

// ---------------------------------------------------------------------------
// Cron registry
// ---------------------------------------------------------------------------

/// Entry in the cron registry for scheduled triggers.
#[derive(Debug, Clone)]
pub struct CronEntry {
    pub cron_expression: String,
    pub node_type: String,
    pub rules: Vec<OrderedRuleRef>,
}

/// Registry of cron expressions for scheduled trigger evaluation.
pub type CronRegistry = Vec<CronEntry>;

// ---------------------------------------------------------------------------
// JSON deserialization types (from play node properties)
// ---------------------------------------------------------------------------

/// Raw rule definition as stored in the play node's `properties.rules` JSON array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDefinition {
    pub name: String,
    /// Execution class (ADR-060). Omitted in JSON → `Reactive`, so every
    /// existing rule definition keeps today's async, post-commit semantics.
    #[serde(default)]
    pub class: RuleClass,
    pub trigger: TriggerDefinition,
    #[serde(default)]
    pub conditions: Vec<String>,
    #[serde(default)]
    pub actions: Vec<ActionDefinition>,
}

/// Raw trigger definition from JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerDefinition {
    #[serde(rename = "type")]
    pub trigger_type: String,
    /// Event name for graph_event triggers
    #[serde(default)]
    pub on: Option<String>,
    /// Node type to match
    #[serde(default)]
    pub node_type: Option<String>,
    /// Property key for property_changed triggers
    #[serde(default)]
    pub property_key: Option<String>,
    /// Cron expression for scheduled triggers
    #[serde(default)]
    pub cron: Option<String>,
}

/// Raw action definition from JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDefinition {
    pub action_type: String,
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub for_each: Option<String>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Errors that can occur when parsing a play's rule definitions.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayParseError {
    InvalidTriggerType(String),
    InvalidEventType(String),
    InvalidActionType(String),
    MissingField(String),
    InvalidJson(String),
    InvalidCondition(crate::playbook::cel::CelCompileError),
}

impl std::fmt::Display for PlayParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTriggerType(t) => write!(f, "invalid trigger type: {}", t),
            Self::InvalidEventType(t) => write!(f, "invalid event type: {}", t),
            Self::InvalidActionType(t) => write!(f, "invalid action type: {}", t),
            Self::MissingField(t) => write!(f, "missing required field: {}", t),
            Self::InvalidJson(t) => write!(f, "invalid JSON: {}", t),
            Self::InvalidCondition(e) => write!(f, "{}", e),
        }
    }
}

/// Parse a `RuleDefinition` (from JSON) into a `ParsedRule`.
///
/// CEL conditions are compiled here, once, and the resulting `Program`s are
/// cached on the rule for reuse across every future evaluation.
pub fn parse_rule(def: &RuleDefinition) -> Result<ParsedRule, PlayParseError> {
    let trigger = parse_trigger(&def.trigger)?;
    let actions = def
        .actions
        .iter()
        .map(parse_action)
        .collect::<Result<Vec<_>, _>>()?;
    let conditions = def
        .conditions
        .iter()
        .map(|expr| CompiledCondition::compile(expr).map_err(PlayParseError::InvalidCondition))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ParsedRule {
        name: def.name.clone(),
        class: def.class,
        trigger,
        conditions,
        actions,
    })
}

fn parse_trigger(def: &TriggerDefinition) -> Result<ParsedTrigger, PlayParseError> {
    match def.trigger_type.as_str() {
        "graph_event" => {
            let on_str = def
                .on
                .as_deref()
                .ok_or_else(|| PlayParseError::MissingField("on".to_string()))?;
            let node_type = def
                .node_type
                .clone()
                .ok_or_else(|| PlayParseError::MissingField("node_type".to_string()))?;

            let on = match on_str {
                "node_created" => GraphEventType::NodeCreated,
                "property_changed" => GraphEventType::PropertyChanged,
                "relationship_added" => GraphEventType::RelationshipAdded,
                "relationship_removed" => GraphEventType::RelationshipRemoved,
                other => {
                    return Err(PlayParseError::InvalidEventType(other.to_string()));
                }
            };

            Ok(ParsedTrigger::GraphEvent {
                on,
                node_type,
                property_key: def.property_key.clone(),
            })
        }
        "scheduled" => {
            let cron = def
                .cron
                .clone()
                .ok_or_else(|| PlayParseError::MissingField("cron".to_string()))?;
            let node_type = def
                .node_type
                .clone()
                .ok_or_else(|| PlayParseError::MissingField("node_type".to_string()))?;

            Ok(ParsedTrigger::Scheduled { cron, node_type })
        }
        other => Err(PlayParseError::InvalidTriggerType(other.to_string())),
    }
}

pub fn parse_action(def: &ActionDefinition) -> Result<ParsedAction, PlayParseError> {
    let action_type = match def.action_type.as_str() {
        "create_node" => ActionType::CreateNode,
        "update_node" => ActionType::UpdateNode,
        "add_relationship" => ActionType::AddRelationship,
        "remove_relationship" => ActionType::RemoveRelationship,
        "reject" => ActionType::Reject,
        other => {
            return Err(PlayParseError::InvalidActionType(other.to_string()));
        }
    };

    Ok(ParsedAction {
        action_type,
        params: def.params.clone(),
        for_each: def.for_each.clone(),
    })
}

/// Parse the `rules` array from a play node's properties JSON.
///
/// Checks both top-level `properties["rules"]` and namespace-nested
/// `properties["play"]["rules"]` to support both in-memory and
/// DB-stored (namespace-normalized) formats.
pub fn parse_rules_from_properties(
    properties: &serde_json::Value,
) -> Result<Vec<RuleDefinition>, PlayParseError> {
    // Try top-level first: {"rules": [...]}
    let rules_value = properties
        .get("rules")
        // Then try inside the "play" namespace: {"play": {"rules": [...]}}
        .or_else(|| properties.get("play").and_then(|pb| pb.get("rules")))
        .ok_or_else(|| PlayParseError::MissingField("rules".to_string()))?;

    serde_json::from_value(rules_value.clone())
        .map_err(|e| PlayParseError::InvalidJson(e.to_string()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ActionType::Reject (ADR-060 §2)
    // -----------------------------------------------------------------------

    #[test]
    fn reject_parses_from_json_action_type() {
        let def = ActionDefinition {
            action_type: "reject".to_string(),
            params: serde_json::json!({ "message": "no" }),
            for_each: None,
        };
        assert_eq!(parse_action(&def).unwrap().action_type, ActionType::Reject);
    }

    #[test]
    fn reject_as_str_round_trips() {
        assert_eq!(ActionType::Reject.as_str(), "reject");
    }

    #[test]
    fn reject_is_a_local_write() {
        // No I/O of any kind — deterministically failing is pure
        // computation — so it must never be excluded from an invariant
        // rule's action list on locality grounds.
        assert!(ActionType::Reject.is_local_write());
    }

    /// Helper: a minimal `OrderedRuleRef` for a given play id / rule index.
    /// The rule content is irrelevant to ordering/identity, so every ref
    /// shares one trivial `ParsedRule`.
    fn make_ref(play_id: &str, rule_index: usize) -> OrderedRuleRef {
        OrderedRuleRef {
            play_id: play_id.to_string(),
            rule_index,
            rule: Arc::new(ParsedRule {
                name: format!("{play_id}-{rule_index}"),
                class: RuleClass::Reactive,
                trigger: ParsedTrigger::GraphEvent {
                    on: GraphEventType::NodeCreated,
                    node_type: "task".to_string(),
                    property_key: None,
                },
                conditions: vec![],
                actions: vec![],
            }),
        }
    }

    // -----------------------------------------------------------------------
    // Ord — play_id is the primary, stable key (ADR-060 §5)
    // -----------------------------------------------------------------------

    #[test]
    fn ord_orders_cross_play_by_play_id_not_rule_index() {
        // "apple" < "zebra" lexically. Give the lexically-earlier play the
        // *larger* rule_index to prove play_id — not rule_index — is the
        // primary key: if rule_index leaked into the primary comparison,
        // this would sort the other way.
        let apple = make_ref("apple", 5);
        let zebra = make_ref("zebra", 0);

        assert!(apple < zebra, "play_id must be the primary sort key");

        let mut rules = [zebra.clone(), apple.clone()];
        rules.sort();
        assert_eq!(
            rules.iter().map(|r| r.play_id.as_str()).collect::<Vec<_>>(),
            vec!["apple", "zebra"]
        );
    }

    #[test]
    fn ord_orders_within_play_by_rule_index() {
        // Existing single-play behavior must be unchanged: same play_id,
        // ties broken by rule_index ascending.
        let r0 = make_ref("pb-1", 0);
        let r1 = make_ref("pb-1", 1);
        let r2 = make_ref("pb-1", 2);

        let mut rules = [r2.clone(), r0.clone(), r1.clone()];
        rules.sort();
        assert_eq!(
            rules.iter().map(|r| r.rule_index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    // -----------------------------------------------------------------------
    // PartialEq/Eq — identity is (play_id, rule_index), independent of Ord
    // -----------------------------------------------------------------------

    #[test]
    fn eq_is_identity_only_ignoring_rule_content() {
        let a = make_ref("pb-1", 0);
        // Same identity (play_id, rule_index) but a distinct `Arc<ParsedRule>`
        // with different content — equality must still hold, since dedup
        // relies on identity, not rule content.
        let b = OrderedRuleRef {
            play_id: "pb-1".to_string(),
            rule_index: 0,
            rule: Arc::new(ParsedRule {
                name: "totally-different-rule".to_string(),
                class: RuleClass::Invariant,
                trigger: ParsedTrigger::Scheduled {
                    cron: "0 * * * * * *".to_string(),
                    node_type: "task".to_string(),
                },
                conditions: vec![],
                actions: vec![],
            }),
        };

        assert_eq!(a, b);

        let c = make_ref("pb-1", 1);
        let d = make_ref("pb-2", 0);
        assert_ne!(a, c, "different rule_index must not be equal");
        assert_ne!(a, d, "different play_id must not be equal");
    }

    #[test]
    fn sort_dedup_collapses_duplicate_identity_regardless_of_order() {
        // `lookup_rules` merges matches from multiple TriggerKeys and relies
        // on `.sort().dedup()` to collapse duplicate (play_id, rule_index)
        // entries — e.g. the same rule matched via both an exact and a
        // wildcard property-key lookup. Verify that behavior still holds
        // with the play_id-keyed Ord: duplicates collapse regardless of
        // insertion order, and distinct identities survive.
        let mut rules = vec![
            make_ref("zebra", 0),
            make_ref("apple", 1),
            make_ref("apple", 0),
            make_ref("apple", 0), // duplicate of the entry above
            make_ref("zebra", 0), // duplicate
        ];
        rules.sort();
        rules.dedup();

        assert_eq!(rules.len(), 3, "duplicates must collapse to one each");
        let identities: Vec<(String, usize)> = rules
            .iter()
            .map(|r| (r.play_id.clone(), r.rule_index))
            .collect();
        assert_eq!(
            identities,
            vec![
                ("apple".to_string(), 0),
                ("apple".to_string(), 1),
                ("zebra".to_string(), 0),
            ]
        );
    }
}
