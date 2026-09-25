//! Shared wire types for NodeSpace.
//!
//! This crate provides the type definitions and conversion functions shared
//! between `nodespace-core` (storage layer), the Tauri command layer
//! (`packages/desktop-app/src-tauri`), and `nodespace-cli` (which uses
//! `flatten_namespaced_properties` to honour the same property contract).
//! All re-use these types directly, eliminating the hand-synced mirror that
//! previously lived in `src-tauri/src/types.rs`.
//!
//! # Dependencies
//!
//! Intentionally minimal: `serde`, `serde_json`, `chrono`, `uuid`,
//! `thiserror`. No database, HTTP, or NLP dependencies.

mod ai_chat;
mod convert;
mod helpers;
mod node;
mod person;
mod project;
mod schema;
mod task;

pub use ai_chat::{AiChatMessage, AiChatNode};
pub use convert::{
    flat_properties_view, flatten_namespaced_properties, flatten_namespaced_properties_at_scope,
    node_to_typed_value, nodes_to_typed_values, promoted_fields,
};
pub use helpers::{is_valid_lifecycle_status, LIFECYCLE_STATUSES};
pub use node::{
    DeleteResult, Node, NodeQuery, NodeReference, NodeUpdate, OrderBy, ValidationError,
};
pub use person::{PersonNode, PersonNodeUpdate};
pub use project::{ProjectNode, ProjectNodeUpdate, DEFAULT_PROJECT_STATUS};
pub use schema::{
    derive_friendly_name, EdgeField, EnumValue, RelationshipCardinality, RelationshipDirection,
    SchemaField, SchemaNode, SchemaProtectionLevel, SchemaRelationship,
};
pub use task::{TaskNode, TaskNodeUpdate, TaskPriority, TaskStatus};
