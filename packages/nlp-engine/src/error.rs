/// Error types for the NLP embedding engine
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EmbeddingError {
    #[error("Model not initialized - call initialize() first")]
    ModelNotInitialized,

    #[error("Tokenizer not initialized - call initialize() first")]
    TokenizerNotInitialized,

    #[error("Model loading failed: {0}")]
    ModelLoadError(String),

    #[error("Tokenization failed: {0}")]
    TokenizationError(String),

    #[error("Inference failed: {0}")]
    InferenceError(String),

    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Model file not found at path: {0}")]
    ModelNotFound(String),

    #[error("Device initialization failed: {0}")]
    DeviceError(String),

    #[cfg(feature = "embedding-service")]
    #[error("Candle error: {0}")]
    CandleError(#[from] candle_core::Error),

    #[cfg(feature = "embedding-service")]
    #[error("Tokenizer error: {0}")]
    TokenizerError(#[from] tokenizers::Error),
}

pub type Result<T> = std::result::Result<T, EmbeddingError>;
