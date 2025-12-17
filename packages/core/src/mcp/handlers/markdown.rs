//! MCP Markdown Import Handler
//!
//! Parses markdown content and creates hierarchical NodeSpace nodes.
//! Preserves heading hierarchy and list indentation as parent-child relationships.
//!
//! As of Issue #676, all handlers use NodeService directly instead of NodeOperations.
//!
//! # Examples
//!
//! ```ignore
//! use serde_json::json;
//! use std::sync::Arc;
//! use nodespace_core::services::NodeService;
//! use nodespace_core::mcp::handlers::markdown::handle_create_nodes_from_markdown;
//!
//! async fn example(node_service: Arc<NodeService>) -> Result<(), Box<dyn std::error::Error>> {
//!     // With explicit title
//!     let params = json!({
//!         "markdown_content": "- Task 1\n- Task 2",
//!         "title": "# My Document"
//!     });
//!     let result = handle_create_nodes_from_markdown(&node_service, params).await?;
//!
//!     // Or let first line become the title automatically
//!     let params = json!({
//!         "markdown_content": "# My Document\n\n- Task 1\n- Task 2"
//!     });
//!     let result = handle_create_nodes_from_markdown(&node_service, params).await?;
//!     println!("Created {} nodes", result["nodes_created"]);
//!     Ok(())
//! }
//! ```

use crate::mcp::types::MCPError;
use crate::models::{Node, TaskNode, TaskStatus};
use crate::services::{CollectionService, CreateNodeParams, NodeService, NodeServiceError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Maximum markdown content size (1MB) to prevent resource exhaustion
///
/// **Rationale:** Prevents DoS attacks from malicious AI agents attempting to import
/// extremely large markdown files. 1MB is sufficient for typical documents (100-1000 nodes)
/// while protecting against memory exhaustion. Most markdown notes are < 100KB.
const MAX_MARKDOWN_SIZE: usize = 1_000_000;

/// Maximum number of nodes that can be created in a single import
const MAX_NODES_PER_IMPORT: usize = 1000;

/// Maximum content length (in characters) for a text node to be rendered as a bullet item
///
/// Text nodes longer than this are treated as standalone paragraphs, not bullet items.
/// This helps distinguish between short list items and longer descriptive text.
const MAX_BULLET_CONTENT_LENGTH: usize = 100;

// ============================================================================
// Two-Phase Batch Creation (Issue #737)
// ============================================================================

/// A node prepared for bulk insertion with pre-calculated hierarchy
///
/// This struct represents a node that has been parsed from markdown but not yet
/// inserted into the database. All IDs, parent references, and ordering are
/// pre-calculated in memory for efficient batch insertion.
#[derive(Debug, Clone)]
pub struct PreparedNode {
    /// Pre-assigned UUID (generated before DB access)
    pub id: String,
    /// Node type (text, header, task, etc.)
    pub node_type: String,
    /// Node content
    pub content: String,
    /// Parent node ID (references another PreparedNode.id or existing node)
    pub parent_id: Option<String>,
    /// Pre-calculated fractional order for sibling positioning
    pub order: f64,
    /// Node properties (status for tasks, etc.)
    pub properties: Value,
}

impl PreparedNode {
    /// Create a new prepared node with pre-assigned ID
    pub fn new(
        id: String,
        node_type: &str,
        content: String,
        parent_id: Option<String>,
        order: f64,
        properties: Value,
    ) -> Self {
        Self {
            id,
            node_type: node_type.to_string(),
            content,
            parent_id,
            order,
            properties,
        }
    }
}

/// Context for in-memory node preparation (no database access)
///
/// Similar to ParserContext but designed for the batch preparation phase.
/// Tracks hierarchy purely in memory with pre-assigned IDs.
struct PrepareContext {
    /// Stack tracking heading hierarchy (h1 → h2 → h3)
    heading_stack: Vec<(String, usize)>, // (node_id, level)
    /// Counter for fractional ordering per parent
    order_per_parent: HashMap<String, f64>,
}

impl PrepareContext {
    fn new(_root_id: Option<String>) -> Self {
        Self {
            heading_stack: Vec::new(),
            order_per_parent: HashMap::new(),
        }
    }

    /// Get current parent ID from heading stack
    fn current_parent_id(&self) -> Option<String> {
        self.heading_stack.last().map(|(id, _)| id.clone())
    }

    /// Pop headings at same or higher level for new heading insertion
    fn pop_headings_for_level(&mut self, level: usize) {
        while let Some((_, stack_level)) = self.heading_stack.last() {
            if *stack_level >= level {
                self.heading_stack.pop();
            } else {
                break;
            }
        }
    }

    /// Add heading to hierarchy stack
    fn push_heading(&mut self, node_id: String, level: usize) {
        self.heading_stack.push((node_id, level));
    }

    /// Get next order value for a parent
    fn next_order(&mut self, parent_id: &Option<String>) -> f64 {
        let key = parent_id.clone().unwrap_or_default();
        let current = self.order_per_parent.get(&key).copied().unwrap_or(0.0);
        let next = current + 1.0;
        self.order_per_parent.insert(key, next);
        next
    }
}

/// Parse markdown into prepared nodes without any database access (Phase 1)
///
/// This is the first phase of the two-phase batch creation optimization.
/// All nodes are prepared in memory with pre-assigned UUIDs and calculated
/// fractional orders, ready for bulk insertion.
///
/// # Arguments
///
/// * `markdown` - The markdown content to parse
/// * `root_id` - Optional existing root node ID (nodes will be children of this)
///
/// # Returns
///
/// Vector of PreparedNode ready for bulk database insertion
pub fn prepare_nodes_from_markdown(
    markdown: &str,
    root_id: Option<String>,
) -> Result<Vec<PreparedNode>, MCPError> {
    let mut prepared_nodes = Vec::new();
    let mut context = PrepareContext::new(root_id.clone());

    // If we have a root_id, treat it as a level-0 heading parent
    if let Some(ref rid) = root_id {
        context.push_heading(rid.clone(), 0);
    }

    // Track indentation-based hierarchy (node_id, indent_level)
    let mut indent_stack: Vec<(String, usize)> = Vec::new();

    // Track last text paragraph for bullet/ordered-list hierarchy
    let mut last_text_node: Option<(String, usize)> = None;

    // Track last content node for code-block/quote-block hierarchy
    let mut last_content_node: Option<String> = None;

    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        // Skip empty lines
        while i < lines.len() && lines[i].trim().is_empty() {
            i += 1;
        }

        if i >= lines.len() {
            break;
        }

        let line = lines[i];
        let indent_level = calculate_indent(line);
        let trimmed = line.trim_start();

        // Check for bullet list item
        let is_bullet = is_bullet_line(trimmed);
        let content_line = if is_bullet {
            trimmed.strip_prefix("- ").unwrap_or(trimmed)
        } else {
            trimmed
        };

        // Detect node type and extract content
        let (node_type, content, heading_level, is_multiline, properties) =
            if let Some(level) = detect_heading(content_line) {
                ("header", content_line.to_string(), Some(level), false, None)
            } else if is_task_line(content_line) {
                let (task_content, task_status) = if content_line.starts_with("- [x] ") {
                    (
                        content_line.strip_prefix("- [x] ").unwrap_or(content_line),
                        "done",
                    )
                } else if content_line.starts_with("- [ ] ") {
                    (
                        content_line.strip_prefix("- [ ] ").unwrap_or(content_line),
                        "open",
                    )
                } else {
                    (content_line, "open")
                };
                (
                    "task",
                    task_content.to_string(),
                    None,
                    false,
                    Some(json!({"status": task_status})),
                )
            } else if content_line.starts_with("```") {
                // Code block
                let mut code_lines = vec![content_line];
                i += 1;
                while i < lines.len() {
                    let code_line = lines[i];
                    code_lines.push(code_line);
                    if code_line.trim_start().starts_with("```") {
                        break;
                    }
                    i += 1;
                }
                ("code-block", code_lines.join("\n"), None, true, None)
            } else if content_line.starts_with("> ") {
                // Quote block
                let mut quote_lines = vec![content_line];
                while i + 1 < lines.len() && lines[i + 1].trim_start().starts_with("> ") {
                    i += 1;
                    quote_lines.push(lines[i].trim_start());
                }
                ("quote-block", quote_lines.join("\n"), None, true, None)
            } else if let Some(num_end) = detect_ordered_list(content_line) {
                // Ordered list
                let first_item_content = &content_line[num_end + 2..];
                let mut list_items = vec![format!("1. {}", first_item_content)];
                let mut j = i + 1;
                while j < lines.len() {
                    if lines[j].trim().is_empty() {
                        j += 1;
                        continue;
                    }
                    let next_line = lines[j].trim_start();
                    if let Some(next_num_end) = detect_ordered_list(next_line) {
                        i = j;
                        list_items.push(format!("1. {}", &next_line[next_num_end + 2..]));
                        j += 1;
                    } else {
                        break;
                    }
                }
                ("ordered-list", list_items.join("\n"), None, true, None)
            } else {
                // Text paragraph
                let mut text_lines = vec![content_line];
                let mut j = i + 1;
                while j < lines.len() {
                    let mut empty_count = 0;
                    while j < lines.len() && lines[j].trim().is_empty() {
                        empty_count += 1;
                        j += 1;
                    }
                    if empty_count >= 1 || j >= lines.len() {
                        break;
                    }
                    let next_line = lines[j].trim_start();
                    let is_special = detect_heading(next_line).is_some()
                        || next_line.starts_with("- ")
                        || next_line.starts_with("```")
                        || next_line.starts_with("> ")
                        || detect_ordered_list(next_line).is_some();
                    if is_special {
                        break;
                    }
                    text_lines.push(next_line);
                    i = j;
                    j += 1;
                }
                (
                    "text",
                    text_lines.join("\n"),
                    None,
                    text_lines.len() > 1,
                    None,
                )
            };

        // Pop indent stack for same or lower indentation
        while let Some((_, stack_indent)) = indent_stack.last() {
            if *stack_indent >= indent_level {
                indent_stack.pop();
            } else {
                break;
            }
        }

        // Determine parent based on hierarchy rules
        let parent_id = if node_type == "code-block" || node_type == "quote-block" {
            last_content_node
                .clone()
                .or_else(|| context.current_parent_id())
        } else if is_bullet && !is_multiline {
            if indent_level > 0 {
                indent_stack
                    .last()
                    .map(|(id, _)| id.clone())
                    .or_else(|| last_text_node.as_ref().map(|(id, _)| id.clone()))
                    .or_else(|| context.current_parent_id())
            } else {
                last_text_node
                    .as_ref()
                    .map(|(id, _)| id.clone())
                    .or_else(|| context.current_parent_id())
            }
        } else if node_type == "ordered-list" {
            last_text_node
                .as_ref()
                .map(|(id, _)| id.clone())
                .or_else(|| context.current_parent_id())
        } else if let Some(h_level) = heading_level {
            context.pop_headings_for_level(h_level);
            indent_stack
                .last()
                .map(|(id, _)| id.clone())
                .or_else(|| context.current_parent_id())
        } else {
            indent_stack
                .last()
                .map(|(id, _)| id.clone())
                .or_else(|| context.current_parent_id())
        };

        // Generate UUID and calculate order
        let node_id = uuid::Uuid::new_v4().to_string();
        let order = context.next_order(&parent_id);

        // Build properties
        let final_properties = properties.unwrap_or_else(|| {
            if node_type == "task" {
                json!({"status": "open"})
            } else {
                json!({})
            }
        });

        // Create prepared node
        prepared_nodes.push(PreparedNode::new(
            node_id.clone(),
            node_type,
            content,
            parent_id,
            order,
            final_properties,
        ));

        // Update hierarchy tracking
        if let Some(h_level) = heading_level {
            context.push_heading(node_id.clone(), h_level);
        }

        if node_type == "text" && !is_multiline && !is_bullet {
            last_text_node = Some((node_id.clone(), indent_level));
        } else if node_type != "text" {
            last_text_node = None;
        }

        if node_type == "header" || (node_type == "text" && !is_bullet) {
            last_content_node = Some(node_id.clone());
        }

        if heading_level.is_none() && (indent_level > 0 || is_bullet) {
            indent_stack.push((node_id, indent_level));
        }

        i += 1;
    }

    Ok(prepared_nodes)
}

/// Parameters for create_nodes_from_markdown method
#[derive(Debug, Deserialize)]
pub struct CreateNodesFromMarkdownParams {
    /// Markdown content to parse into nodes.
    ///
    /// If `title` is provided, all nodes from markdown_content become children of the title node.
    /// If `title` is NOT provided, the first line of markdown_content becomes the root node,
    /// and remaining content becomes children of that root.
    ///
    /// Example with title:
    /// ```text
    /// title: "# Project Alpha"
    /// markdown_content: "## Task 1\nDescription here"
    /// // Creates: "# Project Alpha" (root) -> "## Task 1" (child) -> "Description" (child)
    /// ```
    ///
    /// Example without title (auto-extract first line):
    /// ```text
    /// markdown_content: "# Project Alpha\n## Task 1\nDescription here"
    /// // Creates: "# Project Alpha" (root) -> "## Task 1" (child) -> "Description" (child)
    /// ```
    pub markdown_content: String,

    /// Optional title for the root node.
    ///
    /// If provided, creates a separate root node that all markdown_content nodes
    /// will be children of. Can be:
    /// - A date string (YYYY-MM-DD) to use/create a date root
    /// - Markdown text (e.g., "# My Document" or "Project Notes") to create a text/header root
    ///
    /// If NOT provided, the first line of markdown_content is used as the root node title.
    ///
    /// The parsed root type must be text, header, or date.
    /// Multi-line types (code-block, quote-block, ordered-list) cannot be roots.
    #[serde(default)]
    pub title: Option<String>,

    /// Sync import mode for testing only.
    ///
    /// When false (default), returns immediately with just `root_id`.
    /// Children are inserted in a background task (fire-and-forget).
    ///
    /// When true, waits for all nodes to be created before responding.
    /// This is only useful for tests that need to verify node creation.
    #[serde(default)]
    pub sync_import: bool,

    /// Optional collection path to add the root node to (e.g., "hr:policy:vacation")
    /// Creates collections along the path if they don't exist.
    #[serde(default)]
    pub collection: Option<String>,
}

/// Metadata for a created node (id + type)
#[derive(Debug, Clone, Serialize)]
pub struct NodeMetadata {
    pub id: String,
    pub node_type: String,
}

/// Strategy for determining the container node
#[derive(Debug, Clone)]
enum ContainerStrategy {
    /// Use a date node as the container (auto-created if needed)
    DateContainer(String),
    /// Parse container_title as markdown to create the container node
    /// The parsed node type must be text or header (not multi-line types)
    TitleAsContainer(String),
}

/// Tracks a node in the hierarchy during parsing
#[derive(Debug, Clone)]
struct HierarchyNode {
    node_id: String,
    level: usize,
}

/// Context for building the node hierarchy
struct ParserContext {
    /// Stack tracking heading hierarchy (h1 → h2 → h3)
    heading_stack: Vec<HierarchyNode>,
    /// Stack tracking list indentation
    list_stack: Vec<HierarchyNode>,
    /// Last sibling at current level for ordering
    last_sibling: Option<String>,
    /// All created node IDs (for backward compatibility)
    node_ids: Vec<String>,
    /// All created nodes with metadata (id + type)
    nodes: Vec<NodeMetadata>,
    /// Root node ID (determined by strategy)
    root_id: Option<String>,
    /// Whether the first node has been created (for tracking purposes)
    first_node_created: bool,
}

impl ParserContext {
    fn new_with_strategy(strategy: ContainerStrategy) -> Self {
        // For DateContainer strategy, set root_id immediately
        let root_id = match &strategy {
            ContainerStrategy::DateContainer(date) => Some(date.clone()),
            ContainerStrategy::TitleAsContainer(_) => None, // Will be set after parsing title
        };

        Self {
            heading_stack: Vec::new(),
            list_stack: Vec::new(),
            last_sibling: None,
            node_ids: Vec::new(),
            nodes: Vec::new(),
            root_id,
            first_node_created: false,
        }
    }

    /// Create a parser context for an existing container
    ///
    /// This is a convenience constructor that properly initializes the parser
    /// state for adding children to an existing container node. It sets up the
    /// heading stack and parser flags to treat the existing container as the
    /// root of the hierarchy.
    ///
    /// # Arguments
    ///
    /// * `container_id` - ID of the existing container node
    /// * `container_content` - Content of the existing container (typically the title)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let context = ParserContext::new_for_existing_container(
    ///     "container-123".to_string(),
    ///     "# Project Plan".to_string()
    /// );
    /// // Context is ready to parse markdown and create children under container-123
    /// ```
    fn new_for_existing_container(container_id: String, container_content: String) -> Self {
        let mut context =
            Self::new_with_strategy(ContainerStrategy::TitleAsContainer(container_content));

        // Set root_id to the existing container
        context.root_id = Some(container_id.clone());

        // Set up initial heading stack with container as root (level 0)
        // This makes all parsed nodes children of the container
        context.push_heading(container_id, 0);

        // Mark that we already have a container (skip title parsing)
        context.first_node_created = true;

        context
    }

    /// Get current parent ID based on hierarchy context
    fn current_parent_id(&self) -> Option<String> {
        // List context takes precedence over heading context
        if let Some(list_parent) = self.list_stack.last() {
            return Some(list_parent.node_id.clone());
        }

        if let Some(heading_parent) = self.heading_stack.last() {
            return Some(heading_parent.node_id.clone());
        }

        None
    }

    /// Pop headings at same or higher level to prepare for a new heading
    /// This must be called BEFORE creating the heading node to get the correct parent
    fn pop_headings_for_level(&mut self, level: usize) {
        // Pop headings at same or higher level
        while let Some(top) = self.heading_stack.last() {
            if top.level >= level {
                self.heading_stack.pop();
            } else {
                break;
            }
        }
    }

    /// Add a heading to the hierarchy stack after it's been created
    fn push_heading(&mut self, node_id: String, level: usize) {
        self.heading_stack.push(HierarchyNode { node_id, level });
        self.last_sibling = None; // Reset sibling tracking at new heading level
    }

    /// Track a node for sibling ordering
    fn track_node(&mut self, node_id: String, node_type: String) {
        self.last_sibling = Some(node_id.clone());
        self.node_ids.push(node_id.clone());
        self.nodes.push(NodeMetadata {
            id: node_id,
            node_type,
        });
    }
}

/// Handle create_nodes_from_markdown MCP request
pub async fn handle_create_nodes_from_markdown<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    let start = std::time::Instant::now();

    let params: CreateNodesFromMarkdownParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Validate markdown content size
    if params.markdown_content.len() > MAX_MARKDOWN_SIZE {
        return Err(MCPError::invalid_params(format!(
            "Markdown content exceeds maximum size of {} bytes (got {} bytes)",
            MAX_MARKDOWN_SIZE,
            params.markdown_content.len()
        )));
    }

    // Determine title and remaining content
    // If title is provided, use it; otherwise extract first line from markdown_content
    let (title, remaining_content) = if let Some(ref title) = params.title {
        // Title provided explicitly - use markdown_content as children
        (title.clone(), params.markdown_content.clone())
    } else {
        // No title - extract first non-empty line from markdown_content
        let lines: Vec<&str> = params.markdown_content.lines().collect();
        let first_line = lines
            .iter()
            .find(|line| !line.trim().is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                MCPError::invalid_params(
                    "markdown_content is empty and no title provided".to_string(),
                )
            })?;

        // Find where the first non-empty line is and take everything after it
        let first_line_idx = lines
            .iter()
            .position(|line| !line.trim().is_empty())
            .unwrap_or(0);
        let remaining = lines[first_line_idx + 1..].join("\n");

        (first_line, remaining)
    };

    // Determine container strategy based on title
    let container_strategy = if is_date_format(&title) {
        // title is a date → use date as container
        ContainerStrategy::DateContainer(title.clone())
    } else {
        // title is markdown → parse it to create the container node
        ContainerStrategy::TitleAsContainer(title.clone())
    };

    // ============================================================================
    // Two-Phase Batch Creation (Issue #737)
    // Phase 1: Create container/root node (single node, uses existing path)
    // Phase 2: Batch create all children in a single transaction
    // ============================================================================

    let mut all_node_ids: Vec<String> = Vec::new();
    let mut all_nodes: Vec<NodeMetadata> = Vec::new();
    let root_id: String;

    match container_strategy {
        ContainerStrategy::DateContainer(ref date_id) => {
            // Date container - ensure it exists
            node_service
                .ensure_date_exists(date_id)
                .await
                .map_err(|e| {
                    MCPError::internal_error(format!("Failed to create date container: {}", e))
                })?;
            root_id = date_id.clone();
        }
        ContainerStrategy::TitleAsContainer(ref title_content) => {
            // Parse title to determine node type (header or text)
            let title_prepared = prepare_nodes_from_markdown(title_content, None)?;

            if title_prepared.is_empty() {
                return Err(MCPError::invalid_params(
                    "title could not be parsed into a node".to_string(),
                ));
            }

            if title_prepared.len() != 1 {
                return Err(MCPError::invalid_params(format!(
                    "container_title must parse to exactly one node, got {}",
                    title_prepared.len()
                )));
            }

            let container = &title_prepared[0];
            if !is_valid_container_type(&container.node_type) {
                return Err(MCPError::invalid_params(format!(
                    "container_title parsed to '{}' which cannot be a container. Only text, header, or date nodes can be containers.",
                    container.node_type
                )));
            }

            // Create the root/container node (single node, not batched)
            // Root nodes have no parent
            let container_id = node_service
                .create_node_with_parent(CreateNodeParams {
                    id: Some(container.id.clone()),
                    node_type: container.node_type.clone(),
                    content: container.content.clone(),
                    parent_id: None, // Root node
                    insert_after_node_id: None,
                    properties: container.properties.clone(),
                })
                .await
                .map_err(|e| {
                    MCPError::node_creation_failed(format!("Failed to create container: {}", e))
                })?;

            root_id = container_id.clone();
            all_node_ids.push(container_id.clone());
            all_nodes.push(NodeMetadata {
                id: container_id,
                node_type: container.node_type.clone(),
            });
        }
    }

    // Phase 2: Prepare and insert children
    // Default: Fire-and-forget async for fast MCP response
    // sync_import=true: Wait for completion (for tests only)
    if !remaining_content.trim().is_empty() {
        let prepared_children =
            prepare_nodes_from_markdown(&remaining_content, Some(root_id.clone()))?;

        // Validate node count limit
        if prepared_children.len() + all_nodes.len() > MAX_NODES_PER_IMPORT {
            return Err(MCPError::invalid_params(format!(
                "Import would create {} nodes, exceeding maximum of {}",
                prepared_children.len() + all_nodes.len(),
                MAX_NODES_PER_IMPORT
            )));
        }

        if !prepared_children.is_empty() {
            // Convert PreparedNode to tuple format for bulk_create_hierarchy
            let nodes_for_bulk: Vec<(
                String,
                String,
                String,
                Option<String>,
                f64,
                serde_json::Value,
            )> = prepared_children
                .iter()
                .map(|n| {
                    (
                        n.id.clone(),
                        n.node_type.clone(),
                        n.content.clone(),
                        n.parent_id.clone(),
                        n.order,
                        n.properties.clone(),
                    )
                })
                .collect();

            if params.sync_import {
                // Sync mode (for tests): Insert synchronously and wait
                let created_ids = node_service
                    .bulk_create_hierarchy(nodes_for_bulk)
                    .await
                    .map_err(|e| {
                        MCPError::internal_error(format!("Bulk creation failed: {}", e))
                    })?;

                // Track all created nodes (only needed for sync mode)
                for (i, child) in prepared_children.iter().enumerate() {
                    all_node_ids.push(created_ids[i].clone());
                    all_nodes.push(NodeMetadata {
                        id: created_ids[i].clone(),
                        node_type: child.node_type.clone(),
                    });
                }
            } else {
                // Default: Fire-and-forget async for fast MCP response
                // Use Handle::try_current().spawn() to ensure we're spawning on
                // the current runtime (works in both standalone tokio and Tauri)
                let node_service = Arc::clone(node_service);
                let root_id_for_log = root_id.clone();

                // Get the current tokio runtime handle and spawn on it
                // This should work in both standalone tokio and Tauri contexts
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let insert_start = std::time::Instant::now();
                        match node_service.bulk_create_hierarchy(nodes_for_bulk).await {
                            Ok(created_ids) => {
                                tracing::info!(
                                    root_id = %root_id_for_log,
                                    nodes_created = created_ids.len(),
                                    duration_ms = insert_start.elapsed().as_millis(),
                                    "Background markdown import completed"
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    root_id = %root_id_for_log,
                                    error = %e,
                                    "Background markdown import failed"
                                );
                            }
                        }
                    });
                } else {
                    // Fallback: No tokio runtime available, do synchronous import
                    tracing::warn!("No tokio runtime available, falling back to sync import");
                    let _ = node_service
                        .bulk_create_hierarchy(nodes_for_bulk)
                        .await
                        .map_err(|e| {
                            tracing::error!(error = %e, "Sync fallback bulk creation failed");
                        });
                }
            }
        }
    }

    // Add root node to collection if specified
    let _collection_id = if let Some(path) = &params.collection {
        let collection_service = CollectionService::new(&node_service.store);
        let resolved = collection_service
            .add_to_collection_by_path(&root_id, path)
            .await
            .map_err(|e| match e {
                NodeServiceError::InvalidCollectionPath(msg) => {
                    MCPError::invalid_params(format!("Invalid collection path: {}", msg))
                }
                NodeServiceError::CollectionCycle(msg) => {
                    MCPError::invalid_params(format!("Collection cycle detected: {}", msg))
                }
                _ => MCPError::internal_error(format!("Failed to add to collection: {}", e)),
            })?;
        Some(resolved.leaf_id().to_string())
    } else {
        None
    };

    // Return minimal response - client can fetch details via get_markdown_from_node_id if needed
    // For sync mode (tests), include full details for verification
    // For async mode (default), children are still being created in background
    if params.sync_import {
        let duration_ms = start.elapsed().as_millis();
        tracing::info!(
            duration_ms = duration_ms,
            nodes_created = all_nodes.len(),
            sync_import = true,
            "Markdown import completed (sync)"
        );
        // Sync mode: Include full details for test verification
        Ok(json!({
            "success": true,
            "root_id": root_id,
            "nodes_created": all_nodes.len(),
            "node_ids": all_node_ids,
            "nodes": all_nodes,
            "duration_ms": duration_ms
        }))
    } else {
        // Async mode (default): Return immediately with just root_id
        tracing::info!(
            root_id = %root_id,
            "Markdown import initiated (async)"
        );
        Ok(json!({
            "success": true,
            "root_id": root_id
        }))
    }
}

/// Check if a string matches the date format YYYY-MM-DD
fn is_date_format(s: &str) -> bool {
    use chrono::NaiveDate;
    NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// Check if a node type can be a container
/// Only text, header, and date nodes can be containers (semantically meaningful)
/// Multi-line nodes (code-block, quote-block, ordered-list) cannot be containers
fn is_valid_container_type(node_type: &str) -> bool {
    matches!(node_type, "text" | "header" | "date")
}

/// Detect if a line is a markdown heading
///
/// Returns the heading level (1-6) if the line is a valid heading, None otherwise.
/// Valid headings have 1-6 '#' symbols followed by a space.
///
/// # Examples
/// ```ignore
/// assert_eq!(detect_heading("# Title"), Some(1));
/// assert_eq!(detect_heading("### Subtitle"), Some(3));
/// assert_eq!(detect_heading("#NoSpace"), None);  // Not a heading
/// ```
fn detect_heading(line: &str) -> Option<usize> {
    if !line.starts_with('#') {
        return None;
    }

    let level = line.chars().take_while(|c| *c == '#').count();

    // Verify it's actually a header (has space after #'s)
    if line.chars().nth(level) == Some(' ') && level <= 6 {
        Some(level)
    } else {
        None
    }
}

/// Check if a line is a task (checked or unchecked)
///
/// Tasks are lines starting with "- [ ] " (unchecked) or "- [x] " (checked).
fn is_task_line(line: &str) -> bool {
    line.starts_with("- [ ] ") || line.starts_with("- [x] ")
}

/// Check if a line is a bullet list item (not a task or link)
///
/// Bullets start with "- " but are not tasks ("- [ ]") or links ("- [text](url)").
fn is_bullet_line(line: &str) -> bool {
    if !line.starts_with("- ") {
        return false;
    }

    // Exclude tasks
    if is_task_line(line) {
        return false;
    }

    // Exclude markdown links: "- [text](url)"
    if line.starts_with("- [") && line.contains("](") {
        return false;
    }

    true
}

/// Calculate indentation level from leading whitespace
///
/// Counts leading tabs (1 tab = 4 spaces) and spaces.
/// Only counts actual indentation characters (tabs and spaces), not all whitespace.
///
/// # Examples
/// ```ignore
/// assert_eq!(calculate_indent("    text"), 4);   // 4 spaces
/// assert_eq!(calculate_indent("\ttext"), 4);     // 1 tab = 4 spaces
/// assert_eq!(calculate_indent("  - item"), 2);  // 2 spaces
/// ```
fn calculate_indent(line: &str) -> usize {
    let indent_chars: String = line
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t') // Only tabs and spaces
        .collect();

    indent_chars.chars().filter(|c| *c == '\t').count() * 4
        + indent_chars.chars().filter(|c| *c == ' ').count()
}

/// Check if a line starts an ordered list
///
/// Returns the position after the number and period if this is an ordered list item.
/// Valid ordered lists: "1. Item", "42. Item", etc.
/// Invalid: "sentence 1. next" (period too far from start)
///
/// # Examples
/// ```ignore
/// assert_eq!(detect_ordered_list("1. Item"), Some(1));
/// assert_eq!(detect_ordered_list("42. Item"), Some(2));
/// assert_eq!(detect_ordered_list("This is 1. Not a list"), None);
/// ```
fn detect_ordered_list(line: &str) -> Option<usize> {
    if let Some(num_end) = line.find(". ") {
        // Check that it starts with digit(s) and is reasonably positioned
        if line[..num_end].chars().all(|c| c.is_ascii_digit()) && num_end > 0 && num_end < 5
        // Reasonable list number (1-9999)
        {
            return Some(num_end);
        }
    }
    None
}

/// Parse markdown content and create nodes
///
/// This function processes markdown line-by-line, preserving inline formatting
/// and using indentation + heading levels to determine hierarchy.
///
/// # Arguments
///
/// * `markdown` - The markdown content to parse
/// * `node_service` - NodeService for creating nodes in the database
/// * `context` - Parser context tracking hierarchy and node state
///
/// # Returns
///
/// Returns `Ok(())` on success, or an `MCPError` if node creation fails.
///
/// # Hierarchy Rules
///
/// 1. **Heading hierarchy by level**: H1 → H2 → H3, same-level headers are siblings
/// 2. **Content below headings are children**: Non-heading content becomes child of nearest heading above
/// 3. **Indentation hints (optional)**: Tab/space count before `-` indicates depth
/// 4. **Inline syntax preserved**: `**bold**`, `*italic*`, etc. kept intact
async fn parse_markdown<C>(
    markdown: &str,
    node_service: &Arc<NodeService<C>>,
    context: &mut ParserContext,
) -> Result<(), MCPError>
where
    C: surrealdb::Connection,
{
    // Track indentation-based hierarchy (node_id, indent_level)
    let mut indent_stack: Vec<(String, usize)> = Vec::new();

    // Track the last text paragraph for bullet/ordered-list hierarchy
    let mut last_text_node: Option<(String, usize)> = None; // (node_id, indent_level)

    // Track the last content node (header or text) for code-block/quote-block hierarchy
    // Code blocks and quote blocks should be children of the immediately preceding content
    let mut last_content_node: Option<String> = None; // node_id

    // Track last sibling created per parent to maintain document order
    // Key: parent_id (or empty string for root-level nodes)
    // Value: last sibling node_id created under that parent
    let mut last_sibling_per_parent: HashMap<String, String> = HashMap::new();

    // Process markdown line by line, collecting text paragraphs
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        // Count consecutive empty lines (for paragraph separation)
        while i < lines.len() && lines[i].trim().is_empty() {
            i += 1;
        }

        if i >= lines.len() {
            break;
        }

        let line = lines[i];

        // Detect indentation level using helper function
        let indent_level = calculate_indent(line);

        // Strip leading whitespace
        let trimmed = line.trim_start();

        // Check if this is a bullet list item (not a task or link) using helper functions
        let is_bullet = is_bullet_line(trimmed);
        let content_line = if is_bullet {
            trimmed
                .strip_prefix("- ")
                .expect("is_bullet_line guarantees '- ' prefix exists")
        } else {
            trimmed
        };

        // Detect node type and extract content with inline markdown preserved
        // Tuple: (node_type, content, heading_level, is_multiline, properties)
        let (node_type, content, heading_level, is_multiline, properties) =
            if let Some(level) = detect_heading(content_line) {
                ("header", content_line.to_string(), Some(level), false, None)
            } else if is_task_line(content_line) {
                // Task node - extract content and status from "- [ ] " or "- [x] " prefix
                // Status values follow TaskStatus enum: "open", "done" (not "completed")
                let (task_content, task_status) = if content_line.starts_with("- [x] ") {
                    (
                        content_line.strip_prefix("- [x] ").unwrap_or(content_line),
                        "done", // TaskStatus::Done
                    )
                } else if content_line.starts_with("- [ ] ") {
                    (
                        content_line.strip_prefix("- [ ] ").unwrap_or(content_line),
                        "open", // TaskStatus::Open
                    )
                } else {
                    (content_line, "open")
                };
                (
                    "task",
                    task_content.to_string(),
                    None,
                    false,
                    Some(json!({"status": task_status})),
                )
            } else if content_line.starts_with("```") {
                // Code block - collect until closing ```
                // IMPORTANT: Preserve original whitespace inside code blocks (don't trim_start)
                let mut code_lines = vec![content_line];
                i += 1;
                while i < lines.len() {
                    let code_line = lines[i];
                    // Keep the original line content to preserve indentation
                    code_lines.push(code_line);
                    if code_line.trim_start().starts_with("```") {
                        break;
                    }
                    i += 1;
                }
                let code_content = code_lines.join("\n");
                ("code-block", code_content, None, true, None)
            } else if content_line.starts_with("> ") {
                // Quote block - collect consecutive quote lines
                let mut quote_lines = vec![content_line];
                while i + 1 < lines.len() && lines[i + 1].trim_start().starts_with("> ") {
                    i += 1;
                    quote_lines.push(lines[i].trim_start());
                }
                let quote_content = quote_lines.join("\n");
                ("quote-block", quote_content, None, true, None)
            } else if let Some(num_end) = detect_ordered_list(content_line) {
                // Collect consecutive numbered items into single ordered-list node
                // Each line should start with "1. " as per requirement
                let first_item_content = &content_line[num_end + 2..]; // Skip "N. "
                let mut list_items = vec![format!("1. {}", first_item_content)];
                let mut j = i + 1;
                // Skip empty lines within the list
                while j < lines.len() {
                    if lines[j].trim().is_empty() {
                        j += 1;
                        continue;
                    }
                    let next_line = lines[j].trim_start();
                    if let Some(next_num_end) = detect_ordered_list(next_line) {
                        i = j;
                        let item_content = &next_line[next_num_end + 2..]; // Skip "N. "
                        list_items.push(format!("1. {}", item_content));
                        j += 1;
                    } else {
                        break;
                    }
                }
                let list_content = list_items.join("\n");
                ("ordered-list", list_content, None, true, None)
            } else {
                // Text paragraph - collect consecutive lines with NO empty lines between them
                let mut text_lines = vec![content_line];

                // Look ahead for more text lines (only merge if NO empty lines)
                let mut j = i + 1;
                while j < lines.len() {
                    // Check for empty lines
                    let mut empty_count = 0;
                    while j < lines.len() && lines[j].trim().is_empty() {
                        empty_count += 1;
                        j += 1;
                    }

                    if empty_count >= 1 || j >= lines.len() {
                        // Any empty line or end - stop paragraph
                        break;
                    }

                    if j < lines.len() {
                        let next_line = lines[j].trim_start();

                        // Check if next line is special syntax (stop paragraph)
                        let is_special = detect_heading(next_line).is_some()
                            || next_line.starts_with("- ")
                            || next_line.starts_with("```")
                            || next_line.starts_with("> ")
                            || detect_ordered_list(next_line).is_some();

                        if is_special {
                            break;
                        }

                        // Add to paragraph (no empty lines between)
                        text_lines.push(next_line);
                        i = j;
                        j += 1;
                    } else {
                        break;
                    }
                }

                let text_content = text_lines.join("\n");
                ("text", text_content, None, text_lines.len() > 1, None)
            };

        // Pop indent stack for items at same or lower indentation
        while let Some((_, stack_indent)) = indent_stack.last() {
            if *stack_indent >= indent_level {
                indent_stack.pop();
            } else {
                break;
            }
        }

        // Determine parent based on bullet/ordered-list rules, heading hierarchy, and indentation
        let parent_id = if node_type == "code-block" || node_type == "quote-block" {
            // Code blocks and quote blocks - children of immediately preceding content node
            // This creates semantic grouping: "Description text" → code example
            last_content_node
                .clone()
                .or_else(|| context.current_parent_id())
        } else if is_bullet && !is_multiline {
            // Bullets - should be children of preceding text paragraph OR indented under previous bullet
            // Check indent_stack first for indented bullets (child of previous bullet)
            // Then check last_text_node for bullets at base level (child of text paragraph)
            if indent_level > 0 {
                // Indented bullet - check if there's a parent at lower indent
                indent_stack
                    .last()
                    .map(|(id, _)| id.clone())
                    .or_else(|| {
                        // No indent parent, try text paragraph
                        last_text_node.as_ref().map(|(text_id, _)| text_id.clone())
                    })
                    .or_else(|| context.current_parent_id())
            } else {
                // Non-indented bullet - child of text paragraph
                if let Some((text_id, _)) = &last_text_node {
                    Some(text_id.clone())
                } else {
                    context.current_parent_id()
                }
            }
        } else if node_type == "ordered-list" {
            // Ordered lists - should be children of preceding header or text node
            // Check if there's a recent text node first
            if let Some((text_id, _)) = &last_text_node {
                Some(text_id.clone())
            } else {
                // No text node - use heading parent
                context.current_parent_id()
            }
        } else if let Some(h_level) = heading_level {
            // For headers: pop same-or-higher level headers, then get parent
            context.pop_headings_for_level(h_level);
            indent_stack
                .last()
                .map(|(id, _)| id.clone())
                .or_else(|| context.current_parent_id())
        } else {
            // For non-headers: prefer indentation parent, fallback to heading parent
            indent_stack
                .last()
                .map(|(id, _)| id.clone())
                .or_else(|| context.current_parent_id())
        };

        // Look up the last sibling created under this parent to maintain document order
        let parent_key = parent_id.clone().unwrap_or_default();
        let insert_after = last_sibling_per_parent.get(&parent_key).cloned();

        // Create the node (insert after last sibling to preserve document order)
        let node_id = create_node(
            node_service,
            node_type,
            &content,
            parent_id.clone(),
            context.root_id.clone(),
            insert_after, // Insert after last sibling to maintain document order
            properties,   // Custom properties (e.g., task status)
        )
        .await?;

        // Track this node as the last sibling for its parent
        last_sibling_per_parent.insert(parent_key, node_id.clone());

        // Update heading stack if this was a header
        if let Some(h_level) = heading_level {
            context.push_heading(node_id.clone(), h_level);
        }

        // Track last text node for bullet/ordered-list hierarchy
        // Only NON-BULLET text nodes should update last_text_node
        if node_type == "text" && !is_multiline && !is_bullet {
            last_text_node = Some((node_id.clone(), indent_level));
        } else if node_type != "text" {
            // Non-text node breaks the text context for bullets/lists
            last_text_node = None;
        }

        // Track last content node (header or text) for code-block/quote-block hierarchy
        // Code-blocks and quote-blocks become children of the immediately preceding content
        if node_type == "header" || (node_type == "text" && !is_bullet) {
            last_content_node = Some(node_id.clone());
        }

        // Push to indent stack if this line had indentation OR is a bullet (for nested bullets)
        // Headers use heading_stack for hierarchy, not indent_stack
        // Bullets need to be in stack so indented bullets can find them as parents
        if heading_level.is_none() && (indent_level > 0 || is_bullet) {
            indent_stack.push((node_id.clone(), indent_level));
        }

        // Track node in context
        context.track_node(node_id, node_type.to_string());

        i += 1;
    }

    Ok(())
}

/// Create a node via NodeService
async fn create_node<C>(
    node_service: &Arc<NodeService<C>>,
    node_type: &str,
    content: &str,
    parent_id: Option<String>,
    _root_node_id: Option<String>, // Deprecated - kept for backward compat but ignored (root auto-derived from parent)
    insert_after_node_id: Option<String>, // Insert after this sibling (None = insert at beginning)
    custom_properties: Option<Value>, // Custom properties from markdown parsing (e.g., task status)
) -> Result<String, MCPError>
where
    C: surrealdb::Connection,
{
    // Create node via NodeService (enforces all business rules)
    // Note: container/root is now auto-derived from parent chain by backend
    // Note: sibling ordering uses insert_after_node_id to maintain document order

    // Use custom properties if provided, otherwise use defaults based on node type
    let properties = if let Some(props) = custom_properties {
        props
    } else {
        match node_type {
            "task" => {
                // Task nodes require status field per schema
                // Default to "open" (schema default, Issue #670)
                json!({"status": "open"})
            }
            _ => json!({}),
        }
    };

    node_service
        .create_node_with_parent(CreateNodeParams {
            id: None, // MCP generates IDs server-side
            node_type: node_type.to_string(),
            content: content.to_string(),
            parent_id,
            insert_after_node_id, // Insert after last sibling to preserve document order
            properties,
        })
        .await
        .map_err(|e| {
            // Create preview of content for error message (avoid multi-line content in error)
            let content_preview: String = content
                .chars()
                .take(40)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            let content_display = if content.len() > 40 {
                format!("{}...", content_preview)
            } else {
                content_preview
            };

            MCPError::node_creation_failed(format!(
                "Failed to create {} node '{}': {}",
                node_type, content_display, e
            ))
        })
}

// ============================================================================
// Markdown Export (get_markdown_from_node_id)
// ============================================================================

/// Parameters for get_markdown_from_node_id method
#[derive(Debug, Deserialize)]
pub struct GetMarkdownParams {
    /// Root node ID to export
    pub node_id: String,

    /// Include children recursively (default: true)
    #[serde(default = "default_include_children")]
    pub include_children: bool,

    /// Maximum recursion depth to prevent infinite loops (default: 20)
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,

    /// Include node ID comments in markdown output (default: true)
    /// When true, adds HTML comments with node IDs and versions: `<!-- uuid v1 -->`
    /// When false, produces clean markdown without metadata
    #[serde(default = "default_include_node_ids")]
    pub include_node_ids: bool,
}

fn default_include_children() -> bool {
    true
}
fn default_max_depth() -> usize {
    20
}
fn default_include_node_ids() -> bool {
    true
}

/// Export node hierarchy as markdown with minimal metadata
///
/// Returns clean markdown with HTML comments containing node IDs and versions.
/// The version enables optimistic concurrency control when AI agents update nodes.
///
/// # Example Output
///
/// ```markdown
/// <!-- container-abc123 v1 -->
/// # Project Plan
///
/// <!-- header-def456 v1 -->
/// ## Phase 1
///
/// <!-- task-ghi789 v2 -->
/// - [ ] Review architecture
/// ```
pub async fn handle_get_markdown_from_node_id<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    // Parse parameters
    let params: GetMarkdownParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Use shared get_subtree_data for efficient bulk fetch (same as frontend uses)
    // This performs exactly 3 database queries regardless of tree depth or node count:
    // 1. Fetch root node
    // 2. Fetch all descendant nodes in subtree
    // 3. Fetch all edges in subtree
    let (root_node, node_map, adjacency_list) = node_service
        .get_subtree_data(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get subtree data: {}", e)))?;

    let root_node = root_node.ok_or_else(|| MCPError::node_not_found(&params.node_id))?;

    // Build markdown by traversing hierarchy in memory (no database calls)
    let mut markdown = String::new();
    let mut node_count: usize = 1; // Start with root node

    // Export the container node itself with version for OCC (if node IDs enabled)
    if params.include_node_ids {
        markdown.push_str(&format!(
            "<!-- {} v{} -->\n",
            root_node.id, root_node.version
        ));
    }
    markdown.push_str(&root_node.content);
    markdown.push_str("\n\n");

    // Export children if requested
    if params.include_children {
        // Get direct children from adjacency list (already sorted by order)
        if let Some(child_ids) = adjacency_list.get(&root_node.id) {
            // Export each child and its descendants (children are already in correct order)
            for child_id in child_ids {
                if let Some(child) = node_map.get(child_id) {
                    node_count += export_node_hierarchy(
                        child,
                        &node_map,
                        &adjacency_list,
                        &mut markdown,
                        1, // Start at depth 1 (container is depth 0)
                        params.max_depth,
                        true, // Always include children when recursing
                        params.include_node_ids,
                    );
                }
            }
        }
    }

    // Return result with version for easy OCC reference
    Ok(json!({
        "markdown": markdown,
        "root_node_id": params.node_id,
        "version": root_node.version,
        "node_count": node_count
    }))
}

/// Format task node checkbox based on task status
///
/// Uses TaskNode::from_node for strongly-typed status extraction.
/// Returns "- [x] " for Done tasks, "- [ ] " for all other states.
fn format_task_checkbox(node: &Node) -> &'static str {
    if let Ok(task) = TaskNode::from_node(node.clone()) {
        match task.status() {
            TaskStatus::Done => "- [x] ",
            _ => "- [ ] ", // Open, InProgress, Cancelled all render as unchecked
        }
    } else {
        "- [ ] " // Default to unchecked if conversion fails
    }
}

/// Recursively export node hierarchy to markdown
///
/// Uses pre-fetched data from get_subtree_data for efficient in-memory traversal.
/// Automatically adds bullet formatting for text nodes under headers when appropriate.
/// Returns the number of nodes exported (for accurate node_count in response).
#[allow(clippy::too_many_arguments)]
fn export_node_hierarchy(
    node: &Node,
    node_map: &std::collections::HashMap<String, Node>,
    adjacency_list: &std::collections::HashMap<String, Vec<String>>,
    output: &mut String,
    current_depth: usize,
    max_depth: usize,
    include_children: bool,
    include_node_ids: bool,
) -> usize {
    // Prevent infinite recursion
    if current_depth >= max_depth {
        let content_preview: String = node.content.chars().take(50).collect();
        tracing::warn!(
            "Max depth {} reached at node {} (content: {}{})",
            max_depth,
            node.id,
            content_preview,
            if node.content.len() > 50 { "..." } else { "" }
        );
        return 0; // Node not exported due to max_depth
    }

    let mut count: usize = 1; // This node is being exported

    // Get children from adjacency list (already sorted by order)
    let child_ids = if include_children {
        adjacency_list
            .get(&node.id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    } else {
        &[]
    };

    // Add minimal metadata comment with ID and version for OCC (if enabled)
    if include_node_ids {
        output.push_str(&format!("<!-- {} v{} -->\n", node.id, node.version));
    }

    // Handle task nodes specially - render with checkbox syntax
    if node.node_type == "task" {
        output.push_str(format_task_checkbox(node));
        output.push_str(&node.content);
        output.push_str("\n\n");
    } else {
        // Add content with proper formatting
        output.push_str(&node.content);
        output.push_str("\n\n");
    }

    // Export children with parent context for bullet formatting
    if include_children && !child_ids.is_empty() {
        let export_ctx = ExportContext {
            parent_type: &node.node_type,
            sibling_count: child_ids.len(),
        };

        let params = ExportParams {
            node_map,
            adjacency_list,
            max_depth,
            include_children,
        };

        for child_id in child_ids {
            if let Some(child) = node_map.get(child_id) {
                count += export_node_with_context(
                    child,
                    &params,
                    output,
                    current_depth + 1,
                    export_ctx,
                    include_node_ids,
                );
            }
        }
    }

    count
}

/// Context for exporting nodes with bullet formatting
#[derive(Clone, Copy)]
struct ExportContext<'a> {
    parent_type: &'a str,
    sibling_count: usize,
}

/// Parameters for tree traversal during export
struct ExportParams<'a> {
    node_map: &'a std::collections::HashMap<String, Node>,
    adjacency_list: &'a std::collections::HashMap<String, Vec<String>>,
    max_depth: usize,
    include_children: bool,
}

/// Export node with parent context for bullet formatting
///
/// Uses pre-fetched data from get_subtree_data for efficient in-memory traversal.
/// Adds "- " prefix to text nodes when:
/// 1. Node is a text node
/// 2. Parent is a header node
/// 3. Parent has 2+ children (indicates a list, not single descriptive text)
/// 4. Node has no children itself
///
/// Returns the number of nodes exported (for accurate node_count in response).
fn export_node_with_context(
    node: &Node,
    params: &ExportParams<'_>,
    output: &mut String,
    current_depth: usize,
    context: ExportContext<'_>,
    include_node_ids: bool,
) -> usize {
    // Prevent infinite recursion
    if current_depth >= params.max_depth {
        let content_preview: String = node.content.chars().take(50).collect();
        tracing::warn!(
            "Max depth {} reached at node {} (content: {}{})",
            params.max_depth,
            node.id,
            content_preview,
            if node.content.len() > 50 { "..." } else { "" }
        );
        return 0; // Node not exported due to max_depth
    }

    let mut count: usize = 1; // This node is being exported

    // Get children from adjacency list (already sorted by order)
    let child_ids = params
        .adjacency_list
        .get(&node.id)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let has_children = !child_ids.is_empty();

    // Determine if this node should be rendered as a bullet item
    // Rules: text node + (header OR text) parent + 2+ siblings + no children
    // This covers both direct header children and label→list patterns
    let should_render_as_bullet = node.node_type == "text"
        && node.content.len() < MAX_BULLET_CONTENT_LENGTH
        && (context.parent_type == "header" || context.parent_type == "text")
        && context.sibling_count >= 2
        && !has_children;

    // Add minimal metadata comment with ID and version for OCC (if enabled)
    if include_node_ids {
        output.push_str(&format!("<!-- {} v{} -->\n", node.id, node.version));
    }

    // Handle task nodes specially - render with checkbox syntax
    if node.node_type == "task" {
        output.push_str(format_task_checkbox(node));
        output.push_str(&node.content);
        output.push_str("\n\n");
    } else if should_render_as_bullet {
        // Add content with bullet prefix if applicable
        output.push_str("- ");
        output.push_str(&node.content);
        output.push_str("\n\n");
    } else {
        output.push_str(&node.content);
        output.push_str("\n\n");
    }

    // Recursively export children with context
    if params.include_children && !child_ids.is_empty() {
        let child_ctx = ExportContext {
            parent_type: &node.node_type,
            sibling_count: child_ids.len(),
        };

        for child_id in child_ids {
            if let Some(child) = params.node_map.get(child_id) {
                count += export_node_with_context(
                    child,
                    params,
                    output,
                    current_depth + 1,
                    child_ctx,
                    include_node_ids,
                );
            }
        }
    }

    count
}

// NOTE: sort_by_sibling_chain removed - sibling ordering is now handled via
// has_child edge order field. Children are returned in order from database.

// ============================================================================
// Bulk Root Update (update_root_from_markdown)
// ============================================================================

/// Parameters for update_root_from_markdown method
#[derive(Debug, Deserialize)]
pub struct UpdateRootFromMarkdownParams {
    /// Root node ID to update
    pub root_id: String,
    /// New markdown content (replaces all children)
    pub markdown: String,
}

/// Replace root node's children with nodes parsed from markdown
///
/// Similar to create_nodes_from_markdown but operates on an existing root node.
/// Deletes all existing children and creates new hierarchy from markdown.
///
/// This enables "GitHub-style" bulk updates where AI edits markdown freely
/// and replaces the entire structure at once.
///
/// # Transaction Semantics
///
/// **Important**: This operation is NOT atomic. It performs two sequential phases:
/// 1. Delete all existing children (with individual delete operations)
/// 2. Create new nodes from markdown (with individual create operations)
///
/// If phase 2 fails (e.g., markdown parsing error, resource limits), the root
/// will be left in an inconsistent state with old children deleted but new children
/// not fully created. This is acceptable for AI-driven workflows where:
/// - AI agents can retry the entire operation if it fails
/// - Partial state is better than blocking on transaction complexity
/// - The root node itself remains valid (only children are affected)
///
/// **For production use**: Consider implementing transaction support if atomicity
/// guarantees are required for your use case.
///
/// # Backward Compatibility
///
/// This function accepts both `root_id` (preferred) and `container_id` (deprecated).
/// If both are provided, `root_id` takes precedence.
///
/// # Example
///
/// ```ignore
/// // New style (preferred)
/// let params = json!({
///     "root_id": "root-123",
///     "markdown": "# Updated Plan\n- New task 1\n- New task 2"
/// });
/// // Or deprecated style (still works)
/// let params = json!({
///     "container_id": "root-123",
///     "markdown": "# Updated Plan\n- New task 1\n- New task 2"
/// });
/// let result = handle_update_root_from_markdown(&operations, params).await?;
/// // Old children deleted, new structure created
/// // Returns deletion_failures if any deletes failed
/// ```
pub async fn handle_update_root_from_markdown<C>(
    node_service: &Arc<NodeService<C>>,
    params: Value,
) -> Result<Value, MCPError>
where
    C: surrealdb::Connection,
{
    // Parse parameters
    let params: UpdateRootFromMarkdownParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    let root_id = params.root_id;

    // Validate markdown content size
    if params.markdown.len() > MAX_MARKDOWN_SIZE {
        return Err(MCPError::invalid_params(format!(
            "Markdown content exceeds maximum size of {} bytes (got {} bytes)",
            MAX_MARKDOWN_SIZE,
            params.markdown.len()
        )));
    }

    // Validate root node exists
    let root_node = node_service
        .get_node(&root_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get root node: {}", e)))?
        .ok_or_else(|| MCPError::node_not_found(&root_id))?;

    // Get all existing descendants using graph traversal
    // This gets ALL nodes in the hierarchy, including nested children
    // Critical for proper cleanup of structures like: Root → Header → Tasks
    let mut existing_children = node_service
        .get_descendants(&root_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get children: {}", e)))?;

    // Note: get_nodes_by_root() already excludes the root itself
    // (root has root_node_id = None, not equal to itself)

    // Delete in REVERSE order to avoid version conflicts from sibling chain updates.
    // Background: operations.delete_node() updates the next sibling's version when fixing
    // the sibling chain (see operations/mod.rs:1199-1260). If we delete in forward order:
    //   1. Delete node A (next sibling B's version increments)
    //   2. Try to delete B using stale cached version → VERSION CONFLICT
    // Solution: Delete B first (using current version), then delete A (no conflict).
    // This pattern is CRITICAL - changing deletion order will break sibling chain integrity.
    existing_children.reverse();

    // Delete all existing children (recursively)
    let mut deleted_count = 0;
    let mut deletion_failures = Vec::new();
    for child in existing_children {
        // CRITICAL: Refetch current version before deletion since sibling chain updates
        // can increment versions as we delete. Using stale cached version will cause conflicts.
        let current_node = match node_service.get_node(&child.id).await {
            Ok(Some(node)) => node,
            Ok(None) => {
                // Node already deleted (possible if recursive delete)
                deleted_count += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!("Failed to fetch node {} before deletion: {}", child.id, e);
                deletion_failures.push(json!({
                    "node_id": child.id,
                    "error": format!("Failed to fetch before deletion: {}", e)
                }));
                continue;
            }
        };

        match node_service
            .delete_node_with_occ(&current_node.id, current_node.version)
            .await
        {
            Ok(_) => {
                deleted_count += 1;
            }
            Err(e) => {
                tracing::warn!("Failed to delete child node {}: {}", child.id, e);
                deletion_failures.push(json!({
                    "node_id": child.id,
                    "error": e.to_string()
                }));
            }
        }
    }

    // Create parser context for the existing root node
    // This properly initializes the parser state to treat the root as hierarchy root
    // Note: new_for_existing_container is kept for internal compatibility but works for roots
    let mut context =
        ParserContext::new_for_existing_container(root_id.clone(), root_node.content.clone());

    // Parse the new markdown content and create nodes
    parse_markdown(&params.markdown, node_service, &mut context).await?;

    // Validate we didn't exceed max nodes
    if context.nodes.len() > MAX_NODES_PER_IMPORT {
        return Err(MCPError::invalid_params(format!(
            "Import created {} nodes, exceeding maximum of {}",
            context.nodes.len(),
            MAX_NODES_PER_IMPORT
        )));
    }

    // Return operation results
    Ok(json!({
        "root_id": root_id,
        "nodes_deleted": deleted_count,
        "deletion_failures": deletion_failures,
        "nodes_created": context.nodes.len(),
        "nodes": context.nodes
    }))
}

// Include tests
#[cfg(test)]
#[path = "markdown_test.rs"]
mod markdown_test;
