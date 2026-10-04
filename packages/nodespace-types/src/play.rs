//! The `play` node and its rules (ADR-086 §4 "Structured", §11).
//!
//! A play's `rules` are typed end to end: a tagged [`Trigger`], a tagged
//! [`Action`] with one params struct per action, and open JSON only where a
//! user's schema decides the shape (`properties`, `edge_data`). Every type a
//! user or agent authors rejects unknown fields, so a rule naming a parameter
//! the engine never reads fails to decode instead of saving and doing
//! nothing.
//!
//! Rule keys are snake_case in storage and on the wire alike: a rule is
//! authored and replaced whole, so it has one spelling.
//!
//! A rule, each of its conditions and each of its actions carry a required
//! `description`: what that part means, written by the play's author in the
//! same write (ADR-090 §1). The trigger has none; its fields describe it.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::helpers::deserialize_clearable;
use crate::node::{Node, NodeEnvelope, ValidationError};
use crate::query::QueryFilter;

/// The `node_type` of every play.
pub const PLAY_NODE_TYPE: &str = "play";

// ============================================================================
// Rule vocabulary
// ============================================================================

/// How a rule executes (ADR-060).
///
/// - `Reactive` rules run asynchronously, after the triggering write commits,
///   on every device that observes the event.
/// - `Invariant` rules run synchronously inside the triggering transaction,
///   fail-closed, on the originating device only. Save-time validation holds
///   them to the eligibility rules of ADR-060 §2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum RuleClass {
    Invariant,
    #[default]
    Reactive,
}

/// The graph change a `graph_event` trigger fires on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum GraphEventType {
    /// A node the selector matches was created.
    NodeCreated,
    /// A property changed on a node the selector matches.
    PropertyChanged,
    /// A relationship was added whose source node the selector matches.
    RelationshipAdded,
    /// A relationship was removed whose source node the selector matches.
    RelationshipRemoved,
}

impl GraphEventType {
    /// The name a rule spells this event with.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NodeCreated => "node_created",
            Self::PropertyChanged => "property_changed",
            Self::RelationshipAdded => "relationship_added",
            Self::RelationshipRemoved => "relationship_removed",
        }
    }
}

/// What an [`Action`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    CreateNode,
    UpdateNode,
    AddRelationship,
    RemoveRelationship,
    /// Fails the triggering write with the author's message instead of
    /// writing anything. Invariant rules only (ADR-060 §2).
    Reject,
}

impl ActionType {
    /// The name a rule spells this action with.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateNode => "create_node",
            Self::UpdateNode => "update_node",
            Self::AddRelationship => "add_relationship",
            Self::RemoveRelationship => "remove_relationship",
            Self::Reject => "reject",
        }
    }
}

// ============================================================================
// Selectors
// ============================================================================

/// A selector written out in the rule: the type and filters a query node
/// stores to say which nodes it selects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct InlineSelector {
    /// The node type selected, subtypes included, or `*` for every type.
    pub target_type: String,
    /// Filters every selected node satisfies, as a query node stores them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub filters: Vec<QueryFilter>,
}

/// A selector that names a saved `query` node. The set of nodes is whatever
/// that query's type and filters select when the trigger fires; its sorting,
/// limit and view belong to the viewer and play no part.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SavedQuerySelector {
    pub query_id: String,
}

/// Which nodes a trigger applies to, said the way a query says it
/// (ADR-086 §11): inline, or by reference to a saved query node.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(untagged)]
pub enum Selector {
    Query(SavedQuerySelector),
    Inline(InlineSelector),
}

impl Selector {
    /// An inline selector for every node of `target_type`.
    pub fn of_type(target_type: impl Into<String>) -> Self {
        Self::Inline(InlineSelector {
            target_type: target_type.into(),
            filters: Vec::new(),
        })
    }

    /// A selector that reuses the saved query `query_id`.
    pub fn saved_query(query_id: impl Into<String>) -> Self {
        Self::Query(SavedQuerySelector {
            query_id: query_id.into(),
        })
    }
}

impl std::fmt::Display for Selector {
    /// A short description for logs: `task`, `task (2 filters)`, `query q-1`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Query(saved) => write!(f, "query {}", saved.query_id),
            Self::Inline(inline) if inline.filters.is_empty() => f.write_str(&inline.target_type),
            Self::Inline(inline) => {
                write!(
                    f,
                    "{} ({} filters)",
                    inline.target_type,
                    inline.filters.len()
                )
            }
        }
    }
}

impl<'de> Deserialize<'de> for Selector {
    /// Decoded by hand rather than as an untagged enum, whose only error is
    /// "data did not match any variant": a selector names a `query_id` or a
    /// `target_type`, and a mistake in either is reported as that variant's
    /// own error.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let object = value.as_object().ok_or_else(|| {
            serde::de::Error::custom(
                "a selector is an object: { \"target_type\": ... } or { \"query_id\": ... }",
            )
        })?;
        if object.contains_key("query_id") {
            serde_json::from_value(value)
                .map(Self::Query)
                .map_err(|e| serde::de::Error::custom(format!("saved-query selector: {e}")))
        } else {
            serde_json::from_value(value)
                .map(Self::Inline)
                .map_err(|e| serde::de::Error::custom(format!("selector: {e}")))
        }
    }
}

// ============================================================================
// Triggers
// ============================================================================

/// What makes a rule run. Tagged on `type`; a field that does not belong to
/// the chosen variant is a decode error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// A node or relationship changed.
    GraphEvent {
        on: GraphEventType,
        select: Selector,
        /// `property_changed` only: the one property to watch, namespaced as
        /// `<type>.<field>`. Omitted, any property change fires the rule.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        property_key: Option<String>,
    },
    /// A cron schedule came due. The rule is evaluated against every node the
    /// selector matches.
    Scheduled { cron: String, select: Selector },
}

impl Trigger {
    /// The selector saying which nodes this trigger applies to.
    pub fn selector(&self) -> &Selector {
        match self {
            Self::GraphEvent { select, .. } | Self::Scheduled { select, .. } => select,
        }
    }
}

// ============================================================================
// Actions
// ============================================================================

/// Params of a `create_node` action. Every string may be a `{binding}`
/// template; `properties` is open because the created type's schema decides
/// its keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct CreateNodeParams {
    pub node_type: String,
    /// The schema version of `node_type` the rule was written against. When
    /// given, it must match the installed schema's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "Record<string, unknown>"))]
    pub properties: Option<Map<String, Value>>,
}

/// Params of an `update_node` action. A play never writes `lifecycle_status`:
/// that is governance, not automation state (ADR-087).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct UpdateNodeParams {
    pub node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "Record<string, unknown>"))]
    pub properties: Option<Map<String, Value>>,
    /// Retype the node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
}

/// Params of an `add_relationship` action. `edge_data` is open because the
/// relationship's declared edge fields decide its keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(deny_unknown_fields)]
pub struct AddRelationshipParams {
    pub source_id: String,
    pub relationship_type: String,
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "Record<string, unknown>"))]
    pub edge_data: Option<Map<String, Value>>,
}

/// Params of a `remove_relationship` action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RemoveRelationshipParams {
    pub source_id: String,
    pub relationship_type: String,
    pub target_id: String,
}

/// Params of a `reject` action: the message the refused write fails with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RejectParams {
    pub message: String,
}

/// One step of a rule. Tagged on `action_type`, with that action's own
/// `params` and the author's `description` of what the step does.
///
/// `for_each` runs the action once per node a path reaches
/// (`trigger.node.tasks`, optionally narrowed with `.where(...)`), binding
/// each as `item`. A `reject` has nothing to iterate, so it takes none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(tag = "action_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    CreateNode {
        description: String,
        params: CreateNodeParams,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        for_each: Option<String>,
    },
    UpdateNode {
        description: String,
        params: UpdateNodeParams,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        for_each: Option<String>,
    },
    AddRelationship {
        description: String,
        params: AddRelationshipParams,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        for_each: Option<String>,
    },
    RemoveRelationship {
        description: String,
        params: RemoveRelationshipParams,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        for_each: Option<String>,
    },
    Reject {
        description: String,
        params: RejectParams,
    },
}

impl Action {
    pub fn action_type(&self) -> ActionType {
        match self {
            Self::CreateNode { .. } => ActionType::CreateNode,
            Self::UpdateNode { .. } => ActionType::UpdateNode,
            Self::AddRelationship { .. } => ActionType::AddRelationship,
            Self::RemoveRelationship { .. } => ActionType::RemoveRelationship,
            Self::Reject { .. } => ActionType::Reject,
        }
    }

    /// What the step does, in its author's words.
    pub fn description(&self) -> &str {
        match self {
            Self::CreateNode { description, .. }
            | Self::UpdateNode { description, .. }
            | Self::AddRelationship { description, .. }
            | Self::RemoveRelationship { description, .. }
            | Self::Reject { description, .. } => description,
        }
    }

    /// The path this action iterates, if any.
    pub fn for_each(&self) -> Option<&str> {
        match self {
            Self::CreateNode { for_each, .. }
            | Self::UpdateNode { for_each, .. }
            | Self::AddRelationship { for_each, .. }
            | Self::RemoveRelationship { for_each, .. } => for_each.as_deref(),
            Self::Reject { .. } => None,
        }
    }

    /// This action's params as the JSON the engine resolves `{binding}`
    /// templates in.
    pub fn params_value(&self) -> Value {
        match self {
            Self::CreateNode { params, .. } => to_json(params),
            Self::UpdateNode { params, .. } => to_json(params),
            Self::AddRelationship { params, .. } => to_json(params),
            Self::RemoveRelationship { params, .. } => to_json(params),
            Self::Reject { params, .. } => to_json(params),
        }
    }
}

// ============================================================================
// Rules
// ============================================================================

/// One condition of a rule: a CEL expression over the triggering node, and
/// what it requires in the author's words. A bare expression does not decode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(
    deny_unknown_fields,
    expecting = "an object { \"expr\": \"<CEL>\", \"description\": \"<what must hold>\" }"
)]
pub struct RuleCondition {
    pub expr: String,
    pub description: String,
}

/// One rule of a play: when it runs, what must hold, and what it does.
///
/// Every condition must pass. `description` says what the rule does, in one
/// sentence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuleDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub class: RuleClass,
    pub trigger: Trigger,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub conditions: Vec<RuleCondition>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub actions: Vec<Action>,
}

// ============================================================================
// PlayFields: the play schema's fields
// ============================================================================

/// The storage key of a play's `rules`.
pub const PLAY_RULES_FIELD: &str = "rules";
/// The storage key of a play's user-owned switch.
pub const PLAY_ENABLED_FIELD: &str = "enabled";
/// The storage keys of a play's suspension, which only the engine writes.
pub const PLAY_SUSPENDED_REASON_FIELD: &str = "suspended_reason";
pub const PLAY_SUSPENDED_MESSAGE_FIELD: &str = "suspended_message";
pub const PLAY_SUSPENDED_AT_FIELD: &str = "suspended_at";
/// The three suspension fields together.
pub const PLAY_SUSPENSION_FIELDS: [&str; 3] = [
    PLAY_SUSPENDED_REASON_FIELD,
    PLAY_SUSPENDED_MESSAGE_FIELD,
    PLAY_SUSPENDED_AT_FIELD,
];

/// Why the engine took a play out of service on this device (ADR-087 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PlaySuspensionReason {
    /// The play's rules did not parse or validate.
    ValidationFailed,
    /// One of the play's actions failed.
    ActionFailed,
    /// A chain of rule firings reached the cycle limit.
    CycleLimit,
    /// A schema the play references changed and its rules no longer validate.
    SchemaDrift,
}

impl PlaySuspensionReason {
    /// Every reason, as `(variant, display label)`: the schema's enum.
    pub const ALL: [(Self, &'static str); 4] = [
        (Self::ValidationFailed, "Validation failed"),
        (Self::ActionFailed, "Action failed"),
        (Self::CycleLimit, "Cycle limit"),
        (Self::SchemaDrift, "Schema drift"),
    ];

    /// The stored value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ValidationFailed => "validation_failed",
            Self::ActionFailed => "action_failed",
            Self::CycleLimit => "cycle_limit",
            Self::SchemaDrift => "schema_drift",
        }
    }
}

impl std::fmt::Display for PlaySuspensionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The play schema's fields, decoded from a play node's properties.
///
/// The only reader of a stored play: storage keys are the schema's field
/// names, hoisted by the store under `properties.play.*`.
/// [`Self::from_properties`] reads that bucket, or the flat shape a node built
/// in memory or a create payload carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct PlayFields {
    pub rules: Vec<RuleDefinition>,
    /// What the play automates, in one line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The user's switch. The engine never changes it.
    pub enabled: bool,
    /// Why the engine suspended the play on this device, when it has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspended_reason: Option<PlaySuspensionReason>,
    /// The diagnostic the suspension was logged with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspended_message: Option<String>,
    /// When the engine suspended the play (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspended_at: Option<String>,
}

impl Default for PlayFields {
    /// The schema's defaults: no rules, switched on, not suspended.
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            description: None,
            enabled: true,
            suspended_reason: None,
            suspended_message: None,
            suspended_at: None,
        }
    }
}

impl PlayFields {
    /// Decode a play node's fields.
    ///
    /// # Errors
    ///
    /// `InvalidNodeType` if `node` is not a play, `InvalidProperties` if a
    /// field is present with the wrong shape (see [`Self::from_properties`]).
    pub fn from_node(node: &Node) -> Result<Self, ValidationError> {
        if !crate::CoreNodeType::Play.is_exactly(&node.node_type) {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected '{PLAY_NODE_TYPE}', got '{}'",
                node.node_type
            )));
        }
        Self::from_properties(&node.properties)
    }

    /// Decode from properties in either the hoisted (`properties.play.*`) or
    /// flat shape, preferring the `play` bucket when there is one.
    ///
    /// An absent or `null` field takes the schema's default: no rules, no
    /// description, switched on, not suspended. Keys the schema does not
    /// declare are ignored.
    ///
    /// # Errors
    ///
    /// `InvalidProperties` naming the field. For `rules` the message names
    /// the rule and what in it failed to decode: an unknown trigger type,
    /// event or action, a missing description, a condition written as a bare
    /// expression, a missing param, or a param the action does not take.
    pub fn from_properties(properties: &Value) -> Result<Self, ValidationError> {
        let bucket = play_bucket(properties);

        Ok(Self {
            rules: bucket
                .get(PLAY_RULES_FIELD)
                .filter(|v| !v.is_null())
                .map(decode_rules)
                .transpose()?
                .unwrap_or_default(),
            description: decode_field(bucket, "description")?,
            enabled: decode_field(bucket, PLAY_ENABLED_FIELD)?.unwrap_or(true),
            suspended_reason: decode_field(bucket, PLAY_SUSPENDED_REASON_FIELD)?,
            suspended_message: decode_field(bucket, PLAY_SUSPENDED_MESSAGE_FIELD)?,
            suspended_at: decode_field(bucket, PLAY_SUSPENDED_AT_FIELD)?,
        })
    }

    /// The fields of a play whose stored fields do not all decode: each field
    /// that does decode, and the schema's default for each that does not.
    ///
    /// A play with broken rules is one the engine suspends, so its switch and
    /// its suspension must still be readable.
    pub fn readable_from_properties(properties: &Value) -> Self {
        let bucket = play_bucket(properties);
        Self {
            rules: bucket
                .get(PLAY_RULES_FIELD)
                .and_then(|v| decode_rules(v).ok())
                .unwrap_or_default(),
            description: decode_field(bucket, "description").ok().flatten(),
            enabled: Self::enabled_in(properties),
            suspended_reason: decode_field(bucket, PLAY_SUSPENDED_REASON_FIELD)
                .ok()
                .flatten(),
            suspended_message: decode_field(bucket, PLAY_SUSPENDED_MESSAGE_FIELD)
                .ok()
                .flatten(),
            suspended_at: decode_field(bucket, PLAY_SUSPENDED_AT_FIELD).ok().flatten(),
        }
    }

    /// One stored play field, in either shape, with `null` read as absent.
    ///
    /// For a reader that must not depend on the rules decoding: the engine
    /// reads the switch and the suspension of a play whose rules are broken.
    pub fn stored_field<'a>(properties: &'a Value, key: &str) -> Option<&'a Value> {
        play_bucket(properties).get(key).filter(|v| !v.is_null())
    }

    /// The user's switch as stored: on unless it was set to `false`.
    pub fn enabled_in(properties: &Value) -> bool {
        Self::stored_field(properties, PLAY_ENABLED_FIELD).and_then(Value::as_bool) != Some(false)
    }

    /// Whether the engine has a suspension recorded on the play.
    pub fn suspended_in(properties: &Value) -> bool {
        Self::stored_field(properties, PLAY_SUSPENDED_AT_FIELD).is_some()
    }
}

/// The `play` bucket of stored properties, or the properties themselves when
/// they are flat.
fn play_bucket(properties: &Value) -> &Value {
    properties
        .get(PLAY_NODE_TYPE)
        .filter(|b| b.is_object())
        .unwrap_or(properties)
}

/// Decode one scalar play field; absent and `null` are `None`.
fn decode_field<T: serde::de::DeserializeOwned>(
    bucket: &Value,
    key: &str,
) -> Result<Option<T>, ValidationError> {
    bucket
        .get(key)
        .filter(|v| !v.is_null())
        .map(|v| serde_json::from_value(v.clone()).map_err(|e| invalid(key, &e.to_string())))
        .transpose()
}

/// Decode a stored `rules` value.
fn decode_rules(value: &Value) -> Result<Vec<RuleDefinition>, ValidationError> {
    decode_rule_list(value).map_err(|detail| invalid("rules", &detail))
}

/// Decode a `rules` value one rule at a time, so an error names the rule it
/// is in and the field within it (`conditions[1]`, `actions[0]`).
fn decode_rule_list(value: &Value) -> Result<Vec<RuleDefinition>, String> {
    let rules = value
        .as_array()
        .ok_or_else(|| "expected an array of rules".to_string())?;
    rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            serde_path_to_error::deserialize(rule).map_err(|e| {
                let name = rule.get("name").and_then(Value::as_str);
                let label = match name {
                    Some(name) => format!("rule[{index}] ('{name}')"),
                    None => format!("rule[{index}]"),
                };
                let path = e.path().to_string();
                if path == "." {
                    format!("{label}: {}", e.inner())
                } else {
                    format!("{label}: {path}: {}", e.inner())
                }
            })
        })
        .collect()
}

/// Decode an update's `rules` the way stored rules are decoded, so a typed
/// update's error names the rule and the field too.
fn deserialize_update_rules<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<RuleDefinition>>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    decode_rule_list(&value)
        .map(Some)
        .map_err(|detail| serde::de::Error::custom(format!("rules: {detail}")))
}

fn invalid(key: &str, detail: &str) -> ValidationError {
    ValidationError::InvalidProperties(format!("play field '{key}': {detail}"))
}

// ============================================================================
// Wire shape and typed update
// ============================================================================

/// Wire shape for play nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for a play: the play schema's fields are
/// promoted to the top level and `properties` keeps only extension fields.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PlayNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    #[serde(flatten)]
    pub fields: PlayFields,
}

/// Partial update for a play's fields.
///
/// `rules` is replaced whole and has no clear path (an empty list is how a
/// play has no rules); `description` is tri-state: absent leaves it
/// unchanged, `null` clears it, and a string sets it. `enabled` is the user's
/// switch; writing `true` also clears a suspension. The suspension fields are
/// the engine's, so the update does not carry them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayNodeUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_update_rules"
    )]
    pub rules: Option<Vec<RuleDefinition>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub description: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl PlayNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The flat properties patch this update writes (`{"rules": […]}`); a
    /// cleared field is written as `null`. The service layer moves the keys
    /// into the `play` storage bucket.
    ///
    /// Also the shape a play is created with: a create payload carries the
    /// same storage keys, so building one from an update keeps a single
    /// writer of the stored names.
    pub fn to_properties_patch(&self) -> Value {
        let mut patch = Map::new();
        if let Some(rules) = &self.rules {
            patch.insert(PLAY_RULES_FIELD.to_string(), to_json(rules));
        }
        if let Some(description) = &self.description {
            patch.insert("description".to_string(), to_json(description));
        }
        if let Some(enabled) = self.enabled {
            patch.insert(PLAY_ENABLED_FIELD.to_string(), Value::Bool(enabled));
        }
        Value::Object(patch)
    }
}

/// Serialize a value whose `Serialize` cannot fail (plain data, string keys).
fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("play field types always serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(trigger: Value, actions: Value) -> Value {
        json!({ "name": "r", "description": "Test rule", "trigger": trigger, "actions": actions })
    }

    fn graph_event() -> Value {
        json!({ "type": "graph_event", "on": "node_created", "select": { "target_type": "task" } })
    }

    fn decode(rule: Value) -> Result<RuleDefinition, String> {
        serde_json::from_value(rule).map_err(|e| e.to_string())
    }

    #[test]
    fn a_rule_round_trips_through_its_stored_shape() {
        let stored = json!({
            "name": "roll over",
            "class": "reactive",
            "description": "Test rule",
            "trigger": {
                "type": "scheduled",
                "cron": "0 5 0 * * * *",
                "select": {
                    "target_type": "cycle",
                    "filters": [
                        { "type": "property", "operator": "equals", "property": "state", "value": "active" }
                    ]
                }
            },
            "conditions": [{ "expr": "node.end_date == today()", "description": "Test condition" }],
            "actions": [
                {
                    "description": "Test action",
                    "action_type": "create_node",
                    "params": {
                        "node_type": "cycle",
                        "version": 1,
                        "content": "Next cycle",
                        "properties": { "start_date": "{add_days(trigger.node.end_date, 1)}" }
                    }
                },
                {
                    "description": "Test action",
                    "action_type": "add_relationship",
                    "for_each": "trigger.node.tasks.where(status != 'done')",
                    "params": {
                        "source_id": "{actions[0].result.id}",
                        "relationship_type": "tasks",
                        "target_id": "{item.id}"
                    }
                }
            ]
        });

        let rule = decode(stored.clone()).unwrap();
        assert!(matches!(rule.trigger, Trigger::Scheduled { .. }));
        assert_eq!(rule.actions[0].action_type(), ActionType::CreateNode);
        assert_eq!(
            rule.actions[1].for_each(),
            Some("trigger.node.tasks.where(status != 'done')")
        );
        assert_eq!(
            rule.actions[1].params_value()["target_id"],
            json!("{item.id}")
        );

        let written = serde_json::to_value(&rule).unwrap();
        assert_eq!(decode(written.clone()).unwrap(), rule);
        assert_eq!(written["trigger"]["select"]["target_type"], "cycle");
        assert_eq!(written["actions"][0]["action_type"], "create_node");
    }

    #[test]
    fn class_conditions_and_actions_take_their_defaults() {
        let rule = decode(json!({ "name": "r", "description": "Test rule", "trigger": graph_event() })).unwrap();
        assert_eq!(rule.class, RuleClass::Reactive);
        assert!(rule.conditions.is_empty());
        assert!(rule.actions.is_empty());
    }

    #[test]
    fn an_unknown_trigger_type_event_or_field_is_rejected() {
        for trigger in [
            json!({ "type": "webhook", "select": { "target_type": "task" } }),
            json!({ "type": "graph_event", "on": "node_deleted", "select": { "target_type": "task" } }),
            // A scheduled trigger takes no `on`; a graph event takes no `cron`.
            json!({ "type": "scheduled", "cron": "0 * * * * * *", "on": "node_created", "select": { "target_type": "task" } }),
            json!({ "type": "graph_event", "on": "node_created", "cron": "0 * * * * * *", "select": { "target_type": "task" } }),
            // The selector is `select`, not a bare `node_type`.
            json!({ "type": "graph_event", "on": "node_created", "node_type": "task" }),
        ] {
            assert!(
                decode(rule(trigger.clone(), json!([]))).is_err(),
                "{trigger} must be rejected"
            );
        }
    }

    #[test]
    fn a_selector_is_inline_or_a_saved_query() {
        let inline: Selector = serde_json::from_value(json!({ "target_type": "task" })).unwrap();
        assert_eq!(inline, Selector::of_type("task"));
        assert_eq!(
            serde_json::to_value(&inline).unwrap(),
            json!({ "target_type": "task" })
        );

        let saved: Selector = serde_json::from_value(json!({ "query_id": "q-1" })).unwrap();
        assert_eq!(saved, Selector::saved_query("q-1"));
        assert_eq!(
            serde_json::to_value(&saved).unwrap(),
            json!({ "query_id": "q-1" })
        );

        let filtered: Selector = serde_json::from_value(json!({
            "target_type": "task",
            "filters": [{ "type": "content", "operator": "contains", "value": "x", "case_sensitive": false }]
        }))
        .unwrap();
        let Selector::Inline(inline) = &filtered else {
            panic!("expected an inline selector, got {filtered:?}");
        };
        assert_eq!(inline.filters[0].case_sensitive, Some(false));

        for bad in [
            json!({}),
            json!("task"),
            // A filter is held to the same strictness as the rule around it.
            json!({
                "target_type": "task",
                "filters": [{ "type": "content", "operator": "contains", "value": "x", "caseSensitive": false }]
            }),
            json!({ "target_type": "task", "query_id": "q-1" }),
            json!({ "target_type": "task", "limit": 5 }),
            json!({ "query_id": 7 }),
        ] {
            let err = serde_json::from_value::<Selector>(bad.clone());
            assert!(err.is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn an_unknown_action_is_rejected() {
        let err = decode(rule(
            graph_event(),
            json!([{ "description": "Test action", "action_type": "spawn_agent", "params": {} }]),
        ))
        .unwrap_err();
        assert!(err.contains("spawn_agent"), "{err}");
    }

    /// One case per action: a param the action does not take is refused when
    /// the rule is decoded, so a play that would do nothing cannot be saved.
    #[test]
    fn each_action_rejects_a_param_it_does_not_take() {
        let cases = [
            (
                "create_node",
                json!({ "node_type": "task", "parent_id": "{trigger.node.id}" }),
                "parent_id",
            ),
            (
                "update_node",
                json!({ "node_id": "{trigger.node.id}", "lifecycle_status": "archived" }),
                "lifecycle_status",
            ),
            (
                "add_relationship",
                json!({ "source_id": "a", "relationship_type": "tasks", "target_id": "b", "properties": {} }),
                "properties",
            ),
            (
                "remove_relationship",
                json!({ "source_id": "a", "relationship_type": "tasks", "target_id": "b", "edge_data": {} }),
                "edge_data",
            ),
            (
                "reject",
                json!({ "message": "no", "reason": "because" }),
                "reason",
            ),
        ];
        for (action_type, params, unknown) in cases {
            let err = decode(rule(
                graph_event(),
                json!([{ "description": "Test action", "action_type": action_type, "params": params }]),
            ))
            .unwrap_err();
            assert!(
                err.contains(unknown),
                "{action_type}: the error must name `{unknown}`: {err}"
            );
        }
    }

    #[test]
    fn each_action_requires_its_params() {
        for (action_type, params) in [
            ("create_node", json!({ "content": "x" })),
            ("update_node", json!({ "properties": {} })),
            (
                "add_relationship",
                json!({ "source_id": "a", "target_id": "b" }),
            ),
            (
                "remove_relationship",
                json!({ "source_id": "a", "relationship_type": "t" }),
            ),
            ("reject", json!({})),
        ] {
            assert!(
                decode(rule(
                    graph_event(),
                    json!([{ "description": "Test action", "action_type": action_type, "params": params }]),
                ))
                .is_err(),
                "{action_type} must require its params"
            );
        }
    }

    #[test]
    fn open_json_is_kept_only_for_properties_and_edge_data() {
        let decoded = decode(rule(
            graph_event(),
            json!([
                {
                    "description": "Test action",
                    "action_type": "update_node",
                    "params": { "node_id": "n", "properties": { "anything": { "nested": [1, 2] } } }
                },
                {
                    "description": "Test action",
                    "action_type": "add_relationship",
                    "params": {
                        "source_id": "a", "relationship_type": "t", "target_id": "b",
                        "edge_data": { "role": "owner" }
                    }
                }
            ]),
        ))
        .unwrap();
        assert_eq!(
            decoded.actions[0].params_value()["properties"]["anything"]["nested"],
            json!([1, 2])
        );
        assert_eq!(
            decoded.actions[1].params_value()["edge_data"]["role"],
            "owner"
        );

        // The open leaves are objects, not arbitrary JSON.
        assert!(decode(rule(
            graph_event(),
            json!([{ "description": "Test action", "action_type": "update_node", "params": { "node_id": "n", "properties": "x" } }]),
        ))
        .is_err());
    }

    #[test]
    fn a_reject_action_takes_no_for_each() {
        let err = decode(rule(
            graph_event(),
            json!([{ "description": "Test action", "action_type": "reject", "params": { "message": "no" }, "for_each": "trigger.node.tasks" }]),
        ))
        .unwrap_err();
        assert!(err.contains("for_each"), "{err}");
    }

    #[test]
    fn fields_decode_from_the_bucket_or_the_flat_shape() {
        let rules = json!([rule(graph_event(), json!([]))]);
        for properties in [
            json!({ "play": { "rules": rules, "description": "d" }, "_seed": { "tier": "core" } }),
            json!({ "rules": rules, "description": "d" }),
        ] {
            let fields = PlayFields::from_properties(&properties).unwrap();
            assert_eq!(fields.rules.len(), 1);
            assert_eq!(fields.description.as_deref(), Some("d"));
        }
        assert_eq!(
            PlayFields::from_properties(&json!({})).unwrap(),
            PlayFields::default()
        );
    }

    #[test]
    fn a_malformed_rule_is_reported_by_index_and_name() {
        let err = PlayFields::from_properties(&json!({
            "rules": [
                rule(graph_event(), json!([])),
                { "name": "broken", "description": "Test rule", "trigger": { "type": "nope" } }
            ]
        }))
        .unwrap_err()
        .to_string();
        assert!(err.contains("rule[1] ('broken')"), "{err}");

        let err = PlayFields::from_properties(&json!({ "rules": "none" }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("'rules'"), "{err}");
    }

    /// A rule, each condition and each action must carry a description, and a
    /// condition is an object: the error names the rule and the field.
    #[test]
    fn a_missing_description_or_a_bare_condition_names_the_rule_and_the_field() {
        let described = json!({
            "name": "close parent",
            "description": "Close the parent",
            "class": "reactive",
            "trigger": graph_event(),
            "conditions": [
                { "expr": "node.status == 'done'", "description": "The task is done" }
            ],
            "actions": [{
                "action_type": "reject",
                "description": "Refuse the write",
                "params": { "message": "no" }
            }]
        });
        let decoded = PlayFields::from_properties(&json!({ "rules": [described] })).unwrap();
        assert_eq!(decoded.rules[0].description, "Close the parent");
        assert_eq!(decoded.rules[0].conditions[0].expr, "node.status == 'done'");
        assert_eq!(decoded.rules[0].actions[0].description(), "Refuse the write");
        assert_eq!(serde_json::to_value(&decoded.rules[0]).unwrap(), described);

        let without = |pointer: &str| {
            let mut rule = described.clone();
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            rule.pointer_mut(parent)
                .and_then(Value::as_object_mut)
                .unwrap()
                .remove(key);
            rule
        };
        let mut bare = described.clone();
        bare["conditions"][0] = json!("node.status == 'done'");

        for (rule, expected) in [
            (without("/description"), "missing field `description`"),
            (
                without("/conditions/0/description"),
                "conditions[0]: missing field `description`",
            ),
            (
                without("/conditions/0/expr"),
                "conditions[0]: missing field `expr`",
            ),
            (
                without("/actions/0/description"),
                "actions[0]: missing field `description`",
            ),
            (bare.clone(), "conditions[0]: invalid type: string"),
        ] {
            let stored = PlayFields::from_properties(&json!({ "rules": [rule] }))
                .unwrap_err()
                .to_string();
            assert!(
                stored.contains(&format!("rule[0] ('close parent'): {expected}")),
                "{stored}"
            );

            // The typed update reports the same thing.
            let update = serde_json::from_value::<PlayNodeUpdate>(json!({ "rules": [rule] }))
                .unwrap_err()
                .to_string();
            assert!(
                update.contains(&format!("rule[0] ('close parent'): {expected}")),
                "{update}"
            );
        }

        let bare = PlayFields::from_properties(&json!({ "rules": [bare] }))
            .unwrap_err()
            .to_string();
        assert!(bare.contains(r#"expected an object { "expr""#), "{bare}");
    }

    #[test]
    fn update_replaces_rules_whole_and_clears_description_with_null() {
        let update: PlayNodeUpdate = serde_json::from_value(json!({
            "rules": [rule(graph_event(), json!([]))],
            "description": null
        }))
        .unwrap();
        assert_eq!(update.description, Some(None));
        let patch = update.to_properties_patch();
        assert_eq!(patch["rules"].as_array().unwrap().len(), 1);
        assert!(patch["description"].is_null());

        assert!(PlayNodeUpdate::default().is_empty());
    }

    #[test]
    fn update_writes_the_switch_and_never_a_suspension() {
        let update: PlayNodeUpdate = serde_json::from_value(json!({ "enabled": false })).unwrap();
        assert!(!update.is_empty());
        assert_eq!(update.to_properties_patch(), json!({ "enabled": false }));

        for key in ["suspendedReason", "suspendedMessage", "suspendedAt"] {
            assert!(
                serde_json::from_value::<PlayNodeUpdate>(json!({ key: "x" })).is_err(),
                "a typed update must not carry `{key}`"
            );
        }
    }

    #[test]
    fn a_play_is_on_and_unsuspended_by_default() {
        let fields = PlayFields::from_properties(&json!({})).unwrap();
        assert!(fields.enabled);
        assert_eq!(fields.suspended_reason, None);
        assert!(PlayFields::enabled_in(
            &json!({ "play": { "enabled": null } })
        ));
        assert!(!PlayFields::suspended_in(
            &json!({ "play": { "suspended_at": null } })
        ));
    }

    #[test]
    fn the_switch_and_suspension_decode_from_storage_and_travel_camel_case() {
        let properties = json!({ "play": {
            "rules": [],
            "enabled": false,
            "suspended_reason": "action_failed",
            "suspended_message": "boom",
            "suspended_at": "2026-10-02T10:00:00Z"
        } });
        let fields = PlayFields::from_properties(&properties).unwrap();
        assert!(!fields.enabled);
        assert_eq!(
            fields.suspended_reason,
            Some(PlaySuspensionReason::ActionFailed)
        );
        assert!(!PlayFields::enabled_in(&properties));
        assert!(PlayFields::suspended_in(&properties));

        let wire = serde_json::to_value(&fields).unwrap();
        assert_eq!(wire["enabled"], false);
        assert_eq!(wire["suspendedReason"], "action_failed");
        assert_eq!(wire["suspendedMessage"], "boom");
        assert_eq!(wire["suspendedAt"], "2026-10-02T10:00:00Z");

        let err = PlayFields::from_properties(&json!({ "suspended_reason": "tired" }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("suspended_reason"), "{err}");
    }

    /// The switch and the suspension are read without decoding the rules, so
    /// the engine can still tell that a play with broken rules is switched off.
    #[test]
    fn the_switch_is_readable_when_the_rules_are_not() {
        let properties = json!({ "play": { "rules": "nope", "enabled": false } });
        assert!(PlayFields::from_properties(&properties).is_err());
        assert!(!PlayFields::enabled_in(&properties));
    }

    #[test]
    fn a_play_with_broken_rules_keeps_its_readable_fields() {
        let properties = json!({ "play": {
            "rules": [{ "name": "r", "description": "Test rule", "trigger": { "type": "nope" } }],
            "description": "d",
            "enabled": false,
            "suspended_reason": "validation_failed",
            "suspended_at": "2026-10-02T10:00:00Z"
        } });
        let fields = PlayFields::readable_from_properties(&properties);
        assert!(fields.rules.is_empty());
        assert_eq!(fields.description.as_deref(), Some("d"));
        assert!(!fields.enabled);
        assert_eq!(
            fields.suspended_reason,
            Some(PlaySuspensionReason::ValidationFailed)
        );
        assert!(fields.suspended_at.is_some());
    }

    #[test]
    fn suspension_reasons_spell_their_stored_values() {
        for (reason, _) in PlaySuspensionReason::ALL {
            assert_eq!(
                serde_json::to_value(reason).unwrap(),
                json!(reason.as_str())
            );
        }
    }
}
