use serde::{Deserialize, Serialize};

use crate::helpers::deserialize_clearable;
use crate::node::NodeEnvelope;
use crate::priority::Priority;
use crate::schema::LinkValue;

/// Where a task stands.
///
/// The five core statuses are named, in workflow order; any other string is a
/// status a user added to the task schema. `in_review` is work that is
/// finished and waiting on review, so a task in review has been started.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename_all = "snake_case"))]
pub enum TaskStatus {
    #[default]
    Open,
    InProgress,
    InReview,
    Done,
    Cancelled,
    // Serialized as the bare string, like the core values.
    #[cfg_attr(feature = "ts", ts(untagged))]
    User(String),
}

impl TaskStatus {
    /// The core statuses: every variant but the user-defined one.
    pub const CORE: [Self; 5] = [
        Self::Open,
        Self::InProgress,
        Self::InReview,
        Self::Done,
        Self::Cancelled,
    ];

    /// The status a stored or wire string names. Every string is one: a
    /// value outside the core statuses is a user-defined status.
    pub fn from_value(s: &str) -> Self {
        match s {
            "open" => Self::Open,
            "in_progress" => Self::InProgress,
            "in_review" => Self::InReview,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            other => Self::User(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::InReview => "in_review",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::User(s) => s.as_str(),
        }
    }

    /// Whether this is one of the core statuses rather than a user-defined
    /// one.
    pub fn is_core(&self) -> bool {
        !matches!(self, Self::User(_))
    }

    /// Whether a task with this status has been started: it is being worked
    /// on, or the work is finished and in review.
    pub fn is_started(&self) -> bool {
        matches!(self, Self::InProgress | Self::InReview)
    }
}

impl Serialize for TaskStatus {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TaskStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|s| Self::from_value(&s))
    }
}

/// Wire shape for task nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for `node_type == "task"`. Fields map
/// directly to the TypeScript `TaskNode` interface.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct TaskNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// The pull request that delivered the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<LinkValue>,
    /// The commits that delivered the task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits: Option<Vec<LinkValue>>,
}

/// Partial update for a task's core fields, received from the frontend.
///
/// `status` has no clear path (the schema requires it); the other fields are
/// tri-state: absent leaves the field unchanged, `null` clears it, and a value
/// sets it. Dates accept `YYYY-MM-DD` or RFC 3339 and are stored as
/// `YYYY-MM-DD`. `commits` is replaced whole.
///
/// The update carries the task schema's fields and nothing else. `content` is
/// an envelope field and extension fields (`custom:…`) live in `properties`;
/// both are written through the generic node update.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskNodeUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub priority: Option<Option<Priority>>,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub pull_request: Option<Option<LinkValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub commits: Option<Option<Vec<LinkValue>>>,
}

impl TaskNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self.status.is_none()
            && self.priority.is_none()
            && self.due_date.is_none()
            && self.started_at.is_none()
            && self.completed_at.is_none()
            && self.pull_request.is_none()
            && self.commits.is_none()
    }

    /// The flat, bare-key properties patch this update writes (`{"status":
    /// "done"}`); a cleared field is written as `null`. The service layer
    /// moves the keys into the `task` storage bucket.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::new();
        if let Some(status) = &self.status {
            patch.insert("status".to_string(), serde_json::json!(status));
        }
        if let Some(priority) = &self.priority {
            patch.insert("priority".to_string(), serde_json::json!(priority));
        }
        for (key, value) in [
            ("due_date", &self.due_date),
            ("started_at", &self.started_at),
            ("completed_at", &self.completed_at),
        ] {
            if let Some(value) = value {
                patch.insert(key.to_string(), serde_json::json!(value));
            }
        }
        if let Some(pull_request) = &self.pull_request {
            patch.insert("pull_request".to_string(), serde_json::json!(pull_request));
        }
        if let Some(commits) = &self.commits {
            patch.insert("commits".to_string(), serde_json::json!(commits));
        }
        serde_json::Value::Object(patch)
    }
}

/// Flexible date deserializer: accepts ISO8601 full timestamps, date-only
/// "YYYY-MM-DD" strings, and explicit JSON null (which clears the field).
///
/// The `Option<Option<T>>` pattern distinguishes three cases:
/// - Field absent from JSON → `None` (don't change)
/// - Field present as `null` → `Some(None)` (clear the value)
/// - Field present as a string → `Some(Some(dt))` (set to this value)
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

    #[test]
    fn task_node_update_null_clears_priority() {
        let update: TaskNodeUpdate = serde_json::from_str(r#"{"priority": null}"#).unwrap();
        assert_eq!(update.priority, Some(None));
        assert!(!update.is_empty());
    }

    #[test]
    fn task_node_update_absent_priority_is_unchanged() {
        let update: TaskNodeUpdate = serde_json::from_str(r#"{"status": "done"}"#).unwrap();
        assert_eq!(update.priority, None);
        assert_eq!(update.status, Some(TaskStatus::Done));
    }

    #[test]
    fn task_node_update_sets_core_and_user_priorities() {
        let update: TaskNodeUpdate = serde_json::from_str(r#"{"priority": "high"}"#).unwrap();
        assert_eq!(update.priority, Some(Some(Priority::High)));
        let update: TaskNodeUpdate = serde_json::from_str(r#"{"priority": "urgent"}"#).unwrap();
        assert_eq!(
            update.priority,
            Some(Some(Priority::User("urgent".to_string())))
        );
    }

    /// `content` is an envelope field and `properties` holds extension
    /// fields; neither is part of the typed update, and naming one is an
    /// error rather than a silently dropped write.
    #[test]
    fn task_node_update_rejects_content_and_properties() {
        for json in [
            r#"{"content": "Renamed"}"#,
            r#"{"status": "done", "properties": {"custom:estimate": 3}}"#,
        ] {
            assert!(
                serde_json::from_str::<TaskNodeUpdate>(json).is_err(),
                "{json} must not deserialize as a TaskNodeUpdate"
            );
        }
    }

    #[test]
    fn empty_task_node_update_is_empty() {
        let update: TaskNodeUpdate = serde_json::from_str("{}").unwrap();
        assert!(update.is_empty());
    }

    #[test]
    fn patch_carries_only_the_fields_the_update_names() {
        let update = TaskNodeUpdate {
            status: Some(TaskStatus::InProgress),
            priority: Some(None),
            due_date: Some(Some("2026-03-01".to_string())),
            ..Default::default()
        };
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({
                "status": "in_progress",
                "priority": null,
                "due_date": "2026-03-01"
            })
        );
    }

    #[test]
    fn in_review_is_a_core_status_between_in_progress_and_done() {
        let names: Vec<&str> = TaskStatus::CORE.iter().map(TaskStatus::as_str).collect();
        assert_eq!(
            names,
            ["open", "in_progress", "in_review", "done", "cancelled"]
        );
        assert_eq!(TaskStatus::from_value("in_review"), TaskStatus::InReview);
        assert!(TaskStatus::InReview.is_core());
        assert!(TaskStatus::InReview.is_started());
        assert!(TaskStatus::InProgress.is_started());
        for status in [TaskStatus::Open, TaskStatus::Done, TaskStatus::Cancelled] {
            assert!(!status.is_started(), "{status:?}");
        }
    }

    #[test]
    fn links_are_set_cleared_and_replaced_whole() {
        let update: TaskNodeUpdate = serde_json::from_str(
            r#"{"pullRequest": {"title": "PR 7", "url": "https://example.com/pull/7"},
                "commits": [{"title": "abc123", "url": "https://example.com/commit/abc123"}]}"#,
        )
        .unwrap();
        assert!(!update.is_empty());
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({
                "pull_request": { "title": "PR 7", "url": "https://example.com/pull/7" },
                "commits": [{ "title": "abc123", "url": "https://example.com/commit/abc123" }]
            })
        );

        let cleared: TaskNodeUpdate =
            serde_json::from_str(r#"{"pullRequest": null, "commits": null}"#).unwrap();
        assert_eq!(cleared.pull_request, Some(None));
        assert_eq!(cleared.commits, Some(None));
        assert_eq!(
            cleared.to_properties_patch(),
            serde_json::json!({ "pull_request": null, "commits": null })
        );

        // A link is a closed shape: an extra key is refused, not dropped.
        assert!(serde_json::from_str::<TaskNodeUpdate>(
            r#"{"pullRequest": {"title": "PR", "url": "https://example.com", "state": "open"}}"#
        )
        .is_err());
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
