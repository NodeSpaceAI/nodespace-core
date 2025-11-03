//! Data Models
//!
//! This module contains the core data structures used throughout NodeSpace:
//!
//! - `Node` - Universal node model for all content types
//! - Type-specific structures (Task, Person, etc.) built on Node foundation
//!
//! All entities use the Pure JSON schema approach with data stored in the
//! `properties` field of the universal `nodes` table.

mod node;
pub mod schema;
pub mod time;

pub use node::{
    DeleteResult, FilterOperator, Node, NodeFilter, NodeQuery, NodeUpdate, OrderBy, PropertyFilter,
    ValidationError,
};
pub use schema::{ProtectionLevel, SchemaDefinition, SchemaField};
pub use time::{SystemTimeProvider, TimeProvider};
