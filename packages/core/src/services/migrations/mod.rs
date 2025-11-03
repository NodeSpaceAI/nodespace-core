//! Schema Migration Implementations
//!
//! This module contains concrete migration transforms for different entity schemas.
//! Each submodule provides migration functions for a specific entity type.
//!
//! ## Available Migrations
//!
//! - `task` - Task schema migrations (v1→v2, v2→v3, v3→v4)
//!
//! ## Example Usage
//!
//! ```no_run
//! # use nodespace_core::services::migration_registry::MigrationRegistry;
//! # use nodespace_core::services::migrations;
//! let mut registry = MigrationRegistry::new();
//!
//! // Register all migrations for all schemas
//! migrations::task::register_migrations(&mut registry);
//! // migrations::person::register_migrations(&mut registry); // Future
//! // migrations::project::register_migrations(&mut registry); // Future
//! ```

pub mod task;
