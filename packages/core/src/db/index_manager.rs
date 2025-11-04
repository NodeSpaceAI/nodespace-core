//! Dynamic Index Management for Pure JSON Schema
//!
//! This module provides dynamic JSON path index creation based on query patterns.
//! Instead of predefined indexes on JSON properties, indexes are created on-demand
//! when query frequency exceeds a threshold.
//!
//! # Architecture
//!
//! - **Query-driven**: Creates indexes based on actual usage patterns
//! - **JSON path indexes**: Uses SQLite's JSON path operators for indexing
//! - **No ALTER TABLE risk**: All indexes are CREATE INDEX (safe for desktop)
//! - **Dynamic adaptation**: Database schema evolves with user's query patterns
//!
//! For detailed specifications, see:
//! `/docs/architecture/data/storage-architecture.md`

use crate::db::error::DatabaseError;
use libsql::Database;
use std::sync::Arc;

/// Manages dynamic JSON path indexes for Pure JSON schema
///
/// Creates indexes on JSON properties when query frequency exceeds threshold,
/// enabling performance optimization without schema migrations.
///
/// # Examples
///
/// ```no_run
/// use nodespace_core::db::IndexManager;
/// use std::sync::Arc;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let db = Arc::new(libsql::Builder::new_local(":memory:").build().await?);
///     let index_manager = IndexManager::new(db);
///
///     // Create JSON path index when needed
///     index_manager.create_json_path_index("task", "status").await?;
///
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct IndexManager {
    /// libsql database connection (shared with DatabaseService)
    db: Arc<Database>,
}

impl IndexManager {
    /// Create a new IndexManager with the specified database
    ///
    /// # Arguments
    ///
    /// * `db` - Arc-wrapped Database instance (shared with DatabaseService)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::IndexManager;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = Arc::new(libsql::Builder::new_local(":memory:").build().await?);
    /// let index_manager = IndexManager::new(db);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Get a database connection with busy timeout configured for async contexts
    ///
    /// This helper method ensures IndexManager operations are safe in Tokio's
    /// multi-threaded async runtime by setting a 5-second busy timeout on each
    /// connection. This allows concurrent operations to wait and retry instead
    /// of failing immediately when the database is locked.
    ///
    /// # Why This Matters
    ///
    /// Similar to `DatabaseService::connect_with_timeout()`, this prevents SQLite
    /// thread-safety violations when the Tokio runtime moves futures between
    /// threads at `.await` points.
    async fn connect_with_timeout(&self) -> Result<libsql::Connection, DatabaseError> {
        let conn = self.db.connect().map_err(DatabaseError::LibsqlError)?;

        // Set busy timeout on this connection
        // PRAGMA statements return rows, so we must use query() instead of execute()
        let mut stmt = conn
            .prepare("PRAGMA busy_timeout = 5000")
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare busy timeout: {}", e))
            })?;
        let _ = stmt.query(()).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to set busy timeout: {}", e))
        })?;

        Ok(conn)
    }

    /// Validate identifier for use in SQL statements
    ///
    /// Ensures identifiers contain only alphanumeric characters and underscores,
    /// and start with a letter or underscore. This prevents SQL injection attacks.
    ///
    /// # Arguments
    ///
    /// * `identifier` - The identifier to validate
    /// * `name` - Description of the identifier (for error messages)
    ///
    /// # Errors
    ///
    /// Returns `DatabaseError` if the identifier is invalid
    fn validate_identifier(identifier: &str, name: &str) -> Result<(), DatabaseError> {
        // Must not be empty
        if identifier.is_empty() {
            return Err(DatabaseError::sql_execution(format!(
                "Invalid {}: identifier cannot be empty",
                name
            )));
        }

        // Must start with letter or underscore
        let first_char = identifier.chars().next().unwrap();
        if !first_char.is_alphabetic() && first_char != '_' {
            return Err(DatabaseError::sql_execution(format!(
                "Invalid {}: '{}' must start with a letter or underscore",
                name, identifier
            )));
        }

        // All characters must be alphanumeric or underscore
        if !identifier.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(DatabaseError::sql_execution(format!(
                "Invalid {}: '{}' contains invalid characters (only alphanumeric and underscore allowed)",
                name, identifier
            )));
        }

        Ok(())
    }

    /// Create a JSON path index on a specific property field
    ///
    /// Creates an index using SQLite's `json_extract()` function to index
    /// specific paths within the `properties` JSON column.
    ///
    /// # Arguments
    ///
    /// * `node_type` - The node type to index (e.g., "task", "person")
    /// * `field_name` - The JSON property field to index (e.g., "status", "assignee")
    ///
    /// # Index Naming Convention
    ///
    /// Indexes are named: `idx_json_{node_type}_{field_name}`
    ///
    /// # Safety
    ///
    /// Uses CREATE INDEX IF NOT EXISTS to be idempotent and safe for repeated calls.
    ///
    /// # Errors
    ///
    /// Returns `DatabaseError` if index creation fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::IndexManager;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(libsql::Builder::new_local(":memory:").build().await?);
    /// # let index_manager = IndexManager::new(db);
    /// // Create index on task.status for faster filtering
    /// index_manager.create_json_path_index("task", "status").await?;
    ///
    /// // Create index on task.assignee for person lookup
    /// index_manager.create_json_path_index("task", "assignee").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_json_path_index(
        &self,
        node_type: &str,
        field_name: &str,
    ) -> Result<(), DatabaseError> {
        // Validate inputs to prevent SQL injection
        Self::validate_identifier(node_type, "node_type")?;
        Self::validate_identifier(field_name, "field_name")?;

        let index_name = format!("idx_json_{}_{}", node_type, field_name);
        let json_path = format!("$.{}", field_name);

        let sql = format!(
            "CREATE INDEX IF NOT EXISTS {} ON nodes(json_extract(properties, '{}')) \
             WHERE node_type = '{}'",
            index_name, json_path, node_type
        );

        let conn = self.connect_with_timeout().await?;

        conn.execute(&sql, ()).await.map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create JSON path index '{}': {}",
                index_name, e
            ))
        })?;

        Ok(())
    }

    /// Check if a JSON path index exists
    ///
    /// # Arguments
    ///
    /// * `node_type` - The node type (e.g., "task", "person")
    /// * `field_name` - The JSON property field (e.g., "status", "assignee")
    ///
    /// # Returns
    ///
    /// `true` if the index exists, `false` otherwise
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::IndexManager;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(libsql::Builder::new_local(":memory:").build().await?);
    /// # let index_manager = IndexManager::new(db);
    /// if !index_manager.index_exists("task", "status").await? {
    ///     index_manager.create_json_path_index("task", "status").await?;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn index_exists(
        &self,
        node_type: &str,
        field_name: &str,
    ) -> Result<bool, DatabaseError> {
        // Validate inputs to prevent SQL injection
        Self::validate_identifier(node_type, "node_type")?;
        Self::validate_identifier(field_name, "field_name")?;

        let index_name = format!("idx_json_{}_{}", node_type, field_name);

        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name=?")
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Index existence check failed: {}", e))
            })?;

        let mut rows = stmt.query([index_name]).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to query index existence: {}", e))
        })?;

        Ok(rows
            .next()
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to fetch index result: {}", e))
            })?
            .is_some())
    }

    /// List all JSON path indexes for a specific node type
    ///
    /// # Arguments
    ///
    /// * `node_type` - The node type to list indexes for
    ///
    /// # Returns
    ///
    /// Vector of field names that have JSON path indexes
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::IndexManager;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(libsql::Builder::new_local(":memory:").build().await?);
    /// # let index_manager = IndexManager::new(db);
    /// let task_indexes = index_manager.list_indexes("task").await?;
    /// println!("Task indexes: {:?}", task_indexes);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_indexes(&self, node_type: &str) -> Result<Vec<String>, DatabaseError> {
        // Validate input to prevent SQL injection
        Self::validate_identifier(node_type, "node_type")?;

        let pattern = format!("idx_json_{}_", node_type);

        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name LIKE ?")
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare list indexes query: {}", e))
            })?;

        let pattern_with_wildcard = format!("{}%", pattern);
        let mut rows = stmt
            .query([pattern_with_wildcard])
            .await
            .map_err(|e| DatabaseError::sql_execution(format!("Failed to query indexes: {}", e)))?;

        let mut field_names = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to fetch index row: {}", e))
        })? {
            let index_name: String = row.get(0).map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to get index name: {}", e))
            })?;

            // Extract field name from index name
            // idx_json_task_status -> status
            if let Some(field_name) = index_name.strip_prefix(&pattern) {
                field_names.push(field_name.to_string());
            }
        }

        Ok(field_names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn setup_test_db() -> (TempDir, Arc<Database>) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(libsql::Builder::new_local(&db_path).build().await.unwrap());

        // Create nodes table for testing
        // Use IndexManager's connect_with_timeout for proper concurrency support
        // This ensures the busy_timeout PRAGMA is set correctly, preventing race conditions
        // when tests run concurrently (see Issue #398)
        let index_manager = IndexManager::new(db.clone());
        let conn = index_manager.connect_with_timeout().await.unwrap();
        conn.execute(
            "CREATE TABLE nodes (
                id TEXT PRIMARY KEY,
                node_type TEXT NOT NULL,
                content TEXT NOT NULL,
                properties JSON NOT NULL DEFAULT '{}'
            )",
            (),
        )
        .await
        .unwrap();

        (temp_dir, db)
    }

    #[tokio::test]
    async fn test_create_json_path_index() {
        let (_temp_dir, db) = setup_test_db().await;
        let index_manager = IndexManager::new(db);

        index_manager
            .create_json_path_index("task", "status")
            .await
            .unwrap();

        assert!(index_manager.index_exists("task", "status").await.unwrap());
    }

    #[tokio::test]
    async fn test_index_idempotency() {
        let (_temp_dir, db) = setup_test_db().await;
        let index_manager = IndexManager::new(db);

        // Create index twice - should not error
        index_manager
            .create_json_path_index("task", "status")
            .await
            .unwrap();
        index_manager
            .create_json_path_index("task", "status")
            .await
            .unwrap();

        assert!(index_manager.index_exists("task", "status").await.unwrap());
    }

    #[tokio::test]
    async fn test_list_indexes() {
        let (_temp_dir, db) = setup_test_db().await;
        let index_manager = IndexManager::new(db);

        index_manager
            .create_json_path_index("task", "status")
            .await
            .unwrap();
        index_manager
            .create_json_path_index("task", "assignee")
            .await
            .unwrap();

        let indexes = index_manager.list_indexes("task").await.unwrap();

        assert_eq!(indexes.len(), 2);
        assert!(indexes.contains(&"status".to_string()));
        assert!(indexes.contains(&"assignee".to_string()));
    }

    #[tokio::test]
    async fn test_index_isolation_by_type() {
        let (_temp_dir, db) = setup_test_db().await;
        let index_manager = IndexManager::new(db);

        index_manager
            .create_json_path_index("task", "status")
            .await
            .unwrap();
        index_manager
            .create_json_path_index("person", "email")
            .await
            .unwrap();

        let task_indexes = index_manager.list_indexes("task").await.unwrap();
        let person_indexes = index_manager.list_indexes("person").await.unwrap();

        assert_eq!(task_indexes.len(), 1);
        assert_eq!(person_indexes.len(), 1);
        assert!(task_indexes.contains(&"status".to_string()));
        assert!(person_indexes.contains(&"email".to_string()));
    }

    #[tokio::test]
    async fn test_sql_injection_prevention_create_index() {
        let (_temp_dir, db) = setup_test_db().await;
        let index_manager = IndexManager::new(db);

        // Test SQL injection in node_type
        assert!(index_manager
            .create_json_path_index("task'; DROP TABLE nodes; --", "status")
            .await
            .is_err());

        // Test SQL injection in field_name
        assert!(index_manager
            .create_json_path_index("task", "status'); DROP TABLE nodes; --")
            .await
            .is_err());

        // Test WHERE clause injection
        assert!(index_manager
            .create_json_path_index("task' OR '1'='1", "status")
            .await
            .is_err());

        // Test empty identifiers
        assert!(index_manager
            .create_json_path_index("", "status")
            .await
            .is_err());
        assert!(index_manager
            .create_json_path_index("task", "")
            .await
            .is_err());

        // Test invalid characters
        assert!(index_manager
            .create_json_path_index("task-name", "status")
            .await
            .is_err());
        assert!(index_manager
            .create_json_path_index("task", "field.name")
            .await
            .is_err());

        // Verify valid identifiers still work
        assert!(index_manager
            .create_json_path_index("task", "status")
            .await
            .is_ok());
        assert!(index_manager
            .create_json_path_index("task_type", "field_name")
            .await
            .is_ok());
        assert!(index_manager
            .create_json_path_index("_private", "_internal")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn test_sql_injection_prevention_index_exists() {
        let (_temp_dir, db) = setup_test_db().await;
        let index_manager = IndexManager::new(db);

        // Test SQL injection attempts
        assert!(index_manager
            .index_exists("task'; --", "status")
            .await
            .is_err());
        assert!(index_manager
            .index_exists("task", "status'; --")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_sql_injection_prevention_list_indexes() {
        let (_temp_dir, db) = setup_test_db().await;
        let index_manager = IndexManager::new(db);

        // Test SQL injection attempts
        assert!(index_manager.list_indexes("task'; --").await.is_err());
        assert!(index_manager.list_indexes("task' OR '1'='1").await.is_err());

        // Verify valid identifier works
        assert!(index_manager.list_indexes("task").await.is_ok());
    }
}
