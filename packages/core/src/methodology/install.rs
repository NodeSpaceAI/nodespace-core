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

        let params = rewrite_schema_ids(&ext.params, &renames);
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

        let properties = rewrite_schema_ids(&play.properties(), &renames);
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

/// Rewrite schema ids in a payload to follow any collision re-keying.
///
/// Walks the JSON rather than matching known key names: a schema id appears as
/// `schema_id`, as a trigger's `node_type`, as a relationship `targetType`, and
/// inside a Play's nested rules. Missing one would leave a step pointing at the
/// stranger's schema the re-key existed to avoid.
///
/// Only exact string matches are rewritten, so `"cycle"` becomes `"cycle__2"`
/// while prose mentioning a cycle is untouched.
fn rewrite_schema_ids(
    value: &serde_json::Value,
    renames: &HashMap<String, String>,
) -> serde_json::Value {
    if renames.is_empty() {
        return value.clone();
    }
    match value {
        serde_json::Value::String(s) => match renames.get(s) {
            Some(replacement) => serde_json::json!(replacement),
            None => value.clone(),
        },
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|v| rewrite_schema_ids(v, renames))
                .collect(),
        ),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), rewrite_schema_ids(v, renames)))
                .collect(),
        ),
        _ => value.clone(),
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

/// Follow a re-key through a `create_schema` payload's **id-bearing keys
/// only** — `extends` and each relationship's `targetType`.
///
/// Deliberately NOT [`rewrite_schema_ids`], which is a blanket value walk.
/// That is safe for the Play, vocabulary and skill payloads, whose ids all sit
/// in value position. A `create_schema` payload is a different shape: it also
/// carries user-authored vocabulary in value position — `fields[].name`,
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

    if let Some(relationships) = out.get_mut("relationships").and_then(|v| v.as_array_mut()) {
        for relationship in relationships {
            let Some(target) = relationship.get("targetType").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Some(renamed) = renames.get(target) {
                relationship["targetType"] = serde_json::json!(renamed);
            }
        }
    }

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

    #[test]
    fn rewrite_follows_a_rename_through_nested_payloads() {
        let mut renames = HashMap::new();
        renames.insert("cycle".to_string(), "cycle__2".to_string());

        let payload = serde_json::json!({
            "rules": [{
                "trigger": { "node_type": "cycle", "type": "scheduled" },
                "actions": [{ "params": { "node_type": "cycle" } }],
            }],
            "unrelated": "a cycle of work",
        });

        let out = rewrite_schema_ids(&payload, &renames);
        assert_eq!(out["rules"][0]["trigger"]["node_type"], "cycle__2");
        assert_eq!(
            out["rules"][0]["actions"][0]["params"]["node_type"],
            "cycle__2"
        );
        assert_eq!(
            out["unrelated"], "a cycle of work",
            "only exact id matches are rewritten, never prose"
        );
    }

    #[test]
    fn rewrite_is_identity_without_renames() {
        let payload = serde_json::json!({ "node_type": "cycle" });
        assert_eq!(rewrite_schema_ids(&payload, &HashMap::new()), payload);
    }
}
