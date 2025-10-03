//! NodeSpace Core Business Logic Layer
//!
//! This crate provides the core data management, node operations, and service orchestration
//! for the NodeSpace knowledge management system.
//!
//! # Architecture
//!
//! - **Pure JSON Schema**: All entity data stored in `properties` field (no complementary tables)
//! - **Schema-as-Node**: Schemas stored as nodes with `node_type = "schema"`
//! - **libsql/Turso**: Embedded SQLite-compatible database with sync capability
//! - **Zero Migration Risk**: No ALTER TABLE ever required on user machines
//!
//! # Modules
//!
//! - [`models`] - Data structures (Node, Task, Person, etc.)
//! - [`behaviors`] - Node type system and trait-based behaviors
//! - [`services`] - Business services (NodeService, SchemaService, etc.)
//! - [`db`] - Database layer with libsql integration
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
pub use db::{DatabaseError, DatabaseService};
pub use models::{
    FilterOperator, Node, NodeFilter, NodeUpdate, OrderBy, PropertyFilter, ValidationError,
};
