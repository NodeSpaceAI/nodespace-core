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
                name: "Research & Search".to_string(),
                description: "Search and explore the knowledge graph to find relevant information, discover connections, and answer questions about stored knowledge.".to_string(),
                tool_whitelist: vec![
                    "search_semantic".to_string(),
                    "search_nodes".to_string(),
                    "get_node".to_string(),
                ],
                max_iterations: 4,
                output_format: "text".to_string(),
                guidance_prompts: vec![
                    SeedGuidancePrompt {
                        title: "Research & Search Guidance".to_string(),
                        content: r#"When answering questions about stored knowledge:

SEARCH FIRST: Always call search_semantic with a natural language query. Results are ordered by relevance — the first result is the best match.

RESULT STRUCTURE: Each result contains:
- id: node ID (use this for follow-up get_node calls)
- title: document title
- score: similarity score (0-1, higher = more relevant)
- snippet: short content preview
- markdown: full document content (present for top N results based on include_markdown, default 1)

USE MARKDOWN DIRECTLY: If the top result has a non-empty 'markdown' field, that is the complete document. Summarize or answer from it immediately — do NOT call get_node or search_nodes again.

FETCH ADDITIONAL CONTENT: Only call get_node with format=markdown if you need full content for a lower-ranked result that did not include markdown.

MULTIPLE DOCUMENTS: If the user asks about multiple topics, call search_semantic once per topic rather than searching broadly and fetching each result individually."#.to_string(),
                        priority: 1,
                    },
                ],
            },
            SeedSkill {
                name: "Node Creation".to_string(),
                description: "Create new instances of existing node types — add a task, text note, or an entry for a custom type. Use when user wants to add a new record or item.".to_string(),
                tool_whitelist: vec![
                    "create_node".to_string(),
                    "get_node".to_string(),
                ],
                max_iterations: 3,
                output_format: "text".to_string(),
                guidance_prompts: vec![],
            },
            SeedSkill {
                name: "Schema Creation".to_string(),
                description: "Define a new entity type or schema with custom fields, enums, and relationships. Use when user says 'new type', 'node type', 'define fields', 'create schema', or wants to design a new kind of entity like Project, Customer, or Invoice.".to_string(),
                tool_whitelist: vec![
                    "create_schema".to_string(),
                    "get_node".to_string(),
                ],
                max_iterations: 3,
                output_format: "text".to_string(),
                guidance_prompts: vec![
                    SeedGuidancePrompt {
                        title: "Schema Creation Guidance".to_string(),
                        content: r#"When creating a schema:

FIELDS: Only define type-specific fields. Do NOT add a 'name' or 'title' field — every node already has a built-in content/title field. EXCEPTION: if you use a 'name' placeholder in title_template (e.g. "{name} ({status})"), you MUST define 'name' as a text field so title generation works. A 'description' field is acceptable when it adds value beyond the title. Good fields: status (enum), due_date (date), priority (enum), budget (number), owner (text).

ENUMS: Use lowercase values with readable labels, e.g. {"value": "in_progress", "label": "In Progress"}.

RELATIONSHIPS: Use relationships (not fields) when a field references another node type. The targetType MUST be an existing schema ID from the ENTITY TYPES list in the system prompt — do NOT invent types that aren't listed. If the target type doesn't exist yet, omit the relationship entirely. Examples:
- Invoice billed_to customer (one): {"name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one"}
- Project has_task task (many): {"name": "has_task", "targetType": "task", "direction": "out", "cardinality": "many"}

TITLE TEMPLATE: Set title_template when a node's identity comes from its fields rather than free-form content. Use {field_name} placeholders. CRITICAL: every placeholder in title_template MUST be defined as a field in the fields array. Examples:
- Customer with fields [first_name, last_name]: title_template = "{first_name} {last_name}"
- Invoice with fields [invoice_number, ...]: title_template = "Invoice #{invoice_number}"
- Project with fields [name, status, ...]: title_template = "{name} ({status})"
Omit title_template if the content/title field alone identifies the node.

EXAMPLE — Invoice schema (references existing 'customer' type):
{
  "name": "Invoice",
  "description": "A billing invoice linked to a customer",
  "title_template": "Invoice #{invoice_number}",
  "fields": [
    {"name": "invoice_number", "type": "text", "required": true},
    {"name": "issue_date", "type": "date", "required": true},
    {"name": "due_date", "type": "date"},
    {"name": "amount", "type": "number", "required": true},
    {"name": "status", "type": "enum", "required": true, "coreValues": [
      {"value": "draft", "label": "Draft"},
      {"value": "sent", "label": "Sent"},
      {"value": "paid", "label": "Paid"},
      {"value": "overdue", "label": "Overdue"}
    ]}
  ],
  "relationships": [
    {"name": "billed_to", "targetType": "customer", "direction": "out", "cardinality": "one"}
  ]
}

EXAMPLE — Project schema (title_template uses {name} AND {status}, so BOTH are in fields):
{
  "name": "Project",
  "description": "A tracked project with status and timeline",
  "title_template": "{name} ({status})",
  "fields": [
    {"name": "name", "type": "text", "required": true},
    {"name": "status", "type": "enum", "required": true, "coreValues": [
      {"value": "planning", "label": "Planning"},
      {"value": "active", "label": "Active"},
      {"value": "on_hold", "label": "On Hold"},
      {"value": "completed", "label": "Completed"}
    ]},
    {"name": "start_date", "type": "date"},
    {"name": "due_date", "type": "date"},
    {"name": "budget", "type": "number"},
    {"name": "owner", "type": "text"}
  ],
  "relationships": [
    {"name": "has_task", "targetType": "task", "direction": "out", "cardinality": "many"}
  ]
}"#.to_string(),
                        priority: 1,
                    },
                ],
            },
            SeedSkill {
                name: "Graph Editing".to_string(),
                description: "Modify existing nodes in the knowledge graph - update content, properties, titles, and metadata. For tasks, use update_task_status to change status.".to_string(),
                tool_whitelist: vec![
                    "update_node".to_string(),
                    "update_task_status".to_string(),
                    "get_node".to_string(),
                    "search_nodes".to_string(),
                ],
                max_iterations: 3,
                output_format: "text".to_string(),
                guidance_prompts: vec![],
            },
            SeedSkill {
                name: "Relationship Management".to_string(),
                description: "Create connections between nodes, explore relationships, and traverse the knowledge graph.".to_string(),
                tool_whitelist: vec![
                    "create_relationship".to_string(),
                    "get_related_nodes".to_string(),
                    "get_node".to_string(),
                ],
                max_iterations: 3,
                output_format: "text".to_string(),
                guidance_prompts: vec![],
            },
            SeedSkill {
                name: "Node Deletion".to_string(),
                description: "Delete nodes from the knowledge graph. Use when user wants to remove, delete, or trash a node or record.".to_string(),
                tool_whitelist: vec![
                    "delete_node".to_string(),
                    "get_node".to_string(),
                ],
                max_iterations: 3,
                output_format: "text".to_string(),
                guidance_prompts: vec![],
            },
            SeedSkill {
                name: "Bulk Import".to_string(),
                description: "Import documents and create node hierarchies from markdown. Use when user wants to import, bulk create, or create nodes from a markdown document.".to_string(),
                tool_whitelist: vec![
                    "create_nodes_from_markdown".to_string(),
                ],
                max_iterations: 2,
                output_format: "text".to_string(),
                guidance_prompts: vec![],
            },
            SeedSkill {
                name: "Organization".to_string(),
                description: "Organize nodes into collections and categories. Use when user wants to add to a collection, categorize, or group nodes.".to_string(),
                tool_whitelist: vec![
                    "create_relationship".to_string(),
                    "get_node".to_string(),
                ],
                max_iterations: 3,
                output_format: "text".to_string(),
                guidance_prompts: vec![],
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
#[derive(Debug, Clone)]
pub struct SeedSkill {
    pub name: String,
    pub description: String,
    pub tool_whitelist: Vec<String>,
    pub max_iterations: usize,
    pub output_format: String,
    /// Child prompt nodes containing guidance, few-shot examples, etc.
    pub guidance_prompts: Vec<SeedGuidancePrompt>,
}

/// A child prompt node that provides guidance for a skill.
#[derive(Debug, Clone)]
pub struct SeedGuidancePrompt {
    pub title: String,
    pub content: String,
    pub priority: i64,
}

impl SeedGuidancePrompt {
    /// Convert to a Node for creation via NodeService.
    pub fn to_node(&self) -> Node {
        let mut node = Node::new(
            "prompt".to_string(),
            self.content.clone(),
            serde_json::json!({
                "priority": self.priority,
                "template_syntax": "plain",
                "source": "built-in",
            }),
        );
        node.title = Some(self.title.clone());
        node
    }
}

impl SeedSkill {
    /// Convert to a Node for creation via NodeService.
    pub fn to_node(&self) -> Node {
        let mut node = Node::new(
            "skill".to_string(),
            self.name.clone(),
            serde_json::json!({
                "description": self.description,
                "tool_whitelist": self.tool_whitelist,
                "max_iterations": self.max_iterations,
                "output_format": self.output_format,
            }),
        );
        node.title = Some(self.name.clone());
        node
    }

    /// Convert guidance prompts to child Nodes.
    pub fn guidance_nodes(&self) -> Vec<Node> {
        self.guidance_prompts
            .iter()
            .map(|g| {
                let mut node = Node::new(
                    "prompt".to_string(),
                    g.content.clone(),
                    serde_json::json!({
                        "priority": g.priority,
                        "template_syntax": "plain",
                        "source": "built-in",
                    }),
                );
                node.title = Some(g.title.clone());
                node
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_skills_have_valid_properties() {
        let seeds = SkillPipeline::seed_skill_nodes();
        assert_eq!(seeds.len(), 8, "Should have 8 seed skills");

        for seed in &seeds {
            assert!(!seed.name.is_empty());
            assert!(!seed.description.is_empty());
            assert!(!seed.tool_whitelist.is_empty(), "Skills must have tools");
            assert!(seed.max_iterations > 0);
        }
    }

    #[test]
    fn schema_creation_skill_uses_dedicated_tool() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let schema_skill = seeds
            .iter()
            .find(|s| s.name == "Schema Creation")
            .expect("Schema Creation skill should exist");

        assert!(
            schema_skill
                .tool_whitelist
                .contains(&"create_schema".to_string()),
            "Schema Creation skill should whitelist create_schema"
        );
    }

    #[test]
    fn graph_editing_skill_includes_task_tool() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let editing_skill = seeds
            .iter()
            .find(|s| s.name == "Graph Editing")
            .expect("Graph Editing skill should exist");

        assert!(
            editing_skill
                .tool_whitelist
                .contains(&"update_task_status".to_string()),
            "Graph Editing skill should whitelist update_task_status"
        );
    }

    #[test]
    fn seed_skill_to_node_conversion() {
        let seeds = SkillPipeline::seed_skill_nodes();
        for seed in &seeds {
            let node = seed.to_node();
            assert_eq!(node.node_type, "skill");
            // Node::new() generates a UUID (36 chars with hyphens)
            assert_eq!(node.id.len(), 36, "Node ID should be a UUID");
            assert_eq!(node.id.chars().filter(|c| *c == '-').count(), 4);
            assert_eq!(node.content, seed.name);
            assert!(node.title.is_some());
            assert_eq!(node.title.as_deref().unwrap(), seed.name);

            let whitelist = extract_tool_whitelist(&node);
            assert_eq!(whitelist, seed.tool_whitelist);
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

    // --- Skill whitelist tests for all 8 skills ---

    #[test]
    fn research_search_skill_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.name == "Research & Search")
            .expect("Research & Search skill should exist");
        assert!(
            skill
                .tool_whitelist
                .contains(&"search_semantic".to_string())
                || skill.tool_whitelist.contains(&"search_nodes".to_string()),
            "Research & Search should whitelist search_semantic or search_nodes"
        );
    }

    #[test]
    fn node_creation_skill_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.name == "Node Creation")
            .expect("Node Creation skill should exist");
        assert!(
            skill.tool_whitelist.contains(&"create_node".to_string()),
            "Node Creation should whitelist create_node"
        );
    }

    #[test]
    fn schema_creation_skill_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.name == "Schema Creation")
            .expect("Schema Creation skill should exist");
        assert!(
            skill.tool_whitelist.contains(&"create_schema".to_string()),
            "Schema Creation should whitelist create_schema"
        );
    }

    #[test]
    fn graph_editing_skill_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.name == "Graph Editing")
            .expect("Graph Editing skill should exist");
        assert!(
            skill.tool_whitelist.contains(&"update_node".to_string()),
            "Graph Editing should whitelist update_node"
        );
    }

    #[test]
    fn relationship_management_skill_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.name == "Relationship Management")
            .expect("Relationship Management skill should exist");
        assert!(
            skill
                .tool_whitelist
                .contains(&"create_relationship".to_string()),
            "Relationship Management should whitelist create_relationship"
        );
    }

    #[test]
    fn node_deletion_skill_exists_and_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.name == "Node Deletion")
            .expect("Node Deletion skill should exist");
        assert!(
            skill.tool_whitelist.contains(&"delete_node".to_string()),
            "Node Deletion should whitelist delete_node"
        );
    }

    #[test]
    fn bulk_import_skill_exists_and_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.name == "Bulk Import")
            .expect("Bulk Import skill should exist");
        assert!(
            skill
                .tool_whitelist
                .contains(&"create_nodes_from_markdown".to_string()),
            "Bulk Import should whitelist create_nodes_from_markdown"
        );
    }

    #[test]
    fn organization_skill_exists_and_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.name == "Organization")
            .expect("Organization skill should exist");
        assert!(
            skill
                .tool_whitelist
                .contains(&"create_relationship".to_string()),
            "Organization should whitelist create_relationship"
        );
    }

    #[test]
    fn scope_tools_for_node_deletion_skill() {
        use serde_json::json;

        let pipeline = SkillPipeline::new(None);
        let all_tools = vec![
            ToolDefinition {
                name: "delete_node".into(),
                description: "Delete a node".into(),
                parameters_schema: json!({}),
            },
            ToolDefinition {
                name: "get_node".into(),
                description: "Get a node".into(),
                parameters_schema: json!({}),
            },
            ToolDefinition {
                name: "create_node".into(),
                description: "Create a node".into(),
                parameters_schema: json!({}),
            },
            ToolDefinition {
                name: "search_nodes".into(),
                description: "Search nodes".into(),
                parameters_schema: json!({}),
            },
        ];

        let seeds = SkillPipeline::seed_skill_nodes();
        let deletion_skill = seeds.iter().find(|s| s.name == "Node Deletion").unwrap();

        let skill_match = SkillMatch {
            skill: deletion_skill.to_node(),
            confidence: 0.9,
            intent: ExtractedIntent {
                query: "delete".to_string(),
                from_pattern: true,
            },
            tool_whitelist: deletion_skill.tool_whitelist.clone(),
            max_iterations: deletion_skill.max_iterations,
        };

        let scoped = pipeline.scope_tools(&all_tools, &skill_match);
        assert_eq!(scoped.len(), 2, "Node Deletion should scope to 2 tools");
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"delete_node"));
        assert!(names.contains(&"get_node"));
        assert!(!names.contains(&"create_node"));
        assert!(!names.contains(&"search_nodes"));
    }

    #[test]
    fn scope_tools_for_bulk_import_skill() {
        use serde_json::json;

        let pipeline = SkillPipeline::new(None);
        let all_tools = vec![
            ToolDefinition {
                name: "create_nodes_from_markdown".into(),
                description: "Bulk import".into(),
                parameters_schema: json!({}),
            },
            ToolDefinition {
                name: "create_node".into(),
                description: "Create a node".into(),
                parameters_schema: json!({}),
            },
        ];

        let seeds = SkillPipeline::seed_skill_nodes();
        let import_skill = seeds.iter().find(|s| s.name == "Bulk Import").unwrap();

        let skill_match = SkillMatch {
            skill: import_skill.to_node(),
            confidence: 0.9,
            intent: ExtractedIntent {
                query: "import".to_string(),
                from_pattern: true,
            },
            tool_whitelist: import_skill.tool_whitelist.clone(),
            max_iterations: import_skill.max_iterations,
        };

        let scoped = pipeline.scope_tools(&all_tools, &skill_match);
        assert_eq!(scoped.len(), 1, "Bulk Import should scope to 1 tool");
        assert_eq!(scoped[0].name, "create_nodes_from_markdown");
    }

    #[test]
    fn scope_tools_for_organization_skill() {
        use serde_json::json;

        let pipeline = SkillPipeline::new(None);
        let all_tools = vec![
            ToolDefinition {
                name: "create_relationship".into(),
                description: "Create relationship".into(),
                parameters_schema: json!({}),
            },
            ToolDefinition {
                name: "get_node".into(),
                description: "Get a node".into(),
                parameters_schema: json!({}),
            },
            ToolDefinition {
                name: "delete_node".into(),
                description: "Delete a node".into(),
                parameters_schema: json!({}),
            },
        ];

        let seeds = SkillPipeline::seed_skill_nodes();
        let org_skill = seeds.iter().find(|s| s.name == "Organization").unwrap();

        let skill_match = SkillMatch {
            skill: org_skill.to_node(),
            confidence: 0.9,
            intent: ExtractedIntent {
                query: "organize".to_string(),
                from_pattern: true,
            },
            tool_whitelist: org_skill.tool_whitelist.clone(),
            max_iterations: org_skill.max_iterations,
        };

        let scoped = pipeline.scope_tools(&all_tools, &skill_match);
        assert_eq!(scoped.len(), 2, "Organization should scope to 2 tools");
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"create_relationship"));
        assert!(names.contains(&"get_node"));
        assert!(!names.contains(&"delete_node"));
    }
}
