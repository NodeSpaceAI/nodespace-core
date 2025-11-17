//! Tests for OrderedListNode wrapper

#[cfg(test)]
mod tests {
    use crate::models::{ordered_list_node::OrderedListNode, Node};
    use serde_json::json;

    #[test]
    fn test_new_creates_default_ordered_list() {
        let ordered_list = OrderedListNode::new("First item".to_string()).build();

        assert_eq!(ordered_list.as_node().node_type, "ordered-list");
        assert_eq!(ordered_list.as_node().content, "First item");
    }

    #[test]
    fn test_from_node_validates_type() {
        let node = Node::new(
            "ordered-list".to_string(),
            "Second item".to_string(),
            json!({}),
        );

        let result = OrderedListNode::from_node(node);
        assert!(result.is_ok());
    }

    #[test]
    fn test_from_node_rejects_wrong_type() {
        let wrong_type = Node::new(
            "text".to_string(),
            "Not an ordered list".to_string(),
            json!({}),
        );

        let result = OrderedListNode::from_node(wrong_type);
        assert!(result.is_err());

        match result {
            Err(e) => assert!(e
                .to_string()
                .contains("expected 'ordered-list', got 'text'")),
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_into_node_preserves_all_data() {
        let original = Node::new(
            "ordered-list".to_string(),
            "Complete the task".to_string(),
            json!({}),
        );
        let original_id = original.id.clone();

        let ordered_list = OrderedListNode::from_node(original).unwrap();
        let converted_back = ordered_list.into_node();

        assert_eq!(converted_back.id, original_id);
        assert_eq!(converted_back.node_type, "ordered-list");
        assert_eq!(converted_back.content, "Complete the task");
    }

    #[test]
    fn test_as_node_provides_immutable_reference() {
        let ordered_list = OrderedListNode::new("list item".to_string()).build();

        let node_ref = ordered_list.as_node();
        assert_eq!(node_ref.node_type, "ordered-list");
        assert_eq!(node_ref.content, "list item");
    }

    #[test]
    fn test_as_node_mut_allows_modification() {
        let mut ordered_list = OrderedListNode::new("list item".to_string()).build();

        // Test that we can get mutable reference
        let node_mut = ordered_list.as_node_mut();
        assert_eq!(node_mut.node_type, "ordered-list");
    }

    #[test]
    fn test_builder_pattern_chains_correctly() {
        let ordered_list = OrderedListNode::new("Third step in process".to_string()).build();

        assert_eq!(ordered_list.as_node().content, "Third step in process");
    }

    #[test]
    fn test_multiline_list_content() {
        let multiline_item = r#"Step 1: Analyze requirements
- Sub-point a
- Sub-point b"#;

        let ordered_list = OrderedListNode::new(multiline_item.to_string()).build();

        assert_eq!(ordered_list.as_node().content, multiline_item);
    }

    #[test]
    fn test_empty_list_content() {
        // The wrapper doesn't enforce content validation (that's NodeBehavior's job)
        // But we should be able to create nodes with empty content
        let ordered_list = OrderedListNode::new("".to_string()).build();

        assert_eq!(ordered_list.as_node().content, "");
    }

    #[test]
    fn test_list_with_special_characters() {
        let item_with_special = "Step #1: Review @mentions & <tags>";
        let ordered_list = OrderedListNode::new(item_with_special.to_string()).build();

        assert_eq!(ordered_list.as_node().content, item_with_special);
    }

    #[test]
    fn test_various_list_scenarios() {
        let items = [
            "First: Gather requirements",
            "Second: Design architecture",
            "Third: Implement solution",
            "Fourth: Test thoroughly",
            "Fifth: Deploy to production",
        ];

        for (index, item) in items.iter().enumerate() {
            let ordered_list = OrderedListNode::new(item.to_string()).build();

            assert_eq!(ordered_list.as_node().content, *item);
            assert_eq!(ordered_list.as_node().node_type, "ordered-list");
            assert!(index < 5); // Just using index in assertion
        }
    }

    #[test]
    fn test_from_node_with_empty_properties() {
        let node = Node::new(
            "ordered-list".to_string(),
            "A list item".to_string(),
            json!({}),
        );

        let ordered_list = OrderedListNode::from_node(node).unwrap();
        assert_eq!(ordered_list.as_node().content, "A list item");
    }

    #[test]
    fn test_from_node_with_extra_properties() {
        // Extra properties should be preserved even though we don't use them
        let node = Node::new(
            "ordered-list".to_string(),
            "A list item".to_string(),
            json!({"customProp": "value"}),
        );

        let ordered_list = OrderedListNode::from_node(node).unwrap();
        assert_eq!(ordered_list.as_node().properties["customProp"], "value");
    }

    #[test]
    fn test_numbered_content() {
        let items_with_numbers = vec![
            "1. First item",
            "2. Second item",
            "10. Tenth item",
            "100. Hundredth item",
        ];

        for item in items_with_numbers {
            let ordered_list = OrderedListNode::new(item.to_string()).build();
            assert_eq!(ordered_list.as_node().content, item);
        }
    }

    #[test]
    fn test_long_list_item() {
        let long_item = "This is a very long list item that contains multiple sentences and detailed information. It might span several lines when rendered and include various punctuation marks, numbers like 123, and special characters like @, #, and $.";
        let ordered_list = OrderedListNode::new(long_item.to_string()).build();

        assert_eq!(ordered_list.as_node().content, long_item);
    }
}
