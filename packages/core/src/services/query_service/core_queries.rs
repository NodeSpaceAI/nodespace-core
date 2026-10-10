//! Saved queries that ship with the product (ADR-092 §8, ADR-094).
//!
//! Three list specs, plans and decisions by status, and three are queues of
//! work: "Ready tasks", "In progress" and "Awaiting review". A queue is not a
//! type: it is an ordinary saved query, and the procedure for working it is a
//! skill attached to the query node (ADR-094 §1 and §3).
//!
//! Core queries are one of the seed tables (ADR-086 §10): each is a
//! [`NodeTemplate`] reconciled by its fixed id on every open, like a core
//! Play. A query has no body, so its one aspect is its config, and a query
//! the user edited is kept, with a shipped change recorded for them to decide
//! on (ADR-072, ADR-094 §8).

use crate::markdown::{prepare_nodes_from_template, NodeTemplate, SeedTier};
use crate::models::CoreNodeType;
use crate::services::error::NodeServiceError;
use crate::services::NodeService;
use serde_json::{json, Value};

// A seeded node's identity is its id (ADR-086 §10), so each is a fixed
// literal UUID.
/// Node id of the "Specs by status" saved query.
pub const SPECS_BY_STATUS_QUERY_ID: &str = "3b9c304f-f480-4f1d-8653-5876bb005f7c";
/// Node id of the "Plans by status" saved query.
pub const PLANS_BY_STATUS_QUERY_ID: &str = "ae79002e-913c-448d-8cc1-95912f2ab2f8";
/// Node id of the "Decisions by status" saved query.
pub const DECISIONS_BY_STATUS_QUERY_ID: &str = "d0200c59-7df9-4bc0-bc78-b06d205b085c";
/// Node id of the "Ready tasks" queue.
pub const READY_TASKS_QUERY_ID: &str = "138b68c6-9a67-4fed-8fa5-993953158f59";
/// Node id of the "In progress" queue.
pub const IN_PROGRESS_QUERY_ID: &str = "d1e5c9a8-d19c-401b-9277-40fd873fb205";
/// Node id of the "Awaiting review" queue.
pub const AWAITING_REVIEW_QUERY_ID: &str = "3eff80e8-9672-4896-ac0b-76a169062511";

/// The id of every saved query that ships with the product: the core query
/// table, in the order the queries are seeded.
pub const CORE_QUERY_IDS: &[&str] = &[
    SPECS_BY_STATUS_QUERY_ID,
    PLANS_BY_STATUS_QUERY_ID,
    DECISIONS_BY_STATUS_QUERY_ID,
    READY_TASKS_QUERY_ID,
    IN_PROGRESS_QUERY_ID,
    AWAITING_REVIEW_QUERY_ID,
];

/// A saved query as shipped: `fields` over a definition with nothing set.
///
/// Every field a user can change is stated, the unset ones as `null`: a reset
/// or a taken update replaces what the template names, so a field left out
/// would keep whatever the user had put there.
fn seeded_query(id: &str, title: &str, fields: Value) -> NodeTemplate {
    let mut root_properties = json!({
        "target_type": null,
        "filters": [],
        "sorting": null,
        "limit": null,
        "generated_by": "user",
        "view_config": null,
    });
    if let (Some(stated), Some(fields)) = (root_properties.as_object_mut(), fields.as_object()) {
        stated.extend(fields.clone());
    }
    NodeTemplate {
        id: id.to_string(),
        title: title.to_string(),
        markdown_content: String::new(),
        root_node_type: CoreNodeType::Query.as_str().to_string(),
        root_properties,
        child_node_type: None,
        tier: SeedTier::System,
    }
}

/// Every node of `target_type`, shown as a board with a column per value of
/// `status_field`.
fn by_status(id: &str, title: &str, target_type: CoreNodeType, status_field: &str) -> NodeTemplate {
    seeded_query(
        id,
        title,
        json!({
            "target_type": target_type.as_str(),
            "view_config": {
                "lastView": "kanban",
                "kanban": { "groupBy": status_field },
            },
        }),
    )
}

/// The tasks whose `status` is `status`, most urgent first, then oldest first.
fn tasks_in_status(id: &str, title: &str, status: &str) -> NodeTemplate {
    seeded_query(
        id,
        title,
        json!({
            "target_type": CoreNodeType::Task.as_str(),
            "filters": [{
                "type": "property", "operator": "equals", "property": "status", "value": status
            }],
            "sorting": by_priority_then_age(),
        }),
    )
}

/// Most urgent first, then oldest first. `priority` ascending is urgency
/// order, not alphabetical, and a task with no priority sorts ahead of the
/// scale (`Priority::ABSENT_RANK`).
fn by_priority_then_age() -> Value {
    json!([
        { "field": "priority", "direction": "asc" },
        { "field": "created_at", "direction": "asc" }
    ])
}

/// The filters of "Ready tasks" (ADR-092 §7): an open task with at least one
/// checkbox child, no unfinished `blocked_by` task and no plan that is not
/// approved.
///
/// A task the Plays would refuse to start is not ready: the last filter asks
/// whether the change to `in_progress` would be allowed (ADR-094 §9), so a
/// task with no approved spec that is not marked `requires_spec: false`
/// (ADR-097 §6) is left out however its checklist looks. The conditions
/// before it are statements, not a second rule: a checklist is not a Play's
/// concern, and the blocker and plan conditions are kept as cheap
/// prefilters that bound the dry runs, one per candidate. Switching off a
/// gate Play therefore loosens the dry run and not these. Each of the three
/// relationship conditions is a negation or an existence over the nodes a
/// path reaches:
///
/// - A checkbox is the only type that derives `checked`, and on any other
///   child it reads as absent, so a child whose `checked` exists is a
///   checkbox, ticked or not. Only direct children count.
/// - "No unfinished blocker" keeps a task with no blocker at all.
/// - "No plan that is not approved" keeps a task with no plan.
pub fn ready_tasks_filters() -> Value {
    json!([
        { "type": "property", "operator": "equals", "property": "status", "value": "open" },
        {
            "type": "related", "operator": "exists", "path": ["has_child"],
            "filter": { "type": "property", "operator": "exists", "property": "checked" }
        },
        {
            "type": "related", "operator": "exists", "path": ["blocked_by"], "negate": true,
            "filter": {
                "type": "property", "operator": "in", "property": "status",
                "value": ["done", "cancelled"], "negate": true
            }
        },
        {
            "type": "related", "operator": "exists", "path": ["plan"], "negate": true,
            "filter": {
                "type": "property", "operator": "equals", "property": "plan_status",
                "value": "approved", "negate": true
            }
        },
        { "type": "permitted", "operator": "equals", "property": "status", "value": "in_progress" }
    ])
}

/// Every saved query that ships with the product: the core query seed table.
pub fn core_query_templates() -> Vec<NodeTemplate> {
    vec![
        by_status(
            SPECS_BY_STATUS_QUERY_ID,
            "Specs by status",
            CoreNodeType::Spec,
            "spec_status",
        ),
        by_status(
            PLANS_BY_STATUS_QUERY_ID,
            "Plans by status",
            CoreNodeType::Plan,
            "plan_status",
        ),
        by_status(
            DECISIONS_BY_STATUS_QUERY_ID,
            "Decisions by status",
            CoreNodeType::Decision,
            "decision_status",
        ),
        seeded_query(
            READY_TASKS_QUERY_ID,
            "Ready tasks",
            json!({
                "target_type": CoreNodeType::Task.as_str(),
                "filters": ready_tasks_filters(),
                "sorting": by_priority_then_age(),
            }),
        ),
        tasks_in_status(IN_PROGRESS_QUERY_ID, "In progress", "in_progress"),
        tasks_in_status(AWAITING_REVIEW_QUERY_ID, "Awaiting review", "in_review"),
    ]
}

/// Reconcile the core saved queries with their seed table.
///
/// Runs on every open, through the reconciliation every seeded kind uses
/// ([`NodeService::seed_nodes_from_templates`]): a missing query is created
/// under its fixed id, a shipped change replaces a query nobody edited, and a
/// query the user edited is never overwritten.
pub async fn seed_core_queries(service: &NodeService) -> Result<(), NodeServiceError> {
    let mut groups = Vec::new();
    for template in core_query_templates() {
        let nodes = prepare_nodes_from_template(&template).map_err(|e| {
            NodeServiceError::invalid_update(format!(
                "core query '{}' does not expand: {e}",
                template.title
            ))
        })?;
        groups.push(nodes);
    }
    service.seed_nodes_from_templates(groups).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_types::QueryFields;
    use std::collections::HashSet;

    #[test]
    fn every_seeded_query_decodes_and_targets_one_type() {
        for template in core_query_templates() {
            let fields = QueryFields::from_properties(&template.root_properties)
                .unwrap_or_else(|e| panic!("'{}' must decode as a query: {e}", template.title));
            // A query over every type is not listed where saved views are.
            assert_ne!(fields.target_type, "*", "{}", template.title);
            assert!(template.markdown_content.is_empty(), "{}", template.title);
        }
    }

    #[test]
    fn the_id_table_matches_the_seeded_queries() {
        let seeded: Vec<String> = core_query_templates().into_iter().map(|t| t.id).collect();
        assert_eq!(seeded, CORE_QUERY_IDS);
        let distinct: HashSet<&&str> = CORE_QUERY_IDS.iter().collect();
        assert_eq!(distinct.len(), CORE_QUERY_IDS.len());
        for id in CORE_QUERY_IDS {
            assert!(uuid::Uuid::parse_str(id).is_ok(), "{id} is not a UUID");
        }
    }

    #[test]
    fn seeded_titles_are_distinct() {
        // A saved query is run by its title, and a title two share is refused.
        let titles: HashSet<String> = core_query_templates()
            .into_iter()
            .map(|t| t.title.to_lowercase())
            .collect();
        assert_eq!(titles.len(), CORE_QUERY_IDS.len());
    }
}
