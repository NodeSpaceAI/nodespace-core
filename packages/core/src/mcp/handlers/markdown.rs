//! MCP Markdown Import Handler
//!
//! Parses markdown content and creates hierarchical NodeSpace nodes.
//! Preserves heading hierarchy and list indentation as parent-child relationships.
//!
//! # Examples
//!
//! ```rust,no_run
//! use serde_json::json;
//! use std::sync::Arc;
//! # use crate::services::NodeService;
//! # use crate::mcp::handlers::markdown::handle_create_nodes_from_markdown;
//!
//! # async fn example(service: Arc<NodeService>) -> Result<(), Box<dyn std::error::Error>> {
//! let params = json!({
//!     "markdown_content": "# My Document\n\n- Task 1\n- Task 2",
//!     "container_title": "Imported Notes"
//! });
//! let result = handle_create_nodes_from_markdown(&service, params).await?;
//! println!("Created {} nodes", result["nodes_created"]);
//! # Ok(())
//! # }
//! ```

use crate::mcp::types::MCPError;
use crate::models::Node;
use crate::services::NodeService;
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// Maximum markdown content size (1MB) to prevent resource exhaustion
const MAX_MARKDOWN_SIZE: usize = 1_000_000;

/// Maximum number of nodes that can be created in a single import
const MAX_NODES_PER_IMPORT: usize = 1000;

/// Parameters for create_nodes_from_markdown method
#[derive(Debug, Deserialize)]
pub struct CreateNodesFromMarkdownParams {
    pub markdown_content: String,
    pub container_title: String,
}

/// Metadata for a created node (id + type)
#[derive(Debug, Clone, Serialize)]
pub struct NodeMetadata {
    pub id: String,
    pub node_type: String,
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
    /// Container node ID (first node created becomes the container)
    container_node_id: Option<String>,
    /// Current list item counter for ordered lists (tracks the number prefix for ordered lists)
    ordered_list_counter: usize,
    /// Whether we're in an ordered list
    in_ordered_list: bool,
    /// Whether the first node has been created (becomes the container)
    first_node_created: bool,
}

impl ParserContext {
    fn new_empty() -> Self {
        Self {
            heading_stack: Vec::new(),
            list_stack: Vec::new(),
            last_sibling: None,
            node_ids: Vec::new(),
            nodes: Vec::new(),
            container_node_id: None,
            ordered_list_counter: 0,
            in_ordered_list: false,
            first_node_created: false,
        }
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

    /// Update heading hierarchy when encountering a new heading
    fn push_heading(&mut self, node_id: String, level: usize) {
        // Pop headings at same or lower level
        while let Some(top) = self.heading_stack.last() {
            if top.level >= level {
                self.heading_stack.pop();
            } else {
                break;
            }
        }

        self.heading_stack.push(HierarchyNode { node_id, level });
        self.last_sibling = None; // Reset sibling tracking at new heading level
    }

    /// Push a list level onto the stack
    fn push_list(&mut self, node_id: String, level: usize) {
        self.list_stack.push(HierarchyNode { node_id, level });
    }

    /// Pop a list level from the stack
    fn pop_list(&mut self) {
        self.list_stack.pop();
    }

    /// Track a node for sibling ordering
    fn track_node(&mut self, node_id: String, node_type: String) {
        // If this is the first node, it becomes the container
        if !self.first_node_created {
            self.container_node_id = Some(node_id.clone());
            self.first_node_created = true;
        }

        self.last_sibling = Some(node_id.clone());
        self.node_ids.push(node_id.clone());
        self.nodes.push(NodeMetadata {
            id: node_id,
            node_type,
        });
    }

    /// Check if this is the first node being created
    fn is_first_node(&self) -> bool {
        !self.first_node_created
    }
}

/// Handle create_nodes_from_markdown MCP request
pub async fn handle_create_nodes_from_markdown(
    service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
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

    // Parse markdown and create nodes
    // The first element will become the container node
    let mut context = ParserContext::new_empty();
    parse_markdown(&params.markdown_content, service, &mut context).await?;

    // Validate we didn't exceed max nodes
    if context.nodes.len() > MAX_NODES_PER_IMPORT {
        return Err(MCPError::invalid_params(format!(
            "Import created {} nodes, exceeding maximum of {}",
            context.nodes.len(),
            MAX_NODES_PER_IMPORT
        )));
    }

    // Get the container node ID (first created node)
    let container_node_id = context
        .container_node_id
        .ok_or_else(|| MCPError::internal_error("No container node created".to_string()))?;

    Ok(json!({
        "success": true,
        "container_node_id": container_node_id,
        "nodes_created": context.nodes.len(),
        "node_ids": context.node_ids,
        "nodes": context.nodes
    }))
}

/// Parse markdown content and create nodes
///
/// This function processes markdown using pulldown-cmark's event stream parser
/// and creates hierarchical NodeSpace nodes with proper parent-child relationships.
///
/// # Arguments
///
/// * `markdown` - The markdown content to parse
/// * `service` - NodeService for creating nodes in the database
/// * `context` - Parser context tracking hierarchy and node state
///
/// # Returns
///
/// Returns `Ok(())` on success, or an `MCPError` if node creation fails.
///
/// # Hierarchy Tracking
///
/// - **Heading hierarchy**: h1 → h2 → h3 tracked via heading_stack
/// - **List hierarchy**: Indentation levels tracked via list_stack
/// - **List context takes precedence**: Items nested in lists use list parent, not heading parent
async fn parse_markdown(
    markdown: &str,
    service: &Arc<NodeService>,
    context: &mut ParserContext,
) -> Result<(), MCPError> {
    // Enable GitHub Flavored Markdown extensions (tables, strikethrough, task lists, etc.)
    let mut options = pulldown_cmark::Options::empty();
    options.insert(pulldown_cmark::Options::ENABLE_TABLES);
    options.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
    options.insert(pulldown_cmark::Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(markdown, options);

    let mut current_text = String::new();
    let mut current_tag: Option<Tag> = None;
    let mut code_lang = String::new();
    let mut in_task_item = false;
    let mut task_state = String::new();

    for event in parser {
        match event {
            Event::Start(tag) => {
                current_tag = Some(tag.clone());
                current_text.clear();

                match tag {
                    Tag::CodeBlock(pulldown_cmark::CodeBlockKind::Fenced(lang)) => {
                        code_lang = lang.to_string();
                    }
                    Tag::List(first_number) => {
                        if let Some(num) = first_number {
                            context.in_ordered_list = true;
                            context.ordered_list_counter = num as usize;
                        } else {
                            context.in_ordered_list = false;
                        }
                    }
                    _ => {}
                }
            }

            Event::End(tag_end) => {
                match tag_end {
                    TagEnd::Heading(level) => {
                        let heading_level = heading_level_to_usize(level);
                        let content =
                            format!("{} {}", "#".repeat(heading_level), current_text.trim());

                        let node_id = create_node(
                            service,
                            "header",
                            &content,
                            context.current_parent_id(),
                            context.container_node_id.clone(),
                            context.last_sibling.clone(),
                            context.is_first_node(),
                        )
                        .await?;

                        context.push_heading(node_id.clone(), heading_level);
                        context.track_node(node_id, "header".to_string());
                    }

                    TagEnd::Paragraph => {
                        let content = current_text.trim();
                        if !content.is_empty()
                            && current_tag
                                .as_ref()
                                .map(|t| !matches!(t, Tag::Item))
                                .unwrap_or(false)
                        {
                            let node_id = create_node(
                                service,
                                "text",
                                content,
                                context.current_parent_id(),
                                context.container_node_id.clone(),
                                context.last_sibling.clone(),
                                context.is_first_node(),
                            )
                            .await?;
                            context.track_node(node_id, "text".to_string());
                        }
                    }

                    TagEnd::CodeBlock => {
                        let fence = if code_lang.is_empty() {
                            "```".to_string()
                        } else {
                            format!("```{}", code_lang)
                        };
                        let content = format!("{}\n{}\n```", fence, current_text.trim_end());

                        let node_id = create_node(
                            service,
                            "code-block",
                            &content,
                            context.current_parent_id(),
                            context.container_node_id.clone(),
                            context.last_sibling.clone(),
                            context.is_first_node(),
                        )
                        .await?;
                        context.track_node(node_id, "code-block".to_string());
                        code_lang.clear();
                    }

                    TagEnd::BlockQuote => {
                        if !current_text.is_empty() {
                            let lines: Vec<&str> = current_text.lines().collect();
                            let content = lines
                                .iter()
                                .map(|line| format!("> {}", line.trim()))
                                .collect::<Vec<_>>()
                                .join("\n");

                            let node_id = create_node(
                                service,
                                "quote-block",
                                &content,
                                context.current_parent_id(),
                                context.container_node_id.clone(),
                                context.last_sibling.clone(),
                                context.is_first_node(),
                            )
                            .await?;
                            context.track_node(node_id, "quote-block".to_string());
                        }
                    }

                    TagEnd::Item => {
                        let content = current_text.trim();
                        if !content.is_empty() {
                            // Determine node type and format content
                            let (node_type, formatted_content) = if in_task_item {
                                // Task node with checkbox
                                ("task", format!("- {} {}", task_state, content))
                            } else if context.in_ordered_list {
                                // Ordered list item with number prefix
                                // The counter tracks the sequence (1., 2., 3., etc.) and increments
                                // after each item to maintain proper markdown numbering
                                let formatted =
                                    format!("{}. {}", context.ordered_list_counter, content);
                                context.ordered_list_counter += 1;
                                ("ordered-list", formatted)
                            } else {
                                // Regular unordered list item
                                ("text", format!("- {}", content))
                            };

                            let node_id = create_node(
                                service,
                                node_type,
                                &formatted_content,
                                context.current_parent_id(),
                                context.container_node_id.clone(),
                                context.last_sibling.clone(),
                                context.is_first_node(),
                            )
                            .await?;

                            // Update list hierarchy based on indentation level
                            let list_level = context.list_stack.len();
                            context.push_list(node_id.clone(), list_level);
                            context.track_node(node_id, node_type.to_string());

                            in_task_item = false;
                            task_state.clear();
                        }
                    }

                    TagEnd::List(_) => {
                        context.pop_list();
                        context.in_ordered_list = false;
                        context.ordered_list_counter = 0;
                    }

                    _ => {}
                }

                current_tag = None;
            }

            Event::Text(text) => {
                current_text.push_str(&text);
            }

            Event::Code(code) => {
                current_text.push('`');
                current_text.push_str(&code);
                current_text.push('`');
            }

            Event::TaskListMarker(checked) => {
                in_task_item = true;
                task_state = if checked {
                    "[x]".to_string()
                } else {
                    "[ ]".to_string()
                };
            }

            Event::SoftBreak | Event::HardBreak => {
                current_text.push('\n');
            }

            _ => {}
        }
    }

    Ok(())
}

/// Create a node via NodeService
async fn create_node(
    service: &Arc<NodeService>,
    node_type: &str,
    content: &str,
    parent_id: Option<String>,
    container_node_id: Option<String>,
    before_sibling_id: Option<String>,
    is_first_node: bool,
) -> Result<String, MCPError> {
    let node = Node::new(
        node_type.to_string(),
        content.to_string(),
        parent_id,
        json!({}),
    );

    // First node becomes the container (no container_node_id for itself)
    // All subsequent nodes belong to that container
    let node = if is_first_node {
        Node {
            container_node_id: None, // First node IS the container
            before_sibling_id,
            ..node
        }
    } else {
        Node {
            container_node_id,
            before_sibling_id,
            ..node
        }
    };

    // Create node and provide contextual error on failure
    service.create_node(node).await.map_err(|e| {
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

/// Convert HeadingLevel to usize (1-6)
fn heading_level_to_usize(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
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
}

fn default_include_children() -> bool {
    true
}
fn default_max_depth() -> usize {
    20
}

/// Export node hierarchy as markdown with minimal metadata
///
/// Returns clean markdown with HTML comments containing only node IDs.
/// This format is optimized for AI reading and understanding.
///
/// # Example Output
///
/// ```markdown
/// <!-- container-abc123 -->
/// # Project Plan
///
/// <!-- header-def456 -->
/// ## Phase 1
///
/// <!-- task-ghi789 -->
/// - [ ] Review architecture
/// ```
pub async fn handle_get_markdown_from_node_id(
    service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    // Parse parameters
    let params: GetMarkdownParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Fetch the root node first to validate it exists
    let root_node = service
        .get_node(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?
        .ok_or_else(|| MCPError::node_not_found(&params.node_id))?;

    // Bulk fetch all child nodes in this container (efficient single query)
    use crate::models::{NodeFilter, OrderBy};
    let filter = NodeFilter::new()
        .with_container_node_id(root_node.id.clone())
        .with_order_by(OrderBy::CreatedAsc);

    let all_nodes = service
        .query_nodes(filter)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to query container nodes: {}", e)))?;

    // Build a lookup map for efficient hierarchy reconstruction
    // Include the root node in the map for complete hierarchy
    use std::collections::HashMap;
    let mut nodes_map: HashMap<String, Node> =
        all_nodes.into_iter().map(|n| (n.id.clone(), n)).collect();

    // Add root node to the map
    nodes_map.insert(root_node.id.clone(), root_node.clone());

    // Build markdown by traversing hierarchy in memory
    let mut markdown = String::new();

    // Export the container node itself
    markdown.push_str(&format!("<!-- {} -->\n", root_node.id));
    markdown.push_str(&root_node.content);
    markdown.push_str("\n\n");

    // Export children if requested
    if params.include_children {
        // Find direct children of the container node
        // Note: With first-element-as-container model, children have parent_id = container_id
        // (not None - the container itself has parent_id = None or points to its parent)
        let mut top_level_nodes: Vec<&Node> = nodes_map
            .values()
            .filter(|n| n.parent_id.as_ref() == Some(&root_node.id))
            .collect();

        // Sort by sibling order
        sort_by_sibling_chain(&mut top_level_nodes);

        // Export each top-level node and its descendants
        for node in top_level_nodes {
            export_node_hierarchy(
                node,
                &nodes_map,
                &mut markdown,
                1, // Start at depth 1 (container is depth 0)
                params.max_depth,
                true, // Always include children when recursing
            )?;
        }
    }

    // Return result
    Ok(json!({
        "markdown": markdown,
        "root_node_id": params.node_id,
        "node_count": count_nodes_in_markdown(&markdown)
    }))
}

/// Recursively export node hierarchy to markdown using in-memory node map
fn export_node_hierarchy(
    node: &Node,
    nodes_map: &std::collections::HashMap<String, Node>,
    output: &mut String,
    current_depth: usize,
    max_depth: usize,
    include_children: bool,
) -> Result<(), MCPError> {
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
        return Ok(());
    }

    // Add minimal metadata comment (just ID)
    output.push_str(&format!("<!-- {} -->\n", node.id));

    // Add content
    output.push_str(&node.content);
    output.push_str("\n\n");

    // Recursively export children (if enabled)
    if include_children {
        // Find all children of this node from the in-memory map
        let mut children: Vec<&Node> = nodes_map
            .values()
            .filter(|n| n.parent_id.as_ref() == Some(&node.id))
            .collect();

        // Sort children by sibling order (reconstruct before_sibling_id chain)
        sort_by_sibling_chain(&mut children);

        for child in children {
            export_node_hierarchy(
                child,
                nodes_map,
                output,
                current_depth + 1,
                max_depth,
                include_children,
            )?;
        }
    }

    Ok(())
}

/// Sort nodes by their before_sibling_id chain to maintain visual order
///
/// This function reconstructs the sibling order by following the before_sibling_id chain.
/// The before_sibling_id field means "I come AFTER this node", so we build a forward map
/// to traverse from head to tail.
fn sort_by_sibling_chain(nodes: &mut Vec<&Node>) {
    if nodes.is_empty() {
        return;
    }

    // Build forward map: before_sibling_id -> nodes that come after it
    // Using Vec to detect duplicates (multiple nodes with same before_sibling_id)
    use std::collections::HashMap;
    let mut after_map: HashMap<Option<String>, Vec<&Node>> = HashMap::new();

    for node in nodes.iter() {
        after_map
            .entry(node.before_sibling_id.clone())
            .or_default()
            .push(*node);
    }

    // Detect duplicate before_sibling_ids (data integrity issue)
    for (before_id, nodes_after) in &after_map {
        if nodes_after.len() > 1 {
            tracing::warn!(
                "Multiple nodes have same before_sibling_id: {:?}. This indicates corrupted sibling chain data.",
                before_id
            );
        }
    }

    // Find head: node with None or before_sibling not in this set
    use std::collections::HashSet;
    let node_ids: HashSet<_> = nodes.iter().map(|n| &n.id).collect();
    let head = nodes
        .iter()
        .find(|n| {
            n.before_sibling_id.is_none()
                || !node_ids.contains(n.before_sibling_id.as_ref().unwrap())
        })
        .copied();

    if let Some(mut current) = head {
        let mut sorted = vec![current];
        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(&current.id);

        // Follow the chain forward using after_map
        // Use visited set to detect cycles
        while let Some(next_nodes) = after_map.get(&Some(current.id.clone())) {
            if let Some(&next) = next_nodes.first() {
                // Cycle detection: if we've seen this node before, we have a circular reference
                if visited.contains(next.id.as_str()) {
                    tracing::error!(
                        "Circular sibling chain detected at node {}. This is a data corruption issue.",
                        next.id
                    );
                    break;
                }

                visited.insert(&next.id);
                sorted.push(next);
                current = next;
            } else {
                break;
            }
        }

        // Replace the original vector with sorted nodes
        nodes.clear();
        nodes.extend(sorted);
    }
}

/// Count number of nodes in markdown (by counting HTML comments)
fn count_nodes_in_markdown(markdown: &str) -> usize {
    markdown.matches("<!--").count()
}

// Include tests
#[cfg(test)]
#[path = "markdown_test.rs"]
mod markdown_test;
