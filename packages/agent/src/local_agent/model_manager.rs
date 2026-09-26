//! GGUF model manager: discovery, download with resume, SHA-256 verification,
//! and lifecycle state tracking.
//!
//! Implements the [`ModelManager`] trait from `agent_types` for managing local
//! GGUF model files on disk. Models are downloaded from HuggingFace with HTTP
//! range-request resume support and verified via streaming SHA-256 hash.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::agent_types::{
    ChatModelSpec, DownloadEvent, ModelBackend, ModelError, ModelFamily, ModelInfo, ModelManager,
    ModelStatus,
};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Shared, thread-safe progress callbacks for download events, keyed by
/// model_id so concurrent downloads of different models don't clobber each
/// other's callback (registering or clearing one model's callback must not
/// affect another's).
type ProgressCallback = Arc<RwLock<HashMap<String, Box<dyn Fn(DownloadEvent) + Send + Sync>>>>;

// ---------------------------------------------------------------------------
// Catalog constants
// ---------------------------------------------------------------------------

/// Hard-coded model catalog entry.
struct CatalogEntry {
    id: &'static str,
    family: ModelFamily,
    name: &'static str,
    filename: &'static str,
    size_bytes: u64,
    quantization: &'static str,
    url: &'static str,
    /// SHA-256 hash of the file, lowercase hex, as reported by HuggingFace's
    /// LFS metadata for the pinned commit in `url`. Every catalog entry must
    /// carry a real hash -- `perform_download` treats an empty string as a
    /// hard configuration error, not an opt-out. Re-derive with:
    /// `curl -s "https://huggingface.co/api/models/<org>/<repo>?blobs=true"`
    /// and read the `lfs.sha256` field for the target file.
    sha256: &'static str,
    context_window: u32,
    default_temperature: f32,
    /// KV cache quantization for key tensors. `None` = F16 (default).
    type_k: Option<nodespace_nlp_engine::KvCacheQuantType>,
    /// KV cache quantization for value tensors. `None` = F16 (default).
    type_v: Option<nodespace_nlp_engine::KvCacheQuantType>,
    /// Minimum system RAM (in GiB) required to run this model comfortably.
    min_memory_gb: u8,
}

/// Gemma 4 E4B -- Google's efficient ~4B-effective model; the default
/// (16GB+ Apple Silicon).
const GEMMA_4_E4B: CatalogEntry = CatalogEntry {
    id: "gemma-4-e4b-q4km",
    family: ModelFamily::Gemma4,
    name: "Gemma 4 E4B Instruct Q4_K_M",
    filename: "gemma-4-E4B-it-Q4_K_M.gguf",
    size_bytes: 5_335_289_824, // ~5.0 GB
    quantization: "Q4_K_M",
    url: "https://huggingface.co/ggml-org/gemma-4-E4B-it-GGUF/resolve/2714b5519c6c3516b1000e7c5e1eba998dfe1fe8/gemma-4-E4B-it-Q4_K_M.gguf",
    sha256: "90ce98129eb3e8cc57e62433d500c97c624b1e3af1fcc85dd3b55ad7e0313e9f",
    context_window: 32_768,
    default_temperature: 0.3,
    type_k: None, // F16 — KV cache headroom adequate at E4B weight size
    type_v: None,
    min_memory_gb: 16,
};

/// Gemma 4 26B-A4B -- Google's MoE tier (25.2B total / 3.8B active experts).
/// Optional high-RAM tier, not the default: the agent-matrix eval scored it 12.3/16 mean across 3 reps, below Gemma 4 E4B's 13.7/16
/// -- it under-calls `search_nodes` on query-style follow-ups more often
/// than E4B. Across all 3 reps and every tool call attempted, argument JSON
/// was never malformed (no corrupted field names, no truncated nested
/// structures), unlike the dense Gemma 4 12B it replaced. Q8_0 (not Q4) to keep
/// quantization precision loss out of that comparison. Exposed as an
/// additional selectable tier for users with RAM to spare, not a
/// replacement for the E4B default (`recommended_model_id()` is unchanged).
const GEMMA_4_26B_A4B: CatalogEntry = CatalogEntry {
    id: "gemma-4-26b-a4b-q8",
    family: ModelFamily::Gemma4,
    name: "Gemma 4 26B-A4B Instruct Q8_0",
    filename: "gemma-4-26B-A4B-it-Q8_0.gguf",
    size_bytes: 26_859_860_992,
    quantization: "Q8_0",
    url: "https://huggingface.co/ggml-org/gemma-4-26B-A4B-it-GGUF/resolve/bb4531cda34d1ea09d9814959ed4d5833cf2a4c8/gemma-4-26B-A4B-it-Q8_0.gguf",
    sha256: "b5108fd13147d1c866bb595295bc9d56f5fe744d7209f18421031d0cc47009c6",
    context_window: 32_768,
    default_temperature: 0.3,
    type_k: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
    type_v: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
    min_memory_gb: 32,
};

/// All catalog entries, in preference order.
const CATALOG: &[&CatalogEntry] = &[&GEMMA_4_E4B, &GEMMA_4_26B_A4B];

// ---------------------------------------------------------------------------
// Download state (per-model)
// ---------------------------------------------------------------------------

/// Per-model state tracked during an active download.
struct ActiveDownload {
    cancel_token: CancellationToken,
}

// ---------------------------------------------------------------------------
// GgufModelManager
// ---------------------------------------------------------------------------

/// Concrete [`ModelManager`] for GGUF models stored on the local filesystem.
///
/// Thread-safe: all mutable state lives behind `Arc<RwLock<>>`.
pub struct GgufModelManager {
    /// Base directory where model files are stored.
    models_dir: PathBuf,
    /// Per-model status map (model_id -> status).
    statuses: Arc<RwLock<HashMap<String, ModelStatus>>>,
    /// Active download handles (model_id -> handle).
    active_downloads: Arc<RwLock<HashMap<String, ActiveDownload>>>,
    /// HTTP client for downloading models.
    http_client: reqwest::Client,
    /// Optional progress callback for download events.
    on_progress: ProgressCallback,
    /// ID of the currently loaded model (at most one).
    loaded_model_id: Arc<RwLock<Option<String>>>,
}

impl GgufModelManager {
    /// Create a new model manager using the platform-appropriate data directory.
    ///
    /// Creates the models directory if it does not exist.
    pub fn new() -> Result<Self, ModelError> {
        let models_dir = default_models_dir()?;
        Self::with_dir(models_dir)
    }

    /// Create a model manager with a specific directory (useful for testing).
    pub fn with_dir(models_dir: PathBuf) -> Result<Self, ModelError> {
        std::fs::create_dir_all(&models_dir).map_err(|e| {
            ModelError::Other(anyhow::anyhow!(
                "failed to create models directory {}: {}",
                models_dir.display(),
                e
            ))
        })?;

        let mut initial_statuses = HashMap::new();
        for entry in CATALOG {
            let path = models_dir.join(entry.filename);
            let status = if path.exists() {
                ModelStatus::Ready
            } else if models_dir
                .join(format!("{}.partial", entry.filename))
                .exists()
            {
                // Partial file from interrupted download
                ModelStatus::NotDownloaded
            } else {
                ModelStatus::NotDownloaded
            };
            initial_statuses.insert(entry.id.to_string(), status);
        }

        Ok(Self {
            models_dir,
            statuses: Arc::new(RwLock::new(initial_statuses)),
            active_downloads: Arc::new(RwLock::new(HashMap::new())),
            http_client: reqwest::Client::new(),
            on_progress: Arc::new(RwLock::new(HashMap::new())),
            loaded_model_id: Arc::new(RwLock::new(None)),
        })
    }

    /// Register a progress callback for download events for a specific model.
    pub async fn set_progress_callback(
        &self,
        model_id: &str,
        callback: Box<dyn Fn(DownloadEvent) + Send + Sync>,
    ) {
        let mut guard = self.on_progress.write().await;
        guard.insert(model_id.to_string(), callback);
    }

    /// Clear the registered progress callback for a specific model, dropping
    /// any resources (e.g. channel senders) it holds.
    pub async fn clear_progress_callback(&self, model_id: &str) {
        let mut guard = self.on_progress.write().await;
        guard.remove(model_id);
    }

    /// The default local model: Gemma 4 E4B, per ADR-056. Fixed rather than
    /// RAM-tiered -- 26B-A4B scored below E4B on the agent-matrix eval, so a
    /// machine with more RAM is not a reason to recommend it.
    fn recommended_model_id() -> &'static str {
        GEMMA_4_E4B.id
    }

    /// Get a [`ChatModelSpec`] for the recommended model.
    pub fn recommended_model_spec() -> ChatModelSpec {
        let id = Self::recommended_model_id();
        let entry = find_catalog_entry(id).expect("recommended model must exist in catalog");
        ChatModelSpec {
            model_id: entry.id.to_string(),
            family: entry.family,
            context_window: entry.context_window,
            default_temperature: entry.default_temperature,
            type_k: entry.type_k,
            type_v: entry.type_v,
        }
    }

    /// Look up the [`ModelFamily`] for a given model id.
    pub fn family_for(&self, model_id: &str) -> Result<ModelFamily, ModelError> {
        let entry = find_catalog_entry(model_id)?;
        Ok(entry.family)
    }

    /// Get a [`ChatModelSpec`] for any catalog model by id.
    pub fn model_spec_for(&self, model_id: &str) -> Result<ChatModelSpec, ModelError> {
        let entry = find_catalog_entry(model_id)?;
        Ok(ChatModelSpec {
            model_id: entry.id.to_string(),
            family: entry.family,
            context_window: entry.context_window,
            default_temperature: entry.default_temperature,
            type_k: entry.type_k,
            type_v: entry.type_v,
        })
    }

    /// Return the on-disk path for a model file.
    pub fn model_path(&self, model_id: &str) -> Result<PathBuf, ModelError> {
        let entry = find_catalog_entry(model_id)?;
        Ok(self.models_dir.join(entry.filename))
    }
}

// ---------------------------------------------------------------------------
// ModelManager trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl ModelManager for GgufModelManager {
    async fn list(&self) -> Result<Vec<ModelInfo>, ModelError> {
        let statuses = self.statuses.read().await;
        let mut models = Vec::with_capacity(CATALOG.len());

        for entry in CATALOG {
            let status = statuses
                .get(entry.id)
                .cloned()
                .unwrap_or(ModelStatus::NotDownloaded);

            models.push(ModelInfo {
                id: entry.id.to_string(),
                family: entry.family,
                name: entry.name.to_string(),
                filename: Some(entry.filename.to_string()),
                size_bytes: entry.size_bytes,
                quantization: entry.quantization.to_string(),
                url: Some(entry.url.to_string()),
                sha256: Some(entry.sha256.to_string()),
                backend: ModelBackend::Gguf,
                status,
                min_memory_gb: entry.min_memory_gb,
            });
        }

        Ok(models)
    }

    async fn download(&self, model_id: &str) -> Result<(), ModelError> {
        let entry = find_catalog_entry(model_id)?;

        // A missing SHA-256 is a catalog configuration error: every entry
        // must carry a verifiable hash, so refuse to even start the download
        // rather than fetching multiple GB and only failing verification at
        // the end.
        if entry.sha256.is_empty() {
            return Err(ModelError::VerificationFailed(format!(
                "catalog entry for '{}' has no SHA-256 configured",
                model_id
            )));
        }

        // Check current status
        {
            let statuses = self.statuses.read().await;
            if let Some(status) = statuses.get(model_id) {
                match status {
                    ModelStatus::Downloading { .. } => {
                        return Err(ModelError::DownloadInProgress(model_id.to_string()));
                    }
                    ModelStatus::Ready | ModelStatus::Loaded => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }

        // Check disk space
        check_disk_space(&self.models_dir, entry.size_bytes)?;

        // Create cancellation token for this download
        let cancel_token = CancellationToken::new();
        {
            let mut downloads = self.active_downloads.write().await;
            downloads.insert(
                model_id.to_string(),
                ActiveDownload {
                    cancel_token: cancel_token.clone(),
                },
            );
        }

        // Update status to Downloading
        {
            let mut statuses = self.statuses.write().await;
            statuses.insert(
                model_id.to_string(),
                ModelStatus::Downloading {
                    progress_pct: 0.0,
                    bytes_downloaded: 0,
                    bytes_total: entry.size_bytes,
                },
            );
        }

        let partial_path = self.models_dir.join(format!("{}.partial", entry.filename));
        let final_path = self.models_dir.join(entry.filename);
        let url = entry.url.to_string();
        let expected_sha256 = entry.sha256.to_string();
        let total_size = entry.size_bytes;
        let model_id_owned = model_id.to_string();

        let statuses = self.statuses.clone();
        let active_downloads = self.active_downloads.clone();
        let http_client = self.http_client.clone();
        let on_progress = self.on_progress.clone();

        // Perform download in a spawned task
        let download_result = perform_download(DownloadParams {
            client: http_client,
            url,
            partial_path: partial_path.clone(),
            final_path: final_path.clone(),
            total_size,
            expected_sha256,
            model_id: model_id_owned.clone(),
            cancel_token,
            statuses: statuses.clone(),
            on_progress,
        })
        .await;

        // Clean up active download tracking
        {
            let mut downloads = active_downloads.write().await;
            downloads.remove(&model_id_owned);
        }

        match download_result {
            Ok(()) => {
                let mut statuses = statuses.write().await;
                statuses.insert(model_id_owned, ModelStatus::Ready);
                Ok(())
            }
            Err(e) => {
                let mut statuses = statuses.write().await;
                statuses.insert(
                    model_id_owned,
                    ModelStatus::Error {
                        message: e.to_string(),
                    },
                );
                Err(e)
            }
        }
    }

    async fn cancel_download(&self, model_id: &str) -> Result<(), ModelError> {
        let entry = find_catalog_entry(model_id)?;

        let download = {
            let mut downloads = self.active_downloads.write().await;
            downloads.remove(model_id)
        };

        if let Some(active) = download {
            active.cancel_token.cancel();

            // Clean up partial file
            let partial_path = self.models_dir.join(format!("{}.partial", entry.filename));
            let _ = tokio::fs::remove_file(&partial_path).await;

            let mut statuses = self.statuses.write().await;
            statuses.insert(model_id.to_string(), ModelStatus::NotDownloaded);
            Ok(())
        } else {
            // No active download -- not an error, just a no-op
            Ok(())
        }
    }

    async fn delete(&self, model_id: &str) -> Result<(), ModelError> {
        let entry = find_catalog_entry(model_id)?;

        // Cannot delete a loaded model
        {
            let loaded = self.loaded_model_id.read().await;
            if loaded.as_deref() == Some(model_id) {
                return Err(ModelError::Other(anyhow::anyhow!(
                    "cannot delete model '{}' while it is loaded",
                    model_id
                )));
            }
        }

        let path = self.models_dir.join(entry.filename);
        if path.exists() {
            tokio::fs::remove_file(&path).await.map_err(|e| {
                ModelError::Other(anyhow::anyhow!(
                    "failed to delete model file {}: {}",
                    path.display(),
                    e
                ))
            })?;
        }

        // Also clean up any partial file
        let partial = self.models_dir.join(format!("{}.partial", entry.filename));
        let _ = tokio::fs::remove_file(&partial).await;

        let mut statuses = self.statuses.write().await;
        statuses.insert(model_id.to_string(), ModelStatus::NotDownloaded);
        Ok(())
    }

    async fn load(&self, model_id: &str) -> Result<(), ModelError> {
        let _entry = find_catalog_entry(model_id)?;

        // Verify model is Ready
        {
            let statuses = self.statuses.read().await;
            match statuses.get(model_id) {
                Some(ModelStatus::Ready) => {}
                Some(ModelStatus::Loaded) => return Ok(()),
                Some(status) => {
                    return Err(ModelError::LoadFailed(format!(
                        "model '{}' is not ready (current status: {:?})",
                        model_id, status
                    )));
                }
                None => {
                    return Err(ModelError::NotFound(model_id.to_string()));
                }
            }
        }

        // Unload current model if any
        self.unload().await?;

        // Mark as loaded (actual inference engine loading is handled by
        // ChatInferenceEngine, not the model manager)
        {
            let mut statuses = self.statuses.write().await;
            statuses.insert(model_id.to_string(), ModelStatus::Loaded);
        }
        {
            let mut loaded = self.loaded_model_id.write().await;
            *loaded = Some(model_id.to_string());
        }

        tracing::info!("Model '{}' marked as loaded", model_id);
        Ok(())
    }

    async fn unload(&self) -> Result<(), ModelError> {
        let previous = {
            let mut loaded = self.loaded_model_id.write().await;
            loaded.take()
        };

        if let Some(prev_id) = previous {
            let mut statuses = self.statuses.write().await;
            // Only revert to Ready if it was Loaded (don't clobber Error state)
            if matches!(statuses.get(&prev_id), Some(ModelStatus::Loaded)) {
                statuses.insert(prev_id.clone(), ModelStatus::Ready);
            }
            tracing::info!("Model '{}' unloaded", prev_id);
        }

        Ok(())
    }

    async fn loaded_model(&self) -> Result<Option<String>, ModelError> {
        let loaded = self.loaded_model_id.read().await;
        Ok(loaded.clone())
    }

    async fn recommended_model(&self) -> Result<String, ModelError> {
        Ok(Self::recommended_model_id().to_string())
    }
}

// ---------------------------------------------------------------------------
// Download implementation
// ---------------------------------------------------------------------------

/// Parameters for performing a model download.
struct DownloadParams {
    client: reqwest::Client,
    url: String,
    partial_path: PathBuf,
    final_path: PathBuf,
    total_size: u64,
    expected_sha256: String,
    model_id: String,
    cancel_token: CancellationToken,
    statuses: Arc<RwLock<HashMap<String, ModelStatus>>>,
    on_progress: ProgressCallback,
}

/// Perform the HTTP download with resume support, then verify SHA-256.
async fn perform_download(params: DownloadParams) -> Result<(), ModelError> {
    let DownloadParams {
        client,
        url,
        partial_path,
        final_path,
        total_size,
        expected_sha256,
        model_id,
        cancel_token,
        statuses,
        on_progress,
    } = params;
    use futures::StreamExt;

    // Determine resume offset from existing partial file
    let resume_offset = if partial_path.exists() {
        tokio::fs::metadata(&partial_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0)
    } else {
        0
    };

    tracing::info!(
        "Downloading model '{}' (resume from {} bytes)",
        model_id,
        resume_offset
    );

    // Build request with optional Range header
    let mut request = client.get(&url);
    if resume_offset > 0 {
        request = request.header("Range", format!("bytes={}-", resume_offset));
    }

    let response = request
        .send()
        .await
        .map_err(|e| ModelError::DownloadFailed(format!("HTTP request failed: {}", e)))?;

    let status_code = response.status();
    if !status_code.is_success() && status_code != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(ModelError::DownloadFailed(format!(
            "HTTP {} from {}",
            status_code, url
        )));
    }

    // If we requested a range but the server responded with 200 (not 206),
    // the server is sending the entire file from the beginning. Truncate the
    // partial file to avoid prepending stale bytes.
    let effective_offset = if resume_offset > 0 && status_code == reqwest::StatusCode::OK {
        tracing::warn!(
            "Server returned 200 instead of 206 for range request on '{}'; \
             truncating partial file and restarting from byte 0",
            model_id
        );
        // Truncate the existing partial file
        tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&partial_path)
            .await
            .map_err(|e| {
                ModelError::DownloadFailed(format!(
                    "failed to truncate partial file {}: {}",
                    partial_path.display(),
                    e
                ))
            })?;
        0u64
    } else {
        resume_offset
    };

    // Open file for append (resume) or create
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&partial_path)
        .await
        .map_err(|e| {
            ModelError::DownloadFailed(format!(
                "failed to open partial file {}: {}",
                partial_path.display(),
                e
            ))
        })?;

    let mut bytes_downloaded = effective_offset;
    let mut stream = std::pin::pin!(response.bytes_stream());
    let mut last_progress_report = std::time::Instant::now();
    let mut last_progress_bytes = effective_offset;
    let progress_interval = std::time::Duration::from_millis(250);

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                tracing::info!("Download of '{}' cancelled", model_id);
                drop(file);
                let _ = tokio::fs::remove_file(&partial_path).await;
                return Err(ModelError::DownloadFailed("download cancelled".to_string()));
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        file.write_all(&bytes).await.map_err(|e| {
                            ModelError::DownloadFailed(format!("write failed: {}", e))
                        })?;
                        bytes_downloaded += bytes.len() as u64;

                        // Throttled progress reporting
                        let now = std::time::Instant::now();
                        if now.duration_since(last_progress_report) >= progress_interval {
                            let elapsed = now.duration_since(last_progress_report);
                            let delta_bytes = bytes_downloaded - last_progress_bytes;
                            let speed_bps = if elapsed.as_secs_f64() > 0.0 {
                                (delta_bytes as f64 / elapsed.as_secs_f64()) as u64
                            } else {
                                0
                            };

                            last_progress_report = now;
                            last_progress_bytes = bytes_downloaded;

                            let pct = if total_size > 0 {
                                (bytes_downloaded as f32 / total_size as f32) * 100.0
                            } else {
                                0.0
                            };

                            // Update status map
                            {
                                let mut s = statuses.write().await;
                                s.insert(
                                    model_id.clone(),
                                    ModelStatus::Downloading {
                                        progress_pct: pct,
                                        bytes_downloaded,
                                        bytes_total: total_size,
                                    },
                                );
                            }

                            // Fire progress callback
                            {
                                let guard = on_progress.read().await;
                                if let Some(cb) = guard.get(&model_id) {
                                    cb(DownloadEvent {
                                        model_id: model_id.clone(),
                                        bytes_downloaded,
                                        bytes_total: total_size,
                                        speed_bps,
                                    });
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        return Err(ModelError::DownloadFailed(format!("stream error: {}", e)));
                    }
                    None => break, // download complete
                }
            }
        }
    }

    file.flush()
        .await
        .map_err(|e| ModelError::DownloadFailed(format!("flush failed: {}", e)))?;
    drop(file);

    tracing::info!(
        "Download complete for '{}' ({} bytes), verifying SHA-256...",
        model_id,
        bytes_downloaded
    );

    // Update status to Verifying
    {
        let mut s = statuses.write().await;
        s.insert(model_id.clone(), ModelStatus::Verifying);
    }

    // Stream-verify SHA-256. `download()` already rejects an empty hash
    // before starting the transfer, so this branch should be unreachable in
    // practice; it stays as defense-in-depth in case `perform_download` is
    // ever called from a path that skips that guard.
    if expected_sha256.is_empty() {
        let _ = tokio::fs::remove_file(&partial_path).await;
        return Err(ModelError::VerificationFailed(format!(
            "catalog entry for '{}' has no SHA-256 configured",
            model_id
        )));
    }
    let computed_hash = sha256_file(&partial_path).await?;
    if computed_hash != expected_sha256 {
        // Delete corrupted file
        let _ = tokio::fs::remove_file(&partial_path).await;
        return Err(ModelError::VerificationFailed(format!(
            "SHA-256 mismatch for '{}': expected {}, got {}",
            model_id, expected_sha256, computed_hash
        )));
    }
    tracing::info!("SHA-256 verified for '{}'", model_id);

    // Rename partial to final
    tokio::fs::rename(&partial_path, &final_path)
        .await
        .map_err(|e| {
            ModelError::DownloadFailed(format!(
                "failed to rename {} -> {}: {}",
                partial_path.display(),
                final_path.display(),
                e
            ))
        })?;

    // The stream-verify above already confirmed `final_path`'s bytes match
    // `expected_sha256`. Record that now so the load-time integrity gate
    // (`ChatEngine::load_model` / embedding load) can skip re-hashing this
    // multi-GB file on its very first load instead of re-deriving a fact
    // this download just established.
    nodespace_nlp_engine::config::record_verified_sha256(&final_path, &expected_sha256);

    Ok(())
}

/// Compute SHA-256 hash of a file via streaming reads.
async fn sha256_file(path: &PathBuf) -> Result<String, ModelError> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await.map_err(|e| {
        ModelError::VerificationFailed(format!(
            "failed to open file for verification {}: {}",
            path.display(),
            e
        ))
    })?;

    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024]; // 64 KB chunks

    loop {
        let n = file.read(&mut buf).await.map_err(|e| {
            ModelError::VerificationFailed(format!("read failed during verification: {}", e))
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let hash = hasher.finalize();
    Ok(format!("{:x}", hash))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the models directory.
///
/// Always `~/.nodespace/models/` — consistent with the database path
/// (`~/.nodespace/database/`) resolved by `resolve_db_path` in the daemon.
/// Both paths honour `$HOME`; override with `NODESPACED_DB_PATH` for tests.
fn default_models_dir() -> Result<PathBuf, ModelError> {
    let home = std::env::var("HOME").map_err(|_| {
        ModelError::Other(anyhow::anyhow!(
            "could not determine models directory: $HOME is unset"
        ))
    })?;
    Ok(PathBuf::from(home).join(".nodespace").join("models"))
}

/// Look up a catalog entry by model ID, returning `ModelError::NotFound` if absent.
fn find_catalog_entry(model_id: &str) -> Result<&'static CatalogEntry, ModelError> {
    CATALOG
        .iter()
        .find(|e| e.id == model_id)
        .copied()
        .ok_or_else(|| ModelError::NotFound(model_id.to_string()))
}

/// The pinned SHA-256 for a catalog chat model, looked up by its GGUF file name
/// (basename). Returns `None` for a file that is not in the catalog — a
/// user-supplied / custom model with no pinned digest, which the load path then
/// loads without an integrity gate. Used at load time to hand the expected
/// digest to the inference engine so a swapped-on-disk GGUF is refused.
pub fn expected_sha256_for_filename(filename: &str) -> Option<&'static str> {
    CATALOG
        .iter()
        .find(|e| e.filename == filename)
        .map(|e| e.sha256)
}

/// Detect total system RAM in bytes using `sysinfo`.
pub fn detect_system_ram() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.total_memory()
}

/// Check that enough disk space is available before starting a download.
fn check_disk_space(dir: &Path, required_bytes: u64) -> Result<(), ModelError> {
    use sysinfo::Disks;

    let disks = Disks::new_with_refreshed_list();
    for disk in disks.list() {
        // Find the disk whose mount point is a prefix of our directory
        let mount = disk.mount_point();
        if dir.starts_with(mount) {
            let available = disk.available_space();
            if available < required_bytes {
                return Err(ModelError::DownloadFailed(format!(
                    "insufficient disk space: need {} bytes, only {} available on {}",
                    required_bytes,
                    available,
                    mount.display()
                )));
            }
            return Ok(());
        }
    }

    // Could not determine disk space -- proceed optimistically
    tracing::warn!(
        "Could not determine available disk space for {}",
        dir.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a manager with a temp directory.
    fn test_manager() -> (GgufModelManager, TempDir) {
        let tmp = TempDir::new().unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();
        (mgr, tmp)
    }

    // -- Model-download terminal-state coverage ------------------------------
    //
    // `download()`/`cancel_download()` only ever resolve against the
    // hardcoded, real, multi-GB HuggingFace `CATALOG` entries — there is no
    // seam to point them at fake bytes. `perform_download` is where the actual
    // HTTP-download-and-verify logic lives, fully parameterized by
    // `DownloadParams` (including `url`), so tests below drive it directly
    // against a tiny in-process HTTP server instead of the network.

    /// Spawn a minimal one-shot HTTP/1.1 server that serves `body` for every
    /// accepted connection, ignoring the request entirely (no request
    /// parsing needed — these tests never send a Range header). Returns the
    /// `http://127.0.0.1:<port>/...` base URL. The listener task is detached
    /// and serves connections until the test process exits.
    async fn spawn_fake_model_server(body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake model server");
        let port = listener.local_addr().expect("local_addr").port();

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                tokio::spawn(async move {
                    // Drain the request line/headers; content is irrelevant.
                    let mut buf = [0u8; 1024];
                    let _ = stream.try_read(&mut buf);

                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes()).await;
                    let _ = stream.write_all(&body).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        format!("http://127.0.0.1:{port}")
    }

    /// Spawn a fake server that trickles `body` out a few bytes at a time with
    /// a delay between chunks, so a test has a window to cancel mid-stream.
    async fn spawn_slow_fake_model_server(
        body: Vec<u8>,
        chunk_delay: std::time::Duration,
    ) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind slow fake model server");
        let port = listener.local_addr().expect("local_addr").port();

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                let chunk_delay = chunk_delay;
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.try_read(&mut buf);

                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    if stream.write_all(header.as_bytes()).await.is_err() {
                        return;
                    }
                    for chunk in body.chunks(16) {
                        if stream.write_all(chunk).await.is_err() {
                            return;
                        }
                        let _ = stream.flush().await;
                        tokio::time::sleep(chunk_delay).await;
                    }
                    let _ = stream.shutdown().await;
                });
            }
        });

        format!("http://127.0.0.1:{port}")
    }

    fn test_download_params(
        url: String,
        partial_path: PathBuf,
        final_path: PathBuf,
        total_size: u64,
        expected_sha256: String,
        cancel_token: CancellationToken,
        on_progress: ProgressCallback,
    ) -> DownloadParams {
        DownloadParams {
            client: reqwest::Client::new(),
            url,
            partial_path,
            final_path,
            total_size,
            expected_sha256,
            model_id: "fake-test-model".to_string(),
            cancel_token,
            statuses: Arc::new(RwLock::new(HashMap::new())),
            on_progress,
        }
    }

    #[tokio::test]
    async fn perform_download_completes_and_verifies_sha256() {
        let tmp = TempDir::new().unwrap();
        let body = b"fake gguf model bytes for testing".to_vec();
        let expected_hash = format!("{:x}", Sha256::digest(&body));
        let url = spawn_fake_model_server(body.clone()).await;

        let partial_path = tmp.path().join("model.gguf.partial");
        let final_path = tmp.path().join("model.gguf");
        let on_progress: ProgressCallback = Arc::new(RwLock::new(HashMap::new()));

        let result = perform_download(test_download_params(
            format!("{url}/model.gguf"),
            partial_path.clone(),
            final_path.clone(),
            body.len() as u64,
            expected_hash,
            CancellationToken::new(),
            on_progress,
        ))
        .await;

        assert!(result.is_ok(), "download should succeed: {:?}", result);
        assert!(!partial_path.exists(), "partial file must be renamed away");
        assert!(final_path.exists(), "final file must exist");
        let written = tokio::fs::read(&final_path).await.unwrap();
        assert_eq!(written, body);
    }

    #[tokio::test]
    async fn perform_download_reports_progress_events() {
        let tmp = TempDir::new().unwrap();
        // `perform_download` throttles progress reports to once per 250ms, so
        // only the delay between the first and second chunk needs to exceed
        // that — two small chunks is enough to observe >=1 event without a
        // slow test. (16 bytes/chunk in `spawn_slow_fake_model_server`.)
        let body = vec![0xABu8; 32];
        let expected_hash = format!("{:x}", Sha256::digest(&body));
        let url =
            spawn_slow_fake_model_server(body.clone(), std::time::Duration::from_millis(260)).await;

        let partial_path = tmp.path().join("model.gguf.partial");
        let final_path = tmp.path().join("model.gguf");

        let events: Arc<std::sync::Mutex<Vec<DownloadEvent>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_cb = events.clone();
        let on_progress: ProgressCallback = Arc::new(RwLock::new(HashMap::new()));
        on_progress.write().await.insert(
            "fake-test-model".to_string(),
            Box::new(move |evt: DownloadEvent| {
                events_cb.lock().unwrap().push(evt);
            }),
        );

        let result = perform_download(test_download_params(
            format!("{url}/model.gguf"),
            partial_path,
            final_path,
            body.len() as u64,
            expected_hash,
            CancellationToken::new(),
            on_progress,
        ))
        .await;

        assert!(result.is_ok(), "download should succeed: {:?}", result);
        let captured = events.lock().unwrap();
        assert!(
            !captured.is_empty(),
            "at least one progress event must be reported for a multi-chunk download"
        );
        assert_eq!(captured[0].bytes_total, body.len() as u64);
    }

    #[tokio::test]
    async fn perform_download_sha256_mismatch_is_verification_failed() {
        let tmp = TempDir::new().unwrap();
        let body = b"real bytes that will not match the expected hash".to_vec();
        let url = spawn_fake_model_server(body.clone()).await;

        let partial_path = tmp.path().join("model.gguf.partial");
        let final_path = tmp.path().join("model.gguf");
        let on_progress: ProgressCallback = Arc::new(RwLock::new(HashMap::new()));

        let result = perform_download(test_download_params(
            format!("{url}/model.gguf"),
            partial_path.clone(),
            final_path.clone(),
            body.len() as u64,
            "0".repeat(64), // deliberately wrong hash
            CancellationToken::new(),
            on_progress,
        ))
        .await;

        assert!(matches!(result, Err(ModelError::VerificationFailed(_))));
        assert!(
            !partial_path.exists(),
            "corrupted/mismatched partial file must be deleted, not left on disk"
        );
        assert!(
            !final_path.exists(),
            "final file must never be created on verification failure"
        );
    }

    #[tokio::test]
    async fn perform_download_cancellation_removes_partial_and_terminates() {
        let tmp = TempDir::new().unwrap();
        let body = vec![0xCDu8; 4096];
        let expected_hash = format!("{:x}", Sha256::digest(&body));
        let url =
            spawn_slow_fake_model_server(body.clone(), std::time::Duration::from_millis(100)).await;

        let partial_path = tmp.path().join("model.gguf.partial");
        let final_path = tmp.path().join("model.gguf");
        let on_progress: ProgressCallback = Arc::new(RwLock::new(HashMap::new()));
        let cancel_token = CancellationToken::new();

        // Cancel shortly after the download starts, while the slow server is
        // still trickling bytes — a real mid-stream cancellation, not a
        // pre-cancelled no-op.
        let cancel_clone = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            cancel_clone.cancel();
        });

        let result = perform_download(test_download_params(
            format!("{url}/model.gguf"),
            partial_path.clone(),
            final_path.clone(),
            body.len() as u64,
            expected_hash,
            cancel_token,
            on_progress,
        ))
        .await;

        assert!(
            matches!(&result, Err(ModelError::DownloadFailed(msg)) if msg.contains("cancelled")),
            "expected a cancellation error, got: {:?}",
            result
        );
        assert!(
            !partial_path.exists(),
            "cancel must leave no lingering partial file on disk"
        );
        assert!(!final_path.exists());
    }

    // -- Catalog tests -------------------------------------------------------

    #[tokio::test]
    async fn list_returns_all_catalog_models() {
        let (mgr, _tmp) = test_manager();
        let models = mgr.list().await.unwrap();
        assert_eq!(models.len(), 2);
        assert!(models.iter().any(|m| m.id == "gemma-4-e4b-q4km"));
        assert!(models.iter().any(|m| m.id == "gemma-4-26b-a4b-q8"));
    }

    #[tokio::test]
    async fn list_includes_gemma4_entries_with_correct_metadata() {
        let (mgr, _tmp) = test_manager();
        let models = mgr.list().await.unwrap();

        let e4b = models.iter().find(|m| m.id == "gemma-4-e4b-q4km").unwrap();
        assert_eq!(e4b.family, ModelFamily::Gemma4);
        assert_eq!(e4b.quantization, "Q4_K_M");
        assert!(e4b.size_bytes > 5_000_000_000); // ~5.0 GB
        assert!(e4b
            .url
            .as_ref()
            .is_some_and(|u| u.contains("ggml-org/gemma-4-E4B-it-GGUF")));
        assert_eq!(e4b.min_memory_gb, 16);

        let g26 = models
            .iter()
            .find(|m| m.id == "gemma-4-26b-a4b-q8")
            .unwrap();
        assert_eq!(g26.family, ModelFamily::Gemma4);
        assert_eq!(g26.quantization, "Q8_0");
        assert!(g26.size_bytes > 26_000_000_000); // ~26.9 GB
        assert!(g26
            .url
            .as_ref()
            .is_some_and(|u| u.contains("ggml-org/gemma-4-26B-A4B-it-GGUF")));
        assert_eq!(g26.min_memory_gb, 32);
    }

    #[tokio::test]
    async fn every_catalog_entry_has_a_verifiable_sha256() {
        let (mgr, _tmp) = test_manager();
        let models = mgr.list().await.unwrap();
        for m in &models {
            let hash = m.sha256.as_deref().unwrap_or_default();
            assert_eq!(
                hash.len(),
                64,
                "model '{}' has an invalid/missing sha256: {:?}",
                m.id,
                m.sha256
            );
            assert!(
                hash.chars().all(|c| c.is_ascii_hexdigit()),
                "model '{}' sha256 is not lowercase hex: {}",
                m.id,
                hash
            );
        }
    }

    #[tokio::test]
    async fn every_catalog_entry_pins_a_commit_not_a_moving_ref() {
        let (mgr, _tmp) = test_manager();
        let models = mgr.list().await.unwrap();
        for m in &models {
            let url = m.url.as_deref().unwrap_or_default();
            assert!(
                !url.contains("/resolve/main/"),
                "model '{}' url pins a moving ref instead of a commit: {}",
                m.id,
                url
            );
            // A commit SHA is 40 lowercase hex chars, immediately after
            // "/resolve/" and followed by "/". Assert that segment actually
            // looks like a commit, not just "not literally main".
            let ref_segment = url
                .split("/resolve/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or_default();
            assert_eq!(
                ref_segment.len(),
                40,
                "model '{}' resolve ref '{}' is not a 40-char commit SHA: {}",
                m.id,
                ref_segment,
                url
            );
            assert!(
                ref_segment.chars().all(|c| c.is_ascii_hexdigit()),
                "model '{}' resolve ref '{}' is not hex: {}",
                m.id,
                ref_segment,
                url
            );
        }
    }

    #[test]
    fn no_catalog_entry_has_an_empty_sha256() {
        // download() hard-errors on an empty hash before starting the
        // transfer; this asserts the catalog never actually reaches that
        // guard in practice.
        for entry in CATALOG {
            assert!(
                !entry.sha256.is_empty(),
                "catalog entry '{}' must carry a sha256",
                entry.id
            );
        }
    }

    #[test]
    fn expected_sha256_for_filename_matches_catalog_and_none_for_unknown() {
        let entry = CATALOG.iter().next().expect("catalog is non-empty");
        // A real catalog filename resolves to its pinned 64-hex digest — this is
        // what the chat load gate verifies the on-disk GGUF against.
        assert_eq!(
            expected_sha256_for_filename(entry.filename),
            Some(entry.sha256)
        );
        assert_eq!(
            expected_sha256_for_filename(entry.filename).unwrap().len(),
            64
        );
        // An unknown / user-supplied filename resolves to None — the documented
        // escape hatch (load without a digest to verify against).
        assert_eq!(
            expected_sha256_for_filename("not-a-catalog-model.gguf"),
            None
        );
    }

    #[test]
    fn every_catalog_model_basename_resolves_to_its_pinned_digest() {
        // The chat load gate resolves the expected digest from the model file's
        // basename, and model_path() builds `models_dir.join(entry.filename)` — so
        // the basename is always the catalog filename. This guards the wiring
        // invariant: a regression that dropped the lookup (loading a catalog model
        // unverified) would break here, not slip through.
        for entry in CATALOG {
            assert_eq!(
                expected_sha256_for_filename(entry.filename),
                Some(entry.sha256),
                "catalog model '{}' basename must resolve to its pinned digest",
                entry.id
            );
        }
    }

    #[tokio::test]
    async fn list_reports_not_downloaded_for_fresh_dir() {
        let (mgr, _tmp) = test_manager();
        let models = mgr.list().await.unwrap();
        for m in &models {
            assert!(
                matches!(m.status, ModelStatus::NotDownloaded),
                "expected NotDownloaded for {}, got {:?}",
                m.id,
                m.status
            );
        }
    }

    #[tokio::test]
    async fn list_detects_existing_model_file_as_ready() {
        let tmp = TempDir::new().unwrap();
        // Pre-create a model file
        std::fs::write(tmp.path().join(GEMMA_4_E4B.filename), b"fake model data").unwrap();

        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();
        let models = mgr.list().await.unwrap();

        let e4b = models.iter().find(|m| m.id == "gemma-4-e4b-q4km").unwrap();
        assert!(matches!(e4b.status, ModelStatus::Ready));

        // 26B-A4B should still be NotDownloaded
        let g26 = models
            .iter()
            .find(|m| m.id == "gemma-4-26b-a4b-q8")
            .unwrap();
        assert!(matches!(g26.status, ModelStatus::NotDownloaded));
    }

    // -- RAM recommendation tests --------------------------------------------

    #[tokio::test]
    async fn recommended_model_returns_valid_id() {
        let (mgr, _tmp) = test_manager();
        let rec = mgr.recommended_model().await.unwrap();
        assert_eq!(
            rec, "gemma-4-e4b-q4km",
            "unexpected recommendation: {}",
            rec
        );
    }

    #[test]
    fn recommended_spec_has_valid_fields() {
        let spec = GgufModelManager::recommended_model_spec();
        assert_eq!(spec.model_id, "gemma-4-e4b-q4km");
        assert_eq!(spec.family, ModelFamily::Gemma4);
        assert!(spec.context_window > 0);
        assert!(spec.default_temperature > 0.0);
    }

    #[tokio::test]
    async fn model_spec_for_returns_catalog_values() {
        let (mgr, _tmp) = test_manager();

        let spec = mgr.model_spec_for("gemma-4-e4b-q4km").unwrap();
        assert_eq!(spec.model_id, "gemma-4-e4b-q4km");
        assert_eq!(spec.family, ModelFamily::Gemma4);
        assert_eq!(spec.context_window, 32_768);
        assert!(spec.default_temperature > 0.0);
        // E4B is small enough that F16 KV cache is not the bottleneck.
        assert!(spec.type_k.is_none());

        // 26B-A4B uses Q8_0 KV compression alongside its ~27GB weights.
        let spec = mgr.model_spec_for("gemma-4-26b-a4b-q8").unwrap();
        assert_eq!(spec.model_id, "gemma-4-26b-a4b-q8");
        assert_eq!(spec.family, ModelFamily::Gemma4);
        assert_eq!(
            spec.type_k,
            Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0)
        );
        assert_eq!(
            spec.type_v,
            Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0)
        );
    }

    #[tokio::test]
    async fn model_spec_for_unknown_returns_not_found() {
        let (mgr, _tmp) = test_manager();
        let result = mgr.model_spec_for("nonexistent-model");
        assert!(matches!(result, Err(ModelError::NotFound(_))));
    }

    #[test]
    fn detect_system_ram_returns_nonzero() {
        let ram = detect_system_ram();
        assert!(ram > 0, "system RAM should be > 0, got {}", ram);
    }

    // -- State machine tests -------------------------------------------------

    #[tokio::test]
    async fn load_on_not_downloaded_model_fails() {
        let (mgr, _tmp) = test_manager();
        let result = mgr.load("gemma-4-e4b-q4km").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn load_on_ready_model_transitions_to_loaded() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(GEMMA_4_E4B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("gemma-4-e4b-q4km").await.unwrap();

        let loaded = mgr.loaded_model().await.unwrap();
        assert_eq!(loaded, Some("gemma-4-e4b-q4km".to_string()));

        let models = mgr.list().await.unwrap();
        let m = models.iter().find(|m| m.id == "gemma-4-e4b-q4km").unwrap();
        assert!(matches!(m.status, ModelStatus::Loaded));
    }

    #[tokio::test]
    async fn load_already_loaded_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(GEMMA_4_E4B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("gemma-4-e4b-q4km").await.unwrap();
        // Loading again should succeed (idempotent)
        mgr.load("gemma-4-e4b-q4km").await.unwrap();
        assert_eq!(
            mgr.loaded_model().await.unwrap(),
            Some("gemma-4-e4b-q4km".to_string())
        );
    }

    #[tokio::test]
    async fn load_different_model_unloads_previous() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(GEMMA_4_E4B.filename), b"fake").unwrap();
        std::fs::write(tmp.path().join(GEMMA_4_26B_A4B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("gemma-4-e4b-q4km").await.unwrap();
        mgr.load("gemma-4-26b-a4b-q8").await.unwrap();

        assert_eq!(
            mgr.loaded_model().await.unwrap(),
            Some("gemma-4-26b-a4b-q8".to_string())
        );

        // Previous model should be back to Ready
        let models = mgr.list().await.unwrap();
        let e4b = models.iter().find(|m| m.id == "gemma-4-e4b-q4km").unwrap();
        assert!(
            matches!(e4b.status, ModelStatus::Ready),
            "expected Ready after unload, got {:?}",
            e4b.status
        );
    }

    #[tokio::test]
    async fn unload_sets_status_back_to_ready() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(GEMMA_4_E4B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("gemma-4-e4b-q4km").await.unwrap();
        mgr.unload().await.unwrap();

        assert_eq!(mgr.loaded_model().await.unwrap(), None);
        let models = mgr.list().await.unwrap();
        let m = models.iter().find(|m| m.id == "gemma-4-e4b-q4km").unwrap();
        assert!(matches!(m.status, ModelStatus::Ready));
    }

    #[tokio::test]
    async fn unload_when_nothing_loaded_is_ok() {
        let (mgr, _tmp) = test_manager();
        mgr.unload().await.unwrap();
    }

    // -- Delete tests --------------------------------------------------------

    #[tokio::test]
    async fn delete_removes_file_and_resets_status() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(GEMMA_4_E4B.filename);
        std::fs::write(&path, b"fake model").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.delete("gemma-4-e4b-q4km").await.unwrap();

        assert!(!path.exists());
        let models = mgr.list().await.unwrap();
        let m = models.iter().find(|m| m.id == "gemma-4-e4b-q4km").unwrap();
        assert!(matches!(m.status, ModelStatus::NotDownloaded));
    }

    #[tokio::test]
    async fn delete_loaded_model_fails() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(GEMMA_4_E4B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("gemma-4-e4b-q4km").await.unwrap();
        let result = mgr.delete("gemma-4-e4b-q4km").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_nonexistent_file_is_ok() {
        let (mgr, _tmp) = test_manager();
        // File doesn't exist, but model is in catalog -- should succeed
        mgr.delete("gemma-4-e4b-q4km").await.unwrap();
    }

    // -- Unknown model tests -------------------------------------------------

    #[tokio::test]
    async fn operations_on_unknown_model_return_not_found() {
        let (mgr, _tmp) = test_manager();

        assert!(matches!(
            mgr.download("nonexistent").await,
            Err(ModelError::NotFound(_))
        ));
        assert!(matches!(
            mgr.cancel_download("nonexistent").await,
            Err(ModelError::NotFound(_))
        ));
        assert!(matches!(
            mgr.delete("nonexistent").await,
            Err(ModelError::NotFound(_))
        ));
        assert!(matches!(
            mgr.load("nonexistent").await,
            Err(ModelError::NotFound(_))
        ));
    }

    // -- SHA-256 verification test -------------------------------------------

    #[tokio::test]
    async fn sha256_file_computes_correct_hash() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.bin");
        std::fs::write(&path, b"hello world").unwrap();

        let hash = sha256_file(&path.to_path_buf()).await.unwrap();
        // SHA-256 of "hello world"
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    // -- Model path test -----------------------------------------------------

    #[tokio::test]
    async fn model_path_returns_correct_path() {
        let (mgr, tmp) = test_manager();
        let path = mgr.model_path("gemma-4-e4b-q4km").unwrap();
        assert_eq!(path, tmp.path().join(GEMMA_4_E4B.filename));
    }

    // -- Disk space check test -----------------------------------------------

    #[test]
    fn check_disk_space_passes_for_small_requirement() {
        let tmp = TempDir::new().unwrap();
        // Requesting 1 byte should always pass
        check_disk_space(tmp.path(), 1).unwrap();
    }

    // -- Progress callback lifecycle --------------------------
    //
    // A download's gRPC response stream is backed by an mpsc channel whose
    // sender is cloned into the progress callback. If that callback is never
    // cleared after the download completes, the sender clone it holds keeps
    // the channel (and therefore the stream) open forever, hanging the
    // frontend's await on the streaming Tauri command indefinitely. These
    // tests verify `clear_progress_callback` actually drops the callback (and
    // whatever it's holding), rather than merely a no-op -- and that
    // callbacks are keyed per model_id, so completing one download can't
    // clobber another concurrently-downloading model's callback.

    #[tokio::test]
    async fn clear_progress_callback_drops_previously_set_callback() {
        let (mgr, _tmp) = test_manager();
        let (tx, rx) = tokio::sync::mpsc::channel::<()>(1);

        mgr.set_progress_callback(
            "gemma-4-26b-a4b-q8",
            Box::new(move |_evt| {
                // Hold a sender clone, mirroring the daemon's real callback.
                let _ = tx.try_send(());
            }),
        )
        .await;

        // Sender is alive via the stored callback: the channel is not yet closed.
        assert!(!rx.is_closed());

        mgr.clear_progress_callback("gemma-4-26b-a4b-q8").await;

        // With no other sender clones outstanding, dropping the callback
        // must drop its captured sender, which closes the channel.
        assert!(rx.is_closed());
    }

    #[tokio::test]
    async fn clear_progress_callback_is_idempotent_when_unset() {
        let (mgr, _tmp) = test_manager();
        // Clearing with nothing registered must not panic.
        mgr.clear_progress_callback("gemma-4-26b-a4b-q8").await;
        mgr.clear_progress_callback("gemma-4-26b-a4b-q8").await;
    }

    #[tokio::test]
    async fn clearing_one_models_callback_does_not_affect_a_concurrent_download() {
        // Regression test for the concurrency hazard a shared, unkeyed
        // callback slot would introduce: downloading two different models at
        // once and completing one must not silently kill the other's
        // in-flight progress/ready events.
        let (mgr, _tmp) = test_manager();
        let (tx_a, rx_a) = tokio::sync::mpsc::channel::<()>(1);
        let (tx_b, rx_b) = tokio::sync::mpsc::channel::<()>(1);

        mgr.set_progress_callback(
            "gemma-4-26b-a4b-q8",
            Box::new(move |_evt| {
                let _ = tx_a.try_send(());
            }),
        )
        .await;
        mgr.set_progress_callback(
            "gemma-4-e4b-q4km",
            Box::new(move |_evt| {
                let _ = tx_b.try_send(());
            }),
        )
        .await;

        // Model A's download finishes and its callback is cleared...
        mgr.clear_progress_callback("gemma-4-26b-a4b-q8").await;
        assert!(rx_a.is_closed());

        // ...but model B's download is still in flight, so its callback (and
        // the sender it holds) must remain untouched.
        assert!(!rx_b.is_closed());
        let guard = mgr.on_progress.read().await;
        assert!(guard.contains_key("gemma-4-e4b-q4km"));
        assert!(!guard.contains_key("gemma-4-26b-a4b-q8"));
    }
}
