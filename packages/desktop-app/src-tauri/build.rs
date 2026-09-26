mod build_support;

use std::env;
use std::path::PathBuf;

/// `externalBin` entries from `tauri.conf.json`, without the `binaries/`
/// prefix or platform triple — kept in sync with that file by hand since
/// build.rs has no cheap way to parse it (serde_json isn't a
/// `[build-dependencies]` of this crate, and pulling it in only for this
/// would be disproportionate). If a new sidecar is added there, add its bin
/// name here too, or it simply won't get the staleness guard below — every
/// other `externalBin` behaviour (including tauri-build's own copy step)
/// keeps working either way.
const EXTERNAL_BIN_NAMES: &[&str] = &["nodespaced", "nodespace"];

/// Reconciles each `externalBin` sidecar's staging copy
/// (`src-tauri/binaries/<bin>-<triple>`) with the workspace's own build
/// output (`target/<profile>/<bin>`) so that whichever is newer wins,
/// *before* `tauri_build::build()` runs its own unconditional copy in the
/// opposite direction. See `build_support`'s module doc for the full story
/// on why this exists.
fn sync_external_bin_staging() {
    let target_triple = env::var("TARGET").expect("cargo always sets TARGET for build scripts");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("cargo always sets CARGO_CFG_TARGET_OS");
    let exe_suffix = if target_os == "windows" { ".exe" } else { "" };

    // OUT_DIR is `target/<profile>/build/<pkg>-<hash>/out`; walking up three
    // parents reaches `target/<profile>`, the directory `cargo build --bin
    // <name>` places its output in. This mirrors tauri-build's own (its
    // words) "far from ideal, but there's no other way to get the target
    // dir" derivation in `copy_binaries`, so that our notion of "the fresh
    // build output" points at the exact same file tauri-build is about to
    // overwrite.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("cargo always sets OUT_DIR"));
    let target_dir = out_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("OUT_DIR is always at least three directories below target/<profile>")
        .to_path_buf();

    for bin in EXTERNAL_BIN_NAMES {
        let target_bin = target_dir.join(format!("{bin}{exe_suffix}"));
        let sidecar_bin =
            PathBuf::from("binaries").join(format!("{bin}-{target_triple}{exe_suffix}"));

        match build_support::sync_stale_sidecar(&target_bin, &sidecar_bin) {
            Ok(true) => println!(
                "cargo:warning=refreshed stale sidecar staging file {} from {} \
                 (see build_support.rs for why)",
                sidecar_bin.display(),
                target_bin.display()
            ),
            Ok(false) => {}
            Err(err) => {
                // Best-effort: if reconciliation itself fails (permissions,
                // an unreadable target_bin, ...), fall back to tauri-build's
                // pre-existing behaviour rather than hard-failing the whole
                // build over a freshness nicety.
                println!(
                    "cargo:warning=could not reconcile sidecar staging for {bin}: {err} \
                     (continuing — tauri-build's own externalBin copy is unaffected)"
                );
            }
        }
    }
}

/// The `resources` entry `bun run build:skill` stages, as `tauri.conf.json`
/// declares it.
const SKILL_RESOURCES: &str = "resources/skill/**/*";

/// Leave unstaged sidecars and the unstaged skill out of a debug build's
/// bundle config.
///
/// `tauri_build::build()` copies every declared resource and sidecar into the
/// build output and fails on a missing one. All of them are gitignored build
/// output, so any build of this crate in a fresh worktree — clippy,
/// `cargo check`, `cargo test`, the pre-push gate — used to need them staged
/// first (or copied in from another checkout, which risks a stale daemon),
/// although none of those builds runs the bundled app. A debug build
/// therefore declares only what is staged, and says so.
///
/// Release builds stay strict: a packaged app must never ship without its
/// sidecars or skill. `dev:tauri` and `tauri:build` stage everything before
/// building, so both find it staged and are unaffected. The Tauri-seam tests
/// don't depend on the staged daemon either: the gate points them at
/// `target/debug/nodespaced` via `NODESPACED_TEST_BIN`, and without it they
/// fail with a message naming the command that stages it.
///
/// Works through `TAURI_CONFIG`, which `tauri_build` merges over
/// `tauri.conf.json` as a JSON merge patch. Arrays are replaced whole, so the
/// patch restates each list read from `tauri.conf.json`, minus the unstaged
/// entries. An explicitly set `TAURI_CONFIG` (the tauri CLI's `--config`) is
/// left alone: whoever set it owns the bundle config.
fn drop_unstaged_bundle_entries() {
    if env::var("PROFILE").as_deref() != Ok("debug") || env::var_os("TAURI_CONFIG").is_some() {
        return;
    }
    let target_triple = env::var("TARGET").expect("cargo always sets TARGET for build scripts");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("cargo always sets CARGO_CFG_TARGET_OS");
    let exe_suffix = if target_os == "windows" { ".exe" } else { "" };

    let conf: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("tauri.conf.json").expect("tauri.conf.json is readable"),
    )
    .expect("tauri.conf.json is valid JSON");
    // `None` when the entry isn't a plain list (a resources map), in which
    // case the build stays strict rather than guess at the shape.
    let (Some(declared_bins), Some(declared_resources)) = (
        conf["bundle"]["externalBin"].as_array(),
        conf["bundle"]["resources"].as_array(),
    ) else {
        return;
    };

    let staged_path = |bin: &str| PathBuf::from(format!("{bin}-{target_triple}{exe_suffix}"));
    let skill_dir = PathBuf::from("resources/skill");
    let skill_staged = skill_dir
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some());

    let mut missing: Vec<PathBuf> = Vec::new();
    let external_bins: Vec<serde_json::Value> = declared_bins
        .iter()
        .filter(|entry| {
            let Some(bin) = entry.as_str() else {
                return true;
            };
            let path = staged_path(bin);
            let staged = path.is_file();
            if !staged {
                missing.push(path);
            }
            staged
        })
        .cloned()
        .collect();
    let resources: Vec<serde_json::Value> = declared_resources
        .iter()
        .filter(|entry| skill_staged || entry.as_str() != Some(SKILL_RESOURCES))
        .cloned()
        .collect();
    if !skill_staged {
        missing.push(skill_dir);
    }
    if missing.is_empty() {
        return;
    }

    let patch = serde_json::json!({
        "bundle": { "resources": resources, "externalBin": external_bins }
    });
    // See `main` on `set_var` in a build script.
    env::set_var("TAURI_CONFIG", patch.to_string());
    let names: Vec<String> = missing.iter().map(|p| p.display().to_string()).collect();
    println!(
        "cargo:warning=not staged, left out of this debug build's bundle: {} \
         (`bun run dev:tauri` stages everything; you only need them to run the app)",
        names.join(", ")
    );

    // tauri_build watches only what it copies, so watch the dropped paths
    // here to pick up a later staging step. A path that doesn't exist yet is
    // watched through its nearest existing ancestor: cargo treats a missing
    // path as always changed, which would rerun this script on every build.
    for path in &missing {
        if let Some(watched) = path
            .ancestors()
            .find(|p| !p.as_os_str().is_empty() && p.exists())
        {
            println!("cargo:rerun-if-changed={}", watched.display());
        }
    }
}

fn main() {
    // Compile the Pro-tier proto package alongside the standard Tauri build.
    // The .proto lives under `proto/` in this crate (vendored from
    // `nodespace-sync/nodespaced-pro/proto/`). When sync is checked out
    // as a sibling, `scripts/refresh-pro-proto.ts` re-vendors from the
    // source-of-truth.
    let protoc = protoc_bin_vendored::protoc_bin_path()
        .expect("protoc-bin-vendored is required for the Pro proto build");
    // `set_var` is fine on edition 2021 — build scripts are
    // single-threaded by Cargo's contract. Once the workspace bumps
    // to edition 2024 (or tonic-build past 0.12 lands a
    // `protoc_executable` builder), switch to the builder-method
    // form to stay forward-compatible without the `unsafe` wrap
    // edition 2024 will require for env mutation.
    std::env::set_var("PROTOC", &protoc);
    tonic_build::configure()
        .build_server(false) // Tauri client only; daemon defines the server.
        .compile_protos(&["proto/nodespace_pro.proto"], &["proto"])
        .expect("failed to compile nodespace.pro.v1 proto");

    println!("cargo:rerun-if-changed=proto/nodespace_pro.proto");

    // Must run before tauri_build::build(): that call is what performs the
    // unconditional, direction-reversing copy this guards against.
    sync_external_bin_staging();
    drop_unstaged_bundle_entries();

    tauri_build::build()
}
