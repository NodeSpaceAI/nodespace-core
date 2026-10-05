// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nodespace_app_lib::run(nodespace_app_lib::AppExtensions::none(), context())
}

/// This crate's Tauri context: its `tauri.conf.json`, ACL and assets.
///
/// `generate_context!()` expands here once, because on macOS it embeds the
/// Info.plist under a fixed symbol and a second expansion in the same binary
/// fails to link. Generic over the runtime, so the tests build this same
/// context on `tauri::test::MockRuntime`.
fn context<R: tauri::Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

#[cfg(test)]
mod window_capability_tests;
