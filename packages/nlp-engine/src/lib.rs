/// NodeSpace NLP Engine - Unified Vector Embedding Service
///
/// This crate provides a high-performance embedding service using llama.cpp
/// with nomic-embed-vision for semantic search across NodeSpace knowledge graphs.
///
/// # Features
///
/// - **Local Model Bundling**: GGUF models bundled with application, no network required
/// - **Metal GPU Acceleration**: Native Metal support on macOS via llama.cpp
/// - **Efficient Caching**: LRU cache with automatic eviction for <5ms cache hits
/// - **Asymmetric Embeddings**: Separate prefixes for documents vs queries
/// - **Vision Ready**: Foundation for future multimodal embedding support
///
/// # Example
///
/// ```ignore
/// use nodespace_nlp_engine::{EmbeddingService, EmbeddingConfig};
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = EmbeddingConfig::default();
///     let mut service = EmbeddingService::new(config)?;
///     service.initialize()?;
///
///     // For stored documents
///     let doc_embedding = service.embed_document("Hello, world!")?;
///
///     // For search queries
///     let query_embedding = service.embed_query("greeting")?;
///
///     println!("Embedding dimension: {}", doc_embedding.len()); // 768
///
///     Ok(())
/// }
/// ```
pub mod config;
pub mod embedding;
pub mod error;

// Re-export main types
pub use config::EmbeddingConfig;
pub use embedding::{EmbeddingService, EMBEDDING_DIMENSION};
pub use error::{EmbeddingError, Result};
