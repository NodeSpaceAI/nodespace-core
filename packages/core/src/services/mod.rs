//! Business Services
//!
//! This module contains the core business logic services:
//!
//! - `NodeService` - CRUD operations and hierarchy management
//! - `SchemaService` - Dynamic schema creation and validation (planned)
//! - `SearchService` - Semantic search and query operations (planned)
//!
//! Services coordinate between the database layer and application logic,
//! implementing business rules and orchestrating complex operations.

pub mod error;
pub mod node_service;

pub use error::NodeServiceError;
pub use node_service::NodeService;
