/// NodeSpace NLP Engine - Unified Vector Embedding Service
///
/// This crate provides a high-performance embedding service using Candle + ONNX
/// for semantic search across NodeSpace knowledge graphs and codebases.
///
/// # Features
///
/// - **Local Model Bundling**: Models bundled with application, no network required
/// - **Metal GPU Acceleration**: Optimized performance on macOS with automatic CPU fallback
/// - **Efficient Caching**: LRU cache with automatic eviction for <5ms cache hits
/// - **Batch Operations**: Efficient batch embedding generation
/// - **Turso Integration**: F32_BLOB format for direct database storage
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
///     let embedding = service.generate_embedding("Hello, world!")?;
///     println!("Generated embedding with {} dimensions", embedding.len());
///
///     Ok(())
/// }
/// ```
pub mod config;
pub mod embedding;
pub mod error;

// Re-export main types
pub use config::EmbeddingConfig;
pub use embedding::EmbeddingService;
pub use error::{EmbeddingError, Result};
