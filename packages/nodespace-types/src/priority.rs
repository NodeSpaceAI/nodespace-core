//! Priority scale shared by the core node types that carry a `priority` field
//!
//! `task.priority` and `project.priority` use one vocabulary, so one enum
//! backs both: their typed wire fields, their `core_values`, and the drift
//! guard in core's `core_schemas.rs` that keeps the two in step. A sort by
//! priority follows the order the schema declares the values in, like a sort
//! by any other enum.

use serde::{Deserialize, Serialize};

use crate::core_type::CoreNodeType;

/// The relative-urgency scale of the core `task` and `project` types.
///
/// Core priorities are strongly typed; a priority a user added to the schema
/// (`user_values`, e.g. "critical") is `User(String)`. There is no default:
/// a node without a priority has none.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename_all = "snake_case"))]
pub enum Priority {
    Highest,
    High,
    Medium,
    Low,
    Lowest,
    /// User-defined priority (extended via schema). Serialized as the bare
    /// string, like the core values.
    #[cfg_attr(feature = "ts", ts(untagged))]
    User(String),
}

impl Priority {
    /// The priority a stored or wire string names. Every string is one: a
    /// value outside the core scale is a user-defined priority.
    pub fn from_value(s: &str) -> Self {
        match s {
            "highest" => Self::Highest,
            "high" => Self::High,
            "medium" => Self::Medium,
            "low" => Self::Low,
            "lowest" => Self::Lowest,
            other => Self::User(other.to_string()),
        }
    }

    /// The core node types whose `priority` field uses this scale.
    ///
    /// Every type listed here must declare `priority` with exactly this
    /// enum's core values, in this enum's order, which
    /// `test_priority_variants_match_core_values_bidirectionally` in core's
    /// `core_schemas.rs` checks for each of them.
    pub const NODE_TYPES: [CoreNodeType; 2] = [CoreNodeType::Task, CoreNodeType::Project];

    /// Whether `node_type`'s `priority` field uses this scale.
    pub fn applies_to(node_type: &str) -> bool {
        CoreNodeType::from_id(node_type).is_some_and(|core| Self::NODE_TYPES.contains(&core))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Highest => "highest",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Lowest => "lowest",
            Self::User(s) => s.as_str(),
        }
    }
}

impl Serialize for Priority {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|s| Self::from_value(&s))
    }
}

/// Read a stored `priority` value. Anything but a string is no priority.
pub(crate) fn priority_prop(props: &serde_json::Value) -> Option<Priority> {
    props.get("priority")?.as_str().map(Priority::from_value)
}
