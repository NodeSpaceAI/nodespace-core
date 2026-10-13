//! What names a schema's fields (ADR-100 §5).
//!
//! A stored field name is a key other things are written against: a saved
//! query filters, sorts or groups on it, a play's rule triggers on it, checks
//! it or writes it, and a title template interpolates it. Renaming or removing
//! the field leaves each of them naming something that is gone, so a change
//! to a field is preceded by [`find_field_references`].
//!
//! The lookup is a search over stored definitions with no model call, and it
//! reports what it searched ([`FieldReferences::searched`]) so a caller can
//! say what an empty result does and does not rule out.

use crate::models::PlayFields;
use crate::services::{NodeService, NodeServiceError};
use nodespace_types::{QueryFields, QueryFilter};
use serde::Serialize;
use serde_json::Value;

/// What kind of definition names a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    SavedQuery,
    Play,
    TitleTemplate,
}

/// One definition that names one of the schema's own fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldReference {
    /// The field that is named.
    pub field: String,
    pub kind: ReferenceKind,
    /// The node holding the definition: the query, the play or the schema
    /// whose title template it is.
    pub id: String,
    pub title: String,
    /// Where in the definition: `filter on status`, `rule "close parent",
    /// condition 2`.
    pub place: String,
}

/// The references to a schema's fields, and what was searched to find them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldReferences {
    pub references: Vec<FieldReference>,
    /// The definitions that were read, in words: what a result of none does
    /// not rule out is whatever is not listed here.
    pub searched: Vec<String>,
}

/// Find the saved queries, plays and title templates that name a field
/// `schema_id` declares.
///
/// What is searched is the type and the types that extend it, since a type
/// that extends it reads the field too:
///
/// - saved queries that select one of those types: their filters, sorting and
///   board grouping, but not a condition on a node reached through a
///   relationship, which names a field of the type at its end;
/// - plays with a rule that triggers on one of those types: the trigger's
///   property key and selector filters, each condition's expression and each
///   action's parameters;
/// - the title template of each of those types.
pub async fn find_field_references(
    node_service: &NodeService,
    schema_id: &str,
) -> Result<FieldReferences, NodeServiceError> {
    let searched = vec![
        "saved queries over this type or a type that extends it: filters, sorting and board grouping (a condition on a node reached through a relationship is not followed)".to_string(),
        "plays with a rule that triggers on this type or a type that extends it: the trigger, conditions and actions (a rule that selects by saved query is not matched, and a mention of the name inside a message is counted)".to_string(),
        "title templates of this type and the types that extend it".to_string(),
    ];

    let Some(schema) = node_service.get_schema_node(schema_id).await? else {
        return Ok(FieldReferences {
            references: Vec::new(),
            searched,
        });
    };
    let fields: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
    if fields.is_empty() {
        return Ok(FieldReferences {
            references: Vec::new(),
            searched,
        });
    }

    // The type and every type whose `extends` chain reaches it.
    let mut family: Vec<(String, String, Option<String>)> = Vec::new();
    for other in node_service.get_all_schemas().await? {
        let id = other.envelope.id.clone();
        let chain = node_service.resolve_type_chain(&id).await?;
        if chain.iter().any(|link| link == schema_id) {
            family.push((id, other.envelope.content.clone(), other.title_template));
        }
    }
    let in_family = |type_id: &str| family.iter().any(|(id, _, _)| id == type_id);

    let mut references = Vec::new();

    for node in node_service.query_nodes_by_type("query", false).await? {
        let Ok(query) = QueryFields::from_properties(&node.properties) else {
            continue;
        };
        if !in_family(&query.target_type) {
            continue;
        }
        let title = node_title(&node.content, &node.id);
        for field in &fields {
            for place in query_places(&query, field) {
                references.push(FieldReference {
                    field: (*field).to_string(),
                    kind: ReferenceKind::SavedQuery,
                    id: node.id.clone(),
                    title: title.clone(),
                    place,
                });
            }
        }
    }

    for node in node_service.query_nodes_by_type("play", false).await? {
        let Ok(play) = PlayFields::from_properties(&node.properties) else {
            continue;
        };
        let title = node_title(&node.content, &node.id);
        for rule in &play.rules {
            let Ok(Value::Object(rule_json)) = serde_json::to_value(rule) else {
                continue;
            };
            let triggers_here = rule_json
                .get("trigger")
                .and_then(|t| t.get("select"))
                .and_then(|s| s.get("target_type"))
                .and_then(Value::as_str)
                .is_some_and(in_family);
            if !triggers_here {
                continue;
            }
            for field in &fields {
                for place in rule_places(&rule_json, &rule.name, field) {
                    references.push(FieldReference {
                        field: (*field).to_string(),
                        kind: ReferenceKind::Play,
                        id: node.id.clone(),
                        title: title.clone(),
                        place,
                    });
                }
            }
        }
    }

    for (id, name, template) in &family {
        let Some(template) = template else { continue };
        for field in &fields {
            if template.contains(&format!("{{{field}}}")) {
                references.push(FieldReference {
                    field: (*field).to_string(),
                    kind: ReferenceKind::TitleTemplate,
                    id: id.clone(),
                    title: node_title(name, id),
                    place: format!("title template \"{template}\""),
                });
            }
        }
    }

    Ok(FieldReferences {
        references,
        searched,
    })
}

fn node_title(content: &str, id: &str) -> String {
    let title = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        id.to_string()
    } else {
        title
    }
}

/// Whether `stored` names `field`: the field itself, or a path into it
/// (`repository.url`).
fn names_field(stored: &str, field: &str) -> bool {
    stored == field
        || stored
            .strip_prefix(field)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// Where a saved query names `field`.
fn query_places(query: &QueryFields, field: &str) -> Vec<String> {
    let mut places = Vec::new();
    if query
        .filters
        .iter()
        .any(|filter| filter_names(filter, field))
    {
        places.push(format!("filter on {field}"));
    }
    if query
        .sorting
        .iter()
        .flatten()
        .any(|sort| names_field(&sort.field, field))
    {
        places.push(format!("sorting by {field}"));
    }
    let grouped_by = query
        .view_config
        .as_ref()
        .and_then(|config| config.get("kanban"))
        .and_then(|kanban| kanban.get("groupBy"))
        .and_then(Value::as_str);
    if grouped_by.is_some_and(|group| group == field) {
        places.push(format!("board grouped by {field}"));
    }
    places
}

/// Whether a filter on the query's own type names `field`. A related filter's
/// nested condition is on the node its path reaches, so it is not followed.
fn filter_names(filter: &QueryFilter, field: &str) -> bool {
    if filter.filter.is_some() {
        return false;
    }
    filter
        .property
        .as_deref()
        .is_some_and(|property| names_field(property, field))
}

/// Where a rule that triggers on the type names `field`.
fn rule_places(rule: &serde_json::Map<String, Value>, name: &str, field: &str) -> Vec<String> {
    let mut places = Vec::new();
    let rule_label = format!("rule \"{name}\"");

    if let Some(trigger) = rule.get("trigger") {
        let key_names = trigger
            .get("property_key")
            .and_then(Value::as_str)
            .is_some_and(|key| key.rsplit('.').next() == Some(field));
        let filter_names = trigger
            .get("select")
            .and_then(|select| select.get("filters"))
            .is_some_and(|filters| json_names(filters, field));
        if key_names || filter_names {
            places.push(format!("{rule_label}, trigger"));
        }
    }
    if let Some(Value::Array(conditions)) = rule.get("conditions") {
        for (index, condition) in conditions.iter().enumerate() {
            let named = condition
                .get("expr")
                .and_then(Value::as_str)
                .is_some_and(|expr| mentions(expr, field));
            if named {
                places.push(format!("{rule_label}, condition {}", index + 1));
            }
        }
    }
    if let Some(Value::Array(actions)) = rule.get("actions") {
        for (index, action) in actions.iter().enumerate() {
            let params = action.get("params").unwrap_or(&Value::Null);
            if json_names(params, field) {
                places.push(format!("{rule_label}, action {}", index + 1));
            }
        }
    }
    places
}

/// Whether any key or string in `value` is `field`, or a string mentions it as
/// a whole word (`{trigger.node.status}`, `node.status == 'done'`).
fn json_names(value: &Value, field: &str) -> bool {
    match value {
        Value::String(text) => mentions(text, field),
        Value::Array(items) => items.iter().any(|item| json_names(item, field)),
        Value::Object(map) => map
            .iter()
            .any(|(key, inner)| key == field || json_names(inner, field)),
        _ => false,
    }
}

/// Whether `text` contains `word` with no identifier character on either side.
fn mentions(text: &str, word: &str) -> bool {
    let is_identifier = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + word.len()..].chars().next();
        !before.is_some_and(is_identifier) && !after.is_some_and(is_identifier)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SqliteStore;
    use crate::models::Node;
    use crate::schema::handle_create_schema;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn test_service() -> (Arc<NodeService>, TempDir) {
        let dir = TempDir::new().unwrap();
        let mut store = Arc::new(SqliteStore::new(dir.path().join("test.db")).await.unwrap());
        (Arc::new(NodeService::new(&mut store).await.unwrap()), dir)
    }

    fn places(found: &FieldReferences, kind: ReferenceKind, field: &str) -> Vec<String> {
        let mut places: Vec<String> = found
            .references
            .iter()
            .filter(|r| r.kind == kind && r.field == field)
            .map(|r| format!("{}: {}", r.title, r.place))
            .collect();
        places.sort();
        places
    }

    /// A saved query, a play and a title template that name a field are found
    /// by it, and one that names another field is not.
    #[tokio::test]
    async fn queries_plays_and_title_templates_that_name_a_field_are_found() {
        let (svc, _dir) = test_service().await;
        handle_create_schema(
            &svc,
            json!({
                "name": "Ticket",
                "fields": [
                    { "name": "summary", "type": "text" },
                    { "name": "state", "type": "enum", "coreValues": [
                        { "value": "open", "label": "Open" },
                        { "value": "closed", "label": "Closed" }
                    ] },
                    { "name": "owner_note", "type": "text" }
                ],
                "title_template": "{summary} ({state})"
            }),
        )
        .await
        .expect("create the type");
        handle_create_schema(
            &svc,
            json!({ "name": "Urgent ticket", "extends": "ticket", "fields": [] }),
        )
        .await
        .expect("create the subtype");

        // Over the subtype: it reads `state` too.
        svc.create_node(Node::new(
            "query".to_string(),
            "Urgent and open".to_string(),
            json!({
                "target_type": "urgent-ticket",
                "filters": [{ "type": "property", "operator": "equals", "property": "state", "value": "open" }],
                "sorting": [{ "field": "summary", "direction": "asc" }],
                "view_config": { "kanban": { "groupBy": "state" } }
            }),
        ))
        .await
        .unwrap();
        // Over another type: not searched.
        svc.create_node(Node::new(
            "query".to_string(),
            "Open tasks".to_string(),
            json!({
                "target_type": "task",
                "filters": [{ "type": "property", "operator": "equals", "property": "state", "value": "open" }]
            }),
        ))
        .await
        .unwrap();
        svc.create_node(Node::new(
            "play".to_string(),
            "Tidy closed tickets".to_string(),
            json!({ "rules": [{
                "name": "archive closed",
                "description": "Archives a ticket once it is closed.",
                "trigger": {
                    "type": "graph_event", "on": "property_changed",
                    "select": { "target_type": "ticket" },
                    "property_key": "ticket.state"
                },
                "conditions": [{ "expr": "node.state == 'closed'", "description": "The ticket is closed" }],
                "actions": [{
                    "action_type": "update_node",
                    "description": "Note it",
                    "params": { "node_id": "{trigger.node.id}", "properties": { "summary": "done" } }
                }]
            }] }),
        ))
        .await
        .unwrap();

        let found = find_field_references(&svc, "ticket").await.unwrap();

        assert_eq!(
            places(&found, ReferenceKind::SavedQuery, "state"),
            [
                "Urgent and open: board grouped by state",
                "Urgent and open: filter on state"
            ]
        );
        assert_eq!(
            places(&found, ReferenceKind::SavedQuery, "summary"),
            ["Urgent and open: sorting by summary"]
        );
        assert_eq!(
            places(&found, ReferenceKind::Play, "state"),
            [
                "Tidy closed tickets: rule \"archive closed\", condition 1",
                "Tidy closed tickets: rule \"archive closed\", trigger"
            ]
        );
        assert_eq!(
            places(&found, ReferenceKind::Play, "summary"),
            ["Tidy closed tickets: rule \"archive closed\", action 1"]
        );
        let templates = places(&found, ReferenceKind::TitleTemplate, "state");
        assert_eq!(templates.len(), 1, "{templates:?}");
        assert!(
            templates[0].contains("{summary} ({state})"),
            "{templates:?}"
        );
        assert!(
            found.references.iter().all(|r| r.field != "owner_note"),
            "{found:?}"
        );
        assert_eq!(found.searched.len(), 3);
    }

    #[test]
    fn a_word_is_mentioned_only_whole() {
        assert!(mentions("node.status == 'done'", "status"));
        assert!(mentions("{trigger.node.status}", "status"));
        assert!(!mentions("node.status_text == 'x'", "status"));
        assert!(!mentions("node.substatus", "status"));
        assert!(mentions("status", "status"));
    }

    #[test]
    fn a_path_into_a_field_names_the_field() {
        assert!(names_field("repository.url", "repository"));
        assert!(names_field("status", "status"));
        assert!(!names_field("status_text", "status"));
    }
}
