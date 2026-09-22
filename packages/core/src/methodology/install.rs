//! Installing a methodology recipe.
//!
//! Executes a recipe's steps in order against a live `NodeService`, resolving
//! id collisions deterministically and reporting what happened per step.
//!
//! # Collisions are disclosed, never silent
//!
//! A workspace may already contain something called `cycle`. Adopting it would
//! be wrong — a schema sharing a name need not share a shape, and the recipe's
//! Plays would then target a type that does not have the fields they read.
//! Overwriting it would be worse. So a taken id is re-keyed to the first free
//! deterministic suffix (`cycle` -> `cycle__2`) and reported, letting the
//! caller show the user exactly what landed under which name.
//!
//! Re-keying rewrites every later reference to that id within the same install
//! — a Play targeting `cycle` follows the rename, so the installed set stays
//! internally consistent rather than half-pointing at a stranger's schema.

use crate::markdown::{prepare_nodes_from_template, MarkdownError};
use crate::methodology::{InstallReport, MethodologyRecipe, StepOutcome, StepReport};
use crate::models::Node;
use crate::schema::{handle_create_schema, handle_update_schema};
use crate::services::NodeService;
use std::collections::HashMap;
use std::sync::Arc;

/// How many suffixed ids to try before giving up.
///
/// A workspace with `cycle` through `cycle__16` already in it is not a
/// collision to resolve; something is wrong, and silently creating a
/// seventeenth would compound it.
const MAX_SUFFIX_ATTEMPTS: u32 = 16;

/// Install `recipe` into the graph.
///
/// Steps run in the recipe's declared order — schemas, then vocabulary
/// extensions, then Plays, then skills — because each tier depends on the one
/// before it. A Play whose trigger names a type is rejected by
/// `validate_play_rules` until that type's schema exists, so the order is
/// enforced by the write path rather than merely conventional.
///
/// Stops at the first failure. Later steps are reported as
/// [`StepOutcome::Skipped`] rather than attempted, since a recipe missing its
/// `issue` schema has nothing coherent to install on top.
pub async fn install_recipe(
    node_service: &Arc<NodeService>,
    recipe: &MethodologyRecipe,
) -> InstallReport {
    let mut steps: Vec<StepReport> = Vec::new();
    let mut renames: HashMap<String, String> = HashMap::new();
    let mut failed = false;

    for step in &recipe.schemas {
        if failed {
            steps.push(StepReport::skipped(format!(
                "Create `{}` schema",
                step.schema_id
            )));
            continue;
        }

        let label = format!("Create `{}` schema", step.schema_id);
        let outcome = create_schema_resolving_collisions(node_service, step, &renames).await;
        if let StepOutcome::Suffixed { created, .. } = &outcome {
            renames.insert(step.schema_id.to_string(), created.clone());
        }
        failed |= matches!(outcome, StepOutcome::Failed { .. });
        steps.push(StepReport { label, outcome });
    }

    for ext in &recipe.field_value_extensions {
        let label = format!("Extend `{}.{}` vocabulary", ext.schema_id, ext.field);
        if failed {
            steps.push(StepReport::skipped(label));
            continue;
        }

        let params = rewrite_update_schema_step_ids(&ext.params, &renames);
        let outcome = match handle_update_schema(node_service, params).await {
            Ok(_) => StepOutcome::Created {
                id: resolved_id(ext.schema_id, &renames),
            },
            Err(e) => StepOutcome::Failed {
                message: e.to_string(),
            },
        };
        failed |= matches!(outcome, StepOutcome::Failed { .. });
        steps.push(StepReport { label, outcome });
    }

    for play in &recipe.plays {
        let label = format!("Install Play: {}", play.name);
        if failed {
            steps.push(StepReport::skipped(label));
            continue;
        }

        let properties = rewrite_play_step_ids(&play.properties(), &renames);
        let outcome = create_node_resolving_collisions(
            node_service,
            play.play_id,
            "play",
            play.name,
            properties,
        )
        .await;
        failed |= matches!(outcome, StepOutcome::Failed { .. });
        steps.push(StepReport { label, outcome });
    }

    for template in &recipe.skills {
        let label = format!("Seed skill: {}", template.title);
        if failed {
            steps.push(StepReport::skipped(label));
            continue;
        }

        // Guidance names schema ids in prose, so a re-key would leave it
        // pointing at the stranger's schema the re-key existed to avoid —
        // actively misleading, since that type has none of the fields the
        // guidance describes. Appended as a note rather than rewritten in
        // place: the markdown is sentences, and blind value substitution
        // would corrupt any that happened to contain the word.
        let template = match rename_note(&renames) {
            Some(note) => {
                let mut annotated = template.clone();
                annotated.markdown_content.push_str(&note);
                std::borrow::Cow::Owned(annotated)
            }
            None => std::borrow::Cow::Borrowed(template),
        };

        let outcome = match prepare_nodes_from_template(&template) {
            Ok(nodes) => match node_service.seed_nodes_from_templates(vec![nodes]).await {
                Ok(_) => StepOutcome::Created {
                    id: template.title.clone(),
                },
                Err(e) => StepOutcome::Failed {
                    message: e.to_string(),
                },
            },
            Err(e) => StepOutcome::Failed {
                message: format!("{e}"),
            },
        };
        failed |= matches!(outcome, StepOutcome::Failed { .. });
        steps.push(StepReport { label, outcome });
    }

    InstallReport {
        recipe_id: recipe.id.to_string(),
        success: !failed,
        steps,
    }
}

/// Create a schema, re-keying its id if something already holds it.
///
/// The collision is detected from `create_schema`'s own rejection rather than
/// a prior existence check: checking first would race, and the write path is
/// the only authority on whether an id is free.
async fn create_schema_resolving_collisions(
    node_service: &Arc<NodeService>,
    step: &crate::methodology::SchemaStep,
    renames: &HashMap<String, String>,
) -> StepOutcome {
    // Schema params carry ids too — `extends` names a parent, a relationship's
    // `targetType` names a target — so an earlier re-key has to reach them.
    // The shipped recipe cannot hit this (both point at core `task`, which is
    // never suffixed), but a recipe whose second schema extends its first
    // would otherwise silently extend the stranger's schema.
    let params = rewrite_schema_step_ids(&step.params, renames);

    match handle_create_schema(node_service, params.clone()).await {
        Ok(_) => {
            return StepOutcome::Created {
                id: step.schema_id.to_string(),
            }
        }
        Err(MarkdownError::AlreadyExists { .. }) => {}
        Err(e) => {
            return StepOutcome::Failed {
                message: e.to_string(),
            }
        }
    }

    // Taken. `name` drives the derived id, so suffixing the name is what
    // moves the schema to a free id.
    // The author's own name, never a rewritten one: `name` is display text
    // that derives the id, so substituting it would build the suffix ladder on
    // a string the recipe never wrote.
    let base_name = step.params["name"].as_str().unwrap_or(step.schema_id);
    for n in 2..=MAX_SUFFIX_ATTEMPTS {
        let mut params = params.clone();
        params["name"] = serde_json::json!(format!("{base_name} {n}"));

        match handle_create_schema(node_service, params).await {
            Ok(result) => {
                let created = result["schemaId"]
                    .as_str()
                    .unwrap_or(&format!("{}__{n}", step.schema_id))
                    .to_string();
                return StepOutcome::Suffixed {
                    requested: step.schema_id.to_string(),
                    created,
                };
            }
            Err(MarkdownError::AlreadyExists { .. }) => continue,
            Err(e) => {
                return StepOutcome::Failed {
                    message: e.to_string(),
                }
            }
        }
    }

    StepOutcome::Failed {
        message: format!(
            "`{}` and {} suffixed variants are all taken — resolve the existing types before \
             installing this methodology",
            step.schema_id,
            MAX_SUFFIX_ATTEMPTS - 1
        ),
    }
}

/// Create a node under `preferred_id`, re-keying if that id is taken.
///
/// Mirrors `create_schema_resolving_collisions` above: the create is
/// attempted directly, with no prior existence check — checking first would
/// race, exactly as it would for schemas, since two installs could both see
/// an id as free and both proceed. When the create fails, a follow-up read
/// at the same id is what interprets the rejection, not what gates the
/// attempt: something now occupying `id` means the create lost a genuine
/// collision (try the next suffix); `id` still being free means the
/// rejection was a real fault (report it, rather than burning through every
/// suffix on the same underlying error). A failure of that follow-up read
/// itself is reported too, never treated as "no collision".
async fn create_node_resolving_collisions(
    node_service: &Arc<NodeService>,
    preferred_id: &str,
    node_type: &str,
    content: &str,
    properties: serde_json::Value,
) -> StepOutcome {
    for n in 0..MAX_SUFFIX_ATTEMPTS {
        let id = if n == 0 {
            preferred_id.to_string()
        } else {
            format!("{preferred_id}__{}", n + 1)
        };

        let node = Node::new_with_id(
            id.clone(),
            node_type.to_string(),
            content.to_string(),
            properties.clone(),
        );
        match node_service.create_node(node).await {
            Ok(_) if n == 0 => return StepOutcome::Created { id },
            Ok(_) => {
                return StepOutcome::Suffixed {
                    requested: preferred_id.to_string(),
                    created: id,
                }
            }
            Err(create_err) => match node_service.get_node(&id).await {
                // `id` is now occupied — the create lost a real collision.
                // Try the next suffix.
                Ok(Some(_)) => continue,
                // `id` is free; the create failed for a real reason.
                Ok(None) => {
                    return StepOutcome::Failed {
                        message: create_err.to_string(),
                    }
                }
                // Couldn't even confirm why — report both rather than
                // guessing which one it was.
                Err(read_err) => {
                    return StepOutcome::Failed {
                        message: format!(
                        "create failed ({create_err}), and confirming why also failed: {read_err}"
                    ),
                    }
                }
            },
        }
    }

    StepOutcome::Failed {
        message: format!("`{preferred_id}` and every suffixed variant are already taken"),
    }
}

/// A markdown note naming every re-keyed id, appended to seeded guidance when
/// an install had to move a schema aside.
///
/// Returns `None` when nothing was re-keyed, which is the common case — the
/// guidance then ships exactly as authored.
fn rename_note(renames: &HashMap<String, String>) -> Option<String> {
    if renames.is_empty() {
        return None;
    }

    // Sorted so the note is stable across installs rather than following
    // HashMap iteration order.
    let mut pairs: Vec<(&String, &String)> = renames.iter().collect();
    pairs.sort_unstable();

    let mut note = String::from(
        "\n\n## Type names in this workspace\n\n\
         Some names this guidance uses were already taken when the methodology \
         was installed, so the types were created under different ones. \
         Wherever this skill names the first, use the second:\n\n",
    );
    for (requested, created) in pairs {
        note.push_str(&format!("- `{requested}` → `{created}`\n"));
    }
    note.push_str(
        "\nThe types already holding the original names belong to something else \
         and are unrelated to this methodology.\n",
    );
    Some(note)
}

/// Follow a re-key through the `targetType` of each relationship in
/// `params[key]`.
///
/// Shared because `create_schema` spells the array `relationships` and
/// `update_schema` spells it `add_relationships`, but the element shape and
/// the rule are identical. Two near-identical copies is how a fix reaches one
/// and not the other — the shape of defect this PR has already hit three
/// times.
///
/// Only `targetType` moves. A relationship's `name` and `reverseName` are
/// vocabulary that may legitimately spell a schema id.
fn rewrite_relationship_targets(
    params: &mut serde_json::Value,
    key: &str,
    renames: &HashMap<String, String>,
) {
    let Some(relationships) = params.get_mut(key).and_then(|v| v.as_array_mut()) else {
        return;
    };
    for relationship in relationships {
        let Some(target) = relationship.get("targetType").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(renamed) = renames.get(target) {
            relationship["targetType"] = serde_json::json!(renamed);
        }
    }
}

/// Follow a re-key through an `update_schema` payload's **id-bearing keys
/// only** — `schema_id`, `extends`, and each added relationship's
/// `targetType`.
///
/// The `create_schema` reasoning applies unchanged: `UpdateSchemaParams` mixes
/// references and user vocabulary in value position. `add_field_values[].field`
/// and its `values[].value`, `add_fields[].name`, `remove_fields`,
/// `rename_fields`, and both template strings are vocabulary. Rewriting them
/// retargets an extension at a field that does not exist (loud, since
/// `add_field_values` checks the name) or writes an enum value the recipe
/// never authored (silent).
fn rewrite_update_schema_step_ids(
    params: &serde_json::Value,
    renames: &HashMap<String, String>,
) -> serde_json::Value {
    let mut out = params.clone();
    if renames.is_empty() {
        return out;
    }

    for key in ["schema_id", "extends"] {
        if let Some(id) = out.get(key).and_then(|v| v.as_str()) {
            if let Some(renamed) = renames.get(id) {
                out[key] = serde_json::json!(renamed);
            }
        }
    }

    rewrite_relationship_targets(&mut out, "add_relationships", renames);

    out
}

/// Follow a re-key through a play node's properties — every rule's trigger
/// `node_type`, and each action's `node_type` / `target_type` params.
///
/// A blanket walk over these happens to be safe for the shipped recipe, but
/// only by luck: a rule `name` is free-form vocabulary, a CEL condition is a
/// string, and either could spell a schema id. Twice in this PR a payload was
/// judged safe by inspecting the recipe rather than the shape, and twice that
/// was wrong — so all three payload kinds are key-targeted, and none depends
/// on what the current content happens to contain.
fn rewrite_play_step_ids(
    properties: &serde_json::Value,
    renames: &HashMap<String, String>,
) -> serde_json::Value {
    let mut out = properties.clone();
    if renames.is_empty() {
        return out;
    }

    // `rules` is mirrored under `_seed.default_rules`, and both must follow the
    // rename or a reset would restore rules pointing at the stranger's schema.
    if let Some(rules) = out.get_mut("rules").and_then(|v| v.as_array_mut()) {
        for rule in rules {
            rewrite_rule_ids(rule, renames);
        }
    }
    if let Some(defaults) = out
        .get_mut("_seed")
        .and_then(|seed| seed.get_mut("default_rules"))
        .and_then(|v| v.as_array_mut())
    {
        for rule in defaults {
            rewrite_rule_ids(rule, renames);
        }
    }

    out
}

/// Rewrite the id-bearing keys of one rule in place.
///
/// `TriggerDefinition`'s five fields: `node_type` and `property_key` are
/// id-bearing (the latter through its namespace, see below); `type`, `on` and
/// `cron` are not. `ActionDefinition`'s three: `params.node_type` and
/// `params.target_type` are ids, while `action_type` and `for_each` are not —
/// and `params.relationship_type` is a relationship NAME, matched against
/// `schema.relationships[].name` by the validator, never a schema id.
fn rewrite_rule_ids(rule: &mut serde_json::Value, renames: &HashMap<String, String>) {
    // Read before mutating: `property_key`'s namespace is compared against the
    // node type the rule was AUTHORED with, so rewriting `node_type` first
    // would leave nothing to match against.
    let authored_node_type = rule
        .get("trigger")
        .and_then(|t| t.get("node_type"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // A trigger carrying a `property_key` but no `node_type` never reaches the
    // rewrite below. That is unreachable by construction rather than handled:
    // the namespace guard compares against the authored `node_type`, so with
    // none there is nothing to match, and a `property_changed` trigger without
    // a `node_type` has no type to namespace its key to in the first place.
    if let Some(node_type) = authored_node_type.as_deref() {
        let Some(renamed) = renames.get(node_type) else {
            return rewrite_action_ids(rule, renames);
        };
        rule["trigger"]["node_type"] = serde_json::json!(renamed);

        // `property_key` is `<node_type>.<field>`, so its leading segment is
        // the same schema id and must move with it. Leaving it behind makes
        // the pair jointly incoherent: the trigger indexes under
        // `{issue_2, "issue.status"}` while a real event carries
        // `"issue_2.status"`, and the lookup is an exact match.
        //
        // Guarded on the namespace equalling the authored type, mirroring
        // `lifecycle::renamespace_property_key` — a key namespaced to some
        // OTHER type is not this rename's business.
        if let Some(key) = rule["trigger"]
            .get("property_key")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        {
            if let Some((namespace, field)) = key.split_once('.') {
                if namespace == node_type {
                    rule["trigger"]["property_key"] =
                        serde_json::json!(format!("{renamed}.{field}"));
                }
            }
        }
    }

    rewrite_action_ids(rule, renames);
}

/// Rewrite the id-bearing params of one rule's actions in place.
fn rewrite_action_ids(rule: &mut serde_json::Value, renames: &HashMap<String, String>) {
    let Some(actions) = rule.get_mut("actions").and_then(|v| v.as_array_mut()) else {
        return;
    };
    for action in actions {
        let Some(params) = action.get_mut("params") else {
            continue;
        };
        // `node_type` names a type to create; `target_type` names one to
        // relate to. `relationship_type` is deliberately absent: it is a
        // relationship name, not a schema id.
        for key in ["node_type", "target_type"] {
            if let Some(id) = params.get(key).and_then(|v| v.as_str()) {
                if let Some(renamed) = renames.get(id) {
                    params[key] = serde_json::json!(renamed);
                }
            }
        }
    }
}

/// Follow a re-key through a `create_schema` payload's **id-bearing keys
/// only** — `extends` and each relationship's `targetType`.
///
/// Key-targeted rather than a blanket value walk over the payload, which is
/// what an earlier version of this did. No recipe payload turned out to
/// tolerate a blanket walk: each carries user-authored vocabulary in value
/// position alongside its ids. Here that is `fields[].name`,
/// `friendlyName`, a relationship's `name` and `reverseName`, enum values,
/// `title_template` tokens — and schema ids share one namespace of bare
/// lowercase identifiers with all of it. `cycle`, `issue` and `status` are
/// each plausible as a schema id AND as a field name.
///
/// A blanket walk therefore renames the author's fields behind their back. It
/// fails loudly when a `title_template` references the renamed field, and
/// silently otherwise — storing a field under a name the recipe never wrote,
/// which is the corruption re-keying exists to prevent.
///
/// The id-bearing keys were enumerated from `CreateSchemaParams`' six fields:
/// `name` is display text (and derives the id, so substituting it would build
/// a suffix ladder on a string the author never wrote), `description` is
/// prose, `fields` and `title_template` are user vocabulary. That leaves
/// `extends` and each relationship's `targetType`.
///
/// `EdgeField` also carries a `target_type`, deliberately not handled: it has
/// no consumer anywhere in core — declared, never read — so it cannot hold a
/// live schema reference. If one is ever wired up, it belongs here.
fn rewrite_schema_step_ids(
    params: &serde_json::Value,
    renames: &HashMap<String, String>,
) -> serde_json::Value {
    let mut out = params.clone();
    if renames.is_empty() {
        return out;
    }

    if let Some(parent) = out.get("extends").and_then(|v| v.as_str()) {
        if let Some(renamed) = renames.get(parent) {
            out["extends"] = serde_json::json!(renamed);
        }
    }

    rewrite_relationship_targets(&mut out, "relationships", renames);

    out
}

fn resolved_id(id: &str, renames: &HashMap<String, String>) -> String {
    renames.get(id).cloned().unwrap_or_else(|| id.to_string())
}

impl StepReport {
    fn skipped(label: String) -> Self {
        Self {
            label,
            outcome: StepOutcome::Skipped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Play's rules follow a rename through both the live copy and the
    /// `_seed.default_rules` mirror.
    ///
    /// The mirror matters: `reset_seeded_play_to_default` restores from it, so
    /// a mirror left pointing at the stranger's schema would silently undo the
    /// re-key the moment a user reset the Play.
    #[test]
    fn a_plays_rules_and_seed_mirror_both_follow_a_rename() {
        let mut renames = HashMap::new();
        renames.insert("cycle".to_string(), "cycle_2".to_string());

        let rules = serde_json::json!([{
            "name": "a cycle of work",
            "trigger": { "type": "scheduled", "node_type": "cycle" },
            "actions": [{
                "action_type": "create_node",
                "params": { "node_type": "cycle", "relationship_type": "tasks" },
            }],
        }]);
        let properties = serde_json::json!({
            "rules": rules,
            "_seed": { "default_rules": rules },
        });

        let out = rewrite_play_step_ids(&properties, &renames);

        for path in ["rules", "_seed"] {
            let rule = if path == "rules" {
                &out["rules"][0]
            } else {
                &out["_seed"]["default_rules"][0]
            };
            assert_eq!(rule["trigger"]["node_type"], "cycle_2", "in {path}");
            assert_eq!(
                rule["actions"][0]["params"]["node_type"], "cycle_2",
                "in {path}"
            );
            assert_eq!(
                rule["name"], "a cycle of work",
                "a rule name is vocabulary, not a reference ({path})"
            );
            assert_eq!(
                rule["actions"][0]["params"]["relationship_type"], "tasks",
                "a relationship_type is a name, not a schema id ({path})"
            );
        }
    }

    /// An update payload follows ids and leaves field vocabulary alone.
    #[test]
    fn an_update_payload_rewrites_ids_but_not_field_names() {
        let mut renames = HashMap::new();
        renames.insert("cycle".to_string(), "cycle_2".to_string());

        let params = serde_json::json!({
            "schema_id": "cycle",
            "add_field_values": [{
                "field": "cycle",
                "values": [{ "value": "cycle", "label": "Cycle" }],
            }],
            "add_relationships": [{
                "name": "cycle",
                "targetType": "cycle",
                "reverseName": "cycle",
            }],
        });

        let out = rewrite_update_schema_step_ids(&params, &renames);

        assert_eq!(
            out["schema_id"], "cycle_2",
            "the target schema is a reference"
        );
        assert_eq!(
            out["add_relationships"][0]["targetType"], "cycle_2",
            "a relationship target is a reference"
        );

        assert_eq!(
            out["add_field_values"][0]["field"], "cycle",
            "a field name is vocabulary — rewriting it retargets the extension \
             at a field that does not exist"
        );
        assert_eq!(
            out["add_field_values"][0]["values"][0]["value"], "cycle",
            "an enum value is vocabulary — rewriting it writes a value the \
             recipe never authored"
        );
        assert_eq!(
            out["add_relationships"][0]["name"], "cycle",
            "a relationship name is vocabulary"
        );
        assert_eq!(
            out["add_relationships"][0]["reverseName"], "cycle",
            "a reverse name is vocabulary"
        );
    }

    #[test]
    fn rewrites_are_identity_without_renames() {
        let empty = HashMap::new();
        let schema = serde_json::json!({ "extends": "cycle" });
        let update = serde_json::json!({ "schema_id": "cycle" });
        let play = serde_json::json!({ "rules": [] });

        assert_eq!(rewrite_schema_step_ids(&schema, &empty), schema);
        assert_eq!(rewrite_update_schema_step_ids(&update, &empty), update);
        assert_eq!(rewrite_play_step_ids(&play, &empty), play);
    }
}
