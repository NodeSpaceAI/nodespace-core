//! A saved query in plain language (ADR-100 §6).
//!
//! A query stores no description of itself: its meaning is wholly in its
//! target type, filters, sorting and limit, so the sentence is rendered from
//! them, with no model call. [`describe_query`] is the only renderer, and what
//! a bound chat opens on.
//!
//! The wording stays on the stored names: a field is called what it is called
//! in the schema, so a person reading the description and the agent reading the
//! definition are talking about the same things.

use super::{
    FilterAnchor, FilterOperator, FilterType, QueryFilter, RelationshipHop, RelativeDate,
    SortConfig, SortDirection,
};
use nodespace_types::{QueryFields, ALL_TYPES_TARGET};
use serde_json::Value;

/// What the query lists, in a sentence or two: the target type, each filter,
/// the sorting and the limit, then how the view is shown when that is stated.
///
/// A query with no filters lists every node of its type. Filters are ANDed,
/// as the query service ANDs them.
pub fn describe_query(fields: &QueryFields) -> String {
    let mut sentence = format!("Lists {}", plural_of(&fields.target_type));

    let clauses: Vec<String> = fields.filters.iter().map(filter_clause).collect();
    match clauses.as_slice() {
        [] => {}
        [only] => {
            sentence.push(' ');
            sentence.push_str(only);
        }
        [rest @ .., last] => {
            sentence.push(' ');
            sentence.push_str(&rest.join(", "));
            sentence.push_str(", and ");
            sentence.push_str(last);
        }
    }
    sentence.push('.');

    let sorting: Vec<String> = fields.sorting.iter().flatten().map(sort_phrase).collect();
    if !sorting.is_empty() {
        sentence.push_str(&format!(" Sorted by {}.", sorting.join(", then ")));
    }
    if let Some(limit) = fields.limit {
        sentence.push_str(&format!(" Shows at most {limit}."));
    }
    if let Some(view) = view_phrase(fields.view_config.as_ref()) {
        sentence.push_str(&format!(" {view}"));
    }
    sentence
}

/// The nodes of a type, in the plural: `tasks`, `decisions`, `every node` for a
/// query over every type. A type id's separators read as spaces.
fn plural_of(target_type: &str) -> String {
    if target_type == ALL_TYPES_TARGET {
        return "every node".to_string();
    }
    let name = target_type.replace(['-', '_'], " ");
    let ends = |suffixes: &[&str]| suffixes.iter().any(|s| name.ends_with(s));
    let consonant_y = name.ends_with('y')
        && name
            .chars()
            .rev()
            .nth(1)
            .is_some_and(|c| !"aeiou".contains(c));
    if consonant_y {
        format!("{}ies", &name[..name.len() - 1])
    } else if ends(&["s", "x", "ch", "sh"]) {
        format!("{name}es")
    } else {
        format!("{name}s")
    }
}

/// One filter as a clause that follows the noun: `whose status is open`,
/// `that have a node through child where checked is set`.
fn filter_clause(filter: &QueryFilter) -> String {
    match filter.filter_type {
        FilterType::Related => format!("that {}", related_phrase(filter, "have")),
        FilterType::Relationship => format!("that are {}", relationship_condition(filter)),
        FilterType::Permitted => format!("for which {}", permitted_phrase(filter)),
        FilterType::Property | FilterType::Content | FilterType::Metadata => {
            format!("whose {}", condition(filter))
        }
    }
}

/// A filter as a bare condition on the node it is evaluated against, with no
/// lead-in: `status is open`, `checked is set`, `title contains "report"`.
fn condition(filter: &QueryFilter) -> String {
    match filter.filter_type {
        FilterType::Related => format!("it {}", related_phrase(filter, "has")),
        FilterType::Relationship => format!("it is {}", relationship_condition(filter)),
        FilterType::Permitted => permitted_phrase(filter),
        FilterType::Property | FilterType::Content | FilterType::Metadata => comparison(filter),
    }
}

/// What a related filter requires of the nodes its path reaches: `have a node
/// through child where checked is set`. `has` is the verb that agrees with
/// the subject the caller puts before it.
fn related_phrase(filter: &QueryFilter, has: &str) -> String {
    let path = path_phrase(filter);
    let nested = filter
        .filter
        .as_deref()
        .map(condition)
        .unwrap_or_else(|| "it exists".to_string());
    let article = if filter.is_negated() { "no" } else { "a" };
    format!("{has} {article} node through {path} where {nested}")
}

/// What a permitted filter asks of a rule: `setting status to in_progress
/// would not be refused by a rule`.
fn permitted_phrase(filter: &QueryFilter) -> String {
    let property = filter.property.as_deref().unwrap_or("a field");
    let value = filter
        .value
        .as_ref()
        .map(value_phrase)
        .unwrap_or_else(|| "a value".to_string());
    let verdict = if filter.is_negated() {
        "would be refused"
    } else {
        "would not be refused"
    };
    format!("setting {property} to {value} {verdict} by a rule")
}

/// `status is open`, `due_date is before today`, `title contains "x"`.
fn comparison(filter: &QueryFilter) -> String {
    let subject = match (&filter.filter_type, filter.property.as_deref()) {
        (FilterType::Content, _) => "content",
        (_, Some(property)) => property,
        (_, None) => "content",
    };
    let negated = filter.is_negated();
    let object = filter
        .relative_date
        .as_ref()
        .map(relative_phrase)
        .or_else(|| filter.value.as_ref().map(value_phrase));
    let dated = filter.relative_date.is_some()
        || filter
            .value
            .as_ref()
            .and_then(Value::as_str)
            .is_some_and(looks_like_date);

    let predicate = match (&filter.operator, object) {
        (FilterOperator::Exists, _) => {
            if negated {
                "is not set".to_string()
            } else {
                "is set".to_string()
            }
        }
        (FilterOperator::Equals, Some(object)) => {
            format!("{} {object}", if negated { "is not" } else { "is" })
        }
        (FilterOperator::Contains, Some(object)) => {
            let case = if filter.case_sensitive == Some(true) {
                " (matching case)"
            } else {
                ""
            };
            format!(
                "{} {object}{case}",
                if negated {
                    "does not contain"
                } else {
                    "contains"
                }
            )
        }
        (FilterOperator::In, Some(object)) => {
            format!(
                "{} {object}",
                if negated { "is none of" } else { "is one of" }
            )
        }
        (
            order @ (FilterOperator::GreaterThan
            | FilterOperator::GreaterThanOrEqual
            | FilterOperator::LessThan
            | FilterOperator::LessThanOrEqual),
            Some(object),
        ) => {
            // Negated, an ordering reads as "is not ...", never as the
            // opposite comparison: a node with no value for the field meets
            // neither, and a negated filter keeps it, so "is not on or before
            // today" is what it lists and "is after today" is not.
            let words = match (order, dated) {
                (FilterOperator::GreaterThan, true) => "after",
                (FilterOperator::GreaterThanOrEqual, true) => "on or after",
                (FilterOperator::LessThan, true) => "before",
                (FilterOperator::LessThanOrEqual, true) => "on or before",
                (FilterOperator::GreaterThan, false) => "greater than",
                (FilterOperator::GreaterThanOrEqual, false) => "at least",
                (FilterOperator::LessThan, false) => "less than",
                _ => "at most",
            };
            format!("{} {words} {object}", if negated { "is not" } else { "is" })
        }
        // A comparison with no value to compare against.
        (_, None) => {
            if negated {
                "has no value to compare".to_string()
            } else {
                "has a value to compare".to_string()
            }
        }
    };
    format!("{subject} {predicate}")
}

/// What a relationship filter requires of the path, after the verb:
/// `connected through assignee to the current person`.
fn relationship_condition(filter: &QueryFilter) -> String {
    let path = path_phrase(filter);
    let target = match (&filter.anchor, filter.node_id.as_deref()) {
        (Some(FilterAnchor::CurrentPerson), _) => "the current person".to_string(),
        (None, Some(id)) => format!("node {id}"),
        (None, None) => "any node".to_string(),
    };
    let reaching = format!("{path} to {target}");
    if filter.is_negated() {
        format!("not connected through {reaching}")
    } else {
        format!("connected through {reaching}")
    }
}

/// The relationship names a filter's path walks, in words: the four built-in
/// names read as what they reach, any other as it is stored.
fn path_phrase(filter: &QueryFilter) -> String {
    let Some(path) = &filter.path else {
        return "relationship".to_string();
    };
    let hops: Vec<String> = path.hops().iter().map(hop_phrase).collect();
    if hops.is_empty() {
        "relationship".to_string()
    } else {
        hops.join(" then ")
    }
}

fn hop_phrase(hop: &RelationshipHop) -> String {
    let name = match hop.name.as_str() {
        "has_child" => "child",
        "child_of" => "parent",
        "mentions" => "mention",
        "mentioned_by" => "backlink",
        other => other,
    };
    if hop.open_ended {
        format!("{name} (at any depth)")
    } else {
        name.to_string()
    }
}

/// A date named relative to the day the query runs.
fn relative_phrase(date: &RelativeDate) -> String {
    match date.offset_days.unwrap_or(0) {
        0 => "today".to_string(),
        1 => "1 day after today".to_string(),
        -1 => "1 day before today".to_string(),
        days if days > 0 => format!("{days} days after today"),
        days => format!("{} days before today", days.unsigned_abs()),
    }
}

/// A stored value as a reader would say it: a string as written (quoted when
/// it has spaces), a list as `a, b`.
fn value_phrase(value: &Value) -> String {
    match value {
        Value::String(text) if text.contains(char::is_whitespace) || text.is_empty() => {
            format!("\"{text}\"")
        }
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(value_phrase)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Null => "nothing".to_string(),
        other => other.to_string(),
    }
}

/// `YYYY-MM-DD`, the form a `date` field is stored in.
fn looks_like_date(text: &str) -> bool {
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_ok()
}

fn sort_phrase(sort: &SortConfig) -> String {
    match sort.direction {
        SortDirection::Ascending => sort.field.clone(),
        SortDirection::Descending => format!("{} (descending)", sort.field),
    }
}

/// How the view is shown, from the keys the viewer writes: `lastView` and the
/// board's `kanban.groupBy`. `None` when the query states neither.
fn view_phrase(view_config: Option<&Value>) -> Option<String> {
    let config = view_config?;
    match config.get("lastView").and_then(Value::as_str)? {
        "kanban" => Some(
            match config.pointer("/kanban/groupBy").and_then(Value::as_str) {
                Some(field) => format!("Shown as a board with a column per {field}."),
                None => "Shown as a board.".to_string(),
            },
        ),
        "table" => Some("Shown as a table.".to_string()),
        "list" => Some("Shown as a list.".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::query_service::core_queries::{
        core_query_templates, AWAITING_REVIEW_QUERY_ID, CHANGES_REQUESTED_QUERY_ID,
        DECISIONS_BY_STATUS_QUERY_ID, IN_PROGRESS_QUERY_ID, NEEDS_SHAPING_QUERY_ID,
        PLANS_BY_STATUS_QUERY_ID, READY_TASKS_QUERY_ID, READY_TO_MERGE_QUERY_ID,
        SPECS_BY_STATUS_QUERY_ID, WAITING_ON_YOU_QUERY_ID,
    };
    use serde_json::json;

    fn described(properties: Value) -> String {
        describe_query(&QueryFields::from_properties(&properties).expect("a decodable query"))
    }

    fn filters(filters: Value) -> String {
        described(json!({ "target_type": "task", "filters": filters }))
    }

    #[test]
    fn a_query_with_no_filters_lists_every_node_of_its_type() {
        assert_eq!(described(json!({ "target_type": "task" })), "Lists tasks.");
        assert_eq!(described(json!({})), "Lists every node.");
        assert_eq!(
            described(json!({ "target_type": "software-project" })),
            "Lists software projects."
        );
        assert_eq!(
            described(json!({ "target_type": "category" })),
            "Lists categories."
        );
    }

    #[test]
    fn a_property_filter_reads_with_each_operator() {
        let cases = [
            (
                json!({ "type": "property", "operator": "equals", "property": "status", "value": "open" }),
                "whose status is open",
            ),
            (
                json!({ "type": "property", "operator": "equals", "property": "status", "value": "open", "negate": true }),
                "whose status is not open",
            ),
            (
                json!({ "type": "property", "operator": "in", "property": "status", "value": ["done", "cancelled"] }),
                "whose status is one of done, cancelled",
            ),
            (
                json!({ "type": "property", "operator": "in", "property": "status", "value": ["done", "cancelled"], "negate": true }),
                "whose status is none of done, cancelled",
            ),
            (
                json!({ "type": "property", "operator": "exists", "property": "due_date" }),
                "whose due_date is set",
            ),
            (
                json!({ "type": "property", "operator": "exists", "property": "due_date", "negate": true }),
                "whose due_date is not set",
            ),
            (
                json!({ "type": "property", "operator": "contains", "property": "title", "value": "launch plan" }),
                "whose title contains \"launch plan\"",
            ),
            (
                json!({ "type": "property", "operator": "contains", "property": "title", "value": "x", "case_sensitive": true, "negate": true }),
                "whose title does not contain x (matching case)",
            ),
            (
                json!({ "type": "property", "operator": "gte", "property": "estimate", "value": 3 }),
                "whose estimate is at least 3",
            ),
            (
                json!({ "type": "property", "operator": "gt", "property": "estimate", "value": 3, "negate": true }),
                "whose estimate is not greater than 3",
            ),
            (
                json!({ "type": "property", "operator": "gte", "property": "estimate", "value": 3, "negate": true }),
                "whose estimate is not at least 3",
            ),
            (
                json!({ "type": "property", "operator": "lt", "property": "estimate", "value": 3, "negate": true }),
                "whose estimate is not less than 3",
            ),
            (
                json!({ "type": "property", "operator": "lte", "property": "estimate", "value": 3, "negate": true }),
                "whose estimate is not at most 3",
            ),
        ];
        for (filter, clause) in cases {
            assert_eq!(filters(json!([filter])), format!("Lists tasks {clause}."));
        }
    }

    #[test]
    fn a_content_and_a_metadata_filter_read_as_conditions_on_the_node() {
        assert_eq!(
            filters(json!([
                { "type": "content", "operator": "contains", "value": "invoice" }
            ])),
            "Lists tasks whose content contains invoice."
        );
        assert_eq!(
            filters(json!([
                { "type": "metadata", "operator": "gt", "property": "created_at", "value": "2026-01-01" }
            ])),
            "Lists tasks whose created_at is after 2026-01-01."
        );
    }

    #[test]
    fn a_relative_date_reads_against_today() {
        let on = |operator: &str, offset: Option<i32>| {
            let mut date = json!({ "anchor": "today" });
            if let Some(offset) = offset {
                date["offset_days"] = json!(offset);
            }
            filters(json!([{
                "type": "property", "operator": operator, "property": "due_date",
                "relative_date": date
            }]))
        };
        assert_eq!(
            on("lte", None),
            "Lists tasks whose due_date is on or before today."
        );
        assert_eq!(
            on("gte", Some(-7)),
            "Lists tasks whose due_date is on or after 7 days before today."
        );
        assert_eq!(
            on("lt", Some(1)),
            "Lists tasks whose due_date is before 1 day after today."
        );
        assert_eq!(
            on("equals", Some(14)),
            "Lists tasks whose due_date is 14 days after today."
        );
    }

    /// A negated ordering keeps the nodes with no value for the field, as
    /// every negated filter does, so it reads as "is not ..." rather than as
    /// the opposite comparison, which a node with no value does not meet.
    #[test]
    fn a_negated_ordering_reads_as_not_rather_than_the_opposite_comparison() {
        let not_on = |operator: &str| {
            filters(json!([{
                "type": "property", "operator": operator, "property": "due_date",
                "relative_date": { "anchor": "today" }, "negate": true
            }]))
        };
        assert_eq!(
            not_on("lte"),
            "Lists tasks whose due_date is not on or before today."
        );
        assert_eq!(
            not_on("lt"),
            "Lists tasks whose due_date is not before today."
        );
        assert_eq!(
            not_on("gte"),
            "Lists tasks whose due_date is not on or after today."
        );
        assert_eq!(
            not_on("gt"),
            "Lists tasks whose due_date is not after today."
        );
        assert_eq!(
            filters(json!([
                { "type": "metadata", "operator": "gt", "property": "created_at", "value": "2026-01-01", "negate": true }
            ])),
            "Lists tasks whose created_at is not after 2026-01-01."
        );
    }

    #[test]
    fn a_relationship_filter_names_the_path_and_who_it_must_reach() {
        assert_eq!(
            filters(json!([{
                "type": "relationship", "operator": "equals",
                "path": ["assignee"], "anchor": "current_person"
            }])),
            "Lists tasks that are connected through assignee to the current person."
        );
        assert_eq!(
            filters(json!([{
                "type": "relationship", "operator": "exists",
                "path": ["child_of"], "node_id": "n-1"
            }])),
            "Lists tasks that are connected through parent to node n-1."
        );
        assert_eq!(
            filters(json!([{
                "type": "relationship", "operator": "exists", "negate": true,
                "path": [{ "name": "child_of", "open_ended": true }, "project"]
            }])),
            "Lists tasks that are not connected through parent (at any depth) then project to any node."
        );
    }

    #[test]
    fn a_related_filter_reads_through_the_nodes_it_reaches() {
        assert_eq!(
            filters(json!([{
                "type": "related", "operator": "exists", "path": ["has_child"],
                "filter": { "type": "property", "operator": "exists", "property": "checked" }
            }])),
            "Lists tasks that have a node through child where checked is set."
        );
        assert_eq!(
            filters(json!([{
                "type": "related", "operator": "exists", "path": ["blocked_by"], "negate": true,
                "filter": {
                    "type": "property", "operator": "in", "property": "status",
                    "value": ["done", "cancelled"], "negate": true
                }
            }])),
            "Lists tasks that have no node through blocked_by where status is none of done, cancelled."
        );
        // A related filter whose nested filter is itself one that reaches.
        assert_eq!(
            filters(json!([{
                "type": "related", "operator": "exists", "path": ["blocks"],
                "filter": {
                    "type": "relationship", "operator": "equals",
                    "path": ["assignee"], "anchor": "current_person"
                }
            }])),
            "Lists tasks that have a node through blocks where it is connected through assignee to the current person."
        );
    }

    #[test]
    fn a_permitted_filter_names_the_change_a_rule_would_refuse() {
        assert_eq!(
            filters(json!([{
                "type": "permitted", "operator": "equals",
                "property": "status", "value": "in_progress"
            }])),
            "Lists tasks for which setting status to in_progress would not be refused by a rule."
        );
    }

    #[test]
    fn several_filters_are_listed_and_joined() {
        let two = filters(json!([
            { "type": "property", "operator": "equals", "property": "status", "value": "open" },
            { "type": "property", "operator": "exists", "property": "due_date" }
        ]));
        assert_eq!(
            two,
            "Lists tasks whose status is open, and whose due_date is set."
        );
        let three = filters(json!([
            { "type": "property", "operator": "equals", "property": "status", "value": "open" },
            { "type": "property", "operator": "exists", "property": "due_date" },
            { "type": "content", "operator": "contains", "value": "x" }
        ]));
        assert_eq!(
            three,
            "Lists tasks whose status is open, whose due_date is set, and whose content contains x."
        );
    }

    #[test]
    fn sorting_limit_and_view_follow_the_filters() {
        assert_eq!(
            described(json!({
                "target_type": "task",
                "filters": [{ "type": "property", "operator": "equals", "property": "status", "value": "open" }],
                "sorting": [
                    { "field": "priority", "direction": "asc" },
                    { "field": "created_at", "direction": "desc" }
                ],
                "limit": 20,
                "view_config": { "lastView": "kanban", "kanban": { "groupBy": "status" } }
            })),
            "Lists tasks whose status is open. Sorted by priority, then created_at (descending). \
             Shows at most 20. Shown as a board with a column per status."
        );
        assert_eq!(
            described(json!({ "target_type": "task", "view_config": { "lastView": "table" } })),
            "Lists tasks. Shown as a table."
        );
    }

    /// Each seeded query reads correctly: the fixtures are the shipped
    /// definitions, so a change to one is a change to a sentence here.
    #[test]
    fn the_seeded_queries_read_correctly() {
        let sentence = |id: &str| {
            let template = core_query_templates()
                .into_iter()
                .find(|t| t.id == id)
                .expect("a seeded query");
            described(template.root_properties)
        };
        let by_priority = "Sorted by priority, then created_at.";

        assert_eq!(
            sentence(SPECS_BY_STATUS_QUERY_ID),
            "Lists specs. Shown as a board with a column per spec_status."
        );
        assert_eq!(
            sentence(PLANS_BY_STATUS_QUERY_ID),
            "Lists plans. Shown as a board with a column per plan_status."
        );
        assert_eq!(
            sentence(DECISIONS_BY_STATUS_QUERY_ID),
            "Lists decisions. Shown as a board with a column per decision_status."
        );
        assert_eq!(
            sentence(IN_PROGRESS_QUERY_ID),
            format!(
                "Lists tasks whose status is in_progress, \
                 and whose status is not changes_requested. {by_priority}"
            )
        );
        assert_eq!(
            sentence(AWAITING_REVIEW_QUERY_ID),
            format!("Lists issues whose status is in_review. {by_priority}")
        );
        assert_eq!(
            sentence(CHANGES_REQUESTED_QUERY_ID),
            format!("Lists issues whose status is changes_requested. {by_priority}")
        );
        assert_eq!(
            sentence(READY_TO_MERGE_QUERY_ID),
            format!("Lists issues whose status is ready_to_merge. {by_priority}")
        );
        assert_eq!(
            sentence(READY_TASKS_QUERY_ID),
            format!(
                "Lists tasks whose status is open, \
                 that have a node through child where checked is set, \
                 that have no node through blocked_by where status is none of done, cancelled, \
                 that have no node through plan where plan_status is not approved, \
                 and for which setting status to in_progress would not be refused by a rule. \
                 {by_priority}"
            )
        );
        assert_eq!(
            sentence(NEEDS_SHAPING_QUERY_ID),
            format!(
                "Lists tasks whose status is open, \
                 that have no node through child where checked is set, \
                 and that have no node through blocks where status is set. {by_priority}"
            )
        );
        assert_eq!(
            sentence(WAITING_ON_YOU_QUERY_ID),
            format!(
                "Lists tasks whose status is open, \
                 that are connected through assignee to the current person, \
                 and that have a node through blocks where status is set. {by_priority}"
            )
        );
    }

    #[test]
    fn every_seeded_query_has_a_description() {
        for template in core_query_templates() {
            let text = described(template.root_properties);
            assert!(text.starts_with("Lists "), "{}: {text}", template.title);
            assert!(text.ends_with('.'), "{}: {text}", template.title);
        }
    }
}
