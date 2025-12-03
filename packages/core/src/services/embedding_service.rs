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
//! - ✅ Two-level embeddability (Issue #573) - behavior-driven embedding decisions
//!
//! ## Two-Level Embeddability (Issue #573)
//!
//! The embedding service uses the NodeBehavior trait to determine:
//! 1. **Root Embeddability**: Whether a node should be embedded as a standalone unit
//! 2. **Parent Contribution**: What content a node contributes to its parent's embedding
//!
//! This enables intelligent embedding where:
//! - Text/header nodes are embeddable and contribute to parent embeddings
//! - Task nodes are NOT embedded (action items, not semantic content)
//! - Date nodes are NOT embedded (containers, not content)
//!
//! ## Out of Scope (Future Issues)
//!
//! - ⏳ Vector similarity search implementation (Issue #107 - SurrealDB native vector search)

use crate::behaviors::NodeBehaviorRegistry;
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
///
/// Generic over the database connection type `C` to support both local
/// embedded database (Db) and HTTP client connections.
pub struct NodeEmbeddingService<C = surrealdb::engine::local::Db>
where
    C: surrealdb::Connection,
{
    /// NLP engine for generating embeddings
    nlp_engine: Arc<EmbeddingService>,
    /// SurrealDB store for persisting embeddings
    store: Arc<SurrealStore<C>>,
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
        tracing::info!("NodeEmbeddingService initialized with SurrealDB and NLP engine");
        Self { nlp_engine, store }
    }

    /// Get reference to the NLP engine for direct embedding operations
    pub fn nlp_engine(&self) -> &Arc<EmbeddingService> {
        &self.nlp_engine
    }

    /// Get reference to the SurrealDB store
    pub fn store(&self) -> &Arc<SurrealStore<C>> {
        &self.store
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

    // =========================================================================
    // Two-Level Embeddability Methods (Issue #573)
    // =========================================================================

    /// Determine if a node should be embedded as a root node
    ///
    /// Uses the NodeBehavior trait to check if this node type is embeddable.
    ///
    /// Returns true if:
    /// - Node's behavior says it's embeddable (via get_embeddable_content)
    /// - Node content is not empty
    ///
    /// Returns false if:
    /// - Node behavior says it's not embeddable (e.g., tasks, dates)
    /// - Node content is empty
    ///
    /// # Arguments
    /// * `registry` - The behavior registry for looking up node type behaviors
    /// * `node` - The node to check
    ///
    /// # Examples
    /// ```rust,no_run
    /// # use nodespace_core::services::NodeEmbeddingService;
    /// # use nodespace_core::behaviors::NodeBehaviorRegistry;
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// # async fn example(service: &NodeEmbeddingService, registry: &NodeBehaviorRegistry) {
    /// let text_node = Node::new("text".to_string(), "Hello world".to_string(), json!({}));
    /// assert!(service.should_embed_node(registry, &text_node)); // Text is embeddable
    ///
    /// let task_node = Node::new("task".to_string(), "Buy milk".to_string(), json!({}));
    /// assert!(!service.should_embed_node(registry, &task_node)); // Task is NOT embeddable
    /// # }
    /// ```
    pub fn should_embed_node(&self, registry: &NodeBehaviorRegistry, node: &Node) -> bool {
        if let Some(behavior) = registry.get(&node.node_type) {
            behavior.get_embeddable_content(node).is_some()
        } else {
            // Unknown node type - use default behavior (embeddable if has content)
            !node.content.trim().is_empty()
        }
    }

    /// Build embeddable content by combining node + immediate children contributions
    ///
    /// Combines content from the node and its direct children:
    /// 1. Node's own embeddable content (if it's a root)
    /// 2. Each immediate child's contribution to this node
    ///
    /// Note: Only processes direct children (one level deep), not grandchildren.
    /// This keeps embeddings focused on the node's immediate context.
    ///
    /// The two-level system means:
    /// - Level 1: Root embeddability (should this node be embedded?)
    /// - Level 2: Parent contribution (what does this node add to parent's embedding?)
    ///
    /// # Arguments
    /// * `registry` - The behavior registry for looking up node type behaviors
    /// * `node` - The node to build content for
    ///
    /// # Returns
    /// Combined text with paragraph separation (double newlines)
    ///
    /// # Errors
    /// Returns error if database operations fail
    pub async fn build_embeddable_content(
        &self,
        registry: &NodeBehaviorRegistry,
        node: &Node,
    ) -> Result<String, NodeServiceError> {
        let mut parts = Vec::new();

        // Add node's own content if it should be embedded
        if let Some(behavior) = registry.get(&node.node_type) {
            if let Some(content) = behavior.get_embeddable_content(node) {
                parts.push(content);
            }
        } else if !node.content.trim().is_empty() {
            // Unknown node type - use content directly
            parts.push(node.content.clone());
        }

        // Get children and add their contributions
        let children = self.store.get_children(Some(&node.id)).await.map_err(|e| {
            NodeServiceError::SerializationError(format!("Failed to get children: {}", e))
        })?;

        for child in children {
            if let Some(child_behavior) = registry.get(&child.node_type) {
                // Add child's direct contribution to parent
                if let Some(contribution) = child_behavior.get_parent_contribution(&child) {
                    parts.push(contribution);
                }
            } else if !child.content.trim().is_empty() {
                // Unknown node type - contribute content directly
                parts.push(child.content.clone());
            }
        }

        Ok(parts.join("\n\n"))
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

    // =========================================================================
    // Two-Level Embeddability Versions (Issue #573)
    // =========================================================================

    /// Generate and store embedding for a single node with two-level embeddability
    ///
    /// Uses the NodeBehavior trait to determine:
    /// 1. Whether this node should be embedded (root embeddability)
    /// 2. What content to include from children (parent contribution)
    ///
    /// This method is behavior-aware and respects node type semantics:
    /// - Text nodes: Embedded with their children's contributions
    /// - Task nodes: Skipped (not semantically meaningful to embed)
    /// - Date nodes: Skipped (containers, not content)
    ///
    /// # Arguments
    /// * `registry` - The behavior registry for looking up node type behaviors
    /// * `node` - The node to embed
    ///
    /// # Errors
    /// Returns error if:
    /// - Building embeddable content fails (database error)
    /// - Embedding generation fails
    /// - Database storage fails
    pub async fn embed_container_with_behavior(
        &self,
        registry: &NodeBehaviorRegistry,
        node: &Node,
    ) -> Result<(), NodeServiceError> {
        // Check if this node should be embedded using behavior system
        if !self.should_embed_node(registry, node) {
            tracing::debug!(
                "Skipping non-embeddable node: {} (type: {})",
                node.id,
                node.node_type
            );
            return Ok(());
        }

        // Build embeddable content (node + children contributions)
        let text = self.build_embeddable_content(registry, node).await?;

        if text.trim().is_empty() {
            tracing::debug!("Skipping empty embeddable content for node: {}", node.id);
            return Ok(());
        }

        // Generate embedding using NLP engine
        let embedding = self.nlp_engine.generate_embedding(&text).map_err(|e| {
            NodeServiceError::SerializationError(format!("Embedding generation failed: {}", e))
        })?;

        // Store embedding directly as Vec<f32>
        self.store
            .update_embedding(&node.id, &embedding)
            .await
            .map_err(|e| {
                NodeServiceError::SerializationError(format!("Failed to store embedding: {}", e))
            })?;

        tracing::debug!(
            "Embedded node: {} ({} floats, {} chars of content)",
            node.id,
            embedding.len(),
            text.len()
        );
        Ok(())
    }

    /// Batch process stale embeddings with two-level embeddability
    ///
    /// Uses the NodeBehavior trait to determine which nodes should be embedded
    /// and what content to include. Non-embeddable nodes (tasks, dates) are
    /// skipped automatically.
    ///
    /// # Arguments
    /// * `registry` - The behavior registry for looking up node type behaviors
    /// * `limit` - Maximum number of nodes to process (defaults to DEFAULT_BATCH_SIZE)
    ///
    /// # Returns
    /// Number of successfully embedded nodes
    ///
    /// # Errors
    /// Logs individual failures but continues processing. Returns error only if
    /// the database query for stale nodes fails.
    pub async fn batch_embed_containers_with_behavior(
        &self,
        registry: &NodeBehaviorRegistry,
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

        tracing::info!(
            "Processing {} stale embeddings (with behavior)",
            stale_nodes.len()
        );

        // Filter to embeddable nodes and build their content
        let mut texts = Vec::new();
        let mut embeddable_nodes = Vec::new();

        for node in stale_nodes {
            if self.should_embed_node(registry, &node) {
                match self.build_embeddable_content(registry, &node).await {
                    Ok(text) if !text.trim().is_empty() => {
                        texts.push(text);
                        embeddable_nodes.push(node);
                    }
                    Ok(_) => {
                        tracing::debug!(
                            "Skipping empty content for node: {} (type: {})",
                            node.id,
                            node.node_type
                        );
                    }
                    Err(e) => {
                        tracing::warn!("Failed to build content for node {}: {}", node.id, e);
                    }
                }
            } else {
                tracing::debug!(
                    "Skipping non-embeddable node: {} (type: {})",
                    node.id,
                    node.node_type
                );
            }
        }

        if texts.is_empty() {
            tracing::debug!("No embeddable content found in stale nodes");
            return Ok(0);
        }

        // Create references for the batch call
        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

        // Generate embeddings in batch for efficiency
        let embeddings = self.nlp_engine.generate_batch(text_refs).map_err(|e| {
            NodeServiceError::SerializationError(format!(
                "Batch embedding generation failed: {}",
                e
            ))
        })?;

        // Store each embedding
        let mut success_count = 0;
        for (node, embedding) in embeddable_nodes.iter().zip(embeddings.iter()) {
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
            "Successfully embedded {}/{} embeddable nodes",
            success_count,
            embeddable_nodes.len()
        );
        Ok(success_count)
    }

    /// Search for nodes by semantic similarity
    ///
    /// Generates an embedding for the query text using the NLP engine,
    /// then searches the database for nodes with similar embeddings.
    ///
    /// # Arguments
    /// * `query` - Natural language search query
    /// * `limit` - Maximum number of results to return
    /// * `threshold` - Minimum similarity threshold (0.0-1.0)
    ///
    /// # Returns
    /// Vector of (Node, similarity_score) tuples, sorted by similarity descending
    ///
    /// # Errors
    /// Returns error if:
    /// - Embedding generation fails (NLP engine not initialized)
    /// - Database search fails
    pub async fn semantic_search(
        &self,
        query: &str,
        limit: usize,
        threshold: f32,
    ) -> Result<Vec<(Node, f64)>, NodeServiceError> {
        // Validate query
        if query.trim().is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Search query cannot be empty",
            ));
        }

        // Generate embedding for the query text using NLP engine
        let query_embedding = self.nlp_engine.generate_embedding(query).map_err(|e| {
            NodeServiceError::SerializationError(format!(
                "Failed to generate query embedding: {}",
                e
            ))
        })?;

        // Search database using the query embedding
        let results = self
            .store
            .search_by_embedding(&query_embedding, limit as i64, Some(threshold as f64))
            .await
            .map_err(|e| {
                NodeServiceError::SerializationError(format!("Semantic search failed: {}", e))
            })?;

        tracing::debug!(
            "Semantic search for '{}' returned {} results",
            query,
            results.len()
        );

        Ok(results)
    }
}
