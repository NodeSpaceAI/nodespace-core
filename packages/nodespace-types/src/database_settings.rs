use serde::{Deserialize, Serialize};

use crate::helpers::deserialize_clearable;
use crate::node::NodeEnvelope;

/// Wire shape for the database-settings singleton sent to the frontend.
///
/// Produced by `node_to_typed_value` for a `database-settings` node. Its one
/// field, `required_extensions`, is promoted to the top level.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DatabaseSettingsNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    /// Ids of the extensions a reader needs in order to read this database.
    /// Empty when the settings node stores none.
    pub required_extensions: Vec<String>,
}

/// Partial update for the settings node's core fields, received from the
/// frontend.
///
/// `required_extensions` is tri-state: absent leaves it unchanged, `null`
/// clears it (it then reads as empty), and a list replaces it whole.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DatabaseSettingsNodeUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub required_extensions: Option<Option<Vec<String>>>,
}

impl DatabaseSettingsNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self.required_extensions.is_none()
    }

    /// The flat, bare-key properties patch this update writes
    /// (`{"required_extensions": ["x"]}`); a cleared field is written as
    /// `null`. The service layer moves the keys into the `database-settings`
    /// storage bucket.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::new();
        if let Some(required_extensions) = &self.required_extensions {
            patch.insert(
                "required_extensions".to_string(),
                serde_json::json!(required_extensions),
            );
        }
        serde_json::Value::Object(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The update carries the settings schema's fields only; naming anything
    /// else is an error rather than a silently dropped write.
    #[test]
    fn an_unknown_key_is_rejected() {
        assert!(
            serde_json::from_str::<DatabaseSettingsNodeUpdate>(r#"{"content": "Settings"}"#)
                .is_err()
        );
    }

    #[test]
    fn a_list_must_hold_strings() {
        assert!(serde_json::from_str::<DatabaseSettingsNodeUpdate>(
            r#"{"requiredExtensions": [1]}"#
        )
        .is_err());
    }

    #[test]
    fn absent_null_and_value_are_distinct() {
        let absent: DatabaseSettingsNodeUpdate = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.required_extensions, None);
        assert!(absent.is_empty());

        let cleared: DatabaseSettingsNodeUpdate =
            serde_json::from_str(r#"{"requiredExtensions": null}"#).unwrap();
        assert_eq!(cleared.required_extensions, Some(None));
        assert_eq!(
            cleared.to_properties_patch(),
            serde_json::json!({ "required_extensions": null })
        );

        let set: DatabaseSettingsNodeUpdate =
            serde_json::from_str(r#"{"requiredExtensions": ["fixture"]}"#).unwrap();
        assert_eq!(
            set.to_properties_patch(),
            serde_json::json!({ "required_extensions": ["fixture"] })
        );
    }
}
