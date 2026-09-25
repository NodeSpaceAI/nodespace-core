//! Relationship and mention operations for NodeService.

use super::*;

impl NodeService {
    /// Create a mention relationship between two existing nodes
    ///
    /// Adds an entry to the relationship table (relationship_type = 'mentions') to track that one node mentions another.
    /// This enables backlink/references functionality.
    ///
    /// # Arguments
    ///
    /// * `mentioning_node_id` - ID of the node that contains the mention
    /// * `mentioned_node_id` - ID of the node being mentioned
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Either node doesn't exist
    /// - Database insertion fails
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
    /// // Create mention: "daily-note" mentions "project-planning"
    /// service.create_mention("daily-note-id", "project-planning-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_mention(
        &self,
        mentioning_node_id: &str,
        mentioned_node_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Prevent direct self-references
        if mentioning_node_id == mentioned_node_id {
            return Err(NodeServiceError::ValidationFailed(
                crate::models::ValidationError::InvalidParent(
                    "Cannot create self-referencing mention".to_string(),
                ),
            ));
        }

        // Validate both nodes exist
        if !self.node_exists(mentioning_node_id).await? {
            return Err(NodeServiceError::node_not_found(mentioning_node_id));
        }
        if !self.node_exists(mentioned_node_id).await? {
            return Err(NodeServiceError::node_not_found(mentioned_node_id));
        }

        // Prevent root-level self-references (child mentioning its own root)
        // Get root ID via edge traversal for validation only
        let root_id = self.get_root_id(mentioning_node_id).await?;

        if root_id == mentioned_node_id {
            return Err(NodeServiceError::ValidationFailed(
                crate::models::ValidationError::InvalidParent(
                    "Cannot mention own root (root-level self-reference)".to_string(),
                ),
            ));
        }

        // Store returns relationship ID, service emits event
        // root_id no longer stored - computed dynamically via graph traversal
        let relationship_id = self
            .store
            .create_mention(mentioning_node_id, mentioned_node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Emit event if relationship was created (not already existing)
        if let Some(rel_id) = relationship_id {
            self.emit_event(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent::new(
                    rel_id,
                    mentioning_node_id,
                    mentioned_node_id,
                    "mentions",
                    serde_json::json!({}),
                ),
            });
        }

        Ok(())
    }

    /// Delete a mention relationship between two nodes
    ///
    /// Removes an entry from the relationship table (relationship_type = 'mentions').
    ///
    /// # Arguments
    ///
    /// * `mentioning_node_id` - ID of the node that contains the mention
    /// * `mentioned_node_id` - ID of the node being mentioned
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful (idempotent - succeeds even if mention doesn't exist)
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
    /// service.delete_mention("daily-note-id", "project-planning-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_mention(
        &self,
        mentioning_node_id: &str,
        mentioned_node_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Store returns relationship ID, service emits event
        let relationship_id = self
            .store
            .delete_mention(mentioning_node_id, mentioned_node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // Emit event if relationship was deleted (existed). Normalize
        // ids the same way `RelationshipEvent::new` does for the
        // created/updated variants — see `db::events::node_thing` for
        // the rationale (consumers parse on `:` and reject bare ids).
        if let Some(rel_id) = relationship_id {
            self.emit_event(DomainEvent::RelationshipDeleted {
                id: rel_id,
                from_id: crate::db::events::node_thing(mentioning_node_id),
                to_id: crate::db::events::node_thing(mentioned_node_id),
                relationship_type: "mentions".to_string(),
            });
        }

        Ok(())
    }

    /// Populate outgoing mentions from the relationship table (relationship_type = 'mentions')
    ///
    /// Queries the relationship table to populate outgoing mentions for a node.
    /// Note: mentioned_in (backlinks) is not populated here or anywhere on the
    /// node payload — it's fetched as its own resource via
    /// `get_mentioning_containers`, independently of any node read.
    pub(crate) async fn populate_mentions(&self, node: &mut Node) -> Result<(), NodeServiceError> {
        // Query outgoing mentions (nodes that THIS node references)
        let mentions = self
            .store
            .get_outgoing_mentions(&node.id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get outgoing mentions: {}", e))
            })?;
        node.mentions = mentions;

        Ok(())
    }

    /// Add a mention from one node to another
    ///
    /// Creates a mention relationship in the relationship table (relationship_type = 'mentions').
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the node that is mentioning
    /// * `target_id` - ID of the node being mentioned
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
    /// service.add_mention("node-123", "node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn add_mention(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Prevent direct self-references
        if source_id == target_id {
            return Err(NodeServiceError::ValidationFailed(
                crate::models::ValidationError::InvalidParent(
                    "Cannot create self-referencing mention".to_string(),
                ),
            ));
        }

        // Verify both nodes exist
        if !self.node_exists(source_id).await? {
            return Err(NodeServiceError::node_not_found(source_id));
        }
        if !self.node_exists(target_id).await? {
            return Err(NodeServiceError::node_not_found(target_id));
        }

        // Prevent root-level self-references (child mentioning its own parent)
        if let Ok(Some(parent)) = self.get_parent(source_id).await {
            if parent.id == target_id {
                return Err(NodeServiceError::ValidationFailed(
                    crate::models::ValidationError::InvalidParent(
                        "Cannot mention own parent (root-level self-reference)".to_string(),
                    ),
                ));
            }
        }

        // Store returns relationship ID, service emits event
        // root_id no longer stored - computed dynamically via graph traversal
        let relationship_id = self
            .store
            .create_mention(source_id, target_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to insert mention: {}", e))
            })?;

        // Emit event if relationship was created (not already existing)
        if let Some(rel_id) = relationship_id {
            self.emit_event(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent::new(
                    rel_id,
                    source_id,
                    target_id,
                    "mentions",
                    serde_json::json!({}),
                ),
            });
        }

        Ok(())
    }

    /// Remove a mention from one node to another
    ///
    /// Deletes a mention relationship from the relationship table (relationship_type = 'mentions').
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the node that is mentioning
    /// * `target_id` - ID of the node being mentioned
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
    /// service.remove_mention("node-123", "node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn remove_mention(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Store returns relationship ID, service emits event
        let relationship_id = self
            .store
            .delete_mention(source_id, target_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to delete mention: {}", e))
            })?;

        // Emit event if relationship was deleted (existed). Normalize
        // ids — same rationale as the other `RelationshipDeleted`
        // sites; see `db::events::node_thing`.
        if let Some(rel_id) = relationship_id {
            self.emit_event(DomainEvent::RelationshipDeleted {
                id: rel_id,
                from_id: crate::db::events::node_thing(source_id),
                to_id: crate::db::events::node_thing(target_id),
                relationship_type: "mentions".to_string(),
            });
        }

        Ok(())
    }

    /// Get all nodes that a specific node mentions (outgoing references)
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to get mentions for
    ///
    /// # Returns
    ///
    /// Vector of node IDs that this node mentions
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
    /// let mentions = service.get_mentions("node-123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentions(&self, node_id: &str) -> Result<Vec<String>, NodeServiceError> {
        self.store
            .get_outgoing_mentions(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Get all nodes that mention a specific node (incoming references/backlinks)
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node ID to get backlinks for
    ///
    /// # Returns
    ///
    /// Vector of node IDs that mention this node
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
    /// let backlinks = service.get_mentioned_by("node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentioned_by(&self, node_id: &str) -> Result<Vec<String>, NodeServiceError> {
        self.store
            .get_incoming_mentions(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Get containers (root or task nodes) that mention the target node (backlinks).
    ///
    /// This resolves incoming mentions to their container nodes and deduplicates.
    /// Returns `NodeReference` with {id, title, nodeType} for efficient UI display.
    ///
    /// # Container Resolution Logic
    /// - For task/ai-chat nodes: Uses the node itself (its own container)
    /// - For other nodes: Traverses up the hierarchy to find the root node
    ///
    /// # Performance
    ///
    /// Uses optimized batch queries with recursive ancestor traversal:
    /// - Single query to get all mentioning sources with their ancestor chains
    /// - Single batch query to fetch container nodes
    ///
    /// # Example
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // If nodes A and B (both children of Container X) mention target node,
    /// // returns [NodeReference { id: "container-x-id", title: "...", nodeType: "text" }]
    /// let containers = service.get_mentioning_containers("target-node-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentioning_containers(
        &self,
        node_id: &str,
    ) -> Result<Vec<crate::models::NodeReference>, NodeServiceError> {
        self.store
            .get_incoming_mention_containers(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    // ========================================================================
    // Relationship CRUD Operations (Phase 4)
    // ========================================================================

    /// Create a relationship between two nodes
    ///
    /// Creates an edge in the appropriate relationship table based on the schema definition.
    /// Validates that both nodes exist, enforces cardinality constraints, and supports
    /// edge field data.
    ///
    /// # TODO: UI components needed for relationship interaction
    /// The backend API is complete, but users need UI components to:
    /// - Select nodes to relate (search/dropdown)
    /// - View existing relationships
    /// - Remove relationships
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the source node
    /// * `relationship_name` - Name of the relationship (e.g., "assigned_to")
    /// * `target_id` - ID of the target node
    /// * `edge_data` - Optional JSON data for edge fields
    ///
    /// # Returns
    ///
    /// Ok(()) if successful
    ///
    /// # Errors
    ///
    /// - `NodeNotFound` - Source or target node doesn't exist
    /// - `SchemaNotFound` - Source node's schema doesn't exist
    /// - `RelationshipNotFound` - Relationship not defined in schema
    /// - `TargetTypeMismatch` - Target node type doesn't match schema definition
    /// - A `cardinality: One` source or `reverse_cardinality: One` target does NOT
    ///   error on a second edge — the prior edge is replaced (evicted, then the
    ///   new one inserted), atomically with the insert. This call can still fail
    ///   if the evicted edge's own source relationship is declared `required:
    ///   true` and this was its last edge — the eviction refuses to leave that
    ///   invariant violated, surfacing an error instead.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Create relationship with edge field data
    /// service.create_relationship(
    ///     "task-123",
    ///     "assigned_to",
    ///     "person-456",
    ///     json!({"role": "owner", "assigned_at": "2025-01-15"})
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    /// Find `relationship_name` on `schema_id` or any of its `extends`
    /// ancestors, as a declaration a caller may create an edge under.
    ///
    /// The lookup walks the chain because an extending schema inherits its
    /// ancestors' relationships (ADR-078): `task.blocks` is declared on `task`,
    /// and an `issue` IS a task, so `blocks` must be creatable from an issue.
    /// Checking only the node's own type made inherited relationships
    /// unusable — `create_relationship` rejected them outright, which meant a
    /// subtype could never participate in an edge its parent declares.
    ///
    /// Delegates to [`Self::resolve_relationships`] rather than hand-walking
    /// the chain itself — same chain source (`resolve_type_chain`), same
    /// nearest-first shadowing (`flatten_chain_by_name`), so this can't drift
    /// from the resolver every other relationship-reading call site already
    /// uses. One behavioral difference from the old hand-walk, and a
    /// deliberate one: `resolve_relationships` excludes the `extends`/
    /// `extended_by` type-system bookkeeping row (see its own doc comment),
    /// which the old per-scope `SchemaNode::get_relationship` lookup did not
    /// — that lookup would happily return a schema's own `extends`
    /// declaration if a caller named it as `relationship_name`, letting
    /// `create_relationship` create a real data-level `extends` edge between
    /// two ordinary node instances. That was never a reachable, intentional
    /// path (nothing creates such an edge), and is closed now rather than
    /// preserved.
    async fn resolve_declared_relationship(
        &self,
        schema_id: &str,
        relationship_name: &str,
    ) -> Result<crate::models::schema::SchemaRelationship, NodeServiceError> {
        let (relationships, _owners) = self.resolve_relationships(schema_id).await?;

        relationships
            .into_iter()
            .find(|rel| rel.name == relationship_name)
            .ok_or_else(|| {
                // Nearest scope first is already `resolve_relationships`'s own
                // resolution order; `schema_id` is reported unconditionally
                // here regardless of how deep a match would have been in the
                // chain — that is the type the caller named.
                NodeServiceError::invalid_update(format!(
                    "Relationship '{}' not defined in schema '{}'. Built-in relationships (member_of, has_child, mentions, has_role) are universal.",
                    relationship_name, schema_id
                ))
            })
    }

    /// `_in_tx` equivalent of [`Self::get_node`]'s virtual-date fallback.
    ///
    /// A date node (`YYYY-MM-DD`) with no row yet is still a legitimate
    /// endpoint — it auto-persists the first time something is actually
    /// filed under it, and until then `get_node` synthesizes it on read
    /// rather than reporting `NodeNotFound`. The raw
    /// `SqliteStore::get_node_in_tx` has no such fallback, so a declared
    /// relationship's tx-scoped source/target lookup needs this wrapper —
    /// without it, any custom-relationship create touching an
    /// as-yet-unvisited date page (CLI, agent tool, or the daemon RPC
    /// surface — everything routed through `create_relationship_in_tx`)
    /// would fail with a hard `NodeNotFound` that the old non-tx path never
    /// produced.
    async fn get_node_in_tx_or_virtual_date(
        tx: &NodeServiceTx<'_>,
        id: &str,
    ) -> Result<Option<crate::models::Node>, NodeServiceError> {
        if let Some(node) = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            return Ok(Some(node));
        }
        if is_date_node_id(id) {
            return Ok(Some(crate::models::Node {
                id: id.to_string(),
                node_type: "date".to_string(),
                content: id.to_string(),
                version: 1,
                created_at: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
                properties: serde_json::json!({}),
                mentions: vec![],
                mentioned_in: vec![],
                title: None,
                lifecycle_status: "active".to_string(),
            }));
        }
        Ok(None)
    }

    /// Whether `node_type` satisfies a declaration expecting `expected_type` —
    /// true when they match, or when `expected_type` is an ancestor of
    /// `node_type` (ADR-078).
    pub(super) async fn type_satisfies(
        &self,
        node_type: &str,
        expected_type: &str,
    ) -> Result<bool, NodeServiceError> {
        if node_type == expected_type {
            return Ok(true);
        }
        Ok(self
            .resolve_type_chain(node_type)
            .await?
            .iter()
            .any(|scope| scope == expected_type))
    }

    pub async fn create_relationship(
        &self,
        source_id: &str,
        relationship_name: &str,
        target_id: &str,
        edge_data: serde_json::Value,
    ) -> Result<(), NodeServiceError> {
        // Unified relationship creation - ALL relationships use the `relationship` table
        // The relationship_type field distinguishes between different relationship types

        // Built-in type validation
        let is_builtin = crate::models::schema::is_builtin_relationship(relationship_name);

        if !is_builtin {
            // A declared (custom) relationship's full validation — including
            // the cardinality-one replace on either end — runs against
            // `create_relationship_in_tx`, wrapped in one transaction here so
            // a public, non-tx caller (CLI, agent tools, the daemon RPC
            // surface) gets the same atomicity an invariant Play's
            // `add_relationship` action already gets by calling that method
            // directly. Without this, the replace's evict-then-insert was two
            // separate auto-committed statements: a crash between them left a
            // cardinality-one end with ZERO live edges — worse than the
            // all-or-nothing reject this replaced, and the opposite of the
            // crash-safety this change is meant to provide.
            let service = self.clone();
            let service_for_tx = service.clone();
            let source_id = source_id.to_string();
            let relationship_name = relationship_name.to_string();
            let target_id = target_id.to_string();
            return service
                .with_transaction(move |tx| {
                    Box::pin(async move {
                        service_for_tx
                            .create_relationship_in_tx(
                                tx,
                                &source_id,
                                &relationship_name,
                                &target_id,
                                edge_data,
                            )
                            .await
                    })
                })
                .await;
        }

        // Built-in type-specific validation. Only a builtin reaches here — a
        // declared (custom) relationship returned early above.
        if relationship_name == "member_of" {
            let target = self
                .get_node(target_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(target_id))?;
            if target.node_type != "collection" {
                return Err(NodeServiceError::invalid_update(format!(
                    "member_of target must be a collection node, got '{}'",
                    target.node_type
                )));
            }
            // Collection hierarchy (collection member_of collection) is a
            // DAG, but nothing enforced it — `a member_of b` + `b member_of a`
            // created a cycle that makes the recursive members walk loop. Reject
            // a hierarchy edge that would close a cycle. Only relevant when the
            // source is itself a collection; a content node has no member_of
            // descendants, so the check is a cheap no-op for ordinary membership.
            let source = self
                .get_node(source_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(source_id))?;
            if source.node_type == "collection" {
                self.store
                    .validate_no_member_of_cycle(source_id, target_id)
                    .await
                    .map_err(|e| NodeServiceError::collection_cycle(e.to_string()))?;
            }
        }

        if relationship_name == "has_child" {
            let target = self
                .get_node(target_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(target_id))?;
            if target.node_type == "collection" {
                return Err(NodeServiceError::hierarchy_violation(
                    crate::db::collection_not_root(Some(target_id)),
                ));
            }
        }

        // The outline is single-parent, and every read path assumes it:
        // `get_parent`/`get_parent_id` resolve with `LIMIT 1`, so a second
        // parent does not produce an error — it silently hides one of them
        // and makes which parent a node has depend on row order.
        //
        // A built-in skips the declared-cardinality check below (it has no
        // `SchemaRelationship` to carry `cardinality: One`), so until now
        // nothing rejected the second edge: `relationship create --type
        // has_child` from the CLI, or the agent's `create_relationship`
        // tool, would just add it. Enforce at the service entry point,
        // which every external caller reaches, rather than in each one.
        //
        // Mirrored in `create_relationship_in_tx`. The reparenting paths
        // (`move_node`, `bulk_create_has_child`, `create_parent_edge_in_tx`)
        // do NOT pass through here and need no guard: each is structurally
        // single-parent already — delete-then-insert in one transaction, a
        // skip of already-parented children, or a child created in the same
        // transaction that cannot yet hold a parent.
        if relationship_name == "has_child" {
            if let Some(existing) = self.store.get_parent_id(target_id).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to check existing parent: {e}"))
            })? {
                if existing != source_id {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Node '{target_id}' already has parent '{existing}'; the outline is \
                         single-parent. Move the node instead of adding a second `has_child` \
                         edge."
                    )));
                }
            }
        }

        // For member_of relationships with auto-order, use the atomic
        // add_to_collection method to prevent race conditions. This ensures the
        // order calculation and relationship creation happen in a single query.
        if relationship_name == "member_of" {
            let has_explicit_order = edge_data
                .as_object()
                .map(|o| o.contains_key("order"))
                .unwrap_or(false);

            if !has_explicit_order {
                // Use atomic add_to_collection for auto-ordered member_of
                let created = self
                    .store
                    .add_to_collection(source_id, target_id, &edge_data)
                    .await
                    .map_err(|e| {
                        NodeServiceError::query_failed(format!(
                            "Failed to add to collection: {}",
                            e
                        ))
                    })?;

                // Emit event if relationship was created (not idempotent hit)
                if let Some((id, merged_props)) = created {
                    self.emit_event(DomainEvent::RelationshipCreated {
                        relationship: crate::db::events::RelationshipEvent::new(
                            id,
                            source_id,
                            target_id,
                            "member_of",
                            merged_props,
                        ),
                    });
                }
                return Ok(());
            }
        }

        // Check for existing relationship (idempotency)
        let already_exists = self
            .store
            .relationship_exists(source_id, target_id, relationship_name)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to check existing relationship: {}",
                    e
                ))
            })?;
        if already_exists {
            // Relationship already exists, idempotent success
            return Ok(());
        }

        // Auto-ordered `has_child` with no caller-supplied order: the next
        // fractional key MUST be read and written as one atomic unit, or a
        // concurrent reorder can interleave between the read and the write and
        // collide the order key. The store method does the read → compute →
        // write under the store's write guard. (member_of auto-order is likewise handled
        // atomically above via add_to_collection.) Explicit-order has_child and
        // the unordered builtins fall through to the generic path below.
        if relationship_name == "has_child" && edge_data.get("order").is_none() {
            let (order, rel_id) = self
                .store
                .append_child_edge(source_id, target_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to append child edge: {}", e))
                })?;
            self.refresh_for_rootness(target_id, false, None).await;

            self.emit_event(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent::new(
                    rel_id,
                    source_id,
                    target_id,
                    "has_child",
                    serde_json::json!({ "order": order }),
                ),
            });

            return Ok(());
        }

        // Remaining relationships carry the caller's edge_data as-is: the two
        // auto-ordered builtins (member_of, has_child) returned early above
        // when auto-ordered, and mentions / has_role are unordered. Only a
        // builtin ever reaches here at all — a declared (custom) relationship
        // returned early above, through the transactional
        // `create_relationship_in_tx` path. Builtins normalize a non-object
        // payload to an empty object and carry no declared reverse name (the
        // store derives a builtin's reverse from its forward name).
        let final_edge_data = serde_json::json!(edge_data.as_object().cloned().unwrap_or_default());

        let rel_id = self
            .store
            .create_generic_relationship(
                source_id,
                target_id,
                relationship_name,
                None,
                &final_edge_data,
            )
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to create relationship: {}", e))
            })?;
        if relationship_name == "has_child" {
            self.refresh_for_rootness(target_id, false, None).await;
        }

        self.emit_event(DomainEvent::RelationshipCreated {
            relationship: crate::db::events::RelationshipEvent::new(
                rel_id,
                source_id,
                target_id,
                relationship_name,
                final_edge_data,
            ),
        });

        Ok(())
    }

    /// Tx-scoped twin of [`Self::create_relationship`], for invariant-rule
    /// `add_relationship` actions (ADR-060 §1). Covers the same validation
    /// (built-in target-type checks, schema-declared custom relationships,
    /// edge-field validation, cardinality-one, the ADR-059 §2 `member_of`
    /// root-only gate) against tx-consistent reads via
    /// `SqliteStore::get_node_in_tx`, since `source_id` is very often the
    /// node this same transaction just inserted.
    ///
    /// Deliberately narrower than `create_relationship` in one way: it does
    /// **not** implement the atomic auto-order paths `add_to_collection` /
    /// `append_child_edge` provide for `member_of` / `has_child` when
    /// `edge_data` omits `order` (their read-current-max-then-write has no
    /// transaction-scoped twin here — see
    /// `SqliteStore::create_generic_relationship_in_tx`'s doc). Save-time
    /// validation (`playbook::validation`) rejects an invariant
    /// `add_relationship` action for either of those two types unless
    /// `edge_data` supplies an explicit `order`, so this method never has to
    /// reject at execution time for that reason — an omission here would be
    /// a validation bug, not a normal runtime outcome.
    pub(crate) async fn create_relationship_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        source_id: &str,
        relationship_name: &str,
        target_id: &str,
        edge_data: serde_json::Value,
    ) -> Result<(), NodeServiceError> {
        let is_builtin = crate::models::schema::is_builtin_relationship(relationship_name);

        // See `create_relationship` — a declared relationship's instance edge
        // must carry its declaration's reverse name, or the column lands NULL.
        let mut declared_reverse_name: Option<String> = None;

        // Cardinality-one replace's evictions, gathered below (custom
        // relationships only — always `None` for a builtin) but not yet
        // performed — see the comment at the gathering site for why eviction
        // must wait until after the new edge is inserted.
        let mut evict_after_insert: Option<(Vec<String>, Vec<String>)> = None;

        if is_builtin {
            if relationship_name == "member_of" {
                let target = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), target_id)
                    .await
                    .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
                    .ok_or_else(|| NodeServiceError::node_not_found(target_id))?;
                if target.node_type != "collection" {
                    return Err(NodeServiceError::invalid_update(format!(
                        "member_of target must be a collection node, got '{}'",
                        target.node_type
                    )));
                }
                let source = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), source_id)
                    .await
                    .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
                    .ok_or_else(|| NodeServiceError::node_not_found(source_id))?;
                if source.node_type == "collection" {
                    crate::db::SqliteStore::validate_no_member_of_cycle_in_tx(
                        tx.store_tx(),
                        source_id,
                        target_id,
                    )
                    .await
                    .map_err(|e| NodeServiceError::collection_cycle(e.to_string()))?;
                }
            }

            // Single-parent, same as the non-tx twin. An invariant Play's
            // `add_relationship` action reaches this path, so leaving it out
            // would make the guard's own rationale — enforce where every
            // surface converges — false for the one surface that runs in a
            // transaction.
            if relationship_name == "has_child" {
                if let Some(existing) =
                    crate::db::SqliteStore::get_parent_id_in_tx(tx.store_tx(), target_id)
                        .await
                        .map_err(|e| {
                            NodeServiceError::query_failed(format!(
                                "Failed to check existing parent: {e}"
                            ))
                        })?
                {
                    if existing != source_id {
                        return Err(NodeServiceError::invalid_update(format!(
                            "Node '{target_id}' already has parent '{existing}'; the outline is \
                             single-parent. Move the node instead of adding a second `has_child` \
                             edge."
                        )));
                    }
                }
            }
        } else {
            let source = Self::get_node_in_tx_or_virtual_date(tx, source_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(source_id))?;

            if source.node_type == "schema" {
                return Err(NodeServiceError::invalid_update(format!(
                    "'{}' is a schema node; typed relationships between schemas are declarations \
                     — declare them via update_schema, not create_relationship",
                    source_id
                )));
            }

            let schema_id = &source.node_type;
            let relationship = self
                .resolve_declared_relationship(schema_id, relationship_name)
                .await?;

            declared_reverse_name = Some(relationship.reverse_name.clone());

            let target = Self::get_node_in_tx_or_virtual_date(tx, target_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(target_id))?;

            if target.node_type == "schema" {
                return Err(NodeServiceError::invalid_update(format!(
                    "'{}' is a schema node; typed relationships between schemas are declarations \
                     — declare them via update_schema, not create_relationship",
                    target_id
                )));
            }

            if let Some(expected_type) = &relationship.target_type {
                // A subtype satisfies its ancestor's declared target type
                // (ADR-078): `task.blocks` targets `task`, and an `issue` IS a
                // task, so an issue is a legal target. Comparing the concrete
                // type alone made an inherited relationship undeclarable from
                // one subtype to another.
                if !self
                    .type_satisfies(&target.node_type, expected_type)
                    .await?
                {
                    return Err(NodeServiceError::invalid_update(format!(
                        "Target node type '{}' doesn't match expected type '{}' for relationship '{}'",
                        target.node_type, expected_type, relationship_name
                    )));
                }
            }

            if let Some(edge_fields) = relationship.edge_fields.as_deref() {
                validate_edge_data_against_fields(&edge_data, edge_fields, relationship_name)?;
            }

            // Replace semantics for cardinality 'one' — see the non-tx twin
            // in `create_relationship` for the full rationale (agreement
            // between forward/reverse enforcement, and why an add-then-remove
            // reassignment needs the add half to evict the prior edge rather
            // than fail). This only GATHERS which edges to evict — the
            // actual eviction happens AFTER the new edge is inserted below,
            // not here. Evicting first would mean `remove_relationship_in_tx`'s
            // required-relationship last-edge check counts `source_id`'s
            // edges for this relationship type while it still has exactly
            // the one about to be replaced (cardinality: One guarantees at
            // most one), so it would ALWAYS conclude "this is the last
            // edge" and reject — even though a replacement is about to land
            // in the very same transaction and the invariant would never
            // actually be violated. Reject-first was tried and confirmed
            // broken by review: a `cardinality: One` relationship that is
            // ALSO `required: true` could never be reassigned at all. Insert
            // first, then evict: at that point `source_id` briefly holds
            // both edges, so the required-check correctly sees more than
            // one and evicts the old one cleanly. The store's unique index
            // is on `(in_node, out_node, relationship_type)`, not
            // `(out_node, relationship_type)`, so briefly holding both is
            // not a constraint violation — and it is invisible to any
            // reader outside this transaction regardless.
            let mut forward_targets_to_evict: Vec<String> = Vec::new();
            if relationship.cardinality == crate::models::schema::RelationshipCardinality::One {
                let existing_edges =
                    crate::db::SqliteStore::get_relationship_edges_from_source_in_tx(
                        tx.store_tx(),
                        source_id,
                        relationship_name,
                    )
                    .await
                    .map_err(|e| {
                        NodeServiceError::query_failed(format!(
                            "Failed to check cardinality: {}",
                            e
                        ))
                    })?;
                for (_, existing_target_id) in existing_edges {
                    if existing_target_id == target_id {
                        continue;
                    }
                    forward_targets_to_evict.push(existing_target_id);
                }
            }

            // Reverse cardinality — see the non-tx twin in `create_relationship`
            // for why this is needed alongside the forward check above, and
            // for why matches are scoped to the declaring schema (two schemas
            // may share a forward name toward the same target type as
            // logically distinct relationships). Also gather-only, for
            // symmetry with the forward case — though the required-relationship
            // trap above is specific to the forward direction: an evicted
            // reverse-side edge belongs to a DIFFERENT node than the one
            // gaining the new edge, so inserting first cannot help it the
            // same way (that node's own edge count is genuinely unaffected
            // by this insert), and a real "last required edge" there
            // correctly stays rejected either way.
            let mut reverse_sources_to_evict: Vec<String> = Vec::new();
            if relationship.reverse_cardinality
                == crate::models::schema::RelationshipCardinality::One
            {
                let (_, owners) = self.resolve_relationships(schema_id).await?;
                let declaring_type = owners
                    .get(relationship_name)
                    .cloned()
                    .unwrap_or_else(|| schema_id.clone());

                let existing_edges =
                    crate::db::SqliteStore::get_relationship_edges_into_target_in_tx(
                        tx.store_tx(),
                        target_id,
                        relationship_name,
                    )
                    .await
                    .map_err(|e| {
                        NodeServiceError::query_failed(format!(
                            "Failed to check reverse cardinality: {}",
                            e
                        ))
                    })?;

                for (_, existing_source_id, existing_source_type) in existing_edges {
                    if existing_source_id == source_id {
                        continue;
                    }
                    if !self
                        .type_satisfies(&existing_source_type, &declaring_type)
                        .await?
                    {
                        continue;
                    }
                    reverse_sources_to_evict.push(existing_source_id);
                }
            }

            evict_after_insert = Some((forward_targets_to_evict, reverse_sources_to_evict));
        }

        // Idempotency check (mirrors `create_relationship`'s generic path;
        // the auto-order `member_of`/`has_child` short-circuits are
        // deliberately not reproduced here — see this method's doc).
        let already_exists = crate::db::SqliteStore::relationship_exists_in_tx(
            tx.store_tx(),
            source_id,
            target_id,
            relationship_name,
        )
        .await
        .map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to check existing relationship: {}", e))
        })?;
        if already_exists {
            return Ok(());
        }

        let final_edge_data = if is_builtin {
            serde_json::json!(edge_data.as_object().cloned().unwrap_or_default())
        } else {
            edge_data.clone()
        };

        let rel_id = crate::db::SqliteStore::create_generic_relationship_in_tx(
            tx.store_tx(),
            source_id,
            target_id,
            relationship_name,
            declared_reverse_name.as_deref(),
            &final_edge_data,
        )
        .await
        .map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to create relationship: {}", e))
        })?;
        if relationship_name == "has_child" {
            self.refresh_for_rootness_in_tx(tx, target_id, false, None)
                .await?;
        }

        self.emit_event(DomainEvent::RelationshipCreated {
            relationship: crate::db::events::RelationshipEvent::new(
                rel_id,
                source_id,
                target_id,
                relationship_name,
                final_edge_data,
            ),
        });

        // Now that the new edge is durably inserted (above), evict whatever
        // cardinality-one replace gathered earlier — see the gathering
        // site's comment for why this must happen AFTER the insert rather
        // than before it.
        if let Some((forward_targets_to_evict, reverse_sources_to_evict)) = evict_after_insert {
            for existing_target_id in forward_targets_to_evict {
                self.remove_relationship_in_tx(
                    tx,
                    source_id,
                    relationship_name,
                    &existing_target_id,
                )
                .await?;
            }
            for existing_source_id in reverse_sources_to_evict {
                self.remove_relationship_in_tx(
                    tx,
                    &existing_source_id,
                    relationship_name,
                    target_id,
                )
                .await?;
            }
        }

        Ok(())
    }

    /// Closes the gap `SqliteStore::merge_nodes_in_tx` cannot close on its
    /// own: that store-level step re-points every edge touching the merge's
    /// loser onto the survivor with no notion of a declared relationship's
    /// `cardinality`/`reverse_cardinality` (schema resolution lives here, in
    /// `NodeService`, not the store). When survivor and loser each held
    /// their own compliant edge of the same `cardinality: One` (or
    /// `reverse_cardinality: One`) relationship toward *different* targets,
    /// the repoint leaves the survivor with two live edges where the schema
    /// allows at most one.
    ///
    /// Called by [`Self::merge_nodes`] immediately after
    /// `SqliteStore::merge_nodes_in_tx` returns, in the same transaction,
    /// with that call's `repointed_edges`. For each repointed edge whose
    /// declared relationship is cardinality-one on the end the survivor now
    /// occupies, evicts that edge — the one just re-pointed from the loser —
    /// via [`Self::remove_relationship_in_tx`], the same eviction primitive
    /// `create_relationship_in_tx`'s replace semantics use, keeping the
    /// survivor's own pre-existing edge intact. Mirrors that method's gather
    /// logic but does not need its "insert first, evict after" ordering
    /// trick — the repoint has already landed, so a `required` relationship's
    /// last-edge check sees both edges and cannot mistake this eviction for
    /// removing the last one.
    ///
    /// Built-in relationship types (`has_child`, `mentions`, `member_of`,
    /// `has_role`) carry no declared cardinality and are skipped — `has_child`
    /// single-parent is a separate, structural guarantee untouched by this,
    /// and remains its own pre-existing gap (independent of this method):
    /// a repoint can still hand a survivor two `has_child` parents when
    /// each already had a different one, since that collision isn't caught
    /// by the store's raw unique-index check either (different `in_node`
    /// values, same `out_node`).
    ///
    /// **The reverse branch can abort the whole merge, unlike the forward
    /// one.** The forward branch's eviction target is always the survivor's
    /// OWN edge count, which the repoint has already grown to two — so a
    /// `required` last-edge check there can never fire (see above). The
    /// reverse branch's eviction target is a DIFFERENT node (the repointed
    /// edge's `source_id`, e.g. the loser's own former counterpart), and
    /// that node's edge count is genuinely unaffected by the repoint: if
    /// this is its only edge of a relationship that is ALSO `required: true`
    /// on its schema, [`Self::remove_relationship_in_tx`]'s last-edge guard
    /// rejects the eviction, and that `Err` propagates out of this method,
    /// out of `merge_nodes`'s `with_transaction` closure, and rolls back the
    /// ENTIRE merge — not just this one edge. This is intentional, not a
    /// bug: the conflict is real and irreconcilable (keeping the edge
    /// violates the survivor's `reverse_cardinality: One`; evicting it
    /// violates the source's `required: true`), so failing the merge closed
    /// is the only safe outcome, and it needs no special handling here — the
    /// existing transaction rollback (verified atomic; see
    /// `SqliteStore::with_transaction`) already leaves the database exactly
    /// as it was before the merge was attempted. Exactly mirrors
    /// `create_relationship_in_tx`'s own reverse-cardinality eviction, which
    /// has the identical trap by the same design (see its doc comment).
    ///
    /// Returns the number of repointed edges evicted here, so the caller can
    /// fold them into the merge's overall `edges_dropped` count (and subtract
    /// them from `edges_repointed`, since they did not end up surviving).
    pub(crate) async fn enforce_cardinality_after_merge_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        survivor_id: &str,
        repointed_edges: &[(String, String, String)],
    ) -> Result<u32, NodeServiceError> {
        let mut evicted = 0u32;
        // Cached lazily — only needed once a forward-cardinality edge is seen.
        let mut survivor_node_type: Option<String> = None;

        for (relationship_type, source_id, target_id) in repointed_edges {
            if crate::models::schema::is_builtin_relationship(relationship_type) {
                continue;
            }

            // Forward end: this repointed edge now originates from the
            // survivor. A declared `cardinality: One` means the survivor may
            // hold at most one edge of this type — if the repoint gave it a
            // second (its own pre-existing edge, plus this one from the
            // loser), evict this one.
            if source_id == survivor_id {
                if survivor_node_type.is_none() {
                    let survivor =
                        crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), survivor_id)
                            .await
                            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
                            .ok_or_else(|| NodeServiceError::node_not_found(survivor_id))?;
                    survivor_node_type = Some(survivor.node_type);
                }
                let schema_id = survivor_node_type.as_deref().unwrap_or_default();

                let relationship = match self
                    .resolve_declared_relationship(schema_id, relationship_type)
                    .await
                {
                    Ok(rel) => rel,
                    // Not a declared relationship on this schema (e.g. the
                    // schema changed since the edge was created) — nothing to
                    // enforce here.
                    Err(NodeServiceError::InvalidUpdate(_)) => continue,
                    Err(e) => return Err(e),
                };

                if relationship.cardinality == crate::models::schema::RelationshipCardinality::One {
                    let existing_edges =
                        crate::db::SqliteStore::get_relationship_edges_from_source_in_tx(
                            tx.store_tx(),
                            survivor_id,
                            relationship_type,
                        )
                        .await
                        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

                    let collides = existing_edges
                        .iter()
                        .any(|(_, existing_target)| existing_target != target_id);
                    if collides {
                        self.remove_relationship_in_tx(tx, source_id, relationship_type, target_id)
                            .await?;
                        evicted += 1;
                        continue;
                    }
                }
            }

            // Reverse end: this repointed edge now targets the survivor. A
            // declared `reverse_cardinality: One` means the survivor may be
            // the target of at most one edge of this type (from the
            // declaring schema's sources) — if the repoint gave it a second,
            // evict this one.
            if target_id == survivor_id {
                // Same resolver `create_relationship_in_tx` uses for a
                // relationship's source (see its own call site) — a plain
                // `get_node_in_tx` would silently skip enforcement for a
                // not-yet-persisted virtual node instead of resolving it.
                let Some(source) = Self::get_node_in_tx_or_virtual_date(tx, source_id).await?
                else {
                    continue;
                };

                let relationship = match self
                    .resolve_declared_relationship(&source.node_type, relationship_type)
                    .await
                {
                    Ok(rel) => rel,
                    Err(NodeServiceError::InvalidUpdate(_)) => continue,
                    Err(e) => return Err(e),
                };

                if relationship.reverse_cardinality
                    == crate::models::schema::RelationshipCardinality::One
                {
                    let (_, owners) = self.resolve_relationships(&source.node_type).await?;
                    let declaring_type = owners
                        .get(relationship_type)
                        .cloned()
                        .unwrap_or_else(|| source.node_type.clone());

                    let existing_edges =
                        crate::db::SqliteStore::get_relationship_edges_into_target_in_tx(
                            tx.store_tx(),
                            survivor_id,
                            relationship_type,
                        )
                        .await
                        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

                    let mut collides = false;
                    for (_, existing_source_id, existing_source_type) in &existing_edges {
                        if existing_source_id == source_id {
                            continue;
                        }
                        if !self
                            .type_satisfies(existing_source_type, &declaring_type)
                            .await?
                        {
                            continue;
                        }
                        collides = true;
                        break;
                    }

                    if collides {
                        self.remove_relationship_in_tx(tx, source_id, relationship_type, target_id)
                            .await?;
                        evicted += 1;
                    }
                }
            }
        }

        Ok(evicted)
    }

    /// Tx-scoped twin of [`Self::delete_relationship`], for invariant-rule
    /// `remove_relationship` actions (ADR-060 §1). Reproduces the
    /// required-relationship last-edge protection via tx-consistent reads.
    pub(crate) async fn remove_relationship_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        source_id: &str,
        relationship_name: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        let is_builtin = crate::models::schema::is_builtin_relationship(relationship_name);
        if !is_builtin {
            if let Some(source) = crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), source_id)
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                if source.node_type == "schema" {
                    return Err(NodeServiceError::invalid_update(format!(
                        "'{}' is a schema node; '{}' is a relationship declaration — \
                         remove it via update_schema, not delete_relationship",
                        source_id, relationship_name
                    )));
                }
                // Chain-aware (ADR-078) — see the non-tx twin in
                // `delete_relationship` for the full rationale.
                let (relationships, _) = self.resolve_relationships(&source.node_type).await?;
                let is_required = relationships
                    .iter()
                    .find(|r| r.name == relationship_name)
                    .map(|r| {
                        r.required == Some(true)
                            && r.direction == crate::models::schema::RelationshipDirection::Out
                    })
                    .unwrap_or(false);
                if is_required {
                    let edge_exists = crate::db::SqliteStore::relationship_exists_in_tx(
                        tx.store_tx(),
                        source_id,
                        target_id,
                        relationship_name,
                    )
                    .await
                    .map_err(|e| {
                        NodeServiceError::query_failed(format!(
                            "Failed to check relationship existence: {}",
                            e
                        ))
                    })?;
                    let total = crate::db::SqliteStore::check_relationship_exists_in_tx(
                        tx.store_tx(),
                        source_id,
                        relationship_name,
                    )
                    .await
                    .map_err(|e| {
                        NodeServiceError::query_failed(format!(
                            "Failed to count relationship edges: {}",
                            e
                        ))
                    })?;
                    if edge_exists && total <= 1 {
                        return Err(NodeServiceError::invalid_update(format!(
                            "Relationship '{}' is required and this is its last edge; add another target before removing this one",
                            relationship_name
                        )));
                    }
                }
            }
        }

        let rel_id = crate::db::SqliteStore::get_relationship_id_in_tx(
            tx.store_tx(),
            source_id,
            target_id,
            relationship_name,
        )
        .await
        .map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to get relationship ID: {}", e))
        })?;

        crate::db::SqliteStore::delete_generic_relationship_in_tx(
            tx.store_tx(),
            source_id,
            target_id,
            relationship_name,
        )
        .await
        .map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to delete relationship: {}", e))
        })?;
        if relationship_name == "has_child" && rel_id.is_some() {
            self.refresh_for_rootness_in_tx(tx, target_id, true, Some(source_id))
                .await?;
        }

        if let Some(id) = rel_id {
            self.emit_event(DomainEvent::RelationshipDeleted {
                id,
                from_id: crate::db::events::node_thing(source_id),
                to_id: crate::db::events::node_thing(target_id),
                relationship_type: relationship_name.to_string(),
            });
        }

        Ok(())
    }

    /// Bulk-create `member_of` edges AND emit a `RelationshipCreated` event for
    /// each newly created edge, so the cloud-sync push consumer replicates them.
    ///
    /// The raw `store.bulk_add_to_collections` inserts the rows directly and
    /// emits nothing. That is fine for a purely local write, but the edges then
    /// never reach cloud: they join nodes that are *already* synced, so the
    /// node-oriented "unsynced push sweep" skips them, and every other device
    /// (and any first-time puller) sees those collections empty. Routing the
    /// batch importer's collection assignment through here puts each membership
    /// on the exact same event path as [`Self::create_relationship`], so bulk
    /// import and single-file import replicate identically. Returns the number of
    /// edges actually created (idempotent hits are skipped and not re-emitted).
    pub async fn bulk_add_to_collections_notify(
        &self,
        memberships: &[(String, String)],
    ) -> Result<usize, NodeServiceError> {
        let created = self
            .store
            .bulk_add_to_collections(memberships)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        // The domain-event broadcast channel is bounded (128 slots). A large
        // import can create far more `member_of` edges than that; emitting them
        // all in a tight loop would overflow the channel, making the sync push
        // consumer lag and DROP edges — which then never reach cloud (defeating
        // the whole point). Yield every chunk so the consumer drains between
        // bursts. The chunk stays well under the channel capacity.
        const EMIT_CHUNK: usize = 50;
        for (i, (rel_id, node_id, collection_id, order)) in created.iter().enumerate() {
            self.emit_event(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent::new(
                    rel_id.clone(),
                    node_id,
                    collection_id,
                    "member_of",
                    serde_json::json!({ "order": order }),
                ),
            });
            if (i + 1) % EMIT_CHUNK == 0 {
                tokio::task::yield_now().await;
            }
        }

        Ok(created.len())
    }

    /// Delete a relationship between two nodes
    ///
    /// Removes the edge between the source and target nodes for the specified relationship.
    ///
    /// # TODO: UI components needed for relationship interaction
    ///
    /// # Arguments
    ///
    /// * `source_id` - ID of the source node
    /// * `relationship_name` - Name of the relationship
    /// * `target_id` - ID of the target node
    ///
    /// # Returns
    ///
    /// Ok(()) if successful (idempotent - succeeds even if edge doesn't exist)
    ///
    /// # Errors
    ///
    /// - `NodeNotFound` - Source node doesn't exist
    /// - `SchemaNotFound` - Source node's schema doesn't exist
    /// - `RelationshipNotFound` - Relationship not defined in schema
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
    /// service.delete_relationship("task-123", "assigned_to", "person-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_relationship(
        &self,
        source_id: &str,
        relationship_name: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Unified relationship deletion - ALL relationships use the `relationship` table
        // The relationship_type field distinguishes between different relationship types

        // Required-relationship last-edge protection: a schema
        // relationship declared `required: true` must always retain at least one
        // edge — deleting its final edge would leave the node violating its own
        // schema. Reject only when the targeted edge actually exists AND it is the
        // last remaining edge of that relationship on this source. Removing one of
        // several is fine; deleting a nonexistent edge stays a harmless no-op.
        // Built-in structural relationships are not schema-declared and are exempt.
        let is_builtin = crate::models::schema::is_builtin_relationship(relationship_name);
        if !is_builtin {
            if let Some(source) = self.get_node(source_id).await? {
                // A non-builtin edge whose source is a schema node is a
                // relationship DECLARATION — deleting it here would bypass the
                // live-instance-edge protection `set_schema_relationships`
                // enforces, orphaning every edge written under it.
                if source.node_type == "schema" {
                    return Err(NodeServiceError::invalid_update(format!(
                        "'{}' is a schema node; '{}' is a relationship declaration — \
                         remove it via update_schema, not delete_relationship",
                        source_id, relationship_name
                    )));
                }
                // Chain-aware (ADR-078): an inherited `required: true`
                // relationship (declared on an ancestor, not redeclared on
                // this subtype) must still get last-edge protection — the
                // same merged/effective set `resolve_declared_relationship`
                // resolves against on the create side.
                let (relationships, _) = self.resolve_relationships(&source.node_type).await?;
                // `required` is an outbound-declaration property; only enforce
                // it for a forward (`direction: out`) relationship so an
                // inbound-declared one never counts the wrong edge set.
                let is_required = relationships
                    .iter()
                    .find(|r| r.name == relationship_name)
                    .map(|r| {
                        r.required == Some(true)
                            && r.direction == crate::models::schema::RelationshipDirection::Out
                    })
                    .unwrap_or(false);
                if is_required {
                    let edge_exists = self
                        .store
                        .relationship_exists(source_id, target_id, relationship_name)
                        .await
                        .map_err(|e| {
                            NodeServiceError::query_failed(format!(
                                "Failed to check relationship existence: {}",
                                e
                            ))
                        })?;
                    let total = self
                        .store
                        .check_relationship_exists(source_id, relationship_name)
                        .await
                        .map_err(|e| {
                            NodeServiceError::query_failed(format!(
                                "Failed to count relationship edges: {}",
                                e
                            ))
                        })?;
                    if edge_exists && total <= 1 {
                        return Err(NodeServiceError::invalid_update(format!(
                            "Relationship '{}' is required and this is its last edge; add another target before removing this one",
                            relationship_name
                        )));
                    }
                }
            }
        }

        let rel_id = self
            .store
            .get_relationship_id(source_id, target_id, relationship_name)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get relationship ID: {}", e))
            })?;

        self.store
            .delete_generic_relationship(source_id, target_id, relationship_name)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to delete relationship: {}", e))
            })?;
        if relationship_name == "has_child" && rel_id.is_some() {
            self.refresh_for_rootness(target_id, true, Some(source_id))
                .await;
        }

        // Emit RelationshipDeleted event. Normalize ids — same
        // rationale as the other `RelationshipDeleted` sites; see
        // `db::events::node_thing`.
        if let Some(id) = rel_id {
            self.emit_event(DomainEvent::RelationshipDeleted {
                id,
                from_id: crate::db::events::node_thing(source_id),
                to_id: crate::db::events::node_thing(target_id),
                relationship_type: relationship_name.to_string(),
            });
        }

        Ok(())
    }

    /// Replace the stored edge attributes on an existing typed relationship.
    ///
    /// Overwrites the `properties` JSON of the edge (`source_id` → `target_id`,
    /// of type `relationship_name`) wholesale with `properties`. The edge must
    /// already exist — updating a nonexistent edge is a caller error, surfaced
    /// as `invalid_update`, not a silent no-op. Edits values only: it neither
    /// creates, moves, nor deletes the edge, and never changes its endpoints
    /// — this is the relationship viewer's in-place edge-attribute edit
    /// endpoint. Emits `RelationshipUpdated`
    /// so the change syncs like any other edge write.
    pub async fn update_relationship_properties(
        &self,
        source_id: &str,
        relationship_name: &str,
        target_id: &str,
        properties: serde_json::Value,
    ) -> Result<(), NodeServiceError> {
        // Edge attributes are a JSON object (mirrors `edge_data` on create, which
        // defaults to `{}`). Reject a scalar/array/null so an edit can't replace a
        // structured edge-fields blob with a shape `get_related_nodes_with_edges`
        // consumers don't expect.
        if !properties.is_object() {
            return Err(NodeServiceError::invalid_update(format!(
                "Relationship properties must be a JSON object, got {}",
                match &properties {
                    serde_json::Value::Null => "null",
                    serde_json::Value::Bool(_) => "a boolean",
                    serde_json::Value::Number(_) => "a number",
                    serde_json::Value::String(_) => "a string",
                    serde_json::Value::Array(_) => "an array",
                    serde_json::Value::Object(_) => "an object",
                }
            )));
        }

        // A non-builtin edge whose source is a schema node is a relationship
        // DECLARATION, and its `properties` column holds the authoritative
        // `SchemaRelationship` — overwriting it with edge-attribute JSON would
        // corrupt the declaration (it then fails to parse and silently
        // disappears from every read path). Builtin edges are exempt: e.g. a
        // schema's description subtree legitimately carries `has_child` edges
        // whose order attribute may be rewritten.
        if !crate::models::schema::is_builtin_relationship(relationship_name) {
            if let Some(source) = self.get_node(source_id).await? {
                if source.node_type == "schema" {
                    return Err(NodeServiceError::invalid_update(format!(
                        "'{}' is a schema node; '{}' is a relationship declaration — \
                         edit it via update_schema, not update_relationship_properties",
                        source_id, relationship_name
                    )));
                }

                // Validate the replacement attributes against the declared edge
                // fields, so an in-place edit cannot introduce an enum value
                // that `create_relationship` would have rejected. A missing
                // schema or undeclared relationship is not this method's error
                // to raise — the edge already exists, and the update below
                // reports a genuinely absent edge on its own.
                // Chain-aware (ADR-078): an edge-field enum declared only on an
                // ancestor schema (inherited, not redeclared on this subtype)
                // must still be validated — same merged/effective set
                // `resolve_declared_relationship` resolves against on create.
                // Bound to a local rather than chained off the `await?`: the
                // borrowed edge fields must outlive the relationship list they
                // come from, and an inline chain only keeps that alive by
                // virtue of temporary-lifetime extension in the `if let`
                // scrutinee.
                let (relationships, _) = self.resolve_relationships(&source.node_type).await?;
                if let Some(edge_fields) = relationships
                    .iter()
                    .find(|r| r.name == relationship_name)
                    .and_then(|rel| rel.edge_fields.as_deref())
                {
                    validate_edge_data_against_fields(&properties, edge_fields, relationship_name)?;
                }
            }
        }

        let rel_id = self
            .store
            .update_relationship_properties(source_id, target_id, relationship_name, &properties)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to update relationship properties: {}",
                    e
                ))
            })?;

        let Some(rel_id) = rel_id else {
            return Err(NodeServiceError::invalid_update(format!(
                "Relationship '{}' from '{}' to '{}' does not exist",
                relationship_name, source_id, target_id
            )));
        };

        self.emit_event(DomainEvent::RelationshipUpdated {
            relationship: crate::db::events::RelationshipEvent::new(
                rel_id,
                source_id,
                target_id,
                relationship_name.to_string(),
                properties,
            ),
        });

        Ok(())
    }

    /// Get all related nodes for a given relationship
    ///
    /// Queries the relationship table and returns all target nodes connected via the specified
    /// relationship. Supports both "out" and "in" directions.
    ///
    /// # TODO: UI components needed for relationship interaction
    ///
    /// # Arguments
    ///
    /// * `node_id` - ID of the node to get relationships for
    /// * `relationship_name` - Name of the relationship
    /// * `direction` - Direction to traverse ("out" for forward, "in" for reverse)
    ///
    /// # Returns
    ///
    /// Vector of related nodes
    ///
    /// # Errors
    ///
    /// - `NodeNotFound` - Source node doesn't exist
    /// - `SchemaNotFound` - Source node's schema doesn't exist
    /// - `RelationshipNotFound` - Relationship not defined in schema
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
    /// // Get all people assigned to this task
    /// let assigned = service.get_related_nodes("task-123", "assigned_to", "out").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_related_nodes(
        &self,
        node_id: &str,
        relationship_name: &str,
        direction: &str,
    ) -> Result<Vec<Node>, NodeServiceError> {
        if direction != "out" && direction != "in" {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid direction '{}', must be 'out' or 'in'",
                direction
            )));
        }
        self.store
            .get_nodes_by_relationship(node_id, relationship_name, direction)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get related nodes: {}", e))
            })
    }

    /// Get related nodes together with each connecting edge's stored properties.
    ///
    /// Same traversal as [`get_related_nodes`](Self::get_related_nodes) but also
    /// returns the `relationship.properties` JSON for each edge, so callers can
    /// display edge attributes (e.g. a `role`/`assigned_at` carried on an
    /// `assigned_to` edge). Used by the relationship viewer aggregation
    /// (`rel_ops::get_node_relationships`). Returns `(node,
    /// edge_properties)` pairs.
    pub async fn get_related_nodes_with_edges(
        &self,
        node_id: &str,
        relationship_name: &str,
        direction: &str,
    ) -> Result<Vec<(Node, serde_json::Value)>, NodeServiceError> {
        if direction != "out" && direction != "in" {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid direction '{}', must be 'out' or 'in'",
                direction
            )));
        }
        self.store
            .get_related_nodes_with_edges(node_id, relationship_name, direction)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to get related nodes with edges: {}",
                    e
                ))
            })
    }

    /// Get inbound relationships for a node type
    ///
    /// Returns all relationships from other schemas that point TO this node
    /// type, resolved from the table-hydrated schema set (`get_all_schemas`).
    ///
    /// # Arguments
    ///
    /// * `target_type` - The node type to find inbound relationships for (e.g., "customer")
    ///
    /// # Returns
    ///
    /// Vector of tuples: (source_schema_id, relationship)
    ///
    /// # Example
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
    /// // What relationships point TO customer?
    /// let inbound = service.get_inbound_relationships("customer").await?;
    /// for (source_type, rel) in inbound {
    ///     println!("{}.{} -> customer", source_type, rel.name);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_inbound_relationships(
        &self,
        target_type: &str,
    ) -> Result<Vec<(String, crate::models::schema::SchemaRelationship)>, NodeServiceError> {
        let schemas = self.get_all_schemas().await?;

        // A relationship targeting an ancestor also reaches its descendants
        // (ADR-078): `task.blocks` targets `task`, and an `issue` IS a task, so
        // `blocked_by` must resolve on an issue exactly as it does on a task.
        //
        // Without this the inheritance is half-real — an `issue` inherits
        // `task`'s fields and matches `task`-scoped queries, but a relationship
        // pointing at `task` resolves to nothing from the issue's end. That
        // asymmetry is invisible at save time (a Play referencing
        // `node.blocked_by` validates fine) and silently empty at runtime,
        // which makes a condition over it read as false rather than fail.
        let scope_chain = self.resolve_type_chain(target_type).await?;

        let mut inbound = Vec::new();
        for schema in schemas {
            for relationship in schema.relationships {
                // Include typed relationships matching this target or any of
                // its ancestors, plus untyped (None) relationships.
                let matches = relationship
                    .target_type
                    .as_deref()
                    .map(|t| scope_chain.iter().any(|scope| scope == t))
                    .unwrap_or(true); // None = untyped, applies to all types
                if matches {
                    inbound.push((schema.id.clone(), relationship));
                }
            }
        }

        Ok(inbound)
    }

    /// Get relationship graph summary for NLP
    ///
    /// Returns a summary of all relationships in the system, useful for
    /// NLP to understand the overall data model structure. Chain-aware
    /// (ADR-078): each schema's *effective* relationship set is listed, so
    /// an extending type's inherited relationships appear under it too.
    ///
    /// Excludes the `extends`/`extended_by` type-system bookkeeping edge —
    /// `resolve_relationships` filters it as not a real, instance-carried
    /// relationship (see its doc). A schema's `extends` declaration is
    /// therefore never surfaced as a `(child, "extends", parent)` tuple here.
    ///
    /// # Returns
    ///
    /// Vector of tuples: (source_type, relationship_name, target_type)
    ///
    /// # Example
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
    /// let graph = service.get_relationship_graph().await?;
    /// for (source, rel_name, target) in graph {
    ///     let target_str = target.as_deref().unwrap_or("*");
    ///     println!("{} --{}-> {}", source, rel_name, target_str);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_relationship_graph(
        &self,
    ) -> Result<Vec<(String, String, Option<String>)>, NodeServiceError> {
        let schemas = self.get_all_schemas().await?;

        // Chain-aware (ADR-078): resolve each schema's effective/merged
        // relationship set rather than its own declarations only, so an
        // extending type's inherited relationships (e.g. `task.blocks` as
        // seen from `issue`) show up in the graph exactly as they're
        // actually creatable via `create_relationship`.
        let mut edges = Vec::new();
        for schema in schemas {
            let (relationships, _) = self.resolve_relationships(&schema.id).await?;
            for relationship in relationships {
                edges.push((
                    schema.id.clone(),
                    relationship.name.clone(),
                    relationship.target_type.clone(),
                ));
            }
        }

        Ok(edges)
    }
}

/// Validate edge attribute values against the relationship's declared
/// `edgeFields`, rejecting anything an `enum` field does not admit.
///
/// This is the edge-side counterpart to `validate_node_with_fields` (node
/// properties) and runs on every write path — `create_relationship` and
/// `update_relationship_properties` — so the CLI's `--edge-data`, the daemon,
/// the Tauri commands and the agent tools are all covered by one check rather
/// than each boundary validating (or forgetting to validate) on its own.
///
/// Scope is deliberately limited to enum membership. Broader edge-value typing
/// (numbers, dates, required-ness) is not enforced anywhere today; adding it
/// here would change the acceptance of existing callers well beyond an enum
/// value set. An enum is the case where an unconstrained value is actively
/// harmful: a role of `"onwer"` is a permission that silently grants nothing.
///
/// Keys with no declared edge field are left alone — undeclared edge attributes
/// are an existing, supported shape (the viewer renders them as free text).
fn validate_edge_data_against_fields(
    edge_data: &serde_json::Value,
    edge_fields: &[crate::models::schema::EdgeField],
    relationship_name: &str,
) -> Result<(), NodeServiceError> {
    let Some(obj) = edge_data.as_object() else {
        return Ok(());
    };

    for field in edge_fields {
        if field.field_type != "enum" {
            continue;
        }
        let Some(value) = obj.get(&field.name) else {
            continue;
        };
        // An explicit null clears the attribute rather than setting an illegal
        // value, matching how a null enum is treated on node properties.
        if value.is_null() {
            continue;
        }

        let values = field.core_values.as_deref().unwrap_or(&[]);

        let Some(value_str) = value.as_str() else {
            return Err(NodeServiceError::invalid_update(format!(
                "Edge field '{}' on relationship '{}' is an enum, so its value must be a string \
                 or null, got {}",
                field.name,
                relationship_name,
                match value {
                    serde_json::Value::Bool(_) => "a boolean",
                    serde_json::Value::Number(_) => "a number",
                    serde_json::Value::Array(_) => "an array",
                    serde_json::Value::Object(_) => "an object",
                    _ => "another type",
                }
            )));
        };

        if !values.iter().any(|ev| ev.value == value_str) {
            return Err(NodeServiceError::invalid_update(format!(
                "Invalid value '{}' for enum edge field '{}' on relationship '{}'. Valid values: {}",
                value_str,
                field.name,
                relationship_name,
                values
                    .iter()
                    .map(|ev| format!("{} ({})", ev.label, ev.value))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }

    Ok(())
}
