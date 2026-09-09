//! Two-stage skill-mediated routing (ADR-038).
//!
//! The model reaches capabilities through skills discovered by semantic
//! retrieval, with the LLM judging at two gates and an explicit clarification
//! fallback. Retrieval itself is a deterministic system step the model does
//! not perform, which is where the candidate count is bounded and the trust
//! filter applies.
//!
//! ```text
//! Stage 1 (model):  emit EITHER a search query OR a clarification request
//!                   — a structural choice, not a confidence number
//! [system step]:    semantic retrieval over the skill registry → top-K,
//!                   score-gated. NOT a model tool call.
//! Stage 2 (model):  judge the candidates against the intent → pick one and
//!                   act, or clarify with the candidates as concrete options
//! ```
//!
//! ADR-038 rejects the single-turn pull (handing the model a `search_skills`
//! tool and letting it call retrieval itself), because that removes the
//! system's ability to bound K and enforce the trust boundary.

use crate::agent_types::{SkillCandidate, ToolDefinition};
use nodespace_core::ops::context_ops::EXISTING_SCHEMAS_HEADER;
use serde::Deserialize;

use super::tools::Tool;

/// How many skill candidates retrieval may return.
///
/// This is the bound ADR-038 requires the system — not the model — to own.
/// Three is enough to offer a real choice at Stage 2 (and to name concrete
/// options in a clarification) while keeping the injected instruction payload
/// small, since each candidate carries its full instruction subtree.
pub const RETRIEVAL_TOP_K: usize = 3;

/// Minimum retrieval score for a **read-only** skill to be actionable.
///
/// The mechanical half of the Stage-2 gate. The model's yes/no on the
/// candidate is the other, independently-sourced half; both must pass.
///
/// **Deliberately permissive, and not yet tuned.** Tuning requires an eval
/// that can distinguish a model failure from a harness failure, which the
/// agent matrix currently cannot do. Until that lands, a stricter value would
/// be tightening against an untested target: it would silently hide the long
/// tail, and no measurement would show the loss. Raise this only alongside a
/// measurement that demonstrates the tail being hidden is genuinely noise.
///
/// Note this is a different constant from `skill_ops::SKILL_SEARCH_THRESHOLD`,
/// which stays at `0.0` on purpose: retrieval still *returns* the long tail so
/// the judged gate can see it, and this bar decides what is *actionable*.
/// One gate filters, the other judges.
pub const READ_SKILL_SCORE_BAR: f32 = 0.15;

/// Minimum retrieval score for a **mutating** skill to be actionable.
///
/// ADR-038: "the Stage-2 bar is a property of the matched skill, not a single
/// global constant. Mutating skills warrant a higher bar than read-only ones,
/// because the expensive error — firing the wrong *mutating* tool — changes
/// the graph." Bias the gate against that error.
///
/// **Deliberately permissive, and not yet tuned** — see
/// [`READ_SKILL_SCORE_BAR`]. The ordering (mutating strictly above read-only)
/// is the load-bearing property here; the absolute values are placeholders
/// awaiting a diagnostic eval.
pub const MUTATING_SKILL_SCORE_BAR: f32 = 0.30;

/// Minimum retrieval score for a skill that can **irreversibly remove user
/// data** to be actionable.
///
/// The top rung of the blast-radius ladder ADR-038 describes: read-only <
/// mutating < destructive. The ADR states the bar "is a property of the
/// matched skill, not a single global constant" and that the gates should be
/// biased against the expensive error; deleting the node a user meant to
/// update is the one error in this system that cannot be walked back, so it
/// sits above the general mutating bar rather than sharing it.
///
/// **This value is an untuned placeholder, and knowingly so.**
/// [`READ_SKILL_SCORE_BAR`] and [`MUTATING_SKILL_SCORE_BAR`] carry the same
/// caveat, for the same reason: tuning needs an eval that can separate a model
/// failure from a harness failure. This rung was added to stop a weak
/// destructive match winning rank-1 on requests with no deletion intent —
/// `Node Deletion` was measured as a retrieved candidate on 11 turns across 5
/// scenarios whose prompts marked a status, recorded a decision, set a due
/// date, and asked a question, ranking first on three of them. It was chosen
/// structurally (clearly above the mutating bar, well below the score a
/// genuine "delete X" match earns), not measured against live embeddings.
///
/// The *ordering* is the load-bearing property and is asserted in tests. If a
/// real deletion request is ever seen failing to route, this constant is the
/// first thing to revisit.
pub const DESTRUCTIVE_SKILL_SCORE_BAR: f32 = 0.45;

/// Stage-1's structural choice, recovered from which tool the model called.
///
/// ADR-038 rejects gating on a self-reported confidence number: a numeric
/// self-rating from a small model is not calibrated and invites anchoring.
/// The choice is therefore *which of two typed tools* the model calls, which
/// is also the channel measured strongest for structured output — a tool
/// schema, rather than prose the model must be trusted to follow.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteDecision {
    /// The model formed a search query; run retrieval on it.
    Query(String),
    /// The model could not form a query and asked to clarify.
    Clarify {
        /// The specific question to put to the user.
        question: String,
        /// Concrete options to offer. ADR-038: a bare "what do you mean?" is
        /// the failure mode to avoid.
        options: Vec<String>,
    },
    /// The request bundles multiple distinct, unambiguous intents —
    /// one query per intent, each re-entering retrieval independently the
    /// same way a single `Query` does. Not for a single intent expressed
    /// verbosely, and not a substitute for `Clarify` when the request is
    /// genuinely ambiguous rather than compound.
    Multi(Vec<String>),
}

/// Wire name of the Stage-1 tool that emits a search query.
pub const ROUTE_QUERY_TOOL: &str = "route_query";
/// Wire name of the Stage-1 tool that requests clarification.
pub const ROUTE_CLARIFY_TOOL: &str = "route_clarify";
/// Wire name of the Stage-1 tool that emits multiple per-intent queries.
pub const ROUTE_MULTI_TOOL: &str = "route_multi";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteQueryParams {
    query: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteClarifyParams {
    question: String,
    #[serde(default)]
    options: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteMultiParams {
    queries: Vec<String>,
}

/// The three tools offered at Stage 1, and only these three.
///
/// Offering a fixed, small set makes the model's tool choice a discriminated
/// output: there is no free text to parse, only a tool name and typed
/// arguments. The alternative — asking in prose for a `QUERY:`/`CLARIFY:`
/// prefix — relies on the weakest measured channel and needs a parser whose
/// failures are indistinguishable from model failures.
pub fn stage1_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: ROUTE_QUERY_TOOL.to_string(),
            description: "Use when you understand what the user wants. Provide a short search \
                 query describing the capability needed to fulfil it, keeping both the specific \
                 nouns the user named (what kind of thing, what item) AND the action or \
                 distinguishing detail that determines what kind of capability is needed — a \
                 status word, a value, or a verb like update/create/delete/list — rather than \
                 replacing either with a paraphrase or generalizing to a category-level \
                 description. Describe the task using the user's own subject and intent, not a \
                 reinterpreted or flattened one."
                .to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "A short description of the capability needed, in plain \
                             language, built around the exact noun(s) the user used for the \
                             subject (e.g. keep 'albums', 'venues', 'equipment' — do not \
                             substitute a different word for the same thing, like 'watchlist' \
                             or 'queue' for a category of item the user named directly) AND the \
                             specific action or detail that distinguishes what the user wants \
                             done — e.g. 'update the equipment record worth 2400 to returned', \
                             not just 'equipment items'; do not collapse an update/find-a-\
                             specific-item request into a generic listing description."
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: ROUTE_CLARIFY_TOOL.to_string(),
            description: "Use ONLY when the request is too ambiguous to describe as a capability. \
                 Ask one specific question and offer concrete alternatives — never a bare \
                 'what do you mean?'."
                .to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "One specific question that would resolve the ambiguity."
                    },
                    "options": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Two or more concrete interpretations the user can pick between."
                    }
                },
                "required": ["question"]
            }),
        },
        ToolDefinition {
            name: ROUTE_MULTI_TOOL.to_string(),
            description: "Use when the request contains two or more DISTINCT, UNAMBIGUOUS intents \
                 — the user clearly wants several separate things done, not one thing described at \
                 length. Provide one short query per intent, each built the same way route_query's \
                 query is (keep the subject noun AND the action/detail for that specific intent). \
                 Do NOT use this for a single intent, however it's phrased, and do NOT use this \
                 in place of route_clarify when a single intent is itself ambiguous — those are \
                 different problems."
                .to_string(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "One short capability query per distinct intent, in the \
                             order the user mentioned them, each following the same rules as \
                             route_query's query (keep the subject noun and the action/detail). \
                             Two or more entries required — a single intent belongs in route_query \
                             instead."
                    }
                },
                "required": ["queries"]
            }),
        },
    ]
}

/// Recover Stage-1's decision from the model's tool call.
///
/// Returns `None` when the model called neither routing tool or emitted
/// unparseable arguments. The caller treats that as "no routing decision" and
/// falls through to the general tool surface rather than failing the turn:
/// a routing step that cannot decide must not cost the user their request.
pub fn parse_route_decision(tool_name: &str, arguments_json: &str) -> Option<RouteDecision> {
    match tool_name {
        ROUTE_QUERY_TOOL => {
            let p: RouteQueryParams = serde_json::from_str(arguments_json).ok()?;
            let q = p.query.trim();
            if q.is_empty() {
                return None;
            }
            Some(RouteDecision::Query(q.to_string()))
        }
        ROUTE_CLARIFY_TOOL => {
            let p: RouteClarifyParams = serde_json::from_str(arguments_json).ok()?;
            let question = p.question.trim();
            if question.is_empty() {
                return None;
            }
            Some(RouteDecision::Clarify {
                question: question.to_string(),
                options: p.options,
            })
        }
        ROUTE_MULTI_TOOL => {
            let p: RouteMultiParams = serde_json::from_str(arguments_json).ok()?;
            let queries: Vec<String> = p
                .queries
                .iter()
                .map(|q| q.trim().to_string())
                .filter(|q| !q.is_empty())
                .collect();
            // Fewer than two non-empty queries is not a genuine compound
            // intent — either the model over-called this tool for a single
            // intent (fall through to unrouted retrieval on the raw message
            // rather than trust a one-element "multi"), or every entry was
            // blank (no usable decision at all).
            if queries.len() < 2 {
                return None;
            }
            Some(RouteDecision::Multi(queries))
        }
        _ => None,
    }
}

/// Whether a skill can change graph state, derived from the tools it may fire.
///
/// Blast radius is not a stored property. It is computed from the skill's
/// `tool_whitelist` through the tool registry's own write classification, so
/// it cannot drift from what the skill is actually able to do — adding a
/// mutating tool to a whitelist raises that skill's bar automatically, with
/// no second field to keep in sync.
///
/// An unrecognised tool name counts as mutating. Resolving a name that is not
/// in the registry — a typo, a renamed tool, or a future externally-registered
/// one — yields no write classification at all, and treating that absence as
/// read-only would put the *lower* bar on a skill whose blast radius is
/// unknown. ADR-038 says to bias against the expensive error, so the unknown
/// case belongs on the restrictive side.
///
/// This gates a safety bar, not availability. `stage2_tools` fails open in the
/// other direction on purpose: there, an unknown name simply is not offered,
/// and stranding the model with no tools would be the worse outcome.
pub fn skill_is_mutating(candidate: &SkillCandidate) -> bool {
    candidate
        .tools
        .iter()
        .any(|t| Tool::from_name(t).is_none_or(Tool::is_write))
}

/// Whether a skill can irreversibly remove user data, derived from the tools
/// it may fire.
///
/// Computed from the registry's own classification for the same reason
/// [`skill_is_mutating`] is: adding `delete_node` to a whitelist raises that
/// skill's bar automatically, with no second field to keep in sync.
///
/// Unlike [`skill_is_mutating`], an unrecognised tool name counts as **not**
/// destructive — see [`super::tools::removes_user_data_tool`] for why the
/// unknown case belongs on the opposite side here. In short: an unknown name
/// is already treated as mutating, and treating it as destructive too would
/// apply the strictest bar in the system to any skill with a typo in its
/// whitelist.
pub fn skill_is_destructive(candidate: &SkillCandidate) -> bool {
    candidate
        .tools
        .iter()
        .any(|t| super::tools::removes_user_data_tool(t))
}

/// The retrieval score a candidate must clear to be actionable.
///
/// Scales with blast radius per ADR-038: read-only < mutating < destructive.
/// Checked most-restrictive-first, since a destructive skill is also a
/// mutating one and must not stop at the lower rung.
pub fn score_bar_for(candidate: &SkillCandidate) -> f32 {
    if skill_is_destructive(candidate) {
        DESTRUCTIVE_SKILL_SCORE_BAR
    } else if skill_is_mutating(candidate) {
        MUTATING_SKILL_SCORE_BAR
    } else {
        READ_SKILL_SCORE_BAR
    }
}

/// Whether a candidate clears the **mechanical** half of the Stage-2 gate.
///
/// Passing this is necessary, not sufficient: ADR-038 requires two
/// independently-sourced signals, and the model's judgment supplies the other.
/// A candidate that clears this bar is offered to the model to judge; one that
/// does not is never actionable regardless of what the model says.
pub fn clears_score_gate(candidate: &SkillCandidate) -> bool {
    candidate.score >= score_bar_for(candidate)
}

/// Names of the candidates that clear the score gate, comma-separated, for the
/// `routed_skills` log field.
///
/// The same set `render_candidates_for_prompt` writes into Stage 2's prompt and
/// `stage2_tools` scopes the tool surface from, so a log reader can tell which
/// skill a turn was actually routed to rather than only how many candidates
/// retrieval returned.
///
/// A function rather than an inline expression at the log site specifically so
/// the rendered text is testable. The scraper consuming this field parses to
/// end of line — skill names contain spaces and commas, and `tracing` emits the
/// value unquoted — so its exact shape is a contract, and the first version of
/// that scraper silently matched nothing because the shape was assumed rather
/// than asserted.
pub fn routed_skill_names(candidates: &[SkillCandidate]) -> String {
    candidates
        .iter()
        .filter(|c| clears_score_gate(c))
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every retrieved candidate's name and raw score, comma-separated, for the
/// `all_scores` log field — **not** filtered by [`clears_score_gate`], unlike
/// [`routed_skill_names`].
///
/// `routed_skill_names` and the OTel `routing.top_score` attribute together
/// answer "which skill won" and "how well did the winner score", but neither
/// distinguishes a tiebreak (the runner-up scored 0.81 against the winner's
/// 0.82) from a skill that scored badly on its own core use case (0.3, no
/// close competitor) — two defects with different fixes that look identical
/// from the winner's score alone. This is the field that tells them apart
/// from ordinary logs, without a dedicated measurement run.
///
/// Preserves retrieval's score-descending order (the order `route`'s `merged`
/// is already sorted into) rather than re-sorting, so the log line's order
/// matches rank.
pub fn all_candidate_scores(candidates: &[SkillCandidate]) -> String {
    candidates
        .iter()
        .map(|c| format!("{}={:.3}", c.name, c.score))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render retrieved candidates for injection into the Stage-2 prompt.
///
/// Delivered **in the prompt** rather than as a tool result. ADR-064 rule 4
/// reserves tool results for resolved facts rather than procedures, and the
/// supporting measurement for skill instructions was taken on prompt-rendered
/// text; the same payload returned as a tool result was observed to suppress
/// tool-calling substantially.
///
/// Only candidates clearing the score gate are rendered — the mechanical gate
/// runs before the judged one, so the model is never asked to judge a
/// candidate the system has already ruled out.
pub fn render_candidates_for_prompt(candidates: &[SkillCandidate]) -> Option<String> {
    let eligible: Vec<&SkillCandidate> =
        candidates.iter().filter(|c| clears_score_gate(c)).collect();
    if eligible.is_empty() {
        return None;
    }

    // Phrased as a direct instruction to act, not to deliberate. An earlier
    // wording ("Pick the ONE that fits and carry out its instructions") framed
    // the turn as a selection exercise, and the model answered it in kind:
    // it narrated which tool it would use — in one case emitting the tool-call
    // JSON inside a code block — instead of calling it. Measured on
    // mistral:7b, that wording produced zero tool calls where the unrouted
    // control produced one.
    let mut out = String::from(
        "REFERENCE — procedures relevant to this request. Use whichever applies and IGNORE the \
         rest. Do not describe, quote, or summarise any of it. Your reply must be the tool call \
         itself. If none applies, answer the user normally.\n",
    );
    for (i, c) in eligible.iter().enumerate() {
        out.push_str(&format!("\n--- Candidate {}: {}\n", i + 1, c.name));
        if !c.description.is_empty() {
            out.push_str(&format!("Purpose: {}\n", c.description));
        }
        if !c.instructions.is_empty() {
            out.push_str(&format!("\n{}\n", c.instructions));
        }
        if let Some(meta) = render_schema_metadata(&c.schema_metadata) {
            // Shared heading with context_ops.rs's resident workspace-context
            // block — see EXISTING_SCHEMAS_HEADER's doc comment. This
            // per-candidate rendering is a second, independent site that
            // shows the same schema metadata; a fix applied only to the
            // resident copy left this one to reinforce the exact
            // contamination the resident copy was changed to guard against.
            out.push_str(&format!("\n{EXISTING_SCHEMAS_HEADER}\n{meta}\n"));
        }
    }
    Some(out)
}

/// Wire names of tools that both require routed guidance (see
/// [`super::tools::Tool::requires_routed_guidance`]) and have that guidance
/// actually available: an eligible candidate whitelists the tool AND a
/// `EXISTING SCHEMAS` block will reach this turn's prompt.
///
/// The block reaches the prompt from **either** of two independent sites, and
/// the tool's required parameter cannot tell them apart — it points at a
/// heading, and one shared constant renders that heading at both:
///
/// - **Per-candidate**: this candidate's own `schema_metadata`, rendered into
///   the Stage-2 candidate block by [`render_candidates_for_prompt`].
/// - **Workspace context**: schemas retrieved semantically for the turn by
///   `context_ops::build_workspace_context`, already resident in the system
///   prompt (`agent_loop`'s `session.dynamic_context`).
///
/// Consulting only the first was a reachability bug rather than a
/// conservative approximation: `resolve_query` is whitelisted by a skill
/// (Graph Editing) that declares no `node_types`, so `skill_ops` falls back
/// to "all non-core schemas" for its `schema_metadata`. In a workspace with
/// no custom schema that fallback is empty, and the tool was withheld from
/// every turn — including turns whose workspace context *did* carry the
/// block. The gate's purpose is to never offer a tool whose required
/// parameter points at something the model cannot see; a block the model can
/// see satisfies that purpose whichever site rendered it.
///
/// Both checks require a type *listed*, not merely a heading: each renderer
/// can emit the heading over nothing — `render_candidates_for_prompt` from
/// its header text alone, and `context_ops`'s from stopping once its
/// character budget is spent. Either would strand the model with a required
/// `node_type` pointing at an empty list.
///
/// **Core types are out of scope by construction.** Every path that fills the
/// block today drops `is_core` schemas (`skill_ops`'s *unscoped* non-core
/// fallback — the branch that applies to `resolve_query`'s unscoped
/// whitelisting skill — and `context_ops::parse_and_filter_non_core_schemas`),
/// so a bare-value update against `task`/`text` renders no block from either
/// source and the tool stays withheld. That matches `resolve_query`'s own
/// description, whose examples are all custom-type (an amount, an invoice, a
/// code); it is a deliberate boundary, not an oversight this function should
/// paper over. See `def_resolve_query` for the one latent path that would
/// widen it.
///
/// `render_disabled: true` (mirrors the caller's `session.routing_disabled`)
/// suppresses only the *candidate* path — that flag exists because injecting
/// the candidate block suppresses tool-calling on some models, which says
/// nothing about the resident workspace block that is present regardless.
pub fn tools_with_available_guidance<'a>(
    candidates: &'a [SkillCandidate],
    render_disabled: bool,
    workspace_context: &str,
) -> std::collections::HashSet<&'a str> {
    // The heading alone is not the guidance — at least one type has to be
    // listed under it. `context_ops`'s renderer emits the heading and then
    // breaks out of its per-schema loop once the character budget is spent,
    // so a budget exhausted by the first line leaves a bare heading behind.
    // Matching on the heading alone would offer the tool while its required
    // `node_type` pointed at an empty list — precisely the strand this gate
    // exists to prevent.
    // Both renderers emit one `- <type_id>...` line per type (see
    // `entity_types_block::EntityTypeDescriptor::render_line`), so the first
    // non-blank line after the heading is the check.
    let workspace_has_block = workspace_context
        .split_once(EXISTING_SCHEMAS_HEADER)
        .and_then(|(_, after)| after.lines().find(|l| !l.trim().is_empty()))
        .is_some_and(|first| first.trim_start().starts_with("- "));
    candidates
        .iter()
        .filter(|c| clears_score_gate(c))
        .filter(|c| {
            // A whitelisting candidate still has to clear the score gate: the
            // workspace block makes the *parameter* answerable, it does not
            // make an unmatched skill's tools eligible.
            workspace_has_block
                || (!render_disabled && render_schema_metadata(&c.schema_metadata).is_some())
        })
        .flat_map(|c| c.tools.iter().map(|t| t.as_str()))
        .collect()
}

/// Render a candidate's `schema_metadata` into the compact form the model
/// already sees elsewhere, or `None` when there is nothing to show.
///
/// Delegates to the shared renderer in `nodespace-core`. This block and the
/// workspace-context one are concatenated into a single system prompt under the
/// same heading, so they must describe a type identically; they previously did
/// not, and the model followed guidance whose referent only one of them
/// emitted. The `schema_metadata` JSON this decodes is produced from the same
/// descriptor type, making the encode/decode one reversible mapping rather than
/// two hand-written projections.
fn render_schema_metadata(meta: &serde_json::Value) -> Option<String> {
    let descriptors = nodespace_core::ops::entity_types_block::descriptors_from_json(meta);
    nodespace_core::ops::entity_types_block::render_entity_types(&descriptors)
}

/// Wire names the eligible candidates permit, with the destructive-tool rule
/// applied. The single source of truth for that rule: [`stage2_tools`] scopes
/// from it and [`destructive_tools_withheld`] reports against it, so the log
/// cannot claim something different from what the model was offered.
fn stage2_permitted_names(candidates: &[SkillCandidate]) -> std::collections::HashSet<&str> {
    // Computed by explicit max rather than taking `candidates[0]`: the caller
    // does sort by score descending before truncating to RETRIEVAL_TOP_K
    // (`agent_loop`'s `route`), but a safety property should not depend on
    // another function's ordering staying that way. Ties keep every candidate
    // at the top score, which is the same treatment `declare_write_tool_fields`
    // gives them.
    let top_score = candidates
        .iter()
        .filter(|c| clears_score_gate(c))
        .map(|c| c.score)
        .fold(f32::NEG_INFINITY, f32::max);

    candidates
        .iter()
        .filter(|c| clears_score_gate(c))
        .flat_map(|c| {
            let is_top = c.score >= top_score;
            c.tools
                .iter()
                .map(|t| t.as_str())
                .filter(move |t| is_top || !super::tools::removes_user_data_tool(t))
        })
        .collect()
}

/// The tools Stage 2 may offer, restricted to what the eligible candidates'
/// whitelists permit: the union of them for ordinary tools, and — for tools
/// that irreversibly remove user data — only the whitelist of the candidate
/// that actually won retrieval. See [`stage2_permitted_names`] for why.
///
/// This is the trust boundary ADR-038 places on the system side: the model
/// judges *among* what retrieval surfaced, and can only fire what the matched
/// skills permit. A skill's `tool_whitelist` was previously read only by
/// external ACP agents; this gives it a local-agent consumer.
///
/// Falls back to the full surface when nothing was retrieved, so an
/// unavailable embedding service degrades to today's behaviour rather than
/// leaving the model with no tools at all — minus any tool whose required
/// parameters depend on the `EXISTING SCHEMAS` block (see
/// [`super::tools::Tool::requires_routed_guidance`] and
/// [`fail_open_surface`]). That exclusion is about *eligibility*, not about
/// the block being absent: workspace context renders it independently of
/// routing, so it may well be present on a fail-open turn. But fail-open
/// means retrieval matched nothing, and ADR-038 puts the trust boundary at
/// what retrieval surfaced. The model still has every other tool to answer
/// the request with.
pub fn stage2_tools(candidates: &[SkillCandidate], all: &[ToolDefinition]) -> Vec<ToolDefinition> {
    // The union is right for ordinary tools and wrong for destructive ones.
    // A skill contributes its whole whitelist to the surface merely by being
    // one of the (up to three) candidates above its bar — so a weak
    // second/third-place `Node Deletion` put `delete_node` in front of the
    // model on requests that were recording a decision or setting a due date,
    // while contributing nothing the turn actually needed. That is the
    // opposite of ADR-038's "the expensive error is gated hardest": riding
    // along in retrieval cost the destructive skill nothing.
    //
    // So destructive tools are admitted only from the candidate that actually
    // *won* retrieval this turn. A skill that best matches a deletion request
    // still offers `delete_node`; one that merely placed does not.
    //
    // Note this narrows only which candidates may contribute a destructive
    // tool. It is not a second score gate — the winner still had to clear
    // `DESTRUCTIVE_SKILL_SCORE_BAR` to be eligible at all.
    let permitted = stage2_permitted_names(candidates);

    if permitted.is_empty() {
        return fail_open_surface(all);
    }
    let scoped: Vec<ToolDefinition> = all
        .iter()
        .filter(|t| permitted.contains(t.name.as_str()))
        .cloned()
        .collect();

    // A whitelist naming only tools this build does not register would strand
    // the model with nothing to call. Fail open to the full surface.
    if scoped.is_empty() {
        return fail_open_surface(all);
    }
    scoped
}

/// Names of destructive tools that a gate-clearing candidate whitelisted but
/// [`stage2_tools`] withheld, because that candidate did not win retrieval.
///
/// Purely for the log line — it re-reads the same rule `stage2_tools` applies
/// rather than reimplementing it, so the two cannot disagree about what was
/// withheld. Empty on the overwhelming majority of turns.
///
/// This exists because the failure it reports is otherwise invisible. When
/// scoping removes the tool a turn needed, the model has no evidence a better
/// tool ever existed: best case it asks the user a confused clarifying
/// question, worst case it reaches for the wrong tool from the narrowed set.
/// Neither the daemon log nor the turn output distinguished that from a model
/// that simply failed to call the tool, so a routing defect read as a model
/// defect.
/// Destructive candidates that retrieval returned but
/// [`DESTRUCTIVE_SKILL_SCORE_BAR`] rejected, as `(name, score)`.
///
/// Distinct from [`destructive_tools_withheld`], which reports a skill that
/// *was* eligible but did not win. This reports one that never became
/// eligible: it would have cleared the mutating bar and been actionable before
/// the destructive rung existed.
///
/// Exists to make a specific, acknowledged risk measurable in the field
/// instead of waiting for a user to report it. Two changes landed together
/// that push the same quantity in opposite directions: the bar a deletion
/// match must clear went **up** to `DESTRUCTIVE_SKILL_SCORE_BAR`, while the
/// `Node Deletion` description was narrowed — and a narrower description
/// embeds *further* from an indirectly-phrased real request ("get rid of that
/// old meeting note"). Both moves are individually justified and the bar is a
/// documented placeholder, but neither was measured against live embeddings,
/// so the band a genuine deletion has to land in is not known.
///
/// A turn appearing here is the signal that the bar is too high: the user
/// asked for something the system read as deletion, and it was dropped
/// entirely rather than offered. Scores logged alongside the names so the
/// distribution can be read off existing logs rather than needing a new eval
/// run to discover it.
pub fn destructive_candidates_below_bar(candidates: &[SkillCandidate]) -> Vec<(&str, f32)> {
    candidates
        .iter()
        .filter(|c| skill_is_destructive(c) && !clears_score_gate(c))
        .map(|c| (c.name.as_str(), c.score))
        .collect()
}

pub fn destructive_tools_withheld(candidates: &[SkillCandidate]) -> Vec<&str> {
    let offered: std::collections::HashSet<&str> = stage2_permitted_names(candidates);
    let mut withheld: Vec<&str> = candidates
        .iter()
        .filter(|c| clears_score_gate(c))
        .flat_map(|c| c.tools.iter().map(|t| t.as_str()))
        .filter(|t| super::tools::removes_user_data_tool(t))
        .filter(|t| !offered.contains(t))
        .collect();
    withheld.sort_unstable();
    withheld.dedup();
    withheld
}

/// Declares `field_values` sub-properties (see
/// [`super::tools::with_declared_field_values`]) on any tool in `tools`
/// shaped for it, sourced from the retrieved-schema data of whichever
/// candidate(s) attain the TURN'S GLOBAL top score among those that cleared
/// the gate — not the top score among only the candidates whitelisting that
/// specific tool. Precise about this because the two read very differently:
/// a tool whitelisted ONLY by a candidate that is not (one of) the turn's
/// overall highest scorer(s) is declared from **no** descriptors and stays
/// on the bare-object fallback, even if that candidate is the sole one
/// offering the tool at all.
///
/// `dev-schema-creation.toml` (`packages/agent/goldens/`) is why this is the
/// global max rather than a per-tool one: it offers `create_node` on the
/// same turn as `create_schema` specifically so the model can choose
/// wrongly, and measures it never called. `create_node` there is
/// whitelisted only by the lower-scoring Node Creation candidate — a
/// per-tool max would find Node Creation as "the top scorer among
/// create_node's own whitelisters" (trivially, being the only one) and
/// declare its fields anyway, making the wrong tool easier to use on
/// exactly the turn that must not reward it. The global-max rule is what
/// actually excludes it: `create_node` only gets declared fields when a
/// candidate whitelisting it ALSO happens to be the turn's overall best
/// match, not merely the best match among candidates that want that
/// specific tool.
///
/// **The known cost of that choice**, caught in review: on a genuinely
/// compound turn — two independently-relevant skills clear the gate for two
/// DIFFERENT, non-competing tools, e.g. a request that both creates a
/// ticket and links it to a sprint — `stage2_tools` legitimately offers
/// both tools (it unions every cleared candidate's whitelist, uncapped by
/// score), but this function will decline to declare fields for whichever
/// tool's candidate is not the turn's single highest scorer, even though
/// that candidate is in no sense a distractor. That turn's lower-priority
/// tool falls back to the pre-existing bare-object-plus-prose shape — not a
/// regression relative to today's production (every write tool is on that
/// shape today), just a missed improvement for that turn. Nothing in the
/// corpus exercises a genuine two-different-tools compound turn, so there is
/// no measured guidance on how to tell it apart mechanically from the
/// distractor case using only candidate score and tool whitelist — doing so
/// well likely needs per-skill retrieval scoping (ranking each tool's
/// candidates independently rather than by one global score) to distinguish
/// "beaten by a better candidate for the same tool" from "a different tool
/// entirely, just scored lower."
/// Pinned by `declare_write_tool_fields_does_not_declare_a_non_top_scoring_but_uncontested_tool`
/// below so this is a documented, deliberate trade-off rather than a latent
/// surprise.
///
/// Ties at the top score are unioned rather than one arbitrarily shadowing
/// the other (see `tools::declared_field_values_properties`) — the fixture
/// this snapshot gate exercises scores its two candidates identically on
/// purpose, and both legitimately whitelist `create_node`.
///
/// Deliberately a separate step from [`stage2_tools`], not folded into it —
/// this injects retrieved-schema CONTENT into the tool surface, the same
/// class of payload `agent_loop.rs`'s `candidate_block` skips injecting into
/// the prompt when `session.routing_disabled`. The routing-reliability
/// matrix (`tests/live_openai_compat_routing.rs`) found that injecting that
/// content suppressed tool-calling outright on some served models,
/// independent of the block's content — the finding was about *any*
/// retrieved-schema payload reaching the model, not specifically the
/// prompt-text channel it happened to be measured through. `stage2_tools`'s
/// own tool-list SCOPING (which tools are offered at all) is a different,
/// already-proven-safe mechanism and stays unconditional; this CONTENT step
/// is the caller's responsibility to gate the same way `candidate_block` is,
/// so a model probed unsafe for one retrieved-schema channel is not handed
/// materially the same content through another, unmeasured one.
pub fn declare_write_tool_fields(
    candidates: &[SkillCandidate],
    tools: Vec<ToolDefinition>,
) -> Vec<ToolDefinition> {
    let cleared: Vec<&SkillCandidate> =
        candidates.iter().filter(|c| clears_score_gate(c)).collect();
    let max_score = cleared.iter().map(|c| c.score).fold(f32::MIN, f32::max);
    tools
        .into_iter()
        .map(|tool| {
            let descriptors: Vec<_> = cleared
                .iter()
                .filter(|c| c.score >= max_score && c.tools.iter().any(|t| t == &tool.name))
                .flat_map(|c| {
                    nodespace_core::ops::entity_types_block::descriptors_from_json(
                        &c.schema_metadata,
                    )
                })
                .collect();
            super::tools::with_declared_field_values(tool, &descriptors)
        })
        .collect()
}

/// The full tool surface, minus tools whose required parameters depend on the
/// `EXISTING SCHEMAS` block. Shared by both fail-open branches of
/// [`stage2_tools`] so they can't drift apart.
///
/// The exclusion is an *eligibility* judgement, not a claim that the block is
/// absent. Workspace context can render it independently of routing (see
/// [`tools_with_available_guidance`]), so on this path the block may well be
/// in the prompt. But fail-open means retrieval matched nothing: no skill
/// vouched for this turn, and ADR-038 puts the trust boundary at what
/// retrieval surfaced. Handing over a tool whose whole purpose is resolving
/// an ambiguous reference — when the system could not even identify which
/// capability the request needs — widens the surface at exactly the moment
/// there is least reason to trust it. Every other tool remains available.
fn fail_open_surface(all: &[ToolDefinition]) -> Vec<ToolDefinition> {
    all.iter()
        .filter(|t| !super::tools::requires_routed_guidance_tool(&t.name))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn candidate(name: &str, score: f32, tools: &[&str]) -> SkillCandidate {
        SkillCandidate {
            id: format!("skill-{name}"),
            name: name.to_string(),
            description: format!("{name} description"),
            score,
            tools: tools.iter().map(|t| t.to_string()).collect(),
            instructions: format!("{name} instructions"),
            schema_metadata: json!([]),
        }
    }

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters_schema: json!({}),
        }
    }

    #[test]
    fn stage1_offers_exactly_the_three_routing_tools() {
        let defs = stage1_tool_definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec![ROUTE_QUERY_TOOL, ROUTE_CLARIFY_TOOL, ROUTE_MULTI_TOOL]
        );
    }

    #[test]
    fn route_query_parses_into_a_query_decision() {
        let d = parse_route_decision(ROUTE_QUERY_TOOL, r#"{"query":"create a schema"}"#);
        assert_eq!(d, Some(RouteDecision::Query("create a schema".into())));
    }

    #[test]
    fn route_clarify_parses_question_and_options() {
        let d = parse_route_decision(
            ROUTE_CLARIFY_TOOL,
            r#"{"question":"Which did you mean?","options":["Track debts","Search notes"]}"#,
        );
        assert_eq!(
            d,
            Some(RouteDecision::Clarify {
                question: "Which did you mean?".into(),
                options: vec!["Track debts".into(), "Search notes".into()],
            })
        );
    }

    #[test]
    fn clarify_without_options_still_parses() {
        // `options` is optional on the wire so a model omitting it does not
        // cost the turn; the contract's "be specific" requirement is enforced
        // by the tool description, not by rejecting the call.
        let d = parse_route_decision(ROUTE_CLARIFY_TOOL, r#"{"question":"Which one?"}"#);
        assert!(matches!(d, Some(RouteDecision::Clarify { .. })));
    }

    #[test]
    fn unknown_tool_or_blank_query_yields_no_decision() {
        assert!(parse_route_decision("search_nodes", r#"{"query":"x"}"#).is_none());
        assert!(parse_route_decision(ROUTE_QUERY_TOOL, r#"{"query":"   "}"#).is_none());
        assert!(parse_route_decision(ROUTE_QUERY_TOOL, "not json").is_none());
    }

    #[test]
    fn route_multi_parses_two_or_more_queries() {
        let d = parse_route_decision(
            ROUTE_MULTI_TOOL,
            r#"{"queries":["log a $42 lunch expense","remind me to follow up with Sarah Friday"]}"#,
        );
        assert_eq!(
            d,
            Some(RouteDecision::Multi(vec![
                "log a $42 lunch expense".into(),
                "remind me to follow up with Sarah Friday".into(),
            ]))
        );
    }

    #[test]
    fn route_multi_with_fewer_than_two_usable_queries_yields_no_decision() {
        // A single-element array is not a genuine compound intent — the model
        // over-called route_multi for what should have been route_query.
        // Falling through to "no decision" (unrouted retrieval on the raw
        // message) beats trusting a one-element "multi".
        assert!(parse_route_decision(ROUTE_MULTI_TOOL, r#"{"queries":["one thing"]}"#).is_none());
        // Blank entries don't count toward the two-or-more requirement.
        assert!(
            parse_route_decision(ROUTE_MULTI_TOOL, r#"{"queries":["one thing","   "]}"#).is_none()
        );
        assert!(parse_route_decision(ROUTE_MULTI_TOOL, r#"{"queries":[]}"#).is_none());
    }

    #[test]
    fn route_multi_trims_and_drops_blank_entries_among_valid_ones() {
        let d = parse_route_decision(
            ROUTE_MULTI_TOOL,
            r#"{"queries":["  log expense  ","","  set reminder  "]}"#,
        );
        assert_eq!(
            d,
            Some(RouteDecision::Multi(vec![
                "log expense".into(),
                "set reminder".into(),
            ]))
        );
    }

    #[test]
    fn blast_radius_derives_from_the_tool_whitelist() {
        assert!(!skill_is_mutating(&candidate(
            "research",
            0.5,
            &["search_nodes", "get_node"]
        )));
        assert!(skill_is_mutating(&candidate(
            "schema",
            0.5,
            &["create_schema", "get_node"]
        )));
        // A single write tool among reads is enough to raise the bar.
        assert!(skill_is_mutating(&candidate(
            "deletion",
            0.5,
            &["search_nodes", "delete_node"]
        )));
    }

    #[test]
    fn an_unrecognised_whitelist_tool_is_treated_as_mutating() {
        // Fail-safe: a typo, a renamed tool, or a future externally-registered
        // one is of unknown blast radius, so it takes the higher bar rather
        // than defaulting to read-only.
        let ghost = candidate("ghost", 0.2, &["delete_everything_v2"]);
        assert!(skill_is_mutating(&ghost));
        assert!(
            !clears_score_gate(&ghost),
            "0.2 clears the read bar but must not clear the mutating one"
        );
    }

    #[test]
    fn mutating_skills_carry_a_strictly_higher_bar() {
        // Compile-time: ADR-038 requires the expensive error — firing the
        // wrong *mutating* tool — to be gated harder than a wrong read. Tuning
        // the values is expected; inverting the ordering is not, so it fails
        // the build rather than a test run.
        const _: () = assert!(MUTATING_SKILL_SCORE_BAR > READ_SKILL_SCORE_BAR);
        let read = candidate("research", 0.2, &["search_nodes"]);
        let write = candidate("schema", 0.2, &["create_schema"]);
        // Identical score, different verdict — the bar is a property of the
        // matched skill, not a single global constant.
        assert!(clears_score_gate(&read));
        assert!(!clears_score_gate(&write));
    }

    #[test]
    fn destructive_skills_carry_a_strictly_higher_bar_than_other_mutations() {
        // Compile-time, matching the read/mutating assertion above: the values
        // are expected to be tuned, the ladder's ordering is not.
        const _: () = assert!(DESTRUCTIVE_SKILL_SCORE_BAR > MUTATING_SKILL_SCORE_BAR);

        // Identical score, three different verdicts — one rung per blast radius.
        let read = candidate("research", 0.35, &["search_nodes"]);
        let mutating = candidate("editing", 0.35, &["update_node"]);
        let destructive = candidate("deletion", 0.35, &["delete_node"]);
        assert!(clears_score_gate(&read));
        assert!(clears_score_gate(&mutating));
        assert!(
            !clears_score_gate(&destructive),
            "a skill that can irreversibly remove user data must clear a higher bar than one \
             that merely writes"
        );
    }

    #[test]
    fn a_destructive_skill_that_only_places_cannot_put_delete_node_in_reach() {
        // The #2240 regression. `Node Deletion` was retrieved as a candidate on
        // turns that recorded a decision, marked a status, and set a due date.
        // Because the surface was the union across every eligible candidate, it
        // contributed `delete_node` merely by placing — while the tool the turn
        // actually needed was absent unless some other candidate happened to
        // whitelist it.
        let all = vec![
            tool("create_node"),
            tool("search_nodes"),
            tool("delete_node"),
        ];
        let cands = vec![
            candidate("Node Creation", 0.8, &["create_node", "search_nodes"]),
            candidate("Node Deletion", 0.5, &["delete_node", "search_nodes"]),
        ];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();

        assert!(
            names.contains(&"create_node"),
            "the winning skill's own tool must still be offered: {names:?}"
        );
        assert!(
            !names.contains(&"delete_node"),
            "a destructive skill that placed but did not win retrieval must not put delete_node \
             in reach: {names:?}"
        );
        // The runner-up's non-destructive tools are unaffected — this narrows
        // destructive admission only, it does not drop losing candidates.
        assert!(names.contains(&"search_nodes"));
    }

    #[test]
    fn a_destructive_skill_that_wins_retrieval_still_offers_delete_node() {
        // The regression the change could plausibly cause. Deletion must keep
        // working for requests that actually ask for it.
        let all = vec![tool("create_node"), tool("delete_node"), tool("get_node")];
        let cands = vec![
            candidate("Node Deletion", 0.7, &["delete_node", "get_node"]),
            candidate("Node Creation", 0.5, &["create_node"]),
        ];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"delete_node"),
            "a genuine deletion match must still be able to delete: {names:?}"
        );
    }

    #[test]
    fn destructive_admission_does_not_depend_on_candidate_order() {
        // `stage2_tools` picks the winner by explicit max, so a caller that
        // stopped sorting by score cannot silently widen the destructive
        // surface.
        let all = vec![tool("create_node"), tool("delete_node")];
        let ascending = vec![
            candidate("Node Deletion", 0.5, &["delete_node"]),
            candidate("Node Creation", 0.8, &["create_node"]),
        ];
        let descending = vec![
            candidate("Node Creation", 0.8, &["create_node"]),
            candidate("Node Deletion", 0.5, &["delete_node"]),
        ];
        let names = |c: &[SkillCandidate]| -> Vec<String> {
            let mut n: Vec<String> = stage2_tools(c, &all)
                .iter()
                .map(|t| t.name.clone())
                .collect();
            n.sort();
            n
        };
        assert_eq!(names(&ascending), names(&descending));
        assert!(!names(&ascending).contains(&"delete_node".to_string()));
    }

    #[test]
    fn a_lone_destructive_candidate_is_never_left_with_an_empty_surface() {
        // It wins by default (it is the only eligible candidate), so nothing is
        // withheld and the existing fail-open branches stay untouched.
        let all = vec![tool("delete_node"), tool("create_node")];
        let cands = vec![candidate("Node Deletion", 0.9, &["delete_node"])];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["delete_node"]);
    }

    #[test]
    fn a_destructive_skill_below_its_bar_is_ineligible_everywhere() {
        // `clears_score_gate` is consulted independently by three sites, so the
        // rank-1 half of the fix has to hold at each of them rather than only
        // at the one that scopes tools.
        let all = vec![tool("delete_node"), tool("search_nodes")];
        // Above MUTATING (0.30), below DESTRUCTIVE — the band the rank-1
        // `Node Deletion` turns landed in.
        let cands = vec![candidate("Node Deletion", 0.35, &["delete_node"])];

        assert_eq!(
            routed_skill_names(&cands),
            "",
            "must not be logged as routed"
        );
        assert!(
            render_candidates_for_prompt(&cands).is_none(),
            "must not be rendered into the Stage-2 prompt"
        );
        // No eligible candidate at all -> the existing fail-open branch, not a
        // scoped surface built from an ineligible skill.
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"search_nodes"));
    }

    #[test]
    fn an_unrecognised_tool_name_is_mutating_but_not_destructive() {
        // Deliberate asymmetry — see `removes_user_data_tool`. An unknown name
        // must not attract the strictest bar in the system, and a later
        // "consistency" cleanup that flips it would quietly raise the bar on
        // every skill with a typo'd whitelist.
        let unknown = candidate("plugin", 0.9, &["some_external_tool"]);
        assert!(skill_is_mutating(&unknown));
        assert!(!skill_is_destructive(&unknown));
        assert_eq!(score_bar_for(&unknown), MUTATING_SKILL_SCORE_BAR);
    }

    #[test]
    fn destructive_candidates_rejected_by_the_bar_are_reported_for_the_log() {
        // The band between the mutating and destructive bars — where a real
        // deletion request would be silently dropped if the bar is too high.
        let cands = vec![
            candidate("Node Deletion", 0.35, &["delete_node"]),
            candidate("Node Creation", 0.9, &["create_node"]),
        ];
        let rejected = destructive_candidates_below_bar(&cands);
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0, "Node Deletion");

        // A deletion match that clears the bar is not "rejected" — it is
        // simply routed, and must not show up as evidence the bar is wrong.
        let ok = vec![candidate("Node Deletion", 0.9, &["delete_node"])];
        assert!(destructive_candidates_below_bar(&ok).is_empty());

        // Nor does a non-destructive skill below its own (lower) bar.
        let unrelated = vec![candidate("Research & Search", 0.01, &["search_nodes"])];
        assert!(destructive_candidates_below_bar(&unrelated).is_empty());
    }

    #[test]
    fn withheld_destructive_tools_are_reported_for_the_log() {
        let placed = vec![
            candidate("Node Creation", 0.8, &["create_node"]),
            candidate("Node Deletion", 0.5, &["delete_node"]),
        ];
        assert_eq!(destructive_tools_withheld(&placed), vec!["delete_node"]);

        // Nothing withheld when the destructive skill wins, and nothing
        // withheld on an ordinary turn — this must stay quiet in the log.
        let won = vec![
            candidate("Node Deletion", 0.8, &["delete_node"]),
            candidate("Node Creation", 0.5, &["create_node"]),
        ];
        assert!(destructive_tools_withheld(&won).is_empty());
        let ordinary = vec![candidate("Node Creation", 0.8, &["create_node"])];
        assert!(destructive_tools_withheld(&ordinary).is_empty());
    }

    #[test]
    fn candidates_below_their_bar_are_never_rendered() {
        let cands = vec![
            candidate("weak", 0.01, &["search_nodes"]),
            candidate("strong", 0.9, &["search_nodes"]),
        ];
        let rendered = render_candidates_for_prompt(&cands).expect("one candidate is eligible");
        assert!(rendered.contains("strong"));
        assert!(!rendered.contains("weak instructions"));
    }

    #[test]
    fn routed_skill_names_lists_only_gate_clearing_candidates() {
        let cands = vec![
            candidate("Research & Search", 0.9, &["search_nodes"]),
            candidate("Below The Bar", 0.01, &["search_nodes"]),
            candidate("Node Creation", 0.9, &["search_nodes"]),
        ];
        assert_eq!(
            routed_skill_names(&cands),
            "Research & Search, Node Creation"
        );
    }

    #[test]
    fn routed_skill_names_is_empty_when_nothing_clears_the_gate() {
        // Distinct from "retrieval returned nothing" only in the candidate
        // count logged alongside it; the scraper omits the marker either way.
        let cands = vec![candidate("weak", 0.001, &["create_schema"])];
        assert_eq!(routed_skill_names(&cands), "");
    }

    #[test]
    fn routed_skill_names_matches_what_is_rendered_into_the_prompt() {
        // The field exists to answer "which skill was this turn routed to", so
        // it has to name the same set the model actually sees. Asserting the
        // two agree keeps the log honest if either filter later changes.
        let cands = vec![
            candidate("Graph Editing", 0.9, &["search_nodes"]),
            candidate("Excluded", 0.001, &["search_nodes"]),
        ];
        let rendered = render_candidates_for_prompt(&cands).expect("one is eligible");
        for name in routed_skill_names(&cands).split(", ") {
            assert!(
                rendered.contains(name),
                "{name} logged as routed but absent from the prompt block"
            );
        }
        assert!(!rendered.contains("Excluded"));
    }

    #[test]
    fn no_eligible_candidates_renders_nothing() {
        let cands = vec![candidate("weak", 0.001, &["create_schema"])];
        assert!(render_candidates_for_prompt(&cands).is_none());
    }

    #[test]
    fn all_candidate_scores_includes_every_candidate_regardless_of_the_gate() {
        // Unlike `routed_skill_names`, a below-bar candidate must still
        // appear — this field exists specifically to show the scores the
        // gate filtered out, so a tiebreak or a low-scoring-with-no-
        // competitor situation is distinguishable from the log alone.
        let cands = vec![
            candidate("Research & Search", 0.9, &["search_nodes"]),
            candidate("Below The Bar", 0.01, &["search_nodes"]),
        ];
        assert_eq!(
            all_candidate_scores(&cands),
            "Research & Search=0.900, Below The Bar=0.010"
        );
    }

    #[test]
    fn all_candidate_scores_is_empty_when_retrieval_returned_nothing() {
        assert_eq!(all_candidate_scores(&[]), "");
    }

    #[test]
    fn rendered_candidates_carry_their_instruction_subtree() {
        let cands = vec![candidate("Schema Creation", 0.9, &["create_schema"])];
        let rendered = render_candidates_for_prompt(&cands).unwrap();
        assert!(rendered.contains("Schema Creation instructions"));
        assert!(rendered.contains("Purpose: Schema Creation description"));
    }

    #[test]
    fn stage2_tools_are_scoped_to_eligible_candidate_whitelists() {
        let all = vec![
            tool("search_nodes"),
            tool("get_node"),
            tool("create_schema"),
            tool("delete_node"),
        ];
        let cands = vec![candidate("research", 0.9, &["search_nodes", "get_node"])];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["search_nodes", "get_node"]);
        assert!(
            !names.contains(&"delete_node"),
            "a read-only skill must not put a destructive tool in reach"
        );
    }

    /// A tool shaped like `create_node`/`update_node` — a `field_values`
    /// object parameter alongside others — for exercising
    /// `declare_write_tool_fields` without depending on the real registry.
    fn write_tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "field_values": { "type": "object" }
                }
            }),
        }
    }

    fn schema_metadata_for(type_id: &str, fields: &[(&str, &str)]) -> serde_json::Value {
        use nodespace_core::ops::entity_types_block::{
            EntityFieldDescriptor, EntityTypeDescriptor,
        };
        json!([EntityTypeDescriptor {
            type_id: type_id.to_string(),
            name: Some(type_id.to_string()),
            fields: fields
                .iter()
                .map(|(name, field_type)| EntityFieldDescriptor {
                    name: name.to_string(),
                    field_type: field_type.to_string(),
                    enum_values: Vec::new(),
                    required: false,
                })
                .collect(),
            title_template: None,
        }
        .to_json()])
    }

    fn field_values_property_names(tool: &ToolDefinition) -> Vec<String> {
        tool.parameters_schema["properties"]["field_values"]["properties"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The single-candidate case: its own retrieved schema declares the
    /// write tool's `field_values` fields. Calls `declare_write_tool_fields`
    /// directly, not through `stage2_tools` — the two are deliberately
    /// separate steps (see `declare_write_tool_fields`'s doc comment), and a
    /// production turn only reaches this one when `!routing_disabled`.
    #[test]
    fn declare_write_tool_fields_declares_from_the_sole_candidates_schema() {
        let mut c = candidate("Node Creation", 0.85, &["create_node"]);
        c.schema_metadata =
            schema_metadata_for("ticket", &[("status", "text"), ("assignee", "text")]);
        let declared = declare_write_tool_fields(&[c], vec![write_tool("create_node")]);
        let names = field_values_property_names(&declared[0]);
        assert!(names.contains(&"status".to_string()));
        assert!(names.contains(&"assignee".to_string()));
    }

    /// The distractor case `dev-schema-creation.toml` (packages/agent/goldens/)
    /// measures: a write tool whitelisted only by a LOWER-scoring candidate
    /// than the turn's actual top match must stay on the bare-object
    /// fallback, not gain declarations that would make the wrong tool easier
    /// to reach for.
    #[test]
    fn declare_write_tool_fields_leaves_a_lower_scored_candidates_tool_bare() {
        let mut top = candidate("Schema Creation", 0.9, &["create_schema"]);
        top.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);
        let mut distractor = candidate("Node Creation", 0.5, &["create_node"]);
        distractor.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);

        let declared = declare_write_tool_fields(
            &[top, distractor],
            vec![write_tool("create_schema"), write_tool("create_node")],
        );
        let create_node = declared
            .iter()
            .find(|t| t.name == "create_node")
            .expect("create_node must still be present in the input list");
        assert!(
            field_values_property_names(create_node).is_empty(),
            "the distractor's tool must not receive field declarations"
        );
    }

    /// The documented trade-off from `declare_write_tool_fields`'s doc
    /// comment, pinned rather than left as an untested claim: two
    /// candidates that whitelist two DIFFERENT tools (not competing for the
    /// same tool, unlike the distractor case above) still leave the
    /// lower-scored candidate's tool undeclared, because the selection is
    /// the turn's GLOBAL top score, not a per-tool one. This is deliberate
    /// (see the doc comment for why the distractor case requires it), but
    /// it means a genuinely compound turn — both tools legitimately wanted
    /// — only gets one of them declared. Falls back to the bare-object
    /// shape for the other, not a regression relative to production before
    /// per-candidate field declaration existed, just an unrealized
    /// improvement: per-skill retrieval scoping (see the doc comment above)
    /// would let both tools' top candidates get their fields declared
    /// independently instead of only the turn's single global top scorer.
    #[test]
    fn declare_write_tool_fields_does_not_declare_a_non_top_scoring_but_uncontested_tool() {
        let mut top = candidate("Relationship Management", 0.9, &["create_relationship"]);
        top.schema_metadata = schema_metadata_for("adr", &[("supersedes", "text")]);
        let mut other = candidate("Graph Editing", 0.6, &["update_node"]);
        other.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);

        let declared = declare_write_tool_fields(
            &[top, other],
            vec![write_tool("create_relationship"), write_tool("update_node")],
        );
        let update_node = declared
            .iter()
            .find(|t| t.name == "update_node")
            .expect("update_node must still be present in the input list");
        assert!(
            field_values_property_names(update_node).is_empty(),
            "documented trade-off: a lower-scored candidate's own, uncontested tool still \
             stays undeclared under the global-max rule — if this now fails because the rule \
             changed to a per-tool max, re-verify declare_write_tool_fields_leaves_a_lower_scored_candidates_tool_bare \
             (the distractor case) still passes, since the two tests pull the rule in opposite directions"
        );
    }

    /// Two candidates tied at the top score both whitelisting the same tool:
    /// their schemas are unioned rather than one arbitrarily winning — the
    /// snapshot-gate fixture (`prompt_assembly_snapshot.rs`) scores "Node
    /// Creation" and "Schema Creation" identically on purpose.
    #[test]
    fn declare_write_tool_fields_unions_tied_top_scoring_candidates() {
        let mut a = candidate("A", 0.85, &["create_node"]);
        a.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);
        let mut b = candidate("B", 0.85, &["create_node"]);
        b.schema_metadata = schema_metadata_for("adr", &[("supersedes", "text")]);

        let declared = declare_write_tool_fields(&[a, b], vec![write_tool("create_node")]);
        let names = field_values_property_names(&declared[0]);
        assert!(names.contains(&"status".to_string()));
        assert!(names.contains(&"supersedes".to_string()));
    }

    /// No cleared candidate must leave every tool's `field_values` on the
    /// static bare-object fallback — there is no retrieved schema to declare
    /// from, and the function must not panic or fabricate a declaration on
    /// an empty candidate list (this exercises `declare_write_tool_fields`
    /// directly with `&[]`, unlike `stage2_tools`'s own fail-open path,
    /// which never reaches this function at all — see
    /// `an_ineligible_candidate_does_not_widen_the_tool_surface` and
    /// neighbouring tests below for `stage2_tools`'s fail-open behaviour).
    #[test]
    fn declare_write_tool_fields_is_a_no_op_with_no_cleared_candidates() {
        let declared =
            declare_write_tool_fields(&[], vec![write_tool("create_node"), tool("search_nodes")]);
        let create_node = declared
            .iter()
            .find(|t| t.name == "create_node")
            .expect("create_node must still be present in the input list");
        assert!(field_values_property_names(create_node).is_empty());
    }

    /// `agent_loop.rs` gates this function behind `!session.routing_disabled`
    /// the same way it gates `candidate_block` prompt injection — this pins
    /// that `stage2_tools` itself never calls it, so a caller that forgets
    /// the gate cannot accidentally get it "for free" from tool scoping.
    #[test]
    fn stage2_tools_alone_never_declares_field_values() {
        let mut c = candidate("Node Creation", 0.85, &["create_node"]);
        c.schema_metadata = schema_metadata_for("ticket", &[("status", "text")]);
        let scoped = stage2_tools(&[c], &[write_tool("create_node")]);
        assert!(
            field_values_property_names(&scoped[0]).is_empty(),
            "stage2_tools must not itself declare field_values — that is declare_write_tool_fields's job, gated separately by the caller"
        );
    }

    #[test]
    fn an_ineligible_candidate_does_not_widen_the_tool_surface() {
        let all = vec![tool("search_nodes"), tool("delete_node")];
        // Deletion is mutating, so 0.2 is below its bar though above the read bar.
        let cands = vec![
            candidate("research", 0.9, &["search_nodes"]),
            candidate("deletion", 0.2, &["delete_node"]),
        ];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["search_nodes"]);
    }

    #[test]
    fn empty_retrieval_falls_back_to_the_full_tool_surface() {
        let all = vec![tool("search_nodes"), tool("create_schema")];
        assert_eq!(stage2_tools(&[], &all).len(), 2);
    }

    #[test]
    fn whitelist_naming_only_unregistered_tools_fails_open() {
        let all = vec![tool("search_nodes")];
        let cands = vec![candidate("ghost", 0.9, &["tool_that_does_not_exist"])];
        assert_eq!(
            stage2_tools(&cands, &all).len(),
            1,
            "stranding the model with zero tools is worse than a wide surface"
        );
    }

    #[test]
    fn fail_open_on_empty_retrieval_excludes_resolve_query() {
        // No candidates at all — the emptiest fail-open case. resolve_query's
        // required node_type parameter depends on the EXISTING SCHEMAS
        // block, which only renders alongside a scoped whitelist; handing the
        // tool over here would strand the model with an instruction ("copy
        // the id from the EXISTING SCHEMAS block") pointing at nothing.
        let all = vec![
            tool("search_nodes"),
            tool("resolve_query"),
            tool("get_node"),
        ];
        let scoped = stage2_tools(&[], &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"resolve_query"));
        assert!(names.contains(&"search_nodes"));
        assert!(names.contains(&"get_node"));
    }

    #[test]
    fn a_graph_editing_candidate_below_its_mutating_bar_fails_open_without_resolve_query() {
        // The regression case #1840 calls out explicitly: a Graph Editing
        // candidate scoring in [READ_SKILL_SCORE_BAR, MUTATING_SKILL_SCORE_BAR)
        // — below the bar its own blast radius requires, but above the
        // read-only bar. It must not clear the gate, and the fail-open
        // surface it falls through to must still exclude resolve_query.
        let all = vec![
            tool("search_nodes"),
            tool("resolve_query"),
            tool("update_node"),
        ];
        let cands = vec![candidate(
            "Graph Editing",
            0.2,
            &["update_node", "search_nodes", "resolve_query"],
        )];
        assert!(
            (READ_SKILL_SCORE_BAR..MUTATING_SKILL_SCORE_BAR).contains(&0.2),
            "fixture score must sit in the gap this test targets"
        );
        assert!(!clears_score_gate(&cands[0]));

        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"resolve_query"));
        // And the prompt block that would justify offering it is absent too.
        assert!(render_candidates_for_prompt(&cands).is_none());
    }

    #[test]
    fn a_graph_editing_candidate_that_clears_its_bar_offers_resolve_query_with_its_guidance() {
        // The positive case: when Graph Editing genuinely clears the mutating
        // bar, resolve_query is offered — and the same eligibility filter
        // that scopes stage2_tools also renders the EXISTING SCHEMAS
        // block resolve_query's description depends on, so the tool never
        // reaches the model without its guidance.
        let all = vec![
            tool("search_nodes"),
            tool("resolve_query"),
            tool("update_node"),
        ];
        let mut c = candidate(
            "Graph Editing",
            0.9,
            &["update_node", "search_nodes", "resolve_query"],
        );
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);

        let scoped = stage2_tools(&[c.clone()], &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"resolve_query"));

        let rendered =
            render_candidates_for_prompt(&[c]).expect("cleared candidate renders a block");
        assert!(rendered.contains("EXISTING SCHEMAS"));
    }

    #[test]
    fn whitelist_naming_only_unregistered_tools_still_excludes_resolve_query_on_fail_open() {
        // The other fail-open branch: a whitelist that resolves to nothing
        // registered. Falls through to the same guidance-free surface, so the
        // exclusion must hold here too, not just on empty retrieval.
        let all = vec![tool("search_nodes"), tool("resolve_query")];
        let cands = vec![candidate("ghost", 0.9, &["tool_that_does_not_exist"])];
        let scoped = stage2_tools(&cands, &all);
        let names: Vec<&str> = scoped.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"resolve_query"));
        assert!(names.contains(&"search_nodes"));
    }

    /// A workspace context carrying a rendered entity-types block, as
    /// `context_ops::build_workspace_context` produces it.
    fn workspace_with_block() -> String {
        format!("Collections: none\n{EXISTING_SCHEMAS_HEADER}\n- invoice (amount, status)\n")
    }

    #[test]
    fn tools_with_available_guidance_requires_a_rendered_entity_types_block() {
        // A candidate clearing the gate but with empty schema_metadata (no
        // typed entities) whitelists resolve_query yet renders no entity-
        // types sub-block for it — render_candidates_for_prompt's header text
        // alone would make `candidate_block` `Some`, but that's not the same
        // as resolve_query having guidance available. With no workspace block
        // either, neither site supplies it.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        assert!(tools_with_available_guidance(&cands, false, "").is_empty());
    }

    #[test]
    fn tools_with_available_guidance_includes_a_tool_whose_candidate_renders_real_entity_types() {
        let mut c = candidate("Graph Editing", 0.9, &["resolve_query"]);
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let cands = vec![c];
        let available = tools_with_available_guidance(&cands, false, "");
        assert!(available.contains("resolve_query"));
    }

    #[test]
    fn tools_with_available_guidance_is_empty_when_rendering_is_disabled() {
        // render_disabled mirrors session.routing_disabled: even a candidate
        // with real schema_metadata contributes nothing once injection is
        // suppressed for this turn — absent a workspace block, which is a
        // separate site that flag says nothing about.
        let mut c = candidate("Graph Editing", 0.9, &["resolve_query"]);
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let cands = vec![c];
        assert!(tools_with_available_guidance(&cands, true, "").is_empty());
    }

    #[test]
    fn tools_with_available_guidance_excludes_a_candidate_below_its_score_bar() {
        // Real schema_metadata doesn't matter if the candidate never clears
        // the gate in the first place. resolve_query alone is a read tool
        // (READ_SKILL_SCORE_BAR = 0.15), so the fixture score must sit below
        // that, not just below the mutating bar.
        let mut c = candidate("Graph Editing", 0.01, &["resolve_query"]);
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let cands = vec![c];
        assert!(!clears_score_gate(&cands[0]));
        assert!(tools_with_available_guidance(&cands, false, "").is_empty());
    }

    #[test]
    fn a_workspace_entity_types_block_supplies_guidance_a_candidate_lacks() {
        // The reachability bug this gate had: Graph Editing whitelists
        // resolve_query but declares no `node_types`, so `skill_ops` falls
        // back to "all non-core schemas" — empty in a workspace whose custom
        // schemas didn't match, leaving `schema_metadata` empty even though
        // the resident workspace context did carry the block. The tool's
        // required `node_type` is answerable from that block, so withholding
        // it here stranded the model with no way to resolve an indirect
        // target.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        assert!(
            tools_with_available_guidance(&cands, false, "").is_empty(),
            "no block at either site: still withheld"
        );
        let available = tools_with_available_guidance(&cands, false, &workspace_with_block());
        assert!(
            available.contains("resolve_query"),
            "workspace context carries the block, so the required parameter is answerable"
        );
    }

    #[test]
    fn a_workspace_block_does_not_rescue_a_candidate_below_its_score_bar() {
        // The workspace block makes the *parameter* answerable; it does not
        // make an unmatched skill's tools eligible. Routing's trust boundary
        // (ADR-038) still decides which skills may act.
        let cands = vec![candidate("Graph Editing", 0.01, &["resolve_query"])];
        assert!(!clears_score_gate(&cands[0]));
        assert!(
            tools_with_available_guidance(&cands, false, &workspace_with_block()).is_empty(),
            "score gate still governs eligibility"
        );
    }

    #[test]
    fn a_workspace_block_survives_candidate_injection_being_disabled() {
        // `routing_disabled` exists because injecting the *candidate* block
        // suppresses tool-calling on some served models. The resident
        // workspace block is present regardless of that flag, so the tool's
        // required parameter is still answerable and the tool stays offered.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        let available = tools_with_available_guidance(&cands, true, &workspace_with_block());
        assert!(available.contains("resolve_query"));
    }

    #[test]
    fn a_workspace_block_without_the_shared_heading_does_not_count() {
        // Matching is on the shared heading constant, not on any schema-ish
        // prose: a workspace context listing collections and playbooks but no
        // entity types leaves the required parameter unanswerable.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        let no_block = "Collections: Invoices, Venues\nActive playbooks: none\n";
        assert!(tools_with_available_guidance(&cands, false, no_block).is_empty());
    }

    #[test]
    fn a_heading_with_no_type_listed_under_it_does_not_count() {
        // `context_ops`'s renderer pushes the heading, then breaks out of its
        // per-schema loop once the character budget is spent — a budget
        // exhausted by the very first type line leaves the heading behind with
        // nothing under it. Matching the heading alone would offer the tool
        // while its required `node_type` pointed at an empty list, which is
        // the strand this gate exists to prevent.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        let truncated = format!("COLLECTIONS: Invoices\n\n{EXISTING_SCHEMAS_HEADER}\n");
        assert!(tools_with_available_guidance(&cands, false, &truncated).is_empty());
    }

    #[test]
    fn a_heading_followed_by_prose_rather_than_a_type_line_does_not_count() {
        // Guards the shape of the check itself: both renderers emit `- <id>`
        // lines, so anything else under the heading is not a type listing.
        let cands = vec![candidate("Graph Editing", 0.9, &["resolve_query"])];
        let prose = format!("{EXISTING_SCHEMAS_HEADER}\n(none recorded yet)\n");
        assert!(tools_with_available_guidance(&cands, false, &prose).is_empty());
    }

    #[test]
    fn a_malformed_metadata_entry_does_not_discard_the_others() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([
            {"fields": [{"name": "orphan", "type": "text"}]},
            {"type_id": "task", "fields": [{"name": "title", "type": "text"}]}
        ]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(
            rendered.contains("- task -> title: text"),
            "an entry missing type_id must not take the valid ones with it: {rendered}"
        );
    }

    #[test]
    fn schema_metadata_marks_required_and_renders_title_template() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{
            "type_id": "invoice",
            "title_template": "{reference} - {amount}",
            "fields": [
                {"name": "reference", "type": "string"},
                {"name": "amount", "type": "number", "required": true}
            ]
        }]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        // Same notation the workspace-context block uses; both land in one prompt.
        assert!(
            rendered.contains("- invoice -> reference: string; amount: number, required"),
            "got: {rendered}"
        );
        // create_node's description promises the template is shown here.
        assert!(
            rendered.contains("[title_template: {reference} - {amount}]"),
            "got: {rendered}"
        );
    }

    #[test]
    fn a_zero_field_type_renders_without_a_dangling_separator() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([
            {"type_id": "marker", "fields": []},
            {"type_id": "invoice", "fields": [{"name": "amount", "type": "number"}]}
        ]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        // No trailing ": " promising a field list that never arrives.
        assert!(rendered.contains("- marker\n"), "got: {rendered}");
        assert!(!rendered.contains("- marker:"), "got: {rendered}");
        // The populated form is unaffected.
        assert!(
            rendered.contains("- invoice -> amount: number"),
            "got: {rendered}"
        );
    }

    #[test]
    fn metadata_with_no_usable_entries_renders_no_section() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{"fields": []}]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(!rendered.contains("EXISTING SCHEMAS"));
    }

    /// This site's heading must be the exact shared constant, not an
    /// independently-worded copy — see EXISTING_SCHEMAS_HEADER's doc
    /// comment (#1846: two independently-maintained copies of this heading,
    /// one carrying an anti-copy clause and the other not, let contamination
    /// persist after the first was fixed).
    #[test]
    fn schema_metadata_section_uses_the_shared_header() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{"type_id": "invoice", "fields": []}]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(
            rendered.contains(EXISTING_SCHEMAS_HEADER),
            "got: {rendered}"
        );
    }

    #[test]
    fn schema_metadata_renders_enum_values() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{
            "type_id": "task",
            "fields": [
                {"name": "title", "type": "text"},
                {"name": "status", "type": "enum", "enum_values": ["todo", "done"]}
            ]
        }]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(rendered.contains("- task -> title: text; status: enum {todo, done}"));
    }

    /// Every eligible candidate carries its own copy of the entity block, so
    /// one Stage-2 prompt repeats it `RETRIEVAL_TOP_K` times — plus once more
    /// from the workspace-context path, which `agent_loop` concatenates into
    /// the same prompt under the same heading.
    ///
    /// Measured 2026-07-31 for the duplication question in #1848: **4 copies
    /// per prompt** (3 routing + 1 workspace context), costing ~141 redundant
    /// tokens on a 2-schema workspace and scaling with schema count. The copies
    /// are identical rather than differently scoped, because no seeded skill
    /// declares `node_types` — see `skill_pipeline`'s
    /// `no_seeded_skill_scopes_its_schema_metadata`, which fails if that
    /// premise stops holding.
    ///
    /// The assertion here is the invariant behind that measurement — one copy
    /// per eligible candidate — not the token figure, which is a dated finding
    /// rather than a target to hold constant.
    #[test]
    fn every_eligible_candidate_carries_its_own_entity_block() {
        let meta = json!([
            {
                "type_id": "invoice",
                "name": "Invoice",
                "fields": [
                    {"name": "reference", "type": "string"},
                    {"name": "amount", "type": "number", "required": true},
                    {"name": "status", "type": "enum", "enum_values": ["draft", "sent", "paid"]}
                ],
                "title_template": "{reference}"
            },
            {
                "type_id": "customer",
                "name": "Customer",
                "fields": [
                    {"name": "name", "type": "string", "required": true},
                    {"name": "email", "type": "string"}
                ]
            }
        ]);

        // All RETRIEVAL_TOP_K candidates eligible, each carrying the same
        // scoped schemas — the shape that produces the most copies.
        let candidates: Vec<SkillCandidate> = ["Node Creation", "Graph Editing", "Organization"]
            .iter()
            .map(|n| {
                let mut c = candidate(n, 0.9, &["create_node"]);
                c.schema_metadata = meta.clone();
                c
            })
            .collect();
        assert_eq!(candidates.len(), RETRIEVAL_TOP_K);

        let block = render_candidates_for_prompt(&candidates).expect("all are eligible");

        assert_eq!(
            block.matches("EXISTING SCHEMAS").count(),
            RETRIEVAL_TOP_K,
            "each eligible candidate carries its own copy of the entity block"
        );
    }

    /// Production `schema_metadata` carries a display `name` alongside the id,
    /// and the block must then read exactly as the workspace-context one does —
    /// `- id "Name" -> fields`. Both are concatenated under a single heading,
    /// so a type described two ways there reads to the model as two types.
    ///
    /// The other fixtures in this module omit `name`, which is why this case
    /// needs its own test: the format the agent actually ships would otherwise
    /// go unasserted.
    #[test]
    fn a_named_type_renders_in_the_workspace_context_format() {
        let mut c = candidate("Node Creation", 0.9, &["create_node"]);
        c.schema_metadata = json!([{
            "type_id": "invoice",
            "name": "Invoice",
            "fields": [
                {"name": "amount", "type": "number", "required": true}
            ],
            "title_template": "{reference}"
        }]);
        let rendered = render_candidates_for_prompt(&[c]).unwrap();
        assert!(
            rendered.contains(
                "- invoice \"Invoice\" -> amount: number, required [title_template: {reference}]"
            ),
            "named type must render as `- id \"Name\" -> fields`, got: {rendered}"
        );
    }
}
