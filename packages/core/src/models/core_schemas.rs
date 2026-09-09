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
//! - **person** - Identity primitive (name, email)
//! - **database-settings** - Singleton container for database-level config (sync state, roles)
//!
//! ## Usage
//!
//! Call `get_core_schemas()` to get all core schema definitions.

use crate::models::schema::{
    EnumValue, RelationshipCardinality, RelationshipDirection, SchemaField, SchemaProtectionLevel,
    SchemaRelationship,
};
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
            fields: vec![
                SchemaField {
                    name: "status".to_string(),
                    friendly_name: "Status".to_string(),
                    field_type: "enum".to_string(),
                    local_only: false,
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
                    description: Some(
                        "Current workflow state of the task: open (not started), in_progress \
                         (actively being worked), done (completed), or cancelled (abandoned, \
                         not completed). Drives board columns and completion rollups — a task \
                         counts toward \"done\" totals only when this equals done."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "priority".to_string(),
                    friendly_name: "Priority".to_string(),
                    field_type: "enum".to_string(),
                    local_only: false,
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
                    description: Some(
                        "Relative urgency for triage and sorting (low, medium, high), \
                         independent of status. Not a deadline — use due_date for that. \
                         Absent means no priority has been assigned."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "due_date".to_string(),
                    friendly_name: "Due date".to_string(),
                    field_type: "date".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Date by which the task should be completed. Used for deadline \
                         reminders and overdue detection (a task is overdue when due_date is \
                         in the past and status is not done or cancelled). Absent means no \
                         deadline."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "started_at".to_string(),
                    friendly_name: "Started at".to_string(),
                    field_type: "date".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Date work on the task actually began, distinct from due_date (the \
                         deadline) and created_at (when the task record was made). Set once, \
                         when status first moves to in_progress; not required."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "completed_at".to_string(),
                    friendly_name: "Completed at".to_string(),
                    field_type: "date".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Date the task was finished. Set when status moves to done; used for \
                         completion-rate and cycle-time reporting alongside started_at. Absent \
                         while the task is open, in progress, or cancelled without a recorded \
                         finish date."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
            ],
            // A task's assignee is the derived inverse of person's `tasks`
            // relationship declaration below (mirrors project ↔ task); task
            // carries no relationships entry of its own.
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
        },
        // Project schema - container for tasks, milestones, related work.
        // Name is the node `content`; ownership/membership are graph edges, not
        // properties (Universal Graph). Enum values validated by the schema system.
        //
        // Intentionally not slash-command-creatable. Slash commands are reserved for
        // content primitives (text, headers, checkboxes, code, quotes, ordered lists,
        // rules, tables) plus `task`; entity types like `project` are containers set up
        // deliberately — by the agent, or via the schema/entity surfaces — rather than
        // typed in ad hoc mid-outline. If `/project` appears to be "missing", it is absent
        // by decision, not by oversight.
        //
        // This crate cannot enforce that: slash commands and the surfaces that keep
        // projects reachable are frontend concerns. The rule lives next to the plugin
        // registry, in the desktop app's `core-plugins.ts`.
        SchemaNode {
            id: "project".to_string(),
            content: "Project".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "status".to_string(),
                    friendly_name: "Status".to_string(),
                    field_type: "enum".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue {
                            value: "planning".to_string(),
                            label: "Planning".to_string(),
                        },
                        EnumValue {
                            value: "active".to_string(),
                            label: "Active".to_string(),
                        },
                        EnumValue {
                            value: "completed".to_string(),
                            label: "Completed".to_string(),
                        },
                        EnumValue {
                            value: "archived".to_string(),
                            label: "Archived".to_string(),
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
                    default: Some(serde_json::json!("planning")),
                    description: Some(
                        "Current lifecycle stage of the project: planning (not yet started), \
                         active (underway), completed (finished successfully), archived (no \
                         longer active but retained for reference), or cancelled (abandoned). \
                         Distinct from an individual task's status."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "priority".to_string(),
                    friendly_name: "Priority".to_string(),
                    field_type: "enum".to_string(),
                    local_only: false,
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
                    description: Some(
                        "Relative importance for triage across projects (low, medium, high). \
                         Absent means no priority has been assigned."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "start_date".to_string(),
                    friendly_name: "Start date".to_string(),
                    field_type: "date".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Date work on the project is intended to begin (or began). Used with \
                         end_date to compute the project's planned or actual timeline."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "end_date".to_string(),
                    friendly_name: "End date".to_string(),
                    field_type: "date".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::User,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Date the project is intended to finish (or did finish). Paired with \
                         start_date to compute the project's planned or actual timeline; a \
                         project past its end_date while status is still active or planning \
                         reads as behind schedule."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
            ],
            // A project has many tasks; the inverse (a task's single project) is
            // derived from this declaration, so `task` needs no entry of its own.
            // Seeding persists this as a relationship-table edge between the schema
            // nodes (see NodeService seeding / set_schema_declarations).
            relationships: vec![SchemaRelationship {
                name: "tasks".to_string(),
                target_type: Some("task".to_string()),
                direction: RelationshipDirection::Out,
                cardinality: RelationshipCardinality::Many,
                required: None,
                reverse_name: "project".to_string(),
                reverse_cardinality: RelationshipCardinality::One,
                edge_fields: None,
                description: Some("Tasks belonging to this project".to_string()),
            }],
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
            // ADR-037: opt-in restriction. A Core-protected boolean; the Pro
            // RLS layer reads `properties->>'restrictedToMembers'` to gate access.
            // Default/absent = false = open (organizational). member_of edge
            // `permission` (admin/modify/readOnly) is a free-form edge property the
            // RLS reads directly — no schema field needed.
            fields: vec![
                SchemaField {
                    name: "restrictedToMembers".to_string(),
                    friendly_name: "Restricted to members".to_string(),
                    field_type: "boolean".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!(false)),
                    description: Some(
                        "When true, only person members may access this collection's \
                         nodes (ADR-037 opt-in restriction). Default false = open."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
            ],
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
            fields: vec![
                SchemaField {
                    name: "provider".to_string(),
                    friendly_name: "Provider".to_string(),
                    field_type: "enum".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "model".to_string(),
                    friendly_name: "Model".to_string(),
                    field_type: "text".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "turn_status".to_string(),
                    friendly_name: "Turn status".to_string(),
                    field_type: "enum".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    // Daemon-owned: the inference turn state, independent of the
                    // PTY session lifecycle (`session_status` below). See the
                    // module docs on `AiChatNode` for why these were split out
                    // of one shared `status` key.
                    core_values: Some(vec![
                        EnumValue {
                            value: "idle".to_string(),
                            label: "Idle".to_string(),
                        },
                        EnumValue {
                            value: "processing".to_string(),
                            label: "Processing".to_string(),
                        },
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(false),
                    default: Some(serde_json::json!("idle")),
                    description: Some("Inference turn state, daemon-owned".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "session_status".to_string(),
                    friendly_name: "Session status".to_string(),
                    field_type: "enum".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    // PTY-owned: the session lifecycle, independent of the
                    // inference turn state (`turn_status` above). See the
                    // module docs on `AiChatNode` for why these were split out
                    // of one shared `status` key.
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
                    description: Some("Session lifecycle, PTY-owned".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "last_active".to_string(),
                    friendly_name: "Last active".to_string(),
                    field_type: "datetime".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "context_tokens".to_string(),
                    friendly_name: "Context tokens".to_string(),
                    field_type: "number".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "created_nodes".to_string(),
                    friendly_name: "Created nodes".to_string(),
                    field_type: "array".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "messages".to_string(),
                    friendly_name: "Messages".to_string(),
                    field_type: "array".to_string(),
                    local_only: false,
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
                            friendly_name: "Message sender role".to_string(),
                            field_type: "enum".to_string(),
                            local_only: false,
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
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "content".to_string(),
                            friendly_name: "Message text content".to_string(),
                            field_type: "text".to_string(),
                            local_only: false,
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
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "reasoning".to_string(),
                            friendly_name: "Reasoning".to_string(),
                            field_type: "text".to_string(),
                            local_only: false,
                            protection: SchemaProtectionLevel::Core,
                            core_values: None,
                            user_values: None,
                            indexed: false,
                            required: Some(false),
                            extensible: None,
                            default: None,
                            description: Some(
                                "Model chain-of-thought reasoning toward the answer".to_string(),
                            ),
                            item_type: None,
                            fields: None,
                            item_fields: None,
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "timestamp".to_string(),
                            friendly_name: "Message timestamp".to_string(),
                            field_type: "datetime".to_string(),
                            local_only: false,
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
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "referenced_nodes".to_string(),
                            friendly_name: "Referenced nodes".to_string(),
                            field_type: "array".to_string(),
                            local_only: false,
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
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "tool".to_string(),
                            friendly_name: "Tool".to_string(),
                            field_type: "text".to_string(),
                            local_only: false,
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
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "args".to_string(),
                            friendly_name: "Args".to_string(),
                            field_type: "object".to_string(),
                            local_only: false,
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
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "status".to_string(),
                            friendly_name: "Status".to_string(),
                            field_type: "enum".to_string(),
                            local_only: false,
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
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "result_summary".to_string(),
                            friendly_name: "Result summary".to_string(),
                            field_type: "text".to_string(),
                            local_only: false,
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
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "duration_ms".to_string(),
                            friendly_name: "Duration ms".to_string(),
                            field_type: "number".to_string(),
                            local_only: false,
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
                            unique: None,
                            unique_case_insensitive: None,
                        },
                    ]),
                    unique: None,
                    unique_case_insensitive: None,
                },
                // PTY-capture (mode 2d) properties. session_id + transcript are
                // localOnly (machine-bound resume handle / content-risk raw
                // scrollback) — never pushed, ignored on pull. The derived summary
                // is the intended cross-device artifact and syncs like any field.
                SchemaField {
                    name: "capture:session_id".to_string(),
                    friendly_name: "Session id".to_string(),
                    field_type: "text".to_string(),
                    local_only: true,
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Agent session id — a resume handle that names state on this \
                         machine (e.g. under ~/.claude/); local-only, never synced."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "capture:transcript".to_string(),
                    friendly_name: "Transcript".to_string(),
                    field_type: "text".to_string(),
                    local_only: true,
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Raw PTY terminal scrollback — local-only on content-risk \
                         grounds (may contain secrets, tokens, absolute paths); \
                         never synced. The derived summary carries the cross-device \
                         value instead."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "capture:summary".to_string(),
                    friendly_name: "Summary".to_string(),
                    field_type: "text".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Derived conversation summary — locally-generated prose, the \
                         intended cross-device artifact; syncs."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
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
            fields: vec![
                SchemaField {
                    name: "target_type".to_string(),
                    friendly_name: "Target type".to_string(),
                    field_type: "text".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "filters".to_string(),
                    friendly_name: "Filters".to_string(),
                    field_type: "array".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "sorting".to_string(),
                    friendly_name: "Sorting".to_string(),
                    field_type: "array".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "limit".to_string(),
                    friendly_name: "Result limit".to_string(),
                    field_type: "number".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "generated_by".to_string(),
                    friendly_name: "Generated by".to_string(),
                    field_type: "enum".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "generator_context".to_string(),
                    friendly_name: "Generator context".to_string(),
                    field_type: "text".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "execution_count".to_string(),
                    friendly_name: "Execution count".to_string(),
                    field_type: "number".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "last_executed".to_string(),
                    friendly_name: "Last executed".to_string(),
                    field_type: "datetime".to_string(),
                    local_only: false,
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
                    unique: None,
                    unique_case_insensitive: None,
                },
            ],
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
        },
        // Person schema — identity primitive (first_name, last_name, email).
        // A convergence collision on `email` is journaled as a
        // `UniqueFieldCollision` conflict record (ADR-068), not stored as a
        // property on the node. Display identity is composed by
        // title_template below, the single place the first/last composition
        // rule lives — PersonNodeBehavior::compute_display_name mirrors it
        // for the embedding-content fallback, which title_template doesn't
        // reach.
        SchemaNode {
            id: "person".to_string(),
            content: "Person".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "first_name".to_string(),
                    friendly_name: "First name".to_string(),
                    field_type: "string".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("First name; optional — a person may exist before a name is set".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "last_name".to_string(),
                    friendly_name: "Last name".to_string(),
                    field_type: "string".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Last name; optional — a person may exist before a name is set".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "email".to_string(),
                    friendly_name: "Email".to_string(),
                    field_type: "string".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Email address; optional at schema level, required in practice for invited teammates".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    // Email is a claim, not an identity key: flagged unique so the
                    // UI can suggest an existing match pre-commit, never to reject
                    // a write. Case-insensitive because casing does not distinguish
                    // two otherwise-identical claims.
                    unique: Some(true),
                    unique_case_insensitive: Some(true),
                },
            ],
            // A person has many tasks; the inverse (a task's single assignee)
            // is derived from this declaration, so `task` needs no entry of
            // its own. Mirrors project's `tasks` relationship below.
            relationships: vec![SchemaRelationship {
                name: "tasks".to_string(),
                target_type: Some("task".to_string()),
                direction: RelationshipDirection::Out,
                cardinality: RelationshipCardinality::Many,
                required: None,
                reverse_name: "assignee".to_string(),
                reverse_cardinality: RelationshipCardinality::One,
                edge_fields: None,
                description: Some("Tasks assigned to this person".to_string()),
            }],
            // Whitespace-collapse + trim in interpolate_title_template_with_schema
            // degrades this correctly when one or both fields are empty: one absent
            // field yields just the other; both absent yields "".
            title_template: Some("{first_name} {last_name}".to_string()),
            properties_header_summary_template: None,
        },
        // Agent Guidance schema — unconditional, always-on base system-prompt
        // sections (identity, tool strategy, formatting rules, etc.), assembled
        // by PromptAssembler on every turn. Distinct from `skill`: skill nodes
        // are discovered on demand via search_skills and require a description
        // for semantic matching; agent-guidance nodes carry no discovery
        // metadata and are simply fetched by type. Supersedes the `prompt`
        // schema (ADR-057), which shipped with this same empty shape but no
        // name that described what it was for.
        SchemaNode {
            id: "agent-guidance".to_string(),
            content: "Agent Guidance".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            fields: vec![],
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
        },
        // Skill schema for agent skill definitions (ADR-030)
        SchemaNode {
            id: "skill".to_string(),
            content: "Skill".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "description".to_string(),
                    friendly_name: "Description".to_string(),
                    field_type: "string".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(true),
                    extensible: None,
                    default: None,
                    description: Some(
                        "What this skill does (drives semantic search discovery)".to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "tool_whitelist".to_string(),
                    friendly_name: "Tool whitelist".to_string(),
                    field_type: "array".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(true),
                    extensible: None,
                    default: Some(serde_json::json!([])),
                    description: Some("Tools available when this skill is active".to_string()),
                    item_type: Some("string".to_string()),
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "max_iterations".to_string(),
                    friendly_name: "Max iterations".to_string(),
                    field_type: "number".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!(2)),
                    description: Some("Maximum ReAct loop iterations for this skill".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
            ],
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
        },
        // Database Settings schema — a singleton container for database-level
        // configuration. `sync_enabled` is user intent; `auth_status` is
        // system-managed cloud bind state. ADR-037 moved role and auth_status off
        // PersonNode: role now lives on the has_role edge (person → this node) and
        // auth_status lives here.
        SchemaNode {
            id: "database-settings".to_string(),
            content: "Database Settings".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "sync_enabled".to_string(),
                    friendly_name: "Sync enabled".to_string(),
                    field_type: "boolean".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!(false)),
                    description: Some(
                        "User intent to sync this database to the cloud. Inert on the \
                         free/community tier; the Pro sync daemon reads it to gate sync."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "auth_status".to_string(),
                    friendly_name: "Auth status".to_string(),
                    field_type: "enum".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue {
                            value: "local".to_string(),
                            label: "Local".to_string(),
                        },
                        EnumValue {
                            value: "connected".to_string(),
                            label: "Connected".to_string(),
                        },
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(false),
                    default: Some(serde_json::json!("local")),
                    description: Some(
                        "System-managed cloud bind state. Default local; the Pro sync \
                         daemon sets connected after a Supabase identity bind."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "bound_tenant_schema".to_string(),
                    friendly_name: "Bound tenant schema".to_string(),
                    field_type: "string".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "The cloud tenant this database binds to (ADR-053 per-database \
                         cloud sync), as a Supabase schema name. Empty until the Pro sync \
                         daemon binds the database; inert on the free/community tier."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "bound_tenant_collection".to_string(),
                    friendly_name: "Bound tenant collection".to_string(),
                    field_type: "string".to_string(),
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "The default collection id within the bound tenant (ADR-053 \
                         per-database cloud sync). Empty until the database is bound."
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
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
        assert_eq!(schemas.len(), 18);
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

        assert_eq!(task.fields.len(), 5);
        assert!(task.get_field("status").is_some());
        assert!(task.get_field("priority").is_some());
        assert!(task.get_field("due_date").is_some());
    }

    #[test]
    fn test_project_declares_tasks_relationship() {
        let schemas = get_core_schemas();
        let project = schemas.iter().find(|s| s.id == "project").unwrap();

        // project has-many tasks; the task-side inverse is derived from this one
        // declaration (task carries no relationships entry of its own).
        assert_eq!(project.relationships.len(), 1);
        let rel = &project.relationships[0];
        assert_eq!(rel.name, "tasks");
        assert_eq!(rel.target_type.as_deref(), Some("task"));
        assert_eq!(rel.cardinality, RelationshipCardinality::Many);
        assert_eq!(rel.reverse_name, "project");
        assert_eq!(rel.reverse_cardinality, RelationshipCardinality::One);

        let task = schemas.iter().find(|s| s.id == "task").unwrap();
        assert!(
            task.relationships.is_empty(),
            "task's project link is the derived inverse, not its own declaration"
        );
    }

    #[test]
    fn test_person_declares_tasks_assignee_relationship() {
        let schemas = get_core_schemas();
        let person = schemas.iter().find(|s| s.id == "person").unwrap();

        // person has-many tasks (as assignee); the task-side inverse is derived
        // from this one declaration (task carries no relationships entry of its
        // own — mirrors project's `tasks` relationship).
        assert_eq!(person.relationships.len(), 1);
        let rel = &person.relationships[0];
        assert_eq!(rel.name, "tasks");
        assert_eq!(rel.target_type.as_deref(), Some("task"));
        assert_eq!(rel.cardinality, RelationshipCardinality::Many);
        assert_eq!(rel.reverse_name, "assignee");
        assert_eq!(rel.reverse_cardinality, RelationshipCardinality::One);

        let task = schemas.iter().find(|s| s.id == "task").unwrap();
        assert!(
            task.relationships.is_empty(),
            "task's assignee link is the derived inverse, not its own declaration"
        );
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
    fn test_collection_has_restricted_to_members_field() {
        // ADR-037: opt-in restriction is a Core-protected boolean on collection.
        let schemas = get_core_schemas();
        let collection = schemas.iter().find(|s| s.id == "collection").unwrap();
        let field = collection
            .get_field("restrictedToMembers")
            .expect("collection has restrictedToMembers");
        assert_eq!(field.field_type, "boolean");
        assert_eq!(field.protection, SchemaProtectionLevel::Core);
        assert_eq!(field.default, Some(serde_json::json!(false)));
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

        assert_eq!(ai_chat.fields.len(), 11);
        assert!(ai_chat.get_field("provider").is_some());
        assert!(ai_chat.get_field("model").is_some());
        assert!(ai_chat.get_field("turn_status").is_some());
        assert!(ai_chat.get_field("session_status").is_some());
        assert!(ai_chat.get_field("last_active").is_some());
        assert!(ai_chat.get_field("context_tokens").is_some());
        assert!(ai_chat.get_field("created_nodes").is_some());
        assert!(ai_chat.get_field("messages").is_some());

        // PTY-capture (mode 2d) fields + their localOnly classification: the
        // machine-bound session id and the content-risk raw transcript are
        // localOnly (never synced); the derived summary syncs.
        assert!(ai_chat.get_field("capture:session_id").unwrap().local_only);
        assert!(ai_chat.get_field("capture:transcript").unwrap().local_only);
        assert!(!ai_chat.get_field("capture:summary").unwrap().local_only);
        // Every non-capture field syncs (not localOnly) — parity with prior behavior.
        assert!(!ai_chat.get_field("provider").unwrap().local_only);
        assert!(!ai_chat.get_field("messages").unwrap().local_only);

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
    fn test_agent_guidance_schema_has_fields() {
        let schemas = get_core_schemas();
        let agent_guidance = schemas.iter().find(|s| s.id == "agent-guidance").unwrap();

        assert_eq!(agent_guidance.fields.len(), 0);
    }

    #[test]
    fn test_skill_schema_has_fields() {
        let schemas = get_core_schemas();
        let skill = schemas.iter().find(|s| s.id == "skill").unwrap();

        assert_eq!(skill.fields.len(), 3);
        assert!(skill.get_field("description").is_some());
        assert!(skill.get_field("tool_whitelist").is_some());
        assert!(skill.get_field("max_iterations").is_some());

        // Verify tool_whitelist is an array of strings
        let whitelist = skill.get_field("tool_whitelist").unwrap();
        assert_eq!(whitelist.field_type, "array");
        assert_eq!(whitelist.item_type.as_deref(), Some("string"));
    }

    #[test]
    fn test_database_settings_schema_has_fields() {
        // ADR-037: database-settings is a Core singleton carrying sync_enabled
        // (boolean) and auth_status (Core-protected enum: local/connected).
        // ADR-053 added bound_tenant_schema/bound_tenant_collection (per-database
        // cloud tenant binding).
        let schemas = get_core_schemas();
        let settings = schemas
            .iter()
            .find(|s| s.id == "database-settings")
            .unwrap();

        assert_eq!(settings.fields.len(), 4);

        let sync_enabled = settings
            .get_field("sync_enabled")
            .expect("database-settings has sync_enabled");
        assert_eq!(sync_enabled.field_type, "boolean");
        assert_eq!(sync_enabled.protection, SchemaProtectionLevel::Core);
        assert_eq!(sync_enabled.default, Some(serde_json::json!(false)));

        let auth_status = settings
            .get_field("auth_status")
            .expect("database-settings has auth_status");
        assert_eq!(auth_status.field_type, "enum");
        assert_eq!(auth_status.protection, SchemaProtectionLevel::Core);
        assert_eq!(auth_status.default, Some(serde_json::json!("local")));
        let values: Vec<&str> = auth_status
            .core_values
            .as_ref()
            .unwrap()
            .iter()
            .map(|ev| ev.value.as_str())
            .collect();
        assert_eq!(values, vec!["local", "connected"]);

        for name in ["bound_tenant_schema", "bound_tenant_collection"] {
            let field = settings
                .get_field(name)
                .unwrap_or_else(|| panic!("database-settings has {name}"));
            assert_eq!(field.field_type, "string");
            assert_eq!(field.protection, SchemaProtectionLevel::Core);
            assert_eq!(field.required, Some(false));
            assert_eq!(field.default, None);
        }
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
