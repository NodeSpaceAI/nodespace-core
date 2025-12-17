/// Core embedding service using llama.cpp with nomic-embed-vision
///
/// Provides text and image embeddings using the llama.cpp backend.
/// - Text: Uses asymmetric prefixes (search_document/search_query) for optimal retrieval
/// - Images: Foundation for future multimodal embedding support
use crate::config::EmbeddingConfig;
use crate::error::{EmbeddingError, Result};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

/// Embedding vector dimension for nomic-embed-vision-v1.5
pub const EMBEDDING_DIMENSION: usize = 768;

/// Task prefixes for nomic-embed asymmetric embedding
const SEARCH_DOCUMENT_PREFIX: &str = "search_document: ";
const SEARCH_QUERY_PREFIX: &str = "search_query: ";

#[cfg(feature = "embedding-service")]
use llama_cpp_2::context::params::LlamaContextParams;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::llama_backend::LlamaBackend;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::llama_batch::LlamaBatch;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::model::params::LlamaModelParams;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::model::{AddBos, LlamaModel};

/// Main embedding service using llama.cpp
pub struct EmbeddingService {
    config: EmbeddingConfig,
    #[cfg(feature = "embedding-service")]
    backend: Option<LlamaBackend>,
    #[cfg(feature = "embedding-service")]
    model: Option<LlamaModel>,
    cache: Arc<Mutex<LruCache<String, Vec<f32>>>>,
    initialized: bool,
    #[cfg(feature = "embedding-service")]
    embedding_dimension: usize,
}

impl EmbeddingService {
    /// Create a new embedding service with the given configuration
    pub fn new(config: EmbeddingConfig) -> Result<Self> {
        config.validate().map_err(EmbeddingError::ConfigError)?;

        let cache_capacity = NonZeroUsize::new(config.cache_capacity)
            .ok_or_else(|| EmbeddingError::ConfigError("cache_capacity must be > 0".to_string()))?;

        Ok(Self {
            config,
            #[cfg(feature = "embedding-service")]
            backend: None,
            #[cfg(feature = "embedding-service")]
            model: None,
            cache: Arc::new(Mutex::new(LruCache::new(cache_capacity))),
            initialized: false,
            #[cfg(feature = "embedding-service")]
            embedding_dimension: EMBEDDING_DIMENSION,
        })
    }

    /// Initialize the model (loads from bundled path)
    ///
    /// If the model file is not found, the service will operate in stub mode
    /// returning zero vectors. This allows tests to run without the model.
    pub fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        #[cfg(feature = "embedding-service")]
        {
            tracing::info!("Loading embedding model: {}", self.config.model_name);

            // Resolve model path - if not found, operate in stub mode
            let model_path = match self.config.resolve_model_path() {
                Ok(path) => path,
                Err(e) => {
                    tracing::warn!(
                        "Model not found, operating in stub mode (zero vectors): {}",
                        e
                    );
                    self.initialized = true;
                    return Ok(());
                }
            };

            tracing::info!("Model path resolved to: {:?}", model_path);

            // Initialize llama.cpp backend
            let backend = LlamaBackend::init().map_err(|e| {
                EmbeddingError::ModelLoadError(format!("Backend init failed: {}", e))
            })?;

            // Load model with GPU offloading
            let model_params =
                LlamaModelParams::default().with_n_gpu_layers(self.config.n_gpu_layers);

            let model = LlamaModel::load_from_file(&backend, &model_path, &model_params)
                .map_err(|e| EmbeddingError::ModelLoadError(format!("Model load failed: {}", e)))?;

            // Get actual embedding dimension from model
            self.embedding_dimension = model.n_embd() as usize;
            tracing::info!(
                "Embedding model loaded. Dimension: {}",
                self.embedding_dimension
            );

            self.backend = Some(backend);
            self.model = Some(model);
        }

        #[cfg(not(feature = "embedding-service"))]
        {
            tracing::info!("STUB: Embedding service initialized (feature disabled)");
        }

        self.initialized = true;
        Ok(())
    }

    /// Generate embedding for a document (uses search_document: prefix)
    ///
    /// Use this for content that will be stored and searched against.
    pub fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        let prefixed = format!("{}{}", SEARCH_DOCUMENT_PREFIX, text);
        self.generate_embedding_internal(&prefixed, text)
    }

    /// Generate embedding for a search query (uses search_query: prefix)
    ///
    /// Use this for queries that will search against stored documents.
    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let prefixed = format!("{}{}", SEARCH_QUERY_PREFIX, text);
        self.generate_embedding_internal(&prefixed, text)
    }

    /// Generate embedding for a single text (defaults to document embedding)
    ///
    /// For optimal search quality, prefer using `embed_document` for stored content
    /// and `embed_query` for search queries.
    pub fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_document(text)
    }

    /// Generate embedding for an image (foundation for future multimodal support)
    ///
    /// Currently returns an error - full implementation coming in future release.
    pub fn embed_image(&self, _image_data: &[u8]) -> Result<Vec<f32>> {
        Err(EmbeddingError::InferenceError(
            "Image embedding not yet implemented. Coming in future release.".to_string(),
        ))
    }

    /// Internal embedding generation with caching
    fn generate_embedding_internal(
        &self,
        prefixed_text: &str,
        cache_key: &str,
    ) -> Result<Vec<f32>> {
        if cache_key.is_empty() {
            return Err(EmbeddingError::InvalidInput(
                "Cannot generate embedding for empty text".to_string(),
            ));
        }

        // Check cache first
        {
            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(cached) = cache.get(cache_key) {
                return Ok(cached.clone());
            }
        }

        if !self.initialized {
            return Err(EmbeddingError::ModelNotInitialized);
        }

        #[cfg(feature = "embedding-service")]
        {
            // If model isn't loaded (stub mode), return zero vector
            if self.model.is_none() {
                return Ok(vec![0.0; self.embedding_dimension]);
            }

            let embedding = self.generate_embedding_llama(prefixed_text)?;

            // Cache the result
            {
                let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
                cache.put(cache_key.to_string(), embedding.clone());
            }

            Ok(embedding)
        }

        #[cfg(not(feature = "embedding-service"))]
        {
            // Stub: return zero vector with correct dimensions
            Ok(vec![0.0; EMBEDDING_DIMENSION])
        }
    }

    /// Generate embedding using llama.cpp
    #[cfg(feature = "embedding-service")]
    fn generate_embedding_llama(&self, text: &str) -> Result<Vec<f32>> {
        let backend = self
            .backend
            .as_ref()
            .ok_or(EmbeddingError::ModelNotInitialized)?;
        let model = self
            .model
            .as_ref()
            .ok_or(EmbeddingError::ModelNotInitialized)?;

        // Tokenize
        let tokens = model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| EmbeddingError::TokenizationError(e.to_string()))?;

        // Create context with appropriate batch size for encoder
        let batch_size = std::cmp::max(tokens.len() as u32, 512);
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(self.config.context_size))
            .with_n_batch(batch_size)
            .with_n_ubatch(batch_size)
            .with_n_threads_batch(self.config.n_threads)
            .with_embeddings(true);

        let mut ctx = model.new_context(backend, ctx_params).map_err(|e| {
            EmbeddingError::InferenceError(format!("Context creation failed: {}", e))
        })?;

        // Create batch and encode
        let mut batch = LlamaBatch::new(batch_size as usize, 1);
        batch
            .add_sequence(&tokens, 0, false)
            .map_err(|e| EmbeddingError::InferenceError(format!("Batch add failed: {}", e)))?;

        ctx.clear_kv_cache();
        ctx.encode(&mut batch)
            .map_err(|e| EmbeddingError::InferenceError(format!("Encoding failed: {}", e)))?;

        // Get embeddings
        let embedding = ctx
            .embeddings_seq_ith(0)
            .map_err(|e| EmbeddingError::InferenceError(format!("Get embeddings failed: {}", e)))?;

        // L2 normalize
        Ok(Self::normalize(embedding))
    }

    /// L2 normalize embedding vector
    fn normalize(input: &[f32]) -> Vec<f32> {
        let magnitude = input
            .iter()
            .fold(0.0f32, |acc, &val| val.mul_add(val, acc))
            .sqrt();

        if magnitude > 0.0 {
            input.iter().map(|&val| val / magnitude).collect()
        } else {
            input.to_vec()
        }
    }

    /// Generate embeddings for multiple texts (batch operation)
    pub fn generate_batch(&self, texts: Vec<&str>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // For now, process sequentially (batch optimization can come later)
        texts
            .into_iter()
            .map(|text| self.generate_embedding(text))
            .collect()
    }

    /// Convert embedding vector to F32_BLOB format for storage
    #[must_use]
    pub fn to_blob(embedding: &[f32]) -> Vec<u8> {
        embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Convert F32_BLOB format back to embedding vector
    #[must_use]
    pub fn from_blob(blob: &[u8]) -> Vec<f32> {
        blob.chunks_exact(4)
            .map(|bytes| f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            .collect()
    }

    /// Clear the embedding cache
    pub fn clear_cache(&self) {
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        cache.clear();
    }

    /// Get cache statistics (size, capacity)
    pub fn cache_stats(&self) -> (usize, usize) {
        let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        (cache.len(), cache.cap().get())
    }

    /// Check if service is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get the embedding dimension
    pub fn embedding_dimension(&self) -> usize {
        #[cfg(feature = "embedding-service")]
        {
            self.embedding_dimension
        }
        #[cfg(not(feature = "embedding-service"))]
        {
            EMBEDDING_DIMENSION
        }
    }

    /// Get device information
    pub fn device_info(&self) -> String {
        #[cfg(feature = "embedding-service")]
        {
            if self.initialized {
                format!("llama.cpp (GPU layers: {})", self.config.n_gpu_layers)
            } else {
                "llama.cpp (not initialized)".to_string()
            }
        }
        #[cfg(not(feature = "embedding-service"))]
        {
            "Stub (feature disabled)".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blob_conversion() {
        let embedding = vec![0.1, 0.2, 0.3, -0.4, 1.5];
        let blob = EmbeddingService::to_blob(&embedding);
        let recovered = EmbeddingService::from_blob(&blob);

        assert_eq!(embedding.len(), recovered.len());
        for (original, recovered) in embedding.iter().zip(recovered.iter()) {
            assert!((original - recovered).abs() < 1e-6);
        }
    }

    #[test]
    fn test_service_creation() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config);
        assert!(service.is_ok());

        let service = service.unwrap();
        assert!(!service.is_initialized());
    }

    #[test]
    fn test_cache_stats() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();
        let (len, capacity) = service.cache_stats();
        assert_eq!(len, 0);
        assert!(capacity > 0);
    }

    #[test]
    fn test_normalize() {
        let input = vec![3.0, 4.0];
        let normalized = EmbeddingService::normalize(&input);

        // 3^2 + 4^2 = 25, sqrt(25) = 5
        // 3/5 = 0.6, 4/5 = 0.8
        assert!((normalized[0] - 0.6).abs() < 1e-6);
        assert!((normalized[1] - 0.8).abs() < 1e-6);

        // Verify unit length
        let magnitude: f32 = normalized.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((magnitude - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_normalize_zero_vector() {
        let input = vec![0.0, 0.0, 0.0];
        let normalized = EmbeddingService::normalize(&input);

        // Zero vector should remain zero
        assert!(normalized.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn test_embedding_dimension() {
        assert_eq!(EMBEDDING_DIMENSION, 768);
    }

    #[cfg(not(feature = "embedding-service"))]
    #[tokio::test]
    async fn test_stub_embedding() {
        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let embedding = service.generate_embedding("test").unwrap();
        assert_eq!(embedding.len(), EMBEDDING_DIMENSION);
        assert!(embedding.iter().all(|&x| x == 0.0)); // Stub returns zeros
    }
}
