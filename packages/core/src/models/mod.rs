//! Data Models
//!
//! This module contains the core data structures used throughout NodeSpace:
//!
//! - `Node` - Universal node model for all content types
//! - `Embedding` - Vector embeddings for semantic search (root-aggregate model)
//! - Typed readers (SkillFields) for ergonomic access, and the storage
//!   mapping of the `SchemaNode` wire type (`schema_node`)
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
pub(crate) mod schema_node;

// The AI-chat family (ADR-088) is defined once, in nodespace-types.
pub use conflict::{ConflictKind, ConflictRecord, ConflictStatus, Resolution};
pub use node::{
    DeleteResult, FilterOperator, Node, NodeEnvelope, NodeFilter, NodeQuery, NodeReference,
    NodeRelationship, NodeUpdate, OrderBy, PropertyFilter, TraversalDirection, ValidationError,
};
pub use nodespace_types::{
    AiChatBase, AiChatCompletedWrite, AiChatMessage, AiChatMessageRole, AiChatNativeNode,
    AiChatPendingDeletion, AiChatProvider, AiChatPtyNode, AiChatResolvedEntity,
    AiChatSessionStatus, AiChatTurnOutcome, AiChatTurnStatus, NODESPACE_AGENT,
};
pub use schema::{RelationshipDirection, SchemaField, SchemaFieldType, SchemaProtectionLevel};
pub use time::{SystemTimeProvider, TimeProvider};

// Export type-safe wrappers
pub use embedding::{ChunkInfo, Embedding, EmbeddingConfig, EmbeddingSearchResult, NewEmbedding};
pub use nodespace_types::SchemaNode;
pub use nodespace_types::{SchemaChildrenRule, SchemaParentRule};
pub use nodespace_types::{SkillFields, DEFAULT_SKILL_MAX_ITERATIONS, SKILL_NODE_TYPE};

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
// generic update pipeline (`NodeService::update_task_node` and its siblings).
// Defined once in nodespace-types so the Tauri command layer deserializes the
// same struct the service consumes.
pub use nodespace_types::{
    CollectionNodeUpdate, DatabaseSettingsNodeUpdate, PersonNodeUpdate, ProjectNodeUpdate,
    QueryNodeUpdate, SkillNodeUpdate, TaskNodeUpdate,
};

// The vocabularies of the task and project fields: each type's own status,
// and the priority scale the two share.
pub use nodespace_types::{Priority, ProjectStatus, TaskStatus};

// The stored query's typed fields — the only reader of a query node's
// properties (see `QueryDefinition::from_fields` for the execution mapping).
pub use nodespace_types::{PlayFields, PlayNodeUpdate, PlaySuspensionReason, PLAY_NODE_TYPE};
pub use nodespace_types::{QueryFields, QueryGeneratedBy, QUERY_NODE_TYPE};

// The lifecycle-status allow-list and its validator live in nodespace-types
// (owner of the `Node`/`NodeUpdate` types), re-exported here as the single
// source of truth for the storage-layer write guard.
pub use nodespace_types::{is_valid_lifecycle_status, LIFECYCLE_STATUSES};
