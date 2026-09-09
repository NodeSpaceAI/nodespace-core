//! System tray for `nodespaced` (ADR-031).
//!
//! Owns the menu-bar / notification-area icon and acts as the platform-wide
//! UI launcher. Quit is symmetric across both halves of the app: the tray's
//! own "Quit" also asks any running desktop-app window to close (see
//! [`signal_ui_to_quit`]), and the desktop app's own window-close path
//! (Cmd+Q, the red traffic-light button, or any other close that actually
//! goes through) signals this daemon to stop in turn (see `nodespace-app`'s
//! `daemon_setup::signal_daemon_to_stop`) — a single action from either side
//! fully quits NodeSpace rather than leaving the other half running.
//!
//! Threading: the `tao` event loop must run on the main thread (macOS
//! `NSApplication` is main-thread-only), so the tonic gRPC server runs on a
//! worker tokio runtime and signals back via [`TrayController::shutdown`].

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIconBuilder};

use crate::services::database_manager::{DatabaseId, DatabaseStatus, RegistrySnapshot};

/// PNG used for the menu-bar icon. 32×32 is large enough that macOS, Windows
/// and Linux all downscale gracefully; we keep one asset rather than shipping
/// a per-platform set since the daemon's footprint should stay small.
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray-icon.png");

/// Set on the spawned UI process when the user opens a specific database from
/// the tray. The app treats it as the highest-precedence choice for that
/// launch only; a plain "Open NodeSpace" leaves it unset so the app restores
/// whatever it last had.
///
/// Read at startup by a *cold* launch (no other instance running). When an
/// instance is already running, the environment of a freshly-spawned process
/// is invisible to it — the same database id is also passed as a `--database
/// <id>` CLI argument (see [`TrayState::open_ui`]), which is what the app's
/// `tauri-plugin-single-instance` relaunch handler reads instead.
pub const INITIAL_DATABASE_ENV: &str = "NODESPACE_INITIAL_DATABASE";

/// This build variant's UI pid-file path, mirroring `nodespace-app`'s
/// `daemon_setup::write_own_ui_pid_file` -- both sides derive the identical
/// path from their own build-variant flags (debug/release x community/Pro)
/// rather than one copying the other's answer, exactly like the daemon
/// socket path each side already derives independently. `None` when the
/// home directory can't be resolved, same as every other home-relative path
/// in this crate.
fn ui_pid_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(home.join(nodespace_proto::socket::ui_pid_relative(
        cfg!(debug_assertions),
        cfg!(feature = "pro"),
    )))
}

/// The pid in `pid_file`, if it both parses and belongs to a live process
/// whose argv[0] matches `expected_binary` exactly -- the same verify-
/// before-signal discipline `nodespace-app`'s `daemon_setup::
/// kill_running_daemon` uses for the daemon side of this relationship.
///
/// `None` covers every "nothing to signal" case identically and silently: no
/// file (no UI process ever started, or one started and already exited and
/// cleaned up after itself), unparsable contents, a dead pid, or a live pid
/// some unrelated process now owns.
#[cfg(unix)]
fn verified_ui_pid(pid_file: &Path, expected_binary: &Path) -> Option<i32> {
    let contents = std::fs::read_to_string(pid_file).ok()?;
    let pid: i32 = contents.trim().parse().ok()?;
    let expected = expected_binary.to_string_lossy();
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .ok()?;
    let args = String::from_utf8_lossy(&out.stdout);
    (args.split_whitespace().next() == Some(expected.as_ref())).then_some(pid)
}

/// Ask the running desktop app, if any, to close its window too -- so tray
/// "Quit" fully quits NodeSpace rather than leaving a GUI window pointed at
/// a daemon that's about to disappear underneath it. Best-effort and silent
/// about failure either way: no UI window being open at all (the user
/// already closed it earlier and the daemon has been running tray-only) is
/// the common, expected case here, not an error.
///
/// On Unix, `pid_file` is verified via [`verified_ui_pid`] against
/// `ui_binary` before anything is signalled. SIGTERM there routes into the
/// app's own `initiate_app_quit` -- the exact same window-close +
/// flush-veto path its own Quit menu item and Cmd+Q already use, so a
/// pending unsaved-work flush is still respected. On Windows there is no pid
/// file: `taskkill /F /IM` targets `ui_binary`'s resolved image name
/// directly, force-killing it -- this loses the flush-before-close the Unix
/// path preserves, mirroring the same already-accepted gap `nodespace-app`'s
/// `daemon_setup::kill_running_daemon` (Windows variant) has for the daemon
/// side of a binary-update restart.
///
/// The actual `kill`/`taskkill` call is compiled out under `cfg(test)`: it's
/// a real, irreversible action against whatever process the pid file (or,
/// on Windows, the resolved UI binary's image name) happens to name on the
/// machine `cargo test` runs on, which is not safe to execute as a side
/// effect of a unit test suite. [`verified_ui_pid`] above is a read-only OS
/// query and stays unconditional -- it's what the tests below exercise.
#[cfg(unix)]
fn signal_ui_to_quit(pid_file: Option<&Path>, ui_binary: Option<&Path>) {
    let (Some(pid_file), Some(expected)) = (pid_file, ui_binary) else {
        return;
    };
    let Some(pid) = verified_ui_pid(pid_file, expected) else {
        return;
    };
    #[cfg(not(test))]
    {
        // SAFETY: kill() is always safe to call with a valid pid and signal.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    tracing::info!(pid, "Tray Quit: sent SIGTERM to the UI process");
}

/// Windows counterpart of the Unix [`signal_ui_to_quit`] above. There is no
/// pid file on this platform -- `taskkill /F /IM` targets `ui_binary`'s
/// resolved image name directly, force-killing it. This loses the
/// flush-before-close the Unix path preserves via SIGTERM, mirroring the
/// same already-accepted gap `nodespace-app`'s `daemon_setup::
/// kill_running_daemon` (Windows variant) has for the daemon side of a
/// binary-update restart. `_pid_file` is unused here -- Windows has no
/// equivalent lookup -- but kept in the signature so both platforms' call
/// sites in `run` stay identical.
#[cfg(windows)]
fn signal_ui_to_quit(_pid_file: Option<&Path>, ui_binary: Option<&Path>) {
    let Some(image_name) = ui_binary.and_then(|p| p.file_name()) else {
        return;
    };
    #[cfg(not(test))]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM"])
            .arg(image_name)
            .output();
    }
    tracing::info!(?image_name, "Tray Quit: sent taskkill to the UI process");
}

/// Events the tonic side of the daemon can push into the tray event loop.
///
/// `MenuEvent` is forwarded verbatim from `tray-icon`'s global channel so the
/// `tao` loop can process menu clicks. `RpcStateChanged` is how the gRPC
/// layer reports activity for the live Status label.
enum TrayEvent {
    Menu(MenuEvent),
    RpcStateChanged,
    /// A fresh view of the database registry, rendered into the Databases
    /// submenu. Carries owned data because it crosses from the tokio runtime
    /// into the `!Send` tray loop.
    DatabasesChanged(Box<RegistrySnapshot>),
    /// The gRPC task has stopped running for a reason other than the tray's
    /// own "Quit" menu item -- an OS signal (SIGTERM/SIGINT) drained it, or
    /// it returned an error, or it panicked. Nothing else watches that task,
    /// so without this the tao loop has no way to learn its work is done and
    /// sits forever with a live tray icon fronting a dead gRPC server.
    GrpcTaskFinished,
}

/// Handle the gRPC side of the daemon uses to talk to the tray.
///
/// `shutdown` resolves once when the user picks "Quit" so the tonic server
/// can drain and exit. The RPC counters drive the live Status label.
#[derive(Clone)]
pub struct TrayController {
    proxy: EventLoopProxy<TrayEvent>,
    quit_notify: Arc<tokio::sync::Notify>,
    active_rpcs: Arc<AtomicUsize>,
}

impl TrayController {
    /// Future that resolves when the user selects "Quit". Pass this to
    /// `tonic::transport::Server::serve_with_shutdown` so the gRPC server
    /// exits cleanly before the tray closes.
    pub async fn shutdown(&self) {
        self.quit_notify.notified().await;
    }

    /// Record that an RPC just started. Pair with [`Self::rpc_completed`] —
    /// the difference is what the Status menu shows.
    pub fn rpc_started(&self) {
        self.active_rpcs.fetch_add(1, Ordering::Relaxed);
        // Ignore send errors: the event loop may have exited during shutdown,
        // in which case the count update is irrelevant.
        let _ = self.proxy.send_event(TrayEvent::RpcStateChanged);
    }

    /// Companion to [`Self::rpc_started`]. Every increment has exactly one
    /// matching decrement in the metrics layer, so underflow is impossible
    /// under normal operation.
    pub fn rpc_completed(&self) {
        self.active_rpcs.fetch_sub(1, Ordering::Relaxed);
        let _ = self.proxy.send_event(TrayEvent::RpcStateChanged);
    }

    /// Publish the current database registry to the tray so the Databases
    /// submenu can be (re)rendered.
    ///
    /// The registry lives behind the gRPC runtime and is built well after the
    /// tray loop starts, so it cannot be handed over at seed time — it arrives
    /// here instead. A snapshot that lands before the tray finishes
    /// initializing is retained and applied once it does, so ordering between
    /// the two does not matter.
    pub fn databases_changed(&self, snapshot: RegistrySnapshot) {
        // Ignore send errors: the loop may have exited during shutdown.
        let _ = self
            .proxy
            .send_event(TrayEvent::DatabasesChanged(Box::new(snapshot)));
    }

    /// Tell the tray loop the gRPC task is done, so it exits too instead of
    /// leaving a tray icon fronting a dead server. Call this from whatever is
    /// watching the gRPC task's `JoinHandle` once it resolves -- by a signal
    /// draining it, by it returning an error, or by it panicking -- covering
    /// every way the task can stop other than the tray's own "Quit" click,
    /// which already reaches `ControlFlow::Exit` through the menu handler.
    pub fn grpc_task_finished(&self) {
        let _ = self.proxy.send_event(TrayEvent::GrpcTaskFinished);
    }
}

/// One database as rendered in the tray submenu.
///
/// Kept separate from the menu objects so the labelling rules — which is
/// open, which syncs, which cannot be opened — are decided in a plain
/// function that tests can call without a display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatabaseMenuEntry {
    pub id: DatabaseId,
    pub label: String,
    /// False for a registry entry whose file is gone — shown, but not
    /// selectable, so the menu still explains why it can't be opened.
    pub enabled: bool,
}

/// Render a registry snapshot into tray entries, in registry order.
///
/// The label carries the facts the registry persists — which database is the
/// default, and which sync to a cloud tenant. Those change only when the user
/// explicitly changes them, so a menu rendered once survives them better than
/// it would runtime state. Deliberately NOT whether a database is currently
/// *open*: the idle reaper closes databases minutes into
/// normal use, and this menu is only refreshed when the daemon pushes a new
/// snapshot, so an open marker would be confidently wrong most of the time. A
/// live open indicator belongs with live refresh, tracked separately.
///
/// A missing file replaces the other markers, since neither is meaningful once
/// the file is gone.
pub(crate) fn database_menu_entries(snapshot: &RegistrySnapshot) -> Vec<DatabaseMenuEntry> {
    snapshot
        .databases
        .iter()
        .map(|listing| {
            let mut markers: Vec<&str> = Vec::new();
            let missing = listing.status == DatabaseStatus::Missing;
            if missing {
                markers.push("missing");
            } else {
                if listing.is_default {
                    markers.push("default");
                }
                // Safe to show only because the submenu is now rebuilt on every
                // registry/open-set change. Pushed once at boot it would be
                // confidently wrong within minutes: the idle reaper closes
                // databases and in-app switching opens others.
                if listing.status == DatabaseStatus::Open {
                    markers.push("open");
                }
                if listing.entry.bound_tenant_schema.is_some() {
                    markers.push("synced");
                }
            }

            let label = if markers.is_empty() {
                listing.entry.name.clone()
            } else {
                format!("{} — {}", listing.entry.name, markers.join(" · "))
            };

            DatabaseMenuEntry {
                id: listing.entry.id.clone(),
                label,
                enabled: !missing,
            }
        })
        .collect()
}

/// Tray runtime state. Constructed inside the event loop's `Init` callback
/// because creating the icon before the loop is actually running produces
/// stale icons on macOS (a known upstream tauri-apps/tray-icon bug).
///
/// Not `Send` — `TrayIcon` holds platform handles (`NSStatusItem` on macOS,
/// HWND on Windows) that are tied to the thread that created them.
struct TrayState {
    _tray: tray_icon::TrayIcon,
    status_item: MenuItem,
    ui_binary: Option<PathBuf>,
    /// Retained (not just its id) so [`TrayState::refresh_ui_binary`] can
    /// re-label and re-enable it when the UI binary appears or disappears
    /// while the daemon is running.
    open_item: MenuItem,
    open_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
    /// Databases submenu, repopulated whenever a registry snapshot arrives.
    databases_menu: Submenu,
    /// Menu items currently in that submenu, retained so they can be removed
    /// on the next rebuild, paired with the database each one opens.
    database_items: Vec<(MenuItem, DatabaseId)>,
}

/// Label for the "Open NodeSpace" item.
///
/// When no UI binary can be resolved the item is disabled, and a disabled item
/// reading plain "Open NodeSpace" says only that the action is unavailable,
/// not why. Naming the reason in the label is the only channel the tray has —
/// a `tracing::warn!` into a log file is invisible to someone looking at a
/// menu.
fn open_item_label(ui_available: bool) -> &'static str {
    if ui_available {
        "Open NodeSpace"
    } else {
        "Open NodeSpace (not installed)"
    }
}

/// Build the tray menu. Status starts at "0 active calls" because the daemon
/// hasn't accepted any RPCs yet at the point the tray comes up.
///
/// `ui_available` reflects whether [`resolve_ui_binary`] found a UI binary. It
/// gates "Open NodeSpace" rather than being ignored: an enabled item that does
/// nothing when clicked is the worst of the options, since the user gets no
/// signal at all that the daemon cannot find the GUI. Greyed out, the tray at
/// least says so.
/// Returns the "Open NodeSpace" item itself rather than only its id, because
/// its enabled state is not fixed for the tray's lifetime — see
/// [`TrayState::refresh_ui_binary`].
fn build_menu(
    ui_available: bool,
) -> Result<(Menu, MenuItem, Submenu, MenuItem, tray_icon::menu::MenuId)> {
    let menu = Menu::new();
    let open = MenuItem::new(open_item_label(ui_available), ui_available, None);
    // Starts empty and disabled; the first registry snapshot fills it. The
    // daemon builds the registry after the tray is already up, so an empty
    // submenu is the honest state until then rather than a missing one.
    let databases = Submenu::new("Databases", false);
    let status = MenuItem::new("Status: 0 active calls", false, None);
    let quit = MenuItem::new("Quit", true, None);

    menu.append(&open).context("append Open item")?;
    menu.append(&databases)
        .context("append Databases submenu")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("append separator")?;
    menu.append(&status).context("append Status item")?;
    menu.append(&PredefinedMenuItem::separator())
        .context("append separator")?;
    menu.append(&quit).context("append Quit item")?;

    Ok((menu, status, databases, open, quit.id().clone()))
}

fn load_icon() -> Result<Icon> {
    let image = image::load_from_memory(TRAY_ICON_BYTES)
        .context("decode embedded tray icon")?
        .into_rgba8();
    let (w, h) = image.dimensions();
    Icon::from_rgba(image.into_raw(), w, h).context("build tray Icon from RGBA buffer")
}

/// Standard install locations for the UI binary, in the order they are tried
/// after `NODESPACE_UI_BINARY`. Split out as a pure function so the fallback
/// list is assertable in tests without touching the real filesystem.
///
/// macOS covers both the system-wide install (`/Applications`, what the .pkg
/// and the Homebrew cask write) and a per-user copy under `~/Applications`,
/// which is where a user who drags the app out of the .dmg without admin
/// rights ends up. Linux has no single canonical location for a Tauri
/// AppImage/deb payload, so only the deb/rpm prefix is listed. Windows uses
/// the per-user install root Tauri's NSIS/MSI bundles default to.
fn ui_binary_install_candidates(home: Option<&Path>) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        const BUNDLE_REL: &str = "NodeSpace.app/Contents/MacOS/nodespace-app";
        let mut candidates = vec![PathBuf::from("/Applications").join(BUNDLE_REL)];
        if let Some(home) = home {
            candidates.push(home.join("Applications").join(BUNDLE_REL));
        }
        candidates
    }

    #[cfg(target_os = "linux")]
    {
        let _ = home;
        vec![
            PathBuf::from("/usr/bin/nodespace-app"),
            PathBuf::from("/usr/local/bin/nodespace-app"),
        ]
    }

    #[cfg(target_os = "windows")]
    {
        home.map(|home| {
            home.join("AppData")
                .join("Local")
                .join("NodeSpace")
                .join("nodespace-app.exe")
        })
        .into_iter()
        .collect()
    }

    // A target with no known install location returns an empty list rather
    // than failing to compile. Without this arm every block above is stripped
    // on a fourth target, the body evaluates to `()` where `Vec<PathBuf>` is
    // declared, and the reader gets a type error from inside a `#[cfg]` maze
    // instead of the honest answer, which "nowhere to look" already is.
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = home;
        Vec::new()
    }
}

/// Resolve the Tauri UI binary path for the tray's "Open NodeSpace" item.
///
/// `NODESPACE_UI_BINARY` wins when set, so dev builds and packaged installs
/// can point at different artifacts without recompiling — but it is no longer
/// the only source. The app writes that variable into the launchd plist /
/// systemd unit it installs, and the daemon therefore loses it whenever it is
/// restarted from a job definition that predates the variable: `launchctl
/// kickstart` (the last fallback in the app's bootstrap) restarts the
/// *already-loaded* job rather than re-reading the plist, so an upgrade can
/// leave a perfectly healthy daemon running with no way to find the GUI. The
/// install-location fallback makes that recoverable without another restart.
///
/// Every candidate is checked for existence — a path that does not resolve to
/// a real file is worse than `None`, because it would leave the menu item
/// enabled and then fail at spawn time. `None` (nothing set, nothing
/// installed) is still the correct answer for tests and headless daemon runs,
/// and the caller greys the menu item out rather than leaving it inert.
fn resolve_ui_binary() -> Option<PathBuf> {
    if let Some(from_env) = std::env::var_os("NODESPACE_UI_BINARY") {
        let path = PathBuf::from(from_env);
        if path.is_file() {
            return Some(path);
        }
        // A stale env var — the app moved or was uninstalled since the service
        // definition was written — is worth saying out loud, since it is the
        // one case where the configured answer and the working answer differ.
        tracing::warn!(
            path = %path.display(),
            "NODESPACE_UI_BINARY points at a missing file; falling back to install locations"
        );
    }

    let home = dirs::home_dir();
    ui_binary_install_candidates(home.as_deref())
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Run the tray on the calling thread. **Must be the main thread on macOS.**
///
/// `seed_controller` is invoked synchronously *before* the event loop starts,
/// giving the caller a handle they can hand to the gRPC server (which runs
/// on a separate runtime). The value returned by `seed_controller` is handed
/// back from `run` once "Quit" is selected, so the caller can await any
/// resources it created at seed time (e.g. a gRPC `JoinHandle`).
///
/// Uses `event_loop.run_return` rather than `event_loop.run`: tao's `run`
/// calls `process::exit(0)` on macOS at `ControlFlow::Exit`, which would
/// kill the daemon before the gRPC server finishes draining. `run_return`'s
/// documented caveat (it may not return mid-window-resize) doesn't apply —
/// the daemon has no window, only a tray icon.
pub fn run<T>(seed_controller: impl FnOnce(TrayController) -> T) -> Result<T> {
    use tao::platform::run_return::EventLoopExtRunReturn;

    let mut event_loop: EventLoop<TrayEvent> = EventLoopBuilder::with_user_event().build();

    // Hide from the macOS dock and app switcher — nodespaced is a background
    // agent, not a foreground app. Must be set before the event loop starts.
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();

    // Forward muda's global menu channel into our tao loop. Without this the
    // menu clicks are queued in `MenuEvent::receiver()` and never observed.
    let menu_proxy = proxy.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(TrayEvent::Menu(event));
    }));

    let active_rpcs = Arc::new(AtomicUsize::new(0));
    let quit_notify = Arc::new(tokio::sync::Notify::new());

    let seeded = seed_controller(TrayController {
        proxy: proxy.clone(),
        quit_notify: quit_notify.clone(),
        active_rpcs: active_rpcs.clone(),
    });

    let ui_binary = resolve_ui_binary();
    let mut state: Option<TrayState> = None;
    // The registry is built after the tray loop starts, so a snapshot can
    // arrive before `Init`. Hold the most recent one and apply it as soon as
    // there is a tray to apply it to.
    let mut pending_databases: Option<RegistrySnapshot> = None;

    event_loop.run_return(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(tao::event::StartCause::Init) => {
                match initialize_tray(ui_binary.clone()) {
                    Ok(mut s) => {
                        if let Some(snapshot) = pending_databases.take() {
                            s.rebuild_databases_menu(&snapshot);
                        }
                        state = Some(s);
                    }
                    Err(e) => {
                        tracing::error!(
                            error = ?e,
                            "Failed to initialize system tray; daemon will run without tray"
                        );
                        // Don't exit the loop — gRPC is still serving. The
                        // user can shut down via SIGTERM as before.
                    }
                }
            }

            Event::UserEvent(TrayEvent::Menu(menu_event)) => {
                let Some(s) = state.as_mut() else { return };
                if menu_event.id == s.open_id {
                    if let Err(e) = s.open_ui(None) {
                        tracing::error!(error = ?e, "Failed to spawn UI binary");
                    }
                } else if let Some(database) = s.database_for_menu_id(&menu_event.id) {
                    tracing::info!(database = %database.as_str(), "Tray: opening UI on database");
                    if let Err(e) = s.open_ui(Some(database)) {
                        tracing::error!(error = ?e, "Failed to spawn UI binary");
                    }
                } else if menu_event.id == s.quit_id {
                    tracing::info!("Tray Quit selected — initiating shutdown");
                    // Re-resolve immediately before use, same discipline as
                    // `open_ui` -- the daemon can outlive any single state of
                    // the filesystem, so a path cached at tray-build time
                    // could be stale by the time the user actually clicks
                    // Quit.
                    s.refresh_ui_binary();
                    signal_ui_to_quit(ui_pid_path().as_deref(), s.ui_binary.as_deref());
                    // `notify_waiters` wakes only currently-registered waiters.
                    // The gRPC server's `shutdown().await` is registered at
                    // server-build time (synchronously inside the seed closure
                    // above), so it's guaranteed to be parked here before the
                    // user can click Quit. New consumers of `shutdown()` must
                    // be registered with the same lifetime discipline.
                    quit_notify.notify_waiters();
                    *control_flow = ControlFlow::Exit;
                }
            }

            Event::UserEvent(TrayEvent::DatabasesChanged(snapshot)) => match state.as_mut() {
                Some(s) => s.rebuild_databases_menu(&snapshot),
                // Tray not up yet (or failed to initialize) — keep the latest.
                None => pending_databases = Some(*snapshot),
            },

            Event::UserEvent(TrayEvent::RpcStateChanged) => {
                if let Some(s) = state.as_ref() {
                    let count = active_rpcs.load(Ordering::Relaxed);
                    s.status_item
                        .set_text(format!("Status: {count} active calls"));
                }
            }

            Event::UserEvent(TrayEvent::GrpcTaskFinished) => {
                // The gRPC task is already stopped by the time this arrives
                // -- unlike the Quit branch above, there's nothing left to
                // wake via `quit_notify`, just the loop itself to exit.
                tracing::info!("gRPC task finished outside of tray Quit — shutting down tray loop");
                *control_flow = ControlFlow::Exit;
            }

            _ => {}
        }
    });

    Ok(seeded)
}

fn initialize_tray(ui_binary: Option<PathBuf>) -> Result<TrayState> {
    let icon = load_icon()?;
    let (menu, status_item, databases_menu, open_item, quit_id) = build_menu(ui_binary.is_some())?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("NodeSpace")
        .with_icon(icon)
        .build()
        .context("build TrayIcon")?;

    let open_id = open_item.id().clone();
    Ok(TrayState {
        _tray: tray,
        status_item,
        ui_binary,
        open_item,
        open_id,
        quit_id,
        databases_menu,
        database_items: Vec::new(),
    })
}

impl TrayState {
    /// Re-run [`resolve_ui_binary`] and bring the "Open NodeSpace" item's
    /// label and enabled state back in line with the answer.
    ///
    /// The daemon long outlives any single state of the filesystem — a
    /// LaunchAgent starts it at login, which can easily be before the app
    /// reaches /Applications on a first install, and it keeps running while
    /// the app is moved, upgraded or reinstalled underneath it. Resolving once
    /// at startup would freeze whichever answer happened to be true then, so a
    /// daemon that came up too early would grey the item out permanently even
    /// after the app appeared.
    ///
    /// Returns whether a UI binary is currently available, so callers that
    /// need the answer don't have to re-inspect the field.
    fn refresh_ui_binary(&mut self) -> bool {
        let resolved = resolve_ui_binary();
        let available = resolved.is_some();

        // Only touch the native menu item when the answer actually changed:
        // this runs on every click and every registry snapshot, and a no-op
        // set_text/set_enabled pair on each one is needless platform churn.
        if resolved != self.ui_binary {
            self.open_item.set_text(open_item_label(available));
            self.open_item.set_enabled(available);
            self.ui_binary = resolved;
        }
        available
    }

    /// Replace the Databases submenu contents with `snapshot`.
    ///
    /// Removes the previously-appended items rather than the whole submenu, so
    /// the submenu's own handle (held by the live menu) stays valid.
    fn rebuild_databases_menu(&mut self, snapshot: &RegistrySnapshot) {
        // Clear the tracking list unconditionally, whatever removal reports: an
        // item left in the native submenu but dropped from the list is
        // unreachable (clicks stop resolving) AND gets a duplicate appended
        // beside it on the next rebuild. Forgetting one is the worse failure,
        // so log and keep going rather than bail mid-rebuild.
        for (item, _) in self.database_items.iter() {
            if let Err(e) = self.databases_menu.remove(item) {
                tracing::warn!(error = ?e, "Failed to remove a stale database menu item");
            }
        }
        self.database_items.clear();

        // Every database item opens the UI, so without a UI binary they are as
        // inert as "Open NodeSpace" would be — disable them for the same
        // reason, on top of whatever the entry itself says about the database.
        // Re-resolving here (rather than reading the startup snapshot) is also
        // what refreshes "Open NodeSpace" itself for a daemon that is never
        // clicked but does receive registry snapshots.
        let ui_available = self.refresh_ui_binary();
        let mut failed = 0usize;
        for entry in database_menu_entries(snapshot) {
            let item = MenuItem::new(&entry.label, entry.enabled && ui_available, None);
            match self.databases_menu.append(&item) {
                Ok(()) => self.database_items.push((item, entry.id)),
                Err(e) => {
                    failed += 1;
                    tracing::warn!(error = ?e, "Failed to append a database menu item");
                }
            }
        }
        if failed > 0 {
            tracing::warn!(failed, "Databases submenu is incomplete");
        }

        // Computed on the way out, not only on the success path, so a partial
        // rebuild never leaves reachable items behind a greyed-out parent.
        self.databases_menu
            .set_enabled(!self.database_items.is_empty());
    }

    /// The database a menu id belongs to, if it is one of ours.
    fn database_for_menu_id(&self, id: &tray_icon::menu::MenuId) -> Option<DatabaseId> {
        self.database_items
            .iter()
            .find(|(item, _)| item.id() == id)
            .map(|(_, db)| db.clone())
    }

    /// Spawn the Tauri UI binary, optionally pointing it at `database`.
    ///
    /// Always spawns — the daemon has no reliable way to know whether a UI
    /// process is already alive: a child it spawned itself may have exited
    /// without the tray noticing, and one launched outside the tray (Dock
    /// icon, Spotlight, a previous run) leaves no child here to check at all.
    /// Rather than track liveness in the daemon, dedup is delegated to the app
    /// itself via `tauri-plugin-single-instance`: a genuinely-second launch
    /// detects the running instance, forwards its argv to it, and exits
    /// immediately, so a real UI process almost never survives alongside an
    /// existing one regardless of how many times this is called or how the
    /// first instance was started. Not an absolute guarantee on every
    /// platform: the plugin's macOS backend claims the single-instance lock
    /// with a connect-then-bind probe rather than one atomic OS primitive (Windows
    /// uses a named mutex, Linux a D-Bus name claim — both atomic), so two
    /// processes launched within that probe's scheduling gap can theoretically
    /// both see "nobody's home" and both survive. Narrow and outside this
    /// crate's control; documented here so it isn't mistaken for a promise
    /// this code makes.
    ///
    /// The requested database is passed two ways so either launch path can
    /// read it: as `INITIAL_DATABASE_ENV`, which a *cold* launch (no other
    /// instance running) reads at startup, and as a `--database <id>` CLI
    /// argument, which is what `tauri-plugin-single-instance` forwards to an
    /// *already-running* instance's relaunch handler — that handler has no
    /// visibility into the new process's environment, only its argv. The
    /// already-running instance focuses its window and switches to the
    /// requested database in place on receiving it.
    fn open_ui(&mut self, database: Option<DatabaseId>) -> Result<()> {
        // Re-resolve before using the startup snapshot. The daemon outlives
        // any single state of the filesystem: it is started at login by a
        // LaunchAgent that can easily come up before the app is in
        // /Applications, and it survives the app being moved, reinstalled or
        // upgraded underneath it. A path cached at startup would be wrong in
        // exactly those cases, and re-checking costs one `stat` per click.
        //
        // The returned availability flag is deliberately dropped: this needs
        // the resolved *path*, which it takes from the field below, not the
        // yes/no the menu-rebuild caller wants.
        let _ = self.refresh_ui_binary();

        // Reachable, despite the menu items being disabled whenever
        // `ui_binary` is `None`: the refresh above can itself discover that
        // the app has just disappeared, in the window between the menu being
        // built and this click being handled. Rare, but a real race rather
        // than dead code — hence a hard error, not the `Ok(())` this used to
        // return, which swallowed the failure into a successful-looking no-op
        // and left the caller with nothing to report.
        let path = self.ui_binary.as_ref().context(
            "no NodeSpace UI binary found: NODESPACE_UI_BINARY is unset or stale and the app \
             is not installed in a standard location",
        )?;

        let mut command = build_ui_command(path, database.as_ref());
        let mut child = command
            .spawn()
            .with_context(|| format!("spawn UI binary {}", path.display()))?;

        // Reap on a plain OS thread rather than the tao event loop: a relaunch
        // that the app's own single-instance guard dedupes exits almost
        // immediately, and nothing else in this process ever calls `wait` on
        // it, so an unreaped child would sit as a zombie for the rest of the
        // daemon's lifetime (its parent). `Child` is `Send`, so this never
        // blocks tray event handling.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// Build the command to launch the UI binary, optionally requesting it open on
/// `database`. Split out from [`TrayState::open_ui`] so the exact args/env are
/// assertable in tests without actually spawning a process.
fn build_ui_command(path: &Path, database: Option<&DatabaseId>) -> Command {
    let mut command = Command::new(path);
    if let Some(id) = database {
        // Only set when the user picked a specific database, so a plain
        // "Open NodeSpace" still honours whatever the app last remembered.
        command.env(INITIAL_DATABASE_ENV, id.as_str());
        command.arg("--database").arg(id.as_str());
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::database_manager::{DatabaseEntry, DatabaseListing};

    fn listing_with_default(
        name: &str,
        status: DatabaseStatus,
        tenant: Option<&str>,
        is_default: bool,
    ) -> DatabaseListing {
        DatabaseListing {
            entry: DatabaseEntry {
                id: DatabaseId::from(format!("id-{name}")),
                name: name.to_string(),
                path: PathBuf::from(format!("/tmp/{name}.db")),
                created_at: chrono::Utc::now(),
                last_opened_at: None,
                bound_tenant_schema: tenant.map(str::to_string),
                bound_tenant_collection: None,
            },
            status,
            is_default,
        }
    }

    fn listing(name: &str, status: DatabaseStatus, tenant: Option<&str>) -> DatabaseListing {
        listing_with_default(name, status, tenant, false)
    }

    fn snapshot(databases: Vec<DatabaseListing>) -> RegistrySnapshot {
        RegistrySnapshot {
            databases,
            default_database: None,
        }
    }

    /// The two facts the submenu conveys — which database is the default, and
    /// which sync — with a plain name when neither applies.
    #[test]
    fn labels_carry_default_and_synced_state() {
        let entries = database_menu_entries(&snapshot(vec![
            listing_with_default("Both", DatabaseStatus::Closed, Some("tenant_demo"), true),
            listing_with_default("DefaultOnly", DatabaseStatus::Closed, None, true),
            listing("SyncedOnly", DatabaseStatus::Closed, Some("tenant_demo")),
            listing("Plain", DatabaseStatus::Closed, None),
        ]));

        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "Both — default · synced",
                "DefaultOnly — default",
                "SyncedOnly — synced",
                "Plain",
            ]
        );
        assert!(entries.iter().all(|e| e.enabled));
    }

    /// Which database is open now IS shown. It was omitted while the submenu was
    /// built once at boot, because the idle reaper closes databases and in-app
    /// switching opens others — a marker pushed once would be wrong within minutes
    /// of normal use. The menu is rebuilt on every registry/open-set change now,
    /// so the marker tracks reality.
    #[test]
    fn open_state_is_reflected_in_the_label() {
        let entries = database_menu_entries(&snapshot(vec![
            listing("Alpha", DatabaseStatus::Closed, None),
            listing("Beta", DatabaseStatus::Open, None),
        ]));

        assert_eq!(entries[0].label, "Alpha");
        assert_eq!(entries[1].label, "Beta — open");
    }

    /// Marker order is fixed so a label does not reshuffle between refreshes,
    /// which under live updates would read as flicker rather than information.
    #[test]
    fn markers_render_in_a_stable_order() {
        let entries = database_menu_entries(&snapshot(vec![listing_with_default(
            "All",
            DatabaseStatus::Open,
            Some("tenant_demo"),
            true,
        )]));

        assert_eq!(entries[0].label, "All — default · open · synced");
    }

    /// A missing entry never gains an open marker: its file is gone, so "open"
    /// would be nonsense even though the status enum can only hold one value.
    #[test]
    fn missing_beats_every_other_marker() {
        let entries = database_menu_entries(&snapshot(vec![listing_with_default(
            "Gone",
            DatabaseStatus::Missing,
            Some("tenant_demo"),
            true,
        )]));

        assert_eq!(entries[0].label, "Gone — missing");
        assert!(!entries[0].enabled);
    }

    /// A registry entry whose file is gone is still listed — silently dropping it
    /// would leave the user wondering where the database went — but it cannot be
    /// opened, and neither open nor synced is meaningful for it.
    #[test]
    fn missing_database_is_shown_but_not_selectable() {
        let entries = database_menu_entries(&snapshot(vec![listing(
            "Gone",
            DatabaseStatus::Missing,
            Some("tenant_demo"),
        )]));

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Gone — missing");
        assert!(!entries[0].enabled);
    }

    /// Registry order is preserved and every entry keeps its own id, so a click
    /// opens the database the user actually picked.
    #[test]
    fn entries_keep_registry_order_and_ids() {
        let entries = database_menu_entries(&snapshot(vec![
            listing("First", DatabaseStatus::Closed, None),
            listing("Second", DatabaseStatus::Closed, None),
        ]));

        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["id-First", "id-Second"]);
    }

    #[test]
    fn empty_registry_renders_no_entries() {
        assert!(database_menu_entries(&snapshot(vec![])).is_empty());
    }

    #[test]
    fn embedded_icon_decodes() {
        // Catches the common breakage where someone replaces the icon with a
        // non-PNG or a zero-byte file: `load_icon` exists precisely to bail
        // out before the event loop swallows the failure.
        let icon = load_icon().expect("embedded tray icon must decode");
        // Sanity check: tray-icon doesn't let us read back the size, but the
        // icon function would have errored on an empty rgba buffer.
        drop(icon);
    }

    /// Every `resolve_ui_binary` case that touches the process env lives in
    /// this one test: parallel tests share the process env, so splitting them
    /// into separate `#[test]` fns would let them race and flake.
    ///
    /// Two properties, both load-bearing:
    ///
    /// 1. The env var must name a real file to win. The old contract accepted
    ///    any string, which is what let a stale variable — the app moved or
    ///    was uninstalled since the service definition was written — resolve
    ///    to a path that then failed at spawn time, behind an enabled menu
    ///    item.
    /// 2. Resolution re-reads the filesystem on every call rather than
    ///    memoizing. A daemon started at login by a LaunchAgent routinely
    ///    comes up before the app is in place on a first install; if the "no
    ///    UI binary" answer stuck, its tray would stay greyed out for the rest
    ///    of the session even once the app appeared.
    #[test]
    fn resolve_ui_binary_requires_the_env_var_to_name_a_real_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ui = dir.path().join("nodespace-app");

        // Same path, twice, either side of the file coming into existence.
        std::env::set_var("NODESPACE_UI_BINARY", &ui);
        let before_it_exists = resolve_ui_binary();
        std::fs::write(&ui, b"#!/bin/sh\n").expect("write fake UI binary");
        let after_it_exists = resolve_ui_binary();

        std::env::remove_var("NODESPACE_UI_BINARY");

        assert_ne!(
            before_it_exists.as_deref(),
            Some(ui.as_path()),
            "a stale env var must never be returned — it would leave the menu item enabled \
             and fail only at spawn time"
        );
        assert_eq!(
            after_it_exists.as_deref(),
            Some(ui.as_path()),
            "an env var naming a real file must win outright, and the same path must start \
             resolving as soon as the file exists — resolution cannot be memoized, or a \
             daemon that started before the app was installed stays broken all session"
        );
    }

    /// The fallback list is what makes the tray survive a daemon restarted
    /// without the plist's `EnvironmentVariables` — a `launchctl kickstart`
    /// restarts the already-loaded job and never re-reads the plist, so the
    /// variable can simply be absent from an otherwise healthy daemon.
    #[test]
    fn install_candidates_cover_the_standard_locations() {
        let home = PathBuf::from("/Users/example");
        let candidates = ui_binary_install_candidates(Some(&home));

        // Scoped to the three platforms that actually ship: a target with no
        // known install location legitimately returns an empty list.
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        assert!(
            !candidates.is_empty(),
            "every shipping platform needs at least one install location to fall back to"
        );
        assert!(
            candidates.iter().all(|c| c.is_absolute()),
            "candidates are probed with `is_file()` from the daemon's own working \
             directory, so a relative path would resolve unpredictably: {candidates:?}"
        );

        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                candidates,
                vec![
                    PathBuf::from("/Applications/NodeSpace.app/Contents/MacOS/nodespace-app"),
                    home.join("Applications/NodeSpace.app/Contents/MacOS/nodespace-app"),
                ],
                "the system-wide install must be preferred over a per-user copy"
            );
        }
    }

    /// Without a home directory the list must degrade rather than panic or
    /// produce a path rooted at nothing.
    #[test]
    fn install_candidates_without_a_home_dir_stay_absolute() {
        let candidates = ui_binary_install_candidates(None);
        assert!(candidates.iter().all(|c| c.is_absolute()));
    }

    /// The heart of the fix: a resolvable UI binary leaves the item clickable,
    /// and an unresolvable one greys it out AND says why. An enabled item that
    /// silently does nothing is the exact failure this replaces.
    #[test]
    fn open_item_is_labelled_with_the_reason_when_no_ui_binary_exists() {
        assert_eq!(open_item_label(true), "Open NodeSpace");

        let unavailable = open_item_label(false);
        assert_ne!(
            unavailable, "Open NodeSpace",
            "a disabled item must not read identically to a working one — the label is the \
             only channel the tray has to explain why the action is unavailable"
        );
        assert!(
            unavailable.contains("not installed"),
            "the label should name the reason, got {unavailable:?}"
        );
    }

    /// A plain "Open NodeSpace" (no database requested) must not set the env
    /// var or the CLI flag — otherwise a cold launch would misread it as a
    /// database pick and override whatever the app last remembered.
    #[test]
    fn build_ui_command_without_database_sets_neither_env_nor_arg() {
        let command = build_ui_command(Path::new("/opt/nodespace/ui"), None);

        assert!(command.get_envs().all(|(k, _)| k != INITIAL_DATABASE_ENV));
        assert!(command.get_args().next().is_none());
    }

    /// Picking a specific database must set both the env var (read by a cold
    /// launch at startup) and the `--database <id>` CLI flag (forwarded by
    /// `tauri-plugin-single-instance` to an already-running instance, which has
    /// no visibility into the new process's environment) — one code path
    /// handles both launch scenarios without the tray knowing which applies.
    #[test]
    fn build_ui_command_with_database_sets_env_and_cli_arg() {
        let id = DatabaseId::from("db-123".to_string());
        let command = build_ui_command(Path::new("/opt/nodespace/ui"), Some(&id));

        let env_value = command
            .get_envs()
            .find(|(k, _)| *k == INITIAL_DATABASE_ENV)
            .and_then(|(_, v)| v);
        assert_eq!(env_value, Some(std::ffi::OsStr::new("db-123")));

        let args: Vec<&std::ffi::OsStr> = command.get_args().collect();
        assert_eq!(args, vec!["--database", "db-123"]);
    }
}

/// Coverage for [`verified_ui_pid`] — the read-only lookup half of the
/// "tray Quit also closes the UI window" signal path
/// ([`signal_ui_to_quit`]). Every case here uses this test binary's own,
/// real pid: safe because these functions only ever *read* process state
/// (`std::fs::read_to_string`, `ps`), never signal anything — the actual
/// `kill`/`taskkill` call in `signal_ui_to_quit` is compiled out under
/// `cfg(test)` and is not exercised here (see that function's doc comment).
#[cfg(all(test, unix))]
mod verified_ui_pid_tests {
    use super::verified_ui_pid;

    #[test]
    fn finds_the_pid_when_the_file_and_binary_both_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("ui.pid");
        let this_pid = std::process::id();
        let this_exe = std::env::current_exe().expect("current_exe");
        std::fs::write(&pid_file, this_pid.to_string()).expect("write pid file");

        assert_eq!(verified_ui_pid(&pid_file, &this_exe), Some(this_pid as i32));
    }

    #[test]
    fn rejects_a_pid_file_whose_binary_does_not_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("ui.pid");
        std::fs::write(&pid_file, std::process::id().to_string()).expect("write pid file");

        assert_eq!(
            verified_ui_pid(&pid_file, std::path::Path::new("/not/the/real/binary")),
            None,
            "a real, live pid must still be rejected if its argv0 doesn't match"
        );
    }

    #[test]
    fn returns_none_for_a_missing_pid_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("does-not-exist.pid");
        let this_exe = std::env::current_exe().expect("current_exe");

        assert_eq!(verified_ui_pid(&pid_file, &this_exe), None);
    }

    #[test]
    fn returns_none_for_unparsable_pid_file_contents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("ui.pid");
        std::fs::write(&pid_file, "not-a-pid").expect("write pid file");
        let this_exe = std::env::current_exe().expect("current_exe");

        assert_eq!(verified_ui_pid(&pid_file, &this_exe), None);
    }

    /// A pid that (almost certainly) belongs to no live process at all —
    /// distinct from "belongs to a live process with the wrong argv0".
    #[test]
    fn returns_none_for_a_pid_with_no_live_process() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("ui.pid");
        // PID 1 is always `init`/`launchd` on a real Unix system, never this
        // test binary — using it (rather than a made-up huge number that
        // could coincidentally be unassigned but still "exist" in some
        // container's pid namespace oddities) keeps this deterministic.
        std::fs::write(&pid_file, "1").expect("write pid file");
        let this_exe = std::env::current_exe().expect("current_exe");

        assert_eq!(verified_ui_pid(&pid_file, &this_exe), None);
    }
}

/// Pins the "tray Quit also closes the UI window" wiring at the call site:
/// `run`'s live `tao` event loop can't be driven in a unit test (no real
/// display), so this asserts the source itself calls [`signal_ui_to_quit`]
/// from the `quit_id` branch — the same source-text-assertion precedent
/// `nodespace-app`'s `quit_wiring_tests` module uses for its mirror-image
/// half of this fix.
#[cfg(test)]
mod quit_signals_ui_tests {
    #[test]
    fn tray_quit_signals_the_ui_process_to_close() {
        let source = include_str!("tray.rs");
        let marker = source
            .find("== s.quit_id {")
            .expect("quit_id menu branch not found in tray.rs");
        let window = &source[marker..(marker + 700).min(source.len())];

        assert!(
            window.contains("signal_ui_to_quit(ui_pid_path().as_deref(), s.ui_binary.as_deref())"),
            "the tray's \"Quit\" item must call signal_ui_to_quit so the running desktop app's \
             window also closes, not just the daemon: {window}"
        );
    }
}

/// `tower::Layer` that bumps the tray's "active calls" counter for the
/// duration of every RPC. Wrapping the gRPC service this way means the
/// service implementations don't need to know the tray exists.
pub mod layer {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tower::{Layer, Service};

    use super::TrayController;

    #[derive(Clone)]
    pub struct TrayMetricsLayer {
        controller: TrayController,
    }

    impl TrayMetricsLayer {
        pub fn new(controller: TrayController) -> Self {
            Self { controller }
        }
    }

    impl<S> Layer<S> for TrayMetricsLayer {
        type Service = TrayMetrics<S>;

        fn layer(&self, inner: S) -> Self::Service {
            TrayMetrics {
                inner,
                controller: self.controller.clone(),
            }
        }
    }

    #[derive(Clone)]
    pub struct TrayMetrics<S> {
        inner: S,
        controller: TrayController,
    }

    impl<S, Req> Service<Req> for TrayMetrics<S>
    where
        S: Service<Req> + Clone + Send + 'static,
        S::Future: Send + 'static,
        Req: Send + 'static,
    {
        type Response = S::Response;
        type Error = S::Error;
        type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.inner.poll_ready(cx)
        }

        fn call(&mut self, req: Req) -> Self::Future {
            self.controller.rpc_started();
            // tower's contract: `call` may be invoked again before the
            // previous future resolves, so move the readied service into the
            // future and leave a fresh clone in `self.inner`. We clone first
            // (a separate binding) to avoid an immutable + mutable borrow.
            let clone = self.inner.clone();
            let mut inner = std::mem::replace(&mut self.inner, clone);
            let controller = self.controller.clone();
            Box::pin(async move {
                let result = inner.call(req).await;
                controller.rpc_completed();
                result
            })
        }
    }
}
