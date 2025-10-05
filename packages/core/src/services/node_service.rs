//! Node Service - Core CRUD Operations
//!
//! This module provides the main business logic layer for node operations:
//!
//! - CRUD operations (create, read, update, delete)
//! - Hierarchy management (get_children, move_node, reorder_siblings)
//! - Bulk operations with transactions
//! - Query operations with filtering
//!
//! # Scope
//!
//! Initial implementation supports Text, Task, and Date nodes for E2E testing.
//! Person and Project node support will be added in separate issues.

use crate::behaviors::NodeBehaviorRegistry;
use crate::db::DatabaseService;
use crate::models::{Node, NodeFilter, NodeUpdate, OrderBy};
use crate::services::error::NodeServiceError;
use chrono::{DateTime, NaiveDateTime, Utc};
use std::sync::Arc;

/// Parse timestamp from database - handles both SQLite and RFC3339 formats
fn parse_timestamp(s: &str) -> Result<DateTime<Utc>, String> {
    // Try SQLite format first: "YYYY-MM-DD HH:MM:SS"
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(naive.and_utc());
    }

    // Try RFC3339 format (for old data): "YYYY-MM-DDTHH:MM:SSZ"
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    Err(format!(
        "Unable to parse timestamp '{}' as SQLite or RFC3339 format",
        s
    ))
}

/// Core service for node CRUD and hierarchy operations
///
/// # Examples
///
/// ```no_run
/// use nodespace_core::services::NodeService;
/// use nodespace_core::db::DatabaseService;
/// use nodespace_core::models::Node;
/// use std::path::PathBuf;
/// use serde_json::json;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let db = DatabaseService::new(PathBuf::from("./data/test.db")).await?;
///     let service = NodeService::new(db).await?;
///
///     let node = Node::new(
///         "text".to_string(),
///         "Hello World".to_string(),
///         None,
///         json!({}),
///     );
///
///     let id = service.create_node(node).await?;
///     println!("Created node: {}", id);
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct NodeService {
    /// Database service for persistence
    db: Arc<DatabaseService>,

    /// Behavior registry for validation
    behaviors: Arc<NodeBehaviorRegistry>,
}

impl NodeService {
    /// Create a new NodeService
    ///
    /// Initializes the service with a DatabaseService and creates a default
    /// NodeBehaviorRegistry with Text, Task, and Date behaviors.
    ///
    /// # Arguments
    ///
    /// * `db` - DatabaseService instance
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = DatabaseService::new(PathBuf::from("./data/test.db")).await?;
    /// let service = NodeService::new(db).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(db: DatabaseService) -> Result<Self, NodeServiceError> {
        Ok(Self {
            db: Arc::new(db),
            behaviors: Arc::new(NodeBehaviorRegistry::new()),
        })
    }

    /// Create a new node
    ///
    /// Validates the node using the appropriate behavior (Text, Task, or Date),
    /// then inserts it into the database.
    ///
    /// # Arguments
    ///
    /// * `node` - The node to create
    ///
    /// # Returns
    ///
    /// The ID of the created node
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node validation fails
    /// - Parent node doesn't exist (if parent_id is set)
    /// - Root node doesn't exist (if origin_node_id is set)
    /// - Database insertion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::Node;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// let node = Node::new(
    ///     "text".to_string(),
    ///     "My note".to_string(),
    ///     None,
    ///     json!({}),
    /// );
    /// let id = service.create_node(node).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_node(&self, node: Node) -> Result<String, NodeServiceError> {
        // Validate node using behavior registry
        self.behaviors.validate_node(&node)?;

        // Validate parent exists if parent_id is set
        if let Some(ref parent_id) = node.parent_id {
            let parent_exists = self.node_exists(parent_id).await?;
            if !parent_exists {
                return Err(NodeServiceError::invalid_parent(parent_id));
            }
        }

        // Validate root exists if origin_node_id is set
        if let Some(ref origin_node_id) = node.origin_node_id {
            let root_exists = self.node_exists(origin_node_id).await?;
            if !root_exists {
                return Err(NodeServiceError::invalid_root(origin_node_id));
            }
        }

        // Insert into database
        // Database defaults handle created_at and modified_at timestamps automatically
        let conn = self.db.connect()?;

        let properties_json = serde_json::to_string(&node.properties)
            .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

        conn.execute(
            "INSERT INTO nodes (id, node_type, content, parent_id, origin_node_id, before_sibling_id, properties, embedding_vector)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (
                node.id.as_str(),
                node.node_type.as_str(),
                node.content.as_str(),
                node.parent_id.as_deref(),
                node.origin_node_id.as_deref(),
                node.before_sibling_id.as_deref(),
                properties_json.as_str(),
                node.embedding_vector.as_deref(),
            ),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to insert node: {}", e)))?;

        Ok(node.id)
    }

    /// Get a node by ID
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to fetch
    ///
    /// # Returns
    ///
    /// `Some(Node)` if found, `None` if not found
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// if let Some(node) = service.get_node("node-id-123").await? {
    ///     println!("Found: {}", node.content);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_node(&self, id: &str) -> Result<Option<Node>, NodeServiceError> {
        let conn = self.db.connect()?;

        let mut stmt = conn
            .prepare(
                "SELECT id, node_type, content, parent_id, origin_node_id, before_sibling_id,
                        created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE id = ?",
            )
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

        let mut rows = stmt.query([id]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let node = self.row_to_node(row)?;
            Ok(Some(node))
        } else {
            Ok(None)
        }
    }

    /// Update a node
    ///
    /// Performs a partial update using the NodeUpdate struct. Only provided fields
    /// will be updated. Handles the double-Option pattern for nullable fields.
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to update
    /// * `update` - The fields to update
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Node doesn't exist
    /// - Validation fails after update
    /// - Database update fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// let update = NodeUpdate::new()
    ///     .with_content("Updated content".to_string());
    /// service.update_node("node-id", update).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update_node(&self, id: &str, update: NodeUpdate) -> Result<(), NodeServiceError> {
        if update.is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Update contains no changes",
            ));
        }

        // Get existing node to validate update
        let existing = self
            .get_node(id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(id))?;

        // For simplicity with libsql, we'll fetch the node, apply updates, and replace entirely
        let mut updated = existing.clone();

        if let Some(node_type) = update.node_type {
            updated.node_type = node_type;
        }

        if let Some(content) = update.content {
            updated.content = content;
        }

        if let Some(parent_id) = update.parent_id {
            updated.parent_id = parent_id;
        }

        if let Some(origin_node_id) = update.origin_node_id {
            updated.origin_node_id = origin_node_id;
        }

        if let Some(before_sibling_id) = update.before_sibling_id {
            updated.before_sibling_id = before_sibling_id;
        }

        if let Some(properties) = update.properties {
            updated.properties = properties;
        }

        if let Some(embedding_vector) = update.embedding_vector {
            updated.embedding_vector = embedding_vector;
        }

        // Validate updated node
        self.behaviors.validate_node(&updated)?;

        // Execute update - database will auto-update modified_at via trigger or default
        let conn = self.db.connect()?;
        let properties_json = serde_json::to_string(&updated.properties)
            .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

        conn.execute(
            "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, origin_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ? WHERE id = ?",
            (
                updated.node_type.as_str(),
                updated.content.as_str(),
                updated.parent_id.as_deref(),
                updated.origin_node_id.as_deref(),
                updated.before_sibling_id.as_deref(),
                properties_json.as_str(),
                updated.embedding_vector.as_deref(),
                id,
            ),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to update node: {}", e)))?;

        Ok(())
    }

    /// Delete a node
    ///
    /// Deletes a node and all its children (cascade delete).
    ///
    /// # Arguments
    ///
    /// * `id` - The node ID to delete
    ///
    /// # Errors
    ///
    /// Returns error if node doesn't exist or database deletion fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// service.delete_node("node-id-123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn delete_node(&self, id: &str) -> Result<(), NodeServiceError> {
        let conn = self.db.connect()?;

        let rows_affected = conn
            .execute("DELETE FROM nodes WHERE id = ?", [id])
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to delete node: {}", e)))?;

        if rows_affected == 0 {
            return Err(NodeServiceError::node_not_found(id));
        }

        Ok(())
    }

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// let children = service.get_children("parent-id").await?;
    /// println!("Found {} children", children.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_children(&self, parent_id: &str) -> Result<Vec<Node>, NodeServiceError> {
        let filter = NodeFilter::new()
            .with_parent_id(parent_id.to_string())
            .with_order_by(OrderBy::CreatedAsc);

        let mut children = self.query_nodes(filter).await?;

        // Sort children by sibling linked list (before_sibling_id)
        // This reconstructs the proper order independent of creation time
        self.sort_by_sibling_order(&mut children);

        Ok(children)
    }

    /// Bulk fetch all nodes belonging to an origin node (viewer/page)
    ///
    /// This is the efficient way to load a complete document tree:
    /// 1. Single database query fetches all nodes with the same origin_node_id
    /// 2. In-memory hierarchy reconstruction using parent_id and before_sibling_id
    ///
    /// This avoids making multiple queries for each level of the tree.
    ///
    /// # Arguments
    ///
    /// * `origin_node_id` - The ID of the origin node (e.g., date page ID)
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// // Fetch all nodes for a date page
    /// let nodes = service.get_nodes_by_origin_id("2025-10-05").await?;
    /// println!("Found {} nodes in this document", nodes.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_nodes_by_origin_id(
        &self,
        origin_node_id: &str,
    ) -> Result<Vec<Node>, NodeServiceError> {
        let filter = NodeFilter::new().with_origin_node_id(origin_node_id.to_string());

        self.query_nodes(filter).await
    }

    /// Sort nodes by their sibling linked list order
    ///
    /// Nodes use before_sibling_id to form a linked list:
    /// - before_sibling_id = None means it's the first node
    /// - before_sibling_id = Some(id) means it comes after node with that id
    ///
    /// This function reconstructs the proper order by following the chain.
    fn sort_by_sibling_order(&self, nodes: &mut Vec<Node>) {
        if nodes.is_empty() {
            return;
        }

        // Build a map of id -> node for quick lookup
        let mut node_map: std::collections::HashMap<String, Node> =
            nodes.drain(..).map(|n| (n.id.clone(), n)).collect();

        // Find the first node (before_sibling_id is None)
        let first_node = node_map
            .values()
            .find(|n| n.before_sibling_id.is_none())
            .cloned();

        if let Some(first) = first_node {
            let mut ordered = vec![first.clone()];
            node_map.remove(&first.id);

            // Follow the chain: find nodes that have before_sibling_id = current node's id
            while !node_map.is_empty() {
                let current_id = ordered.last().unwrap().id.clone();

                // Find the next node in the chain
                let next = node_map
                    .values()
                    .find(|n| n.before_sibling_id.as_ref() == Some(&current_id))
                    .cloned();

                if let Some(next_node) = next {
                    let next_id = next_node.id.clone();
                    ordered.push(next_node);
                    node_map.remove(&next_id);
                } else {
                    // Chain broken or multiple chains - append remaining nodes by creation time
                    let mut remaining: Vec<Node> = node_map.values().cloned().collect();
                    remaining.sort_by(|a, b| a.created_at.cmp(&b.created_at));
                    ordered.extend(remaining);
                    break;
                }
            }

            *nodes = ordered;
        } else {
            // No node with before_sibling_id = None found
            // Fall back to creation time ordering
            nodes.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        }
    }

    /// Move a node to a new parent
    ///
    /// Updates the parent_id and origin_node_id of a node, maintaining hierarchy consistency.
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
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// // Move node under new parent
    /// service.move_node("node-id", Some("new-parent-id")).await?;
    ///
    /// // Make node a root
    /// service.move_node("node-id", None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn move_node(
        &self,
        node_id: &str,
        new_parent: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Verify node exists
        let _node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

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

        // Determine new origin_node_id
        let new_origin_node_id = match new_parent {
            Some(parent_id) => {
                // Get parent's origin_node_id, or use parent as root if it's a root node
                let parent = self
                    .get_node(parent_id)
                    .await?
                    .ok_or_else(|| NodeServiceError::invalid_parent(parent_id))?;
                parent.origin_node_id.or(Some(parent_id.to_string()))
            }
            None => None, // Node becomes a root
        };

        let update = NodeUpdate {
            parent_id: Some(new_parent.map(String::from)),
            origin_node_id: Some(new_origin_node_id),
            ..Default::default()
        };

        self.update_node(node_id, update).await
    }

    /// Reorder siblings using before_sibling_id pointer
    ///
    /// Sets the before_sibling_id to position a node in its sibling list.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The node to reorder
    /// * `before_sibling_id` - The sibling to position before (None = end of list)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// // Position node before sibling
    /// service.reorder_siblings("node-id", Some("sibling-id")).await?;
    ///
    /// // Move to end of list
    /// service.reorder_siblings("node-id", None).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reorder_siblings(
        &self,
        node_id: &str,
        before_sibling_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        // Verify node exists
        let _node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(node_id))?;

        // Verify sibling exists if provided
        if let Some(sibling_id) = before_sibling_id {
            let sibling_exists = self.node_exists(sibling_id).await?;
            if !sibling_exists {
                return Err(NodeServiceError::hierarchy_violation(format!(
                    "Sibling node {} does not exist",
                    sibling_id
                )));
            }
        }

        let update = NodeUpdate {
            before_sibling_id: Some(before_sibling_id.map(String::from)),
            ..Default::default()
        };

        self.update_node(node_id, update).await
    }

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::NodeFilter;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// let filter = NodeFilter::new()
    ///     .with_node_type("task".to_string())
    ///     .with_limit(10);
    /// let nodes = service.query_nodes(filter).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_nodes(&self, filter: NodeFilter) -> Result<Vec<Node>, NodeServiceError> {
        let conn = self.db.connect()?;

        // For simple queries, we'll use specific patterns. Complex dynamic queries need a query builder
        // For this implementation, we'll handle the most common cases

        if let Some(ref node_type) = filter.node_type {
            // Query by node_type
            let order_clause = match filter.order_by {
                Some(OrderBy::CreatedAsc) => " ORDER BY created_at ASC",
                Some(OrderBy::CreatedDesc) => " ORDER BY created_at DESC",
                Some(OrderBy::ModifiedAsc) => " ORDER BY modified_at ASC",
                Some(OrderBy::ModifiedDesc) => " ORDER BY modified_at DESC",
                _ => "",
            };

            let limit_clause = filter
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            let query = format!(
                "SELECT id, node_type, content, parent_id, origin_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector FROM nodes WHERE node_type = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

            let mut rows = stmt.query([node_type.as_str()]).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
            })?;

            let mut nodes = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                nodes.push(self.row_to_node(row)?);
            }

            return Ok(nodes);
        } else if let Some(ref parent_id) = filter.parent_id {
            // Query by parent_id
            let order_clause = match filter.order_by {
                Some(OrderBy::CreatedAsc) => " ORDER BY created_at ASC",
                Some(OrderBy::CreatedDesc) => " ORDER BY created_at DESC",
                Some(OrderBy::ModifiedAsc) => " ORDER BY modified_at ASC",
                Some(OrderBy::ModifiedDesc) => " ORDER BY modified_at DESC",
                _ => "",
            };

            let limit_clause = filter
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            let query = format!(
                "SELECT id, node_type, content, parent_id, origin_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector FROM nodes WHERE parent_id = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

            let mut rows = stmt.query([parent_id.as_str()]).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
            })?;

            let mut nodes = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                nodes.push(self.row_to_node(row)?);
            }

            return Ok(nodes);
        } else if let Some(ref origin_node_id) = filter.origin_node_id {
            // Query by origin_node_id (bulk fetch optimization)
            let order_clause = match filter.order_by {
                Some(OrderBy::CreatedAsc) => " ORDER BY created_at ASC",
                Some(OrderBy::CreatedDesc) => " ORDER BY created_at DESC",
                Some(OrderBy::ModifiedAsc) => " ORDER BY modified_at ASC",
                Some(OrderBy::ModifiedDesc) => " ORDER BY modified_at DESC",
                _ => "",
            };

            let limit_clause = filter
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            let query = format!(
                "SELECT id, node_type, content, parent_id, origin_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector FROM nodes WHERE origin_node_id = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

            let mut rows = stmt.query([origin_node_id.as_str()]).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
            })?;

            let mut nodes = Vec::new();
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                nodes.push(self.row_to_node(row)?);
            }

            return Ok(nodes);
        }

        // Default: return all nodes (with optional ordering/limit)
        let order_clause = match filter.order_by {
            Some(OrderBy::CreatedAsc) => " ORDER BY created_at ASC",
            Some(OrderBy::CreatedDesc) => " ORDER BY created_at DESC",
            Some(OrderBy::ModifiedAsc) => " ORDER BY modified_at ASC",
            Some(OrderBy::ModifiedDesc) => " ORDER BY modified_at DESC",
            _ => "",
        };

        let limit_clause = filter
            .limit
            .map(|l| format!(" LIMIT {}", l))
            .unwrap_or_default();

        let query = format!(
            "SELECT id, node_type, content, parent_id, origin_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector FROM nodes{}{}",
            order_clause, limit_clause
        );

        let mut stmt = conn.prepare(&query).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
        })?;

        let mut rows = stmt.query(()).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        let mut nodes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            nodes.push(self.row_to_node(row)?);
        }

        Ok(nodes)
    }

    // Helper methods

    /// Check if a node exists
    async fn node_exists(&self, id: &str) -> Result<bool, NodeServiceError> {
        let conn = self.db.connect()?;

        let mut stmt = conn
            .prepare("SELECT COUNT(*) FROM nodes WHERE id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

        let mut rows = stmt.query([id]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let count: i64 = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            Ok(count > 0)
        } else {
            Ok(false)
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

            if let Some(node) = self.get_node(&current_id).await? {
                if let Some(parent_id) = node.parent_id {
                    current_id = parent_id;
                } else {
                    break; // Reached root without finding node_id
                }
            } else {
                break;
            }
        }

        Ok(false)
    }

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::Node;
    /// # use std::path::PathBuf;
    /// # use serde_json::json;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// let nodes = vec![
    ///     Node::new("text".to_string(), "Note 1".to_string(), None, json!({})),
    ///     Node::new("text".to_string(), "Note 2".to_string(), None, json!({})),
    /// ];
    /// let ids = service.bulk_create(nodes).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_create(&self, nodes: Vec<Node>) -> Result<Vec<String>, NodeServiceError> {
        if nodes.is_empty() {
            return Ok(Vec::new());
        }

        // Validate all nodes first
        for node in &nodes {
            self.behaviors.validate_node(node)?;
        }

        let conn = self.db.connect()?;

        // Begin transaction
        conn.execute("BEGIN TRANSACTION", ()).await.map_err(|e| {
            NodeServiceError::transaction_failed(format!("Failed to begin transaction: {}", e))
        })?;

        let mut ids = Vec::new();

        // Insert all nodes
        // Database defaults handle created_at and modified_at timestamps automatically
        for node in &nodes {
            let properties_json = serde_json::to_string(&node.properties)
                .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

            let result = conn.execute(
                "INSERT INTO nodes (id, node_type, content, parent_id, origin_node_id, before_sibling_id, properties, embedding_vector)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    node.id.as_str(),
                    node.node_type.as_str(),
                    node.content.as_str(),
                    node.parent_id.as_deref(),
                    node.origin_node_id.as_deref(),
                    node.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    node.embedding_vector.as_deref(),
                ),
            )
            .await;

            if let Err(e) = result {
                // Rollback on error
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(NodeServiceError::bulk_operation_failed(format!(
                    "Failed to insert node {}: {}",
                    node.id, e
                )));
            }

            ids.push(node.id.clone());
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            NodeServiceError::transaction_failed(format!("Failed to commit transaction: {}", e))
        })?;

        Ok(ids)
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_core::models::NodeUpdate;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
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

        let conn = self.db.connect()?;

        // Begin transaction
        conn.execute("BEGIN TRANSACTION", ()).await.map_err(|e| {
            NodeServiceError::transaction_failed(format!("Failed to begin transaction: {}", e))
        })?;

        for (id, update) in updates {
            // Apply update similar to update_node but in transaction
            let existing = self
                .get_node(&id)
                .await?
                .ok_or_else(|| NodeServiceError::node_not_found(&id))?;

            let mut updated = existing.clone();

            if let Some(node_type) = update.node_type {
                updated.node_type = node_type;
            }

            if let Some(content) = update.content {
                updated.content = content;
            }

            if let Some(parent_id) = update.parent_id {
                updated.parent_id = parent_id;
            }

            if let Some(origin_node_id) = update.origin_node_id {
                updated.origin_node_id = origin_node_id;
            }

            if let Some(before_sibling_id) = update.before_sibling_id {
                updated.before_sibling_id = before_sibling_id;
            }

            if let Some(properties) = update.properties {
                updated.properties = properties;
            }

            if let Some(embedding_vector) = update.embedding_vector {
                updated.embedding_vector = embedding_vector;
            }

            updated.modified_at = Utc::now();

            // Validate updated node
            if let Err(e) = self.behaviors.validate_node(&updated) {
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(NodeServiceError::bulk_operation_failed(format!(
                    "Failed to validate node {}: {}",
                    id, e
                )));
            }

            // Execute update in transaction - database auto-updates modified_at
            let properties_json = serde_json::to_string(&updated.properties)
                .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

            let result = conn.execute(
                "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, origin_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ? WHERE id = ?",
                (
                    updated.node_type.as_str(),
                    updated.content.as_str(),
                    updated.parent_id.as_deref(),
                    updated.origin_node_id.as_deref(),
                    updated.before_sibling_id.as_deref(),
                    properties_json.as_str(),
                    updated.embedding_vector.as_deref(),
                    id.as_str(),
                ),
            )
            .await;

            if let Err(e) = result {
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(NodeServiceError::bulk_operation_failed(format!(
                    "Failed to update node {}: {}",
                    id, e
                )));
            }
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            NodeServiceError::transaction_failed(format!("Failed to commit transaction: {}", e))
        })?;

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db).await?;
    /// let ids = vec!["node-1".to_string(), "node-2".to_string()];
    /// service.bulk_delete(ids).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_delete(&self, ids: Vec<String>) -> Result<(), NodeServiceError> {
        if ids.is_empty() {
            return Ok(());
        }

        let conn = self.db.connect()?;

        // Begin transaction
        conn.execute("BEGIN TRANSACTION", ()).await.map_err(|e| {
            NodeServiceError::transaction_failed(format!("Failed to begin transaction: {}", e))
        })?;

        for id in &ids {
            let result = conn
                .execute("DELETE FROM nodes WHERE id = ?", [id.as_str()])
                .await;

            if let Err(e) = result {
                // Rollback on error
                let _rollback = conn.execute("ROLLBACK", ()).await;
                return Err(NodeServiceError::bulk_operation_failed(format!(
                    "Failed to delete node {}: {}",
                    id, e
                )));
            }
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            NodeServiceError::transaction_failed(format!("Failed to commit transaction: {}", e))
        })?;

        Ok(())
    }

    /// Upsert a node with automatic parent creation - single transaction
    ///
    /// Creates parent node if it doesn't exist, then upserts the child node.
    /// All operations happen in a single transaction to prevent database locking.
    ///
    /// # Arguments
    /// * `node_id` - ID of the node to upsert
    /// * `content` - Node content
    /// * `node_type` - Type of node (text, task, date)
    /// * `parent_id` - Parent node ID (will be created as date node if missing)
    ///
    /// # Returns
    /// * `Ok(())` - Operation successful
    /// * `Err(NodeServiceError)` - If transaction fails
    pub async fn upsert_node_with_parent(
        &self,
        node_id: &str,
        content: &str,
        node_type: &str,
        parent_id: &str,
        origin_node_id: &str,
        before_sibling_id: Option<&str>,
    ) -> Result<(), NodeServiceError> {
        let conn = self.db.connect_with_timeout().await?;

        // Use DEFERRED transaction for better concurrency with WAL mode
        // Lock is only acquired when first write occurs, not at BEGIN
        conn.execute("BEGIN DEFERRED", ()).await.map_err(|e| {
            NodeServiceError::transaction_failed(format!("Failed to begin transaction: {}", e))
        })?;

        // Use INSERT OR IGNORE for parent - won't error if already exists
        let parent_result = conn.execute(
            "INSERT OR IGNORE INTO nodes (id, node_type, content, parent_id, origin_node_id, before_sibling_id, properties, embedding_vector)
             VALUES (?, 'date', ?, NULL, NULL, NULL, '{}', NULL)",
            (parent_id, parent_id)
        ).await;

        if let Err(e) = parent_result {
            let _rollback = conn.execute("ROLLBACK", ()).await;
            return Err(NodeServiceError::query_failed(format!(
                "Failed to ensure parent exists: {}",
                e
            )));
        }

        // Use INSERT ... ON CONFLICT for node - upsert in single operation
        let node_result = conn.execute(
            "INSERT INTO nodes (id, node_type, content, parent_id, origin_node_id, before_sibling_id, properties, embedding_vector)
             VALUES (?, ?, ?, ?, ?, ?, '{}', NULL)
             ON CONFLICT(id) DO UPDATE SET
                content = excluded.content,
                parent_id = excluded.parent_id,
                origin_node_id = excluded.origin_node_id,
                before_sibling_id = excluded.before_sibling_id,
                modified_at = CURRENT_TIMESTAMP",
            (node_id, node_type, content, parent_id, origin_node_id, before_sibling_id)
        ).await;

        if let Err(e) = node_result {
            let _rollback = conn.execute("ROLLBACK", ()).await;
            return Err(NodeServiceError::query_failed(format!(
                "Failed to upsert node: {}",
                e
            )));
        }

        // Commit transaction
        conn.execute("COMMIT", ()).await.map_err(|e| {
            std::mem::drop(conn.execute("ROLLBACK", ()));
            NodeServiceError::transaction_failed(format!("Failed to commit transaction: {}", e))
        })?;

        Ok(())
    }

    // Helper methods

    /// Convert a database row to a Node
    fn row_to_node(&self, row: libsql::Row) -> Result<Node, NodeServiceError> {
        let id: String = row
            .get(0)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let node_type: String = row
            .get(1)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let content: String = row
            .get(2)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let parent_id: Option<String> = row
            .get(3)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let origin_node_id: Option<String> = row
            .get(4)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let before_sibling_id: Option<String> = row
            .get(5)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let created_at: String = row
            .get(6)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let modified_at: String = row
            .get(7)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let properties_json: String = row
            .get(8)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
        let embedding_vector: Option<Vec<u8>> = row
            .get(9)
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;

        let properties: serde_json::Value =
            serde_json::from_str(&properties_json).map_err(|e| {
                NodeServiceError::serialization_error(format!("Failed to parse properties: {}", e))
            })?;

        // Parse timestamps - handle both SQLite format and RFC3339 (for migration)
        let created_at = parse_timestamp(&created_at).map_err(|e| {
            NodeServiceError::serialization_error(format!(
                "Failed to parse created_at '{}': {}",
                created_at, e
            ))
        })?;

        let modified_at = parse_timestamp(&modified_at).map_err(|e| {
            NodeServiceError::serialization_error(format!(
                "Failed to parse modified_at '{}': {}",
                modified_at, e
            ))
        })?;

        Ok(Node {
            id,
            node_type,
            content,
            parent_id,
            origin_node_id,
            before_sibling_id,
            created_at,
            modified_at,
            properties,
            embedding_vector,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn create_test_service() -> (NodeService, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = DatabaseService::new(db_path).await.unwrap();
        let service = NodeService::new(db).await.unwrap();
        (service, temp_dir)
    }

    #[tokio::test]
    async fn test_create_text_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new(
            "text".to_string(),
            "Hello World".to_string(),
            None,
            json!({}),
        );

        let id = service.create_node(node.clone()).await.unwrap();
        assert_eq!(id, node.id);

        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Hello World");
        assert_eq!(retrieved.node_type, "text");
    }

    #[tokio::test]
    async fn test_create_task_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new(
            "task".to_string(),
            "Implement NodeService".to_string(),
            None,
            json!({"status": "in_progress", "priority": 1}),
        );

        let id = service.create_node(node).await.unwrap();
        let retrieved = service.get_node(&id).await.unwrap().unwrap();

        assert_eq!(retrieved.node_type, "task");
        assert_eq!(retrieved.properties["status"], "in_progress");
        assert_eq!(retrieved.properties["priority"], 1);
    }

    #[tokio::test]
    async fn test_create_date_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new_with_id(
            "2025-01-03".to_string(),
            "date".to_string(),
            "2025-01-03".to_string(),
            None,
            json!({}),
        );

        let id = service.create_node(node).await.unwrap();
        assert_eq!(id, "2025-01-03");

        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.node_type, "date");
        assert_eq!(retrieved.id, "2025-01-03");
    }

    #[tokio::test]
    async fn test_update_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new("text".to_string(), "Original".to_string(), None, json!({}));

        let id = service.create_node(node).await.unwrap();

        let update = NodeUpdate::new().with_content("Updated".to_string());
        service.update_node(&id, update).await.unwrap();

        let retrieved = service.get_node(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Updated");
    }

    #[tokio::test]
    async fn test_delete_node() {
        let (service, _temp) = create_test_service().await;

        let node = Node::new(
            "text".to_string(),
            "To be deleted".to_string(),
            None,
            json!({}),
        );

        let id = service.create_node(node).await.unwrap();
        service.delete_node(&id).await.unwrap();

        let retrieved = service.get_node(&id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_get_children() {
        let (service, _temp) = create_test_service().await;

        let parent = Node::new("text".to_string(), "Parent".to_string(), None, json!({}));
        let parent_id = service.create_node(parent).await.unwrap();

        let child1 = Node::new(
            "text".to_string(),
            "Child 1".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        service.create_node(child1).await.unwrap();

        let child2 = Node::new(
            "text".to_string(),
            "Child 2".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        service.create_node(child2).await.unwrap();

        let children = service.get_children(&parent_id).await.unwrap();
        assert_eq!(children.len(), 2);
    }

    #[tokio::test]
    async fn test_move_node() {
        let (service, _temp) = create_test_service().await;

        let root = Node::new("text".to_string(), "Root".to_string(), None, json!({}));
        let origin_node_id = service.create_node(root).await.unwrap();

        let node = Node::new("text".to_string(), "Node".to_string(), None, json!({}));
        let node_id = service.create_node(node).await.unwrap();

        service
            .move_node(&node_id, Some(&origin_node_id))
            .await
            .unwrap();

        let moved = service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(moved.parent_id, Some(origin_node_id.clone()));
        assert_eq!(moved.origin_node_id, Some(origin_node_id));
    }

    #[tokio::test]
    async fn test_query_nodes_by_type() {
        let (service, _temp) = create_test_service().await;

        service
            .create_node(Node::new(
                "text".to_string(),
                "Text 1".to_string(),
                None,
                json!({}),
            ))
            .await
            .unwrap();
        service
            .create_node(Node::new(
                "task".to_string(),
                "Task 1".to_string(),
                None,
                json!({"status": "pending"}),
            ))
            .await
            .unwrap();
        service
            .create_node(Node::new(
                "text".to_string(),
                "Text 2".to_string(),
                None,
                json!({}),
            ))
            .await
            .unwrap();

        let filter = NodeFilter::new().with_node_type("text".to_string());
        let results = service.query_nodes(filter).await.unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|n| n.node_type == "text"));
    }

    #[tokio::test]
    async fn test_bulk_create() {
        let (service, _temp) = create_test_service().await;

        let nodes = vec![
            Node::new("text".to_string(), "Bulk 1".to_string(), None, json!({})),
            Node::new("text".to_string(), "Bulk 2".to_string(), None, json!({})),
            Node::new(
                "task".to_string(),
                "Bulk Task".to_string(),
                None,
                json!({"status": "pending"}),
            ),
        ];

        let ids = service.bulk_create(nodes.clone()).await.unwrap();
        assert_eq!(ids.len(), 3);

        for (i, id) in ids.iter().enumerate() {
            let node = service.get_node(id).await.unwrap().unwrap();
            assert_eq!(node.content, nodes[i].content);
        }
    }

    #[tokio::test]
    async fn test_bulk_update() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new(
            "text".to_string(),
            "Original 1".to_string(),
            None,
            json!({}),
        );
        let node2 = Node::new(
            "text".to_string(),
            "Original 2".to_string(),
            None,
            json!({}),
        );

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        let updates = vec![
            (
                id1.clone(),
                NodeUpdate::new().with_content("Updated 1".to_string()),
            ),
            (
                id2.clone(),
                NodeUpdate::new().with_content("Updated 2".to_string()),
            ),
        ];

        service.bulk_update(updates).await.unwrap();

        let updated1 = service.get_node(&id1).await.unwrap().unwrap();
        let updated2 = service.get_node(&id2).await.unwrap().unwrap();

        assert_eq!(updated1.content, "Updated 1");
        assert_eq!(updated2.content, "Updated 2");
    }

    #[tokio::test]
    async fn test_bulk_delete() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Delete 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Delete 2".to_string(), None, json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        service
            .bulk_delete(vec![id1.clone(), id2.clone()])
            .await
            .unwrap();

        assert!(service.get_node(&id1).await.unwrap().is_none());
        assert!(service.get_node(&id2).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_circular_reference_prevention() {
        let (service, _temp) = create_test_service().await;

        let parent = Node::new("text".to_string(), "Parent".to_string(), None, json!({}));
        let parent_id = service.create_node(parent).await.unwrap();

        let child = Node::new(
            "text".to_string(),
            "Child".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        let child_id = service.create_node(child).await.unwrap();

        // Attempt to move parent under child (circular reference)
        let result = service.move_node(&parent_id, Some(&child_id)).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            NodeServiceError::CircularReference { .. }
        ));
    }

    #[tokio::test]
    async fn test_reorder_siblings() {
        let (service, _temp) = create_test_service().await;

        let parent = Node::new("text".to_string(), "Parent".to_string(), None, json!({}));
        let parent_id = service.create_node(parent).await.unwrap();

        let child1 = Node::new(
            "text".to_string(),
            "Child 1".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        let child1_id = service.create_node(child1).await.unwrap();

        let child2 = Node::new(
            "text".to_string(),
            "Child 2".to_string(),
            Some(parent_id.clone()),
            json!({}),
        );
        let child2_id = service.create_node(child2).await.unwrap();

        // Reorder child2 to be before child1
        service
            .reorder_siblings(&child2_id, Some(&child1_id))
            .await
            .unwrap();

        let reordered = service.get_node(&child2_id).await.unwrap().unwrap();
        assert_eq!(reordered.before_sibling_id, Some(child1_id));
    }

    #[tokio::test]
    async fn test_transaction_rollback_on_error() {
        let (service, _temp) = create_test_service().await;

        // Create one valid node and one invalid node
        let valid_node = Node::new("text".to_string(), "Valid".to_string(), None, json!({}));
        let mut invalid_node = Node::new("text".to_string(), "".to_string(), None, json!({})); // Empty content invalid for text
        invalid_node.content = "   ".to_string(); // Whitespace-only content

        let nodes = vec![valid_node.clone(), invalid_node];

        // Bulk create should fail
        let result = service.bulk_create(nodes).await;
        assert!(result.is_err());

        // Verify that valid node was NOT created (transaction rolled back)
        let check = service.get_node(&valid_node.id).await.unwrap();
        assert!(check.is_none());
    }

    #[tokio::test]
    async fn test_get_nodes_by_origin_id() {
        let (service, _temp) = create_test_service().await;

        // Create a date node (acts as origin)
        let date_node = Node::new_with_id(
            "2025-10-05".to_string(),
            "date".to_string(),
            "2025-10-05".to_string(),
            None,
            json!({}),
        );
        service.create_node(date_node.clone()).await.unwrap();

        // Create children with origin_node_id pointing to the date
        let child1 = Node::new_with_root(
            "text".to_string(),
            "Child 1".to_string(),
            Some("2025-10-05".to_string()),
            Some("2025-10-05".to_string()),
            json!({}),
        );
        let child2 = Node::new_with_root(
            "text".to_string(),
            "Child 2".to_string(),
            Some("2025-10-05".to_string()),
            Some("2025-10-05".to_string()),
            json!({}),
        );
        let child3 = Node::new_with_root(
            "task".to_string(),
            "Child 3".to_string(),
            Some("2025-10-05".to_string()),
            Some("2025-10-05".to_string()),
            json!({"status": "pending"}),
        );

        service.create_node(child1.clone()).await.unwrap();
        service.create_node(child2.clone()).await.unwrap();
        service.create_node(child3.clone()).await.unwrap();

        // Create a different date node with a child (should not be returned)
        let other_date = Node::new_with_id(
            "2025-10-06".to_string(),
            "date".to_string(),
            "2025-10-06".to_string(),
            None,
            json!({}),
        );
        service.create_node(other_date.clone()).await.unwrap();

        let other_child = Node::new_with_root(
            "text".to_string(),
            "Other child".to_string(),
            Some("2025-10-06".to_string()),
            Some("2025-10-06".to_string()),
            json!({}),
        );
        service.create_node(other_child).await.unwrap();

        // Bulk fetch should return only the 3 children with origin_node_id = "2025-10-05"
        let nodes = service.get_nodes_by_origin_id("2025-10-05").await.unwrap();

        assert_eq!(nodes.len(), 3, "Should return exactly 3 nodes");
        assert!(
            nodes
                .iter()
                .all(|n| n.origin_node_id == Some("2025-10-05".to_string())),
            "All nodes should have origin_node_id = '2025-10-05'"
        );

        // Verify it returns different node types
        let node_types: Vec<&str> = nodes.iter().map(|n| n.node_type.as_str()).collect();
        assert!(node_types.contains(&"text"), "Should contain text nodes");
        assert!(node_types.contains(&"task"), "Should contain task node");

        // Verify the other date's child is not included
        assert!(
            !nodes.iter().any(|n| n.content == "Other child"),
            "Should not return children from other origins"
        );
    }
}
