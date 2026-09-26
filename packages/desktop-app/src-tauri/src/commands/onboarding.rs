//! Onboarding wizard Tauri commands.
//!
//! Handles first-launch setup: PATH configuration and NodeSpace skill
//! installation. Completion state is persisted to `~/.nodespace/config.json`.
//! Skill installation state is tracked separately in `~/.nodespace/setup.json`.

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
/// Deliberately cheap: a config read plus a few `.exists()` probes. The
/// app shell invokes this on EVERY launch just to read `completed`, so
/// nothing expensive belongs here — agent detection, which shells out to the
/// skill installer, is its own command ([`detect_agents`]) that only the
/// callers actually needing it pay for.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardingStatus {
    pub completed: bool,
    pub path_configured: bool,
    pub skill_configured: bool,
    pub path_already_configured: bool,
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
    let path = dir.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(NodespaceConfig::default());
    }
    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("Failed to read config: {e}"))?;
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
    let mut cfg = read_config_at(dir).await?;
    mutate(&mut cfg);
    atomic_file::write_json(dir, CONFIG_FILE, &cfg).await
}

/// Return true if the PATH export line is already present in the given file.
async fn file_contains_nodespace_path(path: &PathBuf) -> bool {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => content.contains("$HOME/.nodespace/bin"),
        Err(_) => false,
    }
}

/// Append `export PATH` line to a shell file if the file exists and the line
/// is not already present.  Returns `true` if the file was modified.
async fn append_path_to_file(path: &PathBuf) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    if file_contains_nodespace_path(path).await {
        return Ok(false);
    }
    let line = "\n# NodeSpace CLI\nexport PATH=\"$HOME/.nodespace/bin:$PATH\"\n";
    let mut content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    content.push_str(line);
    tokio::fs::write(path, content)
        .await
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    Ok(true)
}

/// Remove all NodeSpace PATH lines from a shell file. Returns `true` if modified.
async fn remove_path_from_file(path: &PathBuf) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    if !content.contains("$HOME/.nodespace/bin") {
        return Ok(false);
    }
    let filtered: Vec<&str> = content
        .lines()
        .filter(|l| {
            !l.contains("$HOME/.nodespace/bin") && !l.trim_start().starts_with("# NodeSpace CLI")
        })
        .collect();
    // Strip trailing blank lines left by the separator we wrote, then restore final newline.
    let trimmed: Vec<&str> = filtered
        .iter()
        .copied()
        .rev()
        .skip_while(|l| l.trim().is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let mut result = trimmed.join("\n");
    result.push('\n');
    tokio::fs::write(path, result)
        .await
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
    Ok(true)
}

// ── commands ─────────────────────────────────────────────────────────────────

/// Read persisted onboarding state and detect installed integrations.
#[tauri::command]
pub async fn check_onboarding_status() -> Result<OnboardingStatus, String> {
    let cfg = read_config().await?;

    let home = dirs::home_dir().ok_or("Could not determine home directory")?;

    // Check whether the PATH export is already in any shell config.
    let zshrc = home.join(".zshrc");
    let bash_profile = home.join(".bash_profile");
    let path_already_configured = file_contains_nodespace_path(&zshrc).await
        || file_contains_nodespace_path(&bash_profile).await;

    Ok(OnboardingStatus {
        completed: cfg.onboarding_completed,
        path_configured: cfg.integrations.path_configured,
        skill_configured: cfg.integrations.skill_configured,
        path_already_configured,
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

/// Append the NodeSpace PATH export to `~/.zshrc` and/or `~/.bash_profile`
/// (whichever exist). Idempotent — will not add the line if already present.
/// Updates the `path_configured` flag in config.json.
#[tauri::command]
pub async fn configure_path() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;

    append_path_to_file(&home.join(".zshrc")).await?;
    append_path_to_file(&home.join(".bash_profile")).await?;

    update_config(|cfg| cfg.integrations.path_configured = true).await
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

/// Remove the NodeSpace PATH export from `~/.zshrc` and/or `~/.bash_profile`.
/// Updates the `path_configured` flag in config.json.
#[tauri::command]
pub async fn remove_from_path() -> Result<(), String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    remove_path_from_file(&home.join(".zshrc")).await?;
    remove_path_from_file(&home.join(".bash_profile")).await?;

    update_config(|cfg| cfg.integrations.path_configured = false).await
}

/// Return live integration status (path + skill) without running any installer.
#[tauri::command]
pub async fn get_integrations_status() -> Result<OnboardingStatus, String> {
    check_onboarding_status().await
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
        .map_err(|e| e.to_string())?
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
        .map_err(|e| e.to_string())?
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
    let output = tokio::process::Command::new("git")
        .args(["config", "--get", key])
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
        .map_err(|e| e.to_string())?
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
