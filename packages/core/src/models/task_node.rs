//! Type-Safe TaskNode Wrapper
//!
//! Provides ergonomic, compile-time type-safe access to task node properties
//! while maintaining the universal Node storage model.
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::{Node, TaskNode, TaskStatus};
//! use serde_json::json;
//!
//! // Create from existing node
//! let node = Node::new(
//!     "task".to_string(),
//!     "Implement feature".to_string(),
//!     json!({"status": "open", "priority": 2}),
//! );
//! let task = TaskNode::from_node(node).unwrap();
//!
//! // Type-safe property access
//! assert_eq!(task.status(), TaskStatus::Open);
//! assert_eq!(task.priority(), 2);
//!
//! // Create with builder
//! let task = TaskNode::builder("Write tests".to_string())
//!     .with_status(TaskStatus::InProgress)
//!     .with_priority(3)
//!     .build();
//! ```

use crate::models::{Node, ValidationError};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    /// Task has not been started
    Open,
    /// Task is currently being worked on
    InProgress,
    /// Task has been finished
    Done,
    /// Task has been cancelled/abandoned
    Cancelled,
}

impl FromStr for TaskStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(Self::Open),
            "in_progress" => Ok(Self::InProgress),
            "done" => Ok(Self::Done),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("Invalid task status: {}", s)),
        }
    }
}

impl TaskStatus {
    /// Convert status to string representation
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::InProgress => "in_progress",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Type-safe wrapper for task nodes
///
/// Provides ergonomic access to task-specific properties while maintaining
/// the universal Node storage model underneath.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::models::{Node, TaskNode, TaskStatus};
/// use serde_json::json;
///
/// let node = Node::new(
///     "task".to_string(),
///     "Fix bug".to_string(),
///     json!({"status": "open"}),
/// );
/// let mut task = TaskNode::from_node(node).unwrap();
///
/// task.set_status(TaskStatus::Done);
/// assert_eq!(task.status(), TaskStatus::Done);
/// ```
#[derive(Debug, Clone)]
pub struct TaskNode {
    node: Node,
}

impl TaskNode {
    /// Create a TaskNode from an existing Node
    ///
    /// # Errors
    ///
    /// Returns `ValidationError::InvalidNodeType` if the node type is not "task".
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{Node, TaskNode};
    /// use serde_json::json;
    ///
    /// let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
    /// let task = TaskNode::from_node(node).unwrap();
    /// ```
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        if node.node_type != "task" {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected 'task', got '{}'",
                node.node_type
            )));
        }
        Ok(Self { node })
    }

    /// Create a builder for a new TaskNode with the given content
    ///
    /// Returns a builder for setting additional properties.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{TaskNode, TaskStatus};
    ///
    /// let task = TaskNode::builder("Write tests".to_string())
    ///     .with_status(TaskStatus::InProgress)
    ///     .with_priority(3)
    ///     .build();
    /// ```
    pub fn builder(content: String) -> TaskNodeBuilder {
        TaskNodeBuilder {
            content,
            status: None,
            priority: None,
            due_date: None,
            assignee_id: None,
        }
    }

    /// Get the task's status
    ///
    /// Returns `TaskStatus::Open` if no status is set.
    pub fn status(&self) -> TaskStatus {
        self.node
            .properties
            .get("status")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok())
            .unwrap_or(TaskStatus::Open)
    }

    /// Set the task's status
    pub fn set_status(&mut self, status: TaskStatus) {
        if let Some(obj) = self.node.properties.as_object_mut() {
            obj.insert("status".to_string(), json!(status.as_str()));
        }
    }

    /// Default priority value (medium priority)
    const DEFAULT_PRIORITY: i32 = 2;

    /// Get the task's priority
    ///
    /// Returns `2` (medium priority) if no priority is set.
    /// Valid range is typically 1 (highest) to 4 (lowest).
    pub fn priority(&self) -> i32 {
        self.node
            .properties
            .get("priority")
            .and_then(|v| v.as_i64())
            .map(|v| v as i32)
            .unwrap_or(Self::DEFAULT_PRIORITY)
    }

    /// Set the task's priority
    ///
    /// Valid range is typically 1 (highest) to 4 (lowest).
    pub fn set_priority(&mut self, priority: i32) {
        if let Some(obj) = self.node.properties.as_object_mut() {
            obj.insert("priority".to_string(), json!(priority));
        }
    }

    /// Get the task's due date
    ///
    /// Returns `None` if no due date is set.
    pub fn due_date(&self) -> Option<String> {
        self.node
            .properties
            .get("due_date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Set the task's due date
    ///
    /// Pass `None` to clear the due date.
    pub fn set_due_date(&mut self, due_date: Option<String>) {
        if let Some(obj) = self.node.properties.as_object_mut() {
            if let Some(date) = due_date {
                obj.insert("due_date".to_string(), json!(date));
            } else {
                obj.remove("due_date");
            }
        }
    }

    /// Get the task's assignee ID
    ///
    /// Returns `None` if no assignee is set.
    pub fn assignee_id(&self) -> Option<String> {
        self.node
            .properties
            .get("assignee_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Set the task's assignee ID
    ///
    /// Pass `None` to clear the assignee.
    pub fn set_assignee_id(&mut self, assignee_id: Option<String>) {
        if let Some(obj) = self.node.properties.as_object_mut() {
            if let Some(id) = assignee_id {
                obj.insert("assignee_id".to_string(), json!(id));
            } else {
                obj.remove("assignee_id");
            }
        }
    }

    /// Get a reference to the underlying Node
    pub fn as_node(&self) -> &Node {
        &self.node
    }

    /// Get a mutable reference to the underlying Node
    pub fn as_node_mut(&mut self) -> &mut Node {
        &mut self.node
    }

    /// Convert back to universal Node (consumes wrapper)
    pub fn into_node(self) -> Node {
        self.node
    }
}

/// Builder for creating new TaskNode instances
pub struct TaskNodeBuilder {
    content: String,
    status: Option<TaskStatus>,
    priority: Option<i32>,
    due_date: Option<String>,
    assignee_id: Option<String>,
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
    pub fn with_due_date(mut self, due_date: String) -> Self {
        self.due_date = Some(due_date);
        self
    }

    /// Set the task assignee
    pub fn with_assignee_id(mut self, assignee_id: String) -> Self {
        self.assignee_id = Some(assignee_id);
        self
    }

    /// Build the TaskNode
    pub fn build(self) -> TaskNode {
        let mut properties = serde_json::Map::new();

        // Set status (default to Open if not specified)
        let status = self.status.unwrap_or(TaskStatus::Open);
        properties.insert("status".to_string(), json!(status.as_str()));

        // Set priority (default to 2 if not specified)
        let priority = self.priority.unwrap_or(TaskNode::DEFAULT_PRIORITY);
        properties.insert("priority".to_string(), json!(priority));

        // Set optional fields
        if let Some(due_date) = self.due_date {
            properties.insert("due_date".to_string(), json!(due_date));
        }

        if let Some(assignee_id) = self.assignee_id {
            properties.insert("assignee_id".to_string(), json!(assignee_id));
        }

        let node = Node::new("task".to_string(), self.content, json!(properties));

        TaskNode { node }
    }
}
