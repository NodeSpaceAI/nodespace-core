//! MCP Markdown Import Handler
//!
//! Parses markdown content and creates hierarchical NodeSpace nodes.
//! Preserves heading hierarchy and list indentation as parent-child relationships.

use crate::mcp::types::MCPError;
use crate::models::Node;
use crate::services::NodeService;
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// Parameters for create_nodes_from_markdown method
#[derive(Debug, Deserialize)]
pub struct CreateNodesFromMarkdownParams {
    pub markdown_content: String,
    pub container_title: String,
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
    /// All created node IDs
    node_ids: Vec<String>,
    /// Container node ID (all nodes belong to this container)
    container_node_id: String,
    /// Current list item counter for ordered lists
    ordered_list_counter: usize,
    /// Whether we're in an ordered list
    in_ordered_list: bool,
}

impl ParserContext {
    fn new(container_node_id: String) -> Self {
        Self {
            heading_stack: Vec::new(),
            list_stack: Vec::new(),
            last_sibling: None,
            node_ids: vec![container_node_id.clone()],
            container_node_id,
            ordered_list_counter: 0,
            in_ordered_list: false,
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
    fn track_node(&mut self, node_id: String) {
        self.last_sibling = Some(node_id.clone());
        self.node_ids.push(node_id);
    }
}

/// Handle create_nodes_from_markdown MCP request
pub async fn handle_create_nodes_from_markdown(
    service: &Arc<NodeService>,
    params: Value,
) -> Result<Value, MCPError> {
    let params: CreateNodesFromMarkdownParams = serde_json::from_value(params)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    // Create container node
    let container_node = Node::new(
        "text".to_string(),
        params.container_title.clone(),
        None,
        json!({}),
    );

    let container_node_id = service.create_node(container_node).await.map_err(|e| {
        MCPError::node_creation_failed(format!("Failed to create container node: {}", e))
    })?;

    // Parse markdown and create nodes
    let mut context = ParserContext::new(container_node_id.clone());
    parse_markdown(&params.markdown_content, service, &mut context).await?;

    Ok(json!({
        "success": true,
        "container_node_id": container_node_id,
        "nodes_created": context.node_ids.len(),
        "node_ids": context.node_ids
    }))
}

/// Parse markdown content and create nodes
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
                            Some(context.container_node_id.clone()),
                            context.last_sibling.clone(),
                        )
                        .await?;

                        context.push_heading(node_id.clone(), heading_level);
                        context.track_node(node_id);
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
                                Some(context.container_node_id.clone()),
                                context.last_sibling.clone(),
                            )
                            .await?;
                            context.track_node(node_id);
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
                            Some(context.container_node_id.clone()),
                            context.last_sibling.clone(),
                        )
                        .await?;
                        context.track_node(node_id);
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
                                Some(context.container_node_id.clone()),
                                context.last_sibling.clone(),
                            )
                            .await?;
                            context.track_node(node_id);
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
                                // Ordered list item
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
                                Some(context.container_node_id.clone()),
                                context.last_sibling.clone(),
                            )
                            .await?;

                            // Update list hierarchy based on indentation level
                            let list_level = context.list_stack.len();
                            context.push_list(node_id.clone(), list_level);
                            context.track_node(node_id);

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
) -> Result<String, MCPError> {
    let node = Node::new(
        node_type.to_string(),
        content.to_string(),
        parent_id,
        json!({}),
    );

    let node = Node {
        container_node_id,
        before_sibling_id,
        ..node
    };

    service
        .create_node(node)
        .await
        .map_err(|e| MCPError::node_creation_failed(format!("Failed to create node: {}", e)))
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

// Include tests
#[cfg(test)]
#[path = "markdown_test.rs"]
mod markdown_test;
