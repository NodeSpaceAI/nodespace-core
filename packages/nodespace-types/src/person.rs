use serde::{Deserialize, Serialize};

use crate::helpers::deserialize_clearable;
use crate::node::NodeEnvelope;

/// Wire shape for person nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for `node_type == "person"`. The person
/// schema's core fields (`first_name`, `last_name`, `email`) are promoted to
/// the top level; they map directly to the TypeScript `PersonNode` interface.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct PersonNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Partial update for a person's core fields, received from the frontend.
///
/// Each field is tri-state: absent leaves it unchanged, `null` clears it, and
/// a string sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonNodeUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub first_name: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub last_name: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub email: Option<Option<String>>,
}

impl PersonNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self.first_name.is_none() && self.last_name.is_none() && self.email.is_none()
    }

    /// The flat, bare-key properties patch this update writes (`{"first_name":
    /// "Ada"}`); a cleared field is written as `null`. The service layer moves
    /// the keys into the `person` storage bucket.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::new();
        for (key, value) in [
            ("first_name", &self.first_name),
            ("last_name", &self.last_name),
            ("email", &self.email),
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

    /// The update carries the person schema's fields only; naming anything
    /// else is an error rather than a silently dropped write.
    #[test]
    fn an_unknown_key_is_rejected() {
        assert!(serde_json::from_str::<PersonNodeUpdate>(r#"{"content": "Ada"}"#).is_err());
    }

    #[test]
    fn absent_null_and_value_are_distinct() {
        let update: PersonNodeUpdate =
            serde_json::from_str(r#"{"firstName": "Ada", "email": null}"#).unwrap();
        assert_eq!(update.first_name, Some(Some("Ada".to_string())));
        assert_eq!(update.last_name, None);
        assert_eq!(update.email, Some(None));
    }

    #[test]
    fn patch_carries_only_the_fields_the_update_names() {
        let update = PersonNodeUpdate {
            first_name: Some(Some("Ada".to_string())),
            email: Some(None),
            ..Default::default()
        };
        assert_eq!(
            update.to_properties_patch(),
            serde_json::json!({ "first_name": "Ada", "email": null })
        );
    }

    #[test]
    fn empty_update_is_empty() {
        assert!(PersonNodeUpdate::default().is_empty());
        let update: PersonNodeUpdate = serde_json::from_str("{}").unwrap();
        assert!(update.is_empty());
    }
}
