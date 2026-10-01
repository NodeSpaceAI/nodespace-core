//! A default database that requires an unsupported extension (ADR-083 §2),
//! through the daemon's boot path and the real tonic transport.
//!
//! The daemon builds its router the way `nodespaced` does: it opens the default
//! through `open_default_or_record_refusal`, and on a required-extensions
//! refusal takes an unrouted service set from
//! `unrouted_services_if_default_refused` instead of stopping. The test then
//! checks, over the wire, that the default is refused with the shared status,
//! that the other databases and the process-global handlers are still served,
//! and that the default's file is untouched.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nodespace_agent::pty::PtySessionManager;
use nodespace_core::{NodeService, NodeUpdate, SqliteStore};
use nodespace_daemon::incompatible_database::open_default_or_record_refusal;
use nodespace_daemon::nodespace::{
    CreateDatabaseRequest, CreateNodeRequest, DatabaseStatus as ProtoDatabaseStatus,
    GetDaemonVersionRequest, GetNodeRequest, ListDatabasesRequest,
};
use nodespace_daemon::{
    build_base_router, unrouted_services_if_default_refused, BaseServices, DatabaseManager,
    DatabaseServiceClient, DatabaseServiceImpl, DbManagerLayer, NodeServiceClient,
    SettingsServiceImpl, SharedContext, DATABASE_ID_HEADER,
};
use nodespace_nlp_engine::EmbeddingService;
use nodespace_proto::{extension_names, requires_extension};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tonic::metadata::MetadataValue;
use tonic::{Code, Request};

fn test_context(home: &Path) -> SharedContext {
    let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
    SharedContext {
        pty_manager: Arc::new(PtySessionManager::new()),
        model,
        has_model: false,
        model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        scheduler: Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
        subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
        local_agent: nodespace_daemon::SharedLocalAgent::new(home.join("daemon.toml")),
    }
}

/// A closed database at `dir/name.sqlite`, in a directory of its own, whose
/// settings node requires `ids` — marked through the core library, the way an
/// extension marks one.
async fn database_requiring(dir: &Path, name: &str, ids: &[&str]) -> PathBuf {
    let db_dir = dir.join(name);
    std::fs::create_dir_all(&db_dir).unwrap();
    let path = db_dir.join(format!("{name}.sqlite"));
    {
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
                NodeUpdate::new()
                    .with_properties(serde_json::json!({ "required_extensions": ids })),
            )
            .await
            .unwrap();
    }
    assert!(
        !PathBuf::from(format!("{}-wal", path.display())).exists(),
        "the fixture closed cleanly"
    );
    path
}

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

fn with_db_header<T>(msg: T, id: &str) -> Request<T> {
    let mut req = Request::new(msg);
    req.metadata_mut()
        .insert(DATABASE_ID_HEADER, MetadataValue::try_from(id).unwrap());
    req
}

fn text_node(content: &str) -> CreateNodeRequest {
    CreateNodeRequest {
        node_type: "text".into(),
        content: content.into(),
        parent_id: None,
        properties: String::new(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    }
}

#[tokio::test]
async fn a_refused_default_leaves_the_daemon_serving_the_other_databases() {
    let tempdir = TempDir::new().unwrap();
    let home = tempdir.path();
    let context = test_context(home);
    let default_path = database_requiring(home, "default", &["pro"]).await;
    let default_dir = default_path.parent().unwrap().to_path_buf();
    let other_path = database_requiring(home, "other", &[]).await;
    let before = contents(&default_dir);

    // Boot, as `nodespaced` does.
    let manager = Arc::new(
        DatabaseManager::load(home.join("databases.toml"), context.clone())
            .await
            .unwrap(),
    );
    let default_id = manager
        .ensure_default_registered("Default".into(), default_path.clone())
        .await
        .unwrap();
    let other_id = manager.register(other_path).await.unwrap().id;
    let marker = home.join("incompatible-database.json");
    let open_err = open_default_or_record_refusal(&manager, &default_id, &default_path, &marker)
        .await
        .err()
        .expect("the default is refused");
    let bundle = unrouted_services_if_default_refused(open_err, &context)
        .await
        .expect("a required-extensions refusal does not stop the daemon");
    assert!(!marker.exists(), "the refusal writes no marker");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let base_services = BaseServices {
        node_service: bundle.node_service_grpc.clone(),
        agent_session: bundle.agent_session.clone(),
        import: bundle.import.clone(),
        settings: SettingsServiceImpl::new(home.join("daemon.toml")),
        local_agent: bundle.local_agent.clone(),
        embeddings: bundle.embeddings_service_grpc.clone(),
        database: DatabaseServiceImpl::new(manager.clone()),
    };
    let router = build_base_router(
        tonic::transport::Server::builder().layer(DbManagerLayer::new(manager.clone())),
        base_services,
    );
    let server = tokio::spawn(async move {
        router
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });
    let endpoint = format!("http://{addr}");
    let mut db = loop {
        match DatabaseServiceClient::connect(endpoint.clone()).await {
            Ok(client) => break client,
            Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    };
    let mut node = NodeServiceClient::connect(endpoint.clone()).await.unwrap();

    // The listing marks the default with the ids it requires.
    let listed = db.list(ListDatabasesRequest {}).await.unwrap().into_inner();
    let default_info = listed
        .databases
        .iter()
        .find(|d| d.id == default_id.as_str())
        .unwrap();
    assert_eq!(
        default_info.status,
        ProtoDatabaseStatus::RequiresExtension as i32
    );
    assert_eq!(default_info.unsupported_extensions, vec!["pro".to_string()]);

    // A request for the default, with or without its id, receives the refusal.
    let refused = node
        .get_node(GetNodeRequest {
            node_id: "anything".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Code::FailedPrecondition);
    assert_eq!(
        refused.message(),
        extension_names::refusal_message(&["pro"])
    );
    assert_eq!(
        requires_extension::unsupported_extensions(&refused),
        Some(vec!["pro".to_string()])
    );
    let refused_by_id = node
        .create_node(with_db_header(
            text_node("never written"),
            default_id.as_str(),
        ))
        .await
        .unwrap_err();
    assert_eq!(
        requires_extension::unsupported_extensions(&refused_by_id),
        Some(vec!["pro".to_string()])
    );

    // Another registered database is served.
    let created = node
        .create_node(with_db_header(
            text_node("in the other database"),
            other_id.as_str(),
        ))
        .await
        .unwrap()
        .into_inner();
    let fetched = node
        .get_node(with_db_header(
            GetNodeRequest {
                node_id: created.node_id,
            },
            other_id.as_str(),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(fetched.node_data.unwrap().content, "in the other database");

    // A new database can be created, and a handler that serves process-global
    // state answers from the unrouted set.
    let third = db
        .create(CreateDatabaseRequest {
            name: "Third".into(),
            path: Some(home.join("third.sqlite").display().to_string()),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(third.status, ProtoDatabaseStatus::Open as i32);
    let version = node
        .get_daemon_version(GetDaemonVersionRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(!version.version.is_empty());

    assert_eq!(
        contents(&default_dir),
        before,
        "the refused default is byte-identical, with nothing created or renamed beside it"
    );

    let _ = shutdown_tx.send(());
    server.await.unwrap();
    manager.shutdown_all().await;
}
