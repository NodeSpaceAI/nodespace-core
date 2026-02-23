//! Database initialization and path management commands
//!
//! As of Issue #676, NodeOperations layer is removed - NodeService contains all business logic.
//! As of Issue #690, SchemaService is removed - schema operations use NodeService directly.
//! As of Issue #894, services are registered via AppServices container for hot-swappable DB.

use crate::app_services::{AppServices, EmbeddingState};
use nodespace_core::services::{EmbeddingProcessor, NodeEmbeddingService};
use nodespace_core::{NodeService, SurrealStore};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};
use tokio::fs;

use crate::constants::EMBEDDING_MODEL_FILENAME;

/// Resolve the path to the bundled NLP model (GGUF format for llama.cpp)
///
/// Checks multiple locations in order:
/// 1. Bundled resources (for production builds)
/// 2. User's ~/.nodespace/models/ directory (fallback for dev)
fn resolve_bundled_model_path(app: &AppHandle) -> Result<PathBuf, String> {
    // Try bundled resources first (production builds)
    if let Ok(resource_path) = app.path().resolve(
        format!("resources/models/{}", EMBEDDING_MODEL_FILENAME),
        BaseDirectory::Resource,
    ) {
        if resource_path.exists() {
            tracing::info!("Found bundled model at: {:?}", resource_path);
            return Ok(resource_path);
        }
    }

    // Try ~/.nodespace/models/ fallback (development or user-installed)
    if let Some(home_dir) = dirs::home_dir() {
        let user_model_path = home_dir
            .join(".nodespace")
            .join("models")
            .join(EMBEDDING_MODEL_FILENAME);
        if user_model_path.exists() {
            tracing::info!("Found user model at: {:?}", user_model_path);
            return Ok(user_model_path);
        }
    }

    Err(format!(
        "Model file not found. Please download {} to ~/.nodespace/models/",
        EMBEDDING_MODEL_FILENAME
    ))
}

/// Initialize database services and populate AppServices container.
///
/// Reads database path, model path, and client ID from AppConfig.
/// Populates AppServices with store, node_service, and embedding state.
/// Starts background tasks (MCP server, domain event forwarder).
async fn init_services(app: &AppHandle, config: &crate::config::AppConfig) -> Result<(), String> {
    eprintln!("🔧 [init_services] Starting service initialization...");
    tracing::info!("🔧 [init_services] Starting service initialization...");

    let db_path = config.database_path.clone();
    let model_path = config.model_path.clone();
    let client_id = config.tauri_client_id.clone();

    // Check if already initialized via AppServices
    let services: tauri::State<AppServices> = app.state();
    if services.is_initialized().await {
        eprintln!("⚠️  [init_services] Database already initialized");
        return Err(
            "Database already initialized. Use switch_database for hot-swapping.".to_string(),
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
    tracing::info!("🔧 [init_services] Initializing NodeService...");
    let mut node_service = NodeService::new(&mut store)
        .await
        .map_err(|e| format!("Failed to initialize node service: {}", e))?;
    tracing::info!("✅ [init_services] NodeService initialized");

    // Initialize NLP engine for embeddings
    tracing::info!("🔧 [init_services] Initializing NLP engine...");
    tracing::info!("🔧 [init_services] Using model path: {:?}", model_path);

    let embedding_config = EmbeddingConfig {
        model_path: Some(model_path),
        ..Default::default()
    };

    let mut nlp_engine = EmbeddingService::new(embedding_config)
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
    node_service.set_embedding_waker(processor.waker());
    tracing::info!("✅ [init_services] EmbeddingProcessor waker connected to NodeService");

    // Wake processor on startup to process any existing stale embeddings
    processor.wake();
    tracing::info!("🔔 [init_services] EmbeddingProcessor woken to process stale embeddings");

    let node_service_arc = Arc::new(node_service);
    let processor_arc = Arc::new(processor);

    // Retrieve the shutdown token for background task coordination
    let shutdown_token: tauri::State<crate::ShutdownToken> = app.state();
    let session_token = shutdown_token.child_token();

    // Populate AppServices container (Issue #894)
    eprintln!("🔧 [init_services] Populating AppServices container...");
    tracing::info!("🔧 [init_services] Populating AppServices container...");
    services
        .initialize(
            store.clone(),
            node_service_arc.clone(),
            Some(EmbeddingState {
                service: embedding_service_arc.clone(),
                processor: processor_arc.clone(),
            }),
            config.clone(),
            session_token.clone(),
        )
        .await;
    eprintln!("✅ [init_services] AppServices container populated");
    tracing::info!("✅ [init_services] AppServices container populated");

    // Initialize MCP server now that NodeService is available
    // Pass services directly instead of reading from Tauri state
    if let Err(e) = crate::initialize_mcp_server(
        app.clone(),
        node_service_arc.clone(),
        embedding_service_arc.clone(),
        session_token.clone(),
    ) {
        tracing::error!("❌ Failed to initialize MCP server: {}", e);
        // Don't fail database init if MCP fails - MCP is optional
    }

    // Initialize domain event forwarding with client filtering (#665)
    if let Err(e) = crate::initialize_domain_event_forwarder(
        app.clone(),
        node_service_arc.clone(),
        client_id,
        session_token.clone(),
    ) {
        tracing::error!("❌ Failed to initialize domain event forwarder: {}", e);
    }

    let _ = store; // Store still available for direct access if needed

    tracing::info!("✅ [init_services] Service initialization complete");
    Ok(())
}

/// Initialize database with saved preference or default path
///
/// Checks for previously saved database location preference. If found,
/// uses that path. Otherwise, uses unified ~/.nodespace/database/ location
/// across all platforms.
#[tauri::command]
pub async fn initialize_database(app: AppHandle) -> Result<String, String> {
    // Attempt migration from old location
    crate::preferences::migrate_legacy_database_if_needed(&app).await?;

    // Load preferences
    let prefs = crate::preferences::load_preferences(&app).await?;

    // Determine database path (needed for directory creation)
    let db_path = match &prefs.database_path {
        Some(p) => p.clone(),
        None => crate::preferences::get_default_database_path()?,
    };

    // Ensure database directory exists
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create database directory: {}", e))?;
    }

    // Resolve model path
    let model_path = resolve_bundled_model_path(&app)?;

    // Determine MCP port
    let mcp_port = std::env::var("MCP_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3100);

    // Build AppConfig
    let config = crate::config::AppConfig::from_preferences(&prefs, model_path, mcp_port)?;

    // Show database path on startup
    let db_path_str = db_path.to_string_lossy().to_string();
    eprintln!("📂 Database path: {}", db_path_str);

    // Initialize services (populates AppServices container)
    init_services(&app, &config).await?;

    Ok(db_path_str)
}
