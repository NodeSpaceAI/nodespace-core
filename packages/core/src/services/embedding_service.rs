//! Node Embedding Service
//!
//! **STATUS: Fully integrated with SurrealDB and NLP engine (Issue #489)**
//!
//! ## Features
//!
//! - ✅ Database-level staleness tracking (`mark_embedding_stale`, `get_nodes_with_stale_embeddings`)
//! - ✅ Embedding vector storage in SurrealDB (`update_embedding` method)
//! - ✅ Atomic update operations with version control
//! - ✅ NLP engine integration for embedding generation
//! - ✅ Background batch processing of stale embeddings
//!
//! ## Out of Scope (Future Issues)
//!
//! - ⏳ Vector similarity search implementation (Issue #107 - SurrealDB native vector search)

use crate::db::SurrealStore;
use crate::models::Node;
use crate::services::error::NodeServiceError;
use nodespace_nlp_engine::EmbeddingService;
use std::sync::Arc;

/// Embedding vector dimension for BAAI/bge-small-en-v1.5 model
pub const EMBEDDING_DIMENSION: usize = 384;

/// Default batch size for processing stale embeddings
pub const DEFAULT_BATCH_SIZE: usize = 50;

/// Node embedding service with SurrealDB integration
pub struct NodeEmbeddingService {
    /// NLP engine for generating embeddings
    nlp_engine: Arc<EmbeddingService>,
    /// SurrealDB store for persisting embeddings
    store: Arc<SurrealStore>,
}

impl NodeEmbeddingService {
    /// Create a new NodeEmbeddingService with SurrealDB integration
    ///
    /// # Arguments
    /// * `nlp_engine` - The NLP engine for generating embeddings
    /// * `store` - The SurrealDB store for persisting embeddings
    pub fn new(nlp_engine: Arc<EmbeddingService>, store: Arc<SurrealStore>) -> Self {
        tracing::info!("NodeEmbeddingService initialized with SurrealDB and NLP engine");
        Self { nlp_engine, store }
    }

    /// Get reference to the NLP engine for direct embedding operations
    pub fn nlp_engine(&self) -> &Arc<EmbeddingService> {
        &self.nlp_engine
    }

    /// Extract text content from node for embedding
    ///
    /// Uses the node's content field. If empty, returns empty string.
    fn extract_text_from_node(node: &Node) -> String {
        if node.content.trim().is_empty() {
            String::new()
        } else {
            node.content.clone()
        }
    }

    /// Generate and store embedding for a single node
    ///
    /// Extracts text from the node, generates an embedding using the NLP engine,
    /// converts to binary format, and stores in SurrealDB.
    ///
    /// # Arguments
    /// * `node` - The node to embed
    ///
    /// # Errors
    /// Returns error if:
    /// - Text extraction fails
    /// - Embedding generation fails
    /// - Database storage fails
    pub async fn embed_container(&self, node: &Node) -> Result<(), NodeServiceError> {
        let text = Self::extract_text_from_node(node);

        if text.trim().is_empty() {
            tracing::debug!("Skipping empty node: {}", node.id);
            return Ok(());
        }

        // Generate embedding using NLP engine
        let embedding = self.nlp_engine.generate_embedding(&text).map_err(|e| {
            NodeServiceError::SerializationError(format!("Embedding generation failed: {}", e))
        })?;

        // Store embedding directly as Vec<f32> (no blob conversion needed)
        self.store
            .update_embedding(&node.id, &embedding)
            .await
            .map_err(|e| {
                NodeServiceError::SerializationError(format!("Failed to store embedding: {}", e))
            })?;

        tracing::debug!("Embedded node: {} ({} floats)", node.id, embedding.len());
        Ok(())
    }

    /// Batch process stale embeddings
    ///
    /// Retrieves stale nodes from the database, generates embeddings in batch
    /// using the NLP engine for efficiency, and stores all results.
    ///
    /// # Arguments
    /// * `limit` - Maximum number of nodes to process (defaults to DEFAULT_BATCH_SIZE)
    ///
    /// # Returns
    /// Number of successfully embedded nodes
    ///
    /// # Errors
    /// Logs individual failures but continues processing. Returns error only if
    /// the database query for stale nodes fails.
    pub async fn batch_embed_containers(
        &self,
        limit: Option<usize>,
    ) -> Result<usize, NodeServiceError> {
        let batch_size = limit.unwrap_or(DEFAULT_BATCH_SIZE);

        // Get stale nodes from database
        let stale_nodes = self
            .store
            .get_nodes_with_stale_embeddings(Some(batch_size as i64))
            .await
            .map_err(|e| {
                NodeServiceError::SerializationError(format!("Failed to query stale nodes: {}", e))
            })?;

        if stale_nodes.is_empty() {
            tracing::debug!("No stale embeddings to process");
            return Ok(0);
        }

        tracing::info!("Processing {} stale embeddings", stale_nodes.len());

        // Extract text from all nodes (owned strings to avoid lifetime issues)
        let texts: Vec<String> = stale_nodes
            .iter()
            .map(Self::extract_text_from_node)
            .collect();

        // Create references for the batch call
        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

        // Generate embeddings in batch for efficiency
        let embeddings = self.nlp_engine.generate_batch(text_refs).map_err(|e| {
            NodeServiceError::SerializationError(format!(
                "Batch embedding generation failed: {}",
                e
            ))
        })?;

        // Store each embedding (no blob conversion needed)
        let mut success_count = 0;
        for (node, embedding) in stale_nodes.iter().zip(embeddings.iter()) {
            match self.store.update_embedding(&node.id, embedding).await {
                Ok(_) => {
                    success_count += 1;
                    tracing::debug!("Embedded node: {} ({} floats)", node.id, embedding.len());
                }
                Err(e) => {
                    tracing::error!("Failed to store embedding for node {}: {}", node.id, e);
                    // Continue processing other nodes
                }
            }
        }

        tracing::info!(
            "Successfully embedded {}/{} nodes",
            success_count,
            stale_nodes.len()
        );
        Ok(success_count)
    }
}
