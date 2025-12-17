//! Database initialization and path management commands
//!
//! As of Issue #676, NodeOperations layer is removed - NodeService contains all business logic.
//! As of Issue #690, SchemaService is removed - schema operations use NodeService directly.

use crate::commands::embeddings::EmbeddingState;
use nodespace_core::services::{EmbeddingProcessor, NodeEmbeddingService};
use nodespace_core::{NodeService, SurrealStore};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};
use tokio::fs;

/// Resolve the path to the bundled NLP model
///
/// Checks multiple locations in order:
/// 1. Bundled resources (for production builds)
/// 2. User's ~/.nodespace/models/ directory (fallback for dev)
///
/// # Arguments
/// * `app` - Tauri application handle for resource resolution
///
/// # Returns
/// * `Ok(PathBuf)` - Path to the model directory
/// * `Err(String)` - Error if model not found anywhere
fn resolve_bundled_model_path(app: &AppHandle) -> Result<PathBuf, String> {
    let model_name = "BAAI-bge-small-en-v1.5";

    // Try bundled resources first (production builds)
    if let Ok(resource_path) = app.path().resolve(
        format!("resources/models/{}", model_name),
        BaseDirectory::Resource,
    ) {
        if resource_path.exists() {
            tracing::info!("Found bundled model at: {:?}", resource_path);
            return Ok(resource_path);
        }
    }

    // Try ~/.nodespace/models/ fallback (development or user-installed)
    if let Some(home_dir) = dirs::home_dir() {
        let user_model_path = home_dir.join(".nodespace").join("models").join(model_name);
        if user_model_path.exists() {
            tracing::info!("Found user model at: {:?}", user_model_path);
            return Ok(user_model_path);
        }
    }

    Err(format!(
        "Model file not found at path: Model not found at {:?}. Please install model to ~/.nodespace/models/",
        format!("~/.nodespace/models/{}", model_name)
    ))
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
    let mut store = Arc::new(SurrealStore::new(db_path).await.map_err(|e| {
        let msg = format!("Failed to initialize database: {}", e);
        eprintln!("❌ [init_services] {}", msg);
        msg
    })?);
    eprintln!("✅ [init_services] SurrealDB store initialized");
    tracing::info!("✅ [init_services] SurrealDB store initialized");

    // Initialize node service with SurrealStore
    // NodeService::new() takes &mut Arc to enable cache updates during seeding (Issue #704)
    tracing::info!("🔧 [init_services] Initializing NodeService...");
    let mut node_service = NodeService::new(&mut store)
        .await
        .map_err(|e| format!("Failed to initialize node service: {}", e))?;
    tracing::info!("✅ [init_services] NodeService initialized");

    // NOTE: NodeOperations layer removed (Issue #676) - NodeService contains all business logic
    // NOTE: SchemaService removed (Issue #690) - schema operations use NodeService directly

    // Initialize NLP engine for embeddings
    // First, try to find the bundled model in Tauri resources
    tracing::info!("🔧 [init_services] Initializing NLP engine...");

    let model_path = resolve_bundled_model_path(app)?;
    tracing::info!("🔧 [init_services] Using model path: {:?}", model_path);

    let config = EmbeddingConfig {
        model_path: Some(model_path),
        ..Default::default()
    };

    let mut nlp_engine = EmbeddingService::new(config)
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

    // Initialize background embedding processor (event-driven, Issue #729)
    let processor = EmbeddingProcessor::new(embedding_service_arc.clone())
        .map_err(|e| format!("Failed to initialize embedding processor: {}", e))?;

    // Wire up NodeService to wake processor on embedding changes (Issue #729)
    // This enables event-driven embedding processing without polling
    node_service.set_embedding_waker(processor.waker());
    tracing::info!("✅ [init_services] EmbeddingProcessor waker connected to NodeService");

    // Wake processor on startup to process any existing stale embeddings
    // This handles cases where stale markers exist from previous sessions
    processor.wake();
    tracing::info!("🔔 [init_services] EmbeddingProcessor woken to process stale embeddings");

    let node_service_arc = Arc::new(node_service);
    let processor_arc = Arc::new(processor);

    // Manage all services
    eprintln!("🔧 [init_services] Registering services with Tauri app.manage()...");
    tracing::info!("🔧 [init_services] Registering services with Tauri app.manage()...");
    app.manage(store.clone());
    app.manage(node_service_arc.as_ref().clone());
    // NOTE: NodeOperations removed (Issue #676) - commands use NodeService directly
    // NOTE: SchemaService removed (Issue #690) - schema commands use NodeService directly
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

    // Initialize domain event forwarding with client filtering (#665)
    // Events that originated from this Tauri client are filtered out to prevent feedback loops
    let client_id = "tauri-main".to_string();
    if let Err(e) =
        crate::initialize_domain_event_forwarder(app.clone(), node_service_arc.clone(), client_id)
    {
        tracing::error!("❌ Failed to initialize domain event forwarder: {}", e);
        // Don't fail database init if event forwarding fails - it's not critical
    }

    let _ = store; // Store still available for direct access if needed

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
