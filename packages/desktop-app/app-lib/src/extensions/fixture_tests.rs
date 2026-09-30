//! Fixture extension driving the extension API through `assemble` on
//! `tauri::test::MockRuntime`.
//!
//! The fixture is a Tauri plugin with a command and managed state, the shape
//! any extension takes. Later additions to the extension API extend this
//! fixture, and the check that keeps the Rust and TypeScript API versions in
//! step watches it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::plugin::{Builder as PluginBuilder, Plugin, TauriPlugin};
use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::utils::acl::ExecutionContext;
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, Runtime};

use super::{assemble, AppExtensions, CORE_PLUGIN_NAMES, EXTENSION_API_VERSION};

/// Name of the fixture plugin, which is also its command namespace.
const FIXTURE: &str = "fixture";

/// Managed by the fixture plugin's setup.
struct FixtureState {
    reply: &'static str,
}

#[tauri::command]
fn fixture_ping(state: tauri::State<'_, FixtureState>) -> String {
    state.reply.to_string()
}

/// The fixture extension: one command, and state managed during setup.
fn fixture_plugin<R: Runtime>() -> TauriPlugin<R> {
    PluginBuilder::<R>::new(FIXTURE)
        .invoke_handler(tauri::generate_handler![fixture_ping])
        .setup(|app, _api| {
            app.manage(FixtureState { reply: "pong" });
            Ok(())
        })
        .build()
}

/// A plugin that only records that its setup ran.
fn probe_plugin<R: Runtime>(name: &'static str, ran: &Arc<AtomicBool>) -> TauriPlugin<R> {
    let ran = Arc::clone(ran);
    PluginBuilder::<R>::new(name)
        .setup(move |_app, _api| {
            ran.store(true, Ordering::SeqCst);
            Ok(())
        })
        .build()
}

/// Builds an app from `ext` with no capability granted.
fn build_app(ext: AppExtensions<MockRuntime>) -> App<MockRuntime> {
    assemble(mock_builder(), ext)
        .build(mock_context(noop_assets()))
        .expect("the app builds")
}

/// Builds an app from `ext`, granting `command` to every window.
///
/// The mock context has an empty ACL, and a plugin command is refused unless
/// the ACL allows it. Tauri's only way to allow one on a mock context is this
/// doc-hidden call on the context's runtime authority.
fn build_app_granting(ext: AppExtensions<MockRuntime>, command: &str) -> App<MockRuntime> {
    let mut context = mock_context(noop_assets());
    context
        .runtime_authority_mut()
        .__allow_command(command.to_string(), ExecutionContext::Local);
    assemble(mock_builder(), ext)
        .build(context)
        .expect("the app builds")
}

/// Invokes `command` from a `main` webview the way the frontend would.
fn invoke(app: &App<MockRuntime>, command: &str) -> Result<String, serde_json::Value> {
    let webview = tauri::WebviewWindowBuilder::new(app, "main", Default::default())
        .build()
        .expect("the main window builds");
    let url = if cfg!(any(windows, target_os = "android")) {
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
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    };
    get_ipc_response(&webview, request)
        .map(|body| body.deserialize::<String>().expect("a string reply"))
}

#[test]
fn extension_api_version_is_one_zero() {
    assert_eq!(EXTENSION_API_VERSION, (1, 0));
}

#[test]
fn extension_plugin_setup_runs_and_its_state_is_managed() {
    let app = build_app(AppExtensions::none().plugin(fixture_plugin()));

    let state = app
        .try_state::<FixtureState>()
        .expect("the plugin's setup managed its state");
    assert_eq!(state.reply, "pong");
}

#[test]
fn extension_plugin_command_answers_through_ipc_once_granted() {
    let app = build_app_granting(
        AppExtensions::none().plugin(fixture_plugin()),
        "plugin:fixture|fixture_ping",
    );

    assert_eq!(
        invoke(&app, "plugin:fixture|fixture_ping").expect("the granted command answers"),
        "pong"
    );
}

#[test]
fn extension_plugin_command_is_denied_without_a_grant() {
    let app = build_app(AppExtensions::none().plugin(fixture_plugin()));
    assert!(
        app.try_state::<FixtureState>().is_some(),
        "the plugin is registered, so a refusal below is the capability check's"
    );

    let refusal = invoke(&app, "plugin:fixture|fixture_ping")
        .expect_err("an ungranted plugin command is refused");
    let refusal = refusal.to_string();
    assert!(
        refusal.contains("fixture_ping") && refusal.contains("not allowed"),
        "refused for the missing grant on the command, got: {refusal}"
    );
}

#[test]
fn extension_plugin_named_like_a_core_plugin_is_not_registered() {
    for name in CORE_PLUGIN_NAMES {
        let core_ran = Arc::new(AtomicBool::new(false));
        let extension_ran = Arc::new(AtomicBool::new(false));
        // Stands in for the plugin core registers under this name before it
        // calls `assemble`.
        let builder = mock_builder().plugin(probe_plugin(name, &core_ran));
        assemble(
            builder,
            AppExtensions::none().plugin(probe_plugin(name, &extension_ran)),
        )
        .build(mock_context(noop_assets()))
        .expect("the app builds");

        assert!(
            !extension_ran.load(Ordering::SeqCst),
            "the setup of an extension plugin named {name} must not run"
        );
        assert!(
            core_ran.load(Ordering::SeqCst),
            "the extension plugin named {name} must not replace core's plugin"
        );
    }
}

#[test]
fn second_extension_plugin_with_the_same_name_is_not_registered() {
    let second_ran = Arc::new(AtomicBool::new(false));
    let app = build_app(
        AppExtensions::none()
            .plugin(fixture_plugin())
            .plugin(probe_plugin(FIXTURE, &second_ran)),
    );

    assert!(
        app.try_state::<FixtureState>().is_some(),
        "the first registration stays"
    );
    assert!(
        !second_ran.load(Ordering::SeqCst),
        "the second plugin's setup must not run"
    );
}

#[test]
fn extension_plugins_are_registered_in_the_order_they_were_added() {
    fn recording_plugin(
        name: &'static str,
        order: &Arc<Mutex<Vec<&'static str>>>,
    ) -> TauriPlugin<MockRuntime> {
        let order = Arc::clone(order);
        PluginBuilder::<MockRuntime>::new(name)
            .setup(move |_app, _api| {
                order.lock().unwrap().push(name);
                Ok(())
            })
            .build()
    }

    let order = Arc::new(Mutex::new(Vec::new()));
    build_app(
        AppExtensions::none()
            .plugin(recording_plugin("first", &order))
            .plugin(recording_plugin("second", &order))
            .plugin(recording_plugin("third", &order)),
    );

    assert_eq!(*order.lock().unwrap(), ["first", "second", "third"]);
}

#[test]
#[cfg(desktop)]
fn core_plugin_names_match_the_plugins_core_registers() {
    let mut registered = vec![
        tauri_plugin_single_instance::init::<MockRuntime, _>(|_, _, _| {}).name(),
        tauri_plugin_opener::init::<MockRuntime>().name(),
        tauri_plugin_dialog::init::<MockRuntime>().name(),
    ];
    registered.sort_unstable();

    let mut listed = CORE_PLUGIN_NAMES.to_vec();
    listed.sort_unstable();

    assert_eq!(listed, registered);
}

#[test]
fn none_registers_nothing() {
    for ext in [AppExtensions::none(), AppExtensions::default()] {
        let app = build_app(ext);

        assert!(
            app.try_state::<FixtureState>().is_none(),
            "no plugin ran its setup"
        );
        // Tauri cannot list the plugins an app registered, so only the names
        // this suite could have registered are probed.
        for name in CORE_PLUGIN_NAMES.iter().chain(&[FIXTURE]) {
            assert!(
                !app.handle().remove_plugin(name),
                "no plugin named {name} is registered"
            );
        }
    }
}
