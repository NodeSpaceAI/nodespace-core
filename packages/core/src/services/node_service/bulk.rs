//! Bulk operations for NodeService.

use super::*;

impl NodeService {
    /// Bulk create multiple nodes in a transaction
    ///
    /// Creates multiple nodes atomically. If any node fails validation or insertion,
    /// the entire transaction is rolled back.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of nodes to create
    ///
    /// # Returns
    ///
    /// Vector of created node IDs in the same order as input
    ///
    /// # Errors
    ///
    /// Returns error if any node fails validation or insertion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use nodespace_core::models::Node;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let nodes = vec![
    ///     Node::new("text".to_string(), "Note 1".to_string(), json!({})),
    ///     Node::new("text".to_string(), "Note 2".to_string(), json!({})),
    /// ];
    /// let ids = service.bulk_create(nodes).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_create(&self, nodes: Vec<Node>) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Validate all nodes first (two-step validation)
        for node in &nodes {
            // Step 1: Core behavior validation
            self.behaviors.validate_node(node)?;

            // Step 2: Schema validation
            if node.node_type != "schema" {
                self.validate_node_against_schema(node).await?;
            }
        }

        // Call store trait to execute batch insert in transaction
        let created_nodes = self
            .store
            .batch_create_nodes(nodes)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // NOTE: NodeCreated events are now automatically emitted by store notifier

        // Extract IDs for return (maintaining backward compatibility)
        Ok(created_nodes.into_iter().map(|n| n.id).collect())
    }

    /// Bulk create nodes with hierarchy in a single transaction
    ///
    /// Creates multiple nodes with parent-child relationships atomically.
    /// This method is optimized for markdown import where all node data
    /// (IDs, hierarchy, ordering) is pre-calculated.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of tuples: (id, node_type, content, parent_id, order, properties)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<String>)` - Vector of created node IDs in insertion order
    /// * `Err` - If validation or transaction fails
    ///
    /// # Performance
    ///
    /// This method provides ~10-15x speedup over sequential node creation
    /// by batching all database operations into a single transaction.
    pub async fn bulk_create_hierarchy(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>, NodeServiceError> {
        let Some(nodes_normalized) = self.prepare_bulk_hierarchy_nodes(nodes).await? else {
            return Ok(Vec::new());
        };

        // Find the root ID once - all nodes in a bulk import share the same root
        // Performance optimization: Single DB query instead of N queries
        let root_id = if let Some((_, _, _, Some(first_parent), _, _)) = nodes_normalized.first() {
            self.get_root_id(first_parent).await.ok()
        } else {
            None
        };

        // Delegate to store for atomic batch insert
        let result = self
            .store
            .bulk_create_hierarchy(nodes_normalized)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Queue root for embedding regeneration once
        // All nodes share the same root, so we only need one queue operation
        #[cfg(feature = "nlp")]
        if let Some(root_id) = root_id {
            self.queue_root_for_embedding(&root_id).await;
        }

        Ok(result)
    }

    /// Shared preamble for [`Self::bulk_create_hierarchy`] and
    /// [`Self::bulk_create_hierarchy_in_tx`]: caches each unique node
    /// type's schema fields, normalizes flat properties to namespaced
    /// format, and validates every node against behaviors and (where
    /// applicable) its cached schema. Returns `Ok(None)` for an empty
    /// input (both callers treat that as "nothing to do"), otherwise the
    /// normalized, validated node tuples ready for insertion.
    async fn prepare_bulk_hierarchy_nodes(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<
        Option<
            Vec<(
                String,
                String,
                String,
                Option<String>,
                f64,
                serde_json::Value,
            )>,
        >,
        NodeServiceError,
    > {
        if nodes.is_empty() {
            return Ok(None);
        }

        // Performance optimization: Cache schema lookups by node_type
        // Instead of querying the database for each node, we query once per unique type
        let unique_types: std::collections::HashSet<&str> = nodes
            .iter()
            .map(|(_, node_type, _, _, _, _)| node_type.as_str())
            .collect();

        // Pre-fetch schemas for all unique types (excluding "schema" type itself)
        let mut schema_cache: std::collections::HashMap<
            String,
            Option<Vec<crate::models::SchemaField>>,
        > = std::collections::HashMap::new();
        for node_type in unique_types {
            if node_type != "schema" {
                let fields = match self.get_schema_for_type(node_type).await? {
                    Some(schema_json) => match schema_json.get("fields") {
                        Some(fields_json) => serde_json::from_value(fields_json.clone()).ok(),
                        None => None,
                    },
                    None => None,
                };
                schema_cache.insert(node_type.to_string(), fields);
            }
        }

        // Normalize flat properties to namespaced format before validation
        // Parser emits: { "status": "open" }
        // Storage expects: { "task": { "status": "open" } }
        let nodes_normalized: Vec<_> = nodes
            .into_iter()
            .map(|(id, node_type, content, parent_id, order, properties)| {
                let normalized_props =
                    Self::normalize_flat_properties_to_namespace(&node_type, &properties);
                (id, node_type, content, parent_id, order, normalized_props)
            })
            .collect();

        // Validate all nodes before insertion using cached schemas
        for (id, node_type, content, _, _, properties) in &nodes_normalized {
            // Build temporary Node for validation
            let temp_node = Node {
                id: id.clone(),
                node_type: node_type.clone(),
                content: content.clone(),
                version: 1,
                properties: properties.clone(),
                mentions: vec![],
                mentioned_in: vec![],
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                title: None, // Bulk nodes don't need titles (validated only)
                lifecycle_status: "active".to_string(),
            };

            // Validate via behaviors
            self.behaviors.validate_node(&temp_node)?;

            // Validate against cached schema (skip for schema nodes themselves)
            if node_type != "schema" {
                if let Some(Some(fields)) = schema_cache.get(node_type) {
                    self.validate_node_with_fields(&temp_node, fields)?;
                }
            }
        }

        Ok(Some(nodes_normalized))
    }

    /// `_in_tx` twin of [`Self::bulk_create_hierarchy`] (ADR-069 §1b/S3).
    /// Shares the same schema-cache/validation preamble via
    /// [`Self::prepare_bulk_hierarchy_nodes`]; the insert lands on
    /// `tx.store_tx()` via the store's own `bulk_create_hierarchy_in_tx`
    /// instead of opening a new transaction — this is what lets
    /// `create_description_subtree` compose into `handle_create_schema`'s
    /// outer transaction alongside the schema node and its relationship
    /// declarations. Root embedding-queueing is intentionally NOT
    /// reproduced here: it is derived state outside the boundary by design
    /// (ADR-069 §5) and the one current caller's root here is a schema
    /// node's description subtree, which is not itself embedded — a future
    /// caller that needs it should queue after `with_transaction` commits.
    /// Emits one `NodeCreated` event per inserted node, buffered the same
    /// way `create_node_in_tx` buffers its own.
    pub(crate) async fn bulk_create_hierarchy_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>, NodeServiceError> {
        let Some(nodes_normalized) = self.prepare_bulk_hierarchy_nodes(nodes).await? else {
            return Ok(Vec::new());
        };

        let node_types: Vec<String> = nodes_normalized
            .iter()
            .map(|(_, node_type, ..)| node_type.clone())
            .collect();

        let result = self
            .store
            .bulk_create_hierarchy_in_tx(tx.store_tx(), nodes_normalized)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        for (id, node_type) in result.iter().zip(node_types.iter()) {
            self.emit_event(DomainEvent::NodeCreated {
                node_id: id.clone(),
                node_type: node_type.clone(),
            });
        }

        Ok(result)
    }

    /// Bulk create nodes with root-only notification (for large imports)
    ///
    /// Same as `bulk_create_hierarchy` but only emits domain events for the root node,
    /// making it more efficient for bulk import scenarios where per-node notifications
    /// would overwhelm the system.
    pub async fn bulk_create_hierarchy_root_notify(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Performance optimization: Cache schema lookups by node_type
        let unique_types: std::collections::HashSet<&str> = nodes
            .iter()
            .map(|(_, node_type, _, _, _, _)| node_type.as_str())
            .collect();

        // Pre-fetch schemas for all unique types (excluding "schema" type itself)
        let mut schema_cache: std::collections::HashMap<
            String,
            Option<Vec<crate::models::SchemaField>>,
        > = std::collections::HashMap::new();
        for node_type in unique_types {
            if node_type != "schema" {
                let fields = match self.get_schema_for_type(node_type).await? {
                    Some(schema_json) => match schema_json.get("fields") {
                        Some(fields_json) => serde_json::from_value(fields_json.clone()).ok(),
                        None => None,
                    },
                    None => None,
                };
                schema_cache.insert(node_type.to_string(), fields);
            }
        }

        // Normalize flat properties to namespaced format before validation
        // Parser emits: { "status": "open" }
        // Storage expects: { "task": { "status": "open" } }
        let nodes_normalized: Vec<_> = nodes
            .into_iter()
            .map(|(id, node_type, content, parent_id, order, properties)| {
                let normalized_props =
                    Self::normalize_flat_properties_to_namespace(&node_type, &properties);
                (id, node_type, content, parent_id, order, normalized_props)
            })
            .collect();

        // Validate all nodes before insertion using cached schemas
        for (id, node_type, content, _, _, properties) in &nodes_normalized {
            let temp_node = Node {
                id: id.clone(),
                node_type: node_type.clone(),
                content: content.clone(),
                version: 1,
                properties: properties.clone(),
                mentions: vec![],
                mentioned_in: vec![],
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                title: None,
                lifecycle_status: "active".to_string(),
            };

            self.behaviors.validate_node(&temp_node)?;

            if node_type != "schema" {
                if let Some(Some(fields)) = schema_cache.get(node_type) {
                    self.validate_node_with_fields(&temp_node, fields)?;
                }
            }
        }

        // Find the root ID once
        let root_id = if let Some((_, _, _, Some(first_parent), _, _)) = nodes_normalized.first() {
            self.get_root_id(first_parent).await.ok()
        } else {
            None
        };

        // Delegate to store - use root-only notify variant
        let result = self
            .store
            .bulk_create_hierarchy_root_notify(nodes_normalized, vec![])
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Queue root for embedding regeneration once
        #[cfg(feature = "nlp")]
        if let Some(root_id) = root_id {
            self.queue_root_for_embedding(&root_id).await;
        }

        Ok(result)
    }

    /// Bulk create nodes with trusted input (skips schema validation)
    ///
    /// Optimized for import paths where the source is trusted (like markdown parser).
    /// This method:
    /// - Normalizes flat properties to namespaced format
    /// - Skips schema DB queries (no lookup overhead)
    /// - Skips schema validation (parser output is trusted)
    /// - Still validates via behaviors (type-specific rules)
    ///
    /// # Import Pipeline Optimization
    ///
    /// The markdown parser only creates known node types with correct properties:
    /// - Task nodes get `{"status": "open"}`
    /// - Header, text, code-block nodes get `{}`
    ///
    /// Since the parser is trusted, we skip the expensive schema lookup and
    /// validation, but still normalize properties to the correct storage format.
    ///
    /// # Arguments
    ///
    /// * `nodes` - Vector of (id, node_type, content, parent_id, order, properties) tuples
    ///
    /// # Returns
    ///
    /// Vector of created node IDs
    pub async fn bulk_create_hierarchy_trusted(
        &self,
        nodes: Vec<(
            String,
            String,
            String,
            Option<String>,
            f64,
            serde_json::Value,
        )>,
    ) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Normalize flat properties to namespaced format
        // Parser emits: { "status": "open" }
        // Storage expects: { "task": { "status": "open" } }
        // No schema fields needed - import properties are always simple values
        let nodes_normalized: Vec<_> = nodes
            .into_iter()
            .map(|(id, node_type, content, parent_id, order, properties)| {
                let normalized_props =
                    Self::normalize_flat_properties_to_namespace(&node_type, &properties);
                (id, node_type, content, parent_id, order, normalized_props)
            })
            .collect();

        // Validate via behaviors only (type-specific rules, no schema)
        for (id, node_type, content, _, _, properties) in &nodes_normalized {
            let temp_node = Node {
                id: id.clone(),
                node_type: node_type.clone(),
                content: content.clone(),
                version: 1,
                properties: properties.clone(),
                mentions: vec![],
                mentioned_in: vec![],
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                title: None,
                lifecycle_status: "active".to_string(),
            };

            // Only behavior validation - skip schema validation
            self.behaviors.validate_node(&temp_node)?;
        }

        // Collect embeddable root node IDs (nodes with no parent AND embeddable type)
        // Only these need embedding markers - matches single-create logic
        let root_ids: Vec<String> = nodes_normalized
            .iter()
            .filter_map(|(id, node_type, _, parent_id, _, _)| {
                if parent_id.is_none() && self.is_embeddable_type(node_type) {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();

        // Coalesce per-node Created events into one per root node.
        // Without the guard, bulk_create_hierarchy fires one store notification per
        // inserted node, flooding WatchNodes subscribers on large imports.
        let _batch = self.begin_batch_emit();

        // Delegate to store (fires one notification per inserted node at the store
        // layer; the batch guard above coalesces them into a single flush on drop).
        let result = self
            .store
            .bulk_create_hierarchy_root_notify(nodes_normalized, vec![])
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Create stale embedding markers in bulk (single transaction)
        if !root_ids.is_empty() {
            match self
                .store
                .create_stale_embedding_markers_bulk(&root_ids)
                .await
            {
                Ok(count) => {
                    tracing::debug!("Created {} stale embedding markers", count);
                    // Wake the embedding processor once for all new roots
                    #[cfg(feature = "nlp")]
                    if let Some(waker) = self.embedding_waker.get() {
                        tracing::debug!(
                            "🔔 Waking embedding processor for {} bulk-imported roots",
                            count
                        );
                        waker.wake();
                    }
                }
                Err(e) => {
                    // Log but don't fail - embeddings can be regenerated later
                    tracing::warn!("Failed to create stale embedding markers: {}", e);
                }
            }
        }

        Ok(result)
    }

    /// Bulk update multiple nodes in a transaction
    ///
    /// Updates multiple nodes atomically using a map of node IDs to NodeUpdate structs.
    ///
    /// # Arguments
    ///
    /// * `updates` - Vector of (node_id, NodeUpdate) tuples
    ///
    /// # Errors
    ///
    /// Returns error if any update fails. Transaction is rolled back on failure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let updates = vec![
    ///     ("node-1".to_string(), NodeUpdate::new().with_content("Updated 1".to_string())),
    ///     ("node-2".to_string(), NodeUpdate::new().with_content("Updated 2".to_string())),
    /// ];
    /// service.bulk_update(updates).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_update(
        &self,
        updates: Vec<(String, NodeUpdate)>,
    ) -> Result<(), NodeServiceError> {
        if updates.is_empty() {
            return Ok(());
        }

        // Step 1: Batch-fetch all nodes in a single query
        // This replaces the N+1 pattern where we called get_node() for each update
        let ids: Vec<String> = updates.iter().map(|(id, _)| id.clone()).collect();
        let existing_nodes = self.store.get_nodes_by_ids(&ids).await.map_err(|e| {
            NodeServiceError::bulk_operation_failed(format!(
                "Failed to batch fetch nodes for validation: {}",
                e
            ))
        })?;

        // Step 2: Build the MERGED update candidate for each node, validate it, and
        // record the property-change set for the event.
        //
        // Previously bulk_update wholesale-REPLACED properties with the raw
        // client value (`updated.properties = properties.clone()`) and emitted an
        // empty `changed_properties`. That diverged from the single-update path
        // (which normalizes flat client props → deep-merges into the existing
        // namespaced props) AND silently no-opped every property-change-driven
        // subscriber/play rule. Mirror single-update here: normalize + deep-merge,
        // validate the merged candidate, persist the merged value, and emit the real
        // `changed_properties` computed from old→new.
        //
        // Validation snapshot is taken before the store transaction; any concurrent
        // write landing between here and store.bulk_update is overwritten — intentional
        // under the last-write-wins contract (see store doc).
        let mut merged_updates: Vec<(String, crate::models::NodeUpdate)> =
            Vec::with_capacity(updates.len());
        let mut pending_events: Vec<(String, Node, Vec<crate::db::events::PropertyChange>)> =
            Vec::with_capacity(updates.len());

        for (id, update) in &updates {
            let existing = existing_nodes
                .get(id)
                .ok_or_else(|| NodeServiceError::node_not_found(id))?;

            let mut updated = existing.clone();
            if let Some(node_type) = &update.node_type {
                updated.node_type = node_type.clone();
            }
            if let Some(content) = &update.content {
                updated.content = content.clone();
            }

            // NOTE: Sibling ordering is handled via the has_child order field; bulk
            // updates don't reorder — use move_node.

            let mut changed_properties = Vec::new();
            if let Some(properties) = &update.properties {
                let old_props = updated.properties.clone();
                if updated.node_type == "schema" {
                    // Schema nodes use a flat (non-namespaced) format — deep-merge as-is.
                    Self::deep_merge_namespaced_properties(
                        &mut updated.properties,
                        properties.clone(),
                    );
                } else {
                    let normalized = Self::normalize_flat_properties_to_namespace(
                        &updated.node_type,
                        properties,
                    );
                    Self::deep_merge_namespaced_properties(&mut updated.properties, normalized);
                }
                changed_properties =
                    super::compute_property_changes(&old_props, &updated.properties);
            }

            // Validate the MERGED candidate (PROTECTED + USER-EXTENSIBLE rules).
            self.behaviors.validate_node(&updated).map_err(|e| {
                NodeServiceError::bulk_operation_failed(format!(
                    "Failed to validate node {}: {}",
                    id, e
                ))
            })?;
            if updated.node_type != "schema" {
                self.validate_node_against_schema(&updated)
                    .await
                    .map_err(|e| {
                        NodeServiceError::bulk_operation_failed(format!(
                            "Failed schema validation for node {}: {}",
                            id, e
                        ))
                    })?;
            }

            // Persist the caller's intent for type/content/title/lifecycle, but the
            // MERGED value for properties (so the stored row matches single-update).
            merged_updates.push((
                id.clone(),
                crate::models::NodeUpdate {
                    node_type: update.node_type.clone(),
                    content: update.content.clone(),
                    properties: update
                        .properties
                        .as_ref()
                        .map(|_| updated.properties.clone()),
                    title: update.title.clone(),
                    lifecycle_status: update.lifecycle_status.clone(),
                },
            ));
            pending_events.push((id.clone(), updated, changed_properties));
        }

        // Step 3: All validations passed — perform the atomic bulk update.
        self.store.bulk_update(merged_updates).await.map_err(|e| {
            NodeServiceError::bulk_operation_failed(format!(
                "Failed to execute bulk update transaction: {}",
                e
            ))
        })?;

        // Emit one NodeUpdated event per node (store.bulk_update runs a
        // single SQL transaction with no per-row notify), now carrying the real
        // changed_properties so property-change automation fires.
        for (id, node, changed_properties) in pending_events {
            self.emit_event(DomainEvent::NodeUpdated {
                node_id: id,
                node_type: node.node_type.clone(),
                node,
                changed_properties,
            });
        }

        Ok(())
    }

    /// Bulk delete multiple nodes in a transaction
    ///
    /// Deletes multiple nodes atomically. If any deletion fails, the entire
    /// transaction is rolled back.
    ///
    /// # Arguments
    ///
    /// * `ids` - Vector of node IDs to delete
    ///
    /// # Errors
    ///
    /// Returns error if any deletion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// let ids = vec!["node-1".to_string(), "node-2".to_string()];
    /// service.bulk_delete(ids).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_delete(&self, ids: Vec<String>) -> Result<(), NodeServiceError> {
        if ids.is_empty() {
            return Ok(());
        }

        // Delete in ONE transaction so the documented all-or-nothing contract
        // actually holds. The old loop called store.delete_node per id (each its own
        // autocommit), so a failure on the Nth left the first N-1 committed while the
        // caller got Err and reasonably assumed nothing was deleted → orphaned state /
        // double-delete on retry. Coalesce the Deleted events: one per node.
        let _batch = self.begin_batch_emit();
        self.store
            .bulk_delete(&ids, self.client_id.clone())
            .await
            .map_err(|e| {
                NodeServiceError::bulk_operation_failed(format!(
                    "Failed to bulk delete nodes: {}",
                    e
                ))
            })?;
        // _batch drops here → one flush per deleted node

        Ok(())
    }
}
