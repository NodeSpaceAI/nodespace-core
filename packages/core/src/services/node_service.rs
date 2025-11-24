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
use crate::db::SurrealStore;
use crate::models::{Node, NodeFilter, NodeUpdate};
use crate::services::error::NodeServiceError;
use crate::services::migration_registry::MigrationRegistry;
use chrono::{DateTime, NaiveDateTime, Utc};
use regex::Regex;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

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

/// Parse timestamp from database - handles both SQLite and RFC3339 formats
#[allow(dead_code)]
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
/// use nodespace_core::db::SurrealStore;
/// use nodespace_core::models::Node;
/// use std::path::PathBuf;
/// use serde_json::json;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let db = SurrealStore::new(PathBuf::from("./data/test.db")).await?;
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
    /// * `store` - SurrealStore instance
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let store = Arc::new(SurrealStore::new("./data/nodespace.db".into()).await?);
    /// let service = NodeService::new(store)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(store: Arc<SurrealStore<C>>) -> Result<Self, NodeServiceError> {
        // Create empty migration registry (no migrations registered yet - pre-deployment)
        // Infrastructure exists for future schema evolution post-deployment
        let migration_registry = MigrationRegistry::new();

        Ok(Self {
            store,
            behaviors: Arc::new(NodeBehaviorRegistry::new()),
            migration_registry: Arc::new(migration_registry),
        })
    }

    /// Get access to the underlying SurrealStore
    ///
    /// Useful for advanced operations that need direct database access
    pub fn store(&self) -> &Arc<SurrealStore<C>> {
        &self.store
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
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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

        // Parse schema definition
        // If parsing fails (e.g., old schema format), skip schema validation gracefully
        let schema: crate::models::SchemaDefinition = match serde_json::from_value(schema_json) {
            Ok(s) => s,
            Err(_) => {
                // Schema exists but can't be parsed - skip schema validation
                // This handles backward compatibility with old schema formats
                return Ok(());
            }
        };

        // Use the helper function to validate with the parsed schema
        self.validate_node_with_schema(node, &schema)
    }

    /// Apply schema default values to missing fields using a pre-loaded schema
    ///
    /// For each field in the schema that has a default value, if the field is missing
    /// from the node's properties, add it with the default value.
    ///
    /// # Arguments
    ///
    /// * `node` - Mutable reference to the node to apply defaults to
    /// * `schema` - Pre-loaded schema definition to use
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Defaults applied successfully
    /// * `Err` - Error applying defaults
    fn apply_schema_defaults_with_schema(
        &self,
        node: &mut Node,
        schema: &crate::models::SchemaDefinition,
    ) -> Result<(), NodeServiceError> {
        // Ensure properties is an object
        if !node.properties.is_object() {
            node.properties = serde_json::json!({});
        }

        // Get mutable reference to properties object
        let props_obj = node.properties.as_object_mut().unwrap();

        // Apply defaults for missing fields
        for field in &schema.fields {
            // Check if field is missing
            if !props_obj.contains_key(&field.name) {
                // Apply default value if one is defined
                if let Some(default_value) = &field.default {
                    props_obj.insert(field.name.clone(), default_value.clone());
                }
            }
        }

        Ok(())
    }

    /// Validate a node against a pre-loaded schema definition
    ///
    /// # Arguments
    ///
    /// * `node` - The node to validate
    /// * `schema` - Pre-loaded schema definition to validate against
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Validation passed
    /// * `Err` - Validation failed
    fn validate_node_with_schema(
        &self,
        node: &Node,
        schema: &crate::models::SchemaDefinition,
    ) -> Result<(), NodeServiceError> {
        // Get properties for this node type (supports both flat and nested formats)
        let node_props = node
            .properties
            .get(&node.node_type)
            .or(Some(&node.properties))
            .and_then(|p| p.as_object());

        // Validate each field in the schema
        for field in &schema.fields {
            let field_value = node_props.and_then(|props| props.get(&field.name));

            // Check required fields
            if field.required.unwrap_or(false) && field_value.is_none() {
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
                        let valid_values = schema.get_enum_values(&field.name).unwrap_or_default();

                        if !valid_values.contains(&value_str.to_string()) {
                            return Err(NodeServiceError::invalid_update(format!(
                                "Invalid value '{}' for enum field '{}'. Valid values: {}",
                                value_str,
                                field.name,
                                valid_values.join(", ")
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
        if let Some(props_obj) = node.properties.as_object() {
            if !props_obj.contains_key("_schema_version") {
                // Determine version from schema if exists, otherwise default to 1
                let version =
                    if let Some(schema) = self.get_schema_for_type(&node.node_type).await? {
                        schema.get("version").and_then(|v| v.as_i64()).unwrap_or(1)
                    } else {
                        1 // Default version for types without schema
                    };

                // Add version to node properties IN-MEMORY ONLY
                // Don't persist to database - this prevents overwriting freshly created spoke records
                // Issue #511: After node type conversion, the spoke record has status+_schema_version
                // Backfill would MERGE just _schema_version, but the spoke already has it
                // Persisting backfill is unnecessary and risks race conditions
                if let Some(props_obj) = node.properties.as_object_mut() {
                    props_obj.insert("_schema_version".to_string(), serde_json::json!(version));
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

        // Persist migrated node to database using SurrealStore
        let update = NodeUpdate {
            properties: Some(migrated_node.properties.clone()),
            ..Default::default()
        };
        self.store
            .update_node(&node.id, update)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to persist migrated node: {}", e))
            })?;

        // Update the in-memory node
        *node = migrated_node;

        Ok(())
    }

    pub async fn create_node(&self, mut node: Node) -> Result<String, NodeServiceError> {
        // INVARIANT: Date nodes MUST have node_type="date" AND content=id (enforced by validation).
        // Auto-detect date nodes by ID format (YYYY-MM-DD) to prevent validation failures
        // from incorrect client input. This maintains data integrity regardless of caller mistakes.
        if is_date_node_id(&node.id) {
            node.node_type = "date".to_string();
            node.content = node.id.clone(); // Content MUST match ID for validation
        }

        // Step 1: Core behavior validation (PROTECTED)
        // Validates basic data integrity (non-empty content, correct types, etc.)
        self.behaviors.validate_node(&node)?;

        // Step 1.5: Apply schema defaults and validate
        // Apply default values for missing fields before validation
        // Skip for schema nodes to avoid circular dependency
        //
        // NOTE: We ONLY apply schema defaults, NOT behavior defaults.
        // Behavior defaults (markdown_enabled, auto_save, etc.) are UI preferences
        // that should be handled client-side, not stored in database properties.
        // The properties field is for user data and schema-defined fields only.
        if node.node_type != "schema" {
            // Fetch schema once and reuse it for both operations
            if let Some(schema_json) = self.get_schema_for_type(&node.node_type).await? {
                // Parse schema definition
                if let Ok(schema) =
                    serde_json::from_value::<crate::models::SchemaDefinition>(schema_json.clone())
                {
                    // Apply defaults from schema fields only
                    self.apply_schema_defaults_with_schema(&mut node, &schema)?;

                    // Validate with the same schema
                    self.validate_node_with_schema(&node, &schema)?;
                }
            }
            // If no schema exists, that's fine - just don't add any defaults
            // Properties should contain only what the user explicitly provided
        }

        // NOTE: Parent/container validation removed - now handled by NodeOperations layer
        // The graph-native architecture uses edges for hierarchy, not fields on Node struct

        // Add schema version to properties ONLY if schema has fields
        // For empty schemas (text, date, header, etc.), don't pollute properties with version
        // Schema versioning is only needed for types with schema-defined fields (task, person, etc.)
        let mut properties = node.properties.clone();
        if let Some(schema) = self.get_schema_for_type(&node.node_type).await? {
            // Check if schema has any fields
            if let Some(fields) = schema.get("fields").and_then(|f| f.as_array()) {
                if !fields.is_empty() {
                    // Schema has fields - add version for migration tracking
                    if let Some(version) = schema.get("version").and_then(|v| v.as_i64()) {
                        if let Some(props_obj) = properties.as_object_mut() {
                            props_obj
                                .insert("_schema_version".to_string(), serde_json::json!(version));
                        }
                    }
                }
            }
        }
        // Note: No else clause - if no schema or empty schema, don't add version
        // The backfill_schema_version function will add it on read if needed

        // Update node with properties (only versioned if schema has fields)
        node.properties = properties;

        // NOTE: root_id filtering removed - hierarchy now managed via edges

        // Create node via store
        self.store
            .create_node(node.clone())
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to insert node: {}", e)))?;

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
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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

        self.store
            .create_mention(mentioning_node_id, mentioned_node_id, final_root_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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
        self.store
            .delete_mention(mentioning_node_id, mentioned_node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
                // Date nodes are root-level containers (no parent/container edges)
                let virtual_date = Node {
                    id: id.to_string(),
                    node_type: "date".to_string(),
                    content: id.to_string(), // Content MUST match ID for validation
                    version: 1,
                    created_at: chrono::Utc::now(),
                    modified_at: chrono::Utc::now(),
                    properties: serde_json::json!({}),
                    embedding_vector: None,
                    mentions: vec![],
                    mentioned_by: vec![],
                };
                return Ok(Some(virtual_date));
            }

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
    /// # use nodespace_core::db::SurrealStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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

        // NOTE: Sibling ordering is now handled via has_child edge order field.
        // Use reorder_siblings() or move_node() for ordering changes.

        if let Some(properties) = update.properties {
            // Merge properties instead of replacing them to preserve _schema_version
            if let (Some(existing_obj), Some(new_obj)) =
                (updated.properties.as_object_mut(), properties.as_object())
            {
                // Merge new properties into existing ones
                for (key, value) in new_obj {
                    existing_obj.insert(key.clone(), value.clone());
                }
            } else {
                // If either is not an object, just replace (shouldn't happen normally)
                updated.properties = properties;
            }
        }

        if let Some(embedding_vector) = update.embedding_vector {
            updated.embedding_vector = embedding_vector;
        }

        // Step 1: Core behavior validation (PROTECTED)
        self.behaviors.validate_node(&updated)?;

        // Step 1.5: Apply schema defaults and validate (if node type changed)
        // Apply default values for missing fields when node type changes
        // Skip for schema nodes to avoid circular dependency
        if node_type_changed && updated.node_type != "schema" {
            // Fetch schema once and reuse it for both operations
            if let Some(schema_json) = self.get_schema_for_type(&updated.node_type).await? {
                // Parse schema definition
                if let Ok(schema) =
                    serde_json::from_value::<crate::models::SchemaDefinition>(schema_json.clone())
                {
                    // Apply defaults for the new node type
                    self.apply_schema_defaults_with_schema(&mut updated, &schema)?;

                    // Validate with the same schema
                    self.validate_node_with_schema(&updated, &schema)?;
                }
            }
        } else if updated.node_type != "schema" {
            // Step 2: Schema validation only (node type didn't change)
            self.validate_node_against_schema(&updated).await?;
        }

        // Update node via store
        let node_update = crate::models::NodeUpdate {
            node_type: Some(updated.node_type.clone()),
            content: Some(updated.content.clone()),
            properties: Some(updated.properties.clone()),
            embedding_vector: if updated.embedding_vector.is_some() {
                Some(updated.embedding_vector.clone())
            } else {
                None
            },
        };

        self.store
            .update_node(id, node_update)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

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

        // NOTE: Sibling ordering is now handled via has_child edge order field.
        // Use reorder_siblings() or move_node() for ordering changes.

        if let Some(properties) = update.properties {
            // Merge properties instead of replacing them to preserve _schema_version
            if let (Some(existing_obj), Some(new_obj)) =
                (updated.properties.as_object_mut(), properties.as_object())
            {
                // Merge new properties into existing ones
                for (key, value) in new_obj {
                    existing_obj.insert(key.clone(), value.clone());
                }
            } else {
                // If either is not an object, just replace (shouldn't happen normally)
                updated.properties = properties;
            }
        }

        if let Some(embedding_vector) = update.embedding_vector {
            updated.embedding_vector = embedding_vector;
        }

        // Step 1: Core behavior validation (PROTECTED)
        self.behaviors.validate_node(&updated)?;

        // Step 2: Schema validation (USER-EXTENSIBLE)
        if updated.node_type != "schema" {
            self.validate_node_against_schema(&updated).await?;
        }

        // Create node update
        let node_update = crate::models::NodeUpdate {
            node_type: Some(updated.node_type.clone()),
            content: Some(updated.content.clone()),
            properties: Some(updated.properties.clone()),
            embedding_vector: if updated.embedding_vector.is_some() {
                Some(updated.embedding_vector.clone())
            } else {
                None
            },
        };

        // Perform atomic update with version check
        let result = self
            .store
            .update_node_with_version_check(id, expected_version, node_update)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Check if update succeeded (version matched)
        let rows_affected = if result.is_some() { 1 } else { 0 };

        // If update failed due to version mismatch, return early
        if rows_affected == 0 {
            return Ok(0);
        }

        // Mark embedding as stale if content changed
        if content_changed {
            if let Err(e) = self.store.mark_embedding_stale(id).await {
                // Log warning but don't fail the update
                tracing::warn!("Failed to mark embedding as stale for node {}: {}", id, e);
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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// service.delete_node("node-id-123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_node(
        &self,
        id: &str,
    ) -> Result<crate::models::DeleteResult, NodeServiceError> {
        // Delegate to SurrealStore
        let result = self.store.delete_node(id).await.map_err(|e| {
            NodeServiceError::DatabaseError(crate::db::DatabaseError::SqlExecutionError {
                context: format!("Database operation failed: {}", e),
            })
        })?;

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
        self.store
            .delete_with_version_check(id, expected_version)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to delete node with version check: {}",
                    e
                ))
            })
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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
        // Fetch all nodes and edges in the subtree efficiently
        let nodes = self
            .store
            .get_nodes_in_subtree(parent_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to fetch subtree nodes: {}", e))
            })?;

        let edges = self
            .store
            .get_edges_in_subtree(parent_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to fetch subtree edges: {}", e))
            })?;

        // Build tree structure using adjacency list strategy
        let tree = self
            .build_tree_from_adjacency_list(parent_id, &nodes, &edges)
            .await?;
        Ok(tree)
    }

    /// Build a nested tree structure from nodes and edges using adjacency list strategy
    ///
    /// Constructs a nested JSON tree by:
    /// 1. Creating a HashMap mapping parent_id → Vec<child_id> (adjacency list)
    /// 2. Recursively walking the tree structure using the adjacency list
    /// 3. Building JSON nodes with `children` arrays
    ///
    /// This separates data fetching from tree construction, making the logic testable
    /// and enabling client-side tree modifications.
    ///
    /// # Arguments
    ///
    /// * `root_id` - The root node ID
    /// * `nodes` - All nodes in the subtree (flat list)
    /// * `edges` - All parent-child relationships in the subtree
    ///
    /// # Returns
    ///
    /// JSON structure with nested children arrays, or empty object if root not found
    async fn build_tree_from_adjacency_list(
        &self,
        root_id: &str,
        nodes: &[Node],
        edges: &[crate::EdgeRecord],
    ) -> Result<serde_json::Value, NodeServiceError> {
        use std::collections::HashMap;

        // Create a map of node_id → Node for O(1) lookup
        let mut node_map: HashMap<String, Node> = HashMap::new();
        for node in nodes {
            node_map.insert(node.id.clone(), node.clone());
        }

        // Create adjacency list: parent_id → Vec of (child_id, order)
        // Sorted by order to maintain sibling sequence
        let mut adjacency_list: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        for edge in edges {
            adjacency_list
                .entry(edge.in_node.clone())
                .or_default()
                .push((edge.out_node.clone(), edge.order));
        }

        // Sort children by order for each parent
        for children in adjacency_list.values_mut() {
            children.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        }

        // Fetch root node to include in response
        let root_node = self.get_node(root_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to fetch root node: {}", e))
        })?;

        match root_node {
            Some(root) => {
                // Recursively build tree structure
                let tree_json = self.build_node_tree_recursive(&root, &node_map, &adjacency_list);
                Ok(tree_json)
            }
            None => {
                // Root node not found, return empty object
                Ok(serde_json::json!({}))
            }
        }
    }

    /// Recursively build a tree node with its children
    ///
    /// Helper function for `build_tree_from_adjacency_list` that recursively
    /// constructs JSON nodes with nested children arrays.
    #[allow(clippy::only_used_in_recursion)]
    fn build_node_tree_recursive(
        &self,
        node: &Node,
        node_map: &std::collections::HashMap<String, Node>,
        adjacency_list: &std::collections::HashMap<String, Vec<(String, f64)>>,
    ) -> serde_json::Value {
        let mut json =
            serde_json::to_value(node).unwrap_or(serde_json::Value::Object(Default::default()));

        // Build children array (always present, even if empty for consistency)
        let children: Vec<serde_json::Value> =
            if let Some(children_ids) = adjacency_list.get(&node.id) {
                children_ids
                    .iter()
                    .filter_map(|(child_id, _order)| {
                        node_map.get(child_id).map(|child_node| {
                            self.build_node_tree_recursive(child_node, node_map, adjacency_list)
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
        // A node is a root if it has no incoming has_child edges
        // We check this by trying to get its parent - if parent is None, it's a root
        let parent = self.get_parent(node_id).await?;
        Ok(parent.is_none())
    }

    /// Get the parent of a node (via incoming has_child edge)
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
        // Query for nodes that have has_child edge pointing to this node
        // This is done via SurrealDB graph traversal: <-has_child
        let parent = self
            .store
            .get_parent(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        Ok(parent)
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
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
        // Hierarchy is now managed via edges - use get_children instead
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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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

        // Hierarchy is now managed via edges - use store's move_node
        self.store
            .move_node(node_id, new_parent, None)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Create parent-child edge atomically with before_sibling_id
    ///
    /// Used during node creation to establish parent relationship while preserving
    /// sibling ordering. This is separate from move_node() which is for moving existing nodes.
    ///
    /// # Arguments
    ///
    /// * `child_id` - ID of the child node (must already exist)
    /// * `parent_id` - ID of the parent node
    /// * `before_sibling_id` - Optional sibling to insert before (None = end of list)
    pub async fn create_parent_edge(
        &self,
        child_id: &str,
        parent_id: &str,
        before_sibling_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Convert before_sibling_id to insert_after_sibling_id for store.move_node
        //
        // API semantics (documented in CreateNodeParams):
        //   before_sibling_id = Some(id) → "place me BEFORE this sibling"
        //   before_sibling_id = None → "append at end of list"
        //
        // store.move_node semantics:
        //   insert_after_sibling_id = Some(id) → "insert AFTER this sibling"
        //   insert_after_sibling_id = None → "insert at beginning"
        //
        // Translation:
        //   before_sibling_id = Some(id) → find sibling before target, insert after it
        //   before_sibling_id = None → find last child, insert after it (append at end)
        let insert_after_id = if let Some(target_sibling_id) = before_sibling_id {
            // Get all children in order
            let children = self.get_children(parent_id).await?;

            // Find the position of the target sibling
            let target_pos = children.iter().position(|c| c.id == target_sibling_id);

            if let Some(pos) = target_pos {
                if pos == 0 {
                    // Target is first child, insert at beginning
                    None
                } else {
                    // Insert after the child right before the target
                    Some(children[pos - 1].id.clone())
                }
            } else {
                // Target sibling not found, append at end
                children.last().map(|c| c.id.clone())
            }
        } else {
            // No before_sibling_id specified = append at END of list
            let children = self.get_children(parent_id).await?;
            children.last().map(|c| c.id.clone())
        };

        // Use store's move_node which creates the has_child edge atomically
        self.store
            .move_node(child_id, Some(parent_id), insert_after_id.as_deref())
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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

        // Child ordering is handled via has_child edge order field.
        // Get current parent to move within the same parent
        let parent = self.get_parent(node_id).await?;
        let parent_id = parent.map(|p| p.id);

        // Use move_node to handle edge ordering (insert_after semantics)
        self.store
            .move_node(node_id, parent_id.as_deref(), insert_after)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

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
            embedding_vector: if node.embedding_vector.is_some() {
                Some(node.embedding_vector.clone())
            } else {
                None
            },
        };

        // Perform atomic update with version check
        let result = self
            .store
            .update_node_with_version_check(node_id, expected_version, node_update)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Check if update succeeded (version matched)
        if result.is_none() {
            return Err(NodeServiceError::query_failed(format!(
                "Version conflict: expected version {} for node {}",
                expected_version, node_id
            )));
        }

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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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
    /// - Root nodes (root_id IS NULL)
    ///
    /// # Arguments
    /// * `enabled` - Whether to apply the filter
    /// * `table_alias` - Optional table alias (e.g., Some("n") for "n.node_type")
    ///
    /// # Safety
    /// This function only generates hardcoded SQL fragments - no user input is interpolated.
    /// The filter is applied via string concatenation (not parameterized) because it's
    /// structural SQL (column names and operators), not user data.
    #[allow(dead_code)]
    fn build_container_task_filter(enabled: bool, table_alias: Option<&str>) -> String {
        if !enabled {
            return String::new();
        }

        let prefix = table_alias.map(|a| format!("{}.", a)).unwrap_or_default();
        format!(
            " AND ({}node_type = 'task' OR ({}root_id IS NULL AND {}node_type NOT IN ('date', 'schema')))",
            prefix, prefix, prefix
        )
    }

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

            // Walk up via parent edge
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
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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

        // Extract IDs for return (maintaining backward compatibility)
        Ok(created_nodes.into_iter().map(|n| n.id).collect())
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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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

        // Step 1: Validate all nodes BEFORE performing atomic update
        // This ensures we fail fast before any database changes
        for (id, update) in &updates {
            // Fetch existing node
            let existing = self
                .get_node(id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(id))?;

            let mut updated = existing.clone();

            // Apply partial updates to build validation candidate
            if let Some(node_type) = &update.node_type {
                updated.node_type = node_type.clone();
            }

            if let Some(content) = &update.content {
                updated.content = content.clone();
            }

            // NOTE: Sibling ordering is now handled via has_child edge order field.
            // Bulk updates don't support sibling reordering - use move_node instead.

            if let Some(properties) = &update.properties {
                updated.properties = properties.clone();
            }

            if let Some(embedding_vector) = &update.embedding_vector {
                updated.embedding_vector = embedding_vector.clone();
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

        // Step 2: All validations passed - perform atomic bulk update
        self.store.bulk_update(updates).await.map_err(|e| {
            NodeServiceError::bulk_operation_failed(format!(
                "Failed to execute bulk update transaction: {}",
                e
            ))
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
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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

        // Delete nodes one by one using SurrealStore
        // SurrealDB handles atomicity within each delete operation
        for id in &ids {
            self.store.delete_node(id).await.map_err(|e| {
                NodeServiceError::bulk_operation_failed(format!(
                    "Failed to delete node {}: {}",
                    id, e
                ))
            })?;
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
        _root_id: &str, // Deprecated: hierarchy now managed via edges
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
            self.store.create_node(parent_node).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to create parent node: {}", e))
            })?;
        }

        // Upsert the node (update if exists, create if not)
        if let Some(existing) = self.store.get_node(node_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to check node existence: {}", e))
        })? {
            // Update existing node
            let update = NodeUpdate {
                content: Some(content.to_string()),
                // NOTE: Sibling ordering now handled via has_child edge order field
                ..Default::default()
            };
            self.store
                .update_node(&existing.id, update)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to update node: {}", e))
                })?;
            // Update parent relationship via edge (handles sibling ordering)
            self.store
                .move_node(node_id, Some(parent_id), before_sibling_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to update parent: {}", e))
                })?;
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
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                embedding_vector: None,
            };
            self.store.create_node(node).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to create node: {}", e))
            })?;
            // Create parent relationship via edge (handles sibling ordering)
            self.store
                .move_node(node_id, Some(parent_id), before_sibling_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to set parent: {}", e))
                })?;
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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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

        self.store
            .create_mention(source_id, target_id, source_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to insert mention: {}", e))
            })?;

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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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
        self.store
            .delete_mention(source_id, target_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to delete mention: {}", e))
            })?;

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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
    /// # use nodespace_core::services::node_service::NodeService;
    /// # use nodespace_core::services::database::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = SurrealStore::new(PathBuf::from("./test.db")).await?;
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
        let nodes = self
            .store
            .get_mentioning_containers(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Extract node IDs from the nodes
        Ok(nodes.into_iter().map(|n| n.id).collect())
    }
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

        let store = Arc::new(SurrealStore::new(db_path).await.unwrap());
        let service = NodeService::new(store).unwrap();
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
            json!({"status": "IN_PROGRESS", "priority": "HIGH"}),
        );

        let id = service.create_node(node).await.unwrap();
        let retrieved = service.get_node(&id).await.unwrap().unwrap();

        assert_eq!(retrieved.node_type, "task");
        assert_eq!(retrieved.properties["status"], "IN_PROGRESS");
        assert_eq!(retrieved.properties["priority"], "HIGH");
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
        assert_eq!(date_node.content, "2025-10-13"); // Content MUST match ID for validation
                                                     // Note: Sibling ordering is now on has_child edge order field, not node.before_sibling_id
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

        // Create and persist a date node
        let date_node = Node::new_with_id(
            "2025-10-13".to_string(),
            "date".to_string(),
            "2025-10-13".to_string(),
            json!({"custom": "property"}),
        );

        service.create_node(date_node).await.unwrap();

        // Get the node - should return persisted version with custom property
        let retrieved = service.get_node("2025-10-13").await.unwrap().unwrap();
        assert_eq!(retrieved.properties["custom"], "property");
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
                json!({"status": "OPEN"}),
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
                json!({"status": "OPEN"}),
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
            .move_node(&child1_id, Some(&parent_id))
            .await
            .unwrap();

        let child2 = Node::new("text".to_string(), "Child 2".to_string(), json!({}));
        let child2_id = service.create_node(child2).await.unwrap();
        service
            .move_node(&child2_id, Some(&parent_id))
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
        // Issue #479: Blank content is now valid, so use invalid node type instead
        let invalid_node = Node::new("invalid-type".to_string(), "Content".to_string(), json!({}));

        let nodes = vec![valid_node.clone(), invalid_node];

        // Bulk create should fail (due to invalid node type)
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
    /// Tests for basic node query functionality
    ///
    /// Note: The include_containers_and_tasks filter was removed. These tests now
    /// verify that content search returns ALL matching nodes regardless of type.
    mod container_task_filter_tests {
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
                json!({"status": "OPEN"}),
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
                json!({"task": {"status": "OPEN"}}),
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
                json!({"task": {"status": "OPEN"}}),
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
                json!({"task": {"status": "OPEN"}}),
            );
            let _root_task_id = service.create_node(root_task).await.unwrap();

            let child_task = Node::new_with_id(
                "task-child".to_string(),
                "task".to_string(),
                "Child task".to_string(),
                json!({"task": {"status": "OPEN"}}),
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
                json!({"task": {"status": "OPEN"}}),
            );
            service.create_node(task).await.unwrap();

            // Query with filter = None (default should be false)
            // Use content search to isolate this test's nodes
            let query = crate::models::NodeQuery {
                content_contains: Some("UniqueDefaultTest".to_string()),
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
            service.move_node(&child_id, Some(&root_id)).await.unwrap();

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
            service.move_node(&child1_id, Some(&root_id)).await.unwrap();
            service.move_node(&child2_id, Some(&root_id)).await.unwrap();

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
                json!({"status": "OPEN"}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Make task a child of root
            service.move_node(&task_id, Some(&root_id)).await.unwrap();

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
                json!({"task": {"status": "OPEN"}}),
            );
            let task_id = service.create_node(task).await.unwrap();

            // Establish hierarchy: children belong to their containers
            service
                .move_node(&child1_id, Some(&container1_id))
                .await
                .unwrap();
            service
                .move_node(&child2_id, Some(&container2_id))
                .await
                .unwrap();
            service
                .move_node(&task_id, Some(&container3_id))
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
            service.move_node(&child_id, Some(&root_id)).await.unwrap();

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
                let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));

                let id = service.create_node(node).await.unwrap();
                let retrieved = service.get_node(&id).await.unwrap().unwrap();

                // Verify _schema_version was added
                assert!(retrieved.properties.get("_schema_version").is_some());
                assert_eq!(retrieved.properties["_schema_version"], 1);
            }

            // TODO(#481): Re-enable after SurrealDB migration complete - requires direct SQL access
            #[ignore = "Requires direct SQL access (Issue #481)"]
            #[tokio::test]
            async fn test_backfill_existing_nodes_without_version() {
                // NOTE: Test temporarily disabled - requires direct SQL access to insert nodes without _schema_version
                // This will be re-enabled after SurrealDB migration provides equivalent functionality
                unimplemented!("Requires direct SQL access - deferred to Issue #481");
            }

            // TODO(#481): Re-enable after SurrealDB migration complete - requires direct SQL access
            #[ignore = "Requires direct SQL access (Issue #481)"]
            #[tokio::test]
            async fn test_query_nodes_backfills_versions() {
                // NOTE: Test temporarily disabled - requires direct SQL access to insert nodes without _schema_version
                // This will be re-enabled after SurrealDB migration provides equivalent functionality
                unimplemented!("Requires direct SQL access - deferred to Issue #481");
            }

            #[tokio::test]
            async fn test_auto_created_date_nodes_get_version() {
                let (service, _temp) = create_test_service().await;

                // Directly create a date node (simulating persisted date node)
                // Types without schemas (date, text) get _schema_version via backfill on read
                let date_node = Node::new_with_id(
                    "2025-01-15".to_string(),
                    "date".to_string(),
                    "2025-01-15".to_string(),
                    json!({}),
                );
                service.create_node(date_node).await.unwrap();

                // Retrieve the date node - backfill should add _schema_version
                let retrieved = service.get_node("2025-01-15").await.unwrap().unwrap();
                assert!(
                    retrieved.properties.get("_schema_version").is_some(),
                    "Date nodes should get _schema_version via backfill on read"
                );
                assert_eq!(retrieved.properties["_schema_version"], 1);
            }

            #[tokio::test]
            async fn test_nodes_with_existing_version_not_modified() {
                let (service, _temp) = create_test_service().await;

                // Create a node (will get version 1)
                let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));

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

    mod adjacency_list_tests {
        use super::*;

        // Tests for the adjacency list strategy (recursive graph traversal)
        // Uses SurrealDB's .{..}(->edge->target) syntax for recursive queries

        /// Test get_children_tree with a leaf node (no children)
        #[tokio::test]
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
        async fn test_get_children_tree_single_level() {
            let (service, _temp) = create_test_service().await;

            // Create parent node
            let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
            let parent_id = service.create_node(parent).await.unwrap();

            // Create two children and add to parent using create_parent_edge
            let child1 = Node::new("text".to_string(), "Child 1".to_string(), json!({}));
            let child1_id = service.create_node(child1).await.unwrap();
            service
                .create_parent_edge(&child1_id, &parent_id, None)
                .await
                .unwrap();

            let child2 = Node::new("text".to_string(), "Child 2".to_string(), json!({}));
            let child2_id = service.create_node(child2).await.unwrap();
            service
                .create_parent_edge(&child2_id, &parent_id, None)
                .await
                .unwrap();

            // Get tree - should have parent with 2 children
            let tree = service.get_children_tree(&parent_id).await.unwrap();

            assert_eq!(tree["id"], parent_id);
            let children = tree["children"].as_array().unwrap();
            assert_eq!(children.len(), 2);
            assert_eq!(children[0]["content"], "Child 1");
            assert_eq!(children[1]["content"], "Child 2");
        }

        /// Test get_children_tree with multi-level deep tree
        #[tokio::test]
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
        async fn test_get_children_tree_sibling_ordering() {
            let (service, _temp) = create_test_service().await;

            // Create parent node
            let parent = Node::new("text".to_string(), "Parent".to_string(), json!({}));
            let parent_id = service.create_node(parent).await.unwrap();

            // Add children in order A, B, C - they should maintain this order
            let child_a = Node::new("text".to_string(), "A".to_string(), json!({}));
            let child_a_id = service.create_node(child_a).await.unwrap();
            service
                .create_parent_edge(&child_a_id, &parent_id, None)
                .await
                .unwrap();

            let child_b = Node::new("text".to_string(), "B".to_string(), json!({}));
            let child_b_id = service.create_node(child_b).await.unwrap();
            service
                .create_parent_edge(&child_b_id, &parent_id, None)
                .await
                .unwrap();

            let child_c = Node::new("text".to_string(), "C".to_string(), json!({}));
            let child_c_id = service.create_node(child_c).await.unwrap();
            service
                .create_parent_edge(&child_c_id, &parent_id, None)
                .await
                .unwrap();

            // Get tree - children should be in order A, B, C
            let tree = service.get_children_tree(&parent_id).await.unwrap();

            let children = tree["children"].as_array().unwrap();
            assert_eq!(children.len(), 3);
            assert_eq!(children[0]["content"], "A");
            assert_eq!(children[1]["content"], "B");
            assert_eq!(children[2]["content"], "C");
        }

        /// Test get_children_tree with non-existent root returns empty object
        #[tokio::test]
        async fn test_get_children_tree_nonexistent_root() {
            let (service, _temp) = create_test_service().await;

            // Get tree for non-existent node - should return empty object
            let tree = service.get_children_tree("nonexistent-id").await.unwrap();

            assert!(tree.as_object().unwrap().is_empty());
        }
    }
}
