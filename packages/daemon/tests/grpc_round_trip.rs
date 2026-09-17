//! End-to-end gRPC integration test for the `nodespaced` daemon.
//!
//! Spins the tonic server up in-process against a tempdir-backed SQLite database,
//! drives a `NodeServiceClient` against it, and verifies a CreateNode →
//! GetNode round trip plus a few error-mapping paths. This validates the
//! single acceptance criterion:
//!   > Integration test: start daemon, send GetNode via gRPC client,
//!   > verify response.

use std::sync::Arc;
use std::time::Duration;

use nodespace_core::services::{
    EmbeddingProcessor, EmbeddingScheduler, NodeAccessor, NodeEmbeddingService,
};
use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
use nodespace_daemon::nodespace::{
    create_node_request::Position as CreatePos, node_event::Event as NodeEventKind,
    reorder_node_request::Position as ReorderPos, CreateCollectionRequest, CreateNodeRequest,
    DeleteNodeRequest, Empty, GetChildrenRequest, GetNodeRequest, NodeCollectionsRequest,
    ReorderNodeRequest, SearchRequest, UpdateNodeRequest, WatchRequest,
};
use nodespace_daemon::services::embeddings_service::EmbeddingReady;
use nodespace_daemon::{NodeServiceClient, NodeServiceImpl, NodeServiceServer};
use nodespace_nlp_engine::{EmbeddingConfig as NlpConfig, EmbeddingService};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::sync::RwLock;
use tokio_stream::StreamExt;
use tonic::transport::Server;
use tonic::Code;

/// Start an in-process daemon and return a connected client plus a shutdown
/// handle. The server tears down when `shutdown` is sent — and the temp dir
/// is held alive on the returned tuple so it outlives all RPCs.
async fn spawn_test_daemon() -> (
    NodeServiceClient<tonic::transport::Channel>,
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
    let service = NodeServiceImpl::new(
        node_service,
        Arc::new(tokio::sync::RwLock::new(None)),
        Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
    );

    // Bind to an ephemeral port so parallel test runs don't collide on 50051.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

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

    // Give the server a brief moment to start accepting before we dial it.
    // Connect with retries to remove timing flakiness on slow CI runners
    // (50 * 25ms = 1.25s budget — comfortable for heavily loaded shared CI).
    let endpoint = format!("http://{}", addr);
    let mut last_err = None;
    for _ in 0..50 {
        match NodeServiceClient::connect(endpoint.clone()).await {
            Ok(client) => return (client, shutdown_tx, tempdir),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
    panic!("failed to connect to in-process daemon: {:?}", last_err);
}

/// Like `spawn_test_daemon`, but wires a real (uninitialized — no model file
/// loaded, matching `create_test_nlp_engine` in
/// `packages/core/tests/embedding_service_test.rs`) `NodeEmbeddingService` so
/// `search_nodes` doesn't short-circuit on `Status::unavailable` before
/// reaching `search_ops::search_semantic`. Also returns the underlying
/// `CoreNodeService` handle so tests can create nodes out-of-band (i.e.
/// without going through any search/agent code path) and assert search finds
/// them, per #1940's acceptance criteria.
async fn spawn_test_daemon_with_embeddings() -> (
    NodeServiceClient<tonic::transport::Channel>,
    Arc<CoreNodeService>,
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

    let nlp = Arc::new(EmbeddingService::new(NlpConfig::default()).unwrap());
    let node_accessor: Arc<dyn NodeAccessor> = Arc::new((*node_service).clone());
    let behaviors = node_service.behaviors().clone();
    let embedding_service = Arc::new(NodeEmbeddingService::new(
        nlp,
        store.clone(),
        node_accessor,
        behaviors,
    ));
    let scheduler = Arc::new(EmbeddingScheduler::new());
    let processor = Arc::new(
        EmbeddingProcessor::new(embedding_service.clone(), scheduler.clone(), String::new())
            .expect("failed to init EmbeddingProcessor"),
    );
    let embedding_state = Arc::new(RwLock::new(Some(EmbeddingReady {
        embedding_service,
        processor,
    })));

    let service = NodeServiceImpl::new(node_service.clone(), embedding_state, scheduler);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

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

    let endpoint = format!("http://{}", addr);
    let mut last_err = None;
    for _ in 0..50 {
        match NodeServiceClient::connect(endpoint.clone()).await {
            Ok(client) => return (client, node_service, shutdown_tx, tempdir),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
    panic!("failed to connect to in-process daemon: {:?}", last_err);
}

#[tokio::test]
async fn create_then_get_round_trip() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let created = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "hello from grpc".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create_node failed")
        .into_inner();

    assert!(!created.node_id.is_empty(), "expected a node id");
    assert_eq!(created.node_type, "text");
    let created_data = created.node_data.expect("missing node_data");
    assert_eq!(created_data.content, "hello from grpc");
    assert_eq!(created_data.lifecycle_status, "active");
    assert_eq!(created_data.version, 1);

    let fetched = client
        .get_node(GetNodeRequest {
            node_id: created.node_id.clone(),
        })
        .await
        .expect("get_node failed")
        .into_inner();

    assert_eq!(fetched.node_id, created.node_id);
    let fetched_data = fetched.node_data.expect("missing node_data");
    assert_eq!(fetched_data.id, created.node_id);
    assert_eq!(fetched_data.content, "hello from grpc");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn update_increments_version() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let created = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "v1".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create_node failed")
        .into_inner();

    let updated = client
        .update_node(UpdateNodeRequest {
            node_id: created.node_id.clone(),
            version: None, // exercise auto-fetch path
            node_type: None,
            content: Some("v2".into()),
            properties: None,
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        })
        .await
        .expect("update_node failed")
        .into_inner();

    let data = updated.node_data.expect("missing node_data");
    assert_eq!(data.content, "v2");
    assert!(
        data.version >= 2,
        "expected version bump, got {}",
        data.version
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn get_children_returns_parent_subtree() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let parent = client
        .create_node(CreateNodeRequest {
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
        .expect("create parent")
        .into_inner();

    for label in ["child-a", "child-b"] {
        client
            .create_node(CreateNodeRequest {
                node_type: "text".into(),
                content: label.into(),
                parent_id: Some(parent.node_id.clone()),
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            })
            .await
            .expect("create child");
    }

    let children = client
        .get_children(GetChildrenRequest {
            node_id: parent.node_id.clone(),
        })
        .await
        .expect("get_children failed")
        .into_inner();

    assert_eq!(children.count, 2);
    let contents: Vec<&str> = children.nodes.iter().map(|n| n.content.as_str()).collect();
    assert!(contents.contains(&"child-a"));
    assert!(contents.contains(&"child-b"));

    let _ = shutdown.send(());
}

#[tokio::test]
async fn get_node_missing_returns_not_found() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let err = client
        .get_node(GetNodeRequest {
            node_id: "does-not-exist".into(),
        })
        .await
        .expect_err("expected not_found");

    assert_eq!(err.code(), Code::NotFound);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn delete_node_marks_existed() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let created = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "doomed".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create_node failed")
        .into_inner();

    let deleted = client
        .delete_node(DeleteNodeRequest {
            node_id: created.node_id.clone(),
            version: None,
        })
        .await
        .expect("delete_node failed")
        .into_inner();

    assert_eq!(deleted.node_id, created.node_id);
    assert!(deleted.existed);

    // Subsequent get should now report NotFound.
    let err = client
        .get_node(GetNodeRequest {
            node_id: created.node_id,
        })
        .await
        .expect_err("expected not_found after delete");
    assert_eq!(err.code(), Code::NotFound);

    let _ = shutdown.send(());
}

/// Locks in the graceful-disable contract: when the daemon starts without an
/// `NodeEmbeddingService`, semantic search must report `Unavailable` rather
/// than crashing or returning empty results. Catches future regressions where
/// someone silently swaps the `Option<Arc<NodeEmbeddingService>>` to a panic
/// or a default-empty implementation.
#[tokio::test]
async fn search_nodes_returns_unavailable_without_embedding_service() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let err = client
        .search_nodes(SearchRequest {
            query: "anything".into(),
            ..SearchRequest::default()
        })
        .await
        .expect_err("expected unavailable");

    assert_eq!(err.code(), Code::Unavailable);

    let _ = shutdown.send(());
}

/// #1940 regression, CLI/gRPC path: `nodespace search "*" --type invoice`
/// must enumerate every pre-existing instance of the type, not silently
/// return `count: 0`. Creates the node directly through `NodeService`
/// (out-of-band — not via any search/agent code path) per the issue's
/// acceptance criteria, matching the bug's own repro
/// (`nodespace search "*" --type invoice --json` → `count: 0` although two
/// invoices existed).
#[tokio::test]
async fn search_nodes_wildcard_query_enumerates_out_of_band_node() {
    let (mut client, node_service, shutdown, _tempdir) = spawn_test_daemon_with_embeddings().await;

    let node = nodespace_core::models::Node::new(
        "invoice".to_string(),
        "Some Invoice".to_string(),
        serde_json::json!({ "invoice_number": "AA111", "status": "paid" }),
    );
    let node_id = node.id.clone();
    node_service
        .create_node(node)
        .await
        .expect("out-of-band create_node failed");

    let response = client
        .search_nodes(SearchRequest {
            query: "*".into(),
            node_types: vec!["invoice".into()],
            semantic: true,
            ..SearchRequest::default()
        })
        .await
        .expect("search_nodes failed")
        .into_inner();

    assert_eq!(
        response.count, 1,
        "expected the out-of-band invoice to be found"
    );
    assert_eq!(response.nodes[0].id, node_id);

    let _ = shutdown.send(());
}

/// Empty query must behave identically to "*" on the CLI/gRPC path — no
/// `InvalidArgument`, and both enumerate the same result set.
#[tokio::test]
async fn search_nodes_empty_query_matches_wildcard_query() {
    let (mut client, node_service, shutdown, _tempdir) = spawn_test_daemon_with_embeddings().await;

    node_service
        .create_node(nodespace_core::models::Node::new(
            "invoice".to_string(),
            "Some Invoice".to_string(),
            serde_json::json!({}),
        ))
        .await
        .expect("out-of-band create_node failed");

    let wildcard = client
        .search_nodes(SearchRequest {
            query: "*".into(),
            node_types: vec!["invoice".into()],
            semantic: true,
            ..SearchRequest::default()
        })
        .await
        .expect("search_nodes(\"*\") failed")
        .into_inner();

    let empty = client
        .search_nodes(SearchRequest {
            query: "".into(),
            node_types: vec!["invoice".into()],
            semantic: true,
            ..SearchRequest::default()
        })
        .await
        .expect("search_nodes(\"\") must no longer be InvalidArgument")
        .into_inner();

    assert_eq!(wildcard.count, empty.count);
    assert_eq!(wildcard.count, 1);

    let _ = shutdown.send(());
}

/// A search result can opt into carrying enough content to answer
/// from — the top-ranked results attach their subtree markdown (root plus
/// descendants), rather than forcing a follow-up fetch per hit to reach a
/// document's body. Exercises the enumerate ("*") path since it needs no
/// loaded embedding model; the markdown-attachment code downstream of
/// ranking is identical for a real semantic search.
#[tokio::test]
async fn search_nodes_include_markdown_attaches_subtree_content() {
    let (mut client, node_service, shutdown, _tempdir) = spawn_test_daemon_with_embeddings().await;

    let root_id = node_service
        .create_node(nodespace_core::models::Node::new(
            "text".to_string(),
            "# Team Onboarding".to_string(),
            serde_json::json!({}),
        ))
        .await
        .expect("out-of-band create root failed");

    node_service
        .create_node_with_parent(nodespace_core::services::CreateNodeParams {
            id: None,
            node_type: "text".to_string(),
            content: "New hires start with the setup guide.".to_string(),
            parent_id: Some(root_id.clone()),
            position: nodespace_core::services::InsertPositionOwned::End,
            properties: serde_json::json!({}),
            lifecycle_status: None,
        })
        .await
        .expect("out-of-band create child failed");

    // Default (include_markdown unset -> 0): no markdown attached, matching
    // prior behavior — a plain search stays cheap.
    let bare = client
        .search_nodes(SearchRequest {
            query: "*".into(),
            node_types: vec!["text".into()],
            semantic: true,
            ..SearchRequest::default()
        })
        .await
        .expect("search_nodes failed")
        .into_inner();
    let bare_root = bare
        .nodes
        .iter()
        .find(|n| n.id == root_id)
        .expect("root present in bare results");
    assert!(
        bare_root.markdown.is_empty(),
        "markdown must stay empty unless include_markdown is requested"
    );
    assert_eq!(bare_root.content, "# Team Onboarding");

    // Opt in: the root's markdown now carries the child's content too, so
    // the result answers on its own without a follow-up GetChildren call.
    let expanded = client
        .search_nodes(SearchRequest {
            query: "*".into(),
            node_types: vec!["text".into()],
            semantic: true,
            include_markdown: 5,
            ..SearchRequest::default()
        })
        .await
        .expect("search_nodes with include_markdown failed")
        .into_inner();
    let expanded_root = expanded
        .nodes
        .iter()
        .find(|n| n.id == root_id)
        .expect("root present in expanded results");
    assert!(
        expanded_root
            .markdown
            .contains("New hires start with the setup guide."),
        "expected subtree markdown to include the child's content, got: {:?}",
        expanded_root.markdown
    );
    assert!(expanded_root.markdown.contains("Team Onboarding"));
    // The result's own `content` field is untouched by the opt-in — only the
    // new `markdown` field carries the expanded subtree.
    assert_eq!(expanded_root.content, "# Team Onboarding");

    let _ = shutdown.send(());
}

/// A negative `include_markdown` from a malformed/adversarial client must
/// not panic via `i32 -> usize` wraparound — it clamps to 0 (no markdown),
/// the same as never sending the field.
#[tokio::test]
async fn search_nodes_negative_include_markdown_clamps_to_zero() {
    let (mut client, node_service, shutdown, _tempdir) = spawn_test_daemon_with_embeddings().await;

    let root_id = node_service
        .create_node(nodespace_core::models::Node::new(
            "text".to_string(),
            "Some root".to_string(),
            serde_json::json!({}),
        ))
        .await
        .expect("out-of-band create root failed");

    let response = client
        .search_nodes(SearchRequest {
            query: "*".into(),
            node_types: vec!["text".into()],
            semantic: true,
            include_markdown: -1,
            ..SearchRequest::default()
        })
        .await
        .expect("search_nodes with negative include_markdown failed")
        .into_inner();

    let root = response
        .nodes
        .iter()
        .find(|n| n.id == root_id)
        .expect("root present in results");
    assert!(root.markdown.is_empty());

    let _ = shutdown.send(());
}

/// An oversized `include_markdown` (well above the documented cap) still
/// only attaches markdown to at most 5 results — the upper bound is
/// enforced server-side, not merely by what a well-behaved client happens
/// to send.
#[tokio::test]
async fn search_nodes_oversized_include_markdown_still_caps_at_five() {
    let (mut client, node_service, shutdown, _tempdir) = spawn_test_daemon_with_embeddings().await;

    for i in 0..7 {
        node_service
            .create_node(nodespace_core::models::Node::new(
                "text".to_string(),
                format!("Capped root {i}"),
                serde_json::json!({}),
            ))
            .await
            .expect("out-of-band create root failed");
    }

    let response = client
        .search_nodes(SearchRequest {
            query: "*".into(),
            node_types: vec!["text".into()],
            semantic: true,
            include_markdown: 100,
            ..SearchRequest::default()
        })
        .await
        .expect("search_nodes with oversized include_markdown failed")
        .into_inner();

    assert_eq!(response.count, 7, "expected all 7 seeded roots back");
    let with_markdown = response
        .nodes
        .iter()
        .filter(|n| !n.markdown.is_empty())
        .count();
    assert_eq!(
        with_markdown, 5,
        "include_markdown must be capped at 5 regardless of the requested value"
    );

    let _ = shutdown.send(());
}

/// Per-event timeout for WatchNodes streaming tests. Generous enough to
/// absorb a loaded CI runner but short enough to fail fast when an event is
/// genuinely missing.
const WATCH_EVENT_TIMEOUT: Duration = Duration::from_secs(5);

/// Pull the next event off a WatchNodes stream with a timeout so tests fail
/// fast instead of hanging forever when an event is dropped.
async fn next_event_with_timeout(
    stream: &mut tonic::Streaming<nodespace_daemon::nodespace::NodeEvent>,
) -> nodespace_daemon::nodespace::NodeEvent {
    tokio::time::timeout(WATCH_EVENT_TIMEOUT, stream.next())
        .await
        .expect("timed out waiting for WatchNodes event")
        .expect("stream ended unexpectedly")
        .expect("stream item was an error")
}

/// Acceptance criterion: mutate a node via gRPC, verify the watcher
/// receives the corresponding event. Covers all three event kinds in one go.
#[tokio::test]
async fn watch_nodes_receives_create_update_delete_events() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    // Open the watch stream BEFORE issuing any mutation so we see the events.
    // We use a second client handle for streaming so the main client can keep
    // issuing unary requests without contention with the streaming response.
    let mut watch_client = client.clone();
    let mut stream = watch_client
        .watch_nodes(WatchRequest {
            node_type: String::new(),
            root_id: String::new(),
        })
        .await
        .expect("watch_nodes failed")
        .into_inner();

    // Trigger create → update → delete and observe each event.
    let created = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "watched node".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create_node failed")
        .into_inner();
    let node_id = created.node_id.clone();

    let create_event = next_event_with_timeout(&mut stream).await;
    match create_event.event {
        Some(NodeEventKind::Created(data)) => {
            assert_eq!(data.id, node_id);
            assert_eq!(data.content, "watched node");
            assert_eq!(data.node_type, "text");
        }
        other => panic!("expected Created event, got {:?}", other),
    }

    client
        .update_node(UpdateNodeRequest {
            node_id: node_id.clone(),
            version: None,
            node_type: None,
            content: Some("watched node v2".into()),
            properties: None,
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        })
        .await
        .expect("update_node failed");

    let update_event = next_event_with_timeout(&mut stream).await;
    match update_event.event {
        Some(NodeEventKind::Updated(data)) => {
            assert_eq!(data.id, node_id);
            assert_eq!(data.content, "watched node v2");
            assert!(data.version >= 2);
        }
        other => panic!("expected Updated event, got {:?}", other),
    }

    client
        .delete_node(DeleteNodeRequest {
            node_id: node_id.clone(),
            version: None,
        })
        .await
        .expect("delete_node failed");

    let delete_event = next_event_with_timeout(&mut stream).await;
    match delete_event.event {
        Some(NodeEventKind::Deleted(d)) => {
            assert_eq!(d.node_id, node_id);
            assert_eq!(d.node_type, "text");
        }
        other => panic!("expected Deleted event, got {:?}", other),
    }

    let _ = shutdown.send(());
}

/// Acceptance criterion: multiple concurrent watchers supported
/// simultaneously. Both must receive the same event from a single mutation.
#[tokio::test]
async fn watch_nodes_supports_multiple_concurrent_watchers() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let mut watch_a = client.clone();
    let mut watch_b = client.clone();
    let mut stream_a = watch_a
        .watch_nodes(WatchRequest::default())
        .await
        .expect("watch_a failed")
        .into_inner();
    let mut stream_b = watch_b
        .watch_nodes(WatchRequest::default())
        .await
        .expect("watch_b failed")
        .into_inner();

    let created = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "broadcast me".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create_node failed")
        .into_inner();

    let (event_a, event_b) = tokio::join!(
        next_event_with_timeout(&mut stream_a),
        next_event_with_timeout(&mut stream_b),
    );

    for event in [event_a, event_b] {
        match event.event {
            Some(NodeEventKind::Created(data)) => {
                assert_eq!(data.id, created.node_id);
                assert_eq!(data.content, "broadcast me");
            }
            other => panic!("expected Created event on both watchers, got {:?}", other),
        }
    }

    let _ = shutdown.send(());
}

/// End-to-end regression for ADR-026's C5 extension (daemon-side same-origin
/// echo suppression): a desktop window's own write must never echo back on
/// its own `WatchNodes` stream, over the real tonic wire format — not just
/// the in-process `Request::new()` construction covered by the daemon's unit
/// tests. Same-origin classification was previously a client-side
/// content-comparison guess that repeatedly produced false-positive/false-
/// negative conflict toasts; the daemon is now the sole authority, keyed on a
/// real `x-ns-client-id` metadata header round-tripped over the wire exactly
/// as `packages/desktop-app/src-tauri/src/services/grpc_client.rs`'s
/// `DatabaseIdInterceptor` stamps it on every request.
#[tokio::test]
async fn watch_nodes_over_real_transport_suppresses_own_echo_and_delivers_foreign_writes() {
    let (client, shutdown, _tempdir) = spawn_test_daemon().await;

    // "window-a" and "window-b" simulate two independent GrpcClient instances
    // (two desktop windows / a desktop window + a future second client), each
    // with its own stable x-ns-client-id, exactly as production code stamps
    // it via the interceptor. Both handles below are clones of the SAME tonic
    // client/channel — the daemon distinguishes them purely by the
    // x-ns-client-id metadata header on each request, not by transport
    // connection, so reusing one client with per-request headers is a
    // faithful simulation of two separate GrpcClient processes.
    let mut window_a = client.clone();
    let mut window_b = client.clone();

    let mut watch_a = tonic::Request::new(WatchRequest::default());
    watch_a
        .metadata_mut()
        .insert("x-ns-client-id", "window-a".parse().unwrap());
    let mut stream_a = window_a
        .watch_nodes(watch_a)
        .await
        .expect("watch_a failed")
        .into_inner();

    let mut watch_b = tonic::Request::new(WatchRequest::default());
    watch_b
        .metadata_mut()
        .insert("x-ns-client-id", "window-b".parse().unwrap());
    let mut stream_b = window_b
        .watch_nodes(watch_b)
        .await
        .expect("watch_b failed")
        .into_inner();

    // window-a creates a node, tagging the write with its own client id —
    // exactly as every routed write RPC does in production.
    let mut create = tonic::Request::new(CreateNodeRequest {
        node_type: "text".into(),
        content: "window-a's own write".into(),
        parent_id: None,
        properties: String::new(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    });
    create
        .metadata_mut()
        .insert("x-ns-client-id", "window-a".parse().unwrap());
    let created = window_a
        .create_node(create)
        .await
        .expect("create_node failed")
        .into_inner();

    // window-b (a different client) sees the write.
    let event_b = next_event_with_timeout(&mut stream_b).await;
    match event_b.event {
        Some(NodeEventKind::Created(data)) => assert_eq!(data.id, created.node_id),
        other => panic!(
            "expected window-b to see the foreign write, got {:?}",
            other
        ),
    }

    // window-a's own WatchNodes stream must NOT receive its own write back.
    // Race the real event (if the bug were reintroduced) against a timeout —
    // this must resolve via timeout, not via receiving the echo.
    let no_echo = tokio::time::timeout(Duration::from_millis(500), stream_a.next()).await;
    assert!(
        no_echo.is_err(),
        "window-a's own WatchNodes stream must not see its own write echoed back, got: {:?}",
        no_echo
    );

    let _ = shutdown.send(());
}

/// Verifies the server-side stream closes cleanly when the client drops its
/// receiver, rather than holding the broadcast subscription forever. Matches
/// the AC "Stream closes gracefully when the client disconnects".
#[tokio::test]
async fn watch_nodes_closes_when_client_drops_stream() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    {
        let mut transient = client.clone();
        let _stream = transient
            .watch_nodes(WatchRequest::default())
            .await
            .expect("watch failed")
            .into_inner();
        // Stream is dropped at end of this block; the server-side task should
        // observe the receiver-half being gone via tonic's cancellation
        // signaling and break its loop. We don't have a direct hook into that,
        // but we can verify a fresh watcher still works afterwards (i.e. the
        // server is still healthy and tracking subscribers correctly).
    }

    let mut fresh = client.clone();
    let mut stream = fresh
        .watch_nodes(WatchRequest::default())
        .await
        .expect("fresh watch failed")
        .into_inner();

    client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "post-drop".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create_node failed");

    let event = next_event_with_timeout(&mut stream).await;
    assert!(
        matches!(event.event, Some(NodeEventKind::Created(_))),
        "expected fresh watcher to receive Created event after previous watcher dropped"
    );

    let _ = shutdown.send(());
}

/// Verifies CreateNode rejects malformed property JSON with InvalidArgument
/// rather than letting the parse error reach the ops layer as `Internal`.
#[tokio::test]
async fn create_node_rejects_malformed_properties() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let err = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "irrelevant".into(),
            parent_id: None,
            properties: "{not valid json".into(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect_err("expected invalid_argument");

    assert_eq!(err.code(), Code::InvalidArgument);

    let _ = shutdown.send(());
}

/// Verify that `position: Beginning` places a second node before the first.
#[tokio::test]
async fn test_insert_position_beginning_and_after_proto_decoding() {
    let (mut client, shutdown, _dir) = spawn_test_daemon().await;

    // Create parent
    let parent_resp = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "Parent".into(),
            parent_id: None,
            properties: "{}".into(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create parent")
        .into_inner();
    let parent_id = parent_resp.node_id;

    // Child A — appended via default (End)
    let child_a_resp = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "Child A".into(),
            parent_id: Some(parent_id.clone()),
            properties: "{}".into(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: Some(CreatePos::End(true)),
        })
        .await
        .expect("create child A")
        .into_inner();
    let child_a_id = child_a_resp.node_id;

    // Child B — inserted at Beginning, should become first
    client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "Child B".into(),
            parent_id: Some(parent_id.clone()),
            properties: "{}".into(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: Some(CreatePos::Beginning(true)),
        })
        .await
        .expect("create child B");

    let children = client
        .get_children(GetChildrenRequest {
            node_id: parent_id.clone(),
        })
        .await
        .expect("get children")
        .into_inner();

    let nodes = children.nodes;
    assert_eq!(nodes.len(), 2, "parent should have 2 children");
    assert_eq!(
        nodes[0].content, "Child B",
        "Beginning: Child B should be first"
    );
    assert_eq!(nodes[1].content, "Child A", "End: Child A should be second");

    // Now reorder Child A to Beginning via ReorderNode
    client
        .reorder_node(ReorderNodeRequest {
            node_id: child_a_id.clone(),
            version: 1,
            position: Some(ReorderPos::Beginning(true)),
        })
        .await
        .expect("reorder child A to beginning");

    let children2 = client
        .get_children(GetChildrenRequest {
            node_id: parent_id.clone(),
        })
        .await
        .expect("get children after reorder")
        .into_inner();

    let nodes2 = children2.nodes;
    assert_eq!(
        nodes2[0].content, "Child A",
        "After reorder, Child A should be first"
    );
    assert_eq!(
        nodes2[1].content, "Child B",
        "After reorder, Child B should be second"
    );

    // Reorder Child B to After(Child A) — B should end up second again
    client
        .reorder_node(ReorderNodeRequest {
            node_id: nodes2[1].id.clone(), // Child B
            version: 1,
            position: Some(ReorderPos::After(child_a_id.clone())),
        })
        .await
        .expect("reorder child B after child A");

    let children3 = client
        .get_children(GetChildrenRequest {
            node_id: parent_id.clone(),
        })
        .await
        .expect("get children after After reorder")
        .into_inner();

    assert_eq!(children3.nodes[0].content, "Child A");
    assert_eq!(children3.nodes[1].content, "Child B");

    let _ = shutdown.send(());
}

/// Parity test: create a node with a collection path and a non-default
/// lifecycle status via the gRPC RPC. Verifies the daemon delegates to node_ops
/// correctly — the node lands in the resolved collection and has the requested
/// lifecycle status.
#[tokio::test]
async fn create_node_with_collection_and_lifecycle_status() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    // Create the collection node first so the path resolves.
    let _coll = client
        .create_collection(CreateCollectionRequest {
            name: "test-collection".into(),
            description: String::new(),
        })
        .await
        .expect("create_collection failed");

    let created = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "parity test node".into(),
            parent_id: None,
            properties: String::new(),
            collections: vec!["test-collection".into()],
            collection_ids: Vec::new(),
            lifecycle_status: Some("archived".into()),
            id: None,
            position: None,
        })
        .await
        .expect("create_node failed")
        .into_inner();

    let data = created.node_data.expect("missing node_data");
    assert_eq!(
        data.lifecycle_status, "archived",
        "lifecycle_status must be set"
    );
    // Verify the node is actually a member of the collection via a separate RPC.
    let memberships = client
        .get_node_collections(NodeCollectionsRequest {
            node_id: created.node_id.clone(),
        })
        .await
        .expect("get_node_collections failed")
        .into_inner();
    assert!(
        !memberships.collection_ids.is_empty(),
        "node should belong to at least one collection"
    );

    let _ = shutdown.send(());
}

/// Parity test: update a node without supplying version — the
/// daemon delegates to node_ops, which auto-fetches the current version.
#[tokio::test]
async fn update_node_auto_fetches_version_when_omitted() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let created = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "before".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create_node failed")
        .into_inner();

    // Omit version — node_ops auto-fetches it; no VersionConflict expected.
    let updated = client
        .update_node(UpdateNodeRequest {
            node_id: created.node_id.clone(),
            version: None,
            node_type: None,
            content: Some("after".into()),
            properties: None,
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        })
        .await
        .expect("update_node without version failed")
        .into_inner();

    let data = updated.node_data.expect("missing node_data");
    assert_eq!(data.content, "after");
    assert!(data.version >= 2, "version must be bumped after update");

    let _ = shutdown.send(());
}

/// Parity test: add then remove a collection membership via
/// update_node's add_to_collections (paths) / remove_from_collection_ids (ids) fields.
#[tokio::test]
async fn update_node_add_then_remove_collection_membership() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let _coll = client
        .create_collection(CreateCollectionRequest {
            name: "membership-test".into(),
            description: String::new(),
        })
        .await
        .expect("create_collection failed")
        .into_inner();
    let collection_id = _coll.collection_id;

    let node = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "membership node".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create_node failed")
        .into_inner();
    let node_id = node.node_id.clone();

    // Add to collection via update_node.
    client
        .update_node(UpdateNodeRequest {
            node_id: node_id.clone(),
            version: None,
            node_type: None,
            content: None,
            properties: None,
            add_to_collections: vec!["membership-test".into()],
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: Vec::new(),
            lifecycle_status: None,
        })
        .await
        .expect("add_to_collection failed");

    let after_add = client
        .get_node_collections(NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections failed")
        .into_inner();
    assert!(
        after_add.collection_ids.contains(&collection_id),
        "node should be in collection after add"
    );

    // Remove from collection via update_node.
    client
        .update_node(UpdateNodeRequest {
            node_id: node_id.clone(),
            version: None,
            node_type: None,
            content: None,
            properties: None,
            add_to_collections: Vec::new(),
            add_to_collection_ids: Vec::new(),
            remove_from_collection_ids: vec![collection_id.clone()],
            lifecycle_status: None,
        })
        .await
        .expect("remove_from_collection failed");

    let after_remove = client
        .get_node_collections(NodeCollectionsRequest {
            node_id: node_id.clone(),
        })
        .await
        .expect("get_node_collections failed")
        .into_inner();
    assert!(
        !after_remove.collection_ids.contains(&collection_id),
        "node should not be in collection after remove"
    );

    let _ = shutdown.send(());
}

/// B5 AC: move_node with new_parent_id unset (None) must move to root.
/// Verifies the optional field semantics: unset = root, present = reparent.
#[tokio::test]
async fn move_node_to_root_when_new_parent_id_unset() {
    use nodespace_daemon::nodespace::{GetRootsRequest, MoveNodeRequest};

    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    // Create a parent node and a child.
    let parent = client
        .create_node(CreateNodeRequest {
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
        .expect("create parent")
        .into_inner();

    let child = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "child".into(),
            parent_id: Some(parent.node_id.clone()),
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create child")
        .into_inner();

    // Move child to root — new_parent_id = None (unset).
    client
        .move_node(MoveNodeRequest {
            node_id: child.node_id.clone(),
            version: 1,
            new_parent_id: None,
            position: None,
        })
        .await
        .expect("move_node to root failed");

    // Child should now appear in roots.
    let roots = client
        .get_roots(GetRootsRequest {
            limit: 100,
            offset: 0,
        })
        .await
        .expect("get_roots failed")
        .into_inner();
    let root_ids: Vec<&str> = roots.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(
        root_ids.contains(&child.node_id.as_str()),
        "child should be a root node after move with new_parent_id=None"
    );

    // Original parent should have no children.
    let children_of_parent = client
        .get_children(GetChildrenRequest {
            node_id: parent.node_id.clone(),
        })
        .await
        .expect("get_children failed")
        .into_inner();
    assert_eq!(
        children_of_parent.count, 0,
        "original parent should have no children after move"
    );

    let _ = shutdown.send(());
}

/// B5 AC: move_node with new_parent_id = Some("") must also move to root.
/// The daemon normalizes "" → None so legacy callers aren't broken.
#[tokio::test]
async fn move_node_to_root_when_new_parent_id_empty_string() {
    use nodespace_daemon::nodespace::{GetRootsRequest, MoveNodeRequest};

    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let parent = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "parent2".into(),
            parent_id: None,
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create parent")
        .into_inner();

    let child = client
        .create_node(CreateNodeRequest {
            node_type: "text".into(),
            content: "child2".into(),
            parent_id: Some(parent.node_id.clone()),
            properties: String::new(),
            collections: Vec::new(),
            collection_ids: Vec::new(),
            lifecycle_status: None,
            id: None,
            position: None,
        })
        .await
        .expect("create child")
        .into_inner();

    // Move to root via empty string — daemon normalizes "" to None.
    client
        .move_node(MoveNodeRequest {
            node_id: child.node_id.clone(),
            version: 1,
            new_parent_id: Some(String::new()),
            position: None,
        })
        .await
        .expect("move_node with empty new_parent_id failed");

    let roots = client
        .get_roots(GetRootsRequest {
            limit: 100,
            offset: 0,
        })
        .await
        .expect("get_roots failed")
        .into_inner();
    let root_ids: Vec<&str> = roots.nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(
        root_ids.contains(&child.node_id.as_str()),
        "child should be a root node after move with new_parent_id=''"
    );

    let _ = shutdown.send(());
}

/// Fetch one `QueryNodesSimple` page scoped to nodes whose content contains
/// `marker`, returning just the node ids in response order.
async fn fetch_marked_page(
    client: &mut NodeServiceClient<tonic::transport::Channel>,
    marker: &str,
    limit: u32,
    offset: u32,
) -> Vec<String> {
    use nodespace_daemon::nodespace::QueryNodesSimpleRequest;

    client
        .query_nodes_simple(QueryNodesSimpleRequest {
            id: None,
            mentioned_by: None,
            content_contains: Some(marker.to_string()),
            title_contains: None,
            node_type: Some("text".to_string()),
            limit,
            offset,
            // Unspecified — exercises the daemon's deterministic default
            // rather than an explicitly client-chosen order.
            order_by: 0,
        })
        .await
        .expect("query_nodes_simple failed")
        .into_inner()
        .nodes
        .into_iter()
        .map(|n| n.id)
        .collect()
}

/// Regression test: the daemon's `query_nodes_simple` handler
/// used to hardcode `order_by: None` on every `NodeQuery` it built, so even
/// though `SqliteStore::query_nodes` gained a real `ORDER BY`, the gRPC surface never actually used it. Offset-based pagination
/// through this RPC could therefore return duplicate or skipped rows across
/// pages — the exact failure mode `order_by`/`limit`/`offset` are supposed to
/// prevent. The daemon now always applies a deterministic default order
/// (`order_by_from_proto`), so repeated calls with the same filter and
/// advancing `offset` must tile the full matching set exactly once each, and
/// repeating an identical page must return identical rows every time.
#[tokio::test]
async fn query_nodes_simple_pages_tile_full_set_exactly_once_with_default_ordering() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    const TOTAL: usize = 47;
    const PAGE: u32 = 20;
    const MARKER: &str = "pagination-tiling-probe";

    let mut created_ids = std::collections::HashSet::with_capacity(TOTAL);
    for i in 0..TOTAL {
        let node = client
            .create_node(CreateNodeRequest {
                node_type: "text".into(),
                content: format!("{MARKER} {i:03}"),
                parent_id: None,
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            })
            .await
            .expect("create probe node")
            .into_inner();
        created_ids.insert(node.node_id);
    }

    let page1 = fetch_marked_page(&mut client, MARKER, PAGE, 0).await;
    let page2 = fetch_marked_page(&mut client, MARKER, PAGE, PAGE).await;
    let page3 = fetch_marked_page(&mut client, MARKER, PAGE, PAGE * 2).await;

    assert_eq!(page1.len(), 20, "page 1 must be full");
    assert_eq!(page2.len(), 20, "page 2 must be full");
    assert_eq!(page3.len(), TOTAL - 40, "page 3 holds the remainder");

    let mut combined = page1.clone();
    combined.extend(page2.clone());
    combined.extend(page3.clone());

    let combined_set: std::collections::HashSet<String> = combined.iter().cloned().collect();
    assert_eq!(
        combined_set.len(),
        combined.len(),
        "no row may appear on more than one page — got duplicates across page boundaries"
    );
    assert_eq!(
        combined_set, created_ids,
        "the tiled pages must reconstruct exactly the created set — no gaps"
    );

    // Stability: repeating the same page must return the identical rows in
    // the identical order every time, not just an internally-consistent-but-
    // different subset.
    for attempt in 0..3 {
        let repeat = fetch_marked_page(&mut client, MARKER, PAGE, PAGE).await;
        assert_eq!(
            repeat, page2,
            "attempt {attempt}: repeated identical page must return the identical page"
        );
    }

    let _ = shutdown.send(());
}

/// Regression test: `QueryNodesSimple` used to pass a
/// client-requested `limit` straight through to the store with no ceiling,
/// so a large enough request returned every matching row in one unary
/// message and relied on the gRPC message-size limit (since raised)
/// to badly fail anything larger still. The daemon now clamps `limit` to a
/// maximum page size regardless of what the client asks for — seed a dataset
/// larger than that clamp and confirm the response never exceeds it even
/// when the request asks for far more.
#[tokio::test]
async fn query_nodes_simple_clamps_an_oversized_limit_to_the_max_page_size() {
    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    // Strictly more than the daemon's max page size (500 at the time of
    // writing) so an unclamped response would be observably larger than the
    // clamp — if this ever fails because the constant changed, that's the
    // point: the test pins the exact ceiling.
    const SEEDED: usize = 505;
    const EXPECTED_MAX_PAGE_SIZE: usize = 500;
    const MARKER: &str = "clamp-probe";

    for i in 0..SEEDED {
        client
            .create_node(CreateNodeRequest {
                node_type: "text".into(),
                content: format!("{MARKER} {i:03}"),
                parent_id: None,
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            })
            .await
            .expect("create probe node");
    }

    let page = fetch_marked_page(&mut client, MARKER, 100_000, 0).await;

    assert_eq!(
        page.len(),
        EXPECTED_MAX_PAGE_SIZE,
        "a request for far more than the max page size must be clamped, not \
         let through unbounded — {SEEDED} matching rows exist but the \
         response must cap at {EXPECTED_MAX_PAGE_SIZE}"
    );

    let _ = shutdown.send(());
}

/// `CountNodes`/`CountRoots` must report exact totals without
/// requiring the caller to fetch (and page through) the matching records —
/// a count-only path so a caller like `nodespace
/// diagnostics` doesn't pay to transfer records it only calls `.len()` on.
#[tokio::test]
async fn count_nodes_and_count_roots_report_totals_without_transferring_records() {
    use nodespace_daemon::nodespace::QueryNodesSimpleRequest;

    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    let roots_before = client
        .count_roots(Empty {})
        .await
        .expect("count_roots failed")
        .into_inner()
        .count;

    const SEEDED: i64 = 7;
    const MARKER: &str = "count-probe";
    for i in 0..SEEDED {
        client
            .create_node(CreateNodeRequest {
                node_type: "text".into(),
                content: format!("{MARKER} {i}"),
                parent_id: None,
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            })
            .await
            .expect("create probe node");
    }

    let count = client
        .count_nodes(QueryNodesSimpleRequest {
            id: None,
            mentioned_by: None,
            content_contains: Some(MARKER.to_string()),
            title_contains: None,
            node_type: Some("text".to_string()),
            limit: 0,
            offset: 0,
            order_by: 0,
        })
        .await
        .expect("count_nodes failed")
        .into_inner()
        .count;
    assert_eq!(
        count, SEEDED,
        "count_nodes must report the exact matching total"
    );

    let roots_after = client
        .count_roots(Empty {})
        .await
        .expect("count_roots failed")
        .into_inner()
        .count;
    assert_eq!(
        roots_after,
        roots_before + SEEDED,
        "every probe node is a root (no parent_id) — count_roots must reflect all of them"
    );

    let _ = shutdown.send(());
}

/// `CountQuery` is the `ExecuteQuery` counterpart to `CountNodes`: it answers
/// "how many nodes match these filters?" with a scalar, over the structured
/// filter shape rather than the type/text one.
///
/// The `limit` assertion is the point of the RPC, not an edge case. Counting an
/// `ExecuteQuery` response caps the answer at the rows that response carries, so
/// the query editor's preview could only ever report "at least N". A count has
/// to see past the limit to report a real total.
#[tokio::test]
async fn count_query_reports_exact_total_ignoring_limit_and_sorting() {
    use nodespace_daemon::nodespace::ExecuteQueryRequest;

    let (mut client, shutdown, _tempdir) = spawn_test_daemon().await;

    const SEEDED: i64 = 6;
    const MARKER: &str = "count-query-probe";
    for i in 0..SEEDED {
        client
            .create_node(CreateNodeRequest {
                node_type: "text".into(),
                content: format!("{MARKER} {i}"),
                parent_id: None,
                properties: String::new(),
                collections: Vec::new(),
                collection_ids: Vec::new(),
                lifecycle_status: None,
                id: None,
                position: None,
            })
            .await
            .expect("create probe node");
    }

    let filters = format!(r#"[{{"type":"content","operator":"contains","value":"{MARKER}"}}]"#);

    // A limit well below the true total, and a sort — neither may affect the
    // count the daemon returns.
    let count = client
        .count_query(ExecuteQueryRequest {
            target_type: "text".to_string(),
            filters_json: Some(filters.clone()),
            sorting_json: Some(r#"[{"field":"created_at","direction":"desc"}]"#.to_string()),
            limit: 2,
        })
        .await
        .expect("count_query failed")
        .into_inner()
        .count;
    assert_eq!(
        count, SEEDED,
        "count_query must report the exact matching total, not saturate at the request's limit"
    );

    // And the same filters through ExecuteQuery must select the rows that were
    // counted — the two verbs agreeing is what makes the count trustworthy.
    let rows = client
        .execute_query(ExecuteQueryRequest {
            target_type: "text".to_string(),
            filters_json: Some(filters),
            sorting_json: None,
            limit: 0,
        })
        .await
        .expect("execute_query failed")
        .into_inner()
        .nodes
        .len() as i64;
    assert_eq!(
        rows, count,
        "execute_query and count_query must agree on the same filters"
    );

    let _ = shutdown.send(());
}
