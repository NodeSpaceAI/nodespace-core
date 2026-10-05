use serde::{Deserialize, Serialize};

use crate::helpers::{deserialize_clearable, deserialize_set_only};
use crate::node::NodeEnvelope;

/// Where a spec stands (ADR-092 §1). A closed vocabulary: the seeded rules
/// compare against `approved` and `superseded` by name, so a value added
/// beside them would be one no rule understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum SpecStatus {
    /// Being written.
    #[default]
    Draft,
    /// Confirmed by the requester; a plan against it may be approved.
    Approved,
    /// Replaced by a newer spec; its fields are locked.
    Superseded,
}

impl SpecStatus {
    pub const ALL: [(SpecStatus, &'static str); 3] = [
        (SpecStatus::Draft, "Draft"),
        (SpecStatus::Approved, "Approved"),
        (SpecStatus::Superseded, "Superseded"),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::Superseded => "superseded",
        }
    }
}

/// Wire shape for spec nodes sent to the frontend.
///
/// A spec says what is being built and why. Its `content` is its title and
/// its criteria are its direct `checkbox` children, so neither is a field.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct SpecNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    /// What is being built, why, and for whom.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    /// What may always be done, what needs sign-off, and what must never be
    /// done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundaries: Option<String>,
    pub spec_status: SpecStatus,
}

/// Partial update for a spec's core fields, received from the frontend.
///
/// `spec_status` can be set but not cleared (the schema requires it); the
/// text fields are tri-state: absent leaves the field unchanged, `null`
/// clears it, and a string sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpecNodeUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub objective: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub boundaries: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_set_only"
    )]
    pub spec_status: Option<SpecStatus>,
}

impl SpecNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self.objective.is_none() && self.boundaries.is_none() && self.spec_status.is_none()
    }

    /// The flat, bare-key properties patch this update writes
    /// (`{"spec_status": "approved"}`); a cleared field is written as `null`.
    /// The service layer moves the keys into the `spec` storage bucket.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::new();
        for (key, value) in [
            ("objective", &self.objective),
            ("boundaries", &self.boundaries),
        ] {
            if let Some(value) = value {
                patch.insert(key.to_string(), serde_json::json!(value));
            }
        }
        if let Some(status) = self.spec_status {
            patch.insert("spec_status".to_string(), serde_json::json!(status));
        }
        serde_json::Value::Object(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_serializes_as_its_stored_value_and_defaults_to_draft() {
        for (status, _) in SpecStatus::ALL {
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::json!(status.as_str())
            );
        }
        assert_eq!(SpecStatus::default(), SpecStatus::Draft);
        // The vocabulary is closed: there is no user-defined status.
        assert!(serde_json::from_str::<SpecStatus>(r#""in_review""#).is_err());
    }

    #[test]
    fn absent_null_and_value_are_distinct() {
        let update: SpecNodeUpdate =
            serde_json::from_str(r#"{"objective": "Ship it", "boundaries": null}"#).unwrap();
        assert_eq!(update.objective, Some(Some("Ship it".to_string())));
        assert_eq!(update.boundaries, Some(None));
        assert_eq!(update.spec_status, None);
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "objective": "Ship it", "boundaries": null })
        );
    }

    #[test]
    fn the_status_is_set_but_never_cleared() {
        let update: SpecNodeUpdate = serde_json::from_str(r#"{"specStatus": "approved"}"#).unwrap();
        assert_eq!(update.spec_status, Some(SpecStatus::Approved));
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "spec_status": "approved" })
        );
        assert!(serde_json::from_str::<SpecNodeUpdate>(r#"{"specStatus": null}"#).is_err());
        assert!(serde_json::from_str::<SpecNodeUpdate>(r#"{"specStatus": "done"}"#).is_err());
    }

    /// The update carries the spec schema's fields only: the title is
    /// `content` and the criteria are children.
    #[test]
    fn an_unknown_key_is_rejected() {
        for json in [
            r#"{"content": "Renamed"}"#,
            r#"{"successCriteria": "It works"}"#,
        ] {
            assert!(
                serde_json::from_str::<SpecNodeUpdate>(json).is_err(),
                "{json}"
            );
        }
    }

    #[test]
    fn empty_update_is_empty() {
        let update: SpecNodeUpdate = serde_json::from_str("{}").unwrap();
        assert!(update.is_empty());
    }
}
