//! Settings commands for reading and updating app preferences
//!
//! These commands expose the preferences system to the frontend.
//! Display settings (theme, markdown rendering) take effect immediately.
//! Database settings require an app restart.

use tauri::{AppHandle, Manager};

/// Settings response sent to the frontend
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsResponse {
    /// Currently active database path (from runtime AppConfig)
    pub active_database_path: String,
    /// User's saved database path preference (may differ if restart pending)
    pub saved_database_path: Option<String>,
    /// Display preferences
    pub display: DisplaySettingsResponse,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySettingsResponse {
    pub render_markdown: bool,
    pub theme: String,
}

/// Get current app settings for the Settings UI
#[tauri::command]
pub async fn get_settings(app: AppHandle) -> Result<SettingsResponse, String> {
    let prefs = crate::preferences::load_preferences(&app).await?;
    let config: tauri::State<crate::config::AppConfig> = app.state();

    Ok(SettingsResponse {
        active_database_path: config.database_path.to_string_lossy().to_string(),
        saved_database_path: prefs.database_path.map(|p| p.to_string_lossy().to_string()),
        display: DisplaySettingsResponse {
            render_markdown: prefs.display.render_markdown,
            theme: prefs.display.theme,
        },
    })
}

/// Update display settings (takes effect immediately, no restart required)
///
/// Saves to preferences.json and emits a "settings-changed" Tauri event
/// so all open panes can react to the change.
#[tauri::command]
pub async fn update_display_settings(
    app: AppHandle,
    render_markdown: Option<bool>,
    theme: Option<String>,
) -> Result<(), String> {
    use tauri::Emitter;

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

    // Emit settings-changed event to frontend for reactive updates
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit(
            "settings-changed",
            serde_json::json!({
                "renderMarkdown": prefs.display.render_markdown,
                "theme": prefs.display.theme,
            }),
        );
    }

    Ok(())
}

/// Result of selecting a new database location
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingDatabaseChange {
    pub new_path: String,
    pub requires_restart: bool,
}

/// Open native folder picker and save chosen database path to preferences.
/// Does NOT reinitialize services — app must restart for new database.
#[tauri::command]
pub async fn select_new_database(app: tauri::AppHandle) -> Result<PendingDatabaseChange, String> {
    use tauri_plugin_dialog::{DialogExt, FilePath};

    let folder = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .ok_or_else(|| "No folder selected".to_string())?;

    let folder_path = match folder {
        FilePath::Path(path) => path,
        FilePath::Url(url) => std::path::PathBuf::from(url.path()),
    };

    let db_path = folder_path;

    let mut prefs = crate::preferences::load_preferences(&app).await?;
    prefs.database_path = Some(db_path.clone());
    crate::preferences::save_preferences(&app, &prefs).await?;

    Ok(PendingDatabaseChange {
        new_path: db_path.to_string_lossy().to_string(),
        requires_restart: true,
    })
}

/// Restart the application.
#[tauri::command]
pub fn restart_app(app: tauri::AppHandle) {
    app.restart();
}

/// Reset database path to default. Requires restart.
#[tauri::command]
pub async fn reset_database_to_default(app: tauri::AppHandle) -> Result<String, String> {
    let mut prefs = crate::preferences::load_preferences(&app).await?;
    prefs.database_path = None;
    crate::preferences::save_preferences(&app, &prefs).await?;

    let default_path = crate::preferences::get_default_database_path()?;
    Ok(default_path.to_string_lossy().to_string())
}
