//! Dynamic Index Management for SurrealDB
//!
//! **STATUS: TEMPORARILY DISABLED** (Issue #481)
//!
//! This module is temporarily stubbed out during the SurrealDB migration.
//! Dynamic indexing will be re-implemented with SurrealDB-native index management.
//!
//! TODO(#481): Implement SurrealDB index management
//! - Use SurrealDB's DEFINE INDEX syntax
//! - Implement query-driven index creation
//! - Adapt to SurrealDB's indexing capabilities

use crate::db::error::DatabaseError;

/// Index manager for SurrealDB (temporarily disabled)
pub struct IndexManager {}

impl IndexManager {
    /// Create a new index manager (returns stub)
    pub fn new() -> Self {
        tracing::warn!("IndexManager temporarily disabled during SurrealDB migration (Issue #481)");
        Self {}
    }

    /// Stub: Create JSON path index
    pub fn create_json_path_index(
        &self,
        _node_type: &str,
        _property_name: &str,
    ) -> Result<(), DatabaseError> {
        // No-op: SurrealDB handles indexing differently
        Ok(())
    }
}

impl Default for IndexManager {
    fn default() -> Self {
        Self::new()
    }
}
