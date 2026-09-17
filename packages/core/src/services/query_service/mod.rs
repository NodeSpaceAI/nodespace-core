//! Query Service - Query Execution with SQL Translation
//!
//! This module provides query execution functionality for QueryNode, translating
//! structured query definitions to SQL and executing against the unified
//! node table with JSON properties.
//!
//! # Architecture
//!
//! - **Unified Node Table**: All queries target the `node` table directly
//! - **JSON Properties**: Type-specific properties stored in `properties` JSON column
//! - **Universal Relationship Table**: All relationships in `relationship` table with `relationship_type` discriminator
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
//! use nodespace_core::db::SqliteStore;
//! use std::sync::Arc;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let store = Arc::new(SqliteStore::new("./data/db".into()).await?);
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

use crate::db::SqliteStore;
use crate::models::{Node, TaskPriority};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
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
    store: Arc<SqliteStore>,
}

impl QueryService {
    /// Create a new QueryService
    pub fn new(store: Arc<SqliteStore>) -> Self {
        Self { store }
    }

    /// Execute a query and return matching nodes
    ///
    /// Translates the QueryDefinition to SQL and executes against the database.
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
        // Use SqliteStore's internal query_nodes for proper handling
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
            self.sort_nodes(&mut nodes, sorting, &query.target_type);
        }

        Ok(nodes)
    }

    /// Sort nodes in-place according to the sort configuration
    ///
    /// `target_type` is the query's own target, not each node's type: it
    /// selects the same ordering rules [`Self::resolve_order_field`] used when
    /// building the SQL, so both passes agree.
    fn sort_nodes(&self, nodes: &mut [Node], sorting: &[SortConfig], target_type: &str) {
        if sorting.is_empty() {
            return;
        }

        nodes.sort_by(|a, b| {
            for sort_config in sorting {
                let ordering = self.compare_nodes_by_field(a, b, &sort_config.field, target_type);
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

    /// Compare two nodes by a specific field (Namespaced property access)
    fn compare_nodes_by_field(
        &self,
        a: &Node,
        b: &Node,
        field: &str,
        target_type: &str,
    ) -> std::cmp::Ordering {
        match field {
            // Metadata fields
            "created_at" => a.created_at.cmp(&b.created_at),
            "modified_at" => a.modified_at.cmp(&b.modified_at),
            "content" => a.content.cmp(&b.content),
            "node_type" => a.node_type.cmp(&b.node_type),
            "title" => a.title.cmp(&b.title),
            // Type-specific properties (accessed via namespaced properties JSON)
            // Access properties[node_type][field] for proper namespaced access
            _ => {
                let val_a = a.properties.get(&a.node_type).and_then(|ns| ns.get(field));
                let val_b = b.properties.get(&b.node_type).and_then(|ns| ns.get(field));

                // A task's priority is an enum whose alphabetical order is
                // meaningless, so rank it. This pass runs after the SQL and has
                // the final say, so the condition must match
                // resolve_order_field's exactly — including its `task`-only
                // scope — or the SQL ordering is silently undone here.
                if field == "priority" && target_type == "task" {
                    return self.compare_priority_values(val_a, val_b);
                }

                self.compare_json_values(val_a, val_b)
            }
        }
    }

    /// Compare two `task.priority` values by rank rather than alphabetically
    ///
    /// Ascending yields highest, high, medium, low, lowest, then user-defined
    /// values ordered lexicographically among themselves — the same ordering
    /// [`Self::resolve_order_field`] builds in SQL.
    ///
    /// An absent priority ranks [`TaskPriority::ABSENT_RANK`], before the whole
    /// scale, so it sorts first ascending — the same position the SQL CASE's
    /// `IS NULL` arm gives it. A non-string value is not a valid priority and
    /// cannot be ranked, so it takes `USER_RANK` and tie-breaks on its rendered
    /// form, which is where the `ELSE` arm puts it in SQL. Both rules exist to
    /// keep this pass and the SQL agreeing on every input, not just the
    /// well-formed ones: they disagree only under a LIMIT, where SQL has
    /// already discarded rows before this runs.
    fn compare_priority_values(
        &self,
        a: Option<&serde_json::Value>,
        b: Option<&serde_json::Value>,
    ) -> std::cmp::Ordering {
        /// Rank and sort key for one JSON value, mirroring the SQL CASE arm
        /// that would match it.
        fn key(value: Option<&serde_json::Value>) -> (i16, String) {
            match value {
                None | Some(serde_json::Value::Null) => {
                    (TaskPriority::ABSENT_RANK as i16, String::new())
                }
                Some(serde_json::Value::String(s)) => {
                    // from_str is infallible — every unknown string is User(_)
                    // — but name that fallback rather than letting Default's
                    // Medium stand in for an unparseable value.
                    let priority =
                        TaskPriority::from_str(s).unwrap_or_else(|_| TaskPriority::User(s.clone()));
                    (priority.rank() as i16, s.clone())
                }
                Some(other) => (TaskPriority::USER_RANK as i16, other.to_string()),
            }
        }

        let (rank_a, value_a) = key(a);
        let (rank_b, value_b) = key(b);
        // Ranks tie for two user-defined values; the value string breaks it,
        // mirroring the SQL tiebreaker.
        rank_a.cmp(&rank_b).then_with(|| value_a.cmp(&value_b))
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
        self.store
            .query_node_ids_raw(sql)
            .await
            .context("Failed to execute ID query")
    }

    /// Translate QueryDefinition to SQL
    ///
    /// Builds queries against the unified node table with JSON properties.
    /// All node types use the same query pattern with properties stored inline.
    ///
    /// Properties are now stored in namespaced format:
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
                        self.resolve_order_field(&s.field, &query.target_type, direction)
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

    /// Resolve field name for SQLite queries (Namespaced property access)
    ///
    /// Metadata fields accessed directly: created_at, modified_at, content, node_type, title
    /// Type-specific fields use json_extract: json_extract(properties, '$.task.status')
    ///
    /// NOTE the deliberate asymmetry with [`Self::build_property_filter`],
    /// which resolves the namespaced path itself rather than calling here. A
    /// sort entry is an ordering hint, so reading a same-named top-level column
    /// is the useful reading of `title`; a property filter is an explicit
    /// statement ABOUT the properties JSON, and silently redirecting it to a
    /// column would answer a different question than the caller asked. Do not
    /// "unify" the two without changing that contract first.
    fn resolve_field(&self, field: &str, target_type: &str) -> String {
        if ["created_at", "modified_at", "content", "node_type", "title"].contains(&field) {
            field.to_string()
        } else if target_type == "*" {
            // Properties are namespaced per node type, so a wildcard query has
            // no single literal path — the namespace segment differs row by
            // row. Build it from the row's own node_type: a fixed '$.<field>'
            // is structurally NULL for EVERY row, which SQL then happily
            // ORDERs by and LIMITs on, cutting the real matches before the
            // in-Rust re-sort below ever sees them.
            format!(
                "json_extract(properties, '$.' || node_type || '.{}')",
                field
            )
        } else {
            format!("json_extract(properties, '$.{}.{}')", target_type, field)
        }
    }

    /// Build one ORDER BY term, ranking priority instead of sorting it as text
    ///
    /// Deliberately separate from [`Self::resolve_field`]. `task.priority` is a
    /// string enum whose alphabetical order (`high, highest, low, lowest,
    /// medium`) is meaningless, so ordering by it needs a rank expression —
    /// but `resolve_field`'s output must stay byte-for-byte identical to the
    /// expression `idx_task_priority` is built on, or equality filters silently
    /// stop using that index. Wrapping the CASE in there would trade a working
    /// filter index for a working sort. So the rank lives here, on the ordering
    /// path only, and `resolve_field` is left alone.
    ///
    /// The CASE mirrors [`TaskPriority::rank`]; the two are pinned together by
    /// `test_sql_priority_rank_matches_enum_rank`. User-defined values all land
    /// on the same `ELSE` rank, so the raw value is appended as a tiebreaker to
    /// order them lexicographically among themselves — matching what
    /// [`Self::compare_priority_values`] does in Rust.
    ///
    /// The trade this makes: a CASE is not an indexed expression, so the sort
    /// itself no longer uses `idx_task_priority` and SQLite builds a transient
    /// B-tree for it. Equality *filters* on priority still hit the index, which
    /// is what keeping `resolve_field` untouched buys, and sorting a result set
    /// is the cheaper half. Revisit if priority sorts ever run over row counts
    /// where the transient sort shows up in a profile.
    fn resolve_order_field(&self, field: &str, target_type: &str, direction: &str) -> String {
        let resolved = self.resolve_field(field, target_type);

        // Scoped to `task` only. A wildcard query resolves the namespace from
        // each row's own node_type, so ranking there would impose the task
        // scale on every type's priority (and rank a non-task NULL as a user
        // value instead of sorting it first) — while compare_priority_values
        // ranks only when both nodes are tasks. The two layers would then
        // disagree, and which one won would depend on whether a LIMIT was
        // present. project.priority is a different scale; see the companion
        // issue on whether it should be aligned.
        if field == "priority" && target_type == "task" {
            // A *searched* CASE, deliberately: a simple `CASE <expr> WHEN ...`
            // compares with `=`, and `NULL = 'highest'` is NULL rather than
            // true, so an absent priority would match no arm and fall to ELSE
            // — ranking it as a user-defined value, at the far end of the scale
            // from where compare_priority_values puts it. Because LIMIT applies
            // in SQL before the in-Rust re-sort, that disagreement would drop
            // unprioritized tasks from a limited ascending query that should
            // have returned them first.
            let rank = format!(
                "CASE WHEN {resolved} IS NULL THEN {} \
                 WHEN {resolved} = 'highest' THEN {} WHEN {resolved} = 'high' THEN {} \
                 WHEN {resolved} = 'medium' THEN {} WHEN {resolved} = 'low' THEN {} \
                 WHEN {resolved} = 'lowest' THEN {} ELSE {} END",
                TaskPriority::ABSENT_RANK,
                TaskPriority::Highest.rank(),
                TaskPriority::High.rank(),
                TaskPriority::Medium.rank(),
                TaskPriority::Low.rank(),
                TaskPriority::Lowest.rank(),
                TaskPriority::USER_RANK,
            );
            return format!("{rank} {direction}, {resolved} {direction}");
        }

        format!("{resolved} {direction}")
    }

    // ========== Filter Builders ==========

    /// Build property filter (Namespaced property access)
    ///
    /// Uses SQLite json_extract for property access.
    fn build_property_filter(&self, filter: &QueryFilter, target_type: &str) -> Result<String> {
        let property = filter
            .property
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Property filter missing 'property' field"))?;

        // A property filter always reads the properties JSON, never a
        // same-named top-level column — `resolve_field` is not reused here
        // because its metadata shortcut would make a user field called
        // `title` silently query the title column instead.
        //
        // Under a wildcard the namespace segment differs row by row, so it is
        // taken from the row's own node_type: a fixed '$.<field>' is
        // structurally NULL for EVERY row and matches nothing.
        let field = if target_type == "*" {
            format!(
                "json_extract(properties, '$.' || node_type || '.{}')",
                property
            )
        } else {
            format!("json_extract(properties, '$.{}.{}')", target_type, property)
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

        if !["created_at", "modified_at", "node_type", "content", "title"]
            .contains(&property.as_str())
        {
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
                    // INSTR is case-sensitive in SQLite; no LIKE wildcards needed
                    Ok(format!(
                        "INSTR({}, '{}') > 0",
                        field,
                        self.escape_string(value)
                    ))
                } else {
                    Ok(format!(
                        "LOWER({}) LIKE LOWER('%{}%') ESCAPE '\\'",
                        field,
                        self.escape_string_for_like(value)
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
                Ok(format!("{} IN ({})", field, list.join(", ")))
            }
            FilterOperator::Exists => Ok(format!("{} IS NOT NULL", field)),
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
                        "INSTR({}, '{}') > 0",
                        content_field,
                        self.escape_string(value)
                    ))
                } else {
                    Ok(format!(
                        "LOWER({}) LIKE LOWER('%{}%') ESCAPE '\\'",
                        content_field,
                        self.escape_string_for_like(value)
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

        // Use SQLite subqueries with in_node/out_node column names
        match rel_type {
            RelationshipType::Children => Ok(format!(
                "{} IN (SELECT out_node FROM relationship WHERE in_node = '{}' AND relationship_type = 'has_child')",
                id_field,
                self.escape_string(node_id)
            )),
            RelationshipType::Parent => Ok(format!(
                "{} IN (SELECT in_node FROM relationship WHERE out_node = '{}' AND relationship_type = 'has_child')",
                id_field,
                self.escape_string(node_id)
            )),
            RelationshipType::Mentions => Ok(format!(
                "{} IN (SELECT out_node FROM relationship WHERE in_node = '{}' AND relationship_type = 'mentions')",
                id_field,
                self.escape_string(node_id)
            )),
            RelationshipType::MentionedBy => Ok(format!(
                "{} IN (SELECT in_node FROM relationship WHERE out_node = '{}' AND relationship_type = 'mentions')",
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
            Some(serde_json::Value::Null) => Ok("NULL".to_string()),
            Some(v) => anyhow::bail!("Unsupported value type: {:?}", v),
            None => anyhow::bail!("Missing value"),
        }
    }

    /// Escape single quotes in a string for safe embedding in SQL string literals.
    ///
    /// Use this for equality comparisons (`=`, `IN`). For LIKE patterns, call
    /// `escape_string_for_like` instead, which additionally escapes wildcards.
    fn escape_string(&self, s: &str) -> String {
        s.replace('\'', "''")
    }

    /// Escape a string for use inside a SQL LIKE pattern.
    ///
    /// In addition to single-quote escaping, escapes `\`, `%`, and `_` so the
    /// value is treated as a literal substring. Callers must append
    /// `ESCAPE '\\'` to the LIKE expression.
    fn escape_string_for_like(&self, s: &str) -> String {
        s.replace('\'', "''")
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    }
}

#[cfg(test)]
mod query_service_test;
