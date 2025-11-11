//! Database Layer
//!
//! This module handles all database interactions using libsql/Turso:
//!
//! - Database initialization and connection management
//! - Pure JSON schema queries with JSON path operators
//! - Migration system for schema evolution
//! - Transaction management and error handling
//!
//! The database layer uses the Pure JSON schema approach where all entity
//! data is stored in the `properties` field, eliminating the need for
//! ALTER TABLE operations on user machines.
//!
//! # Abstraction Layer (Phase 1 - Epic #461)
//!
//! The `NodeStore` trait provides a database abstraction layer that enables
//! multiple backend implementations (Turso, SurrealDB) without changing
//! business logic in NodeService.

mod database;
mod error;
mod index_manager;
mod node_store;
mod turso_store;

pub use database::{DatabaseService, DbCreateNodeParams, DbUpdateNodeParams};
pub use error::DatabaseError;
pub use index_manager::IndexManager;
pub use node_store::NodeStore;
pub use turso_store::TursoStore;
