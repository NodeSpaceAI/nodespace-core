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
//! For architecture details, see `../nodespace-docs/archived/architecture/data/surrealdb-only-architecture.md`

mod error;
pub mod events;
pub mod fractional_ordering;
mod index_manager;
mod surreal_store;

pub use error::DatabaseError;
pub use events::{DomainEvent, RelationshipEvent};
pub use fractional_ordering::FractionalOrderCalculator;
pub use index_manager::IndexManager;
pub use surreal_store::{RelationshipRecord, StoreChange, StoreOperation, SurrealStore};

/// Extract the key string from a `RecordId` (table-qualified record identifier).
///
/// Returns only the key portion (e.g., `"abc-123"` from `node:abc-123`).
/// Use this when you need a bare node ID suitable for passing to `get_node()`.
/// For event emission IDs that require the full `table:key` format, build that
/// string explicitly: `format!("{}:{}", id.table, extract_record_key(id))`.
pub fn extract_record_key(record_id: &surrealdb::types::RecordId) -> String {
    use surrealdb::types::RecordIdKey;
    match &record_id.key {
        RecordIdKey::String(s) => s.clone(),
        RecordIdKey::Number(n) => n.to_string(),
        RecordIdKey::Uuid(u) => u.to_string(),
        other => format!("{other:?}"),
    }
}
