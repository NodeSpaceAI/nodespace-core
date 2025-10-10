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
///     let service = NodeService::new(db)?;
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
    /// let service = NodeService::new(db)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(db: DatabaseService) -> Result<Self, NodeServiceError> {
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
    /// - Root node doesn't exist (if container_node_id is set)
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
    /// # let service = NodeService::new(db)?;
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

        // Validate root exists if container_node_id is set
        if let Some(ref container_node_id) = node.container_node_id {
            let root_exists = self.node_exists(container_node_id).await?;
            if !root_exists {
                return Err(NodeServiceError::invalid_root(container_node_id));
            }
        }

        // Insert into database
        // Database defaults handle created_at and modified_at timestamps automatically
        let conn = self.db.connect()?;

        let properties_json = serde_json::to_string(&node.properties)
            .map_err(|e| NodeServiceError::serialization_error(e.to_string()))?;

        conn.execute(
            "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (
                node.id.as_str(),
                node.node_type.as_str(),
                node.content.as_str(),
                node.parent_id.as_deref(),
                node.container_node_id.as_deref(),
                node.before_sibling_id.as_deref(),
                properties_json.as_str(),
                node.embedding_vector.as_deref(),
            ),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to insert node: {}", e)))?;

        Ok(node.id)
    }

    /// Create a mention relationship between two existing nodes
    ///
    /// Adds an entry to the node_mentions table to track that one node mentions another.
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
        // Validate both nodes exist
        if !self.node_exists(mentioning_node_id).await? {
            return Err(NodeServiceError::node_not_found(mentioning_node_id));
        }
        if !self.node_exists(mentioned_node_id).await? {
            return Err(NodeServiceError::node_not_found(mentioned_node_id));
        }

        let conn = self.db.connect()?;

        conn.execute(
            "INSERT OR IGNORE INTO node_mentions (node_id, mentions_node_id)
             VALUES (?, ?)",
            (mentioning_node_id, mentioned_node_id),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to create mention: {}", e)))?;

        Ok(())
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
    /// # let service = NodeService::new(db)?;
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
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id,
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
            let mut node = self.row_to_node(row)?;
            self.populate_mentions(&mut node).await?;
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
    /// # let service = NodeService::new(db)?;
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

        if let Some(container_node_id) = update.container_node_id {
            updated.container_node_id = container_node_id;
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
            "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ? WHERE id = ?",
            (
                updated.node_type.as_str(),
                updated.content.as_str(),
                updated.parent_id.as_deref(),
                updated.container_node_id.as_deref(),
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
    /// # let service = NodeService::new(db)?;
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
    /// # let service = NodeService::new(db)?;
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
    /// 1. Single database query fetches all nodes with the same container_node_id
    /// 2. In-memory hierarchy reconstruction using parent_id and before_sibling_id
    ///
    /// This avoids making multiple queries for each level of the tree.
    ///
    /// # Arguments
    ///
    /// * `container_node_id` - The ID of the origin node (e.g., date page ID)
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
    /// # let service = NodeService::new(db)?;
    /// // Fetch all nodes for a date page
    /// let nodes = service.get_nodes_by_origin_id("2025-10-05").await?;
    /// println!("Found {} nodes in this document", nodes.len());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_nodes_by_origin_id(
        &self,
        container_node_id: &str,
    ) -> Result<Vec<Node>, NodeServiceError> {
        let filter = NodeFilter::new().with_container_node_id(container_node_id.to_string());

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
    /// Updates the parent_id and container_node_id of a node, maintaining hierarchy consistency.
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
    /// # let service = NodeService::new(db)?;
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

        // Determine new container_node_id
        let new_container_node_id = match new_parent {
            Some(parent_id) => {
                // Get parent's container_node_id, or use parent as root if it's a root node
                let parent = self
                    .get_node(parent_id)
                    .await?
                    .ok_or_else(|| NodeServiceError::invalid_parent(parent_id))?;
                parent.container_node_id.or(Some(parent_id.to_string()))
            }
            None => None, // Node becomes a root
        };

        let update = NodeUpdate {
            parent_id: Some(new_parent.map(String::from)),
            container_node_id: Some(new_container_node_id),
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
    /// # let service = NodeService::new(db)?;
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
    /// # let service = NodeService::new(db)?;
    /// let filter = NodeFilter::new()
    ///     .with_node_type("task".to_string())
    ///     .with_limit(10);
    /// let nodes = service.query_nodes(filter).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn query_nodes(&self, filter: NodeFilter) -> Result<Vec<Node>, NodeServiceError> {
        // Use connection with busy timeout for better concurrency handling
        let conn = self.db.connect_with_timeout().await?;

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
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector FROM nodes WHERE node_type = ?{}{}",
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
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector FROM nodes WHERE parent_id = ?{}{}",
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
        } else if let Some(ref container_node_id) = filter.container_node_id {
            // Query by container_node_id (bulk fetch optimization)
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
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector FROM nodes WHERE container_node_id = ?{}{}",
                order_clause, limit_clause
            );

            let mut stmt = conn.prepare(&query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare query: {}", e))
            })?;

            let mut rows = stmt
                .query([container_node_id.as_str()])
                .await
                .map_err(|e| {
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
            "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector FROM nodes{}{}",
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
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
    pub async fn query_nodes_simple(
        &self,
        query: crate::models::NodeQuery,
    ) -> Result<Vec<Node>, NodeServiceError> {
        // Use connection with busy timeout to prevent immediate failure on lock contention
        // This is especially important for queries that might run concurrently with deletes
        let conn = self.db.connect_with_timeout().await?;

        // Priority 1: Query by ID (exact match)
        if let Some(ref id) = query.id {
            if let Some(node) = self.get_node(id).await? {
                return Ok(vec![node]);
            } else {
                return Ok(vec![]);
            }
        }

        // Priority 2: Query nodes that mention a specific node (mentioned_by)
        // Uses the node_mentions table for efficient lookups
        if let Some(ref mentioned_node_id) = query.mentioned_by {
            let limit_clause = query
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            // Query node_mentions table to find nodes that reference the target
            let sql_query = format!(
                "SELECT n.id, n.node_type, n.content, n.parent_id, n.container_node_id, n.before_sibling_id, n.created_at, n.modified_at, n.properties, n.embedding_vector
                 FROM nodes n
                 INNER JOIN node_mentions nm ON n.id = nm.node_id
                 WHERE nm.mentions_node_id = ?{}",
                limit_clause
            );

            let mut stmt = conn.prepare(&sql_query).await.map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to prepare mentioned_by query: {}",
                    e
                ))
            })?;

            let mut rows = stmt
                .query([mentioned_node_id.as_str()])
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to execute mentioned_by query: {}",
                        e
                    ))
                })?;

            let mut nodes = Vec::new();
            // TODO(performance): This creates N+1 query pattern (2 queries per node).
            // Consider implementing populate_mentions_batch() when queries regularly
            // return >50 nodes. See issue #158 for batch optimization implementation.
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                let mut node = self.row_to_node(row)?;
                self.populate_mentions(&mut node).await?;
                nodes.push(node);
            }

            return Ok(nodes);
        }

        // Priority 3: Query by content substring (case-insensitive LIKE)
        if let Some(ref content_search) = query.content_contains {
            let content_pattern = format!("%{}%", content_search);
            let limit_clause = query
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            // Determine SQL query based on whether node_type is specified
            let mut rows = if let Some(ref node_type) = query.node_type {
                // Case 1: Filter by both content and node_type
                let sql = format!(
                    "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector
                     FROM nodes WHERE content LIKE ? AND node_type = ?{}",
                    limit_clause
                );

                let mut stmt = conn.prepare(&sql).await.map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to prepare content_contains query: {}",
                        e
                    ))
                })?;

                stmt.query([content_pattern.as_str(), node_type.as_str()])
                    .await
                    .map_err(|e| {
                        NodeServiceError::query_failed(format!(
                            "Failed to execute content_contains query: {}",
                            e
                        ))
                    })?
            } else {
                // Case 2: Filter by content only
                let sql = format!(
                    "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector
                     FROM nodes WHERE content LIKE ?{}",
                    limit_clause
                );

                let mut stmt = conn.prepare(&sql).await.map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to prepare content_contains query: {}",
                        e
                    ))
                })?;

                stmt.query([content_pattern.as_str()]).await.map_err(|e| {
                    NodeServiceError::query_failed(format!(
                        "Failed to execute content_contains query: {}",
                        e
                    ))
                })?
            };

            // Collect results with mentions populated
            let mut nodes = Vec::new();
            // TODO(performance): This creates N+1 query pattern (2 queries per node).
            // Consider implementing populate_mentions_batch() when queries regularly
            // return >50 nodes. See issue #158 for batch optimization implementation.
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
            {
                let mut node = self.row_to_node(row)?;
                self.populate_mentions(&mut node).await?;
                nodes.push(node);
            }

            return Ok(nodes);
        }

        // Priority 4: Query by node_type only
        if let Some(ref node_type) = query.node_type {
            let limit_clause = query
                .limit
                .map(|l| format!(" LIMIT {}", l))
                .unwrap_or_default();

            let sql_query = format!(
                "SELECT id, node_type, content, parent_id, container_node_id, before_sibling_id, created_at, modified_at, properties, embedding_vector
                 FROM nodes WHERE node_type = ?{}",
                limit_clause
            );

            let mut stmt = conn.prepare(&sql_query).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare node_type query: {}", e))
            })?;

            let mut rows = stmt.query([node_type.as_str()]).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to execute node_type query: {}", e))
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

        // Default: Empty query returns no results (safer than returning all nodes)
        Ok(vec![])
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
    /// # let service = NodeService::new(db)?;
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
                "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    node.id.as_str(),
                    node.node_type.as_str(),
                    node.content.as_str(),
                    node.parent_id.as_deref(),
                    node.container_node_id.as_deref(),
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
    /// # let service = NodeService::new(db)?;
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

            if let Some(container_node_id) = update.container_node_id {
                updated.container_node_id = container_node_id;
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
                "UPDATE nodes SET node_type = ?, content = ?, parent_id = ?, container_node_id = ?, before_sibling_id = ?, modified_at = CURRENT_TIMESTAMP, properties = ?, embedding_vector = ? WHERE id = ?",
                (
                    updated.node_type.as_str(),
                    updated.content.as_str(),
                    updated.parent_id.as_deref(),
                    updated.container_node_id.as_deref(),
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
    /// # let service = NodeService::new(db)?;
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
        container_node_id: &str,
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
            "INSERT OR IGNORE INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
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
            "INSERT INTO nodes (id, node_type, content, parent_id, container_node_id, before_sibling_id, properties, embedding_vector)
             VALUES (?, ?, ?, ?, ?, ?, '{}', NULL)
             ON CONFLICT(id) DO UPDATE SET
                content = excluded.content,
                parent_id = excluded.parent_id,
                container_node_id = excluded.container_node_id,
                before_sibling_id = excluded.before_sibling_id,
                modified_at = CURRENT_TIMESTAMP",
            (node_id, node_type, content, parent_id, container_node_id, before_sibling_id)
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
        let container_node_id: Option<String> = row
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
            container_node_id,
            before_sibling_id,
            created_at,
            modified_at,
            properties,
            embedding_vector,
            mentions: Vec::new(), // Populated separately via populate_mentions()
            mentioned_by: Vec::new(), // Populated separately via populate_mentions()
        })
    }

    /// Populate mentions fields from node_mentions table
    ///
    /// Queries the node_mentions table to populate both outgoing mentions
    /// and incoming mentioned_by references for a node.
    ///
    /// # Arguments
    ///
    /// * `node` - Mutable reference to node to populate
    async fn populate_mentions(&self, node: &mut Node) -> Result<(), NodeServiceError> {
        let conn = self.db.connect()?;

        // Query outgoing mentions (nodes that THIS node references)
        let mut stmt = conn
            .prepare("SELECT mentions_node_id FROM node_mentions WHERE node_id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare mentions query: {}", e))
            })?;

        let mut rows = stmt.query([node.id.as_str()]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute mentions query: {}", e))
        })?;

        let mut mentions = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let mentioned_id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            mentions.push(mentioned_id);
        }
        node.mentions = mentions;

        // Query incoming mentions (nodes that reference THIS node)
        let mut stmt = conn
            .prepare("SELECT node_id FROM node_mentions WHERE mentions_node_id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to prepare mentioned_by query: {}",
                    e
                ))
            })?;

        let mut rows = stmt.query([node.id.as_str()]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute mentioned_by query: {}", e))
        })?;

        let mut mentioned_by = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let mentioning_id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            mentioned_by.push(mentioning_id);
        }
        node.mentioned_by = mentioned_by;

        Ok(())
    }

    /// Add a mention from one node to another
    ///
    /// Creates a mention relationship in the node_mentions table.
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// service.add_mention("node-123", "node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn add_mention(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Verify both nodes exist
        if !self.node_exists(source_id).await? {
            return Err(NodeServiceError::node_not_found(source_id));
        }
        if !self.node_exists(target_id).await? {
            return Err(NodeServiceError::node_not_found(target_id));
        }

        let conn = self.db.connect()?;

        // Use INSERT OR IGNORE to handle duplicates gracefully
        conn.execute(
            "INSERT OR IGNORE INTO node_mentions (node_id, mentions_node_id) VALUES (?, ?)",
            (source_id, target_id),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to insert mention: {}", e)))?;

        Ok(())
    }

    /// Remove a mention from one node to another
    ///
    /// Deletes a mention relationship from the node_mentions table.
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// service.remove_mention("node-123", "node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn remove_mention(
        &self,
        source_id: &str,
        target_id: &str,
    ) -> Result<(), NodeServiceError> {
        let conn = self.db.connect()?;

        conn.execute(
            "DELETE FROM node_mentions WHERE node_id = ? AND mentions_node_id = ?",
            (source_id, target_id),
        )
        .await
        .map_err(|e| NodeServiceError::query_failed(format!("Failed to delete mention: {}", e)))?;

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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// let mentions = service.get_mentions("node-123").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentions(&self, node_id: &str) -> Result<Vec<String>, NodeServiceError> {
        let conn = self.db.connect()?;

        let mut stmt = conn
            .prepare("SELECT mentions_node_id FROM node_mentions WHERE node_id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to prepare mentions query: {}", e))
            })?;

        let mut rows = stmt.query([node_id]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute mentions query: {}", e))
        })?;

        let mut mentions = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let mentioned_id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            mentions.push(mentioned_id);
        }

        Ok(mentions)
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
    /// # use nodespace_core::db::DatabaseService;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = DatabaseService::new(PathBuf::from("./test.db")).await?;
    /// # let service = NodeService::new(db)?;
    /// let backlinks = service.get_mentioned_by("node-456").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_mentioned_by(&self, node_id: &str) -> Result<Vec<String>, NodeServiceError> {
        let conn = self.db.connect()?;

        let mut stmt = conn
            .prepare("SELECT node_id FROM node_mentions WHERE mentions_node_id = ?")
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to prepare mentioned_by query: {}",
                    e
                ))
            })?;

        let mut rows = stmt.query([node_id]).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to execute mentioned_by query: {}", e))
        })?;

        let mut mentioned_by = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::query_failed(e.to_string()))?
        {
            let mentioning_id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::query_failed(e.to_string()))?;
            mentioned_by.push(mentioning_id);
        }

        Ok(mentioned_by)
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
        let service = NodeService::new(db).unwrap();
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
        let container_node_id = service.create_node(root).await.unwrap();

        let node = Node::new("text".to_string(), "Node".to_string(), None, json!({}));
        let node_id = service.create_node(node).await.unwrap();

        service
            .move_node(&node_id, Some(&container_node_id))
            .await
            .unwrap();

        let moved = service.get_node(&node_id).await.unwrap().unwrap();
        assert_eq!(moved.parent_id, Some(container_node_id.clone()));
        assert_eq!(moved.container_node_id, Some(container_node_id));
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

        // Create children with container_node_id pointing to the date
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

        // Bulk fetch should return only the 3 children with container_node_id = "2025-10-05"
        let nodes = service.get_nodes_by_origin_id("2025-10-05").await.unwrap();

        assert_eq!(nodes.len(), 3, "Should return exactly 3 nodes");
        assert!(
            nodes
                .iter()
                .all(|n| n.container_node_id == Some("2025-10-05".to_string())),
            "All nodes should have container_node_id = '2025-10-05'"
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

    #[tokio::test]
    async fn test_add_mention() {
        let (service, _temp) = create_test_service().await;

        // Create two nodes
        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Add mention from node1 to node2
        service.add_mention(&id1, &id2).await.unwrap();

        // Verify mention was added
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0], id2);

        // Verify backlink
        let mentioned_by = service.get_mentioned_by(&id2).await.unwrap();
        assert_eq!(mentioned_by.len(), 1);
        assert_eq!(mentioned_by[0], id1);
    }

    #[tokio::test]
    async fn test_remove_mention() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Add and then remove mention
        service.add_mention(&id1, &id2).await.unwrap();
        service.remove_mention(&id1, &id2).await.unwrap();

        // Verify mention was removed
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(mentions.len(), 0);

        let mentioned_by = service.get_mentioned_by(&id2).await.unwrap();
        assert_eq!(mentioned_by.len(), 0);
    }

    #[tokio::test]
    async fn test_get_node_populates_mentions() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));
        let node3 = Node::new("text".to_string(), "Node 3".to_string(), None, json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();
        let id3 = service.create_node(node3).await.unwrap();

        // Node 1 mentions Node 2 and Node 3
        service.add_mention(&id1, &id2).await.unwrap();
        service.add_mention(&id1, &id3).await.unwrap();

        // Node 2 mentions Node 1
        service.add_mention(&id2, &id1).await.unwrap();

        // Fetch node 1 and verify mentions are populated
        let node = service.get_node(&id1).await.unwrap().unwrap();
        assert_eq!(node.mentions.len(), 2);
        assert!(node.mentions.contains(&id2));
        assert!(node.mentions.contains(&id3));
        assert_eq!(node.mentioned_by.len(), 1);
        assert!(node.mentioned_by.contains(&id2));
    }

    #[tokio::test]
    async fn test_query_mentioned_by() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));
        let node3 = Node::new("text".to_string(), "Node 3".to_string(), None, json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();
        let id3 = service.create_node(node3).await.unwrap();

        // Node 1 and Node 2 mention Node 3
        service.add_mention(&id1, &id3).await.unwrap();
        service.add_mention(&id2, &id3).await.unwrap();

        // Query for nodes that mention Node 3
        let query = crate::models::NodeQuery::mentioned_by(id3.clone());
        let nodes = service.query_nodes_simple(query).await.unwrap();

        assert_eq!(nodes.len(), 2);
        let node_ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        assert!(node_ids.contains(&id1));
        assert!(node_ids.contains(&id2));
    }

    #[tokio::test]
    async fn test_mention_duplicate_handling() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Add same mention twice - should not error (INSERT OR IGNORE)
        service.add_mention(&id1, &id2).await.unwrap();
        service.add_mention(&id1, &id2).await.unwrap();

        // Should still only have one mention
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(mentions.len(), 1);
    }

    #[tokio::test]
    async fn test_mention_nonexistent_node() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let id1 = service.create_node(node1).await.unwrap();

        // Try to mention a non-existent node
        let result = service.add_mention(&id1, "nonexistent").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            NodeServiceError::NodeNotFound { .. }
        ));
    }

    #[tokio::test]
    async fn test_bidirectional_mentions() {
        let (service, _temp) = create_test_service().await;

        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Create bidirectional mentions
        service.add_mention(&id1, &id2).await.unwrap();
        service.add_mention(&id2, &id1).await.unwrap();

        // Verify node 1
        let node1 = service.get_node(&id1).await.unwrap().unwrap();
        assert_eq!(node1.mentions.len(), 1);
        assert_eq!(node1.mentions[0], id2);
        assert_eq!(node1.mentioned_by.len(), 1);
        assert_eq!(node1.mentioned_by[0], id2);

        // Verify node 2
        let node2 = service.get_node(&id2).await.unwrap().unwrap();
        assert_eq!(node2.mentions.len(), 1);
        assert_eq!(node2.mentions[0], id1);
        assert_eq!(node2.mentioned_by.len(), 1);
        assert_eq!(node2.mentioned_by[0], id1);
    }

    #[tokio::test]
    async fn test_create_mention_persists_correctly() {
        let (service, _temp) = create_test_service().await;

        // Create two nodes
        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let node2 = Node::new("text".to_string(), "Node 2".to_string(), None, json!({}));

        let id1 = service.create_node(node1).await.unwrap();
        let id2 = service.create_node(node2).await.unwrap();

        // Create mention using the new create_mention() method
        service.create_mention(&id1, &id2).await.unwrap();

        // Verify the mention persists by checking the node_mentions table
        // We can verify this by getting the mentions for node1
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(mentions.len(), 1, "Node 1 should have exactly one mention");
        assert_eq!(mentions[0], id2, "Node 1 should mention Node 2");

        // Verify idempotency - calling create_mention again should not error
        service.create_mention(&id1, &id2).await.unwrap();
        let mentions = service.get_mentions(&id1).await.unwrap();
        assert_eq!(
            mentions.len(),
            1,
            "Should still have only one mention (INSERT OR IGNORE)"
        );
    }

    #[tokio::test]
    async fn test_create_mention_validates_nodes_exist() {
        let (service, _temp) = create_test_service().await;

        // Create only one node
        let node1 = Node::new("text".to_string(), "Node 1".to_string(), None, json!({}));
        let id1 = service.create_node(node1).await.unwrap();

        // Try to create mention to non-existent node
        let result = service.create_mention(&id1, "nonexistent-id").await;
        assert!(
            result.is_err(),
            "Should error when mentioned node doesn't exist"
        );
        assert!(
            matches!(result.unwrap_err(), NodeServiceError::NodeNotFound { .. }),
            "Should return NodeNotFound error"
        );

        // Try to create mention from non-existent node
        let result = service.create_mention("nonexistent-id", &id1).await;
        assert!(
            result.is_err(),
            "Should error when mentioning node doesn't exist"
        );
        assert!(
            matches!(result.unwrap_err(), NodeServiceError::NodeNotFound { .. }),
            "Should return NodeNotFound error"
        );
    }
}
