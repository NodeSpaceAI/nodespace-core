use std::env;
use std::path::PathBuf;

/// `externalBin` entries from `tauri.conf.json` that the workspace itself
/// builds (so have a `target/<profile>/<bin>` to compare against), without the
/// `binaries/` prefix or platform triple. The skill installer is built by
/// `build:skill`, not cargo, so it has no entry. If a new cargo-built sidecar
/// is added there, add its bin name here too, or it simply won't get the
/// staleness guard below — every other `externalBin` behaviour (including
/// tauri-build's own copy step) keeps working either way.
const EXTERNAL_BIN_NAMES: &[&str] = &["nodespaced", "nodespace"];

/// Reconciles each `externalBin` sidecar's staging copy
/// (`src-tauri/binaries/<bin>-<triple>`) with the workspace's own build
/// output (`target/<profile>/<bin>`) so that whichever is newer wins,
/// *before* `tauri_build::build()` runs its own unconditional copy in the
/// opposite direction. See `nodespace_app_build`'s `sync_stale_sidecar` for
/// the full story on why this exists.
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

        match nodespace_app_build::sync_stale_sidecar(&target_bin, &sidecar_bin) {
            Ok(true) => println!(
                "cargo:warning=refreshed stale sidecar staging file {} from {} \
                 (see nodespace_app_build::sync_stale_sidecar for why)",
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

fn main() {
    // Must run before tauri_build::build(): that call is what performs the
    // unconditional, direction-reversing copy this guards against.
    sync_external_bin_staging();
    // A debug build leaves whatever is unstaged out of its bundle, so building
    // this crate needs no staged sidecar or skill bundle. `dev:tauri` stages
    // the sidecars but not the skill bundle: a dev app installs the skill from
    // the source checkout's `packages/skill/dist/install.js`, which it builds
    // instead. The Tauri-seam tests don't depend on the staged daemon either:
    // the gate points them at `target/debug/nodespaced` via
    // `NODESPACED_TEST_BIN`. A release build stays strict, and `tauri:build`
    // stages everything first.
    nodespace_app_build::drop_unstaged_bundle_entries();

    tauri_build::build()
}
