//! Execute-query ops wrapper.
//!
//! Bridges agent tool calls to `QueryService`. The agent passes a
//! flat-property filter shape; this module maps it to `QueryDefinition`
//! and delegates to `QueryService::execute`.

use crate::models::Node;
use crate::ops::rel_ops::{resolve_relationship_name_for_type, ResolvedRelName};
use crate::ops::OpsError;
use crate::services::node_service::NodeService;
use crate::services::query_service::{
    FilterOperator, FilterType, QueryDefinition, QueryFilter, QueryService, ResolvedRelationship,
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
/// infers it: a filter naming a `relationship_type` or an anchor `node_id` is a
/// relationship filter, and anything naming a `property` is a property filter —
/// the only two shapes the omission is observed for. An item that names none of
/// them is genuinely under-specified and still errors, so the inference never
/// has to guess between `content` and `metadata`.
#[derive(Debug, Default, Deserialize)]
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
    /// Relationship name for a related-node filter (`type: "related"`) — a
    /// schema-declared name (forward or reverse) or a built-in structural
    /// name, resolved against the enclosing query's `target_type` at
    /// conversion time.
    #[serde(default)]
    pub relationship_name: Option<String>,
    /// The nested filter a related-node filter evaluates against the
    /// related node(s). Recursive by construction; validated to at most one
    /// level of `related` nesting (see
    /// `query_service::validate_filter_identifiers`/`MAX_RELATED_DEPTH`).
    #[serde(default)]
    pub filter: Option<Box<AgentFilterItem>>,
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
        // Checked before `relationship_type`/`node_id`: a filter naming
        // `relationship_name` is unambiguously the recursive related-node
        // shape, never the closed-enum bare-membership one, so there is no
        // precedence question between the two the way there is between
        // `property` and `content`/`metadata` below.
        if self.relationship_name.is_some() || self.filter.is_some() {
            return Ok("related");
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

/// Convert one agent filter item to its `QueryService` shape, resolving a
/// [`FilterType::Related`] filter's `relationshipName` against `target_type`
/// along the way.
///
/// `target_type` is the type the *enclosing* query or `Related` filter
/// evaluates the filter against — the root query's own `target_type` for a
/// top-level filter, or the outer filter's resolved `related_type` for a
/// nested one — since that is what a relationship name is resolved relative
/// to (`resolve_relationship_name_for_type`'s `node_type` argument). `"*"`
/// (wildcard) cannot resolve a `Related` filter: there is no single schema to
/// resolve `relationship_name` against, so a `Related` filter under a
/// wildcard query errors rather than guessing which type's declaration
/// applies.
///
/// Async because resolving `relationshipName` is a schema lookup
/// (`resolve_relationships`/`get_inbound_relationships`, both async) — the
/// one reason this whole conversion path, and `to_query_definition` above it,
/// is no longer synchronous.
///
/// Explicitly boxed (`Pin<Box<dyn Future>>`) rather than a plain `async fn`:
/// this function calls itself for a `Related` filter's nested item, and a
/// self-recursive `async fn` is an infinitely-sized future type by
/// construction — boxing is what gives the recursive call a fixed size,
/// exactly as `QueryFilter::validate_identifiers`' depth cap keeps the
/// recursion itself finite at runtime.
fn to_query_filter<'a>(
    node_service: &'a Arc<NodeService>,
    target_type: &'a str,
    item: AgentFilterItem,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<QueryFilter, OpsError>> + Send + 'a>>
{
    Box::pin(to_query_filter_inner(node_service, target_type, item))
}

async fn to_query_filter_inner(
    node_service: &Arc<NodeService>,
    target_type: &str,
    item: AgentFilterItem,
) -> Result<QueryFilter, OpsError> {
    let filter_type = parse_filter_type(item.category()?)?;
    let operator = parse_filter_operator(&item.operator)?;

    let relationship_type = match &item.relationship_type {
        Some(rt) => Some(parse_relationship_type(rt)?),
        None => None,
    };

    let (relationship_name, filter, resolved_relationship) = if filter_type == FilterType::Related {
        let relationship_name = item.relationship_name.clone().ok_or_else(|| {
            OpsError::InvalidParams("Related filter missing 'relationshipName'".to_string())
        })?;
        let nested_item = *item.filter.ok_or_else(|| {
            OpsError::InvalidParams("Related filter missing 'filter'".to_string())
        })?;

        if target_type == "*" {
            return Err(OpsError::InvalidParams(format!(
                "Related filter on relationship '{relationship_name}' cannot be resolved under \
                 a wildcard ('*') target_type — a related-node filter is resolved against one \
                 specific schema, and a wildcard query has none. Scope the query to a concrete \
                 target_type to use a related-node filter."
            )));
        }

        let resolved =
            resolve_relationship_name_for_type(node_service, target_type, &relationship_name)
                .await
                .map_err(|e| match e {
                    OpsError::InvalidParams(msg) => OpsError::InvalidParams(format!(
                        "Related filter's relationshipName '{relationship_name}' could not be \
                 resolved against '{target_type}': {msg}"
                    )),
                    other => other,
                })?;

        let (stored_type, outer_is_in_node, source_type, related_type) = match &resolved {
            // `resolve_relationship_name_for_type` only returns `Builtin`
            // when `relationship_name` IS one of `BUILTIN_RELATIONSHIP_NAMES`
            // — the name itself is already the stored `relationship_type`.
            ResolvedRelName::Builtin => (relationship_name.clone(), true, None, None),
            ResolvedRelName::Forward => {
                let (rels, _owners) = node_service
                    .resolve_relationships(target_type)
                    .await
                    .map_err(|e| {
                        OpsError::Internal(format!("Failed to resolve relationships: {e}"))
                    })?;
                let declared = rels.iter().find(|r| r.name == relationship_name);
                let related_type = declared.and_then(|r| r.target_type.clone());
                (relationship_name.clone(), true, None, related_type)
            }
            ResolvedRelName::InboundForward => {
                // The outer node sits at the target end of ANOTHER schema's
                // forward declaration — the related type is that declaring
                // schema's own type, the source side of the edge. Resolved
                // here (rather than left `None`, falling back to the
                // per-row `node_type` wildcard path) because, unlike a
                // reverse match, more than one schema may declare the same
                // forward name toward this type — see
                // `rel_ops::get_related_nodes`'s doc comment on why
                // `InboundForward` is deliberately NOT narrowed the way
                // `Reverse` is. Not narrowing here would let the nested
                // filter's property lookup silently read the wrong
                // schema's field when two declarers share both a name and a
                // property key, so this looks up the first declarer by
                // name — the same "first match wins" precedence
                // `resolve_relationship_name` itself uses for an untyped
                // declaration.
                let inbound = node_service
                    .get_inbound_relationships(target_type)
                    .await
                    .map_err(|e| {
                        OpsError::Internal(format!("Failed to resolve inbound relationships: {e}"))
                    })?;
                let related_type = inbound
                    .iter()
                    .find(|(_, rel)| rel.name == relationship_name)
                    .map(|(source_type, _)| source_type.clone());
                (relationship_name.clone(), false, None, related_type)
            }
            ResolvedRelName::Reverse {
                forward_name,
                source_type,
            } => (
                forward_name.clone(),
                false,
                source_type.clone(),
                source_type.clone(),
            ),
        };

        let nested = Box::new(
            to_query_filter(
                node_service,
                related_type.as_deref().unwrap_or(target_type),
                nested_item,
            )
            .await?,
        );

        (
            Some(relationship_name),
            Some(nested),
            Some(ResolvedRelationship {
                stored_type,
                outer_is_in_node,
                source_type,
                related_type,
            }),
        )
    } else {
        (None, None, None)
    };

    Ok(QueryFilter {
        filter_type,
        operator,
        property: item.property,
        value: item.value,
        case_sensitive: item.case_sensitive,
        relationship_type,
        node_id: item.node_id,
        relationship_name,
        filter,
        resolved_relationship,
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
/// Async because a [`FilterType::Related`] filter's `relationshipName` is
/// resolved here, against `input.target_type`, via
/// [`resolve_relationship_name_for_type`] — a schema lookup — before the
/// resulting [`QueryDefinition`] ever reaches [`QueryService`]'s (synchronous)
/// SQL compilation. See [`to_query_filter`].
async fn to_query_definition(
    node_service: &Arc<NodeService>,
    input: ExecuteQueryInput,
) -> Result<QueryDefinition, OpsError> {
    let limit = input.limit.unwrap_or(50);

    // Sequential, not `join_all`: filter count is small (a handful per
    // query), and each conversion is at most a couple of schema lookups, so
    // the concurrency isn't worth the added complexity here.
    let mut filters: Vec<QueryFilter> = Vec::with_capacity(input.filters.len());
    for item in input.filters {
        filters.push(to_query_filter(node_service, &input.target_type, item).await?);
    }

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

    /// A bare `NodeService` for tests that call [`to_query_filter`] with a
    /// non-`Related` filter — the resolver it would otherwise need
    /// (`resolve_relationship_name_for_type`) is never reached for those, but
    /// the function always takes a `node_service` argument.
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

    #[tokio::test(flavor = "multi_thread")]
    async fn to_query_filter_property() {
        let (svc, _tmp) = make_test_service().await;
        let item = AgentFilterItem {
            filter_type: Some("property".to_string()),
            operator: "equals".to_string(),
            property: Some("status".to_string()),
            value: Some(json!("open")),
            ..Default::default()
        };
        let qf = to_query_filter(&svc, "task", item).await.unwrap();
        assert_eq!(qf.filter_type, FilterType::Property);
        assert_eq!(qf.operator, FilterOperator::Equals);
        assert_eq!(qf.property.as_deref(), Some("status"));
        assert_eq!(qf.value, Some(json!("open")));
    }

    /// A filter that names a property but omits `type` is complete enough to
    /// run: the category is derivable, and rejecting it turns a correct query
    /// into a tool error over a token the model gains nothing by restating.
    #[tokio::test(flavor = "multi_thread")]
    async fn filter_type_is_inferred_for_a_property_filter() {
        let (svc, _tmp) = make_test_service().await;
        let item: AgentFilterItem = serde_json::from_value(json!({
            "operator": "equals",
            "property": "replacement_cost",
            "value": 2400
        }))
        .expect("`type` must be optional on the wire");
        assert_eq!(item.category().unwrap(), "property");
        let qf = to_query_filter(&svc, "task", item).await.unwrap();
        assert_eq!(qf.filter_type, FilterType::Property);
        assert_eq!(qf.property.as_deref(), Some("replacement_cost"));
    }

    /// A filter naming `property: "content"` with no explicit `type`
    /// must infer the CONTENT category. Content lives in the top-level SQL `content`
    /// column, so routing it to the property path builds a `json_extract(properties,
    /// …)` that is structurally always NULL and silently returns zero results (an
    /// existing "Buy cereal" task returns `count: 0`). A non-content property is
    /// unaffected and still infers the property category.
    #[tokio::test(flavor = "multi_thread")]
    async fn content_property_infers_the_content_filter_not_json_extract() {
        let (svc, _tmp) = make_test_service().await;
        let item: AgentFilterItem = serde_json::from_value(json!({
            "operator": "contains",
            "property": "content",
            "value": "cereal"
        }))
        .expect("`type` must be optional on the wire");
        assert_eq!(item.category().unwrap(), "content");
        assert_eq!(
            to_query_filter(&svc, "task", item)
                .await
                .unwrap()
                .filter_type,
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
            to_query_filter(&svc, "task", prop)
                .await
                .unwrap()
                .filter_type,
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
    #[tokio::test(flavor = "multi_thread")]
    async fn node_type_in_the_category_slot_falls_through_to_inference() {
        let (svc, _tmp) = make_test_service().await;
        let item: AgentFilterItem = serde_json::from_value(json!({
            "type": "task",
            "operator": "equals",
            "property": "status",
            "value": "open"
        }))
        .unwrap();
        assert_eq!(item.category().unwrap(), "property");
        assert!(to_query_filter(&svc, "task", item).await.is_ok());
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
    #[tokio::test(flavor = "multi_thread")]
    async fn filter_with_nothing_to_infer_from_still_errors() {
        let (svc, _tmp) = make_test_service().await;
        let item: AgentFilterItem = serde_json::from_value(json!({
            "operator": "exists",
            "value": true
        }))
        .unwrap();
        assert!(item.category().is_err());
        assert!(to_query_filter(&svc, "task", item).await.is_err());
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
                json!({"name": "rf_project", "fields": [{"name": "status", "type": "string"}]}),
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
                "proj-active",
                "rf_project",
                json!({"status": "active"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "proj-archived",
                "rf_project",
                json!({"status": "archived"}),
            ))
            .await
            .unwrap();
            svc.create_node(node("task-a", "rf_task", json!({})))
                .await
                .unwrap();
            svc.create_node(node("task-b", "rf_task", json!({})))
                .await
                .unwrap();
            svc.create_relationship("task-a", "project", "proj-active", json!({}))
                .await
                .unwrap();
            svc.create_relationship("task-b", "project", "proj-archived", json!({}))
                .await
                .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf_task",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "relationship_name": "project",
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
                Some("task-a")
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
                    "fields": [{"name": "severity", "type": "string"}],
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

            svc.create_node(node("sprint-hot", "rf2_sprint", json!({})))
                .await
                .unwrap();
            svc.create_node(node("sprint-cold", "rf2_sprint", json!({})))
                .await
                .unwrap();
            svc.create_node(node(
                "t-critical",
                "rf2_task",
                json!({"severity": "critical"}),
            ))
            .await
            .unwrap();
            svc.create_node(node("t-minor", "rf2_task", json!({"severity": "minor"})))
                .await
                .unwrap();
            svc.create_node(node("t-minor-2", "rf2_task", json!({"severity": "minor"})))
                .await
                .unwrap();
            // sprint-hot has both a critical and a minor task -- still one match.
            // The forward declaration (`sprint`) lives on `rf2_task`, so the
            // edge is created from the task's end, naming the forward name --
            // `tasks` is the reverse spelling the query filter below uses.
            svc.create_relationship("t-critical", "sprint", "sprint-hot", json!({}))
                .await
                .unwrap();
            svc.create_relationship("t-minor", "sprint", "sprint-hot", json!({}))
                .await
                .unwrap();
            svc.create_relationship("t-minor-2", "sprint", "sprint-cold", json!({}))
                .await
                .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf2_sprint",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "relationship_name": "tasks",
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
                Some("sprint-hot")
            );
        }

        /// A built-in structural relationship (`has_child`) as the Related
        /// filter's relationship_name -- "text nodes whose child task has
        /// status open".
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_builtin_has_child() {
            let (svc, _tmp) = make_test_service().await;

            svc.create_node(node("parent-a", "text", json!({})))
                .await
                .unwrap();
            svc.create_node(node("parent-b", "text", json!({})))
                .await
                .unwrap();
            svc.create_node(task_node("child-open", "open", None))
                .await
                .unwrap();
            svc.create_node(task_node("child-done", "done", None))
                .await
                .unwrap();
            svc.create_relationship("parent-a", "has_child", "child-open", json!({}))
                .await
                .unwrap();
            svc.create_relationship("parent-b", "has_child", "child-done", json!({}))
                .await
                .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "text",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "relationship_name": "has_child",
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
                Some("parent-a")
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
                json!({"name": "rf3_epic", "fields": [{"name": "status", "type": "string"}]}),
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

            svc.create_node(node("epic-active", "rf3_epic", json!({"status": "active"})))
                .await
                .unwrap();
            svc.create_node(node("story-1", "rf3_story", json!({})))
                .await
                .unwrap();
            svc.create_relationship("story-1", "epic", "epic-active", json!({}))
                .await
                .expect("create_relationship must succeed for an inherited relationship");

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf3_story",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "relationship_name": "epic",
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
                    "relationship_name": "project",
                    "filter": {
                        "type": "related",
                        "operator": "equals",
                        "relationship_name": "owner",
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
                    "relationship_name": "not_a_real_relationship",
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

        /// A Related filter cannot be resolved under a wildcard target_type
        /// -- there is no single schema to resolve relationship_name
        /// against, so this errors rather than guessing.
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_under_wildcard_target_type_errors() {
            let (svc, _tmp) = make_test_service().await;

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "*",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "relationship_name": "has_child",
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

        /// The forward name of an inbound relationship, walked backwards
        /// (`ResolvedRelName::InboundForward`) -- a distinct resolution
        /// branch from `Forward`/`Reverse`/`Builtin`, exercised here with
        /// the forward name itself rather than its `reverseName` spelling.
        #[tokio::test(flavor = "multi_thread")]
        async fn related_filter_inbound_forward_relationship() {
            let (svc, _tmp) = make_test_service().await;

            create_schema(
                &svc,
                json!({"name": "rf5_project", "fields": [{"name": "status", "type": "string"}]}),
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
                "rf5-proj-active",
                "rf5_project",
                json!({"status": "active"}),
            ))
            .await
            .unwrap();
            svc.create_node(node("rf5-task-a", "rf5_task", json!({})))
                .await
                .unwrap();
            svc.create_relationship("rf5-task-a", "project", "rf5-proj-active", json!({}))
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
                    "relationship_name": "project",
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

            svc.create_node(task_node("parent-task", "open", None))
                .await
                .unwrap();
            svc.create_node(task_node("child-task", "open", None))
                .await
                .unwrap();
            svc.create_relationship("parent-task", "has_child", "child-task", json!({}))
                .await
                .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "task",
                "filters": [{
                    "type": "relationship",
                    "operator": "equals",
                    "relationship_type": "parent",
                    "node_id": "child-task"
                }]
            }))
            .unwrap();

            let output = execute_query(&svc, input).await.unwrap();
            assert_eq!(output.count, 1);
            assert_eq!(
                output.nodes[0].get("id").and_then(|v| v.as_str()),
                Some("parent-task")
            );
        }
    }
}
