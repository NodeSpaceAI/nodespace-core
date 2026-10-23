//! Onboarding wizard Tauri commands.
//!
//! Handles first-launch setup: PATH configuration and NodeSpace skill
//! installation. Completion state is persisted to `~/.nodespace/config.json`.
//! Skill installation state is tracked separately in `~/.nodespace/setup.json`.

use super::nodes::status_message;
use crate::atomic_file;
use crate::services::GrpcClient;
use crate::skill_setup::{self, SkillSetupResult};
use nodespace_proto::nodespace::{Empty, NodeData, SetLocalPersonIdentityRequest};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;
use tonic::Request;

/// Current onboarding status returned to the frontend on startup.
///
/// Deliberately cheap: a config read, a few rc-file reads and one
/// `nodespace --version` probe. The app shell invokes this on EVERY launch
/// just to read `completed`, so nothing slower belongs here — agent
/// detection, which shells out to the skill installer, is its own command
/// ([`detect_agents`]) that only the callers actually needing it pay for.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingStatus {
    pub completed: bool,
    pub path_configured: bool,
    pub skill_configured: bool,
    /// `nodespace` resolves on the user's shell PATH, by whatever means
    /// (the Homebrew cask, the installer script, our rc line).
    pub path_already_configured: bool,
    /// A known rc file carries NodeSpace's own PATH line, i.e. there is
    /// something for "Remove from PATH" to remove.
    pub path_export_present: bool,
}

/// Which agents the skill installer would target if run right now, as agent
/// ids (`claude-code`, `codex`, `antigravity`, ...).
///
/// Sourced from the installer's own `AGENTS` config rather than a hardcoded
/// home-directory probe here, so this can never name a different set than the
/// install itself targets.
///
/// Separate from [`OnboardingStatus`] because answering it costs a subprocess
/// (see [`skill_setup::detect_agents`]): the onboarding wizard and the
/// Settings → Integrations panel need it, the app shell's per-launch
/// `completed` check does not, and making that check pay for it put a Node
/// runtime start on the startup critical path.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedAgents {
    pub agents: Vec<String>,
    /// True when detection could not run at all (the installer wouldn't
    /// resolve, no runtime on `$PATH`, a non-zero exit). Distinct from an
    /// empty `agents` list, which means detection ran and genuinely found
    /// nothing — the caller must not present a failure as "you have no
    /// coding agents installed".
    pub detection_failed: bool,
}

/// Shape of `~/.nodespace/config.json` on disk.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct NodespaceConfig {
    #[serde(default)]
    onboarding_completed: bool,
    #[serde(default)]
    integrations: IntegrationsConfig,
    /// Set once the user has explicitly saved or skipped the
    /// identity backfill prompt (the nudge shown to an already-onboarded
    /// install whose seeded local person is still blank), so it surfaces at
    /// most once rather than nagging on every launch. The identity itself
    /// stays editable any time from Settings regardless of this flag.
    #[serde(default)]
    identity_prompt_dismissed: bool,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct IntegrationsConfig {
    path_configured: bool,
    skill_configured: bool,
}

// ── helpers ──────────────────────────────────────────────────────────────────

const CONFIG_FILE: &str = "config.json";

/// Serializes every read-modify-write of `config.json` (see [`update_config`]).
/// Settings → Integrations gates its PATH and skill actions behind
/// independent loading flags, so two commands updating different fields can
/// genuinely overlap; without this, both read the file before either write
/// lands and the later rename silently drops the earlier field. It also keeps
/// two writers off `atomic_file::write_json`'s shared temp path. This process
/// is the file's only writer, so an in-process lock suffices — the same
/// pattern as `window_state.rs`'s `SAVE_LOCK`.
static CONFIG_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn nodespace_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".nodespace"))
}

async fn read_config() -> Result<NodespaceConfig, String> {
    read_config_at(&nodespace_dir()?).await
}

async fn read_config_at(dir: &Path) -> Result<NodespaceConfig, String> {
    let raw = match tokio::fs::read_to_string(dir.join(CONFIG_FILE)).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(NodespaceConfig::default()),
        Err(e) => return Err(format!("Failed to read config: {e}")),
    };
    serde_json::from_str(&raw).map_err(|e| format!("Failed to parse config: {e}"))
}

/// Apply `mutate` to the persisted config under [`CONFIG_LOCK`] and write the
/// result back atomically (temp file + rename), so neither a concurrent
/// update nor a crash mid-write can lose or corrupt a field. A read or parse
/// failure aborts rather than writing defaults over the real file.
async fn update_config(mutate: impl FnOnce(&mut NodespaceConfig)) -> Result<(), String> {
    update_config_at(&nodespace_dir()?, mutate).await
}

async fn update_config_at(
    dir: &Path,
    mutate: impl FnOnce(&mut NodespaceConfig),
) -> Result<(), String> {
    let _guard = CONFIG_LOCK.lock().await;
    let mut cfg = read_config_at(dir).await.map_err(|e| {
        format!(
            "refusing to update {CONFIG_FILE}: it could not be read ({e}) — \
             writing now would reset every other persisted field"
        )
    })?;
    mutate(&mut cfg);
    atomic_file::write_json(dir, CONFIG_FILE, &cfg).await
}

/// The comment line that opens the block NodeSpace writes into an rc file.
const BLOCK_COMMENT: &str = "# NodeSpace CLI";
/// The one line that follows [`BLOCK_COMMENT`], per shell family.
const POSIX_EXPORT: &str = "export PATH=\"$HOME/.nodespace/bin:$PATH\"";
const FISH_EXPORT: &str = "fish_add_path $HOME/.nodespace/bin";

const POSIX_EXPORT_LINE: &str = "\n# NodeSpace CLI\nexport PATH=\"$HOME/.nodespace/bin:$PATH\"\n";
const FISH_EXPORT_LINE: &str = "\n# NodeSpace CLI\nfish_add_path $HOME/.nodespace/bin\n";

/// Byte ranges of every block NodeSpace wrote into `content`: the
/// [`BLOCK_COMMENT`] line immediately followed by one of the exact export
/// lines (line endings and trailing blanks ignored), plus the single blank
/// separator line written before it. Anything else mentioning the path (a
/// commented-out line, the user's own export, a line inside an `if`) is not
/// ours and is never matched.
fn nodespace_blocks(content: &str) -> Vec<std::ops::Range<usize>> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for raw in content.split_inclusive('\n') {
        lines.push((offset, raw.len(), raw.trim_end()));
        offset += raw.len();
    }
    let mut blocks = Vec::new();
    let mut i = 0;
    while i + 1 < lines.len() {
        let (start, _, text) = lines[i];
        let (next_start, next_len, next) = lines[i + 1];
        if text == BLOCK_COMMENT && (next == POSIX_EXPORT || next == FISH_EXPORT) {
            let mut from = start;
            if i > 0
                && lines[i - 1].2.is_empty()
                && !blocks
                    .iter()
                    .any(|b: &std::ops::Range<usize>| b.end > lines[i - 1].0)
            {
                from = lines[i - 1].0;
            }
            blocks.push(from..next_start + next_len);
            i += 2;
        } else {
            i += 1;
        }
    }
    blocks
}

/// True when the file has at least one newline and every one is `\r\n`; a
/// mixed or LF file gets LF.
fn all_newlines_are_crlf(content: &str) -> bool {
    let newlines = content.matches('\n').count();
    newlines > 0 && newlines == content.matches("\r\n").count()
}

/// Resolve a symlinked dotfile to the file it points at, so an edit lands on
/// the real target instead of replacing the link with a regular file.
fn resolve_symlink(path: &Path) -> std::path::PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    // A dangling link: follow it one hop at a time, without requiring the end
    // to exist.
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(&current) {
                Ok(target) => {
                    current = current
                        .parent()
                        .map_or(target.clone(), |parent| parent.join(&target));
                }
                Err(_) => break,
            },
            _ => break,
        }
    }
    current
}

/// Replace `path`'s content atomically: write a temp file in the same
/// directory, flush it, copy the original's permissions, and rename it over
/// the target (following a symlink). A failure at any step leaves the
/// original file untouched and removes the temp file.
fn write_rc_atomically(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let target = resolve_symlink(path);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rc".to_string());
    let tmp = target.with_file_name(format!(".{name}.nodespace-tmp-{}", std::process::id()));
    let staged = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(content)?;
        if let Ok(meta) = std::fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.sync_all()
    })();
    if let Err(e) = staged.and_then(|()| std::fs::rename(&tmp, &target)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    #[cfg(unix)]
    if let Some(dir) = target.parent() {
        // Best effort: the content is already in place.
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

async fn write_rc_file(path: &Path, content: String) -> Result<(), String> {
    let owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || write_rc_atomically(&owned, content.as_bytes()))
        .await
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// The user's login shell, as far as PATH setup needs to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginShell {
    Zsh,
    Bash,
    Fish,
    /// Anything else (sh, dash, ksh, unset): the POSIX `~/.profile`.
    Other,
}

impl LoginShell {
    fn from_env(shell: Option<&str>) -> Self {
        let name = shell
            .and_then(|s| Path::new(s).file_name())
            .and_then(|n| n.to_str());
        match name {
            Some("zsh") => Self::Zsh,
            Some("bash") => Self::Bash,
            Some("fish") => Self::Fish,
            _ => Self::Other,
        }
    }

    fn export_line(self) -> &'static str {
        match self {
            Self::Fish => FISH_EXPORT_LINE,
            _ => POSIX_EXPORT_LINE,
        }
    }
}

/// Every file NodeSpace may have written its PATH line to: what the app's
/// wizard writes and what the installer script writes. Detection, writing and
/// removal all use this one set, so a second run never duplicates the line and
/// an uninstall leaves none behind.
fn known_rc_files(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".zshrc"),
        home.join(".bash_profile"),
        home.join(".bashrc"),
        home.join(".profile"),
        home.join(".config").join("fish").join("config.fish"),
    ]
}

/// The one file the line is written to for `shell` when no known file has it.
fn target_rc_file(home: &Path, shell: LoginShell) -> PathBuf {
    match shell {
        LoginShell::Zsh => home.join(".zshrc"),
        LoginShell::Fish => home.join(".config").join("fish").join("config.fish"),
        LoginShell::Bash => bash_target(home, cfg!(target_os = "macos")),
        LoginShell::Other => home.join(".profile"),
    }
}

/// Bash reads `~/.bash_profile` in a login shell and `~/.bashrc` otherwise.
/// macOS terminals always open login shells, which never read `~/.bashrc`, so
/// it is always `~/.bash_profile` there; elsewhere `~/.bashrc`, unless the
/// user already keeps a `~/.bash_profile`.
fn bash_target(home: &Path, macos: bool) -> PathBuf {
    let bash_profile = home.join(".bash_profile");
    if macos || bash_profile.exists() {
        bash_profile
    } else {
        home.join(".bashrc")
    }
}

/// What happened to one rc file.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PathFileOutcome {
    /// The line was appended to an existing file.
    Written,
    /// The file did not exist and was created with the line.
    Created,
    /// The line was already there; nothing changed.
    Unchanged,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PathFileReport {
    pub path: String,
    pub outcome: PathFileOutcome,
}

/// What [`configure_path`] did, so the wizard can report it instead of
/// claiming success.
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PathConfigResult {
    /// `nodespace` already resolved on the PATH, so nothing was touched.
    pub already_on_path: bool,
    pub files: Vec<PathFileReport>,
    /// A new PATH reaches new shells only; true when a terminal or agent
    /// session that is already open still cannot find `nodespace`.
    pub restart_required: bool,
}

/// Return true if the PATH export line is already present in the given file.
async fn file_contains_nodespace_path(path: &Path) -> bool {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => !nodespace_blocks(&content).is_empty(),
        Err(_) => false,
    }
}

/// True when any known rc file carries the NodeSpace PATH line.
async fn any_rc_file_has_path(home: &Path) -> bool {
    for file in known_rc_files(home) {
        if file_contains_nodespace_path(&file).await {
            return true;
        }
    }
    false
}

/// Append `line` to `path`, creating the file (and its directory) when it does
/// not exist. A file that already has the PATH line is left alone.
async fn append_path_to_file(path: &Path, line: &str) -> Result<PathFileOutcome, String> {
    let existing = match tokio::fs::read_to_string(path).await {
        Ok(content) => Some(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    if existing
        .as_deref()
        .is_some_and(|content| !nodespace_blocks(content).is_empty())
    {
        return Ok(PathFileOutcome::Unchanged);
    }
    let created = existing.is_none();
    let mut content = existing.unwrap_or_default();
    if created {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
        }
        content.push_str(line.trim_start_matches('\n'));
    } else if all_newlines_are_crlf(&content) {
        content.push_str(&line.replace('\n', "\r\n"));
    } else {
        content.push_str(line);
    }
    write_rc_file(path, content).await?;
    Ok(if created {
        PathFileOutcome::Created
    } else {
        PathFileOutcome::Written
    })
}

/// Put the PATH line where `shell` will read it, unless a known rc file
/// already has it (never a second export). Fails, naming the file, rather than
/// reporting a write that did not happen.
async fn apply_path_config(home: &Path, shell: LoginShell) -> Result<Vec<PathFileReport>, String> {
    let mut present = Vec::new();
    for file in known_rc_files(home) {
        if file_contains_nodespace_path(&file).await {
            present.push(PathFileReport {
                path: file.display().to_string(),
                outcome: PathFileOutcome::Unchanged,
            });
        }
    }
    if !present.is_empty() {
        return Ok(present);
    }
    let target = target_rc_file(home, shell);
    let outcome = append_path_to_file(&target, shell.export_line()).await?;
    Ok(vec![PathFileReport {
        path: target.display().to_string(),
        outcome,
    }])
}

/// The decision behind [`configure_path`], with the machine-dependent inputs
/// (home, login shell, whether the CLI already resolves) passed in.
async fn configure_path_at(
    home: &Path,
    shell: LoginShell,
    cli_on_path: bool,
) -> Result<PathConfigResult, String> {
    if cli_on_path {
        return Ok(PathConfigResult {
            already_on_path: true,
            files: Vec::new(),
            restart_required: false,
        });
    }
    let files = apply_path_config(home, shell).await?;
    Ok(PathConfigResult {
        already_on_path: false,
        files,
        restart_required: true,
    })
}

/// Remove the blocks NodeSpace wrote from a shell file (see
/// [`nodespace_blocks`]); every other line, including its line endings, is
/// kept byte for byte. Returns `true` if modified.
async fn remove_path_from_file(path: &Path) -> Result<bool, String> {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    let blocks = nodespace_blocks(&content);
    if blocks.is_empty() {
        return Ok(false);
    }
    let mut result = String::with_capacity(content.len());
    let mut cursor = 0;
    for block in blocks {
        result.push_str(&content[cursor..block.start]);
        cursor = block.end;
    }
    result.push_str(&content[cursor..]);
    write_rc_file(path, result).await?;
    Ok(true)
}

// ── commands ─────────────────────────────────────────────────────────────────

/// Read persisted onboarding state and detect installed integrations.
///
/// The PATH probe runs the CLI, so the app shell's every-launch call pays for
/// it only while onboarding is still incomplete (the wizard is the one reader
/// of the field then); [`get_integrations_status`] always probes.
#[tauri::command]
pub async fn check_onboarding_status() -> Result<OnboardingStatus, String> {
    onboarding_status(false).await
}

async fn onboarding_status(always_probe_cli: bool) -> Result<OnboardingStatus, String> {
    let cfg = read_config().await?;

    let home = dirs::home_dir().ok_or("Could not determine home directory")?;

    // The real question is whether `nodespace` resolves, not whether a file
    // mentions it: a Homebrew cask links it into place with no rc edit at all.
    let path_already_configured =
        (always_probe_cli || !cfg.onboarding_completed) && cli_resolves_on_path().await;
    let path_export_present = any_rc_file_has_path(&home).await;

    Ok(OnboardingStatus {
        completed: cfg.onboarding_completed,
        path_configured: cfg.integrations.path_configured,
        skill_configured: cfg.integrations.skill_configured,
        path_already_configured,
        path_export_present,
    })
}

/// Which agents the skill installer would target right now. See
/// [`DetectedAgents`] for why this is separate from
/// [`check_onboarding_status`].
#[tauri::command]
pub async fn detect_agents(app_handle: tauri::AppHandle) -> DetectedAgents {
    match skill_setup::detect_agents(&app_handle).await {
        Some(agents) => DetectedAgents {
            agents,
            detection_failed: false,
        },
        None => DetectedAgents {
            agents: vec![],
            detection_failed: true,
        },
    }
}

/// Add the NodeSpace PATH export to the rc file of the user's login shell
/// (`SHELL`), creating it when missing, and report which file changed.
/// Does nothing when `nodespace` already resolves, and never adds a second
/// export when any known rc file already has one. Updates the
/// `path_configured` flag in config.json only when the PATH is in place.
#[tauri::command]
pub async fn configure_path() -> Result<PathConfigResult, String> {
    if cfg!(windows) {
        return Err(
            "Adding NodeSpace to PATH from the app is not supported on Windows; \
             add the NodeSpace CLI folder to your PATH in System Settings."
                .to_string(),
        );
    }
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    let shell = LoginShell::from_env(std::env::var("SHELL").ok().as_deref());
    let cli_on_path = cli_resolves_on_path().await;

    let result = configure_path_at(&home, shell, cli_on_path).await?;

    // Recorded only when NodeSpace's own line is in place; a CLI that already
    // resolves by other means (the cask) left nothing of ours configured.
    if !result.already_on_path {
        update_config(|cfg| cfg.integrations.path_configured = true).await?;
    }
    Ok(result)
}

/// Whether `nodespace` resolves on a shell PATH approximated by
/// [`skill_setup::check_cli_on_path`] (the system PATH plus the usual
/// per-user bin directories, not the user's own rc files). Runs the CLI, so it
/// is kept off the async worker threads.
async fn cli_resolves_on_path() -> bool {
    tokio::task::spawn_blocking(skill_setup::check_cli_on_path)
        .await
        .unwrap_or(false)
}

/// Install the NodeSpace skill into detected agents (delegates to skill_setup).
/// Idempotent when called via the onboarding wizard; marks skill_configured in config.
///
/// Returns the full result (not just success/failure) so the wizard can name
/// which agents actually got the skill, and which were detected but had
/// nothing to install -- a bare `Ok(())` gave the caller no way to report
/// that, which is what made a correct multi-agent install read as silence.
#[tauri::command]
pub async fn configure_skill(app_handle: tauri::AppHandle) -> Result<SkillSetupResult, String> {
    let result = skill_setup::install_skill(false, &app_handle).await;
    if result.success {
        Ok(result)
    } else {
        Err(result
            .error
            .unwrap_or_else(|| "Skill installation failed".to_string()))
    }
}

/// Run the skill installer (for manual re-trigger from Settings → Integrations).
/// `force = true` bypasses the idempotency guard in setup.json.
#[tauri::command]
pub async fn install_skill(
    force: bool,
    app_handle: tauri::AppHandle,
) -> Result<SkillSetupResult, String> {
    Ok(skill_setup::install_skill(force, &app_handle).await)
}

/// Return the current skill setup status without re-running the installer.
///
/// `agents_installed` is revalidated against the filesystem (via the
/// installer's `status` subcommand) before being returned -- the persisted
/// list is only ever written by a successful install and otherwise never
/// checked, so it would otherwise keep claiming a harness has the skill long
/// after a user deleted that harness's skill directory by hand.
#[tauri::command]
pub async fn get_skill_setup_status(
    app_handle: tauri::AppHandle,
) -> Result<SkillSetupResult, String> {
    let state = skill_setup::read_setup_state()
        .await
        .map_err(|e| e.to_string())?;
    let cli_on_path = skill_setup::check_cli_on_path();
    let agents_installed =
        skill_setup::revalidate_agents_installed_locked(state.agents_installed, &app_handle).await;
    Ok(SkillSetupResult {
        success: state.skill_installed,
        agents_installed,
        agents_skipped: vec![],
        cli_on_path,
        cli_warning: skill_setup::cli_warning(cli_on_path),
        error: None,
        failure_is_new: false,
    })
}

/// Remove the NodeSpace PATH export from every rc file it may have been
/// written to. Updates the `path_configured` flag in config.json.
#[tauri::command]
pub async fn remove_from_path() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    for file in known_rc_files(&home) {
        remove_path_from_file(&file).await?;
    }

    update_config(|cfg| cfg.integrations.path_configured = false).await
}

/// Return live integration status (path + skill) without running any installer.
#[tauri::command]
pub async fn get_integrations_status() -> Result<OnboardingStatus, String> {
    onboarding_status(true).await
}

/// Remove the NodeSpace skill files from every installed-into agent's skill
/// directory (Claude Code, Codex, Antigravity CLI, OpenCode -- whichever the
/// installer's `AGENTS` config currently lists), by delegating to
/// `install.ts`'s own `uninstall` command. Resets both setup.json
/// (authoritative for get_skill_setup_status) and config.json.
#[tauri::command]
pub async fn remove_skill(app_handle: tauri::AppHandle) -> Result<(), String> {
    skill_setup::uninstall_skill(&app_handle).await?;

    update_config(|cfg| cfg.integrations.skill_configured = false).await
}

/// Persist the onboarding completion state to `~/.nodespace/config.json`.
/// `identity_skipped` is true when the main wizard's identity step was shown
/// and the user explicitly skipped it (never asked, or saved, both leave it
/// false). Setting `identity_prompt_dismissed` here mirrors
/// `dismiss_identity_backfill_prompt`: a user who just said no to the exact
/// same prompt during first-run setup should not be hit with the standalone
/// backfill nudge on their very next launch.
#[tauri::command]
pub async fn complete_onboarding(
    path_configured: bool,
    skill_configured: bool,
    identity_skipped: bool,
) -> Result<(), String> {
    update_config(|cfg| {
        cfg.onboarding_completed = true;
        cfg.integrations.path_configured = path_configured;
        cfg.integrations.skill_configured = skill_configured;
        if identity_skipped {
            cfg.identity_prompt_dismissed = true;
        }
    })
    .await
}

// ── local identity (ADR-037) ─────────────────────────────────────────────────

/// The seeded local-user PersonNode's identity, as shown to the frontend.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LocalIdentity {
    pub node_id: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    /// True when neither name field nor email has ever been filled in —
    /// drives both the onboarding wizard's identity step (shown only when
    /// blank) and the backfill nudge for already-onboarded installs.
    pub is_blank: bool,
}

fn local_identity_from_node_data(data: NodeData) -> LocalIdentity {
    let props: serde_json::Value = serde_json::from_str(&data.properties).unwrap_or_default();
    let first_name = props
        .get("person")
        .and_then(|p| p.get("first_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let last_name = props
        .get("person")
        .and_then(|p| p.get("last_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let email = props
        .get("person")
        .and_then(|p| p.get("email"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let is_blank = first_name.is_empty() && last_name.is_empty() && email.is_empty();
    LocalIdentity {
        node_id: data.id,
        first_name,
        last_name,
        email,
        is_blank,
    }
}

/// Read the seeded local-user PersonNode's current identity. `None` only
/// when the database has no person node at all (should not happen post-seed).
#[tauri::command]
pub async fn get_local_identity(
    client: State<'_, GrpcClient>,
) -> Result<Option<LocalIdentity>, String> {
    let mut c = client.client().await;
    let resp = c
        .get_local_person(Request::new(Empty {}))
        .await
        .map_err(status_message)?
        .into_inner();
    Ok(resp
        .node
        .and_then(|n| n.node_data)
        .map(local_identity_from_node_data))
}

/// Write first_name/last_name/email into the seeded local-user PersonNode
/// (never a newly created one — see
/// `NodeService::set_local_person_identity`). All fields are written
/// together; an empty value clears that field.
#[tauri::command]
pub async fn set_local_identity(
    client: State<'_, GrpcClient>,
    first_name: String,
    last_name: String,
    email: String,
) -> Result<LocalIdentity, String> {
    let mut c = client.client().await;
    let resp = c
        .set_local_person_identity(Request::new(SetLocalPersonIdentityRequest {
            first_name,
            last_name,
            email,
        }))
        .await
        .map_err(status_message)?
        .into_inner();
    let data = resp
        .node_data
        .ok_or_else(|| "gRPC response missing node_data".to_string())?;
    Ok(local_identity_from_node_data(data))
}

/// Best-effort suggestion for the identity prompt, sourced from `git config`
/// (both fields), falling back to the OS account's full name for `name`
/// alone. Never written anywhere on its own — the caller shows this for
/// confirmation only ("prefill, do not auto-commit").
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IdentityPrefill {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[tauri::command]
pub async fn get_identity_prefill() -> IdentityPrefill {
    let git_name = run_git_config("user.name").await;
    let git_email = run_git_config("user.email").await;
    let name = match git_name {
        Some(n) => Some(n),
        None => os_full_name().await,
    };
    IdentityPrefill {
        name,
        email: git_email,
    }
}

async fn run_git_config(key: &str) -> Option<String> {
    let mut command = tokio::process::Command::new("git");
    // A GUI process has no console, so Windows would flash one for `git`.
    #[cfg(windows)]
    command.creation_flags(crate::daemon_setup::CREATE_NO_WINDOW);
    let output = command.args(["config", "--get", key]).output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Best-effort, macOS/Linux-oriented (the only currently-shipped desktop
/// platforms): the account's Real Name via `id -F`, distinct from the short
/// login name `whoami`/`$USER` would return. Absent on Windows — the command
/// simply fails to spawn there, which resolves to `None` like any other miss.
async fn os_full_name() -> Option<String> {
    let output = tokio::process::Command::new("id")
        .arg("-F")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Whether the backfill nudge should fire for an already-onboarded install:
/// true only when the seeded local person is still blank AND the user has
/// not already dismissed the nudge once (see `dismiss_identity_backfill_prompt`).
#[tauri::command]
pub async fn should_prompt_identity_backfill(
    client: State<'_, GrpcClient>,
) -> Result<bool, String> {
    let cfg = read_config().await?;
    if cfg.identity_prompt_dismissed {
        return Ok(false);
    }
    let mut c = client.client().await;
    let resp = c
        .get_local_person(Request::new(Empty {}))
        .await
        .map_err(status_message)?
        .into_inner();
    Ok(resp
        .node
        .and_then(|n| n.node_data)
        .map(|data| local_identity_from_node_data(data).is_blank)
        .unwrap_or(false))
}

/// Persist that the backfill nudge has been shown and resolved (saved or
/// skipped) once, so it does not reappear on every subsequent launch. The
/// identity itself stays editable any time from Settings regardless.
#[tauri::command]
pub async fn dismiss_identity_backfill_prompt() -> Result<(), String> {
    update_config(|cfg| cfg.identity_prompt_dismissed = true).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves `CONFIG_LOCK` prevents the lost-update race: concurrent updates
    /// to DIFFERENT fields, all racing the same read-modify-write cycle.
    /// Without the lock a later rename can overwrite a file read before an
    /// earlier update landed, dropping that update's field.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_updates_to_different_fields_all_survive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dir_path = dir.path().to_path_buf();

        let mut tasks = Vec::new();
        for i in 0..20u32 {
            let dir_path = dir_path.clone();
            tasks.push(tokio::spawn(async move {
                update_config_at(&dir_path, |cfg| match i % 4 {
                    0 => cfg.onboarding_completed = true,
                    1 => cfg.integrations.path_configured = true,
                    2 => cfg.integrations.skill_configured = true,
                    _ => cfg.identity_prompt_dismissed = true,
                })
                .await
            }));
        }
        for t in tasks {
            t.await.expect("task panicked").expect("update failed");
        }

        let cfg = read_config_at(&dir_path).await.expect("read failed");
        assert!(cfg.onboarding_completed);
        assert!(cfg.integrations.path_configured);
        assert!(cfg.integrations.skill_configured);
        assert!(cfg.identity_prompt_dismissed);
        assert!(!dir_path.join("config.json.tmp").exists());
    }

    fn export_count(content: &str) -> usize {
        content.matches("$HOME/.nodespace/bin").count()
    }

    #[tokio::test]
    async fn missing_rc_file_is_created_and_reported() {
        let home = tempfile::tempdir().expect("tempdir");
        let result = configure_path_at(home.path(), LoginShell::Zsh, false)
            .await
            .expect("configure");

        let zshrc = home.path().join(".zshrc");
        assert_eq!(
            result.files,
            vec![PathFileReport {
                path: zshrc.display().to_string(),
                outcome: PathFileOutcome::Created,
            }]
        );
        assert!(result.restart_required);
        assert!(!result.already_on_path);
        let written = std::fs::read_to_string(&zshrc).expect("read");
        assert_eq!(export_count(&written), 1);
    }

    #[tokio::test]
    async fn existing_rc_file_is_appended_to_and_keeps_its_content() {
        let home = tempfile::tempdir().expect("tempdir");
        let zshrc = home.path().join(".zshrc");
        std::fs::write(&zshrc, "alias ll='ls -l'\n").expect("seed");

        let result = configure_path_at(home.path(), LoginShell::Zsh, false)
            .await
            .expect("configure");

        assert_eq!(result.files[0].outcome, PathFileOutcome::Written);
        let written = std::fs::read_to_string(&zshrc).expect("read");
        assert!(written.starts_with("alias ll='ls -l'\n"));
        assert_eq!(export_count(&written), 1);
    }

    #[tokio::test]
    async fn a_second_run_leaves_one_export_line() {
        let home = tempfile::tempdir().expect("tempdir");
        configure_path_at(home.path(), LoginShell::Zsh, false)
            .await
            .expect("first");
        let second = configure_path_at(home.path(), LoginShell::Zsh, false)
            .await
            .expect("second");

        assert_eq!(second.files[0].outcome, PathFileOutcome::Unchanged);
        let written = std::fs::read_to_string(home.path().join(".zshrc")).expect("read");
        assert_eq!(export_count(&written), 1);
    }

    #[tokio::test]
    async fn a_line_the_installer_wrote_elsewhere_is_not_duplicated() {
        let home = tempfile::tempdir().expect("tempdir");
        let bashrc = home.path().join(".bashrc");
        std::fs::write(&bashrc, POSIX_EXPORT_LINE).expect("seed");

        let result = configure_path_at(home.path(), LoginShell::Zsh, false)
            .await
            .expect("configure");

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].outcome, PathFileOutcome::Unchanged);
        assert!(!home.path().join(".zshrc").exists());
    }

    #[tokio::test]
    async fn nothing_is_touched_when_the_cli_already_resolves() {
        let home = tempfile::tempdir().expect("tempdir");
        let result = configure_path_at(home.path(), LoginShell::Zsh, true)
            .await
            .expect("configure");

        assert!(result.already_on_path);
        assert!(result.files.is_empty());
        assert!(!result.restart_required);
        assert!(!home.path().join(".zshrc").exists());
    }

    #[tokio::test]
    async fn other_shells_get_their_own_file_and_syntax() {
        let fish_home = tempfile::tempdir().expect("tempdir");
        configure_path_at(fish_home.path(), LoginShell::Fish, false)
            .await
            .expect("fish");
        let fish = std::fs::read_to_string(fish_home.path().join(".config/fish/config.fish"))
            .expect("config.fish");
        assert!(fish.contains("fish_add_path $HOME/.nodespace/bin"));
        assert!(!fish.contains("export PATH"));

        let other_home = tempfile::tempdir().expect("tempdir");
        configure_path_at(
            other_home.path(),
            LoginShell::from_env(Some("/bin/dash")),
            false,
        )
        .await
        .expect("other");
        assert!(other_home.path().join(".profile").exists());
    }

    #[test]
    fn bash_writes_bash_profile_on_macos_and_bashrc_elsewhere() {
        let home = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            bash_target(home.path(), true),
            home.path().join(".bash_profile")
        );
        // A `.bashrc` alone must not win on macOS: login shells skip it.
        std::fs::write(home.path().join(".bashrc"), "").expect("seed");
        assert_eq!(
            bash_target(home.path(), true),
            home.path().join(".bash_profile")
        );
        assert_eq!(bash_target(home.path(), false), home.path().join(".bashrc"));
        std::fs::write(home.path().join(".bash_profile"), "").expect("seed");
        assert_eq!(
            bash_target(home.path(), false),
            home.path().join(".bash_profile")
        );
    }

    #[test]
    fn the_login_shell_comes_from_shell_env() {
        assert_eq!(LoginShell::from_env(Some("/bin/zsh")), LoginShell::Zsh);
        assert_eq!(
            LoginShell::from_env(Some("/opt/homebrew/bin/bash")),
            LoginShell::Bash
        );
        assert_eq!(
            LoginShell::from_env(Some("/usr/bin/fish")),
            LoginShell::Fish
        );
        assert_eq!(LoginShell::from_env(None), LoginShell::Other);
    }

    #[tokio::test]
    async fn commented_or_user_written_lines_are_not_a_nodespace_block() {
        let home = tempfile::tempdir().expect("tempdir");
        let rc = home.path().join(".zshrc");
        let user = "# export PATH=\"$HOME/.nodespace/bin:$PATH\"\n\
                    export PATH=\"$HOME/.nodespace/bin:$HOME/bin:$PATH\"\n\
                    if [ -d \"$HOME/.nodespace/bin\" ]; then\n  echo $HOME/.nodespace/bin\nfi\n";
        std::fs::write(&rc, user).expect("seed");
        assert!(!file_contains_nodespace_path(&rc).await);
        assert!(!remove_path_from_file(&rc).await.expect("remove"));
        assert_eq!(std::fs::read_to_string(&rc).expect("read"), user);

        // The wizard now writes its own block instead of reporting success.
        let outcome = append_path_to_file(&rc, POSIX_EXPORT_LINE)
            .await
            .expect("append");
        assert_eq!(outcome, PathFileOutcome::Written);
        assert!(file_contains_nodespace_path(&rc).await);

        // Removal takes only that block and restores the user's content.
        assert!(remove_path_from_file(&rc).await.expect("remove"));
        assert_eq!(std::fs::read_to_string(&rc).expect("read"), user);
    }

    #[tokio::test]
    async fn crlf_files_keep_their_line_endings() {
        let home = tempfile::tempdir().expect("tempdir");
        let rc = home.path().join(".bashrc");
        std::fs::write(&rc, "keep one\r\nkeep two\r\n").expect("seed");
        append_path_to_file(&rc, POSIX_EXPORT_LINE)
            .await
            .expect("append");
        let written = std::fs::read_to_string(&rc).expect("read");
        assert!(
            written.contains("\r\n# NodeSpace CLI\r\nexport PATH"),
            "{written:?}"
        );
        assert!(!written.replace("\r\n", "").contains('\n'));
        assert!(file_contains_nodespace_path(&rc).await);

        assert!(remove_path_from_file(&rc).await.expect("remove"));
        assert_eq!(
            std::fs::read_to_string(&rc).expect("read"),
            "keep one\r\nkeep two\r\n"
        );
    }

    #[tokio::test]
    async fn a_mixed_line_ending_file_gets_lf() {
        let home = tempfile::tempdir().expect("tempdir");
        let rc = home.path().join(".bashrc");
        std::fs::write(&rc, "one\r\ntwo\n").expect("seed");
        append_path_to_file(&rc, POSIX_EXPORT_LINE)
            .await
            .expect("append");
        let written = std::fs::read_to_string(&rc).expect("read");
        assert!(
            written.ends_with("\n# NodeSpace CLI\nexport PATH=\"$HOME/.nodespace/bin:$PATH\"\n")
        );
        assert!(!written.ends_with("\r\n"));
    }

    #[tokio::test]
    async fn fish_block_and_duplicates_are_removed_exactly() {
        let home = tempfile::tempdir().expect("tempdir");
        let rc = home.path().join("rc");
        std::fs::write(
            &rc,
            format!(
                "a\n{FISH_EXPORT_LINE}b\n{POSIX_EXPORT_LINE}c\n# NodeSpace CLI\nsomething else\n"
            ),
        )
        .expect("seed");
        assert!(remove_path_from_file(&rc).await.expect("remove"));
        // A lone "# NodeSpace CLI" comment not followed by our export is kept.
        assert_eq!(
            std::fs::read_to_string(&rc).expect("read"),
            "a\nb\nc\n# NodeSpace CLI\nsomething else\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_rc_file_is_edited_at_its_target_and_keeps_its_mode() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let home = tempfile::tempdir().expect("tempdir");
        let dotfiles = home.path().join("dotfiles");
        std::fs::create_dir(&dotfiles).expect("mkdir");
        let real = dotfiles.join("zshrc");
        std::fs::write(&real, "keep\n").expect("seed");
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).expect("chmod");
        let link = home.path().join(".zshrc");
        symlink(&real, &link).expect("symlink");

        append_path_to_file(&link, POSIX_EXPORT_LINE)
            .await
            .expect("append");
        assert!(std::fs::symlink_metadata(&link)
            .expect("meta")
            .file_type()
            .is_symlink());
        assert!(std::fs::read_to_string(&real)
            .expect("read")
            .contains(POSIX_EXPORT));
        let mode = std::fs::metadata(&real).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        assert!(remove_path_from_file(&link).await.expect("remove"));
        assert!(std::fs::symlink_metadata(&link)
            .expect("meta")
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&real).expect("read"), "keep\n");
        let leftovers: Vec<_> = std::fs::read_dir(&dotfiles).expect("ls").collect();
        assert_eq!(leftovers.len(), 1, "no temp file left behind");
    }

    /// A write that cannot complete (here: the directory refuses a new temp
    /// file) must leave the original rc file untouched, not truncated.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_write_leaves_the_original_file_intact() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().expect("tempdir");
        let rc = home.path().join(".zshrc");
        let original = format!("keep\n{POSIX_EXPORT_LINE}");
        std::fs::write(&rc, &original).expect("seed");
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o555))
            .expect("chmod dir");
        let probe = std::fs::File::create(home.path().join("probe"));
        let can_write_anyway = probe.is_ok();

        let removed = remove_path_from_file(&rc).await;

        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o755))
            .expect("restore");
        if can_write_anyway {
            return; // running as root: directory modes are not enforced
        }
        assert!(removed.is_err(), "removal must report the failed write");
        assert_eq!(std::fs::read_to_string(&rc).expect("read"), original);
    }

    #[tokio::test]
    async fn removal_clears_every_known_file() {
        let home = tempfile::tempdir().expect("tempdir");
        for name in [".zshrc", ".bashrc", ".profile"] {
            std::fs::write(
                home.path().join(name),
                format!("keep me\n{POSIX_EXPORT_LINE}"),
            )
            .expect("seed");
        }
        for file in known_rc_files(home.path()) {
            remove_path_from_file(&file).await.expect("remove");
        }
        assert!(!any_rc_file_has_path(home.path()).await);
        let kept = std::fs::read_to_string(home.path().join(".zshrc")).expect("read");
        assert_eq!(kept, "keep me\n");
    }

    /// An unparseable config must abort the update rather than write defaults
    /// over it, which would silently reset every other persisted field.
    #[tokio::test]
    async fn unparseable_config_is_not_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(CONFIG_FILE);
        tokio::fs::write(&path, "{ not json").await.expect("seed");

        let result = update_config_at(dir.path(), |cfg| cfg.onboarding_completed = true).await;

        assert!(result.is_err());
        let raw = tokio::fs::read_to_string(&path).await.expect("read");
        assert_eq!(raw, "{ not json");
    }
}
