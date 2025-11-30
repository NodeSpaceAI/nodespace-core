//! Strongly-Typed SchemaNode
//!
//! Provides direct deserialization from spoke table with hub data via record link,
//! eliminating the intermediate JSON `properties` step for true compile-time type safety.
//!
//! # Architecture (Issue #673)
//!
//! **Query Pattern:**
//! ```sql
//! SELECT
//!     id,
//!     is_core,
//!     version AS schema_version,
//!     description,
//!     fields,
//!     node.id AS node_id,
//!     node.content AS content,
//!     node.version AS version,
//!     node.created_at AS created_at,
//!     node.modified_at AS modified_at
//! FROM schema:`task`;
//! ```
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::SchemaNode;
//!
//! // Direct field access (no JSON parsing)
//! // let schema = service.get_schema_node("task").await?;
//! // assert_eq!(schema.is_core, true);
//! // assert!(!schema.fields.is_empty());
//! ```

use crate::models::{Node, SchemaDefinition, SchemaField, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Strongly-typed schema node with direct field access
///
/// Deserializes directly from spoke table with hub data via record link.
/// Combines hub metadata (id, content, timestamps) with spoke-specific
/// schema definition fields (is_core, fields, description).
///
/// # Query Pattern
///
/// ```sql
/// SELECT
///     id,
///     is_core,
///     version AS schema_version,
///     description,
///     fields,
///     node.id AS node_id,
///     node.content AS content,
///     node.version AS version,
///     node.created_at AS created_at,
///     node.modified_at AS modified_at
/// FROM schema:`task`;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaNode {
    // ========================================================================
    // Hub fields (from schema.node.* via record link)
    // ========================================================================
    /// Unique identifier (matches hub node ID, e.g., "task", "date")
    pub id: String,

    /// Display name of the schema (e.g., "Task", "Date")
    pub content: String,

    /// Optimistic concurrency control version (hub's version)
    #[serde(default = "default_version")]
    pub version: i64,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp
    pub modified_at: DateTime<Utc>,

    // ========================================================================
    // Spoke fields (direct from schema table)
    // ========================================================================
    /// Whether this is a core schema (shipped with NodeSpace)
    #[serde(default)]
    pub is_core: bool,

    /// Schema version number (increments on schema changes)
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,

    /// Human-readable description of this schema
    #[serde(default)]
    pub description: String,

    /// List of fields in this schema
    #[serde(default)]
    pub fields: Vec<SchemaField>,
}

fn default_version() -> i64 {
    1
}

fn default_schema_version() -> u32 {
    1
}

impl SchemaNode {
    /// Create a SchemaNode from an existing Node (for backward compatibility)
    ///
    /// This converts the JSON properties pattern to strongly-typed fields.
    /// Prefer using `get_schema_node()` from NodeService for direct deserialization.
    ///
    /// # Errors
    ///
    /// Returns `ValidationError::InvalidNodeType` if the node type is not "schema".
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        if node.node_type != "schema" {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected 'schema', got '{}'",
                node.node_type
            )));
        }

        // Try to deserialize properties as SchemaDefinition
        let schema_def: SchemaDefinition =
            serde_json::from_value(node.properties.clone()).unwrap_or_default();

        Ok(Self {
            id: node.id,
            content: node.content,
            version: node.version,
            created_at: node.created_at,
            modified_at: node.modified_at,
            is_core: schema_def.is_core,
            schema_version: schema_def.version,
            description: schema_def.description,
            fields: schema_def.fields,
        })
    }

    /// Convert to universal Node (for backward compatibility with existing APIs)
    ///
    /// This creates a Node with properties populated from the strongly-typed fields.
    pub fn into_node(self) -> Node {
        let schema_def = SchemaDefinition {
            is_core: self.is_core,
            version: self.schema_version,
            description: self.description,
            fields: self.fields,
        };

        let properties = serde_json::to_value(&schema_def).unwrap_or_else(|_| json!({}));

        Node {
            id: self.id,
            node_type: "schema".to_string(),
            content: self.content,
            version: self.version,
            created_at: self.created_at,
            modified_at: self.modified_at,
            properties,
            embedding_vector: None,
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
        }
    }

    /// Get a reference as Node (creates a temporary Node for compatibility)
    pub fn as_node(&self) -> Node {
        self.clone().into_node()
    }

    /// Convert to SchemaDefinition (for compatibility with existing schema APIs)
    pub fn into_definition(self) -> SchemaDefinition {
        SchemaDefinition {
            is_core: self.is_core,
            version: self.schema_version,
            description: self.description,
            fields: self.fields,
        }
    }

    /// Get as SchemaDefinition reference (creates temporary for compatibility)
    pub fn as_definition(&self) -> SchemaDefinition {
        SchemaDefinition {
            is_core: self.is_core,
            version: self.schema_version,
            description: self.description.clone(),
            fields: self.fields.clone(),
        }
    }

    /// Get all valid values for an enum field (delegates to SchemaDefinition)
    pub fn get_enum_values(&self, field_name: &str) -> Option<Vec<String>> {
        self.as_definition().get_enum_values(field_name)
    }

    /// Check if a field can be deleted (delegates to SchemaDefinition)
    pub fn can_delete_field(&self, field_name: &str) -> bool {
        self.as_definition().can_delete_field(field_name)
    }

    /// Check if a field can be modified (delegates to SchemaDefinition)
    pub fn can_modify_field(&self, field_name: &str) -> bool {
        self.as_definition().can_modify_field(field_name)
    }

    /// Get a field by name
    pub fn get_field(&self, field_name: &str) -> Option<&SchemaField> {
        self.fields.iter().find(|f| f.name == field_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_test_schema_node() -> Node {
        Node::new_with_id(
            "task".to_string(),
            "schema".to_string(),
            "Task".to_string(),
            json!({
                "isCore": true,
                "version": 2,
                "description": "Task tracking schema",
                "fields": [
                    {
                        "name": "status",
                        "type": "enum",
                        "protection": "core",
                        "coreValues": ["open", "in_progress", "done"],
                        "userValues": ["blocked"],
                        "indexed": true,
                        "required": true,
                        "extensible": true,
                        "default": "open"
                    }
                ]
            }),
        )
    }

    #[test]
    fn test_from_node_validates_type() {
        let node = create_test_schema_node();
        assert!(SchemaNode::from_node(node).is_ok());

        let wrong_type = Node::new("text".to_string(), "Test".to_string(), json!({}));
        let result = SchemaNode::from_node(wrong_type);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Expected 'schema'"));
    }

    #[test]
    fn test_from_node_extracts_fields() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        assert_eq!(schema.id, "task");
        assert_eq!(schema.content, "Task");
        assert!(schema.is_core);
        assert_eq!(schema.schema_version, 2);
        assert_eq!(schema.description, "Task tracking schema");
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].name, "status");
    }

    #[test]
    fn test_into_node_preserves_data() {
        let original = create_test_schema_node();
        let original_id = original.id.clone();

        let schema = SchemaNode::from_node(original).unwrap();
        let converted = schema.into_node();

        assert_eq!(converted.id, original_id);
        assert_eq!(converted.node_type, "schema");
        assert_eq!(converted.content, "Task");
    }

    #[test]
    fn test_get_field() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        let status_field = schema.get_field("status");
        assert!(status_field.is_some());
        assert_eq!(status_field.unwrap().field_type, "enum");

        let missing_field = schema.get_field("nonexistent");
        assert!(missing_field.is_none());
    }

    #[test]
    fn test_get_enum_values() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        let values = schema.get_enum_values("status");
        assert!(values.is_some());
        let values = values.unwrap();
        assert!(values.contains(&"open".to_string()));
        assert!(values.contains(&"in_progress".to_string()));
        assert!(values.contains(&"done".to_string()));
        assert!(values.contains(&"blocked".to_string()));
    }

    #[test]
    fn test_can_delete_field() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        // Core fields cannot be deleted
        assert!(!schema.can_delete_field("status"));
        // Non-existent fields cannot be deleted
        assert!(!schema.can_delete_field("nonexistent"));
    }

    #[test]
    fn test_serde_serialization() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        // Uses camelCase for JSON (matching Node struct convention)
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["id"], "task");
        assert_eq!(json["content"], "Task");
        assert_eq!(json["isCore"], true);
        assert_eq!(json["schemaVersion"], 2);
    }

    #[test]
    fn test_serde_deserialization() {
        // Uses camelCase for JSON (matching Node struct convention)
        let json = json!({
            "id": "test-schema",
            "content": "Test Schema",
            "version": 1,
            "createdAt": "2025-01-01T00:00:00Z",
            "modifiedAt": "2025-01-01T00:00:00Z",
            "isCore": false,
            "schemaVersion": 1,
            "description": "A test schema",
            "fields": []
        });

        let schema: SchemaNode = serde_json::from_value(json).unwrap();
        assert_eq!(schema.id, "test-schema");
        assert_eq!(schema.content, "Test Schema");
        assert!(!schema.is_core);
        assert_eq!(schema.schema_version, 1);
        assert_eq!(schema.description, "A test schema");
        assert!(schema.fields.is_empty());
    }

    #[test]
    fn test_into_definition() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();
        let definition = schema.into_definition();

        assert!(definition.is_core);
        assert_eq!(definition.version, 2);
        assert_eq!(definition.description, "Task tracking schema");
        assert_eq!(definition.fields.len(), 1);
    }
}
