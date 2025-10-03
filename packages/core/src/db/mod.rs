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

mod database;
mod error;

pub use database::DatabaseService;
pub use error::DatabaseError;
