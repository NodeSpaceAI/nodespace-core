//! tonic `SettingsService` implementation.
//!
//! Reads and writes daemon configuration from `~/.nodespace/daemon.toml`.
//! Display preferences (theme, render_markdown) are UI-only and live in the
//! Tauri process — this service does not touch them.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};

use crate::nodespace::{
    settings_service_server::SettingsService as GrpcSettingsService, CaptureContentLevel,
    CaptureSettingsResponse, GetCaptureSettingsRequest, ListOpenAiCompatConfigsRequest,
    ListOpenAiCompatConfigsResponse, OpenAiCompatConfig as ProtoOpenAiCompatConfig,
    SetOpenAiCompatConfigsRequest, UpdateCaptureSettingsRequest,
};

/// On-disk representation of `~/.nodespace/daemon.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DaemonConfig {
    #[serde(default)]
    capture: CaptureConfig,
    #[serde(default)]
    openai_compat: OpenAiCompatSettings,
    #[serde(default)]
    mcp: McpConfig,
}

/// MCP passthrough-tool settings persisted in daemon.toml under `[mcp]`.
///
/// ADR-038's Trust Boundary requires "explicit user enablement" before an
/// external tool goes live -- `nodespace mcp` (the stdio server in
/// `packages/cli/src/commands/mcp.rs`) refuses to serve anything unless this
/// is `true`. Defaults to `false`, the same shape [`CaptureConfig`] uses for
/// its own opt-in `enabled` flag: a missing `[mcp]` section (no prior
/// `nodespace mcp install` run, or a config file that predates this field)
/// reads as "not enabled," never as "enabled by omission."
#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub struct McpConfig {
    #[serde(default)]
    pub enabled: bool,
}

/// OpenAI-compatible provider configs persisted in daemon.toml under
/// `[[openai_compat.configs]]`. The Settings GUI is the only writer; the
/// daemon reads this by UUID when loading an `openai-compat:<uuid>` model.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OpenAiCompatSettings {
    #[serde(default)]
    pub configs: Vec<OpenAiCompatConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiCompatConfig {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    /// Model identifier sent as the wire-protocol "model" field — distinct
    /// from `name`, which is only a cosmetic UI label.
    #[serde(default)]
    pub model: String,
    /// Cached verdicts from the routing-reliability probe (see
    /// `nodespace_agent::local_agent::routing_probe`), keyed by the served
    /// model each entry was measured against.
    ///
    /// A single config can back many served models: `/models` discovery lets
    /// one Ollama-style endpoint expose several (`openai-compat:<uuid>:<model>`
    /// — see `parse_openai_compat_id`), each probed and cached independently.
    /// A single scalar here would let one model's verdict leak onto another's
    /// load — a config discovering both `mistral:7b` (suppressed) and
    /// `llama3.1:8b` (clean) must not let the first probe's `false` disable
    /// injection for the second, which was never measured.
    ///
    /// A model absent from this map means "never probed" — e.g. a config
    /// created before this field existed, or a served model whose load has
    /// not completed since. Not part of the gRPC `OpenAiCompatConfig`
    /// message: this is daemon-owned cache state derived from a live probe,
    /// not something the Settings GUI writes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routing_ok: BTreeMap<String, bool>,
}

impl From<ProtoOpenAiCompatConfig> for OpenAiCompatConfig {
    fn from(c: ProtoOpenAiCompatConfig) -> Self {
        Self {
            id: c.id,
            name: c.name,
            base_url: c.base_url,
            api_key: c.api_key,
            model: c.model,
            // The gRPC message carries no probe verdicts — only the Settings
            // GUI writes through this conversion, and `set_open_ai_compat_configs`
            // is responsible for carrying still-valid cached verdicts forward
            // from the config it replaces, rather than this `From` impl
            // inventing any.
            routing_ok: BTreeMap::new(),
        }
    }
}

impl From<OpenAiCompatConfig> for ProtoOpenAiCompatConfig {
    fn from(c: OpenAiCompatConfig) -> Self {
        Self {
            id: c.id,
            name: c.name,
            base_url: c.base_url,
            api_key: c.api_key,
            model: c.model,
        }
    }
}

/// Session capture settings persisted in daemon.toml under `[capture]`.
#[derive(Debug, Serialize, Deserialize)]
pub struct CaptureConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub content: CaptureContentSetting,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            content: CaptureContentSetting::MetadataOnly,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureContentSetting {
    #[default]
    MetadataOnly,
    Summary,
    Full,
}

impl From<CaptureContentSetting> for CaptureContentLevel {
    fn from(s: CaptureContentSetting) -> Self {
        match s {
            CaptureContentSetting::MetadataOnly => CaptureContentLevel::MetadataOnly,
            CaptureContentSetting::Summary => CaptureContentLevel::Summary,
            CaptureContentSetting::Full => CaptureContentLevel::Full,
        }
    }
}

impl From<CaptureContentLevel> for CaptureContentSetting {
    fn from(l: CaptureContentLevel) -> Self {
        match l {
            CaptureContentLevel::MetadataOnly => CaptureContentSetting::MetadataOnly,
            CaptureContentLevel::Summary => CaptureContentSetting::Summary,
            CaptureContentLevel::Full => CaptureContentSetting::Full,
        }
    }
}

/// Read capture settings from the config file at the given path. Used by the
/// capture service to decide whether and how to capture a completed session.
pub async fn read_capture_settings(config_path: &std::path::Path) -> anyhow::Result<CaptureConfig> {
    match tokio::fs::read_to_string(config_path).await {
        Ok(contents) => {
            let config: DaemonConfig = toml::from_str(&contents)
                .map_err(|e| anyhow::anyhow!("failed to parse daemon config: {}", e))?;
            Ok(config.capture)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(CaptureConfig::default()),
        Err(e) => Err(anyhow::anyhow!("failed to read daemon config: {}", e)),
    }
}

/// Read every OpenAI-compatible provider config. Used by `LocalAgentService`
/// to query each configured endpoint for the models it serves.
///
/// A missing config file is not an error — it just means no providers are
/// configured yet.
pub async fn load_openai_compat_configs(
    config_path: &std::path::Path,
) -> anyhow::Result<Vec<OpenAiCompatConfig>> {
    match tokio::fs::read_to_string(config_path).await {
        Ok(contents) => {
            let config: DaemonConfig = toml::from_str(&contents)
                .map_err(|e| anyhow::anyhow!("failed to parse daemon config: {}", e))?;
            Ok(config.openai_compat.configs)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(anyhow::anyhow!("failed to read daemon config: {}", e)),
    }
}

/// Look up a single OpenAI-compatible provider config by UUID. Used by
/// `LocalAgentService` when loading an `openai-compat:<uuid>` model.
pub async fn find_openai_compat_config(
    config_path: &std::path::Path,
    id: &str,
) -> anyhow::Result<Option<OpenAiCompatConfig>> {
    match tokio::fs::read_to_string(config_path).await {
        Ok(contents) => {
            let config: DaemonConfig = toml::from_str(&contents)
                .map_err(|e| anyhow::anyhow!("failed to parse daemon config: {}", e))?;
            Ok(config
                .openai_compat
                .configs
                .into_iter()
                .find(|c| c.id == id))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("failed to read daemon config: {}", e)),
    }
}

/// Read the MCP passthrough-tool's enablement setting from the config file at
/// the given path. Used by `nodespace mcp` (the CLI's stdio server, a
/// separate process from the daemon -- see `packages/cli/src/commands/mcp.rs`)
/// to decide whether it may serve anything at all. A missing file reads as
/// "not enabled," matching [`McpConfig`]'s default -- there is no daemon
/// running required to make this call; it is a plain file read against
/// `~/.nodespace/daemon.toml`, the same file the daemon itself reads via
/// [`SettingsServiceImpl`].
pub async fn read_mcp_settings(config_path: &std::path::Path) -> anyhow::Result<McpConfig> {
    match tokio::fs::read_to_string(config_path).await {
        Ok(contents) => {
            let config: DaemonConfig = toml::from_str(&contents)
                .map_err(|e| anyhow::anyhow!("failed to parse daemon config: {}", e))?;
            Ok(config.mcp)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(McpConfig::default()),
        Err(e) => Err(anyhow::anyhow!("failed to read daemon config: {}", e)),
    }
}

/// Set the MCP passthrough-tool's enablement flag, preserving every other
/// section of `daemon.toml` untouched. This is the explicit-user-enablement
/// write ADR-038's Trust Boundary requires: `nodespace mcp install`/
/// `uninstall` (see `packages/cli/src/commands/mcp.rs`) are the only callers,
/// turning the flag on only once a client's MCP config has actually been
/// written, and off again on uninstall.
///
/// A direct, unlocked read-modify-write against the file -- the same shape
/// [`record_routing_probe_verdict`] already uses to write `daemon.toml` from
/// outside `SettingsServiceImpl`'s RPC surface, for the same reason: this is
/// called from the separate `nodespace` CLI process, which has no RPC
/// connection to serialize through. The realistic race (a concurrent
/// Settings-GUI save via `SettingsServiceImpl::write_config`) is the same
/// small, accepted window documented there.
pub async fn set_mcp_enabled(config_path: &std::path::Path, enabled: bool) -> anyhow::Result<()> {
    let mut config: DaemonConfig = match tokio::fs::read_to_string(config_path).await {
        Ok(contents) => toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse daemon config: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DaemonConfig::default(),
        Err(e) => return Err(anyhow::anyhow!("failed to read daemon config: {e}")),
    };
    config.mcp.enabled = enabled;

    if let Some(parent) = config_path.parent() {
        // Owner-only from birth: this directory holds daemon.toml, which can
        // carry third-party API keys alongside this flag.
        crate::create_dir_owner_only(parent)
            .await
            .map_err(|e| anyhow::anyhow!("failed to create config directory: {e}"))?;
    }
    let serialized = toml::to_string_pretty(&config)
        .map_err(|e| anyhow::anyhow!("failed to serialize daemon config: {e}"))?;
    write_config_atomic(config_path, &serialized)
        .await
        .map_err(|e| match e {
            WriteConfigAtomicError::Write(e) => {
                anyhow::anyhow!("failed to write daemon config: {e}")
            }
            #[cfg(unix)]
            WriteConfigAtomicError::Chmod(e) => {
                anyhow::anyhow!("failed to set daemon config permissions: {e}")
            }
            WriteConfigAtomicError::Rename(e) => {
                anyhow::anyhow!("failed to finalize daemon config write: {e}")
            }
        })
}

/// Which step of [`write_config_atomic`]'s temp-file-then-rename sequence
/// failed, so each caller can report a step-appropriate message while
/// sharing the identical crash-safety mechanics.
enum WriteConfigAtomicError {
    Write(std::io::Error),
    #[cfg(unix)]
    Chmod(std::io::Error),
    Rename(std::io::Error),
}

/// Write `contents` to `config_path` via a temp file created in the same
/// directory (so the final rename is an atomic same-filesystem move),
/// followed by an atomic rename over the real path — rather than truncating
/// `config_path` in place. This guarantees `config_path` always holds
/// either its previous complete content or the new complete content, never
/// a partial write, even if the process crashes or loses power mid-write.
///
/// Shared by `SettingsServiceImpl::write_config` (the RPC-driven
/// read-modify-write, serialized under `write_lock`) and
/// `record_routing_probe_verdict` (a best-effort, unlocked write from
/// outside any RPC — see that function's own doc comment for why it's
/// intentionally not coordinated with `write_lock`): both persist
/// `daemon.toml` and both need the same crash-safety guarantee, independent
/// of that locking question.
///
/// On Unix, the temp file is created already restricted to owner-only via
/// `OpenOptions::mode(0o600)` (see `SettingsServiceImpl::
/// create_tmp_file_owner_only`), since `daemon.toml` can hold real
/// third-party API keys — with an explicit `set_permissions` backstop
/// afterward for the case where a stale temp file from an interrupted prior
/// attempt already exists at `tmp_path`.
///
/// The temp filename is unique per *call*, not just per process
/// (`std::process::id()` alone is not enough): `write_config` and
/// `record_routing_probe_verdict` both resolve to the same
/// `~/.nodespace/daemon.toml` and can run concurrently on the daemon's
/// multi-threaded runtime — `record_routing_probe_verdict` is deliberately
/// unlocked (see its doc comment). A per-process-only name would let two
/// concurrent calls open, write, and rename the *same* temp path
/// (`create_tmp_file_owner_only` uses `truncate(true)`, not
/// `create_new(true)`, so a second opener silently reuses/truncates the
/// first's in-flight file), which could surface a spurious error on an
/// unrelated save or interleave mixed content before either side renames.
/// The monotonic counter guarantees distinct paths across concurrent calls
/// within one process; the pid keeps them distinct across process restarts
/// too, matching the original intent.
async fn write_config_atomic(
    config_path: &std::path::Path,
    contents: &str,
) -> Result<(), WriteConfigAtomicError> {
    static CALL_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let call_id = CALL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_file_name = format!(
        "{}.tmp-{}-{}",
        config_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("daemon.toml"),
        std::process::id(),
        call_id,
    );
    let tmp_path = config_path.with_file_name(tmp_file_name);

    #[cfg(unix)]
    let write_result: std::io::Result<()> = async {
        use tokio::io::AsyncWriteExt;
        let mut file = SettingsServiceImpl::create_tmp_file_owner_only(&tmp_path).await?;
        file.write_all(contents.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }
    .await;
    #[cfg(not(unix))]
    let write_result = tokio::fs::write(&tmp_path, contents).await;

    // A failure partway through this write (e.g. ENOSPC) can still leave a
    // partial file at `tmp_path` — clean it up rather than leaving a stray,
    // possibly secret-bearing file behind.
    if let Err(e) = write_result {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(WriteConfigAtomicError::Write(e));
    }

    // Defensive backstop, not the primary mechanism (see
    // `create_tmp_file_owner_only`): the kernel only applies
    // `OpenOptions::mode` when `open(2)` actually creates the file. If a
    // stale temp file from an interrupted prior attempt already exists at
    // `tmp_path` with different permissions, `create(true)` opens it as-is
    // and the mode argument is silently ignored — so this explicit
    // `set_permissions` call still runs to cover that case. Owner-only:
    // other local accounts on a shared machine must not be able to read the
    // API keys this file can contain. Unix-only; this is a macOS/Linux
    // desktop app.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let result = tokio::fs::set_permissions(&tmp_path, perms).await;
        #[cfg(test)]
        let result = if FAIL_NEXT_CHMOD.with(|f| f.replace(false)) {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected chmod failure (test)",
            ))
        } else {
            result
        };
        if let Err(e) = result {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(WriteConfigAtomicError::Chmod(e));
        }
    }

    // Unlike the chmod injection above, this must skip the *real* rename
    // syscall entirely when firing — POSIX rename is atomic, so a real
    // failure never moves anything; overriding a real (successful) rename's
    // `Ok` after the fact would leave the new content live under
    // `config_path` while this function still reported failure, which is
    // not what a genuine rename failure looks like and would defeat the
    // point of the test using this flag.
    #[cfg(all(test, unix))]
    let rename_result = if FAIL_NEXT_RENAME.with(|f| f.replace(false)) {
        Err(std::io::Error::other("injected rename failure (test)"))
    } else {
        tokio::fs::rename(&tmp_path, config_path).await
    };
    #[cfg(not(all(test, unix)))]
    let rename_result = tokio::fs::rename(&tmp_path, config_path).await;
    if let Err(e) = rename_result {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(WriteConfigAtomicError::Rename(e));
    }

    Ok(())
}

/// Persist a routing-probe verdict for one OpenAI-compatible config.
///
/// Called by `LocalAgentService` after a model load runs the routing probe —
/// not through `SettingsServiceImpl`'s RPC lock, since the probe runs outside
/// any RPC. This is a best-effort, unlocked read-modify-write against the
/// same file `set_open_ai_compat_configs` writes under its own `write_lock`;
/// the two are not coordinated. The realistic worst case is not merely a lost
/// probe verdict (cheap — the next model load re-probes) but a lost *user*
/// edit: if a Settings save and this write interleave, whichever writes last
/// overwrites the file wholesale, discarding the other's change. The window
/// is small (both are sub-millisecond file operations, and a Settings save
/// concurrent with a model load is rare) and nothing here is data the user
/// can't re-enter, so this is accepted rather than worth a cross-service
/// lock — but it is a lost edit, not just a lost cache entry, if it happens.
/// No-ops if the config was deleted or its `base_url` no longer matches what
/// was probed — the verdict would no longer describe the config it was
/// measured against.
pub async fn record_routing_probe_verdict(
    config_path: &std::path::Path,
    id: &str,
    base_url: &str,
    model: &str,
    routing_ok: bool,
) -> anyhow::Result<()> {
    let contents = match tokio::fs::read_to_string(config_path).await {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(anyhow::anyhow!("failed to read daemon config: {e}")),
    };
    let mut config: DaemonConfig = toml::from_str(&contents)
        .map_err(|e| anyhow::anyhow!("failed to parse daemon config: {e}"))?;

    let Some(entry) = config.openai_compat.configs.iter_mut().find(|c| c.id == id) else {
        return Ok(());
    };
    // Matched on `base_url` only, not `entry.model` — `model` here is the
    // served model actually probed (possibly one of several `/models`
    // discovers through this config), which need not equal the config's
    // pinned `model` field at all. The map entry is keyed by `model`, so
    // writing under the probed model's own key is what keeps multiple
    // served models behind one config from colliding.
    if entry.base_url != base_url {
        return Ok(());
    }
    entry.routing_ok.insert(model.to_string(), routing_ok);

    if let Some(parent) = config_path.parent() {
        // Owner-only from birth: this directory holds daemon.toml, which
        // carries third-party API keys.
        crate::create_dir_owner_only(parent)
            .await
            .map_err(|e| anyhow::anyhow!("failed to create config directory: {e}"))?;
    }
    let serialized = toml::to_string_pretty(&config)
        .map_err(|e| anyhow::anyhow!("failed to serialize daemon config: {e}"))?;
    write_config_atomic(config_path, &serialized)
        .await
        .map_err(|e| match e {
            WriteConfigAtomicError::Write(e) => {
                anyhow::anyhow!("failed to write daemon config: {e}")
            }
            #[cfg(unix)]
            WriteConfigAtomicError::Chmod(e) => {
                anyhow::anyhow!("failed to set daemon config permissions: {e}")
            }
            WriteConfigAtomicError::Rename(e) => {
                anyhow::anyhow!("failed to finalize daemon config write: {e}")
            }
        })?;
    Ok(())
}

pub struct SettingsServiceImpl {
    config_path: PathBuf,
    /// Serializes concurrent UpdateCaptureSettings RPCs so read-modify-write
    /// operations on daemon.toml are not interleaved.
    write_lock: Arc<Mutex<()>>,
}

// Test-only fault injection for `write_config_atomic`'s permissions and
// rename steps, shared by both its callers (`write_config` and
// `record_routing_probe_verdict`). When set, the *next* `set_permissions`
// (resp. `rename`) call in `write_config_atomic` on this thread fails as if
// the OS had rejected it, then clears itself. A `thread_local` (not a
// process-global `static`) because `#[tokio::test]` gives each test its own
// current-thread runtime — everything a test awaits, including nested
// `write_config_atomic` calls, runs on that one OS thread — so this stays
// isolated per test even though `cargo test` runs many tests concurrently.
#[cfg(all(test, unix))]
thread_local! {
    static FAIL_NEXT_CHMOD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_NEXT_RENAME: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl SettingsServiceImpl {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            config_path,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Build with the default path `~/.nodespace/daemon.toml`.
    pub fn with_default_path() -> Result<Self, String> {
        let home = std::env::var("HOME")
            .map_err(|_| "$HOME is unset — cannot locate daemon config".to_string())?;
        let path = PathBuf::from(home).join(".nodespace").join("daemon.toml");
        Ok(Self::new(path))
    }

    /// Create (or truncate) the temp file at `path`, already restricted to
    /// owner-only permissions at the instant it's created, via
    /// `OpenOptions::mode(0o600)` (`std::os::unix::fs::OpenOptionsExt`)
    /// rather than creating at default permissions and chmod'ing afterward.
    /// The mode is applied by the `open(2)` syscall itself when it creates
    /// the file, so there is no observable window where the file exists at
    /// default (often world/group-readable) permissions. Mode `0o600` has no
    /// group/other bits to begin with, so a typical umask — which can only
    /// clear bits, never add them — cannot widen it.
    ///
    /// The kernel only applies the mode argument when `open(2)` actually
    /// creates the file: if a stale temp file from an interrupted prior
    /// attempt already exists at `path`, this opens it as-is with whatever
    /// permissions it already had. Callers that need to cover that case
    /// still run an explicit `set_permissions` afterward as a backstop.
    #[cfg(unix)]
    async fn create_tmp_file_owner_only(
        path: &std::path::Path,
    ) -> std::io::Result<tokio::fs::File> {
        // `tokio::fs::OpenOptions` exposes `mode()` as an inherent method
        // mirroring `std::os::unix::fs::OpenOptionsExt` — no trait import
        // needed here, unlike the `std::fs` equivalent.
        tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .await
    }

    async fn read_config(&self) -> Result<DaemonConfig, Status> {
        match tokio::fs::read_to_string(&self.config_path).await {
            Ok(contents) => toml::from_str(&contents)
                .map_err(|e| Status::internal(format!("Failed to parse daemon config: {}", e))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DaemonConfig::default()),
            Err(e) => Err(Status::internal(format!(
                "Failed to read daemon config: {}",
                e
            ))),
        }
    }

    async fn write_config(&self, config: &DaemonConfig) -> Result<(), Status> {
        if let Some(parent) = self.config_path.parent() {
            // Owner-only from birth: this directory holds daemon.toml, which
            // carries third-party API keys.
            crate::create_dir_owner_only(parent).await.map_err(|e| {
                Status::internal(format!("Failed to create config directory: {}", e))
            })?;
        }
        let contents = toml::to_string_pretty(config)
            .map_err(|e| Status::internal(format!("Failed to serialize daemon config: {}", e)))?;

        // Delegate to the shared temp-file-then-atomic-rename helper (also
        // used by `record_routing_probe_verdict`) rather than writing
        // `config_path` in place. This ordering — chmod the not-yet-live
        // temp file, *then* make it live — matters because daemon.toml can
        // hold real third-party API keys (openai_compat.configs): chmod'ing
        // the real path only after writing to it (the old order) meant a
        // chmod failure was reported to the caller as "save failed" while
        // the new content was already durably live and would be read back
        // on the next daemon restart — the caller's retry/error-toast has
        // no way to know that. With the temp file, a failure at any step
        // aborts before the rename, so `config_path` and its permissions
        // are left exactly as they were and the reported failure matches
        // what's actually on disk. See `write_config_atomic` for the full
        // mechanics.
        write_config_atomic(&self.config_path, &contents)
            .await
            .map_err(|e| match e {
                WriteConfigAtomicError::Write(e) => {
                    Status::internal(format!("Failed to write daemon config: {}", e))
                }
                #[cfg(unix)]
                WriteConfigAtomicError::Chmod(e) => {
                    Status::internal(format!("Failed to set daemon config permissions: {}", e))
                }
                WriteConfigAtomicError::Rename(e) => {
                    Status::internal(format!("Failed to finalize daemon config write: {}", e))
                }
            })
    }

    fn capture_to_response(capture: &CaptureConfig) -> CaptureSettingsResponse {
        CaptureSettingsResponse {
            enabled: capture.enabled,
            content: CaptureContentLevel::from(capture.content) as i32,
        }
    }
}

#[tonic::async_trait]
impl GrpcSettingsService for SettingsServiceImpl {
    async fn get_capture_settings(
        &self,
        _request: Request<GetCaptureSettingsRequest>,
    ) -> Result<Response<CaptureSettingsResponse>, Status> {
        let config = self.read_config().await?;
        Ok(Response::new(Self::capture_to_response(&config.capture)))
    }

    async fn update_capture_settings(
        &self,
        request: Request<UpdateCaptureSettingsRequest>,
    ) -> Result<Response<CaptureSettingsResponse>, Status> {
        let req = request.into_inner();
        let _guard = self.write_lock.lock().await;

        let mut config = self.read_config().await?;

        if let Some(enabled) = req.enabled {
            config.capture.enabled = enabled;
        }
        if let Some(content_i32) = req.content {
            let level = CaptureContentLevel::try_from(content_i32)
                .unwrap_or(CaptureContentLevel::MetadataOnly);
            config.capture.content = CaptureContentSetting::from(level);
        }

        self.write_config(&config).await?;
        Ok(Response::new(Self::capture_to_response(&config.capture)))
    }

    async fn list_open_ai_compat_configs(
        &self,
        _request: Request<ListOpenAiCompatConfigsRequest>,
    ) -> Result<Response<ListOpenAiCompatConfigsResponse>, Status> {
        let config = self.read_config().await?;
        Ok(Response::new(ListOpenAiCompatConfigsResponse {
            configs: config
                .openai_compat
                .configs
                .into_iter()
                .map(ProtoOpenAiCompatConfig::from)
                .collect(),
        }))
    }

    async fn set_open_ai_compat_configs(
        &self,
        request: Request<SetOpenAiCompatConfigsRequest>,
    ) -> Result<Response<ListOpenAiCompatConfigsResponse>, Status> {
        let req = request.into_inner();
        let _guard = self.write_lock.lock().await;

        let mut config = self.read_config().await?;
        let previous = std::mem::take(&mut config.openai_compat.configs);
        config.openai_compat.configs = req
            .configs
            .into_iter()
            .map(|proto| {
                let mut c = OpenAiCompatConfig::from(proto);
                // The Settings GUI round-trips every config through this RPC on
                // every save (even one editing an unrelated config), which would
                // otherwise erase every cached probe verdict this id already
                // earned. Carry the whole map forward when `base_url` is
                // unchanged — each entry is already keyed by the served model
                // it was measured against, so entries for models unaffected by
                // whatever the user edited (e.g. the config's pinned `model`
                // field, which only matters for a non-discovery load) stay
                // valid. A `base_url` change means every entry described a
                // different endpoint and none of them apply anymore.
                if let Some(prev) = previous.iter().find(|p| p.id == c.id) {
                    if prev.base_url == c.base_url {
                        c.routing_ok = prev.routing_ok.clone();
                    }
                }
                c
            })
            .collect();

        self.write_config(&config).await?;
        Ok(Response::new(ListOpenAiCompatConfigsResponse {
            configs: config
                .openai_compat
                .configs
                .into_iter()
                .map(ProtoOpenAiCompatConfig::from)
                .collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_impl() -> (SettingsServiceImpl, tempfile::TempDir) {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let config_path = tempdir.path().join("daemon.toml");
        (SettingsServiceImpl::new(config_path), tempdir)
    }

    #[tokio::test]
    async fn list_openai_compat_configs_empty_when_no_file() {
        let (svc, _tempdir) = test_impl();
        let resp = svc
            .list_open_ai_compat_configs(Request::new(ListOpenAiCompatConfigsRequest {}))
            .await
            .expect("list should succeed")
            .into_inner();
        assert!(resp.configs.is_empty());
    }

    #[tokio::test]
    async fn set_then_list_openai_compat_configs_roundtrips() {
        let (svc, _tempdir) = test_impl();
        let config = ProtoOpenAiCompatConfig {
            id: "abc-123".to_string(),
            name: "My Endpoint".to_string(),
            base_url: "https://api.example.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            model: "gpt-4o".to_string(),
        };

        let set_resp = svc
            .set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
                configs: vec![config.clone()],
            }))
            .await
            .expect("set should succeed")
            .into_inner();
        assert_eq!(set_resp.configs.len(), 1);

        let list_resp = svc
            .list_open_ai_compat_configs(Request::new(ListOpenAiCompatConfigsRequest {}))
            .await
            .expect("list should succeed")
            .into_inner();
        assert_eq!(list_resp.configs.len(), 1);
        assert_eq!(list_resp.configs[0].id, "abc-123");
        assert_eq!(list_resp.configs[0].name, "My Endpoint");
        assert_eq!(list_resp.configs[0].base_url, "https://api.example.com/v1");
        assert_eq!(list_resp.configs[0].api_key, "sk-test");
    }

    #[tokio::test]
    async fn set_openai_compat_configs_replaces_full_list() {
        let (svc, _tempdir) = test_impl();
        let first = ProtoOpenAiCompatConfig {
            id: "a".to_string(),
            name: "A".to_string(),
            base_url: "https://a.example.com".to_string(),
            api_key: String::new(),
            model: "model-a".to_string(),
        };
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![first],
        }))
        .await
        .expect("first set should succeed");

        let second = ProtoOpenAiCompatConfig {
            id: "b".to_string(),
            name: "B".to_string(),
            base_url: "https://b.example.com".to_string(),
            api_key: String::new(),
            model: "model-b".to_string(),
        };
        let resp = svc
            .set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
                configs: vec![second],
            }))
            .await
            .expect("second set should succeed")
            .into_inner();

        assert_eq!(resp.configs.len(), 1);
        assert_eq!(resp.configs[0].id, "b");
    }

    #[tokio::test]
    async fn find_openai_compat_config_by_id() {
        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        let config = ProtoOpenAiCompatConfig {
            id: "target".to_string(),
            name: "Target".to_string(),
            base_url: "https://target.example.com".to_string(),
            api_key: "key".to_string(),
            model: "target-model".to_string(),
        };
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![config],
        }))
        .await
        .expect("set should succeed");

        let found = find_openai_compat_config(&config_path, "target")
            .await
            .expect("read should succeed");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "Target");

        let missing = find_openai_compat_config(&config_path, "nonexistent")
            .await
            .expect("read should succeed");
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn find_openai_compat_config_returns_none_when_file_missing() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let config_path = tempdir.path().join("daemon.toml");
        let found = find_openai_compat_config(&config_path, "anything")
            .await
            .expect("missing file should not error");
        assert!(found.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_config_restricts_daemon_toml_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![ProtoOpenAiCompatConfig {
                id: "a".to_string(),
                name: "A".to_string(),
                base_url: "https://a.example.com".to_string(),
                api_key: "sk-secret".to_string(),
                model: "gpt-4o".to_string(),
            }],
        }))
        .await
        .expect("set should succeed");

        let metadata = tokio::fs::metadata(&config_path)
            .await
            .expect("daemon.toml should exist after write");
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "daemon.toml should be owner-read/write only, got {:o}",
            mode
        );
    }

    /// Regression test for the temp-file exposure window: the pre-fix
    /// `write_config` created the temp file via `tokio::fs::write` (default,
    /// typically world/group-readable permissions) and only restricted it to
    /// `0600` in a *separate* `set_permissions` call afterward, leaving a
    /// brief window where the fully-written new content — which can include
    /// real third-party API keys — sat on disk at default permissions.
    /// `create_tmp_file_owner_only` closes that window by passing the
    /// restrictive mode to the `open(2)` syscall that creates the file, so
    /// there is no separate step for a window to exist between: the mode is
    /// already `0600` on the file handle `open` returns, before a single
    /// byte of content has been written. This test proves that directly by
    /// inspecting the freshly created (still-empty) file's permissions.
    #[cfg(unix)]
    #[tokio::test]
    async fn tmp_file_is_owner_only_at_the_instant_of_creation() {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let path = tempdir.path().join("daemon.toml.tmp-test");

        let file = SettingsServiceImpl::create_tmp_file_owner_only(&path)
            .await
            .expect("create should succeed");

        // Inspect permissions on the just-created file handle before any
        // content is written and before any `set_permissions` backstop runs.
        let metadata = file.metadata().await.expect("metadata should succeed");
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "temp file must be owner-only from the instant open(2) creates it, got {:o}",
            mode
        );
    }

    /// Regression test for the bug fixed here: a `set_permissions` failure
    /// on the write path used to happen *after* the new content was already
    /// durably written to `config_path`, so the caller was told the save
    /// failed while the new config was in fact already live. `write_config`
    /// now writes and chmods a temp file first and only renames it over
    /// `config_path` once both succeed, so a chmod failure must leave the
    /// previously-saved config in place — not the new one — matching the
    /// error the caller was given.
    #[cfg(unix)]
    #[tokio::test]
    async fn chmod_failure_after_write_leaves_previous_config_intact_and_reports_the_error() {
        use std::os::unix::fs::PermissionsExt;

        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");

        // Establish a known-good baseline config on disk.
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config(
                "original",
                "https://original.example.com",
                "m",
            )],
        }))
        .await
        .expect("baseline set should succeed");

        // Inject a chmod failure for the *next* write only.
        FAIL_NEXT_CHMOD.with(|f| f.set(true));

        let failed = svc
            .set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
                configs: vec![probe_config(
                    "replacement",
                    "https://replacement.example.com",
                    "m",
                )],
            }))
            .await;

        // The caller must be told the save failed...
        assert!(
            failed.is_err(),
            "a chmod failure on the write path must surface as an error"
        );
        // ...and the flag must have been consumed (proves the injected
        // failure is what actually fired, not some other error).
        assert!(
            !FAIL_NEXT_CHMOD.with(|f| f.get()),
            "injected failure should have been consumed by the write attempt"
        );

        // ...and, unlike the pre-fix behavior, the config on disk must still
        // be the ORIGINAL one — the replacement must never have gone live.
        let found = find_openai_compat_config(&config_path, "replacement")
            .await
            .expect("read should succeed");
        assert!(
            found.is_none(),
            "the replacement config must not be live after a reported save failure"
        );
        let original = find_openai_compat_config(&config_path, "original")
            .await
            .expect("read should succeed");
        assert!(
            original.is_some(),
            "the previously-saved config must survive a failed save"
        );

        // No temp file left behind.
        let mut entries = tokio::fs::read_dir(tempdir.path())
            .await
            .expect("read_dir should succeed");
        let mut leftover_tmp_files = Vec::new();
        while let Some(entry) = entries.next_entry().await.expect("next_entry") {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".tmp-") {
                leftover_tmp_files.push(name);
            }
        }
        assert!(
            leftover_tmp_files.is_empty(),
            "a failed write must not leave a temp file behind, found {:?}",
            leftover_tmp_files
        );

        // Permissions on the surviving file are untouched by the failed
        // attempt.
        let metadata = tokio::fs::metadata(&config_path)
            .await
            .expect("daemon.toml should still exist");
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "surviving daemon.toml should still be owner-only, got {:o}",
            mode
        );

        // A subsequent save with no injected failure succeeds normally,
        // proving the temp-file machinery itself still works.
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config(
                "replacement",
                "https://replacement.example.com",
                "m",
            )],
        }))
        .await
        .expect("save without injected failure should succeed");
        let found = find_openai_compat_config(&config_path, "replacement")
            .await
            .expect("read should succeed");
        assert!(found.is_some(), "a genuine save must still take effect");
    }

    /// Same guarantee as the chmod-failure regression test above, but for
    /// the final `rename` step: a failure finalizing the write (e.g. the
    /// destination became briefly unwritable, or a filesystem-level rename
    /// error) must also leave the previously-saved config in place, clean up
    /// the temp file, and report an error distinct from a plain write
    /// failure.
    #[cfg(unix)]
    #[tokio::test]
    async fn rename_failure_after_chmod_leaves_previous_config_intact_and_reports_the_error() {
        use std::os::unix::fs::PermissionsExt;

        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");

        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config(
                "original",
                "https://original.example.com",
                "m",
            )],
        }))
        .await
        .expect("baseline set should succeed");

        FAIL_NEXT_RENAME.with(|f| f.set(true));

        let failed = svc
            .set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
                configs: vec![probe_config(
                    "replacement",
                    "https://replacement.example.com",
                    "m",
                )],
            }))
            .await;

        assert!(
            failed.is_err(),
            "a rename failure finalizing the write must surface as an error"
        );
        let err_message = failed.unwrap_err().message().to_string();
        assert!(
            err_message.contains("finalize"),
            "rename failure should be reported distinctly from a plain write failure, got: {}",
            err_message
        );
        assert!(
            !FAIL_NEXT_RENAME.with(|f| f.get()),
            "injected failure should have been consumed by the write attempt"
        );

        let found = find_openai_compat_config(&config_path, "replacement")
            .await
            .expect("read should succeed");
        assert!(
            found.is_none(),
            "the replacement config must not be live after a reported save failure"
        );
        let original = find_openai_compat_config(&config_path, "original")
            .await
            .expect("read should succeed");
        assert!(
            original.is_some(),
            "the previously-saved config must survive a failed save"
        );

        let mut entries = tokio::fs::read_dir(tempdir.path())
            .await
            .expect("read_dir should succeed");
        let mut leftover_tmp_files = Vec::new();
        while let Some(entry) = entries.next_entry().await.expect("next_entry") {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".tmp-") {
                leftover_tmp_files.push(name);
            }
        }
        assert!(
            leftover_tmp_files.is_empty(),
            "a failed rename must not leave a temp file behind, found {:?}",
            leftover_tmp_files
        );

        let metadata = tokio::fs::metadata(&config_path)
            .await
            .expect("daemon.toml should still exist");
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "surviving daemon.toml should still be owner-only, got {:o}",
            mode
        );
    }

    #[tokio::test]
    async fn read_mcp_settings_defaults_to_disabled_when_file_missing() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let config_path = tempdir.path().join("daemon.toml");
        let settings = read_mcp_settings(&config_path)
            .await
            .expect("missing file must not error");
        assert!(!settings.enabled);
    }

    #[tokio::test]
    async fn set_mcp_enabled_then_read_roundtrips() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let config_path = tempdir.path().join("daemon.toml");

        set_mcp_enabled(&config_path, true)
            .await
            .expect("enable should succeed");
        let settings = read_mcp_settings(&config_path)
            .await
            .expect("read should succeed");
        assert!(settings.enabled);

        set_mcp_enabled(&config_path, false)
            .await
            .expect("disable should succeed");
        let settings = read_mcp_settings(&config_path)
            .await
            .expect("read should succeed");
        assert!(!settings.enabled);
    }

    #[tokio::test]
    async fn set_mcp_enabled_creates_the_config_directory_when_missing() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let config_path = tempdir.path().join("nested").join("daemon.toml");

        set_mcp_enabled(&config_path, true)
            .await
            .expect("enable should succeed even when the parent dir doesn't exist yet");
        assert!(config_path.exists());
    }

    /// The `[mcp]` flag must not clobber unrelated sections -- a real
    /// daemon.toml can already carry `openai_compat.configs` (with API keys)
    /// when a user first runs `nodespace mcp install`.
    #[tokio::test]
    async fn set_mcp_enabled_preserves_other_sections() {
        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config("a", "https://a.example.com", "gpt-4o")],
        }))
        .await
        .expect("baseline openai_compat set should succeed");

        set_mcp_enabled(&config_path, true)
            .await
            .expect("enable should succeed");

        let found = find_openai_compat_config(&config_path, "a")
            .await
            .expect("read should succeed");
        assert!(
            found.is_some(),
            "enabling MCP must not erase existing openai_compat config"
        );
        let settings = read_mcp_settings(&config_path)
            .await
            .expect("read should succeed");
        assert!(settings.enabled);
    }

    /// Symmetric to the above: writing `openai_compat` config (e.g. a later
    /// Settings-GUI save) must not silently flip `mcp.enabled` back off.
    #[tokio::test]
    async fn set_open_ai_compat_configs_preserves_mcp_enabled() {
        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        set_mcp_enabled(&config_path, true)
            .await
            .expect("enable should succeed");

        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config("a", "https://a.example.com", "gpt-4o")],
        }))
        .await
        .expect("openai_compat set should succeed");

        let settings = read_mcp_settings(&config_path)
            .await
            .expect("read should succeed");
        assert!(
            settings.enabled,
            "an unrelated settings write must not disable MCP"
        );
    }

    fn probe_config(id: &str, base_url: &str, model: &str) -> ProtoOpenAiCompatConfig {
        ProtoOpenAiCompatConfig {
            id: id.to_string(),
            name: "Test Endpoint".to_string(),
            base_url: base_url.to_string(),
            api_key: String::new(),
            model: model.to_string(),
        }
    }

    #[tokio::test]
    async fn record_routing_probe_verdict_persists_and_is_readable() {
        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config("a", "http://localhost:11434/v1", "mistral:7b")],
        }))
        .await
        .expect("set should succeed");

        record_routing_probe_verdict(
            &config_path,
            "a",
            "http://localhost:11434/v1",
            "mistral:7b",
            false,
        )
        .await
        .expect("record should succeed");

        let found = find_openai_compat_config(&config_path, "a")
            .await
            .expect("read should succeed")
            .expect("config exists");
        assert_eq!(found.routing_ok.get("mistral:7b"), Some(&false));
    }

    /// Regression test for a collision introduced (and fixed) while sharing
    /// `write_config_atomic` between `write_config` and
    /// `record_routing_probe_verdict`: in production both resolve to the
    /// same `~/.nodespace/daemon.toml`, and `record_routing_probe_verdict`
    /// is deliberately unlocked (see its doc comment), so the two can run
    /// concurrently on the daemon's real multi-threaded runtime. Keying the
    /// temp filename on `std::process::id()` alone would let concurrent
    /// calls target the *same* temp path — `create_tmp_file_owner_only`
    /// opens with `truncate(true)`, not `create_new(true)`, so a second
    /// opener silently reuses/truncates the first's in-flight file, which
    /// can surface a spurious error on an unrelated save. Firing many
    /// concurrent `write_config_atomic` calls against the same path proves
    /// every call either succeeds cleanly or fails for a real reason, never
    /// because it collided with a sibling call's temp file, and that the
    /// file that lands is always one complete write — never truncated or
    /// mixed content from two writers, and no temp file is left behind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_write_config_atomic_calls_do_not_collide_on_tmp_path() {
        let tempdir = tempfile::TempDir::new().expect("tempdir");
        let config_path = tempdir.path().join("daemon.toml");

        let mut handles = Vec::new();
        for i in 0..20 {
            let path = config_path.clone();
            let contents = format!("value = {i}\n");
            handles.push(tokio::spawn(async move {
                write_config_atomic(&path, &contents).await
            }));
        }
        for handle in handles {
            handle
                .await
                .expect("task should not panic")
                .map_err(|_| ())
                .expect(
                    "a concurrent write_config_atomic call must not fail from colliding with \
                     a sibling call's temp file",
                );
        }

        // The file must hold exactly one complete write, never a mix of two.
        let final_contents = tokio::fs::read_to_string(&config_path)
            .await
            .expect("final file should exist");
        assert!(
            (0..20).any(|i| final_contents == format!("value = {i}\n")),
            "final content must be exactly one complete write, got: {:?}",
            final_contents
        );

        // No leftover temp files once every concurrent writer has finished.
        let mut entries = tokio::fs::read_dir(tempdir.path())
            .await
            .expect("read_dir should succeed");
        let mut leftover_tmp_files = Vec::new();
        while let Some(entry) = entries.next_entry().await.expect("next_entry") {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".tmp-") {
                leftover_tmp_files.push(name);
            }
        }
        assert!(
            leftover_tmp_files.is_empty(),
            "no temp files should remain after concurrent writers finish, found {:?}",
            leftover_tmp_files
        );
    }

    /// Regression test for the crash-safety gap this issue fixes:
    /// `record_routing_probe_verdict` used to write `daemon.toml` in place
    /// via a plain `tokio::fs::write`, so a crash or power loss mid-write
    /// could leave the file truncated/corrupt. It now shares
    /// `write_config_atomic` with `write_config`, so a failure finalizing
    /// the write (the atomic rename) must leave the previously-saved config
    /// completely intact — not truncated, not partially updated — exactly
    /// like the equivalent guarantee already proven for `write_config` in
    /// `rename_failure_after_chmod_leaves_previous_config_intact_and_reports_the_error`.
    #[cfg(unix)]
    #[tokio::test]
    async fn record_routing_probe_verdict_rename_failure_leaves_previous_config_intact() {
        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config("a", "http://localhost:11434/v1", "mistral:7b")],
        }))
        .await
        .expect("set should succeed");

        FAIL_NEXT_RENAME.with(|f| f.set(true));

        let failed = record_routing_probe_verdict(
            &config_path,
            "a",
            "http://localhost:11434/v1",
            "mistral:7b",
            false,
        )
        .await;

        assert!(
            failed.is_err(),
            "a rename failure finalizing the write must surface as an error"
        );
        let err_message = failed.unwrap_err().to_string();
        assert!(
            err_message.contains("finalize"),
            "rename failure should be reported distinctly from a plain write failure, got: {}",
            err_message
        );
        assert!(
            !FAIL_NEXT_RENAME.with(|f| f.get()),
            "injected failure should have been consumed by the write attempt"
        );

        // The verdict must NOT have been recorded — the previous config
        // (with no verdict yet) must survive untouched.
        let found = find_openai_compat_config(&config_path, "a")
            .await
            .expect("read should succeed")
            .expect("config exists");
        assert!(
            found.routing_ok.is_empty(),
            "a failed finalize must not leave a partial or new verdict live"
        );

        // No temp file left behind.
        let mut entries = tokio::fs::read_dir(tempdir.path())
            .await
            .expect("read_dir should succeed");
        let mut leftover_tmp_files = Vec::new();
        while let Some(entry) = entries.next_entry().await.expect("next_entry") {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains(".tmp-") {
                leftover_tmp_files.push(name);
            }
        }
        assert!(
            leftover_tmp_files.is_empty(),
            "a failed rename must not leave a temp file behind, found {:?}",
            leftover_tmp_files
        );

        // A subsequent record with no injected failure succeeds normally,
        // proving the atomic-write machinery itself still works.
        record_routing_probe_verdict(
            &config_path,
            "a",
            "http://localhost:11434/v1",
            "mistral:7b",
            false,
        )
        .await
        .expect("record without injected failure should succeed");
        let found = find_openai_compat_config(&config_path, "a")
            .await
            .expect("read should succeed")
            .expect("config exists");
        assert_eq!(found.routing_ok.get("mistral:7b"), Some(&false));
    }

    #[tokio::test]
    async fn record_routing_probe_verdict_noops_when_config_deleted() {
        let (_svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        // No config file at all yet.
        record_routing_probe_verdict(&config_path, "ghost", "http://x", "m", false)
            .await
            .expect("noop on missing file must not error");
    }

    #[tokio::test]
    async fn record_routing_probe_verdict_noops_when_base_url_changed() {
        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config("a", "http://localhost:11434/v1", "mistral:7b")],
        }))
        .await
        .expect("set should succeed");

        // Probe result for an endpoint this config id no longer points at —
        // must not be written, or a later load would trust a verdict about a
        // server this config no longer names.
        record_routing_probe_verdict(&config_path, "a", "http://other-host:11434/v1", "x", true)
            .await
            .expect("stale-target write should not error, just noop");

        let found = find_openai_compat_config(&config_path, "a")
            .await
            .expect("read should succeed")
            .expect("config exists");
        assert!(
            found.routing_ok.is_empty(),
            "a verdict for a different endpoint must not attach to this config"
        );
    }

    /// The bug this test guards against (found in code review on #1830): a
    /// single OpenAI-compat config can discover several served models
    /// (`openai-compat:<uuid>:<model>` — see `parse_openai_compat_id`), and a
    /// scalar verdict field would let one model's probe result leak onto
    /// another's load. `mistral:7b` measuring suppressed must never disable
    /// injection for `llama3.1:8b`, discovered through the same config and
    /// never itself probed as failing.
    #[tokio::test]
    async fn verdicts_for_different_models_on_the_same_config_do_not_collide() {
        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config("a", "http://localhost:11434/v1", "mistral:7b")],
        }))
        .await
        .expect("set should succeed");

        record_routing_probe_verdict(
            &config_path,
            "a",
            "http://localhost:11434/v1",
            "mistral:7b",
            false,
        )
        .await
        .expect("record should succeed");
        record_routing_probe_verdict(
            &config_path,
            "a",
            "http://localhost:11434/v1",
            "llama3.1:8b",
            true,
        )
        .await
        .expect("record should succeed");

        let found = find_openai_compat_config(&config_path, "a")
            .await
            .expect("read should succeed")
            .expect("config exists");
        assert_eq!(
            found.routing_ok.get("mistral:7b"),
            Some(&false),
            "mistral:7b's own verdict must be preserved"
        );
        assert_eq!(
            found.routing_ok.get("llama3.1:8b"),
            Some(&true),
            "llama3.1:8b must carry its own verdict, not mistral:7b's"
        );
    }

    #[tokio::test]
    async fn set_open_ai_compat_configs_carries_forward_verdicts_when_base_url_unchanged() {
        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config("a", "http://localhost:11434/v1", "mistral:7b")],
        }))
        .await
        .expect("set should succeed");
        record_routing_probe_verdict(
            &config_path,
            "a",
            "http://localhost:11434/v1",
            "mistral:7b",
            false,
        )
        .await
        .expect("record should succeed");
        record_routing_probe_verdict(
            &config_path,
            "a",
            "http://localhost:11434/v1",
            "llama3.1:8b",
            true,
        )
        .await
        .expect("record should succeed");

        // Settings GUI saves again — e.g. the user renamed the config, or
        // changed the pinned `model` field (which only matters for a
        // non-discovery load and does not invalidate verdicts already
        // measured for OTHER models discovered through the same endpoint).
        let mut renamed = probe_config("a", "http://localhost:11434/v1", "mistral:7b");
        renamed.name = "Renamed".to_string();
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![renamed],
        }))
        .await
        .expect("set should succeed");

        let found = find_openai_compat_config(&config_path, "a")
            .await
            .expect("read should succeed")
            .expect("config exists");
        assert_eq!(
            found.routing_ok.get("mistral:7b"),
            Some(&false),
            "an unrelated field edit must not erase a cached probe verdict"
        );
        assert_eq!(
            found.routing_ok.get("llama3.1:8b"),
            Some(&true),
            "verdicts for every model behind this base_url carry forward, not just the pinned one"
        );
    }

    #[tokio::test]
    async fn set_open_ai_compat_configs_drops_verdicts_when_base_url_changes() {
        let (svc, tempdir) = test_impl();
        let config_path = tempdir.path().join("daemon.toml");
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config("a", "http://localhost:11434/v1", "mistral:7b")],
        }))
        .await
        .expect("set should succeed");
        record_routing_probe_verdict(
            &config_path,
            "a",
            "http://localhost:11434/v1",
            "mistral:7b",
            false,
        )
        .await
        .expect("record should succeed");

        // The user points the same config id at a different endpoint
        // entirely — every verdict measured against the old endpoint is
        // meaningless for the new one.
        svc.set_open_ai_compat_configs(Request::new(SetOpenAiCompatConfigsRequest {
            configs: vec![probe_config(
                "a",
                "http://other-host:11434/v1",
                "mistral:7b",
            )],
        }))
        .await
        .expect("set should succeed");

        let found = find_openai_compat_config(&config_path, "a")
            .await
            .expect("read should succeed")
            .expect("config exists");
        assert!(
            found.routing_ok.is_empty(),
            "verdicts measured against the old endpoint must not carry onto the new one"
        );
    }
}
