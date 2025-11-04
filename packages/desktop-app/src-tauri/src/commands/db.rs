//! Database initialization and path management commands

use crate::commands::embeddings::EmbeddingState;
use nodespace_core::operations::NodeOperations;
use nodespace_core::services::{
    EmbeddingProcessor, EmbeddingProcessorConfig, NodeEmbeddingService, SchemaService,
};
use nodespace_core::{DatabaseService, NodeService};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::fs;

const DB_PATH_PREFERENCE_KEY: &str = "database_path";

/// Save database path preference to app config
///
/// This function merges the new database path into existing preferences,
/// preserving any other settings. Uses atomic write-then-rename pattern
/// to prevent corruption on crash/power loss.
///
/// # Arguments
/// * `app` - Tauri application handle
/// * `path` - Database file path to save
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(String)` with error description on failure
async fn save_db_path_preference(app: &AppHandle, path: &Path) -> Result<(), String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get config directory: {}", e))?;

    fs::create_dir_all(&config_dir)
        .await
        .map_err(|e| format!("Failed to create config directory: {}", e))?;

    let pref_file = config_dir.join("preferences.json");

    // Load existing preferences or create new
    let mut prefs = if pref_file.exists() {
        let contents = fs::read_to_string(&pref_file)
            .await
            .map_err(|e| format!("Failed to read existing preferences: {}", e))?;
        serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Update only the database path, preserving other settings
    prefs[DB_PATH_PREFERENCE_KEY] = serde_json::json!(path.to_string_lossy().to_string());

    // Atomic write: write to temp file then rename
    let temp_file = config_dir.join("preferences.json.tmp");
    let serialized = serde_json::to_string_pretty(&prefs)
        .map_err(|e| format!("Failed to serialize preferences: {}", e))?;

    fs::write(&temp_file, serialized)
        .await
        .map_err(|e| format!("Failed to write preferences: {}", e))?;

    fs::rename(&temp_file, &pref_file)
        .await
        .map_err(|e| format!("Failed to save preferences: {}", e))?;

    Ok(())
}

/// Load database path preference from app config
///
/// # Arguments
/// * `app` - Tauri application handle
///
/// # Returns
/// * `Ok(Some(PathBuf))` - Previously saved database path
/// * `Ok(None)` - No saved preference exists
/// * `Err(String)` - Error reading or parsing preferences
async fn load_db_path_preference(app: &AppHandle) -> Result<Option<PathBuf>, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to get config directory: {}", e))?;

    let pref_file = config_dir.join("preferences.json");

    if !pref_file.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&pref_file)
        .await
        .map_err(|e| format!("Failed to read preferences: {}", e))?;

    let prefs: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse preferences: {}", e))?;

    if let Some(path_str) = prefs.get(DB_PATH_PREFERENCE_KEY).and_then(|v| v.as_str()) {
        Ok(Some(PathBuf::from(path_str)))
    } else {
        Ok(None)
    }
}

/// Initialize database services with user-selected or default path
///
/// Checks if services are already initialized to prevent resource leaks
/// and state corruption from multiple initializations.
///
/// # Arguments
/// * `app` - Tauri application handle
/// * `db_path` - Path to database file
///
/// # Returns
/// * `Ok(())` on successful initialization
/// * `Err(String)` if already initialized or initialization fails
///
/// # State Management
/// Uses Tauri's state management via `app.manage()`. Once initialized,
/// services persist for the application lifetime. To change database location,
/// the application must be restarted.
async fn init_services(app: &AppHandle, db_path: PathBuf) -> Result<(), String> {
    // Check if state already exists to prevent reinitialization
    if app.try_state::<DatabaseService>().is_some() {
        return Err(
            "Database already initialized. Restart the app to change location.".to_string(),
        );
    }

    // Initialize database
    let db_service = DatabaseService::new(db_path)
        .await
        .map_err(|e| format!("Failed to initialize database: {}", e))?;

    let db_arc = Arc::new(db_service.clone());

    // Initialize node service
    let node_service = NodeService::new(db_service.clone())
        .map_err(|e| format!("Failed to initialize node service: {}", e))?;

    // Initialize NodeOperations business logic layer (wraps NodeService)
    let node_operations = NodeOperations::new(Arc::new(node_service.clone()));

    // Initialize schema service (wraps NodeService for schema operations)
    let schema_service = SchemaService::new(Arc::new(node_service.clone()));

    // Initialize embedding service (creates its own NLP engine internally)
    let embedding_service = NodeEmbeddingService::new_with_defaults(db_arc.clone())
        .map_err(|e| format!("Failed to initialize embedding service: {}", e))?;
    let embedding_service_arc = Arc::new(embedding_service);

    // Initialize and start background embedding processor
    let processor_config = EmbeddingProcessorConfig::default();
    let processor = Arc::new(EmbeddingProcessor::new(
        embedding_service_arc.clone(),
        db_arc,
        processor_config,
    ));

    // Start the background processor
    processor.clone().start();

    // Manage all services
    app.manage(db_service);
    app.manage(node_service);
    app.manage(node_operations);
    app.manage(schema_service);
    app.manage(EmbeddingState {
        service: embedding_service_arc,
    });
    app.manage(processor);

    // Initialize MCP server now that NodeService is available
    // MCP will use the same NodeService as Tauri commands
    if let Err(e) = crate::initialize_mcp_server(app.clone()) {
        tracing::error!("❌ Failed to initialize MCP server: {}", e);
        // Don't fail database init if MCP fails - MCP is optional
    }

    Ok(())
}

/// Select database location using native folder picker
///
/// Presents a native folder picker dialog to the user. Once selected,
/// saves the preference and initializes database services.
///
/// # Note on Blocking Dialog
/// Uses `blocking_pick_folder()` because user interaction dialogs
/// must synchronously wait for user input before proceeding. This is
/// intentional and required for proper UI/UX.
///
/// # Arguments
/// * `app` - Tauri application handle
///
/// # Returns
/// * `Ok(String)` - Path to the selected database file
/// * `Err(String)` - Error if no folder selected or initialization fails
///
/// # Errors
/// Returns error if:
/// - User cancels the folder picker
/// - Selected path cannot be accessed
/// - Database services are already initialized
/// - Database initialization fails
#[tauri::command]
pub async fn select_db_location(app: AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::{DialogExt, FilePath};

    let folder = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .ok_or_else(|| "No folder selected".to_string())?;

    let folder_path = match folder {
        FilePath::Path(path) => path,
        FilePath::Url(url) => PathBuf::from(url.path()),
    };

    let db_path = folder_path.join("nodespace.db");

    // Save preference
    save_db_path_preference(&app, &db_path).await?;

    // Initialize services
    init_services(&app, db_path.clone()).await?;

    Ok(db_path.to_string_lossy().to_string())
}

/// Migrate database from old platform-specific location to ~/.nodespace/
///
/// Automatically moves existing database from:
/// - macOS: ~/Library/Application Support/com.nodespace.app/nodespace.db
/// - Windows: %APPDATA%/com.nodespace.app/nodespace.db
/// - Linux: ~/.config/com.nodespace.app/nodespace.db
///
/// To new unified location:
/// - All platforms: ~/.nodespace/database/nodespace.db
///
/// This migration happens transparently on first run after update.
async fn migrate_database_if_needed(app: &AppHandle) -> Result<(), String> {
    let old_path = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get old app data directory: {}", e))?
        .join("nodespace.db");

    if !old_path.exists() {
        return Ok(()); // Nothing to migrate
    }

    let home_dir = dirs::home_dir().ok_or_else(|| "Failed to get home directory".to_string())?;

    let new_path = home_dir
        .join(".nodespace")
        .join("database")
        .join("nodespace.db");

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
    save_db_path_preference(app, new_path.as_path()).await?;

    tracing::info!("Migrated database from {:?} to {:?}", old_path, new_path);

    Ok(())
}

/// Initialize database with saved preference or default path
///
/// Checks for previously saved database location preference. If found,
/// uses that path. Otherwise, uses unified ~/.nodespace/database/ location
/// across all platforms.
///
/// This command should be called during application startup before any
/// database operations are attempted.
///
/// # Arguments
/// * `app` - Tauri application handle
///
/// # Returns
/// * `Ok(String)` - Path to the initialized database file
/// * `Err(String)` - Error if initialization fails
///
/// # Default Location (New Unified Path)
/// - All platforms: ~/.nodespace/database/nodespace.db
///
/// # Migration
/// Automatically migrates existing databases from old platform-specific
/// locations on first run.
///
/// # Errors
/// Returns error if:
/// - Database services are already initialized
/// - Cannot determine home directory
/// - Database initialization fails
#[tauri::command]
pub async fn initialize_database(app: AppHandle) -> Result<String, String> {
    // Attempt migration from old location
    migrate_database_if_needed(&app).await?;

    let db_path = if let Some(saved) = load_db_path_preference(&app).await? {
        saved
    } else {
        // Use unified ~/.nodespace/database/ location
        let home_dir =
            dirs::home_dir().ok_or_else(|| "Failed to get home directory".to_string())?;

        home_dir
            .join(".nodespace")
            .join("database")
            .join("nodespace.db")
    };

    // Ensure database directory exists
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create database directory: {}", e))?;
    }

    // Initialize services
    init_services(&app, db_path.clone()).await?;

    Ok(db_path.to_string_lossy().to_string())
}
