//! The embedding model bundled with the app, copied into the daemon's model
//! directory on launch.
//!
//! The daemon loads its embedding model from `models/` under its state
//! directory, and looks only when it starts. The app bundle carries the model
//! under `resources/models/`, where the daemon never looks, so before the app
//! starts the daemon or reconnects to it, [`provision_bundled_model`] copies the
//! bundled file across when no model is there yet:
//!
//! - Anything already at the target path is kept, even a file that fails
//!   verification. Deleting it makes the next launch copy again.
//! - The copy goes to a temporary file in the target directory. It is flushed
//!   to disk, checked against the pinned SHA-256, then renamed into place, so
//!   the daemon never finds a partial or unverified model. A failure removes the
//!   temporary file.
//! - Nothing writes inside the app bundle. The daemon checks the copy again when
//!   it loads it, and keeps its verified-state record next to the copy.
//! - A failure is logged and never stops startup. The daemon then runs with
//!   semantic search off, and the next launch tries again.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager, Runtime};

/// The embedding model's file name: the one name the daemon looks for in its
/// model directory, and the one file the release bundles under
/// `resources/models/`.
const EMBEDDING_MODEL_FILE: &str = "nomic-embed-text-v1.5.Q8_0.gguf";

/// SHA-256 of [`EMBEDDING_MODEL_FILE`], lowercase hex: the digest the daemon
/// requires before it loads the model (ADR-058). The pin lives in
/// `nodespace-nlp-engine`, which the app does not link, so it is repeated here,
/// and a test keeps the two equal.
const EMBEDDING_MODEL_SHA256: &str =
    "3e24342164b3d94991ba9692fdc0dd08e3fd7362e0aacc396a9a5c54a544c3b7";

/// What one launch's provisioning did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provisioned {
    /// The bundled model was copied into place.
    Copied,
    /// Something is already at the target path, and it was kept.
    AlreadyPresent,
    /// The app bundle carries no model, as in a development build.
    NoBundledModel,
}

/// Copies the bundled embedding model into the daemon's model directory when no
/// model is there yet. Call it before the daemon is started or reconnected: the
/// daemon looks for the model only when it starts.
///
/// Never fails. A failure is logged, and the daemon runs with semantic search
/// off until a later launch's copy succeeds.
pub(crate) async fn provision_bundled_model<R: Runtime>(app: &AppHandle<R>) {
    let bundled = match app
        .path()
        .resolve(bundled_model_resource(), BaseDirectory::Resource)
    {
        Ok(path) => path,
        Err(e) => {
            tracing::warn!(error = %e, "cannot resolve the bundled embedding model's path; not copying it");
            return;
        }
    };
    let Some(home) = nodespace_home_from_env() else {
        tracing::warn!(
            "cannot resolve the NodeSpace home directory; not copying the bundled embedding model"
        );
        return;
    };
    let target = model_target_path(&home);

    let logged_target = target.clone();
    let result =
        tokio::task::spawn_blocking(move || provision(&bundled, &target, EMBEDDING_MODEL_SHA256))
            .await;
    match result {
        Ok(Ok(Provisioned::Copied)) => {
            tracing::info!(target = %logged_target.display(), "copied the bundled embedding model");
        }
        Ok(Ok(Provisioned::AlreadyPresent)) => {
            tracing::debug!(target = %logged_target.display(), "an embedding model is already in place");
        }
        Ok(Ok(Provisioned::NoBundledModel)) => {
            tracing::debug!("the app bundle carries no embedding model");
        }
        Ok(Err(e)) => {
            tracing::warn!(
                target = %logged_target.display(),
                error = format!("{e:#}"),
                "could not copy the bundled embedding model; semantic search stays off until a later launch copies it"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "the bundled embedding model copy did not finish");
        }
    }
}

/// The bundled model's path relative to the app's resource directory, where
/// `tauri.conf.json`'s `resources/models/**/*` entry places it.
fn bundled_model_resource() -> PathBuf {
    Path::new("resources")
        .join("models")
        .join(EMBEDDING_MODEL_FILE)
}

/// Where the daemon looks for the model under the NodeSpace home `home`.
fn model_target_path(home: &Path) -> PathBuf {
    home.join(nodespace_proto::socket::STATE_DIR)
        .join("models")
        .join(EMBEDDING_MODEL_FILE)
}

/// The NodeSpace home by the daemon's rule: `NODESPACE_HOME` when it is set,
/// else the user's home directory.
fn nodespace_home(
    nodespace_home_var: Option<String>,
    user_home: Option<PathBuf>,
) -> Option<PathBuf> {
    nodespace_home_var.map(PathBuf::from).or(user_home)
}

/// [`nodespace_home`] from this process's environment. `std::env::var`, as the
/// daemon reads it, so a value that is not valid UTF-8 counts as unset on both
/// sides.
fn nodespace_home_from_env() -> Option<PathBuf> {
    nodespace_home(std::env::var("NODESPACE_HOME").ok(), dirs::home_dir())
}

/// Copies `bundled` to `target` unless [`skip_reason`] says there is nothing to
/// do. On success `target` holds a complete model matching `expected_sha256`.
fn provision(bundled: &Path, target: &Path, expected_sha256: &str) -> Result<Provisioned> {
    if let Some(reason) = skip_reason(bundled, target)? {
        return Ok(reason);
    }
    copy_verified(bundled, target, expected_sha256)
}

/// Why this launch copies nothing, or `None` when it should copy.
///
/// Any entry at `target` is kept, so the check does not follow links: a broken
/// link there is still the user's, not a gap to fill.
fn skip_reason(bundled: &Path, target: &Path) -> io::Result<Option<Provisioned>> {
    if entry_exists(target)? {
        return Ok(Some(Provisioned::AlreadyPresent));
    }
    if !bundled.is_file() {
        return Ok(Some(Provisioned::NoBundledModel));
    }
    Ok(None)
}

/// Whether anything (a file, a directory or a link, broken or not) is at `path`.
fn entry_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Copies `bundled` to `target` through a temporary file in `target`'s
/// directory: copied and flushed to disk, checked against `expected_sha256`, then
/// renamed into place. Any failure removes the temporary file, so `target`
/// either does not exist or holds the verified model.
///
/// Returns [`Provisioned::AlreadyPresent`], keeping what is there, when
/// something appeared at `target` while the copy ran.
fn copy_verified(bundled: &Path, target: &Path, expected_sha256: &str) -> Result<Provisioned> {
    let dir = target
        .parent()
        .context("the model path has no parent directory")?;
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    remove_stale_temp_files(dir);

    let temp = temp_path(target);
    let result =
        write_verified(bundled, &temp, expected_sha256).and_then(|()| commit(&temp, target));
    if !matches!(result, Ok(Provisioned::Copied)) {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// This process's temporary path for `target`: `<file>.tmp-<pid>` beside it.
/// The pid keeps the names of two concurrent copies (a release and a
/// development build, say) apart. One copy's stale-file cleanup can still
/// remove the other's file part-way; that copy then fails with a warning, and
/// the next launch tries again.
fn temp_path(target: &Path) -> PathBuf {
    target.with_file_name(format!("{}{}", temp_prefix(), std::process::id()))
}

fn temp_prefix() -> String {
    format!("{EMBEDDING_MODEL_FILE}.tmp-")
}

/// Removes temporary files an earlier copy left when its process died part-way.
/// Best effort: one that cannot be removed only costs disk space.
fn remove_stale_temp_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let prefix = temp_prefix();
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Copies `bundled` to `temp`, flushes the copy to disk and checks its digest.
fn write_verified(bundled: &Path, temp: &Path, expected_sha256: &str) -> Result<()> {
    let copied = if cfg!(target_os = "macos") {
        clone_to_temp(bundled, temp)
    } else {
        stream_to_temp(bundled, temp)
    };
    copied.with_context(|| format!("cannot copy {} to {}", bundled.display(), temp.display()))?;
    let actual =
        file_sha256(temp).with_context(|| format!("cannot read {} to check it", temp.display()))?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        bail!(
            "the bundled model {} does not match the pinned SHA-256: expected {expected_sha256}, got {actual}",
            bundled.display()
        );
    }
    Ok(())
}

/// `fs::copy`, which clones the file on APFS, so the copy takes no second
/// 146 MB of disk. The clone keeps the bundle's mode, which may be read-only,
/// and Unix flushes through any descriptor, so it is flushed through a
/// read-only one.
fn clone_to_temp(bundled: &Path, temp: &Path) -> io::Result<()> {
    fs::copy(bundled, temp)?;
    File::open(temp)?.sync_all()
}

/// Creates `temp` and streams the bytes into it, flushing through the same
/// handle. Unlike `fs::copy`, this carries over none of the installed file's
/// attributes: on Windows `CopyFileExW` would copy a read-only attribute, and
/// Windows flushes only through a handle opened for writing.
fn stream_to_temp(bundled: &Path, temp: &Path) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp)?;
    io::copy(&mut File::open(bundled)?, &mut file)?;
    file.sync_all()
}

/// Renames `temp` to `target` unless something is at `target` by now, then
/// flushes the directory so the rename itself survives a crash.
///
/// The check and the rename are two steps, and the rename replaces an existing
/// file. Something written to `target` in the microseconds between them would
/// be replaced. The only writer expected there is another launch's copy of these
/// same verified bytes.
fn commit(temp: &Path, target: &Path) -> Result<Provisioned> {
    if entry_exists(target)? {
        return Ok(Provisioned::AlreadyPresent);
    }
    fs::rename(temp, target)
        .with_context(|| format!("cannot rename {} to {}", temp.display(), target.display()))?;
    if let Some(dir) = target.parent() {
        sync_dir(dir);
    }
    Ok(Provisioned::Copied)
}

/// Lowercase-hex SHA-256 of the file at `path`.
fn file_sha256(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(path)?, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Flushes `dir`'s entries. Best effort: if it fails, a crash can at worst lose
/// the rename, and the next launch copies again. On Windows `File::open` cannot
/// open a directory, so there it does nothing.
fn sync_dir(dir: &Path) {
    if cfg!(windows) {
        return;
    }
    if let Err(e) = File::open(dir).and_then(|d| d.sync_all()) {
        tracing::debug!(dir = %dir.display(), error = %e, "could not flush the model directory");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stand-in model bytes. Every test uses a tiny placeholder and its own
    /// digest; nothing here is a real model, and nothing loads one.
    const PLACEHOLDER: &[u8] = b"placeholder embedding model";

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    /// A resource directory holding a placeholder bundled model, and that
    /// model's path.
    fn bundle_with_model() -> (tempfile::TempDir, PathBuf) {
        let resources = tempfile::tempdir().expect("resource dir");
        let bundled = resources.path().join(bundled_model_resource());
        fs::create_dir_all(bundled.parent().unwrap()).expect("bundled models dir");
        fs::write(&bundled, PLACEHOLDER).expect("placeholder model");
        (resources, bundled)
    }

    /// The names in `dir`, sorted.
    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn copies_the_bundled_model_when_none_is_present() {
        let (_resources, bundled) = bundle_with_model();
        let home = tempfile::tempdir().expect("home");
        let target = model_target_path(home.path());

        let outcome = provision(&bundled, &target, &sha256_hex(PLACEHOLDER)).expect("provision");

        assert_eq!(outcome, Provisioned::Copied);
        assert_eq!(fs::read(&target).expect("copied model"), PLACEHOLDER);
        assert_eq!(
            entries(target.parent().unwrap()),
            [EMBEDDING_MODEL_FILE],
            "the model directory holds the model and no temporary file"
        );
    }

    #[test]
    fn an_existing_model_is_never_overwritten() {
        let (_resources, bundled) = bundle_with_model();
        let home = tempfile::tempdir().expect("home");
        let target = model_target_path(home.path());
        fs::create_dir_all(target.parent().unwrap()).expect("models dir");
        fs::write(&target, b"the user's own model").expect("existing model");

        let outcome = provision(&bundled, &target, &sha256_hex(PLACEHOLDER)).expect("provision");

        assert_eq!(outcome, Provisioned::AlreadyPresent);
        assert_eq!(fs::read(&target).expect("model"), b"the user's own model");
    }

    /// A broken link at the target path is the user's too: the check does not
    /// follow links, so the copy keeps it rather than renaming over it.
    #[cfg(unix)]
    #[test]
    fn a_broken_link_at_the_target_is_kept() {
        let (_resources, bundled) = bundle_with_model();
        let home = tempfile::tempdir().expect("home");
        let target = model_target_path(home.path());
        fs::create_dir_all(target.parent().unwrap()).expect("models dir");
        let missing = home.path().join("moved-elsewhere.gguf");
        std::os::unix::fs::symlink(&missing, &target).expect("link");

        let outcome = provision(&bundled, &target, &sha256_hex(PLACEHOLDER)).expect("provision");

        assert_eq!(outcome, Provisioned::AlreadyPresent);
        assert_eq!(fs::read_link(&target).expect("still a link"), missing);
    }

    /// A model that appears at the target while the copy runs is kept, and the
    /// copy's temporary file is removed.
    #[test]
    fn a_model_that_appears_during_the_copy_is_kept() {
        let (_resources, bundled) = bundle_with_model();
        let home = tempfile::tempdir().expect("home");
        let target = model_target_path(home.path());
        fs::create_dir_all(target.parent().unwrap()).expect("models dir");
        fs::write(&target, b"written meanwhile").expect("concurrent model");

        // Past `skip_reason`, as a copy that started before the file appeared.
        let outcome =
            copy_verified(&bundled, &target, &sha256_hex(PLACEHOLDER)).expect("copy_verified");

        assert_eq!(outcome, Provisioned::AlreadyPresent);
        assert_eq!(fs::read(&target).expect("model"), b"written meanwhile");
        assert_eq!(entries(target.parent().unwrap()), [EMBEDDING_MODEL_FILE]);
    }

    /// A copy that fails its digest check leaves neither the model nor a
    /// partial temporary file behind.
    #[test]
    fn a_failed_copy_leaves_no_partial_file() {
        let (_resources, bundled) = bundle_with_model();
        let home = tempfile::tempdir().expect("home");
        let target = model_target_path(home.path());

        let err = provision(&bundled, &target, &sha256_hex(b"some other model"))
            .expect_err("a digest mismatch fails the copy");

        assert!(
            format!("{err:#}").contains("does not match the pinned SHA-256"),
            "unexpected error: {err:#}"
        );
        assert!(!target.exists(), "no model is put in place");
        assert!(
            entries(target.parent().unwrap()).is_empty(),
            "no temporary file is left behind"
        );
    }

    /// The copy used off macOS writes a new file instead of copying the
    /// installed one's attributes, so a read-only bundle still gives a temporary
    /// file that opens for writing, which Windows needs to flush it.
    #[test]
    fn the_streamed_copy_is_writable_even_from_a_read_only_bundle() {
        let (_resources, bundled) = bundle_with_model();
        let mut read_only = fs::metadata(&bundled).expect("bundle").permissions();
        read_only.set_readonly(true);
        fs::set_permissions(&bundled, read_only).expect("make the bundle read-only");
        let dir = tempfile::tempdir().expect("dir");
        let temp = dir.path().join("copy");

        stream_to_temp(&bundled, &temp).expect("stream copy");

        assert_eq!(fs::read(&temp).expect("copy"), PLACEHOLDER);
        assert!(!fs::metadata(&temp).expect("copy").permissions().readonly());
        fs::OpenOptions::new()
            .write(true)
            .open(&temp)
            .expect("the copy opens for writing");
    }

    /// A read-only bundle, as an install can leave it, still provisions through
    /// whichever copy this platform uses: on macOS the clone keeps the
    /// read-only mode, so it must flush without opening for writing.
    #[test]
    fn a_read_only_bundle_is_copied() {
        let (_resources, bundled) = bundle_with_model();
        let mut read_only = fs::metadata(&bundled).expect("bundle").permissions();
        read_only.set_readonly(true);
        fs::set_permissions(&bundled, read_only).expect("make the bundle read-only");
        let home = tempfile::tempdir().expect("home");
        let target = model_target_path(home.path());

        let outcome = provision(&bundled, &target, &sha256_hex(PLACEHOLDER)).expect("provision");

        assert_eq!(outcome, Provisioned::Copied);
        assert_eq!(fs::read(&target).expect("copied model"), PLACEHOLDER);
    }

    /// Temporary files from an earlier copy whose process died are removed by
    /// the next copy.
    #[test]
    fn a_dead_copys_temporary_file_is_removed() {
        let (_resources, bundled) = bundle_with_model();
        let home = tempfile::tempdir().expect("home");
        let target = model_target_path(home.path());
        let models = target.parent().unwrap().to_path_buf();
        fs::create_dir_all(&models).expect("models dir");
        fs::write(models.join(format!("{}1", temp_prefix())), b"half a mod").expect("stale");

        let outcome = provision(&bundled, &target, &sha256_hex(PLACEHOLDER)).expect("provision");

        assert_eq!(outcome, Provisioned::Copied);
        assert_eq!(entries(&models), [EMBEDDING_MODEL_FILE]);
    }

    /// A bundle without the model (a development build) copies nothing and
    /// creates nothing.
    #[test]
    fn a_missing_bundled_model_is_a_no_op() {
        let resources = tempfile::tempdir().expect("resource dir");
        let bundled = resources.path().join(bundled_model_resource());
        let home = tempfile::tempdir().expect("home");
        let target = model_target_path(home.path());

        let outcome = provision(&bundled, &target, &sha256_hex(PLACEHOLDER)).expect("provision");

        assert_eq!(outcome, Provisioned::NoBundledModel);
        assert!(
            entries(home.path()).is_empty(),
            "nothing is created in the home"
        );
    }

    #[test]
    fn nodespace_home_wins_over_the_user_home() {
        assert_eq!(
            nodespace_home(
                Some("/isolated".into()),
                Some(PathBuf::from("/Users/someone"))
            ),
            Some(PathBuf::from("/isolated"))
        );
        assert_eq!(
            nodespace_home(None, Some(PathBuf::from("/Users/someone"))),
            Some(PathBuf::from("/Users/someone"))
        );
        assert_eq!(nodespace_home(None, None), None);
    }

    /// With `NODESPACE_HOME` set, the model lands under it and the user's home
    /// gets nothing. `HOME` points at a scratch directory standing in for the
    /// real one. The environment is process-global: nextest runs each test in
    /// its own process, and every variable is restored before the asserts.
    #[test]
    fn the_copy_follows_nodespace_home() {
        let (_resources, bundled) = bundle_with_model();
        let isolated = tempfile::tempdir().expect("isolated home");
        let user_home = tempfile::tempdir().expect("user home");

        const VARS: [&str; 2] = ["NODESPACE_HOME", "HOME"];
        let saved: Vec<_> = VARS.iter().map(std::env::var_os).collect();
        std::env::set_var("NODESPACE_HOME", isolated.path());
        std::env::set_var("HOME", user_home.path());
        let home = nodespace_home_from_env();
        for (var, value) in VARS.iter().zip(saved) {
            match value {
                Some(value) => std::env::set_var(var, value),
                None => std::env::remove_var(var),
            }
        }

        let home = home.expect("a home");
        assert_eq!(home, isolated.path());
        let target = model_target_path(&home);
        let outcome = provision(&bundled, &target, &sha256_hex(PLACEHOLDER)).expect("provision");

        assert_eq!(outcome, Provisioned::Copied);
        assert_eq!(fs::read(&target).expect("copied model"), PLACEHOLDER);
        assert!(
            entries(user_home.path()).is_empty(),
            "the user's home gets nothing"
        );
    }

    #[test]
    fn the_target_is_the_daemons_model_path() {
        assert_eq!(
            model_target_path(Path::new("/home")),
            Path::new("/home")
                .join(".nodespace")
                .join("models")
                .join("nomic-embed-text-v1.5.Q8_0.gguf")
        );
    }

    /// The daemon refuses a model whose digest differs from nlp-engine's pin,
    /// so the app must check against the same one.
    #[test]
    fn the_pinned_digest_matches_nlp_engines() {
        let config = include_str!("../../../nlp-engine/src/config.rs");
        assert!(
            config.contains(&format!("\"{EMBEDDING_MODEL_SHA256}\"")),
            "EMBEDDING_MODEL_SHA256 differs from nlp-engine's pin; rotate both together"
        );
    }

    /// The daemon looks for exactly one file name, and the release bundles one
    /// file under `resources/models/`; both must be the one this copies.
    #[test]
    fn the_file_name_matches_the_daemon_and_the_release() {
        let daemon = include_str!("../../../daemon/src/services/assembly.rs");
        assert!(
            daemon.contains(&format!(".join(\"{EMBEDDING_MODEL_FILE}\")")),
            "the daemon's model lookup names another file"
        );
        let release = include_str!("../../../../.github/workflows/release.yml");
        let bundling: Vec<&str> = release
            .lines()
            .filter(|line| {
                line.contains("gh release download") && line.contains("resources/models")
            })
            .collect();
        assert!(!bundling.is_empty(), "the release bundles no model");
        let pattern = format!("--pattern \"{EMBEDDING_MODEL_FILE}\"");
        assert!(
            bundling.iter().all(|line| line.contains(&pattern)),
            "the release bundles another file under resources/models/: {bundling:?}"
        );
        let tauri_conf = include_str!("../../src-tauri/tauri.conf.json");
        assert!(
            tauri_conf.contains("\"resources/models/**/*\""),
            "the app bundle no longer includes resources/models/"
        );
    }

    /// The daemon reads the model path only when it starts, so the copy runs
    /// before the startup task starts or reconnects to it, exactly once.
    #[test]
    fn startup_copies_the_model_before_it_starts_the_daemon() {
        let lib = include_str!("lib.rs");
        let copy = lib
            .find("bundled_model::provision_bundled_model(&app_handle).await;")
            .expect("startup copies the bundled model");
        let start = lib
            .find("match ensure_daemon_running(&app_handle).await")
            .expect("startup starts the daemon");
        assert!(copy < start, "the copy runs before the daemon starts");
        assert_eq!(lib.matches("provision_bundled_model(").count(), 1);
    }
}
