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
//! fixtures, and diffs the result against a checked-in golden file. No DB, no
//! daemon, no embedding service, no model: every function called here is
//! synchronous and pure, so the whole suite runs in milliseconds as part of
//! the default `cargo test`.
//!
//! ## The five sites this covers
//!
//! 1. **Resident system prompt** — `assemble_resident_system_prompt` (defined
//!    in this file), which composes the seeded `agent-guidance` nodes
//!    (`PromptAssembler::seed_agent_guidance_nodes`, sourced from
//!    `agent_guidance.rs`) the same way `PromptAssembler::assemble()` does:
//!    each seed's `markdown_content` is parsed into a node subtree via
//!    `prepare_nodes_from_template` (`assemble()`'s callers seed the graph
//!    with exactly this), flattened via the real `flatten_subtree_content`
//!    (matching `fetch_prompt_body`'s traversal), THEN rendered through
//!    Minijinja and joined.
//!
//!    Deliberately does NOT use `PromptAssembler::assemble_static()` — that
//!    helper renders each seed's raw `markdown_content` string directly,
//!    skipping the parse+flatten step entirely. For a single-line seed body
//!    that's a no-op, but `agent_guidance.rs`'s `TOOL_STRATEGY_RULES`/
//!    `SCHEMA_CREATION_RULES` bodies are multi-line with `"HEADER:\n- bullet"`
//!    structure: `prepare_nodes_from_template` parses each bullet into a
//!    child node (stripping the `"- "` prefix) and `flatten_subtree_content`
//!    rejoins them with blank lines — a materially different string than the
//!    raw source text `assemble_static` would emit. `fetch_prompt_body`'s own
//!    doc comment names the historical bug this exact mechanism guards
//!    against ("seed prompt body dropped" — a direct-children-only flatten
//!    silently ate the bullets). Using `assemble_static` here would leave
//!    that mechanism, and any regression in it, outside this gate.
//! 2. **Stage-2 candidate block** — `routing::render_candidates_for_prompt`,
//!    including each candidate's rendered instruction subtree.
//! 3. **Stage-2 tool surface** — `routing::stage2_tools`: names,
//!    descriptions, and full parameter schemas, scoped to the fixture
//!    candidates' whitelists.
//! 4. **Stage-1 request** — `STAGE1_SYSTEM_PROMPT` + `stage1_tool_definitions()`.
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
//! `prepare_nodes_from_template` parser and the real `flatten_subtree_content`
//! subtree-flatten function that `skill_ops::render_skill_instructions` and
//! `PromptAssembler::fetch_prompt_body` both call against a live DB. Building
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
//! assembly code (`PromptAssembler`, `routing.rs`, `context_ops.rs`,
//! `agent_guidance.rs`) against fixture inputs and pin whatever it currently
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
//! UPDATE_GOLDEN=1 cargo test -p nodespace-agent --test prompt_assembly_snapshot
//! ```
//!
//! Then read the `git diff` on the golden files under `tests/golden/
//! prompt_assembly/` before committing — that diff IS the review of what
//! changed in what the model receives.

use std::collections::HashMap;

use nodespace_agent::agent_types::{SkillCandidate, ToolDefinition};
use nodespace_agent::local_agent::agent_loop::STAGE1_SYSTEM_PROMPT;
use nodespace_agent::local_agent::routing::{
    declare_write_tool_fields, render_candidates_for_prompt, stage1_tool_definitions, stage2_tools,
};
use nodespace_agent::local_agent::tools::model_facing_tool_definitions;
use nodespace_agent::prompt_assembler::PromptAssembler;
use nodespace_agent::skill_pipeline::seed_skill_nodes;

use nodespace_core::db::ResolvedEntity;
use nodespace_core::markdown::{prepare_nodes_from_template, NodeTemplate, PreparedNode};
use nodespace_core::models::schema::EnumValue;
use nodespace_core::models::{Node, SchemaField, SchemaProtectionLevel};
use nodespace_core::ops::context_ops::{EntityResolution, PlaybookInfo, WorkspaceContext};
use nodespace_core::ops::entity_types_block::EntityTypeDescriptor;
use nodespace_core::services::flatten_subtree_content;

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
        field_type: field_type.to_string(),
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
        id: "ticket".to_string(),
        content: "Ticket".to_string(),
        version: 1,
        created_at: now,
        modified_at: now,
        is_core: false,
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
        id: "adr".to_string(),
        content: "ADR".to_string(),
        version: 1,
        created_at: now,
        modified_at: now,
        is_core: false,
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
        id: "release".to_string(),
        content: "Release".to_string(),
        version: 1,
        created_at: now,
        modified_at: now,
        is_core: false,
        schema_version: 1,
        fields: Vec::new(),
        relationships: Vec::new(),
        title_template: None,
        properties_header_summary_template: None,
    }
}

fn fixture_workspace_context() -> WorkspaceContext {
    WorkspaceContext {
        collections: vec!["Engineering".to_string(), "Q3 Planning".to_string()],
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
        relevant_schemas: vec![fixture_schema_ticket(), fixture_schema_adr()],
        related_schemas: vec![fixture_schema_release()],
        semantic_schema_count: 2,
        // Populated rather than left `NotRun`, so the assembled prompt this
        // snapshot pins actually contains the entity tier. A `NotRun` fixture
        // renders nothing, which would let the tier's wording and its position
        // relative to EXISTING SCHEMAS drift without the golden file noticing.
        resolved_entities: EntityResolution::Resolved(vec![ResolvedEntity {
            id: "01J8ZQ3K7X2N4P6R8T0V2W4Y6A".to_string(),
            title: "Northwind Trading".to_string(),
            node_type: "company_sold_to".to_string(),
            score: -2.31,
        }]),
    }
}

/// The `schema_metadata` payload a Stage-2 candidate would carry, built
/// through the real `EntityTypeDescriptor::to_json` encoding — the same
/// reversible mapping `skill_ops` uses to produce it and
/// `routing::render_schema_metadata` uses to decode it — rather than a
/// hand-rolled JSON shape.
fn fixture_schema_metadata() -> serde_json::Value {
    serde_json::Value::Array(vec![
        EntityTypeDescriptor::from_schema(&fixture_schema_ticket()).to_json(),
        EntityTypeDescriptor::from_schema(&fixture_schema_adr()).to_json(),
    ])
}

// ---------------------------------------------------------------------------
// Shared subtree-flatten helper — used by both the resident-prompt and
// skill-candidate fixtures below.
// ---------------------------------------------------------------------------

/// Build the `(node_map, adjacency_list)` pair `flatten_subtree_content`
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
// Resident system prompt fixture — real seed -> parse -> flatten -> render,
// no DB. Deliberately reimplements `PromptAssembler::assemble()`'s
// composition using the same building blocks, rather than calling
// `PromptAssembler::assemble_static()` — see the module doc comment for why
// that helper is not a faithful stand-in for multi-line seed bodies.
// ---------------------------------------------------------------------------

/// One seed's contribution to the resident prompt: parse its
/// `markdown_content` into a node subtree exactly as first-run seeding does
/// (`prepare_nodes_from_template`), flatten it exactly as
/// `PromptAssembler::fetch_prompt_body` does (`flatten_subtree_content`,
/// joined with `"\n\n"`), then render through Minijinja exactly as
/// `PromptAssembler::render_template` does (raw text on a render error,
/// never a panic). Returns `None` for an empty body, matching `assemble()`'s
/// own skip.
fn render_seed_prompt_section(
    tmpl: &NodeTemplate,
    ctx: &nodespace_agent::prompt_assembler::TemplateContext,
) -> Option<String> {
    let prepared = prepare_nodes_from_template(tmpl).expect("seed prompt template parses");
    let root_id = prepared[0].id.clone();
    let (node_map, adjacency) = build_node_map_and_adjacency(&prepared);
    let body = flatten_subtree_content(&root_id, &node_map, &adjacency).join("\n\n");
    if body.trim().is_empty() {
        return None;
    }
    let env = minijinja::Environment::new();
    Some(env.render_str(&body, ctx).unwrap_or(body))
}

/// The full resident system prompt: every seed section from
/// `PromptAssembler::seed_agent_guidance_nodes()` (the real production seed
/// list — `agent_guidance.rs` content included), rendered and joined the
/// same way `PromptAssembler::assemble()` joins its sections.
fn assemble_resident_system_prompt(workspace_context: &str) -> String {
    let ctx = nodespace_agent::prompt_assembler::TemplateContext {
        current_date: FIXTURE_DATE.to_string(),
        model_name: FIXTURE_MODEL_NAME.to_string(),
        workspace_context: workspace_context.to_string(),
    };
    let sections: Vec<String> = PromptAssembler::seed_agent_guidance_nodes()
        .iter()
        .filter_map(|tmpl| render_seed_prompt_section(tmpl, &ctx))
        .collect();
    sections.join("\n\n")
}

// ---------------------------------------------------------------------------
// Skill-candidate fixtures — real seeded skill content, no DB
// ---------------------------------------------------------------------------

/// Render a seeded skill's instruction subtree to markdown the same way
/// production does: `skill_ops::render_skill_instructions` fetches
/// `node_service.get_subtree_data` then calls this same
/// `flatten_subtree_content`. Building the node_map/adjacency_list directly
/// from `prepare_nodes_from_template`'s output reproduces that subtree
/// without a database — the seed parse and the subtree flatten are each
/// production code; only the DB round trip between them is skipped (the
/// same trade `PromptAssembler::assemble_static` makes for prompt nodes).
fn render_seed_instructions(tmpl: &NodeTemplate) -> String {
    let prepared = prepare_nodes_from_template(tmpl).expect("seed skill template parses");
    let root_id = prepared[0].id.clone();
    let (node_map, adjacency) = build_node_map_and_adjacency(&prepared);
    flatten_subtree_content(&root_id, &node_map, &adjacency).join("\n\n")
}

fn skill_whitelist(tmpl: &NodeTemplate) -> Vec<String> {
    tmpl.root_properties
        .get("tool_whitelist")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn skill_description(tmpl: &NodeTemplate) -> String {
    tmpl.root_properties
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
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
        },
        SkillCandidate {
            id: "fixture-skill-schema-creation".to_string(),
            name: schema_creation.title.clone(),
            description: skill_description(schema_creation),
            score: 0.85,
            tools: skill_whitelist(schema_creation),
            instructions: render_seed_instructions(schema_creation),
            schema_metadata: metadata,
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
                 UPDATE_GOLDEN=1 cargo test -p nodespace-agent --test prompt_assembly_snapshot\n\n\
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
             If it is NOT intentional, an edit to a model-facing source (agent_guidance.rs, \
             tools.rs, skill_pipeline.rs, routing.rs, context_ops.rs) changed what production \
             sends the model — find it and revert it.\n"
        ));
        out
    }
}

// ---------------------------------------------------------------------------
// The five gates
// ---------------------------------------------------------------------------

/// Site 1: the resident system prompt (`PromptAssembler::assemble()`, via
/// `assemble_resident_system_prompt`'s DB-free reproduction of its seed ->
/// parse -> flatten -> render -> join composition). Also covers `RELEVANT
/// ENTITY TYPES` site 2 (resident workspace context) as it actually reaches
/// the model: substituted into the "Workspace Context Template" seed via
/// `{{ workspace_context }}`, exactly as
/// `local_agent_service.rs::build_workspace_context_string` does.
#[test]
fn resident_system_prompt_matches_golden() {
    let workspace_context =
        fixture_workspace_context().format_for_prompt(WORKSPACE_CONTEXT_MAX_CHARS);
    let system_prompt = assemble_resident_system_prompt(&workspace_context);
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

/// Site 4: the Stage-1 request — `STAGE1_SYSTEM_PROMPT` plus
/// `stage1_tool_definitions()`, exactly as `agent_loop.rs::route` sends it.
#[test]
fn stage1_request_matches_golden() {
    let mut rendered = String::new();
    rendered.push_str("SYSTEM PROMPT:\n");
    rendered.push_str(STAGE1_SYSTEM_PROMPT);
    rendered.push_str("\n\nTOOLS:\n");
    rendered.push_str(&render_tool_definitions(&stage1_tool_definitions()));
    golden::assert_matches("stage1_request", &rendered);
}
