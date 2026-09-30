//! Shared between `build.rs` (which calls this for real, immediately before
//! `tauri_build::build()`) and `nodespace-app-test-support`, which re-exports
//! `sync_stale_sidecar` purely so `tests/it/sidecar_staging_sync_test.rs` can
//! exercise the real algorithm against synthetic trees — no duplicated
//! copy.
//!
//! Pulled into each crate root via `mod build_support;` / `#[path = ...]
//! mod build_support;` rather than published as its own workspace crate:
//! `nodespace-app` cannot take `nodespace-app-test-support` as a
//! `[build-dependencies]` (it links the whole app library and Tauri's test
//! runtime), and `nodespace-app-test-support` cannot depend on the
//! binary-only `nodespace-app`. A shared source file sidesteps both; no
//! crate boundary, no duplication.
//!
//! ## Why this exists
//!
//! `tauri-build`'s own handling of `bundle.externalBin` (`copy_binaries` in
//! `tauri-build-2.6.2/src/lib.rs`) copies each configured sidecar —
//! `src-tauri/binaries/<bin>-<target-triple>` — onto `target/<profile>/<bin>`
//! (the bare cargo build output name, stripped of its triple suffix) every
//! time `nodespace-app`'s build script runs. That copy is unconditional: it
//! `fs::remove_file`s whatever is already at the destination and overwrites
//! it, with no mtime or freshness check of any kind.
//!
//! Because `target/<profile>/` is the *whole workspace's* shared Cargo
//! output directory, `target/<profile>/nodespaced` is also exactly where
//! `cargo build --bin nodespaced` puts the daemon binary from an entirely
//! unrelated crate (`nodespace-daemon`). If the staged sidecar
//! (`src-tauri/binaries/nodespaced-<triple>`) is stale — e.g. it was last
//! refreshed via `bun run build:sidecars` days ago and never rebuilt since —
//! then simply building `nodespace-app` (a plain `cargo build -p
//! nodespace-app`, `cargo test -p nodespace-app`, or anything that reruns
//! its build script) silently clobbers a genuinely fresh
//! `target/<profile>/nodespaced` with those old bytes. This is the reverse
//! of the direction developers expect data to flow (stage the sidecar FROM
//! the build output, not the other way around), which is what makes it so
//! confusing to diagnose — see `daemon_binary_freshness` in
//! `nodespace-app-test-support`, which detects the resulting staleness but
//! cannot prevent it, since by the time a test runs the clobber has already
//! happened.
//!
//! [`sync_stale_sidecar`] closes that gap by running immediately before
//! `tauri_build::build()`, inside the same build script invocation: whichever
//! of the two files (`target/<profile>/<bin>` or the staged sidecar) is
//! newer wins and gets copied onto the other. When the just-built workspace
//! binary is newer, the stale sidecar is refreshed from it *before*
//! `tauri-build` gets a chance to copy the old sidecar back — so
//! `tauri-build`'s own unconditional copy becomes a same-bytes no-op instead
//! of a regression. When the sidecar happens to be newer (e.g. a
//! cross-compiled or hand-staged release binary with no corresponding local
//! `target/<profile>/` build), nothing is touched and `tauri-build` proceeds
//! exactly as it always has.

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

/// The file's modification time, or `None` if it does not exist.
fn mtime(path: &Path) -> io::Result<Option<SystemTime>> {
    match fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.modified()?)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Copies `src` onto `dest`, creating `dest`'s parent directory if needed and
/// preserving the executable bit on Unix (`fs::copy` already preserves
/// permissions bit-for-bit on Unix, but this makes the requirement explicit
/// and keeps parity with Windows, which has no executable bit to preserve).
fn copy_preserving_mode(src: &Path, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(dest)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        fs::set_permissions(dest, perms)?;
    }
    Ok(())
}

/// Reconciles a freshly cargo-built binary (`target_bin`, e.g.
/// `target/debug/nodespaced`) with its Tauri sidecar staging copy
/// (`sidecar_bin`, e.g. `src-tauri/binaries/nodespaced-<triple>`) so that
/// whichever of the two is newer is propagated onto the other — never the
/// reverse. Returns `Ok(true)` if `sidecar_bin` was refreshed from
/// `target_bin`, `Ok(false)` if nothing needed to change.
///
/// Deliberately one-directional in what it *writes*: it only ever refreshes
/// the sidecar from the target binary, never the other way around. Refreshing
/// `target_bin` from `sidecar_bin` here would be redundant — `tauri-build`'s
/// own `copy_binaries` step, which runs immediately after this in
/// `build.rs`, already does exactly that unconditionally. Since this
/// function guarantees the sidecar is never older than `target_bin` before
/// that step runs, `tauri-build`'s copy always ends up moving identical
/// bytes when `target_bin` was the newer side, and legitimately refreshes
/// `target_bin` when the sidecar genuinely was the newer side (e.g. a
/// release sidecar staged with no matching local debug build) — both cases
/// behave correctly with a single copy direction owned here.
///
/// A missing `sidecar_bin` is treated the same as an infinitely stale one:
/// if `target_bin` exists, the sidecar is created from it. A missing
/// `target_bin` is a no-op — there is nothing fresher to reconcile from, so
/// `tauri-build` proceeds exactly as it did before this existed (including
/// its existing hard error if the sidecar is *also* missing).
pub fn sync_stale_sidecar(target_bin: &Path, sidecar_bin: &Path) -> io::Result<bool> {
    let Some(target_mtime) = mtime(target_bin)? else {
        return Ok(false);
    };
    let sidecar_is_fresh =
        matches!(mtime(sidecar_bin)?, Some(sidecar_mtime) if sidecar_mtime >= target_mtime);
    if sidecar_is_fresh {
        return Ok(false);
    }

    copy_preserving_mode(target_bin, sidecar_bin)?;

    // Defensive assertion, not a normal-path check: if this ever fires, the
    // copy above silently didn't take (e.g. a filesystem that doesn't
    // preserve mtimes on copy), which would leave tauri-build free to
    // clobber `target_bin` with stale bytes again right after we return —
    // exactly the bug this function exists to prevent. Fail loudly at build
    // time instead of shipping that regression silently.
    let refreshed_mtime = mtime(sidecar_bin)?;
    assert!(
        matches!(refreshed_mtime, Some(m) if m >= target_mtime),
        "sync_stale_sidecar: refreshed {} from {} but it did not become at least as fresh \
         (mtime {:?} vs {:?}) — the copy did not take as expected",
        sidecar_bin.display(),
        target_bin.display(),
        refreshed_mtime,
        Some(target_mtime),
    );

    Ok(true)
}
