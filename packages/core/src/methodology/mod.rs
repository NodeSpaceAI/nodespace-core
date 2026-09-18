//! Methodology recipes — installable, first-party work-tracking setups.
//!
//! A recipe composes NodeSpace's existing authoring primitives (schema
//! creation, enum-vocabulary extension, Play installation, skill seeding)
//! into a setup a user recognizes on day one — "Linear-style", and later
//! others. It adds no platform capability: every step is a call a user or an
//! agent could already make by hand.
//!
//! # First-party content, installed on demand
//!
//! Recipes sit in the same tier as [`crate::models::core_schemas::get_core_schemas`]
//! and `seed_skill_nodes` — typed Rust shipped with the product. They differ
//! in *when* they install. Core schemas and seeded skills are written to every
//! database at daemon startup; a recipe is written only when a user picks one,
//! because a workspace has at most one methodology and the choice is theirs.
//! Nothing here is reachable from `seed_agent_nodes`.
//!
//! # Why core rather than the frontend
//!
//! Schema steps run through [`crate::schema::handle_create_schema`] and
//! [`crate::schema::handle_update_schema`] in-process, so `extends` cycle
//! detection, `mapsTo` resolution against the inherited field, reverse-name
//! validation and relationship-declaration persistence all apply unchanged. A
//! recipe cannot install a schema a hand-authored call could not.

pub mod linear;

use crate::markdown::NodeTemplate;

/// A methodology recipe: the content to install, plus the identity the GUI
/// picker and the generated reference doc both present it under.
pub struct MethodologyRecipe {
    /// Stable machine id (`linear`) — the GUI picker's key and the generated
    /// reference doc's filename stem.
    pub id: &'static str,
    /// Display name ("Linear-style").
    pub name: &'static str,
    /// One-paragraph summary of what installing this sets up.
    pub description: &'static str,
    /// Schemas to create, in order. A schema must precede anything that
    /// targets it: `validate_play_rules` rejects a rule whose trigger names
    /// a type with no schema, so a mis-ordered recipe fails at install time
    /// rather than silently half-installing.
    pub schemas: Vec<SchemaStep>,
    /// Vocabulary extensions applied after the schemas exist.
    pub field_value_extensions: Vec<FieldValueExtension>,
    /// Plays to install, as seeded (resettable) play nodes.
    pub plays: Vec<PlayStep>,
    /// Skill nodes seeded as usage guidance.
    pub skills: Vec<NodeTemplate>,
}

/// One `create_schema` call.
pub struct SchemaStep {
    /// The schema id this call produces, derived from `params.name`. Named
    /// explicitly so collision detection need not re-derive it.
    pub schema_id: &'static str,
    /// A [`crate::schema::CreateSchemaParams`] payload.
    pub params: serde_json::Value,
}

/// One `add_field_values` call against an existing field's vocabulary.
pub struct FieldValueExtension {
    /// Schema whose field is extended. For an inherited field this is the
    /// *extending* schema (`issue`), not the declaring one (`task`) — the
    /// values land on `issue`'s materialized copy, leaving `task`'s own
    /// vocabulary untouched.
    pub schema_id: &'static str,
    /// Field being extended, for progress reporting.
    pub field: &'static str,
    /// An [`crate::schema::UpdateSchemaParams`] payload carrying
    /// `add_field_values`.
    pub params: serde_json::Value,
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

/// Every recipe this build ships.
///
/// The GUI picker and the reference-doc generator both iterate this, so a
/// recipe added here is offered and documented with no further wiring.
pub fn all_recipes() -> Vec<MethodologyRecipe> {
    vec![linear::recipe()]
}

/// Look up a recipe by its [`MethodologyRecipe::id`].
pub fn recipe_by_id(id: &str) -> Option<MethodologyRecipe> {
    all_recipes().into_iter().find(|r| r.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_recipe_has_a_unique_id() {
        let recipes = all_recipes();
        let mut ids: Vec<&str> = recipes.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "recipe ids must be unique");
    }

    #[test]
    fn recipe_by_id_finds_a_shipped_recipe() {
        assert!(recipe_by_id("linear").is_some());
        assert!(recipe_by_id("nonexistent").is_none());
    }

    /// A seeded play carries its rules twice: live, and as the shipped
    /// default. Without the `_seed` marker `is_seeded_play` reports false and
    /// the play loses both reset and the edit/disable warning.
    #[test]
    fn play_properties_carry_a_seed_marker_and_matching_default_rules() {
        for recipe in all_recipes() {
            for play in &recipe.plays {
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
}
