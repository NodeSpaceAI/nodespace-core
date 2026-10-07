//! An app crate outside the library names its own daemon through the public
//! extension API: it builds a `DaemonProfile` from the library root and hands
//! it over with `AppExtensions::daemon_profile`.

use nodespace_app_lib::{assemble, AppExtensions, DaemonProfile};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

#[test]
fn an_app_crate_names_its_own_daemon_profile() {
    let profile = DaemonProfile {
        binary_name: "custom-daemon",
        extensions: vec!["custom".to_string()],
        service_env: vec![("CUSTOM_MODE".to_string(), "on".to_string())],
    };
    let extensions = AppExtensions::<MockRuntime>::none().daemon_profile(profile);

    assemble(mock_builder(), extensions)
        .build(mock_context(noop_assets()))
        .expect("an app whose extension names a daemon profile builds");
}

#[test]
#[should_panic(expected = "NODESPACED_SOCKET")]
fn an_app_crate_cannot_replace_the_socket_core_registers() {
    let profile = DaemonProfile {
        binary_name: "custom-daemon",
        extensions: Vec::new(),
        service_env: vec![(
            "NODESPACED_SOCKET".to_string(),
            "/tmp/elsewhere.sock".to_string(),
        )],
    };
    let _ = AppExtensions::<MockRuntime>::none().daemon_profile(profile);
}
