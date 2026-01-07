//! Data Models
//!
//! This module contains the core data structures used throughout NodeSpace:
//!
//! - `Node` - Universal node model for all content types
//! - `Embedding` - Vector embeddings for semantic search (root-aggregate model)
//! - Type-safe wrappers (TaskNode, TextNode, DateNode, CodeBlockNode, QuoteBlockNode, OrderedListNode, CollectionNode) for ergonomic access
//! - Core schema definitions for built-in node types
//!
//! All entities use the Pure JSON schema approach with data stored in the
//! `properties` field of the universal `nodes` table.

pub mod core_schemas;
pub mod embedding;
mod node;
pub mod schema;
pub mod time;

// Type-safe node wrappers
mod collection_node;
mod date_node;
mod schema_node;
mod task_node;
mod text_node;

#[cfg(test)]
#[path = "task_node_test.rs"]
mod task_node_test;

#[cfg(test)]
#[path = "text_node_test.rs"]
mod text_node_test;

#[cfg(test)]
#[path = "date_node_test.rs"]
mod date_node_test;

// Type-safe wrappers for core node types
pub mod code_block_node;
#[cfg(test)]
#[path = "code_block_node_test.rs"]
mod code_block_node_test;

pub mod quote_block_node;
#[cfg(test)]
#[path = "quote_block_node_test.rs"]
mod quote_block_node_test;

pub mod ordered_list_node;
#[cfg(test)]
#[path = "ordered_list_node_test.rs"]
mod ordered_list_node_test;

pub use code_block_node::{CodeBlockNode, CodeBlockValidationError};
pub use node::{
    DeleteResult, FilterOperator, Node, NodeFilter, NodeQuery, NodeUpdate, OrderBy, PropertyFilter,
    ValidationError,
};
pub use ordered_list_node::{OrderedListNode, OrderedListValidationError};
pub use quote_block_node::{QuoteBlockNode, QuoteBlockValidationError};
pub use schema::{SchemaField, SchemaProtectionLevel};
pub use time::{SystemTimeProvider, TimeProvider};

// Export type-safe wrappers
pub use collection_node::CollectionNode;
pub use date_node::DateNode;
pub use embedding::{
    is_embeddable_type, ChunkInfo, Embedding, EmbeddingConfig, EmbeddingSearchResult, NewEmbedding,
    EMBEDDABLE_NODE_TYPES,
};
pub use schema_node::SchemaNode;
pub use task_node::{TaskNode, TaskNodeUpdate, TaskPriority, TaskStatus};
pub use text_node::TextNode;

/// Convert a Node to its strongly-typed JSON representation (Issue #673)
///
/// For complex types (task, schema), converts to the typed struct
/// which provides proper field structure. For simple types, returns the generic Node.
///
/// This is the canonical implementation - all entry points (MCP, Tauri, HTTP)
/// should use this function and map the error to their own error type.
///
/// # Example
///
/// ```ignore
/// // In MCP handler
/// node_to_typed_value(node).map_err(MCPError::internal_error)?
///
/// // In Tauri command
/// node_to_typed_value(node).map_err(|e| CommandError { message: e, ... })?
/// ```
pub fn node_to_typed_value(node: Node) -> Result<serde_json::Value, String> {
    match node.node_type.as_str() {
        "task" => {
            let task = TaskNode::from_node(node).map_err(|e| e.to_string())?;
            serde_json::to_value(task)
        }
        "schema" => {
            let schema = SchemaNode::from_node(node).map_err(|e| e.to_string())?;
            serde_json::to_value(schema)
        }
        _ => serde_json::to_value(node),
    }
    .map_err(|e| format!("Failed to serialize node: {}", e))
}

/// Convert a list of Nodes to their strongly-typed JSON representations (Issue #673)
pub fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<serde_json::Value>, String> {
    nodes.into_iter().map(node_to_typed_value).collect()
}
