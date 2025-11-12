//! SurrealStore - NodeStore Implementation for SurrealDB Backend
//!
//! This module implements the `NodeStore` trait for SurrealDB embedded database,
//! providing the abstraction layer that enables hybrid database architecture.
//!
//! # Phase 2 Architecture
//!
//! SurrealStore uses a **hybrid dual-table architecture**:
//! 1. **Universal `nodes` table** - Common metadata, embeddings, hierarchy
//! 2. **Type-specific tables** - Type-safe schemas per entity (`task`, `text`, etc.)
//!
//! # Design Principles
//!
//! 1. **Embedded RocksDB**: Desktop-only backend using `kv-rocksdb` engine
//! 2. **SCHEMALESS Mode**: Core tables use SCHEMALESS for dynamic properties
//! 3. **Record IDs**: Native SurrealDB format `table:uuid` (type embedded in ID)
//! 4. **Zero Regressions**: Maintains exact behavior of TursoStore
//!
//! # Performance Targets (from PoC)
//!
//! - Startup time: <100ms (PoC: 52ms)
//! - 100K nodes query: <200ms (PoC: 104ms)
//! - Deep pagination: <50ms (PoC: 8.3ms)
//! - Complex queries avg: <300ms (PoC: 211ms)
//!
//! # Examples
//!
//! ```rust,no_run
//! use nodespace_core::db::{NodeStore, SurrealStore};
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Create embedded SurrealDB store
//!     let db_path = PathBuf::from("./data/surreal.db");
//!     let store = SurrealStore::new(db_path).await?;
//!
//!     // Use abstraction layer
//!     let node = store.get_node("task:550e8400-e29b-41d4-a716-446655440000").await?;
//!
//!     Ok(())
//! }
//! ```

use crate::db::node_store::NodeStore;
use crate::models::{DeleteResult, Node, NodeQuery, NodeUpdate};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use surrealdb::engine::local::{Db, RocksDb};
use surrealdb::Surreal;

/// SurrealStore implements NodeStore trait for SurrealDB embedded backend
///
/// Uses RocksDB engine for embedded desktop storage with hybrid dual-table
/// architecture for optimal query performance.
pub struct SurrealStore {
    /// SurrealDB connection (embedded RocksDB)
    db: Arc<Surreal<Db>>,
}

impl SurrealStore {
    /// Create a new SurrealStore with embedded RocksDB backend
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to RocksDB database directory
    ///
    /// # Returns
    ///
    /// Initialized SurrealStore with schema setup complete
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Database path is invalid
    /// - RocksDB initialization fails
    /// - Schema initialization fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use nodespace_core::db::SurrealStore;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let store = SurrealStore::new(PathBuf::from("./data/surreal.db")).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(db_path: PathBuf) -> Result<Self> {
        // Initialize embedded RocksDB
        let db = Surreal::new::<RocksDb>(db_path)
            .await
            .context("Failed to initialize SurrealDB with RocksDB backend")?;

        // Use namespace and database
        db.use_ns("nodespace")
            .use_db("nodes")
            .await
            .context("Failed to set namespace/database")?;

        let db = Arc::new(db);

        // Initialize schema
        Self::initialize_schema(&db).await?;

        Ok(Self { db })
    }

    /// Initialize database schema (universal nodes table + core type tables)
    ///
    /// Creates SCHEMALESS tables for flexible property handling while maintaining
    /// core field structure.
    async fn initialize_schema(db: &Surreal<Db>) -> Result<()> {
        // Universal nodes table - SCHEMALESS for maximum flexibility
        db.query(
            "
            DEFINE TABLE IF NOT EXISTS nodes SCHEMALESS;
            ",
        )
        .await
        .context("Failed to create universal nodes table")?;

        // Core type tables - SCHEMALESS for user extensibility
        let core_types = [
            "task",
            "text",
            "date",
            "header",
            "code_block",
            "quote_block",
            "ordered_list",
        ];

        for node_type in core_types {
            db.query(format!(
                "DEFINE TABLE IF NOT EXISTS {} SCHEMALESS;",
                node_type
            ))
            .await
            .with_context(|| format!("Failed to create {} table", node_type))?;
        }

        // Mentions table for reference graph
        db.query(
            "
            DEFINE TABLE IF NOT EXISTS mentions SCHEMALESS TYPE RELATION;
            ",
        )
        .await
        .context("Failed to create mentions table")?;

        Ok(())
    }

    /// Convert Turso-style ID to SurrealDB Record ID format
    ///
    /// Turso IDs are plain UUIDs. SurrealDB uses `table:uuid` format.
    /// This method extracts the type from the node and constructs the Record ID.
    ///
    /// # Arguments
    ///
    /// * `node_type` - The node type (becomes table name)
    /// * `id` - The UUID portion
    ///
    /// # Returns
    ///
    /// SurrealDB Record ID string: `table:uuid`
    fn to_record_id(node_type: &str, id: &str) -> String {
        format!("{}:{}", node_type, id)
    }

    /// Parse SurrealDB Record ID into (table, uuid) components
    ///
    /// # Arguments
    ///
    /// * `record_id` - SurrealDB Record ID (e.g., "task:uuid")
    ///
    /// # Returns
    ///
    /// Tuple of (table_name, uuid_portion)
    #[allow(dead_code)]
    fn parse_record_id(record_id: &str) -> Result<(String, String)> {
        let parts: Vec<&str> = record_id.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(anyhow::anyhow!(
                "Invalid Record ID format: {}. Expected 'table:uuid'",
                record_id
            ));
        }
        Ok((parts[0].to_string(), parts[1].to_string()))
    }
}

#[async_trait]
impl NodeStore for SurrealStore {
    async fn create_node(&self, node: Node) -> Result<Node> {
        // Generate SurrealDB Record ID
        let record_id = Self::to_record_id(&node.node_type, &node.id);

        // Insert into universal nodes table
        let query = "
            CREATE type::thing($table, $id) CONTENT {
                id: type::thing($table, $id),
                node_type: $node_type,
                content: $content,
                parent_id: $parent_id,
                container_node_id: $container_node_id,
                before_sibling_id: $before_sibling_id,
                version: $version,
                created_at: $created_at,
                modified_at: $modified_at,
                properties: $properties,
                embedding_vector: $embedding_vector
            };
        ";

        self.db
            .query(query)
            .bind(("table", node.node_type.clone()))
            .bind(("id", node.id.clone()))
            .bind(("node_type", node.node_type.clone()))
            .bind(("content", node.content.clone()))
            .bind((
                "parent_id",
                node.parent_id
                    .as_ref()
                    .map(|id| Self::to_record_id(&node.node_type, id)),
            ))
            .bind((
                "container_node_id",
                node.container_node_id
                    .as_ref()
                    .map(|id| Self::to_record_id(&node.node_type, id)),
            ))
            .bind((
                "before_sibling_id",
                node.before_sibling_id
                    .as_ref()
                    .map(|id| Self::to_record_id(&node.node_type, id)),
            ))
            .bind(("version", node.version))
            .bind(("created_at", node.created_at.to_rfc3339()))
            .bind(("modified_at", node.modified_at.to_rfc3339()))
            .bind(("properties", node.properties.clone()))
            .bind(("embedding_vector", node.embedding_vector.clone()))
            .await
            .context("Failed to create node in universal table")?;

        // Insert into type-specific table (if properties exist)
        if !node
            .properties
            .as_object()
            .unwrap_or(&serde_json::Map::new())
            .is_empty()
        {
            let mut props = node.properties.clone();
            if let Some(obj) = props.as_object_mut() {
                obj.insert("id".to_string(), serde_json::json!(record_id));
            }

            self.db
                .query("CREATE type::thing($table, $id) CONTENT $properties;")
                .bind(("table", node.node_type.clone()))
                .bind(("id", node.id.clone()))
                .bind(("properties", props))
                .await
                .context("Failed to create node in type-specific table")?;
        }

        // Return the created node
        self.get_node(&node.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after creation"))
    }

    async fn get_node(&self, id: &str) -> Result<Option<Node>> {
        // Parse ID to determine if it's a Record ID or plain UUID
        let query_id = if id.contains(':') {
            id.to_string()
        } else {
            // For plain UUIDs, we need to query by the ID field
            // This is less efficient but maintains compatibility
            let query = "SELECT * FROM nodes WHERE id CONTAINS $id LIMIT 1;";
            let mut response = self
                .db
                .query(query)
                .bind(("id", id.to_string()))
                .await
                .context("Failed to query node by UUID")?;

            let nodes: Vec<Node> = response.take(0).unwrap_or_default();
            return Ok(nodes.into_iter().next());
        };

        // Query by Record ID
        let query = "SELECT * FROM type::thing($record_id);";
        let mut response = self
            .db
            .query(query)
            .bind(("record_id", query_id))
            .await
            .context("Failed to get node")?;

        let nodes: Vec<Node> = response.take(0).unwrap_or_default();
        Ok(nodes.into_iter().next())
    }

    async fn update_node(&self, id: &str, update: NodeUpdate) -> Result<Node> {
        // Fetch current node
        let current = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", id))?;

        let record_id = Self::to_record_id(&current.node_type, &current.id);

        // Build update query for universal table
        let query = "
            UPDATE type::thing($record_id) SET
                content = $content,
                node_type = $node_type,
                parent_id = $parent_id,
                container_node_id = $container_node_id,
                before_sibling_id = $before_sibling_id,
                modified_at = time::now(),
                version = version + 1,
                properties = $properties,
                embedding_vector = $embedding_vector;
        ";

        let updated_content = update.content.unwrap_or(current.content);
        let updated_node_type = update.node_type.unwrap_or(current.node_type.clone());
        let updated_properties = update.properties.unwrap_or(current.properties.clone());

        self.db
            .query(query)
            .bind(("record_id", record_id.clone()))
            .bind(("content", updated_content))
            .bind(("node_type", updated_node_type))
            .bind(("parent_id", update.parent_id.flatten()))
            .bind(("container_node_id", update.container_node_id.flatten()))
            .bind(("before_sibling_id", update.before_sibling_id.flatten()))
            .bind(("properties", updated_properties))
            .bind(("embedding_vector", update.embedding_vector.flatten()))
            .await
            .context("Failed to update node")?;

        // Fetch and return updated node
        self.get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found after update"))
    }

    async fn delete_node(&self, id: &str) -> Result<DeleteResult> {
        // Get node to determine type for Record ID
        let node = match self.get_node(id).await? {
            Some(n) => n,
            None => return Ok(DeleteResult { existed: false }),
        };

        let record_id = Self::to_record_id(&node.node_type, &node.id);

        // Delete from type-specific table
        self.db
            .query(format!("DELETE {};", record_id))
            .await
            .context("Failed to delete from type table")?;

        // Delete from universal nodes table
        self.db
            .query("DELETE FROM nodes WHERE id = $record_id;")
            .bind(("record_id", record_id.clone()))
            .await
            .context("Failed to delete from nodes table")?;

        // Delete mention relationships
        self.db
            .query("DELETE mentions WHERE in = $record_id OR out = $record_id;")
            .bind(("record_id", record_id))
            .await
            .context("Failed to delete mentions")?;

        Ok(DeleteResult { existed: true })
    }

    async fn query_nodes(&self, query: NodeQuery) -> Result<Vec<Node>> {
        let mut sql = "SELECT * FROM nodes".to_string();
        let mut conditions = Vec::new();

        // Add node_type filter
        if let Some(node_type) = &query.node_type {
            conditions.push(format!("node_type = '{}'", node_type));
        }

        // Build WHERE clause
        if !conditions.is_empty() {
            sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
        }

        // Add limit
        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        sql.push(';');

        let mut response = self.db.query(&sql).await.context("Failed to query nodes")?;
        let nodes: Vec<Node> = response.take(0).unwrap_or_default();
        Ok(nodes)
    }

    async fn get_children(&self, parent_id: Option<&str>) -> Result<Vec<Node>> {
        let query = if let Some(pid) = parent_id {
            format!("SELECT * FROM nodes WHERE parent_id = '{}';", pid)
        } else {
            "SELECT * FROM nodes WHERE parent_id IS NONE;".to_string()
        };

        let mut response = self
            .db
            .query(&query)
            .await
            .context("Failed to get children")?;
        let nodes: Vec<Node> = response.take(0).unwrap_or_default();
        Ok(nodes)
    }

    async fn get_nodes_by_container(&self, container_id: &str) -> Result<Vec<Node>> {
        let query = "SELECT * FROM nodes WHERE container_node_id = $container_id;";
        let mut response = self
            .db
            .query(query)
            .bind(("container_id", container_id.to_string()))
            .await
            .context("Failed to get nodes by container")?;

        let nodes: Vec<Node> = response.take(0).unwrap_or_default();
        Ok(nodes)
    }

    async fn search_nodes_by_content(
        &self,
        search_query: &str,
        limit: Option<i64>,
    ) -> Result<Vec<Node>> {
        let mut query = format!(
            "SELECT * FROM nodes WHERE content CONTAINS '{}'",
            search_query
        );

        if let Some(lim) = limit {
            query.push_str(&format!(" LIMIT {}", lim));
        }
        query.push(';');

        let mut response = self
            .db
            .query(&query)
            .await
            .context("Failed to search nodes")?;
        let nodes: Vec<Node> = response.take(0).unwrap_or_default();
        Ok(nodes)
    }

    async fn move_node(&self, id: &str, new_parent_id: Option<&str>) -> Result<()> {
        let node = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found"))?;

        let record_id = Self::to_record_id(&node.node_type, &node.id);

        self.db
            .query("UPDATE type::thing($record_id) SET parent_id = $parent_id;")
            .bind(("record_id", record_id))
            .bind(("parent_id", new_parent_id.map(|s| s.to_string())))
            .await
            .context("Failed to move node")?;

        Ok(())
    }

    async fn reorder_node(&self, id: &str, new_before_sibling_id: Option<&str>) -> Result<()> {
        let node = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found"))?;

        let record_id = Self::to_record_id(&node.node_type, &node.id);

        self.db
            .query("UPDATE type::thing($record_id) SET before_sibling_id = $before_sibling_id;")
            .bind(("record_id", record_id))
            .bind((
                "before_sibling_id",
                new_before_sibling_id.map(|s| s.to_string()),
            ))
            .await
            .context("Failed to reorder node")?;

        Ok(())
    }

    async fn create_mention(
        &self,
        source_id: &str,
        target_id: &str,
        container_id: &str,
    ) -> Result<()> {
        self.db
            .query("RELATE $source->mentions->$target CONTENT { container_id: $container_id };")
            .bind(("source", source_id.to_string()))
            .bind(("target", target_id.to_string()))
            .bind(("container_id", container_id.to_string()))
            .await
            .context("Failed to create mention")?;

        Ok(())
    }

    async fn delete_mention(&self, source_id: &str, target_id: &str) -> Result<()> {
        self.db
            .query("DELETE FROM mentions WHERE in = $source AND out = $target;")
            .bind(("source", source_id.to_string()))
            .bind(("target", target_id.to_string()))
            .await
            .context("Failed to delete mention")?;

        Ok(())
    }

    async fn get_outgoing_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        let query = "SELECT out FROM mentions WHERE in = $node_id;";
        let mut response = self
            .db
            .query(query)
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to get outgoing mentions")?;

        let results: Vec<Value> = response.take(0).unwrap_or_default();
        Ok(results
            .into_iter()
            .filter_map(|v| v.get("out").and_then(|o| o.as_str().map(String::from)))
            .collect())
    }

    async fn get_incoming_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        let query = "SELECT in FROM mentions WHERE out = $node_id;";
        let mut response = self
            .db
            .query(query)
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to get incoming mentions")?;

        let results: Vec<Value> = response.take(0).unwrap_or_default();
        Ok(results
            .into_iter()
            .filter_map(|v| v.get("in").and_then(|i| i.as_str().map(String::from)))
            .collect())
    }

    async fn get_mentioning_containers(&self, node_id: &str) -> Result<Vec<Node>> {
        let query = "SELECT DISTINCT container_id FROM mentions WHERE out = $node_id;";
        let mut response = self
            .db
            .query(query)
            .bind(("node_id", node_id.to_string()))
            .await
            .context("Failed to get mentioning containers")?;

        let container_ids: Vec<String> = response.take(0).unwrap_or_default();

        let mut nodes = Vec::new();
        for container_id in container_ids {
            if let Some(node) = self.get_node(&container_id).await? {
                nodes.push(node);
            }
        }

        Ok(nodes)
    }

    async fn get_schema(&self, node_type: &str) -> Result<Option<Value>> {
        let schema_id = format!("schema:{}", node_type);
        let node = self.get_node(&schema_id).await?;
        Ok(node.map(|n| n.properties))
    }

    async fn update_schema(&self, node_type: &str, schema: &Value) -> Result<()> {
        let schema_id = format!("schema:{}", node_type);

        // Check if schema node exists
        if self.get_node(&schema_id).await?.is_some() {
            // Update existing schema
            let update = NodeUpdate {
                properties: Some(schema.clone()),
                ..Default::default()
            };
            self.update_node(&schema_id, update).await?;
        } else {
            // Create new schema node
            let node = Node::new(
                "schema".to_string(),
                node_type.to_string(),
                None,
                schema.clone(),
            );
            self.create_node(node).await?;
        }

        Ok(())
    }

    async fn get_nodes_without_embeddings(&self, limit: Option<i64>) -> Result<Vec<Node>> {
        let mut query = "SELECT * FROM nodes WHERE embedding_vector IS NONE".to_string();

        if let Some(lim) = limit {
            query.push_str(&format!(" LIMIT {}", lim));
        }
        query.push(';');

        let mut response = self
            .db
            .query(&query)
            .await
            .context("Failed to get nodes without embeddings")?;
        let nodes: Vec<Node> = response.take(0).unwrap_or_default();
        Ok(nodes)
    }

    async fn update_embedding(&self, node_id: &str, embedding: &[u8]) -> Result<()> {
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found"))?;

        let record_id = Self::to_record_id(&node.node_type, &node.id);

        self.db
            .query("UPDATE type::thing($record_id) SET embedding_vector = $embedding;")
            .bind(("record_id", record_id))
            .bind(("embedding", embedding.to_vec()))
            .await
            .context("Failed to update embedding")?;

        Ok(())
    }

    async fn search_by_embedding(
        &self,
        _embedding: &[u8],
        _limit: i64,
    ) -> Result<Vec<(Node, f64)>> {
        // SurrealDB doesn't have built-in vector similarity functions yet
        // This is a placeholder implementation that needs to be enhanced
        // For now, return empty results with a warning
        tracing::warn!("Vector similarity search not yet implemented for SurrealDB");
        Ok(Vec::new())
    }

    async fn batch_create_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Node>> {
        let mut created_nodes = Vec::new();

        for node in nodes {
            let created = self.create_node(node).await?;
            created_nodes.push(created);
        }

        Ok(created_nodes)
    }

    async fn close(&self) -> Result<()> {
        // SurrealDB handles cleanup automatically on drop
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn create_test_store() -> Result<(SurrealStore, TempDir)> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test_surreal.db");
        let store = SurrealStore::new(db_path).await?;
        Ok((store, temp_dir))
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
