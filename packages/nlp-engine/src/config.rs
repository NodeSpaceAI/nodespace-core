/// Configuration for the embedding service
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Maximum supported sequence length for transformer models
/// Limited by attention matrix memory requirements (O(n²))
const MAX_SUPPORTED_SEQUENCE_LENGTH: usize = 8192;

/// Configuration for BERT embedding model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model name or identifier
    pub model_name: String,

    /// Local model path (bundled with application)
    pub model_path: Option<PathBuf>,

    /// Maximum sequence length for tokenization
    pub max_sequence_length: usize,

    /// Enable instruction prefix for queries (e.g., "Represent this sentence for searching:")
    pub use_instruction_prefix: bool,

    /// Maximum cache size (number of embeddings to cache)
    pub cache_capacity: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_name: "BAAI/bge-small-en-v1.5".to_string(),
            model_path: None,
            max_sequence_length: 512,
            use_instruction_prefix: false,
            cache_capacity: 10000,
        }
    }
}

impl EmbeddingConfig {
    /// Get the model path, resolving it from ~/.nodespace/models/
    ///
    /// Uses centralized data directory pattern (same as database):
    /// - macOS/Linux: ~/.nodespace/models/bge-small-en-v1.5/
    /// - Windows: %USERPROFILE%\.nodespace\models\bge-small-en-v1.5\
    ///
    /// This allows:
    /// - Consistent location with database (~/.nodespace/database/)
    /// - Easy version updates without app reinstall
    /// - User-friendly management of data
    /// - Shared models across app versions
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

        let model_path = home_dir
            .join(".nodespace")
            .join("models")
            .join(sanitize_model_name(&self.model_name));

        if model_path.exists() {
            Ok(model_path)
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "Model not found at {:?}. Please install model to ~/.nodespace/models/",
                    model_path
                ),
            ))
        }
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.model_name.is_empty() {
            return Err("model_name cannot be empty".to_string());
        }

        if self.max_sequence_length == 0 {
            return Err("max_sequence_length must be greater than 0".to_string());
        }

        if self.max_sequence_length > MAX_SUPPORTED_SEQUENCE_LENGTH {
            return Err(format!(
                "max_sequence_length cannot exceed {} (transformer attention matrix memory limit)",
                MAX_SUPPORTED_SEQUENCE_LENGTH
            ));
        }

        if self.cache_capacity == 0 {
            return Err("cache_capacity must be greater than 0".to_string());
        }

        Ok(())
    }
}

/// Sanitize model name to be filesystem-safe
/// Replaces all filesystem-unsafe characters with hyphens
/// Filters out control characters for additional safety
fn sanitize_model_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control()) // Remove control characters (0x00-0x1F)
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
        assert_eq!(config.model_name, "BAAI/bge-small-en-v1.5");
        assert_eq!(config.max_sequence_length, 512);
        assert!(!config.use_instruction_prefix);
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

        // Invalid: zero sequence length
        config.model_name = "test".to_string();
        config.max_sequence_length = 0;
        assert!(config.validate().is_err());

        // Invalid: excessive sequence length
        config.max_sequence_length = 10000;
        assert!(config.validate().is_err());

        // Invalid: zero cache capacity
        config.max_sequence_length = 512;
        config.cache_capacity = 0;
        assert!(config.validate().is_err());
    }
}
