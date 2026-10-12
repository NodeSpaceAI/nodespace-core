//! Descriptions of the language models an inference engine can serve: the
//! family, the backend that serves it, its catalog row and its status.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors returned by model catalog operations: listing, downloading,
/// verifying and loading a model.
#[derive(Debug, Error)]
pub enum ModelError {
    /// The requested model ID does not exist in the catalog.
    #[error("model not found: {0}")]
    NotFound(String),

    /// A download was already in progress for this model.
    #[error("download already in progress for model: {0}")]
    DownloadInProgress(String),

    /// Network or I/O failure during download.
    #[error("download failed: {0}")]
    DownloadFailed(String),

    /// Verification (SHA-256 checksum) failed after download.
    #[error("verification failed for model: {0}")]
    VerificationFailed(String),

    /// The model file could not be loaded into memory.
    #[error("failed to load model: {0}")]
    LoadFailed(String),

    /// Catch-all for unexpected errors.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Current status of a model in the local catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModelStatus {
    /// Model is known but not yet downloaded.
    NotDownloaded,
    /// Model is currently being downloaded.
    Downloading {
        /// Download progress as a percentage (0.0 -- 100.0).
        progress_pct: f32,
        /// Bytes downloaded so far.
        bytes_downloaded: u64,
        /// Total bytes to download.
        bytes_total: u64,
    },
    /// Download complete, verifying integrity (SHA-256).
    Verifying,
    /// Model is on disk and ready to be loaded.
    Ready,
    /// Model is loaded into memory and available for inference.
    Loaded,
    /// An error occurred (download, verification, or loading).
    Error {
        /// Human-readable error description.
        message: String,
    },
}

/// Family of language models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelFamily {
    /// Gemma 4 -- Google's multimodal model series (E4B, 26B-A4B).
    Gemma4,
    /// Model served via a user-configured OpenAI-compatible endpoint (family
    /// determined by the remote server).
    OpenAiCompat,
}

/// Backend used to serve a language model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ModelBackend {
    /// Local GGUF model loaded via llama.cpp.
    #[default]
    Gguf,
    /// Model served by a user-configured OpenAI-compatible endpoint. Covers
    /// every remotely-served model, Ollama's `/v1` API included.
    OpenAiCompat,
}

impl ModelBackend {
    /// Wire identifier sent to the frontend in the model catalog.
    ///
    /// Spelled out rather than derived from `Debug`, which would render the
    /// variant as "openaicompat" and silently disagree with the hyphenated
    /// value the frontend matches on.
    pub fn as_wire_str(self) -> &'static str {
        match self {
            ModelBackend::Gguf => "gguf",
            ModelBackend::OpenAiCompat => "openai-compat",
        }
    }
}

/// Metadata about a language model in the local catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Unique identifier for this model.
    pub id: String,
    /// Model family (e.g. Gemma4).
    pub family: ModelFamily,
    /// Human-readable model name.
    pub name: String,
    /// Filename of the model weights on disk.
    pub filename: Option<String>,
    /// Size of the model file in bytes.
    pub size_bytes: u64,
    /// Quantization format (e.g. "Q4_K_M").
    pub quantization: String,
    /// URL to download the model weights.
    pub url: Option<String>,
    /// Expected SHA-256 hash of the model file.
    pub sha256: Option<String>,
    /// Backend used to serve this model.
    #[serde(default)]
    pub backend: ModelBackend,
    /// Current download / load status.
    pub status: ModelStatus,
    /// Minimum system RAM (in GiB) required to run this model comfortably.
    /// Zero means unknown (e.g. for remotely-served models).
    pub min_memory_gb: u8,
}
