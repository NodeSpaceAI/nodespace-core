/// Configuration for the embedding service using llama.cpp
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Offload all model layers to GPU. This is the llama.cpp convention
/// where any value >= total layers offloads everything.
pub const GPU_OFFLOAD_ALL_LAYERS: u32 = 99;

/// SHA-256 of the pinned embedding model `nomic-embed-text-v1.5.Q8_0.gguf`
/// (HuggingFace `nomic-ai/nomic-embed-text-v1.5-GGUF`, commit
/// `0188c9bf409793f810680a5a431e7b899c46104c`), lowercase hex.
///
/// This is the load-time half of the model-integrity gate (ADR-058); the
/// build-time download in `scripts/download-models.ts` pins the SAME digest,
/// and so does the desktop app's copy of its bundled model
/// (`packages/desktop-app/app-lib/src/bundled_model.rs`). Rotating the model
/// MUST update all three in the same change; the app's tests fail on a
/// mismatch.
pub const EMBEDDING_MODEL_SHA256: &str =
    "3e24342164b3d94991ba9692fdc0dd08e3fd7362e0aacc396a9a5c54a544c3b7";

/// Compute the SHA-256 of the file at `path`, streaming in 64 KB chunks so a
/// ~146 MB GGUF does not balloon memory. Returns the lowercase-hex digest.
pub fn compute_file_sha256(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Verify the on-disk file at `path` against an expected SHA-256 digest.
///
/// Returns `Err` describing the mismatch (or read failure) so the caller can
/// refuse to load a tampered or corrupt artifact. This closes the post-install
/// tamper window: even a correctly-downloaded model that is later swapped on
/// disk is rejected at load time. Shared by the embedding model (fixed pinned
/// digest) and the chat models (per-catalog-entry digest).
pub fn verify_file_sha256(path: &Path, expected_sha256: &str) -> Result<(), String> {
    let actual = compute_file_sha256(path).map_err(|e| {
        format!(
            "failed to read model for integrity check {}: {}",
            path.display(),
            e
        )
    })?;
    // Case-insensitive: a hex digest is semantically case-agnostic, so a correctly
    // pinned digest entered in any case still matches (`actual` is lowercase). This
    // keeps a legitimate model from being refused over digest casing.
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(format!(
            "model integrity check FAILED for {}: expected SHA-256 {}, got {}",
            path.display(),
            expected_sha256,
            actual
        ));
    }
    Ok(())
}

/// Verify the on-disk model at `path` against the pinned [`EMBEDDING_MODEL_SHA256`],
/// using the verified-state cache (see [`verify_file_sha256_cached`]).
pub fn verify_model_integrity(path: &Path) -> Result<(), String> {
    verify_file_sha256_cached(path, EMBEDDING_MODEL_SHA256)
}

/// Identity of a file on disk, used to detect whether it changed since it was
/// last hashed: size and mtime always apply; inode additionally applies on
/// unix, where it also catches a same-size/same-mtime swap (e.g. a backup
/// restore that preserves mtime) that size+mtime alone would miss.
///
/// On non-unix targets (Windows), identity is size+mtime only — this is a
/// strictly weaker guarantee than unix's size+mtime+inode, since a same-size
/// same-mtime swap on Windows would not be caught by identity alone (the
/// digest re-check on the next genuine cache miss still catches it
/// eventually, just not as early). A stable Windows file-identity equivalent
/// (`GetFileInformationByHandle`'s file index) could close this gap if it
/// becomes a real concern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileIdentity {
    size: u64,
    mtime_nanos: i128,
    #[cfg(unix)]
    #[serde(skip_serializing_if = "Option::is_none")]
    inode: Option<u64>,
}

impl FileIdentity {
    fn read(path: &Path) -> std::io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        let mtime_nanos = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as i128)
            .unwrap_or(0);

        #[cfg(unix)]
        let inode = {
            use std::os::unix::fs::MetadataExt;
            Some(metadata.ino())
        };

        Ok(Self {
            size: metadata.len(),
            mtime_nanos,
            #[cfg(unix)]
            inode,
        })
    }
}

/// On-disk record of a successful verification: the digest it was verified
/// against plus the file identity at that moment. Stored as a sidecar file
/// (`<model-path>.verified.json`) next to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerifiedRecord {
    digest: String,
    identity: FileIdentity,
}

/// Sidecar path for the verified-state cache record of `model_path`.
fn cache_sidecar_path(model_path: &Path) -> PathBuf {
    let mut name = model_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".verified.json");
    model_path.with_file_name(name)
}

/// Verify `path` against `expected_sha256`, skipping the hash when a prior
/// verification is cached and the file's identity (size, mtime, inode) still
/// matches what was recorded then.
///
/// This exists to remove the repeat cost of [`verify_file_sha256`] on every
/// daemon start: a multi-GB model that was already verified once (at download
/// or on a prior load) doesn't need re-hashing on an unchanged file. Any
/// change to the file — a different size, mtime, or inode — invalidates the
/// cache and forces a full re-hash, so the tamper-detection property is
/// unchanged: only an untouched file skips hashing. A missing, unreadable, or
/// corrupt cache record is treated as a cache miss (fail toward re-hashing,
/// never toward skipping).
pub fn verify_file_sha256_cached(path: &Path, expected_sha256: &str) -> Result<(), String> {
    let sidecar = cache_sidecar_path(path);
    let current_identity = FileIdentity::read(path).map_err(|e| {
        format!(
            "failed to read model for integrity check {}: {}",
            path.display(),
            e
        )
    })?;

    if cache_matches(&sidecar, expected_sha256, &current_identity) {
        tracing::debug!(
            "Skipping SHA-256 re-hash for {} — cached verification matches file identity",
            path.display()
        );
        return Ok(());
    }

    verify_file_sha256(path, expected_sha256)?;

    let record = VerifiedRecord {
        digest: expected_sha256.to_ascii_lowercase(),
        identity: current_identity,
    };
    // Best-effort: a failure to persist the cache only costs a re-hash on the
    // next load, never a false pass, so it does not fail verification itself.
    if let Err(e) = write_verified_record(&sidecar, &record) {
        tracing::warn!(
            "Failed to persist verified-state cache for {}: {}",
            path.display(),
            e
        );
    }

    Ok(())
}

fn cache_matches(sidecar: &Path, expected_sha256: &str, current_identity: &FileIdentity) -> bool {
    match read_verified_record(sidecar) {
        Ok(cached) => {
            cached.digest.eq_ignore_ascii_case(expected_sha256)
                && &cached.identity == current_identity
        }
        Err(_) => false,
    }
}

/// Whether [`verify_file_sha256_cached`] would skip re-hashing `path` right
/// now — i.e. a cache hit. Lets a caller decide whether to surface a
/// "verifying" phase to the user *before* calling the (possibly slow) verify:
/// a hit is about to return near-instantly and isn't worth reporting, while a
/// miss is about to spend real wall-clock time hashing.
///
/// Racy by nature (the file or cache could change between this call and the
/// actual verify), but the only consequence of the race is a UI label being
/// wrong for one load — never a change to what gets verified or refused.
pub fn is_verification_cached(path: &Path, expected_sha256: &str) -> bool {
    let Ok(current_identity) = FileIdentity::read(path) else {
        return false;
    };
    cache_matches(
        &cache_sidecar_path(path),
        expected_sha256,
        &current_identity,
    )
}

/// Record that `path` has already been verified against `digest` by a caller
/// that computed the hash itself (e.g. the download path's own streaming
/// verification), so [`verify_file_sha256_cached`] can skip re-hashing it on
/// the very next load. `digest` is trusted as-is — the caller is asserting it
/// already confirmed the match, not asking this function to check it.
///
/// Best-effort: a failure to persist is silently ignored, since the only
/// consequence is one extra re-hash on the next load, never a false pass.
pub fn record_verified_sha256(path: &Path, digest: &str) {
    let Ok(identity) = FileIdentity::read(path) else {
        return;
    };
    let sidecar = cache_sidecar_path(path);
    let record = VerifiedRecord {
        digest: digest.to_ascii_lowercase(),
        identity,
    };
    if let Err(e) = write_verified_record(&sidecar, &record) {
        tracing::warn!(
            "Failed to persist verified-state cache for {}: {}",
            path.display(),
            e
        );
    }
}

fn read_verified_record(sidecar: &Path) -> std::io::Result<VerifiedRecord> {
    let bytes = std::fs::read(sidecar)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn write_verified_record(sidecar: &Path, record: &VerifiedRecord) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Write to a temp file then rename, so a crash mid-write can't leave a
    // corrupt sidecar that a later read half-parses. `read_verified_record`
    // treats any parse failure as a cache miss regardless, but this keeps the
    // common case clean.
    let tmp = sidecar.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, sidecar)
}

/// Configuration for llama.cpp embedding model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model name or identifier
    pub model_name: String,

    /// Local model path (GGUF file)
    pub model_path: Option<PathBuf>,

    /// Number of GPU layers to offload. Use `GPU_OFFLOAD_ALL_LAYERS` (99) to offload all.
    pub n_gpu_layers: u32,

    /// Context size for embedding
    pub context_size: u32,

    /// Number of threads for CPU inference
    pub n_threads: i32,

    /// Maximum cache size (number of embeddings to cache)
    pub cache_capacity: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model_name: "nomic-embed-text-v1.5".to_string(),
            model_path: None,
            n_gpu_layers: GPU_OFFLOAD_ALL_LAYERS,
            context_size: 8192,
            n_threads: std::thread::available_parallelism()
                .map(|p| p.get() as i32)
                .unwrap_or(4),
            cache_capacity: 10000,
        }
    }
}

impl EmbeddingConfig {
    /// Get the model path.
    ///
    /// An explicit `model_path` is authoritative: it is returned when the file
    /// exists and is `NotFound` when it does not, never swapped for another
    /// file. A caller that owns a home directory resolves the path itself and
    /// passes it; this crate never substitutes the user's copy for it.
    ///
    /// Without one, the model is looked up in the user's home directory, the
    /// default for a standalone caller such as a live test:
    /// - macOS/Linux: ~/.nodespace/models/nomic-embed-text-v1.5.Q8_0.gguf
    /// - Windows: %USERPROFILE%\.nodespace\models\nomic-embed-text-v1.5.Q8_0.gguf
    ///
    /// Resolution matches on filename only; the caller then enforces
    /// [`verify_model_integrity`] against [`EMBEDDING_MODEL_SHA256`] before
    /// loading. Only the pinned Q8_0 *bytes* pass that gate, so a different
    /// quantization (e.g. the `.f16` fallback) or a custom `model_path` pointing
    /// at some other model resolves here but is then rejected at load time —
    /// this is deliberate, so nothing but the pinned artifact reaches llama.cpp.
    pub fn resolve_model_path(&self) -> Result<PathBuf, std::io::Error> {
        if let Some(path) = &self.model_path {
            if path.exists() {
                return Ok(path.clone());
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Model not found at {}", path.display()),
            ));
        }

        // Use centralized ~/.nodespace/models/ directory
        let home_dir = dirs::home_dir().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Cannot determine home directory",
            )
        })?;

        // Try multiple possible filenames
        let base_path = home_dir.join(".nodespace").join("models");
        let possible_names = [
            format!("{}.gguf", sanitize_model_name(&self.model_name)),
            format!("{}.Q8_0.gguf", sanitize_model_name(&self.model_name)),
            format!("{}.f16.gguf", sanitize_model_name(&self.model_name)),
        ];

        for name in &possible_names {
            let model_path = base_path.join(name);
            if model_path.exists() {
                return Ok(model_path);
            }
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "Model not found. Please download nomic-embed-text GGUF to ~/.nodespace/models/. Tried: {:?}",
                possible_names
            ),
        ))
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.model_name.is_empty() {
            return Err("model_name cannot be empty".to_string());
        }

        if self.context_size == 0 {
            return Err("context_size must be greater than 0".to_string());
        }

        if self.cache_capacity == 0 {
            return Err("cache_capacity must be greater than 0".to_string());
        }

        Ok(())
    }
}

/// Sanitize model name to be filesystem-safe
fn sanitize_model_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
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
        assert_eq!(config.model_name, "nomic-embed-text-v1.5");
        assert_eq!(config.n_gpu_layers, GPU_OFFLOAD_ALL_LAYERS);
        assert_eq!(config.context_size, 8192);
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

        // Invalid: zero context size
        config.model_name = "test".to_string();
        config.context_size = 0;
        assert!(config.validate().is_err());

        // Invalid: zero cache capacity
        config.context_size = 8192;
        config.cache_capacity = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_sanitize_model_name() {
        assert_eq!(sanitize_model_name("nomic/embed"), "nomic-embed");
        assert_eq!(sanitize_model_name("model:v1"), "model-v1");
        assert_eq!(sanitize_model_name("normal-name"), "normal-name");
    }

    #[test]
    fn test_pinned_digest_is_lowercase_hex_64() {
        assert_eq!(EMBEDDING_MODEL_SHA256.len(), 64);
        assert!(EMBEDDING_MODEL_SHA256
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn test_compute_file_sha256_known_vector() {
        use std::io::Write;
        // NIST SHA-256 test vector: sha256("abc").
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"abc").unwrap();
        let digest = compute_file_sha256(f.path()).unwrap();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn test_verify_model_integrity_rejects_mismatch() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"not the real model bytes").unwrap();
        let result = verify_model_integrity(f.path());
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("integrity check FAILED"));
        assert!(msg.contains(EMBEDDING_MODEL_SHA256));
    }

    #[test]
    fn test_verify_model_integrity_read_failure_is_error() {
        let result = verify_model_integrity(Path::new("/nonexistent/model.gguf"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to read model"));
    }

    #[test]
    fn test_verify_file_sha256_accepts_match_rejects_mismatch() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"chat model bytes").unwrap();
        let digest = compute_file_sha256(f.path()).unwrap();
        // Matching digest passes.
        assert!(verify_file_sha256(f.path(), &digest).is_ok());
        // A wrong digest is rejected with a descriptive error.
        let result = verify_file_sha256(f.path(), &"0".repeat(64));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("integrity check FAILED"));
    }

    /// Helper: a temp file with known content, living in its own temp dir so
    /// the `.verified.json` sidecar doesn't collide with other tests.
    fn temp_model_file(contents: &[u8]) -> (tempfile::TempDir, PathBuf) {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents).unwrap();
        drop(f);
        (dir, path)
    }

    #[test]
    fn test_cached_verify_hits_cache_on_unchanged_file() {
        let (_dir, path) = temp_model_file(b"model bytes v1");
        let digest = compute_file_sha256(&path).unwrap();

        // First call hashes and populates the cache.
        assert!(verify_file_sha256_cached(&path, &digest).is_ok());
        let sidecar = cache_sidecar_path(&path);
        assert!(sidecar.exists());

        // Second call on the unchanged file must not need to re-read file
        // bytes to reach the same answer — verified via the record still
        // matching after deleting nothing; behaviorally this just re-asserts
        // Ok(), the cache-hit path is exercised regardless of file size.
        assert!(verify_file_sha256_cached(&path, &digest).is_ok());
    }

    #[test]
    fn test_cached_verify_rehashes_and_rejects_modified_file() {
        let (_dir, path) = temp_model_file(b"model bytes v1");
        let digest = compute_file_sha256(&path).unwrap();
        assert!(verify_file_sha256_cached(&path, &digest).is_ok());

        // Swap the file contents in place (same path, different bytes) —
        // simulates a post-install tamper. mtime/size change, so the cache
        // must miss and the stale digest must be rejected.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, b"tampered bytes, different length!!").unwrap();

        let result = verify_file_sha256_cached(&path, &digest);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("integrity check FAILED"));
    }

    #[test]
    fn test_cached_verify_rehashes_after_digest_rotation() {
        let (_dir, path) = temp_model_file(b"model bytes v1");
        let digest_v1 = compute_file_sha256(&path).unwrap();
        assert!(verify_file_sha256_cached(&path, &digest_v1).is_ok());

        // File on disk is unchanged, but the pinned digest rotated (model
        // catalog update). The cache record still points at digest_v1, so it
        // must not be treated as a match for the new pinned digest.
        let wrong_new_digest = "f".repeat(64);
        let result = verify_file_sha256_cached(&path, &wrong_new_digest);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("integrity check FAILED"));
    }

    #[test]
    fn test_cached_verify_falls_back_to_full_verification_on_missing_cache() {
        let (_dir, path) = temp_model_file(b"model bytes, no cache yet");
        let digest = compute_file_sha256(&path).unwrap();
        // No sidecar exists yet — must fall back to a full hash, not skip.
        assert!(verify_file_sha256_cached(&path, &digest).is_ok());
    }

    #[test]
    fn test_cached_verify_falls_back_to_full_verification_on_corrupt_cache() {
        let (_dir, path) = temp_model_file(b"model bytes, corrupt cache");
        let digest = compute_file_sha256(&path).unwrap();

        let sidecar = cache_sidecar_path(&path);
        std::fs::write(&sidecar, b"{ not valid json").unwrap();

        // A corrupt record must not be trusted; verification still succeeds
        // via full re-hash rather than erroring out or silently skipping.
        assert!(verify_file_sha256_cached(&path, &digest).is_ok());
    }

    #[test]
    fn test_is_verification_cached_reflects_hit_and_miss() {
        let (_dir, path) = temp_model_file(b"model bytes for cached-check probe");
        let digest = compute_file_sha256(&path).unwrap();

        // No verification has happened yet — must report a miss.
        assert!(!is_verification_cached(&path, &digest));

        assert!(verify_file_sha256_cached(&path, &digest).is_ok());

        // Now cached and unchanged — must report a hit.
        assert!(is_verification_cached(&path, &digest));

        // A different pinned digest (rotation) must not read as cached.
        assert!(!is_verification_cached(&path, &"a".repeat(64)));
    }

    #[test]
    fn test_record_verified_sha256_makes_is_verification_cached_true() {
        let (_dir, path) = temp_model_file(b"model bytes, verified elsewhere");
        let digest = compute_file_sha256(&path).unwrap();

        // Simulates the download path recording a hash it already computed
        // itself, without ever calling verify_file_sha256_cached.
        assert!(!is_verification_cached(&path, &digest));
        record_verified_sha256(&path, &digest);
        assert!(is_verification_cached(&path, &digest));

        // The subsequent load-time check must then be a cache hit too.
        assert!(verify_file_sha256_cached(&path, &digest).is_ok());
    }

    /// A missing explicit `model_path` is `NotFound`, never the user's
    /// `~/.nodespace/models` copy. `HOME` points at a home holding a placeholder
    /// model (standing in for the real one) for the one synchronous call, then
    /// is restored; nothing else in this crate's unit tests reads it.
    #[test]
    fn test_missing_explicit_model_path_does_not_fall_back_to_home() {
        let user_home = tempfile::tempdir().unwrap();
        let home_models = user_home.path().join(".nodespace").join("models");
        std::fs::create_dir_all(&home_models).unwrap();
        std::fs::write(
            home_models.join("nomic-embed-text-v1.5.Q8_0.gguf"),
            b"placeholder",
        )
        .unwrap();
        let isolated = tempfile::tempdir().unwrap();
        let config = EmbeddingConfig {
            model_path: Some(isolated.path().join("models").join("absent.gguf")),
            ..Default::default()
        };

        let saved_home = std::env::var_os("HOME");
        std::env::set_var("HOME", user_home.path());
        let resolved = config.resolve_model_path();
        match saved_home {
            Some(home) => std::env::set_var("HOME", home),
            None => std::env::remove_var("HOME"),
        }

        let err = resolved.expect_err("a missing explicit path must not resolve");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn test_existing_explicit_model_path_is_chosen() {
        let (_dir, path) = temp_model_file(b"placeholder");
        let config = EmbeddingConfig {
            model_path: Some(path.clone()),
            ..Default::default()
        };

        assert_eq!(config.resolve_model_path().unwrap(), path);
    }

    #[test]
    fn test_cache_sidecar_path_is_adjacent_to_model() {
        let path = PathBuf::from("/models/gemma-4-E4B-it-Q4_K_M.gguf");
        let sidecar = cache_sidecar_path(&path);
        assert_eq!(
            sidecar,
            PathBuf::from("/models/gemma-4-E4B-it-Q4_K_M.gguf.verified.json")
        );
    }
}
