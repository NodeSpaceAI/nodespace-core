// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // A release build has no console at all (see the `windows_subsystem`
    // attribute above), so `eprintln!`/`println!` anywhere in this process —
    // notably `SchemaNode::from_node`'s fields-parse-failure diagnostic,
    // called directly by `commands::schemas`'s Tauri commands — would
    // otherwise be silently discarded. Redirect this process's own stdio to
    // log files before anything else runs, so nothing before this point has
    // already tried (and lost) a diagnostic write. Debug builds keep their
    // attached console (no `windows_subsystem = "windows"` there), so this is
    // gated to release-on-Windows only to avoid silently moving a
    // developer's live terminal output into a log file during `tauri dev`.
    // See `daemon_setup::redirect_gui_stdio_to_log_files`'s doc comment for
    // the real-Windows verification behind this.
    #[cfg(all(windows, not(debug_assertions)))]
    nodespace_app_lib::daemon_setup::redirect_gui_stdio_to_log_files();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime");

    // Set this runtime as Tauri's async runtime before starting the app
    tauri::async_runtime::set(runtime.handle().clone());

    // Run the app within our custom runtime
    runtime.block_on(async { nodespace_app_lib::run() })
}
