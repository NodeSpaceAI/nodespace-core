// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Enable backtraces in release builds for crash diagnostics (#990)
    if std::env::var("RUST_BACKTRACE").is_err() {
        std::env::set_var("RUST_BACKTRACE", "1");
    }

    // Configure tokio runtime with larger thread stack size for SurrealDB performance
    // See: https://surrealdb.com/docs/surrealdb/reference-guide/performance-best-practices
    // Default stack size is too small for SurrealDB's embedded RocksDB operations
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(10 * 1024 * 1024) // 10MiB stack size (recommended by SurrealDB)
        .build()
        .expect("Failed to build tokio runtime");

    // Set this runtime as Tauri's async runtime before starting the app
    tauri::async_runtime::set(runtime.handle().clone());

    // Run the app within our custom runtime
    runtime.block_on(async { nodespace_app_lib::run() })
}
