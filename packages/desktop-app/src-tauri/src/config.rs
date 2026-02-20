//! Runtime application configuration
//!
//! AppConfig is the single source of truth for what the running process uses.
//! It is derived from AppPreferences at startup, enriched with resolved paths
//! and technical settings, then registered as Tauri managed state.
//!
//! AppConfig is NOT serialized — it is rebuilt on every launch.
//! For persistent user settings, see preferences.rs.

use std::path::PathBuf;

/// Runtime application configuration — derived from AppPreferences at startup.
/// Registered as Tauri state via app.manage(). Immutable for the app lifetime.
///
/// Access from any Tauri command via: State<'_, AppConfig>
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Resolved, absolute path to the active SurrealDB database directory
    pub database_path: PathBuf,

    /// Resolved path to the GGUF embedding model file
    pub model_path: PathBuf,

    /// MCP server port (from MCP_PORT env var or default 3100)
    pub mcp_port: u16,

    /// Stable client ID for domain event filtering (prevents UI feedback loops)
    pub tauri_client_id: String,
}

impl AppConfig {
    /// Build runtime config from user preferences and resolved paths.
    ///
    /// Called once during app startup in lib.rs before init_services().
    pub fn from_preferences(
        prefs: &crate::preferences::AppPreferences,
        model_path: PathBuf,
        mcp_port: u16,
    ) -> Result<Self, String> {
        let database_path = match &prefs.database_path {
            Some(p) => p.clone(),
            None => crate::preferences::get_default_database_path()?,
        };

        Ok(AppConfig {
            database_path,
            model_path,
            mcp_port,
            tauri_client_id: crate::constants::TAURI_CLIENT_ID.to_string(),
        })
    }
}
