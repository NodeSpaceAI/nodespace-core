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
//! - **database-settings** - Singleton anchor for database-level configuration and the owner `has_role` edge
//!
//! ## Usage
//!
//! Call `get_core_schemas()` to get all core schema definitions.

use crate::models::schema::{
    EnumValue, RelationshipCardinality, RelationshipDirection, SchemaField, SchemaProtectionLevel,
    SchemaRelationship,
};
use crate::models::{SchemaNode, AI_CHAT_PROVIDERS};
use chrono::Utc;

/// The `database-settings` field that lists the extensions a reader needs in
/// order to read a database correctly (ADR-083 §2).
pub const REQUIRED_EXTENSIONS_FIELD: &str = "required_extensions";

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
            is_abstract: false,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "status".to_string(),
                    friendly_name: "Status".to_string(),
                    field_type: crate::models::SchemaFieldType::Enum,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue::new("open".to_string(), "Open".to_string()),
                        EnumValue::new("in_progress".to_string(), "In Progress".to_string()),
                        EnumValue::new("done".to_string(), "Done".to_string()),
                        EnumValue::new("cancelled".to_string(), "Cancelled".to_string()),
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
                    field_type: crate::models::SchemaFieldType::Enum,
                    local_only: false,
                    protection: SchemaProtectionLevel::User,
                    core_values: Some(vec![
                        EnumValue::new("highest".to_string(), "Highest".to_string()),
                        EnumValue::new("high".to_string(), "High".to_string()),
                        EnumValue::new("medium".to_string(), "Medium".to_string()),
                        EnumValue::new("low".to_string(), "Low".to_string()),
                        EnumValue::new("lowest".to_string(), "Lowest".to_string()),
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(false),
                    extensible: Some(true),
                    default: None,
                    description: Some(
                        "Relative urgency for triage and sorting (highest, high, medium, \
                         low, lowest), independent of status. Sorting follows that urgency \
                         order rather than the alphabetical order of the values; \
                         user-defined values sort after all of them. Not a deadline — use \
                         due_date for that. Absent means no priority has been assigned."
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
                    field_type: crate::models::SchemaFieldType::Date,
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
                    field_type: crate::models::SchemaFieldType::Date,
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
                    field_type: crate::models::SchemaFieldType::Date,
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
            // relationship declaration below (mirrors project ↔ task), and its
            // project the inverse of project's `tasks`; neither needs an entry
            // here. What task does declare is the task↔task links every
            // work-tracking tool treats as first-class: dependency (blocks),
            // a weak association (relates_to), and duplication (duplicates).
            //
            // All three are self-referential — the declaration edge is a
            // task→task self-edge, so both the forward and reverse name land
            // on the same node type. All are Many/Many and optional: absence
            // is the common case, and a task can block several others while
            // being blocked by several itself. Cycles (A blocks B blocks A)
            // are representable; nothing here validates against them, matching
            // every other non-`extends` relationship.
            //
            // None of these names belong in BUILTIN_RELATIONSHIP_NAMES: that
            // list is the built-in STRUCTURAL edges (member_of, has_child,
            // mentions, has_role) that declaration queries exclude, and a
            // schema-declared name resolves through the ordinary resolver
            // instead — same as project's `tasks`.
            relationships: vec![
                SchemaRelationship {
                    name: "blocks".to_string(),
                    target_type: Some("task".to_string()),
                    direction: RelationshipDirection::Out,
                    cardinality: RelationshipCardinality::Many,
                    required: None,
                    reverse_name: "blocked_by".to_string(),
                    reverse_cardinality: RelationshipCardinality::Many,
                    edge_fields: None,
                    description: Some(
                        "Tasks that cannot start/complete until this task is done".to_string(),
                    ),
                },
                SchemaRelationship {
                    name: "relates_to".to_string(),
                    target_type: Some("task".to_string()),
                    direction: RelationshipDirection::Out,
                    cardinality: RelationshipCardinality::Many,
                    required: None,
                    reverse_name: "related_from".to_string(),
                    reverse_cardinality: RelationshipCardinality::Many,
                    edge_fields: None,
                    description: Some(
                        "Tasks this task is related to, with no directional dependency".to_string(),
                    ),
                },
                SchemaRelationship {
                    name: "duplicates".to_string(),
                    target_type: Some("task".to_string()),
                    direction: RelationshipDirection::Out,
                    cardinality: RelationshipCardinality::Many,
                    required: None,
                    reverse_name: "duplicated_by".to_string(),
                    reverse_cardinality: RelationshipCardinality::Many,
                    edge_fields: None,
                    description: Some("The task(s) this task duplicates".to_string()),
                },
            ],
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
            is_abstract: false,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "status".to_string(),
                    friendly_name: "Status".to_string(),
                    field_type: crate::models::SchemaFieldType::Enum,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue::new("planning".to_string(), "Planning".to_string()),
                        EnumValue::new("active".to_string(), "Active".to_string()),
                        EnumValue::new("completed".to_string(), "Completed".to_string()),
                        EnumValue::new("cancelled".to_string(), "Cancelled".to_string()),
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(true),
                    default: Some(serde_json::json!("planning")),
                    description: Some(
                        "Current stage of the project: planning (not yet started), active \
                         (underway), completed (finished successfully), or cancelled \
                         (abandoned). Distinct from an individual task's status."
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
                    field_type: crate::models::SchemaFieldType::Enum,
                    local_only: false,
                    protection: SchemaProtectionLevel::User,
                    core_values: Some(vec![
                        EnumValue::new("highest".to_string(), "Highest".to_string()),
                        EnumValue::new("high".to_string(), "High".to_string()),
                        EnumValue::new("medium".to_string(), "Medium".to_string()),
                        EnumValue::new("low".to_string(), "Low".to_string()),
                        EnumValue::new("lowest".to_string(), "Lowest".to_string()),
                    ]),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(false),
                    extensible: Some(true),
                    default: None,
                    description: Some(
                        "Relative importance for triage across projects (highest, high, \
                         medium, low, lowest) — the same scale as task priority. Sorting \
                         follows that order rather than the alphabetical order of the \
                         values; user-defined values sort after all of them. Absent means \
                         no priority has been assigned."
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
                    field_type: crate::models::SchemaFieldType::Date,
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
                    field_type: crate::models::SchemaFieldType::Date,
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
            is_abstract: false,
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
            is_abstract: false,
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
            is_abstract: false,
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
            is_abstract: false,
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
            is_abstract: false,
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
            is_abstract: false,
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
            is_abstract: false,
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
            is_abstract: false,
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
            is_abstract: false,
            schema_version: 1,
            fields: vec![SchemaField {
                name: "description".to_string(),
                friendly_name: "Description".to_string(),
                field_type: crate::models::SchemaFieldType::Text,
                local_only: false,
                protection: SchemaProtectionLevel::Core,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(false),
                extensible: None,
                default: None,
                description: Some("What the collection is for".to_string()),
                item_type: None,
                fields: None,
                item_fields: None,
                unique: None,
                unique_case_insensitive: None,
            }],
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
            is_abstract: false,
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
            is_abstract: false,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "provider".to_string(),
                    friendly_name: "Provider".to_string(),
                    field_type: crate::models::SchemaFieldType::Enum,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    // Closed set: validation rejects any provider outside
                    // `AI_CHAT_PROVIDERS`, so the enum can't be extensible.
                    core_values: Some(
                        AI_CHAT_PROVIDERS
                            .iter()
                            .map(|(value, label)| {
                                EnumValue::new(value.to_string(), label.to_string())
                            })
                            .collect(),
                    ),
                    user_values: Some(vec![]),
                    indexed: true,
                    required: Some(true),
                    extensible: Some(false),
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
                    field_type: crate::models::SchemaFieldType::Text,
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
                    field_type: crate::models::SchemaFieldType::Enum,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    // Daemon-owned: the inference turn state, independent of the
                    // PTY session lifecycle (`session_status` below). See the
                    // module docs on `AiChatNode` for why these were split out
                    // of one shared `status` key.
                    core_values: Some(vec![
                        EnumValue::new("idle".to_string(), "Idle".to_string()),
                        EnumValue::new("processing".to_string(), "Processing".to_string()),
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
                    field_type: crate::models::SchemaFieldType::Enum,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    // PTY-owned: the session lifecycle, independent of the
                    // inference turn state (`turn_status` above). See the
                    // module docs on `AiChatNode` for why these were split out
                    // of one shared `status` key.
                    core_values: Some(vec![
                        EnumValue::new("active".to_string(), "Active".to_string()),
                        EnumValue::new("archived".to_string(), "Archived".to_string()),
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
                    field_type: crate::models::SchemaFieldType::Datetime,
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
                    field_type: crate::models::SchemaFieldType::Number,
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
                    field_type: crate::models::SchemaFieldType::Array,
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
                    item_type: Some(crate::models::SchemaFieldType::Text),
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "messages".to_string(),
                    friendly_name: "Messages".to_string(),
                    field_type: crate::models::SchemaFieldType::Array,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(true),
                    extensible: None,
                    default: Some(serde_json::json!([])),
                    description: Some("Conversation messages array".to_string()),
                    item_type: Some(crate::models::SchemaFieldType::Object),
                    fields: None,
                    item_fields: Some(vec![
                        SchemaField {
                            name: "role".to_string(),
                            friendly_name: "Message sender role".to_string(),
                            field_type: crate::models::SchemaFieldType::Enum,
                            local_only: false,
                            protection: SchemaProtectionLevel::Core,
                            core_values: Some(vec![
                                EnumValue::new("user".to_string(), "User".to_string()),
                                EnumValue::new("assistant".to_string(), "Assistant".to_string()),
                                EnumValue::new("tool_call".to_string(), "Tool Call".to_string()),
                                EnumValue::new("system".to_string(), "System".to_string()),
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
                            field_type: crate::models::SchemaFieldType::Text,
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
                            field_type: crate::models::SchemaFieldType::Text,
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
                            field_type: crate::models::SchemaFieldType::Datetime,
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
                            field_type: crate::models::SchemaFieldType::Array,
                            local_only: false,
                            protection: SchemaProtectionLevel::Core,
                            core_values: None,
                            user_values: None,
                            indexed: false,
                            required: Some(false),
                            extensible: None,
                            default: None,
                            description: Some("Node IDs referenced in this message".to_string()),
                            item_type: Some(crate::models::SchemaFieldType::Text),
                            fields: None,
                            item_fields: None,
                            unique: None,
                            unique_case_insensitive: None,
                        },
                        SchemaField {
                            name: "tool".to_string(),
                            friendly_name: "Tool".to_string(),
                            field_type: crate::models::SchemaFieldType::Text,
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
                            field_type: crate::models::SchemaFieldType::Object,
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
                            field_type: crate::models::SchemaFieldType::Enum,
                            local_only: false,
                            protection: SchemaProtectionLevel::Core,
                            core_values: Some(vec![
                                EnumValue::new("completed".to_string(), "Completed".to_string()),
                                EnumValue::new("error".to_string(), "Error".to_string()),
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
                            field_type: crate::models::SchemaFieldType::Text,
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
                            field_type: crate::models::SchemaFieldType::Number,
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
                    field_type: crate::models::SchemaFieldType::Text,
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
                    field_type: crate::models::SchemaFieldType::Text,
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
                    field_type: crate::models::SchemaFieldType::Text,
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
                SchemaField {
                    name: "capture:agent_type".to_string(),
                    friendly_name: "Agent".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
                    local_only: false,
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "The external agent a terminal session runs (claude-code, codex, ...)"
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "capture:exit_code".to_string(),
                    friendly_name: "Exit code".to_string(),
                    field_type: crate::models::SchemaFieldType::Number,
                    local_only: false,
                    protection: SchemaProtectionLevel::System,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Exit code of the terminal session's process, once it has ended"
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
            is_abstract: false,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "target_type".to_string(),
                    friendly_name: "Target type".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
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
                    field_type: crate::models::SchemaFieldType::Array,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(true),
                    extensible: None,
                    default: Some(serde_json::json!([])),
                    description: Some("Filter conditions array".to_string()),
                    item_type: Some(crate::models::SchemaFieldType::Object),
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "sorting".to_string(),
                    friendly_name: "Sorting".to_string(),
                    field_type: crate::models::SchemaFieldType::Array,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Sorting configuration array".to_string()),
                    item_type: Some(crate::models::SchemaFieldType::Object),
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "limit".to_string(),
                    friendly_name: "Result limit".to_string(),
                    field_type: crate::models::SchemaFieldType::Number,
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
                    field_type: crate::models::SchemaFieldType::Enum,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue::new("ai".to_string(), "AI Generated".to_string()),
                        EnumValue::new("user".to_string(), "User Created".to_string()),
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
                    field_type: crate::models::SchemaFieldType::Text,
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
                    field_type: crate::models::SchemaFieldType::Number,
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
                    field_type: crate::models::SchemaFieldType::Datetime,
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
                SchemaField {
                    name: "view_config".to_string(),
                    friendly_name: "View configuration".to_string(),
                    field_type: crate::models::SchemaFieldType::Object,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "How the query renders: lastView (list, table or kanban) and \
                         kanban.groupBy"
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
        // Person schema — identity primitive (first_name, last_name, email).
        // A convergence collision on `email` is journaled as a
        // `UniqueFieldCollision` conflict record (ADR-068), not stored as a
        // property on the node. Display identity is composed by
        // title_template below, the single place the first/last composition
        // rule lives.
        SchemaNode {
            id: "person".to_string(),
            content: "Person".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            is_abstract: false,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "first_name".to_string(),
                    friendly_name: "First name".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "First name; optional — a person may exist before a name is set"
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "last_name".to_string(),
                    friendly_name: "Last name".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Last name; optional — a person may exist before a name is set".to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "email".to_string(),
                    friendly_name: "Email".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("Email address (optional)".to_string()),
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
            //
            // `reported_tasks` is the same shape under a distinct name: who
            // filed the task, which is independent of who it was assigned to
            // (often nobody, and often not the same person).
            relationships: vec![
                SchemaRelationship {
                    name: "tasks".to_string(),
                    target_type: Some("task".to_string()),
                    direction: RelationshipDirection::Out,
                    cardinality: RelationshipCardinality::Many,
                    required: None,
                    reverse_name: "assignee".to_string(),
                    reverse_cardinality: RelationshipCardinality::One,
                    edge_fields: None,
                    description: Some("Tasks assigned to this person".to_string()),
                },
                SchemaRelationship {
                    name: "reported_tasks".to_string(),
                    target_type: Some("task".to_string()),
                    direction: RelationshipDirection::Out,
                    cardinality: RelationshipCardinality::Many,
                    required: None,
                    reverse_name: "creator".to_string(),
                    reverse_cardinality: RelationshipCardinality::One,
                    edge_fields: None,
                    description: Some(
                        "Tasks originally reported/created by this person".to_string(),
                    ),
                },
            ],
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
            is_abstract: false,
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
            is_abstract: false,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "description".to_string(),
                    friendly_name: "Description".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
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
                    name: "exclusion".to_string(),
                    friendly_name: "Exclusion".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "What this skill is not for; lowers its discovery score on requests \
                         that match this better than the description"
                            .to_string(),
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
                    field_type: crate::models::SchemaFieldType::Array,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(true),
                    extensible: None,
                    default: Some(serde_json::json!([])),
                    description: Some("Tools available when this skill is active".to_string()),
                    item_type: Some(crate::models::SchemaFieldType::Text),
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "max_iterations".to_string(),
                    friendly_name: "Max iterations".to_string(),
                    field_type: crate::models::SchemaFieldType::Number,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!(
                        crate::models::DEFAULT_SKILL_MAX_ITERATIONS
                    )),
                    description: Some("Maximum ReAct loop iterations for this skill".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "node_types".to_string(),
                    friendly_name: "Node types".to_string(),
                    field_type: crate::models::SchemaFieldType::Array,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!([])),
                    description: Some(
                        "Schema ids this skill is scoped to; empty means unscoped".to_string(),
                    ),
                    item_type: Some(crate::models::SchemaFieldType::Text),
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
        // Play schema — a workflow definition node (`playbook-system.md`,
        // "Built-in Node Types"). The engine queries these by type at startup
        // and on every play mutation.
        //
        // `rules` is the play's rule array. It is NOT validated by the schema's
        // generic machinery: `create_node`/`update_node` call
        // `validate_play_rules` separately, which parses each rule's trigger,
        // conditions and actions. The field is declared here so a play node has
        // a real, inspectable type like every other core node — not to move
        // rule validation into the schema layer.
        //
        // Deliberately no `log`/`playbook_log` schema alongside this one:
        // `create_or_update_log_node` writes nine properties today, and
        // declaring them would switch on required-field and enum enforcement
        // over a surface that has never had it. ADR-076 records what that costs
        // when the declared vocabulary and the writing code have drifted
        // (`ai-chat.status`, 16 broken tests). Tracked separately.
        SchemaNode {
            id: "play".to_string(),
            content: "Play".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            is_abstract: false,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "rules".to_string(),
                    friendly_name: "Rules".to_string(),
                    field_type: crate::models::SchemaFieldType::Array,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    // Not `required`: a play node is created before its rules
                    // are filled in by some flows, and `validate_play_rules`
                    // is what enforces rule shape either way.
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!([])),
                    description: Some(
                        "Rule definitions: each a trigger, conditions and actions".to_string(),
                    ),
                    item_type: Some(crate::models::SchemaFieldType::Object),
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "description".to_string(),
                    friendly_name: "Description".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("What this play automates, in one line".to_string()),
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
        // Database Settings schema — the singleton anchor for database-level
        // configuration and for the owner `has_role` edge (person → this node).
        // Its one field, `required_extensions`, names the extensions a reader
        // needs; the daemon refuses to open a database that lists one it does
        // not support (ADR-083 §2). It lives in this base bucket, so it stays
        // in place when the singleton is retyped to a subtype (ADR-078).
        SchemaNode {
            id: "database-settings".to_string(),
            content: "Database Settings".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            is_abstract: false,
            schema_version: 1,
            fields: vec![SchemaField {
                name: REQUIRED_EXTENSIONS_FIELD.to_string(),
                friendly_name: "Required extensions".to_string(),
                field_type: crate::models::SchemaFieldType::Array,
                local_only: false,
                protection: SchemaProtectionLevel::Core,
                core_values: None,
                user_values: None,
                indexed: false,
                required: Some(false),
                extensible: None,
                default: Some(serde_json::json!([])),
                description: Some(
                    "Ids of the extensions a reader needs in order to read this database \
                     correctly. Empty by default. Core assigns no meaning to any entry; it \
                     refuses to open a database that lists an extension it does not support."
                        .to_string(),
                ),
                item_type: Some(crate::models::SchemaFieldType::Text),
                fields: None,
                item_fields: None,
                unique: None,
                unique_case_insensitive: None,
            }],
            relationships: vec![],
            title_template: None,
            properties_header_summary_template: None,
        },
        // Tool schema — a searchable registry entry for something the agent
        // can call. Content is the tool's display name. The parameter schema
        // is a JSON Schema document with its own validator
        // (`ToolNodeBehavior`), so it is an open object here.
        SchemaNode {
            id: "tool".to_string(),
            content: "Tool".to_string(),
            version: 1,
            created_at: now,
            modified_at: now,
            is_core: true,
            is_abstract: false,
            schema_version: 1,
            fields: vec![
                SchemaField {
                    name: "handler".to_string(),
                    friendly_name: "Handler".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: true,
                    required: Some(true),
                    extensible: None,
                    default: None,
                    description: Some(
                        "Stable key resolving to the handler that runs the tool".to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "description".to_string(),
                    friendly_name: "Description".to_string(),
                    field_type: crate::models::SchemaFieldType::Text,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some(
                        "What the tool does; embedded with its name for discovery".to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "parameter_schema".to_string(),
                    friendly_name: "Parameter schema".to_string(),
                    field_type: crate::models::SchemaFieldType::Object,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: None,
                    description: Some("JSON Schema for the parameters the tool takes".to_string()),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "source".to_string(),
                    friendly_name: "Source".to_string(),
                    field_type: crate::models::SchemaFieldType::Enum,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: Some(vec![
                        EnumValue::new("internal".to_string(), "Internal".to_string()),
                        EnumValue::new("external".to_string(), "External".to_string()),
                    ]),
                    user_values: Some(vec![]),
                    indexed: false,
                    required: Some(false),
                    extensible: Some(false),
                    default: Some(serde_json::json!("internal")),
                    description: Some(
                        "Where the tool comes from: generated by NodeSpace or registered by a user"
                            .to_string(),
                    ),
                    item_type: None,
                    fields: None,
                    item_fields: None,
                    unique: None,
                    unique_case_insensitive: None,
                },
                SchemaField {
                    name: "enabled".to_string(),
                    friendly_name: "Enabled".to_string(),
                    field_type: crate::models::SchemaFieldType::Boolean,
                    local_only: false,
                    protection: SchemaProtectionLevel::Core,
                    core_values: None,
                    user_values: None,
                    indexed: false,
                    required: Some(false),
                    extensible: None,
                    default: Some(serde_json::json!(true)),
                    description: Some("Whether the tool is offered to the model".to_string()),
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
        // Assert the ids, not just the count: a bare length check reports
        // "expected 18, got 19" when a schema is added and says nothing about
        // which one, and passes unchanged if one is swapped for another.
        let mut ids: Vec<&str> = schemas.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        // Sorted, so this literal reads as a set rather than pinning the
        // declaration order in `get_core_schemas`.
        assert_eq!(
            ids,
            [
                "agent-guidance",
                "ai-chat",
                "checkbox",
                "code-block",
                "collection",
                "database-settings",
                "date",
                "header",
                "horizontal-line",
                "ordered-list",
                "person",
                "play",
                "project",
                "query",
                "quote-block",
                "skill",
                "table",
                "task",
                "text",
                "tool",
            ]
        );
    }

    // ---- The core type registry, held to the seeded schemas (ADR-086 §3) ----

    use crate::models::{CoreNodeType, SchemaFieldType, TypeCategory, WireShape};

    fn core_schema(core: CoreNodeType) -> Option<SchemaNode> {
        get_core_schemas()
            .into_iter()
            .find(|s| s.id == core.as_str())
    }

    /// The fields of a core type's whole `extends` chain, nearest first.
    fn chain_fields(core: CoreNodeType) -> Vec<SchemaField> {
        core.chain()
            .into_iter()
            .filter_map(core_schema)
            .flat_map(|schema| schema.fields)
            .collect()
    }

    /// The seeded core schemas name exactly the registry's variants, except
    /// the `schema` meta-type, which has no schema of its own.
    #[test]
    fn the_seeded_schemas_are_exactly_the_registry_minus_the_meta_type() {
        let mut seeded: Vec<String> = get_core_schemas().into_iter().map(|s| s.id).collect();
        seeded.sort();
        let mut registered: Vec<String> = CoreNodeType::ALL
            .into_iter()
            .filter(|core| *core != CoreNodeType::Schema)
            .map(|core| core.as_str().to_string())
            .collect();
        registered.sort();
        assert_eq!(seeded, registered);
        assert!(core_schema(CoreNodeType::Schema).is_none());
    }

    /// Every core behaviour has a seeded schema, and every seeded schema has a
    /// behaviour (the meta-type's behaviour is the one without a schema).
    #[test]
    fn every_core_behavior_has_a_schema_and_every_schema_a_behavior() {
        let registry = crate::behaviors::NodeBehaviorRegistry::new();
        for core in CoreNodeType::ALL {
            assert!(
                registry.get(core.as_str()).is_some(),
                "{core} has no behavior"
            );
        }
        for schema in get_core_schemas() {
            assert!(
                registry.get(&schema.id).is_some(),
                "seeded schema '{}' has no behavior",
                schema.id
            );
        }
        for type_name in registry.get_all_types() {
            let core = CoreNodeType::from_id(&type_name)
                .unwrap_or_else(|| panic!("behavior '{type_name}' is not a core type"));
            assert!(
                core == CoreNodeType::Schema || core_schema(core).is_some(),
                "behavior '{type_name}' has no seeded schema"
            );
        }
    }

    /// A type's category is computed over its whole `extends` chain: a
    /// primitive declares no fields, a structured type holds at least one
    /// object-valued field, and a flat type holds fields but no objects.
    #[test]
    fn each_category_matches_the_fields_its_chain_declares() {
        fn holds_objects(field: &SchemaField) -> bool {
            field.field_type == SchemaFieldType::Object
                || field.item_type == Some(SchemaFieldType::Object)
        }
        for core in CoreNodeType::ALL {
            // The meta-type's fields are the schema node's own typed
            // properties (`fields`, `relationships`), not a seeded schema.
            if core == CoreNodeType::Schema {
                assert_eq!(core.category(), TypeCategory::Structured);
                continue;
            }
            let fields = chain_fields(core);
            let computed = if fields.is_empty() {
                TypeCategory::Primitive
            } else if fields.iter().any(holds_objects) {
                TypeCategory::Structured
            } else {
                TypeCategory::Flat
            };
            assert_eq!(
                core.category(),
                computed,
                "{core}: the registry's category does not match its schema chain"
            );
        }
    }

    /// The registry's parent, abstract flag and title template are the
    /// schema's.
    #[test]
    fn registry_parents_abstract_flags_and_templates_match_the_schemas() {
        for core in CoreNodeType::ALL {
            let Some(schema) = core_schema(core) else {
                continue;
            };
            assert_eq!(
                schema.is_abstract,
                core.is_abstract(),
                "{core}: abstract flag"
            );
            assert_eq!(
                crate::schema::extends_chain::declared_parent(&schema),
                core.parent().map(|p| p.as_str().to_string()),
                "{core}: parent"
            );
            assert_eq!(
                schema.title_template.as_deref(),
                core.info().title_template,
                "{core}: title template"
            );
        }
    }

    /// Core field names are snake_case, apart from the namespaced ones, and
    /// no core field takes the `string` type the vocabulary no longer has.
    #[test]
    fn core_fields_are_snake_case() {
        for schema in get_core_schemas() {
            for field in &schema.fields {
                let bare = field.name.rsplit(':').next().unwrap_or(&field.name);
                assert!(
                    bare.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                    "{}.{} is not snake_case",
                    schema.id,
                    field.name
                );
            }
        }
    }

    /// A type that travels typed promotes exactly its schema's fields, in the
    /// shape and with the writability the schema declares, and a type whose
    /// update is typed has promoted fields to write. The frontend's
    /// `TYPED_CORE_FIELDS` is generated from the promoted fields, so this is
    /// also what keeps it true to the schemas.
    #[test]
    fn typed_wire_shapes_promote_their_schemas_fields() {
        for core in CoreNodeType::ALL {
            let promoted = nodespace_types::core_promoted_fields(core);
            match core.wire() {
                WireShape::Typed { update: true } => {
                    let schema = core_schema(core).expect("a typed core type has a schema");
                    let mut declared: Vec<&str> =
                        schema.fields.iter().map(|f| f.name.as_str()).collect();
                    declared.sort_unstable();
                    let mut storage: Vec<&str> = promoted.iter().map(|f| f.storage).collect();
                    storage.sort_unstable();
                    assert_eq!(storage, declared, "{core}: promoted fields");
                    for promoted in promoted {
                        use crate::models::SchemaFieldType as T;
                        use nodespace_types::PromotedShape as S;
                        let field = schema.get_field(promoted.storage).expect("declared above");
                        let shape = match field.field_type {
                            T::Text | T::Enum | T::Datetime => S::Text,
                            T::Date => S::Date,
                            T::Number => S::Number,
                            T::Array => S::Array,
                            T::Object => S::Object,
                            T::Boolean => panic!(
                                "{core}.{}: a boolean field needs a promoted shape",
                                field.name
                            ),
                        };
                        assert_eq!(promoted.shape, shape, "{core}.{}: shape", field.name);
                        assert_eq!(
                            promoted.read_only,
                            field.protection == SchemaProtectionLevel::System,
                            "{core}.{}: a system field is read-only",
                            field.name
                        );
                    }
                    assert_eq!(nodespace_types::typed_update_fields(core), promoted);
                }
                WireShape::Typed { update: false } | WireShape::Generic | WireShape::Envelope => {
                    assert!(
                        nodespace_types::typed_update_fields(core).is_empty(),
                        "{core} has no typed update"
                    );
                }
            }
        }
    }

    /// Engine diagnostics do not live in the graph: there is no `log` or
    /// `playbook_log` core schema, and Play errors are written to a log file
    /// instead. Pinned as a test because the absence is deliberate — a Play
    /// failure is operational telemetry, not knowledge, and writing it back
    /// into the substrate the engine watches is a self-trigger hazard.
    #[test]
    fn test_no_engine_diagnostic_schema_in_graph() {
        let schemas = get_core_schemas();
        for id in ["log", "playbook_log"] {
            assert!(
                !schemas.iter().any(|s| s.id == id),
                "`{id}` must not be a graph type — engine diagnostics go to the log file"
            );
        }
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
    fn test_project_status_values() {
        let schemas = get_core_schemas();
        let project = schemas.iter().find(|s| s.id == "project").unwrap();
        let status = project.get_field("status").unwrap();

        // No `archived`: retiring a project is archiving the node (ADR-087).
        let values: Vec<&str> = status
            .core_values
            .as_ref()
            .unwrap()
            .iter()
            .map(|v| v.value.as_str())
            .collect();
        assert_eq!(values, ["planning", "active", "completed", "cancelled"]);
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
            !task.relationships.iter().any(|r| r.name == "project"),
            "task's project link is the derived inverse, not its own declaration"
        );
    }

    #[test]
    fn test_person_declares_tasks_assignee_relationship() {
        let schemas = get_core_schemas();
        let person = schemas.iter().find(|s| s.id == "person").unwrap();

        // person has-many tasks (as assignee); the task-side inverse is derived
        // from this one declaration (task carries no `assignee` entry of its
        // own — mirrors project's `tasks` relationship).
        //
        // The count is asserted alongside the lookup so an accidental addition
        // to person still trips a test: `tasks` and `reported_tasks`, no more.
        assert_eq!(person.relationships.len(), 2);
        let rel = person
            .relationships
            .iter()
            .find(|r| r.name == "tasks")
            .expect("person declares tasks");
        assert_eq!(rel.target_type.as_deref(), Some("task"));
        assert_eq!(rel.cardinality, RelationshipCardinality::Many);
        assert_eq!(rel.reverse_name, "assignee");
        assert_eq!(rel.reverse_cardinality, RelationshipCardinality::One);

        let task = schemas.iter().find(|s| s.id == "task").unwrap();
        assert!(
            !task.relationships.iter().any(|r| r.name == "assignee"),
            "task's assignee link is the derived inverse, not its own declaration"
        );
    }

    #[test]
    fn test_task_declares_self_referential_link_relationships() {
        let schemas = get_core_schemas();
        let task = schemas.iter().find(|s| s.id == "task").unwrap();

        // Exactly these three — an accidental fourth declaration on task should
        // trip a test rather than ride along unnoticed.
        assert_eq!(task.relationships.len(), 3);

        // Every one of these is task→task, Many/Many, and optional on both ends.
        for (name, reverse_name) in [
            ("blocks", "blocked_by"),
            ("relates_to", "related_from"),
            ("duplicates", "duplicated_by"),
        ] {
            let rel = task
                .relationships
                .iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("task declares {name}"));
            assert_eq!(
                rel.target_type.as_deref(),
                Some("task"),
                "{name} is self-referential"
            );
            assert_eq!(rel.direction, RelationshipDirection::Out);
            assert_eq!(rel.cardinality, RelationshipCardinality::Many);
            assert_eq!(rel.reverse_name, reverse_name);
            assert_eq!(rel.reverse_cardinality, RelationshipCardinality::Many);
            assert!(rel.required.is_none(), "{name} must be optional");
            assert!(rel.edge_fields.is_none());
        }
    }

    #[test]
    fn test_person_declares_reported_tasks_creator_relationship() {
        let schemas = get_core_schemas();
        let person = schemas.iter().find(|s| s.id == "person").unwrap();

        // Distinct from `tasks`/`assignee`: who filed the task, not who owns it.
        let rel = person
            .relationships
            .iter()
            .find(|r| r.name == "reported_tasks")
            .expect("person declares reported_tasks");
        assert_eq!(rel.target_type.as_deref(), Some("task"));
        assert_eq!(rel.direction, RelationshipDirection::Out);
        assert_eq!(rel.cardinality, RelationshipCardinality::Many);
        assert_eq!(rel.reverse_name, "creator");
        assert_eq!(rel.reverse_cardinality, RelationshipCardinality::One);
        assert!(rel.required.is_none());

        let task = schemas.iter().find(|s| s.id == "task").unwrap();
        assert!(
            !task.relationships.iter().any(|r| r.name == "creator"),
            "task's creator link is the derived inverse, not its own declaration"
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
    fn test_collection_declares_only_an_optional_description() {
        // A collection is grouping only (ADR-083 §5): its one field says what
        // the collection is for, and it carries no access-control fields.
        let schemas = get_core_schemas();
        let collection = schemas.iter().find(|s| s.id == "collection").unwrap();

        let names: Vec<&str> = collection.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["description"]);

        let description = collection.get_field("description").unwrap();
        assert_eq!(description.field_type, "text");
        assert_eq!(description.required, Some(false));
        assert_eq!(description.default, None);
    }

    #[test]
    fn test_person_email_field_is_optional() {
        // The description is seeded into every new database and shown to users
        // and agents, so it must describe the field alone.
        let schemas = get_core_schemas();
        let person = schemas.iter().find(|s| s.id == "person").unwrap();
        let email = person.get_field("email").expect("person has email");
        assert_eq!(email.required, Some(false));
        assert_eq!(
            email.description.as_deref(),
            Some("Email address (optional)")
        );
    }

    #[test]
    fn test_query_schema_has_fields() {
        let schemas = get_core_schemas();
        let query = schemas.iter().find(|s| s.id == "query").unwrap();

        assert_eq!(query.fields.len(), 9);
        assert!(query.get_field("target_type").is_some());
        assert!(query.get_field("filters").is_some());
        assert!(query.get_field("sorting").is_some());
        assert!(query.get_field("limit").is_some());
        assert!(query.get_field("generated_by").is_some());
        assert!(query.get_field("generator_context").is_some());
        assert!(query.get_field("execution_count").is_some());
        assert!(query.get_field("last_executed").is_some());
        assert!(query.get_field("view_config").is_some());
    }

    #[test]
    fn test_ai_chat_schema_has_fields() {
        let schemas = get_core_schemas();
        let ai_chat = schemas.iter().find(|s| s.id == "ai-chat").unwrap();

        assert_eq!(ai_chat.fields.len(), 13);
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
        assert_eq!(
            messages_field.field_type,
            crate::models::SchemaFieldType::Array
        );
        assert_eq!(
            messages_field.item_type,
            Some(crate::models::SchemaFieldType::Object)
        );
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

        assert_eq!(skill.fields.len(), 5);
        assert!(skill.get_field("node_types").is_some());
        assert!(skill.get_field("description").is_some());
        assert!(skill.get_field("exclusion").is_some());
        assert!(skill.get_field("tool_whitelist").is_some());
        assert!(skill.get_field("max_iterations").is_some());

        // Verify tool_whitelist is an array of strings
        let whitelist = skill.get_field("tool_whitelist").unwrap();
        assert_eq!(whitelist.field_type, crate::models::SchemaFieldType::Array);
        assert_eq!(
            whitelist.item_type,
            Some(crate::models::SchemaFieldType::Text)
        );
    }

    #[test]
    fn test_database_settings_schema_declares_only_required_extensions() {
        // database-settings is a Core singleton anchor. Its only field is the
        // neutral list of extensions a database requires (ADR-083 §2): a list
        // of strings, empty by default.
        let schemas = get_core_schemas();
        let settings = schemas
            .iter()
            .find(|s| s.id == "database-settings")
            .expect("database-settings core schema exists");

        assert!(settings.is_core);
        assert_eq!(
            settings
                .fields
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["required_extensions"]
        );
        let field = &settings.fields[0];
        assert_eq!(field.field_type, crate::models::SchemaFieldType::Array);
        assert_eq!(field.item_type, Some(crate::models::SchemaFieldType::Text));
        assert_eq!(field.default, Some(serde_json::json!([])));
        assert_eq!(field.required, Some(false));
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

    /// ADR-076 standing check: `TaskStatus`'s named variants and `task.status`'s
    /// seed `core_values` must stay consistent in both directions.
    ///
    /// A one-time audit only proves correctness at the moment it runs — this is
    /// what catches the *next* drift, e.g. a future `TaskStatus` variant added
    /// without a matching schema update, or vice versa, before it repeats the
    /// `ai-chat.status` incident (enabling enum validation broke 16 daemon
    /// tests because the schema's declared vocabulary and the code writing
    /// status values had silently drifted apart).
    #[test]
    fn test_task_status_variants_match_core_values_bidirectionally() {
        use crate::models::TaskStatus;

        let schemas = get_core_schemas();
        let task = schemas.iter().find(|s| s.id == "task").unwrap();
        let status_field = task.get_field("status").expect("task schema has status");
        let core_value_strings: Vec<&str> = status_field
            .core_values
            .as_ref()
            .expect("status field has core_values")
            .iter()
            .map(|ev| ev.value.as_str())
            .collect();

        // Every named TaskStatus variant (everything but the User(_) catch-all)
        // must have a corresponding core_values entry.
        let named_variants = [
            TaskStatus::Open,
            TaskStatus::InProgress,
            TaskStatus::Done,
            TaskStatus::Cancelled,
        ];
        for variant in &named_variants {
            assert!(
                core_value_strings.contains(&variant.as_str()),
                "TaskStatus::{:?} (\"{}\") has no matching entry in task.status's core_values \
                 ({:?}) — add it to core_schemas.rs's seed definition.",
                variant,
                variant.as_str(),
                core_value_strings
            );
        }

        // Every core_values entry must round-trip through TaskStatus::from_value
        // to a NAMED variant, not fall through to the User(_) catch-all — a
        // core_values entry with no corresponding variant is exactly the
        // reverse drift (schema declares a value the type doesn't know as a
        // first-class variant).
        for value in &core_value_strings {
            let parsed = TaskStatus::from_value(value);
            assert!(
                parsed.is_core(),
                "task.status's core_values entry '{}' does not parse to a named TaskStatus \
                 variant (got TaskStatus::User(_)) — add a matching variant to nodespace-types' TaskStatus \
                 or remove the stray core_values entry.",
                value
            );
        }
    }

    /// The `Priority` sibling of the ADR-076 status drift check above, run
    /// for every core type that shares the scale.
    ///
    /// Each type's `priority` must list exactly the named variants, in rank
    /// order. A named variant with no `core_values` entry is a value the type
    /// accepts but the schema rejects; a `core_values` entry with no named
    /// variant parses to `Priority::User(_)` — validating fine while reporting
    /// `is_core() == false`, so it is silently treated as a user extension of
    /// the very field that declares it, and sorts after the whole scale. The
    /// order is pinned too, so every type's picker lists the scale the same
    /// way the query service ranks it.
    #[test]
    fn test_priority_variants_match_core_values_bidirectionally() {
        use crate::models::Priority;

        let expected: Vec<&str> = [
            Priority::Highest,
            Priority::High,
            Priority::Medium,
            Priority::Low,
            Priority::Lowest,
        ]
        .iter()
        .map(|p| p.as_str())
        .collect();

        let schemas = get_core_schemas();
        for node_type in Priority::NODE_TYPES.map(|core| core.as_str()) {
            let schema = schemas
                .iter()
                .find(|s| s.id == node_type)
                .unwrap_or_else(|| panic!("no core schema for '{}'", node_type));
            let priority_field = schema
                .get_field("priority")
                .unwrap_or_else(|| panic!("{} schema has priority", node_type));
            let core_value_strings: Vec<&str> = priority_field
                .core_values
                .as_ref()
                .expect("priority field has core_values")
                .iter()
                .map(|ev| ev.value.as_str())
                .collect();

            assert_eq!(
                core_value_strings, expected,
                "{}.priority's core_values must be exactly the named Priority variants \
                 in rank order — update core_schemas.rs or nodespace-types' Priority so they agree.",
                node_type
            );
        }
    }

    /// The other direction of the check above: a core type that declares a
    /// `priority` field must be listed in `Priority::NODE_TYPES`. Otherwise
    /// its field would silently sort as text while looking like the shared
    /// scale — the divergence `project.priority` once had. Adding a core type
    /// with a deliberately different scale is fine, but it has to be a
    /// decision made here, not drift.
    #[test]
    fn test_every_core_priority_field_uses_the_shared_scale() {
        use crate::models::Priority;

        for schema in get_core_schemas() {
            if schema.get_field("priority").is_some() {
                assert!(
                    Priority::applies_to(&schema.id),
                    "core schema '{}' declares `priority` but is not in \
                     Priority::NODE_TYPES — add it there (and align its core_values), \
                     or document why its scale differs.",
                    schema.id
                );
            }
        }
    }

    /// The typed wire structs promote exactly the fields each core type's
    /// schema declares — no more, no fewer. `promoted_fields` is the single
    /// list the conversion, the CLI and the agent use, so a field added to a
    /// core schema without it would travel in `properties` and be invisible
    /// to every typed reader, and a stale entry would strip a field that no
    /// longer exists. (The frontend's `TYPED_CORE_FIELDS` is generated from this
    /// same list.)
    #[test]
    fn promoted_fields_match_each_typed_core_schema() {
        let schemas = get_core_schemas();
        for node_type in ["task", "person", "project", "query"] {
            let schema = schemas
                .iter()
                .find(|s| s.id == node_type)
                .unwrap_or_else(|| panic!("core schema '{}' missing", node_type));
            let mut declared: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
            let mut promoted: Vec<&str> = crate::models::promoted_fields(node_type)
                .iter()
                .map(|field| field.storage)
                .collect();
            declared.sort_unstable();
            promoted.sort_unstable();
            assert_eq!(
                promoted, declared,
                "promoted_fields('{}') must list exactly the core schema's fields",
                node_type
            );
        }
    }
}
