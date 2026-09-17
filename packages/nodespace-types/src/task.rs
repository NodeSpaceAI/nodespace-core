use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::helpers::is_active_lifecycle;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TaskStatus {
    #[default]
    Open,
    InProgress,
    Done,
    Cancelled,
    User(String),
}

impl FromStr for TaskStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "open" => Self::Open,
            "in_progress" => Self::InProgress,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            other => Self::User(other.to_string()),
        })
    }
}

impl TaskStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::User(s) => s.as_str(),
        }
    }
}

impl Serialize for TaskStatus {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TaskStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::from_str(&s).unwrap())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TaskPriority {
    Highest,
    High,
    #[default]
    Medium,
    Low,
    Lowest,
    User(String),
}

impl FromStr for TaskPriority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "highest" => Self::Highest,
            "high" => Self::High,
            "medium" => Self::Medium,
            "low" => Self::Low,
            "lowest" => Self::Lowest,
            other => Self::User(other.to_string()),
        })
    }
}

impl TaskPriority {
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

impl Serialize for TaskPriority {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TaskPriority {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::from_str(&s).unwrap())
    }
}

/// Wire shape for task nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for `node_type == "task"`. Fields map
/// directly to the TypeScript `TaskNode` interface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskNode {
    pub id: String,
    #[serde(rename = "nodeType")]
    pub node_type: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub properties: serde_json::Value,
    #[serde(default, skip_serializing_if = "is_active_lifecycle")]
    pub lifecycle_status: String,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<TaskPriority>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

/// Partial update for task-specific properties, received from the frontend.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskNodeUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Option<TaskPriority>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub due_date: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub started_at: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub completed_at: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Flexible date deserializer: accepts ISO8601 full timestamps, date-only
/// "YYYY-MM-DD" strings, and explicit JSON null (which clears the field).
///
/// The `Option<Option<T>>` pattern distinguishes three cases:
/// - Field absent from JSON → `None` (don't change)
/// - Field present as `null` → `Some(None)` (clear the value)
/// - Field present as a string → `Some(Some(dt))` (set to this value)
///
/// Note: The old `src-tauri/types.rs` mirror used `Option<Option<String>>` as
/// the intermediate which caused JSON `null` to map to `None` (no-op) instead
/// of `Some(None)` (clear). This is the corrected implementation.
pub(crate) mod flexible_date {
    use chrono::{DateTime, NaiveDate, Utc};
    use serde::{Deserialize, Deserializer};

    pub fn deserialize_with_null<'de, D>(
        deserializer: D,
    ) -> Result<Option<Option<String>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            None => Ok(Some(None)),
            Some(s) => normalize_to_date(&s).map_err(serde::de::Error::custom),
        }
    }

    fn normalize_to_date(s: &str) -> Result<Option<Option<String>>, String> {
        if NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
            return Ok(Some(Some(s.to_string())));
        }
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Ok(Some(Some(dt.format("%Y-%m-%d").to_string())));
        }
        if let Ok(dt) = s.parse::<DateTime<Utc>>() {
            return Ok(Some(Some(dt.format("%Y-%m-%d").to_string())));
        }
        Err(format!(
            "Invalid date format: '{}'. Expected YYYY-MM-DD or ISO8601",
            s
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This crate carries its own `TaskPriority` copy because it cannot depend
    /// on `nodespace-core`, and it is the one that serializes onto the wire.
    /// The core-side bidirectional drift test guards the *core* copy against
    /// `task.priority`'s schema seed; nothing links this copy to either, so a
    /// value added to the scale but missed here would degrade to
    /// `TaskPriority::User(_)` at the frontend boundary while every suite
    /// stayed green. Pinning the literals here is the cheapest available
    /// stand-in for that missing link.
    #[test]
    fn task_priority_core_scale_parses_to_named_variants() {
        let scale = [
            ("highest", TaskPriority::Highest),
            ("high", TaskPriority::High),
            ("medium", TaskPriority::Medium),
            ("low", TaskPriority::Low),
            ("lowest", TaskPriority::Lowest),
        ];

        for (literal, expected) in &scale {
            let parsed: TaskPriority = literal.parse().expect("from_str is infallible");
            assert_eq!(
                parsed, *expected,
                "'{}' must parse to the named TaskPriority variant, not TaskPriority::User(_) — \
                 keep this copy in step with task.priority's core_values in core_schemas.rs.",
                literal
            );
            assert!(
                !matches!(parsed, TaskPriority::User(_)),
                "'{}' fell through to the user-defined catch-all",
                literal
            );
            assert_eq!(
                parsed.as_str(),
                *literal,
                "'{}' must round-trip through as_str()",
                literal
            );
        }
    }

    #[test]
    fn task_node_update_null_clears_due_date() {
        let json = r#"{"dueDate": null}"#;
        let update: TaskNodeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.due_date, Some(None));
    }

    #[test]
    fn task_node_update_absent_due_date_is_none() {
        let json = r#"{}"#;
        let update: TaskNodeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.due_date, None);
    }

    #[test]
    fn task_node_update_iso8601_due_date_normalizes_to_date_only() {
        let json = r#"{"dueDate": "2025-06-15T00:00:00Z"}"#;
        let update: TaskNodeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.due_date, Some(Some("2025-06-15".to_string())));
    }

    #[test]
    fn task_node_update_date_only_due_date_passes_through() {
        let json = r#"{"dueDate": "2025-06-15"}"#;
        let update: TaskNodeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.due_date, Some(Some("2025-06-15".to_string())));
    }

    #[test]
    fn task_node_update_iso8601_started_at_normalizes() {
        let json = r#"{"startedAt": "2025-06-15T08:30:00Z"}"#;
        let update: TaskNodeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.started_at, Some(Some("2025-06-15".to_string())));
    }

    #[test]
    fn task_node_update_iso8601_completed_at_normalizes() {
        let json = r#"{"completedAt": "2025-06-16T23:59:59Z"}"#;
        let update: TaskNodeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.completed_at, Some(Some("2025-06-16".to_string())));
    }

    #[test]
    fn task_node_update_null_started_at_clears() {
        let json = r#"{"startedAt": null}"#;
        let update: TaskNodeUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.started_at, Some(None));
    }
}
