/// Error types for the chat inference engine.
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ChatError {
    #[error("Chat model not loaded - call load_model() first")]
    ModelNotLoaded,

    #[error("Model loading failed: {0}")]
    ModelLoadError(String),

    #[error("Insufficient memory for context window: {0}")]
    InsufficientMemory(String),

    #[error("Model integrity check failed: {0}")]
    IntegrityError(String),

    #[error("Tokenization failed: {0}")]
    TokenizationError(String),

    #[error("Inference failed: {0}")]
    InferenceError(String),

    /// A `llama_decode` call failed at the backend level (e.g. a Metal
    /// command-buffer OOM). The context has already been torn down so the
    /// next call rebuilds a fresh backend — see
    /// `ChatLlamaState::poison_context`. Kept distinct from
    /// [`ChatError::InferenceError`] so callers and logs can tell "backend
    /// recovered, retry the turn" apart from other inference failures.
    #[error("Backend decode failed (context reset): {0}")]
    BackendDecodeFailed(String),

    #[error("Chat template error: {0}")]
    TemplateError(String),

    #[error("Invalid configuration: {0}")]
    ConfigError(String),

    #[error("Context window exceeded: {0}")]
    ContextOverflow(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ChatError>;
