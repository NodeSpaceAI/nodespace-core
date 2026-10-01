//! Priority scale shared by the core node types that carry a `priority` field
//!
//! `task.priority` and `project.priority` use one vocabulary, so one enum
//! backs both: their typed wire fields, their `core_values`, the rank the
//! query service sorts them by, and the drift guard in core's
//! `core_schemas.rs` that keeps the three in step.

use serde::{Deserialize, Serialize};

use crate::core_type::CoreNodeType;

/// The relative-urgency scale of the core `task` and `project` types.
///
/// Core priorities are strongly typed; a priority a user added to the schema
/// (`user_values`, e.g. "critical") is `User(String)`. There is no default:
/// a node without a priority has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Priority {
    Highest,
    High,
    Medium,
    Low,
    Lowest,
    /// User-defined priority (extended via schema)
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
    /// The query service ranks `priority` by [`Self::rank`] only when the
    /// query targets one of these types; any other type's `priority` (a
    /// user-defined type's bare field, say) is its own vocabulary and sorts as
    /// plain text. Every type listed here must declare `priority` with exactly
    /// this enum's core values, which
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

    /// Rank of this priority for ordering purposes (0 = most urgent)
    ///
    /// Ascending rank yields highest, high, medium, low, lowest — the semantic
    /// urgency order, not the lexicographic one the raw strings would give.
    ///
    /// User-defined priorities all share [`Self::USER_RANK`], one past the core
    /// scale, so they sort after every core value. Since the rank alone cannot
    /// separate two user values, callers must break that tie on the value
    /// string to keep the ordering total; see `QueryService::resolve_order_field`
    /// and `QueryService::compare_priority_values`, which both do exactly that.
    pub fn rank(&self) -> u8 {
        match self {
            Self::Highest => 0,
            Self::High => 1,
            Self::Medium => 2,
            Self::Low => 3,
            Self::Lowest => 4,
            Self::User(_) => Self::USER_RANK,
        }
    }

    /// Rank assigned to every user-defined priority — one past the core scale,
    /// so user values sort after all core values.
    pub const USER_RANK: u8 = 5;

    /// Rank for an absent priority, before the whole scale.
    ///
    /// Signed because it sits below [`Self::Highest`]'s 0; the SQL side needs a
    /// literal it can order against the other ranks, and an absent value sorts
    /// first ascending. This is not a variant of the enum — a missing priority
    /// has no `Priority` at all — so it lives here as the shared constant
    /// both ordering paths rank it by.
    pub const ABSENT_RANK: i8 = -1;
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
