//! Database initialization and path management commands
//!
//! As of Issue #676, NodeOperations layer is removed - NodeService contains all business logic.
//! As of Issue #690, SchemaService is removed - schema operations use NodeService directly.
//! As of Issue #894, services are registered via AppServices container for hot-swappable DB.

use crate::app_services::{AppServices, EmbeddingState};
use nodespace_core::services::{EmbeddingProcessor, NodeEmbeddingService};
use nodespace_core::{NodeService, SurrealStore};
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};
use tokio::fs;

use crate::constants::EMBEDDING_MODEL_FILENAME;

/// Result of creating core services (store, node service, optional embeddings).
///
/// Extracted as a shared helper so both `init_services()` and
/// `switch_database_services()` use identical tiered-init logic.
pub(crate) struct ServiceBundle {
    pub store: Arc<SurrealStore>,
    pub node_service: Arc<NodeService>,
    pub embedding_state: Option<EmbeddingState>,
}

/// Create core services for a given database + model path.
///
/// Tiered initialization:
/// - Store and NodeService failures are fatal (returned as `Err`).
/// - NLP/embedding failures are non-fatal: `embedding_state` is set to `None`
///   and a warning is logged, allowing the app to run without semantic search.
pub(crate) async fn create_service_bundle(
    db_path: PathBuf,
    model_path: PathBuf,
) -> Result<ServiceBundle, String> {
    // Initialize SurrealDB store
    tracing::info!("Initializing SurrealDB store at {:?}...", db_path);
    let mut store = Arc::new(SurrealStore::new(db_path).await.map_err(|e| {
        let msg = format!("Failed to initialize database: {}", e);
        eprintln!("❌ {}", msg);
        msg
    })?);
    tracing::info!("SurrealDB store initialized");

    // Initialize node service
    tracing::info!("Initializing NodeService...");
    let mut node_service = NodeService::new(&mut store)
        .await
        .map_err(|e| format!("Failed to initialize node service: {}", e))?;
    tracing::info!("NodeService initialized");

    // Tiered NLP init: failure here is non-fatal
    tracing::info!("Initializing NLP engine (model: {:?})...", model_path);
    let embedding_state = match create_embedding_state(&store, &mut node_service, &model_path) {
        Ok(state) => {
            tracing::info!("Embedding services initialized");
            Some(state)
        }
        Err(e) => {
            tracing::warn!(
                "NLP/embedding init failed (semantic search disabled): {}",
                e
            );
            None
        }
    };

    let node_service_arc = Arc::new(node_service);

    Ok(ServiceBundle {
        store,
        node_service: node_service_arc,
        embedding_state,
    })
}

/// Attempt to create embedding state (NLP engine + embedding service + processor).
///
/// Separated so the caller can treat the entire block as fallible without
/// cluttering the happy path with nested matches.
fn create_embedding_state(
    store: &Arc<SurrealStore>,
    node_service: &mut NodeService,
    model_path: &Path,
) -> Result<EmbeddingState, String> {
    let embedding_config = EmbeddingConfig {
        model_path: Some(model_path.to_path_buf()),
        ..Default::default()
    };

    let mut nlp_engine = EmbeddingService::new(embedding_config)
        .map_err(|e| format!("Failed to create NLP engine: {}", e))?;

    nlp_engine
        .initialize()
        .map_err(|e| format!("Failed to load NLP model: {}", e))?;

    let nlp_engine_arc = Arc::new(nlp_engine);
    let embedding_service = Arc::new(NodeEmbeddingService::new(nlp_engine_arc, store.clone()));

    let processor = EmbeddingProcessor::new(embedding_service.clone())
        .map_err(|e| format!("Failed to init embedding processor: {}", e))?;

    node_service.set_embedding_waker(processor.waker());
    processor.wake();
    tracing::info!("EmbeddingProcessor waker connected and woken for stale embeddings");

    Ok(EmbeddingState {
        service: embedding_service,
        processor: Arc::new(processor),
    })
}

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
///
/// Uses tiered init: NLP failure is non-fatal (embedding_state = None).
async fn init_services(app: &AppHandle, config: &crate::config::AppConfig) -> Result<(), String> {
    eprintln!("🔧 [init_services] Starting service initialization...");
    tracing::info!("Starting service initialization...");

    let client_id = config.tauri_client_id.clone();

    // Check if already initialized via AppServices
    let services: tauri::State<AppServices> = app.state();
    if services.is_initialized().await {
        eprintln!("⚠️  [init_services] Database already initialized");
        return Err(
            "Database already initialized. Use switch_database for hot-swapping.".to_string(),
        );
    }

    // Create core services via shared helper (tiered NLP init)
    let bundle =
        create_service_bundle(config.database_path.clone(), config.model_path.clone()).await?;

    // Retrieve the shutdown token for background task coordination
    let shutdown_token: tauri::State<crate::ShutdownToken> = app.state();
    let session_token = shutdown_token.child_token();

    // Extract embedding service Arc for MCP (before moving bundle.embedding_state)
    let embedding_service_arc = bundle.embedding_state.as_ref().map(|es| es.service.clone());

    // Populate AppServices container (Issue #894)
    tracing::info!("Populating AppServices container...");
    services
        .initialize(
            bundle.store.clone(),
            bundle.node_service.clone(),
            bundle.embedding_state,
            config.clone(),
            session_token.clone(),
        )
        .await;
    tracing::info!("AppServices container populated");

    // Initialize MCP server — starts even without embeddings (node CRUD still works)
    if let Err(e) = crate::initialize_mcp_server(
        app.clone(),
        bundle.node_service.clone(),
        embedding_service_arc,
        session_token.clone(),
    ) {
        tracing::error!("Failed to initialize MCP server: {}", e);
        // Don't fail database init if MCP fails - MCP is optional
    }

    // Initialize domain event forwarding with client filtering (#665)
    if let Err(e) = crate::initialize_domain_event_forwarder(
        app.clone(),
        bundle.node_service.clone(),
        client_id,
        session_token.clone(),
    ) {
        tracing::error!("Failed to initialize domain event forwarder: {}", e);
    }

    tracing::info!("Service initialization complete");
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
