//! Tests for TextNode wrapper

#[cfg(test)]
mod tests {
    use crate::models::{Node, TextNode};
    use serde_json::json;

    #[test]
    fn test_from_node_validates_type() {
        let node = Node::new("text".to_string(), "Content".to_string(), json!({}));
        assert!(TextNode::from_node(node).is_ok());

        let wrong_type = Node::new("task".to_string(), "Content".to_string(), json!({}));
        let result = TextNode::from_node(wrong_type);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected 'text'"));
    }

    #[test]
    fn test_from_node_with_content() {
        let node = Node::new("text".to_string(), "My note content".to_string(), json!({}));
        let text = TextNode::from_node(node).unwrap();
        assert_eq!(text.as_node().content, "My note content");
    }

    #[test]
    fn test_into_node_preserves_data() {
        let original = Node::new("text".to_string(), "Test content".to_string(), json!({}));
        let original_id = original.id.clone();

        let text = TextNode::from_node(original).unwrap();
        let converted_back = text.into_node();

        assert_eq!(converted_back.id, original_id);
        assert_eq!(converted_back.node_type, "text");
        assert_eq!(converted_back.content, "Test content");
    }

    #[test]
    fn test_as_node_reference() {
        let node = Node::new("text".to_string(), "Content".to_string(), json!({}));
        let text = TextNode::from_node(node).unwrap();

        let node_ref = text.as_node();
        assert_eq!(node_ref.node_type, "text");
        assert_eq!(node_ref.content, "Content");
    }

    #[test]
    fn test_as_node_mut() {
        let node = Node::new("text".to_string(), "Original".to_string(), json!({}));
        let mut text = TextNode::from_node(node).unwrap();

        text.as_node_mut().content = "Updated".to_string();

        assert_eq!(text.as_node().content, "Updated");
    }

    #[test]
    fn test_builder_minimal() {
        let text = TextNode::builder("Simple note".to_string()).build();

        assert_eq!(text.as_node().content, "Simple note");
        assert_eq!(text.as_node().node_type, "text");
    }

    #[test]
    fn test_builder_with_parent() {
        let text = TextNode::builder("Child note".to_string()).build();

        assert_eq!(text.as_node().content, "Child note");
    }

    #[test]
    fn test_properties_are_empty_object() {
        let text = TextNode::builder("Content".to_string()).build();

        // Text nodes should have empty properties object
        assert!(text.as_node().properties.is_object());
        assert_eq!(text.as_node().properties.as_object().unwrap().len(), 0);
    }

    #[test]
    fn test_multiple_text_nodes() {
        let text1 = TextNode::builder("First note".to_string()).build();
        let text2 = TextNode::builder("Second note".to_string()).build();

        assert_ne!(text1.as_node().id, text2.as_node().id);
        assert_eq!(text1.as_node().content, "First note");
        assert_eq!(text2.as_node().content, "Second note");
    }

    #[test]
    fn test_text_node_with_empty_content() {
        let node = Node::new("text".to_string(), String::new(), json!({}));
        let text = TextNode::from_node(node).unwrap();

        assert_eq!(text.as_node().content, "");
    }

    #[test]
    fn test_text_node_with_unicode_content() {
        let content = "Hello 世界 🌍";
        let text = TextNode::builder(content.to_string()).build();

        assert_eq!(text.as_node().content, content);
    }

    #[test]
    fn test_text_node_with_multiline_content() {
        let content = "Line 1\nLine 2\nLine 3";
        let text = TextNode::builder(content.to_string()).build();

        assert_eq!(text.as_node().content, content);
    }

    #[test]
    fn test_text_node_preserves_metadata() {
        let node = Node::new("text".to_string(), "Content".to_string(), json!({}));

        let text = TextNode::from_node(node).unwrap();
        let node_back = text.into_node();

        // Should preserve timestamps
        assert!(node_back.created_at <= chrono::Utc::now());
        assert!(node_back.modified_at <= chrono::Utc::now());
    }
}
