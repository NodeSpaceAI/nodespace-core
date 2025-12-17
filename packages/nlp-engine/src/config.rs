/// Configuration for the embedding service using llama.cpp
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Offload all model layers to GPU. This is the llama.cpp convention
/// where any value >= total layers offloads everything.
pub const GPU_OFFLOAD_ALL_LAYERS: u32 = 99;

/// Configuration for llama.cpp embedding model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model name or identifier
    pub model_name: String,

    /// Local model path (GGUF file)
    pub model_path: Option<PathBuf>,

    /// Number of GPU layers to offload. Use `GPU_OFFLOAD_ALL_LAYERS` (99) to offload all.
    pub n_gpu_layers: u32,

    /// Context size for embedding
    pub context_size: u32,

    /// Number of threads for CPU inference
    pub n_threads: i32,

    /// Maximum cache size (number of embeddings to cache)
    pub cache_capacity: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_name: "nomic-embed-vision-v1.5".to_string(),
            model_path: None,
            n_gpu_layers: GPU_OFFLOAD_ALL_LAYERS,
            context_size: 8192,
            n_threads: std::thread::available_parallelism()
                .map(|p| p.get() as i32)
                .unwrap_or(4),
            cache_capacity: 10000,
        }
    }
}

impl EmbeddingConfig {
    /// Get the model path, resolving it from ~/.nodespace/models/
    ///
    /// Uses centralized data directory pattern:
    /// - macOS/Linux: ~/.nodespace/models/nomic-embed-vision-v1.5.gguf
    /// - Windows: %USERPROFILE%\.nodespace\models\nomic-embed-vision-v1.5.gguf
    pub fn resolve_model_path(&self) -> Result<PathBuf, std::io::Error> {
        if let Some(path) = &self.model_path {
            if path.exists() {
                return Ok(path.clone());
            }
        }

        // Use centralized ~/.nodespace/models/ directory
        let home_dir = dirs::home_dir().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Cannot determine home directory",
            )
        })?;

        // Try multiple possible filenames
        let base_path = home_dir.join(".nodespace").join("models");
        let possible_names = [
            format!("{}.gguf", sanitize_model_name(&self.model_name)),
            format!("{}.Q8_0.gguf", sanitize_model_name(&self.model_name)),
            format!("{}.f16.gguf", sanitize_model_name(&self.model_name)),
        ];

        for name in &possible_names {
            let model_path = base_path.join(name);
            if model_path.exists() {
                return Ok(model_path);
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "Model not found. Please download nomic-embed-vision GGUF to ~/.nodespace/models/. Tried: {:?}",
                possible_names
            ),
        ))
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.model_name.is_empty() {
            return Err("model_name cannot be empty".to_string());
        }

        if self.context_size == 0 {
            return Err("context_size must be greater than 0".to_string());
        }

        if self.cache_capacity == 0 {
            return Err("cache_capacity must be greater than 0".to_string());
        }

        Ok(())
    }
}

/// Sanitize model name to be filesystem-safe
fn sanitize_model_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '<' | '>' | '|' | '"' => '-',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.model_name, "nomic-embed-vision-v1.5");
        assert_eq!(config.n_gpu_layers, GPU_OFFLOAD_ALL_LAYERS);
        assert_eq!(config.context_size, 8192);
        assert_eq!(config.cache_capacity, 10000);
    }

    #[test]
    fn test_config_validation() {
        let mut config = EmbeddingConfig::default();

        // Valid config
        assert!(config.validate().is_ok());

        // Invalid: empty model name
        config.model_name = String::new();
        assert!(config.validate().is_err());

        // Invalid: zero context size
        config.model_name = "test".to_string();
        config.context_size = 0;
        assert!(config.validate().is_err());

        // Invalid: zero cache capacity
        config.context_size = 8192;
        config.cache_capacity = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_sanitize_model_name() {
        assert_eq!(sanitize_model_name("nomic/embed"), "nomic-embed");
        assert_eq!(sanitize_model_name("model:v1"), "model-v1");
        assert_eq!(sanitize_model_name("normal-name"), "normal-name");
    }
}
