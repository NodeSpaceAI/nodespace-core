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
use crate::operations::{CreateNodeParams, NodeOperations};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

/// Maximum markdown content size (1MB) to prevent resource exhaustion
///
/// **Rationale:** Prevents DoS attacks from malicious AI agents attempting to import
/// extremely large markdown files. 1MB is sufficient for typical documents (100-1000 nodes)
/// while protecting against memory exhaustion. Most markdown notes are < 100KB.
const MAX_MARKDOWN_SIZE: usize = 1_000_000;

/// Maximum number of nodes that can be created in a single import
const MAX_NODES_PER_IMPORT: usize = 1000;

/// Parameters for create_nodes_from_markdown method
#[derive(Debug, Deserialize)]
pub struct CreateNodesFromMarkdownParams {
    /// Markdown content to parse into child nodes.
    ///
    /// IMPORTANT: Do NOT include the container_title text in markdown_content
    /// to avoid creating duplicate nodes. The container_title creates a separate
    /// container node, and all markdown_content nodes become children of it.
    ///
    /// Example:
    /// ```
    /// // CORRECT ✅
    /// container_title: "# Project Alpha"
    /// markdown_content: "## Task 1\nDescription here"
    /// // Creates: "# Project Alpha" (container) → "## Task 1" (child) → "Description" (child)
    ///
    /// // INCORRECT ❌
    /// container_title: "# Project Alpha"
    /// markdown_content: "# Project Alpha\n## Task 1\nDescription"
    /// // Creates: "# Project Alpha" (container) → "# Project Alpha" (duplicate child!) → ...
    /// ```
    pub markdown_content: String,

    /// Title for the container node (REQUIRED).
    ///
    /// This creates a separate container node that all markdown_content nodes
    /// will be children of. Can be:
    /// - A date string (YYYY-MM-DD) to use/create a date container
    /// - Markdown text (e.g., "# My Document" or "Project Notes") to create a text/header container
    ///
    /// The parsed container type must be text, header, or date.
    /// Multi-line types (code-block, quote-block, ordered-list) cannot be containers.
    pub container_title: String,
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
    /// Container node ID (determined by strategy)
    container_node_id: Option<String>,
    /// Whether the first node has been created (for tracking purposes)
    first_node_created: bool,
}

impl ParserContext {
    fn new_with_strategy(strategy: ContainerStrategy) -> Self {
        // For DateContainer strategy, set container_node_id immediately
        let container_node_id = match &strategy {
            ContainerStrategy::DateContainer(date) => Some(date.clone()),
            ContainerStrategy::TitleAsContainer(_) => None, // Will be set after parsing title
        };

        Self {
            heading_stack: Vec::new(),
            list_stack: Vec::new(),
            last_sibling: None,
            node_ids: Vec::new(),
            nodes: Vec::new(),
            container_node_id,
            first_node_created: false,
        }
    }

    /// Create a parser context for an existing container
    ///
    /// This is a convenience constructor for update_container_from_markdown
    /// that properly initializes the parser state for adding children to an
    /// existing container node. It sets up the heading stack and parser flags
    /// to treat the existing container as the root of the hierarchy.
    ///
    /// # Arguments
    ///
    /// * `container_id` - ID of the existing container node
    /// * `container_content` - Content of the existing container (typically the title)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// let context = ParserContext::new_for_existing_container(
    ///     "container-123".to_string(),
    ///     "# Project Plan".to_string()
    /// );
    /// // Context is ready to parse markdown and create children under container-123
    /// ```
    fn new_for_existing_container(container_id: String, container_content: String) -> Self {
        let mut context =
            Self::new_with_strategy(ContainerStrategy::TitleAsContainer(container_content));

        // Set container_node_id to the existing container
        context.container_node_id = Some(container_id.clone());

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
pub async fn handle_create_nodes_from_markdown(
    operations: &Arc<NodeOperations>,
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

    // Determine container strategy based on container_title
    let container_strategy = if is_date_format(&params.container_title) {
        // container_title is a date → use date as container
        ContainerStrategy::DateContainer(params.container_title.clone())
    } else {
        // container_title is markdown → parse it to create the container node
        ContainerStrategy::TitleAsContainer(params.container_title.clone())
    };

    let mut context = ParserContext::new_with_strategy(container_strategy.clone());

    // For TitleAsContainer, parse the title first to create the container node
    if let ContainerStrategy::TitleAsContainer(ref title) = container_strategy {
        // Temporarily clear container_node_id so the container node itself is created as a container
        // (with container_node_id = None, which is required for container nodes)
        context.container_node_id = None;

        parse_markdown(title, operations, &mut context).await?;

        // Validate that exactly one node was created and it's a valid container type
        if context.nodes.len() != 1 {
            return Err(MCPError::invalid_params(format!(
                "container_title must parse to exactly one node, got {}",
                context.nodes.len()
            )));
        }

        let container_node = &context.nodes[0];
        if !is_valid_container_type(&container_node.node_type) {
            return Err(MCPError::invalid_params(format!(
                "container_title parsed to '{}' which cannot be a container. Only text, header, or date nodes can be containers.",
                container_node.node_type
            )));
        }

        // Set this node as the container for subsequent nodes
        context.container_node_id = Some(container_node.id.clone());

        // CRITICAL: Set the container as the initial parent for top-level nodes in markdown_content
        // This makes the first heading in markdown_content a CHILD of the container, not a sibling
        context.push_heading(container_node.id.clone(), 0);

        context.first_node_created = true;
    }

    // Parse the main markdown content
    parse_markdown(&params.markdown_content, operations, &mut context).await?;

    // Validate we didn't exceed max nodes
    if context.nodes.len() > MAX_NODES_PER_IMPORT {
        return Err(MCPError::invalid_params(format!(
            "Import created {} nodes, exceeding maximum of {}",
            context.nodes.len(),
            MAX_NODES_PER_IMPORT
        )));
    }

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
/// ```
/// # use nodespace_core::mcp::handlers::markdown::detect_heading;
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
/// ```
/// # use nodespace_core::mcp::handlers::markdown::calculate_indent;
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
/// ```
/// # use nodespace_core::mcp::handlers::markdown::detect_ordered_list;
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
/// * `operations` - NodeOperations for creating nodes in the database
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
async fn parse_markdown(
    markdown: &str,
    operations: &Arc<NodeOperations>,
    context: &mut ParserContext,
) -> Result<(), MCPError> {
    // Track previous sibling for before_sibling_id chain
    let mut last_sibling_at_parent: std::collections::HashMap<Option<String>, String> =
        std::collections::HashMap::new();

    // Track indentation-based hierarchy (node_id, indent_level)
    let mut indent_stack: Vec<(String, usize)> = Vec::new();

    // Track the last text paragraph for bullet/ordered-list hierarchy
    let mut last_text_node: Option<(String, usize)> = None; // (node_id, indent_level)

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
        let (node_type, content, heading_level, is_multiline) =
            if let Some(level) = detect_heading(content_line) {
                ("header", content_line.to_string(), Some(level), false)
            } else if is_task_line(content_line) {
                // Task node
                ("task", content_line.to_string(), None, false)
            } else if content_line.starts_with("```") {
                // Code block - collect until closing ```
                let mut code_lines = vec![content_line];
                i += 1;
                while i < lines.len() {
                    let code_line = lines[i];
                    code_lines.push(code_line.trim_start());
                    if code_line.trim_start().starts_with("```") {
                        break;
                    }
                    i += 1;
                }
                let code_content = code_lines.join("\n");
                ("code-block", code_content, None, true)
            } else if content_line.starts_with("> ") {
                // Quote block - collect consecutive quote lines
                let mut quote_lines = vec![content_line];
                while i + 1 < lines.len() && lines[i + 1].trim_start().starts_with("> ") {
                    i += 1;
                    quote_lines.push(lines[i].trim_start());
                }
                let quote_content = quote_lines.join("\n");
                ("quote-block", quote_content, None, true)
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
                ("ordered-list", list_content, None, true)
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
                ("text", text_content, None, text_lines.len() > 1)
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
        let parent_id = if is_bullet && !is_multiline {
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

        // Get before_sibling_id from the chain
        let before_sibling_id = last_sibling_at_parent.get(&parent_id).cloned();

        // Create the node
        let node_id = create_node(
            operations,
            node_type,
            &content,
            parent_id.clone(),
            context.container_node_id.clone(),
            before_sibling_id,
        )
        .await?;

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

        // Push to indent stack if this line had indentation OR is a bullet (for nested bullets)
        // Headers use heading_stack for hierarchy, not indent_stack
        // Bullets need to be in stack so indented bullets can find them as parents
        if heading_level.is_none() && (indent_level > 0 || is_bullet) {
            indent_stack.push((node_id.clone(), indent_level));
        }

        // Track this node as the last sibling at this parent level
        last_sibling_at_parent.insert(parent_id, node_id.clone());

        // Track node in context
        context.track_node(node_id, node_type.to_string());

        i += 1;
    }

    Ok(())
}

/// Create a node via NodeOperations
async fn create_node(
    operations: &Arc<NodeOperations>,
    node_type: &str,
    content: &str,
    parent_id: Option<String>,
    container_node_id: Option<String>,
    before_sibling_id: Option<String>,
) -> Result<String, MCPError> {
    // Create node via NodeOperations (enforces all business rules)
    // Container node ID is provided from the pre-created container

    // Provide required properties based on node type
    let properties = match node_type {
        "task" => {
            // Task nodes require status field per schema
            // Default to "OPEN" (schema default)
            json!({"status": "OPEN"})
        }
        _ => json!({}),
    };

    operations
        .create_node(CreateNodeParams {
            id: None, // MCP generates IDs server-side
            node_type: node_type.to_string(),
            content: content.to_string(),
            parent_id,
            container_node_id,
            before_sibling_id,
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
}

fn default_include_children() -> bool {
    true
}
fn default_max_depth() -> usize {
    20
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
pub async fn handle_get_markdown_from_node_id(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    // Parse parameters
    let params: GetMarkdownParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Fetch the root node first to validate it exists
    let root_node = operations
        .get_node(&params.node_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get node: {}", e)))?
        .ok_or_else(|| MCPError::node_not_found(&params.node_id))?;

    // Bulk fetch all descendant nodes using graph traversal
    // In graph-native architecture, we traverse the hierarchy recursively
    let all_nodes = operations
        .get_descendants(&root_node.id)
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

    // Export the container node itself with version for OCC
    markdown.push_str(&format!(
        "<!-- {} v{} -->\n",
        root_node.id, root_node.version
    ));
    markdown.push_str(&root_node.content);
    markdown.push_str("\n\n");

    // Export children if requested
    if params.include_children {
        // Find direct children using graph relationships
        // Query for nodes that have incoming edges from the root node
        let child_ids = operations
            .get_children(&root_node.id)
            .await
            .map_err(|e| MCPError::internal_error(format!("Failed to get children: {}", e)))?;

        let mut top_level_nodes: Vec<&Node> = child_ids
            .iter()
            .filter_map(|child| nodes_map.get(&child.id))
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
    _nodes_map: &std::collections::HashMap<String, Node>,
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

    // Add minimal metadata comment with ID and version for OCC
    output.push_str(&format!("<!-- {} v{} -->\n", node.id, node.version));

    // Add content with proper formatting
    // Note: Bullet detection requires parent lookup via graph traversal in production
    // For now, output content as-is (bullet formatting handled during import)
    output.push_str(&node.content);
    output.push_str("\n\n");

    // Recursively export children (if enabled)
    if include_children {
        // TODO: Implement proper child lookup using graph edges
        // For now, skip children in recursive export (markdown export partially broken)
        // The container-level export still works for top-level nodes
        let mut children: Vec<&Node> = Vec::new();

        // Sort children by sibling order (reconstruct before_sibling_id chain)
        sort_by_sibling_chain(&mut children);

        for child in children {
            export_node_hierarchy(
                child,
                _nodes_map,
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

// ============================================================================
// Bulk Container Update (update_container_from_markdown)
// ============================================================================

/// Parameters for update_container_from_markdown method
#[derive(Debug, Deserialize)]
pub struct UpdateContainerFromMarkdownParams {
    /// Container node ID to update
    pub container_id: String,
    /// New markdown content (replaces all children)
    pub markdown: String,
}

/// Replace container's children with nodes parsed from markdown
///
/// Similar to create_nodes_from_markdown but operates on an existing container.
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
/// If phase 2 fails (e.g., markdown parsing error, resource limits), the container
/// will be left in an inconsistent state with old children deleted but new children
/// not fully created. This is acceptable for AI-driven workflows where:
/// - AI agents can retry the entire operation if it fails
/// - Partial state is better than blocking on transaction complexity
/// - The container itself remains valid (only children are affected)
///
/// **For production use**: Consider implementing transaction support if atomicity
/// guarantees are required for your use case.
///
/// # Example
///
/// ```rust,no_run
/// let params = json!({
///     "container_id": "container-123",
///     "markdown": "# Updated Plan\n- New task 1\n- New task 2"
/// });
/// let result = handle_update_container_from_markdown(&operations, params).await?;
/// // Old children deleted, new structure created
/// // Returns deletion_failures if any deletes failed
/// ```
pub async fn handle_update_container_from_markdown(
    operations: &Arc<NodeOperations>,
    params: Value,
) -> Result<Value, MCPError> {
    // Parse parameters
    let params: UpdateContainerFromMarkdownParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Validate markdown content size
    if params.markdown.len() > MAX_MARKDOWN_SIZE {
        return Err(MCPError::invalid_params(format!(
            "Markdown content exceeds maximum size of {} bytes (got {} bytes)",
            MAX_MARKDOWN_SIZE,
            params.markdown.len()
        )));
    }

    // Validate container exists
    let container = operations
        .get_node(&params.container_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get container: {}", e)))?
        .ok_or_else(|| MCPError::node_not_found(&params.container_id))?;

    // Get all existing descendants using graph traversal
    // This gets ALL nodes in the hierarchy, including nested children
    // Critical for proper cleanup of structures like: Container → Header → Tasks
    let mut existing_children = operations
        .get_descendants(&params.container_id)
        .await
        .map_err(|e| MCPError::internal_error(format!("Failed to get children: {}", e)))?;

    // Note: get_nodes_by_container() already excludes the container itself
    // (container has container_node_id = None, not equal to itself)

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
        let current_node = match operations.get_node(&child.id).await {
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

        match operations
            .delete_node(&current_node.id, current_node.version)
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

    // Create parser context for the existing container
    // This properly initializes the parser state to treat the container as root
    let mut context = ParserContext::new_for_existing_container(
        params.container_id.clone(),
        container.content.clone(),
    );

    // Parse the new markdown content and create nodes
    parse_markdown(&params.markdown, operations, &mut context).await?;

    // Validate we didn't exceed max nodes
    if context.nodes.len() > MAX_NODES_PER_IMPORT {
        return Err(MCPError::invalid_params(format!(
            "Import created {} nodes, exceeding maximum of {}",
            context.nodes.len(),
            MAX_NODES_PER_IMPORT
        )));
    }

    Ok(json!({
        "container_id": params.container_id,
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
