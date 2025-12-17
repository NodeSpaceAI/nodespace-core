/// Error types for the NLP embedding engine
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EmbeddingError {
    #[error("Model not initialized - call initialize() first")]
    ModelNotInitialized,

    #[error("Model loading failed: {0}")]
    ModelLoadError(String),

    #[error("Tokenization failed: {0}")]
    TokenizationError(String),

    #[error("Inference failed: {0}")]
    InferenceError(String),

    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Model file not found at path: {0}")]
    ModelNotFound(String),

    #[error("Device initialization failed: {0}")]
    DeviceError(String),
}

pub type Result<T> = std::result::Result<T, EmbeddingError>;
