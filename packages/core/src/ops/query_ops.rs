//! Execute-query ops wrapper.
//!
//! Bridges agent tool calls to `QueryService`. The agent passes a
//! flat-property filter shape; this module maps it to `QueryDefinition`
//! and delegates to `QueryService::execute`.

use crate::models::Node;
use crate::ops::path_ops::resolve_path;
use crate::ops::OpsError;
use crate::services::node_service::NodeService;
use crate::services::query_service::{
    FilterOperator, FilterType, QueryDefinition, QueryFilter, QueryService, RelationshipPath,
    SortConfig, SortDirection,
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
/// infers it: a filter naming a nested `filter` is a related-node filter, one
/// naming a `path` or an anchor `node_id` is a relationship filter, and
/// anything naming a `property` is a property filter. An item that names none
/// of them is genuinely under-specified and still errors, so the inference
/// never has to guess between `content` and `metadata`.
#[derive(Debug, Clone, Default, Deserialize)]
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
    /// The node a relationship filter's `path` must reach.
    #[serde(default)]
    pub node_id: Option<String>,
    /// The relationships to follow from each candidate node, for a
    /// relationship or related-node filter: built-in names (`has_child`,
    /// `mentions`), schema-declared names, or the reverse name of either
    /// (`child_of`, `project`). Resolved against the enclosing query's
    /// `target_type` at conversion time.
    #[serde(default)]
    pub path: Option<RelationshipPath>,
    /// The nested filter a related-node filter evaluates against the nodes
    /// `path` reaches. Recursive by construction; validated to at most one
    /// level of `related` nesting (see
    /// `query_service::validate_filter_identifiers`/`MAX_RELATED_DEPTH`).
    #[serde(default)]
    pub filter: Option<Box<AgentFilterItem>>,
}

/// A single sort config item as passed by the agent tool.
#[derive(Debug, Clone, Deserialize)]
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
        "related" => Ok(FilterType::Related),
        other => Err(OpsError::InvalidParams(format!(
            "Unknown filter type '{}'. Supported: property, content, relationship, metadata, \
             related",
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
        // Checked before `path`/`node_id`: both relationship shapes carry a
        // `path`, and only the related-node one carries a nested `filter`.
        if self.filter.is_some() {
            return Ok("related");
        }
        if self.path.is_some() || self.node_id.is_some() {
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
                 'path' to infer the category from."
            ),
            None => "filter must specify 'type' (property, content, relationship, metadata), \
                     or name a 'property' or 'path' it can be inferred from"
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

/// Convert one agent filter item to a [`QueryFilter`]. The filter's path is
/// carried as written; [`resolve_filters`] resolves it against the schemas.
fn to_query_filter(item: AgentFilterItem) -> Result<QueryFilter, OpsError> {
    let filter_type = parse_filter_type(item.category()?)?;
    let operator = parse_filter_operator(&item.operator)?;

    let walks = matches!(filter_type, FilterType::Relationship | FilterType::Related);
    if walks && item.path.is_none() {
        return Err(OpsError::InvalidParams(format!(
            "A '{}' filter needs a 'path': the relationships to follow from each node, e.g. \
             [\"child_of\"] for its parent, [\"has_child\"] for its children, or a \
             schema-declared name or reverse name such as [\"project\"]",
            item.category()?
        )));
    }
    if filter_type == FilterType::Relationship && item.node_id.is_none() {
        return Err(OpsError::InvalidParams(
            "Relationship filter missing 'node_id': the node the path must reach".to_string(),
        ));
    }
    let filter = match (filter_type == FilterType::Related, item.filter) {
        (true, Some(nested)) => Some(Box::new(to_query_filter(*nested)?)),
        (true, None) => {
            return Err(OpsError::InvalidParams(
                "Related filter missing 'filter'".to_string(),
            ));
        }
        (false, _) => None,
    };

    Ok(QueryFilter {
        filter_type,
        operator,
        property: item.property,
        value: item.value,
        case_sensitive: item.case_sensitive,
        node_id: item.node_id,
        path: item.path.filter(|_| walks),
        filter,
        resolved_path: None,
    })
}

/// Resolve the paths of `filters` against the schemas, so the query service
/// can compile them.
///
/// `target_type` is the type the filters are evaluated against: the query's
/// own `target_type`. A nested filter of a related-node filter is evaluated
/// against the nodes the outer path reaches, so its own path resolves against
/// their declared type. Under a wildcard (`"*"`) there is no schema to
/// resolve against, so only built-in relationships resolve; a
/// schema-declared name errors rather than guessing which type's declaration
/// applies.
///
/// A stored query and a play's selector carry their paths as written, so
/// both come through here every time they run, and when they are saved: a
/// name that resolves to nothing is an error then, not an empty result later.
pub async fn resolve_filters(
    node_service: &NodeService,
    target_type: &str,
    filters: Vec<QueryFilter>,
) -> Result<Vec<QueryFilter>, OpsError> {
    let mut resolved = Vec::with_capacity(filters.len());
    for filter in filters {
        resolved.push(resolve_filter(node_service, target_type, filter).await?);
    }
    Ok(resolved)
}

/// Explicitly boxed (`Pin<Box<dyn Future>>`) rather than a plain `async fn`:
/// this function calls itself for a related-node filter's nested filter, and
/// a self-recursive `async fn` is an infinitely-sized future type by
/// construction — boxing is what gives the recursive call a fixed size.
fn resolve_filter<'a>(
    node_service: &'a NodeService,
    target_type: &'a str,
    mut filter: QueryFilter,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<QueryFilter, OpsError>> + Send + 'a>>
{
    Box::pin(async move {
        let Some(path) = &filter.path else {
            return Ok(filter);
        };
        if path.is_empty() {
            return Err(OpsError::InvalidParams(
                "filter 'path' must name at least one relationship".to_string(),
            ));
        }
        let start_type = (target_type != "*").then_some(target_type);
        let resolved = resolve_path(node_service, start_type, path).await?;

        if let Some(nested) = filter.filter.take() {
            let related_type = resolved.far_type().unwrap_or("*").to_string();
            filter.filter = Some(Box::new(
                resolve_filter(node_service, &related_type, *nested).await?,
            ));
        }
        filter.resolved_path = Some(resolved);
        Ok(filter)
    })
}

fn nodes_to_typed_values(nodes: Vec<Node>) -> Result<Vec<Value>, OpsError> {
    crate::models::nodes_to_typed_values(nodes).map_err(OpsError::Internal)
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
    let query = to_query_definition(node_service, input).await?;

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
///
/// Async because a relationship or related-node filter's `path` is resolved
/// here, against `input.target_type` — a schema lookup — before the resulting
/// [`QueryDefinition`] ever reaches [`QueryService`]'s (synchronous) SQL
/// compilation. See [`resolve_filters`].
async fn to_query_definition(
    node_service: &Arc<NodeService>,
    input: ExecuteQueryInput,
) -> Result<QueryDefinition, OpsError> {
    let limit = input.limit.unwrap_or(50);

    let filters = input
        .filters
        .into_iter()
        .map(to_query_filter)
        .collect::<Result<Vec<_>, _>>()?;
    let filters = resolve_filters(node_service, &input.target_type, filters).await?;

    let sorting: Option<Vec<SortConfig>> = input.sorting.map(|items| {
        items
            .into_iter()
            .map(|s| SortConfig {
                field: s.field,
                direction: parse_sort_direction(s.direction.as_deref().unwrap_or("asc")),
            })
            .collect()
    });

    let query = QueryDefinition {
        target_type: input.target_type,
        filters,
        sorting,
        limit: Some(limit),
    };
    // `QueryService` enforces this itself; checking here too classifies a bad
    // identifier as the caller's error rather than an execution failure.
    query
        .validate_identifiers()
        .map_err(|e| OpsError::InvalidParams(e.to_string()))?;
    Ok(query)
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
    let query = to_query_definition(node_service, input).await?;

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
    use crate::db::SqliteStore;
    use crate::services::NodeService;
    use serde_json::json;
    use tempfile::TempDir;

    /// A `NodeService` over a fresh database: the core schemas and nothing
    /// else.
    async fn make_test_service() -> (Arc<NodeService>, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
        let svc = Arc::new(NodeService::new(&mut store).await.unwrap());
        (svc, tmp)
    }

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
            ..Default::default()
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
            "path": ["child_of"],
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

    // -- Unknown-field rejection (acceptance criterion) --

    /// A typed client sends a stored filter and sort item as serialized, so
    /// the execute input must accept every key they carry and read each back
    /// to the same value. A key added to `QueryFilter` or `SortConfig` and not
    /// to the input fails here instead of failing every saved query.
    ///
    /// The fixtures name every field, with no `..Default::default()`, so a
    /// field added to `QueryFilter` stops this test compiling until it is set
    /// here, however the field is serialized.
    #[test]
    fn the_execute_input_accepts_what_the_stored_types_serialize() {
        let stored = vec![
            QueryFilter {
                filter_type: FilterType::Related,
                operator: FilterOperator::Exists,
                property: None,
                value: None,
                case_sensitive: None,
                node_id: None,
                path: Some(
                    serde_json::from_value(
                        json!([{ "name": "child_of", "open_ended": true }, "project"]),
                    )
                    .unwrap(),
                ),
                filter: Some(Box::new(QueryFilter {
                    filter_type: FilterType::Property,
                    operator: FilterOperator::Contains,
                    property: Some("status".to_string()),
                    value: Some(json!("act")),
                    case_sensitive: Some(false),
                    node_id: None,
                    path: None,
                    filter: None,
                    resolved_path: None,
                })),
                resolved_path: None,
            },
            QueryFilter {
                filter_type: FilterType::Relationship,
                operator: FilterOperator::Equals,
                property: None,
                value: None,
                case_sensitive: None,
                node_id: Some("n1".to_string()),
                path: Some(serde_json::from_value(json!(["mentions"])).unwrap()),
                filter: None,
                resolved_path: None,
            },
        ];
        for filter in stored {
            let item: AgentFilterItem =
                serde_json::from_value(serde_json::to_value(&filter).unwrap()).unwrap();
            assert_eq!(to_query_filter(item).unwrap(), filter);
        }

        for direction in [SortDirection::Ascending, SortDirection::Descending] {
            let sort = SortConfig {
                field: "due_date".to_string(),
                direction,
            };
            let item: AgentSortItem =
                serde_json::from_value(serde_json::to_value(&sort).unwrap()).unwrap();
            assert_eq!(item.field, sort.field);
            assert_eq!(
                parse_sort_direction(item.direction.as_deref().unwrap_or("asc")),
                sort.direction
            );
        }
    }

    /// A typed client sends a filter as it is stored, optional keys included
    /// as explicit nulls. Each reads as absent.
    #[test]
    fn agent_filter_item_reads_an_explicit_null_as_absent() {
        let item: AgentFilterItem = serde_json::from_value(json!({
            "type": "content",
            "operator": "contains",
            "value": "acme",
            "property": null,
            "case_sensitive": null,
            "node_id": null,
            "path": null,
            "filter": null
        }))
        .unwrap();
        assert!(item.property.is_none() && item.case_sensitive.is_none());
        assert!(item.node_id.is_none() && item.path.is_none() && item.filter.is_none());
    }

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

    mod integration {
        use super::*;
        use crate::models::Node;

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

            svc.create_node(task_node(
                "47b0416e-68db-58e9-805c-db17bfe8856d",
                "open",
                None,
            ))
            .await
            .unwrap();
            svc.create_node(task_node(
                "75489349-da91-5de1-bc06-2d7fe6ad7ccc",
                "done",
                None,
            ))
            .await
            .unwrap();
            svc.create_node(task_node(
                "94c19580-a99d-5cba-b653-2f70dcd839d7",
                "open",
                None,
            ))
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

        // -- FilterType::Related (cross-relationship filtering) --

        async fn create_schema(svc: &Arc<NodeService>, definition: serde_json::Value) {
            crate::schema::handle_create_schema(svc, definition)
                .await
                .unwrap_or_else(|e| panic!("schema creation failed: {e}"));
        }

        fn node(id: &str, node_type: &str, props: serde_json::Value) -> Node {
            let mut properties = serde_json::Map::new();
            properties.insert(node_type.to_string(), props);
            Node {
                id: id.to_string(),
                node_type: node_type.to_string(),
                content: format!("{} content", id),
                version: 1,
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                properties: Value::Object(properties),
                mentions: vec![],
                mentioned_in: vec![],
                title: Some(format!("{} title", id)),
                lifecycle_status: "active".to_string(),
            }
        }

        /// The issue's own motivating example: tasks belonging to a project
        /// with status active, through a schema-declared FORWARD relationship
        /// (`task.project` -> `project`).
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_forward_relationship_tasks_by_project_status() {
            let (svc, _tmp) = make_test_service().await;

            create_schema(
                &svc,
                json!({"name": "rf_project", "fields": [{"name": "status", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({
                    "name": "rf_task",
                    "fields": [],
                    "relationships": [{
                        "name": "project",
                        "targetType": "rf_project",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "tasks",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;

            svc.create_node(node(
                "dc9569ca-1867-5917-b11b-8b77a9236717",
                "rf_project",
                json!({"status": "active"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "d65e6776-2ea6-5bff-a3d4-cf789180883f",
                "rf_project",
                json!({"status": "completed"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "d0aaac45-70f6-537e-a669-b6da9229eb6a",
                "rf_task",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "d5255404-47a9-5cbc-ad65-a93d51e23b51",
                "rf_task",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_relationship(
                "d0aaac45-70f6-537e-a669-b6da9229eb6a",
                "project",
                "dc9569ca-1867-5917-b11b-8b77a9236717",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "d5255404-47a9-5cbc-ad65-a93d51e23b51",
                "project",
                "d65e6776-2ea6-5bff-a3d4-cf789180883f",
                json!({}),
            )
            .await
            .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf_task",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["project"],
                    "filter": {
                        "type": "property",
                        "operator": "equals",
                        "property": "status",
                        "value": "active"
                    }
                }]
            }))
            .unwrap();

            let output = execute_query(&svc, input).await.unwrap();
            assert_eq!(
                output.count, 1,
                "expected only task-a, got {:?}",
                output.nodes
            );
            assert_eq!(
                output.nodes[0].get("id").and_then(|v| v.as_str()),
                Some("d0aaac45-70f6-537e-a669-b6da9229eb6a")
            );
        }

        /// The reverse spelling of the same relationship: filtering projects
        /// by "has at least one task with severity critical" through the
        /// declared REVERSE name (`project.tasks`), which is a many-cardinality
        /// relationship from the project's end -- an existence test, not a
        /// single-match test, verified here by having the matching project
        /// have BOTH a critical and a non-critical task.
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_reverse_many_relationship_is_an_existence_test() {
            let (svc, _tmp) = make_test_service().await;

            create_schema(
                &svc,
                json!({
                    "name": "rf2_sprint",
                    "fields": []
                }),
            )
            .await;
            create_schema(
                &svc,
                json!({
                    "name": "rf2_task",
                    "fields": [{"name": "severity", "type": "text"}],
                    "relationships": [{
                        "name": "sprint",
                        "targetType": "rf2_sprint",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "tasks",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;

            svc.create_node(node(
                "036a1de4-68d2-56db-800d-0af9282deb1a",
                "rf2_sprint",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "ec7a4732-33ae-578f-b965-8d89e1d05970",
                "rf2_sprint",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "93bf1145-a86c-5584-a012-c98947702696",
                "rf2_task",
                json!({"severity": "critical"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "a7a56745-c02d-5d04-83df-5e02e9fb3366",
                "rf2_task",
                json!({"severity": "minor"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "201dfae7-602b-5e1d-97c1-23c741f34e19",
                "rf2_task",
                json!({"severity": "minor"}),
            ))
            .await
            .unwrap();
            // sprint-hot has both a critical and a minor task -- still one match.
            // The forward declaration (`sprint`) lives on `rf2_task`, so the
            // edge is created from the task's end, naming the forward name --
            // `tasks` is the reverse spelling the query filter below uses.
            svc.create_relationship(
                "93bf1145-a86c-5584-a012-c98947702696",
                "sprint",
                "036a1de4-68d2-56db-800d-0af9282deb1a",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "a7a56745-c02d-5d04-83df-5e02e9fb3366",
                "sprint",
                "036a1de4-68d2-56db-800d-0af9282deb1a",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "201dfae7-602b-5e1d-97c1-23c741f34e19",
                "sprint",
                "ec7a4732-33ae-578f-b965-8d89e1d05970",
                json!({}),
            )
            .await
            .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf2_sprint",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["tasks"],
                    "filter": {
                        "type": "property",
                        "operator": "equals",
                        "property": "severity",
                        "value": "critical"
                    }
                }]
            }))
            .unwrap();

            let output = execute_query(&svc, input).await.unwrap();
            assert_eq!(
                output.count, 1,
                "expected only sprint-hot, got {:?}",
                output.nodes
            );
            assert_eq!(
                output.nodes[0].get("id").and_then(|v| v.as_str()),
                Some("036a1de4-68d2-56db-800d-0af9282deb1a")
            );
        }

        /// A built-in structural relationship (`has_child`) as the Related
        /// filter's relationship_name -- "text nodes whose child task has
        /// status open".
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_builtin_has_child() {
            let (svc, _tmp) = make_test_service().await;

            svc.create_node(node(
                "764e4e18-2fe9-50fb-97a5-74002e615fbc",
                "text",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "d36de88d-02fc-5996-a026-70683445e160",
                "text",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_node(task_node(
                "6def7f87-3a08-5bd0-ac70-00954b87e7e1",
                "open",
                None,
            ))
            .await
            .unwrap();
            svc.create_node(task_node(
                "9eb5c42d-18c4-5e29-a8d5-2cb5e2799784",
                "done",
                None,
            ))
            .await
            .unwrap();
            svc.create_relationship(
                "764e4e18-2fe9-50fb-97a5-74002e615fbc",
                "has_child",
                "6def7f87-3a08-5bd0-ac70-00954b87e7e1",
                json!({}),
            )
            .await
            .unwrap();
            svc.create_relationship(
                "d36de88d-02fc-5996-a026-70683445e160",
                "has_child",
                "9eb5c42d-18c4-5e29-a8d5-2cb5e2799784",
                json!({}),
            )
            .await
            .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "text",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["has_child"],
                    "filter": {
                        "type": "property",
                        "operator": "equals",
                        "property": "status",
                        "value": "open"
                    }
                }]
            }))
            .unwrap();

            let output = execute_query(&svc, input).await.unwrap();
            assert_eq!(
                output.count, 1,
                "expected only parent-a, got {:?}",
                output.nodes
            );
            assert_eq!(
                output.nodes[0].get("id").and_then(|v| v.as_str()),
                Some("764e4e18-2fe9-50fb-97a5-74002e615fbc")
            );
        }

        /// A relationship declared only on an ancestor schema and inherited
        /// (not redeclared) via ADR-078 `extends` -- the story/epic-through-
        /// task-declared-relationship shape the issue calls out directly.
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_through_extends_inherited_relationship() {
            let (svc, _tmp) = make_test_service().await;

            create_schema(
                &svc,
                json!({"name": "rf3_epic", "fields": [{"name": "status", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({
                    "name": "rf3_task",
                    "fields": [],
                    "relationships": [{
                        "name": "epic",
                        "targetType": "rf3_epic",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "issues",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;
            create_schema(
                &svc,
                json!({"name": "rf3_story", "extends": "rf3_task", "fields": []}),
            )
            .await;

            svc.create_node(node(
                "00e0ccd8-aa1e-53da-92f6-6dab31ad8626",
                "rf3_epic",
                json!({"status": "active"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "2563689f-df77-5f7c-9aad-879d135a915f",
                "rf3_story",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_relationship(
                "2563689f-df77-5f7c-9aad-879d135a915f",
                "epic",
                "00e0ccd8-aa1e-53da-92f6-6dab31ad8626",
                json!({}),
            )
            .await
            .expect("create_relationship must succeed for an inherited relationship");

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf3_story",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["epic"],
                    "filter": {
                        "type": "property",
                        "operator": "equals",
                        "property": "status",
                        "value": "active"
                    }
                }]
            }))
            .unwrap();

            let output = execute_query(&svc, input).await.unwrap();
            assert_eq!(
                output.count, 1,
                "expected the inherited relationship to resolve and filter correctly, got {:?}",
                output.nodes
            );
        }

        /// A Related filter whose own nested filter is ALSO Related (depth 2)
        /// is rejected at validation time -- not silently truncated or
        /// misexecuted.
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_depth_greater_than_one_is_rejected() {
            let (svc, _tmp) = make_test_service().await;

            create_schema(&svc, json!({"name": "rf4_owner", "fields": []})).await;
            create_schema(
                &svc,
                json!({
                    "name": "rf4_project",
                    "fields": [],
                    "relationships": [{
                        "name": "owner",
                        "targetType": "rf4_owner",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "projects",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;
            create_schema(
                &svc,
                json!({
                    "name": "rf4_task",
                    "fields": [],
                    "relationships": [{
                        "name": "project",
                        "targetType": "rf4_project",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "tasks",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf4_task",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["project"],
                    "filter": {
                        "type": "related",
                        "operator": "equals",
                        "path": ["owner"],
                        "filter": {
                            "type": "property",
                            "operator": "exists",
                            "property": "id"
                        }
                    }
                }]
            }))
            .unwrap();

            let err = execute_query(&svc, input).await.unwrap_err();
            assert!(
                matches!(err, OpsError::InvalidParams(_)),
                "expected a depth-cap rejection, got: {err:?}"
            );
        }

        /// A misspelled/undeclared relationship_name is an error, not a
        /// silently empty result -- same posture as
        /// `resolve_relationship_name`.
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_unknown_relationship_name_errors() {
            let (svc, _tmp) = make_test_service().await;

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "task",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["not_a_real_relationship"],
                    "filter": {
                        "type": "property",
                        "operator": "equals",
                        "property": "status",
                        "value": "open"
                    }
                }]
            }))
            .unwrap();

            let err = execute_query(&svc, input).await.unwrap_err();
            assert!(matches!(err, OpsError::InvalidParams(_)));
        }

        /// A schema-declared name cannot be resolved under a wildcard
        /// target_type: there is no single schema to resolve it against, so
        /// this errors rather than guessing.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_declared_name_under_a_wildcard_target_type_errors() {
            let (svc, _tmp) = make_test_service().await;

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "*",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["project"],
                    "filter": {
                        "type": "property",
                        "operator": "equals",
                        "property": "status",
                        "value": "open"
                    }
                }]
            }))
            .unwrap();

            let err = execute_query(&svc, input).await.unwrap_err();
            assert!(
                matches!(&err, OpsError::InvalidParams(m) if m.contains("'*'")),
                "{err:?}"
            );
        }

        /// A built-in relationship needs no schema, so it resolves under a
        /// wildcard too: any node whose child is an open task.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_builtin_name_resolves_under_a_wildcard_target_type() {
            let (svc, _tmp) = make_test_service().await;

            svc.create_node(node(
                "5b7e0b5e-6f0e-4d0a-9d0e-0a8b6f1f2a01",
                "text",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_node(task_node(
                "5b7e0b5e-6f0e-4d0a-9d0e-0a8b6f1f2a02",
                "open",
                None,
            ))
            .await
            .unwrap();
            svc.create_relationship(
                "5b7e0b5e-6f0e-4d0a-9d0e-0a8b6f1f2a01",
                "has_child",
                "5b7e0b5e-6f0e-4d0a-9d0e-0a8b6f1f2a02",
                json!({}),
            )
            .await
            .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "*",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["has_child"],
                    "filter": {
                        "type": "metadata",
                        "operator": "equals",
                        "property": "node_type",
                        "value": "task"
                    }
                }]
            }))
            .unwrap();

            let output = execute_query(&svc, input).await.unwrap();
            assert_eq!(output.count, 1, "{:?}", output.nodes);
            assert_eq!(
                output.nodes[0].get("id").and_then(|v| v.as_str()),
                Some("5b7e0b5e-6f0e-4d0a-9d0e-0a8b6f1f2a01")
            );
        }

        /// The forward name of an inbound relationship, walked backwards
        /// (`ResolvedRelName::InboundForward`) -- a distinct resolution
        /// branch from `Forward`/`Reverse`/`Builtin`, exercised here with
        /// the forward name itself rather than its `reverseName` spelling.
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_inbound_forward_relationship() {
            let (svc, _tmp) = make_test_service().await;

            create_schema(
                &svc,
                json!({"name": "rf5_project", "fields": [{"name": "status", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({
                    "name": "rf5_task",
                    "fields": [],
                    "relationships": [{
                        "name": "project",
                        "targetType": "rf5_project",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "tasks",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;

            svc.create_node(node(
                "9ac587e9-f1b5-5f0c-881d-3719852c028b",
                "rf5_project",
                json!({"status": "active"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "054b66fd-0362-5b43-86a6-1961eda0112b",
                "rf5_task",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_relationship(
                "054b66fd-0362-5b43-86a6-1961eda0112b",
                "project",
                "9ac587e9-f1b5-5f0c-881d-3719852c028b",
                json!({}),
            )
            .await
            .unwrap();

            // Walked from the project's end using the forward name itself
            // ("project"), not its reverseName ("tasks") -- the
            // InboundForward branch, distinct from the Reverse test above.
            // rf5_task declares no fields, so the nested filter matches on
            // a metadata field (node_type) rather than a property.
            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf5_project",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["project"],
                    "filter": {
                        "type": "metadata",
                        "operator": "equals",
                        "property": "node_type",
                        "value": "rf5_task"
                    }
                }]
            }))
            .unwrap();

            let output = execute_query(&svc, input).await.unwrap();
            assert_eq!(
                output.count, 1,
                "expected rf5-proj-active via the InboundForward branch, got {:?}",
                output.nodes
            );
        }

        /// Existing bare-membership relationship filters (`parent`/
        /// `children`/`mentions`/`mentioned_by`) are unaffected by this
        /// issue's changes -- regression coverage alongside the
        /// `query_service` unit tests, exercised through the same
        /// agent-facing conversion path this issue changed.
        #[tokio::test(flavor = "multi_thread")]
        async fn bare_relationship_filter_still_works_unchanged() {
            let (svc, _tmp) = make_test_service().await;

            svc.create_node(task_node(
                "038ebfa1-35f7-5d7c-941f-7502434c9955",
                "open",
                None,
            ))
            .await
            .unwrap();
            svc.create_node(task_node(
                "dd996f5b-a763-58e3-a836-e9bc25d0827c",
                "open",
                None,
            ))
            .await
            .unwrap();
            svc.create_relationship(
                "038ebfa1-35f7-5d7c-941f-7502434c9955",
                "has_child",
                "dd996f5b-a763-58e3-a836-e9bc25d0827c",
                json!({}),
            )
            .await
            .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "task",
                "filters": [{
                    "type": "relationship",
                    "operator": "equals",
                    "path": ["has_child"],
                    "node_id": "dd996f5b-a763-58e3-a836-e9bc25d0827c"
                }]
            }))
            .unwrap();

            let output = execute_query(&svc, input).await.unwrap();
            assert_eq!(output.count, 1);
            assert_eq!(
                output.nodes[0].get("id").and_then(|v| v.as_str()),
                Some("038ebfa1-35f7-5d7c-941f-7502434c9955")
            );
        }

        // -- Paths: several hops, reverse names, open-ended walks --

        /// project ← tasks — task ← has_child — text, with ids that say what
        /// each node is.
        const ACTIVE_PROJECT: &str = "a1000000-0000-4000-8000-000000000001";
        const DONE_PROJECT: &str = "a1000000-0000-4000-8000-000000000002";
        const ACTIVE_TASK: &str = "a1000000-0000-4000-8000-000000000003";
        const DONE_TASK: &str = "a1000000-0000-4000-8000-000000000004";
        const NOTE_UNDER_ACTIVE_TASK: &str = "a1000000-0000-4000-8000-000000000005";
        const NOTE_UNDER_NOTE: &str = "a1000000-0000-4000-8000-000000000006";
        const NOTE_UNDER_DONE_TASK: &str = "a1000000-0000-4000-8000-000000000007";

        /// Two projects, a task in each, and notes nested under the tasks.
        async fn seed_project_tree(svc: &Arc<NodeService>) {
            create_schema(
                svc,
                json!({"name": "pt_project", "fields": [{"name": "status", "type": "text"}]}),
            )
            .await;
            create_schema(
                svc,
                json!({
                    "name": "pt_task",
                    "fields": [],
                    "relationships": [{
                        "name": "project",
                        "targetType": "pt_project",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "tasks",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;

            for (id, node_type, props) in [
                (ACTIVE_PROJECT, "pt_project", json!({"status": "active"})),
                (DONE_PROJECT, "pt_project", json!({"status": "done"})),
                (ACTIVE_TASK, "pt_task", json!({})),
                (DONE_TASK, "pt_task", json!({})),
                (NOTE_UNDER_ACTIVE_TASK, "text", json!({})),
                (NOTE_UNDER_NOTE, "text", json!({})),
                (NOTE_UNDER_DONE_TASK, "text", json!({})),
            ] {
                svc.create_node(node(id, node_type, props)).await.unwrap();
            }
            for (from, name, to) in [
                (ACTIVE_TASK, "project", ACTIVE_PROJECT),
                (DONE_TASK, "project", DONE_PROJECT),
                (ACTIVE_TASK, "has_child", NOTE_UNDER_ACTIVE_TASK),
                (NOTE_UNDER_ACTIVE_TASK, "has_child", NOTE_UNDER_NOTE),
                (DONE_TASK, "has_child", NOTE_UNDER_DONE_TASK),
            ] {
                svc.create_relationship(from, name, to, json!({}))
                    .await
                    .unwrap();
            }
        }

        async fn matching_ids(svc: &Arc<NodeService>, input: serde_json::Value) -> Vec<String> {
            let input: ExecuteQueryInput = serde_json::from_value(input).unwrap();
            let mut ids: Vec<String> = execute_query(svc, input)
                .await
                .unwrap()
                .nodes
                .iter()
                .map(|n| n["id"].as_str().unwrap().to_string())
                .collect();
            ids.sort();
            ids
        }

        /// A relationship filter takes a schema-declared name, not only the
        /// structural ones: the tasks of one project.
        #[tokio::test(flavor = "multi_thread")]
        async fn relationship_filter_follows_a_declared_name() {
            let (svc, _tmp) = make_test_service().await;
            seed_project_tree(&svc).await;

            let tasks = matching_ids(
                &svc,
                json!({
                    "target_type": "pt_task",
                    "filters": [{
                        "type": "relationship", "operator": "equals",
                        "path": ["project"], "node_id": ACTIVE_PROJECT
                    }]
                }),
            )
            .await;
            assert_eq!(tasks, [ACTIVE_TASK]);

            // The same edge from the other end, by its reverse name: the
            // project of one task.
            let projects = matching_ids(
                &svc,
                json!({
                    "target_type": "pt_project",
                    "filters": [{
                        "type": "relationship", "operator": "equals",
                        "path": ["tasks"], "node_id": DONE_TASK
                    }]
                }),
            )
            .await;
            assert_eq!(projects, [DONE_PROJECT]);
        }

        /// Two hops in one filter: notes whose parent task belongs to an
        /// active project. The second name is resolved against the type the
        /// first one reaches.
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_walks_several_hops() {
            let (svc, _tmp) = make_test_service().await;
            seed_project_tree(&svc).await;

            let tasks = matching_ids(
                &svc,
                json!({
                    "target_type": "pt_project",
                    "filters": [{
                        "type": "relationship", "operator": "equals",
                        "path": ["tasks", "has_child"], "node_id": NOTE_UNDER_ACTIVE_TASK
                    }]
                }),
            )
            .await;
            assert_eq!(tasks, [ACTIVE_PROJECT]);

            // A declared name after a built-in hop has no type to be resolved
            // against: any type may be a parent.
            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "text",
                "filters": [{
                    "type": "related", "operator": "equals",
                    "path": ["child_of", "project"],
                    "filter": {
                        "type": "property", "operator": "equals",
                        "property": "status", "value": "active"
                    }
                }]
            }))
            .unwrap();
            let err = execute_query(&svc, input).await.unwrap_err();
            assert!(
                matches!(&err, OpsError::InvalidParams(m) if m.contains("child_of")),
                "{err:?}"
            );
        }

        /// An open-ended hop follows the relationship to every depth:
        /// everything under a task, however deeply nested.
        #[tokio::test(flavor = "multi_thread")]
        async fn relationship_filter_open_ended_hop_reaches_every_depth() {
            let (svc, _tmp) = make_test_service().await;
            seed_project_tree(&svc).await;

            let under_task = matching_ids(
                &svc,
                json!({
                    "target_type": "text",
                    "filters": [{
                        "type": "relationship", "operator": "equals",
                        "path": [{ "name": "child_of", "open_ended": true }],
                        "node_id": ACTIVE_TASK
                    }]
                }),
            )
            .await;
            assert_eq!(under_task, [NOTE_UNDER_ACTIVE_TASK, NOTE_UNDER_NOTE]);

            // The fixed hop reaches the direct child only.
            let direct = matching_ids(
                &svc,
                json!({
                    "target_type": "text",
                    "filters": [{
                        "type": "relationship", "operator": "equals",
                        "path": ["child_of"], "node_id": ACTIVE_TASK
                    }]
                }),
            )
            .await;
            assert_eq!(direct, [NOTE_UNDER_ACTIVE_TASK]);
        }

        /// A query's type selects its subtypes too (ADR-078).
        #[tokio::test(flavor = "multi_thread")]
        async fn a_target_type_matches_its_subtypes() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({"name": "st_ticket", "fields": [{"name": "state", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({"name": "st_bug", "extends": "st_ticket", "fields": []}),
            )
            .await;
            svc.create_node(node(
                "a2000000-0000-4000-8000-000000000001",
                "st_ticket",
                json!({"state": "open"}),
            ))
            .await
            .unwrap();
            // The inherited field is written on the subtype's node and stored
            // in the base type's bucket, where the base-scoped filter reads it.
            svc.create_node(node(
                "a2000000-0000-4000-8000-000000000002",
                "st_bug",
                json!({"state": "open"}),
            ))
            .await
            .unwrap();

            let tickets = matching_ids(
                &svc,
                json!({
                    "target_type": "st_ticket",
                    "filters": [{
                        "type": "property", "operator": "equals",
                        "property": "state", "value": "open"
                    }]
                }),
            )
            .await;
            assert_eq!(
                tickets,
                [
                    "a2000000-0000-4000-8000-000000000001",
                    "a2000000-0000-4000-8000-000000000002"
                ]
            );

            let bugs = matching_ids(&svc, json!({ "target_type": "st_bug", "filters": [] })).await;
            assert_eq!(bugs, ["a2000000-0000-4000-8000-000000000002"]);
        }

        /// A base-type query sorts a subtype's row by the value it holds.
        /// The inherited field lives in the base type's bucket on a subtype's
        /// node, so a sort that read each row's own-type bucket would find no
        /// value on the subtype row and put it first.
        #[tokio::test(flavor = "multi_thread")]
        async fn sorting_a_base_type_query_orders_subtype_rows_by_their_value() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({"name": "so_ticket", "fields": [{"name": "rank", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({"name": "so_bug", "extends": "so_ticket", "fields": []}),
            )
            .await;
            for (id, node_type, rank) in [
                ("a3000000-0000-4000-8000-000000000001", "so_ticket", "a"),
                ("a3000000-0000-4000-8000-000000000002", "so_bug", "m"),
                ("a3000000-0000-4000-8000-000000000003", "so_ticket", "z"),
            ] {
                svc.create_node(node(id, node_type, json!({ "rank": rank })))
                    .await
                    .unwrap();
            }

            let ranks = |direction: &'static str| {
                let svc = Arc::clone(&svc);
                async move {
                    let input: ExecuteQueryInput = serde_json::from_value(json!({
                        "target_type": "so_ticket",
                        "filters": [],
                        "sorting": [{ "field": "rank", "direction": direction }]
                    }))
                    .unwrap();
                    execute_query_nodes(&svc, input)
                        .await
                        .unwrap()
                        .iter()
                        .map(|n| {
                            n.properties["so_ticket"]["rank"]
                                .as_str()
                                .unwrap()
                                .to_string()
                        })
                        .collect::<Vec<_>>()
                }
            };

            assert_eq!(ranks("asc").await, ["a", "m", "z"]);
            assert_eq!(ranks("desc").await, ["z", "m", "a"]);
        }
    }
}
