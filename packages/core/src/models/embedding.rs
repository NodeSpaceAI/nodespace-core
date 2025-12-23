//! Embedding Model for Root-Aggregate Semantic Search
//!
//! This module defines the embedding record stored in the `embedding` table.
//! Each embedding represents the semantic content of a root node and its entire subtree.
//!
//! ## Key Concepts
//!
//! - **Root-Aggregate**: Only root nodes (no parent) get embedded; the embedding
//!   captures the semantic meaning of the entire document tree
//! - **Chunking**: Large content is split into overlapping chunks (512 token limit)
//! - **Staleness**: Embeddings track when they need re-generation
//! - **Error Tracking**: Failed embedding attempts are logged for diagnostics

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default embedding dimension for nomic-embed-text-v1.5 model (768 dimensions)
/// Note: The authoritative constant is `EMBEDDING_DIMENSION` in nlp-engine crate.
/// This is kept for database schema compatibility.
pub const DEFAULT_EMBEDDING_DIMENSION: i32 = 768;

/// Default model name (nomic-embed-text-v1.5 via llama.cpp)
pub const DEFAULT_MODEL_NAME: &str = "nomic-embed-text-v1.5";

/// Maximum tokens per chunk - model context limit (tokens)
pub const MAX_TOKENS_PER_CHUNK: usize = 512;

/// Token overlap between chunks - preserves context across boundaries (tokens)
pub const OVERLAP_TOKENS: usize = 100;

/// Conservative estimate of characters per token for chunking
/// BGE models typically tokenize at ~3-4 chars/token, but technical content
/// with code, markdown, and special characters can be closer to 2.5.
/// Using 3 provides a safety margin to prevent exceeding 512 token limit.
pub const CHARS_PER_TOKEN_ESTIMATE: usize = 3;

/// Embedding record stored in the `embedding` table
///
/// Represents the vector embedding for a root node's aggregated content.
/// Multiple records may exist for the same node if content exceeds chunk size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Embedding {
    /// Unique identifier for this embedding record
    pub id: String,

    /// Reference to the root node (SurrealDB record link format: "node:uuid")
    pub node: String,

    /// The embedding vector (array of floats)
    pub vector: Vec<f32>,

    /// Dimension of the vector (768 for nomic-embed-text-v1.5)
    #[serde(default = "default_dimension")]
    pub dimension: i32,

    /// Model used to generate the embedding
    #[serde(default = "default_model_name")]
    pub model_name: String,

    /// Chunk index (0 for single-chunk content, 0..N for multi-chunk)
    #[serde(default)]
    pub chunk_index: i32,

    /// Character position where this chunk starts in the aggregated content
    #[serde(default)]
    pub chunk_start: i32,

    /// Character position where this chunk ends (exclusive)
    pub chunk_end: Option<i32>,

    /// Total number of chunks for this node
    #[serde(default = "default_total_chunks")]
    pub total_chunks: i32,

    /// Hash of the aggregated content (for change detection)
    pub content_hash: Option<String>,

    /// Number of tokens in the chunk
    pub token_count: Option<i32>,

    /// Whether the embedding needs to be regenerated
    #[serde(default = "default_stale")]
    pub stale: bool,

    /// Number of failed embedding attempts
    #[serde(default)]
    pub error_count: i32,

    /// Most recent error message
    pub last_error: Option<String>,

    /// When the embedding was created
    pub created_at: DateTime<Utc>,

    /// When the embedding was last modified
    pub modified_at: DateTime<Utc>,
}

fn default_dimension() -> i32 {
    DEFAULT_EMBEDDING_DIMENSION
}

fn default_model_name() -> String {
    DEFAULT_MODEL_NAME.to_string()
}

fn default_total_chunks() -> i32 {
    1
}

fn default_stale() -> bool {
    true
}

/// Chunk positioning information for multi-chunk embeddings
#[derive(Debug, Clone)]
pub struct ChunkInfo {
    /// Chunk index (0 for single-chunk, 0..N for multi-chunk)
    pub chunk_index: i32,
    /// Character start position
    pub chunk_start: i32,
    /// Character end position
    pub chunk_end: i32,
    /// Total chunks for this node
    pub total_chunks: i32,
}

/// Parameters for creating a new embedding
#[derive(Debug, Clone)]
pub struct NewEmbedding {
    /// Node ID (without table prefix)
    pub node_id: String,
    /// The embedding vector
    pub vector: Vec<f32>,
    /// Model name (defaults to nomic-embed-text-v1.5)
    pub model_name: Option<String>,
    /// Chunk index (0 for single-chunk)
    pub chunk_index: i32,
    /// Character start position
    pub chunk_start: i32,
    /// Character end position
    pub chunk_end: i32,
    /// Total chunks for this node
    pub total_chunks: i32,
    /// Content hash for change detection
    pub content_hash: String,
    /// Token count
    pub token_count: i32,
}

impl NewEmbedding {
    /// Create a new embedding for single-chunk content
    pub fn single_chunk(
        node_id: impl Into<String>,
        vector: Vec<f32>,
        content_hash: impl Into<String>,
        content_length: i32,
        token_count: i32,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            vector,
            model_name: None,
            chunk_index: 0,
            chunk_start: 0,
            chunk_end: content_length,
            total_chunks: 1,
            content_hash: content_hash.into(),
            token_count,
        }
    }

    /// Create a new embedding for multi-chunk content
    pub fn chunk(
        node_id: impl Into<String>,
        vector: Vec<f32>,
        chunk_info: ChunkInfo,
        content_hash: impl Into<String>,
        token_count: i32,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            vector,
            model_name: None,
            chunk_index: chunk_info.chunk_index,
            chunk_start: chunk_info.chunk_start,
            chunk_end: chunk_info.chunk_end,
            total_chunks: chunk_info.total_chunks,
            content_hash: content_hash.into(),
            token_count,
        }
    }
}

/// Result of a semantic search including similarity score
///
/// The scoring algorithm considers both the best chunk similarity and the
/// breadth of relevance (number of matching chunks). This ensures documents
/// with multiple relevant sections rank higher than those with just one
/// good match.
///
/// ## Scoring Formula
///
/// ```text
/// score = max_similarity * (1 + BREADTH_BOOST * log10(matching_chunks))
/// ```
///
/// Where:
/// - `max_similarity`: Highest cosine similarity among all chunks
/// - `matching_chunks`: Number of chunks exceeding the threshold
/// - `BREADTH_BOOST`: Tuning parameter (default 0.3)
///
/// ## Examples
///
/// | Document | Max Sim | Chunks | Score |
/// |----------|---------|--------|-------|
/// | Doc A    | 0.85    | 1      | 0.85  |
/// | Doc B    | 0.80    | 5      | 0.97  |
///
/// Doc B ranks higher due to broader relevance despite lower max similarity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingSearchResult {
    /// The node ID (extracted from embedding.node)
    pub node_id: String,

    /// Composite relevance score (max_similarity * breadth_boost)
    /// This is the primary ranking score.
    pub score: f64,

    /// Best similarity score across all chunks (raw cosine similarity)
    pub max_similarity: f64,

    /// Number of chunks that matched above the threshold
    pub matching_chunks: i64,

    /// The full node data (fetched inline with the search query for performance)
    /// This eliminates the need for separate get_node calls after search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<super::Node>,
}

/// Configuration for the embedding queue
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Debounce duration before processing (default: 30 seconds)
    pub debounce_duration_secs: u64,
    /// Maximum tokens per chunk (default: 512)
    pub max_tokens_per_chunk: usize,
    /// Token overlap between chunks (default: 100)
    pub overlap_tokens: usize,
    /// Estimated characters per token for chunking (default: 3)
    /// Conservative estimate to prevent exceeding token limits
    pub chars_per_token_estimate: usize,
    /// Maximum descendants to aggregate (default: 1000)
    pub max_descendants: usize,
    /// Maximum content size in bytes (default: 10MB)
    pub max_content_size: usize,
    /// Maximum retry attempts for failed embeddings
    pub max_retries: u8,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            debounce_duration_secs: 30,
            max_tokens_per_chunk: MAX_TOKENS_PER_CHUNK,
            overlap_tokens: OVERLAP_TOKENS,
            chars_per_token_estimate: CHARS_PER_TOKEN_ESTIMATE,
            max_descendants: 1000,
            max_content_size: 10 * 1024 * 1024, // 10MB
            max_retries: 3,
        }
    }
}

/// Node types that are embeddable when they are roots
pub const EMBEDDABLE_NODE_TYPES: &[&str] = &["text", "header", "code-block", "schema"];

/// Check if a node type is embeddable
pub fn is_embeddable_type(node_type: &str) -> bool {
    EMBEDDABLE_NODE_TYPES.contains(&node_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.debounce_duration_secs, 30);
        assert_eq!(config.max_tokens_per_chunk, 512);
        assert_eq!(config.overlap_tokens, 100);
        assert_eq!(config.chars_per_token_estimate, 3);
        assert_eq!(config.max_descendants, 1000);
        assert_eq!(config.max_content_size, 10 * 1024 * 1024);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_embeddable_types() {
        assert!(is_embeddable_type("text"));
        assert!(is_embeddable_type("header"));
        assert!(is_embeddable_type("code-block"));
        assert!(is_embeddable_type("schema"));

        // Not embeddable
        assert!(!is_embeddable_type("task"));
        assert!(!is_embeddable_type("date"));
        assert!(!is_embeddable_type("person"));
        assert!(!is_embeddable_type("ai-chat"));
    }

    #[test]
    fn test_new_embedding_single_chunk() {
        let embedding =
            NewEmbedding::single_chunk("node-123", vec![0.1, 0.2, 0.3], "abc123", 100, 50);

        assert_eq!(embedding.node_id, "node-123");
        assert_eq!(embedding.chunk_index, 0);
        assert_eq!(embedding.chunk_start, 0);
        assert_eq!(embedding.chunk_end, 100);
        assert_eq!(embedding.total_chunks, 1);
        assert_eq!(embedding.token_count, 50);
    }

    #[test]
    fn test_new_embedding_multi_chunk() {
        let chunk_info = ChunkInfo {
            chunk_index: 1,
            chunk_start: 500,
            chunk_end: 1000,
            total_chunks: 3,
        };
        let embedding =
            NewEmbedding::chunk("node-456", vec![0.1, 0.2, 0.3], chunk_info, "def456", 200);

        assert_eq!(embedding.node_id, "node-456");
        assert_eq!(embedding.chunk_index, 1);
        assert_eq!(embedding.chunk_start, 500);
        assert_eq!(embedding.chunk_end, 1000);
        assert_eq!(embedding.total_chunks, 3);
    }
}
