//! The daemon's extension points (ADR-082 §5), used the way a build composing
//! the daemon uses them: through `build_shared_services` and the public API
//! alone.
//!
//! This file records the daemon half of the extension API. A change to it
//! needs an `EXTENSION_API_VERSION` bump (ADR-082 §8), which
//! `scripts/check-extension-api-version.ts` enforces.
//!
//! Every test that calls `build_shared_services` first points the daemon's
//! home at a temporary directory and its embedding model at a file that does
//! not exist, so no test here reads the user's state or loads a model.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nodespace_core::{NodeService, NodeUpdate, SqliteStore};
use nodespace_daemon::{
    build_shared_services, DaemonExtensions, DaemonExtensionsError, DatabaseManager,
    DatabaseRequiresExtensions, SharedServices,
};
use tempfile::TempDir;

/// The extension id the fixture declares.
const FIXTURE_ID: &str = "fixture";

/// Points `NODESPACE_HOME` at a temporary directory and
/// `NODESPACED_MODEL_PATH` at a file that does not exist, and restores both
/// when dropped, a panic included. The environment is process-global; nextest
/// runs each test in its own process.
struct IsolatedDaemonHome {
    home: TempDir,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl IsolatedDaemonHome {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        let saved = ["NODESPACE_HOME", "NODESPACED_MODEL_PATH"]
            .map(|var| (var, std::env::var_os(var)))
            .into();
        std::env::set_var("NODESPACE_HOME", home.path());
        std::env::set_var(
            "NODESPACED_MODEL_PATH",
            home.path().join("no-model-here.gguf"),
        );
        Self { home, saved }
    }

    fn path(&self) -> &Path {
        self.home.path()
    }
}

impl Drop for IsolatedDaemonHome {
    fn drop(&mut self) {
        for (var, value) in self.saved.drain(..) {
            match value {
                Some(value) => std::env::set_var(var, value),
                None => std::env::remove_var(var),
            }
        }
    }
}

/// The daemon's shared services, built as a composing build builds them.
async fn shared_services(extensions: DaemonExtensions) -> SharedServices {
    let (shared, model_task) = build_shared_services(extensions)
        .await
        .expect("the daemon starts");
    assert!(
        model_task.is_none() && !shared.context.has_model,
        "no model is loaded"
    );
    shared
}

/// A closed database at `dir/name/name.sqlite` whose settings node requires
/// `ids`, marked through the core library the way an extension marks one.
async fn database_requiring(dir: &Path, name: &str, ids: &[&str]) -> PathBuf {
    let db_dir = dir.join(name);
    std::fs::create_dir_all(&db_dir).unwrap();
    let path = db_dir.join(format!("{name}.sqlite"));
    let mut store = Arc::new(SqliteStore::new(path.clone()).await.unwrap());
    let node_service = NodeService::new(&mut store).await.unwrap();
    let settings = node_service
        .get_node("database-settings-singleton")
        .await
        .unwrap()
        .unwrap();
    node_service
        .update_node(
            &settings.id,
            settings.version,
            NodeUpdate::new().with_properties(serde_json::json!({ "required_extensions": ids })),
        )
        .await
        .unwrap();
    path
}

/// The unsupported ids a database open was refused for, or `None` when it
/// opened.
async fn refused_for(manager: &DatabaseManager, path: PathBuf) -> Option<Vec<String>> {
    let id = manager.register(path).await.unwrap().id;
    match manager.get_or_open(&id).await {
        Ok(_) => None,
        Err(err) => Some(
            DatabaseRequiresExtensions::find_in(&err)
                .unwrap_or_else(|| panic!("a required-extensions refusal, got {err:#}"))
                .unsupported
                .clone(),
        ),
    }
}

/// A database marked with an id the build declares opens; one that also
/// lists an id the build does not declare is refused for that id alone.
#[tokio::test]
async fn a_database_requiring_a_supported_extension_opens() {
    let home = IsolatedDaemonHome::new();
    let shared = shared_services(DaemonExtensions::none().supported_extension(FIXTURE_ID)).await;
    let marked = database_requiring(home.path(), "marked", &[FIXTURE_ID]).await;
    let mixed = database_requiring(home.path(), "mixed", &[FIXTURE_ID, "other"]).await;
    let manager = DatabaseManager::load(home.path().join("databases.toml"), shared.context)
        .await
        .unwrap();

    assert_eq!(refused_for(&manager, marked).await, None);
    assert_eq!(
        refused_for(&manager, mixed).await,
        Some(vec!["other".to_string()])
    );
}

/// Core's own daemon supports no extension: the same database is refused.
#[tokio::test]
async fn core_refuses_a_database_requiring_any_extension() {
    let home = IsolatedDaemonHome::new();
    let shared = shared_services(DaemonExtensions::none()).await;
    let marked = database_requiring(home.path(), "marked", &[FIXTURE_ID]).await;
    let manager = DatabaseManager::load(home.path().join("databases.toml"), shared.context)
        .await
        .unwrap();

    assert_eq!(
        refused_for(&manager, marked).await,
        Some(vec![FIXTURE_ID.to_string()])
    );
}

/// A malformed extension id stops the daemon before it builds anything.
#[tokio::test]
async fn an_invalid_extension_id_stops_startup() {
    let _home = IsolatedDaemonHome::new();
    let err = build_shared_services(DaemonExtensions::none().supported_extension("Fixture"))
        .await
        .err()
        .expect("startup is refused");

    assert_eq!(
        err.downcast_ref::<DaemonExtensionsError>(),
        Some(&DaemonExtensionsError::InvalidExtensionId(
            "Fixture".to_string()
        ))
    );
}
