//! Acceptance tests for per-database compute scoping (ADR-053).
//!
//! These drive the `DatabaseManager` public API directly (no model needed) to
//! prove the per-database lifecycle guarantees:
//!   - an idle, non-default, non-active database is evicted and reopens
//!     transparently;
//!   - the default and the active database are never evicted;
//!   - `shutdown_all` drops every open database's compute;
//!   - per-database routing keeps one database's nodes invisible to another
//!     (no cross-database leak).
//!
//! Active-first embedding priority is proven cheaply by the `EmbeddingScheduler`
//! unit test in `nodespace-core` (grant ordering with a shared record), which
//! needs no llama model.

use std::sync::Arc;
use std::time::Duration;

use nodespace_agent::pty::PtySessionManager;
use nodespace_core::services::EmbeddingScheduler;
use nodespace_daemon::nodespace::node_service_server::NodeService as _;
use nodespace_daemon::nodespace::{CreateNodeRequest, GetNodeRequest};
use nodespace_daemon::services::DatabaseStatus;
use nodespace_daemon::{DatabaseManager, SharedContext, DATABASE_ID_HEADER};
use nodespace_nlp_engine::EmbeddingService;
use tempfile::TempDir;
use tokio::sync::watch;
use tonic::{Code, Request};

/// A model-less build context that shares an explicit scheduler so tests can
/// drive the active-database signal. `has_model = false` skips all embedding
/// wiring, so the dropped watch sender is harmless.
fn test_context() -> (SharedContext, Arc<EmbeddingScheduler>) {
    let scheduler = Arc::new(EmbeddingScheduler::new());
    let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
    let context = SharedContext {
        pty_manager: Arc::new(PtySessionManager::new()),
        model,
        has_model: false,
        model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        scheduler: scheduler.clone(),
        subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
        local_agent: nodespace_daemon::SharedLocalAgent::new(
            nodespace_daemon::nodespace_dir()
                .expect("nodespace dir")
                .join("daemon.toml"),
        ),
    };
    (context, scheduler)
}

async fn manager_with_two_dbs() -> (Arc<DatabaseManager>, Arc<EmbeddingScheduler>, TempDir) {
    let dir = TempDir::new().unwrap();
    let (context, scheduler) = test_context();
    let manager = Arc::new(
        DatabaseManager::load(dir.path().join("databases.toml"), context)
            .await
            .unwrap(),
    );
    manager
        .ensure_default_registered("Default".into(), dir.path().join("default.db"))
        .await
        .unwrap();
    manager
        .create("Second".into(), Some(dir.path().join("second.db")))
        .await
        .unwrap();
    (manager, scheduler, dir)
}

fn status_of(
    manager_snapshot: &nodespace_daemon::services::RegistrySnapshot,
    name: &str,
) -> DatabaseStatus {
    manager_snapshot
        .databases
        .iter()
        .find(|d| d.entry.name == name)
        .expect("database present in registry")
        .status
}

/// An idle, non-default, non-active database is evicted from the open set and
/// reopens transparently on its next request.
#[tokio::test]
async fn idle_non_default_database_is_evicted_then_reopens() {
    let (manager, _scheduler, _dir) = manager_with_two_dbs().await;
    let second_id = manager
        .list()
        .await
        .databases
        .iter()
        .find(|d| d.entry.name == "Second")
        .unwrap()
        .entry
        .id
        .clone();

    // Open the second database; it is now serving.
    manager.get_or_open(&second_id).await.unwrap();
    assert_eq!(
        status_of(&manager.list().await, "Second"),
        DatabaseStatus::Open
    );

    // Let its idle timer age past the eviction window, then run one sweep.
    tokio::time::sleep(Duration::from_millis(10)).await;
    manager.evict_idle_databases(Duration::from_millis(1)).await;

    // Evicted: no longer open, but the file remains so it reports Closed.
    assert_eq!(
        status_of(&manager.list().await, "Second"),
        DatabaseStatus::Closed,
        "idle non-default database must be evicted from the open set"
    );

    // A subsequent request rebuilds it transparently.
    manager.get_or_open(&second_id).await.unwrap();
    assert_eq!(
        status_of(&manager.list().await, "Second"),
        DatabaseStatus::Open,
        "evicted database must reopen on next request"
    );
}

/// The default database is never evicted, even when idle past the window.
#[tokio::test]
async fn default_database_is_never_evicted() {
    let (manager, _scheduler, _dir) = manager_with_two_dbs().await;
    let default_id = manager.list().await.default_database.unwrap();

    manager.get_or_open(&default_id).await.unwrap();
    assert_eq!(
        status_of(&manager.list().await, "Default"),
        DatabaseStatus::Open
    );

    tokio::time::sleep(Duration::from_millis(10)).await;
    manager.evict_idle_databases(Duration::from_millis(1)).await;

    assert_eq!(
        status_of(&manager.list().await, "Default"),
        DatabaseStatus::Open,
        "the default database must never be evicted"
    );
}

/// The active database (the one with a live edit stream) is never evicted while
/// active, even when idle past the window.
#[tokio::test]
async fn active_database_is_never_evicted() {
    let (manager, scheduler, _dir) = manager_with_two_dbs().await;
    let second_id = manager
        .list()
        .await
        .databases
        .iter()
        .find(|d| d.entry.name == "Second")
        .unwrap()
        .entry
        .id
        .clone();

    manager.get_or_open(&second_id).await.unwrap();
    // Mark the second database active, as a live WatchNodes stream would.
    scheduler.set_active(Some(second_id.to_string()));

    tokio::time::sleep(Duration::from_millis(10)).await;
    manager.evict_idle_databases(Duration::from_millis(1)).await;

    assert_eq!(
        status_of(&manager.list().await, "Second"),
        DatabaseStatus::Open,
        "the active database must never be evicted"
    );
}

/// `shutdown_all` drops every open database's compute and clears the open set.
#[tokio::test]
async fn shutdown_all_closes_every_open_database() {
    let (manager, _scheduler, _dir) = manager_with_two_dbs().await;
    let default_id = manager.list().await.default_database.unwrap();
    let second_id = manager
        .list()
        .await
        .databases
        .iter()
        .find(|d| d.entry.name == "Second")
        .unwrap()
        .entry
        .id
        .clone();

    manager.get_or_open(&default_id).await.unwrap();
    manager.get_or_open(&second_id).await.unwrap();

    manager.shutdown_all().await;

    let snapshot = manager.list().await;
    assert_ne!(status_of(&snapshot, "Default"), DatabaseStatus::Open);
    assert_ne!(status_of(&snapshot, "Second"), DatabaseStatus::Open);
}

/// Per-database routing isolates compute state: an ai-chat node created against
/// one database is invisible to another — no cross-database leak.
#[tokio::test]
async fn ai_chat_nodes_do_not_leak_across_databases() {
    let (manager, _scheduler, dir) = manager_with_two_dbs().await;
    let default_id = manager.list().await.default_database.unwrap();
    let second_id = manager
        .list()
        .await
        .databases
        .iter()
        .find(|d| d.entry.name == "Second")
        .unwrap()
        .entry
        .id
        .clone();
    let _ = dir; // keep the temp dir alive for the duration of the test

    // The registered handler is the default database's, as the serve loop wires
    // it into the router; routing then dispatches per the header.
    let svc = manager
        .get_or_open(&default_id)
        .await
        .unwrap()
        .node_service_grpc
        .clone();

    // Create an ai-chat node targeting the second database.
    let mut create = Request::new(CreateNodeRequest {
        id: None,
        node_type: "ai-chat".into(),
        content: "in-second".into(),
        parent_id: None,
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        properties: serde_json::json!({ "ai-chat": { "messages": [] } }).to_string(),
        position: None,
    });
    create.extensions_mut().insert(manager.clone());
    create
        .metadata_mut()
        .insert(DATABASE_ID_HEADER, second_id.as_str().parse().unwrap());
    let node_id = svc.create_node(create).await.unwrap().into_inner().node_id;

    // Visible when the same second-database header is supplied.
    let mut get_second = Request::new(GetNodeRequest {
        node_id: node_id.clone(),
    });
    get_second.extensions_mut().insert(manager.clone());
    get_second
        .metadata_mut()
        .insert(DATABASE_ID_HEADER, second_id.as_str().parse().unwrap());
    assert!(svc.get_node(get_second).await.is_ok());

    // Invisible to the default database (no header) — no cross-database leak.
    let mut get_default = Request::new(GetNodeRequest { node_id });
    get_default.extensions_mut().insert(manager.clone());
    assert_eq!(
        svc.get_node(get_default).await.unwrap_err().code(),
        Code::NotFound,
        "ai-chat node created in one database must not leak into another"
    );
}

/// The registry publishes a change signal on every mutation.
///
/// The tray's Databases submenu subscribes to this; without it the menu shows the
/// registry as it was at daemon boot, so a database created, renamed or removed
/// afterwards reads wrong until the daemon restarts.
#[tokio::test]
async fn registry_mutations_emit_a_change_signal() {
    let (manager, _scheduler, dir) = manager_with_two_dbs().await;
    let mut changes = manager.subscribe_changes();
    // Consume anything emitted during setup so each assertion below observes only
    // the mutation it performs.
    changes.mark_unchanged();

    let created = manager
        .create("Third".into(), Some(dir.path().join("third.db")))
        .await
        .unwrap();
    assert!(changes.has_changed().unwrap(), "create must signal");
    changes.mark_unchanged();

    manager.rename(&created.id, "Renamed".into()).await.unwrap();
    assert!(changes.has_changed().unwrap(), "rename must signal");
    changes.mark_unchanged();

    manager.remove(&created.id).await.unwrap();
    assert!(changes.has_changed().unwrap(), "remove must signal");
}

/// Opening and evicting also signal — those change which database reads as open,
/// which is exactly what the menu's open marker reports.
#[tokio::test]
async fn open_and_evict_emit_a_change_signal() {
    let (manager, _scheduler, _dir) = manager_with_two_dbs().await;
    let second_id = manager
        .list()
        .await
        .databases
        .iter()
        .find(|d| d.entry.name == "Second")
        .unwrap()
        .entry
        .id
        .clone();

    let mut changes = manager.subscribe_changes();
    changes.mark_unchanged();

    // `create` already opened Second, so evict first — a `get_or_open` here would
    // be a cache hit and legitimately signal nothing.
    tokio::time::sleep(Duration::from_millis(10)).await;
    manager.evict_idle_databases(Duration::from_millis(1)).await;
    assert!(changes.has_changed().unwrap(), "eviction must signal");
    changes.mark_unchanged();

    manager.get_or_open(&second_id).await.unwrap();
    assert!(changes.has_changed().unwrap(), "reopen must signal");
}
