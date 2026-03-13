// Tauri commands module (public for dev-server access)
pub mod commands;

// Application preferences management
pub mod preferences;

// Shared constants
pub mod constants;

// Runtime application configuration
pub mod config;

// Centralized services container (Issue #894)
pub mod app_services;

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

/// Initialize MCP server with shared services
///
/// Takes Arc<NodeService> and Arc<NodeEmbeddingService> directly rather than
/// reading from Tauri state. This supports hot-swapping via AppServices.
///
/// The `cancel_token` is used for graceful shutdown - when cancelled, the MCP
/// server task will be aborted before the Tokio runtime drops.
pub fn initialize_mcp_server(
    app: tauri::AppHandle,
    node_service: std::sync::Arc<nodespace_core::NodeService>,
    embedding_service: Option<std::sync::Arc<nodespace_core::services::NodeEmbeddingService>>,
    cancel_token: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    use futures::FutureExt;

    tracing::info!("🔧 Initializing MCP server service...");

    // Create MCP service with Tauri event callback
    let (mcp_service, callback) = mcp_integration::create_mcp_service_with_events(
        node_service,
        embedding_service,
        app.clone(),
    );

    tracing::info!(
        "✅ McpServerService created on port {}, spawning background task...",
        mcp_service.port()
    );

    // Spawn MCP server task with Tauri event emissions
    // Uses panic protection to prevent silent background task failures
    // Monitors cancel_token for graceful shutdown before runtime drops
    tauri::async_runtime::spawn(async move {
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

    // Initialize tracing — respects RUST_LOG env var, defaults to info for nodespace_core
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("nodespace_core=info")),
        )
        .init();

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

            // Register AppServices container as managed state (Issue #894)
            // Services are populated later via commands/db.rs::init_services()
            app.manage(app_services::AppServices::new());

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
    let shutdown_token_for_events = shutdown_token.clone();
    app.run(move |app_handle, event| match event {
        RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { .. },
            ..
        } => {
            tracing::info!(
                "Window '{}' close requested, performing graceful shutdown...",
                label
            );
            graceful_shutdown(app_handle);
        }
        RunEvent::ExitRequested { code, .. } => {
            tracing::info!(
                "App exit requested (code: {:?}), performing graceful shutdown...",
                code
            );
            graceful_shutdown(app_handle);
        }
        RunEvent::Exit => {
            tracing::info!("App exiting, ensuring shutdown signal sent...");
            shutdown_token_for_events.cancel();
        }
        _ => {}
    });
}

/// Perform graceful shutdown: cancel background tasks, wait for them to exit, then release GPU.
///
/// Guarded by an `AtomicBool` because Tauri may fire both `CloseRequested` and
/// `ExitRequested` events, and we must only run the shutdown sequence once.
pub(crate) fn graceful_shutdown(app_handle: &tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::Manager;

    static SHUTDOWN_ONCE: AtomicBool = AtomicBool::new(false);
    if SHUTDOWN_ONCE.swap(true, Ordering::SeqCst) {
        tracing::debug!("Graceful shutdown already in progress, skipping duplicate call");
        return;
    }

    if let Some(shutdown_token) = app_handle.try_state::<ShutdownToken>() {
        shutdown_token.cancel();
    }
    // Grace period for background tasks (MCP server, domain event forwarder)
    // to exit their tokio::select! loops and drop their Arc references.
    // Must complete BEFORE release_gpu_resources() takes ownership of embedding state,
    // so those tasks don't race with GPU resource teardown.
    std::thread::sleep(std::time::Duration::from_millis(200));
    release_gpu_resources(app_handle);
}

/// Release GPU resources (Metal context and backend) to prevent SIGABRT crash on exit.
///
/// Now accesses embedding state through AppServices container.
/// Runs on a dedicated thread because `graceful_shutdown()` may be called from
/// within the Tokio runtime (Tauri run-event handler), where `block_on` would panic.
pub(crate) fn release_gpu_resources(app_handle: &tauri::AppHandle) {
    use tauri::Manager;

    if let Some(services) = app_handle.try_state::<app_services::AppServices>() {
        let services_clone = services.inner().clone();
        // Spawn a dedicated thread to avoid "cannot block_on inside a runtime" panic.
        // The Tauri run-event handler runs on a Tokio runtime thread, so we need
        // a fresh thread with its own block_on to drive the async GPU release.
        let handle = std::thread::spawn(move || {
            tauri::async_runtime::block_on(async {
                services_clone.release_gpu_resources().await;
            });
        });
        // Wait for GPU release to complete before releasing the backend
        if let Err(e) = handle.join() {
            tracing::error!("GPU resource release thread panicked: {:?}", e);
        }
    }

    // Step 2: Release the global llama backend itself
    // Must happen AFTER all models/contexts are dropped (step 1)
    nodespace_nlp_engine::release_llama_backend();
}
