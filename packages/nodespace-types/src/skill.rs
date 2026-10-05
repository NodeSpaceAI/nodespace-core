use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::helpers::{deserialize_clearable, deserialize_set_only};
use crate::node::{Node, NodeEnvelope, ValidationError};

/// The `node_type` of every skill node.
pub const SKILL_NODE_TYPE: &str = "skill";

/// The relationship from a skill to the schema nodes it is about. Skill
/// search carries those schemas' definitions with a matched skill.
pub const SKILL_APPLIES_TO: &str = "applies_to";

/// The relationship from a skill to a node of any type it is handed over
/// with: a read of that node returns the skill (ADR-094 §3). It delivers the
/// skill and does not scope it.
pub const SKILL_ATTACHED_TO: &str = "attached_to";

/// `max_iterations` when a skill doesn't set one — the core schema's default.
pub const DEFAULT_SKILL_MAX_ITERATIONS: u32 = 2;

/// The typed fields of a `skill` node: its retrieval and dispatch config. The
/// skill's name is the node's `content`, its guidance is its child subtree,
/// and the schemas it is about are its [`SKILL_APPLIES_TO`] edges; none of
/// those is part of this shape.
// The one reader and writer of a skill's stored fields. `skill` has a
// registered core schema, so the store hoists its fields under
// `properties.skill.*`; a node built in memory, a seed template, or a flat
// update patch carries them at the top level instead. `from_properties` reads
// both, preferring the `skill` bucket per field: a reader that guesses one of
// the two shapes reads the other as empty. Serialized, these are the camelCase
// fields the wire `SkillNode` promotes; storage keeps the schema's snake_case
// names (`properties`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct SkillFields {
    /// The requests the skill should handle, worded the way someone would
    /// ask. Embedded with the skill's name for retrieval.
    pub use_for: String,
    /// Requests that sound similar but belong to another skill, scored
    /// against the query to penalize verb-only overlaps. `None` when absent or blank.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_for: Option<String>,
    /// Tools a turn that selects this skill may call.
    pub tool_whitelist: Vec<String>,
    /// ReAct iteration budget for the skill.
    pub max_iterations: u32,
}

impl Default for SkillFields {
    /// What a skill with no stored fields reads as: the schema's defaults.
    fn default() -> Self {
        Self {
            use_for: String::new(),
            not_for: None,
            tool_whitelist: Vec::new(),
            max_iterations: DEFAULT_SKILL_MAX_ITERATIONS,
        }
    }
}

impl SkillFields {
    /// A skill's fields with no `not_for`.
    pub fn new(use_for: impl Into<String>, tool_whitelist: &[&str], max_iterations: u32) -> Self {
        Self {
            use_for: use_for.into(),
            not_for: None,
            tool_whitelist: tool_whitelist.iter().map(|t| t.to_string()).collect(),
            max_iterations,
        }
    }

    /// Set what the skill is not for. A blank value is none.
    pub fn with_not_for(mut self, not_for: impl Into<String>) -> Self {
        self.not_for = normalize_not_for(&not_for.into());
        self
    }

    /// Decode a skill node's fields.
    ///
    /// # Errors
    ///
    /// `InvalidNodeType` if `node` is not a skill, `InvalidProperties` if a
    /// field is present with the wrong type (see [`Self::from_properties`]).
    pub fn from_node(node: &Node) -> Result<Self, ValidationError> {
        if !crate::CoreNodeType::Skill.is_exactly(&node.node_type) {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected '{SKILL_NODE_TYPE}', got '{}'",
                node.node_type
            )));
        }
        Self::from_properties(&node.properties)
    }

    /// Decode a skill's fields from its properties, in either the hoisted
    /// (`properties.skill.*`) or flat shape. For callers that hold a skill's
    /// parts rather than a [`Node`], such as a wire record or a seed template.
    ///
    /// An absent or `null` field takes its default: empty `use_for` and
    /// lists, no `not_for`, [`DEFAULT_SKILL_MAX_ITERATIONS`].
    ///
    /// # Errors
    ///
    /// `InvalidProperties` if a field is present with the wrong type, or
    /// `max_iterations` is not a positive integer.
    pub fn from_properties(properties: &Value) -> Result<Self, ValidationError> {
        let field = |key: &str| {
            properties
                .get(SKILL_NODE_TYPE)
                .and_then(|bucket| bucket.get(key))
                .or_else(|| properties.get(key))
                .filter(|v| !v.is_null())
        };

        let use_for = optional_string(field("use_for"), "use_for")?;
        let not_for = optional_string(field("not_for"), "not_for")?
            .as_deref()
            .and_then(normalize_not_for);
        let tool_whitelist = string_list(field("tool_whitelist"), "tool_whitelist")?;
        let max_iterations = match field("max_iterations") {
            None => DEFAULT_SKILL_MAX_ITERATIONS,
            Some(v) => v
                .as_u64()
                .filter(|n| *n >= 1)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    ValidationError::InvalidProperties(
                        "max_iterations must be a positive integer".to_string(),
                    )
                })?,
        };

        Ok(Self {
            use_for: use_for.unwrap_or_default(),
            not_for,
            tool_whitelist,
            max_iterations,
        })
    }

    /// The skill's config as flat properties — the shape a create or update
    /// writes, which the store hoists under `properties.skill.*`.
    ///
    /// `not_for` is omitted when unset rather than written empty.
    pub fn properties(&self) -> Value {
        let mut props = Map::new();
        props.insert("use_for".to_string(), json!(self.use_for));
        if let Some(not_for) = &self.not_for {
            props.insert("not_for".to_string(), json!(not_for));
        }
        props.insert("tool_whitelist".to_string(), json!(self.tool_whitelist));
        props.insert("max_iterations".to_string(), json!(self.max_iterations));
        Value::Object(props)
    }

    /// A new skill [`Node`] named `name` carrying this config, with a fresh
    /// id.
    pub fn into_node(self, name: impl Into<String>) -> Node {
        Node::new(SKILL_NODE_TYPE.to_string(), name.into(), self.properties())
    }
}

/// Wire shape for skill nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for a `skill` node: the skill schema's
/// fields are promoted to the top level (camelCase, see [`SkillFields`]) and
/// `properties` keeps only extension fields. The skill's name is the
/// envelope's `content`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    #[serde(flatten)]
    pub fields: SkillFields,
}

/// Partial update for a skill's core fields, received from the frontend.
///
/// `use_for` and `tool_whitelist` have no clear path (the schema requires
/// them), and `null` for either is refused rather than read as absent; the
/// other fields are tri-state: absent leaves the field unchanged,
/// `null` clears it, and a value sets it. A list is replaced whole. The
/// skill's name is `content`, an envelope field, and its guidance is its
/// child subtree; both are written through the generic node operations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillNodeUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_set_only"
    )]
    pub use_for: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub not_for: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_set_only"
    )]
    pub tool_whitelist: Option<Vec<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub max_iterations: Option<Option<u32>>,
}

impl SkillNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The flat, bare-key properties patch this update writes
    /// (`{"max_iterations": 3}`); a cleared field is written as `null`. The
    /// service layer moves the keys into the `skill` storage bucket.
    pub fn to_properties_patch(&self) -> Value {
        let mut patch = Map::new();
        if let Some(use_for) = &self.use_for {
            patch.insert("use_for".to_string(), json!(use_for));
        }
        if let Some(not_for) = &self.not_for {
            patch.insert("not_for".to_string(), json!(not_for));
        }
        if let Some(tool_whitelist) = &self.tool_whitelist {
            patch.insert("tool_whitelist".to_string(), json!(tool_whitelist));
        }
        if let Some(max_iterations) = &self.max_iterations {
            patch.insert("max_iterations".to_string(), json!(max_iterations));
        }
        Value::Object(patch)
    }
}

fn normalize_not_for(not_for: &str) -> Option<String> {
    let trimmed = not_for.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn optional_string(value: Option<&Value>, key: &str) -> Result<Option<String>, ValidationError> {
    value
        .map(|v| {
            v.as_str().map(str::to_string).ok_or_else(|| {
                ValidationError::InvalidProperties(format!("{key} must be a string"))
            })
        })
        .transpose()
}

fn string_list(value: Option<&Value>, key: &str) -> Result<Vec<String>, ValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| ValidationError::InvalidProperties(format!("{key} must be an array")))?;
    items
        .iter()
        .map(|item| {
            item.as_str().map(str::to_string).ok_or_else(|| {
                ValidationError::InvalidProperties(format!("{key} items must be strings"))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SkillFields {
        SkillFields::new("Update a record", &["update_node", "get_node"], 3)
            .with_not_for("Delete records")
    }

    #[test]
    fn round_trips_through_a_node() {
        let skill = sample();
        let node = skill.clone().into_node("Graph Editing");
        assert_eq!(node.node_type, "skill");
        assert_eq!(node.content, "Graph Editing");
        assert_eq!(SkillFields::from_node(&node).unwrap(), skill);
    }

    #[test]
    fn round_trips_through_the_hoisted_storage_shape() {
        let skill = sample();
        let mut node = skill.clone().into_node("Graph Editing");
        node.properties = json!({ "skill": skill.properties() });
        assert_eq!(SkillFields::from_node(&node).unwrap(), skill);
    }

    #[test]
    fn minimal_skill_round_trips_without_optional_keys() {
        let skill = SkillFields::new("Search", &["search_nodes"], 4);
        let props = skill.properties();
        assert!(props.get("not_for").is_none());
        assert_eq!(SkillFields::from_properties(&props).unwrap(), skill);
    }

    #[test]
    fn hoisted_field_wins_over_flat_per_field() {
        // A flat patch applied on top of a hoisted node leaves both shapes in
        // one bag; the bucket is the stored value.
        let props = json!({
            "max_iterations": 2,
            "tool_whitelist": ["get_node"],
            "skill": { "max_iterations": 4, "use_for": "d" },
        });
        let skill = SkillFields::from_properties(&props).unwrap();
        assert_eq!(skill.max_iterations, 4);
        assert_eq!(skill.use_for, "d");
        // Absent from the bucket, so read from the flat level.
        assert_eq!(skill.tool_whitelist, vec!["get_node"]);
    }

    #[test]
    fn null_skill_bucket_falls_back_to_flat() {
        let props = json!({ "skill": null, "use_for": "d", "tool_whitelist": ["x"] });
        let skill = SkillFields::from_properties(&props).unwrap();
        assert_eq!(skill.use_for, "d");
        assert_eq!(skill.tool_whitelist, vec!["x"]);
    }

    #[test]
    fn absent_and_null_fields_take_defaults() {
        let skill = SkillFields::from_properties(
            &json!({ "skill": { "not_for": null, "max_iterations": null } }),
        )
        .unwrap();
        assert_eq!(skill, SkillFields::default());
        assert_eq!(skill.max_iterations, DEFAULT_SKILL_MAX_ITERATIONS);
    }

    #[test]
    fn blank_not_for_is_none() {
        let skill =
            SkillFields::from_properties(&json!({ "skill": { "not_for": "   " } })).unwrap();
        assert_eq!(skill.not_for, None);
        assert_eq!(SkillFields::new("", &[], 1).with_not_for(" ").not_for, None);
    }

    #[test]
    fn not_for_is_trimmed() {
        let skill = SkillFields::from_properties(&json!({ "not_for": "  Delete them. " })).unwrap();
        assert_eq!(skill.not_for.as_deref(), Some("Delete them."));
    }

    #[test]
    fn wrong_typed_fields_are_rejected() {
        for (props, message) in [
            (json!({ "use_for": 5 }), "use_for must be a string"),
            (json!({ "not_for": [] }), "not_for must be a string"),
            (
                json!({ "tool_whitelist": "get_node" }),
                "tool_whitelist must be an array",
            ),
            (
                json!({ "tool_whitelist": [1] }),
                "tool_whitelist items must be strings",
            ),
            (
                json!({ "max_iterations": 0 }),
                "max_iterations must be a positive integer",
            ),
            (
                json!({ "max_iterations": -1 }),
                "max_iterations must be a positive integer",
            ),
            (
                json!({ "max_iterations": "3" }),
                "max_iterations must be a positive integer",
            ),
        ] {
            match SkillFields::from_properties(&props) {
                Err(ValidationError::InvalidProperties(m)) => assert_eq!(m, message, "{props}"),
                other => panic!("expected InvalidProperties for {props}, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_a_non_skill_node() {
        let node = Node::new("text".to_string(), "x".to_string(), json!({}));
        assert!(matches!(
            SkillFields::from_node(&node),
            Err(ValidationError::InvalidNodeType(_))
        ));
    }

    /// A required field has no clear path: `null` is refused, naming why,
    /// rather than read as "unchanged" and dropped.
    #[test]
    fn update_refuses_to_clear_a_required_field() {
        for json in [
            r#"{"useFor": null}"#,
            r#"{"toolWhitelist": null}"#,
            r#"{"maxIterations": 3, "useFor": null}"#,
        ] {
            let error = serde_json::from_str::<SkillNodeUpdate>(json)
                .expect_err("null must not clear a required field")
                .to_string();
            assert!(error.contains("cannot be cleared"), "{json}: {error}");
        }
        // Absent is still "unchanged", and a value still sets.
        let update: SkillNodeUpdate =
            serde_json::from_str(r#"{"useFor": "d", "toolWhitelist": []}"#).unwrap();
        assert_eq!(update.use_for.as_deref(), Some("d"));
        assert_eq!(update.tool_whitelist, Some(Vec::new()));
    }

    /// The update carries the skill schema's fields only, by wire name;
    /// naming anything else is an error rather than a silently dropped write.
    #[test]
    fn update_rejects_an_unknown_key() {
        for json in [
            r#"{"content": "Renamed"}"#,
            r#"{"tool_whitelist": []}"#,
            r#"{"nodeTypes": ["task"]}"#,
        ] {
            assert!(
                serde_json::from_str::<SkillNodeUpdate>(json).is_err(),
                "{json} must not deserialize as a SkillNodeUpdate"
            );
        }
    }

    #[test]
    fn update_distinguishes_absent_null_and_value() {
        let update: SkillNodeUpdate = serde_json::from_str(
            r#"{"notFor": null, "maxIterations": 5, "toolWhitelist": ["get_node"]}"#,
        )
        .unwrap();
        assert_eq!(update.use_for, None);
        assert_eq!(update.not_for, Some(None));
        assert_eq!(update.max_iterations, Some(Some(5)));
        assert_eq!(
            update.to_properties_patch(),
            json!({
                "not_for": null,
                "tool_whitelist": ["get_node"],
                "max_iterations": 5
            })
        );
        assert!(!update.is_empty());
        assert!(serde_json::from_str::<SkillNodeUpdate>("{}")
            .unwrap()
            .is_empty());
    }

    /// The wire fields are camelCase and an unset `not_for` is omitted.
    #[test]
    fn fields_serialize_camel_case() {
        assert_eq!(
            serde_json::to_value(SkillFields::new("Search", &["get_node"], 4)).unwrap(),
            json!({
                "useFor": "Search",
                "toolWhitelist": ["get_node"],
                "maxIterations": 4
            })
        );
    }
}
