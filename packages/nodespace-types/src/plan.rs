use serde::{Deserialize, Serialize};

use crate::helpers::{deserialize_clearable, deserialize_set_only};
use crate::node::NodeEnvelope;

/// Where a plan stands (ADR-092 §1). A closed vocabulary, as for a spec: the
/// seeded rules compare against `approved` and `superseded` by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum PlanStatus {
    /// Being written.
    #[default]
    Draft,
    /// Confirmed by the requester; its tasks may start.
    Approved,
    /// Replaced by a newer plan; its fields are locked.
    Superseded,
}

impl PlanStatus {
    pub const ALL: [(PlanStatus, &'static str); 3] = [
        (PlanStatus::Draft, "Draft"),
        (PlanStatus::Approved, "Approved"),
        (PlanStatus::Superseded, "Superseded"),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::Superseded => "superseded",
        }
    }
}

/// Wire shape for plan nodes sent to the frontend.
///
/// A plan says how one spec will be built. Its `content` is its title; the
/// spec it implements and the tasks that carry it out are relationships.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    /// Components, dependencies and sequencing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approach: Option<String>,
    /// What could go wrong with this approach.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risks: Option<String>,
    pub plan_status: PlanStatus,
}

/// Partial update for a plan's core fields, received from the frontend.
///
/// `plan_status` can be set but not cleared (the schema requires it); the
/// text fields are tri-state: absent leaves the field unchanged, `null`
/// clears it, and a string sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanNodeUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub approach: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub risks: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_set_only"
    )]
    pub plan_status: Option<PlanStatus>,
}

impl PlanNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self.approach.is_none() && self.risks.is_none() && self.plan_status.is_none()
    }

    /// The flat, bare-key properties patch this update writes
    /// (`{"plan_status": "approved"}`); a cleared field is written as `null`.
    /// The service layer moves the keys into the `plan` storage bucket.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::new();
        for (key, value) in [("approach", &self.approach), ("risks", &self.risks)] {
            if let Some(value) = value {
                patch.insert(key.to_string(), serde_json::json!(value));
            }
        }
        if let Some(status) = self.plan_status {
            patch.insert("plan_status".to_string(), serde_json::json!(status));
        }
        serde_json::Value::Object(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_serializes_as_its_stored_value_and_defaults_to_draft() {
        for (status, _) in PlanStatus::ALL {
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::json!(status.as_str())
            );
        }
        assert_eq!(PlanStatus::default(), PlanStatus::Draft);
        assert!(serde_json::from_str::<PlanStatus>(r#""accepted""#).is_err());
    }

    #[test]
    fn absent_null_and_value_are_distinct() {
        let update: PlanNodeUpdate =
            serde_json::from_str(r#"{"approach": "Two phases", "risks": null}"#).unwrap();
        assert_eq!(update.approach, Some(Some("Two phases".to_string())));
        assert_eq!(update.risks, Some(None));
        assert_eq!(update.plan_status, None);
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "approach": "Two phases", "risks": null })
        );
    }

    #[test]
    fn the_status_is_set_but_never_cleared() {
        let update: PlanNodeUpdate = serde_json::from_str(r#"{"planStatus": "approved"}"#).unwrap();
        assert_eq!(update.plan_status, Some(PlanStatus::Approved));
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "plan_status": "approved" })
        );
        assert!(serde_json::from_str::<PlanNodeUpdate>(r#"{"planStatus": null}"#).is_err());
    }

    #[test]
    fn an_unknown_key_is_rejected() {
        assert!(serde_json::from_str::<PlanNodeUpdate>(r#"{"content": "Renamed"}"#).is_err());
    }

    #[test]
    fn empty_update_is_empty() {
        let update: PlanNodeUpdate = serde_json::from_str("{}").unwrap();
        assert!(update.is_empty());
    }
}
