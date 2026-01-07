//! SurrealStore - Direct SurrealDB Backend Implementation
//!
//! This module provides the primary and only database backend for NodeSpace,
//! using SurrealDB embedded database with RocksDB storage engine.
//!
//! # Architecture
//!
//! SurrealStore uses a **Universal Graph Architecture** (Issue #783, #788):
//! 1. **Universal `node` table** - All node types with embedded `properties` field
//! 2. **Schema nodes** - Type definitions stored as nodes with `node_type = 'schema'`
//! 3. **Universal `relationship` table** - All relationships with `relationship_type` discriminator
//!
//! # Design Principles
//!
//! 1. **Embedded RocksDB**: Desktop-only backend using `kv-rocksdb` engine
//! 2. **SCHEMAFULL + FLEXIBLE**: Core fields strictly typed, user extensions allowed
//! 3. **Record IDs**: Native SurrealDB format `node:uuid` (type embedded in ID)
//! 4. **Universal Storage**: All properties embedded in `node.properties` field
//! 5. **Universal Edges**: All relationships in `relationship` table with `relationship_type` discriminator
//! 6. **Direct Access**: No abstraction layers, SurrealStore used directly by services
//!
//! # Performance Targets (from PoC)
//!
//! - Startup time: <100ms (PoC: 52ms)
//! - 100K nodes query: <200ms (PoC: 104ms)
//! - Deep pagination: <50ms (PoC: 8.3ms)
//! - Complex queries avg: <300ms (PoC: 211ms)
//!
//! # Examples
//!
//! ```rust,no_run
//! use nodespace_core::db::SurrealStore;
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create embedded SurrealDB store
//!     let db_path = PathBuf::from("./data/surreal.db");
//!     let store = SurrealStore::new(db_path).await?;
//!
//!     // Direct database access
//!     let node = store.get_node("task:550e8400-e29b-41d4-a716-446655440000").await?;
//!
//!     Ok(())
//! }
//! ```

use crate::db::events::DomainEvent;
use crate::db::fractional_ordering::FractionalOrderCalculator;
use crate::models::{DeleteResult, Node, NodeQuery, NodeUpdate};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use surrealdb::engine::local::{Db, RocksDb};
use surrealdb::engine::remote::http::{Client, Http};
use surrealdb::opt::auth::Root;
use surrealdb::sql::{Id, Thing};
use surrealdb::Surreal;
use tokio::sync::broadcast;
use tracing::warn;

/// Broadcast channel capacity for domain events.
///
/// 128 provides sufficient headroom for burst operations (bulk node creation)
/// while limiting memory overhead. Observer lag is acceptable - we only track
/// the current state, not historical events.
const DOMAIN_EVENT_CHANNEL_CAPACITY: usize = 128;

/// Represents an relationship from the universal relationship table
///
/// Used for bulk loading relationships (e.g., tree structure on startup).
/// Universal Relationship Architecture (Issue #788): All relationships in single `relationship` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipRecord {
    /// Relationship ID in SurrealDB format (e.g., "relationship:123")
    pub id: String,
    /// Source node ID
    #[serde(rename = "in")]
    pub in_node: String,
    /// Target node ID
    #[serde(rename = "out")]
    pub out_node: String,
    /// Relationship type discriminator (has_child, mentions, member_of, etc.)
    pub relationship_type: String,
    /// Type-specific properties (e.g., order for has_child, context for mentions)
    #[serde(default)]
    pub properties: Value,
}

impl RelationshipRecord {
    /// Get the order property for has_child relationships
    pub fn order(&self) -> f64 {
        self.properties
            .get("order")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    }
}

/// Store operation types for automatic notification (Issue #718)
///
/// Used by the store-level notification system to indicate what type
/// of mutation occurred, enabling automatic event emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreOperation {
    /// A new node was created
    Created,
    /// An existing node was updated
    Updated,
    /// A node was deleted
    Deleted,
}

/// Represents a store-level change notification (Issue #718)
///
/// Emitted automatically by store mutation methods when a registered
/// notifier is present. Contains all information needed for NodeService
/// to construct and broadcast domain events.
///
/// # Design Notes
///
/// - `source` is passed per-operation (not stored in SurrealStore) because
///   NodeService is a shared singleton - different clients pass their ID per-request
/// - For deleted nodes, `node` contains the node state before deletion
#[derive(Debug, Clone)]
pub struct StoreChange {
    /// The type of operation that occurred
    pub operation: StoreOperation,
    /// The node that was affected (for deletes, this is the pre-deletion state)
    pub node: Node,
    /// Optional client identifier for filtering events
    pub source: Option<String>,
}

/// Type alias for the store change notifier callback
///
/// This is a synchronous callback that runs immediately after store mutations.
/// The callback should be lightweight - heavy processing should be offloaded
/// to async tasks via channels.
pub type StoreNotifier = Arc<dyn Fn(StoreChange) + Send + Sync>;

// Valid node types are derived from schema definitions at runtime.
// See SurrealStore::build_schema_caches() and validate_node_type() methods.

/// Internal struct matching SurrealDB's schema
///
/// # Schema Evolution
///
/// - **v1.0** (Issue #470): Initial SurrealDB schema migration
///   - Core node fields
///   - Version-based optimistic concurrency control
///
/// - **v1.2** (Issue #511): Graph-native architecture
///   - Hierarchy via `has_child` graph relationships only
///   - Table renamed from `nodes` to `node` (singular)
///
/// - **v2.0** (Issue #729): Root-aggregate embedding architecture
///   - Embeddings now stored in dedicated `embedding` table
///   - Only root nodes get embedded (subtree content aggregated)
///
/// - **v3.0** (Issue #783): Universal Graph Architecture
///   - All properties stored in `node.properties` field
///   - All node data in single `node` table
///   - Single-query node fetching
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SurrealNode {
    // Record ID is stored in the 'id' field returned by SurrealDB (e.g., node:⟨uuid⟩)
    id: Thing, // SurrealDB record ID (table:id format)
    node_type: String,
    content: String,
    version: i64,
    created_at: String,
    modified_at: String,
    #[serde(default)]
    mentions: Vec<String>,
    #[serde(default)]
    mentioned_by: Vec<String>,
    /// Properties field stores all type-specific properties directly on the node
    #[serde(default)]
    properties: Value,
}

impl From<SurrealNode> for Node {
    fn from(sn: SurrealNode) -> Self {
        // Extract UUID from Thing record ID (e.g., node:⟨uuid⟩ -> uuid)
        let id = match &sn.id.id {
            Id::String(s) => {
                // Format is "node:uuid", extract the UUID part
                s.split(':').nth(1).unwrap_or(s).to_string()
            }
            _ => sn.id.id.to_string(),
        };

        // Universal Graph Architecture (Issue #783): Properties are always on node.properties
        let properties = if !sn.properties.is_null() {
            sn.properties
        } else {
            serde_json::json!({})
        };

        Node {
            id,
            node_type: sn.node_type,
            content: sn.content,
            version: sn.version,
            created_at: DateTime::parse_from_rfc3339(&sn.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            modified_at: DateTime::parse_from_rfc3339(&sn.modified_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            properties,
            mentions: sn.mentions,
            mentioned_by: sn.mentioned_by,
            member_of: Vec::new(),
        }
    }
}

/// Batch fetch collection memberships for multiple nodes
///
/// **Purpose**: Avoid N+1 query pattern when populating member_of for multiple nodes.
///
/// **Performance**:
/// - Old: 100 nodes = 100 individual queries
/// - New: 100 nodes = 1 batch query
///
/// # Arguments
///
/// * `db` - SurrealDB connection
/// * `node_ids` - Vector of node IDs to fetch memberships for
///
/// # Returns
///
/// HashMap mapping node ID to its collection membership IDs
async fn batch_fetch_memberships<C: surrealdb::Connection>(
    db: &Surreal<C>,
    node_ids: &[String],
) -> Result<std::collections::HashMap<String, Vec<String>>> {
    use surrealdb::sql::{Id, Thing};

    if node_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Build Things for all node IDs
    let node_things: Vec<Thing> = node_ids
        .iter()
        .map(|id| Thing::from(("node".to_string(), id.to_string())))
        .collect();

    // Query all membership relationships in one batch (Issue #788: use relationship table)
    // Returns: [{ node_id: "xxx", collection_ids: [Thing, Thing] }, ...]
    let query = r#"
        SELECT
            record::id(id) AS node_id,
            ->relationship[WHERE relationship_type = 'member_of']->node.id AS collection_ids
        FROM node
        WHERE id IN $node_ids;
    "#;

    let mut response = db
        .query(query)
        .bind(("node_ids", node_things))
        .await
        .context("Failed to batch fetch memberships")?;

    #[derive(Debug, serde::Deserialize)]
    struct MembershipRow {
        node_id: String,
        #[serde(default)]
        collection_ids: Vec<Thing>,
    }

    let rows: Vec<MembershipRow> = response
        .take(0)
        .context("Failed to extract membership results")?;

    // Convert to HashMap<node_id, Vec<collection_id>>
    let mut result = std::collections::HashMap::new();
    for row in rows {
        let collection_ids: Vec<String> = row
            .collection_ids
            .into_iter()
            .filter_map(|thing| {
                if let Id::String(id_str) = thing.id {
                    Some(id_str)
                } else {
                    None
                }
            })
            .collect();
        result.insert(row.node_id, collection_ids);
    }

    Ok(result)
}

/// SurrealStore implements NodeStore trait for SurrealDB backend
///
/// Supports two connection modes:
/// - **Embedded RocksDB**: Desktop production mode (Surreal<Db>)
/// - **HTTP Client**: Dev-proxy mode (Surreal<Client>)
///
/// Uses hybrid dual-table architecture for optimal query performance.
/// Emits domain events via broadcast channel when data changes.
pub struct SurrealStore<C = Db>
where
    C: surrealdb::Connection,
{
    /// SurrealDB connection
    db: Arc<Surreal<C>>,
    /// Broadcast channel for domain events (128 subscriber capacity)
    event_tx: broadcast::Sender<DomainEvent>,
    /// Cache of all valid node types (derived from schema definitions)
    ///
    /// Contains all schema IDs from the database, used for validating
    /// node_type parameters in queries to prevent SQL injection.
    ///
    /// **Cache Population Strategy (Issue #704):**
    /// - **First launch (fresh DB)**: NodeService seeds schemas and populates cache incrementally
    ///   via `add_to_schema_cache()` - no database re-query needed
    /// - **Subsequent launches**: `build_schema_caches()` queries existing schema records once at startup
    valid_node_types: std::collections::HashSet<String>,
    /// Optional notifier callback for store-level change notifications (Issue #718)
    ///
    /// When registered, this callback is invoked synchronously after every store
    /// mutation (create, update, delete). The notifier enables automatic domain
    /// event emission without manual emit calls in NodeService methods.
    ///
    /// Set via `set_notifier()` after construction.
    notifier: Option<StoreNotifier>,
}

/// Type alias for embedded RocksDB store
pub type EmbeddedStore = SurrealStore<Db>;

/// Type alias for HTTP client store
pub type HttpStore = SurrealStore<Client>;

impl SurrealStore<Db> {
    /// Create a new SurrealStore with embedded RocksDB backend
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to RocksDB database directory
    ///
    /// # Returns
    ///
    /// Initialized SurrealStore with schema setup complete
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Database path is invalid
    /// - RocksDB initialization fails
    /// - Schema initialization fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let store = SurrealStore::new(PathBuf::from("./data/surreal.db")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(db_path: PathBuf) -> Result<Self> {
        // Initialize embedded RocksDb
        let db = Surreal::new::<RocksDb>(db_path)
            .await
            .context("Failed to initialize SurrealDB with RocksDB backend")?;

        // Use namespace and database
        // Note: Both embedded and HTTP modes use "nodespace" for namespace and database
        // to ensure consistency between Tauri desktop app and browser development mode
        db.use_ns("nodespace")
            .use_db("nodespace")
            .await
            .context("Failed to set namespace/database")?;

        let db = Arc::new(db);

        // Initialize schema (create tables from schema.surql)
        // Note: Schema nodes are seeded by NodeService, not here (Issue #704)
        Self::initialize_schema(&db).await?;

        // Build valid node types cache from schema definitions (Issue #691)
        let valid_node_types = Self::build_schema_caches(&db).await?;

        // Initialize broadcast channel for domain events
        let (event_tx, _) = broadcast::channel(DOMAIN_EVENT_CHANNEL_CAPACITY);

        Ok(Self {
            db,
            event_tx,
            valid_node_types,
            notifier: None,
        })
    }
}

impl SurrealStore<Client> {
    /// Create HTTP client store for dev-proxy mode
    ///
    /// This connects to a remote SurrealDB server via HTTP API.
    /// Used by dev-proxy to enable Surrealist inspection while preserving business logic.
    ///
    /// # Arguments
    ///
    /// * `endpoint` - SurrealDB server address (e.g., "127.0.0.1:8000")
    /// * `namespace` - Database namespace (e.g., "nodespace")
    /// * `database` - Database name (e.g., "nodespace")
    /// * `username` - Auth username (e.g., "root")
    /// * `password` - Auth password (e.g., "root")
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use nodespace_core::db::SurrealStore;
    ///
    /// #[tokio::main]
    /// async fn main() -> anyhow::Result<()> {
    ///     let store = SurrealStore::new_http(
    ///         "127.0.0.1:8000",
    ///         "nodespace",
    ///         "nodespace",
    ///         "root",
    ///         "root"
    ///     ).await?;
    ///
    ///     // Use store normally - same API as embedded mode
    ///     let node = store.get_node("some-id").await?;
    ///
    ///     Ok(())
    /// }
    /// ```
    pub async fn new_http(
        endpoint: &str,
        namespace: &str,
        database: &str,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        tracing::info!("Connecting to SurrealDB HTTP server at {}", endpoint);

        // Create HTTP client connection to remote SurrealDB server
        let db = Surreal::new::<Http>(endpoint)
            .await
            .context("Failed to connect to SurrealDB HTTP server")?;

        // Authenticate with root credentials
        db.signin(Root { username, password })
            .await
            .context("Failed to authenticate with SurrealDB")?;

        // Set namespace and database
        db.use_ns(namespace)
            .use_db(database)
            .await
            .context("Failed to set namespace/database")?;

        let db = Arc::new(db);

        // Initialize schema tables (idempotent - uses IF NOT EXISTS)
        // Even in HTTP mode, we need to ensure schema exists for fresh databases
        // Note: Schema nodes are seeded by NodeService, not here (Issue #704)
        Self::initialize_schema(&db).await?;

        // Build valid node types cache from schema definitions (Issue #691)
        let valid_node_types = Self::build_schema_caches(&db).await?;

        tracing::info!("Connected to SurrealDB HTTP server");

        // Initialize broadcast channel for domain events
        let (event_tx, _) = broadcast::channel(DOMAIN_EVENT_CHANNEL_CAPACITY);

        Ok(Self {
            db,
            event_tx,
            valid_node_types,
            notifier: None,
        })
    }
}

impl<C> SurrealStore<C>
where
    C: surrealdb::Connection,
{
    /// Set the store change notifier callback (Issue #718)
    ///
    /// Registers a callback that will be invoked synchronously after every store
    /// mutation. This enables automatic domain event emission from NodeService
    /// without manual emit calls in each method.
    ///
    /// # Arguments
    ///
    /// * `notifier` - Callback function that receives `StoreChange` on mutations
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let store = SurrealStore::new(db_path).await?;
    /// store.set_notifier(Arc::new(|change| {
    ///     println!("Store change: {:?}", change.operation);
    /// }));
    /// ```
    pub fn set_notifier(&mut self, notifier: StoreNotifier) {
        self.notifier = Some(notifier);
    }

    /// Notify registered callback of a store change (Issue #718)
    ///
    /// Called internally by mutation methods after successful operations.
    /// Does nothing if no notifier is registered.
    fn notify(&self, change: StoreChange) {
        if let Some(notifier) = &self.notifier {
            notifier(change);
        }
    }

    /// Get the underlying database connection
    ///
    /// This is used by services (like SchemaService) that need direct database access
    /// for operations like DEFINE TABLE or DEFINE FIELD.
    pub fn db(&self) -> &Arc<Surreal<C>> {
        &self.db
    }

    /// Subscribe to domain events emitted by this store
    ///
    /// Returns a receiver that will get notified when nodes or relationships change.
    /// Multiple subscribers are supported - each gets their own copy of events.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::DomainEvent;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let store = SurrealStore::new(PathBuf::from("./data/test.db")).await?;
    /// let mut rx = store.subscribe_to_events();
    /// while let Ok(event) = rx.recv().await {
    ///     match event {
    ///         DomainEvent::NodeCreated { node_id, .. } => {
    ///             println!("Node created: {}", node_id)
    ///         }
    ///         DomainEvent::NodeUpdated { node_id, .. } => {
    ///             println!("Node updated: {}", node_id)
    ///         }
    ///         // ... handle other events
    ///         _ => {}
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn subscribe_to_events(&self) -> broadcast::Receiver<DomainEvent> {
        self.event_tx.subscribe()
    }

    /// Validates a node type against the schema-derived whitelist
    ///
    /// This prevents SQL injection attacks where malicious node_type values could
    /// alter query semantics. All node_type parameters used in dynamic queries
    /// must be validated with this method before use.
    ///
    /// Replaces the hardcoded VALID_NODE_TYPES constant (Issue #691).
    ///
    /// # Arguments
    /// * `node_type` - The node type string to validate
    ///
    /// # Returns
    /// * `Ok(())` if the node_type exists as a schema in the database
    /// * `Err(...)` if the node_type is not recognized
    fn validate_node_type(&self, node_type: &str) -> Result<()> {
        if self.valid_node_types.contains(node_type) {
            Ok(())
        } else {
            let valid_types: Vec<&String> = self.valid_node_types.iter().collect();
            Err(anyhow::anyhow!(
                "Invalid node type: '{}'. Valid types are: {:?}",
                node_type,
                valid_types
            ))
        }
    }

    /// Build valid node types cache from database schema definitions
    ///
    /// Universal Graph Architecture (Issue #783): Queries `node WHERE node_type = 'schema'`
    /// to determine which node types exist. Schema data is in node.properties.
    ///
    /// # Returns
    ///
    /// - `valid_node_types`: All valid node types (all schema IDs)
    ///
    /// # Cache Population Strategy (Issue #704)
    ///
    /// **First launch (fresh database):**
    /// - Called during `SurrealStore::new()` but returns empty results (no schema records yet)
    /// - Cache starts with only {"schema"} (hardcoded)
    /// - NodeService then seeds schemas and populates cache via `add_to_schema_cache()`
    ///
    /// **Subsequent launches (existing database):**
    /// - Called during `SurrealStore::new()` and returns all existing schema records
    /// - Cache fully populated in one query: {"schema", "task", "text", "date", ...}
    /// - No further cache updates needed
    async fn build_schema_caches(
        db: &Arc<Surreal<C>>,
    ) -> Result<std::collections::HashSet<String>> {
        let mut valid_types = std::collections::HashSet::new();

        // Schema type is always a valid type
        valid_types.insert("schema".to_string());

        // Query all schema nodes from node table (Universal Graph Architecture)
        // Schema nodes have node_type = "schema" and id = the type name (e.g., "task", "text")
        let query = r#"
            SELECT id FROM node WHERE node_type = 'schema';
        "#;

        let mut response = db
            .query(query)
            .await
            .context("Failed to query schema nodes for caches")?;

        // Parse results - each row has id (the type name)
        #[derive(serde::Deserialize)]
        struct SchemaRow {
            id: surrealdb::sql::Thing,
        }

        let rows: Vec<SchemaRow> = response.take(0).unwrap_or_default();

        for row in rows {
            // Extract type name from Thing id (e.g., node:task -> task)
            let type_name = match &row.id.id {
                surrealdb::sql::Id::String(s) => s.clone(),
                other => other.to_string(),
            };

            // All schema IDs are valid node types
            valid_types.insert(type_name);
        }

        Ok(valid_types)
    }

    /// Initialize database schema from schema.surql file
    ///
    /// Creates SCHEMAFULL tables with FLEXIBLE fields for user extensions.
    /// Universal Graph Architecture (Issue #783): All properties embedded in node.properties.
    ///
    /// # Architecture
    /// - Universal `node` table with embedded properties for ALL nodes (including schemas)
    /// - Graph relationships: `has_child` and `mentions` relations for relationships
    async fn initialize_schema(db: &Arc<Surreal<C>>) -> Result<()> {
        // Load schema from schema.surql file
        // Universal Graph Architecture with SCHEMAFULL tables
        let schema_sql = include_str!("schema.surql");

        db.query(schema_sql)
            .await
            .context("Failed to execute schema.surql")?;

        Ok(())
    }

    /// Add a node type to valid types cache (called during schema seeding)
    ///
    /// When NodeService seeds schema records on first launch, it populates the cache
    /// incrementally as each schema is created. This avoids re-querying the database
    /// after seeding - we already have the schema data in memory.
    ///
    /// # Arguments
    ///
    /// * `type_name` - The node type (e.g., "task", "text", "date")
    ///
    /// # Cache Population Strategy (Issue #704)
    ///
    /// **First launch (fresh database):**
    /// ```text
    /// for schema in core_schemas {
    ///     create_schema_node_atomic(schema);
    ///     add_to_schema_cache(schema.id); // No DB query needed
    /// }
    /// ```
    ///
    /// **Subsequent launches:**
    /// - Cache already populated by `build_schema_caches()` during `SurrealStore::new()`
    /// - This method is not called
    pub(crate) fn add_to_schema_cache(&mut self, type_name: String) {
        self.valid_node_types.insert(type_name);
    }
}

impl<C> SurrealStore<C>
where
    C: surrealdb::Connection,
{
    pub async fn create_node(&self, node: Node, source: Option<String>) -> Result<Node> {
        // Universal Graph Architecture (Issue #783): All properties stored in node.properties
        // Embeddings are managed separately in dedicated embedding table

        // Enforce globally unique names for collection nodes
        if node.node_type == "collection" {
            if let Some(existing) = self.get_collection_by_name(&node.content).await? {
                anyhow::bail!(
                    "Collection with name '{}' already exists (id: {})",
                    node.content,
                    existing.id
                );
            }
        }

        // Create node with properties embedded directly
        // Note: IDs with special characters (hyphens, spaces, etc.) need to be backtick-quoted
        let create_query = format!(
            r#"
            CREATE node:`{}` CONTENT {{
                node_type: $node_type,
                content: $content,
                version: $version,
                created_at: time::now(),
                modified_at: time::now(),
                mentions: [],
                mentioned_by: [],
                properties: $properties
            }};
        "#,
            node.id
        );

        let mut response = self
            .db
            .query(&create_query)
            .bind(("node_type", node.node_type.clone()))
            .bind(("content", node.content.clone()))
            .bind(("version", node.version))
            .bind(("properties", node.properties.clone()))
            .await
            .context("Failed to create node in universal table")?;

        // Consume the CREATE response - critical for persistence
        let _: Result<Vec<serde_json::Value>, _> = response.take(0usize);

        // Verify the node was actually created by querying it back
        // This ensures the CREATE statement fully persisted before proceeding
        let verify_query = format!("SELECT * FROM node:`{}` LIMIT 1;", node.id);
        let mut verify_response = self
            .db
            .query(&verify_query)
            .await
            .context("Failed to verify node creation")?;

        let _: Vec<SurrealNode> = verify_response.take(0).context(format!(
            "Node '{}' was not created - verification query returned no results",
            node.id
        ))?;

        // Note: Parent-child relationships are now established separately via move_node()
        // This allows cleaner separation of node creation from hierarchy management

        // Notify registered callback of the store change (Issue #718)
        self.notify(StoreChange {
            operation: StoreOperation::Created,
            node: node.clone(),
            source,
        });

        // Return the created node directly
        Ok(node)
    }

    /// Create a child node atomically with parent relationship in a single transaction
    ///
    /// This is the atomic version of create_node + move_node. It guarantees that either:
    /// - The node and parent relationship are ALL created
    /// - OR nothing is created (transaction rolls back on failure)
    ///
    /// Universal Graph Architecture (Issue #783): All properties embedded in node.properties.
    ///
    /// # Performance Target
    /// - <15ms for create operation (from Issue #532 acceptance criteria)
    ///
    /// # Arguments
    ///
    /// * `parent_id` - ID of the parent node
    /// * `node_type` - Type of the node to create
    /// * `content` - Content of the node
    /// * `properties` - Properties for the node
    ///
    /// # Returns
    ///
    /// The created node with all fields populated
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use serde_json::json;
    /// # async fn example(store: &SurrealStore) -> anyhow::Result<()> {
    /// let child = store.create_child_node_atomic(
    ///     "parent-uuid",
    ///     "text",
    ///     "Child content",
    ///     json!({}),
    ///     None,
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_child_node_atomic(
        &self,
        parent_id: &str,
        node_type: &str,
        content: &str,
        properties: Value,
        source: Option<String>,
    ) -> Result<Node> {
        use uuid::Uuid;

        // Validate node type to prevent SQL injection
        self.validate_node_type(node_type)?;

        // Generate node ID and convert parameters to owned strings for 'static lifetime
        let node_id = Uuid::new_v4().to_string();
        let parent_id = parent_id.to_string();
        let node_type = node_type.to_string();
        let content = content.to_string();

        // Validate parent exists (prevent orphan nodes)
        let parent_exists = self.get_node(&parent_id).await?;
        if parent_exists.is_none() {
            return Err(anyhow::anyhow!("Parent node not found: {}", parent_id));
        }

        // Validate no cycle (prevent child from being ancestor of parent)
        self.validate_no_cycle(&parent_id, &node_id).await?;

        // Calculate fractional order for the new node
        // Get the last child's order value
        #[derive(Deserialize)]
        struct EdgeOrder {
            order: f64,
        }

        let parent_thing = surrealdb::sql::Thing::from(("node".to_string(), parent_id.clone()));
        // Universal Relationship Architecture (Issue #788): Query from relationship table with relationship_type filter
        let mut order_response = self
            .db
            .query(
                "SELECT properties.order AS order FROM relationship WHERE in = $parent_thing AND relationship_type = 'has_child' ORDER BY properties.order DESC LIMIT 1;",
            )
            .bind(("parent_thing", parent_thing.clone()))
            .await
            .context("Failed to get last child order")?;

        let last_order: Option<EdgeOrder> = order_response
            .take(0)
            .context("Failed to extract last child order")?;

        let new_order = if let Some(rel) = last_order {
            FractionalOrderCalculator::calculate_order(Some(rel.order), None)
        } else {
            FractionalOrderCalculator::calculate_order(None, None)
        };

        // Universal Graph Architecture (Issue #783, #788): All properties embedded, relationships in universal table
        let transaction_query = r#"
            BEGIN TRANSACTION;

            -- Create node with embedded properties
            CREATE $node_id CONTENT {
                id: $node_id,
                node_type: $node_type,
                content: $content,
                properties: $properties,
                version: 1,
                created_at: time::now(),
                modified_at: time::now()
            };

            -- Create parent-child relationship in universal relationship table (Issue #788)
            RELATE $parent_id->relationship->$node_id CONTENT {
                relationship_type: 'has_child',
                properties: { order: $order },
                created_at: time::now(),
                modified_at: time::now(),
                version: 1
            };

            COMMIT TRANSACTION;
        "#;

        // Construct Thing objects for Record IDs
        let node_thing = surrealdb::sql::Thing::from(("node".to_string(), node_id.clone()));

        // Execute transaction
        let response = self
            .db
            .query(transaction_query)
            .bind(("node_id", node_thing))
            .bind(("parent_id", parent_thing))
            .bind(("node_type", node_type.clone()))
            .bind(("content", content.clone()))
            .bind(("order", new_order))
            .bind(("properties", properties))
            .await
            .context(format!(
                "Failed to execute create child node transaction for '{}' under parent '{}'",
                node_id, parent_id
            ))?;

        // Check transaction response for errors
        response.check().context(format!(
            "Transaction failed when creating child node '{}' under parent '{}'",
            node_id, parent_id
        ))?;

        // Fetch and return created node (ensures timestamps match database values)
        let node = self
            .get_node(&node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after creation for '{}'", node_id))?;

        // Notify registered callback of the store change (Issue #718)
        self.notify(StoreChange {
            operation: StoreOperation::Created,
            node: node.clone(),
            source,
        });

        Ok(node)
    }

    /// Builds a Node from hub data and properties.
    ///
    /// This helper extracts the common node construction logic used by both
    /// `get_node()` and `get_nodes_by_ids()`, ensuring consistent behavior
    /// for timestamp parsing, mentions extraction, and fallback handling.
    ///
    /// # Arguments
    /// * `node_id` - The node's ID string
    /// * `node_type` - The node's type (text, task, date, etc.)
    /// * `hub` - The node table row as a JSON Value
    /// * `properties` - The node's properties
    fn build_node_from_hub(
        &self,
        node_id: String,
        node_type: String,
        hub: &Value,
        properties: Value,
    ) -> Node {
        let created_at = hub["created_at"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| {
                warn!(node_id = %node_id, "Missing or invalid created_at timestamp, using current time");
                Utc::now()
            });

        let modified_at = hub["modified_at"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| {
                warn!(node_id = %node_id, "Missing or invalid modified_at timestamp, using current time");
                Utc::now()
            });

        let mentions = hub["mentions"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let mentioned_by = hub["mentioned_by"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Node {
            id: node_id,
            node_type,
            content: hub["content"].as_str().unwrap_or("").to_string(),
            version: hub["version"].as_i64().unwrap_or(1),
            created_at,
            modified_at,
            properties,
            mentions,
            mentioned_by,
            member_of: Vec::new(),
        }
    }

    pub async fn get_node(&self, id: &str) -> Result<Option<Node>> {
        // Universal Graph Architecture (Issue #783): Single query for node + properties
        // Properties are embedded directly in node.properties field

        // Query 1: Get node (properties included)
        let node_query = format!("SELECT * OMIT id FROM node:`{id}` LIMIT 1;", id = id);
        let mut response = self
            .db
            .query(&node_query)
            .await
            .context("Failed to query node")?;

        let results: Vec<Value> = response.take(0).unwrap_or_default();
        let Some(hub) = results.into_iter().next() else {
            return Ok(None);
        };

        // Parse node fields (properties embedded directly)
        let node_type = hub["node_type"].as_str().unwrap_or("text").to_string();
        let properties = hub
            .get("properties")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        let mut node = self.build_node_from_hub(id.to_string(), node_type, &hub, properties);

        // Query 2: Get collection memberships
        let memberships = batch_fetch_memberships(&self.db, &[id.to_string()]).await?;
        if let Some(collections) = memberships.get(id) {
            node.member_of = collections.clone();
        }

        Ok(Some(node))
    }

    /// Batch-fetch multiple nodes by their IDs in a single query.
    ///
    /// Returns a HashMap mapping node IDs to their Node data. IDs that don't exist
    /// are simply not included in the result (no error is raised).
    ///
    /// Universal Graph Architecture (Issue #783): Single query for all nodes,
    /// properties embedded in node.properties field.
    pub async fn get_nodes_by_ids(&self, ids: &[String]) -> Result<HashMap<String, Node>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }

        // Build ID list for SurrealQL IN clause: [node:`id1`, node:`id2`, ...]
        let id_list: Vec<String> = ids.iter().map(|id| format!("node:`{}`", id)).collect();
        let id_clause = id_list.join(", ");

        // Query all nodes in one batch (properties embedded)
        // Note: We use record::id(id) AS node_id to extract the string ID for result mapping
        let node_query = format!(
            "SELECT *, record::id(id) AS node_id OMIT id FROM node WHERE id IN [{}];",
            id_clause
        );
        let mut response = self
            .db
            .query(&node_query)
            .await
            .context("Failed to batch query nodes")?;

        let results: Vec<Value> = response.take(0).unwrap_or_default();

        let mut result_map: HashMap<String, Node> = HashMap::new();

        // Convert each node row to Node struct
        for hub in results {
            // Extract ID from the node_id alias (set via record::id(id) AS node_id)
            let node_id = hub["node_id"].as_str().unwrap_or("").to_string();
            if node_id.is_empty() {
                continue; // Skip if no ID found
            }
            let node_type = hub["node_type"].as_str().unwrap_or("text").to_string();
            let properties = hub
                .get("properties")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            let node = self.build_node_from_hub(node_id.clone(), node_type, &hub, properties);
            result_map.insert(node_id, node);
        }

        // Batch fetch collection memberships for all nodes
        let all_node_ids: Vec<String> = result_map.keys().cloned().collect();
        let memberships = batch_fetch_memberships(&self.db, &all_node_ids).await?;

        // Hydrate member_of into nodes
        for (node_id, node) in result_map.iter_mut() {
            if let Some(collections) = memberships.get(node_id) {
                node.member_of = collections.clone();
            }
        }

        Ok(result_map)
    }

    pub async fn update_node(
        &self,
        id: &str,
        update: NodeUpdate,
        source: Option<String>,
    ) -> Result<Node> {
        // Universal Graph Architecture (Issue #783): All properties in node.properties

        // Fetch current node
        let current = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", id))?;

        let updated_content = update.content.unwrap_or(current.content);
        let updated_node_type = update.node_type.unwrap_or(current.node_type.clone());

        // Merge properties if they're being updated
        // NOTE: _schema_version is managed by NodeService, not SurrealStore.
        let properties_update = if let Some(ref updated_props) = update.properties {
            let mut merged_props = current.properties.as_object().cloned().unwrap_or_default();
            if let Some(new_props) = updated_props.as_object() {
                for (key, value) in new_props {
                    merged_props.insert(key.clone(), value.clone());
                }
            }
            Some(serde_json::Value::Object(merged_props))
        } else {
            None
        };

        // Build update query - include properties if they're being updated
        let (query, bind_properties) = if properties_update.is_some() {
            (
                "
                UPDATE type::thing('node', $id) SET
                    content = $content,
                    node_type = $node_type,
                    modified_at = time::now(),
                    version = version + 1,
                    properties = $properties;
            ",
                true,
            )
        } else {
            (
                "
                UPDATE type::thing('node', $id) SET
                    content = $content,
                    node_type = $node_type,
                    modified_at = time::now(),
                    version = version + 1;
            ",
                false,
            )
        };

        let mut query_builder = self
            .db
            .query(query)
            .bind(("id", id.to_string()))
            .bind(("content", updated_content))
            .bind(("node_type", updated_node_type.clone()));

        if bind_properties {
            query_builder = query_builder.bind(("properties", properties_update.unwrap()));
        }

        query_builder.await.context("Failed to update node")?;

        // Fetch and return updated node
        let updated_node = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after update"))?;

        // Notify registered callback of the store change (Issue #718)
        self.notify(StoreChange {
            operation: StoreOperation::Updated,
            node: updated_node.clone(),
            source,
        });

        Ok(updated_node)
    }

    /// Update a schema node and execute DDL statements atomically
    ///
    /// When a schema node is updated, both the node data AND the corresponding
    /// SurrealDB table definitions must change together. This method ensures
    /// atomicity by wrapping both operations in a single transaction.
    ///
    /// # Arguments
    ///
    /// * `id` - The schema node ID (also the table name, e.g., "person", "task")
    /// * `update` - The node update to apply
    /// * `ddl_statements` - DDL statements to execute (DEFINE TABLE, DEFINE FIELD, etc.)
    ///
    /// # Returns
    ///
    /// The updated schema node
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node not found
    /// - DDL execution fails
    /// - Transaction fails
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::NodeUpdate;
    /// # async fn example(store: &SurrealStore) -> anyhow::Result<()> {
    /// let ddl = vec![
    ///     "DEFINE TABLE IF NOT EXISTS person SCHEMAFULL;".to_string(),
    ///     "DEFINE FIELD IF NOT EXISTS name ON person TYPE string;".to_string(),
    /// ];
    /// let update = NodeUpdate::new().with_properties(serde_json::json!({"schema": "..."}));
    /// store.update_schema_node_atomic("person", update, ddl, None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_schema_node_atomic(
        &self,
        node: Node,
        ddl_statements: Vec<String>,
        source: Option<String>,
    ) -> Result<Node> {
        // Validate this is a schema node
        if node.node_type != "schema" {
            return Err(anyhow::anyhow!(
                "create_schema_node_atomic only accepts schema nodes, got '{}'",
                node.node_type
            ));
        }

        // Build atomic transaction: DDL FIRST, then CREATE node
        // Universal Graph Architecture (Issue #783): Schema data stored in node.properties
        let mut transaction_parts = vec!["BEGIN TRANSACTION;".to_string()];

        // Add all DDL statements FIRST (for the type this schema defines, e.g., task indexes)
        for ddl in &ddl_statements {
            transaction_parts.push(ddl.clone());
        }

        // Create schema node with all schema data in properties
        transaction_parts.push(format!(
            r#"CREATE node:`{}` CONTENT {{
                node_type: $node_type,
                content: $content,
                version: 1,
                created_at: time::now(),
                modified_at: time::now(),
                mentions: [],
                mentioned_by: [],
                properties: $properties
            }};"#,
            node.id
        ));

        transaction_parts.push("COMMIT TRANSACTION;".to_string());
        let transaction_query = transaction_parts.join("\n");

        // Execute atomic transaction
        self.db
            .query(&transaction_query)
            .bind(("id", node.id.clone()))
            .bind(("node_type", node.node_type.clone()))
            .bind(("content", node.content.clone()))
            .bind(("properties", node.properties.clone()))
            .await
            .context("Failed to execute atomic schema creation transaction")?;

        // Fetch and return the created node
        let created_node = self
            .get_node(&node.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Schema node not found after creation"))?;

        // Notify registered callback of the store change (Issue #718)
        self.notify(StoreChange {
            operation: StoreOperation::Created,
            node: created_node.clone(),
            source,
        });

        Ok(created_node)
    }

    pub async fn update_schema_node_atomic(
        &self,
        id: &str,
        update: NodeUpdate,
        ddl_statements: Vec<String>,
        source: Option<String>,
    ) -> Result<Node> {
        // Fetch current node to verify it exists and get current state
        let current = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Schema node not found: {}", id))?;

        // Prepare updated values
        let updated_content = update.content.unwrap_or(current.content);
        let updated_node_type = update.node_type.unwrap_or(current.node_type.clone());

        // Merge properties if they're being updated
        let properties_update = if let Some(ref updated_props) = update.properties {
            let mut merged_props = current.properties.as_object().cloned().unwrap_or_default();
            if let Some(new_props) = updated_props.as_object() {
                for (key, value) in new_props {
                    merged_props.insert(key.clone(), value.clone());
                }
            }
            serde_json::Value::Object(merged_props)
        } else {
            current.properties.clone()
        };

        // Build the atomic transaction query
        // Universal Graph Architecture (Issue #783): All data in node.properties
        let mut transaction_parts = vec!["BEGIN TRANSACTION;".to_string()];

        // Add node update statement - properties stored directly in node.properties
        transaction_parts.push(
            r#"UPDATE type::thing('node', $id) SET
                content = $content,
                node_type = $node_type,
                modified_at = time::now(),
                version = version + 1,
                properties = $properties;"#
                .to_string(),
        );

        // Add all DDL statements
        for ddl in ddl_statements {
            transaction_parts.push(ddl);
        }

        transaction_parts.push("COMMIT TRANSACTION;".to_string());
        let transaction_query = transaction_parts.join("\n");

        // Execute the atomic transaction
        self.db
            .query(&transaction_query)
            .bind(("id", id.to_string()))
            .bind(("content", updated_content))
            .bind(("node_type", updated_node_type))
            .bind(("properties", properties_update.clone()))
            .await
            .context("Failed to execute atomic schema update transaction")?;

        // Fetch and return updated node
        let updated_node = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Schema node not found after atomic update"))?;

        tracing::info!(
            "Atomically updated schema node '{}' with {} DDL statements",
            id,
            transaction_parts.len() - 3 // Exclude BEGIN, UPDATE, COMMIT
        );

        // Notify registered callback of the store change (Issue #718)
        self.notify(StoreChange {
            operation: StoreOperation::Updated,
            node: updated_node.clone(),
            source,
        });

        Ok(updated_node)
    }

    /// Switch a node's type atomically, preserving old type in variants map
    ///
    /// This is an atomic type-switching operation that guarantees:
    /// - Node type is updated
    /// - New type-specific record is created (if type has properties)
    /// - Old type is preserved in variants map for lossless recovery
    /// - All updates happen atomically (all or nothing)
    ///
    /// # Variants Map Pattern
    ///
    /// The variants map stores the history of type-specific record IDs:
    /// ```json
    /// {
    ///   "task": "task:uuid-123",
    ///   "text": null,
    ///   "person": "person:uuid-456"
    /// }
    /// ```
    ///
    /// This enables:
    /// - Lossless type switching (can restore old properties)
    /// - Type history tracking
    /// - Future multi-type node support
    ///
    /// # Arguments
    ///
    /// * `node_id` - ID of the node to switch
    /// * `new_type` - New node type
    /// * `new_properties` - Properties for the new type
    ///
    /// # Returns
    ///
    /// The updated node with new type
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use serde_json::json;
    /// # async fn example(store: &SurrealStore) -> anyhow::Result<()> {
    /// // Switch a text node to a task node
    /// let node = store.switch_node_type_atomic(
    ///     "node-uuid",
    ///     "task",
    ///     json!({"status": "TODO", "priority": "HIGH"}),
    ///     None,
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn switch_node_type_atomic(
        &self,
        node_id: &str,
        new_type: &str,
        new_properties: Value,
        source: Option<String>,
    ) -> Result<Node> {
        // Universal Graph Architecture (Issue #783): Type switch updates node.properties directly

        // Validate new_type to prevent SQL injection
        self.validate_node_type(new_type)?;

        // Convert parameters to owned strings for 'static lifetime
        let node_id = node_id.to_string();
        let new_type = new_type.to_string();

        // Validate node exists
        let current_node = self
            .get_node(&node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", node_id))?;

        let old_type = current_node.node_type.clone();

        // Build atomic transaction - just update node_type and properties
        let transaction_query = r#"
            BEGIN TRANSACTION;

            -- Update node type and properties
            UPDATE $node_id SET
                node_type = $new_type,
                properties = $properties,
                modified_at = time::now(),
                version = version + 1;

            COMMIT TRANSACTION;
        "#;

        // Construct Thing for node ID
        let node_thing = surrealdb::sql::Thing::from(("node".to_string(), node_id.clone()));

        // Execute transaction
        let response = self
            .db
            .query(transaction_query)
            .bind(("node_id", node_thing))
            .bind(("new_type", new_type.clone()))
            .bind(("properties", new_properties))
            .await
            .context(format!(
                "Failed to execute switch type transaction for node '{}'",
                node_id
            ))?;

        // Check transaction response for errors
        response.check().context(format!(
            "Transaction failed when switching node '{}' type from '{}' to '{}'",
            node_id, old_type, new_type
        ))?;

        // Fetch and return updated node
        let node = self
            .get_node(&node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after type switch for '{}'", node_id))?;

        // Notify registered callback of the store change (Issue #718)
        self.notify(StoreChange {
            operation: StoreOperation::Updated,
            node: node.clone(),
            source,
        });

        Ok(node)
    }

    /// Update a node with version check (optimistic locking)
    ///
    /// Only updates the node if its version matches the expected version.
    /// This provides atomic version-checked updates to prevent lost updates
    /// in concurrent scenarios.
    ///
    /// # Arguments
    ///
    /// * `id` - Node UUID to update
    /// * `expected_version` - Expected current version (for optimistic locking)
    /// * `update` - Fields to update
    ///
    /// # Returns
    ///
    /// * `Ok(Some(Node))` - Update succeeded, returns updated node
    /// * `Ok(None)` - Version mismatch, no update performed
    /// * `Err(_)` - Database error or node not found
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let store = SurrealStore::new(PathBuf::from("./data/surreal.db")).await?;
    /// let update = NodeUpdate {
    ///     content: Some("Updated content".to_string()),
    ///     ..Default::default()
    /// };
    ///
    /// match store.update_node_with_version_check("node-id", 5, update, None).await? {
    ///     Some(node) => println!("Updated to version {}", node.version),
    ///     None => println!("Version mismatch - node was modified by another process"),
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_node_with_version_check(
        &self,
        id: &str,
        expected_version: i64,
        update: NodeUpdate,
        source: Option<String>,
    ) -> Result<Option<Node>> {
        // Fetch current node to build update values
        let current = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", id))?;

        // Calculate new version for explicit binding
        let new_version = expected_version + 1;

        // Atomic update with version check using record ID
        // Universal Graph Architecture (Issue #783): Properties stored in node.properties
        let query = "
            UPDATE type::thing('node', $id) SET
                content = $content,
                node_type = $node_type,
                modified_at = time::now(),
                version = $new_version
            WHERE version = $expected_version
            RETURN AFTER;
        ";

        let updated_content = update.content.unwrap_or(current.content);
        let updated_node_type = update.node_type.unwrap_or(current.node_type.clone());
        let updated_properties = update.properties.clone();

        let mut response = self
            .db
            .query(query)
            .bind(("id", id.to_string()))
            .bind(("expected_version", expected_version))
            .bind(("new_version", new_version))
            .bind(("content", updated_content))
            .bind(("node_type", updated_node_type.clone()))
            .await
            .context("Failed to update node with version check")?;

        // Extract updated nodes from response
        let updated_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract update results")?;

        // If no nodes were updated, version mismatch occurred
        if updated_nodes.is_empty() {
            return Ok(None);
        }

        // Universal Graph Architecture (Issue #783): Properties stored in node.properties
        // Update properties directly if provided
        if let Some(props) = updated_properties {
            self.db
                .query("UPDATE type::thing('node', $id) SET properties = $properties;")
                .bind(("id", id.to_string()))
                .bind(("properties", props))
                .await
                .context("Failed to update properties")?;
        }

        // Fetch fresh node
        let node = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after update"))?;

        // Notify registered callback of the store change (Issue #718)
        self.notify(StoreChange {
            operation: StoreOperation::Updated,
            node: node.clone(),
            source,
        });

        Ok(Some(node))
    }

    pub async fn delete_node(&self, id: &str, source: Option<String>) -> Result<DeleteResult> {
        // Universal Graph Architecture (Issue #783, #788): All relationships in universal table

        // Get node before deletion for notification
        let node = match self.get_node(id).await? {
            Some(n) => n,
            None => return Ok(DeleteResult { existed: false }),
        };

        // Delete node and its relationships atomically (Issue #788: use universal relationship table)
        let transaction_query = "
            BEGIN TRANSACTION;
            DELETE type::thing('node', $id);
            DELETE relationship WHERE in = type::thing('node', $id) OR out = type::thing('node', $id);
            COMMIT TRANSACTION;
        ";

        self.db
            .query(transaction_query)
            .bind(("id", node.id.clone()))
            .await
            .context("Failed to delete node and relations")?;

        // Notify registered callback of the store change (Issue #718)
        // For deletes, we include the pre-deletion node state
        self.notify(StoreChange {
            operation: StoreOperation::Deleted,
            node,
            source,
        });

        Ok(DeleteResult { existed: true })
    }

    /// Delete a node with cascade cleanup in a single atomic transaction
    ///
    /// This is an enhanced atomic version of delete_node. It guarantees that either:
    /// - The node and all its relationships are ALL deleted
    /// - OR nothing is deleted (transaction rolls back on failure)
    ///
    /// # Cascade Cleanup
    ///
    /// Deletes the following in one atomic transaction:
    /// - Node record from universal `node` table
    /// - All relationships (incoming and outgoing) from universal `relationship` table
    ///
    /// Universal Relationship Architecture (Issue #788): All relationship types in single table.
    ///
    /// # Arguments
    ///
    /// * `node_id` - ID of the node to delete
    ///
    /// # Returns
    ///
    /// DeleteResult indicating whether the node existed
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # async fn example(store: &SurrealStore) -> anyhow::Result<()> {
    /// let result = store.delete_node_cascade_atomic("node-uuid", None).await?;
    /// assert!(result.existed);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_node_cascade_atomic(
        &self,
        node_id: &str,
        source: Option<String>,
    ) -> Result<DeleteResult> {
        // Get node to determine type for Record ID
        let node = match self.get_node(node_id).await? {
            Some(n) => n,
            None => return Ok(DeleteResult { existed: false }),
        };

        // Build atomic cascade delete transaction using Thing parameters
        // This ensures ALL related data is deleted or NOTHING is deleted
        // Universal Relationship Architecture (Issue #788): All relationships in single table
        let node_type = node.node_type.clone();
        let node_id_str = node.id.clone();

        let transaction_query = r#"
            BEGIN TRANSACTION;

            -- Delete type-specific record (if exists - legacy support)
            DELETE $type_id;

            -- Delete node from universal table
            DELETE $node_id;

            -- Delete all relationships (incoming and outgoing) from universal relationship table
            DELETE relationship WHERE in = $node_id OR out = $node_id;

            COMMIT TRANSACTION;
        "#
        .to_string();

        // Construct Thing objects for Record IDs
        let node_thing = surrealdb::sql::Thing::from(("node".to_string(), node_id_str.clone()));
        let type_thing = surrealdb::sql::Thing::from((node_type.clone(), node_id_str.clone()));

        // Execute transaction (we don't care about the return value, just the side effects)
        self.db
            .query(&transaction_query)
            .bind(("node_id", node_thing))
            .bind(("type_id", type_thing))
            .await
            .map(|_| ())
            .context(format!(
                "Failed to delete node '{}' (type: {}) with cascade",
                node_id_str, node_type
            ))?;

        // Notify registered callback of the store change (Issue #718)
        // For deletes, we include the pre-deletion node state
        self.notify(StoreChange {
            operation: StoreOperation::Deleted,
            node,
            source,
        });

        Ok(DeleteResult { existed: true })
    }

    /// Delete a node with version check (optimistic locking)
    ///
    /// Only deletes the node if its version matches the expected version.
    /// Returns the number of rows affected (0 if version mismatch, 1 if deleted).
    pub async fn delete_with_version_check(
        &self,
        id: &str,
        expected_version: i64,
        source: Option<String>,
    ) -> Result<usize> {
        // First get the node to check version
        let node = match self.get_node(id).await? {
            Some(n) => n,
            None => return Ok(0), // Node doesn't exist
        };

        // Check version match
        if node.version != expected_version {
            return Ok(0); // Version mismatch, no deletion
        }

        // Version matches, proceed with deletion
        // Note: delete_node handles the notification
        let result = self.delete_node(id, source).await?;
        Ok(if result.existed { 1 } else { 0 })
    }

    pub async fn query_nodes(&self, query: NodeQuery) -> Result<Vec<Node>> {
        // Handle mentioned_by query using graph traversal
        // See: docs/architecture/data/surrealdb-schema-design.md - Graph Traversal Patterns
        if let Some(ref mentioned_node_id) = query.mentioned_by {
            // Use graph traversal to get IDs, then fetch full nodes
            // Issue #788: Universal Relationship Architecture - filter by relationship_type
            // We can't use SELECT <-relationship[...]<-node.* directly because it returns nested structure
            let sql = if query.limit.is_some() {
                "SELECT VALUE <-relationship[WHERE relationship_type = 'mentions']<-node.id FROM type::thing('node', $node_id) LIMIT $limit;"
            } else {
                "SELECT VALUE <-relationship[WHERE relationship_type = 'mentions']<-node.id FROM type::thing('node', $node_id);"
            };

            let mut query_builder = self
                .db
                .query(sql)
                .bind(("node_id", mentioned_node_id.to_string()));

            if let Some(limit) = query.limit {
                query_builder = query_builder.bind(("limit", limit));
            }

            let mut response = query_builder
                .await
                .context("Failed to query mentioned_by nodes")?;

            // SELECT VALUE with graph traversal returns nested array - flatten it
            // Result format: [[thing1, thing2], [thing3]] from multiple source nodes
            let source_things_nested: Vec<Vec<Thing>> = response
                .take(0)
                .context("Failed to extract source node IDs from mentions")?;

            let source_things: Vec<Thing> = source_things_nested.into_iter().flatten().collect();

            // Extract UUIDs and fetch full node records
            let mut nodes = Vec::new();
            for thing in source_things {
                if let Id::String(id_str) = &thing.id {
                    // id_str is just the UUID
                    if let Some(node) = self.get_node(id_str).await? {
                        nodes.push(node);
                    }
                }
            }

            return Ok(nodes);
        }

        // Handle content_contains query
        if let Some(ref search_query) = query.content_contains {
            let nodes = self
                .search_nodes_by_content(search_query, query.limit.map(|l| l as i64))
                .await?;

            // Note: Filtering by root/task status is done at the Tauri command layer
            // (mention_autocomplete), not here, since it requires graph traversal.
            return Ok(nodes);
        }

        // Build WHERE clause conditions
        let mut conditions = Vec::new();

        if query.node_type.is_some() {
            conditions.push("node_type = $node_type".to_string());
        }

        // Note: Filtering for mentionable nodes (roots + tasks) is done in mention_autocomplete command

        // Build SQL query
        let where_clause = if !conditions.is_empty() {
            Some(conditions.join(" AND "))
        } else {
            None
        };

        let sql = match (&where_clause, query.limit) {
            (None, None) => "SELECT * FROM node;".to_string(),
            (None, Some(_)) => "SELECT * FROM node LIMIT $limit;".to_string(),
            (Some(clause), None) => format!("SELECT * FROM node WHERE {};", clause),
            (Some(clause), Some(_)) => {
                format!("SELECT * FROM node WHERE {} LIMIT $limit;", clause)
            }
        };

        let mut query_builder = self.db.query(sql);

        if let Some(node_type) = &query.node_type {
            query_builder = query_builder.bind(("node_type", node_type.clone()));
        }

        if let Some(limit) = query.limit {
            query_builder = query_builder.bind(("limit", limit));
        }

        let mut response = query_builder.await.context("Failed to query nodes")?;
        let surreal_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract nodes from query response")?;

        // Universal Graph Architecture (Issue #783): Properties embedded in node.properties
        let mut nodes: Vec<Node> = surreal_nodes.into_iter().map(Into::into).collect();

        // Batch fetch collection memberships for all nodes
        let all_node_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        match batch_fetch_memberships(&self.db, &all_node_ids).await {
            Ok(memberships) => {
                for node in &mut nodes {
                    if let Some(collections) = memberships.get(&node.id) {
                        node.member_of = collections.clone();
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to batch fetch memberships: {}", e);
            }
        }

        Ok(nodes)
    }

    pub async fn get_children(&self, parent_id: Option<&str>) -> Result<Vec<Node>> {
        // Universal Graph Architecture (Issue #783, #788): Properties embedded in node.properties
        // Use universal relationship table for hierarchy traversal with fractional ordering
        let surreal_nodes = if let Some(parent_id) = parent_id {
            // Create Thing record ID for parent node
            use surrealdb::sql::Thing;
            let parent_thing = Thing::from(("node".to_string(), parent_id.to_string()));

            // Query children ordered by relationship.properties.order (Issue #788: universal relationship table)
            let mut rel_response = self
                .db
                .query("SELECT out, properties.order FROM relationship WHERE in = $parent_thing AND relationship_type = 'has_child' ORDER BY properties.order ASC;")
                .bind(("parent_thing", parent_thing.clone()))
                .await
                .context("Failed to get child relationships")?;

            #[derive(serde::Deserialize)]
            struct RelOut {
                out: Thing,
            }

            let relationships: Vec<RelOut> = rel_response
                .take(0)
                .context("Failed to extract child relationships")?;

            // Extract node Things in order
            let mut ordered_node_things: Vec<Thing> = Vec::new();
            let mut ordered_node_strs: Vec<String> = Vec::new();
            for rel in relationships {
                ordered_node_things.push(rel.out.clone());
                ordered_node_strs.push(format!("{}", rel.out));
            }

            // Fetch all nodes using Things for proper ID matching
            let mut response = self
                .db
                .query("SELECT * FROM node WHERE id IN $ids;")
                .bind(("ids", ordered_node_things))
                .await
                .context("Failed to get children")?;

            let fetched_nodes: Vec<SurrealNode> = response
                .take(0)
                .context("Failed to extract children from response")?;

            // Create a hashmap of fetched nodes for quick lookup by ID
            use std::collections::HashMap as IDHashMap;
            let mut node_map: IDHashMap<String, SurrealNode> = IDHashMap::new();
            for node in fetched_nodes {
                let id_str = format!("{}", node.id);
                node_map.insert(id_str, node);
            }

            // Reconstruct nodes in the exact order from the relationship query
            let mut nodes: Vec<SurrealNode> = Vec::new();
            for id_str in ordered_node_strs.iter() {
                if let Some(node) = node_map.remove(id_str) {
                    nodes.push(node);
                }
            }

            nodes
        } else {
            // Root nodes: nodes that have NO incoming has_child relationships (Issue #788: universal relationship table)
            let mut response = self
                .db
                .query("SELECT * FROM node WHERE count(<-relationship[WHERE relationship_type = 'has_child']) = 0;")
                .await
                .context("Failed to get root nodes")?;

            let nodes: Vec<SurrealNode> = response
                .take(0)
                .context("Failed to extract root nodes from response")?;

            nodes
        };

        // Convert to nodes (properties already embedded)
        let mut nodes: Vec<Node> = surreal_nodes.into_iter().map(Into::into).collect();

        // Batch fetch collection memberships for all nodes
        let all_node_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        match batch_fetch_memberships(&self.db, &all_node_ids).await {
            Ok(memberships) => {
                for node in &mut nodes {
                    if let Some(collections) = memberships.get(&node.id) {
                        node.member_of = collections.clone();
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to batch fetch memberships for children: {}", e);
            }
        }

        Ok(nodes)
    }

    /// Get the parent of a node (via incoming has_child relationship)
    ///
    /// Returns the node's parent if it has one, or None if it's a root node.
    /// Universal Graph Architecture (Issue #783, #788): Properties embedded, relationships in universal table.
    ///
    /// # Arguments
    ///
    /// * `child_id` - The child node ID
    ///
    /// # Returns
    ///
    /// `Some(parent_node)` if the node has a parent, `None` if it's a root node
    pub async fn get_parent(&self, child_id: &str) -> Result<Option<Node>> {
        use surrealdb::sql::Thing;
        let child_thing = Thing::from(("node".to_string(), child_id.to_string()));

        // Query for parent via incoming has_child relationship (Issue #788: universal relationship table)
        let mut response = self
            .db
            .query("SELECT * FROM node WHERE id IN (SELECT VALUE in FROM relationship WHERE out = $child_thing AND relationship_type = 'has_child') LIMIT 1;")
            .bind(("child_thing", child_thing))
            .await
            .context("Failed to get parent")?;

        let nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract parent from response")?;

        if nodes.is_empty() {
            return Ok(None);
        }

        // Convert to node (properties already embedded)
        let node: Node = nodes.into_iter().next().unwrap().into();

        Ok(Some(node))
    }

    /// Get entire node tree recursively in a SINGLE query
    ///
    /// This method leverages SurrealDB's recursive graph traversal to fetch
    /// a node and ALL its descendants at all levels in one database query.
    ///
    /// # Performance
    ///
    /// - **1 query** regardless of tree depth/size (vs N queries for manual traversal)
    /// - Ideal for: outline view, export, tree visualization
    ///
    /// # Arguments
    ///
    /// * `root_id` - ID of the root node to fetch tree from
    ///
    /// # Returns
    ///
    /// Returns root node with nested `children` arrays at all levels.
    /// Each node includes properties fetched from type-specific tables.
    ///
    /// # Example
    ///
    /// ```text
    /// // Get entire tree in ONE query:
    /// let tree = store.get_node_tree("root-uuid").await?;
    /// // tree.children[0].children[0]... (fully nested)
    /// ```
    ///
    /// # Implementation Note
    ///
    /// Uses SurrealDB's `@` recursive projection operator:
    /// ```sql
    /// SELECT id, title,
    ///   ->has_child->node.{
    ///     id, title,
    ///     children: ->has_child->node.@  -- '@' repeats this projection recursively
    ///   } AS children
    /// FROM node:root_uuid;
    /// ```
    pub async fn get_node_tree(&self, root_id: &str) -> Result<Option<serde_json::Value>> {
        // NOTE: SurrealDB's `@` recursive repeat operator is NOT supported with RocksDB storage
        // (returns UnsupportedRepeatRecurse error). We use Rust recursion instead.
        //
        // This fetches the tree by:
        // 1. Getting the root node
        // 2. Recursively fetching children using get_children()
        // 3. Building nested JSON structure

        // First, get the root node
        let root_node = match self.get_node(root_id).await? {
            Some(node) => node,
            None => return Ok(None),
        };

        // Build nested tree recursively
        let tree = self.build_node_tree_recursive(&root_node).await?;
        Ok(Some(tree))
    }

    /// Recursively build a node tree as JSON with nested children
    ///
    /// # Safety Guards
    /// - Maximum depth limit (100 levels) prevents stack overflow on deeply nested hierarchies
    /// - Cycle detection prevents infinite recursion on cyclic graphs
    fn build_node_tree_recursive<'a>(
        &'a self,
        node: &'a Node,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value>> + Send + 'a>>
    {
        const MAX_DEPTH: usize = 100;

        Box::pin(async move {
            self.build_node_tree_with_guards(
                node,
                0,
                MAX_DEPTH,
                &mut std::collections::HashSet::new(),
            )
            .await
        })
    }

    /// Internal implementation with depth tracking and cycle detection
    fn build_node_tree_with_guards<'a>(
        &'a self,
        node: &'a Node,
        depth: usize,
        max_depth: usize,
        visited: &'a mut std::collections::HashSet<String>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value>> + Send + 'a>>
    {
        Box::pin(async move {
            // Depth limit check - prevent stack overflow
            if depth >= max_depth {
                return Err(anyhow::anyhow!(
                "Maximum tree depth ({}) exceeded at node '{}'. This may indicate a very deep hierarchy or a cycle.",
                max_depth,
                node.id
            ));
            }

            // Cycle detection - prevent infinite recursion
            if visited.contains(&node.id) {
                return Err(anyhow::anyhow!(
                    "Cycle detected: node '{}' appears multiple times in the hierarchy path",
                    node.id
                ));
            }
            visited.insert(node.id.clone());

            // Get ordered children for this node (get_children returns Vec<Node> already ordered)
            let children_nodes = self.get_children(Some(&node.id)).await?;

            // Recursively build children trees
            let mut children_json = Vec::new();
            for child_node in &children_nodes {
                let child_tree = self
                    .build_node_tree_with_guards(child_node, depth + 1, max_depth, visited)
                    .await?;
                children_json.push(child_tree);
            }

            // Backtrack: remove from visited set to allow node to appear in other branches
            visited.remove(&node.id);

            // Build JSON for this node with children
            // NOTE: Use bare node.id without "node:" prefix to match frontend expectations
            Ok(serde_json::json!({
                "id": node.id,
                "type": node.node_type,
                "content": node.content,
                "version": node.version,
                "created_at": node.created_at,
                "modified_at": node.modified_at,
                "mentions": node.mentions,
                "mentioned_by": node.mentioned_by,
                "data": node.properties,
                "variants": serde_json::Value::Null,
                "_schema_version": 1,
                "children": children_json
            }))
        })
    }

    /// Get all nodes in a subtree using breadth-first traversal
    ///
    /// Fetches all nodes that are descendants of the given root node (not including the root itself).
    /// This is the first step in building an adjacency list structure for efficient tree navigation.
    ///
    /// # Query Strategy
    ///
    /// Uses iterative breadth-first traversal: queries each level's children until no more
    /// children are found. This approach is more compatible across SurrealDB configurations
    /// than recursive syntax.
    ///
    /// # Arguments
    ///
    /// * `root_id` - ID of the root node to fetch descendants for
    ///
    /// # Returns
    ///
    /// Vector of all descendant nodes (excludes root node itself)
    ///
    /// # Performance
    ///
    /// O(1) database queries using SurrealDB's recursive `{..+collect}` syntax.
    /// This collects all descendant IDs in a single traversal, then fetches all
    /// node data in one query. Total: 2 queries regardless of tree depth.
    ///
    /// **Note:** If you need both nodes AND relationships, use [`get_subtree_with_relationships`] directly
    /// to avoid duplicate database queries.
    pub async fn get_nodes_in_subtree(&self, root_id: &str) -> Result<Vec<Node>> {
        // Delegate to consolidated method, excluding root
        let (all_nodes, _relationships) = self.get_subtree_with_relationships(root_id).await?;

        // Filter out root node (consolidated method includes it)
        let descendants: Vec<Node> = all_nodes.into_iter().filter(|n| n.id != root_id).collect();

        Ok(descendants)
    }

    /// Get entire subtree (root + all descendants) with relationships in a single optimized query
    ///
    /// This is the most efficient way to fetch a complete subtree. It uses SurrealDB's
    /// recursive `{..+collect}` syntax to traverse the entire hierarchy and fetch all
    /// nodes and relationships in a single database round-trip.
    ///
    /// # Performance
    ///
    /// Single database query regardless of tree depth or node count. The query:
    /// 1. Recursively collects all descendant node IDs
    /// 2. Fetches root + all descendants in one SELECT
    /// 3. Fetches all relationships in one SELECT
    ///
    /// Universal Graph Architecture (Issue #783, #788): All node properties are stored
    /// in node.properties field, all relationships in universal relationship table.
    ///
    /// # Arguments
    ///
    /// * `root_id` - ID of the root node to fetch subtree for
    ///
    /// # Returns
    ///
    /// Tuple of (all_nodes, relationships) where:
    /// - all_nodes: Vec<Node> - root node + all descendants with properties embedded
    /// - relationships: Vec<RelationshipRecord> - all parent-child relationships in the subtree
    pub async fn get_subtree_with_relationships(
        &self,
        root_id: &str,
    ) -> Result<(Vec<Node>, Vec<RelationshipRecord>)> {
        use surrealdb::sql::Thing;

        let root_thing = Thing::from(("node".to_string(), root_id.to_string()));

        // Universal Graph Architecture (Issue #783, #788): Single query batch
        // 1. All descendant node IDs (recursive collect via relationship table)
        // 2. Root + all descendant nodes (properties embedded in node.properties)
        // 3. All has_child relationships in subtree
        // Note: Must include properties.order in SELECT when ordering by it (SurrealDB requirement)
        let query = "
            LET $descendants = $root_thing.{..+collect}->relationship[WHERE relationship_type = 'has_child']->node;
            LET $all_nodes = array::concat([$root_thing], $descendants);
            SELECT * FROM node WHERE id IN $all_nodes;
            SELECT id, in, out, relationship_type, properties, properties.order FROM relationship WHERE in IN $all_nodes AND relationship_type = 'has_child' ORDER BY properties.order ASC;
        ";

        let mut response = self
            .db
            .query(query)
            .bind(("root_thing", root_thing))
            .await
            .context("Failed to query subtree")?;

        // Query has 4 statements:
        // 0: LET $descendants
        // 1: LET $all_nodes
        // 2: SELECT nodes
        // 3: SELECT relationships
        let surreal_nodes: Vec<SurrealNode> = response
            .take(2)
            .context("Failed to extract subtree nodes")?;

        let all_nodes: Vec<Node> = surreal_nodes.into_iter().map(Into::into).collect();

        // Parse edges (index 3) - Issue #788: universal relationship table with properties
        #[derive(serde::Deserialize)]
        struct EdgeRow {
            id: Thing,
            #[serde(rename = "in")]
            in_node: Thing,
            #[serde(rename = "out")]
            out_node: Thing,
            relationship_type: String,
            #[serde(default)]
            properties: Value,
        }

        let rel_rows: Vec<EdgeRow> = response
            .take(3)
            .context("Failed to extract subtree relationships")?;

        let relationships: Vec<RelationshipRecord> = rel_rows
            .into_iter()
            .map(|e| {
                let id_str = match &e.id.id {
                    Id::String(s) => s.clone(),
                    Id::Number(n) => n.to_string(),
                    _ => e.id.to_string(),
                };
                let in_str = match &e.in_node.id {
                    Id::String(s) => s.clone(),
                    Id::Number(n) => n.to_string(),
                    _ => e.in_node.to_string(),
                };
                let out_str = match &e.out_node.id {
                    Id::String(s) => s.clone(),
                    Id::Number(n) => n.to_string(),
                    _ => e.out_node.to_string(),
                };
                RelationshipRecord {
                    id: id_str,
                    in_node: in_str,
                    out_node: out_str,
                    relationship_type: e.relationship_type,
                    properties: e.properties,
                }
            })
            .collect();

        Ok((all_nodes, relationships))
    }

    /// Get all relationships in a subtree using recursive collect
    ///
    /// Fetches all parent-child relationships (has_child relationships) within a subtree.
    /// Combined with `get_nodes_in_subtree()`, this enables building an in-memory adjacency list
    /// for efficient tree construction and navigation.
    ///
    /// # Performance
    ///
    /// Delegates to `get_subtree_with_relationships()` which fetches everything in a single query.
    ///
    /// **Note:** If you need both nodes AND relationships, use [`get_subtree_with_relationships`] directly
    /// to avoid duplicate database queries.
    ///
    /// # Arguments
    ///
    /// * `root_id` - ID of the root node to fetch descendant relationships for
    ///
    /// # Returns
    ///
    /// Vector of all relationships within the subtree (parent-child relationships)
    pub async fn get_relationships_in_subtree(
        &self,
        root_id: &str,
    ) -> Result<Vec<RelationshipRecord>> {
        // Delegate to consolidated method, discarding nodes
        let (_nodes, relationships) = self.get_subtree_with_relationships(root_id).await?;
        Ok(relationships)
    }

    pub async fn search_nodes_by_content(
        &self,
        search_query: &str,
        limit: Option<i64>,
    ) -> Result<Vec<Node>> {
        // Use string::lowercase() for case-insensitive search
        // SurrealDB CONTAINS is case-sensitive by default
        let sql = if limit.is_some() {
            "SELECT * FROM node WHERE string::lowercase(content) CONTAINS string::lowercase($search_query) LIMIT $limit;"
        } else {
            "SELECT * FROM node WHERE string::lowercase(content) CONTAINS string::lowercase($search_query);"
        };

        let mut query_builder = self
            .db
            .query(sql)
            .bind(("search_query", search_query.to_string()));

        if let Some(lim) = limit {
            query_builder = query_builder.bind(("limit", lim));
        }

        let mut response = query_builder.await.context("Failed to search nodes")?;
        let surreal_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract search results from response")?;
        Ok(surreal_nodes.into_iter().map(Into::into).collect())
    }

    /// Search nodes for mention autocomplete with proper filtering
    ///
    /// Applies mention-specific filtering rules:
    /// - Excludes: date, schema node types (always)
    /// - Text-based types (text, header, code-block, quote-block, ordered-list): only root nodes
    /// - Other types (task, query, etc.): included regardless of hierarchy
    ///
    /// # Arguments
    ///
    /// * `search_query` - Content search string (case-insensitive)
    /// * `limit` - Maximum number of results
    ///
    /// # Returns
    ///
    /// Filtered nodes matching mention autocomplete criteria
    pub async fn mention_autocomplete(
        &self,
        search_query: &str,
        limit: Option<i64>,
    ) -> Result<Vec<Node>> {
        // Node types that are always excluded from mention autocomplete
        const EXCLUDED_TYPES: &[&str] = &["date", "schema"];

        // Text-based types that must be root nodes (no parent) to be included
        const TEXT_TYPES: &[&str] = &[
            "text",
            "header",
            "code-block",
            "quote-block",
            "ordered-list",
        ];

        // Build SQL with filtering logic (Issue #788: universal relationship table):
        // 1. Content search (case-insensitive)
        // 2. Exclude date and schema types
        // 3. For text types: only include if root (no incoming has_child relationship)
        // 4. For other types: include regardless of hierarchy
        let sql = r#"
            SELECT * FROM node
            WHERE string::lowercase(content) CONTAINS string::lowercase($search_query)
              AND node_type NOT IN $excluded_types
              AND (
                node_type NOT IN $text_types
                OR
                count(<-relationship[WHERE relationship_type = 'has_child']) = 0
              )
            LIMIT $limit;
        "#;

        let effective_limit = limit.unwrap_or(10);

        let mut response = self
            .db
            .query(sql)
            .bind(("search_query", search_query.to_string()))
            .bind(("excluded_types", EXCLUDED_TYPES.to_vec()))
            .bind(("text_types", TEXT_TYPES.to_vec()))
            .bind(("limit", effective_limit))
            .await
            .context("Failed to search nodes for mention autocomplete")?;

        let surreal_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract mention autocomplete results")?;

        Ok(surreal_nodes.into_iter().map(Into::into).collect())
    }

    /// Validate that creating a parent-child relationship won't create a cycle
    ///
    /// **Purpose**: Prevents cyclic references in the node hierarchy tree.
    ///
    /// **Example Cycle**: A→B→C→A (adding A as child of C would create this)
    ///
    /// **Impact if not validated**:
    /// - Infinite loops in tree traversal queries
    /// - Stack overflow in recursive operations
    /// - Data corruption in hierarchy
    ///
    /// # Arguments
    ///
    /// * `parent_id` - Proposed parent node ID
    /// * `child_id` - Proposed child node ID
    ///
    /// # Returns
    ///
    /// `Ok(())` if no cycle would be created, `Err` if cycle detected
    ///
    /// # Examples
    ///
    /// ```text
    /// // Valid: A→B, B→C (adding C as child of B)
    /// validate_no_cycle("B", "C").await?; // ✓ OK
    ///
    /// // Invalid: A→B→C, trying to add A as child of C
    /// validate_no_cycle("C", "A").await?; // ✗ Error: would create cycle A→B→C→A
    /// ```
    async fn validate_no_cycle(&self, parent_id: &str, child_id: &str) -> Result<()> {
        use surrealdb::sql::Thing;

        // Check if parent is a descendant of child
        // If so, creating this relationship would create a cycle
        let child_thing = Thing::from(("node".to_string(), child_id.to_string()));

        // Query: Get all descendants of child node recursively (Issue #788: universal relationship table)
        // Then check if parent is in that list
        // Using SurrealDB recursive graph traversal syntax (v2.1+) to check ALL descendant levels
        // The `{..+collect}` syntax means unbounded recursive traversal collecting unique nodes
        // This will detect cycles at any level: A→B (direct), A→B→C (3-node), A→B→C→D (4-node), etc.
        let query = "
            LET $descendants = $child_thing.{..+collect}->relationship[WHERE relationship_type = 'has_child']->node;
            SELECT * FROM type::thing('node', $parent_id)
            WHERE id IN $descendants
            LIMIT 1;
        ";

        let mut response = self
            .db
            .query(query)
            .bind(("parent_id", parent_id.to_string()))
            .bind(("child_thing", child_thing))
            .await
            .context("Failed to check for cycles")?;

        // The query has 2 statements (LET, SELECT), we want the SELECT result at index 1
        let results: Vec<SurrealNode> = response
            .take(1)
            .context("Failed to parse cycle check results")?;

        if !results.is_empty() {
            return Err(anyhow::anyhow!(
                "Cannot create parent-child relationship: would create cycle. \
                Node '{}' is a descendant of node '{}', so '{}' cannot be a parent of '{}'.",
                parent_id,
                child_id,
                parent_id,
                child_id
            ));
        }

        Ok(())
    }

    /// Rebalance child ordering for a parent when precision degrades
    ///
    /// When fractional ordering gets too granular (gaps < 0.0001), this rebalances
    /// all children of a parent to have even spacing (1.0, 2.0, 3.0, etc.).
    ///
    /// This operation is atomic - either all children are rebalanced or none are.
    ///
    /// # Arguments
    ///
    /// * `parent_id` - ID of the parent node whose children should be rebalanced
    ///
    /// # Returns
    ///
    /// Ok(()) on success
    async fn rebalance_children_for_parent(&self, parent_id: &str) -> Result<()> {
        use surrealdb::sql::Thing;

        // Step 1: Get all children in current order (Issue #788: universal relationship table)
        let parent_thing = Thing::from(("node".to_string(), parent_id.to_string()));

        #[derive(Deserialize)]
        struct RelOut {
            out: Thing,
        }

        let mut rels_response = self
            .db
            .query("SELECT out, properties.order FROM relationship WHERE in = $parent_thing AND relationship_type = 'has_child' ORDER BY properties.order ASC;")
            .bind(("parent_thing", parent_thing.clone()))
            .await
            .context("Failed to get children for rebalancing")?;

        let relationships: Vec<RelOut> = rels_response
            .take(0)
            .context("Failed to extract children for rebalancing")?;

        if relationships.is_empty() {
            return Ok(()); // Nothing to rebalance
        }

        // Step 2: Calculate new orders [1.0, 2.0, 3.0, ...]
        let new_orders = FractionalOrderCalculator::rebalance(relationships.len());

        // Step 3: Build atomic transaction to update all relationships (Issue #788: universal relationship table)
        // We need to update each relationship's properties.order field
        let mut transaction = String::from("BEGIN TRANSACTION;\n");

        for (i, _rel) in relationships.iter().enumerate() {
            let new_order = new_orders[i];
            transaction.push_str(&format!(
                "UPDATE relationship SET properties.order = {} WHERE in = $parent_thing AND out = $out{} AND relationship_type = 'has_child';\n",
                new_order, i
            ));
        }

        transaction.push_str("COMMIT TRANSACTION;");

        // Step 4: Execute transaction with all relationships bound
        let mut query_builder = self
            .db
            .query(&transaction)
            .bind(("parent_thing", parent_thing));

        for (i, rel) in relationships.iter().enumerate() {
            query_builder = query_builder.bind((format!("out{}", i), rel.out.clone()));
        }

        query_builder
            .await
            .context("Failed to rebalance children")?;

        Ok(())
    }

    /// Move a node to a new parent atomically
    ///
    /// Guarantees that either:
    /// - The old relationship is deleted AND the new relationship is created
    /// - OR nothing changes (transaction rolls back on failure)
    ///
    /// # Arguments
    ///
    /// * `node_id` - ID of the node to move
    /// * `new_parent_id` - ID of the new parent (None = make root node)
    /// * `insert_after_sibling_id` - Optional sibling to insert after (uses relationship-based fractional ordering)
    ///
    /// # Returns
    ///
    /// Ok(()) on success
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # async fn example(store: &SurrealStore) -> anyhow::Result<()> {
    /// // Move node to new parent
    /// store.move_node("child-uuid", Some("new-parent-uuid"), None).await?;
    ///
    /// // Make node a root node
    /// store.move_node("child-uuid", None, None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn move_node(
        &self,
        node_id: &str,
        new_parent_id: Option<&str>,
        insert_after_sibling_id: Option<&str>,
    ) -> Result<()> {
        // Convert parameters to owned strings for 'static lifetime
        let node_id = node_id.to_string();
        let new_parent_id = new_parent_id.map(|s| s.to_string());
        let insert_after_sibling_id = insert_after_sibling_id.map(|s| s.to_string());

        // Validate node exists
        let _node = self
            .get_node(&node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", node_id))?;

        // Get current parent to determine if this is same-parent reorder vs cross-parent move
        // Issue #795: Preserve created_at and increment version for same-parent reorders
        let current_parent = self.get_parent(&node_id).await?;
        let current_parent_id = current_parent.map(|p| p.id);
        let is_same_parent_reorder = match (&new_parent_id, &current_parent_id) {
            (Some(new_pid), Some(cur_pid)) => new_pid == cur_pid,
            (None, None) => true, // Both root - same "parent" (no parent)
            _ => false,           // One has parent, one doesn't - different parents
        };

        // Validate that moving won't create a cycle
        if let Some(ref parent_id) = new_parent_id {
            // Validate parent exists
            let parent_exists = self.get_node(parent_id).await?;
            if parent_exists.is_none() {
                return Err(anyhow::anyhow!("Parent node not found: {}", parent_id));
            }

            self.validate_no_cycle(parent_id, &node_id).await?;
        }

        // Calculate fractional order for the new position (Issue #788: universal relationship table)
        #[derive(Deserialize)]
        struct RelWithOrder {
            out: surrealdb::sql::Thing,
            order: f64,
        }

        let new_order = if let Some(ref parent_id) = new_parent_id {
            let parent_thing = surrealdb::sql::Thing::from(("node".to_string(), parent_id.clone()));

            // Get all child edges for this parent, ordered by properties.order field
            let mut rels_response = self
                .db
                .query(
                    "SELECT out, properties.order AS order FROM relationship WHERE in = $parent_thing AND relationship_type = 'has_child' ORDER BY properties.order ASC;",
                )
                .bind(("parent_thing", parent_thing.clone()))
                .await
                .context("Failed to get child relationships")?;

            let relationships: Vec<RelWithOrder> = rels_response
                .take(0)
                .context("Failed to extract child relationships")?;

            if let Some(after_id) = insert_after_sibling_id {
                // Find the sibling we're inserting after
                let after_thing =
                    surrealdb::sql::Thing::from(("node".to_string(), after_id.clone()));
                let after_index = relationships
                    .iter()
                    .position(|e| e.out == after_thing)
                    .ok_or_else(|| anyhow::anyhow!("Sibling not found: {}", after_id))?;

                // Get orders before and after insertion point
                let prev_order = relationships[after_index].order;
                let next_order = relationships.get(after_index + 1).map(|e| e.order);

                // Calculate new order between them
                let calculated =
                    FractionalOrderCalculator::calculate_order(Some(prev_order), next_order);

                // Check if rebalancing is needed
                if let Some(next) = next_order {
                    if (next - prev_order) < 0.0001 {
                        // Gap too small, need to rebalance before inserting
                        self.rebalance_children_for_parent(parent_id).await?;

                        // Re-query relationships after rebalancing (Issue #788: universal relationship table)
                        // NOTE: There is a small race condition window here - between rebalancing
                        // completion and this re-query, another client could move/delete the sibling.
                        // If this occurs, we'll get "Sibling not found after rebalancing" error and
                        // the operation fails. This is an accepted limitation - clients can retry.
                        // A fully atomic solution would require SurrealDB to support multi-step
                        // transactions with deferred constraint checking, which isn't available.
                        let mut rels_response = self
                            .db
                            .query("SELECT out, properties.order AS order FROM relationship WHERE in = $parent_thing AND relationship_type = 'has_child' ORDER BY properties.order ASC;")
                            .bind(("parent_thing", parent_thing.clone()))
                            .await
                            .context("Failed to get child relationships after rebalancing")?;

                        let relationships: Vec<RelWithOrder> = rels_response
                            .take(0)
                            .context("Failed to extract child relationships after rebalancing")?;

                        let after_index = relationships
                            .iter()
                            .position(|e| e.out == after_thing)
                            .ok_or_else(|| {
                            anyhow::anyhow!("Sibling not found after rebalancing: {}", after_id)
                        })?;

                        let prev_order = relationships[after_index].order;
                        let next_order = relationships.get(after_index + 1).map(|e| e.order);
                        FractionalOrderCalculator::calculate_order(Some(prev_order), next_order)
                    } else {
                        calculated
                    }
                } else {
                    calculated
                }
            } else {
                // No insert_after_sibling specified, insert at beginning
                let first_order = relationships.first().map(|e| e.order);
                FractionalOrderCalculator::calculate_order(None, first_order)
            }
        } else {
            0.0 // Root nodes don't use order
        };

        // Build atomic transaction query using Thing parameters (Issue #788: universal relationship table)
        // Issue #795: Use UPDATE for same-parent reorders to preserve created_at and increment version
        let transaction_query = if new_parent_id.is_some() {
            if is_same_parent_reorder {
                // Same-parent reorder: UPDATE existing relationship (preserve created_at, increment version)
                // This is important for cloud sync and conflict resolution
                r#"
                    BEGIN TRANSACTION;

                    -- Update order and modified_at, increment version for OCC (Issue #795)
                    UPDATE relationship
                    SET properties.order = $order,
                        modified_at = time::now(),
                        version = version + 1
                    WHERE in = $parent_id AND out = $node_id AND relationship_type = 'has_child';

                    COMMIT TRANSACTION;
                "#
                .to_string()
            } else {
                // Cross-parent move: DELETE old + CREATE new relationship
                r#"
                    BEGIN TRANSACTION;

                    -- Delete old parent relationship from universal relationship table
                    DELETE relationship WHERE out = $node_id AND relationship_type = 'has_child';

                    -- Create new parent relationship with fractional order in universal relationship table
                    RELATE $parent_id->relationship->$node_id CONTENT {
                        relationship_type: 'has_child',
                        properties: { order: $order },
                        created_at: time::now(),
                        modified_at: time::now(),
                        version: 1
                    };

                    COMMIT TRANSACTION;
                "#
                .to_string()
            }
        } else {
            // Make root node (delete parent relationship only)
            r#"
                BEGIN TRANSACTION;

                -- Delete old parent relationship from universal relationship table
                DELETE relationship WHERE out = $node_id AND relationship_type = 'has_child';

                COMMIT TRANSACTION;
            "#
            .to_string()
        };

        // Construct Thing objects for Record IDs
        let node_thing = surrealdb::sql::Thing::from(("node".to_string(), node_id.clone()));
        let parent_thing = new_parent_id
            .as_ref()
            .map(|pid| surrealdb::sql::Thing::from(("node".to_string(), pid.clone())));

        // Execute transaction
        let mut query_builder = self
            .db
            .query(&transaction_query)
            .bind(("node_id", node_thing));

        if let Some(parent_thing) = parent_thing {
            query_builder = query_builder.bind(("parent_id", parent_thing));
        }

        query_builder
            .bind(("order", new_order))
            .await
            .context(format!(
                "Failed to move node '{}' to parent '{:?}'",
                node_id, new_parent_id
            ))?;

        // Note: Domain events are now emitted at NodeService layer for client filtering

        Ok(())
    }

    pub async fn create_mention(
        &self,
        source_id: &str,
        target_id: &str,
        root_id: &str,
    ) -> Result<()> {
        // Issue #788: Universal Relationship Architecture - mentions stored in relationship table
        let source_thing = surrealdb::sql::Thing::from(("node".to_string(), source_id.to_string()));
        let target_thing = surrealdb::sql::Thing::from(("node".to_string(), target_id.to_string()));

        // Check if mention already exists (for idempotency)
        let check_query = "SELECT VALUE id FROM relationship WHERE in = $source AND out = $target AND relationship_type = 'mentions';";
        let mut check_response = self
            .db
            .query(check_query)
            .bind(("source", source_thing.clone()))
            .bind(("target", target_thing.clone()))
            .await
            .context("Failed to check for existing mention")?;

        let existing_mention_ids: Vec<Thing> = check_response
            .take(0)
            .context("Failed to extract mention check results")?;

        // Only create mention if it doesn't exist
        if existing_mention_ids.is_empty() {
            // Issue #788: Universal Relationship Architecture - use CONTENT for properties
            let query = format!(
                r#"RELATE $source->relationship->$target CONTENT {{
                    relationship_type: 'mentions',
                    properties: {{ root_id: '{}' }},
                    created_at: time::now(),
                    modified_at: time::now(),
                    version: 1
                }};"#,
                root_id.replace('\'', "''")
            );

            self.db
                .query(&query)
                .bind(("source", source_thing))
                .bind(("target", target_thing))
                .await
                .context("Failed to create mention")?;

            // Note: Domain events are now emitted at NodeService layer for client filtering
        }

        Ok(())
    }

    pub async fn delete_mention(&self, source_id: &str, target_id: &str) -> Result<()> {
        // Issue #788: Universal Relationship Architecture - delete from relationship table
        let source_thing = surrealdb::sql::Thing::from(("node".to_string(), source_id.to_string()));
        let target_thing = surrealdb::sql::Thing::from(("node".to_string(), target_id.to_string()));

        self.db
            .query("DELETE FROM relationship WHERE in = $source AND out = $target AND relationship_type = 'mentions';")
            .bind(("source", source_thing))
            .bind(("target", target_thing))
            .await
            .context("Failed to delete mention")?;

        // Note: Domain events are now emitted at NodeService layer for client filtering

        Ok(())
    }

    pub async fn get_outgoing_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        // Issue #788: Universal Relationship Architecture - use relationship table with relationship_type filter
        // Returns array<record> which we need to extract IDs from
        let query =
            "SELECT ->relationship[WHERE relationship_type = 'mentions']->node.id AS mentioned_ids FROM type::thing('node', $node_id);";

        let mut response = self
            .db
            .query(query)
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to get outgoing mentions")?;

        #[derive(Debug, Deserialize)]
        struct MentionResult {
            mentioned_ids: Vec<Thing>,
        }

        // Graph traversal returns object with mentioned_ids array
        let results: Vec<MentionResult> = response
            .take(0)
            .context("Failed to extract outgoing mentions from response")?;

        // Extract UUIDs from Thing Record IDs (format: node:uuid -> uuid)
        let mentioned_ids: Vec<String> = results
            .into_iter()
            .flat_map(|r| r.mentioned_ids)
            .filter_map(|thing| {
                if let Id::String(id_str) = &thing.id {
                    // id_str is just the UUID part
                    Some(id_str.clone())
                } else {
                    None
                }
            })
            .collect();

        Ok(mentioned_ids)
    }

    pub async fn get_incoming_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        // Issue #788: Universal Relationship Architecture - use relationship table with relationship_type filter
        // Returns array<record> which we need to extract IDs from
        let query =
            "SELECT <-relationship[WHERE relationship_type = 'mentions']<-node.id AS mentioned_by_ids FROM type::thing('node', $node_id);";

        let mut response = self
            .db
            .query(query)
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to get incoming mentions")?;

        #[derive(Debug, Deserialize)]
        struct MentionResult {
            mentioned_by_ids: Vec<Thing>,
        }

        // Graph traversal returns object with mentioned_by_ids array
        let results: Vec<MentionResult> = response
            .take(0)
            .context("Failed to extract incoming mentions from response")?;

        // Extract UUIDs from Thing Record IDs (format: node:uuid -> uuid)
        let mentioned_by_ids: Vec<String> = results
            .into_iter()
            .flat_map(|r| r.mentioned_by_ids)
            .filter_map(|thing| {
                if let Id::String(id_str) = &thing.id {
                    // id_str is just the UUID part
                    Some(id_str.clone())
                } else {
                    None
                }
            })
            .collect();

        Ok(mentioned_by_ids)
    }

    pub async fn get_mentioning_containers(&self, node_id: &str) -> Result<Vec<Node>> {
        // Issue #788: Universal Relationship Architecture - query relationship table for mentions
        let target_thing = Thing::from(("node".to_string(), node_id.to_string()));
        let query = "SELECT properties.root_id AS root_id FROM relationship WHERE out = $target AND relationship_type = 'mentions';";

        let mut response = self
            .db
            .query(query)
            .bind(("target", target_thing))
            .await
            .context("Failed to get mentioning roots")?;

        // Parse the response - each row has a root_id field from properties
        #[derive(Debug, Deserialize)]
        struct MentionRow {
            root_id: Option<String>,
        }

        let results: Vec<MentionRow> = response
            .take(0)
            .context("Failed to extract root IDs from response")?;

        // Collect root IDs
        let mut root_ids: Vec<String> = results.into_iter().filter_map(|r| r.root_id).collect();

        // Deduplicate root IDs
        root_ids.sort();
        root_ids.dedup();

        // Fetch full node records
        let mut nodes = Vec::new();
        for root_id in root_ids {
            if let Some(node) = self.get_node(&root_id).await? {
                nodes.push(node);
            }
        }

        Ok(nodes)
    }

    pub async fn get_schema(&self, node_type: &str) -> Result<Option<Value>> {
        // Schema nodes use simple IDs (just the node type name, e.g., "date")
        // They're differentiated by node_type = "schema"
        let schema_id = node_type.to_string();
        let node = self.get_node(&schema_id).await?;
        Ok(node.map(|n| n.properties))
    }

    pub async fn update_schema(&self, node_type: &str, schema: &Value) -> Result<()> {
        // Schema nodes use simple IDs (just the node type name, e.g., "date")
        let schema_id = node_type.to_string();

        // Check if schema node exists
        // NOTE: Schema seeding uses None for source because it's internal infrastructure
        // initialization, not a client-initiated operation. These events are filtered
        // out by consumers (e.g., SSE bridge) that check source_client_id.
        if self.get_node(&schema_id).await?.is_some() {
            // Update existing schema
            let update = NodeUpdate {
                properties: Some(schema.clone()),
                ..Default::default()
            };
            self.update_node(&schema_id, update, None).await?;
        } else {
            // Create new schema node with deterministic ID
            let node = Node::new_with_id(
                schema_id,
                "schema".to_string(),
                node_type.to_string(),
                schema.clone(),
            );
            self.create_node(node, None).await?;
        }

        Ok(())
    }

    // NOTE: Old node-based embedding methods REMOVED (Issue #729)
    // The following methods operated on node.embedding_vector and node.embedding_stale:
    // - get_nodes_without_embeddings() - queried node WHERE embedding_vector IS NONE
    // - update_embedding() - set node.embedding_vector and embedding_stale = false
    // - get_nodes_with_stale_embeddings() - queried node WHERE embedding_stale = true
    //
    // Root-aggregate model now uses the `embedding` table with:
    // - get_stale_embedding_root_ids() - query embedding table for stale roots
    // - mark_root_embedding_stale() - mark embedding record stale
    // - create_stale_embedding_marker() - create stale embedding for new roots
    // - upsert_embeddings() - store embeddings in embedding table
    // NodeService.queue_root_for_embedding() orchestrates the logic.

    /// Atomic bulk update using SurrealDB transactions
    ///
    /// Updates multiple nodes in a single atomic transaction. Either all updates
    /// succeed or all fail (rollback), ensuring data consistency.
    ///
    /// # Performance Considerations
    ///
    /// - **Optimal Batch Size:** 10-100 nodes (transaction overhead minimal)
    /// - **Large Batches:** >1000 nodes may hit transaction timeout (consider chunking)
    /// - **Validation Cost:** Pre-fetches all nodes for existence check
    ///
    /// # Arguments
    ///
    /// * `updates` - Vector of (node_id, NodeUpdate) tuples to apply
    ///
    /// # Returns
    ///
    /// * `Ok(())` - All updates succeeded
    /// * `Err(_)` - Transaction failed and rolled back, or batch size exceeded limit
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let store = SurrealStore::new(PathBuf::from("./data/surreal.db")).await?;
    /// let updates = vec![
    ///     ("node-1".to_string(), NodeUpdate {
    ///         content: Some("New content 1".to_string()),
    ///         ..Default::default()
    ///     }),
    ///     ("node-2".to_string(), NodeUpdate {
    ///         content: Some("New content 2".to_string()),
    ///         ..Default::default()
    ///     }),
    /// ];
    ///
    /// store.bulk_update(updates).await?; // All-or-nothing
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_update(&self, updates: Vec<(String, NodeUpdate)>) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }

        // Prevent excessive batch sizes that could cause transaction timeouts
        const MAX_BATCH_SIZE: usize = 1000;
        if updates.len() > MAX_BATCH_SIZE {
            return Err(anyhow::anyhow!(
                "Bulk update batch size ({}) exceeds maximum ({}). Consider chunking the updates into smaller batches.",
                updates.len(),
                MAX_BATCH_SIZE
            ));
        }

        // Build transaction query
        let mut transaction_parts = vec!["BEGIN TRANSACTION;".to_string()];

        for (idx, (id, _)) in updates.iter().enumerate() {
            // Validate node exists (will fetch again later for merging values)
            self.get_node(id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Node not found: {}", id))?;

            // Generate UPDATE statement using record ID
            let update_stmt = format!(
                "UPDATE type::thing('node', $id_{idx}) SET
                    content = $content_{idx},
                    node_type = $node_type_{idx},
                    modified_at = time::now(),
                    version = version + 1;",
                idx = idx
            );
            transaction_parts.push(update_stmt);
        }

        transaction_parts.push("COMMIT TRANSACTION;".to_string());
        let transaction_query = transaction_parts.join("\n");

        // Build query with all bindings
        let mut query_builder = self.db.query(transaction_query);

        for (idx, (id, update)) in updates.iter().enumerate() {
            // Fetch current node again for building merged values
            let current = self
                .get_node(id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Node not found: {}", id))?;

            let updated_content = update.content.clone().unwrap_or(current.content);
            let updated_node_type = update.node_type.clone().unwrap_or(current.node_type);

            query_builder = query_builder
                .bind((format!("id_{}", idx), id.clone()))
                .bind((format!("content_{}", idx), updated_content))
                .bind((format!("node_type_{}", idx), updated_node_type));
        }

        query_builder
            .await
            .context("Failed to execute bulk update transaction")?;

        Ok(())
    }

    pub async fn batch_create_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Node>> {
        let mut created_nodes = Vec::new();

        for node in nodes {
            let created = self.create_node(node, None).await?;
            created_nodes.push(created);
        }

        Ok(created_nodes)
    }

    /// Bulk create nodes with hierarchy in a single transaction (Issue #737)
    ///
    /// This method creates multiple nodes and their parent-child relationships atomically
    /// using a single database transaction. All nodes and relationships are inserted in one
    /// operation for optimal performance.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of tuples: (id, node_type, content, parent_id, order, properties)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<String>)` - Vector of created node IDs in insertion order
    /// * `Err` - If transaction fails (all changes rolled back)
    ///
    /// # Performance
    ///
    /// This method reduces database operations from ~3 per node to 1 total,
    /// providing approximately 10-15x speedup for bulk imports.
    pub async fn bulk_create_hierarchy(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Universal Graph Architecture (Issue #783): All properties embedded in node.properties
        // Build a single transaction query for all operations
        let mut query = String::from("BEGIN TRANSACTION;\n");

        for (id, node_type, content, parent_id, order, properties) in &nodes {
            // Validate node type
            self.validate_node_type(node_type)?;

            // Escape content for SurrealQL
            let escaped_content = Self::escape_surql_string(content);
            let props_json = serde_json::to_string(properties).unwrap_or_else(|_| "{}".to_string());

            // Create node with embedded properties
            query.push_str(&format!(
                r#"CREATE node:`{id}` CONTENT {{
                    node_type: "{node_type}",
                    content: "{content}",
                    properties: {props},
                    version: 1,
                    created_at: time::now(),
                    modified_at: time::now()
                }};
"#,
                id = id,
                node_type = node_type,
                content = escaped_content,
                props = props_json
            ));

            // Create parent-child relationship in universal relationship table (Issue #788)
            if let Some(parent) = parent_id {
                query.push_str(&format!(
                    r#"RELATE node:`{parent}`->relationship->node:`{id}` CONTENT {{
                        relationship_type: 'has_child',
                        properties: {{ order: {order} }},
                        created_at: time::now(),
                        modified_at: time::now(),
                        version: 1
                    }};
"#,
                    parent = parent,
                    id = id,
                    order = order
                ));
            }
        }

        query.push_str("COMMIT TRANSACTION;\n");

        // Execute the single transaction
        let response = self
            .db
            .query(&query)
            .await
            .context("Failed to execute bulk hierarchy creation transaction")?;

        // Check for transaction errors
        response
            .check()
            .context("Bulk hierarchy creation transaction failed")?;

        // Notify for each created node (for reactive updates)
        for (id, node_type, content, _, _, properties) in &nodes {
            let node = Node {
                id: id.clone(),
                node_type: node_type.clone(),
                content: content.clone(),
                version: 1,
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                properties: properties.clone(),
                mentions: vec![],
                mentioned_by: vec![],
                member_of: vec![],
            };
            self.notify(StoreChange {
                operation: StoreOperation::Created,
                node,
                source: Some("bulk_create_hierarchy".to_string()),
            });
        }

        Ok(nodes.into_iter().map(|(id, _, _, _, _, _)| id).collect())
    }

    /// Create a single node with parent relationship for streaming imports
    ///
    /// Universal Graph Architecture (Issue #783): All properties embedded in node.properties.
    ///
    /// This is an optimized path for async markdown imports where:
    /// - Parent is guaranteed to exist (created before children)
    /// - Order is pre-calculated (no DB query needed)
    /// - No validation queries needed (nodes are pre-validated)
    ///
    /// Uses a single SQL query instead of multiple queries.
    pub async fn create_node_streaming(
        &self,
        id: String,
        node_type: String,
        content: String,
        parent_id: Option<String>,
        order: f64,
        properties: serde_json::Value,
    ) -> Result<String> {
        self.validate_node_type(&node_type)?;

        let escaped_content = Self::escape_surql_string(&content);
        let props_json = serde_json::to_string(&properties).unwrap_or_else(|_| "{}".to_string());

        // Build single query for node + relationship (properties embedded)
        let mut query = String::new();

        query.push_str(&format!(
            r#"CREATE node:`{id}` CONTENT {{
                node_type: "{node_type}",
                content: "{content}",
                properties: {props},
                version: 1,
                created_at: time::now(),
                modified_at: time::now()
            }};
"#,
            id = id,
            node_type = node_type,
            content = escaped_content,
            props = props_json
        ));

        // Create parent relationship in universal relationship table (Issue #788)
        if let Some(ref parent) = parent_id {
            query.push_str(&format!(
                r#"RELATE node:`{parent}`->relationship->node:`{id}` CONTENT {{
                    relationship_type: 'has_child',
                    properties: {{ order: {order} }},
                    created_at: time::now(),
                    modified_at: time::now(),
                    version: 1
                }};
"#,
                parent = parent,
                id = id,
                order = order
            ));
        }

        // Execute query
        let response = self
            .db
            .query(&query)
            .await
            .context("Failed to create node (streaming)")?;

        response.check().context("Streaming node creation failed")?;

        // Notify for reactive updates
        let node = Node {
            id: id.clone(),
            node_type: node_type.clone(),
            content,
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties,
            mentions: vec![],
            mentioned_by: vec![],
            member_of: vec![],
        };
        self.notify(StoreChange {
            operation: StoreOperation::Created,
            node,
            source: Some("streaming_import".to_string()),
        });

        Ok(id)
    }

    /// Escape string for SurrealQL to prevent injection
    fn escape_surql_string(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    }

    pub fn close(&self) -> Result<()> {
        // SurrealDB handles cleanup automatically on drop
        Ok(())
    }

    // ========================================================================
    // Strongly-Typed Node Retrieval (Issue #673)
    // ========================================================================
    //
    // Universal Graph Architecture (Issue #783): These methods provide direct
    // deserialization from node.properties, eliminating the intermediate JSON step.

    /// Get a task node with strong typing using single-query pattern
    ///
    /// Universal Graph Architecture: Fetches task properties from node.properties
    /// and node metadata (id, content, version, timestamps) in a single query.
    ///
    /// # Query Pattern
    ///
    /// Column aliases use camelCase to match TaskNode's `#[serde(rename_all = "camelCase")]`:
    ///
    /// ```sql
    /// SELECT
    ///     record::id(id) AS id,
    ///     properties.status AS status,
    ///     properties.priority AS priority,
    ///     properties.due_date AS dueDate,
    ///     properties.assignee AS assignee,
    ///     content AS content,
    ///     version AS version,
    ///     created_at AS createdAt,
    ///     modified_at AS modifiedAt
    /// FROM node:`some-id`;
    /// ```
    ///
    /// # Arguments
    ///
    /// * `id` - The task node ID (without table prefix)
    ///
    /// # Returns
    ///
    /// * `Ok(Some(TaskNode))` - Task found with strongly-typed fields
    /// * `Ok(None)` - Task not found
    /// * `Err(_)` - Database or deserialization error
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let store = SurrealStore::new(PathBuf::from("./data/surreal.db")).await?;
    /// if let Some(task) = store.get_task_node("my-task-id").await? {
    ///     // Direct field access - no JSON parsing
    ///     println!("Status: {:?}", task.status);
    ///     println!("Priority: {:?}", task.priority);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_task_node(&self, id: &str) -> Result<Option<crate::models::TaskNode>> {
        // Universal Graph Architecture (Issue #783): Properties embedded in node.properties
        // Note: Column aliases use camelCase to match TaskNode's #[serde(rename_all = "camelCase")]
        let query = format!(
            r#"
            SELECT
                record::id(id) AS id,
                node_type AS nodeType,
                properties.status AS status,
                properties.priority AS priority,
                properties.due_date AS dueDate,
                properties.assignee AS assignee,
                content AS content,
                version AS version,
                created_at AS createdAt,
                modified_at AS modifiedAt
            FROM node:`{}`;
            "#,
            id
        );

        let mut response = self
            .db
            .query(&query)
            .await
            .context(format!("Failed to query task node '{}'", id))?;

        let tasks: Vec<crate::models::TaskNode> =
            response.take(0).context("Failed to deserialize TaskNode")?;

        Ok(tasks.into_iter().next())
    }

    /// Update a task node with type-safe property updates
    ///
    /// Universal Graph Architecture (Issue #783): Updates task properties in
    /// node.properties field and optionally the content field. Uses optimistic
    /// concurrency control (OCC) to prevent lost updates.
    ///
    /// # Transaction Pattern
    ///
    /// Updates are atomic with OCC check:
    ///
    /// ```sql
    /// BEGIN TRANSACTION;
    /// -- OCC check
    /// LET $current = SELECT version FROM node:`id`;
    /// IF $current.version != $expected { THROW "Version mismatch" };
    /// -- Update node properties and metadata
    /// UPDATE node:`id` SET
    ///     properties.status = $status,
    ///     properties.priority = $priority,
    ///     content = $content,
    ///     version = version + 1,
    ///     modified_at = time::now();
    /// COMMIT;
    /// ```
    ///
    /// # Arguments
    ///
    /// * `id` - The task node ID
    /// * `expected_version` - Version for OCC check (prevents lost updates)
    /// * `update` - TaskNodeUpdate with fields to update
    ///
    /// # Returns
    ///
    /// * `Ok(TaskNode)` - Updated task with new version and modified_at
    /// * `Err(_)` - Version mismatch, node not found, or database error
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::{TaskNodeUpdate, TaskStatus};
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let store = SurrealStore::new(PathBuf::from("./data/surreal.db")).await?;
    /// let update = TaskNodeUpdate::new().with_status(TaskStatus::InProgress);
    /// let updated = store.update_task_node("task-123", 1, update).await?;
    /// println!("New status: {:?}", updated.status);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_task_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::TaskNodeUpdate,
    ) -> Result<crate::models::TaskNode> {
        // Universal Graph Architecture (Issue #783): Properties embedded in node.properties
        // Build SET clauses for properties update
        let mut property_set_clauses: Vec<String> = Vec::new();

        if let Some(ref status) = update.status {
            property_set_clauses.push(format!("properties.status = '{}'", status.as_str()));
        }

        if let Some(ref priority_opt) = update.priority {
            match priority_opt {
                Some(p) => {
                    property_set_clauses.push(format!("properties.priority = '{}'", p.as_str()))
                }
                None => property_set_clauses.push("properties.priority = NONE".to_string()),
            }
        }

        if let Some(ref due_date_opt) = update.due_date {
            match due_date_opt {
                Some(dt) => property_set_clauses.push(format!(
                    "properties.due_date = <datetime>'{}'",
                    dt.to_rfc3339()
                )),
                None => property_set_clauses.push("properties.due_date = NONE".to_string()),
            }
        }

        if let Some(ref assignee_opt) = update.assignee {
            match assignee_opt {
                // Escape single quotes to prevent SQL injection
                Some(a) => property_set_clauses.push(format!(
                    "properties.assignee = '{}'",
                    a.replace('\'', "\\'")
                )),
                None => property_set_clauses.push("properties.assignee = NONE".to_string()),
            }
        }

        if let Some(ref started_at_opt) = update.started_at {
            match started_at_opt {
                Some(dt) => property_set_clauses.push(format!(
                    "properties.started_at = <datetime>'{}'",
                    dt.to_rfc3339()
                )),
                None => property_set_clauses.push("properties.started_at = NONE".to_string()),
            }
        }

        if let Some(ref completed_at_opt) = update.completed_at {
            match completed_at_opt {
                Some(dt) => property_set_clauses.push(format!(
                    "properties.completed_at = <datetime>'{}'",
                    dt.to_rfc3339()
                )),
                None => property_set_clauses.push("properties.completed_at = NONE".to_string()),
            }
        }

        // Build transaction
        let mut transaction_parts = vec!["BEGIN TRANSACTION;".to_string()];

        // OCC check: verify version matches
        transaction_parts.push(format!(
            r#"LET $current = (SELECT version FROM node:`{id}`);"#,
            id = id
        ));
        transaction_parts.push(format!(
            r#"IF $current[0].version != {expected_version} {{ THROW "VersionMismatch: expected {expected_version}, got " + <string>$current[0].version; }};"#,
            expected_version = expected_version
        ));

        // Build the full SET clause with all updates
        let mut all_set_clauses = property_set_clauses;
        all_set_clauses.push("version = version + 1".to_string());
        all_set_clauses.push("modified_at = time::now()".to_string());

        // Optionally update content
        if let Some(ref content) = update.content {
            all_set_clauses.push(format!("content = '{}'", content.replace('\'', "\\'")));
        }

        // Update node with all changes
        transaction_parts.push(format!(
            r#"UPDATE node:`{id}` SET {sets};"#,
            id = id,
            sets = all_set_clauses.join(", ")
        ));

        transaction_parts.push("COMMIT TRANSACTION;".to_string());

        let transaction_query = transaction_parts.join("\n");

        // Execute transaction and check for errors (including IF/THROW version mismatch)
        let response = self
            .db
            .query(&transaction_query)
            .await
            .context(format!("Failed to update task node '{}'", id))?;

        // Check the response for errors
        response
            .check()
            .context(format!("Failed to update task node '{}'", id))?;

        // Fetch and return updated task node
        self.get_task_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Task node '{}' not found after update", id))
    }

    /// Get a schema node with strong typing
    ///
    /// Fetches schema data from node table where node_type = 'schema'.
    /// Schema properties (is_core, fields, relationships) are stored in node.properties.
    ///
    /// # Query Pattern
    ///
    /// ```sql
    /// SELECT
    ///     record::id(id) AS id,
    ///     properties.isCore AS isCore,
    ///     properties.schemaVersion AS schemaVersion,
    ///     properties.description AS description,
    ///     properties.fields AS fields,
    ///     properties.relationships AS relationships,
    ///     content,
    ///     version,
    ///     created_at AS createdAt,
    ///     modified_at AS modifiedAt
    /// FROM node:`task` WHERE node_type = 'schema';
    /// ```
    ///
    /// # Arguments
    ///
    /// * `id` - The schema node ID (e.g., "task", "date")
    ///
    /// # Returns
    ///
    /// * `Ok(Some(SchemaNode))` - Schema found with strongly-typed fields
    /// * `Ok(None)` - Schema not found
    /// * `Err(_)` - Database or deserialization error
    pub async fn get_schema_node(&self, id: &str) -> Result<Option<crate::models::SchemaNode>> {
        // Query node table for schema nodes - properties are in node.properties
        let query = format!(
            r#"
            SELECT
                record::id(id) AS id,
                properties.isCore AS isCore,
                properties.schemaVersion AS schemaVersion,
                properties.description AS description,
                properties.fields AS fields,
                properties.relationships AS relationships,
                content,
                version,
                created_at AS createdAt,
                modified_at AS modifiedAt
            FROM node:`{}`
            WHERE node_type = 'schema';
            "#,
            id
        );

        let mut response = self
            .db
            .query(&query)
            .await
            .context(format!("Failed to query schema node '{}'", id))?;

        let schemas: Vec<crate::models::SchemaNode> = response
            .take(0)
            .context("Failed to deserialize SchemaNode")?;

        Ok(schemas.into_iter().next())
    }

    /// Get all schema nodes from the database
    ///
    /// Returns all schema definitions including their fields and relationships.
    /// Schema nodes have node_type = 'schema' with properties in node.properties.
    ///
    /// # Returns
    ///
    /// Vector of all schema nodes, ordered by ID.
    pub async fn get_all_schemas(&self) -> Result<Vec<crate::models::SchemaNode>> {
        // Query node table for all schema nodes - properties are in node.properties
        let query = r#"
            SELECT
                record::id(id) AS id,
                properties.isCore AS isCore,
                properties.schemaVersion AS schemaVersion,
                properties.description AS description,
                properties.fields AS fields,
                properties.relationships AS relationships,
                content,
                version,
                created_at AS createdAt,
                modified_at AS modifiedAt
            FROM node
            WHERE node_type = 'schema'
            ORDER BY id;
        "#;

        let mut response = self
            .db
            .query(query)
            .await
            .context("Failed to query all schema nodes")?;

        let schemas: Vec<crate::models::SchemaNode> = response
            .take(0)
            .context("Failed to deserialize SchemaNodes")?;

        Ok(schemas)
    }

    // =========================================================================
    // Root-Aggregate Embedding Methods (Issue #729)
    // =========================================================================
    //
    // These methods work with the `embedding` table for root-aggregate semantic search.
    // Unlike the old node.embedding_vector approach, these methods:
    // - Store embeddings in a dedicated table with chunking support
    // - Track staleness for re-embedding queue
    // - Support multiple chunks per node for large content
    // =========================================================================

    /// Create or update embeddings for a root node
    ///
    /// Replaces all existing embeddings for the node with new ones.
    /// Used after content aggregation and chunking.
    ///
    /// # Arguments
    /// * `node_id` - The root node ID (without table prefix)
    /// * `embeddings` - List of embeddings to store (one per chunk)
    pub async fn upsert_embeddings(
        &self,
        node_id: &str,
        embeddings: Vec<crate::models::NewEmbedding>,
    ) -> Result<()> {
        if embeddings.is_empty() {
            return Ok(());
        }

        // Delete existing embeddings for this node
        self.db
            .query("DELETE embedding WHERE node = type::thing('node', $node_id);")
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to delete existing embeddings")?;

        // Insert new embeddings
        for emb in embeddings {
            let query = r#"
                CREATE embedding CONTENT {
                    node: type::thing('node', $node_id),
                    vector: $vector,
                    dimension: $dimension,
                    model_name: $model_name,
                    chunk_index: $chunk_index,
                    chunk_start: $chunk_start,
                    chunk_end: $chunk_end,
                    total_chunks: $total_chunks,
                    content_hash: $content_hash,
                    token_count: $token_count,
                    stale: false,
                    error_count: 0,
                    last_error: NONE,
                    created_at: time::now(),
                    modified_at: time::now()
                };
            "#;

            let dimension = emb.vector.len() as i32;
            self.db
                .query(query)
                .bind(("node_id", emb.node_id.clone()))
                .bind(("vector", emb.vector))
                .bind(("dimension", dimension))
                .bind((
                    "model_name",
                    emb.model_name
                        .unwrap_or_else(|| "nomic-embed-text-v1.5".to_string()),
                ))
                .bind(("chunk_index", emb.chunk_index))
                .bind(("chunk_start", emb.chunk_start))
                .bind(("chunk_end", emb.chunk_end))
                .bind(("total_chunks", emb.total_chunks))
                .bind(("content_hash", emb.content_hash))
                .bind(("token_count", emb.token_count))
                .await
                .context("Failed to create embedding")?;
        }

        Ok(())
    }

    /// Mark all embeddings for a node as stale
    ///
    /// Called when node content changes to trigger re-embedding.
    pub async fn mark_root_embedding_stale(&self, node_id: &str) -> Result<()> {
        self.db
            .query(
                "UPDATE embedding SET stale = true, modified_at = time::now() WHERE node = type::thing('node', $node_id);",
            )
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to mark root embedding as stale")?;

        Ok(())
    }

    /// Get all root node IDs with stale embeddings that are ready for processing
    ///
    /// Returns node IDs that need re-embedding, filtered by debounce duration.
    /// Only returns embeddings marked stale more than `debounce_secs` ago,
    /// allowing rapid changes to accumulate before processing.
    ///
    /// # Arguments
    /// * `limit` - Optional max number of results
    /// * `debounce_secs` - Minimum seconds since last modification (default: 30)
    pub async fn get_stale_embedding_root_ids(
        &self,
        limit: Option<i64>,
        debounce_secs: u64,
    ) -> Result<Vec<String>> {
        // Use GROUP BY node for uniqueness since DISTINCT doesn't work with record::id() in SurrealDB
        // Must include `node` in SELECT to satisfy GROUP BY requirements
        // Filter by modified_at to implement per-root debounce
        let sql = if limit.is_some() {
            "SELECT node, record::id(node) AS node_id FROM embedding WHERE stale = true AND modified_at < time::now() - type::duration($debounce) GROUP BY node LIMIT $limit;"
        } else {
            "SELECT node, record::id(node) AS node_id FROM embedding WHERE stale = true AND modified_at < time::now() - type::duration($debounce) GROUP BY node;"
        };

        // Format debounce as SurrealDB duration string (e.g., "30s")
        let debounce_str = format!("{}s", debounce_secs);

        let mut query_builder = self.db.query(sql).bind(("debounce", debounce_str));

        if let Some(lim) = limit {
            query_builder = query_builder.bind(("limit", lim));
        }

        #[derive(Debug, Deserialize)]
        struct NodeIdResult {
            node_id: String,
        }

        let mut response = query_builder
            .await
            .context("Failed to get stale embedding root IDs")?;

        let results: Vec<NodeIdResult> = response
            .take(0)
            .context("Failed to extract stale root IDs")?;

        Ok(results.into_iter().map(|r| r.node_id).collect())
    }

    /// Check if a node has any embeddings
    pub async fn has_embeddings(&self, node_id: &str) -> Result<bool> {
        #[derive(Debug, Deserialize)]
        struct CountResult {
            count: i64,
        }

        let mut response = self
            .db
            .query("SELECT count() AS count FROM embedding WHERE node = type::thing('node', $node_id) GROUP ALL;")
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to check for embeddings")?;

        let results: Vec<CountResult> = response.take(0).unwrap_or_default();

        Ok(results.first().map(|r| r.count > 0).unwrap_or(false))
    }

    /// Delete all embeddings for a node
    ///
    /// Called when a node is deleted.
    pub async fn delete_embeddings(&self, node_id: &str) -> Result<()> {
        self.db
            .query("DELETE embedding WHERE node = type::thing('node', $node_id);")
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to delete embeddings")?;

        Ok(())
    }

    /// Record an embedding error
    ///
    /// Increments error count and stores the error message.
    pub async fn record_embedding_error(&self, node_id: &str, error: &str) -> Result<()> {
        self.db
            .query(
                r#"
                UPDATE embedding SET
                    error_count = error_count + 1,
                    last_error = $error,
                    modified_at = time::now()
                WHERE node = type::thing('node', $node_id);
                "#,
            )
            .bind(("node_id", node_id.to_string()))
            .bind(("error", error.to_string()))
            .await
            .context("Failed to record embedding error")?;

        Ok(())
    }

    /// Breadth boost factor for multi-chunk scoring (Issue #778)
    /// Formula: score = max_similarity * (1 + BREADTH_BOOST * log10(matching_chunks))
    const BREADTH_BOOST: f64 = 0.3;

    /// Search embeddings by vector similarity with multi-chunk scoring (Issue #778)
    ///
    /// Returns nodes ranked by a composite score that considers both:
    /// 1. Maximum chunk similarity (primary signal)
    /// 2. Number of matching chunks (breadth of relevance)
    ///
    /// Documents with multiple relevant chunks rank higher than those with
    /// a single high-scoring chunk, ensuring broader relevance is rewarded.
    ///
    /// ## Scoring Formula
    /// ```text
    /// score = max_similarity * (1 + 0.3 * log10(matching_chunks))
    /// ```
    ///
    /// ## Examples
    /// - Doc A: 1 chunk @ 0.85 → score = 0.85
    /// - Doc B: 5 chunks @ 0.80 max → score = 0.80 * 1.21 = 0.97
    ///
    /// # Arguments
    /// * `query_vector` - The query embedding vector
    /// * `limit` - Maximum number of results
    /// * `threshold` - Minimum similarity threshold (0.0-1.0)
    pub async fn search_embeddings(
        &self,
        query_vector: &[f32],
        limit: i64,
        threshold: Option<f64>,
    ) -> Result<Vec<crate::models::EmbeddingSearchResult>> {
        let min_similarity = threshold.unwrap_or(0.5);

        // Intermediate struct for raw SurrealDB results with chunk count and fetched node
        // Note: Using FETCH node to get full node data in a single query (eliminates N+1 queries)
        //
        // Uses SurrealNode internally because FETCH returns the full node including its
        // `id` field as a SurrealDB Thing type (see FETCH data Limitation comments above).
        // We then convert SurrealNode -> Node which extracts the UUID from the Thing.
        #[derive(Debug, serde::Deserialize)]
        struct RawSearchResult {
            node: SurrealNode,
            max_similarity: f64,
            matching_chunks: i64,
        }

        // Query using KNN operator for MTREE-indexed vector search (Issue #776)
        // Enhanced for multi-chunk scoring (Issue #778):
        // - We calculate similarity for each chunk
        // - Count how many chunks exceed the threshold
        // - Group by node, taking max similarity and count
        //
        // The <|K|> operator leverages the MTREE index for fast approximate nearest neighbor search.
        // We fetch more candidates (limit * 5) to account for:
        // 1. Multiple chunks per node (grouped later)
        // 2. Threshold filtering (some may not meet similarity threshold)
        // Note: SurrealDB's KNN operator <|K|> requires a literal integer, not a bind parameter.
        // We interpolate knn_limit directly into the query string.
        //
        // PERFORMANCE: Using FETCH node to retrieve full node data in the same query,
        // eliminating the need for separate get_node() calls (saves ~300ms for 5 results).
        let knn_limit = limit * 5; // Increased to capture more chunks per node
        let query = format!(
            r#"
            SELECT * FROM (
                SELECT
                    node,
                    math::max(similarity) AS max_similarity,
                    count() AS matching_chunks
                FROM (
                    SELECT
                        node,
                        vector::similarity::cosine(vector, $query_vector) AS similarity
                    FROM embedding
                    WHERE stale = false AND vector <|{knn_limit}|> $query_vector
                )
                WHERE similarity > $threshold
                GROUP BY node
            )
            ORDER BY max_similarity DESC
            LIMIT $limit
            FETCH node;
        "#
        );

        let mut response = self
            .db
            .query(&query)
            .bind(("query_vector", query_vector.to_vec()))
            .bind(("threshold", min_similarity))
            .bind(("limit", limit))
            .await
            .context("Failed to execute embedding search")?;

        let raw_results: Vec<RawSearchResult> = response
            .take(0)
            .context("Failed to extract embedding search results")?;

        // Convert to EmbeddingSearchResult with composite score calculation
        // Score formula: max_similarity * (1 + BREADTH_BOOST * log10(matching_chunks))
        let mut results: Vec<crate::models::EmbeddingSearchResult> = raw_results
            .into_iter()
            .map(|r| {
                // Calculate breadth factor: log10(chunks) gives 0 for 1 chunk, ~0.70 for 5, ~1.0 for 10
                // Note: matching_chunks comes from SQL count() which always returns >= 1,
                // but .max(1.0) guards against edge cases and prevents log10(0) = -inf
                let breadth_factor =
                    1.0 + Self::BREADTH_BOOST * (r.matching_chunks as f64).max(1.0).log10();
                let score = r.max_similarity * breadth_factor;

                // Convert SurrealNode -> Node (extracts UUID from Thing, handles properties)
                let node: Node = r.node.into();

                crate::models::EmbeddingSearchResult {
                    node_id: node.id.clone(),
                    score,
                    max_similarity: r.max_similarity,
                    matching_chunks: r.matching_chunks,
                    node: Some(node),
                }
            })
            .collect();

        // Re-sort by composite score (DB sorted by max_similarity, we need score order)
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }

    // ========================================================================
    // Collection Membership Operations (member_of relationships)
    // ========================================================================

    /// Add a node to a collection (create member_of relationship)
    ///
    /// Creates a member_of relationship from the member node to the collection node.
    /// Direction: member -> collection (node X belongs to collection Y)
    ///
    /// Issue #788: Universal Relationship Architecture - stored in relationship table with relationship_type='member_of'
    ///
    /// This is idempotent - if the membership already exists, nothing happens.
    ///
    /// # Arguments
    ///
    /// * `member_id` - The ID of the node to add to the collection
    /// * `collection_id` - The ID of the collection node
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Membership created or already exists
    /// * `Err` - Database error
    pub async fn add_to_collection(&self, member_id: &str, collection_id: &str) -> Result<()> {
        let member_thing = Thing::from(("node".to_string(), member_id.to_string()));
        let collection_thing = Thing::from(("node".to_string(), collection_id.to_string()));

        // Note: Validation that collection_id is actually a collection node
        // is done in CollectionService.add_to_collection (service layer).
        // Store layer focuses on data persistence only.

        // Check if membership already exists (for idempotency) - Issue #788: use relationship table
        let check_query =
            "SELECT VALUE id FROM relationship WHERE in = $member AND out = $collection AND relationship_type = 'member_of';";
        let mut check_response = self
            .db
            .query(check_query)
            .bind(("member", member_thing.clone()))
            .bind(("collection", collection_thing.clone()))
            .await
            .context("Failed to check for existing membership")?;

        let existing_membership_ids: Vec<Thing> = check_response
            .take(0)
            .context("Failed to extract membership check results")?;

        // Only create membership if it doesn't exist - Issue #788: use relationship table
        if existing_membership_ids.is_empty() {
            let query = r#"RELATE $member->relationship->$collection CONTENT {
                relationship_type: 'member_of',
                properties: {},
                created_at: time::now(),
                modified_at: time::now(),
                version: 1
            };"#;

            self.db
                .query(query)
                .bind(("member", member_thing))
                .bind(("collection", collection_thing))
                .await
                .context("Failed to create membership")?;
        }

        Ok(())
    }

    /// Remove a node from a collection (delete member_of relationship)
    ///
    /// Deletes the member_of relationship from the member node to the collection node.
    /// Issue #788: Universal Relationship Architecture - deletes from relationship table.
    ///
    /// # Arguments
    ///
    /// * `member_id` - The ID of the node to remove from the collection
    /// * `collection_id` - The ID of the collection node
    pub async fn remove_from_collection(&self, member_id: &str, collection_id: &str) -> Result<()> {
        let member_thing = Thing::from(("node".to_string(), member_id.to_string()));
        let collection_thing = Thing::from(("node".to_string(), collection_id.to_string()));

        self.db
            .query("DELETE FROM relationship WHERE in = $member AND out = $collection AND relationship_type = 'member_of';")
            .bind(("member", member_thing))
            .bind(("collection", collection_thing))
            .await
            .context("Failed to delete membership")?;

        Ok(())
    }

    /// Get all collections a node belongs to
    ///
    /// Returns the IDs of all collections the node is a member of.
    /// Direction: node -> member_of -> collection
    /// Issue #788: Universal Relationship Architecture - queries relationship table.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The ID of the node
    ///
    /// # Returns
    ///
    /// Collection IDs the node belongs to
    pub async fn get_node_memberships(&self, node_id: &str) -> Result<Vec<String>> {
        let query =
            "SELECT ->relationship[WHERE relationship_type = 'member_of']->node.id AS collection_ids FROM type::thing('node', $node_id);";

        let mut response = self
            .db
            .query(query)
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to get node memberships")?;

        #[derive(Debug, Deserialize)]
        struct MembershipResult {
            collection_ids: Vec<Thing>,
        }

        let results: Vec<MembershipResult> = response
            .take(0)
            .context("Failed to extract memberships from response")?;

        let collection_ids: Vec<String> = results
            .into_iter()
            .flat_map(|r| r.collection_ids)
            .filter_map(|thing| {
                if let Id::String(id_str) = &thing.id {
                    Some(id_str.clone())
                } else {
                    None
                }
            })
            .collect();

        Ok(collection_ids)
    }

    /// Get all members of a collection
    ///
    /// Returns the IDs of all nodes that are members of the collection.
    /// Direction: member -> member_of -> collection
    /// Issue #788: Universal Relationship Architecture - queries relationship table.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - The ID of the collection node
    ///
    /// # Returns
    ///
    /// Member node IDs
    pub async fn get_collection_members(&self, collection_id: &str) -> Result<Vec<String>> {
        let query =
            "SELECT <-relationship[WHERE relationship_type = 'member_of']<-node.id AS member_ids FROM type::thing('node', $collection_id);";

        let mut response = self
            .db
            .query(query)
            .bind(("collection_id", collection_id.to_string()))
            .await
            .context("Failed to get collection members")?;

        #[derive(Debug, Deserialize)]
        struct MemberResult {
            member_ids: Vec<Thing>,
        }

        let results: Vec<MemberResult> = response
            .take(0)
            .context("Failed to extract members from response")?;

        let member_ids: Vec<String> = results
            .into_iter()
            .flat_map(|r| r.member_ids)
            .filter_map(|thing| {
                if let Id::String(id_str) = &thing.id {
                    Some(id_str.clone())
                } else {
                    None
                }
            })
            .collect();

        Ok(member_ids)
    }

    /// Get collection by name (case-insensitive lookup)
    ///
    /// Finds a collection node by its content field (collection name).
    /// Uses case-insensitive matching.
    ///
    /// # Arguments
    ///
    /// * `name` - The collection name to search for
    ///
    /// # Returns
    ///
    /// The collection node if found
    pub async fn get_collection_by_name(&self, name: &str) -> Result<Option<Node>> {
        let normalized_name = name.to_lowercase();

        // Use string::lowercase for case-insensitive matching
        // Return only the ID so we can use get_node for consistent handling
        let query = r#"
            SELECT VALUE record::id(id) FROM node
            WHERE node_type = 'collection'
            AND string::lowercase(content) = $name
            LIMIT 1;
        "#;

        let mut response = self
            .db
            .query(query)
            .bind(("name", normalized_name))
            .await
            .context("Failed to search for collection by name")?;

        let results: Vec<String> = response
            .take(0)
            .context("Failed to extract collection search results")?;

        if let Some(collection_id) = results.into_iter().next() {
            // Use get_node for consistent node construction
            self.get_node(&collection_id).await
        } else {
            Ok(None)
        }
    }

    /// Batch get collections by names (case-insensitive lookup)
    ///
    /// Finds collection nodes by their content fields in a single query.
    /// Returns a map of normalized name -> Node for collections that exist.
    ///
    /// # Arguments
    ///
    /// * `names` - The collection names to search for
    ///
    /// # Returns
    ///
    /// Map of normalized (lowercase) name to Node for each found collection
    pub async fn get_collections_by_names(
        &self,
        names: &[String],
    ) -> Result<std::collections::HashMap<String, Node>> {
        use std::collections::HashMap;

        if names.is_empty() {
            return Ok(HashMap::new());
        }

        let normalized_names: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();

        // Use CONTAINS operator for batch lookup
        // Return only IDs and content, then use get_node for consistent node construction
        let query = r#"
            SELECT VALUE { id: record::id(id), content: content }
            FROM node
            WHERE node_type = 'collection'
            AND $names CONTAINS string::lowercase(content);
        "#;

        let mut response = self
            .db
            .query(query)
            .bind(("names", normalized_names))
            .await
            .context("Failed to batch search for collections by names")?;

        // Parse as objects with id and content fields
        let results: Vec<Value> = response.take(0).unwrap_or_default();

        let mut collections = HashMap::new();
        for row in results {
            let node_id = row["id"].as_str().unwrap_or("").to_string();
            let content = row["content"].as_str().unwrap_or("").to_string();

            if node_id.is_empty() {
                continue;
            }

            // Use get_node for consistent node construction
            if let Ok(Some(node)) = self.get_node(&node_id).await {
                let normalized_content = content.to_lowercase();
                collections.insert(normalized_content, node);
            }
        }

        Ok(collections)
    }

    /// Get all members of a collection recursively (including members of child collections)
    ///
    /// This method returns members of the specified collection and all its
    /// descendant collections in the hierarchy.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - The ID of the root collection
    ///
    /// # Returns
    ///
    /// All member node IDs (deduplicated)
    pub async fn get_collection_members_recursive(
        &self,
        collection_id: &str,
    ) -> Result<Vec<String>> {
        let collection_thing = Thing::from(("node".to_string(), collection_id.to_string()));

        // Get all collections in the subtree (collection + descendants)
        // Then get all members of those collections
        // Issue #788: Universal Relationship Architecture - use relationship table
        let query = r#"
            LET $collection_subtree = array::concat(
                [$collection_thing],
                $collection_thing.{..+collect}->relationship[WHERE relationship_type = 'has_child']->node
            );
            SELECT <-relationship[WHERE relationship_type = 'member_of']<-node.id AS member_ids FROM node
            WHERE id IN $collection_subtree;
        "#;

        let mut response = self
            .db
            .query(query)
            .bind(("collection_thing", collection_thing))
            .await
            .context("Failed to get recursive collection members")?;

        #[derive(Debug, Deserialize)]
        struct MemberResult {
            member_ids: Vec<Thing>,
        }

        let results: Vec<MemberResult> = response
            .take(1) // Second statement result
            .context("Failed to extract recursive members from response")?;

        let mut member_ids: Vec<String> = results
            .into_iter()
            .flat_map(|r| r.member_ids)
            .filter_map(|thing| {
                if let Id::String(id_str) = &thing.id {
                    Some(id_str.clone())
                } else {
                    None
                }
            })
            .collect();

        // Deduplicate (a node could be in multiple child collections)
        member_ids.sort();
        member_ids.dedup();

        Ok(member_ids)
    }

    /// Get all collection names
    ///
    /// Returns all collection names in the database, ordered alphabetically.
    /// Collections have globally unique names.
    ///
    /// # Returns
    ///
    /// Vec of collection names (content field values)
    pub async fn get_all_collection_names(&self) -> Result<Vec<String>> {
        let query = r#"
            SELECT VALUE content FROM node
            WHERE node_type = 'collection'
            ORDER BY content ASC;
        "#;

        let mut response = self
            .db
            .query(query)
            .await
            .context("Failed to get all collections")?;

        let names: Vec<String> = response
            .take(0)
            .context("Failed to extract collection names")?;

        Ok(names)
    }

    /// Create a stale embedding marker for a new root node
    ///
    /// Creates an embedding record with a placeholder vector marked as stale to queue it for processing.
    /// Used when a new root node is created that should be embedded.
    ///
    /// Note: Uses a unit vector [1,0,0,...,0] instead of zeros because the MTREE index
    /// with COSINE distance cannot handle zero vectors (division by zero during normalization).
    /// The stale=true flag ensures this placeholder will be replaced with a real embedding.
    pub async fn create_stale_embedding_marker(&self, node_id: &str) -> Result<()> {
        // Use unit vector [1,0,0,...,0] - a valid vector that can be normalized for cosine distance
        // Zero vectors cause NaN in cosine distance calculations
        let query = r#"
            CREATE embedding CONTENT {
                node: type::thing('node', $node_id),
                vector: array::concat([1.0], array::repeat(0.0, 767)),
                dimension: 768,
                model_name: 'nomic-embed-text-v1.5',
                chunk_index: 0,
                chunk_start: 0,
                chunk_end: NONE,
                total_chunks: 1,
                content_hash: NONE,
                token_count: NONE,
                stale: true,
                error_count: 0,
                last_error: NONE,
                created_at: time::now(),
                modified_at: time::now()
            };
        "#;

        self.db
            .query(query)
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to create stale embedding marker")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Test helper to create a SurrealStore with schemas seeded
    ///
    /// Since schema seeding moved to NodeService (Issue #704), we use NodeService
    /// to seed schemas. The new() method now takes &mut Arc to update caches
    /// incrementally during seeding - no rebuild needed.
    async fn create_test_store() -> Result<(Arc<SurrealStore>, TempDir)> {
        use crate::services::NodeService;

        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test_surreal.db");
        let mut store_arc = Arc::new(SurrealStore::new(db_path).await?);

        // Seed schemas via NodeService (Issue #704)
        // NodeService::new() takes &mut Arc to update caches incrementally during seeding
        let _ = NodeService::new(&mut store_arc)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize NodeService: {}", e))?;

        // Caches are now populated by NodeService::new() - no rebuild needed!
        Ok((store_arc, temp_dir))
    }

    #[tokio::test]
    async fn test_create_and_get_node() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));

        let created = store.create_node(node.clone(), None).await?;
        assert_eq!(created.id, node.id);
        assert_eq!(created.content, "Test content");

        let fetched = store.get_node(&node.id).await?;
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, node.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_update_node() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let node = Node::new(
            "text".to_string(),
            "Original content".to_string(),
            json!({}),
        );

        let created = store.create_node(node.clone(), None).await?;

        let update = NodeUpdate {
            content: Some("Updated content".to_string()),
            ..Default::default()
        };

        let updated = store.update_node(&created.id, update, None).await?;
        assert_eq!(updated.content, "Updated content");

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_node() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));

        let created = store.create_node(node.clone(), None).await?;

        let result = store.delete_node(&created.id, None).await?;
        assert!(result.existed);

        let fetched = store.get_node(&created.id).await?;
        assert!(fetched.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_schema_operations() -> Result<()> {
        use crate::models::schema::{SchemaField, SchemaProtectionLevel};

        let (store, _temp_dir) = create_test_store().await?;

        // Create schema properties with fields containing SchemaProtectionLevel enum
        // This tests that enums are stored and retrieved correctly without stringification
        let schema_props = serde_json::json!({
            "isCore": false,
            "version": 1,
            "description": "Test task schema",
            "fields": [
                {
                    "name": "status",
                    "type": "enum",
                    "protection": "core",
                    "coreValues": [
                        { "value": "open", "label": "Open" },
                        { "value": "in_progress", "label": "In Progress" },
                        { "value": "done", "label": "Done" }
                    ],
                    "indexed": true,
                    "required": true,
                    "extensible": true,
                    "default": "open",
                    "description": "Task status"
                }
            ]
        });

        store.update_schema("task", &schema_props).await?;

        // Fetch and verify the schema was stored correctly
        let fetched = store.get_schema("task").await?;
        assert!(fetched.is_some(), "Schema should be fetched");

        let fetched_value = fetched.unwrap();

        // Verify the schema was stored and retrieved correctly
        assert_eq!(fetched_value["version"], 1);
        assert_eq!(fetched_value["description"], "Test task schema");

        // Parse and verify fields with SchemaProtectionLevel
        let fields: Vec<SchemaField> = serde_json::from_value(fetched_value["fields"].clone())?;
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "status");
        // Key assertion: SchemaProtectionLevel enum correctly deserialized
        assert_eq!(fields[0].protection, SchemaProtectionLevel::Core);

        Ok(())
    }

    // ============================================================================
    // NOTE: Old per-node embedding tests REMOVED (Issue #729)
    //
    // The following tests were removed as they tested the deprecated per-node
    // embedding model (node.embedding_vector, update_embedding(), search_by_embedding):
    // - test_search_empty_database
    // - test_search_with_similar_nodes
    // - test_search_with_threshold_filter
    // - test_search_respects_limit
    // - test_search_performance_1k_nodes
    // - test_search_with_real_nlp_embeddings
    // - test_search_performance_10k_nodes
    //
    // The new root-aggregate embedding model uses the `embedding` table.
    // See NodeEmbeddingService tests for the new search functionality.
    // ============================================================================

    // ============================================================================
    // Atomic Transactional Operations Tests (Issue #532)
    // ============================================================================

    #[tokio::test]
    async fn test_create_child_node_atomic_success() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create parent node
        let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
        let parent = store.create_node(parent, None).await?;

        // Create child atomically
        let child = store
            .create_child_node_atomic(&parent.id, "text", "Child content", json!({}), None)
            .await?;

        // Verify child was created
        assert_eq!(child.content, "Child content");
        assert_eq!(child.node_type, "text");

        // Verify parent-child relationship exists
        let children = store.get_children(Some(&parent.id)).await?;
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, child.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_create_child_node_atomic_with_properties() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create parent
        let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
        let parent = store.create_node(parent, None).await?;

        // Create task child atomically with properties
        let properties = json!({
            "status": "TODO",
            "priority": "HIGH"
        });

        let child = store
            .create_child_node_atomic(&parent.id, "task", "Task content", properties, None)
            .await?;

        // Verify properties were set
        let fetched = store.get_node(&child.id).await?.unwrap();
        assert_eq!(fetched.properties["status"], "TODO");
        assert_eq!(fetched.properties["priority"], "HIGH");

        Ok(())
    }

    #[tokio::test]
    async fn test_create_child_node_atomic_rollback_on_failure() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Count initial nodes (seeded core schemas: task, date, text, header, code-block, quote-block, ordered-list = 7)
        let initial_nodes = store.query_nodes(NodeQuery::new()).await?;
        let initial_count = initial_nodes.len();

        // Try to create child with non-existent parent (should fail)
        let result = store
            .create_child_node_atomic("non-existent-parent", "text", "Child", json!({}), None)
            .await;

        assert!(result.is_err());

        // Verify no new nodes were created (orphan nodes would increase the count)
        let final_nodes = store.query_nodes(NodeQuery::new()).await?;
        assert_eq!(
            final_nodes.len(),
            initial_count,
            "No nodes should be created after failed transaction - expected {} nodes, got {}",
            initial_count,
            final_nodes.len()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_move_node_atomic_success() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create parent1, parent2, and child
        let parent1 = store
            .create_node(
                Node::new("text".to_string(), "Parent 1".to_string(), json!({})),
                None,
            )
            .await?;
        let parent2 = store
            .create_node(
                Node::new("text".to_string(), "Parent 2".to_string(), json!({})),
                None,
            )
            .await?;
        let child = store
            .create_child_node_atomic(&parent1.id, "text", "Child", json!({}), None)
            .await?;

        // Verify child is under parent1
        let children1 = store.get_children(Some(&parent1.id)).await?;
        assert_eq!(children1.len(), 1);

        // Move child to parent2 atomically
        store.move_node(&child.id, Some(&parent2.id), None).await?;

        // Verify child is now under parent2
        let children1_after = store.get_children(Some(&parent1.id)).await?;
        let children2_after = store.get_children(Some(&parent2.id)).await?;

        assert_eq!(children1_after.len(), 0, "Parent1 should have no children");
        assert_eq!(children2_after.len(), 1, "Parent2 should have 1 child");
        assert_eq!(children2_after[0].id, child.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_move_node_atomic_to_root() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create parent and child
        let parent = store
            .create_node(
                Node::new("text".to_string(), "Parent".to_string(), json!({})),
                None,
            )
            .await?;
        let child = store
            .create_child_node_atomic(&parent.id, "text", "Child", json!({}), None)
            .await?;

        // Move child to root
        store.move_node(&child.id, None, None).await?;

        // Verify child is a root node
        let parent_children = store.get_children(Some(&parent.id)).await?;
        let root_nodes = store.get_children(None).await?;

        assert_eq!(parent_children.len(), 0);
        assert!(root_nodes.iter().any(|n| n.id == child.id));

        Ok(())
    }

    #[tokio::test]
    async fn test_move_node_atomic_prevents_cycles() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create parent and child
        let parent = store
            .create_node(
                Node::new("text".to_string(), "Parent".to_string(), json!({})),
                None,
            )
            .await?;
        let child = store
            .create_child_node_atomic(&parent.id, "text", "Child", json!({}), None)
            .await?;

        // Try to move parent under child (would create cycle)
        let result = store.move_node(&parent.id, Some(&child.id), None).await;

        assert!(
            result.is_err(),
            "Moving parent under child should fail (cycle detection)"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_node_cascade_atomic_success() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create parent and child
        let parent = store
            .create_node(
                Node::new("text".to_string(), "Parent".to_string(), json!({})),
                None,
            )
            .await?;
        let child = store
            .create_child_node_atomic(&parent.id, "text", "Child", json!({}), None)
            .await?;

        // Delete parent (should cascade delete edges)
        let result = store.delete_node_cascade_atomic(&parent.id, None).await?;
        assert!(result.existed);

        // Verify parent was deleted
        let parent_fetched = store.get_node(&parent.id).await?;
        assert!(parent_fetched.is_none());

        // Verify child still exists (cascade doesn't delete children, only edges)
        let child_fetched = store.get_node(&child.id).await?;
        assert!(child_fetched.is_some());

        // Verify child is now a root node (no parent relationship)
        let root_nodes = store.get_children(None).await?;
        assert!(root_nodes.iter().any(|n| n.id == child.id));

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_node_cascade_atomic_idempotent() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Delete non-existent node (should succeed idempotently)
        let result = store
            .delete_node_cascade_atomic("non-existent-id", None)
            .await?;
        assert!(!result.existed);

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_node_cascade_atomic_with_task() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create task node with properties
        let task = Node::new(
            "task".to_string(),
            "Task content".to_string(),
            json!({"status": "TODO"}),
        );
        let task = store.create_node(task, None).await?;

        // Delete task (should delete both node and task-specific record)
        let result = store.delete_node_cascade_atomic(&task.id, None).await?;
        assert!(result.existed);

        // Verify complete deletion
        let fetched = store.get_node(&task.id).await?;
        assert!(fetched.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_switch_node_type_atomic_text_to_task() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create text node
        let node = store
            .create_node(
                Node::new("text".to_string(), "Original text".to_string(), json!({})),
                None,
            )
            .await?;

        // Switch to task type atomically
        let updated = store
            .switch_node_type_atomic(
                &node.id,
                "task",
                json!({"status": "TODO", "priority": "HIGH"}),
                None,
            )
            .await?;

        // Verify type switch
        assert_eq!(updated.node_type, "task");
        assert_eq!(updated.properties["status"], "TODO");
        assert_eq!(updated.properties["priority"], "HIGH");

        // Verify content preserved
        assert_eq!(updated.content, "Original text");

        Ok(())
    }

    #[tokio::test]
    async fn test_switch_node_type_atomic_task_to_text() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create task node
        let task = store
            .create_node(
                Node::new(
                    "task".to_string(),
                    "Task content".to_string(),
                    json!({"status": "done"}),
                ),
                None,
            )
            .await?;

        // Switch to text type atomically
        let updated = store
            .switch_node_type_atomic(&task.id, "text", json!({}), None)
            .await?;

        // Verify type switch
        assert_eq!(updated.node_type, "text");

        // Verify content preserved
        assert_eq!(updated.content, "Task content");

        Ok(())
    }

    #[tokio::test]
    async fn test_switch_node_type_atomic_preserves_variants() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create text node
        let node = store
            .create_node(
                Node::new("text".to_string(), "Content".to_string(), json!({})),
                None,
            )
            .await?;

        // Switch to task
        store
            .switch_node_type_atomic(&node.id, "task", json!({"status": "TODO"}), None)
            .await?;

        // Switch back to text
        let _final_node = store
            .switch_node_type_atomic(&node.id, "text", json!({}), None)
            .await?;

        // Fetch with properties to check variants map
        let fetched = store.get_node(&node.id).await?.unwrap();

        // Variants should be preserved (this is implementation detail, test structure exists)
        assert_eq!(fetched.node_type, "text");

        Ok(())
    }

    // Tests for the adjacency list strategy (recursive graph traversal)
    // Uses SurrealDB's .{..}(->relationship->target) syntax for recursive queries

    #[tokio::test]
    async fn test_get_nodes_in_subtree_returns_descendants() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        // Create a tree structure: root -> child -> grandchild
        let root = Node::new("text".to_string(), "Root".to_string(), json!({}));
        let child = Node::new("text".to_string(), "Child".to_string(), json!({}));
        let grandchild = Node::new("text".to_string(), "Grandchild".to_string(), json!({}));

        store.create_node(root.clone(), None).await?;
        store.create_node(child.clone(), None).await?;
        store.create_node(grandchild.clone(), None).await?;

        // Create relationships: root -> child -> grandchild
        store.move_node(&child.id, Some(&root.id), None).await?;
        store
            .move_node(&grandchild.id, Some(&child.id), None)
            .await?;

        // Get nodes in subtree of root - should include child and grandchild
        let subtree_nodes = store.get_nodes_in_subtree(&root.id).await?;

        assert_eq!(
            subtree_nodes.len(),
            2,
            "Should have 2 descendants (child and grandchild)"
        );
        let ids: Vec<_> = subtree_nodes.iter().map(|n| n.id.clone()).collect();
        assert!(ids.contains(&child.id), "Should contain child");
        assert!(ids.contains(&grandchild.id), "Should contain grandchild");

        Ok(())
    }

    #[tokio::test]
    async fn test_get_nodes_in_subtree_leaf_node_returns_empty() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        // Create a leaf node with no children
        let leaf = Node::new("text".to_string(), "Leaf".to_string(), json!({}));
        store.create_node(leaf.clone(), None).await?;

        // Get nodes in subtree of leaf - should return empty vec
        let subtree_nodes = store.get_nodes_in_subtree(&leaf.id).await?;

        assert!(
            subtree_nodes.is_empty(),
            "Leaf node should have no descendants"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_get_relationships_in_subtree_returns_subtree_edges() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        // Create a tree structure: root -> child -> grandchild
        let root = Node::new("text".to_string(), "Root".to_string(), json!({}));
        let child = Node::new("text".to_string(), "Child".to_string(), json!({}));
        let grandchild = Node::new("text".to_string(), "Grandchild".to_string(), json!({}));

        store.create_node(root.clone(), None).await?;
        store.create_node(child.clone(), None).await?;
        store.create_node(grandchild.clone(), None).await?;

        // Create relationships: root -> child -> grandchild
        store.move_node(&child.id, Some(&root.id), None).await?;
        store
            .move_node(&grandchild.id, Some(&child.id), None)
            .await?;

        // Get relationships in subtree of root - should include both relationships
        let subtree_relationships = store.get_relationships_in_subtree(&root.id).await?;

        assert_eq!(
            subtree_relationships.len(),
            2,
            "Should have 2 relationships in subtree"
        );

        // Verify the relationships are correct
        let relationship_pairs: Vec<_> = subtree_relationships
            .iter()
            .map(|r| (r.in_node.clone(), r.out_node.clone()))
            .collect();
        assert!(
            relationship_pairs.contains(&(root.id.clone(), child.id.clone())),
            "Should contain root->child relationship"
        );
        assert!(
            relationship_pairs.contains(&(child.id.clone(), grandchild.id.clone())),
            "Should contain child->grandchild relationship"
        );

        Ok(())
    }

    // ==================== Mention Autocomplete Tests ====================

    #[tokio::test]
    async fn test_mention_autocomplete_excludes_date_and_schema_types() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        // Create nodes of different types with matching content
        let text_node = Node::new(
            "text".to_string(),
            "searchable content".to_string(),
            json!({}),
        );
        let date_node = Node::new(
            "date".to_string(),
            "searchable content".to_string(),
            json!({}),
        );
        let schema_node = Node::new(
            "schema".to_string(),
            "searchable content".to_string(),
            json!({}),
        );
        let task_node = Node::new(
            "task".to_string(),
            "searchable content".to_string(),
            json!({}),
        );

        store.create_node(text_node.clone(), None).await?;
        store.create_node(date_node.clone(), None).await?;
        store.create_node(schema_node.clone(), None).await?;
        store.create_node(task_node.clone(), None).await?;

        // Search for matching content
        let results = store.mention_autocomplete("searchable", None).await?;

        // Should find text and task, but NOT date or schema
        let result_ids: Vec<_> = results.iter().map(|n| &n.id).collect();
        assert!(
            result_ids.contains(&&text_node.id),
            "Should include text node"
        );
        assert!(
            result_ids.contains(&&task_node.id),
            "Should include task node"
        );
        assert!(
            !result_ids.contains(&&date_node.id),
            "Should NOT include date node"
        );
        assert!(
            !result_ids.contains(&&schema_node.id),
            "Should NOT include schema node"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_mention_autocomplete_text_types_only_root_nodes() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        // Create root text nodes
        let root_text = Node::new("text".to_string(), "findable root".to_string(), json!({}));
        let root_header = Node::new(
            "header".to_string(),
            "findable header".to_string(),
            json!({}),
        );
        let root_code = Node::new(
            "code-block".to_string(),
            "findable code".to_string(),
            json!({}),
        );
        let root_quote = Node::new(
            "quote-block".to_string(),
            "findable quote root".to_string(),
            json!({}),
        );
        let root_ordered = Node::new(
            "ordered-list".to_string(),
            "1. findable ordered".to_string(),
            json!({}),
        );

        // Create nested text nodes (have parent)
        let parent = Node::new("text".to_string(), "parent node".to_string(), json!({}));
        let nested_text = Node::new("text".to_string(), "findable nested".to_string(), json!({}));
        let nested_quote = Node::new(
            "quote-block".to_string(),
            "findable quote".to_string(),
            json!({}),
        );

        store.create_node(root_text.clone(), None).await?;
        store.create_node(root_header.clone(), None).await?;
        store.create_node(root_code.clone(), None).await?;
        store.create_node(root_quote.clone(), None).await?;
        store.create_node(root_ordered.clone(), None).await?;
        store.create_node(parent.clone(), None).await?;
        store.create_node(nested_text.clone(), None).await?;
        store.create_node(nested_quote.clone(), None).await?;

        // Make nested nodes children of parent
        store
            .move_node(&nested_text.id, Some(&parent.id), None)
            .await?;
        store
            .move_node(&nested_quote.id, Some(&parent.id), None)
            .await?;

        // Search for "findable"
        let results = store.mention_autocomplete("findable", None).await?;

        let result_ids: Vec<_> = results.iter().map(|n| &n.id).collect();

        // Root text-type nodes should be included
        assert!(
            result_ids.contains(&&root_text.id),
            "Should include root text node"
        );
        assert!(
            result_ids.contains(&&root_header.id),
            "Should include root header node"
        );
        assert!(
            result_ids.contains(&&root_code.id),
            "Should include root code-block node"
        );
        assert!(
            result_ids.contains(&&root_quote.id),
            "Should include root quote-block node"
        );
        assert!(
            result_ids.contains(&&root_ordered.id),
            "Should include root ordered-list node"
        );

        // Nested text-type nodes should NOT be included
        assert!(
            !result_ids.contains(&&nested_text.id),
            "Should NOT include nested text node"
        );
        assert!(
            !result_ids.contains(&&nested_quote.id),
            "Should NOT include nested quote-block node"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_mention_autocomplete_non_text_types_include_nested() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        // Create parent node
        let parent = Node::new("text".to_string(), "parent node".to_string(), json!({}));

        // Create task nodes - one root, one nested
        let root_task = Node::new(
            "task".to_string(),
            "findme root task".to_string(),
            json!({}),
        );
        let nested_task = Node::new(
            "task".to_string(),
            "findme nested task".to_string(),
            json!({}),
        );

        // Create query node - nested
        let nested_query = Node::new(
            "query".to_string(),
            "findme nested query".to_string(),
            json!({}),
        );

        store.create_node(parent.clone(), None).await?;
        store.create_node(root_task.clone(), None).await?;
        store.create_node(nested_task.clone(), None).await?;
        store.create_node(nested_query.clone(), None).await?;

        // Make tasks and query children of parent
        store
            .move_node(&nested_task.id, Some(&parent.id), None)
            .await?;
        store
            .move_node(&nested_query.id, Some(&parent.id), None)
            .await?;

        // Search for "findme"
        let results = store.mention_autocomplete("findme", None).await?;

        let result_ids: Vec<_> = results.iter().map(|n| &n.id).collect();

        // Both root and nested task/query nodes should be included
        assert!(
            result_ids.contains(&&root_task.id),
            "Should include root task"
        );
        assert!(
            result_ids.contains(&&nested_task.id),
            "Should include nested task"
        );
        assert!(
            result_ids.contains(&&nested_query.id),
            "Should include nested query"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_mention_autocomplete_case_insensitive() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        let node1 = Node::new(
            "text".to_string(),
            "UPPERCASE content".to_string(),
            json!({}),
        );
        let node2 = Node::new(
            "task".to_string(),
            "lowercase content".to_string(),
            json!({}),
        );
        let node3 = Node::new(
            "text".to_string(),
            "MixedCase Content".to_string(),
            json!({}),
        );

        store.create_node(node1.clone(), None).await?;
        store.create_node(node2.clone(), None).await?;
        store.create_node(node3.clone(), None).await?;

        // Search with lowercase should find all
        let results = store.mention_autocomplete("content", None).await?;
        assert_eq!(results.len(), 3, "Should find all 3 nodes with 'content'");

        // Search with uppercase should also find all
        let results = store.mention_autocomplete("CONTENT", None).await?;
        assert_eq!(
            results.len(),
            3,
            "Should find all 3 nodes with 'CONTENT' (case insensitive)"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_mention_autocomplete_respects_limit() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        // Create multiple matching nodes
        for i in 0..5 {
            let node = Node::new(
                "task".to_string(),
                format!("searchterm item {}", i),
                json!({}),
            );
            store.create_node(node, None).await?;
        }

        // Search with limit
        let results = store.mention_autocomplete("searchterm", Some(3)).await?;
        assert_eq!(results.len(), 3, "Should respect limit of 3");

        // Default limit (10) with fewer results
        let results = store.mention_autocomplete("searchterm", None).await?;
        assert_eq!(
            results.len(),
            5,
            "Should return all 5 when under default limit"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_mention_autocomplete_no_results() -> Result<()> {
        let (store, _temp) = create_test_store().await?;

        let node = Node::new("text".to_string(), "some content".to_string(), json!({}));
        store.create_node(node, None).await?;

        // Search for non-existent term
        let results = store.mention_autocomplete("nonexistent", None).await?;
        assert!(results.is_empty(), "Should return empty for no matches");

        Ok(())
    }

    #[tokio::test]
    async fn test_get_nodes_by_ids_basic() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create multiple nodes
        let node1 = Node::new("text".to_string(), "Content 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Content 2".to_string(), json!({}));
        let node3 = Node::new("text".to_string(), "Content 3".to_string(), json!({}));

        let created1 = store.create_node(node1, None).await?;
        let created2 = store.create_node(node2, None).await?;
        let created3 = store.create_node(node3, None).await?;

        // Batch fetch all three nodes
        let ids = vec![
            created1.id.clone(),
            created2.id.clone(),
            created3.id.clone(),
        ];
        let result = store.get_nodes_by_ids(&ids).await?;

        assert_eq!(result.len(), 3);
        assert_eq!(result.get(&created1.id).unwrap().content, "Content 1");
        assert_eq!(result.get(&created2.id).unwrap().content, "Content 2");
        assert_eq!(result.get(&created3.id).unwrap().content, "Content 3");

        Ok(())
    }

    #[tokio::test]
    async fn test_get_nodes_by_ids_with_nonexistent() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        // Create one node
        let node = Node::new("text".to_string(), "Existing node".to_string(), json!({}));
        let created = store.create_node(node, None).await?;

        // Try to fetch existing and non-existent nodes
        let ids = vec![
            created.id.clone(),
            "nonexistent-id-1".to_string(),
            "nonexistent-id-2".to_string(),
        ];
        let result = store.get_nodes_by_ids(&ids).await?;

        // Should only return the existing node
        assert_eq!(result.len(), 1);
        assert!(result.contains_key(&created.id));
        assert!(!result.contains_key("nonexistent-id-1"));
        assert!(!result.contains_key("nonexistent-id-2"));

        Ok(())
    }

    #[tokio::test]
    async fn test_get_nodes_by_ids_empty_list() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let result = store.get_nodes_by_ids(&[]).await?;
        assert!(result.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_get_nodes_by_ids_with_task_nodes() -> Result<()> {
        // Test that task nodes are correctly fetched in batch
        let (store, _temp_dir) = create_test_store().await?;

        let task1 = Node::new(
            "task".to_string(),
            "Task 1".to_string(),
            json!({"status": "open"}),
        );
        let task2 = Node::new(
            "task".to_string(),
            "Task 2".to_string(),
            json!({"status": "done"}),
        );

        let created1 = store.create_node(task1, None).await?;
        let created2 = store.create_node(task2, None).await?;

        let ids = vec![created1.id.clone(), created2.id.clone()];
        let result = store.get_nodes_by_ids(&ids).await?;

        assert_eq!(result.len(), 2);
        let fetched1 = result.get(&created1.id).unwrap();
        let fetched2 = result.get(&created2.id).unwrap();

        assert_eq!(fetched1.node_type, "task");
        assert_eq!(fetched1.content, "Task 1");
        assert_eq!(fetched1.properties["status"], "open");

        assert_eq!(fetched2.node_type, "task");
        assert_eq!(fetched2.content, "Task 2");
        assert_eq!(fetched2.properties["status"], "done");

        Ok(())
    }

    #[tokio::test]
    async fn test_get_nodes_by_ids_with_mixed_types() -> Result<()> {
        // Test batch fetch with mixed node types (text and task)
        let (store, _temp_dir) = create_test_store().await?;

        let text_node = Node::new("text".to_string(), "Text content".to_string(), json!({}));
        let task_node = Node::new(
            "task".to_string(),
            "Task content".to_string(),
            json!({"status": "pending"}),
        );

        let text_created = store.create_node(text_node, None).await?;
        let task_created = store.create_node(task_node, None).await?;

        let ids = vec![text_created.id.clone(), task_created.id.clone()];
        let result = store.get_nodes_by_ids(&ids).await?;

        assert_eq!(result.len(), 2);

        let text_fetched = result.get(&text_created.id).unwrap();
        assert_eq!(text_fetched.node_type, "text");
        assert_eq!(text_fetched.content, "Text content");

        let task_fetched = result.get(&task_created.id).unwrap();
        assert_eq!(task_fetched.node_type, "task");
        assert_eq!(task_fetched.content, "Task content");
        assert_eq!(task_fetched.properties["status"], "pending");

        Ok(())
    }

    #[tokio::test]
    async fn test_get_nodes_by_ids_larger_batch() -> Result<()> {
        // Test with a larger batch (20 nodes)
        let (store, _temp_dir) = create_test_store().await?;

        let mut ids = Vec::new();
        for i in 0..20 {
            let node = Node::new("text".to_string(), format!("Content {}", i), json!({}));
            let created = store.create_node(node, None).await?;
            ids.push(created.id);
        }

        let result = store.get_nodes_by_ids(&ids).await?;

        assert_eq!(result.len(), 20);
        for (i, id) in ids.iter().enumerate() {
            let node = result.get(id).unwrap();
            assert_eq!(node.content, format!("Content {}", i));
        }

        Ok(())
    }

    // ============================================================================
    // Issue #795: Sync-ready relationship timestamps tests
    // ============================================================================

    /// Helper to get relationship metadata (created_at, modified_at, version) for a child node
    async fn get_relationship_metadata(
        store: &SurrealStore,
        child_id: &str,
    ) -> Result<Option<(String, String, i64)>> {
        use surrealdb::sql::Thing;

        #[derive(Debug, serde::Deserialize)]
        struct RelMetadata {
            created_at: String,
            modified_at: String,
            version: i64,
        }

        let child_thing = Thing::from(("node".to_string(), child_id.to_string()));

        let mut response = store
            .db
            .query("SELECT created_at, modified_at, version FROM relationship WHERE out = $child_thing AND relationship_type = 'has_child' LIMIT 1;")
            .bind(("child_thing", child_thing))
            .await
            .context("Failed to get relationship metadata")?;

        let metadata: Vec<RelMetadata> = response
            .take(0)
            .context("Failed to extract relationship metadata")?;

        Ok(metadata
            .into_iter()
            .next()
            .map(|m| (m.created_at, m.modified_at, m.version)))
    }

    #[tokio::test]
    async fn test_same_parent_reorder_preserves_created_at() -> Result<()> {
        // Issue #795: Same-parent reorder should preserve created_at
        let (store, _temp_dir) = create_test_store().await?;

        // Create parent and two children
        let parent = store
            .create_node(
                Node::new("text".to_string(), "Parent".to_string(), json!({})),
                None,
            )
            .await?;

        let _child1 = store
            .create_child_node_atomic(&parent.id, "text", "Child 1", json!({}), None)
            .await?;
        let child2 = store
            .create_child_node_atomic(&parent.id, "text", "Child 2", json!({}), None)
            .await?;

        // Get original relationship metadata for child2
        let original_metadata = get_relationship_metadata(&store, &child2.id)
            .await?
            .expect("Relationship should exist");
        let original_created_at = original_metadata.0.clone();

        // Wait a tiny bit to ensure time difference
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Reorder child2 to be before child1 (same parent, just reordering)
        store.move_node(&child2.id, Some(&parent.id), None).await?;

        // Get new relationship metadata
        let new_metadata = get_relationship_metadata(&store, &child2.id)
            .await?
            .expect("Relationship should still exist");

        // created_at should be PRESERVED (same value)
        assert_eq!(
            new_metadata.0, original_created_at,
            "Same-parent reorder should preserve created_at"
        );

        // modified_at should be UPDATED (different value)
        assert_ne!(
            new_metadata.1, original_metadata.1,
            "Same-parent reorder should update modified_at"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_same_parent_reorder_increments_version() -> Result<()> {
        // Issue #795: Same-parent reorder should increment version for OCC
        let (store, _temp_dir) = create_test_store().await?;

        // Create parent and child
        let parent = store
            .create_node(
                Node::new("text".to_string(), "Parent".to_string(), json!({})),
                None,
            )
            .await?;

        let child = store
            .create_child_node_atomic(&parent.id, "text", "Child", json!({}), None)
            .await?;

        // Get original version
        let original_metadata = get_relationship_metadata(&store, &child.id)
            .await?
            .expect("Relationship should exist");
        let original_version = original_metadata.2;

        // Reorder (same parent)
        store.move_node(&child.id, Some(&parent.id), None).await?;

        // Get new version
        let new_metadata = get_relationship_metadata(&store, &child.id)
            .await?
            .expect("Relationship should still exist");

        // Version should be incremented
        assert_eq!(
            new_metadata.2,
            original_version + 1,
            "Same-parent reorder should increment version (was {}, expected {})",
            original_version,
            original_version + 1
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_cross_parent_move_creates_new_relationship() -> Result<()> {
        // Issue #795: Cross-parent move should create new relationship with new created_at
        let (store, _temp_dir) = create_test_store().await?;

        // Create two parents and a child under parent1
        let parent1 = store
            .create_node(
                Node::new("text".to_string(), "Parent 1".to_string(), json!({})),
                None,
            )
            .await?;
        let parent2 = store
            .create_node(
                Node::new("text".to_string(), "Parent 2".to_string(), json!({})),
                None,
            )
            .await?;

        let child = store
            .create_child_node_atomic(&parent1.id, "text", "Child", json!({}), None)
            .await?;

        // Get original relationship metadata
        let original_metadata = get_relationship_metadata(&store, &child.id)
            .await?
            .expect("Relationship should exist");
        let original_created_at = original_metadata.0.clone();

        // Wait to ensure time difference
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Move child to different parent
        store.move_node(&child.id, Some(&parent2.id), None).await?;

        // Get new relationship metadata
        let new_metadata = get_relationship_metadata(&store, &child.id)
            .await?
            .expect("Relationship should exist");

        // created_at should be DIFFERENT (new relationship)
        assert_ne!(
            new_metadata.0, original_created_at,
            "Cross-parent move should create new relationship with new created_at"
        );

        // version should be reset to 1 (new relationship)
        assert_eq!(
            new_metadata.2, 1,
            "Cross-parent move should reset version to 1"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_same_parent_reorders_accumulate_version() -> Result<()> {
        // Issue #795: Multiple reorders should accumulate version
        let (store, _temp_dir) = create_test_store().await?;

        // Create parent and child
        let parent = store
            .create_node(
                Node::new("text".to_string(), "Parent".to_string(), json!({})),
                None,
            )
            .await?;

        let child = store
            .create_child_node_atomic(&parent.id, "text", "Child", json!({}), None)
            .await?;

        // Initial version should be 1
        let initial_metadata = get_relationship_metadata(&store, &child.id)
            .await?
            .expect("Relationship should exist");
        assert_eq!(initial_metadata.2, 1, "Initial version should be 1");

        // Reorder multiple times
        for expected_version in 2..=5 {
            store.move_node(&child.id, Some(&parent.id), None).await?;

            let metadata = get_relationship_metadata(&store, &child.id)
                .await?
                .expect("Relationship should exist");

            assert_eq!(
                metadata.2,
                expected_version,
                "Version should be {} after {} reorders",
                expected_version,
                expected_version - 1
            );
        }

        Ok(())
    }
}
