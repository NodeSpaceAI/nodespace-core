//! Schema Migration Implementations
//!
//! This module contains concrete migration transforms for different entity schemas.
//! Each submodule provides migration functions for a specific entity type.
//!
//! ## Available Migrations
//!
//! Currently no migrations are registered. Migration infrastructure exists for
//! future schema evolution post-deployment.
//!
//! ## Example Usage
//!
//! ```no_run
//! # use nodespace_core::services::migration_registry::MigrationRegistry;
//! # use nodespace_core::services::migrations;
//! let mut registry = MigrationRegistry::new();
//!
//! // Future migrations will be registered here
//! // migrations::task::register_migrations(&mut registry);
//! // migrations::person::register_migrations(&mut registry);
//! ```
