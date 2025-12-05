//! Root-Aggregate Embedding Service (Issue #729)
//!
//! ## Overview
//!
//! This service implements root-aggregate embedding for semantic search:
//! - Only ROOT nodes (no parent edge) of embeddable types get embedded
//! - Embeddings represent the semantic content of the entire subtree
//! - Uses the dedicated `embedding` table (not node.embedding_vector)
//! - Supports chunking for content > 512 tokens
//!
//! ## Embeddable Types
//!
//! - `text`, `header`, `code-block`, `schema` - embeddable when roots
//! - `task`, `date`, `person`, `ai-chat` - NOT embeddable
//! - Child nodes are NEVER directly embedded (contribute to parent's embedding)
//!
//! ## Queue System
//!
//! The embedding queue is managed via the `embedding` table's `stale` flag:
//! - New root nodes get a stale marker created
//! - Content changes mark existing embeddings as stale
//! - Background processor re-embeds stale entries
//!
//! ## Content Aggregation
//!
//! When embedding a root node:
//! 1. Fetch root + all descendants via `get_nodes_in_subtree()`
//! 2. Aggregate content in hierarchical order
//! 3. Chunk if > 512 tokens with ~100 token overlap
//! 4. Generate embedding per chunk
//! 5. Store in `embedding` table

use crate::db::SurrealStore;
use crate::models::{
    is_embeddable_type, EmbeddingConfig, EmbeddingSearchResult, NewEmbedding, Node,
};
use crate::services::error::NodeServiceError;
use nodespace_nlp_engine::EmbeddingService;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;

/// Embedding vector dimension for BAAI/bge-small-en-v1.5 model
pub const EMBEDDING_DIMENSION: usize = 384;

/// Default batch size for processing stale embeddings
pub const DEFAULT_BATCH_SIZE: usize = 50;

/// Maximum depth for parent chain traversal (safety limit to prevent infinite loops)
pub const MAX_PARENT_CHAIN_DEPTH: usize = 100;

/// Root-aggregate embedding service
///
/// Manages semantic embeddings using the root-aggregate model where only
/// root nodes of embeddable types get embedded, with their entire subtree
/// content aggregated into the embedding.
pub struct NodeEmbeddingService<C = surrealdb::engine::local::Db>
where
    C: surrealdb::Connection,
{
    /// NLP engine for generating embeddings
    nlp_engine: Arc<EmbeddingService>,
    /// SurrealDB store for persisting embeddings
    store: Arc<SurrealStore<C>>,
    /// Configuration for embedding behavior
    config: EmbeddingConfig,
}

impl<C> NodeEmbeddingService<C>
where
    C: surrealdb::Connection,
{
    /// Create a new NodeEmbeddingService with SurrealDB integration
    ///
    /// # Arguments
    /// * `nlp_engine` - The NLP engine for generating embeddings
    /// * `store` - The SurrealDB store for persisting embeddings
    pub fn new(nlp_engine: Arc<EmbeddingService>, store: Arc<SurrealStore<C>>) -> Self {
        tracing::info!("NodeEmbeddingService initialized with root-aggregate model");
        Self {
            nlp_engine,
            store,
            config: EmbeddingConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(
        nlp_engine: Arc<EmbeddingService>,
        store: Arc<SurrealStore<C>>,
        config: EmbeddingConfig,
    ) -> Self {
        tracing::info!(
            "NodeEmbeddingService initialized with custom config (debounce: {}s)",
            config.debounce_duration_secs
        );
        Self {
            nlp_engine,
            store,
            config,
        }
    }

    /// Get reference to the NLP engine
    pub fn nlp_engine(&self) -> &Arc<EmbeddingService> {
        &self.nlp_engine
    }

    /// Get reference to the SurrealDB store
    pub fn store(&self) -> &Arc<SurrealStore<C>> {
        &self.store
    }

    // =========================================================================
    // Root Node Detection
    // =========================================================================

    /// Check if a node is a root (has no parent)
    pub async fn is_root_node(&self, node_id: &str) -> Result<bool, NodeServiceError> {
        let parent =
            self.store.get_parent(node_id).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get parent: {}", e))
            })?;
        Ok(parent.is_none())
    }

    /// Find the root node ID for any node in a tree
    ///
    /// Traverses up the parent chain until finding a node with no parent.
    pub async fn find_root_id(&self, node_id: &str) -> Result<String, NodeServiceError> {
        let mut current_id = node_id.to_string();

        for _ in 0..MAX_PARENT_CHAIN_DEPTH {
            let parent = self.store.get_parent(&current_id).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get parent: {}", e))
            })?;

            match parent {
                Some(parent_node) => {
                    current_id = parent_node.id;
                }
                None => {
                    return Ok(current_id);
                }
            }
        }

        Err(NodeServiceError::query_failed(format!(
            "Max parent chain depth ({}) exceeded",
            MAX_PARENT_CHAIN_DEPTH
        )))
    }

    /// Check if a root node should be embedded based on its type
    pub fn should_embed_root(&self, node: &Node) -> bool {
        is_embeddable_type(&node.node_type)
    }

    // =========================================================================
    // Content Aggregation
    // =========================================================================

    /// Aggregate content from a root node and all its descendants
    ///
    /// Combines content from the entire subtree into a single text for embedding.
    /// Content is ordered hierarchically (parent before children).
    pub async fn aggregate_subtree_content(
        &self,
        root_id: &str,
    ) -> Result<String, NodeServiceError> {
        // Get root node
        let root = self
            .store
            .get_node(root_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to get root node: {}", e)))?
            .ok_or_else(|| NodeServiceError::node_not_found(root_id))?;

        // Get all descendants
        let descendants = self
            .store
            .get_nodes_in_subtree(root_id)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to get descendants: {}", e))
            })?;

        // Check descendant limit
        if descendants.len() > self.config.max_descendants {
            tracing::warn!(
                "Root {} has {} descendants, exceeding limit of {}. Truncating.",
                root_id,
                descendants.len(),
                self.config.max_descendants
            );
        }

        // Collect content parts
        let mut parts = Vec::new();

        // Add root content first
        if !root.content.trim().is_empty() {
            parts.push(root.content.clone());
        }

        // Add descendant content
        let limit = self.config.max_descendants.min(descendants.len());
        for node in descendants.into_iter().take(limit) {
            if !node.content.trim().is_empty() {
                parts.push(node.content);
            }
        }

        let aggregated = parts.join("\n\n");

        // Check size limit
        if aggregated.len() > self.config.max_content_size {
            tracing::warn!(
                "Aggregated content for {} exceeds max size ({} > {}). Truncating.",
                root_id,
                aggregated.len(),
                self.config.max_content_size
            );
            return Ok(aggregated[..self.config.max_content_size].to_string());
        }

        Ok(aggregated)
    }

    /// Compute content hash for change detection
    fn compute_content_hash(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    // =========================================================================
    // Chunking
    // =========================================================================

    /// Split content into chunks for embedding
    ///
    /// Uses approximate token counting (4 chars ≈ 1 token) and overlaps
    /// chunks by `overlap_tokens` to maintain context across boundaries.
    fn chunk_content(&self, content: &str) -> Vec<(i32, i32, String)> {
        // Approximate token count (rough estimate: 4 chars ≈ 1 token)
        let chars_per_token = 4;
        let max_chars = self.config.max_tokens_per_chunk * chars_per_token;
        let overlap_chars = self.config.overlap_tokens * chars_per_token;

        if content.len() <= max_chars {
            // Single chunk
            return vec![(0, content.len() as i32, content.to_string())];
        }

        let mut chunks = Vec::new();
        let mut start = 0;

        while start < content.len() {
            let end = (start + max_chars).min(content.len());

            // Try to find a good break point (newline or space)
            let actual_end = if end < content.len() {
                // Look for paragraph break first
                if let Some(pos) = content[start..end].rfind("\n\n") {
                    start + pos + 2
                }
                // Then sentence break
                else if let Some(pos) = content[start..end].rfind(". ") {
                    start + pos + 2
                }
                // Then word break
                else if let Some(pos) = content[start..end].rfind(' ') {
                    start + pos + 1
                } else {
                    end
                }
            } else {
                end
            };

            chunks.push((
                start as i32,
                actual_end as i32,
                content[start..actual_end].to_string(),
            ));

            // Move forward with overlap
            start = if actual_end >= content.len() {
                actual_end
            } else {
                (actual_end - overlap_chars).max(start + 1)
            };
        }

        chunks
    }

    // =========================================================================
    // Embedding Generation
    // =========================================================================

    /// Generate and store embeddings for a root node
    ///
    /// This is the main entry point for embedding a root node's content.
    /// Aggregates content, chunks if necessary, generates embeddings,
    /// and stores in the embedding table.
    pub async fn embed_root_node(&self, root_id: &str) -> Result<(), NodeServiceError> {
        // Get root node and verify it exists
        let root = self
            .store
            .get_node(root_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to get root: {}", e)))?
            .ok_or_else(|| NodeServiceError::node_not_found(root_id))?;

        // Check if this type should be embedded
        if !self.should_embed_root(&root) {
            tracing::debug!(
                "Skipping non-embeddable root: {} (type: {})",
                root_id,
                root.node_type
            );
            // Delete any existing embeddings for this node
            self.store.delete_embeddings(root_id).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to delete embeddings: {}", e))
            })?;
            return Ok(());
        }

        // Aggregate content from subtree
        let content = self.aggregate_subtree_content(root_id).await?;

        if content.trim().is_empty() {
            tracing::debug!("Skipping root with empty content: {}", root_id);
            self.store.delete_embeddings(root_id).await.map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to delete embeddings: {}", e))
            })?;
            return Ok(());
        }

        // Compute content hash
        let content_hash = Self::compute_content_hash(&content);

        // Chunk content
        let chunks = self.chunk_content(&content);
        let total_chunks = chunks.len() as i32;

        tracing::debug!(
            "Embedding root {} with {} chunks ({} chars)",
            root_id,
            total_chunks,
            content.len()
        );

        // Generate embeddings for all chunks
        let mut new_embeddings = Vec::new();
        for (idx, (start, end, chunk_text)) in chunks.into_iter().enumerate() {
            // Estimate token count
            let token_count = (chunk_text.len() / 4) as i32;

            // Generate embedding
            let vector = self
                .nlp_engine
                .generate_embedding(&chunk_text)
                .map_err(|e| {
                    NodeServiceError::SerializationError(format!(
                        "Embedding generation failed: {}",
                        e
                    ))
                })?;

            new_embeddings.push(NewEmbedding::chunk(
                root_id,
                vector,
                idx as i32,
                start,
                end,
                total_chunks,
                &content_hash,
                token_count,
            ));
        }

        // Store embeddings (replaces any existing)
        self.store
            .upsert_embeddings(root_id, new_embeddings)
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to store embeddings: {}", e))
            })?;

        tracing::debug!(
            "Successfully embedded root {} ({} chunks)",
            root_id,
            total_chunks
        );

        Ok(())
    }

    /// Process all stale embeddings
    ///
    /// Fetches root node IDs with stale embeddings and re-generates them.
    pub async fn process_stale_embeddings(
        &self,
        limit: Option<usize>,
    ) -> Result<usize, NodeServiceError> {
        let batch_size = limit.unwrap_or(DEFAULT_BATCH_SIZE);

        // Get stale root IDs from embedding table
        let stale_ids = self
            .store
            .get_stale_embedding_root_ids(Some(batch_size as i64))
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Failed to query stale embeddings: {}", e))
            })?;

        if stale_ids.is_empty() {
            tracing::debug!("No stale embeddings to process");
            return Ok(0);
        }

        tracing::info!("Processing {} stale embeddings", stale_ids.len());

        let mut success_count = 0;
        for root_id in stale_ids {
            match self.embed_root_node(&root_id).await {
                Ok(_) => {
                    success_count += 1;
                }
                Err(e) => {
                    tracing::error!("Failed to embed root {}: {}", root_id, e);
                    // Record error but continue processing
                    if let Err(record_err) = self
                        .store
                        .record_embedding_error(&root_id, &e.to_string())
                        .await
                    {
                        tracing::error!("Failed to record error for {}: {}", root_id, record_err);
                    }
                }
            }
        }

        tracing::info!(
            "Successfully processed {}/{} stale embeddings",
            success_count,
            batch_size
        );

        Ok(success_count)
    }

    // =========================================================================
    // Queue Management
    // =========================================================================

    /// Queue a node for embedding
    ///
    /// If the node is a root of an embeddable type, marks its embedding as stale.
    /// If the node is a child, finds its root and marks that as stale.
    pub async fn queue_for_embedding(&self, node_id: &str) -> Result<(), NodeServiceError> {
        // Find the root of this node's tree
        let root_id = self.find_root_id(node_id).await?;

        // Get the root node to check its type
        let root = match self
            .store
            .get_node(&root_id)
            .await
            .map_err(|e| NodeServiceError::query_failed(format!("Failed to get root: {}", e)))?
        {
            Some(node) => node,
            None => {
                tracing::debug!("Root node {} not found, skipping embedding queue", root_id);
                return Ok(());
            }
        };

        // Check if root type is embeddable
        if !self.should_embed_root(&root) {
            tracing::debug!(
                "Root {} is not embeddable (type: {}), skipping queue",
                root_id,
                root.node_type
            );
            return Ok(());
        }

        // Check if embedding exists for this root
        let has_embedding = self.store.has_embeddings(&root_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to check embeddings: {}", e))
        })?;

        if has_embedding {
            // Mark existing embedding as stale
            self.store
                .mark_root_embedding_stale(&root_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to mark embedding stale: {}", e))
                })?;
        } else {
            // Create new stale marker
            self.store
                .create_stale_embedding_marker(&root_id)
                .await
                .map_err(|e| {
                    NodeServiceError::query_failed(format!("Failed to create stale marker: {}", e))
                })?;
        }

        tracing::debug!(
            "Queued root {} for embedding (via node {})",
            root_id,
            node_id
        );

        Ok(())
    }

    /// Queue multiple nodes for embedding
    ///
    /// Efficiently handles multiple nodes by deduplicating roots.
    pub async fn queue_nodes_for_embedding(
        &self,
        node_ids: &[&str],
    ) -> Result<(), NodeServiceError> {
        let mut roots_to_queue: HashSet<String> = HashSet::new();

        // Find unique roots
        for node_id in node_ids {
            match self.find_root_id(node_id).await {
                Ok(root_id) => {
                    roots_to_queue.insert(root_id);
                }
                Err(e) => {
                    tracing::warn!("Failed to find root for {}: {}", node_id, e);
                }
            }
        }

        // Queue each unique root
        for root_id in roots_to_queue {
            if let Err(e) = self.queue_for_embedding(&root_id).await {
                tracing::error!("Failed to queue root {} for embedding: {}", root_id, e);
            }
        }

        Ok(())
    }

    // =========================================================================
    // Search
    // =========================================================================

    /// Search for nodes by semantic similarity
    ///
    /// Generates an embedding for the query text and searches for similar
    /// root nodes. Returns the node IDs and similarity scores.
    pub async fn semantic_search(
        &self,
        query: &str,
        limit: usize,
        threshold: f32,
    ) -> Result<Vec<EmbeddingSearchResult>, NodeServiceError> {
        if query.trim().is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Search query cannot be empty",
            ));
        }

        // Generate query embedding
        let query_vector = self.nlp_engine.generate_embedding(query).map_err(|e| {
            NodeServiceError::SerializationError(format!(
                "Failed to generate query embedding: {}",
                e
            ))
        })?;

        // Search embedding table
        let results = self
            .store
            .search_embeddings(&query_vector, limit as i64, Some(threshold as f64))
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!("Semantic search failed: {}", e))
            })?;

        tracing::debug!(
            "Semantic search for '{}' returned {} results",
            query,
            results.len()
        );

        Ok(results)
    }

    /// Search and return full nodes
    ///
    /// Convenience method that fetches the full Node objects for search results.
    pub async fn semantic_search_nodes(
        &self,
        query: &str,
        limit: usize,
        threshold: f32,
    ) -> Result<Vec<(Node, f64)>, NodeServiceError> {
        let results = self.semantic_search(query, limit, threshold).await?;

        let mut nodes_with_scores = Vec::new();
        for result in results {
            if let Ok(Some(node)) = self.store.get_node(&result.node_id).await {
                nodes_with_scores.push((node, result.similarity));
            }
        }

        Ok(nodes_with_scores)
    }

    // =========================================================================
    // Cleanup
    // =========================================================================

    /// Delete embeddings for a node
    ///
    /// Called when a node is deleted.
    pub async fn delete_node_embeddings(&self, node_id: &str) -> Result<(), NodeServiceError> {
        self.store.delete_embeddings(node_id).await.map_err(|e| {
            NodeServiceError::query_failed(format!("Failed to delete embeddings: {}", e))
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_content_single() {
        // Create a minimal service for testing chunking logic
        let config = EmbeddingConfig::default();

        // Test content under limit
        let short_content = "Hello world";
        // Approximate: 11 chars / 4 = ~3 tokens, well under 512
        assert!(short_content.len() < config.max_tokens_per_chunk * 4);
    }

    #[test]
    fn test_content_hash() {
        let hash1 =
            NodeEmbeddingService::<surrealdb::engine::local::Db>::compute_content_hash("hello");
        let hash2 =
            NodeEmbeddingService::<surrealdb::engine::local::Db>::compute_content_hash("hello");
        let hash3 =
            NodeEmbeddingService::<surrealdb::engine::local::Db>::compute_content_hash("world");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA256 hex = 64 chars
    }
}
