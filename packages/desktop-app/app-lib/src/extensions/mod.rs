//! Extension points for an app crate built on this library.
//!
//! An app crate outside core can depend on this library and contribute to the
//! desktop app it builds. Everything an extension contributes to Tauri arrives
//! as a [Tauri plugin](tauri::plugin::Plugin): commands, managed state, setup
//! and run-event handling all travel inside it. A `tauri::Builder` holds only
//! one invoke handler and one setup closure, and a second call replaces the
//! first, so extension commands cannot be entries in core's own command list.
//!
//! [`AppExtensions`] collects what an extension contributes and [`assemble`]
//! applies it to a `tauri::Builder`. An app with no extension, built from
//! [`AppExtensions::none`], behaves exactly as core alone does.

use std::collections::HashSet;

use tauri::plugin::Plugin;
use tauri::{Builder, Runtime};

#[cfg(test)]
mod fixture_tests;

/// The version of the extension API, as `(major, minor)`.
///
/// A breaking change to the API bumps the major number and an additive change
/// bumps the minor number. The TypeScript host API carries the same value, and
/// the two must stay equal.
pub const EXTENSION_API_VERSION: (u32, u32) = (1, 0);

/// Core's own Tauri plugins, in the order they are registered.
///
/// This is the only list of them. [`register_core_plugins`] applies it to the
/// builder `run` starts from, and [`core_plugin_names`] reads the names off it
/// for [`assemble`], which skips an extension plugin that reuses one. A plugin
/// registered under an existing name replaces the earlier one without a
/// warning, so an extension plugin with a core plugin's name would replace
/// core's. `run` registers no plugin of its own, so a plugin added here is
/// protected the moment it exists, with no second list to update.
///
/// The single-instance plugin comes first, because it must be registered
/// before any other plugin.
pub(crate) fn core_plugins<R: Runtime>() -> Vec<Box<dyn Plugin<R>>> {
    let mut plugins: Vec<Box<dyn Plugin<R>>> = Vec::new();

    // The daemon's tray always spawns a fresh UI process when a database is
    // picked (see `nodespace_daemon::tray::TrayState::open_ui`) — it has no
    // reliable way to know whether a UI process from an earlier launch is still
    // alive, and none at all when the app was started outside the tray. Rather
    // than the daemon tracking liveness, this plugin makes any second launch
    // detect the running instance, forward its argv to it, and exit
    // immediately, so a real UI process almost never survives alongside an
    // existing one regardless of launch path. Note this plugin's macOS backend
    // claims the lock with a connect-then-bind probe rather than one atomic OS
    // primitive (unlike its Windows/Linux backends), so it is not an absolute
    // guarantee there — see the doc comment on `TrayState::open_ui` in the
    // daemon crate.
    #[cfg(desktop)]
    plugins.push(Box::new(tauri_plugin_single_instance::init::<R, _>(
        |app, argv, _cwd| {
            crate::handle_relaunch(app, &argv);
        },
    )));

    plugins.push(Box::new(tauri_plugin_opener::init::<R>()));
    plugins.push(Box::new(tauri_plugin_dialog::init::<R>()));
    plugins
}

/// The names of [`core_plugins`], in registration order.
pub(crate) fn core_plugin_names<R: Runtime>() -> Vec<&'static str> {
    core_plugins::<R>()
        .iter()
        .map(|plugin| plugin.name())
        .collect()
}

/// Registers [`core_plugins`] on `builder`, in order.
pub(crate) fn register_core_plugins<R: Runtime>(builder: Builder<R>) -> Builder<R> {
    core_plugins::<R>()
        .into_iter()
        .fold(builder, |builder, plugin| builder.plugin_boxed(plugin))
}

/// What an extension contributes to the desktop app.
///
/// Build one with [`AppExtensions::none`] and add contributions with the
/// methods below, then hand it to [`assemble`]. The runtime parameter is
/// `tauri::Wry` unless a test picks another runtime.
pub struct AppExtensions<R: Runtime = tauri::Wry> {
    plugins: Vec<Box<dyn Plugin<R>>>,
}

impl<R: Runtime> AppExtensions<R> {
    /// An extension that contributes nothing.
    #[must_use]
    pub fn none() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Adds a Tauri plugin. Plugins are registered in the order they were added.
    ///
    /// A plugin's setup runs before core's own setup, so it must not assume the
    /// daemon connection, the shutdown token or a running daemon exist yet.
    #[must_use]
    pub fn plugin<P: Plugin<R> + 'static>(mut self, plugin: P) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }
}

// Not derived: `#[derive(Default)]` would require `R: Default`, and a runtime
// type has no reason to implement it.
impl<R: Runtime> Default for AppExtensions<R> {
    fn default() -> Self {
        Self::none()
    }
}

/// Applies an extension's contributions to a `tauri::Builder`.
///
/// This is the extension half of building the app. It registers each extension
/// plugin, in the order it was added, and nothing else. Core's own plugins,
/// commands and setup are applied by the desktop entry point, `run`, around
/// this call.
///
/// # Plugins
///
/// * The caller registers core's own plugins on `builder` first, with
///   [`register_core_plugins`], which puts the single-instance plugin before
///   any other, as that plugin requires. `assemble` registers the extension
///   plugins after them.
/// * A plugin's setup runs inside `Builder::build`, before core's setup.
/// * A plugin's commands are invoked from the frontend as
///   `plugin:<name>|<command>`. Tauri always checks them against the
///   capabilities, so the app crate that owns the capability files must grant
///   them there. Core's capabilities never name an extension's commands.
/// * A plugin whose name is one of core's own plugins, or that reuses the name
///   of an earlier extension plugin, is not registered and is logged as an
///   error. Tauri would otherwise let the later plugin silently replace the
///   earlier one. The first registration wins.
///
/// # Why this is generic over the runtime
///
/// Core's own commands take the concrete `tauri::Wry` handle types, and core's
/// setup starts the real daemon, so neither can run on a mock runtime.
/// `assemble` touches neither, which lets tests drive everything an extension
/// contributes through `tauri::test::MockRuntime`.
#[must_use]
pub fn assemble<R: Runtime>(mut builder: Builder<R>, extensions: AppExtensions<R>) -> Builder<R> {
    let core_names = core_plugin_names::<R>();
    let mut registered: HashSet<&'static str> = HashSet::new();
    for plugin in extensions.plugins {
        let name = plugin.name();
        if core_names.contains(&name) {
            tracing::error!(
                plugin = name,
                "extension plugin skipped: its name belongs to a plugin core registers"
            );
            continue;
        }
        if !registered.insert(name) {
            tracing::error!(
                plugin = name,
                "extension plugin skipped: an earlier extension plugin already uses its name"
            );
            continue;
        }
        builder = builder.plugin_boxed(plugin);
    }
    builder
}
