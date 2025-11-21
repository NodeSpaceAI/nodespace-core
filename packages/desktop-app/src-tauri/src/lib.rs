// Tauri commands module (public for dev-server access)
pub mod commands;

// Application preferences management
pub mod preferences;

// Shared constants
pub mod constants;

// MCP Tauri integration (wraps core MCP with event emissions)
pub mod mcp_integration;

// Background services
pub mod services;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn toggle_sidebar() -> String {
    "Sidebar toggled!".to_string()
}

// Include test module
#[cfg(test)]
mod tests;

/// Initialize LIVE SELECT service for real-time database synchronization
///
/// Spawns background tasks that subscribe to SurrealDB LIVE SELECT queries
/// for node and edge tables. When database records change, Tauri events are
/// emitted to the frontend to trigger UI updates, achieving real-time sync
/// without polling or manual cache management.
pub fn initialize_live_query_service(
    app: tauri::AppHandle,
    store: std::sync::Arc<nodespace_core::SurrealStore>,
) -> anyhow::Result<()> {
    use crate::services::LiveQueryService;
    use futures::FutureExt;

    tracing::info!("🔧 Initializing LIVE SELECT service...");

    // Spawn live query service background tasks
    tauri::async_runtime::spawn(async move {
        let result = std::panic::AssertUnwindSafe(async {
            let live_query = LiveQueryService::new(store, app);
            live_query.run().await
        })
        .catch_unwind()
        .await;

        match result {
            Ok(Ok(_)) => {
                tracing::info!("✅ LIVE SELECT service exited normally");
            }
            Ok(Err(e)) => {
                tracing::error!("❌ LIVE SELECT error: {}", e);
            }
            Err(panic_info) => {
                tracing::error!("💥 LIVE SELECT service panicked: {:?}", panic_info);
            }
        }
    });

    Ok(())
}

/// Initialize MCP server with shared services from Tauri state
///
/// This must be called AFTER the database is initialized and services
/// are available in Tauri's managed state. It retrieves the shared NodeService
/// and NodeEmbeddingService and spawns the MCP server task with them,
/// ensuring MCP and Tauri commands operate on the same database.
pub fn initialize_mcp_server(app: tauri::AppHandle) -> anyhow::Result<()> {
    use crate::commands::embeddings::EmbeddingState;
    use futures::FutureExt;
    use nodespace_core::NodeService;
    use std::sync::Arc;
    use tauri::Manager;

    tracing::info!("🔧 Initializing MCP server...");

    // Get shared services from Tauri state
    // This ensures MCP uses the same database and embedding service as Tauri commands
    let node_service: tauri::State<NodeService> = app.state();
    let node_service_arc = Arc::new(node_service.inner().clone());

    let embedding_state: tauri::State<EmbeddingState> = app.state();
    let embedding_service_arc = embedding_state.service.clone();

    tracing::info!(
        "✅ Using shared NodeService and NodeEmbeddingService, spawning MCP stdio task..."
    );

    // Spawn MCP stdio server task with Tauri event emissions
    // Uses panic protection to prevent silent background task failures
    tauri::async_runtime::spawn(async move {
        // Use FutureExt::catch_unwind for proper async panic catching
        // This avoids the deadlock risk of using block_on inside an async task
        // AssertUnwindSafe is needed because services contain non-UnwindSafe types
        let result = std::panic::AssertUnwindSafe(mcp_integration::run_mcp_server_with_events(
            node_service_arc,
            embedding_service_arc,
            app,
        ))
        .catch_unwind()
        .await;

        match result {
            Ok(Ok(_)) => {
                tracing::info!("✅ MCP server exited normally (stdin closed)");
            }
            Ok(Err(e)) => {
                tracing::error!("❌ MCP server error: {}", e);
                // TODO: Consider emitting Tauri event to notify UI of MCP failure
            }
            Err(panic_info) => {
                tracing::error!("💥 MCP server panicked: {:?}", panic_info);
                // TODO: Consider attempting automatic restart or notifying user
            }
        }
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::{menu::*, Emitter, Manager};

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Create menu items
            let toggle_sidebar = MenuItemBuilder::new("Toggle Sidebar")
                .id("toggle_sidebar")
                .accelerator("CmdOrCtrl+B")
                .build(app)?;

            let quit = MenuItemBuilder::new("Quit")
                .id("quit")
                .accelerator("CmdOrCtrl+Q")
                .build(app)?;

            // Create submenus
            let view_menu = SubmenuBuilder::new(app, "View")
                .items(&[&toggle_sidebar])
                .build()?;

            let file_menu = SubmenuBuilder::new(app, "File").items(&[&quit]).build()?;

            // Create main menu
            let menu = MenuBuilder::new(app)
                .items(&[&file_menu, &view_menu])
                .build()?;

            // Set the menu
            app.set_menu(menu)?;

            // Note: MCP server initialization is deferred until database is initialized
            // See commands/db.rs::init_services() which calls initialize_mcp_server()
            // after NodeService is available in Tauri state

            Ok(())
        })
        .on_menu_event(|app, event| {
            let toggle_sidebar_id = MenuId::new("toggle_sidebar");
            let quit_id = MenuId::new("quit");

            if *event.id() == toggle_sidebar_id {
                // Emit an event to the frontend
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("menu-toggle-sidebar", ());
                    println!("Sidebar toggle requested from menu");
                }
            } else if *event.id() == quit_id {
                std::process::exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            toggle_sidebar,
            commands::db::initialize_database,
            commands::db::select_db_location,
            commands::embeddings::generate_root_embedding,
            commands::embeddings::search_roots,
            commands::embeddings::update_root_embedding,
            #[allow(deprecated)]
            commands::embeddings::schedule_root_embedding_update,
            commands::embeddings::batch_generate_embeddings,
            commands::embeddings::on_root_closed,
            commands::embeddings::on_root_idle,
            commands::embeddings::sync_embeddings,
            commands::embeddings::get_stale_root_count,
            commands::models::ensure_models_installed,
            commands::nodes::create_node,
            commands::nodes::create_root_node,
            commands::nodes::create_node_mention,
            commands::nodes::get_node,
            commands::nodes::update_node,
            commands::nodes::move_node,
            commands::nodes::reorder_node,
            commands::nodes::delete_node,
            commands::nodes::get_children,
            commands::nodes::get_nodes_by_root_id,
            commands::nodes::query_nodes_simple,
            commands::nodes::mention_autocomplete,
            commands::nodes::save_node_with_parent,
            commands::nodes::get_outgoing_mentions,
            commands::nodes::get_incoming_mentions,
            commands::nodes::get_mentioning_roots,
            commands::nodes::delete_node_mention,
            commands::nodes::get_all_edges,
            commands::schemas::get_all_schemas,
            commands::schemas::get_schema_definition,
            commands::schemas::add_schema_field,
            commands::schemas::remove_schema_field,
            commands::schemas::extend_schema_enum,
            commands::schemas::remove_schema_enum_value,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
