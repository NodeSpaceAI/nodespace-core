//! End-to-end integration test for the `ImportService` gRPC handler.
//!
//! Spins up an in-process `nodespaced` with both `NodeService` and
//! `ImportService` registered, drives `ImportMarkdown` and
//! `ImportMarkdownFiles` via a real gRPC client, and verifies the streaming
//! progress protocol end to end.

use std::sync::Arc;
use std::time::Duration;

use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
use nodespace_daemon::nodespace::{
    ImportMarkdownFilesRequest, ImportMarkdownRequest, ImportOptions,
};
use nodespace_daemon::{
    ImportServiceClient, ImportServiceImpl, ImportServiceServer, NodeServiceImpl, NodeServiceServer,
};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;
use tonic::transport::Server;

/// Spin up an in-process daemon with both services registered and return a
/// connected `ImportServiceClient` plus a shutdown handle. Mirrors the pattern
/// in `grpc_round_trip.rs::spawn_test_daemon`.
async fn spawn_import_daemon() -> (
    ImportServiceClient<tonic::transport::Channel>,
    oneshot::Sender<()>,
    TempDir,
) {
    let tempdir = TempDir::new().expect("failed to create tempdir");

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

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

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

    let endpoint = format!("http://{}", addr);
    let mut last_err = None;
    for _ in 0..50 {
        match ImportServiceClient::connect(endpoint.clone()).await {
            Ok(client) => return (client, shutdown_tx, tempdir),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
    panic!("failed to connect to in-process daemon: {:?}", last_err);
}

/// Write a temporary markdown file and return its absolute path.
fn write_temp_md(dir: &TempDir, name: &str, content: &str) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, content).expect("failed to write temp markdown file");
    path.to_str().expect("non-UTF-8 path").to_string()
}

/// ImportMarkdown for a simple file streams progress events ending with step 9
/// carrying a successful `FileImportResult`.
#[tokio::test]
async fn import_markdown_single_file_streams_progress_and_succeeds() {
    let (mut client, shutdown, tempdir) = spawn_import_daemon().await;

    let md_path = write_temp_md(
        &tempdir,
        "hello.md",
        "# Hello\n\nThis is a test document.\n",
    );

    let mut stream = client
        .import_markdown(ImportMarkdownRequest {
            file_path: md_path.clone(),
            options: Some(ImportOptions {
                collection: String::new(),
                use_filename_as_title: true,
                auto_collection_routing: false,
                exclude_patterns: vec![],
                base_directory: String::new(),
                replace: false,
                include_agent_files: false,
                include_hidden: false,
                no_recursion: false,
            }),
        })
        .await
        .expect("import_markdown RPC failed")
        .into_inner();

    let mut steps_seen: Vec<u32> = Vec::new();
    let mut final_results = vec![];

    while let Some(event) = stream.next().await {
        let event = event.expect("stream error");
        steps_seen.push(event.step);
        if event.step == 9 {
            final_results = event.results;
        }
    }

    assert!(
        !steps_seen.is_empty(),
        "expected at least one progress event"
    );
    assert!(
        steps_seen.contains(&9),
        "expected a step-9 completion event, got steps: {:?}",
        steps_seen
    );
    assert_eq!(
        final_results.len(),
        1,
        "expected exactly one result for single-file import"
    );

    let result = &final_results[0];
    assert!(
        result.success,
        "expected success=true, got error: {:?}",
        result.error
    );
    assert_eq!(result.file_path, md_path);
    assert!(
        result.nodes_created > 0,
        "expected at least one node created"
    );

    let _ = shutdown.send(());
}

/// ImportMarkdownFiles for a batch of two files produces one step-9 event
/// with two results, both successful.
#[tokio::test]
async fn import_markdown_files_batch_reports_all_results() {
    let (mut client, shutdown, tempdir) = spawn_import_daemon().await;

    let path_a = write_temp_md(&tempdir, "alpha.md", "# Alpha\n\nFirst doc.\n");
    let path_b = write_temp_md(&tempdir, "beta.md", "# Beta\n\nSecond doc.\n");

    let mut stream = client
        .import_markdown_files(ImportMarkdownFilesRequest {
            file_paths: vec![path_a.clone(), path_b.clone()],
            options: Some(ImportOptions {
                collection: String::new(),
                use_filename_as_title: true,
                auto_collection_routing: false,
                exclude_patterns: vec![],
                base_directory: tempdir.path().to_str().unwrap().to_string(),
                replace: false,
                include_agent_files: false,
                include_hidden: false,
                no_recursion: false,
            }),
        })
        .await
        .expect("import_markdown_files RPC failed")
        .into_inner();

    let mut final_results = vec![];

    while let Some(event) = stream.next().await {
        let event = event.expect("stream error");
        if event.step == 9 {
            final_results = event.results;
        }
    }

    assert_eq!(
        final_results.len(),
        2,
        "expected two results for two-file batch import"
    );

    let paths: Vec<&str> = final_results.iter().map(|r| r.file_path.as_str()).collect();
    assert!(
        paths.contains(&path_a.as_str()),
        "missing result for alpha.md"
    );
    assert!(
        paths.contains(&path_b.as_str()),
        "missing result for beta.md"
    );

    for result in &final_results {
        assert!(
            result.success,
            "expected success=true for {}, got error: {:?}",
            result.file_path, result.error
        );
        assert!(
            result.nodes_created > 0,
            "expected nodes created for {}",
            result.file_path
        );
    }

    let _ = shutdown.send(());
}

/// ImportMarkdown for a non-existent file must produce a step-9 event with
/// `success=false` rather than leaving the stream open or crashing the server.
#[tokio::test]
async fn import_markdown_missing_file_surfaces_failure() {
    let (mut client, shutdown, tempdir) = spawn_import_daemon().await;

    let bad_path = tempdir
        .path()
        .join("does-not-exist.md")
        .to_str()
        .unwrap()
        .to_string();

    let mut stream = client
        .import_markdown(ImportMarkdownRequest {
            file_path: bad_path.clone(),
            options: Some(ImportOptions::default()),
        })
        .await
        .expect("import_markdown RPC itself should not fail (error is in the stream)")
        .into_inner();

    let mut final_results = vec![];

    while let Some(event) = stream.next().await {
        let event = event.expect("stream transport error");
        if event.step == 9 {
            final_results = event.results;
        }
    }

    assert_eq!(
        final_results.len(),
        1,
        "expected one result even on failure"
    );
    let result = &final_results[0];
    assert!(!result.success, "expected success=false for a missing file");
    assert!(
        !result.error.is_empty(),
        "expected a non-empty error message"
    );

    let _ = shutdown.send(());
}
