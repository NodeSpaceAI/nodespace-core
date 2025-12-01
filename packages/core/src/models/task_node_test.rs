//! Tests for TaskNode wrapper

#[cfg(test)]
mod tests {
    use crate::models::{task_node::TaskPriority, task_node::TaskStatus, Node, TaskNode};
    use chrono::{DateTime, Utc};
    use serde_json::json;

    #[test]
    fn test_from_node_validates_type() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        assert!(TaskNode::from_node(node).is_ok());

        let wrong_type = Node::new("text".to_string(), "Test".to_string(), json!({}));
        let result = TaskNode::from_node(wrong_type);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 'task'"));
    }

    #[test]
    fn test_status_getter() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"status": "in_progress"}),
        );
        let task = TaskNode::from_node(node).unwrap();
        assert_eq!(task.status(), TaskStatus::InProgress);
    }

    #[test]
    fn test_status_getter_default() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let task = TaskNode::from_node(node).unwrap();
        // Default status is Open (Issue #670)
        assert_eq!(task.status(), TaskStatus::Open);
    }

    #[test]
    fn test_status_getter_user_defined() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"status": "blocked"}),
        );
        let task = TaskNode::from_node(node).unwrap();
        // User-defined statuses are now valid (schema extensibility)
        assert_eq!(task.status(), TaskStatus::User("blocked".to_string()));
        assert!(task.status().is_user_defined());
        assert!(!task.status().is_core());
    }

    #[test]
    fn test_status_setter() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let mut task = TaskNode::from_node(node).unwrap();

        task.set_status(TaskStatus::Done);

        assert_eq!(task.status(), TaskStatus::Done);
        // Direct field access now
        assert_eq!(task.status, TaskStatus::Done);
        // Status value uses lowercase format (Issue #670)
        assert_eq!(task.as_node().properties["status"], "done");
    }

    #[test]
    fn test_priority_getter() {
        // Priority as string enum value
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"priority": "high"}),
        );
        let task = TaskNode::from_node(node).unwrap();
        assert_eq!(task.get_priority(), TaskPriority::High);
    }

    #[test]
    fn test_priority_getter_default() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let task = TaskNode::from_node(node).unwrap();
        // Default priority is Medium
        assert_eq!(task.get_priority(), TaskPriority::Medium);
    }

    #[test]
    fn test_priority_setter() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let mut task = TaskNode::from_node(node).unwrap();

        task.set_priority(TaskPriority::Low);

        assert_eq!(task.get_priority(), TaskPriority::Low);
        assert_eq!(task.as_node().properties["priority"], "low");
    }

    #[test]
    fn test_due_date_getter() {
        // Use RFC3339 format for proper parsing
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"due_date": "2025-01-15T00:00:00Z"}),
        );
        let task = TaskNode::from_node(node).unwrap();
        assert!(task.due_date().is_some());
        assert!(task.due_date().unwrap().contains("2025-01-15"));
    }

    #[test]
    fn test_due_date_getter_none() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let task = TaskNode::from_node(node).unwrap();
        assert_eq!(task.due_date(), None);
    }

    #[test]
    fn test_due_date_setter() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let mut task = TaskNode::from_node(node).unwrap();

        let due_date = DateTime::parse_from_rfc3339("2025-02-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        task.set_due_date(Some(due_date));

        assert!(task.due_date().is_some());
        assert!(task.due_date().unwrap().contains("2025-02-01"));
    }

    #[test]
    fn test_due_date_clear() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"due_date": "2025-01-15T00:00:00Z"}),
        );
        let mut task = TaskNode::from_node(node).unwrap();

        task.set_due_date(None);

        assert_eq!(task.due_date(), None);
        assert!(task.as_node().properties.get("due_date").is_none());
    }

    #[test]
    fn test_assignee_id_getter() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"assignee_id": "user-123"}),
        );
        let task = TaskNode::from_node(node).unwrap();
        assert_eq!(task.assignee_id(), Some("user-123".to_string()));
    }

    #[test]
    fn test_assignee_id_getter_none() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let task = TaskNode::from_node(node).unwrap();
        assert_eq!(task.assignee_id(), None);
    }

    #[test]
    fn test_assignee_id_setter() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let mut task = TaskNode::from_node(node).unwrap();

        task.set_assignee_id(Some("user-456".to_string()));

        assert_eq!(task.assignee_id(), Some("user-456".to_string()));
        assert_eq!(task.as_node().properties["assignee"], "user-456");
    }

    #[test]
    fn test_assignee_id_clear() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"assignee_id": "user-123"}),
        );
        let mut task = TaskNode::from_node(node).unwrap();

        task.set_assignee_id(None);

        assert_eq!(task.assignee_id(), None);
        assert!(task.as_node().properties.get("assignee").is_none());
    }

    #[test]
    fn test_into_node_preserves_data() {
        // Status and priority use string enum format
        let original = Node::new(
            "task".to_string(),
            "Test task".to_string(),
            json!({"status": "open", "priority": "medium"}),
        );
        let original_id = original.id.clone();

        let task = TaskNode::from_node(original).unwrap();
        let converted_back = task.into_node();

        assert_eq!(converted_back.id, original_id);
        assert_eq!(converted_back.node_type, "task");
        assert_eq!(converted_back.content, "Test task");
        assert_eq!(converted_back.properties["status"], "open");
        assert_eq!(converted_back.properties["priority"], "medium");
    }

    #[test]
    fn test_as_node_reference() {
        // Status uses lowercase format (Issue #670)
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"status": "done"}),
        );
        let task = TaskNode::from_node(node).unwrap();

        let node_ref = task.as_node();
        assert_eq!(node_ref.node_type, "task");
        assert_eq!(node_ref.content, "Test");
    }

    #[test]
    fn test_direct_field_access() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let mut task = TaskNode::from_node(node).unwrap();

        // Direct field mutation
        task.content = "Updated content".to_string();

        assert_eq!(task.content, "Updated content");
        assert_eq!(task.as_node().content, "Updated content");
    }

    #[test]
    fn test_builder_minimal() {
        let task = TaskNode::builder("Implement feature".to_string()).build();

        assert_eq!(task.content, "Implement feature");
        assert_eq!(task.as_node().node_type, "task");
        // Default status is Open (Issue #670)
        assert_eq!(task.status(), TaskStatus::Open);
        // Default priority is Medium
        assert_eq!(task.get_priority(), TaskPriority::Medium);
    }

    #[test]
    fn test_builder_with_status() {
        let task = TaskNode::builder("Write tests".to_string())
            .with_status(TaskStatus::InProgress)
            .build();

        assert_eq!(task.status(), TaskStatus::InProgress);
        // Direct field access
        assert_eq!(task.status, TaskStatus::InProgress);
    }

    #[test]
    fn test_builder_with_priority() {
        let task = TaskNode::builder("Fix bug".to_string())
            .with_priority(TaskPriority::High)
            .build();

        assert_eq!(task.get_priority(), TaskPriority::High);
        // Direct field access
        assert_eq!(task.priority, Some(TaskPriority::High));
    }

    #[test]
    fn test_builder_full() {
        let due_date = DateTime::parse_from_rfc3339("2025-12-31T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let task = TaskNode::builder("Complete project".to_string())
            .with_status(TaskStatus::InProgress)
            .with_priority(TaskPriority::High)
            .with_due_date(due_date)
            .with_assignee("user-789".to_string())
            .build();

        assert_eq!(task.status(), TaskStatus::InProgress);
        assert_eq!(task.get_priority(), TaskPriority::High);
        assert!(task.due_date().is_some());
        assert!(task.due_date().unwrap().contains("2025-12-31"));
        assert_eq!(task.assignee_id(), Some("user-789".to_string()));
    }

    #[test]
    fn test_builder_with_due_date_str() {
        let task = TaskNode::builder("Complete project".to_string())
            .with_due_date_str("2025-12-31T00:00:00Z")
            .build();

        assert!(task.due_date().is_some());
        assert!(task.due_date().unwrap().contains("2025-12-31"));
    }

    #[test]
    fn test_task_status_from_str() {
        // Status values use lowercase format (Issue #670)
        assert_eq!("open".parse::<TaskStatus>().unwrap(), TaskStatus::Open);
        assert_eq!(
            "in_progress".parse::<TaskStatus>().unwrap(),
            TaskStatus::InProgress
        );
        assert_eq!("done".parse::<TaskStatus>().unwrap(), TaskStatus::Done);
        assert_eq!(
            "cancelled".parse::<TaskStatus>().unwrap(),
            TaskStatus::Cancelled
        );
    }

    #[test]
    fn test_task_status_from_str_user_defined() {
        // User-defined statuses are now valid (schema extensibility)
        let result = "blocked".parse::<TaskStatus>();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), TaskStatus::User("blocked".to_string()));

        let result = "review".parse::<TaskStatus>();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), TaskStatus::User("review".to_string()));
    }

    #[test]
    fn test_task_status_as_str() {
        // Status values use lowercase format (Issue #670)
        assert_eq!(TaskStatus::Open.as_str(), "open");
        assert_eq!(TaskStatus::InProgress.as_str(), "in_progress");
        assert_eq!(TaskStatus::Done.as_str(), "done");
        assert_eq!(TaskStatus::Cancelled.as_str(), "cancelled");
        // User-defined statuses preserve their original value
        assert_eq!(TaskStatus::User("blocked".to_string()).as_str(), "blocked");
        assert_eq!(TaskStatus::User("review".to_string()).as_str(), "review");
    }

    #[test]
    fn test_all_status_values() {
        // Updated status values per Issue #670
        let statuses = vec![
            TaskStatus::Open,
            TaskStatus::InProgress,
            TaskStatus::Done,
            TaskStatus::Cancelled,
        ];

        for status in statuses {
            // Round-trip test
            let str_repr = status.as_str();
            let parsed: TaskStatus = str_repr.parse().unwrap();
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn test_multiple_property_updates() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let mut task = TaskNode::from_node(node).unwrap();

        let due_date = DateTime::parse_from_rfc3339("2025-03-15T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Update multiple properties
        task.set_status(TaskStatus::InProgress);
        task.set_priority(TaskPriority::Low);
        task.set_due_date(Some(due_date));
        task.set_assignee_id(Some("user-999".to_string()));

        // Verify all updates
        assert_eq!(task.status(), TaskStatus::InProgress);
        assert_eq!(task.get_priority(), TaskPriority::Low);
        assert!(task.due_date().is_some());
        assert!(task.due_date().unwrap().contains("2025-03-15"));
        assert_eq!(task.assignee_id(), Some("user-999".to_string()));
    }

    #[test]
    fn test_serde_serialization() {
        let task = TaskNode::builder("Serialize me".to_string())
            .with_status(TaskStatus::Done)
            .with_priority(TaskPriority::High)
            .build();

        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["content"], "Serialize me");
        assert_eq!(json["status"], "done");
        assert_eq!(json["priority"], "high");
    }

    #[test]
    fn test_serde_deserialization() {
        // Uses camelCase for JSON (matching Node struct convention)
        let json = json!({
            "id": "test-123",
            "content": "Deserialize me",
            "version": 1,
            "createdAt": "2025-01-01T00:00:00Z",
            "modifiedAt": "2025-01-01T00:00:00Z",
            "status": "in_progress",
            "priority": "medium"
        });

        let task: TaskNode = serde_json::from_value(json).unwrap();
        assert_eq!(task.id, "test-123");
        assert_eq!(task.content, "Deserialize me");
        assert_eq!(task.status, TaskStatus::InProgress);
        assert_eq!(task.priority, Some(TaskPriority::Medium));
    }

    #[test]
    fn test_priority_user_defined() {
        // User-defined priorities are allowed (schema extensibility)
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"priority": "critical"}),
        );
        let task = TaskNode::from_node(node).unwrap();
        assert_eq!(
            task.get_priority(),
            TaskPriority::User("critical".to_string())
        );
        assert!(task.get_priority().is_user_defined());
        assert!(!task.get_priority().is_core());
    }

    #[test]
    fn test_priority_as_str() {
        assert_eq!(TaskPriority::Low.as_str(), "low");
        assert_eq!(TaskPriority::Medium.as_str(), "medium");
        assert_eq!(TaskPriority::High.as_str(), "high");
        assert_eq!(
            TaskPriority::User("critical".to_string()).as_str(),
            "critical"
        );
    }
}
