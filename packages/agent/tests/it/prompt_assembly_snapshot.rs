//! Deterministic, zero-model-call snapshot gate over what NodeSpace actually
//! assembles and sends the model — the half of the golden-prompt methodology
//! `live_stage1_golden_prompts.rs` names as "not yet written":
//!
//! > separate "does this exact prompt text get the right tool call"
//! > (answerable [there], in seconds) from "does the real pipeline assemble
//! > that exact prompt from real inputs" (a distinct, deterministic,
//! > zero-model-call question — the snapshot tests this golden set feeds).
//!
//! This file is that snapshot test. It calls the REAL production assembly
//! functions — not hand-authored stand-ins — against committed, in-process
//! fixtures, and diffs the result against a checked-in golden file. No
//! daemon, no embedding service, no model: the whole suite runs in
//! milliseconds as part of the default `cargo test`.
//!
//! ## The five sites this covers
//!
//! 1. **Resident system prompt** — `assemble_resident_system_prompt` (defined
//!    in this file), which seeds the agent-guidance table into a fresh
//!    database the way the daemon does and calls
//!    `PromptAssembler::assemble()` on it. That is production's own path, end
//!    to end: the table's rows become nodes under their fixed ids, the
//!    assembler fetches those ids in table order, flattens each node's
//!    subtree and renders it through Minijinja. A change to the order of the
//!    sections, to which nodes join the prompt, or to what a body loses in
//!    the parse-and-flatten round trip shows up here as a golden diff.
//! 2. **Stage-2 candidate block** — `routing::render_candidates_for_prompt`,
//!    including each candidate's rendered instruction subtree.
//! 3. **Stage-2 tool surface** — `routing::stage2_tools`: names,
//!    descriptions, and full parameter schemas, scoped to the fixture
//!    candidates' whitelists. A second golden holds the parameters that
//!    change when the turn is held to its offered types
//!    (`routing::hold_to_offered_types`).
//! 4. **Stage-1 request** — `stage1_system_prompt` + `stage1_tool_definitions()`.
//! 5. **`EXISTING SCHEMAS`**, which reaches the prompt from two
//!    independent sites and both are covered separately:
//!    - the Stage-2 candidate block (site 2, inside this file's
//!      `stage2_candidate_block_matches_golden`)
//!    - the resident workspace context
//!      (`context_ops::WorkspaceContext::format_for_prompt`, exercised
//!      directly by `resident_workspace_context_matches_golden` AND
//!      indirectly by `resident_system_prompt_matches_golden`, which feeds
//!      its output through the `{{ workspace_context }}` template variable
//!      exactly as `local_agent_service.rs` does)
//!
//! ## Fixtures, not retrieval
//!
//! Skill instructions come from `skill_pipeline::seed_skill_nodes()` — the
//! real production skill corpus — run through the real
//! `prepare_nodes_from_template` parser and the real `render_subtree_markdown`
//! subtree-flatten function that `skill_ops::render_skill_instructions`
//! calls against a live DB. Building
//! the `node_map`/`adjacency_list` directly from `prepare_nodes_from_template`'s
//! output reproduces that subtree without a database — the seeding parse and
//! the flatten are each production code; only the DB round trip between them
//! is skipped.
//!
//! `schema_metadata` and the resident workspace context's `relevant_schemas`/
//! `related_schemas` are hand-built `SchemaNode` fixtures (a `ticket` and an
//! `adr` type, plus a `release` type for the one-hop RELATED section) —
//! standing in for what semantic schema retrieval would return. Retrieval
//! itself is explicitly out of scope (no embedding service, no DB), so the
//! fixture supplies its *output* and the test exercises everything
//! downstream of it, encoded through the real `EntityTypeDescriptor` JSON
//! contract rather than a hand-rolled shape.
//!
//! ## What the checked-in golden content represents
//!
//! **Day one, this reflects CURRENT production assembly output — not the
//! tuned target from the `packages/agent/goldens/` corpus.**
//! Those TOML case files are hand-authored, live-model-validated *content*
//! for `golden_runner`'s tuning loop; the assertions here run the *actual*
//! assembly code (`PromptAssembler`, `routing.rs`, `context_ops.rs`, the
//! seed tables' Markdown) against fixture inputs and pin whatever it currently
//! emits. This gate is deliberately scoped to only make drift from THIS
//! baseline visible — it does not fix or judge any gap between the two.
//! Bringing production's emitted text in line with the tuned corpus is
//! separate follow-up work, updating these goldens (deliberately, per the
//! workflow below) as it happens.
//!
//! ## Updating a golden
//!
//! A golden file is never written by a bare test failure. To regenerate
//! every golden fragment after a deliberate change to a model-facing
//! constant:
//!
//! ```text
//! UPDATE_GOLDEN=1 cargo test -p nodespace-agent --test it prompt_assembly_snapshot::
//! ```
//!
//! Then read the `git diff` on the golden files under `tests/golden/
//! prompt_assembly/` before committing — that diff IS the review of what
//! changed in what the model receives.

use std::collections::HashMap;
use std::sync::Arc;

use nodespace_agent::agent_types::{SkillCandidate, ToolDefinition};
use nodespace_agent::local_agent::agent_loop::stage1_system_prompt;
use nodespace_agent::local_agent::routing::{
    declare_write_tool_fields, hold_to_offered_types, offered_types, render_candidates_for_prompt,
    render_skill_names_for_prompt, stage1_tool_definitions, stage2_tools,
};
use nodespace_agent::local_agent::tools::model_facing_tool_definitions;
use nodespace_agent::prompt_assembler::PromptAssembler;
use nodespace_agent::skill_pipeline::seed_skill_nodes;

use nodespace_core::db::ResolvedEntity;
use nodespace_core::markdown::{prepare_nodes_from_template, NodeTemplate, PreparedNode};
use nodespace_core::models::schema::EnumValue;
use nodespace_core::models::{Node, SchemaField, SchemaProtectionLevel, SkillFields};
use nodespace_core::ops::context_ops::{
    CollectionSummary, EntityResolution, PlaybookInfo, WorkspaceContext,
};
use nodespace_core::ops::entity_types_block::EntityTypeDescriptor;
use nodespace_core::services::{render_subtree_markdown, NodeService};

/// The skill names production's Stage-1 prompt carries on a freshly seeded
/// registry — what `GraphToolExecutor::skill_names` returns there.
fn seeded_skill_names() -> Vec<String> {
    nodespace_agent::local_agent::agent_loop::stage1_skill_names(
        nodespace_agent::skill_pipeline::seed_skill_nodes()
            .into_iter()
            .map(|t| t.title),
    )
}

// ---------------------------------------------------------------------------
// Fixture constants
// ---------------------------------------------------------------------------

/// Deliberately not tied to wall-clock time — a fixed fixture, not "today",
/// so the golden text never drifts on its own between runs on different days.
const FIXTURE_DATE: &str = "2026-06-15";

/// Matches production's real budget: `local_agent_service.rs`'s
/// `context.format_for_prompt(4000)` call. Using the same number here means
/// a future change to that budget is itself a change this gate would need a
/// golden update for, rather than an untested constant.
///
/// The fixture's rendered context is far under this budget (well under 1000
/// chars), so `format_for_prompt`'s per-section truncation branches (the
/// `out.len() + line.len() > max_chars` checks) are NOT exercised here —
/// this gate only pins the budget *value*, not truncation behavior at the
/// boundary. That's `context_ops.rs`'s own unit-test territory, not
/// something a fixed fixture at snapshot precision is a good fit for (a
/// fixture sized to sit exactly at the boundary would be one content change
/// away from silently drifting off it).
const WORKSPACE_CONTEXT_MAX_CHARS: usize = 4000;

/// Fixed fixture model name — never read from `model_manager` or any
/// runtime source, so this text never varies with what's actually installed.
const FIXTURE_MODEL_NAME: &str = "gemma-4-E4B-it-Q4_K_M";

// ---------------------------------------------------------------------------
// Schema fixtures — stand in for what semantic schema retrieval would return
// ---------------------------------------------------------------------------

fn fixed_timestamp() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-06-15T00:00:00Z")
        .expect("valid fixed fixture timestamp")
        .with_timezone(&chrono::Utc)
}

fn schema_field(name: &str, field_type: &str, required: bool) -> SchemaField {
    SchemaField {
        name: name.to_string(),
        friendly_name: String::new(),
        field_type: field_type.parse().expect("a field type"),
        protection: SchemaProtectionLevel::User,
        core_values: None,
        user_values: None,
        indexed: false,
        required: Some(required),
        extensible: None,
        default: None,
        description: None,
        item_type: None,
        fields: None,
        item_fields: None,
        unique: None,
        unique_case_insensitive: None,
        local_only: false,
    }
}

fn schema_field_enum(name: &str, values: &[&str], required: bool) -> SchemaField {
    let mut field = schema_field(name, "enum", required);
    field.user_values = Some(
        values
            .iter()
            .map(|v| EnumValue::new(v.to_string(), v.to_string()))
            .collect(),
    );
    field
}

/// A user-defined type with a required field, an enum field, an optional
/// field, and a `title_template` — exercises every branch of
/// `EntityFieldDescriptor::render`/`render_line`.
fn fixture_schema_ticket() -> nodespace_core::models::SchemaNode {
    let now = fixed_timestamp();
    nodespace_core::models::SchemaNode {
        envelope: nodespace_core::models::NodeEnvelope {
            created_at: now,
            modified_at: now,
            ..nodespace_core::models::SchemaNode::new("ticket", "Ticket").envelope
        },
        extends: None,
        is_core: false,
        is_abstract: false,
        children: Default::default(),
        parent: Default::default(),
        schema_version: 1,
        fields: vec![
            schema_field("title", "text", true),
            schema_field_enum(
                "status",
                &[
                    "ready_for_dev",
                    "in_dev",
                    "ready_for_review",
                    "in_review",
                    "done",
                ],
                true,
            ),
            schema_field("assignee", "text", false),
            schema_field("sprint", "text", false),
        ],
        relationships: Vec::new(),
        title_template: Some("{title}".to_string()),
        properties_header_summary_template: None,
    }
}

/// A second user-defined type with no `title_template` — exercises the
/// no-template line branch alongside `ticket`'s templated one.
fn fixture_schema_adr() -> nodespace_core::models::SchemaNode {
    let now = fixed_timestamp();
    nodespace_core::models::SchemaNode {
        envelope: nodespace_core::models::NodeEnvelope {
            created_at: now,
            modified_at: now,
            ..nodespace_core::models::SchemaNode::new("adr", "ADR").envelope
        },
        extends: None,
        is_core: false,
        is_abstract: false,
        children: Default::default(),
        parent: Default::default(),
        schema_version: 1,
        fields: vec![
            schema_field("title", "text", true),
            schema_field_enum("status", &["proposed", "accepted", "superseded"], true),
            schema_field("supersedes", "text", false),
        ],
        relationships: Vec::new(),
        title_template: None,
        properties_header_summary_template: None,
    }
}

/// One-hop-related type with no fields of its own — exercises the RELATED
/// (name-only) section of `format_for_prompt`.
fn fixture_schema_release() -> nodespace_core::models::SchemaNode {
    let now = fixed_timestamp();
    nodespace_core::models::SchemaNode {
        envelope: nodespace_core::models::NodeEnvelope {
            created_at: now,
            modified_at: now,
            ..nodespace_core::models::SchemaNode::new("release", "Release").envelope
        },
        extends: None,
        is_core: false,
        is_abstract: false,
        children: Default::default(),
        parent: Default::default(),
        schema_version: 1,
        fields: Vec::new(),
        relationships: Vec::new(),
        title_template: None,
        properties_header_summary_template: None,
    }
}

fn fixture_workspace_context() -> WorkspaceContext {
    WorkspaceContext {
        collections: vec![
            CollectionSummary {
                name: "Engineering".to_string(),
                description: "Design notes, ADRs and tickets for the product team".to_string(),
            },
            // Exercises the no-description rendering branch.
            CollectionSummary::named("Q3 Planning"),
        ],
        active_playbooks: vec![
            PlaybookInfo {
                name: "Sprint Close-Out".to_string(),
                description:
                    "When a ticket's status -> done, check whether its sprint is fully closed."
                        .to_string(),
            },
            // Exercises the no-description rendering branch.
            PlaybookInfo {
                name: "Weekly Triage".to_string(),
                description: String::new(),
            },
        ],
        relevant_schemas: vec![
            EntityTypeDescriptor::from_chain(&fixture_schema_ticket(), []),
            EntityTypeDescriptor::from_chain(&fixture_schema_adr(), []),
        ],
        related_schemas: vec![fixture_schema_release()],
        semantic_schema_count: 2,
        // Populated rather than left `NotRun`, so the assembled prompt this
        // snapshot pins actually contains the entity tier. A `NotRun` fixture
        // renders nothing, which would let the tier's wording and its position
        // relative to EXISTING SCHEMAS drift without the golden file noticing.
        // Cut (`not_shown` > 0) for the same reason: the truncation note is
        // model-facing text, and an uncut fixture would leave it unpinned.
        resolved_entities: EntityResolution::Resolved {
            entities: vec![ResolvedEntity {
                id: "01J8ZQ3K7X2N4P6R8T0V2W4Y6A".to_string(),
                title: "Northwind Trading".to_string(),
                node_type: "company_sold_to".to_string(),
                score: -2.31,
            }],
            not_shown: 1,
        },
    }
}

/// The `schema_metadata` payload a Stage-2 candidate would carry, built
/// through the real `EntityTypeDescriptor::to_json` encoding — the same
/// reversible mapping `skill_ops` uses to produce it and
/// `routing::render_schema_metadata` uses to decode it — rather than a
/// hand-rolled JSON shape.
fn fixture_schema_metadata() -> serde_json::Value {
    serde_json::Value::Array(vec![
        EntityTypeDescriptor::from_chain(&fixture_schema_ticket(), []).to_json(),
        EntityTypeDescriptor::from_chain(&fixture_schema_adr(), []).to_json(),
    ])
}

// ---------------------------------------------------------------------------
// Shared subtree-flatten helper — used by both the resident-prompt and
// skill-candidate fixtures below.
// ---------------------------------------------------------------------------

/// Build the `(node_map, adjacency_list)` pair `render_subtree_markdown`
/// expects, directly from a `prepare_nodes_from_template` parse — the same
/// shape `get_subtree_data` would hand back from a real DB for the
/// equivalent seeded node, minus the round trip.
fn build_node_map_and_adjacency(
    prepared: &[PreparedNode],
) -> (HashMap<String, Node>, HashMap<String, Vec<String>>) {
    let mut node_map = HashMap::new();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for p in prepared {
        node_map.insert(
            p.id.clone(),
            Node::new_with_id(
                p.id.clone(),
                p.node_type.clone(),
                p.content.clone(),
                p.properties.clone(),
            ),
        );
        if let Some(parent_id) = &p.parent_id {
            adjacency
                .entry(parent_id.clone())
                .or_default()
                .push(p.id.clone());
        }
    }
    (node_map, adjacency)
}

// ---------------------------------------------------------------------------
// Resident system prompt fixture — the production path against a fresh
// database: seed the agent-guidance table, then `PromptAssembler::assemble()`.
// ---------------------------------------------------------------------------

/// The full resident system prompt, assembled the way production assembles
/// it: the agent-guidance seeds are reconciled into a fresh database exactly
/// as the daemon's startup does, and `PromptAssembler::assemble()` reads them
/// back by the table's ids, in table order.
async fn assemble_resident_system_prompt(workspace_context: &str) -> String {
    let tmp = tempfile::TempDir::new().expect("temp dir");
    let mut store = Arc::new(
        nodespace_core::db::SqliteStore::new(tmp.path().join("golden.db"))
            .await
            .expect("store opens"),
    );
    let node_service = Arc::new(NodeService::new(&mut store).await.expect("service opens"));
    let groups: Vec<_> = PromptAssembler::seed_agent_guidance_nodes()
        .iter()
        .map(|tmpl| prepare_nodes_from_template(tmpl).expect("seed prompt template parses"))
        .collect();
    node_service
        .seed_nodes_from_templates(groups)
        .await
        .expect("seeding succeeds");

    let ctx = nodespace_agent::prompt_assembler::TemplateContext {
        current_date: FIXTURE_DATE.to_string(),
        model_name: FIXTURE_MODEL_NAME.to_string(),
        workspace_context: workspace_context.to_string(),
        // A fixed identity, so the golden pins the current-user line the
        // model reads on a turn where the user has set one.
        current_user: Some(nodespace_agent::prompt_assembler::CurrentUser {
            id: "5c1e9a47-2d83-4b6f-8e10-7a3f9d2c4b65".to_string(),
            name: "Ada Lovelace".to_string(),
            email: "ada@example.com".to_string(),
        }),
    };
    PromptAssembler::new(node_service)
        .assemble(&ctx, Vec::new())
        .await
        .system_prompt
}

// ---------------------------------------------------------------------------
// Skill-candidate fixtures — real seeded skill content, no DB
// ---------------------------------------------------------------------------

/// Render a seeded skill's instruction subtree to markdown the same way
/// production does: `skill_ops::render_skill_instructions` fetches
/// `node_service.get_subtree_data` then calls this same
/// `render_subtree_markdown`. Building the node_map/adjacency_list directly
/// from `prepare_nodes_from_template`'s output reproduces that subtree
/// without a database — the seed parse and the subtree flatten are each
/// production code; only the DB round trip between them is skipped.
fn render_seed_instructions(tmpl: &NodeTemplate) -> String {
    let prepared = prepare_nodes_from_template(tmpl).expect("seed skill template parses");
    let root_id = prepared[0].id.clone();
    let (node_map, adjacency) = build_node_map_and_adjacency(&prepared);
    render_subtree_markdown(&root_id, &node_map, &adjacency)
}

fn seed_skill(tmpl: &NodeTemplate) -> SkillFields {
    SkillFields::from_properties(&tmpl.root_properties).expect("seed decodes as a skill")
}

fn skill_whitelist(tmpl: &NodeTemplate) -> Vec<String> {
    seed_skill(tmpl).tool_whitelist
}

fn skill_description(tmpl: &NodeTemplate) -> String {
    seed_skill(tmpl).description
}

/// Two real seeded skills standing in for what retrieval would hand Stage 2
/// after a turn matched them. `skill_pipeline::seed_skill_nodes()` is the
/// exact production skill corpus (not text authored for this test); scores
/// are fixed comfortably above both `routing.rs` gate bars so both stay
/// eligible regardless of future bar tuning; `schema_metadata` is the real
/// `EntityTypeDescriptor` JSON encoding.
///
/// "Node Creation" and "Schema Creation" are chosen deliberately: both are
/// mutating skills whose own instructions reference `EXISTING SCHEMAS`
/// directly, and both whitelist real registered tools
/// (`create_node`/`create_schema`/…), so `stage2_tools` exercises its scoped
/// (non-fail-open) branch rather than falling back to the full surface.
fn fixture_candidates() -> Vec<SkillCandidate> {
    let seeds = seed_skill_nodes();
    let node_creation = seeds
        .iter()
        .find(|t| t.title == "Node Creation")
        .expect("seed_skill_nodes must still seed a Node Creation skill");
    let schema_creation = seeds
        .iter()
        .find(|t| t.title == "Schema Creation")
        .expect("seed_skill_nodes must still seed a Schema Creation skill");

    let metadata = fixture_schema_metadata();
    vec![
        SkillCandidate {
            id: "fixture-skill-node-creation".to_string(),
            name: node_creation.title.clone(),
            description: skill_description(node_creation),
            score: 0.85,
            tools: skill_whitelist(node_creation),
            instructions: render_seed_instructions(node_creation),
            schema_metadata: metadata.clone(),
            schemas_linked: false,
            pinned: false,
        },
        SkillCandidate {
            id: "fixture-skill-schema-creation".to_string(),
            name: schema_creation.title.clone(),
            description: skill_description(schema_creation),
            score: 0.85,
            tools: skill_whitelist(schema_creation),
            instructions: render_seed_instructions(schema_creation),
            schema_metadata: metadata,
            schemas_linked: false,
            pinned: false,
        },
    ]
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

/// Render a tool list as name + description + pretty-printed parameter
/// schema, in the order given — order is part of what could drift, so it is
/// preserved rather than sorted.
fn render_tool_definitions(tools: &[ToolDefinition]) -> String {
    let mut out = String::new();
    for t in tools {
        out.push_str(&format!("### {}\n", t.name));
        out.push_str(&format!("description: {}\n", t.description));
        out.push_str("parameters_schema:\n");
        match serde_json::to_string_pretty(&t.parameters_schema) {
            Ok(pretty) => out.push_str(&pretty),
            Err(e) => out.push_str(&format!("<unserializable: {e}>")),
        }
        out.push_str("\n\n");
    }
    out
}

// ---------------------------------------------------------------------------
// Golden comparison harness
// ---------------------------------------------------------------------------

mod golden {
    use std::path::{Path, PathBuf};

    fn dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/prompt_assembly")
    }

    fn path(section: &str) -> PathBuf {
        dir().join(format!("{section}.golden"))
    }

    /// The one and only place a golden file is written. Gated on an explicit
    /// env var read here, at the update site — never inferred from "the
    /// assertion below is about to fail". A bare `cargo test` never reaches
    /// the write branch.
    fn update_requested() -> bool {
        std::env::var("UPDATE_GOLDEN").ok().as_deref() == Some("1")
    }

    /// Compare `actual` against the checked-in golden fragment named
    /// `section`, panicking with a readable line diff on mismatch.
    ///
    /// With `UPDATE_GOLDEN=1` set, writes `actual` as the new golden and
    /// returns without comparing — the explicit, opt-in regeneration path.
    /// Without it, a missing golden file is a hard failure (never silently
    /// created) and a content mismatch is a hard failure with a diff, never
    /// a silent pass or an automatic rewrite.
    pub fn assert_matches(section: &str, actual: &str) {
        let file = path(section);

        if update_requested() {
            std::fs::create_dir_all(dir()).expect("create tests/golden/prompt_assembly");
            std::fs::write(&file, actual)
                .unwrap_or_else(|e| panic!("failed to write golden {}: {e}", file.display()));
            eprintln!(
                "UPDATE_GOLDEN=1: wrote {} ({} bytes) — review with `git diff` before committing",
                file.display(),
                actual.len()
            );
            return;
        }

        let expected = std::fs::read_to_string(&file).unwrap_or_else(|e| {
            panic!(
                "golden file missing or unreadable at {} ({e}).\n\n\
                 Goldens are never auto-created. If `{section}`'s assembled output is new \
                 or its change is deliberate, generate it explicitly:\n\n  \
                 UPDATE_GOLDEN=1 cargo test -p nodespace-agent --test it prompt_assembly_snapshot::\n\n\
                 then review the new file with `git diff` before committing it.",
                file.display()
            )
        });

        if actual == expected {
            return;
        }

        panic!("{}", render_diff(section, &file, &expected, actual));
    }

    /// A readable unified-style line diff, not a two-blob dump — the point
    /// is showing which line of guidance moved.
    fn render_diff(section: &str, file: &Path, expected: &str, actual: &str) -> String {
        use similar::ChangeTag;

        let diff = similar::TextDiff::from_lines(expected, actual);
        let mut out = format!(
            "prompt-assembly drift detected in section `{section}` ({})\n\n",
            file.display()
        );
        for group in diff.grouped_ops(3) {
            for op in group {
                for change in diff.iter_changes(&op) {
                    let sign = match change.tag() {
                        ChangeTag::Delete => '-',
                        ChangeTag::Insert => '+',
                        ChangeTag::Equal => ' ',
                    };
                    out.push_str(&format!("{sign}{change}"));
                }
            }
            out.push_str("---\n");
        }
        out.push_str(&format!(
            "\nIf this change to `{section}`'s assembled output is INTENTIONAL, update the \
             golden:\n\n  UPDATE_GOLDEN=1 cargo test -p nodespace-agent --test \
             prompt_assembly_snapshot\n\n  then review the diff on the golden file itself with \
             `git diff` before committing.\n\n\
             If it is NOT intentional, an edit to a model-facing source (a seed's Markdown \
             under src/seeds/, tools.rs, routing.rs, context_ops.rs) changed what production \
             sends the model — find it and revert it.\n"
        ));
        out
    }
}

// ---------------------------------------------------------------------------
// The five gates
// ---------------------------------------------------------------------------

/// Site 1: the resident system prompt, through `PromptAssembler::assemble()`
/// over a freshly seeded database (see `assemble_resident_system_prompt`).
/// Also covers `RELEVANT ENTITY TYPES` site 2 (resident workspace context) as
/// it actually reaches the model: substituted into the "Workspace Context
/// Template" seed via `{{ workspace_context }}`, exactly as
/// `local_agent_service.rs::build_workspace_context_string` does.
#[tokio::test]
async fn resident_system_prompt_matches_golden() {
    let workspace_context =
        fixture_workspace_context().format_for_prompt(WORKSPACE_CONTEXT_MAX_CHARS);
    let system_prompt = assemble_resident_system_prompt(&workspace_context).await;
    golden::assert_matches("resident_system_prompt", &system_prompt);
}

/// `EXISTING SCHEMAS` site 2 on its own: the raw
/// `WorkspaceContext::format_for_prompt` output, independent of its later
/// substitution into the resident prompt above.
#[test]
fn resident_workspace_context_matches_golden() {
    let rendered = fixture_workspace_context().format_for_prompt(WORKSPACE_CONTEXT_MAX_CHARS);
    golden::assert_matches("resident_workspace_context", &rendered);
}

/// Site 2: the Stage-2 candidate block, including each candidate's rendered
/// instruction subtree. Also covers `EXISTING SCHEMAS` site 1
/// (per-candidate `schema_metadata`).
#[test]
fn stage2_candidate_block_matches_golden() {
    let candidates = fixture_candidates();
    let rendered = render_candidates_for_prompt(&candidates)
        .expect("fixture candidates must clear the score gate and render a non-empty block");
    golden::assert_matches("stage2_candidate_block", &rendered);
}

/// What Stage 2 is shown on a lookup, as `agent_loop.rs` appends it to the
/// resident prompt: the registry's skill list
/// (`routing::render_skill_names_for_prompt`), then the candidate block for a
/// turn Research & Search leads.
///
/// A separate golden from site 2 because that fixture's candidates are both
/// write skills: the search-before-answering and read-before-answering
/// instructions a question about the user's knowledge depends on live in
/// Research & Search's own subtree, and nothing else pins them.
#[test]
fn stage2_lookup_block_matches_golden() {
    let skills = render_skill_names_for_prompt(&seeded_skill_names())
        .expect("a seeded registry names its skills");
    let seeds = seed_skill_nodes();
    let research = seeds
        .iter()
        .find(|t| t.title == "Research & Search")
        .expect("seed_skill_nodes must still seed a Research & Search skill");
    let candidates = vec![SkillCandidate {
        id: "fixture-skill-research-and-search".to_string(),
        name: research.title.clone(),
        description: skill_description(research),
        score: 0.85,
        tools: skill_whitelist(research),
        instructions: render_seed_instructions(research),
        // Research & Search declares no `node_types`, and a workspace with no
        // custom schema leaves its unscoped fallback empty.
        schema_metadata: serde_json::json!([]),
        schemas_linked: false,
        pinned: false,
    }];
    let block = render_candidates_for_prompt(&candidates)
        .expect("a read-only candidate at 0.85 clears its score gate");
    golden::assert_matches("stage2_lookup_block", &format!("{skills}\n\n{block}"));
}

/// Site 3: the scoped Stage-2 tool surface — names, descriptions, and full
/// parameter schemas, restricted to what the fixture candidates whitelist.
#[test]
fn stage2_tool_surface_matches_golden() {
    let candidates = fixture_candidates();
    let all_tools = model_facing_tool_definitions();
    let scoped = stage2_tools(&candidates, &all_tools);
    // `agent_loop.rs` calls this immediately after `stage2_tools`, gated on
    // `!session.routing_disabled` — the fixture models the
    // routing-enabled path (the default, and the only one this fixture's
    // candidates give a meaningful answer for), so it is called
    // unconditionally here to keep this gate a faithful reproduction of what
    // production actually sends on that path. `declare_write_tool_fields`'s
    // own doc comment explains why this is a separate call rather than
    // folded into `stage2_tools` itself.
    let scoped = declare_write_tool_fields(&candidates, scoped);
    assert!(
        scoped.len() < all_tools.len(),
        "fixture candidates should scope the tool surface down from the full {} tools, not \
         fail open to it — check that skill_pipeline::seed_skill_nodes()'s tool_whitelist \
         entries still name registered tools",
        all_tools.len()
    );
    let rendered = render_tool_definitions(&scoped);
    golden::assert_matches("stage2_tool_surface", &rendered);
}

/// Site 3 on a turn held to its offered types: what the tool surface above
/// gains when every candidate carries its linked schemas.
///
/// The golden holds only the parameters that change, each in full, so it shows
/// the `enum` and its description as the model receives them without
/// repeating the whole surface. It is taken over every model-facing tool
/// rather than the fixture candidates' scoped surface, so each tool that can
/// be held appears whichever skills whitelist it. The seeded skills link to no
/// schema; the fixture marks them linked to stand in for skills that do.
#[test]
fn stage2_tool_surface_held_to_offered_types_matches_golden() {
    let candidates: Vec<SkillCandidate> = fixture_candidates()
        .into_iter()
        .map(|c| SkillCandidate {
            schemas_linked: true,
            pinned: false,
            ..c
        })
        .collect();
    let open = declare_write_tool_fields(&candidates, model_facing_tool_definitions());
    let offered = offered_types(&candidates).expect("every fixture candidate is linked");
    let held = hold_to_offered_types(open.clone(), &offered);

    let mut rendered = String::new();
    for (open, held) in open.iter().zip(&held) {
        assert_eq!(open.name, held.name);
        assert_eq!(
            open.description, held.description,
            "the offered types are stated in the parameter schema, not the description"
        );
        let empty = serde_json::Map::new();
        let properties = |tool: &ToolDefinition| {
            tool.parameters_schema["properties"]
                .as_object()
                .unwrap_or(&empty)
                .clone()
        };
        let before = properties(open);
        for (name, declared) in properties(held) {
            if before.get(&name) == Some(&declared) {
                continue;
            }
            rendered.push_str(&format!("### {}.{name}\n", held.name));
            rendered.push_str(
                &serde_json::to_string_pretty(&declared).expect("a JSON value serialises"),
            );
            rendered.push_str("\n\n");
        }
    }
    golden::assert_matches("stage2_tool_surface_offered_types", &rendered);
}

/// Sites 2 and 3 for a turn routed to Play Authoring: the candidate block,
/// then the tool surface as the model receives it.
///
/// Its own golden because nothing else pins it. This is the one built-in
/// skill linked to a schema, so its candidate carries the core `play` schema
/// rather than the fixture's custom types, and its turn is held to its
/// offered types (ADR-090 §6). `update_play`'s parameter schema is where the
/// rule shape is stated, and it reaches no other fixture's surface.
#[test]
fn stage2_play_authoring_matches_golden() {
    let seeds = seed_skill_nodes();
    let authoring = seeds
        .iter()
        .find(|t| t.title == "Play Authoring")
        .expect("seed_skill_nodes must still seed a Play Authoring skill");
    let core_schemas = nodespace_core::models::core_schemas::get_core_schemas();
    let play = core_schemas
        .iter()
        .find(|s| s.envelope.id == "play")
        .expect("play is a core schema");
    let candidates = vec![SkillCandidate {
        id: authoring.id.clone(),
        name: authoring.title.clone(),
        description: skill_description(authoring),
        score: 0.85,
        tools: skill_whitelist(authoring),
        instructions: render_seed_instructions(authoring),
        schema_metadata: serde_json::Value::Array(vec![EntityTypeDescriptor::from_corpus(
            play,
            &core_schemas,
        )
        .to_json()]),
        schemas_linked: true,
        pinned: false,
    }];

    let block = render_candidates_for_prompt(&candidates)
        .expect("a mutating candidate at 0.85 clears its score gate");
    let tools = declare_write_tool_fields(
        &candidates,
        stage2_tools(&candidates, &model_facing_tool_definitions()),
    );
    let offered = offered_types(&candidates).expect("the candidate is linked");
    let tools = hold_to_offered_types(tools, &offered);

    golden::assert_matches(
        "stage2_play_authoring",
        &format!("{block}\n\nTOOLS:\n{}", render_tool_definitions(&tools)),
    );
}

/// Site 4: the Stage-1 request — `stage1_system_prompt` plus
/// `stage1_tool_definitions()`, exactly as `agent_loop.rs::route` sends it.
#[test]
fn stage1_request_matches_golden() {
    let mut rendered = String::new();
    rendered.push_str("SYSTEM PROMPT:\n");
    rendered.push_str(&stage1_system_prompt(
        &seeded_skill_names(),
        &nodespace_agent::local_agent::agent_loop::stage1_type_names(
            ["Invoice".to_string()],
            "how many tasks and invoices are open?",
        ),
    ));
    rendered.push_str("\n\nTOOLS:\n");
    rendered.push_str(&render_tool_definitions(&stage1_tool_definitions()));
    golden::assert_matches("stage1_request", &rendered);
}

/// Every seeded body — skills and resident agent guidance — must reach the
/// model as the markdown its author wrote: same lines, same order, list
/// markers and nesting intact. Blank lines are ignored in the comparison
/// (render joins blocks with exactly one, and a tight list's spacing is
/// pinned by `render_subtree_markdown`'s own unit tests); anything else the
/// parse → render round trip loses fails here, naming the seed and the
/// first line that differs.
#[test]
fn seeded_markdown_round_trips_through_parse_and_render() {
    fn content_lines(markdown: &str) -> Vec<&str> {
        markdown
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .collect()
    }

    let seeds = seed_skill_nodes()
        .into_iter()
        .chain(PromptAssembler::seed_agent_guidance_nodes());
    for tmpl in seeds {
        let rendered = render_seed_instructions(&tmpl);
        let expected = content_lines(&tmpl.markdown_content);
        let actual = content_lines(&rendered);
        let first_diff = expected
            .iter()
            .zip(&actual)
            .position(|(e, a)| e != a)
            .unwrap_or(expected.len().min(actual.len()));
        assert!(
            expected == actual,
            "seed `{}` does not round-trip at line {}:\n  source:   {:?}\n  rendered: {:?}",
            tmpl.title,
            first_diff + 1,
            expected.get(first_diff),
            actual.get(first_diff),
        );
    }
}
