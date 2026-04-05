//! Root-Aggregate Embedding Service (Issue #729, refactored in Issue #1018)
//!
//! ## Overview
//!
//! This service implements root-aggregate embedding for semantic search:
//! - Only ROOT nodes (no parent edge) of embeddable types get embedded
//! - Embeddings represent the semantic content of the entire subtree
//! - Uses the dedicated `embedding` table (not node.embedding_vector)
//! - Supports chunking for content > 512 tokens
//!
//! ## Behavior-Driven Embeddability (Issue #1018)
//!
//! Whether a node is embeddable is determined by its `NodeBehavior::get_embeddable_content()`.
//! No hardcoded type list. Content extraction uses a two-phase approach:
//! - Phase 1 (sync): `behavior.get_embeddable_content(node)` — the node's own content
//! - Phase 2 (async): `behavior.get_aggregated_content(node, accessor)` — child aggregation
//!
//! ## Queue System
//!
//! The embedding queue is managed via the `embedding` table's `stale` flag:
//! - New root nodes get a stale marker created
//! - Content changes mark existing embeddings as stale
//! - Background processor re-embeds stale entries

use crate::behaviors::{CustomNodeBehavior, NodeBehavior, NodeBehaviorRegistry};
use crate::db::SurrealStore;
use crate::models::{EmbeddingConfig, EmbeddingSearchResult, NewEmbedding, Node};
use crate::services::error::NodeServiceError;
use crate::services::{NodeAccessor, SearchScope};
use nodespace_nlp_engine::EmbeddingService;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;

// Re-export embedding dimension from nlp-engine as single source of truth
pub use nodespace_nlp_engine::EMBEDDING_DIMENSION;

/// Default batch size for processing stale embeddings
pub const DEFAULT_BATCH_SIZE: usize = 50;

/// Maximum depth for parent chain traversal (safety limit to prevent infinite loops)
pub const MAX_PARENT_CHAIN_DEPTH: usize = 100;

/// Title keyword boost added to composite score when any query term matches the node title (Issue #936)
///
/// Applied as an additive bonus: `composite_score + TITLE_BOOST` when a match is found.
/// Start at 0.1; tune against benchmark if needed.
///
/// Note: This constant lives here rather than in `surreal_store.rs` because the boost is
/// applied in Rust post-FETCH (after `search_embeddings` returns nodes with titles attached).
/// SurrealDB SQL cannot reference Rust constants, and the title field is only available once
/// the node is fetched — so the boost must be Rust-side, not SQL-side.
pub const TITLE_BOOST: f64 = 0.1;

/// Root-aggregate embedding service (Issue #1018: behavior-driven)
///
/// Manages semantic embeddings using the root-aggregate model where only
/// root nodes get embedded. Whether a node is embeddable is decided by its
/// `NodeBehavior::get_embeddable_content()` — no hardcoded type list.
///
/// Content extraction uses a two-phase approach:
/// - Phase 1 (sync): `behavior.get_embeddable_content(node)` — the node's own content
/// - Phase 2 (async): `behavior.get_aggregated_content(node, accessor)` — child aggregation
pub struct NodeEmbeddingService {
    /// NLP engine for generating embeddings
    nlp_engine: Arc<EmbeddingService>,
    /// SurrealDB store for persisting embeddings and search queries
    store: Arc<SurrealStore>,
    /// Read-only node accessor (backed by NodeService) for behavior-driven content extraction
    node_accessor: Arc<dyn NodeAccessor>,
    /// Behavior registry for looking up node type behaviors
    behaviors: Arc<NodeBehaviorRegistry>,
    /// Configuration for embedding behavior
    config: EmbeddingConfig,
}

impl NodeEmbeddingService {
    /// Create a new NodeEmbeddingService with behavior-driven content extraction (Issue #1018)
    ///
    /// # Arguments
    /// * `nlp_engine` - The NLP engine for generating embeddings
    /// * `store` - The SurrealDB store for persisting embeddings
    /// * `node_accessor` - Read-only accessor (typically NodeService) for fetching nodes
    /// * `behaviors` - Behavior registry for node type lookup
    pub fn new(
        nlp_engine: Arc<EmbeddingService>,
        store: Arc<SurrealStore>,
        node_accessor: Arc<dyn NodeAccessor>,
        behaviors: Arc<NodeBehaviorRegistry>,
    ) -> Self {
        tracing::info!("NodeEmbeddingService initialized with behavior-driven model (Issue #1018)");
        Self {
            nlp_engine,
            store,
            node_accessor,
            behaviors,
            config: EmbeddingConfig::default(),
        }
    }

    /// Create with custom configuration
    pub fn with_config(
        nlp_engine: Arc<EmbeddingService>,
        store: Arc<SurrealStore>,
        node_accessor: Arc<dyn NodeAccessor>,
        behaviors: Arc<NodeBehaviorRegistry>,
        config: EmbeddingConfig,
    ) -> Self {
        tracing::info!(
            "NodeEmbeddingService initialized with custom config (debounce: {}s)",
            config.debounce_duration_secs
        );
        Self {
            nlp_engine,
            store,
            node_accessor,
            behaviors,
            config,
        }
    }

    /// Get reference to the NLP engine
    pub fn nlp_engine(&self) -> &Arc<EmbeddingService> {
        &self.nlp_engine
    }

    /// Get reference to the SurrealDB store
    pub fn store(&self) -> &Arc<SurrealStore> {
        &self.store
    }

    /// Get reference to the embedding configuration
    pub fn config(&self) -> &EmbeddingConfig {
        &self.config
    }

    /// Get the behavior for a node type, falling back to CustomNodeBehavior
    fn behavior_for(&self, node_type: &str) -> Arc<dyn NodeBehavior> {
        self.behaviors
            .get(node_type)
            .unwrap_or_else(|| Arc::new(CustomNodeBehavior::new(node_type)))
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

    // =========================================================================
    // Behavior-Driven Content Extraction (Issue #1018)
    // =========================================================================

    /// Extract full embeddable content for a root node using its behavior.
    ///
    /// Two-phase approach per ADR-029:
    /// 1. `behavior.get_embeddable_content(node)` — sync, the node's own content
    /// 2. `behavior.get_aggregated_content(node, accessor)` — async, child aggregation
    ///
    /// Also prepends the node title if present (Issue #936 title boost).
    ///
    /// Returns `None` if the behavior says this node is not embeddable.
    async fn extract_content_for_embedding(
        &self,
        node: &Node,
    ) -> Result<Option<String>, NodeServiceError> {
        let behavior = self.behavior_for(&node.node_type);

        // Phase 1: node's own content (sync, no I/O)
        let own_content = behavior.get_embeddable_content(node);
        if own_content.is_none() {
            return Ok(None);
        }

        // Phase 2: aggregated content from children (async, optional)
        let aggregated = behavior
            .get_aggregated_content(node, self.node_accessor.as_ref())
            .await;

        // Build full content: title + own + aggregated
        let mut parts = Vec::new();

        // Prepend title so it is included in the embedding (Issue #936)
        if let Some(ref title) = node.title {
            if !title.trim().is_empty() {
                parts.push(title.clone());
            }
        }

        if let Some(own) = own_content {
            if !own.trim().is_empty() {
                parts.push(own);
            }
        }

        if let Some(agg) = aggregated {
            if !agg.trim().is_empty() {
                parts.push(agg);
            }
        }

        if parts.is_empty() {
            return Ok(None);
        }

        let mut full_content = parts.join("\n\n");

        // Check size limit
        if full_content.len() > self.config.max_content_size {
            tracing::warn!(
                "Content for {} exceeds max size ({} > {}). Truncating.",
                node.id,
                full_content.len(),
                self.config.max_content_size
            );
            let truncate_idx =
                Self::find_char_boundary(&full_content, self.config.max_content_size);
            full_content = full_content[..truncate_idx].to_string();
        }

        Ok(Some(full_content))
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
    /// Uses conservative token counting to ensure chunks never exceed the
    /// model's token limit. The `chars_per_token_estimate` config controls
    /// the character-to-token ratio. Overlaps chunks by `overlap_tokens`
    /// to maintain context across boundaries.
    ///
    /// This function is UTF-8 safe - it never splits in the middle of a
    /// multi-byte character (like emojis).
    fn chunk_content(&self, content: &str) -> Vec<(i32, i32, String)> {
        // Use configured chars_per_token estimate (default: 3)
        // BGE models typically tokenize at ~3-4 chars/token, but technical content
        // with code, markdown, and special characters can be closer to 2.5.
        let chars_per_token = self.config.chars_per_token_estimate;
        let max_chars = self.config.max_tokens_per_chunk * chars_per_token;
        let overlap_chars = self.config.overlap_tokens * chars_per_token;

        if content.len() <= max_chars {
            // Single chunk
            return vec![(0, content.len() as i32, content.to_string())];
        }

        let mut chunks = Vec::new();
        let mut start = 0;

        while start < content.len() {
            // Find the byte index that's at most max_chars from start,
            // but ensure it's on a valid UTF-8 character boundary
            let end = Self::find_char_boundary(content, (start + max_chars).min(content.len()));

            // Try to find a good break point (newline or space)
            // All searches use rfind which returns byte positions within the slice
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

            // Move forward with overlap, ensuring we land on a char boundary
            if actual_end >= content.len() {
                // We've processed the entire content, exit the loop
                break;
            }

            // Calculate where to start the next chunk with overlap
            // Ensure we advance at least (chunk_size - overlap) to prevent infinite loops
            let min_advance = max_chars.saturating_sub(overlap_chars).max(1);
            let next_start = start + min_advance;
            start = Self::find_char_boundary(content, next_start);
        }

        chunks
    }

    /// Find the nearest valid UTF-8 character boundary at or before the given byte index.
    ///
    /// This prevents panics when slicing strings with multi-byte characters (emojis, etc.)
    fn find_char_boundary(s: &str, mut index: usize) -> usize {
        if index >= s.len() {
            return s.len();
        }
        // Walk backwards until we find a valid char boundary
        while index > 0 && !s.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    // =========================================================================
    // Embedding Generation
    // =========================================================================

    /// Generate and store embeddings for a root node
    ///
    /// This is the main entry point for embedding a root node's content.
    /// Uses behavior-driven content extraction (Issue #1018):
    /// 1. `behavior.get_embeddable_content()` determines if node is embeddable
    /// 2. `behavior.get_aggregated_content()` gathers child content
    /// 3. Chunks, generates vectors, and stores in the embedding table
    pub async fn embed_root_node(&self, root_id: &str) -> Result<(), NodeServiceError> {
        // Get root node via accessor (applies business rules: mentions, migrations)
        let root = self
            .node_accessor
            .get_node(root_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(root_id))?;

        // Behavior-driven content extraction (replaces should_embed_root + aggregate_subtree_content)
        let content = match self.extract_content_for_embedding(&root).await? {
            Some(c) => c,
            None => {
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
        };

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

            let chunk_info = crate::models::ChunkInfo {
                chunk_index: idx as i32,
                chunk_start: start,
                chunk_end: end,
                total_chunks,
            };
            new_embeddings.push(NewEmbedding::chunk(
                root_id,
                vector,
                chunk_info,
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
    /// Uses per-root debounce: only processes embeddings that were marked stale
    /// more than `debounce_duration_secs` ago (default 30s), allowing rapid
    /// changes to accumulate before processing.
    pub async fn process_stale_embeddings(
        &self,
        limit: Option<usize>,
    ) -> Result<usize, NodeServiceError> {
        let batch_size = limit.unwrap_or(DEFAULT_BATCH_SIZE);

        // Get stale root IDs from embedding table, filtered by debounce duration
        let stale_ids = self
            .store
            .get_stale_embedding_root_ids(
                Some(batch_size as i64),
                self.config.debounce_duration_secs,
                self.config.max_retries,
            )
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
                        .record_embedding_error(&root_id, &e.to_string(), self.config.max_retries)
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

    /// Check if there are stale embeddings that haven't passed the debounce window yet
    ///
    /// Returns true if there are pending embeddings that will need processing
    /// after the debounce period expires.
    pub async fn has_pending_stale_embeddings(&self) -> Result<bool, NodeServiceError> {
        self.store
            .has_pending_stale_embeddings(
                self.config.debounce_duration_secs,
                self.config.max_retries,
            )
            .await
            .map_err(|e| {
                NodeServiceError::query_failed(format!(
                    "Failed to check pending stale embeddings: {}",
                    e
                ))
            })
    }

    // =========================================================================
    // Queue Management
    // =========================================================================

    /// Queue a node for embedding
    ///
    /// If the node is a root of an embeddable type, marks its embedding as stale.
    /// If the node is a child, finds its root and marks that as stale.
    /// Embeddability is determined by `NodeBehavior::get_embeddable_content()` (Issue #1018).
    pub async fn queue_for_embedding(&self, node_id: &str) -> Result<(), NodeServiceError> {
        // Find the root of this node's tree
        let root_id = self.find_root_id(node_id).await?;

        // Get the root node to check its type via behavior
        let root = match self.node_accessor.get_node(&root_id).await? {
            Some(node) => node,
            None => {
                tracing::debug!("Root node {} not found, skipping embedding queue", root_id);
                return Ok(());
            }
        };

        // Check if root type is embeddable via behavior (replaces is_embeddable_type)
        let behavior = self.behavior_for(&root.node_type);
        if behavior.get_embeddable_content(&root).is_none() {
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

    /// Search for nodes using hybrid BM25 + KNN scoring (Issue #951)
    ///
    /// Runs BM25 full-text search and KNN vector search in parallel, then tiers results:
    /// - **Tier 1** (highest confidence): roots in BOTH BM25 and KNN results
    /// - **Tier 2**: roots in KNN only (semantic relevance without keyword match)
    /// - **Tier 3**: roots in BM25 only (keyword match without semantic relevance)
    ///
    /// Within each tier, results are sorted by composite score (existing density formula).
    /// This avoids fragile score normalization between BM25 and cosine similarity.
    pub async fn semantic_search(
        &self,
        query: &str,
        limit: usize,
        threshold: f32,
    ) -> Result<Vec<EmbeddingSearchResult>, NodeServiceError> {
        let total_start = std::time::Instant::now();

        if query.trim().is_empty() {
            return Err(NodeServiceError::invalid_update(
                "Search query cannot be empty",
            ));
        }

        // Generate query embedding (blocking, so do before spawning parallel tasks)
        let embed_start = std::time::Instant::now();
        let query_vector = self.nlp_engine.generate_embedding(query).map_err(|e| {
            NodeServiceError::SerializationError(format!(
                "Failed to generate query embedding: {}",
                e
            ))
        })?;
        let embed_time = embed_start.elapsed();

        // Run BM25 and KNN searches in parallel (Issue #951)
        // BM25 is O(log n) via inverted index — sub-millisecond
        // KNN is ~50-100ms via HNSW — total latency = max(bm25, knn) ≈ same as before
        let search_start = std::time::Instant::now();
        // With tokenized OR search, results are already ranked by combined BM25 score so
        // fewer candidates are needed (top roots bubble up). 2x gives enough headroom after
        // root resolution deduplication without the cost of resolving 5x nodes.
        let bm25_limit = (limit as i64) * 2;
        let (knn_results, bm25_roots) = tokio::join!(
            self.store
                .search_embeddings(&query_vector, limit as i64, Some(threshold as f64)),
            self.store.bm25_search_roots(query, bm25_limit)
        );
        let search_time = search_start.elapsed();

        let mut knn_results = knn_results
            .map_err(|e| NodeServiceError::query_failed(format!("KNN search failed: {}", e)))?;
        let bm25_roots = bm25_roots
            .map_err(|e| NodeServiceError::query_failed(format!("BM25 search failed: {}", e)))?;

        // Apply title keyword boost (Issue #936) to KNN results
        // Punctuation is stripped from tokens so queries like "persistence?" still match.
        let query_terms: Vec<String> = query
            .split_whitespace()
            .map(|t| {
                t.trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase()
            })
            .filter(|t| !t.is_empty())
            .collect();
        for result in &mut knn_results {
            if let Some(ref node) = result.node {
                if let Some(ref title) = node.title {
                    let title_lower = title.to_lowercase();
                    if query_terms
                        .iter()
                        .any(|term| title_lower.contains(term.as_str()))
                    {
                        result.score += TITLE_BOOST;
                    }
                }
            }
        }

        // Tier results by intersection signal (Issue #951)
        //
        // Tier 1: roots present in BOTH KNN and BM25 → highest confidence
        // Tier 2: roots in KNN only → semantic relevance alone
        // Tier 3: roots in BM25 only → keyword match only (no KNN embedding yet, or below threshold)
        //
        // Within each tier: sort by composite score (descending).
        let (mut tier1, mut tier2): (Vec<EmbeddingSearchResult>, Vec<EmbeddingSearchResult>) =
            knn_results
                .into_iter()
                .partition(|r| bm25_roots.contains(&r.node_id));

        tier1.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        tier2.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Build KNN node ID set for tier 3 filtering
        let knn_node_ids: HashSet<String> = tier1
            .iter()
            .chain(tier2.iter())
            .map(|r| r.node_id.clone())
            .collect();

        // Tier 3: BM25-only roots (not in KNN results)
        // These need node data fetched separately since they bypassed the KNN+FETCH query.
        // Only include if we have remaining capacity.
        let knn_count = tier1.len() + tier2.len();
        let tier2_count = tier2.len();
        let mut results: Vec<EmbeddingSearchResult> = Vec::with_capacity(limit);
        results.extend(tier1);
        results.extend(tier2);

        let mut tier3_count = 0usize;
        if knn_count < limit {
            let remaining = limit - knn_count;
            // Sort by node_id for deterministic ordering within Tier 3
            // (no embedding score available to rank by, so stable key prevents non-reproducible results)
            let mut bm25_only_roots: Vec<String> = bm25_roots
                .into_iter()
                .filter(|id| !knn_node_ids.contains(id))
                .collect();
            bm25_only_roots.sort();
            bm25_only_roots.truncate(remaining);

            if !bm25_only_roots.is_empty() {
                // Fetch node data for BM25-only results (no embedding score available)
                // Score is set to 0.0 to indicate keyword-only match (ranked last within tier 3)
                for root_id in bm25_only_roots {
                    if let Ok(Some(node)) = self.store.get_node(&root_id).await {
                        results.push(EmbeddingSearchResult {
                            node_id: root_id,
                            score: 0.0,
                            max_similarity: 0.0,
                            matching_chunks: 0,
                            node: Some(node),
                        });
                        tier3_count += 1;
                    }
                }
            }
        }

        let total_time = total_start.elapsed();
        let intersection_count = results.len() - tier3_count - tier2_count;

        tracing::debug!(
            "HYBRID SEARCH PROFILE: total={:?} | embedding={:?} search={:?} | results={} (tier1={} tier2={} tier3={}) query='{}'",
            total_time,
            embed_time,
            search_time,
            results.len(),
            intersection_count,
            tier2_count,
            tier3_count,
            &query[..query.len().min(50)]
        );

        Ok(results)
    }

    /// Search and return full nodes
    ///
    /// Convenience method that fetches the full Node objects for search results.
    /// Search with scope filtering (Issue #1018)
    ///
    /// Wraps `semantic_search` and applies post-result filtering based on the
    /// `SearchScope`. The scope determines which node types are included in
    /// results — callers declare intent rather than enumerating types.
    ///
    /// Defaults to `SearchScope::Knowledge` when no scope is provided.
    pub async fn semantic_search_scoped(
        &self,
        query: &str,
        limit: usize,
        threshold: f32,
        scope: &SearchScope,
    ) -> Result<Vec<EmbeddingSearchResult>, NodeServiceError> {
        // Fetch extra results to compensate for post-filtering
        let overfetch = limit * 2;
        let mut results = self.semantic_search(query, overfetch, threshold).await?;

        // Apply scope filter
        results.retain(|r| {
            if let Some(ref node) = r.node {
                Self::matches_scope(&node.node_type, scope)
            } else {
                // No node data — keep the result (we can't filter it)
                true
            }
        });

        results.truncate(limit);
        Ok(results)
    }

    /// Check if a node type matches the given search scope
    pub fn matches_scope(node_type: &str, scope: &SearchScope) -> bool {
        match scope {
            SearchScope::Knowledge => matches!(
                node_type,
                "text" | "header" | "code-block" | "schema" | "table"
            ),
            SearchScope::Conversations => node_type == "ai-chat",
            SearchScope::Everything => true,
            SearchScope::Custom {
                include_types,
                exclude_types,
            } => {
                if !include_types.is_empty() && !include_types.iter().any(|t| t == node_type) {
                    return false;
                }
                if exclude_types.iter().any(|t| t == node_type) {
                    return false;
                }
                true
            }
        }
    }

    /// Returns nodes with their composite relevance scores (which account for
    /// both similarity and breadth of matching chunks).
    ///
    /// PERFORMANCE: Node data is now fetched inline with the search query using
    /// SurrealDB's FETCH clause, eliminating N+1 query overhead.
    pub async fn semantic_search_nodes(
        &self,
        query: &str,
        limit: usize,
        threshold: f32,
    ) -> Result<Vec<(Node, f64)>, NodeServiceError> {
        let total_start = std::time::Instant::now();

        let results = self.semantic_search(query, limit, threshold).await?;

        // Nodes are now included in search results via FETCH - no separate queries needed
        let nodes_with_scores: Vec<(Node, f64)> = results
            .into_iter()
            .filter_map(|result| result.node.map(|node| (node, result.score)))
            .collect();

        let total_time = total_start.elapsed();
        tracing::debug!(
            "SEMANTIC SEARCH NODES: total={:?} | nodes={} (inline fetch)",
            total_time,
            nodes_with_scores.len()
        );

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
    use serde_json::json;

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
        let hash1 = NodeEmbeddingService::compute_content_hash("hello");
        let hash2 = NodeEmbeddingService::compute_content_hash("hello");
        let hash3 = NodeEmbeddingService::compute_content_hash("world");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA256 hex = 64 chars
    }

    #[test]
    fn test_find_char_boundary_ascii() {
        let s = "hello world";
        // ASCII characters are all single-byte, so any index is valid
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 0), 0);
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 5), 5);
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 100), s.len());
    }

    #[test]
    fn test_find_char_boundary_emoji() {
        // ✅ is 3 bytes (E2 9C 85)
        let s = "test ✅ done";
        // Byte positions: t(0) e(1) s(2) t(3) ' '(4) ✅(5,6,7) ' '(8) d(9) o(10) n(11) e(12)

        // Index 5 is start of emoji - valid
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 5), 5);

        // Index 6 is inside emoji - should walk back to 5
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 6), 5);

        // Index 7 is inside emoji - should walk back to 5
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 7), 5);

        // Index 8 is after emoji (space) - valid
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 8), 8);
    }

    #[test]
    fn test_find_char_boundary_multiple_emojis() {
        // Test with multiple multi-byte characters
        // 🎉 is 4 bytes, ✅ is 3 bytes
        let s = "🎉 done ✅";
        // Byte positions: 🎉(0-3) ' '(4) d(5) o(6) n(7) e(8) ' '(9) ✅(10-12)

        // Index inside first emoji
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 2), 0);

        // Index 4 (space after emoji)
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 4), 4);

        // Index inside second emoji
        assert_eq!(NodeEmbeddingService::find_char_boundary(s, 11), 10);
    }

    /// Helper struct for testing chunk_content without full service initialization
    struct ChunkTester {
        config: EmbeddingConfig,
    }

    impl ChunkTester {
        fn new() -> Self {
            Self {
                config: EmbeddingConfig::default(),
            }
        }

        fn chunk_content(&self, content: &str) -> Vec<(i32, i32, String)> {
            let chars_per_token = self.config.chars_per_token_estimate;
            let max_chars = self.config.max_tokens_per_chunk * chars_per_token;
            let overlap_chars = self.config.overlap_tokens * chars_per_token;

            if content.len() <= max_chars {
                return vec![(0, content.len() as i32, content.to_string())];
            }

            let mut chunks = Vec::new();
            let mut start = 0;

            while start < content.len() {
                let end = Self::find_char_boundary(content, (start + max_chars).min(content.len()));

                let actual_end = if end < content.len() {
                    if let Some(pos) = content[start..end].rfind("\n\n") {
                        start + pos + 2
                    } else if let Some(pos) = content[start..end].rfind(". ") {
                        start + pos + 2
                    } else if let Some(pos) = content[start..end].rfind(' ') {
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

                if actual_end >= content.len() {
                    break;
                }

                let min_advance = max_chars.saturating_sub(overlap_chars).max(1);
                let next_start = start + min_advance;
                start = Self::find_char_boundary(content, next_start);
            }

            chunks
        }

        fn find_char_boundary(s: &str, mut index: usize) -> usize {
            if index >= s.len() {
                return s.len();
            }
            while index > 0 && !s.is_char_boundary(index) {
                index -= 1;
            }
            index
        }
    }

    #[test]
    fn test_chunk_content_no_infinite_loop() {
        // Regression test for infinite loop bug:
        // When content is just over 2 chunks worth, the overlap calculation
        // could cause start to only advance by 1 byte, creating hundreds of chunks.
        let tester = ChunkTester::new();
        let config = &tester.config;

        // Create content that's about 2.5 chunks worth
        let max_chars = config.max_tokens_per_chunk * config.chars_per_token_estimate;
        let content_len = max_chars * 2 + (max_chars / 2);
        let content = "x".repeat(content_len);

        let chunks = tester.chunk_content(&content);

        // With default config (512 tokens, 100 overlap, 3 chars/token):
        // - max_chars = 1536, overlap_chars = 300
        // - Content ~3840 chars should produce ~3-4 chunks, not hundreds
        assert!(
            chunks.len() <= 10,
            "Expected at most 10 chunks for {}B content, got {} chunks",
            content_len,
            chunks.len()
        );

        // Verify chunks cover the entire content
        assert_eq!(
            chunks.first().unwrap().0,
            0,
            "First chunk should start at 0"
        );
        assert_eq!(
            chunks.last().unwrap().1 as usize,
            content.len(),
            "Last chunk should end at content length"
        );
    }

    #[test]
    fn test_chunk_content_single_chunk() {
        let tester = ChunkTester::new();
        let short_content = "Hello world, this is a short test.";

        let chunks = tester.chunk_content(short_content);

        assert_eq!(chunks.len(), 1, "Short content should be single chunk");
        assert_eq!(chunks[0].0, 0);
        assert_eq!(chunks[0].1 as usize, short_content.len());
        assert_eq!(chunks[0].2, short_content);
    }

    #[test]
    fn test_chunk_content_breaks_at_sentences() {
        let tester = ChunkTester::new();
        let config = &tester.config;
        let max_chars = config.max_tokens_per_chunk * config.chars_per_token_estimate;

        // Create content with a sentence boundary before max_chars
        let first_part = "x".repeat(max_chars - 100);
        let sentence_break = ". This is a new sentence. ";
        let second_part = "y".repeat(max_chars);
        let content = format!("{}{}{}", first_part, sentence_break, second_part);

        let chunks = tester.chunk_content(&content);

        // Should break at the sentence boundary
        assert!(chunks.len() >= 2, "Should have at least 2 chunks");
        // First chunk should end after the sentence break
        assert!(
            chunks[0].2.ends_with(". "),
            "First chunk should end at sentence boundary"
        );
    }

    // =========================================================================
    // Behavior-Driven Embedding Decision Tests (Issue #1018)
    // =========================================================================

    /// Mock NodeAccessor for testing extract_content_for_embedding without a database.
    struct MockNodeAccessor {
        nodes: std::collections::HashMap<String, Node>,
        children: std::collections::HashMap<String, Vec<Node>>,
    }

    impl MockNodeAccessor {
        fn new() -> Self {
            Self {
                nodes: std::collections::HashMap::new(),
                children: std::collections::HashMap::new(),
            }
        }

        fn add_node(&mut self, node: Node) {
            self.nodes.insert(node.id.clone(), node);
        }

        fn set_children(&mut self, parent_id: &str, kids: Vec<Node>) {
            self.children.insert(parent_id.to_string(), kids);
        }
    }

    #[async_trait::async_trait]
    impl NodeAccessor for MockNodeAccessor {
        async fn get_node(
            &self,
            id: &str,
        ) -> Result<Option<Node>, crate::services::error::NodeServiceError> {
            Ok(self.nodes.get(id).cloned())
        }

        async fn get_children(
            &self,
            parent_id: &str,
        ) -> Result<Vec<Node>, crate::services::error::NodeServiceError> {
            Ok(self.children.get(parent_id).cloned().unwrap_or_default())
        }

        async fn get_nodes(
            &self,
            ids: &[&str],
        ) -> Result<Vec<Node>, crate::services::error::NodeServiceError> {
            Ok(ids
                .iter()
                .filter_map(|id| self.nodes.get(*id).cloned())
                .collect())
        }
    }

    /// Verify that extract_content_for_embedding respects behavior decisions:
    /// - Embeddable types (text, header, code-block) return Some
    /// - Non-embeddable types (task, date, collection) return None
    /// - ai-chat extracts messages, not node content
    #[tokio::test]
    async fn test_behavior_driven_embedding_decision() {
        let _accessor = Arc::new(MockNodeAccessor::new());
        let behaviors = Arc::new(NodeBehaviorRegistry::new());

        // We cannot construct a full NodeEmbeddingService without a real NLP engine
        // and SurrealStore, so we test the behavior dispatch logic directly by
        // calling behavior.get_embeddable_content() through the registry,
        // which is exactly what extract_content_for_embedding delegates to.

        // --- Embeddable types ---

        let text_node = Node::new(
            "text".to_string(),
            "Knowledge content".to_string(),
            json!({}),
        );
        let text_behavior = behaviors.get("text").unwrap();
        assert!(
            text_behavior.get_embeddable_content(&text_node).is_some(),
            "text should be embeddable via behavior"
        );

        let header_node = Node::new(
            "header".to_string(),
            "## Architecture Overview".to_string(),
            json!({"headerLevel": 2}),
        );
        let header_behavior = behaviors.get("header").unwrap();
        assert!(
            header_behavior
                .get_embeddable_content(&header_node)
                .is_some(),
            "header should be embeddable via behavior"
        );

        let code_node = Node::new(
            "code-block".to_string(),
            "```python\nprint('hello')".to_string(),
            json!({"language": "python"}),
        );
        let code_behavior = behaviors.get("code-block").unwrap();
        assert!(
            code_behavior.get_embeddable_content(&code_node).is_some(),
            "code-block should be embeddable via behavior"
        );

        // --- Non-embeddable types ---

        let task_node = Node::new(
            "task".to_string(),
            "Fix the bug".to_string(),
            json!({"task": {"status": "open"}}),
        );
        let task_behavior = behaviors.get("task").unwrap();
        assert!(
            task_behavior.get_embeddable_content(&task_node).is_none(),
            "task should NOT be embeddable — behavior returns None"
        );

        let date_node = Node::new_with_id(
            "2025-03-01".to_string(),
            "date".to_string(),
            "2025-03-01".to_string(),
            json!({}),
        );
        let date_behavior = behaviors.get("date").unwrap();
        assert!(
            date_behavior.get_embeddable_content(&date_node).is_none(),
            "date should NOT be embeddable — behavior returns None"
        );

        let coll_node = Node::new(
            "collection".to_string(),
            "Engineering".to_string(),
            json!({}),
        );
        let coll_behavior = behaviors.get("collection").unwrap();
        assert!(
            coll_behavior.get_embeddable_content(&coll_node).is_none(),
            "collection should NOT be embeddable — behavior returns None"
        );

        // --- ai-chat: message-based extraction ---

        let chat_node = Node::new(
            "ai-chat".to_string(),
            "Chat about testing".to_string(),
            json!({
                "messages": [
                    {"role": "user", "content": "How do I write tests?"},
                    {"role": "tool_call", "tool": "search", "result_summary": "3 results"},
                    {"role": "assistant", "content": "Here is a testing guide."}
                ]
            }),
        );
        let chat_behavior = behaviors.get("ai-chat").unwrap();
        let chat_content = chat_behavior.get_embeddable_content(&chat_node);
        assert!(
            chat_content.is_some(),
            "ai-chat with messages should be embeddable"
        );
        let text = chat_content.unwrap();
        assert!(
            text.contains("How do I write tests?"),
            "Should include user message"
        );
        assert!(
            text.contains("Here is a testing guide."),
            "Should include assistant message"
        );
        assert!(
            !text.contains("search"),
            "Should exclude tool_call messages"
        );

        // --- CustomNodeBehavior fallback ---

        let custom_behavior = CustomNodeBehavior::new("invoice");
        let custom_node = Node::new("invoice".to_string(), "INV-2025-001".to_string(), json!({}));
        assert!(
            custom_behavior
                .get_embeddable_content(&custom_node)
                .is_some(),
            "Custom types use default trait impl — embeddable if non-empty"
        );

        // Verify the behavior_for() fallback logic: unknown types get CustomNodeBehavior
        assert!(
            behaviors.get("nonexistent_type").is_none(),
            "Registry should not have a behavior for unknown types"
        );
        // The embedding service uses behavior_for() which falls back to CustomNodeBehavior
        let fallback = CustomNodeBehavior::new("nonexistent_type");
        let unknown_node = Node::new(
            "nonexistent_type".to_string(),
            "Some content".to_string(),
            json!({}),
        );
        assert!(
            fallback.get_embeddable_content(&unknown_node).is_some(),
            "Fallback CustomNodeBehavior should make unknown types embeddable if non-empty"
        );
    }

    /// Verify the MockNodeAccessor works correctly (validates test infrastructure).
    #[tokio::test]
    async fn test_mock_node_accessor() {
        let mut accessor = MockNodeAccessor::new();

        let node = Node::new("text".to_string(), "Test content".to_string(), json!({}));
        let node_id = node.id.clone();
        accessor.add_node(node.clone());

        let child1 = Node::new("text".to_string(), "Child 1".to_string(), json!({}));
        let child2 = Node::new("text".to_string(), "Child 2".to_string(), json!({}));
        accessor.set_children(&node_id, vec![child1.clone(), child2.clone()]);

        // get_node
        let retrieved = accessor.get_node(&node_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().content, "Test content");

        // get_node for unknown ID
        let unknown = accessor.get_node("no-such-id").await.unwrap();
        assert!(unknown.is_none());

        // get_children
        let children = accessor.get_children(&node_id).await.unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].content, "Child 1");
        assert_eq!(children[1].content, "Child 2");

        // get_children for node with no children
        let empty_children = accessor.get_children("no-children").await.unwrap();
        assert!(empty_children.is_empty());

        // get_nodes (batch)
        let id1 = child1.id.clone();
        let id2 = child2.id.clone();
        accessor.add_node(child1);
        accessor.add_node(child2);
        let batch = accessor
            .get_nodes(&[&id1, &id2, "nonexistent"])
            .await
            .unwrap();
        assert_eq!(batch.len(), 2, "Should return only existing nodes");
    }

    /// Verify that text behavior's get_aggregated_content() collects children
    /// via the NodeAccessor interface (the Phase 2 async aggregation).
    #[tokio::test]
    async fn test_text_behavior_aggregated_content_via_accessor() {
        let mut accessor = MockNodeAccessor::new();

        let parent = Node::new("text".to_string(), "Parent note".to_string(), json!({}));
        let parent_id = parent.id.clone();
        accessor.add_node(parent.clone());

        let child_text = Node::new("text".to_string(), "Child paragraph".to_string(), json!({}));
        let child_code = Node::new(
            "code-block".to_string(),
            "```rust\nlet x = 1;".to_string(),
            json!({"language": "rust"}),
        );
        // Task child should NOT contribute (task behavior returns None)
        let child_task = Node::new(
            "task".to_string(),
            "Buy milk".to_string(),
            json!({"task": {"status": "open"}}),
        );

        accessor.set_children(
            &parent_id,
            vec![child_text.clone(), child_code.clone(), child_task.clone()],
        );

        let behavior = crate::behaviors::TextNodeBehavior;
        let aggregated = behavior.get_aggregated_content(&parent, &accessor).await;

        assert!(
            aggregated.is_some(),
            "Should have aggregated content from children"
        );
        let text = aggregated.unwrap();
        assert!(
            text.contains("Child paragraph"),
            "Should include text child contribution"
        );
        assert!(
            text.contains("let x = 1"),
            "Should include code-block child contribution"
        );
        assert!(
            !text.contains("Buy milk"),
            "Should NOT include task child (task returns None for parent contribution)"
        );
    }
}
