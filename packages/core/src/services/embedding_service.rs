//! Node Embedding Service
//!
//! This module provides embedding generation for nodes with adaptive chunking
//! based on content size. It integrates with the NLP engine and Turso's native
//! vector search capabilities.
//!
//! # Features
//!
//! - Adaptive chunking strategy based on token count (< 512, 512-2048, > 2048)
//! - Integration with NLP engine (BAAI/bge-small-en-v1.5, 384 dimensions)
//! - Native Turso vector search with DiskANN indexing
//! - Stale flag tracking for efficient re-embedding
//! - Metadata tracking for embeddings
//!
//! # Architecture
//!
//! Container nodes (@ mention pages, date nodes) are the primary semantic search targets.
//! Embeddings are stored in the `embedding_vector` column as F32_BLOB(384).
//!
//! # Chunking Strategies
//!
//! - **< 512 tokens**: Single embedding for entire container
//! - **512-2048 tokens**: Summary embedding + top-level section embeddings
//! - **> 2048 tokens**: Summary embedding + hierarchical section embeddings

use crate::db::DatabaseService;
use crate::models::Node;
use crate::services::error::NodeServiceError;
use chrono::Utc;
use libsql::params;
use nodespace_nlp_engine::EmbeddingService;
use serde_json::json;
use std::sync::Arc;
use tracing::error;

/// Embedding vector dimension for BAAI/bge-small-en-v1.5 model
pub const EMBEDDING_DIMENSION: usize = 384;

/// Time window (in seconds) to consider a container "recently edited" after closing
const RECENTLY_EDITED_THRESHOLD_SECS: i64 = 30;

/// Idle timeout (in seconds) before triggering re-embedding
const IDLE_TIMEOUT_THRESHOLD_SECS: i64 = 30;

/// Time window (in seconds) for critical containers on app shutdown
const SHUTDOWN_CRITICAL_WINDOW_SECS: i64 = 300; // 5 minutes

/// Node embedding service with adaptive chunking
pub struct NodeEmbeddingService {
    /// NLP engine for generating embeddings
    nlp_engine: Arc<EmbeddingService>,

    /// Database service for persistence
    db: Arc<DatabaseService>,
}

impl NodeEmbeddingService {
    /// Create a new NodeEmbeddingService
    ///
    /// # Arguments
    ///
    /// * `nlp_engine` - Initialized NLP engine for embedding generation
    /// * `db` - Database service for persistence
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use nodespace_core::services::NodeEmbeddingService;
    /// use nodespace_core::db::DatabaseService;
    /// use nodespace_nlp_engine::{EmbeddingService, EmbeddingConfig};
    /// use std::sync::Arc;
    /// use std::path::PathBuf;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
    ///
    /// let mut nlp_engine = EmbeddingService::new(EmbeddingConfig::default())?;
    /// nlp_engine.initialize()?;
    /// let nlp_engine = Arc::new(nlp_engine);
    ///
    /// let service = NodeEmbeddingService::new(nlp_engine, db);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(nlp_engine: Arc<EmbeddingService>, db: Arc<DatabaseService>) -> Self {
        Self { nlp_engine, db }
    }

    /// Create a new NodeEmbeddingService with default configuration
    ///
    /// Automatically initializes the NLP engine with default settings.
    ///
    /// # Arguments
    ///
    /// * `db` - Database service for persistence
    ///
    /// # Errors
    ///
    /// Returns error if NLP engine initialization fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use nodespace_core::services::NodeEmbeddingService;
    /// use nodespace_core::db::DatabaseService;
    /// use std::sync::Arc;
    /// use std::path::PathBuf;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
    /// let service = NodeEmbeddingService::new_with_defaults(db)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new_with_defaults(db: Arc<DatabaseService>) -> Result<Self, Box<dyn std::error::Error>> {
        use nodespace_nlp_engine::EmbeddingConfig;

        let mut nlp_engine = EmbeddingService::new(EmbeddingConfig::default())?;
        nlp_engine.initialize()?;
        let nlp_engine = Arc::new(nlp_engine);

        Ok(Self::new(nlp_engine, db))
    }

    /// Generate embedding for a container node with adaptive chunking
    ///
    /// # Arguments
    ///
    /// * `container_id` - ID of the container node to embed
    ///
    /// # Errors
    ///
    /// Returns `NodeServiceError` if:
    /// - Container node not found
    /// - Embedding generation fails
    /// - Database update fails
    ///
    /// # Chunking Strategy
    ///
    /// - **< 512 tokens**: Embed complete container as single unit
    /// - **512-2048 tokens**: Embed summary + top-level sections
    /// - **> 2048 tokens**: Embed summary + hierarchical sections (recursive)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeEmbeddingService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_nlp_engine::{EmbeddingService, EmbeddingConfig};
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
    /// # let mut nlp_engine = EmbeddingService::new(EmbeddingConfig::default())?;
    /// # nlp_engine.initialize()?;
    /// # let nlp_engine = Arc::new(nlp_engine);
    /// # let service = NodeEmbeddingService::new(nlp_engine, db);
    /// service.embed_container("topic-node-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn embed_container(&self, container_id: &str) -> Result<(), NodeServiceError> {
        // 1. Fetch topic and children
        let container = self.get_node(container_id).await?;
        let children = self.get_children(container_id).await?;

        // 2. Estimate total tokens
        let total_tokens = self.estimate_container_tokens(&container, &children);

        // 3. Apply adaptive chunking strategy
        match total_tokens {
            0..512 => {
                self.embed_complete_container(container_id, &container, &children)
                    .await?;
            }
            512..2048 => {
                self.embed_with_sections(container_id, &container, &children, false)
                    .await?;
            }
            _ => {
                self.embed_with_sections(container_id, &container, &children, true)
                    .await?;
            }
        }

        Ok(())
    }

    /// Search containers using Turso's native vector similarity search
    ///
    /// Uses DiskANN algorithm for fast approximate nearest neighbors.
    ///
    /// # Arguments
    ///
    /// * `query` - Search query text
    /// * `threshold` - Similarity threshold (0.0-1.0, lower = more similar)
    /// * `limit` - Maximum number of results
    ///
    /// # Returns
    ///
    /// Vector of container nodes sorted by similarity (most similar first)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeEmbeddingService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_nlp_engine::{EmbeddingService, EmbeddingConfig};
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
    /// # let mut nlp_engine = EmbeddingService::new(EmbeddingConfig::default())?;
    /// # nlp_engine.initialize()?;
    /// # let nlp_engine = Arc::new(nlp_engine);
    /// # let service = NodeEmbeddingService::new(nlp_engine, db);
    /// let results = service.search_containers("machine learning", 0.7, 20).await?;
    /// for node in results {
    ///     println!("Found: {} ({})", node.content, node.id);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn search_containers(
        &self,
        query: &str,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<Node>, NodeServiceError> {
        // 1. Generate query embedding
        let query_embedding = self.nlp_engine.generate_embedding(query).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Embedding generation failed: {}", e))
        })?;

        let query_blob = EmbeddingService::to_blob(&query_embedding);

        // 2. Use Turso's native vector_top_k function
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT
                n.id, n.node_type, n.content, n.parent_id, n.container_node_id,
                n.before_sibling_id, n.created_at, n.modified_at, n.properties,
                n.embedding_vector, vt.distance
             FROM vector_top_k('idx_nodes_embedding_vector', vector(?), ?) vt
             JOIN nodes n ON n.rowid = vt.rowid
             WHERE n.container_node_id IS NULL
               AND vt.distance < ?",
            )
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Query preparation failed: {}", e))
            })?;

        let mut rows = stmt
            .query(params![query_blob, limit as i64, threshold])
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Query execution failed: {}", e)))?;

        // 3. Convert rows to Node objects
        let mut nodes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Row fetch failed: {}", e)))?
        {
            let node = self.row_to_node(&row)?;
            nodes.push(node);
        }

        Ok(nodes)
    }

    /// Exact vector similarity search (no index, slower but more accurate)
    ///
    /// Uses cosine distance for exact similarity calculation.
    ///
    /// # Arguments
    ///
    /// * `query` - Search query text
    /// * `threshold` - Similarity threshold (0.0-1.0, lower = more similar)
    /// * `limit` - Maximum number of results
    ///
    /// # Returns
    ///
    /// Vector of container nodes sorted by similarity (most similar first)
    pub async fn exact_search_containers(
        &self,
        query: &str,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<Node>, NodeServiceError> {
        let query_embedding = self.nlp_engine.generate_embedding(query).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Embedding generation failed: {}", e))
        })?;

        let query_blob = EmbeddingService::to_blob(&query_embedding);

        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT
                id, node_type, content, parent_id, container_node_id,
                before_sibling_id, created_at, modified_at, properties,
                embedding_vector,
                vector_distance_cosine(embedding_vector, vector(?)) as distance
             FROM nodes
             WHERE container_node_id IS NULL
               AND embedding_vector IS NOT NULL
               AND vector_distance_cosine(embedding_vector, vector(?)) < ?
             ORDER BY distance ASC
             LIMIT ?",
            )
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Query preparation failed: {}", e))
            })?;

        let mut rows = stmt
            .query(params![
                query_blob.clone(),
                query_blob,
                threshold,
                limit as i64
            ])
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Query execution failed: {}", e)))?;

        let mut nodes = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Row fetch failed: {}", e)))?
        {
            let node = self.row_to_node(&row)?;
            nodes.push(node);
        }

        Ok(nodes)
    }

    /// Smart trigger: Re-embed container when it's closed (if recently edited)
    ///
    /// This is called when a user closes a container page. If the topic was edited
    /// within the last 30 seconds, it triggers immediate re-embedding.
    ///
    /// # Arguments
    ///
    /// * `container_id` - ID of the container node that was closed
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::NodeEmbeddingService;
    /// # use nodespace_core::db::DatabaseService;
    /// # use nodespace_nlp_engine::{EmbeddingService, EmbeddingConfig};
    /// # use std::sync::Arc;
    /// # use std::path::PathBuf;
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
    /// # let mut nlp_engine = EmbeddingService::new(EmbeddingConfig::default())?;
    /// # nlp_engine.initialize()?;
    /// # let nlp_engine = Arc::new(nlp_engine);
    /// # let service = NodeEmbeddingService::new(nlp_engine, db);
    /// // User closes container page
    /// service.on_container_closed("topic-id").await?;
    /// # Ok(())
    /// # }
    /// # ```
    pub async fn on_container_closed(&self, container_id: &str) -> Result<(), NodeServiceError> {
        // Check if topic was recently edited (within configured threshold)
        let recently_edited = self
            .was_recently_edited(container_id, RECENTLY_EDITED_THRESHOLD_SECS)
            .await?;

        if recently_edited {
            // Re-embed immediately
            self.embed_container(container_id).await?;
            self.mark_container_embedded(container_id).await?;
        }

        Ok(())
    }

    /// Smart trigger: Re-embed container after idle timeout (30 seconds of no edits)
    ///
    /// This should be called periodically by the frontend to check if a container
    /// has been idle long enough to warrant re-embedding.
    ///
    /// # Arguments
    ///
    /// * `container_id` - ID of the container node to check
    ///
    /// # Returns
    ///
    /// `true` if re-embedding was triggered, `false` if not needed
    pub async fn on_idle_timeout(&self, container_id: &str) -> Result<bool, NodeServiceError> {
        // Check if container is stale and was last edited > configured idle threshold
        let should_embed = self
            .should_embed_after_idle(container_id, IDLE_TIMEOUT_THRESHOLD_SECS)
            .await?;

        if should_embed {
            self.embed_container(container_id).await?;
            self.mark_container_embedded(container_id).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Manually sync all stale containers (for explicit user action)
    ///
    /// This processes all stale containers immediately.
    ///
    /// # Returns
    ///
    /// Number of containers re-embedded
    pub async fn sync_all_stale_containers(&self) -> Result<usize, NodeServiceError> {
        let stale_containers = self.get_all_stale_containers().await?;
        let count = stale_containers.len();

        for container_id in stale_containers {
            if let Err(e) = self.embed_container(&container_id).await {
                error!("Failed to embed container {}: {}", container_id, e);
                continue;
            }
            self.mark_container_embedded(&container_id).await?;
        }

        Ok(count)
    }

    /// Sync critical containers before app shutdown
    ///
    /// Re-embeds containers that were edited within the last 5 minutes.
    ///
    /// # Returns
    ///
    /// Number of containers re-embedded
    pub async fn on_app_shutdown(&self) -> Result<usize, NodeServiceError> {
        // Get topics edited within configured shutdown window
        let critical_containers = self
            .get_recently_edited_containers(SHUTDOWN_CRITICAL_WINDOW_SECS)
            .await?;
        let count = critical_containers.len();

        for container_id in critical_containers {
            if let Err(e) = self.embed_container(&container_id).await {
                error!("Failed to embed container {}: {}", container_id, e);
                continue;
            }
            self.mark_container_embedded(&container_id).await?;
        }

        Ok(count)
    }

    // Private helper methods

    /// Estimate token count from text
    ///
    /// Uses a conservative heuristic: 1 token ≈ 3.5 characters with 20% safety margin.
    /// This ensures we don't underestimate tokens for chunking decisions.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// // "hello world" = 11 chars / 3.5 * 1.2 = 3.77 → 4 tokens
    /// // This is slightly conservative (actual is likely 2-3 tokens)
    /// ```
    fn estimate_tokens(&self, content: &str) -> usize {
        // Conservative estimate: 3.5 chars/token + 20% margin
        // Better to overestimate than underestimate for chunking
        ((content.len() as f32 / 3.5) * 1.2).ceil() as usize
    }

    /// Estimate total tokens for a container (including all children)
    fn estimate_container_tokens(&self, container: &Node, children: &[Node]) -> usize {
        let container_tokens = self.estimate_tokens(&container.content);
        let children_tokens: usize = children
            .iter()
            .map(|n| self.estimate_tokens(&n.content))
            .sum();

        container_tokens + children_tokens
    }

    /// Embed complete container as single unit (< 512 tokens)
    async fn embed_complete_container(
        &self,
        container_id: &str,
        container: &Node,
        children: &[Node],
    ) -> Result<(), NodeServiceError> {
        // Combine all content
        let content = self.combine_content(container, children);

        // Generate embedding
        let embedding = self.nlp_engine.generate_embedding(&content).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Embedding generation failed: {}", e))
        })?;

        let blob = EmbeddingService::to_blob(&embedding);

        // Store with metadata
        let metadata = json!({
            "type": "complete_container",
            "generated_at": Utc::now().to_rfc3339(),
            "token_count": self.estimate_tokens(&content),
        });

        self.store_embedding(container_id, blob, metadata).await
    }

    /// Embed container with sections (512-2048 tokens or > 2048 tokens)
    async fn embed_with_sections(
        &self,
        container_id: &str,
        container: &Node,
        children: &[Node],
        hierarchical: bool,
    ) -> Result<(), NodeServiceError> {
        // 1. Generate summary (simple truncation for MVP)
        let full_content = self.combine_content(container, children);
        let summary = self.simple_summarize(&full_content, 512);

        // 2. Embed summary
        let summary_embedding = self.nlp_engine.generate_embedding(&summary).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Embedding generation failed: {}", e))
        })?;

        let summary_blob = EmbeddingService::to_blob(&summary_embedding);

        let summary_metadata = json!({
            "type": "container_summary",
            "parent_container": container_id,
            "generated_at": Utc::now().to_rfc3339(),
            "token_count": self.estimate_tokens(&summary),
        });

        self.store_embedding(container_id, summary_blob, summary_metadata)
            .await?;

        // 3. Embed sections
        if hierarchical {
            self.embed_sections_hierarchical(container_id, children, 0)
                .await?;
        } else {
            self.embed_top_level_sections(container_id, children)
                .await?;
        }

        Ok(())
    }

    /// Embed top-level sections only (512-2048 tokens)
    async fn embed_top_level_sections(
        &self,
        container_id: &str,
        children: &[Node],
    ) -> Result<(), NodeServiceError> {
        for child in children {
            let embedding = self
                .nlp_engine
                .generate_embedding(&child.content)
                .map_err(|e| {
                    NodeServiceError::QueryFailed(format!("Embedding generation failed: {}", e))
                })?;

            let blob = EmbeddingService::to_blob(&embedding);

            let metadata = json!({
                "type": "container_section",
                "parent_container": container_id,
                "depth": 0,
                "generated_at": Utc::now().to_rfc3339(),
            });

            self.store_embedding(&child.id, blob, metadata).await?;
        }
        Ok(())
    }

    /// Embed sections hierarchically (> 2048 tokens)
    fn embed_sections_hierarchical<'a>(
        &'a self,
        container_id: &'a str,
        children: &'a [Node],
        depth: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), NodeServiceError>> + 'a + Send>,
    > {
        Box::pin(async move {
            for child in children {
                let embedding =
                    self.nlp_engine
                        .generate_embedding(&child.content)
                        .map_err(|e| {
                            NodeServiceError::QueryFailed(format!(
                                "Embedding generation failed: {}",
                                e
                            ))
                        })?;

                let blob = EmbeddingService::to_blob(&embedding);

                let metadata = json!({
                    "type": "container_section",
                    "parent_container": container_id,
                    "depth": depth,
                    "generated_at": Utc::now().to_rfc3339(),
                });

                self.store_embedding(&child.id, blob, metadata).await?;

                // Recurse for nested sections
                let grandchildren = self.get_children(&child.id).await?;
                if !grandchildren.is_empty() {
                    self.embed_sections_hierarchical(container_id, &grandchildren, depth + 1)
                        .await?;
                }
            }
            Ok(())
        })
    }

    /// Combine container and children content
    fn combine_content(&self, container: &Node, children: &[Node]) -> String {
        let mut content = container.content.clone();

        for child in children {
            content.push('\n');
            content.push_str(&child.content);
        }

        content
    }

    /// Simple summarization by truncation (MVP approach)
    ///
    /// For containers > 2048 tokens, we truncate to max_tokens * 4 characters.
    /// Future enhancement: Use Gemma 3 4B-QAT for intelligent summarization.
    fn simple_summarize(&self, content: &str, max_tokens: usize) -> String {
        let max_chars = max_tokens * 4;
        if content.len() <= max_chars {
            content.to_string()
        } else {
            format!("{}...", &content[..max_chars])
        }
    }

    /// Store embedding in database with metadata
    async fn store_embedding(
        &self,
        node_id: &str,
        blob: Vec<u8>,
        metadata: serde_json::Value,
    ) -> Result<(), NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        // Use json() wrapper to ensure SQLite parses the string as JSON type.
        // Without json(), json_set() treats the value as a string literal,
        // causing metadata access to return Null instead of the expected JSON object.
        conn.execute(
            "UPDATE nodes SET
                embedding_vector = ?,
                properties = json_set(properties, '$.embedding_metadata', json(?)),
                modified_at = CURRENT_TIMESTAMP
             WHERE id = ?",
            params![blob, metadata.to_string(), node_id],
        )
        .await
        .map_err(|e| NodeServiceError::QueryFailed(format!("Database update failed: {}", e)))?;

        Ok(())
    }

    /// Get a single node by ID
    async fn get_node(&self, node_id: &str) -> Result<Node, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT id, node_type, content, parent_id, container_node_id,
                    before_sibling_id, version, created_at, modified_at, properties, embedding_vector
             FROM nodes WHERE id = ?",
            )
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Query preparation failed: {}", e))
            })?;

        let mut rows = stmt
            .query(params![node_id])
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Query execution failed: {}", e)))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Row fetch failed: {}", e)))?
        {
            self.row_to_node(&row)
        } else {
            Err(NodeServiceError::node_not_found(format!(
                "Node not found: {}",
                node_id
            )))
        }
    }

    /// Get children of a node
    async fn get_children(&self, parent_id: &str) -> Result<Vec<Node>, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT id, node_type, content, parent_id, container_node_id,
                    before_sibling_id, version, created_at, modified_at, properties, embedding_vector
             FROM nodes WHERE parent_id = ?
             ORDER BY created_at ASC",
            )
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Query preparation failed: {}", e))
            })?;

        let mut rows = stmt
            .query(params![parent_id])
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Query execution failed: {}", e)))?;

        let mut children = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Row fetch failed: {}", e)))?
        {
            children.push(self.row_to_node(&row)?);
        }

        Ok(children)
    }

    /// Convert database row to Node
    fn row_to_node(&self, row: &libsql::Row) -> Result<Node, NodeServiceError> {
        let id: String = row
            .get(0)
            .map_err(|e| NodeServiceError::QueryFailed(format!("Failed to get id: {}", e)))?;
        let node_type: String = row.get(1).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get node_type: {}", e))
        })?;
        let content: String = row
            .get(2)
            .map_err(|e| NodeServiceError::QueryFailed(format!("Failed to get content: {}", e)))?;
        let parent_id: Option<String> = row.get(3).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get parent_id: {}", e))
        })?;
        let container_node_id: Option<String> = row.get(4).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get container_node_id: {}", e))
        })?;
        let before_sibling_id: Option<String> = row.get(5).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get before_sibling_id: {}", e))
        })?;

        let version: i64 = row
            .get(6)
            .map_err(|e| NodeServiceError::QueryFailed(format!("Failed to get version: {}", e)))?;

        let created_at_str: String = row.get(7).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get created_at: {}", e))
        })?;
        let created_at = parse_timestamp(&created_at_str).map_err(NodeServiceError::QueryFailed)?;

        let modified_at_str: String = row.get(8).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get modified_at: {}", e))
        })?;
        let modified_at =
            parse_timestamp(&modified_at_str).map_err(NodeServiceError::QueryFailed)?;

        let properties_str: String = row.get(9).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get properties: {}", e))
        })?;
        let properties: serde_json::Value = serde_json::from_str(&properties_str).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to parse properties JSON: {}", e))
        })?;

        let embedding_vector: Option<Vec<u8>> = row.get(10).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get embedding_vector: {}", e))
        })?;

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

    /// Check if a container was recently edited (within N seconds)
    async fn was_recently_edited(
        &self,
        container_id: &str,
        seconds: i64,
    ) -> Result<bool, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT (julianday('now') - julianday(last_content_update)) * 86400 as seconds_ago,
                        embedding_stale
                 FROM nodes WHERE id = ?",
            )
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Query preparation failed: {}", e))
            })?;

        let mut rows = stmt
            .query(params![container_id])
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Query execution failed: {}", e)))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Row fetch failed: {}", e)))?
        {
            let seconds_ago: Option<f64> = row.get(0).ok();
            let is_stale: bool = row.get(1).unwrap_or(false);

            // Recently edited if stale AND edited within N seconds
            Ok(is_stale && seconds_ago.map(|s| s <= seconds as f64).unwrap_or(false))
        } else {
            Ok(false)
        }
    }

    /// Check if topic should be embedded after idle period
    async fn should_embed_after_idle(
        &self,
        container_id: &str,
        idle_seconds: i64,
    ) -> Result<bool, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT (julianday('now') - julianday(last_content_update)) * 86400 as seconds_ago,
                        embedding_stale
                 FROM nodes WHERE id = ?",
            )
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Query preparation failed: {}", e))
            })?;

        let mut rows = stmt
            .query(params![container_id])
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Query execution failed: {}", e)))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Row fetch failed: {}", e)))?
        {
            let seconds_ago: Option<f64> = row.get(0).ok();
            let is_stale: bool = row.get(1).unwrap_or(false);

            // Should embed if stale AND idle for more than N seconds
            Ok(is_stale
                && seconds_ago
                    .map(|s| s > idle_seconds as f64)
                    .unwrap_or(false))
        } else {
            Ok(false)
        }
    }

    /// Get all stale containers
    pub async fn get_all_stale_containers(&self) -> Result<Vec<String>, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(
                "SELECT id FROM nodes
                 WHERE container_node_id IS NULL AND embedding_stale = TRUE
                 ORDER BY last_content_update DESC",
            )
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Query preparation failed: {}", e))
            })?;

        let mut rows = stmt
            .query(())
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Query execution failed: {}", e)))?;

        let mut container_ids = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Row fetch failed: {}", e)))?
        {
            let id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::QueryFailed(format!("Failed to get id: {}", e)))?;
            container_ids.push(id);
        }

        Ok(container_ids)
    }

    /// Get recently edited containers (within N seconds)
    async fn get_recently_edited_containers(
        &self,
        seconds: i64,
    ) -> Result<Vec<String>, NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        let mut stmt = conn
            .prepare(&format!(
                "SELECT id FROM nodes
                     WHERE container_node_id IS NULL
                       AND embedding_stale = TRUE
                       AND last_content_update > datetime('now', '-{} seconds')
                     ORDER BY last_content_update DESC",
                seconds
            ))
            .await
            .map_err(|e| {
                NodeServiceError::QueryFailed(format!("Query preparation failed: {}", e))
            })?;

        let mut rows = stmt
            .query(())
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Query execution failed: {}", e)))?;

        let mut container_ids = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| NodeServiceError::QueryFailed(format!("Row fetch failed: {}", e)))?
        {
            let id: String = row
                .get(0)
                .map_err(|e| NodeServiceError::QueryFailed(format!("Failed to get id: {}", e)))?;
            container_ids.push(id);
        }

        Ok(container_ids)
    }

    /// Mark container as freshly embedded (clear stale flag)
    async fn mark_container_embedded(&self, container_id: &str) -> Result<(), NodeServiceError> {
        let conn = self.db.connect_with_timeout().await.map_err(|e| {
            NodeServiceError::QueryFailed(format!("Database connection failed: {}", e))
        })?;

        conn.execute(
            "UPDATE nodes
             SET embedding_stale = FALSE,
                 last_embedding_update = CURRENT_TIMESTAMP
             WHERE id = ?",
            params![container_id],
        )
        .await
        .map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to mark container as embedded: {}", e))
        })?;

        Ok(())
    }
}

/// Clone implementation for spawning async tasks
impl Clone for NodeEmbeddingService {
    fn clone(&self) -> Self {
        Self {
            nlp_engine: Arc::clone(&self.nlp_engine),
            db: Arc::clone(&self.db),
        }
    }
}

/// Parse timestamp helper (reused from node_service)
fn parse_timestamp(s: &str) -> Result<chrono::DateTime<Utc>, String> {
    use chrono::NaiveDateTime;

    // Try SQLite format first: "YYYY-MM-DD HH:MM:SS"
    if let Ok(naive) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(naive.and_utc());
    }

    // Try RFC3339 format: "YYYY-MM-DDTHH:MM:SSZ"
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    Err(format!(
        "Unable to parse timestamp '{}' as SQLite or RFC3339 format",
        s
    ))
}

// Comprehensive tests in separate module
#[cfg(test)]
#[path = "embedding_service_test.rs"]
mod embedding_service_test;
