use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::helpers::{deserialize_clearable, is_active_lifecycle};
use crate::node::{Node, ValidationError};

/// The `node_type` of every saved query.
pub const QUERY_NODE_TYPE: &str = "query";

/// `target_type` of a query that names none: every type. Matches the query
/// schema's declared default.
pub const ALL_TYPES_TARGET: &str = "*";

// ============================================================================
// Filter and sort vocabulary
//
// Shared by the stored query (`QueryNode`) and the execution struct
// (`nodespace_core::services::QueryDefinition`), which is why it lives here
// rather than in the query service: a saved query's filters decode straight
// into the types the query service executes.
// ============================================================================

/// Filter type category
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FilterType {
    #[default]
    Property,
    Content,
    Relationship,
    Metadata,
    /// Filter by a *related* node's own properties — "tasks belonging to a
    /// project with status active" — rather than bare membership in a fixed
    /// relationship shape. See [`QueryFilter::relationship_name`] and
    /// [`QueryFilter::filter`].
    Related,
}

/// Comparison operator for filters
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FilterOperator {
    #[default]
    Equals,
    Contains,
    #[serde(rename = "gt")]
    GreaterThan,
    #[serde(rename = "lt")]
    LessThan,
    #[serde(rename = "gte")]
    GreaterThanOrEqual,
    #[serde(rename = "lte")]
    LessThanOrEqual,
    In,
    Exists,
}

/// Relationship type for graph traversal
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipType {
    Parent,
    Children,
    Mentions,
    #[serde(rename = "mentioned_by")]
    MentionedBy,
}

/// Sort direction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    #[serde(rename = "asc")]
    Ascending,
    #[serde(rename = "desc")]
    Descending,
}

/// Individual filter condition
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueryFilter {
    /// Filter category
    #[serde(rename = "type")]
    pub filter_type: FilterType,
    /// Comparison operator
    pub operator: FilterOperator,
    /// Property key for property filters
    pub property: Option<String>,
    /// Expected value
    pub value: Option<serde_json::Value>,
    /// Case sensitivity for text comparisons
    pub case_sensitive: Option<bool>,
    /// Relationship type for relationship filters
    pub relationship_type: Option<RelationshipType>,
    /// Target node ID for relationship filters
    pub node_id: Option<String>,
    /// Relationship name for a [`FilterType::Related`] filter, exactly as the
    /// caller supplied it — a schema-declared name (forward or reverse) or a
    /// built-in structural name. This is the caller-facing identity of the
    /// relationship; [`Self::resolved_relationship`] carries what it resolves
    /// to and is what SQL compilation actually reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relationship_name: Option<String>,
    /// The nested filter a [`FilterType::Related`] filter evaluates against
    /// the related node(s) reached by [`Self::relationship_name`]. Recursive
    /// by construction, but validated by the query service to at most one
    /// level of `Related` nesting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<Box<QueryFilter>>,
    /// How [`Self::relationship_name`] resolved against the query's
    /// `target_type` — the stored `relationship_type`, which side of the edge
    /// the target type sits on, and (for a reverse match) the declaring
    /// type to narrow by. Resolving a name is an async schema lookup, so it
    /// happens once, ahead of SQL compilation, in core's
    /// `query_ops::to_query_definition` — not a caller-facing field (never
    /// serialized, and so never stored on a saved query), but public so a
    /// caller constructing a `QueryDefinition` directly can populate it
    /// without a `NodeService` in hand.
    #[serde(skip)]
    pub resolved_relationship: Option<ResolvedRelationship>,
}

/// Compiled form of a [`FilterType::Related`] filter's relationship name —
/// what the query service's SQL builder needs to compile the join, with no
/// schema lookup of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRelationship {
    /// The `relationship_type` value actually stored in the `relationship`
    /// table — the forward name, whichever end declared it.
    pub stored_type: String,
    /// Which column of the `relationship` row the OUTER (filtered) node
    /// occupies: `true` when the outer node is `in_node` (a forward
    /// traversal, or an inbound-forward walked backwards from the target's
    /// end) and the related node is therefore `out_node`; `false` when the
    /// outer node is `out_node` (a reverse-name traversal) and the related
    /// node is `in_node`.
    pub outer_is_in_node: bool,
    /// For a reverse match, the type that declared the forward relationship —
    /// narrows the related-node set to that type's `extends` descendants,
    /// the same narrowing core's `rel_ops::get_related_nodes` applies.
    /// `None` for a built-in reverse (no declaring schema) or a forward/
    /// inbound-forward match (already unambiguous by construction).
    pub source_type: Option<String>,
    /// The node type the nested filter's own property paths resolve
    /// against — the related node's declared type, when the schema names
    /// one. `None` when the related side has no single declared type (a
    /// built-in, or an untyped declaration), in which case the nested
    /// filter's property access falls back to the same per-row
    /// `'$.' || node_type || '.<field>'` path a wildcard top-level query
    /// uses, since the joined rows may span more than one type.
    pub related_type: Option<String>,
}

/// Sorting configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SortConfig {
    /// Property or field to sort by
    pub field: String,
    /// Sort direction
    pub direction: SortDirection,
}

/// Who created a saved query — the query schema's `generated_by` enum, which
/// is not user-extensible.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QueryGeneratedBy {
    Ai,
    #[default]
    User,
}

// ============================================================================
// QueryFields — the query schema's fields
// ============================================================================

/// The query schema's fields, decoded from a query node's properties.
///
/// This is the only reader of a stored query: storage keys are the schema's
/// snake_case field names (`target_type`, `view_config`, …), hoisted by the
/// store under `properties.query.*`. [`Self::from_properties`] reads that
/// bucket, or the flat shape a node built in memory or a create payload
/// carries. On the wire the same fields travel camelCase at the top level of
/// a [`QueryNode`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryFields {
    /// The node type the query selects, or [`ALL_TYPES_TARGET`].
    pub target_type: String,
    pub filters: Vec<QueryFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sorting: Option<Vec<SortConfig>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    pub generated_by: QueryGeneratedBy,
    /// Parent chat id for an AI-generated query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generator_context: Option<String>,
    /// System-managed.
    pub execution_count: u64,
    /// System-managed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_executed: Option<String>,
    /// How the query renders. Its keys (`lastView`, `kanban.groupBy`) are the
    /// viewer's own vocabulary, not schema field names, so the object is
    /// carried as-is rather than typed here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_config: Option<Value>,
}

impl QueryFields {
    /// Decode a query node's fields.
    ///
    /// # Errors
    ///
    /// `InvalidNodeType` if `node` is not a query, `InvalidProperties` if a
    /// field is present with the wrong shape (see [`Self::from_properties`]).
    pub fn from_node(node: &Node) -> Result<Self, ValidationError> {
        if node.node_type != QUERY_NODE_TYPE {
            return Err(ValidationError::InvalidNodeType(format!(
                "Expected '{QUERY_NODE_TYPE}', got '{}'",
                node.node_type
            )));
        }
        Self::from_properties(&node.properties)
    }

    /// Decode from properties in either the hoisted (`properties.query.*`)
    /// or flat shape, preferring the `query` bucket when there is one.
    ///
    /// An absent or `null` field takes the schema's default: all types, no
    /// filters, `user`, an execution count of 0; the optional fields stay
    /// unset. Keys the schema does not declare (extension fields) are
    /// ignored.
    ///
    /// # Errors
    ///
    /// `InvalidProperties` naming the field, if one is present with the wrong
    /// shape — a filter that is not a `QueryFilter`, or a `view_config` that
    /// is not an object.
    pub fn from_properties(properties: &Value) -> Result<Self, ValidationError> {
        let bucket = properties
            .get(QUERY_NODE_TYPE)
            .filter(|b| b.is_object())
            .unwrap_or(properties);
        let field = |key: &str| bucket.get(key).filter(|v| !v.is_null());

        let view_config = field("view_config").cloned();
        if view_config.as_ref().is_some_and(|v| !v.is_object()) {
            return Err(invalid("view_config", "expected an object"));
        }

        Ok(Self {
            target_type: decode(field("target_type"), "target_type")?
                .unwrap_or_else(|| ALL_TYPES_TARGET.to_string()),
            filters: decode(field("filters"), "filters")?.unwrap_or_default(),
            sorting: decode(field("sorting"), "sorting")?,
            limit: decode(field("limit"), "limit")?,
            generated_by: decode(field("generated_by"), "generated_by")?.unwrap_or_default(),
            generator_context: decode(field("generator_context"), "generator_context")?,
            execution_count: decode(field("execution_count"), "execution_count")?.unwrap_or(0),
            last_executed: decode(field("last_executed"), "last_executed")?,
            view_config,
        })
    }
}

fn decode<T: serde::de::DeserializeOwned>(
    value: Option<&Value>,
    key: &str,
) -> Result<Option<T>, ValidationError> {
    value
        .map(|v| serde_json::from_value(v.clone()).map_err(|e| invalid(key, &e.to_string())))
        .transpose()
}

fn invalid(key: &str, detail: &str) -> ValidationError {
    ValidationError::InvalidProperties(format!("query field '{key}': {detail}"))
}

// ============================================================================
// Wire shape and typed update
// ============================================================================

/// Wire shape for query nodes sent to the frontend.
///
/// Produced by `node_to_typed_value` for `node_type == "query"`: the query
/// schema's fields are promoted to the top level (camelCase, see
/// [`QueryFields`]) and `properties` keeps only extension fields. Maps
/// directly to the TypeScript `QueryNode` interface.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryNode {
    pub id: String,
    #[serde(rename = "nodeType")]
    pub node_type: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub properties: Value,
    #[serde(default, skip_serializing_if = "is_active_lifecycle")]
    pub lifecycle_status: String,
    #[serde(flatten)]
    pub fields: QueryFields,
}

/// Partial update for a query's fields, received from the frontend.
///
/// `target_type`, `filters` and `generated_by` have no clear path (the
/// schema requires them); the other fields are tri-state: absent leaves the
/// field unchanged, `null` clears it, and a value sets it. `view_config` is
/// replaced whole, never merged. The system-managed `execution_count` and
/// `last_executed` are not client-writable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryNodeUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<QueryFilter>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub sorting: Option<Option<Vec<SortConfig>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub limit: Option<Option<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<QueryGeneratedBy>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub generator_context: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_clearable"
    )]
    pub view_config: Option<Option<Value>>,
}

impl QueryNodeUpdate {
    /// True when the update changes nothing.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The flat, snake_case properties patch this update writes
    /// (`{"view_config": {…}}`); a cleared field is written as `null`. The
    /// service layer moves the keys into the `query` storage bucket.
    ///
    /// Also the shape a query is *created* with: a create payload carries the
    /// same storage keys, so building one from an update keeps a single
    /// writer of the stored names.
    pub fn to_properties_patch(&self) -> Value {
        let mut patch = Map::new();
        let mut put = |key: &str, value: Value| {
            patch.insert(key.to_string(), value);
        };
        if let Some(target_type) = &self.target_type {
            put("target_type", Value::String(target_type.clone()));
        }
        if let Some(filters) = &self.filters {
            put("filters", to_json(filters));
        }
        if let Some(sorting) = &self.sorting {
            put("sorting", to_json(sorting));
        }
        if let Some(limit) = &self.limit {
            put("limit", to_json(limit));
        }
        if let Some(generated_by) = &self.generated_by {
            put("generated_by", to_json(generated_by));
        }
        if let Some(generator_context) = &self.generator_context {
            put("generator_context", to_json(generator_context));
        }
        if let Some(view_config) = &self.view_config {
            put("view_config", to_json(view_config));
        }
        Value::Object(patch)
    }
}

/// Serialize a value whose `Serialize` cannot fail (plain data, string keys).
fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("query field types always serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_the_hoisted_bucket_with_schema_defaults() {
        let fields = QueryFields::from_properties(&json!({
            "query": {
                "target_type": "task",
                "filters": [
                    { "type": "property", "operator": "equals", "property": "status", "value": "open" }
                ],
                "view_config": { "lastView": "kanban", "kanban": { "groupBy": "status" } },
                "custom:owner": "ada"
            }
        }))
        .unwrap();

        assert_eq!(fields.target_type, "task");
        assert_eq!(fields.filters.len(), 1);
        assert_eq!(fields.filters[0].filter_type, FilterType::Property);
        assert_eq!(fields.generated_by, QueryGeneratedBy::User);
        assert_eq!(fields.execution_count, 0);
        assert_eq!(fields.sorting, None);
        assert_eq!(fields.view_config.unwrap()["kanban"]["groupBy"], "status");
    }

    #[test]
    fn an_empty_query_targets_every_type() {
        let fields = QueryFields::from_properties(&json!({})).unwrap();
        assert_eq!(fields.target_type, ALL_TYPES_TARGET);
        assert!(fields.filters.is_empty());
    }

    #[test]
    fn a_malformed_field_is_rejected_by_name() {
        let err = QueryFields::from_properties(&json!({
            "filters": [{ "type": "nonsense", "operator": "equals" }]
        }))
        .unwrap_err();
        assert!(err.to_string().contains("'filters'"), "{err}");

        let err = QueryFields::from_properties(&json!({ "view_config": "kanban" })).unwrap_err();
        assert!(err.to_string().contains("'view_config'"), "{err}");
    }

    #[test]
    fn update_null_clears_and_absent_leaves_alone() {
        let update: QueryNodeUpdate = serde_json::from_value(json!({
            "viewConfig": { "lastView": "list" },
            "sorting": null,
            "limit": 25
        }))
        .unwrap();
        assert_eq!(update.sorting, Some(None));
        assert_eq!(update.limit, Some(Some(25)));
        assert_eq!(update.filters, None);

        assert_eq!(
            update.to_properties_patch(),
            json!({ "view_config": { "lastView": "list" }, "sorting": null, "limit": 25 })
        );
    }

    #[test]
    fn update_rejects_a_camel_case_storage_key_or_system_field() {
        for body in [json!({ "target_type": "task" }), json!({ "executionCount": 3 })] {
            assert!(
                serde_json::from_value::<QueryNodeUpdate>(body.clone()).is_err(),
                "{body} must be rejected"
            );
        }
    }

    #[test]
    fn empty_update_is_empty() {
        assert!(QueryNodeUpdate::default().is_empty());
        assert!(!QueryNodeUpdate {
            limit: Some(None),
            ..Default::default()
        }
        .is_empty());
    }
}
