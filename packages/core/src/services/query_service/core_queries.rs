//! Saved queries that ship with the product (ADR-092 §8, ADR-094).
//!
//! Three list specs, plans and decisions by status, and three are queues of
//! work: "Awaiting review", "Changes requested" and "Ready to merge". They
//! follow an issue through review and are written against `issue`, which is
//! the only scope that sees its own statuses (ADR-098 §6). A queue is not a
//! type: it is an ordinary saved query, and the procedure for working it is a
//! skill attached to the query node (ADR-094 §1 and §3).
//!
//! Every seeded query is a view a person works from. A list only a procedure
//! reads ("what is ready?", "what is waiting on me?") is not seeded: the rule
//! or skill that needs it carries its filter and runs it as an ordinary
//! filtered search (ADR-094, amendment of 2026-11-03).
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
/// Node id of the "Awaiting review" queue.
pub const AWAITING_REVIEW_QUERY_ID: &str = "3eff80e8-9672-4896-ac0b-76a169062511";
/// Node id of the "Changes requested" queue.
pub const CHANGES_REQUESTED_QUERY_ID: &str = "6a1f0c52-8b47-4d3e-9a15-c2e07d4b8f13";
/// Node id of the "Ready to merge" queue.
pub const READY_TO_MERGE_QUERY_ID: &str = "e47b9d20-35a6-4c81-b0f2-91d8a65c3e74";

/// The id of every saved query that ships with the product: the core query
/// table, in the order the queries are seeded.
pub const CORE_QUERY_IDS: &[&str] = &[
    SPECS_BY_STATUS_QUERY_ID,
    PLANS_BY_STATUS_QUERY_ID,
    DECISIONS_BY_STATUS_QUERY_ID,
    AWAITING_REVIEW_QUERY_ID,
    CHANGES_REQUESTED_QUERY_ID,
    READY_TO_MERGE_QUERY_ID,
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

/// The `target_type` nodes whose `status` is `status`, most urgent first, then
/// oldest first. Written against `issue`, `status` is the value the issue
/// stores, so `ready_to_merge` is not `in_review` there; written against
/// `task`, an issue's own statuses read as the core status they map to.
fn in_status(id: &str, title: &str, target_type: CoreNodeType, status: &str) -> NodeTemplate {
    seeded_query(
        id,
        title,
        json!({
            "target_type": target_type.as_str(),
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
        in_status(
            AWAITING_REVIEW_QUERY_ID,
            "Awaiting review",
            CoreNodeType::Issue,
            "in_review",
        ),
        in_status(
            CHANGES_REQUESTED_QUERY_ID,
            "Changes requested",
            CoreNodeType::Issue,
            "changes_requested",
        ),
        in_status(
            READY_TO_MERGE_QUERY_ID,
            "Ready to merge",
            CoreNodeType::Issue,
            "ready_to_merge",
        ),
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
    fn the_seeded_set_is_the_three_boards_and_the_three_issue_queues() {
        let seeded: Vec<(String, String)> = core_query_templates()
            .into_iter()
            .map(|t| {
                let target = QueryFields::from_properties(&t.root_properties)
                    .expect("decodes")
                    .target_type;
                (t.title, target)
            })
            .collect();
        let expected: Vec<(String, String)> = [
            ("Specs by status", "spec"),
            ("Plans by status", "plan"),
            ("Decisions by status", "decision"),
            ("Awaiting review", "issue"),
            ("Changes requested", "issue"),
            ("Ready to merge", "issue"),
        ]
        .iter()
        .map(|(title, target)| (title.to_string(), target.to_string()))
        .collect();
        assert_eq!(seeded, expected);
        assert_eq!(CORE_QUERY_IDS.len(), 6);
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
