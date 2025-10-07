// Tauri commands module
mod commands;

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
            commands::models::ensure_models_installed,
            commands::nodes::create_node,
            commands::nodes::get_node,
            commands::nodes::update_node,
            commands::nodes::delete_node,
            commands::nodes::get_children,
            commands::nodes::get_nodes_by_origin_id,
            commands::nodes::query_nodes_simple,
            commands::nodes::save_node_with_parent,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
