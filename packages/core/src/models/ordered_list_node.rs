//! Type-Safe OrderedList Node Wrapper
//!
//! Provides ergonomic, compile-time type-safe access to ordered list node properties
//! while maintaining the universal Node storage model.
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::{Node, OrderedListNode};
//! use serde_json::json;
//!
//! // Create an ordered list
//! let ordered_list = OrderedListNode::new("First item in the list".to_string())
//!     .build();
//!
//! assert_eq!(ordered_list.as_node().content, "First item in the list");
//!
//! // Convert to universal Node for storage
//! let node = ordered_list.into_node();
//! ```

use crate::models::Node;
use serde_json::json;
use thiserror::Error;

/// Validation errors for OrderedListNode operations
#[derive(Error, Debug)]
pub enum OrderedListValidationError {
    #[error("Wrong node type: expected 'ordered-list', got '{actual}'")]
    WrongNodeType { actual: String },

    #[error("Invalid properties format: {0}")]
    InvalidProperties(String),
}

/// Type-safe wrapper for ordered list nodes
///
/// Provides ergonomic access to ordered list properties while maintaining
/// the universal Node storage model underneath.
pub struct OrderedListNode {
    node: Node,
}

impl OrderedListNode {
    /// Create a new ordered list node
    ///
    /// # Arguments
    ///
    /// * `content` - The list item content to store in the node
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::OrderedListNode;
    ///
    /// let ordered_list = OrderedListNode::new("First step: gather requirements".to_string())
    ///     .build();
    /// ```
    #[allow(clippy::new_ret_no_self)]
    pub fn new(content: String) -> OrderedListNodeBuilder {
        OrderedListNodeBuilder { content }
    }

    /// Create an OrderedListNode from an existing universal Node
    ///
    /// # Errors
    ///
    /// Returns `OrderedListValidationError::WrongNodeType` if the node's type is not "ordered-list"
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{Node, OrderedListNode};
    /// use serde_json::json;
    ///
    /// let node = Node::new(
    ///     "ordered-list".to_string(),
    ///     "Second step: implement solution".to_string(),
    ///     None,
    ///     json!({}),
    /// );
    ///
    /// let ordered_list = OrderedListNode::from_node(node).unwrap();
    /// ```
    pub fn from_node(node: Node) -> Result<Self, OrderedListValidationError> {
        if node.node_type != "ordered-list" {
            return Err(OrderedListValidationError::WrongNodeType {
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
    /// use nodespace_core::models::OrderedListNode;
    ///
    /// let ordered_list = OrderedListNode::new("list item".to_string()).build();
    /// let node_id = &ordered_list.as_node().id;
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
    /// use nodespace_core::models::OrderedListNode;
    ///
    /// let mut ordered_list = OrderedListNode::new("list item".to_string()).build();
    /// ordered_list.as_node_mut().content = "updated item".to_string();
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
    /// use nodespace_core::models::OrderedListNode;
    ///
    /// let ordered_list = OrderedListNode::new("list item".to_string())
    ///     .build();
    ///
    /// let node = ordered_list.into_node();
    /// assert_eq!(node.node_type, "ordered-list");
    /// ```
    pub fn into_node(self) -> Node {
        self.node
    }
}

/// Builder for creating new OrderedListNode instances
///
/// Provides a fluent API for configuring ordered list properties before creation.
pub struct OrderedListNodeBuilder {
    content: String,
}

impl OrderedListNodeBuilder {
    /// Build the OrderedListNode
    ///
    /// Creates the underlying universal Node with ordered list properties.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::OrderedListNode;
    ///
    /// let ordered_list = OrderedListNode::new("Complete the first task".to_string())
    ///     .build();
    /// ```
    pub fn build(self) -> OrderedListNode {
        // OrderedList nodes have no custom properties (empty object is correct)
        // All node metadata (content, etc.) is stored in the universal Node
        let properties = json!({});

        let node = Node::new("ordered-list".to_string(), self.content, properties);

        OrderedListNode { node }
    }
}
