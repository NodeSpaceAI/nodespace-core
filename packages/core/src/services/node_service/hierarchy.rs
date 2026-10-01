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
            .map_err(NodeServiceError::from_store)
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
            .map_err(NodeServiceError::from_store)
    }

    /// Count root nodes without listing them — the O(1)-response-size
    /// counterpart to `get_roots`, for callers (e.g. `nodespace diagnostics`)
    /// that only need a total.
    pub async fn count_roots(&self) -> Result<i64, NodeServiceError> {
        self.store
            .count_roots()
            .await
            .map_err(NodeServiceError::from_store)
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
            .map_err(NodeServiceError::from_store)?;

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

        // Collapse each node's `extends` chain into its own bucket before the
        // tree is flattened for the wire, exactly as the single-node read path
        // does — otherwise inherited fields would be missing from tree nodes.
        let node_map: HashMap<String, Node> = self
            .collapse_chain_for_wire(node_map.into_values().collect())
            .await?
            .into_iter()
            .map(|n| (n.id.clone(), n))
            .collect();
        let root_node = root_node.and_then(|root| node_map.get(&root.id).cloned());

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
            .map_err(NodeServiceError::from_store)?;

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
            .map_err(NodeServiceError::from_store)
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
                .map_err(NodeServiceError::from_store)?;

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

    /// Hierarchy rules every move must satisfy, shared by `move_node` and
    /// `move_node_unchecked` so the two variants differ only in OCC:
    /// date containers never move, the new parent must exist and be a
    /// container type, and the move must not create a cycle.
    async fn validate_move(
        &self,
        node: &Node,
        new_parent: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Date nodes are top-level containers and cannot be moved
        if self
            .type_is_a(&node.node_type, crate::models::CoreNodeType::Date)
            .await?
        {
            return Err(NodeServiceError::hierarchy_violation(format!(
                "Date node '{}' cannot be moved (it's a top-level container)",
                node.id
            )));
        }

        let Some(parent_id) = new_parent else {
            return Ok(());
        };

        let parent_node = self
            .get_node(parent_id)
            .await?
            .ok_or_else(|| NodeServiceError::invalid_parent(parent_id))?;

        // Enforce container rule: reject moves into non-container node types
        if !self
            .behavior_for(&parent_node.node_type)
            .await?
            .can_have_children()
        {
            return Err(NodeServiceError::not_a_container(
                parent_id,
                &parent_node.node_type,
            ));
        }

        // Check for circular reference - parent_id cannot be a descendant of node_id
        if self.is_descendant(&node.id, parent_id).await? {
            return Err(TreeInvariantViolation::cycle(
                &node.id,
                parent_id,
                format!(
                    "Cannot move node {} under its descendant {}",
                    node.id, parent_id
                ),
            )
            .into());
        }

        Ok(())
    }

    /// Move a node to a new parent without version checking (no OCC).
    ///
    /// **Prefer `move_node()`** which enforces optimistic concurrency control.
    /// This unchecked variant enforces the same hierarchy rules but skips the
    /// version check, for callers that don't hold the node's version.
    ///
    /// Replaces the node's `has_child` parent edge (or removes it, making the
    /// node a root) and positions it among its new siblings.
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
    /// - New parent doesn't exist or is not a container type
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
        self.move_existing_node(node_id, None, new_parent, position)
            .await
            .map(|_| ())
    }

    /// Move a node to a new parent with OCC (Optimistic Concurrency Control)
    ///
    /// This method validates version before moving, preventing concurrent modifications
    /// from silently overwriting each other. The node's version is bumped after a
    /// successful move.
    ///
    /// Returns the updated node and, when the node landed under a parent, the
    /// store's [`crate::db::ChildPlacement`] for the written edge. The caller
    /// that made the write needs it: echo suppression keeps this write's own
    /// `RelationshipUpdated` events from reaching it, so the reply is the only
    /// place it learns the authoritative order keys (including any re-spread).
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
    /// - New parent doesn't exist or is not a container type
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
    ) -> Result<(Node, Option<crate::db::ChildPlacement>), NodeServiceError> {
        let (updated, placement) = self
            .move_existing_node(node_id, Some(expected_version), new_parent, position)
            .await?;
        Ok((
            updated.expect("a versioned move returns the node it bumped"),
            placement,
        ))
    }

    /// The body of [`Self::move_node`] and [`Self::move_node_unchecked`]:
    /// with `expected_version`, the move is version-checked and bumps the
    /// node, returning it; without, it neither checks nor bumps. Also returns
    /// the store's placement of the written edge when the node landed under a
    /// parent (see [`Self::move_node`]).
    ///
    /// The checks before the transaction read through pooled readers and
    /// only fail fast with a readable error. The guarantees come from the
    /// transaction (ADR-069 §1a): the store re-checks existence, cycles and
    /// membership against the state it writes, and the bump re-checks
    /// `expected_version`, so a concurrent writer that bumped the node after
    /// the pre-check rolls the edge and every re-spread key back. Events are
    /// buffered and flushed only after commit (ADR-069 §2), so a failed move
    /// announces nothing and a committed one announces all of it.
    async fn move_existing_node(
        &self,
        node_id: &str,
        expected_version: Option<i64>,
        new_parent: Option<&str>,
        position: crate::services::InsertPosition<'_>,
    ) -> Result<(Option<Node>, Option<crate::db::ChildPlacement>), NodeServiceError> {
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        if let Some(expected_version) = expected_version {
            if node.version != expected_version {
                return Err(NodeServiceError::version_conflict(
                    node_id,
                    expected_version,
                    node.version,
                ));
            }
        }

        self.validate_move(&node, new_parent).await?;

        let insert_after = self.resolve_insert_position(position, new_parent).await?;

        let node_id = node_id.to_string();
        let new_parent = new_parent.map(str::to_string);
        let service = self.clone();
        self.with_transaction(move |tx| {
            Box::pin(async move {
                let moved = service
                    .move_in_tx(tx, &node_id, new_parent.as_deref(), insert_after.as_deref())
                    .await?;

                // Even though we're only modifying edge relationships, we bump
                // the node version so that concurrent move operations fail with
                // a version conflict. Bumped after the rootness refresh, so the
                // returned node — and its `NodeUpdated` — carry the new title.
                let updated_node = match expected_version {
                    Some(expected_version) => Some(
                        service
                            .update_node_with_version_bump_in_tx(tx, &node_id, expected_version)
                            .await?,
                    ),
                    None => None,
                };

                service.emit_move_events(tx, &node_id, new_parent.as_deref(), &moved);
                Ok((updated_node, new_parent.map(|_| moved.placement)))
            })
        })
        .await
    }

    /// Move `node_id` under `new_parent` (a root when `None`) inside `tx`,
    /// then refresh its title and embeddings for any change of rootness. The
    /// former parent is read inside the transaction with the edge write, so
    /// it is the parent the write actually replaced.
    async fn move_in_tx(
        &self,
        tx: &NodeServiceTx<'_>,
        node_id: &str,
        new_parent: Option<&str>,
        insert_after: Option<&str>,
    ) -> Result<crate::db::NodeMove, NodeServiceError> {
        let moved = crate::db::SqliteStore::move_node_in_tx(
            tx.store_tx(),
            node_id,
            new_parent,
            insert_after,
        )
        .await
        .map_err(NodeServiceError::from_store)?;
        self.refresh_for_rootness_in_tx(
            tx,
            node_id,
            new_parent.is_none(),
            moved
                .former_parent
                .as_deref()
                .filter(|p| Some(*p) != new_parent),
        )
        .await?;
        Ok(moved)
    }

    /// Emit the relationship events for a committed move: the re-spread
    /// siblings and the NEW-parent edge first, so a consumer that
    /// inserts-then-deletes never sees the node parentless mid-move, then the
    /// removal of the OLD parent edge when the parent actually changed
    /// (move-to-root or reparent), so the detach propagates to other devices
    /// rather than leaving a stale edge there.
    fn emit_move_events(
        &self,
        tx: &NodeServiceTx<'_>,
        node_id: &str,
        new_parent: Option<&str>,
        moved: &crate::db::NodeMove,
    ) {
        if let Some(parent_id) = new_parent {
            self.emit_respread_events(tx, parent_id, &moved.placement.respread);
            self.emit_event_in_tx(
                tx,
                DomainEvent::RelationshipUpdated {
                    relationship: crate::db::events::RelationshipEvent::new(
                        format!("relationship:{}:{}", parent_id, node_id),
                        parent_id,
                        node_id,
                        "has_child",
                        serde_json::json!({"order": moved.placement.order}),
                    ),
                },
            );
        }
        self.emit_former_parent_deleted(tx, node_id, new_parent, moved);
    }

    /// Emit `RelationshipDeleted` for the `has_child` edge a committed move
    /// replaced, when the parent actually changed. A same-parent reorder and a
    /// first-time attach replaced no edge and emit nothing.
    fn emit_former_parent_deleted(
        &self,
        tx: &NodeServiceTx<'_>,
        node_id: &str,
        new_parent: Option<&str>,
        moved: &crate::db::NodeMove,
    ) {
        if let Some(old_id) = moved.former_parent.as_deref() {
            if new_parent != Some(old_id) {
                self.emit_event_in_tx(
                    tx,
                    DomainEvent::RelationshipDeleted {
                        id: format!("relationship:{}:{}", old_id, node_id),
                        from_id: crate::db::events::node_thing(old_id),
                        to_id: crate::db::events::node_thing(node_id),
                        relationship_type: "has_child".to_string(),
                    },
                );
            }
        }
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

        let insert_after = self.resolve_reorder_position(node_id, position).await?;

        // The reorder, any sibling re-spread and the version bump are one
        // unit of work (ADR-069 §1a). The checks above read through pooled
        // readers and only fail fast; a concurrent writer can still change
        // the node before the write. The bump runs first inside the
        // transaction and re-checks `expected_version`, so it alone decides
        // who lost a race: any concurrent versioned write — an edit, a move,
        // a move to root — fails this reorder with `VersionConflict` before
        // it writes, rather than with whatever the reorder would then trip
        // over. Nothing else in the reorder reads the node's own row, so the
        // bump's `NodeUpdated` is unaffected by running first. Events are
        // buffered and flushed only after commit (ADR-069 §2).
        let node_id = node_id.to_string();
        let service = self.clone();
        self.with_transaction(move |tx| {
            Box::pin(async move {
                // Even though we're only modifying edge ordering, we bump the
                // node version so that concurrent reorders fail with a
                // version conflict.
                service
                    .update_node_with_version_bump_in_tx(tx, &node_id, expected_version)
                    .await?;

                let Some((parent_id, placement)) =
                    Self::reorder_child_in_tx(tx, &node_id, insert_after.as_deref()).await?
                else {
                    return Err(root_reorder_violation(&node_id));
                };

                service.emit_reorder_event(tx, &node_id, &parent_id, &placement);
                Ok(())
            })
        })
        .await
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
    ///
    /// Returns each child with its bumped version and the order key the store
    /// gave its new edge. The caller that made the write needs those keys: echo
    /// suppression keeps this write's own `RelationshipUpdated` events from
    /// reaching it.
    pub async fn move_children_to_parent(
        &self,
        new_parent_id: &str,
        children: &[(String, i64)],
    ) -> Result<Vec<(Node, f64)>, NodeServiceError> {
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
            .await?
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
            if self
                .type_is_a(&node.node_type, crate::models::CoreNodeType::Date)
                .await?
            {
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
                return Err(TreeInvariantViolation::cycle(
                    node_id,
                    new_parent_id,
                    format!(
                        "Cannot move node {} under its descendant {}",
                        node_id, new_parent_id
                    ),
                )
                .into());
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
        let updated: Vec<(Node, f64)> = self
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
                    .map_err(NodeServiceError::from_store)??;

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

                        service.emit_event_in_tx(
                            tx,
                            crate::db::events::DomainEvent::RelationshipUpdated {
                                relationship: crate::db::events::RelationshipEvent::new(
                                    format!("relationship:{}:{}", new_parent_id, node.id),
                                    &new_parent_id,
                                    &node.id,
                                    "has_child",
                                    serde_json::json!({"order": order}),
                                ),
                            },
                        );

                        updated.push((updated_node, *order));
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
    /// Replaces any existing parent, so this can reparent; a reparent emits
    /// `RelationshipDeleted` for the replaced edge after `RelationshipCreated`
    /// for the new one, the same event shape `move_node` produces.
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
        let existing_parent = self
            .store
            .get_parent_id(child_id)
            .await
            .map_err(NodeServiceError::from_store)?;
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
        // store's move gets a concrete Option<&str>.
        let insert_after = self
            .resolve_insert_position(position, Some(parent_id))
            .await?;

        // The edge write — which replaces any existing parent, so this can
        // reparent — its sibling re-spread and the rootness refresh commit as
        // one transaction (ADR-069 §1a). The parent the refresh treats as
        // left is read inside it, with the write. Events are buffered and
        // flushed only after commit (ADR-069 §2).
        let child_id = child_id.to_string();
        let parent_id = parent_id.to_string();
        let service = self.clone();
        self.with_transaction(move |tx| {
            Box::pin(async move {
                let moved = service
                    .move_in_tx(tx, &child_id, Some(&parent_id), insert_after.as_deref())
                    .await?;

                service.emit_respread_events(tx, &parent_id, &moved.placement.respread);
                service.emit_event_in_tx(
                    tx,
                    DomainEvent::RelationshipCreated {
                        relationship: crate::db::events::RelationshipEvent::new(
                            format!("relationship:{}:{}", parent_id, child_id),
                            &parent_id,
                            &child_id,
                            "has_child",
                            serde_json::json!({"order": moved.placement.order}),
                        ),
                    },
                );
                // A reparent also announces the edge it replaced, after the new
                // one, exactly as `move_node` does.
                service.emit_former_parent_deleted(tx, &child_id, Some(&parent_id), &moved);
                Ok(())
            })
        })
        .await?;

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
    ) -> Result<crate::db::ChildPlacement, NodeServiceError> {
        let resolved = self
            .resolve_insert_position(position, Some(parent_id))
            .await?;
        let insert_after_id: Option<&str> = resolved.as_deref();

        let placement = crate::db::SqliteStore::create_has_child_edge_in_tx(
            tx.store_tx(),
            parent_id,
            child_id,
            insert_after_id,
        )
        .await
        .map_err(NodeServiceError::from_store)?;

        self.emit_respread_events(tx, parent_id, &placement.respread);
        self.emit_event_in_tx(
            tx,
            DomainEvent::RelationshipCreated {
                relationship: crate::db::events::RelationshipEvent::new(
                    format!("relationship:{}:{}", parent_id, child_id),
                    parent_id,
                    child_id,
                    "has_child",
                    serde_json::json!({"order": placement.order}),
                ),
            },
        );

        Ok(placement)
    }

    /// Batched sibling of [`Self::create_parent_edge`] for attaching many
    /// genuinely-unparented children in one store transaction — for example when
    /// applying a batch of hierarchy edges written elsewhere. It attaches each child
    /// under its parent, then emits one `RelationshipCreated` per created edge —
    /// exactly as the per-row `create_parent_edge` does. (Relationship events are not
    /// node-keyed, so `begin_batch_emit` does NOT coalesce them; they broadcast
    /// immediately, one per edge, matching the per-row path — the batching win is the
    /// single DB transaction, not the events.) `edges` is `(parent, child, order)`
    /// carrying the sender's sibling order; a child that already has a parent is
    /// skipped in the store (see `bulk_create_has_child`), so this only attaches
    /// genuinely-unparented children. Returns the number of edges created.
    ///
    /// Unlike `create_parent_edge` this does NOT reposition — it is intended for a
    /// from-scratch batch where every parent is fresh, so the sender's `order`
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
            .map_err(NodeServiceError::from_store)?;
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

    /// [`Self::reorder_node`]'s fast-fail checks and position resolution, run
    /// before its transaction: the node must not be a root and any `After`
    /// sibling must exist. Resolves the position against the node's current
    /// parent and returns the sibling to insert after (`None` = first).
    async fn resolve_reorder_position(
        &self,
        node_id: &str,
        position: crate::services::InsertPosition<'_>,
    ) -> Result<Option<String>, NodeServiceError> {
        let Some(parent_id) = self
            .store
            .get_parent_id(node_id)
            .await
            .map_err(NodeServiceError::from_store)?
        else {
            return Err(root_reorder_violation(node_id));
        };

        if let crate::services::InsertPosition::After(sibling_id) = position {
            if !self.node_exists(sibling_id).await? {
                return Err(NodeServiceError::hierarchy_violation(format!(
                    "Sibling node {} does not exist",
                    sibling_id
                )));
            }
        }

        self.resolve_insert_position(position, Some(&parent_id))
            .await
    }

    /// Reposition `node_id` among its siblings inside `tx`, after
    /// `insert_after` (`None` = first). Returns the parent and the placement,
    /// or `None` without writing if the node is a root.
    ///
    /// The parent is read inside the transaction, so the write is always a
    /// same-parent reorder, never a move back to the parent `insert_after`
    /// was resolved against. If a concurrent move reparented the node after
    /// that resolution, an `insert_after` sibling is no longer among its
    /// children and the store places the node last under its new parent; a
    /// `None` (first) position still places it first.
    pub(crate) async fn reorder_child_in_tx(
        tx: &NodeServiceTx<'_>,
        node_id: &str,
        insert_after: Option<&str>,
    ) -> Result<Option<(String, crate::db::ChildPlacement)>, NodeServiceError> {
        let Some(parent_id) = crate::db::SqliteStore::get_parent_id_in_tx(tx.store_tx(), node_id)
            .await
            .map_err(NodeServiceError::from_store)?
        else {
            return Ok(None);
        };
        let moved = crate::db::SqliteStore::move_node_in_tx(
            tx.store_tx(),
            node_id,
            Some(&parent_id),
            insert_after,
        )
        .await
        .map_err(NodeServiceError::from_store)?;
        Ok(Some((parent_id, moved.placement)))
    }

    /// Emit the `RelationshipUpdated` events for a completed reorder: the
    /// re-spread siblings first, then the reordered edge.
    fn emit_reorder_event(
        &self,
        tx: &NodeServiceTx<'_>,
        node_id: &str,
        parent_id: &str,
        placement: &crate::db::ChildPlacement,
    ) {
        self.emit_respread_events(tx, parent_id, &placement.respread);
        self.emit_event_in_tx(
            tx,
            DomainEvent::RelationshipUpdated {
                relationship: crate::db::events::RelationshipEvent::new(
                    format!("relationship:{}:{}", parent_id, node_id),
                    parent_id,
                    node_id,
                    "has_child",
                    serde_json::json!({"order": placement.order}),
                ),
            },
        );
    }

    /// Emit a `RelationshipUpdated` for each sibling whose order key a
    /// re-spread rewrote (see [`crate::db::ChildPlacement::respread`]).
    ///
    /// Callers emit these BEFORE the written edge's own event, so a client
    /// applying events in order has every sibling on the re-spread keys by
    /// the time the new key arrives.
    fn emit_respread_events(
        &self,
        tx: &NodeServiceTx<'_>,
        parent_id: &str,
        respread: &[(String, f64)],
    ) {
        for (child_id, order) in respread {
            self.emit_event_in_tx(
                tx,
                DomainEvent::RelationshipUpdated {
                    relationship: crate::db::events::RelationshipEvent::new(
                        format!("relationship:{}:{}", parent_id, child_id),
                        parent_id,
                        child_id,
                        "has_child",
                        serde_json::json!({"order": order}),
                    ),
                },
            );
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

/// Render a node's child subtree as markdown: a depth-first pre-order walk of
/// `adjacency_list` (children already in fractional order, then their
/// descendants), emitting each node's non-empty `content`. The root itself is
/// excluded. `node_map`/`adjacency_list` come from `get_subtree_data`.
///
/// Import stores a `- ` bullet's text without its marker; bullet-ness is
/// carried by structure instead: `prepare_nodes_from_markdown` hangs a bullet
/// off the paragraph that introduces it, or off the bullet it is indented
/// under. This re-derives the markers from that outline shape: a `text` node
/// whose parent is also a `text` node is a list item, rendered with a `- `
/// marker and indented two spaces per level of nesting under another list
/// item. Consecutive list items are joined by a single newline (a tight
/// list); every other boundary is a blank line. Other node types
/// (code-block, ordered-list, …) carry their markdown syntax inside `content`
/// and are emitted verbatim. An empty node is skipped and its children take
/// its place in the outline.
///
/// Known limits — structure cannot say everything the markers did. A list
/// with no introducing paragraph (directly under a heading, at the start of
/// a body, or right after a code block, table or ordered list) is stored as
/// a heading's text children, the same shape as paragraphs, so it renders as
/// paragraphs. And an indented continuation paragraph of a list item is
/// stored as that item's text child, so it renders as a nested item.
///
/// `crate::markdown::handle_get_markdown_from_node_id` (markdown export for
/// editing) decides bullets by its own, different rule — this one is the
/// exact inverse of how `prepare_nodes_from_markdown` attaches bullets, which
/// is what prompt rendering of imported guidance needs.
pub fn render_subtree_markdown(
    root_id: &str,
    node_map: &std::collections::HashMap<String, crate::models::Node>,
    adjacency_list: &std::collections::HashMap<String, Vec<String>>,
) -> String {
    /// A node still to visit, with the parent facts that decide whether it
    /// is a list item: a `text` parent makes a `text` node one, and the
    /// parent's own list level (when it is itself a list item) sets nesting.
    struct Pending {
        id: String,
        parent_is_text: bool,
        parent_list_level: Option<usize>,
    }

    fn push_children(
        stack: &mut Vec<Pending>,
        children: Option<&Vec<String>>,
        parent_is_text: bool,
        parent_list_level: Option<usize>,
    ) {
        for id in children.into_iter().flatten().rev() {
            stack.push(Pending {
                id: id.clone(),
                parent_is_text,
                parent_list_level,
            });
        }
    }

    let mut out = String::new();
    let mut prev_was_list_item = false;
    let mut stack = Vec::new();
    push_children(&mut stack, adjacency_list.get(root_id), false, None);
    while let Some(pending) = stack.pop() {
        let children = adjacency_list.get(&pending.id);
        let Some(node) = node_map.get(&pending.id) else {
            tracing::warn!(
                node_id = %pending.id,
                "render_subtree_markdown: id in adjacency_list missing from node_map"
            );
            push_children(&mut stack, children, false, None);
            continue;
        };
        if node.content.is_empty() {
            push_children(
                &mut stack,
                children,
                pending.parent_is_text,
                pending.parent_list_level,
            );
            continue;
        }
        let is_text = crate::models::CoreNodeType::Text.is_exactly(&node.node_type);
        let list_level = (is_text && pending.parent_is_text)
            .then(|| pending.parent_list_level.map_or(0, |level| level + 1));
        if !out.is_empty() {
            let tight = prev_was_list_item && list_level.is_some();
            out.push_str(if tight { "\n" } else { "\n\n" });
        }
        if let Some(level) = list_level {
            out.push_str(&"  ".repeat(level));
            out.push_str("- ");
        }
        out.push_str(&node.content);
        prev_was_list_item = list_level.is_some();
        push_children(&mut stack, children, is_text, list_level);
    }
    out
}

/// Roots have no parent edge to reposition under, so they cannot be reordered.
fn root_reorder_violation(node_id: &str) -> NodeServiceError {
    NodeServiceError::hierarchy_violation(format!(
        "Root node '{}' cannot be reordered (it has no parent)",
        node_id
    ))
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
    async fn get_children_tree_serves_typed_nodes_like_single_node_reads() -> Result<()> {
        // A node page populates the frontend store from this tree, so its nodes
        // must carry the same flattened `properties` a single-node read returns —
        // not storage's `{ "person": { ... } }` bucket.
        let (service, _tmp) = create_test_service().await?;
        let root_id = service
            .create_node(crate::models::Node::new(
                "text".to_string(),
                "root".to_string(),
                serde_json::json!({}),
            ))
            .await?;
        service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "person".to_string(),
                content: String::new(),
                parent_id: Some(root_id.clone()),
                position: crate::services::InsertPositionOwned::End,
                properties: serde_json::json!({ "first_name": "Ada", "last_name": "Lovelace" }),
                lifecycle_status: None,
            })
            .await?;
        service
            .create_node_with_parent(CreateNodeParams {
                id: None,
                node_type: "task".to_string(),
                content: "ship it".to_string(),
                parent_id: Some(root_id.clone()),
                position: crate::services::InsertPositionOwned::End,
                properties: serde_json::json!({ "status": "in_progress" }),
                lifecycle_status: None,
            })
            .await?;

        let tree = service.get_children_tree(&root_id).await?;
        let person = &tree["children"][0];
        // Typed fields are promoted to the top level, as on a single-node read.
        assert_eq!(tree["children"][1]["status"], "in_progress");

        assert_eq!(person["nodeType"], "person");
        assert_eq!(person["firstName"], "Ada");
        assert_eq!(person["lastName"], "Lovelace");
        assert_eq!(
            person["properties"],
            serde_json::json!({}),
            "tree nodes carry neither the storage bucket nor a copy of the typed fields"
        );
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
