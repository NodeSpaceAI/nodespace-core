// Tauri commands module (public for dev-server access)
pub mod commands;

// Application preferences management
pub mod preferences;

// Shared constants
pub mod constants;

// Runtime application configuration
pub mod config;

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

/// Initialize domain event forwarding service for real-time frontend synchronization
///
/// Spawns background tasks that subscribe to domain events from NodeService.
/// When business logic emits domain events (node/edge created/updated/deleted),
/// they are forwarded to the frontend via Tauri events to trigger UI updates,
/// achieving real-time sync through event-driven architecture.
///
/// Events that originated from this Tauri client are filtered out (prevents feedback loop).
///
/// The `cancel_token` is used for graceful shutdown - when cancelled, the forwarder
/// will stop its event loop and exit cleanly before the Tokio runtime drops.
pub fn initialize_domain_event_forwarder(
    app: tauri::AppHandle,
    node_service: std::sync::Arc<nodespace_core::NodeService>,
    client_id: String,
    cancel_token: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    use crate::services::DomainEventForwarder;
    use futures::FutureExt;

    tracing::info!(
        "🔧 Initializing domain event forwarding service (client_id: {})...",
        client_id
    );

    // Spawn domain event forwarding service background task
    tauri::async_runtime::spawn(async move {
        let result = std::panic::AssertUnwindSafe(async {
            let forwarder = DomainEventForwarder::new(node_service, app, client_id);
            forwarder.run(cancel_token).await
        })
        .catch_unwind()
        .await;

        match result {
            Ok(Ok(_)) => {
                tracing::info!("✅ Domain event forwarding service exited normally");
            }
            Ok(Err(e)) => {
                tracing::error!("❌ Domain event forwarding error: {}", e);
            }
            Err(panic_info) => {
                tracing::error!(
                    "💥 Domain event forwarding service panicked: {:?}",
                    panic_info
                );
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
///
/// The `cancel_token` is used for graceful shutdown - when cancelled, the MCP
/// server task will be aborted before the Tokio runtime drops.
///
/// As of Issue #715, uses McpServerService from nodespace-core for managed lifecycle.
pub fn initialize_mcp_server(
    app: tauri::AppHandle,
    cancel_token: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    use crate::commands::embeddings::EmbeddingState;
    use futures::FutureExt;
    use nodespace_core::NodeService;
    use std::sync::Arc;
    use tauri::Manager;

    tracing::info!("🔧 Initializing MCP server service...");

    // Get shared services from Tauri state
    // This ensures MCP uses the same database and embedding service as Tauri commands
    let node_service: tauri::State<NodeService> = app.state();
    let node_service_arc = Arc::new(node_service.inner().clone());

    let embedding_state: tauri::State<EmbeddingState> = app.state();
    let embedding_service_arc = embedding_state.service.clone();

    // Create MCP service with Tauri event callback
    let (mcp_service, callback) = mcp_integration::create_mcp_service_with_events(
        node_service_arc,
        embedding_service_arc,
        app.clone(),
    );

    tracing::info!(
        "✅ McpServerService created on port {}, spawning background task...",
        mcp_service.port()
    );

    // Register MCP service as managed state for potential future access
    app.manage(mcp_service.clone());

    // Spawn MCP server task with Tauri event emissions
    // Uses panic protection to prevent silent background task failures
    // Monitors cancel_token for graceful shutdown before runtime drops
    tauri::async_runtime::spawn(async move {
        // Use FutureExt::catch_unwind for proper async panic catching
        // This avoids the deadlock risk of using block_on inside an async task
        // AssertUnwindSafe is needed because services contain non-UnwindSafe types
        let result = std::panic::AssertUnwindSafe(async {
            tokio::select! {
                res = mcp_service.start_with_callback(callback) => res,
                _ = cancel_token.cancelled() => {
                    tracing::info!("MCP server received shutdown signal");
                    Ok(())
                }
            }
        })
        .catch_unwind()
        .await;

        match result {
            Ok(Ok(_)) => {
                tracing::info!("✅ MCP server exited normally");
            }
            Ok(Err(e)) => {
                tracing::error!("❌ MCP server error: {}", e);
            }
            Err(panic_info) => {
                tracing::error!("💥 MCP server panicked: {:?}", panic_info);
            }
        }
    });

    Ok(())
}

/// Shared shutdown token for graceful background task termination.
///
/// Managed as Tauri state so it can be accessed from both the setup phase
/// (where background tasks are spawned) and the run event handler (where
/// shutdown is triggered). When cancelled, all background tasks (MCP server,
/// domain event forwarder) exit their loops before the Tokio runtime drops.
#[derive(Clone)]
pub struct ShutdownToken(tokio_util::sync::CancellationToken);

impl ShutdownToken {
    fn new() -> Self {
        Self(tokio_util::sync::CancellationToken::new())
    }

    /// Create a child token for a background task.
    /// Cancelling the parent automatically cancels all children.
    pub fn child_token(&self) -> tokio_util::sync::CancellationToken {
        self.0.child_token()
    }

    /// Signal all background tasks to shut down.
    /// Idempotent - safe to call multiple times.
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::{menu::*, Emitter, Manager, RunEvent};

    // Create shutdown token for coordinating graceful background task termination
    let shutdown_token = ShutdownToken::new();
    let shutdown_token_for_setup = shutdown_token.clone();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            // Create menu items
            let toggle_sidebar = MenuItemBuilder::new("Toggle Sidebar")
                .id("toggle_sidebar")
                .accelerator("CmdOrCtrl+B")
                .build(app)?;

            let toggle_status_bar = MenuItemBuilder::new("Toggle Status Bar")
                .id("toggle_status_bar")
                .build(app)?;

            let quit = MenuItemBuilder::new("Quit")
                .id("quit")
                .accelerator("CmdOrCtrl+Q")
                .build(app)?;

            let import_folder = MenuItemBuilder::new("Import Folder...")
                .id("import_folder")
                .accelerator("CmdOrCtrl+Shift+I")
                .build(app)?;

            let new_database = MenuItemBuilder::new("New Database...")
                .id("new_database")
                .build(app)?;

            let open_database = MenuItemBuilder::new("Open Database...")
                .id("open_database")
                .build(app)?;

            let open_settings = MenuItemBuilder::new("Settings...")
                .id("open_settings")
                .accelerator("CmdOrCtrl+,")
                .build(app)?;

            let db_separator = PredefinedMenuItem::separator(app)?;
            let settings_separator = PredefinedMenuItem::separator(app)?;

            let import_submenu = SubmenuBuilder::new(app, "Import")
                .items(&[&import_folder])
                .build()?;

            // Standard Edit menu items for clipboard operations
            // These are required on macOS for Cmd+C/V/X to work in WebView
            let cut = PredefinedMenuItem::cut(app, Some("Cut"))?;
            let copy = PredefinedMenuItem::copy(app, Some("Copy"))?;
            let paste = PredefinedMenuItem::paste(app, Some("Paste"))?;
            let select_all = PredefinedMenuItem::select_all(app, Some("Select All"))?;
            let undo = PredefinedMenuItem::undo(app, Some("Undo"))?;
            let redo = PredefinedMenuItem::redo(app, Some("Redo"))?;

            // Create submenus
            // macOS app menu (first menu is always the app name on macOS)
            let app_menu = SubmenuBuilder::new(app, "NodeSpace")
                .items(&[&quit])
                .build()?;

            let file_menu = SubmenuBuilder::new(app, "File")
                .items(&[
                    &new_database,
                    &open_database,
                    &db_separator,
                    &import_submenu,
                    &settings_separator,
                    &open_settings,
                ])
                .build()?;

            // Edit menu with standard shortcuts (required for macOS WebView clipboard)
            let edit_menu = SubmenuBuilder::new(app, "Edit")
                .items(&[&undo, &redo, &cut, &copy, &paste, &select_all])
                .build()?;

            let view_menu = SubmenuBuilder::new(app, "View")
                .items(&[&toggle_sidebar, &toggle_status_bar])
                .build()?;

            // Create main menu
            let menu = MenuBuilder::new(app)
                .items(&[&app_menu, &file_menu, &edit_menu, &view_menu])
                .build()?;

            // Set the menu
            app.set_menu(menu)?;

            // Register shutdown token as managed state so commands/db.rs can access it
            // when spawning background tasks (MCP server, domain event forwarder)
            app.manage(shutdown_token_for_setup);

            // Note: MCP server initialization is deferred until database is initialized
            // See commands/db.rs::init_services() which calls initialize_mcp_server()
            // after NodeService is available in Tauri state

            Ok(())
        })
        .on_menu_event(|app, event| {
            let toggle_sidebar_id = MenuId::new("toggle_sidebar");
            let toggle_status_bar_id = MenuId::new("toggle_status_bar");
            let quit_id = MenuId::new("quit");
            let import_folder_id = MenuId::new("import_folder");
            let new_database_id = MenuId::new("new_database");
            let open_database_id = MenuId::new("open_database");
            let open_settings_id = MenuId::new("open_settings");

            if *event.id() == toggle_sidebar_id {
                // Emit an event to the frontend
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("menu-toggle-sidebar", ());
                    println!("Sidebar toggle requested from menu");
                }
            } else if *event.id() == toggle_status_bar_id {
                // Emit an event to the frontend to toggle status bar
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("menu-toggle-status-bar", ());
                    println!("Status bar toggle requested from menu");
                }
            } else if *event.id() == import_folder_id {
                // Emit an event to the frontend to open import dialog
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("menu-import-folder", ());
                    println!("Import folder requested from menu");
                }
            } else if *event.id() == new_database_id || *event.id() == open_database_id {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("menu-select-database", ());
                }
            } else if *event.id() == open_settings_id {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.emit("menu-open-settings", ());
                }
            } else if *event.id() == quit_id {
                // Request exit through Tauri's event loop instead of std::process::exit(0)
                // This triggers RunEvent::ExitRequested, allowing proper cleanup
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.close();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            toggle_sidebar,
            commands::db::initialize_database,
            commands::embeddings::generate_root_embedding,
            commands::embeddings::search_roots,
            commands::embeddings::update_root_embedding,
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
            commands::nodes::get_children_tree,
            commands::nodes::get_nodes_by_root_id,
            commands::nodes::query_nodes_simple,
            commands::nodes::mention_autocomplete,
            commands::nodes::save_node_with_parent,
            commands::nodes::get_outgoing_mentions,
            commands::nodes::get_incoming_mentions,
            commands::nodes::get_mentioning_roots,
            commands::nodes::delete_node_mention,
            commands::nodes::update_task_node,
            // Collection commands (Issue #757 - Collection browsing and management UI)
            commands::collections::get_all_collections,
            commands::collections::get_collection_members,
            commands::collections::get_collection_members_recursive,
            commands::collections::get_node_collections,
            commands::collections::add_node_to_collection,
            commands::collections::add_node_to_collection_path,
            commands::collections::remove_node_from_collection,
            commands::collections::find_collection_by_path,
            commands::collections::get_collection_by_name,
            commands::collections::create_collection,
            commands::collections::rename_collection,
            commands::collections::delete_collection,
            // Schema read commands (Issue #690 - mutation commands removed, not used by UI)
            commands::schemas::get_all_schemas,
            commands::schemas::get_schema_definition,
            // Diagnostic commands for debugging persistence issues
            commands::diagnostics::get_database_diagnostics,
            commands::diagnostics::test_node_persistence,
            // File import commands for bulk markdown import
            commands::import::import_markdown_file,
            commands::import::import_markdown_files,
            commands::import::import_markdown_directory,
            // Settings commands
            commands::settings::get_settings,
            commands::settings::update_display_settings,
            commands::settings::select_new_database,
            commands::settings::restart_app,
            commands::settings::reset_database_to_default,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Run with event handler for graceful shutdown
    // This allows proper cleanup of Metal/GPU resources and background tasks before exit
    //
    // Note: On macOS, RunEvent::ExitRequested may not fire reliably (Tauri issue #9198)
    // We handle cleanup in multiple places to ensure resources are released:
    // 1. WindowEvent::CloseRequested - when user clicks X or Cmd+Q
    // 2. RunEvent::ExitRequested - when app is about to exit
    // 3. RunEvent::Exit - final cleanup before process termination
    //
    // The shutdown_token is cancelled to signal background tasks (MCP server,
    // domain event forwarder) to exit their loops before the Tokio runtime drops.
    let shutdown_token_for_events = shutdown_token.clone();
    app.run(move |app_handle, event| {
        match event {
            RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { .. },
                ..
            } => {
                // Window close requested - signal background tasks and release GPU resources
                // This is the most reliable place to do cleanup on macOS
                tracing::info!(
                    "Window '{}' close requested, signaling shutdown and releasing GPU context...",
                    label
                );
                shutdown_token_for_events.cancel();
                // Brief pause to let background tasks exit their loops before
                // releasing GPU resources they may still reference
                std::thread::sleep(std::time::Duration::from_millis(50));
                release_gpu_resources(app_handle);
            }
            RunEvent::ExitRequested { code, .. } => {
                // App exit requested - this may not fire on macOS (Tauri issue #9198)
                tracing::info!(
                    "App exit requested (code: {:?}), performing cleanup...",
                    code
                );
                shutdown_token_for_events.cancel();
                release_gpu_resources(app_handle);
                tracing::info!("Cleanup complete, exiting...");
            }
            RunEvent::Exit => {
                // Final exit - ensure shutdown signal is sent (idempotent)
                tracing::info!("App exiting, ensuring shutdown signal sent...");
                shutdown_token_for_events.cancel();
            }
            _ => {}
        }
    });
}

/// Release GPU resources (Metal context and backend) to prevent SIGABRT crash on exit.
///
/// This must be called before the app exits to properly clean up Metal/GPU
/// resources. The crash occurs when ggml_metal_rsets_free is called during
/// static destruction (__cxa_finalize_ranges) while resources are still in use.
///
/// Cleanup order is critical:
/// 1. Release LlamaState (context + model) - frees Metal residency sets
/// 2. Release global LLAMA_BACKEND - frees the Metal backend itself
///
/// This prevents __cxa_finalize_ranges from encountering Metal resources during
/// static destruction, which would cause SIGABRT in production builds.
fn release_gpu_resources(app_handle: &tauri::AppHandle) {
    use tauri::Manager;

    if let Some(embedding_state) =
        app_handle.try_state::<crate::commands::embeddings::EmbeddingState>()
    {
        tracing::info!("Releasing GPU context to prevent Metal crash...");
        // Step 1: Release the LlamaState (context + model) which holds Metal residency sets
        embedding_state.service.nlp_engine().release_gpu_context();
        tracing::info!("GPU context released successfully");
    }

    // Step 2: Release the global llama backend itself
    // Must happen AFTER all models/contexts are dropped (step 1)
    nodespace_nlp_engine::release_llama_backend();
}
