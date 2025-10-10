//! Topic Embedding Service
//!
//! This module provides embedding generation for topic nodes with adaptive chunking
//! based on content size. It integrates with the NLP engine and Turso's native
//! vector search capabilities.
//!
//! # Features
//!
//! - Adaptive chunking strategy based on token count (< 512, 512-2048, > 2048)
//! - Integration with NLP engine (BAAI/bge-small-en-v1.5, 384 dimensions)
//! - Native Turso vector search with DiskANN indexing
//! - Debounced re-embedding on content changes
//! - Metadata tracking for embeddings
//!
//! # Architecture
//!
//! Topic nodes (@ mention pages) are the primary semantic search targets.
//! Embeddings are stored in the `embedding_vector` column as F32_BLOB(384).
//!
//! # Chunking Strategies
//!
//! - **< 512 tokens**: Single embedding for entire topic
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
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration, Instant};

/// Embedding vector dimension for BAAI/bge-small-en-v1.5 model
pub const EMBEDDING_DIMENSION: usize = 384;

/// Topic embedding service with adaptive chunking
pub struct TopicEmbeddingService {
    /// NLP engine for generating embeddings
    nlp_engine: Arc<EmbeddingService>,

    /// Database service for persistence
    db: Arc<DatabaseService>,

    /// Debouncer state for content change handling
    debouncer: Arc<Mutex<DebouncerState>>,
}

/// Internal state for debouncing re-embedding operations
struct DebouncerState {
    /// Map of topic_id -> last update time
    pending_updates: std::collections::HashMap<String, Instant>,
    /// Debounce delay (5 seconds)
    delay: Duration,
}

impl TopicEmbeddingService {
    /// Create a new TopicEmbeddingService
    ///
    /// # Arguments
    ///
    /// * `nlp_engine` - Initialized NLP engine for embedding generation
    /// * `db` - Database service for persistence
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use nodespace_core::services::TopicEmbeddingService;
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
    /// let service = TopicEmbeddingService::new(nlp_engine, db);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(nlp_engine: Arc<EmbeddingService>, db: Arc<DatabaseService>) -> Self {
        let debouncer_state = DebouncerState {
            pending_updates: std::collections::HashMap::new(),
            delay: Duration::from_secs(5),
        };

        Self {
            nlp_engine,
            db,
            debouncer: Arc::new(Mutex::new(debouncer_state)),
        }
    }

    /// Create a new TopicEmbeddingService with default configuration
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
    /// use nodespace_core::services::TopicEmbeddingService;
    /// use nodespace_core::db::DatabaseService;
    /// use std::sync::Arc;
    /// use std::path::PathBuf;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let db = Arc::new(DatabaseService::new(PathBuf::from("./data/test.db")).await?);
    /// let service = TopicEmbeddingService::new_with_defaults(db)?;
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

    /// Generate embedding for a topic node with adaptive chunking
    ///
    /// # Arguments
    ///
    /// * `topic_id` - ID of the topic node to embed
    ///
    /// # Errors
    ///
    /// Returns `NodeServiceError` if:
    /// - Topic node not found
    /// - Embedding generation fails
    /// - Database update fails
    ///
    /// # Chunking Strategy
    ///
    /// - **< 512 tokens**: Embed complete topic as single unit
    /// - **512-2048 tokens**: Embed summary + top-level sections
    /// - **> 2048 tokens**: Embed summary + hierarchical sections (recursive)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::TopicEmbeddingService;
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
    /// # let service = TopicEmbeddingService::new(nlp_engine, db);
    /// service.embed_topic("topic-node-id").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn embed_topic(&self, topic_id: &str) -> Result<(), NodeServiceError> {
        // 1. Fetch topic and children
        let topic = self.get_node(topic_id).await?;
        let children = self.get_children(topic_id).await?;

        // 2. Estimate total tokens
        let total_tokens = self.estimate_topic_tokens(&topic, &children);

        // 3. Apply adaptive chunking strategy
        match total_tokens {
            0..512 => {
                self.embed_complete_topic(topic_id, &topic, &children)
                    .await?;
            }
            512..2048 => {
                self.embed_with_sections(topic_id, &topic, &children, false)
                    .await?;
            }
            _ => {
                self.embed_with_sections(topic_id, &topic, &children, true)
                    .await?;
            }
        }

        Ok(())
    }

    /// Search topics using Turso's native vector similarity search
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
    /// Vector of topic nodes sorted by similarity (most similar first)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::TopicEmbeddingService;
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
    /// # let service = TopicEmbeddingService::new(nlp_engine, db);
    /// let results = service.search_topics("machine learning", 0.7, 20).await?;
    /// for node in results {
    ///     println!("Found: {} ({})", node.content, node.id);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn search_topics(
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
                n.id, n.node_type, n.content, n.parent_id, n.origin_node_id,
                n.before_sibling_id, n.created_at, n.modified_at, n.properties,
                n.embedding_vector, vt.distance
             FROM vector_top_k('idx_nodes_embedding_vector', vector(?), ?) vt
             JOIN nodes n ON n.rowid = vt.rowid
             WHERE n.node_type = 'topic'
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
    /// Vector of topic nodes sorted by similarity (most similar first)
    pub async fn exact_search_topics(
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
                id, node_type, content, parent_id, origin_node_id,
                before_sibling_id, created_at, modified_at, properties,
                embedding_vector,
                vector_distance_cosine(embedding_vector, vector(?)) as distance
             FROM nodes
             WHERE node_type = 'topic'
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

    /// Schedule a debounced re-embedding for a topic
    ///
    /// Content changes trigger this method, which schedules a re-embedding
    /// after 5 seconds of inactivity. If additional changes occur within
    /// the 5-second window, the timer resets.
    ///
    /// # Arguments
    ///
    /// * `topic_id` - ID of the topic node that changed
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use nodespace_core::services::TopicEmbeddingService;
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
    /// # let service = TopicEmbeddingService::new(nlp_engine, db);
    /// // User edits topic content
    /// service.schedule_update_topic_embedding("topic-id").await;
    /// // If user continues editing, timer resets
    /// service.schedule_update_topic_embedding("topic-id").await;
    /// // After 5 seconds of no changes, re-embedding occurs automatically
    /// # Ok(())
    /// # }
    /// ```
    pub async fn schedule_update_topic_embedding(&self, topic_id: &str) {
        let topic_id = topic_id.to_string();
        let mut state = self.debouncer.lock().await;

        // Update the last change time for this topic
        state
            .pending_updates
            .insert(topic_id.clone(), Instant::now());
        let delay = state.delay;
        drop(state);

        // Spawn background task to handle debounced update
        let service = self.clone();
        let topic_id_clone = topic_id.clone();

        tokio::spawn(async move {
            sleep(delay).await;

            // Check if this is still the most recent update
            let state = service.debouncer.lock().await;
            if let Some(last_update) = state.pending_updates.get(&topic_id_clone) {
                if Instant::now().duration_since(*last_update) >= delay {
                    drop(state);

                    // Perform the actual re-embedding
                    if let Err(e) = service.embed_topic(&topic_id_clone).await {
                        eprintln!("Failed to re-embed topic {}: {}", topic_id_clone, e);
                    }

                    // Remove from pending updates
                    let mut state = service.debouncer.lock().await;
                    state.pending_updates.remove(&topic_id_clone);
                }
            }
        });
    }

    /// Immediately update topic embedding (no debouncing)
    ///
    /// Use this for explicit user actions or batch operations.
    ///
    /// # Arguments
    ///
    /// * `topic_id` - ID of the topic node to update
    pub async fn update_topic_embedding(&self, topic_id: &str) -> Result<(), NodeServiceError> {
        self.embed_topic(topic_id).await
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

    /// Estimate total tokens for a topic (including all children)
    fn estimate_topic_tokens(&self, topic: &Node, children: &[Node]) -> usize {
        let topic_tokens = self.estimate_tokens(&topic.content);
        let children_tokens: usize = children
            .iter()
            .map(|n| self.estimate_tokens(&n.content))
            .sum();

        topic_tokens + children_tokens
    }

    /// Embed complete topic as single unit (< 512 tokens)
    async fn embed_complete_topic(
        &self,
        topic_id: &str,
        topic: &Node,
        children: &[Node],
    ) -> Result<(), NodeServiceError> {
        // Combine all content
        let content = self.combine_content(topic, children);

        // Generate embedding
        let embedding = self.nlp_engine.generate_embedding(&content).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Embedding generation failed: {}", e))
        })?;

        let blob = EmbeddingService::to_blob(&embedding);

        // Store with metadata
        let metadata = json!({
            "type": "complete_topic",
            "generated_at": Utc::now().to_rfc3339(),
            "token_count": self.estimate_tokens(&content),
        });

        self.store_embedding(topic_id, blob, metadata).await
    }

    /// Embed topic with sections (512-2048 tokens or > 2048 tokens)
    async fn embed_with_sections(
        &self,
        topic_id: &str,
        topic: &Node,
        children: &[Node],
        hierarchical: bool,
    ) -> Result<(), NodeServiceError> {
        // 1. Generate summary (simple truncation for MVP)
        let full_content = self.combine_content(topic, children);
        let summary = self.simple_summarize(&full_content, 512);

        // 2. Embed summary
        let summary_embedding = self.nlp_engine.generate_embedding(&summary).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Embedding generation failed: {}", e))
        })?;

        let summary_blob = EmbeddingService::to_blob(&summary_embedding);

        let summary_metadata = json!({
            "type": "topic_summary",
            "parent_topic": topic_id,
            "generated_at": Utc::now().to_rfc3339(),
            "token_count": self.estimate_tokens(&summary),
        });

        self.store_embedding(topic_id, summary_blob, summary_metadata)
            .await?;

        // 3. Embed sections
        if hierarchical {
            self.embed_sections_hierarchical(topic_id, children, 0)
                .await?;
        } else {
            self.embed_top_level_sections(topic_id, children).await?;
        }

        Ok(())
    }

    /// Embed top-level sections only (512-2048 tokens)
    async fn embed_top_level_sections(
        &self,
        topic_id: &str,
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
                "type": "topic_section",
                "parent_topic": topic_id,
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
        topic_id: &'a str,
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
                    "type": "topic_section",
                    "parent_topic": topic_id,
                    "depth": depth,
                    "generated_at": Utc::now().to_rfc3339(),
                });

                self.store_embedding(&child.id, blob, metadata).await?;

                // Recurse for nested sections
                let grandchildren = self.get_children(&child.id).await?;
                if !grandchildren.is_empty() {
                    self.embed_sections_hierarchical(topic_id, &grandchildren, depth + 1)
                        .await?;
                }
            }
            Ok(())
        })
    }

    /// Combine topic and children content
    fn combine_content(&self, topic: &Node, children: &[Node]) -> String {
        let mut content = topic.content.clone();

        for child in children {
            content.push('\n');
            content.push_str(&child.content);
        }

        content
    }

    /// Simple summarization by truncation (MVP approach)
    ///
    /// For topics > 2048 tokens, we truncate to max_tokens * 4 characters.
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

        conn.execute(
            "UPDATE nodes SET
                embedding_vector = ?,
                properties = json_set(properties, '$.embedding_metadata', ?),
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
                "SELECT id, node_type, content, parent_id, origin_node_id,
                    before_sibling_id, created_at, modified_at, properties, embedding_vector
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
                "SELECT id, node_type, content, parent_id, origin_node_id,
                    before_sibling_id, created_at, modified_at, properties, embedding_vector
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
        let origin_node_id: Option<String> = row.get(4).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get origin_node_id: {}", e))
        })?;
        let before_sibling_id: Option<String> = row.get(5).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get before_sibling_id: {}", e))
        })?;

        let created_at_str: String = row.get(6).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get created_at: {}", e))
        })?;
        let created_at = parse_timestamp(&created_at_str).map_err(NodeServiceError::QueryFailed)?;

        let modified_at_str: String = row.get(7).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get modified_at: {}", e))
        })?;
        let modified_at =
            parse_timestamp(&modified_at_str).map_err(NodeServiceError::QueryFailed)?;

        let properties_str: String = row.get(8).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get properties: {}", e))
        })?;
        let properties: serde_json::Value = serde_json::from_str(&properties_str).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to parse properties JSON: {}", e))
        })?;

        let embedding_vector: Option<Vec<u8>> = row.get(9).map_err(|e| {
            NodeServiceError::QueryFailed(format!("Failed to get embedding_vector: {}", e))
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
            mentions: Vec::new(),
            mentioned_by: Vec::new(),
        })
    }
}

/// Clone implementation for spawning async tasks
impl Clone for TopicEmbeddingService {
    fn clone(&self) -> Self {
        Self {
            nlp_engine: Arc::clone(&self.nlp_engine),
            db: Arc::clone(&self.db),
            debouncer: Arc::clone(&self.debouncer),
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
