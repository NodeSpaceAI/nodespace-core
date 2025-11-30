//! Strongly-Typed TaskNode
//!
//! Provides direct deserialization from spoke table with hub data via record link,
//! eliminating the intermediate JSON `properties` step for true compile-time type safety.
//!
//! # Architecture (Issue #673)
//!
//! **Old Pattern (Weak Typing):**
//! ```text
//! DB spoke (task.status)
//!   → Query hub, then query spoke
//!   → Hydrate into Node.properties as JSON
//!   → TaskNode wraps Node
//!   → TaskNode.status() reads from node.properties["status"]
//! ```
//!
//! **New Pattern (Strong Typing):**
//! ```text
//! DB spoke (task.status + task.node.* for hub fields)
//!   → Single query with record link
//!   → Deserialize directly to TaskNode struct
//!   → TaskNode.status is a TaskStatus enum field
//! ```
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::{TaskNode, TaskStatus};
//!
//! // Create with builder (for new tasks)
//! let task = TaskNode::builder("Write tests".to_string())
//!     .with_status(TaskStatus::InProgress)
//!     .with_priority(3)
//!     .build();
//!
//! // Direct field access (no JSON parsing)
//! assert_eq!(task.status, TaskStatus::InProgress);
//! assert_eq!(task.priority, Some(3));
//! ```

use crate::models::{Node, ValidationError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::str::FromStr;

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

/// Strongly-typed task node with direct field access
///
/// Deserializes directly from spoke table with hub data via record link.
/// All fields are strongly typed - no JSON intermediary.
///
/// # Query Pattern
///
/// ```sql
/// SELECT
///     id,
///     status,
///     priority,
///     due_date,
///     assignee,
///     node.id AS node_id,
///     node.content AS content,
///     node.version AS version,
///     node.created_at AS created_at,
///     node.modified_at AS modified_at
/// FROM task:`some-id`;
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
    // Hub fields (from task.node.* via record link)
    // ========================================================================
    /// Unique identifier (matches hub node ID)
    pub id: String,

    /// Primary content/text of the task
    pub content: String,

    /// Optimistic concurrency control version
    #[serde(default = "default_version")]
    pub version: i64,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last modification timestamp
    pub modified_at: DateTime<Utc>,

    // ========================================================================
    // Spoke fields (direct from task table)
    // ========================================================================
    /// Task status (strongly typed enum)
    #[serde(default)]
    pub status: TaskStatus,

    /// Task priority (1 = highest, 4 = lowest)
    #[serde(default)]
    pub priority: Option<i32>,

    /// Due date for the task
    #[serde(default)]
    pub due_date: Option<DateTime<Utc>>,

    /// Assignee node ID
    #[serde(default)]
    pub assignee: Option<String>,
}

fn default_version() -> i64 {
    1
}

impl TaskNode {
    /// Default priority value (medium priority)
    pub const DEFAULT_PRIORITY: i32 = 2;

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
        let status = props
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or_default();

        // Extract priority from properties (supports both integer and string "high"/"medium"/"low")
        let priority = props.get("priority").and_then(|v| {
            if let Some(n) = v.as_i64() {
                Some(n as i32)
            } else if let Some(s) = v.as_str() {
                // Convert string priority to integer (for backward compat with schema format)
                match s {
                    "urgent" | "highest" => Some(1),
                    "high" => Some(2),
                    "medium" | "normal" => Some(3),
                    "low" | "lowest" => Some(4),
                    _ => None,
                }
            } else {
                None
            }
        });

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

        Ok(Self {
            id: node.id,
            content: node.content,
            version: node.version,
            created_at: node.created_at,
            modified_at: node.modified_at,
            status,
            priority,
            due_date,
            assignee,
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
    pub fn into_node(self) -> Node {
        let mut properties = serde_json::Map::new();
        properties.insert("status".to_string(), json!(self.status.as_str()));

        if let Some(priority) = self.priority {
            properties.insert("priority".to_string(), json!(priority));
        }

        if let Some(due_date) = self.due_date {
            properties.insert("due_date".to_string(), json!(due_date.to_rfc3339()));
        }

        if let Some(assignee) = self.assignee {
            properties.insert("assignee".to_string(), json!(assignee));
        }

        Node {
            id: self.id,
            node_type: "task".to_string(),
            content: self.content,
            version: self.version,
            created_at: self.created_at,
            modified_at: self.modified_at,
            properties: json!(properties),
            embedding_vector: None,
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
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

    /// Get the task's priority
    pub fn priority(&self) -> i32 {
        self.priority.unwrap_or(Self::DEFAULT_PRIORITY)
    }

    /// Set the task's priority
    pub fn set_priority(&mut self, priority: i32) {
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
    priority: Option<i32>,
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
    pub fn with_priority(mut self, priority: i32) -> Self {
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

        TaskNode {
            id,
            content: self.content,
            version: 1,
            created_at: now,
            modified_at: now,
            status: self.status.unwrap_or_default(),
            priority: self.priority,
            due_date: self.due_date,
            assignee: self.assignee,
        }
    }
}
