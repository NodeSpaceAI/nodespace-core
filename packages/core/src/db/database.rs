//! Database Connection Management
//!
//! This module provides the core database connection and initialization
//! functionality using libsql/Turso for NodeSpace's Pure JSON architecture.
//!
//! # Architecture
//!
//! - **Path-agnostic**: Accepts any valid PathBuf (user-selectable via Tauri)
//! - **Pure JSON schema**: No migrations required on user machines
//! - **WAL mode**: Write-Ahead Logging for better concurrency
//! - **Foreign keys**: Enabled for referential integrity
//! - **JSON operators**: Native SQLite JSON support
//!
//! For detailed schema specifications, see:
//! `/docs/architecture/business-logic/database-schema.md`

use crate::db::error::DatabaseError;
use libsql::{Builder, Database};
use std::path::PathBuf;
use std::sync::Arc;

/// Database service for managing libsql connection and schema
///
/// # Examples
///
/// ```no_run
/// use nodespace_core::db::DatabaseService;
/// use std::path::PathBuf;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let db_path = PathBuf::from("/path/to/nodespace.db");
///     let db_service = DatabaseService::new(db_path).await?;
///     Ok(())
/// }
/// ```
#[derive(Debug, Clone)]
pub struct DatabaseService {
    /// libsql database connection (wrapped in Arc for sharing)
    pub db: Arc<Database>,

    /// Path to the database file
    pub db_path: PathBuf,
}

impl DatabaseService {
    /// Create a new DatabaseService with the specified database path
    ///
    /// This will:
    /// 1. Ensure the parent directory exists (create if needed)
    /// 2. Open/create the database file
    /// 3. Initialize the schema (CREATE TABLE IF NOT EXISTS)
    /// 4. Enable SQLite features (WAL mode, foreign keys, JSON support)
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to the database file (can be in Dropbox, iCloud, etc.)
    ///
    /// # Errors
    ///
    /// Returns `DatabaseError` if:
    /// - Parent directory cannot be created
    /// - Database connection fails
    /// - Schema initialization fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let db_path = PathBuf::from("./data/nodespace.db");
    /// let db_service = DatabaseService::new(db_path).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(db_path: PathBuf) -> Result<Self, DatabaseError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::PermissionDenied {
                        DatabaseError::permission_denied(db_path.clone())
                    } else {
                        DatabaseError::DirectoryCreationFailed(e)
                    }
                })?;
            }
        }

        // Open database connection using Builder pattern
        let db = Builder::new_local(&db_path)
            .build()
            .await
            .map_err(|e| DatabaseError::connection_failed(db_path.clone(), e))?;

        let service = Self {
            db: Arc::new(db),
            db_path,
        };

        // Initialize schema
        service.initialize_schema().await?;

        Ok(service)
    }

    /// Execute a PRAGMA statement
    ///
    /// PRAGMA statements return rows, so we must use query() instead of execute().
    /// This helper method encapsulates that pattern for cleaner code.
    async fn execute_pragma(
        &self,
        conn: &libsql::Connection,
        pragma: &str,
    ) -> Result<(), DatabaseError> {
        let mut stmt = conn.prepare(pragma).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to execute '{}': {}", pragma, e))
        })?;
        let _ = stmt.query(()).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to execute '{}': {}", pragma, e))
        })?;
        Ok(())
    }

    /// Initialize database schema and configuration
    ///
    /// Creates tables and indexes using CREATE TABLE IF NOT EXISTS,
    /// ensuring idempotent initialization (safe to call multiple times).
    ///
    /// # Schema
    ///
    /// - `nodes` table: Universal node storage with Pure JSON properties
    /// - `node_mentions` table: Many-to-many mention relationships
    /// - Core indexes: type, parent, root, modified, content
    ///
    /// # SQLite Configuration
    ///
    /// - WAL mode: Write-Ahead Logging for better concurrency
    /// - Foreign keys: Enabled for referential integrity
    /// - JSON support: Native SQLite JSON operators enabled
    async fn initialize_schema(&self) -> Result<(), DatabaseError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DatabaseError::initialization_failed(e.to_string()))?;

        // Enable WAL mode for better concurrency
        self.execute_pragma(&conn, "PRAGMA journal_mode = WAL")
            .await?;

        // Enable foreign key constraints
        self.execute_pragma(&conn, "PRAGMA foreign_keys = ON")
            .await?;

        // Create nodes table (Pure JSON schema)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS nodes (
                id TEXT PRIMARY KEY,
                node_type TEXT NOT NULL,
                content TEXT NOT NULL,
                parent_id TEXT,
                root_id TEXT,
                before_sibling_id TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                modified_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                properties JSON NOT NULL DEFAULT '{}',
                embedding_vector BLOB,
                FOREIGN KEY (parent_id) REFERENCES nodes(id) ON DELETE CASCADE,
                FOREIGN KEY (root_id) REFERENCES nodes(id)
            )",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to create nodes table: {}", e))
        })?;

        // Create node_mentions table (bidirectional mention graph)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS node_mentions (
                node_id TEXT NOT NULL,
                mentions_node_id TEXT NOT NULL,
                PRIMARY KEY (node_id, mentions_node_id),
                FOREIGN KEY (node_id) REFERENCES nodes(id) ON DELETE CASCADE,
                FOREIGN KEY (mentions_node_id) REFERENCES nodes(id) ON DELETE CASCADE
            )",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to create node_mentions table: {}", e))
        })?;

        // Create core indexes for nodes table
        self.create_core_indexes(&conn).await?;

        Ok(())
    }

    /// Create core indexes for the nodes table
    ///
    /// These indexes are essential for query performance and never change
    /// (no ALTER TABLE required on user machines).
    async fn create_core_indexes(&self, conn: &libsql::Connection) -> Result<(), DatabaseError> {
        // Index on node_type (most common filter)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_type ON nodes(node_type)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to create index 'idx_nodes_type': {}", e))
        })?;

        // Index on parent_id (hierarchy queries)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_parent ON nodes(parent_id)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create index 'idx_nodes_parent': {}",
                e
            ))
        })?;

        // Index on root_id (bulk fetch by document)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_root ON nodes(root_id)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to create index 'idx_nodes_root': {}", e))
        })?;

        // Index on modified_at (temporal queries)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_modified ON nodes(modified_at)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create index 'idx_nodes_modified': {}",
                e
            ))
        })?;

        // Index on content (text search)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_content ON nodes(content)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create index 'idx_nodes_content': {}",
                e
            ))
        })?;

        // Indexes for node_mentions (bidirectional queries)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_mentions_source ON node_mentions(node_id)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create index 'idx_mentions_source': {}",
                e
            ))
        })?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_mentions_target ON node_mentions(mentions_node_id)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create index 'idx_mentions_target': {}",
                e
            ))
        })?;

        Ok(())
    }

    /// Get a connection to the database
    ///
    /// Returns a new connection that can be used for queries.
    /// Multiple connections can be used concurrently thanks to WAL mode.
    pub fn connect(&self) -> Result<libsql::Connection, DatabaseError> {
        self.db.connect().map_err(DatabaseError::LibsqlError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_database_creation() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db_service = DatabaseService::new(db_path.clone()).await.unwrap();

        assert_eq!(db_service.db_path, db_path);
        assert!(db_path.exists());
    }

    #[tokio::test]
    async fn test_schema_initialization() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db_service = DatabaseService::new(db_path).await.unwrap();
        let conn = db_service.connect().unwrap();

        // Verify nodes table exists
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='nodes'")
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let table_name: String = row.get(0).unwrap();
        assert_eq!(table_name, "nodes");

        // Verify node_mentions table exists
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name='node_mentions'")
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let table_name: String = row.get(0).unwrap();
        assert_eq!(table_name, "node_mentions");
    }

    #[tokio::test]
    async fn test_indexes_created() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db_service = DatabaseService::new(db_path).await.unwrap();
        let conn = db_service.connect().unwrap();

        // Verify core indexes exist
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'idx_%'")
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();

        let mut index_names = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let name: String = row.get(0).unwrap();
            index_names.push(name);
        }

        // Check that all expected indexes exist
        assert!(index_names.contains(&"idx_nodes_type".to_string()));
        assert!(index_names.contains(&"idx_nodes_parent".to_string()));
        assert!(index_names.contains(&"idx_nodes_root".to_string()));
        assert!(index_names.contains(&"idx_nodes_modified".to_string()));
        assert!(index_names.contains(&"idx_nodes_content".to_string()));
        assert!(index_names.contains(&"idx_mentions_source".to_string()));
        assert!(index_names.contains(&"idx_mentions_target".to_string()));
    }

    #[tokio::test]
    async fn test_wal_mode_enabled() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db_service = DatabaseService::new(db_path).await.unwrap();
        let conn = db_service.connect().unwrap();

        // Verify WAL mode is enabled
        let mut stmt = conn.prepare("PRAGMA journal_mode").await.unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let mode: String = row.get(0).unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[tokio::test]
    async fn test_foreign_keys_enabled() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db_service = DatabaseService::new(db_path).await.unwrap();
        let conn = db_service.connect().unwrap();

        // Verify foreign keys are enabled
        let mut stmt = conn.prepare("PRAGMA foreign_keys").await.unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let enabled: i64 = row.get(0).unwrap();
        assert_eq!(enabled, 1);
    }

    #[tokio::test]
    async fn test_parent_directory_creation() {
        let temp_dir = TempDir::new().unwrap();
        let nested_path = temp_dir.path().join("nested").join("dirs").join("test.db");

        let _db_service = DatabaseService::new(nested_path.clone()).await.unwrap();

        assert!(nested_path.exists());
        assert!(nested_path.parent().unwrap().exists());
    }

    #[tokio::test]
    async fn test_idempotent_initialization() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create database twice
        let _db_service1 = DatabaseService::new(db_path.clone()).await.unwrap();
        let db_service2 = DatabaseService::new(db_path.clone()).await.unwrap();

        // Should succeed without errors
        let conn = db_service2.connect().unwrap();
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM sqlite_master WHERE type='table'")
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let count: i64 = row.get(0).unwrap();

        // Should have exactly 2 tables (nodes, node_mentions)
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_concurrent_connections() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db_service = DatabaseService::new(db_path).await.unwrap();

        // Create multiple connections
        let conn1 = db_service.connect().unwrap();
        let conn2 = db_service.connect().unwrap();

        // Both connections should work
        let mut stmt1 = conn1.prepare("SELECT 1").await.unwrap();
        let mut rows1 = stmt1.query(()).await.unwrap();
        let row1 = rows1.next().await.unwrap().unwrap();
        let val1: i64 = row1.get(0).unwrap();
        assert_eq!(val1, 1);

        let mut stmt2 = conn2.prepare("SELECT 2").await.unwrap();
        let mut rows2 = stmt2.query(()).await.unwrap();
        let row2 = rows2.next().await.unwrap().unwrap();
        let val2: i64 = row2.get(0).unwrap();
        assert_eq!(val2, 2);
    }
}
