/**
 * Simple Rust testing examples for NodeSpace
 * Demonstrates practical testing without over-complexity
 */

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    // Simple counter for unique IDs in tests
    static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

    // Example data structure for testing
    #[derive(Debug, PartialEq)]
    pub struct SimpleNode {
        pub id: String,
        pub content: String,
        pub node_type: String,
    }

    impl SimpleNode {
        pub fn new(content: String, node_type: String) -> Self {
            let counter = ID_COUNTER.fetch_add(1, Ordering::SeqCst);
            Self {
                id: format!("node-{}", counter),
                content,
                node_type,
            }
        }

        pub fn validate(&self) -> Result<(), String> {
            if self.content.trim().is_empty() {
                return Err("Content cannot be empty".to_string());
            }
            if !["text", "task", "ai-chat"].contains(&self.node_type.as_str()) {
                return Err("Invalid node type".to_string());
            }
            Ok(())
        }
    }

    // Simple mock store for testing
    pub struct SimpleMockStore {
        nodes: std::collections::HashMap<String, SimpleNode>,
    }

    impl SimpleMockStore {
        pub fn new() -> Self {
            Self {
                nodes: std::collections::HashMap::new(),
            }
        }

        pub fn save(&mut self, node: SimpleNode) -> Result<String, String> {
            node.validate()?;
            let id = node.id.clone();
            self.nodes.insert(id.clone(), node);
            Ok(id)
        }

        pub fn load(&self, id: &str) -> Option<&SimpleNode> {
            self.nodes.get(id)
        }

        pub fn count(&self) -> usize {
            self.nodes.len()
        }
    }

    // Unit tests
    #[test]
    fn test_node_creation() {
        let node = SimpleNode::new("Test content".to_string(), "text".to_string());
        
        assert_eq!(node.content, "Test content");
        assert_eq!(node.node_type, "text");
        assert!(node.id.starts_with("node-"));
    }

    #[test]
    fn test_node_validation_success() {
        let node = SimpleNode::new("Valid content".to_string(), "text".to_string());
        let result = node.validate();
        
        assert!(result.is_ok());
    }

    #[test]
    fn test_node_validation_empty_content() {
        let node = SimpleNode::new("".to_string(), "text".to_string());
        let result = node.validate();
        
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Content cannot be empty");
    }

    #[test]
    fn test_node_validation_invalid_type() {
        let node = SimpleNode::new("Valid content".to_string(), "invalid".to_string());
        let result = node.validate();
        
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Invalid node type");
    }

    // Integration tests
    #[test]
    fn test_store_save_and_load() {
        let mut store = SimpleMockStore::new();
        let node = SimpleNode::new("Test content".to_string(), "text".to_string());
        let node_id = node.id.clone();
        
        let save_result = store.save(node);
        assert!(save_result.is_ok());
        assert_eq!(save_result.unwrap(), node_id);
        
        let loaded = store.load(&node_id);
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().content, "Test content");
    }

    #[test]
    fn test_store_validation_error() {
        let mut store = SimpleMockStore::new();
        let invalid_node = SimpleNode::new("".to_string(), "text".to_string());
        
        let result = store.save(invalid_node);
        assert!(result.is_err());
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_store_multiple_nodes() {
        let mut store = SimpleMockStore::new();
        
        let node1 = SimpleNode::new("First node".to_string(), "text".to_string());
        let node2 = SimpleNode::new("Second node".to_string(), "task".to_string());
        
        store.save(node1).unwrap();
        store.save(node2).unwrap();
        
        assert_eq!(store.count(), 2);
    }

    #[test]
    fn test_load_nonexistent_node() {
        let store = SimpleMockStore::new();
        let result = store.load("nonexistent-id");
        
        assert!(result.is_none());
    }
}