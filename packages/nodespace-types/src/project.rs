use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::helpers::{deserialize_clearable, is_active_lifecycle};
use crate::task::flexible_date;

/// Default `status` for a project that has none stored, matching the project
/// schema's declared default.
pub const DEFAULT_PROJECT_STATUS: &str = "planning";

/// Wire shape for project nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for `node_type == "project"`. The project
/// schema's core fields (`status`, `priority`, `start_date`, `end_date`) are
/// promoted to the top level; they map directly to the TypeScript
/// `ProjectNode` interface.
///
/// `status` and `priority` stay strings rather than enums: both are
/// user-extensible (`user_values`), and the schema, not this struct, owns the
/// vocabulary — the service layer validates writes against it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectNode {
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
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
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
#[serde(rename_all = "camelCase")]
pub struct ProjectNodeUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub priority: Option<Option<String>>,
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
        for (key, value) in [
            ("priority", &self.priority),
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
            status: Some("active".to_string()),
            end_date: Some(None),
            ..Default::default()
        };
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "status": "active", "end_date": null })
        );
    }

    #[test]
    fn invalid_date_is_rejected() {
        let result: Result<ProjectNodeUpdate, _> =
            serde_json::from_str(r#"{"startDate": "next tuesday"}"#);
        assert!(result.is_err());
    }
}
