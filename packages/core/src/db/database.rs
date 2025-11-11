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
//! # Database Connection Patterns
//!
//! ## Async contexts (Tokio runtime)
//!
//! **ALWAYS use `connect_with_timeout()` in async functions** to avoid SQLite
//! thread-safety violations when the Tokio runtime moves futures between threads.
//!
//! The 5-second busy timeout allows concurrent operations to wait and retry
//! instead of failing immediately with `SQLITE_BUSY` errors.
//!
//! ```no_run
//! # use nodespace_core::db::DatabaseService;
//! # use std::path::PathBuf;
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let db_service = DatabaseService::new(PathBuf::from(":memory:")).await?;
//! // ✅ CORRECT: Use connect_with_timeout() in async functions
//! let conn = db_service.connect_with_timeout().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Synchronous contexts
//!
//! Use `connect()` only in single-threaded, synchronous contexts where the
//! connection will not be used across await points.
//!
//! **Note**: Most code in NodeSpace is async, so `connect_with_timeout()` should
//! be your default choice.
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

/// Parameters for node insertion (avoids too-many-arguments lint)
pub struct DbCreateNodeParams<'a> {
    pub id: &'a str,
    pub node_type: &'a str,
    pub content: &'a str,
    pub parent_id: Option<&'a str>,
    pub container_node_id: Option<&'a str>,
    pub before_sibling_id: Option<&'a str>,
    pub properties: &'a str,
    pub embedding_vector: Option<&'a [u8]>,
}

/// Parameters for node update (avoids too-many-arguments lint)
pub struct DbUpdateNodeParams<'a> {
    pub id: &'a str,
    pub node_type: &'a str,
    pub content: &'a str,
    pub parent_id: Option<&'a str>,
    pub container_node_id: Option<&'a str>,
    pub before_sibling_id: Option<&'a str>,
    pub properties: &'a str,
    pub embedding_vector: Option<&'a [u8]>,
    pub content_changed: bool,
    pub is_container: bool,
    pub old_container_id: Option<&'a str>,
    pub new_container_id: Option<&'a str>,
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
        // Check if database file already exists (before we open it)
        // This allows us to optimize the WAL checkpoint - only needed for new databases
        let is_new_database = !db_path.exists();

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

        // Initialize schema (only checkpoints if is_new_database = true)
        service.initialize_schema(is_new_database).await?;

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
    /// # Arguments
    ///
    /// * `is_new_database` - Whether this is a newly created database file.
    ///   If true, performs a WAL checkpoint to flush schema to disk (prevents
    ///   race conditions in tests). If false, skips checkpoint for performance.
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
    async fn initialize_schema(&self, is_new_database: bool) -> Result<(), DatabaseError> {
        // CRITICAL: Must use connect_with_timeout() in async functions to prevent
        // SQLite thread-safety violations when Tokio moves futures between threads.
        // See PR #405 for detailed explanation of this pattern.
        let conn = self.connect_with_timeout().await?;

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
                    version INTEGER NOT NULL DEFAULT 1,
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

        // Force WAL checkpoint only for newly created databases (Issue #255)
        // This prevents race conditions where rapid database swaps in tests
        // cause "no such table" errors due to WAL entries not being flushed.
        // For existing databases, skip checkpoint to avoid unnecessary overhead.
        if is_new_database {
            self.execute_pragma(&conn, "PRAGMA wal_checkpoint(TRUNCATE)")
                .await?;
        }

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
    /// Creates schema definition nodes for core entity types (task, person, date, project, text,
    /// header, code-block, quote-block, ordered-list).
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
            "version": 1,
            "description": "Task tracking schema",
            "fields": [
                {
                    "name": "status",
                    "type": "enum",
                    "protection": "core",
                    "core_values": ["OPEN", "IN_PROGRESS", "DONE"],
                    "user_values": ["BLOCKED"],
                    "indexed": true,
                    "required": true,
                    "extensible": true,
                    "default": "OPEN",
                    "description": "Status"
                },
                {
                    "name": "priority",
                    "type": "enum",
                    "protection": "user",
                    "core_values": ["LOW", "MEDIUM", "HIGH"],
                    "user_values": [],
                    "indexed": true,
                    "required": false,
                    "extensible": true,
                    "description": "Priority"
                },
                {
                    "name": "due_date",
                    "type": "date",
                    "protection": "user",
                    "indexed": true,
                    "required": false,
                    "description": "Due Date"
                },
                {
                    "name": "started_at",
                    "type": "date",
                    "protection": "user",
                    "indexed": false,
                    "required": false,
                    "description": "Started At"
                },
                {
                    "name": "completed_at",
                    "type": "date",
                    "protection": "user",
                    "indexed": false,
                    "required": false,
                    "description": "Completed At"
                },
                {
                    "name": "assignee",
                    "type": "text",
                    "protection": "user",
                    "indexed": true,
                    "required": false,
                    "description": "Assignee"
                }
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
            "version": 1,
            "description": "Person entity schema",
            "fields": [
                {
                    "name": "name",
                    "type": "text",
                    "protection": "core",
                    "indexed": true,
                    "required": false,
                    "description": "Person name"
                },
                {
                    "name": "email",
                    "type": "text",
                    "protection": "core",
                    "indexed": true,
                    "required": false,
                    "description": "Email address"
                }
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

        // Date schema (minimal - date value is stored in node ID)
        let date_schema = json!({
            "is_core": true,
            "version": 1,
            "description": "Date node schema",
            "fields": []
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
            "version": 1,
            "description": "Project management schema",
            "fields": [
                {
                    "name": "name",
                    "type": "text",
                    "protection": "core",
                    "indexed": true,
                    "required": false,
                    "description": "Project name"
                },
                {
                    "name": "status",
                    "type": "text",
                    "protection": "core",
                    "indexed": true,
                    "required": false,
                    "description": "Project status"
                }
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
            "version": 1,
            "description": "Plain text content",
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

        // Header schema (markdown headers h1-h6)
        // Markdown nodes don't have properties - content contains full markdown syntax
        let header_schema = json!({
            "is_core": true,
            "version": 1,
            "description": "Markdown header (h1-h6)",
            "fields": []
        });

        conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            ("header", "schema", "Header", header_schema.to_string()),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to seed header schema: {}", e))
        })?;

        // Code block schema (code blocks with syntax highlighting)
        // Markdown nodes don't have properties - content contains full markdown syntax
        let code_block_schema = json!({
            "is_core": true,
            "version": 1,
            "description": "Code block with syntax highlighting",
            "fields": []
        });

        conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            ("code-block", "schema", "Code Block", code_block_schema.to_string()),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to seed code-block schema: {}", e))
        })?;

        // Quote block schema (markdown block quotes)
        // Markdown nodes don't have properties - content contains full markdown syntax
        let quote_block_schema = json!({
            "is_core": true,
            "version": 1,
            "description": "Markdown block quote",
            "fields": []
        });

        conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            ("quote-block", "schema", "Quote Block", quote_block_schema.to_string()),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to seed quote-block schema: {}", e))
        })?;

        // Ordered list schema (numbered list items)
        // Markdown nodes don't have properties - content contains full markdown syntax
        let ordered_list_schema = json!({
            "is_core": true,
            "version": 1,
            "description": "Numbered list item",
            "fields": []
        });

        conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            ("ordered-list", "schema", "Ordered List", ordered_list_schema.to_string()),
        )
        .await
        .map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to seed ordered-list schema: {}", e))
        })?;

        Ok(())
    }

    /// Drain connections and checkpoint WAL before database swap (Issue #255)
    ///
    /// This method ensures proper synchronization when swapping databases:
    /// 1. Forces WAL checkpoint to flush all pending writes to disk
    /// 2. Adds a small delay to allow any in-flight operations to complete
    ///
    /// This prevents race conditions where operations using stale connections
    /// from the old database fail with "no such table" errors after swap.
    ///
    /// # Implementation Note
    ///
    /// libsql doesn't expose an explicit connection draining API, so we use
    /// WAL checkpoint + small delay as a pragmatic solution. The checkpoint
    /// ensures all writes are committed, and the 10ms delay provides a safety
    /// margin for in-flight operations.
    pub async fn drain_and_checkpoint(&self) -> Result<(), DatabaseError> {
        // Force WAL checkpoint to flush all pending writes
        // CRITICAL: Must use connect_with_timeout() in async functions to prevent
        // SQLite thread-safety violations when Tokio moves futures between threads.
        let conn = self.connect_with_timeout().await?;
        self.execute_pragma(&conn, "PRAGMA wal_checkpoint(TRUNCATE)")
            .await?;

        // Small safety margin for in-flight operations (10ms is sufficient)
        // This is much shorter than the 100ms we used before, but provides
        // deterministic synchronization via the checkpoint above
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        Ok(())
    }

    /// Get a synchronous connection to the database
    ///
    /// **⚠️ WARNING**: Only use this in synchronous, single-threaded contexts.
    /// In async functions or Tokio runtime contexts, use `connect_with_timeout()`
    /// instead to avoid SQLite thread-safety violations.
    ///
    /// Returns a new connection that can be used for queries.
    /// Multiple connections can be used concurrently thanks to WAL mode.
    ///
    /// # When to Use
    ///
    /// - Single-threaded synchronous code only
    /// - Connection won't be used across `.await` points
    /// - **Most NodeSpace code should use `connect_with_timeout()` instead**
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db_service = futures::executor::block_on(
    /// #     DatabaseService::new(PathBuf::from(":memory:"))
    /// # )?;
    /// // ⚠️ Only in synchronous contexts
    /// let conn = db_service.connect()?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn connect(&self) -> Result<libsql::Connection, DatabaseError> {
        self.db.connect().map_err(DatabaseError::LibsqlError)
    }

    /// Get an async connection with busy timeout configured
    ///
    /// **✅ RECOMMENDED**: Use this for all async functions and Tokio runtime contexts.
    /// This is the safe default for NodeSpace's async architecture.
    ///
    /// Sets a 5-second busy timeout so concurrent operations wait and retry
    /// instead of failing immediately when the database is locked. This prevents
    /// SQLite thread-safety violations when the Tokio runtime moves futures
    /// between threads at `.await` points.
    ///
    /// # Why This Matters
    ///
    /// SQLite connections have thread-affinity requirements. When Tokio's async
    /// runtime moves a future to a different thread (which happens at every
    /// `.await`), using a synchronous connection can cause "bad parameter or
    /// other API misuse" errors. The busy timeout ensures operations serialize
    /// gracefully instead of failing.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db_service = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// // ✅ CORRECT: Use in async functions
    /// let conn = db_service.connect_with_timeout().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect_with_timeout(&self) -> Result<libsql::Connection, DatabaseError> {
        // Note: This synchronous connect() call is safe here because it's just
        // creating the connection handle. The actual SQLite operations happen
        // later, and the busy timeout ensures they work correctly in async contexts.
        let conn = self.connect()?;

        // Set busy timeout on this connection
        self.execute_pragma(&conn, "PRAGMA busy_timeout = 5000")
            .await?;

        Ok(conn)
    }

    //
    // NODE STORE OPERATIONS (Phase 1: SQL Extraction)
    // These methods contain the SQL logic extracted from NodeService.
    // They are designed to be wrapped by the NodeStore trait implementation.
    //

    /// Insert a node into the database
    ///
    /// This is the core SQL logic for node creation, extracted from NodeService.
    /// Handles both container and non-container nodes with appropriate embedding stale flags.
    ///
    /// # Arguments
    ///
    /// * `params` - Node creation parameters (see DbCreateNodeParams)
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful
    ///
    /// # Notes
    ///
    /// - Container nodes (container_node_id = None) are marked with embedding_stale = TRUE
    /// - Non-container nodes are inserted without stale flag
    /// - Created_at and modified_at are set automatically by database
    pub async fn db_create_node(
        &self,
        params: DbCreateNodeParams<'_>,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        // Container nodes (container_node_id = None) need stale flag for embedding generation
        let is_container = params.container_node_id.is_none();

        if is_container {
            conn.execute(
                "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector, embedding_stale, last_content_update)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, TRUE, CURRENT_TIMESTAMP)",
                (
                    params.id,
                    params.node_type,
                    params.content,
                    params.parent_id,
                    params.container_node_id,
                    params.before_sibling_id,
                    params.properties,
                    params.embedding_vector,
                ),
            )
            .await
            .map_err(|e| DatabaseError::sql_execution(format!("Failed to insert node: {}", e)))?;
        } else {
            conn.execute(
                "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    params.id,
                    params.node_type,
                    params.content,
                    params.parent_id,
                    params.container_node_id,
                    params.before_sibling_id,
                    params.properties,
                    params.embedding_vector,
                ),
            )
            .await
            .map_err(|e| DatabaseError::sql_execution(format!("Failed to insert node: {}", e)))?;
        }

        Ok(())
    }

    /// Retrieve a single node by ID from the database
    ///
    /// This is the core SQL logic for fetching a node, extracted from NodeService.
    /// Returns the raw database row data without applying business logic transformations.
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to retrieve
    ///
    /// # Returns
    ///
    /// * `Ok(Some(row))` - Node found, returns the libsql Row
    /// * `Ok(None)` - Node not found in database
    /// * `Err(DatabaseError)` - Query execution failed
    ///
    /// # Notes
    ///
    /// - Returns raw Row from libsql (caller must convert to Node)
    /// - Does NOT handle virtual date nodes (NodeService handles that)
    /// - Does NOT populate mentions or apply migrations (NodeService handles that)
    pub async fn db_get_node(&self, id: &str) -> Result<Option<libsql::Row>, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version,
                        created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE id = ?",
            )
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare get_node query: {}", e))
            })?;

        let mut rows = stmt.query([id]).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to execute get_node query: {}", e))
        })?;

        rows.next()
            .await
            .map_err(|e| DatabaseError::sql_execution(e.to_string()))
    }

    /// Update a node in the database
    ///
    /// This is the core SQL logic for node updates, extracted from NodeService.
    /// Handles embedding stale marking for content changes and container moves.
    ///
    /// # Arguments
    ///
    /// * `params` - Node update parameters (see DbUpdateNodeParams)
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful
    ///
    /// # Notes
    ///
    /// - Updates modified_at automatically
    /// - Marks container nodes as stale when content changes
    /// - Marks parent containers as stale when child content changes
    /// - Handles container moves (marks both old and new containers as stale)
    /// - Does NOT validate node structure or handle mentions (NodeService handles that)
    pub async fn db_update_node(
        &self,
        params: DbUpdateNodeParams<'_>,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        // Main node update - mark as stale if content changed and is container
        if params.content_changed && params.is_container {
            conn.execute(
                "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ?, embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                (
                    params.node_type,
                    params.content,
                    params.parent_id,
                    params.container_node_id,
                    params.before_sibling_id,
                    params.properties,
                    params.embedding_vector,
                    params.id,
                ),
            )
            .await
            .map_err(|e| DatabaseError::sql_execution(format!("Failed to update node: {}", e)))?;
        } else {
            conn.execute(
                "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ? WHERE id = ?",
                (
                    params.node_type,
                    params.content,
                    params.parent_id,
                    params.container_node_id,
                    params.before_sibling_id,
                    params.properties,
                    params.embedding_vector,
                    params.id,
                ),
            )
            .await
            .map_err(|e| DatabaseError::sql_execution(format!("Failed to update node: {}", e)))?;
        }

        // If content changed in a child node, mark the parent container as stale
        if params.content_changed && !params.is_container {
            if let Some(container_id) = params.new_container_id {
                conn.execute(
                    "UPDATE nodes SET embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                    [container_id],
                )
                .await
                .map_err(|e| DatabaseError::sql_execution(format!("Failed to mark parent container as stale: {}", e)))?;
            }
        }

        // If container_node_id changed (node moved between containers), mark both old and new containers as stale
        if params.old_container_id != params.new_container_id {
            // Mark old container as stale (if it exists)
            if let Some(old_container_id) = params.old_container_id {
                conn.execute(
                    "UPDATE nodes SET embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                    [old_container_id],
                )
                .await
                .ok(); // Don't fail update if old container no longer exists
            }

            // Mark new container as stale (only if content didn't change - content_changed already handled it above)
            if !params.content_changed {
                if let Some(new_container_id) = params.new_container_id {
                    conn.execute(
                        "UPDATE nodes SET embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                        [new_container_id],
                    )
                    .await
                    .ok(); // Don't fail update if new container doesn't exist
                }
            }
        }

        Ok(())
    }

    /// Delete a node from the database
    ///
    /// This is the core SQL logic for node deletion, extracted from NodeService.
    /// Uses CASCADE to automatically delete children and related data.
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to delete
    ///
    /// # Returns
    ///
    /// Number of rows affected (0 = node didn't exist, >0 = node deleted)
    ///
    /// # Notes
    ///
    /// - DELETE CASCADE automatically removes children (parent_id foreign key)
    /// - DELETE CASCADE automatically removes mentions (node_mentions table)
    /// - Idempotent: deleting non-existent node returns 0 (success)
    /// - Does NOT handle business logic (NodeService handles that)
    pub async fn db_delete_node(&self, id: &str) -> Result<u64, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let rows_affected = conn
            .execute("DELETE FROM nodes WHERE id = ?", [id])
            .await
            .map_err(|e| DatabaseError::sql_execution(format!("Failed to delete node: {}", e)))?;

        Ok(rows_affected)
    }

    //
    // MENTION OPERATIONS (Phase 1: SQL Extraction)
    // These methods contain SQL logic for node mention relationships, extracted from NodeService.
    //

    /// Create a mention relationship between two nodes
    ///
    /// This is the core SQL logic for creating mentions, extracted from NodeService.
    /// Inserts a row into the node_mentions table.
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the node that contains the mention
    /// * `target_id` - ID of the node being mentioned
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful (idempotent - INSERT OR IGNORE)
    ///
    /// # Notes
    ///
    /// - Uses INSERT OR IGNORE for idempotency
    /// - Does NOT validate node existence or prevent self-references (NodeService handles that)
    /// - Cascade deletion handled by foreign key constraints
    pub async fn db_create_mention(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        conn.execute(
            "INSERT OR IGNORE INTO node_mentions (node_id, mentions_node_id)
             VALUES (?, ?)",
            (source_id, target_id),
        )
        .await
        .map_err(|e| DatabaseError::sql_execution(format!("Failed to create mention: {}", e)))?;

        Ok(())
    }

    /// Delete a mention relationship between two nodes
    ///
    /// This is the core SQL logic for deleting mentions, extracted from NodeService.
    /// Removes a row from the node_mentions table.
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the node that contains the mention
    /// * `target_id` - ID of the node being mentioned
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful (idempotent - succeeds even if mention doesn't exist)
    ///
    /// # Notes
    ///
    /// - Idempotent: deleting non-existent mention succeeds
    /// - Does NOT validate node existence (NodeService handles that)
    pub async fn db_delete_mention(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        conn.execute(
            "DELETE FROM node_mentions WHERE node_id = ? AND mentions_node_id = ?",
            (source_id, target_id),
        )
        .await
        .map_err(|e| DatabaseError::sql_execution(format!("Failed to delete mention: {}", e)))?;

        Ok(())
    }

    /// Get all nodes mentioned by a specific node (outgoing mentions)
    ///
    /// This is the core SQL logic for querying outgoing mentions, extracted from NodeService.
    /// Returns node IDs that are mentioned by the source node.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to get mentions for
    ///
    /// # Returns
    ///
    /// Vector of mentioned node IDs
    ///
    /// # Notes
    ///
    /// - Returns empty vector if node has no mentions
    /// - Does NOT validate node existence (NodeService handles that)
    pub async fn db_get_outgoing_mentions(
        &self,
        node_id: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare("SELECT mentions_node_id FROM node_mentions WHERE node_id = ?")
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare mentions query: {}", e))
            })?;

        let mut rows = stmt.query([node_id]).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to execute mentions query: {}", e))
        })?;

        let mut mentions = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::sql_execution(e.to_string()))?
        {
            let mentioned_id: String = row
                .get(0)
                .map_err(|e| DatabaseError::sql_execution(e.to_string()))?;
            mentions.push(mentioned_id);
        }

        Ok(mentions)
    }

    /// Get all nodes that mention a specific node (incoming mentions/backlinks)
    ///
    /// This is the core SQL logic for querying incoming mentions, extracted from NodeService.
    /// Returns node IDs that mention the target node.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to get backlinks for
    ///
    /// # Returns
    ///
    /// Vector of node IDs that mention this node
    ///
    /// # Notes
    ///
    /// - Returns empty vector if node has no backlinks
    /// - Does NOT validate node existence (NodeService handles that)
    pub async fn db_get_incoming_mentions(
        &self,
        node_id: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare("SELECT node_id FROM node_mentions WHERE mentions_node_id = ?")
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare mentioned_by query: {}", e))
            })?;

        let mut rows = stmt.query([node_id]).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to execute mentioned_by query: {}", e))
        })?;

        let mut mentioned_by = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::sql_execution(e.to_string()))?
        {
            let mentioning_id: String = row
                .get(0)
                .map_err(|e| DatabaseError::sql_execution(e.to_string()))?;
            mentioned_by.push(mentioning_id);
        }

        Ok(mentioned_by)
    }

    /// Get container nodes of nodes that mention the target node (container-level backlinks)
    ///
    /// This is the core SQL logic for container-level backlinks, extracted from NodeService.
    /// Resolves incoming mentions to their container nodes and deduplicates.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The target node ID
    ///
    /// # Returns
    ///
    /// Vector of container node IDs (deduplicated)
    ///
    /// # Notes
    ///
    /// - Task and ai-chat nodes use their own ID as container (they are self-contained)
    /// - Other nodes use their container_node_id (or their own ID if root)
    /// - Results are deduplicated via DISTINCT
    /// - Returns empty vector if no nodes mention the target
    pub async fn db_get_mentioning_containers(
        &self,
        node_id: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let query = "
            SELECT DISTINCT
                CASE
                    WHEN n.node_type IN ('task', 'ai-chat') THEN n.id
                    ELSE COALESCE(n.container_node_id, n.id)
                END as container_id
            FROM node_mentions nm
            JOIN nodes n ON nm.node_id = n.id
            WHERE nm.mentions_node_id = ?
        ";

        let mut stmt = conn.prepare(query).await.map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to prepare mentioning_containers query: {}",
                e
            ))
        })?;

        let mut rows = stmt.query([node_id]).await.map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to execute mentioning_containers query: {}",
                e
            ))
        })?;

        let mut containers = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::sql_execution(e.to_string()))?
        {
            let container_id: String = row
                .get(0)
                .map_err(|e| DatabaseError::sql_execution(e.to_string()))?;
            containers.push(container_id);
        }

        Ok(containers)
    }

    //
    // SCHEMA OPERATIONS (Phase 1: SQL Extraction)
    // These methods contain SQL logic for schema queries, extracted from NodeService.
    //

    /// Get schema definition for a node type
    ///
    /// This is the core SQL logic for schema retrieval, extracted from NodeService.
    /// Returns the JSON properties field from the schema node.
    ///
    /// # Arguments
    ///
    /// * `node_type` - The node type to get schema for
    ///
    /// # Returns
    ///
    /// * `Ok(Some(json))` - Schema found and parsed
    /// * `Ok(None)` - No schema exists for this type
    /// * `Err(DatabaseError)` - Query failed or JSON parsing failed
    ///
    /// # Notes
    ///
    /// - Schema nodes have id=node_type and node_type='schema'
    /// - Returns parsed JSON from properties column
    /// - Does NOT apply migrations or defaults (NodeService handles that)
    pub async fn db_get_schema(
        &self,
        node_type: &str,
    ) -> Result<Option<serde_json::Value>, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare("SELECT properties FROM nodes WHERE id = ? AND node_type = 'schema'")
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare schema query: {}", e))
            })?;

        let mut rows = stmt
            .query([node_type])
            .await
            .map_err(|e| DatabaseError::sql_execution(format!("Failed to get schema: {}", e)))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::sql_execution(e.to_string()))?
        {
            let properties_str: String = row.get(0).map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to get properties column: {}", e))
            })?;

            let schema: serde_json::Value = serde_json::from_str(&properties_str).map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to parse schema JSON: {}", e))
            })?;

            Ok(Some(schema))
        } else {
            Ok(None)
        }
    }

    //
    // QUERY OPERATIONS (Phase 1: SQL Extraction)
    // These methods contain SQL logic for complex queries, extracted from NodeService.
    //

    /// Search nodes by content substring (case-insensitive LIKE)
    ///
    /// This is the core SQL logic for content search, extracted from NodeService.
    /// Supports optional node_type filtering and additional container/task filter.
    ///
    /// # Arguments
    ///
    /// * `content_pattern` - SQL LIKE pattern (e.g., "%search%")
    /// * `node_type` - Optional node type to filter by
    /// * `container_task_filter` - Optional SQL filter clause for containers/tasks
    /// * `limit_clause` - SQL LIMIT clause (e.g., " LIMIT 10")
    ///
    /// # Returns
    ///
    /// Rows iterator from the database query
    ///
    /// # Notes
    ///
    /// - Returns raw libsql::Rows iterator (NodeService processes rows)
    /// - Uses LIKE operator for substring matching (not full-text search)
    /// - Does NOT apply migrations or populate mentions (NodeService handles that)
    pub async fn db_search_nodes_by_content(
        &self,
        content_pattern: &str,
        node_type: Option<&str>,
        container_task_filter: &str,
        limit_clause: &str,
    ) -> Result<libsql::Rows, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        if let Some(node_type) = node_type {
            // Filter by both content and node_type
            let sql = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE content LIKE ? AND node_type = ?{}{}",
                container_task_filter, limit_clause
            );

            let mut stmt = conn.prepare(&sql).await.map_err(|e| {
                DatabaseError::sql_execution(format!(
                    "Failed to prepare content_contains query: {}",
                    e
                ))
            })?;

            stmt.query([content_pattern, node_type]).await.map_err(|e| {
                DatabaseError::sql_execution(format!(
                    "Failed to execute content_contains query: {}",
                    e
                ))
            })
        } else {
            // Filter by content only
            let sql = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE content LIKE ?{}{}",
                container_task_filter, limit_clause
            );

            let mut stmt = conn.prepare(&sql).await.map_err(|e| {
                DatabaseError::sql_execution(format!(
                    "Failed to prepare content_contains query: {}",
                    e
                ))
            })?;

            stmt.query([content_pattern]).await.map_err(|e| {
                DatabaseError::sql_execution(format!(
                    "Failed to execute content_contains query: {}",
                    e
                ))
            })
        }
    }

    /// Query nodes with filtering by node_type, parent_id, or container_node_id
    ///
    /// This is the core SQL logic for NodeFilter queries, extracted from NodeService.
    /// Supports filtering by type, parent, or container with optional ordering and limits.
    ///
    /// # Arguments
    ///
    /// * `node_type` - Optional node type filter
    /// * `parent_id` - Optional parent ID filter
    /// * `container_node_id` - Optional container ID filter
    /// * `order_clause` - SQL ORDER BY clause (e.g., " ORDER BY created_at ASC")
    /// * `limit_clause` - SQL LIMIT clause (e.g., " LIMIT 10")
    ///
    /// # Returns
    ///
    /// Rows iterator from the database query
    ///
    /// # Notes
    ///
    /// - Returns raw libsql::Rows iterator (NodeService processes rows)
    /// - Filters are mutually exclusive (priority: type > parent > container > all)
    /// - Does NOT apply migrations or populate mentions (NodeService handles that)
    /// - Caller must consume rows iterator before connection is dropped
    pub async fn db_query_nodes(
        &self,
        node_type: Option<&str>,
        parent_id: Option<&str>,
        container_node_id: Option<&str>,
        order_clause: &str,
        limit_clause: &str,
    ) -> Result<libsql::Rows, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        if let Some(node_type) = node_type {
            // Query by node_type
            let query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector FROM nodes WHERE node_type = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare query: {}", e))
            })?;

            stmt.query([node_type]).await.map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to execute query: {}", e))
            })
        } else if let Some(parent_id) = parent_id {
            // Query by parent_id
            let query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector FROM nodes WHERE parent_id = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare query: {}", e))
            })?;

            stmt.query([parent_id]).await.map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to execute query: {}", e))
            })
        } else if let Some(container_node_id) = container_node_id {
            // Query by container_node_id
            let query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector FROM nodes WHERE container_node_id = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare query: {}", e))
            })?;

            stmt.query([container_node_id]).await.map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to execute query: {}", e))
            })
        } else {
            // Default: return all nodes (with optional ordering/limit)
            let query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector FROM nodes{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare query: {}", e))
            })?;

            stmt.query(()).await.map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to execute query: {}", e))
            })
        }
    }

    /// Batch create multiple nodes in a transaction
    ///
    /// Inserts multiple nodes atomically within a single transaction.
    /// All nodes are inserted with database-generated timestamps.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of node creation parameters
    ///
    /// # Returns
    ///
    /// Vector of created node IDs in the same order as input
    ///
    /// # Errors
    ///
    /// Returns `DatabaseError` if:
    /// - Transaction cannot be started
    /// - Any INSERT fails (transaction is rolled back)
    /// - Transaction cannot be committed
    ///
    /// # Notes
    ///
    /// - Container nodes are marked with embedding_stale = TRUE
    /// - Non-container nodes are inserted without stale flag
    /// - All validations should be done BEFORE calling this method
    /// - Transaction is automatically rolled back on any error
    pub async fn db_batch_create_nodes(
        &self,
        nodes: Vec<DbCreateNodeParams<'_>>,
    ) -> Result<Vec<String>, DatabaseError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.connect_with_timeout().await?;

        // Begin transaction
        conn.execute("BEGIN TRANSACTION", ()).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to begin transaction: {}", e))
        })?;

        let mut created_ids = Vec::new();

        for params in nodes {
            let is_container = params.container_node_id.is_none();

            let result = if is_container {
                conn.execute(
                    "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector, embedding_stale, last_content_update)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, TRUE, CURRENT_TIMESTAMP)",
                    (
                        params.id,
                        params.node_type,
                        params.content,
                        params.parent_id,
                        params.container_node_id,
                        params.before_sibling_id,
                        params.properties,
                        params.embedding_vector,
                    ),
                )
                .await
            } else {
                conn.execute(
                    "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    (
                        params.id,
                        params.node_type,
                        params.content,
                        params.parent_id,
                        params.container_node_id,
                        params.before_sibling_id,
                        params.properties,
                        params.embedding_vector,
                    ),
                )
                .await
            };

            if let Err(e) = result {
                // Rollback on error
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(DatabaseError::sql_execution(format!(
                    "Failed to insert node {}: {}",
                    params.id, e
                )));
            }

            created_ids.push(params.id.to_string());
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            DatabaseError::sql_execution(format!("Failed to commit transaction: {}", e))
        })?;

        Ok(created_ids)
    }

    /// Batch update multiple nodes in a transaction
    ///
    /// Updates multiple nodes atomically within a single transaction.
    /// Each update requires first fetching the existing node to support
    /// partial updates.
    ///
    /// # Arguments
    ///
    /// * `updates` - Vector of (node_id, update_params) tuples
    ///
    /// # Returns
    ///
    /// `Ok(())` if all updates succeed
    ///
    /// # Errors
    ///
    /// Returns `DatabaseError` if:
    /// - Transaction cannot be started
    /// - Any node is not found (transaction is rolled back)
    /// - Any UPDATE fails (transaction is rolled back)
    /// - Transaction cannot be committed
    ///
    /// # Notes
    ///
    /// - All validations should be done BEFORE calling this method
    /// - Each update fetches existing node via SELECT within transaction
    /// - Database auto-updates modified_at timestamp
    /// - Transaction is automatically rolled back on any error
    pub async fn db_batch_update_nodes(
        &self,
        updates: Vec<(&str, DbUpdateNodeParams<'_>)>,
    ) -> Result<(), DatabaseError> {
        if updates.is_empty() {
            return Ok(());
        }

        let conn = self.connect_with_timeout().await?;

        // Begin transaction
        conn.execute("BEGIN TRANSACTION", ()).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to begin transaction: {}", e))
        })?;

        for (id, params) in updates {
            // Query node directly within transaction
            let mut stmt = conn
                .prepare("SELECT id FROM nodes WHERE id = ?")
                .await
                .map_err(|e| {
                    DatabaseError::sql_execution(format!("Failed to prepare query: {}", e))
                })?;

            let mut rows = stmt.query([id]).await.map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to execute query: {}", e))
            })?;

            // Verify node exists
            if rows
                .next()
                .await
                .map_err(|e| DatabaseError::sql_execution(e.to_string()))?
                .is_none()
            {
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(DatabaseError::sql_execution(format!(
                    "Node not found: {}",
                    id
                )));
            }

            // Execute update in transaction
            let result = conn
                .execute(
                    "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ? WHERE id = ?",
                    (
                        params.node_type,
                        params.content,
                        params.parent_id,
                        params.container_node_id,
                        params.before_sibling_id,
                        params.properties,
                        params.embedding_vector,
                        id,
                    ),
                )
                .await;

            if let Err(e) = result {
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(DatabaseError::sql_execution(format!(
                    "Failed to update node {}: {}",
                    id, e
                )));
            }
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            DatabaseError::sql_execution(format!("Failed to commit transaction: {}", e))
        })?;

        Ok(())
    }

    /// Get all children of a parent node
    ///
    /// This is the core SQL logic for fetching child nodes, extracted from NodeService.
    /// Returns nodes filtered by parent_id, ordered by created_at timestamp.
    ///
    /// # Arguments
    ///
    /// * `parent_id` - The ID of the parent node
    ///
    /// # Returns
    ///
    /// `libsql::Rows` iterator containing matching child nodes
    ///
    /// # Notes
    ///
    /// - Returns nodes in creation order (not sibling order)
    /// - NodeService handles sibling ordering via before_sibling_id
    /// - Does NOT validate parent exists (NodeService handles that)
    /// - Empty result is NOT an error (parent may have no children)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// // Fetch all children of a parent node
    /// let mut rows = db.db_get_children("parent-123").await?;
    /// while let Some(row) = rows.next().await? {
    ///     let id: String = row.get(0)?;
    ///     let node_type: String = row.get(1)?;
    ///     println!("Child: {} (type: {})", id, node_type);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn db_get_children(&self, parent_id: &str) -> Result<libsql::Rows, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version,
                        created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE parent_id = ? ORDER BY created_at ASC",
            )
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to prepare get_children query: {}", e))
            })?;

        stmt.query([parent_id]).await.map_err(|e| {
            DatabaseError::sql_execution(format!("Failed to execute get_children query: {}", e))
        })
    }

    /// Get all nodes belonging to a container node
    ///
    /// This is the core SQL logic for fetching nodes by container, extracted from NodeService.
    /// Enables efficient bulk loading of complete document trees in a single query.
    ///
    /// # Arguments
    ///
    /// * `container_node_id` - The ID of the container/origin node (e.g., date page ID)
    ///
    /// # Returns
    ///
    /// `libsql::Rows` iterator containing all nodes with matching container_node_id
    ///
    /// # Notes
    ///
    /// - Returns nodes in creation order (not hierarchical or sibling order)
    /// - NodeService handles tree reconstruction and sibling ordering
    /// - Does NOT validate container exists (NodeService handles that)
    /// - Empty result is NOT an error (container may be empty)
    /// - More efficient than recursive parent_id queries for full document loading
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// // Fetch all nodes belonging to a date page
    /// let mut rows = db.db_get_nodes_by_container("2025-10-05").await?;
    /// let mut count = 0;
    /// while let Some(_row) = rows.next().await? {
    ///     count += 1;
    /// }
    /// println!("Found {} nodes in container", count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn db_get_nodes_by_container(
        &self,
        container_node_id: &str,
    ) -> Result<libsql::Rows, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version,
                        created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE container_node_id = ?",
            )
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!(
                    "Failed to prepare get_nodes_by_container query: {}",
                    e
                ))
            })?;

        stmt.query([container_node_id]).await.map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to execute get_nodes_by_container query: {}",
                e
            ))
        })
    }

    /// Update a node's parent_id (move operation)
    ///
    /// This is the core SQL logic for moving nodes in the hierarchy, extracted from NodeService.
    /// Updates the parent_id field to change a node's position in the tree.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The ID of the node to move
    /// * `new_parent_id` - The new parent node ID (None = make root node)
    ///
    /// # Returns
    ///
    /// Number of rows affected (0 = node didn't exist, 1 = moved successfully)
    ///
    /// # Notes
    ///
    /// - Updates modified_at timestamp automatically
    /// - Does NOT validate node or parent exist (NodeService handles that)
    /// - Does NOT check for circular references (NodeService handles that)
    /// - Does NOT update container_node_id (NodeService handles that separately)
    /// - Idempotent: moving to current parent is allowed (no-op)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// // Move node under new parent
    /// let rows_affected = db.db_move_node("child-123", Some("parent-456")).await?;
    /// assert_eq!(rows_affected, 1);
    ///
    /// // Make node a root (remove parent)
    /// let rows_affected = db.db_move_node("child-123", None).await?;
    /// assert_eq!(rows_affected, 1);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn db_move_node(
        &self,
        node_id: &str,
        new_parent_id: Option<&str>,
    ) -> Result<u64, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let rows_affected = conn
            .execute(
                "UPDATE nodes SET parent_id = ?, modified_at = CURRENT_TIMESTAMP WHERE id = ?",
                (new_parent_id, node_id),
            )
            .await
            .map_err(|e| DatabaseError::sql_execution(format!("Failed to move node: {}", e)))?;

        Ok(rows_affected)
    }

    /// Update a node's before_sibling_id (reorder operation)
    ///
    /// This is the core SQL logic for reordering siblings, extracted from NodeService.
    /// Updates the before_sibling_id field to change a node's position in its sibling list.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The ID of the node to reorder
    /// * `before_sibling_id` - The sibling to position after (None = move to end)
    ///
    /// # Returns
    ///
    /// Number of rows affected (0 = node didn't exist, 1 = reordered successfully)
    ///
    /// # Notes
    ///
    /// - Updates modified_at timestamp automatically
    /// - Does NOT validate node or sibling exist (NodeService handles that)
    /// - Does NOT validate siblings share same parent (NodeService handles that)
    /// - Idempotent: setting same before_sibling_id is allowed (no-op)
    /// - Nodes form a linked list via before_sibling_id:
    ///   - before_sibling_id = None means first in list
    ///   - before_sibling_id = Some(id) means comes after that node
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// // Position node after sibling-123
    /// let rows_affected = db.db_reorder_node("node-456", Some("sibling-123")).await?;
    /// assert_eq!(rows_affected, 1);
    ///
    /// // Move node to front of sibling list
    /// let rows_affected = db.db_reorder_node("node-456", None).await?;
    /// assert_eq!(rows_affected, 1);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn db_reorder_node(
        &self,
        node_id: &str,
        before_sibling_id: Option<&str>,
    ) -> Result<u64, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let rows_affected = conn
            .execute(
                "UPDATE nodes SET before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP WHERE id = ?",
                (before_sibling_id, node_id),
            )
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!("Failed to reorder node: {}", e))
            })?;

        Ok(rows_affected)
    }
    //
    // SCHEMA OPERATIONS (Phase 1: SQL Extraction - Additional)
    // These methods contain SQL logic for schema updates, extracted for SurrealDB migration.
    //

    /// Insert or update a schema definition for a node type
    ///
    /// This is the core SQL logic for schema persistence, extracted for the SurrealDB migration.
    /// Stores schema definitions as nodes with node_type='schema' and id=node_type name.
    ///
    /// # Arguments
    ///
    /// * `node_type` - The node type this schema defines (e.g., "task", "person")
    /// * `schema_name` - Human-readable name for the schema (stored in content field)
    /// * `properties` - JSON schema definition with fields, validation rules, etc.
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful (idempotent - INSERT OR REPLACE)
    ///
    /// # Notes
    ///
    /// - Uses INSERT OR REPLACE for idempotency
    /// - Schema nodes have id=node_type, node_type='schema', content=schema_name
    /// - properties field contains the actual schema JSON
    /// - Automatically sets created_at and modified_at timestamps
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db_service = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// let schema = json!({
    ///     "is_core": false,
    ///     "version": 1,
    ///     "fields": [{"name": "priority", "type": "enum"}]
    /// });
    /// db_service.db_update_schema("custom-type", "Custom Type", &schema.to_string()).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn db_update_schema(
        &self,
        node_type: &str,
        schema_name: &str,
        properties: &str,
    ) -> Result<(), DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        conn.execute(
            "INSERT OR REPLACE INTO nodes (id, node_type, content, properties, created_at, modified_at)
             VALUES (?, 'schema', ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
            (node_type, schema_name, properties),
        )
        .await
        .map_err(|e| DatabaseError::sql_execution(format!("Failed to update schema: {}", e)))?;

        Ok(())
    }

    //
    // EMBEDDING OPERATIONS (Phase 1: SQL Extraction)
    // These methods contain SQL logic for embedding generation and search, extracted for SurrealDB migration.
    //

    /// Get nodes that need embeddings (embedding_vector IS NULL or embedding_stale = TRUE)
    ///
    /// This is the core SQL logic for querying stale embeddings, extracted for the SurrealDB migration.
    /// Returns nodes that either have no embedding or have stale embeddings that need regeneration.
    ///
    /// # Arguments
    ///
    /// * `limit` - Maximum number of nodes to return (for batch processing)
    ///
    /// # Returns
    ///
    /// Vector of node IDs that need embedding generation
    ///
    /// # Notes
    ///
    /// - Filters to container nodes only (container_node_id IS NULL)
    /// - Orders by last_content_update DESC (most recently edited first)
    /// - Returns raw node IDs (caller must fetch full nodes if needed)
    /// - Used by embedding batch processor for background sync
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db_service = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// let stale_ids = db_service.db_get_nodes_without_embeddings(10).await?;
    /// for node_id in stale_ids {
    ///     println!("Node {} needs embedding", node_id);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn db_get_nodes_without_embeddings(
        &self,
        limit: usize,
    ) -> Result<Vec<String>, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare(
                "SELECT id FROM nodes
                 WHERE container_node_id IS NULL
                   AND (embedding_vector IS NULL OR embedding_stale = TRUE)
                 ORDER BY last_content_update DESC
                 LIMIT ?",
            )
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!(
                    "Failed to prepare get_nodes_without_embeddings query: {}",
                    e
                ))
            })?;

        let mut rows = stmt.query([limit as i64]).await.map_err(|e| {
            DatabaseError::sql_execution(format!(
                "Failed to execute get_nodes_without_embeddings query: {}",
                e
            ))
        })?;

        let mut node_ids = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DatabaseError::sql_execution(e.to_string()))?
        {
            let id: String = row
                .get(0)
                .map_err(|e| DatabaseError::sql_execution(format!("Failed to get id: {}", e)))?;
            node_ids.push(id);
        }

        Ok(node_ids)
    }

    /// Update embedding vector for a node and mark as fresh (embedding_stale = FALSE)
    ///
    /// This is the core SQL logic for storing embeddings, extracted for the SurrealDB migration.
    /// Updates the embedding_vector and clears the stale flag.
    ///
    /// # Arguments
    ///
    /// * `node_id` - ID of the node to update
    /// * `embedding_blob` - Raw embedding vector as F32_BLOB (384 dimensions for bge-small-en-v1.5)
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful
    ///
    /// # Notes
    ///
    /// - Sets embedding_stale = FALSE to mark embedding as fresh
    /// - Updates last_embedding_update timestamp
    /// - Does NOT update modified_at (embeddings are metadata, not content changes)
    /// - Expects caller to generate embedding_blob using EmbeddingService::to_blob()
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db_service = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// // Generate embedding blob (normally from EmbeddingService)
    /// let embedding_blob = vec![0u8; 384 * 4]; // 384 f32 values = 1536 bytes
    /// db_service.db_update_embedding("node-id", &embedding_blob).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn db_update_embedding(
        &self,
        node_id: &str,
        embedding_blob: &[u8],
    ) -> Result<(), DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        conn.execute(
            "UPDATE nodes
             SET embedding_vector = ?,
                 embedding_stale = FALSE,
                 last_embedding_update = CURRENT_TIMESTAMP
             WHERE id = ?",
            (embedding_blob, node_id),
        )
        .await
        .map_err(|e| DatabaseError::sql_execution(format!("Failed to update embedding: {}", e)))?;

        Ok(())
    }

    /// Search nodes by embedding vector similarity using Turso's vector_distance_cosine
    ///
    /// This is the core SQL logic for semantic search, extracted for the SurrealDB migration.
    /// Uses cosine distance for similarity (lower = more similar).
    ///
    /// # Arguments
    ///
    /// * `query_blob` - Query embedding vector as F32_BLOB (384 dimensions)
    /// * `threshold` - Maximum distance threshold (0.0-2.0, lower = more similar)
    /// * `limit` - Maximum number of results to return
    ///
    /// # Returns
    ///
    /// Rows iterator with node data and distance scores
    ///
    /// # Notes
    ///
    /// - Uses vector_distance_cosine() for exact similarity calculation
    /// - Filters to container nodes only (container_node_id IS NULL)
    /// - Excludes nodes without embeddings (embedding_vector IS NOT NULL)
    /// - Orders by distance ASC (most similar first)
    /// - Returns raw libsql::Rows (caller must convert to Node structs)
    /// - For approximate search with indexes, use vector_top_k() instead
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db_service = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// let query_blob = vec![0u8; 384 * 4]; // Query embedding
    /// let mut rows = db_service.db_search_by_embedding(&query_blob, 0.7, 20).await?;
    /// while let Some(row) = rows.next().await? {
    ///     // Process row...
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn db_search_by_embedding(
        &self,
        query_blob: &[u8],
        threshold: f32,
        limit: usize,
    ) -> Result<libsql::Rows, DatabaseError> {
        let conn = self.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare(
                "SELECT
                    id, node_type, content, parent_id, container_node_id,
                    before_sibling_id, version, created_at, modified_at, properties,
                    embedding_vector,
                    vector_distance_cosine(embedding_vector, vector(?)) as distance
                 FROM nodes
                 WHERE container_node_id IS NULL
                   AND embedding_vector IS NOT NULL
                   AND vector_distance_cosine(embedding_vector, vector(?)) < ?
                 ORDER BY distance ASC
                 LIMIT ?",
            )
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!(
                    "Failed to prepare search_by_embedding query: {}",
                    e
                ))
            })?;

        stmt.query((query_blob, query_blob, threshold, limit as i64))
            .await
            .map_err(|e| {
                DatabaseError::sql_execution(format!(
                    "Failed to execute search_by_embedding query: {}",
                    e
                ))
            })
    }

    /// Close database connections gracefully
    ///
    /// This is the core SQL logic for connection cleanup, extracted for the SurrealDB migration.
    /// Ensures WAL is checkpointed before closing to prevent data loss.
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful
    ///
    /// # Notes
    ///
    /// - Performs TRUNCATE checkpoint to flush all WAL entries to main database file
    /// - This is critical for preventing "no such table" errors after database swaps
    /// - Should be called before application shutdown or database path changes
    /// - libsql connections are automatically dropped, this ensures clean state
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db_service = DatabaseService::new(PathBuf::from(":memory:")).await?;
    /// // Before app shutdown or database swap
    /// db_service.db_close().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn db_close(&self) -> Result<(), DatabaseError> {
        // Checkpoint WAL to ensure all writes are flushed
        let conn = self.connect_with_timeout().await?;
        self.execute_pragma(&conn, "PRAGMA wal_checkpoint(TRUNCATE)")
            .await?;

        // Connection will be automatically dropped when it goes out of scope
        // libsql handles connection cleanup internally
        Ok(())
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

        // Verify all 9 core schemas exist (including markdown node types)
        let mut stmt = conn
            .prepare(
                "SELECT id, node_type, content FROM nodes WHERE node_type = 'schema' ORDER BY id",
            )
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();

        let expected_schemas = vec![
            ("code-block", "Code Block"),
            ("date", "Date"),
            ("header", "Header"),
            ("ordered-list", "Ordered List"),
            ("person", "Person"),
            ("project", "Project"),
            ("quote-block", "Quote Block"),
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

        // Should still have exactly 9 schema nodes (task, person, date, project, text, header, code-block, quote-block, ordered-list)
        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM nodes WHERE node_type = 'schema'")
            .await
            .unwrap();
        let mut rows = stmt.query(()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        let count: i64 = row.get(0).unwrap();

        assert_eq!(count, 9);
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
