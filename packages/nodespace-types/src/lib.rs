//! Shared wire types for NodeSpace.
//!
//! This crate provides the type definitions and conversion functions shared
//! between `nodespace-core` (storage layer), the Tauri command layer
//! (`packages/desktop-app/app-lib`), and `nodespace-cli` (which uses
//! `flatten_namespaced_properties` to honour the same property contract).
//! All re-use these types directly, eliminating the hand-synced mirror that
//! previously lived in `src-tauri/src/types.rs`.
//!
//! # TypeScript
//!
//! The frontend's wire types are generated from this crate (ADR-086 §8). Every
//! serialized type derives `ts_rs::TS` behind the `ts` feature, and the
//! `gen-ts` binary (`bun run gen:types`) writes one TypeScript file per type.
//! A new wire type needs the derive and an entry in that binary's list;
//! `bun run gen:types` fails until it has both.
//!
//! A field's TypeScript optionality follows what serde writes:
//! - `#[ts(optional_fields)]` on a struct makes each `Option` field `t?: T`,
//!   for structs whose `Option` fields are all skipped when `None`.
//! - `#[ts(optional = nullable)]` on a field makes it omittable while keeping
//!   its type: a `Vec` or `bool` that is skipped when empty or false, or an
//!   `Option` a reader may also send as `null`.
//!
//! # Dependencies
//!
//! Intentionally minimal: `serde`, `serde_json`, `chrono`, `uuid`,
//! `thiserror`. No database, HTTP, or NLP dependencies. `ts-rs` is compiled
//! only with the `ts` feature, which no other crate enables.

mod ai_chat;
mod collection;
mod convert;
mod core_type;
mod database_settings;
mod helpers;
mod incompatible_database;
mod node;
mod person;
mod play;
mod priority;
mod project;
mod query;
mod relationship_path;
mod schema;
mod schema_ops;
mod skill;
mod task;

pub use ai_chat::{
    AiChatBase, AiChatMessageNode, AiChatMessageRole, AiChatNativeNode, AiChatPendingDeleteEdge,
    AiChatProvider, AiChatPtyNode, AiChatResolvedEdge, AiChatSessionStatus, AiChatTurnOutcome,
    AiChatTurnStatus, AiChatWrite, AiChatWroteEdge, AI_CHAT_PENDING_DELETE, AI_CHAT_PINS,
    AI_CHAT_RESOLVED, AI_CHAT_WROTE, NODESPACE_AGENT,
};
pub use collection::{CollectionNode, CollectionNodeUpdate};
pub use convert::{
    core_promoted_fields, flat_properties_view, flatten_namespaced_properties,
    flatten_namespaced_properties_at_scope, node_to_typed_value, nodes_to_typed_values,
    promoted_fields, typed_update_fields, PromotedField, PromotedShape,
};
pub use core_type::{
    checkbox_is_checked, ChildrenRule, ContentRole, CoreNodeType, CoreTypeInfo, CoreTypeKind,
    DerivedAttribute, DerivedValueType, ParentRule, ParticipationRules, StructuralRules,
    TypeCategory, WireShape,
};
pub use database_settings::{DatabaseSettingsNode, DatabaseSettingsNodeUpdate};
pub use helpers::{is_valid_lifecycle_status, LIFECYCLE_STATUSES};
pub use incompatible_database::IncompatibleDatabase;
pub use node::{
    DeleteResult, Node, NodeEnvelope, NodeQuery, NodeReference, NodeUpdate, OrderBy,
    ValidationError,
};
pub use person::{PersonNode, PersonNodeUpdate};
pub use play::{
    Action, ActionType, AddRelationshipParams, CreateNodeParams, GraphEventType, InlineSelector,
    PlayFields, PlayNode, PlayNodeUpdate, PlaySuspensionReason, RejectParams,
    RemoveRelationshipParams, RuleClass, RuleCondition, RuleDefinition, SavedQuerySelector,
    Selector, Trigger, UpdateNodeParams, PLAY_ENABLED_FIELD, PLAY_NODE_TYPE, PLAY_RULES_FIELD,
    PLAY_SUSPENDED_AT_FIELD, PLAY_SUSPENDED_MESSAGE_FIELD, PLAY_SUSPENDED_REASON_FIELD,
    PLAY_SUSPENSION_FIELDS,
};
pub use priority::Priority;
pub use project::{ProjectNode, ProjectNodeUpdate, ProjectStatus};
pub use query::{
    property_segments, FilterOperator, FilterType, PropertyScope, QueryFields, QueryFilter,
    QueryGeneratedBy, QueryNode, QueryNodeUpdate, RelativeDate, RelativeDateAnchor, SortConfig,
    SortDirection, SubtypeBucket, ALL_TYPES_TARGET, QUERY_NODE_TYPE,
};
pub use relationship_path::{
    HopDirection, RelationshipHop, RelationshipPath, ResolvedHop, ResolvedPath,
};
pub use schema::{
    derive_friendly_name, EdgeField, EnumValue, RelationshipCardinality, RelationshipDirection,
    SchemaChildrenRule, SchemaField, SchemaFieldType, SchemaNode, SchemaParentRule,
    SchemaProtectionLevel, SchemaRelationship,
};
pub use schema_ops::{
    CreateSchemaOutput, CreateSchemaParams, FieldRename, FieldValueAddition, SchemaUpdateOutput,
    UpdateSchemaParams,
};
pub use skill::{
    SkillFields, SkillNode, SkillNodeUpdate, DEFAULT_SKILL_MAX_ITERATIONS, SKILL_APPLIES_TO,
    SKILL_NODE_TYPE,
};
pub use task::{TaskNode, TaskNodeUpdate, TaskStatus};
