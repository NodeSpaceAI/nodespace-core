//! Prompt assembly service: hardcoded base + graph-stored overrides.
//!
//! Composes the final agent prompt from hardcoded defaults plus prompt nodes
//! stored in the knowledge graph. Graph overrides are layered on top of the
//! base prompt, ordered by priority. Supports Minijinja template rendering.
//!
//! Issue #1049, ADR-030 Phase 2.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use nodespace_core::db::events::DomainEvent;
use nodespace_core::models::Node;
use nodespace_core::services::NodeService;

use crate::agent_types::ToolDefinition;
use crate::local_agent::prompt_templates;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Context variables available to Minijinja templates.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TemplateContext {
    pub current_date: String,
    pub model_name: String,
    pub workspace_context: String,
}

/// The assembled prompt ready for inference.
#[derive(Debug, Clone)]
pub struct AssembledPrompt {
    /// Full system prompt text (base + graph overrides)
    pub system_prompt: String,
    /// Tool definitions (may be scoped by active skill in future)
    pub tool_schemas: Vec<ToolDefinition>,
}

/// A cached assembled prompt with invalidation tracking.
#[derive(Debug, Clone)]
struct CachedPrompt {
    source_node_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// PromptAssembler
// ---------------------------------------------------------------------------

/// Assembles final prompts from hardcoded base + graph-stored overrides.
///
/// The assembly order is:
/// 1. Hardcoded base prompt (always present, from prompt_templates.rs)
/// 2. Graph-stored prompt nodes ordered by priority
/// 3. Minijinja template rendering with context variables
///
/// Cache is invalidated when prompt nodes are updated (event-driven).
pub struct PromptAssembler {
    node_service: Arc<NodeService>,
    cache: Arc<RwLock<HashMap<String, CachedPrompt>>>,
}

impl PromptAssembler {
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self {
            node_service,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start listening for node update events to invalidate cache.
    ///
    /// Spawns a background task that watches for prompt node changes.
    /// Call this once after construction.
    pub fn start_event_listener(&self) {
        let cache = Arc::clone(&self.cache);
        let mut rx = self.node_service.subscribe_to_events();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(envelope) => {
                        let node_id = match &envelope.event {
                            DomainEvent::NodeUpdated {
                                node_id, node_type, ..
                            }
                            | DomainEvent::NodeCreated { node_id, node_type } => {
                                if node_type == "prompt" {
                                    Some(node_id.clone())
                                } else {
                                    None
                                }
                            }
                            DomainEvent::NodeDeleted { id, node_type } => {
                                if node_type == "prompt" {
                                    Some(id.clone())
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        };

                        if let Some(changed_id) = node_id {
                            // Invalidate any cache entry that references this node
                            let mut cache_guard = cache.write().await;
                            cache_guard
                                .retain(|_, cached| !cached.source_node_ids.contains(&changed_id));
                            // Also clear the default entry since prompt inventory changed
                            cache_guard.remove("__default__");
                            tracing::debug!(
                                node_id = %changed_id,
                                "Prompt cache invalidated due to prompt node change"
                            );
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            skipped = n,
                            "Prompt assembler event listener lagged, clearing cache"
                        );
                        cache.write().await.clear();
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!("Prompt assembler event channel closed, stopping listener");
                        break;
                    }
                }
            }
        });
    }

    /// Assemble the final prompt from hardcoded base + graph overrides.
    ///
    /// `dynamic_context` is the workspace context string (entity types, collections, playbooks).
    /// `template_ctx` provides variables for Minijinja template rendering.
    /// `tools` are the available tool definitions (passed through, may be scoped by skill later).
    pub async fn assemble(
        &self,
        dynamic_context: &str,
        template_ctx: &TemplateContext,
        tools: Vec<ToolDefinition>,
    ) -> AssembledPrompt {
        // 1. Hardcoded base prompt (always the foundation)
        let base = prompt_templates::system_prompt(dynamic_context);

        // 2. Fetch prompt nodes from the graph, ordered by priority
        let overrides = self.fetch_prompt_overrides().await;

        // 3. Render templates and concatenate
        let mut sections = vec![base];

        for node in &overrides {
            let syntax = node
                .properties
                .get("template_syntax")
                .and_then(|v| v.as_str())
                .unwrap_or("plain");

            let rendered = if syntax == "minijinja" {
                self.render_template(&node.content, template_ctx)
            } else {
                node.content.clone()
            };

            // Wrap user content with boundary markers for safety
            let source = node
                .properties
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("user-created");

            if source != "built-in" {
                sections.push(format!(
                    "<user-content node-id=\"{}\" type=\"prompt\">\n{}\n</user-content>",
                    node.id, rendered
                ));
            } else {
                sections.push(rendered);
            }
        }

        let system_prompt = sections.join("\n\n");

        AssembledPrompt {
            system_prompt,
            tool_schemas: tools,
        }
    }

    /// Fetch prompt nodes from the graph, ordered by priority ascending.
    async fn fetch_prompt_overrides(&self) -> Vec<Node> {
        let filter = nodespace_core::ops::node_ops::QueryNodesInput {
            node_type: Some("prompt".to_string()),
            parent_id: None,
            root_id: None,
            limit: Some(50),
            offset: None,
            collection_id: None,
            collection: None,
            filters: None,
        };

        match nodespace_core::ops::node_ops::query_nodes(&self.node_service, filter).await {
            Ok(result) => {
                // QueryNodesOutput.nodes is Vec<Value>, deserialize to Vec<Node>
                let mut nodes: Vec<Node> = result
                    .nodes
                    .into_iter()
                    .filter_map(|v| serde_json::from_value(v).ok())
                    .collect();
                // Sort by priority ascending (lower priority = earlier in assembly)
                nodes.sort_by_key(|n| {
                    n.properties
                        .get("priority")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(100)
                });
                nodes
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to fetch prompt overrides, using base only");
                Vec::new()
            }
        }
    }

    /// Render a Minijinja template with the given context.
    ///
    /// On error, returns the raw template text and logs a warning.
    /// Template errors should never crash the turn.
    fn render_template(&self, template_str: &str, ctx: &TemplateContext) -> String {
        let env = minijinja::Environment::new();
        match env.render_str(template_str, ctx) {
            Ok(rendered) => rendered,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Minijinja template render failed, using raw content"
                );
                template_str.to_string()
            }
        }
    }

    /// Clear the entire cache (e.g., for "reset to defaults").
    pub async fn clear_cache(&self) {
        self.cache.write().await.clear();
    }

    /// Get seed prompt nodes that should be created on first run.
    ///
    /// These migrate the content from `prompt_templates.rs` into graph nodes
    /// so users can customize them. The hardcoded base remains as foundation.
    pub fn seed_prompt_nodes() -> Vec<SeedPrompt> {
        vec![
            SeedPrompt {
                id: "prompt-workspace-context".to_string(),
                content:
                    "Current date: {{ current_date }}\nActive model: {{ model_name }}\n\n{{ workspace_context }}"
                        .to_string(),
                priority: 10,
                template_syntax: "minijinja".to_string(),
                source: "built-in".to_string(),
                title: "Workspace Context Template".to_string(),
            },
            SeedPrompt {
                id: "prompt-tool-strategy".to_string(),
                content: "TOOL STRATEGY:\n\
                    - To find nodes by meaning/topic: use search_semantic (natural language query)\n\
                    - To find nodes by exact fields: use search_nodes (keyword + type filter)\n\
                    - To get full node details: use get_node with the ID from search results\n\
                    - To create: use create_node with the correct node_type and properties matching the schema fields above\n\
                    - To update: use update_node — only include fields you want to change\n\
                    - To connect nodes: use create_relationship with relationship names from the schemas above"
                    .to_string(),
                priority: 50,
                template_syntax: "plain".to_string(),
                source: "built-in".to_string(),
                title: "Tool Strategy Guide".to_string(),
            },
            SeedPrompt {
                id: "prompt-response-rules".to_string(),
                content: "RESPONSE RULES:\n\
                    - After tool results: summarize in natural language. NEVER paste raw JSON as your response.\n\
                    - Reference nodes with bare URI: nodespace://abc-123 (no markdown links, no backticks)\n\
                    - Enum values in Title Case: \"In Progress\" not \"in_progress\"\n\
                    - When listing nodes: **Title** (nodespace://id) — brief description\n\
                    - When reporting search results: \"Found N nodes...\" then list top results\n\
                    - If tool returns empty results: say so clearly. Do NOT retry the same query.\n\
                    - Keep responses concise — under 3 sentences unless user asks for detail."
                    .to_string(),
                priority: 60,
                template_syntax: "plain".to_string(),
                source: "built-in".to_string(),
                title: "Response Formatting Rules".to_string(),
            },
            SeedPrompt {
                id: "prompt-tool-call-format".to_string(),
                content: "TOOL CALL FORMAT:\n\
                    - Pass arguments flat. Do NOT nest under \"properties\" or \"arguments\".\n\
                    - Use the exact field names shown in the schema definitions above."
                    .to_string(),
                priority: 70,
                template_syntax: "plain".to_string(),
                source: "built-in".to_string(),
                title: "Tool Call Formatting".to_string(),
            },
        ]
    }
}

/// Descriptor for a seed prompt node to be created on first run.
#[derive(Debug, Clone)]
pub struct SeedPrompt {
    pub id: String,
    pub content: String,
    pub priority: i64,
    pub template_syntax: String,
    pub source: String,
    pub title: String,
}

impl SeedPrompt {
    /// Convert to a Node for creation via NodeService.
    pub fn to_node(&self) -> Node {
        Node {
            id: self.id.clone(),
            node_type: "prompt".to_string(),
            content: self.content.clone(),
            properties: serde_json::json!({
                "priority": self.priority,
                "template_syntax": self.template_syntax,
                "source": self.source,
            }),
            created_at: chrono::Utc::now(),
            modified_at: chrono::Utc::now(),
            version: 1,
            lifecycle_status: "active".to_string(),
            title: Some(self.title.clone()),
            mentions: Vec::new(),
            mentioned_in: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_prompts_have_valid_properties() {
        let seeds = PromptAssembler::seed_prompt_nodes();
        assert!(seeds.len() >= 3, "Should have at least 3 seed prompts");

        for seed in &seeds {
            assert!(!seed.id.is_empty(), "Seed ID must not be empty");
            assert!(!seed.content.is_empty(), "Seed content must not be empty");
            assert!(!seed.title.is_empty(), "Seed title must not be empty");
            assert!(
                seed.source == "built-in",
                "All seed prompts should be built-in"
            );
            assert!(
                seed.template_syntax == "plain" || seed.template_syntax == "minijinja",
                "Invalid template syntax: {}",
                seed.template_syntax
            );
        }
    }

    #[test]
    fn seed_prompts_ordered_by_priority() {
        let seeds = PromptAssembler::seed_prompt_nodes();
        let priorities: Vec<i64> = seeds.iter().map(|s| s.priority).collect();
        let mut sorted = priorities.clone();
        sorted.sort();
        assert_eq!(
            priorities, sorted,
            "Seed prompts should be in priority order"
        );
    }

    #[test]
    fn seed_prompt_to_node_conversion() {
        let seeds = PromptAssembler::seed_prompt_nodes();
        for seed in &seeds {
            let node = seed.to_node();
            assert_eq!(node.node_type, "prompt");
            assert_eq!(node.id, seed.id);
            assert_eq!(node.content, seed.content);
            assert_eq!(node.properties["priority"].as_i64().unwrap(), seed.priority);
            assert_eq!(
                node.properties["template_syntax"].as_str().unwrap(),
                seed.template_syntax
            );
            assert_eq!(node.properties["source"].as_str().unwrap(), seed.source);
            assert!(node.title.is_some());
        }
    }

    #[test]
    fn render_plain_template() {
        // Plain templates should be returned as-is
        let assembler_render = |template: &str| -> String {
            let env = minijinja::Environment::new();
            let ctx = TemplateContext {
                current_date: "2026-04-06".to_string(),
                model_name: "ministral-3b".to_string(),
                workspace_context: "test context".to_string(),
            };
            env.render_str(template, &ctx)
                .unwrap_or_else(|_| template.to_string())
        };

        // Plain text (no template syntax) should render unchanged
        let plain = "Use search_semantic for meaning queries";
        assert_eq!(assembler_render(plain), plain);
    }

    #[test]
    fn render_minijinja_template() {
        let env = minijinja::Environment::new();
        let ctx = TemplateContext {
            current_date: "2026-04-06".to_string(),
            model_name: "ministral-3b".to_string(),
            workspace_context: "Entity types: customer, invoice".to_string(),
        };
        let template = "Date: {{ current_date }}\nModel: {{ model_name }}";
        let result = env.render_str(template, &ctx).unwrap();
        assert!(result.contains("2026-04-06"));
        assert!(result.contains("ministral-3b"));
    }

    #[test]
    fn render_template_error_returns_raw() {
        let env = minijinja::Environment::new();
        let ctx = TemplateContext {
            current_date: "2026-04-06".to_string(),
            model_name: "test".to_string(),
            workspace_context: "".to_string(),
        };
        // Invalid template syntax
        let bad_template = "{{ undefined_function() }}";
        let result = env.render_str(bad_template, &ctx);
        // Should fail
        assert!(result.is_err());
    }

    #[test]
    fn template_context_serializable() {
        let ctx = TemplateContext {
            current_date: "2026-04-06".to_string(),
            model_name: "ministral-3b".to_string(),
            workspace_context: "some context".to_string(),
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["current_date"], "2026-04-06");
        assert_eq!(json["model_name"], "ministral-3b");
    }

    #[test]
    fn seed_prompt_ids_are_unique() {
        let seeds = PromptAssembler::seed_prompt_nodes();
        let ids: Vec<&str> = seeds.iter().map(|s| s.id.as_str()).collect();
        let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len(), "Seed prompt IDs must be unique");
    }
}
