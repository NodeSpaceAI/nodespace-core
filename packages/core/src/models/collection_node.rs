//! Type-Safe CollectionNode Wrapper
//!
//! Provides ergonomic access to collection nodes while maintaining the universal Node
//! storage model. Collections are organizational containers with globally unique names.
//!
//! # Examples
//!
//! ```rust
//! use nodespace_core::models::{Node, CollectionNode};
//! use serde_json::json;
//!
//! // Create from existing node
//! let node = Node::new(
//!     "collection".to_string(),
//!     "architecture".to_string(),
//!     json!({}),
//! );
//! let collection = CollectionNode::from_node(node).unwrap();
//! assert_eq!(collection.name(), "architecture");
//!
//! // Create with builder
//! let collection = CollectionNode::builder("my-collection".to_string()).build();
//! ```

use crate::models::{Node, ValidationError};
use serde_json::json;

/// Type-safe wrapper for collection nodes
///
/// Collections are organizational containers for grouping nodes.
/// The collection name is stored in the `content` field and must be globally unique.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::models::{Node, CollectionNode};
/// use serde_json::json;
///
/// let node = Node::new(
///     "collection".to_string(),
///     "my-collection".to_string(),
///     json!({}),
/// );
/// let collection = CollectionNode::from_node(node).unwrap();
/// assert_eq!(collection.name(), "my-collection");
/// ```
#[derive(Debug, Clone)]
pub struct CollectionNode {
    node: Node,
}

impl CollectionNode {
    /// Create a CollectionNode from an existing Node
    ///
    /// # Errors
    ///
    /// Returns `ValidationError::InvalidNodeType` if the node type is not "collection".
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::{Node, CollectionNode};
    /// use serde_json::json;
    ///
    /// let node = Node::new("collection".to_string(), "docs".to_string(), json!({}));
    /// let collection = CollectionNode::from_node(node).unwrap();
    /// ```
    pub fn from_node(node: Node) -> Result<Self, ValidationError> {
        if node.node_type != "collection" {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected 'collection', got '{}'",
                node.node_type
            )));
        }
        Ok(Self { node })
    }

    /// Create a builder for a new CollectionNode with the given name
    ///
    /// Returns a builder for setting additional properties.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::models::CollectionNode;
    ///
    /// let collection = CollectionNode::builder("my-collection".to_string())
    ///     .build();
    /// ```
    pub fn builder(name: String) -> CollectionNodeBuilder {
        CollectionNodeBuilder { name }
    }

    /// Get the collection name
    ///
    /// The name is stored in the node's content field.
    pub fn name(&self) -> &str {
        &self.node.content
    }

    /// Get the collection ID
    pub fn id(&self) -> &str {
        &self.node.id
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

/// Builder for creating new CollectionNode instances
pub struct CollectionNodeBuilder {
    name: String,
}

impl CollectionNodeBuilder {
    /// Build the CollectionNode
    pub fn build(self) -> CollectionNode {
        let node = Node::new("collection".to_string(), self.name, json!({}));

        CollectionNode { node }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collection_node_from_node() {
        let node = Node::new(
            "collection".to_string(),
            "test-collection".to_string(),
            json!({}),
        );
        let collection = CollectionNode::from_node(node).unwrap();
        assert_eq!(collection.name(), "test-collection");
    }

    #[test]
    fn test_collection_node_from_wrong_type() {
        let node = Node::new(
            "text".to_string(),
            "not a collection".to_string(),
            json!({}),
        );
        let result = CollectionNode::from_node(node);
        assert!(result.is_err());
    }

    #[test]
    fn test_collection_node_builder() {
        let collection = CollectionNode::builder("my-collection".to_string()).build();
        assert_eq!(collection.name(), "my-collection");
        assert_eq!(collection.as_node().node_type, "collection");
    }

    #[test]
    fn test_collection_node_into_node() {
        let collection = CollectionNode::builder("test".to_string()).build();
        let node = collection.into_node();
        assert_eq!(node.node_type, "collection");
        assert_eq!(node.content, "test");
    }
}
