//! The seeded skills and tools: one table row per built-in.
//!
//! [`SKILL_SEEDS`] is the built-in skill table. A row holds the skill's fixed
//! id, its retrieval and dispatch config, and its guidance as a plain
//! Markdown file under `seeds/skills/`. [`seed_skill_nodes`] turns the rows
//! into [`NodeTemplate`]s; use
//! [`nodespace_core::markdown::prepare_nodes_from_template`] to expand one
//! into the nodes `NodeService::seed_nodes_from_templates` reconciles.
//!
//! A skill body states its own procedure and includes the rules it shares
//! with other skills and with the skill external agents install, by id:
//! `<!-- include: find-then-act -->`. The rule's text lives once, in
//! `seeds/rules/` (see [`crate::skill_rules`]), so two skills that carry the
//! same rule cannot drift apart, and neither can the two surfaces.
//!
//! Skill discovery is LLM-orchestrated through the `search_skills` tool
//! exposed by [`crate::local_agent::tools`]; nothing here routes a turn.
//!
//! Tool nodes (`tool-native`) bridge graph storage to deterministic Rust
//! handlers. Tools stay defined in Rust, each bound to a handler with a typed
//! parameter schema; [`seed_tool_nodes`] seeds one node per
//! [`crate::local_agent::tools::Tool`], under that tool's fixed id.

use crate::skill_rules::{resolve_includes, RuleForm};
use nodespace_core::markdown::{NodeTemplate, SeedTier};
use nodespace_core::models::{CoreNodeType, SkillFields};

/// One built-in skill, as its table row.
#[derive(Debug, Clone, Copy)]
pub struct SkillSeed {
    /// The skill node's fixed id.
    pub id: &'static str,
    /// The skill's name, embedded for retrieval with its description.
    pub title: &'static str,
    /// What the skill is for. Only words for what the skill DOES: an
    /// embedding has no negation.
    pub description: &'static str,
    /// Tools a turn that selects this skill may call.
    pub tools: &'static [&'static str],
    /// ReAct iteration budget for the skill.
    pub max_iterations: u32,
    /// What the skill is not for, scored against the query separately.
    pub exclusion: Option<&'static str>,
    /// The guidance, as plain Markdown with rule includes.
    pub body: &'static str,
}

impl SkillSeed {
    /// The seed this row installs, with its rule includes resolved for the
    /// local agent.
    pub fn template(&self) -> NodeTemplate {
        let mut skill = SkillFields::new(self.description, self.tools, self.max_iterations);
        if let Some(exclusion) = self.exclusion {
            skill = skill.with_exclusion(exclusion);
        }
        NodeTemplate::skill(
            self.id,
            self.title,
            skill,
            resolve_includes(self.body, RuleForm::Agent),
        )
    }
}

/// The built-in skills.
///
/// Each row produces one skill root node plus ordinary markdown children
/// (header/text, inferred from the guidance markdown's structure) carrying the
/// guidance body. Tool whitelists and max_iterations are still stored
/// as properties on the skill node — they're consumed by external (ACP) agents
/// that prefer the older skill-scoped flow. The local agent ignores them and
/// just uses the description/name returned by `search_skills`.
///
/// None links to a schema through `applies_to`: the built-ins are generic
/// across every type, so each takes skill search's unlinked fallback.
pub const SKILL_SEEDS: &[SkillSeed] = &[
    SkillSeed {
        // Research & Search
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c01",
        title: "Research & Search",
        description: "Search and explore the knowledge graph to find relevant information, discover connections, and answer questions about stored knowledge.",
        tools: &["search_semantic", "search_nodes", "get_node"],
        max_iterations: 4,
        exclusion: None,
        body: include_str!("seeds/skills/research-and-search.md"),
    },
    SkillSeed {
        // Node Creation
        //
        // The body includes the shared named-record rule, so the external
        // skill's copy of it (`packages/skill/SKILL.md`) cannot drift from
        // this one on substance.
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c02",
        title: "Node Creation",
        description: "Create new nodes, records, entries, or instances of any type — tasks, text notes, or custom types like Spec, ADR, Ticket. Use when user wants to add, create, or insert a new item, record, entry, or example of an existing type.",
        // `update_node` is whitelisted here as well as on Graph
        // Editing — deliberately, to remove a single point of failure
        // rather than because this skill is about editing.
        //
        // Every write tool in this seed set except `create_relationship`
        // was reachable from exactly ONE skill, while read tools sat in
        // six or seven. With RETRIEVAL_TOP_K = 3, that makes a write
        // tool's availability a lottery: if its sole owner misses the
        // window, the tool does not exist for that turn and the model
        // cannot call it however well it reasons.
        //
        // Measured on the locked model, 6 reps of the same seeded chain
        // on a warm index: the write turn failed 3 times, every failure
        // being a turn where Graph Editing placed 4th or worse and no
        // other candidate carried `update_node`. The two outcomes were
        // exactly:
        //   pass: Node Creation, Node Deletion, Graph Editing
        //   fail: Schema Creation, Node Deletion, Research & Search
        // Node Creation is present on the passing shape and is the
        // nearest neighbour of an update request in embedding space
        // ("add a record" and "change that record" are one user intent
        // expressed two ways), so it is the natural second home.
        //
        // Blast radius is UNCHANGED, not merely bounded: this skill
        // already whitelisted `create_node`, so `skill_is_mutating` was
        // already true for it and `score_bar_for` already returned
        // MUTATING. Adding another mutating tool moves nothing. The
        // destructive rung is untouched either way —
        // `stage2_permitted_names` admits destructive tools only from
        // the retrieval winner, and neither added tool is destructive.
        // `route_clarify` is here so this skill can hand an
        // already-exists collision back to the user rather than
        // resolving it by guessing. Without it, the guidance below
        // ("ask which they meant") would name a tool the turn cannot
        // reach, and the model's only options are to create a
        // duplicate or to refuse in prose — measured: "Add Northwind
        // Trading to the companies we sell to", with Northwind already
        // in the graph and rendered in MENTIONED ENTITIES, produced
        // `create_node` and a silent duplicate on 3 of 3 reps.
        tools: &["create_node", "update_node", "update_task_status", "search_semantic", "search_nodes", "get_node", "route_clarify"],
        max_iterations: 3,
        exclusion: None,
        body: include_str!("seeds/skills/node-creation.md"),
    },
    SkillSeed {
        // Schema Creation
        //
        // Argument shape (which fields to omit, enum value casing, the
        // `title_template` placeholder rule) lives on `create_schema`'s own
        // tool-schema descriptions (`local_agent/tools.rs`) — ADR-064 rule 1 — so
        // the rules that state only that shape (`no-name-title-field`,
        // `name-placeholder-exception`, `fields-from-request-only`, `enum-format`)
        // are not included in the body: duplicating a schema-stated rule into
        // prose is how the two drifted before (the `coreValues`/`core_values`
        // contradiction ADR-064 records). The worked JSON examples moved the same
        // way, onto `create_schema`'s own description, per ADR-038 Finding 2 (the
        // model reproduces the worked example's pattern, so the example belongs
        // where the model reads it right before calling).
        //
        // What the body holds is procedure: which tool to call, in what order, and
        // facts about the API's response contract (already-exists, validation-error
        // retry) that a schema cannot express. Those rules are included from
        // [`crate::skill_rules`], so they cannot drift from the "Schema
        // inspection and management" section of `packages/skill/SKILL.md`, which
        // renders the same rules in their prose form.
        //
        // `SCHEMA_RULES_NOT_IN_PROMPT` (test-only, below this table) is the
        // explicit, reviewed record of every `SCHEMA_RULES` entry the body
        // deliberately does not include — see its own doc comment for why a
        // hand-maintained list, rather than nothing, is the guard.
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c03",
        title: "Schema Creation",
        description: "Set up a structured way to keep track of, log, or maintain records for a kind of thing the user hasn't stored before — specs, sprints, releases, tickets, or any recurring category of item with its own details to fill in. Also covers defining a new entity type or schema with custom fields, enums, and relationships, or modifying an existing schema. Use when the user wants a place to record or organize instances of something new, or says 'new type', 'node type', 'define fields', 'create schema', 'update schema', 'add a field', 'rename a field', or wants to design or change a kind of entity like Spec, Ticket, or ADR.",
        tools: &["create_schema", "update_schema", "get_node"],
        max_iterations: 3,
        exclusion: None,
        body: include_str!("seeds/skills/schema-creation.md"),
    },
    SkillSeed {
        // Graph Editing
        //
        // The body includes shared interaction rules from
        // [`crate::skill_rules`] (find-then-act, ambiguity clarification,
        // dedicated task-status verb, success-means-stop).
        //
        // The allowed-values / id-provenance rules are stated in prose there
        // as well as on `update_node`'s own schema (`local_agent/tools.rs`) — not a
        // duplication ADR-064 forbids, because this text is procedure (WHEN to
        // treat a description as already-resolved vs. needing `resolve_query`),
        // not argument shape. The two invoice worked examples formerly here are
        // deleted outright rather than reworded: no case exercising this skill
        // needs a worked clarification example, and a business-domain one taught
        // nothing this text's own principle didn't already state.
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c04",
        title: "Graph Editing",
        // Names the completion states users actually say ("mark it
        // resolved", "mark it paid"). The prior wording ("Modify
        // existing nodes... update content, properties, titles, and
        // metadata") missed the top-3 for 5 of 7 such requests on the
        // locked embedding model, while the conflict skill (then
        // titled "Conflict Resolution") won them on the shared word
        // "resolve" and left no write tool on Stage 2's surface. Now
        // in the top-3 for all 7.
        //
        // Kept narrow on purpose. A broader draft listing "closed" and
        // "paid" as nouns ("the invoice is paid, the ticket is closed")
        // outranked Node Deletion on "remove the closed tickets",
        // silently withholding delete_node; "keep it" / "stay" is what
        // holds deletion requests on Node Deletion. Guarded in
        // `tests/it/live_skill_retrieval_stability.rs` by
        // `completion_state_updates_route_graph_editing`,
        // `control_conflict_requests_still_route_conflict_journal`,
        // and `control_deletion_requests_are_not_outranked_by_graph_editing`.
        description: "Update a record that already exists and keep it: mark it resolved, done, or paid, or set or change one of its fields, status, title, or content. Use when the user wants an existing item to stay but move to a new state. For tasks, use update_task_status to change status.",
        // `create_node` is whitelisted here as the mirror of
        // `update_node` on Node Creation: "record this" and "change
        // that" are the same user intent inflected two ways, and either
        // skill can win retrieval on either phrasing. Pairing them means
        // whichever one places, the turn can still write. See
        // `no_write_tool_is_reachable_from_only_one_skill`.
        tools: &["update_node", "update_task_status", "create_node", "get_node", "search_nodes", "search_semantic", "resolve_query", "route_clarify"],
        max_iterations: 3,
        // "remove the resolved tickets" still out-ranked Node Deletion
        // (0.855 vs 0.841) on "mark it resolved": the two requests
        // differ only in the verb, which the embedding barely weights.
        // No description wording separates them — every variant that
        // lowered this skill on the deletion request lowered it by
        // the same amount on "mark incident resolved", leaving that
        // completion-state guard 0.003 from falling out of the top 3.
        //
        // An exclusion is scored against the query separately and
        // costs this skill only on requests closer to it than to the
        // description (`skill_ops::exclusion_penalized_score`). This
        // one puts Node Deletion first on "remove the resolved
        // tickets" by +0.047 and leaves every completion-state score
        // unchanged. Its one measured cost: "remove the due date from
        // the launch task" (a field, not a record) loses 0.009 and
        // stays in the top 3.
        //
        // Wording is measured, not intuitive. Two longer drafts ("…or
        // get rid of records so they no longer exist", "Delete or
        // remove records.") also lowered "mark the outage report done"
        // or "record that we decided to use Postgres" by 0.02–0.035;
        // bare verbs aimed at "them" lowered nothing but deletions.
        // Guarded in `tests/it/live_skill_retrieval_stability.rs` by
        // `remove_requests_mentioning_a_state_route_node_deletion`,
        // `removing_a_field_still_reaches_graph_editing`, and
        // `graph_editing_exclusion_leaves_completion_state_scores_unchanged`.
        exclusion: Some("Remove them, delete them, get rid of them, purge them."),
        body: include_str!("seeds/skills/graph-editing.md"),
    },
    SkillSeed {
        // Relationship Management
        //
        // The body of the Relationship Management skill, including
        // the shared find-then-act and success-means-stop rules.
        // The DIRECTION rule below is stated in prose here as well as on
        // `create_relationship`'s own `from_id`/`to_id` parameter descriptions
        // (`local_agent/tools.rs`) — a deliberate duplication, not a drift risk left
        // unguarded: `create_relationship` is whitelisted by BOTH this skill and
        // Organization (this table's "Organization" row), and Organization's
        // own guidance does not restate it. A turn routed to Organization instead of
        // here would see `create_relationship` with only the tool schema's copy of
        // the rule, so the tool schema alone must already carry it — the same
        // reachability argument the Graph Editing row's comment makes for its
        // id-provenance rules.
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c05",
        title: "Relationship Management",
        // The prior wording ("Create connections between nodes,
        // explore relationships, and traverse the knowledge graph")
        // never used the verbs a user actually says for linking two
        // things, so it lost Stage-2 retrieval outright on "point
        // rebuild task at the decision it has to respect" — this
        // skill's own score did not even reach the printed top-3
        // against that query.
        //
        // Measured on the locked embedding model against that exact
        // query plus the seeded control prompts for Node Creation,
        // Graph Editing, Node Deletion, Schema Creation, and
        // Organization: an earlier draft leaning on generic
        // "link"/"connect"/"associate"/"relates to" vocabulary DID
        // clear RETRIEVAL_TOP_K on the failing query, but that same
        // generic vocabulary crowded Organization itself out of its
        // own top-3 on "Add this note to my reading list collection"
        // — reproducing this exact defect for a different skill,
        // since Organization also whitelists create_relationship and
        // its own description already uses "categorize"/"group",
        // semantically adjacent to "associate"/"relate". Dropping the
        // generic verbs and leading with "edge" plus the query's own
        // "depends on"/"must respect"/"points at" phrasing clears the
        // failing query (Relationship Management: unranked 6th at
        // ~0.803 -> 2nd at ~0.837) without displacing Organization's
        // own top-3 on its control prompt.
        description: "Record an edge between two nodes: a task or note that depends on, must respect, or points at another record. Explore or traverse existing relationships between nodes in the knowledge graph.",
        tools: &["create_relationship", "get_related_nodes", "get_node", "search_semantic", "search_nodes"],
        max_iterations: 3,
        exclusion: None,
        body: include_str!("seeds/skills/relationship-management.md"),
    },
    SkillSeed {
        // Node Deletion
        //
        // The body of the Node Deletion skill, including the
        // shared find-then-act, single-item-per-call, and success-means-stop rules.
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c06",
        title: "Node Deletion",
        // Destructive verbs ONLY. Two rules, both learned from
        // measurement, and both about what an embedding encodes.
        //
        // 1. No generic noun tail. An earlier wording ended "...remove,
        //    delete, or trash a node or record", and that trailing noun
        //    made this skill an attractor for anything node-shaped: it
        //    was retrieved on turns that recorded a decision, marked a
        //    status, set a due date, and asked a plain question. Every
        //    one of those prompts is *about* a node or record; only the
        //    verb distinguishes them.
        //
        // 2. NO DISCLAIMER NAMING OTHER OPERATIONS. The fix for (1)
        //    appended "— not to record, update, mark, or look something
        //    up", which made things worse in a way prose review cannot
        //    catch: embeddings have no notion of negation. A sentence
        //    listing "record, update, mark, look up" embeds NEARER those
        //    intents, not further from them. The disclaimer meant to
        //    exclude update requests is what pulled this skill onto them.
        //
        //    Measured on the locked model, write turn "The five-day one
        //    got signed off — mark it that way": this skill was in the
        //    Stage-2 top-3 on ALL 18 turns of a 6-rep run, including all
        //    three that then failed for want of `update_node`. The word
        //    "mark" in the disclaimer is in the user's prompt.
        //
        // The rule this encodes: a retrieval description may contain only
        // words for what the skill DOES. Scoping ("use this only when…")
        // belongs in the instruction subtree, which the model reads as
        // text — not in the description, which is what gets embedded.
        description: "Delete, remove, erase, purge, discard, trash, drop, or get rid of stored content. Take something out of the knowledge graph permanently.",
        tools: &["delete_node", "get_node", "search_semantic", "search_nodes"],
        max_iterations: 3,
        exclusion: None,
        body: include_str!("seeds/skills/node-deletion.md"),
    },
    SkillSeed {
        // Conflict Journal
        //
        // Covers the read tools (`list_conflicts`, `get_conflict`) and the two
        // non-destructive resolution actions (`dismiss_conflict`,
        // `adopt_existing_conflict`). `merge_conflict` is deliberately NOT
        // whitelisted here — it archives a node and re-points its edges, the same
        // "cannot be undone" shape as `delete_node`, so it stays single-owner (see
        // `SINGLE_OWNER_BY_DESIGN` in this module's tests) rather than being offered
        // alongside the lower-stakes actions in this skill.
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c07",
        title: "Conflict Journal",
        // No form of "resolve" in the title or description. Those two
        // are what gets embedded (the guidance markdown is not), and
        // the shared word made this skill the rank-1
        // attractor for any request mentioning "resolved": "delete
        // the resolved incidents" ranked it above Node Deletion
        // (0.978 vs 0.904) on the locked embedding model, and since
        // `delete_node` is offered only from the top tool-bearing
        // candidate, the deletion was silently withheld. Rewording
        // the description alone never fixed it — the title "Conflict
        // Resolution" carried the pull by itself. Guarded in
        // `tests/it/live_skill_retrieval_stability.rs` by
        // `deletion_requests_mentioning_resolved_route_node_deletion`
        // and `control_conflict_requests_still_route_conflict_journal`.
        description: "List, inspect, or dismiss conflicts between colliding nodes recorded in the conflict journal: two records that claim the same identity, duplicates, or sync collisions.",
        tools: &["list_conflicts", "get_conflict", "dismiss_conflict", "adopt_existing_conflict", "search_nodes"],
        max_iterations: 3,
        exclusion: None,
        body: include_str!("seeds/skills/conflict-journal.md"),
    },
    SkillSeed {
        // Node Merge
        //
        // The body of the Node Merge skill, including the shared
        // success-means-stop rule.
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c08",
        title: "Node Merge",
        // `merge_conflict` is destructive (archives the loser node,
        // re-points its edges) the same way delete_node is, so this
        // skill is single-owner by the same ADR-038 reasoning
        // (`SINGLE_OWNER_BY_DESIGN` in this module's tests) rather
        // than being folded into Conflict Journal's lower-stakes
        // whitelist.
        description: "Merge two nodes that both represent the same real thing into one, combining their data and archiving the loser. Use when the user wants two duplicate or colliding records combined into a single record.",
        tools: &["merge_conflict", "dismiss_conflict", "adopt_existing_conflict", "get_conflict", "get_node", "search_nodes"],
        max_iterations: 3,
        exclusion: None,
        body: include_str!("seeds/skills/node-merge.md"),
    },
    SkillSeed {
        // Play Workflow State
        //
        // Covers only `get_workflow_state` — the one Play/Playbook operation that
        // needs its own tool (list/logs/enable/disable all reduce to `search_nodes`/
        // `update_node`, which the model already reaches through other skills, per
        // ADR-035's capability-parity clause).
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c09",
        title: "Play Workflow State",
        description: "Check why a Play automation rule hasn't fired for a node, or what conditions are still unmet, by evaluating that node against every active Play rule that could apply to it. Use when the user asks why an automation, rule, or workflow hasn't triggered, or wants to know what's missing before it will.",
        tools: &["get_workflow_state", "search_semantic", "search_nodes"],
        max_iterations: 3,
        exclusion: None,
        body: include_str!("seeds/skills/play-workflow-state.md"),
    },
    SkillSeed {
        // Bulk Import
        //
        // The body of the Bulk Import skill, including the shared
        // no-followup-search success rule.
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c0a",
        title: "Bulk Import",
        description: "Import documents and create node hierarchies from markdown. Use when user wants to import, bulk create, or create nodes from a markdown document.",
        tools: &["create_nodes_from_markdown"],
        max_iterations: 2,
        exclusion: None,
        body: include_str!("seeds/skills/bulk-import.md"),
    },
    SkillSeed {
        // Organization
        //
        // The body of the Organization skill, including the shared
        // find-then-act, collection-at-create-time, and success-means-stop rules.
        id: "3e9a7c14-5d28-4b61-8f0c-6a2d9e4b7c0b",
        title: "Organization",
        description: "Organize nodes into collections and categories. Use when user wants to add to a collection, categorize, or group nodes.",
        tools: &["create_relationship", "get_node", "search_semantic", "search_nodes"],
        max_iterations: 3,
        exclusion: None,
        body: include_str!("seeds/skills/organization.md"),
    },
];

/// `SCHEMA_RULES` entries deliberately NOT included in the Schema Creation
/// skill's body — the in-app agent prompt — with the reason recorded inline.
///
/// A rule can be registered in [`crate::skill_rules::SCHEMA_RULES`] and
/// rendered into the shipped `SKILL.md` prose without ever reaching this
/// function, meaning the in-app local agent never sees it even though the
/// external shell agent does. That happened once for real (`DELETE_A_SCHEMA`
/// during review of the PR that added it) before anything caught it.
///
/// `schema_rules_are_wired_or_explicitly_excluded` (below) checks every
/// `SCHEMA_RULES` entry against the Schema Creation skill's body and
/// requires each miss to be named here. A hand-maintained exclusion list is
/// itself a drift source, but the alternative already produced a shipped
/// defect: an omission from this list is visible in review, where an
/// omission from the body alone was not.
///
/// All five entries below are field/enum **shape** rules that ADR-064 says
/// belong on `create_schema`'s own tool-schema descriptions
/// (`local_agent/tools.rs`) instead of the prompt, to stop the schema and the
/// prompt from being two independently-maintained copies of the same rule
/// (the `coreValues`/`core_values` casing contradiction ADR-064 records is
/// exactly that drift). The model reads the tool schema immediately before
/// calling `create_schema`, so shape rules belong there rather than in prose
/// interpolated earlier in the prompt. All five now have that tool-schema
/// coverage: `enum-edge-fields`'s content (coreValues, closed vocabulary,
/// reserved relationship names) lives on the `edgeFields` property of
/// `relationships` items in both `def_create_schema()` and
/// `def_update_schema()`.
#[cfg(test)]
const SCHEMA_RULES_NOT_IN_PROMPT: &[&str] = &[
    // Field-naming shape: stated on create_schema's own field description.
    "no-name-title-field",
    // Field-source discipline: not an argument shape, but paired here with
    // the other four per ADR-064's "argument shape lives on the tool schema"
    // grouping rather than getting a sixth, one-off list.
    "fields-from-request-only",
    // title_template/field pairing: stated on create_schema's title_template
    // description.
    "name-placeholder-exception",
    // Enum value/label casing: stated on create_schema's enum field
    // description.
    "enum-format",
    // Edge field shape (coreValues, closed vocabulary, reserved relationship
    // names): stated on the `edgeFields` property of `relationships` items in
    // both create_schema's and update_schema's tool schema
    // (packages/agent/src/local_agent/tools.rs).
    "enum-edge-fields",
];

/// The built-in skill seeds, one per row of [`SKILL_SEEDS`], in table order.
pub fn seed_skill_nodes() -> Vec<NodeTemplate> {
    SKILL_SEEDS.iter().map(SkillSeed::template).collect()
}

/// The built-in tool seeds, one per [`crate::local_agent::tools::Tool`], in
/// registry order.
///
/// Each template produces one `tool-native` node bridging graph storage to a
/// deterministic Rust handler, under the tool's fixed id. Fields:
/// - `handler`: stable key into the handler registry (matches `Tool::name()`)
/// - `description`: embedded for semantic tool discovery
/// - `parameter_schema`: typed JSON Schema the model uses when calling the tool
/// - `enabled`: `true`; a native tool is offered whatever it says
pub fn seed_tool_nodes() -> Vec<NodeTemplate> {
    use crate::local_agent::tools::Tool;
    Tool::ALL
        .iter()
        .map(|tool| {
            let def = tool.definition();
            NodeTemplate {
                id: tool.seed_id().to_string(),
                title: def.name.clone(),
                root_node_type: CoreNodeType::ToolNative.as_str().to_string(),
                root_properties: serde_json::json!({
                    "handler": def.name,
                    "description": def.description,
                    "parameter_schema": def.parameters_schema,
                    "enabled": true,
                }),
                child_node_type: None,
                tier: SeedTier::System,
                markdown_content: String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use nodespace_core::markdown::prepare_nodes_from_template;

    use super::*;

    fn tmpl_skill(tmpl: &NodeTemplate) -> SkillFields {
        SkillFields::from_properties(&tmpl.root_properties)
            .unwrap_or_else(|e| panic!("seed '{}' must decode as a skill: {e}", tmpl.title))
    }

    fn tmpl_tool_whitelist(tmpl: &NodeTemplate) -> Vec<String> {
        tmpl_skill(tmpl).tool_whitelist
    }

    /// The seeded body of the skill titled `title`, rule includes resolved.
    fn skill_body(title: &str) -> String {
        seed_skill_nodes()
            .into_iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("no seeded skill titled {title:?}"))
            .markdown_content
    }

    /// A skill body is the file's text: plain Markdown with no frontmatter,
    /// and every rule it includes is a real one.
    #[test]
    fn skill_bodies_are_plain_markdown_files_whose_includes_resolve() {
        for seed in SKILL_SEEDS {
            assert!(
                seed.body.starts_with("# "),
                "{} must open with its heading, not frontmatter",
                seed.title
            );
            // Panics on an include that names no rule.
            let resolved = seed.template().markdown_content;
            assert!(
                !resolved.contains("<!--"),
                "{} has an unresolved include marker",
                seed.title
            );
        }
    }

    /// Every `SCHEMA_RULES` entry must either reach the in-app agent prompt
    /// (the Schema Creation skill's body) or be named in
    /// [`SCHEMA_RULES_NOT_IN_PROMPT`] with a reason.
    ///
    /// This is a distinct guard from
    /// `packages/cli/tests/it/skill_md_generation.rs`'s
    /// `every_schema_rule_reaches_the_skill`, which only checks that
    /// `rule.prose` reaches the shipped `SKILL.md` — the external shell
    /// agent's surface. Neither that test nor the `prompt_assembly_snapshot`
    /// golden can catch a rule that never reaches that body: an unwired
    /// rule never enters the rendered prompt, so the golden never changes,
    /// and the SKILL.md test only ever looks at the other renderer's output.
    /// `DELETE_A_SCHEMA` shipped exactly that gap once for real before this
    /// test existed — registered, rendered into SKILL.md, and never
    /// included there, so the in-app local agent (holding both
    /// `create_schema` and `delete_node`) never saw it.
    #[test]
    fn schema_rules_are_wired_or_explicitly_excluded() {
        let prompt = skill_body("Schema Creation");

        let mut unaccounted = Vec::new();
        let mut wrongly_excluded = Vec::new();

        for rule in crate::skill_rules::SCHEMA_RULES {
            let wired = prompt.contains(rule.imperative);
            let excluded = SCHEMA_RULES_NOT_IN_PROMPT.contains(&rule.id);

            if wired && excluded {
                // A rule can't be both interpolated into the prompt and
                // recorded as prose-only — the exclusion list would be
                // actively wrong, not merely stale.
                wrongly_excluded.push(rule.id);
            } else if !wired && !excluded {
                unaccounted.push(rule.id);
            }
        }

        assert!(
            wrongly_excluded.is_empty(),
            "these SCHEMA_RULES entries are BOTH included in \
             seeds/skills/schema-creation.md AND listed in SCHEMA_RULES_NOT_IN_PROMPT: {}. \
             Remove them from SCHEMA_RULES_NOT_IN_PROMPT — it must only name rules that \
             are prose-only by design.",
            wrongly_excluded.join(", ")
        );

        assert!(
            unaccounted.is_empty(),
            "these SCHEMA_RULES entries are neither included in \
             seeds/skills/schema-creation.md (the in-app agent prompt) nor listed in \
             SCHEMA_RULES_NOT_IN_PROMPT (packages/agent/src/skill_pipeline.rs): {}. \
             Either include each one in that body, or — \
             if the omission is deliberate, e.g. a field/enum shape rule that belongs on \
             create_schema's own tool schema per ADR-064 — add it to \
             SCHEMA_RULES_NOT_IN_PROMPT with a comment explaining why.",
            unaccounted.join(", ")
        );
    }

    #[test]
    fn seed_skills_have_valid_properties() {
        let seeds = seed_skill_nodes();
        assert_eq!(seeds.len(), 11, "Should have 11 seed skills");

        for seed in &seeds {
            assert!(!seed.title.is_empty());
            // Decoding also rejects a non-positive max_iterations.
            let skill = tmpl_skill(seed);
            assert!(
                !skill.description.is_empty(),
                "Skill '{}' must have a non-empty description",
                seed.title
            );
            assert!(
                !skill.tool_whitelist.is_empty(),
                "Skill '{}' must have tools",
                seed.title
            );
            assert!(
                !seed.markdown_content.is_empty(),
                "Skill '{}' must have non-empty markdown_content (instructions for the model)",
                seed.title
            );
        }
    }

    /// The Node Creation skill must tell the model what to do with a user value
    /// that no listed schema field covers.
    ///
    /// Without this, the guidance only ever covered values that MATCH a listed
    /// field, and a supplied particular with no home was silently discarded
    /// while the user was told the record saved — the agent-matrix scenario-4
    /// failure ("replacement cost 2400" against a schema with no cost field,
    /// where the model replied that the schema "does not currently support
    /// logging replacement costs" and persisted zero properties). Storage
    /// accepts undeclared keys (proved by
    /// `core/tests/it/create_node_property_persistence_test.rs`), so this gap was
    /// purely in what the model was told.
    #[test]
    fn node_creation_guidance_covers_values_with_no_matching_field() {
        let seeds = seed_skill_nodes();
        let node_creation = seeds
            .iter()
            .find(|s| s.title == "Node Creation")
            .expect("Node Creation skill must exist");
        let md = &node_creation.markdown_content;

        assert!(
            md.contains("VALUES WITH NO MATCHING FIELD"),
            "Node Creation guidance must address values the listed fields don't cover"
        );
        // The two failure modes actually observed, both named explicitly so a
        // future reword that drops either one fails here rather than in an eval.
        assert!(
            md.contains("NEVER drop a value"),
            "guidance must forbid discarding an unmatched value"
        );
        // Pins the RULE, not the quoted symptom: a reword that keeps the
        // prohibition but drops the exact phrase should still pass.
        assert!(
            md.contains("NEVER drop a value") && md.contains("gone silently"),
            "guidance must forbid discarding a value and say why it is harmful"
        );
        // The value belongs in THIS call, not behind a schema round-trip that
        // the skill's own tool whitelist cannot make anyway.
        assert!(
            md.contains("Do NOT call create_schema or update_schema"),
            "guidance must route the value into this create_node call"
        );
        let whitelist = tmpl_tool_whitelist(node_creation);
        assert!(
            !whitelist.contains(&"create_schema".to_string())
                && !whitelist.contains(&"update_schema".to_string()),
            "whitelist must not offer a schema escape hatch the guidance forbids"
        );
    }

    /// The Relationship Management and Organization guidance
    /// taught `relation_type` at three call sites while `create_relationship`
    /// and `get_related_nodes` both require `relationship_type` -- a model
    /// that trusted the guidance it was just shown got rejected outright by
    /// `#[serde(deny_unknown_fields)]`. Pins BOTH sides of the same rule
    /// `both_surfaces_agree_on_the_undeclared_key_rule` above pins for Node
    /// Creation: the guidance text names the same parameter the tool schema
    /// actually requires, so a future reword of either side that
    /// reintroduces the mismatch fails here instead of in an eval.
    #[test]
    fn relationship_guidance_teaches_the_real_parameter_name() {
        let seeds = seed_skill_nodes();
        let relationship_md = &seeds
            .iter()
            .find(|s| s.title == "Relationship Management")
            .expect("Relationship Management skill must exist")
            .markdown_content;
        let organization_md = &seeds
            .iter()
            .find(|s| s.title == "Organization")
            .expect("Organization skill must exist")
            .markdown_content;

        let create_relationship_schema = crate::local_agent::tools::Tool::CreateRelationship
            .definition()
            .parameters_schema;
        let required = create_relationship_schema["required"]
            .as_array()
            .expect("create_relationship must declare required parameters");
        let real_param_name = required
            .iter()
            .filter_map(|v| v.as_str())
            .find(|s| s.contains("relationship"))
            .expect("create_relationship's required list must name the relationship-type param");

        for (skill_name, md) in [
            ("Relationship Management", relationship_md),
            ("Organization", organization_md),
        ] {
            assert!(
                md.contains(real_param_name),
                "{skill_name} guidance must teach the tool's actual parameter name \
                 ({real_param_name:?}), got: {md}"
            );
            assert!(
                !md.contains("relation_type"),
                "{skill_name} guidance must not teach `relation_type` -- \
                 create_relationship/get_related_nodes require `relationship_type` \
                 and reject unknown fields, got: {md}"
            );
        }
    }

    /// The same rule must hold on the tool schema itself, which is what the
    /// model sees when routing lands on a skill other than Node Creation — the
    /// scenario-4 traces show routing varying across arms while the empty-call
    /// outcome stayed identical, so guidance alone would leave the gap open on
    /// exactly the paths that were failing.
    #[test]
    fn create_node_tool_description_admits_keys_beyond_listed_fields() {
        let def = crate::local_agent::tools::Tool::CreateNode.definition();
        let props = def.parameters_schema["properties"]["field_values"]["description"]
            .as_str()
            .expect("field_values field must document itself");
        assert!(
            props.contains("Not limited to the listed fields"),
            "create_node must tell the model extra keys are allowed, got: {props}"
        );
    }

    /// Both surfaces state the SAME rule, so neither can be reworded into
    /// contradicting the other while its own test stays green.
    ///
    /// The rule is duplicated deliberately — routing does not always land on
    /// Node Creation, and on those turns the tool schema is the only surface
    /// carrying it — but deliberate duplication silently becomes divergence
    /// without something pinning the two together.
    ///
    /// The invariant pinned is ADR-063's: a value with no matching field is
    /// keyed bare on a user-defined type, and `custom:`-prefixed on a core
    /// type, where unprefixed names are reserved. Getting this wrong writes a
    /// bare key onto a core type — the collision `update_schema` rejects, and
    /// which `create_node` does NOT currently validate.
    #[test]
    fn both_surfaces_agree_on_the_undeclared_key_rule() {
        let node_creation_md = seed_skill_nodes()
            .into_iter()
            .find(|s| s.title == "Node Creation")
            .expect("Node Creation skill must exist")
            .markdown_content;
        let tool_desc = crate::local_agent::tools::Tool::CreateNode
            .definition()
            .parameters_schema["properties"]["field_values"]["description"]
            .as_str()
            .expect("field_values field must document itself")
            .to_string();

        for (surface, text) in [
            ("skill guidance", &node_creation_md),
            ("create_node tool schema", &tool_desc),
        ] {
            // Assert the PAIRING, not the mere presence of "custom:" — the
            // token appears in the worked example too, so a surface that
            // dropped the rule while keeping the example would still pass a
            // bare `contains("custom:")`. Verified by mutation: flipping the
            // tool schema to "bare on a built-in type" must fail this.
            let states_prefix_rule = text.contains("`custom:`-prefixed on a built-in type")
                || text.contains("prefix with `custom:`");
            assert!(
                states_prefix_rule,
                "{surface} must tie the custom: prefix to built-in types, not merely mention it"
            );
            for core_type in ["text", "task", "date"] {
                assert!(
                    text.contains(core_type),
                    "{surface} must name the core type '{core_type}' the prefix rule applies to"
                );
            }
            assert!(
                text.contains("reserved"),
                "{surface} must say why bare keys on core types are disallowed"
            );
        }
    }

    #[test]
    fn seed_skill_template_produces_skill_node() {
        let seeds = seed_skill_nodes();
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
        }
    }

    /// Skill guidance children must come out as real markdown types (`header`,
    /// `text`, ...), not the retired `prompt` type — the entire point of
    /// `child_node_type: None` on every seed. Every seed's `markdown_content`
    /// starts with a `# Heading` line, so this also confirms `header` nodes
    /// are actually produced, not just `text`.
    #[test]
    fn seed_skill_children_are_real_markdown_types_not_prompt() {
        let seeds = seed_skill_nodes();
        for seed in &seeds {
            let nodes = prepare_nodes_from_template(seed)
                .unwrap_or_else(|e| panic!("Template '{}' failed: {:?}", seed.title, e));
            assert!(
                nodes.len() > 1,
                "Skill '{}' must have guidance children, not just the root",
                seed.title
            );

            let children = &nodes[1..];
            assert!(
                children.iter().any(|c| c.node_type == "header"),
                "Skill '{}' guidance starts with a markdown heading and must \
                 produce at least one 'header' child",
                seed.title
            );
            for child in children {
                assert_ne!(
                    child.node_type, "prompt",
                    "Skill '{}' child must not be typed 'prompt' — that type is retired; \
                     children must be ordinary markdown types",
                    seed.title
                );
                assert!(
                    matches!(child.node_type.as_str(), "header" | "text" | "code-block"),
                    "Skill '{}' child has unexpected node_type '{}' — expected 'header', \
                     'text', or 'code-block'",
                    seed.title,
                    child.node_type
                );
            }
        }
    }

    /// Skills whose guidance carries a worked JSON example must produce real
    /// `code-block` children, not JSON flattened into a prose `text` node.
    ///
    /// The seeding pipeline is a genuine markdown import — `prepare_nodes_from_markdown`
    /// turns ` ``` ` fences into `code-block` nodes — so an unfenced JSON example
    /// silently degrades to paragraph text. Fencing is what makes these examples
    /// round-trip as structured markdown (`render_subtree_markdown` re-emits each
    /// node's content verbatim, fence markers included).
    ///
    /// Neither seed carries a multi-line JSON example any more: both moved
    /// their worked example onto their tool's own description
    /// (`create_node`/`create_schema` in `local_agent/tools.rs`), per
    /// ADR-064 rule 1 and Finding 2 — the model reads the tool schema right
    /// before calling, so that is where the pattern it reproduces should
    /// live, not duplicated into the skill's prose too. This test now pins
    /// the negative: a future edit that re-adds a fenced JSON block to
    /// either skill's markdown_content should be a deliberate, reviewed
    /// choice, not a silent reintroduction of the duplication ADR-064
    /// removed.
    #[test]
    fn seed_skill_json_examples_produce_code_block_children() {
        // Neither seed carries a multi-line JSON worked example any more —
        // see the moved-to-tool-description note above. The rest use inline
        // code spans for short call shapes, which stay in `text`.
        let expected: &[(&str, usize)] = &[("Node Creation", 0), ("Schema Creation", 0)];

        for (title, want_blocks) in expected {
            let seed = seed_skill_nodes()
                .into_iter()
                .find(|s| s.title == *title)
                .unwrap_or_else(|| panic!("No seeded skill titled '{title}'"));
            let nodes = prepare_nodes_from_template(&seed)
                .unwrap_or_else(|e| panic!("Template '{title}' failed: {e:?}"));

            let code_blocks: Vec<&str> = nodes[1..]
                .iter()
                .filter(|n| n.node_type == "code-block")
                .map(|n| n.content.as_str())
                .collect();

            assert_eq!(
                code_blocks.len(),
                *want_blocks,
                "Skill '{title}' should produce {want_blocks} 'code-block' child(ren) from its \
                 fenced JSON example(s), got {}. Unfenced JSON is parsed as prose instead.",
                code_blocks.len()
            );

            for block in &code_blocks {
                assert!(
                    block.starts_with("```json"),
                    "Skill '{title}' code-block should keep its ```json fence verbatim so it \
                     round-trips as markdown, got: {}",
                    block.chars().take(40).collect::<String>()
                );
                let body = block
                    .trim_start_matches("```json")
                    .trim_end_matches("```")
                    .trim();
                serde_json::from_str::<serde_json::Value>(body).unwrap_or_else(|e| {
                    panic!("Skill '{title}' fenced example is not valid JSON: {e}\n{body}")
                });
            }
        }
    }

    // -- Skill whitelist / registry validation ---------------------------

    /// Every tool named in a seed skill's `tool_whitelist` must resolve to a
    /// real entry in the tool registry. This is the registry-backed replacement
    /// for the former per-skill drift detectors: a typo or a reference to a
    /// removed tool fails here instead of silently producing a whitelist entry
    /// that can never match a dispatchable tool.
    #[test]
    fn all_skill_whitelist_tools_are_registered() {
        use crate::local_agent::tools::Tool;
        for seed in seed_skill_nodes() {
            for tool_name in tmpl_tool_whitelist(&seed) {
                assert!(
                    Tool::from_name(&tool_name).is_some(),
                    "Skill '{}' whitelists unknown tool '{}' — it is not in the tool registry \
                     (Tool enum). Fix the name or add the tool to the registry.",
                    seed.title,
                    tool_name
                );
            }
        }
    }

    /// Blast radius is derived from each seeded skill's `tool_whitelist`
    /// (ADR-038), so the derivation must agree with what the seeds actually
    /// declare. This pins the classification against the real seed data rather
    /// than a stub, and fails if a seed's whitelist gains or loses a write tool
    /// without that being an intentional change to its Stage-2 bar.
    ///
    /// Both rungs are pinned, not just the mutating one. `Node Deletion` is
    /// currently the only *destructive* seed, and that is a property of the
    /// seed data rather than of the registry: `removes_user_data` pins that
    /// `delete_node` is the only destructive tool, but nothing there stops a
    /// second seed from whitelisting it. Without the third column, adding
    /// `delete_node` to (say) `Organization` would silently move that skill to
    /// the strictest bar in the system — and change which candidate may
    /// contribute destructive tools at all — while every assertion here still
    /// passed, because `should_mutate` was already `true` for it.
    #[test]
    fn seeded_skills_classify_by_blast_radius_as_expected() {
        let expected_blast_radius = [
            // (title, mutating, destructive)
            ("Node Creation", true, false),
            ("Schema Creation", true, false),
            ("Graph Editing", true, false),
            ("Relationship Management", true, false),
            ("Node Deletion", true, true),
            ("Conflict Journal", true, false),
            ("Node Merge", true, true),
            ("Bulk Import", true, false),
            ("Organization", true, false),
            ("Research & Search", false, false),
        ];

        for (title, should_mutate, should_destroy) in expected_blast_radius {
            let seed = seed_skill_nodes()
                .into_iter()
                .find(|t| t.title == title)
                .unwrap_or_else(|| panic!("seed skill '{title}' must exist"));
            let tools = tmpl_tool_whitelist(&seed);
            // Exercise the production classifier rather than restating its
            // rule: a reimplementation here would keep passing while
            // `skill_is_mutating` regressed, which is the opposite of what
            // this test is for.
            let candidate = crate::agent_types::SkillCandidate {
                id: format!("seed-{title}"),
                name: title.to_string(),
                description: String::new(),
                score: 1.0,
                tools: tools.clone(),
                instructions: String::new(),
                schema_metadata: serde_json::json!([]),
            };
            let mutates = crate::local_agent::routing::skill_is_mutating(&candidate);
            assert_eq!(
                mutates, should_mutate,
                "skill '{title}' blast radius changed; whitelist is {tools:?}"
            );
            let destroys = crate::local_agent::routing::skill_is_destructive(&candidate);
            assert_eq!(
                destroys, should_destroy,
                "skill '{title}' gained or lost the ability to irreversibly remove user data; \
                 whitelist is {tools:?}. This changes its Stage-2 score bar AND whether it may \
                 contribute destructive tools when it is not the retrieved winner."
            );
        }
    }

    /// A skill's description is EMBEDDED for retrieval, and embeddings have no
    /// notion of negation: a clause saying "not to record, update, or mark"
    /// lands NEARER those intents, not further from them. A description may
    /// therefore contain only words for what the skill DOES.
    ///
    /// Regression test for a measured defect, not a style rule. `Node Deletion`
    /// carried exactly that disclaimer and was retrieved into the Stage-2 top-3
    /// on all 18 turns of a 6-rep run — including every turn that then failed
    /// for want of `update_node`. The user prompt was "mark it that way"; the
    /// word "mark" was in the disclaimer.
    ///
    /// Scoping rules belong in the instruction subtree (read as text), never in
    /// the description (embedded).
    #[test]
    fn no_seeded_skill_description_names_operations_it_excludes() {
        // A negation followed, within the same clause, by a verb belonging to a
        // different operation. Matching the PAIR rather than the negation alone
        // keeps this on the failure mode that was actually measured.
        let negations = [
            "not to ",
            "not for ",
            "never ",
            "rather than ",
            "instead of ",
            "do not ",
            "don't ",
        ];
        let rival_verbs = [
            "record", "update", "mark", "look up", "create", "delete", "remove", "search", "edit",
            "modify",
        ];

        for tmpl in seed_skill_nodes() {
            let desc = tmpl_skill(&tmpl).description.to_lowercase();

            for neg in negations {
                let Some(at) = desc.find(neg) else { continue };
                let tail = &desc[at..];
                let clause = tail.split(['.', ';']).next().unwrap_or(tail);
                for verb in rival_verbs {
                    assert!(
                        !clause.contains(verb),
                        "skill {:?}'s description contains a negated capability clause \
                         ({neg:?} … {verb:?}): {clause:?}\n\
                         An embedding cannot represent \"not\" — naming an operation here pulls \
                         this skill TOWARD requests for it. Put the scoping rule in the skill's \
                         guidance subtree instead.",
                        tmpl.title,
                    );
                }
            }
        }
    }

    /// An exclusion misfiring is quieter than a missing tool — the right skill
    /// simply ranks lower — and wording that reads as equivalent was measured
    /// to behave very differently. So each one is a measured decision with its
    /// own live guard, and adding one to another skill must be deliberate:
    /// extend this list together with a test in
    /// `tests/it/live_skill_retrieval_stability.rs` showing it leaves that
    /// skill's intended requests unchanged.
    #[test]
    fn only_measured_skills_carry_an_exclusion() {
        let with_exclusion: Vec<String> = seed_skill_nodes()
            .into_iter()
            .filter(|t| tmpl_skill(t).exclusion.is_some())
            .map(|t| t.title)
            .collect();
        assert_eq!(with_exclusion, vec!["Graph Editing".to_string()]);
    }

    /// The verbs a real deletion request uses must all be present, since the
    /// description is the entire retrieval signal for this skill. Guards the
    /// opposite failure from the test above: a description narrowed so far that
    /// a genuine "get rid of this" no longer matches it.
    #[test]
    fn node_deletion_description_carries_the_destructive_verbs() {
        let desc = seed_skill_nodes()
            .into_iter()
            .find(|t| t.title == "Node Deletion")
            .map(|t| tmpl_skill(&t).description)
            .expect("Node Deletion must be seeded")
            .to_lowercase();

        for verb in [
            "delete",
            "remove",
            "erase",
            "purge",
            "discard",
            "trash",
            "get rid of",
        ] {
            assert!(
                desc.contains(verb),
                "Node Deletion's description is missing {verb:?}, a verb a real deletion \
                 request may use. Description: {desc:?}"
            );
        }
    }

    /// The words a real linking request uses must all be present, since the
    /// description is the entire retrieval signal for this skill. Regression
    /// test for a measured defect: the prior wording never used the words a
    /// user actually says for linking two things ("point at", "depends on",
    /// "must respect"), so it lost Stage-2 retrieval outright against a real
    /// linking prompt despite `create_relationship` being exactly the tool
    /// that turn needed.
    ///
    /// Deliberately does NOT assert generic "link"/"connect"/"associate"/
    /// "relate" verbs: an earlier draft carrying those cleared retrieval for
    /// this skill but crowded Organization out of ITS own top-3 (see
    /// `control_prompt_still_routes_organization_every_rep`), since
    /// Organization also whitelists `create_relationship` and its
    /// "categorize"/"group" language sits semantically adjacent to them.
    #[test]
    fn relationship_management_description_carries_the_linking_verbs() {
        let desc = seed_skill_nodes()
            .into_iter()
            .find(|t| t.title == "Relationship Management")
            .map(|t| tmpl_skill(&t).description)
            .expect("Relationship Management must be seeded")
            .to_lowercase();

        for phrase in ["point", "depends on", "must respect", "edge"] {
            assert!(
                desc.contains(phrase),
                "Relationship Management's description is missing {phrase:?}, wording a real \
                 linking request may use. Description: {desc:?}"
            );
        }
    }

    /// No write tool may be reachable from only one skill.
    ///
    /// With `RETRIEVAL_TOP_K = 3`, a single-sourced write tool makes its own
    /// availability a lottery: if its sole owner misses the window, the tool
    /// does not exist for that turn and no amount of model reasoning recovers
    /// it. Measured on the locked model, 6 reps of one seeded chain on a warm
    /// index: the write turn failed 3 times, each failure a turn where Graph
    /// Editing placed outside the top 3 and nothing else carried `update_node`.
    ///
    /// Read tools already satisfy this naturally (`get_node` is in seven
    /// skills, `search_nodes` and `search_semantic` in six). This pins the same
    /// property for writes so a future skill split cannot silently reintroduce
    /// the single point of failure.
    ///
    /// `delete_node` is the deliberate exception: `stage2_permitted_names`
    /// admits destructive tools ONLY from the retrieval winner, so a second
    /// owner would widen the surface without widening availability, and
    /// ADR-038 gates the irreversible error hardest.
    #[test]
    fn no_write_tool_is_reachable_from_only_one_skill() {
        use std::collections::HashMap;

        let mut owners: HashMap<String, Vec<String>> = HashMap::new();
        for tmpl in seed_skill_nodes() {
            for tool in tmpl_tool_whitelist(&tmpl) {
                owners.entry(tool).or_default().push(tmpl.title.clone());
            }
        }

        // Tools whose single owner is a deliberate, reasoned exception.
        //
        // `delete_node`: `stage2_permitted_names` admits destructive tools ONLY
        // from the retrieval winner, so a second owner would widen the surface
        // without widening availability. ADR-038 gates the irreversible error
        // hardest, and that outranks convenience here.
        //
        // `create_schema` / `update_schema`: defining or altering a TYPE is a
        // genuinely distinct intent from writing an instance of one, and the
        // guidance for it is long and specific (enum shapes, title templates,
        // relationship-vs-field). Duplicating that whitelist onto an
        // instance-writing skill would offer the type-editing tools on ordinary
        // record writes — the same over-broad-surface failure as a deletion
        // skill being retrieved for a creation request, in the other direction.
        // Schema Creation also retrieves reliably on "I want to track X"
        // phrasing, which is what actually precedes these calls.
        //
        // `create_nodes_from_markdown`: Bulk Import's whitelist is exactly this
        // one tool, and the intent ("import this document") is lexically
        // unmistakable rather than an inflection of another request.
        //
        // `merge_conflict`: the same reasoning as `delete_node` — it archives
        // a node and re-points its edges, an irreversible-feeling structural
        // change. `stage2_permitted_names`-style destructive gating wants this
        // reachable from exactly one retrieval winner (Node Merge), not
        // offered alongside Conflict Journal's lower-stakes
        // dismiss/adopt-existing actions.
        const SINGLE_OWNER_BY_DESIGN: &[&str] = &[
            "delete_node",
            "create_schema",
            "update_schema",
            "create_nodes_from_markdown",
            "merge_conflict",
        ];

        // Enumerated from the REGISTRY, not from `owners`. Iterating the map
        // only sees tools some whitelist already mentions, so a write tool
        // removed from every whitelist has zero entries, never appears, and the
        // check meant to catch exactly that passes green — the unreachable case
        // being strictly worse than the single-owner case this test is named
        // for.
        let mut offenders: Vec<String> = crate::local_agent::tools::Tool::ALL
            .iter()
            .filter(|tool| tool.is_write())
            .map(|tool| tool.name())
            .filter(|name| !SINGLE_OWNER_BY_DESIGN.contains(name))
            .filter_map(|name| {
                let skills = owners.get(name).map(Vec::as_slice).unwrap_or_default();
                (skills.len() < 2).then(|| {
                    if skills.is_empty() {
                        format!("{name} (reachable from NO skill)")
                    } else {
                        format!("{name} (only {skills:?})")
                    }
                })
            })
            .collect();
        offenders.sort();

        assert!(
            offenders.is_empty(),
            "these write tools are reachable from fewer than two skills: {}\n\
             With RETRIEVAL_TOP_K = 3 that makes availability a lottery — if the sole owner \
             misses the window the tool does not exist for that turn, whatever the model does. \
             A tool reachable from NO skill is unavailable on every turn. \
             Give each a second plausible owner, or add it to SINGLE_OWNER_BY_DESIGN with the \
             reason.",
            offenders.join(", ")
        );
    }

    /// Pairing a write tool onto a second skill only removes the single point
    /// of failure if the skill's GUIDANCE also says when to reach for it.
    /// Whitelisting alone hands the model two write verbs with no rule for
    /// choosing, and the wrong choice is not a no-op: `create_node` where
    /// `update_node` was meant silently duplicates the user's data, and
    /// `update_node` where `create_node` was meant needs an id that does not
    /// exist.
    ///
    /// Pinned as a property rather than fixed once, because the fix was applied
    /// asymmetrically the first time — Node Creation got a disambiguation
    /// clause, Graph Editing got the tool and no clause at all.
    #[test]
    fn a_skill_carrying_both_write_verbs_disambiguates_them_in_its_guidance() {
        for tmpl in seed_skill_nodes() {
            let tools = tmpl_tool_whitelist(&tmpl);
            let carries = |name: &str| tools.iter().any(|t| t == name);
            if !(carries("create_node") && carries("update_node")) {
                continue;
            }
            for tool in ["create_node", "update_node"] {
                assert!(
                    tmpl.markdown_content.contains(tool),
                    "skill {:?} whitelists both create_node and update_node but its guidance \
                     never names {tool}. A model handed two write verbs with no rule for \
                     choosing picks one by phrasing, and the wrong pick either duplicates the \
                     user's record or writes to an id that does not exist.",
                    tmpl.title,
                );
            }
        }
    }

    /// `update_task_status` is the dedicated verb for a task's status, and a
    /// skill offering it alongside `update_node` has to say so — otherwise the
    /// model reaches for `field_values` and the status silently does not move.
    #[test]
    fn a_skill_carrying_update_task_status_names_it_in_its_guidance() {
        for tmpl in seed_skill_nodes() {
            if !tmpl_tool_whitelist(&tmpl)
                .iter()
                .any(|t| t == "update_task_status")
            {
                continue;
            }
            assert!(
                tmpl.markdown_content.contains("update_task_status"),
                "skill {:?} whitelists update_task_status but never names it in its guidance; \
                 the model will set status through update_node's field_values instead, which \
                 does not move a task's status.",
                tmpl.title,
            );
        }
    }

    #[test]
    fn update_schema_is_reachable_via_search_skills() {
        let seeds = seed_skill_nodes();
        let schema_skill = seeds
            .iter()
            .find(|s| s.title == "Schema Creation")
            .expect("Schema Creation skill must exist");
        assert!(
            tmpl_tool_whitelist(schema_skill).contains(&"update_schema".to_string()),
            "update_schema must be in Schema Creation tool_whitelist so the model can reach it"
        );
    }

    /// `UNIQUE_FIELD_FLAGS` reaches the Schema Creation body through an
    /// include marker — a future edit to that file could silently drop the
    /// `unique-field-flags` include with no compiler
    /// error, since the value is still a valid `String` either way. The
    /// `SCHEMA_RULES` structural tests (`no_rule_text_is_empty` etc.) only
    /// check the constant in isolation, not that it actually reaches the
    /// seeded markdown a model sees — this pins the advisory-only claim
    /// specifically, since a model that thinks `unique` is an enforced
    /// constraint could tell a user it will block duplicates, which is false
    /// (see `NodeService::find_duplicate_for`).
    ///
    /// The worked example demonstrating a unique-flagged field lives on
    /// `create_schema`'s own tool description now, per ADR-064 rule 1 and
    /// Finding 2 (the example the model reads right before calling is the
    /// one it reproduces) — checked below by
    /// `create_schema_tool_description_keeps_dev_domain_worked_example`, not
    /// here.
    #[test]
    fn schema_creation_guidance_covers_unique_field_flags() {
        let seeds = seed_skill_nodes();
        let schema_skill = seeds
            .iter()
            .find(|s| s.title == "Schema Creation")
            .expect("Schema Creation skill must exist");
        let md = &schema_skill.markdown_content;

        assert!(
            md.contains("uniqueCaseInsensitive"),
            "Schema Creation guidance must mention uniqueCaseInsensitive"
        );
        assert!(
            md.to_lowercase().contains("advisory only"),
            "Schema Creation guidance must state the unique flag is advisory only, \
             not an enforced constraint"
        );
    }

    /// `ADD_ENUM_VALUES` is interpolated through a format-string placeholder
    /// that a future edit can drop with no compiler error, exactly like the
    /// rules pinned above.
    ///
    /// The discrimination is what's load-bearing, not the mention: `add_fields`
    /// is the operation an agent already knows, and reaching for it here
    /// declares a redundant second field instead of extending the vocabulary
    /// the user asked about. So this pins that the guidance names
    /// `add_field_values`, names `add_fields` as the wrong choice, and states
    /// the `extensible`/`enum` gate — a model told only "there is an
    /// add_field_values" would still guess wrong about which fields accept it.
    #[test]
    fn schema_creation_guidance_covers_add_field_values() {
        let seeds = seed_skill_nodes();
        let schema_skill = seeds
            .iter()
            .find(|s| s.title == "Schema Creation")
            .expect("Schema Creation skill must exist");
        let md = &schema_skill.markdown_content;

        assert!(
            md.contains("add_field_values"),
            "Schema Creation guidance must name the add_field_values operation"
        );
        assert!(
            md.contains("NOT add_fields"),
            "Schema Creation guidance must steer away from add_fields — it is the \
             operation an agent reaches for by default, and it silently declares a \
             new field instead of extending the existing one"
        );
        assert!(
            md.contains("extensible: true"),
            "Schema Creation guidance must state the extensible gate so the model \
             checks eligibility rather than discovering it through a rejection"
        );
    }

    /// `TITLE_TEMPLATE_PLACEHOLDERS` is interpolated through a format-string
    /// placeholder, exactly like the rules pinned above — a future edit can
    /// drop the `{title_template_placeholders}` slot with no compiler error.
    ///
    /// The premise is what's load-bearing here, not the mention: a real
    /// session misread the original wording ("identity comes from its fields
    /// rather than free-form content") as a structured-data-vs-prose
    /// contrast, so it set a `title_template` on a single-field identity
    /// (producing a redundant duplicate field) and hand-assembled a title for
    /// a genuinely composed identity instead of templating it. This pins
    /// that the guidance states `content` IS the name for entity types
    /// (the missing premise), that `title_template` is for assembling from
    /// two or more fields (not any field-derived identity), and that a
    /// single-field identity goes directly into `content` with no template
    /// and no duplicate field — the three-way discrimination a model needs
    /// to avoid both failure directions.
    #[test]
    fn schema_creation_guidance_covers_title_template_discrimination() {
        let seeds = seed_skill_nodes();
        let schema_skill = seeds
            .iter()
            .find(|s| s.title == "Schema Creation")
            .expect("Schema Creation skill must exist");
        let md = &schema_skill.markdown_content;

        assert!(
            md.contains("content is a node's name for entity types"),
            "Schema Creation guidance must state the premise that content is the \
             node's name for entity types — without it, the field-vs-content \
             distinction reads as structured-data-vs-prose instead of \
             one-field-vs-several"
        );
        assert!(
            md.contains("ASSEMBLE a title from two or more fields"),
            "Schema Creation guidance must state that title_template exists to \
             assemble a title from 2+ fields, not merely that identity \
             'comes from fields'"
        );
        assert!(
            md.contains("do NOT set title_template") && md.contains("do NOT add a separate field"),
            "Schema Creation guidance must state that a single-field identity \
             goes directly into content, with no template and no duplicate field"
        );
        assert!(
            md.contains("markdown primitives"),
            "Schema Creation guidance must name markdown primitives as the case \
             where content IS prose, not a name — the exception that keeps the \
             premise from over-generalizing to every node type"
        );
    }

    /// `GROUPING_IS_COLLECTIONS` and `COLLECTION_AT_CREATE_TIME` are
    /// interpolated through format-string placeholders, which a future edit
    /// can drop without any compiler error — the value is a valid `String`
    /// either way, so an unwired rule becomes text the local agent never
    /// sees. Registration in `SCHEMA_RULES`/`INTERACTION_RULES` does not
    /// imply the rule reached the seeded markdown; these pin that it did.
    ///
    /// Both claims are load-bearing rather than decorative. An agent that
    /// still prices a collection as a lookup-create-link sequence picks a
    /// `tags` array over the built-in grouping mechanism, which is the exact
    /// failure the guidance exists to prevent.
    #[test]
    fn schema_creation_guidance_steers_grouping_to_collections() {
        let seeds = seed_skill_nodes();
        let schema_skill = seeds
            .iter()
            .find(|s| s.title == "Schema Creation")
            .expect("Schema Creation skill must exist");
        let md = &schema_skill.markdown_content;

        assert!(
            md.contains("tags"),
            "Schema Creation guidance must name the tags-field false friend"
        );
        assert!(
            md.contains("--collection"),
            "Schema Creation guidance must show the one-call collection form,              not merely assert that collections are preferable"
        );
        assert!(
            md.contains("not more expensive to write than an array element"),
            "Schema Creation guidance must state the cost comparison — an agent              that thinks a collection costs more still reaches for an array"
        );
    }

    /// The Organization skill must teach collection assignment at create time
    /// rather than the lookup-then-link sequence, and must not tell the model
    /// to have the user pre-create a collection the path resolver creates.
    #[test]
    fn organization_guidance_leads_with_the_one_call_form() {
        let seeds = seed_skill_nodes();
        let org_skill = seeds
            .iter()
            .find(|s| s.title == "Organization")
            .expect("Organization skill must exist");
        let md = &org_skill.markdown_content;

        assert!(
            md.contains("create_node takes a collection path directly"),
            "Organization guidance must lead with collection-at-create-time"
        );
        assert!(
            !md.contains("ask the user to create it first"),
            "Organization guidance must not ask the user to pre-create a              collection that resolve_path creates automatically"
        );
    }

    /// A rule that tells the model to "call `foo_bar`" is only actionable if
    /// `foo_bar` is a tool the model actually has. This shipped wrong once:
    /// the `add_field_values` guidance told the model to check eligibility
    /// with `get_schema_definition`, which is the core/CLI-layer RPC name and
    /// not on the local agent's tool surface at all. Nothing caught it —
    /// every other guard checks that a rule *reaches* a prompt, not that what
    /// the rule says is true of the tools that prompt is paired with.
    ///
    /// Scoped to the explicit `call <name>` / `calling <name>` phrasing rather
    /// than every snake_case token, because the rules are dense with
    /// non-tool identifiers (`core_values`, `blocked_by`, `title_template`)
    /// that a blanket scan would flag. The narrow form is what a model reads
    /// as an instruction to emit a tool call, which is exactly the claim that
    /// has to be true.
    #[test]
    fn rules_only_tell_the_model_to_call_tools_that_exist() {
        let known: std::collections::HashSet<&str> = crate::local_agent::tools::Tool::ALL
            .iter()
            .map(|t| t.name())
            .collect();

        // Every rule text on both surfaces, not just the ones reaching the
        // prompt today — a rule moved into the prompt later carries its
        // tool names with it.
        let mut texts: Vec<(&str, &str)> = Vec::new();
        for r in crate::skill_rules::SCHEMA_RULES {
            texts.push((r.id, r.imperative));
        }
        for r in crate::skill_rules::INTERACTION_RULES {
            texts.push((r.id, r.imperative));
        }

        let mut bad: Vec<String> = Vec::new();
        for (id, text) in texts {
            let words: Vec<&str> = text.split_whitespace().collect();
            for pair in words.windows(2) {
                let verb = pair[0].trim_matches(|c: char| !c.is_alphanumeric());
                if !verb.eq_ignore_ascii_case("call") && !verb.eq_ignore_ascii_case("calling") {
                    continue;
                }
                // Tool names reach here bare, backticked, followed by an
                // argument list, or possessive ("get_node's"). Trim the
                // leading punctuation, then cut at the first character that
                // cannot appear in an identifier — which covers `(`, `'` and
                // anything else a future rewording introduces, rather than
                // enumerating separators one at a time.
                let candidate =
                    pair[1].trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_');
                let candidate = candidate
                    .find(|c: char| !c.is_alphanumeric() && c != '_')
                    .map_or(candidate, |end| &candidate[..end]);
                // Only underscored identifiers are tool-name shaped; this is
                // what keeps ordinary prose ("call the", "call is") out.
                if !candidate.contains('_') {
                    continue;
                }
                if !known.contains(candidate) {
                    bad.push(format!("{id} says to call {candidate:?}"));
                }
            }
        }

        assert!(
            bad.is_empty(),
            "these rules instruct the model to call tools that do not exist on the local \
             agent's surface: {}. A model told to call a missing tool either wastes a turn \
             on a hard error or treats the instruction as an unsatisfiable precondition and \
             declines the operation outright. Use a real name from Tool::ALL, or reword so \
             the rule does not name a tool call.",
            bad.join("; ")
        );
    }

    /// The prompt guidance tells the model to reach for `add_field_values`,
    /// but a model only emits a parameter its tool schema declares — so the
    /// guidance is inert unless `update_schema`'s schema actually advertises
    /// it. That declaration was missing entirely until the guidance landed,
    /// which is the exact failure this pins: the Schema Creation body and
    /// the tool schema are edited in different files, and a rule pointing at
    /// an undeclared parameter reads as working guidance right up until the
    /// model can't act on it.
    ///
    /// The prompt-assembly golden also covers this, but only as a byproduct
    /// of snapshotting the whole tool surface — a regeneration accepts any
    /// diff put in front of it, so it records the change rather than
    /// defending the property.
    #[test]
    fn update_schema_tool_schema_declares_add_field_values() {
        let params = crate::local_agent::tools::Tool::UpdateSchema
            .definition()
            .parameters_schema;
        let add_field_values = params
            .get("properties")
            .and_then(|p| p.get("add_field_values"))
            .expect(
                "update_schema's tool schema must declare add_field_values — without it the \
                 model is never told the parameter exists, and the prompt guidance steering \
                 it there cannot be acted on",
            );

        let item_props = add_field_values
            .get("items")
            .and_then(|i| i.get("properties"))
            .expect("add_field_values items must declare properties");
        for key in ["field", "values"] {
            assert!(
                item_props.get(key).is_some(),
                "add_field_values items must declare {key:?} — it is required by \
                 FieldValueAddition, which also denies unknown fields"
            );
        }

        let desc = add_field_values
            .get("description")
            .and_then(|d| d.as_str())
            .expect("add_field_values must carry a description");
        assert!(
            desc.contains("NOT add_fields"),
            "add_field_values' description must distinguish it from add_fields — that \
             confusion is the whole reason the parameter needs guidance, got: {desc:?}"
        );
        assert!(
            desc.contains("extensible: true"),
            "add_field_values' description must state the extensible gate, got: {desc:?}"
        );
    }

    /// The worked example that replaced Schema Creation's inline "EXAMPLE —
    /// Customer schema" block now lives on `create_schema`'s tool
    /// description (ADR-064 rule 1 / Finding 2). Pins its presence and its
    /// dev-workflow domain, so a future edit can't silently drop it or drift
    /// back to a business-domain example.
    #[test]
    fn create_schema_tool_description_keeps_dev_domain_worked_example() {
        let desc = crate::local_agent::tools::Tool::CreateSchema
            .definition()
            .description;
        assert!(
            desc.contains("Example call:") && desc.contains("\"coreValues\""),
            "create_schema's description must keep a worked example with a coreValues enum, got: {desc:?}"
        );
        assert!(
            desc.contains("Ticket") && desc.contains("ready_for_dev"),
            "create_schema's worked example must be dev-workflow domain (Ticket/status enum), got: {desc:?}"
        );
        for business_word in ["Invoice", "Customer", "Product"] {
            assert!(
                !desc.contains(business_word),
                "create_schema's description regressed to a business-domain example: found {business_word:?}"
            );
        }
    }

    /// Schema Creation's description must lead with the natural phrasing users
    /// actually use for wanting a new kind of thing tracked, not only technical
    /// schema vocabulary.
    ///
    /// Agent-matrix scenario 3 traced "I want to keep a record of the equipment
    /// my team checks out..." to semantic retrieval never surfacing this skill
    /// in the top-3 candidates — the description was a keyword list ("new
    /// type", "create schema") that this kind of request doesn't lexically
    /// match, so `create_schema` was unreachable and the model fell back to a
    /// bare `create_node` or split the request across two schemas trying to
    /// recover. This test can't validate retrieval outcome (that needs the
    /// real embedding model — see `bun run eval:agent`), but it pins the
    /// wording itself against an accidental future trim or revert, which
    /// would silently regress scenario 3 with no other signal in `cargo test`
    /// or `bun run test:all`.
    #[test]
    fn schema_creation_description_covers_natural_tracking_phrasing() {
        let seeds = seed_skill_nodes();
        let schema_skill = seeds
            .iter()
            .find(|s| s.title == "Schema Creation")
            .expect("Schema Creation skill must exist");
        let description = tmpl_skill(schema_skill).description;

        let natural_phrases = ["keep track of", "log", "maintain records for"];
        assert!(
            natural_phrases.iter().any(|p| description.contains(p)),
            "Schema Creation description must contain at least one natural-language \
             tracking phrase ({natural_phrases:?}) alongside the technical keyword list, \
             or semantic retrieval will miss requests phrased like \
             'I want to keep a record of X' (agent-matrix scenario 3). Got: {description:?}"
        );
    }

    // -- Tool node seeding tests --------------------------------

    #[test]
    fn seed_tool_nodes_covers_all_registered_tools() {
        use crate::local_agent::tools::Tool;
        let tool_seeds = seed_tool_nodes();
        assert_eq!(
            tool_seeds.len(),
            Tool::ALL.len(),
            "seed_tool_nodes() must produce one node per Tool::ALL entry"
        );
    }

    #[test]
    fn seed_tool_nodes_have_required_properties() {
        for seed in seed_tool_nodes() {
            assert_eq!(seed.root_node_type, "tool-native");

            // Flat, bare field names: the write path moves each into the
            // bucket of the schema that declares it, `tool` or `tool-native`.
            let ns = &seed.root_properties;

            let handler = ns.get("handler").and_then(|v| v.as_str()).unwrap_or("");
            assert!(
                !handler.is_empty(),
                "Tool '{}' must have a handler key",
                seed.title
            );

            assert!(
                ns.get("source").is_none(),
                "Tool '{}' must not carry a source: its type says where it comes from",
                seed.title
            );

            let enabled = ns.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
            assert!(
                enabled,
                "Built-in tool '{}' must be enabled=true",
                seed.title
            );

            assert!(
                ns.get("parameter_schema")
                    .map(|v| v.is_object())
                    .unwrap_or(false),
                "Tool '{}' must have a parameter_schema object",
                seed.title
            );

            let desc = ns.get("description").and_then(|v| v.as_str()).unwrap_or("");
            assert!(
                !desc.is_empty(),
                "Tool '{}' must have a non-empty description",
                seed.title
            );
        }
    }

    #[test]
    fn seed_tool_nodes_handler_keys_match_registry() {
        use crate::local_agent::tools::Tool;
        for seed in seed_tool_nodes() {
            let handler = seed
                .root_properties
                .get("handler")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert!(
                Tool::from_name(handler).is_some(),
                "Tool node '{}' has handler '{}' not in registry",
                seed.title,
                handler
            );
        }
    }

    #[test]
    fn seed_tool_nodes_produce_valid_prepared_nodes() {
        for seed in seed_tool_nodes() {
            let nodes = prepare_nodes_from_template(&seed)
                .unwrap_or_else(|e| panic!("Template '{}' failed: {:?}", seed.title, e));
            assert!(!nodes.is_empty(), "Tool '{}' produced no nodes", seed.title);
            let root = &nodes[0];
            assert_eq!(root.node_type, "tool-native");
        }
    }
}

#[cfg(test)]
mod full_seed_db_tests {
    use super::*;
    use crate::local_agent::tools::Tool;
    use crate::prompt_assembler::PromptAssembler;
    use nodespace_core::db::SqliteStore;
    use nodespace_core::markdown::prepare_nodes_from_template;
    use nodespace_core::services::NodeService;
    use std::sync::Arc;

    /// End-to-end regression for the missing-tools half of the seed bug.
    ///
    /// `seed_nodes_from_templates` inserts every template group with a single
    /// `?` error path: if any node fails validation, the whole remaining batch
    /// is aborted. Previously the `create_schema` tool node (the 6th tool) was
    /// rejected by the external-tool `parameter_schema` depth guard, which
    /// silently dropped it AND every tool seeded after it (update_schema,
    /// update_task_status, create_relationship, get_related_nodes,
    /// search_skills, delete_node, create_nodes_from_markdown). The model was
    /// then offered only the first 5 tools — no create_schema, no search_skills.
    ///
    /// This seeds exactly the way the daemon does (prompts + skills + tools in
    /// one batch) and asserts every registered tool lands in the DB.
    #[tokio::test]
    async fn full_daemon_seed_inserts_all_tool_nodes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db = tmp.path().join("t.db");
        let mut store = Arc::new(SqliteStore::new(db).await.unwrap());
        let ns = Arc::new(NodeService::new(&mut store).await.unwrap());

        let prompts = PromptAssembler::seed_agent_guidance_nodes();
        let skills = seed_skill_nodes();
        let tools = seed_tool_nodes();
        let mut groups = Vec::new();
        for t in prompts.iter().chain(skills.iter()).chain(tools.iter()) {
            groups.push(prepare_nodes_from_template(t).expect("template expands"));
        }
        ns.seed_nodes_from_templates(groups)
            .await
            .expect("full daemon seed must succeed (no tool dropped by validation)");

        let q = nodespace_core::ops::node_ops::query_nodes(
            &ns,
            nodespace_core::ops::node_ops::QueryNodesInput {
                node_type: Some("tool".to_string()),
                limit: Some(256),
                offset: None,
                collection_id: None,
                collection: None,
                filters: None,
            },
        )
        .await
        .unwrap();
        let names: Vec<String> = q
            .nodes
            .iter()
            .filter_map(|n| n.get("content").and_then(|v| v.as_str()).map(String::from))
            .collect();

        assert_eq!(
            names.len(),
            Tool::ALL.len(),
            "all {} tool nodes should be seeded, got {:?}",
            Tool::ALL.len(),
            names
        );
        // The two tools the seed bug previously dropped must be present.
        assert!(
            names.iter().any(|n| n == "create_schema"),
            "create_schema must seed"
        );
        assert!(
            names.iter().any(|n| n == "search_skills"),
            "search_skills must seed"
        );
    }
}
