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

/// Initialize MCP server with database and NodeService
async fn initialize_mcp_server(app: tauri::AppHandle) -> anyhow::Result<()> {
    use nodespace_core::{DatabaseService, NodeService};
    use std::path::PathBuf;
    use std::sync::Arc;

    tracing::info!("🔧 Initializing MCP server...");

    // Determine database path (use dev-specific database)
    let home_dir =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Failed to get home directory"))?;

    let db_path: PathBuf = home_dir
        .join(".nodespace")
        .join("database")
        .join("nodespace-dev.db");

    // Ensure database directory exists
    if let Some(parent) = db_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tracing::info!("📦 MCP using database: {}", db_path.display());

    // Initialize services (same pattern as dev-server.rs)
    let db_service = DatabaseService::new(db_path.clone()).await?;
    let node_service = Arc::new(NodeService::new(db_service)?);

    tracing::info!("✅ Services initialized, spawning MCP stdio task...");

    // Spawn MCP stdio server task with Tauri event emissions
    tauri::async_runtime::spawn(async move {
        if let Err(e) = mcp_integration::run_mcp_server_with_events(node_service, app).await {
            tracing::error!("❌ MCP server error: {}", e);
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

            // Initialize database and services for MCP
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match initialize_mcp_server(app_handle).await {
                    Ok(_) => tracing::info!("✅ MCP server initialized successfully"),
                    Err(e) => tracing::error!("❌ Failed to initialize MCP server: {}", e),
                }
            });

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
