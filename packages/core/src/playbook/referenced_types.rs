//! The node types a play's rules reference.
//!
//! An agent that edits a play has to check a path or a field against the
//! schemas the rules walk (ADR-090 §6). Those are the types each trigger
//! selects, the types its selector's filters and its conditions' paths reach,
//! and the types its actions create, retype to or bind values from.
//!
//! A reference that does not resolve (a type with no schema, a path naming a
//! relationship nobody declares, a saved query that is gone) contributes
//! nothing here: save-time validation is what reports it.

use crate::models::schema::is_reserved_relationship_name;
use crate::ops::path_ops::{resolve_hop, HopResolution};
use crate::playbook::actions::collect_binding_templates_in_value;
use crate::playbook::graph_resolver::declared_collection_type;
use crate::playbook::path_extractor;
use crate::playbook::selectors::selector_query;
use crate::playbook::types::{Action, RuleDefinition, Selector};
use crate::services::NodeService;
use nodespace_types::RelationshipHop;

/// Every type `rules` reference, each once, in the order the rules first
/// name it.
pub async fn referenced_types(node_service: &NodeService, rules: &[RuleDefinition]) -> Vec<String> {
    let mut types = Vec::new();
    for rule in rules {
        let Some(trigger_type) = selected_type(node_service, rule, &mut types).await else {
            // Paths start at the trigger's type; the types the actions name
            // outright still count.
            for action in &rule.actions {
                named_types(action, &mut types);
            }
            continue;
        };

        for condition in &rule.conditions {
            let Ok(extraction) = path_extractor::extract_paths(&condition.expr) else {
                continue;
            };
            let node_paths = extraction
                .paths
                .iter()
                .chain(extraction.collections.iter().map(|c| &c.collection))
                .filter(|path| path.root == "node");
            for path in node_paths {
                let segments: Vec<&str> = path.segments[1..].iter().map(String::as_str).collect();
                walk(node_service, &trigger_type, &segments, &mut types).await;
            }
        }

        for action in &rule.actions {
            named_types(action, &mut types);

            let mut item_type = None;
            if let Some(for_each) = action.for_each() {
                for segments in binding_paths(for_each, "trigger.node.") {
                    walk(node_service, &trigger_type, &segments, &mut types).await;
                    // `.where(...)` is a filter on the collection, not a hop.
                    let collection: Vec<&str> = segments
                        .into_iter()
                        .take_while(|segment| *segment != "where")
                        .collect();
                    item_type = declared_collection_type(node_service, &trigger_type, &collection)
                        .await
                        .ok()
                        .flatten();
                }
            }

            let mut templates = Vec::new();
            collect_binding_templates_in_value(&action.params_value(), &mut templates);
            for template in &templates {
                for segments in binding_paths(template, "trigger.node.") {
                    walk(node_service, &trigger_type, &segments, &mut types).await;
                }
                if let Some(item_type) = &item_type {
                    for segments in binding_paths(template, "item.") {
                        walk(node_service, item_type, &segments, &mut types).await;
                    }
                }
            }
        }
    }
    types
}

/// The type a rule's trigger selects, added to `types` with every type its
/// selector's filters reach. `None` for a selector of every type (`*`) and
/// for a saved query that cannot be read.
async fn selected_type(
    node_service: &NodeService,
    rule: &RuleDefinition,
    types: &mut Vec<String>,
) -> Option<String> {
    let select = rule.trigger.selector();
    let selected = match selector_query(node_service, select).await {
        Ok(query) => {
            add(types, &query.target_type);
            for hop in query
                .filters
                .iter()
                .filter_map(|filter| filter.resolved_path.as_ref())
                .flat_map(|path| &path.hops)
            {
                if let Some(far_type) = &hop.far_type {
                    add(types, far_type);
                }
            }
            query.target_type
        }
        // The selector could not be run. An inline one still names its type.
        Err(_) => match select {
            Selector::Inline(inline) => {
                add(types, &inline.target_type);
                inline.target_type.clone()
            }
            Selector::Query(_) => return None,
        },
    };
    (selected != "*").then_some(selected)
}

/// The types an action names outright: the type it creates, or retypes to.
fn named_types(action: &Action, types: &mut Vec<String>) {
    match action {
        Action::CreateNode { params, .. } => add(types, &params.node_type),
        Action::UpdateNode { params, .. } => {
            if let Some(node_type) = &params.node_type {
                add(types, node_type);
            }
        }
        Action::AddRelationship { .. }
        | Action::RemoveRelationship { .. }
        | Action::Reject { .. } => {}
    }
}

/// Follow `segments` from `start_type` through the relationships the schemas
/// declare, adding each type a hop reaches. Stops at the first segment that
/// is not a relationship: a field, a core key, or a name nobody declares.
async fn walk(
    node_service: &NodeService,
    start_type: &str,
    segments: &[&str],
    types: &mut Vec<String>,
) {
    let mut current = start_type.to_string();
    for segment in segments {
        // A built-in relationship can end on any type, so it names none; the
        // walk carries on from the current type, as validation does.
        if is_reserved_relationship_name(segment) {
            continue;
        }
        let hop = RelationshipHop::fixed(*segment);
        let Ok(HopResolution::Resolved(resolved)) =
            resolve_hop(node_service, Some(&current), &hop).await
        else {
            return;
        };
        let Some(far_type) = resolved.far_type else {
            return;
        };
        add(types, &far_type);
        current = far_type;
    }
}

/// The dot-paths in a binding expression that start with `root`
/// (`trigger.node.` or `item.`), each as its segments after the root.
///
/// A binding is a bare path, a path with `.where(...)` filters, or a function
/// call over paths (`count(trigger.node.tasks)`), so the paths are read out of
/// the text rather than assumed to be the whole of it.
fn binding_paths<'a>(expression: &'a str, root: &str) -> Vec<Vec<&'a str>> {
    expression
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .filter_map(|token| token.strip_prefix(root))
        .map(|rest| rest.split('.').filter(|s| !s.is_empty()).collect())
        .collect()
}

/// Add a type once. `*` selects every type and a `{binding}` is resolved when
/// the rule runs, so neither names one.
fn add(types: &mut Vec<String>, node_type: &str) {
    if node_type == "*" || node_type.contains('{') || types.iter().any(|t| t == node_type) {
        return;
    }
    types.push(node_type.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SqliteStore;
    use crate::schema::handle_create_schema;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn service() -> (Arc<NodeService>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let mut store = Arc::new(SqliteStore::new(tmp.path().join("test.db")).await.unwrap());
        let service = Arc::new(NodeService::new(&mut store).await.unwrap());
        for schema in [
            json!({ "name": "Epic", "fields": [{ "name": "state", "type": "text" }] }),
            json!({
                "name": "Story",
                "fields": [{ "name": "state", "type": "text" }],
                "relationships": [{
                    "name": "epic", "targetType": "epic", "direction": "out",
                    "cardinality": "one", "reverseName": "stories",
                    "reverseCardinality": "many"
                }]
            }),
            json!({ "name": "Retro", "fields": [{ "name": "summary", "type": "text" }] }),
        ] {
            handle_create_schema(&service, schema)
                .await
                .expect("the test's schema is created");
        }
        (service, tmp)
    }

    fn rules(value: serde_json::Value) -> Vec<RuleDefinition> {
        serde_json::from_value(value).expect("the test's rules decode")
    }

    #[tokio::test]
    async fn a_rule_references_its_trigger_its_paths_and_what_its_actions_name() {
        let (service, _tmp) = service().await;
        let rules = rules(json!([{
            "name": "close epic",
            "description": "Close an epic once a story of it is done",
            "trigger": {
                "type": "graph_event", "on": "property_changed",
                "select": { "target_type": "story" }, "property_key": "story.state"
            },
            "conditions": [
                { "expr": "node.epic.state != 'done'", "description": "The epic is still open" }
            ],
            "actions": [{
                "action_type": "create_node",
                "description": "Record a retro for the epic",
                "params": { "node_type": "retro", "content": "{trigger.node.epic.content}" }
            }]
        }]));

        assert_eq!(
            referenced_types(&service, &rules).await,
            ["story", "epic", "retro"]
        );
    }

    /// A reverse name is a relationship too, and `item` paths start at the
    /// type the action's `for_each` iterates.
    #[tokio::test]
    async fn for_each_items_and_reverse_names_are_followed() {
        let (service, _tmp) = service().await;
        let rules = rules(json!([{
            "name": "close stories",
            "description": "Close every story of a finished epic",
            "trigger": {
                "type": "graph_event", "on": "property_changed",
                "select": { "target_type": "epic" }
            },
            "actions": [{
                "action_type": "update_node",
                "description": "Mark the story done",
                "for_each": "trigger.node.stories.where(state != 'done')",
                "params": { "node_id": "{item.id}", "properties": { "state": "done" } }
            }]
        }]));

        assert_eq!(referenced_types(&service, &rules).await, ["epic", "story"]);
    }

    /// What does not resolve names nothing: validation reports it on a write.
    #[tokio::test]
    async fn an_unresolved_reference_contributes_nothing() {
        let (service, _tmp) = service().await;
        let rules = rules(json!([
            {
                "name": "any node",
                "description": "Runs for every node",
                "trigger": { "type": "scheduled", "cron": "0 0 9 * * *", "select": { "target_type": "*" } },
                "actions": [{
                    "action_type": "update_node",
                    "description": "Retype the node the binding names",
                    "params": { "node_id": "{trigger.node.id}", "node_type": "{trigger.node.kind}" }
                }]
            },
            {
                "name": "gone query",
                "description": "Selects by a saved query that is gone",
                "trigger": { "type": "scheduled", "cron": "0 0 9 * * *", "select": { "query_id": "no-such-query" } },
                "conditions": [{ "expr": "node.nowhere.state == 'x'", "description": "Never holds" }]
            }
        ]));

        assert!(referenced_types(&service, &rules).await.is_empty());
    }
}
