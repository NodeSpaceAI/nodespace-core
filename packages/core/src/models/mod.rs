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

pub mod conflict;
pub mod core_schemas;
pub mod embedding;
mod node;
pub mod schema;
pub mod time;

// Type-safe node wrappers
mod ai_chat_node;
mod collection_node;
mod date_node;
mod schema_node;
mod task_node;
mod text_node;

#[cfg(test)]
#[path = "ai_chat_node_test.rs"]
mod ai_chat_node_test;

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

pub use ai_chat_node::{AiChatCompletedWrite, AiChatMessage, AiChatNode, AiChatResolvedEntity};
pub use code_block_node::{CodeBlockNode, CodeBlockValidationError};
pub use conflict::{ConflictKind, ConflictRecord, ConflictStatus, Resolution};
pub use node::{
    DeleteResult, FilterOperator, Node, NodeFilter, NodeQuery, NodeReference, NodeRelationship,
    NodeUpdate, OrderBy, PropertyFilter, TraversalDirection, ValidationError,
};
pub use ordered_list_node::{OrderedListNode, OrderedListValidationError};
pub use quote_block_node::{QuoteBlockNode, QuoteBlockValidationError};
pub use schema::{RelationshipDirection, SchemaField, SchemaProtectionLevel};
pub use time::{SystemTimeProvider, TimeProvider};

// Export type-safe wrappers
pub use collection_node::CollectionNode;
pub use date_node::DateNode;
pub use embedding::{ChunkInfo, Embedding, EmbeddingConfig, EmbeddingSearchResult, NewEmbedding};
pub use schema_node::SchemaNode;
pub use task_node::{TaskNode, TaskNodeUpdate, TaskPriority, TaskStatus};
pub use text_node::TextNode;

// node_to_typed_value and nodes_to_typed_values are the single canonical
// implementations in nodespace-types, re-exported here for all entry points.
pub use nodespace_types::{node_to_typed_value, nodes_to_typed_values};

// The lifecycle-status allow-list and its validator live in nodespace-types
// (owner of the `Node`/`NodeUpdate` types), re-exported here as the single
// source of truth for the storage-layer write guard.
pub use nodespace_types::{is_valid_lifecycle_status, LIFECYCLE_STATUSES};
