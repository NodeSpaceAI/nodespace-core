//! End-to-end gRPC integration test for `AgentSessionService`.
//!
//! Spins up the tonic server in-process with a real `PtySessionManager` and
//! exercises the streaming-output path against a real PTY session.
//!
//! The production `LaunchSession` RPC requires a catalogued agent binary
//! (`claude`, `codex`, ...) on `PATH`, which CI does not have. To keep the
//! test hermetic the session is built directly with [`PtySession::launch_for_test`]
//! (exposed via the agent crate's `testing` feature) and inserted into the
//! manager bypassing the launch path. Every other RPC — `StreamOutput`,
//! `WriteInput`, `ResizeTerminal`, `TerminateSession`, `ListSessions` — runs
//! through the full gRPC stack.

#![cfg(unix)] // PtySession::launch_for_test spawns shell utilities by name.

use std::sync::Arc;
use std::time::Duration;

use nodespace_agent::agent_catalog::context_assembly::GraphContextAssembler;
use nodespace_agent::pty::{PtySession, PtySessionManager};
use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
use nodespace_daemon::nodespace::{
    ListSessionsRequest, ResizeRequest, StreamOutputRequest, TerminateSessionRequest,
    WriteInputRequest,
};
use nodespace_daemon::{AgentSessionHandler, AgentSessionServiceClient, AgentSessionServiceServer};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_stream::StreamExt;
use tonic::transport::Server;
use tonic::Code;

type Client = AgentSessionServiceClient<tonic::transport::Channel>;

/// Bring up the agent-session gRPC server in-process and return a connected
/// client plus the shared manager (so tests can seed sessions directly), the
/// shutdown sender, and the tempdir that backs the SqliteStore. Holding the
/// tempdir in the returned tuple keeps it alive past the test body.
async fn spawn_test_daemon() -> (Client, Arc<PtySessionManager>, oneshot::Sender<()>, TempDir) {
    let tempdir = TempDir::new().expect("tempdir");

    // The assembler holds a NodeService handle even though our tests bypass
    // LaunchSession. SqliteStore is the only realistic way to produce one.
    let mut store = Arc::new(
        SqliteStore::new(tempdir.path().join("daemon-db"))
            .await
            .expect("SqliteStore"),
    );
    let node_service = Arc::new(CoreNodeService::new(&mut store).await.expect("NodeService"));

    let manager = Arc::new(PtySessionManager::new());
    let assembler = Arc::new(GraphContextAssembler::new(
        node_service.clone(),
        Arc::new(tokio::sync::RwLock::new(None)),
    ));
    let capture_config_path = tempdir.path().join("daemon.toml");
    let handler = AgentSessionHandler::new(
        manager.clone(),
        assembler,
        node_service,
        capture_config_path,
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        Server::builder()
            .add_service(AgentSessionServiceServer::new(handler))
            .serve_with_incoming_shutdown(incoming, async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    let endpoint = format!("http://{}", addr);
    let mut last_err = None;
    for _ in 0..50 {
        match AgentSessionServiceClient::connect(endpoint.clone()).await {
            Ok(client) => return (client, manager, shutdown_tx, tempdir),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }
    panic!("failed to connect to in-process daemon: {:?}", last_err);
}

/// Pull chunks from a server-streaming response until the accumulated output
/// contains `needle` or `deadline` elapses, returning everything collected.
async fn collect_until(
    stream: &mut tonic::Streaming<nodespace_daemon::nodespace::OutputChunk>,
    needle: &str,
    deadline: Duration,
) -> Vec<u8> {
    let mut collected = Vec::new();
    let _ = timeout(deadline, async {
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => {
                    collected.extend_from_slice(&chunk.data);
                    if std::str::from_utf8(&collected)
                        .map(|s| s.contains(needle))
                        .unwrap_or(false)
                    {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    })
    .await;
    collected
}

#[tokio::test]
async fn stream_output_delivers_echo_hello() {
    let (mut client, manager, shutdown, _tempdir) = spawn_test_daemon().await;

    // The session runs through the full PTY pipeline so StreamOutput sees
    // real broadcast traffic. We delay the echo by 200 ms so the
    // `client.stream_output(...)` subscriber has time to attach before any
    // bytes hit the broadcast channel — `tokio::sync::broadcast` does not
    // replay missed messages to late subscribers, so a bare `echo hello`
    // races: on a fast machine the reader can drain the PTY and broadcast
    // the chunk before our gRPC client opens the stream.
    let session =
        PtySession::launch_for_test("sh", vec!["-c".into(), "sleep 0.2 && echo hello".into()])
            .expect("launch echo session");
    let id = manager.insert(session).await;

    let mut stream = client
        .stream_output(StreamOutputRequest {
            session_id: id.to_string(),
        })
        .await
        .expect("stream_output rpc accepted")
        .into_inner();

    let collected = collect_until(&mut stream, "hello", Duration::from_secs(3)).await;
    let text = String::from_utf8_lossy(&collected);
    assert!(
        text.contains("hello"),
        "expected 'hello' in PTY output, got: {:?}",
        text
    );

    let _ = shutdown.send(());
}

#[tokio::test]
async fn list_sessions_reflects_manager_state() {
    let (mut client, manager, shutdown, _tempdir) = spawn_test_daemon().await;

    let empty = client
        .list_sessions(ListSessionsRequest {})
        .await
        .expect("list_sessions empty")
        .into_inner();
    assert_eq!(empty.count, 0);
    assert!(empty.sessions.is_empty());

    let session = PtySession::launch_for_test("sh", vec!["-c".into(), "sleep 30".into()])
        .expect("launch sleep session");
    let id = manager.insert(session).await;

    let listed = client
        .list_sessions(ListSessionsRequest {})
        .await
        .expect("list_sessions populated")
        .into_inner();
    assert_eq!(listed.count, 1);
    assert_eq!(listed.sessions[0].session_id, id.to_string());
    assert_eq!(listed.sessions[0].agent_type, "claude-code");
    assert!(listed.sessions[0].started_at > 0);

    // Cleanup so the test does not leak a sleeping child past the shutdown.
    client
        .terminate_session(TerminateSessionRequest {
            session_id: id.to_string(),
        })
        .await
        .expect("terminate rpc");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn write_input_is_echoed_back_through_stream() {
    let (mut client, manager, shutdown, _tempdir) = spawn_test_daemon().await;

    // `cat` echoes stdin back to stdout, which the PTY then loops back to its
    // output stream. Lets us round-trip WriteInput → StreamOutput through gRPC.
    let session = PtySession::launch_for_test("cat", vec![]).expect("launch cat");
    let id = manager.insert(session).await;

    let mut stream = client
        .stream_output(StreamOutputRequest {
            session_id: id.to_string(),
        })
        .await
        .expect("stream_output rpc")
        .into_inner();

    let written = client
        .write_input(WriteInputRequest {
            session_id: id.to_string(),
            data: b"ping\n".to_vec(),
        })
        .await
        .expect("write_input rpc")
        .into_inner();
    assert_eq!(written.bytes_written, 5);

    let collected = collect_until(&mut stream, "ping", Duration::from_secs(3)).await;
    assert!(
        String::from_utf8_lossy(&collected).contains("ping"),
        "expected 'ping' echoed back, got: {:?}",
        String::from_utf8_lossy(&collected)
    );

    // Tear down the cat session through TerminateSession so the test exercises
    // the terminate RPC too.
    let term = client
        .terminate_session(TerminateSessionRequest {
            session_id: id.to_string(),
        })
        .await
        .expect("terminate rpc")
        .into_inner();
    assert!(
        term.was_running,
        "cat session was running, should report true"
    );
    assert_eq!(term.session_id, id.to_string());

    let _ = shutdown.send(());
}

#[tokio::test]
async fn resize_terminal_succeeds_for_active_session() {
    let (mut client, manager, shutdown, _tempdir) = spawn_test_daemon().await;

    let session = PtySession::launch_for_test("sh", vec!["-c".into(), "sleep 5".into()])
        .expect("launch sleep");
    let id = manager.insert(session).await;

    client
        .resize_terminal(ResizeRequest {
            session_id: id.to_string(),
            cols: 120,
            rows: 40,
        })
        .await
        .expect("resize rpc");

    client
        .terminate_session(TerminateSessionRequest {
            session_id: id.to_string(),
        })
        .await
        .expect("terminate rpc");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn resize_terminal_rejects_zero_dimensions() {
    // Unlike LaunchSession (where 0 means "use engine default"), ResizeTerminal
    // rejects zero on either axis. Pins the API-boundary contract documented
    // on ResizeRequest.proto.
    let (mut client, manager, shutdown, _tempdir) = spawn_test_daemon().await;

    let session = PtySession::launch_for_test("sh", vec!["-c".into(), "sleep 5".into()])
        .expect("launch sleep");
    let id = manager.insert(session).await;

    for (cols, rows) in [(0u32, 40u32), (120, 0), (0, 0)] {
        let err = client
            .resize_terminal(ResizeRequest {
                session_id: id.to_string(),
                cols,
                rows,
            })
            .await
            .expect_err(&format!("resize with cols={cols}, rows={rows} should fail"));
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    client
        .terminate_session(TerminateSessionRequest {
            session_id: id.to_string(),
        })
        .await
        .expect("terminate rpc");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn stream_output_disconnect_does_not_kill_session() {
    let (mut client, manager, shutdown, _tempdir) = spawn_test_daemon().await;

    // A long-running session: `cat` waits on stdin forever. If dropping the
    // stream killed the session, the manager's auto-prune would remove it
    // because the watcher would observe the child exiting.
    let session = PtySession::launch_for_test("cat", vec![]).expect("launch cat");
    let id = manager.insert(session).await;

    {
        // Open the stream, then drop it immediately. tonic should close the
        // client end of the broadcast subscription without touching the
        // underlying session.
        let _stream = client
            .stream_output(StreamOutputRequest {
                session_id: id.to_string(),
            })
            .await
            .expect("stream_output rpc")
            .into_inner();
    }

    // Give any spurious cleanup task time to run before we check.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let listed = client
        .list_sessions(ListSessionsRequest {})
        .await
        .expect("list_sessions")
        .into_inner();
    assert_eq!(
        listed.count, 1,
        "session must survive a client stream disconnect"
    );
    assert_eq!(listed.sessions[0].session_id, id.to_string());

    client
        .terminate_session(TerminateSessionRequest {
            session_id: id.to_string(),
        })
        .await
        .expect("terminate rpc");

    let _ = shutdown.send(());
}

#[tokio::test]
async fn unknown_session_returns_not_found() {
    let (mut client, _manager, shutdown, _tempdir) = spawn_test_daemon().await;

    let bogus = uuid::Uuid::new_v4().to_string();

    let err = client
        .stream_output(StreamOutputRequest {
            session_id: bogus.clone(),
        })
        .await
        .expect_err("expected NotFound for unknown session");
    assert_eq!(err.code(), Code::NotFound);

    let err = client
        .write_input(WriteInputRequest {
            session_id: bogus.clone(),
            data: vec![1, 2, 3],
        })
        .await
        .expect_err("expected NotFound for unknown session");
    assert_eq!(err.code(), Code::NotFound);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn invalid_session_id_returns_invalid_argument() {
    let (mut client, _manager, shutdown, _tempdir) = spawn_test_daemon().await;

    let err = client
        .terminate_session(TerminateSessionRequest {
            session_id: "not-a-uuid".into(),
        })
        .await
        .expect_err("expected InvalidArgument for bad UUID");
    assert_eq!(err.code(), Code::InvalidArgument);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn terminate_unknown_session_reports_was_not_running() {
    let (mut client, _manager, shutdown, _tempdir) = spawn_test_daemon().await;

    let bogus = uuid::Uuid::new_v4().to_string();
    let resp = client
        .terminate_session(TerminateSessionRequest {
            session_id: bogus.clone(),
        })
        .await
        .expect("terminate rpc")
        .into_inner();

    assert!(!resp.was_running);
    assert_eq!(resp.session_id, bogus);

    let _ = shutdown.send(());
}

#[tokio::test]
async fn stream_output_reports_dropped_chunks_when_subscriber_lags() {
    let (mut client, manager, shutdown, _tempdir) = spawn_test_daemon().await;

    // Flood far more output than the broadcast buffer (256 chunks of at most
    // 4 KiB) plus the HTTP/2 receive window can hold, while the client
    // deliberately does not read. The daemon's subscriber must then lag, and
    // the gap has to reach the client as a loss marker rather than vanish.
    // The leading sleep lets the stream subscribe before the flood starts.
    let session = PtySession::launch_for_test(
        "sh",
        vec!["-c".into(), "sleep 0.2 && yes | head -c 33554432".into()],
    )
    .expect("launch flood session");
    let id = manager.insert(session).await;

    let mut stream = client
        .stream_output(StreamOutputRequest {
            session_id: id.to_string(),
        })
        .await
        .expect("stream_output rpc accepted")
        .into_inner();

    tokio::time::sleep(Duration::from_secs(2)).await;

    let marker = timeout(Duration::from_secs(10), async {
        while let Some(Ok(chunk)) = stream.next().await {
            if chunk.dropped_chunks > 0 {
                return Some(chunk);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .expect("a lagging subscriber should receive a dropped-chunks marker");

    assert!(
        marker.data.is_empty(),
        "loss marker must not carry output bytes"
    );
    assert!(marker.timestamp_ms > 0);

    client
        .terminate_session(TerminateSessionRequest {
            session_id: id.to_string(),
        })
        .await
        .expect("terminate rpc");

    let _ = shutdown.send(());
}
