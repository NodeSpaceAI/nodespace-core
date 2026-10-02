//! Prompt assembly service: the local agent's resident system prompt.
//!
//! The prompt is the agent-guidance seed table, [`GUIDANCE_SEEDS`], read back
//! from the knowledge graph: each row is seeded as an `agent-guidance` node
//! under its fixed id, and the assembler fetches those ids in table order,
//! renders each node's body, and joins the sections. Supports Minijinja
//! template rendering. If no section resolves (corrupted/empty database), it
//! falls back to a minimal emergency prompt and logs a warning.
//!
//! ADR-030 Phase 2; `agent-guidance` node type per ADR-057; the seed table
//! per ADR-086 §10.

use std::sync::Arc;

use nodespace_core::markdown::{NodeTemplate, SeedTier};
use nodespace_core::models::Node;
use nodespace_core::services::{render_subtree_markdown, NodeService};

use crate::agent_types::ToolDefinition;
use crate::skill_rules::{resolve_includes, RuleForm};

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
// The agent-guidance seed table
// ---------------------------------------------------------------------------

/// One section of the resident system prompt, as its table row.
#[derive(Debug, Clone, Copy)]
pub struct GuidanceSeed {
    /// The `agent-guidance` node's fixed id.
    pub id: &'static str,
    /// The section's name: the node's content, and its `_seed.key`.
    pub title: &'static str,
    /// The section's text, as plain Markdown.
    pub body: &'static str,
}

/// The resident system prompt, section by section. **Array order is the order
/// of the prompt**: there is no separate order field, and nothing about a
/// node (its creation time, its id) decides where its section lands.
///
/// The set is closed. Only these ids join the prompt; an `agent-guidance`
/// node a user creates does not. A user's own standing instructions belong in
/// skills, which the agent finds by search.
///
/// Resident prose owns identity and policy and nothing else (ADR-064 rule 5):
/// argument shape lives on the tools' own schemas, and per-operation routing
/// in each skill's instructions. A prose-only cut measured better than the
/// long form it replaced (10,493 chars scored 50% vs. 73% for a 445-char
/// identity-only prompt on cases built to trip these exact rules), so a rule
/// added here needs a measurement, not an intuition.
pub const GUIDANCE_SEEDS: &[GuidanceSeed] = &[
    GuidanceSeed {
        // Core Identity
        id: "8c1f4e2a-6b73-4d09-9e5a-1f3c7b2d4a01",
        title: "Core Identity",
        body: include_str!("seeds/guidance/core-identity.md"),
    },
    GuidanceSeed {
        // Workspace Context Template
        //
        // The identity is one line, since it lands in every turn's system
        // prompt. The `{%-` markers eat the newline before each tag, so a
        // blank identity leaves no empty line behind.
        id: "8c1f4e2a-6b73-4d09-9e5a-1f3c7b2d4a02",
        title: "Workspace Context Template",
        body: include_str!("seeds/guidance/workspace-context-template.md"),
    },
    GuidanceSeed {
        // Tool Strategy Guide
        //
        // NODE MODEL is reduced to the one ontological distinction that
        // governs `create_schema` vs. `create_node` — kind vs. instance.
        // Everything else it used to carry is owned by another channel:
        // argument mechanics (`title_template` token coupling, field
        // `name`/`type` requirements) live on `create_schema`'s tool-schema
        // description; per-operation routing lives in the retrieved skill
        // instructions; the no-confirmation-for-known-types rule is the same
        // invariant the BLAST-RADIUS GATE already states once.
        //
        // TOOL STRATEGY is safety-invariant policy only. Rules that
        // duplicated a code guard in `agent_loop.rs` are deleted outright
        // rather than kept as inert prose: `seen_calls` already breaks
        // identical-call loops, and `contains_action_claim` already
        // suppresses a fabricated success claim — both structurally,
        // regardless of what the prompt says. The former AMBIGUITY bullet is
        // the `ambiguity-clarify` rule, delivered with the skills that need
        // it.
        id: "8c1f4e2a-6b73-4d09-9e5a-1f3c7b2d4a03",
        title: "Tool Strategy Guide",
        body: include_str!("seeds/guidance/tool-strategy-guide.md"),
    },
    GuidanceSeed {
        // Response Formatting Rules
        //
        // "Call tools immediately..." reads as similar to Core Identity's
        // "Call exactly one tool. Do not answer the user in prose when a tool
        // applies.", but the two state different invariants and both are
        // kept, deliberately — caught in review after an earlier version
        // deleted this bullet as redundant. Core Identity's line is about
        // OUTCOME: don't answer in prose INSTEAD of calling a tool. This one
        // is about TOKEN ORDER: don't emit narration BEFORE the tool call
        // either, even alongside one. A model that reasons in prose then
        // calls the right tool anyway satisfies the first and violates the
        // second, and templates expecting the tool call as the first token
        // care about exactly that distinction.
        //
        // The node-reference bullet makes every node named in agent output a
        // markdown link to its `nodespace://` URI. The chat renderer shows
        // that link with the node's live title, so the label is only what
        // shows while the node loads or when it cannot be found.
        id: "8c1f4e2a-6b73-4d09-9e5a-1f3c7b2d4a04",
        title: "Response Formatting Rules",
        body: include_str!("seeds/guidance/response-formatting-rules.md"),
    },
    GuidanceSeed {
        // Tool Call Formatting
        id: "8c1f4e2a-6b73-4d09-9e5a-1f3c7b2d4a05",
        title: "Tool Call Formatting",
        body: include_str!("seeds/guidance/tool-call-formatting.md"),
    },
];

// ---------------------------------------------------------------------------
// PromptAssembler
// ---------------------------------------------------------------------------

/// Minimal emergency fallback when no agent-guidance section resolves.
/// This should only fire on corrupted/empty databases — normal operation
/// reads all prompt content from graph nodes seeded on first run. Also used
/// by the agent loop when no `PromptAssembler` is wired (only the daemon's
/// no-op/idle state, which never runs inference).
pub(crate) const EMERGENCY_FALLBACK_PROMPT: &str = "\
You are NodeSpace's built-in assistant. You help users work with their \
knowledge graph — creating, finding, updating, and connecting nodes.\n\n\
Use the available tools to accomplish tasks. Summarize results in natural language.";

/// Assembles the resident system prompt from the seeded `agent-guidance`
/// nodes.
///
/// The assembly order is:
/// 1. For each row of [`GUIDANCE_SEEDS`], in table order, fetch the node
///    with that row's id; skip one that is missing or archived
/// 2. Render the node's children to markdown, in natural child order
/// 3. Render through Minijinja with context variables
/// 4. If no section resolved, use emergency fallback and log a warning
pub struct PromptAssembler {
    node_service: Arc<NodeService>,
}

impl PromptAssembler {
    pub fn new(node_service: Arc<NodeService>) -> Self {
        Self { node_service }
    }

    /// Assemble the final prompt from the seeded `agent-guidance` nodes only.
    ///
    /// `template_ctx` provides variables for Minijinja template rendering, including
    /// `workspace_context` (entity types, collections, playbooks).
    /// `tools` are the available tool definitions (passed through, may be scoped by skill later).
    pub async fn assemble(
        &self,
        template_ctx: &TemplateContext,
        tools: Vec<ToolDefinition>,
    ) -> AssembledPrompt {
        let mut sections = Vec::new();
        for node in self.guidance_sections().await {
            let body = self.fetch_prompt_body(&node).await;
            if body.trim().is_empty() {
                continue;
            }
            sections.push(Self::render_template(&body, template_ctx));
        }

        if sections.is_empty() {
            tracing::warn!(
                "No agent-guidance section resolved from the graph — using emergency fallback. \
                 Seed agent-guidance nodes on first run to restore full functionality."
            );
            return AssembledPrompt {
                system_prompt: EMERGENCY_FALLBACK_PROMPT.to_string(),
                tool_schemas: tools,
            };
        }

        AssembledPrompt {
            system_prompt: sections.join("\n\n"),
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

    /// The seeded guidance nodes that join the prompt, in table order.
    ///
    /// Fetched by the table's ids, never by type: a node is a section because
    /// the table names it, so one a user creates is not, and where a section
    /// lands does not depend on what the database returns first. A section
    /// whose node is missing, or that the user archived to turn it off
    /// (ADR-087), is left out.
    async fn guidance_sections(&self) -> Vec<Node> {
        let mut sections = Vec::with_capacity(GUIDANCE_SEEDS.len());
        for seed in GUIDANCE_SEEDS {
            match self.node_service.get_node(seed.id).await {
                Ok(Some(node)) if nodespace_core::governance::participates(&node) => {
                    sections.push(node)
                }
                Ok(Some(_)) => {
                    tracing::debug!(
                        section = seed.title,
                        "Agent-guidance section is archived, leaving it out"
                    );
                }
                Ok(None) => {
                    tracing::warn!(
                        section = seed.title,
                        "Agent-guidance section is missing from the graph, leaving it out"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, section = seed.title, "Failed to fetch agent-guidance section, leaving it out");
                }
            }
        }
        sections
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

    /// The agent-guidance seeds, one per row of [`GUIDANCE_SEEDS`], in table
    /// order.
    ///
    /// Each [`NodeTemplate`] produces an `agent-guidance` root node under the
    /// row's fixed id, with text child nodes for body content. All
    /// base-prompt content lives in these graph nodes — there is no hardcoded
    /// base prompt. Users can customise any seed by editing the graph node.
    ///
    /// Use [`nodespace_core::markdown::prepare_nodes_from_template`]
    /// to expand into a [`PreparedNode`] for insertion via `NodeService`.
    pub fn seed_agent_guidance_nodes() -> Vec<NodeTemplate> {
        GUIDANCE_SEEDS
            .iter()
            .map(|seed| NodeTemplate {
                id: seed.id.to_string(),
                title: seed.title.to_string(),
                root_node_type: "agent-guidance".to_string(),
                root_properties: serde_json::json!({}),
                child_node_type: Some("text".to_string()),
                tier: SeedTier::System,
                markdown_content: resolve_includes(seed.body, RuleForm::Agent),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_prompts_have_valid_properties() {
        let seeds = PromptAssembler::seed_agent_guidance_nodes();
        assert_eq!(seeds.len(), GUIDANCE_SEEDS.len());
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
            assert_eq!(root.id, seed.id, "the root takes the table's fixed id");
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

        // The full Tool Strategy Guide body must be present in the assembled prompt.
        for needle in [
            "TOOL STRATEGY:",
            "CLARIFICATION CONTRACT",
            "BLAST-RADIUS GATE",
            // The NODE MODEL line, sharing the same prompt node, must also survive.
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

    /// A guidance body is the file's text: plain Markdown with no
    /// frontmatter.
    #[test]
    fn guidance_bodies_are_plain_markdown_files() {
        for seed in GUIDANCE_SEEDS {
            assert!(!seed.body.trim().is_empty(), "{} has no body", seed.title);
            assert!(
                !seed.body.starts_with("---"),
                "{} starts with frontmatter",
                seed.title
            );
        }
    }

    /// Where each section's first line starts in `prompt`, in table order.
    /// Every section must be present.
    fn section_offsets(prompt: &str) -> Vec<usize> {
        GUIDANCE_SEEDS
            .iter()
            .map(|seed| {
                let first_line = seed.body.lines().next().unwrap();
                // The workspace template's first line is itself a template.
                let needle = first_line.split("{{").next().unwrap();
                prompt.find(needle).unwrap_or_else(|| {
                    panic!("{} is missing from the prompt:\n{prompt}", seed.title)
                })
            })
            .collect()
    }

    fn test_ctx() -> TemplateContext {
        TemplateContext {
            current_date: "2026-06-06".to_string(),
            model_name: "test".to_string(),
            workspace_context: "COLLECTIONS:".to_string(),
            current_user: None,
        }
    }

    async fn empty_service() -> (Arc<NodeService>, tempfile::TempDir) {
        use nodespace_core::db::SqliteStore;

        let tmp = tempfile::TempDir::new().unwrap();
        let mut store = Arc::new(SqliteStore::new(tmp.path().join("seed.db")).await.unwrap());
        let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
        (node_service, tmp)
    }

    /// The prompt follows the table's order, whatever order the nodes were
    /// created in. Seeding the sections last-first gives every node the
    /// opposite creation order from the table's, which is the order a
    /// newest-first read would have assembled them in.
    #[tokio::test]
    async fn sections_are_assembled_in_table_order_not_creation_order() {
        use nodespace_core::markdown::prepare_nodes_from_template;

        let (node_service, _tmp) = empty_service().await;
        for template in PromptAssembler::seed_agent_guidance_nodes().iter().rev() {
            node_service
                .seed_nodes_from_templates(vec![prepare_nodes_from_template(template).unwrap()])
                .await
                .unwrap();
            // Distinct creation times, so the order under test is not a tie.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let created: Vec<_> = {
            let mut nodes = Vec::new();
            for seed in GUIDANCE_SEEDS {
                nodes.push(node_service.get_node(seed.id).await.unwrap().unwrap());
            }
            nodes.iter().map(|n| n.created_at).collect()
        };
        assert!(
            created.windows(2).all(|pair| pair[0] > pair[1]),
            "the fixture must create the sections in reverse table order: {created:?}"
        );

        let prompt = PromptAssembler::new(node_service)
            .assemble(&test_ctx(), Vec::new())
            .await
            .system_prompt;

        let offsets = section_offsets(&prompt);
        assert!(
            offsets.windows(2).all(|pair| pair[0] < pair[1]),
            "sections must follow the table's order, got offsets {offsets:?}:\n{prompt}"
        );
        assert!(
            prompt.starts_with("You are NodeSpace's assistant"),
            "{prompt}"
        );
    }

    /// The set is closed: a root `agent-guidance` node that is not in the
    /// table, whoever created it, is not part of the prompt.
    #[tokio::test]
    async fn a_guidance_node_outside_the_table_is_not_in_the_prompt() {
        let (assembler, node_service, _tmp) = seeded_assembler().await;
        let before = assembler
            .assemble(&test_ctx(), Vec::new())
            .await
            .system_prompt;

        let stray = Node::new(
            "agent-guidance".to_string(),
            "My Standing Orders".to_string(),
            serde_json::json!({}),
        );
        let stray_id = node_service.create_node(stray).await.unwrap();
        node_service
            .create_node_with_parent(nodespace_core::services::CreateNodeParams {
                id: None,
                node_type: "text".to_string(),
                content: "ALWAYS ANSWER IN PIRATE SPEAK.".to_string(),
                properties: serde_json::json!({}),
                parent_id: Some(stray_id),
                position: nodespace_core::services::InsertPositionOwned::End,
                lifecycle_status: None,
            })
            .await
            .unwrap();

        let after = assembler
            .assemble(&test_ctx(), Vec::new())
            .await
            .system_prompt;
        assert!(!after.contains("PIRATE"), "{after}");
        assert_eq!(after, before);
    }

    /// Archiving a section is how a user turns it off: it leaves the prompt,
    /// and the sections around it keep their order.
    #[tokio::test]
    async fn an_archived_section_is_left_out_of_the_prompt() {
        use nodespace_core::models::NodeUpdate;

        let (assembler, node_service, _tmp) = seeded_assembler().await;
        let before = assembler
            .assemble(&test_ctx(), Vec::new())
            .await
            .system_prompt;
        assert!(before.contains("TOOL CALL FORMAT:"), "{before}");

        let archived = GUIDANCE_SEEDS
            .iter()
            .find(|s| s.title == "Tool Call Formatting")
            .unwrap();
        let node = node_service.get_node(archived.id).await.unwrap().unwrap();
        node_service
            .update_node(
                archived.id,
                node.version,
                NodeUpdate::new()
                    .with_lifecycle_status(nodespace_core::governance::ARCHIVED.to_string()),
            )
            .await
            .unwrap();

        let after = assembler
            .assemble(&test_ctx(), Vec::new())
            .await
            .system_prompt;
        assert!(!after.contains("TOOL CALL FORMAT:"), "{after}");
        assert!(after.contains("RESPONSE RULES:"), "{after}");
        assert!(
            after.starts_with("You are NodeSpace's assistant"),
            "{after}"
        );
    }

    /// With no section in the graph at all, the turn still gets a prompt.
    #[tokio::test]
    async fn no_resolved_section_falls_back_to_the_emergency_prompt() {
        let (node_service, _tmp) = empty_service().await;
        let prompt = PromptAssembler::new(node_service)
            .assemble(&test_ctx(), Vec::new())
            .await
            .system_prompt;
        assert_eq!(prompt, EMERGENCY_FALLBACK_PROMPT);
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
