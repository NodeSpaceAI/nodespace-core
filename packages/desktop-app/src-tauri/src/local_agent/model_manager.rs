//! GGUF model manager: discovery, download with resume, SHA-256 verification,
//! and lifecycle state tracking.
//!
//! Implements the [`ModelManager`] trait from `agent_types` for managing local
//! GGUF model files on disk. Models are downloaded from HuggingFace with HTTP
//! range-request resume support and verified via streaming SHA-256 hash.
//!
//! Issue #1000

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::agent_types::{
    ChatModelSpec, DownloadEvent, ModelError, ModelFamily, ModelInfo, ModelManager, ModelStatus,
};

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
    sha256: &'static str,
    context_window: u32,
    default_temperature: f32,
}

/// Ministral 3B -- fast, lightweight, identical tool reliability.
const MINISTRAL_3B: CatalogEntry = CatalogEntry {
    id: "ministral-3b-q4km",
    family: ModelFamily::Ministral,
    name: "Ministral 3B Instruct Q4_K_M",
    filename: "Ministral-3-3B-Instruct-2512-Q4_K_M.gguf",
    size_bytes: 2_000_000_000, // ~2.0 GB
    quantization: "Q4_K_M",
    url: "https://huggingface.co/lmstudio-community/Ministral-3B-Instruct-2412-GGUF/resolve/main/Ministral-3B-Instruct-2412-Q4_K_M.gguf",
    sha256: "e38fb0e9b1185bb7e87db1e5e648a05a52bef36a5f78ec99dc0ea38f9ab89c79",
    context_window: 131_072,
    default_temperature: 0.3,
};

/// Ministral 8B -- deeper reasoning, vision capable.
const MINISTRAL_8B: CatalogEntry = CatalogEntry {
    id: "ministral-8b-q4km",
    family: ModelFamily::Ministral,
    name: "Ministral 8B Instruct Q4_K_M",
    filename: "Ministral-3-8B-Instruct-2512-Q4_K_M.gguf",
    size_bytes: 4_800_000_000, // ~4.8 GB
    quantization: "Q4_K_M",
    url: "https://huggingface.co/lmstudio-community/Ministral-8B-Instruct-2412-GGUF/resolve/main/Ministral-8B-Instruct-2412-Q4_K_M.gguf",
    sha256: "a7b14fa18a3e78cac28e5b2caa1ef1e41ae1992e8e8acd18f57a9a632789ddb5",
    context_window: 131_072,
    default_temperature: 0.3,
};

/// All catalog entries, in preference order.
const CATALOG: &[&CatalogEntry] = &[&MINISTRAL_3B, &MINISTRAL_8B];

/// RAM threshold (in bytes) above which the 8B model is recommended.
const RAM_THRESHOLD_8B: u64 = 16 * 1024 * 1024 * 1024; // 16 GB

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
    on_progress: Arc<RwLock<Option<Box<dyn Fn(DownloadEvent) + Send + Sync>>>>,
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
            on_progress: Arc::new(RwLock::new(None)),
            loaded_model_id: Arc::new(RwLock::new(None)),
        })
    }

    /// Register a progress callback for download events.
    pub async fn set_progress_callback(&self, callback: Box<dyn Fn(DownloadEvent) + Send + Sync>) {
        let mut guard = self.on_progress.write().await;
        *guard = Some(callback);
    }

    /// Get the recommended model based on system RAM.
    fn recommended_model_id() -> &'static str {
        let total_ram = detect_system_ram();
        if total_ram > RAM_THRESHOLD_8B {
            MINISTRAL_8B.id
        } else {
            MINISTRAL_3B.id
        }
    }

    /// Get a [`ChatModelSpec`] for the recommended model.
    pub fn recommended_model_spec() -> ChatModelSpec {
        let id = Self::recommended_model_id();
        let entry = find_catalog_entry(id).expect("recommended model must exist in catalog");
        ChatModelSpec {
            model_id: entry.id.to_string(),
            context_window: entry.context_window,
            default_temperature: entry.default_temperature,
        }
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
                filename: entry.filename.to_string(),
                size_bytes: entry.size_bytes,
                quantization: entry.quantization.to_string(),
                url: entry.url.to_string(),
                sha256: entry.sha256.to_string(),
                status,
            });
        }

        Ok(models)
    }

    async fn download(&self, model_id: &str) -> Result<(), ModelError> {
        let entry = find_catalog_entry(model_id)?;

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
        let download_result = perform_download(
            http_client,
            url,
            partial_path.clone(),
            final_path.clone(),
            total_size,
            expected_sha256,
            model_id_owned.clone(),
            cancel_token,
            statuses.clone(),
            on_progress,
        )
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

/// Perform the HTTP download with resume support, then verify SHA-256.
async fn perform_download(
    client: reqwest::Client,
    url: String,
    partial_path: PathBuf,
    final_path: PathBuf,
    total_size: u64,
    expected_sha256: String,
    model_id: String,
    cancel_token: CancellationToken,
    statuses: Arc<RwLock<HashMap<String, ModelStatus>>>,
    on_progress: Arc<RwLock<Option<Box<dyn Fn(DownloadEvent) + Send + Sync>>>>,
) -> Result<(), ModelError> {
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
                            last_progress_report = now;
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
                                if let Some(cb) = guard.as_ref() {
                                    cb(DownloadEvent {
                                        model_id: model_id.clone(),
                                        bytes_downloaded,
                                        bytes_total: total_size,
                                        speed_bps: 0, // TODO: calculate from sliding window
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

    // Stream-verify SHA-256
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

/// Resolve the platform-appropriate models directory.
///
/// macOS: `~/Library/Application Support/NodeSpace/models/`
/// Linux: `~/.local/share/NodeSpace/models/`
/// Windows: `%APPDATA%/NodeSpace/models/`
fn default_models_dir() -> Result<PathBuf, ModelError> {
    let data_dir = dirs::data_dir().ok_or_else(|| {
        ModelError::Other(anyhow::anyhow!(
            "could not determine platform data directory"
        ))
    })?;
    Ok(data_dir.join("NodeSpace").join("models"))
}

/// Look up a catalog entry by model ID, returning `ModelError::NotFound` if absent.
fn find_catalog_entry(model_id: &str) -> Result<&'static CatalogEntry, ModelError> {
    CATALOG
        .iter()
        .find(|e| e.id == model_id)
        .copied()
        .ok_or_else(|| ModelError::NotFound(model_id.to_string()))
}

/// Detect total system RAM in bytes using `sysinfo`.
fn detect_system_ram() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.total_memory()
}

/// Check that enough disk space is available before starting a download.
fn check_disk_space(dir: &PathBuf, required_bytes: u64) -> Result<(), ModelError> {
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

    // -- Catalog tests -------------------------------------------------------

    #[tokio::test]
    async fn list_returns_all_catalog_models() {
        let (mgr, _tmp) = test_manager();
        let models = mgr.list().await.unwrap();
        assert_eq!(models.len(), 2);
        assert!(models.iter().any(|m| m.id == "ministral-3b-q4km"));
        assert!(models.iter().any(|m| m.id == "ministral-8b-q4km"));
    }

    #[tokio::test]
    async fn list_models_have_correct_metadata() {
        let (mgr, _tmp) = test_manager();
        let models = mgr.list().await.unwrap();

        let m3b = models.iter().find(|m| m.id == "ministral-3b-q4km").unwrap();
        assert_eq!(m3b.family, ModelFamily::Ministral);
        assert_eq!(m3b.quantization, "Q4_K_M");
        assert!(!m3b.url.is_empty());
        assert!(!m3b.sha256.is_empty());
        assert!(m3b.size_bytes > 0);
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
        std::fs::write(tmp.path().join(MINISTRAL_3B.filename), b"fake model data").unwrap();

        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();
        let models = mgr.list().await.unwrap();

        let m3b = models.iter().find(|m| m.id == "ministral-3b-q4km").unwrap();
        assert!(matches!(m3b.status, ModelStatus::Ready));

        // 8B should still be NotDownloaded
        let m8b = models.iter().find(|m| m.id == "ministral-8b-q4km").unwrap();
        assert!(matches!(m8b.status, ModelStatus::NotDownloaded));
    }

    // -- RAM recommendation tests --------------------------------------------

    #[tokio::test]
    async fn recommended_model_returns_valid_id() {
        let (mgr, _tmp) = test_manager();
        let rec = mgr.recommended_model().await.unwrap();
        assert!(
            rec == "ministral-3b-q4km" || rec == "ministral-8b-q4km",
            "unexpected recommendation: {}",
            rec
        );
    }

    #[test]
    fn recommended_spec_has_valid_fields() {
        let spec = GgufModelManager::recommended_model_spec();
        assert!(spec.model_id == "ministral-3b-q4km" || spec.model_id == "ministral-8b-q4km");
        assert!(spec.context_window > 0);
        assert!(spec.default_temperature > 0.0);
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
        let result = mgr.load("ministral-3b-q4km").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn load_on_ready_model_transitions_to_loaded() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(MINISTRAL_3B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("ministral-3b-q4km").await.unwrap();

        let loaded = mgr.loaded_model().await.unwrap();
        assert_eq!(loaded, Some("ministral-3b-q4km".to_string()));

        let models = mgr.list().await.unwrap();
        let m = models.iter().find(|m| m.id == "ministral-3b-q4km").unwrap();
        assert!(matches!(m.status, ModelStatus::Loaded));
    }

    #[tokio::test]
    async fn load_already_loaded_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(MINISTRAL_3B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("ministral-3b-q4km").await.unwrap();
        // Loading again should succeed (idempotent)
        mgr.load("ministral-3b-q4km").await.unwrap();
        assert_eq!(
            mgr.loaded_model().await.unwrap(),
            Some("ministral-3b-q4km".to_string())
        );
    }

    #[tokio::test]
    async fn load_different_model_unloads_previous() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(MINISTRAL_3B.filename), b"fake").unwrap();
        std::fs::write(tmp.path().join(MINISTRAL_8B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("ministral-3b-q4km").await.unwrap();
        mgr.load("ministral-8b-q4km").await.unwrap();

        assert_eq!(
            mgr.loaded_model().await.unwrap(),
            Some("ministral-8b-q4km".to_string())
        );

        // Previous model should be back to Ready
        let models = mgr.list().await.unwrap();
        let m3b = models.iter().find(|m| m.id == "ministral-3b-q4km").unwrap();
        assert!(
            matches!(m3b.status, ModelStatus::Ready),
            "expected Ready after unload, got {:?}",
            m3b.status
        );
    }

    #[tokio::test]
    async fn unload_sets_status_back_to_ready() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(MINISTRAL_3B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("ministral-3b-q4km").await.unwrap();
        mgr.unload().await.unwrap();

        assert_eq!(mgr.loaded_model().await.unwrap(), None);
        let models = mgr.list().await.unwrap();
        let m = models.iter().find(|m| m.id == "ministral-3b-q4km").unwrap();
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
        let path = tmp.path().join(MINISTRAL_3B.filename);
        std::fs::write(&path, b"fake model").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.delete("ministral-3b-q4km").await.unwrap();

        assert!(!path.exists());
        let models = mgr.list().await.unwrap();
        let m = models.iter().find(|m| m.id == "ministral-3b-q4km").unwrap();
        assert!(matches!(m.status, ModelStatus::NotDownloaded));
    }

    #[tokio::test]
    async fn delete_loaded_model_fails() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join(MINISTRAL_3B.filename), b"fake").unwrap();
        let mgr = GgufModelManager::with_dir(tmp.path().to_path_buf()).unwrap();

        mgr.load("ministral-3b-q4km").await.unwrap();
        let result = mgr.delete("ministral-3b-q4km").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_nonexistent_file_is_ok() {
        let (mgr, _tmp) = test_manager();
        // File doesn't exist, but model is in catalog -- should succeed
        mgr.delete("ministral-3b-q4km").await.unwrap();
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
        let path = mgr.model_path("ministral-3b-q4km").unwrap();
        assert_eq!(path, tmp.path().join(MINISTRAL_3B.filename));
    }

    // -- Disk space check test -----------------------------------------------

    #[test]
    fn check_disk_space_passes_for_small_requirement() {
        let tmp = TempDir::new().unwrap();
        // Requesting 1 byte should always pass
        check_disk_space(&tmp.path().to_path_buf(), 1).unwrap();
    }
}
