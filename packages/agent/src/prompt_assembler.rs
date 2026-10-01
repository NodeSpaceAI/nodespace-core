//! Prompt assembly service: graph-only prompt composition.
//!
//! Composes the final agent prompt exclusively from `agent-guidance` nodes
//! stored in the knowledge graph, assembled in natural child order. Supports
//! Minijinja template rendering. If no `agent-guidance` nodes are found
//! (corrupted/empty database), falls back to a minimal emergency prompt and
//! logs a warning.
//!
//! ADR-030 Phase 2; `agent-guidance` node type per ADR-057.

use std::sync::Arc;

use nodespace_core::markdown::{NodeTemplate, SeedTier};
use nodespace_core::models::Node;
use nodespace_core::services::{render_subtree_markdown, NodeService};

use crate::agent_guidance::{NODE_REFERENCE_FORMAT, SCHEMA_CREATION_RULES, TOOL_STRATEGY_RULES};
use crate::agent_types::ToolDefinition;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Context variables available to Minijinja templates.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TemplateContext {
    pub current_date: String,
    pub model_name: String,
    pub workspace_context: String,
    /// The local user, or `None` while no name or email has been set — a
    /// template tests it to leave the identity line out entirely.
    pub current_user: Option<CurrentUser>,
}

/// The person "me", "my" and "I" refer to: the database's local person node.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CurrentUser {
    /// The person node's id, so the agent can read, filter by or relate to
    /// the user without searching for them first.
    pub id: String,
    /// The person's computed title; empty when only an email is set.
    pub name: String,
    /// Empty when only a name is set.
    pub email: String,
}

/// The assembled prompt ready for inference.
#[derive(Debug, Clone)]
pub struct AssembledPrompt {
    /// Full system prompt text (base + graph overrides)
    pub system_prompt: String,
    /// Tool definitions (may be scoped by active skill in future)
    pub tool_schemas: Vec<ToolDefinition>,
}

// ---------------------------------------------------------------------------
// PromptAssembler
// ---------------------------------------------------------------------------

/// Maximum number of agent-guidance nodes to fetch from the graph.
const MAX_PROMPT_NODES: usize = 50;

/// Minimal emergency fallback when no agent-guidance nodes exist in the graph.
/// This should only fire on corrupted/empty databases — normal operation
/// reads all prompt content from graph nodes seeded on first run. Also used
/// by the agent loop when no `PromptAssembler` is wired (only the daemon's
/// no-op/idle state, which never runs inference).
pub(crate) const EMERGENCY_FALLBACK_PROMPT: &str = "\
You are NodeSpace's built-in assistant. You help users work with their \
knowledge graph — creating, finding, updating, and connecting nodes.\n\n\
Use the available tools to accomplish tasks. Summarize results in natural language.";

/// Assembles final prompts exclusively from graph-stored `agent-guidance` nodes.
///
/// The assembly order is:
/// 1. Fetch root agent-guidance nodes from the graph
/// 2. For each agent-guidance node, fetch children in natural child order and concatenate
/// 3. Render through Minijinja with context variables
/// 4. If no agent-guidance nodes found, use emergency fallback and log a warning
pub struct PromptAssembler {
    node_service: Arc<NodeService>,
}

impl PromptAssembler {
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self { node_service }
    }

    /// Assemble the final prompt from graph-stored `agent-guidance` nodes only.
    ///
    /// `template_ctx` provides variables for Minijinja template rendering, including
    /// `workspace_context` (entity types, collections, playbooks).
    /// `tools` are the available tool definitions (passed through, may be scoped by skill later).
    pub async fn assemble(
        &self,
        template_ctx: &TemplateContext,
        tools: Vec<ToolDefinition>,
    ) -> AssembledPrompt {
        // 1. Fetch root agent-guidance nodes from the graph
        let prompt_nodes = self.fetch_prompt_overrides().await;

        // 2. If no agent-guidance nodes found, use emergency fallback
        if prompt_nodes.is_empty() {
            tracing::warn!(
                "No agent-guidance nodes found in graph — using emergency fallback. \
                 Seed agent-guidance nodes on first run to restore full functionality."
            );
            return AssembledPrompt {
                system_prompt: EMERGENCY_FALLBACK_PROMPT.to_string(),
                tool_schemas: tools,
            };
        }

        // 3. Fetch children for each agent-guidance node, render through minijinja, and concatenate
        let mut sections = Vec::new();

        for node in &prompt_nodes {
            // Fetch children and concatenate their content as the prompt body
            let body = self.fetch_prompt_body(node).await;
            if body.trim().is_empty() {
                continue;
            }
            let rendered = Self::render_template(&body, template_ctx);
            sections.push(rendered);
        }

        let system_prompt = sections.join("\n\n");

        AssembledPrompt {
            system_prompt,
            tool_schemas: tools,
        }
    }

    /// Assemble one turn's prompt: build the [`TemplateContext`] from the
    /// turn's inputs, then [`Self::assemble`].
    ///
    /// The current user is resolved here, inside the call the agent loop
    /// makes on every turn, rather than passed in: nothing upstream holds an
    /// identity that could go stale, so one edited mid-session applies to the
    /// next turn.
    pub async fn assemble_turn(
        &self,
        current_date: &str,
        model_name: &str,
        workspace_context: &str,
        tools: Vec<ToolDefinition>,
    ) -> AssembledPrompt {
        let template_ctx = TemplateContext {
            current_date: current_date.to_string(),
            model_name: model_name.to_string(),
            workspace_context: workspace_context.to_string(),
            current_user: self.current_user().await,
        };
        self.assemble(&template_ctx, tools).await
    }

    /// Resolve the local user for [`TemplateContext::current_user`].
    ///
    /// Reads the local person node on every call rather than caching it.
    /// `None` when neither a name nor an email is set, and on a lookup
    /// failure: the turn runs without the identity line rather than failing.
    ///
    /// Both values are collapsed to single-spaced text. The template gives
    /// the identity one line, and a stored value holding a newline would
    /// otherwise start a line of its own in the system prompt.
    async fn current_user(&self) -> Option<CurrentUser> {
        let person = match self.node_service.get_local_person().await {
            Ok(person) => person?,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to resolve the local person, omitting the current user from the prompt");
                return None;
            }
        };
        let single_line = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let name = single_line(person.title.as_deref().unwrap_or(""));
        let email = single_line(
            person
                .properties
                .get("person")
                .and_then(|p| p.get("email"))
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
        if name.is_empty() && email.is_empty() {
            return None;
        }
        Some(CurrentUser {
            id: person.id,
            name,
            email,
        })
    }

    /// Fetch root-level `agent-guidance` nodes from the graph (no parent).
    async fn fetch_prompt_overrides(&self) -> Vec<Node> {
        let filter = nodespace_core::ops::node_ops::QueryNodesInput {
            node_type: Some("agent-guidance".to_string()),
            limit: Some(MAX_PROMPT_NODES),
            offset: None,
            collection_id: None,
            collection: None,
            filters: None,
        };

        let all_nodes: Vec<Node> = match nodespace_core::ops::node_ops::query_nodes(
            &self.node_service,
            filter,
        )
        .await
        {
            Ok(result) => result
                .nodes
                .into_iter()
                .filter_map(|v| match serde_json::from_value(v) {
                    Ok(node) => Some(node),
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to deserialize agent-guidance node, skipping");
                        None
                    }
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to fetch agent-guidance overrides, using base only");
                return Vec::new();
            }
        };

        // Keep only true root nodes (no parent edge pointing to them).
        // query_nodes ignores parent_id filter, so all agent-guidance nodes are
        // returned; we must post-filter to avoid treating mid-hierarchy nodes as roots.
        let mut roots = Vec::new();
        for node in all_nodes {
            match self.node_service.get_parent(&node.id).await {
                Ok(None) => roots.push(node),
                Ok(Some(_)) => {} // has a parent — skip
                Err(e) => {
                    tracing::warn!(error = %e, node_id = %node.id, "Failed to check parent, skipping node");
                }
            }
        }
        roots
    }

    /// Fetch the full descendant subtree of an agent-guidance node and
    /// render it back to markdown as the body, in natural document
    /// (depth-first, fractional-order) order, with bullet markers restored.
    ///
    /// This walks the **entire** subtree, not just the node's direct
    /// children. Seeded guidance bodies that contain a `HEADER:` line followed
    /// by indented bullets (e.g. the Tool Strategy Guide's `TOOL STRATEGY:`
    /// block) parse into a nested tree: the header line is a direct child of
    /// the root node and the bullets are children of that header line. A
    /// direct-children-only flatten silently dropped the bullet body (issue:
    /// seed prompt body dropped). Walking the subtree preserves the full
    /// intended guidance.
    async fn fetch_prompt_body(&self, node: &Node) -> String {
        let (_root, node_map, adjacency_list) = match self
            .node_service
            .get_subtree_data(&node.id)
            .await
        {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, node_id = %node.id, "Failed to fetch agent-guidance subtree");
                return String::new();
            }
        };

        // Depth-first pre-order traversal starting from the root node's
        // children, following adjacency_list which is already sorted by
        // fractional order. The root node itself is excluded (its content is
        // the short title/label, not the guidance body).
        render_subtree_markdown(&node.id, &node_map, &adjacency_list)
    }

    /// Render a Minijinja template with the given context.
    ///
    /// On error, returns the raw template text and logs a warning.
    /// Template errors should never crash the turn.
    ///
    /// Note: auto-escaping is intentionally disabled (minijinja default) because
    /// output goes into a system prompt, not HTML. Do not enable HTML escaping.
    fn render_template(template_str: &str, ctx: &TemplateContext) -> String {
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

    /// Assemble the base system prompt from seed nodes without a database.
    ///
    /// Uses `markdown_content` as the prompt body for each seed, rendered
    /// through Minijinja. Intended for use in unit/integration tests where
    /// no DB is available.
    pub fn assemble_static(workspace_context: &str, current_date: Option<&str>) -> String {
        let seeds = Self::seed_agent_guidance_nodes();

        let ctx = TemplateContext {
            current_date: current_date.unwrap_or("2025-01-01").to_string(),
            model_name: "test".to_string(),
            workspace_context: workspace_context.to_string(),
            current_user: None,
        };

        let sections: Vec<String> = seeds
            .iter()
            .filter_map(|s| {
                // Body is the markdown_content (child content)
                let body = &s.markdown_content;
                if body.trim().is_empty() {
                    return None;
                }
                Some(Self::render_template(body, &ctx))
            })
            .collect();

        sections.join("\n\n")
    }

    /// Get seed agent-guidance templates for first-run creation.
    ///
    /// Each [`NodeTemplate`] produces an `agent-guidance` root node with text
    /// child nodes for body content. All base-prompt content lives in these
    /// graph nodes — there is no hardcoded base prompt. Users can customise
    /// any seed by editing the graph node.
    ///
    /// Use [`nodespace_core::markdown::prepare_nodes_from_template`]
    /// to expand into a [`PreparedNode`] for insertion via `NodeService`.
    pub fn seed_agent_guidance_nodes() -> Vec<NodeTemplate> {
        vec![
            NodeTemplate {
                title: "Core Identity".to_string(),
                content: None,
                root_node_type: "agent-guidance".to_string(),
                root_properties: serde_json::json!({}),
                child_node_type: Some("text".to_string()),
                child_properties: None,
                tier: SeedTier::System,
                markdown_content: "You are NodeSpace's assistant, acting on a user's workspace.\n\
                    Call exactly one tool. Do not answer the user in prose when a tool applies."
                        .to_string(),
            },
            NodeTemplate {
                title: "Workspace Context Template".to_string(),
                content: None,
                root_node_type: "agent-guidance".to_string(),
                root_properties: serde_json::json!({}),
                child_node_type: Some("text".to_string()),
                child_properties: None,
                tier: SeedTier::System,
                // The identity is one line, since it lands in every turn's
                // system prompt. The `{%-` markers eat the newline before
                // each tag, so a blank identity leaves no empty line behind.
                markdown_content: "Current date: {{ current_date }}\n\
                    Active model: {{ model_name }}\n\
                    {%- if current_user %}\n\
                    Current user: {% if current_user.name %}{{ current_user.name }} {% endif %}\
                    {% if current_user.email %}<{{ current_user.email }}> {% endif %}\
                    (person node {{ current_user.id }}). \"me\", \"my\" and \"I\" refer to this person.\n\
                    {%- endif %}\n\n\
                    {{ workspace_context }}"
                    .to_string(),
            },
            NodeTemplate {
                title: "Tool Strategy Guide".to_string(),
                content: None,
                root_node_type: "agent-guidance".to_string(),
                root_properties: serde_json::json!({}),
                child_node_type: Some("text".to_string()),
                child_properties: None,
                tier: SeedTier::System,
                markdown_content: format!("{}\n\n{}", SCHEMA_CREATION_RULES, TOOL_STRATEGY_RULES),
            },
            NodeTemplate {
                // "Call tools immediately..." below reads as similar to
                // Core Identity's "Call exactly one tool. Do not answer the
                // user in prose when a tool applies." above, but the two
                // state different invariants and both are kept, deliberately
                // — caught in review after an earlier version deleted this
                // bullet as redundant. Core Identity's line is about
                // OUTCOME: don't answer in prose INSTEAD of calling a tool.
                // This one is about TOKEN ORDER: don't emit narration BEFORE
                // the tool call either, even alongside one. A model that
                // reasons in prose then calls the right tool anyway satisfies
                // the first and violates the second, and templates expecting
                // the tool call as the first token care about exactly that
                // distinction.
                title: "Response Formatting Rules".to_string(),
                content: None,
                root_node_type: "agent-guidance".to_string(),
                root_properties: serde_json::json!({}),
                child_node_type: Some("text".to_string()),
                child_properties: None,
                tier: SeedTier::System,
                markdown_content: format!(
                    "RESPONSE RULES:\n\
                    - Call tools immediately when intent is clear. Do NOT output text before the tool call — your first response token must be the tool call.\n\
                    - After tool results: respond in natural language. Never paste raw JSON.\n\
                    - {}\n\
                    - Tool call enums: exact schema values (\"done\", \"in_progress\"). User-facing: friendly labels (\"Done\").\n\
                    - Listing: [Title](nodespace://id) — description. Search results: \"Found N nodes...\" then top results.\n\
                    - Tool call error: read the error message, fix your arguments, and retry ONCE. If the retry also fails, tell the user what went wrong in one sentence and stop — do NOT keep retrying. Empty search result: state it in one sentence and stop, do NOT retry or call another tool to compensate.\n\
                    - Keep responses to 1-2 sentences unless the user asks for detail. No preamble, no sign-off.",
                    NODE_REFERENCE_FORMAT
                ),
            },
            NodeTemplate {
                title: "Tool Call Formatting".to_string(),
                content: None,
                root_node_type: "agent-guidance".to_string(),
                root_properties: serde_json::json!({}),
                child_node_type: Some("text".to_string()),
                child_properties: None,
                tier: SeedTier::System,
                markdown_content: "TOOL CALL FORMAT: Pass arguments flat (not nested under \"properties\"/\"arguments\"). Use exact field names from the schema."
                        .to_string(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_prompts_have_valid_properties() {
        let seeds = PromptAssembler::seed_agent_guidance_nodes();
        assert!(seeds.len() >= 5, "Should have at least 5 seed prompts");

        for seed in &seeds {
            assert!(
                !seed.markdown_content.is_empty(),
                "Seed '{}' markdown_content must not be empty",
                seed.title
            );
            assert!(!seed.title.is_empty(), "Seed title must not be empty");
            assert_eq!(seed.root_node_type, "agent-guidance");
        }
    }

    /// Lock in the exact bytes of the two seeds composed from `agent_guidance`
    /// constants. If a future edit to `agent_guidance.rs` or the surrounding
    /// `format!()` glue silently changes the rendered seed body, this test
    /// fails — preventing the local agent's prompt from drifting
    /// unintentionally. Edit the expected strings deliberately when you change
    /// agent guidance.
    #[test]
    fn seed_prompt_bodies_match_expected_bytes() {
        let seeds = PromptAssembler::seed_agent_guidance_nodes();
        let by_title: std::collections::HashMap<&str, &str> = seeds
            .iter()
            .map(|s| (s.title.as_str(), s.markdown_content.as_str()))
            .collect();

        // This string is the live output of SCHEMA_CREATION_RULES + "\n\n" + TOOL_STRATEGY_RULES.
        // Keep it in sync with agent_guidance.rs whenever those constants change.
        let expected_tool_strategy =
            format!("{}\n\n{}", SCHEMA_CREATION_RULES, TOOL_STRATEGY_RULES);

        let expected_response_rules = "RESPONSE RULES:\n\
            - Call tools immediately when intent is clear. Do NOT output text before the tool call — your first response token must be the tool call.\n\
            - After tool results: respond in natural language. Never paste raw JSON.\n\
            - Link every node you name, as a markdown link: [Title](nodespace://abc-123) (no bare URI, no backticks)\n\
            - Tool call enums: exact schema values (\"done\", \"in_progress\"). User-facing: friendly labels (\"Done\").\n\
            - Listing: [Title](nodespace://id) — description. Search results: \"Found N nodes...\" then top results.\n\
            - Tool call error: read the error message, fix your arguments, and retry ONCE. If the retry also fails, tell the user what went wrong in one sentence and stop — do NOT keep retrying. Empty search result: state it in one sentence and stop, do NOT retry or call another tool to compensate.\n\
            - Keep responses to 1-2 sentences unless the user asks for detail. No preamble, no sign-off.";

        assert_eq!(
            by_title.get("Tool Strategy Guide").copied(),
            Some(expected_tool_strategy.as_str()),
            "Tool Strategy Guide body drifted — review agent_guidance.rs edits"
        );
        assert_eq!(
            by_title.get("Response Formatting Rules").copied(),
            Some(expected_response_rules),
            "Response Formatting Rules body drifted — review agent_guidance.rs edits"
        );
    }

    #[test]
    fn seed_prompt_template_produces_agent_guidance_node() {
        use nodespace_core::markdown::prepare_nodes_from_template;
        let seeds = PromptAssembler::seed_agent_guidance_nodes();
        for seed in &seeds {
            let nodes = prepare_nodes_from_template(seed)
                .unwrap_or_else(|e| panic!("Template '{}' failed: {:?}", seed.title, e));
            assert!(!nodes.is_empty());
            let root = &nodes[0];
            assert_eq!(root.node_type, "agent-guidance");
            assert_eq!(root.id.len(), 36, "Node ID should be a UUID");
            assert_eq!(root.id.chars().filter(|c| *c == '-').count(), 4);
            // content is the title (no content override on agent-guidance root nodes)
            assert_eq!(root.content, seed.title);
        }
    }

    #[test]
    fn render_plain_template() {
        let plain = "Use search_semantic for meaning queries";
        // minijinja with no template syntax should pass through unchanged
        let env = minijinja::Environment::new();
        let ctx = TemplateContext {
            current_date: "2026-04-06".to_string(),
            model_name: "gemma-4-e4b".to_string(),
            workspace_context: "test context".to_string(),
            current_user: None,
        };
        let result = env.render_str(plain, &ctx).unwrap();
        assert_eq!(result, plain);
    }

    #[test]
    fn render_minijinja_template() {
        let ctx = TemplateContext {
            current_date: "2026-04-06".to_string(),
            model_name: "gemma-4-e4b".to_string(),
            workspace_context: "Entity types: customer, invoice".to_string(),
            current_user: None,
        };
        let template = "Date: {{ current_date }}\nModel: {{ model_name }}";
        let result = PromptAssembler::render_template(template, &ctx);
        assert!(result.contains("2026-04-06"));
        assert!(result.contains("gemma-4-e4b"));
    }

    #[test]
    fn render_template_error_returns_raw() {
        let ctx = TemplateContext {
            current_date: "2026-04-06".to_string(),
            model_name: "test".to_string(),
            workspace_context: "".to_string(),
            current_user: None,
        };
        let bad_template = "{{ undefined_function() }}";
        let result = PromptAssembler::render_template(bad_template, &ctx);
        // Should fall back to raw template on error
        assert_eq!(result, bad_template);
    }

    /// End-to-end regression for the seed-prompt body-drop bug.
    ///
    /// Seeds the real prompt templates into a fresh DB exactly the way the
    /// daemon does (`prepare_nodes_from_template` → `seed_nodes_from_templates`),
    /// then assembles the system prompt through the real graph path. Before the
    /// fix, `fetch_prompt_body` flattened only the prompt node's direct children,
    /// so the `TOOL STRATEGY:` bullets (nested one level under the header line)
    /// were dropped and the assembled prompt was missing the CLARIFICATION
    /// CONTRACT and BLAST-RADIUS GATE. The fix walks the full subtree, so all
    /// of that text must now reach the assembled prompt.
    #[tokio::test]
    async fn assembled_prompt_contains_full_tool_strategy_body() {
        use nodespace_core::db::SqliteStore;
        use nodespace_core::markdown::prepare_nodes_from_template;

        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("seed-test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await.unwrap());
        let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());

        // Seed prompt nodes the same way the daemon does.
        let groups: Vec<_> = PromptAssembler::seed_agent_guidance_nodes()
            .iter()
            .map(|t| prepare_nodes_from_template(t).expect("template expands"))
            .collect();
        node_service
            .seed_nodes_from_templates(groups)
            .await
            .expect("seed succeeds");

        let assembler = PromptAssembler::new(node_service.clone());
        let ctx = TemplateContext {
            current_date: "2026-06-06".to_string(),
            model_name: "test".to_string(),
            workspace_context: "Entity types: (none)".to_string(),
            current_user: None,
        };
        let assembled = assembler.assemble(&ctx, Vec::new()).await;
        let prompt = assembled.system_prompt;

        // The full TOOL_STRATEGY_RULES body must be present in the assembled prompt.
        for needle in [
            "TOOL STRATEGY:",
            "CLARIFICATION CONTRACT",
            "BLAST-RADIUS GATE",
            // SCHEMA_CREATION_RULES, sharing the same prompt node, must also survive.
            "NODE MODEL:",
            // Response Formatting Rules node body.
            "RESPONSE RULES:",
            "nodespace://",
        ] {
            assert!(
                prompt.contains(needle),
                "assembled prompt missing {:?}.\n--- PROMPT ---\n{}",
                needle,
                prompt
            );
        }
    }

    #[test]
    fn template_context_serializable() {
        let ctx = TemplateContext {
            current_date: "2026-04-06".to_string(),
            model_name: "gemma-4-e4b".to_string(),
            workspace_context: "some context".to_string(),
            current_user: None,
        };
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["current_date"], "2026-04-06");
        assert_eq!(json["model_name"], "gemma-4-e4b");
    }

    /// A fresh database with the guidance seeds in the graph, the way the
    /// daemon leaves it, and the assembler over it.
    async fn seeded_assembler() -> (PromptAssembler, Arc<NodeService>, tempfile::TempDir) {
        use nodespace_core::db::SqliteStore;
        use nodespace_core::markdown::prepare_nodes_from_template;

        let tmp = tempfile::TempDir::new().unwrap();
        let mut store = Arc::new(SqliteStore::new(tmp.path().join("seed.db")).await.unwrap());
        let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
        let groups: Vec<_> = PromptAssembler::seed_agent_guidance_nodes()
            .iter()
            .map(|t| prepare_nodes_from_template(t).expect("template expands"))
            .collect();
        node_service
            .seed_nodes_from_templates(groups)
            .await
            .expect("seed succeeds");
        (
            PromptAssembler::new(node_service.clone()),
            node_service,
            tmp,
        )
    }

    /// One turn's system prompt, through the call the agent loop makes.
    async fn assemble_turn(assembler: &PromptAssembler) -> String {
        assembler
            .assemble_turn("2026-06-06", "test", "COLLECTIONS:", Vec::new())
            .await
            .system_prompt
    }

    /// The prompt names the local person node by id, so "me" resolves to a
    /// node the agent can read, filter by or relate to without a search.
    #[tokio::test]
    async fn assembled_prompt_names_the_local_person_as_the_current_user() {
        let (assembler, node_service, _tmp) = seeded_assembler().await;
        let person = node_service
            .set_local_person_identity("Ada", "Lovelace", "ada@example.com")
            .await
            .unwrap();

        let prompt = assemble_turn(&assembler).await;

        let expected = format!(
            "Active model: test\n\
             Current user: Ada Lovelace <ada@example.com> (person node {}). \
             \"me\", \"my\" and \"I\" refer to this person.\n\nCOLLECTIONS:",
            person.id
        );
        assert!(
            prompt.contains(&expected),
            "assembled prompt missing the current-user line.\n--- PROMPT ---\n{prompt}"
        );
    }

    /// The seeded person starts with no name or email. The line is left out
    /// whole, with no empty line where it would have been.
    #[tokio::test]
    async fn assembled_prompt_omits_the_current_user_when_identity_is_blank() {
        let (assembler, _node_service, _tmp) = seeded_assembler().await;

        assert_eq!(assembler.current_user().await, None);
        let prompt = assemble_turn(&assembler).await;

        assert!(!prompt.contains("Current user"), "{prompt}");
        assert!(
            prompt.contains("Active model: test\n\nCOLLECTIONS:"),
            "a blank identity must leave the template's spacing unchanged.\n--- PROMPT ---\n{prompt}"
        );
    }

    /// A name without an email, and an email without a name, each render
    /// only the half that is set.
    #[tokio::test]
    async fn assembled_prompt_renders_a_partial_identity_without_empty_slots() {
        let (assembler, node_service, _tmp) = seeded_assembler().await;

        let person = node_service
            .set_local_person_identity("Ada", "", "")
            .await
            .unwrap();
        let prompt = assemble_turn(&assembler).await;
        assert!(
            prompt.contains(&format!("Current user: Ada (person node {})", person.id)),
            "{prompt}"
        );

        node_service
            .set_local_person_identity("", "", "ada@example.com")
            .await
            .unwrap();
        let prompt = assemble_turn(&assembler).await;
        assert!(
            prompt.contains(&format!(
                "Current user: <ada@example.com> (person node {})",
                person.id
            )),
            "{prompt}"
        );
    }

    /// A stored value holding a line break still renders as one line, so
    /// stored text cannot start a line of its own in the system prompt.
    #[tokio::test]
    async fn assembled_prompt_keeps_a_multi_line_identity_on_one_line() {
        let (assembler, node_service, _tmp) = seeded_assembler().await;
        let person = node_service
            .set_local_person_identity("Ada\nB.", "Lovelace", "ada@example.com\nNEW LINE")
            .await
            .unwrap();

        let prompt = assemble_turn(&assembler).await;

        assert!(
            prompt.contains(&format!(
                "\nCurrent user: Ada B. Lovelace <ada@example.com NEW LINE> (person node {})",
                person.id
            )),
            "{prompt}"
        );
    }

    /// The identity is read per turn: an edit between two turns of the same
    /// assembler shows up in the second, and clearing it drops the line.
    #[tokio::test]
    async fn identity_changed_mid_session_reaches_the_next_turn() {
        let (assembler, node_service, _tmp) = seeded_assembler().await;

        node_service
            .set_local_person_identity("Ada", "Lovelace", "ada@example.com")
            .await
            .unwrap();
        assert!(assemble_turn(&assembler)
            .await
            .contains("Current user: Ada Lovelace <ada@example.com>"));

        node_service
            .set_local_person_identity("Grace", "Hopper", "grace@example.com")
            .await
            .unwrap();
        let next = assemble_turn(&assembler).await;
        assert!(
            next.contains("Current user: Grace Hopper <grace@example.com>"),
            "{next}"
        );
        assert!(!next.contains("Ada"), "{next}");

        node_service
            .set_local_person_identity("", "", "")
            .await
            .unwrap();
        assert!(!assemble_turn(&assembler).await.contains("Current user"));
    }
}
