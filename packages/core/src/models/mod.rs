//! Data Models
//!
//! This module contains the core data structures used throughout NodeSpace:
//!
//! - `Node` - Universal node model for all content types
//! - Type-safe wrappers (TaskNode, TextNode, DateNode) for ergonomic access
//!
//! All entities use the Pure JSON schema approach with data stored in the
//! `properties` field of the universal `nodes` table.

mod node;
pub mod schema;
pub mod time;

// Type-safe node wrappers
mod date_node;
mod task_node;
mod text_node;

#[cfg(test)]
#[path = "task_node_test.rs"]
mod task_node_test;

#[cfg(test)]
#[path = "text_node_test.rs"]
mod text_node_test;

#[cfg(test)]
#[path = "date_node_test.rs"]
mod date_node_test;

pub use node::{
    DeleteResult, FilterOperator, Node, NodeFilter, NodeQuery, NodeUpdate, OrderBy, PropertyFilter,
    ValidationError,
};
pub use schema::{ProtectionLevel, SchemaDefinition, SchemaField};
pub use time::{SystemTimeProvider, TimeProvider};

// Export type-safe wrappers
pub use date_node::DateNode;
pub use task_node::{TaskNode, TaskStatus};
pub use text_node::TextNode;
