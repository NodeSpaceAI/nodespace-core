//! Data Models
//!
//! This module contains the core data structures used throughout NodeSpace:
//!
//! - `Node` - Universal node model for all content types
//! - `Embedding` - Vector embeddings for semantic search (root-aggregate model)
//! - Type-safe wrappers (SkillNode, AiChatNode, SchemaNode) for ergonomic access
//! - Core schema definitions for built-in node types
//!
//! All entities use the Pure JSON schema approach with data stored in the
//! `properties` field of the universal `nodes` table.

pub mod conflict;
pub mod core_schemas;
pub mod embedding;
mod node;
pub mod schema;
pub mod time;

// Type-safe node wrappers
mod ai_chat_node;
mod schema_node;

#[cfg(test)]
#[path = "ai_chat_node_test.rs"]
mod ai_chat_node_test;

pub use ai_chat_node::{
    AiChatCompletedWrite, AiChatMessage, AiChatNode, AiChatPendingDeletion, AiChatResolvedEntity,
    AiChatTurnOutcome, AI_CHAT_NODE_TYPE, AI_CHAT_PROVIDERS,
};
pub use conflict::{ConflictKind, ConflictRecord, ConflictStatus, Resolution};
pub use node::{
    DeleteResult, FilterOperator, Node, NodeEnvelope, NodeFilter, NodeQuery, NodeReference,
    NodeRelationship, NodeUpdate, OrderBy, PropertyFilter, TraversalDirection, ValidationError,
};
pub use schema::{RelationshipDirection, SchemaField, SchemaFieldType, SchemaProtectionLevel};
pub use time::{SystemTimeProvider, TimeProvider};

// Export type-safe wrappers
pub use embedding::{ChunkInfo, Embedding, EmbeddingConfig, EmbeddingSearchResult, NewEmbedding};
pub use nodespace_types::{SkillNode, DEFAULT_SKILL_MAX_ITERATIONS, SKILL_NODE_TYPE};
pub use schema_node::SchemaNode;

// The core type registry (ADR-086 §3): the one list of the types NodeSpace
// ships, and what each records.
pub use nodespace_types::{
    ChildrenRule, ContentRole, CoreNodeType, CoreTypeInfo, CoreTypeKind, ParentRule,
    ParticipationRules, StructuralRules, TypeCategory, WireShape,
};

// node_to_typed_value and nodes_to_typed_values are the single canonical
// implementations in nodespace-types, re-exported here for all entry points.
pub use nodespace_types::{
    flat_properties_view, node_to_typed_value, nodes_to_typed_values, promoted_fields,
};

// Typed update payloads for the core types whose writes route through the
// generic update pipeline (`NodeService::update_task_node` /
// `update_person_node` / `update_project_node` / `update_query_node`).
// Defined once in nodespace-types so the Tauri command layer deserializes the
// same struct the service consumes.
pub use nodespace_types::{PersonNodeUpdate, ProjectNodeUpdate, QueryNodeUpdate, TaskNodeUpdate};

// The vocabularies of the task and project fields: `task.status`, and the
// priority scale the two types share.
pub use nodespace_types::{Priority, ProjectStatus, TaskStatus};

// The stored query's typed fields — the only reader of a query node's
// properties (see `QueryDefinition::from_fields` for the execution mapping).
pub use nodespace_types::{QueryFields, QueryGeneratedBy, QUERY_NODE_TYPE};

// The lifecycle-status allow-list and its validator live in nodespace-types
// (owner of the `Node`/`NodeUpdate` types), re-exported here as the single
// source of truth for the storage-layer write guard.
pub use nodespace_types::{is_valid_lifecycle_status, LIFECYCLE_STATUSES};
