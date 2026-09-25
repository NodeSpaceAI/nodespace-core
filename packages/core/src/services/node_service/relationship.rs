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
    async fn resolve_declared_relationship(
        &self,
        schema_id: &str,
        relationship_name: &str,
    ) -> Result<crate::models::schema::SchemaRelationship, NodeServiceError> {
        // Hydrated fetch: declarations come from the relationship table via
        // the one shared store query path (`get_schema_declarations`).
        let chain = self.resolve_type_chain(schema_id).await?;

        for scope in &chain {
            let Some(schema_node) = self.get_schema_node(scope).await? else {
                continue;
            };
            if let Some(rel) = schema_node.get_relationship(relationship_name) {
                return Ok(rel.clone());
            }
        }

        // Nearest scope first, so the node's own type is reported even when the
        // chain is longer — that is the type the caller named.
        Err(NodeServiceError::invalid_update(format!(
            "Relationship '{}' not defined in schema '{}'. Built-in relationships (member_of, has_child, mentions, has_role) are universal.",
            relationship_name, schema_id
        )))
    }

    /// The forward name an edge addressed through an `in`-direction
    /// declaration's name is stored under.
    ///
    /// An edge is stored once: `relationship_type` is the forward (`out`)
    /// name, `in_node` its source, `out_node` its target. An `in` declaration
    /// is the target's view of that same edge, and its `reverse_name` is the
    /// forward name — so `old --superseded_by--> new` is `new --supersedes-->
    /// old` written from the other end. Every write path rewrites such a call
    /// to the forward spelling (endpoints swapped) before touching storage,
    /// so one logical edge has exactly one storage shape and the forward-name
    /// validation (target type, cardinality on both ends, required last-edge
    /// protection) applies to it unchanged.
    ///
    /// `schema_id` is the addressed source's type; `None` (a builtin, or a
    /// source that does not exist) never rewrites. Returns `None` too when
    /// `relationship_name` does not resolve on the extends chain to an `in`
    /// declaration — including an undeclared name, which the caller's own
    /// resolution reports.
    async fn in_declaration_forward_name(
        &self,
        schema_id: Option<&str>,
        relationship_name: &str,
    ) -> Result<Option<String>, NodeServiceError> {
        let Some(schema_id) = schema_id else {
            return Ok(None);
        };
        let (relationships, _) = self.resolve_relationships(schema_id).await?;
        Ok(relationships
            .into_iter()
            .find(|r| {
                r.name == relationship_name
                    && r.direction == crate::models::schema::RelationshipDirection::In
            })
            .map(|r| r.reverse_name))
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

        // A write through an `in` declaration's name is stored as the forward
        // edge — see `in_declaration_forward_name`. Everything below then
        // validates and stores the forward spelling.
        let source_type = if is_builtin {
            None
        } else {
            Self::get_node_in_tx_or_virtual_date(tx, source_id)
                .await?
                .map(|n| n.node_type)
        };
        let forward_name = self
            .in_declaration_forward_name(source_type.as_deref(), relationship_name)
            .await?;
        let requested_name = relationship_name;
        let (source_id, relationship_name, target_id) = forward_endpoints(
            forward_name.as_deref(),
            source_id,
            relationship_name,
            target_id,
        );

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
            // A rewritten `in` write must land on the forward declaration that
            // mirrors it. Schema save enforces the pairing, so this only
            // catches a pair broken below it — `set_schema_relationships`
            // (which Play actions call) writes declarations unvalidated.
            // Storing under anything else (an undeclared name, another `in`,
            // a forward naming a different reverse) would reintroduce a second
            // storage shape. Report it in the caller's own terms.
            let resolved = self
                .resolve_declared_relationship(schema_id, relationship_name)
                .await;
            let relationship = match (&forward_name, resolved) {
                (None, resolved) => resolved?,
                (Some(_), Ok(rel))
                    if rel.direction == crate::models::schema::RelationshipDirection::Out
                        && rel.reverse_name == requested_name =>
                {
                    rel
                }
                (Some(_), _) => {
                    return Err(NodeServiceError::invalid_update(format!(
                        "'{}' on '{}' is the inbound view of '{}.{}', which '{}' does not \
                         declare as an outbound relationship naming it back",
                        requested_name,
                        source_type.as_deref().unwrap_or_default(),
                        schema_id,
                        relationship_name,
                        schema_id
                    )));
                }
            };

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

        // Same forward spelling the create path stored — see
        // `in_declaration_forward_name`.
        let source_type = if is_builtin {
            None
        } else {
            crate::db::SqliteStore::get_node_in_tx(tx.store_tx(), source_id)
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
                .map(|n| n.node_type)
        };
        let forward_name = self
            .in_declaration_forward_name(source_type.as_deref(), relationship_name)
            .await?;
        let (source_id, relationship_name, target_id) = forward_endpoints(
            forward_name.as_deref(),
            source_id,
            relationship_name,
            target_id,
        );

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

        // Same forward spelling the create path stored — see
        // `in_declaration_forward_name`.
        let source_type = if is_builtin {
            None
        } else {
            self.get_node(source_id).await?.map(|n| n.node_type)
        };
        let forward_name = self
            .in_declaration_forward_name(source_type.as_deref(), relationship_name)
            .await?;
        let (source_id, relationship_name, target_id) = forward_endpoints(
            forward_name.as_deref(),
            source_id,
            relationship_name,
            target_id,
        );

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
        let is_builtin = crate::models::schema::is_builtin_relationship(relationship_name);

        // Same forward spelling the create path stored — see
        // `in_declaration_forward_name`.
        let source_type = if is_builtin {
            None
        } else {
            self.get_node(source_id).await?.map(|n| n.node_type)
        };
        let forward_name = self
            .in_declaration_forward_name(source_type.as_deref(), relationship_name)
            .await?;
        // Named as the caller wrote it, for the missing-edge error below.
        let requested = format!("'{relationship_name}' from '{source_id}' to '{target_id}'");
        let (source_id, relationship_name, target_id) = forward_endpoints(
            forward_name.as_deref(),
            source_id,
            relationship_name,
            target_id,
        );

        if !is_builtin {
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
                "Relationship {requested} does not exist"
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
                // Only a forward (`out`) declaration describes stored edges. An
                // `in` declaration is its target's name for another schema's
                // forward edge — writes through it are stored as that edge — so
                // it never has instance edges of its own pointing anywhere.
                if relationship.direction != crate::models::schema::RelationshipDirection::Out {
                    continue;
                }
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

/// `(source, name, target)` as stored: swapped onto `forward_name` when
/// [`NodeService::in_declaration_forward_name`] found one, unchanged otherwise.
fn forward_endpoints<'a>(
    forward_name: Option<&'a str>,
    source_id: &'a str,
    relationship_name: &'a str,
    target_id: &'a str,
) -> (&'a str, &'a str, &'a str) {
    match forward_name {
        Some(name) => (target_id, name, source_id),
        None => (source_id, relationship_name, target_id),
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
