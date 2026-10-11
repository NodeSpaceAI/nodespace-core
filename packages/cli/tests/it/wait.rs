//! `nodespace wait` against an in-process daemon: the condition met later,
//! already met, timeout, a target that can no longer reach it, and a daemon
//! that is not there or goes away.
//!
//! Each test that needs "later" writes only after the wait printed a status
//! line, so the write is known to arrive while the wait is parked on the
//! change stream and not before its first check.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use nodespace_cli::commands::wait::{
    self, Met, NodeWaitArgs, Outcome, QueryChange, QueryWaitArgs, Target, Timing, TimingArgs,
    WaitAction, WaitEnded,
};
use nodespace_cli::{connect, Cli, DatabaseIdInterceptor, NodeClient};
use nodespace_core::{NodeService as CoreNodeService, SqliteStore};
use nodespace_daemon::nodespace::{CreateNodeRequest, DeleteNodeRequest, UpdateNodeRequest};
use nodespace_daemon::{NodeServiceImpl, NodeServiceServer};
use tempfile::TempDir;
use tokio::net::UnixListener;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;

/// A daemon that can be stopped dead and started again on the same socket
/// and the same database, as a restart is.
///
/// The server runs on a runtime of its own. Ending a process closes every
/// connection it holds, open streams included, and shutting that runtime
/// down is what does the same here: aborting the serving task alone leaves
/// its connections, each a task of its own, open.
struct Daemon {
    sock: PathBuf,
    node_service: Arc<CoreNodeService>,
    server: Option<Runtime>,
    _tempdir: TempDir,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Daemon {
    async fn start() -> Self {
        let tempdir = TempDir::new().expect("tempdir");
        let sock = tempdir.path().join("test-daemon.sock");
        let mut store = Arc::new(
            SqliteStore::new(tempdir.path().join("daemon-db"))
                .await
                .expect("open store"),
        );
        let node_service = Arc::new(
            CoreNodeService::new(&mut store)
                .await
                .expect("build NodeService"),
        );
        let server = Some(serve(&sock, node_service.clone()).await);
        Self {
            sock,
            node_service,
            server,
            _tempdir: tempdir,
        }
    }

    /// Stop serving at once, open streams included, and remove the socket.
    fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            server.shutdown_background();
        }
        let _ = std::fs::remove_file(&self.sock);
    }

    async fn restart(&mut self) {
        self.server = Some(serve(&self.sock, self.node_service.clone()).await);
    }

    async fn client(&self) -> NodeClient {
        connect(&self.sock, DatabaseIdInterceptor::none())
            .await
            .expect("connect")
    }
}

async fn serve(sock: &Path, node_service: Arc<CoreNodeService>) -> Runtime {
    let service = NodeServiceImpl::new(
        node_service,
        Arc::new(tokio::sync::RwLock::new(None)),
        Arc::new(nodespace_core::services::EmbeddingScheduler::new()),
    );
    let server = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("server runtime");
    let path = sock.to_path_buf();
    server.spawn(async move {
        let incoming = UnixListenerStream::new(UnixListener::bind(&path).expect("bind socket"));
        Server::builder()
            .add_service(NodeServiceServer::new(service))
            .serve_with_incoming(incoming)
            .await
            .expect("server crashed");
    });
    for _ in 0..200 {
        if connect(sock, DatabaseIdInterceptor::none()).await.is_ok() {
            return server;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("daemon did not start on {}", sock.display());
}

async fn create(
    client: &mut NodeClient,
    node_type: &str,
    content: &str,
    properties: &str,
) -> String {
    client
        .create_node(CreateNodeRequest {
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
        .expect("create node")
        .into_inner()
        .node_id
}

async fn task(client: &mut NodeClient, status: &str) -> String {
    let properties = serde_json::json!({ "status": status }).to_string();
    create(client, "task", "a task", &properties).await
}

async fn set_status(client: &mut NodeClient, id: &str, status: &str) {
    client
        .update_node(UpdateNodeRequest {
            node_id: id.into(),
            properties: Some(serde_json::json!({ "status": status }).to_string()),
            ..UpdateNodeRequest::default()
        })
        .await
        .expect("set status");
}

async fn delete(client: &mut NodeClient, id: &str) {
    client
        .delete_node(DeleteNodeRequest {
            node_id: id.into(),
            ..DeleteNodeRequest::default()
        })
        .await
        .expect("delete node");
}

/// A saved query matching every task that is not done.
async fn not_done_query(client: &mut NodeClient) -> String {
    let definition = serde_json::json!({
        "target_type": "task",
        "filters": [{
            "type": "property", "operator": "equals", "property": "status",
            "value": "done", "negate": true
        }]
    });
    create(client, "query", "Not done", &definition.to_string()).await
}

fn status_is(status: &str) -> String {
    serde_json::json!([{
        "type": "property", "operator": "equals", "property": "status", "value": status
    }])
    .to_string()
}

/// "Its review has an outcome": the status is no longer `in_review`.
fn left_review() -> String {
    serde_json::json!([{
        "type": "property", "operator": "equals", "property": "status",
        "value": "in_review", "negate": true
    }])
    .to_string()
}

fn timing_args() -> TimingArgs {
    TimingArgs {
        timeout: Duration::from_secs(60),
        interval: Duration::from_secs(60),
    }
}

/// A wait that ends on its condition, never on its clock: a generous
/// timeout, and a status line often enough to pace the test by.
fn patient() -> Timing {
    Timing {
        timeout: Duration::from_secs(60),
        interval: Duration::from_millis(50),
        reconnect_for: Duration::from_secs(30),
        reconnect_every: Duration::from_millis(50),
    }
}

fn node_target(id: &str, until: &str, unless: Option<&str>) -> Target {
    Target::node(&NodeWaitArgs {
        id: id.into(),
        until: until.into(),
        unless: unless.map(str::to_string),
        timing: timing_args(),
    })
    .expect("filters parse")
}

fn query_target(query: &str, until: QueryChange, item: Option<&str>) -> Target {
    Target::query(&QueryWaitArgs {
        query: query.into(),
        until,
        item: item.map(str::to_string),
        filters: None,
        timing: timing_args(),
    })
}

/// Status lines as the wait writes them, one message per line.
struct Lines(mpsc::UnboundedSender<String>);

impl Write for Lines {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.0.send(String::from_utf8_lossy(buf).into_owned());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A wait running in the background, with the status lines it prints.
struct Waiting {
    outcome: JoinHandle<anyhow::Result<Outcome>>,
    lines: mpsc::UnboundedReceiver<String>,
}

impl Waiting {
    fn start(sock: &Path, mut target: Target, timing: Timing) -> Self {
        let (tx, lines) = mpsc::unbounded_channel();
        let sock = sock.to_path_buf();
        let outcome = tokio::spawn(async move {
            let mut status = Lines(tx);
            wait::watch(
                &sock,
                &DatabaseIdInterceptor::none(),
                &mut target,
                &timing,
                &mut status,
            )
            .await
        });
        Self { outcome, lines }
    }

    /// The next status line containing `needle`.
    async fn line(&mut self, needle: &str) -> String {
        loop {
            let line = tokio::time::timeout(Duration::from_secs(30), self.lines.recv())
                .await
                .expect("a status line within 30s")
                .expect("the wait ended before printing the line");
            if line.contains(needle) {
                return line;
            }
        }
    }

    async fn outcome(self) -> Outcome {
        tokio::time::timeout(Duration::from_secs(30), self.outcome)
            .await
            .expect("the wait ends within 30s")
            .expect("the wait task panicked")
            .expect("the wait failed")
    }
}

fn met_node_status(outcome: Outcome) -> String {
    let Outcome::Met(Met::Node(context)) = outcome else {
        panic!("expected a met node wait, got {outcome:?}");
    };
    let node = context.node.expect("context node").node.expect("node");
    let properties: serde_json::Value = serde_json::from_str(&node.properties).expect("properties");
    properties
        .pointer("/task/status")
        .or_else(|| properties.get("status"))
        .and_then(|status| status.as_str())
        .expect("a status")
        .to_string()
}

#[tokio::test]
async fn a_condition_that_already_holds_returns_at_once() {
    let daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let id = task(&mut client, "done").await;

    let waiting = Waiting::start(
        &daemon.sock,
        node_target(&id, &left_review(), None),
        patient(),
    );
    assert_eq!(met_node_status(waiting.outcome().await), "done");
}

#[tokio::test]
async fn a_node_wait_returns_when_the_condition_is_met_later() {
    let daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let id = task(&mut client, "in_review").await;

    let mut waiting = Waiting::start(
        &daemon.sock,
        node_target(&id, &left_review(), None),
        patient(),
    );
    let line = waiting.line("waiting on").await;
    assert!(line.contains(&format!("node {id}")), "{line}");

    // A change that does not meet the condition leaves it waiting.
    let other = task(&mut client, "open").await;
    set_status(&mut client, &other, "done").await;
    waiting.line("waiting on").await;

    set_status(&mut client, &id, "in_progress").await;
    assert_eq!(met_node_status(waiting.outcome().await), "in_progress");
}

#[tokio::test]
async fn a_wait_times_out_with_its_own_outcome_after_printing_status_lines() {
    let daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let id = task(&mut client, "in_review").await;

    let mut waiting = Waiting::start(
        &daemon.sock,
        node_target(&id, &left_review(), None),
        Timing {
            timeout: Duration::from_millis(400),
            ..patient()
        },
    );
    waiting.line("waiting on").await;
    assert!(matches!(waiting.outcome().await, Outcome::TimedOut));
}

#[tokio::test]
async fn a_node_that_matches_unless_ends_the_wait_as_unreachable() {
    let daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let id = task(&mut client, "in_review").await;

    let mut waiting = Waiting::start(
        &daemon.sock,
        node_target(&id, &left_review(), Some(&status_is("cancelled"))),
        patient(),
    );
    waiting.line("waiting on").await;
    set_status(&mut client, &id, "cancelled").await;

    let outcome = waiting.outcome().await;
    let Outcome::Unreachable(reason) = outcome else {
        panic!("expected unreachable, got {outcome:?}");
    };
    assert_eq!(reason, format!("node {id} matches --unless"));
}

#[tokio::test]
async fn a_deleted_or_missing_node_ends_the_wait_as_unreachable() {
    let daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let id = task(&mut client, "in_review").await;

    let mut waiting = Waiting::start(
        &daemon.sock,
        node_target(&id, &left_review(), None),
        patient(),
    );
    waiting.line("waiting on").await;
    delete(&mut client, &id).await;
    let gone = format!("node {id} does not exist");
    assert!(matches!(waiting.outcome().await, Outcome::Unreachable(reason) if reason == gone));

    // Already gone when the wait starts.
    let waiting = Waiting::start(
        &daemon.sock,
        node_target(&id, &left_review(), None),
        patient(),
    );
    assert!(matches!(waiting.outcome().await, Outcome::Unreachable(reason) if reason == gone));
}

#[tokio::test]
async fn a_filter_the_daemon_rejects_fails_the_command() {
    let daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let id = task(&mut client, "open").await;
    let unknown = serde_json::json!([{
        "type": "property", "operator": "between", "property": "status", "value": "open"
    }])
    .to_string();

    let waiting = Waiting::start(&daemon.sock, node_target(&id, &unknown, None), patient());
    let error = waiting
        .outcome
        .await
        .expect("the wait task panicked")
        .expect_err("an unknown operator must fail the command");
    assert!(format!("{error:#}").contains("between"), "{error:#}");
}

#[tokio::test]
async fn a_daemon_that_is_not_running_is_its_own_outcome_and_exit_status() {
    let tempdir = TempDir::new().expect("tempdir");
    let sock = tempdir.path().join("absent.sock");

    let waiting = Waiting::start(
        &sock,
        node_target("some-id", &left_review(), None),
        patient(),
    );
    assert!(matches!(
        waiting.outcome().await,
        Outcome::DaemonUnavailable(_)
    ));

    let error = wait::run(
        &sock,
        None,
        WaitAction::Node(NodeWaitArgs {
            id: "some-id".into(),
            until: left_review(),
            unless: None,
            timing: timing_args(),
        }),
        false,
    )
    .await
    .expect_err("no daemon");
    let ended = error.downcast_ref::<WaitEnded>().expect("a WaitEnded");
    assert_eq!(ended.code, wait::EXIT_DAEMON_UNAVAILABLE);
    assert!(ended.message.contains("Is the daemon running?"), "{ended}");
}

#[tokio::test]
async fn a_daemon_that_goes_away_and_stays_away_ends_the_wait() {
    let mut daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let id = task(&mut client, "in_review").await;

    let mut waiting = Waiting::start(
        &daemon.sock,
        node_target(&id, &left_review(), None),
        Timing {
            reconnect_for: Duration::from_millis(300),
            ..patient()
        },
    );
    waiting.line("waiting on").await;
    daemon.stop();

    waiting.line("the change stream ended").await;
    let outcome = waiting.outcome().await;
    assert!(
        matches!(outcome, Outcome::DaemonUnavailable(_)),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn a_wait_survives_a_daemon_restart_and_sees_what_changed_meanwhile() {
    let mut daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let id = task(&mut client, "in_review").await;

    let mut waiting = Waiting::start(
        &daemon.sock,
        node_target(&id, &left_review(), None),
        Timing {
            // Long enough that the change below lands before the reconnect.
            reconnect_every: Duration::from_millis(750),
            ..patient()
        },
    );
    waiting.line("waiting on").await;
    daemon.stop();
    waiting.line("the change stream ended").await;

    // The change is made while the wait is disconnected, so no event for it
    // ever reaches the wait: only the check after reconnecting can see it.
    daemon.restart().await;
    let mut client = daemon.client().await;
    set_status(&mut client, &id, "done").await;

    assert_eq!(met_node_status(waiting.outcome().await), "done");
}

fn met_query(outcome: Outcome) -> (Vec<String>, Vec<String>, Vec<String>) {
    let Outcome::Met(Met::Query { run, gained, lost }) = outcome else {
        panic!("expected a met query wait, got {outcome:?}");
    };
    let ids = run.nodes.iter().map(|node| node.id.clone()).collect();
    (ids, gained, lost)
}

#[tokio::test]
async fn a_query_wait_returns_when_the_result_gains_or_loses_an_item() {
    let daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let query = not_done_query(&mut client).await;
    let first = task(&mut client, "open").await;

    // Gain: a loss does not end it, the gain does.
    let mut waiting = Waiting::start(
        &daemon.sock,
        query_target("Not done", QueryChange::Gain, None),
        patient(),
    );
    let line = waiting.line("waiting on").await;
    assert!(line.contains("saved query 'Not done'"), "{line}");
    set_status(&mut client, &first, "done").await;
    waiting.line("waiting on").await;
    let second = task(&mut client, "open").await;
    let (ids, gained, lost) = met_query(waiting.outcome().await);
    assert_eq!(ids, vec![second.clone()]);
    assert_eq!(gained, vec![second.clone()]);
    assert_eq!(lost, vec![first]);

    // Loss, by the query's id.
    let mut waiting = Waiting::start(
        &daemon.sock,
        query_target(&query, QueryChange::Loss, None),
        patient(),
    );
    waiting.line("waiting on").await;
    set_status(&mut client, &second, "done").await;
    let (ids, gained, lost) = met_query(waiting.outcome().await);
    assert!(ids.is_empty(), "{ids:?}");
    assert!(gained.is_empty(), "{gained:?}");
    assert_eq!(lost, vec![second]);
}

#[tokio::test]
async fn a_query_wait_on_a_named_item_or_any_item_holds_at_once_or_later() {
    let daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    not_done_query(&mut client).await;
    let id = task(&mut client, "done").await;

    // Empty now: `not-empty` and `--item` both wait.
    let mut any = Waiting::start(
        &daemon.sock,
        query_target("Not done", QueryChange::NotEmpty, None),
        patient(),
    );
    let mut named = Waiting::start(
        &daemon.sock,
        query_target("Not done", QueryChange::Change, Some(&id)),
        patient(),
    );
    any.line("waiting on").await;
    named.line("waiting on").await;
    set_status(&mut client, &id, "open").await;
    assert_eq!(met_query(any.outcome().await).0, vec![id.clone()]);
    assert_eq!(met_query(named.outcome().await).0, vec![id.clone()]);

    // Holding when the wait starts: both return at once.
    for target in [
        query_target("Not done", QueryChange::NotEmpty, None),
        query_target("Not done", QueryChange::Change, Some(&id)),
    ] {
        let waiting = Waiting::start(&daemon.sock, target, patient());
        assert_eq!(met_query(waiting.outcome().await).0, vec![id.clone()]);
    }
}

#[tokio::test]
async fn a_deleted_saved_query_or_item_ends_the_wait_as_unreachable() {
    let daemon = Daemon::start().await;
    let mut client = daemon.client().await;
    let query = not_done_query(&mut client).await;
    let id = task(&mut client, "done").await;

    let mut waiting = Waiting::start(
        &daemon.sock,
        query_target(&query, QueryChange::Change, Some(&id)),
        patient(),
    );
    waiting.line("waiting on").await;
    delete(&mut client, &id).await;
    let gone = format!("node {id} does not exist");
    assert!(matches!(waiting.outcome().await, Outcome::Unreachable(reason) if reason == gone));

    let mut waiting = Waiting::start(
        &daemon.sock,
        query_target(&query, QueryChange::Change, None),
        patient(),
    );
    waiting.line("waiting on").await;
    delete(&mut client, &query).await;
    let outcome = waiting.outcome().await;
    let Outcome::Unreachable(reason) = outcome else {
        panic!("expected unreachable, got {outcome:?}");
    };
    assert_eq!(reason, format!("saved query '{query}' does not exist"));
}

#[test]
fn the_wait_commands_parse_with_their_conditions_and_durations() {
    let parse = |argv: &[&str]| Cli::try_parse_from(argv.iter().copied());

    let cli = parse(&[
        "nodespace",
        "wait",
        "node",
        "abc",
        "--until",
        "[]",
        "--unless",
        "[]",
        "--timeout",
        "30m",
        "--interval",
        "15",
    ])
    .expect("a node wait parses");
    let nodespace_cli::Command::Wait {
        action: WaitAction::Node(args),
    } = cli.command
    else {
        panic!("expected a node wait");
    };
    assert_eq!(args.timing.timeout, Duration::from_secs(1800));
    assert_eq!(args.timing.interval, Duration::from_secs(15));

    let cli = parse(&["nodespace", "wait", "query", "Ready tasks"]).expect("a query wait parses");
    let nodespace_cli::Command::Wait {
        action: WaitAction::Query(args),
    } = cli.command
    else {
        panic!("expected a query wait");
    };
    assert_eq!(args.until, QueryChange::Change);
    assert_eq!(args.timing.timeout, Duration::from_secs(3 * 3600));
    assert_eq!(args.timing.interval, Duration::from_secs(60));

    parse(&[
        "nodespace",
        "wait",
        "query",
        "Ready tasks",
        "--until",
        "not-empty",
    ])
    .expect("not-empty parses");
    parse(&["nodespace", "wait", "query", "Ready tasks", "--item", "abc"]).expect("--item parses");
    assert!(
        parse(&[
            "nodespace",
            "wait",
            "query",
            "q",
            "--item",
            "abc",
            "--until",
            "gain"
        ])
        .is_err(),
        "--item replaces --until"
    );
    assert!(
        parse(&["nodespace", "wait", "node", "abc"]).is_err(),
        "a node wait needs its condition"
    );
    assert!(
        parse(&[
            "nodespace",
            "wait",
            "node",
            "abc",
            "--until",
            "[]",
            "--timeout",
            "0"
        ])
        .is_err(),
        "a zero timeout is refused"
    );
}
