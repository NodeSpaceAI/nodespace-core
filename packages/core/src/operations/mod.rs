//! Node Operations Types
//!
//! This module provides types used for node operations.
//!
//! **Note (Issue #676)**: The `NodeOperations` business logic layer has been merged into
//! `NodeService`. This module now only contains supporting types like `CreateNodeParams`
//! that are still used by MCP handlers and Tauri commands.

pub mod sibling_queue;

// Re-export types for convenience
pub use sibling_queue::SiblingOperationQueue;

use serde_json::Value;

/// Parameters for creating a node
///
/// This struct is used by `NodeService::create_node_with_parent()` to encapsulate
/// all parameters needed for node creation.
///
/// # ID Generation Strategy
///
/// The `id` field supports three distinct scenarios:
///
/// 1. **Frontend-provided UUID** (Tauri commands): The frontend pre-generates UUIDs for
///    optimistic UI updates and local state tracking (`persistedNodeIds`). This ensures
///    ID consistency between client and server, preventing sync issues.
///
/// 2. **Auto-generated UUID** (MCP handlers): Server-side generation for external clients
///    like AI assistants. This prevents ID conflicts and maintains security boundaries.
///
/// 3. **Date-based ID** (special case): Date nodes use their content (YYYY-MM-DD format)
///    as the ID, enabling predictable lookups and ensuring uniqueness by date.
///
/// # Security Considerations
///
/// When accepting frontend-provided IDs:
///
/// - **UUID validation**: Non-date nodes must provide valid UUID format. Invalid UUIDs
///   are rejected with `InvalidOperation` error.
/// - **Database constraints**: The database enforces UNIQUE constraint on `nodes.id`,
///   preventing collisions at the storage layer.
/// - **Trust boundary**: Only Tauri commands (trusted in-process frontend) can provide
///   custom IDs. MCP handlers (external AI clients) always use server-side generation.
///
/// # Examples
///
/// ```no_run
/// # use nodespace_core::operations::CreateNodeParams;
/// # use serde_json::json;
/// // Auto-generated ID (MCP path)
/// let params = CreateNodeParams {
///     id: None,
///     node_type: "text".to_string(),
///     content: "Hello World".to_string(),
///     parent_id: Some("parent-123".to_string()),
///     insert_after_node_id: None,
///     properties: json!({}),
/// };
///
/// // Frontend-provided UUID (Tauri path)
/// let frontend_id = uuid::Uuid::new_v4().to_string();
/// let params_with_id = CreateNodeParams {
///     id: Some(frontend_id),
///     node_type: "text".to_string(),
///     content: "Tracked by frontend".to_string(),
///     parent_id: None,
///     insert_after_node_id: None,
///     properties: json!({}),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CreateNodeParams {
    /// Optional ID for the node. If None, will be auto-generated (UUID for most types, content for date nodes)
    pub id: Option<String>,
    /// Type of the node (text, task, date, etc.)
    pub node_type: String,
    /// Content of the node
    pub content: String,
    /// Optional parent node ID (container/root will be auto-derived from parent chain)
    pub parent_id: Option<String>,
    /// Optional sibling to insert after (if None, appends to end)
    pub insert_after_node_id: Option<String>,
    /// Additional node properties as JSON
    pub properties: Value,
}
