//! Tests for TaskNode wrapper

#[cfg(test)]
mod tests {
    use crate::models::{task_node::TaskStatus, Node, TaskNode};
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
        assert_eq!(task.status(), TaskStatus::Pending);
    }

    #[test]
    fn test_status_getter_invalid() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"status": "invalid_status"}),
        );
        let task = TaskNode::from_node(node).unwrap();
        // Should default to Pending on invalid status
        assert_eq!(task.status(), TaskStatus::Pending);
    }

    #[test]
    fn test_status_setter() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let mut task = TaskNode::from_node(node).unwrap();

        task.set_status(TaskStatus::Completed);

        assert_eq!(task.status(), TaskStatus::Completed);
        assert_eq!(task.as_node().properties["status"], "completed");
    }

    #[test]
    fn test_priority_getter() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"priority": 3}),
        );
        let task = TaskNode::from_node(node).unwrap();
        assert_eq!(task.priority(), 3);
    }

    #[test]
    fn test_priority_getter_default() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let task = TaskNode::from_node(node).unwrap();
        assert_eq!(task.priority(), 2);
    }

    #[test]
    fn test_priority_setter() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let mut task = TaskNode::from_node(node).unwrap();

        task.set_priority(4);

        assert_eq!(task.priority(), 4);
        assert_eq!(task.as_node().properties["priority"], 4);
    }

    #[test]
    fn test_due_date_getter() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"due_date": "2025-01-15"}),
        );
        let task = TaskNode::from_node(node).unwrap();
        assert_eq!(task.due_date(), Some("2025-01-15".to_string()));
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

        task.set_due_date(Some("2025-02-01".to_string()));

        assert_eq!(task.due_date(), Some("2025-02-01".to_string()));
        assert_eq!(task.as_node().properties["due_date"], "2025-02-01");
    }

    #[test]
    fn test_due_date_clear() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"due_date": "2025-01-15"}),
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
        assert_eq!(task.as_node().properties["assignee_id"], "user-456");
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
        assert!(task.as_node().properties.get("assignee_id").is_none());
    }

    #[test]
    fn test_into_node_preserves_data() {
        let original = Node::new(
            "task".to_string(),
            "Test task".to_string(),
            json!({"status": "pending", "priority": 2}),
        );
        let original_id = original.id.clone();

        let task = TaskNode::from_node(original).unwrap();
        let converted_back = task.into_node();

        assert_eq!(converted_back.id, original_id);
        assert_eq!(converted_back.node_type, "task");
        assert_eq!(converted_back.content, "Test task");
        assert_eq!(converted_back.properties["status"], "pending");
        assert_eq!(converted_back.properties["priority"], 2);
    }

    #[test]
    fn test_as_node_reference() {
        let node = Node::new(
            "task".to_string(),
            "Test".to_string(),
            json!({"status": "completed"}),
        );
        let task = TaskNode::from_node(node).unwrap();

        let node_ref = task.as_node();
        assert_eq!(node_ref.node_type, "task");
        assert_eq!(node_ref.content, "Test");
    }

    #[test]
    fn test_as_node_mut() {
        let node = Node::new("task".to_string(), "Test".to_string(), json!({}));
        let mut task = TaskNode::from_node(node).unwrap();

        task.as_node_mut().content = "Updated content".to_string();

        assert_eq!(task.as_node().content, "Updated content");
    }

    #[test]
    fn test_builder_minimal() {
        let task = TaskNode::builder("Implement feature".to_string()).build();

        assert_eq!(task.as_node().content, "Implement feature");
        assert_eq!(task.as_node().node_type, "task");
        assert_eq!(task.status(), TaskStatus::Pending);
        assert_eq!(task.priority(), 2);
    }

    #[test]
    fn test_builder_with_status() {
        let task = TaskNode::builder("Write tests".to_string())
            .with_status(TaskStatus::InProgress)
            .build();

        assert_eq!(task.status(), TaskStatus::InProgress);
    }

    #[test]
    fn test_builder_with_priority() {
        let task = TaskNode::builder("Fix bug".to_string())
            .with_priority(1)
            .build();

        assert_eq!(task.priority(), 1);
    }

    #[test]
    fn test_builder_full() {
        let task = TaskNode::builder("Complete project".to_string())
            .with_status(TaskStatus::InProgress)
            .with_priority(1)
            .with_due_date("2025-12-31".to_string())
            .with_assignee_id("user-789".to_string())
            .build();

        assert_eq!(task.status(), TaskStatus::InProgress);
        assert_eq!(task.priority(), 1);
        assert_eq!(task.due_date(), Some("2025-12-31".to_string()));
        assert_eq!(task.assignee_id(), Some("user-789".to_string()));
    }

    #[test]
    fn test_task_status_from_str() {
        assert_eq!(
            "pending".parse::<TaskStatus>().unwrap(),
            TaskStatus::Pending
        );
        assert_eq!(
            "in_progress".parse::<TaskStatus>().unwrap(),
            TaskStatus::InProgress
        );
        assert_eq!(
            "completed".parse::<TaskStatus>().unwrap(),
            TaskStatus::Completed
        );
        assert_eq!(
            "cancelled".parse::<TaskStatus>().unwrap(),
            TaskStatus::Cancelled
        );
    }

    #[test]
    fn test_task_status_from_str_invalid() {
        let result = "invalid".parse::<TaskStatus>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid task status"));
    }

    #[test]
    fn test_task_status_as_str() {
        assert_eq!(TaskStatus::Pending.as_str(), "pending");
        assert_eq!(TaskStatus::InProgress.as_str(), "in_progress");
        assert_eq!(TaskStatus::Completed.as_str(), "completed");
        assert_eq!(TaskStatus::Cancelled.as_str(), "cancelled");
    }

    #[test]
    fn test_all_status_values() {
        let statuses = vec![
            TaskStatus::Pending,
            TaskStatus::InProgress,
            TaskStatus::Completed,
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

        // Update multiple properties
        task.set_status(TaskStatus::InProgress);
        task.set_priority(3);
        task.set_due_date(Some("2025-03-15".to_string()));
        task.set_assignee_id(Some("user-999".to_string()));

        // Verify all updates
        assert_eq!(task.status(), TaskStatus::InProgress);
        assert_eq!(task.priority(), 3);
        assert_eq!(task.due_date(), Some("2025-03-15".to_string()));
        assert_eq!(task.assignee_id(), Some("user-999".to_string()));
    }
}
