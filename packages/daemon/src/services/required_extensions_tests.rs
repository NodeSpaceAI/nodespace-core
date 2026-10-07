//! The required-extensions guard (ADR-083 §2) through the daemon's open and
//! listing paths.
//!
//! A context here supports the ids a test names, as a build composing the
//! daemon declares them through `DaemonExtensions::supported_extension`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nodespace_agent::pty::PtySessionManager;
use nodespace_core::services::EmbeddingScheduler;
use nodespace_core::{NodeService, NodeUpdate, SqliteStore};
use nodespace_nlp_engine::EmbeddingService;
use serde_json::json;
use tokio::sync::watch;

use super::assembly::{
    unrouted_services_if_default_refused, DatabaseRequiresExtensions, RequiredExtensionsUnreadable,
    SharedContext,
};
use super::database_manager::{DatabaseManager, DatabaseStatus};

const SETTINGS_ID: &str = "database-settings-singleton";

/// A model-less context supporting `supported`.
pub(crate) fn context(supported: &[&str]) -> SharedContext {
    let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
    SharedContext {
        pty_manager: Arc::new(PtySessionManager::new()),
        model,
        has_model: false,
        scheduler: Arc::new(EmbeddingScheduler::new()),
        subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
        local_agent: crate::SharedLocalAgent::new(),
        extensions: supported
            .iter()
            .fold(crate::DaemonExtensions::none(), |extensions, id| {
                extensions.supported_extension(*id)
            }),
    }
}

async fn manager(dir: &Path, supported: &[&str]) -> DatabaseManager {
    DatabaseManager::load(dir.join("databases.toml"), context(supported))
        .await
        .unwrap()
}

fn wal_of(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

/// Open `path` as a plain store, the way any app linking core's library
/// would, without the daemon's guard.
async fn open_store(path: &Path) -> (Arc<SqliteStore>, Arc<NodeService>) {
    let mut store = Arc::new(SqliteStore::new(path.to_path_buf()).await.unwrap());
    let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
    (store, node_service)
}

/// Set the settings singleton's `required_extensions`, as an extension does
/// when it marks a database.
async fn require(node_service: &NodeService, ids: &[&str]) {
    let settings = node_service.get_node(SETTINGS_ID).await.unwrap().unwrap();
    node_service
        .update_node(
            &settings.id,
            settings.version,
            NodeUpdate::new().with_properties(json!({ "required_extensions": ids })),
        )
        .await
        .unwrap();
}

/// A closed database at `dir/name` requiring `ids`, in a directory of its own
/// so a test can compare everything in it.
async fn database_requiring(dir: &Path, name: &str, ids: &[&str]) -> PathBuf {
    let db_dir = dir.join(name);
    std::fs::create_dir_all(&db_dir).unwrap();
    let path = db_dir.join(format!("{name}.sqlite"));
    let (store, node_service) = open_store(&path).await;
    require(&node_service, ids).await;
    drop(node_service);
    drop(store);
    assert!(!wal_of(&path).exists(), "the fixture closed cleanly");
    path
}

/// Every file in `dir` with its bytes, sorted by name.
fn contents(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

fn ids(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

/// The open is refused before anything touches the file: the daemon's open
/// seeds agent nodes into a database that lacks them, so a guard that ran
/// after seeding would change these bytes.
#[tokio::test]
async fn a_database_requiring_an_unsupported_extension_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = database_requiring(dir.path(), "marked", &["sync"]).await;
    let mgr = manager(dir.path(), &[]).await;
    let id = mgr.register(path.clone()).await.unwrap().id;
    let before = contents(path.parent().unwrap());

    let err = mgr
        .get_or_open(&id)
        .await
        .err()
        .expect("the open is refused");

    let refusal = DatabaseRequiresExtensions::find_in(&err).expect("a required-extensions refusal");
    assert_eq!(refusal.unsupported, ids(&["sync"]));
    let status = refusal.to_status();
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(
        status.message(),
        nodespace_proto::extension_names::refusal_message(&["sync"])
    );
    assert_eq!(
        nodespace_proto::requires_extension::unsupported_extensions(&status),
        Some(ids(&["sync"]))
    );
    assert_eq!(
        contents(path.parent().unwrap()),
        before,
        "the file is byte-identical and nothing was created or renamed beside it"
    );
    assert_eq!(
        mgr.list().await.databases[0].status,
        DatabaseStatus::RequiresExtension
    );
}

/// The singleton is found by its id after an extension retypes it to a
/// subtype of `database-settings`, and the requirement stays in the base
/// bucket where the guard reads it.
#[tokio::test]
async fn a_singleton_retyped_to_a_subtype_is_still_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("retyped");
    std::fs::create_dir_all(&db_dir).unwrap();
    let path = db_dir.join("db.sqlite");
    {
        let (_store, node_service) = open_store(&path).await;
        nodespace_core::schema::handle_create_schema(
            &node_service,
            json!({ "name": "Fixture Settings", "extends": "database-settings", "fields": [] }),
        )
        .await
        .unwrap();
        require(&node_service, &["sync"]).await;
        let settings = node_service.get_node(SETTINGS_ID).await.unwrap().unwrap();
        let retyped = node_service
            .update_node(
                &settings.id,
                settings.version,
                NodeUpdate {
                    node_type: Some("fixture-settings".to_string()),
                    ..NodeUpdate::new()
                },
            )
            .await
            .unwrap();
        assert_eq!(retyped.node_type, "fixture-settings");
    }
    assert!(!wal_of(&path).exists(), "the fixture closed cleanly");
    let mgr = manager(dir.path(), &[]).await;
    let id = mgr.register(path).await.unwrap().id;

    let err = mgr
        .get_or_open(&id)
        .await
        .err()
        .expect("the open is refused");

    assert_eq!(
        DatabaseRequiresExtensions::find_in(&err).map(|r| r.unsupported.clone()),
        Some(ids(&["sync"]))
    );
}

/// A requirement another process has written but not yet checkpointed is
/// still seen: the guard reads the WAL rather than only the main file.
#[tokio::test]
async fn a_requirement_only_in_the_wal_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("live");
    std::fs::create_dir_all(&db_dir).unwrap();
    let path = db_dir.join("db.sqlite");
    let (store, node_service) = open_store(&path).await;
    require(&node_service, &["sync"]).await;
    assert!(
        wal_of(&path).exists(),
        "the requirement is still in the WAL"
    );
    let mgr = manager(dir.path(), &[]).await;
    let id = mgr.register(path.clone()).await.unwrap().id;
    let before = std::fs::read(&path).unwrap();

    let err = mgr
        .get_or_open(&id)
        .await
        .err()
        .expect("the open is refused");

    assert!(
        DatabaseRequiresExtensions::find_in(&err).is_some(),
        "{err:#}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the main file is untouched"
    );
    drop(node_service);
    drop(store);
}

/// A daemon that supports an extension opens a database requiring it, and
/// still refuses one that also requires an extension it does not support,
/// naming only that one.
#[tokio::test]
async fn a_supported_extension_opens_and_an_unsupported_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let supported = database_requiring(dir.path(), "supported", &["fixture"]).await;
    let mixed = database_requiring(dir.path(), "mixed", &["fixture", "other"]).await;
    let mgr = manager(dir.path(), &["fixture"]).await;
    let supported_id = mgr.register(supported).await.unwrap().id;
    let mixed_id = mgr.register(mixed).await.unwrap().id;

    mgr.get_or_open(&supported_id)
        .await
        .expect("a database requiring only supported extensions opens");
    let err = mgr
        .get_or_open(&mixed_id)
        .await
        .err()
        .expect("the open is refused");

    assert_eq!(
        DatabaseRequiresExtensions::find_in(&err).map(|r| r.unsupported.clone()),
        Some(ids(&["other"]))
    );
    let listed = mgr.list().await;
    let status_of = |id| {
        listed
            .databases
            .iter()
            .find(|d| &d.entry.id == id)
            .map(|d| (d.status, d.unsupported_extensions.clone()))
            .unwrap()
    };
    assert_eq!(status_of(&supported_id), (DatabaseStatus::Open, vec![]));
    assert_eq!(
        status_of(&mixed_id),
        (DatabaseStatus::RequiresExtension, ids(&["other"]))
    );
    mgr.shutdown_all().await;
}

/// A database that was never opened is still marked when listed: the listing
/// probes each closed database without opening it, and writes nothing.
#[tokio::test]
async fn a_closed_never_opened_database_is_marked_in_the_listing() {
    let dir = tempfile::tempdir().unwrap();
    let marked = database_requiring(dir.path(), "marked", &["sync"]).await;
    let plain = database_requiring(dir.path(), "plain", &[]).await;
    let mgr = manager(dir.path(), &[]).await;
    mgr.register(marked.clone()).await.unwrap();
    mgr.register(plain).await.unwrap();
    let before = contents(marked.parent().unwrap());

    let snapshot = mgr.list().await;

    assert_eq!(
        snapshot
            .databases
            .iter()
            .map(|d| (d.status, d.unsupported_extensions.clone()))
            .collect::<Vec<_>>(),
        vec![
            (DatabaseStatus::RequiresExtension, ids(&["sync"])),
            (DatabaseStatus::Closed, vec![]),
        ]
    );
    assert_eq!(contents(marked.parent().unwrap()), before);

    #[cfg(unix)]
    {
        // The first registered database became the default.
        let entries = crate::tray::database_menu_entries(&snapshot);
        assert_eq!(
            entries[0].label,
            format!(
                "marked — default · {}",
                nodespace_proto::extension_names::requirement(&["sync"])
            )
        );
        assert_eq!(entries[1].label, "plain");
    }
}

/// A database whose required extensions cannot be read is refused without
/// touching it, and as the boot default it does not stop the daemon: the boot
/// fallback accepts it as it accepts a refusal, and passes any other failure
/// through.
#[tokio::test]
async fn an_unreadable_database_is_refused_and_does_not_stop_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("unreadable");
    std::fs::create_dir_all(&db_dir).unwrap();
    let path = db_dir.join("unreadable.sqlite");
    std::fs::write(&path, vec![0x42u8; 8192]).unwrap();
    let mgr = Arc::new(manager(dir.path(), &[]).await);
    let id = mgr.register(path.clone()).await.unwrap().id;
    let before = contents(&db_dir);

    // A routed request reports why, since the daemon keeps running with the
    // database closed.
    let mut request = tonic::Request::new(());
    request.extensions_mut().insert(mgr.clone());
    request.metadata_mut().insert(
        crate::DATABASE_ID_HEADER,
        tonic::metadata::MetadataValue::try_from(id.as_str()).unwrap(),
    );
    let status = crate::db_routing::routed_database_services(&request)
        .await
        .err()
        .expect("the routed request is refused");
    assert_eq!(status.code(), tonic::Code::Internal);
    assert!(
        status
            .message()
            .contains("could not read the required extensions"),
        "{}",
        status.message()
    );

    let err = mgr
        .get_or_open(&id)
        .await
        .err()
        .expect("the open fails closed");

    let unreadable = RequiredExtensionsUnreadable::find_in(&err).expect("the guard failed");
    assert_eq!(unreadable.path, path);
    assert!(DatabaseRequiresExtensions::find_in(&err).is_none());
    assert_eq!(contents(&db_dir), before);
    assert!(unrouted_services_if_default_refused(err, &context(&[]))
        .await
        .is_ok());
    assert!(unrouted_services_if_default_refused(
        anyhow::anyhow!("another failure"),
        &context(&[])
    )
    .await
    .is_err());
}
