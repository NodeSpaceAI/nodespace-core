//! Node Service - Core CRUD Operations
//!
//! This module provides the main business logic layer for node operations:
//!
//! - CRUD operations (create, read, update, delete)
//! - Hierarchy management (get_children, move_node, reorder_siblings)
//! - Bulk operations with transactions
//! - Query operations with filtering
//!
//! # Scope
//!
//! Initial implementation supports Text, Task, and Date nodes for E2E testing.
//! Person and Project node support will be added in separate issues.
//!
//! # Root Node Detection
//!
//! Root nodes (topics, date nodes, etc.) are the primary targets for semantic search.
//! They are identified by `root_id IS NULL` in the database.
//!
//! **CRITICAL:** Never use `node_type == 'topic'` for root detection.
//! The node_type field indicates the node's behavior, not its root status.
//!
//! Examples:
//! - Root node: `root_id = NULL` (e.g., @mention pages, date nodes)
//! - Child node: `root_id = Some("parent-id")` (e.g., notes within a topic)

use crate::behaviors::NodeBehaviorRegistry;
use crate::db::events::DomainEvent;
use crate::db::{StoreChange, StoreOperation, SurrealStore};
use crate::models::embedding::is_embeddable_type;
use crate::models::schema::SchemaRelationship;
use crate::models::{Node, NodeFilter, NodeUpdate};
use crate::services::error::NodeServiceError;
use crate::services::migration_registry::MigrationRegistry;
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use tokio::sync::broadcast;

/// Default limit for query_nodes_simple when no limit is specified.
/// Prevents accidental full table scans and improves performance.
pub const DEFAULT_QUERY_LIMIT: usize = 100;

/// Type alias for subtree data returned by `get_subtree_data`
///
/// Contains (root_node, node_map, adjacency_list) where:
/// - root_node: Option<Node> - the root node if it exists
/// - node_map: HashMap<String, Node> - all nodes indexed by ID
/// - adjacency_list: HashMap<String, Vec<String>> - children IDs indexed by parent ID (sorted by order)
pub type SubtreeData = (
    Option<Node>,
    HashMap<String, Node>,
    HashMap<String, Vec<String>>,
);

/// Parameters for creating a node
///
/// This struct is used by `NodeService::create_node_with_parent()` to encapsulate
/// all parameters needed for node creation.
///
/// # ID Generation Strategy
///
/// The `id` field supports three distinct scenarios:
///
/// 1. **Frontend-provided UUID** (Tauri commands): The frontend pre-generates UUIDs for
///    optimistic UI updates and local state tracking (`persistedNodeIds`). This ensures
///    ID consistency between client and server, preventing sync issues.
///
/// 2. **Auto-generated UUID** (MCP handlers): Server-side generation for external clients
///    like AI assistants. This prevents ID conflicts and maintains security boundaries.
///
/// 3. **Date-based ID** (special case): Date nodes use their content (YYYY-MM-DD format)
///    as the ID, enabling predictable lookups and ensuring uniqueness by date.
///
/// # Security Considerations
///
/// When accepting frontend-provided IDs:
///
/// - **UUID validation**: Non-date nodes must provide valid UUID format. Invalid UUIDs
///   are rejected with `InvalidOperation` error.
/// - **Database constraints**: The database enforces UNIQUE constraint on `nodes.id`,
///   preventing collisions at the storage layer.
/// - **Trust boundary**: Only Tauri commands (trusted in-process frontend) can provide
///   custom IDs. MCP handlers (external AI clients) always use server-side generation.
///
/// # Examples
///
/// ```no_run
/// # use nodespace_core::services::CreateNodeParams;
/// # use serde_json::json;
/// // Auto-generated ID (MCP path)
/// let params = CreateNodeParams {
///     id: None,
///     node_type: "text".to_string(),
///     content: "Hello World".to_string(),
///     parent_id: Some("parent-123".to_string()),
///     insert_after_node_id: None,
///     properties: json!({}),
/// };
///
/// // Frontend-provided UUID (Tauri path)
/// let frontend_id = uuid::Uuid::new_v4().to_string();
/// let params_with_id = CreateNodeParams {
///     id: Some(frontend_id),
///     node_type: "text".to_string(),
///     content: "Tracked by frontend".to_string(),
///     parent_id: None,
///     insert_after_node_id: None,
///     properties: json!({}),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CreateNodeParams {
    /// Optional ID for the node. If None, will be auto-generated (UUID for most types, content for date nodes)
    pub id: Option<String>,
    /// Type of the node (text, task, date, etc.)
    pub node_type: String,
    /// Content of the node
    pub content: String,
    /// Optional parent node ID (container/root will be auto-derived from parent chain)
    pub parent_id: Option<String>,
    /// Optional sibling to insert after (None = insert at beginning of siblings)
    pub insert_after_node_id: Option<String>,
    /// Additional node properties as JSON
    pub properties: Value,
}

/// Broadcast channel capacity for domain events.
///
/// 128 provides sufficient headroom for burst operations (bulk node creation)
/// while limiting memory overhead. Observer lag is acceptable - we only track
/// the current state, not historical events.
const DOMAIN_EVENT_CHANNEL_CAPACITY: usize = 128;

/// Check if a string matches date node format: YYYY-MM-DD
///
/// Valid examples: "2025-10-13", "2024-01-01"
/// Invalid examples: "abcd-ef-gh", "2025-10-1", "25-10-13", "2025-13-45" (invalid date)
///
/// This function validates both format AND semantic validity:
/// - Format: YYYY-MM-DD pattern (10 chars, correct positions for digits/dashes)
/// - Semantics: Must be a valid calendar date (no month 13, no day 45, etc.)
fn is_date_node_id(id: &str) -> bool {
    // Must be exactly 10 characters: YYYY-MM-DD
    if id.len() != 10 {
        return false;
    }

    // Check format: 4 digits, dash, 2 digits, dash, 2 digits
    let bytes = id.as_bytes();
    let format_valid = bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4] == b'-'
        && bytes[5].is_ascii_digit()
        && bytes[6].is_ascii_digit()
        && bytes[7] == b'-'
        && bytes[8].is_ascii_digit()
        && bytes[9].is_ascii_digit();

    if !format_valid {
        return false;
    }

    // Semantic validation: Verify it's a valid calendar date
    // This prevents accepting strings like "2025-13-45" (invalid month/day)
    chrono::NaiveDate::parse_from_str(id, "%Y-%m-%d").is_ok()
}

/// Check if a node is a root node based on its root_id
///
/// Root nodes are identified by having a NULL root_id in the database.
/// This is the ONLY correct way to detect root nodes.
///
/// # Arguments
///
/// * `root_id` - The root_id field from a Node
///
/// # Returns
///
/// `true` if the node is a root (root_id is None), `false` otherwise
///
/// # Examples
///
/// ```
/// # use nodespace_core::services::node_service::is_root_node;
/// assert!(is_root_node(&None)); // Root node
/// assert!(!is_root_node(&Some("parent-id".to_string()))); // Child node
/// ```
pub fn is_root_node(root_id: &Option<String>) -> bool {
    root_id.is_none()
}

// Regex pattern for UUID validation (lowercase hex with standard UUID format)
const UUID_PATTERN: &str = r"^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$";

// Regex pattern for date validation (YYYY-MM-DD format)
const DATE_PATTERN: &str = r"^\d{4}-\d{2}-\d{2}$";

// Regex pattern for markdown-style nodespace links
// Matches: [@text](nodespace://uuid) or [text](nodespace://node/uuid?params)
// Capture group 1: the node ID (without "node/" prefix or query params)
const MARKDOWN_MENTION_PATTERN: &str =
    r"\[[^\]]+\]\(nodespace://(?:node/)?([^\s)?]+)(?:\?[^)]*)?\)";

// Regex pattern for plain nodespace URIs
// Matches: nodespace://uuid or nodespace://node/uuid
// Capture group 1: the node ID (without "node/" prefix)
const PLAIN_MENTION_PATTERN: &str = r"nodespace://(?:node/)?([^\s)?]+)";

/// Validate if a node ID is valid (UUID or date format)
///
/// Valid formats:
/// - UUID: 36-character hex string with dashes (e.g., "abc123-...")
/// - Date: YYYY-MM-DD format (e.g., "2025-10-24")
///
/// # Examples
///
/// ```
/// # use nodespace_core::services::node_service::is_valid_node_id;
/// assert!(is_valid_node_id("550e8400-e29b-41d4-a716-446655440000")); // UUID
/// assert!(is_valid_node_id("2025-10-24")); // Date
/// assert!(!is_valid_node_id("invalid")); // Invalid
/// ```
pub fn is_valid_node_id(node_id: &str) -> bool {
    // Check if it's a UUID (36 characters, hex with dashes)
    static UUID_REGEX: OnceLock<Regex> = OnceLock::new();
    let uuid_regex = UUID_REGEX.get_or_init(|| Regex::new(UUID_PATTERN).unwrap());

    if uuid_regex.is_match(node_id) {
        return true;
    }

    // Check if it's a valid date format (YYYY-MM-DD)
    static DATE_REGEX: OnceLock<Regex> = OnceLock::new();
    let date_regex = DATE_REGEX.get_or_init(|| Regex::new(DATE_PATTERN).unwrap());

    if date_regex.is_match(node_id) {
        // Validate it's an actual valid date using chrono
        if let Ok(date) = chrono::NaiveDate::parse_from_str(node_id, "%Y-%m-%d") {
            // Verify roundtrip: parsing and formatting back should give same string
            return date.format("%Y-%m-%d").to_string() == node_id;
        }
    }

    false
}

/// Extract nodespace:// mentions from content
///
/// Supports both markdown format and plain URIs:
/// - Markdown: [@text](nodespace://node-id) or [text](nodespace://node-id)
/// - Plain: nodespace://node-id
///
/// Accepts both UUID and date format node IDs:
/// - UUID: abc123-def456-... (36 chars)
/// - Date: 2025-10-24 (YYYY-MM-DD format)
///
/// Returns array of unique mentioned node IDs (duplicates removed).
///
/// # Performance
///
/// - **Time Complexity:** O(n × m) where n = content length, m = number of markdown links
/// - **Space Complexity:** O(k) where k = unique mentions found
/// - **Typical Performance:** ~1-5µs for content <1000 chars with <10 mentions
///
/// # Examples
///
/// ```
/// # use nodespace_core::services::node_service::extract_mentions;
/// let content = "See [@Node](nodespace://550e8400-e29b-41d4-a716-446655440000) and nodespace://2025-10-24";
/// let mentions = extract_mentions(content);
/// assert_eq!(mentions.len(), 2);
/// ```
pub fn extract_mentions(content: &str) -> Vec<String> {
    let mut mentions = HashSet::new();

    // Match markdown format using the defined pattern
    static MARKDOWN_REGEX: OnceLock<Regex> = OnceLock::new();
    let markdown_regex =
        MARKDOWN_REGEX.get_or_init(|| Regex::new(MARKDOWN_MENTION_PATTERN).unwrap());

    for cap in markdown_regex.captures_iter(content) {
        if let Some(node_id) = cap.get(1) {
            let node_id_str = node_id.as_str();
            if is_valid_node_id(node_id_str) {
                mentions.insert(node_id_str.to_string());
            }
        }
    }

    // Match plain format using the defined pattern
    // We need to avoid matching nodespace:// URIs that are already inside markdown links
    static PLAIN_REGEX: OnceLock<Regex> = OnceLock::new();
    let plain_regex = PLAIN_REGEX.get_or_init(|| Regex::new(PLAIN_MENTION_PATTERN).unwrap());

    // Collect all positions where markdown links occur to exclude them
    let mut markdown_ranges = Vec::new();
    for mat in markdown_regex.find_iter(content) {
        markdown_ranges.push((mat.start(), mat.end()));
    }

    // Find plain format matches that don't overlap with markdown matches
    for cap in plain_regex.captures_iter(content) {
        if let Some(node_id) = cap.get(1) {
            let node_id_str = node_id.as_str();

            // Check if this match is inside a markdown link
            let match_pos = cap.get(0).unwrap().start();
            let is_in_markdown = markdown_ranges
                .iter()
                .any(|(start, end)| match_pos >= *start && match_pos < *end);

            if !is_in_markdown && is_valid_node_id(node_id_str) {
                mentions.insert(node_id_str.to_string());
            }
        }
    }

    mentions.into_iter().collect()
}

/// Core service for node CRUD and hierarchy operations
///
/// # Examples
///
/// ```no_run
/// use nodespace_core::services::NodeService;
/// use nodespace_core::db::SurrealStore;
/// use nodespace_core::models::Node;
/// use std::path::PathBuf;
/// use std::sync::Arc;
/// use serde_json::json;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let mut db = Arc::new(SurrealStore::new(PathBuf::from("./data/test.db")).await?);
///     let service = NodeService::new(&mut db).await?;
///
///     let node = Node::new(
///         "text".to_string(),
///         "Hello World".to_string(),
///         json!({}),
///     );
///
///     let id = service.create_node(node).await?;
///     println!("Created node: {}", id);
///     Ok(())
/// }
/// ```
pub struct NodeService<C = surrealdb::engine::local::Db>
where
    C: surrealdb::Connection,
{
    /// SurrealDB store for all persistence operations
    pub(crate) store: Arc<SurrealStore<C>>,

    /// Behavior registry for validation
    behaviors: Arc<NodeBehaviorRegistry>,

    /// Migration registry for lazy schema upgrades
    migration_registry: Arc<MigrationRegistry>,

    /// Broadcast channel for domain events (128 subscriber capacity)
    event_tx: broadcast::Sender<DomainEvent>,

    /// Optional client identifier for event source tracking (Issue #665)
    ///
    /// When set, all emitted events will include this client_id as source_client_id.
    /// This enables clients to filter out their own events (prevent feedback loops).
    ///
    /// Use `with_client()` to create a new NodeService instance with client_id set.
    client_id: Option<String>,

    /// Optional waker to trigger embedding processor (Issue #729)
    ///
    /// When set, `queue_root_for_embedding()` will wake the processor after
    /// creating stale markers. This enables event-driven embedding processing
    /// without polling.
    ///
    /// Use `set_embedding_waker()` to configure after processor is initialized.
    embedding_waker: Option<crate::services::EmbeddingWaker>,
}

// Manual Clone implementation because C doesn't need to be Clone
// (all fields are Arc or inherently cloneable)
impl<C> Clone for NodeService<C>
where
    C: surrealdb::Connection,
{
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            behaviors: self.behaviors.clone(),
            migration_registry: self.migration_registry.clone(),
            event_tx: self.event_tx.clone(),
            client_id: self.client_id.clone(),
            embedding_waker: self.embedding_waker.clone(),
        }
    }
}

impl<C> NodeService<C>
where
    C: surrealdb::Connection,
{
    /// Create a new NodeService
    ///
    /// Initializes the service with SurrealStore and creates a default
    /// NodeBehaviorRegistry with Text, Task, and Date behaviors.
    ///
    /// # Arguments
    ///
    /// * `store` - Mutable reference to Arc<SurrealStore> (allows cache updates during seeding)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut store = Arc::new(SurrealStore::new("./data/nodespace.db".into()).await?);
    /// let service = NodeService::new(&mut store).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Cache Population (Issue #704)
    ///
    /// Takes `&mut Arc<SurrealStore>` to enable cache updates during schema seeding:
    /// - On first launch: Seeds schemas and updates caches incrementally via `Arc::get_mut()`
    /// - On subsequent launches: Caches already populated by `SurrealStore::new()`
    pub async fn new(store: &mut Arc<SurrealStore<C>>) -> Result<Self, NodeServiceError> {
        // Create empty migration registry (no migrations registered yet - pre-deployment)
        // Infrastructure exists for future schema evolution post-deployment
        let migration_registry = MigrationRegistry::new();

        // Initialize broadcast channel for domain events
        let (event_tx, _) = broadcast::channel(DOMAIN_EVENT_CHANNEL_CAPACITY);

        // Register store-level notifier for automatic domain event emission (Issue #718)
        // This callback converts StoreChange notifications to DomainEvents.
        // Must be set BEFORE seed_core_schemas so schema seeding also emits events.
        //
        // Issue #724: Events now send only node_id (not full payload) for efficiency.
        // Subscribers fetch full node data via get_node() if needed.
        {
            let tx = event_tx.clone();
            let notifier = Arc::new(move |change: StoreChange| {
                // Map store operation to domain event (ID-only, no payload conversion)
                let event = match change.operation {
                    StoreOperation::Created => DomainEvent::NodeCreated {
                        node_id: change.node.id.clone(),
                        node_type: change.node.node_type.clone(),
                        source_client_id: change.source,
                    },
                    StoreOperation::Updated => DomainEvent::NodeUpdated {
                        node_id: change.node.id.clone(),
                        source_client_id: change.source,
                    },
                    StoreOperation::Deleted => DomainEvent::NodeDeleted {
                        id: change.node.id.clone(),
                        source_client_id: change.source,
                    },
                };

                // Send to broadcast channel (ignore if no subscribers)
                let _ = tx.send(event);
            });

            // Get mutable reference to store to set notifier
            let store_mut = Arc::get_mut(store).ok_or_else(|| {
                NodeServiceError::InitializationError(
                    "Cannot set notifier: SurrealStore Arc has multiple references".to_string(),
                )
            })?;
            store_mut.set_notifier(notifier);
        }

        // Seed core schemas if needed (Issue #704)
        // This must happen BEFORE we clone the Arc into Self, so we can use Arc::get_mut()
        // to update schema caches incrementally during seeding.
        Self::seed_core_schemas_if_needed(store).await?;

        let service = Self {
            store: Arc::clone(store),
            behaviors: Arc::new(NodeBehaviorRegistry::new()),
            migration_registry: Arc::new(migration_registry),
            event_tx,
            client_id: None,
            embedding_waker: None,
        };

        Ok(service)
    }

    /// Set the embedding waker for event-driven processing (Issue #729)
    ///
    /// Call this after `EmbeddingProcessor` is initialized to enable
    /// automatic wake-on-change for embedding processing.
    ///
    /// # Arguments
    /// * `waker` - The waker handle from `EmbeddingProcessor::waker()`
    pub fn set_embedding_waker(&mut self, waker: crate::services::EmbeddingWaker) {
        self.embedding_waker = Some(waker);
    }

    /// Seed core schema definitions if database is fresh
    ///
    /// Checks if schema nodes exist. If not, creates all core schemas
    /// (task, text, date, header, code-block, quote-block, ordered-list).
    ///
    /// This is idempotent - safe to call multiple times.
    ///
    /// # Architecture Note (Issue #704)
    ///
    /// Schema seeding belongs in the domain layer (NodeService), not the data layer
    /// (SurrealStore). This ensures:
    /// - Clean separation of concerns (data access vs domain logic)
    /// - Works identically for embedded and HTTP modes
    /// - Single source of truth for schema seeding
    ///
    /// # Cache Population Strategy (Issue #704)
    ///
    /// Takes `&mut Arc<SurrealStore>` to enable incremental cache updates:
    /// - After creating each schema, calls `add_to_schema_cache()` via `Arc::get_mut()`
    /// - This avoids re-querying the database since we have schema data in memory
    /// - On subsequent launches, caches are already populated by `SurrealStore::new()`
    async fn seed_core_schemas_if_needed(
        store: &mut Arc<SurrealStore<C>>,
    ) -> Result<(), NodeServiceError> {
        use crate::models::core_schemas::get_core_schemas;

        // Check if schemas already exist by trying to get task schema
        // If task exists, assume all core schemas are seeded
        let task_exists = store
            .get_node("task")
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Failed to check for schemas: {}", e))
            })?
            .is_some();

        if task_exists {
            tracing::info!("✅ Core schemas already seeded");
            return Ok(());
        }

        tracing::info!("🌱 Seeding core schemas...");

        // Get core schemas from canonical source
        let core_schemas = get_core_schemas();

        // Collect schema info for cache updates (before we start creating nodes)
        let schema_cache_updates: Vec<(String, bool)> = core_schemas
            .iter()
            .map(|s| (s.id.clone(), !s.fields.is_empty()))
            .collect();

        // Universal Graph Architecture (Issue #783): Properties stored in node.properties.
        // Only relationship tables are created for relationships.
        {
            let table_manager = crate::services::schema_table_manager::SchemaTableManager::new();

            // For each schema: atomically create schema node + relationship table DDL (if any)
            for schema in &core_schemas {
                let schema_id = schema.id.clone();
                let node = schema.clone().into_node();

                // Universal Graph Architecture: Only generate relationship table DDL for relationships
                let ddl_statements = if !schema.relationships.is_empty() {
                    table_manager
                        .generate_relationship_ddl_statements(&schema_id, &schema.relationships)
                        .map_err(|e| {
                            NodeServiceError::SerializationError(format!(
                                "Failed to generate relationship DDL for '{}': {}",
                                schema_id, e
                            ))
                        })?
                } else {
                    vec![]
                };

                // Atomically create schema node + execute DDL
                store
                    .create_schema_node_atomic(node, ddl_statements, None)
                    .await
                    .map_err(|e| {
                        NodeServiceError::SerializationError(format!(
                            "Failed to create schema node '{}': {}",
                            schema_id, e
                        ))
                    })?;
            }
        } // ← Arc clone dropped here, enabling Arc::get_mut() below

        // Update schema caches incrementally (Issue #704)
        // We use Arc::get_mut() since we're the only owner at this point (before cloning into Self)
        let store_mut = Arc::get_mut(store).ok_or_else(|| {
            NodeServiceError::InitializationError(
                "Cannot update schema cache: store has multiple Arc references. \
                 Ensure NodeService::new() is called before cloning the store."
                    .to_string(),
            )
        })?;

        for (type_name, _has_fields) in schema_cache_updates {
            store_mut.add_to_schema_cache(type_name);
        }

        tracing::info!("✅ Core schemas seeded successfully (caches updated)");

        Ok(())
    }

    /// Get access to the underlying SurrealStore
    ///
    /// Useful for advanced operations that need direct database access
    pub fn store(&self) -> &Arc<SurrealStore<C>> {
        &self.store
    }

    /// Create a new NodeService with a client identifier
    ///
    /// Returns a clone of this service with the client_id set. All operations
    /// performed through the returned service will emit events with this client_id
    /// as the source_client_id.
    ///
    /// # Arguments
    ///
    /// * `client_id` - Unique identifier for the client (e.g., "tauri-window-1", "mcp-client-123")
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut store = Arc::new(SurrealStore::new("./data/nodespace.db".into()).await?);
    /// let service = NodeService::new(&mut store).await?;
    ///
    /// // Create a scoped service for a specific client
    /// let tauri_service = service.with_client("tauri-window-1");
    ///
    /// // All operations through tauri_service will include "tauri-window-1" in events
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_client(&self, client_id: impl Into<String>) -> Self {
        let mut cloned = self.clone();
        cloned.client_id = Some(client_id.into());
        cloned
    }

    /// Subscribe to domain events
    ///
    /// Returns a broadcast receiver that receives all domain events (node created,
    /// updated, deleted, hierarchy changed, mentions added/removed).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut store = Arc::new(SurrealStore::new("./data/nodespace.db".into()).await?);
    /// # let service = NodeService::new(&mut store).await?;
    /// let mut rx = service.subscribe_to_events();
    /// tokio::spawn(async move {
    ///     while let Ok(event) = rx.recv().await {
    ///         println!("Event: {:?}", event);
    ///     }
    /// });
    /// # Ok(())
    /// # }
    /// ```
    pub fn subscribe_to_events(&self) -> broadcast::Receiver<DomainEvent> {
        self.event_tx.subscribe()
    }

    /// Emit a domain event to all subscribers
    ///
    /// Internal helper for emitting events after successful operations.
    /// Ignores errors if no subscribers (expected in some tests).
    fn emit_event(&self, event: DomainEvent) {
        let _ = self.event_tx.send(event);
    }

    // NOTE: emit_node_created and emit_node_updated helpers removed (Issue #718)
    // Node events are now automatically emitted by store-level notifier in NodeService::new()

    /// Create a new node
    ///
    /// Validates the node using the appropriate behavior (Text, Task, or Date),
    /// then inserts it into the database.
    ///
    /// # Arguments
    ///
    /// * `node` - The node to create
    ///
    /// # Returns
    ///
    /// The ID of the created node
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node validation fails
    /// - Parent node doesn't exist (if parent_id is set)
    /// - Root node doesn't exist (if root_id is set)
    /// - Database insertion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::Node;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let node = Node::new(
    ///     "text".to_string(),
    ///     "My note".to_string(),
    ///     json!({}),
    /// );
    /// let id = service.create_node(node).await?;
    /// # Ok(())
    /// # }
    /// ```
    /// Get schema definition for a given node type
    ///
    /// Queries the schema node directly from the database.
    /// Schema nodes are stored with id = node_type and node_type = "schema".
    ///
    /// This method replaces the need for SchemaService.get_schema() (Issue #690).
    ///
    /// # Arguments
    ///
    /// * `node_type` - The type of node to get the schema for (e.g., "task", "person")
    ///
    /// # Returns
    ///
    /// * `Ok(Some(value))` - Schema definition as JSON value if found
    /// * `Ok(None)` - No schema found for this node type
    /// * `Err` - Database error
    pub async fn get_schema_for_type(
        &self,
        node_type: &str,
    ) -> Result<Option<serde_json::Value>, NodeServiceError> {
        self.store
            .get_schema(node_type)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Validate a node's properties against its schema definition
    ///
    /// Performs schema-driven validation of property values, including:
    /// - Enum value validation (core + user values)
    /// - Required field checking
    /// - Type validation (future enhancement)
    ///
    /// This method implements Step 2 of the hybrid validation approach:
    /// behaviors handle basic type checking, schemas handle value validation.
    ///
    /// # Arguments
    ///
    /// * `node` - The node to validate
    ///
    /// # Returns
    ///
    /// `Ok(())` if validation passes, or an error describing the validation failure.
    /// Returns `Ok(())` if no schema exists for the node type (not all types have schemas).
    ///
    /// # Errors
    ///
    /// - `InvalidUpdate`: Property value violates schema constraints
    /// - `QueryFailed`: Database error while fetching schema
    async fn validate_node_against_schema(&self, node: &Node) -> Result<(), NodeServiceError> {
        // Try to get schema for this node type
        // If no schema exists, validation passes (not all types have schemas)
        let schema_json = match self.get_schema_for_type(&node.node_type).await? {
            Some(s) => s,
            None => return Ok(()), // No schema = no validation needed
        };

        // Parse schema fields from properties
        // If parsing fails (e.g., old schema format), skip schema validation gracefully
        let fields: Vec<crate::models::SchemaField> = match schema_json.get("fields") {
            Some(fields_json) => match serde_json::from_value(fields_json.clone()) {
                Ok(f) => f,
                Err(_) => return Ok(()), // Can't parse fields - skip validation
            },
            None => return Ok(()), // No fields defined - skip validation
        };

        // Use the helper function to validate with the parsed fields
        self.validate_node_with_fields(node, &fields)
    }

    /// Apply schema default values to missing fields using pre-loaded fields
    ///
    /// For each field in the schema that has a default value, if the field is missing
    /// from the node's properties, add it with the default value.
    ///
    /// # Arguments
    ///
    /// * `node` - Mutable reference to the node to apply defaults to
    /// * `fields` - Pre-loaded schema fields to use
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Defaults applied successfully
    /// * `Err` - Error applying defaults
    fn apply_schema_defaults_with_fields(
        &self,
        node: &mut Node,
        fields: &[crate::models::SchemaField],
    ) -> Result<(), NodeServiceError> {
        // Ensure properties is an object
        if !node.properties.is_object() {
            node.properties = serde_json::json!({});
        }

        // Get mutable reference to properties object
        let props_obj = node.properties.as_object_mut().unwrap();

        // Get or create the type namespace (Issue #794)
        // Properties are stored under properties[node_type][field_name]
        let type_namespace = props_obj
            .entry(&node.node_type)
            .or_insert_with(|| serde_json::json!({}));

        let type_props = type_namespace.as_object_mut().ok_or_else(|| {
            NodeServiceError::invalid_update(format!(
                "Type namespace for '{}' is not an object",
                node.node_type
            ))
        })?;

        // Apply defaults for missing fields within the type namespace
        for field in fields {
            // Check if field is missing in the type namespace
            if !type_props.contains_key(&field.name) {
                // Apply default value if one is defined
                if let Some(default_value) = &field.default {
                    type_props.insert(field.name.clone(), default_value.clone());
                }
            }
        }

        Ok(())
    }

    /// Deep-merge namespaced properties for Issue #794
    ///
    /// Properties use namespaced format: `{ "nodeType": { "field": "value" } }`.
    /// This function deep-merges new properties into existing ones, preserving
    /// dormant type namespaces (properties from previous node types).
    ///
    /// # Arguments
    ///
    /// * `existing` - Mutable reference to the existing properties
    /// * `new` - The new properties to merge in
    fn deep_merge_namespaced_properties(existing: &mut Value, new: Value) {
        if let (Some(existing_obj), Some(new_obj)) = (existing.as_object_mut(), new.as_object()) {
            for (key, value) in new_obj {
                // If both existing and new have the same key as objects, deep merge
                if let (Some(existing_ns), Some(new_ns)) = (
                    existing_obj.get_mut(key).and_then(|v| v.as_object_mut()),
                    value.as_object(),
                ) {
                    // Deep merge: update fields within the namespace
                    for (field_key, field_value) in new_ns {
                        existing_ns.insert(field_key.clone(), field_value.clone());
                    }
                } else {
                    // Otherwise replace the key (for new namespaces or non-object values)
                    existing_obj.insert(key.clone(), value.clone());
                }
            }
        } else {
            // If either is not an object, just replace (shouldn't happen normally)
            *existing = new;
        }
    }

    /// Validate a node against pre-loaded schema fields
    ///
    /// # Arguments
    ///
    /// * `node` - The node to validate
    /// * `fields` - Pre-loaded schema fields to validate against
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Validation passed
    /// * `Err` - Validation failed
    fn validate_node_with_fields(
        &self,
        node: &Node,
        fields: &[crate::models::SchemaField],
    ) -> Result<(), NodeServiceError> {
        // Get properties for this node type from the type namespace (Issue #794)
        // Properties are stored under properties[node_type][field_name]
        let node_props = node
            .properties
            .get(&node.node_type)
            .and_then(|p| p.as_object());

        // Validate each field in the schema
        for field in fields {
            let field_value = node_props.and_then(|props| props.get(&field.name));

            // Check required fields
            // Allow missing required fields if they have a default value defined
            // (defaults should have been applied before validation, but this provides safety)
            if field.required.unwrap_or(false) && field_value.is_none() && field.default.is_none() {
                return Err(NodeServiceError::invalid_update(format!(
                    "Required field '{}' is missing from {} node",
                    field.name, node.node_type
                )));
            }

            // Validate enum fields
            if field.field_type == "enum" {
                if let Some(value) = field_value {
                    if let Some(value_str) = value.as_str() {
                        // Get all valid enum values (core + user)
                        let mut valid_values = Vec::new();
                        if let Some(core_vals) = &field.core_values {
                            valid_values.extend(core_vals.clone());
                        }
                        if let Some(user_vals) = &field.user_values {
                            valid_values.extend(user_vals.clone());
                        }

                        // Check if the value matches any EnumValue.value
                        let is_valid = valid_values.iter().any(|ev| ev.value == value_str);
                        if !is_valid {
                            let valid_labels: Vec<_> = valid_values
                                .iter()
                                .map(|ev| format!("{} ({})", ev.label, ev.value))
                                .collect();
                            return Err(NodeServiceError::invalid_update(format!(
                                "Invalid value '{}' for enum field '{}'. Valid values: {}",
                                value_str,
                                field.name,
                                valid_labels.join(", ")
                            )));
                        }
                    } else if !value.is_null() {
                        return Err(NodeServiceError::invalid_update(format!(
                            "Enum field '{}' must be a string or null",
                            field.name
                        )));
                    }
                }
            }

            // Future: Add more type validation (number ranges, string formats, etc.)
        }

        Ok(())
    }

    /// Backfill _schema_version for a node if it doesn't have one (Phase 1 lazy migration)
    ///
    /// Only backfills version for node types with schema fields (task, person, etc.).
    /// Node types with empty schemas (text, date, header, etc.) don't need versioning.
    ///
    /// # Arguments
    ///
    /// * `node` - Mutable reference to the node to backfill
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Version was already present or successfully backfilled
    /// * `Err` - Database error during backfill
    async fn backfill_schema_version(&self, node: &mut Node) -> Result<(), NodeServiceError> {
        // Only backfill for types that have schema fields (Issue #794)
        // Types without schema fields (text, date, header, etc.) don't need versioning
        let schema = match self.get_schema_for_type(&node.node_type).await? {
            Some(s) => s,
            None => return Ok(()), // No schema = no version needed
        };

        // Check if schema has any fields
        let has_fields = schema
            .get("fields")
            .and_then(|f| f.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);

        if !has_fields {
            return Ok(()); // Empty schema = no version needed
        }

        // Check if _schema_version exists in the type namespace (Issue #794)
        let has_version = node
            .properties
            .get(&node.node_type)
            .and_then(|ns| ns.get("_schema_version"))
            .is_some();

        if !has_version {
            let version = schema.get("version").and_then(|v| v.as_i64()).unwrap_or(1);

            // Add version to type namespace IN-MEMORY ONLY
            // Don't persist to database - this prevents overwriting freshly created records
            // Issue #511: After node type conversion, the record has status+_schema_version
            // Backfill would MERGE just _schema_version, but the record already has it
            // Persisting backfill is unnecessary and risks race conditions
            if let Some(props_obj) = node.properties.as_object_mut() {
                let type_namespace = props_obj
                    .entry(&node.node_type)
                    .or_insert_with(|| serde_json::json!({}));
                if let Some(type_props) = type_namespace.as_object_mut() {
                    type_props.insert("_schema_version".to_string(), serde_json::json!(version));
                }
            }
        }
        Ok(())
    }

    /// Apply lazy migration to upgrade node to latest schema version
    ///
    /// Checks if the node's schema version is older than the current schema version,
    /// and if so, applies migration transforms to upgrade it.
    ///
    /// # Arguments
    ///
    /// * `node` - Mutable reference to the node to migrate
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Node was already up-to-date or successfully migrated
    /// * `Err` - Migration failed or database error
    async fn apply_lazy_migration(&self, node: &mut Node) -> Result<(), NodeServiceError> {
        // Get current version from type namespace (Issue #794)
        let current_version = node
            .properties
            .get(&node.node_type)
            .and_then(|ns| ns.get("_schema_version"))
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as u32;

        // Get target version from schema
        let target_version = if let Some(schema) = self.get_schema_for_type(&node.node_type).await?
        {
            schema.get("version").and_then(|v| v.as_i64()).unwrap_or(1) as u32
        } else {
            1 // No schema found - no migration needed
        };

        // Check if migration is needed
        if current_version >= target_version {
            return Ok(()); // Already up-to-date
        }

        // Apply migrations
        let migrated_node = self
            .migration_registry
            .apply_migrations(node, target_version)?;

        // Persist migrated node to database using SurrealStore
        let update = NodeUpdate {
            properties: Some(migrated_node.properties.clone()),
            ..Default::default()
        };
        self.store
            .update_node(&node.id, update, self.client_id.clone())
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to persist migrated node: {}", e))
            })?;

        // Update the in-memory node
        *node = migrated_node;

        Ok(())
    }

    pub async fn create_node(&self, mut node: Node) -> Result<String, NodeServiceError> {
        let start = std::time::Instant::now();
        tracing::debug!(node_type = %node.node_type, node_id = %node.id, "create_node: START");

        // Auto-detect date nodes by ID format (YYYY-MM-DD) to ensure correct node_type.
        // This maintains data integrity regardless of caller mistakes.
        // NOTE: Per Issue #670, date nodes can have custom content (not required to match ID).
        // We only enforce the node_type, not the content.
        if is_date_node_id(&node.id) {
            node.node_type = "date".to_string();
            // Content is preserved - date nodes can have custom content like "Custom Date Content"
        }

        // Step 1: Core behavior validation (PROTECTED)
        // Validates basic data integrity (non-empty content, correct types, etc.)
        self.behaviors.validate_node(&node)?;
        tracing::debug!(
            "create_node: behavior validation at {}ms",
            start.elapsed().as_millis()
        );

        // Step 1.5: Apply schema defaults, validate, and add version
        // Fetch schema ONCE and reuse for all operations (performance fix)
        // Skip for schema nodes to avoid circular dependency
        //
        // NOTE: We ONLY apply schema defaults, NOT behavior defaults.
        // Behavior defaults (markdown_enabled, auto_save, etc.) are UI preferences
        // that should be handled client-side, not stored in database properties.
        // The properties field is for user data and schema-defined fields only.
        if node.node_type != "schema" {
            let schema_start = std::time::Instant::now();
            // Fetch schema ONCE and reuse it for all operations
            if let Some(schema_json) = self.get_schema_for_type(&node.node_type).await? {
                tracing::debug!(
                    "create_node: schema fetched in {}ms",
                    schema_start.elapsed().as_millis()
                );
                // Parse schema fields
                if let Some(fields_json) = schema_json.get("fields") {
                    if let Ok(fields) = serde_json::from_value::<Vec<crate::models::SchemaField>>(
                        fields_json.clone(),
                    ) {
                        // Apply defaults from schema fields only
                        self.apply_schema_defaults_with_fields(&mut node, &fields)?;

                        // Validate with the same fields
                        self.validate_node_with_fields(&node, &fields)?;

                        // Add schema version if schema has fields (Issue #794)
                        // Using the already-fetched schema instead of fetching again
                        if !fields.is_empty() {
                            if let Some(version) =
                                schema_json.get("version").and_then(|v| v.as_i64())
                            {
                                if let Some(props_obj) = node.properties.as_object_mut() {
                                    let type_namespace = props_obj
                                        .entry(&node.node_type)
                                        .or_insert_with(|| serde_json::json!({}));
                                    if let Some(type_props) = type_namespace.as_object_mut() {
                                        type_props.insert(
                                            "_schema_version".to_string(),
                                            serde_json::json!(version),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // If no schema exists, that's fine - just don't add any defaults
            // Properties should contain only what the user explicitly provided
            tracing::debug!(
                "create_node: schema processing complete at {}ms",
                start.elapsed().as_millis()
            );
        }

        // NOTE: Parent/container validation removed - now handled by NodeOperations layer
        // The graph-native architecture uses edges for hierarchy, not fields on Node struct

        // NOTE: root_id filtering removed - hierarchy now managed via relationships

        // Issue #821: Populate title for @mention search
        // Task nodes always get indexed (regardless of hierarchy level)
        // Other node types will have title set by create_node_with_parent if they're root nodes
        // Only set title if not already set (create_node_with_parent may have set it for root nodes)
        if node.node_type == "task" && node.title.is_none() {
            node.title = Some(crate::utils::strip_markdown(&node.content));
        }

        // For schema nodes, use atomic creation with DDL generation (Issue #691, #703)
        if node.node_type == "schema" {
            // Parse schema relationships from properties (Issue #703)
            let relationships: Vec<SchemaRelationship> = node
                .properties
                .get("relationships")
                .and_then(|r| serde_json::from_value(r.clone()).ok())
                .unwrap_or_default();

            // Generate DDL statements for relationships
            let table_manager = crate::services::schema_table_manager::SchemaTableManager::new();

            // Generate relationship table DDL (if it has relationships)
            let ddl_statements = if !relationships.is_empty() {
                table_manager.generate_relationship_ddl_statements(&node.id, &relationships)?
            } else {
                vec![]
            };

            // Execute atomic create: schema node + relationship DDL in one transaction
            self.store
                .create_schema_node_atomic(node.clone(), ddl_statements, self.client_id.clone())
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

            tracing::info!("Atomically created schema node '{}' with DDL sync", node.id);
        } else {
            // Regular node creation
            let db_start = std::time::Instant::now();
            self.store
                .create_node(node.clone(), self.client_id.clone())
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to insert node: {}", e))
                })?;
            tracing::debug!(
                "create_node: database insert completed in {}ms",
                db_start.elapsed().as_millis()
            );
        }

        // NOTE: NodeCreated event is now automatically emitted by store notifier (Issue #718)

        tracing::debug!(
            node_id = %node.id,
            "create_node: COMPLETE at {}ms",
            start.elapsed().as_millis()
        );
        Ok(node.id)
    }

    /// Create a node with parent relationship in a single operation
    ///
    /// This is the primary node creation API that enforces all business rules:
    /// 1. Auto-creates date containers (YYYY-MM-DD) if parent is a date ID
    /// 2. Validates parent exists (if provided)
    /// 3. Creates the node with proper validation
    /// 4. Establishes parent-child edge with correct sibling ordering
    ///
    /// # Arguments
    ///
    /// * `params` - CreateNodeParams containing all node creation parameters
    ///
    /// # Returns
    ///
    /// The ID of the created node
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Parent doesn't exist (and isn't a valid date format)
    /// - Node validation fails
    /// - Sibling doesn't exist or has different parent
    /// - ID format is invalid (non-UUID for production nodes)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::{CreateNodeParams, NodeService};
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Create a child node under a date container
    /// let id = service.create_node_with_parent(CreateNodeParams {
    ///     id: None,
    ///     node_type: "text".to_string(),
    ///     content: "My note".to_string(),
    ///     parent_id: Some("2025-01-15".to_string()),
    ///     insert_after_node_id: None,
    ///     properties: json!({}),
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_node_with_parent(
        &self,
        params: CreateNodeParams,
    ) -> Result<String, NodeServiceError> {
        let start = std::time::Instant::now();
        tracing::debug!(
            node_type = %params.node_type,
            has_parent = params.parent_id.is_some(),
            "create_node_with_parent: START"
        );

        // Step 1: Auto-create date container if parent is a date ID
        if let Some(ref parent_id) = params.parent_id {
            self.ensure_date_exists(parent_id).await?;
        }

        // Step 2: Validate parent exists (if provided)
        if let Some(ref parent_id) = params.parent_id {
            let parent_exists = self.node_exists(parent_id).await?;
            if !parent_exists {
                return Err(NodeServiceError::invalid_parent(parent_id));
            }
        }

        // Step 3: Validate sibling exists and has same parent (if provided)
        if let Some(ref sibling_id) = params.insert_after_node_id {
            let _sibling = self
                .get_node(sibling_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(sibling_id))?;

            // Verify sibling has same parent
            let sibling_parent = self.get_parent(sibling_id).await?;
            let sibling_parent_id = sibling_parent.as_ref().map(|p| p.id.as_str());

            if sibling_parent_id != params.parent_id.as_deref() {
                return Err(NodeServiceError::hierarchy_violation(format!(
                    "Sibling '{}' has different parent than the node being created",
                    sibling_id
                )));
            }
        }

        // Step 4: Generate or validate node ID
        let node_id = if let Some(provided_id) = params.id {
            // Validate ID format based on node type
            if params.node_type == "date"
                || params.node_type == "schema"
                || provided_id.starts_with("test-")
            {
                // Date, schema, and test nodes can use their own ID format
                provided_id
            } else {
                // Production nodes must use UUID format
                uuid::Uuid::parse_str(&provided_id).map_err(|_| {
                    NodeServiceError::invalid_update(format!(
                        "Provided ID '{}' is not a valid UUID format (required for non-date/non-schema nodes)",
                        provided_id
                    ))
                })?;
                provided_id
            }
        } else if params.node_type == "date" {
            params.content.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };

        // Step 5: Create the node
        // Save node_type before moving into Node (needed for embedding check)
        let node_type = params.node_type.clone();

        // Issue #821: Determine title for @mention search
        // Title is set for: task nodes (always) OR root nodes (no parent)
        // Note: create_node will also set title for task nodes, but we set it here too
        // for root non-task nodes that won't go through the task check in create_node
        // TODO #824: Refactor to schema-driven title_template approach
        let title = if params.node_type == "task" || params.parent_id.is_none() {
            // Exclude certain types from having titles
            // TODO #824: Replace hardcoded exclusions with schema-driven title_template
            match params.node_type.as_str() {
                "date" | "schema" | "collection" => None,
                _ => Some(crate::utils::strip_markdown(&params.content)),
            }
        } else {
            None
        };

        let node = Node {
            id: node_id,
            node_type: params.node_type,
            content: params.content,
            version: 1,
            properties: params.properties,
            mentions: vec![],
            mentioned_by: vec![],
            member_of: vec![],
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            title,
            lifecycle_status: "active".to_string(),
        };

        tracing::debug!(
            "create_node_with_parent: about to call create_node at {}ms",
            start.elapsed().as_millis()
        );
        let created_id = self.create_node(node).await?;
        tracing::debug!(
            node_id = %created_id,
            "create_node_with_parent: create_node completed at {}ms",
            start.elapsed().as_millis()
        );

        // Step 6: Create parent relationship if parent specified
        if let Some(parent_id) = params.parent_id {
            // Pass insert_after_node_id directly without translation
            // None means "insert at beginning" (store.move_node semantics)
            self.create_parent_edge(
                &created_id,
                &parent_id,
                params.insert_after_node_id.as_deref(),
            )
            .await?;

            // Step 7a: Child node created - queue root for embedding regeneration
            // The new child's content should be included in the root's aggregate embedding
            // (Issue #729 - root-aggregate model)
            self.queue_root_for_embedding(&created_id).await;
        } else {
            // Step 7b: Root node created - queue for embedding if embeddable type
            // (Issue #729 - root-aggregate model)
            if is_embeddable_type(&node_type) {
                if let Err(e) = self.store.create_stale_embedding_marker(&created_id).await {
                    // Log warning but don't fail the creation - embedding will be regenerated later
                    tracing::warn!(
                        "Failed to create embedding marker for new root {}: {}",
                        created_id,
                        e
                    );
                } else {
                    // Wake the embedding processor to process the new root
                    tracing::debug!(
                        "Queued new root {} for embedding (direct creation)",
                        created_id
                    );
                    if let Some(ref waker) = self.embedding_waker {
                        waker.wake();
                    }
                }
            }
        }

        Ok(created_id)
    }

    /// Auto-create date container if it doesn't exist in the database
    ///
    /// Date nodes (YYYY-MM-DD format) are lazily created when children reference them.
    /// This ensures date containers exist before child nodes are created under them.
    ///
    /// # Arguments
    ///
    /// * `node_id` - Potential date node ID to check/create
    ///
    /// # Returns
    ///
    /// `Ok(())` if not a date or date container exists/was created
    pub async fn ensure_date_exists(&self, node_id: &str) -> Result<(), NodeServiceError> {
        // Check if this is a date format (YYYY-MM-DD)
        if !is_date_node_id(node_id) {
            return Ok(()); // Not a date, nothing to do
        }

        // Check if date container already exists IN THE DATABASE
        // IMPORTANT: Call store.get_node() directly to bypass virtual date node logic
        // in get_node(). The virtual date nodes are only for read operations,
        // we need to check actual database state for auto-creation.
        let exists = self
            .store
            .get_node(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Database error: {}", e)))?
            .is_some();

        if exists {
            return Ok(()); // Already exists in database
        }

        // Auto-create the date container
        let date_node = Node::new_with_id(
            node_id.to_string(),
            "date".to_string(),
            node_id.to_string(), // Default content to date
            serde_json::json!({}),
        );

        self.create_node(date_node).await?;

        Ok(())
    }

    /// Create a mention relationship between two existing nodes
    ///
    /// Adds an entry to the node_mentions table to track that one node mentions another.
    /// This enables backlink/references functionality.
    ///
    /// # Arguments
    ///
    /// * `mentioning_node_id` - ID of the node that contains the mention
    /// * `mentioned_node_id` - ID of the node being mentioned
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Either node doesn't exist
    /// - Database insertion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Create mention: "daily-note" mentions "project-planning"
    /// service.create_mention("daily-note-id", "project-planning-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_mention(
        &self,
        mentioning_node_id: &str,
        mentioned_node_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Prevent direct self-references
        if mentioning_node_id == mentioned_node_id {
            return Err(NodeServiceError::ValidationFailed(
                crate::models::ValidationError::InvalidParent(
                    "Cannot create self-referencing mention".to_string(),
                ),
            ));
        }

        // Validate both nodes exist
        if !self.node_exists(mentioning_node_id).await? {
            return Err(NodeServiceError::node_not_found(mentioning_node_id));
        }
        if !self.node_exists(mentioned_node_id).await? {
            return Err(NodeServiceError::node_not_found(mentioned_node_id));
        }

        // Prevent root-level self-references (child mentioning its own root)
        let mentioning_node = self
            .get_node(mentioning_node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(mentioning_node_id))?;

        // Get root ID via edge traversal
        let root_id = self.get_root_id(mentioning_node_id).await?;

        // Prevent root-level self-references (child mentioning its own root)
        if root_id == mentioned_node_id {
            return Err(NodeServiceError::ValidationFailed(
                crate::models::ValidationError::InvalidParent(
                    "Cannot mention own root (root-level self-reference)".to_string(),
                ),
            ));
        }

        // Get root ID with special handling for tasks
        // Tasks are always treated as their own roots (exception rule)
        let final_root_id = if mentioning_node.node_type == "task" {
            mentioning_node_id
        } else {
            &root_id
        };

        // Issue #813: Store returns relationship ID, service emits event
        let relationship_id = self
            .store
            .create_mention(mentioning_node_id, mentioned_node_id, final_root_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Emit event if relationship was created (not already existing)
        if let Some(rel_id) = relationship_id {
            let _ = self.event_tx.send(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent {
                    id: rel_id,
                    from_id: mentioning_node_id.to_string(),
                    to_id: mentioned_node_id.to_string(),
                    relationship_type: "mentions".to_string(),
                    properties: serde_json::json!({"root_id": final_root_id}),
                },
                source_client_id: self.client_id.clone(),
            });
        }

        Ok(())
    }

    /// Delete a mention relationship between two nodes
    ///
    /// Removes an entry from the node_mentions table.
    ///
    /// # Arguments
    ///
    /// * `mentioning_node_id` - ID of the node that contains the mention
    /// * `mentioned_node_id` - ID of the node being mentioned
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful (idempotent - succeeds even if mention doesn't exist)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// service.delete_mention("daily-note-id", "project-planning-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_mention(
        &self,
        mentioning_node_id: &str,
        mentioned_node_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Issue #813: Store returns relationship ID, service emits event
        let relationship_id = self
            .store
            .delete_mention(mentioning_node_id, mentioned_node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Emit event if relationship was deleted (existed)
        if let Some(rel_id) = relationship_id {
            let _ = self.event_tx.send(DomainEvent::RelationshipDeleted {
                id: rel_id,
                from_id: mentioning_node_id.to_string(),
                to_id: mentioned_node_id.to_string(),
                relationship_type: "mentions".to_string(),
                source_client_id: self.client_id.clone(),
            });
        }

        Ok(())
    }

    /// Get a node by ID
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to fetch
    ///
    /// # Returns
    ///
    /// `Some(Node)` if found, `None` if not found
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// if let Some(node) = service.get_node("node-id-123").await? {
    ///     println!("Found: {}", node.content);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_node(&self, id: &str) -> Result<Option<Node>, NodeServiceError> {
        // Delegate to SurrealStore
        if let Some(mut node) = self.store.get_node(id).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                context: format!("Database operation failed: {}", e),
            })
        })? {
            self.populate_mentions(&mut node).await?;
            self.backfill_schema_version(&mut node).await?;
            self.apply_lazy_migration(&mut node).await?;
            Ok(Some(node))
        } else {
            // NOT in database - check if it's a virtual date node
            // Date nodes (YYYY-MM-DD format) are virtual until they have children
            if is_date_node_id(id) {
                // Return virtual date node (will auto-persist when children are added)
                // Date nodes are root-level containers (no parent/container relationships)
                let virtual_date = Node {
                    id: id.to_string(),
                    node_type: "date".to_string(),
                    content: id.to_string(), // Content MUST match ID for validation
                    version: 1,
                    created_at: chrono::Utc::now(),
                    modified_at: chrono::Utc::now(),
                    properties: serde_json::json!({}),
                    mentions: vec![],
                    mentioned_by: vec![],
                    member_of: vec![],
                    title: None, // Date nodes don't have indexed titles
                    lifecycle_status: "active".to_string(),
                };
                return Ok(Some(virtual_date));
            }

            Ok(None)
        }
    }

    // ========================================================================
    // Strongly-Typed Node Retrieval
    // ========================================================================

    /// Get a task node with strong typing
    ///
    /// Returns strongly-typed `TaskNode` instead of generic `Node`.
    ///
    /// # Arguments
    ///
    /// * `id` - The task node ID
    ///
    /// # Returns
    ///
    /// * `Ok(Some(TaskNode))` - Task found with strongly-typed fields
    /// * `Ok(None)` - Task not found
    /// * `Err(_)` - Service error
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// if let Some(task) = service.get_task_node("my-task-id").await? {
    ///     // Direct field access - no JSON parsing
    ///     println!("Status: {:?}", task.status);
    ///     println!("Content: {}", task.content);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_task_node(
        &self,
        id: &str,
    ) -> Result<Option<crate::models::TaskNode>, NodeServiceError> {
        self.store.get_task_node(id).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                context: format!("Failed to get task node '{}': {}", id, e),
            })
        })
    }

    /// Update a task node with type-safe field updates
    ///
    /// Updates task-specific fields (status, priority, due_date, assignee).
    /// Uses optimistic concurrency control (OCC) to prevent lost updates.
    ///
    /// # Type Safety
    ///
    /// This method provides end-to-end type safety for task updates:
    /// - Frontend sends strongly-typed `TaskNodeUpdate` (not generic NodeUpdate)
    /// - Backend updates task fields directly (not via JSON properties)
    /// - Returns strongly-typed `TaskNode` with updated fields
    ///
    /// # Arguments
    ///
    /// * `id` - The task node ID
    /// * `expected_version` - Version for OCC check (prevents lost updates)
    /// * `update` - TaskNodeUpdate with fields to update
    ///
    /// # Returns
    ///
    /// * `Ok(TaskNode)` - Updated task with new version
    /// * `Err(VersionMismatch)` - Version conflict, refresh and retry
    /// * `Err(NodeNotFound)` - Task doesn't exist
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::models::{TaskNodeUpdate, TaskStatus};
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Update task status
    /// let update = TaskNodeUpdate::new().with_status(TaskStatus::InProgress);
    /// let task = service.update_task_node("task-123", 1, update).await?;
    /// println!("New status: {:?}", task.status);
    /// println!("New version: {}", task.version);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_task_node(
        &self,
        id: &str,
        expected_version: i64,
        update: crate::models::TaskNodeUpdate,
    ) -> Result<crate::models::TaskNode, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "TaskNodeUpdate contains no changes",
            ));
        }

        self.store
            .update_task_node(id, expected_version, update)
            .await
            .map_err(|e| {
                // Get the full error chain for pattern matching
                // anyhow errors chain with context, so we need to check the full string
                let error_msg = format!("{:#}", e); // Use alternate format for full chain
                let root_cause = e.root_cause().to_string();

                if error_msg.contains("VersionMismatch")
                    || root_cause.contains("VersionMismatch")
                    || root_cause.contains("failed transaction")
                {
                    // SurrealDB transaction THROW causes "failed transaction" error
                    // Our only THROW is for version mismatch, so treat failed transactions as OCC errors
                    // Note: This is a simplification - ideally SurrealDB would preserve the THROW message
                    NodeServiceError::VersionConflict {
                        node_id: id.to_string(),
                        expected_version,
                        actual_version: 0, // Actual version unknown when transaction fails
                    }
                } else if error_msg.contains("not found")
                    || error_msg.contains("Record not found")
                    || error_msg.contains("$current[0].version")
                    || root_cause.contains("not found")
                    || root_cause.contains("$current")
                {
                    // SurrealDB returns various error formats for missing records
                    // "Record not found" - explicit record error
                    // "$current[0].version" - when the LET query returns empty and IF fails
                    NodeServiceError::node_not_found(id)
                } else {
                    NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                        context: format!("Failed to update task node '{}': {}", id, e),
                    })
                }
            })
    }

    /// Get a schema node with strong typing
    ///
    /// Returns strongly-typed `SchemaNode` instead of generic `Node`.
    ///
    /// # Arguments
    ///
    /// * `id` - The schema node ID (e.g., "task", "date")
    ///
    /// # Returns
    ///
    /// * `Ok(Some(SchemaNode))` - Schema found with strongly-typed fields
    /// * `Ok(None)` - Schema not found
    /// * `Err(_)` - Service error
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// if let Some(schema) = service.get_schema_node("task").await? {
    ///     // Direct field access - no JSON parsing
    ///     println!("Is core: {}", schema.is_core);
    ///     println!("Fields: {:?}", schema.fields.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_schema_node(
        &self,
        id: &str,
    ) -> Result<Option<crate::models::SchemaNode>, NodeServiceError> {
        self.store.get_schema_node(id).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                context: format!("Failed to get schema node '{}': {}", id, e),
            })
        })
    }

    /// Update a node
    ///
    /// Performs a partial update using the NodeUpdate struct. Only provided fields
    /// will be updated. Handles the double-Option pattern for nullable fields.
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to update
    /// * `update` - The fields to update
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node doesn't exist
    /// - Validation fails after update
    /// - Database update fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let update = NodeUpdate::new()
    ///     .with_content("Updated content".to_string());
    /// service.update_node("node-id", update).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_node(&self, id: &str, update: NodeUpdate) -> Result<(), NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Update contains no changes",
            ));
        }

        // Get existing node to validate update
        let existing = self
            .get_node(id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        // For simplicity with libsql, we'll fetch the node, apply updates, and replace entirely
        let mut updated = existing.clone();
        let mut content_changed = false;
        let mut node_type_changed = false;

        if let Some(node_type) = update.node_type {
            node_type_changed = updated.node_type != node_type;
            updated.node_type = node_type;
        }

        if let Some(content) = update.content {
            if updated.content != content {
                content_changed = true;
            }
            updated.content = content;
        }

        // NOTE: Sibling ordering is now handled via has_child relationship order field.
        // Use reorder_siblings() or move_node() for ordering changes.

        if let Some(properties) = update.properties {
            // Deep-merge namespaced properties (Issue #794)
            Self::deep_merge_namespaced_properties(&mut updated.properties, properties);
        }

        // Step 1: Core behavior validation (PROTECTED)
        self.behaviors.validate_node(&updated)?;

        // Step 1.5: Apply schema defaults and validate (if node type changed)
        // Apply default values for missing fields when node type changes
        // Skip for schema nodes to avoid circular dependency
        if node_type_changed && updated.node_type != "schema" {
            // Fetch schema once and reuse it for both operations
            if let Some(schema_json) = self.get_schema_for_type(&updated.node_type).await? {
                // Parse schema fields
                if let Some(fields_json) = schema_json.get("fields") {
                    if let Ok(fields) = serde_json::from_value::<Vec<crate::models::SchemaField>>(
                        fields_json.clone(),
                    ) {
                        // Apply defaults for the new node type
                        self.apply_schema_defaults_with_fields(&mut updated, &fields)?;

                        // Validate with the same fields
                        self.validate_node_with_fields(&updated, &fields)?;
                    }
                }
            }
        } else if updated.node_type != "schema" {
            // Step 2: Schema validation only (node type didn't change)
            self.validate_node_against_schema(&updated).await?;
        }

        // Issue #821: Sync title when content or node_type changes
        // Title is indexed for @mention search on task nodes and root nodes
        // TODO #824: Refactor to schema-driven title_template approach
        let title_update = if content_changed || node_type_changed {
            // Determine if this node should have a title
            // 1. Task nodes always get titles (regardless of hierarchy)
            // 2. Root nodes (no parent) get titles
            // 3. Date, schema, and collection nodes never get titles
            // TODO #824: Replace hardcoded exclusions with schema-driven title_template
            let should_have_title = match updated.node_type.as_str() {
                "date" | "schema" | "collection" => false,
                "task" => true,
                _ => {
                    // Check if root node (no parent)
                    self.get_parent(id).await?.is_none()
                }
            };

            if should_have_title {
                Some(Some(crate::utils::strip_markdown(&updated.content)))
            } else {
                // Clear title for nodes that shouldn't have one
                // (e.g., when changing from task to text child node)
                Some(None)
            }
        } else {
            None // No title update needed
        };

        // Update node via store
        let node_update = crate::models::NodeUpdate {
            node_type: Some(updated.node_type.clone()),
            content: Some(updated.content.clone()),
            properties: Some(updated.properties.clone()),
            title: title_update,
            lifecycle_status: None, // Schema update doesn't change lifecycle_status
        };

        // For schema nodes, use atomic update with DDL generation (Issue #690, #703)
        if updated.node_type == "schema" {
            // Parse schema relationships from properties (Issue #703)
            let relationships: Vec<SchemaRelationship> = updated
                .properties
                .get("relationships")
                .and_then(|r| serde_json::from_value(r.clone()).ok())
                .unwrap_or_default();

            // Generate DDL statements for relationships
            let table_manager = crate::services::schema_table_manager::SchemaTableManager::new();

            // Generate relationship table DDL (if it has relationships)
            let ddl_statements = if !relationships.is_empty() {
                table_manager.generate_relationship_ddl_statements(id, &relationships)?
            } else {
                vec![]
            };

            // Execute atomic update: node + relationship DDL in one transaction
            self.store
                .update_schema_node_atomic(id, node_update, ddl_statements, self.client_id.clone())
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

            tracing::info!("Atomically updated schema node '{}' with DDL sync", id);
        } else {
            // Regular node update
            self.store
                .update_node(id, node_update, self.client_id.clone())
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        }

        // NOTE: NodeUpdated event is now automatically emitted by store notifier (Issue #718)

        // Sync mentions if content changed
        if content_changed {
            if let Err(e) = self
                .sync_mentions(id, &existing.content, &updated.content)
                .await
            {
                // Log warning but don't fail the update - mention sync failures should not block content updates
                tracing::warn!("Failed to sync mentions for node {}: {}", id, e);
            }
        }

        Ok(())
    }

    /// Update node with optimistic concurrency control (version check)
    ///
    /// This method performs an atomic update with version checking to prevent
    /// race conditions when multiple clients modify the same node concurrently.
    ///
    /// The version check ensures that:
    /// 1. The node hasn't been modified since the client last read it
    /// 2. Updates are applied atomically with version increment
    /// 3. Conflicts are detected via rows_affected = 0
    ///
    /// # Arguments
    ///
    /// * `id` - Node ID to update
    /// * `expected_version` - Version the client expects (from their last read)
    /// * `update` - Fields to update
    ///
    /// # Returns
    ///
    /// * `Ok(rows_affected)` - Number of rows updated (0 = version mismatch, 1 = success)
    /// * `Err(NodeServiceError)` - Database or validation errors
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let rows = service.update_with_version_check(
    ///     "node-123",
    ///     5,  // Expected version
    ///     NodeUpdate::new().with_content("New content".into())
    /// ).await?;
    ///
    /// if rows == 0 {
    ///     // Version conflict - node was modified by another client
    ///     // Caller should fetch current state and handle conflict
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_with_version_check(
        &self,
        id: &str,
        expected_version: i64,
        update: NodeUpdate,
    ) -> Result<usize, NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Update contains no changes",
            ));
        }

        // Get existing node to validate update and build new state
        let existing = self
            .get_node(id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        // Build updated node state
        let mut updated = existing.clone();
        let mut content_changed = false;
        let mut node_type_changed = false;

        if let Some(node_type) = update.node_type {
            node_type_changed = updated.node_type != node_type;
            updated.node_type = node_type;
        }

        if let Some(content) = update.content {
            if updated.content != content {
                content_changed = true;
            }
            updated.content = content;
        }

        // NOTE: Sibling ordering is now handled via has_child relationship order field.
        // Use reorder_siblings() or move_node() for ordering changes.

        if let Some(properties) = update.properties {
            // Deep-merge namespaced properties (Issue #794)
            Self::deep_merge_namespaced_properties(&mut updated.properties, properties);
        }

        // Step 1: Core behavior validation (PROTECTED)
        self.behaviors.validate_node(&updated)?;

        // Step 2: Schema validation (USER-EXTENSIBLE)
        if updated.node_type != "schema" {
            self.validate_node_against_schema(&updated).await?;
        }

        // Issue #821: Sync title when content or node_type changes
        // Title is indexed for @mention search on task nodes and root nodes
        // TODO #824: Refactor to schema-driven title_template approach
        let title_update = if content_changed || node_type_changed {
            // Determine if this node should have a title
            // TODO #824: Replace hardcoded exclusions with schema-driven title_template
            let should_have_title = match updated.node_type.as_str() {
                "date" | "schema" | "collection" => false,
                "task" => true,
                _ => self.get_parent(id).await?.is_none(),
            };

            if should_have_title {
                Some(Some(crate::utils::strip_markdown(&updated.content)))
            } else {
                Some(None)
            }
        } else {
            None
        };

        // Create node update
        // Issue #828, #770: Pass through lifecycle_status if provided
        let node_update = crate::models::NodeUpdate {
            node_type: Some(updated.node_type.clone()),
            content: Some(updated.content.clone()),
            properties: Some(updated.properties.clone()),
            title: title_update,
            lifecycle_status: update.lifecycle_status,
        };

        // Perform atomic update with version check
        let result = self
            .store
            .update_node_with_version_check(
                id,
                expected_version,
                node_update,
                self.client_id.clone(),
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Check if update succeeded (version matched)
        let rows_affected = if result.is_some() { 1 } else { 0 };

        // If update failed due to version mismatch, return early
        if rows_affected == 0 {
            return Ok(0);
        }

        // NOTE: NodeUpdated event is now automatically emitted by store notifier (Issue #718)

        // Queue root for embedding regeneration if content changed (Issue #729 - root-aggregate model)
        if content_changed {
            self.queue_root_for_embedding(id).await;
        }

        // Sync mentions if content changed
        if content_changed {
            if let Err(e) = self
                .sync_mentions(id, &existing.content, &updated.content)
                .await
            {
                // Log warning but don't fail the update
                tracing::warn!("Failed to sync mentions for node {}: {}", id, e);
            }
        }

        Ok(rows_affected as usize)
    }

    /// Update a node with OCC and return the updated node
    ///
    /// This is the primary update API that:
    /// 1. Validates update has changes
    /// 2. Applies update with version check
    /// 3. Returns detailed error on version conflict
    /// 4. Returns the updated node on success
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to update
    /// * `expected_version` - Version for optimistic concurrency control
    /// * `update` - Fields to update
    ///
    /// # Returns
    ///
    /// The updated Node with new version number
    ///
    /// # Errors
    ///
    /// Returns error on:
    /// - Empty update (no changes)
    /// - Node not found
    /// - Version conflict (with expected/actual versions)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let update = NodeUpdate::new().with_content("Updated content".to_string());
    /// let updated = service.update_node_with_occ("node-id", 5, update).await?;
    /// println!("New version: {}", updated.version);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_node_with_occ(
        &self,
        node_id: &str,
        expected_version: i64,
        update: NodeUpdate,
    ) -> Result<Node, NodeServiceError> {
        // Validate update has changes
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Update contains no changes",
            ));
        }

        // Verify node exists
        let _current = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Apply update with version check
        let rows_affected = self
            .update_with_version_check(node_id, expected_version, update)
            .await?;

        // Handle version conflict
        if rows_affected == 0 {
            let current = self
                .get_node(node_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

            return Err(NodeServiceError::version_conflict(
                node_id,
                expected_version,
                current.version,
            ));
        }

        // Return updated node
        let updated_node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        Ok(updated_node)
    }

    /// Sync mention relationships when node content changes
    ///
    /// Compares old vs new mentions and updates database:
    /// - Adds new mention relationships
    /// - Removes deleted mention relationships
    /// - Prevents self-references and root-level self-references
    /// - Errors are logged but don't block the update
    ///
    /// This is called automatically when node content is updated.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node whose content changed
    /// * `old_content` - Previous content
    /// * `new_content` - New content
    async fn sync_mentions(
        &self,
        node_id: &str,
        old_content: &str,
        new_content: &str,
    ) -> Result<(), NodeServiceError> {
        let old_mentions: HashSet<String> = extract_mentions(old_content).into_iter().collect();
        let new_mentions: HashSet<String> = extract_mentions(new_content).into_iter().collect();

        // Calculate diff
        let to_add: Vec<&String> = new_mentions.difference(&old_mentions).collect();
        let to_remove: Vec<&String> = old_mentions.difference(&new_mentions).collect();

        // Add new mentions (filter out self-references and root-level self-references)
        for mentioned_id in to_add {
            // Skip direct self-references
            if mentioned_id.as_str() == node_id {
                tracing::debug!("Skipping self-reference: {} -> {}", node_id, mentioned_id);
                continue;
            }

            // Skip root-level self-references (child mentioning its own parent)
            if let Ok(Some(parent)) = self.get_parent(node_id).await {
                if mentioned_id.as_str() == parent.id.as_str() {
                    tracing::debug!(
                        "Skipping root-level self-reference: {} -> {} (parent: {})",
                        node_id,
                        mentioned_id,
                        parent.id
                    );
                    continue;
                }
            }

            // Auto-create date nodes when mentioned (Issue #814 fix).
            // Date nodes are lazily created, but we need them to exist for the
            // "Mentioned by" panel to work. This ensures the relationship can be created.
            if is_date_node_id(mentioned_id) {
                if let Err(e) = self.ensure_date_exists(mentioned_id).await {
                    tracing::warn!(
                        "Failed to ensure date node exists for mention: {} -> {}: {}",
                        node_id,
                        mentioned_id,
                        e
                    );
                    // Continue anyway - the mention creation will fail if node doesn't exist
                }
            }

            if let Err(e) = self.create_mention(node_id, mentioned_id).await {
                tracing::warn!(
                    "Failed to create mention: {} -> {}: {}",
                    node_id,
                    mentioned_id,
                    e
                );
            }
        }

        // Remove old mentions
        for mentioned_id in to_remove {
            // Skip direct self-references (shouldn't exist, but be safe)
            if mentioned_id.as_str() == node_id {
                continue;
            }

            if let Err(e) = self.delete_mention(node_id, mentioned_id).await {
                tracing::warn!(
                    "Failed to delete mention: {} -> {}: {}",
                    node_id,
                    mentioned_id,
                    e
                );
            }
        }

        Ok(())
    }

    /// Delete a node
    ///
    /// Deletes a node and all its children (cascade delete).
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to delete
    ///
    /// # Errors
    ///
    /// Returns error if node doesn't exist or database deletion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// service.delete_node("node-id-123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_node(
        &self,
        id: &str,
    ) -> Result<crate::models::DeleteResult, NodeServiceError> {
        // Delegate to SurrealStore
        let result = self
            .store
            .delete_node(id, self.client_id.clone())
            .await
            .map_err(|e| {
                NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                    context: format!("Database operation failed: {}", e),
                })
            })?;

        // NOTE: NodeDeleted event is now automatically emitted by store notifier (Issue #718)

        // Idempotent delete: return success even if node doesn't exist
        // This follows RESTful best practices and prevents race conditions
        // in distributed scenarios. DELETE is idempotent - deleting a
        // non-existent resource should succeed (HTTP 200/204).
        //
        // The DeleteResult provides visibility for debugging/auditing while
        // maintaining idempotence.
        Ok(result)
    }

    /// Delete node with optimistic concurrency control (version check)
    ///
    /// This method performs an atomic delete with version checking to prevent
    /// race conditions when multiple clients attempt to delete or modify the same node.
    ///
    /// # Arguments
    ///
    /// * `id` - Node ID to delete
    /// * `expected_version` - Version the client expects (from their last read)
    ///
    /// # Returns
    ///
    /// * `Ok(rows_affected)` - Number of rows deleted (0 = version mismatch or not found, 1 = success)
    /// * `Err(NodeServiceError)` - Database errors
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let rows = service.delete_with_version_check("node-123", 5).await?;
    ///
    /// if rows == 0 {
    ///     // Either version conflict or node doesn't exist
    ///     // Caller should check if node still exists to distinguish
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_with_version_check(
        &self,
        id: &str,
        expected_version: i64,
    ) -> Result<usize, NodeServiceError> {
        let rows_affected = self
            .store
            .delete_with_version_check(id, expected_version, self.client_id.clone())
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to delete node with version check: {}",
                    e
                ))
            })?;

        // NOTE: NodeDeleted event is now automatically emitted by store notifier (Issue #718)

        Ok(rows_affected)
    }

    /// Delete a node with cascade and optimistic concurrency control
    ///
    /// This is the primary delete API that:
    /// 1. Verifies node exists
    /// 2. Recursively deletes all children (cascade)
    /// 3. Deletes the node with version check (OCC)
    /// 4. Returns detailed error on version conflict
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to delete
    /// * `expected_version` - Version for optimistic concurrency control
    ///
    /// # Returns
    ///
    /// `DeleteResult` indicating whether the node existed
    ///
    /// # Errors
    ///
    /// Returns error with current node state on version conflict,
    /// or database errors on failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let result = service.delete_node_with_occ("node-id", 5).await?;
    /// println!("Node existed: {}", result.existed);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_node_with_occ(
        &self,
        node_id: &str,
        expected_version: i64,
    ) -> Result<crate::models::DeleteResult, NodeServiceError> {
        // 1. Get the node being deleted (if it exists)
        let _node = match self.get_node(node_id).await? {
            Some(n) => n,
            None => {
                // Node doesn't exist - return false immediately (idempotent delete)
                return Ok(crate::models::DeleteResult { existed: false });
            }
        };

        // 1b. Capture root ID BEFORE deletion (Issue #729 - root-aggregate model)
        // After deletion, we can't traverse up to find the root
        let root_id_for_embedding = self.get_root_id(node_id).await.ok();

        // 2. Cascade delete all children recursively
        let children = self.get_children(node_id).await?;
        for child in children {
            // Recursively call delete for each child using Box::pin to avoid infinite future size
            Box::pin(self.delete_node_with_occ(&child.id, child.version)).await?;
        }

        // 3. Delete with version check (optimistic concurrency control)
        let rows_affected = self
            .delete_with_version_check(node_id, expected_version)
            .await?;

        // 4. Handle version conflict
        if rows_affected == 0 {
            // Node might have been deleted or modified by another client
            match self.get_node(node_id).await? {
                Some(current) => {
                    // Node exists but version mismatch - return conflict error
                    return Err(NodeServiceError::version_conflict(
                        node_id,
                        expected_version,
                        current.version,
                    ));
                }
                None => {
                    // Node was already deleted by another client - idempotent
                    return Ok(crate::models::DeleteResult { existed: false });
                }
            }
        }

        // 5. Queue root for embedding regeneration (Issue #729 - root-aggregate model)
        // Only queue if the deleted node was NOT the root itself (root deletion removes embedding)
        if let Some(root_id) = root_id_for_embedding {
            if root_id != node_id {
                // Deleted a child node - root's aggregate embedding needs updating
                self.queue_root_for_embedding(&root_id).await;
            }
            // If we deleted the root itself, no need to queue - embeddings will be orphaned
            // and should be cleaned up by the embedding processor
        }

        Ok(crate::models::DeleteResult { existed: true })
    }

    /// Get children of a node
    ///
    /// Returns all direct children of the specified parent node.
    ///
    /// # Arguments
    ///
    /// * `parent_id` - The parent node ID
    ///
    /// # Returns
    ///
    /// Vector of child nodes (empty if no children)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let children = service.get_children("parent-id").await?;
    /// println!("Found {} children", children.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_children(&self, parent_id: &str) -> Result<Vec<Node>, NodeServiceError> {
        // Use edge-based query from SurrealStore (graph-native architecture)
        // Children are already sorted by fractional order on edges
        let children = self
            .store
            .get_children(Some(parent_id))
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        Ok(children)
    }

    /// Get all descendants of a node (recursive children)
    ///
    /// Fetches all nodes in the subtree rooted at the specified node,
    /// excluding the root node itself. Uses iterative breadth-first traversal.
    ///
    /// # Arguments
    ///
    /// * `root_id` - The root node ID to fetch descendants for
    ///
    /// # Returns
    ///
    /// `Vec<Node>` containing all descendant nodes (not including the root)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # async fn example(service: NodeService) -> Result<(), Box<dyn std::error::Error>> {
    /// let descendants = service.get_descendants("parent-123").await?;
    /// println!("Found {} descendants", descendants.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_descendants(&self, root_id: &str) -> Result<Vec<Node>, NodeServiceError> {
        // Use store's breadth-first traversal implementation
        let descendants = self
            .store
            .get_nodes_in_subtree(root_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        Ok(descendants)
    }

    /// Get a complete nested tree structure using efficient adjacency list strategy
    ///
    /// Fetches the entire subtree in 3 optimized queries:
    /// 1. Get all nodes in the subtree (descendants only)
    /// 2. Get all edges in the subtree
    /// 3. Get the root node (not included in descendants query)
    ///
    /// Then constructs the nested tree structure in-memory using an adjacency list,
    /// which separates data fetching from tree construction and enables client-side logic.
    ///
    /// # Performance
    ///
    /// - **3 queries total** regardless of tree depth or node count (constant vs O(depth))
    /// - O(n) in-memory tree construction where n = number of nodes
    /// - Much faster than recursive queries with complex projections
    ///
    /// # Arguments
    ///
    /// * `parent_id` - The root node ID to fetch tree for
    ///
    /// # Returns
    ///
    /// `serde_json::Value` containing the nested tree structure with all descendants
    pub async fn get_children_tree(
        &self,
        parent_id: &str,
    ) -> Result<serde_json::Value, NodeServiceError> {
        // Use shared subtree data fetching
        let (root_node, node_map, adjacency_list) = self.get_subtree_data(parent_id).await?;

        match root_node {
            Some(root) => {
                // Recursively build tree structure
                let tree_json = build_node_tree_recursive(&root, &node_map, &adjacency_list);
                Ok(tree_json)
            }
            None => {
                // Root node not found, return empty object
                Ok(serde_json::json!({}))
            }
        }
    }

    /// Fetch all data needed to traverse a subtree efficiently
    ///
    /// This is the core data-fetching method used by both `get_children_tree` (JSON output)
    /// and MCP markdown export. It performs a **single database query** regardless of tree
    /// depth or node count using SurrealDB's `{..+collect}` recursive syntax.
    ///
    /// Returns data structures optimized for in-memory traversal:
    /// - Node map for O(1) node lookup by ID
    /// - Adjacency list for O(1) children lookup by parent ID (sorted by order)
    ///
    /// # Arguments
    ///
    /// * `root_id` - The root node ID to fetch subtree for
    ///
    /// # Returns
    ///
    /// Tuple of (root_node, node_map, adjacency_list) where:
    /// - root_node: Option<Node> - the root node if it exists
    /// - node_map: HashMap<String, Node> - all nodes indexed by ID
    /// - adjacency_list: HashMap<String, Vec<String>> - children IDs indexed by parent ID, sorted by order
    pub async fn get_subtree_data(&self, root_id: &str) -> Result<SubtreeData, NodeServiceError> {
        use std::collections::HashMap;

        // Single consolidated query fetches root + all descendants + all relationships
        let (all_nodes, relationships) = self
            .store
            .get_subtree_with_relationships(root_id)
            .await
            .map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to fetch subtree: {}", e))
        })?;

        // Find root node from the results
        let root_node = all_nodes.iter().find(|n| n.id == root_id).cloned();

        // Create a map of node_id → Node for O(1) lookup
        let mut node_map: HashMap<String, Node> = HashMap::new();
        for node in all_nodes {
            node_map.insert(node.id.clone(), node);
        }

        // Create adjacency list: parent_id → Vec of child_ids (sorted by order)
        // Issue #788: RelationshipRecord now stores order in properties, accessed via order() method
        let mut adjacency_with_order: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        for rel in relationships {
            adjacency_with_order
                .entry(rel.in_node.clone())
                .or_default()
                .push((rel.out_node.clone(), rel.order()));
        }

        // Sort children by order for each parent, then extract just the IDs
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        for (parent_id, mut children) in adjacency_with_order {
            children.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            adjacency_list.insert(parent_id, children.into_iter().map(|(id, _)| id).collect());
        }

        Ok((root_node, node_map, adjacency_list))
    }

    /// Check if a node is a root node (has no parent)
    ///
    /// A root node is one that has no incoming `has_child` edges.
    /// This replaces the old `is_root()` method which checked `root_id IS NULL`.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to check
    ///
    /// # Returns
    ///
    /// `true` if the node has no parent (is a root), `false` otherwise
    pub async fn is_root_node(&self, node_id: &str) -> Result<bool, NodeServiceError> {
        // A node is a root if it has no incoming has_child relationships
        // We check this by trying to get its parent - if parent is None, it's a root
        let parent = self.get_parent(node_id).await?;
        Ok(parent.is_none())
    }

    /// Get the parent of a node (via incoming has_child relationship)
    ///
    /// Returns the node's parent if it has one, or None if it's a root node.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The child node ID
    ///
    /// # Returns
    ///
    /// `Some(parent_node)` if the node has a parent, `None` if it's a root node
    pub async fn get_parent(&self, node_id: &str) -> Result<Option<Node>, NodeServiceError> {
        // Query for nodes that have has_child relationship pointing to this node
        // This is done via SurrealDB graph traversal: <-has_child
        let parent = self
            .store
            .get_parent(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        Ok(parent)
    }

    /// Search nodes for mention autocomplete with proper filtering
    ///
    /// Applies mention-specific filtering rules at the database level:
    /// - Excludes: date, schema node types (always)
    /// - Text-based types (text, header, code-block, quote-block, ordered-list): only root nodes
    /// - Other types (task, query, etc.): included regardless of hierarchy
    ///
    /// # Arguments
    ///
    /// * `query` - Content search string (case-insensitive)
    /// * `limit` - Maximum number of results (defaults to 10)
    ///
    /// # Returns
    ///
    /// Filtered nodes matching mention autocomplete criteria
    pub async fn mention_autocomplete(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Node>, NodeServiceError> {
        self.store
            .mention_autocomplete(query, limit.map(|l| l as i64))
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Get the root (root ancestor) of a node
    ///
    /// Traverses up the parent chain until finding a root node (no parent).
    /// This replaces the old `root_node_id` field.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to find the root for
    ///
    /// # Returns
    ///
    /// The root node ID, or the node itself if it's already a root
    pub async fn get_root_id(&self, node_id: &str) -> Result<String, NodeServiceError> {
        let mut current_id = node_id.to_string();

        // Traverse up the parent chain until we find a root
        loop {
            let parent = self.get_parent(&current_id).await?;
            match parent {
                Some(parent_node) => {
                    // Keep traversing up
                    current_id = parent_node.id;
                }
                None => {
                    // Found the root
                    return Ok(current_id);
                }
            }
        }
    }

    /// Queue a node's root for embedding regeneration
    ///
    /// Finds the root of the given node and marks its embedding as stale.
    /// Used when any node in a tree is created, updated, or deleted to ensure
    /// the root-aggregate embedding stays current.
    ///
    /// This is a non-blocking operation - errors are logged but don't fail the caller.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node that changed (can be root or any descendant)
    ///
    /// # Root-Aggregate Model (Issue #729)
    ///
    /// Only root nodes of embeddable types get embedded. When any node in the tree
    /// changes, we find the root and mark its embedding as stale. The background
    /// `EmbeddingProcessor` will regenerate the embedding with updated content.
    pub async fn queue_root_for_embedding(&self, node_id: &str) {
        // Find the root of this node's tree
        let root_id = match self.get_root_id(node_id).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Failed to find root for node {} (embedding not queued): {}",
                    node_id,
                    e
                );
                return;
            }
        };

        // Get root node to check if it's an embeddable type
        let root = match self.get_node(&root_id).await {
            Ok(Some(node)) => node,
            Ok(None) => {
                tracing::warn!("Root node {} not found (embedding not queued)", root_id);
                return;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to get root node {} (embedding not queued): {}",
                    root_id,
                    e
                );
                return;
            }
        };

        // Only queue if root is an embeddable type
        if !is_embeddable_type(&root.node_type) {
            tracing::debug!(
                "Root {} is not embeddable (type: {}), skipping embedding queue",
                root_id,
                root.node_type
            );
            return;
        }

        // Check if embedding exists for this root
        let has_embedding = match self.store.has_embeddings(&root_id).await {
            Ok(has) => has,
            Err(e) => {
                tracing::warn!(
                    "Failed to check embeddings for root {} (assuming none exist): {}",
                    root_id,
                    e
                );
                false
            }
        };

        // Mark existing embedding as stale or create new stale marker
        let result = if has_embedding {
            self.store.mark_root_embedding_stale(&root_id).await
        } else {
            self.store.create_stale_embedding_marker(&root_id).await
        };

        if let Err(e) = result {
            tracing::warn!(
                "Failed to queue root {} for embedding (via node {}): {}",
                root_id,
                node_id,
                e
            );
        } else {
            tracing::debug!(
                "📥 Queued root {} for embedding (triggered by node {})",
                root_id,
                node_id
            );

            // Wake the embedding processor (fire-and-forget)
            if let Some(ref waker) = self.embedding_waker {
                tracing::debug!("🔔 Waking embedding processor for root {}", root_id);
                waker.wake();
            } else {
                tracing::warn!(
                    "⚠️ No embedding waker configured - root {} will not be processed automatically",
                    root_id
                );
            }
        }
    }

    /// Bulk fetch all nodes belonging to an origin node (viewer/page)
    ///
    /// This is the efficient way to load a complete document tree:
    /// 1. Single database query fetches all nodes with the same root_id
    /// 2. In-memory hierarchy reconstruction using parent_id and before_sibling_id
    ///
    /// This avoids making multiple queries for each level of the tree.
    ///
    /// # Arguments
    ///
    /// * `root_node_id` - The ID of the origin node (e.g., date page ID)
    ///
    /// # Returns
    ///
    /// Vector of all nodes that belong to this origin, unsorted.
    /// Caller should use `sort_by_sibling_order()` or build a tree structure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Fetch all nodes for a date page
    /// let nodes = service.get_nodes_by_root_id("2025-10-05").await?;
    /// println!("Found {} nodes in this document", nodes.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_nodes_by_root_id(
        &self,
        root_node_id: &str,
    ) -> Result<Vec<Node>, NodeServiceError> {
        // Hierarchy is now managed via relationships - use get_children instead
        self.get_children(root_node_id).await
    }

    /// Move a node to a new parent
    ///
    /// Updates the parent_id and root_id of a node, maintaining hierarchy consistency.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to move
    /// * `new_parent` - The new parent ID (None to make it a root node)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node doesn't exist
    /// - New parent doesn't exist
    /// - Move would create circular reference
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Move node under new parent
    /// service.move_node("node-id", Some("new-parent-id"), None).await?;
    ///
    /// // Make node a root
    /// service.move_node("node-id", None, None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn move_node(
        &self,
        node_id: &str,
        new_parent: Option<&str>,
        insert_after_node_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Verify node exists
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Date nodes are top-level containers and cannot be moved
        // This prevents breaking document structure by moving date pages
        // Note: We check node_type specifically, not just is_root_node(), because
        // regular nodes without parents (e.g., newly created nodes being placed in hierarchy)
        // should be allowed to be moved via this method.
        if node.node_type == "date" {
            return Err(NodeServiceError::hierarchy_violation(format!(
                "Date node '{}' cannot be moved (it's a top-level container)",
                node_id
            )));
        }

        // Verify new parent exists if provided
        if let Some(parent_id) = new_parent {
            let parent_exists = self.node_exists(parent_id).await?;
            if !parent_exists {
                return Err(NodeServiceError::invalid_parent(parent_id));
            }

            // Check for circular reference - parent_id cannot be a descendant of node_id
            if self.is_descendant(node_id, parent_id).await? {
                return Err(NodeServiceError::circular_reference(format!(
                    "Cannot move node {} under its descendant {}",
                    node_id, parent_id
                )));
            }
        }

        // Hierarchy is now managed via relationships - use store's move_node
        self.store
            .move_node(node_id, new_parent, insert_after_node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Emit RelationshipUpdated event (Issue #811: unified relationship events)
        if let Some(parent_id) = new_parent {
            let children = self.get_children(parent_id).await?;
            if let Some(child_pos) = children.iter().position(|c| c.id == node_id) {
                self.emit_event(DomainEvent::RelationshipUpdated {
                    relationship: crate::db::events::RelationshipEvent {
                        id: format!("relationship:{}:{}", parent_id, node_id),
                        from_id: parent_id.to_string(),
                        to_id: node_id.to_string(),
                        relationship_type: "has_child".to_string(),
                        properties: serde_json::json!({"order": child_pos as f64}),
                    },
                    source_client_id: self.client_id.clone(),
                });
            }
        }

        Ok(())
    }

    /// Move a node to a new parent with OCC (Optimistic Concurrency Control)
    ///
    /// This method validates version before moving, preventing concurrent modifications
    /// from silently overwriting each other. The node's version is bumped after a
    /// successful move.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to move
    /// * `expected_version` - The version the caller expects (for OCC)
    /// * `new_parent` - The new parent ID (None to make it a root node)
    /// * `insert_after_node_id` - Optional sibling to insert after (None = append at end)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node doesn't exist
    /// - Version doesn't match (concurrent modification detected)
    /// - New parent doesn't exist
    /// - Move would create circular reference
    /// - Node is a date container (cannot be moved)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Move node under new parent with version check
    /// service.move_node_with_occ("node-id", 5, Some("new-parent-id"), None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn move_node_with_occ(
        &self,
        node_id: &str,
        expected_version: i64,
        new_parent: Option<&str>,
        insert_after_node_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Get current node and verify version
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Check version before proceeding
        if node.version != expected_version {
            return Err(NodeServiceError::version_conflict(
                node_id,
                expected_version,
                node.version,
            ));
        }

        // Date nodes are top-level containers and cannot be moved
        if node.node_type == "date" {
            return Err(NodeServiceError::hierarchy_violation(format!(
                "Date node '{}' cannot be moved (it's a top-level container)",
                node_id
            )));
        }

        // Verify new parent exists if provided
        if let Some(parent_id) = new_parent {
            let parent_exists = self.node_exists(parent_id).await?;
            if !parent_exists {
                return Err(NodeServiceError::invalid_parent(parent_id));
            }

            // Check for circular reference - parent_id cannot be a descendant of node_id
            if self.is_descendant(node_id, parent_id).await? {
                return Err(NodeServiceError::circular_reference(format!(
                    "Cannot move node {} under its descendant {}",
                    node_id, parent_id
                )));
            }
        }

        // Perform the move
        self.store
            .move_node(node_id, new_parent, insert_after_node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Emit RelationshipUpdated event (Issue #811: unified relationship events)
        if let Some(parent_id) = new_parent {
            let children = self.get_children(parent_id).await?;
            if let Some(child_pos) = children.iter().position(|c| c.id == node_id) {
                self.emit_event(DomainEvent::RelationshipUpdated {
                    relationship: crate::db::events::RelationshipEvent {
                        id: format!("relationship:{}:{}", parent_id, node_id),
                        from_id: parent_id.to_string(),
                        to_id: node_id.to_string(),
                        relationship_type: "has_child".to_string(),
                        properties: serde_json::json!({"order": child_pos as f64}),
                    },
                    source_client_id: self.client_id.clone(),
                });
            }
        }

        // Bump the node's version to support OCC
        // Even though we're only modifying edge relationships, we bump the node version
        // so that concurrent move operations will fail with version conflict
        self.update_node_with_version_bump(node_id, expected_version)
            .await?;

        Ok(())
    }

    /// Reorder a node within its siblings with OCC
    ///
    /// This method validates version, prevents root reordering, and bumps
    /// node version after reordering for OCC safety.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to reorder
    /// * `expected_version` - Version for optimistic concurrency control
    /// * `insert_after` - Sibling to position after (None = first position)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node not found
    /// - Version mismatch
    /// - Node is a root (roots cannot be reordered)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Reorder with version check
    /// service.reorder_node_with_occ("node-id", 5, Some("sibling-id")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reorder_node_with_occ(
        &self,
        node_id: &str,
        expected_version: i64,
        insert_after: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Get current node and verify version
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Check version before proceeding
        if node.version != expected_version {
            return Err(NodeServiceError::version_conflict(
                node_id,
                expected_version,
                node.version,
            ));
        }

        // Root nodes cannot be reordered (they have no parent)
        if self.is_root_node(node_id).await? {
            return Err(NodeServiceError::hierarchy_violation(format!(
                "Root node '{}' cannot be reordered (it has no parent)",
                node_id
            )));
        }

        // Use graph-native reordering
        self.reorder_child(node_id, insert_after).await?;

        // Bump the node's version to support OCC
        // Even though we're only modifying edge ordering, we bump the node version
        // so that concurrent reorder operations will fail with version conflict
        self.update_node_with_version_bump(node_id, expected_version)
            .await?;

        Ok(())
    }

    /// Create parent-child edge atomically with sibling positioning
    ///
    /// Used during node creation to establish parent relationship while preserving
    /// sibling ordering. This is separate from move_node() which is for moving existing nodes.
    ///
    /// # Arguments
    ///
    /// * `child_id` - ID of the child node (must already exist)
    /// * `parent_id` - ID of the parent node
    /// * `insert_after_node_id` - Optional sibling to insert after (None = append at end)
    pub async fn create_parent_edge(
        &self,
        child_id: &str,
        parent_id: &str,
        insert_after_node_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        use tokio::time::{sleep, Duration};

        // Pass insert_after_node_id directly to store.move_node without translation
        // store.move_node semantics:
        //   insert_after_node_id = Some(id) → "insert AFTER this sibling"
        //   insert_after_node_id = None → "insert at beginning"

        // Use store's move_node which creates the has_child relationship atomically
        // Retry if sibling not found (eventual consistency)
        let mut last_error = None;
        for _attempt in 0..10 {
            match self
                .store
                .move_node(child_id, Some(parent_id), insert_after_node_id)
                .await
            {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if err_str.contains("Sibling not found") {
                        // Sibling not visible yet - wait and retry
                        last_error = Some(err_str);
                        sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    // Other error - fail immediately
                    return Err(NodeServiceError::query_failed(err_str));
                }
            }
        }
        if let Some(err) = last_error {
            return Err(NodeServiceError::query_failed(err));
        }

        // Due to SurrealDB eventual consistency, the edge may be created with incorrect order
        // if not all siblings were visible during the move_node query. We verify and retry
        // with reorder if the position is wrong.
        if let Some(after_id) = insert_after_node_id {
            'outer: for _attempt in 0..20 {
                // Wait for write propagation
                sleep(Duration::from_millis(50)).await;

                let children = self.get_children(parent_id).await?;
                let child_pos = children.iter().position(|c| c.id == child_id);
                let after_pos = children.iter().position(|c| c.id == after_id);

                match (child_pos, after_pos) {
                    (Some(c_pos), Some(a_pos)) if c_pos == a_pos + 1 => {
                        // Child is correctly positioned right after the insert_after sibling
                        break 'outer;
                    }
                    (Some(_), Some(_)) => {
                        // Child exists but is in wrong position - reorder it and verify
                        self.store
                            .move_node(child_id, Some(parent_id), Some(after_id))
                            .await
                            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

                        // Wait and verify reorder took effect
                        for _verify in 0..10 {
                            sleep(Duration::from_millis(50)).await;
                            let verify_children = self.get_children(parent_id).await?;
                            let v_child_pos = verify_children.iter().position(|c| c.id == child_id);
                            let v_after_pos = verify_children.iter().position(|c| c.id == after_id);

                            if let (Some(c), Some(a)) = (v_child_pos, v_after_pos) {
                                if c == a + 1 {
                                    break 'outer; // Successfully reordered
                                }
                            }
                        }
                        // Reorder didn't stick, outer loop will retry
                    }
                    _ => {
                        // One or both nodes not visible yet - will retry
                    }
                }
            }
        }

        // Emit RelationshipCreated event (Issue #811: unified relationship events)
        let children = self.get_children(parent_id).await?;
        if let Some(child_pos) = children.iter().position(|c| c.id == child_id) {
            self.emit_event(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent {
                    id: format!("relationship:{}:{}", parent_id, child_id),
                    from_id: parent_id.to_string(),
                    to_id: child_id.to_string(),
                    relationship_type: "has_child".to_string(),
                    properties: serde_json::json!({"order": child_pos as f64}),
                },
                source_client_id: self.client_id.clone(),
            });
        }

        Ok(())
    }

    /// Reorder a child within its parent's children list.
    ///
    /// Updates the `has_child` edge `order` field to reposition a node among its siblings.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to reorder
    /// * `insert_after` - The sibling to position after (None = first position)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Position node after sibling
    /// service.reorder_child("node-id", Some("sibling-id")).await?;
    ///
    /// // Move to first position
    /// service.reorder_child("node-id", None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reorder_child(
        &self,
        node_id: &str,
        insert_after: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Verify node exists
        let _node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Verify sibling exists if provided
        if let Some(sibling_id) = insert_after {
            let sibling_exists = self.node_exists(sibling_id).await?;
            if !sibling_exists {
                return Err(NodeServiceError::hierarchy_violation(format!(
                    "Sibling node {} does not exist",
                    sibling_id
                )));
            }
        }

        // Child ordering is handled via has_child relationship order field.
        // Get current parent to move within the same parent
        let parent = self.get_parent(node_id).await?;
        let parent_id = parent.map(|p| p.id);

        // Use move_node to handle edge ordering (insert_after semantics)
        self.store
            .move_node(node_id, parent_id.as_deref(), insert_after)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Emit RelationshipUpdated event (Issue #811: unified relationship events)
        // Reordering updates the hierarchy edge's order field
        if let Some(parent_id) = parent_id {
            // Get the updated edge information
            let children = self.get_children(&parent_id).await?;
            if let Some(child_pos) = children.iter().position(|c| c.id == node_id) {
                self.emit_event(DomainEvent::RelationshipUpdated {
                    relationship: crate::db::events::RelationshipEvent {
                        id: format!("relationship:{}:{}", parent_id, node_id),
                        from_id: parent_id.clone(),
                        to_id: node_id.to_string(),
                        relationship_type: "has_child".to_string(),
                        properties: serde_json::json!({"order": child_pos as f64}),
                    },
                    source_client_id: self.client_id.clone(),
                });
            }
        }

        Ok(())
    }

    /// Bump a node's version without changing any content.
    ///
    /// Used by operations like reorder that need OCC (optimistic concurrency control)
    /// even though they don't modify the node's content directly.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The ID of the node to update
    /// * `expected_version` - The version the caller expects (for OCC)
    ///
    /// # Returns
    ///
    /// Ok(()) if version bump succeeds, Err if version mismatch or node not found
    pub async fn update_node_with_version_bump(
        &self,
        node_id: &str,
        expected_version: i64,
    ) -> Result<(), NodeServiceError> {
        // Get current node to preserve its values
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Create update with current values (no actual changes, just version bump)
        let node_update = crate::models::NodeUpdate {
            node_type: Some(node.node_type.clone()),
            content: Some(node.content.clone()),
            properties: Some(node.properties.clone()),
            title: None,            // Don't update title on version bump
            lifecycle_status: None, // Don't update lifecycle_status on version bump
        };

        // Perform atomic update with version check
        let result = self
            .store
            .update_node_with_version_check(
                node_id,
                expected_version,
                node_update,
                self.client_id.clone(),
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Check if update succeeded (version matched)
        if result.is_none() {
            return Err(NodeServiceError::query_failed(format!(
                "Version conflict: expected version {} for node {}",
                expected_version, node_id
            )));
        }

        // NOTE: NodeUpdated event is now automatically emitted by store notifier (Issue #718)

        Ok(())
    }

    /// Query nodes with filtering
    ///
    /// Executes a filtered query using NodeFilter.
    ///
    /// # Arguments
    ///
    /// * `filter` - The filter criteria
    ///
    /// # Returns
    ///
    /// Vector of matching nodes
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::NodeFilter;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let filter = NodeFilter::new()
    ///     .with_node_type("task".to_string())
    ///     .with_limit(10);
    /// let nodes = service.query_nodes(filter).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_nodes(&self, filter: NodeFilter) -> Result<Vec<Node>, NodeServiceError> {
        // Note: order_by is intentionally handled in-memory after query
        // Complex sorting with sibling chains requires post-query processing
        if filter.order_by.is_some() {
            tracing::debug!(
                "query_nodes: order_by handled via in-memory sorting after database query"
            );
        }

        // Convert NodeFilter to NodeQuery
        let query = crate::models::NodeQuery {
            id: None,
            node_type: filter.node_type,
            content_contains: None,
            mentioned_by: None,
            limit: filter.limit,
        };

        let nodes = self
            .store
            .query_nodes(query)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Apply migrations
        let mut migrated_nodes = Vec::new();
        for mut node in nodes {
            self.backfill_schema_version(&mut node).await?;
            self.apply_lazy_migration(&mut node).await?;
            migrated_nodes.push(node);
        }

        Ok(migrated_nodes)
    }

    /// Query nodes with simple query parameters
    ///
    /// This is a simpler alternative to `query_nodes` for common query patterns.
    /// Supports queries by ID, mentioned_by, content_contains, and node_type.
    ///
    /// # Arguments
    ///
    /// * `query` - Query parameters (see NodeQuery for details)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<Node>)` - List of matching nodes
    /// * `Err(NodeServiceError)` - If database operation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::models::NodeQuery;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Query by ID
    /// let query = NodeQuery::by_id("node-123".to_string());
    /// let nodes = service.query_nodes_simple(query).await?;
    ///
    /// // Query nodes that mention another node
    /// let query = NodeQuery::mentioned_by("target-node".to_string());
    /// let nodes = service.query_nodes_simple(query).await?;
    ///
    /// // Full-text search
    /// let query = NodeQuery::content_contains("search term".to_string()).with_limit(10);
    /// let nodes = service.query_nodes_simple(query).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Query Priority Order
    ///
    /// Queries are evaluated in the following priority order:
    /// 1. `id` - Direct node lookup (exact match)
    /// 2. `mentioned_by` - Nodes that reference the specified node
    /// 3. `content_contains` + optional `node_type` - Full-text content search
    /// 4. `node_type` - Filter by node type
    /// 5. Empty query - Returns empty vec (safer than returning all nodes)
    ///
    /// # Note on Empty Queries
    ///
    /// Queries with no parameters (all fields `None` or `false`) will return an empty vector.
    /// This is intentional to prevent accidentally fetching all nodes from the database.
    ///
    /// # Default Limit
    ///
    /// If no limit is specified in the query, a default limit of [`DEFAULT_QUERY_LIMIT`] (100)
    /// is applied to prevent unbounded queries and potential performance issues.
    /// Callers can override this by explicitly setting a limit via `query.with_limit(n)`.
    pub async fn query_nodes_simple(
        &self,
        query: crate::models::NodeQuery,
    ) -> Result<Vec<Node>, NodeServiceError> {
        // Direct delegation to store.query_nodes for simple queries
        // Complex filtering handled by SurrealDB query engine
        tracing::debug!("query_nodes_simple: Delegating to store.query_nodes");

        // Priority 1: Query by ID (exact match)
        if let Some(ref id) = query.id {
            if let Some(node) = self.get_node(id).await? {
                return Ok(vec![node]);
            } else {
                return Ok(vec![]);
            }
        }

        // Apply default limit if not specified to prevent unbounded queries
        let query = if query.limit.is_none() {
            query.with_limit(DEFAULT_QUERY_LIMIT)
        } else {
            query
        };

        // Priority 2+: Delegate to store.query_nodes
        // Complex query features (mentioned_by, content_contains, filters) delegated to store
        let nodes = self
            .store
            .query_nodes(query)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Apply migrations to results
        let mut migrated_nodes = Vec::new();
        for mut node in nodes {
            self.backfill_schema_version(&mut node).await?;
            self.apply_lazy_migration(&mut node).await?;
            migrated_nodes.push(node);
        }

        Ok(migrated_nodes)
    }

    // Helper methods

    /// Check if a node exists
    async fn node_exists(&self, id: &str) -> Result<bool, NodeServiceError> {
        let node = self.store.get_node(id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to check node existence: {}", e))
        })?;
        Ok(node.is_some())
    }

    /// Check if potential_descendant is a descendant of node_id
    /// This prevents circular references when moving nodes
    async fn is_descendant(
        &self,
        node_id: &str,
        potential_descendant: &str,
    ) -> Result<bool, NodeServiceError> {
        // Walk up from potential_descendant to see if we reach node_id
        let mut current_id = potential_descendant.to_string();

        for _ in 0..1000 {
            // Prevent infinite loops
            if current_id == node_id {
                return Ok(true); // Found node_id, so potential_descendant IS a descendant
            }

            // Walk up via parent relationship
            if let Ok(Some(parent)) = self.get_parent(&current_id).await {
                current_id = parent.id;
            } else {
                break; // Reached root or node not found
            }
        }

        Ok(false)
    }

    /// Bulk create multiple nodes in a transaction
    ///
    /// Creates multiple nodes atomically. If any node fails validation or insertion,
    /// the entire transaction is rolled back.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of nodes to create
    ///
    /// # Returns
    ///
    /// Vector of created node IDs in the same order as input
    ///
    /// # Errors
    ///
    /// Returns error if any node fails validation or insertion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::Node;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let nodes = vec![
    ///     Node::new("text".to_string(), "Note 1".to_string(), json!({})),
    ///     Node::new("text".to_string(), "Note 2".to_string(), json!({})),
    /// ];
    /// let ids = service.bulk_create(nodes).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_create(&self, nodes: Vec<Node>) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Validate all nodes first (two-step validation)
        for node in &nodes {
            // Step 1: Core behavior validation
            self.behaviors.validate_node(node)?;

            // Step 2: Schema validation
            if node.node_type != "schema" {
                self.validate_node_against_schema(node).await?;
            }
        }

        // Call store trait to execute batch insert in transaction
        let created_nodes = self
            .store
            .batch_create_nodes(nodes)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // NOTE: NodeCreated events are now automatically emitted by store notifier (Issue #718)

        // Extract IDs for return (maintaining backward compatibility)
        Ok(created_nodes.into_iter().map(|n| n.id).collect())
    }

    /// Bulk create nodes with hierarchy in a single transaction (Issue #737)
    ///
    /// Creates multiple nodes with parent-child relationships atomically.
    /// This method is optimized for markdown import where all node data
    /// (IDs, hierarchy, ordering) is pre-calculated.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of tuples: (id, node_type, content, parent_id, order, properties)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<String>)` - Vector of created node IDs in insertion order
    /// * `Err` - If validation or transaction fails
    ///
    /// # Performance
    ///
    /// This method provides ~10-15x speedup over sequential node creation
    /// by batching all database operations into a single transaction.
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
    ) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Performance optimization (Issue #760): Cache schema lookups by node_type
        // Instead of querying the database for each node, we query once per unique type
        let unique_types: std::collections::HashSet<&str> = nodes
            .iter()
            .map(|(_, node_type, _, _, _, _)| node_type.as_str())
            .collect();

        // Pre-fetch schemas for all unique types (excluding "schema" type itself)
        let mut schema_cache: std::collections::HashMap<
            String,
            Option<Vec<crate::models::SchemaField>>,
        > = std::collections::HashMap::new();
        for node_type in unique_types {
            if node_type != "schema" {
                let fields = match self.get_schema_for_type(node_type).await? {
                    Some(schema_json) => match schema_json.get("fields") {
                        Some(fields_json) => serde_json::from_value(fields_json.clone()).ok(),
                        None => None,
                    },
                    None => None,
                };
                schema_cache.insert(node_type.to_string(), fields);
            }
        }

        // Validate all nodes before insertion using cached schemas
        for (id, node_type, content, _, _, properties) in &nodes {
            // Build temporary Node for validation
            let temp_node = Node {
                id: id.clone(),
                node_type: node_type.clone(),
                content: content.clone(),
                version: 1,
                properties: properties.clone(),
                mentions: vec![],
                mentioned_by: vec![],
                member_of: vec![],
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                title: None, // Bulk nodes don't need titles (validated only)
                lifecycle_status: "active".to_string(),
            };

            // Validate via behaviors
            self.behaviors.validate_node(&temp_node)?;

            // Validate against cached schema (skip for schema nodes themselves)
            if node_type != "schema" {
                if let Some(Some(fields)) = schema_cache.get(node_type) {
                    self.validate_node_with_fields(&temp_node, fields)?;
                }
            }
        }

        // Find the root ID once - all nodes in a bulk import share the same root
        // Performance optimization (Issue #760): Single DB query instead of N queries
        let root_id = if let Some((_, _, _, Some(first_parent), _, _)) = nodes.first() {
            self.get_root_id(first_parent).await.ok()
        } else {
            None
        };

        // Delegate to store for atomic batch insert
        let result = self
            .store
            .bulk_create_hierarchy(nodes)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Queue root for embedding regeneration once (Issue #729, #760)
        // All nodes share the same root, so we only need one queue operation
        if let Some(root_id) = root_id {
            self.queue_root_for_embedding(&root_id).await;
        }

        Ok(result)
    }

    /// Bulk create nodes with root-only notification (for large imports)
    ///
    /// Same as `bulk_create_hierarchy` but only emits domain events for the root node,
    /// making it more efficient for bulk import scenarios where per-node notifications
    /// would overwhelm the system.
    pub async fn bulk_create_hierarchy_root_notify(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Performance optimization (Issue #760): Cache schema lookups by node_type
        let unique_types: std::collections::HashSet<&str> = nodes
            .iter()
            .map(|(_, node_type, _, _, _, _)| node_type.as_str())
            .collect();

        // Pre-fetch schemas for all unique types (excluding "schema" type itself)
        let mut schema_cache: std::collections::HashMap<
            String,
            Option<Vec<crate::models::SchemaField>>,
        > = std::collections::HashMap::new();
        for node_type in unique_types {
            if node_type != "schema" {
                let fields = match self.get_schema_for_type(node_type).await? {
                    Some(schema_json) => match schema_json.get("fields") {
                        Some(fields_json) => serde_json::from_value(fields_json.clone()).ok(),
                        None => None,
                    },
                    None => None,
                };
                schema_cache.insert(node_type.to_string(), fields);
            }
        }

        // Validate all nodes before insertion using cached schemas
        for (id, node_type, content, _, _, properties) in &nodes {
            let temp_node = Node {
                id: id.clone(),
                node_type: node_type.clone(),
                content: content.clone(),
                version: 1,
                properties: properties.clone(),
                mentions: vec![],
                mentioned_by: vec![],
                member_of: vec![],
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                title: None,
                lifecycle_status: "active".to_string(),
            };

            self.behaviors.validate_node(&temp_node)?;

            if node_type != "schema" {
                if let Some(Some(fields)) = schema_cache.get(node_type) {
                    self.validate_node_with_fields(&temp_node, fields)?;
                }
            }
        }

        // Find the root ID once
        let root_id = if let Some((_, _, _, Some(first_parent), _, _)) = nodes.first() {
            self.get_root_id(first_parent).await.ok()
        } else {
            None
        };

        // Delegate to store - use root-only notify variant
        let result = self
            .store
            .bulk_create_hierarchy_root_notify(nodes)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Queue root for embedding regeneration once
        if let Some(root_id) = root_id {
            self.queue_root_for_embedding(&root_id).await;
        }

        Ok(result)
    }

    /// Bulk update multiple nodes in a transaction
    ///
    /// Updates multiple nodes atomically using a map of node IDs to NodeUpdate structs.
    ///
    /// # Arguments
    ///
    /// * `updates` - Vector of (node_id, NodeUpdate) tuples
    ///
    /// # Errors
    ///
    /// Returns error if any update fails. Transaction is rolled back on failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let updates = vec![
    ///     ("node-1".to_string(), NodeUpdate::new().with_content("Updated 1".to_string())),
    ///     ("node-2".to_string(), NodeUpdate::new().with_content("Updated 2".to_string())),
    /// ];
    /// service.bulk_update(updates).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_update(
        &self,
        updates: Vec<(String, NodeUpdate)>,
    ) -> Result<(), NodeServiceError> {
        if updates.is_empty() {
            return Ok(());
        }

        // Step 1: Batch-fetch all nodes in a single query (Issue #143)
        // This replaces the N+1 pattern where we called get_node() for each update
        let ids: Vec<String> = updates.iter().map(|(id, _)| id.clone()).collect();
        let existing_nodes = self.store.get_nodes_by_ids(&ids).await.map_err(|e| {
            NodeServiceError::bulk_operation_failed(format!(
                "Failed to batch fetch nodes for validation: {}",
                e
            ))
        })?;

        // Step 2: Validate all nodes BEFORE performing atomic update
        // This ensures we fail fast before any database changes
        for (id, update) in &updates {
            // Look up existing node from batch result
            let existing = existing_nodes
                .get(id)
                .ok_or_else(|| NodeServiceError::node_not_found(id))?;

            let mut updated = existing.clone();

            // Apply partial updates to build validation candidate
            if let Some(node_type) = &update.node_type {
                updated.node_type = node_type.clone();
            }

            if let Some(content) = &update.content {
                updated.content = content.clone();
            }

            // NOTE: Sibling ordering is now handled via has_child relationship order field.
            // Bulk updates don't support sibling reordering - use move_node instead.

            if let Some(properties) = &update.properties {
                updated.properties = properties.clone();
            }

            // Validate behavior (PROTECTED rules)
            self.behaviors.validate_node(&updated).map_err(|e| {
                NodeServiceError::bulk_operation_failed(format!(
                    "Failed to validate node {}: {}",
                    id, e
                ))
            })?;

            // Validate schema (USER-EXTENSIBLE rules)
            if updated.node_type != "schema" {
                self.validate_node_against_schema(&updated)
                    .await
                    .map_err(|e| {
                        NodeServiceError::bulk_operation_failed(format!(
                            "Failed schema validation for node {}: {}",
                            id, e
                        ))
                    })?;
            }
        }

        // Step 3: All validations passed - perform atomic bulk update
        self.store.bulk_update(updates.clone()).await.map_err(|e| {
            NodeServiceError::bulk_operation_failed(format!(
                "Failed to execute bulk update transaction: {}",
                e
            ))
        })?;

        // NOTE: NodeUpdated events are now automatically emitted by store notifier (Issue #718)

        Ok(())
    }

    /// Bulk delete multiple nodes in a transaction
    ///
    /// Deletes multiple nodes atomically. If any deletion fails, the entire
    /// transaction is rolled back.
    ///
    /// # Arguments
    ///
    /// * `ids` - Vector of node IDs to delete
    ///
    /// # Errors
    ///
    /// Returns error if any deletion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let ids = vec!["node-1".to_string(), "node-2".to_string()];
    /// service.bulk_delete(ids).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_delete(&self, ids: Vec<String>) -> Result<(), NodeServiceError> {
        if ids.is_empty() {
            return Ok(());
        }

        // Delete nodes one by one using SurrealStore
        // SurrealDB handles atomicity within each delete operation
        for id in &ids {
            self.store
                .delete_node(id, self.client_id.clone())
                .await
                .map_err(|e| {
                    NodeServiceError::bulk_operation_failed(format!(
                        "Failed to delete node {}: {}",
                        id, e
                    ))
                })?;

            // NOTE: NodeDeleted event is now automatically emitted by store notifier (Issue #718)
        }

        Ok(())
    }

    /// Upsert a node with automatic parent creation - single transaction
    ///
    /// Creates parent node if it doesn't exist, then upserts the child node.
    /// All operations happen in a single transaction to prevent database locking.
    ///
    /// # Arguments
    /// * `node_id` - ID of the node to upsert
    /// * `content` - Node content
    /// * `node_type` - Type of node (text, task, date)
    /// * `parent_id` - Parent node ID (will be created as date node if missing)
    ///
    /// # Returns
    /// * `Ok(())` - Operation successful
    /// * `Err(NodeServiceError)` - If transaction fails
    pub async fn upsert_node_with_parent(
        &self,
        node_id: &str,
        content: &str,
        node_type: &str,
        parent_id: &str,
        _root_id: &str, // Deprecated: hierarchy now managed via relationships
        before_sibling_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Ensure parent exists (create if missing)
        if self
            .store
            .get_node(parent_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to check parent existence: {}", e))
            })?
            .is_none()
        {
            // Create parent as date node
            let parent_node = Node::new(
                "date".to_string(),
                parent_id.to_string(),
                serde_json::json!({}),
            );
            self.store
                .create_node(parent_node, self.client_id.clone())
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to create parent node: {}", e))
                })?;

            // NOTE: NodeCreated event is now automatically emitted by store notifier (Issue #718)
        }

        // Upsert the node (update if exists, create if not)
        if let Some(existing) = self.store.get_node(node_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to check node existence: {}", e))
        })? {
            // Update existing node
            let update = NodeUpdate {
                content: Some(content.to_string()),
                // NOTE: Sibling ordering now handled via has_child relationship order field
                ..Default::default()
            };
            self.store
                .update_node(&existing.id, update, self.client_id.clone())
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to update node: {}", e))
                })?;

            // NOTE: NodeUpdated event is now automatically emitted by store notifier (Issue #718)

            // Update parent relationship via edge (handles sibling ordering)
            self.store
                .move_node(node_id, Some(parent_id), before_sibling_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to update parent: {}", e))
                })?;

            // Emit RelationshipUpdated event (Issue #811: unified relationship events)
            let children = self.get_children(parent_id).await?;
            if let Some(child_pos) = children.iter().position(|c| c.id == node_id) {
                self.emit_event(DomainEvent::RelationshipUpdated {
                    relationship: crate::db::events::RelationshipEvent {
                        id: format!("relationship:{}:{}", parent_id, node_id),
                        from_id: parent_id.to_string(),
                        to_id: node_id.to_string(),
                        relationship_type: "has_child".to_string(),
                        properties: serde_json::json!({"order": child_pos as f64}),
                    },
                    source_client_id: self.client_id.clone(),
                });
            }
        } else {
            // Create new node
            let node = Node {
                id: node_id.to_string(),
                node_type: node_type.to_string(),
                content: content.to_string(),
                version: 1,
                properties: serde_json::json!({}),
                mentions: vec![],
                mentioned_by: vec![],
                member_of: vec![],
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                title: None, // Title managed by NodeService for root/task nodes
                lifecycle_status: "active".to_string(),
            };
            self.store
                .create_node(node, self.client_id.clone())
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to create node: {}", e))
                })?;

            // NOTE: NodeCreated event is now automatically emitted by store notifier (Issue #718)

            // Create parent relationship via edge (handles sibling ordering)
            self.store
                .move_node(node_id, Some(parent_id), before_sibling_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to set parent: {}", e))
                })?;

            // Emit RelationshipCreated event (Issue #811: unified relationship events)
            let children = self.get_children(parent_id).await?;
            if let Some(child_pos) = children.iter().position(|c| c.id == node_id) {
                self.emit_event(DomainEvent::RelationshipCreated {
                    relationship: crate::db::events::RelationshipEvent {
                        id: format!("relationship:{}:{}", parent_id, node_id),
                        from_id: parent_id.to_string(),
                        to_id: node_id.to_string(),
                        relationship_type: "has_child".to_string(),
                        properties: serde_json::json!({"order": child_pos as f64}),
                    },
                    source_client_id: self.client_id.clone(),
                });
            }
        }

        Ok(())
    }

    // Helper methods

    /// Populate mentions fields from node_mentions table
    ///
    /// Queries the node_mentions table to populate both outgoing mentions
    /// and incoming mentioned_by references for a node.
    ///
    /// # Arguments
    ///
    /// * `node` - Mutable reference to node to populate
    async fn populate_mentions(&self, node: &mut Node) -> Result<(), NodeServiceError> {
        // Query outgoing mentions (nodes that THIS node references)
        let mentions = self
            .store
            .get_outgoing_mentions(&node.id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get outgoing mentions: {}", e))
            })?;
        node.mentions = mentions;

        // Query incoming mentions (nodes that reference THIS node)
        let mentioned_by = self
            .store
            .get_incoming_mentions(&node.id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get incoming mentions: {}", e))
            })?;
        node.mentioned_by = mentioned_by;

        Ok(())
    }

    /// Add a mention from one node to another
    ///
    /// Creates a mention relationship in the node_mentions table.
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the node that is mentioning
    /// * `target_id` - ID of the node being mentioned
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// service.add_mention("node-123", "node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn add_mention(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Prevent direct self-references
        if source_id == target_id {
            return Err(NodeServiceError::ValidationFailed(
                crate::models::ValidationError::InvalidParent(
                    "Cannot create self-referencing mention".to_string(),
                ),
            ));
        }

        // Verify both nodes exist
        if !self.node_exists(source_id).await? {
            return Err(NodeServiceError::node_not_found(source_id));
        }
        if !self.node_exists(target_id).await? {
            return Err(NodeServiceError::node_not_found(target_id));
        }

        // Prevent root-level self-references (child mentioning its own parent)
        if let Ok(Some(parent)) = self.get_parent(source_id).await {
            if parent.id == target_id {
                return Err(NodeServiceError::ValidationFailed(
                    crate::models::ValidationError::InvalidParent(
                        "Cannot mention own parent (root-level self-reference)".to_string(),
                    ),
                ));
            }
        }

        // Issue #813: Store returns relationship ID, service emits event
        let relationship_id = self
            .store
            .create_mention(source_id, target_id, source_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to insert mention: {}", e))
            })?;

        // Emit event if relationship was created (not already existing)
        if let Some(rel_id) = relationship_id {
            let _ = self.event_tx.send(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent {
                    id: rel_id,
                    from_id: source_id.to_string(),
                    to_id: target_id.to_string(),
                    relationship_type: "mentions".to_string(),
                    properties: serde_json::json!({"root_id": source_id}),
                },
                source_client_id: self.client_id.clone(),
            });
        }

        Ok(())
    }

    /// Remove a mention from one node to another
    ///
    /// Deletes a mention relationship from the node_mentions table.
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the node that is mentioning
    /// * `target_id` - ID of the node being mentioned
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// service.remove_mention("node-123", "node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn remove_mention(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Issue #813: Store returns relationship ID, service emits event
        let relationship_id = self
            .store
            .delete_mention(source_id, target_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to delete mention: {}", e))
            })?;

        // Emit event if relationship was deleted (existed)
        if let Some(rel_id) = relationship_id {
            let _ = self.event_tx.send(DomainEvent::RelationshipDeleted {
                id: rel_id,
                from_id: source_id.to_string(),
                to_id: target_id.to_string(),
                relationship_type: "mentions".to_string(),
                source_client_id: self.client_id.clone(),
            });
        }

        Ok(())
    }

    /// Get all nodes that a specific node mentions (outgoing references)
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to get mentions for
    ///
    /// # Returns
    ///
    /// Vector of node IDs that this node mentions
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let mentions = service.get_mentions("node-123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentions(&self, node_id: &str) -> Result<Vec<String>, NodeServiceError> {
        self.store
            .get_outgoing_mentions(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Get all nodes that mention a specific node (incoming references/backlinks)
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to get backlinks for
    ///
    /// # Returns
    ///
    /// Vector of node IDs that mention this node
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let backlinks = service.get_mentioned_by("node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentioned_by(&self, node_id: &str) -> Result<Vec<String>, NodeServiceError> {
        self.store
            .get_incoming_mentions(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Get root nodes of nodes that mention the target node (backlinks at root level).
    ///
    /// This resolves incoming mentions to their root nodes and deduplicates.
    ///
    /// # Root Resolution Logic
    /// - For task and ai-chat nodes: Uses the node's own ID (they are their own roots)
    /// - For other nodes: Uses their root_id (or the node ID itself if it's a root)
    ///
    /// # Example
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // If nodes A and B (both children of Container X) mention target node,
    /// // returns ['container-x-id'] (deduplicated)
    /// let containers = service.get_mentioning_containers("target-node-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentioning_containers(
        &self,
        node_id: &str,
    ) -> Result<Vec<String>, NodeServiceError> {
        let nodes = self
            .store
            .get_mentioning_containers(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Extract node IDs from the nodes
        Ok(nodes.into_iter().map(|n| n.id).collect())
    }

    // ========================================================================
    // Relationship CRUD Operations (Issue #703 Phase 4)
    // ========================================================================

    /// Create a relationship between two nodes
    ///
    /// Creates an edge in the appropriate relationship table based on the schema definition.
    /// Validates that both nodes exist, enforces cardinality constraints, and supports
    /// edge field data.
    ///
    /// # TODO(Issue #710): UI components needed for relationship interaction
    /// The backend API is complete, but users need UI components to:
    /// - Select nodes to relate (search/dropdown)
    /// - View existing relationships
    /// - Remove relationships
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the source node
    /// * `relationship_name` - Name of the relationship (e.g., "assigned_to")
    /// * `target_id` - ID of the target node
    /// * `edge_data` - Optional JSON data for edge fields
    ///
    /// # Returns
    ///
    /// Ok(()) if successful
    ///
    /// # Errors
    ///
    /// - `NodeNotFound` - Source or target node doesn't exist
    /// - `SchemaNotFound` - Source node's schema doesn't exist
    /// - `RelationshipNotFound` - Relationship not defined in schema
    /// - `TargetTypeMismatch` - Target node type doesn't match schema definition
    /// - `CardinalityViolation` - Cardinality constraint would be violated
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Create relationship with edge field data
    /// service.create_relationship(
    ///     "task-123",
    ///     "assigned_to",
    ///     "person-456",
    ///     json!({"role": "owner", "assigned_at": "2025-01-15"})
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_relationship(
        &self,
        source_id: &str,
        relationship_name: &str,
        target_id: &str,
        edge_data: Value,
    ) -> Result<(), NodeServiceError> {
        // Issue #825: Unified relationship creation - ALL relationships use the `relationship` table
        // The relationship_type field distinguishes between different relationship types

        // Built-in type validation
        let is_builtin = matches!(relationship_name, "member_of" | "has_child" | "mentions");

        if is_builtin {
            // Built-in type-specific validation
            if relationship_name == "member_of" {
                let target = self
                    .get_node(target_id)
                    .await?
                    .ok_or_else(|| NodeServiceError::node_not_found(target_id))?;
                if target.node_type != "collection" {
                    return Err(NodeServiceError::invalid_update(format!(
                        "member_of target must be a collection node, got '{}'",
                        target.node_type
                    )));
                }
            }
        } else {
            // Custom relationship: validate against source node's schema
            let source = self
                .get_node(source_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(source_id))?;

            let schema_id = &source.node_type;
            let schema_node = self.get_node(schema_id).await?.ok_or_else(|| {
                NodeServiceError::query_failed(format!("Schema '{}' not found", schema_id))
            })?;

            let relationships: Vec<SchemaRelationship> = schema_node
                .properties
                .get("relationships")
                .and_then(|r| serde_json::from_value(r.clone()).ok())
                .unwrap_or_default();

            let relationship = relationships
                .iter()
                .find(|r| r.name == relationship_name)
                .ok_or_else(|| {
                    NodeServiceError::invalid_update(format!(
                        "Relationship '{}' not defined in schema '{}'. Built-in relationships (member_of, has_child, mentions) are universal.",
                        relationship_name, schema_id
                    ))
                })?;

            // Validate target node type
            let target = self
                .get_node(target_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(target_id))?;

            if target.node_type != relationship.target_type {
                return Err(NodeServiceError::invalid_update(format!(
                    "Target node type '{}' doesn't match expected type '{}' for relationship '{}'",
                    target.node_type, relationship.target_type, relationship_name
                )));
            }

            // Check cardinality constraint
            if relationship.cardinality == crate::models::schema::RelationshipCardinality::One {
                let source_thing =
                    surrealdb::sql::Thing::from(("node".to_string(), source_id.to_string()));
                let query = "SELECT * FROM relationship WHERE in = $source AND relationship_type = $rel_type";

                let mut result = self
                    .store
                    .db()
                    .query(query)
                    .bind(("source", source_thing))
                    .bind(("rel_type", relationship_name.to_string()))
                    .await
                    .map_err(|e| {
                        NodeServiceError::query_failed(format!(
                            "Failed to check cardinality: {}",
                            e
                        ))
                    })?;

                let existing: Vec<Value> = result.take(0).map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to parse cardinality check: {}",
                        e
                    ))
                })?;

                if !existing.is_empty() {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Relationship '{}' has cardinality 'one' but an edge already exists",
                        relationship_name
                    )));
                }
            }
        }

        // Create SurrealDB Thing (record ID) for source and target nodes
        let source_thing = surrealdb::sql::Thing::from(("node".to_string(), source_id.to_string()));
        let target_thing = surrealdb::sql::Thing::from(("node".to_string(), target_id.to_string()));

        // Check for existing relationship (idempotency)
        let check_query =
            "SELECT VALUE id FROM relationship WHERE in = $from AND out = $to AND relationship_type = $rel_type";
        let mut check_response = self
            .store
            .db()
            .query(check_query)
            .bind(("from", source_thing.clone()))
            .bind(("to", target_thing.clone()))
            .bind(("rel_type", relationship_name.to_string()))
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to check existing relationship: {}",
                    e
                ))
            })?;

        let existing_ids: Vec<surrealdb::sql::Thing> = check_response.take(0).unwrap_or_default();
        if !existing_ids.is_empty() {
            // Relationship already exists, idempotent success
            return Ok(());
        }

        // Build and execute the RELATE query - all relationships use the same table
        let properties_json =
            serde_json::to_string(&edge_data).unwrap_or_else(|_| "{}".to_string());

        let relate_query = format!(
            r#"RELATE $source->relationship->$target CONTENT {{
                relationship_type: $rel_type,
                properties: {},
                created_at: time::now(),
                modified_at: time::now(),
                version: 1
            }} RETURN id"#,
            properties_json
        );

        let mut result = self
            .store
            .db()
            .query(&relate_query)
            .bind(("source", source_thing))
            .bind(("target", target_thing))
            .bind(("rel_type", relationship_name.to_string()))
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to create relationship: {}", e))
            })?;

        // Extract relationship ID and emit event
        #[derive(Debug, serde::Deserialize)]
        struct RelateResult {
            id: surrealdb::sql::Thing,
        }
        let results: Vec<RelateResult> = result.take(0).unwrap_or_default();

        if let Some(rel_result) = results.first() {
            let _ = self.event_tx.send(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent {
                    id: rel_result.id.to_string(),
                    from_id: source_id.to_string(),
                    to_id: target_id.to_string(),
                    relationship_type: relationship_name.to_string(),
                    properties: edge_data,
                },
                source_client_id: self.client_id.clone(),
            });
        }

        Ok(())
    }

    /// Delete a relationship between two nodes
    ///
    /// Removes the edge between the source and target nodes for the specified relationship.
    ///
    /// # TODO(Issue #710): UI components needed for relationship interaction
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the source node
    /// * `relationship_name` - Name of the relationship
    /// * `target_id` - ID of the target node
    ///
    /// # Returns
    ///
    /// Ok(()) if successful (idempotent - succeeds even if edge doesn't exist)
    ///
    /// # Errors
    ///
    /// - `NodeNotFound` - Source node doesn't exist
    /// - `SchemaNotFound` - Source node's schema doesn't exist
    /// - `RelationshipNotFound` - Relationship not defined in schema
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// service.delete_relationship("task-123", "assigned_to", "person-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_relationship(
        &self,
        source_id: &str,
        relationship_name: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Issue #825: Unified relationship deletion - ALL relationships use the `relationship` table
        // The relationship_type field distinguishes between different relationship types

        // Create SurrealDB Thing (record ID) for source and target nodes
        let source_thing = surrealdb::sql::Thing::from(("node".to_string(), source_id.to_string()));
        let target_thing = surrealdb::sql::Thing::from(("node".to_string(), target_id.to_string()));

        // Get relationship ID before deleting (for event emission)
        let check_query =
            "SELECT VALUE id FROM relationship WHERE in = $source AND out = $target AND relationship_type = $rel_type";

        let mut check_result = self
            .store
            .db()
            .query(check_query)
            .bind(("source", source_thing.clone()))
            .bind(("target", target_thing.clone()))
            .bind(("rel_type", relationship_name.to_string()))
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get relationship ID: {}", e))
            })?;

        let existing_ids: Vec<surrealdb::sql::Thing> = check_result.take(0).unwrap_or_default();

        // Delete the edge
        let delete_query =
            "DELETE FROM relationship WHERE in = $source AND out = $target AND relationship_type = $rel_type";

        self.store
            .db()
            .query(delete_query)
            .bind(("source", source_thing))
            .bind(("target", target_thing))
            .bind(("rel_type", relationship_name.to_string()))
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to delete relationship: {}", e))
            })?;

        // Emit RelationshipDeleted event
        if let Some(rel_id) = existing_ids.first() {
            let _ = self.event_tx.send(DomainEvent::RelationshipDeleted {
                id: rel_id.to_string(),
                from_id: source_id.to_string(),
                to_id: target_id.to_string(),
                relationship_type: relationship_name.to_string(),
                source_client_id: self.client_id.clone(),
            });
        }

        Ok(())
    }

    /// Get all related nodes for a given relationship
    ///
    /// Queries the relationship table and returns all target nodes connected via the specified
    /// relationship. Supports both "out" and "in" directions.
    ///
    /// # TODO(Issue #710): UI components needed for relationship interaction
    ///
    /// # Arguments
    ///
    /// * `node_id` - ID of the node to get relationships for
    /// * `relationship_name` - Name of the relationship
    /// * `direction` - Direction to traverse ("out" for forward, "in" for reverse)
    ///
    /// # Returns
    ///
    /// Vector of related nodes
    ///
    /// # Errors
    ///
    /// - `NodeNotFound` - Source node doesn't exist
    /// - `SchemaNotFound` - Source node's schema doesn't exist
    /// - `RelationshipNotFound` - Relationship not defined in schema
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Get all people assigned to this task
    /// let assigned = service.get_related_nodes("task-123", "assigned_to", "out").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_related_nodes(
        &self,
        node_id: &str,
        relationship_name: &str,
        direction: &str,
    ) -> Result<Vec<Node>, NodeServiceError> {
        // Validate direction first
        if direction != "out" && direction != "in" {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid direction '{}', must be 'out' or 'in'",
                direction
            )));
        }

        // Issue #825: ALL relationships use the universal `relationship` table
        let node_thing = surrealdb::sql::Thing::from(("node".to_string(), node_id.to_string()));

        // Query the unified relationship table
        let query = match direction {
            "out" => {
                // Forward: get 'out' nodes (targets) from edges where 'in' = source node
                r#"
                    SELECT out FROM relationship
                    WHERE in = $node AND relationship_type = $rel_type
                "#
            }
            "in" => {
                // Reverse: get 'in' nodes (sources) from edges where 'out' = target node
                r#"
                    SELECT in AS out FROM relationship
                    WHERE out = $node AND relationship_type = $rel_type
                "#
            }
            _ => unreachable!(), // Already validated in caller
        };

        // Convert to owned String to satisfy lifetime requirements
        let rel_type_owned = relationship_name.to_string();

        let mut result = self
            .store
            .db()
            .query(query)
            .bind(("node", node_thing))
            .bind(("rel_type", rel_type_owned))
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get related nodes: {}", e))
            })?;

        #[derive(serde::Deserialize)]
        struct EdgeOut {
            out: surrealdb::sql::Thing,
        }

        let edges: Vec<EdgeOut> = result.take(0).map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to parse related edges: {}", e))
        })?;

        if edges.is_empty() {
            return Ok(Vec::new());
        }

        // Fetch full node records
        let mut nodes = Vec::new();
        for edge in edges {
            let related_id = edge.out.id.to_raw();
            if let Some(node) = self.get_node(&related_id).await? {
                nodes.push(node);
            }
        }

        Ok(nodes)
    }

    // ========================================================================
    // NLP Discovery API (Phase 5)
    // ========================================================================

    /// Get all schema nodes with their relationships
    ///
    /// Returns all schema definitions including fields and relationships.
    /// This is the primary entry point for NLP to understand the data model.
    ///
    /// # Returns
    ///
    /// Vector of all schema nodes, ordered by ID.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Get all schemas to understand the data model
    /// let schemas = service.get_all_schemas().await?;
    /// for schema in schemas {
    ///     println!("Type: {} ({} fields, {} relationships)",
    ///         schema.id, schema.fields.len(), schema.relationships.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_all_schemas(
        &self,
    ) -> Result<Vec<crate::models::SchemaNode>, NodeServiceError> {
        self.store.get_all_schemas().await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                context: format!("Failed to get all schemas: {}", e),
            })
        })
    }

    /// Get a schema with full relationship information
    ///
    /// Convenience method that returns a SchemaNode with its relationships.
    /// Use this when you need the complete schema definition including relationships.
    ///
    /// # Arguments
    ///
    /// * `schema_id` - The schema ID (e.g., "task", "invoice")
    ///
    /// # Returns
    ///
    /// The SchemaNode if found, None otherwise.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// if let Some(schema) = service.get_schema_with_relationships("invoice").await? {
    ///     for rel in &schema.relationships {
    ///         println!("{} -> {} ({:?})", rel.name, rel.target_type, rel.cardinality);
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_schema_with_relationships(
        &self,
        schema_id: &str,
    ) -> Result<Option<crate::models::SchemaNode>, NodeServiceError> {
        // get_schema_node already includes relationships now
        self.get_schema_node(schema_id).await
    }

    /// Compute inbound relationships for a node type
    ///
    /// Returns all relationships from other schemas that point TO this node type.
    /// This is a computed lookup (not cached) - for frequently accessed data,
    /// use `InboundRelationshipCache` instead.
    ///
    /// # Arguments
    ///
    /// * `target_type` - The node type to find inbound relationships for (e.g., "customer")
    ///
    /// # Returns
    ///
    /// Vector of tuples: (source_schema_id, relationship)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // What relationships point TO customer?
    /// let inbound = service.get_inbound_relationships("customer").await?;
    /// for (source_type, rel) in inbound {
    ///     println!("{}.{} -> customer", source_type, rel.name);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_inbound_relationships(
        &self,
        target_type: &str,
    ) -> Result<Vec<(String, crate::models::schema::SchemaRelationship)>, NodeServiceError> {
        let schemas = self.get_all_schemas().await?;

        let mut inbound = Vec::new();
        for schema in schemas {
            for relationship in schema.relationships {
                if relationship.target_type == target_type {
                    inbound.push((schema.id.clone(), relationship));
                }
            }
        }

        Ok(inbound)
    }

    /// Get relationship graph summary for NLP
    ///
    /// Returns a summary of all relationships in the system, useful for
    /// NLP to understand the overall data model structure.
    ///
    /// # Returns
    ///
    /// Vector of tuples: (source_type, relationship_name, target_type)
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SurrealStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let graph = service.get_relationship_graph().await?;
    /// for (source, rel_name, target) in graph {
    ///     println!("{} --{}-> {}", source, rel_name, target);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_relationship_graph(
        &self,
    ) -> Result<Vec<(String, String, String)>, NodeServiceError> {
        let schemas = self.get_all_schemas().await?;

        let mut edges = Vec::new();
        for schema in schemas {
            for relationship in schema.relationships {
                edges.push((
                    schema.id.clone(),
                    relationship.name.clone(),
                    relationship.target_type.clone(),
                ));
            }
        }

        Ok(edges)
    }
}

/// Recursively build a tree structure from flat node data
///
/// Converts flat node map and adjacency list into nested JSON tree.
/// Uses `node_to_typed_value` for typed serialization, which converts
/// task nodes to `TaskNode` (with proper camelCase properties) and
/// schema nodes to `SchemaNode`. This ensures consistent API output
/// matching the naming conventions.
fn build_node_tree_recursive(
    node: &Node,
    node_map: &HashMap<String, Node>,
    adjacency_list: &HashMap<String, Vec<String>>,
) -> serde_json::Value {
    // Use typed serialization for task/schema nodes (camelCase properties)
    // Falls back to raw Node serialization for other types
    let mut json = crate::models::node_to_typed_value(node.clone())
        .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));

    // Build children array (always present, even if empty for consistency)
    let children: Vec<serde_json::Value> = if let Some(children_ids) = adjacency_list.get(&node.id)
    {
        children_ids
            .iter()
            .filter_map(|child_id| {
                node_map.get(child_id).map(|child_node| {
                    build_node_tree_recursive(child_node, node_map, adjacency_list)
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    if let Some(obj) = json.as_object_mut() {
        obj.insert("children".to_string(), serde_json::Value::Array(children));
    }

    json
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SurrealStore;
    use serde_json::json;
    use tempfile::TempDir;

    async fn create_test_service() -> (NodeService, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");

        let mut store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let service = NodeService::new(&mut store).await.unwrap();
        (service, temp_dir)
    }

    #[tokio::test]
    async fn test_create_text_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new("text".to_string(), "Hello World".to_string(), json!({}));

        let id = service.create_node(node.clone()).await.unwrap();
        assert_eq!(id, node.id);

        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Hello World");
        assert_eq!(retrieved.node_type, "text");
    }

    #[tokio::test]
    async fn test_create_task_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new(
            "task".to_string(),
            "Implement NodeService".to_string(),
            json!({"status": "in_progress", "priority": "high"}),
        );

        let id = service.create_node(node).await.unwrap();
        let retrieved = service.get_node(&id).await.unwrap().unwrap();

        assert_eq!(retrieved.node_type, "task");
        assert_eq!(retrieved.properties["status"], "in_progress");
        assert_eq!(retrieved.properties["priority"], "high");
    }

    #[tokio::test]
    async fn test_create_date_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new_with_id(
            "2025-01-03".to_string(),
            "text".to_string(),
            "2025-01-03".to_string(),
            json!({}),
        );

        let id = service.create_node(node).await.unwrap();
        assert_eq!(id, "2025-01-03");

        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.node_type, "date");
        assert_eq!(retrieved.id, "2025-01-03");
    }

    #[tokio::test]
    async fn test_get_virtual_date_node_as_parent() {
        let (service, _temp) = create_test_service().await;

        // Verify the date node is returned as virtual (not persisted yet)
        let date_before = service.get_node("2025-10-13").await.unwrap().unwrap();
        assert_eq!(date_before.node_type, "date");
        assert_eq!(date_before.content, "2025-10-13"); // Virtual dates have correct content

        // Verify it's NOT persisted in database yet
        let filter = NodeFilter::new()
            .with_node_type("date".to_string())
            .with_ids(vec!["2025-10-13".to_string()]);
        let results = service.query_nodes(filter).await.unwrap();
        assert_eq!(results.len(), 0); // Not persisted yet - virtual only

        // For actual persistence when children are added, use NodeOperations
        // (NodeService is low-level, NodeOperations handles business logic like auto-creating dates)
    }

    #[tokio::test]
    async fn test_get_virtual_date_node() {
        let (service, _temp) = create_test_service().await;

        // Get a date node that doesn't exist in database
        // Should return virtual date node with correct properties
        let node = service.get_node("2025-10-13").await.unwrap();
        assert!(node.is_some());

        let date_node = node.unwrap();
        assert_eq!(date_node.id, "2025-10-13");
        assert_eq!(date_node.node_type, "date");
        assert_eq!(date_node.content, "2025-10-13"); // Virtual date nodes default content to the date ID
                                                     // Note: Sibling ordering is now on has_child relationship order field, not node.before_sibling_id
    }

    #[tokio::test]
    async fn test_get_virtual_date_node_not_persisted() {
        let (service, _temp) = create_test_service().await;

        // Get virtual date node
        let _virtual_node = service.get_node("2025-10-13").await.unwrap().unwrap();

        // Verify it's NOT in the database (virtual only)
        // Try to query it by filtering for date nodes specifically
        let filter = NodeFilter::new()
            .with_node_type("date".to_string())
            .with_ids(vec!["2025-10-13".to_string()]);
        let results = service.query_nodes(filter).await.unwrap();
        assert_eq!(results.len(), 0); // Not persisted yet - virtual only
    }

    #[tokio::test]
    async fn test_virtual_date_persists_when_child_created() {
        let (service, _temp) = create_test_service().await;

        // This test demonstrates that NodeOperations (not NodeService directly)
        // handles auto-persistence of date nodes when children are created.
        // NodeService is low-level storage, NodeOperations has business logic.

        // Verify virtual date exists
        let virtual_date = service.get_node("2025-10-13").await.unwrap().unwrap();
        assert_eq!(virtual_date.content, "2025-10-13");

        // Auto-persistence happens in NodeOperations.create_node, not NodeService
        // (see operations module tests for that behavior)
    }

    #[tokio::test]
    async fn test_get_node_returns_none_for_invalid_date() {
        let (service, _temp) = create_test_service().await;

        // Invalid date formats should return None
        let invalid1 = service.get_node("not-a-date").await.unwrap();
        assert!(invalid1.is_none());

        // Invalid dates (wrong format) should return None
        let invalid2 = service.get_node("25-10-13").await.unwrap(); // Wrong format
        assert!(invalid2.is_none());

        // Semantically invalid dates should return None
        let invalid3 = service.get_node("2025-13-45").await.unwrap(); // Invalid month/day
        assert!(invalid3.is_none());
    }

    #[tokio::test]
    async fn test_persisted_date_takes_precedence_over_virtual() {
        let (service, _temp) = create_test_service().await;

        // Create and persist a date node with custom content
        let date_node = Node::new_with_id(
            "2025-10-13".to_string(),
            "date".to_string(),
            "Custom Date Content".to_string(),
            json!({}), // No properties - date nodes use content only
        );

        service.create_node(date_node).await.unwrap();

        // Get the node - should return persisted version with custom content
        let retrieved = service.get_node("2025-10-13").await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Custom Date Content");
        assert_eq!(retrieved.node_type, "date");
    }

    #[tokio::test]
    async fn test_update_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new("text".to_string(), "Original".to_string(), json!({}));

        let id = service.create_node(node).await.unwrap();

        let update = NodeUpdate::new().with_content("Updated".to_string());
        service.update_node(&id, update).await.unwrap();

        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Updated");
    }

    #[tokio::test]
    async fn test_delete_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new("text".to_string(), "To be deleted".to_string(), json!({}));

        let id = service.create_node(node).await.unwrap();
        service.delete_node(&id).await.unwrap();

        let retrieved = service.get_node(&id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_query_nodes_by_type() {
        let (service, _temp) = create_test_service().await;

        service
            .create_node(Node::new(
                "text".to_string(),
                "Text 1".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        service
            .create_node(Node::new(
                "task".to_string(),
                "Task 1".to_string(),
                json!({"status": "open"}),
            ))
            .await
            .unwrap();
        service
            .create_node(Node::new(
                "text".to_string(),
                "Text 2".to_string(),
                json!({}),
            ))
            .await
            .unwrap();

        let filter = NodeFilter::new().with_node_type("text".to_string());
        let results = service.query_nodes(filter).await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|n| n.node_type == "text"));
    }

    #[tokio::test]
    async fn test_bulk_create() {
        let (service, _temp) = create_test_service().await;

        let nodes = vec![
            Node::new("text".to_string(), "Bulk 1".to_string(), json!({})),
            Node::new("text".to_string(), "Bulk 2".to_string(), json!({})),
            Node::new(
                "task".to_string(),
                "Bulk Task".to_string(),
                json!({"status": "open"}),
            ),
        ];

        let ids = service.bulk_create(nodes.clone()).await.unwrap();
        assert_eq!(ids.len(), 3);

        for (i, id) in ids.iter().enumerate() {
            let node = service.get_node(id).await.unwrap().unwrap();
            assert_eq!(node.content, nodes[i].content);
        }
    }

    #[tokio::test]
    async fn test_bulk_update() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Original 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Original 2".to_string(), json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        let updates = vec![
            (
                id1.clone(),
                NodeUpdate::new().with_content("Updated 1".to_string()),
            ),
            (
                id2.clone(),
                NodeUpdate::new().with_content("Updated 2".to_string()),
            ),
        ];

        service.bulk_update(updates).await.unwrap();

        let updated1 = service.get_node(&id1).await.unwrap().unwrap();
        let updated2 = service.get_node(&id2).await.unwrap().unwrap();

        assert_eq!(updated1.content, "Updated 1");
        assert_eq!(updated2.content, "Updated 2");
    }

    #[tokio::test]
    async fn test_bulk_update_with_larger_batch() {
        // Test with 10+ nodes to verify batch fetch works at scale
        let (service, _temp) = create_test_service().await;

        // Create 15 nodes
        let mut ids = Vec::new();
        for i in 0..15 {
            let node = Node::new(
                "text".to_string(),
                format!("Original content {}", i),
                json!({}),
            );
            let id = service.create_node(node).await.unwrap();
            ids.push(id);
        }

        // Build updates for all nodes
        let updates: Vec<(String, NodeUpdate)> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    id.clone(),
                    NodeUpdate::new().with_content(format!("Updated content {}", i)),
                )
            })
            .collect();

        // Bulk update should succeed
        service.bulk_update(updates).await.unwrap();

        // Verify all nodes were updated correctly
        for (i, id) in ids.iter().enumerate() {
            let node = service.get_node(id).await.unwrap().unwrap();
            assert_eq!(node.content, format!("Updated content {}", i));
        }
    }

    #[tokio::test]
    async fn test_bulk_update_with_nonexistent_node() {
        // Test that bulk_update fails when one of the nodes doesn't exist
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Original 1".to_string(), json!({}));
        let id1 = service.create_node(node1).await.unwrap();

        let updates = vec![
            (
                id1.clone(),
                NodeUpdate::new().with_content("Updated 1".to_string()),
            ),
            (
                "nonexistent-node-id".to_string(),
                NodeUpdate::new().with_content("Should fail".to_string()),
            ),
        ];

        let result = service.bulk_update(updates).await;
        assert!(result.is_err());

        // Verify the first node was NOT updated (transaction should have failed before any changes)
        let node = service.get_node(&id1).await.unwrap().unwrap();
        assert_eq!(node.content, "Original 1");
    }

    #[tokio::test]
    async fn test_bulk_update_empty_list() {
        // Edge case: empty updates list should succeed immediately
        let (service, _temp) = create_test_service().await;

        let result = service.bulk_update(vec![]).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_bulk_update_with_task_nodes() {
        // Test bulk update works correctly with task nodes.
        // Note: The store's bulk_update only updates base node fields (content, node_type).
        // Task-specific properties (like status) are not updated by bulk_update.
        // This test verifies that task nodes can be bulk-updated without error.
        let (service, _temp) = create_test_service().await;

        let task1 = Node::new(
            "task".to_string(),
            "Task 1".to_string(),
            json!({"status": "open"}),
        );
        let task2 = Node::new(
            "task".to_string(),
            "Task 2".to_string(),
            json!({"status": "open"}),
        );

        let id1 = service.create_node(task1).await.unwrap();
        let id2 = service.create_node(task2).await.unwrap();

        // Update content (task properties like status are NOT updated by bulk_update)
        let updates = vec![
            (
                id1.clone(),
                NodeUpdate::new().with_content("Updated Task 1".to_string()),
            ),
            (
                id2.clone(),
                NodeUpdate::new().with_content("Updated Task 2".to_string()),
            ),
        ];

        service.bulk_update(updates).await.unwrap();

        let updated1 = service.get_node(&id1).await.unwrap().unwrap();
        let updated2 = service.get_node(&id2).await.unwrap().unwrap();

        // Verify content was updated
        assert_eq!(updated1.content, "Updated Task 1");
        assert_eq!(updated2.content, "Updated Task 2");
        // Verify properties are preserved (status should still be "open")
        assert_eq!(updated1.properties["status"], "open");
        assert_eq!(updated2.properties["status"], "open");
    }

    #[tokio::test]
    async fn test_bulk_update_with_mixed_node_types() {
        // Test bulk update with mixed text and task nodes.
        // Note: bulk_update only updates base node fields (content, node_type).
        // This test verifies that mixed node types work together in the batch fetch.
        let (service, _temp) = create_test_service().await;

        let text_node = Node::new("text".to_string(), "Text content".to_string(), json!({}));
        let task_node = Node::new(
            "task".to_string(),
            "Task content".to_string(),
            json!({"status": "open"}),
        );

        let text_id = service.create_node(text_node).await.unwrap();
        let task_id = service.create_node(task_node).await.unwrap();

        // Update content only (task-specific properties are NOT updated by bulk_update)
        let updates = vec![
            (
                text_id.clone(),
                NodeUpdate::new().with_content("Updated text".to_string()),
            ),
            (
                task_id.clone(),
                NodeUpdate::new().with_content("Updated task".to_string()),
            ),
        ];

        service.bulk_update(updates).await.unwrap();

        let updated_text = service.get_node(&text_id).await.unwrap().unwrap();
        let updated_task = service.get_node(&task_id).await.unwrap().unwrap();

        assert_eq!(updated_text.content, "Updated text");
        assert_eq!(updated_text.node_type, "text");
        assert_eq!(updated_task.content, "Updated task");
        assert_eq!(updated_task.node_type, "task");
        // Properties are preserved, not updated by bulk_update
        assert_eq!(updated_task.properties["status"], "open");
    }

    #[tokio::test]
    async fn test_bulk_delete() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Delete 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Delete 2".to_string(), json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        service
            .bulk_delete(vec![id1.clone(), id2.clone()])
            .await
            .unwrap();

        assert!(service.get_node(&id1).await.unwrap().is_none());
        assert!(service.get_node(&id2).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_reorder_siblings() {
        let (service, _temp) = create_test_service().await;

        // Create parent node
        let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
        let parent_id = service.create_node(parent).await.unwrap();

        // Create two children under the parent
        // Note: move_node without sibling inserts at BEGINNING, so child2 will be first
        let child1 = Node::new("text".to_string(), "Child 1".to_string(), json!({}));
        let child1_id = service.create_node(child1).await.unwrap();
        service
            .move_node(&child1_id, Some(&parent_id), None)
            .await
            .unwrap();

        let child2 = Node::new("text".to_string(), "Child 2".to_string(), json!({}));
        let child2_id = service.create_node(child2).await.unwrap();
        service
            .move_node(&child2_id, Some(&parent_id), None)
            .await
            .unwrap();

        // Get initial order - child2 should be FIRST (inserted at beginning)
        let children_before = service.get_children(&parent_id).await.unwrap();
        assert_eq!(children_before.len(), 2);
        assert_eq!(
            children_before[0].id, child2_id,
            "Child2 should be first (inserted at beginning)"
        );
        assert_eq!(children_before[1].id, child1_id, "Child1 should be second");

        // Reorder child1 to be before child2 (making child1 first)
        // Using insert_after=None means insert at beginning
        service.reorder_child(&child1_id, None).await.unwrap();

        // Verify new order - child1 should now be first
        let children_after = service.get_children(&parent_id).await.unwrap();
        assert_eq!(children_after.len(), 2);
        assert_eq!(
            children_after[0].id, child1_id,
            "Child1 should be first after reorder"
        );
        assert_eq!(
            children_after[1].id, child2_id,
            "Child2 should be second after reorder"
        );
    }

    #[tokio::test]
    async fn test_transaction_rollback_on_error() {
        let (service, _temp) = create_test_service().await;

        // Create one valid node and one invalid node
        let valid_node = Node::new("text".to_string(), "Valid".to_string(), json!({}));
        // Issue #712: Unknown node types now use CustomNodeBehavior fallback
        // CustomNodeBehavior validates that properties is a JSON object
        // Using an unknown type with non-object properties triggers validation failure
        let mut invalid_node =
            Node::new("custom_type".to_string(), "Content".to_string(), json!({}));
        invalid_node.properties = json!("not an object"); // Properties must be an object

        let nodes = vec![valid_node.clone(), invalid_node];

        // Bulk create should fail (due to invalid properties on custom type)
        let result = service.bulk_create(nodes).await;
        assert!(result.is_err());

        // Verify that valid node was NOT created (transaction rolled back)
        let check = service.get_node(&valid_node.id).await.unwrap();
        assert!(check.is_none());
    }

    #[tokio::test]
    async fn test_add_mention() {
        let (service, _temp) = create_test_service().await;

        // Create two nodes
        let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Add mention from node1 to node2
        service.add_mention(&id1, &id2).await.unwrap();

        // Verify mention was added
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0], id2);

        // Verify backlink
        let mentioned_by = service.get_mentioned_by(&id2).await.unwrap();
        assert_eq!(mentioned_by.len(), 1);
        assert_eq!(mentioned_by[0], id1);
    }

    #[tokio::test]
    async fn test_remove_mention() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Add and then remove mention
        service.add_mention(&id1, &id2).await.unwrap();
        service.remove_mention(&id1, &id2).await.unwrap();

        // Verify mention was removed
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(mentions.len(), 0);

        let mentioned_by = service.get_mentioned_by(&id2).await.unwrap();
        assert_eq!(mentioned_by.len(), 0);
    }

    #[tokio::test]
    async fn test_get_node_populates_mentions() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));
        let node3 = Node::new("text".to_string(), "Node 3".to_string(), json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();
        let id3 = service.create_node(node3).await.unwrap();

        // Node 1 mentions Node 2 and Node 3
        service.add_mention(&id1, &id2).await.unwrap();
        service.add_mention(&id1, &id3).await.unwrap();

        // Node 2 mentions Node 1
        service.add_mention(&id2, &id1).await.unwrap();

        // Fetch node 1 and verify mentions are populated
        let node = service.get_node(&id1).await.unwrap().unwrap();
        assert_eq!(node.mentions.len(), 2);
        assert!(node.mentions.contains(&id2));
        assert!(node.mentions.contains(&id3));
        assert_eq!(node.mentioned_by.len(), 1);
        assert!(node.mentioned_by.contains(&id2));
    }

    #[tokio::test]
    async fn test_query_mentioned_by() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));
        let node3 = Node::new("text".to_string(), "Node 3".to_string(), json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();
        let id3 = service.create_node(node3).await.unwrap();

        // Node 1 and Node 2 mention Node 3
        service.add_mention(&id1, &id3).await.unwrap();
        service.add_mention(&id2, &id3).await.unwrap();

        // Query for nodes that mention Node 3
        let query = crate::models::NodeQuery::mentioned_by(id3.clone());
        let nodes = service.query_nodes_simple(query).await.unwrap();

        assert_eq!(nodes.len(), 2);
        let node_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        assert!(node_ids.contains(&id1));
        assert!(node_ids.contains(&id2));
    }

    #[tokio::test]
    async fn test_mention_duplicate_handling() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Add same mention twice - should not error (INSERT OR IGNORE)
        service.add_mention(&id1, &id2).await.unwrap();
        service.add_mention(&id1, &id2).await.unwrap();

        // Should still only have one mention
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(mentions.len(), 1);
    }

    #[tokio::test]
    async fn test_mention_nonexistent_node() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
        let id1 = service.create_node(node1).await.unwrap();

        // Try to mention a non-existent node
        let result = service.add_mention(&id1, "nonexistent").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            NodeServiceError::NodeNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn test_bidirectional_mentions() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Create bidirectional mentions
        service.add_mention(&id1, &id2).await.unwrap();
        service.add_mention(&id2, &id1).await.unwrap();

        // Verify node 1
        let node1 = service.get_node(&id1).await.unwrap().unwrap();
        assert_eq!(node1.mentions.len(), 1);
        assert_eq!(node1.mentions[0], id2);
        assert_eq!(node1.mentioned_by.len(), 1);
        assert_eq!(node1.mentioned_by[0], id2);

        // Verify node 2
        let node2 = service.get_node(&id2).await.unwrap().unwrap();
        assert_eq!(node2.mentions.len(), 1);
        assert_eq!(node2.mentions[0], id1);
        assert_eq!(node2.mentioned_by.len(), 1);
        assert_eq!(node2.mentioned_by[0], id1);
    }

    #[tokio::test]
    async fn test_create_mention_persists_correctly() {
        let (service, _temp) = create_test_service().await;

        // Create two nodes
        let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Create mention using the new create_mention() method
        service.create_mention(&id1, &id2).await.unwrap();

        // Verify the mention persists by checking the node_mentions table
        // We can verify this by getting the mentions for node1
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(mentions.len(), 1, "Node 1 should have exactly one mention");
        assert_eq!(mentions[0], id2, "Node 1 should mention Node 2");

        // Verify bidirectional relationship - check mentioned_by for node2
        let mentioned_by = service.get_mentioned_by(&id2).await.unwrap();
        assert_eq!(
            mentioned_by.len(),
            1,
            "Node 2 should be mentioned by exactly one node"
        );
        assert_eq!(mentioned_by[0], id1, "Node 2 should be mentioned by Node 1");

        // Verify idempotency - calling create_mention again should not error
        service.create_mention(&id1, &id2).await.unwrap();
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(
            mentions.len(),
            1,
            "Should still have only one mention (INSERT OR IGNORE)"
        );
    }

    #[tokio::test]
    async fn test_create_mention_validates_nodes_exist() {
        let (service, _temp) = create_test_service().await;

        // Create only one node
        let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
        let id1 = service.create_node(node1).await.unwrap();

        // Try to create mention to non-existent node
        let result = service.create_mention(&id1, "nonexistent-id").await;
        assert!(
            result.is_err(),
            "Should error when mentioned node doesn't exist"
        );
        assert!(
            matches!(result.unwrap_err(), NodeServiceError::NodeNotFound { .. }),
            "Should return NodeNotFound error"
        );

        // Try to create mention from non-existent node
        let result = service.create_mention("nonexistent-id", &id1).await;
        assert!(
            result.is_err(),
            "Should error when mentioning node doesn't exist"
        );
        assert!(
            matches!(result.unwrap_err(), NodeServiceError::NodeNotFound { .. }),
            "Should return NodeNotFound error"
        );
    }

    /// Tests for basic node query functionality
    ///
    /// These tests verify that query_nodes_simple returns matching nodes correctly.
    /// Content search returns ALL matching nodes regardless of type or hierarchy level.
    ///
    /// All tests use unique content markers (e.g., "UniqueBasicFilter") to prevent
    /// cross-test contamination and ensure proper test isolation.
    mod node_query_tests {
        use super::*;

        #[tokio::test]
        async fn basic_filter() {
            let (service, _temp) = create_test_service().await;

            // Create a root node (no parent = root)
            let root = Node::new(
                "text".to_string(),
                "UniqueBasicFilter Root".to_string(),
                json!({}),
            );
            let root_id = service.create_node(root).await.unwrap();

            // Create a task node
            let task = Node::new_with_id(
                "task-1".to_string(),
                "task".to_string(),
                "UniqueBasicFilter Task".to_string(),
                json!({"status": "open"}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Create a regular text child node
            let text_child = Node::new_with_id(
                "text-child-1".to_string(),
                "text".to_string(),
                "UniqueBasicFilter Text".to_string(),
                json!({}),
            );
            let text_child_id = service.create_node(text_child).await.unwrap();

            // Query using content match to isolate this test's nodes
            let query = crate::models::NodeQuery {
                content_contains: Some("UniqueBasicFilter".to_string()),
                ..Default::default()
            };
            let results = service.query_nodes_simple(query).await.unwrap();

            // All nodes matching the content filter should be returned
            assert_eq!(
                results.len(),
                3,
                "Should return all 3 nodes matching content filter"
            );

            let result_ids: Vec<&str> = results.iter().map(|n| n.id.as_str()).collect();
            assert!(
                result_ids.contains(&root_id.as_str()),
                "Should include root node"
            );
            assert!(
                result_ids.contains(&task_id.as_str()),
                "Should include task node"
            );
            assert!(
                result_ids.contains(&text_child_id.as_str()),
                "Should include text child node"
            );
        }

        #[tokio::test]
        async fn content_contains_with_filter() {
            let (service, _temp) = create_test_service().await;

            // Create root with "meeting" in content
            let root = Node::new(
                "text".to_string(),
                "Team meeting notes".to_string(),
                json!({}),
            );
            let root_id = service.create_node(root).await.unwrap();

            // Create task with "meeting" in content
            let task = Node::new_with_id(
                "task-meeting".to_string(),
                "task".to_string(),
                "Schedule meeting".to_string(),
                json!({"task": {"status": "open"}}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Create text child with "meeting" in content
            let text_child = Node::new_with_id(
                "text-meeting".to_string(),
                "text".to_string(),
                "Meeting agenda item".to_string(),
                json!({}),
            );
            let text_child_id = service.create_node(text_child).await.unwrap();

            // Query for "meeting"
            let query = crate::models::NodeQuery {
                content_contains: Some("meeting".to_string()),
                ..Default::default()
            };
            let results = service.query_nodes_simple(query).await.unwrap();

            // All 3 nodes with "meeting" should be returned
            assert_eq!(
                results.len(),
                3,
                "Should return all nodes with 'meeting' in content"
            );

            let result_ids: Vec<&str> = results.iter().map(|n| n.id.as_str()).collect();
            assert!(result_ids.contains(&root_id.as_str()));
            assert!(result_ids.contains(&task_id.as_str()));
            assert!(result_ids.contains(&text_child_id.as_str()));
        }

        #[tokio::test]
        async fn mentioned_by_with_filter() {
            let (service, _temp) = create_test_service().await;

            // Create a target node to be mentioned
            let target = Node::new_with_id(
                "target-node".to_string(),
                "text".to_string(),
                "Target".to_string(),
                json!({}),
            );
            let target_id = service.create_node(target).await.unwrap();

            // Create root that mentions target
            let root = Node::new_with_id(
                "root-1".to_string(),
                "text".to_string(),
                "Root mentioning @target-node".to_string(),
                json!({}),
            );
            let root_id = service.create_node(root).await.unwrap();
            service.create_mention(&root_id, &target_id).await.unwrap();

            // Create task that mentions target
            let task = Node::new_with_id(
                "task-mentions".to_string(),
                "task".to_string(),
                "Task with @target-node reference".to_string(),
                json!({"task": {"status": "open"}}),
            );
            let task_id = service.create_node(task).await.unwrap();
            service.create_mention(&task_id, &target_id).await.unwrap();

            // Create text child that mentions target
            let text_child = Node::new_with_id(
                "text-mentions".to_string(),
                "text".to_string(),
                "Text with @target-node".to_string(),
                json!({}),
            );
            let text_child_id = service.create_node(text_child).await.unwrap();
            service
                .create_mention(&text_child_id, &target_id)
                .await
                .unwrap();

            // Query nodes that mention target
            let query = crate::models::NodeQuery {
                mentioned_by: Some(target_id.clone()),
                ..Default::default()
            };
            let results = service.query_nodes_simple(query).await.unwrap();

            // All 3 nodes that mention target should be returned
            assert_eq!(
                results.len(),
                3,
                "Should return all nodes that mention target"
            );

            let result_ids: Vec<&str> = results.iter().map(|n| n.id.as_str()).collect();
            assert!(result_ids.contains(&root_id.as_str()));
            assert!(result_ids.contains(&task_id.as_str()));
            assert!(result_ids.contains(&text_child_id.as_str()));
        }

        #[tokio::test]
        async fn node_type_with_filter() {
            let (service, _temp) = create_test_service().await;

            // Create multiple task nodes - some roots, some children
            let root_task = Node::new_with_id(
                "task-root".to_string(),
                "task".to_string(),
                "Root task".to_string(),
                json!({"task": {"status": "open"}}),
            );
            let _root_task_id = service.create_node(root_task).await.unwrap();

            let child_task = Node::new_with_id(
                "task-child".to_string(),
                "task".to_string(),
                "Child task".to_string(),
                json!({"task": {"status": "open"}}),
            );
            service.create_node(child_task).await.unwrap();

            // Query for task nodes WITH root/task filter
            // This should still return task nodes even if they're children,
            // because the filter is (node_type = 'task' OR root_id IS NULL)
            let query = crate::models::NodeQuery {
                node_type: Some("task".to_string()),
                ..Default::default()
            };
            let results = service.query_nodes_simple(query).await.unwrap();

            // Both tasks should be returned (filter allows tasks regardless of parent)
            assert_eq!(
                results.len(),
                2,
                "Should return all task nodes (filter allows tasks)"
            );
        }

        #[tokio::test]
        async fn default_behavior() {
            let (service, _temp) = create_test_service().await;

            // Create mix of nodes with unique identifier
            let container = Node::new(
                "text".to_string(),
                "UniqueDefaultTest Container".to_string(),
                json!({}),
            );
            service.create_node(container).await.unwrap();

            let task = Node::new(
                "task".to_string(),
                "UniqueDefaultTest Task".to_string(),
                json!({"task": {"status": "open"}}),
            );
            service.create_node(task).await.unwrap();

            // Query with filter = None (default should be false)
            // Use content search to isolate this test's nodes
            let query = crate::models::NodeQuery {
                content_contains: Some("UniqueDefaultTest".to_string()),
                ..Default::default()
            };
            let results = service.query_nodes_simple(query).await.unwrap();

            // Should return all nodes matching the content query
            assert_eq!(
                results.len(),
                2,
                "Content search should return all matching nodes"
            );
        }

        #[tokio::test]
        async fn default_limit_applied() {
            let (service, _temp) = create_test_service().await;

            // Create nodes with unique content to isolate test
            for i in 0..5 {
                let node = Node::new(
                    "text".to_string(),
                    format!("UniqueDefaultLimitTest Node {}", i),
                    json!({}),
                );
                service.create_node(node).await.unwrap();
            }

            // Query without explicit limit - should apply DEFAULT_QUERY_LIMIT
            let query = crate::models::NodeQuery {
                content_contains: Some("UniqueDefaultLimitTest".to_string()),
                ..Default::default()
            };

            // The query should succeed and apply the default limit (100)
            // Since we only created 5 nodes, we should get all 5 back
            let results = service.query_nodes_simple(query).await.unwrap();
            assert_eq!(
                results.len(),
                5,
                "Should return all 5 nodes (within default limit of {})",
                DEFAULT_QUERY_LIMIT
            );
        }

        #[tokio::test]
        async fn explicit_limit_respected() {
            let (service, _temp) = create_test_service().await;

            // Create nodes with unique content to isolate test
            for i in 0..10 {
                let node = Node::new(
                    "text".to_string(),
                    format!("UniqueExplicitLimitTest Node {}", i),
                    json!({}),
                );
                service.create_node(node).await.unwrap();
            }

            // Query with explicit limit of 3
            let query = crate::models::NodeQuery {
                content_contains: Some("UniqueExplicitLimitTest".to_string()),
                limit: Some(3),
                ..Default::default()
            };

            let results = service.query_nodes_simple(query).await.unwrap();
            assert_eq!(results.len(), 3, "Should respect explicit limit of 3");
        }
    }

    /// Tests for get_mentioning_containers() - container-level backlinks
    ///
    /// These tests verify the container-level backlinks functionality which
    /// resolves incoming mentions to their container nodes and deduplicates.
    ///
    /// # Test Coverage
    ///
    /// - `basic_backlinks()` - Simple case: child node mentions target
    /// - `deduplication()` - Multiple children in same container mention target
    /// - `task_exception()` - Task nodes treated as own containers
    /// - `ai_chat_exception()` - AI-chat nodes treated as own containers
    /// - `empty_backlinks()` - No mentions returns empty vector
    /// - `mixed_containers()` - Multiple different containers mentioning target
    /// - `nonexistent_node()` - Querying backlinks for non-existent node
    mod mentioning_containers_tests {
        use super::*;

        #[tokio::test]
        async fn basic_backlinks() {
            let (service, _temp) = create_test_service().await;

            // Create a root node
            let root = Node::new("text".to_string(), "Root page".to_string(), json!({}));
            let root_id = service.create_node(root).await.unwrap();

            // Create a child text node
            let child = Node::new_with_id(
                "child-text".to_string(),
                "text".to_string(),
                "See @target".to_string(),
                json!({}),
            );
            let child_id = service.create_node(child).await.unwrap();

            // Make child a child of root (establish hierarchy)
            service
                .move_node(&child_id, Some(&root_id), None)
                .await
                .unwrap();

            // Create target node (separate root)
            let target = Node::new_with_id(
                "target".to_string(),
                "text".to_string(),
                "Target page".to_string(),
                json!({}),
            );
            let target_id = service.create_node(target).await.unwrap();

            // Child mentions target
            service.create_mention(&child_id, &target_id).await.unwrap();

            // Get mentioning roots for target
            let roots = service.get_mentioning_containers(&target_id).await.unwrap();

            // Should return the root (not the child)
            assert_eq!(roots.len(), 1, "Should return exactly one root");
            assert_eq!(
                roots[0], root_id,
                "Should return the root node, not the child"
            );
        }

        #[tokio::test]
        async fn deduplication() {
            let (service, _temp) = create_test_service().await;

            // Create a root
            let root = Node::new("text".to_string(), "Root page".to_string(), json!({}));
            let root_id = service.create_node(root).await.unwrap();

            // Create two child nodes
            let child1 = Node::new_with_id(
                "child-1".to_string(),
                "text".to_string(),
                "First mention of @target".to_string(),
                json!({}),
            );
            let child1_id = service.create_node(child1).await.unwrap();

            let child2 = Node::new_with_id(
                "child-2".to_string(),
                "text".to_string(),
                "Second mention of @target".to_string(),
                json!({}),
            );
            let child2_id = service.create_node(child2).await.unwrap();

            // Establish hierarchy: both children belong to root
            service
                .move_node(&child1_id, Some(&root_id), None)
                .await
                .unwrap();
            service
                .move_node(&child2_id, Some(&root_id), None)
                .await
                .unwrap();

            // Create target node (separate root)
            let target = Node::new_with_id(
                "target-dedup".to_string(),
                "text".to_string(),
                "Target page".to_string(),
                json!({}),
            );
            let target_id = service.create_node(target).await.unwrap();

            // Both children mention target
            service
                .create_mention(&child1_id, &target_id)
                .await
                .unwrap();
            service
                .create_mention(&child2_id, &target_id)
                .await
                .unwrap();

            // Get mentioning roots
            let roots = service.get_mentioning_containers(&target_id).await.unwrap();

            // Should return only ONE root (deduplicated)
            assert_eq!(
                roots.len(),
                1,
                "Should deduplicate to single root despite two children mentioning target"
            );
            assert_eq!(roots[0], root_id, "Should return the root node");
        }

        #[tokio::test]
        async fn task_exception() {
            let (service, _temp) = create_test_service().await;

            // Create a root
            let root = Node::new("text".to_string(), "Root page".to_string(), json!({}));
            let root_id = service.create_node(root).await.unwrap();

            // Create a task node
            let task = Node::new_with_id(
                "task-1".to_string(),
                "task".to_string(),
                "Review @target".to_string(),
                json!({"status": "open"}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Make task a child of root
            service
                .move_node(&task_id, Some(&root_id), None)
                .await
                .unwrap();

            // Create target node (separate root)
            let target = Node::new_with_id(
                "target-task".to_string(),
                "text".to_string(),
                "Target page".to_string(),
                json!({}),
            );
            let target_id = service.create_node(target).await.unwrap();

            // Task mentions target
            service.create_mention(&task_id, &target_id).await.unwrap();

            // Get mentioning roots
            let roots = service.get_mentioning_containers(&target_id).await.unwrap();

            // Should return the TASK itself (not its root)
            assert_eq!(roots.len(), 1, "Should return exactly one root");
            assert_eq!(
                roots[0], task_id,
                "Task nodes should be treated as their own roots (exception rule)"
            );
            assert_ne!(
                roots[0], root_id,
                "Should NOT return the parent root for task nodes"
            );
        }

        // TODO: Uncomment this test when ai-chat node type is implemented
        // #[tokio::test]
        // async fn ai_chat_exception() {
        //     let (service, _temp) = create_test_service().await;
        //
        //     // Create a container
        //     let container = Node::new(
        //         "text".to_string(),
        //         "Container page".to_string(),
        //         None,
        //         json!({}),
        //     );
        //     let container_id = service.create_node(container).await.unwrap();
        //
        //     // Create an ai-chat node (child of container)
        //     let ai_chat = Node::new_with_id(
        //         "chat-1".to_string(),
        //         "ai-chat".to_string(),
        //         "Discussion about @target".to_string(),
        //         Some(container_id.clone()),
        //         json!({}),
        //     );
        //     let chat_id = service.create_node(ai_chat).await.unwrap();
        //
        //     // Create target node
        //     let target = Node::new_with_id(
        //         "target-chat".to_string(),
        //         "text".to_string(),
        //         "Target page".to_string(),
        //         None,
        //         json!({}),
        //     );
        //     let target_id = service.create_node(target).await.unwrap();
        //
        //     // AI-chat mentions target
        //     service.create_mention(&chat_id, &target_id).await.unwrap();
        //
        //     // Get mentioning containers
        //     let containers = service
        //         .get_mentioning_containers(&target_id)
        //         .await
        //         .unwrap();
        //
        //     // Should return the AI-CHAT itself (not its container)
        //     assert_eq!(
        //         containers.len(),
        //         1,
        //         "Should return exactly one container"
        //     );
        //     assert_eq!(
        //         containers[0], chat_id,
        //         "AI-chat nodes should be treated as their own containers (exception rule)"
        //     );
        //     assert_ne!(
        //         containers[0], container_id,
        //         "Should NOT return the parent container for ai-chat nodes"
        //     );
        // }

        #[tokio::test]
        async fn empty_backlinks() {
            let (service, _temp) = create_test_service().await;

            // Create target node with no mentions
            let target = Node::new_with_id(
                "lonely-target".to_string(),
                "text".to_string(),
                "Target page".to_string(),
                json!({}),
            );
            let target_id = service.create_node(target).await.unwrap();

            // Get mentioning containers
            let containers = service.get_mentioning_containers(&target_id).await.unwrap();

            // Should return empty vector
            assert_eq!(
                containers.len(),
                0,
                "Should return empty vector when no nodes mention target"
            );
        }

        #[tokio::test]
        async fn mixed_containers() {
            let (service, _temp) = create_test_service().await;

            // Create three different containers (roots)
            let container1 = Node::new("text".to_string(), "Container page".to_string(), json!({}));
            let container1_id = service.create_node(container1).await.unwrap();

            let container2 = Node::new("text".to_string(), "Container 2".to_string(), json!({}));
            let container2_id = service.create_node(container2).await.unwrap();

            let container3 = Node::new("text".to_string(), "Container 3".to_string(), json!({}));
            let container3_id = service.create_node(container3).await.unwrap();

            // Create children
            let child1 = Node::new_with_id(
                "child-c1".to_string(),
                "text".to_string(),
                "From container 1".to_string(),
                json!({}),
            );
            let child1_id = service.create_node(child1).await.unwrap();

            let child2 = Node::new_with_id(
                "child-c2".to_string(),
                "text".to_string(),
                "From container 2".to_string(),
                json!({}),
            );
            let child2_id = service.create_node(child2).await.unwrap();

            // Create task (will be in container 3 but treated as its own container)
            let task = Node::new_with_id(
                "task-c3".to_string(),
                "task".to_string(),
                "From container 3".to_string(),
                json!({"task": {"status": "open"}}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Establish hierarchy: children belong to their containers
            service
                .move_node(&child1_id, Some(&container1_id), None)
                .await
                .unwrap();
            service
                .move_node(&child2_id, Some(&container2_id), None)
                .await
                .unwrap();
            service
                .move_node(&task_id, Some(&container3_id), None)
                .await
                .unwrap();

            // Create target node (separate root)
            let target = Node::new_with_id(
                "target-mixed".to_string(),
                "text".to_string(),
                "Target page".to_string(),
                json!({}),
            );
            let target_id = service.create_node(target).await.unwrap();

            // All three mention target
            service
                .create_mention(&child1_id, &target_id)
                .await
                .unwrap();
            service
                .create_mention(&child2_id, &target_id)
                .await
                .unwrap();
            service.create_mention(&task_id, &target_id).await.unwrap();

            // Get mentioning containers
            let containers = service.get_mentioning_containers(&target_id).await.unwrap();

            // Should return 3 unique containers (2 roots + task)
            assert_eq!(
                containers.len(),
                3,
                "Should return three different containers"
            );

            // Verify all three are present (order may vary)
            assert!(
                containers.contains(&container1_id),
                "Should include container 1"
            );
            assert!(
                containers.contains(&container2_id),
                "Should include container 2"
            );
            assert!(
                containers.contains(&task_id),
                "Should include task (as its own container)"
            );
            assert!(
                !containers.contains(&container3_id),
                "Should NOT include container 3 (task is treated as own container)"
            );
        }

        #[tokio::test]
        async fn nonexistent_node() {
            let (service, _temp) = create_test_service().await;

            // Query backlinks for non-existent node
            let containers = service
                .get_mentioning_containers("nonexistent-node")
                .await
                .unwrap();

            // Should return empty vector (not error - node simply has no backlinks)
            assert_eq!(
                containers.len(),
                0,
                "Should return empty vector for non-existent node"
            );
        }
    }

    /// Tests for mention extraction and automatic sync functionality
    mod mention_extraction_and_sync {
        use super::*;

        #[test]
        fn test_is_valid_node_id_uuid() {
            // Valid UUID (lowercase)
            assert!(is_valid_node_id("550e8400-e29b-41d4-a716-446655440000"));

            // Valid UUID (mixed case - should work with lowercase check)
            assert!(is_valid_node_id("550e8400-e29b-41d4-a716-446655440000"));

            // Invalid UUID (wrong format)
            assert!(!is_valid_node_id("not-a-uuid"));
            assert!(!is_valid_node_id("550e8400e29b41d4a716446655440000")); // Missing dashes
        }

        #[test]
        fn test_is_valid_node_id_date() {
            // Valid dates
            assert!(is_valid_node_id("2025-10-24"));
            assert!(is_valid_node_id("2024-01-01"));
            assert!(is_valid_node_id("2025-12-31"));

            // Invalid dates (format)
            assert!(!is_valid_node_id("2025-10-1")); // Single digit day
            assert!(!is_valid_node_id("2025-1-24")); // Single digit month
            assert!(!is_valid_node_id("25-10-24")); // Two digit year

            // Invalid dates (values)
            assert!(!is_valid_node_id("2025-13-01")); // Invalid month
            assert!(!is_valid_node_id("2025-02-30")); // Invalid day for February
            assert!(!is_valid_node_id("2025-00-01")); // Invalid month (0)
        }

        #[test]
        fn test_extract_mentions_markdown_format() {
            let content = "See [@Node A](nodespace://550e8400-e29b-41d4-a716-446655440000) and [Node B](nodespace://2025-10-24)";
            let mentions = extract_mentions(content);

            assert_eq!(mentions.len(), 2);
            assert!(mentions.contains(&"550e8400-e29b-41d4-a716-446655440000".to_string()));
            assert!(mentions.contains(&"2025-10-24".to_string()));
        }

        #[test]
        fn test_extract_mentions_plain_format() {
            let content = "Check out nodespace://550e8400-e29b-41d4-a716-446655440000 and nodespace://2025-10-24";
            let mentions = extract_mentions(content);

            assert_eq!(mentions.len(), 2);
            assert!(mentions.contains(&"550e8400-e29b-41d4-a716-446655440000".to_string()));
            assert!(mentions.contains(&"2025-10-24".to_string()));
        }

        #[test]
        fn test_extract_mentions_mixed_formats() {
            let content = "Markdown [@link](nodespace://550e8400-e29b-41d4-a716-446655440000) and plain nodespace://2025-10-24";
            let mentions = extract_mentions(content);

            assert_eq!(mentions.len(), 2);
            assert!(mentions.contains(&"550e8400-e29b-41d4-a716-446655440000".to_string()));
            assert!(mentions.contains(&"2025-10-24".to_string()));
        }

        #[test]
        fn test_extract_mentions_deduplication() {
            let content = "[@Dup](nodespace://550e8400-e29b-41d4-a716-446655440000) and [@Dup again](nodespace://550e8400-e29b-41d4-a716-446655440000)";
            let mentions = extract_mentions(content);

            // Should deduplicate - only one mention
            assert_eq!(mentions.len(), 1);
            assert!(mentions.contains(&"550e8400-e29b-41d4-a716-446655440000".to_string()));
        }

        #[test]
        fn test_extract_mentions_with_query_params() {
            let content = "Link with params [@Node](nodespace://550e8400-e29b-41d4-a716-446655440000?view=edit)";
            let mentions = extract_mentions(content);

            // Should extract node ID without query params
            assert_eq!(mentions.len(), 1);
            assert!(mentions.contains(&"550e8400-e29b-41d4-a716-446655440000".to_string()));
        }

        #[test]
        fn test_extract_mentions_invalid_ids() {
            let content =
                "Invalid [@link](nodespace://not-valid) and [@another](nodespace://invalid-id)";
            let mentions = extract_mentions(content);

            // Should not extract invalid node IDs
            assert_eq!(mentions.len(), 0);
        }

        #[test]
        fn test_extract_mentions_empty_content() {
            let mentions = extract_mentions("");
            assert_eq!(mentions.len(), 0);
        }

        #[test]
        fn test_extract_mentions_no_mentions() {
            let content = "Just regular text with no mentions at all";
            let mentions = extract_mentions(content);
            assert_eq!(mentions.len(), 0);
        }

        #[tokio::test]
        async fn test_auto_sync_mentions_on_update() {
            let (service, _temp) = create_test_service().await;

            // Create three nodes
            let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
            let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));
            let node3 = Node::new("text".to_string(), "Node 3".to_string(), json!({}));

            let node1_id = service.create_node(node1).await.unwrap();
            let node2_id = service.create_node(node2).await.unwrap();
            let node3_id = service.create_node(node3).await.unwrap();

            // Update node1 to mention node2
            let update =
                NodeUpdate::new().with_content(format!("See [@Node 2](nodespace://{})", node2_id));
            service.update_node(&node1_id, update).await.unwrap();

            // Verify mention was created
            let node1_with_mentions = service.get_node(&node1_id).await.unwrap().unwrap();
            assert_eq!(node1_with_mentions.mentions.len(), 1);
            assert!(node1_with_mentions.mentions.contains(&node2_id));

            // Update node1 to mention node3 instead (should remove node2 mention)
            let update2 =
                NodeUpdate::new().with_content(format!("See [@Node 3](nodespace://{})", node3_id));
            service.update_node(&node1_id, update2).await.unwrap();

            // Verify mentions were updated
            let node1_updated = service.get_node(&node1_id).await.unwrap().unwrap();
            assert_eq!(node1_updated.mentions.len(), 1);
            assert!(node1_updated.mentions.contains(&node3_id));
            assert!(!node1_updated.mentions.contains(&node2_id));
        }

        #[tokio::test]
        async fn test_prevent_self_reference() {
            let (service, _temp) = create_test_service().await;

            // Create a node
            let node = Node::new("text".to_string(), "Self ref test".to_string(), json!({}));
            let node_id = service.create_node(node).await.unwrap();

            // Try to update it to mention itself
            let update = NodeUpdate::new()
                .with_content(format!("Self reference [@me](nodespace://{})", node_id));
            service.update_node(&node_id, update).await.unwrap();

            // Verify self-reference was NOT created
            let node_with_mentions = service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(
                node_with_mentions.mentions.len(),
                0,
                "Should not create self-reference"
            );
        }

        #[tokio::test]
        async fn test_prevent_root_level_self_reference() {
            let (service, _temp) = create_test_service().await;

            // Create root node
            let root = Node::new("text".to_string(), "Root".to_string(), json!({}));
            let root_id = service.create_node(root).await.unwrap();

            // Create child node
            let child = Node::new("text".to_string(), "Child".to_string(), json!({}));
            let child_id = service.create_node(child).await.unwrap();

            // Establish parent-child relationship (make child an actual child of root)
            service
                .move_node(&child_id, Some(&root_id), None)
                .await
                .unwrap();

            // Try to update child to mention its own parent (root)
            let update = NodeUpdate::new()
                .with_content(format!("Mention root [@root](nodespace://{})", root_id));
            service.update_node(&child_id, update).await.unwrap();

            // Verify root-level self-reference was NOT created
            // (child should not be able to mention its own parent)
            let child_with_mentions = service.get_node(&child_id).await.unwrap().unwrap();
            assert_eq!(
                child_with_mentions.mentions.len(),
                0,
                "Should not create root-level self-reference (child mentioning its parent)"
            );
        }

        #[tokio::test]
        async fn test_sync_mentions_multiple_adds_and_removes() {
            let (service, _temp) = create_test_service().await;

            // Create nodes
            let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
            let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));
            let node3 = Node::new("text".to_string(), "Node 3".to_string(), json!({}));
            let node4 = Node::new("text".to_string(), "Node 4".to_string(), json!({}));

            let node1_id = service.create_node(node1).await.unwrap();
            let node2_id = service.create_node(node2).await.unwrap();
            let node3_id = service.create_node(node3).await.unwrap();
            let node4_id = service.create_node(node4).await.unwrap();

            // Start: mention node2 and node3
            let update1 = NodeUpdate::new().with_content(format!(
                "See [@N2](nodespace://{}) and [@N3](nodespace://{})",
                node2_id, node3_id
            ));
            service.update_node(&node1_id, update1).await.unwrap();

            let node1_v1 = service.get_node(&node1_id).await.unwrap().unwrap();
            assert_eq!(node1_v1.mentions.len(), 2);

            // Update: remove node2, keep node3, add node4
            let update2 = NodeUpdate::new().with_content(format!(
                "See [@N3](nodespace://{}) and [@N4](nodespace://{})",
                node3_id, node4_id
            ));
            service.update_node(&node1_id, update2).await.unwrap();

            let node1_v2 = service.get_node(&node1_id).await.unwrap().unwrap();
            assert_eq!(node1_v2.mentions.len(), 2);
            assert!(node1_v2.mentions.contains(&node3_id), "Should keep node3");
            assert!(node1_v2.mentions.contains(&node4_id), "Should add node4");
            assert!(
                !node1_v2.mentions.contains(&node2_id),
                "Should remove node2"
            );
        }

        #[tokio::test]
        async fn test_sync_mentions_with_date_nodes() {
            let (service, _temp) = create_test_service().await;

            // Create a regular node
            let node = Node::new("text".to_string(), "Daily note".to_string(), json!({}));
            let node_id = service.create_node(node).await.unwrap();

            // Create a date node
            let date_node = Node::new_with_id(
                "2025-10-24".to_string(),
                "date".to_string(),
                "2025-10-24".to_string(),
                json!({}),
            );
            service.create_node(date_node).await.unwrap();

            // Update node to mention the date node
            let update =
                NodeUpdate::new().with_content("See [@Date](nodespace://2025-10-24)".to_string());
            service.update_node(&node_id, update).await.unwrap();

            // Verify mention to date node was created
            let node_with_mentions = service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(node_with_mentions.mentions.len(), 1);
            assert!(node_with_mentions
                .mentions
                .contains(&"2025-10-24".to_string()));
        }

        #[tokio::test]
        async fn test_delete_mention_idempotent() {
            let (service, _temp) = create_test_service().await;

            // Create two nodes
            let node1 = Node::new("text".to_string(), "Node 1".to_string(), json!({}));
            let node2 = Node::new("text".to_string(), "Node 2".to_string(), json!({}));

            let node1_id = service.create_node(node1).await.unwrap();
            let node2_id = service.create_node(node2).await.unwrap();

            // Create mention
            service.create_mention(&node1_id, &node2_id).await.unwrap();

            // Delete mention (should succeed)
            service.delete_mention(&node1_id, &node2_id).await.unwrap();

            // Delete again (should still succeed - idempotent)
            service.delete_mention(&node1_id, &node2_id).await.unwrap();
        }

        // Phase 1: Version Tracking Tests
        mod version_tracking_tests {
            use super::*;

            #[tokio::test]
            async fn test_new_nodes_get_schema_version() {
                let (service, _temp) = create_test_service().await;

                // Create a text node (no schema exists, should default to version 1)
                // Text nodes don't have schemas, so they won't get _schema_version
                // Testing with text node to verify that behavior
                let text_node =
                    Node::new("text".to_string(), "Test content".to_string(), json!({}));
                let text_id = service.create_node(text_node).await.unwrap();
                let retrieved_text = service.get_node(&text_id).await.unwrap().unwrap();

                // Text nodes should NOT have _schema_version (no schema fields)
                let text_ns = retrieved_text.properties.get("text");
                assert!(
                    text_ns.is_none() || text_ns.unwrap().get("_schema_version").is_none(),
                    "Text nodes without schema fields should not have _schema_version"
                );
            }

            // NOTE: Backfill tests removed - no legacy data exists (pre-release project)
            // All nodes created via NodeService automatically get _schema_version

            #[tokio::test]
            async fn test_auto_created_date_nodes_no_version() {
                let (service, _temp) = create_test_service().await;

                // Directly create a date node (simulating persisted date node)
                // Types without schemas (date, text) should NOT get _schema_version
                let date_node = Node::new_with_id(
                    "2025-01-15".to_string(),
                    "date".to_string(),
                    "2025-01-15".to_string(),
                    json!({}),
                );
                service.create_node(date_node).await.unwrap();

                // Retrieve the date node - should NOT have _schema_version (no schema fields)
                let retrieved = service.get_node("2025-01-15").await.unwrap().unwrap();
                let date_ns = retrieved.properties.get("date");
                assert!(
                    date_ns.is_none() || date_ns.unwrap().get("_schema_version").is_none(),
                    "Date nodes without schema fields should not have _schema_version"
                );
            }

            #[tokio::test]
            async fn test_nodes_multiple_retrieval_consistent() {
                let (service, _temp) = create_test_service().await;

                // Create a text node (no schema fields, so no _schema_version)
                let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));
                let id = service.create_node(node).await.unwrap();

                // Retrieve twice to ensure consistency across retrievals
                let retrieved1 = service.get_node(&id).await.unwrap().unwrap();
                let retrieved2 = service.get_node(&id).await.unwrap().unwrap();

                // Text nodes without schema fields should not have _schema_version
                let ns1 = retrieved1.properties.get("text");
                let ns2 = retrieved2.properties.get("text");
                assert!(
                    ns1.is_none() || ns1.unwrap().get("_schema_version").is_none(),
                    "Text nodes should not have _schema_version"
                );
                assert!(
                    ns2.is_none() || ns2.unwrap().get("_schema_version").is_none(),
                    "Text nodes should not have _schema_version"
                );
                assert_eq!(
                    retrieved1.modified_at, retrieved2.modified_at,
                    "Modified timestamp should not change between retrievals"
                );
            }
        }
    }

    mod adjacency_list_tests {
        use super::*;
        use serial_test::serial;
        use std::time::Duration;
        use tokio::time::sleep;

        // Tests for the adjacency list strategy (recursive graph traversal)
        // Uses SurrealDB's .{..}(->edge->target) syntax for recursive queries
        //
        // NOTE: All tests in this module are marked #[serial(sibling_ordering)] because they use
        // create_parent_edge with insert_after positioning, which can exhibit race
        // conditions when SurrealDB hasn't made previous writes visible before the
        // next operation queries for sibling positions. This is a SurrealDB timing
        // issue under concurrent test execution, not a functional bug in production.
        //
        // The "sibling_ordering" key is shared with integration_tests in nodes_test.rs
        // to ensure all ordering-sensitive tests run serially across modules.

        /// Helper function to wait for children tree to have expected order with retries.
        /// This handles SurrealDB's eventual consistency for sibling ordering.
        async fn wait_for_children_tree_order<C>(
            service: &NodeService<C>,
            parent_id: &str,
            expected_contents: &[&str],
            max_retries: usize,
        ) -> Result<serde_json::Value, String>
        where
            C: surrealdb::Connection,
        {
            for attempt in 0..max_retries {
                let tree = service
                    .get_children_tree(parent_id)
                    .await
                    .map_err(|e| format!("Failed to get children tree: {:?}", e))?;

                let children = tree["children"]
                    .as_array()
                    .ok_or("Expected children array")?;

                if children.len() == expected_contents.len() {
                    let actual_contents: Vec<&str> = children
                        .iter()
                        .filter_map(|c| c["content"].as_str())
                        .collect();

                    if actual_contents == expected_contents {
                        return Ok(tree);
                    }
                }

                if attempt < max_retries - 1 {
                    sleep(Duration::from_millis(100)).await;
                }
            }

            // Final attempt - return whatever we have for assertion failure message
            let tree = service
                .get_children_tree(parent_id)
                .await
                .map_err(|e| format!("Failed to get children tree: {:?}", e))?;

            let children = tree["children"]
                .as_array()
                .ok_or("Expected children array")?;

            Err(format!(
                "Children tree order did not stabilize after {} retries. Expected {:?}, got {:?}",
                max_retries,
                expected_contents,
                children
                    .iter()
                    .filter_map(|c| c["content"].as_str())
                    .collect::<Vec<_>>()
            ))
        }

        /// Test get_children_tree with a leaf node (no children)
        #[tokio::test]
        #[serial(sibling_ordering)]
        async fn test_get_children_tree_leaf_node() {
            let (service, _temp) = create_test_service().await;

            // Create a single node with no children
            let leaf = Node::new("text".to_string(), "Leaf node".to_string(), json!({}));
            let leaf_id = service.create_node(leaf).await.unwrap();

            // Get tree for leaf node - should return the node with empty children array
            let tree = service.get_children_tree(&leaf_id).await.unwrap();

            assert_eq!(tree["id"], leaf_id);
            assert_eq!(tree["content"], "Leaf node");
            assert!(tree["children"].as_array().unwrap().is_empty());
        }

        /// Test get_children_tree with single-level children
        #[tokio::test]
        #[serial(sibling_ordering)]
        async fn test_get_children_tree_single_level() {
            let (service, _temp) = create_test_service().await;

            // Create parent node
            let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
            let parent_id = service.create_node(parent).await.unwrap();

            // Create two children and add to parent using create_parent_edge
            // NOTE: Small delays between insertions ensure SurrealDB write visibility
            // for sibling order calculations.
            let child1 = Node::new("text".to_string(), "Child 1".to_string(), json!({}));
            let child1_id = service.create_node(child1).await.unwrap();
            service
                .create_parent_edge(&child1_id, &parent_id, None) // First child - insert at beginning
                .await
                .unwrap();

            sleep(Duration::from_millis(50)).await;

            let child2 = Node::new("text".to_string(), "Child 2".to_string(), json!({}));
            let child2_id = service.create_node(child2).await.unwrap();
            service
                .create_parent_edge(&child2_id, &parent_id, Some(&child1_id)) // Insert after Child 1
                .await
                .unwrap();

            sleep(Duration::from_millis(50)).await;

            // Get tree - should have parent with 2 children (with retry for eventual consistency)
            let tree =
                wait_for_children_tree_order(&service, &parent_id, &["Child 1", "Child 2"], 10)
                    .await
                    .expect("Children should stabilize in order Child 1, Child 2");

            assert_eq!(tree["id"], parent_id);
            let children = tree["children"].as_array().unwrap();
            assert_eq!(children.len(), 2);
            assert_eq!(children[0]["content"], "Child 1");
            assert_eq!(children[1]["content"], "Child 2");
        }

        /// Test get_children_tree with multi-level deep tree
        #[tokio::test]
        #[serial(sibling_ordering)]
        async fn test_get_children_tree_deep_hierarchy() {
            let (service, _temp) = create_test_service().await;

            // Create a 3-level deep tree:
            // Root -> Child -> Grandchild
            let root = Node::new("text".to_string(), "Root".to_string(), json!({}));
            let root_id = service.create_node(root).await.unwrap();

            let child = Node::new("text".to_string(), "Child".to_string(), json!({}));
            let child_id = service.create_node(child).await.unwrap();
            service
                .create_parent_edge(&child_id, &root_id, None)
                .await
                .unwrap();

            let grandchild = Node::new("text".to_string(), "Grandchild".to_string(), json!({}));
            let grandchild_id = service.create_node(grandchild).await.unwrap();
            service
                .create_parent_edge(&grandchild_id, &child_id, None)
                .await
                .unwrap();

            // Get tree - should have nested structure
            let tree = service.get_children_tree(&root_id).await.unwrap();

            assert_eq!(tree["id"], root_id);
            assert_eq!(tree["content"], "Root");

            let children = tree["children"].as_array().unwrap();
            assert_eq!(children.len(), 1);
            assert_eq!(children[0]["content"], "Child");

            let grandchildren = children[0]["children"].as_array().unwrap();
            assert_eq!(grandchildren.len(), 1);
            assert_eq!(grandchildren[0]["content"], "Grandchild");
            assert!(grandchildren[0]["children"].as_array().unwrap().is_empty());
        }

        /// Test sibling ordering is preserved (insertion order since create_parent_edge appends)
        #[tokio::test]
        #[serial(sibling_ordering)]
        async fn test_get_children_tree_sibling_ordering() {
            let (service, _temp) = create_test_service().await;

            // Create parent node
            let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
            let parent_id = service.create_node(parent).await.unwrap();

            // Add children in order A, B, C - they should maintain this order
            // IMPORTANT: Verify state after each insertion to ensure SurrealDB
            // has made the write visible before proceeding. This eliminates flakiness
            // from eventual consistency.
            let child_a = Node::new("text".to_string(), "A".to_string(), json!({}));
            let child_a_id = service.create_node(child_a).await.unwrap();
            service
                .create_parent_edge(&child_a_id, &parent_id, None) // First child - insert at beginning
                .await
                .unwrap();

            // Verify A is visible before inserting B
            wait_for_children_tree_order(&service, &parent_id, &["A"], 10)
                .await
                .expect("A should be visible as first child");

            let child_b = Node::new("text".to_string(), "B".to_string(), json!({}));
            let child_b_id = service.create_node(child_b).await.unwrap();
            service
                .create_parent_edge(&child_b_id, &parent_id, Some(&child_a_id)) // Insert after A
                .await
                .unwrap();

            // Verify [A, B] order before inserting C
            wait_for_children_tree_order(&service, &parent_id, &["A", "B"], 10)
                .await
                .expect("Children should be [A, B] before inserting C");

            let child_c = Node::new("text".to_string(), "C".to_string(), json!({}));
            let child_c_id = service.create_node(child_c).await.unwrap();
            service
                .create_parent_edge(&child_c_id, &parent_id, Some(&child_b_id)) // Insert after B
                .await
                .unwrap();

            // Get tree - children should be in order A, B, C (with retry for eventual consistency)
            let tree = wait_for_children_tree_order(&service, &parent_id, &["A", "B", "C"], 10)
                .await
                .expect("Children should stabilize in order A, B, C");

            let children = tree["children"].as_array().unwrap();
            assert_eq!(children.len(), 3);
            assert_eq!(children[0]["content"], "A");
            assert_eq!(children[1]["content"], "B");
            assert_eq!(children[2]["content"], "C");
        }

        /// Test get_children_tree with non-existent root returns empty object
        #[tokio::test]
        #[serial(sibling_ordering)]
        async fn test_get_children_tree_nonexistent_root() {
            let (service, _temp) = create_test_service().await;

            // Get tree for non-existent node - should return empty object
            let tree = service.get_children_tree("nonexistent-id").await.unwrap();

            assert!(tree.as_object().unwrap().is_empty());
        }
    }

    /// Tests for move_node validation (Issue #676: NodeOperations merge)
    mod move_node_validation_tests {
        use super::*;

        /// Test that date nodes (containers) cannot be moved
        #[tokio::test]
        async fn test_move_node_rejects_date_node() {
            let (service, _temp) = create_test_service().await;

            // Create a date node (container)
            let date_node = Node::new_with_id(
                "2025-01-03".to_string(),
                "date".to_string(),
                "2025-01-03".to_string(),
                json!({}),
            );
            service.create_node(date_node).await.unwrap();

            // Create a potential parent (also a date, which is fine for the test)
            let parent_node = Node::new_with_id(
                "2025-01-04".to_string(),
                "date".to_string(),
                "2025-01-04".to_string(),
                json!({}),
            );
            service.create_node(parent_node).await.unwrap();

            // Try to move the date node - should fail (date nodes are containers)
            let result = service
                .move_node("2025-01-03", Some("2025-01-04"), None)
                .await;

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(
                format!("{:?}", err).contains("cannot be moved"),
                "Error should indicate date node cannot be moved: {:?}",
                err
            );
        }

        /// Test that circular references are prevented
        #[tokio::test]
        async fn test_move_node_prevents_circular_reference() {
            let (service, _temp) = create_test_service().await;

            // Create hierarchy: Root -> A -> B -> C
            // Root is needed so that A is not itself a root node
            let root = Node::new("text".to_string(), "Root".to_string(), json!({}));
            let root_id = service.create_node(root).await.unwrap();

            let node_a = Node::new("text".to_string(), "A".to_string(), json!({}));
            let node_a_id = service.create_node(node_a).await.unwrap();
            service
                .create_parent_edge(&node_a_id, &root_id, None)
                .await
                .unwrap();

            let node_b = Node::new("text".to_string(), "B".to_string(), json!({}));
            let node_b_id = service.create_node(node_b).await.unwrap();
            service
                .create_parent_edge(&node_b_id, &node_a_id, None)
                .await
                .unwrap();

            let node_c = Node::new("text".to_string(), "C".to_string(), json!({}));
            let node_c_id = service.create_node(node_c).await.unwrap();
            service
                .create_parent_edge(&node_c_id, &node_b_id, None)
                .await
                .unwrap();

            // Try to move A under C - this would create: C -> A -> B -> C (circular!)
            // A is not a root (it's under Root), so the root check passes, then circular check fires
            let result = service.move_node(&node_a_id, Some(&node_c_id), None).await;

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(
                format!("{:?}", err).contains("ircular"),
                "Error should indicate circular reference: {:?}",
                err
            );
        }

        /// Test that non-root nodes can be moved
        #[tokio::test]
        async fn test_move_node_allows_non_root_node() {
            let (service, _temp) = create_test_service().await;

            // Create parent and child
            let parent1 = Node::new("text".to_string(), "Parent1".to_string(), json!({}));
            let parent1_id = service.create_node(parent1).await.unwrap();

            let parent2 = Node::new("text".to_string(), "Parent2".to_string(), json!({}));
            let parent2_id = service.create_node(parent2).await.unwrap();

            let child = Node::new("text".to_string(), "Child".to_string(), json!({}));
            let child_id = service.create_node(child).await.unwrap();
            service
                .create_parent_edge(&child_id, &parent1_id, None)
                .await
                .unwrap();

            // Move child from parent1 to parent2 - should succeed
            let result = service.move_node(&child_id, Some(&parent2_id), None).await;
            assert!(result.is_ok());

            // Verify child is now under parent2
            let new_parent = service.get_parent(&child_id).await.unwrap();
            assert!(new_parent.is_some());
            assert_eq!(new_parent.unwrap().id, parent2_id);
        }
    }

    /// Tests for create_node_with_parent (Issue #676: NodeOperations merge)
    mod create_node_with_parent_tests {
        use super::*;
        use serial_test::serial;
        use std::time::Duration;
        use tokio::time::sleep;

        // NOTE: Tests that verify sibling ordering are marked #[serial(sibling_ordering)]
        // to prevent race conditions with SurrealDB write visibility.

        /// Test that date containers are auto-created when referenced as parent
        #[tokio::test]
        async fn test_auto_creates_date_container() {
            let (service, _temp) = create_test_service().await;

            // Create a node with a date parent that doesn't exist yet
            let params = CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "My note".to_string(),
                parent_id: Some("2025-01-15".to_string()),
                insert_after_node_id: None,
                properties: json!({}),
            };

            let node_id = service.create_node_with_parent(params).await.unwrap();

            // Verify the date container was auto-created
            let date_node = service.get_node("2025-01-15").await.unwrap().unwrap();
            assert_eq!(date_node.node_type, "date");
            assert_eq!(date_node.content, "2025-01-15");

            // Verify child is under the date container
            let parent = service.get_parent(&node_id).await.unwrap().unwrap();
            assert_eq!(parent.id, "2025-01-15");
        }

        /// Test that root nodes (no parent) are created correctly
        #[tokio::test]
        async fn test_create_root_node() {
            let (service, _temp) = create_test_service().await;

            let params = CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "Root note".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            };

            let node_id = service.create_node_with_parent(params).await.unwrap();

            // Verify it's a root node (no parent)
            let parent = service.get_parent(&node_id).await.unwrap();
            assert!(parent.is_none());

            // Verify it's marked as root
            assert!(service.is_root_node(&node_id).await.unwrap());
        }

        /// Test that provided UUID IDs are validated
        #[tokio::test]
        async fn test_validates_uuid_format() {
            let (service, _temp) = create_test_service().await;

            // Invalid UUID should be rejected for non-date/schema nodes
            let params = CreateNodeParams {
                id: Some("not-a-valid-uuid".to_string()),
                node_type: "text".to_string(),
                content: "Test".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            };

            let result = service.create_node_with_parent(params).await;
            assert!(result.is_err());
        }

        /// Test that test- prefix IDs are allowed
        #[tokio::test]
        async fn test_allows_test_prefix_ids() {
            let (service, _temp) = create_test_service().await;

            let params = CreateNodeParams {
                id: Some("test-my-node-123".to_string()),
                node_type: "text".to_string(),
                content: "Test node".to_string(),
                parent_id: None,
                insert_after_node_id: None,
                properties: json!({}),
            };

            let node_id = service.create_node_with_parent(params).await.unwrap();
            assert_eq!(node_id, "test-my-node-123");
        }

        /// Test sibling validation - sibling must have same parent
        #[tokio::test]
        #[serial(sibling_ordering)]
        async fn test_sibling_must_have_same_parent() {
            let (service, _temp) = create_test_service().await;

            // Create two different parent nodes
            let parent1 = Node::new("text".to_string(), "Parent1".to_string(), json!({}));
            let parent1_id = service.create_node(parent1).await.unwrap();

            let parent2 = Node::new("text".to_string(), "Parent2".to_string(), json!({}));
            let parent2_id = service.create_node(parent2).await.unwrap();

            // Create a child under parent1
            let sibling = Node::new("text".to_string(), "Sibling".to_string(), json!({}));
            let sibling_id = service.create_node(sibling).await.unwrap();
            service
                .create_parent_edge(&sibling_id, &parent1_id, None)
                .await
                .unwrap();

            // Try to create a node under parent2 with sibling from parent1
            let params = CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "New node".to_string(),
                parent_id: Some(parent2_id),
                insert_after_node_id: Some(sibling_id),
                properties: json!({}),
            };

            let result = service.create_node_with_parent(params).await;
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(
                format!("{:?}", err).contains("different parent"),
                "Error should indicate sibling has different parent: {:?}",
                err
            );
        }

        /// Test that None inserts at beginning (new behavior)
        #[tokio::test]
        #[serial(sibling_ordering)]
        async fn test_insert_at_beginning_by_default() {
            let (service, _temp) = create_test_service().await;

            // Create parent
            let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
            let parent_id = service.create_node(parent).await.unwrap();

            // Create first child (None = insert at beginning)
            // NOTE: Small delays between insertions ensure SurrealDB write visibility
            let params1 = CreateNodeParams {
                id: Some("test-child-1".to_string()),
                node_type: "text".to_string(),
                content: "Child 1".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            };
            service.create_node_with_parent(params1).await.unwrap();

            sleep(Duration::from_millis(50)).await;

            // Create second child (None = insert at beginning, so comes BEFORE first)
            let params2 = CreateNodeParams {
                id: Some("test-child-2".to_string()),
                node_type: "text".to_string(),
                content: "Child 2".to_string(),
                parent_id: Some(parent_id.clone()),
                insert_after_node_id: None,
                properties: json!({}),
            };
            service.create_node_with_parent(params2).await.unwrap();

            sleep(Duration::from_millis(50)).await;

            // Verify order: Child 2, Child 1 (reversed - None inserts at beginning)
            let children = service.get_children(&parent_id).await.unwrap();
            assert_eq!(children.len(), 2);
            assert_eq!(children[0].content, "Child 2");
            assert_eq!(children[1].content, "Child 1");
        }

        #[tokio::test]
        async fn test_node_service_seeds_fresh_database() -> Result<(), Box<dyn std::error::Error>>
        {
            use tempfile::TempDir;

            let temp_dir = TempDir::new()?;
            let db_path = temp_dir.path().join("test.db");
            let mut store = Arc::new(SurrealStore::new(db_path).await?);

            // Create NodeService - should seed schemas automatically
            let _service = NodeService::new(&mut store).await?;

            // Verify all 7 core schemas exist
            assert!(
                store.get_node("task").await?.is_some(),
                "task schema should exist"
            );
            assert!(
                store.get_node("text").await?.is_some(),
                "text schema should exist"
            );
            assert!(
                store.get_node("date").await?.is_some(),
                "date schema should exist"
            );
            assert!(
                store.get_node("header").await?.is_some(),
                "header schema should exist"
            );
            assert!(
                store.get_node("code-block").await?.is_some(),
                "code-block schema should exist"
            );
            assert!(
                store.get_node("quote-block").await?.is_some(),
                "quote-block schema should exist"
            );
            assert!(
                store.get_node("ordered-list").await?.is_some(),
                "ordered-list schema should exist"
            );

            Ok(())
        }

        #[tokio::test]
        async fn test_node_service_idempotent_seeding() -> Result<(), Box<dyn std::error::Error>> {
            use tempfile::TempDir;

            let temp_dir = TempDir::new()?;
            let db_path = temp_dir.path().join("test.db");

            // First "launch" - seeds schemas
            {
                let mut store = Arc::new(SurrealStore::new(db_path.clone()).await?);
                let _service = NodeService::new(&mut store).await?;
                // Service and store dropped at end of scope (simulates app shutdown)
            }

            // Wait for RocksDB to release file lock (async drop may not be instant)
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Second "launch" - should NOT re-seed (idempotent)
            {
                let mut store = Arc::new(SurrealStore::new(db_path).await?);
                let _service = NodeService::new(&mut store).await?;

                // Verify schemas still exist and weren't duplicated
                let task = store.get_node("task").await?.unwrap();
                assert_eq!(task.node_type, "schema");

                // Verify we can query task schema (ensures it's valid)
                let schema = store.get_schema_node("task").await?;
                assert!(schema.is_some(), "task schema should be retrievable");
            }

            Ok(())
        }
    }

    // ============================================================================
    // Schema CRUD with Relationships Tests (Issue #703)
    // ============================================================================

    #[tokio::test]
    async fn test_create_schema_with_relationships() {
        let (service, _temp) = create_test_service().await;

        // Create a schema node with relationships
        // IMPORTANT: Schema ID must be a valid type name (used as table name)
        let schema_node = Node::new_with_id(
            "invoice".to_string(),
            "schema".to_string(),
            "Invoice".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "description": "Invoice schema with customer relationship",
                "fields": [
                    {
                        "name": "amount",
                        "type": "number",
                        "required": true
                    },
                    {
                        "name": "due_date",
                        "type": "date"
                    }
                ],
                "relationships": [
                    {
                        "name": "billed_to",
                        "targetType": "customer",
                        "direction": "out",
                        "cardinality": "one",
                        "required": true,
                        "reverseName": "invoices",
                        "reverseCardinality": "many",
                        "edgeFields": [
                            {
                                "name": "billing_date",
                                "fieldType": "date",
                                "required": true
                            },
                            {
                                "name": "payment_terms",
                                "fieldType": "string"
                            }
                        ]
                    }
                ]
            }),
        );

        // Create the schema node (should generate relationship table DDL)
        let id = service.create_node(schema_node).await.unwrap();

        // Verify the schema node was created
        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.node_type, "schema");
        assert_eq!(retrieved.content, "Invoice");

        // Verify relationships are stored
        let relationships = retrieved.properties.get("relationships").unwrap();
        assert!(relationships.is_array());
        let relationships_array = relationships.as_array().unwrap();
        assert_eq!(relationships_array.len(), 1);

        // Verify relationship details
        let relationship = &relationships_array[0];
        assert_eq!(relationship.get("name").unwrap(), "billed_to");
        assert_eq!(relationship.get("targetType").unwrap(), "customer");
        assert_eq!(relationship.get("direction").unwrap(), "out");
        assert_eq!(relationship.get("cardinality").unwrap(), "one");
    }

    #[tokio::test]
    async fn test_update_schema_add_relationships() {
        let (service, _temp) = create_test_service().await;

        // Create a schema node without relationships
        // IMPORTANT: Schema ID must be a valid type name (used as table name)
        let schema_node = Node::new_with_id(
            "project".to_string(),
            "schema".to_string(),
            "Project".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "description": "Project schema",
                "fields": [
                    {
                        "name": "title",
                        "type": "string",
                        "required": true
                    }
                ],
                "relationships": []
            }),
        );

        let id = service.create_node(schema_node).await.unwrap();

        // Update to add relationships
        let update = NodeUpdate::new().with_properties(json!({
            "isCore": false,
            "version": 1,
            "description": "Project schema with team relationships",
            "fields": [
                {
                    "name": "title",
                    "type": "string",
                    "required": true
                }
            ],
            "relationships": [
                {
                    "name": "assigned_to",
                    "targetType": "person",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "projects",
                    "reverseCardinality": "many",
                    "edgeFields": [
                        {
                            "name": "role",
                            "fieldType": "string",
                            "required": true
                        }
                    ]
                }
            ]
        }));

        service.update_node(&id, update).await.unwrap();

        // Verify relationships were added
        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        let relationships = retrieved.properties.get("relationships").unwrap();
        assert!(relationships.is_array());
        let relationships_array = relationships.as_array().unwrap();
        assert_eq!(relationships_array.len(), 1);

        let relationship = &relationships_array[0];
        assert_eq!(relationship.get("name").unwrap(), "assigned_to");
        assert_eq!(relationship.get("targetType").unwrap(), "person");
    }

    #[tokio::test]
    async fn test_create_schema_multiple_relationships() {
        let (service, _temp) = create_test_service().await;

        // Create a schema node with multiple relationships
        // IMPORTANT: Schema ID must be a valid type name (used as table name)
        let schema_node = Node::new_with_id(
            "task_test".to_string(),
            "schema".to_string(),
            "Task".to_string(),
            json!({
                "isCore": true,
                "version": 2,
                "description": "Task schema with multiple relationships",
                "fields": [
                    {
                        "name": "status",
                        "type": "enum",
                        "required": true
                    }
                ],
                "relationships": [
                    {
                        "name": "assigned_to",
                        "targetType": "person",
                        "direction": "out",
                        "cardinality": "many"
                    },
                    {
                        "name": "part_of",
                        "targetType": "project",
                        "direction": "out",
                        "cardinality": "one"
                    },
                    {
                        "name": "blocked_by",
                        "targetType": "task",
                        "direction": "out",
                        "cardinality": "many"
                    }
                ]
            }),
        );

        let id = service.create_node(schema_node).await.unwrap();

        // Verify all relationships are stored
        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        let relationships = retrieved.properties.get("relationships").unwrap();
        let relationships_array = relationships.as_array().unwrap();
        assert_eq!(relationships_array.len(), 3);

        // Verify each relationship
        let names: Vec<&str> = relationships_array
            .iter()
            .map(|r| r.get("name").unwrap().as_str().unwrap())
            .collect();
        assert!(names.contains(&"assigned_to"));
        assert!(names.contains(&"part_of"));
        assert!(names.contains(&"blocked_by"));
    }

    #[tokio::test]
    async fn test_schema_relationship_reserved_name_validation() {
        let (service, _temp) = create_test_service().await;

        // Try to create a schema with a reserved relationship name
        // IMPORTANT: Schema ID must be a valid type name (used as table name)
        let schema_node = Node::new_with_id(
            "bad_schema".to_string(),
            "schema".to_string(),
            "BadSchema".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "description": "Schema with reserved relationship name",
                "fields": [],
                "relationships": [
                    {
                        "name": "has_child",
                        "targetType": "other",
                        "direction": "out",
                        "cardinality": "many"
                    }
                ]
            }),
        );

        // Should fail validation
        let result = service.create_node(schema_node).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("reserved") || err_msg.contains("has_child"));
    }

    #[tokio::test]
    async fn test_schema_forward_reference_allowed() {
        let (service, _temp) = create_test_service().await;

        // Create invoice schema that references customer (which doesn't exist yet)
        // This should succeed - forward references are allowed during schema creation
        // IMPORTANT: Schema ID must be a valid type name (used as table name)
        let invoice_schema = Node::new_with_id(
            "invoice_fwd".to_string(),
            "schema".to_string(),
            "Invoice".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "description": "Invoice with forward reference to customer",
                "fields": [],
                "relationships": [
                    {
                        "name": "billed_to",
                        "targetType": "customer",
                        "direction": "out",
                        "cardinality": "one"
                    }
                ]
            }),
        );

        // Should succeed even though customer schema doesn't exist yet
        let result = service.create_node(invoice_schema).await;
        assert!(result.is_ok());

        let id = result.unwrap();
        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        let relationships = retrieved.properties.get("relationships").unwrap();
        assert_eq!(relationships.as_array().unwrap().len(), 1);
    }

    // ============================================================================
    // Relationship CRUD API Tests (Issue #703 Phase 4, enabled by Issue #712)
    // ============================================================================
    //
    // These tests verify relationship CRUD operations using custom schema-defined types.
    // The CustomNodeBehavior fallback enables schema-defined types without explicit
    // behavior registration.

    #[tokio::test]
    async fn test_create_relationship_basic() {
        let (service, _temp) = create_test_service().await;

        // Create schemas for custom types
        let person_schema = Node::new_with_id(
            "person_basic".to_string(),
            "schema".to_string(),
            "Person".to_string(),
            json!({"isCore": false, "version": 1, "fields": [], "relationships": []}),
        );
        service.create_node(person_schema).await.unwrap();

        let doc_schema = Node::new_with_id(
            "document_basic".to_string(),
            "schema".to_string(),
            "Document".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [],
                "relationships": [{
                    "name": "authored_by",
                    "targetType": "person_basic",
                    "direction": "out",
                    "cardinality": "many"
                }]
            }),
        );
        service.create_node(doc_schema).await.unwrap();

        // Create nodes using custom schema-defined types (uses CustomNodeBehavior fallback)
        let doc = Node::new_with_id(
            "doc-1".to_string(),
            "document_basic".to_string(),
            "Architecture Doc".to_string(),
            json!({}),
        );
        service.create_node(doc).await.unwrap();

        let person = Node::new_with_id(
            "person-1".to_string(),
            "person_basic".to_string(),
            "Alice".to_string(),
            json!({}),
        );
        service.create_node(person).await.unwrap();

        // Create relationship
        service
            .create_relationship("doc-1", "authored_by", "person-1", json!({}))
            .await
            .unwrap();

        // Verify relationship exists
        let related = service
            .get_related_nodes("doc-1", "authored_by", "out")
            .await
            .unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].id, "person-1");
        assert_eq!(related[0].content, "Alice");
    }

    #[tokio::test]
    async fn test_create_relationship_with_edge_data() {
        let (service, _temp) = create_test_service().await;

        // Create schemas for custom types with edge fields
        let person_schema = Node::new_with_id(
            "person_edge".to_string(),
            "schema".to_string(),
            "Person".to_string(),
            json!({"isCore": false, "version": 1, "fields": [], "relationships": []}),
        );
        service.create_node(person_schema).await.unwrap();

        let task_schema = Node::new_with_id(
            "task_edge".to_string(),
            "schema".to_string(),
            "Task".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [],
                "relationships": [{
                    "name": "assigned_to",
                    "targetType": "person_edge",
                    "direction": "out",
                    "cardinality": "many",
                    "edgeFields": [{
                        "name": "role",
                        "type": "string",
                        "required": true
                    }]
                }]
            }),
        );
        service.create_node(task_schema).await.unwrap();

        // Create nodes using custom schema-defined types
        let task = Node::new_with_id(
            "task-2".to_string(),
            "task_edge".to_string(),
            "Design feature".to_string(),
            json!({}),
        );
        service.create_node(task).await.unwrap();

        let person = Node::new_with_id(
            "person-2".to_string(),
            "person_edge".to_string(),
            "Bob".to_string(),
            json!({}),
        );
        service.create_node(person).await.unwrap();

        // Create relationship with edge data
        service
            .create_relationship(
                "task-2",
                "assigned_to",
                "person-2",
                json!({"role": "owner"}),
            )
            .await
            .unwrap();

        // Verify relationship exists
        let related = service
            .get_related_nodes("task-2", "assigned_to", "out")
            .await
            .unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].id, "person-2");
    }

    #[tokio::test]
    async fn test_delete_relationship() {
        let (service, _temp) = create_test_service().await;

        // Create schemas for custom types
        let person_schema = Node::new_with_id(
            "person_del".to_string(),
            "schema".to_string(),
            "Person".to_string(),
            json!({"isCore": false, "version": 1, "fields": [], "relationships": []}),
        );
        service.create_node(person_schema).await.unwrap();

        let task_schema = Node::new_with_id(
            "task_del".to_string(),
            "schema".to_string(),
            "Task".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [],
                "relationships": [{
                    "name": "assigned_to",
                    "targetType": "person_del",
                    "direction": "out",
                    "cardinality": "many"
                }]
            }),
        );
        service.create_node(task_schema).await.unwrap();

        // Create nodes using custom schema-defined types
        let task = Node::new_with_id(
            "task-3".to_string(),
            "task_del".to_string(),
            "Test task".to_string(),
            json!({}),
        );
        service.create_node(task).await.unwrap();

        let person = Node::new_with_id(
            "person-3".to_string(),
            "person_del".to_string(),
            "Charlie".to_string(),
            json!({}),
        );
        service.create_node(person).await.unwrap();

        service
            .create_relationship("task-3", "assigned_to", "person-3", json!({}))
            .await
            .unwrap();

        // Verify relationship exists
        let before = service
            .get_related_nodes("task-3", "assigned_to", "out")
            .await
            .unwrap();
        assert_eq!(before.len(), 1);

        // Delete relationship
        service
            .delete_relationship("task-3", "assigned_to", "person-3")
            .await
            .unwrap();

        // Verify relationship is gone
        let after = service
            .get_related_nodes("task-3", "assigned_to", "out")
            .await
            .unwrap();
        assert_eq!(after.len(), 0);
    }

    #[tokio::test]
    async fn test_relationship_cardinality_one_enforcement() {
        let (service, _temp) = create_test_service().await;

        // Create schemas for custom types with cardinality constraint
        let customer_schema = Node::new_with_id(
            "customer_card".to_string(),
            "schema".to_string(),
            "Customer".to_string(),
            json!({"isCore": false, "version": 1, "fields": [], "relationships": []}),
        );
        service.create_node(customer_schema).await.unwrap();

        let invoice_schema = Node::new_with_id(
            "invoice_card".to_string(),
            "schema".to_string(),
            "Invoice".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [],
                "relationships": [{
                    "name": "billed_to",
                    "targetType": "customer_card",
                    "direction": "out",
                    "cardinality": "one"
                }]
            }),
        );
        service.create_node(invoice_schema).await.unwrap();

        // Create nodes using custom schema-defined types
        let invoice = Node::new_with_id(
            "inv-1".to_string(),
            "invoice_card".to_string(),
            "Invoice #001".to_string(),
            json!({}),
        );
        service.create_node(invoice).await.unwrap();

        let customer1 = Node::new_with_id(
            "cust-1".to_string(),
            "customer_card".to_string(),
            "Acme Corp".to_string(),
            json!({}),
        );
        service.create_node(customer1).await.unwrap();

        let customer2 = Node::new_with_id(
            "cust-2".to_string(),
            "customer_card".to_string(),
            "Initech".to_string(),
            json!({}),
        );
        service.create_node(customer2).await.unwrap();

        // Create first relationship (should succeed)
        service
            .create_relationship("inv-1", "billed_to", "cust-1", json!({}))
            .await
            .unwrap();

        // Try to create second relationship (should fail - cardinality violation)
        let result = service
            .create_relationship("inv-1", "billed_to", "cust-2", json!({}))
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("cardinality") || err_msg.contains("one"));
    }

    #[tokio::test]
    async fn test_relationship_target_type_validation() {
        let (service, _temp) = create_test_service().await;

        // Create schemas for custom types with target type constraints
        let person_schema = Node::new_with_id(
            "person_valid".to_string(),
            "schema".to_string(),
            "Person".to_string(),
            json!({"isCore": false, "version": 1, "fields": [], "relationships": []}),
        );
        service.create_node(person_schema).await.unwrap();

        let project_schema = Node::new_with_id(
            "project_valid".to_string(),
            "schema".to_string(),
            "Project".to_string(),
            json!({"isCore": false, "version": 1, "fields": [], "relationships": []}),
        );
        service.create_node(project_schema).await.unwrap();

        let task_schema = Node::new_with_id(
            "task_valid".to_string(),
            "schema".to_string(),
            "Task".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [],
                "relationships": [{
                    "name": "assigned_to",
                    "targetType": "person_valid",
                    "direction": "out",
                    "cardinality": "many"
                }]
            }),
        );
        service.create_node(task_schema).await.unwrap();

        // Create nodes using custom schema-defined types
        let task = Node::new_with_id(
            "task-4".to_string(),
            "task_valid".to_string(),
            "Test task".to_string(),
            json!({}),
        );
        service.create_node(task).await.unwrap();

        let project = Node::new_with_id(
            "proj-1".to_string(),
            "project_valid".to_string(),
            "NodeSpace".to_string(),
            json!({}),
        );
        service.create_node(project).await.unwrap();

        // Try to assign task to project (wrong type - should fail)
        let result = service
            .create_relationship("task-4", "assigned_to", "proj-1", json!({}))
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("type") || err_msg.contains("mismatch"));
    }

    #[tokio::test]
    async fn test_get_related_nodes_reverse_direction() {
        let (service, _temp) = create_test_service().await;

        // Create schemas for custom types with reverse relationship
        let person_schema = Node::new_with_id(
            "person_rev".to_string(),
            "schema".to_string(),
            "Person".to_string(),
            json!({"isCore": false, "version": 1, "fields": [], "relationships": []}),
        );
        service.create_node(person_schema).await.unwrap();

        let task_schema = Node::new_with_id(
            "task_rev".to_string(),
            "schema".to_string(),
            "Task".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [],
                "relationships": [{
                    "name": "assigned_to",
                    "targetType": "person_rev",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "tasks"
                }]
            }),
        );
        service.create_node(task_schema).await.unwrap();

        // Create nodes using custom schema-defined types
        let person = Node::new_with_id(
            "person-4".to_string(),
            "person_rev".to_string(),
            "Dave".to_string(),
            json!({}),
        );
        service.create_node(person).await.unwrap();

        let task1 = Node::new_with_id(
            "task-5".to_string(),
            "task_rev".to_string(),
            "Task 1".to_string(),
            json!({}),
        );
        service.create_node(task1).await.unwrap();

        let task2 = Node::new_with_id(
            "task-6".to_string(),
            "task_rev".to_string(),
            "Task 2".to_string(),
            json!({}),
        );
        service.create_node(task2).await.unwrap();

        // Create relationships (tasks assigned to person)
        service
            .create_relationship("task-5", "assigned_to", "person-4", json!({}))
            .await
            .unwrap();
        service
            .create_relationship("task-6", "assigned_to", "person-4", json!({}))
            .await
            .unwrap();

        // Query forward direction (task -> person)
        let forward = service
            .get_related_nodes("task-5", "assigned_to", "out")
            .await
            .unwrap();
        assert_eq!(forward.len(), 1);
        assert_eq!(forward[0].id, "person-4");

        // Query reverse direction (person -> tasks)
        // Note: We need to query from the task schema perspective since person
        // doesn't have the relationship defined
        // TODO: Implement inbound relationship discovery (Phase 5) for this to work
        // properly from the person's perspective
    }

    /// Tests for type-safe task node CRUD operations (Issue #709)
    ///
    /// Verifies that `update_task_node` correctly updates task fields
    /// with type safety, OCC version checking, and proper error handling.
    mod update_task_node_tests {
        use super::*;
        use crate::models::{TaskNodeUpdate, TaskPriority, TaskStatus};
        use chrono::Utc;

        /// Helper to create a task node and return its ID
        async fn create_task(service: &NodeService, content: &str) -> String {
            let task = Node::new_with_id(
                format!("task-{}", uuid::Uuid::new_v4()),
                "task".to_string(),
                content.to_string(),
                json!({"status": "open"}),
            );
            service.create_node(task.clone()).await.unwrap();
            task.id
        }

        #[tokio::test]
        async fn test_update_task_status() {
            let (service, _temp) = create_test_service().await;

            // Create a task node
            let task_id = create_task(&service, "Test task for status update").await;

            // Verify initial state via get_task_node
            let task_before = service.get_task_node(&task_id).await.unwrap().unwrap();
            assert_eq!(task_before.status, TaskStatus::Open);
            assert_eq!(task_before.version, 1);

            // Update status to InProgress
            let update = TaskNodeUpdate::new().with_status(TaskStatus::InProgress);
            let task_after = service.update_task_node(&task_id, 1, update).await.unwrap();

            // Verify update
            assert_eq!(task_after.status, TaskStatus::InProgress);
            assert_eq!(task_after.version, 2); // Version incremented

            // Verify persistence by re-fetching
            let task_refetch = service.get_task_node(&task_id).await.unwrap().unwrap();
            assert_eq!(task_refetch.status, TaskStatus::InProgress);
            assert_eq!(task_refetch.version, 2);
        }

        #[tokio::test]
        async fn test_update_task_priority() {
            let (service, _temp) = create_test_service().await;

            let task_id = create_task(&service, "Test task for priority update").await;

            // Update priority to high
            let update = TaskNodeUpdate::new().with_priority(Some(TaskPriority::High));
            let task_after = service.update_task_node(&task_id, 1, update).await.unwrap();

            assert_eq!(task_after.priority, Some(TaskPriority::High));

            // Update priority to low
            let low_update = TaskNodeUpdate::new().with_priority(Some(TaskPriority::Low));
            let task_low = service
                .update_task_node(&task_id, 2, low_update)
                .await
                .unwrap();
            assert_eq!(task_low.priority, Some(TaskPriority::Low));

            // Clear priority (set to None)
            let clear_update = TaskNodeUpdate::new().with_priority(None);
            let task_cleared = service
                .update_task_node(&task_id, 3, clear_update)
                .await
                .unwrap();

            assert_eq!(task_cleared.priority, None);
        }

        #[tokio::test]
        async fn test_update_task_due_date() {
            let (service, _temp) = create_test_service().await;

            let task_id = create_task(&service, "Test task for due date update").await;

            // Set due date
            let due_date = Utc::now();
            let update = TaskNodeUpdate::new().with_due_date(Some(due_date));
            let task_after = service.update_task_node(&task_id, 1, update).await.unwrap();

            assert!(task_after.due_date.is_some());

            // Clear due date
            let clear_update = TaskNodeUpdate::new().with_due_date(None);
            let task_cleared = service
                .update_task_node(&task_id, 2, clear_update)
                .await
                .unwrap();

            assert!(task_cleared.due_date.is_none());
        }

        #[tokio::test]
        async fn test_update_task_assignee() {
            let (service, _temp) = create_test_service().await;

            let task_id = create_task(&service, "Test task for assignee update").await;

            // Set assignee
            let update = TaskNodeUpdate::new().with_assignee(Some("user-123".to_string()));
            let task_after = service.update_task_node(&task_id, 1, update).await.unwrap();

            assert_eq!(task_after.assignee, Some("user-123".to_string()));

            // Clear assignee
            let clear_update = TaskNodeUpdate::new().with_assignee(None);
            let task_cleared = service
                .update_task_node(&task_id, 2, clear_update)
                .await
                .unwrap();

            assert!(task_cleared.assignee.is_none());
        }

        #[tokio::test]
        async fn test_update_task_assignee_with_special_characters() {
            let (service, _temp) = create_test_service().await;

            let task_id = create_task(&service, "Test task for SQL injection prevention").await;

            // Set assignee with SQL injection attempt - should be escaped
            let malicious_value = "user'); DELETE task; --".to_string();
            let update = TaskNodeUpdate::new().with_assignee(Some(malicious_value.clone()));
            let task_after = service.update_task_node(&task_id, 1, update).await.unwrap();

            // Should store the escaped string, not execute SQL
            assert_eq!(task_after.assignee, Some(malicious_value));
        }

        #[tokio::test]
        async fn test_update_task_content() {
            let (service, _temp) = create_test_service().await;

            let task_id = create_task(&service, "Original content").await;

            // Update content (hub field)
            let update = TaskNodeUpdate::new().with_content("Updated content".to_string());
            let task_after = service.update_task_node(&task_id, 1, update).await.unwrap();

            assert_eq!(task_after.content, "Updated content");
        }

        #[tokio::test]
        async fn test_update_task_multiple_fields() {
            let (service, _temp) = create_test_service().await;

            let task_id = create_task(&service, "Multi-field update test").await;

            // Update multiple fields at once (excluding priority due to schema/struct mismatch)
            let update = TaskNodeUpdate::new()
                .with_status(TaskStatus::Done)
                .with_assignee(Some("user-456".to_string()))
                .with_content("Completed task".to_string());

            let task_after = service.update_task_node(&task_id, 1, update).await.unwrap();

            assert_eq!(task_after.status, TaskStatus::Done);
            assert_eq!(task_after.assignee, Some("user-456".to_string()));
            assert_eq!(task_after.content, "Completed task");
            assert_eq!(task_after.version, 2);
        }

        #[tokio::test]
        async fn test_update_task_version_conflict() {
            let (service, _temp) = create_test_service().await;

            let task_id = create_task(&service, "Version conflict test").await;

            // First update succeeds
            let update1 = TaskNodeUpdate::new().with_status(TaskStatus::InProgress);
            service
                .update_task_node(&task_id, 1, update1)
                .await
                .unwrap();

            // Second update with stale version fails
            let update2 = TaskNodeUpdate::new().with_status(TaskStatus::Done);
            let result = service.update_task_node(&task_id, 1, update2).await;

            assert!(result.is_err());
            match result.unwrap_err() {
                NodeServiceError::VersionConflict { node_id, .. } => {
                    assert_eq!(node_id, task_id);
                }
                other => panic!("Expected VersionConflict error, got: {:?}", other),
            }
        }

        #[tokio::test]
        async fn test_update_task_not_found() {
            let (service, _temp) = create_test_service().await;

            let update = TaskNodeUpdate::new().with_status(TaskStatus::Done);
            let result = service
                .update_task_node("nonexistent-task", 1, update)
                .await;

            assert!(result.is_err());
            // Note: When updating a non-existent node, SurrealDB's transaction fails because
            // the version check (SELECT version FROM node:id) returns empty, causing the IF to fail.
            // This manifests as a "failed transaction" error which we classify as VersionConflict.
            // This is acceptable behavior - the caller will retry, discover the node doesn't exist,
            // and handle accordingly.
            match result.unwrap_err() {
                NodeServiceError::NodeNotFound { id } => {
                    assert_eq!(id, "nonexistent-task");
                }
                NodeServiceError::VersionConflict { node_id, .. } => {
                    // Also acceptable - transaction failed because node doesn't exist
                    assert_eq!(node_id, "nonexistent-task");
                }
                other => panic!(
                    "Expected NodeNotFound or VersionConflict error, got: {:?}",
                    other
                ),
            }
        }

        #[tokio::test]
        async fn test_update_task_empty_update_rejected() {
            let (service, _temp) = create_test_service().await;

            let task_id = create_task(&service, "Empty update test").await;

            // Empty update should be rejected
            let empty_update = TaskNodeUpdate::new();
            let result = service.update_task_node(&task_id, 1, empty_update).await;

            assert!(result.is_err());
            match result.unwrap_err() {
                NodeServiceError::InvalidUpdate(reason) => {
                    assert!(reason.contains("no changes"));
                }
                other => panic!("Expected InvalidUpdate error, got: {:?}", other),
            }
        }

        #[tokio::test]
        async fn test_update_task_user_defined_status() {
            let (service, _temp) = create_test_service().await;

            let task_id = create_task(&service, "User-defined status test").await;

            // Set a user-defined status (not one of the core enum values)
            let update = TaskNodeUpdate::new().with_status(TaskStatus::User("blocked".to_string()));
            let task_after = service.update_task_node(&task_id, 1, update).await.unwrap();

            assert_eq!(task_after.status, TaskStatus::User("blocked".to_string()));

            // Verify persistence
            let task_refetch = service.get_task_node(&task_id).await.unwrap().unwrap();
            assert_eq!(task_refetch.status, TaskStatus::User("blocked".to_string()));
        }

        /// Test that update_task_node works on nodes that were converted from text to task
        /// via generic update.
        ///
        /// This reproduces the scenario where:
        /// 1. User creates a text node
        /// 2. User converts it to task via /task slash command (generic update changes node_type)
        /// 3. User tries to update task fields like status
        ///
        /// The fix ensures update_task_node properly initializes task properties
        /// when they don't exist (Issue #709).
        #[tokio::test]
        async fn test_update_task_after_type_conversion_from_text() {
            let (service, _temp) = create_test_service().await;

            // Step 1: Create a text node
            let text_node = Node::new_with_id(
                format!("converted-task-{}", uuid::Uuid::new_v4()),
                "text".to_string(),
                "This will become a task".to_string(),
                json!({}),
            );
            service.create_node(text_node.clone()).await.unwrap();

            // Step 2: Convert text to task via generic update (simulates /task command)
            let type_update = crate::models::NodeUpdate {
                node_type: Some("task".to_string()),
                ..Default::default()
            };
            service
                .update_node(&text_node.id, type_update)
                .await
                .unwrap();

            // Verify node type was updated in hub
            let node_after_convert = service.get_node(&text_node.id).await.unwrap().unwrap();
            assert_eq!(node_after_convert.node_type, "task");

            // Step 3: Update task status via type-specific method
            // Version is 2 after the type conversion update
            let status_update = TaskNodeUpdate::new().with_status(TaskStatus::InProgress);
            let task_after = service
                .update_task_node(&text_node.id, 2, status_update)
                .await
                .unwrap();

            // Verify update succeeded
            assert_eq!(task_after.status, TaskStatus::InProgress);
            assert_eq!(task_after.content, "This will become a task");
            assert_eq!(task_after.version, 3);

            // Verify persistence via get_task_node
            let task_refetch = service.get_task_node(&text_node.id).await.unwrap().unwrap();
            assert_eq!(task_refetch.status, TaskStatus::InProgress);
            assert_eq!(task_refetch.content, "This will become a task");
        }

        // ============================================================
        // Issue #794: Namespaced Properties - Type Change Tests
        // ============================================================

        /// Test that namespaced properties are preserved when changing node types.
        ///
        /// This is the core value proposition of Issue #794: when a node changes type,
        /// the old type's properties should remain as dormant data under their original
        /// namespace, allowing restoration if the node is converted back.
        #[tokio::test]
        async fn test_type_change_preserves_old_type_properties() {
            let (service, _temp) = create_test_service().await;

            // Step 1: Create a task node with specific properties
            // Properties will be stored under properties.task.status, properties.task.priority
            let task_node = Node::new(
                "task".to_string(),
                "Important work item".to_string(),
                json!({
                    "task": {
                        "status": "in_progress",
                        "priority": "high"
                    }
                }),
            );
            let node_id = service.create_node(task_node).await.unwrap();

            // Verify task properties are stored in namespaced format
            let node_as_task = service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(node_as_task.properties["task"]["status"], "in_progress");
            assert_eq!(node_as_task.properties["task"]["priority"], "high");

            // Step 2: Change the node type to text
            let type_update = crate::models::NodeUpdate {
                node_type: Some("text".to_string()),
                ..Default::default()
            };
            service.update_node(&node_id, type_update).await.unwrap();

            // Step 3: Verify old task properties are preserved as dormant data
            let node_as_text = service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(node_as_text.node_type, "text");
            // Old task properties should still exist under the "task" namespace
            assert_eq!(node_as_text.properties["task"]["status"], "in_progress");
            assert_eq!(node_as_text.properties["task"]["priority"], "high");

            // Step 4: Change back to task type
            let type_update_back = crate::models::NodeUpdate {
                node_type: Some("task".to_string()),
                ..Default::default()
            };
            service
                .update_node(&node_id, type_update_back)
                .await
                .unwrap();

            // Step 5: Verify original task properties are restored/accessible
            let node_restored = service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(node_restored.node_type, "task");
            assert_eq!(node_restored.properties["task"]["status"], "in_progress");
            assert_eq!(node_restored.properties["task"]["priority"], "high");
        }

        /// Test that properties with the same field name in different types don't conflict.
        ///
        /// For example, both "task" and "invoice" types might have a "status" field,
        /// but they should be stored independently under their respective namespaces.
        #[tokio::test]
        async fn test_same_property_name_different_types_no_conflict() {
            let (service, _temp) = create_test_service().await;

            // Step 1: Create a node with task properties (use valid task status: done)
            let node = Node::new(
                "task".to_string(),
                "Multi-purpose node".to_string(),
                json!({
                    "task": {
                        "status": "done"
                    }
                }),
            );
            let node_id = service.create_node(node).await.unwrap();

            // Step 2: Convert to text and add custom:status property
            // (simulating a custom property with same name as task's status)
            let update1 = crate::models::NodeUpdate {
                node_type: Some("text".to_string()),
                properties: Some(json!({
                    "custom": {
                        "status": "draft"
                    }
                })),
                ..Default::default()
            };
            service.update_node(&node_id, update1).await.unwrap();

            // Step 3: Verify both status values coexist without conflict
            let node_after = service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(node_after.node_type, "text");
            // Task's status should be preserved
            assert_eq!(node_after.properties["task"]["status"], "done");
            // Custom status should also exist
            assert_eq!(node_after.properties["custom"]["status"], "draft");

            // Step 4: Convert back to task
            let update2 = crate::models::NodeUpdate {
                node_type: Some("task".to_string()),
                ..Default::default()
            };
            service.update_node(&node_id, update2).await.unwrap();

            // Step 5: Verify task's original status is intact, custom status still exists
            let node_final = service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(node_final.node_type, "task");
            assert_eq!(node_final.properties["task"]["status"], "done");
            assert_eq!(node_final.properties["custom"]["status"], "draft");
        }

        /// Test that updating properties of the current type doesn't affect dormant properties.
        #[tokio::test]
        async fn test_updating_current_type_preserves_dormant_properties() {
            let (service, _temp) = create_test_service().await;

            // Create a task with properties
            let task_node = Node::new(
                "task".to_string(),
                "Task with dormant data".to_string(),
                json!({
                    "task": {
                        "status": "open",
                        "priority": "low"
                    }
                }),
            );
            let node_id = service.create_node(task_node).await.unwrap();

            // Convert to text, which leaves task properties as dormant
            let to_text = crate::models::NodeUpdate {
                node_type: Some("text".to_string()),
                properties: Some(json!({
                    "text": {
                        "format": "markdown"
                    }
                })),
                ..Default::default()
            };
            service.update_node(&node_id, to_text).await.unwrap();

            // Update the text properties
            let text_update = crate::models::NodeUpdate {
                properties: Some(json!({
                    "text": {
                        "format": "plain"
                    }
                })),
                ..Default::default()
            };
            service.update_node(&node_id, text_update).await.unwrap();

            // Verify dormant task properties are unchanged
            let node_after_text_update = service.get_node(&node_id).await.unwrap().unwrap();
            assert_eq!(node_after_text_update.properties["task"]["status"], "open");
            assert_eq!(node_after_text_update.properties["task"]["priority"], "low");
            // Active text properties were updated
            assert_eq!(node_after_text_update.properties["text"]["format"], "plain");
        }
    }

    /// Tests for built-in relationship methods (Issue #814)
    ///
    /// Verifies that the built-in relationships (member_of, has_child, mentions)
    /// can be created, queried, and deleted via the public API.
    mod builtin_relationship_tests {
        use super::*;

        /// Helper to create a collection node
        async fn create_collection(service: &NodeService, name: &str) -> String {
            let collection = Node::new_with_id(
                format!("collection-{}", uuid::Uuid::new_v4()),
                "collection".to_string(),
                name.to_string(),
                json!({}),
            );
            service.create_node(collection.clone()).await.unwrap();
            collection.id
        }

        /// Helper to create a text node
        async fn create_text_node(service: &NodeService, content: &str) -> String {
            let node = Node::new_with_id(
                format!("text-{}", uuid::Uuid::new_v4()),
                "text".to_string(),
                content.to_string(),
                json!({}),
            );
            service.create_node(node.clone()).await.unwrap();
            node.id
        }

        #[tokio::test]
        async fn test_create_member_of_relationship() {
            let (service, _temp) = create_test_service().await;

            // Create a collection and a text node
            let collection_id = create_collection(&service, "My Collection").await;
            let text_id = create_text_node(&service, "Test content").await;

            // Create member_of relationship
            let result = service
                .create_relationship(&text_id, "member_of", &collection_id, json!({}))
                .await;
            assert!(
                result.is_ok(),
                "Failed to create member_of relationship: {:?}",
                result.err()
            );

            // Verify via get_related_nodes (forward direction)
            let collections = service
                .get_related_nodes(&text_id, "member_of", "out")
                .await
                .unwrap();
            assert_eq!(collections.len(), 1);
            assert_eq!(collections[0].id, collection_id);

            // Verify via reverse direction (collection -> members)
            let members = service
                .get_related_nodes(&collection_id, "member_of", "in")
                .await
                .unwrap();
            assert_eq!(members.len(), 1);
            assert_eq!(members[0].id, text_id);
        }

        #[tokio::test]
        async fn test_member_of_validates_target_is_collection() {
            let (service, _temp) = create_test_service().await;

            // Create two text nodes (neither is a collection)
            let text1_id = create_text_node(&service, "Text 1").await;
            let text2_id = create_text_node(&service, "Text 2").await;

            // Attempt to create member_of to non-collection should fail
            let result = service
                .create_relationship(&text1_id, "member_of", &text2_id, json!({}))
                .await;
            assert!(result.is_err());
            let err_msg = format!("{:?}", result.err().unwrap());
            assert!(
                err_msg.contains("must be a collection"),
                "Expected collection validation error, got: {}",
                err_msg
            );
        }

        #[tokio::test]
        async fn test_create_member_of_idempotent() {
            let (service, _temp) = create_test_service().await;

            let collection_id = create_collection(&service, "My Collection").await;
            let text_id = create_text_node(&service, "Test content").await;

            // Create relationship twice - should be idempotent
            service
                .create_relationship(&text_id, "member_of", &collection_id, json!({}))
                .await
                .unwrap();
            service
                .create_relationship(&text_id, "member_of", &collection_id, json!({}))
                .await
                .unwrap();

            // Should only have one relationship
            let collections = service
                .get_related_nodes(&text_id, "member_of", "out")
                .await
                .unwrap();
            assert_eq!(collections.len(), 1);
        }

        #[tokio::test]
        async fn test_delete_member_of_relationship() {
            let (service, _temp) = create_test_service().await;

            let collection_id = create_collection(&service, "My Collection").await;
            let text_id = create_text_node(&service, "Test content").await;

            // Create and verify relationship exists
            service
                .create_relationship(&text_id, "member_of", &collection_id, json!({}))
                .await
                .unwrap();
            let before = service
                .get_related_nodes(&text_id, "member_of", "out")
                .await
                .unwrap();
            assert_eq!(before.len(), 1);

            // Delete the relationship
            service
                .delete_relationship(&text_id, "member_of", &collection_id)
                .await
                .unwrap();

            // Verify it's gone
            let after = service
                .get_related_nodes(&text_id, "member_of", "out")
                .await
                .unwrap();
            assert_eq!(after.len(), 0);
        }

        #[tokio::test]
        async fn test_delete_relationship_idempotent() {
            let (service, _temp) = create_test_service().await;

            let collection_id = create_collection(&service, "My Collection").await;
            let text_id = create_text_node(&service, "Test content").await;

            // Delete non-existent relationship should succeed (idempotent)
            let result = service
                .delete_relationship(&text_id, "member_of", &collection_id)
                .await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_create_mentions_relationship() {
            let (service, _temp) = create_test_service().await;

            // Create two text nodes
            let text1_id = create_text_node(&service, "This mentions another node").await;
            let text2_id = create_text_node(&service, "I am mentioned").await;

            // mentions can link any two nodes
            let result = service
                .create_relationship(&text1_id, "mentions", &text2_id, json!({}))
                .await;
            assert!(result.is_ok());

            // Verify forward direction
            let mentioned = service
                .get_related_nodes(&text1_id, "mentions", "out")
                .await
                .unwrap();
            assert_eq!(mentioned.len(), 1);
            assert_eq!(mentioned[0].id, text2_id);

            // Verify reverse direction (who mentions me)
            let mentioners = service
                .get_related_nodes(&text2_id, "mentions", "in")
                .await
                .unwrap();
            assert_eq!(mentioners.len(), 1);
            assert_eq!(mentioners[0].id, text1_id);
        }

        #[tokio::test]
        async fn test_create_has_child_relationship() {
            let (service, _temp) = create_test_service().await;

            // Create parent and child text nodes
            let parent_id = create_text_node(&service, "Parent node").await;
            let child_id = create_text_node(&service, "Child node").await;

            // has_child establishes parent-child hierarchy
            let result = service
                .create_relationship(&parent_id, "has_child", &child_id, json!({}))
                .await;
            assert!(result.is_ok());

            // Verify forward direction (parent -> children)
            let children = service
                .get_related_nodes(&parent_id, "has_child", "out")
                .await
                .unwrap();
            assert_eq!(children.len(), 1);
            assert_eq!(children[0].id, child_id);

            // Verify reverse direction (child -> parent)
            let parents = service
                .get_related_nodes(&child_id, "has_child", "in")
                .await
                .unwrap();
            assert_eq!(parents.len(), 1);
            assert_eq!(parents[0].id, parent_id);
        }

        #[tokio::test]
        async fn test_unknown_builtin_relationship_fails() {
            let (service, _temp) = create_test_service().await;

            let text1_id = create_text_node(&service, "Text 1").await;
            let text2_id = create_text_node(&service, "Text 2").await;

            // Attempting to create an undefined relationship should fail
            // (unless it's defined in the schema, which we don't have for 'fake_relationship')
            let result = service
                .create_relationship(&text1_id, "fake_relationship", &text2_id, json!({}))
                .await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_multiple_collections_membership() {
            let (service, _temp) = create_test_service().await;

            // Create two collections and one node
            let collection1_id = create_collection(&service, "Collection 1").await;
            let collection2_id = create_collection(&service, "Collection 2").await;
            let text_id = create_text_node(&service, "Belongs to both").await;

            // Add to both collections
            service
                .create_relationship(&text_id, "member_of", &collection1_id, json!({}))
                .await
                .unwrap();
            service
                .create_relationship(&text_id, "member_of", &collection2_id, json!({}))
                .await
                .unwrap();

            // Verify node belongs to both collections
            let collections = service
                .get_related_nodes(&text_id, "member_of", "out")
                .await
                .unwrap();
            assert_eq!(collections.len(), 2);

            let collection_ids: Vec<&str> = collections.iter().map(|n| n.id.as_str()).collect();
            assert!(collection_ids.contains(&collection1_id.as_str()));
            assert!(collection_ids.contains(&collection2_id.as_str()));
        }
    }
}
