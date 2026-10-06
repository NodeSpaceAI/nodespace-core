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
    EnumRank, FilterOperator, FilterType, PropertyScope, QueryDefinition, QueryFilter,
    QueryService, RelationshipPath, RelativeDate, SortConfig, SortDirection, SubtypeBucket,
    NODE_COLUMNS,
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
    /// Filter category: "property", "content", "relationship", "metadata",
    /// "related", "permitted". Optional — see the type-level note; omitted
    /// values are inferred from the other fields rather than rejected. A
    /// "permitted" filter is never inferred: it names a `property` and a
    /// `value` like a property filter, and means a different question.
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
    /// A date relative to the day the query runs, in place of `value`, for a
    /// property filter on a date field: `{"anchor": "today"}`, optionally
    /// with `"offset_days": N`.
    #[serde(default)]
    pub relative_date: Option<RelativeDate>,
    /// Case sensitivity for text comparisons (default: true).
    #[serde(default)]
    pub case_sensitive: Option<bool>,
    /// Negate the filter: keep the nodes its condition does not hold for.
    /// On a related-node filter, keep the nodes whose `path` reaches no node
    /// matching the nested `filter`.
    #[serde(default)]
    pub negate: Option<bool>,
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
        "permitted" => Ok(FilterType::Permitted),
        other => Err(OpsError::InvalidParams(format!(
            "Unknown filter type '{}'. Supported: property, content, relationship, metadata, \
             related, permitted",
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

/// Whether `s` names one of the filter categories.
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
        relative_date: item.relative_date,
        case_sensitive: item.case_sensitive,
        negate: item.negate,
        node_id: item.node_id,
        path: item.path.filter(|_| walks),
        filter,
        resolved_path: None,
        property_scope: PropertyScope::default(),
    })
}

/// Resolve a property filter's `property` or a sort's `field` against the
/// fields `target_type` declares, its inherited ones included. A name that is
/// a path into an object field's value (`repository.url`) is checked segment
/// by segment. A plain field name passes unchecked, as it always has: an
/// undeclared one matches nothing.
///
/// Every segment but the last must be an object field, and each must be
/// declared in the one before it. A link field is followed by one of its two
/// parts, `title` or `url`, which ends the path. A wildcard query has no schema to check
/// against, so a path is refused there.
///
/// Returns where the rows of a query for `target_type` keep the field: an
/// inherited field is stored under the schema that declares it, on a node of
/// any type in the chain, and a subtype that declares the field again keeps
/// it in its own bucket (see [`subtype_buckets`]).
async fn resolve_property(
    node_service: &NodeService,
    declared_fields: &mut DeclaredFields,
    target_type: &str,
    name: &str,
    label: &str,
) -> Result<PropertyScope, OpsError> {
    let segments = nodespace_types::property_segments(name);
    if target_type == "*" {
        if segments.len() < 2 {
            return Ok(PropertyScope::default());
        }
        return Err(OpsError::InvalidParams(format!(
            "{label} '{name}' is a path into a field's value, which is checked against a \
             type's schema: name the type the query selects instead of every type"
        )));
    }
    let TypeFields { fields, owners } = declared_fields.of(node_service, target_type).await?;
    let bucket = owners
        .get(segments[0])
        .filter(|owner| owner.as_str() != target_type)
        .cloned();
    if segments.len() < 2 {
        // An enum has no path into it, so a path has no subtype buckets.
        let subtypes = subtype_buckets(node_service, declared_fields, target_type, name).await?;
        return Ok(PropertyScope { bucket, subtypes });
    }

    let mut declared: &[crate::models::SchemaField] = fields;
    for (index, segment) in segments.iter().enumerate() {
        let reached = segments[..=index].join(".");
        let Some(field) = declared.iter().find(|field| field.name == *segment) else {
            return Err(OpsError::InvalidParams(format!(
                "{label} '{name}': the '{target_type}' schema declares no field '{reached}'"
            )));
        };
        if index + 1 == segments.len() {
            break;
        }
        declared = match (&field.field_type, field.fields.as_deref()) {
            (crate::models::SchemaFieldType::Object, Some(nested)) => nested,
            // A link declares no fields: its two parts are its shape, and one
            // of them ends the path.
            (crate::models::SchemaFieldType::Link, _) => {
                return match &segments[index + 1..] {
                    [part] if *part == "title" || *part == "url" => Ok(PropertyScope {
                        bucket,
                        subtypes: Vec::new(),
                    }),
                    _ => Err(OpsError::InvalidParams(format!(
                        "{label} '{name}': '{reached}' is a link field of '{target_type}', and \
                         only its 'title' and 'url' can be read"
                    ))),
                };
            }
            _ => {
                return Err(OpsError::InvalidParams(format!(
                    "{label} '{name}': '{reached}' is a {} field of '{target_type}' with no \
                     declared fields inside it, so a path cannot continue past it",
                    field.field_type
                )));
            }
        };
    }
    Ok(PropertyScope {
        bucket,
        subtypes: Vec::new(),
    })
}

/// The subtypes of `target_type` whose rows keep `field` in a bucket of
/// their own, with what each stored value reads as at `target_type`
/// (ADR-078).
///
/// A subtype that adds values to an inherited enum declares the field
/// itself, so its nodes, and those of the types extending it, store the
/// field under it. A query for the base type still reads the field on those
/// rows, each added value as the base value it maps to. A field
/// `target_type` does not declare has none: a base-type query does not see a
/// subtype's own field.
async fn subtype_buckets(
    node_service: &NodeService,
    declared_fields: &mut DeclaredFields,
    target_type: &str,
    field: &str,
) -> Result<Vec<SubtypeBucket>, OpsError> {
    let target = declared_fields.of(node_service, target_type).await?;
    let Some(owner) = target.owners.get(field).cloned() else {
        return Ok(Vec::new());
    };
    // Only an enum takes added values, so only an enum is declared again by
    // a subtype: any other field needs no read of the subtypes' schemas.
    let is_enum = target.fields.iter().any(|declared| {
        declared.name == field && declared.field_type == crate::models::SchemaFieldType::Enum
    });
    if !is_enum {
        return Ok(Vec::new());
    }
    let target_fields = target.fields.clone();

    let mut buckets: Vec<SubtypeBucket> = Vec::new();
    for subtype in declared_fields.subtypes(node_service, target_type).await? {
        let TypeFields { fields, owners } = declared_fields.of(node_service, &subtype).await?;
        let Some(bucket) = owners.get(field).filter(|bucket| **bucket != owner) else {
            continue;
        };
        if let Some(known) = buckets.iter_mut().find(|known| known.bucket == *bucket) {
            known.node_types.push(subtype);
            continue;
        }
        let values = fields
            .iter()
            .filter(|declared| declared.name == field)
            .flat_map(|declared| {
                declared
                    .core_values
                    .iter()
                    .flatten()
                    .chain(declared.user_values.iter().flatten())
            })
            .filter_map(|value| {
                let reads_as = crate::schema::extends_chain::resolve_value_at_scope(
                    field,
                    &value.value,
                    fields,
                    &target_fields,
                );
                (reads_as.as_deref() != Some(value.value.as_str()))
                    .then(|| (value.value.clone(), reads_as))
            })
            .collect();
        buckets.push(SubtypeBucket {
            bucket: bucket.clone(),
            node_types: vec![subtype],
            values,
        });
    }
    Ok(buckets)
}

/// Resolve where each sort field is stored on the rows of `target_type`,
/// and check each one that is a path into an object field's value against
/// the schema, as [`resolve_filters`] does for a property filter.
///
/// An enum field also gets the order its values sort in: the order its
/// schema declares them ([`enum_rank`]).
pub async fn resolve_sorting(
    node_service: &NodeService,
    target_type: &str,
    sorting: Vec<SortConfig>,
) -> Result<Vec<SortConfig>, OpsError> {
    let mut declared_fields = DeclaredFields::default();
    resolve_sorting_with(node_service, &mut declared_fields, target_type, sorting).await
}

/// [`resolve_sorting`], reading the schemas through `declared_fields`.
async fn resolve_sorting_with(
    node_service: &NodeService,
    declared_fields: &mut DeclaredFields,
    target_type: &str,
    sorting: Vec<SortConfig>,
) -> Result<Vec<SortConfig>, OpsError> {
    let mut resolved = Vec::with_capacity(sorting.len());
    for mut sort in sorting {
        // A node column is read as the column, whatever a schema declares.
        if NODE_COLUMNS.contains(&sort.field.as_str()) {
            resolved.push(sort);
            continue;
        }
        sort.scope = resolve_property(
            node_service,
            declared_fields,
            target_type,
            &sort.field,
            "sort field",
        )
        .await?;
        sort.rank = enum_rank(node_service, declared_fields, target_type, &sort.field).await?;
        resolved.push(sort);
    }
    Ok(resolved)
}

/// The order `field` sorts in on the rows of a query for `target_type`, when
/// it is an enum: the values its schema declares, core values first and then
/// the ones a user added, for any enum on any type.
///
/// The order is the one declared by the type that first declares the field.
/// A type that inherits the field, and may have added values of its own, is
/// ranked on the field as that first type reads it, so an added value sorts
/// where the value it maps to does.
///
/// A query over every type has no one schema and is not ranked, and neither
/// is a path into an object field's value.
async fn enum_rank(
    node_service: &NodeService,
    declared_fields: &mut DeclaredFields,
    target_type: &str,
    field: &str,
) -> Result<Option<EnumRank>, OpsError> {
    if target_type == "*" {
        return Ok(None);
    }
    let is_enum = |fields: &[crate::models::SchemaField]| {
        fields.iter().any(|declared| {
            declared.name == field && declared.field_type == crate::models::SchemaFieldType::Enum
        })
    };
    if !is_enum(&declared_fields.of(node_service, target_type).await?.fields) {
        return Ok(None);
    }

    // The chain is nearest first, so the last type on it that reads the
    // field as an enum is the one that first declares it.
    let chain = node_service
        .resolve_type_chain(target_type)
        .await
        .map_err(|e| OpsError::Internal(e.to_string()))?;
    let mut declaring_type = target_type.to_string();
    for ancestor in chain {
        if is_enum(&declared_fields.of(node_service, &ancestor).await?.fields) {
            declaring_type = ancestor;
        }
    }

    let values = enum_values(
        &declared_fields
            .of(node_service, &declaring_type)
            .await?
            .fields,
        field,
    );
    let scope = if declaring_type == target_type {
        None
    } else {
        Some(PropertyScope {
            subtypes: subtype_buckets(node_service, declared_fields, &declaring_type, field)
                .await?,
            bucket: Some(declaring_type),
        })
    };
    Ok(Some(EnumRank { values, scope }))
}

/// The values `fields` declares for the enum `field`, in declared order:
/// the core values, then the ones a user added. Empty when `field` is not an
/// enum there.
fn enum_values(fields: &[crate::models::SchemaField], field: &str) -> Vec<String> {
    fields
        .iter()
        .filter(|declared| {
            declared.name == field && declared.field_type == crate::models::SchemaFieldType::Enum
        })
        .flat_map(|declared| {
            declared
                .core_values
                .iter()
                .flatten()
                .chain(declared.user_values.iter().flatten())
        })
        .map(|value| value.value.clone())
        .collect()
}

/// One type's effective fields and the type that declares each one.
struct TypeFields {
    fields: Vec<crate::models::SchemaField>,
    owners: std::collections::HashMap<String, String>,
}

/// What the schemas say about the types one query's names are resolved
/// against, each read once.
#[derive(Default)]
struct DeclaredFields {
    types: std::collections::HashMap<String, TypeFields>,
    /// Each type's subtypes, itself left out.
    subtypes: std::collections::HashMap<String, Vec<String>>,
}

impl DeclaredFields {
    async fn of(
        &mut self,
        node_service: &NodeService,
        node_type: &str,
    ) -> Result<&TypeFields, OpsError> {
        if !self.types.contains_key(node_type) {
            let (fields, owners, _) = node_service
                .resolve_field_owners(node_type)
                .await
                .map_err(|e| OpsError::Internal(e.to_string()))?;
            self.types
                .insert(node_type.to_string(), TypeFields { fields, owners });
        }
        Ok(&self.types[node_type])
    }

    async fn subtypes(
        &mut self,
        node_service: &NodeService,
        node_type: &str,
    ) -> Result<Vec<String>, OpsError> {
        if !self.subtypes.contains_key(node_type) {
            let mut subtypes = node_service
                .store()
                .get_subtype_closure(node_type)
                .await
                .map_err(|e| OpsError::Internal(e.to_string()))?;
            subtypes.retain(|subtype| subtype != node_type);
            self.subtypes.insert(node_type.to_string(), subtypes);
        }
        Ok(self.subtypes[node_type].clone())
    }
}

/// Resolve the paths of `filters` against the schemas, so the query service
/// can compile them, and check each property filter's path into an object
/// field's value against the schema that declares it.
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
    let mut declared_fields = DeclaredFields::default();
    resolve_filters_with(node_service, &mut declared_fields, target_type, filters).await
}

/// [`resolve_filters`], reading the schemas through `declared_fields`.
async fn resolve_filters_with(
    node_service: &NodeService,
    declared_fields: &mut DeclaredFields,
    target_type: &str,
    filters: Vec<QueryFilter>,
) -> Result<Vec<QueryFilter>, OpsError> {
    let mut resolved = Vec::with_capacity(filters.len());
    for filter in filters {
        resolved.push(resolve_filter(node_service, declared_fields, target_type, filter).await?);
    }
    Ok(resolved)
}

/// Explicitly boxed (`Pin<Box<dyn Future>>`) rather than a plain `async fn`:
/// this function calls itself for a related-node filter's nested filter, and
/// a self-recursive `async fn` is an infinitely-sized future type by
/// construction — boxing is what gives the recursive call a fixed size.
fn resolve_filter<'a>(
    node_service: &'a NodeService,
    declared_fields: &'a mut DeclaredFields,
    target_type: &'a str,
    mut filter: QueryFilter,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<QueryFilter, OpsError>> + Send + 'a>>
{
    Box::pin(async move {
        if filter.filter_type == FilterType::Property {
            if let Some(property) = &filter.property {
                filter.property_scope = resolve_property(
                    node_service,
                    declared_fields,
                    target_type,
                    property,
                    "filter property",
                )
                .await?;
            }
        }
        if filter.is_permitted() && target_type != "*" {
            // The change is a write to this field, and a write to a field
            // the type does not declare fails for every node.
            let property = filter.property.as_deref().unwrap_or_default();
            let declared = declared_fields.of(node_service, target_type).await?;
            if !declared.owners.contains_key(property) {
                return Err(OpsError::InvalidParams(format!(
                    "permitted filter: the '{target_type}' schema declares no field '{property}'"
                )));
            }
            // A value the enum does not list can be written to no node the
            // query returns: said here, once, and not as every candidate
            // left out. A subtype may have added values of its own, and its
            // nodes are among the query's rows, so its list counts too.
            let mut types = vec![target_type.to_string()];
            types.extend(declared_fields.subtypes(node_service, target_type).await?);
            let mut values: Vec<String> = Vec::new();
            for node_type in &types {
                for value in enum_values(
                    &declared_fields.of(node_service, node_type).await?.fields,
                    property,
                ) {
                    if !values.contains(&value) {
                        values.push(value);
                    }
                }
            }
            if let Some(value) = filter.value.as_ref().and_then(Value::as_str) {
                if !values.is_empty() && !values.iter().any(|listed| listed == value) {
                    return Err(OpsError::InvalidParams(format!(
                        "permitted filter: '{value}' is not a value of '{target_type}.{property}'. \
                         Its values are: {}",
                        values.join(", ")
                    )));
                }
            }
        }
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
                resolve_filter(node_service, declared_fields, &related_type, *nested).await?,
            ));
        }
        filter.resolved_path = Some(resolved);
        Ok(filter)
    })
}

/// The rows of a query for `target_type`, in the shape an agent tool call
/// reads: projected to the queried type's scope, then converted with each
/// row's remaining chain folded into its own bucket (ADR-078).
///
/// The order matters. Projection drops the buckets outside the scope, and
/// the collapse then folds what is left, so a base-type query returns a
/// subtype's row with the base type's fields only. A query over every type
/// (`*`) has no scope, and each row keeps its whole chain's fields.
async fn rows_at_scope(
    node_service: &NodeService,
    nodes: Vec<Node>,
    target_type: &str,
) -> Result<Vec<Value>, OpsError> {
    let projected = node_service
        .project_nodes_to_scope(nodes, Some(target_type))
        .await?;
    crate::ops::node_ops::nodes_to_typed_values(node_service, projected).await
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
    execute_query_nodes_excluding(node_service, input, &[]).await
}

/// [`execute_query_nodes`], leaving out every node whose type is one of
/// `excluded` or extends one of them. The exclusion is part of the statement,
/// so `limit` rows come back whenever that many match.
pub async fn execute_query_nodes_excluding(
    node_service: &Arc<NodeService>,
    input: ExecuteQueryInput,
    excluded: &[crate::models::CoreNodeType],
) -> Result<Vec<Node>, OpsError> {
    let query = to_query_definition(node_service, input).await?;
    Ok(run_definition(node_service, &query, excluded).await?.nodes)
}

// ============================================================================
// Running a definition, `permitted` filters included
// ============================================================================

/// The rows a definition returned.
#[derive(Debug)]
pub struct QueryRows {
    pub nodes: Vec<Node>,
    /// The candidates a `permitted` filter could not be evaluated for: a
    /// rule's condition did not resolve, or the change could not be made to
    /// that node at all. They are left out of `nodes`. Counted up to the
    /// point the limit was reached; 0 for a query with no such filter.
    pub unresolved: usize,
}

/// `query` with its `permitted` filters taken out: the statement the query
/// service runs, and the filters evaluated per node after it.
fn split_permitted(query: &QueryDefinition) -> (QueryDefinition, Vec<QueryFilter>) {
    let (permitted, filters) = query
        .filters
        .iter()
        .cloned()
        .partition(QueryFilter::is_permitted);
    (
        QueryDefinition {
            target_type: query.target_type.clone(),
            filters,
            sorting: query.sorting.clone(),
            limit: query.limit,
        },
        permitted,
    )
}

/// What a query's `permitted` filters say about one candidate.
enum Permission {
    Kept,
    Excluded,
    Unresolved,
}

/// Ask each `permitted` filter of `node`: a dry run of the change it names
/// (ADR-094 §9). A negated filter keeps the nodes the change would be
/// rejected for. A node the dry run cannot answer for is neither: a rule
/// that does not resolve for it, or a change its own type refuses.
///
/// # Errors
///
/// A failure that says nothing about the node (the store could not be read)
/// fails the query, as it fails any other.
async fn permission(
    node_service: &NodeService,
    node: &Node,
    permitted: &[QueryFilter],
) -> Result<Permission, OpsError> {
    use crate::services::NodeServiceError;
    for filter in permitted {
        let (Some(property), Some(value)) = (&filter.property, &filter.value) else {
            return Ok(Permission::Unresolved);
        };
        let update = crate::models::NodeUpdate {
            properties: Some(serde_json::json!({ property: value })),
            ..Default::default()
        };
        let allowed = match node_service.dry_run_update(node, update).await {
            Ok(crate::services::DryRunVerdict::Allowed) => true,
            Ok(crate::services::DryRunVerdict::Rejected { .. }) => false,
            Ok(crate::services::DryRunVerdict::Unresolved {
                play_id,
                rule_name,
                reason,
            }) => {
                tracing::warn!(
                    node_id = %node.id, %play_id, %rule_name, %reason,
                    "A permitted filter could not evaluate a rule for a node; the node is \
                     left out"
                );
                return Ok(Permission::Unresolved);
            }
            // The change is refused for this node by its own type: a
            // type that does not take the value, say. Any other error fails
            // the query, so a new variant that is a refusal of one node's
            // change belongs in this list.
            Err(
                e @ (NodeServiceError::ValidationFailed(_)
                | NodeServiceError::InvalidUpdate(_)
                | NodeServiceError::UnknownNodeType { .. }),
            ) => {
                tracing::warn!(
                    node_id = %node.id, %property, error = %e,
                    "A permitted filter's change could not be made to a node; the node is \
                     left out"
                );
                return Ok(Permission::Unresolved);
            }
            Err(e) => return Err(OpsError::from(e)),
        };
        if allowed == filter.is_negated() {
            return Ok(Permission::Excluded);
        }
    }
    Ok(Permission::Kept)
}

/// Run a checked definition. This is how a definition that may hold a
/// `permitted` filter is run: the query service refuses one, since no SQL
/// condition answers it.
///
/// The statement runs with the query's other filters and its sorting, and
/// each `permitted` filter is then asked of the candidates in that order
/// until the limit is filled, so sort and limit apply to what the filter
/// keeps and a limited query costs no more dry runs than it needs.
pub async fn run_definition(
    node_service: &NodeService,
    query: &QueryDefinition,
    excluded: &[crate::models::CoreNodeType],
) -> Result<QueryRows, OpsError> {
    let query_service = QueryService::new(node_service.store().clone());
    let failed = |e: anyhow::Error| OpsError::Internal(format!("execute_query failed: {e}"));
    if !query.filters.iter().any(QueryFilter::is_permitted) {
        return Ok(QueryRows {
            nodes: query_service
                .execute_excluding(query, excluded)
                .await
                .map_err(failed)?,
            unresolved: 0,
        });
    }

    let (mut statement, permitted) = split_permitted(query);
    let limit = statement.limit.take();
    let candidates = query_service
        .execute_excluding(&statement, excluded)
        .await
        .map_err(failed)?;

    let mut rows = QueryRows {
        nodes: Vec::new(),
        unresolved: 0,
    };
    for node in candidates {
        if limit.is_some_and(|limit| rows.nodes.len() >= limit) {
            break;
        }
        match permission(node_service, &node, &permitted).await? {
            Permission::Kept => rows.nodes.push(node),
            Permission::Excluded => {}
            Permission::Unresolved => rows.unresolved += 1,
        }
    }
    Ok(rows)
}

/// Whether `node` is one of the rows a checked definition returns: the
/// query service's membership test, then the definition's `permitted`
/// filters asked of the node. A node they cannot be evaluated for is not a
/// member.
pub async fn definition_matches(
    node_service: &NodeService,
    query: &QueryDefinition,
    node: &Node,
) -> Result<bool, OpsError> {
    let (statement, permitted) = split_permitted(query);
    let is_member = QueryService::new(node_service.store().clone())
        .matches(&statement, &node.id)
        .await
        .map_err(|e| OpsError::Internal(format!("membership query failed: {e}")))?;
    Ok(is_member
        && matches!(
            permission(node_service, node, &permitted).await?,
            Permission::Kept
        ))
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
    let limit = input.limit.unwrap_or(DEFAULT_QUERY_LIMIT);

    let filters = input
        .filters
        .into_iter()
        .map(to_query_filter)
        .collect::<Result<Vec<_>, _>>()?;

    let sorting: Option<Vec<SortConfig>> = input.sorting.map(|items| {
        items
            .into_iter()
            .map(|s| SortConfig {
                field: s.field,
                direction: parse_sort_direction(s.direction.as_deref().unwrap_or("asc")),
                ..Default::default()
            })
            .collect()
    });

    checked_definition(
        node_service,
        input.target_type,
        filters,
        sorting,
        Some(limit),
    )
    .await
}

/// Rows a query returns when neither it nor its caller names a limit.
const DEFAULT_QUERY_LIMIT: usize = 50;

/// The definition the query service runs, with everything that needs the
/// schemas done first: relationship paths resolved, each path into an object
/// field's value checked, and where each field is stored resolved, in the
/// filters and the sorting alike.
pub(crate) async fn checked_definition(
    node_service: &NodeService,
    target_type: String,
    filters: Vec<QueryFilter>,
    sorting: Option<Vec<SortConfig>>,
    limit: Option<usize>,
) -> Result<QueryDefinition, OpsError> {
    let query = QueryDefinition {
        target_type,
        filters,
        sorting,
        limit,
    };
    // `QueryService` enforces this itself; checking here too classifies a bad
    // identifier as the caller's error rather than an execution failure. It
    // runs ahead of the schema checks so a malformed name is reported as
    // malformed, not as undeclared.
    query
        .validate_identifiers()
        .map_err(|e| OpsError::InvalidParams(e.to_string()))?;

    let QueryDefinition {
        target_type,
        filters,
        sorting,
        limit,
    } = query;
    // One read of each schema serves the filters and the sorting.
    let mut declared_fields = DeclaredFields::default();
    let filters =
        resolve_filters_with(node_service, &mut declared_fields, &target_type, filters).await?;
    let sorting = match sorting {
        Some(sorting) => Some(
            resolve_sorting_with(node_service, &mut declared_fields, &target_type, sorting).await?,
        ),
        None => None,
    };
    Ok(QueryDefinition {
        target_type,
        filters,
        sorting,
        limit,
    })
}

// ============================================================================
// Running a saved query
// ============================================================================

/// A saved query to run, named by its id or its title.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSavedQueryInput {
    /// The query node's id, or its title.
    pub query: String,
    /// Filters ANDed with the stored ones for this run only.
    #[serde(default)]
    pub filters: Vec<AgentFilterItem>,
    /// At most this many nodes. It can lower the stored limit, never raise
    /// it.
    #[serde(default)]
    pub limit: Option<usize>,
    /// The most rows the caller's transport returns, whatever the stored
    /// limit says, and what a query with no limit of its own returns. Set by
    /// the caller in code, never read from input.
    #[serde(skip)]
    pub max_rows: Option<usize>,
}

/// What running a saved query returned.
#[derive(Debug)]
pub struct SavedQueryRun {
    /// The query node that ran.
    pub query_id: String,
    /// The type it selects, or `*`.
    pub target_type: String,
    /// The row limit the run was held to: the stored one, a lower one asked
    /// for, or the caller's. As many rows as this may not be every match.
    pub limit: usize,
    pub nodes: Vec<Node>,
    /// The candidates a `permitted` filter could not be evaluated for, which
    /// the run left out: see [`QueryRows::unresolved`].
    pub unresolved: usize,
}

/// The saved query `reference` names: a query node's id, or the title of
/// exactly one query.
///
/// A title is compared whole, ignoring case and surrounding whitespace. An
/// archived query is found by id only, like any archived node.
///
/// # Errors
///
/// `NotFound` when nothing has that id or title, and `InvalidParams` when the
/// id is a node of another type or several queries share the title; the
/// message says which, and lists the ids to choose from.
pub async fn find_saved_query(
    node_service: &NodeService,
    reference: &str,
) -> Result<Node, OpsError> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(OpsError::InvalidParams(
            "name the saved query to run by its id or its title".to_string(),
        ));
    }

    // An id that names a node of another type is reported only once no query
    // has the reference as its title: some ids read as titles (a date's id is
    // the date), and a query may be titled the same.
    let mut other_type = None;
    if let Some(node) = node_service.get_node(reference).await? {
        if node_service
            .type_is_a(&node.node_type, crate::models::CoreNodeType::Query)
            .await?
        {
            return Ok(node);
        }
        other_type = Some(node.node_type);
    }

    // There are few saved queries, and the comparison folds case the way a
    // person reads a title, which SQL's ASCII-only LOWER does not.
    let every_query = QueryDefinition {
        target_type: nodespace_types::QUERY_NODE_TYPE.to_string(),
        filters: Vec::new(),
        sorting: None,
        limit: None,
    };
    let wanted = reference.to_lowercase();
    let mut titled: Vec<Node> = QueryService::new(node_service.store().clone())
        .execute(&every_query)
        .await
        .map_err(|e| OpsError::Internal(format!("saved query lookup failed: {e}")))?
        .into_iter()
        .filter(|node| node.content.trim().to_lowercase() == wanted)
        .collect();

    match titled.len() {
        0 => Err(match other_type {
            Some(node_type) => OpsError::InvalidParams(format!(
                "'{reference}' is a '{node_type}' node, not a saved query"
            )),
            None => OpsError::NotFound {
                id: format!("no saved query has the id or title '{reference}'"),
            },
        }),
        1 => Ok(titled.remove(0)),
        count => Err(OpsError::InvalidParams(format!(
            "{count} saved queries are titled '{reference}': {}. Run one by its id.",
            titled
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Run a saved query: its stored filters, sorting, limit and relative dates,
/// with `input.filters` ANDed on for this run. Nothing is written to the
/// query node.
///
/// `excluded` leaves out those types and every type extending one of them.
pub async fn run_saved_query_nodes_excluding(
    node_service: &Arc<NodeService>,
    input: RunSavedQueryInput,
    excluded: &[crate::models::CoreNodeType],
) -> Result<SavedQueryRun, OpsError> {
    let node = find_saved_query(node_service, &input.query).await?;
    let fields = crate::models::QueryFields::from_properties(&node.properties)
        .map_err(|e| OpsError::InvalidParams(format!("saved query '{}': {e}", node.id)))?;

    let mut filters = fields.filters;
    for item in input.filters {
        filters.push(to_query_filter(item)?);
    }
    let limit = match (fields.limit, input.limit) {
        (Some(stored), Some(asked)) => stored.min(asked),
        // A query that names no limit selects everything it matches, as it
        // does in the app and in a play: the caller's ceiling bounds it when
        // there is one, so a run is not cut at a default nobody chose.
        (stored, asked) => stored
            .or(asked)
            .or(input.max_rows)
            .unwrap_or(DEFAULT_QUERY_LIMIT),
    };
    let limit = input.max_rows.map_or(limit, |max| limit.min(max));

    let query = checked_definition(
        node_service,
        fields.target_type,
        filters,
        fields.sorting,
        Some(limit),
    )
    .await?;
    let rows = run_definition(node_service, &query, excluded).await?;

    Ok(SavedQueryRun {
        query_id: node.id,
        target_type: query.target_type,
        limit,
        nodes: rows.nodes,
        unresolved: rows.unresolved,
    })
}

/// [`run_saved_query_nodes_excluding`] with nothing left out.
pub async fn run_saved_query_nodes(
    node_service: &Arc<NodeService>,
    input: RunSavedQueryInput,
) -> Result<SavedQueryRun, OpsError> {
    run_saved_query_nodes_excluding(node_service, input, &[]).await
}

/// What [`run_saved_query_excluding`] returns: a saved query's rows as typed
/// JSON values, the shape an agent tool call reads.
#[derive(Debug)]
pub struct SavedQueryOutput {
    pub output: ExecuteQueryOutput,
    /// The row limit the run was held to. A result of that many rows may not
    /// be every match.
    pub limit: usize,
    /// The query node that ran.
    pub query_id: String,
    /// See [`QueryRows::unresolved`].
    pub unresolved: usize,
}

/// [`run_saved_query_nodes_excluding`], returning typed JSON values.
pub async fn run_saved_query_excluding(
    node_service: &Arc<NodeService>,
    input: RunSavedQueryInput,
    excluded: &[crate::models::CoreNodeType],
) -> Result<SavedQueryOutput, OpsError> {
    let run = run_saved_query_nodes_excluding(node_service, input, excluded).await?;
    let count = run.nodes.len();
    Ok(SavedQueryOutput {
        output: ExecuteQueryOutput {
            nodes: rows_at_scope(node_service, run.nodes, &run.target_type).await?,
            count,
            collection_id: None,
        },
        limit: run.limit,
        query_id: run.query_id,
        unresolved: run.unresolved,
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
///
/// A query with a `permitted` filter has no statement that counts it: its
/// candidates are loaded and the filter asked of each, so the count costs a
/// dry run per candidate.
pub async fn count_query(
    node_service: &Arc<NodeService>,
    input: ExecuteQueryInput,
) -> Result<i64, OpsError> {
    let mut query = to_query_definition(node_service, input).await?;
    if query.filters.iter().any(QueryFilter::is_permitted) {
        query.sorting = None;
        query.limit = None;
        let rows = run_definition(node_service, &query, &[]).await?;
        return Ok(rows.nodes.len() as i64);
    }

    let query_service = QueryService::new(node_service.store().clone());
    query_service
        .count(&query)
        .await
        .map_err(|e| OpsError::Internal(format!("count_query failed: {}", e)))
}

/// Execute a structured property query, returning typed JSON values.
///
/// For callers (agent tool call) that want the typed-value shape rather than
/// raw `Node`s. Unlike [`execute_query_nodes`], each row is projected to
/// `target_type`'s scope and carries the fields that type reads (ADR-078).
pub async fn execute_query(
    node_service: &Arc<NodeService>,
    input: ExecuteQueryInput,
) -> Result<ExecuteQueryOutput, OpsError> {
    execute_query_excluding(node_service, input, &[]).await
}

/// [`execute_query`], leaving out every node whose type is one of `excluded`
/// or extends one of them.
pub async fn execute_query_excluding(
    node_service: &Arc<NodeService>,
    input: ExecuteQueryInput,
    excluded: &[crate::models::CoreNodeType],
) -> Result<ExecuteQueryOutput, OpsError> {
    let target_type = input.target_type.clone();
    let nodes = execute_query_nodes_excluding(node_service, input, excluded).await?;
    let count = nodes.len();
    let typed_nodes = rows_at_scope(node_service, nodes, &target_type).await?;

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
                relative_date: None,
                case_sensitive: None,
                negate: Some(true),
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
                    relative_date: None,
                    case_sensitive: Some(false),
                    negate: Some(true),
                    node_id: None,
                    path: None,
                    filter: None,
                    resolved_path: None,
                    property_scope: PropertyScope::default(),
                })),
                resolved_path: None,
                property_scope: PropertyScope::default(),
            },
            QueryFilter {
                filter_type: FilterType::Relationship,
                operator: FilterOperator::Equals,
                property: None,
                value: None,
                relative_date: None,
                case_sensitive: None,
                negate: None,
                node_id: Some("n1".to_string()),
                path: Some(serde_json::from_value(json!(["mentions"])).unwrap()),
                filter: None,
                resolved_path: None,
                property_scope: PropertyScope::default(),
            },
            QueryFilter {
                filter_type: FilterType::Property,
                operator: FilterOperator::LessThanOrEqual,
                property: Some("due_date".to_string()),
                relative_date: Some(RelativeDate {
                    offset_days: Some(7),
                    ..Default::default()
                }),
                ..Default::default()
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
                ..Default::default()
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
                json!({"name": "rf-project", "fields": [{"name": "status", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({
                    "name": "rf-task",
                    "fields": [],
                    "relationships": [{
                        "name": "project",
                        "targetType": "rf-project",
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
                "rf-project",
                json!({"status": "active"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "d65e6776-2ea6-5bff-a3d4-cf789180883f",
                "rf-project",
                json!({"status": "completed"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "d0aaac45-70f6-537e-a669-b6da9229eb6a",
                "rf-task",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "d5255404-47a9-5cbc-ad65-a93d51e23b51",
                "rf-task",
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
                "target_type": "rf-task",
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
                    "name": "rf2-sprint",
                    "fields": []
                }),
            )
            .await;
            create_schema(
                &svc,
                json!({
                    "name": "rf2-task",
                    "fields": [{"name": "severity", "type": "text"}],
                    "relationships": [{
                        "name": "sprint",
                        "targetType": "rf2-sprint",
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
                "rf2-sprint",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "ec7a4732-33ae-578f-b965-8d89e1d05970",
                "rf2-sprint",
                json!({}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "93bf1145-a86c-5584-a012-c98947702696",
                "rf2-task",
                json!({"severity": "critical"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "a7a56745-c02d-5d04-83df-5e02e9fb3366",
                "rf2-task",
                json!({"severity": "minor"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "201dfae7-602b-5e1d-97c1-23c741f34e19",
                "rf2-task",
                json!({"severity": "minor"}),
            ))
            .await
            .unwrap();
            // sprint-hot has both a critical and a minor task -- still one match.
            // The forward declaration (`sprint`) lives on `rf2-task`, so the
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
                "target_type": "rf2-sprint",
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
                json!({"name": "rf3-epic", "fields": [{"name": "status", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({
                    "name": "rf3-task",
                    "fields": [],
                    "relationships": [{
                        "name": "epic",
                        "targetType": "rf3-epic",
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
                json!({"name": "rf3-story", "extends": "rf3-task", "fields": []}),
            )
            .await;

            svc.create_node(node(
                "00e0ccd8-aa1e-53da-92f6-6dab31ad8626",
                "rf3-epic",
                json!({"status": "active"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "2563689f-df77-5f7c-9aad-879d135a915f",
                "rf3-story",
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
                "target_type": "rf3-story",
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

            create_schema(&svc, json!({"name": "rf4-owner", "fields": []})).await;
            create_schema(
                &svc,
                json!({
                    "name": "rf4-project",
                    "fields": [],
                    "relationships": [{
                        "name": "owner",
                        "targetType": "rf4-owner",
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
                    "name": "rf4-task",
                    "fields": [],
                    "relationships": [{
                        "name": "project",
                        "targetType": "rf4-project",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "tasks",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf4-task",
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
                json!({"name": "rf5-project", "fields": [{"name": "status", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({
                    "name": "rf5-task",
                    "fields": [],
                    "relationships": [{
                        "name": "project",
                        "targetType": "rf5-project",
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
                "rf5-project",
                json!({"status": "active"}),
            ))
            .await
            .unwrap();
            svc.create_node(node(
                "054b66fd-0362-5b43-86a6-1961eda0112b",
                "rf5-task",
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
            // rf5-task declares no fields, so the nested filter matches on
            // a metadata field (node_type) rather than a property.
            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "rf5-project",
                "filters": [{
                    "type": "related",
                    "operator": "equals",
                    "path": ["project"],
                    "filter": {
                        "type": "metadata",
                        "operator": "equals",
                        "property": "node_type",
                        "value": "rf5-task"
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
                json!({"name": "pt-project", "fields": [{"name": "status", "type": "text"}]}),
            )
            .await;
            create_schema(
                svc,
                json!({
                    "name": "pt-task",
                    "fields": [],
                    "relationships": [{
                        "name": "project",
                        "targetType": "pt-project",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "tasks",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;

            for (id, node_type, props) in [
                (ACTIVE_PROJECT, "pt-project", json!({"status": "active"})),
                (DONE_PROJECT, "pt-project", json!({"status": "done"})),
                (ACTIVE_TASK, "pt-task", json!({})),
                (DONE_TASK, "pt-task", json!({})),
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
                    "target_type": "pt-task",
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
                    "target_type": "pt-project",
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
                    "target_type": "pt-project",
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
                json!({"name": "st-ticket", "fields": [{"name": "state", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({"name": "st-bug", "extends": "st-ticket", "fields": []}),
            )
            .await;
            svc.create_node(node(
                "a2000000-0000-4000-8000-000000000001",
                "st-ticket",
                json!({"state": "open"}),
            ))
            .await
            .unwrap();
            // The inherited field is written on the subtype's node and stored
            // in the base type's bucket, where the base-scoped filter reads it.
            svc.create_node(node(
                "a2000000-0000-4000-8000-000000000002",
                "st-bug",
                json!({"state": "open"}),
            ))
            .await
            .unwrap();

            let tickets = matching_ids(
                &svc,
                json!({
                    "target_type": "st-ticket",
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

            let bugs = matching_ids(&svc, json!({ "target_type": "st-bug", "filters": [] })).await;
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
                json!({"name": "so-ticket", "fields": [{"name": "rank", "type": "text"}]}),
            )
            .await;
            create_schema(
                &svc,
                json!({"name": "so-bug", "extends": "so-ticket", "fields": []}),
            )
            .await;
            for (id, node_type, rank) in [
                ("a3000000-0000-4000-8000-000000000001", "so-ticket", "a"),
                ("a3000000-0000-4000-8000-000000000002", "so-bug", "m"),
                ("a3000000-0000-4000-8000-000000000003", "so-ticket", "z"),
            ] {
                svc.create_node(node(id, node_type, json!({ "rank": rank })))
                    .await
                    .unwrap();
            }

            let ranks = |direction: &'static str| {
                let svc = Arc::clone(&svc);
                async move {
                    let input: ExecuteQueryInput = serde_json::from_value(json!({
                        "target_type": "so-ticket",
                        "filters": [],
                        "sorting": [{ "field": "rank", "direction": direction }]
                    }))
                    .unwrap();
                    execute_query_nodes(&svc, input)
                        .await
                        .unwrap()
                        .iter()
                        .map(|n| {
                            n.properties["so-ticket"]["rank"]
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

        /// The ids a query returns, in the order it returns them.
        async fn ordered_ids(svc: &Arc<NodeService>, input: serde_json::Value) -> Vec<String> {
            let input: ExecuteQueryInput = serde_json::from_value(input).unwrap();
            execute_query_nodes(svc, input)
                .await
                .unwrap()
                .into_iter()
                .map(|n| n.id)
                .collect()
        }

        /// A query for a subtype sorts by a field the subtype inherits, read
        /// from the bucket of the type that declares it, and a limit keeps
        /// the first rows of that order.
        #[tokio::test(flavor = "multi_thread")]
        async fn sorting_a_subtype_query_orders_by_an_inherited_field() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({
                    "name": "si-ticket",
                    "fields": [
                        { "name": "rank", "type": "text" },
                        { "name": "repository", "type": "object", "fields": [
                            { "name": "url", "type": "text" }
                        ] }
                    ]
                }),
            )
            .await;
            create_schema(
                &svc,
                json!({ "name": "si-bug", "extends": "si-ticket", "fields": [
                    { "name": "severity", "type": "text" }
                ] }),
            )
            .await;
            // Created out of order, with the url order the reverse of the
            // rank order.
            let id = |n: u8| format!("a4000000-0000-4000-8000-00000000000{n}");
            for (n, rank, url) in [
                (1, "c", "d.git"),
                (2, "f", "a.git"),
                (3, "a", "f.git"),
                (4, "e", "b.git"),
                (5, "b", "e.git"),
                (6, "d", "c.git"),
            ] {
                svc.create_node(node(
                    &id(n),
                    "si-bug",
                    json!({ "rank": rank, "severity": "low", "repository": { "url": url } }),
                ))
                .await
                .unwrap();
            }
            let by_rank = [id(3), id(5), id(1), id(6), id(4), id(2)];
            let reversed: Vec<String> = by_rank.iter().rev().cloned().collect();

            let bugs = |field: &'static str, direction: &'static str, limit: usize| {
                let svc = Arc::clone(&svc);
                async move {
                    ordered_ids(
                        &svc,
                        json!({
                            "target_type": "si-bug", "filters": [], "limit": limit,
                            "sorting": [{ "field": field, "direction": direction }]
                        }),
                    )
                    .await
                }
            };
            assert_eq!(bugs("rank", "asc", 50).await, by_rank);
            assert_eq!(bugs("rank", "desc", 50).await, reversed);
            assert_eq!(bugs("rank", "asc", 2).await, by_rank[..2]);
            assert_eq!(bugs("rank", "desc", 2).await, reversed[..2]);

            // A path into an inherited object field.
            assert_eq!(bugs("repository.url", "asc", 50).await, reversed);
            assert_eq!(bugs("repository.url", "desc", 2).await, by_rank[..2]);

            // A query over every type reads each row's own bucket and then
            // its ancestors'.
            let every_type = ordered_ids(
                &svc,
                json!({
                    "target_type": "*", "filters": [], "limit": 3,
                    "sorting": [{ "field": "rank", "direction": "desc" }]
                }),
            )
            .await;
            assert_eq!(every_type, reversed[..3]);

            // The inherited field is read from the declaring type's bucket;
            // a type's own field keeps the expression it always had.
            let scope_of = |target: &'static str| {
                let svc = Arc::clone(&svc);
                async move {
                    let sort = SortConfig {
                        field: "rank".to_string(),
                        ..Default::default()
                    };
                    resolve_sorting(&svc, target, vec![sort])
                        .await
                        .unwrap()
                        .remove(0)
                        .scope
                }
            };
            assert_eq!(
                scope_of("si-bug").await.bucket.as_deref(),
                Some("si-ticket")
            );
            assert_eq!(scope_of("si-ticket").await, PropertyScope::default());
        }

        /// A subtype of a type on the shared urgency scale sorts its
        /// inherited `priority` by rank, not alphabetically, and a limit
        /// cuts by that rank.
        #[tokio::test(flavor = "multi_thread")]
        async fn sorting_a_subtype_by_an_inherited_priority_ranks_it() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({ "name": "sp-bug", "extends": "task", "fields": [] }),
            )
            .await;
            let id = |n: u8| format!("a5000000-0000-4000-8000-00000000000{n}");
            for (n, priority) in [(1, "low"), (2, "highest"), (3, "medium"), (4, "high")] {
                svc.create_node(node(&id(n), "sp-bug", json!({ "priority": priority })))
                    .await
                    .unwrap();
            }
            let by_urgency = [id(2), id(4), id(3), id(1)];

            let bugs = |direction: &'static str, limit: usize| {
                let svc = Arc::clone(&svc);
                async move {
                    ordered_ids(
                        &svc,
                        json!({
                            "target_type": "sp-bug", "filters": [], "limit": limit,
                            "sorting": [{ "field": "priority", "direction": direction }]
                        }),
                    )
                    .await
                }
            };
            assert_eq!(bugs("asc", 50).await, by_urgency);
            assert_eq!(bugs("asc", 2).await, by_urgency[..2]);
            assert_eq!(bugs("desc", 1).await, [id(1)]);
        }

        /// A subtype that adds values to an inherited enum keeps the field in
        /// its own bucket. A query for the base type still reads it on those
        /// rows, each added value as the base value it maps to: in a filter,
        /// in a sort and in the row it returns. A query for the subtype reads
        /// the value as stored.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_base_type_query_reads_a_subtypes_added_enum_value_as_the_base_value() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({
                    "name": "me-ticket",
                    "fields": [{
                        "name": "state", "type": "enum", "extensible": true,
                        "coreValues": [
                            { "value": "open", "label": "Open" },
                            { "value": "done", "label": "Done" }
                        ]
                    }]
                }),
            )
            .await;
            create_schema(
                &svc,
                json!({ "name": "me-bug", "extends": "me-ticket", "fields": [] }),
            )
            .await;
            crate::schema::handle_update_schema(
                &svc,
                json!({
                    "schema_id": "me-bug",
                    "add_field_values": [{
                        "field": "state",
                        "values": [{ "value": "backlog", "label": "Backlog", "mapsTo": "open" }]
                    }]
                }),
            )
            .await
            .unwrap();

            const TICKET_OPEN: &str = "a6000000-0000-4000-8000-000000000001";
            const TICKET_DONE: &str = "a6000000-0000-4000-8000-000000000002";
            const BUG_BACKLOG: &str = "a6000000-0000-4000-8000-000000000003";
            const BUG_OPEN: &str = "a6000000-0000-4000-8000-000000000004";
            const BUG_DONE: &str = "a6000000-0000-4000-8000-000000000005";
            for (id, node_type, state) in [
                (TICKET_OPEN, "me-ticket", "open"),
                (TICKET_DONE, "me-ticket", "done"),
                (BUG_BACKLOG, "me-bug", "backlog"),
                (BUG_OPEN, "me-bug", "open"),
                (BUG_DONE, "me-bug", "done"),
            ] {
                svc.create_node(node(id, node_type, json!({ "state": state })))
                    .await
                    .unwrap();
            }
            // The subtype's rows hold the field under the subtype.
            let stored = svc.get_node(BUG_OPEN).await.unwrap().unwrap();
            assert_eq!(stored.properties["me-bug"]["state"], "open");

            let matching = |target: &'static str, filter: Value| {
                let svc = Arc::clone(&svc);
                async move {
                    matching_ids(&svc, json!({ "target_type": target, "filters": [filter] })).await
                }
            };
            let state_is = |value: &str| json!({ "type": "property", "operator": "equals", "property": "state", "value": value });
            assert_eq!(
                matching("me-ticket", state_is("open")).await,
                [TICKET_OPEN, BUG_BACKLOG, BUG_OPEN]
            );
            let mut not_open = state_is("open");
            not_open["negate"] = json!(true);
            assert_eq!(
                matching("me-ticket", not_open).await,
                [TICKET_DONE, BUG_DONE]
            );
            assert_eq!(
                matching(
                    "me-ticket",
                    json!({ "type": "property", "operator": "in", "property": "state", "value": ["done"] })
                )
                .await,
                [TICKET_DONE, BUG_DONE]
            );
            // The base type has no `backlog`.
            assert!(matching("me-ticket", state_is("backlog")).await.is_empty());
            // At its own type the subtype's value is read as stored.
            assert_eq!(matching("me-bug", state_is("backlog")).await, [BUG_BACKLOG]);
            assert_eq!(matching("me-bug", state_is("open")).await, [BUG_OPEN]);

            // Sorted and returned at the base type, every row carries the
            // base type's value in the base type's bucket. The order is the
            // base type's declared one, `open` then `done`, and a subtype's
            // added value sorts where the value it maps to does.
            let states = |direction: &'static str, limit: usize| {
                let svc = Arc::clone(&svc);
                async move {
                    let input: ExecuteQueryInput = serde_json::from_value(json!({
                        "target_type": "me-ticket", "filters": [], "limit": limit,
                        "sorting": [{ "field": "state", "direction": direction }]
                    }))
                    .unwrap();
                    let nodes = execute_query_nodes(&svc, input).await.unwrap();
                    svc.project_nodes_to_scope(nodes, Some("me-ticket"))
                        .await
                        .unwrap()
                }
            };
            let rows = states("asc", 50).await;
            let read: Vec<&str> = rows
                .iter()
                .map(|n| n.properties["me-ticket"]["state"].as_str().unwrap())
                .collect();
            assert_eq!(read, ["open", "open", "open", "done", "done"]);
            assert!(rows.iter().all(|n| n.properties.get("me-bug").is_none()));
            let mut last_two: Vec<String> =
                states("desc", 2).await.into_iter().map(|n| n.id).collect();
            last_two.sort();
            assert_eq!(last_two, [TICKET_DONE, BUG_DONE]);
        }

        /// An enum extended at two levels of a chain: a value added at the
        /// lower level reads through both mappings at the top type and
        /// through one at the middle type, and a type that extends the
        /// lowest one without adding values keeps the field in its bucket.
        #[tokio::test(flavor = "multi_thread")]
        async fn an_enum_extended_at_two_levels_reads_at_each_type_above() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({
                    "name": "ml-a",
                    "fields": [{
                        "name": "state", "type": "enum", "extensible": true,
                        "coreValues": [
                            { "value": "open", "label": "Open" },
                            { "value": "done", "label": "Done" }
                        ]
                    }]
                }),
            )
            .await;
            for (name, extends) in [("ml-b", "ml-a"), ("ml-c", "ml-b"), ("ml-d", "ml-c")] {
                create_schema(
                    &svc,
                    json!({ "name": name, "extends": extends, "fields": [] }),
                )
                .await;
            }
            for (schema, value, maps_to) in
                [("ml-b", "backlog", "open"), ("ml-c", "icebox", "backlog")]
            {
                crate::schema::handle_update_schema(
                    &svc,
                    json!({
                        "schema_id": schema,
                        "add_field_values": [{
                            "field": "state",
                            "values": [{ "value": value, "label": value, "mapsTo": maps_to }]
                        }]
                    }),
                )
                .await
                .unwrap();
            }

            const A_OPEN: &str = "a8000000-0000-4000-8000-000000000001";
            const B_BACKLOG: &str = "a8000000-0000-4000-8000-000000000002";
            const C_ICEBOX: &str = "a8000000-0000-4000-8000-000000000003";
            const D_ICEBOX: &str = "a8000000-0000-4000-8000-000000000004";
            const D_DONE: &str = "a8000000-0000-4000-8000-000000000005";
            for (id, node_type, state) in [
                (A_OPEN, "ml-a", "open"),
                (B_BACKLOG, "ml-b", "backlog"),
                (C_ICEBOX, "ml-c", "icebox"),
                (D_ICEBOX, "ml-d", "icebox"),
                (D_DONE, "ml-d", "done"),
            ] {
                svc.create_node(node(id, node_type, json!({ "state": state })))
                    .await
                    .unwrap();
            }

            let matching = |target: &'static str, value: &'static str| {
                let svc = Arc::clone(&svc);
                async move {
                    matching_ids(
                        &svc,
                        json!({ "target_type": target, "filters": [{
                            "type": "property", "operator": "equals",
                            "property": "state", "value": value
                        }] }),
                    )
                    .await
                }
            };
            assert_eq!(
                matching("ml-a", "open").await,
                [A_OPEN, B_BACKLOG, C_ICEBOX, D_ICEBOX]
            );
            assert_eq!(matching("ml-a", "done").await, [D_DONE]);
            assert_eq!(
                matching("ml-b", "backlog").await,
                [B_BACKLOG, C_ICEBOX, D_ICEBOX]
            );
            assert!(matching("ml-b", "icebox").await.is_empty());
            assert_eq!(matching("ml-c", "icebox").await, [C_ICEBOX, D_ICEBOX]);

            // The field sorts in the order the top type declares it, `open`
            // then `done`, at every type that reads it. Every other row
            // reads as `open` there, so the one `done` row is last
            // ascending, and first descending, at the top type and at the
            // middle one alike.
            for target in ["ml-a", "ml-b"] {
                let first = ordered_ids(
                    &svc,
                    json!({
                        "target_type": target, "filters": [], "limit": 1,
                        "sorting": [{ "field": "state", "direction": "desc" }]
                    }),
                )
                .await;
                assert_eq!(first, [D_DONE], "{target}");
            }

            // The row returned carries the value the queried type reads.
            let icebox = svc.get_node(D_ICEBOX).await.unwrap().unwrap();
            for (scope, reads_as) in [("ml-a", "open"), ("ml-b", "backlog"), ("ml-c", "icebox")] {
                let projected = svc
                    .project_nodes_to_scope(vec![icebox.clone()], Some(scope))
                    .await
                    .unwrap();
                let read = projected[0]
                    .properties
                    .as_object()
                    .unwrap()
                    .values()
                    .find_map(|bucket| bucket.get("state"))
                    .and_then(Value::as_str);
                assert_eq!(read, Some(reads_as), "{scope}");
            }
        }

        /// A subtype that adds its own values to an inherited `priority`
        /// still sorts by urgency: each added value ranks where the value it
        /// maps to does.
        #[tokio::test(flavor = "multi_thread")]
        async fn sorting_by_a_priority_a_subtype_added_values_to_ranks_what_they_map_to() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({ "name": "su-issue", "extends": "task", "fields": [] }),
            )
            .await;
            crate::schema::handle_update_schema(
                &svc,
                json!({
                    "schema_id": "su-issue",
                    "add_field_values": [{
                        "field": "priority",
                        "values": [
                            { "value": "urgent", "label": "Urgent", "mapsTo": "highest" },
                            { "value": "none", "label": "No priority", "mapsTo": "lowest" }
                        ]
                    }]
                }),
            )
            .await
            .unwrap();
            let id = |n: u8| format!("a7000000-0000-4000-8000-00000000000{n}");
            for (n, priority) in [(1, "none"), (2, "medium"), (3, "urgent"), (4, "high")] {
                svc.create_node(node(&id(n), "su-issue", json!({ "priority": priority })))
                    .await
                    .unwrap();
            }
            let by_urgency = [id(3), id(4), id(2), id(1)];

            for target in ["su-issue", "task"] {
                let sorted = |direction: &'static str, limit: usize| {
                    let svc = Arc::clone(&svc);
                    async move {
                        ordered_ids(
                            &svc,
                            json!({
                                "target_type": target, "filters": [], "limit": limit,
                                "sorting": [{ "field": "priority", "direction": direction }]
                            }),
                        )
                        .await
                    }
                };
                assert_eq!(sorted("asc", 50).await, by_urgency, "{target}");
                assert_eq!(sorted("asc", 1).await, [id(3)], "{target}");
                assert_eq!(sorted("desc", 1).await, [id(1)], "{target}");
            }
        }

        // -- Enum order --

        /// Any enum on any type sorts by the order its schema declares its
        /// values in, core values first and then the ones a user added,
        /// ascending and descending, and a limit cuts by that order. A node
        /// with no value sorts first, and a value the schema does not list
        /// sorts last.
        #[tokio::test(flavor = "multi_thread")]
        async fn sorting_by_any_enum_follows_its_declared_values_core_then_user() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({
                    "name": "eo-ticket",
                    "fields": [{
                        "name": "stage", "type": "enum", "extensible": true,
                        // Alphabetically: doing, shipped, triage.
                        "coreValues": [
                            { "value": "triage", "label": "Triage" },
                            { "value": "doing", "label": "Doing" },
                            { "value": "shipped", "label": "Shipped" }
                        ]
                    }]
                }),
            )
            .await;
            crate::schema::handle_update_schema(
                &svc,
                json!({
                    "schema_id": "eo-ticket",
                    "add_field_values": [{
                        "field": "stage",
                        // Alphabetically: archived, blocked.
                        "values": [
                            { "value": "blocked", "label": "Blocked" },
                            { "value": "archived", "label": "Archived" }
                        ]
                    }]
                }),
            )
            .await
            .unwrap();

            let id = |n: u8| format!("a8000000-0000-4000-8000-00000000000{n}");
            for (n, stage) in [
                (1, Some("archived")),
                (2, Some("shipped")),
                (3, None),
                (4, Some("triage")),
                (5, Some("blocked")),
                (6, Some("doing")),
            ] {
                let props = stage.map_or(json!({}), |stage| json!({ "stage": stage }));
                svc.create_node(node(&id(n), "eo-ticket", props))
                    .await
                    .unwrap();
            }
            let declared = [id(3), id(4), id(6), id(2), id(5), id(1)];

            let sorted = |direction: &'static str, limit: usize| {
                let svc = Arc::clone(&svc);
                async move {
                    ordered_ids(
                        &svc,
                        json!({
                            "target_type": "eo-ticket", "filters": [], "limit": limit,
                            "sorting": [{ "field": "stage", "direction": direction }]
                        }),
                    )
                    .await
                }
            };
            assert_eq!(sorted("asc", 50).await, declared);
            assert_eq!(sorted("asc", 3).await, declared[..3]);
            let mut reversed = declared.clone();
            reversed.reverse();
            assert_eq!(sorted("desc", 50).await, reversed);
            assert_eq!(sorted("desc", 2).await, reversed[..2]);

            // The order is the schema's, read when the query runs.
            let rank = resolve_sorting(
                &svc,
                "eo-ticket",
                vec![SortConfig {
                    field: "stage".to_string(),
                    ..Default::default()
                }],
            )
            .await
            .unwrap()
            .remove(0)
            .rank
            .expect("an enum field is ranked");
            assert_eq!(
                rank.values,
                ["triage", "doing", "shipped", "blocked", "archived"]
            );
            assert_eq!(rank.scope, None);

            // Two values the schema does not list, written past validation:
            // after every declared value, ordered between themselves as text.
            for (n, value) in [(7, "mystery"), (8, "enigma")] {
                svc.create_node(node(&id(n), "eo-ticket", json!({ "stage": "triage" })))
                    .await
                    .unwrap();
                svc.store()
                    .write()
                    .await
                    .execute(
                        &format!(
                            "UPDATE node SET properties = \
                             json_set(properties, '$.\"eo-ticket\".stage', '{value}') \
                             WHERE id = '{}'",
                            id(n)
                        ),
                        (),
                    )
                    .await
                    .unwrap();
            }
            let mut with_unlisted = declared.to_vec();
            with_unlisted.extend([id(8), id(7)]);
            assert_eq!(sorted("asc", 50).await, with_unlisted);
            assert_eq!(sorted("desc", 2).await, [id(7), id(8)]);
        }

        /// A field that is not an enum, a path into an object value and a
        /// query over every type are sorted as stored: only an enum of a
        /// named type has a declared order.
        #[tokio::test(flavor = "multi_thread")]
        async fn only_an_enum_of_a_named_type_is_ranked() {
            let (svc, _tmp) = make_test_service().await;
            let rank_of = |target: &'static str, field: &'static str| {
                let svc = Arc::clone(&svc);
                async move {
                    let sort = SortConfig {
                        field: field.to_string(),
                        ..Default::default()
                    };
                    resolve_sorting(&svc, target, vec![sort])
                        .await
                        .unwrap()
                        .remove(0)
                        .rank
                }
            };
            let status = rank_of("task", "status").await.expect("task.status");
            assert_eq!(
                status.values,
                ["open", "in_progress", "in_review", "done", "cancelled"]
            );
            let priority = rank_of("project", "priority").await.expect("priority");
            assert_eq!(
                priority.values,
                ["highest", "high", "medium", "low", "lowest"]
            );
            assert_eq!(rank_of("task", "due_date").await, None);
            assert_eq!(rank_of("task", "created_at").await, None);
            assert_eq!(rank_of("*", "status").await, None);
        }

        // -- Permitted --

        /// A `permitted` filter names one change, a declared field set to a
        /// value, and is asked of the nodes the query selects. Anything else
        /// is refused when the query is checked, not answered with nothing.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_permitted_filter_names_one_change_to_a_declared_field() {
            let (svc, _tmp) = make_test_service().await;
            let permitted = |extra: serde_json::Value| {
                let mut filter = json!({ "type": "permitted", "operator": "equals" });
                for (key, value) in extra.as_object().unwrap() {
                    filter[key] = value.clone();
                }
                filter
            };
            for (filter, fragment) in [
                (
                    permitted(json!({ "property": "status" })),
                    "missing 'value'",
                ),
                (
                    permitted(json!({ "value": "in_progress" })),
                    "missing 'property'",
                ),
                (
                    permitted(json!({
                        "property": "status", "value": ["in_progress"], "operator": "in"
                    })),
                    "its operator is 'equals'",
                ),
                (
                    permitted(json!({ "property": "statsu", "value": "in_progress" })),
                    "declares no field 'statsu'",
                ),
                (
                    permitted(json!({ "property": "status", "value": "in_progres" })),
                    "'in_progres' is not a value of 'task.status'",
                ),
                (
                    json!({
                        "type": "related", "operator": "exists", "path": ["blocked_by"],
                        "filter": permitted(json!({ "property": "status", "value": "done" }))
                    }),
                    "not of the nodes a related filter reaches",
                ),
            ] {
                let input: ExecuteQueryInput = serde_json::from_value(
                    json!({ "target_type": "task", "filters": [filter.clone()] }),
                )
                .unwrap();
                let err = execute_query_nodes(&svc, input).await.unwrap_err();
                assert!(
                    matches!(&err, OpsError::InvalidParams(message) if message.contains(fragment)),
                    "{filter}: expected '{fragment}', got {err:?}"
                );
            }
        }

        /// A value a subtype added to an inherited enum is not refused on a
        /// query for the base type: the subtype's nodes are among its rows,
        /// and the change can be made to them.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_permitted_filter_takes_a_value_a_subtype_added() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({ "name": "pv-issue", "extends": "task", "fields": [] }),
            )
            .await;
            crate::schema::handle_update_schema(
                &svc,
                json!({
                    "schema_id": "pv-issue",
                    "add_field_values": [{
                        "field": "status",
                        "values": [{ "value": "triage", "label": "Triage", "mapsTo": "open" }]
                    }]
                }),
            )
            .await
            .unwrap();
            const ISSUE: &str = "aa000000-0000-4000-8000-000000000001";
            const TASK: &str = "aa000000-0000-4000-8000-000000000002";
            svc.create_node(node(ISSUE, "pv-issue", json!({ "status": "open" })))
                .await
                .unwrap();
            svc.create_node(task_node(TASK, "open", None))
                .await
                .unwrap();

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "task",
                "filters": [{
                    "type": "permitted", "operator": "equals",
                    "property": "status", "value": "triage"
                }]
            }))
            .unwrap();
            let query = to_query_definition(&svc, input).await.unwrap();
            let rows = run_definition(&svc, &query, &[]).await.unwrap();
            // The issue takes the value; the plain task's own type does not.
            let ids: Vec<&str> = rows.nodes.iter().map(|n| n.id.as_str()).collect();
            assert_eq!(ids, [ISSUE]);
            assert_eq!(rows.unresolved, 1);
        }

        /// The query service has no SQL for a `permitted` filter and refuses
        /// a definition that still holds one, so no caller can run the
        /// statement without the filter and return what it would have kept
        /// back. Run through `query_ops`, with no rule to reject anything,
        /// the filter keeps every candidate.
        #[tokio::test(flavor = "multi_thread")]
        async fn the_query_service_refuses_a_permitted_filter_it_cannot_answer() {
            let (svc, _tmp) = make_test_service().await;
            svc.create_node(task_node(
                "a9000000-0000-4000-8000-000000000001",
                "open",
                None,
            ))
            .await
            .unwrap();
            let query = checked_definition(
                &svc,
                "task".to_string(),
                vec![to_query_filter(
                    serde_json::from_value(json!({
                        "type": "permitted", "operator": "equals",
                        "property": "status", "value": "in_progress"
                    }))
                    .unwrap(),
                )
                .unwrap()],
                None,
                None,
            )
            .await
            .unwrap();

            let query_service = QueryService::new(svc.store().clone());
            for refused in [
                query_service.execute(&query).await.map(|_| ()),
                query_service.count(&query).await.map(|_| ()),
                query_service
                    .matches(&query, "a9000000-0000-4000-8000-000000000001")
                    .await
                    .map(|_| ()),
            ] {
                let message = format!("{:#}", refused.unwrap_err());
                assert!(message.contains("permitted"), "{message}");
            }

            let rows = run_definition(&svc, &query, &[]).await.unwrap();
            assert_eq!(rows.nodes.len(), 1);
            assert_eq!(rows.unresolved, 0);
        }

        // -- Negation --

        /// A negated filter keeps what its condition does not hold for, a
        /// node with no value for the field included; a negated `exists` is
        /// "has no value".
        #[tokio::test(flavor = "multi_thread")]
        async fn a_negated_filter_keeps_the_nodes_its_condition_does_not_hold_for() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({"name": "ng-item", "fields": [{"name": "owner", "type": "text"}]}),
            )
            .await;
            const ADA: &str = "b1000000-0000-4000-8000-000000000001";
            const BOB: &str = "b1000000-0000-4000-8000-000000000002";
            const NOBODY: &str = "b1000000-0000-4000-8000-000000000003";
            svc.create_node(node(ADA, "ng-item", json!({ "owner": "ada" })))
                .await
                .unwrap();
            svc.create_node(node(BOB, "ng-item", json!({ "owner": "bob" })))
                .await
                .unwrap();
            svc.create_node(node(NOBODY, "ng-item", json!({})))
                .await
                .unwrap();

            let not_ada = matching_ids(
                &svc,
                json!({ "target_type": "ng-item", "filters": [{
                    "type": "property", "operator": "equals", "property": "owner",
                    "value": "ada", "negate": true
                }] }),
            )
            .await;
            assert_eq!(not_ada, [BOB, NOBODY]);

            let unowned = matching_ids(
                &svc,
                json!({ "target_type": "ng-item", "filters": [{
                    "type": "property", "operator": "exists", "property": "owner", "negate": true
                }] }),
            )
            .await;
            assert_eq!(unowned, [NOBODY]);

            let not_this_node = matching_ids(
                &svc,
                json!({ "target_type": "ng-item", "filters": [{
                    "type": "content", "operator": "contains", "value": ADA, "negate": true
                }] }),
            )
            .await;
            assert_eq!(not_this_node, [BOB, NOBODY]);
        }

        // -- Saved queries as queues --

        const READY_QUERY: &str = "c0000000-0000-4000-8000-0000000000aa";

        /// Seven tasks around the two conditions of a ready queue, on the
        /// core `task` type's `blocked_by` and a user-defined plan type:
        ///
        /// - `FREE` has no blocker and no plan
        /// - `UNBLOCKED` is blocked by a done task and a cancelled one
        /// - `BLOCKED` is blocked by an open task
        /// - `HALF_BLOCKED` is blocked by a done task and an open one
        /// - `PLANNED` has an approved plan
        /// - `DRAFTED` has a draft plan
        /// - the blockers themselves: `DONE`, `CANCELLED` (finished) and
        ///   `OPEN` (open, unblocked, no plan)
        mod queue {
            pub const FREE: &str = "c0000000-0000-4000-8000-000000000001";
            pub const UNBLOCKED: &str = "c0000000-0000-4000-8000-000000000002";
            pub const BLOCKED: &str = "c0000000-0000-4000-8000-000000000003";
            pub const HALF_BLOCKED: &str = "c0000000-0000-4000-8000-000000000004";
            pub const PLANNED: &str = "c0000000-0000-4000-8000-000000000005";
            pub const DRAFTED: &str = "c0000000-0000-4000-8000-000000000006";
            pub const DONE: &str = "c0000000-0000-4000-8000-000000000011";
            pub const CANCELLED: &str = "c0000000-0000-4000-8000-000000000012";
            pub const OPEN: &str = "c0000000-0000-4000-8000-000000000013";
            pub const APPROVED_PLAN: &str = "c0000000-0000-4000-8000-000000000021";
            pub const DRAFT_PLAN: &str = "c0000000-0000-4000-8000-000000000022";
        }

        fn no_unfinished_blocker() -> serde_json::Value {
            json!({
                "type": "related", "operator": "exists", "path": ["blocked_by"], "negate": true,
                "filter": {
                    "type": "property", "operator": "in", "property": "status",
                    "value": ["done", "cancelled"], "negate": true
                }
            })
        }

        fn no_unapproved_plan() -> serde_json::Value {
            json!({
                "type": "related", "operator": "exists", "path": ["work_plan"], "negate": true,
                "filter": {
                    "type": "property", "operator": "equals", "property": "plan_status",
                    "value": "approved", "negate": true
                }
            })
        }

        async fn seed_queue(svc: &Arc<NodeService>) {
            use queue::*;
            create_schema(
                svc,
                json!({
                    "name": "wf-plan",
                    "fields": [{"name": "plan_status", "type": "text"}],
                    "relationships": [{
                        "name": "plans",
                        "targetType": "task",
                        "direction": "out",
                        "cardinality": "many",
                        "reverseName": "work_plan",
                        "reverseCardinality": "many"
                    }]
                }),
            )
            .await;

            for (id, status) in [
                (FREE, "open"),
                (UNBLOCKED, "open"),
                (BLOCKED, "open"),
                (HALF_BLOCKED, "open"),
                (PLANNED, "open"),
                (DRAFTED, "open"),
                (DONE, "done"),
                (CANCELLED, "cancelled"),
                (OPEN, "open"),
            ] {
                svc.create_node(task_node(id, status, None)).await.unwrap();
            }
            for (blocker, blocked) in [
                (DONE, UNBLOCKED),
                (CANCELLED, UNBLOCKED),
                (OPEN, BLOCKED),
                (DONE, HALF_BLOCKED),
                (OPEN, HALF_BLOCKED),
            ] {
                svc.create_relationship(blocker, "blocks", blocked, json!({}))
                    .await
                    .unwrap();
            }
            for (plan, plan_status, task) in [
                (APPROVED_PLAN, "approved", PLANNED),
                (DRAFT_PLAN, "draft", DRAFTED),
            ] {
                svc.create_node(node(plan, "wf-plan", json!({ "plan_status": plan_status })))
                    .await
                    .unwrap();
                svc.create_relationship(plan, "plans", task, json!({}))
                    .await
                    .unwrap();
            }
        }

        async fn save_query(svc: &Arc<NodeService>, id: &str, title: &str, fields: Value) {
            let mut query = node(id, "query", fields);
            query.content = title.to_string();
            query.title = Some(title.to_string());
            svc.create_node(query)
                .await
                .unwrap_or_else(|e| panic!("saving query '{title}' failed: {e}"));
        }

        async fn run_ids(svc: &Arc<NodeService>, input: Value) -> Vec<String> {
            let input: RunSavedQueryInput = serde_json::from_value(input).unwrap();
            let mut ids: Vec<String> = run_saved_query_nodes(svc, input)
                .await
                .unwrap()
                .nodes
                .into_iter()
                .map(|n| n.id)
                .collect();
            ids.sort();
            ids
        }

        async fn run_error(svc: &Arc<NodeService>, input: Value) -> OpsError {
            let input: RunSavedQueryInput = serde_json::from_value(input).unwrap();
            run_saved_query_nodes(svc, input).await.unwrap_err()
        }

        /// "No `blocked_by` task whose status is not done or cancelled" keeps
        /// a task with no blocker at all, and one whose every blocker is
        /// finished.
        #[tokio::test(flavor = "multi_thread")]
        async fn tasks_with_no_unfinished_blocker_can_be_saved_and_run() {
            use queue::*;
            let (svc, _tmp) = make_test_service().await;
            seed_queue(&svc).await;
            save_query(
                &svc,
                READY_QUERY,
                "Unblocked tasks",
                json!({ "target_type": "task", "filters": [no_unfinished_blocker()] }),
            )
            .await;

            assert_eq!(
                run_ids(&svc, json!({ "query": READY_QUERY })).await,
                [FREE, UNBLOCKED, PLANNED, DRAFTED, DONE, CANCELLED, OPEN]
            );
        }

        /// "No plan whose `plan_status` is not approved" keeps a task with no
        /// plan at all.
        #[tokio::test(flavor = "multi_thread")]
        async fn tasks_with_no_unapproved_plan_can_be_saved_and_run() {
            use queue::*;
            let (svc, _tmp) = make_test_service().await;
            seed_queue(&svc).await;
            save_query(
                &svc,
                READY_QUERY,
                "Planned tasks",
                json!({ "target_type": "task", "filters": [no_unapproved_plan()] }),
            )
            .await;

            assert_eq!(
                run_ids(&svc, json!({ "query": READY_QUERY })).await,
                [
                    FREE,
                    UNBLOCKED,
                    BLOCKED,
                    HALF_BLOCKED,
                    PLANNED,
                    DONE,
                    CANCELLED,
                    OPEN
                ]
            );
        }

        /// A saved query runs by id and by title with its stored filters,
        /// sorting and limit.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_saved_query_runs_by_id_and_by_title() {
            use queue::*;
            let (svc, _tmp) = make_test_service().await;
            seed_queue(&svc).await;
            save_query(
                &svc,
                READY_QUERY,
                "Startable tasks",
                json!({
                    "target_type": "task",
                    "filters": [
                        { "type": "property", "operator": "equals", "property": "status", "value": "open" },
                        no_unfinished_blocker(),
                        no_unapproved_plan()
                    ]
                }),
            )
            .await;

            let ready = [FREE, UNBLOCKED, PLANNED, OPEN];
            assert_eq!(run_ids(&svc, json!({ "query": READY_QUERY })).await, ready);
            assert_eq!(
                run_ids(&svc, json!({ "query": "Startable tasks" })).await,
                ready
            );
            assert_eq!(
                run_ids(&svc, json!({ "query": "  startable TASKS " })).await,
                ready
            );

            let run = run_saved_query_nodes(
                &svc,
                serde_json::from_value(json!({ "query": "Startable tasks" })).unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(run.query_id, READY_QUERY);
            assert_eq!(run.target_type, "task");
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_saved_query_runs_with_its_sorting_limit_and_relative_dates() {
            let (svc, _tmp) = make_test_service().await;
            let today = chrono::Local::now().date_naive();
            let day = |offset: i64| {
                (today + chrono::Duration::days(offset))
                    .format("%Y-%m-%d")
                    .to_string()
            };
            const OVERDUE: &str = "c1000000-0000-4000-8000-000000000001";
            const TODAY: &str = "c1000000-0000-4000-8000-000000000002";
            const TOMORROW: &str = "c1000000-0000-4000-8000-000000000003";
            for (id, offset) in [(OVERDUE, -3), (TODAY, 0), (TOMORROW, 1)] {
                svc.create_node(task_node(id, "open", Some(&day(offset))))
                    .await
                    .unwrap();
            }
            save_query(
                &svc,
                READY_QUERY,
                "Due",
                json!({
                    "target_type": "task",
                    "filters": [{
                        "type": "property", "operator": "lte", "property": "due_date",
                        "relative_date": { "anchor": "today" }
                    }],
                    "sorting": [{ "field": "due_date", "direction": "desc" }],
                    "limit": 5
                }),
            )
            .await;

            let ordered = |input: Value| {
                let svc = Arc::clone(&svc);
                async move {
                    run_saved_query_nodes(&svc, serde_json::from_value(input).unwrap())
                        .await
                        .unwrap()
                        .nodes
                        .into_iter()
                        .map(|n| n.id)
                        .collect::<Vec<_>>()
                }
            };
            assert_eq!(ordered(json!({ "query": "Due" })).await, [TODAY, OVERDUE]);
            // A run-time limit lowers the stored one and never raises it.
            assert_eq!(
                ordered(json!({ "query": "Due", "limit": 1 })).await,
                [TODAY]
            );
            assert_eq!(
                ordered(json!({ "query": "Due", "limit": 500 })).await,
                [TODAY, OVERDUE]
            );
        }

        /// Filters given at run time are ANDed with the stored ones, negated
        /// ones included, and the stored query is left as it was.
        #[tokio::test(flavor = "multi_thread")]
        async fn run_time_filters_narrow_a_run_and_leave_the_query_unchanged() {
            use queue::*;
            let (svc, _tmp) = make_test_service().await;
            seed_queue(&svc).await;
            save_query(
                &svc,
                READY_QUERY,
                "Unblocked tasks",
                json!({ "target_type": "task", "filters": [no_unfinished_blocker()] }),
            )
            .await;
            let before = svc.get_node(READY_QUERY).await.unwrap().unwrap();

            assert_eq!(
                run_ids(
                    &svc,
                    json!({ "query": READY_QUERY, "filters": [
                        { "property": "status", "operator": "equals", "value": "open" },
                        no_unapproved_plan()
                    ] })
                )
                .await,
                [FREE, UNBLOCKED, PLANNED, OPEN]
            );

            let after = svc.get_node(READY_QUERY).await.unwrap().unwrap();
            assert_eq!(after.properties, before.properties);
            assert_eq!(after.version, before.version);
            assert_eq!(
                run_ids(&svc, json!({ "query": READY_QUERY })).await.len(),
                7,
                "the next run is not narrowed"
            );
        }

        /// A reference that names no saved query, or more than one, fails
        /// with a message that says which.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_reference_matching_no_query_or_several_says_which() {
            let (svc, _tmp) = make_test_service().await;
            const FIRST: &str = "c2000000-0000-4000-8000-000000000001";
            const SECOND: &str = "c2000000-0000-4000-8000-000000000002";
            const TASK: &str = "c2000000-0000-4000-8000-000000000003";
            let all_tasks = json!({ "target_type": "task", "filters": [] });
            save_query(&svc, FIRST, "Review queue", all_tasks.clone()).await;
            save_query(&svc, SECOND, "review queue", all_tasks).await;
            svc.create_node(task_node(TASK, "open", None))
                .await
                .unwrap();

            let none = run_error(&svc, json!({ "query": "Triage" })).await;
            assert!(matches!(none, OpsError::NotFound { .. }), "{none:?}");
            assert!(
                none.to_string()
                    .contains("no saved query has the id or title 'Triage'"),
                "{none}"
            );

            let several = run_error(&svc, json!({ "query": "Review queue" })).await;
            assert!(matches!(several, OpsError::InvalidParams(_)), "{several:?}");
            let message = several.to_string();
            assert!(
                message.contains("2 saved queries are titled 'Review queue'"),
                "{message}"
            );
            assert!(
                message.contains(FIRST) && message.contains(SECOND),
                "{message}"
            );

            let not_a_query = run_error(&svc, json!({ "query": TASK })).await;
            assert!(
                not_a_query
                    .to_string()
                    .contains("is a 'task' node, not a saved query"),
                "{not_a_query}"
            );

            let unnamed = run_error(&svc, json!({ "query": "  " })).await;
            assert!(matches!(unnamed, OpsError::InvalidParams(_)), "{unnamed:?}");

            // A reference that is another node's id and also a query's title
            // names the query.
            const TITLED_AS_AN_ID: &str = "c2000000-0000-4000-8000-000000000004";
            save_query(
                &svc,
                TITLED_AS_AN_ID,
                TASK,
                json!({ "target_type": "task", "filters": [] }),
            )
            .await;
            assert_eq!(run_ids(&svc, json!({ "query": TASK })).await, [TASK]);
        }

        // -- Paths into object values --

        const REPO_A: &str = "d0000000-0000-4000-8000-000000000001";
        const REPO_B: &str = "d0000000-0000-4000-8000-000000000002";
        const NO_REPO: &str = "d0000000-0000-4000-8000-000000000003";

        async fn seed_repositories(svc: &Arc<NodeService>) {
            create_schema(
                svc,
                json!({
                    "name": "op-project",
                    "fields": [
                        { "name": "label", "type": "text" },
                        { "name": "repository", "type": "object", "fields": [
                            { "name": "url", "type": "text" },
                            { "name": "host", "type": "object", "fields": [
                                { "name": "name", "type": "text" }
                            ] }
                        ] }
                    ]
                }),
            )
            .await;
            for (id, url, host) in [
                (REPO_A, "https://example.com/a.git", "zeta"),
                (REPO_B, "https://example.com/b.git", "alpha"),
            ] {
                svc.create_node(node(
                    id,
                    "op-project",
                    json!({ "repository": { "url": url, "host": { "name": host } } }),
                ))
                .await
                .unwrap();
            }
            svc.create_node(node(NO_REPO, "op-project", json!({ "label": "none" })))
                .await
                .unwrap();
        }

        /// A property filter and a sort reach a field inside an object value,
        /// to any declared depth.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_filter_and_a_sort_reach_into_an_object_field() {
            let (svc, _tmp) = make_test_service().await;
            seed_repositories(&svc).await;

            let by_url = matching_ids(
                &svc,
                json!({ "target_type": "op-project", "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "repository.url", "value": "https://example.com/b.git"
                }] }),
            )
            .await;
            assert_eq!(by_url, [REPO_B]);

            let not_a = matching_ids(
                &svc,
                json!({ "target_type": "op-project", "filters": [{
                    "type": "property", "operator": "equals", "negate": true,
                    "property": "repository.url", "value": "https://example.com/a.git"
                }] }),
            )
            .await;
            assert_eq!(not_a, [REPO_B, NO_REPO]);

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "op-project",
                "filters": [{
                    "type": "property", "operator": "exists", "property": "repository.host.name"
                }],
                "sorting": [{ "field": "repository.host.name", "direction": "asc" }]
            }))
            .unwrap();
            let sorted: Vec<String> = execute_query_nodes(&svc, input)
                .await
                .unwrap()
                .into_iter()
                .map(|n| n.id)
                .collect();
            assert_eq!(sorted, [REPO_B, REPO_A]);
        }

        /// A filter and a sort reach a link field's two parts, `title` and
        /// `url`, and nothing else under it.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_filter_and_a_sort_reach_into_a_link_field() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({
                    "name": "op-checkout",
                    "fields": [{ "name": "repository", "type": "link" }]
                }),
            )
            .await;
            for (id, title, url) in [
                (REPO_A, "zeta", "https://example.com/a.git"),
                (REPO_B, "alpha", "https://example.com/b.git"),
            ] {
                svc.create_node(node(
                    id,
                    "op-checkout",
                    json!({ "repository": { "title": title, "url": url } }),
                ))
                .await
                .unwrap();
            }
            svc.create_node(node(NO_REPO, "op-checkout", json!({})))
                .await
                .unwrap();

            let by_url = matching_ids(
                &svc,
                json!({ "target_type": "op-checkout", "filters": [{
                    "type": "property", "operator": "equals",
                    "property": "repository.url", "value": "https://example.com/b.git"
                }] }),
            )
            .await;
            assert_eq!(by_url, [REPO_B]);

            let input: ExecuteQueryInput = serde_json::from_value(json!({
                "target_type": "op-checkout",
                "filters": [{
                    "type": "property", "operator": "exists", "property": "repository.title"
                }],
                "sorting": [{ "field": "repository.title", "direction": "asc" }]
            }))
            .unwrap();
            let sorted: Vec<String> = execute_query_nodes(&svc, input)
                .await
                .unwrap()
                .into_iter()
                .map(|n| n.id)
                .collect();
            assert_eq!(sorted, [REPO_B, REPO_A]);

            for property in ["repository.host", "repository.url.scheme"] {
                let input: ExecuteQueryInput = serde_json::from_value(json!({
                    "target_type": "op-checkout",
                    "filters": [{ "type": "property", "operator": "exists", "property": property }]
                }))
                .unwrap();
                let refused = execute_query(&svc, input).await.unwrap_err().to_string();
                assert!(
                    refused.contains("only its 'title' and 'url' can be read"),
                    "{property}: {refused}"
                );
            }
        }

        /// A path the schema does not declare is refused when the query is
        /// saved and when it runs, in a filter and in a sort, with a message
        /// naming it.
        #[tokio::test(flavor = "multi_thread")]
        async fn an_undeclared_path_is_refused_at_save_time_and_at_run_time() {
            let (svc, _tmp) = make_test_service().await;
            seed_repositories(&svc).await;

            let filter_on = |property: &str| json!([{ "type": "property", "operator": "exists", "property": property }]);
            for (property, expected) in [
                ("repository.branch", "declares no field 'repository.branch'"),
                ("mirror.url", "declares no field 'mirror'"),
                ("label.length", "'label' is a text field"),
                ("repository.url.scheme", "'repository.url' is a text field"),
            ] {
                let input: ExecuteQueryInput = serde_json::from_value(
                    json!({ "target_type": "op-project", "filters": filter_on(property) }),
                )
                .unwrap();
                let at_run = execute_query(&svc, input).await.unwrap_err();
                assert!(matches!(at_run, OpsError::InvalidParams(_)), "{at_run:?}");
                assert!(at_run.to_string().contains(expected), "{at_run}");
                assert!(at_run.to_string().contains(property), "{at_run}");

                let mut query = node(
                    READY_QUERY,
                    "query",
                    json!({ "target_type": "op-project", "filters": filter_on(property) }),
                );
                query.content = "Undeclared".to_string();
                let at_save = svc.create_node(query).await.unwrap_err();
                assert!(at_save.to_string().contains(expected), "{at_save}");
            }

            let sorted = json!({
                "target_type": "op-project",
                "filters": [],
                "sorting": [{ "field": "repository.branch", "direction": "asc" }]
            });
            let at_run = execute_query(&svc, serde_json::from_value(sorted.clone()).unwrap())
                .await
                .unwrap_err();
            assert!(
                at_run
                    .to_string()
                    .contains("sort field 'repository.branch'"),
                "{at_run}"
            );
            let mut query = node(READY_QUERY, "query", sorted);
            query.content = "Undeclared sort".to_string();
            let at_save = svc.create_node(query).await.unwrap_err();
            assert!(
                at_save
                    .to_string()
                    .contains("sort field 'repository.branch'"),
                "{at_save}"
            );

            // A name that could not be formatted into a statement is refused
            // when the query is saved, in a filter and in a sort.
            for fields in [
                json!({ "target_type": "op-project", "filters": filter_on("la bel") }),
                json!({ "target_type": "op-project", "filters": [],
                    "sorting": [{ "field": "la'bel", "direction": "asc" }] }),
            ] {
                let mut query = node(READY_QUERY, "query", fields);
                query.content = "Malformed".to_string();
                let at_save = svc.create_node(query).await.unwrap_err();
                assert!(
                    at_save.to_string().contains("contains invalid characters"),
                    "{at_save}"
                );
            }

            // A query over every type has no schema to check a path against.
            let wildcard: ExecuteQueryInput = serde_json::from_value(
                json!({ "target_type": "*", "filters": filter_on("repository.url") }),
            )
            .unwrap();
            let err = execute_query(&svc, wildcard).await.unwrap_err();
            assert!(err.to_string().contains("name the type"), "{err}");
        }

        /// A subtype's node keeps an inherited field in the bucket of the
        /// type that declares it. A filter on a query for the subtype reads
        /// it there, plain or as a path, so its negation does not match a
        /// node the field is set on.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_filter_on_a_subtype_reads_an_inherited_field_where_it_is_stored() {
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({
                    "name": "in-ticket",
                    "fields": [
                        { "name": "state", "type": "text" },
                        { "name": "repository", "type": "object", "fields": [
                            { "name": "url", "type": "text" }
                        ] }
                    ]
                }),
            )
            .await;
            create_schema(
                &svc,
                json!({ "name": "in-bug", "extends": "in-ticket", "fields": [
                    { "name": "severity", "type": "text" }
                ] }),
            )
            .await;
            const OPEN: &str = "e0000000-0000-4000-8000-000000000001";
            const CLOSED: &str = "e0000000-0000-4000-8000-000000000002";
            for (id, state, url) in [(OPEN, "open", "a.git"), (CLOSED, "closed", "b.git")] {
                svc.create_node(node(
                    id,
                    "in-bug",
                    json!({ "state": state, "severity": "low", "repository": { "url": url } }),
                ))
                .await
                .unwrap();
            }

            let bugs = |filter: Value| {
                let svc = Arc::clone(&svc);
                async move {
                    matching_ids(
                        &svc,
                        json!({ "target_type": "in-bug", "filters": [filter] }),
                    )
                    .await
                }
            };
            let state_closed = json!({ "type": "property", "operator": "equals", "property": "state", "value": "closed" });
            assert_eq!(bugs(state_closed.clone()).await, [CLOSED]);
            let mut not_closed = state_closed;
            not_closed["negate"] = json!(true);
            assert_eq!(bugs(not_closed).await, [OPEN]);

            assert_eq!(
                bugs(json!({
                    "type": "property", "operator": "equals", "negate": true,
                    "property": "repository.url", "value": "a.git"
                }))
                .await,
                [CLOSED]
            );
            // The subtype's own field is in its own bucket.
            assert_eq!(
                bugs(json!({
                    "type": "property", "operator": "equals", "property": "severity", "value": "low"
                }))
                .await,
                [OPEN, CLOSED]
            );

            // Reached through a built-in relationship the far end has no one
            // type, and each related row is read at its own type's chain.
            const PARENT_OF_OPEN: &str = "e0000000-0000-4000-8000-000000000011";
            const PARENT_OF_CLOSED: &str = "e0000000-0000-4000-8000-000000000012";
            for (parent, child) in [(PARENT_OF_OPEN, OPEN), (PARENT_OF_CLOSED, CLOSED)] {
                svc.create_node(node(parent, "in-ticket", json!({ "state": "open" })))
                    .await
                    .unwrap();
                svc.create_relationship(parent, "has_child", child, json!({}))
                    .await
                    .unwrap();
            }
            let no_child_that_is_not_closed = matching_ids(
                &svc,
                json!({ "target_type": "in-ticket", "filters": [
                    { "type": "related", "operator": "exists", "path": ["has_child"], "negate": true,
                      "filter": { "type": "property", "operator": "equals", "property": "state",
                                  "value": "closed", "negate": true } }
                ] }),
            )
            .await;
            assert_eq!(
                no_child_that_is_not_closed,
                [OPEN, CLOSED, PARENT_OF_CLOSED]
            );
        }

        /// A caller's row ceiling bounds a stored limit, and is what a query
        /// with no limit of its own returns.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_callers_row_ceiling_bounds_the_stored_limit() {
            let (svc, _tmp) = make_test_service().await;
            for n in 1..=4 {
                svc.create_node(task_node(
                    &format!("e1000000-0000-4000-8000-00000000000{n}"),
                    "open",
                    None,
                ))
                .await
                .unwrap();
            }
            const LIMITED: &str = "e1000000-0000-4000-8000-0000000000a1";
            const UNLIMITED: &str = "e1000000-0000-4000-8000-0000000000a2";
            save_query(
                &svc,
                LIMITED,
                "Limited",
                json!({ "target_type": "task", "filters": [], "limit": 3 }),
            )
            .await;
            save_query(
                &svc,
                UNLIMITED,
                "Unlimited",
                json!({ "target_type": "task", "filters": [] }),
            )
            .await;

            let count = |query: &'static str, max_rows: Option<usize>| {
                let svc = Arc::clone(&svc);
                async move {
                    let mut input: RunSavedQueryInput =
                        serde_json::from_value(json!({ "query": query })).unwrap();
                    input.max_rows = max_rows;
                    run_saved_query_nodes(&svc, input)
                        .await
                        .unwrap()
                        .nodes
                        .len()
                }
            };
            assert_eq!(count("Limited", None).await, 3);
            assert_eq!(count("Limited", Some(2)).await, 2);
            assert_eq!(count("Limited", Some(500)).await, 3);
            assert_eq!(count("Unlimited", Some(2)).await, 2);
            assert_eq!(count("Unlimited", Some(500)).await, 4);
        }

        /// A path is still held to identifier characters: nothing in it can
        /// end the JSON path it is formatted into.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_path_cannot_break_out_of_the_json_path() {
            let (svc, _tmp) = make_test_service().await;
            seed_repositories(&svc).await;

            for property in [
                "repository.url') OR 1=1 --",
                "repository.\"url\"",
                "repository. url",
                "repository url",
                "repository..url",
                ".repository",
                "repository.",
                "repository.url[0]",
                "repository.$",
            ] {
                for input in [
                    json!({ "target_type": "op-project", "filters": [{
                        "type": "property", "operator": "exists", "property": property
                    }] }),
                    json!({ "target_type": "op-project", "filters": [],
                        "sorting": [{ "field": property, "direction": "asc" }] }),
                ] {
                    let err = execute_query(&svc, serde_json::from_value(input).unwrap())
                        .await
                        .unwrap_err();
                    assert!(
                        matches!(err, OpsError::InvalidParams(_)),
                        "{property:?} must be refused as the caller's error, got {err:?}"
                    );
                }
            }
        }

        // -- Row shape: the fields a returned row carries (ADR-078) --

        const SHAPED_BUG: &str = "a9000000-0000-4000-8000-000000000001";

        /// `rs-bug` extends `rs-ticket`, adding a field of its own and a
        /// value to the inherited enum; one bug holds a value in each.
        async fn seed_shaped_bug(svc: &Arc<NodeService>) {
            create_schema(
                svc,
                json!({
                    "name": "rs-ticket",
                    "fields": [
                        {
                            "name": "state", "type": "enum", "extensible": true,
                            "coreValues": [
                                { "value": "open", "label": "Open" },
                                { "value": "done", "label": "Done" }
                            ]
                        },
                        { "name": "owner", "type": "text" }
                    ]
                }),
            )
            .await;
            create_schema(
                svc,
                json!({
                    "name": "rs-bug", "extends": "rs-ticket",
                    "fields": [{ "name": "severity", "type": "text" }]
                }),
            )
            .await;
            crate::schema::handle_update_schema(
                svc,
                json!({
                    "schema_id": "rs-bug",
                    "add_field_values": [{
                        "field": "state",
                        "values": [{ "value": "backlog", "label": "Backlog", "mapsTo": "open" }]
                    }]
                }),
            )
            .await
            .unwrap();
            svc.create_node(node(
                SHAPED_BUG,
                "rs-bug",
                json!({ "state": "backlog", "owner": "ann", "severity": "low" }),
            ))
            .await
            .unwrap();
            // The inherited field is stored under the type that declares it,
            // which is what a single-bucket conversion drops.
            let stored = svc.get_node(SHAPED_BUG).await.unwrap().unwrap();
            assert_eq!(stored.properties["rs-ticket"]["owner"], "ann");
        }

        /// The one row's flat properties, as an agent tool call reads them.
        fn shaped_bug_properties(output: &ExecuteQueryOutput) -> &Value {
            assert_eq!(output.count, 1);
            assert_eq!(output.nodes[0]["id"], SHAPED_BUG);
            &output.nodes[0]["properties"]
        }

        async fn shaped_bug_at(svc: &Arc<NodeService>, input: Value) -> ExecuteQueryOutput {
            execute_query(svc, serde_json::from_value(input).unwrap())
                .await
                .unwrap()
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_subtype_query_returns_inherited_fields_with_its_own() {
            let (svc, _tmp) = make_test_service().await;
            seed_shaped_bug(&svc).await;

            let output = shaped_bug_at(&svc, json!({ "target_type": "rs-bug" })).await;
            assert_eq!(
                shaped_bug_properties(&output),
                &json!({ "state": "backlog", "owner": "ann", "severity": "low" })
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_base_type_query_returns_a_subtype_row_with_the_base_fields_only() {
            let (svc, _tmp) = make_test_service().await;
            seed_shaped_bug(&svc).await;

            // The subtype's added `backlog` reads as the `open` it maps to,
            // and the subtype's own `severity` is not a field of the base.
            let output = shaped_bug_at(&svc, json!({ "target_type": "rs-ticket" })).await;
            assert_eq!(
                shaped_bug_properties(&output),
                &json!({ "state": "open", "owner": "ann" })
            );
        }

        /// A type extending the core `task`: its row carries the inherited
        /// `priority` and the defaulted `status` at both scopes, and its own
        /// field at its own scope only.
        #[tokio::test(flavor = "multi_thread")]
        async fn a_subtype_of_a_core_type_returns_the_core_fields_it_inherits() {
            const BUG: &str = "a9000000-0000-4000-8000-000000000004";
            let (svc, _tmp) = make_test_service().await;
            create_schema(
                &svc,
                json!({
                    "name": "rs-task-bug", "extends": "task",
                    "fields": [{ "name": "severity", "type": "text" }]
                }),
            )
            .await;
            svc.create_node(node(
                BUG,
                "rs-task-bug",
                json!({ "priority": "high", "severity": "low" }),
            ))
            .await
            .unwrap();

            for (target_type, severity) in [("rs-task-bug", Some("low")), ("task", None)] {
                let output = shaped_bug_at(
                    &svc,
                    json!({ "target_type": target_type, "filters": [{
                        "type": "content", "operator": "contains", "value": BUG
                    }] }),
                )
                .await;
                assert_eq!(output.count, 1, "{target_type}");
                let properties = &output.nodes[0]["properties"];
                assert_eq!(properties["priority"], "high", "{target_type}");
                assert!(properties["status"].is_string(), "{target_type}");
                assert_eq!(
                    properties.get("severity").and_then(Value::as_str),
                    severity,
                    "{target_type}"
                );
            }
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_query_over_every_type_returns_the_whole_chain() {
            let (svc, _tmp) = make_test_service().await;
            seed_shaped_bug(&svc).await;

            let output = shaped_bug_at(
                &svc,
                json!({ "target_type": "*", "filters": [{
                    "type": "content", "operator": "contains", "value": SHAPED_BUG
                }] }),
            )
            .await;
            assert_eq!(
                shaped_bug_properties(&output),
                &json!({ "state": "backlog", "owner": "ann", "severity": "low" })
            );
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_saved_query_run_returns_rows_in_the_same_shape() {
            let (svc, _tmp) = make_test_service().await;
            seed_shaped_bug(&svc).await;
            for (id, title, target_type) in [
                ("a9000000-0000-4000-8000-000000000002", "Bugs", "rs-bug"),
                (
                    "a9000000-0000-4000-8000-000000000003",
                    "Tickets",
                    "rs-ticket",
                ),
            ] {
                save_query(
                    &svc,
                    id,
                    title,
                    json!({ "target_type": target_type, "filters": [] }),
                )
                .await;
            }

            for (title, expected) in [
                (
                    "Bugs",
                    json!({ "state": "backlog", "owner": "ann", "severity": "low" }),
                ),
                ("Tickets", json!({ "state": "open", "owner": "ann" })),
            ] {
                let input = serde_json::from_value(json!({ "query": title })).unwrap();
                let output = run_saved_query_excluding(&svc, input, &[])
                    .await
                    .unwrap()
                    .output;
                assert_eq!(shaped_bug_properties(&output), &expected, "{title}");
            }
        }
    }
}
