//! Hierarchy, tree-navigation, and move/reorder operations for NodeService.
use super::*;

impl NodeService {
    /// Get children of a node
    ///
    /// Returns all direct children of the specified parent node.
    ///
    /// # Arguments
    ///
    /// * `parent_id` - The parent node ID
    ///
    /// # Returns
    ///
    /// Vector of child nodes (empty if no children)
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
    /// let children = service.get_children("parent-id").await?;
    /// println!("Found {} children", children.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_children(&self, parent_id: &str) -> Result<Vec<Node>, NodeServiceError> {
        // Use edge-based query from SqliteStore (graph-native architecture)
        // Children are already sorted by fractional order on edges
        self.store
            .get_children(parent_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Returns all root nodes — nodes with no parent edge in the graph.
    pub async fn get_roots(
        &self,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<Node>, NodeServiceError> {
        self.store
            .get_roots(limit, offset)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Count root nodes without listing them — the O(1)-response-size
    /// counterpart to `get_roots`, for callers (e.g. `nodespace diagnostics`)
    /// that only need a total.
    pub async fn count_roots(&self) -> Result<i64, NodeServiceError> {
        self.store
            .count_roots()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Get all descendants of a node (recursive children)
    ///
    /// Fetches all nodes in the subtree rooted at the specified node,
    /// excluding the root node itself. Uses iterative breadth-first traversal.
    ///
    /// # Arguments
    ///
    /// * `root_id` - The root node ID to fetch descendants for
    ///
    /// # Returns
    ///
    /// `Vec<Node>` containing all descendant nodes (not including the root)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # async fn example(service: NodeService) -> Result<(), Box<dyn std::error::Error>> {
    /// let descendants = service.get_descendants("parent-123").await?;
    /// println!("Found {} descendants", descendants.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_descendants(&self, root_id: &str) -> Result<Vec<Node>, NodeServiceError> {
        // Use store's breadth-first traversal implementation
        let descendants = self
            .store
            .get_nodes_in_subtree(root_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        Ok(descendants)
    }

    /// Get a complete nested tree structure using efficient adjacency list strategy
    pub async fn get_children_tree(
        &self,
        parent_id: &str,
    ) -> Result<serde_json::Value, NodeServiceError> {
        // Use shared subtree data fetching
        let (root_node, node_map, adjacency_list) = self.get_subtree_data(parent_id).await?;

        // Checked before the mention lookup and tree build/serialization below,
        // so an oversized subtree fails fast without paying for that remaining
        // work — rather than producing a JSON payload the gRPC transport then
        // rejects at decode time with an opaque OutOfRange. This does NOT
        // short-circuit the DB read above: get_subtree_with_relationships is a
        // single consolidated query that already fetched the whole subtree by
        // the time node_map.len() is known, since splitting "count first, fetch
        // second" would need a separate query.
        if node_map.len() > MAX_TREE_NODES {
            return Err(NodeServiceError::tree_too_large(
                parent_id,
                node_map.len(),
                MAX_TREE_NODES,
            ));
        }

        match root_node {
            Some(root) => {
                // Backlinks (mentioned_in) are fetched as their own resource via
                // `get_mentioning_containers`, in parallel with children — not
                // carried on the tree payload. See ADR/mentions-and-references.md.
                //
                // Recursively build tree structure. Errors on cyclic or
                // pathologically deep hierarchy data rather than recursing
                // until the stack aborts the process.
                build_node_tree_recursive(&root, &node_map, &adjacency_list)
            }
            None => {
                // Root node not found, return empty object
                Ok(serde_json::json!({}))
            }
        }
    }

    /// Fetch all data needed to traverse a subtree efficiently
    pub async fn get_subtree_data(&self, root_id: &str) -> Result<SubtreeData, NodeServiceError> {
        use std::collections::HashMap;

        // Single consolidated query fetches root + all descendants + all relationships
        let (all_nodes, relationships) = self
            .store
            .get_subtree_with_relationships(root_id)
            .await
            .map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to fetch subtree: {}", e))
        })?;

        // Find root node from the results
        let root_node = all_nodes.iter().find(|n| n.id == root_id).cloned();

        // Create a map of node_id → Node for O(1) lookup
        let mut node_map: HashMap<String, Node> = HashMap::new();
        for node in all_nodes {
            node_map.insert(node.id.clone(), node);
        }

        // Create adjacency list: parent_id → Vec of child_ids (sorted by order)
        // RelationshipRecord now stores order in properties, accessed via order() method
        let mut adjacency_with_order: HashMap<String, Vec<(String, f64)>> = HashMap::new();
        for rel in relationships {
            adjacency_with_order
                .entry(rel.in_node.clone())
                .or_default()
                .push((rel.out_node.clone(), rel.order()));
        }

        // Sort children by order for each parent, then extract just the IDs
        let mut adjacency_list: HashMap<String, Vec<String>> = HashMap::new();
        for (parent_id, mut children) in adjacency_with_order {
            children.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            adjacency_list.insert(parent_id, children.into_iter().map(|(id, _)| id).collect());
        }

        Ok((root_node, node_map, adjacency_list))
    }

    /// Check if a node is a root node (has no parent)
    pub async fn is_root_node(&self, node_id: &str) -> Result<bool, NodeServiceError> {
        // A node is a root if it has no incoming has_child relationships
        // We check this by trying to get its parent - if parent is None, it's a root
        let parent = self.get_parent(node_id).await?;
        Ok(parent.is_none())
    }

    /// Get the parent of a node (via incoming has_child relationship)
    pub async fn get_parent(&self, node_id: &str) -> Result<Option<Node>, NodeServiceError> {
        // Query for nodes that have has_child relationship pointing to this node
        // This is done by querying the relationships table for has_child edges into this node
        let parent = self
            .store
            .get_parent(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        Ok(parent)
    }

    /// Get the incoming `has_child` edge's own `modified_at` timestamp.
    ///
    /// This is distinct from [`get_parent`](Self::get_parent), which returns
    /// the *parent node's* `modified_at`, and from the child node's own
    /// `modified_at`: it is the relationship row's timestamp, tracking only
    /// when the parent edge itself was last written (created, re-pointed, or
    /// reordered) — independent of either endpoint node's own edit history.
    /// Callers that need edge-level recency specifically (e.g. comparing a
    /// local vs. a remote parent edge to resolve which structural change is
    /// newer) read this instead of either node's timestamp, since a node's
    /// `modified_at` conflates content and structural changes and does not
    /// isolate the edge's own history.
    ///
    /// Returns `None` if the node currently has no parent edge (it's a root).
    pub async fn get_parent_edge_modified_at(
        &self,
        node_id: &str,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, NodeServiceError> {
        self.store
            .get_parent_edge_modified_at(node_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))
    }

    /// Get the root (root ancestor) of a node
    pub async fn get_root_id(&self, node_id: &str) -> Result<String, NodeServiceError> {
        let mut current_id = node_id.to_string();

        // Traverse up the parent chain until we find a root
        // Uses get_parent_id for efficiency (no full node fetch)
        loop {
            let parent_id = self
                .store
                .get_parent_id(&current_id)
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

            match parent_id {
                Some(pid) => {
                    // Keep traversing up
                    current_id = pid;
                }
                None => {
                    // Found the root
                    return Ok(current_id);
                }
            }
        }
    }

    /// Bulk fetch all nodes belonging to an origin node (viewer/page)
    ///
    /// This is the efficient way to load a complete document tree:
    /// 1. Single database query fetches all nodes with the same root_id
    /// 2. In-memory hierarchy reconstruction using parent_id and before_sibling_id
    ///
    /// This avoids making multiple queries for each level of the tree.
    ///
    /// # Arguments
    ///
    /// * `root_node_id` - The ID of the origin node (e.g., date page ID)
    ///
    /// # Returns
    ///
    /// Vector of all nodes that belong to this origin, unsorted.
    /// Caller should use `sort_by_sibling_order()` or build a tree structure.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Fetch all nodes for a date page
    /// let nodes = service.get_nodes_by_root_id("2025-10-05").await?;
    /// println!("Found {} nodes in this document", nodes.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_nodes_by_root_id(
        &self,
        root_node_id: &str,
    ) -> Result<Vec<Node>, NodeServiceError> {
        // Hierarchy is now managed via relationships - use get_children instead
        self.get_children(root_node_id).await
    }

    /// Move a node to a new parent without version checking (no OCC).
    ///
    /// **Prefer `move_node()`** which enforces optimistic concurrency control.
    /// This unchecked variant is for internal operations (imports, type
    /// conversions) where version conflicts are not a concern.
    ///
    /// Updates the parent_id and root_id of a node, maintaining hierarchy consistency.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to move
    /// * `new_parent` - The new parent ID (None to make it a root node)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node doesn't exist
    /// - New parent doesn't exist
    /// - Move would create circular reference
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::{NodeService, InsertPosition};
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Move node under new parent, appending at end
    /// service.move_node_unchecked("node-id", Some("new-parent-id"), InsertPosition::End).await?;
    ///
    /// // Make node a root
    /// service.move_node_unchecked("node-id", None, InsertPosition::End).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn move_node_unchecked(
        &self,
        node_id: &str,
        new_parent: Option<&str>,
        position: crate::services::InsertPosition<'_>,
    ) -> Result<(), NodeServiceError> {
        // Verify node exists
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Date nodes are top-level containers and cannot be moved
        if node.node_type == "date" {
            return Err(NodeServiceError::hierarchy_violation(format!(
                "Date node '{}' cannot be moved (it's a top-level container)",
                node_id
            )));
        }

        // Verify new parent exists if provided
        if let Some(parent_id) = new_parent {
            let parent_exists = self.node_exists(parent_id).await?;
            if !parent_exists {
                return Err(NodeServiceError::invalid_parent(parent_id));
            }

            // Check for circular reference - parent_id cannot be a descendant of node_id
            if self.is_descendant(node_id, parent_id).await? {
                return Err(NodeServiceError::circular_reference(format!(
                    "Cannot move node {} under its descendant {}",
                    node_id, parent_id
                )));
            }
        }

        let former_parent = self.get_parent(node_id).await?.map(|p| p.id);
        let insert_after = self.resolve_insert_position(position, new_parent).await?;

        // Hierarchy is now managed via relationships - use store's move_node
        let actual_order = self
            .store
            .move_node(node_id, new_parent, insert_after.as_deref())
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        self.refresh_for_rootness(
            node_id,
            new_parent.is_none(),
            former_parent.as_deref().filter(|p| Some(*p) != new_parent),
        )
        .await;

        // Emit RelationshipUpdated event (unified relationship events)
        if let Some(parent_id) = new_parent {
            self.emit_event(DomainEvent::RelationshipUpdated {
                relationship: crate::db::events::RelationshipEvent::new(
                    format!("relationship:{}:{}", parent_id, node_id),
                    parent_id,
                    node_id,
                    "has_child",
                    serde_json::json!({"order": actual_order}),
                ),
            });
        }

        Ok(())
    }

    /// Move a node to a new parent with OCC (Optimistic Concurrency Control)
    ///
    /// This method validates version before moving, preventing concurrent modifications
    /// from silently overwriting each other. The node's version is bumped after a
    /// successful move.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to move
    /// * `expected_version` - The version the caller expects (for OCC)
    /// * `new_parent` - The new parent ID (None to make it a root node)
    /// * `position` - Where to insert among the new parent's children
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node doesn't exist
    /// - Version doesn't match (concurrent modification detected)
    /// - New parent doesn't exist
    /// - Move would create circular reference
    /// - Node is a date container (cannot be moved)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::{NodeService, InsertPosition};
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Move node under new parent, appending at end
    /// service.move_node("node-id", 5, Some("new-parent-id"), InsertPosition::End).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn move_node(
        &self,
        node_id: &str,
        expected_version: i64,
        new_parent: Option<&str>,
        position: crate::services::InsertPosition<'_>,
    ) -> Result<Node, NodeServiceError> {
        // Get current node and verify version
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Check version before proceeding
        if node.version != expected_version {
            return Err(NodeServiceError::version_conflict(
                node_id,
                expected_version,
                node.version,
            ));
        }

        // Date nodes are top-level containers and cannot be moved
        if node.node_type == "date" {
            return Err(NodeServiceError::hierarchy_violation(format!(
                "Date node '{}' cannot be moved (it's a top-level container)",
                node_id
            )));
        }

        // Verify new parent exists if provided
        if let Some(parent_id) = new_parent {
            let parent_node = self
                .get_node(parent_id)
                .await?
                .ok_or_else(|| NodeServiceError::invalid_parent(parent_id))?;

            // Enforce container rule: reject moves into non-container node types
            if !self
                .behavior_for(&parent_node.node_type)
                .can_have_children()
            {
                return Err(NodeServiceError::not_a_container(
                    parent_id,
                    &parent_node.node_type,
                ));
            }

            // Check for circular reference - parent_id cannot be a descendant of node_id
            if self.is_descendant(node_id, parent_id).await? {
                return Err(NodeServiceError::circular_reference(format!(
                    "Cannot move node {} under its descendant {}",
                    node_id, parent_id
                )));
            }
        }

        // Capture the OLD parent before the move so we can surface its edge removal
        // (sync-epic S3): the store deletes the old has_child
        // edge but, historically, only a RelationshipUpdated for the NEW parent was
        // emitted (gated on Some). A move-to-root (new_parent = None) therefore
        // emitted no relationship event at all, so the detach never propagated to
        // other devices; a reparent left a stale cloud edge. We now also emit a
        // RelationshipDeleted for the old parent whenever the parent actually changes.
        let old_parent_id = self.get_parent(node_id).await?.map(|p| p.id);

        let insert_after = self.resolve_insert_position(position, new_parent).await?;

        // Perform the move
        let actual_order = self
            .store
            .move_node(node_id, new_parent, insert_after.as_deref())
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        self.refresh_for_rootness(
            node_id,
            new_parent.is_none(),
            old_parent_id.as_deref().filter(|p| Some(*p) != new_parent),
        )
        .await;

        // ADR-069 §2: bump the version BEFORE emitting any event, not after.
        // The store's move_node and this bump remain two separate atomic
        // writes (the store confirmed already-atomic for the move itself —
        // merging them into one DB transaction is not what this fix needs),
        // but a consumer must never be told about a move before the write
        // that makes it OCC-safe has actually landed. The previous order
        // announced the move first; a failed bump then left consumers
        // believing a move that was not durably version-committed, with no
        // way to tell from the event stream alone.
        //
        // Even though we're only modifying edge relationships, we bump the
        // node version so that concurrent move operations will fail with
        // version conflict. Returns the updated node with new version so
        // frontend can sync its local state.
        let updated_node = self
            .update_node_with_version_bump(node_id, expected_version)
            .await?;

        // Emit RelationshipUpdated event (unified relationship events).
        // Emit the NEW-parent edge first so a consumer that inserts-then-deletes
        // never sees the node parentless mid-move.
        if let Some(parent_id) = new_parent {
            self.emit_event(DomainEvent::RelationshipUpdated {
                relationship: crate::db::events::RelationshipEvent::new(
                    format!("relationship:{}:{}", parent_id, node_id),
                    parent_id,
                    node_id,
                    "has_child",
                    serde_json::json!({"order": actual_order}),
                ),
            });
        }

        // Surface the removal of the OLD parent edge when the parent actually
        // changed (move-to-root OR reparent) — not on a same-parent position move.
        if let Some(old_id) = old_parent_id {
            if new_parent != Some(old_id.as_str()) {
                self.emit_event(DomainEvent::RelationshipDeleted {
                    id: format!("relationship:{}:{}", old_id, node_id),
                    from_id: crate::db::events::node_thing(&old_id),
                    to_id: crate::db::events::node_thing(node_id),
                    relationship_type: "has_child".to_string(),
                });
            }
        }

        Ok(updated_node)
    }

    /// Reorder a node within its siblings with OCC
    ///
    /// This method validates version, prevents root reordering, and bumps
    /// node version after reordering for OCC safety.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to reorder
    /// * `expected_version` - Version for optimistic concurrency control
    /// * `insert_after` - Sibling to position after (None = first position)
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node not found
    /// - Version mismatch
    /// - Node is a root (roots cannot be reordered)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::{NodeService, InsertPosition};
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Reorder after a sibling
    /// service.reorder_node("node-id", 5, InsertPosition::After("sibling-id")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reorder_node(
        &self,
        node_id: &str,
        expected_version: i64,
        position: crate::services::InsertPosition<'_>,
    ) -> Result<(), NodeServiceError> {
        // Get current node and verify version
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Check version before proceeding
        if node.version != expected_version {
            return Err(NodeServiceError::version_conflict(
                node_id,
                expected_version,
                node.version,
            ));
        }

        // Root nodes cannot be reordered (they have no parent)
        if self.is_root_node(node_id).await? {
            return Err(NodeServiceError::hierarchy_violation(format!(
                "Root node '{}' cannot be reordered (it has no parent)",
                node_id
            )));
        }

        // Use graph-native reordering. ADR-069 §2/S4: the write happens here,
        // but the event is deferred until after the version bump below — see
        // `reorder_child_write`'s doc comment for why, mirroring the
        // `move_node` fix.
        let (parent_id, actual_order) = self.reorder_child_write(node_id, position).await?;

        // Bump the node's version to support OCC
        // Even though we're only modifying edge ordering, we bump the node version
        // so that concurrent reorder operations will fail with version conflict
        // Note: We discard the returned Node since reorder_node returns ()
        let _ = self
            .update_node_with_version_bump(node_id, expected_version)
            .await?;

        self.emit_reorder_event(node_id, parent_id.as_deref(), actual_order);

        Ok(())
    }

    /// Atomically re-parent an ordered set of existing children to `new_parent_id`
    /// in a single transaction (all-or-nothing OCC).
    ///
    /// The edge swap, each child's title refresh and version bump all run in ONE
    /// transaction. If any child has a version mismatch — at the edge swap or at
    /// the bump — the entire batch is rolled back: nothing moves, nothing is
    /// bumped. Each child's `RelationshipUpdated` event (for the frontend
    /// hierarchy-sync path to reconcile order idempotently) is buffered and
    /// flushed only after that transaction commits (ADR-069 §2).
    ///
    /// # Arguments
    ///
    /// * `new_parent_id` — freshly-created split node; must be an empty container
    /// * `children`      — `(node_id, expected_version)` pairs in sibling order
    pub async fn move_children_to_parent(
        &self,
        new_parent_id: &str,
        children: &[(String, i64)],
    ) -> Result<Vec<Node>, NodeServiceError> {
        if children.is_empty() {
            return Ok(Vec::new());
        }

        // Verify new parent exists and can hold children.
        let parent_node = self
            .get_node(new_parent_id)
            .await?
            .ok_or_else(|| NodeServiceError::invalid_parent(new_parent_id))?;

        if !self
            .behavior_for(&parent_node.node_type)
            .can_have_children()
        {
            return Err(NodeServiceError::not_a_container(
                new_parent_id,
                &parent_node.node_type,
            ));
        }

        // Pre-validation: fetch all children, check versions, apply move_node guards.
        // Version conflicts return immediately before any write touches the DB.
        let mut nodes = Vec::with_capacity(children.len());
        let mut former_parents = Vec::with_capacity(children.len());
        for (node_id, expected_version) in children {
            let node = self
                .get_node(node_id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(node_id.as_str()))?;

            if node.version != *expected_version {
                return Err(NodeServiceError::version_conflict(
                    node_id,
                    *expected_version,
                    node.version,
                ));
            }

            // Date nodes are top-level containers and cannot be moved.
            if node.node_type == "date" {
                return Err(NodeServiceError::hierarchy_violation(format!(
                    "Date node '{}' cannot be moved (it's a top-level container)",
                    node_id
                )));
            }

            // Root nodes have no has_child edge to replace, so the in-transaction
            // swap would fail with a generic store error. Reject them here so
            // callers get a clear hierarchy-violation error before any write.
            let Some(former_parent) = self.get_parent(node_id).await?.map(|p| p.id) else {
                return Err(NodeServiceError::hierarchy_violation(format!(
                    "Root node '{}' cannot be batch-moved (no parent edge to replace)",
                    node_id
                )));
            };

            // Cycle guard: the new parent must not be a descendant of any moved child.
            if self.is_descendant(node_id, new_parent_id).await? {
                return Err(NodeServiceError::circular_reference(format!(
                    "Cannot move node {} under its descendant {}",
                    node_id, new_parent_id
                )));
            }

            nodes.push(node);
            former_parents.push(former_parent);
        }

        // The edge swap, rootness refresh, version bumps and event emission
        // form one unit of work (ADR-069 §1a): committing the swap on its own
        // let a concurrent write to a child between it and the bump fail the
        // RPC after the edges had already moved, so the frontend rolled back
        // children the backend had re-parented, and their events never fired.
        // The store re-validates each version inside the transaction
        // (eliminates the TOCTOU against the pre-validation above).
        let new_parent_id = new_parent_id.to_string();
        let children: Vec<(String, i64)> = children.to_vec();
        let service = self.clone();
        let updated: Vec<Node> = self
            .with_transaction(move |tx| {
                Box::pin(async move {
                    let children_with_versions: Vec<(&str, i64)> = children
                        .iter()
                        .map(|(id, ver)| (id.as_str(), *ver))
                        .collect();
                    // The second `?` turns a `VersionConflict` into this
                    // closure's `Err`, which is what rolls back the edges
                    // already swapped for earlier children.
                    let orders = crate::db::SqliteStore::move_children_to_parent_in_tx(
                        tx.store_tx(),
                        &new_parent_id,
                        &children_with_versions,
                    )
                    .await
                    .map_err(|e| NodeServiceError::query_failed(e.to_string()))??;

                    let mut updated = Vec::with_capacity(nodes.len());
                    for ((node, order), former_parent) in
                        nodes.iter().zip(&orders).zip(&former_parents)
                    {
                        // The new parent need not be in the child's tree, so
                        // the child may have left one.
                        service
                            .refresh_for_rootness_in_tx(
                                tx,
                                &node.id,
                                false,
                                Some(former_parent.as_str()).filter(|p| *p != new_parent_id),
                            )
                            .await?;

                        let updated_node = service
                            .update_node_with_version_bump_in_tx(tx, &node.id, node.version)
                            .await?;

                        service.emit_event(crate::db::events::DomainEvent::RelationshipUpdated {
                            relationship: crate::db::events::RelationshipEvent::new(
                                format!("relationship:{}:{}", new_parent_id, node.id),
                                &new_parent_id,
                                &node.id,
                                "has_child",
                                serde_json::json!({"order": order}),
                            ),
                        });

                        updated.push(updated_node);
                    }
                    Ok(updated)
                })
            })
            .await?;

        Ok(updated)
    }

    /// Resolve an `InsertPosition` to a concrete `Option<String>` for the store layer.
    ///
    /// - `Beginning` → `None` (store interprets `None` as "before the first child")
    /// - `End`       → `Some(last_child_id)` (or `None` if the parent has no children yet)
    /// - `After(id)` → `Some(id.to_string())`
    async fn resolve_insert_position(
        &self,
        position: crate::services::InsertPosition<'_>,
        parent_id: Option<&str>,
    ) -> Result<Option<String>, NodeServiceError> {
        match position {
            crate::services::InsertPosition::Beginning => Ok(None),
            crate::services::InsertPosition::After(id) => Ok(Some(id.to_string())),
            crate::services::InsertPosition::End => {
                if let Some(pid) = parent_id {
                    let children = self.get_children(pid).await?;
                    Ok(children.last().map(|n| n.id.clone()))
                } else {
                    // `End` with no parent (root-level moves) resolves to `None`.
                    Ok(None)
                }
            }
        }
    }

    /// Create parent-child edge atomically with sibling positioning
    ///
    /// Used during node creation to establish parent relationship while preserving
    /// sibling ordering. This is separate from move_node() which is for moving existing nodes.
    ///
    /// # Arguments
    ///
    /// * `child_id` - ID of the child node (must already exist)
    /// * `parent_id` - ID of the parent node
    /// * `position` - Where to insert among the parent's children
    pub async fn create_parent_edge(
        &self,
        child_id: &str,
        parent_id: &str,
        position: crate::services::InsertPosition<'_>,
    ) -> Result<(), NodeServiceError> {
        tracing::debug!(
            child_id = %child_id,
            parent_id = %parent_id,
            position = ?position,
            "create_parent_edge: START"
        );

        // Idempotency guard for the alice-side echo — if
        // `child_id` is already a child of `parent_id` AND the position is
        // End (no explicit reorder hint), treat this call as a no-op.
        // `Beginning` and `After(_)` still trigger a real reorder.
        let existing_parent = self.get_parent(child_id).await?.map(|p| p.id);
        if matches!(position, crate::services::InsertPosition::End)
            && existing_parent.as_deref() == Some(parent_id)
        {
            tracing::debug!(
                child_id = %child_id,
                parent_id = %parent_id,
                "create_parent_edge: edge already exists with End position, treating as no-op"
            );
            return Ok(());
        }

        // Resolve InsertPosition::End to the actual last sibling id so the
        // store's move_node gets a concrete Option<&str>.
        let resolved = self
            .resolve_insert_position(position, Some(parent_id))
            .await?;
        let insert_after_id: Option<&str> = resolved.as_deref();

        // SQLite is synchronous/ACID: move_node commits before returning; the result
        // is immediately visible on the next read. Trust the single call result.
        let actual_order = self
            .store
            .move_node(child_id, Some(parent_id), insert_after_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        // `move_node` replaces any existing parent, so this can reparent.
        self.refresh_for_rootness(
            child_id,
            false,
            existing_parent.as_deref().filter(|p| *p != parent_id),
        )
        .await;

        // Emit RelationshipCreated event (unified relationship events)
        self.emit_event(DomainEvent::RelationshipCreated {
            relationship: crate::db::events::RelationshipEvent::new(
                format!("relationship:{}:{}", parent_id, child_id),
                parent_id,
                child_id,
                "has_child",
                serde_json::json!({"order": actual_order}),
            ),
        });

        tracing::debug!("create_parent_edge: COMPLETE");
        Ok(())
    }

    /// `_in_tx` twin of [`Self::create_parent_edge`] (ADR-069 §1b/S2). Same
    /// idempotency posture is NOT reproduced here — callers reach this only
    /// from `create_node_with_parent_in_tx`, where `child_id` is a node this
    /// same transaction just created via `create_node_in_tx`, so it can
    /// never already have a parent edge to be idempotent against. The
    /// sibling-position resolution runs against a pooled reader exactly like
    /// `resolve_insert_position` does today (existing siblings under
    /// `parent_id` are necessarily already-committed data, not something
    /// this transaction is concurrently mutating), then the edge insert
    /// itself lands on `tx.store_tx()`.
    pub(crate) async fn create_parent_edge_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        child_id: &str,
        parent_id: &str,
        position: crate::services::InsertPosition<'_>,
    ) -> Result<(), NodeServiceError> {
        let resolved = self
            .resolve_insert_position(position, Some(parent_id))
            .await?;
        let insert_after_id: Option<&str> = resolved.as_deref();

        let actual_order = crate::db::SqliteStore::create_has_child_edge_in_tx(
            tx.store_tx(),
            parent_id,
            child_id,
            insert_after_id,
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        self.emit_event(DomainEvent::RelationshipCreated {
            relationship: crate::db::events::RelationshipEvent::new(
                format!("relationship:{}:{}", parent_id, child_id),
                parent_id,
                child_id,
                "has_child",
                serde_json::json!({"order": actual_order}),
            ),
        });

        Ok(())
    }

    /// Batched sibling of [`Self::create_parent_edge`] for the sync-apply cold-sweep's
    /// reconnect path: attach many genuinely-unparented children under their parents in
    /// ONE store transaction, then emit one `RelationshipCreated` per created edge —
    /// exactly as the per-row `create_parent_edge` does. (Relationship events are not
    /// node-keyed, so `begin_batch_emit` does NOT coalesce them; they broadcast
    /// immediately, one per edge, matching the per-row path — the batching win is the
    /// single DB transaction, not the events.) `edges` is `(parent, child, order)`
    /// carrying the sender's sibling order; a child that already has a parent is
    /// skipped in the store (see `bulk_create_has_child`), so this only attaches
    /// genuinely-unparented children. Returns the number of edges created.
    ///
    /// Unlike `create_parent_edge` this does NOT reposition — it is intended for the
    /// from-scratch (cold) sweep where every parent is fresh, so the sender's `order`
    /// values are the final sibling order. Repositioning / non-fresh parents stay on
    /// the per-row path.
    pub async fn bulk_create_has_child_edges(
        &self,
        edges: &[(String, String, f64)],
    ) -> Result<usize, NodeServiceError> {
        if edges.is_empty() {
            return Ok(0);
        }
        let created = self
            .store
            .bulk_create_has_child(edges)
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        for (parent, child, order) in &created {
            self.refresh_for_rootness(child, false, None).await;
            self.emit_event(DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent::new(
                    format!("relationship:{}:{}", parent, child),
                    parent,
                    child,
                    "has_child",
                    serde_json::json!({ "order": order }),
                ),
            });
        }
        Ok(created.len())
    }

    /// Reorder a child within its parent's children list.
    ///
    /// Updates the `has_child` edge `order` field to reposition a node among its siblings.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to reorder
    /// * `position` - Where to place the node among its siblings
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::{NodeService, InsertPosition};
    /// # use nodespace_core::db::SqliteStore;
    /// # use std::path::PathBuf;
    /// # use std::sync::Arc;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let mut db = Arc::new(SqliteStore::new(PathBuf::from("./test.db")).await?);
    /// # let service = NodeService::new(&mut db).await?;
    /// // Position node after sibling
    /// service.reorder_child("node-id", InsertPosition::After("sibling-id")).await?;
    ///
    /// // Move to first position
    /// service.reorder_child("node-id", InsertPosition::Beginning).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reorder_child(
        &self,
        node_id: &str,
        position: crate::services::InsertPosition<'_>,
    ) -> Result<(), NodeServiceError> {
        let (parent_id, actual_order) = self.reorder_child_write(node_id, position).await?;
        self.emit_reorder_event(node_id, parent_id.as_deref(), actual_order);
        Ok(())
    }

    /// The write half of [`Self::reorder_child`], with event emission split
    /// out (ADR-069 §2/S4). [`Self::reorder_node`] calls this directly and
    /// defers the emit until after its own version bump, so a consumer is
    /// never told about a reorder before the write that makes it OCC-safe
    /// has landed — the same ordering fix [`Self::move_node`] has. A
    /// standalone call to `reorder_child` still emits immediately via the
    /// wrapper above, unchanged from its existing public contract.
    async fn reorder_child_write(
        &self,
        node_id: &str,
        position: crate::services::InsertPosition<'_>,
    ) -> Result<(Option<String>, f64), NodeServiceError> {
        // Verify node exists
        let _node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Verify sibling exists for After variant
        if let crate::services::InsertPosition::After(sibling_id) = position {
            let sibling_exists = self.node_exists(sibling_id).await?;
            if !sibling_exists {
                return Err(NodeServiceError::hierarchy_violation(format!(
                    "Sibling node {} does not exist",
                    sibling_id
                )));
            }
        }

        // Get current parent to move within the same parent
        let parent = self.get_parent(node_id).await?;
        let parent_id = parent.map(|p| p.id);

        let insert_after = self
            .resolve_insert_position(position, parent_id.as_deref())
            .await?;

        // Use move_node to handle edge ordering
        let actual_order = self
            .store
            .move_node(node_id, parent_id.as_deref(), insert_after.as_deref())
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        Ok((parent_id, actual_order))
    }

    /// Emit the `RelationshipUpdated` event for a completed reorder.
    /// Reordering updates the hierarchy edge's order field.
    fn emit_reorder_event(&self, node_id: &str, parent_id: Option<&str>, actual_order: f64) {
        if let Some(parent_id) = parent_id {
            self.emit_event(DomainEvent::RelationshipUpdated {
                relationship: crate::db::events::RelationshipEvent::new(
                    format!("relationship:{}:{}", parent_id, node_id),
                    parent_id,
                    node_id,
                    "has_child",
                    serde_json::json!({"order": actual_order}),
                ),
            });
        }
    }

    /// Check if potential_descendant is a descendant of node_id
    /// This prevents circular references when moving nodes
    async fn is_descendant(
        &self,
        node_id: &str,
        potential_descendant: &str,
    ) -> Result<bool, NodeServiceError> {
        // Walk up from potential_descendant to see if we reach node_id
        let mut current_id = potential_descendant.to_string();

        for _ in 0..1000 {
            // Prevent infinite loops
            if current_id == node_id {
                return Ok(true); // Found node_id, so potential_descendant IS a descendant
            }

            // Walk up via parent relationship
            if let Ok(Some(parent)) = self.get_parent(&current_id).await {
                current_id = parent.id;
            } else {
                break; // Reached root or node not found
            }
        }

        Ok(false)
    }
}

/// Flatten a node's child subtree into ordered content sections: a depth-first
/// pre-order walk of `adjacency_list` (children already in fractional order, then
/// their descendants), collecting each node's non-empty `content`. The root itself
/// is excluded. `node_map`/`adjacency_list` come from `get_subtree_data`.
pub fn flatten_subtree_content(
    root_id: &str,
    node_map: &std::collections::HashMap<String, crate::models::Node>,
    adjacency_list: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut parts = Vec::new();
    let mut stack: Vec<String> = adjacency_list
        .get(root_id)
        .map(|c| c.iter().rev().cloned().collect())
        .unwrap_or_default();
    while let Some(id) = stack.pop() {
        match node_map.get(&id) {
            Some(node) => {
                if !node.content.is_empty() {
                    parts.push(node.content.clone());
                }
            }
            None => tracing::warn!(
                node_id = %id,
                "flatten_subtree_content: id in adjacency_list missing from node_map"
            ),
        }
        if let Some(children) = adjacency_list.get(&id) {
            for c in children.iter().rev() {
                stack.push(c.clone());
            }
        }
    }
    parts
}

#[cfg(test)]
mod tree_size_limit_tests {
    use super::*;
    use crate::db::SqliteStore;
    use anyhow::{Context, Result};
    use chrono::Utc;
    use tempfile::TempDir;

    /// Creates 30k+ nodes; slow. Skipped unless explicitly opted in, matching
    /// the convention in `db::sqlite_store::nodes::large_subtree_chunking_tests`.
    /// Run explicitly with:
    /// `RUN_LONG_TESTS=1 cargo test --lib -p nodespace-core tree_size_limit_tests -- --nocapture`.
    macro_rules! require_long_tests {
        () => {
            if std::env::var("RUN_LONG_TESTS").is_err() {
                eprintln!(
                    "skipping {}: set RUN_LONG_TESTS=1 to run (creates 20k+ nodes; slow)",
                    module_path!()
                );
                return Ok(());
            }
        };
    }

    async fn create_test_service() -> Result<(NodeService, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let mut store = Arc::new(SqliteStore::new(db_path).await?);
        let service = NodeService::new(&mut store).await?;
        Ok((service, temp_dir))
    }

    /// Directly bulk-inserts `count` flat `has_child` descendants of `root_id`,
    /// bypassing the service's node-by-node create path — too slow at this
    /// scale for a test. Mirrors
    /// `db::sqlite_store::nodes::large_subtree_chunking_tests::seed_flat_subtree`.
    async fn seed_flat_children(store: &SqliteStore, root_id: &str, count: usize) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let child_ids: Vec<String> = (0..count)
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        const SEED_CHUNK: usize = 5_000;

        for chunk in child_ids.chunks(SEED_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len())
                .map(|i| format!("(?{i}, 'text', '', '{{}}', NULL, 'active', 1, '{now}', '{now}')"))
                .collect();
            let sql = format!(
                "INSERT INTO node (id, node_type, content, properties, title, lifecycle_status, version, created_at, modified_at) VALUES {}",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            store
                .write()
                .await
                .execute(&sql, params)
                .await
                .context("Failed to seed child node chunk")?;
        }

        for chunk in child_ids.chunks(SEED_CHUNK) {
            let placeholders: Vec<String> = (1..=chunk.len())
                .map(|i| {
                    format!(
                        "('{root_id}', ?{i}, 'has_child', 'child_of', '{{}}', 1, '{now}', '{now}')"
                    )
                })
                .collect();
            let sql = format!(
                "INSERT INTO relationship (in_node, out_node, relationship_type, reverse_relationship_type, properties, version, created_at, modified_at) VALUES {}",
                placeholders.join(", ")
            );
            let params: Vec<libsql::Value> = chunk
                .iter()
                .map(|id| libsql::Value::Text(id.clone()))
                .collect();
            store
                .write()
                .await
                .execute(&sql, params)
                .await
                .context("Failed to seed relationship chunk")?;
        }

        Ok(())
    }

    #[tokio::test]
    async fn get_children_tree_rejects_subtree_over_the_node_limit() -> Result<()> {
        require_long_tests!();
        let (service, _tmp) = create_test_service().await?;
        let root_id = service
            .create_node(crate::models::Node::new(
                "text".to_string(),
                "root".to_string(),
                serde_json::json!({}),
            ))
            .await?;
        seed_flat_children(&service.store, &root_id, MAX_TREE_NODES + 1).await?;

        let err = service
            .get_children_tree(&root_id)
            .await
            .expect_err("a subtree over MAX_TREE_NODES must be refused, not serialized");

        assert!(
            matches!(err, NodeServiceError::TreeTooLarge { .. }),
            "expected TreeTooLarge, got: {err:?}"
        );
        assert!(
            err.to_string().contains(&MAX_TREE_NODES.to_string()),
            "error should name the configured limit: {err}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn get_children_tree_accepts_subtree_at_exactly_the_node_limit() -> Result<()> {
        require_long_tests!();
        let (service, _tmp) = create_test_service().await?;
        let root_id = service
            .create_node(crate::models::Node::new(
                "text".to_string(),
                "root".to_string(),
                serde_json::json!({}),
            ))
            .await?;
        // Root counts toward the total, so seed MAX_TREE_NODES - 1 children.
        seed_flat_children(&service.store, &root_id, MAX_TREE_NODES - 1).await?;

        let tree = service
            .get_children_tree(&root_id)
            .await
            .expect("a subtree at exactly MAX_TREE_NODES must be served, not refused");
        assert_eq!(
            tree["children"].as_array().map(|c| c.len()),
            Some(MAX_TREE_NODES - 1)
        );
        Ok(())
    }
}
