//! End-to-end integration test for the `nodespace` CLI.
//!
//! Spins an in-process `nodespaced` gRPC server up against a tempdir-backed
//! SQLite database, then drives the CLI's command handlers (via the library
//! surface) at it. This validates that the CLI's gRPC plumbing — connection,
//! request construction, response unwrapping, error mapping — works end to
//! end against the same service stack the real binary uses.
//!
//! We exercise the handlers directly rather than spawning the compiled
//! binary so test failures point at the code path under test rather than
//! at fork/exec or stdout-capture plumbing.
//!
//! Two harnesses back the tests:
//! - [`spawn_test_daemon`] serves a single, plain `NodeService` — enough for the
//!   node/mention/schema/search flows that don't touch the database registry.
//! - [`spawn_routing_daemon`] serves the full multi-database stack (ADR-053):
//!   `DbManagerLayer` + `DatabaseService` + `NodeService` over a `DatabaseManager`
//!   seeded with one default database, so `database` subcommands and
//!   `--database` routing can be exercised over the real transport.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nodespace_agent::pty::PtySessionManager;
use nodespace_cli::{commands, connect, connect_database, DatabaseIdInterceptor, NodeClient};
use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
use nodespace_daemon::nodespace::{
    ConflictsForNodeRequest, CreateDatabaseRequest, CreateNodeRequest, GetConflictRequest,
    GetNodeRequest, GetRelatedNodesRequest, ListDatabasesRequest, NodeSortOrder,
    QueryNodesSimpleRequest,
};
use nodespace_daemon::{
    DatabaseManager, DatabaseServiceImpl, DatabaseServiceServer, DbManagerLayer, NodeServiceImpl,
    NodeServiceServer, SharedContext,
};
use nodespace_nlp_engine::EmbeddingService;
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::{oneshot, watch};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tonic::Code;

/// Spawn an in-process daemon over a temp-dir UDS and return the socket path.
async fn spawn_test_daemon() -> (PathBuf, oneshot::Sender<()>, TempDir) {
    let tempdir = TempDir::new().expect("failed to create tempdir");
    let sock_path = tempdir.path().join("test-daemon.sock");

    let mut store = Arc::new(
        SqliteStore::new(tempdir.path().join("daemon-db"))
            .await
            .expect("failed to open SqliteStore"),
    );
    let node_service = Arc::new(
        CoreNodeService::new(&mut store)
            .await
            .expect("failed to build NodeService"),
    );
    let service = NodeServiceImpl::new(
        node_service,
        Arc::new(tokio::sync::RwLock::new(None)),
        Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
    );

    let listener = UnixListener::bind(&sock_path).expect("failed to bind test UDS socket");
    let incoming = UnixListenerStream::new(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        Server::builder()
            .add_service(NodeServiceServer::new(service))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    for _ in 0..50 {
        if connect(&sock_path, DatabaseIdInterceptor::none())
            .await
            .is_ok()
        {
            return (sock_path, shutdown_tx, tempdir);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "daemon did not start accepting connections on {}",
        sock_path.display()
    );
}

/// A model-less build context — with `has_model = false` no embedding wiring
/// runs, so the dropped watch sender is harmless (it is never read).
fn routing_test_context() -> SharedContext {
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

/// Spawn an in-process daemon over a temp-dir UDS serving the full ADR-053
/// multi-database stack: the `DbManagerLayer` injects a `DatabaseManager` into
/// every request, `DatabaseService` manages the registry, and `NodeService`
/// routes by the `x-ns-database-id` header. The registry is seeded with a single
/// default database so header-less requests work and a second can be created.
async fn spawn_routing_daemon() -> (PathBuf, oneshot::Sender<()>, TempDir) {
    let tempdir = TempDir::new().expect("failed to create tempdir");
    let sock_path = tempdir.path().join("routing-daemon.sock");
    let registry_path = tempdir.path().join("databases.toml");
    let default_db = tempdir.path().join("default.db");

    let manager = Arc::new(
        DatabaseManager::load(registry_path, routing_test_context())
            .await
            .expect("failed to load DatabaseManager"),
    );
    let default_id = manager
        .ensure_default_registered("Default".into(), default_db)
        .await
        .expect("failed to register default database");
    // Open the default through the manager and serve that bundle's NodeService
    // (exactly as the daemon boot path does) so the manager's cache is the served
    // handle — no double-open.
    let default_bundle = manager
        .get_or_open(&default_id)
        .await
        .expect("failed to open default database");
    let node_default = default_bundle.node_service_grpc.clone();

    let listener = UnixListener::bind(&sock_path).expect("failed to bind routing UDS socket");
    let incoming = UnixListenerStream::new(listener);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let mgr = manager.clone();
    tokio::spawn(async move {
        Server::builder()
            .layer(DbManagerLayer::new(mgr.clone()))
            .add_service(DatabaseServiceServer::new(DatabaseServiceImpl::new(mgr)))
            .add_service(NodeServiceServer::new(node_default))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    for _ in 0..50 {
        if connect_database(&sock_path).await.is_ok() {
            return (sock_path, shutdown_tx, tempdir);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "routing daemon did not start accepting connections on {}",
        sock_path.display()
    );
}

/// Route a fresh `NodeService` client to `id` via the resolved routing header.
async fn node_client_for(sock: &std::path::Path, id: &str) -> nodespace_cli::NodeClient {
    let interceptor = DatabaseIdInterceptor::for_id(id).expect("build interceptor");
    connect(sock, interceptor)
        .await
        .expect("connect routed node")
}

#[tokio::test]
async fn create_get_update_children_delete_round_trip() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            node_type: "text".into(),
            content: "root via CLI".into(),
            parent: None,
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("create root");

    let mut raw_client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw client connect");

    let created = raw_client
        .create_node(nodespace_daemon::nodespace::CreateNodeRequest {
            node_type: "text".into(),
            content: "parent".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed parent")
        .into_inner();
    let parent_id = created.node_id;

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            node_type: "text".into(),
            content: "child via CLI".into(),
            parent: Some(parent_id.clone()),
            collections: vec![],
            collection_ids: vec![],
        }),
        false,
    )
    .await
    .expect("create child");

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Get(commands::node::GetArgs {
            id: parent_id.clone(),
        }),
        false,
    )
    .await
    .expect("get parent");

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            id: parent_id.clone(),
            content: Some("parent updated via CLI".into()),
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("update parent");

    let fetched = raw_client
        .get_node(GetNodeRequest {
            node_id: parent_id.clone(),
        })
        .await
        .expect("post-update fetch")
        .into_inner();
    assert_eq!(
        fetched.node_data.expect("node_data").content,
        "parent updated via CLI"
    );

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Children(commands::node::ChildrenArgs {
            id: parent_id.clone(),
        }),
        true,
    )
    .await
    .expect("list children");

    let children = raw_client
        .get_children(nodespace_daemon::nodespace::GetChildrenRequest {
            node_id: parent_id.clone(),
        })
        .await
        .expect("children fetch")
        .into_inner();
    assert_eq!(
        children.count, 1,
        "expected exactly one child seeded via CLI"
    );
    assert_eq!(children.nodes.len(), 1, "nodes len must match count");
    assert_eq!(children.nodes[0].content, "child via CLI");

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Delete(commands::node::DeleteArgs {
            id: parent_id.clone(),
        }),
        false,
    )
    .await
    .expect("delete parent");

    let err = raw_client
        .get_node(GetNodeRequest { node_id: parent_id })
        .await
        .expect_err("expected not_found after delete");
    assert_eq!(err.code(), Code::NotFound);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn get_missing_node_surfaces_not_found() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::node::run(
        &mut client,
        commands::node::NodeAction::Get(commands::node::GetArgs {
            id: "does-not-exist".into(),
        }),
        false,
    )
    .await
    .expect_err("expected error");

    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert_eq!(status.code(), Code::NotFound);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn search_without_embedding_service_reports_unavailable() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::search::run(
        &mut client,
        commands::search::SearchArgs {
            query: "anything".into(),
            node_types: vec![],
            collection: None,
            collection_id: None,
            filters: None,
            threshold: 0.0,
            limit: 0,
            include_content: false,
        },
        true,
    )
    .await
    .expect_err("expected unavailable");

    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert_eq!(status.code(), Code::Unavailable);

    let _ = shutdown.send(());
}

/// `nodespace skill guidance` is dispatched specially in `lib.rs::run` (the
/// one `skill` subcommand that opens a `NodeClient`, unlike
/// install/uninstall/status). This proves that wiring reaches the real
/// `NodeService.SearchNodes` RPC end to end -- same embedding-gated failure
/// mode as plain `search`, since `skill guidance` deliberately reuses that
/// RPC rather than adding a parallel code path. The success path (a real
/// seeded skill's guidance coming back with a provenance envelope) is
/// covered without an embedding model in `packages/agent`'s
/// `skill_guidance_fetch_mechanism` test, which reads the same seeded
/// `skill` nodes directly off `NodeService` instead of through this
/// semantic-search-gated RPC.
#[tokio::test]
async fn skill_guidance_without_embedding_service_reports_unavailable() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::skill::run_guidance(
        &mut client,
        commands::skill::GuidanceArgs {
            query: "write an ADR".into(),
            limit: 3,
        },
        true,
    )
    .await
    .expect_err("expected unavailable");

    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert_eq!(status.code(), Code::Unavailable);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn diagnostics_collect_reports_counts_and_recency() {
    let (sock, shutdown, _tempdir) = spawn_routing_daemon().await;
    let mut node = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect node");
    let mut seed = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect seed");
    let mut db = connect_database(&sock).await.expect("connect database");

    let baseline = commands::diagnostics::collect(&mut node, &mut db, None).await;
    assert!(
        baseline.errors.is_empty(),
        "baseline collect must not produce errors: {:?}",
        baseline.errors
    );
    assert!(
        baseline.total_node_count.is_some(),
        "a successful query must report a count, not unknown"
    );
    // The registry has exactly the seeded default, and it is the target when no
    // database is selected.
    assert_eq!(baseline.databases.len(), 1, "one registered database");
    assert!(baseline.databases[0].is_default);
    assert_eq!(baseline.targeted_database_id, baseline.databases[0].id);

    let root = seed
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "root".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed root")
        .into_inner();

    let mut last_child_id = String::new();
    for label in ["child-1", "child-2"] {
        tokio::time::sleep(Duration::from_millis(20)).await;
        last_child_id = seed
            .create_node(CreateNodeRequest {
                node_type: "text".into(),
                content: label.into(),
                parent_id: Some(root.node_id.clone()),
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            })
            .await
            .unwrap_or_else(|e| panic!("seed {label}: {e}"))
            .into_inner()
            .node_id;
    }

    let report = commands::diagnostics::collect(&mut node, &mut db, None).await;
    assert_eq!(
        report.total_node_count,
        baseline.total_node_count.map(|n| n + 3),
        "expected three additional nodes vs baseline"
    );
    assert_eq!(
        report.root_node_count,
        baseline.root_node_count.map(|n| n + 1),
        "expected one additional root node vs baseline"
    );
    assert!(
        report.database_size_bytes.unwrap_or(0) > 0,
        "targeted database file should have a nonzero size after writes"
    );
    assert_eq!(
        report
            .recent_node_ids
            .as_ref()
            .and_then(|ids| ids.first())
            .map(String::as_str),
        Some(last_child_id.as_str())
    );
    assert!(
        report.errors.is_empty(),
        "happy-path collect must not surface errors: {:?}",
        report.errors
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn connect_refused_returns_friendly_error() {
    let err = connect(
        std::path::Path::new("/tmp/nodespace-no-such-daemon.sock"),
        DatabaseIdInterceptor::none(),
    )
    .await
    .expect_err("expected refusal");

    let msg = format!("{}", err);
    assert!(
        msg.contains("Could not connect to nodespaced"),
        "expected friendly error, got: {msg}"
    );
    assert!(
        msg.contains("Is the daemon running?"),
        "expected remediation hint, got: {msg}"
    );
}

#[tokio::test]
async fn node_query_by_type() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    raw.create_node(CreateNodeRequest {
        node_type: "task".into(),
        content: "do the thing".into(),
        parent_id: None,
        properties: String::new(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed task");

    raw.create_node(CreateNodeRequest {
        node_type: "text".into(),
        content: "some text".into(),
        parent_id: None,
        properties: String::new(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed text");

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Query(commands::node::QueryArgs {
            id: None,
            mentioned_by: None,
            content_contains: None,
            title_contains: None,
            node_type: Some("task".into()),
            limit: 0,
            offset: 0,
        }),
        true,
    )
    .await
    .expect("query by type");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_export_markdown() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let root = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "# Root Document".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed root")
        .into_inner();

    raw.create_node(CreateNodeRequest {
        node_type: "text".into(),
        content: "Child paragraph".into(),
        parent_id: Some(root.node_id.clone()),
        properties: String::new(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed child");

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Export(commands::node::ExportArgs {
            id: root.node_id.clone(),
            children: true,
            max_depth: 0,
            node_ids: false,
        }),
        true,
    )
    .await
    .expect("export markdown");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_batch_get_and_update() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let a = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "node-a".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed a")
        .into_inner()
        .node_id;

    let b = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "node-b".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed b")
        .into_inner()
        .node_id;

    // batch-get: both found, one missing
    commands::node::run(
        &mut client,
        commands::node::NodeAction::BatchGet(commands::node::BatchGetArgs {
            ids: vec![a.clone(), b.clone(), "does-not-exist".into()],
        }),
        true,
    )
    .await
    .expect("batch-get");

    // batch-update (auto-version)
    let updates_json = serde_json::json!([
        {"node_id": a, "content": "node-a updated"},
        {"node_id": b, "content": "node-b updated"},
    ])
    .to_string();

    commands::node::run(
        &mut client,
        commands::node::NodeAction::BatchUpdate(commands::node::BatchUpdateArgs {
            updates: updates_json,
        }),
        true,
    )
    .await
    .expect("batch-update");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn mention_create_query_delete() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let source = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "source node".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed source")
        .into_inner()
        .node_id;

    let target = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "target node".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed target")
        .into_inner()
        .node_id;

    commands::mention::run(
        &mut client,
        commands::mention::MentionAction::Create(commands::mention::CreateMentionArgs {
            from: source.clone(),
            to: target.clone(),
        }),
        true,
    )
    .await
    .expect("create mention");

    commands::mention::run(
        &mut client,
        commands::mention::MentionAction::Outgoing(commands::mention::MentionQueryArgs {
            id: source.clone(),
        }),
        true,
    )
    .await
    .expect("outgoing mentions");

    commands::mention::run(
        &mut client,
        commands::mention::MentionAction::Incoming(commands::mention::MentionQueryArgs {
            id: target.clone(),
        }),
        true,
    )
    .await
    .expect("incoming mentions");

    commands::mention::run(
        &mut client,
        commands::mention::MentionAction::Delete(commands::mention::DeleteMentionArgs {
            from: source.clone(),
            to: target.clone(),
        }),
        true,
    )
    .await
    .expect("delete mention");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn schema_list_and_get() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    // The test daemon has no custom schemas; list should return an empty result without error.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::List(commands::schema::SchemaListArgs {}),
        true,
    )
    .await
    .expect("schema list");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn schema_create_and_update_round_trip() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Invoice",
                    "fields": [
                        {"name": "amount", "type": "number"}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema create");

    // Fetch the created schema back via the existing read path to confirm it landed.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Get(commands::schema::SchemaGetArgs {
            id: "invoice".into(),
        }),
        true,
    )
    .await
    .expect("schema get after create");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Update(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "schema_id": "invoice",
                    "add_fields": [
                        {"name": "currency", "type": "string"}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema update");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn schema_delete_requires_relationship_declarations_removed_first() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    // A self-referential declaration is enough to trip the guard, and needs
    // only one schema to set up.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Memo",
                    "fields": [{"name": "body", "type": "text"}],
                    "relationships": [{
                        "name": "supersedes",
                        "targetType": "memo",
                        "direction": "out",
                        "cardinality": "one",
                        "reverseName": "superseded_by",
                        "reverseCardinality": "one"
                    }]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema create");

    // The declaration blocks the delete, and the rejection names the fix.
    let err = commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Delete(commands::schema::SchemaDeleteArgs {
            id: "memo".into(),
        }),
        true,
    )
    .await
    .expect_err("delete must be refused while a declaration remains");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("schema_has_declarations"),
        "rejection should name the guard: {msg}"
    );
    assert!(
        msg.contains("update_schema"),
        "rejection should name the fix: {msg}"
    );
    // The guidance tells agents to act on the count, so the count has to be
    // in the message. A self-reference is one stored edge, not two, even
    // though it touches this schema at both ends.
    assert!(
        msg.contains("1 relationship declaration(s)"),
        "rejection should name how many declarations remain: {msg}"
    );

    // Clearing the declaration unblocks it.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Update(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "schema_id": "memo",
                    "remove_relationships": ["supersedes"]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema update removing the declaration");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Delete(commands::schema::SchemaDeleteArgs {
            id: "memo".into(),
        }),
        true,
    )
    .await
    .expect("schema delete after clearing declarations");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Get(commands::schema::SchemaGetArgs { id: "memo".into() }),
        true,
    )
    .await
    .expect_err("the deleted schema must no longer resolve");

    let _ = shutdown.send(());
}

/// Seed two `person` nodes sharing the same (case-insensitively) unique
/// `email`, which trips the create-path `detect_unique_field_collisions`
/// hook and journals an open `UniqueFieldCollision` record naming both. The
/// `person` type's `unique_case_insensitive` email field is a system-seeded
/// schema (`NodeService::new` seeds it), so no explicit `create_schema` call
/// is needed — mirrors `person_duplicate_convergence_test.rs`'s fixture.
///
/// `email_local_part` must be distinct per call within a test: the field is
/// case-insensitively unique across every active `person` node in the store,
/// so reusing a value across two calls would make the second pair collide
/// with the first pair's nodes too, not just with each other.
async fn seed_colliding_people(raw: &mut NodeClient, email_local_part: &str) -> (String, String) {
    let email = format!("{email_local_part}@example.com");
    let alice = raw
        .create_node(CreateNodeRequest {
            node_type: "person".into(),
            content: "Alice".into(),
            parent_id: None,
            properties: serde_json::json!({"person": {"email": email}}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed alice")
        .into_inner()
        .node_id;

    let bob = raw
        .create_node(CreateNodeRequest {
            node_type: "person".into(),
            content: "Bob".into(),
            parent_id: None,
            properties: serde_json::json!({"person": {"email": email.to_uppercase()}}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed bob (colliding email)")
        .into_inner()
        .node_id;

    (alice, bob)
}

#[tokio::test]
async fn conflicts_list_show_and_dismiss_round_trip() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let (alice, _bob) = seed_colliding_people(&mut raw, "list-show-dismiss").await;

    // list --node finds the journaled collision.
    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::List(commands::conflicts::ListArgs {
            status: None,
            kind: None,
            node: Some(alice.clone()),
            limit: None,
        }),
        true,
    )
    .await
    .expect("conflicts list --node");

    let conflicts = raw
        .conflicts_for_node(ConflictsForNodeRequest {
            node_id: alice.clone(),
        })
        .await
        .expect("raw conflicts_for_node")
        .into_inner()
        .conflicts;
    let open = conflicts
        .iter()
        .find(|c| c.kind == "unique_field_collision" && c.status == "open")
        .expect("the colliding email must have journaled a conflict");
    let conflict_id = open.id.clone();

    // show fetches the same record by its own id.
    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Show(commands::conflicts::ShowArgs {
            conflict_id: conflict_id.clone(),
        }),
        true,
    )
    .await
    .expect("conflicts show");

    // dismiss resolves it as Dismissed.
    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Dismiss(commands::conflicts::DismissArgs {
            conflict_id: conflict_id.clone(),
        }),
        true,
    )
    .await
    .expect("conflicts dismiss");

    let after = raw
        .get_conflict(GetConflictRequest {
            conflict_id: conflict_id.clone(),
        })
        .await
        .expect("raw get_conflict after dismiss")
        .into_inner()
        .conflict
        .expect("record must still exist");
    assert_eq!(after.status, "dismissed");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn conflicts_show_missing_id_surfaces_a_clear_error() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Show(commands::conflicts::ShowArgs {
            conflict_id: "does-not-exist".into(),
        }),
        false,
    )
    .await
    .expect_err("expected error for an unknown conflict id");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("does-not-exist"),
        "error should name the missing id: {msg}"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn conflicts_adopt_and_merge_round_trip() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    // adopt, on its own pair.
    let (alice, _bob) = seed_colliding_people(&mut raw, "adopt-pair").await;
    let conflict = raw
        .conflicts_for_node(ConflictsForNodeRequest {
            node_id: alice.clone(),
        })
        .await
        .expect("raw conflicts_for_node")
        .into_inner()
        .conflicts
        .into_iter()
        .find(|c| c.kind == "unique_field_collision" && c.status == "open")
        .expect("journaled collision");

    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Adopt(commands::conflicts::AdoptArgs {
            conflict_id: conflict.id.clone(),
            keep: alice.clone(),
        }),
        true,
    )
    .await
    .expect("conflicts adopt");

    let after = raw
        .get_conflict(GetConflictRequest {
            conflict_id: conflict.id,
        })
        .await
        .expect("raw get_conflict after adopt")
        .into_inner()
        .conflict
        .expect("record must still exist");
    assert_eq!(after.status, "resolved");

    // merge, on a fresh pair with --conflict-id inferring the loser.
    let (carol, dave) = seed_colliding_people(&mut raw, "merge-pair").await;
    let merge_conflict = raw
        .conflicts_for_node(ConflictsForNodeRequest {
            node_id: carol.clone(),
        })
        .await
        .expect("raw conflicts_for_node")
        .into_inner()
        .conflicts
        .into_iter()
        .find(|c| c.kind == "unique_field_collision" && c.status == "open")
        .expect("journaled collision");

    commands::conflicts::run(
        &mut client,
        commands::conflicts::ConflictsAction::Merge(commands::conflicts::MergeArgs {
            survivor: carol.clone(),
            loser: None,
            conflict_id: Some(merge_conflict.id.clone()),
        }),
        true,
    )
    .await
    .expect("conflicts merge (loser inferred from conflict_id)");

    let loser_after = raw
        .get_node(GetNodeRequest {
            node_id: dave.clone(),
        })
        .await
        .expect("raw get_node on merged-away loser")
        .into_inner()
        .node_data
        .expect("loser row must still exist, archived");
    assert_eq!(loser_after.lifecycle_status, "archived");

    let closed = raw
        .get_conflict(GetConflictRequest {
            conflict_id: merge_conflict.id,
        })
        .await
        .expect("raw get_conflict after merge")
        .into_inner()
        .conflict
        .expect("record must still exist");
    assert_eq!(closed.status, "resolved");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn schema_create_rejects_malformed_params() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some("not json".into()),
            params_file: None,
        }),
        true,
    )
    .await
    .expect_err("malformed params_json should error");
    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("params_json"),
        "expected status message to name the offending field, got: {}",
        status.message()
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn execute_query_filters_by_property() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    raw.create_node(CreateNodeRequest {
        node_type: "task".into(),
        content: "task one".into(),
        parent_id: None,
        properties: serde_json::json!({"status": "open"}).to_string(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed open task");

    raw.create_node(CreateNodeRequest {
        node_type: "task".into(),
        content: "task two".into(),
        parent_id: None,
        properties: serde_json::json!({"status": "done"}).to_string(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .expect("seed done task");

    commands::query::run(
        &mut client,
        commands::query::QueryArgs {
            target_type: "task".into(),
            filters: Some(
                serde_json::json!([
                    {"type": "property", "operator": "equals", "property": "status", "value": "open"}
                ])
                .to_string(),
            ),
            sorting: None,
            limit: 0,
        },
        true,
    )
    .await
    .expect("execute query");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn relationship_create_and_get() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    // Relationships must be schema-defined (find-then-edit guidance: the
    // relationship name must already exist on the source node's schema).
    // Create a "ticket" schema with a "blocks" -> "ticket" relationship, then
    // two ticket instances to relate.
    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Ticket",
                    "fields": [{"name": "title", "type": "string"}],
                    "relationships": [
                        {"name": "blocks", "targetType": "ticket", "direction": "out", "cardinality": "many", "reverseName": "blocked_by", "reverseCardinality": "many"}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("create ticket schema");

    let source = raw
        .create_node(CreateNodeRequest {
            node_type: "ticket".into(),
            content: "source".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed source")
        .into_inner()
        .node_id;

    let target = raw
        .create_node(CreateNodeRequest {
            node_type: "ticket".into(),
            content: "target".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed target")
        .into_inner()
        .node_id;

    commands::relationship::run(
        &mut client,
        commands::relationship::RelationshipAction::Create(commands::relationship::CreateArgs {
            from: source.clone(),
            relationship_name: "blocks".into(),
            to: target.clone(),
            edge_data: None,
        }),
        true,
    )
    .await
    .expect("create relationship");

    commands::relationship::run(
        &mut client,
        commands::relationship::RelationshipAction::Get(commands::relationship::GetArgs {
            id: source.clone(),
            relationship_name: "blocks".into(),
            direction: commands::relationship::Direction::Out,
        }),
        true,
    )
    .await
    .expect("get related nodes");

    let _ = shutdown.send(());
}

/// `relationship get` is a documented read path that does NOT go through
/// `output.rs`, so it needs its own guard: the daemon builds
/// `related_nodes_json` itself, and a plain `to_value(Node)` there would
/// serialize properties exactly as stored — namespaced under the schema id.
#[tokio::test]
async fn relationship_get_emits_flat_properties() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    commands::schema::run(
        &mut client,
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Ticket",
                    "fields": [{"name": "severity", "type": "string"}],
                    "relationships": [
                        {"name": "blocks", "targetType": "ticket", "direction": "out", "cardinality": "many", "reverseName": "blocked_by", "reverseCardinality": "many"}
                    ]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("create ticket schema");

    let make_ticket = |content: &str, severity: &str| {
        let mut raw = raw.clone();
        let req = CreateNodeRequest {
            node_type: "ticket".into(),
            content: content.into(),
            parent_id: None,
            properties: serde_json::json!({"severity": severity}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        };
        async move {
            raw.create_node(req)
                .await
                .expect("seed ticket")
                .into_inner()
                .node_id
        }
    };

    let source = make_ticket("source", "low").await;
    let target = make_ticket("target", "high").await;

    commands::relationship::run(
        &mut client,
        commands::relationship::RelationshipAction::Create(commands::relationship::CreateArgs {
            from: source.clone(),
            relationship_name: "blocks".into(),
            to: target.clone(),
            edge_data: None,
        }),
        true,
    )
    .await
    .expect("create relationship");

    let response = raw
        .get_related_nodes(GetRelatedNodesRequest {
            node_id: source.clone(),
            relationship_name: "blocks".into(),
            direction: "out".into(),
        })
        .await
        .expect("get related nodes")
        .into_inner();

    let related: serde_json::Value =
        serde_json::from_str(&response.related_nodes_json).expect("parse related_nodes_json");
    let first = &related[0];

    assert_eq!(
        first["properties"]["severity"], "high",
        "related nodes must carry flat properties like every other read path"
    );
    assert!(
        first["properties"].get("ticket").is_none(),
        "the schema id must not be observable in `relationship get` output"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn relationship_create_rejects_relationship_undefined_on_source_schema() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    // Plain "text" nodes have no schema-defined relationships at all, so
    // any non-built-in relationship name must be rejected (find-then-edit
    // guidance: the relationship must already exist on the source's schema).
    let source = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "source".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed source")
        .into_inner()
        .node_id;

    let target = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "target".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed target")
        .into_inner()
        .node_id;

    let err = commands::relationship::run(
        &mut client,
        commands::relationship::RelationshipAction::Create(commands::relationship::CreateArgs {
            from: source,
            relationship_name: "not_a_defined_relationship".into(),
            to: target,
            edge_data: None,
        }),
        true,
    )
    .await
    .expect_err("relationship not defined on schema should error");
    let status = err
        .chain()
        .find_map(|e| e.downcast_ref::<tonic::Status>())
        .expect("expected tonic::Status in error chain");
    assert!(
        status.message().contains("not_a_defined_relationship")
            || status.message().contains("not defined"),
        "expected error to name the undefined relationship, got: {}",
        status.message()
    );

    let _ = shutdown.send(());
}

// `nodespace relationship get --direction` is a clap ValueEnum (only "out"/"in"
// are constructible), so an invalid direction can no longer reach the CLI's
// GetArgs at all — clap itself rejects it before this test's code would run.
// The daemon-side validation this used to exercise is still real (any other
// gRPC client, not just this CLI, can send an arbitrary string on the wire),
// so this drives GetRelatedNodesRequest directly against the raw client.
#[tokio::test]
async fn get_related_nodes_rpc_rejects_invalid_direction() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let status = raw
        .get_related_nodes(GetRelatedNodesRequest {
            node_id: "some-id".into(),
            relationship_name: "blocks".into(),
            direction: "sideways".into(),
        })
        .await
        .expect_err("invalid direction should error");
    assert_eq!(status.code(), Code::InvalidArgument);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_update_sets_properties_and_preserves_content() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let id = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "original content".into(),
            parent_id: None,
            properties: serde_json::json!({"existing": "keep-me"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed node")
        .into_inner()
        .node_id;

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            id: id.clone(),
            content: None,
            properties: vec![("added".into(), serde_json::json!("value"))],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("update properties only");

    let node = raw
        .get_node(GetNodeRequest {
            node_id: id.clone(),
        })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node_data");
    assert_eq!(
        node.content, "original content",
        "content must be preserved when only properties are set"
    );
    let props: serde_json::Value =
        serde_json::from_str(&node.properties).expect("parse properties");
    // Typed properties are namespaced under the node's type key on the wire
    // (properties.<node_type>.<field>), per the typed-value shape produced by
    // crate::models::node_to_typed_value.
    assert_eq!(props["text"]["added"], "value");
    assert_eq!(
        props["text"]["existing"], "keep-me",
        "existing properties must be deep-merged, not replaced"
    );

    let _ = shutdown.send(());
}

/// The CLI must never expose the storage-layer property nesting.
///
/// A consumer that parsed `--json` and read `.properties.status` got `None`
/// against the nested storage shape and could report "status unknown" about
/// real data. These assert both halves of the contract at once: storage stays
/// namespaced under the type key, while what the CLI emits is flat.
#[tokio::test]
async fn cli_json_output_is_flat_while_storage_stays_nested() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    // --- a core type -----------------------------------------------------
    let task_id = raw
        .create_node(CreateNodeRequest {
            node_type: "task".into(),
            content: "Buy groceries".into(),
            parent_id: None,
            properties: serde_json::json!({"status": "open"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create task")
        .into_inner()
        .node_id;

    let task = raw
        .get_node(GetNodeRequest {
            node_id: task_id.clone(),
        })
        .await
        .expect("get task")
        .into_inner()
        .node_data
        .expect("node_data");

    // Storage is namespaced — unchanged by this fix.
    let stored: serde_json::Value =
        serde_json::from_str(&task.properties).expect("parse stored properties");
    assert_eq!(
        stored["task"]["status"], "open",
        "storage must stay nested under the type key"
    );

    // The CLI surface is flat.
    let emitted = nodespace_cli::output::node_to_json(&task);
    assert_eq!(
        emitted["properties"]["status"], "open",
        "`jq '.properties.status'` must work with no second parse"
    );
    assert!(
        emitted["properties"].get("task").is_none(),
        "the schema id must not be observable in CLI output"
    );
    assert!(
        emitted["properties"].get("_schema_version").is_none(),
        "`_`-prefixed internals must not reach a CLI consumer"
    );

    // --- a user-defined type ---------------------------------------------
    commands::schema::run(
        &mut raw.clone(),
        commands::schema::SchemaAction::Create(commands::schema::SchemaParamsArgs {
            params: Some(
                serde_json::json!({
                    "name": "Venue",
                    "fields": [{"name": "capacity", "type": "number"}]
                })
                .to_string(),
            ),
            params_file: None,
        }),
        true,
    )
    .await
    .expect("schema create");

    let venue_id = raw
        .create_node(CreateNodeRequest {
            node_type: "venue".into(),
            content: "Grand Hall".into(),
            parent_id: None,
            properties: serde_json::json!({"capacity": 250}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create venue")
        .into_inner()
        .node_id;

    let venue = raw
        .get_node(GetNodeRequest { node_id: venue_id })
        .await
        .expect("get venue")
        .into_inner()
        .node_data
        .expect("node_data");

    let emitted = nodespace_cli::output::node_to_json(&venue);
    assert_eq!(
        emitted["properties"]["capacity"], 250,
        "a user-defined type flattens by the same rule as a core type"
    );
    assert!(
        emitted["properties"].get("venue").is_none(),
        "the schema id must not be observable for user-defined types either"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_update_rejects_empty_args() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::node::run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            id: "irrelevant".into(),
            content: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect_err("update with no content and no properties should error");
    assert!(err.to_string().contains("--content"));

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_set_status_updates_status_property() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let id = raw
        .create_node(CreateNodeRequest {
            node_type: "task".into(),
            content: "a task".into(),
            parent_id: None,
            properties: serde_json::json!({"status": "open"}).to_string(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed task")
        .into_inner()
        .node_id;

    commands::node::run(
        &mut client,
        commands::node::NodeAction::SetStatus(commands::node::SetStatusArgs {
            id: id.clone(),
            status: "done".into(),
        }),
        true,
    )
    .await
    .expect("set status");

    let node = raw
        .get_node(GetNodeRequest {
            node_id: id.clone(),
        })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node_data");
    let props: serde_json::Value =
        serde_json::from_str(&node.properties).expect("parse properties");
    assert_eq!(props["task"]["status"], "done");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn node_set_status_rejects_invalid_status() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::node::run(
        &mut client,
        commands::node::NodeAction::SetStatus(commands::node::SetStatusArgs {
            id: "irrelevant".into(),
            status: "not-a-real-status".into(),
        }),
        true,
    )
    .await
    .expect_err("invalid status should error");
    assert!(err.to_string().contains("invalid status"));

    let _ = shutdown.send(());
}

#[tokio::test]
async fn database_registry_round_trip() {
    let (sock, shutdown, tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    // list: succeeds and reports the seeded default.
    commands::database::run(&mut db, commands::database::DatabaseAction::List, true)
        .await
        .expect("database list");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    assert_eq!(listed.databases.len(), 1);
    let default_id = listed.default_database_id.clone();
    assert!(!default_id.is_empty());

    // create: registers a second database.
    let second_path = tempdir.path().join("second.db").display().to_string();
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Create(commands::database::CreateArgs {
            name: "Second".into(),
            path: Some(second_path),
        }),
        true,
    )
    .await
    .expect("database create");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    assert_eq!(listed.databases.len(), 2);
    let second_id = listed
        .databases
        .iter()
        .find(|d| d.name == "Second")
        .expect("second registered")
        .id
        .clone();
    // Create makes the database file before registering it — it exists on disk
    // as soon as the command returns, without any request having routed to it.
    assert!(
        tempdir.path().join("second.db").exists(),
        "create must create the database file"
    );

    // Route a write to the second database, proving the freshly created
    // database serves requests immediately.
    let mut node_second = node_client_for(&sock, &second_id).await;
    node_second
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "open the second database".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("open second database");

    // rename: resolves by name and relabels.
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Rename(commands::database::RenameArgs {
            database: "Second".into(),
            new_name: "Renamed".into(),
        }),
        true,
    )
    .await
    .expect("database rename");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    let renamed = listed
        .databases
        .iter()
        .find(|d| d.id == second_id)
        .expect("still registered by id");
    assert_eq!(renamed.name, "Renamed");

    // use: sets the daemon-wide default to the renamed database.
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Use(commands::database::UseArgs {
            database: "Renamed".into(),
        }),
        true,
    )
    .await
    .expect("database use");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    assert_eq!(listed.default_database_id, second_id);

    // Put the default back so removing the (now non-default) original succeeds.
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Use(commands::database::UseArgs {
            database: default_id.clone(),
        }),
        true,
    )
    .await
    .expect("database use back to default");

    // remove: unregisters by id without deleting the file.
    let second_file = tempdir.path().join("second.db");
    commands::database::run(
        &mut db,
        commands::database::DatabaseAction::Remove(commands::database::RemoveArgs {
            database: second_id.clone(),
        }),
        true,
    )
    .await
    .expect("database remove");
    let listed = db
        .list(ListDatabasesRequest {})
        .await
        .expect("raw list")
        .into_inner();
    assert_eq!(listed.databases.len(), 1);
    assert!(
        listed.databases.iter().all(|d| d.id != second_id),
        "removed database must be gone from the registry"
    );
    assert!(
        second_file.exists(),
        "remove must not delete the underlying database file"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn select_database_by_name_resolves_to_id() {
    let (sock, shutdown, tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    let created = db
        .create(CreateDatabaseRequest {
            name: "Workspace".into(),
            path: Some(tempdir.path().join("workspace.db").display().to_string()),
        })
        .await
        .expect("create database")
        .into_inner();

    // A name resolves to the matching id...
    let resolved = commands::database::resolve_database_id_by_selection(&mut db, "Workspace")
        .await
        .expect("resolve by name");
    assert_eq!(resolved, created.id);

    // ...and the id resolves to itself.
    let resolved_by_id = commands::database::resolve_database_id_by_selection(&mut db, &created.id)
        .await
        .expect("resolve by id");
    assert_eq!(resolved_by_id, created.id);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn unregistered_database_selection_errors() {
    let (sock, shutdown, _tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    let err = commands::database::resolve_database_id_by_selection(&mut db, "no-such-database")
        .await
        .expect_err("unregistered selection must error");
    assert!(
        err.to_string().contains("no database named or with id"),
        "expected a clear not-registered error, got: {err}"
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn ambiguous_name_selection_errors_but_id_still_resolves() {
    // The daemon does not enforce unique names, so two databases can share a
    // name. Selecting by that name must fail with an ambiguity error; selecting
    // by id must still resolve (id match wins over any name match).
    let (sock, shutdown, tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    let first = db
        .create(CreateDatabaseRequest {
            name: "work".into(),
            path: Some(tempdir.path().join("work-a.db").display().to_string()),
        })
        .await
        .expect("create first work")
        .into_inner();
    db.create(CreateDatabaseRequest {
        name: "work".into(),
        path: Some(tempdir.path().join("work-b.db").display().to_string()),
    })
    .await
    .expect("create second work");

    let err = commands::database::resolve_database_id_by_selection(&mut db, "work")
        .await
        .expect_err("a name shared by two databases must be ambiguous");
    let msg = err.to_string();
    assert!(
        msg.contains("ambiguous"),
        "expected an ambiguity error, got: {msg}"
    );
    // Both colliding ids should be listed so the user can disambiguate.
    assert!(
        msg.contains(&first.id),
        "ambiguity error should list the colliding ids, got: {msg}"
    );

    // Selecting by id sidesteps the ambiguity.
    let resolved = commands::database::resolve_database_id_by_selection(&mut db, &first.id)
        .await
        .expect("id resolves unambiguously even with a duplicate name");
    assert_eq!(resolved, first.id);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn database_routing_isolates_writes() {
    let (sock, shutdown, tempdir) = spawn_routing_daemon().await;
    let mut db = connect_database(&sock).await.expect("connect database");

    // Register a second database.
    db.create(CreateDatabaseRequest {
        name: "Second".into(),
        path: Some(tempdir.path().join("second.db").display().to_string()),
    })
    .await
    .expect("create second")
    .into_inner();

    // Resolve its id by name and route a NodeService client to it.
    let second_id = commands::database::resolve_database_id_by_selection(&mut db, "Second")
        .await
        .expect("resolve second");
    let mut node_second = node_client_for(&sock, &second_id).await;

    // Create a node routed to the second database via the node command handler.
    commands::node::run(
        &mut node_second,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            node_type: "text".into(),
            content: "isolated-to-second".into(),
            parent: None,
            collections: vec![],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("create in second");

    let query = QueryNodesSimpleRequest {
        id: None,
        mentioned_by: None,
        content_contains: Some("isolated-to-second".into()),
        title_contains: None,
        node_type: None,
        limit: 0,
        offset: 0,
        order_by: NodeSortOrder::Unspecified as i32,
    };

    // Visible from the second database...
    let in_second = node_second
        .query_nodes_simple(query.clone())
        .await
        .expect("query second")
        .into_inner();
    assert_eq!(
        in_second.nodes.len(),
        1,
        "the write must be visible in the database it was routed to"
    );

    // ...and invisible from the default (header-less) database — no cross-database bleed.
    let mut node_default = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect default");
    let in_default = node_default
        .query_nodes_simple(query)
        .await
        .expect("query default")
        .into_inner();
    assert!(
        in_default.nodes.is_empty(),
        "the write must not leak into the default database"
    );

    let _ = shutdown.send(());
}

/// tonic's default decode limit is 4 MiB, which several list-shaped RPCs in
/// this contract can legitimately exceed on a real database: `QueryNodesSimple`
/// returns every matching node's full record in a single message. This seeds a
/// response comfortably past that default and asserts both halves of the
/// contract — a client left on the default limit fails with `OutOfRange`
/// (proving the payload really does cross the boundary, so the test can't go
/// vacuous), while the client the CLI actually builds reads it in full.
#[tokio::test]
async fn query_nodes_simple_handles_response_over_the_default_grpc_limit() {
    const DEFAULT_TONIC_DECODE_LIMIT: usize = 4 * 1024 * 1024;
    const NODE_COUNT: usize = 90;
    const CONTENT_BYTES: usize = 60_000;

    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut seed = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect seed");

    // 90 × 60 KB ≈ 5.1 MB of content alone — past 4 MiB even before proto
    // framing and the rest of each record.
    let query = QueryNodesSimpleRequest {
        id: None,
        mentioned_by: None,
        content_contains: None,
        title_contains: None,
        node_type: None,
        // Requesting far more than MAX_QUERY_NODES_SIMPLE_LIMIT is
        // deliberate: the server clamps the row count, not this test's
        // point (a large-CONTENT response still exceeds tonic's default
        // decode limit well under that row cap — see NODE_COUNT/CONTENT_BYTES
        // above).
        limit: 100_000,
        offset: 0,
        order_by: NodeSortOrder::Unspecified as i32,
    };

    // The daemon bootstraps its own nodes (schemas and friends), so count what
    // is already there rather than assuming an empty database.
    let baseline = seed
        .query_nodes_simple(query.clone())
        .await
        .expect("baseline query")
        .into_inner()
        .nodes
        .len();

    let filler = "x".repeat(CONTENT_BYTES);
    for _ in 0..NODE_COUNT {
        seed.create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: filler.clone(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed large node");
    }

    // A client on tonic's default decode limit cannot read this response.
    let mut default_limit_client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect default-limit")
        .max_decoding_message_size(DEFAULT_TONIC_DECODE_LIMIT);
    let status = default_limit_client
        .query_nodes_simple(query.clone())
        .await
        .expect_err("a >4 MiB response must exceed tonic's default decode limit");
    assert_eq!(
        status.code(),
        Code::OutOfRange,
        "expected the decode-limit failure this test exists to prevent, got: {status}"
    );

    // The client the CLI builds reads the same response in full.
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let response = client
        .query_nodes_simple(query)
        .await
        .expect("configured client must read a multi-megabyte response")
        .into_inner();
    assert_eq!(
        response.nodes.len(),
        baseline + NODE_COUNT,
        "every seeded node must come back"
    );

    let _ = shutdown.send(());
}

/// When the node query fails, diagnostics must report the count as unknown and
/// exit non-zero — never present a failed query as a zero-node database.
#[tokio::test]
async fn diagnostics_reports_unknown_counts_when_the_node_query_fails() {
    let (sock, shutdown, _tempdir) = spawn_routing_daemon().await;
    let mut seed = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect seed");
    let mut db = connect_database(&sock).await.expect("connect database");

    // Seed real nodes so a reported count of 0 would be provably wrong.
    for label in ["alpha", "beta", "gamma"] {
        seed.create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: label.into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .unwrap_or_else(|e| panic!("seed {label}: {e}"));
    }

    // Clamping the decode limit reproduces what an oversized response does to a
    // real client, without having to seed one here.
    let mut starved = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect starved")
        .max_decoding_message_size(1);

    let report = commands::diagnostics::collect(&mut starved, &mut db, None).await;

    assert!(
        report.total_node_count.is_none(),
        "a failed node query must report an unknown count, not a number: {:?}",
        report.total_node_count
    );
    assert!(
        report.recent_node_ids.is_none(),
        "recency is derived from the failed query and must be unknown too"
    );
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("QueryNodesSimple failed")),
        "the underlying failure must be surfaced: {:?}",
        report.errors
    );
    // The registry enumeration rides an unclamped client, so the report is
    // partial rather than empty — that is exactly the state that must not read
    // as a success.
    assert!(
        !report.databases.is_empty(),
        "registry enumeration should still succeed"
    );

    let err = commands::diagnostics::run(
        &mut starved,
        &mut db,
        None,
        commands::diagnostics::DiagnosticsArgs {},
        false,
    )
    .await
    .expect_err("diagnostics must exit non-zero when a query failed");
    assert!(
        err.to_string().contains("diagnostics incomplete"),
        "expected a loud failure, got: {err}"
    );

    let _ = shutdown.send(());
}

/// `node create --collection <path>` files the node in one call, auto-creating
/// every missing path segment, and is repeatable so a node can join several
/// collections without a follow-up write.
#[tokio::test]
async fn node_create_collection_paths_are_repeatable_and_auto_create() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            node_type: "text".into(),
            content: "collected via CLI".into(),
            parent: None,
            // Neither path exists yet: `docs:rust` is nested, so `docs` and
            // `rust` are both created and wired member_of.
            collections: vec!["docs:rust".into(), "reference".into()],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("create with collections");

    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let found = raw
        .query_nodes_simple(QueryNodesSimpleRequest {
            node_type: Some("text".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .expect("query")
        .into_inner();
    let node_id = found
        .nodes
        .iter()
        .find(|n| n.content == "collected via CLI")
        .expect("created node")
        .id
        .clone();

    let memberships = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert_eq!(
        memberships.collection_ids.len(),
        2,
        "one create call must produce both memberships: {:?}",
        memberships.collection_ids
    );

    // The nested path's intermediate segment exists as its own collection, so
    // `docs:rust` resolved as a hierarchy rather than as a flat label.
    let collections = raw
        .query_nodes_simple(QueryNodesSimpleRequest {
            node_type: Some("collection".into()),
            limit: 20,
            ..Default::default()
        })
        .await
        .expect("query collections")
        .into_inner();
    let names: Vec<&str> = collections
        .nodes
        .iter()
        .map(|n| n.content.as_str())
        .collect();
    for expected in ["docs", "rust", "reference"] {
        assert!(
            names.contains(&expected),
            "missing auto-created collection '{expected}' in {names:?}"
        );
    }

    let _ = shutdown.send(());
}

/// `node update --collection <path>` adds membership after the fact, and
/// `--remove-collection-id` takes it away again.
#[tokio::test]
async fn node_update_collection_adds_and_removes_membership() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let created = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "late joiner".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed node")
        .into_inner();
    let node_id = created.node_id;

    // --collection alone is enough to make an update meaningful: no --content
    // or --property is required.
    commands::node::run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            id: node_id.clone(),
            content: None,
            properties: vec![],
            collections: vec!["archive:2026".into()],
            collection_ids: vec![],
            remove_collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect("update with collection");

    let after_add = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert_eq!(after_add.collection_ids.len(), 1);
    let leaf_id = after_add.collection_ids[0].clone();

    commands::node::run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            id: node_id.clone(),
            content: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![leaf_id.clone()],
        }),
        true,
    )
    .await
    .expect("update removing collection");

    let after_remove = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest { node_id })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert!(
        after_remove.collection_ids.is_empty(),
        "membership should be gone: {:?}",
        after_remove.collection_ids
    );

    let _ = shutdown.send(());
}

/// Removal takes collection IDs, not paths — the asymmetry with
/// `add_to_collections` (which takes paths) is why the field is named
/// `remove_from_collection_ids`.
///
/// This pins the consequence, not just the name. Removal detaches an existing
/// `member_of` edge, so a path passed where an id belongs matches no edge and
/// `delete_relationship` reports success: the call returns Ok and the
/// membership survives. That is a silent data-integrity failure rather than an
/// error, so a rename back to a symmetric `remove_from_collections` — which
/// would make passing a path look correct — must fail here.
#[tokio::test]
async fn removing_by_path_instead_of_id_does_not_silently_drop_membership() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");
    let mut raw = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("raw connect");

    let created = raw
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "membership survives a bad removal".into(),
            parent_id: None,
            properties: String::new(),
            collections: vec!["ops:oncall".into()],
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("seed node")
        .into_inner();
    let node_id = created.node_id;

    let before = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert_eq!(before.collection_ids.len(), 1, "fixture must be a member");

    // Pass the PATH where an id belongs — the mistake the old symmetric field
    // name invited. It removes nothing.
    commands::node::run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            id: node_id.clone(),
            content: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec!["ops:oncall".into()],
        }),
        true,
    )
    .await
    .expect("the call itself succeeds — that is precisely the hazard");

    let after = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert_eq!(
        after.collection_ids, before.collection_ids,
        "a path is not an id: nothing should have been removed"
    );

    // The id form is what actually detaches the edge.
    let leaf_id = before.collection_ids[0].clone();
    commands::node::run(
        &mut client,
        commands::node::NodeAction::Update(commands::node::UpdateArgs {
            id: node_id.clone(),
            content: None,
            properties: vec![],
            collections: vec![],
            collection_ids: vec![],
            remove_collection_ids: vec![leaf_id],
        }),
        true,
    )
    .await
    .expect("removal by id");

    let removed = raw
        .get_node_collections(nodespace_daemon::nodespace::NodeCollectionsRequest { node_id })
        .await
        .expect("get_node_collections")
        .into_inner();
    assert!(
        removed.collection_ids.is_empty(),
        "removal by id must actually detach: {:?}",
        removed.collection_ids
    );

    let _ = shutdown.send(());
}

/// A collection that cannot be resolved must fail the command rather than
/// leaving the caller with a silently uncollected node.
#[tokio::test]
async fn node_create_unresolvable_collection_is_an_error() {
    let (sock, shutdown, _tempdir) = spawn_test_daemon().await;
    let mut client = connect(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let err = commands::node::run(
        &mut client,
        commands::node::NodeAction::Create(commands::node::CreateArgs {
            node_type: "text".into(),
            content: "should fail".into(),
            parent: None,
            // An empty path has no segments to resolve.
            collections: vec!["".into()],
            collection_ids: vec![],
        }),
        true,
    )
    .await
    .expect_err("an unresolvable collection path must surface as an error");
    assert!(
        err.to_string().contains("CreateNode RPC failed"),
        "unexpected error: {err}"
    );

    let _ = shutdown.send(());
}

/// `--collection` and `--collection-id` are mutually exclusive at the parser,
/// matching how `search` already handles the same pair.
#[test]
fn collection_path_and_id_flags_are_mutually_exclusive() {
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct CreateHarness {
        #[command(flatten)]
        args: commands::node::CreateArgs,
    }

    #[derive(Parser, Debug)]
    struct UpdateHarness {
        #[command(flatten)]
        args: commands::node::UpdateArgs,
    }

    let err = CreateHarness::try_parse_from([
        "create",
        "--type",
        "text",
        "--content",
        "x",
        "--collection",
        "docs",
        "--collection-id",
        "abc",
    ])
    .expect_err("create must reject both collection forms at once");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);

    let err = UpdateHarness::try_parse_from([
        "update",
        "node-1",
        "--collection",
        "docs",
        "--collection-id",
        "abc",
    ])
    .expect_err("update must reject both collection forms at once");
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);

    // Each form on its own parses, and both are repeatable.
    let ok = CreateHarness::try_parse_from([
        "create",
        "--type",
        "text",
        "--content",
        "x",
        "--collection",
        "docs:rust",
        "--collection",
        "reference",
    ])
    .expect("repeated --collection must parse");
    assert_eq!(ok.args.collections, vec!["docs:rust", "reference"]);
}
