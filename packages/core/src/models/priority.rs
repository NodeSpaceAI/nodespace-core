//! Priority scale shared by the core node types that carry a `priority` field
//!
//! `task.priority` and `project.priority` use one vocabulary, so one enum
//! backs both: their `core_values`, the rank the query service sorts them by,
//! and the drift guard in `core_schemas.rs` that keeps the three in step.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Priority enumeration
///
/// The relative-urgency scale of the core `task` and `project` types.
/// Values use lowercase format for consistency across all layers:
/// - "highest" - Highest priority
/// - "high" - High priority
/// - "medium" - Medium priority (default)
/// - "low" - Low priority
/// - "lowest" - Lowest priority
/// - User-defined priorities via schema extension (e.g., "critical", "urgent")
///
/// Core priorities are strongly typed; user-defined priorities use `User(String)`.
/// This aligns with the schema system's `core_values` / `user_values` model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Priority {
    /// Highest priority
    Highest,
    /// High priority
    High,
    /// Medium priority (default)
    #[default]
    Medium,
    /// Low priority
    Low,
    /// Lowest priority
    Lowest,
    /// User-defined priority (extended via schema)
    User(String),
}

impl FromStr for Priority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "highest" => Ok(Self::Highest),
            "high" => Ok(Self::High),
            "medium" => Ok(Self::Medium),
            "low" => Ok(Self::Low),
            "lowest" => Ok(Self::Lowest),
            // Any other value is treated as user-defined
            other => Ok(Self::User(other.to_string())),
        }
    }
}

impl Priority {
    /// The core node types whose `priority` field uses this scale.
    ///
    /// The query service ranks `priority` by [`Self::rank`] only when the
    /// query targets one of these types; any other type's `priority` (a
    /// user-defined type's bare field, say) is its own vocabulary and sorts as
    /// plain text. Every type listed here must declare `priority` with exactly
    /// this enum's core values, which
    /// `test_priority_variants_match_core_values_bidirectionally` in
    /// `core_schemas.rs` checks for each of them.
    pub const NODE_TYPES: [crate::models::CoreNodeType; 2] = [
        crate::models::CoreNodeType::Task,
        crate::models::CoreNodeType::Project,
    ];

    /// Whether `node_type`'s `priority` field uses this scale.
    pub fn applies_to(node_type: &str) -> bool {
        crate::models::CoreNodeType::from_id(node_type)
            .is_some_and(|core| Self::NODE_TYPES.contains(&core))
    }

    /// Convert priority to string representation
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
    ///
    /// Kept in sync with `core_values` by
    /// `test_priority_variants_match_core_values_bidirectionally`
    /// in `core_schemas.rs`.
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

    /// Check if this is a core (built-in) priority
    pub fn is_core(&self) -> bool {
        !matches!(self, Self::User(_))
    }

    /// Check if this is a user-defined priority
    pub fn is_user_defined(&self) -> bool {
        matches!(self, Self::User(_))
    }
}

impl Serialize for Priority {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Priority {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_str(&s).unwrap()) // from_str never fails: unknown strings map to User(_)
    }
}
