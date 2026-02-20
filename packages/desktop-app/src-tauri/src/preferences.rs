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

    fs::create_dir_all(&config_dir)
        .await
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let pref_file = config_dir.join(PREF_FILE);
    let temp_file = config_dir.join(format!("{}.tmp", PREF_FILE));

    let serialized = serde_json::to_string_pretty(prefs)
        .map_err(|e| format!("Failed to serialize preferences: {}", e))?;

    // Atomic write: write to temp file, then rename
    fs::write(&temp_file, serialized)
        .await
        .map_err(|e| format!("Failed to write preferences: {}", e))?;

    fs::rename(&temp_file, &pref_file)
        .await
        .map_err(|e| format!("Failed to save preferences: {}", e))?;

    Ok(())
}

/// Get default database path for current platform
///
/// Checks `NODESPACE_DB_PATH` environment variable first, then falls back to
/// unified path across all platforms: `~/.nodespace/database/nodespace`
///
/// # Environment Variable
/// * `NODESPACE_DB_PATH` - Override default database location (useful for testing/benchmarks)
///
/// # Returns
/// * `Ok(PathBuf)` - Default database path
/// * `Err(String)` - If home directory cannot be determined
pub fn get_default_database_path() -> Result<PathBuf, String> {
    // Check for environment variable override first
    if let Ok(env_path) = std::env::var("NODESPACE_DB_PATH") {
        tracing::info!("Using database path from NODESPACE_DB_PATH: {}", env_path);
        return Ok(PathBuf::from(env_path));
    }

    let home_dir = dirs::home_dir().ok_or_else(|| "Failed to get home directory".to_string())?;

    Ok(home_dir
        .join(".nodespace")
        .join("database")
        .join("nodespace"))
}

/// Migrate database from old platform-specific location to ~/.nodespace/
///
/// Automatically moves existing database from:
/// - macOS: ~/Library/Application Support/com.nodespace.app/nodespace.db
/// - Windows: %APPDATA%/com.nodespace.app/nodespace.db
/// - Linux: ~/.config/com.nodespace.app/nodespace.db
///
/// To new unified location:
/// - All platforms: ~/.nodespace/database/nodespace
///
/// This migration happens transparently on first run after update.
///
/// # Arguments
/// * `app` - Tauri application handle for determining old path location
///
/// # Returns
/// * `Ok(())` - Migration completed or no migration needed
/// * `Err(String)` - Error during migration
pub async fn migrate_legacy_database_if_needed(app: &AppHandle) -> Result<(), String> {
    let old_path = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get old app data directory: {}", e))?
        .join("nodespace");

    if !old_path.exists() {
        return Ok(()); // Nothing to migrate
    }

    let new_path = get_default_database_path()?;

    if new_path.exists() {
        return Ok(()); // Already migrated
    }

    // Create new directory
    if let Some(parent) = new_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create new database directory: {}", e))?;
    }

    // Copy database to new location
    fs::copy(&old_path, &new_path)
        .await
        .map_err(|e| format!("Failed to migrate database: {}", e))?;

    // Save new preference
    let prefs = AppPreferences {
        database_path: Some(new_path.clone()),
        ..Default::default()
    };
    save_preferences(app, &prefs).await?;

    tracing::info!("Migrated database from {:?} to {:?}", old_path, new_path);

    Ok(())
}
