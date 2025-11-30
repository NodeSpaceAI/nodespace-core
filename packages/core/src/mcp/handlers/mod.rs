//! MCP Request Handlers
//!
//! Handler modules for different MCP operations.
//!
//! Note: Schema-specific handlers removed per Issue #690.
//! Use generic CRUD (create_node, update_node, query_nodes) for schema management.

pub mod initialize;
pub mod markdown;
pub mod natural_language_schema;
pub mod nodes;
pub mod search;
pub mod tools;
