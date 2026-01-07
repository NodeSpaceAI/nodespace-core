//! Query Service - Query Execution with SQL Translation
//!
//! This module provides query execution functionality for QueryNode, translating
//! structured query definitions to SurrealQL and executing against the unified
//! node table with JSON properties.
//!
//! # Architecture
//!
//! - **Unified Node Table**: All queries target the `node` table directly
//! - **JSON Properties**: Type-specific properties stored in `properties` JSON column
//! - **Universal Relationship Table**: All relationships in `relationship` table with `relationship_type` discriminator (Issue #788)
//! - **No FETCH**: Single table queries, no record link resolution needed
//!
//! # Query Pattern Examples
//!
//! - Type filter: `SELECT * FROM node WHERE node_type = 'task'`
//! - Property filter: `SELECT * FROM node WHERE properties.status = 'open'`
//! - Relationship: `SELECT * FROM node WHERE id IN (SELECT VALUE out FROM relationship WHERE in = node:⟨parent⟩ AND relationship_type = 'has_child')`
//!
//! # Examples
//!
//! ```rust,no_run
//! use nodespace_core::services::{QueryService, QueryDefinition};
//! use nodespace_core::db::SurrealStore;
//! use std::sync::Arc;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let store = Arc::new(SurrealStore::new("./data/db".into()).await?);
//! let query_service = QueryService::new(store);
//!
//! let query = QueryDefinition {
//!     target_type: "task".to_string(),
//!     filters: vec![],
//!     sorting: None,
//!     limit: Some(50),
//! };
//!
//! let results = query_service.execute(&query).await?;
//! # Ok(())
//! # }
//! ```

use crate::db::SurrealStore;
use crate::models::Node;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Structured query definition matching QueryNode fields
///
/// This struct matches the TypeScript QueryNode interface from
/// `packages/desktop-app/src/lib/types/query.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryDefinition {
    /// Target node type: 'task', 'text', 'date', or '*' for all types
    pub target_type: String,
    /// Filter conditions to apply
    pub filters: Vec<QueryFilter>,
    /// Optional sorting configuration
    pub sorting: Option<Vec<SortConfig>>,
    /// Optional result limit (default: 50)
    pub limit: Option<usize>,
}

/// Filter type category
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FilterType {
    Property,
    Content,
    Relationship,
    Metadata,
}

/// Comparison operator for filters
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FilterOperator {
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// Sorting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortConfig {
    /// Property or field to sort by
    pub field: String,
    /// Sort direction
    pub direction: SortDirection,
}

/// Service for executing queries against the database
pub struct QueryService {
    store: Arc<SurrealStore>,
}

impl QueryService {
    /// Create a new QueryService
    pub fn new(store: Arc<SurrealStore>) -> Self {
        Self { store }
    }

    /// Execute a query and return matching nodes
    ///
    /// Translates the QueryDefinition to SurrealQL and executes against the database.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Query building fails (invalid filter syntax)
    /// - Database query execution fails
    /// - Result deserialization fails
    pub async fn execute(&self, query: &QueryDefinition) -> Result<Vec<Node>> {
        let sql = self.build_query(query)?;

        // Execute query to get basic node data (without FETCH to avoid Thing deserialization)
        // Use SurrealStore's internal query_nodes for proper handling
        // For now we'll use a simple direct query and manually fetch properties

        // Get node IDs that match the query
        let node_ids = self.execute_query_for_ids(&sql).await?;

        // If no results, return empty vector
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Fetch full nodes using the store's get_node method
        let mut nodes = Vec::new();
        for id in node_ids {
            if let Some(node) = self.store.get_node(&id).await? {
                nodes.push(node);
            }
        }

        // Re-apply sorting in Rust to guarantee sort order
        // This ensures consistent sorting even if database ordering behaves unexpectedly
        if let Some(sorting) = &query.sorting {
            self.sort_nodes(&mut nodes, sorting);
        }

        Ok(nodes)
    }

    /// Sort nodes in-place according to the sort configuration
    fn sort_nodes(&self, nodes: &mut [Node], sorting: &[SortConfig]) {
        if sorting.is_empty() {
            return;
        }

        nodes.sort_by(|a, b| {
            for sort_config in sorting {
                let ordering = self.compare_nodes_by_field(a, b, &sort_config.field);
                let ordering = match sort_config.direction {
                    SortDirection::Ascending => ordering,
                    SortDirection::Descending => ordering.reverse(),
                };
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            std::cmp::Ordering::Equal
        });
    }

    /// Compare two nodes by a specific field (Issue #794: Namespaced property access)
    fn compare_nodes_by_field(&self, a: &Node, b: &Node, field: &str) -> std::cmp::Ordering {
        match field {
            // Metadata fields
            "created_at" => a.created_at.cmp(&b.created_at),
            "modified_at" => a.modified_at.cmp(&b.modified_at),
            "content" => a.content.cmp(&b.content),
            "node_type" => a.node_type.cmp(&b.node_type),
            // Type-specific properties (accessed via namespaced properties JSON)
            // Access properties[node_type][field] for proper namespaced access
            _ => {
                let val_a = a.properties.get(&a.node_type).and_then(|ns| ns.get(field));
                let val_b = b.properties.get(&b.node_type).and_then(|ns| ns.get(field));
                self.compare_json_values(val_a, val_b)
            }
        }
    }

    /// Compare two JSON values for sorting
    fn compare_json_values(
        &self,
        a: Option<&serde_json::Value>,
        b: Option<&serde_json::Value>,
    ) -> std::cmp::Ordering {
        match (a, b) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(va), Some(vb)) => {
                // Compare based on JSON value type
                match (va, vb) {
                    (serde_json::Value::String(sa), serde_json::Value::String(sb)) => sa.cmp(sb),
                    (serde_json::Value::Number(na), serde_json::Value::Number(nb)) => {
                        let fa = na.as_f64().unwrap_or(0.0);
                        let fb = nb.as_f64().unwrap_or(0.0);
                        fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
                    }
                    (serde_json::Value::Bool(ba), serde_json::Value::Bool(bb)) => ba.cmp(bb),
                    // For mixed types or arrays/objects, convert to string and compare
                    _ => va.to_string().cmp(&vb.to_string()),
                }
            }
        }
    }

    /// Execute query and return matching node IDs
    async fn execute_query_for_ids(&self, sql: &str) -> Result<Vec<String>> {
        // Keep SELECT * for proper sorting, but extract only IDs from results
        // This avoids "Missing order idiom" errors with SELECT VALUE id

        let mut response = self
            .store
            .db()
            .query(sql)
            .await
            .context("Failed to execute ID query")?;

        // Extract records with Thing IDs
        use surrealdb::sql::Thing;

        #[derive(Debug, Serialize, Deserialize)]
        struct IdRecord {
            id: Thing,
        }

        let records: Vec<IdRecord> = response
            .take(0)
            .context("Failed to extract IDs from query")?;

        // Convert Thing to String (extract UUID part)
        let ids: Vec<String> = records
            .into_iter()
            .filter_map(|record| {
                if let surrealdb::sql::Id::String(id) = record.id.id {
                    Some(id)
                } else {
                    None
                }
            })
            .collect();

        Ok(ids)
    }

    /// Translate QueryDefinition to SurrealQL
    ///
    /// Builds queries against the unified node table with JSON properties.
    /// All node types use the same query pattern with properties stored inline.
    ///
    /// Issue #794: Properties are now stored in namespaced format:
    /// properties[node_type][field_name] instead of properties[field_name]
    fn build_query(&self, query: &QueryDefinition) -> Result<String> {
        let mut sql = String::from("SELECT * FROM node");
        let mut conditions = Vec::new();

        // Add type filter if not wildcard
        if query.target_type != "*" {
            conditions.push(format!("node_type = '{}'", query.target_type));
        }

        // Build filter conditions (pass target_type for namespaced property access)
        for filter in &query.filters {
            match filter.filter_type {
                FilterType::Property => {
                    conditions.push(self.build_property_filter(filter, &query.target_type)?)
                }
                FilterType::Content => conditions.push(self.build_content_filter(filter)?),
                FilterType::Relationship => {
                    conditions.push(self.build_relationship_filter(filter)?)
                }
                FilterType::Metadata => conditions.push(self.build_metadata_filter(filter)?),
            }
        }

        // Apply WHERE clause
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }

        // Add sorting (pass target_type for namespaced property access)
        if let Some(sorting) = &query.sorting {
            if !sorting.is_empty() {
                sql.push_str(" ORDER BY ");
                let clauses: Vec<String> = sorting
                    .iter()
                    .map(|s| {
                        let direction = match s.direction {
                            SortDirection::Ascending => "ASC",
                            SortDirection::Descending => "DESC",
                        };
                        format!(
                            "{} {}",
                            self.resolve_field(&s.field, &query.target_type),
                            direction
                        )
                    })
                    .collect();
                sql.push_str(&clauses.join(", "));
            }
        }

        // Add limit
        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        sql.push(';');
        Ok(sql)
    }

    /// Resolve field name for queries (Issue #794: Namespaced property access)
    ///
    /// Metadata fields accessed directly: created_at, modified_at, content, node_type
    /// Type-specific fields accessed via namespaced properties JSON: properties.task.status
    ///
    /// For wildcard queries (*), we can't namespace, so we fall back to flat access.
    fn resolve_field(&self, field: &str, target_type: &str) -> String {
        if ["created_at", "modified_at", "content", "node_type"].contains(&field) {
            field.to_string()
        } else if target_type == "*" {
            // Wildcard query - can't namespace (would need to check each node's type)
            // Fall back to flat access for wildcard queries
            format!("properties.{}", field)
        } else {
            // Namespaced property access: properties[node_type][field]
            format!("properties.{}.{}", target_type, field)
        }
    }

    // ========== Filter Builders ==========

    /// Build property filter (Issue #794: Namespaced property access)
    ///
    /// Access via namespaced properties JSON: properties.task.status = 'open'
    fn build_property_filter(&self, filter: &QueryFilter, target_type: &str) -> Result<String> {
        let property = filter
            .property
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Property filter missing 'property' field"))?;

        let field = if target_type == "*" {
            // Wildcard query - can't namespace
            format!("properties.{}", property)
        } else {
            // Namespaced property access
            format!("properties.{}.{}", target_type, property)
        };
        self.build_filter_condition(&field, &filter.operator, filter)
    }

    /// Build content filter
    ///
    /// Direct access: content CONTAINS 'text'
    fn build_content_filter(&self, filter: &QueryFilter) -> Result<String> {
        self.build_content_condition("content", filter)
    }

    /// Build relationship filter
    ///
    /// Uses id for filtering: id IN (SELECT...)
    fn build_relationship_filter(&self, filter: &QueryFilter) -> Result<String> {
        self.build_relationship_condition("id", filter)
    }

    /// Build metadata filter
    ///
    /// Direct access: created_at >= '2025-01-01'
    fn build_metadata_filter(&self, filter: &QueryFilter) -> Result<String> {
        let property = filter
            .property
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Metadata filter missing property"))?;

        if !["created_at", "modified_at", "node_type", "content"].contains(&property.as_str()) {
            anyhow::bail!("Invalid metadata field: {}", property);
        }

        self.build_filter_condition(property, &filter.operator, filter)
    }

    // ========== Shared Filter Building Logic ==========

    /// Build a filter condition with the given field and operator
    fn build_filter_condition(
        &self,
        field: &str,
        operator: &FilterOperator,
        filter: &QueryFilter,
    ) -> Result<String> {
        match operator {
            FilterOperator::Equals => {
                let value = self.format_value(filter.value.as_ref())?;
                Ok(format!("{} = {}", field, value))
            }
            FilterOperator::Contains => {
                let value = filter
                    .value
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Contains requires string value"))?;
                if filter.case_sensitive.unwrap_or(true) {
                    Ok(format!(
                        "{} CONTAINS '{}'",
                        field,
                        self.escape_string(value)
                    ))
                } else {
                    Ok(format!(
                        "string::lowercase({}) CONTAINS string::lowercase('{}')",
                        field,
                        self.escape_string(value)
                    ))
                }
            }
            FilterOperator::GreaterThan => {
                let value = self.format_value(filter.value.as_ref())?;
                Ok(format!("{} > {}", field, value))
            }
            FilterOperator::LessThan => {
                let value = self.format_value(filter.value.as_ref())?;
                Ok(format!("{} < {}", field, value))
            }
            FilterOperator::GreaterThanOrEqual => {
                let value = self.format_value(filter.value.as_ref())?;
                Ok(format!("{} >= {}", field, value))
            }
            FilterOperator::LessThanOrEqual => {
                let value = self.format_value(filter.value.as_ref())?;
                Ok(format!("{} <= {}", field, value))
            }
            FilterOperator::In => {
                let values = filter
                    .value
                    .as_ref()
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| anyhow::anyhow!("In requires array value"))?;
                let list: Vec<String> = values
                    .iter()
                    .map(|v| self.format_value(Some(v)))
                    .collect::<Result<_>>()?;
                Ok(format!("{} IN [{}]", field, list.join(", ")))
            }
            FilterOperator::Exists => Ok(format!("{} IS NOT NONE", field)),
        }
    }

    /// Build content filter condition (shared logic)
    fn build_content_condition(&self, content_field: &str, filter: &QueryFilter) -> Result<String> {
        let value = filter
            .value
            .as_ref()
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Content filter requires string value"))?;

        match filter.operator {
            FilterOperator::Contains => {
                if filter.case_sensitive.unwrap_or(true) {
                    Ok(format!(
                        "{} CONTAINS '{}'",
                        content_field,
                        self.escape_string(value)
                    ))
                } else {
                    Ok(format!(
                        "string::lowercase({}) CONTAINS string::lowercase('{}')",
                        content_field,
                        self.escape_string(value)
                    ))
                }
            }
            FilterOperator::Equals => Ok(format!(
                "{} = '{}'",
                content_field,
                self.escape_string(value)
            )),
            _ => anyhow::bail!("Unsupported content operator: {:?}", filter.operator),
        }
    }

    /// Build relationship filter condition (shared logic)
    fn build_relationship_condition(&self, id_field: &str, filter: &QueryFilter) -> Result<String> {
        let rel_type = filter
            .relationship_type
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing relationshipType"))?;
        let node_id = filter
            .node_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing nodeId"))?;

        // Issue #788: Universal Relationship Architecture - use relationship table with type filter
        match rel_type {
            RelationshipType::Children => Ok(format!(
                "{} IN (SELECT VALUE out FROM relationship WHERE in = node:⟨{}⟩ AND relationship_type = 'has_child')",
                id_field,
                self.escape_string(node_id)
            )),
            RelationshipType::Parent => Ok(format!(
                "{} IN (SELECT VALUE in FROM relationship WHERE out = node:⟨{}⟩ AND relationship_type = 'has_child')",
                id_field,
                self.escape_string(node_id)
            )),
            RelationshipType::Mentions => Ok(format!(
                "{} IN (SELECT VALUE out FROM relationship WHERE in = node:⟨{}⟩ AND relationship_type = 'mentions')",
                id_field,
                self.escape_string(node_id)
            )),
            RelationshipType::MentionedBy => Ok(format!(
                "{} IN (SELECT VALUE in FROM relationship WHERE out = node:⟨{}⟩ AND relationship_type = 'mentions')",
                id_field,
                self.escape_string(node_id)
            )),
        }
    }

    /// Format a JSON value for SQL
    fn format_value(&self, value: Option<&serde_json::Value>) -> Result<String> {
        match value {
            Some(serde_json::Value::String(s)) => Ok(format!("'{}'", self.escape_string(s))),
            Some(serde_json::Value::Number(n)) => Ok(n.to_string()),
            Some(serde_json::Value::Bool(b)) => Ok(b.to_string()),
            Some(serde_json::Value::Null) => Ok("NONE".to_string()),
            Some(v) => anyhow::bail!("Unsupported value type: {:?}", v),
            None => anyhow::bail!("Missing value"),
        }
    }

    /// Escape single quotes in strings for SQL safety
    fn escape_string(&self, s: &str) -> String {
        s.replace('\'', "\\'")
    }
}

#[cfg(test)]
mod query_service_test;
