//! Type-Safe CodeBlock Node Wrapper
//!
//! Provides ergonomic, compile-time type-safe access to code block node properties
//! while maintaining the universal Node storage model.
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::{Node, CodeBlockNode};
//! use serde_json::json;
//!
//! // Create a code block with TypeScript syntax
//! let code_block = CodeBlockNode::builder("const x: number = 42;".to_string())
//!     .with_language("typescript")
//!     .build();
//!
//! assert_eq!(code_block.language(), "typescript");
//! assert_eq!(code_block.as_node().content, "const x: number = 42;");
//!
//! // Convert to universal Node for storage
//! let node = code_block.into_node();
//! ```

use crate::models::Node;
use serde_json::json;
use thiserror::Error;

/// Validation errors for CodeBlockNode operations
#[derive(Error, Debug)]
pub enum CodeBlockValidationError {
    #[error("Wrong node type: expected 'code-block', got '{actual}'")]
    WrongNodeType { actual: String },

    #[error("Invalid properties format: {0}")]
    InvalidProperties(String),
}

/// Type-safe wrapper for code block nodes
///
/// Provides ergonomic access to code block properties while maintaining
/// the universal Node storage model underneath.
pub struct CodeBlockNode {
    node: Node,
}

impl CodeBlockNode {
    /// Create a new code block node builder with default language (plaintext)
    ///
    /// # Arguments
    ///
    /// * `content` - The code content to store in the block
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::CodeBlockNode;
    ///
    /// let code_block = CodeBlockNode::builder("print('Hello, World!')".to_string())
    ///     .with_language("python")
    ///     .build();
    /// ```
    pub fn builder(content: String) -> CodeBlockNodeBuilder {
        CodeBlockNodeBuilder {
            content,
            language: "plaintext".to_string(),
        }
    }

    /// Create a CodeBlockNode from an existing universal Node
    ///
    /// # Errors
    ///
    /// Returns `CodeBlockValidationError::WrongNodeType` if the node's type is not "code-block"
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{Node, CodeBlockNode};
    /// use serde_json::json;
    ///
    /// let node = Node::new(
    ///     "code-block".to_string(),
    ///     "const x = 1;".to_string(),
    ///     json!({"language": "javascript"}),
    /// );
    ///
    /// let code_block = CodeBlockNode::from_node(node).unwrap();
    /// assert_eq!(code_block.language(), "javascript");
    /// ```
    pub fn from_node(node: Node) -> Result<Self, CodeBlockValidationError> {
        if node.node_type != "code-block" {
            return Err(CodeBlockValidationError::WrongNodeType {
                actual: node.node_type.clone(),
            });
        }
        Ok(Self { node })
    }

    /// Get the programming language for syntax highlighting
    ///
    /// Returns "plaintext" if no language is specified or if the property is missing.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::CodeBlockNode;
    ///
    /// let code_block = CodeBlockNode::builder("SELECT * FROM users;".to_string())
    ///     .with_language("sql")
    ///     .build();
    ///
    /// assert_eq!(code_block.language(), "sql");
    /// ```
    pub fn language(&self) -> &str {
        self.node
            .properties
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("plaintext")
    }

    /// Set the programming language for syntax highlighting
    ///
    /// # Arguments
    ///
    /// * `language` - Language identifier (e.g., "rust", "typescript", "python")
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{Node, CodeBlockNode};
    /// use serde_json::json;
    ///
    /// let node = Node::new(
    ///     "code-block".to_string(),
    ///     "fn main() {}".to_string(),
    ///     json!({}),
    /// );
    ///
    /// let mut code_block = CodeBlockNode::from_node(node).unwrap();
    /// code_block.set_language("rust");
    ///
    /// assert_eq!(code_block.language(), "rust");
    /// ```
    pub fn set_language(&mut self, language: impl Into<String>) {
        let language = language.into();
        if let Some(obj) = self.node.properties.as_object_mut() {
            obj.insert("language".to_string(), json!(language));
        }
    }

    /// Get immutable reference to the underlying universal Node
    ///
    /// Useful for accessing common node fields (id, created_at, etc.)
    /// without consuming the wrapper.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::CodeBlockNode;
    ///
    /// let code_block = CodeBlockNode::builder("code".to_string()).build();
    /// let node_id = &code_block.as_node().id;
    /// ```
    pub fn as_node(&self) -> &Node {
        &self.node
    }

    /// Get mutable reference to the underlying universal Node
    ///
    /// Allows direct modification of universal node fields when needed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::CodeBlockNode;
    ///
    /// let mut code_block = CodeBlockNode::builder("code".to_string()).build();
    /// code_block.as_node_mut().content = "updated code".to_string();
    /// ```
    pub fn as_node_mut(&mut self) -> &mut Node {
        &mut self.node
    }

    /// Convert the wrapper back into a universal Node
    ///
    /// Consumes the wrapper and returns the underlying Node for storage operations.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::CodeBlockNode;
    ///
    /// let code_block = CodeBlockNode::builder("code".to_string())
    ///     .with_language("rust")
    ///     .build();
    ///
    /// let node = code_block.into_node();
    /// assert_eq!(node.node_type, "code-block");
    /// ```
    pub fn into_node(self) -> Node {
        self.node
    }
}

/// Builder for creating new CodeBlockNode instances
///
/// Provides a fluent API for configuring code block properties before creation.
pub struct CodeBlockNodeBuilder {
    content: String,
    language: String,
}

impl CodeBlockNodeBuilder {
    /// Set the programming language for the code block
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::CodeBlockNode;
    ///
    /// let code_block = CodeBlockNode::builder("#!/bin/bash\necho 'Hello'".to_string())
    ///     .with_language("bash")
    ///     .build();
    /// ```
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = language.into();
        self
    }

    /// Build the CodeBlockNode
    ///
    /// Creates the underlying universal Node with code block properties.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::CodeBlockNode;
    ///
    /// let code_block = CodeBlockNode::builder("const x = 1;".to_string())
    ///     .with_language("javascript")
    ///     .build();
    /// ```
    pub fn build(self) -> CodeBlockNode {
        let properties = json!({
            "language": self.language,
        });

        let node = Node::new("code-block".to_string(), self.content, properties);

        CodeBlockNode { node }
    }
}
