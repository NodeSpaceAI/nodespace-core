use serde::{Deserialize, Serialize};

use crate::helpers::deserialize_set_only;
use crate::node::NodeEnvelope;

/// Where a decision stands (ADR-092 §1). A closed vocabulary: the seeded
/// lock compares against `superseded` by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    /// Put forward, not yet agreed.
    #[default]
    Proposed,
    /// Agreed; the work it governs follows it.
    Accepted,
    /// Replaced by a newer decision; its field is locked.
    Superseded,
}

impl DecisionStatus {
    pub const ALL: [(DecisionStatus, &'static str); 3] = [
        (DecisionStatus::Proposed, "Proposed"),
        (DecisionStatus::Accepted, "Accepted"),
        (DecisionStatus::Superseded, "Superseded"),
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Accepted => "accepted",
            Self::Superseded => "superseded",
        }
    }
}

/// Wire shape for decision nodes sent to the frontend.
///
/// A decision records what was decided and why. Its `content` is its title
/// and its body is its children, so its status is its only field.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DecisionNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own field is the typed one below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    pub decision_status: DecisionStatus,
}

/// Partial update for a decision's core field, received from the frontend.
///
/// `decision_status` can be set but not cleared (the schema requires it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionNodeUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_set_only"
    )]
    pub decision_status: Option<DecisionStatus>,
}

impl DecisionNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self.decision_status.is_none()
    }

    /// The flat, bare-key properties patch this update writes
    /// (`{"decision_status": "accepted"}`). The service layer moves the key
    /// into the `decision` storage bucket.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::new();
        if let Some(status) = self.decision_status {
            patch.insert("decision_status".to_string(), serde_json::json!(status));
        }
        serde_json::Value::Object(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_serializes_as_its_stored_value_and_defaults_to_proposed() {
        for (status, _) in DecisionStatus::ALL {
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                serde_json::json!(status.as_str())
            );
        }
        assert_eq!(DecisionStatus::default(), DecisionStatus::Proposed);
        assert!(serde_json::from_str::<DecisionStatus>(r#""approved""#).is_err());
    }

    #[test]
    fn the_status_is_set_but_never_cleared() {
        let update: DecisionNodeUpdate =
            serde_json::from_str(r#"{"decisionStatus": "accepted"}"#).unwrap();
        assert_eq!(update.decision_status, Some(DecisionStatus::Accepted));
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "decision_status": "accepted" })
        );
        assert!(serde_json::from_str::<DecisionNodeUpdate>(r#"{"decisionStatus": null}"#).is_err());
    }

    #[test]
    fn an_unknown_key_is_rejected() {
        assert!(serde_json::from_str::<DecisionNodeUpdate>(r#"{"content": "Renamed"}"#).is_err());
    }

    #[test]
    fn empty_update_is_empty() {
        let update: DecisionNodeUpdate = serde_json::from_str("{}").unwrap();
        assert!(update.is_empty());
    }
}
