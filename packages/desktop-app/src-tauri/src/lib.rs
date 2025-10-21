// Tauri commands module (public for dev-server access)
pub mod commands;

// Shared constants
pub mod constants;

// HTTP dev server module (feature-gated for development only)
#[cfg(feature = "dev-server")]
pub mod dev_server;

// MCP Tauri integration (wraps core MCP with event emissions)
pub mod mcp_integration;

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

/// Initialize MCP server with shared NodeService from Tauri state
///
/// This must be called AFTER the database is initialized and NodeService
/// is available in Tauri's managed state. It retrieves the shared NodeService
/// and spawns the MCP server task with it, ensuring MCP and Tauri commands
/// operate on the same database.
pub fn initialize_mcp_server(app: tauri::AppHandle) -> anyhow::Result<()> {
    use futures::FutureExt;
    use nodespace_core::NodeService;
    use std::sync::Arc;
    use tauri::Manager;

    tracing::info!("🔧 Initializing MCP server...");

    // Get shared NodeService from Tauri state
    // This ensures MCP uses the same database as Tauri commands
    let node_service: tauri::State<NodeService> = app.state();
    let node_service_arc = Arc::new(node_service.inner().clone());

    tracing::info!("✅ Using shared NodeService, spawning MCP stdio task...");

    // Spawn MCP stdio server task with Tauri event emissions
    // Uses panic protection to prevent silent background task failures
    tauri::async_runtime::spawn(async move {
        // Use FutureExt::catch_unwind for proper async panic catching
        // This avoids the deadlock risk of using block_on inside an async task
        // AssertUnwindSafe is needed because NodeService contains non-UnwindSafe types
        let result = std::panic::AssertUnwindSafe(mcp_integration::run_mcp_server_with_events(
            node_service_arc,
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
            commands::embeddings::generate_container_embedding,
            commands::embeddings::search_containers,
            commands::embeddings::update_container_embedding,
            #[allow(deprecated)]
            commands::embeddings::schedule_container_embedding_update,
            commands::embeddings::batch_generate_embeddings,
            commands::embeddings::on_container_closed,
            commands::embeddings::on_container_idle,
            commands::embeddings::sync_embeddings,
            commands::embeddings::get_stale_container_count,
            commands::models::ensure_models_installed,
            commands::nodes::create_node,
            commands::nodes::create_container_node,
            commands::nodes::create_node_mention,
            commands::nodes::get_node,
            commands::nodes::update_node,
            commands::nodes::delete_node,
            commands::nodes::get_children,
            commands::nodes::get_nodes_by_container_id,
            commands::nodes::query_nodes_simple,
            commands::nodes::save_node_with_parent,
            commands::nodes::get_outgoing_mentions,
            commands::nodes::get_incoming_mentions,
            commands::nodes::delete_node_mention,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
