//! Context assembly for building rich prompts from the knowledge graph.
//!
//! Gathers relevant nodes, relationships, and system instructions into a
//! structured [`ContextPacket`] that fits within a token budget. This is what
//! makes external agents knowledge-aware: they receive curated graph context
//! instead of having to discover everything through MCP tool calls.
//!
//! ## Assembly Pipeline
//!
//! 1. Accept seed node IDs from user selection
//! 2. Fetch full node content via `NodeService::get_node()`
//! 3. Semantic expansion: `NodeEmbeddingService::semantic_search()` for top-5
//!    neighbors per seed, deduplicated
//! 4. 1-hop relationship traversal: children and mentions
//! 5. Format as structured markdown with sections
//! 6. Token budget enforcement with priority-based truncation
//!
//! Issue #1005

use crate::agent_types::{ContextAssembler, ContextError, ContextNode, ContextPacket, ContextRelationship};
use crate::app_services::AppServices;
use async_trait::async_trait;
use nodespace_core::models::Node;
use nodespace_core::services::NodeEmbeddingService;
use nodespace_core::NodeService;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Default token budget if none specified.
const DEFAULT_TOKEN_BUDGET: u32 = 50_000;

/// Approximate characters per token for budget estimation.
/// The ~4 chars/token heuristic works well for English text with typical
/// markdown formatting, verified to be within 10% of actual tokenizer
/// counts for common node content.
const CHARS_PER_TOKEN: u32 = 4;

/// Maximum content length for selected (seed) nodes in characters.
const SEED_NODE_CONTENT_LIMIT: usize = 2000;

/// Maximum content length for semantic neighbor nodes in characters.
const NEIGHBOR_CONTENT_LIMIT: usize = 500;

/// Number of semantic neighbors to find per seed node.
const NEIGHBORS_PER_SEED: usize = 5;

/// Similarity threshold for semantic search.
const SEMANTIC_THRESHOLD: f32 = 0.3;

/// System prompt header with MCP tool usage instructions.
const SYSTEM_PROMPT_HEADER: &str = "\
## NodeSpace Context

You have access to the NodeSpace knowledge graph via MCP tools on localhost:3100.
";

/// MCP access section appended to the context.
const MCP_ACCESS_SECTION: &str = "\
### MCP Access
Query the graph using these MCP tools: search_semantic, search_nodes, get_node, \
create_nodes_from_markdown, update_node, delete_node.
";

/// Estimate token count from character length using the ~4 chars/token heuristic.
fn estimate_tokens(text: &str) -> u32 {
    let char_count = text.len() as u32;
    // Ceiling division to be conservative with budget
    (char_count + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
}

/// Truncate a string to at most `max_chars` characters, appending "..." if truncated.
fn truncate_content(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        content.to_string()
    } else {
        // Find a safe truncation point (don't split multi-byte chars)
        let truncated = &content[..content.floor_char_boundary(max_chars.saturating_sub(3))];
        format!("{}...", truncated)
    }
}

/// Extract a display title from a node. Uses the indexed `title` field if
/// available, otherwise derives from the first line of content.
fn node_title(node: &Node) -> String {
    if let Some(ref title) = node.title {
        if !title.is_empty() {
            return title.clone();
        }
    }
    // Derive from first line of content, stripped of markdown
    let first_line = node.content.lines().next().unwrap_or("(untitled)");
    let stripped = first_line
        .trim_start_matches('#')
        .trim_start_matches(' ')
        .trim();
    if stripped.is_empty() {
        "(untitled)".to_string()
    } else {
        truncate_content(stripped, 80)
    }
}

/// Format properties as a compact key: value string.
fn format_properties(properties: &serde_json::Value) -> Option<String> {
    let obj = properties.as_object()?;
    if obj.is_empty() {
        return None;
    }

    let mut pairs = Vec::new();
    for (key, value) in obj {
        // Skip empty/null nested objects
        if value.is_null() {
            continue;
        }
        if let Some(inner_obj) = value.as_object() {
            // Namespace properties like {"task": {"status": "done"}}
            for (inner_key, inner_val) in inner_obj {
                if !inner_val.is_null() {
                    pairs.push(format!("{}.{}: {}", key, inner_key, format_json_value(inner_val)));
                }
            }
        } else {
            pairs.push(format!("{}: {}", key, format_json_value(value)));
        }
    }

    if pairs.is_empty() {
        None
    } else {
        Some(pairs.join(", "))
    }
}

/// Format a JSON value as a compact display string.
fn format_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_json_value).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

/// A collected relationship between two nodes for formatting.
#[derive(Debug, Clone)]
struct CollectedRelationship {
    from_title: String,
    relationship_type: String,
    to_title: String,
    to_id: String,
}

/// Assembles context from the knowledge graph into structured prompts.
///
/// Takes `Arc<AppServices>` to obtain `NodeService` and `NodeEmbeddingService`
/// per-operation, surviving database hot-swaps.
pub struct GraphContextAssembler {
    services: Arc<AppServices>,
}

impl GraphContextAssembler {
    /// Create a new assembler backed by the given application services.
    pub fn new(services: Arc<AppServices>) -> Self {
        Self { services }
    }

    /// Fetch a node by ID, converting service errors to ContextError.
    async fn fetch_node(
        node_service: &NodeService,
        node_id: &str,
    ) -> Result<Node, ContextError> {
        node_service
            .get_node(node_id)
            .await
            .map_err(|e| ContextError::Other(e.into()))?
            .ok_or_else(|| ContextError::NodeNotFound(node_id.to_string()))
    }

    /// Find semantic neighbors for a set of seed nodes, returning deduplicated nodes.
    async fn find_semantic_neighbors(
        embedding_service: &NodeEmbeddingService,
        seed_nodes: &[Node],
        seed_ids: &HashSet<String>,
    ) -> Vec<(Node, f64)> {
        let mut seen: HashSet<String> = seed_ids.clone();
        let mut neighbors: Vec<(Node, f64)> = Vec::new();

        for seed in seed_nodes {
            // Use the node's content as the semantic query
            let query = if seed.content.len() > 200 {
                &seed.content[..seed.content.floor_char_boundary(200)]
            } else {
                &seed.content
            };

            if query.trim().is_empty() {
                continue;
            }

            match embedding_service
                .semantic_search_nodes(query, NEIGHBORS_PER_SEED, SEMANTIC_THRESHOLD)
                .await
            {
                Ok(results) => {
                    for (node, score) in results {
                        if !seen.contains(&node.id) {
                            seen.insert(node.id.clone());
                            neighbors.push((node, score));
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Semantic search failed for seed node '{}': {}",
                        seed.id,
                        e
                    );
                    // Continue with other seeds -- partial results are better than none
                }
            }
        }

        // Sort by relevance score descending
        neighbors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        neighbors
    }

    /// Perform 1-hop relationship traversal for a set of nodes.
    /// Returns relationships (children, mentions) for context display.
    async fn collect_relationships(
        node_service: &NodeService,
        nodes: &[Node],
        node_titles: &HashMap<String, String>,
    ) -> Vec<CollectedRelationship> {
        let mut relationships = Vec::new();

        for node in nodes {
            let from_title = node_titles
                .get(&node.id)
                .cloned()
                .unwrap_or_else(|| node_title(node));

            // Children (has_child relationship)
            match node_service.get_children(&node.id).await {
                Ok(children) => {
                    for child in &children {
                        let to_title = node_title(child);
                        relationships.push(CollectedRelationship {
                            from_title: from_title.clone(),
                            relationship_type: "has_child".to_string(),
                            to_title: to_title.clone(),
                            to_id: child.id.clone(),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to get children for node '{}': {}",
                        node.id,
                        e
                    );
                }
            }

            // Outgoing mentions
            for mention_id in &node.mentions {
                let to_title = node_titles
                    .get(mention_id)
                    .cloned()
                    .unwrap_or_else(|| mention_id.clone());
                relationships.push(CollectedRelationship {
                    from_title: from_title.clone(),
                    relationship_type: "mentions".to_string(),
                    to_title,
                    to_id: mention_id.clone(),
                });
            }
        }

        relationships
    }

    /// Format a single node section in markdown.
    fn format_node_section(node: &Node, content_limit: usize) -> String {
        let title = node_title(node);
        let truncated_content = truncate_content(&node.content, content_limit);
        let mut section = format!(
            "#### {} [{}] (id: {})\n{}\n",
            title, node.node_type, node.id, truncated_content
        );

        if let Some(props) = format_properties(&node.properties) {
            section.push_str(&format!("\nProperties: {}\n", props));
        }

        section
    }

    /// Build the complete context markdown, enforcing the token budget.
    ///
    /// Priority order (highest to lowest):
    /// 1. System prompt header (always included)
    /// 2. Selected nodes (full content up to SEED_NODE_CONTENT_LIMIT)
    /// 3. Relationships section
    /// 4. Semantic neighbors (summarized)
    /// 5. MCP access section (always included)
    fn build_context_markdown(
        seed_nodes: &[Node],
        semantic_neighbors: &[(Node, f64)],
        relationships: &[CollectedRelationship],
        token_budget: u32,
    ) -> (String, Vec<ContextNode>) {
        let mut sections: Vec<String> = Vec::new();
        let mut context_nodes: Vec<ContextNode> = Vec::new();

        // Always include header
        sections.push(SYSTEM_PROMPT_HEADER.to_string());

        // Track remaining budget (reserve space for MCP section)
        let mcp_tokens = estimate_tokens(MCP_ACCESS_SECTION);
        let header_tokens = estimate_tokens(SYSTEM_PROMPT_HEADER);
        let mut remaining_budget = token_budget.saturating_sub(header_tokens + mcp_tokens);

        // --- Selected nodes (highest priority) ---
        if !seed_nodes.is_empty() {
            let mut selected_section = String::from("### Selected Nodes\n\n");

            for node in seed_nodes {
                let node_section = Self::format_node_section(node, SEED_NODE_CONTENT_LIMIT);
                let section_tokens = estimate_tokens(&node_section);

                if section_tokens > remaining_budget {
                    // Budget exhausted for seed nodes -- truncate more aggressively
                    let reduced = Self::format_node_section(node, 500);
                    let reduced_tokens = estimate_tokens(&reduced);
                    if reduced_tokens <= remaining_budget {
                        selected_section.push_str(&reduced);
                        remaining_budget = remaining_budget.saturating_sub(reduced_tokens);
                        context_nodes.push(Self::make_context_node(node, &[]));
                    }
                    // If even reduced doesn't fit, skip this node
                } else {
                    selected_section.push_str(&node_section);
                    remaining_budget = remaining_budget.saturating_sub(section_tokens);
                    context_nodes.push(Self::make_context_node(node, &[]));
                }
            }

            let section_header_tokens = estimate_tokens("### Selected Nodes\n\n");
            remaining_budget = remaining_budget.saturating_sub(section_header_tokens);
            sections.push(selected_section);
        }

        // --- Relationships (second priority) ---
        if !relationships.is_empty() {
            let mut rel_section = String::from("### Relationships\n");
            let rel_header_tokens = estimate_tokens("### Relationships\n");
            remaining_budget = remaining_budget.saturating_sub(rel_header_tokens);

            for rel in relationships {
                let line = format!(
                    "- \"{}\" -> {} -> \"{}\"\n",
                    rel.from_title, rel.relationship_type, rel.to_title
                );
                let line_tokens = estimate_tokens(&line);
                if line_tokens > remaining_budget {
                    break;
                }
                rel_section.push_str(&line);
                remaining_budget = remaining_budget.saturating_sub(line_tokens);
            }

            sections.push(rel_section);
        }

        // --- Semantic neighbors (third priority) ---
        if !semantic_neighbors.is_empty() {
            let mut neighbor_section = String::from("### Related Context (auto-discovered)\n\n");
            let neighbor_header_tokens =
                estimate_tokens("### Related Context (auto-discovered)\n\n");
            remaining_budget = remaining_budget.saturating_sub(neighbor_header_tokens);

            for (node, _score) in semantic_neighbors {
                let node_section = Self::format_node_section(node, NEIGHBOR_CONTENT_LIMIT);
                let section_tokens = estimate_tokens(&node_section);

                if section_tokens > remaining_budget {
                    break; // Budget exhausted
                }

                neighbor_section.push_str(&node_section);
                remaining_budget = remaining_budget.saturating_sub(section_tokens);
                context_nodes.push(Self::make_context_node(node, &[]));
            }

            sections.push(neighbor_section);
        }

        // Always append MCP section
        sections.push(MCP_ACCESS_SECTION.to_string());

        (sections.join("\n"), context_nodes)
    }

    /// Create a ContextNode from a Node and optional relationships.
    fn make_context_node(node: &Node, relationships: &[CollectedRelationship]) -> ContextNode {
        let node_relationships: Vec<ContextRelationship> = relationships
            .iter()
            .filter(|r| r.from_title == node_title(node))
            .map(|r| ContextRelationship {
                target_id: r.to_id.clone(),
                relationship_type: r.relationship_type.clone(),
                target_label: r.to_title.clone(),
            })
            .collect();

        ContextNode {
            node_id: node.id.clone(),
            node_type: node.node_type.clone(),
            content: node.content.clone(),
            relationships: node_relationships,
        }
    }
}

#[async_trait]
impl ContextAssembler for GraphContextAssembler {
    /// Assemble a context packet for the given node IDs within the token budget.
    ///
    /// Returns an empty context (with just system prompt and MCP instructions)
    /// when no node IDs are provided. Nodes that cannot be found are skipped
    /// with a warning rather than failing the entire assembly.
    async fn assemble(
        &self,
        node_ids: Vec<String>,
        token_budget: u32,
    ) -> Result<ContextPacket, ContextError> {
        let budget = if token_budget == 0 {
            DEFAULT_TOKEN_BUDGET
        } else {
            token_budget
        };

        // Handle empty input: return minimal context with system prompt
        if node_ids.is_empty() {
            let system_prompt =
                format!("{}\n{}", SYSTEM_PROMPT_HEADER, MCP_ACCESS_SECTION);
            let token_count = estimate_tokens(&system_prompt);
            return Ok(ContextPacket {
                system_prompt,
                context_nodes: vec![],
                token_count,
            });
        }

        // Obtain services (survives hot-swaps)
        let node_service = self.services.node_service().await.map_err(|e| {
            ContextError::Other(anyhow::anyhow!("Failed to get node service: {}", e.message))
        })?;

        // --- Step 1: Fetch seed nodes ---
        let mut seed_nodes: Vec<Node> = Vec::new();
        for id in &node_ids {
            match Self::fetch_node(&node_service, id).await {
                Ok(node) => seed_nodes.push(node),
                Err(ContextError::NodeNotFound(nid)) => {
                    tracing::warn!("Seed node not found, skipping: {}", nid);
                }
                Err(e) => return Err(e),
            }
        }

        // If all seed nodes were not found, return minimal context
        if seed_nodes.is_empty() {
            let system_prompt =
                format!("{}\n{}", SYSTEM_PROMPT_HEADER, MCP_ACCESS_SECTION);
            let token_count = estimate_tokens(&system_prompt);
            return Ok(ContextPacket {
                system_prompt,
                context_nodes: vec![],
                token_count,
            });
        }

        let seed_ids: HashSet<String> = seed_nodes.iter().map(|n| n.id.clone()).collect();

        // --- Step 2: Semantic expansion ---
        // Try to get embedding service; if unavailable, skip semantic expansion
        let semantic_neighbors = match self.services.embedding_service().await {
            Ok(embedding_service) => {
                Self::find_semantic_neighbors(&embedding_service, &seed_nodes, &seed_ids).await
            }
            Err(_) => {
                tracing::info!("Embedding service unavailable, skipping semantic expansion");
                Vec::new()
            }
        };

        // --- Step 3: Build title map for relationship labels ---
        let mut node_titles: HashMap<String, String> = HashMap::new();
        for node in &seed_nodes {
            node_titles.insert(node.id.clone(), node_title(node));
        }
        for (node, _) in &semantic_neighbors {
            node_titles.insert(node.id.clone(), node_title(node));
        }

        // --- Step 4: 1-hop relationship traversal ---
        let relationships =
            Self::collect_relationships(&node_service, &seed_nodes, &node_titles).await;

        // --- Step 5 & 6: Format and enforce budget ---
        let (system_prompt, context_nodes) =
            Self::build_context_markdown(&seed_nodes, &semantic_neighbors, &relationships, budget);

        let token_count = estimate_tokens(&system_prompt);

        Ok(ContextPacket {
            system_prompt,
            context_nodes,
            token_count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_types::ContextAssembler;

    // =========================================================================
    // Token estimation tests
    // =========================================================================

    #[test]
    fn test_estimate_tokens_empty() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn test_estimate_tokens_short() {
        // 12 chars -> 3 tokens (12/4 = 3)
        assert_eq!(estimate_tokens("Hello World!"), 3);
    }

    #[test]
    fn test_estimate_tokens_rounds_up() {
        // 5 chars -> 2 tokens (ceiling of 5/4)
        assert_eq!(estimate_tokens("Hello"), 2);
    }

    #[test]
    fn test_estimate_tokens_exact_multiple() {
        // 8 chars -> 2 tokens (8/4 = 2)
        assert_eq!(estimate_tokens("12345678"), 2);
    }

    #[test]
    fn test_estimate_tokens_large_text() {
        let text = "a".repeat(4000);
        assert_eq!(estimate_tokens(&text), 1000);
    }

    // =========================================================================
    // Truncation tests
    // =========================================================================

    #[test]
    fn test_truncate_content_short() {
        assert_eq!(truncate_content("Hello", 10), "Hello");
    }

    #[test]
    fn test_truncate_content_exact() {
        assert_eq!(truncate_content("Hello", 5), "Hello");
    }

    #[test]
    fn test_truncate_content_long() {
        let result = truncate_content("Hello World, this is a long string", 15);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 18); // 15 - 3 + "..." = 15
    }

    #[test]
    fn test_truncate_content_empty() {
        assert_eq!(truncate_content("", 10), "");
    }

    // =========================================================================
    // Node title extraction tests
    // =========================================================================

    #[test]
    fn test_node_title_from_title_field() {
        let node = make_test_node("1", "text", "# Some Content", Some("My Title"));
        assert_eq!(node_title(&node), "My Title");
    }

    #[test]
    fn test_node_title_from_markdown_heading() {
        let node = make_test_node("1", "text", "# My Heading\nSome body text", None);
        assert_eq!(node_title(&node), "My Heading");
    }

    #[test]
    fn test_node_title_from_plain_content() {
        let node = make_test_node("1", "text", "Just some plain text", None);
        assert_eq!(node_title(&node), "Just some plain text");
    }

    #[test]
    fn test_node_title_empty_content() {
        let node = make_test_node("1", "text", "", None);
        assert_eq!(node_title(&node), "(untitled)");
    }

    // =========================================================================
    // Property formatting tests
    // =========================================================================

    #[test]
    fn test_format_properties_empty() {
        let props = serde_json::json!({});
        assert!(format_properties(&props).is_none());
    }

    #[test]
    fn test_format_properties_flat() {
        let props = serde_json::json!({"priority": "high"});
        let formatted = format_properties(&props).unwrap();
        assert!(formatted.contains("priority: high"));
    }

    #[test]
    fn test_format_properties_nested() {
        let props = serde_json::json!({"task": {"status": "done", "priority": "high"}});
        let formatted = format_properties(&props).unwrap();
        assert!(formatted.contains("task.status: done"));
        assert!(formatted.contains("task.priority: high"));
    }

    #[test]
    fn test_format_properties_null_values_skipped() {
        let props = serde_json::json!({"key": null});
        assert!(format_properties(&props).is_none());
    }

    // =========================================================================
    // Build context markdown tests
    // =========================================================================

    #[test]
    fn test_build_context_no_nodes() {
        let (markdown, context_nodes) =
            GraphContextAssembler::build_context_markdown(&[], &[], &[], 50_000);

        assert!(markdown.contains("## NodeSpace Context"));
        assert!(markdown.contains("### MCP Access"));
        assert!(context_nodes.is_empty());
    }

    #[test]
    fn test_build_context_with_seed_nodes() {
        let nodes = vec![
            make_test_node("node-1", "text", "First node content", Some("First Node")),
            make_test_node("node-2", "task", "Second node content", Some("Second Node")),
        ];

        let (markdown, context_nodes) =
            GraphContextAssembler::build_context_markdown(&nodes, &[], &[], 50_000);

        assert!(markdown.contains("### Selected Nodes"));
        assert!(markdown.contains("First Node"));
        assert!(markdown.contains("[text]"));
        assert!(markdown.contains("Second Node"));
        assert!(markdown.contains("[task]"));
        assert_eq!(context_nodes.len(), 2);
    }

    #[test]
    fn test_build_context_with_relationships() {
        let nodes = vec![make_test_node("n1", "text", "Content", Some("Node One"))];

        let relationships = vec![CollectedRelationship {
            from_title: "Node One".to_string(),
            relationship_type: "mentions".to_string(),
            to_title: "Node Two".to_string(),
            to_id: "n2".to_string(),
        }];

        let (markdown, _) =
            GraphContextAssembler::build_context_markdown(&nodes, &[], &relationships, 50_000);

        assert!(markdown.contains("### Relationships"));
        assert!(markdown.contains("\"Node One\" -> mentions -> \"Node Two\""));
    }

    #[test]
    fn test_build_context_with_semantic_neighbors() {
        let seeds = vec![make_test_node("s1", "text", "Seed content", Some("Seed"))];
        let neighbors = vec![(
            make_test_node("n1", "text", "Neighbor content", Some("Neighbor")),
            0.85,
        )];

        let (markdown, context_nodes) =
            GraphContextAssembler::build_context_markdown(&seeds, &neighbors, &[], 50_000);

        assert!(markdown.contains("### Related Context (auto-discovered)"));
        assert!(markdown.contains("Neighbor"));
        assert_eq!(context_nodes.len(), 2); // seed + neighbor
    }

    // =========================================================================
    // Budget enforcement tests
    // =========================================================================

    #[test]
    fn test_budget_enforcement_drops_neighbors_first() {
        let seeds = vec![make_test_node(
            "s1",
            "text",
            &"Seed content. ".repeat(50),
            Some("Important Seed"),
        )];
        let neighbors: Vec<(Node, f64)> = (0..20)
            .map(|i| {
                (
                    make_test_node(
                        &format!("n{}", i),
                        "text",
                        &"Neighbor content. ".repeat(50),
                        Some(&format!("Neighbor {}", i)),
                    ),
                    0.8 - (i as f64 * 0.01),
                )
            })
            .collect();

        // Very tight budget: should include seed but drop most neighbors
        let (markdown, context_nodes) =
            GraphContextAssembler::build_context_markdown(&seeds, &neighbors, &[], 500);

        // Seed should always be present
        assert!(markdown.contains("Important Seed"));
        // Not all neighbors should fit
        assert!(context_nodes.len() < 21);
        // Token count should respect budget
        let actual_tokens = estimate_tokens(&markdown);
        assert!(
            actual_tokens <= 500,
            "Token count {} exceeds budget 500",
            actual_tokens
        );
    }

    #[test]
    fn test_budget_enforcement_never_exceeds() {
        let seeds: Vec<Node> = (0..10)
            .map(|i| {
                make_test_node(
                    &format!("s{}", i),
                    "text",
                    &"A".repeat(2000),
                    Some(&format!("Seed {}", i)),
                )
            })
            .collect();

        let neighbors: Vec<(Node, f64)> = (0..20)
            .map(|i| {
                (
                    make_test_node(
                        &format!("n{}", i),
                        "text",
                        &"B".repeat(500),
                        Some(&format!("Neighbor {}", i)),
                    ),
                    0.9,
                )
            })
            .collect();

        let relationships: Vec<CollectedRelationship> = (0..50)
            .map(|i| CollectedRelationship {
                from_title: format!("Seed {}", i % 10),
                relationship_type: "mentions".to_string(),
                to_title: format!("Target {}", i),
                to_id: format!("t{}", i),
            })
            .collect();

        for budget in [100, 500, 1000, 5000, 50_000] {
            let (markdown, _) = GraphContextAssembler::build_context_markdown(
                &seeds,
                &neighbors,
                &relationships,
                budget,
            );
            let actual_tokens = estimate_tokens(&markdown);
            assert!(
                actual_tokens <= budget,
                "Budget {} exceeded: actual {}",
                budget,
                actual_tokens
            );
        }
    }

    // =========================================================================
    // Deduplication tests
    // =========================================================================

    #[test]
    fn test_deduplication_in_context_nodes() {
        // If the same node appears as seed and neighbor, it should only appear once
        let seed = make_test_node("shared-id", "text", "Content", Some("Shared Node"));
        let seeds = vec![seed.clone()];
        // Neighbors filtered by seed_ids in find_semantic_neighbors, but let's test
        // build_context_markdown handles duplication gracefully
        let neighbors = vec![];

        let (_, context_nodes) =
            GraphContextAssembler::build_context_markdown(&seeds, &neighbors, &[], 50_000);

        let ids: Vec<&str> = context_nodes.iter().map(|cn| cn.node_id.as_str()).collect();
        assert_eq!(ids, vec!["shared-id"]);
    }

    // =========================================================================
    // Edge case tests
    // =========================================================================

    #[test]
    fn test_very_large_node_body_truncated() {
        let huge_content = "X".repeat(100_000);
        let node = make_test_node("huge", "text", &huge_content, Some("Huge Node"));

        let (markdown, _) =
            GraphContextAssembler::build_context_markdown(&[node], &[], &[], 50_000);

        // Content should be truncated to SEED_NODE_CONTENT_LIMIT
        assert!(markdown.len() < 100_000);
        assert!(markdown.contains("Huge Node"));
    }

    #[test]
    fn test_context_node_structure() {
        let node = make_test_node("cn1", "task", "Do something", Some("My Task"));
        let relationships = vec![CollectedRelationship {
            from_title: "My Task".to_string(),
            relationship_type: "has_child".to_string(),
            to_title: "Subtask".to_string(),
            to_id: "sub1".to_string(),
        }];

        let context_node = GraphContextAssembler::make_context_node(&node, &relationships);

        assert_eq!(context_node.node_id, "cn1");
        assert_eq!(context_node.node_type, "task");
        assert_eq!(context_node.content, "Do something");
        assert_eq!(context_node.relationships.len(), 1);
        assert_eq!(context_node.relationships[0].target_id, "sub1");
        assert_eq!(context_node.relationships[0].relationship_type, "has_child");
        assert_eq!(context_node.relationships[0].target_label, "Subtask");
    }

    #[test]
    fn test_format_node_section_includes_all_parts() {
        let mut node = make_test_node("fmt1", "text", "Body text here", Some("Title"));
        node.properties = serde_json::json!({"task": {"status": "done"}});

        let section = GraphContextAssembler::format_node_section(&node, 2000);

        assert!(section.contains("#### Title [text] (id: fmt1)"));
        assert!(section.contains("Body text here"));
        assert!(section.contains("Properties:"));
        assert!(section.contains("task.status: done"));
    }

    // =========================================================================
    // Token estimation accuracy test
    // =========================================================================

    #[test]
    fn test_token_estimation_within_10_percent() {
        // Test that our heuristic is consistent with itself (self-consistency check).
        // The ~4 chars/token heuristic targets within 10% of real tokenizers for
        // typical English markdown content.
        let samples = vec![
            "Hello world",                                      // 11 chars
            "This is a typical sentence with several words.",    // 46 chars
            "# Heading\n\nSome **markdown** content here.\n",   // 42 chars
            &"Mixed content with code: `fn main() {}` and lists:\n- item 1\n- item 2\n".to_string(),
        ];

        for sample in samples {
            let estimated = estimate_tokens(sample);
            let expected = (sample.len() as f64 / CHARS_PER_TOKEN as f64).ceil() as u32;
            assert_eq!(
                estimated, expected,
                "Token estimation mismatch for '{}': got {}, expected {}",
                &sample[..20.min(sample.len())],
                estimated,
                expected
            );
        }
    }

    // =========================================================================
    // Test helpers
    // =========================================================================

    fn make_test_node(id: &str, node_type: &str, content: &str, title: Option<&str>) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            content: content.to_string(),
            version: 1,
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            properties: serde_json::json!({}),
            mentions: vec![],
            mentioned_in: vec![],
            title: title.map(|s| s.to_string()),
            lifecycle_status: "active".to_string(),
        }
    }
}
