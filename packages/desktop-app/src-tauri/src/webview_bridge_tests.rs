//! The debug bridge stays out of every build that ships.
//!
//! The bridge runs arbitrary JavaScript in the webview over an unauthenticated
//! local socket. These tests pin the ways it is kept out of a release or
//! bundle build: the dependency and feature are opt-in, no shipped build path
//! passes the feature, and the committed Tauri config is the production one.

fn manifest() -> toml::Table {
    include_str!("../Cargo.toml")
        .parse()
        .expect("Cargo.toml is TOML")
}

#[test]
fn the_bridge_dependency_is_optional_and_pinned_exactly() {
    let manifest = manifest();
    let dependency = &manifest["dependencies"]["tauri-plugin-mcp-bridge"];

    assert_eq!(dependency["optional"].as_bool(), Some(true));
    let version = dependency["version"].as_str().expect("a version");
    assert!(
        version.starts_with('='),
        "the bridge is pinned to one exact version, got {version}"
    );
}

#[test]
fn the_bridge_feature_is_not_on_by_default_or_through_another_feature() {
    let manifest = manifest();
    let features = manifest["features"].as_table().expect("a [features] table");

    assert!(features.contains_key("debug-bridge"));
    for (name, members) in features {
        if name == "debug-bridge" {
            continue;
        }
        let members = members.as_array().expect("a feature lists its members");
        assert!(
            members
                .iter()
                .filter_map(|member| member.as_str())
                .all(|member| !member.contains("debug-bridge") && !member.contains("mcp-bridge")),
            "feature `{name}` must not pull in the debug bridge"
        );
    }
    assert!(
        !features.contains_key("default"),
        "a default feature set could enable the bridge in a shipped build"
    );
}

#[test]
fn no_shipped_build_script_passes_the_bridge_feature() {
    let package: serde_json::Value =
        serde_json::from_str(include_str!("../../package.json")).expect("package.json is JSON");
    let scripts = package["scripts"].as_object().expect("a scripts table");

    assert!(
        scripts["dev:tauri"]
            .as_str()
            .expect("a dev:tauri script")
            .contains("debug-bridge"),
        "the development run turns the bridge on"
    );
    for (name, script) in scripts {
        let script = script.as_str().expect("a script is a string");
        if name == "dev:tauri" {
            continue;
        }
        assert!(
            !script.contains("debug-bridge"),
            "script `{name}` must not enable the debug bridge"
        );
    }
}

/// Everything outside this package that builds or releases the app: the root
/// scripts, the tooling in `scripts/` (tests of it excluded, they name what they
/// forbid) and the CI workflows. None of them may turn the bridge on.
#[test]
fn no_repository_build_or_release_path_passes_the_bridge_feature() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut files = vec![root.join("package.json")];
    for (dir, extension) in [("scripts", "ts"), (".github/workflows", "yml")] {
        let entries = std::fs::read_dir(root.join(dir)).expect("the directory is readable");
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.extension().and_then(|e| e.to_str()) == Some(extension)
                && !name.contains(".test.")
            {
                files.push(path);
            }
        }
    }
    assert!(files.len() > 5, "the scan reaches the repository's files");

    for file in files {
        let text = std::fs::read_to_string(&file).expect("the file is readable");
        assert!(
            !text.contains("debug-bridge"),
            "{} must not enable the debug bridge",
            file.display()
        );
    }
}

/// The plugin's helper script needs `window.__TAURI__`. `dev:tauri` turns it on
/// for the development run with a `--config` override; the committed config,
/// which release and bundle builds read, keeps it off and keeps the strict CSP.
#[test]
fn the_committed_tauri_config_is_the_production_one() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json is JSON");

    assert_eq!(config["app"]["withGlobalTauri"], serde_json::json!(false));
    assert_eq!(
        config["app"]["security"]["csp"],
        serde_json::json!(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' asset: https://asset.localhost data:; \
             connect-src 'self' ipc: http://ipc.localhost; font-src 'self' data:; \
             object-src 'none'"
        )
    );
}

#[cfg(not(feature = "debug-bridge"))]
#[test]
fn a_build_without_the_feature_does_not_register_the_bridge() {
    let capability: serde_json::Value =
        serde_json::from_str(nodespace_app_lib::window_capability::CAPABILITY)
            .expect("the capability is JSON");
    assert!(
        !capability.to_string().contains("mcp-bridge"),
        "core's window capability never grants the bridge"
    );
}

/// With the feature on, the bridge's capability must resolve against this
/// crate's ACL, which is built from this crate's direct dependencies.
#[cfg(feature = "debug-bridge")]
#[test]
fn the_bridge_capability_resolves_against_this_crates_acl() {
    use tauri::test::mock_builder;

    let app = mock_builder()
        .build(crate::context())
        .expect("the app builds from this crate's context");

    nodespace_app_lib::webview_bridge::grant(&app).expect("the bridge capability is added");
}
