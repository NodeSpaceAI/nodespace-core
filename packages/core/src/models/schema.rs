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
//!   ],
//!   "relationships": [
//!     {
//!       "name": "assigned_to",
//!       "targetType": "person",
//!       "direction": "out",
//!       "cardinality": "many",
//!       "reverseName": "tasks",
//!       "reverseCardinality": "many",
//!       "edgeFields": [
//!         { "name": "role", "type": "string" }
//!       ]
//!     }
//!   ]
//! }
//! ```
//!
//! ## Relationships
//!
//! Schemas can define relationships to other node types. Relationships are stored
//! in edge tables and support:
//!
//! - **Edge table storage**: Edge table is single source of truth
//! - **Bidirectional querying**: Both directions query the same edge table
//! - **Edge fields**: Custom properties on the relationship itself
//! - **Cardinality**: "one" or "many" constraints (enforced at application level)
//!
//! See [`docs/architecture/data/schema-relational-fields.md`] for complete details.

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

/// A field stored on an edge (relationship) between nodes
///
/// Edge fields are simpler than schema fields - they don't support:
/// - Protection levels (all edge fields are user-defined)
/// - Nested types (edge fields use primitives only)
/// - Enum extension (no coreValues/userValues)
///
/// Edge fields live/die with the relationship definition.
///
/// ## Example
/// ```json
/// {
///   "name": "role",
///   "type": "string",
///   "indexed": true,
///   "required": false,
///   "description": "The role of this assignment"
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EdgeField {
    /// Field name (must be unique within the edge)
    pub name: String,

    /// Field type: "string" | "number" | "boolean" | "date" | "record"
    #[serde(rename = "type")]
    pub field_type: String,

    /// Whether to create an index on this field
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed: Option<bool>,

    /// Whether this field is required for new edges
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Default value for new edges
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,

    /// Target type for record fields (e.g., "person")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_type: Option<String>,

    /// Human-readable description (for NLP)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Definition of a relationship between node types
///
/// Relationships create edge tables in SurrealDB. Both directions of a
/// relationship query the same edge table. Cardinality constraints are
/// enforced at the application level during relationship creation.
///
/// ## Edge Table Naming
/// Edge tables are named: `{source_type}_{relationship_name}_{target_type}`
/// Example: `invoice_billed_to_customer`
///
/// ## Example
/// ```json
/// {
///   "name": "billed_to",
///   "targetType": "customer",
///   "direction": "out",
///   "cardinality": "one",
///   "required": true,
///   "reverseName": "invoices",
///   "reverseCardinality": "many",
///   "edgeFields": [
///     { "name": "billing_date", "type": "date", "required": true },
///     { "name": "payment_terms", "type": "string" }
///   ]
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaRelationship {
    /// Relationship name (e.g., "billed_to", "assigned_to")
    ///
    /// Must start with a letter and contain only alphanumeric characters,
    /// underscores, and hyphens. Reserved names: has_child, mentions, node, data
    pub name: String,

    /// Target node type (e.g., "customer", "person")
    ///
    /// When `None`, the relationship accepts any target node type (untyped/generic).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub target_type: Option<String>,

    /// Direction: "out" (this->target) or "in" (target->this)
    ///
    /// Most relationships use "out" direction. The "in" direction is for
    /// cases where the relationship is conceptually owned by the target.
    pub direction: RelationshipDirection,

    /// Cardinality: "one" or "many"
    ///
    /// Enforced at application level when creating edges.
    /// - "one": Only one edge allowed from source to any target
    /// - "many": Multiple edges allowed from source to different targets
    pub cardinality: RelationshipCardinality,

    /// Whether this relationship is required for new nodes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,

    /// Suggested reverse relationship name (for NLP discovery)
    ///
    /// This does NOT mutate the target schema. It's metadata for NLP to
    /// understand how to describe the relationship from the target's perspective.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverse_name: Option<String>,

    /// Reverse cardinality (from target's perspective)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverse_cardinality: Option<RelationshipCardinality>,

    /// Auto-computed edge table name (can be overridden)
    ///
    /// If not specified, computed as: `{source_type}_{name}_{target_type}` when `target_type` is set,
    /// or `{source_type}_{name}` when `target_type` is `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_table: Option<String>,

    /// Fields stored on the edge itself
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_fields: Option<Vec<EdgeField>>,

    /// Human-readable description (for NLP)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl SchemaRelationship {
    /// Compute the edge table name for this relationship
    ///
    /// Returns the explicit `edge_table` if set, otherwise computes it as:
    /// `{source_type}_{relationship_name}_{target_type}`
    pub fn compute_edge_table_name(&self, source_type: &str) -> String {
        self.edge_table
            .clone()
            .unwrap_or_else(|| match &self.target_type {
                Some(target) => format!("{}_{}_{}", source_type, self.name, target),
                None => format!("{}_{}", source_type, self.name),
            })
    }
}

/// Direction of a relationship
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipDirection {
    /// Outgoing relationship: this node -> target node
    Out,
    /// Incoming relationship: target node -> this node
    In,
}

/// Cardinality constraint for relationships
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipCardinality {
    /// Only one edge allowed (e.g., invoice can only be billed to ONE customer)
    One,
    /// Multiple edges allowed (e.g., task can be assigned to MANY people)
    Many,
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

    // =========================================================================
    // EdgeField Tests
    // =========================================================================

    #[test]
    fn test_edge_field_serialization() {
        let field = EdgeField {
            name: "role".to_string(),
            field_type: "string".to_string(),
            indexed: Some(true),
            required: Some(false),
            default: Some(json!("member")),
            target_type: None,
            description: Some("Assignment role".to_string()),
        };

        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["name"], "role");
        assert_eq!(json["type"], "string");
        assert_eq!(json["indexed"], true);
        assert_eq!(json["required"], false);
        assert_eq!(json["default"], "member");
        assert_eq!(json["description"], "Assignment role");
        // target_type should be absent (skip_serializing_if = None)
        assert!(json.get("targetType").is_none());
    }

    #[test]
    fn test_edge_field_deserialization() {
        let json = json!({
            "name": "billing_date",
            "type": "date",
            "required": true,
            "indexed": true
        });

        let field: EdgeField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "billing_date");
        assert_eq!(field.field_type, "date");
        assert_eq!(field.required, Some(true));
        assert_eq!(field.indexed, Some(true));
        assert!(field.default.is_none());
        assert!(field.target_type.is_none());
        assert!(field.description.is_none());
    }

    #[test]
    fn test_edge_field_with_record_type() {
        let field = EdgeField {
            name: "approved_by".to_string(),
            field_type: "record".to_string(),
            indexed: Some(true),
            required: None,
            default: None,
            target_type: Some("person".to_string()),
            description: Some("Who approved this".to_string()),
        };

        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["type"], "record");
        assert_eq!(json["targetType"], "person");
    }

    #[test]
    fn test_edge_field_minimal() {
        // Test minimal edge field (only required fields)
        let json = json!({
            "name": "simple",
            "type": "string"
        });

        let field: EdgeField = serde_json::from_value(json).unwrap();
        assert_eq!(field.name, "simple");
        assert_eq!(field.field_type, "string");
        assert!(field.indexed.is_none());
        assert!(field.required.is_none());
        assert!(field.default.is_none());
    }

    // =========================================================================
    // RelationshipDirection Tests
    // =========================================================================

    #[test]
    fn test_relationship_direction_serialization() {
        assert_eq!(
            serde_json::to_value(&RelationshipDirection::Out).unwrap(),
            "out"
        );
        assert_eq!(
            serde_json::to_value(&RelationshipDirection::In).unwrap(),
            "in"
        );
    }

    #[test]
    fn test_relationship_direction_deserialization() {
        let out: RelationshipDirection = serde_json::from_value(json!("out")).unwrap();
        assert_eq!(out, RelationshipDirection::Out);

        let r#in: RelationshipDirection = serde_json::from_value(json!("in")).unwrap();
        assert_eq!(r#in, RelationshipDirection::In);
    }

    // =========================================================================
    // RelationshipCardinality Tests
    // =========================================================================

    #[test]
    fn test_relationship_cardinality_serialization() {
        assert_eq!(
            serde_json::to_value(&RelationshipCardinality::One).unwrap(),
            "one"
        );
        assert_eq!(
            serde_json::to_value(&RelationshipCardinality::Many).unwrap(),
            "many"
        );
    }

    #[test]
    fn test_relationship_cardinality_deserialization() {
        let one: RelationshipCardinality = serde_json::from_value(json!("one")).unwrap();
        assert_eq!(one, RelationshipCardinality::One);

        let many: RelationshipCardinality = serde_json::from_value(json!("many")).unwrap();
        assert_eq!(many, RelationshipCardinality::Many);
    }

    // =========================================================================
    // SchemaRelationship Tests
    // =========================================================================

    #[test]
    fn test_schema_relationship_serialization() {
        let relationship = SchemaRelationship {
            name: "billed_to".to_string(),
            target_type: Some("customer".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::One,
            required: Some(true),
            reverse_name: Some("invoices".to_string()),
            reverse_cardinality: Some(RelationshipCardinality::Many),
            edge_table: None,
            edge_fields: Some(vec![
                EdgeField {
                    name: "billing_date".to_string(),
                    field_type: "date".to_string(),
                    indexed: Some(true),
                    required: Some(true),
                    default: None,
                    target_type: None,
                    description: None,
                },
                EdgeField {
                    name: "payment_terms".to_string(),
                    field_type: "string".to_string(),
                    indexed: None,
                    required: None,
                    default: Some(json!("net-30")),
                    target_type: None,
                    description: None,
                },
            ]),
            description: Some("Customer this invoice is billed to".to_string()),
        };

        let json = serde_json::to_value(&relationship).unwrap();

        assert_eq!(json["name"], "billed_to");
        assert_eq!(json["targetType"], "customer");
        assert_eq!(json["direction"], "out");
        assert_eq!(json["cardinality"], "one");
        assert_eq!(json["required"], true);
        assert_eq!(json["reverseName"], "invoices");
        assert_eq!(json["reverseCardinality"], "many");
        assert!(json.get("edgeTable").is_none()); // Not set, should be absent
        assert_eq!(json["edgeFields"].as_array().unwrap().len(), 2);
        assert_eq!(json["edgeFields"][0]["name"], "billing_date");
        assert_eq!(json["edgeFields"][1]["default"], "net-30");
    }

    #[test]
    fn test_schema_relationship_deserialization() {
        let json = json!({
            "name": "assigned_to",
            "targetType": "person",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "tasks",
            "reverseCardinality": "many",
            "edgeFields": [
                {
                    "name": "role",
                    "type": "string",
                    "indexed": true
                },
                {
                    "name": "assigned_at",
                    "type": "date",
                    "required": true
                }
            ]
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();

        assert_eq!(relationship.name, "assigned_to");
        assert_eq!(relationship.target_type, Some("person".to_string()));
        assert_eq!(relationship.direction, RelationshipDirection::Out);
        assert_eq!(relationship.cardinality, RelationshipCardinality::Many);
        assert_eq!(relationship.reverse_name, Some("tasks".to_string()));
        assert_eq!(
            relationship.reverse_cardinality,
            Some(RelationshipCardinality::Many)
        );
        assert!(relationship.required.is_none());
        assert!(relationship.edge_table.is_none());

        let edge_fields = relationship.edge_fields.unwrap();
        assert_eq!(edge_fields.len(), 2);
        assert_eq!(edge_fields[0].name, "role");
        assert_eq!(edge_fields[1].name, "assigned_at");
    }

    #[test]
    fn test_schema_relationship_minimal() {
        // Test minimal relationship (only required fields)
        let json = json!({
            "name": "parent_of",
            "targetType": "document",
            "direction": "out",
            "cardinality": "many"
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();

        assert_eq!(relationship.name, "parent_of");
        assert_eq!(relationship.target_type, Some("document".to_string()));
        assert_eq!(relationship.direction, RelationshipDirection::Out);
        assert_eq!(relationship.cardinality, RelationshipCardinality::Many);
        assert!(relationship.required.is_none());
        assert!(relationship.reverse_name.is_none());
        assert!(relationship.reverse_cardinality.is_none());
        assert!(relationship.edge_table.is_none());
        assert!(relationship.edge_fields.is_none());
        assert!(relationship.description.is_none());
    }

    #[test]
    fn test_schema_relationship_with_custom_edge_table() {
        let relationship = SchemaRelationship {
            name: "collaborates_with".to_string(),
            target_type: Some("person".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: Some("collaborations".to_string()),
            edge_fields: None,
            description: None,
        };

        let json = serde_json::to_value(&relationship).unwrap();
        assert_eq!(json["edgeTable"], "collaborations");
    }

    #[test]
    fn test_compute_edge_table_name_auto() {
        let relationship = SchemaRelationship {
            name: "billed_to".to_string(),
            target_type: Some("customer".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::One,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: None,
            edge_fields: None,
            description: None,
        };

        let edge_table = relationship.compute_edge_table_name("invoice");
        assert_eq!(edge_table, "invoice_billed_to_customer");
    }

    #[test]
    fn test_compute_edge_table_name_explicit() {
        let relationship = SchemaRelationship {
            name: "assigned_to".to_string(),
            target_type: Some("person".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: Some("assignments".to_string()),
            edge_fields: None,
            description: None,
        };

        let edge_table = relationship.compute_edge_table_name("task");
        assert_eq!(edge_table, "assignments"); // Uses explicit name, ignores source_type
    }

    #[test]
    fn test_schema_relationship_incoming_direction() {
        // Test "in" direction (less common but valid)
        let json = json!({
            "name": "owned_by",
            "targetType": "organization",
            "direction": "in",
            "cardinality": "one"
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();
        assert_eq!(relationship.direction, RelationshipDirection::In);
    }

    #[test]
    fn test_schema_relationship_untyped_deserialization() {
        // target_type absent → None (untyped/generic relationship)
        let json = json!({
            "name": "related",
            "direction": "out",
            "cardinality": "many"
        });

        let relationship: SchemaRelationship = serde_json::from_value(json).unwrap();
        assert_eq!(relationship.name, "related");
        assert!(relationship.target_type.is_none());
    }

    #[test]
    fn test_schema_relationship_untyped_serialization() {
        let relationship = SchemaRelationship {
            name: "related".to_string(),
            target_type: None,
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: None,
            edge_fields: None,
            description: None,
        };

        let json = serde_json::to_value(&relationship).unwrap();
        assert_eq!(json["name"], "related");
        // targetType absent when None
        assert!(json.get("targetType").is_none());
    }

    #[test]
    fn test_compute_edge_table_name_none_target() {
        // When target_type is None, table name is {source}_{name}
        let relationship = SchemaRelationship {
            name: "related".to_string(),
            target_type: None,
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::Many,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: None,
            edge_fields: None,
            description: None,
        };

        let edge_table = relationship.compute_edge_table_name("note");
        assert_eq!(edge_table, "note_related");
    }

    #[test]
    fn test_compute_edge_table_name_some_target() {
        // When target_type is Some, table name is {source}_{name}_{target}
        let relationship = SchemaRelationship {
            name: "billed_to".to_string(),
            target_type: Some("customer".to_string()),
            direction: RelationshipDirection::Out,
            cardinality: RelationshipCardinality::One,
            required: None,
            reverse_name: None,
            reverse_cardinality: None,
            edge_table: None,
            edge_fields: None,
            description: None,
        };

        let edge_table = relationship.compute_edge_table_name("invoice");
        assert_eq!(edge_table, "invoice_billed_to_customer");
    }
}
