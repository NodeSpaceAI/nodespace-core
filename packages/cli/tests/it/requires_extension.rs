//! A database that requires an extension this build does not support
//! (ADR-083 §2), through the compiled `nodespace` binary.
//!
//! The binary is spawned against an in-process daemon built the way
//! `nodespaced` boots with a refused default, so the test sees exactly what a
//! user or an agent tool sees: the exit status, stdout and stderr.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use nodespace_agent::pty::PtySessionManager;
use nodespace_core::{Node, NodeService, NodeUpdate, SqliteStore};
use nodespace_daemon::incompatible_database::open_default_or_record_refusal;
use nodespace_daemon::{
    unrouted_services_if_default_refused, DatabaseManager, DatabaseServiceImpl,
    DatabaseServiceServer, DbManagerLayer, NodeServiceServer, SharedContext,
};
use nodespace_nlp_engine::EmbeddingService;
use nodespace_proto::extension_names;
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::process::Command;
use tokio::sync::{oneshot, watch};
use tokio_stream::wrappers::UnixListenerStream;
use tonic::service::Interceptor;
use tonic::transport::Server;

fn context() -> SharedContext {
    let (_tx, model) = watch::channel::<Option<Arc<EmbeddingService>>>(None);
    SharedContext {
        pty_manager: Arc::new(PtySessionManager::new()),
        model,
        has_model: false,
        model_load_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        scheduler: Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
        subtree_gate_factory: Arc::new(std::sync::OnceLock::new()),
        local_agent: nodespace_daemon::SharedLocalAgent::new(),
        extensions: nodespace_daemon::DaemonExtensions::none(),
    }
}

/// A closed database at `dir/name.sqlite` whose settings node requires `ids`
/// and that holds one text node, whose id is returned with the path.
async fn database_requiring(dir: &Path, name: &str, ids: &[&str]) -> (PathBuf, String) {
    let path = dir.join(format!("{name}.sqlite"));
    let mut store = Arc::new(SqliteStore::new(path.clone()).await.unwrap());
    let node_service = NodeService::new(&mut store).await.unwrap();
    let node_id = node_service
        .create_node(Node::new(
            "text".to_string(),
            format!("in {name}"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
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
    (path, node_id)
}

struct Daemon {
    home: TempDir,
    sock: PathBuf,
    other_node_id: String,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Boot a daemon whose default database requires `sync`, with a second
/// database `other` holding one node.
async fn spawn_daemon() -> Daemon {
    spawn_daemon_with(Ok::<_, tonic::Status>).await
}

/// [`spawn_daemon`], with `intercept` run before every node-service call.
async fn spawn_daemon_with(intercept: impl Interceptor + Clone + Send + 'static) -> Daemon {
    let home = TempDir::new().unwrap();
    let context = context();
    let (default_path, _) = database_requiring(home.path(), "marked", &["sync"]).await;
    let (other_path, other_node_id) = database_requiring(home.path(), "other", &[]).await;

    let manager = Arc::new(
        DatabaseManager::load(home.path().join("databases.toml"), context.clone())
            .await
            .unwrap(),
    );
    let default_id = manager
        .ensure_default_registered("marked".into(), default_path.clone())
        .await
        .unwrap();
    manager.register(other_path).await.unwrap();
    let marker = home.path().join("incompatible-database.json");
    let err = open_default_or_record_refusal(&manager, &default_id, &default_path, &marker)
        .await
        .err()
        .expect("the default is refused");
    let bundle = unrouted_services_if_default_refused(err, &context)
        .await
        .unwrap();

    let sock = home.path().join("daemon.sock");
    let incoming = UnixListenerStream::new(UnixListener::bind(&sock).unwrap());
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let node = bundle.node_service_grpc.clone();
    tokio::spawn(async move {
        Server::builder()
            .layer(DbManagerLayer::new(manager.clone()))
            .add_service(DatabaseServiceServer::new(DatabaseServiceImpl::new(
                manager,
            )))
            .add_service(NodeServiceServer::with_interceptor(node, intercept))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });
    for _ in 0..50 {
        if nodespace_cli::connect_database(&sock).await.is_ok() {
            return Daemon {
                home,
                sock,
                other_node_id,
                shutdown: Some(shutdown_tx),
            };
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("daemon did not start");
}

async fn nodespace(daemon: &Daemon, args: &[&str]) -> Output {
    let child = Command::new(env!("CARGO_BIN_EXE_nodespace"))
        .arg("--socket")
        .arg(&daemon.sock)
        .args(args)
        .env("NODESPACE_HOME", daemon.home.path())
        .env("HOME", daemon.home.path())
        .env_remove("NODESPACE_DATABASE")
        .env_remove("NODESPACED_SOCKET")
        .kill_on_drop(true)
        .output();
    tokio::time::timeout(Duration::from_secs(30), child)
        .await
        .expect("nodespace finished")
        .expect("nodespace ran")
}

/// The refusal as the CLI prints it, from the shared module.
fn refusal(ids: &[&str]) -> String {
    format!(
        "{}\n{}: {}",
        extension_names::refusal_message(ids),
        extension_names::DOWNLOAD_LABEL,
        extension_names::DOWNLOAD_URL
    )
}

#[tokio::test]
async fn a_command_routed_to_a_refused_database_exits_non_zero_with_the_refusal() {
    let daemon = spawn_daemon().await;

    for args in [
        vec!["node", "get", "anything"],
        vec!["--database", "marked", "node", "get", "anything"],
        vec!["--json", "search", "anything"],
        vec!["diagnostics"],
        vec!["--database", "marked", "diagnostics"],
        vec!["--json", "diagnostics"],
    ] {
        let out = nodespace(&daemon, &args).await;
        assert!(!out.status.success(), "{args:?} must fail");
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            format!("Error: {}\n", refusal(&["sync"])),
            "{args:?}"
        );
        assert!(out.stdout.is_empty(), "{args:?}");
    }

    // The other database is still served.
    let out = nodespace(
        &daemon,
        &["--database", "other", "node", "get", &daemon.other_node_id],
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[tokio::test]
async fn database_list_marks_the_refused_database() {
    let daemon = spawn_daemon().await;

    let out = nodespace(&daemon, &["database", "list"]).await;
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let marked = stdout
        .lines()
        .find(|line| line.contains("marked"))
        .expect("the refused database is listed");
    assert!(marked.contains("requires_extension"), "{marked}");
    assert!(
        marked.ends_with(&format!("  ({})", extension_names::requirement(&["sync"]))),
        "{marked}"
    );
    let other = stdout.lines().find(|line| line.contains("other")).unwrap();
    assert!(!other.contains('('), "{other}");

    let out = nodespace(&daemon, &["--json", "database", "list"]).await;
    assert!(out.status.success());
    let listed: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let marked = listed["databases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "marked")
        .unwrap();
    assert_eq!(marked["status"], "requires_extension");
    assert_eq!(
        marked["unsupported_extensions"],
        serde_json::json!(["sync"])
    );
    assert_eq!(
        marked["refusal"],
        serde_json::json!(extension_names::refusal_message(&["sync"]))
    );
    let other = listed["databases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "other")
        .unwrap();
    assert_eq!(other["refusal"], serde_json::Value::Null);
}

/// Fails every node-service call with a FAILED_PRECONDITION that is not the
/// required-extensions refusal.
#[derive(Clone)]
struct RefuseByRule;

impl Interceptor for RefuseByRule {
    fn call(&mut self, _: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        Err(tonic::Status::failed_precondition("a rule refused"))
    }
}

/// A failure other than the refusal, even another FAILED_PRECONDITION, leaves
/// diagnostics reporting as before: each failing query is listed and the run
/// exits non-zero, with no refusal text.
#[tokio::test]
async fn diagnostics_lists_each_failing_query_when_the_failure_is_not_the_refusal() {
    let daemon = spawn_daemon_with(RefuseByRule).await;

    let out = nodespace(&daemon, &["--database", "other", "diagnostics"]).await;
    assert!(!out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    for rpc in [
        "CountNodes",
        "CountRoots",
        "QueryNodesSimple",
        "GetAllSchemas",
        "GetDaemonMemory",
    ] {
        assert!(
            stdout.contains(&format!("  - {rpc} failed: ")),
            "{rpc} is listed: {stdout}"
        );
    }
    assert!(
        stdout.contains("  - CountNodes failed: FailedPrecondition: a rule refused\n"),
        "a failed query is reported by its code and message alone: {stdout}"
    );
    assert!(!stdout.contains("MetadataMap"), "{stdout}");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "Error: diagnostics incomplete: 5 query/IO failure(s) — see the Errors section above\n"
    );
}

/// A failed command prints the daemon's code and message under its own
/// context, without the status's metadata map or details.
#[tokio::test]
async fn a_failed_command_prints_the_status_without_its_metadata() {
    let daemon = spawn_daemon_with(RefuseByRule).await;

    let out = nodespace(&daemon, &["--database", "other", "node", "get", "anything"]).await;
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("FailedPrecondition: a rule refused"),
        "{stderr}"
    );
    assert!(!stderr.contains("MetadataMap"), "{stderr}");
    assert!(!stderr.contains("details:"), "{stderr}");
}

/// Diagnostics of a served database lists a refused one with what it needs,
/// as `database list` does.
#[tokio::test]
async fn diagnostics_says_what_a_refused_database_needs() {
    let daemon = spawn_daemon().await;

    let out = nodespace(&daemon, &["--database", "other", "diagnostics"]).await;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let marked = stdout
        .lines()
        .find(|line| line.contains("[requires_extension]"))
        .unwrap_or_else(|| panic!("the refused database is listed: {stdout}"));
    assert!(
        marked.ends_with(&format!("  ({})", extension_names::requirement(&["sync"]))),
        "{marked}"
    );
    let other = stdout
        .lines()
        .find(|line| line.contains("other ["))
        .unwrap();
    assert!(!other.contains('('), "{other}");

    let out = nodespace(&daemon, &["--json", "--database", "other", "diagnostics"]).await;
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let databases = report["databases"].as_array().unwrap();
    let marked = databases.iter().find(|d| d["name"] == "marked").unwrap();
    assert_eq!(
        marked["unsupported_extensions"],
        serde_json::json!(["sync"])
    );
    assert_eq!(
        marked["refusal"],
        serde_json::json!(extension_names::refusal_message(&["sync"]))
    );
    let other = databases.iter().find(|d| d["name"] == "other").unwrap();
    assert_eq!(other["unsupported_extensions"], serde_json::json!([]));
    assert_eq!(other["refusal"], serde_json::Value::Null);
}

/// Making a refused database the default succeeds, and warns on stderr that
/// requests routed to it are refused; any other database draws no warning.
#[tokio::test]
async fn database_use_warns_when_the_new_default_is_refused() {
    let daemon = spawn_daemon().await;

    let out = nodespace(&daemon, &["database", "use", "other"]).await;
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "{:?}", out.stderr);

    let out = nodespace(&daemon, &["--json", "database", "use", "marked"]).await;
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with(&format!(
            "Warning: {}.",
            extension_names::refusal_message(&["sync"])
        )),
        "{stderr}"
    );
    let info: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(info["status"], "requires_extension");
}
