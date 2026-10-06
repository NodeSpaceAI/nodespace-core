//! Settings commands for reading and updating app preferences.
//!
//! The settings the daemon acts on (session capture, external tools, provider
//! configs) are fields of each database's `database-settings` node and are
//! read and written through that node's typed update (ADR-095); nothing here
//! touches them.
//!
//! The active database path is read from the `DatabaseService` registry
//! (ADR-053: one daemon, multiple local databases), which is the source of
//! truth for which database is default.
//!
//! Display preferences (theme, render_markdown) are UI-only state that remain
//! in Tauri local storage and are never sent to the daemon.

use crate::services::GrpcClient;
use nodespace_proto::nodespace::ListDatabasesRequest;
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
