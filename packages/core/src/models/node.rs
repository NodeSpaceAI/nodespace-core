//! Node Data Structures
//!
//! This module defines the core `Node` struct and related types for NodeSpace's
//! Pure JSON universal node system.
//!
//! # Architecture
//!
//! - **Universal Node**: Single struct represents all content types
//! - **Pure JSON Schema**: All entity-specific data in `properties` field
//! - **Schema-as-Node**: Schemas stored as nodes with `node_type = "schema"`
//! - **Zero Migration Risk**: No ALTER TABLE required for new entity types
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::Node;
//! use serde_json::json;
//!
//! // Create a text node
//! let text_node = Node::new(
//!     "text".to_string(),
//!     "My first note".to_string(),
//!     None,
//!     json!({}),
//! );
//!
//! // Create a task node with properties
//! let task_node = Node::new(
//!     "task".to_string(),
//!     "Implement Pure JSON schema".to_string(),
//!     Some("2025-01-03".to_string()),
//!     json!({
//!         "status": "IN_PROGRESS",
//!         "priority": "high",
//!         "due_date": "2025-01-10"
//!     }),
//! );
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Default version value for serde deserialization (version 1)
fn default_version() -> i64 {
    1
}

/// Validation errors for Node operations
#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid node type: {0}")]
    InvalidNodeType(String),

    #[error("Invalid node ID format: {0}")]
    InvalidId(String),

    #[error("Invalid parent reference: {0}")]
    InvalidParent(String),

    #[error("Invalid root reference: {0}")]
    InvalidRoot(String),

    #[error("Properties validation failed: {0}")]
    InvalidProperties(String),
}

/// Universal Node structure for all content types in NodeSpace.
///
/// # Fields
///
/// - `id`: Unique identifier (UUID for most nodes, `YYYY-MM-DD` for date nodes)
/// - `node_type`: Type identifier (e.g., "text", "task", "person", "date", "schema")
/// - `content`: Primary content/text of the node
/// - `parent_id`: Optional reference to parent node (creation context)
/// - `container_node_id`: Optional reference to container node (NULL means this node IS a container)
/// - `before_sibling_id`: Optional reference for sibling ordering
/// - `created_at`: Timestamp when node was created
/// - `modified_at`: Timestamp when node was last modified
/// - `properties`: JSON object containing all entity-specific fields
/// - `embedding_vector`: Optional vector embedding for AI semantic search
///
/// # Pure JSON Schema Pattern
///
/// ALL entity-specific data is stored in the `properties` field. This eliminates
/// the need for complementary tables and enables zero-migration schema evolution.
///
/// # Examples
///
/// ```rust
/// # use nodespace_core::models::Node;
/// # use serde_json::json;
/// // Task node with properties
/// let task = Node::new(
///     "task".to_string(),
///     "Write documentation".to_string(),
///     Some("2025-01-03".to_string()),
///     json!({
///         "status": "todo",
///         "assignee": "person-uuid-123",
///         "due_date": "2025-01-10",
///         "priority": "high"
///     }),
/// );
///
/// // Date node with deterministic ID
/// let date = Node::new_with_id(
///     "2025-01-03".to_string(),
///     "date".to_string(),
///     "2025-01-03".to_string(),
///     None,
///     json!({
///         "timezone": "UTC",
///         "is_holiday": false
///     }),
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    /// Unique identifier (UUID or deterministic like YYYY-MM-DD for dates)
    pub id: String,

    /// Node type (e.g., "text", "task", "person", "date", "schema")
    pub node_type: String,

    /// Primary content/text of the node
    pub content: String,

    /// Parent node ID (creation context, not ownership)
    pub parent_id: Option<String>,

    /// Container node ID (NULL means this node IS a container/root)
    pub container_node_id: Option<String>,

    /// Sibling ordering reference (single-pointer linked list)
    pub before_sibling_id: Option<String>,

    /// Optimistic concurrency control version (incremented on each update)
    /// Used to detect conflicting concurrent writes from MCP clients and Frontend UI
    #[serde(default = "default_version")]
    pub version: i64,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp
    pub modified_at: DateTime<Utc>,

    /// All entity-specific fields (Pure JSON schema)
    pub properties: serde_json::Value,

    /// Optional vector embedding for semantic search (F32 blob)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_vector: Option<Vec<u8>>,

    /// Outgoing mentions - IDs of nodes that THIS node references
    /// Example: If this node's content includes "@node-123", then mentions = ["node-123"]
    /// Stored in node_mentions table as (this.id, mentioned_node_id)
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,

    /// Incoming mentions - IDs of nodes that reference THIS node (backlinks)
    /// Example: If node-456 mentions this node, then mentioned_by = ["node-456"]
    /// Computed from node_mentions table WHERE mentions_node_id = this.id
    /// Read-only field, populated on query
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mentioned_by: Vec<String>,
}

impl Node {
    /// Create a new Node with auto-generated UUID
    ///
    /// # Arguments
    ///
    /// * `node_type` - Type identifier (e.g., "text", "task")
    /// * `content` - Primary content/text
    /// * `parent_id` - Optional parent node reference (creation context)
    /// * `properties` - JSON object with entity-specific fields
    ///
    /// # Note on `container_node_id`
    ///
    /// This constructor sets `container_node_id = parent_id`, which is correct for:
    /// - Root nodes: `parent_id = None` → `container_node_id = None` (node IS root)
    /// - Direct children of root: `parent_id = Some(root)` → `container_node_id = Some(root)`
    ///
    /// For nested hierarchies (child of child), you should use `new_with_root()`
    /// to explicitly specify the root document ID.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// // Create a root node
    /// let root = Node::new(
    ///     "text".to_string(),
    ///     "Hello World".to_string(),
    ///     None,
    ///     json!({}),
    /// );
    ///
    /// // Create a direct child of root
    /// let child = Node::new(
    ///     "text".to_string(),
    ///     "Child content".to_string(),
    ///     Some(root.id.clone()),
    ///     json!({}),
    /// );
    /// ```
    pub fn new(
        node_type: String,
        content: String,
        parent_id: Option<String>,
        properties: serde_json::Value,
    ) -> Self {
        let now = Utc::now();
        let id = Uuid::new_v4().to_string();

        // IMPORTANT: container_node_id defaults to parent_id for simple hierarchies.
        // This is correct for root nodes and direct children of root.
        // For nested hierarchies, use new_with_root() instead.
        let container_node_id = parent_id.clone();

        Self {
            id,
            node_type,
            content,
            parent_id,
            container_node_id,
            before_sibling_id: None,
            version: 1,
            created_at: now,
            modified_at: now,
            properties,
            embedding_vector: None,
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
        }
    }

    /// Create a new Node with auto-generated UUID and explicit container_node_id
    ///
    /// Use this constructor for nested hierarchies where the root document
    /// is different from the immediate parent.
    ///
    /// # Arguments
    ///
    /// * `node_type` - Type identifier (e.g., "text", "task")
    /// * `content` - Primary content/text
    /// * `parent_id` - Optional parent node reference (creation context)
    /// * `container_node_id` - Optional root document reference (NULL = this node IS root)
    /// * `properties` - JSON object with entity-specific fields
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// // Create nested hierarchy: root -> child -> grandchild
    /// let container_node_id = "root-uuid".to_string();
    /// let child_id = "child-uuid".to_string();
    ///
    /// let grandchild = Node::new_with_root(
    ///     "text".to_string(),
    ///     "Grandchild content".to_string(),
    ///     Some(child_id),           // Parent is the child
    ///     Some(container_node_id.clone()),    // Root is the root document
    ///     json!({}),
    /// );
    /// ```
    pub fn new_with_root(
        node_type: String,
        content: String,
        parent_id: Option<String>,
        container_node_id: Option<String>,
        properties: serde_json::Value,
    ) -> Self {
        let now = Utc::now();
        let id = Uuid::new_v4().to_string();

        Self {
            id,
            node_type,
            content,
            parent_id,
            container_node_id,
            before_sibling_id: None,
            version: 1,
            created_at: now,
            modified_at: now,
            properties,
            embedding_vector: None,
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
        }
    }

    /// Create a new Node with specified ID (for deterministic IDs like dates)
    ///
    /// # Arguments
    ///
    /// * `id` - Explicit ID (e.g., "2025-01-03" for date nodes)
    /// * `node_type` - Type identifier
    /// * `content` - Primary content/text
    /// * `parent_id` - Optional parent node reference
    /// * `properties` - JSON object with entity-specific fields
    ///
    /// # Note on `container_node_id`
    ///
    /// Like `new()`, this sets `container_node_id = parent_id`. For explicit container_node_id
    /// control, construct the Node directly using struct initialization.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// let date_node = Node::new_with_id(
    ///     "2025-01-03".to_string(),
    ///     "date".to_string(),
    ///     "2025-01-03".to_string(),
    ///     None,
    ///     json!({ "timezone": "UTC" }),
    /// );
    /// ```
    pub fn new_with_id(
        id: String,
        node_type: String,
        content: String,
        parent_id: Option<String>,
        properties: serde_json::Value,
    ) -> Self {
        let now = Utc::now();
        let container_node_id = parent_id.clone();

        Self {
            id,
            node_type,
            content,
            parent_id,
            container_node_id,
            before_sibling_id: None,
            version: 1,
            created_at: now,
            modified_at: now,
            properties,
            embedding_vector: None,
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
        }
    }

    /// Validate node structure and required fields
    ///
    /// # Note
    ///
    /// Content is allowed to be empty (Issue #484). Blank nodes are valid
    /// during editing and are created when users press Enter.
    ///
    /// # Errors
    ///
    /// Returns `ValidationError` if:
    /// - `id` is empty
    /// - `node_type` is empty
    /// - `properties` is not a JSON object
    /// - Node references itself as parent, root, or sibling (circular reference)
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// let node = Node::new(
    ///     "text".to_string(),
    ///     "Valid content".to_string(),
    ///     None,
    ///     json!({}),
    /// );
    /// assert!(node.validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.id.is_empty() {
            return Err(ValidationError::MissingField("id".to_string()));
        }

        if self.node_type.is_empty() {
            return Err(ValidationError::MissingField("node_type".to_string()));
        }

        // Issue #484: Allow blank content for text nodes (users can create blank nodes during editing)
        // This is valid behavior - blank nodes are created by pressing Enter and are
        // intended to be filled in by the user or deleted if left empty.
        // Related to Issue #479: Phase 1 - Ephemeral node elimination

        if !self.properties.is_object() {
            return Err(ValidationError::InvalidProperties(
                "properties must be a JSON object".to_string(),
            ));
        }

        // Validate no circular references (self-referencing)
        if let Some(parent_id) = &self.parent_id {
            if parent_id == &self.id {
                return Err(ValidationError::InvalidParent(
                    "Node cannot be its own parent".to_string(),
                ));
            }
        }

        if let Some(container_node_id) = &self.container_node_id {
            if container_node_id == &self.id {
                return Err(ValidationError::InvalidRoot(
                    "Node cannot be its own root".to_string(),
                ));
            }
        }

        if let Some(sibling_id) = &self.before_sibling_id {
            if sibling_id == &self.id {
                return Err(ValidationError::InvalidParent(
                    "Node cannot be its own sibling".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Check if this node is a root node (no parent or container_node_id is None)
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// let root = Node::new("text".to_string(), "Root".to_string(), None, json!({}));
    /// assert!(root.is_root());
    ///
    /// let child = Node::new(
    ///     "text".to_string(),
    ///     "Child".to_string(),
    ///     Some("parent-id".to_string()),
    ///     json!({})
    /// );
    /// assert!(!child.is_root());
    /// ```
    pub fn is_root(&self) -> bool {
        self.container_node_id.is_none()
    }

    /// Update the node's content
    pub fn set_content(&mut self, content: String) {
        self.content = content;
        self.modified_at = Utc::now();
    }

    /// Update the node's properties
    pub fn set_properties(&mut self, properties: serde_json::Value) {
        self.properties = properties;
        self.modified_at = Utc::now();
    }

    /// Merge properties with existing properties (shallow merge)
    pub fn merge_properties(&mut self, updates: serde_json::Value) {
        if let (Some(existing), Some(new)) = (self.properties.as_object_mut(), updates.as_object())
        {
            for (key, value) in new {
                existing.insert(key.clone(), value.clone());
            }
            self.modified_at = Utc::now();
        }
    }

    /// Set the embedding vector
    pub fn set_embedding(&mut self, embedding: Vec<u8>) {
        self.embedding_vector = Some(embedding);
        self.modified_at = Utc::now();
    }
}

/// Custom deserializer for optional fields that accepts both plain values and nested Options
///
/// This enables TypeScript frontend to send `{"beforeSiblingId": "value"}` instead of
/// requiring the more complex `{"beforeSiblingId": {"value": "value"}}` structure.
///
/// Maps three input formats to the double-Option pattern:
/// - Missing field → None (don't update)
/// - null → Some(None) (set to NULL)
/// - "value" → Some(Some("value")) (set to value)
fn deserialize_optional_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    // Accept either T or Option<T> from JSON
    // Missing field is handled by #[serde(default)] on the struct field
    Ok(Some(Option::<T>::deserialize(deserializer)?))
}

/// Partial node update structure for PATCH operations
///
/// All fields are optional to support partial updates. Only provided fields
/// will be updated in the database.
///
/// # Double-Option Pattern for Nullable Fields
///
/// Fields like `parent_id`, `container_node_id`, `before_sibling_id`, and `embedding_vector`
/// use a double-`Option` pattern to distinguish between three states:
///
/// - `None`: Don't change this field (omit from update)
/// - `Some(None)`: Set the field to NULL (remove the reference)
/// - `Some(Some(value))`: Set the field to the specified value
///
/// This pattern is essential for PATCH operations where you need to distinguish
/// between "don't update" and "set to NULL".
///
/// # Examples
///
/// ```rust
/// # use nodespace_core::models::NodeUpdate;
/// # use serde_json::json;
/// // Update only content (don't touch parent_id)
/// let update = NodeUpdate {
///     content: Some("Updated content".to_string()),
///     ..Default::default()
/// };
///
/// // Update content and clear parent_id (set to NULL)
/// let update = NodeUpdate {
///     content: Some("Orphaned content".to_string()),
///     parent_id: Some(None),  // Set parent_id to NULL
///     ..Default::default()
/// };
///
/// // Update content and set new parent_id
/// let update = NodeUpdate {
///     content: Some("New content".to_string()),
///     parent_id: Some(Some("new-parent-id".to_string())),  // Set to specific value
///     properties: Some(json!({"status": "DONE"})),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeUpdate {
    /// Update node type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,

    /// Update primary content
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Update parent reference
    ///
    /// Uses double-Option pattern:
    /// - `None`: Don't change parent_id
    /// - `Some(None)`: Set parent_id to NULL (remove parent)
    /// - `Some(Some(id))`: Set parent_id to the specified ID
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_field"
    )]
    pub parent_id: Option<Option<String>>,

    /// Update root reference
    ///
    /// Uses double-Option pattern:
    /// - `None`: Don't change container_node_id
    /// - `Some(None)`: Set container_node_id to NULL (this node becomes root)
    /// - `Some(Some(id))`: Set container_node_id to the specified ID
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_field"
    )]
    pub container_node_id: Option<Option<String>>,

    /// Update sibling ordering
    ///
    /// Uses double-Option pattern:
    /// - `None`: Don't change before_sibling_id
    /// - `Some(None)`: Set before_sibling_id to NULL (no explicit ordering)
    /// - `Some(Some(id))`: Set before_sibling_id to the specified ID
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_field"
    )]
    pub before_sibling_id: Option<Option<String>>,

    /// Update or merge properties
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<serde_json::Value>,

    /// Update embedding vector
    ///
    /// Uses double-Option pattern:
    /// - `None`: Don't change embedding_vector
    /// - `Some(None)`: Set embedding_vector to NULL (remove embedding)
    /// - `Some(Some(vec))`: Set embedding_vector to the specified bytes
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_field"
    )]
    pub embedding_vector: Option<Option<Vec<u8>>>,
}

impl NodeUpdate {
    /// Create a new empty NodeUpdate
    pub fn new() -> Self {
        Self::default()
    }

    /// Set content update
    pub fn with_content(mut self, content: String) -> Self {
        self.content = Some(content);
        self
    }

    /// Set properties update
    pub fn with_properties(mut self, properties: serde_json::Value) -> Self {
        self.properties = Some(properties);
        self
    }

    /// Set node type update
    pub fn with_node_type(mut self, node_type: String) -> Self {
        self.node_type = Some(node_type);
        self
    }

    /// Check if update contains any changes
    pub fn is_empty(&self) -> bool {
        self.node_type.is_none()
            && self.content.is_none()
            && self.parent_id.is_none()
            && self.container_node_id.is_none()
            && self.before_sibling_id.is_none()
            && self.properties.is_none()
            && self.embedding_vector.is_none()
    }
}

/// Result of a delete operation
///
/// Provides metadata about the delete operation while maintaining idempotence.
/// The operation always succeeds (returns Ok), but provides visibility into
/// whether the node actually existed.
///
/// # Idempotence
///
/// DELETE operations are idempotent per REST/HTTP standards (RFC 7231).
/// Deleting a non-existent resource succeeds - the `existed` field provides
/// debugging/auditing visibility without breaking idempotence.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::models::DeleteResult;
///
/// // Node existed and was deleted
/// let result = DeleteResult { existed: true };
/// assert!(result.existed);
///
/// // Node didn't exist (idempotent success)
/// let result = DeleteResult { existed: false };
/// assert!(!result.existed);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteResult {
    /// Whether the node existed before deletion
    ///
    /// - `true`: Node existed and was deleted
    /// - `false`: Node didn't exist (idempotent no-op)
    pub existed: bool,
}

impl DeleteResult {
    /// Create a DeleteResult indicating the node existed
    pub fn existed() -> Self {
        Self { existed: true }
    }

    /// Create a DeleteResult indicating the node didn't exist
    pub fn not_found() -> Self {
        Self { existed: false }
    }
}

/// Comparison operator for property filters
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FilterOperator {
    /// Equality (=)
    Equals,
    /// Inequality (!=)
    NotEquals,
    /// Greater than (>)
    GreaterThan,
    /// Greater than or equal (>=)
    GreaterThanOrEqual,
    /// Less than (<)
    LessThan,
    /// Less than or equal (<=)
    LessThanOrEqual,
    /// String contains (LIKE %value%)
    Contains,
    /// String starts with (LIKE value%)
    StartsWith,
    /// String ends with (LIKE %value)
    EndsWith,
}

/// Property filter for JSON path queries
///
/// Enables filtering nodes based on values in their `properties` JSON field.
///
/// # JSON Path Format
///
/// Paths must use JSONPath syntax starting with `$`:
/// - `"$.status"` - Top-level property
/// - `"$.metadata.priority"` - Nested property
/// - `"$"` - Root (entire properties object)
///
/// Invalid paths will be rejected by the `new()` constructor.
///
/// # Examples
///
/// ```rust
/// # use nodespace_core::models::{PropertyFilter, FilterOperator};
/// # use serde_json::json;
/// // Filter for tasks with status = "done"
/// let filter = PropertyFilter::new(
///     "$.status".to_string(),
///     FilterOperator::Equals,
///     json!("done"),
/// ).unwrap();
///
/// // Filter for tasks with priority >= "high"
/// let filter = PropertyFilter::new(
///     "$.priority".to_string(),
///     FilterOperator::GreaterThanOrEqual,
///     json!("high"),
/// ).unwrap();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyFilter {
    /// JSON path to the property (e.g., "$.status", "$.metadata.priority")
    pub path: String,

    /// Comparison operator
    pub operator: FilterOperator,

    /// Value to compare against
    pub value: serde_json::Value,
}

impl PropertyFilter {
    /// Create a new PropertyFilter with path validation
    ///
    /// # Arguments
    ///
    /// * `path` - JSONPath string (must start with `$`)
    /// * `operator` - Comparison operator to use
    /// * `value` - Value to compare against
    ///
    /// # Errors
    ///
    /// Returns `ValidationError::InvalidProperties` if:
    /// - Path doesn't start with `$`
    /// - Path contains invalid characters (e.g., `..`)
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::models::{PropertyFilter, FilterOperator};
    /// # use serde_json::json;
    /// // Valid path
    /// let filter = PropertyFilter::new(
    ///     "$.status".to_string(),
    ///     FilterOperator::Equals,
    ///     json!("done"),
    /// );
    /// assert!(filter.is_ok());
    ///
    /// // Invalid path (missing $ prefix)
    /// let filter = PropertyFilter::new(
    ///     "status".to_string(),
    ///     FilterOperator::Equals,
    ///     json!("done"),
    /// );
    /// assert!(filter.is_err());
    /// ```
    pub fn new(
        path: String,
        operator: FilterOperator,
        value: serde_json::Value,
    ) -> Result<Self, ValidationError> {
        // Validate path starts with "$"
        if !path.starts_with('$') {
            return Err(ValidationError::InvalidProperties(format!(
                "JSON path must start with '$': {}",
                path
            )));
        }

        // Validate no consecutive dots (invalid JSONPath)
        if path.contains("..") {
            return Err(ValidationError::InvalidProperties(format!(
                "JSON path contains invalid consecutive dots: {}",
                path
            )));
        }

        // Validate path doesn't end with a dot (incomplete path)
        if path.len() > 1 && path.ends_with('.') {
            return Err(ValidationError::InvalidProperties(format!(
                "JSON path cannot end with '.': {}",
                path
            )));
        }

        Ok(Self {
            path,
            operator,
            value,
        })
    }
}

/// Simple query parameters for basic node queries
///
/// This is a simpler alternative to `NodeFilter` for common query patterns
/// used by the NodeReferenceService. For advanced queries, use `NodeFilter`.
///
/// # Examples
///
/// ```rust
/// # use nodespace_core::models::NodeQuery;
/// // Query by ID
/// let query = NodeQuery {
///     id: Some("node-123".to_string()),
///     ..Default::default()
/// };
///
/// // Query nodes that mention another node
/// let query = NodeQuery {
///     mentioned_by: Some("target-node-id".to_string()),
///     ..Default::default()
/// };
///
/// // Full-text search with limit
/// let query = NodeQuery {
///     content_contains: Some("search term".to_string()),
///     limit: Some(10),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeQuery {
    /// Query by specific node ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Query nodes that mention this node ID (in their mentions array)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentioned_by: Option<String>,

    /// Query nodes by content substring (case-insensitive LIKE)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_contains: Option<String>,

    /// Query by node type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,

    /// Filter to only include referenceable nodes for @mention autocomplete.
    ///
    /// When `true`, applies SQL filter: `(node_type = 'task' OR container_node_id IS NULL)`
    ///
    /// This includes:
    /// - **Task nodes**: `WHERE node_type = 'task'` (tasks are always referenceable)
    /// - **Container/root nodes**: `WHERE container_node_id IS NULL` (top-level documents)
    ///
    /// This excludes:
    /// - Text child nodes (e.g., paragraphs within a document)
    /// - Other non-task children that shouldn't appear in @mention suggestions
    ///
    /// # Use Case
    /// Primarily used by @mention autocomplete to show only nodes that make sense
    /// to reference (containers and tasks), not individual content fragments.
    ///
    /// # Default
    /// When `None`, defaults to `false` (no filtering applied).
    ///
    /// # Note on Interaction with `node_type`
    /// When combined with `node_type: Some("task")`, the filter becomes redundant
    /// since tasks are always included. However, this is harmless and maintains consistent
    /// query behavior across all parameter combinations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_containers_and_tasks: Option<bool>,

    /// Limit number of results
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl NodeQuery {
    /// Create a new empty query
    pub fn new() -> Self {
        Self::default()
    }

    /// Query by ID
    pub fn by_id(id: String) -> Self {
        Self {
            id: Some(id),
            ..Default::default()
        }
    }

    /// Query nodes that mention a specific node
    pub fn mentioned_by(node_id: String) -> Self {
        Self {
            mentioned_by: Some(node_id),
            ..Default::default()
        }
    }

    /// Query by content search
    pub fn content_contains(search: String) -> Self {
        Self {
            content_contains: Some(search),
            ..Default::default()
        }
    }

    /// Query by node type
    pub fn by_type(node_type: String) -> Self {
        Self {
            node_type: Some(node_type),
            ..Default::default()
        }
    }

    /// Set result limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }
}

/// Sort order specification for query results
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum OrderBy {
    /// Sort by creation time, oldest first
    CreatedAsc,
    /// Sort by creation time, newest first
    CreatedDesc,
    /// Sort by modification time, oldest first
    ModifiedAsc,
    /// Sort by modification time, newest first
    ModifiedDesc,
    /// Sort by content alphabetically, A-Z
    ContentAsc,
    /// Sort by content alphabetically, Z-A
    ContentDesc,
    /// Sort by node type alphabetically
    NodeTypeAsc,
    /// Sort by node type reverse alphabetically
    NodeTypeDesc,
}

/// Node filter for query operations
///
/// Supports common query patterns for filtering nodes by various criteria,
/// including temporal ranges, JSON property values, and custom sort orders.
///
/// # Examples
///
/// ```rust
/// # use nodespace_core::models::{NodeFilter, PropertyFilter, FilterOperator, OrderBy};
/// # use serde_json::json;
/// # use chrono::{Utc, Duration};
/// // Filter by node type
/// let filter = NodeFilter::new()
///     .with_node_type("task".to_string());
///
/// // Filter by parent
/// let filter = NodeFilter::new()
///     .with_parent_id("2025-01-03".to_string());
///
/// // Complex filter: tasks created in last 7 days with high priority
/// let week_ago = Utc::now() - Duration::days(7);
/// let filter = NodeFilter::new()
///     .with_node_type("task".to_string())
///     .with_created_after(week_ago)
///     .with_property_filter(PropertyFilter {
///         path: "$.priority".to_string(),
///         operator: FilterOperator::Equals,
///         value: json!("high"),
///     })
///     .with_order_by(OrderBy::CreatedDesc)
///     .with_limit(10);
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeFilter {
    /// Filter by node type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,

    /// Filter by parent ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    /// Filter by root ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container_node_id: Option<String>,

    /// Filter by specific IDs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,

    /// Filter for root nodes only
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_root: Option<bool>,

    /// Filter by content search (substring match)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_contains: Option<String>,

    /// Filter by creation date - nodes created after this time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_after: Option<DateTime<Utc>>,

    /// Filter by creation date - nodes created before this time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_before: Option<DateTime<Utc>>,

    /// Filter by modification date - nodes modified after this time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_after: Option<DateTime<Utc>>,

    /// Filter by modification date - nodes modified before this time
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_before: Option<DateTime<Utc>>,

    /// Filter by JSON property values (can specify multiple)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub property_filters: Option<Vec<PropertyFilter>>,

    /// Sort order specification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_by: Option<OrderBy>,

    /// Limit number of results
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,

    /// Offset for pagination
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

impl NodeFilter {
    /// Create a new empty filter
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by node type
    pub fn with_node_type(mut self, node_type: String) -> Self {
        self.node_type = Some(node_type);
        self
    }

    /// Filter by parent ID
    pub fn with_parent_id(mut self, parent_id: String) -> Self {
        self.parent_id = Some(parent_id);
        self
    }

    /// Filter by root ID
    pub fn with_container_node_id(mut self, container_node_id: String) -> Self {
        self.container_node_id = Some(container_node_id);
        self
    }

    /// Filter by specific IDs
    pub fn with_ids(mut self, ids: Vec<String>) -> Self {
        self.ids = Some(ids);
        self
    }

    /// Filter for root nodes only
    pub fn with_is_root(mut self, is_root: bool) -> Self {
        self.is_root = Some(is_root);
        self
    }

    /// Filter by content substring
    pub fn with_content_contains(mut self, content: String) -> Self {
        self.content_contains = Some(content);
        self
    }

    /// Filter by creation date - nodes created after this time
    pub fn with_created_after(mut self, created_after: DateTime<Utc>) -> Self {
        self.created_after = Some(created_after);
        self
    }

    /// Filter by creation date - nodes created before this time
    pub fn with_created_before(mut self, created_before: DateTime<Utc>) -> Self {
        self.created_before = Some(created_before);
        self
    }

    /// Filter by modification date - nodes modified after this time
    pub fn with_modified_after(mut self, modified_after: DateTime<Utc>) -> Self {
        self.modified_after = Some(modified_after);
        self
    }

    /// Filter by modification date - nodes modified before this time
    pub fn with_modified_before(mut self, modified_before: DateTime<Utc>) -> Self {
        self.modified_before = Some(modified_before);
        self
    }

    /// Add a property filter (can be called multiple times)
    pub fn with_property_filter(mut self, filter: PropertyFilter) -> Self {
        if let Some(ref mut filters) = self.property_filters {
            filters.push(filter);
        } else {
            self.property_filters = Some(vec![filter]);
        }
        self
    }

    /// Set multiple property filters at once
    pub fn with_property_filters(mut self, filters: Vec<PropertyFilter>) -> Self {
        self.property_filters = Some(filters);
        self
    }

    /// Set sort order
    pub fn with_order_by(mut self, order_by: OrderBy) -> Self {
        self.order_by = Some(order_by);
        self
    }

    /// Set result limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set result offset
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_node_creation() {
        let node = Node::new(
            "text".to_string(),
            "Test content".to_string(),
            None,
            json!({}),
        );

        assert!(!node.id.is_empty());
        assert_eq!(node.node_type, "text");
        assert_eq!(node.content, "Test content");
        assert!(node.parent_id.is_none());
        assert!(node.is_root());
    }

    #[test]
    fn test_node_with_deterministic_id() {
        let node = Node::new_with_id(
            "2025-01-03".to_string(),
            "date".to_string(),
            "2025-01-03".to_string(),
            None,
            json!({"timezone": "UTC"}),
        );

        assert_eq!(node.id, "2025-01-03");
        assert_eq!(node.node_type, "date");
        assert!(node.is_root());
    }

    #[test]
    fn test_node_validation() {
        let node = Node::new(
            "text".to_string(),
            "Valid content".to_string(),
            None,
            json!({}),
        );

        assert!(node.validate().is_ok());
    }

    #[test]
    fn test_node_validation_accepts_blank_content() {
        // Issue #484: Blank content is now allowed (supports ephemeral node elimination from #479)
        let mut node = Node::new(
            "text".to_string(),
            "Valid content".to_string(),
            None,
            json!({}),
        );
        node.content = String::new();

        // Should validate successfully - blank nodes are valid
        assert!(node.validate().is_ok());
    }

    #[test]
    fn test_node_validation_invalid_properties() {
        let mut node = Node::new(
            "text".to_string(),
            "Valid content".to_string(),
            None,
            json!({}),
        );
        node.properties = json!("not an object");

        assert!(matches!(
            node.validate(),
            Err(ValidationError::InvalidProperties(_))
        ));
    }

    #[test]
    fn test_node_validation_circular_parent() {
        let mut node = Node::new("text".to_string(), "Test".to_string(), None, json!({}));
        // Set parent_id to self (circular reference)
        node.parent_id = Some(node.id.clone());

        assert!(matches!(
            node.validate(),
            Err(ValidationError::InvalidParent(_))
        ));
    }

    #[test]
    fn test_node_validation_circular_root() {
        let mut node = Node::new("text".to_string(), "Test".to_string(), None, json!({}));
        // Set container_node_id to self (circular reference)
        node.container_node_id = Some(node.id.clone());

        assert!(matches!(
            node.validate(),
            Err(ValidationError::InvalidRoot(_))
        ));
    }

    #[test]
    fn test_node_validation_circular_sibling() {
        let mut node = Node::new("text".to_string(), "Test".to_string(), None, json!({}));
        // Set before_sibling_id to self (circular reference)
        node.before_sibling_id = Some(node.id.clone());

        assert!(matches!(
            node.validate(),
            Err(ValidationError::InvalidParent(_))
        ));
    }

    #[test]
    fn test_node_is_root() {
        let root = Node::new("text".to_string(), "Root".to_string(), None, json!({}));
        assert!(root.is_root());

        let child = Node::new(
            "text".to_string(),
            "Child".to_string(),
            Some("parent-id".to_string()),
            json!({}),
        );
        assert!(!child.is_root());
    }

    #[test]
    fn test_node_new_with_root() {
        let container_node_id = "root-123".to_string();
        let parent_id = "parent-456".to_string();

        let node = Node::new_with_root(
            "text".to_string(),
            "Grandchild".to_string(),
            Some(parent_id.clone()),
            Some(container_node_id.clone()),
            json!({}),
        );

        assert_eq!(node.parent_id, Some(parent_id));
        assert_eq!(node.container_node_id, Some(container_node_id));
        assert!(!node.is_root());
    }

    #[test]
    fn test_node_content_update() {
        let mut node = Node::new("text".to_string(), "Original".to_string(), None, json!({}));
        let original_modified = node.modified_at;

        node.set_content("Updated".to_string());

        assert_eq!(node.content, "Updated");
        // Modified time should be >= original (might be equal on fast systems)
        assert!(node.modified_at >= original_modified);
    }

    #[test]
    fn test_node_properties_update() {
        let mut node = Node::new(
            "task".to_string(),
            "Task".to_string(),
            None,
            json!({"status": "todo"}),
        );

        node.set_properties(json!({"status": "done"}));

        assert_eq!(node.properties["status"], "done");
    }

    #[test]
    fn test_node_properties_merge() {
        let mut node = Node::new(
            "task".to_string(),
            "Task".to_string(),
            None,
            json!({"status": "todo", "priority": "low"}),
        );

        node.merge_properties(json!({"status": "done"}));

        assert_eq!(node.properties["status"], "done");
        assert_eq!(node.properties["priority"], "low"); // Original value preserved
    }

    #[test]
    fn test_node_set_embedding() {
        let mut node = Node::new(
            "text".to_string(),
            "Test content".to_string(),
            None,
            json!({}),
        );

        assert!(node.embedding_vector.is_none());
        let original_modified = node.modified_at;

        // Set embedding vector
        let embedding = vec![1u8, 2u8, 3u8, 4u8];
        node.set_embedding(embedding.clone());

        assert_eq!(node.embedding_vector, Some(embedding));
        // Modified time should be >= original (might be equal on fast systems)
        assert!(node.modified_at >= original_modified);

        // Verify serialization works
        let json = serde_json::to_string(&node).unwrap();
        let deserialized: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(node.embedding_vector, deserialized.embedding_vector);
    }

    #[test]
    fn test_node_update_builder() {
        let update = NodeUpdate::new()
            .with_content("Updated content".to_string())
            .with_properties(json!({"status": "done"}));

        assert_eq!(update.content, Some("Updated content".to_string()));
        assert_eq!(update.properties, Some(json!({"status": "done"})));
        assert!(!update.is_empty());
    }

    #[test]
    fn test_node_update_is_empty() {
        let update = NodeUpdate::new();
        assert!(update.is_empty());

        let update = NodeUpdate::new().with_content("test".to_string());
        assert!(!update.is_empty());
    }

    #[test]
    fn test_node_filter_builder() {
        let filter = NodeFilter::new()
            .with_node_type("task".to_string())
            .with_parent_id("2025-01-03".to_string())
            .with_limit(10);

        assert_eq!(filter.node_type, Some("task".to_string()));
        assert_eq!(filter.parent_id, Some("2025-01-03".to_string()));
        assert_eq!(filter.limit, Some(10));
    }

    #[test]
    fn test_node_filter_temporal() {
        use chrono::Duration;

        let now = Utc::now();
        let week_ago = now - Duration::days(7);
        let tomorrow = now + Duration::days(1);

        let filter = NodeFilter::new()
            .with_created_after(week_ago)
            .with_created_before(tomorrow)
            .with_modified_after(week_ago);

        assert_eq!(filter.created_after, Some(week_ago));
        assert_eq!(filter.created_before, Some(tomorrow));
        assert_eq!(filter.modified_after, Some(week_ago));
        assert!(filter.modified_before.is_none());
    }

    #[test]
    fn test_node_filter_property_filters() {
        let filter1 = PropertyFilter::new(
            "$.status".to_string(),
            FilterOperator::Equals,
            json!("done"),
        )
        .unwrap();

        let filter2 = PropertyFilter::new(
            "$.priority".to_string(),
            FilterOperator::GreaterThan,
            json!("medium"),
        )
        .unwrap();

        let node_filter = NodeFilter::new()
            .with_property_filter(filter1.clone())
            .with_property_filter(filter2.clone());

        assert_eq!(node_filter.property_filters.as_ref().unwrap().len(), 2);
        assert_eq!(
            node_filter.property_filters.as_ref().unwrap()[0].path,
            "$.status"
        );
        assert_eq!(
            node_filter.property_filters.as_ref().unwrap()[1].path,
            "$.priority"
        );
    }

    #[test]
    fn test_node_filter_order_by() {
        let filter = NodeFilter::new()
            .with_order_by(OrderBy::CreatedDesc)
            .with_limit(20);

        assert_eq!(filter.order_by, Some(OrderBy::CreatedDesc));
        assert_eq!(filter.limit, Some(20));
    }

    #[test]
    fn test_property_filter_serialization() {
        let filter = PropertyFilter::new(
            "$.metadata.tags".to_string(),
            FilterOperator::Contains,
            json!("important"),
        )
        .unwrap();

        let json = serde_json::to_string(&filter).unwrap();
        let deserialized: PropertyFilter = serde_json::from_str(&json).unwrap();

        assert_eq!(filter.path, deserialized.path);
        assert_eq!(filter.operator, deserialized.operator);
        assert_eq!(filter.value, deserialized.value);
    }

    #[test]
    fn test_property_filter_valid_paths() {
        // Valid root path
        let filter = PropertyFilter::new("$".to_string(), FilterOperator::Equals, json!({}));
        assert!(filter.is_ok());

        // Valid simple path
        let filter = PropertyFilter::new(
            "$.status".to_string(),
            FilterOperator::Equals,
            json!("done"),
        );
        assert!(filter.is_ok());

        // Valid nested path
        let filter = PropertyFilter::new(
            "$.metadata.priority".to_string(),
            FilterOperator::Equals,
            json!("high"),
        );
        assert!(filter.is_ok());

        // Valid deeply nested path
        let filter = PropertyFilter::new(
            "$.a.b.c.d".to_string(),
            FilterOperator::Equals,
            json!("value"),
        );
        assert!(filter.is_ok());
    }

    #[test]
    fn test_property_filter_invalid_paths() {
        // Missing $ prefix
        let filter =
            PropertyFilter::new("status".to_string(), FilterOperator::Equals, json!("done"));
        assert!(filter.is_err());
        assert!(matches!(filter, Err(ValidationError::InvalidProperties(_))));

        // Consecutive dots
        let filter = PropertyFilter::new(
            "$.status..priority".to_string(),
            FilterOperator::Equals,
            json!("done"),
        );
        assert!(filter.is_err());

        // Trailing dot
        let filter = PropertyFilter::new(
            "$.status.".to_string(),
            FilterOperator::Equals,
            json!("done"),
        );
        assert!(filter.is_err());

        // Empty path after $
        let filter = PropertyFilter::new("$.".to_string(), FilterOperator::Equals, json!("done"));
        assert!(filter.is_err());
    }

    #[test]
    fn test_node_serialization() {
        let node = Node::new(
            "text".to_string(),
            "Test".to_string(),
            None,
            json!({"key": "value"}),
        );

        let json = serde_json::to_string(&node).unwrap();
        let deserialized: Node = serde_json::from_str(&json).unwrap();

        assert_eq!(node.id, deserialized.id);
        assert_eq!(node.node_type, deserialized.node_type);
        assert_eq!(node.content, deserialized.content);
        assert_eq!(node.properties, deserialized.properties);
    }

    #[test]
    fn test_task_node_with_properties() {
        let task = Node::new(
            "task".to_string(),
            "Implement feature".to_string(),
            Some("2025-01-03".to_string()),
            json!({
                "status": "IN_PROGRESS",
                "assignee": "person-123",
                "due_date": "2025-01-10",
                "priority": "HIGH"
            }),
        );

        assert_eq!(task.node_type, "task");
        assert_eq!(task.properties["status"], "IN_PROGRESS");
        assert_eq!(task.properties["priority"], "HIGH");
        assert!(!task.is_root());
    }
}
