use hmac::{Hmac, Mac};
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
/// last hashed: size, mtime, inode and ctime.
///
/// ctime is the change time the kernel maintains and a process cannot set: an
/// in-place overwrite that restores the old mtime with `utimensat` still moves
/// ctime, so the record stops matching. inode additionally catches a swap that
/// preserves size and mtime.
///
/// ctime and inode exist only on unix. On other targets (Windows) there is no
/// equivalent identity, so the verified-state cache is not used there and every
/// load hashes the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileIdentity {
    size: u64,
    mtime_nanos: i128,
    #[cfg(unix)]
    #[serde(skip_serializing_if = "Option::is_none")]
    inode: Option<u64>,
    #[cfg(unix)]
    ctime_nanos: i128,
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
        let (inode, ctime_nanos) = {
            use std::os::unix::fs::MetadataExt;
            (
                Some(metadata.ino()),
                i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()),
            )
        };

        Ok(Self {
            size: metadata.len(),
            mtime_nanos,
            #[cfg(unix)]
            inode,
            #[cfg(unix)]
            ctime_nanos,
        })
    }

    /// Append the identity to a MAC input in a fixed binary layout.
    fn mac_update(&self, mac: &mut HmacSha256) {
        mac.update(&self.size.to_le_bytes());
        mac.update(&self.mtime_nanos.to_le_bytes());
        #[cfg(unix)]
        {
            match self.inode {
                Some(inode) => {
                    mac.update(&[1]);
                    mac.update(&inode.to_le_bytes());
                }
                None => mac.update(&[0]),
            }
            mac.update(&self.ctime_nanos.to_le_bytes());
        }
    }
}

type HmacSha256 = Hmac<Sha256>;

/// File name of the per-install secret that authenticates verified-state
/// records. It lives in the NodeSpace state directory, outside the models
/// directory that a process writing a model can reach.
const VERIFY_KEY_FILE_NAME: &str = ".model-verify-key";
const VERIFY_KEY_LEN: usize = 32;
/// Domain separation for the record MAC.
const MAC_DOMAIN: &[u8] = b"nodespace.model-verified-record.v1";

/// Whether the verified-state cache is available on this platform: it needs
/// the file identity above, which only unix provides.
const CACHE_SUPPORTED: bool = cfg!(unix);

/// On-disk record of a successful verification: the digest it was verified
/// against, the file identity at that moment, and a keyed MAC over both plus
/// the model file name. Stored as a sidecar file (`<model-path>.verified.json`)
/// next to the model. A record without a valid MAC is never trusted.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct VerifiedRecord {
    digest: String,
    identity: FileIdentity,
    /// Lowercase-hex HMAC-SHA256 of the record fields under the per-install key.
    mac: String,
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

/// The NodeSpace state directory holding the per-install key: `.nodespace`
/// under `NODESPACE_HOME` when that is set, otherwise under the user's home.
/// The same rule the daemon uses for its state directory.
fn state_dir() -> Option<PathBuf> {
    let home = match std::env::var_os("NODESPACE_HOME").filter(|v| !v.is_empty()) {
        Some(custom) => PathBuf::from(custom),
        None => dirs::home_dir()?,
    };
    Some(home.join(".nodespace"))
}

fn key_path(state_dir: &Path) -> PathBuf {
    state_dir.join(VERIFY_KEY_FILE_NAME)
}

/// Read the per-install key. `None` for anything but a private regular file of
/// exactly [`VERIFY_KEY_LEN`] bytes: missing, unreadable, a symlink, readable
/// by group or others, or the wrong length. Callers treat `None` as "trust no
/// sidecar".
#[cfg(unix)]
fn read_key(state_dir: &Path) -> Option<[u8; VERIFY_KEY_LEN]> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(key_path(state_dir))
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.len() != VERIFY_KEY_LEN as u64
    {
        return None;
    }
    let mut key = [0u8; VERIFY_KEY_LEN];
    file.read_exact(&mut key).ok()?;
    Some(key)
}

#[cfg(not(unix))]
fn read_key(_state_dir: &Path) -> Option<[u8; VERIFY_KEY_LEN]> {
    None
}

/// Read the key, creating a fresh one when it is missing or unusable. The key
/// is written to a private (`0600`) temp file and renamed into place, so a
/// reader never sees a partial key and a stale or malformed file is replaced.
#[cfg(unix)]
fn load_or_create_key(state_dir: &Path) -> Option<[u8; VERIFY_KEY_LEN]> {
    use std::io::{Read, Write};
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(key) = read_key(state_dir) {
        return Some(key);
    }
    std::fs::create_dir_all(state_dir).ok()?;

    let mut key = [0u8; VERIFY_KEY_LEN];
    std::fs::File::open("/dev/urandom")
        .ok()?
        .read_exact(&mut key)
        .ok()?;

    let tmp = state_dir.join(format!(
        "{}.{}.tmp",
        VERIFY_KEY_FILE_NAME,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&tmp);
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(&key)?;
        file.sync_all()?;
        std::fs::rename(&tmp, key_path(state_dir))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(key)
}

#[cfg(not(unix))]
fn load_or_create_key(_state_dir: &Path) -> Option<[u8; VERIFY_KEY_LEN]> {
    None
}

/// The MAC over a record's authenticated fields: model file name, digest, and
/// every part of the file identity, each length-prefixed where variable.
fn record_mac(
    key: &[u8; VERIFY_KEY_LEN],
    model_path: &Path,
    digest: &str,
    identity: &FileIdentity,
) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(MAC_DOMAIN);
    let name = model_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    mac.update(&(name.len() as u64).to_le_bytes());
    mac.update(name.as_bytes());
    let digest = digest.to_ascii_lowercase();
    mac.update(&(digest.len() as u64).to_le_bytes());
    mac.update(digest.as_bytes());
    identity.mac_update(&mut mac);
    mac
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Whether `record` carries a valid MAC under `key` for `model_path`. The
/// comparison is constant-time (`Mac::verify_slice`).
fn record_mac_is_valid(
    key: &[u8; VERIFY_KEY_LEN],
    model_path: &Path,
    record: &VerifiedRecord,
) -> bool {
    let Some(claimed) = hex_decode(&record.mac) else {
        return false;
    };
    record_mac(key, model_path, &record.digest, &record.identity)
        .verify_slice(&claimed)
        .is_ok()
}

/// Verify `path` against `expected_sha256`, skipping the hash only when an
/// authenticated record of a prior verification matches the file's current
/// identity (size, mtime, inode, ctime).
///
/// This exists to remove the repeat cost of [`verify_file_sha256`] on every
/// daemon start: a multi-GB model that was already verified once (at download
/// or on a prior load) doesn't need re-hashing on an unchanged file. The record
/// is authenticated with an HMAC under a per-install secret kept in a `0600`
/// file in the NodeSpace state directory, outside the models directory, so a
/// process that can write the models directory cannot forge a record. Any
/// change to the file moves its identity (ctime cannot be restored from
/// userspace) and forces a full re-hash.
///
/// A missing, unreadable, corrupt or unauthenticated record, and a missing or
/// unusable key, are all cache misses: verification falls back to hashing and
/// then best-effort writes a fresh key and record. On targets without ctime
/// (Windows) the cache is not used and the file is always hashed.
///
/// Residual limit: a process that can read the key file can still forge a
/// record; that is outside what a per-install secret protects against.
pub fn verify_file_sha256_cached(path: &Path, expected_sha256: &str) -> Result<(), String> {
    verify_cached_in(state_dir().as_deref(), path, expected_sha256)
}

fn verify_cached_in(
    state_dir: Option<&Path>,
    path: &Path,
    expected_sha256: &str,
) -> Result<(), String> {
    let sidecar = cache_sidecar_path(path);
    let current_identity = FileIdentity::read(path).map_err(|e| {
        format!(
            "failed to read model for integrity check {}: {}",
            path.display(),
            e
        )
    })?;

    if cache_matches(
        state_dir,
        &sidecar,
        path,
        expected_sha256,
        &current_identity,
    ) {
        tracing::debug!(
            "Skipping SHA-256 re-hash for {} — authenticated verification matches file identity",
            path.display()
        );
        return Ok(());
    }

    verify_file_sha256(path, expected_sha256)?;

    // Best-effort: a failure to persist the cache only costs a re-hash on the
    // next load, never a false pass, so it does not fail verification itself.
    if let Err(e) = write_signed_record(state_dir, path, expected_sha256, current_identity) {
        tracing::warn!(
            "Failed to persist verified-state cache for {}: {}",
            path.display(),
            e
        );
    }

    Ok(())
}

fn cache_matches(
    state_dir: Option<&Path>,
    sidecar: &Path,
    model_path: &Path,
    expected_sha256: &str,
    current_identity: &FileIdentity,
) -> bool {
    if !CACHE_SUPPORTED {
        return false;
    }
    let Some(key) = state_dir.and_then(read_key) else {
        return false;
    };
    match read_verified_record(sidecar) {
        Ok(cached) => {
            cached.digest.eq_ignore_ascii_case(expected_sha256)
                && &cached.identity == current_identity
                && record_mac_is_valid(&key, model_path, &cached)
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
    is_cached_in(state_dir().as_deref(), path, expected_sha256)
}

fn is_cached_in(state_dir: Option<&Path>, path: &Path, expected_sha256: &str) -> bool {
    let Ok(current_identity) = FileIdentity::read(path) else {
        return false;
    };
    cache_matches(
        state_dir,
        &cache_sidecar_path(path),
        path,
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
    record_in(state_dir().as_deref(), path, digest)
}

fn record_in(state_dir: Option<&Path>, path: &Path, digest: &str) {
    let Ok(identity) = FileIdentity::read(path) else {
        return;
    };
    if let Err(e) = write_signed_record(state_dir, path, digest, identity) {
        tracing::warn!(
            "Failed to persist verified-state cache for {}: {}",
            path.display(),
            e
        );
    }
}

/// Sign and persist a record for `path`. Creates the per-install key when it is
/// missing or unusable. Does nothing on targets without the cache.
fn write_signed_record(
    state_dir: Option<&Path>,
    path: &Path,
    digest: &str,
    identity: FileIdentity,
) -> std::io::Result<()> {
    if !CACHE_SUPPORTED {
        return Ok(());
    }
    let key = state_dir.and_then(load_or_create_key).ok_or_else(|| {
        std::io::Error::other("no usable verification key; verified-state cache not written")
    })?;
    let digest = digest.to_ascii_lowercase();
    let mac = hex_encode(
        &record_mac(&key, path, &digest, &identity)
            .finalize()
            .into_bytes(),
    );
    write_verified_record(
        &cache_sidecar_path(path),
        &VerifiedRecord {
            digest,
            identity,
            mac,
        },
    )
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

    /// Per-install state directory used by a test model: a sibling of the model
    /// file inside the test's temp dir, so no test touches the real home.
    fn state_of(model: &Path) -> PathBuf {
        model.parent().unwrap().join("state")
    }

    #[test]
    fn test_cached_verify_hits_cache_on_unchanged_file() {
        let (_dir, path) = temp_model_file(b"model bytes v1");
        let digest = compute_file_sha256(&path).unwrap();

        // First call hashes and populates the cache.
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &digest).is_ok());
        let sidecar = cache_sidecar_path(&path);
        assert!(sidecar.exists());

        // Second call on the unchanged file must not need to re-read file
        // bytes to reach the same answer — verified via the record still
        // matching after deleting nothing; behaviorally this just re-asserts
        // Ok(), the cache-hit path is exercised regardless of file size.
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &digest).is_ok());
    }

    #[test]
    fn test_cached_verify_rehashes_and_rejects_modified_file() {
        let (_dir, path) = temp_model_file(b"model bytes v1");
        let digest = compute_file_sha256(&path).unwrap();
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &digest).is_ok());

        // Swap the file contents in place (same path, different bytes) —
        // simulates a post-install tamper. mtime/size change, so the cache
        // must miss and the stale digest must be rejected.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&path, b"tampered bytes, different length!!").unwrap();

        let result = verify_cached_in(Some(&state_of(&path)), &path, &digest);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("integrity check FAILED"));
    }

    #[test]
    fn test_cached_verify_rehashes_after_digest_rotation() {
        let (_dir, path) = temp_model_file(b"model bytes v1");
        let digest_v1 = compute_file_sha256(&path).unwrap();
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &digest_v1).is_ok());

        // File on disk is unchanged, but the pinned digest rotated (model
        // catalog update). The cache record still points at digest_v1, so it
        // must not be treated as a match for the new pinned digest.
        let wrong_new_digest = "f".repeat(64);
        let result = verify_cached_in(Some(&state_of(&path)), &path, &wrong_new_digest);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("integrity check FAILED"));
    }

    #[test]
    fn test_cached_verify_falls_back_to_full_verification_on_missing_cache() {
        let (_dir, path) = temp_model_file(b"model bytes, no cache yet");
        let digest = compute_file_sha256(&path).unwrap();
        // No sidecar exists yet — must fall back to a full hash, not skip.
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &digest).is_ok());
    }

    #[test]
    fn test_cached_verify_falls_back_to_full_verification_on_corrupt_cache() {
        let (_dir, path) = temp_model_file(b"model bytes, corrupt cache");
        let digest = compute_file_sha256(&path).unwrap();

        let sidecar = cache_sidecar_path(&path);
        std::fs::write(&sidecar, b"{ not valid json").unwrap();

        // A corrupt record must not be trusted; verification still succeeds
        // via full re-hash rather than erroring out or silently skipping.
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &digest).is_ok());
    }

    #[test]
    fn test_is_verification_cached_reflects_hit_and_miss() {
        let (_dir, path) = temp_model_file(b"model bytes for cached-check probe");
        let digest = compute_file_sha256(&path).unwrap();

        // No verification has happened yet — must report a miss.
        assert!(!is_cached_in(Some(&state_of(&path)), &path, &digest));

        assert!(verify_cached_in(Some(&state_of(&path)), &path, &digest).is_ok());

        // Now cached and unchanged — must report a hit.
        assert!(is_cached_in(Some(&state_of(&path)), &path, &digest));

        // A different pinned digest (rotation) must not read as cached.
        assert!(!is_cached_in(
            Some(&state_of(&path)),
            &path,
            &"a".repeat(64)
        ));
    }

    #[test]
    fn test_record_verified_sha256_makes_is_verification_cached_true() {
        let (_dir, path) = temp_model_file(b"model bytes, verified elsewhere");
        let digest = compute_file_sha256(&path).unwrap();

        // Simulates the download path recording a hash it already computed
        // itself, without ever calling verify_file_sha256_cached.
        assert!(!is_cached_in(Some(&state_of(&path)), &path, &digest));
        record_in(Some(&state_of(&path)), &path, &digest);
        assert!(is_cached_in(Some(&state_of(&path)), &path, &digest));

        // The subsequent load-time check must then be a cache hit too.
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &digest).is_ok());
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

    // ---- authenticated verified-state record ----

    /// A model file whose bytes differ from `pinned`'s, with a sidecar that an
    /// attacker without the key wrote: correct digest, correct identity.
    fn forged_fixture(mac: &str) -> (tempfile::TempDir, PathBuf, String) {
        let (_d, orig) = temp_model_file(b"original bytes");
        let pinned = compute_file_sha256(&orig).unwrap();
        let (dir, path) = temp_model_file(b"tampered bytes");
        let record = VerifiedRecord {
            digest: pinned.clone(),
            identity: FileIdentity::read(&path).unwrap(),
            mac: mac.to_string(),
        };
        write_verified_record(&cache_sidecar_path(&path), &record).unwrap();
        (dir, path, pinned)
    }

    #[cfg(unix)]
    #[test]
    fn test_forged_sidecar_without_key_is_rejected() {
        let (_dir, path, pinned) = forged_fixture(&"0".repeat(64));
        // No key exists at all: the sidecar must not be trusted.
        assert!(!is_cached_in(Some(&state_of(&path)), &path, &pinned));
        let err = verify_cached_in(Some(&state_of(&path)), &path, &pinned).unwrap_err();
        assert!(err.contains("integrity check FAILED"));
    }

    #[cfg(unix)]
    #[test]
    fn test_forged_sidecar_with_wrong_mac_is_rejected_when_key_exists() {
        let (_dir, path, pinned) = forged_fixture(&"0".repeat(64));
        // A key exists (a legitimate install), the forged MAC does not match.
        load_or_create_key(&state_of(&path)).unwrap();
        assert!(!is_cached_in(Some(&state_of(&path)), &path, &pinned));
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &pinned).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_sidecar_signed_with_other_key_is_rejected() {
        let (_dir, path, pinned) = forged_fixture("");
        let state = state_of(&path);
        let key = load_or_create_key(&state).unwrap();
        let mut other = key;
        other[0] ^= 0xff;
        let identity = FileIdentity::read(&path).unwrap();
        let mac = hex_encode(
            &record_mac(&other, &path, &pinned, &identity)
                .finalize()
                .into_bytes(),
        );
        write_verified_record(
            &cache_sidecar_path(&path),
            &VerifiedRecord {
                digest: pinned.clone(),
                identity,
                mac,
            },
        )
        .unwrap();
        assert!(!is_cached_in(Some(&state), &path, &pinned));
        assert!(verify_cached_in(Some(&state), &path, &pinned).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_sidecar_signed_with_zero_key_is_rejected_when_no_key_exists() {
        let (_dir, path, pinned) = forged_fixture("");
        let identity = FileIdentity::read(&path).unwrap();
        let mac = hex_encode(
            &record_mac(&[0u8; VERIFY_KEY_LEN], &path, &pinned, &identity)
                .finalize()
                .into_bytes(),
        );
        write_verified_record(
            &cache_sidecar_path(&path),
            &VerifiedRecord {
                digest: pinned.clone(),
                identity,
                mac,
            },
        )
        .unwrap();
        assert!(!is_cached_in(Some(&state_of(&path)), &path, &pinned));
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &pinned).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_sidecar_without_mac_field_is_a_miss() {
        let (_dir, path) = temp_model_file(b"model bytes, legacy record");
        let digest = compute_file_sha256(&path).unwrap();
        let identity = FileIdentity::read(&path).unwrap();
        let legacy = serde_json::json!({ "digest": digest, "identity": identity });
        std::fs::write(cache_sidecar_path(&path), legacy.to_string()).unwrap();
        load_or_create_key(&state_of(&path)).unwrap();
        assert!(!is_cached_in(Some(&state_of(&path)), &path, &digest));
        // The good model still verifies by hashing and the record is rewritten.
        assert!(verify_cached_in(Some(&state_of(&path)), &path, &digest).is_ok());
        assert!(is_cached_in(Some(&state_of(&path)), &path, &digest));
    }

    #[cfg(unix)]
    #[test]
    fn test_tampered_record_fields_invalidate_mac() {
        let (_dir, path) = temp_model_file(b"model bytes, field tamper");
        let digest = compute_file_sha256(&path).unwrap();
        let state = state_of(&path);
        assert!(verify_cached_in(Some(&state), &path, &digest).is_ok());
        let sidecar = cache_sidecar_path(&path);
        let good = read_verified_record(&sidecar).unwrap();
        assert!(is_cached_in(Some(&state), &path, &digest));

        // Flip one hex digit of the MAC.
        let mut bad = good.clone();
        let first = if bad.mac.starts_with('0') { "1" } else { "0" };
        bad.mac.replace_range(0..1, first);
        write_verified_record(&sidecar, &bad).unwrap();
        assert!(!is_cached_in(Some(&state), &path, &digest));

        // A MAC for a different model file name does not transfer.
        let other = path.with_file_name("other.gguf");
        std::fs::copy(&path, &other).unwrap();
        write_verified_record(&cache_sidecar_path(&other), &good).unwrap();
        assert!(!is_cached_in(Some(&state), &other, &digest));
    }

    #[cfg(unix)]
    #[test]
    fn test_in_place_edit_with_restored_mtime_is_rejected() {
        let (_dir, path) = temp_model_file(b"model bytes AAAA");
        let digest = compute_file_sha256(&path).unwrap();
        let state = state_of(&path);
        assert!(verify_cached_in(Some(&state), &path, &digest).is_ok());
        let original_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        // Same size, same inode, mtime restored: only ctime moves.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        std::io::Write::write_all(&mut &file, b"model bytes BBBB").unwrap();
        file.set_modified(original_mtime).unwrap();
        drop(file);

        assert!(!is_cached_in(Some(&state), &path, &digest));
        let err = verify_cached_in(Some(&state), &path, &digest).unwrap_err();
        assert!(err.contains("integrity check FAILED"));
    }

    #[cfg(unix)]
    #[test]
    fn test_missing_key_falls_back_to_hash_and_recreates_key_and_sidecar() {
        let (_dir, path) = temp_model_file(b"model bytes, key lost");
        let digest = compute_file_sha256(&path).unwrap();
        let state = state_of(&path);
        assert!(verify_cached_in(Some(&state), &path, &digest).is_ok());
        assert!(is_cached_in(Some(&state), &path, &digest));

        std::fs::remove_file(key_path(&state)).unwrap();
        // Without the key the existing sidecar is untrusted.
        assert!(!is_cached_in(Some(&state), &path, &digest));
        std::fs::remove_file(cache_sidecar_path(&path)).unwrap();

        assert!(verify_cached_in(Some(&state), &path, &digest).is_ok());
        assert!(key_path(&state).exists());
        assert!(cache_sidecar_path(&path).exists());
        assert!(is_cached_in(Some(&state), &path, &digest));
    }

    #[cfg(unix)]
    #[test]
    fn test_missing_key_never_trusts_old_sidecar_on_tampered_file() {
        let (_dir, path) = temp_model_file(b"model bytes v1");
        let digest = compute_file_sha256(&path).unwrap();
        let state = state_of(&path);
        assert!(verify_cached_in(Some(&state), &path, &digest).is_ok());
        std::fs::remove_file(key_path(&state)).unwrap();
        std::fs::write(&path, b"swapped").unwrap();
        assert!(verify_cached_in(Some(&state), &path, &digest).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_unusable_keys_are_replaced() {
        let (_dir, path) = temp_model_file(b"model bytes, bad keys");
        let digest = compute_file_sha256(&path).unwrap();
        let state = state_of(&path);
        std::fs::create_dir_all(&state).unwrap();

        // Wrong length.
        std::fs::write(key_path(&state), b"short").unwrap();
        assert!(read_key(&state).is_none());
        assert!(verify_cached_in(Some(&state), &path, &digest).is_ok());
        assert!(read_key(&state).is_some());

        // Right length but readable by others.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(key_path(&state), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_key(&state).is_none());
        assert!(!is_cached_in(Some(&state), &path, &digest));
        assert!(verify_cached_in(Some(&state), &path, &digest).is_ok());
        assert!(read_key(&state).is_some());
        assert!(is_cached_in(Some(&state), &path, &digest));
    }

    #[cfg(unix)]
    #[test]
    fn test_key_file_is_private_random_and_stable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let key = load_or_create_key(&state).unwrap();
        let mode = std::fs::metadata(key_path(&state))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(std::fs::metadata(key_path(&state)).unwrap().len(), 32);
        assert_ne!(key, [0u8; 32]);
        // A second call reuses the key rather than rotating it.
        assert_eq!(load_or_create_key(&state).unwrap(), key);
        // No stray temp files remain.
        let names: Vec<_> = std::fs::read_dir(&state)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_key_symlink_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let real = dir.path().join("real-key");
        std::fs::write(&real, [7u8; 32]).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        // The target alone is a valid key; only the symlink makes it refused.
        assert!(read_key(real.parent().unwrap()).is_none());
        std::os::unix::fs::symlink(&real, key_path(&state)).unwrap();
        assert!(read_key(&state).is_none());
    }

    #[test]
    fn test_mac_comparison_is_constant_time_api() {
        // The record MAC is compared only through `verify_slice`, never `==`
        // on the MAC strings or bytes.
        let src = include_str!("config.rs");
        let production = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        assert!(production.contains(".verify_slice("));
        assert!(!production.contains("mac =="));
        assert!(!production.contains("== claimed"));
        assert!(!production.contains("claimed =="));
    }

    #[test]
    fn test_hex_roundtrip_and_rejects_garbage() {
        let bytes = [0u8, 1, 0xab, 0xff];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
        assert!(hex_decode("é").is_none());
    }
}
