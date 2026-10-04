//! Node governance: the one participation check (ADR-087).
//!
//! `lifecycle_status` is governance state, with the same meaning for every
//! node type: `active` is in normal use, `archived` is hidden. An archived
//! node participates in nothing: search, default queries, counts and lists,
//! the `@` picker, the agent's context and tool reads, and the play engine.
//!
//! This module is the only code that reads the field. Every surface asks it,
//! through one of two forms of the same rule:
//!
//! - [`participates`] / [`is_visible`], for a decision about a node already
//!   in memory;
//! - [`participates_sql`] / [`visible_sql`], for a query, so the database
//!   applies the rule instead of a filter after the fact.
//!
//! The only opt-in is `include_archived`, for a surface that must list
//! archived nodes, such as one that unarchives them. A read that names a node
//! (by id, or the structure under an id: its children, its subtree) is not a
//! participation surface; it returns what was asked for.
//!
//! The module also owns the per-type participation rules the registry
//! records (ADR-086 §3): which types the `@` picker offers and which are left
//! out of default queries. Each follows the `extends` chain.
//!
//! `scripts/check-lifecycle-reads.ts` fails on a `lifecycle_status` read
//! anywhere else, so a new surface can't apply its own variant or forget the
//! rule.

use crate::db::schema::is_not_a_sql;
use crate::models::{CoreNodeType, Node};

pub use nodespace_types::{is_valid_lifecycle_status, LIFECYCLE_STATUSES};

/// In normal use. The default.
pub const ACTIVE: &str = "active";
/// Hidden: the node participates in nothing. Its owner can unarchive it.
pub const ARCHIVED: &str = "archived";

const COLUMN: &str = "lifecycle_status";
/// The field's key on a node's wire JSON.
const WIRE_KEY: &str = "lifecycleStatus";

/// Whether `name` is the governance field, in its stored or its wire spelling.
///
/// For the code that must refuse it by name: a play condition can't read it
/// and no play action takes it as a parameter (ADR-087 §5).
pub fn is_lifecycle_field(name: &str) -> bool {
    name == COLUMN || name == WIRE_KEY
}

/// Whether `text` names the governance field as one of its identifiers: a
/// binding path such as `trigger.node.lifecycleStatus`, or an expression
/// that reads it.
pub fn names_lifecycle_field(text: &str) -> bool {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(is_lifecycle_field)
}

/// Remove the governance field from a node's JSON, or from each node of an
/// array of them. What a play's bindings see: a play doesn't read a node's
/// lifecycle, so the value isn't there to be read (ADR-087 §5).
pub fn remove_lifecycle(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove(COLUMN);
            map.remove(WIRE_KEY);
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(remove_lifecycle),
        _ => {}
    }
}

/// Whether `node` takes part in search, lists, mentions, the agent's context
/// and automation.
pub fn participates(node: &Node) -> bool {
    node.lifecycle_status == ACTIVE
}

/// Whether a read returns `node`: it participates, or the read opted in to
/// archived nodes.
pub fn is_visible(node: &Node, include_archived: bool) -> bool {
    include_archived || participates(node)
}

/// A SQL predicate that is true for a participating row.
///
/// `alias` is the `node` table's alias in the query, or `""` when the column
/// is unqualified.
pub fn participates_sql(alias: &str) -> String {
    if alias.is_empty() {
        format!("{COLUMN} = '{ACTIVE}'")
    } else {
        format!("{alias}.{COLUMN} = '{ACTIVE}'")
    }
}

/// [`participates_sql`], or `None` when the read opted in to archived nodes
/// and so has no lifecycle condition at all.
pub fn visible_sql(alias: &str, include_archived: bool) -> Option<String> {
    (!include_archived).then(|| participates_sql(alias))
}

/// Whether writing `status` to a node archives it. For the store's write
/// paths, which hold the new value and not the node.
pub fn archives(status: &str) -> bool {
    status == ARCHIVED
}

/// Whether a write archived or unarchived a node, given its state before and
/// after. Either way its embedding root is re-queued: an unarchived node is
/// embedded again, and an archived child leaves its root's aggregate.
pub fn participation_changed(before: &Node, after: &Node) -> bool {
    participates(before) != participates(after)
}

/// A SQL predicate that is true when `type_column` holds a type the `@`
/// mention picker offers. The registry's `mentionable` rule, so a type
/// extending an unmentionable one is left out too.
pub fn mentionable_sql(type_column: &str) -> String {
    is_not_a_sql(type_column, &CoreNodeType::not_mentionable())
}

/// A SQL predicate that is true when `type_column` holds a type default
/// queries, counts and lists include, or `None` when the registry excludes no
/// type. The registry's `excluded_from_default_queries` rule, following
/// `extends`.
pub fn in_default_queries_sql(type_column: &str) -> Option<String> {
    excluded_types_sql(type_column, &CoreNodeType::excluded_from_default_queries())
}

/// A SQL predicate that is true when `type_column` holds none of `excluded`
/// and no type extending one of them, or `None` when `excluded` is empty.
///
/// For a surface that leaves out types default queries include: the agent's
/// node search never returns a conversation. The types resolve through the
/// ancestry table, so a new subtype is left out with no change to the caller.
pub fn excluded_types_sql(type_column: &str, excluded: &[CoreNodeType]) -> Option<String> {
    (!excluded.is_empty()).then(|| is_not_a_sql(type_column, excluded))
}

/// The conditions a default query, count or list puts on the `node` table:
/// the row participates, and its type isn't left out of default queries.
///
/// `alias` is the `node` table's alias, or `""`. With `include_archived` the
/// lifecycle condition is dropped; the type rule still applies.
pub fn default_query_conditions(alias: &str, include_archived: bool) -> Vec<String> {
    let type_column = if alias.is_empty() {
        "node_type".to_string()
    } else {
        format!("{alias}.node_type")
    };
    visible_sql(alias, include_archived)
        .into_iter()
        .chain(in_default_queries_sql(&type_column))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(status: &str) -> Node {
        let mut node = Node::new("text".to_string(), "probe".to_string(), json!({}));
        node.lifecycle_status = status.to_string();
        node
    }

    #[test]
    fn an_archived_node_does_not_participate() {
        assert!(participates(&node(ACTIVE)));
        assert!(!participates(&node(ARCHIVED)));
    }

    #[test]
    fn include_archived_is_the_only_way_to_see_an_archived_node() {
        assert!(is_visible(&node(ACTIVE), false));
        assert!(!is_visible(&node(ARCHIVED), false));
        assert!(is_visible(&node(ARCHIVED), true));
    }

    #[test]
    fn the_sql_fragment_states_the_same_rule_as_the_predicate() {
        assert_eq!(participates_sql(""), "lifecycle_status = 'active'");
        assert_eq!(participates_sql("n"), "n.lifecycle_status = 'active'");
        assert_eq!(visible_sql("n", true), None);
        assert_eq!(visible_sql("", false), Some(participates_sql("")));
    }

    #[test]
    fn archiving_and_unarchiving_change_participation() {
        assert!(participation_changed(&node(ACTIVE), &node(ARCHIVED)));
        assert!(participation_changed(&node(ARCHIVED), &node(ACTIVE)));
        assert!(!participation_changed(&node(ACTIVE), &node(ACTIVE)));
        assert!(!participation_changed(&node(ARCHIVED), &node(ARCHIVED)));
    }

    #[test]
    fn only_the_archived_status_archives() {
        assert!(archives(ARCHIVED));
        assert!(!archives(ACTIVE));
    }

    #[test]
    fn the_governance_field_is_recognised_by_name() {
        assert!(is_lifecycle_field("lifecycle_status"));
        assert!(is_lifecycle_field("lifecycleStatus"));
        assert!(!is_lifecycle_field("status"));
    }

    #[test]
    fn a_path_or_expression_naming_the_field_is_recognised() {
        assert!(names_lifecycle_field("trigger.node.lifecycleStatus"));
        assert!(names_lifecycle_field("item.lifecycle_status"));
        assert!(names_lifecycle_field(
            "count(trigger.node.tasks.where(t, t.lifecycle_status == 'active'))"
        ));
        assert!(!names_lifecycle_field("trigger.node.status"));
        assert!(!names_lifecycle_field(
            "trigger.node.my_lifecycle_status_note"
        ));
    }

    #[test]
    fn a_nodes_json_loses_the_field_in_both_spellings() {
        let mut one = serde_json::to_value(node(ARCHIVED)).unwrap();
        assert!(one.get("lifecycleStatus").is_some());
        remove_lifecycle(&mut one);
        assert!(one.get("lifecycleStatus").is_none());
        assert!(one.get("content").is_some());

        let mut many = json!([{ "id": "a", "lifecycle_status": "archived" }, { "id": "b" }]);
        remove_lifecycle(&mut many);
        assert_eq!(many, json!([{ "id": "a" }, { "id": "b" }]));
    }

    #[test]
    fn the_statuses_named_here_are_the_valid_set() {
        assert_eq!(LIFECYCLE_STATUSES, [ACTIVE, ARCHIVED]);
    }

    #[test]
    fn the_mentionable_rule_resolves_through_the_ancestry_table() {
        let sql = mentionable_sql("node_type");
        assert!(sql.starts_with("node_type NOT IN (SELECT node_type FROM type_ancestry"));
        for core in CoreNodeType::not_mentionable() {
            assert!(sql.contains(&format!("'{}'", core.as_str())), "{core}");
        }
    }

    #[test]
    fn a_type_rule_with_nothing_to_exclude_adds_no_condition() {
        assert_eq!(excluded_types_sql("node_type", &[]), None);
        assert_eq!(
            excluded_types_sql("n.node_type", &[CoreNodeType::AiChat]),
            Some(is_not_a_sql("n.node_type", &[CoreNodeType::AiChat]))
        );
    }

    #[test]
    fn default_query_conditions_drop_only_the_lifecycle_rule_on_opt_in() {
        let default = default_query_conditions("n", false);
        assert!(default.contains(&participates_sql("n")));
        let opted_in = default_query_conditions("n", true);
        assert!(!opted_in.contains(&participates_sql("n")));
        assert_eq!(default.len(), opted_in.len() + 1);
    }
}
