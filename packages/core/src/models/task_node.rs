//! Strongly-Typed TaskNode
//!
//! Provides compile-time type safety for task nodes with strongly-typed status,
//! priority, and date fields. Uses Universal Graph Architecture.
//!
//! # Architecture
//!
//! **Universal Graph Architecture:**
//! ```text
//! DB node table (node.properties.status, node.properties.priority, etc.)
//!   → Single query from node table
//!   → Properties extracted from node.properties JSON
//!   → Deserialize directly to TaskNode struct with typed enums
//!   → TaskNode.status is a TaskStatus enum field
//! ```
//!
//! # Serialization
//!
//! When serialized (for Tauri/HTTP responses), outputs a flat structure with typed fields:
//! ```json
//! {
//!   "id": "task-123",
//!   "nodeType": "task",
//!   "content": "Implement feature",
//!   "status": "open",
//!   "priority": "medium",
//!   "dueDate": null
//! }
//! ```
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::{TaskNode, TaskStatus, TaskPriority};
//!
//! // Create with builder (for new tasks)
//! let task = TaskNode::builder("Write tests".to_string())
//!     .with_status(TaskStatus::InProgress)
//!     .with_priority(TaskPriority::High)
//!     .build();
//!
//! // Direct field access (no JSON parsing)
//! assert_eq!(task.status, TaskStatus::InProgress);
//! assert_eq!(task.priority, Some(TaskPriority::High));
//! ```

use crate::models::{Node, ValidationError};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::str::FromStr;

/// Parse a date string (YYYY-MM-DD or RFC 3339) into a YYYY-MM-DD string.
/// Returns None if the input is empty or unparseable.
pub(crate) fn parse_to_date_string(s: &str) -> Option<String> {
    // Already YYYY-MM-DD
    if NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() {
        return Some(s.to_string());
    }
    // RFC 3339 / ISO 8601 — extract the date portion
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.format("%Y-%m-%d").to_string());
    }
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Some(dt.format("%Y-%m-%d").to_string());
    }
    None
}

/// Custom deserializer for schema `date` fields.
/// Accepts YYYY-MM-DD or RFC 3339 on input; always stores/returns YYYY-MM-DD.
/// Handles the double-Option pattern: None = field absent (no change), Some(None) = clear.
mod flexible_date {
    use serde::{self, Deserialize, Deserializer};

    pub fn deserialize_with_null<'de, D>(
        deserializer: D,
    ) -> Result<Option<Option<String>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            None => Ok(Some(None)),
            Some(s) => match super::parse_to_date_string(&s) {
                Some(date_str) => Ok(Some(Some(date_str))),
                None => Err(serde::de::Error::custom(format!(
                    "Invalid date format: '{}'. Expected YYYY-MM-DD or ISO8601",
                    s
                ))),
            },
        }
    }
}

/// Task status enumeration
///
/// Represents the lifecycle states of a task node.
/// Values use lowercase format for consistency across all layers:
/// - "open" - Not started (default)
/// - "in_progress" - Currently being worked on
/// - "done" - Finished
/// - "cancelled" - Cancelled/abandoned
/// - User-defined statuses via schema extension (e.g., "blocked", "review")
///
/// Core statuses are strongly typed; user-defined statuses use `User(String)`.
/// This aligns with the schema system's `core_values` / `user_values` model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TaskStatus {
    /// Task has not been started
    #[default]
    Open,
    /// Task is currently being worked on
    InProgress,
    /// Task has been finished
    Done,
    /// Task has been cancelled/abandoned
    Cancelled,
    /// User-defined status (extended via schema)
    User(String),
}

impl FromStr for TaskStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(Self::Open),
            "in_progress" => Ok(Self::InProgress),
            "done" => Ok(Self::Done),
            "cancelled" => Ok(Self::Cancelled),
            // Any other value is treated as user-defined
            other => Ok(Self::User(other.to_string())),
        }
    }
}

impl TaskStatus {
    /// Convert status to string representation
    pub fn as_str(&self) -> &str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::User(s) => s.as_str(),
        }
    }

    /// Check if this is a core (built-in) status
    pub fn is_core(&self) -> bool {
        !matches!(self, Self::User(_))
    }

    /// Check if this is a user-defined status
    pub fn is_user_defined(&self) -> bool {
        matches!(self, Self::User(_))
    }
}

impl Serialize for TaskStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TaskStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_str(&s).unwrap()) // from_str never fails now
    }
}

/// Task priority enumeration
///
/// Represents the priority levels of a task node.
/// Values use lowercase format for consistency across all layers:
/// - "highest" - Highest priority
/// - "high" - High priority
/// - "medium" - Medium priority (default)
/// - "low" - Low priority
/// - "lowest" - Lowest priority
/// - User-defined priorities via schema extension (e.g., "critical", "urgent")
///
/// Core priorities are strongly typed; user-defined priorities use `User(String)`.
/// This aligns with the schema system's `core_values` / `user_values` model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TaskPriority {
    /// Highest priority
    Highest,
    /// High priority
    High,
    /// Medium priority (default)
    #[default]
    Medium,
    /// Low priority
    Low,
    /// Lowest priority
    Lowest,
    /// User-defined priority (extended via schema)
    User(String),
}

impl FromStr for TaskPriority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "highest" => Ok(Self::Highest),
            "high" => Ok(Self::High),
            "medium" => Ok(Self::Medium),
            "low" => Ok(Self::Low),
            "lowest" => Ok(Self::Lowest),
            // Any other value is treated as user-defined
            other => Ok(Self::User(other.to_string())),
        }
    }
}

impl TaskPriority {
    /// Convert priority to string representation
    pub fn as_str(&self) -> &str {
        match self {
            Self::Highest => "highest",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::Lowest => "lowest",
            Self::User(s) => s.as_str(),
        }
    }

    /// Check if this is a core (built-in) priority
    pub fn is_core(&self) -> bool {
        !matches!(self, Self::User(_))
    }

    /// Check if this is a user-defined priority
    pub fn is_user_defined(&self) -> bool {
        matches!(self, Self::User(_))
    }
}

impl Serialize for TaskPriority {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TaskPriority {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_str(&s).unwrap()) // from_str never fails: unknown strings map to User(_)
    }
}

/// Strongly-typed task node with direct field access
///
/// Uses Universal Graph Architecture - properties stored in node.properties JSON.
/// All fields are strongly typed - TaskStatus and TaskPriority enums.
///
/// # Query Pattern (Universal Graph Architecture)
///
/// ```sql
/// SELECT
///     record::id(id) AS id,
///     node_type AS nodeType,
///     properties.status AS status,
///     properties.priority AS priority,
///     properties.due_date AS dueDate,
///     content,
///     version,
///     created_at AS createdAt,
///     modified_at AS modifiedAt
/// FROM node:`some-id`;
/// ```
///
/// When serialized (for Tauri/HTTP responses), outputs a flat structure with typed fields:
/// ```json
/// {
///   "id": "task-123",
///   "nodeType": "task",
///   "content": "Fix bug",
///   "status": "done",
///   "priority": "medium"
/// }
/// ```
///
/// # Examples
///
/// ```rust
/// use nodespace_core::models::{TaskNode, TaskStatus};
///
/// let task = TaskNode::builder("Fix bug".to_string())
///     .with_status(TaskStatus::Done)
///     .build();
///
/// // Direct field access
/// assert_eq!(task.status, TaskStatus::Done);
/// assert_eq!(task.content, "Fix bug");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskNode {
    // ========================================================================
    // Node fields (from node table)
    // ========================================================================
    /// Unique identifier
    pub id: String,

    /// Node type (always "task" for TaskNode)
    #[serde(rename = "nodeType")]
    pub node_type: String,

    /// Primary content/text of the task
    pub content: String,

    /// Optimistic concurrency control version
    #[serde(default = "default_version")]
    pub version: i64,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp
    pub modified_at: DateTime<Utc>,

    /// Properties object (for schema-driven UI compatibility)
    /// Contains task fields for generic Node consumers
    #[serde(default)]
    pub properties: serde_json::Value,

    // ========================================================================
    // Task-specific fields (from node.properties)
    // ========================================================================
    /// Task status (strongly typed enum)
    #[serde(default)]
    pub status: TaskStatus,

    /// Task priority (strongly typed enum: highest, high, medium, low, lowest)
    #[serde(default)]
    pub priority: Option<TaskPriority>,

    /// Due date for the task (YYYY-MM-DD)
    #[serde(default)]
    pub due_date: Option<String>,

    /// Started at date (YYYY-MM-DD, when task moved to in_progress)
    #[serde(default)]
    pub started_at: Option<String>,

    /// Completed at date (YYYY-MM-DD, when task moved to done)
    #[serde(default)]
    pub completed_at: Option<String>,
}

fn default_version() -> i64 {
    1
}

impl TaskNode {
    /// Default priority value
    pub const DEFAULT_PRIORITY: TaskPriority = TaskPriority::Medium;

    /// Create a TaskNode from an existing Node (for backward compatibility)
    ///
    /// This converts the JSON properties pattern to strongly-typed fields.
    /// Prefer using `get_task_node()` from NodeService for direct deserialization.
    ///
    /// # Property Formats
    ///
    /// Supports both property formats:
    /// - Nested format: `properties.task.status`
    /// - Flat format: `properties.status` (used by the markdown importer)
    ///
    /// # Errors
    ///
    /// Returns `ValidationError::InvalidNodeType` if the node type is not "task".
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        if node.node_type != "task" {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected 'task', got '{}'",
                node.node_type
            )));
        }

        // Try nested format first, fall back to flat format (used by the markdown importer)
        let task_props = node
            .properties
            .get("task")
            .and_then(|v| v.as_object())
            .map(|obj| serde_json::Value::Object(obj.clone()));
        let props = task_props.as_ref().unwrap_or(&node.properties);

        // Extract status from properties
        let status: TaskStatus = props
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        // Extract priority from properties (string enum format)
        let priority = props
            .get("priority")
            .and_then(|v| v.as_str())
            .map(|s| TaskPriority::from_str(s).unwrap_or_default());

        // Extract due_date from properties — normalize to YYYY-MM-DD (accept RFC 3339 for migration)
        let due_date = props
            .get("due_date")
            .and_then(|v| v.as_str())
            .and_then(parse_to_date_string);

        // Extract started_at from properties — normalize to YYYY-MM-DD
        let started_at = props
            .get("started_at")
            .and_then(|v| v.as_str())
            .and_then(parse_to_date_string);

        // Extract completed_at from properties — normalize to YYYY-MM-DD
        let completed_at = props
            .get("completed_at")
            .and_then(|v| v.as_str())
            .and_then(parse_to_date_string);

        // Build properties object for schema-driven UI compatibility
        // Use camelCase keys per naming conventions (snake_case in DB, camelCase in JSON API)
        let mut props = serde_json::Map::new();
        props.insert("status".to_string(), json!(status.as_str()));
        if let Some(ref p) = priority {
            props.insert("priority".to_string(), json!(p.as_str()));
        }
        if let Some(ref d) = due_date {
            props.insert("dueDate".to_string(), json!(d));
        }
        if let Some(ref s) = started_at {
            props.insert("startedAt".to_string(), json!(s));
        }
        if let Some(ref c) = completed_at {
            props.insert("completedAt".to_string(), json!(c));
        }
        props.insert("_schemaVersion".to_string(), json!(1));

        Ok(Self {
            id: node.id,
            node_type: "task".to_string(),
            content: node.content,
            version: node.version,
            created_at: node.created_at,
            modified_at: node.modified_at,
            properties: json!(props),
            status,
            priority,
            due_date,
            started_at,
            completed_at,
        })
    }

    /// Create a builder for a new TaskNode with the given content
    pub fn builder(content: String) -> TaskNodeBuilder {
        TaskNodeBuilder {
            content,
            status: None,
            priority: None,
            due_date: None,
        }
    }

    /// Convert to universal Node (for backward compatibility with existing APIs)
    ///
    /// This creates a Node with properties populated from the strongly-typed fields.
    /// Uses snake_case property keys — the canonical on-disk storage form that the
    /// persistence layer writes and that [`TaskNode::from_node`] reads back, so the
    /// domain→Node→domain round trip is lossless.
    pub fn into_node(self) -> Node {
        let mut properties = serde_json::Map::new();
        properties.insert("status".to_string(), json!(self.status.as_str()));

        if let Some(priority) = self.priority {
            properties.insert("priority".to_string(), json!(priority));
        }

        if let Some(due_date) = self.due_date {
            properties.insert("due_date".to_string(), json!(due_date));
        }

        if let Some(started_at) = self.started_at {
            properties.insert("started_at".to_string(), json!(started_at));
        }

        if let Some(completed_at) = self.completed_at {
            properties.insert("completed_at".to_string(), json!(completed_at));
        }

        Node {
            id: self.id,
            node_type: "task".to_string(),
            content: self.content.clone(),
            version: self.version,
            created_at: self.created_at,
            modified_at: self.modified_at,
            properties: json!(properties),
            mentions: Vec::new(),
            mentioned_in: Vec::new(),
            title: Some(crate::utils::strip_markdown(&self.content)), // Task nodes have indexed titles
            lifecycle_status: "active".to_string(),
        }
    }

    /// Get a reference as Node (creates a temporary Node for compatibility)
    ///
    /// Note: This is less efficient than direct field access. Prefer using
    /// the strongly-typed fields directly when possible.
    pub fn as_node(&self) -> Node {
        self.clone().into_node()
    }

    // ========================================================================
    // Convenience methods for backward compatibility
    // ========================================================================

    /// Get the task's status (for API compatibility)
    pub fn status(&self) -> TaskStatus {
        self.status.clone()
    }

    /// Set the task's status
    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
        self.modified_at = Utc::now();
    }

    /// Get the task's priority as TaskPriority enum
    pub fn get_priority(&self) -> TaskPriority {
        self.priority.clone().unwrap_or(Self::DEFAULT_PRIORITY)
    }

    /// Set the task's priority
    pub fn set_priority(&mut self, priority: TaskPriority) {
        self.priority = Some(priority);
        self.modified_at = Utc::now();
    }

    /// Get the task's due date as a YYYY-MM-DD string
    pub fn due_date(&self) -> Option<String> {
        self.due_date.clone()
    }

    /// Set the task's due date from a YYYY-MM-DD or RFC 3339 string
    pub fn set_due_date(&mut self, due_date: Option<&str>) {
        self.due_date = due_date.and_then(parse_to_date_string);
        self.modified_at = Utc::now();
    }
}

/// Builder for creating new TaskNode instances
pub struct TaskNodeBuilder {
    content: String,
    status: Option<TaskStatus>,
    priority: Option<TaskPriority>,
    due_date: Option<String>,
}

impl TaskNodeBuilder {
    /// Set the task status
    pub fn with_status(mut self, status: TaskStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Set the task priority
    pub fn with_priority(mut self, priority: TaskPriority) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Set the task due date from a YYYY-MM-DD or RFC 3339 string
    pub fn with_due_date(mut self, due_date: &str) -> Self {
        self.due_date = parse_to_date_string(due_date);
        self
    }

    /// Set the task due date from string (alias for with_due_date)
    pub fn with_due_date_str(mut self, due_date: &str) -> Self {
        self.due_date = parse_to_date_string(due_date);
        self
    }

    /// Build the TaskNode
    pub fn build(self) -> TaskNode {
        let now = Utc::now();
        let id = uuid::Uuid::new_v4().to_string();

        let status = self.status.unwrap_or_default();

        // Build properties object for schema-driven UI compatibility
        // Use camelCase keys per naming conventions (snake_case in DB, camelCase in JSON API)
        let mut props = serde_json::Map::new();
        props.insert("status".to_string(), json!(status.as_str()));
        if let Some(p) = &self.priority {
            props.insert("priority".to_string(), json!(p.as_str()));
        }
        if let Some(d) = &self.due_date {
            props.insert("dueDate".to_string(), json!(d));
        }
        props.insert("_schemaVersion".to_string(), json!(1));

        TaskNode {
            id,
            node_type: "task".to_string(),
            content: self.content,
            version: 1,
            created_at: now,
            modified_at: now,
            properties: json!(props),
            status,
            priority: self.priority,
            due_date: self.due_date,
            started_at: None,
            completed_at: None,
        }
    }
}

/// Partial update structure for task nodes
///
/// Supports updating task-specific fields (status, priority, due_date)
/// as well as content. Uses Option for each field to enable partial updates.
///
/// # Double-Option Pattern
///
/// Some fields use double-Option to distinguish between:
/// - `None` - Don't change this field
/// - `Some(None)` - Set the field to NULL
/// - `Some(Some(value))` - Set to specific value
///
/// # Examples
///
/// ```rust
/// use nodespace_core::models::{TaskNodeUpdate, TaskStatus};
///
/// // Update only status
/// let update = TaskNodeUpdate::new().with_status(TaskStatus::InProgress);
///
/// // Update status and clear due date
/// let update = TaskNodeUpdate::new()
///     .with_status(TaskStatus::Done)
///     .with_due_date(None);  // Clears the due date
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskNodeUpdate {
    /// Update task status (task property)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,

    /// Update task priority (task property)
    /// - `None` - Don't change
    /// - `Some(None)` - Clear priority
    /// - `Some(Some(p))` - Set to priority p (highest, high, medium, low, lowest, or user-defined)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Option<TaskPriority>>,

    /// Update due date (task property)
    /// - `None` - Don't change
    /// - `Some(None)` - Clear due date
    /// - `Some(Some(s))` - Set to YYYY-MM-DD date (also accepts RFC 3339, normalised on write)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub due_date: Option<Option<String>>,

    /// Update started_at date (task property)
    /// - `None` - Don't change
    /// - `Some(None)` - Clear started_at
    /// - `Some(Some(s))` - Set to YYYY-MM-DD date (also accepts RFC 3339, normalised on write)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub started_at: Option<Option<String>>,

    /// Update completed_at date (task property)
    /// - `None` - Don't change
    /// - `Some(None)` - Clear completed_at
    /// - `Some(Some(s))` - Set to YYYY-MM-DD date (also accepts RFC 3339, normalised on write)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub completed_at: Option<Option<String>>,

    /// Update content (node field)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl TaskNodeUpdate {
    /// Create a new empty update
    pub fn new() -> Self {
        Self::default()
    }

    /// Set status update
    pub fn with_status(mut self, status: TaskStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Set priority update (Some(value) to set, None to clear)
    pub fn with_priority(mut self, priority: Option<TaskPriority>) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Set due date update (Some(value) to set, None to clear).
    /// Accepts YYYY-MM-DD or RFC 3339; normalizes to YYYY-MM-DD.
    pub fn with_due_date(mut self, due_date: Option<&str>) -> Self {
        self.due_date = Some(due_date.and_then(parse_to_date_string));
        self
    }

    /// Set content update
    pub fn with_content(mut self, content: String) -> Self {
        self.content = Some(content);
        self
    }

    /// Check if the update contains any changes
    pub fn is_empty(&self) -> bool {
        self.status.is_none()
            && self.priority.is_none()
            && self.due_date.is_none()
            && self.started_at.is_none()
            && self.completed_at.is_none()
            && self.content.is_none()
    }

    /// Check if this update contains task property fields
    pub fn has_property_fields(&self) -> bool {
        self.status.is_some()
            || self.priority.is_some()
            || self.due_date.is_some()
            || self.started_at.is_some()
            || self.completed_at.is_some()
    }

    /// Check if this update contains content field
    pub fn has_content_field(&self) -> bool {
        self.content.is_some()
    }

    /// Merge this update's task-property fields into a node's namespaced
    /// `properties` value (the `{ "task": { ... } }` object).
    ///
    /// Shared by the write path (`SqliteStore::update_task_node`, which persists
    /// the result) and the title-computation path (`NodeService::update_task_node`,
    /// which needs the post-merge node to recompute a `title_template`-driven
    /// title) so the two can never drift. Content is deliberately not handled here
    /// — it's a node column, not a task property. No-op if `properties` (or its
    /// `task` entry) isn't a JSON object.
    pub fn apply_to_properties(&self, properties: &mut serde_json::Value) {
        let Some(root) = properties.as_object_mut() else {
            return;
        };
        let Some(task) = root
            .entry("task")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
        else {
            return;
        };

        if let Some(ref status) = self.status {
            task.insert("status".to_string(), serde_json::json!(status.as_str()));
        }
        if let Some(ref priority_opt) = self.priority {
            match priority_opt {
                Some(p) => {
                    task.insert("priority".to_string(), serde_json::json!(p.as_str()));
                }
                None => {
                    task.remove("priority");
                }
            }
        }
        if let Some(ref due_date_opt) = self.due_date {
            match due_date_opt {
                Some(s) => {
                    task.insert("due_date".to_string(), serde_json::json!(s));
                }
                None => {
                    task.remove("due_date");
                }
            }
        }
        if let Some(ref started_at_opt) = self.started_at {
            match started_at_opt {
                Some(s) => {
                    task.insert("started_at".to_string(), serde_json::json!(s));
                }
                None => {
                    task.remove("started_at");
                }
            }
        }
        if let Some(ref completed_at_opt) = self.completed_at {
            match completed_at_opt {
                Some(s) => {
                    task.insert("completed_at".to_string(), serde_json::json!(s));
                }
                None => {
                    task.remove("completed_at");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_node_update_null_started_at() {
        // This is the JSON sent when user clicks to clear a date
        let json = r#"{"startedAt": null}"#;
        let update: TaskNodeUpdate = serde_json::from_str(json).unwrap();

        // started_at should be Some(None) to indicate "clear this field"
        // NOT None which means "don't change this field"
        assert!(
            update.started_at.is_some(),
            "started_at should be Some(None) for null value, but got None"
        );
        assert!(
            update.started_at.clone().unwrap().is_none(),
            "Inner value should be None (clear the field)"
        );

        // is_empty() should return false because we're explicitly clearing the field
        assert!(
            !update.is_empty(),
            "Update should NOT be empty when clearing started_at"
        );
    }

    #[test]
    fn test_task_node_update_absent_started_at() {
        // When field is absent, it should be None (don't change)
        let json = r#"{}"#;
        let update: TaskNodeUpdate = serde_json::from_str(json).unwrap();

        assert!(
            update.started_at.is_none(),
            "started_at should be None when absent"
        );
        assert!(
            update.is_empty(),
            "Empty JSON should result in empty update"
        );
    }
}

/// Round-trip property test for the `TaskNode` domain type.
///
/// `TaskNode` is the authoritative in-memory model for `task` nodes: it owns the
/// `from_node` (parse) and `into_node` (serialize) conversions the daemon relies
/// on. If a field is added to the struct but forgotten in either direction — or
/// the two directions disagree on a property key — the field is silently dropped
/// on the next write. This proptest turns that silent drop into a test failure by
/// asserting `from_node(task.into_node())` recovers every typed field: the
/// domain→storage→domain round trip.
#[cfg(test)]
mod roundtrip_proptests {
    use super::*;
    use proptest::prelude::*;

    fn task_status() -> impl Strategy<Value = TaskStatus> {
        prop_oneof![
            Just(TaskStatus::Open),
            Just(TaskStatus::InProgress),
            Just(TaskStatus::Done),
            Just(TaskStatus::Cancelled),
            "[a-z][a-z_]{0,15}".prop_map(TaskStatus::User),
        ]
    }

    fn task_priority() -> impl Strategy<Value = Option<TaskPriority>> {
        prop_oneof![
            Just(None),
            Just(Some(TaskPriority::Highest)),
            Just(Some(TaskPriority::High)),
            Just(Some(TaskPriority::Medium)),
            Just(Some(TaskPriority::Low)),
            Just(Some(TaskPriority::Lowest)),
            "[a-z][a-z_]{0,15}".prop_map(|s| Some(TaskPriority::User(s))),
        ]
    }

    /// Optional `YYYY-MM-DD` date string — the normalized on-disk form.
    fn opt_date() -> impl Strategy<Value = Option<String>> {
        prop_oneof![
            Just(None),
            (2000u32..2100, 1u32..=12, 1u32..=28)
                .prop_map(|(y, m, d)| Some(format!("{:04}-{:02}-{:02}", y, m, d))),
        ]
    }

    proptest! {
        /// `from_node(task.into_node())` recovers every typed field.
        #[test]
        fn task_node_survives_node_round_trip(
            content in "[ -~]{0,40}",
            status in task_status(),
            priority in task_priority(),
            due_date in opt_date(),
            started_at in opt_date(),
            completed_at in opt_date(),
        ) {
            let original = TaskNode {
                id: "task-roundtrip".to_string(),
                node_type: "task".to_string(),
                content: content.clone(),
                version: 1,
                created_at: Utc::now(),
                modified_at: Utc::now(),
                properties: serde_json::json!({}),
                status: status.clone(),
                priority: priority.clone(),
                due_date: due_date.clone(),
                started_at: started_at.clone(),
                completed_at: completed_at.clone(),
            };

            let recovered = TaskNode::from_node(original.clone().into_node())
                .expect("round trip through Node should re-parse as a task");

            prop_assert_eq!(&recovered.content, &content);
            prop_assert_eq!(&recovered.status, &status);
            prop_assert_eq!(&recovered.priority, &priority);
            prop_assert_eq!(&recovered.due_date, &due_date);
            prop_assert_eq!(&recovered.started_at, &started_at);
            prop_assert_eq!(&recovered.completed_at, &completed_at);
        }
    }
}
