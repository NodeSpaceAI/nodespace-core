//! Pre-turn skill discovery pipeline.
//!
//! Runs before each agent turn to detect user intent and inject the most
//! relevant skill into the prompt context. Uses pattern matching + semantic
//! search (zero model inference cost).
//!
//! Issue #1050, ADR-030 Phase 3.

use std::sync::Arc;

use nodespace_core::models::Node;
use nodespace_core::services::NodeEmbeddingService;

use crate::agent_types::ToolDefinition;
use crate::intent::{self, ExtractedIntent};

/// High confidence threshold: full skill details returned.
pub const SKILL_HIGH_CONFIDENCE: f64 = 0.8;

/// Medium confidence threshold: description only returned.
pub const SKILL_MEDIUM_CONFIDENCE: f64 = 0.6;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Result of the pre-turn skill discovery pipeline.
#[derive(Debug, Clone)]
pub struct SkillMatch {
    /// The matched skill node
    pub skill: Node,
    /// Similarity score (0.0 - 1.0)
    pub confidence: f64,
    /// The intent query that led to this match
    pub intent: ExtractedIntent,
    /// Tool names whitelisted by this skill
    pub tool_whitelist: Vec<String>,
    /// Max iterations for this skill's ReAct loop
    pub max_iterations: usize,
}

/// Configuration for the skill pipeline.
#[derive(Debug, Clone)]
pub struct SkillPipelineConfig {
    /// Minimum confidence threshold for skill injection (default: 0.8)
    pub confidence_threshold: f64,
    /// Maximum number of skill candidates to consider (default: 3)
    pub search_limit: usize,
}

impl Default for SkillPipelineConfig {
    fn default() -> Self {
        Self {
            confidence_threshold: SKILL_HIGH_CONFIDENCE,
            search_limit: 3,
        }
    }
}

// ---------------------------------------------------------------------------
// SkillPipeline
// ---------------------------------------------------------------------------

/// Pre-turn skill discovery pipeline.
///
/// Pipeline:
/// 1. Extract intent from user message (pattern match + filler strip)
/// 2. Semantic search for matching skill nodes
/// 3. Apply confidence threshold
/// 4. Return matched skill or None (base agent fallback)
pub struct SkillPipeline {
    embedding_service: Option<Arc<NodeEmbeddingService>>,
    config: SkillPipelineConfig,
}

impl SkillPipeline {
    pub fn new(embedding_service: Option<Arc<NodeEmbeddingService>>) -> Self {
        Self {
            embedding_service,
            config: SkillPipelineConfig::default(),
        }
    }

    pub fn with_config(mut self, config: SkillPipelineConfig) -> Self {
        self.config = config;
        self
    }

    /// Run the pre-turn pipeline: extract intent -> search skills -> threshold.
    ///
    /// Returns `Some(SkillMatch)` if a skill above the confidence threshold
    /// was found, `None` otherwise (caller should use base agent with all tools).
    pub async fn find_skill(&self, user_message: &str) -> Option<SkillMatch> {
        // Step 1: Extract intent
        let intent = intent::extract_intent(user_message);

        if intent.query.is_empty() {
            tracing::debug!("Empty intent extracted, skipping skill search");
            return None;
        }

        tracing::debug!(
            query = %intent.query,
            from_pattern = intent.from_pattern,
            "Intent extracted from user message"
        );

        // Step 2: Semantic search for skill nodes
        let embedding_service = self.embedding_service.as_ref()?;

        let search_results = match embedding_service
            .semantic_search(&intent.query, self.config.search_limit * 2, 0.3)
            .await
        {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!(error = %e, "Skill semantic search failed");
                return None;
            }
        };

        // Filter to skill nodes only
        let skill_results: Vec<_> = search_results
            .into_iter()
            .filter(|r| {
                r.node
                    .as_ref()
                    .map(|n| n.node_type == "skill")
                    .unwrap_or(false)
            })
            .take(self.config.search_limit)
            .collect();

        if skill_results.is_empty() {
            tracing::debug!(
                intent = %intent.query,
                "No skill nodes found for intent"
            );
            return None;
        }

        // Step 3: Confidence threshold
        let top = &skill_results[0];
        let confidence = top.max_similarity;

        tracing::info!(
            intent = %intent.query,
            skill_id = %top.node_id,
            skill_name = top.node.as_ref().map(|n| n.content.as_str()).unwrap_or("?"),
            confidence = confidence,
            threshold = self.config.confidence_threshold,
            "Skill search result"
        );

        if confidence < self.config.confidence_threshold {
            tracing::debug!(
                confidence = confidence,
                threshold = self.config.confidence_threshold,
                "Skill below confidence threshold, using base agent"
            );
            return None;
        }

        // Step 4: Build SkillMatch from the top result
        let skill_node = top.node.clone()?;
        let tool_whitelist = extract_tool_whitelist(&skill_node);
        let max_iterations =
            crate::props::get_prop(&skill_node.properties, "skill", "max_iterations")
                .and_then(|v| v.as_u64())
                .unwrap_or(2) as usize;

        Some(SkillMatch {
            skill: skill_node,
            confidence,
            intent,
            tool_whitelist,
            max_iterations,
        })
    }

    /// Filter tool definitions to only those in the skill's whitelist.
    ///
    /// If the whitelist is empty, returns all tools (no filtering).
    pub fn scope_tools(
        &self,
        all_tools: &[ToolDefinition],
        skill_match: &SkillMatch,
    ) -> Vec<ToolDefinition> {
        if skill_match.tool_whitelist.is_empty() {
            return all_tools.to_vec();
        }

        all_tools
            .iter()
            .filter(|t| skill_match.tool_whitelist.contains(&t.name))
            .cloned()
            .collect()
    }

    /// Get default seed skill nodes for first-run creation.
    pub fn seed_skill_nodes() -> Vec<SeedSkill> {
        vec![
            SeedSkill {
                markdown_content: "# Research & Search".to_string(),
                root_properties: serde_json::json!({
                    "description": "Search and explore the knowledge graph to find relevant information, discover connections, and answer questions about stored knowledge.",
                    "tool_whitelist": ["search_semantic", "search_nodes", "get_node"],
                    "max_iterations": 2,
                    "output_format": "text",
                }),
            },
            SeedSkill {
                markdown_content: "# Node Creation".to_string(),
                root_properties: serde_json::json!({
                    "description": "Create new instances of existing node types — add a task, text note, or an entry for a custom type. Use when user wants to add a new record or item.",
                    "tool_whitelist": ["create_node", "get_node"],
                    "max_iterations": 2,
                    "output_format": "text",
                }),
            },
            SeedSkill {
                markdown_content: "# Schema Creation".to_string(),
                root_properties: serde_json::json!({
                    "description": "Define a new entity type or schema with custom fields, enums, and relationships. Use when user says 'new type', 'node type', 'define fields', 'create schema', or wants to design a new kind of entity like Project, Customer, or Invoice.",
                    "tool_whitelist": ["create_schema", "get_node"],
                    "max_iterations": 2,
                    "output_format": "text",
                }),
            },
            SeedSkill {
                markdown_content: "# Graph Editing".to_string(),
                root_properties: serde_json::json!({
                    "description": "Modify existing nodes in the knowledge graph - update content, properties, titles, and metadata. For tasks, use update_task_status to change status.",
                    "tool_whitelist": ["update_node", "update_task_status", "get_node", "search_nodes"],
                    "max_iterations": 2,
                    "output_format": "text",
                }),
            },
            SeedSkill {
                markdown_content: "# Relationship Management".to_string(),
                root_properties: serde_json::json!({
                    "description": "Create connections between nodes, explore relationships, and traverse the knowledge graph.",
                    "tool_whitelist": ["create_relationship", "get_related_nodes", "get_node"],
                    "max_iterations": 2,
                    "output_format": "text",
                }),
            },
        ]
    }
}

/// Extract tool_whitelist from a skill node's properties.
fn extract_tool_whitelist(node: &Node) -> Vec<String> {
    crate::props::get_prop(&node.properties, "skill", "tool_whitelist")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Descriptor for a seed skill node to be created on first run.
///
/// Uses the unified markdown import pipeline via `prepare_nodes_from_template`.
/// The first line of `markdown_content` (optionally prefixed with `#`) becomes
/// the skill node's content/title. Any remaining sections become child prompt nodes.
#[derive(Debug, Clone)]
pub struct SeedSkill {
    /// Markdown string whose first line is the skill name (e.g. `# Research & Search`).
    /// Subsequent sections, if any, become child prompt nodes.
    pub markdown_content: String,
    /// Properties merged into the root skill node (description, tool_whitelist, etc.).
    pub root_properties: serde_json::Value,
}

impl SeedSkill {
    /// Convert to a root `Node` and optional child `Node`s via the markdown template pipeline.
    ///
    /// Returns `(skill_node, guidance_nodes)`.
    pub fn to_nodes(&self) -> (Node, Vec<Node>) {
        use nodespace_core::mcp::handlers::markdown::prepare_nodes_from_template;

        let child_properties = serde_json::json!({
            "source": "built-in",
            "template_syntax": "plain",
        });

        let (root_prepared, children_prepared) = prepare_nodes_from_template(
            &self.markdown_content,
            "skill",
            &self.root_properties,
            "prompt",
            &child_properties,
        )
        .expect("SeedSkill markdown must be non-empty and parseable");

        let mut root_node = Node::new(
            root_prepared.node_type.clone(),
            root_prepared.content.clone(),
            root_prepared.properties.clone(),
        );
        root_node.title = Some(root_prepared.content.clone());

        let child_nodes = children_prepared
            .into_iter()
            .map(|p| {
                let mut node =
                    Node::new(p.node_type.clone(), p.content.clone(), p.properties.clone());
                node.title = Some(p.content.clone());
                node
            })
            .collect();

        (root_node, child_nodes)
    }

    /// Return the skill name (first non-empty line of markdown, stripped of heading marker).
    pub fn name(&self) -> &str {
        self.markdown_content
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().trim_start_matches('#').trim())
            .unwrap_or("")
    }

    /// Return the tool whitelist from root_properties.
    pub fn tool_whitelist(&self) -> Vec<String> {
        self.root_properties
            .get("tool_whitelist")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_skills_have_valid_properties() {
        let seeds = SkillPipeline::seed_skill_nodes();
        assert_eq!(seeds.len(), 5, "Should have 5 seed skills");

        for seed in &seeds {
            assert!(!seed.name().is_empty());
            assert!(
                seed.root_properties.get("description").is_some(),
                "Seed must have description"
            );
            assert!(!seed.tool_whitelist().is_empty(), "Skills must have tools");
            assert!(
                seed.root_properties
                    .get("max_iterations")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    > 0,
                "max_iterations must be > 0"
            );
        }
    }

    #[test]
    fn schema_creation_skill_uses_dedicated_tool() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let schema_skill = seeds
            .iter()
            .find(|s| s.name() == "Schema Creation")
            .expect("Schema Creation skill should exist");

        assert!(
            schema_skill
                .tool_whitelist()
                .contains(&"create_schema".to_string()),
            "Schema Creation skill should whitelist create_schema"
        );
    }

    #[test]
    fn graph_editing_skill_includes_task_tool() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let editing_skill = seeds
            .iter()
            .find(|s| s.name() == "Graph Editing")
            .expect("Graph Editing skill should exist");

        assert!(
            editing_skill
                .tool_whitelist()
                .contains(&"update_task_status".to_string()),
            "Graph Editing skill should whitelist update_task_status"
        );
    }

    #[test]
    fn seed_skill_to_nodes_conversion() {
        let seeds = SkillPipeline::seed_skill_nodes();
        for seed in &seeds {
            let (node, children) = seed.to_nodes();
            assert_eq!(node.node_type, "skill");
            // Node::new() generates a UUID (36 chars with hyphens)
            assert_eq!(node.id.len(), 36, "Node ID should be a UUID");
            assert_eq!(node.id.chars().filter(|c| *c == '-').count(), 4);
            assert_eq!(node.content, seed.name());
            assert!(node.title.is_some());
            assert_eq!(node.title.as_deref().unwrap(), seed.name());
            // No guidance prompts in current seeds
            assert!(
                children.is_empty(),
                "Current seeds have no guidance prompts"
            );

            let whitelist = extract_tool_whitelist(&node);
            assert_eq!(whitelist, seed.tool_whitelist());
        }
    }

    #[test]
    fn extract_whitelist_from_node() {
        let node = Node {
            id: "test".to_string(),
            node_type: "skill".to_string(),
            content: "Test Skill".to_string(),
            properties: serde_json::json!({
                "tool_whitelist": ["search_semantic", "get_node"]
            }),
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            version: 1,
            lifecycle_status: "active".to_string(),
            title: None,
            mentions: Vec::new(),
            mentioned_in: Vec::new(),
        };
        let whitelist = extract_tool_whitelist(&node);
        assert_eq!(whitelist, vec!["search_semantic", "get_node"]);
    }

    #[test]
    fn extract_whitelist_empty_when_missing() {
        let node = Node {
            id: "test".to_string(),
            node_type: "skill".to_string(),
            content: "Test".to_string(),
            properties: serde_json::json!({}),
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            version: 1,
            lifecycle_status: "active".to_string(),
            title: None,
            mentions: Vec::new(),
            mentioned_in: Vec::new(),
        };
        assert!(extract_tool_whitelist(&node).is_empty());
    }

    #[test]
    fn scope_tools_filters_correctly() {
        use serde_json::json;

        let pipeline = SkillPipeline::new(None);
        let all_tools = vec![
            ToolDefinition {
                name: "search_semantic".into(),
                description: "Search".into(),
                parameters_schema: json!({}),
            },
            ToolDefinition {
                name: "create_node".into(),
                description: "Create".into(),
                parameters_schema: json!({}),
            },
            ToolDefinition {
                name: "update_node".into(),
                description: "Update".into(),
                parameters_schema: json!({}),
            },
        ];

        let skill_match = SkillMatch {
            skill: Node {
                id: "test".to_string(),
                node_type: "skill".to_string(),
                content: "Search".to_string(),
                properties: serde_json::json!({}),
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                version: 1,
                lifecycle_status: "active".to_string(),
                title: None,
                mentions: Vec::new(),
                mentioned_in: Vec::new(),
            },
            confidence: 0.9,
            intent: ExtractedIntent {
                query: "search".to_string(),
                from_pattern: true,
            },
            tool_whitelist: vec!["search_semantic".to_string()],
            max_iterations: 2,
        };

        let scoped = pipeline.scope_tools(&all_tools, &skill_match);
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].name, "search_semantic");
    }

    #[test]
    fn scope_tools_empty_whitelist_returns_all() {
        use serde_json::json;

        let pipeline = SkillPipeline::new(None);
        let all_tools = vec![
            ToolDefinition {
                name: "a".into(),
                description: "".into(),
                parameters_schema: json!({}),
            },
            ToolDefinition {
                name: "b".into(),
                description: "".into(),
                parameters_schema: json!({}),
            },
        ];

        let skill_match = SkillMatch {
            skill: Node {
                id: "test".to_string(),
                node_type: "skill".to_string(),
                content: "Empty".to_string(),
                properties: serde_json::json!({}),
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                version: 1,
                lifecycle_status: "active".to_string(),
                title: None,
                mentions: Vec::new(),
                mentioned_in: Vec::new(),
            },
            confidence: 0.9,
            intent: ExtractedIntent {
                query: "test".to_string(),
                from_pattern: false,
            },
            tool_whitelist: vec![], // Empty whitelist = no filtering
            max_iterations: 2,
        };

        let scoped = pipeline.scope_tools(&all_tools, &skill_match);
        assert_eq!(scoped.len(), 2, "Empty whitelist should return all tools");
    }

    #[test]
    fn default_config_values() {
        let config = SkillPipelineConfig::default();
        assert!((config.confidence_threshold - SKILL_HIGH_CONFIDENCE).abs() < f64::EPSILON);
        assert_eq!(config.search_limit, 3);
    }
}
