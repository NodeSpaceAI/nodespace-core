use serde::{Deserialize, Serialize};

use crate::helpers::deserialize_clearable;
use crate::node::NodeEnvelope;

/// Wire shape for collection nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for a `collection` node. The collection
/// schema's one field, `description`, is promoted to the top level; the
/// collection's name is the envelope's `content`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct CollectionNode {
    /// The fields every node carries. `properties` holds extension fields
    /// only; the type's own fields are the typed ones below.
    #[serde(flatten)]
    pub envelope: NodeEnvelope,
    /// What the collection is for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Partial update for a collection's core fields, received from the frontend.
///
/// `description` is tri-state: absent leaves it unchanged, `null` clears it,
/// and a string sets it. The collection's name is `content`, an envelope
/// field, and is written through the rename operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionNodeUpdate {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub description: Option<Option<String>>,
}

impl CollectionNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self.description.is_none()
    }

    /// The flat, bare-key properties patch this update writes
    /// (`{"description": "Accounts we bill"}`); a cleared field is written as
    /// `null`. The service layer moves the keys into the `collection` storage
    /// bucket.
    pub fn to_properties_patch(&self) -> serde_json::Value {
        let mut patch = serde_json::Map::new();
        if let Some(description) = &self.description {
            patch.insert("description".to_string(), serde_json::json!(description));
        }
        serde_json::Value::Object(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The update carries the collection schema's fields only; naming anything
    /// else is an error rather than a silently dropped write.
    #[test]
    fn an_unknown_key_is_rejected() {
        assert!(serde_json::from_str::<CollectionNodeUpdate>(r#"{"content": "hr"}"#).is_err());
    }

    #[test]
    fn absent_null_and_value_are_distinct() {
        let absent: CollectionNodeUpdate = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.description, None);
        assert!(absent.is_empty());

        let cleared: CollectionNodeUpdate =
            serde_json::from_str(r#"{"description": null}"#).unwrap();
        assert_eq!(cleared.description, Some(None));
        assert_eq!(
            cleared.to_properties_patch(),
            serde_json::json!({ "description": null })
        );

        let set: CollectionNodeUpdate =
            serde_json::from_str(r#"{"description": "Accounts we bill"}"#).unwrap();
        assert_eq!(
            set.to_properties_patch(),
            serde_json::json!({ "description": "Accounts we bill" })
        );
    }
}
