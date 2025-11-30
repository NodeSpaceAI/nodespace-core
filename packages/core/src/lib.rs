//! NodeSpace Core Business Logic Layer
//!
//! This crate provides the core data management, node operations, and service orchestration
//! for the NodeSpace knowledge management system.
//!
//! # Architecture
//!
//! - **SurrealDB**: Embedded database with built-in versioning and RBAC support
//! - **SCHEMALESS Storage**: Flexible property storage for dynamic node types
//! - **Node Type System**: Trait-based behaviors for validation and processing
//! - **Future Features**: Version history and collaborative sync with permissions
//!
//! # Modules
//!
//! - [`models`] - Data structures (Node, Task, Person, etc.)
//! - [`behaviors`] - Node type system and trait-based behaviors
//! - [`services`] - Business services (NodeService, SchemaService, etc.)
//! - [`db`] - Database layer with SurrealDB integration
//! - [`mcp`] - MCP stdio server for AI agent integration

pub mod behaviors;
pub mod db;
pub mod mcp;
pub mod models;
pub mod services;

// Re-exports
pub use behaviors::{
    DateNodeBehavior, NodeBehavior, NodeBehaviorRegistry, ProcessingError, TaskNodeBehavior,
    TextNodeBehavior,
};
pub use db::{
    DatabaseError, DomainEvent, EdgeRecord, EdgeRelationship, HierarchyRelationship,
    MentionRelationship, SurrealStore,
};
pub use models::{
    FilterOperator, Node, NodeFilter, NodeQuery, NodeUpdate, OrderBy, PropertyFilter,
    ValidationError,
};
pub use services::{CreateNodeParams, NodeService, NodeServiceError};
