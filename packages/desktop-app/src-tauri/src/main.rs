// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nodespace_app_lib::run(
        nodespace_app_lib::AppExtensions::none(),
        tauri::generate_context!(),
    )
}
