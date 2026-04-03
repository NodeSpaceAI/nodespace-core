/// Error types for the chat inference engine.
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ChatError {
    #[error("Chat model not loaded - call load_model() first")]
    ModelNotLoaded,

    #[error("Model loading failed: {0}")]
    ModelLoadError(String),

    #[error("Tokenization failed: {0}")]
    TokenizationError(String),

    #[error("Inference failed: {0}")]
    InferenceError(String),

    #[error("Chat template error: {0}")]
    TemplateError(String),

    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    #[error("Context window exceeded: {0}")]
    ContextOverflow(String),

    #[error("Tool-call parse error: {0}")]
    ToolCallParseError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ChatError>;
