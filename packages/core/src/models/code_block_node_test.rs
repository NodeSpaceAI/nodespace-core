//! Tests for CodeBlockNode wrapper

#[cfg(test)]
mod tests {
    use crate::models::{code_block_node::CodeBlockNode, Node};
    use serde_json::json;

    #[test]
    fn test_new_creates_default_code_block() {
        let code_block = CodeBlockNode::new("print('hello')".to_string()).build();

        assert_eq!(code_block.as_node().node_type, "code-block");
        assert_eq!(code_block.as_node().content, "print('hello')");
        assert_eq!(code_block.language(), "plaintext"); // Default language
    }

    #[test]
    fn test_with_language_sets_language() {
        let code_block = CodeBlockNode::new("print('hello')".to_string())
            .with_language("python")
            .build();

        assert_eq!(code_block.language(), "python");
    }

    #[test]
    fn test_from_node_validates_type() {
        let node = Node::new(
            "code-block".to_string(),
            "const x = 1;".to_string(),
            json!({"language": "javascript"}),
        );

        let result = CodeBlockNode::from_node(node);
        assert!(result.is_ok());
    }

    #[test]
    fn test_from_node_rejects_wrong_type() {
        let wrong_type = Node::new(
            "text".to_string(),
            "Not a code block".to_string(),
            json!({}),
        );

        let result = CodeBlockNode::from_node(wrong_type);
        assert!(result.is_err());

        match result {
            Err(e) => assert!(e.to_string().contains("expected 'code-block', got 'text'")),
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_language_getter_returns_correct_value() {
        let node = Node::new(
            "code-block".to_string(),
            "SELECT * FROM users;".to_string(),
            json!({"language": "sql"}),
        );

        let code_block = CodeBlockNode::from_node(node).unwrap();
        assert_eq!(code_block.language(), "sql");
    }

    #[test]
    fn test_language_getter_handles_missing_property() {
        let node = Node::new(
            "code-block".to_string(),
            "some code".to_string(),
            json!({}), // No language property
        );

        let code_block = CodeBlockNode::from_node(node).unwrap();
        assert_eq!(code_block.language(), "plaintext"); // Default
    }

    #[test]
    fn test_language_getter_handles_invalid_type() {
        let node = Node::new(
            "code-block".to_string(),
            "some code".to_string(),
            json!({"language": 123}), // Wrong type (number instead of string)
        );

        let code_block = CodeBlockNode::from_node(node).unwrap();
        assert_eq!(code_block.language(), "plaintext"); // Falls back to default
    }

    #[test]
    fn test_set_language_updates_property() {
        let node = Node::new(
            "code-block".to_string(),
            "fn main() {}".to_string(),
            json!({}),
        );

        let mut code_block = CodeBlockNode::from_node(node).unwrap();
        code_block.set_language("rust");

        assert_eq!(code_block.language(), "rust");
        assert_eq!(code_block.as_node().properties["language"], "rust");
    }

    #[test]
    fn test_set_language_overwrites_existing() {
        let node = Node::new(
            "code-block".to_string(),
            "code".to_string(),
            json!({"language": "javascript"}),
        );

        let mut code_block = CodeBlockNode::from_node(node).unwrap();
        assert_eq!(code_block.language(), "javascript");

        code_block.set_language("typescript");
        assert_eq!(code_block.language(), "typescript");
    }

    #[test]
    fn test_into_node_preserves_all_data() {
        let original = Node::new(
            "code-block".to_string(),
            "const x = 1;".to_string(),
            json!({"language": "javascript"}),
        );
        let original_id = original.id.clone();

        let code_block = CodeBlockNode::from_node(original).unwrap();
        let converted_back = code_block.into_node();

        assert_eq!(converted_back.id, original_id);
        assert_eq!(converted_back.node_type, "code-block");
        assert_eq!(converted_back.content, "const x = 1;");
        assert_eq!(converted_back.properties["language"], "javascript");
    }

    #[test]
    fn test_as_node_provides_immutable_reference() {
        let code_block = CodeBlockNode::new("code".to_string()).build();

        let node_ref = code_block.as_node();
        assert_eq!(node_ref.node_type, "code-block");
        assert_eq!(node_ref.content, "code");
    }

    #[test]
    fn test_as_node_mut_allows_modification() {
        let mut code_block = CodeBlockNode::new("code".to_string()).build();

        // Test that we can get mutable reference
        let node_mut = code_block.as_node_mut();
        assert_eq!(node_mut.node_type, "code-block");
    }

    #[test]
    fn test_builder_pattern_chains_correctly() {
        let code_block = CodeBlockNode::new("#!/bin/bash\necho 'test'".to_string())
            .with_language("bash")
            .build();

        assert_eq!(code_block.language(), "bash");
        assert_eq!(code_block.as_node().content, "#!/bin/bash\necho 'test'");
    }

    #[test]
    fn test_various_languages() {
        let languages = vec![
            "rust",
            "typescript",
            "javascript",
            "python",
            "go",
            "sql",
            "bash",
            "html",
            "css",
            "json",
        ];

        for lang in languages {
            let code_block = CodeBlockNode::new(format!("code in {}", lang))
                .with_language(lang)
                .build();

            assert_eq!(code_block.language(), lang);
        }
    }

    #[test]
    fn test_multiline_code_content() {
        let multiline_code = r#"function fibonacci(n) {
    if (n <= 1) return n;
    return fibonacci(n - 1) + fibonacci(n - 2);
}

console.log(fibonacci(10));"#;

        let code_block = CodeBlockNode::new(multiline_code.to_string())
            .with_language("javascript")
            .build();

        assert_eq!(code_block.as_node().content, multiline_code);
        assert_eq!(code_block.language(), "javascript");
    }

    #[test]
    fn test_empty_code_content() {
        // The wrapper doesn't enforce content validation (that's NodeBehavior's job)
        // But we should be able to create nodes with empty content
        let code_block = CodeBlockNode::new("".to_string())
            .with_language("plaintext")
            .build();

        assert_eq!(code_block.as_node().content, "");
        assert_eq!(code_block.language(), "plaintext");
    }

    #[test]
    fn test_special_characters_in_language() {
        // Languages might have special naming (c++, c#, etc.)
        let code_block = CodeBlockNode::new("int main() {}".to_string())
            .with_language("c++")
            .build();

        assert_eq!(code_block.language(), "c++");
    }
}
