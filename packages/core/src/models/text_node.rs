//! Type-Safe TextNode Wrapper
//!
//! Provides ergonomic access to text nodes while maintaining the universal Node
//! storage model. Text nodes have minimal properties - most data is in the content field.
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::{Node, TextNode};
//! use serde_json::json;
//!
//! // Create from existing node
//! let node = Node::new(
//!     "text".to_string(),
//!     "My note content".to_string(),
//!     None,
//!     json!({}),
//! );
//! let text = TextNode::from_node(node).unwrap();
//!
//! // Create with builder
//! let text = TextNode::builder("Another note".to_string()).build();
//! ```

use crate::models::{Node, ValidationError};
use serde_json::json;

/// Type-safe wrapper for text nodes
///
/// Text nodes are the simplest node type, primarily containing content
/// with minimal properties.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::models::{Node, TextNode};
/// use serde_json::json;
///
/// let node = Node::new(
///     "text".to_string(),
///     "My content".to_string(),
///     None,
///     json!({}),
/// );
/// let text = TextNode::from_node(node).unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct TextNode {
    node: Node,
}

impl TextNode {
    /// Create a TextNode from an existing Node
    ///
    /// # Errors
    ///
    /// Returns `ValidationError::InvalidNodeType` if the node type is not "text".
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{Node, TextNode};
    /// use serde_json::json;
    ///
    /// let node = Node::new("text".to_string(), "Content".to_string(), None, json!({}));
    /// let text = TextNode::from_node(node).unwrap();
    /// ```
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        if node.node_type != "text" {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected 'text', got '{}'",
                node.node_type
            )));
        }
        Ok(Self { node })
    }

    /// Create a builder for a new TextNode with the given content
    ///
    /// Returns a builder for setting additional properties.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::TextNode;
    ///
    /// let text = TextNode::builder("My note".to_string())
    ///     .build();
    /// ```
    pub fn builder(content: String) -> TextNodeBuilder {
        TextNodeBuilder { content }
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

/// Builder for creating new TextNode instances
pub struct TextNodeBuilder {
    content: String,
}

impl TextNodeBuilder {
    /// Build the TextNode
    pub fn build(self) -> TextNode {
        let node = Node::new("text".to_string(), self.content, json!({}));

        TextNode { node }
    }
}
