/// Core embedding service using Candle + ONNX
/// Follows patterns from nodespace BERT implementation guide
use crate::config::EmbeddingConfig;
use crate::error::{EmbeddingError, Result};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

#[cfg(feature = "embedding-service")]
use candle_core::{Device, Tensor};
#[cfg(feature = "embedding-service")]
use candle_onnx::simple_eval;
#[cfg(feature = "embedding-service")]
use tokenizers::Tokenizer;

/// Main embedding service
pub struct EmbeddingService {
    config: EmbeddingConfig,
    #[cfg(feature = "embedding-service")]
    model: Option<candle_onnx::onnx::ModelProto>,
    #[cfg(feature = "embedding-service")]
    tokenizer: Option<Tokenizer>,
    #[cfg(feature = "embedding-service")]
    device: Device,
    cache: Arc<Mutex<LruCache<String, Vec<f32>>>>,
    initialized: bool,
}

impl EmbeddingService {
    /// Create a new embedding service with the given configuration
    pub fn new(config: EmbeddingConfig) -> Result<Self> {
        // Validate config
        config.validate().map_err(EmbeddingError::ConfigError)?;

        #[cfg(feature = "embedding-service")]
        let device = {
            // Try Metal GPU first (macOS), fall back to CPU
            Device::new_metal(0).unwrap_or_else(|_| {
                tracing::info!("Metal GPU not available, using CPU");
                Device::Cpu
            })
        };

        let cache_capacity = NonZeroUsize::new(config.cache_capacity)
            .ok_or_else(|| EmbeddingError::ConfigError("cache_capacity must be > 0".to_string()))?;

        Ok(Self {
            config,
            #[cfg(feature = "embedding-service")]
            model: None,
            #[cfg(feature = "embedding-service")]
            tokenizer: None,
            #[cfg(feature = "embedding-service")]
            device,
            cache: Arc::new(Mutex::new(LruCache::new(cache_capacity))),
            initialized: false,
        })
    }

    /// Initialize the model (loads from bundled path)
    pub fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        #[cfg(feature = "embedding-service")]
        {
            tracing::info!("Loading embedding model: {}", self.config.model_name);

            // Resolve model path
            let model_path = self
                .config
                .resolve_model_path()
                .map_err(|e| EmbeddingError::ModelNotFound(e.to_string()))?;

            tracing::info!("Model path resolved to: {:?}", model_path);

            // Load ONNX model
            let model_file = model_path.join("model.onnx");
            if !model_file.exists() {
                return Err(EmbeddingError::ModelNotFound(format!(
                    "Model file not found: {:?}",
                    model_file
                )));
            }

            let model = candle_onnx::read_file(&model_file)
                .map_err(|e| EmbeddingError::ModelLoadError(e.to_string()))?;

            // Load tokenizer
            let tokenizer_file = model_path.join("tokenizer.json");
            if !tokenizer_file.exists() {
                return Err(EmbeddingError::ModelNotFound(format!(
                    "Tokenizer file not found: {:?}",
                    tokenizer_file
                )));
            }

            let tokenizer = Tokenizer::from_file(&tokenizer_file)
                .map_err(|e| EmbeddingError::TokenizationError(e.to_string()))?;

            self.model = Some(model);
            self.tokenizer = Some(tokenizer);

            tracing::info!("Embedding model initialized successfully");
        }

        #[cfg(not(feature = "embedding-service"))]
        {
            tracing::info!("STUB: Embedding service initialized (feature disabled)");
        }

        self.initialized = true;
        Ok(())
    }

    /// Generate embedding for a single text
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Service not initialized (`ModelNotInitialized`)
    /// - Tokenization fails (`TokenizationError`)
    /// - Model inference fails (`InferenceError`)
    ///
    /// # Panics
    ///
    /// Panics if the cache mutex is poisoned (unrecoverable concurrency error)
    pub fn generate_embedding(&self, text: &str) -> Result<Vec<f32>> {
        // Check cache first
        {
            let mut cache = self.cache.lock().unwrap();
            if let Some(cached) = cache.get(text) {
                return Ok(cached.clone());
            }
        }

        if !self.initialized {
            return Err(EmbeddingError::ModelNotInitialized);
        }

        #[cfg(feature = "embedding-service")]
        {
            let embedding = self.generate_embedding_internal(text)?;

            // Cache the result (LRU will automatically evict oldest if full)
            {
                let mut cache = self.cache.lock().unwrap();
                cache.put(text.to_string(), embedding.clone());
            }

            Ok(embedding)
        }

        #[cfg(not(feature = "embedding-service"))]
        {
            // Stub: return zero vector with correct dimensions
            Ok(vec![0.0; 384])
        }
    }

    /// Generate embeddings for multiple texts (true batch operation)
    /// Checks cache first, then processes all uncached texts in a single forward pass
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Service not initialized (`ModelNotInitialized`)
    /// - Tokenization fails for any text (`TokenizationError`)
    /// - Model inference fails (`InferenceError`)
    ///
    /// # Panics
    ///
    /// Panics if the cache mutex is poisoned (unrecoverable concurrency error)
    pub fn generate_batch(&self, texts: Vec<&str>) -> Result<Vec<Vec<f32>>> {
        if !self.initialized {
            return Err(EmbeddingError::ModelNotInitialized);
        }

        #[cfg(feature = "embedding-service")]
        {
            // Separate cached and uncached texts
            let mut results: Vec<Option<Vec<f32>>> = Vec::with_capacity(texts.len());
            let mut uncached_indices = Vec::new();
            let mut uncached_texts = Vec::new();

            {
                let mut cache = self.cache.lock().unwrap();
                for (i, text) in texts.iter().enumerate() {
                    if let Some(cached) = cache.get(*text) {
                        results.push(Some(cached.clone()));
                    } else {
                        results.push(None);
                        uncached_indices.push(i);
                        uncached_texts.push(*text);
                    }
                }
            }

            // Process uncached texts as a single batch
            if !uncached_texts.is_empty() {
                let batch_embeddings = self.generate_batch_internal(&uncached_texts)?;

                // Insert into cache and results (LRU will auto-evict if full)
                {
                    let mut cache = self.cache.lock().unwrap();
                    for (idx, embedding) in uncached_indices.into_iter().zip(batch_embeddings) {
                        cache.put(texts[idx].to_string(), embedding.clone());
                        results[idx] = Some(embedding);
                    }
                }
            }

            // Unwrap all results (all should be Some at this point)
            Ok(results
                .into_iter()
                .map(|r| r.expect("All results should be populated"))
                .collect())
        }

        #[cfg(not(feature = "embedding-service"))]
        {
            // Stub: return zero vectors
            Ok(vec![vec![0.0; 384]; texts.len()])
        }
    }

    /// Internal batch processing - processes multiple texts in a single forward pass
    #[cfg(feature = "embedding-service")]
    fn generate_batch_internal(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let model = self
            .model
            .as_ref()
            .ok_or(EmbeddingError::ModelNotInitialized)?;
        let tokenizer = self
            .tokenizer
            .as_ref()
            .ok_or(EmbeddingError::TokenizerNotInitialized)?;

        // Tokenize all texts
        let mut all_token_ids = Vec::with_capacity(texts.len());
        let mut all_attention_masks = Vec::with_capacity(texts.len());
        let mut max_len = 0;

        for text in texts {
            let encoding = tokenizer
                .encode(*text, true)
                .map_err(|e| EmbeddingError::TokenizationError(e.to_string()))?;

            let tokens = encoding.get_ids().to_vec();
            let attention_mask = encoding.get_attention_mask().to_vec();

            max_len = max_len.max(tokens.len());
            all_token_ids.push(tokens);
            all_attention_masks.push(attention_mask);
        }

        // Pad all sequences to max_len
        for tokens in &mut all_token_ids {
            while tokens.len() < max_len {
                tokens.push(0); // Padding token
            }
        }
        for mask in &mut all_attention_masks {
            while mask.len() < max_len {
                mask.push(0); // Padding mask
            }
        }

        // Flatten into [batch_size * seq_len] and reshape to [batch_size, seq_len]
        let batch_size = texts.len();
        let flattened_tokens: Vec<u32> = all_token_ids.into_iter().flatten().collect();
        let flattened_masks: Vec<u32> = all_attention_masks.into_iter().flatten().collect();

        let token_ids = Tensor::from_vec(flattened_tokens, (batch_size, max_len), &self.device)
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?;

        let attention_mask_tensor =
            Tensor::from_vec(flattened_masks, (batch_size, max_len), &self.device)
                .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?;

        // Run inference
        let inputs = std::collections::HashMap::from([
            ("input_ids".to_string(), token_ids),
            ("attention_mask".to_string(), attention_mask_tensor.clone()),
        ]);

        let outputs = simple_eval(model, inputs)
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?;

        // Extract last_hidden_state [batch_size, seq_len, hidden_dim]
        let hidden_state = outputs
            .get("last_hidden_state")
            .or_else(|| outputs.values().next())
            .ok_or_else(|| EmbeddingError::InferenceError("No output from model".to_string()))?;

        // Apply mean pooling to entire batch
        let pooled = Self::mean_pooling(hidden_state, &attention_mask_tensor)?;

        // Convert to Vec<Vec<f32>> - one embedding per input text
        let mut embeddings = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let embedding_tensor = pooled.get(i)?;
            let embedding: Vec<f32> = embedding_tensor
                .to_vec1()
                .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?;
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }

    #[cfg(feature = "embedding-service")]
    fn generate_embedding_internal(&self, text: &str) -> Result<Vec<f32>> {
        let model = self
            .model
            .as_ref()
            .ok_or(EmbeddingError::ModelNotInitialized)?;
        let tokenizer = self
            .tokenizer
            .as_ref()
            .ok_or(EmbeddingError::TokenizerNotInitialized)?;

        // Tokenize input
        let encoding = tokenizer
            .encode(text, true)
            .map_err(|e| EmbeddingError::TokenizationError(e.to_string()))?;

        let tokens = encoding.get_ids();
        let attention_mask = encoding.get_attention_mask();

        // Create tensors
        let token_ids = Tensor::new(tokens, &self.device)
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?
            .unsqueeze(0)?; // Add batch dimension [1, seq_len]

        let attention_mask_tensor = Tensor::new(attention_mask, &self.device)
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?
            .unsqueeze(0)?; // [1, seq_len]

        // Run inference
        let inputs = std::collections::HashMap::from([
            ("input_ids".to_string(), token_ids),
            ("attention_mask".to_string(), attention_mask_tensor.clone()),
        ]);

        let outputs = simple_eval(model, inputs)
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?;

        // Extract last_hidden_state [batch_size, seq_len, hidden_dim]
        let hidden_state = outputs
            .get("last_hidden_state")
            .or_else(|| outputs.values().next())
            .ok_or_else(|| EmbeddingError::InferenceError("No output from model".to_string()))?;

        // Apply mean pooling with attention mask
        let pooled = Self::mean_pooling(hidden_state, &attention_mask_tensor)?;

        // Convert to Vec<f32>
        let embedding: Vec<f32> = pooled
            .squeeze(0)?
            .to_vec1()
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?;

        Ok(embedding)
    }

    /// Mean pooling with attention mask weighting
    /// Takes hidden states [batch, seq_len, hidden_dim] and attention mask [batch, seq_len]
    /// Returns pooled embeddings [batch, hidden_dim] with L2 normalization
    #[cfg(feature = "embedding-service")]
    fn mean_pooling(hidden_state: &Tensor, attention_mask: &Tensor) -> Result<Tensor> {
        // Expand attention mask to [batch, seq_len, hidden_dim]
        let mask_expanded = attention_mask
            .unsqueeze(2)?
            .broadcast_as(hidden_state.shape())
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?;

        // Apply mask: hidden_state * mask
        let masked = hidden_state
            .mul(&mask_expanded)
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?;

        // Sum over sequence dimension
        let summed = masked.sum(1)?;

        // Count non-padding tokens and add small epsilon to avoid division by zero
        let count = attention_mask
            .sum(1)?
            .unsqueeze(1)?
            .clamp(1.0, f64::INFINITY)?; // Minimum 1.0 to avoid division by zero

        // Mean pooling: sum / count
        let pooled = summed
            .div(&count)
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))?;

        // L2 normalize
        let norm = pooled
            .sqr()?
            .sum_keepdim(1)?
            .sqrt()?
            .clamp(1e-12, f64::INFINITY)?; // Avoid division by zero

        pooled
            .div(&norm)
            .map_err(|e| EmbeddingError::InferenceError(e.to_string()))
    }

    /// Convert embedding vector to F32_BLOB format for Turso storage
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
        let mut cache = self.cache.lock().unwrap();
        cache.clear();
    }

    /// Get cache statistics (size, capacity)
    pub fn cache_stats(&self) -> (usize, usize) {
        let cache = self.cache.lock().unwrap();
        let len = cache.len();
        let capacity = cache.cap().get();
        (len, capacity)
    }

    /// Check if service is initialized
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Get device information
    #[cfg(feature = "embedding-service")]
    pub fn device_info(&self) -> String {
        match &self.device {
            Device::Cpu => "CPU".to_string(),
            Device::Cuda(_) => "CUDA GPU".to_string(),
            Device::Metal(_) => "Metal GPU".to_string(),
        }
    }

    #[cfg(not(feature = "embedding-service"))]
    pub fn device_info(&self) -> String {
        "Stub (feature disabled)".to_string()
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

    #[cfg(not(feature = "embedding-service"))]
    #[tokio::test]
    async fn test_stub_embedding() {
        let config = EmbeddingConfig::default();
        let mut service = EmbeddingService::new(config).unwrap();
        service.initialize().unwrap();

        let embedding = service.generate_embedding("test").unwrap();
        assert_eq!(embedding.len(), 384); // bge-small-en-v1.5 dimension
        assert!(embedding.iter().all(|&x| x == 0.0)); // Stub returns zeros
    }
}
