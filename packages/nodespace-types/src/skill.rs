use serde_json::{json, Map, Value};

use crate::node::{Node, ValidationError};

/// The `node_type` of every skill node.
pub const SKILL_NODE_TYPE: &str = "skill";

/// `max_iterations` when a skill doesn't set one — the core schema's default.
pub const DEFAULT_SKILL_MAX_ITERATIONS: u32 = 2;

/// Strongly typed view of a `skill` node: its name plus the retrieval and
/// dispatch config stored in its properties.
///
/// This is the only place a skill field is read from or written to the
/// properties bag. `skill` has a registered core schema, so the store hoists
/// its fields under `properties.skill.*`; a node built in memory, a seed
/// template, or a flat update patch carries them at the top level instead.
/// [`SkillNode::from_properties`] reads both, preferring the `skill` bucket
/// per field. Every hand-rolled reader that guessed only one of the two
/// shapes read that field as empty.
///
/// The skill's guidance body is its child subtree, not a property, so it is
/// not part of this model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillNode {
    /// The skill's name, stored as the node's `content`.
    pub name: String,
    /// What the skill is for. Drives the skill's embedding for retrieval.
    pub description: String,
    /// What the skill is *not* for, scored against the query to penalize
    /// verb-only overlaps. `None` when absent or blank.
    pub exclusion: Option<String>,
    /// Tools a turn that selects this skill may call.
    pub tool_whitelist: Vec<String>,
    /// ReAct iteration budget for the skill.
    pub max_iterations: u32,
    /// Schema ids this skill is scoped to. Empty means unscoped.
    pub node_types: Vec<String>,
}

impl SkillNode {
    /// A skill with no exclusion and no type scope.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        tool_whitelist: &[&str],
        max_iterations: u32,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            exclusion: None,
            tool_whitelist: tool_whitelist.iter().map(|t| t.to_string()).collect(),
            max_iterations,
            node_types: Vec::new(),
        }
    }

    /// Set what the skill is not for. A blank exclusion is no exclusion.
    pub fn with_exclusion(mut self, exclusion: impl Into<String>) -> Self {
        self.exclusion = normalize_exclusion(&exclusion.into());
        self
    }

    /// Scope the skill to these schema ids.
    pub fn with_node_types(mut self, node_types: &[&str]) -> Self {
        self.node_types = node_types.iter().map(|t| t.to_string()).collect();
        self
    }

    /// Decode a skill node.
    ///
    /// # Errors
    ///
    /// `InvalidNodeType` if `node` is not a skill, `InvalidProperties` if a
    /// field is present with the wrong type (see [`Self::from_properties`]).
    pub fn from_node(node: &Node) -> Result<Self, ValidationError> {
        if node.node_type != SKILL_NODE_TYPE {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected '{SKILL_NODE_TYPE}', got '{}'",
                node.node_type
            )));
        }
        Self::from_properties(&node.content, &node.properties)
    }

    /// Decode a skill from its name and properties, in either the hoisted
    /// (`properties.skill.*`) or flat shape. For callers that hold a skill's
    /// parts rather than a [`Node`], such as a wire record or a seed template.
    ///
    /// An absent or `null` field takes its default: empty description and
    /// lists, no exclusion, [`DEFAULT_SKILL_MAX_ITERATIONS`].
    ///
    /// # Errors
    ///
    /// `InvalidProperties` if a field is present with the wrong type, or
    /// `max_iterations` is not a positive integer.
    pub fn from_properties(name: &str, properties: &Value) -> Result<Self, ValidationError> {
        let field = |key: &str| {
            properties
                .get(SKILL_NODE_TYPE)
                .and_then(|bucket| bucket.get(key))
                .or_else(|| properties.get(key))
                .filter(|v| !v.is_null())
        };

        let description = optional_string(field("description"), "description")?;
        let exclusion = optional_string(field("exclusion"), "exclusion")?
            .as_deref()
            .and_then(normalize_exclusion);
        let tool_whitelist = string_list(field("tool_whitelist"), "tool_whitelist")?;
        let node_types = string_list(field("node_types"), "node_types")?;
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
            name: name.to_string(),
            description: description.unwrap_or_default(),
            exclusion,
            tool_whitelist,
            max_iterations,
            node_types,
        })
    }

    /// The skill's config as flat properties — the shape a create or update
    /// writes, which the store hoists under `properties.skill.*`.
    ///
    /// `exclusion` and `node_types` are omitted when unset rather than
    /// written empty.
    pub fn properties(&self) -> Value {
        let mut props = Map::new();
        props.insert("description".to_string(), json!(self.description));
        if let Some(exclusion) = &self.exclusion {
            props.insert("exclusion".to_string(), json!(exclusion));
        }
        props.insert("tool_whitelist".to_string(), json!(self.tool_whitelist));
        props.insert("max_iterations".to_string(), json!(self.max_iterations));
        if !self.node_types.is_empty() {
            props.insert("node_types".to_string(), json!(self.node_types));
        }
        Value::Object(props)
    }

    /// A new skill [`Node`] carrying this config, with a fresh id.
    pub fn into_node(self) -> Node {
        let properties = self.properties();
        Node::new(SKILL_NODE_TYPE.to_string(), self.name, properties)
    }
}

fn normalize_exclusion(exclusion: &str) -> Option<String> {
    let trimmed = exclusion.trim();
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

    fn sample() -> SkillNode {
        SkillNode::new(
            "Graph Editing",
            "Update a record",
            &["update_node", "get_node"],
            3,
        )
        .with_exclusion("Delete records")
        .with_node_types(&["invoice"])
    }

    #[test]
    fn round_trips_through_a_node() {
        let skill = sample();
        let node = skill.clone().into_node();
        assert_eq!(node.node_type, "skill");
        assert_eq!(node.content, "Graph Editing");
        assert_eq!(SkillNode::from_node(&node).unwrap(), skill);
    }

    #[test]
    fn round_trips_through_the_hoisted_storage_shape() {
        let skill = sample();
        let mut node = skill.clone().into_node();
        node.properties = json!({ "skill": skill.properties() });
        assert_eq!(SkillNode::from_node(&node).unwrap(), skill);
    }

    #[test]
    fn minimal_skill_round_trips_without_optional_keys() {
        let skill = SkillNode::new("Research", "Search", &["search_nodes"], 4);
        let props = skill.properties();
        assert!(props.get("exclusion").is_none());
        assert!(props.get("node_types").is_none());
        assert_eq!(
            SkillNode::from_properties("Research", &props).unwrap(),
            skill
        );
    }

    #[test]
    fn hoisted_field_wins_over_flat_per_field() {
        // A flat patch applied on top of a hoisted node leaves both shapes in
        // one bag; the bucket is the stored value.
        let props = json!({
            "max_iterations": 2,
            "tool_whitelist": ["get_node"],
            "skill": { "max_iterations": 4, "description": "d" },
        });
        let skill = SkillNode::from_properties("s", &props).unwrap();
        assert_eq!(skill.max_iterations, 4);
        assert_eq!(skill.description, "d");
        // Absent from the bucket, so read from the flat level.
        assert_eq!(skill.tool_whitelist, vec!["get_node"]);
    }

    #[test]
    fn null_skill_bucket_falls_back_to_flat() {
        let props = json!({ "skill": null, "description": "d", "tool_whitelist": ["x"] });
        let skill = SkillNode::from_properties("s", &props).unwrap();
        assert_eq!(skill.description, "d");
        assert_eq!(skill.tool_whitelist, vec!["x"]);
    }

    #[test]
    fn absent_and_null_fields_take_defaults() {
        let skill = SkillNode::from_properties(
            "s",
            &json!({ "skill": { "exclusion": null, "max_iterations": null } }),
        )
        .unwrap();
        assert_eq!(
            skill,
            SkillNode::new("s", "", &[], DEFAULT_SKILL_MAX_ITERATIONS)
        );
    }

    #[test]
    fn blank_exclusion_is_none() {
        let skill =
            SkillNode::from_properties("s", &json!({ "skill": { "exclusion": "   " } })).unwrap();
        assert_eq!(skill.exclusion, None);
        assert_eq!(
            SkillNode::new("s", "", &[], 1)
                .with_exclusion(" ")
                .exclusion,
            None
        );
    }

    #[test]
    fn exclusion_is_trimmed() {
        let skill =
            SkillNode::from_properties("s", &json!({ "exclusion": "  Delete them. " })).unwrap();
        assert_eq!(skill.exclusion.as_deref(), Some("Delete them."));
    }

    #[test]
    fn wrong_typed_fields_are_rejected() {
        for (props, message) in [
            (json!({ "description": 5 }), "description must be a string"),
            (json!({ "exclusion": [] }), "exclusion must be a string"),
            (
                json!({ "tool_whitelist": "get_node" }),
                "tool_whitelist must be an array",
            ),
            (
                json!({ "tool_whitelist": [1] }),
                "tool_whitelist items must be strings",
            ),
            (json!({ "node_types": {} }), "node_types must be an array"),
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
            match SkillNode::from_properties("s", &props) {
                Err(ValidationError::InvalidProperties(m)) => assert_eq!(m, message, "{props}"),
                other => panic!("expected InvalidProperties for {props}, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_a_non_skill_node() {
        let node = Node::new("text".to_string(), "x".to_string(), json!({}));
        assert!(matches!(
            SkillNode::from_node(&node),
            Err(ValidationError::InvalidNodeType(_))
        ));
    }
}
