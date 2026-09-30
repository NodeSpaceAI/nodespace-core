//! Guards against the mechanism behind a confusing false-positive
//! `daemon_binary_freshness_test.rs` was written to catch the *symptom* of,
//! not the cause: `tauri-build`'s handling of `bundle.externalBin`
//! unconditionally copies each staged sidecar
//! (`src-tauri/binaries/<bin>-<triple>`) onto `target/<profile>/<bin>` every
//! time `nodespace-app`'s build script runs, with no freshness check. Since
//! `target/<profile>/` is the whole workspace's shared Cargo output
//! directory, that copy silently clobbers a genuinely fresh `cargo build
//! --bin nodespaced` output with a stale sidecar whenever the sidecar
//! staging file (last refreshed via `bun run build:sidecars`) is older than
//! it — confirmed by hand: `rm target/debug/nodespaced`, `cargo build --bin
//! nodespaced` (fresh, correct), then simply building `nodespace-app`
//! reverted it to an old, stale copy.
//!
//! `build.rs` now calls `sync_stale_sidecar` immediately before
//! `tauri_build::build()` so the two files agree on the newer content first
//! — see `build_support.rs`'s module doc for the full mechanism. These cases
//! drive that same function (re-exported by
//! `nodespace-app-test-support` for exactly this purpose) against synthetic
//! trees rather than the real workspace, for the same reason
//! `daemon_binary_freshness_test.rs` does: the real answer depends on
//! whichever binaries happen to exist on the machine running the test.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use nodespace_app_test_support::sync_stale_sidecar;

fn write_at(path: &Path, mtime: SystemTime, contents: &[u8]) {
    fs::create_dir_all(path.parent().expect("path must have a parent"))
        .expect("create parent dirs");
    fs::write(path, contents).expect("write file");
    let file = fs::File::open(path).expect("reopen for mtime stamp");
    file.set_modified(mtime).expect("set modification time");
}

#[test]
fn a_stale_staged_sidecar_is_refreshed_from_the_fresher_build_output_before_tauri_build_runs() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let now = SystemTime::now();
    let target_bin = tmp.path().join("target/debug/nodespaced");
    let sidecar_bin = tmp.path().join("binaries/nodespaced-aarch64-apple-darwin");

    // Reproduces the exact failure mode: a sidecar staged days ago, and a
    // `cargo build --bin nodespaced` that just produced a genuinely fresh
    // binary in the shared target/ directory.
    write_at(
        &sidecar_bin,
        now - Duration::from_secs(60 * 60 * 24 * 30),
        b"stale sidecar from weeks ago",
    );
    write_at(&target_bin, now, b"the daemon this checkout just built");

    let refreshed = sync_stale_sidecar(&target_bin, &sidecar_bin).expect("sync succeeds");

    assert!(
        refreshed,
        "a sidecar staged before the fresh build output must be refreshed from it, or \
         tauri-build's own copy_binaries would clobber target/debug/nodespaced right back to \
         these stale bytes"
    );
    assert_eq!(
        fs::read(&sidecar_bin).expect("read sidecar"),
        b"the daemon this checkout just built",
        "tauri-build copies FROM this file INTO target/debug/nodespaced next — it must now \
         carry the fresh build's bytes, not the stale ones"
    );
}

#[test]
fn a_sidecar_at_least_as_fresh_as_the_build_output_is_left_alone() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let now = SystemTime::now();
    let target_bin = tmp.path().join("target/debug/nodespaced");
    let sidecar_bin = tmp.path().join("binaries/nodespaced-aarch64-apple-darwin");

    // The normal case after `bun run build:sidecars`: both files come from
    // the same build, sidecar staged last.
    write_at(
        &target_bin,
        now - Duration::from_secs(1),
        b"just-built daemon",
    );
    write_at(&sidecar_bin, now, b"freshly staged sidecar");

    let refreshed = sync_stale_sidecar(&target_bin, &sidecar_bin).expect("sync succeeds");

    assert!(
        !refreshed,
        "an already-fresh sidecar must not be touched — tauri-build's own copy handles this \
         case correctly on its own"
    );
    assert_eq!(
        fs::read(&sidecar_bin).expect("read sidecar"),
        b"freshly staged sidecar"
    );
}

#[test]
fn a_first_ever_build_with_no_staged_sidecar_yet_gets_one_created() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let target_bin = tmp.path().join("target/debug/nodespaced");
    let sidecar_bin = tmp.path().join("binaries/nodespaced-aarch64-apple-darwin");

    // A fresh worktree: `binaries/` is entirely .gitignore'd, so before the
    // first `bun run build:sidecars` it doesn't exist at all — only a bare
    // `cargo build --bin nodespaced` has run.
    write_at(
        &target_bin,
        SystemTime::now(),
        b"first build in a fresh worktree",
    );
    assert!(!sidecar_bin.exists());

    let refreshed = sync_stale_sidecar(&target_bin, &sidecar_bin).expect("sync succeeds");

    assert!(
        refreshed,
        "a missing sidecar is infinitely stale — treat it the same as an old one so \
         tauri-build's copy_binaries doesn't hard-fail with \"does not exist\""
    );
    assert_eq!(
        fs::read(&sidecar_bin).expect("read sidecar"),
        b"first build in a fresh worktree"
    );
}

#[test]
fn no_local_build_output_yet_leaves_the_sidecar_untouched() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let target_bin = tmp.path().join("target/debug/nodespaced");
    let sidecar_bin = tmp.path().join("binaries/nodespaced-aarch64-apple-darwin");
    write_at(
        &sidecar_bin,
        SystemTime::now(),
        b"a hand-staged or cross-compiled sidecar",
    );

    let refreshed = sync_stale_sidecar(&target_bin, &sidecar_bin).expect("sync succeeds");

    assert!(
        !refreshed,
        "with nothing fresher built locally, tauri-build's own copy_binaries proceeds exactly \
         as it did before this guard existed"
    );
    assert_eq!(
        fs::read(&sidecar_bin).expect("read sidecar"),
        b"a hand-staged or cross-compiled sidecar"
    );
}
