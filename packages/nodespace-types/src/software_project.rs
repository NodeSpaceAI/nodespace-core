use serde::{Deserialize, Serialize};

use crate::helpers::deserialize_clearable;
use crate::node::NodeEnvelope;
use crate::priority::Priority;
use crate::project::{ProjectNodeUpdate, ProjectStatus};
use crate::schema::LinkValue;
use crate::task::flexible_date;

/// Wire shape for software-project nodes sent to the frontend: a project's
/// fields and the two that say where its source is (ADR-098).
///
/// Produced by `node_to_typed_value` for a node whose type is exactly
/// `software-project`. Fields map directly to the TypeScript
/// `SoftwareProjectNode` interface. The status is a project's: a software
/// project adds no status of its own.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct SoftwareProjectNode {
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
    /// The project's source repository. A client binds a checkout to the
    /// project by comparing the checkout's remote with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<LinkValue>,
    /// The folder on this machine that holds the project's checkout. It is
    /// machine-bound: absent on a machine that has not set its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout_path: Option<String>,
}

/// Partial update for a software project's core fields, received from the
/// frontend.
///
/// The fields of a [`ProjectNodeUpdate`] and the software project's own:
/// `repository` and `checkout_path`. `status` has no clear path (the schema
/// requires it); the other fields are tri-state: absent leaves the field
/// unchanged, `null` clears it, and a value sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SoftwareProjectNodeUpdate {
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub repository: Option<Option<LinkValue>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub checkout_path: Option<Option<String>>,
}

impl SoftwareProjectNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self.status.is_none()
            && self.priority.is_none()
            && self.start_date.is_none()
            && self.end_date.is_none()
            && self.repository.is_none()
            && self.checkout_path.is_none()
    }

    /// The flat, bare-key properties patch this update writes; a cleared
    /// field is written as `null`. The service layer moves each key into the
    /// bucket of the schema that declares it: `project` for the inherited
    /// fields and `software-project` for its own.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        // The inherited fields are written exactly as a project's update
        // writes them.
        let inherited = ProjectNodeUpdate {
            status: self.status.clone(),
            priority: self.priority.clone(),
            start_date: self.start_date.clone(),
            end_date: self.end_date.clone(),
        };
        let serde_json::Value::Object(mut patch) = inherited.to_properties_patch() else {
            unreachable!("a project update's patch is an object");
        };
        if let Some(repository) = &self.repository {
            patch.insert("repository".to_string(), serde_json::json!(repository));
        }
        if let Some(checkout_path) = &self.checkout_path {
            patch.insert(
                "checkout_path".to_string(),
                serde_json::json!(checkout_path),
            );
        }
        serde_json::Value::Object(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inherited_fields_are_written_as_a_projects_are() {
        let update: SoftwareProjectNodeUpdate = serde_json::from_str(
            r#"{"status": "active", "priority": "high",
                "startDate": "2026-03-01T09:00:00Z", "endDate": null}"#,
        )
        .unwrap();
        assert_eq!(update.status, Some(ProjectStatus::Active));
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({
                "status": "active",
                "priority": "high",
                "start_date": "2026-03-01",
                "end_date": null
            })
        );
    }

    #[test]
    fn repository_is_set_and_cleared() {
        let update: SoftwareProjectNodeUpdate = serde_json::from_str(
            r#"{"repository": {"title": "core", "url": "https://example.com/a/core"}}"#,
        )
        .unwrap();
        assert!(!update.is_empty());
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({
                "repository": { "title": "core", "url": "https://example.com/a/core" }
            })
        );
        let cleared: SoftwareProjectNodeUpdate =
            serde_json::from_str(r#"{"repository": null}"#).unwrap();
        assert_eq!(cleared.repository, Some(None));
        assert_eq!(
            cleared.to_properties_patch(),
            serde_json::json!({ "repository": null })
        );
    }

    #[test]
    fn checkout_path_is_set_and_cleared() {
        let update: SoftwareProjectNodeUpdate =
            serde_json::from_str(r#"{"checkoutPath": "/work/core"}"#).unwrap();
        assert!(!update.is_empty());
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "checkout_path": "/work/core" })
        );
        let cleared: SoftwareProjectNodeUpdate =
            serde_json::from_str(r#"{"checkoutPath": null}"#).unwrap();
        assert_eq!(cleared.checkout_path, Some(None));
        assert_eq!(
            cleared.to_properties_patch(),
            serde_json::json!({ "checkout_path": null })
        );
    }

    #[test]
    fn an_empty_update_is_empty_and_an_unknown_key_is_rejected() {
        let update: SoftwareProjectNodeUpdate = serde_json::from_str("{}").unwrap();
        assert!(update.is_empty());
        for json in [
            r#"{"content": "Apollo"}"#,
            r#"{"pullRequest": {"title": "PR 7", "url": "https://example.com/pull/7"}}"#,
        ] {
            assert!(
                serde_json::from_str::<SoftwareProjectNodeUpdate>(json).is_err(),
                "{json} must not deserialize as a SoftwareProjectNodeUpdate"
            );
        }
    }
}
