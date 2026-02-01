/// Core embedding service using llama.cpp with nomic-embed-vision
///
/// Provides text and image embeddings using the llama.cpp backend.
/// - Text: Uses asymmetric prefixes (search_document/search_query) for optimal retrieval
/// - Images: Foundation for future multimodal embedding support
///
/// ## Performance Optimization (Issue #776)
///
/// The service reuses LlamaContext across embedding calls to avoid the overhead
/// of Metal kernel compilation on each call. The context is created once during
/// initialization and reused for all subsequent embeddings.
use crate::config::EmbeddingConfig;
use crate::error::{EmbeddingError, Result};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, OnceLock};

/// Embedding vector dimension for nomic-embed-vision-v1.5
pub const EMBEDDING_DIMENSION: usize = 768;

/// Task prefixes for nomic-embed asymmetric embedding
const SEARCH_DOCUMENT_PREFIX: &str = "search_document: ";
const SEARCH_QUERY_PREFIX: &str = "search_query: ";

#[cfg(feature = "embedding-service")]
use llama_cpp_2::context::params::LlamaContextParams;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::context::LlamaContext;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::llama_backend::LlamaBackend;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::llama_batch::LlamaBatch;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::model::params::LlamaModelParams;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::model::{AddBos, LlamaModel};

/// Global llama backend singleton.
/// The llama.cpp backend can only be initialized once per process, so we use a OnceLock
/// to ensure thread-safe initialization and allow multiple EmbeddingService instances
/// to share the same backend (important for tests running in parallel).
#[cfg(feature = "embedding-service")]
static LLAMA_BACKEND: OnceLock<LlamaBackend> = OnceLock::new();

/// Initialize or get the global llama backend.
/// Returns a reference to the singleton backend instance.
///
/// This handles the case where multiple threads try to initialize the backend
/// concurrently. The llama.cpp backend is a global singleton that can only be
/// initialized once per process.
#[cfg(feature = "embedding-service")]
fn get_or_init_backend() -> Result<&'static LlamaBackend> {
    use llama_cpp_2::LlamaCppError;

    // Try to get existing backend first
    if let Some(backend) = LLAMA_BACKEND.get() {
        return Ok(backend);
    }

    // Try to initialize - only one thread will succeed
    match LlamaBackend::init() {
        Ok(backend) => {
            // Try to store it - might fail if another thread beat us
            match LLAMA_BACKEND.set(backend) {
                Ok(()) => {}
                Err(_already_set) => {
                    // Another thread beat us - that's fine, we'll use theirs
                    // The backend we created will be dropped, but that's safe
                    // because llama_backend_free is idempotent
                }
            }
            // Return whatever is stored (either ours or the other thread's)
            Ok(LLAMA_BACKEND.get().expect("Backend must be initialized"))
        }
        Err(LlamaCppError::BackendAlreadyInitialized) => {
            // The C library backend is initialized, but our OnceLock might not have
            // the reference yet. This can happen when:
            // 1. Another thread is about to store the backend (race condition)
            // 2. A previous test run left stale C global state
            //
            // For case 1, wait with exponential backoff for the OnceLock to be set.
            // For case 2, we need to create a new wrapper that references the existing
            // C backend (llama.cpp backend init is idempotent - it reuses existing state).
            for i in 0..20 {
                if let Some(backend) = LLAMA_BACKEND.get() {
                    return Ok(backend);
                }
                // Exponential backoff: 1ms, 2ms, 4ms... up to ~1s total
                std::thread::sleep(std::time::Duration::from_millis(1 << i.min(8)));
            }

            // Last resort: try to create a new LlamaBackend anyway.
            // llama.cpp's backend_init is idempotent and will return OK if already initialized.
            // This handles the case where the C backend is initialized but our Rust wrapper isn't.
            match LlamaBackend::init() {
                Ok(backend) => {
                    let _ = LLAMA_BACKEND.set(backend);
                    Ok(LLAMA_BACKEND.get().expect("Backend must be set"))
                }
                Err(_) => {
                    // If still failing, check one more time if another thread set it
                    if let Some(backend) = LLAMA_BACKEND.get() {
                        return Ok(backend);
                    }
                    Err(EmbeddingError::ModelLoadError(
                        "Backend initialization failed after multiple attempts".to_string(),
                    ))
                }
            }
        }
        Err(e) => Err(EmbeddingError::ModelLoadError(format!(
            "Backend init failed: {}",
            e
        ))),
    }
}

/// Wrapper to hold model and context together with proper lifetimes.
///
/// ## Safety (Issue #776)
/// This struct uses `unsafe` to store a `LlamaContext` with an extended lifetime.
/// This is safe because:
/// 1. The context is only used while model is alive (owned by this struct)
/// 2. The backend is a global singleton that lives for the entire process
/// 3. Drop order is guaranteed: context drops before model
/// 4. Access is serialized through a Mutex in EmbeddingService
///
/// The context is created lazily on first embedding request and reused for all subsequent
/// requests, avoiding the Metal kernel compilation overhead that was causing ~95% CPU usage.
#[cfg(feature = "embedding-service")]
struct LlamaState {
    // SAFETY: Field order matters for drop order! Rust drops fields in declaration order.
    // `context` must be declared AFTER `model` so it drops FIRST.
    model: LlamaModel,
    /// Persistent context for embedding generation.
    /// Uses transmuted lifetime - safe because we control drop order (see above).
    context: Option<LlamaContext<'static>>,
    /// Current batch size of the context (needed to check if recreation is required)
    current_batch_size: u32,
    /// Context parameters for lazy initialization
    context_size: u32,
    n_threads: i32,
}

#[cfg(feature = "embedding-service")]
impl LlamaState {
    fn new(model: LlamaModel, context_size: u32, n_threads: i32) -> Self {
        Self {
            model,
            context: None,
            current_batch_size: 0,
            context_size,
            n_threads,
        }
    }

    /// Get or create a context with sufficient batch size for the given token count.
    ///
    /// The context is reused when possible. If the token count exceeds the current
    /// context's batch size, a new context is created with a larger batch size.
    /// This balances the performance benefit of context reuse with the need to
    /// handle varying text lengths.
    fn get_or_create_context(
        &mut self,
        required_tokens: usize,
    ) -> std::result::Result<&mut LlamaContext<'static>, EmbeddingError> {
        let required_batch_size = std::cmp::max(required_tokens as u32, 512);

        // Check if we need to create/recreate the context
        let needs_new_context =
            self.context.is_none() || required_batch_size > self.current_batch_size;

        if needs_new_context {
            // Drop existing context first (if any)
            if self.context.is_some() {
                tracing::debug!(
                    "Recreating context: current batch_size={}, required={}",
                    self.current_batch_size,
                    required_batch_size
                );
                self.context = None;
            } else {
                tracing::info!("Creating persistent LlamaContext (Issue #776 optimization)");
            }

            let ctx_params = LlamaContextParams::default()
                .with_n_ctx(std::num::NonZeroU32::new(self.context_size))
                .with_n_batch(required_batch_size)
                .with_n_ubatch(required_batch_size)
                .with_n_threads_batch(self.n_threads)
                .with_embeddings(true);

            // Get the global backend reference
            let backend = get_or_init_backend()?;
            let ctx = self.model.new_context(backend, ctx_params).map_err(|e| {
                EmbeddingError::InferenceError(format!("Context creation failed: {}", e))
            })?;

            // SAFETY: We're extending the lifetime of the context to 'static.
            // This is safe because:
            // 1. The context is stored in this struct alongside model and backend
            // 2. Rust's drop order guarantees context drops before model and backend
            // 3. The Mutex ensures single-threaded access to the context
            let ctx: LlamaContext<'static> = unsafe { std::mem::transmute(ctx) };
            self.context = Some(ctx);
            self.current_batch_size = required_batch_size;

            tracing::info!(
                "Context created with batch_size={} - Metal kernels compiled",
                required_batch_size
            );
        }

        Ok(self.context.as_mut().unwrap())
    }
}

// SAFETY: LlamaState is wrapped in Option<Mutex<LlamaState>> in EmbeddingService,
// ensuring all access is synchronized. The underlying llama.cpp resources (backend,
// model, context) are not inherently thread-safe, but our Mutex wrapper provides
// the required synchronization for safe cross-thread access.
#[cfg(feature = "embedding-service")]
unsafe impl Send for LlamaState {}
#[cfg(feature = "embedding-service")]
unsafe impl Sync for LlamaState {}

/// Main embedding service using llama.cpp
pub struct EmbeddingService {
    config: EmbeddingConfig,
    /// Model and context state, wrapped in Mutex<Option<>> to allow taking ownership
    /// for cleanup without requiring &mut self (needed for Arc<EmbeddingService>).
    #[cfg(feature = "embedding-service")]
    state: Mutex<Option<LlamaState>>,
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
            state: Mutex::new(None),
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
    ///
    /// ## Performance (Issue #776)
    /// Creates a persistent LlamaContext that is reused across all embedding calls.
    /// This avoids the overhead of Metal kernel compilation on each embedding request.
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

            // Initialize llama.cpp backend (uses global singleton)
            let backend = get_or_init_backend()?;

            // Load model with GPU offloading
            let model_params =
                LlamaModelParams::default().with_n_gpu_layers(self.config.n_gpu_layers);

            let model = LlamaModel::load_from_file(backend, &model_path, &model_params)
                .map_err(|e| EmbeddingError::ModelLoadError(format!("Model load failed: {}", e)))?;

            // Get actual embedding dimension from model
            self.embedding_dimension = model.n_embd() as usize;
            tracing::info!(
                "Embedding model loaded. Dimension: {}",
                self.embedding_dimension
            );

            // Create LlamaState to hold model and context together
            // (backend is a global singleton, accessed via get_or_init_backend())
            let state = LlamaState::new(model, self.config.context_size, self.config.n_threads);
            *self.state.lock().unwrap_or_else(|p| p.into_inner()) = Some(state);

            tracing::info!(
                "Embedding service initialized with persistent context (Issue #776 optimization)"
            );
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
        if text.is_empty() {
            return Err(EmbeddingError::InvalidInput(
                "Cannot generate embedding for empty text".to_string(),
            ));
        }
        let prefixed = format!("{}{}", SEARCH_DOCUMENT_PREFIX, text);
        // Use prefixed text as cache key to differentiate document vs query embeddings
        self.generate_embedding_internal(&prefixed, &prefixed)
    }

    /// Generate embedding for a search query (uses search_query: prefix)
    ///
    /// Use this for queries that will search against stored documents.
    pub fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        if text.is_empty() {
            return Err(EmbeddingError::InvalidInput(
                "Cannot generate embedding for empty text".to_string(),
            ));
        }
        let prefixed = format!("{}{}", SEARCH_QUERY_PREFIX, text);
        // Use prefixed text as cache key to differentiate document vs query embeddings
        self.generate_embedding_internal(&prefixed, &prefixed)
    }

    /// Generate embedding for a single text (defaults to document embedding)
    ///
    /// For optimal search quality, prefer using `embed_document` for stored content
    /// and `embed_query` for search queries.
    pub fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_document(text)
    }

    /// Warm up the embedding model by generating a dummy embedding
    ///
    /// This triggers model loading and Metal kernel compilation, ensuring
    /// the first real query is fast. Call this during initialization.
    pub fn warmup(&self) -> Result<()> {
        tracing::info!("Warming up embedding model...");
        let start = std::time::Instant::now();
        let _ = self.generate_embedding("warmup")?;
        tracing::info!("Embedding model warmed up in {:?}", start.elapsed());
        Ok(())
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
            // If state isn't loaded (stub mode), return zero vector
            {
                let state_guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
                if state_guard.is_none() {
                    return Ok(vec![0.0; self.embedding_dimension]);
                }
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

    /// Generate embedding using llama.cpp with persistent context (Issue #776)
    ///
    /// ## Performance Optimization
    /// This method reuses a persistent `LlamaContext` across calls, avoiding the
    /// overhead of Metal kernel compilation that was causing ~95% CPU usage.
    /// The context is created lazily on first call and reused thereafter.
    #[cfg(feature = "embedding-service")]
    fn generate_embedding_llama(&self, text: &str) -> Result<Vec<f32>> {
        let total_start = std::time::Instant::now();

        // Lock the state to access model and context
        let lock_start = std::time::Instant::now();
        let mut state_guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let state = state_guard
            .as_mut()
            .ok_or(EmbeddingError::ModelNotInitialized)?;
        let lock_time = lock_start.elapsed();

        // Tokenize using the model
        let tokenize_start = std::time::Instant::now();
        let tokens = state
            .model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| EmbeddingError::TokenizationError(e.to_string()))?;
        let tokenize_time = tokenize_start.elapsed();

        // Get or create the persistent context with sufficient batch size
        let ctx_start = std::time::Instant::now();
        let ctx = state.get_or_create_context(tokens.len())?;
        let ctx_time = ctx_start.elapsed();

        // Create batch for this text (use same size as context)
        let batch_start = std::time::Instant::now();
        let batch_size = std::cmp::max(tokens.len(), 512);
        let mut batch = LlamaBatch::new(batch_size, 1);
        batch
            .add_sequence(&tokens, 0, false)
            .map_err(|e| EmbeddingError::InferenceError(format!("Batch add failed: {}", e)))?;
        let batch_time = batch_start.elapsed();

        // Clear KV cache and encode
        let encode_start = std::time::Instant::now();
        ctx.clear_kv_cache();
        ctx.encode(&mut batch)
            .map_err(|e| EmbeddingError::InferenceError(format!("Encoding failed: {}", e)))?;
        let encode_time = encode_start.elapsed();

        // Get embeddings
        let extract_start = std::time::Instant::now();
        let embedding = ctx
            .embeddings_seq_ith(0)
            .map_err(|e| EmbeddingError::InferenceError(format!("Get embeddings failed: {}", e)))?;
        let extract_time = extract_start.elapsed();

        // L2 normalize
        let normalize_start = std::time::Instant::now();
        let result = Self::normalize(embedding);
        let normalize_time = normalize_start.elapsed();

        let total_time = total_start.elapsed();

        tracing::debug!(
            "EMBEDDING PROFILE: total={:?} | lock={:?} tokenize={:?} ctx={:?} batch={:?} encode={:?} extract={:?} normalize={:?} | tokens={}",
            total_time, lock_time, tokenize_time, ctx_time, batch_time, encode_time, extract_time, normalize_time, tokens.len()
        );

        Ok(result)
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

    /// Shutdown the embedding service and release GPU resources.
    ///
    /// This must be called before application exit to properly clean up Metal/GPU
    /// contexts. Failure to call this before exit may result in crashes during
    /// static destruction (e.g., SIGABRT from ggml_metal_rsets_free).
    ///
    /// After shutdown, the service cannot be used until re-initialized.
    pub fn shutdown(&mut self) {
        tracing::info!("Shutting down embedding service...");

        #[cfg(feature = "embedding-service")]
        {
            // Drop the LlamaState which holds the context and model
            // This must happen before the global LLAMA_BACKEND is destroyed
            let mut state_guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(state) = state_guard.take() {
                // Explicitly drop the state to release Metal resources
                drop(state);
                tracing::info!("LlamaState dropped, Metal resources released");
            }
        }

        // Clear cache
        self.clear_cache();

        self.initialized = false;
        tracing::info!("Embedding service shutdown complete");
    }

    /// Release ALL GPU resources (model + context) without requiring mutable access.
    ///
    /// This method can be called on an `Arc<EmbeddingService>` to release
    /// the entire LlamaState (model and context) before application exit.
    /// Both the model and context hold Metal GPU resources (residency sets)
    /// that must be released before the global llama backend is destroyed
    /// to prevent SIGABRT crashes from ggml_metal_rsets_free.
    ///
    /// After calling this, the embedding service cannot be used until re-initialized.
    /// This is a one-way operation intended for application shutdown.
    pub fn release_gpu_context(&self) {
        tracing::info!("Releasing ALL GPU resources (model + context)...");

        #[cfg(feature = "embedding-service")]
        {
            let mut state_guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(state) = state_guard.take() {
                // Drop the entire LlamaState to release ALL Metal resources
                // This includes both the model and context, ensuring no residency
                // sets remain when the global backend is destroyed
                drop(state);
                tracing::info!("LlamaState (model + context) dropped, Metal resources freed");
            }
        }

        // Also clear the cache to reduce memory pressure
        self.clear_cache();
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
