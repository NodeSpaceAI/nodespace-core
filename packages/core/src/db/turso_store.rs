//! TursoStore - NodeStore Implementation for Turso/libsql Backend
//!
//! This module implements the `NodeStore` trait for Turso (libsql) database,
//! providing the abstraction layer that enables the SurrealDB migration.
//!
//! # Phase 1 Architecture
//!
//! TursoStore wraps the existing DatabaseService and delegates all operations
//! to the extracted `db_*` methods. This provides a thin abstraction layer
//! with zero business logic.
//!
//! # Design Principles
//!
//! 1. **Pure Delegation**: All methods delegate to DatabaseService
//! 2. **Row Conversion**: Handles libsql::Row → Node model conversion
//! 3. **Virtual Nodes**: Preserves date node migration logic
//! 4. **Zero Regressions**: Maintains exact behavior of existing implementation
//!
//! # Performance Target
//!
//! Trait dispatch overhead must be <5% vs direct DatabaseService calls.
//!
//! # Examples
//!
//! ```rust,no_run
//! use nodespace_core::db::{NodeStore, TursoStore, DatabaseService};
//! use std::sync::Arc;
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create database service
//!     let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
//!
//!     // Wrap in NodeStore trait
//!     let store: Arc<dyn NodeStore> = Arc::new(TursoStore::new(db));
//!
//!     // Use abstraction layer
//!     let node = store.get_node("node-123").await?;
//!
//!     Ok(())
//! }
//! ```

use crate::db::node_store::NodeStore;
use crate::db::{DatabaseService, DbCreateNodeParams, DbUpdateNodeParams};
use crate::models::{DeleteResult, Node, NodeQuery, NodeUpdate};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDateTime, Utc};
use libsql::Row;
use serde_json::Value;
use std::sync::Arc;

/// TursoStore implements NodeStore trait for Turso/libsql backend
///
/// This is a thin wrapper around DatabaseService that provides the
/// NodeStore trait abstraction for Phase 1 of the SurrealDB migration.
pub struct TursoStore {
    /// Underlying database service (extracted SQL operations)
    db: Arc<DatabaseService>,
}

impl TursoStore {
    /// Create a new TursoStore wrapper
    ///
    /// # Arguments
    ///
    /// * `db` - Arc to DatabaseService with extracted SQL operations
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::{TursoStore, DatabaseService};
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = Arc::new(DatabaseService::new(PathBuf::from("./test.db")).await?);
    /// let store = TursoStore::new(db);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(db: Arc<DatabaseService>) -> Self {
        Self { db }
    }

    /// Parse timestamp from database - handles both SQLite and RFC3339 formats
    ///
    /// SQLite CURRENT_TIMESTAMP returns: "YYYY-MM-DD HH:MM:SS"
    /// Old data might use RFC3339: "YYYY-MM-DDTHH:MM:SSZ"
    fn parse_timestamp(s: &str) -> Result<DateTime<Utc>> {
        // Try SQLite format first: "YYYY-MM-DD HH:MM:SS"
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Ok(naive.and_utc());
        }

        // Try RFC3339 format (for old data): "YYYY-MM-DDTHH:MM:SSZ"
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Ok(dt.with_timezone(&Utc));
        }

        Err(anyhow::anyhow!(
            "Unable to parse timestamp '{}' as SQLite or RFC3339 format",
            s
        ))
    }

    /// Convert libsql::Row to Node model
    ///
    /// Handles all field conversions including optional fields and JSON properties.
    /// This is the central conversion point for all query operations.
    ///
    /// # Row Format
    ///
    /// Expected columns (in order):
    /// - id (TEXT)
    /// - node_type (TEXT)
    /// - content (TEXT)
    /// - parent_id (TEXT, nullable)
    /// - container_node_id (TEXT, nullable)
    /// - before_sibling_id (TEXT, nullable)
    /// - version (INTEGER)
    /// - created_at (TEXT, ISO 8601)
    /// - modified_at (TEXT, ISO 8601)
    /// - properties (TEXT, JSON)
    /// - embedding_vector (BLOB, nullable)
    fn row_to_node(row: &Row) -> Result<Node> {
        let id: String = row.get(0).context("Failed to get id")?;
        let node_type: String = row.get(1).context("Failed to get node_type")?;
        let content: String = row.get(2).context("Failed to get content")?;
        let parent_id: Option<String> = row.get(3).context("Failed to get parent_id")?;
        let container_node_id: Option<String> =
            row.get(4).context("Failed to get container_node_id")?;
        let before_sibling_id: Option<String> =
            row.get(5).context("Failed to get before_sibling_id")?;
        let version: i64 = row.get(6).context("Failed to get version")?;
        let created_at_str: String = row.get(7).context("Failed to get created_at")?;
        let modified_at_str: String = row.get(8).context("Failed to get modified_at")?;
        let properties_json: String = row.get(9).context("Failed to get properties")?;
        let embedding_vector: Option<Vec<u8>> =
            row.get(10).context("Failed to get embedding_vector")?;

        // Parse timestamps - handles both SQLite and RFC3339 formats
        let created_at =
            Self::parse_timestamp(&created_at_str).context("Failed to parse created_at")?;
        let modified_at =
            Self::parse_timestamp(&modified_at_str).context("Failed to parse modified_at")?;

        // Parse properties JSON
        let properties: Value =
            serde_json::from_str(&properties_json).context("Failed to parse properties JSON")?;

        Ok(Node {
            id,
            node_type,
            content,
            parent_id,
            container_node_id,
            before_sibling_id,
            version,
            created_at,
            modified_at,
            properties,
            embedding_vector,
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
        })
    }
}

#[async_trait]
impl NodeStore for TursoStore {
    async fn create_node(&self, node: Node) -> Result<Node> {
        // Serialize properties to JSON
        let properties_json =
            serde_json::to_string(&node.properties).context("Failed to serialize properties")?;

        // Delegate to DatabaseService
        let params = DbCreateNodeParams {
            id: &node.id,
            node_type: &node.node_type,
            content: &node.content,
            parent_id: node.parent_id.as_deref(),
            container_node_id: node.container_node_id.as_deref(),
            before_sibling_id: node.before_sibling_id.as_deref(),
            properties: &properties_json,
            embedding_vector: node.embedding_vector.as_deref(),
        };

        self.db
            .db_create_node(params)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create node: {}", e))?;

        // Fetch and return the created node
        self.get_node(&node.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after creation"))
    }

    async fn get_node(&self, id: &str) -> Result<Option<Node>> {
        match self
            .db
            .db_get_node(id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get node: {}", e))?
        {
            Some(row) => Ok(Some(Self::row_to_node(&row)?)),
            None => Ok(None),
        }
    }

    async fn update_node(&self, id: &str, update: NodeUpdate) -> Result<Node> {
        // Fetch current node to build update
        let current = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", id))?;

        // Determine if content changed BEFORE consuming update.content
        let content_changed = update.content.is_some();

        // Store old_container_id BEFORE moving current.container_node_id
        let old_container_id = current.container_node_id.clone();

        // Apply updates to current node
        let updated_node = Node {
            id: current.id.clone(),
            node_type: update.node_type.unwrap_or(current.node_type),
            content: update.content.unwrap_or(current.content),
            parent_id: match update.parent_id {
                None => current.parent_id,
                Some(new_parent) => new_parent,
            },
            container_node_id: match update.container_node_id {
                None => old_container_id.clone(),
                Some(new_container) => new_container,
            },
            before_sibling_id: match update.before_sibling_id {
                None => current.before_sibling_id,
                Some(new_sibling) => new_sibling,
            },
            version: current.version, // Will be incremented by database
            created_at: current.created_at,
            modified_at: Utc::now(),
            properties: update.properties.unwrap_or(current.properties),
            embedding_vector: match update.embedding_vector {
                None => current.embedding_vector,
                Some(new_embedding) => new_embedding,
            },
            mentions: current.mentions,
            mentioned_by: current.mentioned_by,
        };

        // Serialize properties
        let properties_json = serde_json::to_string(&updated_node.properties)
            .context("Failed to serialize properties")?;

        // Determine if this is a container node and track container changes
        let is_container = updated_node.container_node_id.is_none();

        // Delegate to DatabaseService
        let params = DbUpdateNodeParams {
            id,
            node_type: &updated_node.node_type,
            content: &updated_node.content,
            parent_id: updated_node.parent_id.as_deref(),
            container_node_id: updated_node.container_node_id.as_deref(),
            before_sibling_id: updated_node.before_sibling_id.as_deref(),
            properties: &properties_json,
            embedding_vector: updated_node.embedding_vector.as_deref(),
            content_changed,
            is_container,
            old_container_id: old_container_id.as_deref(),
            new_container_id: updated_node.container_node_id.as_deref(),
        };

        self.db
            .db_update_node(params)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to update node: {}", e))?;

        // Fetch and return updated node
        self.get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after update"))
    }

    async fn delete_node(&self, id: &str) -> Result<DeleteResult> {
        let rows_affected = self
            .db
            .db_delete_node(id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to delete node: {}", e))?;

        Ok(DeleteResult {
            existed: rows_affected > 0,
        })
    }

    async fn query_nodes(&self, query: NodeQuery) -> Result<Vec<Node>> {
        // Convert NodeQuery to individual parameters for db_query_nodes
        // db_query_nodes expects: node_type, parent_id, container_node_id, order_clause, limit_clause

        // Build order clause (default to empty string for no ordering)
        let order_clause = ""; // NodeQuery doesn't have order_by field, use default

        // Build limit clause
        let limit_clause = query
            .limit
            .map(|l| format!(" LIMIT {}", l))
            .unwrap_or_default();

        let mut rows = self
            .db
            .db_query_nodes(
                query.node_type.as_deref(),
                None, // parent_id - NodeQuery doesn't have this field
                None, // container_node_id - NodeQuery doesn't have this field
                order_clause,
                &limit_clause,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to query nodes: {}", e))?;

        let mut nodes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch row: {}", e))?
        {
            nodes.push(Self::row_to_node(&row)?);
        }

        Ok(nodes)
    }

    async fn get_children(&self, parent_id: Option<&str>) -> Result<Vec<Node>> {
        // db_get_children takes &str, not Option<&str>
        // For None (root nodes), we need to use a different query approach
        let mut nodes = Vec::new();

        if let Some(parent_id) = parent_id {
            // Get children of specific parent
            let mut rows = self
                .db
                .db_get_children(parent_id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get children: {}", e))?;

            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to fetch row: {}", e))?
            {
                nodes.push(Self::row_to_node(&row)?);
            }
        } else {
            // Get root nodes (nodes with parent_id IS NULL)
            // Use db_query_nodes with parent_id filter
            let mut rows = self
                .db
                .db_query_nodes(None, None, None, "", "")
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get root nodes: {}", e))?;

            // Filter for nodes with parent_id IS NULL
            while let Some(row) = rows
                .next()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to fetch row: {}", e))?
            {
                let node = Self::row_to_node(&row)?;
                if node.parent_id.is_none() {
                    nodes.push(node);
                }
            }
        }

        Ok(nodes)
    }

    async fn get_nodes_by_container(&self, container_id: &str) -> Result<Vec<Node>> {
        let mut rows = self
            .db
            .db_get_nodes_by_container(container_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get nodes by container: {}", e))?;

        let mut nodes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch row: {}", e))?
        {
            nodes.push(Self::row_to_node(&row)?);
        }

        Ok(nodes)
    }

    async fn search_nodes_by_content(&self, query: &str, limit: Option<i64>) -> Result<Vec<Node>> {
        // db_search_nodes_by_content expects: content_pattern, node_type, container_task_filter, limit_clause
        let content_pattern = format!("%{}%", query);
        let limit_clause = limit.map(|l| format!(" LIMIT {}", l)).unwrap_or_default();

        let mut rows = self
            .db
            .db_search_nodes_by_content(
                &content_pattern,
                None, // node_type - no filter
                "",   // container_task_filter - no filter
                &limit_clause,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Failed to search nodes: {}", e))?;

        let mut nodes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch row: {}", e))?
        {
            nodes.push(Self::row_to_node(&row)?);
        }

        Ok(nodes)
    }

    async fn move_node(&self, id: &str, new_parent_id: Option<&str>) -> Result<()> {
        self.db
            .db_move_node(id, new_parent_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to move node: {}", e))?;

        Ok(())
    }

    async fn reorder_node(&self, id: &str, new_before_sibling_id: Option<&str>) -> Result<()> {
        self.db
            .db_reorder_node(id, new_before_sibling_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to reorder node: {}", e))?;

        Ok(())
    }

    async fn create_mention(
        &self,
        source_id: &str,
        target_id: &str,
        _container_id: &str, // Ignored - db_create_mention doesn't use container_id
    ) -> Result<()> {
        self.db
            .db_create_mention(source_id, target_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create mention: {}", e))?;

        Ok(())
    }

    async fn delete_mention(&self, source_id: &str, target_id: &str) -> Result<()> {
        self.db
            .db_delete_mention(source_id, target_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to delete mention: {}", e))?;

        Ok(())
    }

    async fn get_outgoing_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        self.db
            .db_get_outgoing_mentions(node_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get outgoing mentions: {}", e))
    }

    async fn get_incoming_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        self.db
            .db_get_incoming_mentions(node_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get incoming mentions: {}", e))
    }

    async fn get_mentioning_containers(&self, node_id: &str) -> Result<Vec<Node>> {
        let container_ids = self
            .db
            .db_get_mentioning_containers(node_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get mentioning containers: {}", e))?;

        // Fetch full node details for each container ID
        let mut nodes = Vec::new();
        for container_id in container_ids {
            if let Some(node) = self.get_node(&container_id).await? {
                nodes.push(node);
            }
        }

        Ok(nodes)
    }

    async fn get_schema(&self, node_type: &str) -> Result<Option<Value>> {
        self.db
            .db_get_schema(node_type)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get schema: {}", e))
    }

    async fn update_schema(&self, node_type: &str, schema: &Value) -> Result<()> {
        // db_update_schema expects: node_type (id), schema_name (content), properties (JSON string)
        // The schema Value contains the full schema, we serialize it to JSON
        let properties_json =
            serde_json::to_string(schema).context("Failed to serialize schema")?;

        // Use node_type as both ID and human-readable name
        // This matches the existing behavior where schema nodes use the node type as both
        self.db
            .db_update_schema(node_type, node_type, &properties_json)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to update schema: {}", e))?;

        Ok(())
    }

    async fn get_nodes_without_embeddings(&self, limit: Option<i64>) -> Result<Vec<Node>> {
        // Convert Option<i64> to usize, defaulting to a reasonable limit if None
        let limit_usize = limit.unwrap_or(100) as usize;

        let node_ids = self
            .db
            .db_get_nodes_without_embeddings(limit_usize)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get nodes without embeddings: {}", e))?;

        // Fetch full nodes for each ID
        let mut nodes = Vec::new();
        for id in node_ids {
            if let Some(node) = self.get_node(&id).await? {
                nodes.push(node);
            }
        }

        Ok(nodes)
    }

    async fn update_embedding(&self, node_id: &str, embedding: &[u8]) -> Result<()> {
        self.db
            .db_update_embedding(node_id, embedding)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to update embedding: {}", e))?;

        Ok(())
    }

    async fn search_by_embedding(&self, embedding: &[u8], limit: i64) -> Result<Vec<(Node, f64)>> {
        // db_search_by_embedding expects: query_blob, threshold, limit
        // Use a permissive threshold (1.0 = accept all) since NodeStore trait doesn't have threshold
        let threshold = 1.0_f32;
        let limit_usize = limit as usize;

        let mut rows = self
            .db
            .db_search_by_embedding(embedding, threshold, limit_usize)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to search by embedding: {}", e))?;

        let mut results = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch row: {}", e))?
        {
            // Row format: [...node fields..., distance]
            // distance is in the last column (index 11)
            let distance: f64 = row.get(11).context("Failed to get distance")?;

            // Convert to similarity score (1.0 - distance for cosine distance)
            let similarity = 1.0 - distance;

            // Convert row to node (first 11 columns)
            let node = Self::row_to_node(&row)?;

            results.push((node, similarity));
        }

        Ok(results)
    }

    async fn batch_create_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Node>> {
        // Convert nodes to DbCreateNodeParams and track IDs
        let mut params_vec = Vec::new();
        let mut properties_strings = Vec::new();

        for node in &nodes {
            let properties_json = serde_json::to_string(&node.properties)
                .context("Failed to serialize properties")?;
            properties_strings.push(properties_json);
        }

        for (i, node) in nodes.iter().enumerate() {
            params_vec.push(DbCreateNodeParams {
                id: &node.id,
                node_type: &node.node_type,
                content: &node.content,
                parent_id: node.parent_id.as_deref(),
                container_node_id: node.container_node_id.as_deref(),
                before_sibling_id: node.before_sibling_id.as_deref(),
                properties: &properties_strings[i],
                embedding_vector: node.embedding_vector.as_deref(),
            });
        }

        // Create nodes and get their IDs
        let created_ids = self
            .db
            .db_batch_create_nodes(params_vec)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to batch create nodes: {}", e))?;

        // Fetch and return created nodes
        let mut created_nodes = Vec::new();
        for id in created_ids {
            if let Some(node) = self.get_node(&id).await? {
                created_nodes.push(node);
            }
        }

        Ok(created_nodes)
    }

    async fn close(&self) -> Result<()> {
        self.db
            .db_close()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to close database: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Node;
    use serde_json::json;
    use tempfile::TempDir;

    async fn create_test_store() -> Result<(TursoStore, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        let db = Arc::new(DatabaseService::new(db_path).await?);
        Ok((TursoStore::new(db), temp_dir))
    }

    #[tokio::test]
    async fn test_create_and_get_node() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let node = Node::new(
            "text".to_string(),
            "Test content".to_string(),
            None,
            json!({}),
        );

        let created = store.create_node(node.clone()).await?;
        assert_eq!(created.id, node.id);
        assert_eq!(created.content, "Test content");

        let fetched = store.get_node(&node.id).await?;
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().id, node.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_update_node() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let node = Node::new(
            "text".to_string(),
            "Original content".to_string(),
            None,
            json!({}),
        );

        let created = store.create_node(node.clone()).await?;

        let update = NodeUpdate {
            content: Some("Updated content".to_string()),
            ..Default::default()
        };

        let updated = store.update_node(&created.id, update).await?;
        assert_eq!(updated.content, "Updated content");

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_node() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let node = Node::new(
            "text".to_string(),
            "Test content".to_string(),
            None,
            json!({}),
        );

        let created = store.create_node(node.clone()).await?;

        let result = store.delete_node(&created.id).await?;
        assert!(result.existed);

        let fetched = store.get_node(&created.id).await?;
        assert!(fetched.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_get_children() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let parent = Node::new("text".to_string(), "Parent".to_string(), None, json!({}));
        let parent = store.create_node(parent).await?;

        let child1 = Node::new(
            "text".to_string(),
            "Child 1".to_string(),
            Some(parent.id.clone()),
            json!({}),
        );
        let child2 = Node::new(
            "text".to_string(),
            "Child 2".to_string(),
            Some(parent.id.clone()),
            json!({}),
        );

        store.create_node(child1).await?;
        store.create_node(child2).await?;

        let children = store.get_children(Some(&parent.id)).await?;
        assert_eq!(children.len(), 2);

        Ok(())
    }

    #[tokio::test]
    async fn test_mentions() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let source = Node::new("text".to_string(), "Source".to_string(), None, json!({}));
        let target = Node::new("text".to_string(), "Target".to_string(), None, json!({}));
        let container = Node::new("text".to_string(), "Container".to_string(), None, json!({}));

        let source = store.create_node(source).await?;
        let target = store.create_node(target).await?;
        let container = store.create_node(container).await?;

        store
            .create_mention(&source.id, &target.id, &container.id)
            .await?;

        let outgoing = store.get_outgoing_mentions(&source.id).await?;
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0], target.id);

        let incoming = store.get_incoming_mentions(&target.id).await?;
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0], source.id);

        store.delete_mention(&source.id, &target.id).await?;

        let outgoing = store.get_outgoing_mentions(&source.id).await?;
        assert_eq!(outgoing.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_schema_operations() -> Result<()> {
        let (store, _temp_dir) = create_test_store().await?;

        let schema = json!({
            "type": "object",
            "properties": {
                "status": {"type": "string"}
            }
        });

        store.update_schema("task", &schema).await?;

        let fetched = store.get_schema("task").await?;
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap(), schema);

        Ok(())
    }
}
