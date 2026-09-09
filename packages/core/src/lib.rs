#![recursion_limit = "512"]
//! NodeSpace Core Business Logic Layer
//!
//! This crate provides the core data management, node operations, and service orchestration
//! for the NodeSpace knowledge management system.
//!
//! # Architecture
//!
//! - **SQLite (libsql)**: Embedded database with sqlite-vec for vector search
//! - **SCHEMALESS Storage**: Flexible property storage for dynamic node types
//! - **Node Type System**: Trait-based behaviors for validation and processing
//! - **Future Features**: Version history and collaborative sync with permissions
//!
//! # Modules
//!
//! - [`models`] - Data structures (Node, Task, Person, etc.)
//! - [`behaviors`] - Node type system and trait-based behaviors
//! - [`services`] - Business services (NodeService, etc.)
//! - [`db`] - Database layer with SQLite (libsql) integration
//! - [`markdown`] - Markdown import/export and templating (library)
//! - [`schema`] - Schema creation and updates (library)

pub mod agent_params;
pub mod behaviors;
pub mod conflict_sweep;
pub mod db;
pub mod markdown;
pub mod models;
pub mod node_batch;
pub mod ops;
pub mod playbook;
pub mod schema;
pub mod services;
pub mod utils;

// Re-exports
pub use behaviors::{
    AiChatNodeBehavior, CollectionNodeBehavior, CustomNodeBehavior, DateNodeBehavior, NodeBehavior,
    NodeBehaviorRegistry, ProcessingError, TaskNodeBehavior, TextNodeBehavior,
};
pub use db::{
    DatabaseError, DomainEvent, EventEnvelope, EventMetadata, PlaybookExecutionContext,
    PropertyChange, RelationshipEvent, RelationshipRecord, SqliteStore,
};
pub use models::{
    FilterOperator, Node, NodeFilter, NodeQuery, NodeUpdate, OrderBy, PropertyFilter, SchemaNode,
    TaskNode, TaskNodeUpdate, TaskStatus, ValidationError,
};
pub use playbook::PlaybookEngine;
pub use services::{
    CreateNodeParams, InsertPosition, InsertPositionOwned, NodeService, NodeServiceError,
};
