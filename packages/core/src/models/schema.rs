//! Schema Management Types
//!
//! This module contains data structures for managing user-defined entity schemas
//! in NodeSpace. Schemas are stored as regular nodes with `node_type = 'schema'`
//! and follow the Pure JSON schema-as-node pattern.
//!
//! ## Schema Protection Levels
//!
//! - `Core`: Cannot be modified or deleted (UI components depend on these fields)
//! - `User`: Fully modifiable/deletable by users
//! - `System`: Auto-managed internal fields, read-only
//!
//! ## Example Schema Node
//!
//! ```json
//! {
//!   "id": "task",
//!   "node_type": "schema",
//!   "content": "Task",
//!   "properties": {
//!     "is_core": true,
//!     "version": 2,
//!     "description": "Task tracking schema",
//!     "fields": [
//!       {
//!         "name": "status",
//!         "type": "enum",
//!         "protection": "core",
//!         "core_values": ["OPEN", "IN_PROGRESS", "DONE"],
//!         "user_values": ["BLOCKED"],
//!         "extensible": true,
//!         "indexed": true,
//!         "required": true,
//!         "default": "OPEN"
//!       }
//!     ]
//!   }
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Simpler field definition for schema creation (user input)
///
/// This is a simplified structure used when creating new schemas via MCP or API.
/// It gets converted to `SchemaField` internally with proper defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDefinition {
    /// Field name
    pub name: String,

    /// Field type (e.g., "string", "number", "person", "project", etc.)
    #[serde(rename = "type")]
    pub field_type: String,

    /// Whether this field is required
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Default value for the field
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,

    /// For nested objects, the structure definition
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<HashMap<String, serde_json::Value>>,
}

/// Protection level for schema fields
///
/// Determines whether a field can be modified or deleted by users.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProtectionLevel {
    /// Core fields that cannot be modified or deleted
    ///
    /// These are fields that UI components or core functionality depend on.
    /// Examples: task.status, person.name
    Core,

    /// User-defined fields that can be freely modified or deleted
    ///
    /// These are fields added by users for custom properties.
    User,

    /// System-managed fields that are read-only
    ///
    /// These are auto-managed internal fields like timestamps.
    /// Reserved for future use.
    System,
}

/// Definition of a single field in a schema
///
/// Supports various field types including primitives, enums, arrays, and objects.
/// Enum fields can have protected core values and user-extensible values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaField {
    /// Field name (must be unique within schema)
    pub name: String,

    /// Field type (e.g., "string", "number", "boolean", "enum", "array", "object")
    #[serde(rename = "type")]
    pub field_type: String,

    /// Protection level determining mutability
    pub protection: ProtectionLevel,

    /// Protected enum values (cannot be removed) - enum fields only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_values: Option<Vec<String>>,

    /// User-extensible enum values (can be added/removed) - enum fields only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_values: Option<Vec<String>>,

    /// Whether this field should be indexed for faster queries
    pub indexed: bool,

    /// Whether this field is required (cannot be null/undefined)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Whether enum values can be extended by users
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensible: Option<bool>,

    /// Default value for the field
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,

    /// Human-readable description of the field
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Type of items in array fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,

    /// Nested fields for object types (RECURSIVE!)
    ///
    /// When field_type = "object", this defines the structure of the nested object.
    /// Example: address field with street, city, zip nested fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<SchemaField>>,

    /// For array of objects, define the object structure
    ///
    /// When field_type = "array" and item_type = "object", this defines the
    /// structure of objects in the array.
    /// Example: contacts array where each item has name, email, phone fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_fields: Option<Vec<SchemaField>>,
}

/// Complete schema definition for an entity type
///
/// Stored in the `properties` field of schema nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaDefinition {
    /// Whether this is a core schema (shipped with NodeSpace)
    pub is_core: bool,

    /// Schema version number (increments on any change)
    ///
    /// Used for lazy migration: nodes track which version validated them,
    /// and are automatically upgraded when accessed if schema version is newer.
    pub version: u32,

    /// Human-readable description of this schema
    pub description: String,

    /// List of fields in this schema
    pub fields: Vec<SchemaField>,
}

impl SchemaDefinition {
    /// Get all valid values for an enum field (core + user values combined)
    ///
    /// Returns `None` if the field doesn't exist or isn't an enum.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let schema = get_task_schema();
    /// let status_values = schema.get_enum_values("status");
    /// // Returns: Some(["OPEN", "IN_PROGRESS", "DONE", "BLOCKED"])
    /// ```
    pub fn get_enum_values(&self, field_name: &str) -> Option<Vec<String>> {
        let field = self.fields.iter().find(|f| f.name == field_name)?;

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

    /// Check if a field can be deleted based on its protection level
    ///
    /// Only `User` protected fields can be deleted.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let schema = get_task_schema();
    /// assert!(!schema.can_delete_field("status"));  // Core field
    /// assert!(schema.can_delete_field("priority"));  // User field
    /// ```
    pub fn can_delete_field(&self, field_name: &str) -> bool {
        if let Some(field) = self.fields.iter().find(|f| f.name == field_name) {
            field.protection == ProtectionLevel::User
        } else {
            false
        }
    }

    /// Check if a field can be modified based on its protection level
    ///
    /// Only `User` protected fields can be modified (type changes, etc.).
    /// Core/System fields are immutable.
    pub fn can_modify_field(&self, field_name: &str) -> bool {
        if let Some(field) = self.fields.iter().find(|f| f.name == field_name) {
            field.protection == ProtectionLevel::User
        } else {
            false
        }
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

    fn create_test_schema() -> SchemaDefinition {
        SchemaDefinition {
            is_core: true,
            version: 1,
            description: "Test schema".to_string(),
            fields: vec![
                SchemaField {
                    name: "status".to_string(),
                    field_type: "enum".to_string(),
                    protection: ProtectionLevel::Core,
                    core_values: Some(vec!["OPEN".to_string(), "DONE".to_string()]),
                    user_values: Some(vec!["BLOCKED".to_string()]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(true),
                    default: Some(json!("OPEN")),
                    description: Some("Task status".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "priority".to_string(),
                    field_type: "number".to_string(),
                    protection: ProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(json!(0)),
                    description: Some("Task priority".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
            ],
        }
    }

    #[test]
    fn test_get_enum_values() {
        let schema = create_test_schema();

        let values = schema.get_enum_values("status").unwrap();
        assert_eq!(values.len(), 3);
        assert!(values.contains(&"OPEN".to_string()));
        assert!(values.contains(&"DONE".to_string()));
        assert!(values.contains(&"BLOCKED".to_string()));

        assert!(schema.get_enum_values("priority").is_none());
        assert!(schema.get_enum_values("nonexistent").is_none());
    }

    #[test]
    fn test_can_delete_field() {
        let schema = create_test_schema();

        assert!(!schema.can_delete_field("status")); // Core field
        assert!(schema.can_delete_field("priority")); // User field
        assert!(!schema.can_delete_field("nonexistent")); // Doesn't exist
    }

    #[test]
    fn test_can_modify_field() {
        let schema = create_test_schema();

        assert!(!schema.can_modify_field("status")); // Core field
        assert!(schema.can_modify_field("priority")); // User field
        assert!(!schema.can_modify_field("nonexistent")); // Doesn't exist
    }

    #[test]
    fn test_serialization() {
        let schema = create_test_schema();
        let json = serde_json::to_value(&schema).unwrap();

        assert_eq!(json["is_core"], true);
        assert_eq!(json["version"], 1);
        assert_eq!(json["fields"][0]["name"], "status");
        assert_eq!(json["fields"][0]["protection"], "core");
    }

    #[test]
    fn test_deserialization() {
        let json = json!({
            "is_core": true,
            "version": 2,
            "description": "Task schema",
            "fields": [
                {
                    "name": "status",
                    "type": "enum",
                    "protection": "core",
                    "core_values": ["OPEN", "DONE"],
                    "indexed": true
                }
            ]
        });

        let schema: SchemaDefinition = serde_json::from_value(json).unwrap();
        assert_eq!(schema.version, 2);
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].protection, ProtectionLevel::Core);
    }

    #[test]
    fn test_nested_field_serialization() {
        let nested_schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Person with nested address".to_string(),
            fields: vec![SchemaField {
                name: "address".to_string(),
                field_type: "object".to_string(),
                protection: ProtectionLevel::User,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(false),
                extensible: None,
                default: None,
                description: Some("Address information".to_string()),
                item_type: None,
                fields: Some(vec![
                    SchemaField {
                        name: "street".to_string(),
                        field_type: "string".to_string(),
                        protection: ProtectionLevel::User,
                        core_values: None,
                        user_values: None,
                        indexed: false,
                        required: Some(false),
                        extensible: None,
                        default: None,
                        description: Some("Street address".to_string()),
                        item_type: None,
                        fields: None,
                        item_fields: None,
                    },
                    SchemaField {
                        name: "city".to_string(),
                        field_type: "string".to_string(),
                        protection: ProtectionLevel::User,
                        core_values: None,
                        user_values: None,
                        indexed: true,
                        required: Some(false),
                        extensible: None,
                        default: None,
                        description: Some("City".to_string()),
                        item_type: None,
                        fields: None,
                        item_fields: None,
                    },
                ]),
                item_fields: None,
            }],
        };

        let json = serde_json::to_value(&nested_schema).unwrap();
        assert_eq!(json["fields"][0]["name"], "address");
        assert_eq!(json["fields"][0]["type"], "object");
        assert_eq!(json["fields"][0]["fields"][0]["name"], "street");
        assert_eq!(json["fields"][0]["fields"][1]["name"], "city");
        assert_eq!(json["fields"][0]["fields"][1]["indexed"], true);
    }

    #[test]
    fn test_nested_field_deserialization() {
        let json = json!({
            "is_core": false,
            "version": 1,
            "description": "Person with nested address",
            "fields": [
                {
                    "name": "address",
                    "type": "object",
                    "protection": "user",
                    "indexed": false,
                    "fields": [
                        {
                            "name": "city",
                            "type": "string",
                            "protection": "user",
                            "indexed": true
                        }
                    ]
                }
            ]
        });

        let schema: SchemaDefinition = serde_json::from_value(json).unwrap();
        assert_eq!(schema.fields.len(), 1);
        assert_eq!(schema.fields[0].name, "address");

        let nested_fields = schema.fields[0].fields.as_ref().unwrap();
        assert_eq!(nested_fields.len(), 1);
        assert_eq!(nested_fields[0].name, "city");
        assert!(nested_fields[0].indexed);
    }

    #[test]
    fn test_array_of_objects_serialization() {
        let schema = SchemaDefinition {
            is_core: false,
            version: 1,
            description: "Person with contacts array".to_string(),
            fields: vec![SchemaField {
                name: "contacts".to_string(),
                field_type: "array".to_string(),
                protection: ProtectionLevel::User,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(false),
                extensible: None,
                default: None,
                description: Some("Contact list".to_string()),
                item_type: Some("object".to_string()),
                fields: None,
                item_fields: Some(vec![SchemaField {
                    name: "email".to_string(),
                    field_type: "string".to_string(),
                    protection: ProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Email address".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                }]),
            }],
        };

        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json["fields"][0]["name"], "contacts");
        assert_eq!(json["fields"][0]["type"], "array");
        assert_eq!(json["fields"][0]["item_type"], "object");
        assert_eq!(json["fields"][0]["item_fields"][0]["name"], "email");
        assert_eq!(json["fields"][0]["item_fields"][0]["indexed"], true);
    }
}
