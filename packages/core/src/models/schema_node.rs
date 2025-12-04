//! Strongly-Typed SchemaNode
//!
//! Provides direct deserialization from spoke table with hub data via record link,
//! eliminating the intermediate JSON `properties` step for true compile-time type safety.
//!
//! # Architecture
//!
//! **Query Pattern:**
//! ```sql
//! SELECT
//!     record::id(id) AS id,
//!     is_core,
//!     version AS schema_version,
//!     description,
//!     fields,
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
//! // let mut schema = service.get_schema_node("task").await?.unwrap();
//! // schema.fields.push(new_field);
//! // schema.schema_version += 1;
//! // store.update_schema_node(schema).await?;
//! ```

use crate::models::schema::{EnumValue, SchemaField, SchemaProtectionLevel, SchemaRelationship};
use crate::models::{Node, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Strongly-typed schema node with direct field access
///
/// Deserializes directly from spoke table with hub data via record link.
/// Combines hub metadata (id, content, timestamps) with spoke-specific
/// schema definition fields (is_core, fields, description).
///
/// Fields are public for direct mutation. After modifying, persist via
/// `store.update_schema_node(schema)`.
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

    /// List of relationships to other node types
    ///
    /// Relationships define edges to other schemas. Edge tables are automatically
    /// created when the schema is saved. See [`SchemaRelationship`] for details.
    #[serde(default)]
    pub relationships: Vec<SchemaRelationship>,
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

        // Extract fields from properties JSON
        let is_core = node
            .properties
            .get("isCore")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let schema_version = node
            .properties
            .get("version")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .unwrap_or(1);

        let description = node
            .properties
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        let fields: Vec<SchemaField> = node
            .properties
            .get("fields")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let relationships: Vec<SchemaRelationship> = node
            .properties
            .get("relationships")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        Ok(Self {
            id: node.id,
            content: node.content,
            version: node.version,
            created_at: node.created_at,
            modified_at: node.modified_at,
            is_core,
            schema_version,
            description,
            fields,
            relationships,
        })
    }

    /// Convert to universal Node (for compatibility with existing APIs)
    ///
    /// This creates a Node with properties populated from the strongly-typed fields.
    pub fn into_node(self) -> Node {
        let properties = serde_json::json!({
            "isCore": self.is_core,
            "version": self.schema_version,
            "description": self.description,
            "fields": self.fields,
            "relationships": self.relationships,
        });

        Node {
            id: self.id,
            node_type: "schema".to_string(),
            content: self.content,
            version: self.version,
            created_at: self.created_at,
            modified_at: self.modified_at,
            properties,
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
        }
    }

    /// Get a field by name
    pub fn get_field(&self, name: &str) -> Option<&SchemaField> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Get a mutable field by name
    pub fn get_field_mut(&mut self, name: &str) -> Option<&mut SchemaField> {
        self.fields.iter_mut().find(|f| f.name == name)
    }

    /// Get all valid values for an enum field (core + user values combined)
    ///
    /// Returns `None` if the field doesn't exist or isn't an enum.
    /// Returns `EnumValue` structs with both value and label.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let schema = store.get_schema_node("task").await?.unwrap();
    /// let status_values = schema.get_enum_values("status");
    /// // Returns: Some([EnumValue { value: "open", label: "Open" }, ...])
    /// ```
    pub fn get_enum_values(&self, field_name: &str) -> Option<Vec<EnumValue>> {
        let field = self.get_field(field_name)?;

        // Only return values for enum fields
        if field.field_type != "enum" {
            return None;
        }

        let mut values = Vec::new();
        if let Some(core_vals) = &field.core_values {
            values.extend(core_vals.clone());
        }
        if let Some(user_vals) = &field.user_values {
            values.extend(user_vals.clone());
        }

        Some(values)
    }

    /// Get all valid value strings for an enum field (for validation)
    ///
    /// Returns only the value strings, not the labels. Use this for validation
    /// when checking if a value is valid for an enum field. For UI display where
    /// you need both values and labels, use [`get_enum_values`] instead.
    ///
    /// # Example
    /// ```ignore
    /// let valid_values = schema.get_enum_value_strings("status");
    /// // Returns: Some(["open", "in_progress", "done", "blocked"])
    /// ```
    pub fn get_enum_value_strings(&self, field_name: &str) -> Option<Vec<String>> {
        self.get_enum_values(field_name)
            .map(|values| values.into_iter().map(|v| v.value).collect())
    }

    /// Check if a field can be deleted based on its protection level
    ///
    /// Only `User` protected fields can be deleted.
    pub fn can_delete_field(&self, field_name: &str) -> bool {
        self.get_field(field_name)
            .map(|f| f.protection == SchemaProtectionLevel::User)
            .unwrap_or(false)
    }

    /// Check if a field can be modified based on its protection level
    ///
    /// Only `User` protected fields can be modified (type changes, etc.).
    /// Core/System fields are immutable.
    pub fn can_modify_field(&self, field_name: &str) -> bool {
        self.get_field(field_name)
            .map(|f| f.protection == SchemaProtectionLevel::User)
            .unwrap_or(false)
    }

    // ========================================================================
    // Relationship helpers
    // ========================================================================

    /// Get a relationship by name
    pub fn get_relationship(&self, name: &str) -> Option<&SchemaRelationship> {
        self.relationships.iter().find(|r| r.name == name)
    }

    /// Get a mutable relationship by name
    pub fn get_relationship_mut(&mut self, name: &str) -> Option<&mut SchemaRelationship> {
        self.relationships.iter_mut().find(|r| r.name == name)
    }

    /// Check if this schema has any relationships defined
    pub fn has_relationships(&self) -> bool {
        !self.relationships.is_empty()
    }

    /// Get all relationships targeting a specific node type
    pub fn get_relationships_to(&self, target_type: &str) -> Vec<&SchemaRelationship> {
        self.relationships
            .iter()
            .filter(|r| r.target_type == target_type)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::schema::SchemaProtectionLevel;
    use serde_json::json;

    fn create_test_schema_node() -> Node {
        Node::new(
            "schema".to_string(),
            "task".to_string(),
            json!({
                "isCore": true,
                "version": 2,
                "description": "Task tracking schema",
                "fields": [
                    {
                        "name": "status",
                        "type": "enum",
                        "protection": "core",
                        "coreValues": [
                            { "value": "open", "label": "Open" },
                            { "value": "done", "label": "Done" }
                        ],
                        "indexed": true
                    }
                ]
            }),
        )
    }

    #[test]
    fn test_from_node_validates_type() {
        let node = create_test_schema_node();
        assert!(SchemaNode::from_node(node).is_ok());

        let wrong_type = Node::new("task".to_string(), "Test".to_string(), json!({}));
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
        assert_eq!(converted.content, "task");
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
    fn test_direct_field_mutation() {
        let node = create_test_schema_node();
        let mut schema = SchemaNode::from_node(node).unwrap();

        // Direct field mutation
        schema.description = "Updated description".to_string();
        schema.schema_version += 1;

        assert_eq!(schema.description, "Updated description");
        assert_eq!(schema.schema_version, 3);
    }

    #[test]
    fn test_add_field_via_push() {
        let node = create_test_schema_node();
        let mut schema = SchemaNode::from_node(node).unwrap();

        let new_field = SchemaField {
            name: "priority".to_string(),
            field_type: "number".to_string(),
            protection: SchemaProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: Some(false),
            extensible: None,
            default: Some(json!(0)),
            description: Some("Priority level".to_string()),
            item_type: None,
            fields: None,
            item_fields: None,
        };

        schema.fields.push(new_field);
        assert_eq!(schema.fields.len(), 2);
        assert!(schema.get_field("priority").is_some());
    }

    #[test]
    fn test_get_enum_values() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        let values = schema.get_enum_values("status").unwrap();
        assert_eq!(values.len(), 2);
        assert!(values.iter().any(|v| v.value == "open"));
        assert!(values.iter().any(|v| v.value == "done"));
        // Verify labels are present
        assert!(values.iter().any(|v| v.label == "Open"));
        assert!(values.iter().any(|v| v.label == "Done"));

        // Test the string-only helper
        let value_strings = schema.get_enum_value_strings("status").unwrap();
        assert_eq!(value_strings.len(), 2);
        assert!(value_strings.contains(&"open".to_string()));
        assert!(value_strings.contains(&"done".to_string()));

        // Non-enum field should return None
        assert!(schema.get_enum_values("nonexistent").is_none());
    }

    #[test]
    fn test_can_delete_field() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        // Core field cannot be deleted
        assert!(!schema.can_delete_field("status"));
        // Non-existent field returns false
        assert!(!schema.can_delete_field("nonexistent"));
    }

    #[test]
    fn test_serde_serialization() {
        let node = create_test_schema_node();
        let schema = SchemaNode::from_node(node).unwrap();

        // Uses camelCase for JSON
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["isCore"], true);
        assert_eq!(json["schemaVersion"], 2);
        assert_eq!(json["description"], "Task tracking schema");
    }

    #[test]
    fn test_serde_deserialization() {
        // Direct deserialization (simulates spoke table query result)
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
}
