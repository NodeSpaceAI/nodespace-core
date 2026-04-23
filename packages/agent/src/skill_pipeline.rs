//! Pre-turn skill discovery pipeline.
//!
//! Runs before each agent turn to detect user intent and inject the most
//! relevant skill into the prompt context. Uses pattern matching + semantic
//! search (zero model inference cost).
//!
//! Issue #1050, ADR-030 Phase 3.

use std::sync::Arc;

use nodespace_core::mcp::handlers::markdown::NodeTemplate;
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

    /// Get default seed skill templates for first-run creation.
    ///
    /// Each [`NodeTemplate`] produces one skill root node plus any guidance
    /// prompt children defined in its markdown body.  Use
    /// [`nodespace_core::mcp::handlers::markdown::prepare_nodes_from_template`]
    /// to expand a template into a flat list of [`PreparedNode`]s before
    /// inserting them via `NodeService::bulk_create_hierarchy`.
    pub fn seed_skill_nodes() -> Vec<NodeTemplate> {
        vec![
            NodeTemplate {
                title: "Research & Search".to_string(),
                content: None,
                root_node_type: "skill".to_string(),
                root_properties: serde_json::json!({
                    "description": "Search and explore the knowledge graph to find relevant information, discover connections, and answer questions about stored knowledge.",
                    "tool_whitelist": ["search_semantic", "search_nodes", "get_node"],
                    "max_iterations": 4,

                }),
                child_node_type: Some("prompt".to_string()),
                child_properties: None,
                markdown_content: r#"# Research & Search Guidance

When answering questions about stored knowledge:

SEARCH FIRST: Always call search_semantic with a natural language query. Results are ordered by relevance — the first result is the best match.

RESULT STRUCTURE: Each result contains:
- id: node ID (use this for follow-up get_node calls)
- title: document title
- score: similarity score (0-1, higher = more relevant)
- snippet: short content preview
- markdown: full document content (present for top N results based on include_markdown, default 1)

USE MARKDOWN DIRECTLY: If the top result has a non-empty 'markdown' field, that is the complete document. Summarize or answer from it immediately — do NOT call get_node or search_nodes again.

FETCH ADDITIONAL CONTENT: Only call get_node with format=markdown if you need full content for a lower-ranked result that did not include markdown.

PARAMETER GUIDANCE:
- Use 'collection' to narrow search to a namespace/folder (e.g. collection="Architecture").
- Use 'node_types' to filter by type (e.g. node_types=["task"]) — prefer over 'collection' for type-based filtering.
- Use 'scope'="conversations" when the user asks about past chats or conversation history.
- Use 'threshold' to tune precision: default 0.3. Lower to 0.1-0.2 for broader recall when results are sparse.
- Use 'include_archived'=true only when the user explicitly asks for archived or historical content.
- Use 'exclude_collections' to suppress noisy collections (e.g. exclude_collections=["Archived"]).
- Use 'include_edges'=true to get relationship data (outgoing 'mentions' edges) with each result — saves a separate get_related_nodes call.
- Use 'graph_boost'=true to rank well-connected nodes higher (blends similarity with graph connectivity). Useful when the user wants the most referenced/central node on a topic.
- Use 'property_filters' for simple key-value filtering (e.g. property_filters={"status": "done"}). Prefer 'node_types' for type filtering.

MULTIPLE DOCUMENTS: If the user asks about multiple topics, call search_semantic once per topic rather than searching broadly and fetching each result individually.

LISTING BY TYPE OR PROPERTY: To list all nodes of a type or filtered by a property, use search_nodes (not search_semantic). Pass query="" to skip the title filter. Examples:
- "find all my open tasks" → search_nodes(query="", node_type="task", filters={"status": "open"})
- "list all customers" → search_nodes(query="", node_type="<customer-schema-id>")
- "find tasks for Acme" → search_nodes(query="", node_type="task", filters={"company": "Acme"})"#.to_string(),
            },
            NodeTemplate {
                title: "Node Creation".to_string(),
                content: None,
                root_node_type: "skill".to_string(),
                root_properties: serde_json::json!({
                    "description": "Create new nodes, records, entries, or instances of any type — tasks, text notes, or custom types like Project, Customer, Invoice. Use when user wants to add, create, or insert a new item, record, entry, or example of an existing type.",
                    "tool_whitelist": ["create_node", "get_node"],
                    "max_iterations": 3,

                }),
                child_node_type: Some("prompt".to_string()),
                child_properties: None,
                markdown_content: String::new(),
            },
            NodeTemplate {
                title: "Schema Creation".to_string(),
                content: None,
                root_node_type: "skill".to_string(),
                root_properties: serde_json::json!({
                    "description": "Define a new entity type or schema with custom fields, enums, and relationships. Use when user says 'new type', 'node type', 'define fields', 'create schema', or wants to design a new kind of entity like Project, Customer, or Invoice.",
                    "tool_whitelist": ["create_schema", "get_node"],
                    "max_iterations": 3,

                }),
                child_node_type: Some("prompt".to_string()),
                child_properties: None,
                markdown_content: r#"# Schema Creation Guidance

When creating a schema:

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
            },
            NodeTemplate {
                title: "Graph Editing".to_string(),
                content: None,
                root_node_type: "skill".to_string(),
                root_properties: serde_json::json!({
                    "description": "Modify existing nodes in the knowledge graph - update content, properties, titles, and metadata. For tasks, use update_task_status to change status.",
                    "tool_whitelist": ["update_node", "update_task_status", "get_node", "search_nodes"],
                    "max_iterations": 3,

                }),
                child_node_type: Some("prompt".to_string()),
                child_properties: None,
                markdown_content: String::new(),
            },
            NodeTemplate {
                title: "Relationship Management".to_string(),
                content: None,
                root_node_type: "skill".to_string(),
                root_properties: serde_json::json!({
                    "description": "Create connections between nodes, explore relationships, and traverse the knowledge graph.",
                    "tool_whitelist": ["create_relationship", "get_related_nodes", "get_node"],
                    "max_iterations": 3,

                }),
                child_node_type: Some("prompt".to_string()),
                child_properties: None,
                markdown_content: String::new(),
            },
            NodeTemplate {
                title: "Node Deletion".to_string(),
                content: None,
                root_node_type: "skill".to_string(),
                root_properties: serde_json::json!({
                    "description": "Delete nodes from the knowledge graph. Use when user wants to remove, delete, or trash a node or record.",
                    "tool_whitelist": ["delete_node", "get_node"],
                    "max_iterations": 3,

                }),
                child_node_type: Some("prompt".to_string()),
                child_properties: None,
                markdown_content: String::new(),
            },
            NodeTemplate {
                title: "Bulk Import".to_string(),
                content: None,
                root_node_type: "skill".to_string(),
                root_properties: serde_json::json!({
                    "description": "Import documents and create node hierarchies from markdown. Use when user wants to import, bulk create, or create nodes from a markdown document.",
                    "tool_whitelist": ["create_nodes_from_markdown"],
                    "max_iterations": 2,

                }),
                child_node_type: Some("prompt".to_string()),
                child_properties: None,
                markdown_content: String::new(),
            },
            NodeTemplate {
                title: "Organization".to_string(),
                content: None,
                root_node_type: "skill".to_string(),
                root_properties: serde_json::json!({
                    "description": "Organize nodes into collections and categories. Use when user wants to add to a collection, categorize, or group nodes.",
                    "tool_whitelist": ["create_relationship", "get_node"],
                    "max_iterations": 3,

                }),
                child_node_type: Some("prompt".to_string()),
                child_properties: None,
                markdown_content: String::new(),
            },
        ]
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use nodespace_core::mcp::handlers::markdown::prepare_nodes_from_template;

    use super::*;

    /// Extract tool_whitelist from a skill NodeTemplate's root_properties.
    fn tmpl_tool_whitelist(tmpl: &NodeTemplate) -> Vec<String> {
        tmpl.root_properties
            .get("tool_whitelist")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Extract max_iterations from a skill NodeTemplate's root_properties.
    fn tmpl_max_iterations(tmpl: &NodeTemplate) -> usize {
        tmpl.root_properties
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize
    }

    #[test]
    fn seed_skills_have_valid_properties() {
        let seeds = SkillPipeline::seed_skill_nodes();
        assert_eq!(seeds.len(), 8, "Should have 8 seed skills");

        for seed in &seeds {
            assert!(!seed.title.is_empty());
            assert!(
                seed.root_properties
                    .get("description")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false),
                "Skill '{}' must have a non-empty description",
                seed.title
            );
            assert!(
                !tmpl_tool_whitelist(seed).is_empty(),
                "Skill '{}' must have tools",
                seed.title
            );
            assert!(
                tmpl_max_iterations(seed) > 0,
                "Skill '{}' must have max_iterations > 0",
                seed.title
            );
        }
    }

    #[test]
    fn schema_creation_skill_uses_dedicated_tool() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let schema_skill = seeds
            .iter()
            .find(|s| s.title == "Schema Creation")
            .expect("Schema Creation skill should exist");

        assert!(
            tmpl_tool_whitelist(schema_skill).contains(&"create_schema".to_string()),
            "Schema Creation skill should whitelist create_schema"
        );
    }

    #[test]
    fn graph_editing_skill_includes_task_tool() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let editing_skill = seeds
            .iter()
            .find(|s| s.title == "Graph Editing")
            .expect("Graph Editing skill should exist");

        assert!(
            tmpl_tool_whitelist(editing_skill).contains(&"update_task_status".to_string()),
            "Graph Editing skill should whitelist update_task_status"
        );
    }

    #[test]
    fn seed_skill_template_produces_skill_node() {
        let seeds = SkillPipeline::seed_skill_nodes();
        for seed in &seeds {
            let nodes = prepare_nodes_from_template(seed)
                .unwrap_or_else(|e| panic!("Template '{}' failed: {:?}", seed.title, e));
            assert!(
                !nodes.is_empty(),
                "Template '{}' produced no nodes",
                seed.title
            );
            let root = &nodes[0];
            assert_eq!(root.node_type, "skill");
            assert_eq!(root.id.len(), 36, "Node ID should be a UUID");
            assert_eq!(root.id.chars().filter(|c| *c == '-').count(), 4);
            assert_eq!(root.content, seed.title);
            // root_properties are stored in the PreparedNode's properties
            let whitelist = root
                .properties
                .get("tool_whitelist")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            assert_eq!(whitelist, tmpl_tool_whitelist(seed));
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
            .find(|s| s.title == "Research & Search")
            .expect("Research & Search skill should exist");
        let wl = tmpl_tool_whitelist(skill);
        assert!(
            wl.contains(&"search_semantic".to_string()) || wl.contains(&"search_nodes".to_string()),
            "Research & Search should whitelist search_semantic or search_nodes"
        );
    }

    #[test]
    fn node_creation_skill_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.title == "Node Creation")
            .expect("Node Creation skill should exist");
        assert!(
            tmpl_tool_whitelist(skill).contains(&"create_node".to_string()),
            "Node Creation should whitelist create_node"
        );
    }

    #[test]
    fn schema_creation_skill_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.title == "Schema Creation")
            .expect("Schema Creation skill should exist");
        assert!(
            tmpl_tool_whitelist(skill).contains(&"create_schema".to_string()),
            "Schema Creation should whitelist create_schema"
        );
    }

    #[test]
    fn graph_editing_skill_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.title == "Graph Editing")
            .expect("Graph Editing skill should exist");
        assert!(
            tmpl_tool_whitelist(skill).contains(&"update_node".to_string()),
            "Graph Editing should whitelist update_node"
        );
    }

    #[test]
    fn relationship_management_skill_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.title == "Relationship Management")
            .expect("Relationship Management skill should exist");
        assert!(
            tmpl_tool_whitelist(skill).contains(&"create_relationship".to_string()),
            "Relationship Management should whitelist create_relationship"
        );
    }

    #[test]
    fn node_deletion_skill_exists_and_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.title == "Node Deletion")
            .expect("Node Deletion skill should exist");
        assert!(
            tmpl_tool_whitelist(skill).contains(&"delete_node".to_string()),
            "Node Deletion should whitelist delete_node"
        );
    }

    #[test]
    fn bulk_import_skill_exists_and_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.title == "Bulk Import")
            .expect("Bulk Import skill should exist");
        assert!(
            tmpl_tool_whitelist(skill).contains(&"create_nodes_from_markdown".to_string()),
            "Bulk Import should whitelist create_nodes_from_markdown"
        );
    }

    #[test]
    fn organization_skill_exists_and_whitelist() {
        let seeds = SkillPipeline::seed_skill_nodes();
        let skill = seeds
            .iter()
            .find(|s| s.title == "Organization")
            .expect("Organization skill should exist");
        assert!(
            tmpl_tool_whitelist(skill).contains(&"create_relationship".to_string()),
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
        let deletion_tmpl = seeds.iter().find(|s| s.title == "Node Deletion").unwrap();
        let nodes = prepare_nodes_from_template(deletion_tmpl).unwrap();
        let tool_whitelist = tmpl_tool_whitelist(deletion_tmpl);

        let skill_match = SkillMatch {
            skill: Node::new(
                nodes[0].node_type.clone(),
                nodes[0].content.clone(),
                nodes[0].properties.clone(),
            ),
            confidence: 0.9,
            intent: ExtractedIntent {
                query: "delete".to_string(),
                from_pattern: true,
            },
            tool_whitelist,
            max_iterations: tmpl_max_iterations(deletion_tmpl),
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
        let import_tmpl = seeds.iter().find(|s| s.title == "Bulk Import").unwrap();
        let nodes = prepare_nodes_from_template(import_tmpl).unwrap();
        let tool_whitelist = tmpl_tool_whitelist(import_tmpl);

        let skill_match = SkillMatch {
            skill: Node::new(
                nodes[0].node_type.clone(),
                nodes[0].content.clone(),
                nodes[0].properties.clone(),
            ),
            confidence: 0.9,
            intent: ExtractedIntent {
                query: "import".to_string(),
                from_pattern: true,
            },
            tool_whitelist,
            max_iterations: tmpl_max_iterations(import_tmpl),
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
        let org_tmpl = seeds.iter().find(|s| s.title == "Organization").unwrap();
        let nodes = prepare_nodes_from_template(org_tmpl).unwrap();
        let tool_whitelist = tmpl_tool_whitelist(org_tmpl);

        let skill_match = SkillMatch {
            skill: Node::new(
                nodes[0].node_type.clone(),
                nodes[0].content.clone(),
                nodes[0].properties.clone(),
            ),
            confidence: 0.9,
            intent: ExtractedIntent {
                query: "organize".to_string(),
                from_pattern: true,
            },
            tool_whitelist,
            max_iterations: tmpl_max_iterations(org_tmpl),
        };

        let scoped = pipeline.scope_tools(&all_tools, &skill_match);
        assert_eq!(scoped.len(), 2, "Organization should scope to 2 tools");
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"create_relationship"));
        assert!(names.contains(&"get_node"));
        assert!(!names.contains(&"delete_node"));
    }
}
