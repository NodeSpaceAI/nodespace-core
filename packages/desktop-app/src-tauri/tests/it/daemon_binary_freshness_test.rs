//! Guards the ADR-048 suite against the failure mode that is hardest to
//! diagnose from inside it: running a real `nodespaced` that is not a build of
//! this checkout.
//!
//! An out-of-date daemon still speaks the current gRPC surface, so every RPC in
//! these tests succeeds and only the daemon's *behaviour* differs. That
//! surfaced once as `ai_chat_send_to_idle_test` timing out after 300s with the
//! chat node back at `turnStatus: "idle"` and no assistant reply — the pre-existing
//! silent-idle-on-inference-failure behaviour of an old daemon, indistinguishable
//! from a regression in whatever change was being pushed. Nothing in the repo
//! keeps either candidate binary (the hand-staged `src-tauri/binaries/` sidecar,
//! or `target/debug/nodespaced`) in sync with the source tree, and cargo's own
//! mtime-based freshness check treats artifacts copied in from another checkout
//! as up to date, so the fixture checks it directly.
//!
//! These cases drive `daemon_binary_freshness` against synthetic trees rather
//! than the real workspace: the real answer depends on when the machine last
//! built, which is exactly the thing under test.

use std::path::Path;
use std::time::{Duration, SystemTime};

use nodespace_app_test_support::daemon_binary_freshness;

/// Write `path` (creating parents) and stamp its modification time.
fn write_at(path: &Path, mtime: SystemTime) {
    std::fs::create_dir_all(path.parent().expect("path must have a parent"))
        .expect("create parent dirs");
    let file = std::fs::File::create(path).expect("create file");
    file.set_modified(mtime).expect("set modification time");
}

/// A workspace root with one daemon source file stamped at `source_mtime`.
fn workspace_with_daemon_source(root: &Path, source_mtime: SystemTime) {
    write_at(&root.join("packages/daemon/src/main.rs"), source_mtime);
}

#[test]
fn a_binary_older_than_the_daemon_sources_is_rejected() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path();
    let now = SystemTime::now();

    workspace_with_daemon_source(root, now);
    let binary = root.join("target/debug/nodespaced");
    write_at(&binary, now - Duration::from_secs(60 * 60 * 24 * 30));

    let err = daemon_binary_freshness(&binary, root)
        .expect_err("a month-old binary against just-written sources must be refused");
    assert!(
        err.contains("is OLDER than"),
        "the message must say which way the mismatch runs, got: {err}"
    );
    assert!(
        err.contains("main.rs"),
        "the message must name the source file that outdates the binary, got: {err}"
    );
    assert!(
        err.contains("cargo build --bin nodespaced"),
        "the message must say how to fix it, got: {err}"
    );
}

#[test]
fn a_binary_newer_than_the_daemon_sources_is_accepted() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path();
    let now = SystemTime::now();

    workspace_with_daemon_source(root, now - Duration::from_secs(60));
    let binary = root.join("target/debug/nodespaced");
    write_at(&binary, now);

    assert!(
        daemon_binary_freshness(&binary, root).is_ok(),
        "a binary built after the last source edit is the normal, passing case"
    );
}

/// The newest source file decides, not the first one found: a daemon rebuilt
/// after a `packages/daemon` edit is still stale if `packages/agent` (which is
/// compiled into it) changed afterwards. `packages/agent`'s turn-completion
/// code is precisely where this suite's ai-chat behaviour comes from.
#[test]
fn staleness_is_judged_against_every_crate_compiled_into_the_daemon() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path();
    let now = SystemTime::now();

    write_at(
        &root.join("packages/daemon/src/main.rs"),
        now - Duration::from_secs(600),
    );
    let binary = root.join("target/debug/nodespaced");
    write_at(&binary, now - Duration::from_secs(300));
    assert!(
        daemon_binary_freshness(&binary, root).is_ok(),
        "binary postdates the only source so far"
    );

    write_at(
        &root.join("packages/agent/src/local_agent/agent_loop.rs"),
        now,
    );
    let err = daemon_binary_freshness(&binary, root)
        .expect_err("an agent-crate edit after the build must invalidate the binary");
    assert!(
        err.contains("agent_loop.rs"),
        "the message must name the crate that moved on, got: {err}"
    );
}

/// Unknowable inputs stay silent. This check exists to catch one specific
/// mismatch, not to become a second way for the fixture to refuse to start —
/// a tree with no daemon sources to compare against, or a binary path that
/// cannot be stat'd, is not evidence of staleness.
#[test]
fn an_unanswerable_comparison_is_not_treated_as_stale() {
    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path();
    let now = SystemTime::now();

    let binary = root.join("target/debug/nodespaced");
    write_at(&binary, now - Duration::from_secs(60 * 60 * 24 * 365));
    assert!(
        daemon_binary_freshness(&binary, root).is_ok(),
        "no daemon sources under root — nothing to compare against"
    );

    workspace_with_daemon_source(root, now);
    assert!(
        daemon_binary_freshness(&root.join("target/debug/does-not-exist"), root).is_ok(),
        "a binary that cannot be stat'd is reported by the caller that tries to spawn it"
    );
}
