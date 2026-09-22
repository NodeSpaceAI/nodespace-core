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

/// A SQL string and the values bound to its placeholders
///
/// Filter values never reach the SQL text; they are collected here and bound by
/// the driver. The invariant that makes this safe by construction rather than
/// by audit: the only way to place a value into the SQL is
/// [`Self::bind`], which appends to `params` and returns the matching `?N` in
/// the same step. A caller cannot produce a placeholder without also supplying
/// its value, and cannot supply a value without the number advancing, so the
/// two can never drift out of agreement.
///
/// Identifier positions (table, column and JSON path fragments) are the
/// exception SQL itself imposes: they cannot be bound, so they are still
/// formatted into the text and must keep arriving pre-validated by
/// `query_ops::validate_identifier`.
#[derive(Debug, Default)]
struct BoundSql {
    sql: String,
    params: Vec<libsql::Value>,
}

impl BoundSql {
    /// Record `value` as a bound parameter and return its placeholder
    ///
    /// The returned `?N` is 1-based and matches the value's position in
    /// `params`, which is what libsql binds positionally.
    fn bind(&mut self, value: libsql::Value) -> String {
        self.params.push(value);
        format!("?{}", self.params.len())
    }
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
        let built = self.build_query(query)?;

        // Execute query to get basic node data (without FETCH to avoid Thing deserialization)
        // Use SqliteStore's internal query_nodes for proper handling
        // For now we'll use a simple direct query and manually fetch properties

        // Get node IDs that match the query
        let node_ids = self.execute_query_for_ids(&built).await?;

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
    /// `IS NULL` arm gives it (`json_extract` yields SQL NULL for a JSON null
    /// too, so one arm covers both). A non-string value is not a valid priority
    /// and cannot be ranked, so it takes `USER_RANK`, which is where the `ELSE`
    /// arm puts it in SQL.
    ///
    /// Scope of the agreement, stated precisely because the bug this replaced
    /// hid behind a comment claiming more than it delivered: the **rank**
    /// matches SQL for every input, and the **tie-break within a rank** matches
    /// for strings, where both order the raw value. It does not match for
    /// non-strings — this keys on `to_string()` while SQLite orders integers
    /// before text — but `task.priority` is an enum field, and
    /// `validate_node_with_fields` rejects a non-null non-string on every write
    /// path, so no such row exists to sort. Agreement matters because SQL
    /// applies LIMIT before this pass runs, discarding rows it has already
    /// ordered.
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
    async fn execute_query_for_ids(&self, built: &BoundSql) -> Result<Vec<String>> {
        self.store
            .query_node_ids_raw(&built.sql, built.params.clone())
            .await
            .context("Failed to execute ID query")
    }

    /// Build the ` WHERE ...` suffix selecting the rows a query matches, with
    /// the values bound to its placeholders, or an empty clause when nothing
    /// constrains them.
    ///
    /// This is the whole of what "which rows does this query match?" means, and
    /// it is deliberately the only place that answers it: [`Self::build_query`]
    /// and [`Self::build_count_query`] must select and count the *same* rows, so
    /// they share this rather than each assembling conditions. Ordering and
    /// limiting are not part of matching and stay with the caller — a count has
    /// neither.
    ///
    /// Every value the caller contributes — filter operands, the target type —
    /// is bound through [`BoundSql::bind`] rather than formatted into the text.
    /// Because all of them live in the WHERE clause, both callers inherit the
    /// binding by sharing this, and neither can reintroduce interpolation on its
    /// own. Identifiers (the JSON path segments naming a node type and property)
    /// cannot be bound in SQL and are still interpolated; they arrive
    /// allowlisted by `query_ops::validate_identifier`.
    ///
    /// The returned placeholders are numbered from `?1`, so a caller must not
    /// bind anything of its own ahead of this clause.
    fn build_where_clause(&self, query: &QueryDefinition) -> Result<BoundSql> {
        let mut built = BoundSql::default();
        let mut conditions = Vec::new();

        // Add type filter if not wildcard. `node_type` here is compared as a
        // value, so it binds — unlike the same string used as a JSON path
        // segment below, which cannot.
        if query.target_type != "*" {
            let placeholder = built.bind(libsql::Value::Text(query.target_type.clone()));
            conditions.push(format!("node_type = {}", placeholder));
        }

        // Build filter conditions (pass target_type for namespaced property access)
        for filter in &query.filters {
            let condition = match filter.filter_type {
                FilterType::Property => {
                    self.build_property_filter(filter, &query.target_type, &mut built)?
                }
                FilterType::Content => self.build_content_filter(filter, &mut built)?,
                FilterType::Relationship => self.build_relationship_filter(filter, &mut built)?,
                FilterType::Metadata => self.build_metadata_filter(filter, &mut built)?,
            };
            conditions.push(condition);
        }

        if !conditions.is_empty() {
            built.sql = format!(" WHERE {}", conditions.join(" AND "));
        }
        Ok(built)
    }

    /// Count the nodes a query matches, without materializing them
    ///
    /// The counting counterpart to [`Self::execute`]: same rows, same WHERE
    /// clause, but a scalar instead of hydrated [`Node`]s. A caller that only
    /// needs a total (the query editor's preview) should use this rather than
    /// `execute` + `.len()`, which pays to select ids, `get_node` each one, and
    /// transfer every column of every match purely to discard them.
    ///
    /// `sorting` and `limit` on the definition are ignored: ordering cannot
    /// change a count, and a limit would cap the answer at the very ceiling this
    /// exists to remove. The total returned is exact for any number of matches.
    ///
    /// # Errors
    ///
    /// Returns an error if query building or database execution fails.
    pub async fn count(&self, query: &QueryDefinition) -> Result<i64> {
        let built = self.build_count_query(query)?;

        self.store
            .count_nodes_raw(&built.sql, built.params)
            .await
            .context("Failed to execute count query")
    }

    /// Translate QueryDefinition to a `SELECT COUNT(*)` over the same rows
    /// [`Self::build_query`] would select.
    ///
    /// Emits no ORDER BY and no LIMIT — see [`Self::count`] for why neither
    /// belongs on a count. Filter values are bound, inherited from
    /// [`Self::build_where_clause`]: the count shares the select's clause, so it
    /// cannot differ in how it treats a value any more than it can in which rows
    /// it matches.
    fn build_count_query(&self, query: &QueryDefinition) -> Result<BoundSql> {
        let mut built = self.build_where_clause(query)?;
        built.sql = format!("SELECT COUNT(*) FROM node{};", built.sql);
        Ok(built)
    }

    /// Translate QueryDefinition to SQL
    ///
    /// Builds queries against the unified node table with JSON properties.
    /// All node types use the same query pattern with properties stored inline.
    ///
    /// Properties are now stored in namespaced format:
    /// properties[node_type][field_name] instead of properties[field_name]
    ///
    /// Filter values are bound rather than interpolated — see
    /// [`Self::build_where_clause`], which owns every binding. ORDER BY and
    /// LIMIT, added here, contribute no values.
    fn build_query(&self, query: &QueryDefinition) -> Result<BoundSql> {
        let mut built = self.build_where_clause(query)?;
        built.sql = format!("SELECT * FROM node{}", built.sql);

        // Add sorting (pass target_type for namespaced property access)
        if let Some(sorting) = &query.sorting {
            if !sorting.is_empty() {
                built.sql.push_str(" ORDER BY ");
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
                built.sql.push_str(&clauses.join(", "));
            }
        }

        // Add limit. A `usize` has no string representation SQL could
        // misinterpret, so this is a number formatted into the text rather than
        // bound — which also keeps the LIMIT visible to the query planner.
        if let Some(limit) = query.limit {
            built.sql.push_str(&format!(" LIMIT {}", limit));
        }

        built.sql.push(';');
        Ok(built)
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
    fn build_property_filter(
        &self,
        filter: &QueryFilter,
        target_type: &str,
        built: &mut BoundSql,
    ) -> Result<String> {
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
        self.build_filter_condition(&field, &filter.operator, filter, built)
    }

    /// Build content filter
    ///
    /// Direct access: content CONTAINS 'text'
    fn build_content_filter(&self, filter: &QueryFilter, built: &mut BoundSql) -> Result<String> {
        self.build_content_condition("content", filter, built)
    }

    /// Build relationship filter
    ///
    /// Uses id for filtering: id IN (SELECT...)
    fn build_relationship_filter(
        &self,
        filter: &QueryFilter,
        built: &mut BoundSql,
    ) -> Result<String> {
        self.build_relationship_condition("id", filter, built)
    }

    /// Build metadata filter
    ///
    /// Direct access: created_at >= '2025-01-01'
    fn build_metadata_filter(&self, filter: &QueryFilter, built: &mut BoundSql) -> Result<String> {
        let property = filter
            .property
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Metadata filter missing property"))?;

        if !["created_at", "modified_at", "node_type", "content", "title"]
            .contains(&property.as_str())
        {
            anyhow::bail!("Invalid metadata field: {}", property);
        }

        self.build_filter_condition(property, &filter.operator, filter, built)
    }

    // ========== Shared Filter Building Logic ==========

    /// Build a filter condition with the given field and operator
    ///
    /// Every operand reaches SQL as a bound parameter. The one thing that still
    /// needs escaping is LIKE wildcards, which are a matter of `LIKE` pattern
    /// semantics rather than of SQL syntax: `%` and `_` are metacharacters
    /// *inside* the value, so binding the string does not stop them from
    /// widening the match. See [`Self::escape_string_for_like`].
    fn build_filter_condition(
        &self,
        field: &str,
        operator: &FilterOperator,
        filter: &QueryFilter,
        built: &mut BoundSql,
    ) -> Result<String> {
        let comparison = |op: &str, built: &mut BoundSql| -> Result<String> {
            let value = Self::bind_json_value(filter.value.as_ref(), built)?;
            Ok(format!("{} {} {}", field, op, value))
        };

        match operator {
            FilterOperator::Equals => comparison("=", built),
            FilterOperator::Contains => {
                let value = filter
                    .value
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Contains requires string value"))?;
                if filter.case_sensitive.unwrap_or(true) {
                    // INSTR is case-sensitive in SQLite; no LIKE wildcards needed
                    let placeholder = built.bind(libsql::Value::Text(value.to_string()));
                    Ok(format!("INSTR({}, {}) > 0", field, placeholder))
                } else {
                    // The wildcards are escaped in the bound value itself, so
                    // the pattern's own `%` delimiters are concatenated in SQL
                    // rather than wrapped around an interpolated string.
                    let placeholder =
                        built.bind(libsql::Value::Text(self.escape_string_for_like(value)));
                    Ok(format!(
                        "LOWER({}) LIKE LOWER('%' || {} || '%') ESCAPE '\\'",
                        field, placeholder
                    ))
                }
            }
            FilterOperator::GreaterThan => comparison(">", built),
            FilterOperator::LessThan => comparison("<", built),
            FilterOperator::GreaterThanOrEqual => comparison(">=", built),
            FilterOperator::LessThanOrEqual => comparison("<=", built),
            FilterOperator::In => {
                let values = filter
                    .value
                    .as_ref()
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| anyhow::anyhow!("In requires array value"))?;
                // The list length is data-dependent, so the placeholders are
                // generated one per member rather than being a fixed count.
                let placeholders: Vec<String> = values
                    .iter()
                    .map(|v| Self::bind_json_value(Some(v), built))
                    .collect::<Result<_>>()?;
                Ok(format!("{} IN ({})", field, placeholders.join(", ")))
            }
            FilterOperator::Exists => Ok(format!("{} IS NOT NULL", field)),
        }
    }

    /// Build content filter condition (shared logic)
    fn build_content_condition(
        &self,
        content_field: &str,
        filter: &QueryFilter,
        built: &mut BoundSql,
    ) -> Result<String> {
        let value = filter
            .value
            .as_ref()
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Content filter requires string value"))?;

        match filter.operator {
            FilterOperator::Contains => {
                if filter.case_sensitive.unwrap_or(true) {
                    let placeholder = built.bind(libsql::Value::Text(value.to_string()));
                    Ok(format!("INSTR({}, {}) > 0", content_field, placeholder))
                } else {
                    let placeholder =
                        built.bind(libsql::Value::Text(self.escape_string_for_like(value)));
                    Ok(format!(
                        "LOWER({}) LIKE LOWER('%' || {} || '%') ESCAPE '\\'",
                        content_field, placeholder
                    ))
                }
            }
            FilterOperator::Equals => {
                let placeholder = built.bind(libsql::Value::Text(value.to_string()));
                Ok(format!("{} = {}", content_field, placeholder))
            }
            _ => anyhow::bail!("Unsupported content operator: {:?}", filter.operator),
        }
    }

    /// Build relationship filter condition (shared logic)
    ///
    /// The relationship type is a fixed literal chosen by the match arm, not
    /// caller input, so only the node id needs binding.
    fn build_relationship_condition(
        &self,
        id_field: &str,
        filter: &QueryFilter,
        built: &mut BoundSql,
    ) -> Result<String> {
        let rel_type = filter
            .relationship_type
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing relationshipType"))?;
        let node_id = filter
            .node_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing nodeId"))?;

        // Which column the id is matched against, and which is selected, is the
        // only thing the relationship type varies.
        let (selected, matched, relationship_type) = match rel_type {
            RelationshipType::Children => ("out_node", "in_node", "has_child"),
            RelationshipType::Parent => ("in_node", "out_node", "has_child"),
            RelationshipType::Mentions => ("out_node", "in_node", "mentions"),
            RelationshipType::MentionedBy => ("in_node", "out_node", "mentions"),
        };

        let placeholder = built.bind(libsql::Value::Text(node_id.clone()));
        Ok(format!(
            "{} IN (SELECT {} FROM relationship WHERE {} = {} AND relationship_type = '{}')",
            id_field, selected, matched, placeholder, relationship_type
        ))
    }

    /// Bind a JSON filter operand as a SQL parameter, returning its placeholder
    ///
    /// Only the scalar JSON types have a SQL counterpart; an array or object in
    /// a scalar operand position is a malformed filter and is rejected rather
    /// than stringified into something that would compare unequal to everything.
    fn bind_json_value(value: Option<&serde_json::Value>, built: &mut BoundSql) -> Result<String> {
        let bound = match value {
            Some(serde_json::Value::String(s)) => libsql::Value::Text(s.clone()),
            Some(serde_json::Value::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    libsql::Value::Integer(i)
                } else if let Some(f) = n.as_f64() {
                    libsql::Value::Real(f)
                } else {
                    // Serde only produces a number outside both ranges for u64
                    // values above i64::MAX; there is no lossless SQLite
                    // counterpart, so this fails rather than silently wrapping.
                    anyhow::bail!("Unsupported numeric value: {}", n)
                }
            }
            // SQLite has no boolean type; it stores them as 0/1 integers, which
            // is also how this codebase writes them into the properties JSON.
            Some(serde_json::Value::Bool(b)) => libsql::Value::Integer(i64::from(*b)),
            Some(v) => anyhow::bail!("Unsupported value type: {:?}", v),
            // A JSON `null` arrives here as `None`, not `Some(Value::Null)`:
            // `QueryFilter::value` is an `Option`, so serde folds an explicit
            // null and an absent key into the same thing. An earlier
            // `Some(Value::Null) => libsql::Value::Null` arm was therefore
            // unreachable from the wire, and binding NULL would have been wrong
            // even if reached — `json_extract(...) = NULL` is never true in SQL,
            // so it matches nothing rather than finding unset fields.
            //
            // The message names the two real intents because the model reaching
            // this point has usually confused filtering with projection: asked
            // "when did we sign Northwind?", it emitted `{"operator": "equals",
            // "property": "signed_date", "value": null}` to mean "return that
            // field". Filters only narrow which NODES match; every matching node
            // already carries all of its properties.
            None => anyhow::bail!(
                "Filter has no value. A filter selects which nodes match, not \
                 which fields are returned — every matching node already includes \
                 all of its properties, so drop the filter to read one. To match \
                 only nodes where a property is set, use the 'exists' operator."
            ),
        };
        Ok(built.bind(bound))
    }

    /// Escape a string for use inside a SQL LIKE pattern.
    ///
    /// Escapes `\`, `%`, and `_` so the value is treated as a literal substring
    /// rather than a pattern. Callers must append `ESCAPE '\\'` to the LIKE
    /// expression.
    ///
    /// This survives the move to bound parameters because it is not about SQL
    /// syntax: binding stops a value from being read as SQL, but the bound
    /// string is still interpreted as a LIKE *pattern*, where `%` and `_` keep
    /// their wildcard meaning. Quote escaping, by contrast, is gone — that was
    /// syntax, and binding subsumes it.
    fn escape_string_for_like(&self, s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    }
}

#[cfg(test)]
mod query_service_test;
