//! Settings commands for reading and updating app preferences
//!
//! These commands expose the preferences system to the frontend.
//! Display settings (theme, markdown rendering) take effect immediately.
//! Database settings now hot-swap services without requiring a restart.

use crate::app_services::AppServices;
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

/// Result of a database switch operation
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseSwitchResult {
    pub new_path: String,
    pub success: bool,
}

/// Get current app settings for the Settings UI
#[tauri::command]
pub async fn get_settings(
    app: AppHandle,
    services: tauri::State<'_, AppServices>,
) -> Result<SettingsResponse, String> {
    let prefs = crate::preferences::load_preferences(&app).await?;
    let config = services.config().await.map_err(|e| e.message)?;

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

/// Open native folder picker, save chosen database path, and hot-swap services.
#[tauri::command]
pub async fn select_new_database(
    app: tauri::AppHandle,
    services: tauri::State<'_, AppServices>,
) -> Result<DatabaseSwitchResult, String> {
    use tauri::Emitter;
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

    // Hot-swap database services
    switch_database_services(&app, &services, db_path.clone()).await?;

    // Emit database-changed event to frontend
    let path_str = db_path.to_string_lossy().to_string();
    let _ = app.emit("database-changed", &path_str);

    Ok(DatabaseSwitchResult {
        new_path: path_str,
        success: true,
    })
}

/// Restart the application with graceful GPU/background task shutdown.
///
/// Without explicit cleanup, `app.restart()` calls `std::process::exit()` which
/// triggers C++ destructors via `__cxa_finalize_ranges`. The Metal residency sets
/// for the embedding model are still active, causing a SIGABRT assertion failure
/// in `ggml_metal_rsets_free`.
#[tauri::command]
pub fn restart_app(app: tauri::AppHandle) {
    tracing::info!("Restart requested, performing graceful shutdown...");
    crate::graceful_shutdown(&app);
    tracing::info!("Graceful shutdown complete, restarting app...");
    app.restart();
}

/// Reset database path to default and hot-swap to the default database.
#[tauri::command]
pub async fn reset_database_to_default(
    app: tauri::AppHandle,
    services: tauri::State<'_, AppServices>,
) -> Result<String, String> {
    use tauri::Emitter;

    let mut prefs = crate::preferences::load_preferences(&app).await?;
    prefs.database_path = None;
    crate::preferences::save_preferences(&app, &prefs).await?;

    let default_path = crate::preferences::get_default_database_path()?;

    // Hot-swap to default database
    switch_database_services(&app, &services, default_path.clone()).await?;

    let path_str = default_path.to_string_lossy().to_string();
    let _ = app.emit("database-changed", &path_str);

    Ok(path_str)
}

/// Hot-swap database services: create new store, node service, and embeddings,
/// then atomically replace the running services and restart background tasks.
async fn switch_database_services(
    app: &AppHandle,
    services: &AppServices,
    new_db_path: std::path::PathBuf,
) -> Result<(), String> {
    use crate::app_services::EmbeddingState;
    use nodespace_core::services::{EmbeddingProcessor, NodeEmbeddingService};
    use nodespace_core::{NodeService, SurrealStore};
    use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};
    use std::sync::Arc;

    // Get current config for model path etc.
    let old_config = services.config().await.map_err(|e| e.message)?;

    tracing::info!("🔧 Switching database to: {:?}", new_db_path);

    // Ensure directory exists
    if let Some(parent) = new_db_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create database directory: {}", e))?;
    }

    // Create new store
    let mut store = Arc::new(
        SurrealStore::new(new_db_path.clone())
            .await
            .map_err(|e| format!("Failed to initialize new database: {}", e))?,
    );

    // Create new NodeService
    let mut node_service = NodeService::new(&mut store)
        .await
        .map_err(|e| format!("Failed to initialize node service: {}", e))?;

    // Create new embedding engine
    let embedding_config = EmbeddingConfig {
        model_path: Some(old_config.model_path.clone()),
        ..Default::default()
    };

    let embedding_state = match EmbeddingService::new(embedding_config) {
        Ok(mut nlp_engine) => match nlp_engine.initialize() {
            Ok(()) => {
                let nlp_arc = Arc::new(nlp_engine);
                let emb_service = NodeEmbeddingService::new(nlp_arc.clone(), store.clone());
                let emb_service_arc = Arc::new(emb_service);
                let processor = EmbeddingProcessor::new(emb_service_arc.clone())
                    .map_err(|e| format!("Failed to init embedding processor: {}", e))?;
                node_service.set_embedding_waker(processor.waker());
                processor.wake();
                let processor_arc = Arc::new(processor);
                Some(EmbeddingState {
                    service: emb_service_arc,
                    processor: processor_arc,
                })
            }
            Err(e) => {
                tracing::warn!("Failed to load NLP model during switch: {}", e);
                None
            }
        },
        Err(e) => {
            tracing::warn!("Failed to create NLP engine during switch: {}", e);
            None
        }
    };

    let node_service_arc = Arc::new(node_service);

    // Build new config
    let new_config = crate::config::AppConfig {
        database_path: new_db_path,
        model_path: old_config.model_path,
        mcp_port: old_config.mcp_port,
        tauri_client_id: old_config.tauri_client_id.clone(),
    };

    // Create new session token
    let shutdown_token: tauri::State<crate::ShutdownToken> = app.state();
    let new_session_token = shutdown_token.child_token();

    // Hot-swap services
    let embedding_service_arc = embedding_state.as_ref().map(|es| es.service.clone());
    services
        .switch_database(
            store.clone(),
            node_service_arc.clone(),
            embedding_state,
            new_config.clone(),
            new_session_token.clone(),
        )
        .await;

    // Restart background services with new session token
    if let Some(emb_svc) = embedding_service_arc {
        if let Err(e) = crate::initialize_mcp_server(
            app.clone(),
            node_service_arc.clone(),
            emb_svc,
            new_session_token.clone(),
        ) {
            tracing::error!("Failed to restart MCP server after switch: {}", e);
        }
    }

    if let Err(e) = crate::initialize_domain_event_forwarder(
        app.clone(),
        node_service_arc.clone(),
        new_config.tauri_client_id.clone(),
        new_session_token,
    ) {
        tracing::error!(
            "Failed to restart domain event forwarder after switch: {}",
            e
        );
    }

    tracing::info!("✅ Database switch complete");
    Ok(())
}
