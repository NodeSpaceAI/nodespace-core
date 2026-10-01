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
use crate::models::{Node, Priority};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// The filter and sort vocabulary is shared with the stored query
// (`nodespace_types::QueryFields`), so a saved query's filters decode straight
// into the types executed here.
pub use nodespace_types::{
    FilterOperator, FilterType, QueryFilter, RelationshipType, ResolvedRelationship, SortConfig,
    SortDirection,
};

/// Structured query definition: what a query selects, for execution
///
/// The execution shape, not the stored one. A saved query's fields are
/// decoded by [`nodespace_types::QueryFields`] and mapped here by
/// [`QueryDefinition::from_fields`] — never by deserializing a stored
/// `properties` blob into this struct. Matches the TypeScript
/// `QueryDefinition` in `packages/desktop-app/src/lib/types/query.ts`.
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

impl QueryDefinition {
    /// The definition a saved query executes.
    pub fn from_fields(fields: &nodespace_types::QueryFields) -> Self {
        Self {
            target_type: fields.target_type.clone(),
            filters: fields.filters.clone(),
            sorting: fields.sorting.clone(),
            limit: fields.limit,
        }
    }

    /// Check every identifier this definition would place into SQL text
    ///
    /// The target type, each property filter's property and each sort field
    /// become JSON path segments (`json_extract(properties, '$.<type>.<field>')`)
    /// or column names, and SQL cannot bind an identifier, so they are
    /// formatted into the statement. Each must match `[A-Za-z0-9_:-]+`, which
    /// covers every real node type, property key and metadata field while
    /// leaving no quote, dot or whitespace to break out of the path literal.
    /// `*` is accepted only as the target type, where it means "all types" and
    /// never reaches the text.
    ///
    /// [`QueryService`] runs this before building any statement, so a
    /// definition constructed directly is held to the same rule as one mapped
    /// from agent input. Callers that want to report a malformed query as a
    /// caller error rather than an execution failure may run it first.
    pub fn validate_identifiers(&self) -> Result<()> {
        if self.target_type != "*" {
            validate_identifier(&self.target_type, "target_type")?;
        }
        for filter in &self.filters {
            validate_filter_identifiers(filter, 0)?;
        }
        for sort in self.sorting.iter().flatten() {
            validate_identifier(&sort.field, "sort field")?;
        }
        Ok(())
    }
}

/// Maximum nesting depth a [`FilterType::Related`] filter's own `filter`
/// may reach: 0 means the outermost `Related` filter's nested condition
/// must not itself be `Related`. Every named use case so far (tasks by
/// project status, sprints by task severity) is one hop; unbounded depth
/// is unvalidated scope with no consuming case yet, and the recursive
/// shape makes raising this a validator change, not a shape change.
const MAX_RELATED_DEPTH: usize = 0;

/// Check a filter's own identifiers, and recurse into a nested
/// [`FilterType::Related`] filter — both its `relationship_name` (which
/// becomes a bound value, not formatted into SQL text, but is still worth
/// rejecting early as a caller error) and its own `filter`, which is
/// walked the same way [`QueryDefinition::validate_identifiers`] walks a
/// top-level filter list.
///
/// `depth` counts `Related` nesting already consumed by the time this
/// filter is reached: 0 for a top-level filter, 1 for the nested `filter`
/// of a top-level `Related` filter, and so on. A `Related` filter at
/// `depth > MAX_RELATED_DEPTH` is rejected outright — see
/// [`MAX_RELATED_DEPTH`] for why the cap is enforced here rather
/// than left to the executor to silently truncate or misexecute.
fn validate_filter_identifiers(filter: &QueryFilter, depth: usize) -> Result<()> {
    if let Some(property) = &filter.property {
        validate_identifier(property, "filter property")?;
    }
    if filter.filter_type == FilterType::Related {
        if depth > MAX_RELATED_DEPTH {
            anyhow::bail!(
                "Related filter nesting depth {} exceeds the maximum of {} — \
                 a Related filter's own nested filter must not itself be Related",
                depth + 1,
                MAX_RELATED_DEPTH + 1
            );
        }
        let relationship_name = filter
            .relationship_name
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Related filter missing 'relationshipName'"))?;
        validate_identifier(relationship_name, "filter relationshipName")?;
        let nested = filter
            .filter
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Related filter missing 'filter'"))?;
        validate_filter_identifiers(nested, depth + 1)?;
    }
    Ok(())
}

/// Reject an identifier that is unsafe to format into SQL text
///
/// See [`QueryDefinition::validate_identifiers`] for the allowlist and why it
/// is needed.
fn validate_identifier(value: &str, label: &str) -> Result<()> {
    if value.is_empty() {
        anyhow::bail!("{} must not be empty", label);
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':')
    {
        anyhow::bail!(
            "{} '{}' contains invalid characters; only [A-Za-z0-9_:-] are allowed",
            label,
            value
        );
    }
    Ok(())
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
/// formatted into the text, and [`QueryService::build_where_clause`] rejects
/// the definition first unless [`QueryDefinition::validate_identifiers`]
/// passes.
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

/// Rewrite every `?N` placeholder in `condition` to `?(N + offset)`
///
/// A [`FilterType::Related`] filter's nested condition is compiled against
/// its own fresh [`BoundSql`] (see [`QueryService::build_related_filter`]),
/// so its placeholders start again from `?1` — correct in isolation, but
/// wrong once its parameters are appended to the outer statement's own list,
/// where they no longer sit at the front. This renumbers the SQL text to
/// match where they actually landed, after the caller has already appended
/// the nested params to the outer list and recorded its length beforehand as
/// `offset`.
///
/// A single left-to-right pass over the text, copying every non-placeholder
/// byte verbatim and emitting `?(N + offset)` in place of each `?N` found —
/// not a series of whole-string `str::replace` calls keyed by original
/// number. That repeated-replace shape looks sound (descending order so
/// `?10` isn't matched as `?1` followed by a stray `0`) but is not: replacing
/// `?2` with, say, `?12` before `?1` has been replaced writes the literal
/// text `"?12"` into the string, and the later, unrelated replacement of
/// `?1` -> `?11` then matches the `"?1"` *inside* that already-written
/// `"?12"` too, corrupting it to `"?112"`. A single forward pass has no such
/// hazard: each placeholder is consumed exactly once, character by character,
/// and the cursor never revisits text already emitted.
fn renumber_placeholders(condition: &str, offset: usize) -> String {
    if offset == 0 {
        return condition.to_string();
    }

    let chars: Vec<char> = condition.chars().collect();
    let mut result = String::with_capacity(condition.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '?' {
            let start = i + 1;
            let mut end = start;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                let digits: String = chars[start..end].iter().collect();
                let n: usize = digits.parse().unwrap();
                result.push_str(&format!("?{}", n + offset));
                i = end;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
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

                // A task's or project's priority is an enum whose alphabetical
                // order is meaningless, so rank it. This pass runs after the SQL
                // and has the final say, so the condition must match
                // resolve_order_field's exactly — including its
                // `Priority::NODE_TYPES`-only scope — or the SQL ordering is
                // silently undone here.
                if field == "priority" && Priority::applies_to(target_type) {
                    return self.compare_priority_values(val_a, val_b);
                }

                self.compare_json_values(val_a, val_b)
            }
        }
    }

    /// Compare two `priority` values of a [`Priority::NODE_TYPES`] type by
    /// rank rather than alphabetically
    ///
    /// Ascending yields highest, high, medium, low, lowest, then user-defined
    /// values ordered lexicographically among themselves — the same ordering
    /// [`Self::resolve_order_field`] builds in SQL.
    ///
    /// An absent priority ranks [`Priority::ABSENT_RANK`], before the whole
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
    /// before text — but `priority` is an enum field on every type ranked
    /// here, and
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
                    (Priority::ABSENT_RANK as i16, String::new())
                }
                Some(serde_json::Value::String(s)) => {
                    (Priority::from_value(s).rank() as i16, s.clone())
                }
                Some(other) => (Priority::USER_RANK as i16, other.to_string()),
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
    /// cannot be bound in SQL and are still interpolated, so this checks them
    /// against [`QueryDefinition::validate_identifiers`] before emitting any
    /// text. It is the one step every statement shares, which makes the check
    /// unskippable: no caller can reach SQL with an identifier it has not
    /// passed. Sort fields are checked here too, although only
    /// [`Self::build_query`] emits them, so that a malformed definition is
    /// rejected the same way whichever verb it is asked with.
    ///
    /// The returned placeholders are numbered from `?1`, so a caller must not
    /// bind anything of its own ahead of this clause.
    fn build_where_clause(&self, query: &QueryDefinition) -> Result<BoundSql> {
        query.validate_identifiers()?;

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
                FilterType::Related => self.build_related_filter("id", filter, &mut built)?,
            };
            conditions.push(condition);
        }

        // A saved query is a default query: an archived node is in none of
        // them, and neither is a type the registry leaves out (ADR-087 §2).
        // The definition has no opt-in; an archived node is reached by id.
        conditions.extend(crate::governance::default_query_conditions("", false));

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
    /// Deliberately separate from [`Self::resolve_field`]. `priority` on the
    /// [`Priority::NODE_TYPES`] types is a string enum whose alphabetical
    /// order (`high, highest, low, lowest, medium`) is meaningless, so
    /// ordering by it needs a rank expression — but `resolve_field`'s output
    /// must stay byte-for-byte identical to the expression each type's
    /// priority index (`idx_task_priority`, `idx_project_priority`) is built
    /// on, or equality filters silently stop using it. Wrapping the CASE in there would trade a working
    /// filter index for a working sort. So the rank lives here, on the ordering
    /// path only, and `resolve_field` is left alone.
    ///
    /// The CASE mirrors [`Priority::rank`]; the two are pinned together by
    /// `test_sql_priority_rank_matches_enum_rank`. User-defined values all land
    /// on the same `ELSE` rank, so the raw value is appended as a tiebreaker to
    /// order them lexicographically among themselves — matching what
    /// [`Self::compare_priority_values`] does in Rust.
    ///
    /// The trade this makes: a CASE is not an indexed expression, so the sort
    /// itself no longer uses the priority index and SQLite builds a transient
    /// B-tree for it. Equality *filters* on priority still hit the index, which
    /// is what keeping `resolve_field` untouched buys, and sorting a result set
    /// is the cheaper half. Revisit if priority sorts ever run over row counts
    /// where the transient sort shows up in a profile.
    fn resolve_order_field(&self, field: &str, target_type: &str, direction: &str) -> String {
        let resolved = self.resolve_field(field, target_type);

        // Scoped to the types that share the scale. A wildcard query resolves
        // the namespace from each row's own node_type, so ranking there would
        // impose the shared scale on every type's priority — including a
        // user-defined type's bare `priority`, which is its own vocabulary —
        // while compare_priority_values ranks only for these same targets. The
        // two layers would then disagree, and which one won would depend on
        // whether a LIMIT was present.
        if field == "priority" && Priority::applies_to(target_type) {
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
                Priority::ABSENT_RANK,
                Priority::Highest.rank(),
                Priority::High.rank(),
                Priority::Medium.rank(),
                Priority::Low.rank(),
                Priority::Lowest.rank(),
                Priority::USER_RANK,
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

    /// Build a related-node filter condition (`FilterType::Related`)
    ///
    /// Extends [`Self::build_relationship_condition`]'s bare-membership shape
    /// with a nested condition evaluated against the related node(s): resolve
    /// the relationship's stored type and direction (already done ahead of
    /// SQL compilation — see [`QueryFilter::resolved_relationship`]), then
    /// compile to `id_field IN (SELECT <out|in>_node FROM relationship JOIN
    /// node ON node.id = relationship.<in|out>_node WHERE relationship_type =
    /// ? AND <nested condition>)`. The nested condition is a recursive call
    /// into the same filter-building logic used for a top-level filter, using
    /// `related_type` (or, absent one, the same per-row `node_type` fallback
    /// [`Self::resolve_field`] uses under a wildcard query) to resolve the
    /// nested filter's own property paths against the JOINed row.
    ///
    /// A many-cardinality relationship needs no special casing: `IN (SELECT
    /// ...)` is already an existence test over however many rows the subquery
    /// returns, whether that is zero, one, or many — "connected to at least
    /// one project matching..." falls out of the shape for free.
    fn build_related_filter(
        &self,
        id_field: &str,
        filter: &QueryFilter,
        built: &mut BoundSql,
    ) -> Result<String> {
        let resolved = filter.resolved_relationship.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Related filter's relationshipName was not resolved before SQL compilation \
                 (expected `to_query_definition` to populate `resolved_relationship`)"
            )
        })?;
        let nested = filter
            .filter
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Related filter missing 'filter'"))?;

        validate_identifier(&resolved.stored_type, "resolved relationship type")?;
        if let Some(related_type) = &resolved.related_type {
            validate_identifier(related_type, "resolved relationship related type")?;
        }
        if let Some(source_type) = &resolved.source_type {
            validate_identifier(source_type, "resolved relationship source type")?;
        }

        // The outer node's column vs. the related node's column are opposite
        // ends of the same row — exactly the (selected, matched) split
        // `build_relationship_condition` uses, just decided by direction
        // rather than by a fixed match arm.
        let (outer_column, related_column) = if resolved.outer_is_in_node {
            ("in_node", "out_node")
        } else {
            ("out_node", "in_node")
        };

        // The nested condition is compiled as its own independent statement
        // — `SELECT id FROM node WHERE <condition>` — rather than spliced
        // into the same FROM clause as the relationship join. This is
        // deliberate, not incidental: the outer query this whole condition
        // is built for is itself `SELECT * FROM node WHERE ...`, so an
        // unqualified `node` introduced by a join here would sit in the same
        // lexical scope as the outer query's own `node` and every bare
        // column reference the nested condition builds (`properties`,
        // `content`, `created_at`, ...) would be ambiguous between the two.
        // Nesting the property condition in its own subquery instead gives
        // it a `node` with no other `node` anywhere in its scope chain, at
        // the cost of one extra (indexed, id-keyed) subquery level rather
        // than a single join.
        let mut nested_built = BoundSql::default();
        let nested_condition = match nested.filter_type {
            FilterType::Property => {
                let related_type = resolved.related_type.as_deref().unwrap_or("*");
                self.build_property_filter(nested, related_type, &mut nested_built)?
            }
            FilterType::Content => self.build_content_filter(nested, &mut nested_built)?,
            FilterType::Relationship => {
                self.build_relationship_filter(nested, &mut nested_built)?
            }
            FilterType::Metadata => self.build_metadata_filter(nested, &mut nested_built)?,
            // Rejected at validation time (`QueryFilter::validate_identifiers`'s
            // depth cap) before this method is ever reached.
            FilterType::Related => {
                anyhow::bail!("Related filter nesting depth exceeds the maximum of 1")
            }
        };
        // The nested subquery's own placeholders are renumbered onto the end
        // of the outer `built`'s parameter list — `BoundSql::bind` numbers
        // sequentially from the whole statement's start, so a subquery built
        // in isolation cannot know its final placeholder numbers up front.
        let offset = built.params.len();
        built.params.extend(nested_built.params);
        let nested_condition = renumber_placeholders(&nested_condition, offset);

        let relationship_type_placeholder =
            built.bind(libsql::Value::Text(resolved.stored_type.clone()));

        // A reverse match is already exact without a per-type filter: the
        // resolver's precedence means a reverse name is declared by exactly
        // one schema, so `relationship_type` alone (bound above) identifies
        // the edge kind precisely — no further narrowing by `related_type`
        // is needed here the way `rel_ops::get_related_nodes` narrows its
        // *returned nodes*, because this compiles a set-membership condition
        // rather than materializing rows to filter in Rust.
        //
        // `EXISTS`, not `id_field IN (SELECT ...)`: the two look
        // interchangeable but are not once the subquery is correlated back
        // to the SAME column the outer clause tests. `id IN (SELECT out_node
        // FROM relationship WHERE in_node = node.id)` asks "is this row's own
        // id a member of the set of related ids" — for row `task-a` related
        // to `proj-active`, that tests `'task-a' IN ('proj-active')`, which
        // is false for every row whose id differs from its own related
        // node's id (i.e. always, barring a self-referential edge). `id_field`
        // is not a stand-in for "this row" the way it is in
        // `build_relationship_condition`'s sibling method, where the
        // outer-side check is bound to a caller-supplied literal `node_id`
        // instead of correlated to the row under test — `EXISTS` is the
        // correct shape once the correlation and the tested column are the
        // same column.
        //
        // The correlation is written as `node.{id_field}`, explicitly
        // qualified, never a bare `{id_field}`: `relationship` has its OWN
        // `id` primary-key column (a random edge-row id, unrelated to any
        // node), so inside `EXISTS (SELECT ... FROM relationship WHERE ...)`
        // an unqualified `id` resolves to `relationship.id` — the innermost
        // table that has a same-named column — not to the outer `node.id`
        // this is meant to correlate against. That silently compared every
        // edge's own random row id to itself, matching nothing, rather than
        // raising a "no such column" error that would have caught it
        // immediately. `id_field` is always `"id"` at every call site
        // ([`Self::build_related_filter`] is invoked only from
        // [`Self::build_where_clause`] with the literal `"id"`), so it is
        // qualified against the literal `node` table here rather than
        // threading a caller-supplied qualifier through for a case that does
        // not arise.
        Ok(format!(
            "EXISTS (SELECT 1 FROM relationship \
             WHERE relationship.{outer_column} = node.{id_field} \
             AND relationship.relationship_type = {relationship_type_placeholder} \
             AND relationship.{related_column} IN (SELECT id FROM node WHERE {nested_condition}))"
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
