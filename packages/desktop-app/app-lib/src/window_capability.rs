//! Core's window capability: the IPC permissions core's frontend needs in the
//! main window.
//!
//! Tauri refuses every IPC call that no capability grants. A capability file in
//! an app crate's `capabilities/` directory is read by that crate's
//! `tauri_build::build()` and embedded by its `generate_context!()`, so a file
//! kept in core's app crate would reach no other app crate built on this
//! library. Instead `run` adds this capability at startup with
//! [`Manager::add_capability`](tauri::Manager::add_capability), and every app
//! crate gets it unchanged.
//!
//! The capability grants window `main`:
//!
//! * `core:default`, Tauri's own default commands (events, app info, paths,
//!   window and webview basics);
//! * `core:window:allow-destroy` and `core:window:allow-close`, which closing
//!   the window and quitting need;
//! * `opener:default` and `dialog:default`, for opening links and the file
//!   dialogs.
//!
//! Tauri resolves it against the app's ACL, which `tauri_build` assembles from
//! the permission files of the app crate's direct dependencies only. So the
//! app crate depends directly on every plugin crate [`plugin_crates`] lists,
//! even though this library registers the plugins themselves.
//!
//! That list is part of the app-crate contract the extension API versions
//! (ADR-082 §8). Granting a permission of a plugin it does not yet name makes
//! every app crate built on an earlier core panic at startup until it adds the
//! dependency, so it is a major change to `EXTENSION_API_VERSION`.
//!
//! Its identifier, `core-window`, keeps it apart from a capability an app crate
//! grants from its own files, which Tauri's scaffold names `default`.

use tauri::{Manager, Runtime};

/// Core's window capability, in the JSON form `Manager::add_capability` takes.
pub const CAPABILITY: &str = include_str!("window-capability.json");

/// Adds core's window capability to the app. `run` calls it first in core's
/// setup, before anything can make an IPC call.
///
/// # Panics
///
/// Tauri panics, rather than returning an error, when it cannot resolve the
/// capability: when a permission names a plugin whose permission files the
/// app's ACL lacks, because the app crate does not depend directly on that
/// plugin's crate (see [`plugin_crates`]).
pub fn add<R: Runtime, M: Manager<R>>(manager: &M) -> tauri::Result<()> {
    manager.add_capability(CAPABILITY)
}

/// The Tauri plugin crates whose permissions core's window capability grants,
/// sorted and each named once. The app crate must depend directly on each of
/// them, or [`add`] panics at startup. An app crate can check its own manifest
/// against this list in a test.
///
/// A permission `<plugin>:<name>` belongs to the crate `tauri-plugin-<plugin>`;
/// `core:` permissions belong to Tauri itself and need no plugin crate.
pub fn plugin_crates() -> Vec<String> {
    plugin_crates_in(CAPABILITY)
}

/// [`plugin_crates`] of the capability `capability`.
fn plugin_crates_in(capability: &str) -> Vec<String> {
    let capability: serde_json::Value =
        serde_json::from_str(capability).expect("a capability is JSON");
    let permissions = capability["permissions"]
        .as_array()
        .expect("a capability lists its permissions");

    let mut crates: Vec<String> = permissions
        .iter()
        .filter_map(|permission| {
            // A permission is its identifier, or an object carrying it with a scope.
            permission
                .as_str()
                .or_else(|| permission["identifier"].as_str())
        })
        .filter_map(|identifier| identifier.split_once(':').map(|(plugin, _)| plugin))
        .filter(|plugin| *plugin != "core")
        .map(|plugin| format!("tauri-plugin-{plugin}"))
        .collect();
    crates.sort_unstable();
    crates.dedup();
    crates
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capability grants window `main` exactly what core's app crate
    /// granted it from its own capability file.
    #[test]
    fn the_capability_grants_the_main_window_what_the_frontend_needs() {
        let capability: serde_json::Value =
            serde_json::from_str(CAPABILITY).expect("the capability is JSON");

        assert_eq!(capability["identifier"], "core-window");
        assert_eq!(capability["windows"], serde_json::json!(["main"]));
        assert_eq!(
            capability["permissions"],
            serde_json::json!([
                "core:default",
                "core:window:allow-destroy",
                "core:window:allow-close",
                "opener:default",
                "dialog:default"
            ])
        );
        assert_eq!(
            plugin_crates(),
            ["tauri-plugin-dialog", "tauri-plugin-opener"],
            "the plugins the capability grants are part of the app-crate contract: granting \
             a new one is a major EXTENSION_API_VERSION bump"
        );
    }

    #[test]
    fn plugin_crates_names_the_crate_of_each_granted_plugin_once() {
        let capability = r#"{
            "identifier": "fixture",
            "windows": ["main"],
            "permissions": [
                "core:default",
                "core:window:allow-close",
                "opener:default",
                "opener:allow-open-url",
                { "identifier": "dialog:allow-open", "allow": [{ "path": "$HOME" }] },
                "clipboard-manager:default",
                "an-app-permission"
            ]
        }"#;

        assert_eq!(
            plugin_crates_in(capability),
            [
                "tauri-plugin-clipboard-manager",
                "tauri-plugin-dialog",
                "tauri-plugin-opener",
            ]
        );
    }
}
