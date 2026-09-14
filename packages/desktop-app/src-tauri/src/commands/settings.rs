//! Settings commands for reading and updating app preferences.
//!
//! Daemon config (gRPC address) is owned by `nodespaced` and fetched/updated
//! via the `SettingsService` gRPC RPC.
//!
//! The active database path is not daemon config — it is read from the
//! `DatabaseService` registry (ADR-053: one daemon, multiple local
//! databases), which is the source of truth for which database is default.
//!
//! Display preferences (theme, render_markdown) are UI-only state that remain
//! in Tauri local storage and are never sent to the daemon.

use crate::services::GrpcClient;
use nodespace_proto::nodespace::{
    GetCaptureSettingsRequest, ListDatabasesRequest, ListOpenAiCompatConfigsRequest,
    OpenAiCompatConfig as ProtoOpenAiCompatConfig, SetOpenAiCompatConfigsRequest,
    UpdateCaptureSettingsRequest,
};
use tauri::AppHandle;

/// Settings response sent to the frontend.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsResponse {
    /// Path of the default registered database (from the DatabaseService
    /// registry). Empty string if no default is set.
    pub active_database_path: String,
    /// Display preferences.
    pub display: DisplaySettingsResponse,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySettingsResponse {
    pub render_markdown: bool,
    pub theme: String,
}

/// Get current app settings for the Settings UI.
///
/// The default database path is fetched from the `DatabaseService` registry;
/// display preferences are read from local Tauri storage.
#[tauri::command]
pub async fn get_settings(
    app: AppHandle,
    grpc_client: tauri::State<'_, GrpcClient>,
) -> Result<SettingsResponse, String> {
    let prefs = crate::preferences::load_preferences(&app).await?;

    let mut client = grpc_client.database_service_client().await;
    let listing = client
        .list(ListDatabasesRequest {})
        .await
        .map_err(|e| format!("Failed to list databases: {}", e))?
        .into_inner();

    let active_database_path = listing
        .databases
        .iter()
        .find(|db| db.is_default)
        .map(|db| db.path.clone())
        .unwrap_or_default();

    Ok(SettingsResponse {
        active_database_path,
        display: DisplaySettingsResponse {
            render_markdown: prefs.display.render_markdown,
            theme: prefs.display.theme,
        },
    })
}

/// Update display settings (takes effect immediately, no restart required).
///
/// Saves to preferences.json and emits a "settings-changed" Tauri event
/// so all open panes can react to the change.
#[tauri::command]
pub async fn update_display_settings(
    app: AppHandle,
    render_markdown: Option<bool>,
    theme: Option<String>,
) -> Result<(), String> {
    let mut prefs = crate::preferences::load_preferences(&app).await?;

    if let Some(rm) = render_markdown {
        prefs.display.render_markdown = rm;
    }
    if let Some(t) = &theme {
        if !["system", "light", "dark"].contains(&t.as_str()) {
            return Err(format!(
                "Invalid theme value: '{}'. Must be system, light, or dark.",
                t
            ));
        }
        prefs.display.theme = t.clone();
    }

    crate::preferences::save_preferences(&app, &prefs).await?;

    // Display preferences aren't database-scoped — route to the focused
    // window (see `window_routing`).
    crate::window_routing::emit_routed(
        &app,
        "settings-changed",
        serde_json::json!({
            "renderMarkdown": prefs.display.render_markdown,
            "theme": prefs.display.theme,
        }),
        None,
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Capture settings
// ---------------------------------------------------------------------------

/// Capture settings response sent to the frontend.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CaptureSettingsResult {
    pub enabled: bool,
    /// "metadata_only" | "summary" | "full"
    pub content: String,
}

/// Get session capture settings from the daemon.
#[tauri::command]
pub async fn get_capture_settings(
    grpc_client: tauri::State<'_, GrpcClient>,
) -> Result<CaptureSettingsResult, String> {
    let mut client = grpc_client.settings_client().await;
    let resp = client
        .get_capture_settings(GetCaptureSettingsRequest {})
        .await
        .map_err(|e| format!("Failed to get capture settings: {}", e))?
        .into_inner();

    Ok(CaptureSettingsResult {
        enabled: resp.enabled,
        content: content_level_to_str(resp.content),
    })
}

/// Update session capture settings.
#[tauri::command]
pub async fn update_capture_settings(
    grpc_client: tauri::State<'_, GrpcClient>,
    enabled: Option<bool>,
    content: Option<String>,
) -> Result<CaptureSettingsResult, String> {
    let content_i32 = content.as_deref().map(str_to_content_level).transpose()?;

    let mut client = grpc_client.settings_client().await;
    let resp = client
        .update_capture_settings(UpdateCaptureSettingsRequest {
            enabled,
            content: content_i32,
        })
        .await
        .map_err(|e| format!("Failed to update capture settings: {}", e))?
        .into_inner();

    Ok(CaptureSettingsResult {
        enabled: resp.enabled,
        content: content_level_to_str(resp.content),
    })
}

fn content_level_to_str(level: i32) -> String {
    match level {
        1 => "summary".to_string(),
        2 => "full".to_string(),
        _ => "metadata_only".to_string(),
    }
}

fn str_to_content_level(s: &str) -> Result<i32, String> {
    match s {
        "metadata_only" => Ok(0),
        "summary" => Ok(1),
        "full" => Ok(2),
        other => Err(format!(
            "Invalid content level '{}'. Must be metadata_only, summary, or full.",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible provider configs
// ---------------------------------------------------------------------------

/// An OpenAI-compatible provider config as sent to/from the frontend. Mirrors
/// `OpenAiCompatConfig` in `$lib/types/ai-chat-node.ts`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiCompatConfigResult {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    /// Model identifier sent as the wire-protocol "model" field — distinct
    /// from `name`, which is only a cosmetic UI label.
    pub model: String,
}

impl From<ProtoOpenAiCompatConfig> for OpenAiCompatConfigResult {
    fn from(c: ProtoOpenAiCompatConfig) -> Self {
        Self {
            id: c.id,
            name: c.name,
            base_url: c.base_url,
            api_key: c.api_key,
            model: c.model,
        }
    }
}

impl From<OpenAiCompatConfigResult> for ProtoOpenAiCompatConfig {
    fn from(c: OpenAiCompatConfigResult) -> Self {
        Self {
            id: c.id,
            name: c.name,
            base_url: c.base_url,
            api_key: c.api_key,
            model: c.model,
        }
    }
}

/// Get all configured OpenAI-compatible provider configs from the daemon.
#[tauri::command]
pub async fn get_openai_compat_configs(
    grpc_client: tauri::State<'_, GrpcClient>,
) -> Result<Vec<OpenAiCompatConfigResult>, String> {
    let mut client = grpc_client.settings_client().await;
    let resp = client
        .list_open_ai_compat_configs(ListOpenAiCompatConfigsRequest {})
        .await
        .map_err(|e| format!("Failed to list OpenAI-compat configs: {}", e))?
        .into_inner();

    Ok(resp
        .configs
        .into_iter()
        .map(OpenAiCompatConfigResult::from)
        .collect())
}

/// Replace the full set of OpenAI-compatible provider configs on the daemon.
#[tauri::command]
pub async fn set_openai_compat_configs(
    grpc_client: tauri::State<'_, GrpcClient>,
    configs: Vec<OpenAiCompatConfigResult>,
) -> Result<Vec<OpenAiCompatConfigResult>, String> {
    let mut client = grpc_client.settings_client().await;
    let resp = client
        .set_open_ai_compat_configs(SetOpenAiCompatConfigsRequest {
            configs: configs
                .into_iter()
                .map(ProtoOpenAiCompatConfig::from)
                .collect(),
        })
        .await
        .map_err(|e| format!("Failed to save OpenAI-compat configs: {}", e))?
        .into_inner();

    Ok(resp
        .configs
        .into_iter()
        .map(OpenAiCompatConfigResult::from)
        .collect())
}

// ---------------------------------------------------------------------------
// Windows daemon autorun (HKCU Run key) maintenance
// ---------------------------------------------------------------------------
//
// `daemon_setup::register_autorun_windows` writes an HKCU `Run` entry so the
// daemon restarts on next login — the only platform where the app does this
// itself rather than handing the daemon to a service manager (launchd on
// macOS, systemd on Linux both have their own uninstall path already). That
// write had no corresponding way for a user to discover or remove it; these
// two commands surface one from the Settings → About screen. Both are always
// registered (`generate_handler!` needs the item to exist unconditionally)
// but only do anything on Windows — macOS/Linux report "nothing to do"
// rather than the command being absent, so the frontend doesn't need its own
// platform check.

/// Whether the daemon's Windows autorun entry is currently registered.
/// Always `false` on macOS/Linux. The Settings UI calls this to decide
/// whether to show the "remove startup entry" action at all.
#[tauri::command]
pub fn windows_autorun_present() -> bool {
    #[cfg(windows)]
    {
        crate::daemon_setup::autorun_windows_present()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Remove the daemon's Windows autorun entry. Returns `Ok(true)` if an entry
/// was found and removed, `Ok(false)` if there was nothing to remove
/// (including every non-Windows platform). Errors only on an actual removal
/// failure.
#[tauri::command]
pub fn remove_windows_autorun() -> Result<bool, String> {
    #[cfg(windows)]
    {
        crate::daemon_setup::remove_autorun_windows().map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}
