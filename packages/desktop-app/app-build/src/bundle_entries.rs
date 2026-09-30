//! Leaving the bundle entries that are not staged out of a debug build.

use std::env;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Leave the sidecars and resources that are not staged out of a debug build's
/// bundle config.
///
/// `tauri_build::build()` copies every declared sidecar and resource into the
/// build output and fails on a missing one. Staged files are usually build
/// output, so any build of the app crate in a fresh checkout — clippy,
/// `cargo check`, `cargo test`, a merge gate — would need them staged first
/// (or copied in from another checkout, which risks a stale binary), although
/// none of those builds runs the bundled app. A debug build therefore declares
/// only what is staged, and says so in a `cargo:warning`.
///
/// Release builds stay strict: a packaged app must never ship without its
/// sidecars or resources.
///
/// - A sidecar (`bundle.externalBin`) is staged when
///   `<entry>-<target triple><exe suffix>` is a file.
/// - A resource (`bundle.resources`, list form) is staged when its base
///   directory exists and is not empty. The base directory is the run of whole
///   path components before the first `*`, `?` or `[`, or the entry itself when
///   it has none. An entry that is not a string is left as declared, and a
///   `resources` map leaves the whole config alone.
///
/// The config is read from `tauri.conf.json` in the build script's working
/// directory, the app crate's root. The result goes through `TAURI_CONFIG`,
/// which `tauri_build` merges over that file as a JSON merge patch. Arrays are
/// replaced whole, so the patch restates each list read from the file, minus
/// the unstaged entries. An explicitly set `TAURI_CONFIG` (the tauri CLI's
/// `--config`) is left alone: whoever set it owns the bundle config.
///
/// Sets `TAURI_CONFIG` for the rest of the process, so call it from the build
/// script's `main`, before `tauri_build::build()`.
///
/// # Panics
///
/// If `tauri.conf.json` cannot be read or is not valid JSON.
pub fn drop_unstaged_bundle_entries() {
    if !applies(
        env::var("PROFILE").ok().as_deref(),
        env::var_os("TAURI_CONFIG").is_some(),
    ) {
        return;
    }
    let target_triple = env::var("TARGET").expect("cargo always sets TARGET for build scripts");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("cargo always sets CARGO_CFG_TARGET_OS");
    let exe_suffix = if target_os == "windows" { ".exe" } else { "" };

    let conf: Value = serde_json::from_str(
        &std::fs::read_to_string("tauri.conf.json").expect("tauri.conf.json is readable"),
    )
    .expect("tauri.conf.json is valid JSON");
    let Some(Plan { patch, missing }) =
        plan(&conf, &target_triple, exe_suffix, Path::is_file, |dir| {
            dir.read_dir()
                .is_ok_and(|mut entries| entries.next().is_some())
        })
    else {
        return;
    };

    // `set_var` is fine on edition 2021 — build scripts are single-threaded by
    // Cargo's contract.
    env::set_var("TAURI_CONFIG", patch.to_string());
    let names: Vec<String> = missing.iter().map(|p| p.display().to_string()).collect();
    println!(
        "cargo:warning=not staged, left out of this debug build's bundle: {} \
         (only needed to run the app; stage them to include them)",
        names.join(", ")
    );

    // tauri_build watches only what it copies, so watch the dropped paths
    // here to pick up a later staging step.
    for path in &missing {
        if let Some(watched) = nearest_existing_ancestor(path, Path::exists) {
            println!("cargo:rerun-if-changed={}", watched.display());
        }
    }
}

/// Whether the drop applies: a debug build whose bundle config nobody set
/// explicitly.
fn applies(profile: Option<&str>, tauri_config_is_set: bool) -> bool {
    profile == Some("debug") && !tauri_config_is_set
}

/// What a debug build leaves out of its bundle config.
#[derive(Debug, PartialEq)]
struct Plan {
    /// The `TAURI_CONFIG` merge patch: `bundle.externalBin` and
    /// `bundle.resources`, each restated without its unstaged entries.
    patch: Value,
    /// What the dropped entries stand for, in declaration order: the sidecars'
    /// staged paths, then the resources' base directories.
    missing: Vec<PathBuf>,
}

/// Works out what to leave out of `conf`'s bundle, without touching the
/// environment or the file system: `file_exists` and `dir_has_entries` answer
/// for the sidecar files and the resource directories. `None` when nothing is
/// missing, and also when the config isn't a shape this understands, in which
/// case the build stays strict rather than guess.
fn plan(
    conf: &Value,
    target_triple: &str,
    exe_suffix: &str,
    file_exists: impl Fn(&Path) -> bool,
    dir_has_entries: impl Fn(&Path) -> bool,
) -> Option<Plan> {
    // Not a plain list (a resources map, say): stay strict.
    let (Some(declared_bins), Some(declared_resources)) = (
        conf["bundle"]["externalBin"].as_array(),
        conf["bundle"]["resources"].as_array(),
    ) else {
        return None;
    };

    let staged_sidecar = |bin: &str| PathBuf::from(format!("{bin}-{target_triple}{exe_suffix}"));
    // A glob-free entry can be a single file, so a file counts too.
    let has_content = |path: &Path| file_exists(path) || dir_has_entries(path);

    let mut missing = Vec::new();
    // Non-string entries are kept as declared rather than guessed at.
    let mut external_bins = Vec::new();
    for entry in declared_bins {
        match entry.as_str().map(staged_sidecar) {
            Some(staged) if !file_exists(&staged) => missing.push(staged),
            _ => external_bins.push(entry.clone()),
        }
    }
    let mut resources = Vec::new();
    for entry in declared_resources {
        match entry.as_str().map(base_dir) {
            Some(base) if !has_content(&base) => missing.push(base),
            _ => resources.push(entry.clone()),
        }
    }
    if missing.is_empty() {
        return None;
    }

    Some(Plan {
        patch: json!({
            "bundle": { "resources": resources, "externalBin": external_bins }
        }),
        missing,
    })
}

/// The directory a `resources` entry draws from: the run of whole path
/// components before its first glob character, or the entry itself when it has
/// none. `resources/skill/**/*` is judged by `resources/skill`; `binaries/tool-*`
/// by `binaries`, since `tool-` is only the start of a file name.
fn base_dir(entry: &str) -> PathBuf {
    let dir = match entry.find(['*', '?', '[']) {
        None => Path::new(entry),
        Some(glob_at) => {
            let literal = &entry[..glob_at];
            if literal.ends_with(std::path::is_separator) {
                Path::new(literal)
            } else {
                Path::new(literal).parent().unwrap_or(Path::new(""))
            }
        }
    };
    // `components` also drops a trailing separator, so the path reads the same
    // in the warning however the entry was written.
    let dir: PathBuf = dir.components().collect();
    if dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        dir
    }
}

/// The nearest ancestor of `path` (itself included) that exists. A path that
/// doesn't exist yet can't be watched directly: cargo treats a missing path as
/// always changed, which would rerun the build script on every build.
fn nearest_existing_ancestor(path: &Path, exists: impl Fn(&Path) -> bool) -> Option<&Path> {
    path.ancestors()
        .find(|ancestor| !ancestor.as_os_str().is_empty() && exists(ancestor))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIPLE: &str = "aarch64-apple-darwin";

    /// `plan` over a synthetic tree: `files` are the staged files, `populated`
    /// the directories with something in them.
    fn plan_over(conf: &Value, files: &[&str], populated: &[&str]) -> Option<Plan> {
        plan(
            conf,
            TRIPLE,
            "",
            |path| files.iter().any(|f| path == Path::new(f)),
            |path| populated.iter().any(|d| path == Path::new(d)),
        )
    }

    fn conf(external_bin: Value, resources: Value) -> Value {
        json!({ "bundle": { "externalBin": external_bin, "resources": resources } })
    }

    fn paths(items: &[&str]) -> Vec<PathBuf> {
        items.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn keeps_everything_when_everything_is_staged() {
        let conf = conf(
            json!(["binaries/daemon", "binaries/cli"]),
            json!(["resources/models/**/*", "resources/skill/**/*"]),
        );
        let files = [
            "binaries/daemon-aarch64-apple-darwin",
            "binaries/cli-aarch64-apple-darwin",
        ];
        let populated = ["resources/models", "resources/skill"];

        assert_eq!(plan_over(&conf, &files, &populated), None);
    }

    #[test]
    fn drops_an_unstaged_sidecar() {
        let conf = conf(
            json!(["binaries/daemon", "binaries/cli"]),
            json!(["resources/models/**/*"]),
        );
        let files = ["binaries/daemon-aarch64-apple-darwin"];

        let plan = plan_over(&conf, &files, &["resources/models"]).expect("a sidecar is missing");

        assert_eq!(
            plan.patch,
            json!({ "bundle": {
                "externalBin": ["binaries/daemon"],
                "resources": ["resources/models/**/*"],
            } })
        );
        assert_eq!(plan.missing, paths(&["binaries/cli-aarch64-apple-darwin"]));
    }

    #[test]
    fn looks_for_the_executable_suffix_of_the_target_platform() {
        let conf = conf(json!(["binaries/daemon"]), json!([]));

        let plan = plan(
            &conf,
            "x86_64-pc-windows-msvc",
            ".exe",
            |path| path == Path::new("binaries/daemon-x86_64-pc-windows-msvc"),
            |_| false,
        )
        .expect("the .exe sidecar is missing");

        assert_eq!(
            plan.missing,
            paths(&["binaries/daemon-x86_64-pc-windows-msvc.exe"])
        );
    }

    #[test]
    fn drops_a_resource_whose_base_directory_is_empty_or_missing() {
        // The directory predicate is false for both an empty directory and one
        // that doesn't exist, so one fixture stands for both.
        let conf = conf(
            json!([]),
            json!([
                "resources/skill/**/*",
                "resources/models/**/*",
                "assets/*.png"
            ]),
        );

        let plan = plan_over(&conf, &[], &["resources/models"]).expect("two resources are missing");

        assert_eq!(
            plan.patch,
            json!({ "bundle": { "externalBin": [], "resources": ["resources/models/**/*"] } })
        );
        assert_eq!(plan.missing, paths(&["resources/skill", "assets"]));
    }

    #[test]
    fn keeps_a_resource_whose_base_directory_has_a_file() {
        // The `.gitkeep` case: a tracked placeholder is all a fresh checkout
        // has in the models directory, and it keeps the glob from erroring.
        let conf = conf(json!(["binaries/daemon"]), json!(["resources/models/**/*"]));

        let plan = plan_over(&conf, &[], &["resources/models"]).expect("the sidecar is missing");

        assert_eq!(
            plan.patch,
            json!({ "bundle": { "externalBin": [], "resources": ["resources/models/**/*"] } })
        );
        assert_eq!(
            plan.missing,
            paths(&["binaries/daemon-aarch64-apple-darwin"])
        );
    }

    #[test]
    fn a_partial_filename_glob_is_judged_by_its_directory() {
        let conf = conf(json!([]), json!(["binaries/tool-*"]));

        // `binaries/tool-` is not a directory; `binaries` is, and it has files.
        assert_eq!(plan_over(&conf, &[], &["binaries"]), None);

        let plan = plan_over(&conf, &[], &[]).expect("the directory is empty");
        assert_eq!(plan.missing, paths(&["binaries"]));
    }

    #[test]
    fn a_glob_free_resource_is_kept_when_it_is_a_staged_file() {
        let conf = conf(json!([]), json!(["docs/LICENSE"]));

        assert_eq!(plan_over(&conf, &["docs/LICENSE"], &[]), None);

        let plan = plan_over(&conf, &[], &[]).expect("the file is missing");
        assert_eq!(plan.missing, paths(&["docs/LICENSE"]));
    }

    #[test]
    fn leaves_non_string_entries_as_declared() {
        let bin_object = json!({ "path": "binaries/odd" });
        let resource_object = json!({ "src": "resources/odd/**/*" });
        let conf = conf(
            json!([bin_object, "binaries/daemon", 7]),
            json!([resource_object, "resources/skill/**/*"]),
        );

        let plan = plan_over(&conf, &[], &[]).expect("string entries are missing");

        assert_eq!(
            plan.patch,
            json!({ "bundle": {
                "externalBin": [bin_object, 7],
                "resources": [resource_object],
            } })
        );
        assert_eq!(
            plan.missing,
            paths(&["binaries/daemon-aarch64-apple-darwin", "resources/skill"])
        );
    }

    #[test]
    fn a_resources_map_leaves_the_config_alone() {
        let conf = conf(
            json!(["binaries/daemon"]),
            json!({ "resources/skill": "skill" }),
        );

        assert_eq!(plan_over(&conf, &[], &[]), None);
    }

    #[test]
    fn a_config_without_a_sidecar_list_is_left_alone() {
        let conf = json!({ "bundle": { "resources": ["resources/skill/**/*"] } });

        assert_eq!(plan_over(&conf, &[], &[]), None);
    }

    #[test]
    fn core_config_with_nothing_staged_matches_todays_patch() {
        let core: Value = serde_json::from_str(include_str!("../../src-tauri/tauri.conf.json"))
            .expect("tauri.conf.json is valid JSON");

        // Only the models directory is there in a fresh checkout: it holds a
        // tracked placeholder. Neither a sidecar nor the skill is staged.
        let plan = plan_over(&core, &[], &["resources/models"]).expect("nothing is staged");

        assert_eq!(
            plan.patch,
            json!({ "bundle": { "resources": ["resources/models/**/*"], "externalBin": [] } })
        );
        // The list the warning prints, as text: a `Path` compares equal with or
        // without a trailing separator, the warning doesn't.
        let printed: Vec<String> = plan
            .missing
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        assert_eq!(
            printed,
            [
                "binaries/nodespaced-aarch64-apple-darwin",
                "binaries/nodespace-aarch64-apple-darwin",
                "binaries/nodespace-skill-installer-aarch64-apple-darwin",
                "resources/skill",
            ]
        );
    }

    #[test]
    fn base_dir_is_the_whole_components_before_the_first_glob_character() {
        for (entry, expected) in [
            ("resources/skill/**/*", "resources/skill"),
            ("resources/models/**/*", "resources/models"),
            ("binaries/tool-*", "binaries"),
            ("assets/a?/b", "assets"),
            ("assets/[ab]/c", "assets"),
            ("*.png", "."),
            ("assets/icon.png", "assets/icon.png"),
            ("assets/", "assets"),
        ] {
            // As text: a `Path` compares equal with or without a trailing separator.
            assert_eq!(base_dir(entry).to_str(), Some(expected), "entry {entry:?}");
        }
    }

    #[test]
    fn only_a_debug_build_without_an_explicit_config_drops_entries() {
        assert!(applies(Some("debug"), false));
        assert!(!applies(Some("release"), false));
        assert!(!applies(Some("debug"), true));
        assert!(!applies(None, false));
    }

    #[test]
    fn watches_the_nearest_existing_ancestor_of_a_dropped_path() {
        let path = Path::new("resources/skill");

        let exists = |p: &Path| p == Path::new("resources");
        assert_eq!(
            nearest_existing_ancestor(path, exists),
            Some(Path::new("resources"))
        );

        let exists = |p: &Path| p == Path::new("resources") || p == Path::new("resources/skill");
        assert_eq!(nearest_existing_ancestor(path, exists), Some(path));

        assert_eq!(nearest_existing_ancestor(path, |_| false), None);

        // The empty path, where a relative path's ancestors end, is not a
        // path to watch even if something claims it exists.
        let exists = |p: &Path| p.as_os_str().is_empty();
        assert_eq!(nearest_existing_ancestor(path, exists), None);
    }
}
