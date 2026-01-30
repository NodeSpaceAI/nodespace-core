//! Core Schema Definitions
//!
//! This module contains the canonical definitions for all core schemas in NodeSpace.
//! These are the schemas that ship with the application and cannot be modified by users.
//!
//! ## Core Schemas
//!
//! - **task** - Task tracking with status, priority, dates
//! - **text** - Plain text content
//! - **date** - Daily note containers
//! - **header** - Markdown headers (h1-h6)
//! - **code-block** - Code blocks with syntax highlighting
//! - **quote-block** - Blockquotes for citations
//! - **ordered-list** - Numbered list items
//!
//! ## Usage
//!
//! Call `get_core_schemas()` to get all core schema definitions.

use crate::models::schema::{EnumValue, SchemaField, SchemaProtectionLevel};
use crate::models::SchemaNode;
use chrono::Utc;

/// Get all core schema definitions as SchemaNode instances
///
/// Returns the 7 core schemas (task, text, date, header, code-block, quote-block, ordered-list)
/// ready to be converted to Node via `schema.into_node()` for database seeding.
pub fn get_core_schemas() -> Vec<SchemaNode> {
    let now = Utc::now();

    vec![
        // Task schema with status, priority, dates, and assignee
        SchemaNode {
            id: "task".to_string(),
            content: "Task".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Task tracking schema".to_string(),
            fields: vec![
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
                            value: "in_progress".to_string(),
                            label: "In Progress".to_string(),
                        },
                        EnumValue {
                            value: "done".to_string(),
                            label: "Done".to_string(),
                        },
                        EnumValue {
                            value: "cancelled".to_string(),
                            label: "Cancelled".to_string(),
                        },
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(true),
                    default: Some(serde_json::json!("open")),
                    description: Some("Task status".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "priority".to_string(),
                    field_type: "enum".to_string(),
                    protection: SchemaProtectionLevel::User,
                    core_values: Some(vec![
                        EnumValue {
                            value: "low".to_string(),
                            label: "Low".to_string(),
                        },
                        EnumValue {
                            value: "medium".to_string(),
                            label: "Medium".to_string(),
                        },
                        EnumValue {
                            value: "high".to_string(),
                            label: "High".to_string(),
                        },
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(false),
                    extensible: Some(true),
                    default: None,
                    description: Some("Task priority".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "due_date".to_string(),
                    field_type: "date".to_string(),
                    protection: SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Due date".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "started_at".to_string(),
                    field_type: "date".to_string(),
                    protection: SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Started at".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "completed_at".to_string(),
                    field_type: "date".to_string(),
                    protection: SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Completed at".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "assignee".to_string(),
                    field_type: "text".to_string(),
                    protection: SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Assignee".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
            ],
            relationships: vec![],
        },
        // Text schema - plain text content (no extra fields)
        SchemaNode {
            id: "text".to_string(),
            content: "Text".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Plain text content".to_string(),
            fields: vec![],
            relationships: vec![],
        },
        // Date schema - daily note containers (no extra fields)
        SchemaNode {
            id: "date".to_string(),
            content: "Date".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Date node schema".to_string(),
            fields: vec![],
            relationships: vec![],
        },
        // Header schema - markdown headers (no extra fields)
        SchemaNode {
            id: "header".to_string(),
            content: "Header".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Markdown header (h1-h6)".to_string(),
            fields: vec![],
            relationships: vec![],
        },
        // Code block schema - code with syntax highlighting (no extra fields)
        SchemaNode {
            id: "code-block".to_string(),
            content: "Code Block".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Code block with syntax highlighting".to_string(),
            fields: vec![],
            relationships: vec![],
        },
        // Quote block schema - blockquotes (no extra fields)
        SchemaNode {
            id: "quote-block".to_string(),
            content: "Quote Block".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Blockquote for citations".to_string(),
            fields: vec![],
            relationships: vec![],
        },
        // Ordered list schema - numbered list items (no extra fields)
        SchemaNode {
            id: "ordered-list".to_string(),
            content: "Ordered List".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Numbered list item".to_string(),
            fields: vec![],
            relationships: vec![],
        },
        // Collection schema - hierarchical labels for organizing nodes
        SchemaNode {
            id: "collection".to_string(),
            content: "Collection".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Hierarchical label for organizing nodes into groups".to_string(),
            fields: vec![],        // Uses content for name
            relationships: vec![], // member_of is a native edge, not schema-defined
        },
        // Query schema - saved query definitions
        SchemaNode {
            id: "query".to_string(),
            content: "Query".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Query definition for filtering and searching nodes".to_string(),
            fields: vec![
                SchemaField {
                    name: "target_type".to_string(),
                    field_type: "text".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(true),
                    extensible: None,
                    default: Some(serde_json::json!("*")),
                    description: Some("Target node type to query (* for all)".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "filters".to_string(),
                    field_type: "array".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(true),
                    extensible: None,
                    default: Some(serde_json::json!([])),
                    description: Some("Filter conditions array".to_string()),
                    item_type: Some("object".to_string()),
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "sorting".to_string(),
                    field_type: "array".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Sorting configuration array".to_string()),
                    item_type: Some("object".to_string()),
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "limit".to_string(),
                    field_type: "number".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!(50)),
                    description: Some("Result limit".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "generated_by".to_string(),
                    field_type: "enum".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue {
                            value: "ai".to_string(),
                            label: "AI Generated".to_string(),
                        },
                        EnumValue {
                            value: "user".to_string(),
                            label: "User Created".to_string(),
                        },
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(false),
                    default: Some(serde_json::json!("user")),
                    description: Some("Who created the query".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "generator_context".to_string(),
                    field_type: "text".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Parent chat ID for AI-generated queries".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "execution_count".to_string(),
                    field_type: "number".to_string(),
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!(0)),
                    description: Some("Number of times query has been executed".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "last_executed".to_string(),
                    field_type: "date".to_string(),
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Timestamp of last execution".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
            ],
            relationships: vec![],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_core_schemas_returns_all_nine() {
        let schemas = get_core_schemas();
        assert_eq!(schemas.len(), 9);
    }

    #[test]
    fn test_all_schemas_are_core() {
        let schemas = get_core_schemas();
        for schema in &schemas {
            assert!(schema.is_core, "Schema {} should be core", schema.id);
        }
    }

    #[test]
    fn test_task_schema_has_fields() {
        let schemas = get_core_schemas();
        let task = schemas.iter().find(|s| s.id == "task").unwrap();

        assert_eq!(task.fields.len(), 6);
        assert!(task.get_field("status").is_some());
        assert!(task.get_field("priority").is_some());
        assert!(task.get_field("due_date").is_some());
    }

    #[test]
    fn test_simple_schemas_have_no_fields() {
        let schemas = get_core_schemas();

        for id in &[
            "text",
            "date",
            "header",
            "code-block",
            "quote-block",
            "ordered-list",
            "collection",
        ] {
            let schema = schemas.iter().find(|s| s.id == *id).unwrap();
            assert!(
                schema.fields.is_empty(),
                "Schema {} should have no fields",
                id
            );
        }
    }

    #[test]
    fn test_query_schema_has_fields() {
        let schemas = get_core_schemas();
        let query = schemas.iter().find(|s| s.id == "query").unwrap();

        assert_eq!(query.fields.len(), 8);
        assert!(query.get_field("target_type").is_some());
        assert!(query.get_field("filters").is_some());
        assert!(query.get_field("sorting").is_some());
        assert!(query.get_field("limit").is_some());
        assert!(query.get_field("generated_by").is_some());
        assert!(query.get_field("generator_context").is_some());
        assert!(query.get_field("execution_count").is_some());
        assert!(query.get_field("last_executed").is_some());
    }

    #[test]
    fn test_schemas_convert_to_node() {
        let schemas = get_core_schemas();
        for schema in schemas {
            let node = schema.into_node();
            assert_eq!(node.node_type, "schema");
            assert!(node.properties.get("isCore").unwrap().as_bool().unwrap());
        }
    }
}
