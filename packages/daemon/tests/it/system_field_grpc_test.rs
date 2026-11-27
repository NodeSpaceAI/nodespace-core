//! The gRPC server the Tauri commands and the `nodespace` CLI both call
//! refuses a node write that names a `system` field, naming the field and the
//! type, and the node is left as it was.

use std::sync::Arc;
use std::time::Duration;

use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
use nodespace_daemon::nodespace::{
    BatchUpdateItem, CreateNodeRequest, GetNodeRequest, UpdateNodeRequest, UpdateNodesBatchRequest,
};
use nodespace_daemon::{NodeServiceClient, NodeServiceImpl, NodeServiceServer};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tonic::transport::Server;

async fn spawn() -> (NodeServiceClient<tonic::transport::Channel>, TempDir) {
    let tempdir = TempDir::new().expect("tempdir");
    let mut store = Arc::new(
        SqliteStore::new(tempdir.path().join("daemon-db"))
            .await
            .expect("store"),
    );
    let node_service = Arc::new(CoreNodeService::new(&mut store).await.expect("service"));
    let service = NodeServiceImpl::new(
        node_service,
        Arc::new(tokio::sync::RwLock::new(None)),
        Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    tokio::spawn(async move {
        Server::builder()
            .add_service(NodeServiceServer::new(service))
            .serve_with_incoming(incoming)
            .await
            .expect("server crashed");
    });
    for _ in 0..50 {
        if let Ok(client) = NodeServiceClient::connect(format!("http://{addr}")).await {
            return (client, tempdir);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("failed to connect to the in-process daemon");
}

fn create_request(properties: &str) -> CreateNodeRequest {
    CreateNodeRequest {
        node_type: "ai-chat-pty".into(),
        content: "Session".into(),
        parent_id: None,
        properties: properties.into(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    }
}

fn update_request(id: &str, properties: &str) -> UpdateNodeRequest {
    UpdateNodeRequest {
        node_id: id.into(),
        version: None,
        node_type: None,
        content: None,
        properties: Some(properties.into()),
        add_to_collections: Vec::new(),
        add_to_collection_ids: Vec::new(),
        remove_from_collection_ids: Vec::new(),
        lifecycle_status: None,
        typed_client: false,
        ..Default::default()
    }
}

fn names_field(message: &str) {
    assert!(
        message.contains("'summary'") && message.contains("'ai-chat'"),
        "{message}"
    );
}

#[tokio::test]
async fn the_server_refuses_a_system_field_on_create_update_and_batch() {
    let (mut client, _tmp) = spawn().await;

    let refused = client
        .create_node(create_request(
            r#"{"agent":"claude-code","summary":"forged"}"#,
        ))
        .await
        .expect_err("create is refused");
    names_field(refused.message());

    let id = client
        .create_node(create_request(r#"{"agent":"claude-code"}"#))
        .await
        .expect("a chat without system fields")
        .into_inner()
        .node_id;

    let refused = client
        .update_node(update_request(&id, r#"{"summary":"forged"}"#))
        .await
        .expect_err("update is refused");
    names_field(refused.message());

    let batch = client
        .update_nodes_batch(UpdateNodesBatchRequest {
            updates: vec![BatchUpdateItem {
                node_id: id.clone(),
                version: None,
                content: None,
                node_type: None,
                properties: Some(r#"{"summary":"forged"}"#.into()),
                ..Default::default()
            }],
        })
        .await;
    match batch {
        Ok(response) => {
            let response = response.into_inner();
            assert!(response.updated.is_empty(), "{response:?}");
            names_field(&response.failed.first().expect("a failure").error);
        }
        Err(status) => names_field(status.message()),
    }

    let node = client
        .get_node(GetNodeRequest { node_id: id })
        .await
        .expect("get")
        .into_inner()
        .node_data
        .expect("node");
    assert!(
        !node.properties.contains("forged"),
        "a refusal changes nothing: {}",
        node.properties
    );
}
