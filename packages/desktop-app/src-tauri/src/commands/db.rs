//! Database initialization and path management commands

use crate::commands::embeddings::EmbeddingState;
use nodespace_core::operations::NodeOperations;
use nodespace_core::services::{EmbeddingProcessor, NodeEmbeddingService, SchemaService};
use nodespace_core::{NodeService, SurrealStore};
use nodespace_nlp_engine::EmbeddingService;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tokio::fs;

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
    eprintln!("🔧 [init_services] Starting service initialization...");
    tracing::info!("🔧 [init_services] Starting service initialization...");

    // Check if state already exists to prevent reinitialization
    if app.try_state::<SurrealStore>().is_some() {
        eprintln!("⚠️  [init_services] Database already initialized");
        return Err(
            "Database already initialized. Restart the app to change location.".to_string(),
        );
    }

    // Initialize SurrealDB store
    eprintln!("🔧 [init_services] Initializing SurrealDB store...");
    tracing::info!("🔧 [init_services] Initializing SurrealDB store...");
    let store = Arc::new(SurrealStore::new(db_path).await.map_err(|e| {
        let msg = format!("Failed to initialize database: {}", e);
        eprintln!("❌ [init_services] {}", msg);
        msg
    })?);
    eprintln!("✅ [init_services] SurrealDB store initialized");
    tracing::info!("✅ [init_services] SurrealDB store initialized");

    // Initialize node service with SurrealStore
    tracing::info!("🔧 [init_services] Initializing NodeService...");
    let node_service = NodeService::new(store.clone())
        .map_err(|e| format!("Failed to initialize node service: {}", e))?;

    let node_service_arc = Arc::new(node_service);
    tracing::info!("✅ [init_services] NodeService initialized");

    // Initialize NodeOperations business logic layer (wraps NodeService)
    let node_operations = NodeOperations::new(node_service_arc.clone());

    // Initialize schema service (wraps NodeService for schema operations)
    let schema_service = SchemaService::new(node_service_arc.clone());

    // Initialize NLP engine for embeddings
    tracing::info!("🔧 [init_services] Initializing NLP engine...");
    let mut nlp_engine = EmbeddingService::new(Default::default())
        .map_err(|e| format!("Failed to initialize NLP engine: {}", e))?;

    // Initialize the NLP engine (loads model)
    nlp_engine
        .initialize()
        .map_err(|e| format!("Failed to load NLP model: {}", e))?;

    let nlp_engine_arc = Arc::new(nlp_engine);
    tracing::info!("✅ [init_services] NLP engine initialized");

    // Initialize embedding service with SurrealStore
    let embedding_service = NodeEmbeddingService::new(nlp_engine_arc.clone(), store.clone());
    let embedding_service_arc = Arc::new(embedding_service);

    // Initialize background embedding processor
    let processor = EmbeddingProcessor::new(embedding_service_arc.clone())
        .map_err(|e| format!("Failed to initialize embedding processor: {}", e))?;
    let processor_arc = Arc::new(processor);

    // Manage all services
    eprintln!("🔧 [init_services] Registering services with Tauri app.manage()...");
    tracing::info!("🔧 [init_services] Registering services with Tauri app.manage()...");
    app.manage(store.clone());
    app.manage(node_service_arc.as_ref().clone());
    app.manage(node_operations);
    app.manage(schema_service);
    app.manage(EmbeddingState {
        service: embedding_service_arc,
        processor: processor_arc.clone(),
    });
    app.manage(processor_arc);
    eprintln!("✅ [init_services] All services registered with Tauri");
    tracing::info!("✅ [init_services] All services registered with Tauri");

    // Initialize MCP server now that NodeService is available
    // MCP will use the same NodeService as Tauri commands
    if let Err(e) = crate::initialize_mcp_server(app.clone()) {
        tracing::error!("❌ Failed to initialize MCP server: {}", e);
        // Don't fail database init if MCP fails - MCP is optional
    }

    // DISABLED: Domain event forwarding causes feedback loops with optimistic UI updates
    // The current architecture emits events from SurrealStore (data layer) without
    // source_client_id tracking, so Tauri receives its own events back and corrupts state.
    // See GitHub issue for rearchitecture plan: events should be emitted at NodeService
    // layer with client_id awareness so subscribers can filter their own events.
    // TODO: Re-enable after implementing source_client_id filtering (#665)
    //
    // if let Err(e) = crate::initialize_domain_event_forwarder(app.clone(), store.clone()) {
    //     tracing::error!("❌ Failed to initialize domain event forwarder: {}", e);
    // }
    let _ = store; // Silence unused warning until re-enabled

    tracing::info!("✅ [init_services] Service initialization complete");
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

    let db_path = folder_path.join("nodespace");

    // Save preference
    let mut prefs = crate::preferences::load_preferences(&app).await?;
    prefs.database_path = Some(db_path.clone());
    crate::preferences::save_preferences(&app, &prefs).await?;

    // Initialize services
    init_services(&app, db_path.clone()).await?;

    Ok(db_path.to_string_lossy().to_string())
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
    crate::preferences::migrate_legacy_database_if_needed(&app).await?;

    // Load preferences
    let prefs = crate::preferences::load_preferences(&app).await?;

    // Determine database path
    let db_path = if let Some(saved_path) = prefs.database_path {
        saved_path
    } else {
        crate::preferences::get_default_database_path()?
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
