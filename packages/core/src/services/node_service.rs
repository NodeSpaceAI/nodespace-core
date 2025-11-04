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
//! # Container Node Detection
//!
//! Container nodes (topics, date nodes, etc.) are the primary targets for semantic search.
//! They are identified by `container_node_id IS NULL` in the database.
//!
//! **CRITICAL:** Never use `node_type == 'topic'` for container detection.
//! The node_type field indicates the node's behavior, not its container status.
//!
//! Examples:
//! - Container node: `container_node_id = NULL` (e.g., @mention pages, date nodes)
//! - Child node: `container_node_id = Some("parent-id")` (e.g., notes within a topic)

use crate::behaviors::NodeBehaviorRegistry;
use crate::db::DatabaseService;
use crate::models::{Node, NodeFilter, NodeUpdate, OrderBy};
use crate::services::error::NodeServiceError;
use crate::services::migration_registry::MigrationRegistry;
use crate::services::migrations;
use chrono::{DateTime, NaiveDateTime, Utc};
use regex::Regex;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

/// Special container node ID that represents "no container" (root-level nodes)
///
/// The frontend uses "root" as a sentinel value to indicate nodes at the root level.
/// The backend converts this to NULL in the database for proper relational semantics.
const ROOT_CONTAINER_ID: &str = "root";

/// Check if a string matches date node format: YYYY-MM-DD
///
/// Valid examples: "2025-10-13", "2024-01-01"
/// Invalid examples: "abcd-ef-gh", "2025-10-1", "25-10-13"
fn is_date_node_id(id: &str) -> bool {
    // Must be exactly 10 characters: YYYY-MM-DD
    if id.len() != 10 {
        return false;
    }

    // Check format: 4 digits, dash, 2 digits, dash, 2 digits
    let bytes = id.as_bytes();
    bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
        && bytes[4] == b'-'
        && bytes[5].is_ascii_digit()
        && bytes[6].is_ascii_digit()
        && bytes[7] == b'-'
        && bytes[8].is_ascii_digit()
        && bytes[9].is_ascii_digit()
}

/// Check if a node is a container node based on its container_node_id
///
/// Container nodes are identified by having a NULL container_node_id in the database.
/// This is the ONLY correct way to detect container nodes.
///
/// # Arguments
///
/// * `container_node_id` - The container_node_id field from a Node
///
/// # Returns
///
/// `true` if the node is a container (container_node_id is None), `false` otherwise
///
/// # Examples
///
/// ```
/// # use nodespace_core::services::node_service::is_container_node;
/// assert!(is_container_node(&None)); // Container node
/// assert!(!is_container_node(&Some("parent-id".to_string()))); // Child node
/// ```
pub fn is_container_node(container_node_id: &Option<String>) -> bool {
    container_node_id.is_none()
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

/// Parse timestamp from database - handles both SQLite and RFC3339 formats
fn parse_timestamp(s: &str) -> Result<DateTime<Utc>, String> {
    // Try SQLite format first: "YYYY-MM-DD HH:MM:SS"
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(naive.and_utc());
    }

    // Try RFC3339 format (for old data): "YYYY-MM-DDTHH:MM:SSZ"
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    Err(format!(
        "Unable to parse timestamp '{}' as SQLite or RFC3339 format",
        s
    ))
}

/// Core service for node CRUD and hierarchy operations
///
/// # Examples
///
/// ```no_run
/// use nodespace_core::services::NodeService;
/// use nodespace_core::db::DatabaseService;
/// use nodespace_core::models::Node;
/// use std::path::PathBuf;
/// use serde_json::json;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let db = DatabaseService::new(PathBuf::from("./data/test.db")).await?;
///     let service = NodeService::new(db)?;
///
///     let node = Node::new(
///         "text".to_string(),
///         "Hello World".to_string(),
///         None,
///         json!({}),
///     );
///
///     let id = service.create_node(node).await?;
///     println!("Created node: {}", id);
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct NodeService {
    /// Database service for persistence
    db: Arc<DatabaseService>,

    /// Behavior registry for validation
    behaviors: Arc<NodeBehaviorRegistry>,

    /// Migration registry for lazy schema upgrades
    migration_registry: Arc<MigrationRegistry>,
}

impl NodeService {
    /// Create a new NodeService
    ///
    /// Initializes the service with a DatabaseService and creates a default
    /// NodeBehaviorRegistry with Text, Task, and Date behaviors.
    ///
    /// # Arguments
    ///
    /// * `db` - DatabaseService instance
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = DatabaseService::new(PathBuf::from("./data/test.db")).await?;
    /// let service = NodeService::new(db)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(db: DatabaseService) -> Result<Self, NodeServiceError> {
        // Create and initialize migration registry with all registered migrations
        let mut migration_registry = MigrationRegistry::new();
        migrations::task::register_migrations(&mut migration_registry);

        Ok(Self {
            db: Arc::new(db),
            behaviors: Arc::new(NodeBehaviorRegistry::new()),
            migration_registry: Arc::new(migration_registry),
        })
    }

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
    /// - Root node doesn't exist (if container_node_id is set)
    /// - Database insertion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::Node;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// let node = Node::new(
    ///     "text".to_string(),
    ///     "My note".to_string(),
    ///     None,
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
    /// # Arguments
    ///
    /// * `node_type` - The type of node to get the schema for (e.g., "task", "person")
    ///
    /// # Returns
    ///
    /// * `Ok(Some(value))` - Schema definition as JSON value if found
    /// * `Ok(None)` - No schema found for this node type
    /// * `Err` - Database error
    async fn get_schema_for_type(
        &self,
        node_type: &str,
    ) -> Result<Option<serde_json::Value>, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await?;

        let mut stmt = conn
            .prepare("SELECT properties FROM nodes WHERE id = ? AND node_type = 'schema'")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare schema query: {}", e))
            })?;

        let mut rows = stmt
            .query([node_type])
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to get schema: {}", e)))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let properties_str: String = row.get(0).map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get properties column: {}", e))
            })?;

            let schema: serde_json::Value = serde_json::from_str(&properties_str)
                .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

            Ok(Some(schema))
        } else {
            Ok(None)
        }
    }

    /// Backfill _schema_version for a node if it doesn't have one (Phase 1 lazy migration)
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
        if let Some(props_obj) = node.properties.as_object() {
            if !props_obj.contains_key("_schema_version") {
                // Node doesn't have schema version - backfill it
                let version =
                    if let Some(schema) = self.get_schema_for_type(&node.node_type).await? {
                        schema.get("version").and_then(|v| v.as_i64()).unwrap_or(1)
                    } else {
                        1 // Default to version 1 if no schema found
                    };

                // Add version to node properties
                let mut updated_props = node.properties.clone();
                if let Some(props_obj) = updated_props.as_object_mut() {
                    props_obj.insert("_schema_version".to_string(), serde_json::json!(version));
                }

                // Persist the backfilled version to database
                let conn = self.db.connect_with_timeout().await?;
                let properties_json = serde_json::to_string(&updated_props)
                    .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

                conn.execute(
                    "UPDATE nodes SET properties = ?, modified_at = CURRENT_TIMESTAMP WHERE id = ?",
                    (properties_json.as_str(), node.id.as_str()),
                )
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to backfill schema version: {}",
                        e
                    ))
                })?;

                // Update the in-memory node with new properties
                node.properties = updated_props;
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
        // Get current version from node
        let current_version = node
            .properties
            .get("_schema_version")
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

        // Persist migrated node to database
        let conn = self.db.connect_with_timeout().await?;
        let properties_json = serde_json::to_string(&migrated_node.properties)
            .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

        conn.execute(
            "UPDATE nodes SET properties = ?, modified_at = CURRENT_TIMESTAMP WHERE id = ?",
            (properties_json.as_str(), node.id.as_str()),
        )
        .await
        .map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to persist migrated node: {}", e))
        })?;

        // Update the in-memory node
        *node = migrated_node;

        Ok(())
    }

    pub async fn create_node(&self, mut node: Node) -> Result<String, NodeServiceError> {
        // INVARIANT: Date nodes MUST have node_type="date" (enforced by validation).
        // Auto-detect date nodes by ID format (YYYY-MM-DD) to prevent validation failures
        // from incorrect client input. This maintains data integrity regardless of caller mistakes.
        if is_date_node_id(&node.id) {
            node.node_type = "date".to_string();
        }

        // Validate node using behavior registry
        self.behaviors.validate_node(&node)?;

        // Validate parent exists if parent_id is set
        // Auto-create date nodes if they don't exist
        if let Some(ref parent_id) = node.parent_id {
            let parent_exists = self.node_exists(parent_id).await?;
            if !parent_exists {
                // Check if this is a date node (format: YYYY-MM-DD)
                if is_date_node_id(parent_id) {
                    // Auto-create the date node with schema version
                    let mut date_properties = serde_json::Map::new();

                    // Add schema version for date node
                    if let Some(schema) = self.get_schema_for_type("date").await? {
                        if let Some(version) = schema.get("version").and_then(|v| v.as_i64()) {
                            date_properties
                                .insert("_schema_version".to_string(), serde_json::json!(version));
                        }
                    } else {
                        // No schema found - use version 1 as default
                        date_properties.insert("_schema_version".to_string(), serde_json::json!(1));
                    }

                    let date_node = Node {
                        id: parent_id.clone(),
                        node_type: "date".to_string(),
                        content: String::new(),
                        parent_id: None,
                        container_node_id: None,
                        before_sibling_id: None,
                        version: 1,
                        properties: serde_json::Value::Object(date_properties),
                        mentions: vec![],
                        mentioned_by: vec![],
                        created_at: chrono::Utc::now(),
                        modified_at: chrono::Utc::now(),
                        embedding_vector: None,
                    };

                    // Insert date node directly (skip validation to avoid recursion)
                    let conn = self.db.connect_with_timeout().await?;
                    let properties_json = serde_json::to_string(&date_node.properties)
                        .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;
                    conn.execute(
                        "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, version, properties, embedding_vector)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        (
                            date_node.id.as_str(),
                            date_node.node_type.as_str(),
                            date_node.content.as_str(),
                            date_node.parent_id.as_deref(),
                            date_node.container_node_id.as_deref(),
                            date_node.before_sibling_id.as_deref(),
                            date_node.version,
                            properties_json,
                            date_node.embedding_vector.as_deref(),
                        ),
                    )
                    .await
                    .map_err(|e| NodeServiceError::query_failed(format!("Failed to auto-create date node: {}", e)))?;
                } else {
                    return Err(NodeServiceError::invalid_parent(parent_id));
                }
            }
        }

        // Validate root exists if container_node_id is set
        // Special case: ROOT_CONTAINER_ID is treated as null (no container node)
        if let Some(ref container_node_id) = node.container_node_id {
            if container_node_id != ROOT_CONTAINER_ID {
                let root_exists = self.node_exists(container_node_id).await?;
                if !root_exists {
                    return Err(NodeServiceError::invalid_root(container_node_id));
                }
            }
        }

        // Insert into database
        // Database defaults handle created_at and modified_at timestamps automatically
        let conn = self.db.connect_with_timeout().await?;

        // Add schema version to properties
        // Get schema for this node type and extract version
        let mut properties = node.properties.clone();
        if let Some(schema) = self.get_schema_for_type(&node.node_type).await? {
            if let Some(version) = schema.get("version").and_then(|v| v.as_i64()) {
                // Add _schema_version to properties
                if let Some(props_obj) = properties.as_object_mut() {
                    props_obj.insert("_schema_version".to_string(), serde_json::json!(version));
                }
            }
        } else {
            // No schema found - use version 1 as default
            if let Some(props_obj) = properties.as_object_mut() {
                props_obj.insert("_schema_version".to_string(), serde_json::json!(1));
            }
        }

        let properties_json = serde_json::to_string(&properties)
            .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

        // Convert ROOT_CONTAINER_ID to None (null in database)
        let container_node_id_value = node
            .container_node_id
            .as_deref()
            .filter(|id| *id != ROOT_CONTAINER_ID);

        // Mark container nodes as stale for initial embedding generation
        // Container nodes are identified by container_node_id being NULL
        // This ensures new containers will be picked up by the background embedding processor
        let is_container = container_node_id_value.is_none();

        if is_container {
            conn.execute(
                "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector, embedding_stale, last_content_update)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, TRUE, CURRENT_TIMESTAMP)",
                (
                    node.id.as_str(),
                    node.node_type.as_str(),
                    node.content.as_str(),
                    node.parent_id.as_deref(),
                    container_node_id_value,
                    node.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    node.embedding_vector.as_deref(),
                ),
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to insert node: {}", e)))?;
        } else {
            conn.execute(
                "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    node.id.as_str(),
                    node.node_type.as_str(),
                    node.content.as_str(),
                    node.parent_id.as_deref(),
                    container_node_id_value,
                    node.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    node.embedding_vector.as_deref(),
                ),
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to insert node: {}", e)))?;
        }

        Ok(node.id)
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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

        // Prevent container-level self-references (child mentioning its own container)
        let mentioning_node = self
            .get_node(mentioning_node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(mentioning_node_id))?;

        // If the mentioning node has a container, prevent it from mentioning that container
        if let Some(container_id) = &mentioning_node.container_node_id {
            if container_id == mentioned_node_id {
                return Err(NodeServiceError::ValidationFailed(
                    crate::models::ValidationError::InvalidParent(
                        "Cannot mention own container (container-level self-reference)".to_string(),
                    ),
                ));
            }
        }

        let conn = self.db.connect_with_timeout().await?;

        conn.execute(
            "INSERT OR IGNORE INTO node_mentions (node_id, mentions_node_id)
             VALUES (?, ?)",
            (mentioning_node_id, mentioned_node_id),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to create mention: {}", e)))?;

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// service.delete_mention("daily-note-id", "project-planning-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_mention(
        &self,
        mentioning_node_id: &str,
        mentioned_node_id: &str,
    ) -> Result<(), NodeServiceError> {
        let conn = self.db.connect_with_timeout().await?;

        conn.execute(
            "DELETE FROM node_mentions WHERE node_id = ? AND mentions_node_id = ?",
            (mentioning_node_id, mentioned_node_id),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to delete mention: {}", e)))?;

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// if let Some(node) = service.get_node("node-id-123").await? {
    ///     println!("Found: {}", node.content);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_node(&self, id: &str) -> Result<Option<Node>, NodeServiceError> {
        let conn = self.db.connect()?;

        let mut stmt = conn
            .prepare(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version,
                        created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE id = ?",
            )
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

        let mut rows = stmt.query([id]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let mut node = self.row_to_node(row)?;
            self.populate_mentions(&mut node).await?;
            self.backfill_schema_version(&mut node).await?;
            self.apply_lazy_migration(&mut node).await?;
            Ok(Some(node))
        } else {
            Ok(None)
        }
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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

        if let Some(node_type) = update.node_type {
            updated.node_type = node_type;
        }

        if let Some(content) = update.content {
            if updated.content != content {
                content_changed = true;
            }
            updated.content = content;
        }

        if let Some(parent_id) = update.parent_id {
            updated.parent_id = parent_id;
        }

        if let Some(ref container_node_id) = update.container_node_id {
            // Convert ROOT_CONTAINER_ID to None (null in database) - same as CREATE operation
            updated.container_node_id = match container_node_id {
                Some(id) if id == ROOT_CONTAINER_ID => None,
                other => other.clone(),
            };
        }

        if let Some(before_sibling_id) = update.before_sibling_id {
            updated.before_sibling_id = before_sibling_id;
        }

        if let Some(properties) = update.properties {
            updated.properties = properties;
        }

        if let Some(embedding_vector) = update.embedding_vector {
            updated.embedding_vector = embedding_vector;
        }

        // Validate updated node
        self.behaviors.validate_node(&updated)?;

        // Execute update - database will auto-update modified_at via trigger or default
        let conn = self.db.connect_with_timeout().await?;
        let properties_json = serde_json::to_string(&updated.properties)
            .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

        // Determine if this node is a container (container_node_id IS NULL)
        let is_container = updated.container_node_id.is_none();

        // If content changed and this is a container node, mark as stale
        if content_changed && is_container {
            conn.execute(
                "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ?, embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                (
                    updated.node_type.as_str(),
                    updated.content.as_str(),
                    updated.parent_id.as_deref(),
                    updated.container_node_id.as_deref(),
                    updated.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    updated.embedding_vector.as_deref(),
                    id,
                ),
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to update node: {}", e)))?;
        } else {
            conn.execute(
                "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ? WHERE id = ?",
                (
                    updated.node_type.as_str(),
                    updated.content.as_str(),
                    updated.parent_id.as_deref(),
                    updated.container_node_id.as_deref(),
                    updated.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    updated.embedding_vector.as_deref(),
                    id,
                ),
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to update node: {}", e)))?;
        }

        // If content changed in a child node, mark the parent container as stale too
        // This ensures container embeddings stay fresh when children are modified
        if content_changed && !is_container {
            if let Some(ref container_id) = updated.container_node_id {
                conn.execute(
                    "UPDATE nodes SET embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                    [container_id.as_str()],
                )
                .await
                .map_err(|e| NodeServiceError::query_failed(format!("Failed to mark parent container as stale: {}", e)))?;
            }
        }

        // If container_node_id changed (node moved between containers), mark both old and new containers as stale
        // Moving a node affects the content of both containers semantically
        if update.container_node_id.is_some()
            && existing.container_node_id != updated.container_node_id
        {
            // Mark old container as stale (if it exists)
            if let Some(ref old_container_id) = existing.container_node_id {
                conn.execute(
                    "UPDATE nodes SET embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                    [old_container_id.as_str()],
                )
                .await
                .ok(); // Don't fail update if old container no longer exists
            }

            // Mark new container as stale (if different from what content_changed handled)
            // Only needed if content didn't change (content_changed already handled new container above)
            if !content_changed {
                if let Some(ref new_container_id) = updated.container_node_id {
                    conn.execute(
                        "UPDATE nodes SET embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                        [new_container_id.as_str()],
                    )
                    .await
                    .ok(); // Don't fail update if new container doesn't exist
                }
            }
        }

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
    /// ```rust
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

        if let Some(node_type) = update.node_type {
            updated.node_type = node_type;
        }

        if let Some(content) = update.content {
            if updated.content != content {
                content_changed = true;
            }
            updated.content = content;
        }

        if let Some(parent_id) = update.parent_id {
            updated.parent_id = parent_id;
        }

        if let Some(ref container_node_id) = update.container_node_id {
            // Convert ROOT_CONTAINER_ID to None (null in database) - same as CREATE operation
            updated.container_node_id = match container_node_id {
                Some(id) if id == ROOT_CONTAINER_ID => None,
                other => other.clone(),
            };
        }

        if let Some(before_sibling_id) = update.before_sibling_id {
            updated.before_sibling_id = before_sibling_id;
        }

        if let Some(properties) = update.properties {
            updated.properties = properties;
        }

        if let Some(embedding_vector) = update.embedding_vector {
            updated.embedding_vector = embedding_vector;
        }

        // Validate updated node
        self.behaviors.validate_node(&updated)?;

        // Execute update with version check
        let conn = self.db.connect_with_timeout().await?;
        let properties_json = serde_json::to_string(&updated.properties)
            .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

        // Determine if this node is a container (container_node_id IS NULL)
        let is_container = updated.container_node_id.is_none();

        // Build UPDATE query with version check and atomic version increment
        // Returns 0 rows if version mismatch, 1 row if successful
        let rows_affected = if content_changed && is_container {
            conn.execute(
                "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ?, embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP, version = version + 1 WHERE id = ? AND version = ?",
                (
                    updated.node_type.as_str(),
                    updated.content.as_str(),
                    updated.parent_id.as_deref(),
                    updated.container_node_id.as_deref(),
                    updated.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    updated.embedding_vector.as_deref(),
                    id,
                    expected_version,
                ),
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to update node with version check: {}", e)))?
        } else {
            conn.execute(
                "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ?, version = version + 1 WHERE id = ? AND version = ?",
                (
                    updated.node_type.as_str(),
                    updated.content.as_str(),
                    updated.parent_id.as_deref(),
                    updated.container_node_id.as_deref(),
                    updated.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    updated.embedding_vector.as_deref(),
                    id,
                    expected_version,
                ),
            )
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to update node with version check: {}", e)))?
        };

        // If version mismatch (rows_affected = 0), return early
        // Caller will handle conflict by fetching current state
        if rows_affected == 0 {
            return Ok(0);
        }

        // Update succeeded - handle side effects (embedding staleness, mention sync)

        // If content changed in a child node, mark the parent container as stale too
        if content_changed && !is_container {
            if let Some(ref container_id) = updated.container_node_id {
                conn.execute(
                    "UPDATE nodes SET embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                    [container_id.as_str()],
                )
                .await
                .map_err(|e| NodeServiceError::query_failed(format!("Failed to mark parent container as stale: {}", e)))?;
            }
        }

        // If container_node_id changed (node moved between containers), mark both old and new containers as stale
        if update.container_node_id.is_some()
            && existing.container_node_id != updated.container_node_id
        {
            // Mark old container as stale (if it exists)
            if let Some(ref old_container_id) = existing.container_node_id {
                conn.execute(
                    "UPDATE nodes SET embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                    [old_container_id.as_str()],
                )
                .await
                .ok(); // Don't fail update if old container no longer exists
            }

            // Mark new container as stale (if different from what content_changed handled)
            if !content_changed {
                if let Some(ref new_container_id) = updated.container_node_id {
                    conn.execute(
                        "UPDATE nodes SET embedding_stale = TRUE, last_content_update = CURRENT_TIMESTAMP WHERE id = ?",
                        [new_container_id.as_str()],
                    )
                    .await
                    .ok(); // Don't fail update if new container doesn't exist
                }
            }
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

    /// Sync mention relationships when node content changes
    ///
    /// Compares old vs new mentions and updates database:
    /// - Adds new mention relationships
    /// - Removes deleted mention relationships
    /// - Prevents self-references and container-level self-references
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

        // Get node's container for validation (needed for container-level self-reference check)
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Add new mentions (filter out self-references and container-level self-references)
        for mentioned_id in to_add {
            // Skip direct self-references
            if mentioned_id.as_str() == node_id {
                tracing::debug!("Skipping self-reference: {} -> {}", node_id, mentioned_id);
                continue;
            }

            // Skip container-level self-references (child mentioning its own container)
            if let Some(container_id) = &node.container_node_id {
                if mentioned_id.as_str() == container_id {
                    tracing::debug!(
                        "Skipping container-level self-reference: {} -> {} (container: {})",
                        node_id,
                        mentioned_id,
                        container_id
                    );
                    continue;
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// service.delete_node("node-id-123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_node(
        &self,
        id: &str,
    ) -> Result<crate::models::DeleteResult, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await?;

        let rows_affected = conn
            .execute("DELETE FROM nodes WHERE id = ?", [id])
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to delete node: {}", e)))?;

        // Idempotent delete: return success even if node doesn't exist
        // This follows RESTful best practices and prevents race conditions
        // in distributed scenarios. DELETE is idempotent - deleting a
        // non-existent resource should succeed (HTTP 200/204).
        //
        // The DeleteResult provides visibility for debugging/auditing while
        // maintaining idempotence.
        Ok(if rows_affected > 0 {
            crate::models::DeleteResult::existed()
        } else {
            crate::models::DeleteResult::not_found()
        })
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
    /// ```rust
    /// let rows = service.delete_with_version_check("node-123", 5).await?;
    ///
    /// if rows == 0 {
    ///     // Either version conflict or node doesn't exist
    ///     // Caller should check if node still exists to distinguish
    /// }
    /// ```
    pub async fn delete_with_version_check(
        &self,
        id: &str,
        expected_version: i64,
    ) -> Result<usize, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await?;

        let rows_affected = conn
            .execute(
                "DELETE FROM nodes WHERE id = ? AND version = ?",
                (id, expected_version),
            )
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to delete node with version check: {}",
                    e
                ))
            })?;

        Ok(rows_affected as usize)
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// let children = service.get_children("parent-id").await?;
    /// println!("Found {} children", children.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_children(&self, parent_id: &str) -> Result<Vec<Node>, NodeServiceError> {
        let filter = NodeFilter::new()
            .with_parent_id(parent_id.to_string())
            .with_order_by(OrderBy::CreatedAsc);

        let mut children = self.query_nodes(filter).await?;

        // Sort children by sibling linked list (before_sibling_id)
        // This reconstructs the proper order independent of creation time
        self.sort_by_sibling_order(&mut children);

        Ok(children)
    }

    /// Bulk fetch all nodes belonging to an origin node (viewer/page)
    ///
    /// This is the efficient way to load a complete document tree:
    /// 1. Single database query fetches all nodes with the same container_node_id
    /// 2. In-memory hierarchy reconstruction using parent_id and before_sibling_id
    ///
    /// This avoids making multiple queries for each level of the tree.
    ///
    /// # Arguments
    ///
    /// * `container_node_id` - The ID of the origin node (e.g., date page ID)
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// // Fetch all nodes for a date page
    /// let nodes = service.get_nodes_by_container_id("2025-10-05").await?;
    /// println!("Found {} nodes in this document", nodes.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_nodes_by_container_id(
        &self,
        container_node_id: &str,
    ) -> Result<Vec<Node>, NodeServiceError> {
        let filter = NodeFilter::new().with_container_node_id(container_node_id.to_string());

        self.query_nodes(filter).await
    }

    /// Sort nodes by their sibling linked list order
    ///
    /// Nodes use before_sibling_id to form a linked list:
    /// - before_sibling_id = None means it's the first node
    /// - before_sibling_id = Some(id) means it comes after node with that id
    ///
    /// This function reconstructs the proper order by following the chain.
    fn sort_by_sibling_order(&self, nodes: &mut Vec<Node>) {
        if nodes.is_empty() {
            return;
        }

        // Build a map of id -> node for quick lookup
        let mut node_map: std::collections::HashMap<String, Node> =
            nodes.drain(..).map(|n| (n.id.clone(), n)).collect();

        // Find the first node (before_sibling_id is None)
        let first_node = node_map
            .values()
            .find(|n| n.before_sibling_id.is_none())
            .cloned();

        if let Some(first) = first_node {
            let mut ordered = vec![first.clone()];
            node_map.remove(&first.id);

            // Follow the chain: find nodes that have before_sibling_id = current node's id
            while !node_map.is_empty() {
                let current_id = ordered.last().unwrap().id.clone();

                // Find the next node in the chain
                let next = node_map
                    .values()
                    .find(|n| n.before_sibling_id.as_ref() == Some(&current_id))
                    .cloned();

                if let Some(next_node) = next {
                    let next_id = next_node.id.clone();
                    ordered.push(next_node);
                    node_map.remove(&next_id);
                } else {
                    // Chain broken or multiple chains - append remaining nodes by creation time
                    let mut remaining: Vec<Node> = node_map.values().cloned().collect();
                    remaining.sort_by(|a, b| a.created_at.cmp(&b.created_at));
                    ordered.extend(remaining);
                    break;
                }
            }

            *nodes = ordered;
        } else {
            // No node with before_sibling_id = None found
            // Fall back to creation time ordering
            nodes.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        }
    }

    /// Move a node to a new parent
    ///
    /// Updates the parent_id and container_node_id of a node, maintaining hierarchy consistency.
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// // Move node under new parent
    /// service.move_node("node-id", Some("new-parent-id")).await?;
    ///
    /// // Make node a root
    /// service.move_node("node-id", None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn move_node(
        &self,
        node_id: &str,
        new_parent: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Verify node exists
        let _node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

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

        // Determine new container_node_id
        let new_container_node_id = match new_parent {
            Some(parent_id) => {
                // Get parent's container_node_id, or use parent as root if it's a root node
                let parent = self
                    .get_node(parent_id)
                    .await?
                    .ok_or_else(|| NodeServiceError::invalid_parent(parent_id))?;
                parent.container_node_id.or(Some(parent_id.to_string()))
            }
            None => None, // Node becomes a root
        };

        let update = NodeUpdate {
            parent_id: Some(new_parent.map(String::from)),
            container_node_id: Some(new_container_node_id),
            ..Default::default()
        };

        self.update_node(node_id, update).await
    }

    /// Reorder siblings using before_sibling_id pointer
    ///
    /// Sets the before_sibling_id to position a node in its sibling list.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to reorder
    /// * `before_sibling_id` - The sibling to position before (None = end of list)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// // Position node before sibling
    /// service.reorder_siblings("node-id", Some("sibling-id")).await?;
    ///
    /// // Move to end of list
    /// service.reorder_siblings("node-id", None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reorder_siblings(
        &self,
        node_id: &str,
        before_sibling_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Verify node exists
        let _node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Verify sibling exists if provided
        if let Some(sibling_id) = before_sibling_id {
            let sibling_exists = self.node_exists(sibling_id).await?;
            if !sibling_exists {
                return Err(NodeServiceError::hierarchy_violation(format!(
                    "Sibling node {} does not exist",
                    sibling_id
                )));
            }
        }

        let update = NodeUpdate {
            before_sibling_id: Some(before_sibling_id.map(String::from)),
            ..Default::default()
        };

        self.update_node(node_id, update).await
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::NodeFilter;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// let filter = NodeFilter::new()
    ///     .with_node_type("task".to_string())
    ///     .with_limit(10);
    /// let nodes = service.query_nodes(filter).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_nodes(&self, filter: NodeFilter) -> Result<Vec<Node>, NodeServiceError> {
        // Use connection with busy timeout for better concurrency handling
        let conn = self.db.connect_with_timeout().await?;

        // For simple queries, we'll use specific patterns. Complex dynamic queries need a query builder
        // For this implementation, we'll handle the most common cases

        if let Some(ref node_type) = filter.node_type {
            // Query by node_type
            let order_clause = match filter.order_by {
                Some(OrderBy::CreatedAsc) => " ORDER BY created_at ASC",
                Some(OrderBy::CreatedDesc) => " ORDER BY created_at DESC",
                Some(OrderBy::ModifiedAsc) => " ORDER BY modified_at ASC",
                Some(OrderBy::ModifiedDesc) => " ORDER BY modified_at DESC",
                _ => "",
            };

            let limit_clause = filter
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            let query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector FROM nodes WHERE node_type = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

            let mut rows = stmt.query([node_type.as_str()]).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
            })?;

            let mut nodes = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                let mut node = self.row_to_node(row)?;
                self.backfill_schema_version(&mut node).await?;
                self.apply_lazy_migration(&mut node).await?;
                nodes.push(node);
            }

            return Ok(nodes);
        } else if let Some(ref parent_id) = filter.parent_id {
            // Query by parent_id
            let order_clause = match filter.order_by {
                Some(OrderBy::CreatedAsc) => " ORDER BY created_at ASC",
                Some(OrderBy::CreatedDesc) => " ORDER BY created_at DESC",
                Some(OrderBy::ModifiedAsc) => " ORDER BY modified_at ASC",
                Some(OrderBy::ModifiedDesc) => " ORDER BY modified_at DESC",
                _ => "",
            };

            let limit_clause = filter
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            let query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector FROM nodes WHERE parent_id = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

            let mut rows = stmt.query([parent_id.as_str()]).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
            })?;

            let mut nodes = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                let mut node = self.row_to_node(row)?;
                self.backfill_schema_version(&mut node).await?;
                self.apply_lazy_migration(&mut node).await?;
                nodes.push(node);
            }

            return Ok(nodes);
        } else if let Some(ref container_node_id) = filter.container_node_id {
            // Query by container_node_id (bulk fetch optimization)
            let order_clause = match filter.order_by {
                Some(OrderBy::CreatedAsc) => " ORDER BY created_at ASC",
                Some(OrderBy::CreatedDesc) => " ORDER BY created_at DESC",
                Some(OrderBy::ModifiedAsc) => " ORDER BY modified_at ASC",
                Some(OrderBy::ModifiedDesc) => " ORDER BY modified_at DESC",
                _ => "",
            };

            let limit_clause = filter
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            let query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector FROM nodes WHERE container_node_id = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

            let mut rows = stmt
                .query([container_node_id.as_str()])
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
                })?;

            let mut nodes = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                let mut node = self.row_to_node(row)?;
                self.backfill_schema_version(&mut node).await?;
                self.apply_lazy_migration(&mut node).await?;
                nodes.push(node);
            }

            return Ok(nodes);
        }

        // Default: return all nodes (with optional ordering/limit)
        let order_clause = match filter.order_by {
            Some(OrderBy::CreatedAsc) => " ORDER BY created_at ASC",
            Some(OrderBy::CreatedDesc) => " ORDER BY created_at DESC",
            Some(OrderBy::ModifiedAsc) => " ORDER BY modified_at ASC",
            Some(OrderBy::ModifiedDesc) => " ORDER BY modified_at DESC",
            _ => "",
        };

        let limit_clause = filter
            .limit
            .map(|l| format!(" LIMIT {}", l))
            .unwrap_or_default();

        let query = format!(
            "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector FROM nodes{}{}",
            order_clause, limit_clause
        );

        let mut stmt = conn.prepare(&query).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
        })?;

        let mut rows = stmt.query(()).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        let mut nodes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            nodes.push(self.row_to_node(row)?);
        }

        Ok(nodes)
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
    ///
    /// // Filter-only query (return all containers and tasks)
    /// let query = NodeQuery { include_containers_and_tasks: Some(true), ..Default::default() };
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
    /// 5. `include_containers_and_tasks` - Filter-only query (returns all matching nodes)
    /// 6. Empty query - Returns empty vec (safer than returning all nodes)
    ///
    /// # Note on Empty Queries
    ///
    /// Queries with no parameters (all fields `None` or `false`) will return an empty vector.
    /// This is intentional to prevent accidentally fetching all nodes from the database.
    /// To query all containers and tasks, use `include_containers_and_tasks: Some(true)`.
    /// Helper function to generate container/task filter SQL clause
    ///
    /// Returns a SQL WHERE clause fragment that filters to only include:
    /// - Task nodes (node_type = 'task')
    /// - Container nodes (container_node_id IS NULL)
    ///
    /// # Arguments
    /// * `enabled` - Whether to apply the filter
    /// * `table_alias` - Optional table alias (e.g., Some("n") for "n.node_type")
    ///
    /// # Safety
    /// This function only generates hardcoded SQL fragments - no user input is interpolated.
    /// The filter is applied via string concatenation (not parameterized) because it's
    /// structural SQL (column names and operators), not user data.
    fn build_container_task_filter(enabled: bool, table_alias: Option<&str>) -> String {
        if !enabled {
            return String::new();
        }

        let prefix = table_alias.map(|a| format!("{}.", a)).unwrap_or_default();
        format!(
            " AND ({}node_type = 'task' OR {}container_node_id IS NULL)",
            prefix, prefix
        )
    }

    pub async fn query_nodes_simple(
        &self,
        query: crate::models::NodeQuery,
    ) -> Result<Vec<Node>, NodeServiceError> {
        // Use connection with busy timeout to prevent immediate failure on lock contention
        // This is especially important for queries that might run concurrently with deletes
        let conn = self.db.connect_with_timeout().await?;

        // Determine if container/task filtering should be applied
        let filter_enabled = query.include_containers_and_tasks.unwrap_or(false);

        // Priority 1: Query by ID (exact match)
        if let Some(ref id) = query.id {
            if let Some(node) = self.get_node(id).await? {
                return Ok(vec![node]);
            } else {
                return Ok(vec![]);
            }
        }

        // Priority 2: Query nodes that mention a specific node (mentioned_by)
        // Uses the node_mentions table for efficient lookups
        if let Some(ref mentioned_node_id) = query.mentioned_by {
            let limit_clause = query
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            let container_task_filter =
                Self::build_container_task_filter(filter_enabled, Some("n"));

            // Query node_mentions table to find nodes that reference the target
            let sql_query = format!(
                "SELECT n.id, n.node_type, n.content, n.parent_id, n.container_node_id, n.before_sibling_id, n.version, n.created_at, n.modified_at, n.properties, n.embedding_vector
                 FROM nodes n
                 INNER JOIN node_mentions nm ON n.id = nm.node_id
                 WHERE nm.mentions_node_id = ?{}{}",
                container_task_filter, limit_clause
            );

            let mut stmt = conn.prepare(&sql_query).await.map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to prepare mentioned_by query: {}",
                    e
                ))
            })?;

            let mut rows = stmt
                .query([mentioned_node_id.as_str()])
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to execute mentioned_by query: {}",
                        e
                    ))
                })?;

            let mut nodes = Vec::new();
            // TODO(performance): This creates N+1 query pattern (2 queries per node).
            // Consider implementing populate_mentions_batch() when queries regularly
            // return >50 nodes. See issue #158 for batch optimization implementation.
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                let mut node = self.row_to_node(row)?;
                self.populate_mentions(&mut node).await?;
                nodes.push(node);
            }

            return Ok(nodes);
        }

        // Priority 3: Query by content substring (case-insensitive LIKE)
        if let Some(ref content_search) = query.content_contains {
            let content_pattern = format!("%{}%", content_search);
            let limit_clause = query
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            // Generate filter clause without table alias (queries nodes table directly)
            let container_task_filter = Self::build_container_task_filter(filter_enabled, None);

            // Determine SQL query based on whether node_type is specified
            let mut rows = if let Some(ref node_type) = query.node_type {
                // Case 1: Filter by both content and node_type
                let sql = format!(
                    "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector
                     FROM nodes WHERE content LIKE ? AND node_type = ?{}{}",
                    container_task_filter, limit_clause
                );

                let mut stmt = conn.prepare(&sql).await.map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to prepare content_contains query: {}",
                        e
                    ))
                })?;

                stmt.query([content_pattern.as_str(), node_type.as_str()])
                    .await
                    .map_err(|e| {
                        NodeServiceError::query_failed(format!(
                            "Failed to execute content_contains query: {}",
                            e
                        ))
                    })?
            } else {
                // Case 2: Filter by content only
                let sql = format!(
                    "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector
                     FROM nodes WHERE content LIKE ?{}{}",
                    container_task_filter, limit_clause
                );

                let mut stmt = conn.prepare(&sql).await.map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to prepare content_contains query: {}",
                        e
                    ))
                })?;

                stmt.query([content_pattern.as_str()]).await.map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to execute content_contains query: {}",
                        e
                    ))
                })?
            };

            // Collect results with mentions populated
            let mut nodes = Vec::new();
            // TODO(performance): This creates N+1 query pattern (2 queries per node).
            // Consider implementing populate_mentions_batch() when queries regularly
            // return >50 nodes. See issue #158 for batch optimization implementation.
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                let mut node = self.row_to_node(row)?;
                self.populate_mentions(&mut node).await?;
                nodes.push(node);
            }

            return Ok(nodes);
        }

        // Priority 4: Query by node_type only
        if let Some(ref node_type) = query.node_type {
            let limit_clause = query
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            // Generate filter clause without table alias (queries nodes table directly)
            let container_task_filter = Self::build_container_task_filter(filter_enabled, None);

            let sql_query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE node_type = ?{}{}",
                container_task_filter, limit_clause
            );

            let mut stmt = conn.prepare(&sql_query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare node_type query: {}", e))
            })?;

            let mut rows = stmt.query([node_type.as_str()]).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to execute node_type query: {}", e))
            })?;

            let mut nodes = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                let mut node = self.row_to_node(row)?;
                self.backfill_schema_version(&mut node).await?;
                self.apply_lazy_migration(&mut node).await?;
                nodes.push(node);
            }

            return Ok(nodes);
        }

        // Priority 5: Filter-only query (return nodes matching container/task filter)
        // This handles cases where only the filter is specified without other query parameters
        if filter_enabled {
            let container_task_filter = Self::build_container_task_filter(filter_enabled, None);
            let limit_clause = query
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            // WHERE 1=1 is a SQL idiom for dynamic WHERE clauses - it's always true,
            // allowing the filter to be appended via AND clause. This is clearer than
            // conditional WHERE clause generation and maintains consistent query structure.
            let sql_query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, version, created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE 1=1{}{}",
                container_task_filter, limit_clause
            );

            let mut stmt = conn.prepare(&sql_query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

            let mut rows = stmt.query([] as [&str; 0]).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
            })?;

            let mut nodes = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                let mut node = self.row_to_node(row)?;
                self.backfill_schema_version(&mut node).await?;
                self.apply_lazy_migration(&mut node).await?;
                nodes.push(node);
            }

            return Ok(nodes);
        }

        // Default: Empty query returns no results (safer than returning all nodes)
        Ok(vec![])
    }

    // Helper methods

    /// Check if a node exists
    async fn node_exists(&self, id: &str) -> Result<bool, NodeServiceError> {
        let conn = self.db.connect()?;

        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM nodes WHERE id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

        let mut rows = stmt.query([id]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let count: i64 = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            Ok(count > 0)
        } else {
            Ok(false)
        }
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

            if let Some(node) = self.get_node(&current_id).await? {
                if let Some(parent_id) = node.parent_id {
                    current_id = parent_id;
                } else {
                    break; // Reached root without finding node_id
                }
            } else {
                break;
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::Node;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// let nodes = vec![
    ///     Node::new("text".to_string(), "Note 1".to_string(), None, json!({})),
    ///     Node::new("text".to_string(), "Note 2".to_string(), None, json!({})),
    /// ];
    /// let ids = service.bulk_create(nodes).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_create(&self, nodes: Vec<Node>) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Validate all nodes first
        for node in &nodes {
            self.behaviors.validate_node(node)?;
        }

        let conn = self.db.connect()?;

        // Begin transaction
        conn.execute("BEGIN TRANSACTION", ()).await.map_err(|e| {
            NodeServiceError::transaction_failed(format!("Failed to begin transaction: {}", e))
        })?;

        let mut ids = Vec::new();

        // Insert all nodes
        // Database defaults handle created_at and modified_at timestamps automatically
        for node in &nodes {
            let properties_json = serde_json::to_string(&node.properties)
                .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

            let result = conn.execute(
                "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    node.id.as_str(),
                    node.node_type.as_str(),
                    node.content.as_str(),
                    node.parent_id.as_deref(),
                    node.container_node_id.as_deref(),
                    node.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    node.embedding_vector.as_deref(),
                ),
            )
            .await;

            if let Err(e) = result {
                // Rollback on error
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(NodeServiceError::bulk_operation_failed(format!(
                    "Failed to insert node {}: {}",
                    node.id, e
                )));
            }

            ids.push(node.id.clone());
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            NodeServiceError::transaction_failed(format!("Failed to commit transaction: {}", e))
        })?;

        Ok(ids)
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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

        let conn = self.db.connect()?;

        // Begin transaction
        conn.execute("BEGIN TRANSACTION", ()).await.map_err(|e| {
            NodeServiceError::transaction_failed(format!("Failed to begin transaction: {}", e))
        })?;

        for (id, update) in updates {
            // Apply update similar to update_node but in transaction
            let existing = self
                .get_node(&id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(&id))?;

            let mut updated = existing.clone();

            if let Some(node_type) = update.node_type {
                updated.node_type = node_type;
            }

            if let Some(content) = update.content {
                updated.content = content;
            }

            if let Some(parent_id) = update.parent_id {
                updated.parent_id = parent_id;
            }

            if let Some(container_node_id) = update.container_node_id {
                updated.container_node_id = container_node_id;
            }

            if let Some(before_sibling_id) = update.before_sibling_id {
                updated.before_sibling_id = before_sibling_id;
            }

            if let Some(properties) = update.properties {
                updated.properties = properties;
            }

            if let Some(embedding_vector) = update.embedding_vector {
                updated.embedding_vector = embedding_vector;
            }

            updated.modified_at = Utc::now();

            // Validate updated node
            if let Err(e) = self.behaviors.validate_node(&updated) {
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(NodeServiceError::bulk_operation_failed(format!(
                    "Failed to validate node {}: {}",
                    id, e
                )));
            }

            // Execute update in transaction - database auto-updates modified_at
            let properties_json = serde_json::to_string(&updated.properties)
                .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

            let result = conn.execute(
                "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ? WHERE id = ?",
                (
                    updated.node_type.as_str(),
                    updated.content.as_str(),
                    updated.parent_id.as_deref(),
                    updated.container_node_id.as_deref(),
                    updated.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    updated.embedding_vector.as_deref(),
                    id.as_str(),
                ),
            )
            .await;

            if let Err(e) = result {
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(NodeServiceError::bulk_operation_failed(format!(
                    "Failed to update node {}: {}",
                    id, e
                )));
            }
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            NodeServiceError::transaction_failed(format!("Failed to commit transaction: {}", e))
        })?;

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// let ids = vec!["node-1".to_string(), "node-2".to_string()];
    /// service.bulk_delete(ids).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_delete(&self, ids: Vec<String>) -> Result<(), NodeServiceError> {
        if ids.is_empty() {
            return Ok(());
        }

        let conn = self.db.connect()?;

        // Begin transaction
        conn.execute("BEGIN TRANSACTION", ()).await.map_err(|e| {
            NodeServiceError::transaction_failed(format!("Failed to begin transaction: {}", e))
        })?;

        for id in &ids {
            let result = conn
                .execute("DELETE FROM nodes WHERE id = ?", [id.as_str()])
                .await;

            if let Err(e) = result {
                // Rollback on error
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(NodeServiceError::bulk_operation_failed(format!(
                    "Failed to delete node {}: {}",
                    id, e
                )));
            }
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            NodeServiceError::transaction_failed(format!("Failed to commit transaction: {}", e))
        })?;

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
        container_node_id: &str,
        before_sibling_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        let conn = self.db.connect_with_timeout().await?;

        // Use DEFERRED transaction for better concurrency with WAL mode
        // Lock is only acquired when first write occurs, not at BEGIN
        conn.execute("BEGIN DEFERRED", ()).await.map_err(|e| {
            NodeServiceError::transaction_failed(format!("Failed to begin transaction: {}", e))
        })?;

        // Use INSERT OR IGNORE for parent - won't error if already exists
        let parent_result = conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
             VALUES (?, 'date', ?, NULL, NULL, NULL, '{}', NULL)",
            (parent_id, parent_id)
        ).await;

        if let Err(e) = parent_result {
            let _rollback = conn.execute("ROLLBACK", ()).await;
            return Err(NodeServiceError::query_failed(format!(
                "Failed to ensure parent exists: {}",
                e
            )));
        }

        // Use INSERT ... ON CONFLICT for node - upsert in single operation
        let node_result = conn.execute(
            "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
             VALUES (?, ?, ?, ?, ?, ?, '{}', NULL)
             ON CONFLICT(id) DO UPDATE SET
                content = excluded.content,
                parent_id = excluded.parent_id,
                container_node_id = excluded.container_node_id,
                before_sibling_id = excluded.before_sibling_id,
                modified_at = CURRENT_TIMESTAMP",
            (node_id, node_type, content, parent_id, container_node_id, before_sibling_id)
        ).await;

        if let Err(e) = node_result {
            let _rollback = conn.execute("ROLLBACK", ()).await;
            return Err(NodeServiceError::query_failed(format!(
                "Failed to upsert node: {}",
                e
            )));
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            NodeServiceError::transaction_failed(format!("Failed to commit transaction: {}", e))
        })?;

        Ok(())
    }

    // Helper methods

    /// Convert a database row to a Node
    fn row_to_node(&self, row: libsql::Row) -> Result<Node, NodeServiceError> {
        let id: String = row
            .get(0)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let node_type: String = row
            .get(1)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let content: String = row
            .get(2)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let parent_id: Option<String> = row
            .get(3)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let container_node_id: Option<String> = row
            .get(4)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let before_sibling_id: Option<String> = row
            .get(5)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let version: i64 = row
            .get(6)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let created_at: String = row
            .get(7)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let modified_at: String = row
            .get(8)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let properties_json: String = row
            .get(9)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let embedding_vector: Option<Vec<u8>> = row
            .get(10)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        let properties: serde_json::Value =
            serde_json::from_str(&properties_json).map_err(|e| {
                NodeServiceError::serialization_error(format!("Failed to parse properties: {}", e))
            })?;

        // Parse timestamps - handle both SQLite format and RFC3339 (for migration)
        let created_at = parse_timestamp(&created_at).map_err(|e| {
            NodeServiceError::serialization_error(format!(
                "Failed to parse created_at '{}': {}",
                created_at, e
            ))
        })?;

        let modified_at = parse_timestamp(&modified_at).map_err(|e| {
            NodeServiceError::serialization_error(format!(
                "Failed to parse modified_at '{}': {}",
                modified_at, e
            ))
        })?;

        Ok(Node {
            id,
            node_type,
            content,
            parent_id,
            container_node_id,
            before_sibling_id,
            version,
            created_at,
            modified_at,
            properties,
            embedding_vector,
            mentions: Vec::new(), // Populated separately via populate_mentions()
            mentioned_by: Vec::new(), // Populated separately via populate_mentions()
        })
    }

    /// Populate mentions fields from node_mentions table
    ///
    /// Queries the node_mentions table to populate both outgoing mentions
    /// and incoming mentioned_by references for a node.
    ///
    /// # Arguments
    ///
    /// * `node` - Mutable reference to node to populate
    async fn populate_mentions(&self, node: &mut Node) -> Result<(), NodeServiceError> {
        let conn = self.db.connect()?;

        // Query outgoing mentions (nodes that THIS node references)
        let mut stmt = conn
            .prepare("SELECT mentions_node_id FROM node_mentions WHERE node_id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare mentions query: {}", e))
            })?;

        let mut rows = stmt.query([node.id.as_str()]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute mentions query: {}", e))
        })?;

        let mut mentions = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let mentioned_id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            mentions.push(mentioned_id);
        }
        node.mentions = mentions;

        // Query incoming mentions (nodes that reference THIS node)
        let mut stmt = conn
            .prepare("SELECT node_id FROM node_mentions WHERE mentions_node_id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to prepare mentioned_by query: {}",
                    e
                ))
            })?;

        let mut rows = stmt.query([node.id.as_str()]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute mentioned_by query: {}", e))
        })?;

        let mut mentioned_by = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let mentioning_id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            mentioned_by.push(mentioning_id);
        }
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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

        // Prevent container-level self-references (child mentioning its own container)
        let source_node = self
            .get_node(source_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(source_id))?;

        if let Some(container_id) = &source_node.container_node_id {
            if container_id == target_id {
                return Err(NodeServiceError::ValidationFailed(
                    crate::models::ValidationError::InvalidParent(
                        "Cannot mention own container (container-level self-reference)".to_string(),
                    ),
                ));
            }
        }

        let conn = self.db.connect()?;

        // Use INSERT OR IGNORE to handle duplicates gracefully
        conn.execute(
            "INSERT OR IGNORE INTO node_mentions (node_id, mentions_node_id) VALUES (?, ?)",
            (source_id, target_id),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to insert mention: {}", e)))?;

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// service.remove_mention("node-123", "node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn remove_mention(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        let conn = self.db.connect()?;

        conn.execute(
            "DELETE FROM node_mentions WHERE node_id = ? AND mentions_node_id = ?",
            (source_id, target_id),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to delete mention: {}", e)))?;

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// let mentions = service.get_mentions("node-123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentions(&self, node_id: &str) -> Result<Vec<String>, NodeServiceError> {
        let conn = self.db.connect()?;

        let mut stmt = conn
            .prepare("SELECT mentions_node_id FROM node_mentions WHERE node_id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare mentions query: {}", e))
            })?;

        let mut rows = stmt.query([node_id]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute mentions query: {}", e))
        })?;

        let mut mentions = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let mentioned_id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            mentions.push(mentioned_id);
        }

        Ok(mentions)
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// let backlinks = service.get_mentioned_by("node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentioned_by(&self, node_id: &str) -> Result<Vec<String>, NodeServiceError> {
        let conn = self.db.connect()?;

        let mut stmt = conn
            .prepare("SELECT node_id FROM node_mentions WHERE mentions_node_id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to prepare mentioned_by query: {}",
                    e
                ))
            })?;

        let mut rows = stmt.query([node_id]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute mentioned_by query: {}", e))
        })?;

        let mut mentioned_by = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let mentioning_id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            mentioned_by.push(mentioning_id);
        }

        Ok(mentioned_by)
    }

    /// Get container nodes of nodes that mention the target node (backlinks at container level).
    ///
    /// This resolves incoming mentions to their container nodes and deduplicates.
    ///
    /// # Container Resolution Logic
    /// - For task and ai-chat nodes: Uses the node's own ID (they are their own containers)
    /// - For other nodes: Uses their container_node_id (or the node ID itself if it's a root)
    ///
    /// # Example
    /// ```no_run
    /// # use nodespace_core::services::node_service::NodeService;
    /// # use nodespace_core::services::database::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
        let conn = self.db.connect()?;

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
            NodeServiceError::query_failed(format!(
                "Failed to prepare mentioning_containers query: {}",
                e
            ))
        })?;

        let mut rows = stmt.query([node_id]).await.map_err(|e| {
            NodeServiceError::query_failed(format!(
                "Failed to execute mentioning_containers query: {}",
                e
            ))
        })?;

        let mut containers = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let container_id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            containers.push(container_id);
        }

        Ok(containers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn create_test_service() -> (NodeService, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = DatabaseService::new(db_path).await.unwrap();
        let service = NodeService::new(db).unwrap();
        (service, temp_dir)
    }

    #[tokio::test]
    async fn test_create_text_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new(
            "text".to_string(),
            "Hello World".to_string(),
            None,
            json!({}),
        );

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
            None,
            json!({"status": "in_progress", "priority": 1}),
        );

        let id = service.create_node(node).await.unwrap();
        let retrieved = service.get_node(&id).await.unwrap().unwrap();

        assert_eq!(retrieved.node_type, "task");
        assert_eq!(retrieved.properties["status"], "in_progress");
        assert_eq!(retrieved.properties["priority"], 1);
    }

    #[tokio::test]
    async fn test_create_date_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new_with_id(
            "2025-01-03".to_string(),
            "text".to_string(),
            "2025-01-03".to_string(),
            None,
            json!({}),
        );

        let id = service.create_node(node).await.unwrap();
        assert_eq!(id, "2025-01-03");

        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.node_type, "date");
        assert_eq!(retrieved.id, "2025-01-03");
    }

    #[tokio::test]
    async fn test_auto_create_date_node_parent() {
        let (service, _temp) = create_test_service().await;

        // Create a text node with a date node parent that doesn't exist yet
        let text_node = Node::new_with_id(
            "test-node-1".to_string(),
            "text".to_string(),
            "Hello from a date".to_string(),
            Some("2025-10-13".to_string()), // Date node parent that doesn't exist
            json!({}),
        );

        // Should succeed - date node should be auto-created
        let id = service.create_node(text_node).await.unwrap();
        assert_eq!(id, "test-node-1");

        // Verify the text node was created
        let retrieved_text = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved_text.node_type, "text");
        assert_eq!(retrieved_text.content, "Hello from a date");
        assert_eq!(retrieved_text.parent_id, Some("2025-10-13".to_string()));

        // Verify the date node was auto-created
        let retrieved_date = service.get_node("2025-10-13").await.unwrap().unwrap();
        assert_eq!(retrieved_date.node_type, "date");
        assert_eq!(retrieved_date.id, "2025-10-13");
        assert_eq!(retrieved_date.parent_id, None);
        assert_eq!(retrieved_date.content, ""); // Date nodes have empty content
    }

    #[tokio::test]
    async fn test_update_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new("text".to_string(), "Original".to_string(), None, json!({}));

        let id = service.create_node(node).await.unwrap();

        let update = NodeUpdate::new().with_content("Updated".to_string());
        service.update_node(&id, update).await.unwrap();

        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Updated");
    }

    #[tokio::test]
    async fn test_delete_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new(
            "text".to_string(),
            "To be deleted".to_string(),
            None,
            json!({}),
        );

        let id = service.create_node(node).await.unwrap();
        service.delete_node(&id).await.unwrap();

        let retrieved = service.get_node(&id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_get_children() {
        let (service, _temp) = create_test_service().await;

        let parent = Node::new("text".to_string(), "Parent".to_string(), None, json!({}));
        let parent_id = service.create_node(parent).await.unwrap();

        let child1 = Node::new(
            "text".to_string(),
            "Child 1".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        service.create_node(child1).await.unwrap();

        let child2 = Node::new(
            "text".to_string(),
            "Child 2".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        service.create_node(child2).await.unwrap();

        let children = service.get_children(&parent_id).await.unwrap();
        assert_eq!(children.len(), 2);
    }

    #[tokio::test]
    async fn test_move_node() {
        let (service, _temp) = create_test_service().await;

        let root = Node::new("text".to_string(), "Root".to_string(), None, json!({}));
        let container_node_id = service.create_node(root).await.unwrap();

        let node = Node::new("text".to_string(), "Node".to_string(), None, json!({}));
        let node_id = service.create_node(node).await.unwrap();

        service
            .move_node(&node_id, Some(&container_node_id))
            .await
            .unwrap();

        let moved = service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(moved.parent_id, Some(container_node_id.clone()));
        assert_eq!(moved.container_node_id, Some(container_node_id));
    }

    #[tokio::test]
    async fn test_query_nodes_by_type() {
        let (service, _temp) = create_test_service().await;

        service
            .create_node(Node::new(
                "text".to_string(),
                "Text 1".to_string(),
                None,
                json!({}),
            ))
            .await
            .unwrap();
        service
            .create_node(Node::new(
                "task".to_string(),
                "Task 1".to_string(),
                None,
                json!({"status": "pending"}),
            ))
            .await
            .unwrap();
        service
            .create_node(Node::new(
                "text".to_string(),
                "Text 2".to_string(),
                None,
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
            Node::new("text".to_string(), "Bulk 1".to_string(), None, json!({})),
            Node::new("text".to_string(), "Bulk 2".to_string(), None, json!({})),
            Node::new(
                "task".to_string(),
                "Bulk Task".to_string(),
                None,
                json!({"status": "pending"}),
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

        let node1 = Node::new(
            "text".to_string(),
            "Original 1".to_string(),
            None,
            json!({}),
        );
        let node2 = Node::new(
            "text".to_string(),
            "Original 2".to_string(),
            None,
            json!({}),
        );

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
    async fn test_bulk_delete() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Delete 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Delete 2".to_string(), None, json!({}));

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
    async fn test_circular_reference_prevention() {
        let (service, _temp) = create_test_service().await;

        let parent = Node::new("text".to_string(), "Parent".to_string(), None, json!({}));
        let parent_id = service.create_node(parent).await.unwrap();

        let child = Node::new(
            "text".to_string(),
            "Child".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        let child_id = service.create_node(child).await.unwrap();

        // Attempt to move parent under child (circular reference)
        let result = service.move_node(&parent_id, Some(&child_id)).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            NodeServiceError::CircularReference { .. }
        ));
    }

    #[tokio::test]
    async fn test_reorder_siblings() {
        let (service, _temp) = create_test_service().await;

        let parent = Node::new("text".to_string(), "Parent".to_string(), None, json!({}));
        let parent_id = service.create_node(parent).await.unwrap();

        let child1 = Node::new(
            "text".to_string(),
            "Child 1".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        let child1_id = service.create_node(child1).await.unwrap();

        let child2 = Node::new(
            "text".to_string(),
            "Child 2".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        let child2_id = service.create_node(child2).await.unwrap();

        // Reorder child2 to be before child1
        service
            .reorder_siblings(&child2_id, Some(&child1_id))
            .await
            .unwrap();

        let reordered = service.get_node(&child2_id).await.unwrap().unwrap();
        assert_eq!(reordered.before_sibling_id, Some(child1_id));
    }

    #[tokio::test]
    async fn test_transaction_rollback_on_error() {
        let (service, _temp) = create_test_service().await;

        // Create one valid node and one invalid node
        let valid_node = Node::new("text".to_string(), "Valid".to_string(), None, json!({}));
        let mut invalid_node = Node::new("text".to_string(), "".to_string(), None, json!({})); // Empty content invalid for text
        invalid_node.content = "   ".to_string(); // Whitespace-only content

        let nodes = vec![valid_node.clone(), invalid_node];

        // Bulk create should fail
        let result = service.bulk_create(nodes).await;
        assert!(result.is_err());

        // Verify that valid node was NOT created (transaction rolled back)
        let check = service.get_node(&valid_node.id).await.unwrap();
        assert!(check.is_none());
    }

    #[tokio::test]
    async fn test_get_nodes_by_container_id() {
        let (service, _temp) = create_test_service().await;

        // Create a date node (acts as container)
        let date_node = Node::new_with_id(
            "2025-10-05".to_string(),
            "text".to_string(),
            "2025-10-05".to_string(),
            None,
            json!({}),
        );
        service.create_node(date_node.clone()).await.unwrap();

        // Create children with container_node_id pointing to the date
        let child1 = Node::new_with_root(
            "text".to_string(),
            "Child 1".to_string(),
            Some("2025-10-05".to_string()),
            Some("2025-10-05".to_string()),
            json!({}),
        );
        let child2 = Node::new_with_root(
            "text".to_string(),
            "Child 2".to_string(),
            Some("2025-10-05".to_string()),
            Some("2025-10-05".to_string()),
            json!({}),
        );
        let child3 = Node::new_with_root(
            "task".to_string(),
            "Child 3".to_string(),
            Some("2025-10-05".to_string()),
            Some("2025-10-05".to_string()),
            json!({"status": "pending"}),
        );

        service.create_node(child1.clone()).await.unwrap();
        service.create_node(child2.clone()).await.unwrap();
        service.create_node(child3.clone()).await.unwrap();

        // Create a different date node with a child (should not be returned)
        let other_date = Node::new_with_id(
            "2025-10-06".to_string(),
            "text".to_string(),
            "2025-10-06".to_string(),
            None,
            json!({}),
        );
        service.create_node(other_date.clone()).await.unwrap();

        let other_child = Node::new_with_root(
            "text".to_string(),
            "Other child".to_string(),
            Some("2025-10-06".to_string()),
            Some("2025-10-06".to_string()),
            json!({}),
        );
        service.create_node(other_child).await.unwrap();

        // Bulk fetch should return only the 3 children with container_node_id = "2025-10-05"
        let nodes = service
            .get_nodes_by_container_id("2025-10-05")
            .await
            .unwrap();

        assert_eq!(nodes.len(), 3, "Should return exactly 3 nodes");
        assert!(
            nodes
                .iter()
                .all(|n| n.container_node_id == Some("2025-10-05".to_string())),
            "All nodes should have container_node_id = '2025-10-05'"
        );

        // Verify it returns different node types
        let node_types: Vec<&str> = nodes.iter().map(|n| n.node_type.as_str()).collect();
        assert!(node_types.contains(&"text"), "Should contain text nodes");
        assert!(node_types.contains(&"task"), "Should contain task node");

        // Verify the other date's child is not included
        assert!(
            !nodes.iter().any(|n| n.content == "Other child"),
            "Should not return children from other origins"
        );
    }

    #[tokio::test]
    async fn test_add_mention() {
        let (service, _temp) = create_test_service().await;

        // Create two nodes
        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

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

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

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

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));
        let node3 = Node::new("text".to_string(), "Node 3".to_string(), None, json!({}));

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

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));
        let node3 = Node::new("text".to_string(), "Node 3".to_string(), None, json!({}));

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

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

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

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
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

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

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
        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

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
        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
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

    /// Tests for include_containers_and_tasks filter functionality
    ///
    /// These tests verify the filter that restricts query results to only
    /// task nodes and container nodes (nodes with no parent), excluding
    /// regular text children and other non-referenceable content.
    ///
    /// # Test Organization
    ///
    /// - `basic_filter()` - Core filter behavior (includes/excludes correct nodes)
    /// - `content_contains_with_filter()` - Filter + full-text search integration
    /// - `mentioned_by_with_filter()` - Filter + mention query integration
    /// - `node_type_with_filter()` - Filter + type query interaction (edge case: tasks always included)
    /// - `default_behavior()` - Verifies filter defaults to false (no filtering)
    ///
    /// All tests use unique content markers (e.g., "UniqueBasicFilter") to prevent
    /// cross-test contamination and ensure proper test isolation.
    mod container_task_filter_tests {
        use super::*;

        #[tokio::test]
        async fn basic_filter() {
            let (service, _temp) = create_test_service().await;

            // Create a container node (no parent = root/container)
            let container = Node::new(
                "text".to_string(),
                "UniqueBasicFilter Container".to_string(),
                None,
                json!({}),
            );
            let container_id = service.create_node(container).await.unwrap();

            // Create a task node (child of container)
            let task = Node::new_with_id(
                "task-1".to_string(),
                "task".to_string(),
                "UniqueBasicFilter Task".to_string(),
                Some(container_id.clone()),
                json!({"status": "pending"}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Create a regular text child node (should be filtered out)
            let text_child = Node::new_with_id(
                "text-child-1".to_string(),
                "text".to_string(),
                "UniqueBasicFilter Text".to_string(),
                Some(container_id.clone()),
                json!({}),
            );
            let text_child_id = service.create_node(text_child).await.unwrap();

            // Query WITH filter enabled - use content match to isolate this test's nodes
            let query_with_filter = crate::models::NodeQuery {
                content_contains: Some("UniqueBasicFilter".to_string()),
                include_containers_and_tasks: Some(true),
                ..Default::default()
            };
            let filtered_results = service.query_nodes_simple(query_with_filter).await.unwrap();

            // Verify we only get container and task nodes
            assert_eq!(
                filtered_results.len(),
                2,
                "Should return exactly 2 nodes (container + task)"
            );

            let result_ids: Vec<&str> = filtered_results.iter().map(|n| n.id.as_str()).collect();
            assert!(
                result_ids.contains(&container_id.as_str()),
                "Should include container node"
            );
            assert!(
                result_ids.contains(&task_id.as_str()),
                "Should include task node"
            );
            assert!(
                !result_ids.contains(&text_child_id.as_str()),
                "Should NOT include text child node"
            );

            // Query WITHOUT filter - should get all 3 nodes created in this test
            let query_no_filter = crate::models::NodeQuery {
                content_contains: Some("UniqueBasicFilter".to_string()),
                include_containers_and_tasks: Some(false),
                ..Default::default()
            };
            let unfiltered_results = service.query_nodes_simple(query_no_filter).await.unwrap();

            assert_eq!(
                unfiltered_results.len(),
                3,
                "Should return all 3 nodes when filter disabled"
            );
        }

        #[tokio::test]
        async fn content_contains_with_filter() {
            let (service, _temp) = create_test_service().await;

            // Create container with "meeting" in content
            let container = Node::new(
                "text".to_string(),
                "Team meeting notes".to_string(),
                None,
                json!({}),
            );
            let container_id = service.create_node(container).await.unwrap();

            // Create task with "meeting" in content (child of container)
            let task = Node::new_with_id(
                "task-meeting".to_string(),
                "task".to_string(),
                "Schedule meeting".to_string(),
                Some(container_id.clone()),
                json!({}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Create text child with "meeting" in content (should be filtered out)
            let text_child = Node::new_with_id(
                "text-meeting".to_string(),
                "text".to_string(),
                "Meeting agenda item".to_string(),
                Some(container_id.clone()),
                json!({}),
            );
            service.create_node(text_child).await.unwrap();

            // Query for "meeting" WITH container/task filter
            let query = crate::models::NodeQuery {
                content_contains: Some("meeting".to_string()),
                include_containers_and_tasks: Some(true),
                ..Default::default()
            };
            let results = service.query_nodes_simple(query).await.unwrap();

            // Should only return container and task, not the text child
            assert_eq!(
                results.len(),
                2,
                "Should return only container and task nodes with 'meeting'"
            );

            let result_ids: Vec<&str> = results.iter().map(|n| n.id.as_str()).collect();
            assert!(result_ids.contains(&container_id.as_str()));
            assert!(result_ids.contains(&task_id.as_str()));
        }

        #[tokio::test]
        async fn mentioned_by_with_filter() {
            let (service, _temp) = create_test_service().await;

            // Create a target node to be mentioned
            let target = Node::new_with_id(
                "target-node".to_string(),
                "text".to_string(),
                "Target".to_string(),
                None,
                json!({}),
            );
            let target_id = service.create_node(target).await.unwrap();

            // Create container that mentions target
            let container = Node::new_with_id(
                "container-1".to_string(),
                "text".to_string(),
                "Container mentioning @target-node".to_string(),
                None,
                json!({}),
            );
            let container_id = service.create_node(container).await.unwrap();
            service
                .create_mention(&container_id, &target_id)
                .await
                .unwrap();

            // Create task that mentions target (child of container)
            let task = Node::new_with_id(
                "task-mentions".to_string(),
                "task".to_string(),
                "Task with @target-node reference".to_string(),
                Some(container_id.clone()),
                json!({}),
            );
            let task_id = service.create_node(task).await.unwrap();
            service.create_mention(&task_id, &target_id).await.unwrap();

            // Create text child that mentions target (should be filtered out)
            let text_child = Node::new_with_id(
                "text-mentions".to_string(),
                "text".to_string(),
                "Text with @target-node".to_string(),
                Some(container_id.clone()),
                json!({}),
            );
            let text_child_id = service.create_node(text_child).await.unwrap();
            service
                .create_mention(&text_child_id, &target_id)
                .await
                .unwrap();

            // Query nodes that mention target WITH container/task filter
            let query = crate::models::NodeQuery {
                mentioned_by: Some(target_id.clone()),
                include_containers_and_tasks: Some(true),
                ..Default::default()
            };
            let results = service.query_nodes_simple(query).await.unwrap();

            // Should only return container and task
            assert_eq!(
                results.len(),
                2,
                "Should return only container and task that mention target"
            );

            let result_ids: Vec<&str> = results.iter().map(|n| n.id.as_str()).collect();
            assert!(result_ids.contains(&container_id.as_str()));
            assert!(result_ids.contains(&task_id.as_str()));
            assert!(!result_ids.contains(&text_child_id.as_str()));
        }

        #[tokio::test]
        async fn node_type_with_filter() {
            let (service, _temp) = create_test_service().await;

            // Create multiple task nodes - some containers, some children
            let container_task = Node::new_with_id(
                "task-container".to_string(),
                "task".to_string(),
                "Container task".to_string(),
                None,
                json!({}),
            );
            let container_task_id = service.create_node(container_task).await.unwrap();

            let child_task = Node::new_with_id(
                "task-child".to_string(),
                "task".to_string(),
                "Child task".to_string(),
                Some(container_task_id.clone()),
                json!({}),
            );
            service.create_node(child_task).await.unwrap();

            // Query for task nodes WITH container/task filter
            // This should still return task nodes even if they're children,
            // because the filter is (node_type = 'task' OR container_node_id IS NULL)
            let query = crate::models::NodeQuery {
                node_type: Some("task".to_string()),
                include_containers_and_tasks: Some(true),
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
                None,
                json!({}),
            );
            service.create_node(container).await.unwrap();

            let task = Node::new(
                "task".to_string(),
                "UniqueDefaultTest Task".to_string(),
                None,
                json!({}),
            );
            service.create_node(task).await.unwrap();

            // Query with filter = None (default should be false)
            // Use content search to isolate this test's nodes
            let query = crate::models::NodeQuery {
                content_contains: Some("UniqueDefaultTest".to_string()),
                include_containers_and_tasks: None, // Defaults to false
                ..Default::default()
            };
            let results = service.query_nodes_simple(query).await.unwrap();

            // Should return all nodes (filter defaults to false via unwrap_or)
            assert_eq!(
                results.len(),
                2,
                "Default behavior should not filter (include_containers_and_tasks defaults to false)"
            );
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

            // Create a container node
            let container = Node::new(
                "text".to_string(),
                "Container page".to_string(),
                None,
                json!({}),
            );
            let container_id = service.create_node(container).await.unwrap();

            // Create a child text node in the container
            let child = Node::new_with_id(
                "child-text".to_string(),
                "text".to_string(),
                "See @target".to_string(),
                Some(container_id.clone()),
                json!({}),
            );
            let child_id = service.create_node(child).await.unwrap();

            // Create target node
            let target = Node::new_with_id(
                "target".to_string(),
                "text".to_string(),
                "Target page".to_string(),
                None,
                json!({}),
            );
            let target_id = service.create_node(target).await.unwrap();

            // Child mentions target
            service.create_mention(&child_id, &target_id).await.unwrap();

            // Get mentioning containers for target
            let containers = service.get_mentioning_containers(&target_id).await.unwrap();

            // Should return the container (not the child)
            assert_eq!(containers.len(), 1, "Should return exactly one container");
            assert_eq!(
                containers[0], container_id,
                "Should return the container node, not the child"
            );
        }

        #[tokio::test]
        async fn deduplication() {
            let (service, _temp) = create_test_service().await;

            // Create a container
            let container = Node::new(
                "text".to_string(),
                "Container page".to_string(),
                None,
                json!({}),
            );
            let container_id = service.create_node(container).await.unwrap();

            // Create two child nodes in the same container
            let child1 = Node::new_with_id(
                "child-1".to_string(),
                "text".to_string(),
                "First mention of @target".to_string(),
                Some(container_id.clone()),
                json!({}),
            );
            let child1_id = service.create_node(child1).await.unwrap();

            let child2 = Node::new_with_id(
                "child-2".to_string(),
                "text".to_string(),
                "Second mention of @target".to_string(),
                Some(container_id.clone()),
                json!({}),
            );
            let child2_id = service.create_node(child2).await.unwrap();

            // Create target node
            let target = Node::new_with_id(
                "target-dedup".to_string(),
                "text".to_string(),
                "Target page".to_string(),
                None,
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

            // Get mentioning containers
            let containers = service.get_mentioning_containers(&target_id).await.unwrap();

            // Should return only ONE container (deduplicated)
            assert_eq!(
                containers.len(),
                1,
                "Should deduplicate to single container despite two children mentioning target"
            );
            assert_eq!(
                containers[0], container_id,
                "Should return the container node"
            );
        }

        #[tokio::test]
        async fn task_exception() {
            let (service, _temp) = create_test_service().await;

            // Create a container
            let container = Node::new(
                "text".to_string(),
                "Container page".to_string(),
                None,
                json!({}),
            );
            let container_id = service.create_node(container).await.unwrap();

            // Create a task node (child of container)
            let task = Node::new_with_id(
                "task-1".to_string(),
                "task".to_string(),
                "Review @target".to_string(),
                Some(container_id.clone()),
                json!({"status": "pending"}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Create target node
            let target = Node::new_with_id(
                "target-task".to_string(),
                "text".to_string(),
                "Target page".to_string(),
                None,
                json!({}),
            );
            let target_id = service.create_node(target).await.unwrap();

            // Task mentions target
            service.create_mention(&task_id, &target_id).await.unwrap();

            // Get mentioning containers
            let containers = service.get_mentioning_containers(&target_id).await.unwrap();

            // Should return the TASK itself (not its container)
            assert_eq!(containers.len(), 1, "Should return exactly one container");
            assert_eq!(
                containers[0], task_id,
                "Task nodes should be treated as their own containers (exception rule)"
            );
            assert_ne!(
                containers[0], container_id,
                "Should NOT return the parent container for task nodes"
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
                None,
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

            // Create three different containers
            let container1 = Node::new(
                "text".to_string(),
                "Container page".to_string(),
                None,
                json!({}),
            );
            let container1_id = service.create_node(container1).await.unwrap();

            let container2 = Node::new(
                "text".to_string(),
                "Container 2".to_string(),
                None,
                json!({}),
            );
            let container2_id = service.create_node(container2).await.unwrap();

            let container3 = Node::new(
                "text".to_string(),
                "Container 3".to_string(),
                None,
                json!({}),
            );
            let container3_id = service.create_node(container3).await.unwrap();

            // Create children in different containers
            let child1 = Node::new_with_id(
                "child-c1".to_string(),
                "text".to_string(),
                "From container 1".to_string(),
                Some(container1_id.clone()),
                json!({}),
            );
            let child1_id = service.create_node(child1).await.unwrap();

            let child2 = Node::new_with_id(
                "child-c2".to_string(),
                "text".to_string(),
                "From container 2".to_string(),
                Some(container2_id.clone()),
                json!({}),
            );
            let child2_id = service.create_node(child2).await.unwrap();

            // Create task in container 3 (should return itself)
            let task = Node::new_with_id(
                "task-c3".to_string(),
                "task".to_string(),
                "From container 3".to_string(),
                Some(container3_id.clone()),
                json!({}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Create target node
            let target = Node::new_with_id(
                "target-mixed".to_string(),
                "text".to_string(),
                "Target page".to_string(),
                None,
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

            // Should return 3 unique containers (2 dates + task)
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
            let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
            let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));
            let node3 = Node::new("text".to_string(), "Node 3".to_string(), None, json!({}));

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
            let node = Node::new(
                "text".to_string(),
                "Self ref test".to_string(),
                None,
                json!({}),
            );
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
        async fn test_prevent_container_level_self_reference() {
            let (service, _temp) = create_test_service().await;

            // Create container node
            let container = Node::new("text".to_string(), "Container".to_string(), None, json!({}));
            let container_id = service.create_node(container).await.unwrap();

            // Create child node within container
            let mut child = Node::new("text".to_string(), "Child".to_string(), None, json!({}));
            child.container_node_id = Some(container_id.clone());
            let child_id = service.create_node(child).await.unwrap();

            // Try to update child to mention its own container
            let update = NodeUpdate::new().with_content(format!(
                "Mention container [@container](nodespace://{})",
                container_id
            ));
            service.update_node(&child_id, update).await.unwrap();

            // Verify container-level self-reference was NOT created
            let child_with_mentions = service.get_node(&child_id).await.unwrap().unwrap();
            assert_eq!(
                child_with_mentions.mentions.len(),
                0,
                "Should not create container-level self-reference"
            );
        }

        #[tokio::test]
        async fn test_sync_mentions_multiple_adds_and_removes() {
            let (service, _temp) = create_test_service().await;

            // Create nodes
            let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
            let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));
            let node3 = Node::new("text".to_string(), "Node 3".to_string(), None, json!({}));
            let node4 = Node::new("text".to_string(), "Node 4".to_string(), None, json!({}));

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
            let node = Node::new(
                "text".to_string(),
                "Daily note".to_string(),
                None,
                json!({}),
            );
            let node_id = service.create_node(node).await.unwrap();

            // Create a date node
            let date_node = Node::new_with_id(
                "2025-10-24".to_string(),
                "text".to_string(),
                "October 24".to_string(),
                None,
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
            let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
            let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

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
                let node = Node::new(
                    "text".to_string(),
                    "Test content".to_string(),
                    None,
                    json!({}),
                );

                let id = service.create_node(node).await.unwrap();
                let retrieved = service.get_node(&id).await.unwrap().unwrap();

                // Verify _schema_version was added
                assert!(retrieved.properties.get("_schema_version").is_some());
                assert_eq!(retrieved.properties["_schema_version"], 1);
            }

            #[tokio::test]
            async fn test_backfill_existing_nodes_without_version() {
                let (service, _temp) = create_test_service().await;

                // Manually insert a node without _schema_version (simulating existing data)
                let conn = service.db.connect_with_timeout().await.unwrap();
                let node_id = "test-node-no-version";
                let properties_json = json!({"some_field": "value"}).to_string();

                conn.execute(
                    "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    (
                        node_id,
                        "text",
                        "Test content",
                        None::<&str>,
                        None::<&str>,
                        None::<&str>,
                        properties_json.as_str(),
                        None::<&str>,
                    ),
                )
                .await
                .unwrap();

                // Retrieve the node - should trigger backfill
                let retrieved = service.get_node(node_id).await.unwrap().unwrap();

                // Verify _schema_version was backfilled
                assert!(retrieved.properties.get("_schema_version").is_some());
                assert_eq!(retrieved.properties["_schema_version"], 1);

                // Verify original field is still present
                assert_eq!(retrieved.properties["some_field"], "value");

                // Verify the backfill was persisted to database
                let retrieved_again = service.get_node(node_id).await.unwrap().unwrap();
                assert_eq!(retrieved_again.properties["_schema_version"], 1);
            }

            #[tokio::test]
            async fn test_query_nodes_backfills_versions() {
                let (service, _temp) = create_test_service().await;

                // Manually insert multiple nodes without _schema_version
                let conn = service.db.connect_with_timeout().await.unwrap();

                for i in 1..=3 {
                    let node_id = format!("test-node-{}", i);
                    let properties_json = json!({"field": i}).to_string();

                    conn.execute(
                        "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                        (
                            node_id.as_str(),
                            "text",
                            format!("Content {}", i).as_str(),
                            None::<&str>,
                            None::<&str>,
                            None::<&str>,
                            properties_json.as_str(),
                            None::<&str>,
                        ),
                    )
                    .await
                    .unwrap();
                }

                // Query all text nodes - should trigger backfill for all
                let filter = NodeFilter::new().with_node_type("text".to_string());
                let nodes = service.query_nodes(filter).await.unwrap();

                // Verify all nodes got _schema_version backfilled
                assert!(nodes.len() >= 3);
                for node in &nodes {
                    assert!(node.properties.get("_schema_version").is_some());
                    assert_eq!(node.properties["_schema_version"], 1);
                }
            }

            #[tokio::test]
            async fn test_auto_created_date_nodes_get_version() {
                let (service, _temp) = create_test_service().await;

                // Create a node with date parent (will auto-create date node)
                let text_node = Node::new_with_id(
                    "test-text-node".to_string(),
                    "text".to_string(),
                    "Test content".to_string(),
                    Some("2025-01-15".to_string()),
                    json!({}),
                );

                service.create_node(text_node).await.unwrap();

                // Verify the auto-created date node has _schema_version
                let date_node = service.get_node("2025-01-15").await.unwrap().unwrap();
                assert!(date_node.properties.get("_schema_version").is_some());
                assert_eq!(date_node.properties["_schema_version"], 1);
            }

            #[tokio::test]
            async fn test_nodes_with_existing_version_not_modified() {
                let (service, _temp) = create_test_service().await;

                // Create a node (will get version 1)
                let node = Node::new(
                    "text".to_string(),
                    "Test content".to_string(),
                    None,
                    json!({}),
                );

                let id = service.create_node(node).await.unwrap();

                // Retrieve twice to ensure version doesn't get modified on subsequent access
                let retrieved1 = service.get_node(&id).await.unwrap().unwrap();
                let retrieved2 = service.get_node(&id).await.unwrap().unwrap();

                assert_eq!(retrieved1.properties["_schema_version"], 1);
                assert_eq!(retrieved2.properties["_schema_version"], 1);
                assert_eq!(
                    retrieved1.modified_at, retrieved2.modified_at,
                    "Modified timestamp should not change on backfill skip"
                );
            }
        }
    }
}
