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
//! - **checkbox** - Checkbox items
//! - **query** - Query/search nodes
//! - **collection** - Collection containers
//! - **horizontal-line** - Horizontal rule / thematic break
//! - **table** - GFM markdown table
//!
//! ## Usage
//!
//! Call `get_core_schemas()` to get all core schema definitions.

use crate::models::schema::{EnumValue, SchemaField, SchemaProtectionLevel};
use crate::models::SchemaNode;
use chrono::Utc;

/// Get all core schema definitions as SchemaNode instances
///
/// Returns all core schemas ready to be converted to Node via `schema.into_node()`
/// for database seeding.
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
            title_template: None,
            properties_header_summary_template: None,
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
            title_template: None,
            properties_header_summary_template: None,
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
            title_template: None,
            properties_header_summary_template: None,
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
            title_template: None,
            properties_header_summary_template: None,
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
            title_template: None,
            properties_header_summary_template: None,
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
            title_template: None,
            properties_header_summary_template: None,
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
            title_template: None,
            properties_header_summary_template: None,
        },
        // Horizontal line schema - thematic break (no extra fields)
        SchemaNode {
            id: "horizontal-line".to_string(),
            content: "Horizontal Line".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Horizontal rule / thematic break".to_string(),
            fields: vec![],
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
        },
        // Table schema - GFM markdown table (no extra fields)
        SchemaNode {
            id: "table".to_string(),
            content: "Table".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "GFM markdown table with alignment support".to_string(),
            fields: vec![],
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
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
            title_template: None,
            properties_header_summary_template: None,
        },
        // Checkbox schema - pure content node with state encoded in content string
        SchemaNode {
            id: "checkbox".to_string(),
            content: "Checkbox".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "Checkbox item — markdown annotation, not a managed task".to_string(),
            fields: vec![],
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
        },
        // AI Chat schema - conversation nodes with messages as nested properties
        SchemaNode {
            id: "ai-chat".to_string(),
            content: "AI Chat".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "AI conversation node with messages stored as nested properties"
                .to_string(),
            fields: vec![
                SchemaField {
                    name: "provider".to_string(),
                    field_type: "enum".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue {
                            value: "native".to_string(),
                            label: "Native (Local)".to_string(),
                        },
                        EnumValue {
                            value: "anthropic".to_string(),
                            label: "Anthropic".to_string(),
                        },
                        EnumValue {
                            value: "gemini".to_string(),
                            label: "Gemini".to_string(),
                        },
                        EnumValue {
                            value: "mistral".to_string(),
                            label: "Mistral".to_string(),
                        },
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(true),
                    default: Some(serde_json::json!("native")),
                    description: Some("AI provider for this conversation".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "model".to_string(),
                    field_type: "text".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Model identifier used for this conversation".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "status".to_string(),
                    field_type: "enum".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue {
                            value: "active".to_string(),
                            label: "Active".to_string(),
                        },
                        EnumValue {
                            value: "archived".to_string(),
                            label: "Archived".to_string(),
                        },
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(false),
                    default: Some(serde_json::json!("active")),
                    description: Some("Conversation status".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "last_active".to_string(),
                    field_type: "date".to_string(),
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Timestamp of last activity".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "context_tokens".to_string(),
                    field_type: "number".to_string(),
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!(0)),
                    description: Some(
                        "Approximate token count of conversation context".to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "created_nodes".to_string(),
                    field_type: "array".to_string(),
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!([])),
                    description: Some(
                        "IDs of nodes created by the agent during this chat".to_string(),
                    ),
                    item_type: Some("text".to_string()),
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "messages".to_string(),
                    field_type: "array".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(true),
                    extensible: None,
                    default: Some(serde_json::json!([])),
                    description: Some("Conversation messages array".to_string()),
                    item_type: Some("object".to_string()),
                    fields: None,
                    item_fields: Some(vec![
                        SchemaField {
                            name: "role".to_string(),
                            field_type: "enum".to_string(),
                            protection: SchemaProtectionLevel::Core,
                            core_values: Some(vec![
                                EnumValue {
                                    value: "user".to_string(),
                                    label: "User".to_string(),
                                },
                                EnumValue {
                                    value: "assistant".to_string(),
                                    label: "Assistant".to_string(),
                                },
                                EnumValue {
                                    value: "tool_call".to_string(),
                                    label: "Tool Call".to_string(),
                                },
                                EnumValue {
                                    value: "system".to_string(),
                                    label: "System".to_string(),
                                },
                            ]),
                            user_values: Some(vec![]),
                            indexed: false,
                            required: Some(true),
                            extensible: Some(false),
                            default: None,
                            description: Some("Message sender role".to_string()),
                            item_type: None,
                            fields: None,
                            item_fields: None,
                        },
                        SchemaField {
                            name: "content".to_string(),
                            field_type: "text".to_string(),
                            protection: SchemaProtectionLevel::Core,
                            core_values: None,
                            user_values: None,
                            indexed: false,
                            required: Some(false),
                            extensible: None,
                            default: None,
                            description: Some("Message text content".to_string()),
                            item_type: None,
                            fields: None,
                            item_fields: None,
                        },
                        SchemaField {
                            name: "timestamp".to_string(),
                            field_type: "date".to_string(),
                            protection: SchemaProtectionLevel::System,
                            core_values: None,
                            user_values: None,
                            indexed: false,
                            required: Some(false),
                            extensible: None,
                            default: None,
                            description: Some("Message timestamp".to_string()),
                            item_type: None,
                            fields: None,
                            item_fields: None,
                        },
                        SchemaField {
                            name: "referenced_nodes".to_string(),
                            field_type: "array".to_string(),
                            protection: SchemaProtectionLevel::Core,
                            core_values: None,
                            user_values: None,
                            indexed: false,
                            required: Some(false),
                            extensible: None,
                            default: None,
                            description: Some("Node IDs referenced in this message".to_string()),
                            item_type: Some("text".to_string()),
                            fields: None,
                            item_fields: None,
                        },
                        SchemaField {
                            name: "tool".to_string(),
                            field_type: "text".to_string(),
                            protection: SchemaProtectionLevel::Core,
                            core_values: None,
                            user_values: None,
                            indexed: false,
                            required: Some(false),
                            extensible: None,
                            default: None,
                            description: Some(
                                "Tool name (for tool_call role messages)".to_string(),
                            ),
                            item_type: None,
                            fields: None,
                            item_fields: None,
                        },
                        SchemaField {
                            name: "args".to_string(),
                            field_type: "object".to_string(),
                            protection: SchemaProtectionLevel::Core,
                            core_values: None,
                            user_values: None,
                            indexed: false,
                            required: Some(false),
                            extensible: None,
                            default: None,
                            description: Some(
                                "Tool call arguments (for tool_call role messages)".to_string(),
                            ),
                            item_type: None,
                            fields: None,
                            item_fields: None,
                        },
                        SchemaField {
                            name: "status".to_string(),
                            field_type: "enum".to_string(),
                            protection: SchemaProtectionLevel::Core,
                            core_values: Some(vec![
                                EnumValue {
                                    value: "completed".to_string(),
                                    label: "Completed".to_string(),
                                },
                                EnumValue {
                                    value: "error".to_string(),
                                    label: "Error".to_string(),
                                },
                            ]),
                            user_values: Some(vec![]),
                            indexed: false,
                            required: Some(false),
                            extensible: Some(false),
                            default: None,
                            description: Some(
                                "Tool execution status (for tool_call role messages)".to_string(),
                            ),
                            item_type: None,
                            fields: None,
                            item_fields: None,
                        },
                        SchemaField {
                            name: "result_summary".to_string(),
                            field_type: "text".to_string(),
                            protection: SchemaProtectionLevel::Core,
                            core_values: None,
                            user_values: None,
                            indexed: false,
                            required: Some(false),
                            extensible: None,
                            default: None,
                            description: Some(
                                "Archived summary of tool result (full result nulled at write time)"
                                    .to_string(),
                            ),
                            item_type: None,
                            fields: None,
                            item_fields: None,
                        },
                        SchemaField {
                            name: "duration_ms".to_string(),
                            field_type: "number".to_string(),
                            protection: SchemaProtectionLevel::System,
                            core_values: None,
                            user_values: None,
                            indexed: false,
                            required: Some(false),
                            extensible: None,
                            default: None,
                            description: Some(
                                "Duration of tool execution in milliseconds".to_string(),
                            ),
                            item_type: None,
                            fields: None,
                            item_fields: None,
                        },
                    ]),
                },
            ],
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
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
            title_template: None,
            properties_header_summary_template: None,
        },
        // Prompt schema for AI agent prompts (ADR-030)
        SchemaNode {
            id: "prompt".to_string(),
            content: "Prompt".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            description: "AI agent prompt template".to_string(),
            fields: vec![
                SchemaField {
                    name: "priority".to_string(),
                    field_type: "number".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!(100)),
                    description: Some("Assembly ordering priority (lower = earlier)".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "template_syntax".to_string(),
                    field_type: "enum".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue {
                            value: "plain".to_string(),
                            label: "Plain Text".to_string(),
                        },
                        EnumValue {
                            value: "minijinja".to_string(),
                            label: "Minijinja Template".to_string(),
                        },
                    ]),
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: Some(false),
                    default: Some(serde_json::json!("plain")),
                    description: Some("Template rendering syntax".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
                SchemaField {
                    name: "source".to_string(),
                    field_type: "enum".to_string(),
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue {
                            value: "built-in".to_string(),
                            label: "Built-in".to_string(),
                        },
                        EnumValue {
                            value: "user-modified".to_string(),
                            label: "User Modified".to_string(),
                        },
                        EnumValue {
                            value: "user-created".to_string(),
                            label: "User Created".to_string(),
                        },
                    ]),
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: Some(false),
                    default: Some(serde_json::json!("user-created")),
                    description: Some("Prompt origin for upgrade safety".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                },
            ],
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_core_schemas_returns_all() {
        let schemas = get_core_schemas();
        assert_eq!(schemas.len(), 14);
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
            "checkbox",
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
    fn test_ai_chat_schema_has_fields() {
        let schemas = get_core_schemas();
        let ai_chat = schemas.iter().find(|s| s.id == "ai-chat").unwrap();

        assert_eq!(ai_chat.fields.len(), 7);
        assert!(ai_chat.get_field("provider").is_some());
        assert!(ai_chat.get_field("model").is_some());
        assert!(ai_chat.get_field("status").is_some());
        assert!(ai_chat.get_field("last_active").is_some());
        assert!(ai_chat.get_field("context_tokens").is_some());
        assert!(ai_chat.get_field("created_nodes").is_some());
        assert!(ai_chat.get_field("messages").is_some());

        // Verify messages has item_fields (nested schema for message objects)
        let messages_field = ai_chat.get_field("messages").unwrap();
        assert_eq!(messages_field.field_type, "array");
        assert_eq!(messages_field.item_type.as_deref(), Some("object"));
        let item_fields = messages_field.item_fields.as_ref().unwrap();
        assert!(item_fields.iter().any(|f| f.name == "role"));
        assert!(item_fields.iter().any(|f| f.name == "content"));
        assert!(item_fields.iter().any(|f| f.name == "timestamp"));
        assert!(item_fields.iter().any(|f| f.name == "referenced_nodes"));
        assert!(item_fields.iter().any(|f| f.name == "tool"));
        assert!(item_fields.iter().any(|f| f.name == "args"));
        assert!(item_fields.iter().any(|f| f.name == "status"));
        assert!(item_fields.iter().any(|f| f.name == "result_summary"));
        assert!(item_fields.iter().any(|f| f.name == "duration_ms"));
    }

    #[test]
    fn test_prompt_schema_has_fields() {
        let schemas = get_core_schemas();
        let prompt = schemas.iter().find(|s| s.id == "prompt").unwrap();

        assert_eq!(prompt.fields.len(), 3);
        assert!(prompt.get_field("priority").is_some());
        assert!(prompt.get_field("template_syntax").is_some());
        assert!(prompt.get_field("source").is_some());
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
