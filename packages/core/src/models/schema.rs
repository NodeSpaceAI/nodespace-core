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
//!   "nodeType": "schema",
//!   "content": "Task",
//!   "isCore": true,
//!   "schemaVersion": 2,
//!   "description": "Task tracking schema",
//!   "fields": [
//!     {
//!       "name": "status",
//!       "type": "enum",
//!       "protection": "core",
//!       "coreValues": [
//!         { "value": "open", "label": "Open" },
//!         { "value": "in_progress", "label": "In Progress" },
//!         { "value": "done", "label": "Done" }
//!       ],
//!       "userValues": [
//!         { "value": "blocked", "label": "Blocked" }
//!       ],
//!       "extensible": true,
//!       "indexed": true,
//!       "required": true,
//!       "default": "open"
//!     }
//!   ]
//! }
//! ```

use serde::{Deserialize, Serialize};

/// A single enum value with its display label
///
/// Used for enum fields to provide both the stored value and a human-readable label.
/// Array order determines display order in UI dropdowns.
///
/// ## Example
/// ```json
/// { "value": "in_progress", "label": "In Progress" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnumValue {
    /// The actual value stored in the database
    pub value: String,
    /// Human-readable display label for UI/MCP clients
    pub label: String,
}

/// Protection level for schema fields
///
/// Determines whether a field can be modified or deleted by users.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SchemaProtectionLevel {
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
///
/// Uses camelCase serialization to match frontend TypeScript conventions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaField {
    /// Field name (must be unique within schema)
    pub name: String,

    /// Field type (e.g., "string", "number", "boolean", "enum", "array", "object")
    #[serde(rename = "type")]
    pub field_type: String,

    /// Protection level determining mutability
    pub protection: SchemaProtectionLevel,

    /// Protected enum values (cannot be removed) - enum fields only
    /// Array order determines display order in UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_values: Option<Vec<EnumValue>>,

    /// User-extensible enum values (can be added/removed) - enum fields only
    /// These appear after core_values in display order.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_values: Option<Vec<EnumValue>>,

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_test_field() -> SchemaField {
        SchemaField {
            name: "status".to_string(),
            field_type: "enum".to_string(),
            protection: SchemaProtectionLevel::Core,
            core_values: Some(vec![
                EnumValue {
                    value: "open".to_string(),
                    label: "Open".to_string(),
                },
                EnumValue {
                    value: "done".to_string(),
                    label: "Done".to_string(),
                },
            ]),
            user_values: Some(vec![EnumValue {
                value: "blocked".to_string(),
                label: "Blocked".to_string(),
            }]),
            indexed: true,
            required: Some(true),
            extensible: Some(true),
            default: Some(json!("open")),
            description: Some("Task status".to_string()),
            item_type: None,
            fields: None,
            item_fields: None,
        }
    }

    #[test]
    fn test_schema_field_serialization() {
        let field = create_test_field();
        let json = serde_json::to_value(&field).unwrap();

        assert_eq!(json["name"], "status");
        assert_eq!(json["protection"], "core");
        // field_type serializes to "type" due to #[serde(rename = "type")]
        assert_eq!(json["type"], "enum");
        // core_values serializes to coreValues
        assert!(json["coreValues"].is_array());
        assert_eq!(json["indexed"], true);
    }

    #[test]
    fn test_schema_field_deserialization() {
        let json = json!({
            "name": "status",
            "type": "enum",
            "protection": "core",
            "coreValues": [
                { "value": "open", "label": "Open" },
                { "value": "done", "label": "Done" }
            ],
            "indexed": true
        });

        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "status");
        assert_eq!(field.field_type, "enum");
        assert_eq!(field.protection, SchemaProtectionLevel::Core);
        assert!(field.indexed);

        let core_values = field.core_values.unwrap();
        assert_eq!(core_values.len(), 2);
        assert_eq!(core_values[0].value, "open");
        assert_eq!(core_values[0].label, "Open");
    }

    #[test]
    fn test_protection_level_serialization() {
        assert_eq!(
            serde_json::to_value(&SchemaProtectionLevel::Core).unwrap(),
            "core"
        );
        assert_eq!(
            serde_json::to_value(&SchemaProtectionLevel::User).unwrap(),
            "user"
        );
        assert_eq!(
            serde_json::to_value(&SchemaProtectionLevel::System).unwrap(),
            "system"
        );
    }

    #[test]
    fn test_nested_field_serialization() {
        let address_field = SchemaField {
            name: "address".to_string(),
            field_type: "object".to_string(),
            protection: SchemaProtectionLevel::User,
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
                    protection: SchemaProtectionLevel::User,
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
                    protection: SchemaProtectionLevel::User,
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
        };

        let json = serde_json::to_value(&address_field).unwrap();
        assert_eq!(json["name"], "address");
        assert_eq!(json["type"], "object");
        assert_eq!(json["fields"][0]["name"], "street");
        assert_eq!(json["fields"][1]["name"], "city");
        assert_eq!(json["fields"][1]["indexed"], true);
    }

    #[test]
    fn test_nested_field_deserialization() {
        let json = json!({
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
        });

        let field: SchemaField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "address");
        assert_eq!(field.field_type, "object");

        let nested_fields = field.fields.as_ref().unwrap();
        assert_eq!(nested_fields.len(), 1);
        assert_eq!(nested_fields[0].name, "city");
        assert!(nested_fields[0].indexed);
    }

    #[test]
    fn test_array_of_objects_serialization() {
        let contacts_field = SchemaField {
            name: "contacts".to_string(),
            field_type: "array".to_string(),
            protection: SchemaProtectionLevel::User,
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
                protection: SchemaProtectionLevel::User,
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
        };

        let json = serde_json::to_value(&contacts_field).unwrap();
        assert_eq!(json["name"], "contacts");
        assert_eq!(json["type"], "array");
        // item_type serializes to itemType with camelCase
        assert_eq!(json["itemType"], "object");
        // item_fields serializes to itemFields with camelCase
        assert_eq!(json["itemFields"][0]["name"], "email");
        assert_eq!(json["itemFields"][0]["indexed"], true);
    }
}
