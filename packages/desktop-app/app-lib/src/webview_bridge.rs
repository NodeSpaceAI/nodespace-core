//! A development-only bridge that lets an agent inspect and drive the running
//! app: evaluate JavaScript in the webview, read the DOM and console, capture
//! screenshots, observe and invoke Tauri commands.
//!
//! WKWebView exposes no remote debugging protocol, so the bridge is a plugin
//! inside the app that serves a WebSocket to a local client. The plugin runs
//! arbitrary JavaScript in the main window and its socket carries no
//! authentication, so everything here is built to make a shipped build unable
//! to have it:
//!
//! * the module, the plugin dependency and every call site exist only under the
//!   `debug-bridge` cargo feature, which no release or bundle script passes;
//! * a release-profile build with the feature is a compile error
//!   (`compile_error!` in `lib.rs`), so the feature cannot be turned on for a
//!   build that ships;
//! * the socket binds to loopback only, never the plugin's default of every
//!   interface;
//! * the permission is a capability of its own, added only here, so the
//!   production window capability ([`crate::window_capability`]) does not
//!   change.
//!
//! The plugin's own helper script calls `window.__TAURI__`, which exists only
//! when `app.withGlobalTauri` is on. The committed `tauri.conf.json` keeps it
//! off; `bun run dev:tauri` turns it on for the development run with a
//! `--config` override.

use tauri::plugin::TauriPlugin;
use tauri::{Manager, Runtime};
use tauri_plugin_mcp_bridge::Config;

/// The bridge's capability, in the JSON form `Manager::add_capability` takes.
/// Its identifier keeps it apart from core's `core-window` capability.
pub const CAPABILITY: &str = r#"{
  "identifier": "debug-bridge",
  "description": "Debug bridge permissions for the main window (development builds only)",
  "windows": ["main"],
  "permissions": ["mcp-bridge:default"]
}"#;

/// The bridge plugin, bound to loopback.
///
/// Building it has no effects: the plugin starts its socket in its setup hook,
/// which runs once when the app is built (see `extensions::core_plugins`).
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_mcp_bridge::init_with_config(Config::localhost_only())
}

/// Grants the bridge's permissions to the main window. `run` calls it in setup
/// after core's window capability.
///
/// # Panics
///
/// Tauri panics when the app's ACL lacks the bridge's permission files, which
/// happens when the app crate does not depend directly on
/// `tauri-plugin-mcp-bridge` under the same feature.
pub fn grant<R: Runtime, M: Manager<R>>(manager: &M) -> tauri::Result<()> {
    manager.add_capability(CAPABILITY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capability_grants_only_the_bridge_to_the_main_window() {
        let capability: serde_json::Value =
            serde_json::from_str(CAPABILITY).expect("the capability is JSON");

        assert_eq!(capability["identifier"], "debug-bridge");
        assert_eq!(capability["windows"], serde_json::json!(["main"]));
        assert_eq!(
            capability["permissions"],
            serde_json::json!(["mcp-bridge:default"])
        );
    }

    #[test]
    fn the_plugin_listens_on_loopback_only() {
        let config = Config::localhost_only();
        assert_eq!(config.bind_address, "127.0.0.1");
    }

    #[test]
    fn the_production_window_capability_does_not_name_the_bridge() {
        assert!(
            !crate::window_capability::CAPABILITY.contains("mcp-bridge"),
            "the bridge's permission belongs to its own capability, never core's"
        );
    }
}
