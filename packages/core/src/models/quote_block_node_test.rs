//! Tests for QuoteBlockNode wrapper

#[cfg(test)]
mod tests {
    use crate::models::{quote_block_node::QuoteBlockNode, Node};
    use serde_json::json;

    #[test]
    fn test_new_creates_default_quote_block() {
        let quote_block = QuoteBlockNode::new("To be or not to be".to_string()).build();

        assert_eq!(quote_block.as_node().node_type, "quote-block");
        assert_eq!(quote_block.as_node().content, "To be or not to be");
    }

    #[test]
    fn test_from_node_validates_type() {
        let node = Node::new(
            "quote-block".to_string(),
            "All that glitters is not gold".to_string(),
            json!({}),
        );

        let result = QuoteBlockNode::from_node(node);
        assert!(result.is_ok());
    }

    #[test]
    fn test_from_node_rejects_wrong_type() {
        let wrong_type = Node::new(
            "text".to_string(),
            "Not a quote block".to_string(),
            json!({}),
        );

        let result = QuoteBlockNode::from_node(wrong_type);
        assert!(result.is_err());

        match result {
            Err(e) => assert!(e.to_string().contains("expected 'quote-block', got 'text'")),
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_into_node_preserves_all_data() {
        let original = Node::new(
            "quote-block".to_string(),
            "The only thing we have to fear is fear itself".to_string(),
            json!({}),
        );
        let original_id = original.id.clone();

        let quote_block = QuoteBlockNode::from_node(original).unwrap();
        let converted_back = quote_block.into_node();

        assert_eq!(converted_back.id, original_id);
        assert_eq!(converted_back.node_type, "quote-block");
        assert_eq!(
            converted_back.content,
            "The only thing we have to fear is fear itself"
        );
    }

    #[test]
    fn test_as_node_provides_immutable_reference() {
        let quote_block = QuoteBlockNode::new("quote".to_string()).build();

        let node_ref = quote_block.as_node();
        assert_eq!(node_ref.node_type, "quote-block");
        assert_eq!(node_ref.content, "quote");
    }

    #[test]
    fn test_as_node_mut_allows_modification() {
        let mut quote_block = QuoteBlockNode::new("quote".to_string()).build();

        // Test that we can get mutable reference
        let node_mut = quote_block.as_node_mut();
        assert_eq!(node_mut.node_type, "quote-block");
    }

    #[test]
    fn test_builder_pattern_chains_correctly() {
        let quote_block = QuoteBlockNode::new("A journey of a thousand miles".to_string()).build();

        assert_eq!(
            quote_block.as_node().content,
            "A journey of a thousand miles"
        );
    }

    #[test]
    fn test_multiline_quote_content() {
        let multiline_quote = r#"Be yourself; everyone else is already taken.
It is better to be hated for what you are
than to be loved for what you are not."#;

        let quote_block = QuoteBlockNode::new(multiline_quote.to_string()).build();

        assert_eq!(quote_block.as_node().content, multiline_quote);
    }

    #[test]
    fn test_empty_quote_content() {
        // The wrapper doesn't enforce content validation (that's NodeBehavior's job)
        // But we should be able to create nodes with empty content
        let quote_block = QuoteBlockNode::new("".to_string()).build();

        assert_eq!(quote_block.as_node().content, "");
    }

    #[test]
    fn test_quote_with_special_characters() {
        let quote_with_special = "He said, \"Don't quote me on this!\" — Anonymous";
        let quote_block = QuoteBlockNode::new(quote_with_special.to_string()).build();

        assert_eq!(quote_block.as_node().content, quote_with_special);
    }

    #[test]
    fn test_various_quote_scenarios() {
        let quotes = vec![
            "In the middle of difficulty lies opportunity.",
            "The future belongs to those who believe in the beauty of their dreams.",
            "Success is not final, failure is not fatal: it is the courage to continue that counts.",
        ];

        for quote in quotes {
            let quote_block = QuoteBlockNode::new(quote.to_string()).build();

            assert_eq!(quote_block.as_node().content, quote);
            assert_eq!(quote_block.as_node().node_type, "quote-block");
        }
    }

    #[test]
    fn test_from_node_with_empty_properties() {
        let node = Node::new("quote-block".to_string(), "A quote".to_string(), json!({}));

        let quote_block = QuoteBlockNode::from_node(node).unwrap();
        assert_eq!(quote_block.as_node().content, "A quote");
    }

    #[test]
    fn test_from_node_with_extra_properties() {
        // Extra properties should be preserved even though we don't use them
        let node = Node::new(
            "quote-block".to_string(),
            "A quote".to_string(),
            json!({"customProp": "value"}),
        );

        let quote_block = QuoteBlockNode::from_node(node).unwrap();
        assert_eq!(quote_block.as_node().properties["customProp"], "value");
    }
}
