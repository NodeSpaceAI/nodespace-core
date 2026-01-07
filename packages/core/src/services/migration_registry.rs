//! Schema Migration Registry
//!
//! This module provides infrastructure for managing schema migration transforms
//! using a lazy migration strategy. Migrations are applied on-demand when nodes
//! are accessed, rather than in bulk operations.
//!
//! ## Architecture
//!
//! - **Lazy Migration**: Nodes upgrade when accessed, not via bulk operations
//! - **Version Tracking**: Each node tracks `_schema_version` in properties
//! - **Transform Functions**: Pure functions that upgrade nodes (v1→v2, v2→v3)
//! - **Migration Chaining**: Automatically chains transforms (v1→v2→v3)
//! - **Desktop-Safe**: No coordinated migrations across installations
//!
//! ## Example Usage
//!
//! ```no_run
//! # use nodespace_core::services::migration_registry::MigrationRegistry;
//! # use nodespace_core::models::Node;
//! # use nodespace_core::services::NodeServiceError;
//! # use serde_json::json;
//! # fn main() -> Result<(), NodeServiceError> {
//! let mut registry = MigrationRegistry::new();
//!
//! // Register a migration transform
//! // Issue #794: Properties are namespaced under properties[node_type]
//! registry.register_migration(
//!     "task",    // schema ID
//!     1,         // from version
//!     2,         // to version
//!     |node| {
//!         // Transform the node (e.g., add new field with default value)
//!         let mut node = node.clone();
//!         // Issue #794: Write to namespaced format
//!         if let Some(task_ns) = node
//!             .properties
//!             .as_object_mut()
//!             .and_then(|obj| obj.get_mut("task"))
//!             .and_then(|v| v.as_object_mut())
//!         {
//!             task_ns.insert("priority".to_string(), json!(0));
//!             task_ns.insert("_schema_version".to_string(), json!(2));
//!         }
//!         Ok(node)
//!     }
//! );
//!
//! // Apply migrations to upgrade a node
//! // Issue #794: Properties are namespaced under properties[node_type]
//! let mut old_node = Node::new("task".to_string(), "Test".to_string(), json!({"task": {"_schema_version": 1}}));
//!
//! let upgraded = registry.apply_migrations(&old_node, 2)?;
//! assert_eq!(upgraded.properties["task"]["_schema_version"], 2);
//! # Ok(())
//! # }
//! ```

use crate::models::Node;
use crate::services::NodeServiceError;
use std::collections::HashMap;

/// Type alias for migration transform functions
///
/// Transform functions are pure: they take a node and return a transformed node.
/// They should not have side effects and must update the node's `_schema_version`.
///
/// # Arguments
///
/// * `node` - The node to transform (at version N)
///
/// # Returns
///
/// The transformed node (at version N+1), or an error if migration fails
pub type MigrationTransform = fn(&Node) -> Result<Node, NodeServiceError>;

/// Helper function to get _schema_version from a node's properties.
///
/// Issue #794: Properties are now namespaced under properties[node_type][field_name].
/// This function reads _schema_version from the namespaced location.
///
/// # Arguments
///
/// * `node` - The node to get the schema version from
///
/// # Returns
///
/// The schema version as u32, or None if not found
fn get_schema_version(node: &Node) -> Option<u32> {
    // Issue #794: Read from namespaced format: properties[node_type]._schema_version
    node.properties
        .get(&node.node_type)
        .and_then(|type_props| type_props.get("_schema_version"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32)
}

/// Registry for schema migration transforms
///
/// Stores migration functions keyed by (schema_id, from_version, to_version)
/// and provides methods to apply migrations with automatic chaining.
///
/// # Example
///
/// ```ignore
/// use nodespace_core::services::migration_registry::MigrationRegistry;
/// let mut registry = MigrationRegistry::new();
///
/// // Register migrations
/// registry.register_migration("task", 1, 2, task_v1_to_v2);
/// registry.register_migration("task", 2, 3, task_v2_to_v3);
///
/// // Apply chained migrations (v1→v2→v3)
/// let upgraded = registry.apply_migrations(&node, 3)?;
/// ```
pub struct MigrationRegistry {
    /// Map of (schema_id, from_version, to_version) → transform function
    migrations: HashMap<(String, u32, u32), MigrationTransform>,
}

impl MigrationRegistry {
    /// Create a new empty migration registry
    pub fn new() -> Self {
        Self {
            migrations: HashMap::new(),
        }
    }

    /// Register a migration transform for a schema version upgrade
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema identifier (e.g., "task", "person")
    /// * `from_version` - The source schema version
    /// * `to_version` - The target schema version
    /// * `transform` - The migration function to apply
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::migration_registry::MigrationRegistry;
    /// # use nodespace_core::models::Node;
    /// # use nodespace_core::services::NodeServiceError;
    /// # use serde_json::json;
    /// let mut registry = MigrationRegistry::new();
    ///
    /// registry.register_migration("task", 1, 2, |node| {
    ///     let mut node = node.clone();
    ///     if let Some(obj) = node.properties.as_object_mut() {
    ///         // Add new field with default value
    ///         obj.insert("priority".to_string(), json!(0));
    ///         obj.insert("_schema_version".to_string(), json!(2));
    ///     }
    ///     Ok(node)
    /// });
    /// ```
    pub fn register_migration(
        &mut self,
        schema_id: impl Into<String>,
        from_version: u32,
        to_version: u32,
        transform: MigrationTransform,
    ) {
        let key = (schema_id.into(), from_version, to_version);
        self.migrations.insert(key, transform);
    }

    /// Apply migrations to upgrade a node from its current version to a target version
    ///
    /// This method automatically chains migrations if needed. For example, if a node
    /// is at version 1 and needs to reach version 3, it will apply v1→v2, then v2→v3.
    ///
    /// # Arguments
    ///
    /// * `node` - The node to migrate (must have `_schema_version` in properties)
    /// * `target_version` - The target schema version
    ///
    /// # Returns
    ///
    /// The migrated node at the target version
    ///
    /// # Errors
    ///
    /// - `SerializationError`: Node doesn't have `_schema_version` property
    /// - `SerializationError`: Missing migration path from current to target version
    /// - Any error returned by migration transform functions
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::migration_registry::MigrationRegistry;
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// # let mut registry = MigrationRegistry::new();
    /// # let node = Node::new("task".to_string(), "Test".to_string(), json!({"_schema_version": 1}));
    /// // Automatically chains v1→v2→v3
    /// let upgraded = registry.apply_migrations(&node, 3)?;
    /// assert_eq!(upgraded.properties["_schema_version"], 3);
    /// # Ok::<(), nodespace_core::services::NodeServiceError>(())
    /// ```
    pub fn apply_migrations(
        &self,
        node: &Node,
        target_version: u32,
    ) -> Result<Node, NodeServiceError> {
        // Get current version from node properties (Issue #794: namespaced format)
        let current_version = get_schema_version(node).ok_or_else(|| {
            NodeServiceError::SerializationError(
                "Node missing _schema_version property".to_string(),
            )
        })?;

        // No migration needed if already at target version
        if current_version == target_version {
            return Ok(node.clone());
        }

        // No migration needed if already newer than target
        if current_version > target_version {
            return Ok(node.clone());
        }

        // Apply migration chain: current → current+1 → current+2 → ... → target
        let mut migrated_node = node.clone();
        let mut current = current_version;

        while current < target_version {
            let next_version = current + 1;
            let key = (node.node_type.clone(), current, next_version);

            // Find migration for this step
            let transform = self.migrations.get(&key).ok_or_else(|| {
                NodeServiceError::SerializationError(format!(
                    "No migration found for {} from version {} to {}",
                    node.node_type, current, next_version
                ))
            })?;

            // Apply transform
            migrated_node = transform(&migrated_node)?;

            // Verify version was updated (Issue #794: namespaced format)
            let new_version = get_schema_version(&migrated_node).ok_or_else(|| {
                NodeServiceError::SerializationError(format!(
                    "Migration {} v{}→v{} did not set _schema_version",
                    node.node_type, current, next_version
                ))
            })?;

            if new_version != next_version {
                return Err(NodeServiceError::SerializationError(format!(
                    "Migration {} v{}→v{} set version to {} instead of {}",
                    node.node_type, current, next_version, new_version, next_version
                )));
            }

            current = next_version;
        }

        Ok(migrated_node)
    }

    /// Check if a migration path exists from one version to another
    ///
    /// This checks if all intermediate migrations are registered.
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema identifier
    /// * `from_version` - Starting version
    /// * `to_version` - Target version
    ///
    /// # Returns
    ///
    /// `true` if all migration steps exist, `false` otherwise
    pub fn has_migration_path(&self, schema_id: &str, from_version: u32, to_version: u32) -> bool {
        if from_version >= to_version {
            return true;
        }

        let mut current = from_version;
        while current < to_version {
            let next = current + 1;
            let key = (schema_id.to_string(), current, next);
            if !self.migrations.contains_key(&key) {
                return false;
            }
            current = next;
        }

        true
    }

    /// Get the number of registered migrations
    pub fn migration_count(&self) -> usize {
        self.migrations.len()
    }
}

impl Default for MigrationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Helper to create a test node with version (Issue #794: namespaced format)
    fn create_node_with_version(version: u32) -> Node {
        Node::new(
            "task".to_string(),
            "Test task".to_string(),
            json!({
                "task": {
                    "_schema_version": version,
                    "status": "OPEN"
                }
            }),
        )
    }

    // Migration v1→v2: Add priority field (Issue #794: namespaced format)
    fn task_v1_to_v2(node: &Node) -> Result<Node, NodeServiceError> {
        let mut node = node.clone();
        if let Some(task_ns) = node
            .properties
            .as_object_mut()
            .and_then(|obj| obj.get_mut("task"))
            .and_then(|v| v.as_object_mut())
        {
            task_ns.insert("priority".to_string(), json!(0));
            task_ns.insert("_schema_version".to_string(), json!(2));
        }
        Ok(node)
    }

    // Migration v2→v3: Add assignee field (Issue #794: namespaced format)
    fn task_v2_to_v3(node: &Node) -> Result<Node, NodeServiceError> {
        let mut node = node.clone();
        if let Some(task_ns) = node
            .properties
            .as_object_mut()
            .and_then(|obj| obj.get_mut("task"))
            .and_then(|v| v.as_object_mut())
        {
            task_ns.insert("assignee".to_string(), json!(null));
            task_ns.insert("_schema_version".to_string(), json!(3));
        }
        Ok(node)
    }

    // Migration v3→v4: Rename status values (Issue #794: namespaced format)
    fn task_v3_to_v4(node: &Node) -> Result<Node, NodeServiceError> {
        let mut node = node.clone();
        if let Some(task_ns) = node
            .properties
            .as_object_mut()
            .and_then(|obj| obj.get_mut("task"))
            .and_then(|v| v.as_object_mut())
        {
            // Rename OPEN → TODO
            if let Some(status) = task_ns.get("status") {
                if status == "OPEN" {
                    task_ns.insert("status".to_string(), json!("TODO"));
                }
            }
            task_ns.insert("_schema_version".to_string(), json!(4));
        }
        Ok(node)
    }

    #[test]
    fn test_register_migration() {
        let mut registry = MigrationRegistry::new();
        assert_eq!(registry.migration_count(), 0);

        registry.register_migration("task", 1, 2, task_v1_to_v2);
        assert_eq!(registry.migration_count(), 1);
    }

    #[test]
    fn test_single_migration() {
        let mut registry = MigrationRegistry::new();
        registry.register_migration("task", 1, 2, task_v1_to_v2);

        let node = create_node_with_version(1);
        let migrated = registry.apply_migrations(&node, 2).unwrap();

        // Issue #794: Properties are namespaced under properties[node_type]
        assert_eq!(migrated.properties["task"]["_schema_version"], 2);
        assert_eq!(migrated.properties["task"]["priority"], 0);
        assert_eq!(migrated.properties["task"]["status"], "OPEN"); // Original field preserved
    }

    #[test]
    fn test_no_migration_needed_same_version() {
        let mut registry = MigrationRegistry::new();
        registry.register_migration("task", 1, 2, task_v1_to_v2);

        let node = create_node_with_version(2);
        let migrated = registry.apply_migrations(&node, 2).unwrap();

        // Should return clone without changes
        // Issue #794: Properties are namespaced under properties[node_type]
        assert_eq!(migrated.properties["task"]["_schema_version"], 2);
        assert!(migrated.properties["task"].get("priority").is_none()); // v2 field not added
    }

    #[test]
    fn test_no_migration_needed_newer_version() {
        let mut registry = MigrationRegistry::new();
        registry.register_migration("task", 1, 2, task_v1_to_v2);

        let node = create_node_with_version(3);
        let migrated = registry.apply_migrations(&node, 2).unwrap();

        // Should return clone without downgrading
        // Issue #794: Properties are namespaced under properties[node_type]
        assert_eq!(migrated.properties["task"]["_schema_version"], 3);
    }

    #[test]
    fn test_migration_chaining() {
        let mut registry = MigrationRegistry::new();
        registry.register_migration("task", 1, 2, task_v1_to_v2);
        registry.register_migration("task", 2, 3, task_v2_to_v3);
        registry.register_migration("task", 3, 4, task_v3_to_v4);

        let node = create_node_with_version(1);
        let migrated = registry.apply_migrations(&node, 4).unwrap();

        // All migrations should be applied
        // Issue #794: Properties are namespaced under properties[node_type]
        assert_eq!(migrated.properties["task"]["_schema_version"], 4);
        assert_eq!(migrated.properties["task"]["priority"], 0); // Added in v1→v2
        assert_eq!(migrated.properties["task"]["assignee"], json!(null)); // Added in v2→v3
        assert_eq!(migrated.properties["task"]["status"], "TODO"); // Changed in v3→v4
    }

    #[test]
    fn test_partial_migration_chain() {
        let mut registry = MigrationRegistry::new();
        registry.register_migration("task", 1, 2, task_v1_to_v2);
        registry.register_migration("task", 2, 3, task_v2_to_v3);
        registry.register_migration("task", 3, 4, task_v3_to_v4);

        // Start from v2, go to v4
        let node = create_node_with_version(2);
        let migrated = registry.apply_migrations(&node, 4).unwrap();

        // Issue #794: Properties are namespaced under properties[node_type]
        assert_eq!(migrated.properties["task"]["_schema_version"], 4);
        assert!(migrated.properties["task"].get("priority").is_none()); // Not added (started at v2)
        assert_eq!(migrated.properties["task"]["assignee"], json!(null)); // Added in v2→v3
        assert_eq!(migrated.properties["task"]["status"], "TODO"); // Changed in v3→v4
    }

    #[test]
    fn test_missing_migration_error() {
        let mut registry = MigrationRegistry::new();
        registry.register_migration("task", 1, 2, task_v1_to_v2);
        // Missing v2→v3 migration

        let node = create_node_with_version(1);
        let result = registry.apply_migrations(&node, 3);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, NodeServiceError::SerializationError(_)));
    }

    #[test]
    fn test_missing_schema_version_error() {
        let registry = MigrationRegistry::new();

        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"status": "OPEN"}), // No _schema_version
        );

        let result = registry.apply_migrations(&node, 2);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_migration_path() {
        let mut registry = MigrationRegistry::new();
        registry.register_migration("task", 1, 2, task_v1_to_v2);
        registry.register_migration("task", 2, 3, task_v2_to_v3);

        assert!(registry.has_migration_path("task", 1, 2));
        assert!(registry.has_migration_path("task", 1, 3));
        assert!(!registry.has_migration_path("task", 1, 4)); // Missing v3→v4
        assert!(registry.has_migration_path("task", 2, 2)); // Same version
    }

    #[test]
    fn test_migration_preserves_other_fields() {
        let mut registry = MigrationRegistry::new();
        registry.register_migration("task", 1, 2, task_v1_to_v2);

        // Issue #794: Properties are namespaced under properties[node_type]
        let node = Node::new(
            "task".to_string(),
            "Test task".to_string(),
            json!({
                "task": {
                    "_schema_version": 1,
                    "status": "OPEN",
                    "custom_field": "custom_value",
                    "nested": {
                        "deep": "value"
                    }
                }
            }),
        );

        let migrated = registry.apply_migrations(&node, 2).unwrap();

        // New field added (Issue #794: namespaced)
        assert_eq!(migrated.properties["task"]["priority"], 0);

        // All original fields preserved (Issue #794: namespaced)
        assert_eq!(migrated.properties["task"]["status"], "OPEN");
        assert_eq!(migrated.properties["task"]["custom_field"], "custom_value");
        assert_eq!(migrated.properties["task"]["nested"]["deep"], "value");
    }
}
