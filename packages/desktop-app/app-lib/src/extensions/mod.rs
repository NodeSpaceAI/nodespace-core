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

/// Names of the Tauri plugins core registers itself.
///
/// A plugin registered under an existing name replaces the earlier one without
/// a warning, so an extension plugin that reused one of these names would
/// replace core's plugin. [`assemble`] skips such a plugin instead.
pub(crate) const CORE_PLUGIN_NAMES: &[&str] = &["single-instance", "opener", "dialog"];

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
/// * The caller registers core's own plugins on `builder` first, with the
///   single-instance plugin before any other, as that plugin requires.
///   `assemble` registers the extension plugins after them.
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
    let mut registered: HashSet<&'static str> = HashSet::new();
    for plugin in extensions.plugins {
        let name = plugin.name();
        if CORE_PLUGIN_NAMES.contains(&name) {
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
