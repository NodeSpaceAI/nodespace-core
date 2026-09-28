//! Per-database subtree access gate wiring (ADR-041 + ADR-053).
//!
//! The gate used to be injected once, into the boot database's `NodeService`. Under
//! per-database routing that left a hole: a request carrying `x-ns-database-id: <other>` is
//! dispatched to *that* database's service, which never received a gate and so kept the
//! always-allow default. These tests pin that every opened database gets a gate built for its
//! own id, and that a cascade delete routed to a non-default database consults it.

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use nodespace_agent::pty::PtySessionManager;
use nodespace_core::services::node_service::access_gate::{
    SubtreeAccessDecision, SubtreeAccessGate,
};
use nodespace_core::services::EmbeddingScheduler;
use nodespace_daemon::nodespace::node_service_server::NodeService as _;
use nodespace_daemon::nodespace::{CreateNodeRequest, DeleteNodeRequest};
use nodespace_daemon::{DatabaseManager, SharedContext, DATABASE_ID_HEADER};
use nodespace_nlp_engine::EmbeddingService;
use tempfile::TempDir;
use tokio::sync::watch;
use tonic::{Code, Request};

/// Denies every check, reporting a count derived from the database it was built for. The
/// count is the identity channel: it is how a test tells *which* database's gate answered,
/// which is the whole point — a shared gate instance would answer every database with the
/// first one's identity.
struct StampGate {
    stamp: u64,
}

#[async_trait]
impl SubtreeAccessGate for StampGate {
    async fn check_subtree_access(&self, _node_ids: &[String]) -> SubtreeAccessDecision {
        SubtreeAccessDecision::Denied {
            inaccessible_count: self.stamp,
        }
    }
}

/// Build context whose factory hands each database a gate stamped with a per-database number,
/// and records the ids it was asked to build for.
fn gated_context() -> (SharedContext, Arc<Mutex<Vec<String>>>) {
    let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_for_factory = seen.clone();

    let factory = OnceLock::new();
    factory
        .set(Arc::new(move |db_id: &str| {
            let mut ids = seen_for_factory.lock().unwrap();
            ids.push(db_id.to_string());
            // Stamp = 1-based order of first construction, so each database's gate is
            // distinguishable from every other's.
            let stamp = ids.len() as u64;
            Arc::new(StampGate { stamp }) as Arc<dyn SubtreeAccessGate>
        }) as nodespace_daemon::SubtreeGateFactory)
        .ok()
        .expect("factory set once");

    let context = SharedContext {
        pty_manager: Arc::new(PtySessionManager::new()),
        model,
        has_model: false,
        model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        scheduler: Arc::new(EmbeddingScheduler::new()),
        subtree_gate_factory: Arc::new(factory),
        local_agent: nodespace_daemon::SharedLocalAgent::new(
            nodespace_daemon::nodespace_dir()
                .expect("nodespace dir")
                .join("daemon.toml"),
        ),
    };
    (context, seen)
}

async fn manager_with_two_dbs() -> (Arc<DatabaseManager>, Arc<Mutex<Vec<String>>>, TempDir) {
    let dir = TempDir::new().unwrap();
    let (context, seen) = gated_context();
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
    (manager, seen, dir)
}

async fn id_of(manager: &DatabaseManager, name: &str) -> nodespace_daemon::services::DatabaseId {
    manager
        .list()
        .await
        .databases
        .iter()
        .find(|d| d.entry.name == name)
        .expect("database registered")
        .entry
        .id
        .clone()
}

/// Every database opened through the manager is handed a gate built for its OWN id.
#[tokio::test]
async fn each_opened_database_gets_a_gate_for_its_own_id() {
    let (manager, seen, _dir) = manager_with_two_dbs().await;
    let default_id = id_of(&manager, "Default").await;
    let second_id = id_of(&manager, "Second").await;

    manager.get_or_open(&default_id).await.unwrap();
    manager.get_or_open(&second_id).await.unwrap();

    let built = seen.lock().unwrap().clone();
    assert!(
        built.contains(&default_id.as_str().to_string())
            && built.contains(&second_id.as_str().to_string()),
        "expected a gate built for each database id, got {built:?}"
    );
}

/// A cascade delete routed to a NON-default database is refused by that database's gate —
/// the hole this wiring closes. Before it, the request reached a service still carrying the
/// always-allow default and the delete went through.
#[tokio::test]
async fn delete_in_non_default_database_consults_that_databases_gate() {
    let (manager, seen, _dir) = manager_with_two_dbs().await;
    let second_id = id_of(&manager, "Second").await;

    let services = manager.get_or_open(&second_id).await.unwrap();
    let svc = &services.node_service_grpc;

    let mut create = Request::new(CreateNodeRequest {
        content: "doomed".into(),
        node_type: "text".into(),
        ..Default::default()
    });
    create.extensions_mut().insert(manager.clone());
    create
        .metadata_mut()
        .insert(DATABASE_ID_HEADER, second_id.as_str().parse().unwrap());
    let node_id = svc.create_node(create).await.unwrap().into_inner().node_id;

    let mut delete = Request::new(DeleteNodeRequest {
        node_id,
        ..Default::default()
    });
    delete.extensions_mut().insert(manager.clone());
    delete
        .metadata_mut()
        .insert(DATABASE_ID_HEADER, second_id.as_str().parse().unwrap());
    let err = svc
        .delete_node(delete)
        .await
        .expect_err("the non-default database's gate must refuse this delete");

    assert_eq!(
        err.code(),
        Code::FailedPrecondition,
        "refusal should surface as FAILED_PRECONDITION, got {err:?}"
    );

    // The gate that answered was built for the SECOND database, not the default. Position in
    // `seen` is the stamp, so this also proves the instances are distinct rather than shared.
    let built = seen.lock().unwrap().clone();
    let expected_stamp = built
        .iter()
        .position(|id| id.as_str() == second_id.as_str())
        .unwrap()
        + 1;
    let reported = err
        .metadata()
        .get("x-subtree-inaccessible-count")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .expect("refusal carries the inaccessible count");
    assert_eq!(
        reported, expected_stamp as u64,
        "the answering gate must be the one built for {second_id}, not another database's"
    );
}
