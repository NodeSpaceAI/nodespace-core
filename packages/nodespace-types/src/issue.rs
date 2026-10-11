use serde::{Deserialize, Serialize};

use crate::helpers::deserialize_clearable;
use crate::node::NodeEnvelope;
use crate::priority::Priority;
use crate::schema::LinkValue;
use crate::task::{flexible_date, TaskNodeUpdate, TaskStatus};

/// Where an issue stands (ADR-098).
///
/// An issue has every status a task has, and two of its own for review:
/// `changes_requested` and `ready_to_merge`. Any other string is a status a
/// user added. Read at `task` scope, each of the two reads as the task status
/// [`Self::task_status`] names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename_all = "snake_case"))]
pub enum IssueStatus {
    #[default]
    Open,
    InProgress,
    InReview,
    /// Reviewed, with findings to fix.
    ChangesRequested,
    /// Passed review, waiting to land.
    ReadyToMerge,
    Done,
    Cancelled,
    // Serialized as the bare string, like the core values.
    #[cfg_attr(feature = "ts", ts(untagged))]
    User(String),
}

impl IssueStatus {
    /// The statuses an issue adds to a task's.
    pub const OWN: [Self; 2] = [Self::ChangesRequested, Self::ReadyToMerge];

    /// The core statuses, in workflow order: every variant but the
    /// user-defined one.
    pub const CORE: [Self; 7] = [
        Self::Open,
        Self::InProgress,
        Self::InReview,
        Self::ChangesRequested,
        Self::ReadyToMerge,
        Self::Done,
        Self::Cancelled,
    ];

    /// The status a stored or wire string names. Every string is one: a
    /// value outside the core statuses is a user-defined status.
    pub fn from_value(s: &str) -> Self {
        match s {
            "changes_requested" => Self::ChangesRequested,
            "ready_to_merge" => Self::ReadyToMerge,
            other => TaskStatus::from_value(other).into(),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Open => TaskStatus::Open.as_str(),
            Self::InProgress => TaskStatus::InProgress.as_str(),
            Self::InReview => TaskStatus::InReview.as_str(),
            Self::ChangesRequested => "changes_requested",
            Self::ReadyToMerge => "ready_to_merge",
            Self::Done => TaskStatus::Done.as_str(),
            Self::Cancelled => TaskStatus::Cancelled.as_str(),
            Self::User(s) => s.as_str(),
        }
    }

    /// The name shown for one of the issue's own statuses. `None` for a
    /// status a task has too, whose label is the task schema's.
    pub fn own_label(&self) -> Option<&'static str> {
        match self {
            Self::ChangesRequested => Some("Changes Requested"),
            Self::ReadyToMerge => Some("Ready to Merge"),
            Self::Open
            | Self::InProgress
            | Self::InReview
            | Self::Done
            | Self::Cancelled
            | Self::User(_) => None,
        }
    }

    /// What this status reads as at `task` scope: itself for a status a task
    /// has, and the task status it maps to for one of the issue's own. This
    /// is the one definition of the mapping; the issue schema's `maps_to`
    /// values are built from it.
    pub fn task_status(&self) -> TaskStatus {
        match self {
            Self::Open => TaskStatus::Open,
            Self::InProgress | Self::ChangesRequested => TaskStatus::InProgress,
            Self::InReview | Self::ReadyToMerge => TaskStatus::InReview,
            Self::Done => TaskStatus::Done,
            Self::Cancelled => TaskStatus::Cancelled,
            Self::User(s) => TaskStatus::User(s.clone()),
        }
    }

    /// Whether this is one of the core statuses rather than a user-defined
    /// one.
    pub fn is_core(&self) -> bool {
        !matches!(self, Self::User(_))
    }
}

impl From<TaskStatus> for IssueStatus {
    fn from(status: TaskStatus) -> Self {
        match status {
            TaskStatus::Open => Self::Open,
            TaskStatus::InProgress => Self::InProgress,
            TaskStatus::InReview => Self::InReview,
            TaskStatus::Done => Self::Done,
            TaskStatus::Cancelled => Self::Cancelled,
            TaskStatus::User(s) => Self::User(s),
        }
    }
}

impl Serialize for IssueStatus {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for IssueStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|s| Self::from_value(&s))
    }
}

/// Wire shape for issue nodes sent to the frontend: a task's fields and the
/// issue's own (ADR-098).
///
/// Produced by `node_to_typed_value` for a node whose type is exactly
/// `issue`. Fields map directly to the TypeScript `IssueNode` interface.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct IssueNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    pub status: IssueStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// The pull request that delivers the issue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<LinkValue>,
    /// The commits that deliver the issue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits: Option<Vec<LinkValue>>,
}

/// Partial update for an issue's core fields, received from the frontend.
///
/// The fields of a [`TaskNodeUpdate`], with the issue's status vocabulary,
/// and the issue's own: `pull_request`, and `commits`, which is replaced
/// whole. `status` has no clear path (the schema requires it); the other
/// fields are tri-state: absent leaves the field unchanged, `null` clears it,
/// and a value sets it.
///
/// `content` is an envelope field and extension fields (`custom:…`) live in
/// `properties`; both are written through the generic node update.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssueNodeUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<IssueStatus>,
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

impl IssueNodeUpdate {
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

    /// The flat, bare-key properties patch this update writes; a cleared
    /// field is written as `null`. The service layer moves each key into the
    /// bucket of the schema that declares it: `task` for the inherited fields
    /// and `issue` for the status and the issue's own.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        // The inherited fields are written exactly as a task's update writes
        // them; the status is written by its issue name, not its task one.
        let inherited = TaskNodeUpdate {
            status: None,
            priority: self.priority.clone(),
            due_date: self.due_date.clone(),
            started_at: self.started_at.clone(),
            completed_at: self.completed_at.clone(),
        };
        let serde_json::Value::Object(mut patch) = inherited.to_properties_patch() else {
            unreachable!("a task update's patch is an object");
        };
        if let Some(status) = &self.status {
            patch.insert("status".to_string(), serde_json::json!(status));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_has_a_tasks_statuses_and_two_of_its_own() {
        let names: Vec<&str> = IssueStatus::CORE.iter().map(IssueStatus::as_str).collect();
        assert_eq!(
            names,
            [
                "open",
                "in_progress",
                "in_review",
                "changes_requested",
                "ready_to_merge",
                "done",
                "cancelled"
            ]
        );
        // Every task status is an issue status of the same name, and reads
        // as itself at task scope.
        for status in TaskStatus::CORE {
            let issue = IssueStatus::from(status.clone());
            assert_eq!(issue.as_str(), status.as_str());
            assert_eq!(IssueStatus::from_value(status.as_str()), issue);
            assert_eq!(issue.task_status(), status);
            assert_eq!(issue.own_label(), None);
            assert!(!IssueStatus::OWN.contains(&issue));
        }
        for status in IssueStatus::OWN {
            assert!(status.is_core());
            assert!(status.own_label().is_some());
            assert_eq!(IssueStatus::from_value(status.as_str()), status);
            // A task does not have it: it is a user value there.
            assert!(!TaskStatus::from_value(status.as_str()).is_core());
        }
    }

    #[test]
    fn an_own_status_reads_as_a_core_task_status() {
        assert_eq!(
            IssueStatus::ChangesRequested.task_status(),
            TaskStatus::InProgress
        );
        assert_eq!(
            IssueStatus::ReadyToMerge.task_status(),
            TaskStatus::InReview
        );
        for status in IssueStatus::OWN {
            assert!(status.task_status().is_core());
        }
    }

    #[test]
    fn a_user_status_is_kept() {
        let status = IssueStatus::from_value("triage");
        assert_eq!(status, IssueStatus::User("triage".to_string()));
        assert!(!status.is_core());
        assert_eq!(serde_json::to_value(&status).unwrap(), "triage");
        assert_eq!(status.task_status(), TaskStatus::User("triage".to_string()));
    }

    #[test]
    fn the_update_takes_an_issue_status_and_writes_it_by_name() {
        let update: IssueNodeUpdate =
            serde_json::from_str(r#"{"status": "ready_to_merge", "priority": null}"#).unwrap();
        assert_eq!(update.status, Some(IssueStatus::ReadyToMerge));
        assert_eq!(update.priority, Some(None));
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "status": "ready_to_merge", "priority": null })
        );
    }

    #[test]
    fn inherited_dates_normalize_as_a_tasks_do() {
        let update: IssueNodeUpdate = serde_json::from_str(
            r#"{"dueDate": "2026-03-01T09:00:00Z", "startedAt": null, "completedAt": "2026-03-04"}"#,
        )
        .unwrap();
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({
                "due_date": "2026-03-01",
                "started_at": null,
                "completed_at": "2026-03-04"
            })
        );
    }

    #[test]
    fn links_are_set_cleared_and_replaced_whole() {
        let update: IssueNodeUpdate = serde_json::from_str(
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

        let cleared: IssueNodeUpdate =
            serde_json::from_str(r#"{"pullRequest": null, "commits": null}"#).unwrap();
        assert_eq!(cleared.pull_request, Some(None));
        assert_eq!(cleared.commits, Some(None));
        assert_eq!(
            cleared.to_properties_patch(),
            serde_json::json!({ "pull_request": null, "commits": null })
        );

        // A link is a closed shape: an extra key is refused, not dropped.
        assert!(serde_json::from_str::<IssueNodeUpdate>(
            r#"{"pullRequest": {"title": "PR", "url": "https://example.com", "state": "open"}}"#
        )
        .is_err());
    }

    #[test]
    fn an_empty_update_is_empty_and_an_unknown_key_is_rejected() {
        let update: IssueNodeUpdate = serde_json::from_str("{}").unwrap();
        assert!(update.is_empty());
        for json in [
            r#"{"content": "Renamed"}"#,
            r#"{"repository": {"title": "core", "url": "https://example.com/a/core"}}"#,
        ] {
            assert!(
                serde_json::from_str::<IssueNodeUpdate>(json).is_err(),
                "{json} must not deserialize as an IssueNodeUpdate"
            );
        }
    }
}
