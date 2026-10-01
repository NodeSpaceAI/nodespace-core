//! Installing a methodology playbook.
//!
//! Executes a playbook's steps in order against a live `NodeService`, resolving
//! id collisions deterministically and reporting what happened per step.
//!
//! # Collisions are disclosed, never silent
//!
//! A workspace may already contain something called `cycle`. Adopting it would
//! be wrong — a schema sharing a name need not share a shape, and the playbook's
//! Plays would then target a type that does not have the fields they read.
//! Overwriting it would be worse. So a taken id is re-keyed to the first free
//! deterministic suffix (`cycle` -> `cycle__2`) and reported, letting the
//! caller show the user exactly what landed under which name.
//!
//! Re-keying rewrites every later reference to that id within the same install
//! — a Play or a saved view targeting `cycle` follows the rename, so the
//! installed set stays internally consistent rather than half-pointing at a
//! stranger's schema.

use crate::markdown::{prepare_nodes_from_template, MarkdownError, NodeTemplate};
use crate::methodology::skills::{playbook_overview_skill, InstalledIds};
use crate::methodology::{InstallReport, MethodologyPlaybook, StepOutcome, StepReport, ViewStep};
use crate::models::Node;
use crate::playbook::types::{Action, RuleDefinition, Selector, Trigger};
use crate::schema::{handle_create_schema, handle_update_schema};
use crate::services::{FilterType, NodeService, QueryDefinition, QueryFilter};
use std::collections::HashMap;
use std::sync::Arc;

/// How many suffixed ids to try before giving up.
///
/// A workspace with `cycle` through `cycle__16` already in it is not a
/// collision to resolve; something is wrong, and silently creating a
/// seventeenth would compound it.
const MAX_SUFFIX_ATTEMPTS: u32 = 16;

/// Install `playbook` into the graph.
///
/// Steps run in the playbook's declared order — schemas, then vocabulary
/// extensions, then Plays, then skills, then saved views, then the
/// bundle-level overview skill — because each tier depends on the ones before
/// it. A Play whose trigger names a type is rejected by
/// `validate_play_rules` until that type's schema exists, so the order is
/// enforced by the write path rather than merely conventional. The overview
/// comes last because it names the ids every earlier step landed under.
///
/// Stops at the first failure. Later steps are reported as
/// [`StepOutcome::Skipped`] rather than attempted, since a playbook missing its
/// `issue` schema has nothing coherent to install on top.
pub async fn install_playbook(
    node_service: &Arc<NodeService>,
    playbook: &MethodologyPlaybook,
) -> InstallReport {
    let mut steps: Vec<StepReport> = Vec::new();
    let mut renames: HashMap<String, String> = HashMap::new();
    let mut installed = InstalledIds::default();
    let mut failed = false;

    for step in &playbook.schemas {
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
        if let Some(id) = landed_id(&outcome) {
            installed
                .schemas
                .push((step.schema_id.to_string(), id.to_string()));
        }
        failed |= matches!(outcome, StepOutcome::Failed { .. });
        steps.push(StepReport { label, outcome });
    }

    for ext in &playbook.field_value_extensions {
        let label = if ext.adds_field() {
            format!("Add `{}` field to `{}`", ext.field, ext.schema_id)
        } else {
            format!("Extend `{}.{}` vocabulary", ext.schema_id, ext.field)
        };
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

    for play in &playbook.plays {
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
        if let Some(id) = landed_id(&outcome) {
            installed
                .plays
                .push((play.name.to_string(), id.to_string()));
        }
        failed |= matches!(outcome, StepOutcome::Failed { .. });
        steps.push(StepReport { label, outcome });
    }

    for template in &playbook.skills {
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

        let outcome = seed_skill(node_service, &template).await;
        if landed_id(&outcome).is_some() {
            installed.skills.push(template.title.clone());
        }
        failed |= matches!(outcome, StepOutcome::Failed { .. });
        steps.push(StepReport { label, outcome });
    }

    for view in &playbook.views {
        let label = format!("Seed view: {}", view.name);
        if failed {
            steps.push(StepReport::skipped(label));
            continue;
        }

        let properties = ViewStep {
            definition: rewrite_view_step_ids(&view.definition, &renames),
            view_config: view.view_config.clone(),
            ..*view
        }
        .properties();
        let outcome = create_node_resolving_collisions(
            node_service,
            view.view_id,
            "query",
            view.name,
            properties,
        )
        .await;
        if let Some(id) = landed_id(&outcome) {
            installed
                .views
                .push((view.name.to_string(), id.to_string()));
        }
        failed |= matches!(outcome, StepOutcome::Failed { .. });
        steps.push(StepReport { label, outcome });
    }

    let overview = playbook_overview_skill(playbook.overview, &installed);
    let label = format!("Seed skill: {}", overview.title);
    if failed {
        steps.push(StepReport::skipped(label));
    } else {
        let outcome = seed_skill(node_service, &overview).await;
        failed |= matches!(outcome, StepOutcome::Failed { .. });
        steps.push(StepReport { label, outcome });
    }

    InstallReport {
        playbook_id: playbook.id.to_string(),
        success: !failed,
        steps,
    }
}

/// The id a step's target landed under, or `None` if it did not land.
fn landed_id(outcome: &StepOutcome) -> Option<&str> {
    match outcome {
        StepOutcome::Created { id } => Some(id),
        StepOutcome::Suffixed { created, .. } => Some(created),
        StepOutcome::Skipped | StepOutcome::Failed { .. } => None,
    }
}

/// Seed one skill template, reporting it under its title.
async fn seed_skill(node_service: &Arc<NodeService>, template: &NodeTemplate) -> StepOutcome {
    match prepare_nodes_from_template(template) {
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
    // The shipped playbook cannot hit this (both point at core `task`, which is
    // never suffixed), but a playbook whose second schema extends its first
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
    // a string the playbook never wrote.
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

/// The id a node is re-keyed to when its preferred id is taken: a UUID
/// derived from the preferred id and the attempt number.
///
/// Every node other than a date, a schema and the settings singleton has a
/// UUID (ADR-086 §10), so a re-key cannot append a suffix the way a schema's
/// type name does. Deriving it keeps the re-keyed id the same on every device
/// that hits the same collision.
///
/// `None` when the preferred id is not a UUID: there is then nothing to derive
/// a distinct id from, and a seeded node's id must be one.
fn rekeyed_node_id(preferred_id: &str, attempt: u32) -> Option<String> {
    let namespace = uuid::Uuid::parse_str(preferred_id).ok()?;
    Some(uuid::Uuid::new_v5(&namespace, attempt.to_string().as_bytes()).to_string())
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
            match rekeyed_node_id(preferred_id, n + 1) {
                Some(id) => id,
                None => {
                    return StepOutcome::Failed {
                        message: format!(
                            "`{preferred_id}` is taken and is not a UUID, so it cannot be re-keyed"
                        ),
                    }
                }
            }
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
/// `add_field_values` checks the name) or writes an enum value the playbook
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
/// selector, and each action's `node_type` param.
///
/// A blanket walk over these happens to be safe for the shipped playbook, but
/// only by luck: a rule `name` is free-form vocabulary, a CEL condition is a
/// string, and either could spell a schema id. So the rules are decoded as the
/// typed [`RuleDefinition`]s they are and rewritten field by field: which
/// fields hold a schema id is read off the types, not off what the current
/// content happens to contain.
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
    if let Some(rules) = out.get_mut("rules") {
        rewrite_rules(rules, renames);
    }
    if let Some(defaults) = out
        .get_mut("_seed")
        .and_then(|seed| seed.get_mut("default_rules"))
    {
        rewrite_rules(defaults, renames);
    }

    out
}

/// Rewrite the schema ids in a stored `rules` array in place. Rules that do
/// not decode are left as they are: the play's own validation reports them
/// when the step is installed.
fn rewrite_rules(rules: &mut serde_json::Value, renames: &HashMap<String, String>) {
    let Ok(mut decoded) = serde_json::from_value::<Vec<RuleDefinition>>(rules.clone()) else {
        return;
    };
    for rule in &mut decoded {
        rewrite_rule_ids(rule, renames);
    }
    if let Ok(rewritten) = serde_json::to_value(decoded) {
        *rules = rewritten;
    }
}

/// Rewrite the id-bearing fields of one rule in place.
///
/// A trigger's ids are its selector's type (and, for a selector with
/// filters, the type ids those filters compare against) and the namespace of
/// a `property_changed` trigger's `property_key`. `on` and `cron` are not
/// ids, and a saved-query selector names a node, not a type. Of the actions,
/// only a `node_type` param is an id: `relationship_type` is a relationship
/// NAME, matched against `schema.relationships[].name` by the validator, and
/// the other params are node ids, content and field values.
fn rewrite_rule_ids(rule: &mut RuleDefinition, renames: &HashMap<String, String>) {
    match &mut rule.trigger {
        Trigger::GraphEvent {
            select,
            property_key,
            ..
        } => {
            // `property_key` is `<node_type>.<field>`, so its leading segment
            // is the same schema id and must move with the selector's type.
            // Leaving it behind makes the pair jointly incoherent: the
            // trigger indexes under `{issue_2, "issue.status"}` while a real
            // event carries `"issue_2.status"`, and the lookup is an exact
            // match.
            //
            // Guarded on the namespace equalling the AUTHORED type, mirroring
            // `lifecycle::renamespace_property_key` — a key namespaced to
            // some OTHER type is not this rename's business. Read before the
            // selector is rewritten, so there is still a type to match.
            if let (Selector::Inline(inline), Some(key)) = (&*select, property_key.as_mut()) {
                if let (Some(renamed), Some((namespace, field))) =
                    (renames.get(&inline.target_type), key.split_once('.'))
                {
                    if namespace == inline.target_type {
                        *key = format!("{renamed}.{field}");
                    }
                }
            }
            rewrite_selector_ids(select, renames);
        }
        Trigger::Scheduled { select, .. } => rewrite_selector_ids(select, renames),
    }

    for action in &mut rule.actions {
        let node_type = match action {
            Action::CreateNode { params, .. } => Some(&mut params.node_type),
            Action::UpdateNode { params, .. } => params.node_type.as_mut(),
            Action::AddRelationship { .. }
            | Action::RemoveRelationship { .. }
            | Action::Reject { .. } => None,
        };
        if let Some(node_type) = node_type {
            if let Some(renamed) = renames.get(node_type.as_str()) {
                *node_type = renamed.clone();
            }
        }
    }
}

/// Follow a re-key through a selector: its target type, and the value of any
/// filter on `node_type`. A saved-query selector names a query node, which a
/// schema re-key does not touch.
fn rewrite_selector_ids(select: &mut Selector, renames: &HashMap<String, String>) {
    let Selector::Inline(inline) = select else {
        return;
    };
    if let Some(renamed) = renames.get(&inline.target_type) {
        inline.target_type = renamed.clone();
    }
    rewrite_node_type_filters(&mut inline.filters, renames);
}

/// Follow a re-key through a saved view's **id-bearing fields only** —
/// its target type, and the value of any `metadata` filter on `node_type`.
///
/// Enumerated from `QueryDefinition`'s shape, as for the other tiers. A
/// `property` filter's `property` is a field name within the target type and
/// its `value` is vocabulary (an enum value like `in_review` may spell
/// anything); `content` filters match body text; `relationship` filters name
/// a built-in edge kind and a literal node id; sort fields are field names;
/// the view config's `kanban.groupBy` is a field name. None of those is a
/// schema id.
///
/// A `metadata` filter on `node_type` is the exception: its value *is* a type
/// id, compared against the node's stored type. Left behind, it would narrow a
/// retargeted board back onto the stranger's type and show nothing.
fn rewrite_view_step_ids(
    definition: &QueryDefinition,
    renames: &HashMap<String, String>,
) -> QueryDefinition {
    let mut out = definition.clone();
    if let Some(renamed) = renames.get(&out.target_type) {
        out.target_type = renamed.clone();
    }

    rewrite_node_type_filters(&mut out.filters, renames);

    out
}

/// Follow a re-key through the filters that compare against a type id: a
/// `metadata` filter on `node_type`, whose value *is* a type id (a single id
/// for `equals`, a list of ids for `in`). Every other filter's value is
/// vocabulary or text and is left alone.
fn rewrite_node_type_filters(filters: &mut [QueryFilter], renames: &HashMap<String, String>) {
    for filter in filters {
        // A related-node filter's nested filter may compare a type id too.
        if let Some(nested) = filter.filter.as_deref_mut() {
            rewrite_node_type_filters(std::slice::from_mut(nested), renames);
        }
        let is_node_type_filter = filter.filter_type == FilterType::Metadata
            && filter.property.as_deref() == Some("node_type");
        if !is_node_type_filter {
            continue;
        }
        match filter.value.as_mut() {
            Some(serde_json::Value::String(id)) => {
                if let Some(renamed) = renames.get(id.as_str()) {
                    *id = renamed.clone();
                }
            }
            Some(serde_json::Value::Array(ids)) => {
                for id in ids {
                    if let Some(renamed) = id.as_str().and_then(|s| renames.get(s)) {
                        *id = serde_json::json!(renamed);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Follow a re-key through a `create_schema` payload's **id-bearing keys
/// only** — `extends` and each relationship's `targetType`.
///
/// Key-targeted rather than a blanket value walk over the payload, which is
/// what an earlier version of this did. No playbook payload turned out to
/// tolerate a blanket walk: each carries user-authored vocabulary in value
/// position alongside its ids. Here that is `fields[].name`,
/// `friendlyName`, a relationship's `name` and `reverseName`, enum values,
/// `title_template` tokens — and schema ids share one namespace of bare
/// lowercase identifiers with all of it. `cycle`, `issue` and `status` are
/// each plausible as a schema id AND as a field name.
///
/// A blanket walk therefore renames the author's fields behind their back. It
/// fails loudly when a `title_template` references the renamed field, and
/// silently otherwise — storing a field under a name the playbook never wrote,
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
            "trigger": {
                "type": "scheduled",
                "cron": "0 5 0 * * * *",
                "select": {
                    "target_type": "cycle",
                    "filters": [{
                        "type": "metadata", "operator": "equals",
                        "property": "node_type", "value": "cycle",
                    }],
                },
            },
            "actions": [
                {
                    "action_type": "create_node",
                    "params": { "node_type": "cycle", "content": "cycle" },
                },
                {
                    "action_type": "add_relationship",
                    "params": {
                        "source_id": "{actions[0].result.id}",
                        "relationship_type": "tasks",
                        "target_id": "{trigger.node.id}",
                    },
                },
            ],
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
            assert_eq!(
                rule["trigger"]["select"]["target_type"], "cycle_2",
                "in {path}"
            );
            assert_eq!(
                rule["trigger"]["select"]["filters"][0]["value"], "cycle_2",
                "a selector's node_type filter compares against a type id ({path})"
            );
            assert_eq!(
                rule["actions"][0]["params"]["node_type"], "cycle_2",
                "in {path}"
            );
            assert_eq!(
                rule["actions"][0]["params"]["content"], "cycle",
                "content is text, not a reference ({path})"
            );
            assert_eq!(
                rule["name"], "a cycle of work",
                "a rule name is vocabulary, not a reference ({path})"
            );
            assert_eq!(
                rule["actions"][1]["params"]["relationship_type"], "tasks",
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
             playbook never authored"
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

    /// A `node_type` filter nested inside a related-node filter follows the
    /// rename too.
    #[test]
    fn a_nested_node_type_filter_follows_a_rename() {
        let mut renames = HashMap::new();
        renames.insert("issue".to_string(), "issue_2".to_string());

        let mut filters: Vec<QueryFilter> = serde_json::from_value(serde_json::json!([{
            "type": "related", "operator": "equals", "path": ["blocks"],
            "filter": {
                "type": "metadata", "operator": "equals",
                "property": "node_type", "value": "issue"
            }
        }]))
        .unwrap();
        rewrite_node_type_filters(&mut filters, &renames);

        assert_eq!(
            filters[0].filter.as_ref().unwrap().value,
            Some(serde_json::json!("issue_2"))
        );
        // A path names relationships, never a schema id.
        assert_eq!(
            serde_json::to_value(&filters[0].path).unwrap(),
            serde_json::json!(["blocks"])
        );
    }

    /// A view follows a rename through its target and any `node_type`
    /// metadata filter, and leaves every field name and value alone.
    #[test]
    fn a_views_target_and_node_type_filter_follow_a_rename() {
        let mut renames = HashMap::new();
        renames.insert("issue".to_string(), "issue_2".to_string());

        let definition: QueryDefinition = serde_json::from_value(serde_json::json!({
            "targetType": "issue",
            "filters": [
                {
                    "type": "metadata", "operator": "equals",
                    "property": "node_type", "value": "issue",
                },
                {
                    "type": "metadata", "operator": "in",
                    "property": "node_type", "value": ["issue", "task"],
                },
                {
                    "type": "property", "operator": "equals",
                    "property": "issue", "value": "issue",
                },
                {
                    "type": "metadata", "operator": "contains",
                    "property": "content", "value": "issue",
                },
            ],
            "sorting": [{ "field": "issue", "direction": "asc" }],
        }))
        .unwrap();

        let out = rewrite_view_step_ids(&definition, &renames);

        assert_eq!(out.target_type, "issue_2", "the target is a reference");
        assert_eq!(
            out.filters[0].value,
            Some(serde_json::json!("issue_2")),
            "a node_type filter's value is a type id"
        );
        assert_eq!(
            out.filters[1].value,
            Some(serde_json::json!(["issue_2", "task"])),
            "each id in a node_type `in` filter follows the rename; others are untouched"
        );

        assert_eq!(
            out.filters[2], definition.filters[2],
            "a property filter's field and value are vocabulary"
        );
        assert_eq!(
            out.filters[3], definition.filters[3],
            "a metadata filter on another field carries text, not a type id"
        );
        assert_eq!(
            out.sorting, definition.sorting,
            "a sort field is a field name"
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
        let view = QueryDefinition {
            target_type: "cycle".to_string(),
            filters: vec![],
            sorting: None,
            limit: None,
        };
        let out = rewrite_view_step_ids(&view, &empty);
        assert_eq!(out.target_type, view.target_type);
        assert_eq!(out.filters, view.filters);
        assert_eq!(out.sorting, view.sorting);
        assert_eq!(out.limit, view.limit);
    }
}
