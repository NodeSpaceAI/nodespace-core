//! End-to-end gRPC test for the `DatabaseService` registry manager and
//! per-request database routing (ADR-053: "One Daemon, Multiple Local
//! Databases").
//!
//! Unlike the unit tests (which call handlers directly with a manager injected
//! into request extensions), this drives the *real* tonic transport: the
//! `DbManagerLayer` must inject the `DatabaseManager` into each request over the
//! wire, `DatabaseService.Create` must register a second database, and a
//! subsequent `NodeService` write carrying the `x-ns-database-id` header must
//! land in that second database — invisible to the default. This is the
//! user-visible path that makes multi-database routing reachable.

use std::sync::Arc;
use std::time::Duration;

use nodespace_agent::pty::PtySessionManager;
use nodespace_daemon::nodespace::DatabaseStatus as ProtoDatabaseStatus;
use nodespace_daemon::nodespace::{
    node_event::Event as NodeEventKind, CreateDatabaseRequest, CreateNodeRequest, GetNodeRequest,
    ListDatabasesRequest, WatchRequest,
};
use nodespace_daemon::{
    DatabaseManager, DatabaseServiceClient, DatabaseServiceImpl, DatabaseServiceServer,
    DbManagerLayer, NodeServiceClient, NodeServiceServer, SharedContext, DATABASE_ID_HEADER,
};
use nodespace_nlp_engine::EmbeddingService;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request, Streaming};

/// Read from a `WatchNodes` stream until a `Created` event for `node_id`
/// arrives (skipping any unrelated events), returning the whole envelope so the
/// caller can assert its `database_id`. Bounded by a per-message timeout.
async fn await_created(
    stream: &mut Streaming<nodespace_daemon::nodespace::NodeEvent>,
    node_id: &str,
) -> String {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), stream.message())
            .await
            .expect("timed out waiting for a watch event")
            .expect("watch stream error")
            .expect("watch stream closed");
        if let Some(NodeEventKind::Created(ref data)) = event.event {
            if data.id == node_id {
                return event.database_id;
            }
        }
    }
}

/// A model-less build context — with `has_model = false` no embedding wiring
/// runs, so the dropped watch sender is harmless (it is never read).
fn test_context() -> SharedContext {
    let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
    SharedContext {
        pty_manager: Arc::new(PtySessionManager::new()),
        model,
        has_model: false,
        model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        scheduler: Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
        subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
        local_agent: nodespace_daemon::SharedLocalAgent::new(
            nodespace_daemon::nodespace_dir()
                .expect("nodespace dir")
                .join("daemon.toml"),
        ),
    }
}

/// Attach the `x-ns-database-id` routing header to a request.
fn with_db_header<T>(msg: T, id: &str) -> Request<T> {
    let mut req = Request::new(msg);
    req.metadata_mut()
        .insert(DATABASE_ID_HEADER, MetadataValue::try_from(id).unwrap());
    req
}

/// Spin up an in-process daemon serving `DatabaseService` + `NodeService` behind
/// the `DbManagerLayer`, backed by a tempdir registry with one default database.
async fn spawn() -> (
    DatabaseServiceClient<Channel>,
    NodeServiceClient<Channel>,
    oneshot::Sender<()>,
    TempDir,
) {
    let tempdir = TempDir::new().unwrap();
    let registry_path = tempdir.path().join("databases.toml");
    let default_db = tempdir.path().join("default.db");

    let manager = Arc::new(
        DatabaseManager::load(registry_path, test_context())
            .await
            .unwrap(),
    );
    let default_id = manager
        .ensure_default_registered("Default".into(), default_db)
        .await
        .unwrap();
    // Open the default through the manager and register that bundle's NodeService
    // (exactly as the daemon boot path does) so the manager's cache is the served
    // handle — no double-open.
    let default_bundle = manager.get_or_open(&default_id).await.unwrap();
    let node_default = default_bundle.node_service_grpc.clone();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let mgr = manager.clone();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .layer(DbManagerLayer::new(mgr.clone()))
            .add_service(DatabaseServiceServer::new(DatabaseServiceImpl::new(mgr)))
            .add_service(NodeServiceServer::new(node_default))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    let endpoint = format!("http://{addr}");
    for _ in 0..50 {
        if let Ok(db) = DatabaseServiceClient::connect(endpoint.clone()).await {
            let node = NodeServiceClient::connect(endpoint.clone()).await.unwrap();
            return (db, node, shutdown_tx, tempdir);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("failed to connect to in-process daemon");
}

#[tokio::test]
async fn create_second_database_then_route_a_write_to_it() {
    let (mut db, mut node, shutdown, tempdir) = spawn().await;

    // The layer injects the manager over the wire → List is reachable and reports
    // just the default database.
    let listed = db.list(ListDatabasesRequest {}).await.unwrap().into_inner();
    assert_eq!(listed.databases.len(), 1);
    let default_id = listed.default_database_id.clone();
    assert!(!default_id.is_empty());

    // Register a second database via the service.
    let second = db
        .create(CreateDatabaseRequest {
            name: "Second".into(),
            path: Some(tempdir.path().join("second.db").display().to_string()),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!second.is_default);
    // Create makes the file and opens the database before registering it, so
    // the response already reports Open and the file exists on disk.
    assert_eq!(second.status, ProtoDatabaseStatus::Open as i32);
    assert!(tempdir.path().join("second.db").exists());
    let second_id = second.id.clone();

    // Write a node addressed to the second database via the routing header.
    let created = node
        .create_node(with_db_header(
            CreateNodeRequest {
                node_type: "text".into(),
                content: "only in the second database".into(),
                parent_id: None,
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            },
            &second_id,
        ))
        .await
        .unwrap()
        .into_inner();
    let node_id = created.node_id.clone();
    assert!(!node_id.is_empty());

    // Visible from the second database...
    let from_second = node
        .get_node(with_db_header(
            GetNodeRequest {
                node_id: node_id.clone(),
            },
            &second_id,
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        from_second.node_data.unwrap().content,
        "only in the second database"
    );

    // ...and invisible from the default database (header-less) — real isolation
    // over the transport, not just at the routing layer.
    let miss = node
        .get_node(GetNodeRequest {
            node_id: node_id.clone(),
        })
        .await
        .unwrap_err();
    assert_eq!(miss.code(), Code::NotFound);

    // The second database is still listed Open after serving the routed write.
    let listed = db.list(ListDatabasesRequest {}).await.unwrap().into_inner();
    assert_eq!(listed.databases.len(), 2);
    let second_listed = listed.databases.iter().find(|d| d.id == second_id).unwrap();
    assert_eq!(second_listed.status, ProtoDatabaseStatus::Open as i32);

    // A header naming an unregistered database is rejected over the wire, never
    // silently served from the default.
    let rejected = node
        .create_node(with_db_header(
            CreateNodeRequest {
                node_type: "text".into(),
                content: "nowhere".into(),
                parent_id: None,
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            },
            "ZZZ-NOT-REGISTERED",
        ))
        .await
        .unwrap_err();
    assert_eq!(rejected.code(), Code::NotFound);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn watch_events_are_stamped_with_the_serving_database_id() {
    let (mut db, node, shutdown, tempdir) = spawn().await;

    let default_id = db
        .list(ListDatabasesRequest {})
        .await
        .unwrap()
        .into_inner()
        .default_database_id;

    let second_id = db
        .create(CreateDatabaseRequest {
            name: "Second".into(),
            path: Some(tempdir.path().join("second.db").display().to_string()),
        })
        .await
        .unwrap()
        .into_inner()
        .id;

    // Open the watch streams BEFORE mutating so the events are observed. Each
    // stream is routed to its database by the header (the default stream omits
    // it). Separate client handles keep the streaming responses independent.
    let mut watch_default = node.clone();
    let mut default_stream = watch_default
        .watch_nodes(Request::new(WatchRequest {
            node_type: String::new(),
            root_id: String::new(),
        }))
        .await
        .unwrap()
        .into_inner();
    let mut watch_second = node.clone();
    let mut second_stream = watch_second
        .watch_nodes(with_db_header(
            WatchRequest {
                node_type: String::new(),
                root_id: String::new(),
            },
            &second_id,
        ))
        .await
        .unwrap()
        .into_inner();

    // A write routed to the second database → its watch event carries the second
    // database's id, not the default's.
    let mut writer = node.clone();
    let in_second = writer
        .create_node(with_db_header(
            CreateNodeRequest {
                node_type: "text".into(),
                content: "second-db node".into(),
                parent_id: None,
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            },
            &second_id,
        ))
        .await
        .unwrap()
        .into_inner()
        .node_id;
    assert_eq!(
        await_created(&mut second_stream, &in_second).await,
        second_id
    );

    // A header-less write → the default stream stamps the default database's id.
    let in_default = writer
        .create_node(Request::new(CreateNodeRequest {
            node_type: "text".into(),
            content: "default-db node".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        }))
        .await
        .unwrap()
        .into_inner()
        .node_id;
    assert_eq!(
        await_created(&mut default_stream, &in_default).await,
        default_id
    );

    let _ = shutdown.send(());
}
