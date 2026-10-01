use serde::{Deserialize, Serialize};

use crate::helpers::deserialize_clearable;
use crate::node::NodeEnvelope;
use crate::priority::Priority;
use crate::task::flexible_date;

/// Where a project stands.
///
/// The core statuses are strongly typed; a status a user added to the schema
/// (`user_values`) is `User(String)`. The default, [`Self::Planning`], is the
/// project schema's declared default: what a project with no stored status
/// has.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(rename_all = "snake_case"))]
pub enum ProjectStatus {
    #[default]
    Planning,
    Active,
    Completed,
    Cancelled,
    // Serialized as the bare string, like the core values.
    #[cfg_attr(feature = "ts", ts(untagged))]
    User(String),
}

impl ProjectStatus {
    /// The status a stored or wire string names. Every string is one: a
    /// value outside the core statuses is a user-defined status.
    pub fn from_value(s: &str) -> Self {
        match s {
            "planning" => Self::Planning,
            "active" => Self::Active,
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            other => Self::User(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Planning => "planning",
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::User(s) => s.as_str(),
        }
    }

    /// Whether this is one of the core statuses rather than a user-defined
    /// one.
    pub fn is_core(&self) -> bool {
        !matches!(self, Self::User(_))
    }
}

impl Serialize for ProjectStatus {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProjectStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|s| Self::from_value(&s))
    }
}

/// Wire shape for project nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for `node_type == "project"`. The project
/// schema's core fields (`status`, `priority`, `start_date`, `end_date`) are
/// promoted to the top level; they map directly to the TypeScript
/// `ProjectNode` interface.
///
/// `status` is the project's own vocabulary; `priority` is the scale `task`
/// shares. Both are user-extensible, and the service layer validates a write
/// against the schema's declared values.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct ProjectNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    pub status: ProjectStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<String>,
}

/// Partial update for a project's core fields, received from the frontend.
///
/// `status` has no clear path (the schema requires it); the other fields are
/// tri-state: absent leaves the field unchanged, `null` clears it, and a value
/// sets it. Dates accept `YYYY-MM-DD` or RFC 3339 and are stored as
/// `YYYY-MM-DD`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectNodeUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ProjectStatus>,
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
    pub start_date: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub end_date: Option<Option<String>>,
}

impl ProjectNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self.status.is_none()
            && self.priority.is_none()
            && self.start_date.is_none()
            && self.end_date.is_none()
    }

    /// The flat, bare-key properties patch this update writes (`{"status":
    /// "active"}`); a cleared field is written as `null`. The service layer
    /// moves the keys into the `project` storage bucket.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::new();
        if let Some(status) = &self.status {
            patch.insert("status".to_string(), serde_json::json!(status));
        }
        if let Some(priority) = &self.priority {
            patch.insert("priority".to_string(), serde_json::json!(priority));
        }
        for (key, value) in [
            ("start_date", &self.start_date),
            ("end_date", &self.end_date),
        ] {
            if let Some(value) = value {
                patch.insert(key.to_string(), serde_json::json!(value));
            }
        }
        serde_json::Value::Object(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_normalize_and_null_clears() {
        let update: ProjectNodeUpdate = serde_json::from_str(
            r#"{"startDate": "2026-03-01T09:00:00Z", "endDate": null, "priority": null}"#,
        )
        .unwrap();
        assert_eq!(update.start_date, Some(Some("2026-03-01".to_string())));
        assert_eq!(update.end_date, Some(None));
        assert_eq!(update.priority, Some(None));
        assert_eq!(update.status, None);
    }

    #[test]
    fn patch_carries_only_the_fields_the_update_names() {
        let update = ProjectNodeUpdate {
            status: Some(ProjectStatus::Active),
            end_date: Some(None),
            ..Default::default()
        };
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "status": "active", "end_date": null })
        );
    }

    /// The update carries the project schema's fields only; naming anything
    /// else is an error rather than a silently dropped write.
    #[test]
    fn an_unknown_key_is_rejected() {
        assert!(serde_json::from_str::<ProjectNodeUpdate>(r#"{"content": "Apollo"}"#).is_err());
    }

    #[test]
    fn priority_is_the_shared_scale() {
        let update: ProjectNodeUpdate = serde_json::from_str(r#"{"priority": "highest"}"#).unwrap();
        assert_eq!(update.priority, Some(Some(Priority::Highest)));
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "priority": "highest" })
        );
    }

    #[test]
    fn invalid_date_is_rejected() {
        let result: Result<ProjectNodeUpdate, _> =
            serde_json::from_str(r#"{"startDate": "next tuesday"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn status_is_typed_and_keeps_a_user_value() {
        let core: ProjectNodeUpdate = serde_json::from_str(r#"{"status": "completed"}"#).unwrap();
        assert_eq!(core.status, Some(ProjectStatus::Completed));
        let user: ProjectNodeUpdate = serde_json::from_str(r#"{"status": "on_hold"}"#).unwrap();
        assert_eq!(
            user.status,
            Some(ProjectStatus::User("on_hold".to_string()))
        );
        assert_eq!(
            user.to_properties_patch(),
            serde_json::json!({ "status": "on_hold" })
        );
        assert_eq!(ProjectStatus::default().as_str(), "planning");
        assert!(!ProjectStatus::from_value("on_hold").is_core());
    }
}
