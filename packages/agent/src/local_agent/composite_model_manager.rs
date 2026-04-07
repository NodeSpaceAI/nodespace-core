//! Composite model manager: routes between GGUF and Ollama backends.
//!
//! Implements the [`ModelManager`] trait by delegating to either the GGUF model manager
//! (for local GGUF models) or the Ollama model manager (for models served by a local
//! Ollama daemon). Models are distinguished by the "ollama:" prefix on the model ID.
//!
//! Issue #1058

use std::sync::Arc;

use async_trait::async_trait;

use crate::agent_types::{DownloadEvent, ModelError, ModelInfo, ModelManager};
use crate::local_agent::model_manager::GgufModelManager;
use crate::local_agent::ollama_model_manager::OllamaModelManager;

/// Prefix used to identify Ollama models in the composite manager.
pub const OLLAMA_PREFIX: &str = "ollama:";

// ---------------------------------------------------------------------------
// CompositeModelManager
// ---------------------------------------------------------------------------

/// Concrete [`ModelManager`] that routes between GGUF and Ollama backends.
///
/// Models are identified as Ollama models if their ID starts with "ollama:".
/// All other models are treated as GGUF models.
///
/// Thread-safe: delegates to underlying managers which are thread-safe.
pub struct CompositeModelManager {
    /// GGUF model manager for local model files.
    gguf: Arc<GgufModelManager>,
    /// Ollama model manager for models served by Ollama daemon.
    ollama: Arc<OllamaModelManager>,
}

impl CompositeModelManager {
    /// Create a new composite model manager from GGUF and Ollama managers.
    pub fn new(gguf: Arc<GgufModelManager>, ollama: Arc<OllamaModelManager>) -> Self {
        Self { gguf, ollama }
    }

    /// Check if a model ID represents an Ollama model.
    pub fn is_ollama(model_id: &str) -> bool {
        model_id.starts_with(OLLAMA_PREFIX)
    }

    /// Strip the "ollama:" prefix from a model ID.
    ///
    /// Returns the original model ID if it does not have the prefix.
    pub fn strip_ollama_prefix(model_id: &str) -> &str {
        model_id.strip_prefix(OLLAMA_PREFIX).unwrap_or(model_id)
    }

    /// Add the "ollama:" prefix to a model ID.
    pub fn add_ollama_prefix(model_id: &str) -> String {
        format!("{}{}", OLLAMA_PREFIX, model_id)
    }

    /// Get a reference to the GGUF manager.
    pub fn gguf_manager(&self) -> &Arc<GgufModelManager> {
        &self.gguf
    }

    /// Get a reference to the Ollama manager.
    pub fn ollama_manager(&self) -> &Arc<OllamaModelManager> {
        &self.ollama
    }

    /// Check if the Ollama daemon is available and reachable.
    pub async fn ollama_available(&self) -> bool {
        self.ollama.is_available().await
    }

    /// Set the progress callback for GGUF downloads.
    pub async fn set_gguf_progress_callback(&self, cb: Box<dyn Fn(DownloadEvent) + Send + Sync>) {
        self.gguf.set_progress_callback(cb).await;
    }

    /// Set the progress callback for Ollama downloads.
    pub async fn set_ollama_progress_callback(&self, cb: Box<dyn Fn(DownloadEvent) + Send + Sync>) {
        self.ollama.set_progress_callback(cb).await;
    }
}

#[async_trait]
impl ModelManager for CompositeModelManager {
    async fn list(&self) -> Result<Vec<ModelInfo>, ModelError> {
        // Get GGUF models first
        let mut models = self.gguf.list().await?;

        // Add Ollama models if the daemon is available
        if self.ollama.is_available().await {
            let ollama_models = self.ollama.list().await?;
            for mut model in ollama_models {
                // Prepend "ollama:" prefix to Ollama model IDs
                model.id = Self::add_ollama_prefix(&model.id);
                models.push(model);
            }
        }

        Ok(models)
    }

    async fn download(&self, model_id: &str) -> Result<(), ModelError> {
        if Self::is_ollama(model_id) {
            self.ollama
                .download(Self::strip_ollama_prefix(model_id))
                .await
        } else {
            self.gguf.download(model_id).await
        }
    }

    async fn cancel_download(&self, model_id: &str) -> Result<(), ModelError> {
        if Self::is_ollama(model_id) {
            self.ollama
                .cancel_download(Self::strip_ollama_prefix(model_id))
                .await
        } else {
            self.gguf.cancel_download(model_id).await
        }
    }

    async fn delete(&self, model_id: &str) -> Result<(), ModelError> {
        if Self::is_ollama(model_id) {
            self.ollama
                .delete(Self::strip_ollama_prefix(model_id))
                .await
        } else {
            self.gguf.delete(model_id).await
        }
    }

    async fn load(&self, model_id: &str) -> Result<(), ModelError> {
        if Self::is_ollama(model_id) {
            self.ollama.load(Self::strip_ollama_prefix(model_id)).await
        } else {
            self.gguf.load(model_id).await
        }
    }

    async fn unload(&self) -> Result<(), ModelError> {
        // Unload from GGUF first
        let gguf_result = self.gguf.unload().await;

        // Then unload from Ollama (only if available)
        let ollama_result = if self.ollama.is_available().await {
            self.ollama.unload().await
        } else {
            Ok(())
        };

        // Return the first error if both fail, otherwise Ok
        gguf_result.and(ollama_result)
    }

    async fn loaded_model(&self) -> Result<Option<String>, ModelError> {
        // Check GGUF first
        if let Some(id) = self.gguf.loaded_model().await? {
            return Ok(Some(id));
        }

        // Then check Ollama (only if available)
        if self.ollama.is_available().await {
            if let Some(id) = self.ollama.loaded_model().await? {
                return Ok(Some(Self::add_ollama_prefix(&id)));
            }
        }

        Ok(None)
    }

    async fn recommended_model(&self) -> Result<String, ModelError> {
        self.gguf.recommended_model().await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ollama_prefix() {
        // Test various model IDs
        assert!(CompositeModelManager::is_ollama("ollama:llama3.2:3b"));
        assert!(!CompositeModelManager::is_ollama("ministral-3b-q4km"));
        assert!(CompositeModelManager::is_ollama("ollama:"));
        assert!(!CompositeModelManager::is_ollama(""));
    }

    #[test]
    fn test_strip_ollama_prefix() {
        // Strip the prefix correctly
        assert_eq!(
            CompositeModelManager::strip_ollama_prefix("ollama:llama3.2:3b"),
            "llama3.2:3b"
        );
        // Leave unchanged if no prefix
        assert_eq!(
            CompositeModelManager::strip_ollama_prefix("ministral"),
            "ministral"
        );
    }

    #[test]
    fn test_add_ollama_prefix() {
        // Add prefix correctly
        assert_eq!(
            CompositeModelManager::add_ollama_prefix("llama3.2:3b"),
            "ollama:llama3.2:3b"
        );
    }
}
