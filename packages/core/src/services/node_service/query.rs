//! Query operations for NodeService.

use super::*;

impl NodeService {
    /// Query nodes with filtering
    ///
    /// Executes a filtered query using NodeFilter.
    ///
    /// # Arguments
    ///
    /// * `filter` - The filter criteria
    ///
    /// # Returns
    ///
    /// Vector of matching nodes
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use nodespace_core::models::NodeFilter;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let filter = NodeFilter::new()
    ///     .with_node_type("task".to_string())
    ///     .with_limit(10);
    /// let nodes = service.query_nodes(filter).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_nodes(&self, filter: NodeFilter) -> Result<Vec<Node>, NodeServiceError> {
        // When property filters are present, fetch all matching rows from DB and
        // filter in memory. Safety cap prevents accidental OOM on large datasets.
        const PROPERTY_FILTER_FETCH_CAP: usize = 10_000;
        let (db_limit, db_offset) = if filter.property_filters.is_some() {
            (Some(PROPERTY_FILTER_FETCH_CAP), None)
        } else {
            (filter.limit, filter.offset)
        };

        // Convert NodeFilter to NodeQuery. order_by is forwarded through so the
        // store applies it in SQL (ORDER BY before LIMIT/OFFSET) rather than
        // relying on in-memory sorting that never actually happened.
        let query = crate::models::NodeQuery {
            id: None,
            ids: filter.ids.clone(),
            node_type: filter.node_type.clone(),
            content_contains: filter.content_contains.clone(),
            title_contains: filter.title_contains.clone(),
            mentioned_by: None,
            order_by: filter.order_by.clone(),
            limit: db_limit,
            offset: db_offset,
        };

        let nodes = self
            .store
            .query_nodes(query)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Apply property filters in-memory if present
        let result_nodes = if let Some(ref property_filters) = filter.property_filters {
            // The query's own `node_type` sets the read scope (ADR-078): a
            // filter authored against a base type is evaluated at that base's
            // scope even when the matched row is a descendant instance.
            // Resolved once per query, not per row.
            let scope_chain = match filter.node_type.as_deref() {
                Some(nt) if nt != "*" => Some(self.resolve_type_chain(nt).await?),
                _ => None,
            };
            let mut filtered =
                Self::apply_property_filters(nodes, property_filters, scope_chain.as_deref());
            // Apply offset in memory
            if let Some(offset) = filter.offset {
                if offset < filtered.len() {
                    filtered = filtered.split_off(offset);
                } else {
                    filtered.clear();
                }
            }
            // Apply limit in memory
            if let Some(limit) = filter.limit {
                filtered.truncate(limit);
            }
            filtered
        } else {
            nodes
        };

        Ok(result_nodes)
    }

    /// Apply property filters in-memory to a list of nodes.
    ///
    /// Properties are stored in namespaced format: `{ "task": { "status": "open" } }`.
    /// PropertyFilter paths use JSONPath: `"$.status"`.
    /// This resolves the path against each node's type namespace.
    /// `scope_chain` is the query's own read scope (ADR-078) — the chain of
    /// the type its `node_type` filter named, nearest-first. `None` falls back
    /// to each node's own type, which is the untyped-query case and the
    /// pre-`extends` behavior.
    fn apply_property_filters(
        nodes: Vec<Node>,
        filters: &[PropertyFilter],
        scope_chain: Option<&[String]>,
    ) -> Vec<Node> {
        nodes
            .into_iter()
            .filter(|node| {
                filters
                    .iter()
                    .all(|f| Self::node_matches_property_filter(node, f, scope_chain))
            })
            .collect()
    }

    /// Check if a single node matches a single property filter.
    fn node_matches_property_filter(
        node: &Node,
        filter: &PropertyFilter,
        scope_chain: Option<&[String]>,
    ) -> bool {
        // Extract property path from JSONPath "$.field" or "$.field.subfield"
        // PropertyFilter::new() validates the "$." prefix, so strip_prefix should always succeed.
        let path = match filter.path.strip_prefix("$.") {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "PropertyFilter path '{}' missing expected '$.' prefix — skipping filter",
                    filter.path
                );
                return false;
            }
        };
        let segments: Vec<&str> = path.split('.').collect();

        // Resolve value from namespaced properties, searching each bucket in
        // the query's scope chain nearest-first (ADR-078). A filter authored
        // against a base type resolves an inherited field from its declaring
        // ancestor's bucket; a field outside the scope does not resolve, so
        // the filter does not match — which is what keeps a base-scoped query
        // from depending on a subtype's own fields.
        let scope_chain = scope_chain.unwrap_or(std::slice::from_ref(&node.node_type));
        let mut current = None;
        for scope in scope_chain {
            let mut candidate = node.properties.get(scope.as_str());
            for segment in &segments {
                candidate = candidate.and_then(|v| v.get(*segment));
            }
            if candidate.is_some() {
                current = candidate;
                break;
            }
        }

        let Some(actual_value) = current else {
            return false; // Property not found = doesn't match
        };

        match &filter.operator {
            FilterOperator::Equals => actual_value == &filter.value,
            FilterOperator::NotEquals => actual_value != &filter.value,
            FilterOperator::Contains => match (actual_value.as_str(), filter.value.as_str()) {
                (Some(actual), Some(expected)) => {
                    actual.to_lowercase().contains(&expected.to_lowercase())
                }
                _ => false,
            },
            FilterOperator::StartsWith => match (actual_value.as_str(), filter.value.as_str()) {
                (Some(actual), Some(expected)) => {
                    actual.to_lowercase().starts_with(&expected.to_lowercase())
                }
                _ => false,
            },
            FilterOperator::EndsWith => match (actual_value.as_str(), filter.value.as_str()) {
                (Some(actual), Some(expected)) => {
                    actual.to_lowercase().ends_with(&expected.to_lowercase())
                }
                _ => false,
            },
            FilterOperator::GreaterThan => {
                Self::compare_property_values(actual_value, &filter.value)
                    == Some(std::cmp::Ordering::Greater)
            }
            FilterOperator::GreaterThanOrEqual => {
                matches!(
                    Self::compare_property_values(actual_value, &filter.value),
                    Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                )
            }
            FilterOperator::LessThan => {
                Self::compare_property_values(actual_value, &filter.value)
                    == Some(std::cmp::Ordering::Less)
            }
            FilterOperator::LessThanOrEqual => {
                matches!(
                    Self::compare_property_values(actual_value, &filter.value),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                )
            }
        }
    }

    /// Compare two JSON values for ordering (used by GT/LT operators)
    fn compare_property_values(
        a: &serde_json::Value,
        b: &serde_json::Value,
    ) -> Option<std::cmp::Ordering> {
        match (a, b) {
            (serde_json::Value::Number(na), serde_json::Value::Number(nb)) => {
                let fa = na.as_f64()?;
                let fb = nb.as_f64()?;
                fa.partial_cmp(&fb)
            }
            (serde_json::Value::String(sa), serde_json::Value::String(sb)) => Some(sa.cmp(sb)),
            (serde_json::Value::Bool(ba), serde_json::Value::Bool(bb)) => Some(ba.cmp(bb)),
            _ => None,
        }
    }

    /// Query nodes with simple query parameters
    ///
    /// This is a simpler alternative to `query_nodes` for common query patterns.
    /// Supports queries by ID, mentioned_by, content_contains, and node_type.
    ///
    /// # Arguments
    ///
    /// * `query` - Query parameters (see NodeQuery for details)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<Node>)` - List of matching nodes
    /// * `Err(NodeServiceError)` - If database operation fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::models::NodeQuery;
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Query by ID
    /// let query = NodeQuery::by_id("node-123".to_string());
    /// let nodes = service.query_nodes_simple(query).await?;
    ///
    /// // Query nodes that mention another node
    /// let query = NodeQuery::mentioned_by("target-node".to_string());
    /// let nodes = service.query_nodes_simple(query).await?;
    ///
    /// // Full-text search
    /// let query = NodeQuery::content_contains("search term".to_string()).with_limit(10);
    /// let nodes = service.query_nodes_simple(query).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Query Priority Order
    ///
    /// Queries are evaluated in the following priority order:
    /// 1. `id` - Direct node lookup (exact match)
    /// 2. `mentioned_by` - Nodes that reference the specified node
    /// 3. `content_contains` + optional `node_type` - Full-text content search
    /// 4. `node_type` - Filter by node type
    /// 5. Empty query - Returns empty vec (safer than returning all nodes)
    ///
    /// # Note on Empty Queries
    ///
    /// Queries with no parameters (all fields `None` or `false`) will return an empty vector.
    /// This is intentional to prevent accidentally fetching all nodes from the database.
    ///
    /// # Default Limit
    ///
    /// If no limit is specified in the query, a default limit of [`DEFAULT_QUERY_LIMIT`] (100)
    /// is applied to prevent unbounded queries and potential performance issues.
    /// Callers can override this by explicitly setting a limit via `query.with_limit(n)`.
    /// Project a query's results to the queried type's scope (ADR-078).
    ///
    /// Returns each node with its properties reduced to the buckets visible at
    /// `node_type`'s scope, so a `task`-scoped query yields rows carrying
    /// task's fields and nothing else, whatever their concrete type. Querying
    /// a type that extends nothing, or with no type filter, returns the nodes
    /// untouched.
    ///
    /// Deliberately **not** applied inside `query_nodes_simple` itself. A
    /// projected node has had properties removed from the in-memory struct, so
    /// a caller that reads, mutates and writes one back would silently drop
    /// the fields outside its read scope — and there are such callers
    /// (`skill_updater` round-trips a node it queried). Projection belongs at
    /// a boundary where results are leaving for a client and cannot be written
    /// back, so it is offered here and applied by the daemon's read RPCs
    /// rather than imposed on every internal query.
    pub async fn project_nodes_to_scope(
        &self,
        nodes: Vec<Node>,
        node_type: Option<&str>,
    ) -> Result<Vec<Node>, NodeServiceError> {
        let Some(nt) = node_type.filter(|nt| *nt != "*") else {
            return Ok(nodes);
        };

        // The scope is the QUERIED type's own chain — `["ticket"]` for an
        // unextended base, `["bug", "ticket"]` when the query itself names a
        // subtype. Note this is the queried type's ancestry, not the matched
        // node's: projecting a bug at ticket scope means keeping ticket's
        // buckets, and ticket's chain is what names them.
        let chain = self.resolve_type_chain(nt).await?;
        let scopes: Vec<&str> = chain.iter().map(String::as_str).collect();

        Ok(nodes
            .into_iter()
            .map(|mut node| {
                // A node of exactly the queried type carries only buckets
                // already in scope, so projecting it is the identity — skip
                // the rebuild rather than reallocate every row of an
                // unextended query.
                if node.node_type != nt {
                    node.properties = Self::project_properties_to_scope(&node.properties, &scopes);
                }
                node
            })
            .collect())
    }

    /// Keep only the buckets named in `scopes`, plus `_`-prefixed bookkeeping.
    ///
    /// Storage shape is preserved rather than flattened: the wire layer
    /// flattens separately, and returning a flattened object here would make a
    /// projected node structurally different from an unprojected one.
    fn project_properties_to_scope(
        properties: &serde_json::Value,
        scopes: &[&str],
    ) -> serde_json::Value {
        let Some(obj) = properties.as_object() else {
            return properties.clone();
        };

        serde_json::Value::Object(
            obj.iter()
                .filter(|(k, _)| k.starts_with('_') || scopes.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    }

    pub async fn query_nodes_simple(
        &self,
        query: crate::models::NodeQuery,
    ) -> Result<Vec<Node>, NodeServiceError> {
        // Direct delegation to store.query_nodes for simple queries
        // Complex filtering handled by the SQLite query engine
        tracing::debug!("query_nodes_simple: Delegating to store.query_nodes");

        // Priority 1: Query by ID (exact match)
        if let Some(ref id) = query.id {
            if let Some(node) = self.get_node(id).await? {
                return Ok(vec![node]);
            } else {
                return Ok(vec![]);
            }
        }

        // Apply default limit if not specified to prevent unbounded queries
        let query = if query.limit.is_none() {
            query.with_limit(DEFAULT_QUERY_LIMIT)
        } else {
            query
        };

        // Priority 2+: Delegate to store.query_nodes
        // Complex query features (mentioned_by, content_contains, filters) delegated to store
        let nodes = self
            .store
            .query_nodes(query)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        Ok(nodes)
    }

    /// Count nodes matching `query` without fetching the matching
    /// records — the O(1)-response-size counterpart to
    /// `query_nodes_simple`, for callers (e.g. `nodespace diagnostics`) that
    /// only need a total. Mirrors `query_nodes_simple`'s id-lookup priority
    /// tier so the two never disagree on what an `id`-only query means, then
    /// delegates the rest to `store.count_nodes`. `limit`/`offset`/`order_by`
    /// on `query` are ignored — meaningless for a scalar count.
    pub async fn count_nodes(
        &self,
        query: crate::models::NodeQuery,
    ) -> Result<i64, NodeServiceError> {
        // Priority 1: Query by ID (exact match) — mirrors query_nodes_simple.
        if let Some(ref id) = query.id {
            return Ok(if self.get_node(id).await?.is_some() {
                1
            } else {
                0
            });
        }

        self.store
            .count_nodes(&query)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }
}

impl NodeService {
    /// Search nodes for mention autocomplete with proper filtering
    pub async fn mention_autocomplete(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Node>, NodeServiceError> {
        self.store
            .mention_autocomplete(query, limit.map(|l| l as i64))
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }
}
