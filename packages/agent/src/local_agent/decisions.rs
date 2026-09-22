//! Named, recorded decision points for the three selections an agent turn
//! makes: which skill retrieval matched, which schema applies, and which
//! operation to call. The latter two are the model's, made implicitly inside
//! generation; the first is deterministic retrieval, recorded because it is
//! upstream of both and constrains them.
//!
//! Both decisions exist today only as emergent properties of whatever tool call
//! the model happens to emit. Nothing names them, so nothing can score them:
//! the only available signal is end-to-end scenario pass/fail, which ADR-056
//! states "describe[s] the harness as much as the model" while guidance and
//! eval prompts share wording. Both of ADR-056's own E4B matrix failures are
//! decision failures rather than generation failures — a wrong operation
//! (`execute_query` where `search_nodes` was wanted) and a turn that stopped
//! after the read without the write — and neither is separable from the
//! scenario result that contains it.
//!
//! This module changes nothing about *who* decides. The model still makes both
//! calls exactly as before. What it adds is a record of what the model was
//! choosing between and what it chose, so the decision can be scored on its own
//! rather than inferred from a downstream failure.
//!
//! ## Why this is not a third gate
//!
//! ADR-038 establishes two independently-sourced gates — Stage 1's structural choice and
//! Stage 2's retrieval score plus the model's judgment — and closes with an
//! explicit warning: *"Avoid inventing a third number."* Nothing here gates,
//! filters, scores, or rejects. The candidate sets recorded are the ones the
//! pipeline already computed (`stage2_tools` for operations, the retrieved
//! schema metadata for schemas), and the outcomes recorded are the ones the
//! model already produced. Removing this module would change no behaviour.
//!
//! ## Why the candidate set is recorded, not just the outcome
//!
//! "The model called `search_nodes`" is not scoreable on its own: whether that
//! was right depends on what else was on offer. A turn where `search_nodes` was
//! the only query tool and a turn where it competed with three others are
//! different decisions with the same outcome. The eval needs both halves, so
//! both are recorded.

use super::tools::Tool;
use crate::agent_types::SkillCandidate;

/// A single recorded decision: what was on offer, and what was chosen.
///
/// Deliberately not generic over the candidate type. The two decisions carry
/// different candidate shapes (tool wire names; schema type ids) but the same
/// recorded shape, and collapsing them into one struct keeps the log format
/// identical across both — which is what lets one eval parser read both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRecord {
    /// Which decision this is. Stable across log lines; the eval joins on it.
    pub kind: DecisionKind,
    /// Everything the model could have picked, in the order it was presented.
    ///
    /// Order matters: presentation order is a known influence on small-model
    /// selection, so an eval that finds a position bias needs to see it.
    pub candidates: Vec<String>,
    /// What the model actually picked, or `None` when it picked nothing.
    ///
    /// `None` is a real and distinct outcome, not a missing value: a turn that
    /// declined to act is different from one that acted wrongly, and ADR-056's
    /// Scenario 6 failure (read fired, write never did) is exactly the former.
    /// Folding it into "no record" would hide the failure class that motivated
    /// this module.
    pub selected: Option<String>,
}

/// Which of the three selections a [`DecisionRecord`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    /// Which skill retrieval matched, which scopes everything downstream.
    ///
    /// Recorded because it is the layer the observed failures actually occur
    /// at. A type-definition request that retrieves Node Creation instead of
    /// Schema Creation never sees `create_schema` — `stage2_tools` scopes it
    /// out — so the turn reads as an operation failure when it is a retrieval
    /// failure. Without this record the two are indistinguishable, and the fix
    /// for one does nothing for the other.
    Skill,
    /// Which schema/entity type the turn is acting on.
    Schema,
    /// Which tool the turn calls.
    Operation,
}

impl DecisionKind {
    /// Stable wire name for the structured log and the eval's join key.
    ///
    /// A method rather than `Debug`, because `Debug` output is not a contract
    /// and a rename would silently break every recorded baseline.
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::Skill => "skill",
            DecisionKind::Schema => "schema",
            DecisionKind::Operation => "operation",
        }
    }
}

impl DecisionRecord {
    /// Whether the model selected something outside the candidate set.
    ///
    /// This should be impossible for operations — the tool surface is scoped
    /// before the model sees it, and llama.cpp's grammar constrains the call
    /// envelope to a registered tool name. It is *not* impossible for schemas:
    /// nothing constrains a `node_type` argument to a retrieved candidate, so
    /// the model can and does name a type that was never on offer. That case is
    /// the single most diagnostic signal this record carries, because it cannot
    /// be explained as a hard choice between plausible options.
    pub fn selected_off_menu(&self) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|s| !self.candidates.iter().any(|c| c == s))
    }

    /// Render the candidate set for one structured-log field.
    ///
    /// Comma-separated to match `routed_skill_names`, whose shape the log
    /// scrapers here already parse. That function's own doc comment records why
    /// the exact shape is a contract rather than a detail: the first scraper
    /// written against it silently matched nothing because its shape was assumed
    /// instead of asserted.
    pub fn candidates_field(&self) -> String {
        self.candidates.join(", ")
    }
}

/// Record the operation decision for one inference round.
///
/// `offered` is the scoped tool surface the model was shown — the output of
/// `routing::stage2_tools`, after every narrowing the pipeline applies. `called`
/// is the tool names the model emitted this round, in emission order.
///
/// Only the first call is recorded as `selected`. A round emitting several calls
/// is one decision about where to start, not several independent decisions: the
/// later calls are conditioned on the first having been chosen, so scoring them
/// as peers would count one judgment two or three times. The full set stays
/// visible in the existing `tool_calls_parsed` span attribute for anyone
/// reconstructing a multi-call round.
pub fn record_operation(offered: &[String], called: &[String]) -> DecisionRecord {
    DecisionRecord {
        kind: DecisionKind::Operation,
        candidates: offered.to_vec(),
        selected: called.first().cloned(),
    }
}

/// Record the schema decision for a turn.
///
/// `candidates` is the retrieved schema type ids the turn had available;
/// `selected` is the type the model actually acted on, where the call named one.
///
/// A turn that names no type at all yields `selected: None` rather than no
/// record. That distinguishes "had schemas to choose from and used none" — a
/// real outcome worth scoring, and the shape of a turn that answered from
/// conversation instead of the graph — from "was never offered any."
pub fn record_schema(candidates: &[String], selected: Option<String>) -> DecisionRecord {
    DecisionRecord {
        kind: DecisionKind::Schema,
        candidates: candidates.to_vec(),
        selected,
    }
}

/// Record the skill decision for a turn.
///
/// Candidates are every skill retrieval returned, in score order — including
/// ones that did not clear their own bar, because "the right skill was
/// retrieved but scored below its blast-radius bar" and "the right skill was
/// never retrieved" are different failures with different fixes, and a
/// candidate list filtered to gate-clearers cannot tell them apart.
///
/// The selection is [`routing::leading_tool_bearing_candidate`]: the one whose
/// whitelist actually leads `stage2_tools`. `None` when nothing qualifies,
/// which is the fail-open case — the turn runs on the full tool surface, and no
/// skill was chosen.
///
/// **Clearing the score gate is not sufficient**, and an earlier version of
/// this function took the first gate-clearer, which was wrong. A schema-typed
/// retrieval hit carries `tools: []` and is pinned by the lexical backstop at a
/// confidence above any cosine-derived score a real skill can reach, so on
/// every turn that names a schema outright — which is most of the schema
/// scenarios in the decision eval, by construction — it sorts first and would
/// have been recorded as the skill that led the turn. It leads nothing: it
/// contributes no tool to the offered surface. `routing` owns that predicate
/// because two consumers already re-derived it and were both wrong the same
/// way.
///
/// The tool-less candidate still appears in `candidates`: it was genuinely
/// retrieved, and "retrieved but contributed nothing" is worth seeing.
///
/// Unlike the other two decisions this is not made by the model at all; it is
/// deterministic retrieval plus a score bar. It is recorded anyway because it
/// is upstream of both of the model's own decisions and constrains them: an
/// operation the whitelist excluded was never a choice the model could make.
pub fn record_skill(candidates: &[SkillCandidate]) -> DecisionRecord {
    DecisionRecord {
        kind: DecisionKind::Skill,
        candidates: candidates.iter().map(|c| c.name.clone()).collect(),
        selected: super::routing::leading_tool_bearing_candidate(candidates)
            .map(|c| c.name.clone()),
    }
}

/// The schema type ids a turn's retrieval made available.
///
/// Reads the same `schema_metadata` the Stage-2 prompt block renders from, so
/// the recorded candidate set is what the model was actually shown rather than a
/// second derivation that could drift from it. Decoding goes through
/// `entity_types_block`'s shared descriptor for the same reason: that module
/// exists because two independent renderers of this data drifted, and the model
/// received an instruction whose referent only one of them emitted.
///
/// Deduplicated with order preserved — several candidates can carry the same
/// type, and a duplicated option is not a wider choice.
pub fn schema_candidates(candidates: &[SkillCandidate]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for c in candidates {
        for d in nodespace_core::ops::entity_types_block::descriptors_from_json(&c.schema_metadata)
        {
            if !seen.contains(&d.type_id) {
                seen.push(d.type_id);
            }
        }
    }
    seen
}

/// The schema type a tool call acts on, where the call names one.
///
/// Reads `node_type` (the argument every schema-scoped tool uses for this) and
/// falls back to `type` for `create_schema`, whose argument is the type being
/// defined rather than one being referenced.
///
/// Returns `None` for a tool that is not schema-scoped at all — `get_node` by
/// id, say — because such a turn made no schema decision. Recording one would
/// put a null in the denominator of every accuracy figure computed from this.
pub fn selected_schema(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    if !schema_scoped(tool_name) {
        return None;
    }
    args.get("node_type")
        .or_else(|| args.get("type"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
}

/// Whether a tool's arguments carry a schema/type reference worth scoring.
///
/// Derived from the [`Tool`] enum rather than a hand-written name list, so a
/// tool added to the enum is a compile error here rather than a silent omission
/// from the recorded decision — the same reason `Tool::ALL` is compiler-checked
/// for completeness.
fn schema_scoped(tool_name: &str) -> bool {
    match Tool::from_name(tool_name) {
        Some(
            Tool::CreateNode
            | Tool::UpdateNode
            | Tool::CreateSchema
            | Tool::UpdateSchema
            | Tool::SearchNodes
            | Tool::ResolveQuery,
        ) => true,
        // Explicitly enumerated rather than a `_ => false` catch-all: a new
        // variant should force a decision about whether it is schema-scoped,
        // which a wildcard would silently answer as "no".
        Some(
            Tool::SearchSemantic
            | Tool::GetNode
            | Tool::UpdateTaskStatus
            | Tool::CreateRelationship
            | Tool::GetRelatedNodes
            | Tool::SearchSkills
            | Tool::DeleteNode
            | Tool::CreateNodesFromMarkdown
            | Tool::RouteClarify
            | Tool::ListConflicts
            | Tool::GetConflict
            | Tool::DismissConflict
            | Tool::AdoptExistingConflict
            | Tool::MergeConflict
            | Tool::GetWorkflowState,
        ) => false,
        // An unrecognised name is not schema-scoped: it is either a malformed
        // generation or a tool this build does not register, and neither is a
        // schema decision the model can be scored on.
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn candidate(schema_metadata: serde_json::Value) -> SkillCandidate {
        SkillCandidate {
            id: "id".into(),
            name: "Skill".into(),
            description: "desc".into(),
            score: 0.5,
            tools: vec![],
            instructions: String::new(),
            schema_metadata,
        }
    }

    fn skill(name: &str, score: f32, tools: &[&str]) -> SkillCandidate {
        SkillCandidate {
            id: name.to_lowercase().replace(' ', "-"),
            name: name.into(),
            description: "desc".into(),
            score,
            tools: tools.iter().map(|t| t.to_string()).collect(),
            instructions: String::new(),
            schema_metadata: json!(null),
        }
    }

    #[test]
    fn skill_records_every_candidate_and_the_top_gate_clearer() {
        // Score order as retrieval returns it. `search_nodes` is read-only, so
        // the read bar (0.15) applies and 0.80 clears it comfortably.
        let cands = [
            skill("Node Creation", 0.80, &["search_nodes"]),
            skill("Schema Creation", 0.72, &["search_nodes"]),
        ];
        let rec = record_skill(&cands);
        assert_eq!(rec.kind, DecisionKind::Skill);
        assert_eq!(rec.candidates, vec!["Node Creation", "Schema Creation"]);
        assert_eq!(rec.selected.as_deref(), Some("Node Creation"));
    }

    /// "Retrieved but below its bar" and "never retrieved" are different
    /// failures with different fixes, so a below-bar candidate must still
    /// appear in the candidate list rather than being filtered out of it.
    #[test]
    fn skill_keeps_below_bar_candidates_visible_as_candidates() {
        // 0.05 clears no bar; `create_schema` earns the mutating bar (0.30).
        let cands = [skill("Schema Creation", 0.05, &["create_schema"])];
        let rec = record_skill(&cands);
        assert_eq!(rec.candidates, vec!["Schema Creation"]);
        assert_eq!(
            rec.selected, None,
            "nothing cleared its bar, so no skill led the turn"
        );
    }

    /// The fail-open case: retrieval returned nothing, the turn runs on the
    /// full tool surface, and no skill was chosen.
    #[test]
    fn skill_with_no_candidates_records_none() {
        let rec = record_skill(&[]);
        assert!(rec.candidates.is_empty());
        assert_eq!(rec.selected, None);
    }

    /// The regression this function was corrected for. A schema-typed retrieval
    /// hit carries `tools: []` and the lexical backstop pins it at confidence
    /// 1.0 — above any cosine-derived score a real skill can reach — so it
    /// sorts first on every turn that names a schema outright. It leads
    /// nothing: it contributes no tool to the offered surface. Taking the first
    /// gate-clearing candidate recorded it as the winner and mislabelled
    /// precisely the scenarios the decision eval was built to score.
    #[test]
    fn a_tool_less_schema_hit_does_not_lead_the_turn() {
        let schema_hit = skill("Company Sold To", 1.0, &[]);
        let real_skill = skill("Node Creation", 0.62, &["create_node"]);
        let rec = record_skill(&[schema_hit, real_skill]);

        assert_eq!(
            rec.selected.as_deref(),
            Some("Node Creation"),
            "a candidate whitelisting no tool contributes nothing to the offered surface, \
             so it cannot be the skill that led the turn"
        );
        assert_eq!(
            rec.candidates,
            vec!["Company Sold To", "Node Creation"],
            "it was genuinely retrieved, so it stays visible as a candidate — \
             'retrieved but contributed nothing' is worth seeing"
        );
    }

    /// Selection does not depend on the caller's ordering. `route` sorts by
    /// score descending today, but the rule about which candidate leads is an
    /// explicit max rather than a positional assumption.
    #[test]
    fn leading_candidate_is_the_highest_scorer_not_the_first() {
        let weak = skill("Research", 0.20, &["search_nodes"]);
        let strong = skill("Node Creation", 0.80, &["create_node"]);
        let rec = record_skill(&[weak, strong]);
        assert_eq!(rec.selected.as_deref(), Some("Node Creation"));
    }

    #[test]
    fn operation_records_the_offered_surface_and_the_first_call() {
        let offered = vec!["search_nodes".to_string(), "create_node".to_string()];
        let rec = record_operation(&offered, &["create_node".to_string()]);
        assert_eq!(rec.kind, DecisionKind::Operation);
        assert_eq!(rec.candidates, offered);
        assert_eq!(rec.selected.as_deref(), Some("create_node"));
    }

    /// A multi-call round is one decision about where to start. Scoring the
    /// later calls as peers would count a single judgment more than once.
    #[test]
    fn operation_records_only_the_first_of_several_calls() {
        let offered = vec!["search_nodes".to_string(), "update_node".to_string()];
        let rec = record_operation(
            &offered,
            &["search_nodes".to_string(), "update_node".to_string()],
        );
        assert_eq!(rec.selected.as_deref(), Some("search_nodes"));
    }

    /// The Scenario-6 shape: the model was offered tools and called none. This
    /// must survive as a recorded outcome rather than vanishing, since it is one
    /// of the two failures that motivated the module.
    #[test]
    fn operation_with_no_call_records_none_not_absence() {
        let offered = vec!["search_nodes".to_string()];
        let rec = record_operation(&offered, &[]);
        assert_eq!(rec.selected, None);
        assert_eq!(rec.candidates.len(), 1);
    }

    #[test]
    fn schema_candidates_are_deduplicated_in_presentation_order() {
        let cands = vec![
            candidate(json!([{"type_id": "invoice"}, {"type_id": "customer"}])),
            candidate(json!([{"type_id": "invoice"}, {"type_id": "project"}])),
        ];
        assert_eq!(
            schema_candidates(&cands),
            vec![
                "invoice".to_string(),
                "customer".to_string(),
                "project".to_string()
            ]
        );
    }

    #[test]
    fn schema_candidates_are_empty_when_retrieval_carried_no_metadata() {
        assert!(schema_candidates(&[candidate(json!(null))]).is_empty());
    }

    #[test]
    fn selected_schema_reads_node_type_for_a_schema_scoped_tool() {
        let args = json!({"node_type": "invoice", "content": "x"});
        assert_eq!(
            selected_schema("create_node", &args),
            Some("invoice".to_string())
        );
    }

    /// `create_schema` names the type it defines in `type`, not `node_type`.
    #[test]
    fn selected_schema_falls_back_to_type_for_create_schema() {
        let args = json!({"type": "venue", "fields": []});
        assert_eq!(
            selected_schema("create_schema", &args),
            Some("venue".to_string())
        );
    }

    /// A turn that fetched a node by id made no schema decision. Recording one
    /// would put a null in the denominator of every accuracy figure.
    #[test]
    fn selected_schema_is_none_for_a_tool_that_is_not_schema_scoped() {
        let args = json!({"node_type": "invoice"});
        assert_eq!(selected_schema("get_node", &args), None);
    }

    #[test]
    fn selected_schema_is_none_for_an_unrecognised_tool_name() {
        let args = json!({"node_type": "invoice"});
        assert_eq!(selected_schema("not_a_real_tool", &args), None);
    }

    #[test]
    fn selected_schema_ignores_a_blank_type() {
        let args = json!({"node_type": "   "});
        assert_eq!(selected_schema("create_node", &args), None);
    }

    /// The most diagnostic signal the record carries: a type the model named
    /// that retrieval never offered. Unlike a hard choice between plausible
    /// candidates, this cannot be explained as a close call.
    #[test]
    fn off_menu_selection_is_detected() {
        let rec = record_schema(&["invoice".to_string()], Some("album".to_string()));
        assert!(rec.selected_off_menu());

        let on_menu = record_schema(&["invoice".to_string()], Some("invoice".to_string()));
        assert!(!on_menu.selected_off_menu());
    }

    /// Selecting nothing is not selecting something off-menu — the two are
    /// different failures and an eval must not conflate them.
    #[test]
    fn no_selection_is_not_off_menu() {
        let rec = record_schema(&["invoice".to_string()], None);
        assert!(!rec.selected_off_menu());
    }

    /// The log shape is a contract: `routed_skill_names` records that the first
    /// scraper written against its format silently matched nothing because the
    /// shape was assumed rather than asserted.
    #[test]
    fn candidates_field_is_comma_separated_like_routed_skill_names() {
        let rec = record_operation(
            &["search_nodes".to_string(), "get_node".to_string()],
            &["get_node".to_string()],
        );
        assert_eq!(rec.candidates_field(), "search_nodes, get_node");
    }

    #[test]
    fn candidates_field_is_empty_for_an_empty_surface() {
        assert_eq!(record_operation(&[], &[]).candidates_field(), "");
    }

    #[test]
    fn kind_wire_names_are_stable() {
        assert_eq!(DecisionKind::Skill.as_str(), "skill");
        assert_eq!(DecisionKind::Schema.as_str(), "schema");
        assert_eq!(DecisionKind::Operation.as_str(), "operation");
    }
}
