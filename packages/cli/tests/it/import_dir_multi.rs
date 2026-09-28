//! Integration tests for `nodespace import dir` accepting several directories
//! in one call.
//!
//! Spins an in-process daemon with both `NodeService` and `ImportService`
//! registered over a temp-dir UDS, then drives `commands::import::run`
//! directly — the same library surface the compiled CLI binary calls —
//! against it. `commands::import::run` only prints; results are verified by
//! querying the underlying `NodeService`/`CollectionService` directly rather
//! than capturing stdout, matching the pattern in `cli_integration.rs`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nodespace_cli::{commands, connect_import, DatabaseIdInterceptor};
use nodespace_core::models::NodeFilter;
use nodespace_core::services::CollectionService;
use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
use nodespace_daemon::{
    ImportServiceImpl, ImportServiceServer, NodeServiceImpl, NodeServiceServer,
};
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;

/// Spin up an in-process daemon over a temp-dir UDS with both `NodeService`
/// and `ImportService` registered, returning a connected `ImportClient`'s
/// socket path, the underlying `CoreNodeService` (for out-of-band
/// verification), and a shutdown handle.
async fn spawn_import_test_daemon() -> (PathBuf, Arc<CoreNodeService>, oneshot::Sender<()>, TempDir)
{
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
    let node_svc = NodeServiceImpl::new(
        Arc::clone(&node_service),
        Arc::new(tokio::sync::RwLock::new(None)),
        Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
    );
    let import_svc = ImportServiceImpl::new(Arc::clone(&node_service));

    let listener = UnixListener::bind(&sock_path).expect("failed to bind test UDS socket");
    let incoming = UnixListenerStream::new(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        Server::builder()
            .add_service(NodeServiceServer::new(node_svc))
            .add_service(ImportServiceServer::new(import_svc))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    for _ in 0..50 {
        if connect_import(&sock_path, DatabaseIdInterceptor::none())
            .await
            .is_ok()
        {
            return (sock_path, node_service, shutdown_tx, tempdir);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "daemon did not start accepting connections on {}",
        sock_path.display()
    );
}

/// Write a markdown file at `dir/rel_path`, creating parent folders as
/// needed, and return its absolute path.
fn write_md(dir: &std::path::Path, rel_path: &str, content: &str) -> String {
    let path = dir.join(rel_path);
    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir -p");
    std::fs::write(&path, content).expect("write markdown file");
    path.to_str().expect("non-UTF-8 path").to_string()
}

fn dir_args(
    directories: Vec<String>,
    auto_collection_routing: bool,
) -> commands::import::ImportDirArgs {
    commands::import::ImportDirArgs {
        directories,
        collection: None,
        use_filename_as_title: false,
        auto_collection_routing,
        exclude_patterns: vec![],
        include_agent_files: false,
        include_hidden: false,
        no_recursive: false,
        replace: false,
    }
}

/// `import dir <a> <b>` imports files from both directories in one call.
#[tokio::test]
async fn import_dir_accepts_multiple_directories_in_one_call() {
    let (sock, node_service, shutdown, _tempdir) = spawn_import_test_daemon().await;
    let mut client = connect_import(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    write_md(dir_a.path(), "alpha.md", "# Alpha marker unique content");
    write_md(dir_b.path(), "beta.md", "# Beta marker unique content");

    commands::import::run(
        &mut client,
        commands::import::ImportAction::Dir(dir_args(
            vec![
                dir_a.path().to_str().unwrap().to_string(),
                dir_b.path().to_str().unwrap().to_string(),
            ],
            false,
        )),
        true,
    )
    .await
    .expect("import dir with two directories");

    let alpha = node_service
        .query_nodes(NodeFilter::new().with_content_contains("Alpha marker".to_string()))
        .await
        .expect("query alpha");
    let beta = node_service
        .query_nodes(NodeFilter::new().with_content_contains("Beta marker".to_string()))
        .await
        .expect("query beta");

    assert_eq!(
        alpha.len(),
        1,
        "expected the file from the first directory to be imported"
    );
    assert_eq!(
        beta.len(),
        1,
        "expected the file from the second directory to be imported"
    );

    let _ = shutdown.send(());
}

/// `--auto-collection-routing` with several directories routes each file
/// relative to *its own* directory's root, not a synthesised common
/// ancestor. Two directories each carry an identically-named subfolder
/// (`meeting-notes/`); both files must land in the SAME collection ("Meeting
/// Notes") rather than two different ones keyed by which temp directory
/// happened to hold them.
#[tokio::test]
async fn import_dir_multi_routes_auto_collection_relative_to_each_directorys_own_root() {
    let (sock, node_service, shutdown, _tempdir) = spawn_import_test_daemon().await;
    let mut client = connect_import(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    write_md(
        dir_a.path(),
        "meeting-notes/standup.md",
        "# Standup notes from team A",
    );
    write_md(
        dir_b.path(),
        "meeting-notes/retro.md",
        "# Retro notes from team B",
    );

    commands::import::run(
        &mut client,
        commands::import::ImportAction::Dir(dir_args(
            vec![
                dir_a.path().to_str().unwrap().to_string(),
                dir_b.path().to_str().unwrap().to_string(),
            ],
            true, // --auto-collection-routing
        )),
        true,
    )
    .await
    .expect("import dir with auto-collection-routing across two directories");

    let standup = node_service
        .query_nodes(NodeFilter::new().with_content_contains("Standup notes".to_string()))
        .await
        .expect("query standup")
        .into_iter()
        .next()
        .expect("standup root imported");
    let retro = node_service
        .query_nodes(NodeFilter::new().with_content_contains("Retro notes".to_string()))
        .await
        .expect("query retro")
        .into_iter()
        .next()
        .expect("retro root imported");

    let collection_service = CollectionService::new(node_service.store(), &node_service);
    // `get_node_collections` returns collection IDs, not names — resolve the
    // expected "Meeting Notes" collection's own id so both assertions below
    // check "member of THIS SAME collection node" rather than a name string
    // that could coincidentally match two distinct collections.
    let meeting_notes = collection_service
        .find_collection_by_path("Meeting Notes")
        .await
        .expect("resolve Meeting Notes collection")
        .expect("a \"Meeting Notes\" collection must exist after import");

    let standup_collections = collection_service
        .get_node_collections(&standup.id)
        .await
        .expect("standup collections");
    let retro_collections = collection_service
        .get_node_collections(&retro.id)
        .await
        .expect("retro collections");

    assert_eq!(
        standup_collections,
        vec![meeting_notes.id.clone()],
        "file under dir_a/meeting-notes/ must route relative to dir_a's own root"
    );
    assert_eq!(
        retro_collections,
        vec![meeting_notes.id.clone()],
        "file under dir_b/meeting-notes/ must route relative to dir_b's own root, \
         landing in the SAME collection as dir_a's file rather than a separate \
         one keyed by a synthesised common ancestor"
    );

    // A collection membership check alone can't distinguish "routed relative
    // to its own root" from "routed relative to the WRONG directory but the
    // path's final segment still happened to be 'meeting-notes'" — collection
    // names are globally unique, so a garbled multi-segment path ending in
    // "Meeting Notes" resolves to the same leaf node regardless of what
    // (possibly bogus) parent chain preceded it. What a wrong base_directory
    // DOES observably do is attach that bogus parent chain to the "Meeting
    // Notes" collection itself (collections form a DAG; a leaf can pick up
    // extra parents). So also assert "Meeting Notes" stays a top-level
    // collection with no parent of its own — which only holds if BOTH
    // directories' calls resolved a plain one-segment "Meeting Notes" path,
    // never a multi-segment one grown from stripping the wrong base.
    let meeting_notes_parents = collection_service
        .get_node_collections(&meeting_notes.id)
        .await
        .expect("meeting notes' own parent collections");
    assert!(
        meeting_notes_parents.is_empty(),
        "\"Meeting Notes\" must stay a top-level collection — a non-empty parent list means \
         one of the two directories' files was routed relative to the wrong base, producing \
         a multi-segment path (e.g. the whole absolute path) that still happened to END in \
         \"meeting-notes\" but nested the real collection under bogus parents: {meeting_notes_parents:?}"
    );

    let _ = shutdown.send(());
}

/// A directory that fails to scan (doesn't exist) does not stop the other,
/// valid directory's files from importing — but the overall call still
/// reports failure (a non-zero exit via the returned `Err`), so a script
/// checking the exit code notices that something in the invocation failed
/// rather than seeing a bare success.
#[tokio::test]
async fn import_dir_multi_directory_failure_does_not_abort_the_rest_but_the_call_still_errors() {
    let (sock, node_service, shutdown, _tempdir) = spawn_import_test_daemon().await;
    let mut client = connect_import(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let good_dir = TempDir::new().unwrap();
    write_md(good_dir.path(), "ok.md", "# Survives a sibling failure");
    let missing_dir = good_dir
        .path()
        .join("does-not-exist-at-all")
        .to_str()
        .unwrap()
        .to_string();

    let err = commands::import::run(
        &mut client,
        commands::import::ImportAction::Dir(dir_args(
            vec![missing_dir, good_dir.path().to_str().unwrap().to_string()],
            false,
        )),
        true,
    )
    .await
    .expect_err("a partial failure must be reported as an error, not swallowed as success");
    assert!(
        err.to_string().contains('1'),
        "expected the error to mention the one failed directory, got: {err}"
    );

    let survivors = node_service
        .query_nodes(
            NodeFilter::new().with_content_contains("Survives a sibling failure".to_string()),
        )
        .await
        .expect("query survivor");

    assert_eq!(
        survivors.len(),
        1,
        "the valid directory's file must still be imported despite the missing sibling"
    );

    let _ = shutdown.send(());
}

/// A single directory (the original one-directory form) keeps behaving
/// exactly as before: still imports normally through the same `run_dir` entry point.
#[tokio::test]
async fn import_dir_single_directory_still_works() {
    let (sock, node_service, shutdown, _tempdir) = spawn_import_test_daemon().await;
    let mut client = connect_import(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let dir = TempDir::new().unwrap();
    write_md(dir.path(), "solo.md", "# Solo directory import");

    commands::import::run(
        &mut client,
        commands::import::ImportAction::Dir(dir_args(
            vec![dir.path().to_str().unwrap().to_string()],
            false,
        )),
        true,
    )
    .await
    .expect("import dir with a single directory");

    let solo = node_service
        .query_nodes(NodeFilter::new().with_content_contains("Solo directory import".to_string()))
        .await
        .expect("query solo");

    assert_eq!(solo.len(), 1);

    let _ = shutdown.send(());
}

/// Two directories that each contain a same-named file at the same relative
/// position (both have a top-level `README.md`) compute the identical
/// deterministic root id server-side (derived from the path relative to
/// each call's own `base_directory`). Silently proceeding would either skip
/// the second file as "already imported" or, with `--replace`, overwrite
/// the first's content with the second's. The whole call must instead
/// refuse up front, before either RPC is even issued, so neither file is
/// written.
#[tokio::test]
async fn import_dir_multi_refuses_a_cross_directory_identity_collision() {
    let (sock, node_service, shutdown, _tempdir) = spawn_import_test_daemon().await;
    let mut client = connect_import(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    write_md(dir_a.path(), "README.md", "# Overview from team A");
    write_md(dir_b.path(), "README.md", "# Overview from team B");

    let err = commands::import::run(
        &mut client,
        commands::import::ImportAction::Dir(dir_args(
            vec![
                dir_a.path().to_str().unwrap().to_string(),
                dir_b.path().to_str().unwrap().to_string(),
            ],
            false,
        )),
        true,
    )
    .await
    .expect_err("a cross-directory identity collision must be refused, not silently imported");
    assert!(
        err.to_string().contains("README.md"),
        "expected the error to name the colliding file, got: {err}"
    );

    // Neither file was written -- the refusal happens before any RPC.
    let team_a = node_service
        .query_nodes(NodeFilter::new().with_content_contains("Overview from team A".to_string()))
        .await
        .expect("query team a");
    let team_b = node_service
        .query_nodes(NodeFilter::new().with_content_contains("Overview from team B".to_string()))
        .await
        .expect("query team b");
    assert_eq!(
        team_a.len(),
        0,
        "colliding import must not have written team A's file"
    );
    assert_eq!(
        team_b.len(),
        0,
        "colliding import must not have written team B's file"
    );

    let _ = shutdown.send(());
}

/// The identity-collision check only fires on an actual key collision — two
/// directories with differently-named files (even ones that happen to share
/// a subfolder name, like `notes/`) must import normally, matching the
/// existing auto-collection-routing test's fixture shape.
#[tokio::test]
async fn import_dir_multi_no_collision_when_relative_paths_differ() {
    let (sock, node_service, shutdown, _tempdir) = spawn_import_test_daemon().await;
    let mut client = connect_import(&sock, DatabaseIdInterceptor::none())
        .await
        .expect("connect");

    let dir_a = TempDir::new().unwrap();
    let dir_b = TempDir::new().unwrap();
    write_md(dir_a.path(), "notes/standup.md", "# No collision standup");
    write_md(dir_b.path(), "notes/retro.md", "# No collision retro");

    commands::import::run(
        &mut client,
        commands::import::ImportAction::Dir(dir_args(
            vec![
                dir_a.path().to_str().unwrap().to_string(),
                dir_b.path().to_str().unwrap().to_string(),
            ],
            false,
        )),
        true,
    )
    .await
    .expect("differently-named files under a shared subfolder name must not collide");

    let standup = node_service
        .query_nodes(NodeFilter::new().with_content_contains("No collision standup".to_string()))
        .await
        .expect("query standup");
    let retro = node_service
        .query_nodes(NodeFilter::new().with_content_contains("No collision retro".to_string()))
        .await
        .expect("query retro");
    assert_eq!(standup.len(), 1);
    assert_eq!(retro.len(), 1);

    let _ = shutdown.send(());
}
