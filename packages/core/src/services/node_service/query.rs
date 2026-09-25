//! Query operations for NodeService.

use super::*;

/// A query's read scope (ADR-078): which buckets its results are read from,
/// and the field definitions needed to resolve extended enum values back to
/// the vocabulary the query's author could know about.
///
/// Built once per query by `build_scope_context` and consulted per row. Every
/// schema read happens during construction, so the per-row filter path stays
/// synchronous and store-free — filter evaluation is per-row, and a schema
/// read inside that loop would be one round-trip per matched node.
pub(crate) struct ScopeContext {
    /// The queried type's own chain, nearest-first — the buckets in scope.
    chain: Vec<String>,
    /// The type the query named, i.e. the scope values resolve *to*.
    scope_type: String,
    /// Effective fields at the queried scope: what vocabulary a filter
    /// authored against this type can refer to.
    scope_fields: Vec<crate::models::SchemaField>,
    /// Effective fields per descendant type, keyed by node_type — where the
    /// `maps_to` declarations live. Only holds types that differ from the
    /// queried one; an exact-type match needs no resolution.
    node_fields: std::collections::HashMap<String, Vec<crate::models::SchemaField>>,
}

impl ScopeContext {
    /// The buckets in scope, nearest-first.
    fn chain(&self) -> &[String] {
        &self.chain
    }

    /// Whether the queried scope declares this field — i.e. whether a filter
    /// authored at this scope is entitled to read it at all.
    fn declares_field(&self, name: &str) -> bool {
        self.scope_fields.iter().any(|f| f.name == name)
    }

    /// Whether a node of this type could carry a value needing resolution.
    ///
    /// False for a node of exactly the queried type — it is already reading at
    /// its native scope — and for any type with no pre-resolved fields, which
    /// is every type when nothing extends the queried one.
    fn may_resolve_values(&self, node_type: &str) -> bool {
        node_type != self.scope_type && self.node_fields.contains_key(node_type)
    }

    /// Resolve one stored value into the queried scope's vocabulary, or `None`
    /// if it cannot be expressed there.
    fn resolve_value(&self, field: &str, stored: &str, node_type: &str) -> Option<String> {
        let node_fields = self.node_fields.get(node_type)?;
        crate::schema::extends_chain::resolve_value_at_scope(
            field,
            stored,
            node_fields,
            &self.scope_fields,
        )
    }
}

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
        // Property filters are evaluated in memory (ADR-078 scope resolution
        // needs per-row schema context SQL can't express), so offset/limit can
        // only apply AFTER filtering: fetch the whole type-scoped set, unpaged.
        // A fetch cap here would silently drop every match past it with no
        // signal to the caller.
        let (db_limit, db_offset) = if filter.property_filters.is_some() {
            (None, None)
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
            let scope = self
                .build_scope_context(filter.node_type.as_deref())
                .await?;
            let mut filtered =
                Self::apply_property_filters(nodes, property_filters, scope.as_ref());
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

    /// Build the read scope for a query's `node_type`, if it names one
    /// (ADR-078).
    ///
    /// Resolved once per query, never per row — filter evaluation is per-row,
    /// so a schema read inside the row loop would turn an in-memory filter into
    /// one round-trip per matched node. Returns `None` when there is nothing to
    /// scope by (no type filter, the `*` wildcard) or when no schema extends
    /// anything, in which case filtering keeps its pre-`extends` behavior.
    async fn build_scope_context(
        &self,
        node_type: Option<&str>,
    ) -> Result<Option<ScopeContext>, NodeServiceError> {
        let Some(nt) = node_type.filter(|nt| *nt != "*") else {
            return Ok(None);
        };

        let chain = self.resolve_type_chain(nt).await?;
        let scope_fields = self.resolve_field_owners(nt).await?.0;

        // Pre-resolve the effective fields of every type that could appear in
        // this query's results — the descendant closure — because `maps_to`
        // resolution needs the *node's* field definitions (where the mapping
        // lives) and runs inside a synchronous filter with no store access.
        // The closure is exactly the set the query engine already expanded the
        // type filter into, so this adds no rows, and it is empty of extra
        // work whenever nothing extends the queried type.
        let mut node_fields = std::collections::HashMap::new();
        for subtype in self.store.get_subtype_closure(nt).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to resolve subtypes for scope: {e}"))
        })? {
            if subtype == nt {
                continue;
            }
            let fields = self.resolve_field_owners(&subtype).await?.0;
            node_fields.insert(subtype, fields);
        }

        Ok(Some(ScopeContext {
            chain,
            scope_type: nt.to_string(),
            scope_fields,
            node_fields,
        }))
    }

    /// Apply property filters in-memory to a list of nodes.
    ///
    /// Properties are stored in namespaced format: `{ "task": { "status": "open" } }`.
    /// PropertyFilter paths use JSONPath: `"$.status"`.
    /// This resolves the path against each node's type namespace.
    /// `scope` is the query's own read scope (ADR-078) — resolved once per
    /// query, not per row. `None` falls back to each node's own type, which is
    /// the untyped-query case and the pre-`extends` behavior.
    fn apply_property_filters(
        nodes: Vec<Node>,
        filters: &[PropertyFilter],
        scope: Option<&ScopeContext>,
    ) -> Vec<Node> {
        nodes
            .into_iter()
            .filter(|node| {
                filters
                    .iter()
                    .all(|f| Self::node_matches_property_filter(node, f, scope))
            })
            .collect()
    }

    /// Check if a single node matches a single property filter.
    fn node_matches_property_filter(
        node: &Node,
        filter: &PropertyFilter,
        scope: Option<&ScopeContext>,
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

        // Resolve the value from namespaced properties (ADR-078).
        //
        // Which buckets to search is decided by whether the field exists at
        // the query's scope at all, NOT by the query's bucket chain alone. A
        // field the query's scope declares may physically live in a subtype's
        // bucket: extending an inherited enum materializes the field onto the
        // extending schema, which makes that schema its declaring owner and
        // moves where instances store it. Searching only the query's chain
        // would miss exactly the values `maps_to` exists to translate.
        //
        // A field the query's scope does NOT declare stays invisible, which is
        // what keeps a base-scoped query from depending on a subtype's own
        // fields.
        let own_chain = std::slice::from_ref(&node.node_type);
        let search_chain = match (scope, segments.as_slice()) {
            (Some(ctx), [field]) if ctx.declares_field(field) => own_chain,
            (Some(ctx), _) => ctx.chain(),
            (None, _) => own_chain,
        };
        let mut current = None;
        for scope_name in search_chain {
            let mut candidate = node.properties.get(scope_name.as_str());
            for segment in &segments {
                candidate = candidate.and_then(|v| v.get(*segment));
            }
            if candidate.is_some() {
                current = candidate;
                break;
            }
        }
        // Fall back to every bucket when the scope declares the field but the
        // node's own chain does not hold it — a deeper descendant may own it.
        if current.is_none() {
            if let (Some(ctx), [field]) = (scope, segments.as_slice()) {
                if ctx.declares_field(field) {
                    current = node
                        .properties
                        .as_object()
                        .and_then(|obj| obj.values().find_map(|b| b.get(field)));
                }
            }
        }

        let Some(actual_value) = current else {
            return false; // Property not found = doesn't match
        };

        // Resolve an extended enum value to what it means at the query's scope
        // (ADR-078). A filter authored against a base type compares against
        // that type's vocabulary, so an `issue` node storing `backlog` must
        // compare as `todo` — the value the filter's author could know about.
        // An unresolvable value fails the filter rather than falling back to
        // the raw value, which a base-scoped filter has no way to interpret.
        //
        // Only single-segment paths resolve: `maps_to` maps a field's enum
        // values, not positions inside a nested object.
        let resolved;
        let actual_value = match (scope, actual_value.as_str(), segments.as_slice()) {
            (Some(ctx), Some(raw), [field]) if ctx.may_resolve_values(&node.node_type) => {
                match ctx.resolve_value(field, raw, &node.node_type) {
                    Some(value) => {
                        resolved = serde_json::Value::String(value);
                        &resolved
                    }
                    None => return false,
                }
            }
            _ => actual_value,
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

    /// Fold a node's inherited buckets into its own, for the wire.
    ///
    /// Read surfaces outside this crate — the CLI, the wire flattener — have
    /// no store access and so cannot resolve an `extends` chain. They flatten
    /// a single bucket, which is correct for an unextended node and drops
    /// every inherited field for an extending one.
    ///
    /// Collapsing the chain here, where the chain *is* known, lets those
    /// surfaces keep their single-bucket rule unchanged. Crucially it also
    /// keeps them able to distinguish a dormant bucket (left by an earlier
    /// `node_type` change) from an inherited one: a dormant bucket is not in
    /// the chain, so it is neither folded in nor exposed — the behavior
    /// `node_to_json_hides_dormant_namespaces` pins.
    ///
    /// The node's own bucket wins any collision, matching nearest-scope-first.
    pub async fn collapse_chain_for_wire(
        &self,
        nodes: Vec<Node>,
    ) -> Result<Vec<Node>, NodeServiceError> {
        // One query for every `extends` edge, then resolve each node's chain
        // in memory. Doing this per node would mean a full scan of the edge
        // table per row — 501 queries for a 500-row result, on the frontend's
        // main read path. An empty map also answers the existence check, so
        // this replaces the separate `has_any_extends_edge` guard rather than
        // adding to it.
        let parent_map = self
            .store
            .get_extends_parent_map()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        if parent_map.is_empty() {
            return Ok(nodes);
        }
        let lookup = move |id: &str| parent_map.get(id).cloned();

        // Chains are memoized across rows: a result set is typically a handful
        // of distinct types over many nodes.
        let mut chains: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        let mut out = Vec::with_capacity(nodes.len());
        for mut node in nodes {
            let chain = chains.entry(node.node_type.clone()).or_insert_with(|| {
                crate::schema::extends_chain::resolve_ancestor_chain(&node.node_type, &lookup)
            });
            if chain.len() > 1 {
                node.properties = Self::collapse_properties(&node.properties, chain);
            }
            out.push(node);
        }
        Ok(out)
    }

    /// Merge each in-chain bucket into the node's own, nearest scope winning.
    fn collapse_properties(properties: &serde_json::Value, chain: &[String]) -> serde_json::Value {
        let Some(obj) = properties.as_object() else {
            return properties.clone();
        };
        let Some(own_type) = chain.first() else {
            return properties.clone();
        };

        let mut own = serde_json::Map::new();
        for scope in chain {
            let Some(bucket) = obj.get(scope.as_str()).and_then(|v| v.as_object()) else {
                continue;
            };
            for (k, v) in bucket {
                own.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }

        let mut out = serde_json::Map::new();
        for (k, v) in obj {
            // Ancestor buckets are now represented inside the own bucket;
            // anything else (bookkeeping, dormant namespaces) passes through
            // untouched so downstream rules about it still apply.
            if chain.iter().any(|s| s == k) {
                continue;
            }
            out.insert(k.clone(), v.clone());
        }
        out.insert(own_type.clone(), serde_json::Value::Object(own));

        serde_json::Value::Object(out)
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

#[cfg(test)]
mod scope_context_tests {
    //! `ScopeContext` resolves every schema read up front, so the per-row
    //! filter path stays synchronous and store-free (ADR-078).
    //!
    //! Property filtering runs per row. A schema read inside that loop would
    //! turn an in-memory filter into one DB round-trip per matched node —
    //! 10,000 of them at the fetch cap. The design has this property today;
    //! these pin it so a future edit that reintroduces an `await` there fails
    //! here rather than silently regressing into per-row I/O.

    use super::*;

    /// The signature `apply_property_filters` must keep: synchronous, and
    /// carrying no `NodeService` or store handle.
    type ApplyFilters = fn(Vec<Node>, &[PropertyFilter], Option<&ScopeContext>) -> Vec<Node>;

    /// The signature `node_matches_property_filter` must keep — the body that
    /// runs once per row.
    type MatchesFilter = fn(&Node, &PropertyFilter, Option<&ScopeContext>) -> bool;

    /// The filter path must stay a plain synchronous function that receives
    /// no `NodeService` and no store handle.
    ///
    /// This is a **compile-time** assertion, and that is the whole point:
    /// adding an `.await` inside `node_matches_property_filter` makes it
    /// `async`, which changes its type from `fn(..) -> bool` to
    /// `fn(..) -> impl Future`, and these coercions stop compiling. A runtime
    /// test cannot catch that — there is no store handle in scope to count
    /// queries against, precisely because the signature does not carry one.
    ///
    /// Taking the functions as `fn` pointers (not closures) is what makes the
    /// check bite: a closure would coerce around an added parameter, a bare
    /// `fn` item will not.
    #[test]
    fn filter_path_is_synchronous_and_store_free() {
        // `apply_property_filters` — the per-row loop's owner.
        let _apply: ApplyFilters = NodeService::apply_property_filters;

        // `node_matches_property_filter` — the body evaluated once per row.
        let _matches: MatchesFilter = NodeService::node_matches_property_filter;
    }

    /// Every `ScopeContext` field is populated by construction, so nothing the
    /// per-row path consults can be lazily fetched later.
    ///
    /// The lazy-fetch shape is the realistic regression: not an `await` added
    /// to the filter body outright, but a field changed to `Option<_>` and
    /// filled on first use, which would need store access from inside the row
    /// loop. Reading every accessor off a fully-constructed context — with no
    /// `NodeService` anywhere in scope — pins that they answer from owned
    /// data alone.
    #[test]
    fn scope_context_answers_every_read_from_owned_data() {
        let enum_field = crate::models::SchemaField {
            name: "state".to_string(),
            ..Default::default()
        };

        let ctx = ScopeContext {
            chain: vec!["ticket".to_string(), "workitem".to_string()],
            scope_type: "ticket".to_string(),
            scope_fields: vec![enum_field.clone()],
            node_fields: std::collections::HashMap::from([("bug".to_string(), vec![enum_field])]),
        };

        assert_eq!(ctx.chain(), ["ticket", "workitem"]);
        assert!(ctx.declares_field("state"));
        assert!(!ctx.declares_field("severity"));

        // A node of exactly the queried type reads natively — no resolution.
        assert!(!ctx.may_resolve_values("ticket"));
        // A type with no pre-resolved fields has nothing to resolve through.
        assert!(!ctx.may_resolve_values("unrelated"));
        // A descendant carrying pre-resolved fields does.
        assert!(ctx.may_resolve_values("bug"));

        // `resolve_value` reaches only into `node_fields`/`scope_fields`; an
        // unknown type is `None` rather than a lookup.
        assert_eq!(ctx.resolve_value("state", "open", "unknown"), None);
    }
}
