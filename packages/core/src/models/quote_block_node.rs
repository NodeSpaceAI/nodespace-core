//! Type-Safe QuoteBlock Node Wrapper
//!
//! Provides ergonomic, compile-time type-safe access to quote block node properties
//! while maintaining the universal Node storage model.
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::{Node, QuoteBlockNode};
//! use serde_json::json;
//!
//! // Create a quote block
//! let quote_block = QuoteBlockNode::builder("To be or not to be".to_string())
//!     .build();
//!
//! assert_eq!(quote_block.as_node().content, "To be or not to be");
//!
//! // Convert to universal Node for storage
//! let node = quote_block.into_node();
//! ```

use crate::models::Node;
use serde_json::json;
use thiserror::Error;

/// Validation errors for QuoteBlockNode operations
#[derive(Error, Debug)]
pub enum QuoteBlockValidationError {
    #[error("Wrong node type: expected 'quote-block', got '{actual}'")]
    WrongNodeType { actual: String },

    #[error("Invalid properties format: {0}")]
    InvalidProperties(String),
}

/// Type-safe wrapper for quote block nodes
///
/// Provides ergonomic access to quote block properties while maintaining
/// the universal Node storage model underneath.
pub struct QuoteBlockNode {
    node: Node,
}

impl QuoteBlockNode {
    /// Create a new quote block node builder
    ///
    /// # Arguments
    ///
    /// * `content` - The quoted text content to store in the block
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::QuoteBlockNode;
    ///
    /// let quote_block = QuoteBlockNode::builder("The only thing we have to fear is fear itself".to_string())
    ///     .build();
    /// ```
    pub fn builder(content: String) -> QuoteBlockNodeBuilder {
        QuoteBlockNodeBuilder { content }
    }

    /// Create a QuoteBlockNode from an existing universal Node
    ///
    /// # Errors
    ///
    /// Returns `QuoteBlockValidationError::WrongNodeType` if the node's type is not "quote-block"
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{Node, QuoteBlockNode};
    /// use serde_json::json;
    ///
    /// let node = Node::new(
    ///     "quote-block".to_string(),
    ///     "All that glitters is not gold".to_string(),
    ///     json!({}),
    /// );
    ///
    /// let quote_block = QuoteBlockNode::from_node(node).unwrap();
    /// ```
    pub fn from_node(node: Node) -> Result<Self, QuoteBlockValidationError> {
        if node.node_type != "quote-block" {
            return Err(QuoteBlockValidationError::WrongNodeType {
                actual: node.node_type.clone(),
            });
        }
        Ok(Self { node })
    }

    /// Get immutable reference to the underlying universal Node
    ///
    /// Useful for accessing common node fields (id, created_at, etc.)
    /// without consuming the wrapper.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::QuoteBlockNode;
    ///
    /// let quote_block = QuoteBlockNode::builder("quote".to_string()).build();
    /// let node_id = &quote_block.as_node().id;
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
    /// use nodespace_core::models::QuoteBlockNode;
    ///
    /// let mut quote_block = QuoteBlockNode::builder("quote".to_string()).build();
    /// quote_block.as_node_mut().content = "updated quote".to_string();
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
    /// use nodespace_core::models::QuoteBlockNode;
    ///
    /// let quote_block = QuoteBlockNode::builder("quote".to_string())
    ///     .build();
    ///
    /// let node = quote_block.into_node();
    /// assert_eq!(node.node_type, "quote-block");
    /// ```
    pub fn into_node(self) -> Node {
        self.node
    }
}

/// Builder for creating new QuoteBlockNode instances
///
/// Provides a fluent API for configuring quote block properties before creation.
pub struct QuoteBlockNodeBuilder {
    content: String,
}

impl QuoteBlockNodeBuilder {
    /// Build the QuoteBlockNode
    ///
    /// Creates the underlying universal Node with quote block properties.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::QuoteBlockNode;
    ///
    /// let quote_block = QuoteBlockNode::builder("A journey of a thousand miles begins with a single step".to_string())
    ///     .build();
    /// ```
    pub fn build(self) -> QuoteBlockNode {
        // QuoteBlock nodes have no custom properties (empty object is correct)
        // All node metadata (content, etc.) is stored in the universal Node
        let properties = json!({});

        let node = Node::new("quote-block".to_string(), self.content, properties);

        QuoteBlockNode { node }
    }
}
