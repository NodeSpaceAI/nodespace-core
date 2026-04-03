/// NodeSpace NLP Engine - Embedding and Chat Inference Services
///
/// This crate provides high-performance local LLM services using llama.cpp:
///
/// - **Embeddings**: Semantic search via nomic-embed-vision GGUF models
/// - **Chat Inference**: Streaming text generation with tool-call parsing
///
/// Both services share a single llama.cpp backend and can coexist on the
/// same GPU (validated with Metal on macOS).
///
/// # Features
///
/// - **Local Model Bundling**: GGUF models bundled with application, no network required
/// - **Metal GPU Acceleration**: Native Metal support on macOS via llama.cpp
/// - **Efficient Caching**: LRU cache with automatic eviction for <5ms cache hits
/// - **Asymmetric Embeddings**: Separate prefixes for documents vs queries
/// - **Streaming Chat**: Token-by-token generation with callback-based streaming
/// - **Tool-Call Parsing**: Mistral raw GGUF `[TOOL_CALLS]` format parser
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
pub mod chat;
pub mod config;
pub mod embedding;
pub mod error;

// Re-export embedding types
pub use config::EmbeddingConfig;
pub use embedding::{release_llama_backend, EmbeddingService, EMBEDDING_DIMENSION};
pub use error::{EmbeddingError, Result};

// Re-export chat types
pub use chat::error::ChatError;
pub use chat::parser::{parse_tool_calls, ParseResult, ParsedToolCall, StreamingToolCallParser};
pub use chat::types::{
    ChatChunk, ChatConfig, ChatMessageInput, ChatUsage, LoadedModelInfo, ToolSpec,
};
pub use chat::ChatEngine;
