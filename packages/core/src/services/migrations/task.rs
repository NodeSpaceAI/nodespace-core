//! Task Schema Migrations
//!
//! This module contains migration transforms for the Task entity schema.
//! Each migration upgrades nodes from one schema version to the next.
//!
//! ## Migration History
//!
//! - **v1 → v2**: Added `priority` field (default: 0)
//! - **v2 → v3**: Added `assignee` field (default: null)
//! - **v3 → v4**: Renamed status values (OPEN → TODO, IN_PROGRESS → DOING)
//!
//! ## Usage
//!
//! ```no_run
//! # use nodespace_core::services::migration_registry::MigrationRegistry;
//! # use nodespace_core::services::migrations::task;
//! let mut registry = MigrationRegistry::new();
//!
//! // Register all task migrations
//! task::register_migrations(&mut registry);
//! ```

use crate::models::Node;
use crate::services::migration_registry::MigrationRegistry;
use crate::services::NodeServiceError;
use serde_json::json;

/// Register all task schema migrations
///
/// This function registers all migration transforms for the task schema
/// with the provided migration registry.
///
/// # Arguments
///
/// * `registry` - The migration registry to register transforms with
///
/// # Example
///
/// ```no_run
/// # use nodespace_core::services::migration_registry::MigrationRegistry;
/// # use nodespace_core::services::migrations::task;
/// let mut registry = MigrationRegistry::new();
/// task::register_migrations(&mut registry);
/// ```
pub fn register_migrations(registry: &mut MigrationRegistry) {
    registry.register_migration("task", 1, 2, migrate_v1_to_v2);
    registry.register_migration("task", 2, 3, migrate_v2_to_v3);
    registry.register_migration("task", 3, 4, migrate_v3_to_v4);
}

/// Migrate task from v1 to v2: Add priority field
///
/// Adds a `priority` field with default value 0 to all task nodes.
///
/// # Schema Changes
///
/// - Added: `priority` (number, default: 0)
///
/// # Example
///
/// **Before (v1):**
/// ```json
/// {
///   "_schema_version": 1,
///   "status": "OPEN"
/// }
/// ```
///
/// **After (v2):**
/// ```json
/// {
///   "_schema_version": 2,
///   "status": "OPEN",
///   "priority": 0
/// }
/// ```
fn migrate_v1_to_v2(node: &Node) -> Result<Node, NodeServiceError> {
    let mut node = node.clone();

    if let Some(obj) = node.properties.as_object_mut() {
        // Add priority field with default value
        obj.insert("priority".to_string(), json!(0));

        // Update schema version
        obj.insert("_schema_version".to_string(), json!(2));
    }

    Ok(node)
}

/// Migrate task from v2 to v3: Add assignee field
///
/// Adds an `assignee` field (nullable) to track task ownership.
///
/// # Schema Changes
///
/// - Added: `assignee` (string | null, default: null)
///
/// # Example
///
/// **Before (v2):**
/// ```json
/// {
///   "_schema_version": 2,
///   "status": "OPEN",
///   "priority": 0
/// }
/// ```
///
/// **After (v3):**
/// ```json
/// {
///   "_schema_version": 3,
///   "status": "OPEN",
///   "priority": 0,
///   "assignee": null
/// }
/// ```
fn migrate_v2_to_v3(node: &Node) -> Result<Node, NodeServiceError> {
    let mut node = node.clone();

    if let Some(obj) = node.properties.as_object_mut() {
        // Add assignee field (nullable)
        obj.insert("assignee".to_string(), json!(null));

        // Update schema version
        obj.insert("_schema_version".to_string(), json!(3));
    }

    Ok(node)
}

/// Migrate task from v3 to v4: Rename status values
///
/// Updates status enum values to match new naming convention:
/// - OPEN → TODO
/// - IN_PROGRESS → DOING
/// - DONE → COMPLETED
///
/// # Schema Changes
///
/// - Modified: `status` enum values renamed
///
/// # Example
///
/// **Before (v3):**
/// ```json
/// {
///   "_schema_version": 3,
///   "status": "OPEN",
///   "priority": 0,
///   "assignee": null
/// }
/// ```
///
/// **After (v4):**
/// ```json
/// {
///   "_schema_version": 4,
///   "status": "TODO",
///   "priority": 0,
///   "assignee": null
/// }
/// ```
fn migrate_v3_to_v4(node: &Node) -> Result<Node, NodeServiceError> {
    let mut node = node.clone();

    if let Some(obj) = node.properties.as_object_mut() {
        // Rename status values
        if let Some(status) = obj.get("status").and_then(|v| v.as_str()) {
            let new_status = match status {
                "OPEN" => "TODO",
                "IN_PROGRESS" => "DOING",
                "DONE" => "COMPLETED",
                other => other, // Preserve unknown values
            };
            obj.insert("status".to_string(), json!(new_status));
        }

        // Update schema version
        obj.insert("_schema_version".to_string(), json!(4));
    }

    Ok(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_task_node(version: u32, status: &str) -> Node {
        Node::new(
            "task".to_string(),
            "Test task".to_string(),
            None,
            json!({
                "_schema_version": version,
                "status": status
            }),
        )
    }

    #[test]
    fn test_migrate_v1_to_v2_adds_priority() {
        let node = create_task_node(1, "OPEN");
        let migrated = migrate_v1_to_v2(&node).unwrap();

        assert_eq!(migrated.properties["_schema_version"], 2);
        assert_eq!(migrated.properties["priority"], 0);
        assert_eq!(migrated.properties["status"], "OPEN"); // Preserved
    }

    #[test]
    fn test_migrate_v2_to_v3_adds_assignee() {
        let mut node = create_task_node(2, "OPEN");
        if let Some(obj) = node.properties.as_object_mut() {
            obj.insert("priority".to_string(), json!(5));
        }

        let migrated = migrate_v2_to_v3(&node).unwrap();

        assert_eq!(migrated.properties["_schema_version"], 3);
        assert_eq!(migrated.properties["assignee"], json!(null));
        assert_eq!(migrated.properties["priority"], 5); // Preserved
    }

    #[test]
    fn test_migrate_v3_to_v4_renames_status_open() {
        let node = create_task_node(3, "OPEN");
        let migrated = migrate_v3_to_v4(&node).unwrap();

        assert_eq!(migrated.properties["_schema_version"], 4);
        assert_eq!(migrated.properties["status"], "TODO");
    }

    #[test]
    fn test_migrate_v3_to_v4_renames_status_in_progress() {
        let node = create_task_node(3, "IN_PROGRESS");
        let migrated = migrate_v3_to_v4(&node).unwrap();

        assert_eq!(migrated.properties["_schema_version"], 4);
        assert_eq!(migrated.properties["status"], "DOING");
    }

    #[test]
    fn test_migrate_v3_to_v4_renames_status_done() {
        let node = create_task_node(3, "DONE");
        let migrated = migrate_v3_to_v4(&node).unwrap();

        assert_eq!(migrated.properties["_schema_version"], 4);
        assert_eq!(migrated.properties["status"], "COMPLETED");
    }

    #[test]
    fn test_migrate_v3_to_v4_preserves_unknown_status() {
        let node = create_task_node(3, "CUSTOM_STATUS");
        let migrated = migrate_v3_to_v4(&node).unwrap();

        assert_eq!(migrated.properties["_schema_version"], 4);
        assert_eq!(migrated.properties["status"], "CUSTOM_STATUS");
    }

    #[test]
    fn test_migration_chain_v1_to_v4() {
        let mut registry = MigrationRegistry::new();
        register_migrations(&mut registry);

        let node = create_task_node(1, "OPEN");
        let migrated = registry.apply_migrations(&node, 4).unwrap();

        assert_eq!(migrated.properties["_schema_version"], 4);
        assert_eq!(migrated.properties["status"], "TODO"); // OPEN → TODO
        assert_eq!(migrated.properties["priority"], 0); // Added in v1→v2
        assert_eq!(migrated.properties["assignee"], json!(null)); // Added in v2→v3
    }

    #[test]
    fn test_register_migrations_adds_all() {
        let mut registry = MigrationRegistry::new();
        assert_eq!(registry.migration_count(), 0);

        register_migrations(&mut registry);

        assert_eq!(registry.migration_count(), 3);
        assert!(registry.has_migration_path("task", 1, 4));
    }

    #[test]
    fn test_migration_preserves_custom_fields() {
        let mut node = create_task_node(1, "OPEN");
        if let Some(obj) = node.properties.as_object_mut() {
            obj.insert("custom_field".to_string(), json!("custom_value"));
            obj.insert("tags".to_string(), json!(["important", "urgent"]));
        }

        let migrated = migrate_v1_to_v2(&node).unwrap();

        // New field added
        assert_eq!(migrated.properties["priority"], 0);

        // Custom fields preserved
        assert_eq!(migrated.properties["custom_field"], "custom_value");
        assert_eq!(migrated.properties["tags"][0], "important");
        assert_eq!(migrated.properties["tags"][1], "urgent");
    }
}
