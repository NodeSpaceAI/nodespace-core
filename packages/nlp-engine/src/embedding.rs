/// Core embedding service using llama.cpp with nomic-embed-vision
///
/// Provides text and image embeddings using the llama.cpp backend.
/// - Text: Uses asymmetric prefixes (search_document/search_query) for optimal retrieval
/// - Images: Foundation for future multimodal embedding support
///
/// ## Performance Optimization
///
/// The service reuses LlamaContext across embedding calls to avoid the overhead
/// of Metal kernel compilation on each call. The context is created once during
/// initialization and reused for all subsequent embeddings.
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
use llama_cpp_2::context::LlamaContext;
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
use llama_cpp_2::llama_backend::LlamaBackend;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::llama_batch::LlamaBatch;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::model::params::LlamaModelParams;
#[cfg(feature = "embedding-service")]
use llama_cpp_2::model::{AddBos, LlamaModel};

/// Global llama backend singleton.
///
/// Uses `Mutex<Option<LlamaBackend>>` instead of `OnceLock` so the backend can be
/// explicitly cleared during shutdown. This prevents SIGABRT crashes from
/// `__cxa_finalize_ranges` trying to destroy Metal/GPU resources during static
/// destruction when the C runtime state is already partially torn down.
///
/// The llama.cpp backend can only be initialized once per process.
/// The Mutex ensures thread-safe initialization and allows multiple services
/// (embedding, chat) to share the same backend.
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
static LLAMA_BACKEND: Mutex<Option<LlamaBackend>> = Mutex::new(None);

/// A loaded model whose GPU resources the atexit handler must release.
///
/// Implemented for every `Mutex<Option<T>>` slot a service keeps its loaded
/// model+context in, so embedding (`LlamaState`) and chat (`ChatLlamaState`)
/// share one registry despite their distinct state types.
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
trait GpuResidentState: Send + Sync {
    /// Drop the loaded state, if any. Returns whether something was dropped.
    fn release(&self) -> bool;
    /// Whether the slot still holds a loaded state.
    fn is_loaded(&self) -> bool;
}

#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
impl<T: Send> GpuResidentState for Mutex<Option<T>> {
    fn release(&self) -> bool {
        self.lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .is_some()
    }

    fn is_loaded(&self) -> bool {
        self.lock().unwrap_or_else(|p| p.into_inner()).is_some()
    }
}

/// Global registry of loaded model states for atexit cleanup.
///
/// When `NSApplication terminate:` → `exit()` → `__cxa_finalize_ranges` runs,
/// C++ static destructors destroy the Metal device, which asserts that all
/// residency sets are freed. `exit()` skips Rust `Drop`, so our `atexit`
/// handler runs first and drops every registered state — embedding and chat
/// alike — releasing their residency sets before the backend goes.
///
/// Each service stores its state in an `Arc<Mutex<Option<_>>>` and registers
/// that Arc here on load. Graceful shutdown/unload also clears the slot — the
/// atexit handler's `.take()` returns `None` in that case (harmless).
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
static GPU_STATES: Mutex<Vec<Arc<dyn GpuResidentState>>> = Mutex::new(Vec::new());

/// Register an `atexit` handler that releases GPU resources before C++ static
/// destructors run. Called once when the first model is loaded.
///
/// Shared between embedding and chat services since both load models
/// into the same global llama.cpp backend.
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
pub(crate) fn register_atexit_handler() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    extern "C" fn cleanup() {
        // Drop every loaded model+context first (releases Metal residency sets)
        release_registered_states();
        // Then drop the backend
        {
            let mut guard = LLAMA_BACKEND.lock().unwrap_or_else(|p| p.into_inner());
            if guard.take().is_some() {
                eprintln!("[atexit] LlamaBackend dropped");
            }
        }
    }
    unsafe {
        libc::atexit(cleanup);
    }
}

/// Drop every registered model state. Returns how many were still loaded.
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
fn release_registered_states() -> usize {
    let states = GPU_STATES.lock().unwrap_or_else(|p| p.into_inner());
    let released = states.iter().filter(|state| state.release()).count();
    if released > 0 {
        eprintln!("[atexit] {released} model state(s) dropped, Metal residency sets released");
    }
    released
}

/// Register a service's model-state slot in the global registry for atexit
/// cleanup. Call after every successful load.
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
pub(crate) fn register_state_for_cleanup<T: Send + 'static>(state: &Arc<Mutex<Option<T>>>) {
    let state: Arc<dyn GpuResidentState> = Arc::clone(state) as _;
    let mut states = GPU_STATES.lock().unwrap_or_else(|p| p.into_inner());
    // Prune stale entries (unloaded slots) and avoid registering a slot twice
    // when the same service reloads.
    states.retain(|s| s.is_loaded() && !Arc::ptr_eq(s, &state));
    states.push(state);
}

/// Initialize or get the global llama backend.
/// Returns a guard holding a reference to the singleton backend instance.
///
/// This handles the case where multiple threads try to initialize the backend
/// concurrently. The llama.cpp backend is a global singleton that can only be
/// initialized once per process.
///
/// The returned `BackendGuard` must be held for the duration of backend usage
/// (e.g., model loading, context creation).
///
/// Shared between embedding and chat services.
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
pub(crate) fn get_or_init_backend() -> std::result::Result<BackendGuard, String> {
    use llama_cpp_2::LlamaCppError;

    let mut guard = LLAMA_BACKEND.lock().unwrap_or_else(|p| p.into_inner());

    // Already initialized - return the guard
    if guard.is_some() {
        return Ok(BackendGuard(guard));
    }

    // Try to initialize - only one thread will succeed (Mutex ensures exclusivity)
    match LlamaBackend::init() {
        Ok(backend) => {
            *guard = Some(backend);
            Ok(BackendGuard(guard))
        }
        Err(LlamaCppError::BackendAlreadyInitialized) => {
            // The C library backend is initialized but our Option is None.
            // This can happen if the backend was cleared during a previous shutdown
            // but the C global state persists (e.g., in tests).
            // Try again - llama.cpp backend init is idempotent.
            match LlamaBackend::init() {
                Ok(backend) => {
                    *guard = Some(backend);
                    Ok(BackendGuard(guard))
                }
                Err(_) => Err("Backend initialization failed after multiple attempts".to_string()),
            }
        }
        Err(e) => Err(format!("Backend init failed: {}", e)),
    }
}

/// RAII guard that holds a lock on the global LLAMA_BACKEND.
///
/// Dereferences to `&LlamaBackend` for convenient use with llama.cpp APIs.
/// The lock is released when the guard is dropped.
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
pub(crate) struct BackendGuard(std::sync::MutexGuard<'static, Option<LlamaBackend>>);

#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
impl std::ops::Deref for BackendGuard {
    type Target = LlamaBackend;

    fn deref(&self) -> &Self::Target {
        self.0
            .as_ref()
            .expect("BackendGuard created with None backend")
    }
}

/// Maximum time to wait for the process-global backend to initialize and the
/// model to load onto the GPU.
///
/// A normal load completes in a few seconds. A genuinely stuck first-time
/// backend/Metal init would otherwise hang the process at 0% CPU forever.
/// Exceeding this budget returns an error so the caller fails fast with a
/// diagnostic instead of hanging silently. The budget is generous so it never
/// trips on a slow-but-progressing load on a cold or constrained machine.
#[cfg(feature = "embedding-service")]
const MODEL_LOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Initialize the global backend (if needed) and load a model from `model_path`,
/// bounded by `timeout`.
///
/// The blocking backend-init + model-load runs on a dedicated worker thread so a
/// genuinely stuck load fails fast with an error instead of hanging the process
/// at 0% CPU. The worker acquires the global backend Mutex via
/// [`get_or_init_backend`] — serializing loads across services exactly as
/// before — and returns the loaded `LlamaModel`, which is `Send`. The backend
/// `MutexGuard` is created and dropped entirely within the worker thread, so it
/// never crosses a thread boundary.
///
/// NOTE: if a load genuinely deadlocks, the abandoned worker thread still holds
/// the backend Mutex, so subsequent loads in the same process would block on it.
/// That is an acceptable tradeoff for this pathological case — the point is that
/// the *first* stuck load returns an error instead of hanging the process
/// forever at 0% CPU.
#[cfg(feature = "embedding-service")]
fn load_model_with_timeout(
    model_path: std::path::PathBuf,
    n_gpu_layers: u32,
    timeout: std::time::Duration,
) -> std::result::Result<LlamaModel, String> {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("llama-model-load".to_string())
        .spawn(move || {
            let result = (|| {
                // Holds the backend Mutex for the duration of the load.
                let backend = get_or_init_backend()?;
                let model_params = LlamaModelParams::default().with_n_gpu_layers(n_gpu_layers);
                LlamaModel::load_from_file(&backend, &model_path, &model_params)
                    .map_err(|e| format!("Model load failed: {}", e))
            })();
            // Receiver may have already timed out and dropped; ignore send errors.
            let _ = tx.send(result);
        })
        .map_err(|e| format!("Failed to spawn model-load thread: {}", e))?;

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "Model load timed out after {}s (possible Metal/GPU backend deadlock)",
            timeout.as_secs()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("Model-load thread terminated without returning a result".to_string())
        }
    }
}

/// Explicitly release the global llama backend to prevent SIGABRT on process exit.
///
/// This drops the `LlamaBackend` while we still have full control of the process,
/// rather than letting `__cxa_finalize_ranges` destroy it during static destruction
/// when Metal/GPU state may already be partially torn down.
///
/// Must be called AFTER all `LlamaState` instances (models, contexts) are dropped.
/// Safe to call multiple times (idempotent).
#[cfg(any(feature = "embedding-service", feature = "chat-service"))]
pub fn release_llama_backend() {
    let mut guard = LLAMA_BACKEND.lock().unwrap_or_else(|p| p.into_inner());
    if guard.take().is_some() {
        tracing::info!("Global LLAMA_BACKEND released, Metal resources freed");
    }
}

/// Explicitly release the global llama backend (no-op when features are disabled).
#[cfg(not(any(feature = "embedding-service", feature = "chat-service")))]
pub fn release_llama_backend() {
    // No-op: no backend to release when llama features are disabled
}

/// Wrapper to hold model and context together with proper lifetimes.
///
/// ## Safety
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
                tracing::info!("Creating persistent LlamaContext");
            }

            let ctx_params = LlamaContextParams::default()
                .with_n_ctx(std::num::NonZeroU32::new(self.context_size))
                .with_n_batch(required_batch_size)
                .with_n_ubatch(required_batch_size)
                .with_n_threads_batch(self.n_threads)
                .with_embeddings(true);

            // Get the global backend (holds Mutex lock for duration of context creation)
            let backend = get_or_init_backend().map_err(EmbeddingError::ModelLoadError)?;
            let ctx = self.model.new_context(&backend, ctx_params).map_err(|e| {
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
    /// Model and context state, wrapped in Arc<Mutex<Option<>>> to allow:
    /// 1. Taking ownership for cleanup without requiring &mut self (Arc)
    /// 2. Registration in the global GPU_STATES for atexit cleanup
    #[cfg(feature = "embedding-service")]
    state: Arc<Mutex<Option<LlamaState>>>,
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
            state: Arc::new(Mutex::new(None)),
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
    /// ## Performance
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

            // Load-time integrity gate (ADR-058): refuse to load a model whose
            // bytes don't match the pinned SHA-256. This closes the post-install
            // tamper window — a swapped GGUF is parsed by native llama.cpp with
            // full daemon authority, so a hostile artifact must never reach it.
            crate::config::verify_model_integrity(&model_path).map_err(|e| {
                tracing::error!("{}", e);
                EmbeddingError::IntegrityError(e)
            })?;

            // Initialize the llama.cpp backend (global singleton) and load the
            // model onto the GPU. This runs on a dedicated worker thread bounded
            // by a timeout so a genuinely stuck first-time Metal/GPU init fails
            // fast with an error instead of hanging the process at 0% CPU.
            let model =
                load_model_with_timeout(model_path, self.config.n_gpu_layers, MODEL_LOAD_TIMEOUT)
                    .map_err(EmbeddingError::ModelLoadError)?;

            // Get actual embedding dimension from model
            self.embedding_dimension = model.n_embd() as usize;
            tracing::info!(
                "Embedding model loaded. Dimension: {}",
                self.embedding_dimension
            );

            // Create LlamaState and store in per-instance state.
            // Also register with global atexit handler so Metal resources are released
            // even if NSApplication terminate: bypasses our graceful shutdown path.
            let state = LlamaState::new(model, self.config.context_size, self.config.n_threads);
            *self.state.lock().unwrap_or_else(|p| p.into_inner()) = Some(state);
            register_atexit_handler();
            register_state_for_cleanup(&self.state);

            tracing::info!("Embedding service initialized with persistent context");
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

    /// Generate embedding using llama.cpp with persistent context
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

    /// The atexit registry must hold chat state alongside embedding state:
    /// a chat slot that never reaches it keeps its Metal residency sets alive
    /// past backend teardown and SIGABRTs on quit. Exercised on registered
    /// entries directly — draining the whole global registry would unload
    /// models other tests in this process have loaded.
    #[cfg(any(feature = "embedding-service", feature = "chat-service"))]
    #[test]
    fn registry_releases_states_of_distinct_types() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static DROPS: AtomicUsize = AtomicUsize::new(0);
        struct EmbeddingLike;
        struct ChatLike;
        impl Drop for EmbeddingLike {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }
        impl Drop for ChatLike {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }

        let embedding = Arc::new(Mutex::new(Some(EmbeddingLike)));
        let chat = Arc::new(Mutex::new(Some(ChatLike)));
        register_state_for_cleanup(&embedding);
        register_state_for_cleanup(&chat);
        // A reload of the same service re-registers its slot; it must not
        // appear twice.
        register_state_for_cleanup(&chat);

        let registered: Vec<Arc<dyn GpuResidentState>> = {
            let states = GPU_STATES.lock().unwrap_or_else(|p| p.into_inner());
            let embedding: Arc<dyn GpuResidentState> = embedding.clone();
            let chat: Arc<dyn GpuResidentState> = chat.clone();
            let ours: Vec<_> = states
                .iter()
                .filter(|s| Arc::ptr_eq(s, &embedding) || Arc::ptr_eq(s, &chat))
                .cloned()
                .collect();
            assert_eq!(ours.len(), 2, "each slot registered exactly once");
            ours
        };

        for state in &registered {
            assert!(state.release());
            assert!(!state.is_loaded());
            assert!(!state.release(), "a released slot releases nothing twice");
        }
        assert_eq!(DROPS.load(Ordering::SeqCst), 2);
        assert!(embedding.lock().unwrap().is_none());
        assert!(chat.lock().unwrap().is_none());
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
