use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::helpers::{default_lifecycle_status, default_version, deserialize_clearable};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct NodeReference {
    pub id: String,
    pub title: Option<String>,
    pub node_type: String,
}

/// The fields every node carries, whatever its type (ADR-086 §2).
///
/// The generic [`Node`] is the envelope itself, and every typed node struct
/// embeds it, so no typed shape can omit a universal field. On a typed node,
/// `properties` holds only extension fields: each field the type's schema
/// chain declares has one home, its typed top-level field.
///
/// `lifecycle_status` is always serialized. It is governance state (ADR-087),
/// and a reader that had to infer `active` from an absent key could not tell
/// that from a shape that never carried the field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct NodeEnvelope {
    pub id: String,
    pub node_type: String,
    pub content: String,
    #[serde(default = "default_version")]
    pub version: i64,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub modified_at: DateTime<Utc>,
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>"))]
    pub properties: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub mentions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub mentioned_in: Vec<NodeReference>,
    // Nullable as well as omittable in TypeScript: this struct is also read
    // back, and a reader takes `null` for no title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub title: Option<String>,
    #[serde(default = "default_lifecycle_status")]
    pub lifecycle_status: String,
}

/// The generic node: the envelope, with every field of its type inside
/// `properties`. A primitive type has no fields, so this is also its whole
/// wire shape.
pub type Node = NodeEnvelope;

impl NodeEnvelope {
    pub fn new(node_type: String, content: String, properties: serde_json::Value) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            node_type,
            content,
            version: 1,
            created_at: now,
            modified_at: now,
            properties,
            mentions: Vec::new(),
            mentioned_in: Vec::new(),
            title: None,
            lifecycle_status: "active".to_string(),
        }
    }

    pub fn new_with_id(
        id: String,
        node_type: String,
        content: String,
        properties: serde_json::Value,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            node_type,
            content,
            version: 1,
            created_at: now,
            modified_at: now,
            properties,
            mentions: Vec::new(),
            mentioned_in: Vec::new(),
            title: None,
            lifecycle_status: "active".to_string(),
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.id.is_empty() {
            return Err(ValidationError::MissingField("id".to_string()));
        }
        if self.node_type.is_empty() {
            return Err(ValidationError::MissingField("node_type".to_string()));
        }
        if !self.properties.is_object() {
            return Err(ValidationError::InvalidProperties(
                "properties must be a JSON object".to_string(),
            ));
        }
        Ok(())
    }

    pub fn set_content(&mut self, content: String) {
        self.content = content;
        self.modified_at = Utc::now();
    }

    pub fn set_properties(&mut self, properties: serde_json::Value) {
        self.properties = properties;
        self.modified_at = Utc::now();
    }
}

/// Sort order specification for query results
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum OrderBy {
    /// Sort by creation time, oldest first
    CreatedAsc,
    /// Sort by creation time, newest first
    CreatedDesc,
    /// Sort by modification time, oldest first
    ModifiedAsc,
    /// Sort by modification time, newest first
    ModifiedDesc,
    /// Sort by content alphabetically, A-Z
    ContentAsc,
    /// Sort by content alphabetically, Z-A
    ContentDesc,
    /// Sort by node type alphabetically
    NodeTypeAsc,
    /// Sort by node type reverse alphabetically
    NodeTypeDesc,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct NodeQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Restrict the result to this explicit set of ids (e.g. a collection's
    /// members). Translated to an `id IN (…)` clause, chunked under SQLite's
    /// bound-parameter ceiling. `None` = no id restriction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentioned_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_contains: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_contains: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    /// Sort order applied by the store before `limit`/`offset` are sliced,
    /// so pagination is stable across repeated calls with the same query.
    /// `None` leaves the underlying result order store-defined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_by: Option<OrderBy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    /// Also return archived nodes. The default leaves them out: an archived
    /// node participates in nothing (ADR-087 §2). Omittable on the wire.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>"))]
    pub include_archived: bool,
}

impl NodeQuery {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn by_id(id: String) -> Self {
        Self {
            id: Some(id),
            ..Default::default()
        }
    }

    pub fn mentioned_by(node_id: String) -> Self {
        Self {
            mentioned_by: Some(node_id),
            ..Default::default()
        }
    }

    pub fn content_contains(search: String) -> Self {
        Self {
            content_contains: Some(search),
            ..Default::default()
        }
    }

    pub fn by_type(node_type: String) -> Self {
        Self {
            node_type: Some(node_type),
            ..Default::default()
        }
    }

    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(optional_fields))]
#[serde(rename_all = "camelCase")]
pub struct NodeUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "Record<string, unknown>"))]
    pub properties: Option<serde_json::Value>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub title: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_status: Option<String>,
}

impl NodeUpdate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_content(mut self, content: String) -> Self {
        self.content = Some(content);
        self
    }

    pub fn with_properties(mut self, properties: serde_json::Value) -> Self {
        self.properties = Some(properties);
        self
    }

    pub fn with_node_type(mut self, node_type: String) -> Self {
        self.node_type = Some(node_type);
        self
    }

    pub fn with_title(mut self, title: Option<String>) -> Self {
        self.title = Some(title);
        self
    }

    pub fn with_lifecycle_status(mut self, lifecycle_status: String) -> Self {
        self.lifecycle_status = Some(lifecycle_status);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.node_type.is_none()
            && self.content.is_none()
            && self.properties.is_none()
            && self.title.is_none()
            && self.lifecycle_status.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DeleteResult {
    pub existed: bool,
    pub deleted_count: u64,
}

impl DeleteResult {
    pub fn existed() -> Self {
        Self {
            existed: true,
            deleted_count: 1,
        }
    }

    pub fn not_found() -> Self {
        Self {
            existed: false,
            deleted_count: 0,
        }
    }
}

#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("Invalid node type: {0}")]
    InvalidNodeType(String),
    #[error("Invalid node ID format: {0}")]
    InvalidId(String),
    #[error("Invalid parent reference: {0}")]
    InvalidParent(String),
    #[error("Invalid root reference: {0}")]
    InvalidRoot(String),
    #[error("Properties validation failed: {0}")]
    InvalidProperties(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_update_title_is_tri_state() {
        let absent: NodeUpdate = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.title, None);
        let cleared: NodeUpdate = serde_json::from_str(r#"{"title": null}"#).unwrap();
        assert_eq!(cleared.title, Some(None));
        let set: NodeUpdate = serde_json::from_str(r#"{"title": "Plan"}"#).unwrap();
        assert_eq!(set.title, Some(Some("Plan".to_string())));
    }
}
