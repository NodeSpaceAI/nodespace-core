//! Database Layer
//!
//! This module handles all database interactions using SurrealDB:
//!
//! - Database initialization and connection management
//! - SCHEMALESS storage for flexible node properties
//! - Built-in version history support (future feature)
//! - Fine-grained RBAC for collaborative sync (future feature)
//!
//! # Architecture
//!
//! NodeSpace uses SurrealDB as its primary and only database backend.
//! SurrealDB was chosen for:
//!
//! - Built-in record versioning (for version history)
//! - Native RBAC support (for collaborative sync with permissions)
//! - High performance query capabilities
//! - Embedded deployment (no external dependencies)
//!
//! For architecture details, see `/docs/architecture/data/surrealdb-schema-design.md`

mod error;
pub mod events;
pub mod fractional_ordering;
mod index_manager;
mod surreal_store;

pub use error::DatabaseError;
pub use events::{DomainEvent, EdgeRelationship, HierarchyRelationship, MentionRelationship};
pub use fractional_ordering::FractionalOrderCalculator;
pub use index_manager::IndexManager;
pub use surreal_store::{EdgeRecord, EmbeddedStore, HttpStore, SurrealStore};
