//! Ollama model manager: interact with locally-running Ollama daemon.
//!
//! Implements the [`ModelManager`] trait for models served by a local Ollama instance.
//! Models are managed entirely by Ollama (download, caching, loading); this implementation
//! acts as a client to the Ollama HTTP API.
//!
//! Issue #1058

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::warn;

use crate::agent_types::{
    DownloadEvent, ModelBackend, ModelError, ModelFamily, ModelInfo, ModelManager, ModelStatus,
};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Shared, thread-safe progress callback for download events.
type ProgressCallback = Arc<RwLock<Option<Box<dyn Fn(DownloadEvent) + Send + Sync>>>>;

// ---------------------------------------------------------------------------
// Ollama API response types (private)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
    size: u64,
}

#[derive(Deserialize)]
struct OllamaPullProgress {
    status: String,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    completed: Option<u64>,
}

#[derive(Deserialize)]
struct OllamaPsResponse {
    models: Vec<OllamaRunningModel>,
}

#[derive(Deserialize)]
struct OllamaRunningModel {
    name: String,
}

// ---------------------------------------------------------------------------
// OllamaModelManager
// ---------------------------------------------------------------------------

/// Concrete [`ModelManager`] for models served by a local Ollama daemon.
///
/// Thread-safe: progress callback lives behind `Arc<RwLock<>>`.
pub struct OllamaModelManager {
    http_client: reqwest::Client,
    base_url: String,
    on_progress: ProgressCallback,
    loaded_model_id: Arc<RwLock<Option<String>>>,
}

impl OllamaModelManager {
    /// Create a new Ollama model manager pointing to the default Ollama daemon.
    ///
    /// Uses `http://127.0.0.1:11434` as the default base URL.
    pub fn new() -> Self {
        Self::with_base_url("http://127.0.0.1:11434".to_string())
    }

    /// Create a new Ollama model manager with a custom base URL.
    ///
    /// Useful for testing or connecting to non-default Ollama instances.
    pub fn with_base_url(base_url: String) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            base_url,
            on_progress: Arc::new(RwLock::new(None)),
            loaded_model_id: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the progress callback for download events.
    pub async fn set_progress_callback(&self, cb: Box<dyn Fn(DownloadEvent) + Send + Sync>) {
        *self.on_progress.write().await = Some(cb);
    }

    /// Check if Ollama daemon is reachable.
    ///
    /// Makes a GET request to `/api/tags` with a 2-second timeout.
    /// Returns `false` if the daemon is unreachable or times out.
    pub async fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.http_client.get(&url).send(),
        )
        .await
        {
            Ok(Ok(resp)) => resp.status().is_success(),
            _ => false,
        }
    }

    /// Helper to fire a progress event.
    async fn fire_progress(&self, event: DownloadEvent) {
        if let Some(cb) = self.on_progress.read().await.as_ref() {
            cb(event);
        }
    }
}

impl Default for OllamaModelManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelManager for OllamaModelManager {
    async fn list(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let url = format!("{}/api/tags", self.base_url);

        // If Ollama is unreachable, return empty list (not an error).
        let response = match self.http_client.get(&url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("ollama list: request failed: {}", e);
                return Ok(vec![]);
            }
        };

        if !response.status().is_success() {
            warn!("ollama list: bad status {}", response.status());
            return Ok(vec![]);
        }

        let tags_resp: OllamaTagsResponse = match response.json().await {
            Ok(data) => data,
            Err(e) => {
                warn!("ollama list: failed to parse response: {}", e);
                return Ok(vec![]);
            }
        };

        let models = tags_resp
            .models
            .into_iter()
            .map(|model| ModelInfo {
                id: model.name.clone(),
                family: ModelFamily::Ollama,
                name: model.name,
                filename: None,
                size_bytes: model.size,
                quantization: String::new(),
                url: None,
                sha256: None,
                backend: ModelBackend::Ollama,
                status: ModelStatus::Ready,
            })
            .collect();

        Ok(models)
    }

    async fn download(&self, model_id: &str) -> Result<(), ModelError> {
        let url = format!("{}/api/pull", self.base_url);

        let body = serde_json::json!({
            "name": model_id,
            "stream": true
        });

        let response = self
            .http_client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ModelError::DownloadFailed(format!("request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ModelError::DownloadFailed(format!(
                "ollama pull returned {}",
                response.status()
            )));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| ModelError::DownloadFailed(format!("stream error: {}", e)))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete lines
            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                if let Ok(progress) = serde_json::from_str::<OllamaPullProgress>(&line) {
                    if progress.status.contains("pulling") && progress.total.is_some() {
                        let total = progress.total.unwrap_or(0);
                        let completed = progress.completed.unwrap_or(0);
                        self.fire_progress(DownloadEvent {
                            model_id: model_id.to_string(),
                            bytes_downloaded: completed,
                            bytes_total: total,
                            speed_bps: 0,
                        })
                        .await;
                    }

                    if progress.status == "success" {
                        return Ok(());
                    }
                }
            }
        }

        Ok(())
    }

    async fn cancel_download(&self, model_id: &str) -> Result<(), ModelError> {
        warn!(
            "ollama: cancel_download not supported for model '{}'",
            model_id
        );
        Ok(())
    }

    async fn delete(&self, model_id: &str) -> Result<(), ModelError> {
        let url = format!("{}/api/delete", self.base_url);

        let body = serde_json::json!({
            "name": model_id
        });

        let response = self
            .http_client
            .delete(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ModelError::Other(e.into()))?;

        match response.status() {
            status if status.is_success() => Ok(()),
            reqwest::StatusCode::NOT_FOUND => Err(ModelError::NotFound(model_id.to_string())),
            status => Err(ModelError::Other(anyhow::anyhow!(
                "delete failed with status {}",
                status
            ))),
        }
    }

    async fn load(&self, model_id: &str) -> Result<(), ModelError> {
        let url = format!("{}/api/generate", self.base_url);

        let body = serde_json::json!({
            "model": model_id,
            "keep_alive": "30m",
            "prompt": ""
        });

        let response = self
            .http_client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| ModelError::LoadFailed(format!("request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ModelError::LoadFailed(format!(
                "ollama generate returned {}",
                response.status()
            )));
        }

        // Consume the response body to ensure the request completes
        let _ = response.bytes().await;

        // Store the loaded model ID
        *self.loaded_model_id.write().await = Some(model_id.to_string());

        Ok(())
    }

    async fn unload(&self) -> Result<(), ModelError> {
        let loaded = self.loaded_model_id.read().await;

        if let Some(model_id) = loaded.as_ref() {
            let url = format!("{}/api/generate", self.base_url);

            let body = serde_json::json!({
                "model": model_id,
                "keep_alive": "0",
                "prompt": ""
            });

            let response = self
                .http_client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| ModelError::Other(e.into()))?;

            if !response.status().is_success() {
                warn!("ollama unload: request returned {}", response.status());
            }

            // Consume response
            let _ = response.bytes().await;
        }

        // Always clear, even if no model was loaded or request failed
        drop(loaded);
        *self.loaded_model_id.write().await = None;

        Ok(())
    }

    async fn loaded_model(&self) -> Result<Option<String>, ModelError> {
        let url = format!("{}/api/ps", self.base_url);

        if let Ok(response) = self.http_client.get(&url).send().await {
            if response.status().is_success() {
                if let Ok(ps_resp) = response.json::<OllamaPsResponse>().await {
                    if let Some(model) = ps_resp.models.first() {
                        return Ok(Some(model.name.clone()));
                    }
                }
            }
        }

        // Fallback: check local loaded_model_id
        Ok(self.loaded_model_id.read().await.clone())
    }

    async fn recommended_model(&self) -> Result<String, ModelError> {
        Ok("llama3.2:3b".to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_is_ollama_not_available() {
        let manager = OllamaModelManager::with_base_url("http://127.0.0.1:19999".to_string());
        assert!(!manager.is_available().await);
    }

    #[test]
    fn test_parse_tags_response() {
        let json = r#"{"models":[{"name":"llama3.2:3b","size":2147023008}]}"#;
        let resp: OllamaTagsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.models.len(), 1);
        assert_eq!(resp.models[0].name, "llama3.2:3b");
        assert_eq!(resp.models[0].size, 2147023008);
    }

    #[test]
    fn test_parse_pull_progress() {
        let json = r#"{"status":"pulling manifest","total":1000,"completed":500}"#;
        let progress: OllamaPullProgress = serde_json::from_str(json).unwrap();
        assert_eq!(progress.status, "pulling manifest");
        assert_eq!(progress.total, Some(1000));
        assert_eq!(progress.completed, Some(500));
    }

    #[test]
    fn test_parse_ps_response() {
        let json = r#"{"models":[{"name":"llama3.2:3b"}]}"#;
        let resp: OllamaPsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.models.len(), 1);
        assert_eq!(resp.models[0].name, "llama3.2:3b");
    }

    #[tokio::test]
    async fn test_list_returns_empty_when_unavailable() {
        let manager = OllamaModelManager::with_base_url("http://127.0.0.1:19999".to_string());
        let result = manager.list().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_recommended_model() {
        let manager = OllamaModelManager::new();
        let result = manager.recommended_model().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "llama3.2:3b");
    }

    #[tokio::test]
    async fn test_default_constructor() {
        let manager = OllamaModelManager::default();
        assert_eq!(manager.base_url, "http://127.0.0.1:11434");
    }

    #[tokio::test]
    async fn test_unload_when_nothing_loaded() {
        let manager = OllamaModelManager::new();
        let result = manager.unload().await;
        assert!(result.is_ok());
    }
}
