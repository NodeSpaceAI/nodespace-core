//! Strongly-Typed TaskNode
//!
//! Provides compile-time type safety for task nodes with strongly-typed status,
//! priority, and date fields. Uses Universal Graph Architecture (Issue #783).
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
//!   "dueDate": null,
//!   "assigneeId": null
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
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::str::FromStr;

/// Custom deserializer for flexible date parsing
/// Accepts both "YYYY-MM-DD" (date only) and "YYYY-MM-DDTHH:MM:SSZ" (full ISO8601)
mod flexible_date {
    use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
    use serde::{self, Deserialize, Deserializer};

    /// Deserialize with explicit null handling for `#[serde(deserialize_with = "...")]`
    ///
    /// When using `deserialize_with` with `default`, serde calls this function only when
    /// the field is present. This function properly maps JSON `null` to `Some(None)`.
    pub fn deserialize_with_null<'de, D>(
        deserializer: D,
    ) -> Result<Option<Option<DateTime<Utc>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        // When using deserialize_with, we receive the raw value (null or string)
        // wrapped in Option only once (not Option<Option<String>>)
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            None => Ok(Some(None)), // Field is null - clear value (the key insight!)
            Some(s) => parse_date_string(&s).map_err(serde::de::Error::custom),
        }
    }

    /// Parse a date string into DateTime<Utc>
    fn parse_date_string(s: &str) -> Result<Option<Option<DateTime<Utc>>>, String> {
        // Try full ISO8601 first
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Ok(Some(Some(dt.with_timezone(&Utc))));
        }
        // Try "YYYY-MM-DDTHH:MM:SSZ" format
        if let Ok(dt) = s.parse::<DateTime<Utc>>() {
            return Ok(Some(Some(dt)));
        }
        // Try date-only "YYYY-MM-DD" format
        if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            let datetime = date.and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap());
            return Ok(Some(Some(DateTime::from_naive_utc_and_offset(
                datetime, Utc,
            ))));
        }
        Err(format!(
            "Invalid date format: '{}'. Expected YYYY-MM-DD or ISO8601",
            s
        ))
    }
}

/// Task status enumeration
///
/// Represents the lifecycle states of a task node.
/// Values use lowercase format for consistency across all layers (Issue #670):
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
/// - "low" - Low priority
/// - "medium" - Medium priority (default)
/// - "high" - High priority
/// - User-defined priorities via schema extension (e.g., "critical", "urgent")
///
/// Core priorities are strongly typed; user-defined priorities use `User(String)`.
/// This aligns with the schema system's `core_values` / `user_values` model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TaskPriority {
    /// Low priority
    Low,
    /// Medium priority (default)
    #[default]
    Medium,
    /// High priority
    High,
    /// User-defined priority (extended via schema)
    User(String),
}

impl FromStr for TaskPriority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            // Any other value is treated as user-defined
            other => Ok(Self::User(other.to_string())),
        }
    }
}

impl TaskPriority {
    /// Convert priority to string representation
    pub fn as_str(&self) -> &str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
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
        Ok(Self::from_str(&s).unwrap()) // from_str never fails now
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
///     properties.assignee AS assignee,
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

    /// Task priority (strongly typed enum: low, medium, high)
    #[serde(default)]
    pub priority: Option<TaskPriority>,

    /// Due date for the task
    #[serde(default)]
    pub due_date: Option<DateTime<Utc>>,

    /// Assignee node ID
    #[serde(default)]
    pub assignee: Option<String>,

    /// Started at timestamp (when task moved to in_progress)
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,

    /// Completed at timestamp (when task moved to done)
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
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
    /// Supports both property formats (Issue #397):
    /// - New nested format: `properties.task.status`
    /// - Old flat format: `properties.status` (deprecated, for backward compat)
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

        // Try new nested format first, fall back to old flat format (Issue #397)
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

        // Extract due_date from properties (try parsing as DateTime)
        let due_date = props
            .get("due_date")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        // Extract assignee from properties
        // Note: Supports both "assignee_id" (legacy) and "assignee" (canonical) field names.
        // The struct field is `assignee`, and `into_node()` serializes as "assignee".
        // We read both for backward compatibility with any older data using "assignee_id".
        let assignee = props
            .get("assignee_id")
            .or_else(|| props.get("assignee"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Extract started_at from properties
        let started_at = props
            .get("started_at")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        // Extract completed_at from properties
        let completed_at = props
            .get("completed_at")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        // Build properties object for schema-driven UI compatibility
        // Use camelCase keys per naming conventions (snake_case in DB, camelCase in JSON API)
        let mut props = serde_json::Map::new();
        props.insert("status".to_string(), json!(status.as_str()));
        if let Some(ref p) = priority {
            props.insert("priority".to_string(), json!(p.as_str()));
        }
        if let Some(ref d) = due_date {
            props.insert("dueDate".to_string(), json!(d.to_rfc3339()));
        }
        if let Some(ref a) = assignee {
            props.insert("assignee".to_string(), json!(a));
        }
        if let Some(ref s) = started_at {
            props.insert("startedAt".to_string(), json!(s.to_rfc3339()));
        }
        if let Some(ref c) = completed_at {
            props.insert("completedAt".to_string(), json!(c.to_rfc3339()));
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
            assignee,
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
            assignee: None,
        }
    }

    /// Convert to universal Node (for backward compatibility with existing APIs)
    ///
    /// This creates a Node with properties populated from the strongly-typed fields.
    /// Uses camelCase keys per naming conventions (snake_case in DB, camelCase in JSON API).
    pub fn into_node(self) -> Node {
        let mut properties = serde_json::Map::new();
        properties.insert("status".to_string(), json!(self.status.as_str()));

        if let Some(priority) = self.priority {
            properties.insert("priority".to_string(), json!(priority));
        }

        if let Some(due_date) = self.due_date {
            properties.insert("dueDate".to_string(), json!(due_date.to_rfc3339()));
        }

        if let Some(assignee) = self.assignee {
            properties.insert("assignee".to_string(), json!(assignee));
        }

        if let Some(started_at) = self.started_at {
            properties.insert("startedAt".to_string(), json!(started_at.to_rfc3339()));
        }

        if let Some(completed_at) = self.completed_at {
            properties.insert("completedAt".to_string(), json!(completed_at.to_rfc3339()));
        }

        Node {
            id: self.id,
            node_type: "task".to_string(),
            content: self.content,
            version: self.version,
            created_at: self.created_at,
            modified_at: self.modified_at,
            properties: json!(properties),
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
            member_of: Vec::new(),
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

    /// Get the task's due date as string (for API compatibility)
    pub fn due_date(&self) -> Option<String> {
        self.due_date.map(|dt| dt.to_rfc3339())
    }

    /// Set the task's due date
    pub fn set_due_date(&mut self, due_date: Option<DateTime<Utc>>) {
        self.due_date = due_date;
        self.modified_at = Utc::now();
    }

    /// Get the task's assignee ID (for API compatibility)
    pub fn assignee_id(&self) -> Option<String> {
        self.assignee.clone()
    }

    /// Set the task's assignee ID
    pub fn set_assignee_id(&mut self, assignee_id: Option<String>) {
        self.assignee = assignee_id;
        self.modified_at = Utc::now();
    }
}

/// Builder for creating new TaskNode instances
pub struct TaskNodeBuilder {
    content: String,
    status: Option<TaskStatus>,
    priority: Option<TaskPriority>,
    due_date: Option<DateTime<Utc>>,
    assignee: Option<String>,
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

    /// Set the task due date
    pub fn with_due_date(mut self, due_date: DateTime<Utc>) -> Self {
        self.due_date = Some(due_date);
        self
    }

    /// Set the task due date from string (for convenience)
    pub fn with_due_date_str(mut self, due_date: &str) -> Self {
        if let Ok(dt) = DateTime::parse_from_rfc3339(due_date) {
            self.due_date = Some(dt.with_timezone(&Utc));
        }
        self
    }

    /// Set the task assignee
    pub fn with_assignee(mut self, assignee: String) -> Self {
        self.assignee = Some(assignee);
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
            props.insert("dueDate".to_string(), json!(d.to_rfc3339()));
        }
        if let Some(a) = &self.assignee {
            props.insert("assignee".to_string(), json!(a));
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
            assignee: self.assignee,
            started_at: None,
            completed_at: None,
        }
    }
}

/// Partial update structure for task nodes
///
/// Supports updating task-specific fields (status, priority, due_date, assignee)
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
    /// - `Some(Some(p))` - Set to priority p (low, medium, high, or user-defined)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<Option<TaskPriority>>,

    /// Update due date (task property)
    /// - `None` - Don't change
    /// - `Some(None)` - Clear due date
    /// - `Some(Some(dt))` - Set to specific date
    ///
    /// Accepts both "YYYY-MM-DD" and full ISO8601 formats
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub due_date: Option<Option<DateTime<Utc>>>,

    /// Update assignee (task property)
    /// - `None` - Don't change
    /// - `Some(None)` - Clear assignee
    /// - `Some(Some(id))` - Set to specific assignee
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Option<String>>,

    /// Update started_at date (task property)
    /// - `None` - Don't change
    /// - `Some(None)` - Clear started_at
    /// - `Some(Some(dt))` - Set to specific date
    ///
    /// Accepts both "YYYY-MM-DD" and full ISO8601 formats
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub started_at: Option<Option<DateTime<Utc>>>,

    /// Update completed_at date (task property)
    /// - `None` - Don't change
    /// - `Some(None)` - Clear completed_at
    /// - `Some(Some(dt))` - Set to specific date
    ///
    /// Accepts both "YYYY-MM-DD" and full ISO8601 formats
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "flexible_date::deserialize_with_null"
    )]
    pub completed_at: Option<Option<DateTime<Utc>>>,

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

    /// Set due date update (Some(value) to set, None to clear)
    pub fn with_due_date(mut self, due_date: Option<DateTime<Utc>>) -> Self {
        self.due_date = Some(due_date);
        self
    }

    /// Set assignee update (Some(value) to set, None to clear)
    pub fn with_assignee(mut self, assignee: Option<String>) -> Self {
        self.assignee = Some(assignee);
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
            && self.assignee.is_none()
            && self.started_at.is_none()
            && self.completed_at.is_none()
            && self.content.is_none()
    }

    /// Check if this update contains task property fields
    pub fn has_property_fields(&self) -> bool {
        self.status.is_some()
            || self.priority.is_some()
            || self.due_date.is_some()
            || self.assignee.is_some()
            || self.started_at.is_some()
            || self.completed_at.is_some()
    }

    /// Check if this update contains content field
    pub fn has_content_field(&self) -> bool {
        self.content.is_some()
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
            update.started_at.unwrap().is_none(),
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
