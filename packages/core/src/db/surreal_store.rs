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

use crate::models::{DeleteResult, Node, NodeQuery, NodeUpdate};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use surrealdb::engine::local::{Db, RocksDb};
use surrealdb::sql::{Id, Thing};
use surrealdb::Surreal;

/// Internal struct matching SurrealDB's schema with 'uuid' field
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SurrealNode {
    uuid: String,
    node_type: String,
    content: String,
    parent_id: Option<String>,
    container_node_id: Option<String>,
    before_sibling_id: Option<String>,
    version: i64,
    created_at: String,
    modified_at: String,
    properties: Value,
    embedding_vector: Option<Vec<u8>>,
    #[serde(default)]
    mentions: Vec<String>,
    #[serde(default)]
    mentioned_by: Vec<String>,
}

impl From<SurrealNode> for Node {
    fn from(sn: SurrealNode) -> Self {
        Node {
            id: sn.uuid,
            node_type: sn.node_type,
            content: sn.content,
            parent_id: sn.parent_id,
            container_node_id: sn.container_node_id,
            before_sibling_id: sn.before_sibling_id,
            version: sn.version,
            created_at: DateTime::parse_from_rfc3339(&sn.created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            modified_at: DateTime::parse_from_rfc3339(&sn.modified_at)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
            properties: sn.properties,
            embedding_vector: sn.embedding_vector,
            mentions: sn.mentions,
            mentioned_by: sn.mentioned_by,
        }
    }
}

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

impl SurrealStore {
    pub async fn create_node(&self, node: Node) -> Result<Node> {
        // Generate SurrealDB Record ID
        let record_id = Self::to_record_id(&node.node_type, &node.id);

        // Insert into universal nodes table
        let query = "
            CREATE type::thing($table, $id) CONTENT {
                uuid: $uuid,
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
            .bind(("table", "nodes"))
            .bind(("id", record_id.clone()))
            .bind(("uuid", node.id.clone()))
            .bind(("node_type", node.node_type.clone()))
            .bind(("content", node.content.clone()))
            .bind(("parent_id", node.parent_id.clone()))
            .bind(("container_node_id", node.container_node_id.clone()))
            .bind(("before_sibling_id", node.before_sibling_id.clone()))
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
                obj.insert("uuid".to_string(), serde_json::json!(node.id));
            }

            self.db
                .query("CREATE type::thing($table, $id) CONTENT $properties;")
                .bind(("table", node.node_type.clone()))
                .bind(("id", node.id.clone()))
                .bind(("properties", props))
                .await
                .context("Failed to create node in type-specific table")?;
        }

        // Return the created node directly (avoids triggering backfill/migration during creation)
        // The node was successfully created with the values provided, so we can return it as-is
        Ok(node)
    }

    pub async fn get_node(&self, id: &str) -> Result<Option<Node>> {
        // Query by UUID field
        let query = "SELECT * FROM nodes WHERE uuid = $uuid LIMIT 1;";
        let mut response = self
            .db
            .query(query)
            .bind(("uuid", id.to_string()))
            .await
            .context("Failed to query node by UUID")?;

        let surreal_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract query results")?;

        Ok(surreal_nodes.into_iter().map(Into::into).next())
    }

    pub async fn update_node(&self, id: &str, update: NodeUpdate) -> Result<Node> {
        // Fetch current node
        let current = self
            .get_node(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", id))?;

        // Build update query for universal table by UUID
        let query = "
            UPDATE nodes SET
                content = $content,
                node_type = $node_type,
                parent_id = $parent_id,
                container_node_id = $container_node_id,
                before_sibling_id = $before_sibling_id,
                modified_at = time::now(),
                version = version + 1,
                properties = $properties,
                embedding_vector = $embedding_vector
            WHERE uuid = $uuid;
        ";

        let updated_content = update.content.unwrap_or(current.content);
        let updated_node_type = update.node_type.unwrap_or(current.node_type.clone());
        let updated_properties = update.properties.unwrap_or(current.properties.clone());

        self.db
            .query(query)
            .bind(("uuid", id.to_string()))
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

    pub async fn delete_node(&self, id: &str) -> Result<DeleteResult> {
        // Get node to determine type for Record ID
        let node = match self.get_node(id).await? {
            Some(n) => n,
            None => return Ok(DeleteResult { existed: false }),
        };

        // Use transaction for atomicity (all or nothing)
        let transaction_query = "
            BEGIN TRANSACTION;
            DELETE type::thing($table, $id);
            DELETE FROM nodes WHERE uuid = $uuid;
            DELETE mentions WHERE in.uuid = $uuid OR out.uuid = $uuid;
            COMMIT TRANSACTION;
        ";

        self.db
            .query(transaction_query)
            .bind(("table", node.node_type.clone()))
            .bind(("id", node.id.clone()))
            .bind(("uuid", node.id.clone()))
            .await
            .context("Failed to delete node and relations")?;

        Ok(DeleteResult { existed: true })
    }

    /// Delete a node with version check (optimistic locking)
    ///
    /// Only deletes the node if its version matches the expected version.
    /// Returns the number of rows affected (0 if version mismatch, 1 if deleted).
    pub async fn delete_with_version_check(
        &self,
        id: &str,
        expected_version: i64,
    ) -> Result<usize> {
        // First get the node to check version
        let node = match self.get_node(id).await? {
            Some(n) => n,
            None => return Ok(0), // Node doesn't exist
        };

        // Check version match
        if node.version != expected_version {
            return Ok(0); // Version mismatch, no deletion
        }

        // Version matches, proceed with deletion
        let result = self.delete_node(id).await?;
        Ok(if result.existed { 1 } else { 0 })
    }

    pub async fn query_nodes(&self, query: NodeQuery) -> Result<Vec<Node>> {
        // Handle mentioned_by query using graph traversal
        if let Some(ref mentioned_node_id) = query.mentioned_by {
            // Get the mentioned node to construct proper Record ID
            let mentioned_node = self
                .get_node(mentioned_node_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Target node not found: {}", mentioned_node_id))?;

            let record_id = Self::to_record_id(&mentioned_node.node_type, &mentioned_node.id);
            let thing = Thing::from(("nodes", Id::String(record_id)));

            // Query nodes that have mentions pointing to this node
            let sql = if query.limit.is_some() {
                "SELECT VALUE in FROM mentions WHERE out = $target_thing LIMIT $limit;"
            } else {
                "SELECT VALUE in FROM mentions WHERE out = $target_thing;"
            };

            let mut query_builder = self.db.query(sql).bind(("target_thing", thing));

            if let Some(limit) = query.limit {
                query_builder = query_builder.bind(("limit", limit));
            }

            let mut response = query_builder
                .await
                .context("Failed to query mentioned_by nodes")?;

            let source_things: Vec<Thing> = response
                .take(0)
                .context("Failed to extract source nodes from mentions")?;

            // Fetch full node records for each source
            let mut nodes = Vec::new();
            for thing in source_things {
                if let Id::String(id_str) = &thing.id {
                    // Extract UUID from "node_type:uuid" format
                    if let Some(uuid) = id_str.split(':').nth(1) {
                        if let Some(node) = self.get_node(uuid).await? {
                            nodes.push(node);
                        }
                    }
                }
            }

            // Apply include_containers_and_tasks filter if specified
            if let Some(true) = query.include_containers_and_tasks {
                nodes.retain(|n| n.node_type == "task" || n.container_node_id.is_none());
            }

            return Ok(nodes);
        }

        // Handle content_contains query
        if let Some(ref search_query) = query.content_contains {
            let mut nodes = self
                .search_nodes_by_content(search_query, query.limit.map(|l| l as i64))
                .await?;

            // Apply include_containers_and_tasks filter if specified
            if let Some(true) = query.include_containers_and_tasks {
                nodes.retain(|n| n.node_type == "task" || n.container_node_id.is_none());
            }

            return Ok(nodes);
        }

        // Build WHERE clause conditions
        let mut conditions = Vec::new();

        if query.node_type.is_some() {
            conditions.push("node_type = $node_type".to_string());
        }

        if let Some(true) = query.include_containers_and_tasks {
            // Include tasks OR nodes without container (top-level/containers)
            conditions.push("(node_type = 'task' OR container_node_id IS NONE)".to_string());
        }

        // Build SQL query
        let where_clause = if !conditions.is_empty() {
            Some(conditions.join(" AND "))
        } else {
            None
        };

        let sql = match (&where_clause, query.limit) {
            (None, None) => "SELECT * FROM nodes;".to_string(),
            (None, Some(_)) => "SELECT * FROM nodes LIMIT $limit;".to_string(),
            (Some(clause), None) => format!("SELECT * FROM nodes WHERE {};", clause),
            (Some(clause), Some(_)) => {
                format!("SELECT * FROM nodes WHERE {} LIMIT $limit;", clause)
            }
        };

        let mut query_builder = self.db.query(sql);

        if let Some(node_type) = &query.node_type {
            query_builder = query_builder.bind(("node_type", node_type.clone()));
        }

        if let Some(limit) = query.limit {
            query_builder = query_builder.bind(("limit", limit));
        }

        let mut response = query_builder.await.context("Failed to query nodes")?;
        let surreal_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract nodes from query response")?;
        Ok(surreal_nodes.into_iter().map(Into::into).collect())
    }

    pub async fn get_children(&self, parent_id: Option<&str>) -> Result<Vec<Node>> {
        let (query, has_parent) = if parent_id.is_some() {
            ("SELECT * FROM nodes WHERE parent_id = $parent_id;", true)
        } else {
            ("SELECT * FROM nodes WHERE parent_id IS NONE;", false)
        };

        let mut query_builder = self.db.query(query);

        if has_parent {
            query_builder = query_builder.bind(("parent_id", parent_id.unwrap().to_string()));
        }

        let mut response = query_builder.await.context("Failed to get children")?;
        let surreal_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract children from response")?;
        Ok(surreal_nodes.into_iter().map(Into::into).collect())
    }

    pub async fn get_nodes_by_container(&self, container_id: &str) -> Result<Vec<Node>> {
        let query = "SELECT * FROM nodes WHERE container_node_id = $container_id;";
        let mut response = self
            .db
            .query(query)
            .bind(("container_id", container_id.to_string()))
            .await
            .context("Failed to get nodes by container")?;

        let surreal_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract nodes by container from response")?;
        Ok(surreal_nodes.into_iter().map(Into::into).collect())
    }

    pub async fn search_nodes_by_content(
        &self,
        search_query: &str,
        limit: Option<i64>,
    ) -> Result<Vec<Node>> {
        let sql = if limit.is_some() {
            "SELECT * FROM nodes WHERE content CONTAINS $search_query LIMIT $limit;"
        } else {
            "SELECT * FROM nodes WHERE content CONTAINS $search_query;"
        };

        let mut query_builder = self
            .db
            .query(sql)
            .bind(("search_query", search_query.to_string()));

        if let Some(lim) = limit {
            query_builder = query_builder.bind(("limit", lim));
        }

        let mut response = query_builder.await.context("Failed to search nodes")?;
        let surreal_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract search results from response")?;
        Ok(surreal_nodes.into_iter().map(Into::into).collect())
    }

    pub async fn move_node(&self, id: &str, new_parent_id: Option<&str>) -> Result<()> {
        self.db
            .query("UPDATE nodes SET parent_id = $parent_id WHERE uuid = $uuid;")
            .bind(("uuid", id.to_string()))
            .bind(("parent_id", new_parent_id.map(|s| s.to_string())))
            .await
            .context("Failed to move node")?;

        Ok(())
    }

    pub async fn reorder_node(&self, id: &str, new_before_sibling_id: Option<&str>) -> Result<()> {
        self.db
            .query("UPDATE nodes SET before_sibling_id = $before_sibling_id WHERE uuid = $uuid;")
            .bind(("uuid", id.to_string()))
            .bind((
                "before_sibling_id",
                new_before_sibling_id.map(|s| s.to_string()),
            ))
            .await
            .context("Failed to reorder node")?;

        Ok(())
    }

    pub async fn create_mention(
        &self,
        source_id: &str,
        target_id: &str,
        container_id: &str,
    ) -> Result<()> {
        // Get node types to construct proper Record IDs
        let source_node = self
            .get_node(source_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Source node not found: {}", source_id))?;
        let target_node = self
            .get_node(target_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Target node not found: {}", target_id))?;

        // Construct Thing objects for proper Record ID binding
        let source_record_id = Self::to_record_id(&source_node.node_type, &source_node.id);
        let target_record_id = Self::to_record_id(&target_node.node_type, &target_node.id);

        let source_thing = Thing::from(("nodes", Id::String(source_record_id)));
        let target_thing = Thing::from(("nodes", Id::String(target_record_id)));

        // Check if mention already exists (for idempotency)
        let check_query = "SELECT VALUE id FROM mentions WHERE in = $source AND out = $target;";
        let mut check_response = self
            .db
            .query(check_query)
            .bind(("source", source_thing.clone()))
            .bind(("target", target_thing.clone()))
            .await
            .context("Failed to check for existing mention")?;

        let existing_mention_ids: Vec<Thing> = check_response
            .take(0)
            .context("Failed to extract mention check results")?;

        // Only create mention if it doesn't exist
        if existing_mention_ids.is_empty() {
            // RELATE statement using Thing objects
            let query =
                "RELATE $source->mentions->$target CONTENT { container_id: $container_id };";

            self.db
                .query(query)
                .bind(("source", source_thing))
                .bind(("target", target_thing))
                .bind(("container_id", container_id.to_string()))
                .await
                .context("Failed to create mention")?;
        }

        Ok(())
    }

    pub async fn delete_mention(&self, source_id: &str, target_id: &str) -> Result<()> {
        // Get node types to construct proper Record IDs
        let source_node = self
            .get_node(source_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Source node not found: {}", source_id))?;
        let target_node = self
            .get_node(target_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Target node not found: {}", target_id))?;

        // Construct Thing objects for proper Record ID binding
        let source_record_id = Self::to_record_id(&source_node.node_type, &source_node.id);
        let target_record_id = Self::to_record_id(&target_node.node_type, &target_node.id);

        let source_thing = Thing::from(("nodes", Id::String(source_record_id)));
        let target_thing = Thing::from(("nodes", Id::String(target_record_id)));

        self.db
            .query("DELETE FROM mentions WHERE in = $source AND out = $target;")
            .bind(("source", source_thing))
            .bind(("target", target_thing))
            .await
            .context("Failed to delete mention")?;

        Ok(())
    }

    pub async fn get_outgoing_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        // Get node type to construct proper Record ID
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", node_id))?;

        // Construct Thing for proper Record ID binding
        let record_id = Self::to_record_id(&node.node_type, &node.id);
        let thing = Thing::from(("nodes", Id::String(record_id)));

        let query = "SELECT out FROM mentions WHERE in = $node_thing;";
        let mut response = self
            .db
            .query(query)
            .bind(("node_thing", thing))
            .await
            .context("Failed to get outgoing mentions")?;

        #[derive(Debug, Deserialize)]
        struct MentionOut {
            out: Thing,
        }

        let results: Vec<MentionOut> = response
            .take(0)
            .context("Failed to extract outgoing mentions from response")?;

        // Extract UUIDs from Thing Record IDs
        // Thing.id is Id::String("node_type:uuid"), so we need to extract just the UUID part
        Ok(results
            .into_iter()
            .filter_map(|m| {
                if let Id::String(id_str) = &m.out.id {
                    // id_str format: "node_type:uuid", extract UUID (after last colon)
                    id_str.split(':').nth(1).map(String::from)
                } else {
                    None
                }
            })
            .collect())
    }

    pub async fn get_incoming_mentions(&self, node_id: &str) -> Result<Vec<String>> {
        // Get node type to construct proper Record ID
        let node = self
            .get_node(node_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Node not found: {}", node_id))?;

        // Construct Thing for proper Record ID binding
        let record_id = Self::to_record_id(&node.node_type, &node.id);
        let thing = Thing::from(("nodes", Id::String(record_id)));

        let query = "SELECT in FROM mentions WHERE out = $node_thing;";
        let mut response = self
            .db
            .query(query)
            .bind(("node_thing", thing))
            .await
            .context("Failed to get incoming mentions")?;

        #[derive(Debug, Deserialize)]
        struct MentionIn {
            #[serde(rename = "in")]
            in_field: Thing,
        }

        let results: Vec<MentionIn> = response
            .take(0)
            .context("Failed to extract incoming mentions from response")?;

        // Extract UUIDs from Thing Record IDs
        Ok(results
            .into_iter()
            .filter_map(|m| {
                if let Id::String(id_str) = &m.in_field.id {
                    // id_str format: "node_type:uuid", extract UUID (after first colon)
                    id_str.split(':').nth(1).map(String::from)
                } else {
                    None
                }
            })
            .collect())
    }

    pub async fn get_mentioning_containers(&self, node_id: &str) -> Result<Vec<Node>> {
        // Get node type to construct proper Record ID
        // If node doesn't exist, return empty array (not an error)
        let node = match self.get_node(node_id).await? {
            Some(n) => n,
            None => return Ok(Vec::new()),
        };

        // Construct Thing for proper Record ID binding
        let record_id = Self::to_record_id(&node.node_type, &node.id);
        let thing = Thing::from(("nodes", Id::String(record_id)));

        let query = "SELECT container_id FROM mentions WHERE out = $node_thing;";
        let mut response = self
            .db
            .query(query)
            .bind(("node_thing", thing))
            .await
            .context("Failed to get mentioning containers")?;

        #[derive(Debug, Deserialize)]
        struct MentionRecord {
            container_id: String,
        }

        let mention_records: Vec<MentionRecord> = response
            .take(0)
            .context("Failed to extract container IDs from response")?;

        // Deduplicate container IDs
        let mut container_ids: Vec<String> = mention_records
            .into_iter()
            .map(|m| m.container_id)
            .collect();
        container_ids.sort();
        container_ids.dedup();

        // Fetch full node records
        let mut nodes = Vec::new();
        for container_id in container_ids {
            if let Some(node) = self.get_node(&container_id).await? {
                nodes.push(node);
            }
        }

        Ok(nodes)
    }

    pub async fn get_schema(&self, node_type: &str) -> Result<Option<Value>> {
        let schema_id = format!("schema:{}", node_type);
        let node = self.get_node(&schema_id).await?;
        Ok(node.map(|n| n.properties))
    }

    pub async fn update_schema(&self, node_type: &str, schema: &Value) -> Result<()> {
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
            // Create new schema node with deterministic ID
            let node = Node::new_with_id(
                schema_id,
                "schema".to_string(),
                node_type.to_string(),
                None,
                schema.clone(),
            );
            self.create_node(node).await?;
        }

        Ok(())
    }

    pub async fn get_nodes_without_embeddings(&self, limit: Option<i64>) -> Result<Vec<Node>> {
        let sql = if limit.is_some() {
            "SELECT * FROM nodes WHERE embedding_vector IS NONE LIMIT $limit;"
        } else {
            "SELECT * FROM nodes WHERE embedding_vector IS NONE;"
        };

        let mut query_builder = self.db.query(sql);

        if let Some(lim) = limit {
            query_builder = query_builder.bind(("limit", lim));
        }

        let mut response = query_builder
            .await
            .context("Failed to get nodes without embeddings")?;
        let surreal_nodes: Vec<SurrealNode> = response
            .take(0)
            .context("Failed to extract nodes without embeddings from response")?;
        Ok(surreal_nodes.into_iter().map(Into::into).collect())
    }

    pub async fn update_embedding(&self, node_id: &str, embedding: &[u8]) -> Result<()> {
        self.db
            .query("UPDATE nodes SET embedding_vector = $embedding WHERE uuid = $uuid;")
            .bind(("uuid", node_id.to_string()))
            .bind(("embedding", embedding.to_vec()))
            .await
            .context("Failed to update embedding")?;

        Ok(())
    }

    pub fn search_by_embedding(&self, _embedding: &[u8], _limit: i64) -> Result<Vec<(Node, f64)>> {
        // SurrealDB doesn't have built-in vector similarity functions yet
        // This is a placeholder implementation that needs to be enhanced
        // For now, return empty results with a warning
        tracing::warn!("Vector similarity search not yet implemented for SurrealDB");
        Ok(Vec::new())
    }

    pub async fn batch_create_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Node>> {
        let mut created_nodes = Vec::new();

        for node in nodes {
            let created = self.create_node(node).await?;
            created_nodes.push(created);
        }

        Ok(created_nodes)
    }

    pub fn close(&self) -> Result<()> {
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
