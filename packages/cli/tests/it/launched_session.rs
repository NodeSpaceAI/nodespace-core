//! A session the daemon launches for a project (ADR-093 §8), end to end.
//!
//! The daemon serves two databases and launches an agent in the second. The
//! agent is a shell script standing in for `claude`, found on a search path
//! the test owns, so nothing here depends on an installed agent. The script
//! prints where it runs and what its environment names, then runs the compiled
//! `nodespace` binary the way an agent or its plugin would: with no flags,
//! from the environment the launch gave it.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nodespace_cli::{
    connect, connect_database, connect_session, DatabaseIdInterceptor, NodeClient, SessionClient,
};
use nodespace_daemon::nodespace::{
    CreateDatabaseRequest, CreateNodeRequest, GetNodeRequest, LaunchSessionRequest,
    StreamOutputRequest, TerminateSessionRequest, UpdateNodeRequest, WriteInputRequest,
};
use nodespace_daemon::{
    AgentSessionServiceServer, DatabaseManager, DatabaseServiceImpl, DatabaseServiceServer,
    DbManagerLayer, NodeServiceServer,
};
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tonic::Code;

use crate::cli_integration::routing_test_context;

/// What the stand-in agent prints once it has run its commands.
const DONE: &str = "AGENT-DONE";

struct Daemon {
    sock: PathBuf,
    home: TempDir,
    /// The id of the second, non-default database.
    second_id: String,
    _shutdown: oneshot::Sender<()>,
}

impl Daemon {
    async fn node(&self) -> NodeClient {
        let interceptor = DatabaseIdInterceptor::for_id(&self.second_id).expect("interceptor");
        connect(&self.sock, interceptor)
            .await
            .expect("connect node")
    }

    async fn session(&self) -> SessionClient {
        let interceptor = DatabaseIdInterceptor::for_id(&self.second_id).expect("interceptor");
        connect_session(&self.sock, interceptor)
            .await
            .expect("connect session")
    }

    /// A folder standing in for the project's checkout, resolved the way a
    /// process reads its own working directory back.
    fn checkout(&self) -> PathBuf {
        let dir = self.home.path().join("checkout");
        std::fs::create_dir_all(&dir).expect("create checkout");
        dir.canonicalize().expect("resolve checkout")
    }
}

/// Write the stand-in `claude` into `bin`. It waits for a line of input, so
/// the test is streaming before it prints, then waits for a second before it
/// exits, so the test decides when the session ends.
fn write_agent(bin: &Path, task_id: &str) {
    let nodespace = env!("CARGO_BIN_EXE_nodespace");
    let script = format!(
        r#"#!/bin/sh
read _go
echo "CWD=$(pwd -P)"
echo "LAUNCHED_FOR=${{NODESPACE_LAUNCHED_FOR-unset}}"
echo "DATABASE=$NODESPACE_DATABASE"
echo "SOCKET=${{NODESPACED_SOCKET-unset}}"
echo "PATH=$PATH"
'{nodespace}' node get {task_id}
echo "GET-EXIT=$?"
'{nodespace}' session report-harness-session "harness-$NODESPACE_SESSION"
echo "REPORT-EXIT=$?"
echo {DONE}
read _end
"#
    );
    std::fs::create_dir_all(bin).expect("create bin");
    let path = bin.join("claude");
    std::fs::write(&path, script).expect("write agent");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod agent");
}

/// The multi-database stack with `AgentSessionService` beside it, a second
/// database registered, and agents found only in the test's own folder.
async fn spawn_daemon() -> Daemon {
    let home = TempDir::new().expect("tempdir");
    // The daemon's own files (its capture settings among them) are read from
    // here, not from the home of whoever runs the test. Each test is a process
    // of its own under nextest, so the variable is this test's alone.
    std::env::set_var("NODESPACE_HOME", home.path().join("nodespace-home"));
    // The daemon's own environment names another socket. A session is told the
    // one this daemon serves, whatever its environment held.
    std::env::set_var("NODESPACED_SOCKET", "/nonexistent/not-this-daemon.sock");
    let sock = home.path().join("daemon.sock");

    let context = routing_test_context();
    context.pty_manager.set_daemon_socket(sock.clone());
    let search_path = std::env::join_paths([
        home.path().join("bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ])
    .expect("search path");
    context.pty_manager.set_agent_search_path(search_path);

    let manager = Arc::new(
        DatabaseManager::load(home.path().join("databases.toml"), context)
            .await
            .expect("load DatabaseManager"),
    );
    let default_id = manager
        .ensure_default_registered("Default".into(), home.path().join("default.db"))
        .await
        .expect("register default");
    let bundle = manager
        .get_or_open(&default_id)
        .await
        .expect("open default");
    let node = bundle.node_service_grpc.clone();
    let agent_session = bundle.agent_session.clone();

    let listener = UnixListener::bind(&sock).expect("bind socket");
    let (shutdown, shutdown_rx) = oneshot::channel::<()>();
    let served = manager.clone();
    tokio::spawn(async move {
        Server::builder()
            .layer(DbManagerLayer::new(served.clone()))
            .add_service(DatabaseServiceServer::new(DatabaseServiceImpl::new(served)))
            .add_service(NodeServiceServer::new(node))
            .add_service(AgentSessionServiceServer::new(agent_session))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async move {
                let _ = shutdown_rx.await;
            })
            .await
            .expect("server crashed");
    });

    let mut db = None;
    for _ in 0..50 {
        if let Ok(client) = connect_database(&sock).await {
            db = Some(client);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let second_id = db
        .expect("daemon accepts connections")
        .create(CreateDatabaseRequest {
            name: "Second".into(),
            path: Some(home.path().join("second.db").display().to_string()),
        })
        .await
        .expect("create second database")
        .into_inner()
        .id;

    Daemon {
        sock,
        home,
        second_id,
        _shutdown: shutdown,
    }
}

async fn create(node: &mut NodeClient, node_type: &str, content: &str, properties: &str) -> String {
    node.create_node(CreateNodeRequest {
        node_type: node_type.into(),
        content: content.into(),
        parent_id: None,
        properties: properties.into(),
        collections: Vec::new(),
        collection_ids: Vec::new(),
        lifecycle_status: None,
        id: None,
        position: None,
    })
    .await
    .unwrap_or_else(|e| panic!("create {node_type}: {e}"))
    .into_inner()
    .node_id
}

/// A node's stored properties, as the daemon returns them.
async fn properties(node: &mut NodeClient, id: &str) -> serde_json::Value {
    let data = node
        .get_node(GetNodeRequest {
            node_id: id.to_string(),
        })
        .await
        .expect("get node")
        .into_inner()
        .node_data
        .expect("node data");
    serde_json::from_str(&data.properties).expect("properties are JSON")
}

fn launch_for(project_id: &str) -> LaunchSessionRequest {
    LaunchSessionRequest {
        agent_type: "claude-code".into(),
        prompt: None,
        cols: 0,
        rows: 0,
        node_id: None,
        project_id: Some(project_id.to_string()),
        project_folder: None,
        task_id: None,
    }
}

/// Start the stand-in agent's commands and return what it printed up to
/// [`DONE`].
async fn run_agent(session: &mut SessionClient, session_id: &str) -> String {
    let mut stream = session
        .stream_output(StreamOutputRequest {
            session_id: session_id.to_string(),
        })
        .await
        .expect("stream output")
        .into_inner();
    send_line(session, session_id).await;

    let mut output = String::new();
    let read = timeout(Duration::from_secs(30), async {
        while let Ok(Some(chunk)) = stream.message().await {
            output.push_str(&String::from_utf8_lossy(&chunk.data));
            // The agent's own line, not the terminal's echo of the script.
            if output.contains(&format!("{DONE}\r\n")) {
                break;
            }
        }
    })
    .await;
    assert!(read.is_ok(), "the agent did not finish, printed: {output}");
    output
}

async fn send_line(session: &mut SessionClient, session_id: &str) {
    session
        .write_input(WriteInputRequest {
            session_id: session_id.to_string(),
            data: b"\n".to_vec(),
        })
        .await
        .expect("write input");
}

/// Wait for the daemon to record the end of the session on `chat_id`, and
/// return the chat's own fields.
async fn ended_chat(node: &mut NodeClient, chat_id: &str) -> serde_json::Value {
    for _ in 0..200 {
        let chat = properties(node, chat_id).await["ai-chat-pty"].clone();
        if chat["session_status"] == "ended" {
            return chat;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the session on {chat_id} was never recorded as ended");
}

#[tokio::test]
async fn a_session_launched_for_a_project_runs_in_its_folder_against_its_database() {
    let daemon = spawn_daemon().await;
    let mut node = daemon.node().await;
    let mut session = daemon.session().await;

    let project_id = create(&mut node, "project", "Widgets", "").await;
    let task_id = create(&mut node, "task", "Add the gauge", "").await;
    let chat_id = create(
        &mut node,
        "ai-chat-pty",
        "Session",
        r#"{"agent":"claude-code"}"#,
    )
    .await;
    write_agent(&daemon.home.path().join("bin"), &task_id);
    let checkout = daemon.checkout();

    // The project has no folder on this machine yet, and the launch names none.
    let refused = session
        .launch_session(launch_for(&project_id))
        .await
        .expect_err("a project with no folder cannot host a session");
    assert_eq!(refused.code(), Code::FailedPrecondition);
    assert!(refused.message().contains("Widgets"), "{refused}");

    // A launch refused for another reason stores no folder: here, a task
    // that does not exist.
    let unknown_task = session
        .launch_session(LaunchSessionRequest {
            project_folder: Some(checkout.display().to_string()),
            task_id: Some("no-such-task".into()),
            ..launch_for(&project_id)
        })
        .await
        .expect_err("an unknown task is refused");
    assert_eq!(unknown_task.code(), Code::NotFound);
    assert!(properties(&mut node, &project_id).await["project"]["checkout_path"].is_null());

    // A folder that does not exist is refused, and not remembered.
    let missing = session
        .launch_session(LaunchSessionRequest {
            project_folder: Some(checkout.join("nowhere").display().to_string()),
            ..launch_for(&project_id)
        })
        .await
        .expect_err("a folder that does not exist is refused");
    assert_eq!(missing.code(), Code::InvalidArgument);
    assert!(properties(&mut node, &project_id).await["project"]["checkout_path"].is_null());

    let launched = session
        .launch_session(LaunchSessionRequest {
            node_id: Some(chat_id.clone()),
            project_folder: Some(checkout.display().to_string()),
            task_id: Some(task_id.clone()),
            ..launch_for(&project_id)
        })
        .await
        .expect("launch for the project")
        .into_inner();
    assert_eq!(Path::new(&launched.working_dir), checkout);

    let output = run_agent(&mut session, &launched.session_id).await;

    // It runs in the project's folder, and nothing was written there.
    assert!(
        output.contains(&format!("CWD={}", checkout.display())),
        "{output}"
    );
    assert_eq!(std::fs::read_dir(&checkout).unwrap().count(), 0);
    // Its environment names the task it was launched for, and the database.
    assert!(
        output.contains(&format!("LAUNCHED_FOR={task_id}")),
        "{output}"
    );
    assert!(
        output.contains(&format!("DATABASE={}", daemon.second_id)),
        "{output}"
    );
    // A `nodespace` command with no flags reads that database: the task
    // exists in the second database and nowhere else.
    assert!(output.contains("GET-EXIT=0"), "{output}");
    assert!(output.contains("Add the gauge"), "{output}");
    assert!(output.contains("REPORT-EXIT=0"), "{output}");

    // The folder is remembered on the project: the next launch names none.
    assert_eq!(
        properties(&mut node, &project_id).await["project"]["checkout_path"],
        checkout.display().to_string()
    );
    // The agent's PATH is the search path it was found on, so it can run what
    // detection saw.
    let bin = daemon.home.path().join("bin");
    assert!(
        output.contains(&format!("PATH={}:", bin.display())),
        "{output}"
    );

    let again = session
        .launch_session(launch_for(&project_id))
        .await
        .expect("the remembered folder is used")
        .into_inner();
    session
        .terminate_session(TerminateSessionRequest {
            session_id: again.session_id.clone(),
        })
        .await
        .expect("end the second session");
    assert_eq!(Path::new(&again.working_dir), checkout);

    // The session's end records the id the agent reported for itself.
    send_line(&mut session, &launched.session_id).await;
    let chat = ended_chat(&mut node, &chat_id).await;
    assert_eq!(
        chat["session_id"],
        format!("harness-{}", launched.session_id)
    );
}

/// Sessions in one project share a working directory, so the directory says
/// nothing about which conversation is whose: each session's own report does.
#[tokio::test]
async fn two_sessions_in_one_project_each_record_their_own_harness_session() {
    let daemon = spawn_daemon().await;
    let mut node = daemon.node().await;
    let mut session = daemon.session().await;

    let project_id = create(&mut node, "project", "Widgets", "").await;
    let task_id = create(&mut node, "task", "Add the gauge", "").await;
    write_agent(&daemon.home.path().join("bin"), &task_id);
    let checkout = daemon.checkout();

    let mut launched = Vec::new();
    for title in ["First", "Second"] {
        let chat_id = create(
            &mut node,
            "ai-chat-pty",
            title,
            r#"{"agent":"claude-code"}"#,
        )
        .await;
        let response = session
            .launch_session(LaunchSessionRequest {
                node_id: Some(chat_id.clone()),
                project_folder: Some(checkout.display().to_string()),
                ..launch_for(&project_id)
            })
            .await
            .expect("launch")
            .into_inner();
        assert_eq!(Path::new(&response.working_dir), checkout);
        launched.push((chat_id, response.session_id));
    }

    // Both are running in the one folder before either reports or ends.
    for (chat_id, session_id) in &launched {
        let output = run_agent(&mut session, session_id).await;
        // With no task named, the session is launched for its chat node.
        assert!(
            output.contains(&format!("LAUNCHED_FOR={chat_id}")),
            "{output}"
        );
    }
    for (_, session_id) in &launched {
        send_line(&mut session, session_id).await;
    }

    for (chat_id, session_id) in &launched {
        let chat = ended_chat(&mut node, chat_id).await;
        assert_eq!(chat["session_id"], format!("harness-{session_id}"));
    }
}

#[tokio::test]
async fn a_launch_with_no_project_runs_in_a_private_folder_with_the_environment_set() {
    let daemon = spawn_daemon().await;
    let mut node = daemon.node().await;
    let mut session = daemon.session().await;
    let task_id = create(&mut node, "task", "Add the gauge", "").await;
    write_agent(&daemon.home.path().join("bin"), &task_id);

    // A folder is a project's: naming one without a project is refused.
    let refused = session
        .launch_session(LaunchSessionRequest {
            project_id: None,
            project_folder: Some(daemon.checkout().display().to_string()),
            ..launch_for("")
        })
        .await
        .expect_err("a folder needs a project");
    assert_eq!(refused.code(), Code::InvalidArgument);

    let launched = session
        .launch_session(LaunchSessionRequest {
            project_id: None,
            ..launch_for("")
        })
        .await
        .expect("launch with no project")
        .into_inner();
    // Under the daemon's own home, not the home of whoever runs the test.
    let working_dir = PathBuf::from(&launched.working_dir);
    assert_eq!(
        working_dir,
        daemon
            .home
            .path()
            .join("nodespace-home")
            .join(".nodespace")
            .join("agent-sessions")
            .join(&launched.session_id)
    );

    let output = run_agent(&mut session, &launched.session_id).await;
    assert!(
        output.contains(&format!("DATABASE={}", daemon.second_id)),
        "{output}"
    );
    assert!(output.contains("GET-EXIT=0"), "{output}");
    // The session is told the socket this daemon serves.
    assert!(
        output.contains(&format!(
            "SOCKET={}\r\n",
            daemon.home.path().join("daemon.sock").display()
        )),
        "{output}"
    );
    // Nothing was launched for, so the variable is absent, not empty.
    assert!(output.contains("LAUNCHED_FOR=unset\r\n"), "{output}");
    // No context file and no copy of the skill are written.
    assert_eq!(std::fs::read_dir(&working_dir).unwrap().count(), 0);

    send_line(&mut session, &launched.session_id).await;
}

/// The stored folder can be written by any update of the project, so a launch
/// checks it as it checks one it is handed.
#[tokio::test]
async fn a_stored_folder_that_is_not_absolute_refuses_the_launch() {
    let daemon = spawn_daemon().await;
    let mut node = daemon.node().await;
    let mut session = daemon.session().await;
    let project_id = create(&mut node, "project", "Widgets", "").await;
    write_agent(&daemon.home.path().join("bin"), "unused");

    node.update_node(UpdateNodeRequest {
        node_id: project_id.clone(),
        properties: Some(r#"{"checkout_path":"."}"#.into()),
        ..Default::default()
    })
    .await
    .expect("store a relative folder");

    let refused = session
        .launch_session(launch_for(&project_id))
        .await
        .expect_err("a relative folder is not where a session runs");
    assert_eq!(refused.code(), Code::FailedPrecondition);
    assert!(refused.message().contains("absolute path"), "{refused}");
}
