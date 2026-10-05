//! Core's window capability against this app crate.
//!
//! nodespace-app-lib adds the capability at startup, and Tauri resolves it
//! against the ACL this crate's `tauri_build::build()` assembles from the
//! permission files of its direct dependencies. A capability that names a
//! plugin this crate does not depend on directly therefore builds cleanly and
//! panics only when the app starts. These tests catch it first, against the
//! context `main` runs the app with, built on `tauri::test::MockRuntime`.

use std::panic::{catch_unwind, AssertUnwindSafe};

use nodespace_app_lib::window_capability;
use tauri::test::{get_ipc_response, mock_builder, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{App, WebviewWindow, WebviewWindowBuilder};

/// A command `core:default` grants that has no effect and needs no arguments.
const APP_VERSION_COMMAND: &str = "plugin:app|version";

/// This crate's app on the mock runtime, with this crate's config and ACL and
/// no capability added.
fn app() -> App<MockRuntime> {
    mock_builder()
        .build(crate::context())
        .expect("the app builds from this crate's context")
}

fn window(app: &App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
    WebviewWindowBuilder::new(app, label, Default::default())
        .build()
        .expect("the window builds")
}

/// Invokes `command` from `window` the way the frontend would.
fn invoke(window: &WebviewWindow<MockRuntime>, command: &str) -> Result<String, String> {
    let url = if cfg!(windows) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    let request = InvokeRequest {
        cmd: command.into(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: url.parse().expect("a valid URL"),
        body: tauri::ipc::InvokeBody::default(),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    };
    get_ipc_response(window, request)
        .map(|body| body.deserialize::<String>().expect("a string reply"))
        .map_err(|refusal| refusal.to_string())
}

#[test]
fn the_app_crate_depends_directly_on_every_plugin_the_window_capability_grants() {
    let manifest: toml::Table = include_str!("../Cargo.toml")
        .parse()
        .expect("Cargo.toml is TOML");
    let dependencies = manifest["dependencies"]
        .as_table()
        .expect("Cargo.toml has a [dependencies] table");
    let plugin_crates = window_capability::plugin_crates();
    assert!(
        !plugin_crates.is_empty(),
        "the capability grants plugin permissions, so there are crates to check"
    );

    let missing: Vec<&String> = plugin_crates
        .iter()
        .filter(|name| !dependencies.contains_key(name.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "core's window capability grants permissions of {missing:?}, which this crate's \
         [dependencies] must name directly, or the app panics at startup"
    );
}

#[test]
fn the_window_capability_resolves_against_this_crates_acl() {
    let app = app();

    let added = catch_unwind(AssertUnwindSafe(|| window_capability::add(&app)));

    match added {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("adding core's window capability failed: {error}"),
        Err(_) => panic!(
            "Tauri could not resolve core's window capability against this crate's ACL: a \
             permission names a plugin whose crate this crate does not depend on directly"
        ),
    }
}

/// The capability is what lets the frontend in: before it is added the main
/// window's calls are refused, after it they are answered, and a window with
/// another label stays refused.
#[test]
fn the_window_capability_grants_the_main_window_and_no_other() {
    let app = app();
    let main = window(&app, "main");
    let other = window(&app, "other");

    let refusal = invoke(&main, APP_VERSION_COMMAND)
        .expect_err("with no capability, the main window's call is refused");
    assert!(
        refusal.contains("not allowed"),
        "refused by the ACL, got: {refusal}"
    );

    window_capability::add(&app).expect("the capability is added");

    assert_eq!(
        invoke(&main, APP_VERSION_COMMAND),
        Ok(app.package_info().version.to_string())
    );
    let refusal =
        invoke(&other, APP_VERSION_COMMAND).expect_err("the capability names only the main window");
    assert!(
        refusal.contains("not allowed"),
        "refused by the ACL, got: {refusal}"
    );
}

/// `build.removeUnusedCommands` strips every plugin command that no capability
/// file grants. Core's window capability is added at runtime, not from a file,
/// so turning it on would strip the opener and dialog commands the capability
/// grants. Tauri also reads the setting under its kebab-case alias.
#[test]
fn the_tauri_config_leaves_unused_plugin_commands_in() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json is JSON");

    for key in ["removeUnusedCommands", "remove-unused-commands"] {
        assert_ne!(
            config["build"][key],
            serde_json::Value::Bool(true),
            "build.{key} strips the plugin commands core's window capability grants"
        );
    }
}
