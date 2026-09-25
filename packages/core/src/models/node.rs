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
//!     json!({}),
//! );
//!
//! // Create a task node with properties (status uses lowercase format)
//! let task_node = Node::new(
//!     "task".to_string(),
//!     "Implement Pure JSON schema".to_string(),
//!     json!({
//!         "status": "in_progress",
//!         "priority": "high",
//!         "due_date": "2025-01-10"
//!     }),
//! );
//! ```

use serde::{Deserialize, Serialize};

// Wire types are defined once in nodespace-types and re-exported here.
// nodespace-core adds the core-only types below (NodeFilter, NodeRelationship, etc.)
// that are not part of the shared wire surface.
pub use nodespace_types::{
    DeleteResult, Node, NodeQuery, NodeReference, NodeUpdate, OrderBy, ValidationError,
};

/// Direction to traverse an edge relative to a node
///
/// - `Out`: The relationship points FROM this node TO another (outgoing)
/// - `In`: The relationship points FROM another node TO this node (incoming)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraversalDirection {
    /// Outgoing relationship: this node -> other node
    Out,
    /// Incoming relationship: other node -> this node
    In,
}

/// Represents a relationship with another node, including direction
///
/// Used for returning relationships in subtree queries where we need to know
/// both the related node and the direction of the relationship.
///
/// For outgoing relationships (direction = Out):
/// - `id` is the target node ID
/// - The current node points TO this node
///
/// For incoming relationships (direction = In):
/// - `id` is the source node ID
/// - This node points TO the current node
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRelationship {
    /// The related node's ID
    pub id: String,
    /// The related node's title (for display)
    pub title: Option<String>,
    /// Direction of the relationship relative to the node this is attached to
    pub direction: TraversalDirection,
    /// Type of relationship (e.g., "mentions", "has_child", "member_of")
    pub relationship_type: String,
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

/// Node filter for query operations
///
/// Supports common query patterns for filtering nodes by various criteria,
/// including JSON property values and custom sort orders. Date-range
/// querying lives in `QueryService`'s metadata filters, not here.
///
/// # Examples
///
/// ```rust
/// # use nodespace_core::models::{NodeFilter, PropertyFilter, FilterOperator, OrderBy};
/// # use serde_json::json;
/// // Filter by node type
/// let filter = NodeFilter::new()
///     .with_node_type("task".to_string());
///
/// // Complex filter: newest high-priority tasks
/// let filter = NodeFilter::new()
///     .with_node_type("task".to_string())
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

    /// Filter by specific IDs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,

    /// Filter by content search (substring match)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_contains: Option<String>,

    /// Filter by title substring (case-insensitive)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_contains: Option<String>,

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

    /// Filter by specific IDs
    pub fn with_ids(mut self, ids: Vec<String>) -> Self {
        self.ids = Some(ids);
        self
    }

    /// Filter by content substring
    pub fn with_content_contains(mut self, content: String) -> Self {
        self.content_contains = Some(content);
        self
    }

    /// Filter by title substring
    pub fn with_title_contains(mut self, title: String) -> Self {
        self.title_contains = Some(title);
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
        let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));

        assert!(!node.id.is_empty());
        assert_eq!(node.node_type, "text");
        assert_eq!(node.content, "Test content");
    }

    #[test]
    fn test_node_with_deterministic_id() {
        let node = Node::new_with_id(
            "2025-01-03".to_string(),
            "date".to_string(),
            "2025-01-03".to_string(),
            json!({"timezone": "UTC"}),
        );

        assert_eq!(node.id, "2025-01-03");
        assert_eq!(node.node_type, "date");
    }

    #[test]
    fn test_node_validation() {
        let node = Node::new("text".to_string(), "Valid content".to_string(), json!({}));

        assert!(node.validate().is_ok());
    }

    #[test]
    fn test_node_validation_accepts_blank_content() {
        // Blank content is now allowed (supports ephemeral node elimination)
        let mut node = Node::new("text".to_string(), "Valid content".to_string(), json!({}));
        node.content = String::new();

        // Should validate successfully - blank nodes are valid
        assert!(node.validate().is_ok());
    }

    #[test]
    fn test_node_validation_invalid_properties() {
        let mut node = Node::new("text".to_string(), "Valid content".to_string(), json!({}));
        node.properties = json!("not an object");

        assert!(matches!(
            node.validate(),
            Err(ValidationError::InvalidProperties(_))
        ));
    }

    // NOTE: test_node_validation_circular_sibling removed - sibling ordering is now
    // handled via has_child edge order field. Circular sibling references are no longer
    // possible at the node level since before_sibling_id is stored on edges, not nodes.

    #[test]
    fn test_node_content_update() {
        let mut node = Node::new("text".to_string(), "Original".to_string(), json!({}));
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
            json!({"status": "todo", "priority": "low"}),
        );

        node.merge_properties(json!({"status": "done"}));

        assert_eq!(node.properties["status"], "done");
        assert_eq!(node.properties["priority"], "low"); // Original value preserved
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
            .with_limit(10);

        assert_eq!(filter.node_type, Some("task".to_string()));
        assert_eq!(filter.limit, Some(10));
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
        // Status/priority values use lowercase format
        let task = Node::new(
            "task".to_string(),
            "Implement feature".to_string(),
            json!({
                "status": "in_progress",
                "due_date": "2025-01-10",
                "priority": "high"
            }),
        );

        assert_eq!(task.node_type, "task");
        assert_eq!(task.properties["status"], "in_progress");
        assert_eq!(task.properties["priority"], "high");
    }

    // Lifecycle Status Tests

    #[test]
    fn test_node_lifecycle_status_default() {
        // Test that lifecycle_status defaults to "active" for new nodes
        let node = Node::new("text".to_string(), "Test".to_string(), json!({}));
        assert_eq!(node.lifecycle_status, "active");
    }

    #[test]
    fn test_node_lifecycle_status_default_with_id() {
        // Test that lifecycle_status defaults to "active" for nodes with explicit ID
        let node = Node::new_with_id(
            "2025-01-26".to_string(),
            "date".to_string(),
            "2025-01-26".to_string(),
            json!({}),
        );
        assert_eq!(node.lifecycle_status, "active");
    }

    #[test]
    fn test_node_lifecycle_status_serialization_active_skipped() {
        // Test that "active" lifecycle_status is skipped during serialization
        let node = Node::new("text".to_string(), "Test".to_string(), json!({}));
        let json = serde_json::to_string(&node).unwrap();

        // "active" should be skipped (not serialized)
        assert!(!json.contains("lifecycleStatus"));
    }

    #[test]
    fn test_node_lifecycle_status_serialization_archived_included() {
        // Test that non-"active" lifecycle_status is serialized
        let mut node = Node::new("text".to_string(), "Test".to_string(), json!({}));
        node.lifecycle_status = "archived".to_string();

        let json = serde_json::to_string(&node).unwrap();

        // "archived" should be serialized
        assert!(json.contains("lifecycleStatus"));
        assert!(json.contains("archived"));
    }

    #[test]
    fn test_node_lifecycle_status_deserialization_default() {
        // Test that missing lifecycle_status defaults to "active" during deserialization
        let json = r#"{
            "id": "123",
            "nodeType": "text",
            "content": "Test",
            "version": 1,
            "createdAt": "2025-01-26T00:00:00Z",
            "modifiedAt": "2025-01-26T00:00:00Z",
            "properties": {}
        }"#;

        let node: Node = serde_json::from_str(json).unwrap();
        assert_eq!(node.lifecycle_status, "active");
    }

    #[test]
    fn test_node_lifecycle_status_deserialization_archived() {
        // Test that lifecycle_status is properly deserialized when present
        let json = r#"{
            "id": "123",
            "nodeType": "text",
            "content": "Test",
            "version": 1,
            "createdAt": "2025-01-26T00:00:00Z",
            "modifiedAt": "2025-01-26T00:00:00Z",
            "properties": {},
            "lifecycleStatus": "archived"
        }"#;

        let node: Node = serde_json::from_str(json).unwrap();
        assert_eq!(node.lifecycle_status, "archived");
    }

    #[test]
    fn test_node_lifecycle_status_deserialization_deleted() {
        // Test that lifecycle_status "deleted" is properly deserialized
        let json = r#"{
            "id": "123",
            "nodeType": "text",
            "content": "Test",
            "version": 1,
            "createdAt": "2025-01-26T00:00:00Z",
            "modifiedAt": "2025-01-26T00:00:00Z",
            "properties": {},
            "lifecycleStatus": "deleted"
        }"#;

        let node: Node = serde_json::from_str(json).unwrap();
        assert_eq!(node.lifecycle_status, "deleted");
    }
}
