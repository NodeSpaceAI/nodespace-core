//! Play Engine Types
//!
//! Core data structures for the play engine: parsed play representation,
//! trigger keys for O(1) rule matching, and execution work items.
//!
//! These types are the engine's compiled form of a play (ADR-086 §1): built
//! from the typed wire rules ([`RuleDefinition`], defined once in
//! `nodespace-types`) by [`parse_rule`], and never serialized. They hold the
//! wire type's enums rather than re-declaring them.

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;

use crate::playbook::cel::CompiledCondition;

pub use nodespace_types::{
    Action, ActionType, GraphEventType, InlineSelector, PlayFields, RuleClass, RuleCondition,
    RuleDefinition, SavedQuerySelector, Selector, Trigger,
};

/// Maximum depth for play execution chains (ADR-060 §5).
///
/// A Play's actions are themselves graph writes, so they can trigger further
/// Plays. When `depth + 1 > MAX_CHAIN_DEPTH`, the engine stops processing the
/// work item and disables the offending play, which is what stops a cycle of
/// Plays triggering each other from running unbounded.
///
/// Enforced in two places: `PlaybookEngine`'s work-item loop, and
/// `persisted_chain_depth` (`db::events`), which clamps a depth read back off
/// a node's properties into `0..=MAX_CHAIN_DEPTH` rather than trusting it.
pub const MAX_CHAIN_DEPTH: u8 = 10;

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
    /// The play's compiled rules. Empty for a play whose rules do not parse.
    pub rules: Vec<Arc<ParsedRule>>,
    /// Whether the play runs, and if not, why.
    pub status: PlayStatus,
    /// The user's switch as last read from the node.
    pub enabled: bool,
    /// The node's stored `rules` as last read, to tell a rule edit from any
    /// other update.
    pub stored_rules: serde_json::Value,
}

/// Whether a play runs, and if not, why (ADR-087 §5).
///
/// A cache of the runnable check, rebuilt from the play node at load and on
/// every play update. Only a `Runnable` play has rules in the trigger index
/// and the cron registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayStatus {
    /// It participates, it is enabled, it is not suspended and its rules
    /// validate.
    Runnable,
    /// The node is archived, so it participates in nothing.
    Archived,
    /// The user switched it off (`enabled` is `false`).
    Disabled,
    /// The engine took it out of service on this device and recorded why on
    /// the node.
    Suspended,
}

impl PlayStatus {
    /// The status a play node's own state gives it. `Runnable` here still
    /// depends on the rules validating, which the engine checks next.
    ///
    /// This is the one place the engine decides whether a play runs: the
    /// participation check, then the user's switch, then the suspension.
    pub fn of_node(node: &crate::models::Node) -> Self {
        use crate::models::PlayFields;
        if !crate::governance::participates(node) {
            Self::Archived
        } else if !PlayFields::enabled_in(&node.properties) {
            Self::Disabled
        } else if PlayFields::suspended_in(&node.properties) {
            Self::Suspended
        } else {
            Self::Runnable
        }
    }
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

/// A rule's trigger, compiled.
#[derive(Debug, Clone)]
pub enum ParsedTrigger {
    GraphEvent {
        on: GraphEventType,
        /// The type the trigger's selector names. A graph event is matched
        /// against the type of the node the event is about, so its selector
        /// is always a bare type.
        node_type: String,
        /// Only present for `PropertyChanged`
        property_key: Option<String>,
    },
    Scheduled {
        cron: String,
        /// Which nodes the rule is evaluated against when the schedule comes
        /// due: an inline type and filters, or a saved query.
        select: Selector,
    },
}

impl ParsedTrigger {
    /// The type this trigger is registered on, when the rule itself names it:
    /// the type of a graph event's selector or of a scheduled trigger's inline
    /// selector.
    ///
    /// `None` for a scheduled trigger that selects through a saved query: the
    /// type is the query node's `target_type`, read when the play is validated
    /// and again each time the schedule fires (see
    /// [`crate::playbook::selectors::selector_query`]).
    pub fn registered_type(&self) -> Option<&str> {
        match self {
            Self::GraphEvent { node_type, .. } => Some(node_type),
            Self::Scheduled {
                select: Selector::Inline(inline),
                ..
            } => Some(&inline.target_type),
            Self::Scheduled {
                select: Selector::Query(_),
                ..
            } => None,
        }
    }
}

/// An action, compiled: its params as the JSON whose `{binding}` templates
/// are resolved each time the action runs.
#[derive(Debug, Clone)]
pub struct ParsedAction {
    pub action_type: ActionType,
    pub params: serde_json::Value,
    /// Optional iteration over a collection
    pub for_each: Option<String>,
}

/// Whether an action performs only a local graph write.
///
/// ADR-060 §2 requires an invariant rule's actions to be **local writes
/// only**: a transaction cannot await an LLM call, a network request, a PTY,
/// or any external service, and holding a write transaction across an
/// unbounded wait is a correctness and liveness hazard.
///
/// Every graph-mutation action is a pure local graph mutation, so this
/// returns `true` for all of them. `Reject` also returns `true`: it performs
/// no I/O of any kind — deterministically returning an error is pure
/// computation — so it can never violate the local-writes-only guarantee this
/// check exists to enforce. It is written as an exhaustive `match` rather
/// than a blanket `true` deliberately: adding a non-local action type later
/// (LLM/network/PTY/external) will fail to compile until it is classified
/// here, so such an action can never silently become eligible for an
/// invariant rule.
pub fn is_local_write(action_type: ActionType) -> bool {
    match action_type {
        ActionType::CreateNode
        | ActionType::UpdateNode
        | ActionType::AddRelationship
        | ActionType::RemoveRelationship
        | ActionType::Reject => true,
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
    /// What a scheduled scan worked out for every node it selected. `None`
    /// for an event-triggered work item.
    pub scan: Option<Arc<ScanContext>>,
}

/// What a scheduled scan resolves once, for every node it selected, before
/// enqueuing a work item per node.
#[derive(Debug, Default)]
pub struct ScanContext {
    /// The type the scan's selector selects: the scope its rules read each
    /// node at (ADR-078). For a saved-query selector this is the query's
    /// `target_type` as it stood when the schedule fired.
    pub target_type: String,
    /// The rules' condition paths, resolved for all the scanned nodes at
    /// once and keyed by the node each was resolved from.
    pub paths: crate::playbook::graph_resolver::PathCache,
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
    /// Which nodes the entry's rules are evaluated against. Rules sharing a
    /// cron expression and a selector share one entry, and so one query.
    pub select: Selector,
    pub rules: Vec<OrderedRuleRef>,
}

/// Registry of cron expressions for scheduled trigger evaluation.
pub type CronRegistry = Vec<CronEntry>;

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Errors that can occur when compiling a play's rules.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayParseError {
    /// The stored `rules` do not decode as typed rules: an unknown trigger
    /// type, event or action, a missing param, or a param the action does not
    /// take. The message names the rule and the field.
    InvalidRules(String),
    /// A trigger's selector is one its trigger type cannot use.
    UnsupportedSelector(String),
    InvalidCondition(crate::playbook::cel::CelCompileError),
}

impl std::fmt::Display for PlayParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRules(t) => write!(f, "{}", t),
            Self::UnsupportedSelector(t) => write!(f, "unsupported selector: {}", t),
            Self::InvalidCondition(e) => write!(f, "{}", e),
        }
    }
}

/// Compile a typed [`RuleDefinition`] into a `ParsedRule`.
///
/// CEL conditions are compiled here, once, and the resulting `Program`s are
/// cached on the rule for reuse across every future evaluation.
pub fn parse_rule(def: &RuleDefinition) -> Result<ParsedRule, PlayParseError> {
    let trigger = parse_trigger(&def.trigger)?;
    let actions = def.actions.iter().map(parse_action).collect();
    let conditions = def
        .conditions
        .iter()
        .map(|condition| {
            CompiledCondition::compile(&condition.expr).map_err(PlayParseError::InvalidCondition)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ParsedRule {
        name: def.name.clone(),
        class: def.class,
        trigger,
        conditions,
        actions,
    })
}

fn parse_trigger(trigger: &Trigger) -> Result<ParsedTrigger, PlayParseError> {
    match trigger {
        Trigger::GraphEvent {
            on,
            select,
            property_key,
        } => {
            // An event is matched in memory against the type of the node it
            // is about, before any query could run, and an invariant rule is
            // matched inside the triggering transaction. Filters and saved
            // queries select by running a query, which is what a scheduled
            // scan does; here the conditions are where a rule narrows.
            let node_type = match select {
                Selector::Inline(inline) if inline.filters.is_empty() => inline.target_type.clone(),
                Selector::Inline(_) => {
                    return Err(PlayParseError::UnsupportedSelector(
                        "a graph_event trigger selects by type only; move the selector's \
                         filters into the rule's conditions, or use a scheduled trigger"
                            .to_string(),
                    ));
                }
                Selector::Query(_) => {
                    return Err(PlayParseError::UnsupportedSelector(
                        "a graph_event trigger selects by type only; a saved query can \
                         select for a scheduled trigger"
                            .to_string(),
                    ));
                }
            };
            Ok(ParsedTrigger::GraphEvent {
                on: *on,
                node_type,
                property_key: property_key.clone(),
            })
        }
        Trigger::Scheduled { cron, select } => Ok(ParsedTrigger::Scheduled {
            cron: cron.clone(),
            select: select.clone(),
        }),
    }
}

/// Compile a typed [`Action`]. Its params become the JSON the executor
/// resolves `{binding}` templates in.
pub fn parse_action(action: &Action) -> ParsedAction {
    ParsedAction {
        action_type: action.action_type(),
        params: action.params_value(),
        for_each: action.for_each().map(str::to_string),
    }
}

/// Decode the typed rules from a play node's stored properties.
///
/// A play's declared fields are stored in its type bucket, so the rules are
/// at `properties["play"]["rules"]`. [`PlayFields`] is the one reader of
/// them; a play with no `rules` has none.
pub fn parse_rules_from_properties(
    properties: &serde_json::Value,
) -> Result<Vec<RuleDefinition>, PlayParseError> {
    PlayFields::from_properties(properties)
        .map(|fields| fields.rules)
        .map_err(|e| PlayParseError::InvalidRules(e.to_string()))
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
        let action: Action = serde_json::from_value(serde_json::json!({
            "description": "Test action",
            "action_type": "reject",
            "params": { "message": "no" }
        }))
        .unwrap();
        assert_eq!(parse_action(&action).action_type, ActionType::Reject);
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
        assert!(is_local_write(ActionType::Reject));
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
                    select: Selector::of_type("task"),
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
