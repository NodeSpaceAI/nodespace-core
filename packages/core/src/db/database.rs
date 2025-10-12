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
use crate::services::EMBEDDING_DIMENSION;
use libsql::{Builder, Database};
use serde_json::json;
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

    /// Drain pending database connections
    ///
    /// This method provides a grace period for in-flight database operations to complete
    /// before the DatabaseService is replaced. This is essential for the HTTP dev server
    /// test infrastructure where databases are rapidly swapped between test cases.
    ///
    /// # Implementation Note
    ///
    /// libsql doesn't expose an explicit connection drain API, so we use a small delay
    /// to let pending operations complete naturally. This is a pragmatic workaround for
    /// Issue #255 (database initialization race condition).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let old_db = DatabaseService::new(PathBuf::from("old.db")).await?;
    /// old_db.drain_connections().await?;
    /// let new_db = DatabaseService::new(PathBuf::from("new.db")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn drain_connections(&self) -> Result<(), DatabaseError> {
        // Add small delay to let pending operations complete
        // 100ms provides sufficient time for connection pool cleanup
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        Ok(())
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

        // Set busy timeout to 5 seconds (5000ms)
        // This makes SQLite wait up to 5s instead of failing immediately on lock
        self.execute_pragma(&conn, "PRAGMA busy_timeout = 5000")
            .await?;

        // Enable foreign key constraints
        self.execute_pragma(&conn, "PRAGMA foreign_keys = ON")
            .await?;

        // Create nodes table (Pure JSON schema)
        conn.execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS nodes (
                    id TEXT PRIMARY KEY,
                    node_type TEXT NOT NULL,
                    content TEXT NOT NULL,
                    parent_id TEXT,
                    container_node_id TEXT,
                    before_sibling_id TEXT,
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    modified_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                    properties JSON NOT NULL DEFAULT '{{}}',
                    -- 384-dimensional embedding vectors from BAAI/bge-small-en-v1.5 model (PR #198)
                    embedding_vector F32_BLOB({}),
                    embedding_stale BOOLEAN DEFAULT FALSE,
                    last_content_update DATETIME,
                    last_embedding_update DATETIME,
                    -- Parent deletion cascades to children (tree structure)
                    FOREIGN KEY (parent_id) REFERENCES nodes(id) ON DELETE CASCADE,
                    -- Container deletion cascades to all contained nodes
                    FOREIGN KEY (container_node_id) REFERENCES nodes(id) ON DELETE CASCADE,
                    -- Sibling deletion nulls the reference (maintain chain integrity)
                    FOREIGN KEY (before_sibling_id) REFERENCES nodes(id) ON DELETE SET NULL
                )",
                EMBEDDING_DIMENSION
            ),
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

        // Seed core schemas
        self.seed_core_schemas(&conn).await?;

        // Force WAL checkpoint to ensure schema is written to disk (Issue #255)
        // This prevents race conditions where rapid database swaps in tests
        // cause "no such table" errors due to WAL entries not being flushed
        self.execute_pragma(&conn, "PRAGMA wal_checkpoint(TRUNCATE)")
            .await?;

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

        // Index on container_node_id (bulk fetch by container)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_container ON nodes(container_node_id)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create index 'idx_nodes_container': {}",
                e
            ))
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

        // Index on mentions array in properties JSON (for mentioned_by queries)
        // This helps SQLite optimize queries that use JSON_EACH on properties.mentions
        // Note: SQLite doesn't directly index JSON arrays, but this index on the extracted
        // mentions field helps the query planner. For better performance at scale, consider
        // migrating to the node_mentions table.
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_properties_mentions ON nodes(json_extract(properties, '$.mentions'))",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create index 'idx_properties_mentions': {}",
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

        // Index on before_sibling_id (sibling ordering)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_before_sibling ON nodes(before_sibling_id)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create index 'idx_nodes_before_sibling': {}",
                e
            ))
        })?;

        // Index on created_at (temporal queries)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_created ON nodes(created_at)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create index 'idx_nodes_created': {}",
                e
            ))
        })?;

        // Vector index on embedding_vector (semantic search with DiskANN)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_embedding_vector
             ON nodes(libsql_vector_idx(embedding_vector))",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to create vector index 'idx_nodes_embedding_vector': {}",
                e
            ))
        })?;

        // Index on embedding_stale for efficient stale topic queries
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_nodes_stale ON nodes(embedding_stale, node_type)",
            (),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to create index 'idx_nodes_stale': {}", e))
        })?;

        Ok(())
    }

    /// Seed core schemas as nodes
    ///
    /// Creates schema definition nodes for core entity types (task, person, date, project, text).
    /// These schemas define the structure and indexed fields for each node type using the
    /// Schema-as-Node pattern.
    ///
    /// Schema nodes have:
    /// - `id` = type name (e.g., "task")
    /// - `node_type` = "schema"
    /// - `content` = human-readable name (e.g., "Task")
    /// - `properties` = JSON with `is_core: true` and `fields` array
    ///
    /// This is idempotent - uses INSERT OR IGNORE to safely handle repeated initialization.
    async fn seed_core_schemas(&self, conn: &libsql::Connection) -> Result<(), DatabaseError> {
        // Task schema
        let task_schema = json!({
            "is_core": true,
            "fields": [
                {"name": "status", "type": "text", "indexed": true},
                {"name": "assignee", "type": "person", "indexed": true},
                {"name": "due_date", "type": "date", "indexed": true},
                {"name": "description", "type": "text", "indexed": false}
            ]
        });

        conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            ("task", "schema", "Task", task_schema.to_string()),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to seed task schema: {}", e))
        })?;

        // Person schema
        let person_schema = json!({
            "is_core": true,
            "fields": [
                {"name": "name", "type": "text", "indexed": true},
                {"name": "email", "type": "text", "indexed": true}
            ]
        });

        conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            ("person", "schema", "Person", person_schema.to_string()),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to seed person schema: {}", e))
        })?;

        // Date schema
        let date_schema = json!({
            "is_core": true,
            "fields": [
                {"name": "date", "type": "date", "indexed": true}
            ]
        });

        conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            ("date", "schema", "Date", date_schema.to_string()),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to seed date schema: {}", e))
        })?;

        // Project schema
        let project_schema = json!({
            "is_core": true,
            "fields": [
                {"name": "name", "type": "text", "indexed": true},
                {"name": "status", "type": "text", "indexed": true}
            ]
        });

        conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            ("project", "schema", "Project", project_schema.to_string()),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to seed project schema: {}", e))
        })?;

        // Text schema (minimal - just content)
        let text_schema = json!({
            "is_core": true,
            "fields": []
        });

        conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            ("text", "schema", "Text", text_schema.to_string()),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to seed text schema: {}", e))
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

    /// Get a connection with busy timeout configured
    ///
    /// Sets a 5-second busy timeout so operations wait instead of failing
    /// immediately when the database is locked.
    pub async fn connect_with_timeout(&self) -> Result<libsql::Connection, DatabaseError> {
        let conn = self.connect()?;

        // Set busy timeout on this connection
        self.execute_pragma(&conn, "PRAGMA busy_timeout = 5000")
            .await?;

        Ok(conn)
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
        assert!(index_names.contains(&"idx_nodes_container".to_string()));
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

        // Should have 4 tables:
        // - nodes (main table)
        // - node_mentions (mention relationships)
        // - libsql_vector_index (created by vector index extension)
        // - libsql_vector_index_metadata (metadata for vector index)
        assert_eq!(
            count, 4,
            "Expected 4 tables including libsql vector index tables"
        );
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

    #[tokio::test]
    async fn test_core_schemas_seeded() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db_service = DatabaseService::new(db_path).await.unwrap();
        let conn = db_service.connect().unwrap();

        // Verify all 5 core schemas exist
        let mut stmt = conn
            .prepare(
                "SELECT id, node_type, content FROM nodes WHERE node_type = 'schema' ORDER BY id",
            )
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();

        let expected_schemas = vec![
            ("date", "Date"),
            ("person", "Person"),
            ("project", "Project"),
            ("task", "Task"),
            ("text", "Text"),
        ];

        for (expected_id, expected_content) in expected_schemas {
            let row = rows.next().await.unwrap().unwrap();
            let id: String = row.get(0).unwrap();
            let node_type: String = row.get(1).unwrap();
            let content: String = row.get(2).unwrap();

            assert_eq!(id, expected_id);
            assert_eq!(node_type, "schema");
            assert_eq!(content, expected_content);
        }

        // Verify no more rows
        assert!(rows.next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_schema_properties_structure() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let db_service = DatabaseService::new(db_path).await.unwrap();
        let conn = db_service.connect().unwrap();

        // Check task schema properties
        let mut stmt = conn
            .prepare("SELECT properties FROM nodes WHERE id = 'task'")
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let properties: String = row.get(0).unwrap();

        // Verify it's valid JSON with expected structure
        let json: serde_json::Value = serde_json::from_str(&properties).unwrap();
        assert_eq!(json["is_core"], true);
        assert!(json["fields"].is_array());

        let fields = json["fields"].as_array().unwrap();
        assert!(!fields.is_empty());

        // Verify ALL fields have required properties (not just the first)
        for field in fields {
            assert!(
                field["name"].is_string(),
                "Field missing 'name': {:?}",
                field
            );
            assert!(
                field["type"].is_string(),
                "Field missing 'type': {:?}",
                field
            );
            assert!(
                field["indexed"].is_boolean(),
                "Field missing 'indexed': {:?}",
                field
            );
        }
    }

    #[tokio::test]
    async fn test_schema_seeding_idempotency() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        // Create database twice
        let _db_service1 = DatabaseService::new(db_path.clone()).await.unwrap();
        let db_service2 = DatabaseService::new(db_path.clone()).await.unwrap();

        let conn = db_service2.connect().unwrap();

        // Should still have exactly 5 schema nodes
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM nodes WHERE node_type = 'schema'")
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let count: i64 = row.get(0).unwrap();

        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn test_stale_embedding_index_exists() {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db_service = DatabaseService::new(db_path).await.unwrap();

        let conn = db_service.connect().unwrap();

        // Query sqlite_master to verify the index was created
        let mut stmt = conn
            .prepare(
                "SELECT name, sql FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_nodes_stale'",
            )
            .await
            .unwrap();

        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap();

        assert!(row.is_some(), "Index idx_nodes_stale was not created");

        let row = row.unwrap();
        let index_name: String = row.get(0).unwrap();
        let index_sql: String = row.get(1).unwrap();

        assert_eq!(index_name, "idx_nodes_stale");
        assert!(
            index_sql.contains("embedding_stale"),
            "Index should include embedding_stale column"
        );
        assert!(
            index_sql.contains("node_type"),
            "Index should include node_type column"
        );
    }
}
