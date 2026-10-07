//! With no embedding model file, embedding-backed RPCs say so (non-retryable);
//! before the daemon has settled that, they still say the model is loading.

use std::sync::Arc;

use nodespace_core::services::EmbeddingScheduler;
use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
use nodespace_daemon::nodespace::node_service_server::NodeService;
use nodespace_daemon::nodespace::SearchRequest;
use nodespace_daemon::services::{build_shared_services, DaemonExtensions};
use nodespace_daemon::NodeServiceImpl;
use tempfile::TempDir;
use tokio::sync::RwLock;
use tonic::{Code, Request};

fn search() -> Request<SearchRequest> {
    Request::new(SearchRequest {
        query: "hello".into(),
        ..Default::default()
    })
}

#[tokio::test]
async fn search_distinguishes_missing_model_from_loading() {
    let tempdir = TempDir::new().unwrap();
    let mut store = Arc::new(SqliteStore::new(tempdir.path().join("db")).await.unwrap());
    let node_service = Arc::new(CoreNodeService::new(&mut store).await.unwrap());
    let service = NodeServiceImpl::new(
        node_service,
        Arc::new(RwLock::new(None)),
        Arc::new(EmbeddingScheduler::new()),
    );

    // The daemon has not yet looked for a model file: a load may be pending.
    let loading = service.search_nodes(search()).await.unwrap_err();
    assert_eq!(loading.code(), Code::Unavailable);
    assert!(loading.message().contains("loading, please retry"));

    // Nothing else in this process reads the variable (nextest runs each test
    // in its own process).
    std::env::set_var(
        "NODESPACED_MODEL_PATH",
        tempdir.path().join("no-model-here.gguf"),
    );
    let (shared, model_task) = build_shared_services(DaemonExtensions::none())
        .await
        .unwrap();
    assert!(model_task.is_none() && !shared.context.has_model);

    let missing = service.search_nodes(search()).await.unwrap_err();
    assert_eq!(missing.code(), Code::FailedPrecondition);
    assert!(missing.message().contains("no embedding model"));
    assert!(!missing.message().contains("retry"));
}

#[tokio::test]
async fn search_reports_a_failed_model_load_as_non_retryable() {
    let tempdir = TempDir::new().unwrap();
    let mut store = Arc::new(SqliteStore::new(tempdir.path().join("db")).await.unwrap());
    let node_service = Arc::new(CoreNodeService::new(&mut store).await.unwrap());
    let service = NodeServiceImpl::new(
        node_service,
        Arc::new(RwLock::new(None)),
        Arc::new(EmbeddingScheduler::new()),
    );

    // A file is there, but it is not a model, so the background load fails.
    let not_a_model = tempdir.path().join("not-a-model.gguf");
    std::fs::write(&not_a_model, b"this is not a model").unwrap();
    std::env::set_var("NODESPACED_MODEL_PATH", &not_a_model);
    let (shared, model_task) = build_shared_services(DaemonExtensions::none())
        .await
        .unwrap();
    assert!(shared.context.has_model);
    model_task
        .expect("a model file exists, so a load is spawned")
        .await
        .unwrap();

    let failed = service.search_nodes(search()).await.unwrap_err();
    assert_eq!(failed.code(), Code::FailedPrecondition);
    assert!(failed.message().contains("failed to load"));
    assert!(!failed.message().contains("retry"));
}
