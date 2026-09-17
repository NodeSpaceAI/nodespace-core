//! Execute-query ops wrapper.
//!
//! Bridges agent tool calls to `QueryService`. The agent passes a
//! flat-property filter shape; this module maps it to `QueryDefinition`
//! and delegates to `QueryService::execute`.

use crate::models::Node;
use crate::ops::OpsError;
use crate::services::node_service::NodeService;
use crate::services::query_service::{
    FilterOperator, FilterType, QueryDefinition, QueryFilter, QueryService, SortConfig,
    SortDirection,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

// ============================================================================
// Agent-facing filter shape
// ============================================================================

/// A single filter item as passed by the agent tool.
///
/// `type` is optional on the wire. A model that has worked out the hard part —
/// which property to compare, with which operator, against which value —
/// routinely omits the category discriminator, and rejecting an otherwise
/// complete and correct filter over a token that is derivable from the other
/// fields turns a solved query into a tool error. [`AgentFilterItem::category`]
/// infers it: a filter naming a `relationship_type` or an anchor `node_id` is a
/// relationship filter, and anything naming a `property` is a property filter —
/// the only two shapes the omission is observed for. An item that names none of
/// them is genuinely under-specified and still errors, so the inference never
/// has to guess between `content` and `metadata`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFilterItem {
    /// Filter category: "property", "content", "relationship", "metadata".
    /// Optional — see the type-level note; omitted values are inferred from the
    /// other fields rather than rejected.
    #[serde(rename = "type", default)]
    pub filter_type: Option<String>,
    /// Comparison operator: "equals", "contains", "gt", "lt", "gte", "lte",
    /// "in", "exists".
    pub operator: String,
    /// Property key (for property and metadata filters).
    #[serde(default)]
    pub property: Option<String>,
    /// Value to compare against.
    #[serde(default)]
    pub value: Option<Value>,
    /// Case sensitivity for text comparisons (default: true).
    #[serde(default)]
    pub case_sensitive: Option<bool>,
    /// Relationship type for relationship filters.
    #[serde(default)]
    pub relationship_type: Option<String>,
    /// Target node ID for relationship filters.
    #[serde(default)]
    pub node_id: Option<String>,
}

/// A single sort config item as passed by the agent tool.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSortItem {
    pub field: String,
    #[serde(default)]
    pub direction: Option<String>,
}

// ============================================================================
// Input / Output
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ExecuteQueryInput {
    /// Target node type ("task", "text", etc.) or "*" for all types.
    pub target_type: String,
    /// List of filter conditions.
    #[serde(default)]
    pub filters: Vec<AgentFilterItem>,
    /// Optional sorting.
    #[serde(default)]
    pub sorting: Option<Vec<AgentSortItem>>,
    /// Max results to return (default: 50).
    #[serde(default)]
    pub limit: Option<usize>,
}

pub type ExecuteQueryOutput = crate::ops::node_ops::QueryNodesOutput;

// ============================================================================
// Conversion helpers
// ============================================================================

fn parse_filter_type(s: &str) -> Result<FilterType, OpsError> {
    match s {
        "property" => Ok(FilterType::Property),
        "content" => Ok(FilterType::Content),
        "relationship" => Ok(FilterType::Relationship),
        "metadata" => Ok(FilterType::Metadata),
        other => Err(OpsError::InvalidParams(format!(
            "Unknown filter type '{}'. Supported: property, content, relationship, metadata",
            other
        ))),
    }
}

fn parse_filter_operator(s: &str) -> Result<FilterOperator, OpsError> {
    match s {
        "equals" => Ok(FilterOperator::Equals),
        "contains" => Ok(FilterOperator::Contains),
        "gt" => Ok(FilterOperator::GreaterThan),
        "lt" => Ok(FilterOperator::LessThan),
        "gte" => Ok(FilterOperator::GreaterThanOrEqual),
        "lte" => Ok(FilterOperator::LessThanOrEqual),
        "in" => Ok(FilterOperator::In),
        "exists" => Ok(FilterOperator::Exists),
        other => Err(OpsError::InvalidParams(format!(
            "Unknown operator '{}'. Supported: equals, contains, gt, lt, gte, lte, in, exists",
            other
        ))),
    }
}

fn parse_sort_direction(s: &str) -> SortDirection {
    match s {
        "desc" => SortDirection::Descending,
        _ => SortDirection::Ascending,
    }
}

fn parse_relationship_type(
    s: &str,
) -> Result<crate::services::query_service::RelationshipType, OpsError> {
    use crate::services::query_service::RelationshipType;
    match s {
        "parent" => Ok(RelationshipType::Parent),
        "children" => Ok(RelationshipType::Children),
        "mentions" => Ok(RelationshipType::Mentions),
        "mentioned_by" => Ok(RelationshipType::MentionedBy),
        // A relationship *filter* spans only the structural graph. A
        // schema-declared name — forward or reverse — is a traversal, not a
        // filter, and belongs to `get_related_nodes`; say so, because the name
        // itself is usually correct and only the verb is wrong.
        other => Err(OpsError::InvalidParams(format!(
            "Unknown relationship type '{}'. Supported: parent, children, mentions, \
             mentioned_by. Schema-declared relationship names (and their reverseName) \
             are not filterable here — traverse them with get_related_nodes / \
             `nodespace relationship get <id> --type {}` instead.",
            other, other
        ))),
    }
}

impl AgentFilterItem {
    /// The filter category, as given or inferred from the other fields.
    ///
    /// Errors only when the item names nothing to infer from — that is a filter
    /// with no subject at all, which no category would rescue.
    fn category(&self) -> Result<&str, OpsError> {
        // A supplied category is honoured only when it actually names one.
        //
        // The model routinely writes the *node* type here instead — `"type":
        // "task"` alongside `"property": "status"` — which is a category-slot
        // confusion, not a filter it meant differently: `node_type` is a
        // sibling parameter and carries the same value on the same call.
        // Observed 3 of 3 on the locked model, and it is the same rejection the
        // original production report hit, so the two are one defect.
        //
        // Falling through to inference rather than erroring costs nothing in
        // precision: the fields inference reads (`property`,
        // `relationship_type`, `node_id`) name the filter's actual subject, so
        // a filter naming `status` is a property filter whatever the model put
        // in the category slot. An item that names no subject still errors
        // below, and a *correct* category is still taken as given.
        if let Some(t) = self.filter_type.as_deref() {
            if is_known_category(t) {
                return Ok(t);
            }
        }
        if self.relationship_type.is_some() || self.node_id.is_some() {
            return Ok("relationship");
        }
        if let Some(prop) = self.property.as_deref() {
            // `content` is the top-level SQL `content` column, not a key inside the
            // `properties` JSON blob — so a filter naming it must route to the
            // content search (`LOWER(content) LIKE …`), NOT the property path, which
            // builds `json_extract(properties, '$.<type>.content')` that is
            // structurally always NULL and yields a silent `count: 0` false-negative
            // (an existing "Buy cereal" task returns nothing). `title` is a
            // top-level column too, and routes to the metadata category that
            // reads it directly, for the same reason. Any other property name
            // keeps the property path.
            return Ok(match prop {
                "content" => "content",
                "title" => "metadata",
                _ => "property",
            });
        }
        // Nothing to infer from. When the model *did* supply a category, name it
        // — otherwise the error reads as "you omitted type" to a caller who
        // supplied one, and points the repair at the wrong field.
        Err(OpsError::InvalidParams(match self.filter_type.as_deref() {
            Some(given) => format!(
                "Unknown filter type '{given}'. Supported: property, content, relationship, \
                 metadata. Note 'type' is the filter category, not the node type — use the \
                 'node_type' parameter for that. This filter also names no 'property' or \
                 'relationship_type' to infer the category from."
            ),
            None => "filter must specify 'type' (property, content, relationship, metadata), \
                     or name a 'property' or 'relationship_type' it can be inferred from"
                .to_string(),
        }))
    }
}

/// Whether `s` names one of the four filter categories.
///
/// Defers to [`parse_filter_type`] rather than re-listing the set, so there is
/// exactly one authority on what a category is and the two cannot drift apart.
fn is_known_category(s: &str) -> bool {
    parse_filter_type(s).is_ok()
}

fn to_query_filter(item: AgentFilterItem) -> Result<QueryFilter, OpsError> {
    let filter_type = parse_filter_type(item.category()?)?;
    let operator = parse_filter_operator(&item.operator)?;

    let relationship_type = match &item.relationship_type {
        Some(rt) => Some(parse_relationship_type(rt)?),
        None => None,
    };

    Ok(QueryFilter {
        filter_type,
        operator,
        property: item.property,
        value: item.value,
        case_sensitive: item.case_sensitive,
        relationship_type,
        node_id: item.node_id,
    })
}

fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<Value>, OpsError> {
    crate::models::nodes_to_typed_values(nodes).map_err(OpsError::Internal)
}

// ============================================================================
// Identifier validation
// ============================================================================

/// Validate that an identifier (node type, property name, sort field) only
/// contains characters that are safe to interpolate into a SQL identifier
/// position. The allowlist is `[A-Za-z0-9_:-]` which covers all real node
/// types, property keys, and metadata fields while blocking injection vectors.
fn validate_identifier(value: &str, label: &str) -> Result<(), OpsError> {
    if value == "*" {
        return Ok(());
    }
    if value.is_empty() {
        return Err(OpsError::InvalidParams(format!(
            "{} must not be empty",
            label
        )));
    }
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':')
    {
        Ok(())
    } else {
        Err(OpsError::InvalidParams(format!(
            "{} '{}' contains invalid characters; only [A-Za-z0-9_:-] are allowed",
            label, value
        )))
    }
}

// ============================================================================
// Operation
// ============================================================================

/// Execute a structured property query via `QueryService`, returning raw
/// domain `Node`s.
///
/// Converts the agent's flat filter shape into a `QueryDefinition` and
/// delegates to `QueryService::execute`, which generates proper SQL
/// `json_extract` conditions against SQLite. Shared by `execute_query`
/// (agent tool call, typed-JSON output) and the gRPC `ExecuteQuery` handler
/// (proto `NodeData` output) so validation/mapping isn't duplicated.
pub async fn execute_query_nodes(
    node_service: &Arc<NodeService>,
    input: ExecuteQueryInput,
) -> Result<Vec<Node>, OpsError> {
    let query = to_query_definition(input)?;

    let query_service = QueryService::new(node_service.store().clone());
    query_service
        .execute(&query)
        .await
        .map_err(|e| OpsError::Internal(format!("execute_query failed: {}", e)))
}

/// Validate the agent's filter shape and map it to a [`QueryDefinition`].
///
/// Shared by the executing and counting entry points so the two cannot drift:
/// a filter that selects some set of rows must count that same set, and both
/// the identifier validation and the filter/sort mapping are what decide which
/// set that is.
fn to_query_definition(input: ExecuteQueryInput) -> Result<QueryDefinition, OpsError> {
    validate_identifier(&input.target_type, "target_type")?;

    for item in &input.filters {
        if let Some(prop) = &item.property {
            validate_identifier(prop, "filter property")?;
        }
    }

    if let Some(sorting) = &input.sorting {
        for sort in sorting {
            validate_identifier(&sort.field, "sort field")?;
        }
    }

    let limit = input.limit.unwrap_or(50);

    let filters: Vec<QueryFilter> = input
        .filters
        .into_iter()
        .map(to_query_filter)
        .collect::<Result<_, _>>()?;

    let sorting: Option<Vec<SortConfig>> = input.sorting.map(|items| {
        items
            .into_iter()
            .map(|s| SortConfig {
                field: s.field,
                direction: parse_sort_direction(s.direction.as_deref().unwrap_or("asc")),
            })
            .collect()
    });

    Ok(QueryDefinition {
        target_type: input.target_type,
        filters,
        sorting,
        limit: Some(limit),
    })
}

/// Count the nodes a structured query matches, without materializing them.
///
/// The counting counterpart to [`execute_query_nodes`], backing the query
/// editor's preview: it answers "how many nodes match these filters?" with a
/// scalar rather than transferring every match to call `.len()` on the result.
///
/// The input's `sorting` and `limit` are still validated — an invalid sort
/// field is a malformed query whichever verb it is asked with — but neither
/// reaches the SQL, since ordering cannot change a count and a limit would cap
/// the very total this is asked for. The count is exact for any number of
/// matches.
pub async fn count_query(
    node_service: &Arc<NodeService>,
    input: ExecuteQueryInput,
) -> Result<i64, OpsError> {
    let query = to_query_definition(input)?;

    let query_service = QueryService::new(node_service.store().clone());
    query_service
        .count(&query)
        .await
        .map_err(|e| OpsError::Internal(format!("count_query failed: {}", e)))
}

/// Execute a structured property query, returning typed JSON values.
///
/// Thin wrapper over [`execute_query_nodes`] for callers (agent tool call)
/// that want the typed-value shape rather than raw `Node`s.
pub async fn execute_query(
    node_service: &Arc<NodeService>,
    input: ExecuteQueryInput,
) -> Result<ExecuteQueryOutput, OpsError> {
    let nodes = execute_query_nodes(node_service, input).await?;
    let count = nodes.len();
    let typed_nodes = nodes_to_typed_values(nodes)?;

    Ok(ExecuteQueryOutput {
        nodes: typed_nodes,
        count,
        collection_id: None,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_all_operators() {
        for (s, expected) in [
            ("equals", FilterOperator::Equals),
            ("contains", FilterOperator::Contains),
            ("gt", FilterOperator::GreaterThan),
            ("lt", FilterOperator::LessThan),
            ("gte", FilterOperator::GreaterThanOrEqual),
            ("lte", FilterOperator::LessThanOrEqual),
            ("in", FilterOperator::In),
            ("exists", FilterOperator::Exists),
        ] {
            let parsed = parse_filter_operator(s).unwrap();
            assert_eq!(parsed, expected);
        }
    }

    #[test]
    fn parse_unknown_operator_is_error() {
        let err = parse_filter_operator("like").unwrap_err();
        assert!(matches!(err, OpsError::InvalidParams(_)));
    }

    #[test]
    fn parse_all_filter_types() {
        for s in ["property", "content", "relationship", "metadata"] {
            assert!(parse_filter_type(s).is_ok());
        }
        assert!(parse_filter_type("unknown").is_err());
    }

    #[test]
    fn to_query_filter_property() {
        let item = AgentFilterItem {
            filter_type: Some("property".to_string()),
            operator: "equals".to_string(),
            property: Some("status".to_string()),
            value: Some(json!("open")),
            case_sensitive: None,
            relationship_type: None,
            node_id: None,
        };
        let qf = to_query_filter(item).unwrap();
        assert_eq!(qf.filter_type, FilterType::Property);
        assert_eq!(qf.operator, FilterOperator::Equals);
        assert_eq!(qf.property.as_deref(), Some("status"));
        assert_eq!(qf.value, Some(json!("open")));
    }

    /// A filter that names a property but omits `type` is complete enough to
    /// run: the category is derivable, and rejecting it turns a correct query
    /// into a tool error over a token the model gains nothing by restating.
    #[test]
    fn filter_type_is_inferred_for_a_property_filter() {
        let item: AgentFilterItem = serde_json::from_value(json!({
            "operator": "equals",
            "property": "replacement_cost",
            "value": 2400
        }))
        .expect("`type` must be optional on the wire");
        assert_eq!(item.category().unwrap(), "property");
        let qf = to_query_filter(item).unwrap();
        assert_eq!(qf.filter_type, FilterType::Property);
        assert_eq!(qf.property.as_deref(), Some("replacement_cost"));
    }

    /// A filter naming `property: "content"` with no explicit `type`
    /// must infer the CONTENT category. Content lives in the top-level SQL `content`
    /// column, so routing it to the property path builds a `json_extract(properties,
    /// …)` that is structurally always NULL and silently returns zero results (an
    /// existing "Buy cereal" task returns `count: 0`). A non-content property is
    /// unaffected and still infers the property category.
    #[test]
    fn content_property_infers_the_content_filter_not_json_extract() {
        let item: AgentFilterItem = serde_json::from_value(json!({
            "operator": "contains",
            "property": "content",
            "value": "cereal"
        }))
        .expect("`type` must be optional on the wire");
        assert_eq!(item.category().unwrap(), "content");
        assert_eq!(
            to_query_filter(item).unwrap().filter_type,
            FilterType::Content
        );

        let prop: AgentFilterItem = serde_json::from_value(json!({
            "operator": "equals",
            "property": "status",
            "value": "open"
        }))
        .unwrap();
        assert_eq!(prop.category().unwrap(), "property");
        assert_eq!(
            to_query_filter(prop).unwrap().filter_type,
            FilterType::Property
        );
    }

    /// Relationship filters carry their own distinguishing fields, so they are
    /// inferable too — and must not be mistaken for property filters.
    #[test]
    fn filter_type_is_inferred_for_a_relationship_filter() {
        let item: AgentFilterItem = serde_json::from_value(json!({
            "operator": "equals",
            "relationship_type": "children",
            "node_id": "abc-123"
        }))
        .unwrap();
        assert_eq!(item.category().unwrap(), "relationship");
    }

    /// An explicit `type` always wins over inference — a caller that says
    /// `content` gets `content`, even alongside a `property` key.
    #[test]
    fn explicit_filter_type_overrides_inference() {
        let item: AgentFilterItem = serde_json::from_value(json!({
            "type": "metadata",
            "operator": "equals",
            "property": "created_at",
            "value": "2026-01-01"
        }))
        .unwrap();
        assert_eq!(item.category().unwrap(), "metadata");
    }

    /// The reported shape: the model wrote the *node* type into the category
    /// slot while `node_type` carried the same value alongside. The filter names
    /// `status`, so its subject is unambiguous and it must run as a property
    /// filter rather than being rejected over the mislabelled slot.
    #[test]
    fn node_type_in_the_category_slot_falls_through_to_inference() {
        let item: AgentFilterItem = serde_json::from_value(json!({
            "type": "task",
            "operator": "equals",
            "property": "status",
            "value": "open"
        }))
        .unwrap();
        assert_eq!(item.category().unwrap(), "property");
        assert!(to_query_filter(item).is_ok());
    }

    /// Falling through must not become "accept anything". With the category slot
    /// wrong *and* no subject named, there is still nothing to infer from — and
    /// the error must name the value the caller actually supplied rather than
    /// telling them they omitted a field they did not omit.
    #[test]
    fn unknown_category_with_nothing_to_infer_from_errors_naming_the_value() {
        let item: AgentFilterItem = serde_json::from_value(json!({
            "type": "task",
            "operator": "exists",
            "value": true
        }))
        .unwrap();
        let err = item.category().unwrap_err().to_string();
        assert!(
            err.contains("task") && err.contains("node_type"),
            "the error must name the supplied value and point at the right parameter, got: {err}"
        );
    }

    /// Inference must not paper over a filter with no subject at all — there is
    /// nothing to infer from, and silently picking a category would run a query
    /// the caller never described.
    #[test]
    fn filter_with_nothing_to_infer_from_still_errors() {
        let item: AgentFilterItem = serde_json::from_value(json!({
            "operator": "exists",
            "value": true
        }))
        .unwrap();
        assert!(item.category().is_err());
        assert!(to_query_filter(item).is_err());
    }

    #[test]
    fn execute_query_input_deserializes_minimal() {
        let v = json!({"target_type": "task"});
        let input: ExecuteQueryInput = serde_json::from_value(v).unwrap();
        assert_eq!(input.target_type, "task");
        assert!(input.filters.is_empty());
        assert!(input.sorting.is_none());
        assert!(input.limit.is_none());
    }

    #[test]
    fn execute_query_input_deserializes_full() {
        let v = json!({
            "target_type": "task",
            "filters": [
                {"type": "property", "operator": "equals", "property": "status", "value": "open"}
            ],
            "sorting": [{"field": "due_date", "direction": "asc"}],
            "limit": 25
        });
        let input: ExecuteQueryInput = serde_json::from_value(v).unwrap();
        assert_eq!(input.filters.len(), 1);
        assert_eq!(input.sorting.as_ref().unwrap().len(), 1);
        assert_eq!(input.limit, Some(25));
    }

    // -- Unknown-field rejection (acceptance criterion, #1816) --

    #[test]
    fn agent_filter_item_rejects_unknown_field() {
        let args = json!({
            "type": "property",
            "operator": "equals",
            "property": "status",
            "caseSensitive": false
        });
        let err = serde_json::from_value::<AgentFilterItem>(args).unwrap_err();
        assert!(
            err.to_string().contains("caseSensitive"),
            "expected error naming `caseSensitive`, got: {err}"
        );
    }

    #[test]
    fn agent_sort_item_rejects_unknown_field() {
        let args = json!({ "field": "due_date", "direction": "asc", "order": "asc" });
        let err = serde_json::from_value::<AgentSortItem>(args).unwrap_err();
        assert!(
            err.to_string().contains("order"),
            "expected error naming `order`, got: {err}"
        );
    }

    #[test]
    fn validate_identifier_accepts_valid() {
        assert!(validate_identifier("task", "t").is_ok());
        assert!(validate_identifier("due_date", "t").is_ok());
        assert!(validate_identifier("custom:field", "t").is_ok());
        assert!(validate_identifier("my-type", "t").is_ok());
        assert!(validate_identifier("*", "t").is_ok());
    }

    #[test]
    fn validate_identifier_rejects_injection() {
        assert!(validate_identifier("'; DROP TABLE node; --", "t").is_err());
        assert!(validate_identifier("a b", "t").is_err());
        assert!(validate_identifier("a.b", "t").is_err());
        assert!(validate_identifier("", "t").is_err());
    }

    mod integration {
        use super::*;
        use crate::db::SqliteStore;
        use crate::models::Node;
        use crate::services::NodeService;
        use tempfile::TempDir;

        async fn make_test_service() -> (Arc<NodeService>, TempDir) {
            let tmp = TempDir::new().unwrap();
            let db_path = tmp.path().join("test.db");
            let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
            let svc = Arc::new(NodeService::new(&mut store).await.unwrap());
            (svc, tmp)
        }

        fn task_node(id: &str, status: &str, due_date: Option<&str>) -> Node {
            let mut props = json!({"status": status});
            if let Some(d) = due_date {
                props["due_date"] = json!(d);
            }
            Node {
                id: id.to_string(),
                node_type: "task".to_string(),
                content: format!("Task {}", id),
                version: 1,
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                properties: props,
                mentions: vec![],
                mentioned_in: vec![],
                title: Some(format!("Task {}", id)),
                lifecycle_status: "active".to_string(),
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn execute_query_filters_by_status() {
            let (svc, _tmp) = make_test_service().await;

            svc.create_node(task_node("t1", "open", None))
                .await
                .unwrap();
            svc.create_node(task_node("t2", "done", None))
                .await
                .unwrap();
            svc.create_node(task_node("t3", "open", None))
                .await
                .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "task",
                "filters": [
                    {"type": "property", "operator": "equals", "property": "status", "value": "open"}
                ]
            }))
            .unwrap();

            let output = execute_query(&svc, input).await.unwrap();
            assert_eq!(
                output.count, 2,
                "expected 2 open tasks, got {}",
                output.count
            );
            for node in &output.nodes {
                assert_eq!(node.get("status").and_then(|v| v.as_str()), Some("open"));
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn execute_query_rejects_invalid_identifier() {
            let (svc, _tmp) = make_test_service().await;

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "task'; DROP TABLE node; --",
                "filters": []
            }))
            .unwrap();

            let err = execute_query(&svc, input).await.unwrap_err();
            assert!(matches!(err, OpsError::InvalidParams(_)));
        }
    }
}
