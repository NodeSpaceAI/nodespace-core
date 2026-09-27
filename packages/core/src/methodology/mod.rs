//! Methodology playbooks — installable, first-party work-tracking setups.
//!
//! A playbook composes NodeSpace's existing authoring primitives (schema
//! creation, enum-vocabulary extension, Play installation, skill seeding,
//! saved-view seeding) into a setup a user recognizes on day one —
//! "Linear-style", and later others. It adds no platform capability: every step is a call a user or an
//! agent could already make by hand.
//!
//! # First-party content, installed on demand
//!
//! Playbooks sit in the same tier as [`crate::models::core_schemas::get_core_schemas`]
//! and `seed_skill_nodes` — typed Rust shipped with the product. They differ
//! in *when* they install. Core schemas and seeded skills are written to every
//! database at daemon startup; a playbook is written only when a user picks one,
//! because a workspace has at most one methodology and the choice is theirs.
//! Nothing here is reachable from `seed_agent_nodes`.
//!
//! # Why core rather than the frontend
//!
//! Schema steps run through [`crate::schema::handle_create_schema`] and
//! [`crate::schema::handle_update_schema`] in-process, so `extends` cycle
//! detection, `mapsTo` resolution against the inherited field, reverse-name
//! validation and relationship-declaration persistence all apply unchanged. A
//! playbook cannot install a schema a hand-authored call could not.

pub mod install;
pub mod linear;
pub mod skills;
pub mod spec_driven;

pub use install::install_playbook;

use crate::markdown::NodeTemplate;
use crate::models::{QueryGeneratedBy, QueryNodeUpdate};
use crate::services::QueryDefinition;
use serde::{Deserialize, Serialize};

/// A methodology playbook: the content to install, plus the identity the GUI
/// picker and the generated reference doc both present it under.
pub struct MethodologyPlaybook {
    /// Stable machine id (`linear`) — the GUI picker's key and the generated
    /// reference doc's filename stem.
    pub id: &'static str,
    /// Display name ("Linear-style").
    pub name: &'static str,
    /// One-paragraph summary of what installing this sets up.
    pub description: &'static str,
    /// Schemas to create, in order. A schema must precede anything that
    /// targets it: `validate_play_rules` rejects a rule whose trigger names
    /// a type with no schema, so a mis-ordered playbook fails at install time
    /// rather than silently half-installing.
    pub schemas: Vec<SchemaStep>,
    /// Vocabulary extensions and added fields, applied after the schemas exist.
    pub field_value_extensions: Vec<FieldValueExtension>,
    /// Plays to install, as seeded (resettable) play nodes.
    pub plays: Vec<PlayStep>,
    /// Skill nodes seeded as usage guidance, authored as markdown files under
    /// `skills/<id>/` and loaded with [`skills::playbook_skill`] — see
    /// [`skills`] for the file format.
    pub skills: Vec<NodeTemplate>,
    /// The bundle-level skill: what this Playbook is, the constraints that
    /// come with it, and — appended at install time by
    /// [`skills::playbook_overview_skill`] — the ids everything actually
    /// landed under. The graph-resident answer to "what workflow is this
    /// workspace using?", so an agent in an installed workspace never has to
    /// reach for the install doc to find out.
    ///
    /// Markdown source in the same format as [`Self::skills`], titled
    /// `<name> Workspace` — the title SKILL.md tells an agent to look for.
    /// Kept apart from `skills` because its body depends on the install:
    /// it is seeded last, once every id is known.
    pub overview: &'static str,
    /// Saved views — pre-configured boards and lists — seeded as `query`
    /// nodes, so the install lands with something to look at rather than a
    /// type the user has to build a view over by hand. Installed last:
    /// every view targets a schema, which must exist first.
    pub views: Vec<ViewStep>,
}

/// One `create_schema` call.
pub struct SchemaStep {
    /// The schema id this call produces, derived from `params.name`. Named
    /// explicitly so collision detection need not re-derive it.
    pub schema_id: &'static str,
    /// A [`crate::schema::CreateSchemaParams`] payload.
    pub params: serde_json::Value,
}

/// One `update_schema` call extending an existing schema: `add_field_values`
/// against a field's vocabulary, or `add_fields` adding a field outright.
pub struct FieldValueExtension {
    /// Schema whose field is extended. For an inherited field this is the
    /// *extending* schema (`issue`), not the declaring one (`task`) — the
    /// values land on `issue`'s materialized copy, leaving `task`'s own
    /// vocabulary untouched.
    pub schema_id: &'static str,
    /// Field being extended, for progress reporting.
    pub field: &'static str,
    /// An [`crate::schema::UpdateSchemaParams`] payload carrying
    /// `add_field_values` or `add_fields`.
    pub params: serde_json::Value,
}

impl FieldValueExtension {
    /// Whether this step adds a field rather than extending a vocabulary.
    pub fn adds_field(&self) -> bool {
        self.params.get("add_fields").is_some()
    }
}

/// One Play, installed as a seeded `play` node.
pub struct PlayStep {
    /// Stable id for the play node.
    pub play_id: &'static str,
    /// Display name, stored as the node's content.
    pub name: &'static str,
    /// Human-readable purpose, stored as `description`.
    pub description: &'static str,
    /// The rules array, as `RuleDefinition` JSON.
    pub rules: serde_json::Value,
}

impl PlayStep {
    /// The play node's properties.
    ///
    /// `rules` is written twice — once live, once under `_seed.default_rules`
    /// — which is what makes the Play resettable via
    /// [`crate::playbook::seeded::reset_seeded_play_to_default`] and marks it
    /// seeded for the edit/disable warning path (ADR-060 §8).
    pub fn properties(&self) -> serde_json::Value {
        serde_json::json!({
            "description": self.description,
            "rules": self.rules,
            "_seed": { "default_rules": self.rules },
        })
    }
}

/// One saved view, installed as a `query` node.
///
/// A saved query is an ordinary node whose fields carry both *what* it shows
/// (the definition) and *how* it renders (`view_config`), so the board
/// travels with the node — a seeded Kanban opens as a Kanban, grouped the
/// way the playbook authored it, with no per-user setup.
pub struct ViewStep {
    /// Stable id for the query node.
    pub view_id: &'static str,
    /// Display name, stored as the node's content.
    pub name: &'static str,
    /// What the view selects.
    pub definition: QueryDefinition,
    /// The view configuration: `lastView` (`list` | `table` | `kanban`), plus
    /// `kanban.groupBy` naming the field whose values become columns.
    pub view_config: serde_json::Value,
}

impl ViewStep {
    /// The query node's properties: the query schema's snake_case storage
    /// keys, the same shape the query viewer creates when a user saves a view
    /// by hand.
    pub fn properties(&self) -> serde_json::Value {
        let definition = &self.definition;
        QueryNodeUpdate {
            target_type: Some(definition.target_type.clone()),
            filters: Some(definition.filters.clone()),
            sorting: definition.sorting.clone().map(Some),
            limit: definition.limit.map(Some),
            generated_by: Some(QueryGeneratedBy::User),
            generator_context: None,
            view_config: Some(Some(self.view_config.clone())),
        }
        .to_properties_patch()
    }
}

/// What one step did, reported so the caller can disclose it.
///
/// Collision handling is disclosed rather than silent: a step whose target id
/// was taken reports [`StepOutcome::Suffixed`] naming both ids, so the user
/// sees that their existing `cycle` was left alone and a `cycle__2` created
/// instead — never a dialog, never a surprise.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum StepOutcome {
    /// Created under the id the playbook asked for.
    Created { id: String },
    /// The requested id was taken; created under a suffixed id instead.
    Suffixed { requested: String, created: String },
    /// Not attempted, because an earlier step failed.
    Skipped,
    /// Failed. The install stops here.
    Failed { message: String },
}

/// One row of an [`InstallReport`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepReport {
    pub label: String,
    pub outcome: StepOutcome,
}

/// The result of installing a playbook: one outcome per step, in execution order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub playbook_id: String,
    pub steps: Vec<StepReport>,
    /// Whether every step completed (`Created` or `Suffixed`).
    pub success: bool,
}

impl InstallReport {
    /// Ids re-keyed because the playbook's preferred id was taken — the
    /// disclosure the GUI surfaces without blocking on a confirmation.
    pub fn suffixed(&self) -> Vec<(&str, &str)> {
        self.steps
            .iter()
            .filter_map(|s| match &s.outcome {
                StepOutcome::Suffixed { requested, created } => {
                    Some((requested.as_str(), created.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    /// The first failure's message, if the install stopped early.
    pub fn failure(&self) -> Option<&str> {
        self.steps.iter().find_map(|s| match &s.outcome {
            StepOutcome::Failed { message } => Some(message.as_str()),
            _ => None,
        })
    }
}

/// Every playbook this build ships.
///
/// The GUI picker iterates this, so a playbook added here is offered with no
/// further wiring. Its CLI reference doc is not automatic: it needs a region in
/// `packages/cli/examples/gen_skill_md.rs` and a `references/` file to hold it.
pub fn all_playbooks() -> Vec<MethodologyPlaybook> {
    vec![linear::playbook(), spec_driven::playbook()]
}

/// Look up a playbook by its [`MethodologyPlaybook::id`].
pub fn playbook_by_id(id: &str) -> Option<MethodologyPlaybook> {
    all_playbooks().into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_playbook_has_a_unique_id() {
        let playbooks = all_playbooks();
        let mut ids: Vec<&str> = playbooks.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "playbook ids must be unique");
    }

    #[test]
    fn playbook_by_id_finds_a_shipped_playbook() {
        assert!(playbook_by_id("linear").is_some());
        assert!(playbook_by_id("spec-driven").is_some());
        assert!(playbook_by_id("nonexistent").is_none());
    }

    /// A seeded play carries its rules twice: live, and as the shipped
    /// default. Without the `_seed` marker `is_seeded_play` reports false and
    /// the play loses both reset and the edit/disable warning.
    #[test]
    fn play_properties_carry_a_seed_marker_and_matching_default_rules() {
        for playbook in all_playbooks() {
            for play in &playbook.plays {
                let props = play.properties();
                let seed = props
                    .get("_seed")
                    .unwrap_or_else(|| panic!("{} must carry _seed", play.play_id));
                assert_eq!(
                    seed.get("default_rules"),
                    props.get("rules"),
                    "{}'s shipped default must match its live rules",
                    play.play_id
                );
            }
        }
    }

    /// A seeded view must be a query the backend will execute and a view
    /// config the viewer will honour. Both are plain JSON here, so a typo in
    /// either is otherwise invisible until someone opens the board.
    #[test]
    fn every_view_is_an_executable_query_with_a_renderable_view_config() {
        for playbook in all_playbooks() {
            for view in &playbook.views {
                let definition = &view.definition;
                definition
                    .validate_identifiers()
                    .unwrap_or_else(|e| panic!("{}: {e}", view.view_id));

                let last_view = view.view_config["lastView"].as_str();
                assert!(
                    matches!(last_view, Some("list" | "table" | "kanban")),
                    "{}: lastView must be list, table or kanban, got {last_view:?}",
                    view.view_id
                );
                if last_view == Some("kanban") {
                    assert!(
                        view.view_config["kanban"]["groupBy"].is_string(),
                        "{}: a kanban view needs a groupBy to derive its columns",
                        view.view_id
                    );
                }

                // A misspelt target is a valid identifier and an empty board.
                let target = definition.target_type.as_str();
                let known = playbook.schemas.iter().any(|s| s.schema_id == target)
                    || crate::models::core_schemas::get_core_schemas()
                        .iter()
                        .any(|s| s.id == target);
                assert!(
                    known,
                    "{}: targets `{target}`, which is neither a core type nor one this \
                     playbook creates",
                    view.view_id
                );
            }
        }
    }
}
