// Tauri event channel constants for the agent subsystem (kept in desktop-app
// because they depend on Tauri, which is not a dependency of nodespace-agent).
pub mod agent_events;

// Tauri commands module (public for dev-server access)
pub mod commands;

// Local type mirrors for command layer (severs nodespace_core dep from commands/)
pub mod types;

// Application preferences management
pub mod preferences;

// Shared constants
pub mod constants;

// Background services
pub mod services;

// gRPC-backed node event watcher. Inert until activated —
// see watcher.rs module docs for activation gating.
pub mod watcher;

// Daemon lifecycle: launchd (macOS), systemd (Linux), direct spawn (Windows)
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub mod daemon_setup;

// First-launch skill installer
pub mod skill_setup;

// App update check (detect newer releases)
pub mod update_check;

// Shared atomic write-to-temp-then-rename helper for JSON config files.
mod atomic_file;

// Window <-> database routing: window label/pin tracking and the
// emit-routing helper that replaces every hardcoded "main" window emit.
pub mod window_routing;

// Per-database window size/position persistence.
pub mod window_state;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn toggle_sidebar() -> String {
    "Sidebar toggled!".to_string()
}

/// True when the frontend debug channel is enabled (env `NS_FRONTEND_LOG` set to
/// a file path). The frontend gates its forwarding on this so normal builds pay
/// no IPC cost; diagnostics set the env per window to capture console messages,
/// invoke/network calls, DOM snapshots, and store dumps as structured NDJSON.
#[tauri::command]
fn frontend_log_enabled() -> bool {
    std::env::var_os("NS_FRONTEND_LOG").is_some()
}

/// Append one NDJSON debug-channel line to the file named by `NS_FRONTEND_LOG`.
/// `line` is a pre-serialized JSON object (see `DebugEvent` on the frontend) —
/// this command is a dumb sink with no schema knowledge of its contents.
/// No-op if the env var is unset. Best-effort (diagnostic only) — never fails
/// the caller.
#[tauri::command]
fn frontend_log(line: String) {
    if let Some(path) = std::env::var_os("NS_FRONTEND_LOG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Report the current daemon health to the frontend.
///
/// Returns "healthy", "starting", or "not_running". The frontend uses this
/// to decide whether to show an error state.
#[tauri::command]
async fn check_daemon_status() -> String {
    daemon_status_body().await
}

/// The body of [`check_daemon_status`], factored out and made `pub` so the
/// readiness integration test in `tests/` — a separate crate — can call the
/// exact same logic the Tauri command invokes, including its
/// `resolve_socket_path()` call. Kept out of the `#[tauri::command]`-
/// annotated function itself: that macro generates hidden crate-scoped
/// items keyed to the function's identifier, and marking the annotated
/// function `pub` collides with them (`E0255`, defined multiple times).
pub async fn daemon_status_body() -> String {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        use daemon_setup::{check_daemon_socket, DaemonStatus};

        // Probe the SAME socket the gRPC client dials (honors NODESPACED_SOCKET).
        let socket_path = crate::services::grpc_client::resolve_socket_path();
        return match check_daemon_socket(socket_path.as_path()).await {
            DaemonStatus::Healthy => "healthy".to_string(),
            DaemonStatus::Starting => "starting".to_string(),
            DaemonStatus::NotRunning => "not_running".to_string(),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    "healthy".to_string()
}

// Include test module
#[cfg(test)]
mod tests;

// Regression coverage for the CloseRequested-veto shutdown race — its own
// module (rather than folded into `tests`) since it needs `tauri::test`'s
// MockRuntime to drive real RunEvent delivery, not just plain unit tests.
#[cfg(test)]
mod shutdown_tests;

/// Shared shutdown token for graceful background task termination.
///
/// Managed as Tauri state so it can be accessed from both the setup phase
/// (where background tasks are spawned) and the run event handler (where
/// shutdown is triggered). When cancelled, all background tasks exit their
/// loops before the Tokio runtime drops.
#[derive(Clone)]
pub struct ShutdownToken {
    cancellation: tokio_util::sync::CancellationToken,
    /// Guards the one-shot shutdown sequence in [`graceful_shutdown`]. Lives
    /// on the token itself rather than a function-local `static`: a `static`
    /// is process-global, so it would (a) leak across every test in this
    /// binary instead of resetting per `ShutdownToken` instance, and (b) — for
    /// the window-per-database work — trip on the first window's close and
    /// silently no-op every other window's teardown. One token per app today
    /// still means one guard per app; this just puts it in a place that
    /// scales to one-per-window later without another rewrite.
    shutdown_started: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl ShutdownToken {
    fn new() -> Self {
        Self {
            cancellation: tokio_util::sync::CancellationToken::new(),
            shutdown_started: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Create a child token for a background task.
    /// Cancelling the parent automatically cancels all children.
    pub fn child_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancellation.child_token()
    }

    /// Signal all background tasks to shut down.
    /// Idempotent - safe to call multiple times.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// True once [`Self::cancel`] has run.
    #[cfg(test)]
    fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    /// True once [`graceful_shutdown`] has run at least once — i.e. the app
    /// actually reached `ExitRequested`/`Exit`, by any path. [`arm_quit_watchdog`]
    /// reads this to tell "the normal close path got the app into its real
    /// shutdown sequence" apart from "nothing has happened yet", so it only
    /// forces an exit in the latter case.
    fn is_shutdown_started(&self) -> bool {
        self.shutdown_started
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Marks the one-shot shutdown sequence as started. Returns `true` the
    /// first time it's called on this token, `false` on every later call —
    /// [`graceful_shutdown`] uses this to run its teardown exactly once even
    /// though both `ExitRequested` and `Exit` drive it.
    fn begin_shutdown_once(&self) -> bool {
        !self
            .shutdown_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    }
}

/// A database id from a relaunch that arrived before the frontend had
/// registered its `tray:select-database` listener.
///
/// Tauri does not buffer or replay an event emitted while it has zero current
/// listeners (confirmed against the `tauri` event-emission source — it looks
/// up registered handlers for the event name at the instant `emit` runs and
/// nothing more), and the listener only exists once `app-shell.svelte` has
/// mounted and its `listen()` IPC round-trip has resolved — a relaunch in that
/// narrow window (webview still booting) would otherwise focus the window but
/// silently drop the switch. `handle_relaunch` stashes here in addition to
/// emitting; the frontend pulls it once via
/// [`take_pending_tray_database_selection`] right after registering the
/// listener, so the switch lands either way `switchTo` is idempotent against a
/// value the live event also happened to deliver.
///
/// A plain module-level `Mutex` rather than Tauri-managed state: the
/// single-instance plugin's relaunch callback can fire before this app's own
/// `app.manage(..)` calls inside `.setup()` have run (both start during
/// `Builder::run`, and the plugin is deliberately registered first), so
/// managed state would need its own readiness check; a `static` has none.
static PENDING_TRAY_DATABASE_SELECTION: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);

/// One-shot read of a database id [`handle_relaunch`] stashed before the
/// frontend's `tray:select-database` listener existed to receive the live
/// event. Consumes the value so a later, unrelated call never re-applies a
/// stale selection.
#[tauri::command]
fn take_pending_tray_database_selection() -> Option<String> {
    PENDING_TRAY_DATABASE_SELECTION.lock().unwrap().take()
}

/// Handle a relaunch forwarded by `tauri-plugin-single-instance`: the daemon's
/// tray always spawns a fresh UI process (it cannot reliably tell whether one
/// is already alive), so every click after the first arrives here instead of
/// as a second window.
///
/// Always focuses the running window — cross-process activation
/// (`NSRunningApplication::activate` on macOS, the equivalent on Windows/Linux)
/// is exactly what `WebviewWindow::set_focus` does under the hood, so a plain
/// "Open NodeSpace" click while the app is already running now does something:
/// it raises the window instead of silently no-op'ing. If the relaunch also
/// named a specific database (`--database <id>`, set by the tray only when the
/// user picked one from the submenu), forward it to the frontend — both as a
/// live event and stashed for the pull-based fallback above — so it can
/// switch in place via `databaseStore.switchTo` — the same path an in-app
/// switcher click already uses.
fn handle_relaunch<R: tauri::Runtime>(app: &tauri::AppHandle<R>, argv: &[String]) {
    use tauri::Emitter;

    // Stash the requested database *before* resolving a window to focus —
    // deliberately ahead of (and independent from) the early-return below.
    // `resolve_focus_window` returning `None` means there is no window to
    // raise right now (e.g. every window was closed while the app kept
    // running in the tray), not that the pick should be discarded: a window
    // created later still needs to pull this via
    // `take_pending_tray_database_selection` once it registers its listener.
    // Stashing only after the early return would silently drop exactly the
    // case the pull-based fallback exists to catch.
    let pending_id = parse_relaunch_database_arg(argv);
    if let Some(id) = &pending_id {
        *PENDING_TRAY_DATABASE_SELECTION.lock().unwrap() = Some(id.clone());
    }

    let Some(window) = window_routing::resolve_focus_window(app) else {
        tracing::warn!("single-instance relaunch: no window to focus");
        return;
    };
    if let Err(e) = window.unminimize() {
        tracing::warn!(error = %e, "failed to unminimize the main window on relaunch");
    }
    if let Err(e) = window.show() {
        tracing::warn!(error = %e, "failed to show the main window on relaunch");
    }
    if let Err(e) = window.set_focus() {
        tracing::warn!(error = %e, "failed to focus the main window on relaunch");
    }

    if let Some(id) = pending_id {
        // Scoped to exactly the window just focused above via `emit_to`, NOT
        // a bare `window.emit(...)` — `Emitter::emit` is never scoped to the
        // object it's called on (confirmed against the `tauri` 2.11 source;
        // see `window_routing::resolve_target_labels`'s doc comment), so with
        // more than one window open a bare `.emit()` here would broadcast the
        // switch to every window instead of just the one being raised.
        if let Err(e) = window.emit_to(window.label(), "tray:select-database", id) {
            tracing::warn!(error = %e, "failed to emit tray:select-database");
        }
    }
}

/// Extract the database id from a relaunch's argv, as passed by the daemon's
/// tray via `--database <id>` (the CLI counterpart of
/// `nodespace_daemon::tray::INITIAL_DATABASE_ENV`, forwarded here because the
/// environment of the just-exited relaunch process is not visible to the
/// already-running instance) when the user picks a specific database while
/// the app is already running. `None` when the relaunch carried no database
/// request (a plain "Open NodeSpace") or the flag's value is missing or blank.
fn parse_relaunch_database_arg(argv: &[String]) -> Option<String> {
    let value = argv
        .iter()
        .position(|a| a == "--database")
        .and_then(|i| argv.get(i + 1))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::{menu::*, Manager};

    // Initialize tracing — respects RUST_LOG env var, defaults to info for nodespace_app
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("nodespace_app=info")),
        )
        .try_init()
        .ok();

    // Create shutdown token for coordinating graceful background task
    // termination. `app.run`'s event handler (`handle_run_event`) no longer
    // needs its own clone — it looks the token up via managed state instead,
    // the same way `graceful_shutdown` always has.
    let shutdown_token_for_setup = ShutdownToken::new();

    // Single-instance guard: the daemon's tray always spawns a fresh UI
    // process when a database is picked (see `nodespace_daemon::tray::
    // TrayState::open_ui`) — it has no reliable way to know whether a UI
    // process from an earlier launch is still alive, and none at all when the
    // app was started outside the tray. Rather than the daemon tracking
    // liveness, this plugin makes any second launch detect the running
    // instance, forward its argv to it, and exit immediately, so a real UI
    // process almost never survives alongside an existing one regardless of
    // launch path. Must be registered before any other plugin (upstream
    // requirement). Note this plugin's macOS backend claims the lock with a
    // connect-then-bind probe rather than one atomic OS primitive (unlike its
    // Windows/Linux backends), so it is not an absolute guarantee there — see
    // the doc comment on `TrayState::open_ui` in the daemon crate.
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            handle_relaunch(app, &argv);
        }));
    }

    let app = builder
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

            let open_settings = MenuItemBuilder::new("Settings...")
                .id("open_settings")
                .accelerator("CmdOrCtrl+,")
                .build(app)?;

            let open_integrations = MenuItemBuilder::new("Integrations...")
                .id("open_integrations")
                .build(app)?;

            let settings_separator = PredefinedMenuItem::separator(app)?;
            let integrations_separator = PredefinedMenuItem::separator(app)?;

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
                    &import_submenu,
                    &settings_separator,
                    &open_settings,
                    &integrations_separator,
                    &open_integrations,
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

            // Register shutdown token as managed state for background task coordination.
            app.manage(shutdown_token_for_setup.clone());

            // Window <-> database pin registry — every emit-routing decision
            // in `window_routing::emit_routed` reads this.
            app.manage(window_routing::WindowDatabaseRegistry::default());

            // Kill the running daemon if its binary is stale (size mismatch vs bundled
            // sidecar). Must run before connect_lazy so the frontend never connects to
            // an outdated daemon. ensure_daemon_running (spawned below) then extracts
            // the fresh binary and restarts via launchd/systemd.
            #[cfg(unix)]
            crate::daemon_setup::kill_stale_daemon_sync();

            // Manage the gRPC client EAGERLY via a lazy channel (connects on first
            // RPC). Previously `manage(GrpcClient)` ran only after the async connect
            // below, so a frontend command issued at startup (e.g. the date page's
            // `get_children_tree`) raced it and got a fatal "state not managed for
            // field `client`", closing the view. With the client
            // managed up front, an early call instead yields a retryable transport
            // error until the daemon is reachable.
            #[cfg(any(unix, windows))]
            app.manage(crate::services::GrpcClient::connect_lazy());

            // Best-effort update check: on a background task so it never delays
            // startup, and it only emits when a strictly-newer release exists — no
            // event on "up to date" or any failure, so the frontend banner appears
            // solely on a real update. Independent of the daemon path below.
            {
                let update_app_handle = app.handle().clone();
                let current_version = app.package_info().version.to_string();
                tauri::async_runtime::spawn(async move {
                    let status = update_check::check_for_update(&current_version).await;
                    if status.update_available {
                        window_routing::emit_routed(
                            &update_app_handle,
                            update_check::UPDATE_AVAILABLE_EVENT,
                            &status,
                            None,
                        );
                    }
                });
            }

            // Spawn async task to start the daemon (if needed) and wire up the
            // watcher, token stream, and Pro probe on the (already-managed) client.
            // setup() is synchronous so we can't block_on here — spawn a task instead.
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            {
                use tauri::Emitter;

                let app_handle = app.handle().clone();
                let session_token = shutdown_token_for_setup.child_token();

                tauri::async_runtime::spawn(async move {
                    // Ensure nodespaced service is installed and running (launchd on macOS, systemd on Linux).
                    {
                        use daemon_setup::{ensure_daemon_running, DaemonStatus};

                        // Signal the frontend to hold off on gRPC calls until the daemon is ready.
                        window_routing::emit_routed(&app_handle, "daemon-status", "starting", None);

                        match ensure_daemon_running(&app_handle).await {
                            Ok(DaemonStatus::Healthy) => {
                                tracing::info!("nodespaced is running");
                                window_routing::emit_routed(
                                    &app_handle,
                                    "daemon-status",
                                    "healthy",
                                    None,
                                );
                            }
                            Ok(status) => {
                                tracing::warn!("nodespaced not yet healthy: {:?}", status);
                                window_routing::emit_routed(
                                    &app_handle,
                                    "daemon-status",
                                    "not_running",
                                    None,
                                );
                            }
                            Err(e) => {
                                tracing::error!("Daemon setup failed: {:#}", e);
                                window_routing::emit_routed(
                                    &app_handle,
                                    "daemon-status",
                                    "not_running",
                                    None,
                                );
                            }
                        }
                    }

                    // First-launch: install NodeSpace skill into detected agents.
                    // Idempotent — no-op once ~/.nodespace/setup.json marks skill_installed.
                    {
                        use crate::skill_setup;
                        // WARN logging (once per new failure, DEBUG on repeats) happens
                        // inside install_skill itself — see its module docs. Emitting to
                        // the frontend only happens here: this is the one fire-and-forget
                        // call site with no other way to reach the user (the onboarding/
                        // manual-retry commands already return their result synchronously
                        // to an awaiting caller, so emitting there too would show the same
                        // failure twice).
                        let result = skill_setup::install_skill(false, &app_handle).await;
                        if result.success {
                            if !result.agents_installed.is_empty() {
                                tracing::info!(
                                    "NodeSpace skill installed into: {:?}",
                                    result.agents_installed
                                );
                            }
                            if let Some(warning) = result.cli_warning {
                                tracing::warn!("{}", warning);
                                window_routing::emit_routed(
                                    &app_handle,
                                    "skill:cli-missing",
                                    serde_json::json!({ "warning": warning }),
                                    None,
                                );
                            }
                        } else if result.failure_is_new {
                            if let Some(error) = result.error {
                                window_routing::emit_routed(
                                    &app_handle,
                                    "skill:install-failed",
                                    serde_json::json!({ "error": error }),
                                    None,
                                );
                            }
                        }
                    }

                    // The gRPC client is already managed (lazy) above. Fetch it and
                    // wire the watcher, token stream, and Pro probe; the underlying
                    // channel connects on first use.
                    use tauri::Manager;
                    let grpc_client = (*app_handle.state::<crate::services::GrpcClient>()).clone();
                    let channel = grpc_client.channel().await;
                    // The watcher rides the shared client so its WatchNodes stream
                    // targets the active database and re-subscribes on switch (ADR-053).
                    watcher::spawn(app_handle.clone(), grpc_client.clone(), session_token);
                    // Subscribe to token stream for ai-chat node inference events.
                    commands::local_agent::start_token_stream_subscription(
                        app_handle.clone(),
                        grpc_client,
                    );

                    // Pro capability probe: a single WatchSyncStatus call on the same
                    // channel. Community `nodespaced` returns `Status::Unimplemented`
                    // → tier=Community → sync pill stays hidden. Pro `nodespaced-pro`
                    // answers → tier=Pro → pill renders and listens for `sync:status`.
                    let pro = crate::services::ProClient::probe_on_channel(channel).await;
                    let tier = pro.tier().await;
                    let last_status = pro.last_status().await;
                    // No `pro_activate_database` call has landed yet at probe time, so
                    // this is always empty — the frontend's per-database sync store
                    // attributes an empty database_id to whichever database it
                    // currently considers active (see `isActiveDatabaseEvent`'s
                    // convention, mirrored in `pro-sync.svelte.ts`).
                    let database_id = pro.attributed_database_id().await;
                    let payload = serde_json::json!({
                        "tier": tier,
                        "initial_status": last_status.as_ref().map(|s| {
                            serde_json::json!({
                                "state": s.state,
                                "detail": s.detail,
                                "user_email": s.user_email,
                                "database_id": database_id,
                            })
                        }),
                    });
                    app_handle.manage(pro);
                    if let Err(e) = app_handle.emit("pro:tier-detected", payload) {
                        tracing::warn!(error = %e, "failed to emit pro:tier-detected");
                    }
                    tracing::info!(?tier, "Pro capability probe done");

                    // Re-establish the "daemon unreachable" signal lost when the
                    // eager lazy client replaced the connect()-or-emit path. The lazy
                    // channel never fails at startup, so a genuinely-down daemon would
                    // otherwise leave the UI silently empty. After the probe has
                    // exercised the channel, confirm reachability with the same socket
                    // check `check_daemon_status` uses and emit `not_running` so
                    // app-shell shows its error banner + retry. `Starting`
                    // is transient — only `NotRunning` trips the banner.
                    {
                        use daemon_setup::{check_daemon_socket, DaemonStatus};
                        // Probe the SAME socket the gRPC client dials (honors
                        // NODESPACED_SOCKET), not the hardcoded default — else a
                        // socket override falsely reports the daemon down.
                        let socket_path = crate::services::grpc_client::resolve_socket_path();
                        if matches!(
                            check_daemon_socket(socket_path.as_path()).await,
                            DaemonStatus::NotRunning
                        ) {
                            tracing::error!("nodespaced unreachable after startup");
                            window_routing::emit_routed(
                                &app_handle,
                                "daemon-status",
                                "not_running",
                                None,
                            );
                        }
                    }
                });
            }

            // Streaming task registry for PTY session cancellation
            app.manage(commands::agent_session::StreamingTaskRegistry::default());

            Ok(())
        })
        .on_menu_event(|app, event| {
            let toggle_sidebar_id = MenuId::new("toggle_sidebar");
            let toggle_status_bar_id = MenuId::new("toggle_status_bar");
            let quit_id = MenuId::new("quit");
            let import_folder_id = MenuId::new("import_folder");
            let open_settings_id = MenuId::new("open_settings");
            let open_integrations_id = MenuId::new("open_integrations");

            if *event.id() == toggle_sidebar_id {
                // Route to the focused window (none of these menu events are
                // database-scoped — see `window_routing`).
                window_routing::emit_routed(app, "menu-toggle-sidebar", (), None);
                println!("Sidebar toggle requested from menu");
            } else if *event.id() == toggle_status_bar_id {
                window_routing::emit_routed(app, "menu-toggle-status-bar", (), None);
                println!("Status bar toggle requested from menu");
            } else if *event.id() == import_folder_id {
                window_routing::emit_routed(app, "menu-import-folder", (), None);
                println!("Import folder requested from menu");
            } else if *event.id() == open_settings_id {
                window_routing::emit_routed(app, "menu-open-settings", (), None);
            } else if *event.id() == open_integrations_id {
                window_routing::emit_routed(app, "menu-open-integrations", (), None);
            } else if *event.id() == quit_id {
                // Still start the normal window-close path when a window
                // exists -- it's what carries the pending-write flush (see
                // `app-initialization.ts`'s `registerTauriCloseHandler`) --
                // but that path depends on a window existing at all and on
                // its webview being alive enough to run the flush and call
                // `destroy()`. Neither holds if every window is already
                // closed or the webview has crashed/hung, so `Quit` would
                // otherwise silently do nothing. `arm_quit_watchdog` bounds
                // how long this waits before forcing an unconditional exit.
                if let Some(window) = window_routing::resolve_focus_window(app) {
                    let _ = window.close();
                }
                if let Some(shutdown_token) = app.try_state::<ShutdownToken>() {
                    arm_quit_watchdog(
                        shutdown_token.inner().clone(),
                        QUIT_WATCHDOG_TIMEOUT,
                        || {
                            tracing::error!(
                                timeout_secs = QUIT_WATCHDOG_TIMEOUT.as_secs(),
                                "Tray Quit did not reach ExitRequested via the normal close \
                             path in time -- forcing exit."
                            );
                            // `AppHandle::exit` only posts a message onto the
                            // native event loop's proxy (confirmed against the
                            // vendored `tauri-runtime-wry` source) -- it needs
                            // that loop to still be pumping, which is exactly
                            // what's in question if the normal path never got
                            // here. `std::process::exit` is unconditional
                            // regardless of what any thread, including the main
                            // one, is doing -- matching `nodespace-daemon`'s
                            // `arm_shutdown_watchdog`, which this mirrors.
                            std::process::exit(0);
                        },
                    );
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            toggle_sidebar,
            frontend_log_enabled,
            frontend_log,
            check_daemon_status,
            take_pending_tray_database_selection,
            window_routing::pin_window_database,
            update_check::check_for_update_command,
            commands::pro_sync::pro_tier,
            commands::pro_sync::pro_current_status,
            commands::pro_sync::pro_subscribe_sync_status,
            commands::pro_sync::pro_initiate_oauth,
            commands::pro_sync::pro_signout,
            commands::pro_sync::pro_enable_sync,
            commands::pro_sync::pro_activate_database,
            commands::pro_sync::pro_set_member,
            commands::pro_sync::pro_remove_member,
            commands::pro_sync::pro_leave_collection,
            commands::pro_sync::pro_list_members,
            commands::pro_sync::pro_create_invite,
            commands::pro_sync::pro_accept_invite,
            commands::pro_sync::pro_request_join,
            commands::pro_sync::pro_join_collection,
            commands::pro_sync::pro_list_joinable_collections,
            commands::pro_sync::pro_approve_request,
            commands::pro_sync::pro_list_invites,
            commands::pro_sync::pro_list_requests,
            commands::pro_sync::pro_revoke_invite,
            commands::pro_sync::pro_current_person,
            commands::conflicts::list_conflicts,
            commands::conflicts::conflicts_for_node,
            commands::conflicts::resolve_conflict,
            commands::embeddings::generate_root_embedding,
            commands::embeddings::search_roots,
            commands::embeddings::update_root_embedding,
            commands::embeddings::batch_generate_embeddings,
            commands::embeddings::on_root_closed,
            commands::embeddings::on_root_idle,
            commands::embeddings::sync_embeddings,
            commands::embeddings::get_stale_root_count,
            commands::nodes::create_node,
            commands::nodes::create_root_node,
            commands::nodes::create_node_mention,
            commands::nodes::get_node,
            commands::nodes::find_duplicate,
            commands::nodes::probe_and_recover_channel,
            commands::nodes::update_node,
            commands::nodes::move_node,
            commands::nodes::move_children_to_parent,
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
            commands::nodes::get_node_relationships,
            commands::nodes::create_relationship,
            commands::nodes::delete_relationship,
            commands::nodes::update_relationship_properties,
            commands::nodes::delete_node_mention,
            commands::nodes::update_task_node,
            // Collection commands (browsing and management UI)
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
            // Schema read commands (mutation commands removed, not used by UI)
            commands::schemas::get_all_schemas,
            commands::schemas::get_schema_definition,
            // File import commands for bulk markdown import
            commands::import::import_markdown_file,
            commands::import::import_markdown_files,
            commands::import::import_markdown_directory,
            // Settings commands
            commands::settings::get_settings,
            commands::settings::update_display_settings,
            commands::settings::get_capture_settings,
            commands::settings::update_capture_settings,
            commands::settings::get_openai_compat_configs,
            commands::settings::set_openai_compat_configs,
            // Local database registry commands (ADR-053)
            commands::database::get_daemon_version,
            commands::database::list_databases,
            commands::database::create_database,
            commands::database::register_database,
            commands::database::set_default_database,
            commands::database::rename_database,
            commands::database::remove_database,
            commands::database::set_active_database,
            commands::database::initial_database_id,
            // Local agent commands
            commands::local_agent::local_agent_status,
            commands::local_agent::local_agent_cancel_turn,
            commands::local_agent::ensure_model_ready,
            commands::local_agent::list_local_models,
            // Chat model management commands
            commands::chat_models::chat_model_list,
            commands::chat_models::chat_model_recommended,
            commands::chat_models::chat_model_download,
            commands::chat_models::chat_model_cancel_download,
            commands::chat_models::chat_model_delete,
            commands::chat_models::chat_model_load,
            commands::chat_models::chat_model_unload,
            commands::chat_models::get_system_ram_gb,
            // PTY agent session commands
            commands::agent_session::launch_session,
            commands::agent_session::write_input,
            commands::agent_session::resize_terminal,
            commands::agent_session::terminate_session,
            commands::agent_session::list_sessions,
            commands::agent_session::check_agent_availability,
            // First-launch onboarding wizard + Settings integrations panel
            commands::onboarding::check_onboarding_status,
            commands::onboarding::configure_path,
            commands::onboarding::remove_from_path,
            commands::onboarding::configure_skill,
            commands::onboarding::complete_onboarding,
            commands::onboarding::install_skill,
            commands::onboarding::remove_skill,
            commands::onboarding::get_skill_setup_status,
            commands::onboarding::get_integrations_status,
            // Local identity (ADR-037)
            commands::onboarding::get_local_identity,
            commands::onboarding::set_local_identity,
            commands::onboarding::get_identity_prefill,
            commands::onboarding::should_prompt_identity_backfill,
            commands::onboarding::dismiss_identity_backfill_prompt,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Run with event handler for graceful shutdown. Routing lives in
    // `handle_run_event` (module-level, not inlined here) so it can be
    // exercised directly against `tauri::test`'s MockRuntime without a real
    // window — see `shutdown_tests`.
    app.run(move |app_handle, event| handle_run_event(app_handle, &event));
}

/// Route one `RunEvent` to the right shutdown action.
///
/// `WindowEvent::CloseRequested` deliberately does NOT call
/// [`graceful_shutdown`]. Tauri fires it the instant the OS asks to close a
/// window — before the frontend (`app-initialization.ts`'s
/// `onCloseRequested`) has had any chance to decide whether to veto it and
/// flush pending writes first. Whenever a frontend listener is registered
/// for that event, which ours always is, Tauri's own manager holds the
/// window open pending that decision regardless of what the listener
/// ultimately does (confirmed against the `tauri` 2.11 source —
/// `manager/window.rs`'s `on_window_event` calls `api.prevent_close()`
/// unconditionally whenever `has_js_listener` is true, before the frontend
/// callback has even run). The window only actually closes once the
/// frontend explicitly calls `currentWindow.destroy()` — immediately when
/// there's nothing to flush, or after `flushAllPending()` resolves (that
/// call self-bounds to 5s, so this never waits unboundedly). `destroy()`
/// bypasses `CloseRequested` entirely and is what drives `ExitRequested`
/// then `Exit`, so gating `graceful_shutdown` on those two instead means it
/// can only run once the flush has already finished or hit its own timeout —
/// never mid-flush, and never before a vetoed close's later retry.
///
/// A single-window assumption is baked into this today (one `ShutdownToken`
/// per app). Window-per-database work will need a per-window token instead —
/// not built here, see `ShutdownToken::shutdown_started`'s doc comment for
/// why the one-shot guard already lives somewhere that scales to that.
fn handle_run_event<R: tauri::Runtime>(app_handle: &tauri::AppHandle<R>, event: &tauri::RunEvent) {
    use tauri::Manager;

    match event {
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { .. },
            ..
        } => {
            tracing::debug!(
                "Window '{}' close requested — deferring shutdown to ExitRequested/Exit so a \
                 frontend flush-before-close veto isn't cut short",
                label
            );
        }
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } => {
            // Drop the window's pin so a later event never routes to a label
            // nothing is listening on, or to a different, newer window that
            // happens to reuse the same label (see `WindowDatabaseRegistry::unpin`).
            if let Some(registry) = app_handle.try_state::<window_routing::WindowDatabaseRegistry>()
            {
                registry.unpin(label);
            }
        }
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Moved(_),
            ..
        } => {
            persist_window_geometry(app_handle, label);
        }
        tauri::RunEvent::ExitRequested { code, .. } => {
            tracing::info!(
                "App exit requested (code: {:?}), performing graceful shutdown...",
                code
            );
            graceful_shutdown(app_handle);
        }
        tauri::RunEvent::Exit => {
            // Safety net in case ExitRequested was ever skipped — cancel()
            // and graceful_shutdown's guard are both idempotent, so this is
            // a no-op on the (expected) common path where ExitRequested
            // already ran it.
            tracing::info!("App exiting, ensuring shutdown signal sent...");
            graceful_shutdown(app_handle);
        }
        _ => {}
    }
}

/// Best-effort persist `label`'s current content size + frame position,
/// keyed by the database it is pinned to. Re-queries the window live rather
/// than threading the `Resized`/`Moved` event's own payload through, since
/// either event alone only carries half the geometry (size XOR position) —
/// querying both fresh avoids reasoning about stitching a partial update
/// onto stale state. No-ops (nothing to key the save by) for a window that
/// hasn't pinned a database yet — the bootstrap window, in the brief window
/// between being shown and the frontend's first `pin_window_database` call.
///
/// Captures `inner_size()`, NOT `outer_size()` — [`window_routing::
/// pin_window_database`]'s restore path applies the saved width/height via
/// `WebviewWindow::set_size`, which `tauri-runtime-wry` dispatches as
/// `WindowMessage::SetSize` -> `Window::set_inner_size` (confirmed against
/// the vendored source). Saving the outer (decoration-inclusive) size here
/// while the restore path applies it as an inner size would inflate the
/// window by the decoration height on every restore, and the `Resized`
/// event that restore itself triggers would persist the inflated value,
/// compounding the error on every subsequent boot/switch. Position has no
/// such mismatch — `set_position` dispatches to `Window::set_outer_position`
/// — so `outer_position()` is captured and restored unchanged.
fn persist_window_geometry<R: tauri::Runtime>(app_handle: &tauri::AppHandle<R>, label: &str) {
    use tauri::Manager;

    let Some(registry) = app_handle.try_state::<window_routing::WindowDatabaseRegistry>() else {
        return;
    };
    let Some(database_id) = registry.database_for_window(label) else {
        return;
    };
    let Some(window) = app_handle.get_webview_window(label) else {
        return;
    };
    let (Ok(size), Ok(position)) = (window.inner_size(), window.outer_position()) else {
        return;
    };

    let geometry = window_state::WindowGeometry {
        width: size.width,
        height: size.height,
        x: position.x,
        y: position.y,
    };
    let app_handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = window_state::save_geometry(&app_handle, &database_id, geometry).await {
            tracing::warn!(error = %e, "failed to persist window geometry");
        }
    });
}

/// Pins the fix for the outer-vs-inner size mismatch at the call
/// site itself, since it cannot be exercised behaviorally: `tauri::test::
/// MockRuntime` — the only window-backed test double available in this
/// workspace (no real display in CI/this environment) — stubs both
/// `inner_size()` and `outer_size()` to the identical `PhysicalSize { 0, 0
/// }`, so a black-box unit test through `persist_window_geometry` cannot
/// distinguish which accessor actually ran; a real OS window is the only
/// thing that would report them differently. Saving `outer_size()` (the
/// original bug) while the restore path in `window_routing::
/// pin_window_database` applies the saved width/height via `set_size` —
/// which dispatches to `Window::set_inner_size`, confirmed against the
/// vendored `tauri-runtime-wry` source — inflates the window by the
/// decoration height on every restore, compounding on every subsequent
/// boot/switch since the restore's own `Resized` event re-persists the
/// inflated value.
#[cfg(test)]
mod window_geometry_capture_tests {
    #[test]
    fn persist_window_geometry_captures_inner_size_not_outer_size_for_width_and_height() {
        let source = include_str!("lib.rs");
        let start = source
            .find("fn persist_window_geometry")
            .expect("persist_window_geometry not found in lib.rs");
        let end = source[start..]
            .find("fn graceful_shutdown")
            .map(|offset| start + offset)
            .expect("graceful_shutdown not found after persist_window_geometry in lib.rs");
        let function_source = &source[start..end];

        assert!(
            function_source.contains("window.inner_size()"),
            "persist_window_geometry must capture width/height via window.inner_size() — \
             the restore path (set_size) applies the saved value as an inner size"
        );
        assert!(
            !function_source.contains("window.outer_size()"),
            "persist_window_geometry must NOT capture width/height via window.outer_size() — \
             saving the outer (decoration-inclusive) size while the restore path applies it as \
             an inner size inflates the window by the decoration height on every restore"
        );
        // Position is the other half of the round trip and must stay outer on
        // both sides (set_position -> Window::set_outer_position) — pinned here
        // too so a well-intentioned future "fix" doesn't flip position to inner
        // by the same mistaken symmetry reasoning that caused the width/height bug above.
        assert!(
            function_source.contains("window.outer_position()"),
            "persist_window_geometry must capture x/y via window.outer_position() — the \
             restore path (set_position) applies the saved value as an outer position"
        );
    }
}

/// Perform graceful shutdown: cancel background tasks.
///
/// Guarded by [`ShutdownToken::begin_shutdown_once`] so the sequence runs
/// exactly once per token even though `ExitRequested` and `Exit` can both
/// drive it for the same exit (see `handle_run_event`).
///
/// No blocking sleep here: the previous 200ms `std::thread::sleep` ran on
/// the Tauri event-loop thread — the same thread that services the webview —
/// which stalls the UI at exactly the moment a flush needs to make progress.
/// It also couldn't have helped the background tasks it was meant to give
/// time to: they run on the async runtime's own worker threads and react to
/// `cancel()` independently of how long this thread blocks afterward.
pub(crate) fn graceful_shutdown<R: tauri::Runtime>(app_handle: &tauri::AppHandle<R>) {
    use tauri::Manager;

    let Some(shutdown_token) = app_handle.try_state::<ShutdownToken>() else {
        return;
    };

    if !shutdown_token.begin_shutdown_once() {
        tracing::debug!("Graceful shutdown already in progress, skipping duplicate call");
        return;
    }

    shutdown_token.cancel();
    tracing::info!("Shutdown: complete");
}

/// Bounds how long the tray "Quit" item waits for the normal window-close
/// path (`WindowEvent::CloseRequested` -> frontend flush -> `destroy()` ->
/// `ExitRequested` -> `Exit`) before [`arm_quit_watchdog`] forces an
/// unconditional exit instead. The frontend's own flush self-bounds to 5s
/// (`shared-node-store.svelte.ts`'s `flushAllPending`), so this adds a few
/// seconds of margin for the IPC round trip and event propagation on top of
/// that -- generous enough not to cut off a real flush, but still bounded so
/// "Quit" can never hang forever: not for a healthy webview whose close
/// handler never runs, not for a crashed/hung one, and not for a window-less
/// app (every window already closed).
const QUIT_WATCHDOG_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// Spawn a thread that runs `on_timeout` once, after `timeout`, unless
/// `shutdown_token.is_shutdown_started()` is already true by then -- i.e.
/// the normal close path already got the app into its real shutdown
/// sequence (`ExitRequested`/`Exit` fired by any means), making a forced
/// exit redundant. A plain OS thread, not a tokio task, so it still fires
/// even if the webview or the async runtime is the thing that's wedged.
///
/// Mirrors `nodespace-daemon`'s `arm_shutdown_watchdog`; split out with an
/// injectable `on_timeout` for the same reason that one is -- unit-testable
/// without triggering a real process exit.
fn arm_quit_watchdog(
    shutdown_token: ShutdownToken,
    timeout: std::time::Duration,
    on_timeout: impl FnOnce() + Send + 'static,
) {
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        if !shutdown_token.is_shutdown_started() {
            on_timeout();
        }
    });
}

#[cfg(test)]
mod relaunch_tests {
    use super::parse_relaunch_database_arg;

    #[test]
    fn parses_database_id_from_argv() {
        let argv = vec![
            "/path/to/nodespace-app".to_string(),
            "--database".to_string(),
            "db-123".to_string(),
        ];
        assert_eq!(
            parse_relaunch_database_arg(&argv),
            Some("db-123".to_string())
        );
    }

    #[test]
    fn returns_none_without_the_flag() {
        let argv = vec!["/path/to/nodespace-app".to_string()];
        assert_eq!(parse_relaunch_database_arg(&argv), None);
    }

    #[test]
    fn returns_none_when_the_flag_has_no_value() {
        let argv = vec![
            "/path/to/nodespace-app".to_string(),
            "--database".to_string(),
        ];
        assert_eq!(parse_relaunch_database_arg(&argv), None);
    }

    #[test]
    fn returns_none_for_a_blank_value() {
        let argv = vec![
            "/path/to/nodespace-app".to_string(),
            "--database".to_string(),
            "   ".to_string(),
        ];
        assert_eq!(parse_relaunch_database_arg(&argv), None);
    }

    /// A flag value that happens to look like another flag is still just a
    /// value — parsing is positional (whatever follows `--database`), not a
    /// search for the next `--`-prefixed token.
    #[test]
    fn takes_the_very_next_token_even_if_flag_shaped() {
        let argv = vec![
            "/path/to/nodespace-app".to_string(),
            "--database".to_string(),
            "--not-actually-a-flag".to_string(),
        ];
        assert_eq!(
            parse_relaunch_database_arg(&argv),
            Some("--not-actually-a-flag".to_string())
        );
    }

    /// Both halves of the pull-based-fallback contract live in ONE test:
    /// `PENDING_TRAY_DATABASE_SELECTION` is a process-wide `static`, so
    /// separate tests touching it would race under the default parallel
    /// test runner and flake (mirrors `resolve_ui_binary_honors_env_var` in
    /// the daemon crate's tray tests, which does the same for its own
    /// process-wide env var). Covers: a relaunch with no window to focus at
    /// all (every window closed while the app kept running in the tray)
    /// must not panic, AND must still stash the requested database — the
    /// early return must not skip stashing, since a window created later
    /// still needs to pull it; a plain relaunch (no `--database` — the
    /// "Open NodeSpace" case) stashes nothing, so an unrelated later pull
    /// never sees a stale "open nothing in particular" mistaken for a real
    /// request; a relaunch naming a database (with a window to focus)
    /// stashes it (in addition to emitting the live event, not exercised
    /// here — that needs a real listener); a first pull observes it; a
    /// second pull observes nothing (consumed, not re-delivered).
    #[test]
    fn handle_relaunch_pull_based_fallback_stash_and_consume_cycle() {
        // No window exists yet — the early-return branch must still stash
        // before bailing out, not lose the pick.
        let windowless_app = tauri::test::mock_app();
        super::handle_relaunch(
            &windowless_app.handle().clone(),
            &[
                "/path/to/nodespace-app".to_string(),
                "--database".to_string(),
                "x".to_string(),
            ],
        );
        assert_eq!(
            super::take_pending_tray_database_selection(),
            Some("x".to_string()),
            "a relaunch with no window to focus must still stash the requested database"
        );

        let app = tauri::test::mock_app();
        tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("failed to build mock window");
        let handle = app.handle().clone();

        super::handle_relaunch(&handle, &["/path/to/nodespace-app".to_string()]);
        assert_eq!(
            super::take_pending_tray_database_selection(),
            None,
            "a plain relaunch (no --database) must not stash anything"
        );

        super::handle_relaunch(
            &handle,
            &[
                "/path/to/nodespace-app".to_string(),
                "--database".to_string(),
                "db-456".to_string(),
            ],
        );
        assert_eq!(
            super::take_pending_tray_database_selection(),
            Some("db-456".to_string())
        );
        assert_eq!(
            super::take_pending_tray_database_selection(),
            None,
            "a second pull must not re-deliver an already-consumed selection"
        );
    }
}
