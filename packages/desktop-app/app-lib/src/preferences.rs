//! Application preferences management
//!
//! Handles loading/saving user preferences for the Tauri app.
//! Preferences are stored in platform-specific config directory.

use std::path::PathBuf;

use tauri::{AppHandle, Manager};
use tokio::fs;

const PREF_FILE: &str = "preferences.json";

/// App-wide preferences structure
/// All fields use #[serde(default)] so existing preferences.json files
/// without the new fields will deserialize without error.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AppPreferences {
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_path: Option<PathBuf>,

    #[serde(default)]
    pub display: DisplayPreferences,

    #[serde(default)]
    pub import_sources: ImportSourcePreferences,
}

/// Display-related user preferences
/// Changes take effect immediately (no restart required)
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DisplayPreferences {
    /// Whether to render markdown in node content (default: false = raw text)
    #[serde(default)]
    pub render_markdown: bool,

    /// Color theme: "system", "light", or "dark" (default: "system")
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for DisplayPreferences {
    fn default() -> Self {
        Self {
            render_markdown: false,
            theme: default_theme(),
        }
    }
}

fn default_theme() -> String {
    "system".to_string()
}

/// Import source configuration (future: Notion, Confluence, etc.)
/// Currently empty — serde default ensures zero-breakage deserialization
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ImportSourcePreferences {}

/// Load preferences from config file
///
/// # Arguments
/// * `app` - Tauri application handle
///
/// # Returns
/// * `Ok(AppPreferences)` - Loaded preferences or defaults if file doesn't exist
/// * `Err(String)` - Error if config directory cannot be determined or file parsing fails
pub async fn load_preferences(app: &AppHandle) -> Result<AppPreferences, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get config directory: {}", e))?;

    let pref_file = config_dir.join(PREF_FILE);

    if !pref_file.exists() {
        return Ok(AppPreferences::default());
    }

    let contents = fs::read_to_string(&pref_file)
        .await
        .map_err(|e| format!("Failed to read preferences: {}", e))?;

    serde_json::from_str(&contents).map_err(|e| format!("Failed to parse preferences: {}", e))
}

/// Save preferences to config file
///
/// Uses atomic write pattern (write-to-temp, then rename) to prevent
/// corruption on crash or power loss.
///
/// # Arguments
/// * `app` - Tauri application handle
/// * `prefs` - Preferences to save
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(String)` on failure
pub async fn save_preferences(app: &AppHandle, prefs: &AppPreferences) -> Result<(), String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get config directory: {}", e))?;

    crate::atomic_file::write_json(&config_dir, PREF_FILE, prefs).await
}
